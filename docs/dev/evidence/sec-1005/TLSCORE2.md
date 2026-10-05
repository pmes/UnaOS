# TLSCORE2 (LEDGER SR58) — TLS 1.2, TLS 1.3 resumption, RFC 5280 path building, stapled OCSP, SCTs

Branch `exec-sec-tlscore2`, cut at a1c447ed; `exec-rmbp-merge13` then `exec-net-httpcore` (@52e78e73) merged first
(ca4671d1). Commits: M1 cf45da71, M2 7986b0c0, M3a bb028383, M3b 5973d64e, M3 395e0e8a, M4 d3d2d58c, this doc on top.
Crate `unaos/libs/sys/tls_core` (`no_std` + `alloc`, `forbid(unsafe_code)`), primitives from `crypto_core`. No new
crate; no third-party crate under either.

## Finding

tls_core spoke TLS 1.3 only, with no resumption and a chain builder that only looked at what the server sent. It now:

* speaks **TLS 1.2** (RFC 5246) with ECDHE_ECDSA / ECDHE_RSA and AES-128/256-GCM (RFC 5288/5289) or
  ChaCha20-Poly1305 (RFC 7905), **extended master secret required** (RFC 7627), SNI, ALPN, signature_algorithms,
  **renegotiation refused** (RFC 5746 SCSV + empty renegotiation_info; a HelloRequest gets a warning
  no_renegotiation), **downgrade sentinels** checked (RFC 8446 §4.1.3), RFC 5705 exporter;
* **resumes TLS 1.3 sessions** (RFC 8446 §2.2, §4.2.11, §4.6.1): NewSessionTicket → PSK, offered with psk_dhe_ke only
  and a binder (recomputed after HRR), selected_identity and suite hash checked, Certificate/CertificateVerify skipped;
  **0-RTT refused**; tickets live in a **caller-owned `TicketStore`** (host: `http_core::host::SharedTicketStore`,
  memory or a 0600 file; metal: Holocron later);
* **builds paths per RFC 5280 §6** with backtracking over the server's certificates **and an intermediate pool**
  (`TrustStore::intermediates`), so cross-signed roots resolve to whichever anchor the store holds, expired cross-signs
  are passed over, and an omitted intermediate is found without AIA fetching; **name constraints** (dNSName, iPAddress,
  rfc822Name, URI, directoryName) of every CA on the path apply to every certificate below it;
* **verifies stapled OCSP** (RFC 6960 via RFC 6066 §8 in 1.2 and RFC 8446 §4.4.2.1 in 1.3) when present —
  issuer-signed or delegated responder, SHA-1/SHA-256/384/512 CertIDs, freshness — revoked → `certificate_revoked`,
  invalid → `bad_certificate_status_response`; **SCTs** (RFC 6962) parsed from the leaf, the TLS extension and the OCSP
  response and reported in `Negotiated::scts`.

Callers: `http_core::host` (Aether, Vein, Gneiss through gneiss_pal) and `vein_ring3` (Lumen/NET.ELF on the metal)
both call `ClientConfig::enable_tls12()`; http_core also gives every Agent a ticket store.

## Spec sections covered

| spec | what |
|---|---|
| RFC 5246 §5, §6.2.3.3, §6.3, §7.3–7.4.9 | PRF (P_SHA256/P_SHA384, in crypto_core `tls12_prf` and the trait default), AEAD record layer (AAD = seq‖type‖version‖length, 2^14+2048 bound), key block, the full ECDHE handshake, Finished, empty client Certificate on request |
| RFC 5288 / 5289 / 7905 | GCM 4-byte salt + 8-byte explicit nonce (= sequence number); ChaCha20-Poly1305 IV ⊕ seq |
| RFC 8422 §5.1–5.4 | named curves only (x25519, secp256r1), uncompressed points, ServerKeyExchange signature over randoms‖params, Ed25519 under ECDHE_ECDSA; a 1.2 SignatureAndHashAlgorithm binds a hash, not a curve |
| RFC 7627 | session_hash master secret; the legacy master secret is never computed |
| RFC 5746 | SCSV, empty renegotiation_info required if present, HelloRequest → no_renegotiation (warning) |
| RFC 8446 §4.1.3 | DOWNGRD\x01 / \x00 refused when 1.3 was offered, accepted by a 1.2-only configuration |
| RFC 5705 | exporter on 1.2 |
| RFC 8446 §4.2.9–4.2.11, §4.6.1, §4.1.4 | psk_key_exchange_modes, pre_shared_key last, obfuscated age, binder, HRR with PSK, identity/hash checks, lifetime ≤ 7 days |
| RFC 5280 §4.2.1.10, §4.2.2.1, §6.1.3 (b)(c) | name constraints of every type we can evaluate; AIA parsed (URIs reported, not fetched) |
| RFC 6960 §4.1.1, §4.2.1, §4.2.2.2 | OCSPResponse / BasicOCSPResponse, CertID matching, responder authorisation, thisUpdate/nextUpdate |
| RFC 6066 §8, RFC 8446 §4.4.2.1 | status_request offered; CertificateStatus (1.2), leaf CertificateEntry extension (1.3) |
| RFC 6962 §3.3 | SCT lists from the X.509 extension, the TLS extension, the OCSP single extension |
| FIPS 180-4 §6.1 | SHA-1 in crypto_core — only to match OCSP CertIDs, never a signature |

