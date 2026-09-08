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

//! BOOTROOT (orin 22) — **find the disk this kernel is running from, by looking for this kernel
//! on it.**
//!
//! # The direction (Peter, 2026-09-08)
//!
//! > "It is an OS booting off an SD card. The card is the hard drive. Every boot is stone cold — no
//! > prefs, no special checks. Boot cold, boot dumb, presume nothing about the machine, even though
//! > we keep booting the same machine."
//!
//! and, on being handed the boot medium's identity by a loader:
//!
//! > "WTF does it matter what method I choose to boot? You are assuming too much."
//!
//! So the kernel is **not told** where it came from. It brings up every disk driver the board has
//! (none behind a knob), enumerates the disks, and finds the one that has THIS KERNEL on it — by
//! CONTENT. That disk is the hard drive. No board, slot, bus, serial, card geometry, boot method or
//! knob enters the decision, and this module carries no board `cfg` anywhere in it.
//!
//! What it replaced: the Orin reached a working `/` through a per-board knob that named the Tegra
//! card (`sdmmcroot`, orin 16, ledger A28), and the Pi reached one because `shell.rs` hard-bound
//! `NativeBackend` at `/`. Both are the same mistake in two spellings — the answer written down in
//! advance. Deleted in this arc's first commit; this module stands in their place, once, for every
//! board.
//!
//! # What is compared, exactly
//!
//! **`WINDOW` = 4096 bytes of `.text`, starting at the symbol `_start`.** The two things compared
//! are:
//!
//! * the bytes at `_start` **in this running kernel's memory**, and
//! * the bytes at the corresponding **file offset in a candidate file** on some FAT volume.
//!
//! `_start` is the right symbol for three reasons, each measured rather than assumed:
//!
//! 1. **It exists on all three link layouts.** x86 UEFI (`main.rs:14`), aarch64 UEFI (`main.rs:21`)
//!    and the Pi's flat bare-metal image (`main.rs:30`, a `global_asm!` block placing it in
//!    `.text.boot`, which `pi-baremetal.ld` puts first at the load address). It is
//!    `#[unsafe(no_mangle)]` in every one, which is why this module can name it from the LIBRARY
//!    crate although it is defined in the BINARY crate.
//! 2. **It IS the image entry point**, so it doubles as the anchor that maps runtime addresses to
//!    file offsets with no second assumption: on the aarch64 image `readelf -h` reports entry
//!    `0x64790` and `readelf -s` reports `_start` at `0x64790`.
//! 3. **`.text` is the only section whose bytes are the same in the file and in RAM.**
//!    - Never `.bss`/`.data`: on the Pi the VideoCore firmware loads `kernel8.img` FLAT, then
//!      `_start` zeroes BSS and early boot writes into the image region long before any VFS work —
//!      a window there would compare a running kernel against its own pristine image and never
//!      match.
//!    - Unaffected by PIE relocation: the aarch64 kernel is `Type: DYN`, and `readelf -r` reports
//!      ZERO relocations whose offset falls inside the `R E` segment `[0x61000, 0x150fb4)` — none in
//!      the window `[0x64790, 0x65790)` in particular. aarch64 and x86_64 code is pc-relative
//!      (`adrp`/`add`, RIP-relative), so `.text` is not patched at load on either.
//!
//! # Per-candidate test
//!
//! Every FILE on the volume with `size >= WINDOW` is a candidate. Its first bytes say which of two
//! shapes it is:
//!
//! * **ELF** (`\x7fELF`, 64-bit, little-endian): parse `e_entry`, `e_phoff`, `e_phentsize`,
//!   `e_phnum`; read the program-header table (bounded, and from further sectors when it does not
//!   sit in the first); find the `PT_LOAD` containing the window. `bias = _start_runtime - e_entry`,
//!   so a segment's runtime base is `p_vaddr + bias` and the window's file offset is
//!   `p_offset + (window_runtime - (p_vaddr + bias))`, bounds-checked against `p_filesz`.
//! * **flat image** (anything else): the OS builds these with the entry at file offset 0
//!   (`llvm-objcopy -O binary` over a link whose first section is `.text.boot`), so
//!   `file_off = window_runtime - _start_runtime`.
//!
//! Then `WINDOW` bytes are read at that offset with [`crate::fs::fat::FatFs::read_at`] — a BOUNDED
//! range read, never a whole file — and compared. Equal ⇒ this file is the running kernel.
//!
//! A candidate that is some OTHER program (`VUG.ELF`, `STAT.ELF`) parses fine and yields a file
//! offset computed from ITS entry; the bytes there are not this kernel's `.text`, so it does not
//! match. The test contains no name, extension, size or directory heuristic at all.
//!
//! # Counting, never first-wins
//!
//! The walk visits EVERY source and does not stop at the first hit.
//!
//! * exactly 1 ⇒ bind it.
//! * 0 ⇒ `[vfs] root -> NONE`, one witness naming what was looked for and what was found, and an
//!   **EMPTY mount table** — the verbs answer `-ENODEV`. Never a guess at another disk.
//! * ≥ 2 ⇒ **REFUSE**, loudly, listing `source:path` for each. Two disks carrying this kernel is a
//!   question about the machine that the machine has to answer; silently picking one is how a boot
//!   roots on a medium it did not come from.
//!
//! # Cost
//!
//! The mount table is rebuilt per verb, so the RESULT is cached in [`CACHE`] — the answer, not a
//! verdict state machine — and the witness therefore prints once. Bounded: depth ≤ [`MAX_DEPTH`],
//! ≤ [`MAX_ENTRIES`] directory entries over the whole walk (a cap that is HIT is a named reason, not
//! a silent truncation), one short read per candidate to classify it plus one window read per
//! plausible one.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::fat::{self, BlockSource, DirEntry, FatFs};

