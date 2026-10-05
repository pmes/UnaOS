# HOLOCRON1 — Holocron gets a keyring, a bus surface, an SSH agent, and its first consumer

LEDGER SR33 · branch `exec-host-holocron1` · cut from `676ca0e9` · host first · CODEX §2 Holocron ("The Key").
CRYPTOCORE (SR27, `exec-sec-crypto` ed49c387) is MERGED into this branch (39c52809) and is the default suite (8fc3baf8).

## Finding

Holocron had no surface anything used: its README was a design note with no crate, Lumen/Vein read the
Claude API key from an environment variable named by a preference (B323), there was no bus verb to ask
for a secret, no per-user keyring on UnaFS, and no agent.

## What exists now

| piece | where | what |
|---|---|---|
| keyring core | `unaos/libs/sys/holocron_core` (`no_std` + `alloc`, one dependency: CRYPTOCORE) | v1 secret and ring file formats, the `Sealer`/`Signer`/`Entropy` seam, the INSECURE test suite (feature `test-suite`, tests only), the ring (Argon2id key from the login password, session-held, `lock` wipes), the bus codec, the dispatcher (owner-only, rate-limited unlock, `Store` seam), the SSH-agent framing, the consumer rule |
| production suite | `holocron_core/src/cc.rs` | `CryptoCore`: Argon2id (RFC 9106) + HKDF-SHA-256 (RFC 5869) + ChaCha20-Poly1305 (RFC 8439) + Ed25519 (RFC 8032); `DrbgEntropy`: CRYPTOCORE's ChaCha20 DRBG over a fallible source — the host daemon's entropy |
| host handler | `handlers/holocron` | `DirStore` (`~/.holocron`, 0700/0600, atomic replace, no symlinks), `UnaFsStore` (same layout on a UnaFS volume, metadata as typed attributes), SO_PEERCRED principal, daemon (bus socket + agent socket, idle lock), client |
| CLI | `tools/holocron` (package `holocron-cli`, binary `holocron`) | `daemon`, `init`, `unlock`, `lock`, `status`, `put`, `get`, `list`, `delete`, `keygen`, `sign`, `agent-env` |
| first consumer | `handlers/vein/src/provider.rs` | `ProviderSlot::load` asks Holocron for `vein/claude.api_key` first |

## The seam (the whole contract CRYPTOCORE must meet)

```rust
pub trait Sealer {
    const SUITE: u8;
    fn derive_key(&self, password: &[u8], salt: &[u8; SALT_LEN], params: &KdfParams) -> Result<Key, SealError>;
    fn subkey(&self, key: &Key, salt: &[u8], info: &[u8]) -> Key;
    fn seal(&self, key: &Key, nonce: &[u8; NONCE_LEN], aad: &[u8], plaintext: &[u8]) -> Vec<u8>;
    fn open(&self, key: &Key, nonce: &[u8; NONCE_LEN], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, SealError>;
}
pub trait Signer {
    const REAL: bool;
    fn public_key(&self, seed: &[u8; 32]) -> [u8; 32];
    fn sign(&self, seed: &[u8; 32], msg: &[u8]) -> [u8; 64];
    fn verify(&self, public: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool;
}
pub trait Entropy {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), EntropyError>;
}
```

`Entropy` is FALLIBLE: an `Err` refuses the operation — `Ring::create` and `seal_secret` return
`RingError::Entropy`, the bus answers `IO` (-5), a minted SSH key is never minted, nothing reaches the store.
`DrbgEntropy::new` fails when the source cannot seed the DRBG, and the daemon then refuses to start
(`tests/cc.rs::cc_entropy_failure_refuses_to_seal`).

`derive_key` = Argon2id v0x13, 32-byte tag; `subkey` = HKDF-SHA-256 → 32 bytes; `seal`/`open` =
ChaCha20-Poly1305 returning `ct || tag16`. `Signer` = RFC 8032 PureEdDSA over a 32-byte seed.

## Formats (spec text: `holocron_core/src/format.rs`)

