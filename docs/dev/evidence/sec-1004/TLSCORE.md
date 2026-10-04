# TLSCORE (LEDGER SR28) — UnaOS's own TLS 1.3 client and X.509 path validation

Branch `exec-sec-tls`, crate `unaos/libs/sys/tls_core` (`#![no_std]` + `alloc`, `#![forbid(unsafe_code)]`, a member
of the root workspace). Commits: M1 147757fd, M2 099c6f7d, M3 af3eff59, M4 8d592580, this doc on top.

## Finding

TLS 1.3 and the Web PKI path check are written here from RFC 8446 / 5280 / 6125 with **no third-party crate under
the protocol**: record layer, key schedule, transcript, handshake state machine, DER, X.509, chain building, name
matching, PEM trust store. Every primitive is reached through `trait CryptoProvider` (`src/crypto.rs`). The product
provider is CRYPTOCORE (SR27); a RustCrypto provider exists only as a test operand (feature `test-provider`, enabled
by nothing but this crate's own `[dev-dependencies]` self-reference). Non-dev dependencies: **none**.

## Spec sections covered

| RFC 8446 | what |
|---|---|
| §4.1.1–4.1.4 | ClientHello (SNI, supported_groups x25519+P-256, key_share, signature_algorithms filtered by `supports_signature`, ALPN, supported_versions, psk_key_exchange_modes absent), ServerHello checks (suite/extension/session-id echo, downgrade sentinel), HelloRetryRequest (cookie, group change, message_hash transcript) |
| §4.2 | EncryptedExtensions checks (ALPN must be one we offered; unsolicited extensions refused) |
| §4.4.1–4.4.4 | Certificate (chain to the verifier), CertificateVerify (scheme must be offered and CV-legal; 64×0x20 context), Finished both ways (constant-time compare) |
| §4.6.1, §4.6.3 | NewSessionTicket parsed and kept (resumption not offered), KeyUpdate sent and answered both directions |
| §5.1–5.5 | record framing limits, TLSInnerPlaintext with padding, per-record nonce, AEAD, change_cipher_spec tolerance, D.4 middlebox compat mode |
| §6 | alerts: close_notify both ways, fatal alerts sent with the right description, peer alerts surfaced |
| §7.1–7.5 | HKDF-Extract/Expand-Label, Derive-Secret, every traffic secret, finished keys, exporter + resumption master, resumption PSK |
| RFC 5280 §4.1, §4.2.1.{1,2,3,9,10,12}, §6 (Web PKI profile) | DER-strict parse; AKI/SKI, KeyUsage, BasicConstraints + pathLen, NameConstraints (dNSName/iPAddress), EKU serverAuth; unknown critical extension rejects; path building with backtracking over the presented intermediates (any order, junk skipped), signature budget 64, depth 6 |
| RFC 6125 §6 | SAN dNSName / iPAddress only (CN ignored), left-most-label wildcard only, IDNA A-labels compared case-insensitively |

## Oracles and results

1. **RFC 8448 byte-exact (KAT).** Section 3 (simple 1-RTT): every secret checked byte-exact — x25519 publics, ECDHE,
   early, derived(early), handshake, c/s hs traffic, hs keys+IVs, both finished keys, server verify_data,
   derived(hs), master, c/s ap traffic, exporter master, ap keys+IVs, resumption master, resumption PSK, and the
   three transcript hashes: **yes, all**. Every record of the trace sealed/opened byte-exact (ClientHello,
   ServerHello, the encrypted server flight, client Finished, NewSessionTicket, app data, close_notify). Sections 5
   (HRR) and the D.4 compat trace drive the full state machine end to end with the client's written bytes compared to
   the trace. Tampering any record byte → the right alert.
2. **Handshake oracle — OpenSSL (Python `ssl.SSLContext`, OpenSSL 3.0.13) on 127.0.0.1**, PKI generated per run
   (Python `cryptography` 41 is broken in this container — pyo3 panic — so `tests/oracle/gen_certs_openssl.sh` ran;
   skipped with a message if neither works): root P-256 → intermediate (pathlen 0) → leaf `tlscore.test`.
   All 7 cases pass, both ends agree on suite and version, the server counts every request byte, close_notify clean:
   `TLS_AES_128_GCM_SHA256`, `TLS_AES_256_GCM_SHA384`, `TLS_CHACHA20_POLY1305_SHA256` each alone (+ KeyUpdate
   answered); **HRR** to P-256; **Ed25519** leaf; **RSA** leaf (rsa_pss_rsae_sha256 CV); bulk 100 000-byte POST in
   1000-byte records + 300 000-byte response byte-exact. Refusals seen from both ends: foreign root → client
   `UnknownIssuer`, server `TLSV1_ALERT_UNKNOWN_CA`; wrong name → client `NameMismatch`, server `BAD_CERTIFICATE`.
3. **Public hosts with the Mozilla bundle** (121 roots; 1 unusable key: e-Szigno TLS Root CA 2023, P-521).
   `example.com` and `cloudflare.com` are **403 at this container's egress policy** (unreachable — not a TLS
   verdict). `anthropic.com` **VERIFIED** (4-cert chain, ECDSA P-384 intermediates, `GET` → 301), `www.anthropic.com`
   VERIFIED (200), `raw.githubusercontent.com` VERIFIED (RSA chain, 301). `github.com`, `pypi.org`,
   `index.crates.io`, `registry.npmjs.org` are **re-terminated by the egress proxy** for our ClientHello: the chain
   presented is `[host] [Egress Gateway SDS Issuing CA] [sandbox-egress-gateway… CA]` / `[CCR agent-proxy
   interception CA]`, which the Mozilla bundle correctly **REFUSES (UnknownIssuer)**; with the egress path's own CAs
   (picked by name out of /root/.ccr/ca-bundle.crt, reported separately, never in place of Mozilla) the same
   connections **VERIFY** and the GETs return 200/400. That is the verifier refusing a MITM and accepting the MITM only
   when told to trust it.
