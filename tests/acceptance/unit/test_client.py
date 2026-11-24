# SPDX-License-Identifier: MIT OR Apache-2.0
"""Unit tests of the API client (5.3 to 5.6, 5.16, 5.17) against a small fake server in this process."""
import json
import os
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from harness.client import ApiError, Client  # noqa: E402


class Fake(BaseHTTPRequestHandler):
    """Record each request. Answer from the table `ROUTES`: (method, path) to (status, headers, body)."""

    seen: list = []
    routes: dict = {}

    def _answer(self):
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        Fake.seen.append((self.command, self.path, dict(self.headers), body))
        status, headers, payload = Fake.routes.get((self.command, self.path.split("?")[0]), (404, {}, b'{"error":"not_found","message":"x","retryable":false}'))
        if callable(payload):
            payload = payload(self.path)
        self.send_response(status)
        for name, value in headers.items():
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    do_GET = do_PUT = do_POST = do_DELETE = _answer

    def log_message(self, *args):
        pass


class ClientTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Fake)
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()
        cls.client = Client("http://127.0.0.1:%d/" % cls.server.server_address[1])

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def setUp(self):
        Fake.seen = []
        Fake.routes = {}

    def test_a_create_sends_if_none_match_and_the_body_of_5_5(self):
        Fake.routes[("PUT", "/api/environments/e/hosts/h")] = (201, {"ETag": '"%s"' % ("a" * 64)}, b'{"etag":"%s","version":1}' % (b"a" * 64))
        answer = self.client.create_document("/api/environments/e/hosts/h", {"format": 1}, "why")
        method, path, headers, body = Fake.seen[0]
        self.assertEqual(headers["If-None-Match"], "*")
        self.assertNotIn("If-Match", headers)
        self.assertEqual(headers["Content-Type"], "application/json")
        self.assertEqual(json.loads(body), {"document": {"format": 1}, "message": "why"})
        self.assertEqual(answer.etag, "a" * 64)

    def test_a_save_sends_a_quoted_if_match(self):
        Fake.routes[("PUT", "/x")] = (200, {}, b"{}")
        self.client.save_document("/x", {"format": 1}, "b" * 64)
        self.assertEqual(Fake.seen[0][2]["If-Match"], '"%s"' % ("b" * 64))
        self.assertNotIn("If-None-Match", Fake.seen[0][2])
        self.assertNotIn("message", json.loads(Fake.seen[0][3]), "an empty message is absent (5.5)")

    def test_an_error_answer_raises_with_the_code(self):
        Fake.routes[("POST", "/api/runs")] = (409, {}, b'{"error":"check_used","message":"m","retryable":false}')
        with self.assertRaises(ApiError) as caught:
            self.client.start_run({"kind": "deploy"})
        self.assertEqual((caught.exception.status, caught.exception.code), (409, "check_used"))

    def test_an_expected_status_does_not_raise(self):
        Fake.routes[("POST", "/api/runs")] = (422, {}, b'{"error":"invalid_run_request","message":"m","retryable":false,"errors":[]}')
        answer = self.client.start_run({}, expect=(201, 422))
        self.assertEqual(answer.status, 422)

    def test_replace_creates_then_saves(self):
        Fake.routes[("GET", "/d")] = (404, {}, b"{}")
        Fake.routes[("PUT", "/d")] = (201, {}, b"{}")
        self.client.replace_document("/d", {"format": 1})
        self.assertEqual(Fake.seen[-1][2]["If-None-Match"], "*")
        Fake.routes[("GET", "/d")] = (200, {"ETag": '"%s"' % ("c" * 64)}, b"{}")
        Fake.routes[("PUT", "/d")] = (200, {}, b"{}")
        self.client.replace_document("/d", {"format": 1})
        self.assertEqual(Fake.seen[-1][2]["If-Match"], '"%s"' % ("c" * 64))

    def test_lists_follow_the_cursor(self):
        def page(path):
            if "after=c1" in path:
                return b'{"items":[3]}'
            return b'{"items":[1,2],"next":"c1"}'

        Fake.routes[("GET", "/api/environments")] = (200, {}, page)
        self.assertEqual(self.client.list_all("/api/environments", limit=2), [1, 2, 3])
        self.assertIn("limit=2", Fake.seen[0][1])

    def test_events_are_read_until_a_page_is_empty(self):
        def page(path):
            if "after=0" in path:
                return b'{"items":[{"seq":1},{"seq":2}],"last_seq":2,"complete":false}'
            return b'{"items":[],"last_seq":2,"complete":true}'

        Fake.routes[("GET", "/api/runs/r/events")] = (200, {}, page)
        self.assertEqual([e["seq"] for e in self.client.events("r")], [1, 2])

    def test_the_output_is_read_by_offset(self):
        data = b"abcdef"

        def page(path):
            offset = int(path.split("offset=")[1].split("&")[0])
            return data[offset : offset + 4]

        Fake.routes[("GET", "/api/runs/r/output")] = (200, {"X-Pupitre-Output-Length": "6", "X-Pupitre-Run-Complete": "true"}, page)
        self.assertEqual(self.client.output("r"), data)

    def test_wait_run_returns_a_final_status_and_times_out(self):
        states = iter([b'{"status":"queued"}', b'{"status":"running"}', b'{"status":"passed"}'])
        Fake.routes[("GET", "/api/runs/r")] = (200, {}, lambda _p: next(states))
        self.assertEqual(self.client.wait_run("r", step=0.01)["status"], "passed")
        Fake.routes[("GET", "/api/runs/s")] = (200, {}, b'{"status":"running"}')
        with self.assertRaises(TimeoutError):
            self.client.wait_run("s", timeout=0.2, step=0.05)

    def test_a_secret_name_is_quoted_in_the_path(self):
        Fake.routes[("PUT", "/api/environments/e/secrets/api.token")] = (200, {}, b'{"fingerprint":"0123456789abcdef"}')
        self.assertEqual(self.client.put_secret("/api/environments/e", "api.token", "v" * 8)["fingerprint"], "0123456789abcdef")

    def test_the_base_must_end_with_a_slash(self):
        with self.assertRaises(ValueError):
            Client("http://127.0.0.1:1")
        with self.assertRaises(ValueError):
            Client("https://127.0.0.1:1/")

    def test_the_base_path_prefixes_each_route(self):
        Fake.routes[("GET", "/console/api/session")] = (200, {}, b"{}")
        client = Client("http://127.0.0.1:%d/console/" % self.server.server_address[1])
        client.call("GET", "/api/session")
        self.assertEqual(Fake.seen[0][1], "/console/api/session")


if __name__ == "__main__":
    unittest.main()
