//! CHARTER: Principia — fulfiller
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// PREFCHANGED (rmbp-ledger B558) — `tests prefchanged`: ring-3 delivery of PrefChanged (verb 19), driven through the
// PRODUCTION paths with a scratch mailbox row (the BUSX86-STAMP / BANDY3 fixtures' row 6 on both arches):
//
//   1. subscribe: a verb-19 REQUEST (body `system.display.idle`) through the arch's own MSEND body
//      (`busx_msend_for` / `sys_msend_for`), its reply popped from the row's mailbox (status 0), one prefix held;
//   2. write: `system.display.idle_min` moved through the bus client (`pref_set`) — `prefs::changed` →
//      `on_changed` → `r3_deliver` → the arch's `prefs_changed_push`;
//   3. MRECV: the row's mailbox pop (the dequeue `SYS_MRECV` runs) returns exactly ONE frame inside one bounded pass:
//      kind REPLY, verb 19, corr 0, status 0, the kernel reply principal, body `system.display.idle_min` NUL …;
//   4. teardown: `unsubscribe_row` (the line both arches' `clear_handle_row` runs) leaves no prefix, and the
//      restoring write delivers NOTHING to the row.
//
// A live tenant's subscription on the scratch row is taken out first and put back last; the operator's value is
// restored through the same bus client.

use super::*;
use crate::bus::BUS_KIND_REQUEST;

/// The scratch mailbox row (x86 slot / aarch64 ASID) — the bus fixtures' caller row.
const FX_ROW: usize = 6;
/// The fixture's subscription prefix and the key it writes.
const FX_PREFIX: &str = "system.display.idle";
const FX_ADDR: &[u8] = b"system.display.idle_min";
/// One bounded pass: a relayed set may land up to `RELAY_WAIT_MS` later, so the pass waits four of them.
const FX_PASS_MS: u64 = RELAY_WAIT_MS * 4;

/// Pop the row's mailbox for one bounded pass: `(PrefChanged frames, all well-formed for FX_ADDR, reply status
/// to `corr` if one was met)`.
fn pass(row: usize, corr: u32) -> (u32, bool, Option<i32>) {
    let (mut n, mut ok, mut st) = (0u32, true, None);
    let t0 = crate::arch::ms();
    let mut spins = 0u32;
    loop {
        while let Some(fr) = crate::arch::syscall::prefchanged_fx_pop(row) {
            let Ok(h) = crate::bus::frame_parse(&fr) else { continue };
            if h.kind == BUS_KIND_REPLY && h.verb == una_abi::BUS_VERB_PREF_CHANGED && h.corr == 0 {
                let body = &fr[BUS_HDR_LEN..];
                n += 1;
                ok &= h.status == 0 && h.principal[0] == PRIN_KERNEL_REPLY && body.starts_with(FX_ADDR) && body.get(FX_ADDR.len()) == Some(&0);
            } else if h.kind == BUS_KIND_REPLY && h.corr == corr && corr != 0 {
                st = Some(h.status);
            }
        }
        spins += 1;
        if (corr != 0 && st.is_some()) || crate::arch::ms().saturating_sub(t0) >= FX_PASS_MS || spins >= RELAY_SPIN_MAX {
            return (n, ok, st);
        }
        core::hint::spin_loop();
    }
}

pub fn selftest() {
    use crate::prefs::key;
    let row = FX_ROW;
    let saved = take_row(row);
    let _ = pass(row, 0); // whatever was queued on the scratch row is not this fixture's

    // 1. subscribe through the production MSEND body.
    let corr = 0x5042_0558u32;
    let mut f = alloc::vec![0u8; BUS_HDR_LEN + FX_PREFIX.len()];
    let n = crate::bus::build_request(una_abi::BUS_VERB_PREF_CHANGED, corr, FX_PREFIX.as_bytes(), &mut f);
    let sent = match crate::bus::frame_parse(&f[..n]) {
        Ok(h) if h.kind == BUS_KIND_REQUEST => crate::arch::syscall::prefchanged_fx_send(row, &f[..n]),
        _ => EINVAL,
    };
    let (_, _, ack) = pass(row, corr);
    let held = row_prefixes(row);
    let sub_ok = sent == 0 && ack == Some(0) && held == 1;

    // 2. write a key under the prefix; 3. one pass of the mailbox returns its frame.
    let live0 = crate::video::dimidle::idle_min() as i64;
    let orig = sys_int(key::IDLE_MIN, 0, 1440).unwrap_or(live0);
    let want: i64 = if orig == 7 { 9 } else { 7 };
    let d0 = R3_DELIVERED.load(Ordering::Relaxed);
    let set_ok = pref_set(crate::prefs::NS, key::IDLE_MIN, PrefValue::Int(want)).is_ok();
    let (frames, body_ok, _) = pass(row, 0);
    let delivered = R3_DELIVERED.load(Ordering::Relaxed).wrapping_sub(d0);

    // 4. teardown unsubscribes: no prefix left, and the restoring write reaches nobody on the row.
    unsubscribe_row(row);
    let gone = row_prefixes(row) == 0;
    let _ = pref_set(crate::prefs::NS, key::IDLE_MIN, PrefValue::Int(orig));
    let (after, _, _) = pass(row, 0);

    if let Some(sub) = saved {
        let cell = &SUBS[row];
        locked(|| *cell.lock() = Some(sub));
        R3_SUBS.fetch_add(1, Ordering::AcqRel);
    }
    let pass_ok = sub_ok && set_ok && frames == 1 && body_ok && gone && after == 0;
    serial_println!(
        "[prefchanged] row={} sent={} ack={:?} held={} want={} orig={} set={} delivered={} r3_dropped={} via={}",
        row, sent, ack, held, want, orig, set_ok as u8, delivered, R3_DROPPED.load(Ordering::Relaxed), last_via()
    );
    serial_println!(
        ":: PREFCHANGED: subscribe={} frames={} body={} teardown={} after={} -> {} ::",
        if sub_ok { "ok" } else { "FAIL" }, frames, if body_ok && frames > 0 { "ok" } else { "bad" }, if gone { "unsubscribed" } else { "HELD" }, after,
        if pass_ok { "PASS" } else { "FAIL" }
    );
}
