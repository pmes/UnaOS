// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — fs-core
//!
//! NAMEINDEX (rmbp-ledger B432) — the kernel side of UnaFS's volume-wide name index. The index itself is the
//! shared core's (`unafs::fs::nameindex`: `una:fsname` facts written by the core on create / rename / unlink,
//! `find_names` = one range scan); this file only (1) builds it once on a volume written before it existed —
//! a LOGIN task (R93: the desktop is built at login; R80: nothing tests or migrates at boot), in chunks so the
//! IRQ-masked mount lock is never held for the whole volume — and (2) carries `tests nameindex`.
//!
//! Wire: `[unafs] name-index ready (marked)` / `[unafs] name-index built names=<n> ms=<n> chunks=<n>` /
//! `[unafs] name-index build FAILED at=<id> err=<e>`; `tests nameindex` →
//! `:: NAMEINDEX: names=<n> prefix_ms=<n> src=index -> PASS :: kat=… hits=<n> scanned=<n>`.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Inodes one service pass indexes (one commit).
const CHUNK: usize = 512;

static OWED: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
static NEXT: AtomicU64 = AtomicU64::new(1);
static NAMES: AtomicU64 = AtomicU64::new(0);
static CHUNKS: AtomicU32 = AtomicU32::new(0);
static T0: AtomicU64 = AtomicU64::new(0);

/// Is `/` the native UnaFS volume (the only root the index lives on)?
fn native_root() -> bool {
    crate::shell::vfs_mount_table().volume_name("/").map(|n| n == "native").unwrap_or(false)
}

/// `users::login`: the session opened — check the volume's name index and build it if it is missing.
pub fn post_login() {
    OWED.store(true, Ordering::Release);
}

/// The login task's pass — chained from the desktop's service tick (`settings::service`). Idle: one load.
/// One chunk per pass; the volume's own creates/renames keep their facts meanwhile (the build is idempotent).
pub fn service() {
    if !OWED.load(Ordering::Acquire) {
        return;
    }
    if !native_root() {
        OWED.store(false, Ordering::Release);
        return;
    }
    if !STARTED.swap(true, Ordering::AcqRel) {
        match crate::fs::unafs::with_unafs(|fs| fs.name_index_ready()) {
            Ok(Ok(true)) => {
                serial_println!("[unafs] name-index ready (marked)");
                OWED.store(false, Ordering::Release);
                return;
            }
            Ok(Ok(false)) => {
                T0.store(crate::arch::ms(), Ordering::Release);
                NEXT.store(1, Ordering::Release);
                NAMES.store(0, Ordering::Release);
                CHUNKS.store(0, Ordering::Release);
            }
            Ok(Err(e)) => {
                serial_println!("[unafs] name-index check FAILED err={:?}", e);
                OWED.store(false, Ordering::Release);
                return;
            }
            Err(_) => {
                STARTED.store(false, Ordering::Release); // busy or unmounted: ask again next pass
                return;
            }
        }
    }
    let from = NEXT.load(Ordering::Acquire);
    match crate::fs::unafs::with_unafs(|fs| fs.name_index_build_step(from, CHUNK).and_then(|r| if r.2 { fs.name_index_mark().map(|_| r) } else { Ok(r) })) {
        Ok(Ok((next, names, done))) => {
            NEXT.store(next, Ordering::Release);
            let total = NAMES.fetch_add(names as u64, Ordering::AcqRel) + names as u64;
            let chunks = CHUNKS.fetch_add(1, Ordering::AcqRel) + 1;
            if done {
                serial_println!("[unafs] name-index built names={} ms={} chunks={}", total, crate::arch::ms().saturating_sub(T0.load(Ordering::Acquire)), chunks);
                OWED.store(false, Ordering::Release);
            }
        }
        Ok(Err(e)) => {
            serial_println!("[unafs] name-index build FAILED at={} err={:?}", from, e);
            OWED.store(false, Ordering::Release);
        }
        Err(_) => {} // busy: the next pass retries this chunk
    }
}

/// `tests nameindex` registration, once (rides `filetype::ensure_tests`).
pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("nameindex", selftest);
    }
}

