# CTCORE (LEDGER SR60) — Certificate Transparency, must-staple + CRLs + AIA, ML-KEM-768 and X25519MLKEM768

Branch `exec-sec-ctcore`, cut at 179b0658; `exec-sec-tlscore2` (@58e8ced2: merge13 + httpcore + TLSCORE2) merged
first (92f0eb06). Commits: M1 c5eecf47, M2 66b19578, M3 21f32771; M4 (the ELF numbers and the kernel check below — no code) and this
doc in the M5 commit. Crates touched: `unaos/libs/sys/tls_core`, `unaos/libs/sys/crypto_core` (both `no_std` + `alloc`, zero
dependencies), `vein_ring3`, `http_core`, `user-lumen` (one report line), the builder, `tools/trust-bundle`.
No new crate. No third-party crate added anywhere.

## Finding

TLSCORE2 parsed SCTs but verified none, OCSP was soft-fail with no must-staple, the intermediate pool was
caller-supplied with nothing to fill it, and the only key exchange was classical. Now:

* **Certificate Transparency (RFC 6962 / RFC 9162), `tls_core::ct`.** A strict JSON reader; `LogList` for Google's
  v3 schema (Apple's list uses the same shape), every log's id checked as SHA-256(SPKI) on load, operator history
  kept; **SCT signature verification** for all three deliveries — embedded (a `PreCert` over SHA-256 of the
  issuer's SPKI and the leaf TBS re-encoded without the SCT extension), TLS extension and stapled OCSP
  (`x509_entry`) — on ECDSA P-256 and RSA PKCS#1 v1.5 (RFC 6962 §2.1.4); **Chrome's CT policy transcribed from
  Chromium's `chrome_ct_policy_enforcer.cc`** (both options, 2 / 3 distinct logs at 180 days, operator diversity
  as of each SCT's time, retired logs by date, tiled logs need `leaf_index` and an RFC 6962 log alongside, a list
  older than 70 days is not enforced); **RFC 9162 §2.1.3.2 inclusion proofs** with the RFC 6962 MerkleTreeLeaf of
  an SCT's entry; a detached list-signature check. `TrustStore.ct` carries the list and `CtMode::{Report,
  Strict}`; `CertVerdict.ct` / `Negotiated.ct` carry the verdict; **`ct=` policy / no_scts / insufficient /
  bad_sig / stale_list / off**; strict refuses the middle three with `bad_certificate`. Lumen prints
  `transport=tls verified=<issuer> ct=<verdict>` (vein_ring3 loads `/system/trust/ctlogs.jsn`); http_core attaches
  the staged list (`UNAOS_CT=strict` for strict).
* **Must-staple (RFC 7633).** The TLS Feature extension is parsed; a must-staple leaf without a verified `good`
  staple (absent, `unknown`, or never requested) fails with `bad_certificate_status_response`.
* **CRLs (RFC 5280 §5, checked per §6.3), `x509::crl`.** Caller-fetched CRLs (`TrustStore.crls`,
  `add_crl_der` / `add_crls_pem`) are checked over every certificate of the validated path: issuer, AKI,
  cRLSign, signature, freshness (nextUpdate required), issuingDistributionPoint scope, indirect CRLs refused, the
  highest cRLNumber wins; **delta CRLs** apply when the complete CRL or the certificate announces `freshestCRL`
  (§5.2.6), `removeFromCRL` releases a hold, `certificateHold` counts as revoked. Soft by default (an unusable CRL
  is reported, ignored); `TrustStore.require_revocation` hard-fails "no usable revocation information".
* **AIA caIssuers seam, `x509::aia`.** `IssuerFetcher` + `AiaVerifier`: on `UnknownIssuer`, fetch the presented
  certificates' caIssuers URIs (DER, certs-only CMS `.p7c`, or PEM), add the CA certificates to an untrusted copy
  of the pool, validate again (≤ 4 fetches). http_core's Agent does it by default through `HttpIssuerFetcher`
  (http:// only, 64 KiB, its own proxy/resolve settings).
* **CCADB intermediates.** `tools/trust-bundle` stages Mozilla's CCADB set as `system/trust/inters.pem` (8.3 for
  the metal) or takes `--intermediates-from FILE`; the builder stages it pin-checked; vein_ring3 and http_core load
  it as the pool. **Not staged in this container** (CCADB and Firefox remote settings are refused by the egress).
* **ML-KEM (FIPS 203) and SHA-3 (FIPS 202) in crypto_core.** `sha3`: Keccak-f[1600], SHA3-224/256/384/512,
  SHAKE128/256. `mlkem`: all three parameter sets written from Algorithms 3–21 (ByteEncode/Decode, Compress /
  Decompress, SampleNTT rejection sampling, SamplePolyCBD, NTT/NTT⁻¹ with ζ = 17, MultiplyNTTs/BaseCaseMultiply,
  K-PKE, ML-KEM KeyGen/Encaps/Decaps with implicit rejection, the §7.2/§7.3 input checks), `mlkem768` fixed-size.
* **X25519MLKEM768 (draft-ietf-tls-ecdhe-mlkem) in tls_core**, codepoint 0x11EC: client share ek ‖ X25519 (1216 B),
  server share ciphertext ‖ X25519 (1120 B), secret ML-KEM ss ‖ X25519 ss. `ClientConfig` defaults to
  `[X25519MLKEM768, X25519, secp256r1]`; with the hybrid first, **X25519 is shared alongside** (no extra round trip
  to a classical server); classical configurations keep TLSCORE's single share. TLS 1.3 only (a 1.2
  ServerKeyExchange naming it is refused). `CryptoProvider` gained `mlkem768_keypair`, `mlkem768_decaps`,
  `supports_group`; CRYPTOCORE implements them; the RustCrypto test operand does not (so it never offers it).

No new handler: CT, revocation and the KEM are shared-core capabilities under Holocron's (crypto_core) and the
transport's (tls_core) existing charters, with no bus surface of their own.

## Spec sections covered

| spec | what |
|---|---|
| RFC 6962 §2.1, §3.2, §3.3, §3.4 | SCT v1 wire form, the digitally-signed struct (both entry types), LogID = SHA-256(key), MerkleTreeLeaf |
| RFC 9162 §2.1.1, §2.1.3.1–2 | MTH, inclusion proof verification (PATH reproduced in the Python oracle) |
| Google CT log list v3 schema; Chrome CT policy | `log_list.json` parsing (logs + tiled_logs, states, previous_operators, temporal_interval read); `chrome_ct_policy_enforcer.cc` CheckCTPolicyCompliance + GetOperatorForLog |
| RFC 7633 §4 | TLS Feature status_request, hard-fail |
| RFC 5280 §5.1–5.3, §5.2.4–5.2.6, §6.3 | CertificateList, cRLNumber, deltaCRLIndicator, issuingDistributionPoint, freshestCRL, reasonCode (removeFromCRL, certificateHold), certificateIssuer (refused), path-wide checking |
| RFC 5280 §4.2.2.1, §4.2.1.13; RFC 5652 §5.1 | caIssuers fetch, cRLDistributionPoints URIs, certs-only SignedData |
| FIPS 202 §3.3, §5.1, §6.1–6.2 | Keccak-f[1600], pad10*1, SHA3-*, SHAKE* |
| FIPS 203 §4 (Alg 3–12), §5 (13–15), §6 (16–18), §7 (19–21, input checks), §8 | ML-KEM-512/768/1024 |
| draft-ietf-tls-ecdhe-mlkem §3 | X25519MLKEM768 shares and secret order |

## Oracles and results (all on the PRODUCT provider, CRYPTOCORE)

1. **Real SCTs** (`tests/m9_ct.rs`): Chromium's own log list (`components/certificate_transparency/data/log_list.json`
   at chromium@750ecf97, v93.4, 2026-10-04, 69 logs, 0 refused on load; sha256-pinned, fetched at test time).
   **12/12 embedded SCTs** of the five captured leaves verify; **5/5 leaves meet Chrome's policy** (Sectigo, IPng,
   Let's Encrypt, Cloudflare, Google, DigiCert logs); **24/24 single-SCT alterations** (one signature byte, the
   timestamp) refused; all altered / wrong issuer key / the same SCTs presented as TLS-delivered → `bad_sig`; no
   SCTs → `no_scts`; a list 71 days old → `stale_list`.
2. **Chrome's policy branches**: 18/18 (lifetime quorum, one operator, retired before/after, same log twice, tiled
   only, tiled + RFC 6962, missing leaf_index, both Option-1 cases, a retired log's TLS SCT, unknown log, no SCTs,
   operator history, stale list).
3. **Inclusion proofs**: transparency-dev/merkle `testdata/inclusion` (pinned, 98 files) **98/98** (6 accepted, 92
   refused as `wantErr` says); trees of 1..33 leaves built by Python hashlib: **561/561** proofs verified, 560
   altered proofs refused, every MTH equal.
4. **List signature** vs `openssl dgst -sha256 -verify`: 4/4 (RSA and P-256 keys, list and tampered list).
5. **TLS-delivered SCTs end to end**: a local CT log pair (`tests/oracle/ct_log.py`: an independent RFC 6962
   encoder, OpenSSL ECDSA) through `s_server -serverinfo` in TLS 1.2 (ServerHello) and 1.3 (leaf
   CertificateEntry): **16/16** — `policy`, `insufficient`, `bad_sig`, `no_scts`, report and strict; every strict
   refusal seen by the server as `alert bad certificate`.
6. **Chromium `securityState` on 10 public sites** (`tests/oracle/chromium_ct.js`: CDP
   `certificateTransparencyCompliance` + the served chain, to judge the same bytes): **0/10 comparable here** —
   anthropic.com, www.anthropic.com, claude.ai, raw.githubusercontent.com, github.com, pypi.org, index.crates.io,
   registry.npmjs.org all `ERR_CERT_AUTHORITY_INVALID` (the egress gateway re-terminates TLS and Chromium rightly
   refuses its CA — verification was not disabled to force it); www.google.com and en.wikipedia.org refused by the
   egress policy (`ERR_TUNNEL_CONNECTION_FAILED`). The policy is instead transcribed from Chromium's source and run
   on Chromium's own list (items 1–2).
7. **Must-staple** (`tests/m10_revocation.rs`, `gen_pki2.sh` leaf `ms`): **12/12** over TLS 1.2 and 1.3 — good
   staple accepted; none / `unknown` / not requested → refused, s_server logs alert 113 (bad certificate status
   response); revoked staple → `certificate_revoked`; a non-must-staple leaf without staple still accepted.
8. **CRLs vs `openssl verify -crl_check[_all] -use_deltas`** (CRLs from `gen_pki2.sh` via `openssl ca -gencrl`):
   **11/12 agree** (empty, keyCompromise, hold, hold + delta removeFromCRL, empty + delta keyCompromise, stale,
   wrong key, none, leaf + intermediate clean, intermediate revoked by the root's CRL, intermediate without CRL)
   **+ 1 documented divergence**: a delta CRL with no complete CRL — OpenSSL treats it as authoritative
   (`revoked`), RFC 5280 §5.2.4 / §6.3.3 only apply a delta on top of a complete CRL, so tls_core has no usable CRL
   (`BadCrl` when revocation is required). Both refuse the chain. Live: s_server with a CRL-revoked leaf, TLS 1.2
   and 1.3, refused, server sees `alert certificate revoked` (2/2).
9. **AIA**: s_server without `-cert_chain`; leaves pointing at `.p7c`, `.der`, a 404 and a foreign CA: **4/4**
   (verified / verified / unknown issuer / unknown issuer); `openssl verify -untrusted <the fetched cert>` agrees.
   http_core: the same through an Agent, `aia: false` → `cert-unknown-issuer`, `aia: true` → 200.
10. **ML-KEM KATs** (NIST ACVP, usnistgov/ACVP-Server@975de31e, `internalProjection.json`, pinned in
    `kat/vectors.txt`, fetched at test time, skipped offline): **240/240 = 100 %** — keyGen 75, encapsulation 75
    (and our decapsulation of our own ciphertext returns the same key), decapsulation 30 (valid and modified
    ciphertexts: implicit rejection), encapsulationKeyCheck + decapsulationKeyCheck 60 — for ML-KEM-512, -768 and
    -1024. **SHA-3/SHAKE**: SHA3-224/256/384/512 AFT + MCT, SHAKE128/256 AFT + VOT: **1364/1364** byte-oriented
    vectors; 6690 counted unsupported (bit-length messages/outputs, SHAKE's MCT, the 64 GiB LDT).
11. **X25519MLKEM768 vs OpenSSL 3.5** (`tests/m11_hybrid.rs`): Node 22's TLS on its OpenSSL 3.5.5, restricted by
    group: **7/7** — hybrid-only server → hybrid; hybrid+X25519 → hybrid; X25519-only → X25519 with no HRR (the
    alongside share); P-256-only → HRR; a hybrid-only client share vs X25519 server → HRR to X25519; no common group
    either way → both refuse (`no suitable key share`). On the wire: our ClientHello shares (0x11EC, 1216 B) +
    (0x001D, 32 B); the server's 0x11EC share 1120 B. **OpenSSL 3.5.9 built in the scratchpad**
    (`tests/oracle/build_openssl35.sh`) — `s_server -brief` reports **`Peer Temp Key: X25519MLKEM768`**: 4/4 (two
    hybrid configurations, X25519, prime256v1).

**Tests**: tls_core **51** (TLSCORE/TLSCORE2's 40 unchanged and green; m9 5, m10 3, m11 2; a JSON unit test),
vein_ring3 **13**, http_core host_e2e **8** + aia_e2e 1 (+ the rest of its suites), crypto_core unit 12 + KAT sets
m1..m7. Gate: `cargo test --release -p crypto_core -p tls_core -p vein_ring3 -p http_core --features host` all
green (with `OPENSSL35=` set for the s_server oracle; without it that one test skips).

## Metal (arroyo's own `build_user_lumen_x86` / `build_user_net_x86`, extracted verbatim and run on each tree)

| ELF | before (92f0eb06) | after (tip) | window need / cap | ELFENTRY |
|---|---|---|---|---|
| LUMEN-X86.ELF | 276 880 B | **326 152 B** (+49 272) | 3 662 168 / 4 194 304 | PASS |
| NET-X86.ELF | 226 136 B | **271 216 B** (+45 080) | 533 056 / 4 194 304 | PASS |

`./arroyo check` (the kernel links crypto_core; this arc changes no kernel file — `git diff 92f0eb06 -- unaos/crates/kernel`
is empty): every x86 leg that ran **rc=0** (x86-all, -vsyncpace, -ioapic, -netring3, -selfdiag) and the arm-virt legs
rc=0; **arm-pi and all 58 arm-tegra legs rc=101 on a PRE-EXISTING trunk break**, the same five errors in each —
`crates/kernel/src/install/partition.rs:1610-1612` names x86-only items (`super::selfinstall`, `block::WriteGrant`,
`BlockHandle::Ahci`, `grant_range`, `sata_is_boot_device`) without the `target_arch = "x86_64"` gate (UNAFSGROW
6df64116, already in 179b0658). A finding for the queue, not touched here.

## Constant-time statement (ML-KEM)

Branch-free and table-free on secrets: CBD sampling, NTT butterflies, base-case products, reduction mod q
(Barrett + masked subtraction), Compress_d (division by q replaced by a reciprocal multiply and a masked
correction — no hardware `div` on secret data, the KyberSlash class), m′ encoding, Decaps' re-encryption compare
(`ct_eq` over the whole ciphertext) and the K′/K̄ choice (a mask). Variable-time by design, on public data only:
SampleNTT's rejection loop (ρ is part of `ek`), the input checks and lengths. Multiplications are u32×u32→u64
(constant-time on x86_64/AArch64, the crate-wide caveat). Not done: a dudect-style timing measurement or a formal
proof; secrets are wiped best-effort (`black_box`), not with volatile writes (`deny(unsafe_code)`).

## Honest ceiling

* **CT list provenance**: Google's signed `log_list.json` + `log_list.sig` live on www.gstatic.com, which this
  egress refuses; the list used is the one Chromium compiles in, from the Chromium source tree at a pinned commit —
  authenticated by the git commit, **its signature unverified** (SOURCE says so). `tools/trust-bundle` verifies
  the signature when gstatic is reachable, but **Google's list key is not yet pinned in-tree** (it is fetched beside
  the list). **Apple's list** (valid.apple.com) is unreachable: not staged; Apple's own policy is not implemented
  (its logs would only contribute keys).
* **CT scope**: no auditing (no log is ever contacted; inclusion proofs only when a caller supplies one); no
  consistency proofs; `temporal_interval` read but not enforced (log admission, not a client rule in Chromium);
  strict mode refuses private-PKI chains too (Chrome exempts locally-trusted anchors — strict is opt-in); the
  Chromium `securityState` cross-check could not run here (0/10, item 6).
* **Revocation**: CRLs are caller-fetched (nothing fetches cRLDistributionPoints automatically); no indirect CRLs,
  no onlySomeReasons partitions, deltas only with freshestCRL; OCSP is still stapled-only (no OCSP fetching); the
  metal loads no CRLs and has no AIA fetcher (NET plumbing owed); CCADB not staged here.
* **ML-KEM / hybrid**: X25519MLKEM768 only (no SecP256r1MLKEM768 / pure ML-KEM), client role only, a fresh ML-KEM
  key per connection; bit-oriented SHA-3 lengths not supported (bytes only); never run on metal (the ELFs fit).

## Owed

1. Google's signed log list + an in-tree pin of Google's list key, and Apple's list, on a network that reaches
   gstatic / valid.apple.com (the tool path exists; run `tools/trust-bundle`).
2. CCADB intermediates (`tools/trust-bundle` on a network that reaches ccadb.my.salesforce-sites.com, or
   `--intermediates-from`).
3. The Chromium `securityState` cross-check on an unproxied network: `NODE_PATH=$(npm root -g) node
   tests/oracle/chromium_ct.js OUT <10 hosts>`, then judge each saved chain with `ct::evaluate`.
4. NET (metal) `IssuerFetcher` and CRL fetching + loading in vein_ring3; automatic cRLDistributionPoints fetching
   on the host (http_core) if wanted.
5. From TLSCORE2, still open: Holocron TicketStore on the metal, 1.2 resumption / client certificates, RFC 5280
   §7.1 name normalisation.

## How to continue

`cargo test --release -p tls_core` (python3 + openssl needed; `node` for m11; `OPENSSL35=` or
`sh tests/oracle/build_openssl35.sh` for the 3.5 s_server oracle; each test skips cleanly without its tool).
CT: `tls_core::ct::{LogList::parse_v3, evaluate, chrome_policy, merkle}`; a new policy case → `chrome_policy_branches`
in `tests/m9_ct.rs`. Revocation scenarios: `tests/oracle/gen_pki2.sh` (the `crl` helper makes any CRL shape);
cases in `tests/m10_revocation.rs`. ML-KEM: `cargo test --release -p crypto_core --test kat m7`
(`tools/crypto-check mlkem/` prints the counts). Metal sizes: source arroyo's two functions (as done here) or
`./arroyo esp-x86`.

## Third-party crates

None added; none in any product path (tls_core → crypto_core, both zero-dependency). Unchanged: tls_core's
RustCrypto test operand (feature `test-provider`, TLSCORE's original tests only) — a test operand, not chicken wire.
External ORACLES only (never linked): OpenSSL 3.0.13 CLI, OpenSSL 3.5.5 (inside Node 22), OpenSSL 3.5.9 (built in
the scratchpad), Python 3 hashlib, Chromium via Playwright.
