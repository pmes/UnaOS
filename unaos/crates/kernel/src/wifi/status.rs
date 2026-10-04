// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! WIFI1 — the recorded verdict of the bring-up ladder, so the `wifi` verb and `tests wifi` report
//! what the LAST pass (boot, or a `wifi up`) actually achieved rather than re-running the device.
//!
//! CHARTER: Kernel — kernel-by-ruling. The driver (`bringup.rs`) WRITES these on its one pass; the
//! shell verb and the test fixture READ them. A single set of relaxed atomics, single-writer
//! (`bringup_once` runs once per boot from the main loop, one core), so `Relaxed` is correct — the
//! same ordering `wifi/mod.rs`'s own statics use, for the same reason.

use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};

/// `-> UPLOADED` was printed this boot: the PSM started and published a revision past the §S4 floor.
static UCODE_OK: AtomicBool = AtomicBool::new(false);
/// The revision the running microcode published (`0` until an upload handshake runs).
static UCODE_REV: AtomicU16 = AtomicU16::new(0);
/// The microcode's self-published build date word (for the `[wifi] ucode=ok … date=` witness).
static UCODE_DATE: AtomicU16 = AtomicU16::new(0);
/// The d11 core reached and its enable rule satisfied (arc 2 got past `d11 FOUND` with all MATCHes).
static D11_UP: AtomicBool = AtomicBool::new(false);
/// Initvals records applied on the last upload (`0` until the initvals rung runs).
static INITVALS_WROTE: AtomicU32 = AtomicU32::new(0);
/// Beacons the last scan decoded. `u32::MAX` means "no scan has run".
static SCAN_COUNT: AtomicU32 = AtomicU32::new(u32::MAX);

pub fn set_ucode(ok: bool, rev: u16, date: u16) {
    UCODE_OK.store(ok, Ordering::Relaxed);
    UCODE_REV.store(rev, Ordering::Relaxed);
    UCODE_DATE.store(date, Ordering::Relaxed);
}
pub fn set_d11_up(up: bool) {
    D11_UP.store(up, Ordering::Relaxed);
}
pub fn set_initvals(n: u32) {
    INITVALS_WROTE.store(n, Ordering::Relaxed);
}
pub fn set_scan(n: u32) {
    SCAN_COUNT.store(n, Ordering::Relaxed);
}

pub fn ucode_ok() -> bool {
    UCODE_OK.load(Ordering::Relaxed)
}
pub fn ucode_rev() -> u16 {
    UCODE_REV.load(Ordering::Relaxed)
}
pub fn ucode_date() -> u16 {
    UCODE_DATE.load(Ordering::Relaxed)
}
pub fn d11_up() -> bool {
    D11_UP.load(Ordering::Relaxed)
}
pub fn initvals_wrote() -> u32 {
    INITVALS_WROTE.load(Ordering::Relaxed)
}
/// `None` when no scan has run this boot, else the beacon count.
pub fn scan_count() -> Option<u32> {
    match SCAN_COUNT.load(Ordering::Relaxed) {
        u32::MAX => None,
        n => Some(n),
    }
}
