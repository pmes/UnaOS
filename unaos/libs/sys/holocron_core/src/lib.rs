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
//! | [`testseal`] | the INSECURE test suite (suite 0xFE) used until CRYPTOCORE folds |
//! | `cc` (feature `crypto_core`) | the production adapter: Argon2id, HKDF-SHA-256, ChaCha20-Poly1305, Ed25519 |
//! | [`ring`] | the ring: Argon2id key from the login password, held for the session, `lock` wipes it |
//! | [`wire`] | the bus verbs (144..=151), bodies, statuses, principal projection |
//! | [`service`] | the dispatcher: owner-only, rate-limited unlock, the `Store` seam |
//! | [`agent`] | the SSH agent protocol framing over the ring's Ed25519 keys |
//! | [`keysource`] | the consumer rule: ask Holocron first, fall back only on NotFound |
//!
//! Nothing here touches a file, a socket or a syscall; each ring brings its own [`service::Store`] and
//! transport. No dependencies.
#![no_std]
#![deny(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod agent;
#[cfg(feature = "crypto_core")]
pub mod cc;
pub mod format;
pub mod keysource;
pub mod name;
pub mod ring;
pub mod seal;
pub mod service;
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
