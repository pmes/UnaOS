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

impl LogRing {
    /// Append what fits, counting what does not. The ring keeps the EARLIEST bytes (the banner and the
    /// self-tests, which is what the log is for) and stops when full rather than evicting the head.
    fn append(&mut self, bytes: &[u8]) {
        let room = RING_CAP - self.len;
        if room == 0 {
            self.dropped = self.dropped.saturating_add(bytes.len());
            return;
        }
        let n = core::cmp::min(room, bytes.len());
        let at = self.len;
        self.buf[at..at + n].copy_from_slice(&bytes[..n]);
        self.len += n;
        if n < bytes.len() {
            self.dropped = self.dropped.saturating_add(bytes.len() - n);
        }
    }
}

static RING: Mutex<LogRing> = Mutex::new(LogRing {
    buf: [0u8; RING_CAP],
    len: 0,
    dropped: 0,
});

/// SERWIT-2W: `capture` used to format each line into a 256-byte stack buffer and copy that in, which
/// **silently truncated every line longer than 256 bytes** — 264 of the tree's format strings exceed
/// even 240 (see `serial_ring::SLOT_LEN`), so `UNAOS.LOG` was quietly clipping the widest diagnostics
/// it existed to preserve. The buffer is gone: `LogRing` is itself the `fmt::Write` sink now, so a line
/// is formatted STRAIGHT into the ring, whole, with no intermediate width limit and no stack frame at
/// all. The only bound left is the ring's own capacity, which was always counted.
impl fmt::Write for LogRing {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.append(s.as_bytes());
        Ok(())
    }
}

/// SERWIT-2 staging ring for the recorder tap. See [`capture`].
///
/// 64 slots × 240 bytes ≈ 15 KiB of `.bss`, the same depth as the primary wire's and the FTDI mirror's.
static STAGE: crate::serial_ring::LineRing<64, 240> = crate::serial_ring::LineRing::new();

/// Lines staged for the log but not yet copied into the capture ring.
pub fn staged_in_flight() -> u64 {
    STAGE.in_flight()
}

/// Additive hook on the serial print seam. Append the formatted line's bytes to the ring. Zero
/// behaviour change to what is printed; alloc-free; `try_lock` only, so it is safe from IRQ-masked
/// print contexts. Never takes the xHCI lock and never allocates.
///
/// **SERWIT-2 — what changed, and what deliberately did NOT.** The contention branch used to be
/// nothing: the comment called this "a diagnostic ring, not a ledger" and reasoned that contention was
/// rare because the serial lock serialises prints. That reasoning was wrong in one specific way — this
/// tap runs OUTSIDE the serial lock and outside the mask (`arch/x86_64/serial.rs` calls it after the
/// locked region), so every core arrives here at once and contention is not rare at all, it is the
/// normal state under a multi-core burst. `UNAOS.LOG` is the whole log a consumer of the shareable
/// `vm-image` will ever have; a hole in it is indistinguishable from a boot that never printed.
///
/// So a contended line is now DEFERRED into the lock-free [`STAGE`] ring and folded in by the next
/// holder, in order. **No I/O is introduced anywhere near this path** — that was the risk worth naming,
/// because `service()` below does real FAT block writes. The drain is a `copy_from_slice` into a static
/// byte array and nothing else: no mount, no allocation, no block device, no `hlt`. The recorder's I/O
/// stays exactly where it was, in the IF=1 main loop. Overflow of the byte ring itself is unchanged and
/// still counted into `dropped`, which `snapshot` already writes into the file.
pub fn capture(args: fmt::Arguments) {
    let tap = &crate::serial_ring::TAP_FLIGHTREC;
    tap.submit();
    if let Some(mut ring) = RING.try_lock() {
        drain_staged(&mut ring);
        let _ = fmt::write(&mut *ring, args);
        tap.absorb();
        return;
    }
    match STAGE.stage(args) {
        crate::serial_ring::Staged::Whole => {
            tap.note_staged();
            return;
        }
        crate::serial_ring::Staged::Truncated => {
            tap.note_staged();
            tap.tear();
            return;
        }
        crate::serial_ring::Staged::Full => {}
    }
    // Staging ring full: one free retry at the sink before the line is declared lost.
    if let Some(mut ring) = RING.try_lock() {
        drain_staged(&mut ring);
        let _ = fmt::write(&mut *ring, args);
        tap.absorb();
        return;
    }
    tap.drop_line();
}

/// Fold every staged line into the byte ring, in order. **Caller must hold `RING`.** Pure memcpy — see
/// the I/O note in [`capture`].
fn drain_staged(ring: &mut LogRing) {
    let n = STAGE.drain(|s| ring.append(s.as_bytes()));
    crate::serial_ring::TAP_FLIGHTREC.absorb_n(n);
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

    // SERWIT-2: poll the mirror-tap ledgers and put any un-announced loss on the wire, plus the
    // one-shot verdict. This lives HERE, ahead of the storage gate, for a plain reason: the
    // announcement must run from an IF=1, unlocked, NON-PRINT context (announcing from inside a tap
    // would recurse through `_print`), and `service()` is the only such site on the x86 main loop that
    // this arc's lane owns. It is a few relaxed atomic loads on the idle path and prints nothing at all
    // when no tap has lost anything. It must precede the `block::info()` early return, or a machine
    // with no storage would never announce.
    crate::serial_ring::mirror_service();

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
