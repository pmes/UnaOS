# VEINTLS (LEDGER SR36): UnaOS's own TLS under Lumen

Branch `exec-host-veintls`, cut at 43743808 (exec-sec-tls and exec-sec-crypto merged in). exec-sec-crypto ed49c387
(P-384, RSA verify, the tls_core adapter) was merged mid-arc at 925b2b21. Commits: M1 6440aa1f, M3 a419cd14,
fold 925b2b21, M1b 50198cd7, M2 a7f0554a, M4 75212d43, plus this doc.

## Finding

`vein_ring3` carried `embedded-tls` 0.19 with `UnsecureProvider`, so it checked no certificate at all. The API key
crossed the wire only when the operator set `vein.tls = "insecure"`. Every TLS byte under Lumen and NET.ELF is now
UnaOS code: `tls_core` (TLSCORE) runs the protocol and X.509 path validation, and `crypto_core` (CRYPTOCORE,
through its adapter folded into tls_core as `cryptocore_provider.rs`) supplies every primitive. Every connection is
verified against `/system/trust/roots.pem` at the kernel's wall-clock time, and the host name is matched as
RFC 6125 requires. The insecure mode is gone. `embedded-tls`, `embedded-io` and `rand_core` have left the tree.

## What changed

| where | what |
|---|---|
| `unaos/libs/sys/vein_ring3` | `tls.rs`: `with_session` (tls_core `Client` over any `vein_core::client::Transport`, `WebPkiVerifier` that records the issuer CN, ALPN http/1.1, close_notify), `Session` as the Transport for `client::exchange`, `describe(TlsError)` gives stable reasons (`cert-unknown-issuer`, `cert-name-mismatch`, `clock-unset`, …). `clock.rs`: `SysClock` over SYS_TIME, plus `CLOCK_FLOOR` 2026-10-04 (an earlier reading counts as unset and the handshake is not attempted). `trust.rs`: SYS_STAT plus SYS_OPEN plus SYS_READ of `/system/trust/roots.pem` into `TrustStore::from_pem` (1 MiB cap). `heap.rs`: a power-of-two size-class `GlobalAlloc` over SYS_SBRK (O(1), frees and reuses, align ≤ 4096). `provider.rs` (feature `cryptocore`): `CryptoCoreProvider::with_entropy(GetrandomEntropy(SYS_GETRANDOM))`. `lib.rs`: `exchange_over` (the transport-agnostic exchange the host test drives), `send(…, Option<&TlsContext>, …) -> Sent { out, verified }`, `Stage::{Handshake,TlsStream}(TlsFail)`, `Stage::KeyOverPlain` (a key is never encoded for http://), the request head is zeroed on every path, and `TlsSetup` (provider + store + clock; `verify()` feeds the key rule). It is now a root-workspace member. |
| `vein_core::prefs` | `TlsPolicy`/`tls_policy` are deleted. `plan(p, ep, key, verify: Verify)`, where `Verify::{Ready, NoProvider, NoTrustStore, NoClock}`. Any value other than `Ready` gives `Echo(Reason::TlsUnverified(v))`, and the reason text names the missing piece ("key not sent: …"). |
| `una-abi` + kernel | **`SYS_TIME` = 60**: `-> UTC Unix seconds / -EAGAIN` from `clock::unix_now`. It is unconditional on x86_64 and aarch64, a same-line fold on the `SYS_CLOSE` arm, with the body at the file tail. Number 59 belongs to RING3ABI2's `SYS_WHOAMI`. Ring 3 had no wall clock before this. `kernel/lumen.rs` (`tests lumen`) uses the same rule: trust store staged plus clock set. |
| `crates/user-lumen` | `#[global_allocator]` heap, `TlsSetup::load()`. Start line: `… transport=<t> trust=<n|none> clock=<set|unset> [crypto=<why>]`. Each reply: `:: LUMEN: reply … transport=tls verified=<issuer CN> ::`, and the window notes `transport=tls verified=<issuer CN>`. A failure prints `tls=<why>`. The insecure note is deleted. Feature `cryptocore` (passed by arroyo's `build_user_lumen_x86`). `build-std` gains `alloc`. |
| `crates/user-net` | NET.ELF now runs on vein_ring3: clear `GET /` on :80, then a **verified** TLS `GET /` on :443 → `tls=<status> verified=<CN>`, or `tls=skip reason=<no-provider|no-trust-store|clock-unset>`. It links at the ELF window using RING3ABI2's `user-net-x86.ld` verbatim, with a 256 KiB stack and `--features cryptocore`. `tls-spike/` is deleted. |
| prefs schema | `vein.tls` **never had a row** (PRINCIPIA2's scanner could not see `one("vein.tls", …)`), so the only thing to delete was code. `tools/prefs-schema-check.py` gains **R7** (dotted literals in a file that speaks `BUS_VERB_PREF_GET`). Its `--selftest` plants `one("vein.tls", …)` and must go red, so the deletion is enforced. R7 found two live keys without rows, `vein.endpoint` and `vein.key_file`; both are now declared, and `PREFS_SCHEMA_BLESS=1 cargo test -p prefs_core --test schema_gate` re-blessed `docs/dev/PREFS-SCHEMA.md` (27/27). |
| builder | `stage_trust`: `<repo>/system/trust/roots.pem` goes to `system/trust/roots.pem` on the ESP **and** the DATA volume (the one the kernel reads). The sha256 is computed with `crypto_core::sha2::sha256` and must equal `system/trust/roots.pem.sha256`, otherwise the build stops. If the bundle is absent, that is said and nothing is staged (LUMEN then answers Echo with "no trust store"). |

**The 40-byte path cap:** `/system/trust/roots.pem` is 23 bytes, within the kernel's SYS_OPEN `MAX_NAME` = 40 on both
arches (const-asserted in `trust.rs`). On x86 the dynamic-open path walks it on the mounted FAT volume as
`SYSTEM/TRUST/ROOTS.PEM` (every component is 8.3). RING3ABI2 does not need to lift the cap for this file. The cap
binds `vein.key_file` instead (its schema row says ≤ 40).

## Trust bundle

Run `tools/trust-bundle` before `./arroyo esp-x86`. Today's bundle is certifi 2026.7.22 (curl.se is unreachable
here), 121 roots (1 key unusable: P-521), 240 216 B,
**sha256 `9cc2a774b5198dcff14d9be1e66091f538975d867ce029a96bce15a55dfd730f`** (pinned in `system/trust/roots.pem.sha256`).
The file stays gitignored.

## Oracle and tests

`cargo test --release -p vein_ring3 -p tls_core`: all green (vein_ring3: 4 heap unit + 9 integration; tls_core
25). The vein_ring3 tests run on the **product provider** (`CryptoCoreProvider`, tls_core `cryptocore-std`). No
third-party crypto is involved.

1. **Offline, Python `ssl` = OpenSSL 3.0.13, TLS 1.3 only, 127.0.0.1** (`tests/oracle/messages_server.py`, a
   Messages-API-shaped endpoint with a chunked SSE answer in awkward chunk sizes, plus thinking/signature deltas,
   ping and an escaped quote). The PKI is generated per run by tls_core's openssl script. Over a std socket:
   - P-256 leaf: client `transport=tls verified=TLSCORE Oracle Intermediate status=200 stop=EndTurn text_bytes=3047`.
     Server: `version=TLSv1.3 cipher=TLS_AES_256_GCM_SHA384 alpn=http/1.1 method=POST path=/v1/messages
     key=<the test key> version_hdr=2023-06-01 clen=178 got=178 stream=true model=claude-opus-5-5`, clean close_notify.
     The decoded text matches byte for byte.
   - RSA leaf (rsa_pss_rsae_sha256 CertificateVerify) and Ed25519 leaf: both verified, 200, EndTurn.
   - Refusals. Each one ends **before one application byte**; the server reports `HANDSHAKE-FAIL … app_bytes=0`, so
     the key never left:
     - foreign root → `cert-unknown-issuer`, and OpenSSL sees `TLSV1_ALERT_UNKNOWN_CA`
     - wrong name → `cert-name-mismatch`, and OpenSSL sees `BAD_CERTIFICATE`
     - clock 0 → `clock-unset`, with no ClientHello sent
     - no context → `no-tls-context`, with nothing written
     - a key for http:// → `KeyOverPlain`
   - The request head that held the key is all zeros after every one of these.
2. **Online, the real `api.anthropic.com`, no key.** Against the Mozilla bundle the result is REFUSED
   `cert-unknown-issuer`: this container's egress gateway re-terminates TLS (chain `*.anthropic.com` ← Egress
   Gateway SDS Issuing CA ← sandbox-egress-gateway CA), so the verifier is correctly refusing a MITM. With the
   egress CAs as the trust store (`/root/.ccr/ca-bundle.crt`, 155 anchors, reported separately and never in place
   of Mozilla) the same client VERIFIED `issuer=Egress Gateway SDS Issuing CA (production)`, and the real Messages
   API answered `401` with the decoded error event `"x-api-key header is required"`.
3. The real Mozilla bundle through `trust::parse`: loaded 121, rejected 0, sha256 = pin. Builder test: staged
   byte-identical, and a 1-bit tamper panics.
4. vein_core `the_rules`: every `Verify` other than `Ready` keeps the key home, a relay never gets the key, and no
   reason text says "insecure".

**Ring-3 builds** (arroyo's lines, `--features cryptocore`, after the CRYPTOCORE merge):
- user-lumen x86: **rc=0**, stripped LUMEN-X86.ELF 189 368 B, `ELFENTRY … -> PASS`. `transport=tls verified=` is
  present in the image (`LC_ALL=C grep -a -o -F`).
- user-net x86: rc=0, 160 568 B, ELFENTRY PASS.
- aarch64 check legs of both: rc=0.
- Before the merge, the same lumen line stopped at the provider stub's `compile_error!`, which named the adapter +
  `drbg`, RSA-PSS/PKCS#1 v1.5 and P-384. That stub has been replaced by the adapter.
- `./arroyo check`: shell, braces, x86_64 OK, aarch64 OK, bootloader OK and the first coverage leg (x86-all rc=0)
  passed both before and after the merge. The 87-leg sweep was stopped (about 1.5 h) and was not run to the end.

## Honest ceiling

* **Never run on metal.** The handshake, SYS_TIME, the FAT read of the 240 KB bundle and the heap are all untested
  on the rMBP. Whether the rMBP's clock is anchored (RTC/SNTP) at `lumen` time decides `clock=set`.
* The trust store is parsed at every program start (about 240 KB read plus 121 certificates parsed): the cost on
  metal is unmeasured.
* No revocation, no OCSP, no CT, no resumption (TLSCORE's ceiling). Only one connection per exchange; no keep-alive.
* `tls_core`'s own tests still run on its RustCrypto `test-provider` (TLSCORE owns that switch; CRYPTOCORE reports
  25/25 with the adapter swapped in).
* NET.ELF has no argv on this branch (RING3ABI2 adds `argv[1]`), so its host is fixed.
* The aarch64 LUMEN/NET images are owed (ELF window on aarch64: RING3ABI2).

## Owed / fold notes

1. **Fold with RING3ABI2 (exec-rmbp-ring3abi2).** Expect conflicts in:
   - `user-net/src/main.rs`: take VEINTLS's, then re-add RING3ABI2's `args()` host (about 6 lines).
   - arroyo `build_user_net_x86`: the RUSTFLAGS/cargo lines; take `stack-size=0x40000 … --features cryptocore`.
     The rest of that hunk is identical.
   - the una-abi tail (keep both: 59 WHOAMI, 60 TIME).
   - doc comments in `vein_core/src/prefs.rs`.
   - `vein_ring3/src/key.rs` (theirs adds `default_path`; no overlap).
   - `user-lumen/src/main.rs` (keep both).
2. Flight: `tests lumen` gains `tls=verified` once a metal handshake has run. Read `:: LUMEN: reply … transport=tls
   verified=…` and NET's `tls=<status> verified=<CN>`.
3. Holocron (SR33) should hand vein_ring3 the key in place of the key file.

## Third-party crates this arc leans on

None in the product path: vein_ring3, tls_core (`cryptocore`) and crypto_core have zero third-party dependencies,
and so do vein_ring3's dev-dependencies. Removed: `embedded-tls` 0.19, `embedded-io` 0.7 and `rand_core` 0.6 (chicken
wire: they did the TLS). Unchanged and outside this arc: tls_core's test-only RustCrypto operand (feature
`test-provider`, its own tests only), `font8x8` 0.3.1 in user-lumen (utility: a glyph table), and builder's `fatfs`
0.3 / `crc32fast` 1 (utilities).
