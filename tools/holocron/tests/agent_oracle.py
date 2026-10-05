#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
# HOLOCRON1 M3 oracle: an SSH agent CLIENT written independently from draft-ietf-sshm-ssh-agent §3/§4 and
# RFC 8709 §4/§6, whose signature check is RFC 8032 §6's OWN Python reference verification (transcribed
# below; Python stdlib only — hashlib.sha512). None of Holocron's code is on this side of the socket.
# (The distro's pyca/cryptography is broken in this container — its pyo3 binding panics on import — so the
# RFC's reference code is the oracle; it has no dependency to break.)
# usage: agent_oracle.py <agent socket> ; prints one line per identity, exit 0 on success.
import socket, struct, sys, hashlib

# ---- RFC 8032 §6 reference (verification half), verbatim in substance ----
p = 2**255 - 19
L = 2**252 + 27742317777372353535851937790883648493
def modp_inv(x): return pow(x, p - 2, p)
d = -121665 * modp_inv(121666) % p
modp_sqrt_m1 = pow(2, (p - 1) // 4, p)
def sha512_modq(s): return int.from_bytes(hashlib.sha512(s).digest(), "little") % L
def point_add(P, Q):
    A, B = (P[1] - P[0]) * (Q[1] - Q[0]) % p, (P[1] + P[0]) * (Q[1] + Q[0]) % p
    C, D = 2 * P[3] * Q[3] * d % p, 2 * P[2] * Q[2] % p
    E, F, G, H = B - A, D - C, D + C, B + A
    return (E * F, G * H, F * G, E * H)
def point_mul(s, P):
    Q = (0, 1, 1, 0)
    while s > 0:
        if s & 1: Q = point_add(Q, P)
        P = point_add(P, P)
        s >>= 1
    return Q
def point_equal(P, Q):
    if (P[0] * Q[2] - Q[0] * P[2]) % p != 0: return False
    if (P[1] * Q[2] - Q[1] * P[2]) % p != 0: return False
    return True
def recover_x(y, sign):
    if y >= p: return None
    x2 = (y * y - 1) * modp_inv(d * y * y + 1)
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (p + 3) // 8, p)
    if (x * x - x2) % p != 0: x = x * modp_sqrt_m1 % p
    if (x * x - x2) % p != 0: return None
    if (x & 1) != sign: x = p - x
    return x
g_y = 4 * modp_inv(5) % p
g_x = recover_x(g_y, 0)
G = (g_x, g_y, 1, g_x * g_y % p)
def point_decompress(s):
    if len(s) != 32: raise Exception("Invalid input length for decompression")
    y = int.from_bytes(s, "little")
    sign = y >> 255
    y &= (1 << 255) - 1
    x = recover_x(y, sign)
    if x is None: return None
    return (x, y, 1, x * y % p)
def point_compress(P):
    zinv = modp_inv(P[2])
    x, y = P[0] * zinv % p, P[1] * zinv % p
    return int.to_bytes(y | ((x & 1) << 255), 32, "little")
def verify(public, msg, signature):
    if len(public) != 32: raise Exception("Bad public key length")
    if len(signature) != 64: raise Exception("Bad signature length")
    A = point_decompress(public)
    if not A: return False
    Rs = signature[:32]
    R = point_decompress(Rs)
    if not R: return False
    s = int.from_bytes(signature[32:], "little")
    if s >= L: return False
    h = sha512_modq(Rs + public + msg)
    sB = point_mul(s, G)
    hA = point_mul(h, A)
    return point_equal(sB, point_add(R, hA))
# ---- end RFC 8032 §6 ----

def rpc(sock_path, msg):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(sock_path)
    s.sendall(struct.pack(">I", len(msg)) + msg)
    n = struct.unpack(">I", recvn(s, 4))[0]
    out = recvn(s, n)
    s.close()
    return out

def recvn(s, n):
    b = b""
    while len(b) < n:
        c = s.recv(n - len(b))
        if not c:
            raise EOFError
        b += c
    return b

def sstr(b, o):
    n = struct.unpack(">I", b[o:o + 4])[0]
    return b[o + 4:o + 4 + n], o + 4 + n

path = sys.argv[1]
ans = rpc(path, bytes([11]))
assert ans[0] == 12, f"type {ans[0]}"
n = struct.unpack(">I", ans[1:5])[0]
o = 5
ids = []
for _ in range(n):
    blob, o = sstr(ans, o)
    comment, o = sstr(ans, o)
    ids.append((blob, comment.decode()))
assert o == len(ans), "trailing bytes in IDENTITIES_ANSWER"
data = b"holocron-oracle session data \x00\x01\x02"
for blob, comment in ids:
    alg, q = sstr(blob, 0)
    raw, q = sstr(blob, q)
    assert alg == b"ssh-ed25519" and len(raw) == 32 and q == len(blob), "RFC 8709 §4 blob"
    req = bytes([13]) + struct.pack(">I", len(blob)) + blob + struct.pack(">I", len(data)) + data + struct.pack(">I", 0)
    rsp = rpc(path, req)
    assert rsp[0] == 14, f"sign type {rsp[0]}"
    sigblob, end = sstr(rsp, 1)
    assert end == len(rsp)
    alg, pos = sstr(sigblob, 0)
    sig, pos = sstr(sigblob, pos)
    assert alg == b"ssh-ed25519" and len(sig) == 64 and pos == len(sigblob)
    if comment.endswith("[TEST-INSECURE]"):
        verdict = "blob-parsed sig-shape-ok (test signer: no Ed25519 claim)"
    else:
        assert verify(raw, data, sig), "RFC 8032 verification FAILED"
        assert not verify(raw, data + b"x", sig), "verifier accepts a changed message"
        verdict = "ED25519-VERIFIED(rfc8032-ref)"
    print(f"{raw.hex()} {comment} {verdict}")
# Self-test of the transcribed verifier on RFC 8032 §7.1 TEST 2 before trusting any verdict above.
t2_pub = bytes.fromhex("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c")
t2_sig = bytes.fromhex("92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00")
assert verify(t2_pub, b"\x72", t2_sig) and not verify(t2_pub, b"\x73", t2_sig), "oracle self-test"
print(f"identities={len(ids)}")
