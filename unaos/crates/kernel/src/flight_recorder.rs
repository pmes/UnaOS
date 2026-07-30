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
//!     public `fat.rs` entry points the shell's `write` command uses (call-never-edit; creating a file
//!     in an existing FAT volume is not a format change). Write-through and bounded (a stalled USB
//!     write is `FatError::Io`, never a hang). Failures are honest-and-silent: one witness line, never
//!     a panic, never a block on boot. Re-flushes when the ring has grown since the last successful
//!     write, rate-limited so late lines still reach the disk before a hard power-off.
//!
//! # SINGLE FAT WRITER (2026-07-26) — why the flush RESERVES the file once and then writes IN PLACE
//!
//! The flush used to re-create `UNAOS.LOG` on every pass: `delete_located` (free ~32 clusters) +
//! `create_in_root` (a root-directory-sector RMW) + `write_grow` (re-allocate + re-chain those
//! clusters). That made the BSP main loop a SECOND, unsynchronized FAT/directory mutator running
//! concurrently with the demo chain's writers on the AP cores (the U10/U10c/U10d op drains, the shell,
//! the storage service task). `fs/fat.rs`'s `with_fat_lock` / `with_dir_lock` are deliberately INERT on
//! x86 (masking IRQs across the `hlt`-driven xHCI BOT pump would hang the core — see their doc
//! comments), so nothing serialized the two: A/B-proven cross-linked chains (`GROW.BIN` chain length
//! 5/6 where 2 was expected) and delete-witness first-free snapshots stolen mid-verdict; recorder
//! stubbed out -> 0/3 FAIL, recorder on -> 3/3 FAIL.
//!
//! The fix removes the recorder from the set of FAT mutators entirely, instead of trying to serialize
//! two of them:
//!
//!   1. **RESERVE, once.** On the FIRST main-loop pass where a block device is present, the recorder
//!      makes `UNAOS.LOG` exist at a FIXED `RESERVE_BYTES` size (create + one `write_grow` of zeros,
//!      or reuse an already-large-enough entry from a previous boot untouched). This is the recorder's
//!      ONLY lifetime FAT/directory mutation, and it is exclusive BY CONSTRUCTION, not by luck: it runs
//!      on the BSP, in the main loop, at a call site that PRECEDES every fixture/launcher spawn in the
//!      same iteration (`main.rs`: `flight_recorder::service()` sits above `u2_probe_once()` and the
//!      whole `U*_probe_once` chain at both loop sites), and every other x86 FAT writer is gated on the
//!      same `block::info()` that has only just become `Some`. No other writer can exist yet.
//!   2. **Write IN PLACE, forever after.** Every later flush is a single `FatFs::write_at` over the
//!      reserved chain: strictly bounded to clusters already in the file, never allocating or freeing a
//!      cluster, never writing a FAT entry, never touching a directory sector (`fat.rs:1225` contract).
//!      It therefore cannot interact with any other writer at all — not the demo chain, not the shell,
//!      not the storage service task — under EITHER knob state.
//!
//! This is deliberately NOT "route the recorder through the `irqstorage` storage service task": that
//! would only remove the race when `irqstorage` is ON, and the demo-chain writers (the `witness`-gated
//! U10x drains) are a real second writer with the knob OFF too — which is precisely the configuration
//! of the gate that was failing 3/3 (`UNAOS_HUBSTORAGE=1 ./arroyo test-fat sf 300`). The reserve-then-
//! write-in-place shape is knob-agnostic and strictly stronger: after bootstrap the recorder is not a
//! FAT writer at all, so there is nothing left to serialize.
//!
//! Cost of the shape: `UNAOS.LOG` is always `RESERVE_BYTES` on disk, with the captured log as its
//! prefix, an explicit end-of-log marker line, and zero padding after it. The reserve is one bounded
//! 64 KiB zero-fill at boot (the same order as the flush the old code did every 4096 iterations).

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicUsize, Ordering};
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
    let mut out = alloc::vec::Vec::with_capacity(ring.len + 256);
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
    // An explicit end-of-log marker. The file is a FIXED-SIZE reservation (see the module doc), so the
    // bytes after this line are zero padding, not log — say so in the file rather than leaving a reader
    // to guess where the log stops.
    let end = alloc::format!(
        "\n:: FLIGHTREC: end of log ({} captured byte(s); the remainder of this {}-byte file is reserved padding) ::\n",
        ring.len,
        RESERVE_BYTES
    );
    out.extend_from_slice(end.as_bytes());
    Some((out, ring.len))
}

