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
//! # The window is not enough on its own — VERSIONWIN, the second range
//!
//! Peter, 2026-09-08, on two builds and two cards: *"not making assumptions and not tying them
//! together, possibly staining the testing of the newer version."*
//!
//! 4 KiB of `.text` at `_start` is EARLY BOOT CODE, which barely changes. Two builds from different
//! commits can be byte-identical there while differing everywhere else — and with the first-found
//! rule above, a NEWER kernel booted from the reader could then match an OLDER install's image on
//! the onboard slot, enumerate it first, and root the new build on the old build's disk. Silently,
//! and exactly while someone is testing the new build.
//!
//! So the compared bytes include this build's own identity: [`UNAOS_BUILD_STAMP`], a magic-prefixed
//! byte array carrying `UNAOS_GIT_SHA`, placed in `.text` and compared as a SECOND range whose file
//! offset is derived by the SAME rule as the window's (`offset_of`). A candidate must match BOTH.
//! The witness prints `sha=` beside `match=`.
//!
//! **The limit is stated where the mechanism is** (see the `VERSIONWIN` block below): two builds
//! from the SAME COMMIT share a stamp by design, because same-commit byte identity is load-bearing
//! for this fleet's identity gates. Two DIRTY trees at one commit are separated by `arroyo`'s
//! tracked-diff suffix. If the env was absent at build time the stamp reads `unstamped`, the witness
//! says so, and the decision falls back to code bytes — announced, never silent.
//!
//! # Counting — per DISK, and another one is HOME SOIL (HOMESOIL, Peter, 2026-09-08)
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
//!   found, and that is also what carries VFS-3's hot-plug behaviour forward.)
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
//! # The other disks — home soil, at `/volumes/<NAME>`, with their OWN write posture
//!
//! > Peter, 2026-09-08: "what if the disk has a label? here again you are hard coding — `/usb0` and
//! > `/usb1` are meaningless outside the kernel."
//!
//! Every enumerated disk carrying a FAT volume that is not the root is mounted at
//! `/volumes/<NAME>`, where NAME is **the volume's own label, read off the medium**. No bus, no
//! slot, no index appears in any path: `/usb1` is a fact about which controller a card happens to
//! be hanging off, which is the kernel's business and nobody else's. The witness still carries
//! `source=`, so the bus is on the wire where it belongs.
//!
//! ## The NAME, and why it cannot overflow anything
//!
//! > Peter, 2026-09-08: "joe user might get scared by some crazy disk name appearing if you use the
//! > serial… is it possible to know if there's no volume name set, or a name containing illegal —
//! > possibly even harmful — volume name meant to overflow memory."
//!
//! * **Source**: [`crate::fs::fat::FatFs::label_raw`] — the root directory's `ATTR_VOLUME_ID` entry
//!   when the volume has one, else the BPB's `BS_VolLab`. Both are FIXED 11-byte fields and both
//!   come back as `[u8; 11]`. **No length is ever read from the medium**, so there is no length to
//!   be wrong about and an overflow is impossible by construction rather than by check.
//! * **Unnamed is a KNOWN VALUE, not an absence**: all-spaces, or the conventional `NO NAME`
//!   placeholder, mount as `/volumes/Untitled`. **The serial NEVER appears in a path** — it appears
//!   on the witness line, where an operator can read it and a file manager cannot frighten anyone
//!   with it.
//! * **Sanitize by WHITELIST**, never by blacklist: `A`–`Z`, `a`–`z`, `0`–`9`, space and the
//!   punctuation FAT itself permits in a label (``! # $ % & ' ( ) - @ ^ _ ` { } ~``). EVERY other
//!   byte — control bytes, `/`, NUL, `.` in the wrong place, anything ≥ 0x80 — becomes `_`. Trailing
//!   spaces are trimmed; `.`, `..` and empty become `Untitled`. A path separator therefore cannot
//!   reach the resolver, which is the actual attack this rule is against.
//! * **A collision gets a numeric suffix** — `Untitled`, `Untitled 1`, `Untitled 2` — the macOS
//!   shape, assigned in enumeration order.
//! * **If ANY byte was altered**, the mount line carries `label_raw=<22 hex>`: a card whose label is
//!   trying something announces itself, instead of quietly becoming `Untitled` like every honest
//!   unnamed volume. An unaltered label prints no `label_raw`, so the field's PRESENCE is the signal.
//!
//! ## Posture
//!
//! **The posture is the SOURCE's own**, sampled from the very `FatBackend` that gets mounted:
//! `rw = !FatBackend::read_only()`, which forwards to `BlockSource::write_veto`. `Usb` is WRITABLE
//! (the Pi's verified BOT WRITE(10) path — forcing a read-only mount here would be a behaviour
//! change on the Pi), `TegraSd` is vetoed in every cfg so the Orin's slot card is read-only BY THE
//! VETO rather than by this mount, and `Default` is CONDITIONAL on FRGUARD's `default_writable()`,
//! which is a RUNTIME state and not a property of the volume — so there is no fixed expectation for
//! a `Default`-sourced mount anywhere, in code or in a spec row. One witness line per mount:
//! `[vfs] volume mounted /volumes/UNAOS-PI source=global rw=yes ::`.
//!
//! ## A friend's UnaFS volume
//!
//! Checked at the DECLARATION SITE (`unaos/libs/fs/unafs/src/superblock.rs`, `pub struct
//! Superblock`): the fields are `magic`, `version`, `block_size`, `block_count`, `root_inode`,
//! `catalog_inode`. **UnaFS carries no volume label.** So a non-root disk's UnaFS volume has no name
//! to mount it under, and inventing one — the serial, the handle, an index — is exactly what the
//! rule above forbids. It is ANNOUNCED and left alone:
//! `[vfs] unafs volume on global — unnamed, not mounted ::`. When UnaFS grows a volume name, the
//! same naming rules apply to it and this becomes a mount.
//!
//! **Nothing on a non-root disk influences root.** VFS-3's `/usb` hot-plug mount is this rule's
//! ancestor, not a special case beside it: the same volume, the same posture, the same present-only
//! condition — under a name the person holding the card chose.
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

