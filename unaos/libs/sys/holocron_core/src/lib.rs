// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `holocron_core` — the keyring core of Holocron ("The Key", CODEX §2), shared by Ring 0 and Ring 3
//! (HOLOCRON1, LEDGER SR33).
//!
//! CHARTER: Holocron — shared-core. Holocron (`handlers/holocron`, CODEX Secrets handler: keyring, SSH
//! agent, wallet, biometric auth) owns secrets and their policy. This crate is the part both rings link:
//!
//! | module | what |
//! |---|---|
//! | [`format`] | the v1 secret file and ring file formats (fail-closed parsers, header KATs) |
//! | [`seal`] | the `Sealer` / `Signer` / `Entropy` traits — the whole contract CRYPTOCORE must meet |
//! | `testseal` (feature `test-suite`) | the INSECURE test suite (suite 0xFE) — tests and fixtures only |
//! | [`cc`] | the production suite over CRYPTOCORE: Argon2id, HKDF-SHA-256, ChaCha20-Poly1305, Ed25519, and the DRBG entropy bridge |
//! | [`ring`] | the ring: Argon2id key from the login password, held for the session, `lock` wipes it |
//! | [`wire`] | the bus verbs (144..=151), bodies, statuses, principal projection |
//! | [`frame`] | the BANDY v1 frame around a body — the metal transport (HOLOCRON2, B355) |
//! | [`service`] | the dispatcher: owner-only, rate-limited unlock, the `Store` seam, the session cache (`SessionStore`) |
//! | [`agent`] | the SSH agent protocol framing over the ring's Ed25519 keys |
//! | [`keysource`] | the consumer rule: ask Holocron first, fall back only on NotFound |
//! | [`root`] | THE store root (`<home>/.holocron`), named once for both rings, and the one-time legacy migration (B448) |
//!
//! ## The ring unlocks once per session (LUMENFAST2, rmbp-ledger B515)
//!
//! The ring key `K` is derived ONCE per session: at login (RINGLOGIN, B465: the kernel runs Argon2id over the
//! typed password and posts `K` in a take-once door the fulfiller applies with
//! [`service::Holocron::unlock_with_key`]) or at a typed `unlock`. Every later request answers from memory:
//! [`ring::Ring`] holds `K`, and [`service::SessionStore`] holds the SEALED bytes of the ring file and of each
//! secret file read or written this session (and the "absent" answers), so a consumer's second ask — Lumen's
//! `vein/claude.api_key` at every start — is one bus round trip with no KDF and no medium read. A lock (the lock
//! screen's door LOCK, the `lock` verb) wipes `K` and the cache together ([`Store::forget`](service::Store::forget));
//! the next unlock derives again and reads the medium afresh. Logout ends the session's programs (SECLOGIN), the
//! fulfiller with them.
//!
//! Why this is no weaker than what it replaces: the material a consumer asks for (an API key) already lived
//! in the CONSUMER's process for its whole TLS session; the cache lives in the secrets handler's own process,
//! which is the one place the ring key must live anyway, and it holds ciphertext — a plaintext exists only for
//! the request that opens it. Re-reading the medium on every request bought nothing against a reader of this
//! process's memory (it holds `K`), and on the metal each read was a fresh path walk (flight 28: 900 ms of them
//! per Lumen start). What the cache gives up is noticing a file written behind the fulfiller's back before the
//! next lock — on the metal the fulfiller is the store's only writer, so HOLOCRON.ELF opts in; the host daemon
//! does not.
//!
//! Nothing here touches a file, a socket or a syscall; each ring brings its own [`service::Store`] and
//! transport. One dependency: CRYPTOCORE (UnaOS-built).
#![no_std]
#![deny(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod agent;
pub mod cc;
pub mod format;
pub mod frame;
pub mod keysource;
pub mod name;
pub mod ring;
pub mod root;
pub mod seal;
pub mod service;
#[cfg(feature = "test-suite")]
pub mod testseal;
pub mod wire;
pub mod zero;

/// Equality of two byte strings in time independent of their contents (lengths are public).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        d |= x ^ y;
    }
    core::hint::black_box(d) == 0
}
