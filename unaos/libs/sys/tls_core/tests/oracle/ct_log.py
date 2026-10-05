#!/usr/bin/env python3
"""CTCORE oracle: a local Certificate Transparency log pair, written independently of tls_core.

  ct_log.py keys DIR                      two P-256 log keys (operators "Oracle A" / "Oracle B") and DIR/log_list.json
                                          in Google's v3 schema (log_id = SHA-256(SPKI DER), base64)
  ct_log.py sct DIR LOG LEAF.pem TS_MS    a v1 SCT over x509_entry(LEAF) signed by log LOG (a|b), hex on stdout
  ct_log.py serverinfo OUT V SCT_HEX...   an OpenSSL -serverinfo PEM carrying the SCT list (V=1: TLS 1.2
                                          ServerHello; V=2: SERVERINFOV2 for the TLS 1.3 leaf CertificateEntry)

The signed struct (RFC 6962 §3.2) is encoded here with struct/hashlib; the ECDSA signature is OpenSSL's
(`openssl dgst -sha256 -sign`). Nothing here imports tls_core's logic.
"""
import base64, hashlib, json, os, struct, subprocess, sys

def run(*a, inp=None):
    return subprocess.run(a, input=inp, check=True, capture_output=True).stdout

def der_of(pem_path, kind="x509"):
    if kind == "x509":
        return run("openssl", "x509", "-in", pem_path, "-outform", "DER")
    return run("openssl", "pkey", "-in", pem_path, "-pubout", "-outform", "DER")

def keys(d):
    logs = []
    for name, op in (("a", "Oracle A"), ("b", "Oracle B")):
        k = os.path.join(d, f"log_{name}.key")
        run("openssl", "ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", k)
        spki = der_of(k, "key")
        logs.append((op, {
            "description": f"CTCORE oracle log {name}",
            "log_id": base64.b64encode(hashlib.sha256(spki).digest()).decode(),
            "key": base64.b64encode(spki).decode(),
            "url": f"https://ct.invalid/{name}/",
            "mmd": 86400,
            "state": {"usable": {"timestamp": "2024-01-01T00:00:00Z"}},
        }))
    ll = {"version": "1.0", "log_list_timestamp": "2026-10-01T00:00:00Z",
          "operators": [{"name": op, "email": ["x@invalid"], "logs": [l]} for op, l in logs]}
    open(os.path.join(d, "log_list.json"), "w").write(json.dumps(ll, indent=2))

def sct(d, name, leaf_pem, ts):
    leaf = der_of(leaf_pem)
    k = os.path.join(d, f"log_{name}.key")
    log_id = hashlib.sha256(der_of(k, "key")).digest()
    ts = int(ts)
    ext = b""
    # digitally-signed struct: version, signature_type, timestamp, entry_type, ASN.1Cert<1..2^24-1>, extensions<0..2^16-1>
    signed = struct.pack(">BBQH", 0, 0, ts, 0) + len(leaf).to_bytes(3, "big") + leaf + struct.pack(">H", len(ext)) + ext
    sig = run("openssl", "dgst", "-sha256", "-sign", k, inp=signed)
    body = struct.pack(">B", 0) + log_id + struct.pack(">Q", ts) + struct.pack(">H", len(ext)) + ext
    body += struct.pack(">BBH", 4, 3, len(sig)) + sig
    print(body.hex())

def serverinfo(out, v, scts):
    items = b"".join(struct.pack(">H", len(s)) + s for s in (bytes.fromhex(x) for x in scts))
    lst = struct.pack(">H", len(items)) + items
    ext = struct.pack(">HH", 18, len(lst)) + lst
    if v == "2":
        # SERVERINFOV2: a 4-byte context — SSL_EXT_TLS1_3_CERTIFICATE (0x1000) | SSL_EXT_CLIENT_HELLO (0x80)
        # | SSL_EXT_TLS1_2_SERVER_HELLO (0x100), as OpenSSL's serverinfo_v2 expects.
        data, label = struct.pack(">I", 0x1000 | 0x100 | 0x80) + ext, "SERVERINFOV2 FOR signed_certificate_timestamp"
    else:
        data, label = ext, "SERVERINFO FOR signed_certificate_timestamp"
    b = base64.encodebytes(data).decode()
    open(out, "w").write(f"-----BEGIN {label}-----\n{b}-----END {label}-----\n")

if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "keys":
        keys(sys.argv[2])
    elif cmd == "sct":
        sct(*sys.argv[2:6])
    elif cmd == "serverinfo":
        serverinfo(sys.argv[2], sys.argv[3], sys.argv[4:])
