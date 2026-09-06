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

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use unaos_boot_info::PixelFormat;

use crate::fs::fat::{FatError, FatFs};
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

/// PRTSCR — perform a pending capture, if there is one. Call from a device-service pass: task
/// context, interrupts enabled, no driver lock held.
///
/// Costs one relaxed load per call when idle, which is why it can sit unconditionally beside
/// `fat::probe_once()` at every storage-ready pass this kernel carries.
pub fn service() {
    if !PENDING.load(Ordering::Relaxed) {
        return;
    }
    // Clear BEFORE the work, not after: a press that lands mid-capture should arm the NEXT one
    // rather than be swallowed by our own clear. (On the Orin that press is decoded from INSIDE
    // this capture's own storage write — see the module note — and this clear-first order is
    // what makes it a second capture instead of a lost one.)
    PENDING.store(false, Ordering::Relaxed);
    match capture() {
        Ok(shot) => {
            CAPTURES.fetch_add(1, Ordering::Relaxed);
            DEFERRED_SAID.store(false, Ordering::Relaxed);
            serial_println!(
                ":: PRTSCR: {} {}x{} {} bytes -> OK ::",
                shot.name,
                shot.width,
                shot.height,
                shot.bytes
            );
        }
        // PRTSCR2: the door is held by another task's capture (the `screenshot` verb). Not a
        // refusal of the request — it is re-armed and runs on the first pass after the door opens.
        // Said once per episode so a 7 s verb capture does not print 28 copies of the same line.
        Err(Refusal::InFlight) => {
            PENDING.store(true, Ordering::Relaxed);
            if !DEFERRED_SAID.swap(true, Ordering::Relaxed) {
                Refusal::InFlight.report();
            }
        }
        Err(why) => {
            REFUSALS.fetch_add(1, Ordering::Relaxed);
            DEFERRED_SAID.store(false, Ordering::Relaxed);
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
    /// Nothing mounted on any program-source handle.
    NoVolume(FatError),
    /// The volume mounted and refuses writes: `(source, label, reason)`.
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
                ":: PRTSCR: no FAT volume on any program-source handle ({:?}; handles={}) — capture skipped ::",
                e,
                crate::drivers::block::source_census()
            ),
            Refusal::ReadOnly(source, label, why) => serial_println!(
                ":: PRTSCR: REFUSED READ-ONLY (source={} label={} reason={}) — capture skipped ::",
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
                alloc::format!("screenshot: REFUSED READ-ONLY ({})", source)
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

/// The capture proper, under the door [`capture`] holds.
///
/// Order is chosen so the cheap refusals come first: the panel and the volume are settled before a
/// single pixel is read or a single byte allocated, so "there is nowhere to put it" costs nothing.
fn capture_inner() -> Result<Shot, Refusal> {
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

    // 2. The volume, and its own veto, before anything is built.
    let fs = crate::fs::fat::mount_program_source().map_err(Refusal::NoVolume)?;
    if let Some(why) = fs.write_veto() {
        return Err(Refusal::ReadOnly(fs.source_name(), fs.label(), why));
    }

    // 3. A name nothing else owns.
    let name = next_free_name(&fs)?;

    // PRTSCR2: name it on the wire BEFORE it can exist on the medium. From here every exit is one
    // of `-> OK`, a `— capture skipped` refusal, or a boot that ended inside this capture — and the
    // last one is what a `SCREEN<n>.PNG` at 0 bytes means (`write_grow` publishes size LAST).
    let need = PngEncoder::encoded_len(width, height).unwrap_or(0);
    serial_println!(
        ":: PRTSCR: {} {}x{} -> capturing ({} bytes reserved; the verdict line follows — a boot cut before it leaves the entry at 0 bytes) ::",
        name, width, height, need
    );

    // 4. Encode. `PngEncoder::new` reserves the whole output up front, so an allocator refusal
    //    arrives here — before any pixel is read — rather than halfway down the screen.
    let mut enc = PngEncoder::new(width, height)
        .map_err(|e| Refusal::Encode(e, width, height, need))?;
    let mut row: Vec<u8> = Vec::new();
    if row.try_reserve_exact(width as usize * 3).is_err() {
        return Err(Refusal::Encode(PngError::OutOfMemory, width, height, need));
    }
    for y in 0..info.height {
        row.clear();
        for x in 0..info.width {
            // `read_pixel` is the format authority (see the module note). A pixel it cannot decode
            // cannot happen here — the layout was checked above — but an out-of-length tail row on a
            // firmware whose reported height overruns its own buffer would answer `None`, and black
            // is the honest answer for "this pixel is not in the framebuffer".
            let rgb = panel.read_pixel(x, y).unwrap_or(0);
            row.push(((rgb >> 16) & 0xFF) as u8);
            row.push(((rgb >> 8) & 0xFF) as u8);
            row.push((rgb & 0xFF) as u8);
        }
        enc.push_row(&row)
            .map_err(|e| Refusal::Encode(e, width, height, need))?;
    }
    let bytes = enc.finish().map_err(|e| Refusal::Encode(e, width, height, need))?;

    // 5. Write it. The entry is fresh (first_cluster = 0, size = 0), so the grow starts at 0 —
    //    the same four-step recipe `shell::fs_write` uses, minus the truncate branch, which cannot
    //    apply: `next_free_name` only ever returns a name the root does not hold.
    let (dir_lba, dir_off) = match busy_retry(|| fs.create_in_dir(0, &name, 0x20)) {
        Ok((_, lba, off)) => (lba, off),
        Err(e) => return Err(Refusal::Fat("create", e)),
    };
    let written = match busy_retry(|| fs.write_grow(0, 0, dir_lba, dir_off, 0, &bytes)) {
        Ok((written, _, _)) => written,
        Err(e) => return Err(Refusal::Fat("write", e)),
    };
    if written != bytes.len() {
        return Err(Refusal::Short(name, written, bytes.len()));
    }
    Ok(Shot { name, width, height, bytes: written })
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
///  * *A volume that vetoes writes* — on x86 the program-source ladder falls back to the internal
///    SD reader (mounted read-only by SDHC-4c) until the USB stick registers, and only THEN does it
///    prefer the global handle. Latching on that veto gives up a second before the writable volume
///    arrives, which is exactly what the first run of this witness did.
///
/// So both are announced ONCE, for the log's sake, and then waited through. A boot that never gets
/// a writable volume (a plain `./arroyo test`, which attaches no FAT-bearing device) leaves exactly
/// one honest line and never a false FAIL.
#[cfg(feature = "prtscrst")]
pub fn selftest_once() {
    static DONE: AtomicBool = AtomicBool::new(false);
    static SAID_NO_VOLUME: AtomicBool = AtomicBool::new(false);
    static SAID_READ_ONLY: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    let fs = match crate::fs::fat::mount_program_source() {
        Ok(fs) => fs,
        Err(e) => {
            if !SAID_NO_VOLUME.swap(true, Ordering::Relaxed) {
                serial_println!(
                    ":: PRTSCR-ST: no FAT volume on any program-source handle ({:?}; handles={}) — still waiting; a boot that never gets one leaves the capture selftest SKIPPED ::",
                    e,
                    crate::drivers::block::source_census()
                );
            }
            return;
        }
    };
    if let Some(why) = fs.write_veto() {
        if !SAID_READ_ONLY.swap(true, Ordering::Relaxed) {
            serial_println!(
                ":: PRTSCR-ST: program source is {} and vetoes writes ({}) — still waiting for a writable volume ::",
                fs.source_name(),
                why
            );
        }
        return;
    }
    DONE.store(true, Ordering::Relaxed);

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