/// The comparison window: 4096 bytes of `.text` at `_start`. See the module docs for why the
/// section, the symbol and the length are each what they are.
pub const WINDOW: usize = 4096;

/// Directory recursion bound. The root is depth 0; a directory at depth `MAX_DEPTH - 1` is not
/// descended into.
pub const MAX_DEPTH: u32 = 4;

/// Total directory entries examined across the WHOLE walk (every source, every directory). Hitting
/// it is a witnessed reason (`walk-cap-hit`), never a quiet stop.
pub const MAX_ENTRIES: u32 = 4096;

// The image entry point, defined in the BINARY crate (`main.rs`) under `#[unsafe(no_mangle)]` on all
// three targets. Declared here, never CALLED here — only its address is ever taken.
unsafe extern "C" {
    fn _start();
}

/// The runtime address of the comparison window: `_start`.
#[inline]
pub fn window_addr() -> usize {
    _start as *const () as usize
}

/// The window as this kernel currently holds it in RAM.
///
/// Sound in the way that matters: it reads `WINDOW` bytes of the kernel's OWN `.text`, mapped
/// readable-and-executable on every target that got far enough to run this code, and `_start` sits
/// `0x3790` into a `0xeffb4`-byte `.text` on the aarch64 image, so the window cannot run off the end
/// of the section.
fn window_mem() -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(window_addr() as *const u8, WINDOW) }
}

/// One file that IS this running kernel.
#[derive(Clone)]
pub struct Found {
    /// The block source its volume is read through.
    pub source: BlockSource,
    /// Its path on that volume, e.g. `/KERNEL8.IMG` or `/kernel.elf`.
    pub path: String,
    /// The volume's `BS_VolID` — reported, never used to decide anything.
    pub vol_id: u32,
    /// The file offset at which the window matched.
    pub file_off: u32,
}

/// Why the walk bound nothing. Spelled exactly as the `reason=` field prints it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NoRoot {
    /// Not one source has a device behind it. The machine showed the kernel no disks at all.
    NoDiskEnumerated,
    /// Disks, volumes, candidates — and none of them is this kernel.
    KernelNotFound,
    /// [`MAX_ENTRIES`] was reached, so "not found" cannot be claimed honestly.
    WalkCapHit,
    /// Two or more files ARE this kernel. Refused; the witness lists them.
    MultipleKernels,
}

impl NoRoot {
    pub fn as_str(self) -> &'static str {
        match self {
            NoRoot::NoDiskEnumerated => "no-disk-enumerated",
            NoRoot::KernelNotFound => "kernel-not-found-on-any-volume",
            NoRoot::WalkCapHit => "walk-cap-hit",
            NoRoot::MultipleKernels => "multiple-kernels",
        }
    }
}