## Oracles and results (all on the PRODUCT provider, CRYPTOCORE)

1. **TLS 1.2 vs OpenSSL** (Python `ssl` 3.0.13 pinned to TLSv1.2 with ONE suite each, `tests/oracle/tls_server.py`):
   **10/10** — all six suites, P-256 ECDHE, Ed25519 leaf, and 100 000-byte POST / 300 000-byte response in 1000-byte
   records over AES-256-GCM and ChaCha20; both ends agree on version and suite, ALPN http/1.1, the server counts every
   byte, close_notify clean; RFC 5705 exporter works. The Finished exchange is the key-derivation oracle. PRF KATs:
   the two published P_SHA256/P_SHA384 vectors, reproduced byte for byte by `openssl kdf TLS1-PRF`.
   Version negotiation against a dual-stack server: offer both → 1.3; 1.2-only client → 1.2 (sentinel accepted);
   1.3-only client vs 1.2 server → protocol_version both ends. Refusals in 1.2: unknown CA / name mismatch, OpenSSL
   sees UNKNOWN_CA / BAD_CERTIFICATE. `s_server -sigalgs RSA+SHA256`: PKCS#1 v1.5 ServerKeyExchange verified;
   `s_server` `r` (HelloRequest): answered no_renegotiation, s_server logs `ssl3_read_bytes:no renegotiation`, one
   handshake only. http_core's whole e2e suite (chunked, gzip/deflate, redirects + cookies, 1 MiB PUT, paced SSE,
   keep-alive on one connection) passes against `http_server.py` pinned to TLS 1.2 (ECDHE-ECDSA-AES256-GCM-SHA384).
2. **Resumption vs OpenSSL's `session_reused`** (`tests/m6_resumption.rs`): full → 2 tickets kept; second connection
   **resumed** (no certificate, server `reused=True`); through **HelloRetryRequest** resumed; from the **persisted byte
   form** resumed; a ticket of another hash is not offered and is put back; an altered identity → server declines,
   verified full handshake; an altered PSK → the server refuses (`BINDER_DOES_NOT_VERIFY`); an expired ticket is never
   offered; no store → no psk modes, never resumed. http_core: two Agents sharing a ticket FILE (0600): every
   connection after the first resumed (`/stats` reused = connections − 1).
3. **Chain building vs `openssl verify`** (same anchors, untrusted set and instant, `tests/oracle/gen_pki2.sh`):
   **7/7 agree** — new root trusted (cross-sign ignored), only old root (path through the cross-sign), expired
   cross-sign first (backtrack), only an expired cross-sign (refused), omitted intermediate (refused), omitted
   intermediate from the pool, pool + cross-sign. Live: `s_server` without `-cert_chain` refused, then accepted with
   the pool (`pool_intermediates=1`). **Name constraints 6/6 agree**: inside, dNSName outside, excluded, rfc822Name
   outside, directoryName outside, URI outside.
4. **OCSP staples from `s_server -status_file`**, responses made by `openssl ocsp`, over **TLS 1.2 and 1.3**:
   none → NotStapled; issuer-signed SHA-1 CertID → Good; SHA-256 CertID → Good; delegated responder → Good(delegated);
   revoked → refused, s_server sees `alert certificate revoked`; signed by the root (unauthorised) → refused, s_server
   sees `bad certificate status response`; 12/12. A staple for another serial and a week-old staple are refused.
5. **SCTs**: the five captured public leaves carry 12 embedded SCTs (2–3 each, SHA-256/ECDSA, timestamps before the
   capture); a synthetic SCT list in a 1.2 `s_server -serverinfo` extension is reported exactly.
