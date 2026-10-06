// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling (B485 HIDSTALL, R103: the pointer's halt, stall and dock release, counted)
//!
//! HIDSTALL (rmbp-ledger B485) — flight 26 read `STOP-NOTE … addr=5|6 class=xact-err-burn` at session open,
//! `[lag] stall … stage=hid hid_gap_ms=49 pump=desktop-app pump_ms=2xx` every 2 s, a 3–4 s key queue and a dock
//! press whose release never launched. The causes, as the wire names them (docs/dev/evidence/rmbp-1005/hidstall.md):
//!
//! * the halted endpoints are the BT HID PROXY (`05ac:820a`/`820b`), burned the moment BTHID's HCI Reset takes the
//!   radio — they carry no input; a halted INPUT endpoint is now cleared and re-armed (`[hid] recovered …`);
//! * the 49 ms gap is the Dock's 2 s Trash poll — a UnaFS query, and every UnaFS transaction is IRQ-masked, so the
//!   `hid-pump` task on the service core cannot run for its length; the Trash is now read on a change only;
//! * a pin press arms a launch-on-release; no release within [`DOCK_RELEASE_TIMEOUT_MS`] launches it.
//!
//! This file holds the arc's counters (atomics, every arch) and `tests hidstall` (R80: behind the verb, nothing at
//! boot): `:: HIDSTALL: halted_at_login=<n> recovered=<n> proxy_retired=<n> hid_stall_s=<n> key_queue_max_ms=<n>
//! dock_timeouts=<n> masked_max_ms=<n> -> PASS|FAIL ::` — PASS iff no input endpoint stayed halted, no desktop second
//! had a HID gap at or over [`HID_GAP_STALL_MS`], and no key sat in the queue over the INPUTSTALL bound.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

/// A pin press with no release (and no travel) for this long launches (DOCKRELEASE).
pub const DOCK_RELEASE_TIMEOUT_MS: u64 = 400;
/// A desktop second whose longest gap between two HID passes reached this is a HID stall second (one 60 Hz frame:
/// the pass cadence is one 1 ms tick, so a frame without a pass is a pointer the glass sees stutter).
pub const HID_GAP_STALL_MS: u64 = 16;
/// The key-queue bound the fixture reads (INPUTSTALL's `bound=`).
pub const KEY_QUEUE_BOUND_MS: u64 = 50; // = `video::lag::STALL_MS`

static HALTED: AtomicU32 = AtomicU32::new(0);
static RECOVERED: AtomicU32 = AtomicU32::new(0);
static PROXY_RETIRED: AtomicU32 = AtomicU32::new(0);
static HID_STALL_S: AtomicU32 = AtomicU32::new(0);
static KEYQ_MAX_MS: AtomicU64 = AtomicU64::new(0);
static DOCK_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static MASKED_MAX_MS: AtomicU64 = AtomicU64::new(0);
static SEC_MASKED_MS: AtomicU64 = AtomicU64::new(0);
/// Per interrupt-endpoint index: when it halted (ms), for the recovery's `after_ms=`.
static HALT_AT: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];

/// Is a halted interrupt endpoint the BT HID proxy (BTPROXY): burned by transaction errors, never a report, a boot
/// keyboard/mouse packet size (the internal keyboard is mps 10), on a controller that claimed a BT radio. Pure.
pub fn is_proxy(class: &str, reports: u32, mps: u16, radio: bool) -> bool {
    radio && class == "xact-err-burn" && reports == 0 && mps <= 8
}

/// May an `xact-err-burn` halt be cleared? Yes for an endpoint that has carried input (a burst of bus errors on a live
/// device, not a device saying no); never for the proxy (its EP0 would only time out under the HID lock). Pure.
pub fn xact_recoverable(class: &str, proxy: bool, reports: u32) -> bool {
    class == "xact-err-burn" && !proxy && reports > 0
}

/// The service walk saw endpoint `ep_i` halt.
pub fn note_halt(ep_i: usize) {
    if let Some(a) = HALT_AT.get(ep_i) {
        a.store(crate::arch::ms().max(1), Relaxed);
    }
}

