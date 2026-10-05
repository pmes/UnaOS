// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//! SELFBUILD1 (B344, ROADMAP §1c SH-5 rung) — UnaOS compiles a C program ON ITSELF: the static Linux `tcc` staged as
//! `/apps/TCC.LNX` runs under the Linux ABI shim, compiles `/apps/HELLO.C` into `<home>/hello.lnx` on the volume, and that
//! output runs under the same shim. Plus the measurement legs: `SYSKAT.LNX` (the syscall known-answer tests, second witness
//! of `tests linuxabi`) and `PROBE.LNX` (a static busybox, the biggest static Linux binary the host could build: its syscall
//! count and its `-ENOSYS` list are the surface a bigger toolchain will hit first). `rustc` itself is NOT probed (no static
//! rustc can be built in the build container) — the doc (`docs/dev/evidence/rmbp-1005/SELFBUILD1.md`) says what it would need.
//!
//! The binaries are built on the HOST by `arroyo` (tcc from its upstream source, LGPL — only the binary is staged on the
//! volume, its source never enters this tree) and staged by the builder into `APPS/`. Absent `TCC.LNX` = `-> SKIP`.

use super::{run_path, Report};
use alloc::string::String;
use alloc::vec::Vec;

pub const TCC: &str = "/apps/TCC.LNX";
pub const SRC: &str = "/apps/HELLO.C";
pub const KAT: &str = "/apps/SYSKAT.LNX";
pub const PROBE: &str = "/apps/PROBE.LNX";
/// What HELLO.C prints (crates/user-linux-hello/c/hello.c).
const HELLO_LINE: &str = "hello from tcc on unaos";
/// The probe's workload: a standalone busybox shell (applets re-exec `/proc/self/exe`), a pipe, a file read, a listing.
const PROBE_SH: &str = "echo probe | wc -c; ls /apps; head -n 3 /apps/HELLO.C; uname -a";

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

fn run_cap(path: &str, argv: &[&str], ms: u64) -> (Result<Report, String>, String) {
    let mut cap = String::new();
    let r = run_path(path, argv, ms, false, &mut |l| {
        cap.push_str(l);
        cap.push('\n');
    });
    (r, cap)
}

/// `tests linuxabi` second witness: `SYSKAT.LNX` (crates/user-linux-hello/c/syskat.c) — mmap/mprotect/munmap on real pages,
/// brk growth, openat/fstat/newfstatat/readlinkat/getdents64 over the VFS, pipe2/dup3, the accepted signal calls,
/// clock_gettime/nanosleep, getrandom, uname, the start-up no-ops. It exits with the id of the first failed check.
pub fn kat() {
    let (r, cap) = run_cap(KAT, &[KAT], 5_000);
    match r {
        Err(e) if e.contains("-ENOENT") => {
            serial_println!(":: LINUXABI-KAT: path={} checks=0 fail=none exit=? syscalls=0 enosys=[] -> SKIP reason=fixture-not-staged ::", KAT)
        }
        Err(e) => serial_println!(":: LINUXABI-KAT: path={} checks=0 fail=load exit=? syscalls=0 enosys=[] -> FAIL ({}) ::", KAT, e),
        Ok(rep) => {
            let checks = cap
                .lines()
                .find_map(|l| l.strip_prefix("syskat ok checks="))
                .and_then(|n| n.trim().parse::<u32>().ok())
                .unwrap_or(0);
            let failed = cap.lines().find_map(|l| l.strip_prefix("syskat fail ")).map(|s| String::from(s.trim()));
            let pass = rep.pass && rep.exit == "0" && checks > 0 && failed.is_none();
            serial_println!(
                ":: LINUXABI-KAT: path={} checks={} fail={} exit={} syscalls={} enosys=[{}] -> {} ::",
                KAT,
                checks,
                failed.as_deref().unwrap_or("none"),
                rep.exit,
                rep.nsys,
                list(&rep.enosys),
                if pass { "PASS" } else { "FAIL" }
            );
        }
    }
}

fn exists(path: &str) -> Option<u64> {
    let mt = crate::shell::vfs_mount_table();
    mt.stat(&crate::shell::vfs_path(path)).ok().map(|s| s.size)
}

