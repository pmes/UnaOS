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
//!      or reuse an already-large-enough entry from a previous boot untouched, then stamp its head in
//!      place so a reader can tell which boot claimed it — FRSTAMP, see `boot_stamp`). This is the recorder's
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
//! `RESERVE_BYTES` (256 KiB + slack) zero-fill at boot — bigger than the old per-4096-iteration
//! flush, but still one bounded write on the reserve pass only.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use spin::Mutex;

/// Ring capacity. 64 KiB held "a full vm-image boot" when that claim was written, but the FTDI
/// module's own measured bench boots record 65 731 pre-console bytes on the SAME stream — and this
/// ring drops NEWEST when full, so the tail of the boot (the kepler block, the GPACE/BPACE
/// ledgers) is precisely what a too-small ring loses. 256 KiB matches the FTDI capture ring
/// (GR15) and absorbs the `logts` prefix overhead (+12 bytes/line). A fixed static (BSS) — no heap.
const RING_CAP: usize = 256 * 1024;

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
        let _ = ring_write(&mut ring, args);
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
        let _ = ring_write(&mut ring, args);
        tap.absorb();
        return;
    }
    tap.drop_line();
}

/// This sink's line-start flag — mutated only while `RING` is held (by the prefixing writer and by
/// [`drain_staged`]'s bare-byte correction).
#[cfg(feature = "logts")]
static LINE_START: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// CLOCK-2b: the direct (lock-held) write into the boot-log ring, timestamp-prefixed under `logts`
/// so `UNAOS.LOG` self-dates the same way the FTDI capture does — this file is the whole log a
/// `vm-image` consumer will ever have. Staged lines are folded in bare by [`drain_staged`] on
/// purpose: a prefix rendered at drain time would stamp the drain, not the emission.
fn ring_write(ring: &mut LogRing, args: fmt::Arguments) -> fmt::Result {
    #[cfg(feature = "logts")]
    {
        use core::fmt::Write;
        crate::logts::TapPrefixWriter { inner: ring, state: &LINE_START }.write_fmt(args)
    }
    #[cfg(not(feature = "logts"))]
    fmt::write(ring, args)
}

