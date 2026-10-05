#!/usr/bin/env python3
"""TLSCORE oracle server: one TLS 1.3 connection through Python's ssl module (OpenSSL), then exit.

usage: server.py <certdir> <leaf> <groups: any|p256>
Prints the listening port, then one result line: "OK version=... cipher=... alpn=... got=<request body bytes>"
or "HANDSHAKE-FAIL <reason>".
"""
import socket, ssl, sys

d, leaf, groups = sys.argv[1:4]
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.minimum_version = ssl.TLSVersion.TLSv1_3
ctx.maximum_version = ssl.TLSVersion.TLSv1_3
ctx.load_cert_chain(f"{d}/{leaf}_chain.pem", f"{d}/{leaf}.key")
ctx.set_alpn_protocols(["http/1.1"])
if groups == "p256":
    ctx.set_ecdh_curve("prime256v1")  # x25519 share from the client → HelloRetryRequest
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 0))
s.listen(1)
s.settimeout(30)
print(s.getsockname()[1], flush=True)
conn, _ = s.accept()
conn.settimeout(30)
try:
    t = ctx.wrap_socket(conn, server_side=True)
except Exception as e:  # the client refused us (alert) or we refused it
    print("HANDSHAKE-FAIL", repr(e), flush=True)
    sys.exit(0)
req = b""
while b"\r\n\r\n" not in req:
    c = t.recv(65536)
    if not c:
        break
    req += c
head, _, body = req.partition(b"\r\n\r\n")
lines = head.decode().split("\r\n")
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
path = lines[0].split(" ")[1]
size = int(path.rsplit("/", 1)[1]) if path.startswith("/size/") else 0
info = f"version={t.version()} cipher={t.cipher()[0]} alpn={t.selected_alpn_protocol()} got={len(body)}"
payload = (b"hello from python ssl " + info.encode() + b"\n") + bytes((i * 7) & 0xFF for i in range(size))
t.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: %d\r\nConnection: close\r\n\r\n" % len(payload) + payload)
try:
    t.unwrap()  # close_notify, then wait for the client's
except Exception as e:
    info += f" unwrap={e!r}"
print("OK", info, flush=True)
