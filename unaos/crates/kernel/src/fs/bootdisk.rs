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
//! # Counting — per DISK, and a second one is HOME SOIL (TWOCARD, Peter, 2026-09-08)
//!
//! > "booting dumb means booting dumb. If it sees another UnaOS disk it is home soil and nothing
//! > more."
//!
//! and, on why the two must not be tied together:
//!
//! > "not making assumptions and not tying them together, possibly staining the testing of the
//! > newer version."
//!
//! The walk visits EVERY source and does not stop at the first hit. What it counts is DISKS —
//! block DEVICES — not files:
//!
//! * **exactly one disk** carries this kernel ⇒ bind it, `matches=1`.
//! * **several disks** carry it ⇒ the FIRST in enumeration order is root; every other one is HOME
//!   SOIL and is mounted like any other non-root disk (§The other disks). The witness names them
//!   all: `matches=N home=<src:path,…>`. There is no refusal: refusing to boot because a second
//!   UnaOS card is plugged in is an assumption about the machine, which is the thing this module
//!   exists not to make. (`reason=multiple-kernels` was this module's answer for one afternoon and
//!   is DELETED from the vocabulary.)
//! * **two copies on ONE disk** is ONE disk — a decoy beside the real image changes nothing about
//!   which medium this kernel came off. It binds, and the witness says `files=2`.
//! * **zero** ⇒ `[vfs] root -> NONE`, one witness naming what was looked for and what was found,
//!   and NO `/`, `/boot` or `/apps` — the verbs answer `-ENODEV`. Never a guess at another disk.
//!   (The non-root disks below are still mounted: they are home soil whether or not a root was
//!   found, and that is also what preserves VFS-3's `/usb` behaviour exactly.)
//!
//! **The loader's serial is not consulted here at all.** `drivers::block::BOOT_VOLUME_SERIAL` is a
//! seam for INSTALL-SELF and FRGUARD (`docs/dev/OS/09_FILESYSTEM/vfs.md` §14.7); root does not read
//! it, and this module does not call `fat::locate_boot_volume`.
//!
//! # One disk can wear two names — DEDUPE BY DEVICE
//!
//! `drivers::block::publish_usb_geometry`'s `#[cfg(not(all(target_arch = "aarch64", feature =
//! "baremetal")))]` variant — the one the tegra build compiles — stores the SAME `BlockDeviceInfo`
//! into BOTH `BLOCK_DEVICE` (read as [`BlockSource::Default`]) and `USB_BLOCK_DEVICE` (read as
//! [`BlockSource::Usb`]). So on the Orin one card is reachable under two source names, and a walk
//! that keyed on the source would count it as two disks and mount it beside itself.
//!
//! The key is therefore the DEVICE: [`DiskId`] = `(num_blocks, BS_VolID)` — the geometry
//! [`crate::fs::fat::source_blocks`] reports, plus the mounted volume's own serial from
//! `FatFs::volume_fingerprint`. Both are read from the MEDIUM, so two handles onto one card agree
//! by construction. `drivers::block::BlockDeviceId` is deliberately NOT the key: its first field is
//! `handle`, which is precisely what differs between the two names for the one card. A deduped
//! source is named on the wire (`aliased=usb->global`), never dropped silently.
//!
//! # The other disks — home soil, at a bus-named point, with their OWN write posture
//!
//! Every enumerated disk carrying a FAT volume that is not the root is mounted at an INDEXED,
//! bus-named point: `/usb`, `/usb1`, … for the xHCI mass-storage handles ([`BlockSource::Default`],
//! [`BlockSource::Usb`]) and `/sd`, `/sd1`, … for a controller slot (`Sdhc`, `TegraSd`). The point
//! name is the BUS, and the witness carries `source=` beside it, so the point is never the only
//! identification of a disk.
//!
//! **The posture is the SOURCE's own**, sampled from the very `FatBackend` that gets mounted:
//! `rw = !FatBackend::read_only()`, which forwards to `BlockSource::write_veto`. `Usb` is WRITABLE
//! (the Pi's verified BOT WRITE(10) path — forcing a read-only mount here would be a behaviour
//! change on the Pi), `TegraSd` is vetoed in every cfg so `/sd` on the Orin is read-only BY THE
//! VETO rather than by this mount, and `Default` is CONDITIONAL on FRGUARD's `default_writable()`,
//! which is a RUNTIME state and not a property of the volume — so there is no fixed expectation for
//! a `Default`-sourced mount anywhere, in code or in a spec row. One witness line per mount:
//! `[vfs] disk mounted /usb source=global rw=yes ::`.
//!
//! **Nothing on a non-root disk influences root.** VFS-3's `/usb` hot-plug mount is this rule's
//! first instance rather than a special case beside it: same path, same posture, same condition.
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

