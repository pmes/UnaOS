// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//! SELFBUILD3 (B353, ROADMAP §1c SH-5) — `tests selfbuild3`: lazy memory and a libc on the volume.
//!
//! 1. `SYSKAT3.LNX /apps/HELLO.C <size> <home>kat3.tmp` (crates/user-linux-hello/c/syskat3.c, freestanding): mmaps HELLO.C
//!    MAP_PRIVATE and reads its LAST byte through the mapping (== pread), writes a private RW file mapping without touching the
//!    file, writes two MAP_SHARED pages of a scratch file and reads them back with pread after msync and after munmap, mmaps
//!    256 MiB anonymous, touches one page per MiB and counts the resident pages with mincore (256, not 65536), PROT_NONE
//!    reservations made RW in part, mprotect round trips keeping contents, MADV_DONTNEED re-zeroing, brk past the old 64 MiB,
//!    and 160 MiB with EVERY page touched (40960 resident: past the 96 MiB heap share, into the user frame pool).
//! 2. `TCC.LNX -static -o <home>hellop.lnx /apps/PRINTF.C` against musl staged under `/apps/LIB` (crt1.o crti.o crtn.o libc.a,
//!    headers in /apps/LIB/include, tcc's own headers + libtcc1.a in /apps/LIB/tcc — the paths TCC.LNX is configured with), and
//!    the output prints `hello printf from tcc+musl on unaos 42`.
//!
//! Witness: `:: SELFBUILD3: mmap=ok shared=ok anon_mib=256 resident_pages=<n> libc=musl tcc_libc=ok -> PASS ::`, plus
//! `:: LINUXABI-KAT3: … ::` and a `[selfbuild3] kernel: …` line (peak resident pages, faults by kind, write-back pages, the pool).
//! Absent fixtures = SKIP (staging is arroyo's `build_selfbuild_x86` + the builder; musl needs egress at build time).

use super::{run_path, vm, Report};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

pub const KAT3: &str = "/apps/SYSKAT3.LNX";
pub const PRINTF_C: &str = "/apps/PRINTF.C";
pub const LIBC_A: &str = "/apps/LIB/libc.a";
pub const PRINTF_LINE: &str = "hello printf from tcc+musl on unaos 42";

fn exists(path: &str) -> Option<u64> {
    crate::shell::vfs_mount_table().stat(&crate::shell::vfs_path(path)).ok().map(|s| s.size)
}

