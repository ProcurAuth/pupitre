#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""A throwaway fake of the Pupitre server, for the work on the page. A development tool: it is not part of the product.

It serves the files of `web/` as the server of 5.23 does (the shell with the markers, the static files below
/static/<build>/, the headers of 7.5) and a small in-memory version of the API of section 5 with synthetic data. It uses
the Python standard library only. It listens on 127.0.0.1 and has no persistence.

Use:
    python3 -I tests/page/fake_api.py [--port 8099] [--base-path /] [--mode open|authzen] [--no-runner] [--no-apply]
                                      [--slow-ms 0] [--max-document-bytes 262144] [--fixture standard|empty|big]

Control routes for the tests (below /__fake/): POST /__fake/reset, POST /__fake/deny with {"actions": [...]},
POST /__fake/session with {"subject": "..."}, GET /__fake/log (the writes that the server saw).
"""

import argparse
import base64
import hashlib
import json
import os
import re
import sys
import threading
import time
import urllib.parse
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
WEB = os.path.join(ROOT, "web")
ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,62}$")
LABEL_RE = re.compile(r"^[a-z][a-z0-9_]{0,62}$")
KEY_RE = re.compile(r"^[a-z][a-z0-9_]{0,62}(\.[a-z][a-z0-9_]{0,62}){0,7}$")
RUN_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}$")

CSP = ("default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; "
       "base-uri 'self'; form-action 'self'; frame-ancestors 'none'")
SECURITY_HEADERS = {
    "Content-Security-Policy": CSP,
    "X-Content-Type-Options": "nosniff",
    "X-Frame-Options": "DENY",
    "Referrer-Policy": "same-origin",
    "Cross-Origin-Resource-Policy": "same-origin",
    "Cross-Origin-Opener-Policy": "same-origin",
    "Permissions-Policy": "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
}
CONTENT_TYPES = {".css": "text/css; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".svg": "image/svg+xml",
                 ".woff2": "font/woff2", "": "text/plain; charset=utf-8"}


# --- the canonical form (2.4) --------------------------------------------------------------------------------------------

def canon(value, depth=0):
    pad, end = "  " * (depth + 1), "  " * depth
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, list):
        if not value:
            return "[]"
        return "[\n" + ",\n".join(pad + canon(v, depth + 1) for v in value) + "\n" + end + "]"
    if not value:
        return "{}"
    names = sorted(value, key=lambda s: s.encode())
    return "{\n" + ",\n".join(pad + json.dumps(n, ensure_ascii=False) + ": " + canon(value[n], depth + 1) for n in names) + "\n" + end + "}"


def canonical_text(doc):
    return canon(doc) + "\n"


def etag_of(text):
    return hashlib.sha256(text.encode()).hexdigest()


def now_iso(t=None):
    t = time.time() if t is None else t
    return time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(t)) + ".%03dZ" % int((t % 1) * 1000)


# --- the catalog (4.2, 5.15) -------------------------------------------------------------------------------------------------

def key(path, kind, desc, **kw):
    d = {"path": path, "kind": kind, "description": desc}
    d.update(kw)
    return d


def relay_keys():
    keys = [
        key("limits.max_clients", "integer", "The most clients at one time.", min=1, max=100000, default=100),
        key("limits.max_idle", "duration", "The longest idle time of a client.", min="1s", max="1h", default="30s"),
        key("limits.max_message", "size", "The largest message in bytes.", min=1024, max=16777216, default=1048576),
        key("log.format", "enum", "The format of the log.", values=["text", "json"], default="text"),
        key("log.level", "enum", "The lowest level of the log.", values=["error", "warn", "info", "debug"], default="info"),
        key("net.listen", "socket_addr", "The address of the listener."),
        key("net.public_name", "text", "The name that the clients use.", min=1, max=253, rule="dns_name"),
        key("net.tls", "bool", "Use TLS on the listener.", default=True),
        key("storage.password", "text", "The password of the database account.", min=8, max=1024, rule="any", secret=True),
        key("storage.url", "text", "The address of the database.", min=1, max=512, rule="no_control"),
        key("tls.ca_file", "path", "The file of the CA."),
        key("upstream.hosts", "text_list", "The hosts to forward to.", min_items=0, max_items=16, item_min=1, item_max=253, rule="host_or_ip"),
        key("upstream.token", "text", "The token for the upstream.", min=16, max=256, rule="no_control_no_space", secret=True),
    ]
    return sorted(keys, key=lambda k: k["path"].encode())


def feed_keys():
    return sorted([
        key("batch.size", "integer", "The count of records in a batch.", min=1, max=10000, default=500),
        key("log.level", "enum", "The lowest level of the log.", values=["warn", "info", "debug"], default="info"),
        key("source.url", "text", "The address of the source.", min=1, max=512, rule="no_control"),
        key("source.password", "text", "The password of the source.", min=8, max=256, rule="any", secret=True),
    ], key=lambda k: k["path"].encode())


def gateway_keys():
    return sorted([
        key("limits.max_clients", "integer", "The most clients at one time.", min=1, max=5000, default=50),
        key("routes.default", "text", "The default route.", min=1, max=64, rule="printable_ascii", default="main"),
    ], key=lambda k: k["path"].encode())


def bulk_keys(n=400):
    keys = []
    for i in range(n):
        seg = "svc%03d" % i
        keys.append(key("%s.limit" % seg, "integer", "A limit of the service %d." % i, min=0, max=100000, default=i))
        if i % 4 == 0:
            keys.append(key("%s.mode" % seg, "enum", "The mode of the service %d." % i, values=["off", "slow", "fast"], default="slow"))
        if i % 50 == 0:
            keys.append(key("%s.secret" % seg, "text", "A secret of the service %d." % i, min=8, max=128, rule="any", secret=True))
    return sorted(keys, key=lambda k: k["path"].encode())


SCHEMAS = {
    "relay": {"id": "relay", "group": "relay_hosts", "variable": "relay_config", "keys": relay_keys()},
    "feed": {"id": "feed", "group": "feed_hosts", "variable": "feed_config", "keys": feed_keys()},
    "gateway": {"id": "gateway", "group": "gateway_hosts", "variable": "gateway_config", "keys": gateway_keys()},
    "bulk": {"id": "bulk", "group": "bulk_hosts", "variable": "bulk_config", "keys": bulk_keys()},
}

PLAYBOOKS = [
    {"id": "deploy-relay", "kind": "deploy"}, {"id": "render-relay", "kind": "render"}, {"id": "status-relay", "kind": "status"},
    {"id": "deploy-feed", "kind": "deploy"}, {"id": "status-feed", "kind": "status"},
    {"id": "deploy-gateway", "kind": "deploy"}, {"id": "deploy-bulk", "kind": "deploy"},
    {"id": "restart-service", "kind": "command"}, {"id": "reload-config", "kind": "command"}, {"id": "vacuum", "kind": "command"},
    {"id": "rotate-logs", "kind": "command"},
]
COMMANDS = [
    {"id": "restart-service", "label": "Restart the service", "playbook": "restart-service", "target": "unit", "check": False,
     "params": [{"name": "grace", "label": "Grace period", "type": "duration", "required": True, "min_duration": "1s", "max_duration": "5m"}]},
    {"id": "reload-config", "label": "Reload the configuration", "playbook": "reload-config", "target": "unit", "check": True, "params": []},
    {"id": "vacuum", "label": "Clean the old data of a host", "playbook": "vacuum", "target": "host", "check": False,
     "params": [{"name": "older_than", "label": "Older than, in days", "type": "integer", "required": False, "min": 1, "max": 365},
                {"name": "mode", "label": "Mode", "type": "choice", "required": True, "choices": ["fast", "full"]},
                {"name": "reason", "label": "Reason", "type": "text", "required": False, "min": 3, "max": 80},
                {"name": "dry", "label": "Dry run", "type": "boolean", "required": True}]},
    {"id": "rotate-logs", "label": "Rotate the logs", "playbook": "rotate-logs", "target": "environment", "check": False, "params": []},
]


def schema_for_catalog(s):
    return {"id": s["id"], "group": s["group"], "variable": s["variable"], "keys": s["keys"]}


# --- value checks (2.12), the same rules as the page ---------------------------------------------------------------------------

def dns_name(t):
    return 0 < len(t) <= 253 and all(re.fullmatch(r"[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?", l) for l in t.split("."))


def is_ip(t):
    import ipaddress
    try:
        ipaddress.ip_address(t)
        return True
    except ValueError:
        return False


def rule_ok(rule, t):
    ctl = any(ord(c) < 32 or 127 <= ord(c) < 160 for c in t)
    return {"any": True, None: True, "no_control": not ctl, "printable_ascii": all(32 <= ord(c) < 127 for c in t),
            "no_control_no_space": not ctl and " " not in t, "dns_name": dns_name(t), "host_or_ip": dns_name(t) or is_ip(t)}.get(rule, False)


def parse_duration(t):
    m = re.fullmatch(r"(0|[1-9][0-9]*)(ms|s|m|h)", t) if isinstance(t, str) else None
    return int(m.group(1)) * {"ms": 1, "s": 1000, "m": 60000, "h": 3600000}[m.group(2)] if m else None


def check_value(d, v):
    k = d["kind"]
    isint = isinstance(v, int) and not isinstance(v, bool) and 0 <= v <= 9007199254740991
    if k == "text":
        ok = isinstance(v, str) and "\0" not in v and d["min"] <= len(v.encode()) <= d["max"] and rule_ok(d.get("rule"), v)
    elif k == "text_list":
        ok = isinstance(v, list) and d["min_items"] <= len(v) <= d["max_items"] and all(
            isinstance(i, str) and d["item_min"] <= len(i.encode()) <= d["item_max"] and rule_ok(d.get("rule"), i) for i in v)
    elif k == "enum":
        ok = v in d["values"]
    elif k in ("integer", "size"):
        ok = isint and d["min"] <= v <= d["max"]
    elif k == "duration":
        ms = parse_duration(v)
        ok = ms is not None and parse_duration(d["min"]) <= ms <= parse_duration(d["max"])
    elif k == "bool":
        ok = isinstance(v, bool)
    elif k == "path":
        ok = isinstance(v, str) and v.startswith("/") and len(v.encode()) <= 4096
    elif k == "socket_addr":
        ok = isinstance(v, str) and re.fullmatch(r"(\d{1,3}(\.\d{1,3}){3}|\[[0-9a-f:]+\]):\d{1,5}", v) is not None
    else:
        ok = False
    return None if ok else "expected a valid %s, found %s" % (k, json.dumps(v))


# --- the store ---------------------------------------------------------------------------------------------------------------------

class Fail(Exception):
    def __init__(self, status, code, message=None, **extra):
        self.status, self.code, self.message, self.extra = status, code, message, extra


MESSAGES = {
    "bad_request": "The request does not have the shape of its route.", "unauthenticated": "The identity token is not valid.",
    "forbidden": "You do not have the right to do this.", "not_found": "The item does not exist.",
    "method_not_allowed": "The method is not valid for this route.", "conflict": "The document changed after your read.",
    "already_exists": "The document exists.", "in_use": "Other items use this item.", "not_empty": "The item is not empty.",
    "check_required": "The apply needs a check that passed.", "check_used": "An apply used this check.", "check_too_old": "The check is too old.",
    "run_finished": "The run is finished.", "invalid_document": "The document is not valid.", "invalid_secret": "The secret is not valid.",
    "invalid_run_request": "The run request is not valid.", "limit_exceeded": "A limit of the store stops this write.",
    "precondition_required": "The write needs a precondition.", "ceiling_exceeded": "The run has too many hosts.",
    "apply_disabled": "Apply and commands are off in this mode.", "cross_site_request": "The request comes from another site.",
    "unsupported_media_type": "The body is not JSON.", "run_queue_full": "The queue is full.",
}


class Store:
    def __init__(self, fixture="standard"):
        self.lock = threading.RLock()
        self.docs = {}      # key -> {"doc", "text", "etag", "version"}
        self.hist = {}      # key -> [version dicts]
        self.counter = {}   # key -> last version number
        self.secrets = {}   # (scope, name) -> dict
        self.runs = {}
        self.run_order = []
        self.log = []
        self.denied = set()
        self.inject = []    # [{"match": text, "method": text, "status": n, "error": code, "times": n}]
        self.subject = "alice"
        self.load(fixture)

    # documents
    def get(self, k):
        return self.docs.get(k)

    def put(self, k, doc, precond, by, message="", kind=None):
        with self.lock:
            cur = self.docs.get(k)
            if precond == "create":
                if cur:
                    raise Fail(409, "already_exists")
            else:
                if not cur:
                    raise Fail(404, "not_found")
                if cur["etag"] != precond:
                    raise Fail(409, "conflict", etag=cur["etag"])
            text = canonical_text(doc)
            self.counter[k] = self.counter.get(k, 0) + 1
            v = self.counter[k]
            rec = {"doc": doc, "text": text, "etag": etag_of(text), "version": v}
            self.docs[k] = rec
            self.hist.setdefault(k, []).append({"version": v, "kind": kind or ("create" if precond == "create" else "save"), "saved_by": by,
                                                "saved_at": now_iso(), "message": message, "etag": rec["etag"], "text": text})
            return rec

    def delete(self, k, etag, by):
        with self.lock:
            cur = self.docs.get(k)
            if not cur:
                raise Fail(404, "not_found")
            if cur["etag"] != etag:
                raise Fail(409, "conflict", etag=cur["etag"])
            self.counter[k] = self.counter.get(k, 0) + 1
            del self.docs[k]
            self.hist.setdefault(k, []).append({"version": self.counter[k], "kind": "delete", "saved_by": by, "saved_at": now_iso(), "message": "", "etag": None, "text": None})

    def seed(self, k, doc, message="", by="import", kind="import", versions=1):
        rec = self.put(k, doc, "create", by, message, kind)
        return rec

    def load_big(self):
        """The sizes of 1.8: 64 environments, 1,000 hosts and 300 units in one environment, 100 hosts in one unit, a schema of 500 keys."""
        for i in range(63):
            self.seed("environments/env-%03d" % i, {"format": 1, "label": "Environment %03d" % i})
        self.seed("environments/large", {"format": 1, "label": "Large"})
        for i in range(1000):
            self.seed("environments/large/hosts/h-%04d" % i, {"format": 1, "address": "h-%04d.example.invalid" % i, "labels": ["region_eu"] if i % 2 else []})
        schemas = ["relay", "feed", "gateway"]
        for i in range(299):
            sc = schemas[i % 3]
            hosts = sorted("h-%04d" % ((i * 3 + k) % 1000) for k in range(3))
            self.seed("environments/large/units/u-%04d" % i, {"format": 1, "label": "Unit %04d" % i, "schema": sc, "hosts": hosts,
                                                              "playbooks": {"deploy": "deploy-%s" % sc}})
        self.seed("environments/large/units/bulk", {"format": 1, "label": "Bulk", "schema": "bulk", "hosts": sorted("h-%04d" % i for i in range(100)), "playbooks": {"deploy": "deploy-bulk"}})
        now = time.time()
        for i in range(600):
            self.add_history_run(now - 60 * (i + 2), "passed" if i % 5 else "failed", "deploy", "check", "large", "bulk")

    def load(self, fixture):
        if fixture == "empty":
            return
        if fixture == "big":
            return self.load_big()
        s = self.seed
        s("environments/production", {"format": 1, "label": "Production"}, "First import")
        s("environments/staging", {"format": 1, "label": "Staging"}, "First import")
        s("environments/sandbox-platform", {"format": 1, "label": "Sandbox of the platform team"})
        for e, names in (("production", ["host-a", "host-b", "host-c", "host-d", "host-e", "host-f"]), ("staging", ["stg-a", "stg-b"])):
            for i, h in enumerate(names):
                labels = ["region_eu"] + (["canary"] if i == 0 else [])
                doc = {"format": 1, "address": ("%s.example.invalid" % h) if i % 2 == 0 else "203.0.113.%d" % (10 + i), "labels": sorted(labels)}
                if i == 1:
                    doc["port"] = 2222
                s("environments/%s/hosts/%s" % (e, h), doc)
        s("environments/production/common", {"format": 1, "values": {"log.level": "debug", "limits.max_idle": "2m"}, "notes": {"log.level": "Debug for the incident of 2026-11-02."}})
        s("environments/production/units/relay", {"format": 1, "label": "Relay", "schema": "relay", "hosts": ["host-a", "host-b", "host-c"],
                                                   "playbooks": {"deploy": "deploy-relay", "render": "render-relay", "status": "status-relay"}})
        s("environments/production/units/feed", {"format": 1, "label": "Feed ingest", "schema": "feed", "hosts": ["host-d", "host-e"],
                                                  "playbooks": {"deploy": "deploy-feed", "status": "status-feed"}})
        s("environments/production/units/gateway", {"format": 1, "label": "Metrics gateway", "schema": "gateway", "hosts": ["host-a", "host-b", "host-d", "host-e"],
                                                     "playbooks": {"deploy": "deploy-gateway"}})
        s("environments/production/units/relay/values", {"format": 1, "values": {"limits.max_clients": 300, "log.format": "json", "net.listen": "0.0.0.0:7443"},
                                                          "notes": {"limits.max_clients": "Raised for the match of 2026-11-30."}})
        s("environments/production/units/relay/hosts/host-a/values", {"format": 1, "values": {"limits.max_clients": 500}})
        s("environments/staging/units/relay", {"format": 1, "label": "Relay", "schema": "relay", "hosts": ["stg-a"], "playbooks": {"deploy": "deploy-relay"}})
        # more versions of the values of the unit relay, for the history
        k = "environments/production/units/relay/values"
        for n in (2, 3, 4):
            cur = self.docs[k]
            doc = json.loads(json.dumps(cur["doc"]))
            doc["values"]["limits.max_clients"] = 100 * n
            self.put(k, doc, cur["etag"], "alice" if n % 2 else "bob", "Change %d of the limit" % n, "save")
        cur = self.docs[k]
        doc = json.loads(json.dumps(cur["doc"]))
        doc["values"]["limits.max_clients"] = 300
        self.put(k, doc, cur["etag"], "alice", "Back to 300", "save")
        self.set_secret("environment:production/unit:relay", "storage.password", "s3cret-value-1", "alice")
        self.set_secret("environment:production", "upstream.token", "tok-0123456789abcdef", "alice")
        now = time.time()
        for i, (status, kind, mode) in enumerate((("passed", "deploy", "check"), ("failed", "deploy", "check"), ("passed", "status", "apply"), ("canceled", "command", "apply"))):
            self.add_history_run(now - 3600 * (i + 2), status, kind, mode, "production", "relay")

    def add_history_run(self, t, status, kind, mode, env, unit):
        rid = new_run_id(t)
        rec = {"format": 1, "id": rid, "kind": kind, "mode": mode, "environment": env, "node": "environment:%s/unit:%s" % (env, unit),
               "hosts": ["host-a", "host-b"], "playbook": "deploy-relay" if kind == "deploy" else "status-relay", "subject": "bob",
               "created_at": now_iso(t), "started_at": now_iso(t + 1), "finished_at": now_iso(t + 9), "status": status, "snapshot": hashlib.sha256(rid.encode()).hexdigest()[:64]}
        if kind == "command":
            rec.update({"command": "restart-service", "params": {"grace": "10s"}})
        if status == "failed":
            rec["reason"] = "exit_code"
            rec["exit_code"] = 2
        if status == "canceled":
            rec["cancel"] = {"by": "alice", "at": now_iso(t + 4)}
        self.runs[rid] = {"record": rec, "static": True}
        self.run_order.append(rid)

    # secrets
    def set_secret(self, scope, name, value, by):
        self.secrets[(scope, name)] = {"value": value, "updated_at": now_iso(), "updated_by": by}

    def fingerprint(self, scope, name):
        s = self.secrets.get((scope, name))
        return hashlib.sha256(("%s\n%s\n%s" % (scope, name, s["value"])).encode()).hexdigest()[:16] if s else None


_run_counter = [0]


def new_run_id(t=None):
    t = time.time() if t is None else t
    _run_counter[0] += 1
    ms = int(t * 1000) & 0xFFFFFFFFFFFF
    h = "%012x" % ms
    rnd = uuid.uuid4().hex
    return "%s-%s-7%s-8%s-%s" % (h[:8], h[8:], rnd[:3], rnd[3:6], rnd[6:18])


# --- the server ---------------------------------------------------------------------------------------------------------------------

class State:
    def __init__(self, args):
        self.args = args
        self.store = Store(args.fixture)
        self.base = args.base_path
        self.max_page = 200
        files = []
        for dirpath, _, names in os.walk(WEB):
            for n in names:
                full = os.path.join(dirpath, n)
                rel = os.path.relpath(full, WEB).replace(os.sep, "/")
                if rel != "shell.html":
                    files.append(rel)
        files.sort(key=lambda s: s.encode())
        h = hashlib.sha256()
        for rel in files:
            data = open(os.path.join(WEB, rel), "rb").read()
            h.update(rel.encode() + b"\n" + str(len(data)).encode() + b"\n" + data)
        self.build = h.hexdigest()[:12]
        self.assets = files

    def shell(self):
        text = open(os.path.join(WEB, "shell.html"), encoding="utf-8").read()
        return text.replace("{{base_path}}", self.base).replace("{{build}}", self.build).encode()


def b64(n):
    return base64.urlsafe_b64encode(str(n).encode()).decode().rstrip("=")


def unb64(s):
    try:
        return int(base64.urlsafe_b64decode(s + "=" * (-len(s) % 4)).decode())
    except Exception:
        raise Fail(400, "bad_request")


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "fake-pupitre"
    state = None

    def log_message(self, fmt, *args):
        if os.environ.get("FAKE_VERBOSE"):
            sys.stderr.write("%s %s\n" % (self.command, self.path))

    # --- plumbing
    def send_json(self, status, body, headers=None):
        data = json.dumps(body, separators=(",", ":"), ensure_ascii=False).encode()
        self.send_bytes(status, data, "application/json; charset=utf-8", headers, api=True)

    def send_bytes(self, status, data, ctype, headers=None, api=False, cache=None):
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)) if self.command != "HEAD" else str(len(data)))
        self.send_header("X-Request-Id", uuid.uuid4().hex[:16])
        for k, v in SECURITY_HEADERS.items():
            self.send_header(k, v)
        self.send_header("Cache-Control", cache or "no-store")
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(data)

    def fail(self, f):
        if f.code == "approval_cannot_wait":
            body = {"error": f.code}
        else:
            body = {"error": f.code, "message": f.message or MESSAGES.get(f.code, "The request failed."), "retryable": f.status == 503}
            body.update(f.extra)
        self.send_json(f.status, body)

    def body_json(self):
        raw = self.raw_body
        try:
            return json.loads(raw.decode()) if raw else None
        except Exception:
            raise Fail(400, "bad_request")

    def do_HEAD(self):
        self.do_GET()

    def do_GET(self):
        self.route("GET")

    def do_POST(self):
        self.route("POST")

    def do_PUT(self):
        self.route("PUT")

    def do_DELETE(self):
        self.route("DELETE")

    def do_OPTIONS(self):
        self.fail(Fail(405, "method_not_allowed"))

    def route(self, method):
        st = self.state
        if st.args.slow_ms:
            time.sleep(st.args.slow_ms / 1000)
        # The body is read at once, so that a refusal before the read never leaves bytes in the connection.
        n = int(self.headers.get("Content-Length") or 0)
        self.raw_body = self.rfile.read(n) if n else b""
        parsed = urllib.parse.urlparse(self.path)
        path = parsed.path
        q = urllib.parse.parse_qs(parsed.query, keep_blank_values=True)
        try:
            if path.startswith("/__fake/"):
                return self.control(method, path[8:])
            if not path.startswith(st.base):
                raise Fail(404, "not_found")
            rel = path[len(st.base):]
            if rel.startswith("static/"):
                return self.static(rel)
            if rel.startswith("api/"):
                if method in ("POST", "PUT", "DELETE"):
                    self.write_guard()
                return self.api(method, rel[4:], q)
            if method not in ("GET", "HEAD"):
                raise Fail(405, "method_not_allowed")
            return self.send_bytes(200, st.shell(), "text/html; charset=utf-8", cache="no-cache")
        except Fail as f:
            self.fail(f)
        except BrokenPipeError:
            pass
        except Exception as e:  # a defect of the fake
            sys.stderr.write("fake error: %r\n" % (e,))
            import traceback
            traceback.print_exc()
            self.fail(Fail(500, "internal_error"))

    def write_guard(self):
        site = self.headers.get("Sec-Fetch-Site")
        if site is not None and site not in ("same-origin", "none"):
            raise Fail(403, "cross_site_request")
        origin = self.headers.get("Origin")
        if site is None and origin is not None:
            if origin == "null" or urllib.parse.urlparse(origin).netloc != self.headers.get("Host"):
                raise Fail(403, "cross_site_request")
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            ct = (self.headers.get("Content-Type") or "").replace(" ", "").lower()
            if ct not in ("application/json", "application/json;charset=utf-8"):
                raise Fail(415, "unsupported_media_type")

    def static(self, rel):
        st = self.state
        parts = rel.split("/", 2)
        if len(parts) < 3 or parts[1] != st.build or parts[2] not in st.assets:
            raise Fail(404, "not_found")
        data = open(os.path.join(WEB, parts[2]), "rb").read()
        ext = os.path.splitext(parts[2])[1]
        self.send_bytes(200, data, CONTENT_TYPES.get(ext, "application/octet-stream"), cache="private, max-age=31536000, immutable")

    def control(self, method, name):
        st = self.state
        if name == "pure_tests.js" and method in ("GET", "HEAD"):
            data = open(os.path.join(ROOT, "tests", "page", "pure_tests.js"), "rb").read()
            return self.send_bytes(200, data, CONTENT_TYPES[".js"])
        body = self.body_json() if method == "POST" else None
        if name == "reset":
            st.store = Store(st.args.fixture)
            return self.send_json(200, {})
        if name == "fail":
            st.store.inject.append(body)
            return self.send_json(200, {})
        if name == "deny":
            st.store.denied = set((body or {}).get("actions", []))
            return self.send_json(200, {})
        if name == "session":
            st.store.subject = (body or {}).get("subject", "alice")
            return self.send_json(200, {})
        if name == "log":
            return self.send_json(200, {"items": st.store.log})
        raise Fail(404, "not_found")

    # --- the API
    def need(self, action):
        if action in self.state.store.denied:
            raise Fail(403, "forbidden", action=action)

    def check_params(self, q, allowed):
        for k, v in q.items():
            if k not in allowed or len(v) != 1:
                raise Fail(400, "bad_request")

    def page_params(self, q, extra=()):
        self.check_params(q, {"limit", "after", *extra})
        limit = self.state.max_page
        if "limit" in q:
            if not re.fullmatch(r"[1-9][0-9]*", q["limit"][0]) or int(q["limit"][0]) > limit:
                raise Fail(400, "bad_request")
            limit = int(q["limit"][0])
        after = unb64(q["after"][0]) if "after" in q else 0
        return limit, after

    def paged(self, q, items, extra=()):
        limit, after = self.page_params(q, extra)
        chunk = items[after:after + limit]
        out = {"items": chunk}
        if after + limit < len(items):
            out["next"] = b64(after + limit)
        return out

    def api(self, method, rest, q):
        st = self.state
        store = st.store
        for rule in list(store.inject):
            if rule["match"] in rest and rule.get("method", method) == method:
                rule["times"] = rule.get("times", 1) - 1
                if rule["times"] <= 0:
                    store.inject.remove(rule)
                raise Fail(rule["status"], rule["error"], **rule.get("extra", {}))
        s = st.args
        segs = [urllib.parse.unquote(x) for x in rest.split("/")]
        user = store.subject if s.mode == "authzen" else "-"
        if segs == ["session"]:
            if method != "GET":
                raise Fail(405, "method_not_allowed")
            self.need("access")
            allow = (s.mode == "authzen") or not s.no_apply
            runner = {"allow_apply": bool(allow and not s.no_runner), "enabled": not s.no_runner}
            return self.send_json(200, {"limits": {"max_document_bytes": s.max_document_bytes, "max_hosts": 4, "max_page_items": st.max_page},
                                        "mode": s.mode, "runner": runner, "subject": user, "version": "0.1.0"})
        if segs == ["catalog"]:
            self.need("access")
            out = {"schemas": [schema_for_catalog(v) for v in sorted(SCHEMAS.values(), key=lambda x: x["id"])]}
            if not s.no_runner:
                out["commands"] = COMMANDS
                out["playbooks"] = PLAYBOOKS
            return self.send_json(200, out)
        if segs[0] == "runs":
            return self.runs_api(method, segs[1:], q, user)
        if segs[0] == "environments":
            return self.env_api(method, segs[1:], q, user)
        raise Fail(404, "not_found")

    # documents -----------------------------------------------------------------------------------------------------
    def read_doc(self, k, action):
        self.need(action)
        rec = self.state.store.get(k)
        if not rec:
            raise Fail(404, "not_found")
        self.send_bytes(200, rec["text"].encode(), "application/json; charset=utf-8", {"ETag": '"%s"' % rec["etag"]}, api=True)

    def precondition(self):
        inm, im = self.headers.get("If-None-Match"), self.headers.get("If-Match")
        if inm is None and im is None:
            raise Fail(428, "precondition_required")
        if inm is not None and im is not None:
            raise Fail(400, "bad_request")
        if inm is not None:
            if inm != "*":
                raise Fail(400, "bad_request")
            return "create"
        m = re.fullmatch(r'"([0-9a-f]{64})"', im)
        if not m:
            raise Fail(400, "bad_request")
        return m.group(1)

    def write_doc(self, k, action, kind, validate, user, creating_only=False):
        self.need(action)
        precond = self.precondition()
        body = self.body_json()
        if not isinstance(body, dict) or "document" not in body or set(body) - {"document", "message"}:
            raise Fail(400, "bad_request")
        message = body.get("message", "")
        if not isinstance(message, str) or len(message) > 500:
            raise Fail(422, "invalid_document", errors=[{"path": "message", "message": "expected a text of 0 to 500 characters"}])
        errs = validate(body["document"])
        if errs:
            raise Fail(422, "invalid_document", errors=errs)
        if len(canonical_text(body["document"]).encode()) > self.state.args.max_document_bytes:
            raise Fail(413, "payload_too_large")
        rec = self.state.store.put(k, body["document"], precond, user, message)
        self.state.store.log.append({"op": "put", "key": k, "version": rec["version"], "message": message})
        self.send_json(201 if precond == "create" else 200, {"etag": rec["etag"], "version": rec["version"]}, {"ETag": '"%s"' % rec["etag"]})

    def delete_doc(self, k, action, user, check=None):
        self.need(action)
        im = self.headers.get("If-Match")
        if im is None:
            raise Fail(428, "precondition_required")
        m = re.fullmatch(r'"([0-9a-f]{64})"', im)
        if not m:
            raise Fail(400, "bad_request")
        if not self.state.store.get(k):
            raise Fail(404, "not_found")
        if check:
            check()
        self.state.store.delete(k, m.group(1), user)
        self.state.store.log.append({"op": "delete", "key": k})
        self.send_json(200, {"version": self.state.store.counter[k]})

    def history(self, k, action, q, extra):
        self.need(action)
        store = self.state.store
        if k not in store.hist:
            raise Fail(404, "not_found")
        self.check_params(q, {"limit", "before"})
        limit = self.state.max_page
        if "limit" in q:
            if not re.fullmatch(r"[1-9][0-9]*", q["limit"][0]) or int(q["limit"][0]) > limit:
                raise Fail(400, "bad_request")
            limit = int(q["limit"][0])
        items = [h for h in reversed(store.hist[k])]
        if "before" in q:
            before = int(q["before"][0])
            items = [h for h in items if h["version"] < before]
        page = items[:limit]
        out = {"items": [{kk: vv for kk, vv in h.items() if kk != "text" and not (kk == "etag" and vv is None)} for h in page]}
        if len(items) > limit:
            out["next"] = str(page[-1]["version"])
        self.send_json(200, out)

    def history_version(self, k, action, n):
        self.need(action)
        store = self.state.store
        for h in store.hist.get(k, []):
            if str(h["version"]) == n and h["text"] is not None:
                return self.send_bytes(200, h["text"].encode(), "application/json; charset=utf-8", {"ETag": '"%s"' % h["etag"]}, api=True)
        raise Fail(404, "not_found")

    # validation ----------------------------------------------------------------------------------------------------------
    def v_environment(self, d):
        errs = []
        if not isinstance(d, dict) or set(d) != {"format", "label"} or d.get("format") != 1:
            return [{"path": "", "message": "expected an object with format and label"}]
        if not isinstance(d["label"], str) or not (1 <= len(d["label"]) <= 64) or any(ord(c) < 32 for c in d["label"]):
            errs.append({"path": "label", "message": "expected a text of 1 to 64 characters with no control character, found %s" % json.dumps(d["label"])})
        return errs

    def v_host(self, e):
        def check(d):
            if not isinstance(d, dict) or not set(d) <= {"format", "address", "port", "labels"} or d.get("format") != 1 or "address" not in d:
                return [{"path": "", "message": "expected an object with format, address, port and labels"}]
            errs = []
            a = d["address"]
            if not isinstance(a, str) or not (is_ip(a) or (dns_name(a) and not a.split(".")[-1].isdigit())):
                errs.append({"path": "address", "message": "expected a DNS name or an IP address, found %s" % json.dumps(a)})
            if "port" in d and not (isinstance(d["port"], int) and 1 <= d["port"] <= 65535):
                errs.append({"path": "port", "message": "expected an integer from 1 to 65535, found %s" % json.dumps(d["port"])})
            labels = d.get("labels", [])
            groups = {sc["group"] for sc in SCHEMAS.values()}
            if not isinstance(labels, list) or len(labels) > 16 or labels != sorted(set(labels)) or any(not isinstance(l, str) or not LABEL_RE.match(l) or l in groups for l in labels):
                errs.append({"path": "labels", "message": "expected 0 to 16 host labels in byte order, each one time, found %s" % json.dumps(labels)})
            return errs
        return check

    def v_unit(self, e, u):
        def check(d):
            if not isinstance(d, dict) or set(d) != {"format", "label", "schema", "playbooks", "hosts"} or d.get("format") != 1:
                return [{"path": "", "message": "expected an object with format, label, schema, playbooks and hosts"}]
            errs = []
            if not isinstance(d["label"], str) or not (1 <= len(d["label"]) <= 64):
                errs.append({"path": "label", "message": "expected a text of 1 to 64 characters"})
            if d["schema"] not in SCHEMAS:
                errs.append({"path": "schema", "message": "expected one of the schemas, found %s" % json.dumps(d["schema"])})
            pb = d["playbooks"]
            if not isinstance(pb, dict) or "deploy" not in pb or set(pb) - {"deploy", "render", "status"}:
                errs.append({"path": "playbooks", "message": "expected deploy, and render and status as options"})
            elif not self.state.args.no_runner:
                kinds = {p["id"]: p["kind"] for p in PLAYBOOKS}
                for k, v in pb.items():
                    if kinds.get(v) != k:
                        errs.append({"path": "playbooks.%s" % k, "message": "expected the id of a playbook of the kind %s, found %s" % (k, json.dumps(v))})
            hosts = d["hosts"]
            if not isinstance(hosts, list) or hosts != sorted(set(hosts)):
                errs.append({"path": "hosts", "message": "expected host ids in byte order, each one time"})
            else:
                for h in hosts:
                    if not self.state.store.get("environments/%s/hosts/%s" % (e, h)):
                        errs.append({"path": "hosts", "message": "the host %s has no document" % h})
            old = self.state.store.get("environments/%s/units/%s" % (e, u))
            if old and isinstance(hosts, list):
                for h in old["doc"]["hosts"]:
                    if h not in hosts and self.state.store.get("environments/%s/units/%s/hosts/%s/values" % (e, u, h)):
                        raise Fail(409, "in_use", errors=[{"path": "hosts", "message": "the host %s has values for this unit" % h}])
            return errs
        return check

    def v_values(self, kind, e, u=None, h=None):
        def check(d):
            if not isinstance(d, dict) or not set(d) <= {"format", "values", "notes"} or d.get("format") != 1 or not isinstance(d.get("values"), dict):
                return [{"path": "", "message": "expected an object with format, values and notes"}]
            errs = []
            if len(d["values"]) > 1024:
                errs.append({"path": "values", "message": "expected at most 1024 keys"})
            if kind == "common":
                defs = {}
                for sc in SCHEMAS.values():
                    for kd in sc["keys"]:
                        defs.setdefault(kd["path"], []).append(kd)
            else:
                unit = self.state.store.get("environments/%s/units/%s" % (e, u))
                if not unit:
                    return [{"path": "", "message": "the unit does not exist"}]
                defs = {kd["path"]: [kd] for kd in SCHEMAS[unit["doc"]["schema"]]["keys"]}
                if kind == "host_values" and h not in unit["doc"]["hosts"]:
                    return [{"path": "", "message": "the host is not in the unit"}]
            for path, v in d["values"].items():
                if path not in defs:
                    errs.append({"path": path, "message": "expected a key of the schema, found %s" % json.dumps(path)})
                    continue
                if any(kd.get("secret") for kd in defs[path]):
                    errs.append({"path": path, "message": "expected a key that is not a secret key"})
                    continue
                for kd in defs[path]:
                    m = check_value(kd, v)
                    if m:
                        errs.append({"path": path, "message": m})
                        break
            for path, n in (d.get("notes") or {}).items():
                if path not in d["values"] or not isinstance(n, str) or not (1 <= len(n) <= 500):
                    errs.append({"path": path, "message": "expected a note of 1 to 500 characters for a key with a value"})
            return errs
        return check

    # routes ------------------------------------------------------------------------------------------------------------------
    def env_items(self):
        out = []
        for k, rec in self.state.store.docs.items():
            if re.fullmatch(r"environments/[^/]+", k):
                out.append((k.split("/")[1], rec))
        return sorted(out, key=lambda x: x[0])

    def children(self, e, kind):
        pre = "environments/%s/%s/" % (e, kind)
        out = []
        for k, rec in self.state.store.docs.items():
            if k.startswith(pre) and "/" not in k[len(pre):]:
                out.append((k[len(pre):], rec))
        return sorted(out, key=lambda x: x[0])

    def has_env(self, e):
        return self.state.store.get("environments/%s" % e) is not None

    def env_api(self, method, segs, q, user):
        store = self.state.store
        allowed = {"GET", "PUT", "DELETE", "POST"}
        if not segs:
            if method == "GET":
                self.need("config.read")
                items = []
                for e, rec in self.env_items():
                    items.append({"id": e, "label": rec["doc"]["label"], "etag": rec["etag"], "version": rec["version"]})
                return self.send_json(200, self.paged(q, items))
            if method == "POST":
                self.need("inventory.write")
                body = self.body_json()
                if not isinstance(body, dict) or set(body) - {"id", "document", "message"} or "id" not in body or "document" not in body:
                    raise Fail(400, "bad_request")
                if not isinstance(body["id"], str) or not ID_RE.match(body["id"]):
                    raise Fail(422, "invalid_document", errors=[{"path": "id", "message": "expected an id of 1 to 63 characters, found %s" % json.dumps(body["id"])}])
                errs = self.v_environment(body["document"])
                if errs:
                    raise Fail(422, "invalid_document", errors=errs)
                if len(self.env_items()) >= 64:
                    raise Fail(422, "limit_exceeded", limit="limits.max_environments")
                rec = store.put("environments/%s" % body["id"], body["document"], "create", user, body.get("message", ""))
                return self.send_json(201, {"etag": rec["etag"], "version": rec["version"]})
            raise Fail(405, "method_not_allowed")
        e = segs[0]
        if not ID_RE.match(e):
            raise Fail(404, "not_found")
        ek = "environments/%s" % e
        rest = segs[1:]
        if not rest:
            if method == "GET":
                return self.read_doc(ek, "config.read")
            if method == "PUT":
                return self.write_doc(ek, "inventory.write", "environment", self.v_environment, user)
            if method == "DELETE":
                def check():
                    if any(k.startswith(ek + "/") for k in store.docs):
                        raise Fail(409, "not_empty", errors=[{"path": "", "message": "the environment has other documents"}])
                return self.delete_doc(ek, "inventory.write", user, check)
            raise Fail(405, "method_not_allowed")
        if rest == ["history"] and method == "GET":
            return self.history(ek, "config.read", q, None)
        if len(rest) == 2 and rest[0] == "history" and method == "GET":
            return self.history_version(ek, "config.read", rest[1])
        if not self.has_env(e):
            self.need("config.read")
            raise Fail(404, "not_found")
        if rest[0] == "common":
            k = ek + "/common"
            return self.doc_routes(method, rest[1:], q, k, "config.read", "config.write", self.v_values("common", e), user)
        if rest[0] == "hosts":
            if len(rest) == 1:
                self.need("config.read")
                items = [{"id": h, "document": r["doc"], "etag": r["etag"], "version": r["version"]} for h, r in self.children(e, "hosts")]
                return self.send_json(200, self.paged(q, items))
            h = rest[1]
            if not ID_RE.match(h):
                raise Fail(404, "not_found")
            hk = "%s/hosts/%s" % (ek, h)

            def in_use():
                for u, r in self.children(e, "units"):
                    if h in r["doc"]["hosts"]:
                        raise Fail(409, "in_use", errors=[{"path": "", "message": "the unit %s holds the host" % u}])
            return self.doc_routes(method, rest[2:], q, hk, "config.read", "inventory.write", self.v_host(e), user, delete_check=in_use)
        if rest[0] == "units":
            if len(rest) == 1:
                self.need("config.read")
                items = []
                for u, r in self.children(e, "units"):
                    item = {"id": u, "label": r["doc"]["label"], "schema": r["doc"]["schema"], "hosts": r["doc"]["hosts"], "etag": r["etag"], "version": r["version"]}
                    missing = [h for h in r["doc"]["hosts"] if not store.get("%s/hosts/%s" % (ek, h))]
                    if missing:
                        item["errors"] = [{"path": "hosts", "message": "The host %s has no document." % h} for h in missing]
                    items.append(item)
                return self.send_json(200, self.paged(q, items))
            u = rest[1]
            if not ID_RE.match(u):
                raise Fail(404, "not_found")
            uk = "%s/units/%s" % (ek, u)
            tail = rest[2:]
            if not tail or tail[0] == "history":
                def not_empty():
                    if store.get(uk + "/values") or any(k.startswith(uk + "/hosts/") for k in store.docs):
                        raise Fail(409, "not_empty", errors=[{"path": "", "message": "the unit has values"}])
                return self.doc_routes(method, tail, q, uk, "config.read", "inventory.write", self.v_unit(e, u), user, delete_check=not_empty)
            if tail[0] == "values":
                return self.doc_routes(method, tail[1:], q, uk + "/values", "config.read", "config.write", self.v_values("unit_values", e, u), user)
            if tail[0] == "hosts" and len(tail) >= 2:
                h = tail[1]
                return self.doc_routes(method, tail[3:], q, "%s/hosts/%s/values" % (uk, h), "config.read", "config.write", self.v_values("host_values", e, u, h), user) if len(tail) >= 3 and tail[2] == "values" else self.nf()
            if tail == ["effective"] and method == "GET":
                return self.effective(e, u, q)
            if tail[0] == "secrets":
                return self.secrets_api(method, tail[1:], e, u, user)
        if rest[0] == "secrets":
            return self.secrets_api(method, rest[1:], e, None, user)
        raise Fail(404, "not_found")

    def nf(self):
        raise Fail(404, "not_found")

    def doc_routes(self, method, tail, q, k, ra, wa, validate, user, delete_check=None):
        if not tail:
            if method == "GET":
                return self.read_doc(k, ra)
            if method == "PUT":
                return self.write_doc(k, wa, None, validate, user)
            if method == "DELETE":
                return self.delete_doc(k, wa, user, delete_check)
            raise Fail(405, "method_not_allowed")
        if tail == ["history"] and method == "GET":
            return self.history(k, ra, q, None)
        if len(tail) == 2 and tail[0] == "history" and method == "GET":
            return self.history_version(k, ra, tail[1])
        raise Fail(404, "not_found")

    def effective(self, e, u, q):
        store = self.state.store
        self.check_params(q, {"host"})
        self.need("config.read")
        unit = store.get("environments/%s/units/%s" % (e, u))
        if not unit:
            raise Fail(404, "not_found")
        host = q["host"][0] if "host" in q else None
        if host is not None and host not in unit["doc"]["hosts"]:
            raise Fail(404, "not_found")
        schema = SCHEMAS[unit["doc"]["schema"]]
        env = (store.get("environments/%s/common" % e) or {"doc": {"values": {}}})["doc"]["values"]
        uv = (store.get("environments/%s/units/%s/values" % (e, u)) or {"doc": {"values": {}}})["doc"]["values"]
        hv = {}
        if host:
            hv = (store.get("environments/%s/units/%s/hosts/%s/values" % (e, u, host)) or {"doc": {"values": {}}})["doc"]["values"]
        keys = []
        for kd in schema["keys"]:
            p = kd["path"]
            item = {"path": p, "kind": kd["kind"], "secret": bool(kd.get("secret"))}
            layers = {}
            if item["secret"]:
                fe = store.fingerprint("environment:%s" % e, p)
                fu = store.fingerprint("environment:%s/unit:%s" % (e, u), p)
                if fe:
                    layers["environment"] = {"fingerprint": fe}
                if fu:
                    layers["unit"] = {"fingerprint": fu}
            else:
                if "default" in kd:
                    layers["default"] = kd["default"]
                for name, src in (("environment", env), ("unit", uv), ("host", hv)):
                    if p in src:
                        layers[name] = src[p]
            if layers:
                top = [n for n in ("default", "environment", "unit", "host") if n in layers][-1]
                item["layer"] = top
                if not item["secret"]:
                    item["value"] = layers[top]
                item["layers"] = layers
            keys.append(item)
        out = {"environment": e, "keys": keys, "schema": unit["doc"]["schema"], "unit": u}
        if host:
            out["host"] = host
        self.send_json(200, out)

    # secrets -------------------------------------------------------------------------------------------------------------
    def secrets_api(self, method, tail, e, u, user):
        store = self.state.store
        scope = "environment:%s" % e if u is None else "environment:%s/unit:%s" % (e, u)
        if u is not None and not store.get("environments/%s/units/%s" % (e, u)):
            self.need("config.read")
            raise Fail(404, "not_found")
        if not tail:
            if method != "GET":
                raise Fail(405, "method_not_allowed")
            self.need("config.read")
            items = [{"name": n, "fingerprint": store.fingerprint(sc, n), "updated_at": v["updated_at"], "updated_by": v["updated_by"]}
                     for (sc, n), v in sorted(store.secrets.items(), key=lambda kv: kv[0][1]) if sc == scope]
            return self.send_json(200, {"items": items})
        name = tail[0]
        self.need("secret.write")
        if method == "PUT":
            body = self.body_json()
            if not isinstance(body, dict) or set(body) != {"value"} or not isinstance(body["value"], str):
                raise Fail(400, "bad_request")
            if u is None:
                defs = [kd for sc in SCHEMAS.values() for kd in sc["keys"] if kd["path"] == name and kd.get("secret")]
            else:
                defs = [kd for kd in SCHEMAS[store.get("environments/%s/units/%s" % (e, u))["doc"]["schema"]]["keys"] if kd["path"] == name and kd.get("secret")]
            if not defs:
                raise Fail(422, "invalid_secret", errors=[{"path": name, "message": "expected a secret key of a schema"}])
            v = body["value"]
            if not (8 <= len(v.encode()) <= 65536) or "\0" in v:
                raise Fail(422, "invalid_secret", errors=[{"path": name, "message": "expected a text of 8 to 65536 bytes"}])
            for kd in defs:
                if check_value({**kd}, v):
                    raise Fail(422, "invalid_secret", errors=[{"path": name, "message": "expected a value that passes the check of the key"}])
            store.set_secret(scope, name, v, user)
            store.log.append({"op": "secret", "scope": scope, "name": name})
            return self.send_json(200, {"fingerprint": store.fingerprint(scope, name)})
        if method == "DELETE":
            if (scope, name) not in store.secrets:
                raise Fail(404, "not_found")
            del store.secrets[(scope, name)]
            store.log.append({"op": "secret-delete", "scope": scope, "name": name})
            return self.send_json(200, {})
        raise Fail(405, "method_not_allowed")

    # runs ---------------------------------------------------------------------------------------------------------------------
    def runs_api(self, method, tail, q, user):
        st = self.state
        store = st.store
        if st.args.no_runner:
            raise Fail(404, "not_found")
        if not tail:
            if method == "POST":
                return self.create_run(user)
            if method == "GET":
                self.need("run.read")
                self.check_params(q, {"limit", "after", "environment", "status"})
                items = []
                for rid in reversed(store.run_order):
                    rec = self.run_record(rid)
                    if "environment" in q and rec["environment"] != q["environment"][0]:
                        continue
                    if "status" in q and rec["status"] != q["status"][0]:
                        continue
                    items.append(rec)
                return self.send_json(200, self.paged(q, items, extra=("environment", "status")))
            raise Fail(405, "method_not_allowed")
        rid = tail[0]
        if not RUN_RE.match(rid) or rid not in store.runs:
            self.need("run.read")
            raise Fail(404, "not_found")
        self.need("run.read")
        if len(tail) == 1 and method == "GET":
            return self.send_json(200, self.run_record(rid))
        if tail[1:] == ["cancel"] and method == "POST":
            return self.cancel_run(rid, user)
        if tail[1:] == ["events"] and method == "GET":
            return self.run_events(rid, q)
        if tail[1:] == ["output"] and method == "GET":
            return self.run_output(rid, q)
        raise Fail(404, "not_found")

    def create_run(self, user):
        st = self.state
        store = st.store
        body = self.body_json()
        if not isinstance(body, dict):
            raise Fail(400, "bad_request")
        kind, mode = body.get("kind"), body.get("mode")
        if (kind, mode) not in (("deploy", "check"), ("deploy", "apply"), ("status", "apply"), ("command", "check"), ("command", "apply")):
            raise Fail(422, "invalid_run_request", errors=[{"path": "kind", "message": "expected a kind and a mode of the table"}])
        action = {("deploy", "check"): "run.check", ("deploy", "apply"): "run.apply", ("status", "apply"): "run.check"}.get((kind, mode), "command.run")
        self.need(action)
        if (mode == "apply" or kind == "command") and st.args.mode == "open" and st.args.no_apply:
            raise Fail(403, "apply_disabled")
        errs = []
        if mode == "apply" and "check_run" in body:
            chk = store.runs.get(body["check_run"])
            if not chk:
                raise Fail(404, "not_found")
            rec = self.run_record(body["check_run"])
            if rec["mode"] != "check" or rec["status"] != "passed":
                raise Fail(409, "check_required")
            if chk.get("applied_by"):
                raise Fail(409, "check_used")
            if time.time() - chk["finish"] > 1800:
                raise Fail(409, "check_too_old")
            if set(body) - {"kind", "mode", "check_run", "message"}:
                raise Fail(422, "invalid_run_request", errors=[{"path": "", "message": "expected no other member with check_run"}])
            node, hosts, params, command = rec["node"], rec["hosts"], rec.get("params"), rec.get("command")
        else:
            allowed = {"kind", "mode", "node", "hosts", "message"} | ({"command", "params"} if kind == "command" else set())
            if mode == "apply" and kind == "deploy":
                raise Fail(422, "invalid_run_request", errors=[{"path": "check_run", "message": "expected the id of a check run"}])
            for k in set(body) - allowed:
                errs.append({"path": k, "message": "this member is not valid for this request"})
            node, hosts, params, command = body.get("node"), body.get("hosts"), body.get("params", {}), body.get("command")
            m = re.fullmatch(r"environment:([a-z0-9-]+)(?:/unit:([a-z0-9-]+)(?:/host:([a-z0-9-]+))?)?", node or "")
            if not m:
                errs.append({"path": "node", "message": "expected a node path"})
            else:
                e, u, h = m.groups()
                if not store.get("environments/%s" % e):
                    raise Fail(404, "not_found")
                if kind == "command":
                    cmd = next((c for c in COMMANDS if c["id"] == command), None)
                    if not cmd:
                        errs.append({"path": "command", "message": "expected the id of a command"})
                    else:
                        want = "host" if h else "unit" if u else "environment"
                        if want != cmd["target"]:
                            errs.append({"path": "node", "message": "expected a node of the type %s" % cmd["target"]})
                        if mode == "check" and not cmd["check"]:
                            errs.append({"path": "mode", "message": "this command has no check"})
                        if mode == "apply" and cmd["check"]:
                            errs.append({"path": "check_run", "message": "this command needs a check run"})
                        errs += self.check_cmd_params(cmd, params)
                elif not u or h and False:
                    errs.append({"path": "node", "message": "expected a node of the type unit or host"})
                else:
                    unit = store.get("environments/%s/units/%s" % (e, u))
                    if not unit:
                        errs.append({"path": "node", "message": "the unit does not exist"})
                    elif kind == "status" and "status" not in unit["doc"]["playbooks"]:
                        errs.append({"path": "node", "message": "the unit has no status playbook"})
                if not isinstance(hosts, list) or not hosts or hosts != sorted(set(hosts)):
                    errs.append({"path": "hosts", "message": "expected 1 or more host ids in byte order, each one time"})
                elif u and store.get("environments/%s/units/%s" % (e, u)):
                    uh = store.get("environments/%s/units/%s" % (e, u))["doc"]["hosts"]
                    for x in hosts:
                        if x not in uh:
                            errs.append({"path": "hosts", "message": "the host %s is not in the unit" % x})
                elif kind == "command" and isinstance(hosts, list):
                    for x in hosts:
                        if not store.get("environments/%s/hosts/%s" % (e, x)):
                            errs.append({"path": "hosts", "message": "the host %s has no document" % x})
            msg = body.get("message", "")
            if not isinstance(msg, str) or len(msg) > 500:
                errs.append({"path": "message", "message": "expected a text of 0 to 500 characters"})
            if errs:
                raise Fail(422, "invalid_run_request", errors=errs)
        if len(hosts) > 4:
            raise Fail(403, "ceiling_exceeded", ceiling="max_hosts", limit=4)
        if sum(1 for r in store.runs.values() if r["record"]["status"] == "queued") >= 50:
            raise Fail(503, "run_queue_full")
        t = time.time()
        rid = new_run_id(t)
        m = re.fullmatch(r"environment:([a-z0-9-]+)(?:/unit:([a-z0-9-]+))?(?:/host:([a-z0-9-]+))?", node)
        env, unit = m.group(1), m.group(2)
        playbook = body.get("command") and next(c["playbook"] for c in COMMANDS if c["id"] == body["command"]) if kind == "command" else (
            store.get("environments/%s/units/%s" % (env, unit))["doc"]["playbooks"]["status" if kind == "status" else "deploy"])
        if mode == "apply" and "check_run" in body:
            playbook = store.runs[body["check_run"]]["record"]["playbook"]
        rec = {"format": 1, "id": rid, "kind": kind, "mode": mode, "environment": env, "node": node, "hosts": hosts, "playbook": playbook, "subject": user,
               "created_at": now_iso(t), "status": "queued"}
        if command:
            rec["command"] = command
            rec["params"] = params
        if body.get("message"):
            rec["message"] = body["message"]
        if mode == "apply" and "check_run" in body:
            rec["check_run"] = body["check_run"]
            store.runs[body["check_run"]]["applied_by"] = rid
            store.runs[body["check_run"]]["record"]["applied_by"] = rid
        has_render = kind == "deploy" and mode == "check" and bool(store.get("environments/%s/units/%s" % (env, unit))["doc"]["playbooks"].get("render"))
        fail = "fail" in (body.get("message") or "")
        store.runs[rid] = {"record": rec, "t0": t, "plan": plan_run(rec, has_render, fail), "snapshot": self.snapshot(env), "static": False}
        store.run_order.append(rid)
        store.log.append({"op": "run", "id": rid, "kind": kind, "mode": mode})
        self.send_json(201, self.run_record(rid), {"Location": st.base + "api/runs/" + rid})

    def check_cmd_params(self, cmd, params):
        errs = []
        if not isinstance(params, dict):
            return [{"path": "params", "message": "expected an object"}]
        decl = {p["name"]: p for p in cmd["params"]}
        for n in params:
            if n not in decl:
                errs.append({"path": n, "message": "this parameter is not declared"})
        for n, p in decl.items():
            if n not in params:
                if p["required"]:
                    errs.append({"path": n, "message": "this parameter is necessary"})
                continue
            v, t = params[n], p["type"]
            ok = ((t == "text" and isinstance(v, str) and p["min"] <= len(v) <= p["max"]) or
                  (t == "integer" and isinstance(v, int) and not isinstance(v, bool) and p["min"] <= v <= p["max"]) or
                  (t == "boolean" and isinstance(v, bool)) or
                  (t == "choice" and v in p["choices"]) or
                  (t == "duration" and parse_duration(v) is not None and parse_duration(p["min_duration"]) <= parse_duration(v) <= parse_duration(p["max_duration"])))
            if not ok:
                errs.append({"path": n, "message": "expected a valid %s, found %s" % (t, json.dumps(v))})
        return errs

    def snapshot(self, env):
        h = hashlib.sha256()
        store = self.state.store
        for k in sorted(store.docs):
            if k.startswith("environments/%s/" % env):
                h.update(k.encode() + store.docs[k]["etag"].encode())
        for (sc, n), v in sorted(store.secrets.items()):
            if sc.startswith("environment:%s" % env):
                h.update((sc + n + v["value"]).encode())
        return h.hexdigest()

    def run_record(self, rid):
        st = self.state.store
        run = st.runs[rid]
        rec = dict(run["record"])
        if run.get("static"):
            return rec
        el = time.time() - run["t0"]
        plan = run["plan"]
        canceled = run.get("canceled_at")
        if canceled is not None:
            rec["status"] = "canceled"
            rec["started_at"] = now_iso(run["t0"] + plan["start"])
            rec["finished_at"] = now_iso(canceled)
            rec["cancel"] = run["cancel"]
            run["finish"] = canceled
            return rec
        if el < plan["start"]:
            rec["status"] = "queued"
            return rec
        rec["started_at"] = now_iso(run["t0"] + plan["start"])
        if "snapshot" not in rec:
            rec["snapshot"] = run["snapshot"] if rec["mode"] == "check" else None
        # the guard at the start of an apply (4.24)
        if rec["mode"] == "apply" and "check_run" in rec:
            chk = st.runs[rec["check_run"]]["snapshot"]
            if self.snapshot(rec["environment"]) != chk:
                rec["status"] = "failed"
                rec["reason"] = "snapshot_changed"
                rec["finished_at"] = rec["started_at"]
                rec.pop("snapshot", None)
                run["finish"] = run["t0"] + plan["start"]
                return rec
        rec["snapshot"] = run["snapshot"]
        if el < plan["end"]:
            rec["status"] = "running"
            rec["runner"] = {"node": "fake", "pid": 4242}
            return rec
        rec["finished_at"] = now_iso(run["t0"] + plan["end"])
        run["finish"] = run["t0"] + plan["end"]
        rec["status"] = "failed" if plan["fail"] else "passed"
        rec["exit_code"] = 2 if plan["fail"] else 0
        if plan["fail"]:
            rec["reason"] = "exit_code"
        rec["stats"] = plan["stats"]
        return rec

    def cancel_run(self, rid, user):
        self.need("run.check")
        run = self.state.store.runs[rid]
        rec = self.run_record(rid)
        if rec["status"] in ("passed", "failed", "canceled", "lost"):
            raise Fail(409, "run_finished")
        run["canceled_at"] = time.time()
        run["cancel"] = {"by": user, "at": now_iso()}
        self.send_json(200, self.run_record(rid))

    def run_events(self, rid, q):
        self.check_params(q, {"after", "limit"})
        run = self.state.store.runs[rid]
        after = int(q["after"][0]) if "after" in q else 0
        limit = int(q["limit"][0]) if "limit" in q else self.state.max_page
        if limit < 1 or limit > self.state.max_page:
            raise Fail(400, "bad_request")
        rec = self.run_record(rid)
        final = rec["status"] in ("passed", "failed", "canceled", "lost")
        if run.get("static"):
            events = static_events(rec)
        else:
            el = (run.get("canceled_at") or time.time()) - run["t0"]
            events = [dict(ev, seq=i + 1) for i, ev in enumerate(e for e in run["plan"]["events"] if e["at"] <= el)]
            for ev in events:
                ev["t"] = now_iso(run["t0"] + ev.pop("at"))
        items = [e for e in events if e["seq"] > after][:limit]
        last = items[-1]["seq"] if items else after
        complete = final and (not events or last >= events[-1]["seq"])
        self.send_json(200, {"items": items, "last_seq": last, "complete": complete})

    def run_output(self, rid, q):
        self.check_params(q, {"offset", "limit"})
        run = self.state.store.runs[rid]
        offset = int(q["offset"][0]) if "offset" in q else 0
        limit = int(q["limit"][0]) if "limit" in q else 1048576
        if limit < 1 or limit > 1048576 or offset < 0:
            raise Fail(400, "bad_request")
        rec = self.run_record(rid)
        final = rec["status"] in ("passed", "failed", "canceled", "lost")
        if run.get("static"):
            data = b"PLAY [Deploy] ***\nok: [host-a]\nok: [host-b]\n"
        else:
            el = (run.get("canceled_at") or time.time()) - run["t0"]
            data = b"".join(line for at, line in run["plan"]["output"] if at <= el)
        self.send_bytes(200, data[offset:offset + limit], "application/octet-stream",
                        {"X-Pupitre-Output-Length": str(len(data)), "X-Pupitre-Run-Complete": "true" if final else "false"})


def plan_run(rec, has_render, fail):
    """The events and the output of a simulated run, with the time at which each one appears."""
    hosts = rec["hosts"]
    events, output = [], []
    t = 0.9
    step = 0.35

    def ev(phase, **kw):
        nonlocal t
        events.append(dict({"v": 1, "phase": phase, "at": t}, **kw))
        t += step

    def out(text):
        output.append((t, text.encode()))

    check = rec["mode"] == "check"
    if has_render:
        ev("render", kind="playbook_start", playbook="render-relay.yml", check=False)
        out("PLAY [Render the configuration] ****\n")
        ev("render", kind="task_start", task="Write the configuration files")
        out("TASK [Write the configuration files] ****\nok: [localhost]\n")
        ev("render", kind="task_start", task="Check the configuration with the service")
        out("TASK [Check the configuration with the service] ****\nok: [localhost]\n")
    ev("main", kind="playbook_start", playbook=rec["playbook"] + ".yml", check=check)
    out("PLAY [%s] ****\n" % rec["playbook"])
    ev("main", kind="play_start", play="Run %s" % rec["playbook"])
    tasks = ["Gather the facts", "Install the release", "Write the configuration file", "Print the banner", "Restart the service"]
    counts = {h: {"changed": 0, "failed": 0, "ignored": 0, "ok": 0, "rescued": 0, "skipped": 0, "unreachable": 0} for h in hosts}
    for i, task in enumerate(tasks):
        ev("main", kind="task_start", task=task)
        out("\nTASK [%s] ****\n" % task)
        for hi, h in enumerate(hosts):
            status = "ok"
            e = {"kind": "host_result", "host": h, "task": task}
            if task == "Write the configuration file":
                status = "changed"
                e["diff"] = "--- before: /etc/service/service.hcl\n+++ after: /etc/service/service.hcl\n@@ -4,3 +4,3 @@\n-max_clients = 200\n+max_clients = 500\n"
            if task == "Install the release" and hi == 0:
                status = "skipped"
            if task == "Print the banner":
                e["message"] = "\x1b[2J\x1b[31mALL IS WELL\x1b[0m\rfixed text \u202etxet"
            if fail and i == 3 and hi == len(hosts) - 1:
                status = "failed"
                e["message"] = "The task failed. The command ended with the code 2."
            e["status"] = status
            ev("main", **e)
            c = counts[h]
            if status == "failed":
                c["failed"] += 1
            elif status == "skipped":
                c["skipped"] += 1
            else:
                c["ok"] += 1
                if status == "changed":
                    c["changed"] += 1
            line = "%s: [%s]" % (status, h)
            if "message" in e:
                line += ' => {"msg": "%s"}' % e["message"]
            out(line + "\n")
        if fail and i == 3:
            break
    ev("main", kind="stats", hosts=counts)
    out("\nPLAY RECAP ****\n" + "".join("%s : ok=%d changed=%d failed=%d skipped=%d\n" % (h, c["ok"], c["changed"], c["failed"], c["skipped"]) for h, c in counts.items()))
    return {"start": 0.8, "end": t + 0.2, "events": events, "output": output, "fail": fail, "stats": counts}


def static_events(rec):
    hosts = rec["hosts"]
    events = [{"seq": 1, "v": 1, "phase": "main", "t": rec["created_at"], "kind": "playbook_start", "playbook": rec["playbook"], "check": rec["mode"] == "check"}]
    for h in hosts:
        events.append({"seq": len(events) + 1, "v": 1, "phase": "main", "t": rec["created_at"], "kind": "host_result", "host": h, "task": "Gather the facts", "status": "ok"})
    return events


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8099)
    ap.add_argument("--base-path", default="/")
    ap.add_argument("--mode", default="authzen", choices=["open", "authzen"])
    ap.add_argument("--no-runner", action="store_true")
    ap.add_argument("--no-apply", action="store_true")
    ap.add_argument("--slow-ms", type=int, default=0)
    ap.add_argument("--max-document-bytes", type=int, default=262144)
    ap.add_argument("--fixture", default="standard", choices=["standard", "empty", "big"])
    args = ap.parse_args()
    if not (args.base_path.startswith("/") and args.base_path.endswith("/")):
        sys.exit("The base path starts and ends with a slash.")
    Handler.state = State(args)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    server.daemon_threads = True
    print("fake pupitre on http://127.0.0.1:%d%s build %s" % (args.port, args.base_path, Handler.state.build), flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
