// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//! SELFBUILD4 (B356, ROADMAP §1c SH-5) — `tests selfbuild4`: the first Rust program on UnaOS, a real fork + pipe, and a
//! compile-and-run in one shell line.
//!
//! 1. `SYSKAT4.LNX <home>kat4.tmp` (crates/user-linux-hello/c/syskat4.c): copy-on-write fork (user stores, a kernel store
//!    from `read()`, an mprotect round trip — the parent keeps its bytes), MAP_SHARED across fork, a re-executed image whose
//!    .data pages and mid-page .bss read right (lazy execve), SIGBUS for a file page past EOF.
//! 2. `RUST.LNX /apps/HELLO.C` (crates/user-linux-rust, `x86_64-unknown-linux-musl`, static, non-PIE): std::thread x4 on a
//!    Mutex<u64> to 400000, a HashMap, std::fs::read_to_string, env::args, println!, Instant, a caught panic.
//! 3. `PROBE.LNX sh -c 'sh -c "echo a; echo b" | cat'` — busybox ash forks both sides of a pipe; `a` then `b` come out.
//! 4. `PROBE.LNX sh -c "/apps/TCC.LNX -static -o <home>hello4.lnx /apps/PRINTF.C && <home>hello4.lnx"` — ash forks, execs tcc
//!    (lazily), waits, execs the output: `hello printf from tcc+musl on unaos 42`. Needs `/apps/LIB` (SELFBUILD3); else skip.
//!
//! Wire: `:: SELFBUILD4: rust=ok threads=4 counter=400000 hashmap=ok fs=ok panic_caught=1 fork=ok pipe=ok exec_lazy=1
//! cow_pages=<n> -> PASS ::`, after `:: LINUXABI-KAT4: … ::`, `[selfbuild4] exec: …`, `[selfbuild4] tcc_run=…` and
//! `[selfbuild4] kernel: …`. PASS also needs the KAT and (when `/apps/LIB` is staged) the tcc line. Absent RUST.LNX = SKIP.

use super::{cow, exec, run_path, vm, Report};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

pub const RUST: &str = "/apps/RUST.LNX";
pub const KAT4: &str = "/apps/SYSKAT4.LNX";
pub const PIPE_SH: &str = "sh -c \"echo a; echo b\" | cat";

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

fn kat4() -> String {
    if exists(KAT4).is_none() {
        serial_println!(":: LINUXABI-KAT4: -> SKIP (fixture not staged) ::");
        return String::from("skip");
    }
    let scratch = alloc::format!("{}kat4.tmp", super::sys::home_prefix());
    let (r, cap) = run_cap(KAT4, &[KAT4, scratch.as_str()], 30_000);
    let rep = match r {
        Ok(rep) => rep,
        Err(e) => {
            serial_println!(":: LINUXABI-KAT4: -> FAIL (load: {}) ::", e);
            return String::from("fail(load)");
        }
    };
    serial_println!("{}", rep.witness(KAT4));
    let Some(line) = cap.lines().find(|l| l.starts_with("syskat4 ")) else {
        serial_println!(":: LINUXABI-KAT4: exit={} blocked={} -> FAIL (no verdict line) ::", rep.exit, rep.blocked);
        return alloc::format!("fail(exit={})", rep.exit);
    };
    let g = |k| String::from(field(line, k).unwrap_or("?"));
    let ok = rep.pass && rep.exit == "0" && line.starts_with("syskat4 ok ");
    serial_println!(
        ":: LINUXABI-KAT4: cow={} cow_kernel={} cow_prot={} shared_fork={} exec_lazy={} sigbus={} fail={} exit={} -> {} ::",
        g("cow"), g("cow_kernel"), g("cow_prot"), g("shared_fork"), g("exec_lazy"), g("sigbus"), g("fail"), rep.exit,
        if ok { "PASS" } else { "FAIL" }
    );
    if ok { String::from("ok") } else { alloc::format!("fail({})", g("fail")) }
}