6. **Negative suite by a man in the middle rewriting a real OpenSSL handshake** (`tests/m8_negative.rs`):
   **10/10 refused with the expected alert, the server reporting it**: the downgrade attack (supported_versions stripped
   from our hello → DOWNGRD → illegal_parameter), EMS stripped → handshake_failure, non-empty renegotiation_info →
   handshake_failure, unsolicited session_ticket → unsupported_extension, a 1.3 key_share in a 1.2 ServerHello →
   illegal_parameter, a suite never offered → illegal_parameter, an SKE curve never offered → illegal_parameter, an
   altered SKE signature → decrypt_error, an altered server random → decrypt_error, an unsolicited 1.3 extension →
   unsupported_extension. The sentinel is observed: a 1.2-only client sees `DOWNGRD\x01` from a dual-stack server.
7. **tlsfuzzer**: not fetchable (github.com 403 at the egress, not on PyPI), and it drives a SERVER under test — none
   of its scripts can target a client; item 6 is the client-side counterpart. **0 tlsfuzzer scripts run.**
8. **A 1.2-only public host**: not reachable — badssl.com (tls-v1-2.badssl.com:1012) is refused by the egress policy
   and every allowed host is re-terminated by the egress gateway. Instead: a **1.2-only client through the gateway to
   api.anthropic.com** completed (ECDHE-ECDSA-AES128-GCM, X25519, EMS, 2 SCTs) and HTTP answered 405, verified against
   the egress CA (reported separately, never in place of Mozilla).

**Tests:** tls_core 40 (the original 25 green — m4's suite-name `match` gained one wildcard arm because the enum grew;
no assertion changed — plus m5 4, m6 3, m7 5, m8 3), vein_ring3 4 + 9, http_core host_e2e 8 (+2), crypto_core +2 unit
KATs. Gate: `cargo test --release -p tls_core -p vein_ring3 -p http_core --features host` all green.

**Metal (arroyo's own `build_user_lumen_x86` / `build_user_net_x86`, sourced unchanged):** LUMEN-X86.ELF **215 408 →
276 880 B** (need 3 612 896 of 4 194 304), NET-X86.ELF **164 664 → 226 136 B** (need 487 976), both `ELFENTRY … PASS`.

## Honest ceiling

* TLS 1.2: full handshakes only — no session-ID or RFC 5077 ticket resumption; no CBC, static RSA, FFDHE, RC4, 3DES,
  NULL/export (by design); curves x25519 + P-256 only; no client certificates; renegotiation never.
* Resumption: TLS 1.3 only; one PSK identity per hello; 0-RTT never (replayable, not forward-secret for early data).
* No AIA fetching (URIs parsed and reported); the intermediate pool is caller-supplied — `tools/trust-bundle` does not
  yet fetch Mozilla's CCADB intermediate list.
* Names compared by encoding (no RFC 5280 §7.1 normalisation) for chaining and directoryName constraints; no policy
  processing; otherName/x400/ediParty/registeredID constraints refuse certificates carrying such names.
* OCSP: stapled only, soft-fail when absent (no must-staple RFC 7633, no OCSP fetching, no CRLs); response nonce not
  required (stapled). SCTs are parsed, NOT verified (no CT log list / keys) and not required.
* The metal images are bigger by 61 KB; never run on metal (VEINTLS' ceiling stands).

## Owed

1. Must-staple (RFC 7633) and a CT log list to verify SCTs (Chrome's policy) — both need data shipped beside roots.pem.
2. `tools/trust-bundle` → also stage the CCADB intermediate set as `system/trust/intermediates.pem`; loaders call
   `TrustStore::add_intermediates_pem`.
3. Holocron's `TicketStore` on the metal (B355); vein_ring3 passes no store today (no resumption on the metal yet).
4. X25519MLKEM768 (TLSCORE's owed item stands).

## How to continue

`cargo test --release -p tls_core` runs everything (python3 + openssl needed; each test skips cleanly without them).
New 1.2 behaviour: add a case to `tests/m5_tls12.rs` (`tls_server.py --tls 1.2 --ciphers …`); a new refusal: a
`Case` in `tests/m8_negative.rs` (the `Mitm` in `tests/support` rewrites the client's first record and the server's
plaintext handshake messages); PKI scenarios live in `tests/oracle/gen_pki2.sh`.

## Third-party crates

None added; none in the product path (tls_core → crypto_core, both zero-dependency). tls_core's RustCrypto test
operand (feature `test-provider`, TLSCORE's original 25 tests only) is unchanged; TLSCORE2's tests run on CRYPTOCORE.
