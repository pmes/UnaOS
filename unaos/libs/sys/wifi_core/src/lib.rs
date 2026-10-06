// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `wifi_core` — the BCM4331 driver's shared core (WIFI1, rmbp-ledger B338).
//!
//! CHARTER: Kernel — shared-core. The kernel's `src/wifi/` driver (`d11.rs`, `verb.rs`) and the host
//! tests link the SAME codecs, so the bytes the radio is fed and the frames it hands back are decoded
//! by one implementation, tested on the host (`cargo test -p wifi_core`).
//!
//! * [`fw`] — the firmware container: one 8-byte header, the microcode word-stream and the initvals
//!   record-stream. Pinned on OUR metal (`bcm4331.md` §S4, "W3 ANSWERED ON METAL"); the kernel's
//!   `firmware.rs` classifier is the older twin and agrees with it rule for rule.
//! * [`ieee80211`] — the management and data frames a station needs to scan and join an OPEN network:
//!   beacon / probe-response parse, open-system authentication, association request/response, and the
//!   data-frame <-> Ethernet (LLC/SNAP) conversion a smoltcp device needs.
//! * [`sprom`] — WIFI6 (B439): the SPROM shadow's identity fields (rung S2r). Its offsets are the
//!   tree's own transcription in `bcm4331.md` §S2r, marked `[ONE-SOURCE]`, and corroborated on metal
//!   by the PCI subsystem id; a read-path layout only.
//!
//! SOURCES: IEEE Std 802.11 frame formats (`[PUBLIC]`) and the tree's metal pins. **No Linux driver
//! source was read for this crate**, and nothing here is Broadcom-specific beyond the container the
//! user's own extracted files carry. UnaOS ships no firmware: this crate parses bytes the user placed
//! on the volume (CLEAN_ROOM_POLICY §4).
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod fw;
pub mod ieee80211;
pub mod sprom;
