#!/usr/bin/env python3
"""VEINTLS oracle: a Messages-API-shaped HTTPS endpoint on 127.0.0.1 through Python's ssl (OpenSSL), one
connection, then exit. It is NOT ours, which is the point: vein_ring3's client (tls_core + vein_core) must
complete a verified TLS 1.3 handshake with it, POST /v1/messages, and decode its chunked SSE answer.

usage: messages_server.py <certdir> <leaf>
Prints the listening port, then ONE result line:
  OK version=<v> cipher=<c> alpn=<a> method=<m> path=<p> key=<x-api-key or -> version_hdr=<anthropic-version>
     clen=<Content-Length> got=<body bytes> stream=<true|false> model=<model> user=<first user text>
or
  HANDSHAKE-FAIL <reason> app_bytes=0
(after a failed handshake no application byte can have been read: the key never crossed the wire).
"""
import json, socket, ssl, sys

d, leaf = sys.argv[1:3]
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.minimum_version = ssl.TLSVersion.TLSv1_3
ctx.maximum_version = ssl.TLSVersion.TLSv1_3
ctx.load_cert_chain(f"{d}/{leaf}_chain.pem", f"{d}/{leaf}.key")
ctx.set_alpn_protocols(["http/1.1"])
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
except Exception as e:
    print("HANDSHAKE-FAIL", repr(e).replace("\n", " "), "app_bytes=0", flush=True)
    sys.exit(0)
req = b""
while b"\r\n\r\n" not in req:
    c = t.recv(65536)
    if not c:
        break
    req += c
head, _, body = req.partition(b"\r\n\r\n")
lines = head.decode().split("\r\n")
hdr = {}
for l in lines[1:]:
    k, _, v = l.partition(":")
    hdr[k.strip().lower()] = v.strip()
clen = int(hdr.get("content-length", "0"))
while len(body) < clen:
    c = t.recv(65536)
    if not c:
        break
    body += c
method, path = lines[0].split(" ")[0:2]
j = json.loads(body.decode())
user = next((m["content"] if isinstance(m["content"], str) else m["content"][0]["text"]) for m in j["messages"] if m["role"] == "user")

# The answer, shaped as the Messages API streams it (event names, thinking + signature deltas the client must
# skip, a ping, text deltas with a JSON escape, message_delta with the stop reason, message_stop), sent with
# chunked transfer encoding in deliberately awkward chunk sizes.
events = [
    ("message_start", {"type": "message_start", "message": {"id": "msg_veintls", "type": "message", "role": "assistant", "content": [], "model": j["model"]}}),
    ("content_block_start", {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
    ("content_block_delta", {"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "c2ln"}}),
    ("content_block_stop", {"type": "content_block_stop", "index": 0}),
    ("ping", {"type": "ping"}),
    ("content_block_start", {"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
    ("content_block_delta", {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Verified "}}),
    ("content_block_delta", {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "hello, \"" + user + "\"\n"}}),
    ("content_block_delta", {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "x" * 3000}}),
    ("content_block_stop", {"type": "content_block_stop", "index": 1}),
    ("message_delta", {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 9}}),
    ("message_stop", {"type": "message_stop"}),
]
sse = b"".join(f"event: {n}\ndata: {json.dumps(e)}\n\n".encode() for n, e in events)
out = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
i, sizes, k = 0, [1, 7, 300, 4096, 13], 0
while i < len(sse):
    n = sizes[k % len(sizes)]
    k += 1
    part = sse[i:i + n]
    i += n
    out += b"%x\r\n" % len(part) + part + b"\r\n"
out += b"0\r\n\r\n"
t.sendall(out)
info = (f"version={t.version()} cipher={t.cipher()[0]} alpn={t.selected_alpn_protocol()} method={method} path={path} "
        f"key={hdr.get('x-api-key', '-')} version_hdr={hdr.get('anthropic-version', '-')} clen={clen} got={len(body)} "
        f"stream={str(j.get('stream')).lower()} model={j['model']} user={user}")
try:
    t.unwrap()
except Exception as e:
    info += f" unwrap={type(e).__name__}"
print("OK", info, flush=True)
