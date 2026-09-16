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
//! into BOTH `BLOCK_DEVICE` (read as [`BlockSource::Default`]) and USB registry entry 0 (read as
//! [`BlockSource::Usb`]). So on the Orin one card is reachable under two source names, and a walk
//! that keyed on the source would count it as two disks and mount it beside itself.
//!
//! [`DiskId`] = `(num_blocks, BS_VolID)` — the geometry [`crate::fs::fat::source_blocks`] reports,
//! plus the mounted volume's own serial from `FatFs::volume_fingerprint` — is the CANDIDATE key,
//! and it is a question, not an answer. Both fields are read from the MEDIUM, which is what makes
//! two handles onto one card agree; it is also what makes two CLONES agree. A card imaged
//! byte-for-byte from another carries the same `BS_VolID` and the same size, so a dedupe keyed on
//! content alone merges two real devices into one, hides the second from `/volumes`, and prints a
//! witness claiming — falsely — that one device wore two names (rmbp-ledger B98).
//!
//! **Identity therefore comes from the ENUMERATOR, never from the bytes.** The answer is
//! [`crate::fs::fat::same_device`], the one predicate this kernel has for the question: two
//! `drivers::block::BlockDeviceInfo` records name one device when the slot is LIVE (non-zero), the
//! slots are EQUAL, and `num_blocks` agrees. `slot_id` is a registry fact — a replug lands on a new
//! slot, and two live devices never share one — and a clone cannot forge it.
//! `drivers::block::BlockDeviceId` is deliberately not the key: its first field is `handle`, which
//! is precisely what differs between the two names for the one card.
//!
//! **USBREG (SO33) adds the second half of the key, and it is the half a content check cannot have.**
//! Two cards in ONE multi-slot card reader are one xHCI slot and two SCSI LOGICAL UNITS, so their
//! registry records carry the SAME `slot_id`; if the two cards are also the same size,
//! [`crate::fs::fat::same_device`] answers `true` and this walk would merge two real disks — B98's
//! defect arriving by a second road. The LUN is what separates them and
//! `drivers::block::USB_DISKS` is where the LUN lives, so each disk carries the registry INDEX its
//! source names ([`crate::fs::fat::source_unit`]) and [`admit`] asks `same_device` only of pairs
//! whose indices agree. Two entries of the registry are two devices by the enumerator's own
//! bookkeeping. `None` (a source that is not a USB registry entry — the Pi's microSD in the global,
//! an `Sdhc` card) proves nothing and falls through to `same_device` alone, so every pre-USBREG
//! answer on every board is unchanged. There is still exactly ONE same-device predicate.
//!
//! Two sources are ONE disk — walked once, mounted once, `aliased=usb->global` on the wire — only
//! when that predicate PROVES it. Equal content WITHOUT the proof (two clones; or two zero-slot
//! sources, since `register_sd`, `register_sdhc` and `register_tegra_sd` all stamp the `slot_id: 0`
//! sentinel for a card that never enumerated on a bus) mounts BOTH disks and says so:
//! `aliased=ambiguous:<other>?<this>`. Two friends who look alike are two friends. Nothing is
//! dropped silently in either branch, and root stays first-found — mounting is not exclusive.
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

use crate::drivers::block::BlockDeviceInfo;
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
    /// CLONEALIAS: the block registry's record for the device behind [`Disk::source`]
    /// ([`crate::fs::fat::source_device`]), kept so a LATER source can be tested against this one
    /// with [`crate::fs::fat::same_device`]. `None` when no device is registered on that handle —
    /// which is a refusal to prove sameness, never a match.
    pub dev: Option<BlockDeviceInfo>,
    /// The volume's label BYTES, exactly as they sit on the medium — a FIXED 11-byte field, never a
    /// length taken from the card. `sanitize_label` turns this into the mount-point name; the raw
    /// bytes are kept so the witness can print `label_raw=` when sanitizing had to alter one.
    pub label: [u8; 11],
    /// Every path on this disk whose bytes ARE this running kernel. Usually 0 or 1; a decoy copy
    /// beside the real image makes it 2 and changes nothing (`files=2`).
    pub hits: Vec<Hit>,
    /// Other source names that resolved to this SAME device and were therefore not walked again.
    pub aliases: Vec<&'static str>,
    /// CLONEALIAS: the ALREADY-ADMITTED source names whose [`DiskId`] equals this disk's while the
    /// enumerator refused to prove them the same device — a clone, or a pair of `slot_id: 0`
    /// sources. This disk was admitted and walked ANYWAY; the field exists so the witness can say
    /// `aliased=ambiguous:<other>?<this>` instead of the walk silently losing one of them.
    pub ambiguous: Vec<&'static str>,
    /// USBREG (SO33): which USB block-registry entry this disk's source names, or `None` for a
    /// source that is not a USB registry entry ([`crate::fs::fat::source_unit`]).
    ///
    /// It is the second half of the dedupe, and the half `same_device` structurally cannot supply.
    /// Two cards in ONE multi-slot reader are one xHCI slot and two LOGICAL UNITS, so their
    /// `BlockDeviceInfo` records carry the SAME `slot_id` — and if the two cards are the same size,
    /// `same_device` answers `true` and the walk merges two real disks into one, which is A48/B98's
    /// defect with a different cause. The LUN is what separates them, the registry is where the LUN
    /// lives, and the registry INDEX is that fact in one byte. See [`admit`].
    pub unit: Option<u8>,
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

/// SO38 (X86BIND): the LAST no-root answer, and the present-source fingerprint it was taken under.
///
/// **A NO-ROOT SURVEY IS NOT AN ANSWER, IT IS AN OBSERVATION WITH A TIMESTAMP** — that is the whole
/// of this defect. `CACHE` above is for the ANSWER and only ever holds a survey that BOUND a root;
/// a walk that found none lands here instead, and is reconsidered the moment the machine's disks
/// change. See [`survey`].
static PENDING: Mutex<Option<(String, Survey)>> = Mutex::new(None);

/// SO38: how many walks answered "no root" before one bound. `0` on every board whose medium is up
/// before the first verb (the Pi, the Orin) — this counter is the x86 story and it is on the wire.
static RESURVEYS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// SO38: which sources have a device behind them RIGHT NOW, as a short stable string.
///
/// Registry lookups only — [`fat::source_present`] reads `drivers::block`'s cells and touches no
/// medium — so asking it per verb costs nothing next to the walk it decides against.
fn present_fingerprint() -> String {
    let mut out = String::new();
    for src in fat::live_sources() {
        if fat::source_present(src) {
            if !out.is_empty() {
                out.push(',');
            }
            out.push_str(src.name());
        }
    }
    if out.is_empty() {
        out.push('-');
    }
    out
}