/// TWOCARD (orin 22): the identity of one block DEVICE, good across the two `BlockSource` names a
/// single card can wear. See the module docs §"One disk can wear two names".
///
/// `num_blocks` comes from the block registry ([`crate::fs::fat::source_blocks`]) and `vol_id` is
/// the mounted volume's `BS_VolID` — both facts about the MEDIUM, never about the handle, which is
/// why `drivers::block::BlockDeviceId` (whose first field IS the handle) cannot serve here.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DiskId {
    pub num_blocks: u64,
    pub vol_id: u32,
}

/// TWOCARD: one file on one disk whose bytes ARE this running kernel.
#[derive(Clone)]
pub struct Hit {
    pub path: String,
    pub file_off: u32,
}

/// TWOCARD: one enumerated DISK carrying a FAT volume, and what the walk found on it.
#[derive(Clone)]
pub struct Disk {
    /// The first source name this device answered to — the one its mount reads through.
    pub source: BlockSource,
    pub id: DiskId,
    /// Every path on this disk whose bytes ARE this running kernel. Usually 0 or 1; a decoy copy
    /// beside the real image makes it 2 and changes nothing (`files=2`).
    pub hits: Vec<Hit>,
    /// Other source names that resolved to this SAME device and were therefore not walked again.
    pub aliases: Vec<&'static str>,
}

/// Why the walk bound nothing. Spelled exactly as the `reason=` field prints it.
///
/// ⚠ `MultipleKernels` / `reason=multiple-kernels` USED TO BE HERE and is deleted (TWOCARD, Peter's
/// two-card rule): several disks carrying this kernel is not an error, it is home soil.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NoRoot {
    /// Not one source has a device behind it. The machine showed the kernel no disks at all.
    NoDiskEnumerated,
    /// Disks, volumes, candidates — and none of them is this kernel.
    KernelNotFound,
    /// [`MAX_ENTRIES`] was reached, so "not found" cannot be claimed honestly.
    WalkCapHit,
}

impl NoRoot {
    pub fn as_str(self) -> &'static str {
        match self {
            NoRoot::NoDiskEnumerated => "no-disk-enumerated",
            NoRoot::KernelNotFound => "kernel-not-found-on-any-volume",
            NoRoot::WalkCapHit => "walk-cap-hit",
        }
    }
}

/// The walk's answer about the ROOT alone.
#[derive(Clone)]
pub enum Verdict {
    /// A disk carries this kernel — the first one, if several do.
    Bound(Found),
    /// No disk does. Nothing is bound at `/`, `/boot` or `/apps`.
    None_(NoRoot),
}

/// TWOCARD: the whole answer — the root (if any) and every OTHER disk with the point it gets.
#[derive(Clone)]
pub struct Survey {
    /// The disk this kernel was found on: the FIRST in enumeration order that carries it.
    pub root: Option<Found>,
    /// How many files on the ROOT disk are this kernel (`files=`). 2 means a decoy beside it.
    pub root_files: u32,
    /// Why there is no root. `Some` exactly when `root` is `None`.
    pub reason: Option<NoRoot>,
    /// Home soil: `(source, mount point)` for every enumerated non-root disk with a FAT volume,
    /// in enumeration order. Mounted whether or not a root was found.
    pub others: Vec<(BlockSource, String)>,
}

