#!/usr/bin/env python3
"""Generates the static X.509 fixtures for tests/m3_x509.rs (run once; outputs are committed).

Needs Python `cryptography`. Every certificate is valid 2025-01-01 .. 2035-01-01 unless the case says otherwise;
the tests validate at 2026-10-04 with a FixedClock. Output: tests/data/x509/<name>.der
"""
import datetime, ipaddress, os, sys
from cryptography import x509
from cryptography.x509.oid import NameOID, ExtendedKeyUsageOID, ObjectIdentifier
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec, ed25519, rsa, padding

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "data", "x509")
os.makedirs(OUT, exist_ok=True)
NB = datetime.datetime(2025, 1, 1)
NA = datetime.datetime(2035, 1, 1)
serial = [1000]

def name(cn):
    return x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, cn)])

def ku(cert_sign=False, digital=False):
    return x509.KeyUsage(digital_signature=digital, content_commitment=False, key_encipherment=False,
                         data_encipherment=False, key_agreement=False, key_cert_sign=cert_sign, crl_sign=cert_sign,
                         encipher_only=False, decipher_only=False)

def make(subject_cn, subject_key, issuer_cn, issuer_key, ca=False, pathlen=None, san=None, ips=None, nb=NB, na=NA,
         key_usage=True, cert_sign=True, eku=None, nc=None, extra=None, hash_alg="sha256", pss=False, bc=True):
    serial[0] += 1
    b = (x509.CertificateBuilder().subject_name(name(subject_cn)).issuer_name(name(issuer_cn))
         .public_key(subject_key.public_key()).serial_number(serial[0]).not_valid_before(nb).not_valid_after(na))
    if bc:
        b = b.add_extension(x509.BasicConstraints(ca=ca, path_length=pathlen), critical=True)
    if key_usage:
        b = b.add_extension(ku(cert_sign=ca and cert_sign, digital=not ca or not cert_sign), critical=True)
    names = [x509.DNSName(d) for d in (san or [])] + [x509.IPAddress(ipaddress.ip_address(i)) for i in (ips or [])]
    if names:
        b = b.add_extension(x509.SubjectAlternativeName(names), critical=False)
    if eku:
        b = b.add_extension(x509.ExtendedKeyUsage(eku), critical=False)
    if nc:
        b = b.add_extension(nc, critical=True)
    b = b.add_extension(x509.SubjectKeyIdentifier.from_public_key(subject_key.public_key()), critical=False)
    b = b.add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(issuer_key.public_key()), critical=False)
    for e, crit in (extra or []):
        b = b.add_extension(e, critical=crit)
    if isinstance(issuer_key, ed25519.Ed25519PrivateKey):
        return b.sign(issuer_key, None)
    h = {"sha256": hashes.SHA256(), "sha384": hashes.SHA384()}[hash_alg]
    if pss:
        return b.sign(issuer_key, h, rsa_padding=padding.PSS(mgf=padding.MGF1(h), salt_length=h.digest_size))
    return b.sign(issuer_key, h)

def save(n, cert):
    with open(os.path.join(OUT, n + ".der"), "wb") as f:
        f.write(cert.public_bytes(serialization.Encoding.DER))

p256 = lambda: ec.generate_private_key(ec.SECP256R1())
root_k, inter_k, leaf_k = p256(), p256(), p256()
SAN = ["example.test", "*.wild.example.test"]

root = make("TLSCORE Test Root", root_k, "TLSCORE Test Root", root_k, ca=True)
save("root", root)
inter = make("TLSCORE Test Intermediate", inter_k, "TLSCORE Test Root", root_k, ca=True, pathlen=0)
save("inter", inter)
save("leaf_ok", make("example.test", leaf_k, "TLSCORE Test Intermediate", inter_k, san=SAN, ips=["127.0.0.1", "::1"],
                     eku=[ExtendedKeyUsageOID.SERVER_AUTH]))
save("leaf_expired", make("example.test", leaf_k, "TLSCORE Test Intermediate", inter_k, san=SAN,
                          na=datetime.datetime(2026, 1, 1)))
save("leaf_notyet", make("example.test", leaf_k, "TLSCORE Test Intermediate", inter_k, san=SAN,
                         nb=datetime.datetime(2027, 1, 1)))
save("leaf_nosan", make("example.test", leaf_k, "TLSCORE Test Intermediate", inter_k))
save("leaf_eku_client", make("example.test", leaf_k, "TLSCORE Test Intermediate", inter_k, san=SAN,
                             eku=[ExtendedKeyUsageOID.CLIENT_AUTH]))
