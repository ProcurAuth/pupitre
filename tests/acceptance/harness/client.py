# SPDX-License-Identifier: MIT OR Apache-2.0
"""A small client of the HTTP API of Pupitre (section 5). It uses the Python standard library only."""
import http.client
import json
import time
import urllib.parse

FINAL = ("passed", "failed", "canceled", "lost")


class ApiError(Exception):
    """An answer with a status that the caller did not expect (5.4)."""

    def __init__(self, status: int, code: str, body, method: str = "", path: str = "") -> None:
        super().__init__("%s %s gave %d %s: %s" % (method, path, status, code, body))
        self.status = status
        self.code = code
        self.body = body


class Response:
    """An answer: the status, the headers (names in lower case), and the bytes of the body."""

    def __init__(self, status: int, headers: dict, body: bytes) -> None:
        self.status = status
        self.headers = headers
        self.body = body

    def json(self):
        return json.loads(self.body.decode("utf-8"))

    @property
    def etag(self):
        """The etag without the quotes (5.5), or None."""
        value = self.headers.get("etag")
        return value.strip('"') if value else None


ACCEPTANCE_HOST = "pupitre.example.invalid"


class Client:
    """The client of one instance. `base` is `http://host:port/` and ends with the base path (5.2)."""

    def __init__(self, base: str, timeout: float = 20.0) -> None:
        parts = urllib.parse.urlsplit(base)
        if parts.scheme != "http" or not parts.hostname or not parts.path.endswith("/"):
            raise ValueError("the base is an http URL that ends with /")
        self.host = parts.hostname
        self.port = parts.port or 80
        self.base_path = parts.path
        self.timeout = timeout

    # -- the transport --------------------------------------------------------------------------------------------

    def request(self, method: str, path: str, body=None, headers: dict = None, raw_body: bytes = None) -> Response:
        """Send one request. `path` starts with `/` and has no base path. The body is a JSON value."""
        send = dict(headers or {})
        data = raw_body
        if body is not None:
            data = json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
            send.setdefault("Content-Type", "application/json")
        send.setdefault("Accept", "application/json")
        # The configurations of the harness list this one name in `server.allowed_hosts` (7.4).
        send.setdefault("Host", ACCEPTANCE_HOST)
        connection = http.client.HTTPConnection(self.host, self.port, timeout=self.timeout)
        try:
            connection.request(method, self.base_path.rstrip("/") + path, body=data, headers=send)
            answer = connection.getresponse()
            payload = answer.read()
            return Response(answer.status, {k.lower(): v for k, v in answer.getheaders()}, payload)
        finally:
            connection.close()

    def call(self, method: str, path: str, body=None, headers: dict = None, expect=(200,)) -> Response:
        """Send a request and check the status. An error answer raises `ApiError` with its code (5.4)."""
        answer = self.request(method, path, body=body, headers=headers)
        if answer.status not in expect:
            code, detail = "", answer.body[:300]
            try:
                detail = answer.json()
                code = detail.get("error", "")
            except (ValueError, AttributeError):
                pass
            raise ApiError(answer.status, code, detail, method, path)
        return answer

    # -- the documents (5.5) --------------------------------------------------------------------------------------

    def get_document(self, path: str):
        """Read a document. Return the pair (document, etag)."""
        answer = self.call("GET", path)
        return answer.json(), answer.etag

    def create_document(self, path: str, document: dict, message: str = "", expect=(201,)) -> Response:
        """`PUT` with `If-None-Match: *` (create)."""
        return self.call("PUT", path, body=_write_body(document, message), headers={"If-None-Match": "*"}, expect=expect)

    def save_document(self, path: str, document: dict, etag: str, message: str = "", expect=(200,)) -> Response:
        """`PUT` with `If-Match` (save)."""
        return self.call("PUT", path, body=_write_body(document, message), headers={"If-Match": '"%s"' % etag}, expect=expect)

    def replace_document(self, path: str, document: dict, message: str = "") -> Response:
        """Create the document, or save it over the current one."""
        answer = self.request("GET", path)
        if answer.status == 404:
            return self.create_document(path, document, message)
        return self.save_document(path, document, answer.etag, message)

    def delete_document(self, path: str, etag: str) -> Response:
        return self.call("DELETE", path, headers={"If-Match": '"%s"' % etag})

    def create_environment(self, env: str, label: str) -> Response:
        """`POST /api/environments` (5.8). An environment that exists is not an error."""
        answer = self.request("POST", "/api/environments", body={"id": env, "document": {"format": 1, "label": label}, "message": "acceptance"})
        if answer.status not in (201, 409):
            raise ApiError(answer.status, "", answer.body[:300], "POST", "/api/environments")
        return answer

    # -- the secrets (5.14) ---------------------------------------------------------------------------------------

    def put_secret(self, scope_path: str, name: str, value: str) -> dict:
        """`scope_path` is `/api/environments/<e>` or `/api/environments/<e>/units/<u>`."""
        return self.call("PUT", "%s/secrets/%s" % (scope_path, urllib.parse.quote(name, safe="")), body={"value": value}).json()

    # -- the runs (5.16 to 5.18) ----------------------------------------------------------------------------------

    def start_run(self, request: dict, expect=(201,)) -> Response:
        return self.call("POST", "/api/runs", body=request, expect=expect)

    def get_run(self, run: str) -> dict:
        return self.call("GET", "/api/runs/%s" % run).json()

    def cancel_run(self, run: str, expect=(200,)) -> Response:
        return self.call("POST", "/api/runs/%s/cancel" % run, expect=expect)

    def wait_run(self, run: str, until=FINAL, timeout: float = 120.0, step: float = 0.5) -> dict:
        """Poll a run until its status is in `until`. Return the record."""
        deadline = time.monotonic() + timeout
        record = self.get_run(run)
        while record["status"] not in until:
            if time.monotonic() >= deadline:
                raise TimeoutError("run %s has the status %s after %ss, expected one of %s" % (run, record["status"], timeout, list(until)))
            time.sleep(step)
            record = self.get_run(run)
        return record

    def events(self, run: str) -> list:
        """All events of a run (5.17), read page by page."""
        items: list = []
        after = 0
        while True:
            page = self.call("GET", "/api/runs/%s/events?after=%d" % (run, after)).json()
            items += page["items"]
            if not page["items"]:
                return items
            after = page["last_seq"]

    def output(self, run: str) -> bytes:
        """The stored output of a run (5.17), read page by page."""
        chunks: list = []
        offset = 0
        while True:
            answer = self.call("GET", "/api/runs/%s/output?offset=%d&limit=1048576" % (run, offset))
            chunks.append(answer.body)
            offset += len(answer.body)
            if not answer.body or offset >= int(answer.headers["x-pupitre-output-length"]):
                return b"".join(chunks)

    def list_all(self, path: str, limit: int = 200) -> list:
        """All items of a list route (5.6)."""
        items: list = []
        after = None
        separator = "&" if "?" in path else "?"
        while True:
            suffix = "%slimit=%d" % (separator, limit) + ("&after=%s" % after if after else "")
            page = self.call("GET", path + suffix).json()
            items += page["items"]
            after = page.get("next")
            if not after:
                return items


def _write_body(document: dict, message: str) -> dict:
    body = {"document": document}
    if message:
        body["message"] = message
    return body
