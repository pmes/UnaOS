# CRYPTOCORE — the cryptographic primitives UnaOS owns (LEDGER SR27)

Branch `exec-sec-crypto` (cut from 392d6412). Crate `unaos/libs/sys/crypto_core`: `#![no_std]`, optional
`alloc`/`std` conveniences, `#![deny(unsafe_code)]`, **zero dependencies** (not even a utility crate). The kernel
links it by path. The ring-3 workspace has it as a member. `tools/crypto-check` prints every known-answer set.

CHARTER: Holocron, shared-core (CODEX §2). Holocron owns keys and key policy. This crate is the arithmetic
both rings link. It holds no keys and has no bus surface, so it needs no handler of its own.

## Finding

Before this arc, ring 3's TLS was the third-party `embedded-tls` with no certificate verification. The kernel
had its own SHA-256/HMAC/PBKDF2 (`hash.rs`) and its own entropy pool (`rand.rs`). Nothing could do an AEAD, a
curve, a signature or a memory-hard password hash.

## Milestones

| M | commit | content |
|---|---|---|
| 1 | d74813aa | SHA-224/256/384/512, HMAC, HKDF, PBKDF2; the kernel's `hash.rs` is now a thin re-export of this crate (keeps CRC-32) |
| 2 | 823e9171 | ChaCha20, Poly1305, ChaCha20-Poly1305 + XChaCha20-Poly1305; bitsliced AES-128/192/256 (Boyar–Peralta S-box circuit, no tables); GCM/GMAC with constant-time GHASH |
| 3 | 66c835b7 | X25519, Ed25519, P-256 ECDH + ECDSA (RFC 6979), SEC 1, strict DER |
| 4 | 59a4636e | BLAKE2b, Argon2id/i/d (v0x10 + v0x13), ChaCha20 fast-key-erasure DRBG over `trait Entropy`, `ct_eq`/`ct_select`/`Zeroize`; `tools/crypto-check`; kernel `rand::KernelEntropy` |
| 5 | d1b59e0d | P-384 ECDH + ECDSA (RFC 6979 with HMAC-SHA-384) on a const-generic Montgomery core (`bignum.rs`); the DER reader shared with P-256 |
| 6 | 1b3a4fc6 | RSA verification: RSASSA-PSS + RSASSA-PKCS1-v1_5 (RFC 8017); the TLSCORE adapter completed |

## Specifications covered

FIPS 180-4 · RFC 2104 / FIPS 198-1 · RFC 5869 · RFC 8018 §5.2 · RFC 8439 (§2.3–2.8, XChaCha per
draft-irtf-cfrg-xchacha) · FIPS 197 · SP 800-38D · RFC 7748 · RFC 8032 (§5.1, PureEdDSA) · SP 800-186
(P-256, P-384) · FIPS 186-5 §6 · RFC 6979 · SEC 1 §2.3 · RFC 7693 · RFC 9106 · RFC 8017 §5.2.2, §8.1.2,
§8.2.2, §9.1.2, §9.2, §B.2.1 · Bernstein, "Fast-key-erasure random-number generators" (2017).

## Oracle method

The oracle is the published known answers, run as host tests (`cargo test -p crypto_core --release`, one
`#[test]` per milestone) and by `tools/crypto-check`. The vector files that are too large to commit are pinned
by URL and sha256 in `kat/vectors.txt`. They are fetched with the system `curl` at test time, checked against
the pin with this crate's own SHA-256 (proven first by the embedded FIPS 180-4 vectors), and the set is
reported SKIPPED when offline (`CRYPTO_OFFLINE=1` forces this). The same harness has a JSON reader (Wycheproof)
and a CAVP `.rsp` reader, both written here.

There are also two live oracles, from the consumers:

