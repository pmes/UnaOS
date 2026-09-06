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

//! PRTSCR — the screen capture: panel pixels to `SCREEN<n>.PNG` at the root of the FAT volume.
//!
//! Two ways in, one mechanism:
//!
//!  * the `screenshot` shell verb — arch-neutral, and the reason the mechanism is testable at all
//!    without a keyboard;
//!  * the **Print Screen key** (HID usage 0x46), which does not type a character and therefore has
//!    no representation in `pal::Event`. Its hook lives at the HID decoders' press edge and does
//!    exactly one thing: [`request`] sets a flag. The capture itself happens in [`service`], on the
//!    device-service pass, for the reason `holocron`'s call site states at length — a filesystem
//!    write issued from inside `service_ehci_hid()` would contend the xHCI storage loan *from
//!    inside the EHCI service pass* and hold the internal keyboard and trackpad hostage for its
//!    whole duration. A screenshot is seconds of work. It does not belong in an input pump.
//!
//! ## The panel is read WITHOUT the panel lock, and that is deliberate
//!
//! `video::mod`'s LOCKFIX rule forbids a bare blocking `WRITER.lock()` from anything preemptible or
//! masked. The sanctioned paint-path door is [`crate::video::panel_snapshot`], which hands back a
//! `FrameBuffer` — a `Copy` HANDLE (base address, length, geometry), not a guard. The lock is
//! released the instant the snapshot returns, and every pixel read afterwards is an ordinary
//! volatile load through that handle. So a capture that takes seconds holds nothing for any of them.
//!
//! What that costs is honest: the compositor may paint between two of our scanlines, so a capture
//! taken while the screen is moving can tear. For a screenshot that is cosmetic, and it is the right
//! trade — the alternative is a ~20 MiB frame copy taken under a lock the whole machine's paint path
//! needs, which is the WEDGE-8 shape this kernel spent three arcs eliminating.
//!
//! ## What the pixels are, and where that is decided
//!
//! [`FrameBuffer::read_pixel`](crate::video::FrameBuffer::read_pixel) is the format authority: it is
//! the documented inverse of `put_pixel` and returns `0x00RRGGBB` for `PixelFormat::Rgb` and
//! `PixelFormat::Bgr` alike, decoding from the `FrameBufferInfo` the firmware reported (UEFI GOP on
//! x86 — BGRx on the rMBP — or the VideoCore mailbox on the Pi). We do NOT assume a byte order; we
//! ask that function, and we refuse a layout it has no colour inverse for (`U8` greyscale averaging
//! is lossy and not invertible) rather than inventing one. PNG then stores plain RGB8 triples.
//!
//! ## Naming
//!
//! `SCREEN0.PNG` .. `SCREEN99.PNG` at the volume root, first free index wins. **An existing capture
//! is never overwritten**: the search asks the filesystem for each candidate and takes the first
//! `NotFound`. When all hundred are taken the verb refuses and says so — it does not wrap around and
//! clobber `SCREEN0.PNG`. The names are deliberately 8.3-clean so they need no long-name entry.
//!
//! ## Where a capture may land — PRTSCR-VOL, the two-rung target ladder
//!
//! A capture wants "a writable FAT volume the operator can carry away", which is NOT the same
//! question `mount_program_source` answers ("the volume this system is bound to"). On a machine
//! whose boot medium is read-only by policy — the 2012 rMBP boots from the internal SD reader,
//! which SDHC-4c mounts read-only outside the reserved flight-recorder extent — the two answers
//! permanently diverge: flight-3 proved that `program_source()` under a `BM_SUBSTITUTED` verdict
//! returns the Sdhc handle on every call, and FRGUARD's `default_writable()` vetoes the global slot
//! under that same verdict, so a capture that only ever consults the program source waits for a
//! writable volume that CANNOT arrive on that bench. [`mount_capture_target`] is the fix:
//!
//!  1. **The program source**, when it admits writes — every boot whose program volume is writable
//!     (QEMU `test-fat`, a stick-booted x86 machine, the Pi's microSD) behaves exactly as before.
//!  2. **The dedicated USB mass-storage handle** (`BlockSource::Usb`), when rung 1 is read-only or
//!     absent. `publish_usb_geometry` populates it on EVERY stick arrival, boot-time or hot-plug
//!     (Boot AI-2 proved hot-plug reaches the FAT layer on metal), and its read/write paths bypass
//!     the backend selector entirely. Crucially this does NOT weaken FRGUARD: the refusal FRGUARD
//!     exists for is a write aimed at the BOOT VOLUME silently landing on whatever claimed the
//!     global slot. This rung aims at the stick BY NAME — the operator's own carry-away medium,
//!     which is exactly where a screenshot belongs — and the global slot's veto stands untouched.
//!
//! ## One capture at a time, and the wire names every state (PRTSCR2)
//!
//! A capture is seconds of work — on the Orin, 1920x1200 encodes and writes 6.9 MB over USB BOT in
//! ~7.9 s (render3b, `[pstrip] gapmax=7894ms`) — and the FAT write is crash-consistent in exactly
//! one direction: `write_grow` publishes the directory entry's size LAST (`fs/fat.rs`, "SAFE
//! ORDER"), so a boot cut mid-write leaves the entry `create_in_dir` made at its original 0 bytes.
//! That 0-byte `SCREEN<n>.PNG` is therefore not a mystery file but the interrupted-write signature,
//! and this module's job is to make sure the wire has already NAMED that file before the entry can
//! exist. Three rules follow:
//!
//!  * **`capture` announces before it commits.** The `-> capturing` line prints after the name is
//!    chosen and before a pixel is read, so every capture the wire sees ends in exactly one of
//!    `-> OK`, a `— capture skipped` refusal, or nothing after `-> capturing` — and the last one
//!    means the boot ended inside the capture, which the operator can then read off the log.
//!  * **One capture at a time.** [`IN_FLIGHT`] is taken at the door of [`capture`] and released on
//!    every exit path. The Print Screen key and the `screenshot` verb reach `capture` from different
//!    tasks (the device-service pass and the console), and `next_free_name` -> `create_in_dir`
//!    cannot de-duplicate across two concurrent callers — both would choose the same free index.
//!    A caller that finds the door taken gets [`Refusal::InFlight`], a named refusal on the wire.
//!  * **A press during a capture is deferred, not dropped.** The xHCI event ring is drained by the
//!    same `drain_event_ring_once` whether `poll_events` or the synchronous BOT pump is running, so
//!    the keyboard's press edge is decoded — and [`request`] runs — from INSIDE the storage write of
//!    the capture already in flight. [`service`] cleared [`PENDING`] before starting that capture,
//!    so the press re-arms it and the next service pass runs the second capture. Should `service`
//!    itself ever meet the door taken (a verb capture on the console task), it re-arms `PENDING`
//!    and says so once; the request is serviced when the door opens.
//!
//! ## PRTSCR-ASYNC — the capture is SLICED, so the machine is never wedged for it (SR2)
//!
//! Everything above was true and the machine still died for the duration. `service()` ran the whole
//! encode-and-write as ONE call from inside the device-service pass, and that pass is also the pass
//! that polls the keyboard and the trackpad: **70 s on the 2012 rMBP** (15.5 MB at ~220 KB/s, with
//! `[deadman] pmp=0` throughout), **6–9 s on the Orin** at 1920x1200. Peter, at the glass on
//! render7: *"3 presses in a row didn't do 3"* — and the census agreed, 8 armed / 7 OK / 1 silent.
//! The deferral above cannot fire for a press the input pump never gets to decode.
//!
//! So the capture is now a **state machine that runs in bounded slices**, and `service()` advances
//! it by one slice per pass:
//!
//!  * [`Job::begin`] does the cheap refusals, chooses the name, prints `-> capturing`, and builds
//!    the encoder. Everything that could refuse still refuses before a pixel is read — and the
//!    volume it settles on is [`mount_capture_target`]'s, the PRTSCR-VOL ladder above, NOT
//!    `mount_program_source`: slicing must not quietly re-narrow the target back to rung 1 and
//!    strand the rMBP's read-only boot medium all over again.
//!  * [`Phase::Encode`] pushes [`SLICE_ROWS`] scanlines at a time into the streaming encoder.
//!  * [`Phase::Write`] writes [`SLICE_WRITE`] bytes per `write_grow`, **in order**, from offset 0
//!    upward, so what is on the medium is always a valid PREFIX of the finished PNG.
//!  * [`Job::slice`] runs those units until [`slice_budget`] cycles have been spent, then returns.
//!    The budget is a fraction of the arch's own `hw_wait_budget()`, so it is a DURATION on every
//!    board (~31 ms on x86, ~37 ms on QEMU virt, ~43 ms on the Pi, ~75 ms on the Orin) without this
//!    module learning any board's clock rate.
//!
//! **No lock is held across a slice.** The open job lives in [`JOB`], a `spin::Mutex<Option<Job>>`
//! that is locked exactly twice per slice — once to move the job out, once to move it back — and
//! never while a pixel is read or a byte written. The FAT/BOT layer takes and releases its own loan
//! inside each `write_grow` exactly as it did for the single big write.
//!
//! **What changed for the operator, precisely.** A press that lands during an open capture prints
//! the named [`Refusal::InFlight`] line (once per episode) **and stays armed**, so it runs as the
//! next capture the moment the open one reaches its verdict: three presses in a row now make three
//! files, with the collapse — when two presses land inside one slice window — named on the wire
//! instead of silent. Progress is `:: PRTSCR: slice n=… bytes=…/… ::`, capped at
//! [`SLICE_LINES_MAX`] lines per capture so a 484-slice rMBP write does not become 484 lines.
//!
//! **The interrupted-write signature moves, and this is the one thing a reader must relearn.**
//! `write_grow` publishes the directory size last *per call*, and there are now many calls, so a
//! boot cut mid-capture no longer leaves `SCREEN<n>.PNG` at 0 bytes — it leaves it at the last
//! slice boundary the volume accepted, a truncated PNG with a valid header and no `IEND`. The
//! `-> capturing` line still states the RESERVED length, so `size < reserved` and a missing verdict
//! line are together the interrupted-write signature. A 0-byte file still means the cut landed
//! before the first slice.
//!
//! **`capture()` itself is still synchronous** — it is the same state machine driven to completion
//! in a loop, so the `screenshot` verb and PRTSCR-ST get byte-identical behaviour and the same wire.
//! Only the Print Screen path, whose driver is the input pump, is sliced.
//!
//! ## The volume may leave while the capture is still writing — PRTSCR-ASYNC/UNPLUG
//!
//! Slicing turns a hypothetical into a routine one: a capture that used to own the machine for one
//! uninterruptible write now spans seconds of passes during which the operator can pull the stick,
//! and rung 2 of the ladder above aims at exactly the medium most likely to be pulled.
//!
//! **What happened before this section existed** (the honest one-sentence answer, and it is not "it
//! faults"): nothing dangled and nothing paniced — `drivers/block.rs`'s USB-UNPLUG retraction clears
//! `USB_BLOCK_DEVICE`, every block entry point re-reads the registry through `info()` / `usb_info()`
//! on EVERY call and geometry-bounds the LBA against that fresh snapshot, so the next `write_grow`
//! failed honestly with `BlockError::NotReady` and PRTSCR reported it as the GENERIC
//! `Refusal::Fat("write", …)` line, which names a FAT errno and never names the disconnection.
//!
//! Two things were wrong with that. The refusal was unreadable — an operator who pulled a stick got
//! a `write failed -EIO` and had to infer the cause — and, worse, the honest failure is only
//! guaranteed while the handle stays EMPTY. A retract followed by a replug (or a different stick on
//! a recycled xHCI slot) refills `USB_BLOCK_DEVICE` with a DIFFERENT disk, and the by-value `FatFs`
//! this job parked between slices still holds the old volume's LBAs. The next `write_grow` would
//! then be geometry-bounds-checked against the new disk and pass — a write through a stale handle,
//! onto a stranger's filesystem. That is the case slicing creates and the one this refuses.
//!
//! **The probe, and which of rung 2's facts it uses.** Both of them, because they answer different
//! halves. [`Job`] records, at `begin`, whether this mount is USB-backed at all, plus
//! `block::usb_publish_gen()` — the generation `publish_usb_geometry` bumps on EVERY arrival. Before
//! each volume-touching step (`create_in_dir`, and every `write_grow`) a USB-backed job requires
//! **`block::usb_info().is_some()`** — the geometry publish is still standing, so the stick did not
//! merely leave — **and the generation to be unchanged** — so no arrival has replaced it. Either
//! test failing is [`Refusal::Vanished`], a named line carrying the byte count reached, and the
//! write is NOT issued: the stale handle is never written through. A capture that is not USB-backed
//! (the Pi's microSD, QEMU's `test-fat` image) skips the probe entirely and costs nothing.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use spin::Mutex;
use unaos_boot_info::PixelFormat;

