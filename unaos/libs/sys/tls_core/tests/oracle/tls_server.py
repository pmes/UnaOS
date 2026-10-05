#!/usr/bin/env python3
"""TLSCORE2 oracle server: Python's ssl module (OpenSSL) on 127.0.0.1, NOT ours. Serves N connections on one
SSLContext (so TLS 1.3 session tickets issued on one connection resume on the next), one request each.

usage: tls_server.py <certdir> <leaf> [--tls 1.2|1.3|any] [--ciphers OPENSSL-NAME] [--curve NAME]
                     [--conns N] [--tickets N] [--no-alpn]
Prints the port, then one line per connection:
  OK version=<v> cipher=<c> alpn=<a> reused=<True|False> got=<request body bytes>
  HANDSHAKE-FAIL <reason>
"""
import argparse, socket, ssl, sys

ap = argparse.ArgumentParser()
ap.add_argument("certdir")
ap.add_argument("leaf")
ap.add_argument("--tls", default="1.2")
ap.add_argument("--ciphers", default=None)
ap.add_argument("--curve", default=None)
ap.add_argument("--conns", type=int, default=1)
ap.add_argument("--tickets", type=int, default=None)
ap.add_argument("--no-alpn", action="store_true")
a = ap.parse_args()

ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
V = ssl.TLSVersion
if a.tls == "1.2":
    ctx.minimum_version = V.TLSv1_2
    ctx.maximum_version = V.TLSv1_2
elif a.tls == "1.3":
    ctx.minimum_version = V.TLSv1_3
    ctx.maximum_version = V.TLSv1_3
else:
    ctx.minimum_version = V.TLSv1_2
    ctx.maximum_version = V.TLSv1_3
if a.ciphers:
    ctx.set_ciphers(a.ciphers)
if a.curve:
    ctx.set_ecdh_curve(a.curve)
if a.tickets is not None:
    ctx.num_tickets = a.tickets
ctx.load_cert_chain(f"{a.certdir}/{a.leaf}_chain.pem", f"{a.certdir}/{a.leaf}.key")
if not a.no_alpn:
    ctx.set_alpn_protocols(["http/1.1"])

s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 0))
s.listen(4)
s.settimeout(30)
print(s.getsockname()[1], flush=True)
for _ in range(a.conns):
    conn, _ = s.accept()
    conn.settimeout(30)
    try:
        t = ctx.wrap_socket(conn, server_side=True)
    except Exception as e:  # the client refused us (alert) or we refused it
        print("HANDSHAKE-FAIL", repr(e), flush=True)
        continue
    try:
        req = b""
        while b"\r\n\r\n" not in req:
            c = t.recv(65536)
            if not c:
                break
            req += c
        head, _, body = req.partition(b"\r\n\r\n")
        lines = head.decode(errors="replace").split("\r\n")
        clen = 0
        for l in lines[1:]:
            k, _, v = l.partition(":")
            if k.lower() == "content-length":
                clen = int(v)
        while len(body) < clen:
            c = t.recv(65536)
            if not c:
                break
            body += c
        path = lines[0].split(" ")[1] if " " in lines[0] else "/"
        size = int(path.rsplit("/", 1)[1]) if path.startswith("/size/") else 0
        info = (f"version={t.version()} cipher={t.cipher()[0]} alpn={t.selected_alpn_protocol()} "
                f"reused={t.session_reused} got={len(body)}")
        payload = (b"hello from python ssl " + info.encode() + b"\n") + bytes((i * 7) & 0xFF for i in range(size))
        t.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: %d\r\nConnection: close\r\n\r\n" % len(payload) + payload)
        try:
            t.unwrap()  # close_notify, then wait for the client's
        except Exception as e:
            info += f" unwrap={e!r}"
        print("OK", info, flush=True)
    except Exception as e:
        print("SERVE-FAIL", repr(e), flush=True)
