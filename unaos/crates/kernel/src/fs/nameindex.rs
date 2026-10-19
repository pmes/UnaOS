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
        crate::tests::register("nameindex", selftest); crate::tests::register("nameindextx", tx_selftest); // NAMEINDEXTX (B680): LINE-NEUTRAL append — `tests nameindextx` (file tail)
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

// ---------------------------------------------------------------------------------------------------------------
// NAMEINDEXTX (rmbp-ledger B680, BP-storage-16, BP-bar-14) — the name index cannot drift.
//
// The hook is the shared core's, BELOW the VFS: every name-changing primitive the native backend calls
// (`vfs.rs` `NativeBackend::create` → `UnaFS::create_file`/`mkdir` → `add_entry`, fs.rs:1864-1865;
// `create_files_batch`, fs.rs:1729; `unlink`, fs.rs:2017; `rmdir`, fs.rs:2126; `rename`, fs.rs:2242; fsck's
// `relink_inode`, fs.rs:3528) applies its `una:fsname` facts through `index_apply` in the SAME transaction as
// the directory rewrite, at most `NAME_WORDS_MAX` facts per name. No caller writes a name fact and no second
// hook exists (one writer, BP-storage-1); the only build is the login migration of an UNMARKED volume above.
// This fixture measures that through the mount table — the path every shell verb, syscall and Quarry takes:
// 2000 creates in 20 folders of 100 (a folder's listing is rewritten per create: bounded for QEMU), 200
// renames (every i%10==0), 200 unlinks (every i%10==5), then one `find_names` per original name. hits = the
// survivors found at their CURRENT path; misses = survivors not found; stale = any answer for a gone or
// renamed-away name, or any extra answer for a survivor. `rebuild=` = build chunks run during the fixture
// (must be 0) and the index must be ready (marked) before and after. `per_op_us` = the mean wall time of the
// 2400 VFS mutations; the max rides the detail line.
//
// Wire: `[nameindextx] detail ready=<yes|no> rebuild=<n> max_op_us=<n> errs=<create>/<rename>/<unlink> q_ms=<n>`
// then `:: NAMEINDEXTX: created=<n> renamed=<n> deleted=<n> hits=<n> misses=<n> stale=<n> per_op_us=<n> -> PASS ::`
// (FAIL carries ` reason=<word>` before the closing `::`; a non-UnaFS root is SKIP).
// ---------------------------------------------------------------------------------------------------------------

const TX_ROOT: &str = "/var/nixtx";
const TX_DIRS: usize = 20;
const TX_PER: usize = 100;

fn tx_dir(i: usize) -> alloc::string::String {
    alloc::format!("{}/d{:02}", TX_ROOT, i / TX_PER)
}

/// The original leaf of file `i` (no word separator: ONE name fact per file, so a query names one object).
fn tx_leaf(i: usize) -> alloc::string::String {
    alloc::format!("nixt{:04}", i)
}

/// The renamed-to leaf of file `i`.
fn tx_moved(i: usize) -> alloc::string::String {
    alloc::format!("nixr{:04}", i)
}

fn tx_renamed(i: usize) -> bool {
    i % 10 == 0
}

fn tx_deleted(i: usize) -> bool {
    i % 10 == 5
}

/// Remove the fixture's tree (a stale one from an interrupted run, and the run's own at the end).
fn tx_clear(mt: &crate::fs::vfs::MountTable) {
    use crate::fs::vfs::KERNEL_PRINCIPAL;
    for d in 0..TX_DIRS {
        let dir = alloc::format!("{}/d{:02}", TX_ROOT, d);
        if let Ok(ents) = mt.read_dir(&dir) {
            for e in ents {
                let _ = mt.unlink(&alloc::format!("{}/{}", dir, e.name), KERNEL_PRINCIPAL);
            }
            let _ = mt.remove_dir(&dir, KERNEL_PRINCIPAL);
        }
    }
    let _ = mt.remove_dir(TX_ROOT, KERNEL_PRINCIPAL);
}