4. **Captured public chains vs `openssl verify`.** The real chains of pypi.org, index.crates.io, registry.npmjs.org,
   anthropic.com, raw.githubusercontent.com (captured with `openssl s_client -showcerts`, tests/data/public, 36 KB,
   `CAPTURED_AT` 1791157287) validate under tls_core at the capture instant against the Mozilla bundle AND under
   `openssl verify -x509_strict -purpose sslserver -verify_hostname`: 5/5 agree; a wrong name → `NameMismatch` 5/5.
   (Covers the GTS Root R4 cross-signed by GlobalSign path and the ISRG Root X2 ← X1 cross-sign.)
5. **Wycheproof ECDSA P-256/SHA-256 through the trait**: 484/484 agree.

KAT/test count: m1 5, m2 10, m3 7, m4 3 = **25 tests**, `cargo test --release -p tls_core` all pass.

## The fold — the exact trait CRYPTOCORE implements

`tls_core::crypto::CryptoProvider` (object-safe, `&dyn` everywhere). Required: `random`, `hash(HashAlg, &[&[u8]])
-> Digest`, `hmac`, `aead_seal`/`aead_open` (in place on `Vec<u8>`, tag appended/stripped; AES-128/256-GCM,
ChaCha20-Poly1305), `x25519_keypair`/`x25519_shared` (reject all-zero), `p256_keypair`/`p256_ecdh` (SEC1
uncompressed, on-curve check), `ecdsa_verify(EcCurve, HashAlg, sec1, msg, sig_der)`, `ed25519_verify`. Defaults a
provider may override: `hkdf_extract`, `hkdf_expand`, `hkdf_expand_label`; `rsa_pss_verify(hash, n, e, msg, sig)`
and `rsa_pkcs1_verify` (default `Unsupported`); `supports_signature` (default: ECDSA P-256 + Ed25519),
`supports_aead`, `supports_hash`. HashAlg = SHA-256/384/512.