use crate::fs::fat::{BlockSource, FatError, FatFs};
use crate::video::FrameBuffer;
use crate::video::png::{PngEncoder, PngError};

/// Highest capture index. `SCREEN99.PNG` is 11 characters — still 8.3, still no long-name entry.
const MAX_CAPTURES: u32 = 100;

/// How many times a FAT operation may answer `Busy` before we give up on it.
///
/// `Busy` is the block layer refusing to WAIT for a loan it could not take instantly — under
/// WEDGE-8 that is the fix working, not a fault (`drivers/block.rs`'s note: "a NORMAL, RETRYABLE
/// outcome — not a wedge verdict"; `docs/dev/OS/07_USB_STORAGE/usb_xhci.md` §32.3). So it is
/// retried with bounded patience exactly as `fs::fat`'s own RMW wrappers retry it, and only a
/// budget that actually expires becomes `-EAGAIN` for the operator.
const BUSY_ATTEMPTS: u32 = 64;

/// PRTSCR-ASYNC — how long one slice may run, as a DIVISOR of the arch's hardware-wait budget.
///
/// `arch::hw_wait_budget()` is the one wall-clock quantity both arches already express in
/// `now_cycles()` units — an honest `tsc_hz * 2 s` on x86 once the APIC calibration lands, and a
/// CNTFRQ-derived 2.4 s / 2.78 s / 4.8 s on QEMU virt / the Pi 4 / the Orin. Dividing it is how this
/// module gets a duration on a board whose clock rate it does not know: 2 s / 64 ≈ 31 ms on x86,
/// ~37 ms on virt, ~43 ms on the Pi, ~75 ms on the Orin. All comfortably inside a human's
/// input-latency floor, and all bounded — which is the whole property SR2 asks for.
const SLICE_BUDGET_DIV: u64 = 64;