/// The walk's answer.
#[derive(Clone)]
pub enum Verdict {
    /// Exactly one file is this kernel.
    Bound(Found),
    /// Zero, or two or more. Either way nothing is bound and the table stays EMPTY.
    None_(NoRoot),
}

/// The cached answer (module docs §Cost). A plain spin lock rather than the interrupt-masked idiom
/// the unafs `MOUNT` uses: this cell is touched only by the mount-table builder on the shell/verb
/// path, never from an interrupt handler, and the walk runs OUTSIDE the lock so a long disk read
/// never holds it.
static CACHE: Mutex<Option<Verdict>> = Mutex::new(None);

/// Find the disk this kernel is running from. Cached; the witness prints on the first call only.
pub fn locate() -> Verdict {
    if let Some(v) = CACHE.lock().as_ref() {
        return v.clone();
    }
    let v = walk_and_witness();
    *CACHE.lock() = Some(v.clone());
    v
}

/// `global=present usb=absent sdhc=unbuilt tegra-sd=absent` — every handle the block layer defines,
/// named as [`crate::drivers::block::SourceCensus`] names it (ONE vocabulary across both layers, the
/// FATVERB rule), and `unbuilt` for the ones this image does not carry, which is a different fact
/// from "the handle is there and empty".
fn disk_census() -> String {
    let mut out = String::new();
    for (name, state) in [
        ("global", presence(BlockSource::Default)),
        ("usb", presence(BlockSource::Usb)),
        ("sdhc", sdhc_state()),
        ("tegra-sd", tegra_sd_state()),
    ] {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(name);
        out.push('=');
        out.push_str(state);
    }
    out
}

fn presence(src: BlockSource) -> &'static str {
    if fat::source_present(src) { "present" } else { "absent" }
}

fn sdhc_state() -> &'static str {
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    {
        presence(BlockSource::Sdhc)
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "sdhcblk")))]
    {
        "unbuilt"
    }
}

fn tegra_sd_state() -> &'static str {
    #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
    {
        presence(BlockSource::TegraSd)
    }
    #[cfg(not(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc")))]
    {
        "unbuilt"
    }
}