fn run_cap(path: &str, argv: &[&str], ms: u64) -> (Result<Report, String>, String) {
    let mut cap = String::new();
    let r = run_path(path, argv, ms, false, &mut |l| {
        cap.push_str(l);
        cap.push('\n');
    });
    (r, cap)
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace().find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

fn kernel_line() {
    serial_println!(
        "[selfbuild3] kernel: peak_resident={} faults_anon={} faults_file={} writeback_pages={} pool_peak={} pool_pages_used={} limit_pages={} refusals={}",
        vm::PEAK_RESIDENT.load(Ordering::Relaxed),
        vm::FAULTS_ANON.load(Ordering::Relaxed),
        vm::FAULTS_FILE.load(Ordering::Relaxed),
        vm::WRITEBACK_PAGES.load(Ordering::Relaxed),
        vm::POOL_PEAK.load(Ordering::Relaxed),
        vm::POOL_PAGES_USED.load(Ordering::Relaxed),
        vm::limit_pages(),
        vm::REFUSALS.load(Ordering::Relaxed)
    );
}

/// The KAT leg: `(mmap, shared, anon_mib, resident_pages, all_ok)`; `mmap == "skip"` when the fixture is absent.
fn kat3() -> (String, String, String, String, bool) {
    let skip = || (String::from("skip"), String::from("skip"), String::from("0"), String::from("0"), false);
    let Some(src_size) = exists(super::selfbuild::SRC) else {
        serial_println!(":: LINUXABI-KAT3: -> SKIP (HELLO.C not staged) ::");
        return skip();
    };
    let size = alloc::format!("{}", src_size);
    let scratch = alloc::format!("{}kat3.tmp", super::sys::home_prefix());
    let argv = [KAT3, super::selfbuild::SRC, size.as_str(), scratch.as_str()];
    for c in [&vm::PEAK_RESIDENT, &vm::FAULTS_ANON, &vm::FAULTS_FILE, &vm::WRITEBACK_PAGES, &vm::REFUSALS, &vm::POOL_PEAK] {
        c.store(0, Ordering::Relaxed);
    }
    let (r, cap) = run_cap(KAT3, &argv, 30_000);
    let rep = match r {
        Err(e) if e.contains("-ENOENT") => {
            serial_println!(":: LINUXABI-KAT3: -> SKIP (fixture not staged) ::");
            return skip();
        }
        Err(e) => {
            serial_println!(":: LINUXABI-KAT3: -> FAIL (load: {}) ::", e);
            let f = String::from("fail(load)");
            return (f.clone(), f, String::from("0"), String::from("0"), false);
        }
        Ok(rep) => rep,
    };
    serial_println!("{}", rep.witness(KAT3));
    kernel_line();
    for l in cap.lines().filter(|l| l.starts_with("syskat3")).take(4) {
        serial_println!("[selfbuild3] kat3: {}", l);
    }
    let Some(line) = cap.lines().find(|l| l.starts_with("syskat3 ")) else {
        serial_println!(":: LINUXABI-KAT3: exit={} blocked={} -> FAIL (no verdict line) ::", rep.exit, rep.blocked);
        let f = alloc::format!("fail(exit={})", rep.exit);
        return (f.clone(), f, String::from("0"), String::from("0"), false);
    };
    let g = |k| String::from(field(line, k).unwrap_or("?"));
    let clean = rep.pass && rep.exit == "0" && line.starts_with("syskat3 ok ");
    serial_println!(
        ":: LINUXABI-KAT3: mmap={} shared={} anon_mib={} resident_pages={} prot={} brk={} madv={} big_mib={} fail={} exit={} -> {} ::",
        g("mmap"), g("shared"), g("anon_mib"), g("resident_pages"), g("prot"), g("brk"), g("madv"), g("big_mib"), g("fail"), rep.exit,
        if clean { "PASS" } else { "FAIL" }
    );
    (g("mmap"), g("shared"), g("anon_mib"), g("resident_pages"), clean)
}

/// The libc leg: `(libc, tcc_libc)`.
fn tcc_libc() -> (String, String) {
    if exists(LIBC_A).is_none() {
        return (String::from("none"), String::from("skip"));
    }
    let libc = String::from("musl");
    if exists(super::selfbuild::TCC).is_none() || exists(PRINTF_C).is_none() {
        return (libc, String::from("skip"));
    }
    let out = alloc::format!("{}hellop.lnx", super::sys::home_prefix());
    let _ = crate::shell::vfs_mount_table().unlink(&crate::shell::vfs_path(&out), crate::fs::vfs::KERNEL_PRINCIPAL);
    let argv: Vec<&str> = alloc::vec!["tcc", "-static", "-o", out.as_str(), PRINTF_C];
    serial_println!("[selfbuild3] linux {} -static -o {} {}", super::selfbuild::TCC, out, PRINTF_C);
    let (r, cap) = run_cap(super::selfbuild::TCC, &argv, 60_000);
    for l in cap.lines().take(8) {
        serial_println!("[selfbuild3] tcc: {}", l);
    }
    match &r {
        Ok(rep) => {
            serial_println!("{}", rep.witness(super::selfbuild::TCC));
            let made = exists(&out).unwrap_or(0);
            if !(rep.pass && rep.exit == "0" && made > 0) {
                return (libc, alloc::format!("fail(tcc exit={} out_bytes={})", rep.exit, made));
            }
            serial_println!("[selfbuild3] output {} bytes={}", out, made);
        }
        Err(e) => {
            serial_println!("[selfbuild3] TCC.LNX: {}", e);
            return (libc, String::from("fail(tcc load)"));
        }
    }
    let (h, hcap) = run_cap(&out, &[out.as_str()], 5_000);
    let v = match h {
        Ok(rep) => {
            serial_println!("{}", rep.witness(&out));
            for l in hcap.lines().take(2) {
                serial_println!("[selfbuild3] hellop: {}", l);
            }
            if rep.pass && rep.exit == "0" && hcap.lines().any(|l| l == PRINTF_LINE) {
                String::from("ok")
            } else {
                alloc::format!("fail(run exit={})", rep.exit)
            }
        }
        Err(e) => {
            serial_println!("[selfbuild3] {}: {}", out, e);
            String::from("fail(run load)")
        }
    };
    (libc, v)
}

/// `tests selfbuild3`.
pub fn selftest() {
    let (mmap, shared, anon, resident, kat_ok) = kat3();
    let (libc, tcc) = tcc_libc();
    let verdict = if (mmap != "skip" && !kat_ok) || tcc.starts_with("fail") {
        "FAIL"
    } else if kat_ok && tcc == "ok" {
        "PASS"
    } else {
        "SKIP"
    };
    serial_println!(
        ":: SELFBUILD3: mmap={} shared={} anon_mib={} resident_pages={} libc={} tcc_libc={} -> {} ::",
        mmap, shared, anon, resident, libc, tcc, verdict
    );
}