// =====================================================================================
// VERSIONWIN (orin 22) — the build stamp, and why the code window alone is not enough.
//
// rmbp 16 named the hole and Peter ruled on it (2026-09-08): "not making assumptions and not tying
// them together, possibly staining the testing of the newer version." A 4 KiB slice of `.text` at
// `_start` is early-boot code that barely changes; two builds from DIFFERENT COMMITS can be
// byte-identical THERE while differing everywhere else. With HOMESOIL's first-found rule that is a
// real failure: a NEWER kernel, booted from the card in the reader, could match an OLDER install's
// image on the onboard slot, enumerate it first, and root the new build on the old build's disk —
// silently, and precisely while someone is trying to test the new build.
//
// THE FIX is to put the build's own identity INTO the compared bytes. `UNAOS_GIT_SHA` is exported
// by `arroyo` under its `# BUILD-SHA-1` marker (grep the marker — the line number differs between
// trees) and read here with `option_env!`. It is placed in `.text` as a magic-prefixed byte array
// and compared as a SECOND range, derived from the candidate file's layout EXACTLY as the window is
// (`offset_of`) — not as a name, a date or a size.
//
// THE LIMIT, STATED AT THE SITE: two builds from the SAME COMMIT share a stamp BY DESIGN. That is
// not an oversight to be papered over with a per-build nonce — same-commit byte identity is
// load-bearing for this fleet's identity gates (the knob-off/knob-on comparisons, the staged-media
// sha). What the stamp separates is COMMITS. Two DIRTY trees at one commit are separated by
// `arroyo`'s tracked-diff suffix (same marker, `-d<6hex>`), and a clean build keeps exact bytes.
//
// IF THE ENV WAS ABSENT at build time the stamp reads `unstamped`, the witness says so, and the
// walk still decides on code bytes — announced, never silent.
// =====================================================================================

/// The stamp's magic prefix. Distinctive on purpose: `strings kernel.elf | grep UNAOS-BUILD-STAMP`
/// is how a build's identity is read back out of an artifact.
pub const STAMP_MAGIC: &[u8; 20] = b"UNAOS-BUILD-STAMP-1:";

/// Bytes reserved for the sha text after the magic. 8 hex + a `-d<6hex>` dirty suffix is 16; the
/// rest is zero padding, which costs nothing and leaves room for a longer marker.
pub const STAMP_SHA_MAX: usize = 28;

/// Total stamped length — the second compared range.
pub const STAMP_LEN: usize = STAMP_MAGIC.len() + STAMP_SHA_MAX;

/// This build's identity as `arroyo` exported it, or `unstamped` when nothing did.
pub const BUILD_SHA: &str = match option_env!("UNAOS_GIT_SHA") {
    Some(s) => s,
    None => "unstamped",
};

const fn build_stamp() -> [u8; STAMP_LEN] {
    let mut out = [0u8; STAMP_LEN];
    let mut i = 0;
    while i < STAMP_MAGIC.len() {
        out[i] = STAMP_MAGIC[i];
        i += 1;
    }
    let s = BUILD_SHA.as_bytes();
    let mut j = 0;
    while j < s.len() && j < STAMP_SHA_MAX {
        out[STAMP_MAGIC.len() + j] = s[j];
        j += 1;
    }
    out
}

