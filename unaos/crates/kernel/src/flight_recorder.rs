// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! FLIGHT-RECORDER (x86) — capture the serial boot log into a bounded in-kernel ring and flush it to
//! `UNAOS.LOG` on the FAT boot volume, so a consumer who boots the shareable `vm-image` in stock
//! QEMU/UTM/VirtualBox/VMware (with NO UnaOS harness flags, hence no serial capture) can copy the
//! whole boot log off the image afterward and send it back.
//!
//! Two halves, both x86-only:
//!   * **capture** — a THIRD additive tap on the single x86 serial print seam
//!     (`arch/x86_64/serial.rs::_print`), alongside `ftdi::mirror` and `selftest::capture`. It copies
//!     the exact formatted line bytes into a fixed static byte ring. Alloc-free, `try_lock` only, never
//!     blocks, drops (counting) on lock contention or when the ring is full — so it is safe from the
//!     IRQ-masked print context and never perturbs what is printed. **On by default** (no knob): the
//!     capture is a bounded memcpy under a `try_lock`, and the whole point is that a consumer gets the
//!     log with zero flags. The ring holds the EARLIEST bytes (the boot banner + self-tests), which is
//!     what the log is for, and stops (counting dropped bytes) once full rather than evicting the head.
//!   * **flush** — `service()` runs from the x86 main loop (right after `fat::probe_once`). Once a
//!     block device is up it mounts the FAT volume and writes the ring to `/UNAOS.LOG` via the SAME
//!     public `fat.rs` entry points the shell's `write` command uses (`find_located` /
//!     `create_in_root` / `delete_located` / `write_grow` — call-never-edit; creating a file in an
//!     existing FAT volume is not a format change). Write-through and bounded (a stalled USB write is
//!     `FatError::Io`, never a hang). Failures are honest-and-silent: one witness line, never a panic,
//!     never a block on boot. Re-flushes when the ring has grown since the last successful write,
//!     rate-limited so late lines still reach the disk before a hard power-off without churning the FAT.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;

/// Ring capacity. A full vm-image boot to the shell is a few hundred short serial lines
/// (`[INFO]…` / `::…::` / `-> PASS`), well under 64 KiB; sized to hold the whole boot log without
/// evicting the banner. A fixed static (BSS) — no heap.
const RING_CAP: usize = 64 * 1024;

struct LogRing {
    buf: [u8; RING_CAP],
    len: usize,
    dropped: usize,
}

static RING: Mutex<LogRing> = Mutex::new(LogRing {
    buf: [0u8; RING_CAP],
    len: 0,
    dropped: 0,
});

/// A tiny alloc-free `fmt::Write` sink: format `Arguments` into a fixed stack buffer so `capture`
/// touches the heap never (safe from any print context). Lines longer than the buffer are truncated
/// (boot lines are short); the newline the caller appended is inside `args`.
struct StackBuf {
    buf: [u8; 256],
    len: usize,
}
impl StackBuf {
    fn new() -> Self {
        StackBuf { buf: [0u8; 256], len: 0 }
    }
    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}
impl fmt::Write for StackBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            if self.len >= self.buf.len() {
                break; // truncate silently
            }
            self.buf[self.len] = b;
            self.len += 1;
        }
        Ok(())
    }
}

/// Additive hook on the serial print seam. Append the formatted line's bytes to the ring. Zero
/// behaviour change to what is printed; alloc-free; `try_lock` only (drops on contention) so it is
/// safe from IRQ-masked print contexts. Never takes the xHCI lock and never allocates.
pub fn capture(args: fmt::Arguments) {
    let mut sb = StackBuf::new();
    let _ = fmt::write(&mut sb, args);
    let bytes = sb.as_bytes();
    if bytes.is_empty() {
        return;
    }
    if let Some(mut ring) = RING.try_lock() {
        let room = RING_CAP - ring.len;
        if room == 0 {
            ring.dropped = ring.dropped.saturating_add(bytes.len());
            return;
        }
        let n = core::cmp::min(room, bytes.len());
        let at = ring.len;
        ring.buf[at..at + n].copy_from_slice(&bytes[..n]);
        ring.len += n;
        if n < bytes.len() {
            ring.dropped = ring.dropped.saturating_add(bytes.len() - n);
        }
    }
    // Lock contended -> drop silently (a diagnostic ring, not a ledger). Prints are serialised by the
    // serial lock the caller already holds, so contention here is rare.
}

/// Current number of captured bytes (for the flush's grow-detection). Cheap `try_lock`; `None` if the
/// ring is momentarily locked by a concurrent `capture` (retry next iteration).
fn captured_len() -> Option<usize> {
    RING.try_lock().map(|r| r.len)
}