/// PRTSCR-ASYNC — scanlines encoded between two budget checks. A row is `width` volatile loads and
/// an `extend_from_slice`; 64 of them at 1920 px is ~123k loads, well under a millisecond, so the
/// granularity costs nothing and the budget check does not dominate.
const SLICE_ROWS: u32 = 64;

/// PRTSCR-ASYNC — bytes handed to `write_grow` per call, and therefore the LONGEST uninterruptible
/// span of a capture: a `write_grow` cannot be preempted from outside, so this constant IS the
/// worst-case wedge. 32 KiB is ~145 ms on the rMBP's measured 220 KB/s and ~37 ms on the Orin's
/// 870 KB/s — against 70 s and 7.9 s for the single write it replaces.
///
/// It is not smaller because each call re-walks the file's cluster chain (CHAINGROW gave that walk a
/// FAT-sector cache precisely for windowed writers like this one, so a 15.5 MB file costs ~4 sector
/// reads per call — ~1900 extra reads over the whole capture, ~1–2% of a 70 s write). Halving the
/// slice doubles that overhead to buy 70 ms; this is the knee.
const SLICE_WRITE: usize = 32 * 1024;

/// PRTSCR-ASYNC — how many `slice` progress lines one capture may print. The rMBP's 15.5 MB capture
/// is ~484 slices; the wire is evidence, not a progress bar, so it gets the opening ones (which is
/// where a capture that dies early dies) and the verdict line carries the total.
const SLICE_LINES_MAX: u32 = 8;

/// PRTSCR — a capture has been asked for and not yet performed. Set by [`request`] (from the HID
/// decoders' press edge, where nothing may block), cleared by [`service`] (on the device-service
/// pass, where I/O is legal).
///
/// A plain flag, not a counter: holding Print Screen down through a capture that takes seconds
/// should produce one file, not a queue of them. Extra presses during a capture collapse into the
/// single pending request, and [`REQUESTS`] records how many arrived so the collapse is visible
/// rather than silent.
static PENDING: AtomicBool = AtomicBool::new(false);

/// PRTSCR — total Print Screen press edges seen, whether or not each produced a file. The
/// denominator for [`CAPTURES`]: "the key was pressed and nothing appeared" and "the key never
/// arrived" are different failures and a census that cannot tell them apart is worthless on metal.
static REQUESTS: AtomicU32 = AtomicU32::new(0);

/// PRTSCR — captures that reached a written file.
static CAPTURES: AtomicU32 = AtomicU32::new(0);

/// PRTSCR — capture attempts that ended in a refusal (no volume, read-only, full, I/O).
static REFUSALS: AtomicU32 = AtomicU32::new(0);

/// PRTSCR2 — the capture door: `true` while a [`capture`] is running on ANY task. Taken by
/// compare-exchange at the top of `capture`, released on every exit path (one release site, after
/// the inner body returns). See the module note: two callers past `next_free_name` at once would
/// both take the same free index and `create_in_dir` would make two entries with one name.
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// PRTSCR2 — `service` has already said "deferred: capture in flight" for the request it is
/// holding. One line per deferral episode, not one per 250 ms sweep: cleared whenever a capture
/// reaches a verdict through `service`.
static DEFERRED_SAID: AtomicBool = AtomicBool::new(false);

/// PRTSCR-ASYNC — a sliced capture is OPEN: [`JOB`] holds it (or a task is between two of its
/// slices, holding it on the stack), and [`IN_FLIGHT`] is held on its behalf until it reaches a
/// verdict. Read on the idle path so `service` can stay two relaxed loads when nothing is happening.
static SLICING: AtomicBool = AtomicBool::new(false);

/// PRTSCR-ASYNC — the open capture, parked between slices.
///
/// The lock is taken EXACTLY twice per slice — once to move the job out, once to move it back — and
/// never across a pixel read, an encode, or a write. That is deliberate and is the rule this arc was
/// briefed against: a lock held across a slice would reinvent the wedge one layer down. The moves
/// are moves of a `String`, a `Vec` and a `FatFs`; nothing is copied.
static JOB: Mutex<Option<Job>> = Mutex::new(None);

/// PRTSCR — `(requests, captures, refusals)`.
pub fn census() -> (u32, u32, u32) {
    (
        REQUESTS.load(Ordering::Relaxed),
        CAPTURES.load(Ordering::Relaxed),
        REFUSALS.load(Ordering::Relaxed),
    )
}

/// PRTSCR — **the key hook, and the whole of what runs in the input pump.**
///
/// Called from both HID decoders on the Print Screen press EDGE. One relaxed store and one relaxed
/// increment: no allocation, no lock, no I/O, no print beyond the single witness line the caller
/// emits. Everything a screenshot actually costs happens later, in [`service`].
pub fn request() {
    REQUESTS.fetch_add(1, Ordering::Relaxed);
    PENDING.store(true, Ordering::Relaxed);
}

/// PRTSCR — advance the capture by one bounded slice, opening one first if a request is armed.
/// Call from a device-service pass: task context, interrupts enabled, no driver lock held.
///
/// **PRTSCR-ASYNC: this call is bounded** ([`slice_budget`], ~31–75 ms depending on the board) and
/// returns so its caller can poll the keyboard again. It used to run the whole encode-and-write —
/// 6–9 s on the Orin, 70 s on the rMBP — from inside the pass that services input, which is SR2.
///
/// Costs two relaxed loads per call when idle, which is why it can sit unconditionally beside
/// `fat::probe_once()` at every storage-ready pass this kernel carries.
pub fn service() {
    if !PENDING.load(Ordering::Relaxed) && !SLICING.load(Ordering::Relaxed) {
        return;
    }

    // (a) A capture is already open. Advance it by one slice and hand the machine back.
    if SLICING.load(Ordering::Relaxed) {
        // PRTSCR-ASYNC: a press that landed inside this capture. It is NAMED — once per episode,
        // not once per pass — and left armed, so it becomes the next capture rather than a silent
        // collapse. This is the line that was structurally unreachable before slicing: the input
        // pump could not decode the press at all while the write owned the pass.
        if PENDING.load(Ordering::Relaxed) && !DEFERRED_SAID.swap(true, Ordering::Relaxed) {
            Refusal::InFlight.report();
        }
        let open = { JOB.lock().take() };
        let mut job = match open {
            Some(job) => job,
            // Another task is between two slices of this same capture, holding it on its stack.
            None => return,
        };
        match job.slice() {
            Ok(None) => *JOB.lock() = Some(job),
            Ok(Some(shot)) => finish(Ok(shot)),
            Err(why) => finish(Err(why)),
        }
        return;
    }

    // (b) Nothing open and a request is armed: open one.
    //
    // Clear BEFORE the work, not after: a press that lands mid-capture should arm the NEXT one
    // rather than be swallowed by our own clear. (On the Orin that press is decoded from INSIDE
    // this capture's own storage write — see the module note — and this clear-first order is
    // what makes it a second capture instead of a lost one.)
    PENDING.store(false, Ordering::Relaxed);
    if IN_FLIGHT
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        // PRTSCR2: the door is held by another task's synchronous capture (the `screenshot` verb).
        // Not a refusal of the request — it is re-armed and opens on the first pass after the door
        // opens. Said once per episode so a 7 s verb capture does not print 28 copies of the line.
        PENDING.store(true, Ordering::Relaxed);
        if !DEFERRED_SAID.swap(true, Ordering::Relaxed) {
            Refusal::InFlight.report();
        }
        return;
    }
    match Job::begin() {
        Ok(job) => {
            *JOB.lock() = Some(job);
            SLICING.store(true, Ordering::Release);
        }
        // Every cheap refusal (no panel, no volume, read-only, all names taken, allocator) still
        // arrives here, before a pixel is read — `begin` kept that order, and kept the PRTSCR-VOL
        // ladder that decides which volume is being refused about.
        Err(why) => finish(Err(why)),
    }
}

