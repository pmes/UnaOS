// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! DRAGDROP2 (rmbp-ledger B470) — the ring-3 drop protocol's client. The kernel pushed `INPUT_EV_DROP` (payload
//! `[15:8]` token, `[7:0]` count) to this program's input ring; [`get`] asks `BUS_VERB_DROP_GET` for that token
//! and copies the reply body (the paths, newline-joined; parse with `una_abi::drop_paths`) into `out`.
//! Synchronous, like `prefs::get`: one request, then block for its reply.

use una_abi::BUS_VERB_DROP_GET;

/// The paths of drop `token` into `out`; `Ok(len)`. `Err(-ENOENT)` = taken already or never this program's.
/// DROPLOOPS (SR179): one ask through `crate::bus::ask` (`una_abi::bus_ask`): a frame met before the reply is kept.
pub fn get(token: u8, out: &mut [u8]) -> Result<usize, i64> {
    crate::bus::ask(BUS_VERB_DROP_GET, &[token], out)
}