/// `tests selfbuild` — `:: SELFBUILD1: tcc=<ok|enosys:[…]|fail(…)|skip> hello=<ok|fail(…)|skip> probe=<name> probe_syscalls=<n>
/// probe_missing=[…] -> PASS|FAIL|SKIP ::`. PASS = tcc compiled HELLO.C into a file AND that file ran and printed its line.
/// The probe is measurement (its count and `-ENOSYS` list), not part of the verdict.
pub fn selftest() {
    let out = alloc::format!("{}hello.lnx", super::sys::home_prefix());
    if exists(TCC).is_none() || exists(SRC).is_none() {
        serial_println!(
            ":: SELFBUILD1: tcc=skip hello=skip probe=none probe_syscalls=0 probe_missing=[] -> SKIP reason=tcc-hello-not-staged ::"
        );
        return;
    }
    // A stale output from an earlier run must not pass for a fresh one.
    let _ = crate::shell::vfs_mount_table().unlink(&crate::shell::vfs_path(&out), crate::fs::vfs::KERNEL_PRINCIPAL);
    let argv: Vec<&str> = alloc::vec!["tcc", "-nostdlib", "-static", "-o", out.as_str(), "-x", "c", SRC]; // SELFBUILD4: `-x c` — tcc types a file by its extension and the volume's name ends in upper-case `.C` (host-proven: `unrecognized file type` without it)
    serial_println!("[selfbuild] linux {} -nostdlib -static -o {} -x c {}", TCC, out, SRC);
    let (r, cap) = run_cap(TCC, &argv, 30_000);
    for l in cap.lines().take(8) {
        serial_println!("[selfbuild] tcc: {}", l);
    }
    let tcc = match &r {
        Ok(rep) => {
            serial_println!("{}", rep.witness(TCC));
            let made = exists(&out).unwrap_or(0);
            if !rep.enosys.is_empty() {
                alloc::format!("enosys:[{}]", list(&rep.enosys))
            } else if rep.pass && rep.exit == "0" && made > 0 {
                serial_println!("[selfbuild] output {} bytes={}", out, made);
                String::from("ok")
            } else {
                alloc::format!("fail(exit={} out_bytes={})", rep.exit, made)
            }
        }
        Err(e) => {
            serial_println!("[selfbuild] TCC.LNX: {}", e);
            String::from("fail(load)")
        }
    };
    let hello = if tcc == "ok" {
        let (h, hcap) = run_cap(&out, &[out.as_str()], 5_000);
        match h {
            Ok(rep) => {
                serial_println!("{}", rep.witness(&out));
                if rep.pass && rep.exit == "0" && hcap.lines().any(|l| l == HELLO_LINE) {
                    String::from("ok")
                } else {
                    alloc::format!("fail(exit={})", rep.exit)
                }
            }
            Err(e) => {
                serial_println!("[selfbuild] {}: {}", out, e);
                String::from("fail(load)")
            }
        }
    } else {
        String::from("skip")
    };
    let (probe, pn, pm) = if exists(PROBE).is_some() {
        let (p, pcap) = run_cap(PROBE, &["busybox", "sh", "-c", PROBE_SH], 20_000);
        match p {
            Ok(rep) => {
                serial_println!("{}", rep.witness(PROBE));
                serial_println!("[selfbuild] probe lines={} exit={}", pcap.lines().count(), rep.exit);
                (String::from("busybox"), rep.nsys, list(&rep.enosys))
            }
            Err(e) => {
                serial_println!("[selfbuild] PROBE.LNX: {}", e);
                (String::from("busybox(load-fail)"), 0, String::new())
            }
        }
    } else {
        (String::from("none"), 0, String::new())
    };
    let pass = tcc == "ok" && hello == "ok";
    serial_println!(
        ":: SELFBUILD1: tcc={} hello={} probe={} probe_syscalls={} probe_missing=[{}] -> {} ::",
        tcc,
        hello,
        probe,
        pn,
        pm,
        if pass { "PASS" } else { "FAIL" }
    );
}