struct Rust {
    rust: String,
    threads: String,
    counter: String,
    hashmap: String,
    fs: String,
    panic: String,
    lazy: bool,
}

fn rust() -> Rust {
    let mut r = Rust {
        rust: String::from("skip"),
        threads: String::from("0"),
        counter: String::from("0"),
        hashmap: String::from("skip"),
        fs: String::from("skip"),
        panic: String::from("0"),
        lazy: false,
    };
    if exists(RUST).is_none() {
        serial_println!("[selfbuild4] {} not staged -> rust=skip", RUST);
        return r;
    }
    serial_println!("[selfbuild4] linux {} {}", RUST, super::selfbuild::SRC);
    let (res, cap) = run_cap(RUST, &[RUST, super::selfbuild::SRC], 30_000);
    // The root image's load is the LAST one now (the probe never execs): its lazy numbers.
    let (fb, rb, lp, ep, nv) = (
        exec::LAST_FILE_BYTES.load(Ordering::Relaxed),
        exec::LAST_READ_BYTES.load(Ordering::Relaxed),
        exec::LAST_LAZY_PAGES.load(Ordering::Relaxed),
        exec::LAST_EAGER_PAGES.load(Ordering::Relaxed),
        exec::LAST_VMAS.load(Ordering::Relaxed),
    );
    serial_println!("[selfbuild4] exec: path={} file_bytes={} read_at_exec={} lazy_pages={} eager_pages={} vmas={}", RUST, fb, rb, lp, ep, nv);
    r.lazy = lp > 0 && rb < fb;
    for l in cap.lines().take(6) {
        serial_println!("[selfbuild4] rust: {}", l);
    }
    let rep = match res {
        Ok(rep) => rep,
        Err(e) => {
            serial_println!("[selfbuild4] {}: {}", RUST, e);
            r.rust = String::from("fail(load)");
            return r;
        }
    };
    serial_println!("{}", rep.witness(RUST));
    let Some(line) = cap.lines().find(|l| l.starts_with("rust ")) else {
        r.rust = alloc::format!("fail(exit={})", rep.exit);
        return r;
    };
    let g = |k| String::from(field(line, k).unwrap_or("?"));
    r.threads = g("threads");
    r.counter = g("counter");
    r.hashmap = g("hashmap");
    r.fs = g("fs");
    r.panic = g("panic_caught");
    r.rust = if rep.pass && rep.exit == "0" && line.starts_with("rust ok ") {
        String::from("ok")
    } else if let Some(w) = line.strip_prefix("rust fail ") {
        alloc::format!("fail({})", w.split_whitespace().next().unwrap_or("?"))
    } else {
        alloc::format!("fail(exit={})", rep.exit)
    };
    r
}

/// The pipeline: `(fork, pipe)`.
fn pipeline() -> (String, String) {
    if exists(super::selfbuild::PROBE).is_none() {
        return (String::from("skip"), String::from("skip"));
    }
    serial_println!("[selfbuild4] linux {} sh -c '{}'", super::selfbuild::PROBE, PIPE_SH);
    let (r, cap) = run_cap(super::selfbuild::PROBE, &["sh", "-c", PIPE_SH], 20_000);
    for l in cap.lines().take(4) {
        serial_println!("[selfbuild4] pipe: {}", l);
    }
    match r {
        Ok(rep) => {
            serial_println!("{}", rep.witness(super::selfbuild::PROBE));
            let lines: Vec<&str> = cap.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
            let fork = if rep.forks >= 2 { String::from("ok") } else { alloc::format!("fail(forks={})", rep.forks) };
            let pipe = if rep.pass && rep.exit == "0" && lines == ["a", "b"] {
                String::from("ok")
            } else {
                alloc::format!("fail(exit={} lines={})", rep.exit, lines.len())
            };
            (fork, pipe)
        }
        Err(e) => {
            serial_println!("[selfbuild4] {}: {}", super::selfbuild::PROBE, e);
            (String::from("fail(load)"), String::from("fail(load)"))
        }
    }
}