/// Snapshot the captured bytes into a heap `Vec` (the flush runs at IF=1 in the main loop, so alloc
/// is fine), releasing the ring lock before the slow block I/O of the write. Returns the file bytes
/// AND the captured length under the SAME lock, so the caller records exactly what it wrote — never
/// a truncated snapshot marked as a full flush. `None` if the ring is momentarily locked (retry).
/// The `dropped` trailer keeps a full-ring log self-describing.
fn snapshot() -> Option<(alloc::vec::Vec<u8>, usize)> {
    let ring = RING.try_lock()?;
    let mut out = alloc::vec::Vec::with_capacity(ring.len + 128);
    // A self-identifying header IN THE FILE only (never emitted on the live serial stream, so the boot
    // output is byte-unchanged). Gives the tester a clear "this is a UnaOS boot log" marker and a
    // stable grep target — the bootloader's own banner runs in a separate UEFI binary before the
    // kernel's serial tap exists, so it is not in the captured ring.
    out.extend_from_slice(b":: UnaOS flight-recorder boot log (UNAOS.LOG) ::\n");
    out.extend_from_slice(&ring.buf[..ring.len]);
    if ring.dropped > 0 {
        let note = alloc::format!(
            "\n:: FLIGHTREC: {} byte(s) dropped (ring full / contended) ::\n",
            ring.dropped
        );
        out.extend_from_slice(note.as_bytes());
    }
    Some((out, ring.len))
}

const LOG_NAME: &str = "UNAOS.LOG";
/// Re-flush throttle: only re-write the log every N main-loop iterations even when it has grown, so a
/// steadily-printing kernel does not churn the FAT. Boot is bounded, so this mostly bounds the tail.
const FLUSH_EVERY_ITERS: u32 = 4096;

/// Write `data` to `/UNAOS.LOG` (truncate-or-create at the volume root) via the shell's proven public
/// `fat.rs` write path. Returns the bytes written or a `FatError`. Bounded + write-through by the
/// block layer; never edits `fat.rs`.
fn write_log(data: &[u8]) -> Result<usize, crate::fs::fat::FatError> {
    use crate::fs::fat::{self, FatError};
    let fs = fat::mount()?;
    // Truncate an existing UNAOS.LOG (free its chain, recreate a fresh 0-length entry), else create.
    let (dir_lba, dir_off) = match fs.find_located(LOG_NAME) {
        Ok((de, dl, doff)) => {
            fs.delete_located(dl, doff, de.first_cluster())?;
            let (_, dl2, doff2) = fs.create_in_root(LOG_NAME, 0x20)?;
            (dl2, doff2)
        }
        Err(FatError::NotFound) => {
            let (_, dl, doff) = fs.create_in_root(LOG_NAME, 0x20)?;
            (dl, doff)
        }
        Err(e) => return Err(e),
    };
    if data.is_empty() {
        return Ok(0);
    }
    let (written, _new_size, _first) = fs.write_grow(0, 0, dir_lba, dir_off, 0, data)?;
    Ok(written)
}

/// Drive the flight recorder from the x86 main loop (call every iteration; cheap when idle). Once a
/// block device is present, flush the captured log to `/UNAOS.LOG`: the first time storage comes up,
/// and thereafter whenever the ring has grown since the last successful flush (throttled). Honest-and-
/// silent: one witness on the first success, one on failure; never panics, never blocks boot.
pub fn service() {
    static LAST_FLUSHED: AtomicUsize = AtomicUsize::new(usize::MAX); // MAX = never flushed
    static ITERS: AtomicUsize = AtomicUsize::new(0);
    static ANNOUNCED: AtomicBool = AtomicBool::new(false);

    if crate::drivers::block::info().is_none() {
        return; // storage not up yet — nothing to flush to
    }

    let last = LAST_FLUSHED.load(Ordering::Relaxed);
    let first_time = last == usize::MAX;

    // Cheap growth gate first (a bare length read) so we don't allocate a snapshot every iteration.
    let cur = match captured_len() {
        Some(l) => l,
        None => return, // ring momentarily locked by a concurrent print — retry next iteration
    };
    if !first_time {
        // Only re-flush when there are NEW bytes, and only every FLUSH_EVERY_ITERS iterations so a
        // chatty kernel does not churn the FAT.
        if cur <= last {
            return;
        }
        let n = ITERS.fetch_add(1, Ordering::Relaxed) as u32;
        if n % FLUSH_EVERY_ITERS != 0 {
            return;
        }
    }

    // Take the file bytes AND their captured length under ONE lock — so LAST_FLUSHED records exactly
    // what we wrote, never a contention-truncated snapshot marked as a full flush.
    let (data, len) = match snapshot() {
        Some(pair) => pair,
        None => return, // contended at the snapshot moment — retry next iteration, marker unchanged
    };
    match write_log(&data) {
        Ok(written) => {
            LAST_FLUSHED.store(len, Ordering::Relaxed);
            if !ANNOUNCED.swap(true, Ordering::Relaxed) {
                serial_println!(
                    ":: FLIGHTREC: boot log -> {} ({} bytes) -> PASS ::",
                    LOG_NAME,
                    written
                );
            }
        }
        Err(e) => {
            use crate::fs::fat::FatError;
            // Do not spin retrying a broken volume: mark this length flushed so we only retry once the
            // log grows further. One honest witness (once) — never a panic.
            LAST_FLUSHED.store(len, Ordering::Relaxed);
            match e {
                // No FAT boot volume here (a raw/non-FAT stick, or no disk). This is the NORMAL case
                // for the default `test` (raw usb.img) and any non-FAT medium — there is simply
                // nowhere to record to, not an error. Skip silently (no scary witness line).
                FatError::NotFat | FatError::NoDisk => {}
                // A real write error (I/O stall, full volume, bad chain) AFTER a successful mount —
                // one honest witness, once.
                _ => {
                    if !ANNOUNCED.swap(true, Ordering::Relaxed) {
                        serial_println!(":: FLIGHTREC: flush to {} write error ({:?}) ::", LOG_NAME, e);
                    }
                }
            }
        }
    }
}
