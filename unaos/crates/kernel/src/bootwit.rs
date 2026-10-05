// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling
//!
//! SELFDIAG M1 (rmbp-ledger B324, R82) — THE BOOT LOG ON DISK. A boot's verdicts used to exist only on the
//! serial wire; the smart installer's diagnosis program (`APPS/DIAG.ELF`) has to read them on the machine.
//! So the kernel keeps every witness line of the boot — `:: TAG: … -> PASS|FAIL|SKIP ::` and the `:: BOOT:`
//! line, the grammar `diag_core::witness::keep` defines — and writes them to `/var/log/boot.<n>.witness` on
//! the UnaFS root:
//!
//! * **capture** — [`note`] sits at the head of `serial_line::emit_src` (beside QUIETBOOT's tag tally): a
//!   cheap prefix test, then a `try_lock` copy into a 64 KiB static ring. Never blocks (contention counts a
//!   loss), never allocates, safe from a masked context. Non-FAIL lines stop at 56 KiB so a FAIL line still
//!   finds room late in the boot.
//! * **write** — [`mark_desktop`] (from `bootpace::boot_line`, right after the `:: BOOT:` line) arms one
//!   write that [`service`] performs from the main loop (IRQs on, no lock held, heap available); `power::
//!   reboot`/`shutdown` call [`flush`] directly, so the lines printed after desktop-ready (the `tests` runs)
//!   reach the disk too. ONLY when `/` is the native volume (`volume_name("/") == "native"`): on a FAT root
//!   nothing is written (the card's FAT is the boot medium; the log is the system volume's).
//! * **rotation** — `/var/log/boot.last` names the newest `n`; a boot takes `last + 1` at its first write and
//!   unlinks `boot.<n-8>.witness`, so the last [`KEEP`] boots are on disk.
//! * **the loop's other half** — at its first write a boot also reads `/var/log/diag.<n-1>.md` (the record
//!   DIAG.ELF wrote during the previous boot) and, if its verdict is not yet filled, appends `## next boot
//!   <n>` with each recorded tag's verdict on THIS boot (`diag_core::record::next_boot`).
//!
//! Nothing is printed (R80). `tests selfdiag` reports the state ([`state`]).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

/// Boots kept on disk.
pub const KEEP: u64 = 8;
pub const DIR: &str = "/var/log";
const EXT: &str = ".witness";
const CAP: usize = 64 * 1024;
const SOFT: usize = 56 * 1024;
const LINE_CAP: usize = 512;

struct Ring {
    b: [u8; CAP],
    n: usize,
}
static RING: spin::Mutex<Ring> = spin::Mutex::new(Ring { b: [0; CAP], n: 0 });
static KEPT: AtomicU64 = AtomicU64::new(0);
static LOST: AtomicU64 = AtomicU64::new(0);
static DESKTOP_PENDING: AtomicBool = AtomicBool::new(false);
static IN_WRITE: AtomicBool = AtomicBool::new(false);
/// This boot's number once assigned (0 = not yet).
static BOOT_N: AtomicU64 = AtomicU64::new(0);
static WRITES: AtomicU64 = AtomicU64::new(0);
/// 0 = nothing tried, 1 = written, 2 = root is not UnaFS, 3 = a write failed.
static STATE: AtomicU8 = AtomicU8::new(0);

