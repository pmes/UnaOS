#!/usr/bin/env python3
"""TLSCORE oracle PKI via Python `cryptography`: root (P-256) → intermediate (P-256, pathlen 0) → leaves
(p256, ed25519, rsa). usage: gen_certs.py <outdir>. Writes <leaf>.key, <leaf>_chain.pem (leaf + intermediate), root.pem."""
import datetime, ipaddress, sys
from cryptography import x509
from cryptography.x509.oid import NameOID, ExtendedKeyUsageOID
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec, ed25519, rsa

out = sys.argv[1]
now = datetime.datetime.now(datetime.timezone.utc)
def nm(cn): return x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, cn)])
def cert(cn, key, icn, ikey, ca, pathlen=None, san=False):
    b = (x509.CertificateBuilder().subject_name(nm(cn)).issuer_name(nm(icn)).public_key(key.public_key())
         .serial_number(x509.random_serial_number()).not_valid_before(now - datetime.timedelta(days=1))
         .not_valid_after(now + datetime.timedelta(days=30))
         .add_extension(x509.BasicConstraints(ca=ca, path_length=pathlen), critical=True)
         .add_extension(x509.KeyUsage(digital_signature=not ca, content_commitment=False, key_encipherment=False,
                        data_encipherment=False, key_agreement=False, key_cert_sign=ca, crl_sign=ca,
                        encipher_only=False, decipher_only=False), critical=True))
    if san:
        b = b.add_extension(x509.SubjectAlternativeName([x509.DNSName("tlscore.test"), x509.DNSName("localhost"),
                            x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]), critical=False)
        b = b.add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.SERVER_AUTH]), critical=False)
    return b.sign(ikey, hashes.SHA256())
pem = lambda c: c.public_bytes(serialization.Encoding.PEM)
rk, ik = ec.generate_private_key(ec.SECP256R1()), ec.generate_private_key(ec.SECP256R1())
root = cert("TLSCORE Oracle Root", rk, "TLSCORE Oracle Root", rk, True)
inter = cert("TLSCORE Oracle Intermediate", ik, "TLSCORE Oracle Root", rk, True, 0)
open(f"{out}/root.pem", "wb").write(pem(root))
for name, key in [("p256", ec.generate_private_key(ec.SECP256R1())), ("ed25519", ed25519.Ed25519PrivateKey.generate()),
                  ("rsa", rsa.generate_private_key(public_exponent=65537, key_size=2048))]:
    leaf = cert("tlscore.test", key, "TLSCORE Oracle Intermediate", ik, False, san=True)
    open(f"{out}/{name}_chain.pem", "wb").write(pem(leaf) + pem(inter))
    open(f"{out}/{name}.key", "wb").write(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                                            serialization.NoEncryption()))