/// The whole walk, plus the ONE witness line it owes.
fn walk_and_witness() -> Verdict {
    let mut budget: u32 = MAX_ENTRIES;
    let mut candidates: u32 = 0;
    let mut matches: Vec<Found> = Vec::new();
    let mut any_disk = false;
    let mut cap_hit = false;

    for src in fat::ALL_SOURCES {
        if fat::source_present(*src) {
            any_disk = true;
        }
        let Ok(fs) = fat::mount_source(*src) else { continue };
        let vol_id = fs.volume_fingerprint().0;
        let rows = match fs.read_root() {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !walk_rows(&fs, *src, vol_id, "", 0, &rows, &mut budget, &mut candidates, &mut matches) {
            cap_hit = true;
        }
    }

    let win = window_addr();
    if matches.len() == 1 {
        let f = matches.remove(0);
        serial_println!(
            "[vfs] root = boot volume serial=0x{:08x} source={} match={} unafs={} matches=1 \
             window_off={:#x} window_len={} file_off={:#x} candidates={} disks={} ::",
            f.vol_id,
            f.source.name(),
            f.path,
            unafs_state(f.source),
            win,
            WINDOW,
            f.file_off,
            candidates,
            disk_census()
        );
        return Verdict::Bound(f);
    }

    let reason = if !matches.is_empty() {
        NoRoot::MultipleKernels
    } else if !any_disk {
        NoRoot::NoDiskEnumerated
    } else if cap_hit {
        NoRoot::WalkCapHit
    } else {
        NoRoot::KernelNotFound
    };
    let mut list = String::new();
    for f in &matches {
        if !list.is_empty() {
            list.push(',');
        }
        list.push_str(f.source.name());
        list.push(':');
        list.push_str(&f.path);
    }
    if list.is_empty() {
        list.push('-');
    }
    serial_println!(
        "[vfs] root -> NONE reason={} matches={} matched={} disks={} candidates={} \
         window_off={:#x} window_len={} ::",
        reason.as_str(),
        matches.len(),
        list,
        disk_census(),
        candidates,
        win,
        WINDOW
    );
    Verdict::None_(reason)
}

/// One directory's rows, depth-first. Returns `false` if the entry budget ran out inside it.
#[allow(clippy::too_many_arguments)]
fn walk_rows(
    fs: &FatFs,
    src: BlockSource,
    vol_id: u32,
    prefix: &str,
    depth: u32,
    rows: &[DirEntry],
    budget: &mut u32,
    candidates: &mut u32,
    matches: &mut Vec<Found>,
) -> bool {
    for de in rows.iter() {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        let name = de.name();
        if name == "." || name == ".." || name.is_empty() {
            continue;
        }
        let mut path = String::with_capacity(prefix.len() + 1 + name.len());
        path.push_str(prefix);
        path.push('/');
        path.push_str(name);

        if de.is_dir {
            if depth + 1 >= MAX_DEPTH {
                continue;
            }
            let Ok(sub) = fs.read_dir(de.first_cluster()) else { continue };
            if !walk_rows(fs, src, vol_id, &path, depth + 1, &sub, budget, candidates, matches) {
                return false;
            }
            continue;
        }

        if (de.size as usize) < WINDOW {
            continue;
        }
        *candidates += 1;
        if let Some(file_off) = window_offset_in(fs, de) {
            if compare_window(fs, de, file_off) {
                matches.push(Found { source: src, path, vol_id, file_off });
            }
        }
    }
    true
}

/// Where in THIS file would this kernel's window live, if this file were this kernel? `None` when the
/// question has no answer for the file's shape (a header we cannot read exactly, `PT_LOAD`s that do
/// not cover the window, a short read, an offset past EOF).
fn window_offset_in(fs: &FatFs, de: &DirEntry) -> Option<u32> {
    let mut head: Vec<u8> = Vec::new();
    fs.read_at(de.first_cluster(), de.size, 0, &mut head, 64).ok()?;
    if head.len() < 64 {
        return None;
    }

    // (b) NOT an ELF — a flat image, entry at file offset 0 by the convention this OS's own build
    // uses for one (`llvm-objcopy -O binary` over a link whose first section holds `_start`).
    if head[0..4] != *b"\x7fELF" {
        let off = (window_addr() as u64).checked_sub(window_addr() as u64)?;
        if off.checked_add(WINDOW as u64)? > de.size as u64 {
            return None;
        }
        return u32::try_from(off).ok();
    }

    // (a) ELF, 64-bit little-endian only: this OS builds nothing else, and a header we cannot read
    // exactly is a file we decline to claim rather than guess at.
    if head[4] != 2 || head[5] != 1 {
        return None;
    }
    let e_entry = u64le(&head, 0x18);
    let e_phoff = u64le(&head, 0x20);
    let e_phentsize = u16le(&head, 0x36) as u64;
    let e_phnum = u16le(&head, 0x38) as u64;
    if e_phentsize < 56 || e_phnum == 0 || e_phnum > 64 {
        return None;
    }
    let table_len = e_phentsize.checked_mul(e_phnum)?;
    if e_phoff.checked_add(table_len)? > de.size as u64 {
        return None;
    }

    // The phdr table may extend past the first sector: read exactly it, bounded, wherever it is.
    let mut phdrs: Vec<u8> = Vec::new();
    fs.read_at(
        de.first_cluster(),
        de.size,
        u32::try_from(e_phoff).ok()?,
        &mut phdrs,
        usize::try_from(table_len).ok()?,
    )
    .ok()?;
    if (phdrs.len() as u64) < table_len {
        return None;
    }

    // `_start` IS the entry, so the load bias is exactly the difference between where the entry is
    // RUNNING and where this file says it was LINKED. Wrapping, because a link base above the
    // runtime address is legal and the difference is still the right modular offset.
    let bias = (window_addr() as u64).wrapping_sub(e_entry);
    let win = window_addr() as u64;
    for i in 0..e_phnum {
        let p = &phdrs[usize::try_from(i * e_phentsize).ok()?..];
        if u32le(p, 0) != 1 {
            continue; // PT_LOAD only
        }
        let p_offset = u64le(p, 0x08);
        let p_vaddr = u64le(p, 0x10);
        let p_filesz = u64le(p, 0x20);
        let seg_mem_base = p_vaddr.wrapping_add(bias);
        if win < seg_mem_base {
            continue;
        }
        let within = win - seg_mem_base;
        if within.checked_add(WINDOW as u64)? > p_filesz {
            continue;
        }
        let file_off = p_offset.checked_add(within)?;
        if file_off.checked_add(WINDOW as u64)? > de.size as u64 {
            return None;
        }
        return u32::try_from(file_off).ok();
    }
    None
}

/// Read the window out of the file and compare it, byte for byte, with the window in RAM.
fn compare_window(fs: &FatFs, de: &DirEntry, file_off: u32) -> bool {
    let mut buf: Vec<u8> = Vec::new();
    if fs.read_at(de.first_cluster(), de.size, file_off, &mut buf, WINDOW).is_err() {
        return false;
    }
    buf.len() == WINDOW && buf.as_slice() == window_mem()
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64le(b: &[u8], o: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(v)
}

// =====================================================================================
// The layout over THAT disk — the OS's own rule, in the one place it is written down.
// =====================================================================================

/// Is there a native UnaFS volume on the disk we matched, and can `/` honestly be it?
///
/// `present` / `absent` / `present-on-other-handle` / `unbuilt`. The third value states the K4
/// write-coherence hazard as a fact instead of risking it: the shared `unafs::MOUNT` is
/// handle-DISCOVERING (UNAFSBIND, `fs/unafs.rs`) — it probes `bind_probe_candidates()` in enum order
/// and `Global` wins outright whenever it holds a volume — so on a machine where two disks carry
/// UnaFS volumes the shared mount may be riding a DIFFERENT disk from the one this kernel booted
/// off. Binding `NativeBackend` at `/` there would root the system on a medium it did not come from,
/// and building a SECOND live mount to avoid that is precisely what `unafs::mount_on`'s own doc
/// forbids ("two of them live at once … is a K4 write-coherence hazard"). So it is neither: the
/// value is reported on the wire and `/` falls back to the FAT volume the kernel WAS found on.
fn unafs_state(_src: BlockSource) -> &'static str {
    #[cfg(target_arch = "aarch64")]
    {
        let handle = fat::handle_of(_src);
        if crate::fs::unafs::locate_on(handle).is_err() {
            return "absent";
        }
        // The disk HAS a volume. Force the shared bind, then ask which disk it settled on.
        let _ = crate::fs::unafs::with_unafs(|_| ());
        match crate::fs::unafs::mount_bound_handle() {
            Some(h) if h == handle => "present",
            _ => "present-on-other-handle",
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        "unbuilt"
    }
}

/// Bind `/`, `/boot` and `/apps` over the disk this kernel was found on.
///
/// * `/boot` — the FAT volume the kernel was found in.
/// * `/apps` — the SAME volume, rooted at its `APPS/` directory, under the SAME volume NAME. A
///   distinct name would make `same_volume("/boot", "/apps")` answer false about one card, which is
///   the C1 aliasing defect (rmbp 15) in a new spelling; `fs/vfs.rs`'s `volume_id` is the identity
///   these two must agree on.
/// * `/` — that DISK's native UnaFS volume when it has one AND the shared mount is riding that disk;
///   otherwise `/boot`'s volume, so a card carrying only a FAT volume still has a root.
///
/// Nothing found ⇒ **nothing is bound**. The caller's table stays EMPTY and the verbs answer
/// `-ENODEV`, which is the honest answer; a guess at another disk is not.
pub fn bind(mt: &mut crate::fs::vfs::MountTable) {
    use crate::fs::vfs::{FatBackend, KERNEL_PRINCIPAL};
    let Verdict::Bound(found) = locate() else { return };
    let src = found.source;

    let native_root = unafs_state(src) == "present";
    #[cfg(target_arch = "aarch64")]
    if native_root {
        mt.mount("/", alloc::boxed::Box::new(crate::fs::vfs::NativeBackend::new("native")));
    }
    if !native_root {
        mt.mount(
            "/",
            alloc::boxed::Box::new(FatBackend::new_source("boot", KERNEL_PRINCIPAL, true, src)),
        );
    }
    mt.mount(
        "/boot",
        alloc::boxed::Box::new(FatBackend::new_source("boot", KERNEL_PRINCIPAL, true, src)),
    );
    mt.mount(
        "/apps",
        alloc::boxed::Box::new(
            FatBackend::new_source("boot", KERNEL_PRINCIPAL, true, src)
                .rooted(crate::fs::fat::APPS_DIR),
        ),
    );
}