/// PRTSCR-ASYNC — release the door and print the one verdict line a `-> capturing` is owed.
///
/// The single exit for the sliced path, so `SLICING` and `IN_FLIGHT` cannot be left set by a branch
/// that forgot them — the PRTSCR2 "released on every exit path" rule, now that there are more exits.
fn finish(verdict: Result<Shot, Refusal>) {
    SLICING.store(false, Ordering::Relaxed);
    IN_FLIGHT.store(false, Ordering::Release);
    DEFERRED_SAID.store(false, Ordering::Relaxed);
    match verdict {
        Ok(shot) => {
            CAPTURES.fetch_add(1, Ordering::Relaxed);
            serial_println!(
                ":: PRTSCR: {} {}x{} {} bytes -> OK ::",
                shot.name,
                shot.width,
                shot.height,
                shot.bytes
            );
        }
        Err(why) => {
            REFUSALS.fetch_add(1, Ordering::Relaxed);
            why.report();
        }
    }
}

/// A capture that landed: what was written, where, and how big.
pub struct Shot {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
}

/// Why a capture did not happen. Every variant carries what it inspected, not just what was
/// missing — the WINX-8 refusal discipline.
pub enum Refusal {
    /// No framebuffer attached, or the panel lock was contended while masked.
    NoPanel,
    /// The panel's pixel layout has no colour inverse (`U8` greyscale, or an unknown format).
    NoFormat(PixelFormat),
    /// Nothing mounted on the program-source handle NOR the dedicated USB handle.
    NoVolume(FatError),
    /// The program-source volume mounted and refuses writes — `(source, label, reason)` — and the
    /// USB rung of the ladder had no writable volume to offer either.
    ReadOnly(&'static str, String, &'static str),
    /// `SCREEN0.PNG` .. `SCREEN99.PNG` are all taken. We do not overwrite.
    AllTaken,
    /// The PNG encoder declined, with the geometry it declined for.
    Encode(PngError, u32, u32, usize),
    /// A FAT operation failed: `(what we were doing, the error)`.
    Fat(&'static str, FatError),
    /// The write was accepted but short: `(name, written, wanted)`.
    Short(String, usize, usize),
    /// PRTSCR2 — another task's capture holds the door ([`IN_FLIGHT`]). The verb reports it and
    /// stops; [`service`] re-arms the request and runs it once the door opens.
    InFlight,
    /// PRTSCR-ASYNC/UNPLUG — the USB volume this capture was writing left (or was replaced by a
    /// different disk on a recycled xHCI slot) between two slices: `(name, bytes reached, wanted)`.
    /// The write that would have gone through the now-stale handle was NOT issued.
    Vanished(String, usize, usize),
}

impl Refusal {
    /// One honest serial line naming the reason AND what was inspected. Mirrors the WINX-8 skip
    /// lines: a guard with a `return`, never a panic, never silence.
    pub fn report(&self) {
        match self {
            Refusal::NoPanel => serial_println!(
                ":: PRTSCR: no panel attached (or the panel lock was contended while masked) — capture skipped ::"
            ),
            Refusal::NoFormat(f) => serial_println!(
                ":: PRTSCR: panel layout {:?} has no RGB inverse — capture skipped ::", f
            ),
            Refusal::NoVolume(e) => serial_println!(
                ":: PRTSCR: no FAT volume on the program-source or USB handles ({:?}; handles={}) — capture skipped ::",
                e,
                crate::drivers::block::source_census()
            ),
            Refusal::ReadOnly(source, label, why) => serial_println!(
                ":: PRTSCR: REFUSED READ-ONLY (source={} label={} reason={}) — no writable USB volume attached either — capture skipped ::",
                source,
                if label.is_empty() { "-" } else { label.as_str() },
                why
            ),
            Refusal::AllTaken => serial_println!(
                ":: PRTSCR: SCREEN0.PNG..SCREEN{}.PNG all present at the volume root — capture skipped (nothing overwritten) ::",
                MAX_CAPTURES - 1
            ),
            Refusal::Encode(e, w, h, need) => serial_println!(
                ":: PRTSCR: encoder declined ({:?}) for {}x{} needing {} bytes — capture skipped ::",
                e, w, h, need
            ),
            Refusal::Fat(what, e) => serial_println!(
                ":: PRTSCR: {} failed {} ({:?}; handles={}) — capture skipped ::",
                what,
                fat_errno(*e),
                e,
                crate::drivers::block::source_census()
            ),
            Refusal::Short(name, written, wanted) => serial_println!(
                ":: PRTSCR: {} short write {} of {} bytes — capture INCOMPLETE ::",
                name, written, wanted
            ),
            Refusal::InFlight => serial_println!(
                ":: PRTSCR: refused — capture in flight (another task holds the capture door; a key request is re-armed and runs after it) ::"
            ),
            Refusal::Vanished(name, done, total) => serial_println!(
                ":: PRTSCR: {} — volume vanished mid-capture at {}/{} bytes (usb geometry retracted or a newer publish replaced it; handles={}) — capture ABANDONED, nothing written through the stale handle ::",
                name,
                done,
                total,
                crate::drivers::block::source_census()
            ),
        }
    }