/// An endpoint was retired: the proxy is counted apart (one line says what it is); any other is a halt that stayed.
pub fn note_retired(ctl: usize, addr: u8, ep: u8, kind: &str, proxy: bool, reports: u32) {
    if proxy {
        PROXY_RETIRED.fetch_add(1, Relaxed);
        serial_println!(
            "[hid] proxy-retired ctl={} addr={} ep=IN{} kind={} reports={} why=bt-hci-owns-radio (the BT HID proxy; input rides the internal keyboard and trackpad)",
            ctl, addr, ep, kind, reports
        );
    } else {
        HALTED.fetch_add(1, Relaxed);
        serial_println!("[hid] halted ctl={} addr={} ep=IN{} kind={} reports={} -> retired (HIDSTALL: not recovered)", ctl, addr, ep, kind, reports);
    }
}

/// A halted endpoint was cleared (ClearFeature ENDPOINT_HALT) and re-armed.
pub fn note_recovered(ctl: usize, addr: u8, ep: u8, ep_i: usize) {
    RECOVERED.fetch_add(1, Relaxed);
    let t = HALT_AT.get(ep_i).map(|a| a.swap(0, Relaxed)).unwrap_or(0);
    let after = if t == 0 { 0 } else { crate::arch::ms().saturating_sub(t) };
    serial_println!("[hid] recovered ctl={} addr={} ep=IN{} after_ms={}", ctl, addr, ep, after);
}

/// `lag`'s second roll at the desktop: the second's longest HID gap and the minute's key-queue max so far.
pub fn note_sec(hid_gap_ms: u64, keyq_ms: u64) {
    if hid_gap_ms >= HID_GAP_STALL_MS {
        HID_STALL_S.fetch_add(1, Relaxed);
    }
    KEYQ_MAX_MS.fetch_max(keyq_ms, Relaxed);
}

/// One UnaFS transaction attempt's IRQ-masked span (ms).
pub fn note_masked(ms: u64) {
    SEC_MASKED_MS.fetch_max(ms, Relaxed);
    MASKED_MAX_MS.fetch_max(ms, Relaxed);
}

/// The second's longest masked UnaFS span, taken (reset) at `lag`'s roll.
pub fn take_sec_masked() -> u64 {
    SEC_MASKED_MS.swap(0, Relaxed)
}

/// A pin press timed out into its launch.
pub fn note_dock_timeout() {
    DOCK_TIMEOUTS.fetch_add(1, Relaxed);
}

/// The verdict. Pure.
pub fn verdict(halted: u32, hid_stall_s: u32, keyq_ms: u64) -> bool {
    halted == 0 && hid_stall_s == 0 && keyq_ms <= KEY_QUEUE_BOUND_MS
}

/// Register `tests hidstall` once.
pub fn ensure() {
    use core::sync::atomic::AtomicBool;
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, core::sync::atomic::Ordering::AcqRel) {
        crate::tests::register("hidstall", selftest);
    }
}

fn selftest() {
    let h = HALTED.load(Relaxed);
    let s = HID_STALL_S.load(Relaxed);
    let k = KEYQ_MAX_MS.load(Relaxed);
    serial_println!(
        ":: HIDSTALL: halted_at_login={} recovered={} proxy_retired={} hid_stall_s={} key_queue_max_ms={} dock_timeouts={} masked_max_ms={} -> {} :: hid_gap_bound_ms={} key_bound_ms={} release_timeout_ms={}",
        h, RECOVERED.load(Relaxed), PROXY_RETIRED.load(Relaxed), s, k, DOCK_TIMEOUTS.load(Relaxed), MASKED_MAX_MS.load(Relaxed),
        if verdict(h, s, k) { "PASS" } else { "FAIL" }, HID_GAP_STALL_MS, KEY_QUEUE_BOUND_MS, DOCK_RELEASE_TIMEOUT_MS
    );
}