- **The TLSCORE fold proof.** I copied `exec-sec-tls@7abcca1f` read-only into scratch with `git archive` and
  dropped `adapters/tls_core_provider.rs` in as `src/cryptocore_provider.rs`. Every test file's
  `RustCryptoProvider` import was renamed to `CryptoCoreProvider`, with no other test changes. Result of
  `cargo test --release -p tls_core`: **25/25 pass**. That covers the RFC 8448 traces end to end (including
  the RSA server certificates), Wycheproof ECDSA through the trait, and live OpenSSL servers: three suites, HRR
  to P-256, P-256 / Ed25519 / RSA-PSS leaves, KeyUpdate and a 300 KB transfer. It also covers captured public
  chains, and public hosts checked against the pinned Mozilla bundle (certifi 2026.7.22, sha256 9cc2a774…).
  Both anthropic.com hosts verified through Let's Encrypt YE1 (P-384), and raw.githubusercontent.com verified
  with RSA-PSS.
- **The HOLOCRON1 fold proof.** I copied `unaos/libs/sys/holocron_core` read-only into scratch, enabled its
  `crypto_core` dependency exactly as its `Cargo.toml` comment says, and ran
  `cargo test --release -p holocron_core --features crypto_core`: **28/28 pass**, including `tests/cc.rs` 3/3.
  That is RFC 8032 TEST 1 through the agent, plus a real Argon2id + HKDF + ChaCha20-Poly1305 ring. Holocron's
  `src/cc.rs` compiles against this crate unchanged.

## Known-answer counts (`tools/crypto-check`, all online, 32.5 s)

**TOTAL: 49 sets, 52,534 pass, 0 fail, 302 unsupported** (SHA-1 vectors only), 0 skipped.

| primitive | sets (pass) |
|---|---|
| SHA-2 | FIPS 180-4 examples 14 · CAVP ShortMsg 388 · LongMsg 384 · streaming splits 5584 |
| HMAC | RFC 4231 34 · Wycheproof 522 |
| HKDF | RFC 5869 6 · Wycheproof 169 |
| PBKDF2 | RFC 7914 / RFC 6070-SHA256 9 · Wycheproof 60 |
| ChaCha20 / Poly1305 / AEAD | RFC 8439 5 / 12 / 7 · BoringSSL 66 · Wycheproof 325 |
| AES | FIPS 197 + AESAVS S-box 102 · AESAVS VarKey/VarTxt 1280 |
| AES-GCM | CAVP 31,500 · Wycheproof 316 |
| X25519 | RFC 7748 8 · Wycheproof 518 |
| Ed25519 | RFC 8032 3 · sign.input 1024 · Wycheproof 151 |
| P-256 | RFC 6979 7 · CAVP KeyPair 10 · SigGen 60 · SigVer 60 · KAS-ECDH 30 · Wycheproof ECDH 967 · ECDSA 746 |
| P-384 | RFC 6979 A.2.6 11 · CAVP KeyPair 10 · SigGen 60 · SigVer 60 · KAS-ECDH 30 · Wycheproof ECDH 1837 · ECDSA 1326 |
| RSA verify | CAVP 186-3 SigVer15 360 · SigVerPSS 360 · Wycheproof PKCS#1 v1.5 2589 · PSS 1161 · PKCS#1 sig-gen 35 |
| BLAKE2b | RFC 7693 29 · BLAKE2 KAT 256 |
| Argon2 | RFC 9106 §5 6 · PHC reference test.c 24 |
| DRBG | construction 8 |
| ct | helpers 5 |

The RSA Labs `pkcs-1v2-1d2-vec` PSS vectors are all SHA-1, so the CAVP 186-3 files are the PKCS #1 known answers
here.

## Constant-time statement

Each module states its discipline in its doc comment. The rules are: no branch and no memory index depends on a
secret, a secret choice is a mask, and multiplications are on `u64`/`u128` operands (constant-time on x86_64 and
AArch64, but not on Cortex-M3-class cores).

- Constant-time in key and data: hashes, HMAC, HKDF, PBKDF2, ChaCha20, Poly1305, AES (bitsliced) and GHASH.
- Constant-time in secret scalars: X25519, Ed25519 signing, and P-256/P-384 keygen, ECDH and signing.
- Variable-time by design, because every input is public: verification (Ed25519, ECDSA, RSA) and point
  decoding.
