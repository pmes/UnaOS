// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//! SELFBUILD5 (B357, ROADMAP §1c SH-5) — `tests selfbuild5`: UnaOS LINKS a Rust program.
//!
//! 1. `SYSKAT5.LNX` (crates/user-linux-hello/c/syskat5.c): mremap grow in place / shrink, move (the leaves re-homed, the
//!    old range gone), MREMAP_FIXED, MREMAP_DONTUNMAP, the error answers; the alternate signal stack at delivery (the frame
//!    on it, SS_ONSTACK inside the handler, EPERM to change it there, uc_stack, a handler without SA_ONSTACK on the normal
//!    stack, SS_DISABLE); getcpu, madvise(MADV_HUGEPAGE), sched_yield.
//! 2. `LLD.LNX -flavor gnu @/lib/rust/link.rsp -o <home>rust2.lnx` — a static `ld.lld` (LLVM 22.1.8, the toolchain's
//!    LLVM release, built by arroyo) links RUST.LNX's own object files against the staged musl crt, libc.a, libunwind.a and
//!    the std rlibs under /lib/rust (the response file is the host rustc's own ld.lld line with UnaOS paths).
//! 3. `<home>rust2.lnx /apps/HELLO.C` — the relinked program runs: `rust ok threads=4 counter=400000 …`, exit 0.
//!
//! Wire: `:: SELFBUILD5: mremap=ok altstack=ok lld=ok link_ms=<n> link_peak_mib=<n> relink_runs=1 -> PASS ::`, after
//! `:: LINUXABI-KAT5: … ::`, `[selfbuild5] link: …` and `[selfbuild5] kernel: …`. Absent LLD.LNX / link.rsp = lld=skip
//! (the verdict is then SKIP when the KAT passed).

use super::{remap, run_path, vm, Report};
use alloc::string::String;
use core::sync::atomic::Ordering;

pub const KAT5: &str = "/apps/SYSKAT5.LNX";
pub const LLD: &str = "/apps/LLD.LNX";
pub const RSP: &str = "/lib/rust/link.rsp";

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

/// `(mremap, altstack)`.
fn kat5() -> (String, String) {
    if exists(KAT5).is_none() {
        serial_println!(":: LINUXABI-KAT5: -> SKIP (fixture not staged) ::");
        return (String::from("skip"), String::from("skip"));
    }
    let (r, cap) = run_cap(KAT5, &[KAT5], 30_000);
    let rep = match r {
        Ok(rep) => rep,
        Err(e) => {
            serial_println!(":: LINUXABI-KAT5: -> FAIL (load: {}) ::", e);
            return (String::from("fail(load)"), String::from("fail(load)"));
        }
    };
    serial_println!("{}", rep.witness(KAT5));
    let Some(line) = cap.lines().find(|l| l.starts_with("syskat5 ")) else {
        serial_println!(":: LINUXABI-KAT5: exit={} blocked={} -> FAIL (no verdict line) ::", rep.exit, rep.blocked);
        let f = alloc::format!("fail(exit={})", rep.exit);
        return (f.clone(), f);
    };
    let g = |k| String::from(field(line, k).unwrap_or("?"));
    let ok = rep.pass && rep.exit == "0" && line.starts_with("syskat5 ok ");
    serial_println!(
        ":: LINUXABI-KAT5: grow={} move={} fixed={} dontunmap={} errors={} altstack={} small={} fail={} exit={} -> {} ::",
        g("grow"), g("move"), g("fixed"), g("dontunmap"), g("errors"), g("altstack"), g("small"), g("fail"), rep.exit,
        if ok { "PASS" } else { "FAIL" }
    );
    let mremap = ["grow", "move", "fixed", "dontunmap", "errors"].iter().find(|k| g(k) != "ok").map_or(String::from("ok"), |k| alloc::format!("fail({}={})", k, g(k)));
    let alt = if g("altstack") == "ok" { String::from("ok") } else { alloc::format!("fail({})", g("altstack")) };
    if !ok && mremap == "ok" && alt == "ok" {
        let f = alloc::format!("fail(exit={})", rep.exit); // `small` or the exit failed
        return (f.clone(), f);
    }
    (mremap, alt)
}

struct Link {
    lld: String,
    ms: u64,
    peak_mib: u64,
    runs: u32,
}

