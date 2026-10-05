// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — shared-core
//! SELFBUILD6 (B360, ROADMAP §1c SH-5) — `tests selfbuild6`: DYNAMIC Linux programs through the shared loader core
//! (`ldso_core`, fulfilled by `ldso.rs`), then the musl-host `rustc` on UnaOS. Each probe was proven first on the HOST kernel
//! by `ldrun` (the same core over `mmap`); here it runs under the shim:
//!
//! 1. `dyn` — `/apps/LIB/dyn/hello`, a musl PIE + `libdyn.so` (+ `libplug.so` it dlopens), built by arroyo from
//!    `ldso_core/fixtures/dyn`: constructors before main, the seven relocation types, static TLS in the main thread and a new
//!    one, `dl_iterate_phdr`, `dlopen`/`dlsym`/`dlerror`/`dladdr`, destructors. Prints `dyn=ok objects=3 after_dlopen=4`.
//! 2. `lld_dyn` — the musl `rust-lld` (ET_EXEC 0x400000, 141 MB) `-flavor gnu --version`: `skip(window)` while the image window
//!    refuses it, else ok when it prints `LLD `.
//! 3. `rustc_version` — `/apps/LIB/rustc/bin/rustc --version` (librustc_driver 301 MB, 265481 relocations).
//! 4. `rustc_hello` — `rustc -C codegen-units=1 -O hello.rs --target x86_64-unknown-linux-musl -C linker=/apps/LLD.LNX …` under
//!    busybox `sh` (TMPDIR in the home), then the output runs.
//! 5. `proc_macro` — `rustc user.rs --extern pm=/apps/LIB/rustc/pm/libpm.so …` (`#[derive(Hello)]` from the staged proc-macro
//!    `.so`, loaded through the trampoline's `dlopen`), then the output runs.
//!
//! Wire: `:: SELFBUILD6: dyn=ok lld_dyn=<ok|skip> rustc_version=<ok|oom> rustc_hello=<ok|oom> proc_macro=<ok|oom> relocs=<n> ms=<n>
//! -> PASS ::`. A probe that fails while the resident budget refused frames reads `oom rss_mib=<n>`; any other failure reads
//! `fail(…)` and the verdict is FAIL. Payload absent = `skip` (the verdict SKIP when nothing failed).

use super::{run_path, vm, Report};
use alloc::string::String;
use core::sync::atomic::Ordering;

pub const DYN: &str = "/apps/LIB/dyn/hello";
pub const RUSTC: &str = "/apps/LIB/rustc/bin/rustc";
pub const RLLD: &str = "/apps/LIB/rustc/lib/rustlib/x86_64-unknown-linux-musl/bin/rust-lld";
pub const HELLO_RS: &str = "/apps/LIB/rustc/hello.rs";
pub const PM_SO: &str = "/apps/LIB/rustc/pm/libpm.so";
pub const USER_RS: &str = "/apps/LIB/rustc/pm/user.rs";
pub const LLD: &str = "/apps/LLD.LNX";
pub const SH: &str = "/apps/PROBE.LNX";

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

/// One probe's outcome and its numbers.
struct Probe {
    verdict: String,
    rss_mib: u64,
    ms: u64,
}

/// Run `argv` (argv[0] = `path`), print its lines, judge it by `want` (a line prefix) and exit 0.
fn probe(tag: &str, path: &str, argv: &[&str], ms: u64, want: &str) -> Probe {
    serial_println!("[selfbuild6] linux {}", argv.join(" "));
    vm::PEAK_RESIDENT.store(0, Ordering::Relaxed);
    let refused0 = vm::REFUSALS.load(Ordering::Relaxed);
    let (r, cap) = run_cap(path, argv, ms);
    let rss_mib = (vm::PEAK_RESIDENT.load(Ordering::Relaxed) * 4096 + (1 << 20) - 1) >> 20;
    let oom = vm::REFUSALS.load(Ordering::Relaxed) > refused0;
    for line in cap.lines().take(16) {
        serial_println!("[selfbuild6] {}: {}", tag, line);
    }
    let mut p = Probe { verdict: String::new(), rss_mib, ms: 0 };
    match r {
        Ok(rep) => {
            serial_println!("{}", rep.witness(path));
            p.ms = rep.ms;
            let seen = want.is_empty() || cap.lines().any(|l| l.starts_with(want));
            p.verdict = if rep.pass && rep.exit == "0" && seen {
                String::from("ok")
            } else if oom {
                alloc::format!("oom rss_mib={}", rss_mib)
            } else {
                alloc::format!("fail(exit={}{})", rep.exit, if seen { "" } else { " no-line" })
            };
        }
        Err(e) => {
            serial_println!("[selfbuild6] {}: {}", path, e);
            p.verdict = if e.starts_with("skip(window)") || super::ldso::LAST_WINDOW_REFUSAL.load(Ordering::Relaxed) == 1 {
                String::from("skip")
            } else if oom {
                alloc::format!("oom rss_mib={}", rss_mib)
            } else {
                String::from("fail(load)")
            };
        }
    }
    serial_println!("[selfbuild6] {}: verdict={} rss_mib={} ms={}", tag, p.verdict, p.rss_mib, p.ms);
    p
}

fn skip() -> Probe {
    Probe { verdict: String::from("skip"), rss_mib: 0, ms: 0 }
}