/// `tests nameindex` (typed, R80): the index is ready; `names` = `una:fsname` keys on the volume; `prefix_ms` =
/// one `find_names("test", 64)`; the live KAT creates `/var/NameIndexKat.txt`, finds it by `NAMEINDEXK`
/// (leaf prefix, case-folded), renames it to `nikat-moved.txt` (the old prefix answers nothing; the
/// new leaf prefix and the word prefix `moved` answer), unlinks it (nothing answers). PASS = ready + the KAT + (`/system/test-f` absent or `test`
/// hits it).
pub fn selftest() {
    if !native_root() {
        serial_println!(":: NAMEINDEX: names=0 prefix_ms=0 src=none -> SKIP :: reason=root-not-unafs ::");
        return;
    }
    let ready = matches!(crate::fs::unafs::with_unafs(|fs| fs.name_index_ready()), Ok(Ok(true)));
    if !ready {
        let owed = OWED.load(Ordering::Acquire);
        serial_println!(":: NAMEINDEX: names=0 prefix_ms=0 src=none -> FAIL :: reason={} next={} ::", if owed { "building" } else { "unbuilt" }, NEXT.load(Ordering::Acquire));
        return;
    }
    let names = crate::fs::unafs::with_unafs(|fs| fs.name_index_count()).ok().and_then(|r| r.ok()).unwrap_or(0);
    let t0 = crate::arch::ms();
    let probe = crate::fs::unafs::with_unafs(|fs| fs.find_names("test", 64)).ok().and_then(|r| r.ok());
    let prefix_ms = crate::arch::ms().saturating_sub(t0);
    let (hits, scanned) = probe.as_ref().map(|f| (f.hits.len(), f.scanned)).unwrap_or((0, 0));
    let testf = crate::shell::vfs_mount_table().stat("/system/test-f").is_ok();
    let testf_ok = !testf || probe.as_ref().map(|f| f.hits.iter().any(|h| h.path.starts_with("/system/test-f"))).unwrap_or(false);
    let kat = crate::fs::unafs::with_unafs(|fs| -> Result<&'static str, &'static str> {
        let count = |fs: &mut crate::fs::unafs::KernelUnaFS, q: &str| fs.find_names(q, 8).map(|f| f.hits.iter().filter(|h| h.path.contains("ikat") || h.path.contains("IndexKat")).count()).unwrap_or(99);
        let var = fs.resolve_path("/var").map_err(|_| "no-var")?;
        let _ = fs.unlink(var, "NameIndexKat.txt");
        let _ = fs.unlink(var, "nikat-moved.txt");
        fs.create_file(var, alloc::string::String::from("NameIndexKat.txt")).map_err(|_| "create")?;
        if count(fs, "NAMEINDEXK") != 1 {
            let _ = fs.unlink(var, "NameIndexKat.txt");
            return Err("create-find");
        }
        fs.rename(var, "NameIndexKat.txt", var, "nikat-moved.txt").map_err(|_| "rename")?;
        if count(fs, "nameindexk") != 0 || count(fs, "nikat-m") != 1 || count(fs, "moved") != 1 {
            let _ = fs.unlink(var, "nikat-moved.txt");
            return Err("rename-find");
        }
        fs.unlink(var, "nikat-moved.txt").map_err(|_| "unlink")?;
        if count(fs, "nikat") != 0 {
            return Err("unlink-find");
        }
        Ok("create,rename,unlink")
    });
    let kat_word = match kat {
        Ok(Ok(w)) => w,
        Ok(Err(w)) => w,
        Err(_) => "busy",
    };
    let kat_ok = matches!(kat, Ok(Ok(_)));
    let ok = kat_ok && testf_ok && names > 0;
    serial_println!(
        ":: NAMEINDEX: names={} prefix_ms={} src=index -> {} :: kat={} hits={} scanned={} test_f={} ::",
        names, prefix_ms, if ok { "PASS" } else { "FAIL" }, kat_word, hits, scanned, if !testf { "absent" } else if testf_ok { "ok" } else { "FAIL" }
    );
}