* Secret file `/home/<u>/.holocron/<ns>/<name>`: `"HCRN" v1 suite header_len | Argon2id m,t,p | file salt(16) | nonce(12) | created i64 | sealed_len | kind_len label_len kind label | sealed body`. File key = HKDF(ring key, file salt, `"holocron/v1/secret"`). AAD = `header || ns || 0x00 || name`: renaming, moving, or editing a label/kind/created/params fails authentication.
* Ring file `/home/<u>/.holocron/.ring`: `"HCRR" v1 suite 0 | Argon2id m,t,p | salt | nonce | owner (user:<name>#<uid>) | verifier(48)`; the verifier is `VERIFIER_PLAINTEXT` sealed under HKDF(ring key, salt, `"holocron/v1/verifier"`) with the header as AAD, so a wrong password is caught without touching a secret, and a header edit (owner, params, suite) refuses unlock.
* Typed attributes on UnaFS (`created` Int, `kind` Str, `label` Str): a mirror for `query`, never the authority.
* KDF floor 19 MiB / t=2 / p=1 (OWASP 2023); default RFC 9106 §4 "second recommended" (64 MiB, t=3, p=4). A header below the floor is refused before the KDF runs.
* Suite 0x01 = production, 0xFE = TEST-INSECURE; each sealer refuses the other's files.

## Bus surface (M2) — `holocron_core/src/wire.rs`

Verbs 144..=151 in BANDY3's registrable range: `SecretGet`, `SecretPut`, `SecretList`, `SecretDelete`, `Unlock`
(bit0 = create), `Lock`, `Sign`, `Status`. Statuses are una-abi errnos. Policy, in order: caller principal must
equal the owner (`DENIED` before the body is decoded); a ring owned by someone else is `DENIED`; `Unlock` gets
3 free failures then 1 s, 2 s, 4 s … capped at 5 min of `RATE_LIMITED` (no KDF run); `SecretGet` says
`NOT_FOUND` for an absent secret (the only fallback answer), `LOCKED` for a present one while locked; a tampered
file is `CORRUPT`, never `NOT_FOUND`. On the host the stamp is SO_PEERCRED → `user:<name>#<uid>` from
`/etc/passwd`; on the metal it is the kernel's `PRIN_USER` record (`principal_from_record`; every other record
kind projects to `None`). Secrets never ride `bandy`'s Synapse: a broadcast channel with no principal.

## SSH agent (M3) — `holocron_core/src/agent.rs`

draft-ietf-sshm-ssh-agent §3 framing; `REQUEST_IDENTITIES`, `SIGN_REQUEST`, `LOCK`, `UNLOCK`; everything else
(including `ADD_IDENTITY`) is `FAILURE`, so keys only enter through `SecretPut`/`holocron keygen`. RFC 8709 §4/§6
blobs. Keys live in namespace `ssh`, kind `ssh-ed25519`, plaintext = the RFC 8032 seed; an empty `SecretPut`
body mints the seed from Holocron's entropy. A test-signer key's comment carries ` [TEST-INSECURE]`.

## Oracles

| what | oracle | result |
|---|---|---|
| formats | header KATs written field by field from the layout table (not from the encoder), parser refusals per field | 5 KATs, 18 refusal cases green |
| bus bodies | body KATs written from the verb table | green |
| agent bytes | blob/signature/frame KATs written from RFC 8709 and the agent draft | green |
| agent over a real socket | `tools/holocron/tests/agent_oracle.py`: an independent agent client; signature checked by RFC 8032 §6's own Python reference verifier (self-tested on §7.1 TEST 2 before trusting it) | **ED25519-VERIFIED(rfc8032-ref)** on the production suite; blob/shape-only under the test signer |
| `ssh-add -l` | OpenSSH | SKIPPED: OpenSSH is not installed in this container; the e2e test runs it when present |
| the real primitives | CRYPTOCORE merged; `cargo test -p holocron_core -p holocron -p holocron-cli` (before the merge, the same from a scratch copy against 66c835b7) | **every test green**, including RFC 8032 §7.1 TEST 1 and TEST 2 through `Signer`, TEST 1's signature through the agent and the `Sign` verb byte for byte, a real Argon2id (19 MiB, t=2) ring with wrong-password, tamper and cross-suite refusals |