fn link() -> Link {
    let mut l = Link { lld: String::from("skip"), ms: 0, peak_mib: 0, runs: 0 };
    if exists(LLD).is_none() || exists(RSP).is_none() {
        serial_println!("[selfbuild5] {} or {} not staged -> lld=skip", LLD, RSP);
        return l;
    }
    let out = alloc::format!("{}rust2.lnx", super::sys::home_prefix());
    let _ = crate::shell::vfs_mount_table().unlink(&crate::shell::vfs_path(&out), crate::fs::vfs::KERNEL_PRINCIPAL);
    let rsp = alloc::format!("@{}", RSP);
    serial_println!("[selfbuild5] linux {} -flavor gnu {} -o {}", LLD, rsp, out);
    vm::PEAK_RESIDENT.store(0, Ordering::Relaxed);
    let (r, cap) = run_cap(LLD, &[LLD, "-flavor", "gnu", rsp.as_str(), "-o", out.as_str()], 600_000);
    let peak = vm::PEAK_RESIDENT.load(Ordering::Relaxed);
    l.peak_mib = (peak * 4096 + (1 << 20) - 1) >> 20;
    for line in cap.lines().take(12) {
        serial_println!("[selfbuild5] lld: {}", line);
    }
    let rep = match r {
        Ok(rep) => rep,
        Err(e) => {
            serial_println!("[selfbuild5] {}: {}", LLD, e);
            l.lld = String::from("fail(load)");
            return l;
        }
    };
    serial_println!("{}", rep.witness(LLD));
    l.ms = rep.ms;
    let size = exists(&out).unwrap_or(0);
    serial_println!("[selfbuild5] link: out={} out_bytes={} link_ms={} peak_resident_pages={} exit={}", out, size, rep.ms, peak, rep.exit);
    if !(rep.pass && rep.exit == "0" && size > 0) {
        l.lld = alloc::format!("fail(exit={} out_bytes={})", rep.exit, size);
        return l;
    }
    l.lld = String::from("ok");
    // The relinked program runs.
    serial_println!("[selfbuild5] linux {} {}", out, super::selfbuild::SRC);
    let (r, cap) = run_cap(&out, &[out.as_str(), super::selfbuild::SRC], 30_000);
    for line in cap.lines().take(4) {
        serial_println!("[selfbuild5] rust2: {}", line);
    }
    match r {
        Ok(rep) => {
            serial_println!("{}", rep.witness(&out));
            if rep.pass && rep.exit == "0" && cap.lines().any(|x| x.starts_with("rust ok threads=4 counter=400000 ")) {
                l.runs = 1;
            }
        }
        Err(e) => serial_println!("[selfbuild5] {}: {}", out, e),
    }
    l
}

/// `tests selfbuild5`.
pub fn selftest() {
    remap::reset();
    let (mremap, alt) = kat5();
    let l = link();
    serial_println!(
        "[selfbuild5] kernel: mremap_calls={} mremap_inplace={} mremap_moved={} mremap_pages={} altstack_deliveries={} faults_file={} faults_anon={} peak_resident={}",
        remap::MREMAP_CALLS.load(Ordering::Relaxed),
        remap::MREMAP_INPLACE.load(Ordering::Relaxed),
        remap::MREMAP_MOVED.load(Ordering::Relaxed),
        remap::MREMAP_PAGES.load(Ordering::Relaxed),
        remap::ALTSTACK_DELIVERIES.load(Ordering::Relaxed),
        vm::FAULTS_FILE.load(Ordering::Relaxed),
        vm::FAULTS_ANON.load(Ordering::Relaxed),
        vm::PEAK_RESIDENT.load(Ordering::Relaxed)
    );
    let any_fail = mremap.starts_with("fail") || alt.starts_with("fail") || l.lld.starts_with("fail") || (l.lld == "ok" && l.runs == 0);
    let verdict = if any_fail {
        "FAIL"
    } else if mremap == "ok" && alt == "ok" && l.lld == "ok" && l.runs == 1 {
        "PASS"
    } else {
        "SKIP"
    };
    serial_println!(
        ":: SELFBUILD5: mremap={} altstack={} lld={} link_ms={} link_peak_mib={} relink_runs={} -> {} ::",
        mremap, alt, l.lld, l.ms, l.peak_mib, l.runs, verdict
    );
}