/// Fold every staged line into the byte ring, in order. **Caller must hold `RING`.** Pure memcpy — see
/// the I/O note in [`capture`].
///
/// CLOCK-2b: staged bytes bypass the prefixing writer (deliberately — see [`ring_write`]), so the
/// drainer re-trues the sink's line-start flag from the last byte it pushed; a staged fragment
/// without a trailing `\n` would otherwise leave the flag claiming line-start and the next prefix
/// would land mid-line.
fn drain_staged(ring: &mut LogRing) {
    #[cfg(feature = "logts")]
    let mut last_byte: Option<u8> = None;
    let n = STAGE.drain(|s| {
        ring.append(s.as_bytes());
        #[cfg(feature = "logts")]
        {
            last_byte = s.as_bytes().last().copied().or(last_byte);
        }
    });
    crate::serial_ring::TAP_FLIGHTREC.absorb_n(n);
    #[cfg(feature = "logts")]
    if let Some(b) = last_byte {
        LINE_START.store(b == b'\n', core::sync::atomic::Ordering::Relaxed);
    }
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
    // FRSTAMP: the boot-identity line is FIRST, at offset 0, so the flush's own bytes supersede the
    // `state=reserved` stamp this boot wrote at reservation time (see `boot_stamp`). Everything below
    // it is this boot's capture.
    out.extend_from_slice(boot_stamp(REUSED.load(Ordering::Relaxed), true).as_bytes());
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
/// `RING_CAP` covers the four fixed-form header/trailer lines (the FRSTAMP boot-identity line, the
/// self-identifying header, the `dropped` note and the end-of-log marker — under ~360 bytes even with
/// 20-digit counter values) with room to spare.
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
/// The same `reused` verdict, kept for the whole boot so the FRSTAMP line can carry it. `PAD_NEXT`
/// cannot serve: it is consumed (swapped false) by the first flush, which is exactly the flush whose
/// header has to say which case the reservation took.
static REUSED: AtomicBool = AtomicBool::new(false);

// ===================== FRVOL (GR21) — the recorder's volume is latched at reserve =====================
//
// `LOG_FIRST` / `LOG_SIZE` name a cluster chain and a byte count. They do not name a DISK. Every flush
// re-derives the disk from `fat::mount()`, which is hardcoded to `BlockSource::Default` (`fs/fat.rs`),
// i.e. whatever device occupies the global `BLOCK_DEVICE` slot at that instant — and on x86 that slot
// can change occupant while the kernel runs, because `publish_usb_geometry` claims it on every USB
// storage enumeration including a hot-plug. So "cluster 2, 262 656 bytes" can be resolved against one
// medium at reservation time and a DIFFERENT medium one flush later, with every offset still in range
// and every write reporting PASS. That is not a hypothetical: Boot AI-2 wrote the recorder's file onto
// a card the operator had inserted to read.
//
// The block-layer guard (`block::default_writable`, FRGUARD) refuses the WRITE in the case it can see.
// This latch is the recorder's own half and it is independent of it: it binds the reservation to the
// identity of the volume it ran on, and every later flush must land on THAT volume or be refused. Two
// identities are kept because each closes what the other cannot:
//
//   * the FAT volume fingerprint `(BS_VolID, count_of_clusters)` — `FatFs::volume_fingerprint`, the same
//     pair the aarch64 UNAFS.ATR ACL store binds to. It survives a re-enumeration of the same physical
//     card (a re-plug changes the xHCI slot id but not the serial), so a legitimate replug is not a
//     refusal;
//   * the block device geometry `num_blocks` — it catches a second volume that happens to carry the
//     same serial (a byte-for-byte image written to two differently sized cards), which the fingerprint
//     alone cannot tell apart.
//
// Both are captured inside `reserve_log`, from the very mount the reservation mutates, so there is no
// window between "which volume did we reserve on" and "which volume did we record". A mismatch is a
// REFUSAL with a witness naming both — never a silent retarget, and never a write.
static LOG_VOL_LATCHED: AtomicBool = AtomicBool::new(false);
static LOG_VOL_ID: AtomicU32 = AtomicU32::new(0);
static LOG_VOL_CLUSTERS: AtomicU32 = AtomicU32::new(0);
static LOG_VOL_BLOCKS: AtomicU64 = AtomicU64::new(0);
/// One-shot latch for the mismatch witness, so a throttled-but-repeating flush names it once.
static LOG_VOL_REFUSED: AtomicBool = AtomicBool::new(false);

/// FRVOL: bind the recorder to `fs` — the volume the reservation is about to mutate. Called from
/// `reserve_log` immediately after its mount succeeds and BEFORE any directory or FAT byte is touched,
/// so even a reservation that fails half way has already recorded which medium it was working on.
fn latch_volume(fs: &crate::fs::fat::FatFs) {
    let (vol_id, clusters) = fs.volume_fingerprint();
    LOG_VOL_ID.store(vol_id, Ordering::Relaxed);
    LOG_VOL_CLUSTERS.store(clusters, Ordering::Relaxed);
    LOG_VOL_BLOCKS.store(
        crate::drivers::block::info().map(|d| d.num_blocks).unwrap_or(0),
        Ordering::Relaxed,
    );
    LOG_VOL_LATCHED.store(true, Ordering::Release);
}

/// FRVOL: is `fs` the volume the reservation ran on? `true` when nothing is latched yet (the reserve
/// pass itself, which is what does the latching) so the check adds no ordering requirement of its own.
/// Refusing on a mismatch is the whole point: the alternative is writing this boot's log over 262 656
/// bytes of somebody else's medium at offsets that are all perfectly in range.
fn volume_matches(fs: &crate::fs::fat::FatFs) -> bool {
    if !LOG_VOL_LATCHED.load(Ordering::Acquire) {
        return true;
    }
    let (vol_id, clusters) = fs.volume_fingerprint();
    let blocks = crate::drivers::block::info().map(|d| d.num_blocks).unwrap_or(0);
    vol_id == LOG_VOL_ID.load(Ordering::Relaxed)
        && clusters == LOG_VOL_CLUSTERS.load(Ordering::Relaxed)
        && blocks == LOG_VOL_BLOCKS.load(Ordering::Relaxed)
}

/// FRVOL: the refusal witness. Names BOTH volumes, because "the recorder refused" is only falsifiable
/// if a reader can see which medium it was reserved on and which one `Default` resolves to now.
fn refuse_foreign_volume(fs: &crate::fs::fat::FatFs) -> crate::fs::fat::FatError {
    if !LOG_VOL_REFUSED.swap(true, Ordering::Relaxed) {
        let (vol_id, clusters) = fs.volume_fingerprint();
        let blocks = crate::drivers::block::info().map(|d| d.num_blocks).unwrap_or(0);
        serial_println!(
            ":: FR: flush REFUSED — {} was reserved on volume id={:#010x} clusters={} blocks={}, but \
             BlockSource::Default now resolves to volume id={:#010x} clusters={} blocks={}; the boot \
             volume changed under the recorder and it does not follow (first, once) ::",
            LOG_NAME,
            LOG_VOL_ID.load(Ordering::Relaxed),
            LOG_VOL_CLUSTERS.load(Ordering::Relaxed),
            LOG_VOL_BLOCKS.load(Ordering::Relaxed),
            vol_id,
            clusters,
            blocks
        );
    }
    crate::fs::fat::FatError::Unsupported
}

/// FRSTAMP — the file's FIRST line: which boot owns these bytes.
///
/// The stale-log trap this closes: `reserve_log`'s reuse case leaves a previous boot's `UNAOS.LOG`
/// byte-identical on disk, and a boot that dies before its first flush therefore hands the reader a
/// complete, plausible, WRONG log with no way to tell — the reader was left cross-matching `hz=` out
/// of the body by hand. So the reservation itself now stamps the file, and the stamp names the state
/// it was written in:
///
///   * `state=reserved` — written the instant the file was claimed, before any capture reached it.
///     The bytes BELOW it are not this boot's log (in the reuse case they are the previous boot's).
///   * `state=flushed` — written by [`snapshot`] as the first line of a real flush. The bytes below
///     it are this boot's capture.
///
/// The guarantee begins AT RESERVATION, not at power-on: a boot that dies before storage comes up
/// never reaches `service()`'s reserve pass and leaves the previous boot's file wholly intact —
/// including its `state=flushed` stamp, which then truthfully describes a PREVIOUS boot. A file-only
/// reader cannot detect that window from the file alone; cross-matching `hz=`/body content against
/// an independent record (the FTDI capture) remains the check for it. What the stamp closes is the
/// stale-file trap for every boot that got as far as claiming the file.
///
/// The boot identity is `hz` (the ledger's counter rate, from `bootpace::origin_hz`) plus `cy` (the
/// raw free-running counter at the moment of the stamp — monotonic within a boot, unrelated across
/// boots). `hz=0` means the timebase was not calibrated yet; it is printed as 0 and `cy` stays raw
/// ticks, because a fabricated millisecond here is exactly the kind of number a later reader would
/// trust. Two boots of the same image differ in `cy` even when `hz` is identical.
fn boot_stamp(reused: bool, flushed: bool) -> alloc::string::String {
    alloc::format!(
        ":: FR-BOOT: hz={} cy={} reused={} state={} ::\n",
        crate::bootpace::origin_hz(),
        crate::arch::now_cycles(),
        reused,
        if flushed { "flushed" } else { "reserved" }
    )
}

/// Write the `state=reserved` stamp over the head of the freshly reserved file.
///
/// IN PLACE (`write_at` over the already-reserved chain), so this is NOT a FAT mutation and the
/// single-FAT-writer invariant of the module doc is untouched — the reservation remains the
/// recorder's only lifetime FAT/directory mutation. Cost is the one sector the stamp lands in.
///
/// In the reuse case the stamp is followed by a note line and NOTHING ELSE: the previous boot's
/// remaining bytes are deliberately left in place rather than zeroed. If this boot never flushes,
/// that older log is the only evidence on the volume, and the two lines above it are enough to stop
/// a reader from mistaking it for this boot's.
fn stamp_reservation(reused: bool) -> Result<usize, crate::fs::fat::FatError> {
    let mut head = boot_stamp(reused, false);
    if reused {
        head.push_str(
            ":: FR: the bytes below are a PREVIOUS boot's log (its head overwritten by this stamp) until this boot's first flush replaces them ::\n",
        );
    }
    write_log(head.as_bytes())
}

/// The recorder's ONE lifetime FAT/directory mutation: make `/UNAOS.LOG` exist at exactly
/// `RESERVE_BYTES`. Returns `(first_cluster, on-disk size, reused)`, where `reused` is true only in
/// the first case below — the one that leaves a PREVIOUS boot's bytes on disk and therefore obliges
/// the first flush to pad (see `PAD_NEXT`). In the other two cases this call has just zero-filled the
/// whole file, so padding it again would be ~1026 redundant single-sector BOT transactions.
///
/// Three cases:
///   * already present and already big enough (a previous boot of the same image) — reuse it UNTOUCHED,
///     zero mutation at all (the caller then stamps its head in place; see [`stamp_reservation`]);
///   * present but too small / chainless (an old short log) — `delete_located` + `create_in_root` +
///     one `write_grow` of zeros;
///   * absent — `create_in_root` + one `write_grow` of zeros.
///
/// A directory under the name is a permanent failure (`IsDirectory`) — never delete it.
fn reserve_log() -> Result<(u32, u32, bool), crate::fs::fat::FatError> {
    use crate::fs::fat::{self, FatError};
    let fs = fat::mount()?;
    // FRVOL (GR21): bind the recorder to THIS volume before the first directory byte moves. Every
    // later flush re-derives its disk from `BlockSource::Default`, which a hot-plug can re-point.
    latch_volume(&fs);
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
    // FRVOL (GR21): the mount above resolved `BlockSource::Default` AGAIN. If the global slot changed
    // occupant since the reservation, `first`/`size` now address a stranger's clusters — refuse.
    if !volume_matches(&fs) {
        return Err(refuse_foreign_volume(&fs));
    }
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

    // FRGUARD (GR21): do not even ATTEMPT a reservation on a volume the block layer will refuse to
    // write. The refusal would surface anyway — every mutating path below ends at `write_block` /
    // `write_blocks`, which fail closed — but it would surface LATE and in the wrong voice: the
    // reuse case (an existing, large-enough `UNAOS.LOG` already on the foreign medium) mutates
    // nothing, so `reserve_log` succeeds and the boot prints `reserved … stamped=false`, a line that
    // reads like a success against a card the recorder must not touch. Asking first turns that into
    // one honest sentence. Latched to state 2 (permanent): a substitution is not a transient.
    // Byte-inert wherever `default_writable()` is the constant `true` — every x86 build without
    // `sdhcblk`, and every QEMU-virt aarch64 build.
    if !crate::drivers::block::default_writable()
        && RESERVED
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    {
        serial_println!(
            ":: FR: {} NOT reserved — the block layer refuses Default WRITEs here (the canonical \
             volume is not the one in the global slot); this boot's log stays in RAM ::",
            LOG_NAME
        );
    }

    // SINGLE FAT WRITER: reserve the file on the FIRST pass storage is present — before any fixture
    // launcher / storage service submitter can exist (this call site precedes every `U*_probe_once` in
    // the same main-loop iteration, and they gate on the same `block::info()`). This is the recorder's
    // only FAT/directory mutation for the whole boot.
    // The claim is a CAS, not load-then-store: `service()` has three call sites (two BSP loop
    // shapes and the SCHED-X86 `x86_usb_pump` task), and a load/store gate would let a second
    // caller pass `!= 1` below and flush `state=flushed` over offset 0 while the first was still
    // writing `state=reserved` there (GR16 review). The winner holds the in-progress state (3)
    // until the FRSTAMP is on disk, so nobody can flush a file whose claim line is mid-write.
    if RESERVED
        .compare_exchange(0, 3, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        match reserve_log() {
            Ok((first, size, reused)) => {
                LOG_FIRST.store(first, Ordering::Release);
                LOG_SIZE.store(size, Ordering::Release);
                // FRWRITE: pad the first flush ONLY when we reused a previous boot's file — the
                // create/grow paths just zero-filled it, so padding would rewrite known zeros at a
                // cost of ~513 READ(10) + ~513 WRITE(10) single-sector BOT transactions
                // (RESERVE_BYTES / 512).
                PAD_NEXT.store(reused, Ordering::Relaxed);
                REUSED.store(reused, Ordering::Relaxed);
                // FRSTAMP: claim the file for THIS boot now, while it is provably ours, rather than at
                // the first flush — a boot that dies before flushing must not leave a previous boot's
                // log looking fresh. Write-in-place, so still not a FAT mutation.
                let stamp = stamp_reservation(reused).is_ok();
                RESERVED.store(1, Ordering::Release);
                serial_println!(
                    ":: FR: {} reserved {} bytes @cluster {} reused={} stamped={} — flushes are write-in-place only (single FAT writer preserved) ::",
                    LOG_NAME,
                    size,
                    first,
                    reused,
                    stamp
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
