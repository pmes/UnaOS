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
//!
//! SMALLFIX8 (rmbp-ledger B516): flight 28's `[hid] stall census holder=usb-pump:flight-recorder n=12 worst_ms=483` is
//! THIS write — the whole snapshot (up to the recorder's 256 KiB `RING_CAP`) rewritten into one IRQ-masked UnaFS
//! transaction every [`EVERY`] passes of the `usb-pump`, the task that shares its core with the `hid-pump`: each flush
//! was up to 483 ms with no HID pass. On x86 the flush now runs on its own `bootlog-flush` task on a worker core that
//! is neither the render nor the service core (`smp::xhci_worker_cpu`, the Dock's Trash read's rule — UnaFS may take
//! the storage loan), paced at [`FLUSH_MS`]; the pump's step only starts it. With no such core it stays inline, as
//! before. Wire: `[boot] log flush on=worker cpu=<c> every_ms=<n>` once, then `[boot] log flushed n=1 captured=<b>
//! took_ms=<t> on=worker` for the first flush.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// Boot logs kept in `/var/log`.
pub const KEEP: u32 = 16;
const DIR: &str = "/var/log";
/// Main-loop passes between flush attempts (the recorder's own `FLUSH_EVERY_ITERS`).
const EVERY: usize = 4096;
/// SMALLFIX8: the worker's pace between flush attempts — the inline cadence's 4096 passes at the pump's ~1 ms nap.
#[cfg(all(feature = "unafs", target_arch = "x86_64"))]
const FLUSH_MS: u64 = 4096;
/// SMALLFIX8: the `bootlog-flush` task has been started (the pump's step returns at once after that).
#[cfg(all(feature = "unafs", target_arch = "x86_64"))]
static FLUSHER: AtomicBool = AtomicBool::new(false);

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
    #[cfg(all(feature = "unafs", target_arch = "x86_64"))]
    if off_pump(snapshot) {
        return true; // SMALLFIX8 (B516): the flush is the `bootlog-flush` worker's, off the hid pump's core
    }
    if FAILED.load(Ordering::Relaxed) || ITERS.fetch_add(1, Ordering::Relaxed) % EVERY != 0 {
        return true;
    }
    flush(snapshot);
    true
}

/// SMALLFIX8: start the `bootlog-flush` task once on a worker core apart from the render, service and hid-pump cores;
/// `false` = no such core (the caller flushes inline, as before).
#[cfg(all(feature = "unafs", target_arch = "x86_64"))]
fn off_pump(snapshot: fn() -> Option<(Vec<u8>, usize)>) -> bool {
    if FLUSHER.load(Ordering::Acquire) {
        return true;
    }
    let Some(cpu) = crate::arch::smp::xhci_worker_cpu(0).filter(|&c| c != crate::hidstall::pump_cpu()) else { return false };
    if FLUSHER.swap(true, Ordering::AcqRel) {
        return true;
    }
    serial_println!("[boot] log flush on=worker cpu={} every_ms={} (SMALLFIX8: the boot-log write leaves the hid pump's core) ::", cpu, FLUSH_MS);
    crate::arch::sched::spawn("bootlog-flush", flusher, snapshot as usize, cpu, crate::arch::sched::PRIO_NORMAL);
    true
}

/// `bootlog-flush`: one flush per [`FLUSH_MS`] for the session (a flush with nothing new returns at once); exits only
/// when a flush failed (the log stays in RAM, said once by [`flush`]).
#[cfg(all(feature = "unafs", target_arch = "x86_64"))]
fn flusher(snap: usize) {
    // SAFETY: `snap` is the `fn() -> Option<(Vec<u8>, usize)>` `off_pump` passed as its spawn argument, unchanged.
    let snapshot: fn() -> Option<(Vec<u8>, usize)> = unsafe { core::mem::transmute::<usize, fn() -> Option<(Vec<u8>, usize)>>(snap) };
    let mut said = false;
    while !FAILED.load(Ordering::Relaxed) {
        let t0 = crate::arch::ms();
        let before = LAST_LEN.load(Ordering::Relaxed);
        flush(snapshot);
        let after = LAST_LEN.load(Ordering::Relaxed);
        if !said && after != before && after != usize::MAX {
            said = true;
            serial_println!("[boot] log flushed n=1 captured={} took_ms={} on=worker (SMALLFIX8) ::", after, crate::arch::ms().saturating_sub(t0));
        }
        crate::arch::sched::sleep_ms(FLUSH_MS);
    }
}

/// One flush: the snapshot written over `/var/log/boot-<n>.log` when it grew (`<n>` chosen on the first).
fn flush(snapshot: fn() -> Option<(Vec<u8>, usize)>) {
    let mt = crate::shell::vfs_mount_table();
    if !mt.volume_name("/").map(|n| n == "native").unwrap_or(false) {
        return;
    }
    let Some((body, len)) = snapshot() else { return };
    if len == LAST_LEN.load(Ordering::Relaxed) {
        return;
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
                return;
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
}

/// SESSIONWEDGE (rmbp-ledger B525, `tests sessionwedge`): the `bootlog-flush` task was started (SMALLFIX8). It runs on
/// `xhci_worker_cpu(0)` — in the Exclusive tier the SAME core as `worker_cpu(0)`, where `smc-batt` and the witness tasks
/// live — so its IRQ-masked UnaFS write is a stall of that whole core, and the verb names it among the armed.
#[cfg(all(feature = "unafs", target_arch = "x86_64"))]
pub fn flusher_armed() -> bool {
    FLUSHER.load(Ordering::Relaxed)
}
