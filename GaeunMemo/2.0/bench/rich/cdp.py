"""Minimal Chrome DevTools Protocol client (stdlib only) for headless Edge.

    with Edge() as e:
        e.open(file_uri)
        value = e.eval("1 + 1")

Used by the rich-text differential test to run v1.1's ui/rich.js in real Chromium.
Only talks to 127.0.0.1; the browser profile is a throwaway temp directory.
"""
import base64
import json
import os
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.request

EDGE = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"


class WebSocket:
    def __init__(self, url):
        assert url.startswith("ws://")
        hostport, path = url[5:].split("/", 1)
        host, port = hostport.split(":")
        self.s = socket.create_connection((host, int(port)), timeout=60)
        key = base64.b64encode(os.urandom(16)).decode()
        req = (f"GET /{path} HTTP/1.1\r\nHost: {hostport}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
               f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n")
        self.s.sendall(req.encode())
        head = b""
        while b"\r\n\r\n" not in head:
            head += self.s.recv(1)
        if b" 101 " not in head.split(b"\r\n", 1)[0]:
            raise RuntimeError(head.decode(errors="replace"))

    def _recv_exact(self, n):
        buf = bytearray()
        while len(buf) < n:
            chunk = self.s.recv(n - len(buf))
            if not chunk:
                raise ConnectionError("closed")
            buf += chunk
        return bytes(buf)

    def send(self, text):
        data = text.encode()
        head = bytearray([0x81])
        n = len(data)
        if n < 126:
            head.append(0x80 | n)
        elif n < 65536:
            head.append(0x80 | 126)
            head += n.to_bytes(2, "big")
        else:
            head.append(0x80 | 127)
            head += n.to_bytes(8, "big")
        mask = os.urandom(4)
        head += mask
        self.s.sendall(bytes(head) + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def recv(self):
        parts = []
        while True:
            b0, b1 = self._recv_exact(2)
            n = b1 & 0x7F
            if n == 126:
                n = int.from_bytes(self._recv_exact(2), "big")
            elif n == 127:
                n = int.from_bytes(self._recv_exact(8), "big")
            payload = self._recv_exact(n)
            op = b0 & 0x0F
            if op == 9:  # ping
                continue
            parts.append(payload)
            if b0 & 0x80:
                return b"".join(parts).decode()


class Edge:
    def __enter__(self):
        self.profile = tempfile.mkdtemp(prefix="gm-cdp-")
        self.proc = subprocess.Popen([EDGE, "--headless=new", f"--user-data-dir={self.profile}", "--no-first-run",
                                      "--disable-extensions", "--allow-file-access-from-files",
                                      "--remote-debugging-port=0", "about:blank"],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        port_file = os.path.join(self.profile, "DevToolsActivePort")
        for _ in range(200):
            if os.path.exists(port_file) and os.path.getsize(port_file) > 0:
                break
            time.sleep(0.05)
        self.port = int(open(port_file).read().split()[0])
        pages = json.load(urllib.request.urlopen(f"http://127.0.0.1:{self.port}/json/list"))
        page = next(p for p in pages if p.get("type") == "page")
        self.ws = WebSocket(page["webSocketDebuggerUrl"])
        self.next_id = 0
        return self

    def call(self, method, **params):
        self.next_id += 1
        mid = self.next_id
        self.ws.send(json.dumps({"id": mid, "method": method, "params": params}))
        while True:
            msg = json.loads(self.ws.recv())
            if msg.get("id") == mid:
                if "error" in msg:
                    raise RuntimeError(msg["error"])
                return msg["result"]

    def open(self, url):
        self.call("Page.enable")
        self.call("Page.navigate", url=url)
        for _ in range(200):
            if self.eval("document.readyState") == "complete":
                return
            time.sleep(0.05)

    def eval(self, expr):
        r = self.call("Runtime.evaluate", expression=expr, returnByValue=True, awaitPromise=True)
        if "exceptionDetails" in r:
            raise RuntimeError(r["exceptionDetails"].get("exception", {}).get("description", r["exceptionDetails"]))
        return r["result"].get("value")

    def __exit__(self, *a):
        try:
            self.call("Browser.close")
        except Exception:
            pass
        try:
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()
        shutil.rmtree(self.profile, ignore_errors=True)


if __name__ == "__main__":
    with Edge() as e:
        e.open("about:blank")
        print(e.eval("const d=document.createElement('div'); d.style.whiteSpace='pre-wrap'; document.body.appendChild(d);"
                     " d.innerHTML='a<div>b<br></div><div><br></div>c<div>x\\n</div>y'; JSON.stringify(d.innerText)"))