/// The full survey — root plus home soil.
///
/// # SO38 — a NONE is never latched for the boot (X86BIND, 2026-09-15)
///
/// This function used to cache whatever the first walk said, root or no root. That is correct on a
/// board whose medium is up before anything asks (the Pi's microSD, the Orin's card) and WRONG
/// everywhere else, because x86 storage arrives asynchronously: xHCI finishes its deferred SCSI
/// bring-up long after the shell can build a mount table. MEASURED on q35 — the first
/// `shell::vfs_mount_table()` runs at serial line 156 with `disks=global=absent usb=absent
/// sdhc=absent`, and the USB disk that carries this kernel publishes at line 996. The latched NONE
/// handed every later caller an EMPTY namespace for the rest of the boot: `/`, `/boot` and `/apps`
/// unbound, the verbs answering `-ENODEV`, four TSTE legs red. LEDGER SO38, found by LOGIN M1 on
/// aarch64/virt 2026-09-12 and fixed here because X86BIND is the arc that made it bite.
///
/// So: **only a survey that BOUND A ROOT is cached.** A no-root walk goes to [`PENDING`] with the
/// fingerprint of the sources that were present when it ran, and the next caller re-walks as soon
/// as that set CHANGES. The fingerprint is the difference between this and a spin: re-walking
/// because a disk appeared is the point; re-walking twenty times against an unchanged machine is
/// just cost, and on QEMU virt — where no block device ever carries this kernel, so `root` is
/// permanently `None` — it would put a full FAT walk under every single verb. Same answer either
/// way; strictly fewer walks.
pub fn survey() -> Survey {
    if let Some(v) = CACHE.lock().as_ref() {
        return v.clone();
    }
    let fp = present_fingerprint();
    // The machine has not changed since the last walk said "no root", so neither has the answer.
    if let Some((seen, v)) = PENDING.lock().as_ref() {
        if *seen == fp {
            return v.clone();
        }
    }
    // The walk runs OUTSIDE both locks (see [`CACHE`]): a long disk read never holds one.
    let v = walk_and_witness();
    if v.root.is_some() {
        *PENDING.lock() = None;
        *CACHE.lock() = Some(v.clone());
    } else {
        RESURVEYS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        *PENDING.lock() = Some((fp, v.clone()));
    }
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
    // USBREG (SO33): the EXTRA USB registry entries, appended — never interleaved. The four fields
    // above keep their exact spelling and order, so every capture and every doc that greps
    // `global=` / `usb=` / `sdhc=` / `tegra-sd=` reads the same line it always did; a machine with
    // one USB disk (every x86 bench, every Pi bench, every QEMU leg) renders the string unchanged,
    // because there is nothing to append. A two-card reader adds ` usb1=present`, which is the
    // difference this arc exists to make visible.
    for ix in 1..crate::drivers::block::MAX_USB_DISKS {
        if crate::drivers::block::usb_disk_present(ix) {
            out.push(' ');
            out.push_str(BlockSource::UsbN(ix as u8).name());
            out.push_str("=present");
        }
    } push_ahci_census(&mut out); // AHCIBOOT: the SATA disks, APPENDED after the USB rung and never interleaved — the four original fields keep their exact spelling and order, so every capture and doc that greps `global=` / `usb=` / `sdhc=` / `tegra-sd=` reads the line it always did, and a machine with no SATA disk renders the string unchanged. AHCI's own report named the gap this closes: the capture read `handles=global=present sdhc=present` with no `ahci=` term at all.
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
pub(crate) fn sanitize_label(raw: &[u8; 11]) -> (String, bool) {
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
/// CLONEALIAS (orin 24, rmbp-ledger B98): an equal [`DiskId`] is the QUESTION. It is answered by
/// [`fat::same_device`] over the two sources' registry records — the enumerator's identity, which a
/// byte-clone cannot forge — and ONLY a `true` there collapses two sources into one disk. Equal
/// content with no such proof admits the second source as its OWN disk and records the first on its
/// [`Disk::ambiguous`] list, so the witness names the pair rather than the walk losing a real card.
/// `dev: None` (nothing registered on that handle) proves nothing and therefore aliases nothing.
///
/// Pure over its arguments, so `homesoil_selftest` can drive it with synthetic devices.
fn admit(
    disks: &mut Vec<Disk>,
    src: BlockSource,
    id: DiskId,
    dev: Option<BlockDeviceInfo>,
    label: [u8; 11],
    unit: Option<u8>,
) -> bool {
    let mut twins: Vec<&'static str> = Vec::new();
    for d in disks.iter_mut() {
        if d.id != id {
            continue;
        }
        let proven = match (d.dev.as_ref(), dev.as_ref()) {
            (Some(a), Some(b)) => fat::same_device(a, b) && units_agree(d.unit, unit),
            _ => false,
        };
        if proven {
            d.aliases.push(src.name());
            return false;
        }
        // USBREG (SO33): a pair the REGISTRY separates is not AMBIGUOUS — it is two disks we can
        // name. `ambiguous` means "equal content and the enumerator proved nothing either way": two
        // clones, or two `slot_id: 0` sources. Two cards in one reader are the opposite case — the
        // registry's `(slot, LUN)` key is a positive statement that they are different devices — so
        // recording ambiguity here would put the literal token `ambiguous` on the `[vfs] root …`
        // witness for a pair nothing is uncertain about, and any check keyed on `aliased=` would
        // fire on a healthy two-card boot. Caught by leg 7 on its first run, not by reading:
        // `aliased=ambiguous:usb?usb1` with `same_device=true merged_when_same_unit=true`, which
        // reddened `./arroyo test` (rc=1, `serial.log:74`) exactly as a FAIL should.
        if !units_agree(d.unit, unit) {
            continue;
        }
        twins.push(d.source.name());
    }
    disks.push(Disk {
        source: src,
        id,
        dev,
        label,
        hits: Vec::new(),
        aliases: Vec::new(),
        ambiguous: twins,
        unit,
    });
    true
}

/// USBREG (SO33): may these two sources be ONE disk, as far as the USB block registry is concerned?
///
/// This is NOT a second same-device predicate and it must not become one — [`fat::same_device`] is
/// the ONE test for "are these two registry records the same physical device", and it keeps that job.
/// This narrows which PAIRS that test is even asked about, and it does it with a fact `same_device`
/// cannot see: two entries of the USB registry are, by the registry's own key, two different
/// `(slot, LUN)` pairs — two cards in one multi-slot reader, which share an xHCI slot and can share a
/// size. `same_device` compares `(slot_id, num_blocks)`, so on two same-size cards in one reader it
/// answers `true`, and without this guard the walk would merge them: one real card mounted, the other
/// gone from `/volumes`, and a witness claiming an alias that does not exist. That is exactly
/// orin-ledger A48 / rmbp-ledger B98 arriving by a second road.
///
/// `None` on either side means "not a USB registry entry", which proves nothing either way, so the
/// pair falls through to `same_device` alone and every pre-USBREG answer is unchanged:
///  * **x86 / tegra, one card** — `Default` resolves to unit 0 (it holds the primary USB disk) and
///    `Usb` is unit 0: equal, `same_device` decides, `aliased=usb->global` as before.
///  * **Pi** — `Default` is the microSD, so `None`; `same_device`'s `slot_id != 0` guard answers
///    `false` first, as it always did.
///  * **two cards in one reader** — units 0 and 1: refused here, both admitted, both walked, both
///    mounted, no ambiguity claimed (they are not ambiguous; they are two disks and we know it).
fn units_agree(a: Option<u8>, b: Option<u8>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

/// HOMESOIL: render the `aliased=` field of the `[vfs] root …` witness. Pure over the disk list —
/// split out for the same reason [`plan`] is, so the fixture can assert the wire text itself
/// instead of the state behind it.
///
/// Three values, and the reader must be able to tell them apart:
///  * `-` — nothing was deduped and nothing was ambiguous;
///  * `usb->global` — the usb handle is the global handle's card, PROVEN by
///    [`fat::same_device`]: walked once, mounted once;
///  * `ambiguous:global?usb` — CLONEALIAS: identical content, no proof of one device, so BOTH were
///    admitted, walked and mounted. Ambiguous entries come FIRST, so the field begins with the
///    literal token `ambiguous` whenever any pair was ambiguous and a check keyed on `aliased=`
///    fires on this case rather than reading green.
fn aliased_field(disks: &[Disk]) -> String {
    let mut out = String::new();
    for d in disks.iter() {
        for t in d.ambiguous.iter() {
            if !out.is_empty() {
                out.push(',');
            } else {
                out.push_str("ambiguous:");
            }
            out.push_str(t);
            out.push('?');
            out.push_str(d.source.name());
        }
    }
    for d in disks.iter() {
        for a in d.aliases.iter() {
            if !out.is_empty() {
                out.push(',');
            }
            out.push_str(a);
            out.push_str("->");
            out.push_str(d.source.name());
        }
    }
    if out.is_empty() {
        out.push('-');
    }
    out
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

    // USBREG (SO33): the LIVE source list. `ALL_SOURCES` is the compiled-in source KINDS and cannot
    // name a second card in a reader — the registry index is a runtime fact — so a walk over it
    // could only ever find one USB disk however many the machine has. `fat::live_sources` is that
    // same list with the USB rung expanded over the block registry's occupied entries, in the same
    // order, so a one-disk machine walks exactly the list it walked before.
    for src in fat::live_sources() {
        let src = &src;
        if fat::source_present(*src) {
            any_disk = true;
        }
        let Ok(fs) = fat::mount_source(*src) else { continue };
        let vol_id = fs.volume_fingerprint().0;
        // DEDUPE BY DEVICE, before the walk and before the mount: on the tegra build one card is
        // published under BOTH `Default` and `Usb` (module docs §"One disk can wear two names").
        // CLONEALIAS: `id` is the CONTENT question; `dev` is the enumerator's answer to it, and
        // only `dev` can tell one card under two names from two cards imaged off each other.
        let id = DiskId { num_blocks: fat::source_blocks(*src).unwrap_or(0), vol_id };
        let dev = fat::source_device(*src);
        // HOMESOIL: the label is read HERE, off the volume that is already mounted for the walk —
        // one mount, one read, and the bytes travel with the disk rather than being fetched again
        // at bind time from a volume that may have been swapped underneath us.
        let label = fs.label_raw();
        // Admitted even when its root directory turns out to be unreadable below: it HAS a FAT
        // volume (the mount succeeded), so it is a disk this machine has, and home soil is a fact
        // about the disk, not about what could be walked on it.
        if !admit(&mut disks, *src, id, dev, label, fat::source_unit(*src)) {
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

    let aliased = aliased_field(&disks);

    if let Some((ix, f)) = root_ix.and_then(|ix| plan_root_hit(&disks, ix).map(|h| (ix, Found { // PLANWALK (X86BIND's STOP, rmbp-queue): `hits[0]` is reached through a GUARD now, never by indexing — `plan` names a hitless disk in no code path that exists today, and that is exactly the kind of fact that stays true until somebody edits one line in another function. `and_then` collapses the broken-invariant case onto the walk's EXISTING no-root road below, which already prints a reason, so a boot that cannot name its root SAYS SO instead of panicking in the boot path. ⚠ LINE-NEUTRAL: this arm is EIGHT source lines, exactly as many as the `if let Some(ix)` / `let d` / `let f = Found { … };` it replaces, because `panic::Location` embeds source lines and this file's tail blocks record what a mid-file insertion costs. See `plan_root_hit` at the tail for the defect and why a `debug_assert!` was rejected.
            source: disks[ix].source,
            path: h.path.clone(),
            vol_id: disks[ix].id.vol_id,
            file_off: h.file_off,
        }))) {
        // `d` is re-bound here because the witness below reads `d.hits.len()` and `d.source`.
        let d = &disks[ix];
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
        // SO38: how long the answer took to become available, said out loud exactly once — on the
        // walk that finally bound. `n=0` never prints, so a board whose medium is up before the
        // first verb (the Pi, the Orin) keeps a byte-identical `[vfs]` block and a reader who sees
        // this line knows the root arrived LATE rather than at once.
        let resurveys = RESURVEYS.load(core::sync::atomic::Ordering::Relaxed);
        if resurveys > 0 {
            serial_println!(
                "[vfs] resurvey n={} ms={} bound_on_pass={} disks={} :: SO38: a no-root survey is \
                 not cached — {} earlier walk(s) found none before this disk enumerated ::",
                resurveys,
                crate::arch::ticks(),
                resurveys + 1,
                disk_census(),
                resurveys
            );
        }
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
    bind_root(mt, found.source, unafs_state(found.source), announce); #[cfg(feature = "sdwritefx")] sdwrite_fixture(mt, found.source); // SDWRITE (A60): the metal fixture, armed by UNAOS_SDWRITE=1, on the root this call just bound.
}

/// UNAFSROOT (orin 24): the ROOT DISK's three mounts, as a function of ONE fact — the
/// [`unafs_state`] string for the disk this kernel was found on.
///
/// It is a separate function from [`bind`] for a reason that is not tidiness. Everything above it
/// needs real hardware: a survey, a FAT mount per source, a shared-mount bind. This does not — it
/// needs a state string and a table — so the OS's own layout rule can be driven with both answers
/// at the real call, on the real code path, by [`homesoil_selftest`] leg 6. Before this split the
/// rule was reachable only by booting a card that HAD a native volume, which is precisely the card
/// nobody had (render12, 2026-09-09: `unafs=absent`, so `/` had never once been the native volume
/// on this board and the branch had never executed).
///
/// * `/`     — the disk's native UnaFS volume when `unafs` is `present`, else the FAT boot volume.
///             `present` already means BOTH "this disk carries a volume" AND "the shared mount is
///             riding this disk" (see [`unafs_state`]); the other three values
///             (`absent`, `present-on-other-handle`, `unbuilt`) all mean the FAT, each for its own
///             stated reason, and collapsing them here is what keeps that reasoning in one place.
/// * `/boot` — the FAT volume the kernel was found in, always, native root or not. The kernel is
///             loaded by firmware out of FAT, so it can never live at the root of the native
///             volume; `/boot` is where it does live and that is the whole of this line's content.
/// * `/apps` — the SAME FAT volume rooted at `APPS/`, under the SAME volume NAME as `/boot` so
///             `same_volume("/boot", "/apps")` stays true about one card. Programs resolve from the
///             FAT boot volume whether or not `/` is native — a native `/` does not move them.
///
/// THE POSTURE ON THE WIRE. Each mount says `rw=`, and each `rw=` is sampled from the thing being
/// mounted rather than derived a second time: the FAT mounts read `FatBackend::read_only()` off the
/// very backend handed to `mt.mount`, and the native root reads `BlockSource::write_veto()` — the
/// single definition both of those forward to, and the same predicate `block::write_block` itself
/// enforces. For a `Default`-sourced root that resolves to FRGUARD's `default_writable()`, a
/// RUNTIME state, so there is deliberately no fixed expectation for it anywhere: the wire reports
/// what it read. Nothing here weakens, bypasses or re-implements that gate.
pub(crate) fn bind_root(
    mt: &mut crate::fs::vfs::MountTable,
    src: BlockSource,
    unafs: &str,
    announce: bool,
) {
    use crate::fs::vfs::{FatBackend, KERNEL_PRINCIPAL};

    #[cfg(target_arch = "aarch64")]
    let native_root = unafs == "present";
    // `NativeBackend` is `#[cfg(target_arch = "aarch64")]` in fs/vfs.rs, so on a build that does not
    // have the type there is no native root to bind whatever the state string says. This cannot
    // change a real boot's answer — [`unafs_state`] returns `"unbuilt"` on those targets — but it
    // makes leg 6's `present` case HONEST on x86_64 (it asserts the FAT fallback there, and says so
    // on the wire) instead of asking for a mount the type system does not have.
    #[cfg(not(target_arch = "aarch64"))]
    let native_root = {
        let _ = unafs;
        false
    };

    #[cfg(all(target_arch = "aarch64", not(feature = "sdwrite")))] // SDWRITE (A60): knob-off keeps this arm verbatim; the twin that samples the BACKEND is folded onto the closing line below.
    if native_root {
        mt.mount("/", alloc::boxed::Box::new(crate::fs::vfs::NativeBackend::new("native")));
        if announce {
            // The discriminating words are LITERALS in the format string, not a `{}` carrying a
            // runtime kind — the `/volumes/` lesson three screens up, same reason: an artifact
            // census (`LC_ALL=C grep -a -o -F`) must be able to find the sentence it certifies.
            serial_println!(
                "[vfs] root mount / = native unafs volume source={} rw={} ::",
                src.name(),
                if src.write_veto().is_none() { "yes" } else { "no" }
            );
        }
    } #[cfg(all(target_arch = "aarch64", feature = "sdwrite"))] if native_root { native_root_mount(mt, src, announce); } // SDWRITE (A60), second finding: the native arm samples the backend it MOUNTS, not the BlockSource. See `native_root_mount` at the file tail.
    if !native_root {
        let be = FatBackend::new_source("boot", KERNEL_PRINCIPAL, true, src);
        let rw = !be.read_only();
        mt.mount("/", alloc::boxed::Box::new(be));
        if announce {
            serial_println!(
                "[vfs] root mount / = fat boot volume source={} rw={} ::",
                src.name(),
                if rw { "yes" } else { "no" }
            );
        }
    }

    let boot = FatBackend::new_source("boot", KERNEL_PRINCIPAL, true, src);
    let boot_rw = !boot.read_only();
    mt.mount("/boot", alloc::boxed::Box::new(boot));
    mt.mount(
        "/apps",
        alloc::boxed::Box::new(
            FatBackend::new_source("boot", KERNEL_PRINCIPAL, true, src)
                .rooted(crate::fs::fat::APPS_DIR),
        ),
    );
    if announce {
        serial_println!(
            "[vfs] boot mount /boot = fat boot volume source={} rw={} ::",
            src.name(),
            if boot_rw { "yes" } else { "no" }
        );
        serial_println!(
            "[vfs] apps mount /apps = fat boot volume source={} rooted={} ::",
            src.name(),
            crate::fs::fat::APPS_DIR
        );
    }
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
// Leg 5 (CLONEALIAS) is the same idea one level up: this machine can present neither a byte-CLONE
// of its own card nor a second xHCI slot, so it drives `admit` with synthetic `BlockDeviceInfo`
// records that differ ONLY in `slot_id` and asserts that identical content is NOT enough to make
// two sources one disk. It carries its own positive control on `fat::same_device`, because a
// predicate that answered `false` to everything would satisfy the negative clauses for free.
//
// `witness`-gated, in the default-quiet idiom (`arroyo` arms `witness` for exactly the four battery
// commands), and driven from the cached walk so it runs once, on the boot that builds the first
// mount table. Uncounted `:: HOMESOIL: … PASS ::` lines, beside the `[vfs] root` witness.
// =====================================================================================

/// HOMESOIL: the counting, dedupe, indexing and posture-sampling legs. See the block comment above.
#[cfg(feature = "witness")]
fn homesoil_selftest() {
    use crate::fs::vfs::{FatBackend, KERNEL_PRINCIPAL};
    // SO38 (X86BIND): the latch `usbreg_selftest` has always carried, and this one relied on
    // `walk_and_witness` running exactly once to do without. That stopped being true when a no-root
    // survey stopped being cached: the walk now re-runs whenever the machine's disk set changes, so
    // without this the whole leg battery would replay — and re-print its PASS/FAIL lines — once per
    // change. The legs are synthetic and deterministic, so running them again proves nothing; the
    // cost is a log a reader stops trusting.
    {
        use core::sync::atomic::{AtomicBool, Ordering};
        static DONE: AtomicBool = AtomicBool::new(false);
        if DONE.swap(true, Ordering::Relaxed) {
            return;
        }
    }

    // --- leg 1: TWO disks carrying this kernel — first wins, second is home soil at its LABEL. ---
    const L_SPARE: [u8; 11] = *b"SPARE      ";
    let mut ds: Vec<Disk> = Vec::new();
    assert_admit(
        &mut ds,
        BlockSource::Default,
        DiskId { num_blocks: 100, vol_id: 0xaaaa_0001 },
        Some(synth_dev(3, 100)),
        *b"UNAOS-BOOT ",
        // USBREG: the units are SUPPLIED here, never read off the live registry — these disks are
        // synthetic and the machine running the fixture has its own. The global is not a USB disk in
        // this leg (two independent devices, different slots), so it names no registry entry.
        None,
    );
    assert_admit(
        &mut ds,
        BlockSource::Usb,
        DiskId { num_blocks: 200, vol_id: 0xbbbb_0002 },
        Some(synth_dev(4, 200)),
        L_SPARE,
        Some(0),
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
    // `BlockDeviceInfo` into both `BLOCK_DEVICE` and USB registry entry 0 — one record, so one LIVE
    // slot on both sides. CLONEALIAS: that live slot is now what earns the dedupe; equal content
    // alone no longer does, which is exactly what leg 5 below shows.
    let same = DiskId { num_blocks: 100, vol_id: 0xaaaa_0001 };
    let one_card = synth_dev(7, 100);
    let mut ad: Vec<Disk> = Vec::new();
    // USBREG: ONE card, so BOTH names resolve to the SAME registry entry — unit 0 on both sides,
    // which is what `fat::source_unit` answers on a live x86/tegra boot with the primary USB disk in
    // the global slot. Equal units let `same_device` decide, and it proves the alias.
    let first = admit(&mut ad, BlockSource::Default, same, Some(one_card), L_SPARE, Some(0));
    let second = admit(&mut ad, BlockSource::Usb, same, Some(one_card), L_SPARE, Some(0));
    ad[0].hits.push(Hit { path: String::from("/KERNEL8.IMG"), file_off: 0 });
    let (aroot, aothers) = plan(&ad, &[false]);
    let leg2 = first
        && !second
        && ad.len() == 1
        && ad[0].aliases.len() == 1
        && ad[0].aliases[0] == "usb"
        && ad[0].ambiguous.is_empty()
        && aliased_field(&ad) == "usb->global"
        && aroot == Some(0)
        && aothers.is_empty();
    serial_println!(
        ":: HOMESOIL: alias disks={} aliases={} aliased={} root_ix={:?} others={} :: {} ::",
        ad.len(),
        ad[0].aliases.len(),
        aliased_field(&ad),
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

    // --- leg 5: CLONEALIAS — a byte-CLONE is two disks; one card under two names is still one. --
    // The defect this leg exists for (rmbp-ledger B98): `admit` keyed on `DiskId`, which is pure
    // CONTENT, so a card imaged byte-for-byte from another — same size, same `BS_VolID` — was
    // deduped away. One real card vanished from `/volumes` and the witness asserted a false alias.
    // A single-card QEMU machine can present neither a clone nor a second slot, so the whole
    // predicate would otherwise be reasoned about and never executed.
    //
    // NEGATIVE: identical content, DIFFERENT live slots (a replug lands on a new slot, and two live
    // devices never share one) -> BOTH admitted, and the wire says `ambiguous:global?usb`.
    let clone_id = DiskId { num_blocks: 100, vol_id: 0xaaaa_0001 };
    let mut cl: Vec<Disk> = Vec::new();
    let c_first = admit(&mut cl, BlockSource::Default, clone_id, Some(synth_dev(7, 100)), L_SPARE, None);
    let c_second = admit(&mut cl, BlockSource::Usb, clone_id, Some(synth_dev(9, 100)), L_SPARE, Some(0));
    let neg = c_first
        && c_second
        && cl.len() == 2
        && cl[0].aliases.is_empty()
        && cl[1].ambiguous.len() == 1
        && cl[1].ambiguous[0] == "global"
        && aliased_field(&cl) == "ambiguous:global?usb";
    // SLOT-0 SENTINEL, the Pi's shape: `register_sd` / `register_sdhc` / `register_tegra_sd` stamp
    // `slot_id: 0` for a card that never enumerated on a bus, so it carries NO enumerator identity
    // and two such sources are never PROVEN one device — the `slot_id != 0` clause refuses first,
    // ahead of any comparison (pi 7's caveat, now a guard).
    let mut z: Vec<Disk> = Vec::new();
    let z_first = admit(&mut z, BlockSource::Default, clone_id, Some(synth_dev(0, 100)), L_SPARE, None);
    let z_second = admit(&mut z, BlockSource::Usb, clone_id, Some(synth_dev(0, 100)), L_SPARE, Some(0));
    let zero = z_first && z_second && z.len() == 2 && aliased_field(&z) == "ambiguous:global?usb";
    // POSITIVE CONTROL on the predicate itself, so a `same_device` that answered `false` to
    // everything could not make the two clauses above pass vacuously.
    let live = synth_dev(7, 100);
    let pos = fat::same_device(&live, &synth_dev(7, 100))
        && !fat::same_device(&live, &synth_dev(9, 100))
        && !fat::same_device(&synth_dev(0, 100), &synth_dev(0, 100))
        && !fat::same_device(&live, &synth_dev(7, 200));
    let leg5 = neg && zero && pos;
    serial_println!(
        ":: HOMESOIL: clone disks={} aliased={} slot0_disks={} slot0_aliased={} \
         same_device(live,live)={} (live,other)={} (0,0)={} :: {} ::",
        cl.len(),
        aliased_field(&cl),
        z.len(),
        aliased_field(&z),
        fat::same_device(&live, &synth_dev(7, 100)),
        fat::same_device(&live, &synth_dev(9, 100)),
        fat::same_device(&synth_dev(0, 100), &synth_dev(0, 100)),
        if leg5 { "PASS" } else { "FAIL" }
    );

    unafsroot_selftest();
}

/// USBREG (SO33): HOMESOIL leg 7 as its OWN entry point, for exactly the reason leg 6 has one.
///
/// `homesoil_selftest` runs from [`walk_and_witness`], and the QEMU boots that host the fixtures
/// (`test`, `test-arm`) never reach a filesystem verb — orin 26 measured it: `[vfs]` 0 lines,
/// `HOMESOIL` 0 lines on both captures. A leg that only runs when an operator types `ls` on metal is
/// a leg that ships unexecuted, and the two-disk path is the LAST thing in this arc that should ship
/// that way, because no machine in this fleet can present the hardware it is about. So the boot path
/// calls this through [`unafsroot_selftest`], which `main.rs` invokes on the one heap-up line x86,
/// virt and the Pi all pass through; `homesoil_selftest` reaches it by the same call, and the latch
/// makes the second arrival a no-op.
#[cfg(feature = "witness")]
fn usbreg_selftest() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // --- leg 7: USBREG — TWO USB DISKS IN ONE READER. ------------------------------------------
    // The leg this arc owes, and the one no machine in this fleet can present: a multi-slot card
    // reader is ONE xHCI device whose card slots are LOGICAL UNITS, so two cards share a slot id and
    // differ only by LUN — which the block registry keys on and `BlockDeviceInfo` does not carry.
    // QEMU's `usb-storage` is single-LUN on both arches, so without this the whole two-disk path
    // would ship reasoned-about and unexecuted. Driven at the real `admit`/`plan` call, on the real
    // code path, with synthetic disks, exactly as legs 1, 2 and 5 are.
    //
    // The disks are deliberately made INDISTINGUISHABLE BY CONTENT AND BY SLOT: same `num_blocks`,
    // same `BS_VolID`, same synthetic `slot_id` — which is the honest model of two same-size cards
    // in one reader, and the case where `fat::same_device` answers TRUE. That is the point. Without
    // `units_agree` the walk merges them and one real card disappears from `/volumes`; the leg's
    // NEGATIVE CONTROL below re-runs the identical setup with equal units and asserts that it DOES
    // merge, so a `units_agree` that answered `false` to everything could not pass this vacuously.
    //
    // ROOT BY CONTENT, unchanged: the hit is put on the SECOND disk only, and the leg asserts that
    // root is the second disk — first-found among the disks that carry this kernel, never
    // first-enumerated, never the unit the driver happened to publish first.
    const L_CARD_A: [u8; 11] = *b"UNAOS-BOOT ";
    const L_CARD_B: [u8; 11] = *b"UNAOS-DATA ";
    let reader = synth_dev(5, 100);
    let twin_id = DiskId { num_blocks: 100, vol_id: 0xcccc_0003 };
    let mut two: Vec<Disk> = Vec::new();
    let u0 = admit(&mut two, BlockSource::Usb, twin_id, Some(reader), L_CARD_A, Some(0));
    let u1 = admit(&mut two, BlockSource::UsbN(1), twin_id, Some(reader), L_CARD_B, Some(1));
    // FIRST, with NO kernel on either card: BOTH are home soil, and each gets its OWN
    // `/volumes/<LABEL>` point from its own label. This is the shape the arc owes on the wire — two
    // cards in one reader, two mounts, distinct points — and it is asserted before any root exists,
    // so it cannot be satisfied by the root-binding path instead.
    let (nroot, nothers) = plan(&two, &[false, false]);
    // THEN the kernel on the SECOND card. Root is chosen by CONTENT, so it must be disk 1 — not
    // disk 0, which enumerated first and is the unit the driver published first.
    two[1].hits.push(Hit { path: String::from("/KERNEL8.IMG"), file_off: 0 });
    let (troot, tothers) = plan(&two, &[false, false]);
    // The positive control: the SAME content, the SAME slot, the SAME unit — one card under two
    // names — must still collapse to one disk. Two entries of the registry are two devices; two
    // names for one entry are one device; and this pair of assertions is what separates them.
    let mut one: Vec<Disk> = Vec::new();
    let m0 = admit(&mut one, BlockSource::Usb, twin_id, Some(reader), L_CARD_A, Some(0));
    let m1 = admit(&mut one, BlockSource::Default, twin_id, Some(reader), L_CARD_A, Some(0));
    let leg7 = u0
        && u1
        && two.len() == 2
        // No root: both cards mounted under /volumes, at DISTINCT points, from their own labels.
        && nroot.is_none()
        && nothers.len() == 2
        && nothers[0].point == "/volumes/UNAOS-BOOT"
        && nothers[1].point == "/volumes/UNAOS-DATA"
        && nothers[0].point != nothers[1].point
        && nothers[0].source.name() == "usb"
        && nothers[1].source.name() == "usb1"
        && !nothers[0].altered
        && !nothers[1].altered
        // NOT aliased and NOT ambiguous: these are two disks and the registry knows it.
        && two[0].aliases.is_empty()
        && two[1].aliases.is_empty()
        && two[1].ambiguous.is_empty()
        && aliased_field(&two) == "-"
        && troot == Some(1)
        // The other disk is home soil at its OWN label, and the two points are DISTINCT.
        && tothers.len() == 1
        && tothers[0].point == "/volumes/UNAOS-BOOT"
        && tothers[0].source.name() == "usb"
        && two[1].source.name() == "usb1"
        // Same content, same slot, EQUAL units -> one disk, and the wire says so.
        && m0
        && !m1
        && one.len() == 1
        && aliased_field(&one) == "global->usb"
        // And the predicate that separates the two cases, asked directly in both directions.
        && !units_agree(Some(0), Some(1))
        && units_agree(Some(1), Some(1))
        && units_agree(None, Some(1))
        // ...while `same_device` itself still says TRUE for this pair, which is precisely why the
        // unit guard has to exist. If this clause ever reads false the leg is passing for the wrong
        // reason and the assertion above it proves nothing.
        && fat::same_device(&reader, &reader);
    serial_println!(
        ":: HOMESOIL: usbreg disks={} noroot_points={}+{} root_ix={:?} others={} point={} \
         sources={}+{} aliased={} merged_when_same_unit={} same_device={} :: {} ::",
        two.len(),
        if nothers.is_empty() { "-" } else { nothers[0].point.as_str() },
        if nothers.len() > 1 { nothers[1].point.as_str() } else { "-" },
        troot,
        tothers.len(),
        if tothers.is_empty() { "-" } else { tothers[0].point.as_str() },
        two[0].source.name(),
        if two.len() > 1 { two[1].source.name() } else { "-" },
        aliased_field(&two),
        one.len() == 1,
        fat::same_device(&reader, &reader),
        if leg7 { "PASS" } else { "FAIL" }
    ); planwalk_selftest(); // PLANWALK (leg 8) — the plan/walk invariant, fed the state that used to PANIC. ⚠ SAME-LINE fold and the BODY is a FILE-TAIL append, so no `panic::Location` in this file moves; the two tail blocks below already record why that matters here. LAST in this battery because it is the only leg that drives a GUARD rather than the planner.

}

/// UNAFSROOT (orin 26): HOMESOIL leg 6 as its own entry point, because the QEMU boots that host the
/// fixtures (`test`, `test-arm`) never reach a filesystem verb, so nothing under
/// [`walk_and_witness`] executes there — measured on this arc's own captures: `[vfs]` 0 lines,
/// `HOMESOIL` 0 lines on both `target/serial.log` and `target/serial-arm.log`. A leg that only
/// runs when an operator types `ls` on metal is a leg that ships unexecuted, which is the defect
/// class the split in [`bind_root`] exists to end. So the boot path calls this once, under
/// `witness`, on both arches (main.rs, same-line appends), and [`homesoil_selftest`] calls it too
/// so the metal wire keeps all six legs together; the latch makes the second call a no-op.
#[cfg(feature = "witness")]
pub fn unafsroot_selftest() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // USBREG (SO33): leg 7 rides the same boot-path entry point, for the same reason — see
    // [`usbreg_selftest`]. It carries its own latch, so calling it here and from
    // `homesoil_selftest` prints it exactly once.
    usbreg_selftest(); #[cfg(feature = "sdwrite")] sdwrite_posture_selftest(); crate::drivers::block::usbunpub_selftest(); crate::drivers::block::lba32_selftest(); // SDWRITE (A60) leg 8: the posture/polarity matrix, latched like its siblings. BLOCKSMALL: legs 9 (USBUNPUB — a stick REMOVAL advances the cache-invalidation generation) and 10 (LBA32/SR15 — the SCSI READ(10) sector argument REFUSES above 32 bits instead of wrapping onto the boot sector) ride this same entry point, for the reason leg 7 does: they live in `drivers/block.rs`, which has no boot-path witness seam of its own, and this is the ONE heap-up line x86, virt and the Pi all pass through — so `test`/`test-arm` execute them instead of shipping them reasoned-about. Both carry their own latch. Ordering matters and is deliberate: at heap-up the USB registry is still EMPTY (xHCI has not enumerated), which is what makes leg 9's synthetic publish/retract pair unable to disturb a live boot volume. Line-neutral append, this file's stated rule.
    // --- leg 6: UNAFSROOT — the root disk's LAYOUT RULE, driven with every answer it takes. -----
    // `bind_root` is the one place `/`, `/boot` and `/apps` are decided, and until this leg the
    // `present` answer had never executed anywhere (render12: `unafs=absent`, no card carried a
    // native volume). Table shape ONLY: `NativeBackend::new` and `FatBackend::new_source` hold a
    // name, a principal and a source and touch no disk until resolved for I/O; `volume_name`,
    // `mount_root` and `prefixes` are accessors. `same_volume` is deliberately NOT asked here —
    // `FatBackend::volume_id` mounts the source to fingerprint it, and a fixture that runs AHEAD
    // of the walk must not be the first thing to touch the card. `announce=false`: no wire line
    // is owed by a table nothing will ever resolve through.
    //
    // The four answers `unafs_state` can give, each with the root it must produce:
    //   present                  -> `/` native (aarch64) — on a build without `NativeBackend` the
    //                               FAT, and the leg asserts THAT, so an x86 boot proves the same
    //                               function honestly instead of skipping it;
    //   absent, present-on-other-handle, unbuilt -> `/` FAT, on every build.
    // `/boot` is the FAT volume `boot` and `/apps` is the same name rooted at `APPS_DIR`, in all
    // four tables — a native `/` moves neither.
    #[cfg(target_arch = "aarch64")]
    const ROOT_WHEN_PRESENT: &str = "native";
    #[cfg(not(target_arch = "aarch64"))]
    const ROOT_WHEN_PRESENT: &str = "boot";
    let shape = |state: &str| -> (String, String, String, String, usize) {
        let mut mt = crate::fs::vfs::MountTable::new();
        bind_root(&mut mt, BlockSource::Default, state, false);
        let name = |p: &str| mt.volume_name(p).unwrap_or_else(|_| String::from("-"));
        let apps_root = mt
            .resolve("/apps")
            .map(|(b, _)| String::from(b.mount_root()))
            .unwrap_or_else(|_| String::from("-"));
        (name("/"), name("/boot"), name("/apps"), apps_root, mt.prefixes().len())
    };
    let sp = shape("present");
    let sa = shape("absent");
    let so = shape("present-on-other-handle");
    let su = shape("unbuilt");
    // `FatBackend::rooted` stores the root in canonical `/`-led component form, so the expectation
    // is derived the same way from the same constant rather than compared to the bare name.
    let apps_root_want = alloc::format!("/{}", crate::fs::fat::APPS_DIR);
    let fat_rooted = |s: &(String, String, String, String, usize)| {
        s.1 == "boot" && s.2 == "boot" && s.3 == apps_root_want && s.4 == 3
    };
    let leg6 = sp.0 == ROOT_WHEN_PRESENT
        && fat_rooted(&sp)
        && sa.0 == "boot"
        && fat_rooted(&sa)
        && so.0 == "boot"
        && fat_rooted(&so)
        && su.0 == "boot"
        && fat_rooted(&su);
    serial_println!(
        ":: HOMESOIL: root rule present=/:{} /boot:{} /apps:{}@{} absent=/:{} other=/:{} unbuilt=/:{} \
         mounts={} :: {} ::",
        sp.0,
        sp.1,
        sp.2,
        sp.3,
        sa.0,
        so.0,
        su.0,
        sp.4,
        if leg6 { "PASS" } else { "FAIL" }
    );
}

/// CLONEALIAS: a synthetic `BlockDeviceInfo` for the fixture — the two fields
/// [`fat::same_device`] reads are the arguments; the rest are inert filler the predicate never
/// looks at. The type expresses two devices that differ ONLY in slot, which is what the negative
/// leg needs.
#[cfg(feature = "witness")]
fn synth_dev(slot_id: u8, num_blocks: u64) -> BlockDeviceInfo {
    BlockDeviceInfo {
        slot_id,
        block_size: 512,
        num_blocks,
        vendor: *b"UNAOS   ",
        product: *b"SYNTHETIC DISK  ",
    }
}

/// Admit and say so on the wire if it unexpectedly aliased — a synthetic setup that silently
/// collapsed would make the leg above vacuous.
#[cfg(feature = "witness")]
fn assert_admit(
    disks: &mut Vec<Disk>,
    src: BlockSource,
    id: DiskId,
    dev: Option<BlockDeviceInfo>,
    label: [u8; 11],
    unit: Option<u8>,
) {
    if !admit(disks, src, id, dev, label, unit) {
        serial_println!(":: HOMESOIL: setup source={} aliased unexpectedly :: FAIL ::", src.name());
    }
}

// ===================== SDWRITE (A60, 2026-09-12) — the root's write POSTURE on the wire =====================
//
// APPENDED AT THE FILE TAIL so no `core::panic::Location` in this file moves; every edit above is
// line-for-line in place. See `drivers/block.rs`'s SDWRITE section for the posture itself.

/// SDWRITE: the `rw=` WORD, from a veto. One mapping, used by the native root's announce and driven
/// BOTH WAYS by leg 8 — so inverting it is a red fixture and not a quiet lie on the wire.
#[cfg(all(target_arch = "aarch64", feature = "sdwrite"))]
fn native_root_rw_word(veto: Option<&'static str>) -> &'static str {
    if veto.is_none() { "yes" } else { "no" }
}

/// SDWRITE (A60, second finding): bind `/` to the native volume and announce it with a posture
/// sampled from THE BACKEND BEING MOUNTED.
///
/// The pre-A60 arm mounted a `NativeBackend` and then printed `rw=` from `BlockSource::write_veto()`
/// — a question about a different object. That is the same defect `HOMESOIL` had already fixed for
/// `/volumes/` and `/boot` ("ONE sample, from the backend being mounted — not a second derivation of
/// the posture"), left standing on the one mount where it mattered most. Here the sample comes off
/// the very backend handed to `mt.mount`, and `NativeBackend::write_veto` forwards the block layer's
/// answer for the handle the shared unafs mount is riding — so `rw=yes` on `/` is a statement about
/// the disk, not a hope about it.
#[cfg(all(target_arch = "aarch64", feature = "sdwrite"))]
fn native_root_mount(mt: &mut crate::fs::vfs::MountTable, src: BlockSource, announce: bool) {
    use crate::fs::vfs::VfsBackend;
    let be = crate::fs::vfs::NativeBackend::new("native");
    let rw = native_root_rw_word(be.write_veto());
    mt.mount("/", alloc::boxed::Box::new(be));
    if announce {
        // The discriminating words stay LITERALS in the format string — the `/volumes/` lesson: an
        // artifact census (`LC_ALL=C grep -a -o -F`) must be able to find the sentence it certifies.
        serial_println!(
            "[vfs] root mount / = native unafs volume source={} rw={} ::",
            src.name(),
            rw
        );
    }
}

/// SDWRITE leg 8 — THE POSTURE/POLARITY MATRIX, and it runs on QEMU `virt`.
///
/// WHAT IT CAN AND CANNOT PROVE, said first. **No QEMU machine models the Tegra234 SDHCI** — `arroyo`
/// launches `q35`, `raspi4b` and generic `virt`, and zero tegra machines — so nothing here issues a
/// CMD24 or touches a card. What IS testable without the controller is the thing A60 was opened
/// about: the REPORT. The pre-A60 tree could print `rw=yes` over a path that could not write one
/// byte, because the veto was decided in `drivers/block.rs` and re-stated in `fs/fat.rs`, and two
/// statements of one answer drift. This leg asserts they are one answer.
///
/// Three claims, each driven rather than read:
///  1. **The mapping, both ways.** `native_root_rw_word(None) == "yes"` and `…(Some(_)) == "no"` —
///     the same function the native root's announce uses. Invert it and this reds.
///  2. **One answer per source.** For EVERY source in `fat::ALL_SOURCES`, the FAT layer's
///     `BlockSource::write_veto()` and the block layer's `handle_write_veto(handle_of(src))` agree on
///     whether a write is admitted. A second policy in either layer reds this the moment it differs —
///     which is exactly the drift the `TegraSd` arm was (a veto that only ECHOED one two layers down).
///  3. **The polarity that SHIPS.** `posture=` is `cfg!(feature = "sdwrite")`, printed so a capture
///     says which build it came from; and on a build that HAS the card handle, `TegraSd`'s veto is
///     asserted to match it in both directions. On a build without `tegra`+`sdmmc` the field reads
///     `card=absent` — the leg says what it did not test instead of passing quietly.
#[cfg(all(feature = "witness", feature = "sdwrite"))]
pub fn sdwrite_posture_selftest() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let posture = cfg!(feature = "sdwrite");

    // (1) the mapping, driven both ways.
    #[cfg(target_arch = "aarch64")]
    let map_ok = native_root_rw_word(None) == "yes" && native_root_rw_word(Some("refused")) == "no";
    // `NativeBackend` and its announce are aarch64-only, so on x86 there is no mapping to drive and
    // the leg says so rather than asserting a function that does not exist.
    #[cfg(not(target_arch = "aarch64"))]
    let map_ok = true;
    #[cfg(target_arch = "aarch64")]
    const MAP_FIELD: &str = "yes/no";
    #[cfg(not(target_arch = "aarch64"))]
    const MAP_FIELD: &str = "n/a(x86)";

    // (2) one answer per source — the FAT view and the block view, compared for every source.
    let mut agree = true;
    let mut census = String::new();
    for src in fat::ALL_SOURCES {
        let fat_admits = src.write_veto().is_none();
        let blk_admits =
            crate::drivers::block::handle_write_veto(fat::handle_of(*src)).is_none();
        if fat_admits != blk_admits {
            agree = false;
        }
        if !census.is_empty() {
            census.push(' ');
        }
        census.push_str(src.name());
        census.push('=');
        census.push_str(if fat_admits { "rw" } else { "ro" });
        if fat_admits != blk_admits {
            census.push_str("!DISAGREES");
        }
    }

    // (3) the polarity that ships, on a build that has the card handle.
    #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
    let (card_field, card_ok) = (
        if crate::fs::fat::BlockSource::TegraSd.write_veto().is_none() { "admits" } else { "refuses" },
        crate::fs::fat::BlockSource::TegraSd.write_veto().is_none() == posture
            && crate::drivers::block::tegra_sd_writes_admitted() == posture,
    );
    #[cfg(not(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc")))]
    let (card_field, card_ok) = ("absent", true);

    let leg8 = map_ok && agree && card_ok;
    serial_println!(
        ":: SDWRITE-POSTURE: posture={} map={} sources {} card={} (no QEMU machine models Tegra SDHCI — this leg tests the REPORT, never the medium) :: {} ::",
        if posture { "on" } else { "off" },
        MAP_FIELD,
        census,
        card_field,
        if leg8 { "PASS" } else { "FAIL" }
    );
}

/// SDWRITE — THE METAL FIXTURE (`UNAOS_SDWRITE=1` → the `sdwritefx` feature).
///
/// One file, one block, four steps, on the root this boot actually bound: create `/SDWRITE.TXT`,
/// write one 512-byte stamped block into it, read it back, delete it. The verdict line is
/// `:: SDWRITE: root=<source> wrote=1 readback=match deleted=1 -> PASS ::`; a FAIL names the STEP
/// that refused, because "it failed" about a four-step sequence is not a diagnosis.
///
/// **CARD SAFETY, stated because this is the first code in this OS that writes an operator's card
/// outside the armed ladder.** The fixture names exactly ONE path, `/SDWRITE.TXT`, at the root of
/// the mounted volume, and it creates it, writes it, reads it and unlinks it. It never opens, moves,
/// truncates or deletes anything else, it never touches `/boot` or `/apps`, it never addresses an
/// LBA itself, and every sector it does reach is one the filesystem chose for that file. The only
/// other bytes that change on the medium are the filesystem metadata a create-plus-delete of one
/// file necessarily rewrites (the FAT/directory entry, or UnaFS's journal and CoW root) — that is
/// inherent to creating a file at all, and it is the whole of the delta. Any write outside that is a
/// DEFECT, and the boot that shows one is evidence against this code, not against the card.
///
/// It is DEFAULT OFF and it is not witness-gated: it is an explicitly armed experiment, so it prints
/// on the boot that armed it and does not exist on any other.
#[cfg(feature = "sdwritefx")]
fn sdwrite_fixture(mt: &crate::fs::vfs::MountTable, src: BlockSource) {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL};
    const PATH: &str = "/SDWRITE.TXT";
    const N: usize = 512;
    // A stamped block: position-varying, so a short or shifted read-back cannot match by accident.
    let mut want: Vec<u8> = Vec::with_capacity(N);
    for i in 0..N {
        want.push((i as u8).wrapping_mul(31) ^ 0x5a);
    }
    let fail = |step: &str, why: &str| {
        serial_println!(
            ":: SDWRITE: root={} step={} why={} -> FAIL ::",
            src.name(),
            step,
            why
        );
    };
    // The posture the root reports, read back from the mount table so the fixture and the wire agree.
    let veto = match mt.write_veto(PATH) {
        Ok(v) => v,
        Err(_) => {
            fail("veto", "no backend resolves the root");
            return;
        }
    };
    if let Some(why) = veto {
        fail("veto", why);
        return;
    }
    if mt.create(PATH, NodeKind::File, KERNEL_PRINCIPAL).is_err() {
        fail("create", "the root refused to create SDWRITE.TXT");
        return;
    }
    let wrote = match mt.write(PATH, 0, &want, KERNEL_PRINCIPAL) {
        Ok(n) if n == N => 1u32,
        Ok(_) => {
            fail("write", "short write");
            let _ = mt.unlink(PATH, KERNEL_PRINCIPAL);
            return;
        }
        Err(_) => {
            fail("write", "the write was refused");
            let _ = mt.unlink(PATH, KERNEL_PRINCIPAL);
            return;
        }
    };
    let back = match mt.read(PATH, 0, N) {
        Ok(b) => b,
        Err(_) => {
            fail("readback", "the read-back was refused");
            let _ = mt.unlink(PATH, KERNEL_PRINCIPAL);
            return;
        }
    };
    let matched = back.len() == N && back.as_slice() == want.as_slice();
    if !matched {
        fail("readback", "the bytes that came back are not the bytes that went down");
        let _ = mt.unlink(PATH, KERNEL_PRINCIPAL);
        return;
    }
    if mt.unlink(PATH, KERNEL_PRINCIPAL).is_err() {
        fail("delete", "SDWRITE.TXT could not be removed — IT IS STILL ON THE CARD");
        return;
    }
    if mt.stat(PATH).is_ok() {
        fail("delete", "SDWRITE.TXT still resolves after unlink");
        return;
    }
    serial_println!(
        ":: SDWRITE: root={} wrote={} readback=match deleted=1 -> PASS ::",
        src.name(),
        wrote
    );
}

// =========================================================================================
// AHCIBOOT (rmbp-ledger B89, second rung) — the SATA disks enter the walk, and the wire fixture
// that proves the whole chain end to end.
//
// APPENDED AT THE FILE TAIL, and the one change above is a LINE-NEUTRAL fold, for the reason the
// RMDIR-UNAFS block already states: `panic::Location` embeds source line numbers, so a line
// inserted mid-file moves every panic site below it in the knob-OFF image.
//
// ### The walk needed ONE change, and it is not in this file
//
// `walk_and_witness` iterates `fat::live_sources()`. That function is where USBREG expanded the USB
// rung over the block registry, and it is where this arc expands the SATA rung over the AHCI
// registry — appended at the END of the list. So the walk gains the SATA disks without a second
// walker, which is the invariant `ALL_SOURCES`' own doc comment states: a board that grows a disk
// gains it in both places or in neither. Every consequence follows from that one line:
//
//   * `admit` deduplicates a SATA disk against the USB stick by the SAME `fat::same_device`
//     proof every other pair goes through — and the AHCI registry's key is the HBA PORT, from the
//     enumerator, so two identically sized SATA disks are two disks and the walk says so;
//   * each SATA volume is probed BY CONTENT — `mount_source` tries superfloppy, then GPT, then the
//     MBR slots, reading sector 0 and each partition's BPB off the medium (LAWS §3: root is the
//     volume the kernel was found on, BY CONTENT);
//   * `plan` hands every non-root SATA volume a `/volumes/<LABEL>` point with its OWN write posture,
//     which for a SATA volume is always READ-ONLY; and
//   * root binding is unchanged where a machine still boots off USB, because the SATA rung is
//     APPENDED and `plan` picks the FIRST disk carrying this kernel. A SATA disk becomes `/` only
//     when nothing earlier carries the kernel — which is exactly the "UnaOS installed on and
//     booting from the internal disk" case B89 is aiming at.
//
// ### Why the fixture does not call `survey()`
//
// `survey` CACHES its first answer for the boot, and this fixture runs at PCI enumeration time —
// long before USB storage finishes its deferred SCSI bring-up. Driving the cached walk from here
// would latch a survey taken with no USB disk registered and hand every later caller a root of
// NONE: the hazard `fs/users.rs` already records in as many words. So the fixture walks the SATA
// rung directly, through the very functions the walk uses, and leaves the cache untouched.

/// AHCIBOOT: append ` ahci<p>=present` for every live SATA disk, in registry-index order.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
fn push_ahci_census(out: &mut String) {
    for ix in 0..crate::drivers::block::MAX_AHCI_DISKS {
        if let Some(port) = crate::drivers::block::ahci_port_at(ix) {
            out.push(' ');
            out.push_str(BlockSource::Ahci(port).name());
            out.push_str("=present");
        }
    }
}

/// No SATA handle in this image, so the census string is byte-identical to its pre-AHCIBOOT self.
#[cfg(not(all(target_arch = "x86_64", feature = "ahci")))]
#[inline(always)]
fn push_ahci_census(_out: &mut String) {}

/// AHCIBOOT: the marker file the QEMU fixture stages on the SATA disk's FAT volume.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
const AHCIBOOT_FILE: &str = "AHCIBOOT.TXT";

/// AHCIBOOT: how many bytes of it, and what they are.
///
/// 4096 bytes — EIGHT whole sectors — on purpose, and the size is the instrument. A whole-sector run
/// goes through `fat::read_sectors`, i.e. the COUNTED `read_blocks_ahci_port` path, so the fixture
/// exercises the multi-sector arm rather than only the single-sector one a short file would touch.
/// The content is generated, not stored: byte `i` is `b'A' + (i % 26)`, which the host stages with
/// the same rule. A read that returns zeros — the go-red mutation — fails the comparison on byte 0.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
const AHCIBOOT_BYTES: usize = 4096;

/// AHCIBOOT: the expected byte at offset `i` of [`AHCIBOOT_FILE`].
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
#[inline]
fn ahciboot_expect(i: usize) -> u8 {
    b'A' + (i % 26) as u8
}

/// AHCIBOOT: FNV-1a 64 over the bytes that came back — a READABLE FINGERPRINT for the wire, not the
/// verdict. The verdict is the byte-by-byte comparison against [`ahciboot_expect`], which is why a
/// non-cryptographic digest is enough here and why nothing downstream keys on this number.
///
/// **It is not `sha=` and the reason is a real finding, not a preference.** `crate::hash` — which
/// carries this tree's one SHA-256 — is `#[cfg]`-gated on a feature list (`lib.rs:103-109`:
/// `installdemo`, `install_target`, `piinstall`, `selfhost`, `holocron`, `selfup`, `facet`, `login`,
/// `ga10bprobe5`) that `ahci` is not on. `./arroyo check`'s `x86-all` leg carries several of those,
/// so the first cut of this fixture COMPILED GREEN under `check` and then failed to build the
/// `test` artifact with `E0433: cannot find hash in crate` — LAWS §5's "an instrument's presence is
/// proven in the artifact, never in the check", paid for in one build. Adding `feature = "ahci"` to
/// that list is a one-term same-line append (the convention that line already documents) and is the
/// right fix, but it is a MODULE DECLARATION LINE in `lib.rs`, which this brief forbids touching
/// (FC2CHECK). Reported instead of taken; until then the wire says `fnv=` and means it.
///
/// The constants are FNV-1a 64's own (offset basis 0xcbf29ce484222325, prime 0x100000001b3), the
/// same pair `video::paper::fnv1a` uses — it is `pub(super)` and therefore unreachable from here,
/// so this is a second CALL SITE of one published algorithm rather than a second policy.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
fn ahciboot_fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// AHCIBOOT: the wire fixture — every SATA volume this machine has, listed BY CONTENT, and the
/// marker file read back through the VFS over the AHCI source.
///
/// Driven from `drivers::ahci::probe`'s tail: the last statement of the one enumeration pass, with
/// every HBA and port lock released and the kernel heap long since up (`main.rs` allocates it before
/// `pci::init`). It is the only site on the x86 boot path that runs AFTER the AHCI registry is
/// populated and does not require an operator — `fs::bootdisk::bind` is reached from
/// `shell::vfs_mount_table`, whose SATA-bearing arm is `target_arch = "aarch64"`, so on x86 the walk
/// itself is never driven on a headless boot (measured: `[vfs]` 0 lines on the AHCI arc's capture).
/// That is a REPORTED gap, not one this arc closes — see the module doc and this arc's report.
///
/// **It is a READER and nothing else.** It mounts, lists, reads and drops. No sector is written, and
/// none could be: `write_block_ahci` refuses in every cfg and the image compiles no ATA write opcode.
///
/// It can say NO in distinguishable ways, which is why all of them print:
/// * no `[bootdisk]` line at all → the AHCI registry is empty, i.e. no SATA disk answered IDENTIFY.
///   The producing path still ran and said so on the closing line, so the silence is never read as
///   a pass (LAWS §5);
/// * `[bootdisk] … vol=- ` → the disk is readable and carries no FAT volume this reader accepts;
/// * `-> FAIL` → a SATA volume carried the marker file and the bytes that came back are not the
///   bytes the host staged. `-> FAIL` is in `mbench.py`'s `DEFAULT_FORBIDS`, so that reds the run.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn ahciboot_selftest() {
    use crate::fs::vfs::{FatBackend, MountTable, KERNEL_PRINCIPAL};
    static RAN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if RAN.swap(true, core::sync::atomic::Ordering::Relaxed) {
        return;
    }

    let mut sata = 0u32;
    let mut volumes = 0u32;
    let mut staged = 0u32;

    // The SATA rung of the walk's own source list — `fat::live_sources()`, filtered to the sources
    // this arc appended. Reading it here rather than re-deriving it from the registry is the point:
    // if the expansion were missing, this loop would find nothing and the closing line would say so.
    for src in crate::fs::fat::live_sources() {
        let port = match src {
            BlockSource::Ahci(p) => p,
            _ => continue,
        };
        sata += 1;
        let blocks = fat::source_blocks(src).unwrap_or(0);
        // BY CONTENT: `volume_serials` walks the superfloppy BPB, every GPT entry and every MBR slot
        // and parses each candidate's BPB off the medium. It is the same candidate set `mount_source`
        // trusts, so a volume named here is a volume the mount path can bind.
        let serials = fat::volume_serials(src);
        let (label, vol_id) = match fat::mount_source(src) {
            Ok(fs) => (sanitize_label(&fs.label_raw()).0, fs.volume_fingerprint().0),
            Err(_) => (String::from("-"), 0),
        };
        if vol_id != 0 || label != "-" {
            volumes += 1;
        }
        serial_println!(
            "[bootdisk] volume source=ahci port={} vol={} serial=0x{:08x} blocks={} volumes_by_content={} ::",
            port, label, vol_id, blocks, serials.len()
        );

        // The marker file, read through the VFS — a `FatBackend` over this source at `/`, exactly
        // the backend `bind` mounts a home-soil volume with. That is what makes this a proof about
        // the CHAIN (VFS -> FAT -> BlockSource::Ahci -> block registry -> AHCI driver -> the wire)
        // rather than about any one layer.
        let mut mt = MountTable::new();
        mt.mount(
            "/",
            alloc::boxed::Box::new(FatBackend::new_source("ahciboot", KERNEL_PRINCIPAL, true, src)),
        );
        let path = alloc::format!("/{}", AHCIBOOT_FILE);
        if mt.stat(&path).is_err() {
            continue;
        }
        staged += 1;
        let got = match mt.read(&path, 0, AHCIBOOT_BYTES) {
            Ok(v) => v,
            Err(_) => {
                serial_println!(
                    ":: AHCIBOOT: source=ahci port={} vol={} found={} bytes=0 fnv=---------------- -> FAIL ::",
                    port, label, AHCIBOOT_FILE
                );
                continue;
            }
        };
        let fnv = ahciboot_fnv1a(&got);
        let ok = got.len() == AHCIBOOT_BYTES
            && got.iter().enumerate().all(|(i, b)| *b == ahciboot_expect(i));
        serial_println!(
            ":: AHCIBOOT: source=ahci port={} vol={} found={} bytes={} fnv={:016x} -> {} ::",
            port,
            label,
            AHCIBOOT_FILE,
            got.len(),
            fnv,
            if ok { "PASS" } else { "FAIL" }
        );
    }

    // The closing census. It prints unconditionally, so "no SATA disk" and "the fixture never ran"
    // are different lines on the wire rather than the same silence.
    serial_println!(
        "[bootdisk] ahci census: sata_sources={} with_fat_volume={} carrying_{}={} ::",
        sata, volumes, AHCIBOOT_FILE, staged
    );
}

// ===================== PLANWALK (2026-09-15) — the plan/walk invariant, in code =====================
//
// APPENDED AT THE FILE TAIL so no `core::panic::Location` in this file moves; the one change above is
// an EIGHT-LINE-FOR-EIGHT-LINE replacement and the fixture's call is a same-line fold. Same
// discipline, and the same reason, as the SDWRITE and AHCIBOOT blocks above.
//
// THE DEFECT, and it was REPORTED rather than found by a gate. `plan` picks `root_ix` with
// `position(|d| !d.hits.is_empty())`, so the disk it names always has at least one hit — and
// `walk_and_witness` read `d.hits[0]` on that strength. The two facts lived in two functions with
// nothing joining them. X86BIND's one-line go-red turned the gap into a KERNEL PANIC in the boot
// path (`bootdisk.rs:1007 index out of bounds`), on a machine with no console yet, and that seat
// wrote it into the queue as a STOP. LAWS §5 names the shape: an invariant nobody checks is an
// invariant nobody has.
//
// WHY A FUNCTION AND NOT AN ASSERT. `debug_assert!` is compiled out of every image we ship or boot,
// so it cannot fire where the defect lives; a `const _` cannot see a runtime index at all. The join
// has to be code that RUNS in release, and the honest answer to a broken invariant here is not to
// stop the machine — it is to bind no root and say why, which is a road the walk already has.

/// PLANWALK: the hit `walk_and_witness` may read for the disk `plan` named, or `None` — after one
/// witness line — when the planner named a disk with no hits at all.
///
/// Both refusals are real: `disks.get` answers an index past the end of the list (the shape a future
/// planner bug would take), and `hits.first()` answers the empty-hits case (the shape X86BIND's
/// go-red produced). Neither can panic, and the caller's `and_then` sends both onto the walk's
/// existing no-root road.
fn plan_root_hit(disks: &[Disk], root_ix: usize) -> Option<&Hit> {
    let d = disks.get(root_ix)?;
    match d.hits.first() {
        Some(h) => Some(h),
        None => {
            serial_println!(
                "[vfs] plan-walk INVARIANT BROKEN root_ix={} hits=0 source={} disks={} :: plan() \
                 named a disk with no matching file and walk_and_witness would have indexed it — \
                 no root is bound on this pass ::",
                root_ix,
                d.source.name(),
                disks.len()
            );
            None
        }
    }
}

/// PLANWALK: HOMESOIL leg 8 — the guard, fed the state that used to panic.
///
/// This is the only place the broken state can be produced on purpose: `plan` cannot name a hitless
/// disk, which is precisely why nothing was measuring what happens when the two functions disagree.
///
/// FALSIFIABLE IN BOTH DIRECTIONS, and the negative one is the point. A disk with NO hits must come
/// back `None` **and the machine must still be running to print this line** — a panicking guard
/// never reaches the `serial_println!`, which is the "no panic" half stated as a measurement rather
/// than as an absence. A disk WITH a hit must come back carrying THAT hit, so a guard that simply
/// answered `None` to everything would red here instead of silently disabling root binding on every
/// boot. The third leg drives an index past the end of the list, the other way `get` can be wrong.
#[cfg(feature = "witness")]
fn planwalk_selftest() {
    let mut hitless: Vec<Disk> = Vec::new();
    assert_admit(
        &mut hitless,
        BlockSource::Default,
        DiskId { num_blocks: 100, vol_id: 0xdead_0001 },
        Some(synth_dev(9, 100)),
        *b"NOHITS     ",
        None,
    );
    // The guard's line is the SAME line a real break would print, so say whose it is BEFORE it
    // appears. LAUNCH-AR's rule, inverted: here it is the FIXTURE that must not be mistaken for the
    // witness — a reader who meets `INVARIANT BROKEN` in a log is entitled to know which one it is.
    serial_println!(
        ":: HOMESOIL: planwalk — the next [vfs] plan-walk INVARIANT BROKEN line is THIS FIXTURE's, \
         fed a synthetic disk with hits=0; a real one would name the boot walk's own disks ::"
    );
    let guarded = plan_root_hit(&hitless, 0).is_none();
    hitless[0].hits.push(Hit { path: String::from("/KERNEL8.IMG"), file_off: 7 });
    let control = plan_root_hit(&hitless, 0).map(|h| (h.path.clone(), h.file_off))
        == Some((String::from("/KERNEL8.IMG"), 7));
    let past_end = plan_root_hit(&hitless, 9).is_none();
    let leg8 = guarded && control && past_end;
    serial_println!(
        ":: HOMESOIL: planwalk hitless_guarded={} hit_found={} past_end_guarded={} alive=yes :: {} ::",
        guarded,
        control,
        past_end,
        if leg8 { "PASS" } else { "FAIL" }
    );
}