    /// The one-sentence form for a console. The serial line above carries the forensics; the panel
    /// clips at 128-180 columns, so the operator gets the verdict and the capture gets the census
    /// (FATVERB's two-sinks-two-lengths rule).
    pub fn sentence(&self) -> String {
        match self {
            Refusal::NoPanel => String::from("screenshot: no panel attached"),
            Refusal::NoFormat(f) => alloc::format!("screenshot: panel layout {:?} has no RGB inverse", f),
            Refusal::NoVolume(e) => alloc::format!("screenshot: no FAT filesystem ({:?})", e),
            Refusal::ReadOnly(source, _, _) => {
                alloc::format!("screenshot: REFUSED READ-ONLY ({}); plug a writable USB FAT volume", source)
            }
            Refusal::AllTaken => alloc::format!(
                "screenshot: SCREEN0..SCREEN{}.PNG all present — delete one", MAX_CAPTURES - 1
            ),
            Refusal::Encode(e, w, h, need) => {
                alloc::format!("screenshot: encoder declined ({:?}) for {}x{} ({} bytes)", e, w, h, need)
            }
            Refusal::Fat(what, e) => alloc::format!("screenshot: {}: {} ({:?})", what, fat_errno(*e), e),
            Refusal::Short(name, written, wanted) => {
                alloc::format!("screenshot: {}: short write {} of {} bytes", name, written, wanted)
            }
            Refusal::InFlight => {
                String::from("screenshot: a capture is already in flight — retry after its verdict")
            }
            Refusal::Vanished(name, done, total) => alloc::format!(
                "screenshot: {}: volume vanished mid-capture at {}/{} bytes", name, done, total
            ),
        }
    }
}

/// The errno spelling the shell's FAT verbs use, so a PRTSCR line and an `ls` line name the same
/// failure the same way. (`shell::fat_errno` is private to that file; this is the same mapping over
/// the same public enum.)
fn fat_errno(e: FatError) -> &'static str {
    match e {
        FatError::NoDisk => "-ENODEV",
        FatError::Io => "-EIO",
        FatError::NotFat => "-ENOTSUP",
        FatError::Unsupported => "-EINVAL",
        FatError::NotFound => "-ENOENT",
        FatError::IsDirectory => "-EISDIR",
        FatError::BadChain => "-EIO",
        FatError::NoSpace => "-ENOSPC",
        FatError::OutOfVolume => "-EIO",
        FatError::Busy => "-EAGAIN",
    }
}