/// The build stamp, in `.text`.
///
/// `.text` for the SAME reason the window is `.text` (module docs §"What is compared"): it is the
/// only section whose bytes are identical in the file and in RAM on all three link layouts. The
/// input section name is `.text.unaos_build_stamp`, which every link this OS performs folds into
/// `.text` — `crates/kernel/pi-baremetal.ld` does it explicitly (`.text : { *(.text .text.*) }`) and
/// the two UEFI links inherit lld's default script, which does the same.
///
/// `#[used]` + `#[unsafe(no_mangle)]` because nothing ever reads this array through its Rust name in
/// a way the optimizer can see — only its ADDRESS is taken, in [`stamp_addr`] — so without both it
/// is a static the compiler is entitled to drop and the linker is entitled to garbage-collect.
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.unaos_build_stamp")]
pub static UNAOS_BUILD_STAMP: [u8; STAMP_LEN] = build_stamp();

/// The runtime address of the build stamp.
#[inline]
pub fn stamp_addr() -> usize {
    core::ptr::addr_of!(UNAOS_BUILD_STAMP) as *const u8 as usize
}

/// The stamp as this kernel currently holds it in RAM.
fn stamp_mem() -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(stamp_addr() as *const u8, STAMP_LEN) }
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

/// HOMESOIL (orin 22): the identity of one block DEVICE, good across the two `BlockSource` names a
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

/// HOMESOIL: one file on one disk whose bytes ARE this running kernel.
#[derive(Clone)]
pub struct Hit {
    pub path: String,
    pub file_off: u32,
}

/// HOMESOIL: one enumerated DISK carrying a FAT volume, and what the walk found on it.
#[derive(Clone)]
pub struct Disk {
    /// The first source name this device answered to — the one its mount reads through.
    pub source: BlockSource,
    pub id: DiskId,
    /// The volume's label BYTES, exactly as they sit on the medium — a FIXED 11-byte field, never a
    /// length taken from the card. `sanitize_label` turns this into the mount-point name; the raw
    /// bytes are kept so the witness can print `label_raw=` when sanitizing had to alter one.
    pub label: [u8; 11],
    /// Every path on this disk whose bytes ARE this running kernel. Usually 0 or 1; a decoy copy
    /// beside the real image makes it 2 and changes nothing (`files=2`).
    pub hits: Vec<Hit>,
    /// Other source names that resolved to this SAME device and were therefore not walked again.
    pub aliases: Vec<&'static str>,
}

/// Why the walk bound nothing. Spelled exactly as the `reason=` field prints it.
///
/// ⚠ `MultipleKernels` / `reason=multiple-kernels` USED TO BE HERE and is deleted (HOMESOIL, Peter's
/// home-soil rule): several disks carrying this kernel is not an error, it is home soil.
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

/// HOMESOIL: one non-root disk — home soil — with the `/volumes/<NAME>` point it was given and
/// everything the witness owes about how that name was arrived at.
#[derive(Clone)]
pub struct Home {
    /// The source its mount reads through. Printed as `source=`; never part of the path.
    pub source: BlockSource,
    /// The full mount point, `/volumes/<NAME>`, already made unique against its siblings. This is
    /// what gets MOUNTED.
    pub point: String,
    /// Just the NAME — the unique leaf of `point`, without the `/volumes/` prefix. This is what the
    /// witness PRINTS (beside a literal `/volumes/`, so the path is greppable in the artifact), and
    /// it is the `FatBackend`'s volume name. Carried rather than re-split off `point` at each use:
    /// two derivations of one value drift, and both of these are load-bearing.
    pub name: String,
    /// The label bytes this name was derived from — printed as `label_raw=` only when `altered`.
    pub label_raw: [u8; 11],
    /// Did sanitizing have to change any byte? `true` ⇒ the witness prints `label_raw=`, so a card
    /// carrying a hostile or unprintable label announces itself instead of quietly being `Untitled`.
    pub altered: bool,
    /// Does this disk ALSO carry a UnaFS volume? UnaFS has no volume label (checked at
    /// `unafs::superblock::Superblock`), so there is no name to mount it under — it is announced.
    pub unafs: bool,
}

