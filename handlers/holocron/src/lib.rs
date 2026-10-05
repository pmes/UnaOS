// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Holocron on the host — "The Key" (CODEX §2: keyring, SSH agent, wallet, biometric auth).
//!
//! CHARTER: Holocron — handler. The keyring policy, file formats, bus codec and agent framing live in
//! `holocron_core` (shared with the metal). This crate brings the host's half:
//!
//! * [`store`] — the `~/.holocron/<ns>/<name>` directory store (0700/0600, atomic replace) and
//!   [`unafs_store`] — the same layout on a UnaFS volume with the metadata as typed attributes;
//! * [`principal`] — the host's stamp: the peer uid the kernel reports for a Unix socket
//!   (SO_PEERCRED), projected to `user:<name>#<uid>`, the string the UnaOS kernel stamps;
//! * [`daemon`] — the Holocron socket (the bus verbs) and the SSH agent socket;
//! * [`client`] — what consumers link (Vein's API key, the `holocron` CLI).
//!
//! Secrets never ride the `bandy` Synapse: it is an in-process BROADCAST channel with no principal, so
//! every subscriber would see every reply. Holocron's bus is point-to-point and principal-stamped.
#![feature(peer_credentials_unix_socket)]

pub mod client;
pub mod daemon;
pub mod principal;
pub mod store;
pub mod unafs_store;

pub use holocron_core;

/// The sealer the host daemon runs: CRYPTOCORE's Argon2id + HKDF-SHA-256 + ChaCha20-Poly1305.
pub type HostSealer = holocron_core::cc::CryptoCore;
/// The signer the host daemon runs: CRYPTOCORE's Ed25519.
pub type HostSigner = holocron_core::cc::CryptoCore;
/// The entropy the host daemon draws: CRYPTOCORE's ChaCha20 DRBG seeded from /dev/urandom; a failure
/// refuses the operation.
pub type HostEntropy = holocron_core::cc::DrbgEntropy<crypto_core::drbg::OsEntropy>;
/// The host service: the production suite over the `~/.holocron` directory.
pub type HostHolocron = holocron_core::service::Holocron<HostSealer, HostSigner, store::DirStore, HostEntropy>;

/// Seed the host entropy (`Err` when /dev/urandom cannot deliver: the daemon then refuses to start).
pub fn host_entropy() -> Result<HostEntropy, holocron_core::seal::EntropyError> {
    holocron_core::cc::DrbgEntropy::new(crypto_core::drbg::OsEntropy, b"UnaOS Holocron host daemon")
}