save("leaf_is_ca", make("example.test", leaf_k, "TLSCORE Test Intermediate", inter_k, san=SAN, ca=True))
save("leaf_unknown_critical", make("example.test", leaf_k, "TLSCORE Test Intermediate", inter_k, san=SAN,
     extra=[(x509.UnrecognizedExtension(ObjectIdentifier("1.3.6.1.4.1.99999.1"), b"\x05\x00"), True)]))
save("leaf_unknown_noncritical", make("example.test", leaf_k, "TLSCORE Test Intermediate", inter_k, san=SAN,
     extra=[(x509.UnrecognizedExtension(ObjectIdentifier("1.3.6.1.4.1.99999.1"), b"\x05\x00"), False)]))

# A CA:FALSE "intermediate" and one whose KeyUsage lacks keyCertSign, each issuing a leaf.
bad_k = p256()
save("inter_not_ca", make("TLSCORE Bad Intermediate", bad_k, "TLSCORE Test Root", root_k, ca=False, san=None))
save("inter_no_certsign", make("TLSCORE Bad Intermediate", bad_k, "TLSCORE Test Root", root_k, ca=True, cert_sign=False))
save("leaf_under_bad", make("example.test", leaf_k, "TLSCORE Bad Intermediate", bad_k, san=SAN))

# pathLen: inter (pathlen 0) -> inter2 -> leaf is one CA too many.
inter2_k = p256()
save("inter2", make("TLSCORE Test Intermediate 2", inter2_k, "TLSCORE Test Intermediate", inter_k, ca=True))
save("leaf_under_inter2", make("example.test", leaf_k, "TLSCORE Test Intermediate 2", inter2_k, san=SAN))

# Name constraints.
nc_k = p256()
save("inter_nc", make("TLSCORE NC Intermediate", nc_k, "TLSCORE Test Root", root_k, ca=True,
     nc=x509.NameConstraints(permitted_subtrees=[x509.DNSName("allowed.test")],
                             excluded_subtrees=[x509.DNSName("bad.allowed.test")])))
save("leaf_nc_ok", make("host.allowed.test", leaf_k, "TLSCORE NC Intermediate", nc_k, san=["host.allowed.test"]))
save("leaf_nc_outside", make("evil.test", leaf_k, "TLSCORE NC Intermediate", nc_k, san=["evil.test"]))
save("leaf_nc_excluded", make("x.bad.allowed.test", leaf_k, "TLSCORE NC Intermediate", nc_k, san=["x.bad.allowed.test"]))

# Ed25519 root -> Ed25519 leaf.
ed_root_k, ed_leaf_k = ed25519.Ed25519PrivateKey.generate(), ed25519.Ed25519PrivateKey.generate()
save("ed_root", make("TLSCORE Ed25519 Root", ed_root_k, "TLSCORE Ed25519 Root", ed_root_k, ca=True))
save("ed_leaf", make("example.test", ed_leaf_k, "TLSCORE Ed25519 Root", ed_root_k, san=SAN))

# RSA-2048 root -> leaf signed PKCS#1 v1.5 and leaf signed RSASSA-PSS.
rsa_root_k = rsa.generate_private_key(public_exponent=65537, key_size=2048)
save("rsa_root", make("TLSCORE RSA Root", rsa_root_k, "TLSCORE RSA Root", rsa_root_k, ca=True))
save("rsa_leaf_pkcs1", make("example.test", leaf_k, "TLSCORE RSA Root", rsa_root_k, san=SAN))
save("rsa_leaf_pss", make("example.test", leaf_k, "TLSCORE RSA Root", rsa_root_k, san=SAN, pss=True))

# P-384 root signs a P-256 intermediate with ecdsa-with-SHA384 (the Let's Encrypt E-series shape).
p384_root_k = ec.generate_private_key(ec.SECP384R1())
save("p384_root", make("TLSCORE P-384 Root", p384_root_k, "TLSCORE P-384 Root", p384_root_k, ca=True, hash_alg="sha384"))
p384_inter_k = p256()
save("p384_inter", make("TLSCORE P-384 Intermediate", p384_inter_k, "TLSCORE P-384 Root", p384_root_k, ca=True,
                        hash_alg="sha384"))
save("p384_leaf", make("example.test", leaf_k, "TLSCORE P-384 Intermediate", p384_inter_k, san=SAN))

# An unrelated certificate servers sometimes append.
other_k = p256()
save("unrelated", make("Unrelated", other_k, "Unrelated", other_k, ca=True))
print("wrote", len(os.listdir(OUT)), "fixtures to", OUT)
