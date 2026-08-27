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
    // rather than be swallowed by our own clear.
    PENDING.store(false, Ordering::Relaxed);
    match capture() {
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
/// Order is chosen so the cheap refusals come first: the panel and the volume are settled before a
/// single pixel is read or a single byte allocated, so "there is nowhere to put it" costs nothing.
pub fn capture() -> Result<Shot, Refusal> {
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

    // 4. Encode. `PngEncoder::new` reserves the whole output up front, so an allocator refusal
    //    arrives here — before any pixel is read — rather than halfway down the screen.
    let need = PngEncoder::encoded_len(width, height).unwrap_or(0);
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