/// HOMESOIL: the whole answer — the root (if any) and every OTHER disk with the point it gets.
#[derive(Clone)]
pub struct Survey {
    /// The disk this kernel was found on: the FIRST in enumeration order that carries it.
    pub root: Option<Found>,
    /// How many files on the ROOT disk are this kernel (`files=`). 2 means a decoy beside it.
    pub root_files: u32,
    /// Why there is no root. `Some` exactly when `root` is `None`.
    pub reason: Option<NoRoot>,
    /// Home soil: one [`Home`] per enumerated non-root disk with a FAT volume, in enumeration
    /// order. Mounted whether or not a root was found.
    pub others: Vec<Home>,
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

/// HOMESOIL: the prefix every non-root volume hangs under. Not a mount itself — an ANCESTOR of
/// mounts, which `shell::vfs_ls_collect` lists synthetically so `/volumes` can be browsed.
pub const VOLUMES: &str = "/volumes";

/// HOMESOIL: the name an unnamed, blank-named or unusable-named volume gets. Never the serial: a
/// path is a thing a PERSON reads, and `/volumes/3F7A1C88` frightens people for no gain. The serial
/// is on the witness line, which is where an operator who needs it will look.
pub const UNTITLED: &str = "Untitled";

/// HOMESOIL: is this byte allowed in a `/volumes/<NAME>` path component?
///
/// A WHITELIST, deliberately — the blacklist spelling of this ("reject `/` and NUL") is the one that
/// keeps being wrong, because the set of bytes that mean something to some later consumer is not
/// knowable from here. Letters, digits, space, and the punctuation the FAT specification itself
/// permits in a volume label. Everything else — control bytes, `/`, NUL, `\`, `:`, `*`, `?`, `"`,
/// `<`, `>`, `|`, `+`, `,`, `;`, `=`, `[`, `]`, `.`, and every byte ≥ 0x80 — is out.
fn label_byte_ok(b: u8) -> bool {
    b.is_ascii_uppercase()
        || b.is_ascii_lowercase()
        || b.is_ascii_digit()
        || b == b' '
        || matches!(
            b,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'(' | b')' | b'-' | b'@' | b'^' | b'_'
                | b'`' | b'{' | b'}' | b'~'
        )
}

/// HOMESOIL: 11 label bytes ⇒ `(name, altered)`.
///
/// `altered` is `true` when the RESULT is not the label the medium carries — a byte was replaced, or
/// the whole thing was rejected in favour of [`UNTITLED`] — which is what makes `label_raw=` on the
/// witness line a signal rather than noise. Note what does NOT count as altered: trailing spaces,
/// and the two KNOWN "no name" spellings (an all-blank field, and the `NO NAME` placeholder every
/// formatter writes). Those are honest unnamed volumes, not suspicious ones.
///
/// The input is `[u8; 11]` by type. There is no length here that came from a card, and no buffer
/// that a card's contents can size, so this cannot overflow anything — by construction, not by check.
fn sanitize_label(raw: &[u8; 11]) -> (String, bool) {
    // The PAD comes off first, and only off the END: a short label is stored space-padded, and some
    // formatters NUL-pad instead. Stripping the pad before sanitizing is what keeps `UNAOS` from
    // becoming `UNAOS______`. A NUL in the MIDDLE of a label is not padding — it survives this trim
    // and is sanitized like any other disallowed byte, and it counts as an alteration.
    let mut end = raw.len();
    while end > 0 && (raw[end - 1] == b' ' || raw[end - 1] == 0) {
        end -= 1;
    }
    let mut out = String::new();
    let mut altered = false;
    for &b in raw[..end].iter() {
        if label_byte_ok(b) {
            out.push(b as char);
        } else {
            out.push('_');
            altered = true;
        }
    }
    let trimmed = out.trim_end_matches(' ');
    // The two KNOWN "unnamed" values — an empty field and the formatter's `NO NAME` placeholder.
    // Neither is an alteration: they are honest unnamed volumes, and `label_raw=` must stay a signal
    // about a SUSPICIOUS card rather than firing on every blank one.
    //
    // `.` and `..` need NO case here, and that is a property of the whitelist rather than an
    // oversight: `.` is not in the FAT label character set and `label_byte_ok` does not admit it, so
    // `..` sanitizes to `__` — a legal path component — long before any "is this name `..`" test
    // could run. A guard for it would be a check that cannot fire. What keeps that true is asserted
    // instead, on the live predicate, in `homesoil_selftest` leg 3.
    if trimmed.is_empty() || trimmed == "NO NAME" {
        return (String::from(UNTITLED), altered);
    }
    (String::from(trimmed), altered)
}

/// HOMESOIL: `/volumes/<name>`, made unique against the points already handed out — `Untitled`,
/// then `Untitled 1`, `Untitled 2`, … (the macOS shape), assigned in enumeration order.
///
/// The suffix is a fact about a COLLISION between two cards on this machine, not about either card,
/// so it is not `altered` and it does not reach the witness's `label_raw=`.
/// Returns `(unique name, full point)` — BOTH, from the one value, so the witness and the mount
/// cannot come to disagree about what this volume is called.
fn next_volume_point(used: &mut Vec<String>, name: &str) -> (String, String) {
    let mut candidate = String::from(name);
    let mut n = 0u32;
    while used.iter().any(|u| u == &candidate) {
        n += 1;
        candidate = alloc::format!("{} {}", name, n);
    }
    used.push(candidate.clone());
    let point = alloc::format!("{}/{}", VOLUMES, candidate);
    (candidate, point)
}

/// HOMESOIL: admit a source as a NEW disk, or record it as an ALIAS of one already admitted.
/// `true` ⇒ it is new and must be walked; `false` ⇒ this device has been walked already under
/// another name and must not be walked, counted or mounted twice.
///
/// Pure over its arguments, so `homesoil_selftest` can drive it with synthetic devices.
fn admit(disks: &mut Vec<Disk>, src: BlockSource, id: DiskId, label: [u8; 11]) -> bool {
    if let Some(d) = disks.iter_mut().find(|d| d.id == id) {
        d.aliases.push(src.name());
        return false;
    }
    disks.push(Disk { source: src, id, label, hits: Vec::new(), aliases: Vec::new() });
    true
}

/// HOMESOIL: pick the root and hand every other disk its `/volumes/<NAME>` point. Split out from the
/// walk on purpose — it is pure, so the `homesoil_selftest` fixture can drive it with SYNTHETIC
/// disks and prove the counting, the dedupe and the NAMING without a second card in the machine.
///
/// `unafs` is passed in rather than probed here so this stays pure: the walk knows which disks carry
/// a UnaFS volume, and asking the block layer from inside a planner would make it untestable.
fn plan(disks: &[Disk], unafs: &[bool]) -> (Option<usize>, Vec<Home>) {
    let root_ix = disks.iter().position(|d| !d.hits.is_empty());
    let mut used: Vec<String> = Vec::new();
    let mut others: Vec<Home> = Vec::new();
    for (i, d) in disks.iter().enumerate() {
        if Some(i) == root_ix {
            continue;
        }
        let (name, altered) = sanitize_label(&d.label);
        let (uniq, point) = next_volume_point(&mut used, &name);
        others.push(Home {
            source: d.source,
            point,
            name: uniq,
            label_raw: d.label,
            altered,
            unafs: unafs.get(i).copied().unwrap_or(false),
        });
    }
    (root_ix, others)
}

/// The whole walk, plus the ONE witness line it owes.
fn walk_and_witness() -> Survey {
    // HOMESOIL: the synthetic legs, on the cached path so they run exactly once, ahead of the walk
    // they describe. Default-quiet (`witness`-gated); see the block comment at the file tail.
    #[cfg(feature = "witness")]
    homesoil_selftest();
    let mut budget: u32 = MAX_ENTRIES;
    let mut candidates: u32 = 0;
    let mut disks: Vec<Disk> = Vec::new();
    let mut unafs: Vec<bool> = Vec::new();
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
        // HOMESOIL: the label is read HERE, off the volume that is already mounted for the walk —
        // one mount, one read, and the bytes travel with the disk rather than being fetched again
        // at bind time from a volume that may have been swapped underneath us.
        let label = fs.label_raw();
        // Admitted even when its root directory turns out to be unreadable below: it HAS a FAT
        // volume (the mount succeeded), so it is a disk this machine has, and home soil is a fact
        // about the disk, not about what could be walked on it.
        if !admit(&mut disks, *src, id, label) {
            continue;
        }
        unafs.push(unafs_present(*src));
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

    let (root_ix, others) = plan(&disks, &unafs);
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
            "[vfs] root = boot volume serial=0x{:08x} source={} match={} sha={} unafs={} \
             matches={} home={} files={} aliased={} window_off={:#x} window_len={} \
             file_off={:#x} candidates={} disks={} ::",
            f.vol_id,
            f.source.name(),
            f.path,
            BUILD_SHA,
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
        "[vfs] root -> NONE reason={} sha={} matches=0 matched=- home={} files=0 aliased={} \
         disks={} candidates={} window_off={:#x} window_len={} ::",
        reason.as_str(),
        BUILD_SHA,
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
            // VERSIONWIN: BOTH ranges, or it is not this kernel. The code window says "same early
            // `.text`", which two builds from different commits can share; the stamp says "same
            // build". Neither alone is the claim.
            if compare_window(fs, de, file_off) && compare_stamp(fs, de) {
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
    offset_of(fs, de, window_addr(), WINDOW)
}

/// VERSIONWIN: the general form — where in THIS file would the `len` bytes this kernel holds at
/// runtime address `addr` live, if this file were this kernel? The window and the build stamp are
/// both derived through it, so the second compared range cannot be located by a different rule from
/// the first. `_start` remains the ONE anchor: it is the image entry, so it is what ties a runtime
/// address to a file offset on both shapes.
fn offset_of(fs: &FatFs, de: &DirEntry, addr: usize, len: usize) -> Option<u32> {
    let mut head: Vec<u8> = Vec::new();
    fs.read_at(de.first_cluster(), de.size, 0, &mut head, 64).ok()?;
    if head.len() < 64 {
        return None;
    }

    // (b) NOT an ELF — a flat image, entry at file offset 0 by the convention this OS's own build
    // uses for one (`llvm-objcopy -O binary` over a link whose first section holds `_start`). So a
    // runtime address maps to the file by its distance from `_start`, which is 0 in the file.
    if head[0..4] != *b"\x7fELF" {
        let off = (addr as u64).checked_sub(window_addr() as u64)?;
        if off.checked_add(len as u64)? > de.size as u64 {
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
    let win = addr as u64;
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
        if within.checked_add(len as u64)? > p_filesz {
            continue;
        }
        let file_off = p_offset.checked_add(within)?;
        if file_off.checked_add(len as u64)? > de.size as u64 {
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

/// VERSIONWIN: the SECOND compared range — this build's stamp, at the offset the file's own layout
/// puts it, derived through [`offset_of`] exactly as the window is.
///
/// Called only for a candidate whose WINDOW already matched, so the cost on the eleven files that
/// are not this kernel is unchanged; a real match pays one extra header read and one 48-byte read.
/// A candidate whose layout cannot place the stamp (no `PT_LOAD` covers it, the offset runs past
/// EOF, a short read) is DECLINED rather than guessed at — the same conservative direction the
/// window derivation takes.
fn compare_stamp(fs: &FatFs, de: &DirEntry) -> bool {
    let Some(off) = offset_of(fs, de, stamp_addr(), STAMP_LEN) else { return false };
    let mut buf: Vec<u8> = Vec::new();
    if fs.read_at(de.first_cluster(), de.size, off, &mut buf, STAMP_LEN).is_err() {
        return false;
    }
    buf.len() == STAMP_LEN && buf.as_slice() == stamp_mem()
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

/// HOMESOIL: does THIS disk carry a UnaFS volume? The `locate_on` half of [`unafs_state`] and
/// nothing else — deliberately no `with_unafs`, because forcing the shared bind is a side effect
/// that has no business running for a disk we are only going to NAME on the wire.
///
/// Used for the home-soil announcement: UnaFS has no volume label (checked at
/// `unafs::superblock::Superblock`, whose fields are magic / version / block_size / block_count /
/// root_inode / catalog_inode), so a friend's UnaFS volume has no name to be mounted under and
/// inventing one is what §"The other disks" forbids.
fn unafs_present(_src: BlockSource) -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        crate::fs::unafs::locate_on(fat::handle_of(_src)).is_ok()
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        false
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
/// HOMESOIL: every other enumerated disk with a FAT volume is mounted at `/volumes/<NAME>` — the
/// volume's OWN label, sanitized by whitelist — with ITS OWN write posture, sampled from the very
/// backend that gets mounted (`rw = !FatBackend::read_only()`, which forwards to
/// `BlockSource::write_veto`). There is deliberately no fixed expectation for a `Default`-sourced
/// mount: its veto is conditional on FRGUARD's `default_writable()`, a runtime state. VFS-3's `/usb`
/// is this rule's ancestor, not a special case beside it.
pub fn bind(mt: &mut crate::fs::vfs::MountTable) {
    use crate::fs::vfs::{FatBackend, KERNEL_PRINCIPAL};
    let s = survey();

    // The mount table is rebuilt PER VERB, so the per-mount witnesses are latched exactly the way
    // the root witness is (which rides the cached walk): announced once, on the first table built.
    let announce = !MOUNTS_ANNOUNCED.swap(true, core::sync::atomic::Ordering::Relaxed);

    for h in s.others.iter() {
        // The backend's volume NAME is the point's unique leaf, so `same_volume` answers about the
        // mount a person can see rather than about a bus the path no longer names.
        //
        // It is also what the witness PRINTS, with `/volumes/` spelled as a LITERAL in the format
        // string below rather than folded into the argument. The wire is byte-identical either way;
        // what changes is that `strings kernel.elf | grep 'volume mounted /volumes/'` can find it.
        // A `{}` carrying the whole runtime-built point leaves only `[vfs] volume mounted ` in
        // rodata, and an artifact census for the path would then read 0 on a correct image — which
        // is how a census comes to certify the wrong thing. (Found by this arc's own gate.)
        //
        // The leaf is the one `next_volume_point` handed out, carried on [`Home`] rather than
        // re-split off `point` here: a witness that re-derives what it prints from a string can
        // drift from the thing it is describing, and the literal above only stays honest while the
        // point really is `/volumes/` + this name. `point` is what gets MOUNTED; `name` is what
        // gets PRINTED; `plan` built both from the same value in the same statement.
        let vol_name = h.name.as_str();
        let be = FatBackend::new_source(vol_name, KERNEL_PRINCIPAL, true, h.source);
        // ONE sample, from the backend being mounted — not a second derivation of the posture.
        let rw = !be.read_only();
        if announce {
            // `label_raw=` appears ONLY when sanitizing altered a byte. Its PRESENCE is the signal,
            // so an ordinary card's line stays short and a card whose label is trying something
            // shows all 11 bytes as hex, un-interpreted.
            if h.altered {
                serial_println!(
                    "[vfs] volume mounted /volumes/{} source={} rw={} label_raw={} ::",
                    vol_name,
                    h.source.name(),
                    if rw { "yes" } else { "no" },
                    hex11(&h.label_raw)
                );
            } else {
                serial_println!(
                    "[vfs] volume mounted /volumes/{} source={} rw={} ::",
                    vol_name,
                    h.source.name(),
                    if rw { "yes" } else { "no" }
                );
            }
            if h.unafs {
                // No volume label exists in the UnaFS superblock, so there is no honest name for
                // this volume and it is left where it is. Said out loud, never silently skipped.
                serial_println!(
                    "[vfs] unafs volume on {} — unnamed, not mounted ::",
                    h.source.name()
                );
            }
        }
        mt.mount(&h.point, alloc::boxed::Box::new(be));
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

/// HOMESOIL: 11 label bytes as 22 lowercase hex digits, un-interpreted. Fixed width by the type, so
/// the field on the wire is always exactly 22 characters and a reader can slice it without parsing.
fn hex11(raw: &[u8; 11]) -> String {
    let mut s = String::new();
    for &b in raw.iter() {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap_or('0'));
    }
    s
}

/// One-shot latch for the `[vfs] volume mounted …` witnesses. See [`bind`].
static MOUNTS_ANNOUNCED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

// =====================================================================================
// HOMESOIL self-test — the legs a single-card QEMU machine cannot show us.
//
// The end-to-end proofs (a real byte flip on the card, a decoy beside the image) run on the QEMU
// Pi leg. What that machine CANNOT do is present a SECOND disk, present ONE disk under TWO handles,
// or present a card whose LABEL is hostile — so the counting, the dedupe and the whole naming rule
// would otherwise be reasoned about and never executed, which is the defect class this fleet has
// paid for repeatedly. The planner ([`admit`], [`plan`], [`sanitize_label`], [`next_volume_point`])
// is pure over its inputs precisely so it can be driven here with SYNTHETIC devices and SYNTHETIC
// label bytes, at the real call, on the real code path.
//
// Leg 3 is RED-FIRST in the only sense available without a card: it feeds the exact byte sequence
// amendment 03 v4 names (`..`, NUL, `/`, DEL, and a byte ≥ 0x80) and asserts that what comes back
// carries NO separator and NO byte outside the whitelist — the resolver never sees one.
//
// `witness`-gated, in the default-quiet idiom (`arroyo` arms `witness` for exactly the four battery
// commands), and driven from the cached walk so it runs once, on the boot that builds the first
// mount table. Uncounted `:: HOMESOIL: … PASS ::` lines, beside the `[vfs] root` witness.
// =====================================================================================

/// HOMESOIL: the counting, dedupe, indexing and posture-sampling legs. See the block comment above.
#[cfg(feature = "witness")]
fn homesoil_selftest() {
    use crate::fs::vfs::{FatBackend, KERNEL_PRINCIPAL};

    // --- leg 1: TWO disks carrying this kernel — first wins, second is home soil at its LABEL. ---
    const L_SPARE: [u8; 11] = *b"SPARE      ";
    let mut ds: Vec<Disk> = Vec::new();
    assert_admit(
        &mut ds,
        BlockSource::Default,
        DiskId { num_blocks: 100, vol_id: 0xaaaa_0001 },
        *b"UNAOS-BOOT ",
    );
    assert_admit(
        &mut ds,
        BlockSource::Usb,
        DiskId { num_blocks: 200, vol_id: 0xbbbb_0002 },
        L_SPARE,
    );
    ds[0].hits.push(Hit { path: String::from("/KERNEL8.IMG"), file_off: 0 });
    ds[1].hits.push(Hit { path: String::from("/KERNEL8.IMG"), file_off: 0 });
    let (root_ix, others) = plan(&ds, &[false, false]);
    let matching = ds.iter().filter(|d| !d.hits.is_empty()).count();
    let leg1 = root_ix == Some(0)
        && matching == 2
        && others.len() == 1
        && others[0].point == "/volumes/SPARE"
        && !others[0].altered;
    serial_println!(
        ":: HOMESOIL: two-disks root_ix={:?} matches={} others={} point={} :: {} ::",
        root_ix,
        matching,
        others.len(),
        if others.is_empty() { "-" } else { others[0].point.as_str() },
        if leg1 { "PASS" } else { "FAIL" }
    );

    // --- leg 2: ONE device under TWO source names — walked once, mounted once. ------------------
    // This is the Orin's real shape: `publish_usb_geometry`'s non-baremetal variant stores one
    // `BlockDeviceInfo` into both `BLOCK_DEVICE` and `USB_BLOCK_DEVICE`.
    let same = DiskId { num_blocks: 100, vol_id: 0xaaaa_0001 };
    let mut ad: Vec<Disk> = Vec::new();
    let first = admit(&mut ad, BlockSource::Default, same, L_SPARE);
    let second = admit(&mut ad, BlockSource::Usb, same, L_SPARE);
    ad[0].hits.push(Hit { path: String::from("/KERNEL8.IMG"), file_off: 0 });
    let (aroot, aothers) = plan(&ad, &[false]);
    let leg2 = first
        && !second
        && ad.len() == 1
        && ad[0].aliases.len() == 1
        && ad[0].aliases[0] == "usb"
        && aroot == Some(0)
        && aothers.is_empty();
    serial_println!(
        ":: HOMESOIL: alias disks={} aliases={} root_ix={:?} others={} :: {} ::",
        ad.len(),
        ad[0].aliases.len(),
        aroot,
        aothers.len(),
        if leg2 { "PASS" } else { "FAIL" }
    );

    // --- leg 3: the NAME rule — sanitize by whitelist, KNOWN unnamed values, collisions. --------
    // Five labels chosen to cover every branch of `sanitize_label`, including the hostile one.
    const L_EVIL: [u8; 11] = *b"..\x00/\x7f\xff AB  ";
    let named = sanitize_label(b"UNAOS-PI   "); // ordinary; `-` is FAT-permitted, so untouched
    let blank = sanitize_label(b"           "); // all spaces: a KNOWN unnamed value
    let noname = sanitize_label(b"NO NAME    "); // the formatter's placeholder: also KNOWN
    let dots = sanitize_label(b"..         "); // `.` is NOT whitelisted, so `..` cannot survive
    let evil = sanitize_label(&L_EVIL); // separators, NUL, DEL, a byte >= 0x80
    let mut used: Vec<String> = Vec::new();
    // `.1` is the full point; `.0` is the leaf the witness prints. Both are asserted, because the
    // wire is `/volumes/` (a literal) + the leaf, and a leaf that did not match its own point would
    // print a path nothing is mounted at.
    let c0 = next_volume_point(&mut used, &blank.0);
    let c1 = next_volume_point(&mut used, &noname.0);
    let c2 = next_volume_point(&mut used, &named.0);
    let c3 = next_volume_point(&mut used, &named.0);
    let leaves_agree = [&c0, &c1, &c2, &c3]
        .iter()
        .all(|(n, p)| *p == alloc::format!("{}/{}", VOLUMES, n));
    // What the RESOLVER must never see, asserted on the PRODUCED string rather than argued about:
    // every byte of every name this function can emit is in the whitelist, so `/`, `\`, NUL, control
    // bytes and everything ≥ 0x80 are absent by the same rule that admits the rest.
    let clean = evil.0.bytes().all(label_byte_ok)
        && dots.0.bytes().all(label_byte_ok)
        && !evil.0.contains('/');
    // The live form of the claim `sanitize_label` makes about `.`: it is out of the charset, so no
    // `.`/`..` special case is needed and none is written. If the whitelist ever admits `.`, this
    // fails here rather than in a path resolver.
    let dot_excluded = !label_byte_ok(b'.') && !dots.0.contains('.') && dots.0 == "__";
    let leg3 = named.0 == "UNAOS-PI"
        && !named.1
        && blank.0 == UNTITLED
        && !blank.1
        && noname.0 == UNTITLED
        && !noname.1
        && dots.1
        && evil.1
        && clean
        && dot_excluded
        && leaves_agree
        && c0.1 == "/volumes/Untitled"
        && c1.1 == "/volumes/Untitled 1"
        && c2.1 == "/volumes/UNAOS-PI"
        && c3.1 == "/volumes/UNAOS-PI 1";
    serial_println!(
        ":: HOMESOIL: names {} blank={} noname={} dots={} evil={} raw={} points {} {} {} {} :: {} ::",
        named.0,
        blank.0,
        noname.0,
        dots.0,
        evil.0,
        hex11(&L_EVIL),
        c0.1,
        c1.1,
        c2.1,
        c3.1,
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
        let be = FatBackend::new_source(UNTITLED, KERNEL_PRINCIPAL, true, *src);
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
        ":: HOMESOIL: posture {} (rw == !read_only, one sample per source) :: {} ::",
        census,
        if leg4 { "PASS" } else { "FAIL" }
    );
}

/// Admit and say so on the wire if it unexpectedly aliased — a synthetic setup that silently
/// collapsed would make the leg above vacuous.
#[cfg(feature = "witness")]
fn assert_admit(disks: &mut Vec<Disk>, src: BlockSource, id: DiskId, label: [u8; 11]) {
    if !admit(disks, src, id, label) {
        serial_println!(":: HOMESOIL: setup source={} aliased unexpectedly :: FAIL ::", src.name());
    }
}
