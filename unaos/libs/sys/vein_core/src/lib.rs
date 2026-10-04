// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vein — shared-core
//!
//! VEINCORE (rmbp-ledger B304; ROADMAP §3b "port the bus, not the binary"). Vein's shared core: what the
//! host handler (`handlers/vein`), the ring-3 fulfiller (`crates/user-vein` → `APPS/VEIN.BIN`) and the
//! kernel's relay plumbing (`vein rsp`, `tests vein`) all link, so the rings cannot drift.
//!
//! * [`wire`] — the chat verbs' bus bodies (ChatSend, ChatReply, ChatCancel, ChatStatus), base64 and the
//!   `[vein-relay]` line format. No allocation. The KATs are the spec of record; the human-readable
//!   layouts are at the top of `docs/dev/evidence/rmbp-1004/VEINCORE.md`.
//! * [`provider`] — the `Provider` trait and the two metal providers of v1: `Echo` and `Relay`; the
//!   ChatReply chunker. No allocation.
//! * [`model`] / [`context`] (feature `alloc`) — the conversation model (`ChatMessage`, `Conversation`,
//!   `ChatRequest`, `ChatResponse`, `StopReason`: the SAME names as VEINPROV's `gneiss_pal::api`, reconciled
//!   at the fold) and the pure half of the context assembler.
#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod provider;
pub mod wire;

#[cfg(feature = "alloc")]
pub mod context;
#[cfg(feature = "alloc")]
pub mod model;
