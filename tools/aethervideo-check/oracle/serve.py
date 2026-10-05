#!/usr/bin/env python3
# AUDIOTRACK (SR45) oracle fixture: python's http.server with HTTP byte ranges (RFC 9110 §14),
# so Chromium can seek the http <video> and Aether's media cache takes its ranged path.
#   python3 serve.py <port> <dir>
import http.server, os, re, sys

class Ranged(http.server.SimpleHTTPRequestHandler):
    def send_head(self):
        path = self.translate_path(self.path)
        m = re.match(r"bytes=(\d+)-(\d*)$", self.headers.get("Range", ""))
        if not m or not os.path.isfile(path):
            return super().send_head()
        size = os.path.getsize(path)
        a = int(m.group(1)); b = min(int(m.group(2)) if m.group(2) else size - 1, size - 1)
        if a >= size:
            self.send_response(416); self.send_header("Content-Range", f"bytes */{size}"); self.end_headers(); return None
        f = open(path, "rb"); f.seek(a)
        self.send_response(206)
        self.send_header("Content-Type", self.guess_type(path))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Range", f"bytes {a}-{b}/{size}")
        self.send_header("Content-Length", str(b - a + 1))
        self.end_headers()
        self.range_left = b - a + 1
        return f

    def copyfile(self, src, dst):
        n = getattr(self, "range_left", None)
        if n is None:
            return super().copyfile(src, dst)
        while n > 0:
            chunk = src.read(min(65536, n))
            if not chunk: break
            dst.write(chunk); n -= len(chunk)

    def log_message(self, *a):
        pass

os.chdir(sys.argv[2])
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Ranged).serve_forever()
