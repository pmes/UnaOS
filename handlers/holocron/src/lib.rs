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

/// The sealer the host daemon runs: CRYPTOCORE with `--features crypto_core` (after SR27 folds), the
/// INSECURE test suite otherwise — which the daemon announces on every start.
#[cfg(feature = "crypto_core")]
pub type HostSealer = holocron_core::cc::CryptoCore;
/// The signer the host daemon runs.
#[cfg(feature = "crypto_core")]
pub type HostSigner = holocron_core::cc::CryptoCore;
/// The sealer the host daemon runs: CRYPTOCORE with `--features crypto_core` (after SR27 folds), the
/// INSECURE test suite otherwise — which the daemon announces on every start.
#[cfg(not(feature = "crypto_core"))]
pub type HostSealer = holocron_core::testseal::TestSealer;
/// The signer the host daemon runs.
#[cfg(not(feature = "crypto_core"))]
pub type HostSigner = holocron_core::testseal::TestSigner;
/// The host service: the host suite over the `~/.holocron` directory and `/dev/urandom`.
pub type HostHolocron = holocron_core::service::Holocron<HostSealer, HostSigner, store::DirStore, daemon::OsEntropy>;
