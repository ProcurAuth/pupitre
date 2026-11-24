#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Checks of the page in a headless browser, against the fake API of `fake_api.py`.

A development tool. It needs Chrome or Chromium on the host and uses the Python standard library only.
It starts the fake server and the browser by itself, and stops them at the end.

Use:
    python3 -I tests/page/browser_tests.py [--shots DIR] [-k NAME] [--list]

--shots DIR writes the screenshots of each page in the light and the dark theme, at 1920 and 390 pixels.
Exit code: 0 when each check passes, 1 when a check fails, 77 when no browser is on the host (the checks do not run).
"""

import argparse
import json
import os
import socket
import subprocess
import sys
import time
import traceback
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from cdp import Browser, find_browser  # noqa: E402

CSP = ("default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; "
       "base-uri 'self'; form-action 'self'; frame-ancestors 'none'")
NO_PROXY = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Server:
    def __init__(self, *args):
        self.port = free_port()
        self.proc = subprocess.Popen([sys.executable, "-I", os.path.join(HERE, "fake_api.py"), "--port", str(self.port), *args],
                                     stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        line = self.proc.stdout.readline()
        if "fake pupitre" not in line:
            raise RuntimeError("The fake server did not start: %s" % line)
        self.base_path = "/"
        for a, b in zip(args, args[1:]):
            if a == "--base-path":
                self.base_path = b
        self.url = "http://127.0.0.1:%d%s" % (self.port, self.base_path)

    def call(self, method, path, body=None, headers=None, absolute=False):
        data = json.dumps(body).encode() if body is not None else None
        h = {"Content-Type": "application/json", **(headers or {})}
        req = urllib.request.Request((self.url + path) if not absolute else ("http://127.0.0.1:%d%s" % (self.port, path)), data=data, method=method, headers=h)
        try:
            with NO_PROXY.open(req, timeout=10) as r:
                raw = r.read()
                return r.status, (json.loads(raw) if raw and r.headers.get("Content-Type", "").startswith("application/json") else raw), dict(r.headers)
        except urllib.error.HTTPError as e:
            raw = e.read()
            try:
                return e.code, json.loads(raw), dict(e.headers)
            except ValueError:
                return e.code, raw, dict(e.headers)

    def fake(self, name, body=None):
        return self.call("POST", "__fake/" + name, body or {}, absolute=True) if False else self.call("POST", "/__fake/" + name, body or {}, absolute=True)

    def stop(self):
        self.proc.terminate()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


class Failure(AssertionError):
    pass


def check(cond, message):
    if not cond:
        raise Failure(message)


TESTS = []


def test(fn):
    TESTS.append(fn)
    return fn


def reset(srv):
    srv.fake("reset")
    srv.fake("deny", {"actions": []})
    srv.fake("session", {"subject": "alice"})


def open_page(b, srv, path="", wait="!!document.querySelector('#app-main > *')"):
    b.take_errors()
    b.goto(srv.url + path)
    b.wait("document.documentElement.getAttribute('data-pupitre-hold') === null", 10)
    b.wait(wait, 10)


def sel(testid):
    return "[data-testid=%s]" % json.dumps(testid) if False else '[data-testid="%s"]' % testid


def no_errors(b, what="", allow=()):
    """Fails on a console error. A refusal of the server that a check causes on purpose (a status in `allow`) is not an error."""
    time.sleep(0.2)
    errors = [e for e in b.take_errors() if not any(("status of %d" % code) in e for code in allow)]
    check(not errors, "Console errors %s: %s" % (what, errors))


# --- the checks ----------------------------------------------------------------------------------------------------------------------------

@test
def shell_and_headers(b, srv):
    status, body, headers = srv.call("GET", "")
    check(status == 200, "shell status %s" % status)
    text = body.decode()
    check("{{" not in text, "A marker stays in the shell")
    check("<style" not in text and "style=" not in text, "The shell has an inline style")
    check("innerHTML" not in text, "The shell has innerHTML")
    import re
    inline = [m for m in re.findall(r"<script\b([^>]*)>", text) if "src=" not in m]
    check(not inline, "The shell has an inline script: %s" % inline)
    check(headers.get("Content-Security-Policy") == CSP, "CSP header differs")
    check("<link rel=\"icon\" href=\"data:,\">" in text, "The shell asks for a favicon file")
    open_page(b, srv, "")
    no_errors(b, "on the first page")
    check(b.eval("document.title") == "Environments - Pupitre", "title %s" % b.eval("document.title"))


@test
def pure_modules(b, srv):
    """The rules of the values, the canonical form, the visible control characters, the routes and the formats (tests/page/pure_tests.js)."""
    open_page(b, srv, "")
    names = ["validate", "canonical", "ctl", "routes", "format"]
    expr = ("(async () => { const build = document.querySelector('meta[name=pupitre-build]').content;"
            "const base = new URL('static/' + build + '/js/', document.baseURI).href; const m = {};"
            "for (const n of %s) m[n] = await import(base + n + '.js');"
            "const t = await import(location.origin + '/__fake/pure_tests.js'); return JSON.stringify(t.run(m)); })()" % json.dumps(names))
    result = json.loads(b.eval(expr, await_promise=True))
    check(result["count"] > 200, "The pure checks ran too few assertions: %s" % result["count"])
    check(not result["failures"], "Failures of the pure checks:\n  " + "\n  ".join(result["failures"]))
    print("      %d assertions of the pure modules" % result["count"])


@test
def no_name_before_session(b, srv):
    # The shell has no product name (5.23). The name shows after the session answers.
    status, body, _ = srv.call("GET", "")
    check(b"Pupitre" not in body, "The shell holds the product name")


@test
def environments_list_and_search(b, srv):
    reset(srv)
    open_page(b, srv, "", "document.querySelectorAll('[data-testid^=row-]').length === 3")
    b.fill('[data-testid="list-search"]', "stag")
    b.wait("document.querySelectorAll('[data-testid^=row-]').length === 1")
    b.fill('[data-testid="list-search"]', "zzz")
    b.wait("!!document.querySelector('[data-testid=list-empty]')")
    no_errors(b)


@test
def environment_create_and_delete(b, srv):
    reset(srv)
    open_page(b, srv, "?new=1", "!!document.querySelector('[data-testid=create]')")
    check(b.eval("document.querySelector('[data-testid=create]').disabled"), "Create is not off at first")
    b.fill('[data-testid="id"]', "Bad Id")
    check(b.eval("document.querySelector('[data-testid=create]').disabled"), "Create is on with a bad id")
    b.fill('[data-testid="id"]', "qa")
    b.fill('[data-testid="label"]', "Quality")
    check(not b.eval("document.querySelector('[data-testid=create]').disabled"), "Create is off with valid fields")
    b.click('[data-testid="create"]')
    b.wait("location.pathname === '/e/qa'")
    b.wait("!!document.querySelector('[data-testid=list-empty]')")
    b.click('[data-testid="delete-environment"]')
    b.click('[data-testid="confirm-delete-yes"]')
    b.wait("location.pathname === '/'")
    no_errors(b)


@test
def environment_delete_not_empty(b, srv):
    reset(srv)
    open_page(b, srv, "e/staging", "!!document.querySelector('[data-testid=delete-environment]')")
    b.click('[data-testid="delete-environment"]')
    b.click('[data-testid="confirm-delete-yes"]')
    b.wait("!!document.querySelector('.errors:not([hidden])')")
    check("Other items" in b.text(".errors") or "holds other" in b.text(".errors") or "empty" in b.text(".errors"), "No not_empty text: %s" % b.text(".errors"))


@test
def tabs_switch_without_reload(b, srv):
    reset(srv)
    open_page(b, srv, "e/production", "document.querySelectorAll('[data-testid^=row-]').length === 3")
    b.eval("window.__marker = 1")
    b.click('[data-testid="tab-hosts"]')
    b.wait("document.querySelectorAll('[data-testid^=row-]').length === 6")
    check(b.eval("window.__marker") == 1, "The page reloaded at a tab change")
    check(b.eval("location.search") == "?tab=hosts", "The address has no tab: %s" % b.eval("location.search"))
    b.click('[data-testid="tab-secrets"]')
    b.wait("!!document.querySelector('[data-testid=secret-upstream\\\\.token]')")
    b.eval("history.back()")
    b.wait("document.querySelectorAll('[data-testid^=row-]').length === 3 || document.querySelectorAll('[data-testid^=row-]').length === 6", 8)
    no_errors(b)


@test
def unit_values_edit_and_save(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/units/relay", "!!document.querySelector('[data-testid=\"edit-limits.max_clients\"]')")
    check(b.eval("document.querySelector('[data-testid=save]').disabled"), "Save is on with no change")
    b.click('[data-testid="edit-limits.max_clients"]')
    b.fill('[data-testid="panel-limits.max_clients"] [data-testid=value]', "500000")
    check(b.eval("document.querySelector('[data-testid=\"panel-limits.max_clients\"] [data-testid=done]').disabled"), "Done is on with a wrong value")
    err = b.text('[data-testid="panel-limits.max_clients"] .field__error')
    check(err == "expected an integer from 1 to 100000, found 500000", "error text: %r" % err)
    b.fill('[data-testid="panel-limits.max_clients"] [data-testid=value]', "0500")
    check(b.eval("document.querySelector('[data-testid=\"panel-limits.max_clients\"] [data-testid=done]').disabled"), "A leading zero is accepted")
    b.fill('[data-testid="panel-limits.max_clients"] [data-testid=value]', "500")
    b.fill('[data-testid="panel-limits.max_clients"] [data-testid=note]', "Raised for the test.")
    b.click('[data-testid="panel-limits.max_clients"] [data-testid=done]')
    check(b.eval("document.querySelector('[data-testid=\"row-limits.max_clients\"]').dataset.draft") == "changed", "No draft mark")
    check(not b.eval("document.querySelector('[data-testid=save]').disabled"), "Save is off with a change")
    b.fill('[data-testid="message"]', "Raise the limit")
    b.click('[data-testid="save"]')
    b.wait("document.querySelector('[data-testid=save-status]').textContent.startsWith('Saved')")
    status, doc, _ = srv.call("GET", "api/environments/production/units/relay/values")
    check(status == 200 and doc["values"]["limits.max_clients"] == 500, "The document is not saved: %s" % doc)
    check(doc["notes"]["limits.max_clients"] == "Raised for the test.", "The note is not saved")
    status, hist, _ = srv.call("GET", "api/environments/production/units/relay/values/history?limit=1")
    check(hist["items"][0]["message"] == "Raise the limit", "The message is not in the history")
    check(b.eval("document.querySelector('[data-testid=save]').disabled"), "Save is on after a save")
    no_errors(b)


@test
def unit_values_remove_and_revert(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/units/relay", "!!document.querySelector('[data-testid=\"remove-log.format\"]')")
    b.click('[data-testid="remove-log.format"]')
    check(b.eval("document.querySelector('[data-testid=\"row-log.format\"]').dataset.draft") == "removed", "No removed mark")
    # The value that applies now is the one of the layer below: the default.
    check(b.text('[data-testid="row-log.format"] .v-val') == "text", "The layer below does not show: %r" % b.text('[data-testid="row-log.format"] .v-val'))
    b.click('[data-testid="revert-log.format"]')
    check(b.text('[data-testid="row-log.format"] .v-val') == "json", "Revert does not restore")
    check(b.eval("document.querySelector('[data-testid=save]').disabled"), "Save is on after a revert")


@test
def conflict_flow(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/units/relay", "!!document.querySelector('[data-testid=\"edit-limits.max_clients\"]')")
    b.click('[data-testid="edit-limits.max_clients"]')
    b.fill('[data-testid="panel-limits.max_clients"] [data-testid=value]', "900")
    b.click('[data-testid="panel-limits.max_clients"] [data-testid=done]')
    # Another person saves the same document.
    status, cur, hdr = srv.call("GET", "api/environments/production/units/relay/values")
    etag = hdr["ETag"].strip('"')
    cur["values"]["limits.max_clients"] = 700
    status, res, _ = srv.call("PUT", "api/environments/production/units/relay/values", {"document": cur, "message": "other"}, {"If-Match": '"%s"' % etag})
    check(status == 200, "Other write: %s %s" % (status, res))
    b.click('[data-testid="save"]')
    b.wait("!!document.querySelector('[data-testid=conflict]')")
    check(b.count('[data-testid=conflict] .diff__line--add') >= 1 and b.count('[data-testid=conflict] .diff__line--del') >= 1, "The diff of the conflict is not shown")
    check("900" in b.text('[data-testid=conflict] .diff__line--add .diff__text') and "700" in b.text('[data-testid=conflict] .diff__line--del .diff__text'), "The two versions are not in the diff")
    b.click('[data-testid="conflict-keep"]')
    b.wait("!document.querySelector('[data-testid=conflict]')")
    b.wait("!document.querySelector('[data-testid=save]').disabled")
    b.click('[data-testid="save"]')
    b.wait("document.querySelector('[data-testid=save-status]').textContent.startsWith('Saved')")
    status, doc, _ = srv.call("GET", "api/environments/production/units/relay/values")
    check(doc["values"]["limits.max_clients"] == 900, "The kept edit is not saved")
    no_errors(b, allow=(409,))


@test
def conflict_use_current(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/units/relay", "!!document.querySelector('[data-testid=\"edit-limits.max_clients\"]')")
    b.click('[data-testid="edit-limits.max_clients"]')
    b.fill('[data-testid="panel-limits.max_clients"] [data-testid=value]', "900")
    b.click('[data-testid="panel-limits.max_clients"] [data-testid=done]')
    status, cur, hdr = srv.call("GET", "api/environments/production/units/relay/values")
    cur["values"]["limits.max_clients"] = 700
    srv.call("PUT", "api/environments/production/units/relay/values", {"document": cur}, {"If-Match": hdr["ETag"]})
    b.click('[data-testid="save"]')
    b.wait("!!document.querySelector('[data-testid=conflict]')")
    b.click('[data-testid="conflict-use"]')
    b.wait("document.querySelector('[data-testid=\"row-limits.max_clients\"] .v-val').textContent === '700'")
    check(b.eval("document.querySelector('[data-testid=save]').disabled"), "Save is on after use current")


@test
def forbidden_shows_action(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/units/relay", "!!document.querySelector('[data-testid=\"edit-limits.max_clients\"]')")
    srv.fake("deny", {"actions": ["config.write"]})
    b.click('[data-testid="edit-limits.max_clients"]')
    b.fill('[data-testid="panel-limits.max_clients"] [data-testid=value]', "250")
    b.click('[data-testid="panel-limits.max_clients"] [data-testid=done]')
    b.click('[data-testid="save"]')
    b.wait("!!document.querySelector('.notice--error')")
    text = b.text('.notice--error')
    check("config.write" in text, "The notice has no action: %s" % text)
    check("pdp" not in text.lower(), "A PDP detail shows")
    srv.fake("deny", {"actions": []})


@test
def invalid_server_error_marks_row(b, srv):
    # A value that the page accepts but the server refuses: the fake refuses a text of the key net.public_name that is not a DNS name... the page checks it too.
    reset(srv)
    open_page(b, srv, "e/production/units/relay", "!!document.querySelector('[data-testid=\"edit-net.public_name\"]')")
    b.click('[data-testid="edit-net.public_name"]')
    b.fill('[data-testid="panel-net.public_name"] [data-testid=value]', "bad name")
    check(b.eval("document.querySelector('[data-testid=\"panel-net.public_name\"] [data-testid=done]').disabled"), "A name with a space is accepted")
    b.fill('[data-testid="panel-net.public_name"] [data-testid=value]', "relay.example.invalid")
    b.click('[data-testid="panel-net.public_name"] [data-testid=done]')
    check(b.text('[data-testid="row-net.public_name"] .v-val') == "relay.example.invalid", "The value is not in the row")


@test
def secret_set_and_delete(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/units/relay", "!!document.querySelector('[data-testid=\"secret-upstream.token\"]')")
    b.click('[data-testid="secret-upstream.token"]')
    b.fill('[data-testid="panel-upstream.token"] [data-testid=secret-value]', "short")
    check(b.eval("document.querySelector('[data-testid=\"panel-upstream.token\"] [data-testid=secret-set]').disabled"), "A short secret is accepted")
    b.fill('[data-testid="panel-upstream.token"] [data-testid=secret-value]', "unit-token-0123456789")
    b.click('[data-testid="panel-upstream.token"] [data-testid=secret-set]')
    b.wait("!document.querySelector('[data-testid=\"panel-upstream.token\"]')")
    layer = b.text('[data-testid="row-upstream.token"] .v-layer')
    check(layer.strip() == "unit", "The unit layer does not show: %r" % layer)
    html = b.eval("document.documentElement.outerHTML")
    check("unit-token-0123456789" not in html, "The secret value is in the page")
    check(b.eval("document.querySelector('[data-testid=\"panel-upstream.token\"]') === null"), "The panel stays")
    # delete
    b.click('[data-testid="secret-upstream.token"]')
    b.click('[data-testid="panel-upstream.token"] [data-testid=secret-delete]')
    b.click('[data-testid="confirm-secret-yes"]')
    b.wait("document.querySelector('[data-testid=\"row-upstream.token\"] .v-layer').textContent.trim() === 'environment'")
    no_errors(b)


@test
def secrets_tab(b, srv):
    reset(srv)
    open_page(b, srv, "e/production?tab=secrets", "!!document.querySelector('[data-testid=\"secret-upstream.token\"]')")
    b.fill('[data-testid="secret-name"]', "upstream.token")
    b.fill('[data-testid="secret-value"]', "environment-token-abcdef")
    b.click('[data-testid="secret-set"]')
    b.wait("document.querySelector('[data-testid=\"secret-value\"]').value === ''")
    check("environment-token-abcdef" not in b.eval("document.documentElement.outerHTML"), "A secret value is in the page")
    b.click('[data-testid="delete-upstream.token"]')
    b.click('[data-testid="confirm-secret-delete-yes"]')
    b.wait("!document.querySelector('[data-testid=\"secret-upstream.token\"]')")


@test
def common_values_edit(b, srv):
    reset(srv)
    open_page(b, srv, "e/production?tab=common", "!!document.querySelector('[data-testid=\"edit-log.level\"]')")
    b.click('[data-testid="edit-log.level"]')
    # log.level is an enum in two schemas: the common value must be in both lists.
    opts = b.eval("[...document.querySelectorAll('[data-testid=\"panel-log.level\"] select option')].map(o => o.value)")
    check([o for o in opts if o] == ["warn", "info", "debug"], "options %s" % opts)
    b.fill('[data-testid="panel-log.level"] [data-testid=value]', "warn")
    b.click('[data-testid="panel-log.level"] [data-testid=done]')
    b.click('[data-testid="save"]')
    b.wait("document.querySelector('[data-testid=save-status]').textContent.startsWith('Saved')")
    status, doc, _ = srv.call("GET", "api/environments/production/common")
    check(doc["values"]["log.level"] == "warn", "common not saved: %s" % doc)


@test
def host_create_edit_delete(b, srv):
    reset(srv)
    open_page(b, srv, "e/production?tab=hosts&new=1", "!!document.querySelector('[data-testid=create]')")
    b.fill('[data-testid="id"]', "host-z")
    b.fill('[data-testid="address"]', "300.1.1.1")
    check(b.eval("document.querySelector('[data-testid=create]').disabled"), "A bad address is accepted")
    b.fill('[data-testid="address"]', "198.51.100.7")
    b.fill('[data-testid="port"]', "70000")
    check(b.eval("document.querySelector('[data-testid=create]').disabled"), "A bad port is accepted")
    b.fill('[data-testid="port"]', "2200")
    b.eval("(() => { const i = document.querySelector('[data-testid=label-input]'); i.focus(); i.value = 'canary'; i.dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true})); })()")
    b.wait("!!document.querySelector('[data-testid=label-canary]')")
    b.click('[data-testid="create"]')
    b.wait("location.pathname === '/e/production/hosts/host-z'")
    b.wait("!!document.querySelector('[data-testid=delete]')")
    status, doc, _ = srv.call("GET", "api/environments/production/hosts/host-z")
    check(doc["address"] == "198.51.100.7" and doc["port"] == 2200 and doc["labels"] == ["canary"], "host doc %s" % doc)
    b.fill('[data-testid="address"]', "host-z.example.invalid")
    b.click('[data-testid="save"]')
    b.wait("document.querySelector('.note-line[role=status]').textContent.startsWith('Saved')")
    b.click('[data-testid="delete"]')
    b.click('[data-testid="confirm-delete-yes"]')
    b.wait("location.pathname === '/e/production' && location.search === '?tab=hosts'")


@test
def host_in_use_delete_refused(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/hosts/host-a", "!!document.querySelector('[data-testid=delete]')")
    b.click('[data-testid="delete"]')
    b.click('[data-testid="confirm-delete-yes"]')
    b.wait("!!document.querySelector('.errors:not([hidden])')")


@test
def unit_create_and_hosts_edit(b, srv):
    reset(srv)
    open_page(b, srv, "e/production?tab=units&new=1", "!!document.querySelector('[data-testid=create]')")
    b.fill('[data-testid="id"]', "newunit")
    b.fill('[data-testid="label"]', "A new unit")
    b.fill('[data-testid="schema"]', "feed")
    b.fill('[data-testid="playbook-deploy"]', "deploy-feed")
    b.click('[data-testid="host-host-f"]')
    check(not b.eval("document.querySelector('[data-testid=create]').disabled"), "Create is off with valid fields")
    b.click('[data-testid="create"]')
    b.wait("location.pathname === '/e/production/units/newunit'")
    b.wait("!!document.querySelector('[data-testid=hosts-section]') && !!document.querySelector('[data-testid=\"host-host-f\"]')")
    b.click('[data-testid="edit-hosts"]')
    b.click('[data-testid="hostbox-list"] [data-testid="host-host-e"]')
    b.click('[data-testid="hosts-apply"]')
    b.wait("document.querySelectorAll('[data-testid=hosts-section] table [data-testid^=host-]').length === 2")
    status, doc, _ = srv.call("GET", "api/environments/production/units/newunit")
    check(doc["hosts"] == ["host-e", "host-f"], "unit hosts %s" % doc["hosts"])


@test
def history_and_diff(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/units/relay/history", "!!document.querySelector('[data-testid=diff-view]')")
    check(b.count("[data-testid=diff-view] .diff__line--add") == 1, "The diff of the two newest versions is not shown")
    b.click('[data-testid="pick-1"]')
    b.click('[data-testid="compare"]')
    b.wait("document.querySelector('[data-testid=diff-view] .diff__title').textContent.startsWith('Version 1 to version')")
    b.click('[data-testid="doc-unit"]')
    b.wait("location.search === '?doc=unit' && document.querySelectorAll('[data-testid^=version-]').length === 1")
    no_errors(b)


@test
def run_flow_check_then_apply(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/units/relay/run", "!!document.querySelector('[data-testid=start-check]')")
    check(not b.eval("document.querySelector('[data-testid=start-check]').disabled"), "Start check is off with all hosts picked")
    b.fill('[data-testid="run-message"]', "Check for the test")
    b.click('[data-testid="start-check"]')
    b.wait("location.search.startsWith('?check=')", 8)
    b.wait("!!document.querySelector('[data-testid=run-view]')", 8)
    b.wait("!!document.querySelector('[data-testid=apply]')", 25)
    check(b.count("[data-testid=host-host-a][data-host], [data-testid=run-hosts] tr[data-testid^=host-]") >= 3, "The host rows are missing")
    check(not b.eval("document.querySelector('[data-testid=apply]').disabled"), "Apply is off")
    out = b.text("[data-testid=run-output]")
    check("PLAY RECAP" in out, "The output is not complete")
    check("\u241b" in out and "\u240d" in out, "The control characters are not visible")
    check("\x1b" not in out and "\r" not in out, "A raw control character is in the output")
    b.click('[data-testid="apply"]')
    b.wait("location.pathname.startsWith('/runs/')", 8)
    b.wait("document.querySelector('[data-testid=run-facts]') && document.querySelector('[data-testid=run-facts]').textContent.includes('Check')")
    b.wait("document.querySelector('[data-testid=run-facts] .dot--ok') !== null", 25)
    no_errors(b)


@test
def run_flow_reload_shows_the_check(b, srv):
    reset(srv)
    status, rec, _ = srv.call("POST", "api/runs", {"kind": "deploy", "mode": "check", "node": "environment:production/unit:relay", "hosts": ["host-a", "host-b"]})
    check(status == 201, "create run: %s %s" % (status, rec))
    open_page(b, srv, "e/production/units/relay/run?check=" + rec["id"], "!!document.querySelector('[data-testid=run-view]')")
    b.wait("!!document.querySelector('[data-testid=apply]')", 25)
    check(b.eval("location.search") == "?check=" + rec["id"], "The address lost the check")
    no_errors(b)


@test
def run_flow_drops_a_check_of_another_unit_or_kind(b, srv):
    reset(srv)
    # A check of the unit `feed`, named in the address of the unit `relay`: the page must not offer to apply it.
    status, other, _ = srv.call("POST", "api/runs", {"kind": "deploy", "mode": "check", "node": "environment:production/unit:feed", "hosts": ["host-d"]})
    check(status == 201, "create run: %s %s" % (status, other))
    open_page(b, srv, "e/production/units/relay/run?check=" + other["id"], "!!document.querySelector('[data-testid=start-check]')")
    time.sleep(1.5)
    check(not b.eval("!!document.querySelector('[data-testid=apply]') || !!document.querySelector('[data-testid=run-view]')"), "The page shows the check of another unit")
    b.wait("location.search === ''", 5)
    check(not b.eval("document.querySelector('[data-testid=start-check]').disabled"), "Start check stays off")
    # A status run is not a check run.
    status, st, _ = srv.call("POST", "api/runs", {"kind": "status", "mode": "apply", "node": "environment:production/unit:relay", "hosts": ["host-a"]})
    check(status == 201, "create status run: %s %s" % (status, st))
    open_page(b, srv, "e/production/units/relay/run?check=" + st["id"], "!!document.querySelector('[data-testid=start-check]')")
    time.sleep(1.5)
    check(not b.eval("!!document.querySelector('[data-testid=apply]') || !!document.querySelector('[data-testid=run-view]')"), "The page shows a status run")
    # An address with a check that is not a run id gives a clean page.
    open_page(b, srv, "e/production/units/relay/run?check=..%2Fruns", "!!document.querySelector('[data-testid=start-check]')")
    check(not b.eval("!!document.querySelector('[data-testid=run-view]')"), "A bad check shows a run")
    no_errors(b, allow=(404,))


@test
def run_flow_snapshot_changed(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/units/relay/run", "!!document.querySelector('[data-testid=start-check]')")
    b.click('[data-testid="start-check"]')
    b.wait("!!document.querySelector('[data-testid=apply]')", 25)
    status, cur, hdr = srv.call("GET", "api/environments/production/units/relay/values")
    cur["values"]["limits.max_clients"] = 777
    status, _, _ = srv.call("PUT", "api/environments/production/units/relay/values", {"document": cur}, {"If-Match": hdr["ETag"]})
    check(status == 200, "write between check and apply: %s" % status)
    b.click('[data-testid="apply"]')
    b.wait("location.pathname.startsWith('/runs/')", 8)
    b.wait("document.querySelector('[data-testid=run-facts] .dot--error') !== null", 15)
    text = b.text('[data-testid=run-facts]')
    check("A value changed after the check" in text, "No reason text: %s" % text)


@test
def run_check_fails_shows_reason(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/units/relay/run", "!!document.querySelector('[data-testid=start-check]')")
    b.fill('[data-testid="run-message"]', "please fail")
    b.click('[data-testid="start-check"]')
    b.wait("!!document.querySelector('[data-testid=new-check]')", 25)
    check(b.count("[data-testid=apply]") == 0, "Apply shows after a failed check")
    check(b.count("[data-testid=run-hosts] .bad") >= 1, "The failed host is not marked")
    b.click('[data-testid="new-check"]')
    b.wait("!document.querySelector('[data-testid=run-view]')")
    check(not b.eval("document.querySelector('[data-testid=start-check]').disabled"), "Start check stays off after a reset")


@test
def run_page_cancel(b, srv):
    reset(srv)
    status, rec, _ = srv.call("POST", "api/runs", {"kind": "deploy", "mode": "check", "node": "environment:production/unit:relay", "hosts": ["host-a", "host-b"]})
    check(status == 201, "create run: %s %s" % (status, rec))
    open_page(b, srv, "runs/" + rec["id"], "!!document.querySelector('[data-testid=cancel-run]')")
    b.wait("!document.querySelector('[data-testid=cancel-run]').hidden")
    b.click('[data-testid="cancel-run"]')
    b.click('[data-testid="confirm-cancel-yes"]')
    b.wait("document.querySelector('[data-testid=run-facts] .dot--warn') !== null", 8)
    b.wait("document.querySelector('[data-testid=cancel-run]').hidden")


@test
def run_output_control_characters(b, srv):
    reset(srv)
    status, rec, _ = srv.call("POST", "api/runs", {"kind": "deploy", "mode": "check", "node": "environment:production/unit:relay", "hosts": ["host-a", "host-b"]})
    open_page(b, srv, "runs/" + rec["id"], "!!document.querySelector('[data-testid=run-view]')")
    b.wait("document.querySelector('[data-testid=run-output]').textContent.includes('PLAY RECAP')", 30)
    out = b.text("[data-testid=run-output]")
    for ch in ("\x1b", "\r", "\x07", "\u202e"):
        check(ch not in out, "A raw control character stays: %r" % ch)
    check("\u241b[2J" in out, "The ESC symbol is missing")
    check("\u240d" in out, "The CR symbol is missing")
    check("<U+202E>" in out, "The bidi mark is missing")
    ctl = b.count("[data-testid=run-output] .ctl")
    check(ctl >= 4, "The symbols are not marked: %s" % ctl)
    # An event with a message that has control characters: the details show them too.
    b.click("[data-testid^=event-more-]")
    check(b.eval("document.querySelectorAll('.evdetail').length") == 1, "No detail")
    no_errors(b)


@test
def events_details_and_hosts_table(b, srv):
    reset(srv)
    status, rec, _ = srv.call("POST", "api/runs", {"kind": "deploy", "mode": "check", "node": "environment:production/unit:relay", "hosts": ["host-a", "host-b"]})
    open_page(b, srv, "runs/" + rec["id"], "!!document.querySelector('[data-testid=run-view]')")
    b.wait("document.querySelector('[data-testid=run-facts] .dot--ok') !== null", 30)
    rows = b.count("[data-testid=run-hosts] tbody tr")
    check(rows == 2, "host rows %s" % rows)
    changed = b.eval("document.querySelector('[data-testid=\"run-hosts\"] tbody tr td:nth-child(4)').textContent")
    check(changed == "1", "changed count %r" % changed)


@test
def command_without_check(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/commands/restart-service", "!!document.querySelector('[data-testid=run-command]')")
    check(b.eval("document.querySelector('[data-testid=run-command]').disabled"), "Run is on with no input")
    b.fill('[data-testid="unit"]', "relay")
    b.click('[data-testid="host-host-a"]')
    b.fill('[data-testid="param-grace"]', "120s")
    check(b.eval("document.querySelector('[data-testid=run-command]').disabled"), "A grace of 120s is accepted: not canonical")
    err = b.text("#app-main .params .field__error")
    check("2m" in err, "No canonical hint: %s" % err)
    b.fill('[data-testid="param-grace"]', "10m")
    check(b.eval("document.querySelector('[data-testid=run-command]').disabled"), "A grace of 10m is accepted: above the range")
    b.fill('[data-testid="param-grace"]', "10s")
    check(not b.eval("document.querySelector('[data-testid=run-command]').disabled"), "Run is off with valid input")
    b.click('[data-testid="run-command"]')
    b.wait("location.pathname.startsWith('/runs/')", 8)
    b.wait("document.querySelector('[data-testid=run-facts]').textContent.includes('restart-service')")
    status, items, _ = srv.call("GET", "api/runs?limit=1")
    check(items["items"][0]["params"] == {"grace": "10s"}, "params %s" % items["items"][0])


@test
def command_with_check(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/commands/reload-config", "!!document.querySelector('[data-testid=start-check]')")
    b.fill('[data-testid="unit"]', "feed")
    b.click('[data-testid="host-host-d"]')
    b.click('[data-testid="start-check"]')
    b.wait("!!document.querySelector('[data-testid=apply]')", 25)
    b.click('[data-testid="apply"]')
    b.wait("location.pathname.startsWith('/runs/')", 8)


@test
def host_target_command(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/commands/vacuum", "!!document.querySelector('[data-testid=run-command]')")
    b.fill('[data-testid="unit"]', "relay")
    b.fill('[data-testid="host"]', "host-b")
    b.fill('[data-testid="param-mode"]', "fast")
    b.fill('[data-testid="param-dry"]', "true")
    b.fill('[data-testid="param-older_than"]', "12")
    check(not b.eval("document.querySelector('[data-testid=run-command]').disabled"), "Run is off: %s" % b.text("#app-main .params"))
    b.click('[data-testid="run-command"]')
    b.wait("location.pathname.startsWith('/runs/')", 8)
    status, items, _ = srv.call("GET", "api/runs?limit=1")
    run = items["items"][0]
    check(run["node"] == "environment:production/unit:relay/host:host-b" and run["hosts"] == ["host-b"], "node %s" % run)
    check(run["params"] == {"dry": True, "mode": "fast", "older_than": 12}, "params %s" % run["params"])


@test
def command_over_the_host_limit(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/commands/rotate-logs", "!!document.querySelector('[data-testid=run-command]')")
    b.click('[data-testid="hostbox-all"]')
    check(b.eval("document.querySelector('[data-testid=run-command]').disabled"), "Run is on with 6 hosts and a limit of 4")
    check(b.eval("document.querySelector('[data-testid=hostbox]').hasAttribute('data-over')"), "The box does not show the limit")
    check("the limit is 4" in b.text("[data-testid=hostbox-count]"), "The count does not name the limit: %s" % b.text("[data-testid=hostbox-count]"))
    b.click('[data-testid="hostbox-none"]')
    b.click('[data-testid="host-host-a"]')
    check(not b.eval("document.querySelector('[data-testid=run-command]').disabled"), "Run is off with one host")


@test
def server_refusals_show_text(b, srv):
    reset(srv)
    cases = [
        ({"status": 403, "error": "approval_cannot_wait"}, "needs an approval"),
        ({"status": 403, "error": "ceiling_exceeded", "extra": {"ceiling": "max_hosts", "limit": 2}}, "The limit is 2 hosts"),
        ({"status": 403, "error": "apply_disabled"}, "off in this mode"),
        ({"status": 503, "error": "run_queue_full"}, "queue of runs is full"),
        ({"status": 403, "error": "forbidden", "extra": {"action": "run.check"}}, "run.check"),
    ]
    for rule, text in cases:
        open_page(b, srv, "e/production/units/relay/run", "!!document.querySelector('[data-testid=start-check]')")
        srv.fake("fail", {"match": "runs", "method": "POST", **rule})
        b.click('[data-testid="start-check"]')
        b.wait("document.querySelector('#app-main .step .note-line[role=status]').textContent.length > 0")
        got = b.text("#app-main .step .note-line[role=status]")
        check(text in got, "%s: expected %r in %r" % (rule["error"], text, got))
        check(b.count("[data-testid=run-view]") == 0, "A run view shows after a refusal")
    check("pdp" not in got.lower(), "A PDP detail shows")


@test
def errors_of_the_catalog_and_the_session(b, srv):
    reset(srv)
    srv.fake("fail", {"match": "catalog", "status": 503, "error": "pdp_unavailable"})
    open_page(b, srv, "e/production/units/relay", "!!document.querySelector('[data-testid=page-error]')")
    check("decision service" in b.text("[data-testid=page-error]"), "text: %s" % b.text("[data-testid=page-error]"))
    b.click("[data-testid=page-error] button")
    b.wait("!!document.querySelector('[data-testid=\"edit-limits.max_clients\"]')")
    srv.fake("fail", {"match": "session", "status": 401, "error": "unauthenticated"})
    open_page(b, srv, "", "!!document.querySelector('[data-testid=unauthorized]')")
    check(b.count("[data-testid=app-version]") == 0, "The version shows without a session")
    srv.fake("fail", {"match": "session", "status": 503, "error": "store_unavailable"})
    open_page(b, srv, "", "!!document.querySelector('[data-testid=no-answer]')")
    check("store" in b.text("[data-testid=no-answer]"), "text: %s" % b.text("[data-testid=no-answer]"))
    check(b.count("#app-header .brand__name") == 0, "The name of the product shows without a session")


@test
def hostbox_keyboard(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/units/relay/run", "!!document.querySelector('[data-testid=hostbox-list]')")
    check(b.count("[data-testid=hostbox-list] [aria-selected=true]") == 3, "All hosts are not picked at first")
    b.eval("document.querySelector('[data-testid=\"host-host-a\"]').focus()")
    for key in (" ", "ArrowDown", " "):
        b.eval("document.activeElement.dispatchEvent(new KeyboardEvent('keydown', {key: %s, bubbles: true}))" % json.dumps(key))
    check(b.eval("[...document.querySelectorAll('[data-testid=hostbox-list] [aria-selected=false]')].map(n => n.dataset.id).join()") == "host-a,host-b", "keyboard picks: %s" % b.eval("[...document.querySelectorAll('[data-testid=hostbox-list] [aria-selected=false]')].map(n => n.dataset.id).join()"))
    check(b.eval("document.activeElement.dataset.id") == "host-b", "The focus does not follow the arrow")
    b.click('[data-testid="hostbox-all"]')
    check(b.count("[data-testid=hostbox-list] [aria-selected=true]") == 3, "Select all")


@test
def status_run(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/units/relay/run?kind=status", "!!document.querySelector('[data-testid=start-status]')")
    b.click('[data-testid="start-status"]')
    b.wait("location.pathname.startsWith('/runs/')", 8)


@test
def runs_list_filters_and_poll(b, srv):
    reset(srv)
    open_page(b, srv, "runs", "document.querySelectorAll('[data-testid^=run-]').length >= 4")
    b.fill('[data-testid="filter-status"]', "failed")
    b.wait("document.querySelectorAll('tr[data-testid^=run-]').length === 1")
    check(b.eval("location.search") == "?status=failed", "address %s" % b.eval("location.search"))
    b.fill('[data-testid="filter-status"]', "")
    b.wait("document.querySelectorAll('tr[data-testid^=run-]').length >= 4")
    srv.call("POST", "api/runs", {"kind": "deploy", "mode": "check", "node": "environment:production/unit:relay", "hosts": ["host-a"]})
    # The poll refreshes the list while a run is not final.
    b.click('[data-testid="filter-status"]')
    b.fill('[data-testid="filter-status"]', "")
    b.wait("document.querySelectorAll('tr[data-testid^=run-]').length >= 5", 8)


@test
def not_found_pages(b, srv):
    reset(srv)
    for path in ("nothing/here", "e/Bad_Id", "runs/not-a-uuid", "e/production/units/relay/unknown", "e/production?tab=zzz"):
        open_page(b, srv, path, "!!document.querySelector('[data-testid=not-found]')")
        check(b.text("[data-testid=not-found] h1") == "Page not found", "Not found text for %s" % path)
        check(b.count("[data-testid=not-found] a[href]") == 1, "No link in %s" % path)
    open_page(b, srv, "e/nothing", "!!document.querySelector('[data-testid=not-found]')")
    open_page(b, srv, "e/production/units/nothing", "!!document.querySelector('[data-testid=not-found]')")
    open_page(b, srv, "e/production/units/relay/hosts/host-f", "!!document.querySelector('[data-testid=not-found]')")


@test
def access_denied_shows_error_page_and_no_write(b, srv):
    reset(srv)
    srv.fake("deny", {"actions": ["access"]})
    open_page(b, srv, "", "!!document.querySelector('[data-testid=forbidden]')")
    text = b.text("[data-testid=forbidden]")
    check("access" in text, "The action is not in the text: %s" % text)
    check(b.count("[data-testid=create], [data-testid=save], [data-testid=new-environment]") == 0, "A write control shows without a session")
    srv.fake("deny", {"actions": []})


@test
def theme_menu_and_persistence(b, srv):
    reset(srv)
    open_page(b, srv, "", "!!document.querySelector('[data-testid=theme-toggle]')")
    b.click('[data-testid="theme-toggle"]')
    b.click('[data-testid="theme-dark"]')
    check(b.eval("document.documentElement.getAttribute('data-theme')") == "dark", "Dark does not apply")
    b.goto(srv.url)
    b.wait("document.documentElement.getAttribute('data-pupitre-hold') === null")
    check(b.eval("document.documentElement.getAttribute('data-theme')") == "dark", "The theme is lost at a reload")
    b.eval("localStorage.removeItem('pupitre.theme')")


@test
def leave_guard(b, srv):
    reset(srv)
    open_page(b, srv, "e/production/units/relay", "!!document.querySelector('[data-testid=\"edit-limits.max_clients\"]')")
    b.click('[data-testid="edit-limits.max_clients"]')
    b.fill('[data-testid="panel-limits.max_clients"] [data-testid=value]', "250")
    b.click('[data-testid="panel-limits.max_clients"] [data-testid=done]')
    b.click('a.crumbs__item--link')
    b.wait("!!document.querySelector('[data-testid=leave-dialog]')")
    check(b.eval("location.pathname") == "/e/production/units/relay", "The page left without a question")
    b.eval("document.querySelector('[data-testid=leave-dialog] .btn').click()")
    b.wait("!document.querySelector('[data-testid=leave-dialog]')")
    check(b.eval("location.pathname") == "/e/production/units/relay", "Stay did not stay")


# --- the layouts --------------------------------------------------------------------------------------------------------------------------

SHOT_ROUTES = [
    ("environments", ""),
    ("environment-units", "e/production"),
    ("environment-hosts", "e/production?tab=hosts"),
    ("environment-common", "e/production?tab=common"),
    ("environment-secrets", "e/production?tab=secrets"),
    ("environment-runs", "e/production?tab=runs"),
    ("unit", "e/production/units/relay"),
    ("unit-host", "e/production/units/relay/hosts/host-a"),
    ("host", "e/production/hosts/host-a"),
    ("history", "e/production/units/relay/history"),
    ("run-unit", "e/production/units/relay/run"),
    ("commands", "e/production/commands"),
    ("command", "e/production/commands/vacuum"),
    ("runs", "runs"),
    ("not-found", "nothing/here"),
    ("new-unit", "e/production?tab=units&new=1"),
    ("new-host", "e/production?tab=hosts&new=1"),
    ("new-environment", "?new=1"),
]


def layouts(b, srv, shots=None):
    reset(srv)
    status, rec, _ = srv.call("POST", "api/runs", {"kind": "deploy", "mode": "check", "node": "environment:production/unit:relay", "hosts": ["host-a", "host-b"]})
    routes = SHOT_ROUTES + [("run", "runs/" + rec["id"])]
    time.sleep(7)  # the run ends
    problems = []
    for width, height in ((1920, 1080), (390, 844)):
        for dark in (False, True):
            b.viewport(width, height, dark=dark, mobile=(width < 500))
            for name, path in routes:
                b.take_errors()
                b.goto(srv.url + path)
                b.wait("document.documentElement.getAttribute('data-pupitre-hold') === null", 10)
                time.sleep(0.6)
                errors = b.take_errors()
                over = b.eval("document.documentElement.scrollWidth - document.documentElement.clientWidth")
                theme = b.eval("document.documentElement.getAttribute('data-theme')")
                if errors:
                    problems.append("%s %dx%d %s: errors %s" % (name, width, height, theme, errors))
                if over > 0:
                    wide = b.eval("[...document.querySelectorAll('body *')].filter(n => n.getBoundingClientRect().right > document.documentElement.clientWidth + 1).slice(0, 3).map(n => n.tagName + '.' + n.className)")
                    problems.append("%s %dx%d %s: horizontal scroll of %dpx, for example %s" % (name, width, height, theme, over, wide))
                if width == 1920 and not dark:
                    unnamed = b.eval("""[...document.querySelectorAll('#app-header button, #app-header a, #app-main button, #app-main a, #app-main input, #app-main select, #app-main textarea')]
                        .filter(n => !n.hidden && n.getAttribute('type') !== 'hidden')
                        .filter(n => {
                          const label = (n.getAttribute('aria-label') || n.getAttribute('aria-labelledby') || n.innerText || n.textContent || '').trim();
                          const byFor = n.id && document.querySelector('label[for="' + n.id + '"]');
                          return !label && !byFor && !n.getAttribute('title');
                        }).slice(0, 3).map(n => n.tagName + '.' + n.className + '[' + (n.dataset.testid || '') + ']')""")
                    if unnamed:
                        problems.append("%s: controls with no accessible name: %s" % (name, unnamed))
                if theme != ("dark" if dark else "light"):
                    problems.append("%s: theme is %s, expected %s" % (name, theme, "dark" if dark else "light"))
                if shots:
                    os.makedirs(shots, exist_ok=True)
                    b.screenshot(os.path.join(shots, "%s-%s-%d.png" % (name, "dark" if dark else "light", width)))
    b.viewport(1440, 900)
    check(not problems, "Layout problems:\n  " + "\n  ".join(problems))


@test
def layouts_all_pages(b, srv):
    layouts(b, srv, ARGS.shots)


# --- the other modes -------------------------------------------------------------------------------------------------------------------------

def other_server(b, args, fn):
    srv = Server(*args)
    try:
        fn(b, srv)
    finally:
        srv.stop()


@test
def base_path(b, srv):
    def run(b, s):
        open_page(b, s, "", "document.querySelectorAll('[data-testid^=row-]').length === 3")
        check(b.eval("location.pathname") == "/console/", "path %s" % b.eval("location.pathname"))
        b.click('[data-testid="row-production"] a')
        b.wait("location.pathname === '/console/e/production'")
        b.click('[data-testid="row-relay"] a')
        b.wait("!!document.querySelector('[data-testid=\"edit-limits.max_clients\"]')")
        check(b.eval("location.pathname") == "/console/e/production/units/relay", "path %s" % b.eval("location.pathname"))
        b.click('[data-testid="link-runs"]')
        b.wait("location.pathname === '/console/runs'")
        open_page(b, s, "e/production/units/relay/history", "!!document.querySelector('[data-testid=diff-view]')")
        no_errors(b)
    other_server(b, ["--base-path", "/console/"], run)


@test
def mode_open_no_apply(b, srv):
    def run(b, s):
        open_page(b, s, "e/production", "!!document.querySelector('[data-testid=tab-units]')")
        check(b.count("[data-testid=commands]") == 0, "Commands show with apply off")
        check(b.count("[data-testid=subject]:not([hidden])") == 0, "The subject shows in the mode open")
        open_page(b, s, "e/production/units/relay/run", "!!document.querySelector('[data-testid=start-check]')")
        b.click('[data-testid="start-check"]')
        b.wait("!!document.querySelector('[data-testid=check-result] .callout--success')", 30)
        b.wait("!!document.querySelector('[data-testid=apply-box] .callout--info')")
        check(b.count("[data-testid=apply]") == 0, "Apply shows with apply off")
        open_page(b, s, "e/production/commands", "!!document.querySelector('[data-testid=not-found]')")
    other_server(b, ["--mode", "open", "--no-apply"], run)


@test
def no_runner(b, srv):
    def run(b, s):
        open_page(b, s, "e/production", "!!document.querySelector('[data-testid=tab-units]')")
        check(b.count("[data-testid=link-runs]") == 0 and b.count("[data-testid=tab-runs]") == 0 and b.count("[data-testid=commands]") == 0, "A run control shows")
        open_page(b, s, "runs", "!!document.querySelector('[data-testid=not-found]')")
        open_page(b, s, "e/production/units/relay", "!!document.querySelector('[data-testid=\"edit-limits.max_clients\"]')")
        check(b.count("[data-testid=check-apply]") == 0, "Check and apply shows with no runner")
        # a unit still names playbooks by id: the form asks for the ids
        open_page(b, s, "e/production?tab=units&new=1", "!!document.querySelector('[data-testid=create]')")
        check(b.eval("document.querySelector('[data-testid=playbook-deploy]').tagName") == "INPUT", "The form shows a list with no catalog")
    other_server(b, ["--no-runner"], run)


@test
def big_store(b, srv):
    """The sizes of 1.8: 64 environments, 1,000 hosts and 300 units in one environment, 100 hosts in a unit, a schema of 500 keys, 600 runs."""
    def run(b, s):
        def timed(path, ready, limit=4.0):
            start = time.time()
            open_page(b, s, path, ready)
            took = time.time() - start
            check(took < limit, "%s needs %.1fs, the limit is %.1fs" % (path, took, limit))
            return took
        timed("", "document.querySelectorAll('[data-testid^=row-]').length === 64")
        timed("e/large?tab=hosts", "document.querySelectorAll('[data-testid^=row-]').length === 1000")
        timed("e/large", "document.querySelectorAll('[data-testid^=row-]').length === 299 + 1")
        timed("e/large/units/bulk", "document.querySelectorAll('[data-testid=values-body] tr').length > 500")
        check(b.eval("document.documentElement.scrollWidth <= document.documentElement.clientWidth"), "horizontal scroll with 500 keys")
        b.fill('[data-testid="values-search"]', "svc007")
        n = b.eval("document.querySelectorAll('[data-testid=values-body] tr:not([hidden])').length")
        check(0 < n < 8, "The filter keeps %s rows" % n)
        b.click('[data-testid="edit-svc007.limit"]')
        b.fill('[data-testid="panel-svc007.limit"] [data-testid=value]', "42")
        b.click('[data-testid="panel-svc007.limit"] [data-testid=done]')
        b.click('[data-testid="save"]')
        b.wait("document.querySelector('[data-testid=save-status]').textContent.startsWith('Saved')")
        timed("e/large?tab=units&new=1", "document.querySelectorAll('[data-testid^=host-h-]').length === 1000", 5.0)
        open_page(b, s, "runs", "document.querySelectorAll('tr[data-testid^=run-]').length === 100")
        b.eval("document.querySelector('[data-testid=runs-more]').scrollIntoView()")
        b.wait("document.querySelectorAll('tr[data-testid^=run-]').length >= 200", 8)
        no_errors(b)
    other_server(b, ["--fixture", "big"], run)


@test
def document_size_limit(b, srv):
    def run(b, s):
        open_page(b, s, "e/production/units/relay", "!!document.querySelector('[data-testid=\"edit-limits.max_idle\"]')")
        b.click('[data-testid="edit-limits.max_idle"]')
        b.fill('[data-testid="panel-limits.max_idle"] [data-testid=value]', "30s")
        b.fill('[data-testid="panel-limits.max_idle"] [data-testid=note]', "x" * 480)
        b.click('[data-testid="panel-limits.max_idle"] [data-testid=done]')
        b.click('[data-testid="save"]')
        b.wait("document.querySelector('[data-testid=save-status]').textContent.includes('The limit is 600 bytes')")
        status, log, _ = s.call("GET", "/__fake/log", absolute=True)
        check(not [e for e in log["items"] if e["op"] == "put"], "The page sent a document above the limit")
        no_errors(b)
    other_server(b, ["--max-document-bytes", "600"], run)


@test
def empty_store(b, srv):
    def run(b, s):
        open_page(b, s, "", "!!document.querySelector('[data-testid=list-empty]')")
        no_errors(b)
    other_server(b, ["--fixture", "empty"], run)


@test
def slow_server_no_flash(b, srv):
    def run(b, s):
        b.take_errors()
        b.goto(s.url + "e/production/units/relay")
        # While the data loads, the page shows no header text of the product before the session (5.23).
        b.wait("document.documentElement.getAttribute('data-pupitre-hold') === null", 15)
        b.wait("!!document.querySelector('[data-testid=\"edit-limits.max_clients\"]')", 15)
        no_errors(b)
    other_server(b, ["--slow-ms", "120"], run)


# --- the runner ---------------------------------------------------------------------------------------------------------------------------------

ARGS = None


def main():
    global ARGS
    ap = argparse.ArgumentParser()
    ap.add_argument("--shots", default=None)
    ap.add_argument("-k", default=None)
    ap.add_argument("--list", action="store_true")
    ARGS = ap.parse_args()
    if ARGS.list:
        for t in TESTS:
            print(t.__name__)
        return 0
    if not find_browser():
        print("No browser found. The checks of the page did not run.")
        return 77
    chosen = [t for t in TESTS if not ARGS.k or any(k in t.__name__ for k in ARGS.k.split(","))]
    srv = Server()
    b = Browser()
    passed, failed = 0, []
    try:
        b.viewport(1440, 900)
        for t in chosen:
            start = time.time()
            try:
                t(b, srv)
                passed += 1
                print("ok    %s (%.1fs)" % (t.__name__, time.time() - start), flush=True)
            except Exception as e:  # a failure or a defect of the check
                failed.append(t.__name__)
                print("FAIL  %s: %s" % (t.__name__, e), flush=True)
                if not isinstance(e, Failure):
                    traceback.print_exc()
                try:
                    b.viewport(1440, 900)
                except Exception:
                    pass
    finally:
        b.close()
        srv.stop()
    print("%d passed, %d failed" % (passed, len(failed)))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
