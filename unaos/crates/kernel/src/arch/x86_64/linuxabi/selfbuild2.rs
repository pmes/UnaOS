// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//! SELFBUILD2 (B349, ROADMAP §1c SH-5) — `tests selfbuild2`: the threaded surface a modern toolchain needs, proven by
//! `SYSKAT2.LNX` (crates/user-linux-hello/c/syskat2.c: 4 threads from clone + clone3 contending a futex mutex to 4 x 100000,
//! futex WAIT/WAKE/BITSET/REQUEUE/timeouts, eventfd2 + epoll woken from a thread, statx of /apps/HELLO.C against the size the
//! loader sees, ftruncate/fallocate/flock on a scratch file, socketpair, sendfile, a signal delivered once through
//! rt_sigreturn), then the static glibc busybox `PROBE.LNX` running `sh -c "echo hi; cat /apps/HELLO.C"`.
//!
//! Witness: `:: LINUXABI-KAT2: threads=4 counter=400000 futex_waits=<n> epoll=ok statx=ok sigreturn=ok -> PASS ::` and
//! `:: SELFBUILD2: kat2=<ok|fail(..)|skip> probe=<ok|fail(..)|skip> hi=<0|1> cat_lines=<n> probe_syscalls=<n> probe_missing=[..]
//! -> PASS|FAIL|SKIP ::`. Absent fixtures = SKIP (staging is arroyo's `build_selfbuild_x86` + the builder).

use super::selfbuild::{PROBE, SRC};
use super::{run_path, signal, thread, Report};
use alloc::string::String;
use core::sync::atomic::Ordering;

pub const KAT2: &str = "/apps/SYSKAT2.LNX";
/// The probe's workload, exactly as the ledger row names it.
pub const PROBE_SH: &str = "echo hi; cat /apps/HELLO.C";

fn list(v: &[u32]) -> String {
    let mut s = String::new();
    for (i, n) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&alloc::format!("{}", n));
    }
    s
}

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

/// The value of `key=` in a `syskat2 ok …` line.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace().find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

/// SYSKAT2 leg: `"ok"`, `"fail(<id>)"`, `"fail(load)"` or `"skip"`.
fn kat2() -> String {
    let Some(src_size) = exists(SRC) else {
        serial_println!(":: LINUXABI-KAT2: threads=0 counter=0 futex_waits=0 epoll=skip statx=skip sigreturn=skip -> SKIP (HELLO.C not staged) ::");
        return String::from("skip");
    };
    let size = alloc::format!("{}", src_size);
    let scratch = alloc::format!("{}kat2.tmp", super::sys::home_prefix());
    let argv = [KAT2, SRC, size.as_str(), scratch.as_str()];
    let (r, cap) = run_cap(KAT2, &argv, 20_000);
    let rep = match r {
        Err(e) if e.contains("-ENOENT") => {
            serial_println!(":: LINUXABI-KAT2: threads=0 counter=0 futex_waits=0 epoll=skip statx=skip sigreturn=skip -> SKIP (fixture not staged) ::");
            return String::from("skip");
        }
        Err(e) => {
            serial_println!(":: LINUXABI-KAT2: threads=0 counter=0 futex_waits=0 epoll=? statx=? sigreturn=? -> FAIL (load: {}) ::", e);
            return String::from("fail(load)");
        }
        Ok(rep) => rep,
    };
    serial_println!("{}", rep.witness(KAT2));
    serial_println!(
        "[selfbuild2] kernel: threads_spawned={} futex_blocked={} signals_delivered={} sigreturns={}",
        thread::SPAWNED.load(Ordering::Acquire),
        thread::FUTEX_BLOCKED.load(Ordering::Acquire),
        signal::DELIVERED.load(Ordering::Acquire),
        signal::RETURNS.load(Ordering::Acquire)
    );
    let ok_line = cap.lines().find(|l| l.starts_with("syskat2 ok "));
    let failed = cap.lines().find_map(|l| l.strip_prefix("syskat2 fail ")).map(|s| String::from(s.trim()));
    match (ok_line, failed) {
        (Some(l), None) if rep.pass && rep.exit == "0" => {
            let g = |k| field(l, k).unwrap_or("?");
            let pass = g("threads") == "4" && g("counter") == "400000" && g("epoll") == "ok" && g("statx") == "ok" && g("sigreturn") == "ok";
            serial_println!(
                ":: LINUXABI-KAT2: threads={} counter={} futex_waits={} epoll={} statx={} sigreturn={} -> {} ::",
                g("threads"), g("counter"), g("futex_waits"), g("epoll"), g("statx"), g("sigreturn"),
                if pass { "PASS" } else { "FAIL" }
            );
            if pass { String::from("ok") } else { String::from("fail(fields)") }
        }
        (_, f) => {
            let id = f.unwrap_or_else(|| String::from("none"));
            serial_println!(
                ":: LINUXABI-KAT2: threads=? counter=? futex_waits=? epoll=? statx=? sigreturn=? fail={} exit={} blocked={} -> FAIL ::",
                id, rep.exit, rep.blocked
            );
            alloc::format!("fail({})", id)
        }
    }
}

/// `tests selfbuild2`.
pub fn selftest() {
    let kat = kat2();
    let first = crate::shell::vfs_mount_table()
        .read(&crate::shell::vfs_path(SRC), 0, 256)
        .ok()
        .map(|b| String::from(String::from_utf8_lossy(&b).lines().next().unwrap_or("")))
        .unwrap_or_default();
    let (probe, hi, cat_lines, pn, pm) = if exists(PROBE).is_some() && exists(SRC).is_some() {
        serial_println!("[selfbuild2] linux {} sh -c \"{}\"", PROBE, PROBE_SH);
        let (p, cap) = run_cap(PROBE, &["busybox", "sh", "-c", PROBE_SH], 20_000);
        match p {
            Ok(rep) => {
                serial_println!("{}", rep.witness(PROBE));
                let mut it = cap.lines();
                let hi = it.next() == Some("hi");
                let body: alloc::vec::Vec<&str> = it.collect();
                let cat_ok = !first.is_empty() && body.first().copied() == Some(first.as_str());
                for l in cap.lines().take(3) {
                    serial_println!("[selfbuild2] probe: {}", l);
                }
                let v = if rep.pass && rep.exit == "0" && hi && cat_ok && rep.enosys.is_empty() {
                    String::from("ok")
                } else {
                    alloc::format!("fail(exit={} hi={} cat={})", rep.exit, hi as u8, cat_ok as u8)
                };
                (v, hi as u8, body.len(), rep.nsys, list(&rep.enosys))
            }
            Err(e) => {
                serial_println!("[selfbuild2] PROBE.LNX: {}", e);
                (String::from("fail(load)"), 0, 0, 0, String::new())
            }
        }
    } else {
        (String::from("skip"), 0, 0, 0, String::new())
    };
    let verdict = if kat == "skip" && probe == "skip" {
        "SKIP"
    } else if kat == "ok" && (probe == "ok" || probe == "skip") {
        "PASS"
    } else {
        "FAIL"
    };
    serial_println!(
        ":: SELFBUILD2: kat2={} probe={} hi={} cat_lines={} probe_syscalls={} probe_missing=[{}] -> {} ::",
        kat, probe, hi, cat_lines, pn, pm, verdict
    );
}