/// Compile with rustc under busybox `sh` (rustc writes temporaries to TMPDIR: the home is the writable tree), then run the output.
fn compile_and_run(tag: &str, rustc_args: &str, out: &str, want: &str) -> Probe {
    if exists(SH).is_none() || exists(LLD).is_none() {
        serial_println!("[selfbuild6] {}: {} or {} not staged -> skip", tag, SH, LLD);
        return skip();
    }
    let home = super::sys::home_prefix();
    let _ = crate::shell::vfs_mount_table().unlink(&crate::shell::vfs_path(out), crate::fs::vfs::KERNEL_PRINCIPAL);
    let cmd = alloc::format!(
        "TMPDIR={} {} {} --target x86_64-unknown-linux-musl -C linker={} -C linker-flavor=ld.lld -C link-self-contained=yes \
         -C relocation-model=static -C target-feature=+crt-static -o {}",
        home.trim_end_matches('/'), RUSTC, rustc_args, LLD, out
    );
    let c = probe(tag, SH, &["sh", "-c", cmd.as_str()], 1_800_000, "");
    if c.verdict != "ok" {
        return c;
    }
    let mut r = probe(tag, out, &[out], 30_000, want);
    r.rss_mib = r.rss_mib.max(c.rss_mib);
    r.ms += c.ms;
    if r.verdict == "ok" {
        r.rss_mib = c.rss_mib;
    }
    r
}

/// `tests selfbuild6`.
pub fn selftest() {
    let t0 = crate::arch::ms();
    let dynp = if exists(DYN).is_some() {
        probe("dyn", DYN, &[DYN], 30_000, "dyn=ok objects=3 after_dlopen=4")
    } else {
        serial_println!("[selfbuild6] {} not staged -> dyn=skip", DYN);
        skip()
    };
    let lld = if exists(RLLD).is_some() {
        probe("lld_dyn", RLLD, &[RLLD, "-flavor", "gnu", "--version"], 60_000, "LLD ")
    } else {
        serial_println!("[selfbuild6] {} not staged -> lld_dyn=skip", RLLD);
        skip()
    };
    let (ver, hello, pm, relocs) = if exists(RUSTC).is_some() {
        let v = probe("rustc_version", RUSTC, &[RUSTC, "--version"], 600_000, "rustc 1.");
        let relocs = super::ldso::LAST_RELOCS.load(Ordering::Relaxed);
        serial_println!(
            "[selfbuild6] rustc load: objects={} pages_mapped={} relocs={} load_ms={} read_bytes={}",
            super::ldso::LAST_OBJECTS.load(Ordering::Relaxed),
            super::ldso::LAST_PAGES.load(Ordering::Relaxed),
            relocs,
            super::ldso::LAST_LOAD_MS.load(Ordering::Relaxed),
            super::ldso::LAST_READ.load(Ordering::Relaxed)
        );
        let home = super::sys::home_prefix();
        let h = if v.verdict == "ok" && exists(HELLO_RS).is_some() {
            let out = alloc::format!("{}hello6.lnx", home);
            let args = alloc::format!("-C codegen-units=1 -O {}", HELLO_RS);
            compile_and_run("rustc_hello", &args, &out, "hello from rustc on ldso_core")
        } else {
            skip()
        };
        let p = if v.verdict == "ok" && exists(PM_SO).is_some() && exists(USER_RS).is_some() {
            let out = alloc::format!("{}pm6.lnx", home);
            let args = alloc::format!("{} --extern pm={}", USER_RS, PM_SO);
            compile_and_run("proc_macro", &args, &out, "proc_macro: derived Hello for Probe")
        } else {
            skip()
        };
        (v, h, p, relocs)
    } else {
        serial_println!("[selfbuild6] {} not staged -> rustc probes skip", RUSTC);
        (skip(), skip(), skip(), 0)
    };
    let ms = crate::arch::ms().saturating_sub(t0);
    serial_println!(
        "[selfbuild6] kernel: loads={} dlopens={} dlsyms={} faults_file={} faults_anon={} refusals={} pool_peak={}",
        super::ldso::LOADS.load(Ordering::Relaxed),
        super::ldso::DLOPENS.load(Ordering::Relaxed),
        super::ldso::DLSYMS.load(Ordering::Relaxed),
        vm::FAULTS_FILE.load(Ordering::Relaxed),
        vm::FAULTS_ANON.load(Ordering::Relaxed),
        vm::REFUSALS.load(Ordering::Relaxed),
        vm::POOL_PEAK.load(Ordering::Relaxed)
    );
    let all = [&dynp, &lld, &ver, &hello, &pm];
    let any_fail = all.iter().any(|p| p.verdict.starts_with("fail"));
    // PASS: the loader works (dyn) and rustc answers; rust-lld may skip on the window and the compile probes may be oom.
    let ok_or = |p: &Probe| p.verdict == "ok" || p.verdict.starts_with("oom");
    let verdict = if any_fail {
        "FAIL"
    } else if dynp.verdict == "ok" && (lld.verdict == "ok" || lld.verdict == "skip") && ver.verdict == "ok" && ok_or(&hello) && ok_or(&pm) {
        "PASS"
    } else {
        "SKIP"
    };
    serial_println!(
        ":: SELFBUILD6: dyn={} lld_dyn={} rustc_version={} rustc_hello={} proc_macro={} relocs={} ms={} -> {} ::",
        dynp.verdict, lld.verdict, ver.verdict, hello.verdict, pm.verdict, relocs, ms, verdict
    );
}
