// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — fs-core
//!
//! UNAFSTIME (B308, kernel half of audit B292): the kernel is the clock the UnaFS crate stamps
//! inodes with. F3F4 gave v6 inodes `ctime/mtime/atime` and a hook (`unafs::clock::set_clock_hook`);
//! without it a `no_std` build stamps 0. [`install_clock_hook`] runs at every mount attempt
//! (`fs/unafs.rs` `mount_on`, beside the warn hook) and is idempotent.
//!
//! The hook is called INSIDE `with_unafs`'s masked `MOUNT` hold, so it must never spin: it reads
//! [`crate::clock::try_unix_now`] and, on a momentarily contended anchor, answers the last second it
//! saw (never a guess past it). An unanchored clock answers 0 — the crate's honest "unknown", which
//! the VFS renders as `None`, never a fabricated date.
//!
//! The VFS face: [`vfs_time`] turns an inode stamp into the listing's `VfsTime`; [`ctime_of`] reads a
//! native object's ctime through `UnaFS::stat` (`vfs::Stat` gains no field: keep-both fold).

#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
static INSTALLED: AtomicBool = AtomicBool::new(false);
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
static LAST_SECS: AtomicU64 = AtomicU64::new(0);

/// The hook body: unix seconds, never blocking.
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
fn kernel_unix_secs() -> u64 {
    match crate::clock::try_unix_now() {
        Some(s) => {
            LAST_SECS.fetch_max(s, Ordering::Relaxed);
            s
        }
        None => LAST_SECS.load(Ordering::Relaxed),
    }
}

/// Install the kernel clock into the unafs crate (once; later calls are no-ops).
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
pub fn install_clock_hook() {
    if !INSTALLED.swap(true, Ordering::AcqRel) {
        if let Some(s) = crate::clock::try_unix_now() {
            LAST_SECS.store(s, Ordering::Relaxed);
        }
        ::unafs::clock::set_clock_hook(kernel_unix_secs);
    }
}

/// Has the hook been installed this boot?
pub fn hook_installed() -> bool {
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))]
    {
        INSTALLED.load(Ordering::Acquire)
    }
    #[cfg(not(any(target_arch = "aarch64", feature = "unafs")))]
    {
        false
    }
}

/// An inode stamp (unix seconds) as the listing's civil `VfsTime`; 0 (unknown) is `None`.
pub fn vfs_time(secs: u64) -> Option<crate::fs::vfs::VfsTime> {
    if secs == 0 {
        return None;
    }
    let (y, mo, d, h, mi, s) = crate::clock::civil_from_unix(secs);
    Some(crate::fs::vfs::VfsTime { year: y as u16, month: mo as u8, day: d as u8, hour: h as u8, min: mi as u8, sec: s as u8 })
}

/// `(ctime, mtime)` of the native object at `path` (namespace path), through `UnaFS::stat`.
/// `None` on a volume with no object id (FAT) or when the stamp is 0 (unknown).
pub fn times_of(mt: &crate::fs::vfs::MountTable, path: &str) -> Option<(u64, u64)> {
    let id = mt.stat(path).ok()?.id?;
    times_of_id(id)
}

/// `(ctime, mtime)` of a native inode id.
pub fn times_of_id(id: u64) -> Option<(u64, u64)> {
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))]
    {
        let st = crate::fs::unafs::with_unafs(|fs| fs.stat(id).ok()).ok()??;
        Some((st.ctime, st.mtime))
    }
    #[cfg(not(any(target_arch = "aarch64", feature = "unafs")))]
    {
        let _ = id;
        None
    }
}

/// The native ctime of `path`, `None` when unknown.
pub fn ctime_of(mt: &crate::fs::vfs::MountTable, path: &str) -> Option<u64> {
    times_of(mt, path).map(|t| t.0).filter(|c| *c != 0)
}

/// `stat`'s extra line on native: `  ctime:  <civil> (<unix>)`. Silent on FAT.
pub fn stat_ctime_line(console: &mut crate::console::Console, mt: &crate::fs::vfs::MountTable, path: &str) {
    if let Some(c) = ctime_of(mt, path) {
        if let Some(t) = vfs_time(c) {
            console.println(&alloc::format!(
                "  ctime:  {:04}-{:02}-{:02} {:02}:{:02}:{:02} ({} unix seconds)",
                t.year, t.month, t.day, t.hour, t.min, t.sec, c
            ));
        }
    }
}

/// `:: UNAFSTIME: hook=1 mtime=<ok> ctime=<ok> -> PASS|SKIP ::` — under the user's home on the
/// native volume: create a scratch file, its mtime and ctime lie within 5 s of the kernel clock;
/// write it a second later, its mtime advances. SKIP (reason=no-unafs-volume) on FAT, SKIP
/// (reason=clock-unset) while the kernel clock has no anchor.
pub fn selftest(base: &str) {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL as P};
    let t = crate::shell::vfs_mount_table();
    let hook = hook_installed() as u8;
    let native = matches!(t.stat(base), Ok(st) if st.id.is_some());
    if !native {
        serial_println!(":: UNAFSTIME: hook={} mtime=- ctime=- reason=no-unafs-volume -> SKIP ::", hook);
        return;
    }
    let Some(now) = crate::clock::unix_now() else {
        serial_println!(":: UNAFSTIME: hook={} mtime=- ctime=- reason=clock-unset -> SKIP ::", hook);
        return;
    };
    let path = alloc::format!("{}/TIMEFX.TXT", base.trim_end_matches('/'));
    let _ = t.unlink(&path, P);
    let created = t.create(&path, NodeKind::File, P).is_ok();
    let st0 = t.stat(&path).ok();
    let m0 = st0.as_ref().and_then(|s| s.mtime).unwrap_or(0);
    let c0 = ctime_of(&t, &path).unwrap_or(0);
    let near = |x: u64| x != 0 && x.abs_diff(now) <= 5;
    // A second must pass for a whole-second stamp to advance.
    let t0 = crate::arch::ms();
    while crate::arch::ms().saturating_sub(t0) < 1_100 {
        core::hint::spin_loop();
    }
    let wrote = t.write(&path, 0, b"time", P).is_ok();
    let m1 = t.stat(&path).ok().and_then(|s| s.mtime).unwrap_or(0);
    let listed = t
        .read_dir(base)
        .ok()
        .and_then(|v| v.into_iter().find(|e| e.name == "TIMEFX.TXT"))
        .and_then(|e| e.mtime)
        .is_some();
    let _ = t.unlink(&path, P);
    let mtime_ok = created && near(m0) && wrote && m1 > m0 && listed;
    let ctime_ok = near(c0);
    let pass = hook == 1 && mtime_ok && ctime_ok;
    serial_println!(
        ":: UNAFSTIME: hook={} mtime={} ctime={} m0={} m1={} c0={} now={} -> {} ::",
        hook,
        if mtime_ok { "ok" } else { "bad" },
        if ctime_ok { "ok" } else { "bad" },
        m0, m1, c0, now,
        if pass { "PASS" } else { "FAIL" }
    );
}