/// `tests nameindextx` (typed, R80).
pub fn tx_selftest() {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL};
    const TOTAL: usize = TX_DIRS * TX_PER;
    if !native_root() {
        serial_println!(":: NAMEINDEXTX: created=0 renamed=0 deleted=0 hits=0 misses=0 stale=0 per_op_us=0 -> SKIP :: reason=root-not-unafs ::");
        return;
    }
    let ready = || matches!(crate::fs::unafs::with_unafs(|fs| fs.name_index_ready()), Ok(Ok(true)));
    let fail = |reason: &str| serial_println!(":: NAMEINDEXTX: created=0 renamed=0 deleted=0 hits=0 misses=0 stale=0 per_op_us=0 -> FAIL reason={} ::", reason);
    if !ready() {
        fail(if OWED.load(Ordering::Acquire) { "index-building" } else { "index-unbuilt" });
        return;
    }
    let chunks0 = CHUNKS.load(Ordering::Acquire);
    let mt = crate::shell::vfs_mount_table();
    tx_clear(&mt);
    if mt.stat("/var").is_err() {
        fail("no-var");
        return;
    }
    if mt.create(TX_ROOT, NodeKind::Dir, KERNEL_PRINCIPAL).is_err() {
        fail("mkdir");
        return;
    }
    for d in 0..TX_DIRS {
        if mt.create(&alloc::format!("{}/d{:02}", TX_ROOT, d), NodeKind::Dir, KERNEL_PRINCIPAL).is_err() {
            tx_clear(&mt);
            fail("mkdir");
            return;
        }
    }
    // The 2400 timed mutations: one VFS call each, one transaction each.
    let (mut op_us, mut max_us, mut ops) = (0u64, 0u64, 0u64);
    let mut timed = |r: bool, t0: u64| {
        let dt = crate::hidstall::now_us().saturating_sub(t0);
        op_us += dt;
        max_us = max_us.max(dt);
        ops += 1;
        r
    };
    let (mut created, mut renamed, mut deleted) = (0usize, 0usize, 0usize);
    let (mut c_err, mut r_err, mut u_err) = (0usize, 0usize, 0usize);
    for i in 0..TOTAL {
        let path = alloc::format!("{}/{}", tx_dir(i), tx_leaf(i));
        let t0 = crate::hidstall::now_us();
        if timed(mt.create(&path, NodeKind::File, KERNEL_PRINCIPAL).is_ok(), t0) { created += 1 } else { c_err += 1 }
    }
    for i in (0..TOTAL).filter(|&i| tx_renamed(i)) {
        let (from, to) = (alloc::format!("{}/{}", tx_dir(i), tx_leaf(i)), alloc::format!("{}/{}", tx_dir(i), tx_moved(i)));
        let t0 = crate::hidstall::now_us();
        if timed(mt.rename(&from, &to, KERNEL_PRINCIPAL).is_ok(), t0) { renamed += 1 } else { r_err += 1 }
    }
    for i in (0..TOTAL).filter(|&i| tx_deleted(i)) {
        let path = alloc::format!("{}/{}", tx_dir(i), tx_leaf(i));
        let t0 = crate::hidstall::now_us();
        if timed(mt.unlink(&path, KERNEL_PRINCIPAL).is_ok(), t0) { deleted += 1 } else { u_err += 1 }
    }
    let per_op_us = if ops > 0 { op_us / ops } else { 0 };
    // The search: no build, no rebuild — straight to the index, one query (and one lock hold) per name.
    let q0 = crate::arch::ms();
    let (mut hits, mut misses, mut stale) = (0usize, 0usize, 0usize);
    let ask = |leaf: &str| -> Option<alloc::vec::Vec<alloc::string::String>> {
        crate::fs::unafs::with_unafs(|fs| fs.find_names(leaf, 4)).ok().and_then(|r| r.ok()).map(|f| f.hits.into_iter().map(|h| h.path).filter(|p| p.contains("/nixtx/")).collect())
    };
    for i in 0..TOTAL {
        let dir = tx_dir(i);
        if tx_deleted(i) {
            stale += ask(&tx_leaf(i)).map(|v| v.len()).unwrap_or(0);
            continue;
        }
        if tx_renamed(i) {
            stale += ask(&tx_leaf(i)).map(|v| v.len()).unwrap_or(0);
        }
        let leaf = if tx_renamed(i) { tx_moved(i) } else { tx_leaf(i) };
        let want = alloc::format!("{}/{}", dir, leaf);
        match ask(&leaf) {
            Some(v) => {
                let found = v.iter().any(|p| p.ends_with(want.as_str()));
                if found { hits += 1 } else { misses += 1 }
                stale += v.len() - usize::from(found);
            }
            None => misses += 1,
        }
    }
    let q_ms = crate::arch::ms().saturating_sub(q0);
    let rebuild = CHUNKS.load(Ordering::Acquire).wrapping_sub(chunks0);
    let ready_after = ready();
    tx_clear(&mt);
    let survivors = TOTAL - TOTAL / 10;
    let reason = if c_err + r_err + u_err > 0 {
        "op-err"
    } else if !ready_after {
        "index-unmarked"
    } else if rebuild != 0 {
        "rebuilt"
    } else if misses > 0 {
        "misses"
    } else if stale > 0 {
        "stale"
    } else if hits != survivors {
        "hits"
    } else {
        ""
    };
    serial_println!(
        "[nameindextx] detail ready={} rebuild={} max_op_us={} errs={}/{}/{} q_ms={}",
        if ready_after { "yes" } else { "no" }, rebuild, max_us, c_err, r_err, u_err, q_ms
    );
    if reason.is_empty() {
        serial_println!(":: NAMEINDEXTX: created={} renamed={} deleted={} hits={} misses={} stale={} per_op_us={} -> PASS ::", created, renamed, deleted, hits, misses, stale, per_op_us);
    } else {
        serial_println!(":: NAMEINDEXTX: created={} renamed={} deleted={} hits={} misses={} stale={} per_op_us={} -> FAIL reason={} ::", created, renamed, deleted, hits, misses, stale, per_op_us, reason);
    }
}