(pyca/cryptography was the first choice of oracle; the distro copy panics on import in its pyo3 binding, so the
RFC's reference code, which has no dependency, is the oracle.)

Mutation (go-red): disabling the owner check in `Holocron::authorise` turns `owner_only_before_decode`,
`unlock_rate_limit` and `another_principal_is_denied_by_the_peer_credential` red.

## Tests

`cargo test -p holocron_core -p holocron -p holocron-cli` on the real primitives: **37 pass, 0 fail** —
holocron_core 1 unit + 12 kat + 9 bus + 3 agent + 4 cc (RFC 8032 TEST 1/2, the agent signing TEST 1, a real
Argon2id ring, the entropy refusal); holocron 4 m1_store + 3 m2_bus; holocron-cli 1 e2e (the daemon on the
production suite, Argon2id 64 MiB/t=3, `ED25519-VERIFIED(rfc8032-ref)` from the independent agent client).
The store/socket/bus tests use the test suite deliberately (fast KDF); the cc and e2e tests are the real
ones. Vein: 3 new provider tests (Holocron wins / fallback / refuse, and a live daemon end to end).

## Third-party crates

holocron_core: none (CRYPTOCORE is UnaOS's own). holocron: none of its own; through `unafs` (default features off) it links UnaFS's own
utilities bincode 2.0.1, serde 1.0.229, thiserror 2.0.21, libm 0.2.16 — none does Holocron's work. holocron-cli:
none. Vein: no new crate. The cryptography is CRYPTOCORE's (UnaOS-built), never a crate.

## Honest ceiling

* No metal fulfiller: HOLOCRON.ELF (register verbs 144..=151 with BUS_VERB_REGISTER, store through
  SYS_OPEN/SYS_ATTR_SET, entropy from SYS_GETRANDOM) is owed. The metal Vein (`unaos/crates/user-vein`, the
  ring-3 VEIN.BIN) holds no credential today (its providers are echo and relay; the relay's key is consumed
  host-side), so there is no key read on the metal to redirect yet; `holocron_core::keysource::decide` is the
  rule it links when it gains one (an unregistered verb's -ENOENT is the same decision as NotFound).
* No biometric auth, no wallet, no out-of-band confirmation prompt, no TPM sealing (CODEX lists them).
* Password entry uses `stty -echo` on a terminal; no pinentry.
* The SSH agent signs Ed25519 only (no RSA/ECDSA, no certificates, no `session-bind@openssh.com`).
* The daemon is not supervised (no systemd unit / launchd plist, no lock on screen-lock).
* UnaFS replaces a secret by unlink + create (its `write_data` is grow-only), so a crash between the two loses
  that one secret; the directory store is atomic (temp + fsync + rename + dir fsync).

## Owed

1. CRYPTOCORE's own fold to a track lands with or before this branch (it is merged here, not on trunk).
2. HOLOCRON.ELF (metal fulfiller) and the kernel's verb registration; LOGIN hands it the password at session start.
3. Seat: CODEX §2 entry for Holocron's bus verbs and `tools/holocron`; `docs/dev/exec-branches.txt` line at the fold.
4. `ssh-add -l` / `ssh -T git@…` on a bench with OpenSSH.

## Handler charter (for the seat's CODEX §2 entry)

**Holocron — Secrets ("The Key").** Owns every credential a person holds — API keys, passwords, SSH and signing
keys — and the policy for releasing them. Secrets are files under `/home/<u>/.holocron/<ns>/<name>`, sealed at
rest under a ring key derived from the login password, held in memory only while the session is unlocked, and
released only to the principal the kernel stamps as their owner. Consumers ask, they never read the store: bus
verbs `SecretGet/SecretPut/SecretList/SecretDelete/Unlock/Lock/Sign/Status` (144..=151); the SSH agent protocol
on the agent socket. The arithmetic is CRYPTOCORE's.

## How a future executor continues

Read this file, then `holocron_core/src/lib.rs` (module table). The core has no I/O: a new transport brings a
`Store`, an `Entropy` (on the metal: `DrbgEntropy` over crypto_core's `GetrandomEntropy`) and calls
`Holocron::handle(caller, verb, body, now_ms, now_unix)`. `cargo test -p holocron_core -p holocron -p
holocron-cli -- --nocapture` and look for `ED25519-VERIFIED(rfc8032-ref)` in the e2e output.