/// Run a FAT operation, retrying a `Busy` answer with bounded patience.
///
/// `Busy` means the block device was on loan and this call declined to wait for it — a healthy slow
/// transaction, not a failure. `fs::fat`'s own RMW wrappers retry it exactly this way; the budget is
/// the same hardware-handshake budget, so an operation that never gets the loan still terminates and
/// still tells the truth (`-EAGAIN`) instead of spinning.
///
/// `hlt` only while unmasked: halting with interrupts off is the WEDGE-8 death, and the block layer
/// makes the same distinction at its own claim site.
fn busy_retry<R>(mut op: impl FnMut() -> Result<R, FatError>) -> Result<R, FatError> {
    let start = crate::arch::now_cycles();
    let budget = crate::arch::hw_wait_budget();
    for _ in 0..BUSY_ATTEMPTS {
        match op() {
            Err(FatError::Busy) => {}
            other => return other,
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= budget {
            break;
        }
        if !crate::arch::irqs_masked() {
            crate::hlt();
        }
    }
    Err(FatError::Busy)
}

/// PRTSCR-VOL — mount the volume a capture may write, by the two-rung ladder the module note
/// states: the program source when it admits writes, else the dedicated USB mass-storage handle.
///
/// The refusal returned when BOTH rungs decline describes rung 1 — the more informative failure:
/// `ReadOnly` names the vetoing source (and its report adds that no writable USB volume was
/// attached either), `NoVolume` carries the mount error and the handle census. Rung 2 is consulted
/// fresh on every call, which is what makes a LATER-ARRIVING stick reachable: `usb_info()` re-reads
/// the registry, so the pass after `publish_usb_geometry` runs sees the new volume with no cache to
/// invalidate.
fn mount_capture_target() -> Result<FatFs, Refusal> {
    let primary = match crate::fs::fat::mount_program_source() {
        Ok(fs) => match fs.write_veto() {
            None => return Ok(fs),
            Some(why) => Refusal::ReadOnly(fs.source_name(), fs.label(), why),
        },
        Err(e) => Refusal::NoVolume(e),
    };
    // Rung 2: the stick under its OWN handle — never the ambient global, whose FRGUARD veto is not
    // ours to bypass. Gated on the registry so an absent stick costs one lock, not a mount attempt.
    if crate::drivers::block::usb_info().is_some() {
        if let Ok(fs) = crate::fs::fat::mount_source(crate::fs::fat::BlockSource::Usb) {
            // `Usb`'s write_veto is `None` today; asked anyway so this ladder keeps telling the
            // truth if that arm ever grows a refusal.
            if fs.write_veto().is_none() {
                return Ok(fs);
            }
        }
    }
    Err(primary)
}

/// PRTSCR-ASYNC/UNPLUG — does this mount ride the USB stick, and is therefore hot-unpluggable
/// underneath an open sliced capture?
///
/// Two ways it can, and both must be caught, because [`mount_capture_target`]'s two rungs reach the
/// same disk by different names:
///
///  * rung 2 mounted it explicitly — `source_name()` is `BlockSource::Usb`'s;
///  * rung 1 mounted the PROGRAM SOURCE and on x86 that IS the stick, because
///    `publish_usb_geometry` claims the global slot as well as the dedicated one on any target
///    without the aarch64 backend selector. The two handles are the same disk exactly when they
///    carry the same xHCI `slot_id`, which is the comparison `unpublish_usb_geometry` itself uses.
///
/// Deliberately conservative in the other direction: on the Pi the microSD holds the global with
/// `slot_id: 0` while a stick holds the USB handle, the ids differ, and a capture to the card is
/// therefore NOT probed — pulling an unrelated stick must not refuse it.
fn usb_backed(fs: &FatFs) -> bool {
    let name = fs.source_name();
    if name == BlockSource::Usb.name() {
        return true;
    }
    if name == BlockSource::Default.name() {
        return match (crate::drivers::block::info(), crate::drivers::block::usb_info()) {
            (Some(global), Some(usb)) => global.slot_id == usb.slot_id,
            _ => false,
        };
    }
    false
}

/// The first `SCREEN<n>.PNG` the volume root does not already hold.
///
/// Asks the filesystem per candidate rather than scanning a directory listing, because
/// `locate_in_dir` matches on BOTH the 8.3 short name and any long name — a file whose long name
/// differs from its short name would slip past a listing scan and then be duplicated by
/// `create_in_dir`, which does not de-duplicate. Cluster `0` is the root on every FAT kind here.
fn next_free_name(fs: &FatFs) -> Result<String, Refusal> {
    for n in 0..MAX_CAPTURES {
        let name = alloc::format!("SCREEN{}.PNG", n);
        match busy_retry(|| match fs.locate_in_dir(0, &name) {
            Ok(hit) => Ok(Some(hit)),
            Err(FatError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }) {
            Ok(None) => return Ok(name),
            Ok(Some(_)) => continue,
            Err(e) => return Err(Refusal::Fat("root lookup", e)),
        }
    }
    Err(Refusal::AllTaken)
}

/// PRTSCR — **capture the panel and write it to the volume root as a PNG.** Task context only.
///
/// PRTSCR2: the door. One capture at a time on the whole machine — the body is [`capture_inner`],
/// and [`IN_FLIGHT`] is released HERE, after it returns, whichever of its exits it took. A second
/// caller is told [`Refusal::InFlight`] and is never let past `next_free_name`, which is the only
/// point at which two captures could choose one name.
pub fn capture() -> Result<Shot, Refusal> {
    if IN_FLIGHT
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return Err(Refusal::InFlight);
    }
    let verdict = capture_inner();
    IN_FLIGHT.store(false, Ordering::Release);
    verdict
}

/// The capture proper, under the door [`capture`] holds — the SAME state machine [`service`]
/// slices, driven straight to completion here.
///
/// PRTSCR-ASYNC: the synchronous form is kept, and kept as a driver of the sliced machine rather
/// than a second copy of the work, because two callers genuinely want a verdict in hand — the
/// `screenshot` shell verb (which prints a sentence to the console it was typed at) and
/// [`selftest_once`] (which reads the file back and scores it). What they lose is nothing: the wire
/// is identical, and the slice boundaries they run through are the same ones the key path parks at.
fn capture_inner() -> Result<Shot, Refusal> {
    let mut job = Job::begin()?;
    loop {
        if let Some(shot) = job.slice()? {
            return Ok(shot);
        }
    }
}

/// PRTSCR-ASYNC — one slice's worth of budget, in `arch::now_cycles()` units. See
/// [`SLICE_BUDGET_DIV`] for why the arch's hardware-wait budget is the right thing to divide.
fn slice_budget() -> u64 {
    let b = crate::arch::hw_wait_budget() / SLICE_BUDGET_DIV;
    // A calibration that has not happened yet must not produce a zero-length slice that makes no
    // progress per pass: one unit of work always runs, so the floor only bounds the LOOP.
    if b == 0 { 1 } else { b }
}

/// PRTSCR-ASYNC — a capture in progress: everything the next slice needs and nothing it does not.
///
/// `FatFs` is a handful of scalars and `FrameBuffer` is a `Copy` handle whose base is a `usize`
/// precisely so it can live in a static (`framebuffer.rs`'s `unsafe impl Send`), so parking this
/// between passes introduces no new sharing claim: the panel is read exactly as the one-shot capture
/// read it — without the panel lock, through the handle, tearing accepted (see the module note).
struct Job {
    fs: FatFs,
    panel: FrameBuffer,
    /// `SCREEN<n>.PNG`. Moved out into the [`Shot`] at the verdict.
    name: String,
    width: u32,
    height: u32,
    /// The reserved length the `-> capturing` line published. Denominator of the slice witness.
    need: usize,
    /// Slices spent so far — the witness's `n`, and what [`SLICE_LINES_MAX`] caps.
    slices: u32,
    /// PRTSCR-ASYNC/UNPLUG — this mount rides the hot-unpluggable USB stick ([`usb_backed`]), so
    /// the liveness probe applies to it. `false` for the Pi's microSD and QEMU's `test-fat` image,
    /// where the probe would cost a lock per unit and can never fire.
    usb_backed: bool,
    /// PRTSCR-ASYNC/UNPLUG — `block::usb_publish_gen()` as it stood when this job opened. A DIFFERENT
    /// value means an arrival has republished the handle since — a replug, or another disk on a
    /// recycled xHCI slot — and this job's parked `FatFs` addresses a volume that is no longer there.
    vol_gen: u64,
    phase: Phase,
}

/// PRTSCR-ASYNC — which half of the capture the next unit of work belongs to. Strictly sequential:
/// the PNG cannot be written before `finish` patches the IDAT length and appends `IEND`, so the
/// whole encode precedes the whole write. That is also what keeps the on-medium bytes a valid
/// PREFIX of the final file at every slice boundary.
enum Phase {
    /// Reading the panel into the streaming encoder, `y` rows done.
    Encode { enc: PngEncoder, row: Vec<u8>, y: u32 },
    /// Writing the finished bytes to the volume in order, `done` bytes published.
    Write {
        bytes: Vec<u8>,
        done: usize,
        first: u32,
        size: u32,
        dir_lba: u64,
        dir_off: usize,
    },
    /// Transient placeholder while a unit of work owns the phase by value. Never observed by a
    /// caller: every path that takes the phase out puts one back or returns a verdict.
    Spent,
}

/// What one unit of work produced.
enum Step {
    More(Phase),
    Done(Shot),
}

impl Job {
    /// Everything that can refuse, and nothing that takes time — the order [`capture_inner`] used
    /// to run inline. The panel and the volume are settled, the name is chosen and announced, and
    /// the output buffer is reserved, all before a single pixel is read.
    fn begin() -> Result<Job, Refusal> {
        // 1. The panel — through the sanctioned door, and only for the HANDLE. See the module note.
        let panel = crate::video::panel_snapshot().ok_or(Refusal::NoPanel)?;
        if !panel.is_ready() {
            return Err(Refusal::NoPanel);
        }
        let info = panel.info();
        if !matches!(info.pixel_format, PixelFormat::Rgb | PixelFormat::Bgr) {
            return Err(Refusal::NoFormat(info.pixel_format));
        }
        let (width, height) = (info.width as u32, info.height as u32);

        // 2. The volume, by the PRTSCR-VOL ladder (module note), before anything is built. This is
        //    `mount_capture_target`, NOT `mount_program_source`: rung 2 is the whole reason a
        //    read-only-boot-medium bench can capture at all, and a sliced capture must inherit it.
        let fs = mount_capture_target()?;
        // PRTSCR-ASYNC/UNPLUG: the two facts the liveness probe compares against, taken now, while
        // the volume is known good. `usb_publish_gen` is read AFTER the mount so a publish that
        // raced the mount is already reflected — a stale-low generation would refuse a live disk.
        let usb_backed = usb_backed(&fs);
        let vol_gen = crate::drivers::block::usb_publish_gen();

        // 3. A name nothing else owns.
        let name = next_free_name(&fs)?;

        // PRTSCR2: name it on the wire BEFORE it can exist on the medium. From here every exit is
        // one of `-> OK`, a `— capture skipped` refusal, or a boot that ended inside this capture.
        // PRTSCR-ASYNC moved what that last one leaves behind: no longer always a 0-byte entry but
        // an entry SHORTER than the reserved length, because the size is published per slice.
        let need = PngEncoder::encoded_len(width, height).unwrap_or(0);
        serial_println!(
            ":: PRTSCR: {} {}x{} -> capturing ({} bytes reserved; the verdict line follows — a boot cut before it leaves the entry short of that) ::",
            name, width, height, need
        );

        // 4. The encoder. `PngEncoder::new` reserves the whole output up front, so an allocator
        //    refusal arrives here — before any pixel is read — rather than halfway down the screen.
        let enc =
            PngEncoder::new(width, height).map_err(|e| Refusal::Encode(e, width, height, need))?;
        let mut row: Vec<u8> = Vec::new();
        if row.try_reserve_exact(width as usize * 3).is_err() {
            return Err(Refusal::Encode(PngError::OutOfMemory, width, height, need));
        }

        Ok(Job {
            fs,
            panel,
            name,
            width,
            height,
            need,
            slices: 0,
            usb_backed,
            vol_gen,
            phase: Phase::Encode { enc, row, y: 0 },
        })
    }

    /// PRTSCR-ASYNC/UNPLUG — is the volume this job opened still the volume it would be writing?
    ///
    /// Both of rung 2's facts, because they answer different halves of the question: the geometry
    /// publish still standing (`usb_info().is_some()` — the stick did not simply leave) AND the
    /// publish generation unchanged (no arrival has replaced it with a different disk on the same
    /// or a recycled slot). See the module note's UNPLUG section for why presence alone is not
    /// enough: the block layer's own bounds check would pass a stale LBA against a NEW disk.
    ///
    /// Cheap: two atomic loads and, for the first, one uncontended spin lock — per volume-touching
    /// step, of which a capture has a few hundred, against the ~1900 sector reads the same capture
    /// already spends on chain walks.
    fn volume_alive(&self) -> bool {
        if !self.usb_backed {
            return true;
        }
        crate::drivers::block::usb_info().is_some()
            && crate::drivers::block::usb_publish_gen() == self.vol_gen
    }

    /// PRTSCR-ASYNC — run units of work until the slice budget is spent, then hand the machine
    /// back. `Ok(None)` means "more to do"; `Ok(Some(shot))` is the finished capture.
    ///
    /// The budget is checked AFTER a unit, never before: one unit always runs, so a caller whose
    /// clock is not yet calibrated still makes progress and cannot livelock.
    fn slice(&mut self) -> Result<Option<Shot>, Refusal> {
        let start = crate::arch::now_cycles();
        let budget = slice_budget();
        loop {
            let phase = core::mem::replace(&mut self.phase, Phase::Spent);
            match self.unit(phase)? {
                Step::Done(shot) => return Ok(Some(shot)),
                Step::More(next) => self.phase = next,
            }
            if crate::arch::now_cycles().wrapping_sub(start) >= budget {
                self.slices += 1;
                if self.slices <= SLICE_LINES_MAX {
                    let done = match &self.phase {
                        Phase::Write { done, .. } => *done,
                        _ => 0,
                    };
                    serial_println!(
                        ":: PRTSCR: slice n={} bytes={}/{} ::",
                        self.slices,
                        done,
                        self.need
                    );
                }
                return Ok(None);
            }
        }
    }

    /// One unit of work: [`SLICE_ROWS`] scanlines, or [`SLICE_WRITE`] bytes.
    fn unit(&mut self, phase: Phase) -> Result<Step, Refusal> {
        match phase {
            Phase::Encode { mut enc, mut row, mut y } => {
                let end = core::cmp::min(y.saturating_add(SLICE_ROWS), self.height);
                while y < end {
                    row.clear();
                    for x in 0..self.width as usize {
                        // `read_pixel` is the format authority (see the module note). A pixel it
                        // cannot decode cannot happen here — the layout was checked in `begin` —
                        // but an out-of-length tail row on a firmware whose reported height
                        // overruns its own buffer would answer `None`, and black is the honest
                        // answer for "this pixel is not in the framebuffer".
                        let rgb = self.panel.read_pixel(x, y as usize).unwrap_or(0);
                        row.push(((rgb >> 16) & 0xFF) as u8);
                        row.push(((rgb >> 8) & 0xFF) as u8);
                        row.push((rgb & 0xFF) as u8);
                    }
                    enc.push_row(&row)
                        .map_err(|e| Refusal::Encode(e, self.width, self.height, self.need))?;
                    y += 1;
                }
                if y < self.height {
                    return Ok(Step::More(Phase::Encode { enc, row, y }));
                }
                let bytes = enc
                    .finish()
                    .map_err(|e| Refusal::Encode(e, self.width, self.height, self.need))?;
                // PRTSCR-ASYNC/UNPLUG: the encode spent seconds of passes during which the stick
                // could have gone. This is the first volume-touching step since `begin` verified
                // the mount, so it is probed like every write below — an entry created on a disk
                // that left, or on a stranger's, is exactly the stale-handle write this refuses.
                if !self.volume_alive() {
                    return Err(Refusal::Vanished(
                        core::mem::take(&mut self.name),
                        0,
                        bytes.len(),
                    ));
                }
                // The entry is created only now, with the pixels already in hand: the same
                // four-step recipe `shell::fs_write` uses, minus the truncate branch, which cannot
                // apply — `next_free_name` only ever returns a name the root does not hold.
                let (dir_lba, dir_off) = match busy_retry(|| self.fs.create_in_dir(0, &self.name, 0x20)) {
                    Ok((_, lba, off)) => (lba, off),
                    Err(e) => return Err(Refusal::Fat("create", e)),
                };
                Ok(Step::More(Phase::Write {
                    bytes,
                    done: 0,
                    first: 0,
                    size: 0,
                    dir_lba,
                    dir_off,
                }))
            }
            Phase::Write { bytes, mut done, mut first, mut size, dir_lba, dir_off } => {
                // PRTSCR-ASYNC/UNPLUG: probe BEFORE the write, never after — the whole point is that
                // the write is not issued. `done` is the byte count the wire reports, and it is the
                // count the medium actually holds, because `write_grow` published each slice's size
                // as it went.
                if !self.volume_alive() {
                    return Err(Refusal::Vanished(
                        core::mem::take(&mut self.name),
                        done,
                        bytes.len(),
                    ));
                }
                // In order, from `done` upward. `start == size` on every call after the first, so
                // no hole is ever asked for, and each call publishes the grown size + chain head —
                // which is what makes the partial file on the medium a valid PNG PREFIX rather than
                // a size that claims bytes the data does not back.
                let take = core::cmp::min(SLICE_WRITE, bytes.len() - done);
                let at = done;
                let chunk = &bytes[at..at + take];
                let (wrote, new_size, new_first) =
                    match busy_retry(|| self.fs.write_grow(first, size, dir_lba, dir_off, at as u32, chunk)) {
                        Ok(t) => t,
                        Err(e) => return Err(Refusal::Fat("write", e)),
                    };
                if wrote != take {
                    return Err(Refusal::Short(
                        core::mem::take(&mut self.name),
                        at + wrote,
                        bytes.len(),
                    ));
                }
                done += wrote;
                size = new_size;
                first = new_first;
                if done < bytes.len() {
                    Ok(Step::More(Phase::Write { bytes, done, first, size, dir_lba, dir_off }))
                } else {
                    Ok(Step::Done(Shot {
                        name: core::mem::take(&mut self.name),
                        width: self.width,
                        height: self.height,
                        bytes: done,
                    }))
                }
            }
            // Unreachable: `slice` is the only caller and it always hands back a live phase.
            // Answered rather than panicked, per this module's guard-with-a-return discipline.
            Phase::Spent => Err(Refusal::Fat("slice", FatError::Io)),
        }
    }
}

// ================================ PRTSCR-ST — THE BOOT-TIME WITNESS ================================
//
// A capture is not provable from a `check` and not provable from a plain `./arroyo test`, because
// the verb needs an operator at a prompt and the key needs a finger. This drives the REAL
// [`capture`] — the same function the verb and the Print Screen edge call, never a transcription of
// it — once, at boot, and then reads back what landed ON THE MEDIUM through the block layer.
//
// **Its own knob, default OFF** (`UNAOS_PRTSCRST=1`), by the rule that gave `hcronst` a knob apart
// from `holocron` and `sdw` one apart from `sdhcblk`: *a boot that did not ask to WRITE the boot
// medium must be incapable of doing so.* Off the knob this function and its call sites vanish
// entirely, so the gate run (`UNAOS_WC=1 ./arroyo test`) and every shipped image are byte-alike.
//
// It does NOT clean up after itself, and that is deliberate — `btbond::selftest_once` sets the same
// precedent. The written file is the deliverable: `./arroyo test-fat sf` leaves a real `SCREEN0.PNG`
// in `builder/fat-sf.img`, which a host can extract with `mcopy` and decode with a real zlib. A
// kernel that says PASS is evidence; a PNG a foreign decoder opens is proof.
//
// Re-running it on an image that already holds captures is safe and is itself a demonstration: the
// free-name search takes the next index, and a hundredth run refuses rather than overwriting.

/// PRTSCR-ST — drive one real capture at boot and verify what reached the medium.
///
/// One-shot, and **the latch is taken only on a pass that reached a WRITABLE volume.** Both of the
/// states that precede one are transient and neither is a verdict:
///
///  * *No volume at all* — storage enumerates asynchronously, so the early passes have none.
///  * *A volume that vetoes writes* — on a machine whose boot medium is read-only by policy (the
///    rMBP's internal SD reader under SDHC-4c) this state is PERMANENT for the program source:
///    flight-3 proved the `BM_SUBSTITUTED` verdict pins `program_source()` to the Sdhc handle and
///    FRGUARD vetoes the global, so no amount of waiting on THAT mount ever ends. The wait is real
///    anyway because [`mount_capture_target`]'s second rung re-reads the USB registry every pass —
///    a FAT stick hot-plugged minutes after boot reaches `publish_usb_geometry`, the next
///    storage-ready pass mounts it under its own handle, and the deferred selftest runs THEN.
///
/// So both states are announced ONCE, for the log's sake, and then waited through — and the moment
/// a writable volume ends a wait, the arrival is announced too, so the log shows the deferred run
/// firing rather than a PASS appearing out of nowhere. A boot that never gets a writable volume
/// (a plain `./arroyo test`, which attaches no FAT-bearing device) leaves exactly one honest line
/// and never a false FAIL.
#[cfg(feature = "prtscrst")]
pub fn selftest_once() {
    static DONE: AtomicBool = AtomicBool::new(false);
    static SAID_NO_VOLUME: AtomicBool = AtomicBool::new(false);
    static SAID_READ_ONLY: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    let fs = match mount_capture_target() {
        Ok(fs) => fs,
        Err(Refusal::NoVolume(e)) => {
            if !SAID_NO_VOLUME.swap(true, Ordering::Relaxed) {
                serial_println!(
                    ":: PRTSCR-ST: no FAT volume on the program-source or USB handles ({:?}; handles={}) — still waiting; a boot that never gets one leaves the capture selftest SKIPPED ::",
                    e,
                    crate::drivers::block::source_census()
                );
            }
            return;
        }
        Err(Refusal::ReadOnly(source, _, why)) => {
            if !SAID_READ_ONLY.swap(true, Ordering::Relaxed) {
                serial_println!(
                    ":: PRTSCR-ST: program source is {} and vetoes writes ({}) — still waiting for a writable volume; a FAT USB volume plugged in NOW will be adopted on arrival ::",
                    source,
                    why
                );
            }
            return;
        }
        // `mount_capture_target` returns only the two refusals above; anything else would be a
        // future variant, and waiting on it silently would be the dead-loop shape this selftest
        // exists to disprove — so it reports (once, like its siblings) and keeps polling.
        Err(other) => {
            static SAID_OTHER: AtomicBool = AtomicBool::new(false);
            if !SAID_OTHER.swap(true, Ordering::Relaxed) {
                other.report();
            }
            return;
        }
    };
    DONE.store(true, Ordering::Relaxed);
    if SAID_NO_VOLUME.load(Ordering::Relaxed) || SAID_READ_ONLY.load(Ordering::Relaxed) {
        let label = fs.label();
        serial_println!(
            ":: PRTSCR-ST: writable volume arrived (source={} label={}) — running the deferred capture selftest ::",
            fs.source_name(),
            if label.is_empty() { "-" } else { label.as_str() }
        );
    }

    let shot = match capture() {
        Ok(shot) => shot,
        Err(why) => {
            why.report();
            serial_println!(":: PRTSCR-ST: FAIL — the capture itself refused (line above) ::");
            return;
        }
    };
    serial_println!(
        ":: PRTSCR: {} {}x{} {} bytes -> OK ::",
        shot.name, shot.width, shot.height, shot.bytes
    );

    // Read back through the block layer — the directory entry the volume actually holds, and the
    // file's own first and last bytes. Head and tail rather than the whole file: at 2880x1800 the
    // whole file is 15.5 MiB, and the three facts that matter are structural. A truncated write
    // cannot pass all three, because the size is the directory's own and the IEND is at the end.
    let (de, _, _) = match fs.locate_in_dir(0, &shot.name) {
        Ok(hit) => hit,
        Err(e) => {
            serial_println!(
                ":: PRTSCR-ST: FAIL — {} is not in the root after the write ({:?}) ::", shot.name, e
            );
            return;
        }
    };
    if de.size as usize != shot.bytes {
        serial_println!(
            ":: PRTSCR-ST: FAIL — {} is {} bytes on disk, {} were written ::",
            shot.name, de.size, shot.bytes
        );
        return;
    }
    let mut head: Vec<u8> = Vec::new();
    let mut tail: Vec<u8> = Vec::new();
    if fs.read_at(de.first_cluster(), de.size, 0, &mut head, 33).is_err()
        || fs.read_at(de.first_cluster(), de.size, de.size - 12, &mut tail, 12).is_err()
    {
        serial_println!(":: PRTSCR-ST: FAIL — {} could not be read back ::", shot.name);
        return;
    }
    let sig_ok = head.len() >= 33 && head[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let ihdr_ok = sig_ok && &head[12..16] == b"IHDR";
    let w = if ihdr_ok { u32::from_be_bytes([head[16], head[17], head[18], head[19]]) } else { 0 };
    let h = if ihdr_ok { u32::from_be_bytes([head[20], head[21], head[22], head[23]]) } else { 0 };
    let colour_ok = ihdr_ok && head[24] == 8 && head[25] == 2 && head[26] == 0 && head[28] == 0;
    let iend_ok = tail.len() == 12 && &tail[4..8] == b"IEND";
    let dims_ok = w == shot.width && h == shot.height;

    if sig_ok && ihdr_ok && colour_ok && dims_ok && iend_ok {
        serial_println!(
            ":: PRTSCR-ST: {} on the medium — {} bytes, PNG signature OK, IHDR {}x{} depth 8 colour 2 non-interlaced, IEND OK -> PASS ::",
            shot.name, de.size, w, h
        );
    } else {
        serial_println!(
            ":: PRTSCR-ST: FAIL — {} sig={} ihdr={} colour={} dims={}x{} (want {}x{}) iend={} ::",
            shot.name, sig_ok, ihdr_ok, colour_ok, w, h, shot.width, shot.height, iend_ok
        );
    }
}