const LOG_NAME: &str = "UNAOS.LOG";
/// Re-flush throttle: only re-write the log every N main-loop iterations even when it has grown, so a
/// steadily-printing kernel does not churn the FAT. Boot is bounded, so this mostly bounds the tail.
const FLUSH_EVERY_ITERS: u32 = 4096;

/// The FIXED on-disk size `UNAOS.LOG` is reserved at. Must hold the self-identifying header + the whole
/// `RING_CAP` ring + the `dropped` note + the end-of-log marker, so a full ring never needs the file to
/// grow — a grow would be a FAT mutation, and the whole point of the reservation is that the recorder
/// performs exactly ONE of those, at boot, when it is provably the only writer. 512 bytes of slack over
/// `RING_CAP` covers the three fixed-form trailer/header lines with room to spare.
const RESERVE_BYTES: usize = RING_CAP + 512;

/// Bootstrap state for the reservation. `0` = not attempted, `1` = reserved (`LOG_FIRST`/`LOG_SIZE` are
/// live and flushes may write in place), `2` = permanently failed (no FAT volume, or an I/O/space error
/// — never retried, so a broken volume is never churned).
static RESERVED: AtomicU8 = AtomicU8::new(0);
/// The reserved file's chain head + on-disk size, published by the bootstrap and read by every flush.
/// Both are stable for the boot: `write_at` never changes either.
static LOG_FIRST: AtomicU32 = AtomicU32::new(0);
static LOG_SIZE: AtomicU32 = AtomicU32::new(0);
/// Whether the next flush is the FIRST one over the reservation AND that reservation REUSED a file
/// from a previous boot, so the first flush must pad out to `RESERVE_BYTES` to clear the stale tail.
/// Later flushes write only their (monotonically longer) prefix, which overwrites the previous
/// flush's end marker.
///
/// FRWRITE (2026-07-26): this used to default to `true`, so EVERY boot's first flush wrote the whole
/// `RESERVE_BYTES` — including the boots where `reserve_log` had *just* zero-filled the entire file
/// itself. On x86 a block write is one 512-byte WRITE(10) (`block.rs:277` passes `blocks = 1` into a
/// 512-byte `scsi_data_buffer`), and `fat.rs::write_at` read-modify-writes every touched sector, so
/// that redundant pad cost ~129 READ(10)s + ~129 WRITE(10)s — a doubling of the recorder's boot I/O
/// spent overwriting known zeros with zeros. The pad is only ever needed in the ONE case that leaves
/// a stale tail: reusing an already-large-enough file. `reserve_log` now says which case it took.
static PAD_NEXT: AtomicBool = AtomicBool::new(false);

