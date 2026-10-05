# SPDX-License-Identifier: LGPL-3.0-or-later
# Test server for tests/http.rs: `git http-backend` behind a tiny HTTP/1.1 server (smart mode,
# Git-Protocol forwarded as GIT_PROTOCOL so protocol v2 is negotiated), or static files (dumb).
import http.server, os, socketserver, subprocess, sys

ROOT, MODE = sys.argv[1], sys.argv[2]

class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def body(self):
        if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
            out = b""
            while True:
                n = int(self.rfile.readline().strip(), 16)
                if n == 0:
                    self.rfile.readline()
                    return out
                out += self.rfile.read(n)
                self.rfile.readline()
        n = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(n) if n else b""

    def reply(self, status, headers, payload):
        self.send_response(status)
        for k, v in headers:
            if k.lower() not in ("content-length", "status"):
                self.send_header(k, v)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def serve(self, method):
        data = self.body()
        path, _, query = self.path.partition("?")
        if MODE == "dumb":
            p = os.path.join(ROOT, path.lstrip("/"))
            if os.path.isfile(p):
                self.reply(200, [("Content-Type", "application/octet-stream")], open(p, "rb").read())
            else:
                self.reply(404, [], b"not found\n")
            return
        env = dict(os.environ, GIT_PROJECT_ROOT=ROOT, GIT_HTTP_EXPORT_ALL="1", REQUEST_METHOD=method,
                   PATH_INFO=path, QUERY_STRING=query, CONTENT_TYPE=self.headers.get("Content-Type", ""),
                   CONTENT_LENGTH=str(len(data)), REMOTE_ADDR="127.0.0.1", SERVER_PROTOCOL="HTTP/1.1")
        if self.headers.get("Git-Protocol"):
            env["GIT_PROTOCOL"] = self.headers["Git-Protocol"]
        if self.headers.get("Content-Encoding"):
            env["HTTP_CONTENT_ENCODING"] = self.headers["Content-Encoding"]
        out = subprocess.run(["git", "http-backend"], input=data, env=env, capture_output=True).stdout
        sep = out.find(b"\r\n\r\n")
        if sep < 0:
            head, payload = out, b""
        else:
            head, payload = out[:sep], out[sep + 4:]
        status, headers = 200, []
        for line in head.decode("latin-1").split("\r\n"):
            if ":" not in line:
                continue
            k, v = line.split(":", 1)
            if k.lower() == "status":
                status = int(v.strip().split()[0])
            headers.append((k.strip(), v.strip()))
        self.reply(status, headers, payload)

    def do_GET(self):
        self.serve("GET")

    def do_POST(self):
        self.serve("POST")

class S(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True

s = S(("127.0.0.1", 0), H)
print(s.server_address[1], flush=True)
s.serve_forever()