/// The capture tap: `line` is the whole formatted line (with its `\n`).
#[inline]
pub fn note(line: &[u8]) {
    if line.len() < 8 || !line.starts_with(b":: ") {
        return;
    }
    let body = line.strip_suffix(b"\n").unwrap_or(line);
    let Ok(s) = core::str::from_utf8(body) else { return };
    if !diag_core::witness::keep(s) {
        return;
    }
    let fail = diag_core::witness::verdict(s) == Some(diag_core::witness::Verdict::Fail);
    let Some(mut r) = RING.try_lock() else {
        LOST.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let mut k = s.len().min(LINE_CAP);
    while k > 0 && !s.is_char_boundary(k) {
        k -= 1;
    }
    let lim = if fail { CAP } else { SOFT };
    if r.n + k + 1 > lim {
        drop(r);
        LOST.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let n = r.n;
    r.b[n..n + k].copy_from_slice(&s.as_bytes()[..k]);
    r.b[n + k] = b'\n';
    r.n = n + k + 1;
    KEPT.fetch_add(1, Ordering::Relaxed);
}

/// `bootpace::boot_line` — the desktop is up: arm the first write.
pub fn mark_desktop() {
    DESKTOP_PENDING.store(true, Ordering::Release);
}

/// The main loop's pass: performs the armed desktop-ready write.
pub fn service() {
    if DESKTOP_PENDING.load(Ordering::Acquire) && DESKTOP_PENDING.swap(false, Ordering::AcqRel) {
        flush("desktop");
    }
}

/// `(state word, boot n, lines kept, lines lost, writes)`.
pub fn state() -> (&'static str, u64, u64, u64, u64) {
    let w = match STATE.load(Ordering::Relaxed) {
        1 => "written",
        2 => "fat-root",
        3 => "error",
        _ => "none",
    };
    (w, BOOT_N.load(Ordering::Relaxed), KEPT.load(Ordering::Relaxed), LOST.load(Ordering::Relaxed), WRITES.load(Ordering::Relaxed))
}

/// The captured lines so far (a copy).
pub fn snapshot() -> Vec<u8> {
    let r = RING.lock();
    r.b[..r.n].to_vec()
}

pub fn witness_path(n: u64) -> String {
    format!("{}/boot.{}{}", DIR, n, EXT)
}

pub fn diag_path(n: u64) -> String {
    format!("{}/diag.{}{}", DIR, n, ".md")
}

pub fn last_path() -> String {
    format!("{}/boot.last", DIR)
}

// ── VFS helpers (the kernel principal; prefs.rs's shape) ─────────────────────────────────────────────

pub(crate) fn read_all(mt: &crate::fs::vfs::MountTable, p: &str, max: usize) -> Option<Vec<u8>> {
    let st = mt.stat(p).ok()?;
    if st.size as usize > max {
        return None;
    }
    if st.size == 0 {
        return Some(Vec::new());
    }
    mt.read(p, 0, st.size as usize).ok()
}

pub(crate) fn write_all(mt: &crate::fs::vfs::MountTable, p: &str, b: &[u8]) -> bool {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL};
    let _ = mt.unlink(p, KERNEL_PRINCIPAL);
    if mt.create(p, NodeKind::File, KERNEL_PRINCIPAL).is_err() {
        return false;
    }
    let mut off = 0usize;
    while off < b.len() {
        match mt.write(p, off as u64, &b[off..], KERNEL_PRINCIPAL) {
            Ok(0) | Err(_) => return false,
            Ok(w) => off += w,
        }
    }
    true
}

pub(crate) fn ensure_dir(mt: &crate::fs::vfs::MountTable, d: &str) {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL};
    if mt.stat(d).is_err() {
        let _ = mt.create(d, NodeKind::Dir, KERNEL_PRINCIPAL);
    }
}

/// Whether `/` is the native UnaFS volume.
pub fn root_is_unafs(mt: &crate::fs::vfs::MountTable) -> bool {
    matches!(mt.volume_name("/"), Ok(v) if v == "native")
}

/// Write this boot's witness file now (`why` = `desktop` | `reboot` | `shutdown`). Silent.
pub fn flush(why: &str) {
    if IN_WRITE.swap(true, Ordering::AcqRel) {
        return;
    }
    let mt = crate::shell::vfs_mount_table();
    if !root_is_unafs(&mt) {
        STATE.store(2, Ordering::Relaxed);
        IN_WRITE.store(false, Ordering::Release);
        return;
    }
    ensure_dir(&mt, "/var");
    ensure_dir(&mt, DIR);
    let mut first = false;
    let mut n = BOOT_N.load(Ordering::Relaxed);
    if n == 0 {
        let last = read_all(&mt, &last_path(), 64).and_then(|b| diag_core::parse_dec(trim(&b))).unwrap_or(0);
        n = last + 1;
        BOOT_N.store(n, Ordering::Relaxed);
        first = true;
        let _ = write_all(&mt, &last_path(), format!("{}\n", n).as_bytes());
        if n > KEEP {
            let _ = mt.unlink(&witness_path(n - KEEP), crate::fs::vfs::KERNEL_PRINCIPAL);
        }
    }
    let body = snapshot();
    let mut text = format!(
        "# boot {} witness lines={} lost={} at={} ms={}\n",
        n,
        KEPT.load(Ordering::Relaxed),
        LOST.load(Ordering::Relaxed),
        why,
        crate::arch::ms()
    )
    .into_bytes();
    text.extend_from_slice(&body);
    let ok = write_all(&mt, &witness_path(n), &text);
    STATE.store(if ok { 1 } else { 3 }, Ordering::Relaxed);
    if ok {
        WRITES.fetch_add(1, Ordering::Relaxed);
    }
    if first && n > 1 {
        fill_next_boot(&mt, n - 1, n, &body);
    }
    IN_WRITE.store(false, Ordering::Release);
}

/// Append `## next boot <n>` to `diag.<prev>.md` when it exists and is not filled yet.
pub(crate) fn fill_next_boot(mt: &crate::fs::vfs::MountTable, prev: u64, n: u64, witness: &[u8]) -> bool {
    let p = diag_path(prev);
    let Some(md) = read_all(mt, &p, 256 * 1024) else { return false };
    let (Ok(mds), Ok(ws)) = (core::str::from_utf8(&md), core::str::from_utf8(witness)) else { return false };
    let mut add = [0u8; 2048];
    let mut o = diag_core::Out::new(&mut add);
    if !diag_core::record::next_boot(mds, n, ws, &mut o) {
        return false;
    }
    let k = o.len();
    let mut all = md.clone();
    all.extend_from_slice(&add[..k]);
    write_all(mt, &p, &all)
}

fn trim(b: &[u8]) -> &[u8] {
    let mut e = b.len();
    while e > 0 && (b[e - 1] == b'\n' || b[e - 1] == b'\r' || b[e - 1] == b' ') {
        e -= 1;
    }
    &b[..e]
}
