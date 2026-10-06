// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling (B490 LOADERSTALL: reads the loader's stage record off the boot-info seam)
//!
//! LOADERSTALL (rmbp-ledger B490) — the kernel's half of the loader's own lines. The x86 UEFI loader
//! (`crates/bootloader/src/stages.rs`) hands over `unaos_boot_info::LoaderStages`: raw TSC at the end of
//! each loader stage, the last status it read, the UART it drove, the volumes the firmware published, the
//! kernel bytes it read, its read retries and how many stage budgets ran out. Converted ONCE, with the rate
//! `apic::calibrate` measured (the BOOTCLOCK rule: the loader's own rate is for its panel only), into the
//! boot's one line, printed right after `:: BOOTCLOCK:`:
//!
//! `:: LOADER: firmware->loader=<ms> loader->kernel=<ms> stages=<name:ms,...> volumes=<n> kernel_bytes=<n>
//! uart=<none|efi-serial|com1> retries=<n> timeouts=<n> ::`
//!
//! `loader->kernel` is the loader's whole phase: its entry stamp to the kernel's `entry` stamp. An older
//! `bootloader.efi` writes no record: the line says `record=absent`, never a number.
//!
//! `tests loader` (R80: behind the verb, nothing at boot) reads the kept record back:
//! `:: LOADER: stages=<n> slowest=<stage>:<ms> timeouts=<n> -> PASS|FAIL ::` — PASS when the record is
//! present, its stamps are monotonic up to the kernel's entry, all seven stages are there and no budget ran out.

use alloc::string::String;
use core::fmt::Write;
use unaos_boot_info::{LoaderStages, LOADER_STAGE_NAMES, LOADER_UART_NAMES};

/// The record, its loader-entry stamp and the kernel's entry stamp, kept for `tests loader`.
static KEPT: crate::sync::Mutex<Option<(LoaderStages, u64, u64)>> = crate::sync::Mutex::new(None);

fn dur(cy: u64, hz: u64) -> String {
    if hz >= 1000 { alloc::format!("{}ms", cy / (hz / 1000)) } else { alloc::format!("{}cy", cy) }
}

/// The stamps are one counter: loader entry <= every stage end (in order) <= the kernel's entry.
fn sane(r: &LoaderStages, entry: u64, kentry: u64) -> bool {
    if !r.present() || entry == 0 {
        return false;
    }
    let mut prev = entry;
    for i in 0..r.count as usize {
        if r.end_tsc[i] < prev || r.id[i] as usize >= LOADER_STAGE_NAMES.len() {
            return false;
        }
        prev = r.end_tsc[i];
    }
    prev <= kentry
}

/// `(name, cycles)` per stage.
fn stages(r: &LoaderStages, entry: u64) -> alloc::vec::Vec<(&'static str, u64)> {
    let mut v = alloc::vec::Vec::new();
    let mut prev = entry;
    for i in 0..r.count as usize {
        v.push((LOADER_STAGE_NAMES[r.id[i] as usize], r.end_tsc[i] - prev));
        prev = r.end_tsc[i];
    }
    v
}

/// The boot's one `:: LOADER:` line. `entry` is BOOTCLOCK's loader-entry stamp.
pub fn report(r: LoaderStages, entry: u64) {
    let hz = crate::bootpace::origin_hz();
    let kentry = crate::bootpace::origin_cycles();
    *KEPT.lock() = Some((r, entry, kentry));
    if !r.present() {
        serial_println!(":: LOADER: record=absent (a bootloader.efi older than LOADERSTALL) firmware->loader={} ::", if entry != 0 { dur(entry, hz) } else { String::from("absent") });
        return;
    }
    if !sane(&r, entry, kentry) {
        serial_println!(":: LOADER: record=insane count={} (stamps not monotonic up to the kernel's entry) ::", r.count);
        return;
    }
    let mut list = String::new();
    for (i, (n, cy)) in stages(&r, entry).iter().enumerate() {
        let _ = write!(list, "{}{}:{}", if i == 0 { "" } else { "," }, n, dur(*cy, hz));
    }
    serial_println!(
        ":: LOADER: firmware->loader={} loader->kernel={} stages={} volumes={} kernel_bytes={} uart={} retries={} timeouts={} ::",
        dur(entry, hz),
        dur(kentry - entry, hz),
        list,
        r.volumes,
        r.kernel_bytes,
        LOADER_UART_NAMES.get(r.uart as usize).copied().unwrap_or("?"),
        r.retries,
        r.timeouts
    );
}

/// Register `tests loader` once.
pub fn ensure_tests() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("loader", selftest);
    }
}

/// `tests loader`: the handed-over record, read back.
pub fn selftest() {
    let kept = *KEPT.lock();
    let Some((r, entry, kentry)) = kept else {
        serial_println!(":: LOADER: stages=0 slowest=- timeouts=0 record=unread -> FAIL ::");
        return;
    };
    if !r.present() {
        serial_println!(":: LOADER: stages=0 slowest=- timeouts=0 record=absent -> FAIL ::");
        return;
    }
    let ok_order = sane(&r, entry, kentry);
    let hz = crate::bootpace::origin_hz();
    let (mut slow_n, mut slow_cy) = ("-", 0u64);
    if ok_order {
        for (n, cy) in stages(&r, entry) {
            if cy >= slow_cy {
                slow_n = n;
                slow_cy = cy;
            }
        }
    }
    let pass = ok_order && r.count as usize == LOADER_STAGE_NAMES.len() && r.timeouts == 0;
    serial_println!(
        ":: LOADER: stages={} slowest={}:{} timeouts={}{} -> {} ::",
        r.count,
        slow_n,
        dur(slow_cy, hz),
        r.timeouts,
        if ok_order { "" } else { " order=insane" },
        if pass { "PASS" } else { "FAIL" }
    );
}
