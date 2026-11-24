# SPDX-License-Identifier: MIT OR Apache-2.0
"""A small client of the Chrome DevTools Protocol, for the checks of the page in a headless browser.

It uses the Python standard library only (a minimal WebSocket client, RFC 6455). A development tool: it is not part of the product.
"""

import base64
import json
import os
import shutil
import socket
import struct
import subprocess
import tempfile
import threading
import time
import urllib.request

BROWSERS = ("google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome")


def find_browser():
    for name in BROWSERS:
        path = shutil.which(name)
        if path:
            return path
    return None


class WebSocket:
    def __init__(self, url):
        assert url.startswith("ws://")
        host_port, _, path = url[5:].partition("/")
        host, _, port = host_port.partition(":")
        self.sock = socket.create_connection((host, int(port)), timeout=30)
        key = base64.b64encode(os.urandom(16)).decode()
        request = ("GET /%s HTTP/1.1\r\nHost: %s\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                   "Sec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\n\r\n" % (path, host_port, key))
        self.sock.sendall(request.encode())
        buf = b""
        while b"\r\n\r\n" not in buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise ConnectionError("The browser closed the connection.")
            buf += chunk
        head, _, rest = buf.partition(b"\r\n\r\n")
        if b" 101 " not in head.split(b"\r\n")[0]:
            raise ConnectionError("The WebSocket handshake failed: %r" % head[:80])
        self.buf = rest
        self.lock = threading.Lock()

    def send(self, text):
        data = text.encode()
        header = bytearray([0x81])
        n = len(data)
        if n < 126:
            header.append(0x80 | n)
        elif n < 65536:
            header.append(0x80 | 126)
            header += struct.pack(">H", n)
        else:
            header.append(0x80 | 127)
            header += struct.pack(">Q", n)
        mask = os.urandom(4)
        header += mask
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
        with self.lock:
            self.sock.sendall(bytes(header) + masked)

    def _read(self, n):
        while len(self.buf) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ConnectionError("The browser closed the connection.")
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def recv(self):
        message = b""
        while True:
            b1, b2 = self._read(2)
            fin, opcode = b1 & 0x80, b1 & 0x0F
            n = b2 & 0x7F
            if n == 126:
                n = struct.unpack(">H", self._read(2))[0]
            elif n == 127:
                n = struct.unpack(">Q", self._read(8))[0]
            payload = self._read(n)
            if opcode == 0x8:
                raise ConnectionError("The browser closed the WebSocket.")
            if opcode == 0x9:
                continue
            message += payload
            if fin:
                return message.decode()

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass


class Browser:
    """One headless browser with one page. `events` holds the console messages, the exceptions and the failed requests."""

    def __init__(self, binary=None, port=9333):
        self.binary = binary or find_browser()
        if not self.binary:
            raise RuntimeError("No browser found. Install Chrome or Chromium to run the checks of the page.")
        self.port = port
        self.profile = tempfile.mkdtemp(prefix="pupitre-page-")
        self.proc = subprocess.Popen([
            self.binary, "--headless=new", "--no-sandbox", "--disable-gpu", "--hide-scrollbars", "--no-first-run",
            "--no-default-browser-check", "--disable-extensions", "--disable-background-networking",
            "--remote-debugging-port=%d" % port, "--user-data-dir=%s" % self.profile, "about:blank"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.ws = None
        self.next_id = 0
        self.results = {}
        self.events = []
        self.errors = []
        self.requests = {}
        self._connect()

    def _connect(self):
        deadline = time.time() + 20
        target = None
        while time.time() < deadline:
            try:
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                with opener.open("http://127.0.0.1:%d/json" % self.port, timeout=2) as r:
                    targets = json.load(r)
                pages = [t for t in targets if t.get("type") == "page"]
                if pages:
                    target = pages[0]
                    break
            except Exception:
                time.sleep(0.2)
        if not target:
            raise RuntimeError("The browser did not start.")
        self.ws = WebSocket(target["webSocketDebuggerUrl"])
        self.reader = threading.Thread(target=self._loop, daemon=True)
        self.reader.start()
        for domain in ("Page", "Runtime", "Log", "Network"):
            self.call("%s.enable" % domain)
        # A page that holds changes asks "Leave this page?" with the event beforeunload. A check that leaves such a page must not wait for
        # an answer, so the browser of the checks ignores the listeners of this event. The dialog of the page itself stays testable.
        self.call("Page.addScriptToEvaluateOnNewDocument", {"source": (
            "const add = EventTarget.prototype.addEventListener;"
            "EventTarget.prototype.addEventListener = function (type, ...rest) { if (type === 'beforeunload') return; return add.call(this, type, ...rest); };")})

    def _loop(self):
        try:
            while True:
                message = json.loads(self.ws.recv())
                if "id" in message:
                    self.results[message["id"]] = message
                else:
                    self._event(message)
        except Exception:
            pass

    def _event(self, m):
        method, p = m.get("method"), m.get("params", {})
        if method == "Runtime.consoleAPICalled" and p.get("type") in ("error", "warning", "assert"):
            text = " ".join(str(a.get("value", a.get("description", ""))) for a in p.get("args", []))
            self.errors.append("console.%s: %s" % (p["type"], text))
        elif method == "Runtime.exceptionThrown":
            d = p.get("exceptionDetails", {})
            self.errors.append("exception: %s %s" % (d.get("text", ""), (d.get("exception") or {}).get("description", "")))
        elif method == "Log.entryAdded":
            e = p.get("entry", {})
            if e.get("level") in ("error", "warning"):
                self.errors.append("log.%s: %s %s" % (e["level"], e.get("text", ""), e.get("url", "")))
        elif method == "Network.responseReceived":
            r = p.get("response", {})
            self.requests[p.get("requestId")] = (r.get("status"), r.get("url"))
        elif method == "Network.loadingFailed":
            self.errors.append("request failed: %s %s" % (p.get("errorText"), p.get("requestId")))
        self.events.append(m)

    def call(self, method, params=None, timeout=30):
        self.next_id += 1
        i = self.next_id
        self.ws.send(json.dumps({"id": i, "method": method, "params": params or {}}))
        deadline = time.time() + timeout
        while time.time() < deadline:
            if i in self.results:
                r = self.results.pop(i)
                if "error" in r:
                    raise RuntimeError("%s: %s" % (method, r["error"]))
                return r.get("result", {})
            time.sleep(0.005)
        raise TimeoutError(method)

    # --- helpers
    def goto(self, url, wait_ms=0):
        self.call("Page.navigate", {"url": url})
        self.wait("document.readyState === 'complete'", 15)
        if wait_ms:
            time.sleep(wait_ms / 1000)

    def eval(self, expr, await_promise=False):
        r = self.call("Runtime.evaluate", {"expression": expr, "returnByValue": True, "awaitPromise": await_promise})
        if "exceptionDetails" in r:
            raise RuntimeError("eval failed: %s" % json.dumps(r["exceptionDetails"])[:300])
        return r["result"].get("value")

    def wait(self, expr, timeout=10):
        deadline = time.time() + timeout
        last = None
        while time.time() < deadline:
            try:
                last = self.eval(expr)
                if last:
                    return last
            except RuntimeError:
                pass
            time.sleep(0.05)
        raise TimeoutError("Timeout while the browser waited for: %s" % expr)

    def viewport(self, width, height, dark=False, mobile=False):
        self.call("Emulation.setDeviceMetricsOverride", {"width": width, "height": height, "deviceScaleFactor": 1, "mobile": mobile})
        self.call("Emulation.setEmulatedMedia", {"features": [{"name": "prefers-color-scheme", "value": "dark" if dark else "light"}]})

    def screenshot(self, path, full=False):
        r = self.call("Page.captureScreenshot", {"format": "png", "captureBeyondViewport": full})
        with open(path, "wb") as f:
            f.write(base64.b64decode(r["data"]))

    def click(self, selector):
        self.wait("!!document.querySelector(%s)" % json.dumps(selector), 8)
        self.eval("document.querySelector(%s).click()" % json.dumps(selector))

    def fill(self, selector, text):
        self.wait("!!document.querySelector(%s)" % json.dumps(selector), 8)
        self.eval("(() => { const n = document.querySelector(%s); n.focus(); n.value = %s; n.dispatchEvent(new Event('input', {bubbles: true})); n.dispatchEvent(new Event('change', {bubbles: true})); })()" % (json.dumps(selector), json.dumps(text)))

    def text(self, selector):
        return self.eval("(document.querySelector(%s) || {}).textContent" % json.dumps(selector))

    def count(self, selector):
        return self.eval("document.querySelectorAll(%s).length" % json.dumps(selector))

    def take_errors(self):
        out, self.errors = self.errors, []
        return out

    def close(self):
        try:
            if self.ws:
                self.ws.close()
        finally:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
            shutil.rmtree(self.profile, ignore_errors=True)