/// The recorder's ONE lifetime FAT/directory mutation: make `/UNAOS.LOG` exist at exactly
/// `RESERVE_BYTES`. Returns `(first_cluster, on-disk size, reused)`, where `reused` is true only in
/// the first case below — the one that leaves a PREVIOUS boot's bytes on disk and therefore obliges
/// the first flush to pad (see `PAD_NEXT`). In the other two cases this call has just zero-filled the
/// whole file, so padding it again would be ~258 redundant single-sector BOT transactions.
///
/// Three cases:
///   * already present and already big enough (a previous boot of the same image) — reuse it UNTOUCHED,
///     zero mutation at all;
///   * present but too small / chainless (an old short log) — `delete_located` + `create_in_root` +
///     one `write_grow` of zeros;
///   * absent — `create_in_root` + one `write_grow` of zeros.
///
/// A directory under the name is a permanent failure (`IsDirectory`) — never delete it.
fn reserve_log() -> Result<(u32, u32, bool), crate::fs::fat::FatError> {
    use crate::fs::fat::{self, FatError};
    let fs = fat::mount()?;
    let (dir_lba, dir_off) = match fs.find_located(LOG_NAME) {
        Ok((de, _dl, _doff)) if de.is_dir => return Err(FatError::IsDirectory),
        Ok((de, _dl, _doff))
            if de.size as usize >= RESERVE_BYTES && de.first_cluster() >= 2 =>
        {
            // Big enough already: reuse the existing chain in place. NO FAT/dir mutation whatsoever.
            return Ok((de.first_cluster(), de.size, true));
        }
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
    // One bounded zero-fill grow (alloc + chain + zero + publish the size to the directory LAST). The
    // heap buffer is fine here: the flush half runs at IF=1 from the main loop.
    let zeros = alloc::vec![0u8; RESERVE_BYTES];
    let (_written, new_size, first) = fs.write_grow(0, 0, dir_lba, dir_off, 0, &zeros)?;
    if first < 2 || (new_size as usize) < RESERVE_BYTES {
        return Err(FatError::NoSpace); // short reservation — refuse to write in place over it
    }
    Ok((first, new_size, false))
}

/// Write `data` to the reserved `/UNAOS.LOG` chain, IN PLACE. `write_at` is strictly bounded (clusters
/// already in the chain only; no FAT entry written, no directory sector touched — `fat.rs:1225`), so
/// this is NOT a FAT mutation and cannot race any other writer. Returns the bytes written.
fn write_log(data: &[u8]) -> Result<usize, crate::fs::fat::FatError> {
    let first = LOG_FIRST.load(Ordering::Acquire);
    let size = LOG_SIZE.load(Ordering::Acquire);
    let fs = crate::fs::fat::mount()?; // read-only: re-reads the BPB, mutates nothing
    if data.is_empty() {
        return Ok(0);
    }
    fs.write_at(first, size, 0, data)
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

    // SINGLE FAT WRITER: reserve the file on the FIRST pass storage is present — before any fixture
    // launcher / storage service submitter can exist (this call site precedes every `U*_probe_once` in
    // the same main-loop iteration, and they gate on the same `block::info()`). This is the recorder's
    // only FAT/directory mutation for the whole boot.
    if RESERVED.load(Ordering::Acquire) == 0 {
        match reserve_log() {
            Ok((first, size, reused)) => {
                LOG_FIRST.store(first, Ordering::Release);
                LOG_SIZE.store(size, Ordering::Release);
                // FRWRITE: pad the first flush ONLY when we reused a previous boot's file — the
                // create/grow paths just zero-filled it, so padding would rewrite known zeros at a
                // cost of ~129 READ(10) + ~129 WRITE(10) single-sector BOT transactions.
                PAD_NEXT.store(reused, Ordering::Relaxed);
                RESERVED.store(1, Ordering::Release);
                serial_println!(
                    ":: FR: {} reserved {} bytes @cluster {} reused={} — flushes are write-in-place only (single FAT writer preserved) ::",
                    LOG_NAME,
                    size,
                    first,
                    reused
                );
            }
            Err(e) => {
                RESERVED.store(2, Ordering::Release);
                use crate::fs::fat::FatError;
                match e {
                    // No FAT boot volume here (a raw/non-FAT stick, or no disk). The NORMAL case for the
                    // default `test` (raw usb.img) — nowhere to record to, not an error. Silent.
                    FatError::NotFat | FatError::NoDisk => {}
                    _ => serial_println!(
                        ":: FR: {} reservation failed ({:?}) — boot log not recorded to disk ::",
                        LOG_NAME,
                        e
                    ),
                }
            }
        }
    }
    if RESERVED.load(Ordering::Acquire) != 1 {
        return; // no reservation (no FAT volume, or a permanent failure) — never write
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
    let (mut data, len) = match snapshot() {
        Some(pair) => pair,
        None => return, // contended at the snapshot moment — retry next iteration, marker unchanged
    };
    // The log can never exceed the reservation (RESERVE_BYTES = RING_CAP + slack), but clamp rather
    // than trust the arithmetic: `write_at` would silently short-write anyway, and a truncated tail is
    // better than a surprise.
    data.truncate(RESERVE_BYTES);
    if PAD_NEXT.swap(false, Ordering::Relaxed) {
        // FIRST flush over the reservation: pad to the full file so a REUSED file's stale tail from a
        // previous boot is cleared. Data sectors only — still not a FAT mutation.
        data.resize(RESERVE_BYTES, 0);
    }
    match write_log(&data) {
        Ok(written) => {
            LAST_FLUSHED.store(len, Ordering::Relaxed);
            // BPACE: the first SUCCESSFUL flush — the boot's first sustained WRITE workload, and
            // (with `PAD_NEXT` on a reused file) by far its largest. `d=` from `fat-mount` is what
            // the write path costs; the flushes that follow are throttled and re-flush on growth,
            // so only this first one belongs in a boot ledger.
            {
                static FR_FIRST: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                crate::bootpace::record_once(&FR_FIRST, "fr-flush");
            }
            if !ANNOUNCED.swap(true, Ordering::Relaxed) {
                serial_println!(
                    ":: FLIGHTREC: boot log -> {} ({} captured bytes into a {}-byte in-place write) -> PASS ::",
                    LOG_NAME,
                    len,
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
