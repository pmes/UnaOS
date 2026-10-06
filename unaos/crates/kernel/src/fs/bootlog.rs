//! CHARTER: Kernel — fs-core
//!
//! R99 / ROOTDISK2 (rmbp-ledger B401): the boot log leaves the boot FAT. The flight recorder's `UNAOS.LOG` reservation
//! (`flight_recorder::service`'s FAT path) is no longer taken on a build with UnaFS: [`divert`] owns the flush and
//! writes the recorder's own snapshot (the same ring, FLIGHTRING's on the merged tree — only the TARGET changes) to
//! `/var/log/boot-<n>.log` on the UnaFS root once one is bound; `<n>` is one past the newest there, and the oldest are
//! pruned to [`KEEP`]. Until a native root is bound the log stays in RAM; it never touches the FAT. The bench's capture
//! is the serial wire, unaffected.
//!
//! Wire: `:: FR: UNAOS.LOG not reserved — R99 …` once, then `[boot] log -> /var/log/boot-<n>.log … ::`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// Boot logs kept in `/var/log`.
pub const KEEP: u32 = 16;
const DIR: &str = "/var/log";
/// Main-loop passes between flush attempts (the recorder's own `FLUSH_EVERY_ITERS`).
const EVERY: usize = 4096;

static SAID: AtomicBool = AtomicBool::new(false);
static ITERS: AtomicUsize = AtomicUsize::new(0);
static LAST_LEN: AtomicUsize = AtomicUsize::new(usize::MAX);
/// The chosen `<n>` (0 = not chosen yet).
static N: AtomicU32 = AtomicU32::new(0);
static FAILED: AtomicBool = AtomicBool::new(false);

fn num(name: &str) -> Option<u32> {
    name.strip_prefix("boot-")?.strip_suffix(".log")?.parse().ok()
}

fn choose(mt: &crate::fs::vfs::MountTable) -> Option<u32> {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL};
    if mt.stat("/var").is_err() {
        let _ = mt.create("/var", NodeKind::Dir, KERNEL_PRINCIPAL);
    }
    if mt.stat(DIR).is_err() {
        let _ = mt.create(DIR, NodeKind::Dir, KERNEL_PRINCIPAL);
    }
    let have: Vec<u32> = mt.read_dir(DIR).ok()?.iter().filter_map(|e| num(&e.name)).collect();
    let n = have.iter().copied().max().unwrap_or(0) + 1;
    for old in have.iter().filter(|k| **k + KEEP <= n) {
        let _ = mt.unlink(&alloc::format!("{}/boot-{}{}", DIR, old, ".log"), KERNEL_PRINCIPAL);
    }
    let path = alloc::format!("{}/boot-{}{}", DIR, n, ".log");
    mt.create(&path, NodeKind::File, KERNEL_PRINCIPAL).ok()?;
    serial_println!("[boot] log -> {} (R99: the boot log lives on UnaFS; kept={}) ::", path, KEEP);
    Some(n)
}

/// `flight_recorder::service`, after the FRGUARD verdict: `true` = this module owns the flush (the caller returns
/// before its FAT reservation). Every UnaFS build owns it — the FAT reservation is gone (R99).
pub fn divert(snapshot: fn() -> Option<(Vec<u8>, usize)>) -> bool {
    if !cfg!(feature = "unafs") {
        return false;
    }
    if !SAID.swap(true, Ordering::Relaxed) {
        serial_println!(":: FR: UNAOS.LOG not reserved — R99: the boot FAT is sacred; the boot log goes to /var/log/boot-<n>.log on UnaFS (RAM until the root is bound) ::");
    }
    if FAILED.load(Ordering::Relaxed) || ITERS.fetch_add(1, Ordering::Relaxed) % EVERY != 0 {
        return true;
    }
    let mt = crate::shell::vfs_mount_table();
    if !mt.volume_name("/").map(|n| n == "native").unwrap_or(false) {
        return true;
    }
    let Some((body, len)) = snapshot() else { return true };
    if len == LAST_LEN.load(Ordering::Relaxed) {
        return true;
    }
    let n = match N.load(Ordering::Relaxed) {
        0 => match choose(&mt) {
            Some(n) => {
                N.store(n, Ordering::Relaxed);
                n
            }
            None => {
                FAILED.store(true, Ordering::Relaxed);
                serial_println!("[boot] log NOT written: {} could not be made on the root (the log stays in RAM) ::", DIR);
                return true;
            }
        },
        n => n,
    };
    let path: String = alloc::format!("{}/boot-{}{}", DIR, n, ".log");
    let k = crate::fs::vfs::KERNEL_PRINCIPAL;
    match mt.write(&path, 0, &body, k) {
        Ok(_) => {
            let _ = mt.truncate(&path, body.len() as u64, k);
            LAST_LEN.store(len, Ordering::Relaxed);
        }
        Err(e) => {
            FAILED.store(true, Ordering::Relaxed);
            serial_println!("[boot] log flush failed path={} err={:?} (the log stays in RAM) ::", path, e);
        }
    }
    true
}
