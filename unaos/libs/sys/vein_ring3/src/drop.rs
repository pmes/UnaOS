// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! DRAGDROP2 (rmbp-ledger B470) — the ring-3 drop protocol's client. The kernel pushed `INPUT_EV_DROP` (payload
//! `[15:8]` token, `[7:0]` count) to this program's input ring; [`get`] asks `BUS_VERB_DROP_GET` for that token
//! and copies the reply body (the paths, newline-joined; parse with `una_abi::drop_paths`) into `out`.
//! Synchronous, like `prefs::get`: one request, then block for its reply.

use crate::sys::sys;
use una_abi::{BUS_FRAME_MAX, BUS_HDR_LEN, BUS_KIND_REPLY, BUS_KIND_REQUEST, BUS_MAGIC, BUS_VERB_DROP_GET, BUS_VERSION, SYS_MRECV, SYS_MSEND};

static mut RX: [u8; BUS_FRAME_MAX] = [0; BUS_FRAME_MAX];
static mut CORR: u32 = 0x4452_0000;

/// The paths of drop `token` into `out`; `Ok(len)`. `Err(-ENOENT)` = taken already or never this program's.
pub fn get(token: u8, out: &mut [u8]) -> Result<usize, i64> {
    let rx = unsafe { &mut *core::ptr::addr_of_mut!(RX) };
    let corr = unsafe {
        CORR = CORR.wrapping_add(1);
        CORR
    };
    let mut tx = [0u8; BUS_HDR_LEN + 1];
    tx[0..4].copy_from_slice(&BUS_MAGIC);
    tx[4] = BUS_VERSION;
    tx[5] = BUS_KIND_REQUEST;
    tx[6] = BUS_VERB_DROP_GET;
    tx[8..12].copy_from_slice(&corr.to_le_bytes());
    tx[48..52].copy_from_slice(&1u32.to_le_bytes());
    tx[BUS_HDR_LEN] = token;
    let s = sys(SYS_MSEND, tx.as_ptr() as u64, tx.len() as u64, 0, 0);
    if s != 0 {
        return Err(s);
    }
    for _ in 0..8 {
        let n = sys(SYS_MRECV, rx.as_mut_ptr() as u64, rx.len() as u64, 0, 0);
        if n < 0 {
            return Err(n);
        }
        let n = n as usize;
        if n < BUS_HDR_LEN || rx[0..4] != BUS_MAGIC || rx[5] != BUS_KIND_REPLY || rx[8..12] != corr.to_le_bytes() {
            continue; // a stale frame: not ours
        }
        let status = i32::from_le_bytes([rx[12], rx[13], rx[14], rx[15]]) as i64;
        if status != 0 {
            return Err(status);
        }
        let blen = (u32::from_le_bytes([rx[48], rx[49], rx[50], rx[51]]) as usize).min(n - BUS_HDR_LEN);
        let m = blen.min(out.len());
        out[..m].copy_from_slice(&rx[BUS_HDR_LEN..BUS_HDR_LEN + m]);
        return Ok(m);
    }
    Err(una_abi::EIO)
}