/// The cached answer (module docs §Cost). A plain spin lock rather than the interrupt-masked idiom
/// the unafs `MOUNT` uses: this cell is touched only by the mount-table builder on the shell/verb
/// path, never from an interrupt handler, and the walk runs OUTSIDE the lock so a long disk read
/// never holds it.
static CACHE: Mutex<Option<Survey>> = Mutex::new(None);

/// The full survey — root plus home soil. Cached; the witness prints on the first call only.
pub fn survey() -> Survey {
    if let Some(v) = CACHE.lock().as_ref() {
        return v.clone();
    }
    let v = walk_and_witness();
    *CACHE.lock() = Some(v.clone());
    v
}

/// Find the disk this kernel is running from. Cached; the witness prints on the first call only.
pub fn locate() -> Verdict {
    let s = survey();
    match s.root {
        Some(f) => Verdict::Bound(f),
        None => Verdict::None_(s.reason.unwrap_or(NoRoot::KernelNotFound)),
    }
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

/// TWOCARD: the mount point a non-root disk gets. The BUS, not the medium — `/usb*` for the two
/// xHCI mass-storage handles, `/sd*` for a controller slot. The witness prints `source=` beside it,
/// so the point is never the only identification of a disk.
fn bus_name(src: BlockSource) -> &'static str {
    match src {
        BlockSource::Default | BlockSource::Usb => "usb",
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        BlockSource::Sdhc => "sd",
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        BlockSource::TegraSd => "sd",
    }
}