/// tcc compiles PRINTF.C against the staged musl and the shell runs the output, one command line.
fn tcc_run() -> String {
    let tcc = super::selfbuild::TCC;
    if exists(super::selfbuild3::LIBC_A).is_none() || exists(tcc).is_none() || exists(super::selfbuild3::PRINTF_C).is_none() || exists(super::selfbuild::PROBE).is_none() {
        return String::from("skip");
    }
    let out = alloc::format!("{}hello4.lnx", super::sys::home_prefix());
    let _ = crate::shell::vfs_mount_table().unlink(&crate::shell::vfs_path(&out), crate::fs::vfs::KERNEL_PRINCIPAL);
    let line = alloc::format!("{} -static -o {} {} && {}", tcc, out, super::selfbuild3::PRINTF_C, out);
    serial_println!("[selfbuild4] linux {} sh -c \"{}\"", super::selfbuild::PROBE, line);
    let (r, cap) = run_cap(super::selfbuild::PROBE, &["sh", "-c", line.as_str()], 90_000);
    for l in cap.lines().take(8) {
        serial_println!("[selfbuild4] tcc: {}", l);
    }
    match r {
        Ok(rep) => {
            serial_println!("{}", rep.witness(super::selfbuild::PROBE));
            if rep.pass && rep.exit == "0" && cap.lines().any(|l| l.trim() == super::selfbuild3::PRINTF_LINE) {
                String::from("ok")
            } else {
                alloc::format!("fail(exit={} out_bytes={})", rep.exit, exists(&out).unwrap_or(0))
            }
        }
        Err(e) => {
            serial_println!("[selfbuild4] {}: {}", super::selfbuild::PROBE, e);
            String::from("fail(load)")
        }
    }
}

/// `tests selfbuild4`.
pub fn selftest() {
    let kat = kat4();
    let r = rust();
    for c in [&cow::COW_FORKS, &cow::COW_SHARED, &cow::COW_COPIES, &cow::COW_REUSES] {
        c.store(0, Ordering::Relaxed);
    }
    let (fork, pipe) = pipeline();
    let tcc = tcc_run();
    serial_println!("[selfbuild4] tcc_run={}", tcc);
    let cow_pages = cow::COW_SHARED.load(Ordering::Relaxed);
    serial_println!(
        "[selfbuild4] kernel: cow_forks={} cow_shared={} cow_copies={} cow_reuses={} lazy_execs={} faults_file={} faults_anon={} peak_resident={}",
        cow::COW_FORKS.load(Ordering::Relaxed),
        cow_pages,
        cow::COW_COPIES.load(Ordering::Relaxed),
        cow::COW_REUSES.load(Ordering::Relaxed),
        exec::LAZY_EXECS.load(Ordering::Relaxed),
        vm::FAULTS_FILE.load(Ordering::Relaxed),
        vm::FAULTS_ANON.load(Ordering::Relaxed),
        vm::PEAK_RESIDENT.load(Ordering::Relaxed)
    );
    let parts_ok = r.rust == "ok" && fork == "ok" && pipe == "ok" && r.lazy && cow_pages > 0;
    let any_fail = r.rust.starts_with("fail") || fork.starts_with("fail") || pipe.starts_with("fail") || kat.starts_with("fail") || tcc.starts_with("fail");
    let verdict = if any_fail || (r.rust == "ok" && !parts_ok && fork != "skip") {
        "FAIL"
    } else if parts_ok && kat == "ok" && (tcc == "ok" || tcc == "skip") {
        "PASS"
    } else {
        "SKIP"
    };
    serial_println!(
        ":: SELFBUILD4: rust={} threads={} counter={} hashmap={} fs={} panic_caught={} fork={} pipe={} exec_lazy={} cow_pages={} -> {} ::",
        r.rust, r.threads, r.counter, r.hashmap, r.fs, r.panic, fork, pipe, if r.lazy { 1 } else { 0 }, cow_pages, verdict
    );
}