- Argon2: data-independent addressing for Argon2i and the first half-pass of Argon2id. Argon2d and the rest of
  Argon2id use data-dependent addressing, which is the algorithm's GPU-hardness trade-off and inherent to it.
- Comparison of tags and MACs: `ct_eq`.
- Key-bearing types zeroize on drop.

## Kernel leg

The leg is one x86 metal-shape check, run from `unaos/crates/kernel`:
`cargo +nightly check --release --target ../../x86_64-unaos.json -Z build-std=core,compiler_builtins,alloc
-Z build-std-features=compiler-builtins-mem -Z json-target-spec --features
"wc,quarry,ftdirx,login,loginst,nvidia-kepler-vblank,smc,usbnet,hda,hda-tone,facet,beam,sdw,selfhost,linuxabi,ahci,unafs,busreg"`.
Result: **rc=0** at M4 and again on the final crate. The brief's `kepler_vblank` is spelled
`nvidia-kepler-vblank` in the Cargo feature table. The target was deleted after each run.

## Adapters (what the siblings consume)

- **TLSCORE** (`trait CryptoProvider`): `adapters/tls_core_provider.rs`, a drop-in for
  `tls_core/src/cryptocore_provider.rs`. The orphan rule puts the `impl` in tls_core, because tls_core depends
  on crypto_core. Fold: add feature `cryptocore = ["dep:crypto_core"]`, plus `cryptocore-std` for host
  `new()`, the path dependency with `features = ["alloc"]`, and `#[cfg(feature = "cryptocore")] pub mod
  cryptocore_provider;`. Constructors mirror the test provider (`new`, `with_rng_pool`, `pool_remaining`), plus
  `with_entropy(Box<dyn Entropy>)` for the kernel and ring 3. Randomness comes from `ChaChaDrbg`.
  `supports_signature` is true for all nine schemes.
- **HOLOCRON1** (`Sealer`/`Signer`): its `src/cc.rs` already targets this API (`argon2::hash`,
  `hkdf::hkdf::<Sha256>`, `chacha20poly1305::{seal,open}`, `ed25519::{SigningKey::from_seed, verify}`). Fold:
  uncomment its dependency line and set `crypto_core = ["dep:crypto_core"]`. Its own `trait Entropy` is
  infallible (`fill(&mut self, buf)`). Bridge it with a wrapper over `crypto_core::drbg::ChaChaDrbg<E>` that
  turns `Err(Error::Entropy)` into Holocron's failure policy (refuse to seal).
- **Ring 3 entropy**: `drbg::GetrandomEntropy::new(|buf| sys_getrandom(buf))`, which loops short counts and
  surfaces errors. **Kernel**: `rand::KernelEntropy` (tail-appended to `rand.rs`, no consumer yet).

## Honest ceiling (what is NOT here)

- No SHA-1, so SHA-1 KATs are counted as unsupported.
- No RSA keygen, signing or decryption (by design: verify only).
- No P-521, SHA-3 or BLAKE3, and no ML-KEM/ML-DSA.
- The DRBG has no standard KAT; it is proven against its own definition, with ChaCha20 and SHA-256 underneath
  both KAT-proven.
- No timing-measurement oracle (dudect-style) has been run. The constant-time claims rest on construction and
  review.

## Owed

1. SHA-3 / SHAKE (FIPS 202) and BLAKE3.
2. P-521, if a public chain ever needs it.
3. `fs/users.rs` moves from PBKDF2-HMAC-SHA256 (now through this crate) to Argon2id. This is a record-format
   change plus a kernel memory budget, so it needs its own gated arc.
4. The kernel's `SYS_GETRANDOM` generator swapped onto `ChaChaDrbg<KernelEntropy>`.
5. `arch/aarch64/syscall.rs`'s private SHA-256 retired onto this crate.
6. A dudect-style timing harness.
7. The fold itself: two adapter files and three Cargo lines, as listed above.

## Third-party crates

None in `crypto_core` or `crypto-check`. The `test-provider` crates (RustCrypto) belong to TLSCORE's test
operand, not to this arc.