/// TWOCARD: `/usb`, then `/usb1`, `/usb2`, … — the first disk on a bus takes the bare name, and the
/// index is per bus, assigned in enumeration order. `used` carries one counter per bus seen.
fn next_point(used: &mut Vec<(&'static str, u32)>, bus: &'static str) -> String {
    let n = match used.iter_mut().find(|(b, _)| *b == bus) {
        Some(slot) => {
            slot.1 += 1;
            slot.1
        }
        None => {
            used.push((bus, 0));
            0
        }
    };
    if n == 0 {
        let mut p = String::from("/");
        p.push_str(bus);
        p
    } else {
        alloc::format!("/{}{}", bus, n)
    }
}

/// TWOCARD: admit a source as a NEW disk, or record it as an ALIAS of one already admitted.
/// `true` ⇒ it is new and must be walked; `false` ⇒ this device has been walked already under
/// another name and must not be walked, counted or mounted twice.
///
/// Pure over its arguments, so `twocard_selftest` can drive it with synthetic devices.
fn admit(disks: &mut Vec<Disk>, src: BlockSource, id: DiskId) -> bool {
    if let Some(d) = disks.iter_mut().find(|d| d.id == id) {
        d.aliases.push(src.name());
        return false;
    }
    disks.push(Disk { source: src, id, hits: Vec::new(), aliases: Vec::new() });
    true
}

/// TWOCARD: pick the root and hand every other disk a point. Split out from the walk on purpose —
/// it is pure, so the `twocard_selftest` fixture can drive it with SYNTHETIC disks and prove the
/// counting, the dedupe and the indexing without a second card in the machine.
fn plan(disks: &[Disk]) -> (Option<usize>, Vec<(BlockSource, String)>) {
    let root_ix = disks.iter().position(|d| !d.hits.is_empty());
    let mut used: Vec<(&'static str, u32)> = Vec::new();
    let mut others: Vec<(BlockSource, String)> = Vec::new();
    for (i, d) in disks.iter().enumerate() {
        if Some(i) == root_ix {
            continue;
        }
        others.push((d.source, next_point(&mut used, bus_name(d.source))));
    }
    (root_ix, others)
}

/// The whole walk, plus the ONE witness line it owes.
fn walk_and_witness() -> Survey {
    // TWOCARD: the synthetic legs, on the cached path so they run exactly once, ahead of the walk
    // they describe. Default-quiet (`witness`-gated); see the block comment at the file tail.
    #[cfg(feature = "witness")]
    twocard_selftest();
    let mut budget: u32 = MAX_ENTRIES;
    let mut candidates: u32 = 0;
    let mut disks: Vec<Disk> = Vec::new();
    let mut any_disk = false;
    let mut cap_hit = false;

    for src in fat::ALL_SOURCES {
        if fat::source_present(*src) {
            any_disk = true;
        }
        let Ok(fs) = fat::mount_source(*src) else { continue };
        let vol_id = fs.volume_fingerprint().0;
        // DEDUPE BY DEVICE, before the walk and before the mount: on the tegra build one card is
        // published under BOTH `Default` and `Usb` (module docs §"One disk can wear two names").
        let id = DiskId { num_blocks: fat::source_blocks(*src).unwrap_or(0), vol_id };
        // Admitted even when its root directory turns out to be unreadable below: it HAS a FAT
        // volume (the mount succeeded), so it is a disk this machine has, and home soil is a fact
        // about the disk, not about what could be walked on it.
        if !admit(&mut disks, *src, id) {
            continue;
        }
        let mut hits: Vec<Hit> = Vec::new();
        if let Ok(rows) = fs.read_root() {
            if !walk_rows(&fs, "", 0, &rows, &mut budget, &mut candidates, &mut hits) {
                cap_hit = true;
            }
        }
        if let Some(d) = disks.last_mut() {
            d.hits = hits;
        }
    }

    let (root_ix, others) = plan(&disks);
    let matching = disks.iter().filter(|d| !d.hits.is_empty()).count();
    let win = window_addr();

    // `home=` names every OTHER disk that also carries this kernel — one entry per DISK, spelled
    // `source:path` with that disk's first hit. Not an error, not a refusal: home soil.
    let mut home = String::new();
    for (i, d) in disks.iter().enumerate() {
        if Some(i) == root_ix || d.hits.is_empty() {
            continue;
        }
        if !home.is_empty() {
            home.push(',');
        }
        home.push_str(d.source.name());
        home.push(':');
        home.push_str(&d.hits[0].path);
    }
    if home.is_empty() {
        home.push('-');
    }

    // `aliased=` names every source that resolved to a device already walked — `usb->global` reads
    // "the usb handle is the global handle's card". Deduped, never dropped silently.
    let mut aliased = String::new();
    for d in disks.iter() {
        for a in d.aliases.iter() {
            if !aliased.is_empty() {
                aliased.push(',');
            }
            aliased.push_str(a);
            aliased.push_str("->");
            aliased.push_str(d.source.name());
        }
    }
    if aliased.is_empty() {
        aliased.push('-');
    }

    if let Some(ix) = root_ix {
        let d = &disks[ix];
        let f = Found {
            source: d.source,
            path: d.hits[0].path.clone(),
            vol_id: d.id.vol_id,
            file_off: d.hits[0].file_off,
        };
        serial_println!(
            "[vfs] root = boot volume serial=0x{:08x} source={} match={} unafs={} matches={} \
             home={} files={} aliased={} window_off={:#x} window_len={} file_off={:#x} \
             candidates={} disks={} ::",
            f.vol_id,
            f.source.name(),
            f.path,
            unafs_state(f.source),
            matching,
            home,
            d.hits.len(),
            aliased,
            win,
            WINDOW,
            f.file_off,
            candidates,
            disk_census()
        );
        let root_files = d.hits.len() as u32;
        return Survey { root: Some(f), root_files, reason: None, others };
    }

    let reason = if !any_disk {
        NoRoot::NoDiskEnumerated
    } else if cap_hit {
        NoRoot::WalkCapHit
    } else {
        NoRoot::KernelNotFound
    };
    serial_println!(
        "[vfs] root -> NONE reason={} matches=0 matched=- home={} files=0 aliased={} disks={} \
         candidates={} window_off={:#x} window_len={} ::",
        reason.as_str(),
        home,
        aliased,
        disk_census(),
        candidates,
        win,
        WINDOW
    );
    Survey { root: None, root_files: 0, reason: Some(reason), others }
}

/// One directory's rows, depth-first. Returns `false` if the entry budget ran out inside it.
#[allow(clippy::too_many_arguments)]
fn walk_rows(
    fs: &FatFs,
    prefix: &str,
    depth: u32,
    rows: &[DirEntry],
    budget: &mut u32,
    candidates: &mut u32,
    hits: &mut Vec<Hit>,
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
            if !walk_rows(fs, &path, depth + 1, &sub, budget, candidates, hits) {
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
                hits.push(Hit { path, file_off });
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
/// Nothing found ⇒ **nothing is bound at `/`, `/boot` or `/apps`**. The verbs answer `-ENODEV`,
/// which is the honest answer; a guess at another disk is not. The home-soil mounts below are
/// independent of that: they are the disks the machine HAS, and whether this kernel was found on
/// one of them does not change what the others are.
///
/// TWOCARD: every other enumerated disk with a FAT volume is mounted at its bus point with ITS OWN
/// write posture, sampled from the very backend that gets mounted (`rw = !FatBackend::read_only()`,
/// which forwards to `BlockSource::write_veto`). There is deliberately no fixed expectation for a
/// `Default`-sourced mount: its veto is conditional on FRGUARD's `default_writable()`, a runtime
/// state. VFS-3's `/usb` is this rule's first instance, not a special case beside it.
pub fn bind(mt: &mut crate::fs::vfs::MountTable) {
    use crate::fs::vfs::{FatBackend, KERNEL_PRINCIPAL};
    let s = survey();

    // The mount table is rebuilt PER VERB, so the per-mount witnesses are latched exactly the way
    // the root witness is (which rides the cached walk): announced once, on the first table built.
    let announce = !MOUNTS_ANNOUNCED.swap(true, core::sync::atomic::Ordering::Relaxed);

    for (osrc, point) in s.others.iter() {
        let be = FatBackend::new_source(bus_name(*osrc), KERNEL_PRINCIPAL, true, *osrc);
        // ONE sample, from the backend being mounted — not a second derivation of the posture.
        let rw = !be.read_only();
        if announce {
            serial_println!(
                "[vfs] disk mounted {} source={} rw={} ::",
                point,
                osrc.name(),
                if rw { "yes" } else { "no" }
            );
        }
        mt.mount(point, alloc::boxed::Box::new(be));
    }

    let Some(found) = s.root else { return };
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

/// One-shot latch for the `[vfs] disk mounted …` witnesses. See [`bind`].
static MOUNTS_ANNOUNCED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

// =====================================================================================
// TWOCARD self-test — the legs a single-card QEMU machine cannot show us.
//
// The end-to-end proofs (a real byte flip on the card, a decoy beside the image) run on the QEMU
// Pi leg. What that machine CANNOT do is present a SECOND disk, or present ONE disk under TWO
// handles, so the counting, the dedupe and the point indexing would otherwise be reasoned about
// and never executed — which is the defect class this fleet has paid for repeatedly. The planner
// ([`admit`] and [`plan`]) is pure over its inputs precisely so it can be driven here with
// SYNTHETIC devices, at the real call, on the real code path.
//
// `witness`-gated, in the default-quiet idiom (`arroyo` arms `witness` for exactly the four battery
// commands), and driven from the cached walk so it runs once, on the boot that builds the first
// mount table. Uncounted `:: TWOCARD: … PASS ::` lines, beside the `[vfs] root` witness.
// =====================================================================================

/// TWOCARD: the counting, dedupe, indexing and posture-sampling legs. See the block comment above.
#[cfg(feature = "witness")]
fn twocard_selftest() {
    use crate::fs::vfs::{FatBackend, KERNEL_PRINCIPAL};

    // --- leg 1: TWO disks carrying this kernel — first wins, second is home soil, matches=2. ----
    let mut ds: Vec<Disk> = Vec::new();
    assert_admit(&mut ds, BlockSource::Default, DiskId { num_blocks: 100, vol_id: 0xaaaa_0001 });
    assert_admit(&mut ds, BlockSource::Usb, DiskId { num_blocks: 200, vol_id: 0xbbbb_0002 });
    ds[0].hits.push(Hit { path: String::from("/KERNEL8.IMG"), file_off: 0 });
    ds[1].hits.push(Hit { path: String::from("/KERNEL8.IMG"), file_off: 0 });
    let (root_ix, others) = plan(&ds);
    let matching = ds.iter().filter(|d| !d.hits.is_empty()).count();
    let leg1 = root_ix == Some(0) && matching == 2 && others.len() == 1 && others[0].1 == "/usb";
    serial_println!(
        ":: TWOCARD: two-disks root_ix={:?} matches={} others={} point={} :: {} ::",
        root_ix,
        matching,
        others.len(),
        if others.is_empty() { "-" } else { others[0].1.as_str() },
        if leg1 { "PASS" } else { "FAIL" }
    );

    // --- leg 2: ONE device under TWO source names — walked once, mounted once. ------------------
    // This is the Orin's real shape: `publish_usb_geometry`'s non-baremetal variant stores one
    // `BlockDeviceInfo` into both `BLOCK_DEVICE` and `USB_BLOCK_DEVICE`.
    let same = DiskId { num_blocks: 100, vol_id: 0xaaaa_0001 };
    let mut ad: Vec<Disk> = Vec::new();
    let first = admit(&mut ad, BlockSource::Default, same);
    let second = admit(&mut ad, BlockSource::Usb, same);
    ad[0].hits.push(Hit { path: String::from("/KERNEL8.IMG"), file_off: 0 });
    let (aroot, aothers) = plan(&ad);
    let leg2 = first
        && !second
        && ad.len() == 1
        && ad[0].aliases.len() == 1
        && ad[0].aliases[0] == "usb"
        && aroot == Some(0)
        && aothers.is_empty();
    serial_println!(
        ":: TWOCARD: alias disks={} aliases={} root_ix={:?} others={} :: {} ::",
        ad.len(),
        ad[0].aliases.len(),
        aroot,
        aothers.len(),
        if leg2 { "PASS" } else { "FAIL" }
    );

    // --- leg 3: the point index is per bus, in enumeration order. -------------------------------
    let mut used: Vec<(&'static str, u32)> = Vec::new();
    let p0 = next_point(&mut used, "usb");
    let p1 = next_point(&mut used, "usb");
    let p2 = next_point(&mut used, "sd");
    let p3 = next_point(&mut used, "usb");
    let leg3 = p0 == "/usb" && p1 == "/usb1" && p2 == "/sd" && p3 == "/usb2";
    serial_println!(
        ":: TWOCARD: points {} {} {} {} :: {} ::",
        p0,
        p1,
        p2,
        p3,
        if leg3 { "PASS" } else { "FAIL" }
    );

    // --- leg 4: `rw=` is ONE SAMPLE of the mount that gets bound. --------------------------------
    // The assertion is CONSISTENCY WITHIN ONE CALL, never a fixed expectation per source: a
    // `Default` mount's veto is conditional on FRGUARD's `default_writable()`, which is runtime
    // state. So this compares the two readings the witness and the mount would each take, and says
    // what it read rather than what it wanted.
    let mut leg4 = true;
    let mut census = String::new();
    for src in fat::ALL_SOURCES {
        let be = FatBackend::new_source(bus_name(*src), KERNEL_PRINCIPAL, true, *src);
        let rw = !be.read_only();
        if rw != src.write_veto().is_none() {
            leg4 = false;
        }
        if !census.is_empty() {
            census.push(' ');
        }
        census.push_str(src.name());
        census.push('=');
        census.push_str(if rw { "rw" } else { "ro" });
    }
    serial_println!(
        ":: TWOCARD: posture {} (rw == !read_only, one sample per source) :: {} ::",
        census,
        if leg4 { "PASS" } else { "FAIL" }
    );
}

/// Admit and say so on the wire if it unexpectedly aliased — a synthetic setup that silently
/// collapsed would make the leg above vacuous.
#[cfg(feature = "witness")]
fn assert_admit(disks: &mut Vec<Disk>, src: BlockSource, id: DiskId) {
    if !admit(disks, src, id) {
        serial_println!(":: TWOCARD: setup source={} aliased unexpectedly :: FAIL ::", src.name());
    }
}
