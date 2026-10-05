#!/usr/bin/env python3
"""HTTPCORE oracle server: Python's http.server (HTTP/1.1, keep-alive) on 127.0.0.1, optionally under Python ssl
(OpenSSL, TLS 1.3 ONLY, ALPN http/1.1). It is NOT ours, which is the point: http_core's host client must speak
to it byte-correctly.

usage: http_server.py <certdir|-> [leaf]     prints the port, then serves until killed.

Routes: /hello (Content-Length), /chunked (awkward chunks + a trailer), /gzip and /deflate (Content-Encoding,
chunked), /redirect (302 + Set-Cookie) -> /final (echoes Cookie), /redirect307 (POST kept), /echo (any method:
JSON of what arrived), /sse (paced text/event-stream), /stats (connections accepted, last ALPN), /page (the
Chromium oracle page: gzip when asked).
"""
import gzip, hashlib, json, ssl, sys, threading, time, zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

STATS = {"connections": 0, "alpn": None, "version": None}
LOCK = threading.Lock()
BIG = ("".join(f"line {i:05d}: the quick brown fox jumps over the lazy dog\n" for i in range(2000))).encode()
PAGE = (b"<!doctype html><html><head><meta charset=utf-8><title>HTTPCORE oracle</title></head><body>"
        + b"<h1>Una \xc3\xa9t\xc3\xa9</h1>" + b"".join(b"<p>paragraph %d &amp; more</p>\n" % i for i in range(400))
        + b"</body></html>\n")


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def setup(self):
        super().setup()
        with LOCK:
            STATS["connections"] += 1
            if isinstance(self.connection, ssl.SSLSocket):
                STATS["alpn"] = self.connection.selected_alpn_protocol()
                STATS["version"] = self.connection.version()

    def body(self):
        n = int(self.headers.get("content-length", "0") or 0)
        return self.rfile.read(n) if n else b""

    def send_chunked(self, parts, trailer=None, extra=()):
        self.send_header("Transfer-Encoding", "chunked")
        if trailer:
            self.send_header("Trailer", trailer[0])
        for k, v in extra:
            self.send_header(k, v)
        self.end_headers()
        for p in parts:
            self.wfile.write(b"%x;ext=1\r\n%s\r\n" % (len(p), p))
            self.wfile.flush()
        self.wfile.write(b"0\r\n")
        if trailer:
            self.wfile.write(("%s: %s\r\n" % trailer).encode())
        self.wfile.write(b"\r\n")
        self.wfile.flush()

    def fixed(self, code, body, ctype="text/plain", extra=()):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        for k, v in extra:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def handle_any(self):
        p = self.path.split("?")[0]
        if p == "/hello":
            return self.fixed(200, b"hello from python\n")
        if p == "/stats":
            with LOCK:
                return self.fixed(200, json.dumps(STATS).encode(), "application/json")
        if p == "/chunked":
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            parts, pos, sizes = [], 0, [1, 7001, 13, 4096, 2]
            while pos < len(BIG):
                n = sizes[len(parts) % len(sizes)]
                parts.append(BIG[pos:pos + n])
                pos += n
            return self.send_chunked(parts, ("X-Checksum", hashlib.sha256(BIG).hexdigest()))
        if p in ("/gzip", "/deflate", "/page"):
            data = PAGE if p == "/page" else BIG
            ae = self.headers.get("accept-encoding", "")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8" if p == "/page" else "text/plain")
            enc = None
            if p == "/deflate" and "deflate" in ae:
                enc, z = "deflate", zlib.compress(data)
            elif "gzip" in ae:
                enc, z = "gzip", gzip.compress(data, mtime=0)
            else:
                z = data
            extra = [("Content-Encoding", enc)] if enc else []
            return self.send_chunked([z[i:i + 997] for i in range(0, len(z), 997)], extra=extra)
        if p == "/redirect":
            return self.fixed(302, b"moved", extra=[("Location", "/final?from=redirect"), ("Set-Cookie", "sid=abc123; Path=/; HttpOnly")])
        if p == "/redirect307":
            self.body()
            return self.fixed(307, b"", extra=[("Location", "/echo")])
        if p == "/final":
            return self.fixed(200, ("cookie=" + (self.headers.get("cookie") or "")).encode())
        if p == "/echo":
            b = self.body()
            out = {"method": self.command, "path": self.path, "len": len(b), "sha256": hashlib.sha256(b).hexdigest(),
                   "headers": {k.lower(): v for k, v in self.headers.items()}}
            return self.fixed(200, json.dumps(out).encode(), "application/json")
        if p == "/sse":
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()
            for i in range(5):
                ev = ("event: tick\ndata: {\"n\":%d,\"t\":%f}\n\n" % (i, time.time())).encode()
                for piece in (ev[:5], ev[5:]):
                    self.wfile.write(b"%x\r\n%s\r\n" % (len(piece), piece))
                    self.wfile.flush()
                time.sleep(0.25)
            self.wfile.write(b"0\r\n\r\n")
            self.wfile.flush()
            return
        return self.fixed(404, b"no such route\n")

    do_GET = do_POST = do_PUT = do_DELETE = do_PATCH = handle_any

    def do_HEAD(self):
        self.send_response(200)
        self.send_header("Content-Length", "12345")
        self.end_headers()


srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
srv.daemon_threads = True
if sys.argv[1] != "-":
    d, leaf = sys.argv[1], sys.argv[2]
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.minimum_version = ssl.TLSVersion.TLSv1_3
    ctx.maximum_version = ssl.TLSVersion.TLSv1_3
    ctx.load_cert_chain(f"{d}/{leaf}_chain.pem", f"{d}/{leaf}.key")
    ctx.set_alpn_protocols(["http/1.1"])
    srv.socket = ctx.wrap_socket(srv.socket, server_side=True, do_handshake_on_connect=True)
print(srv.server_address[1], flush=True)
try:
    srv.serve_forever()
except KeyboardInterrupt:
    pass
