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



pub mod principal;
pub mod store;
pub mod unafs_store;

pub use holocron_core;