**Fold proof (run here, in a scratch workspace, deleted after):** CRYPTOCORE's `adapters/tls_core_provider.rs`
(exec-sec-crypto 66c835b7 + its working tree) dropped into tls_core as `src/cryptocore_provider.rs` compiles
against this trait unchanged, and with it swapped in for the test provider: m1 RFC 8448 secrets + records 5/5
byte-exact; Wycheproof 484/484; the **live OpenSSL handshakes pass for all three suites, HRR, P-256 and Ed25519
leaves**. What fails is exactly what CRYPTOCORE lacks: no RSA → the RFC 8448 end-to-end traces (RSA server
certificate) stop at CertificateVerify with `Unsupported("RSASSA-PSS…")`, the RSA fixture chain fails, and the RSA
OpenSSL leaf ends in `handshake_failure` because we (correctly) offered no RSA scheme. No P-384 → chains with
ECDSA-P-384 intermediates (anthropic.com: Let's Encrypt YE1/Root YE) cannot verify. So for the Web PKI the
fold owes, in CRYPTOCORE: **RSASSA-PSS + PKCS#1 v1.5 verify (2048–4096) and ECDSA P-384 verify**; without them
the product client reaches ECDSA-P-256/Ed25519 servers whose chains are P-256-only.

At the fold: copy the adapter, add the `cryptocore`/`cryptocore-std` features (text in the adapter's header), and
switch the tests with `use tls_core::cryptocore_provider::CryptoCoreProvider as RustCryptoProvider` once RSA and P-384
land; until then keep the m2 RFC 8448 traces on the test provider.

## Trust bundle

`tools/trust-bundle` fetches https://curl.se/ca/cacert.pem (blocked here → falls back to certifi on PyPI, the same
Mozilla extraction, recorded as such), refuses fewer than 100 roots, writes `system/trust/roots.pem` (gitignored,
240 KB > the 200 KB rule), `roots.pem.sha256` and `SOURCE`. Today: certifi 2026.7.22, sha256
`9cc2a774b5198dcff14d9be1e66091f538975d867ce029a96bce15a55dfd730f`, 121 roots. **Builder line** (for arroyo, not
edited by this arc):

    mkdir -p "$ESP_DIR/system/trust" && cp "$WORKSPACE_DIR/system/trust/roots.pem" "$ESP_DIR/system/trust/roots.pem"

loaded with `tls_core::x509::TrustStore::from_pem`.

## Honest ceiling (NOT done)

* No PSK / resumption / 0-RTT (tickets are parsed and dropped); no client certificates (post-handshake auth refused).
* No revocation: no OCSP, no OCSP stapling, no CRL; no Certificate Transparency (SCTs ignored).
* X.509: Name comparison is byte-equality (no RFC 5280 §7.1 normalisation — Web PKI CAs encode identically, all 5
  captured chains pass); certificatePolicies / policy constraints / inhibitAnyPolicy not processed (non-critical
  ones are ignored, critical ones reject); name constraints only for dNSName/iPAddress (others → refuse when
  they are the only constraint); no P-521, no Ed448, no DSA; no AIA fetching of missing intermediates.
* Groups: X25519 and P-256 only (no X25519MLKEM768 hybrid yet); suites: the three mandatory-ish TLS 1.3 suites.
* No HTTP — the oracle's GET is a byte string; `vein_ring3` still uses `embedded-tls` until the fold wires this in.

## Owed

1. CRYPTOCORE: RSA verify (PSS + PKCS#1 v1.5) and P-384 ECDSA, then the fold (adapter + features + test switch).
2. Replace `embedded-tls` under `vein_ring3` with tls_core; builder stages `system/trust/roots.pem`.
3. Resumption (PSK-DHE), OCSP stapling, X25519MLKEM768.

## Third-party crates (all TEST-ONLY, feature `test-provider`, versions = latest on crates.io at the time added)

sha2 0.11.0, hmac 0.13.0, aes-gcm 0.11.1, chacha20poly1305 0.11.0, x25519-dalek 3.0.0, p256 0.14.0, p384 0.14.0,
ed25519-dalek 3.0.0, rsa 0.10.0-rc.18 (pre-release: the only line on the same digest/signature generation as the
current sha2/p256 — R83's pre-release clause), getrandom 0.4.3. They are the *test operand* that stands in for
CRYPTOCORE; none is linked into a product build and none touches the protocol. Product dependencies: none.
