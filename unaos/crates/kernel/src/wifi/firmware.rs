// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// WIFI-1 — locate, validate and stage the user-supplied firmware SET from the program-source volume.
//
// ## UnaOS ships no firmware
// `docs/MANIFESTO/CLEAN_ROOM_POLICY.md` §4: proprietary blobs are supplied by the USER at runtime.
// This module reads files the user placed on the media. It never synthesizes microcode — GR22 and
// `bcm4331.md` §S4 both settle that authoring is not the route — and it never fetches one.
//
// ## The set is THREE files, not one
// `bcm4331.md` §S4 pins what the d11 core needs, from measured core rev 29 + PHY type 7 (HT), three
// boots: microcode `ucode29_mimo.fw`, register initvals `ht0initvals29.fw`, and band-switch initvals
// `ht0bsinitvals29.fw`. No PCM image (rev 29 does not use one). A single blob cannot feed arc 2, so
// the loader stages the whole set and its terminal verdict is all-present or names what is missing.
//
// ## Where the files must be
// The loader reads the PROGRAM SOURCE first — `block::program_source()`, which since PSRC (GR26)
// prefers the handle carrying the boot-volume serial and falls back to the historical global-then-
// `Sdhc` order. That is the FAT-verb law — a "wherever I can find it" read FOLLOWS THE PROGRAM
// SOURCE — and firmware is exactly that class of read. Reaching for `mount()` instead would find
// nothing on a machine booted from the internal reader, where the program-bearing volume is mounted
// and the global handle is empty at the same time.
//
// **PSRC (GR26) — and then the OTHER populated handle, for any role still missing.** See
// [`stage_attempt`]'s doc for the argument. The short form: these blobs are user-supplied and travel
// on whichever medium the user plugged in, this module writes no sector, and a role already staged is
// never replaced — so widening the search here costs nothing the preference was protecting.
//
// Directories, in order: the volume root, then `/B43/`, then `/FIRMWARE/`. Within each directory each
// file is tried by its canonical b43 name first, then by an 8.3 alias — the canonical names are
// 14-19 characters and need VFAT long-file-name entries, which the reader supports (PI-FS-3) but a
// plainly-formatted volume may not carry. The aliases make the set stageable either way.
//
// **First existing name wins, per file.** A file that exists but fails validation is REJECTED and the
// loader does NOT fall through to the next alias or directory for that role: a present-but-wrong
// `ucode29_mimo.fw` is a fact about the user's media that must be reported, not routed around.
//
// READS ONLY — this module never writes a sector, never creates a directory entry, never mutates the
// FAT.
//
// ## What is validated, and how honestly
//
//   * **Presence** — pinned. Reported per file.
//   * **Bounds** — pinned by us, not by the vendor. `MIN_BLOB_BYTES`/`MAX_BLOB_BYTES` bracket the
//     sizes §S4 describes ("tens of KB for the ucode, a few KB each for the initvals") with generous
//     headroom. Outside the band is a hard REJECT, because staging a 4 GiB "firmware" into the kernel
//     heap is a worse failure than refusing it. A short read (chain ends before the directory size)
//     is likewise a REJECT, never a silent truncation.
//   * **Container header — W3 ANSWERED ON METAL (rmbp1-boot1, 2026-08).** The old A-vs-B
//     "documented disagreement" is resolved: the header is ALWAYS the 8-byte form — `type` u8,
//     `ver` u8, 2 reserved, `be32 size` — and what varies per file is the MEANING of `size` and the
//     SHAPE of the payload, selected by the `type` byte:
//
//       - **`type` 0x75 (`hdr=words`)** — the microcode: `size` is the payload BYTE count
//         (`size == len - 8`), payload a stream of be32 words. Measured: ucode29_mimo.fw
//         len=39760 declared=39752, whole words.
//       - **`type` 0x69 (`hdr=records`)** — the initvals: `size` is the payload RECORD count, and
//         the payload is a stream of variable-length records — `be16 offset` (bit 15 flags a
//         32-bit value) followed by a `be16` or `be32` value (4 or 6 bytes per record). Measured:
//         ht0initvals29.fw declared=477, walks as exactly 477 records consuming the payload
//         exactly; ht0bsinitvals29.fw declared=35, likewise exact — the same file both old
//         layouts REJECTED (178 bytes: not len-8, not whole words).
//
//     PROVENANCE of the record framing: hypothesis-falsification against the user's own extracted
//     set on this bench plus the rmbp1-boot1 capture — the framing above is the one reading under
//     which both initvals files' walked record counts equal their declared `size` while consuming
//     their payloads exactly. No driver source was read. What the walk pins is the FRAMING; what
//     the `offset` field means to the d11 core is arc-3 evidence, not this module's claim.
//     `classify_header` therefore VERIFIES rather than guesses: a `records` verdict is issued only
//     when the walk itself succeeds with the declared count.
//
// ## Failure posture — SETTLED vs DEFERRED, and why the difference is not cosmetic
// Every file gets one line and the pass ends with exactly one terminal verdict. No panic.
//
// One pass is not always one boot's answer, and WIFI-REARM is the correction. The pass runs only
// after `block::program_source()` reports a handle, but a registered handle is not a settled
// transport: on the rMBP the USB stick's block device is published by the xHCI storage bring-up and
// the first mount through it can still return `NoDisk`/`Io`/`Busy` while that transport finishes
// coming up. Treating that as the boot's terminal answer spends the one attempt on a "not yet".
//
// So [`stage_attempt`] classifies its own failure instead of assuming it is final:
//
//   * **`Settled`** — every volume that was tried mounted and each role got its verdict, OR a mount
//     failed for a reason a later pass cannot change (`NotFat`, `Unsupported`, a corrupt chain): a
//     volume that is not FAT now will not be FAT in two seconds. One terminal verdict, printed here.
//   * **`Retry(stage)`** — a mount, or a root-directory read, failed for a reason that CAN change
//     (`NoDisk`, `Io`, `Busy`). Nothing terminal was printed; the caller re-attempts under its own
//     bounded budget and prints the exhaustion line if it runs out.
//   * **`Pending`** (WIFI-REACH, GR26) — every volume PRESENT this attempt was searched and the set
//     is still incomplete, but no SECOND handle existed to search, so a volume that has not yet
//     enumerated could still carry the missing roles. Nothing terminal was printed; the caller holds
//     arc 2 and the verdict and re-attempts on the USB storage-ready edge or a bounded deadline. It
//     is returned only on a non-committing attempt — see [`stage_attempt`]'s `commit` argument — so
//     the "exactly one terminal verdict per boot" rule is preserved: `Pending` is never the last
//     word, the committing attempt always prints COMPLETE or INCOMPLETE.
//
// ## PSRC (GR26) meets WIFI-REARM — where the no-double-stage guarantee now lives
// Pre-PSRC, both `Retry` arms sat ABOVE the `for spec in FW_SET` loop, so a retry was by construction
// reachable only before any role had been staged: that placement was the whole no-double-stage
// argument. PSRC widens the search to a SECOND volume ([`stage_pass`] over the program source, then
// the alternate handle), so a `Retry` can now be returned by the alternate volume AFTER the program
// source already staged one or two roles — the arm-placement argument no longer holds on its own.
//
// The guarantee is preserved by a stronger mechanism that PSRC brought with it: [`stage_pass`] skips
// any role already present in `STAGED` (`with_staged(...).is_some()`), on every volume and every
// re-attempt. So a role staged on volume 1 is never restaged on volume 2, and a role staged on
// attempt N is never restaged on attempt N+1. `STAGED` still needs no reset between attempts — not
// because a retry stages nothing (it may now resume a half-built set on purpose), but because the
// per-role skip makes resuming idempotent. The `Retry` return still prints nothing terminal, so the
// "exactly one terminal verdict" rule holds across the whole two-volume, multi-attempt search.
//
// ## FWPIN (rmbp-ledger B455, SEC-2026-10-06 F5) — pinned, boot card only, malformed refused
// A name is not an identity. Every candidate is SHA-256'd (`crypto_core::sha2`, the kernel's one SHA-256)
// and admitted ONLY when the digest equals its role's row in `unaos/firmware/b43.pins` — a file of FACTS
// about Peter's bunker copy (role + digest), never bytes. No row => `REJECTED … reason=unpinned sha256=…`
// (the digest is printed so the seat can pin it against arroyo's WIFI-FW build line); a different digest
// => `reason=pin-mismatch`; a `violates-layout`/unrecognized container => `reason=violates-layout`. Nothing
// refused enters `STAGED`, so no consumer (arc 2's upload) can ever see an unpinned image.
//
// The search is the BOOT CARD's own volumes only (R95: a removable volume is data, never a microcode
// source): pass 1 the program source's FAT (the ESP), pass 2 the same card's native UnaFS root — where
// arroyo's WIFI-FW block puts the set (`/FIRMWARE/`) — and only when the native volume is bound to the
// program-source handle. PSRC's alternate-handle pass (a USB stick) is GONE, and with it WIFI-REACH's
// `Pending`: no second volume is awaited, so every attempt that is not a `Retry` is terminal.
//
// ## Sourcing (see the note in `mod.rs`)
// File names, core revision and PHY type come from `bcm4331.md` §S4, which states its own sourcing
// separately and differs from this file's — see the cross-reference in `mod.rs`. Nothing here was
// derived from Linux driver source.

use alloc::string::String;
use alloc::vec::Vec;
use crate::sync::Mutex;

use crate::fs::fat::{self, DirEntry, FatError, FatFs};

/// One member of the firmware set.
struct FwSpec {
    /// Short role tag used in the witness lines and by arc 2 to select a staged image.
    role: &'static str,
    /// Accepted filenames, in search order: canonical b43 name first, then 8.3 aliases.
    names: &'static [&'static str],
}

/// The set `bcm4331.md` §S4 pins for d11 core rev 29 + HT PHY. `B43.FW` is retained as a legacy
/// alias for the microcode only — it was WIFI-1's original single-blob name and some media may
/// already carry it — but it is not the documented staging name.
const FW_SET: &[FwSpec] = &[
    FwSpec { role: "ucode",      names: &["ucode29_mimo.fw",     "UCODE29.FW", "B43.FW"] },
    FwSpec { role: "initvals",   names: &["ht0initvals29.fw",    "HT0IV29.FW"] },
    FwSpec { role: "bsinitvals", names: &["ht0bsinitvals29.fw",  "HT0BSI29.FW"] },
];

/// How many members a COMPLETE set has. Exported so arc 2's completeness gate reads the same number
/// this module's own terminal verdict does — a second literal in `bringup.rs` is exactly how "3/3
/// staged" and "the set is complete" would drift apart without either line changing.
///
/// `wifi2`-gated because arc 2 is its only consumer: this module's own verdict uses `FW_SET.len()`
/// directly, so leaving the alias ungated would emit an unused-constant warning on every arc-1-only
/// build. Both spellings read the same array, which is the point.
#[cfg(feature = "wifi2")]
pub(crate) const FW_SET_LEN: usize = FW_SET.len();

/// Directories searched, in order. `None` = the volume root.
const SEARCH_DIRS: &[Option<&str>] = &[None, Some("B43"), Some("FIRMWARE")];

/// Below this a file cannot be one of these images — it is a stub, a placeholder, or truncated.
/// §S4's "a few KB each" overstated the smallest member: the real `ht0bsinitvals29.fw` measured on
/// metal (GR27, 2026-08) is 178 bytes, so the floor sits below that while still rejecting stubs.
const MIN_BLOB_BYTES: u32 = 128;
/// Above this we refuse rather than commit the kernel heap. §S4: the ucode is "tens of KB"; 4 MiB is
/// ~two orders of magnitude of headroom and still bounded.
const MAX_BLOB_BYTES: u32 = 4 * 1024 * 1024;

/// FWPIN (B455): the pin file, compiled in. Rows `<role> <64 hex>`; `#` lines and blanks ignored. Ships
/// EMPTY of rows: the digests come from Peter's bunker copy via arroyo's WIFI-FW build line, never invented.
const PINS: &str = include_str!("../../../../firmware/b43.pins");

fn nib(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The pinned SHA-256 for `role`, or `None` (no row, or a malformed one — both refuse as `unpinned`).
fn pin_for(role: &str) -> Option<[u8; 32]> {
    for line in PINS.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let (Some(r), Some(h), None) = (it.next(), it.next(), it.next()) else { continue };
        if r != role || h.len() != 64 {
            continue;
        }
        let b = h.as_bytes();
        let mut out = [0u8; 32];
        let mut ok = true;
        for i in 0..32 {
            match (nib(b[2 * i]), nib(b[2 * i + 1])) {
                (Some(hi), Some(lo)) => out[i] = (hi << 4) | lo,
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            return Some(out);
        }
    }
    None
}

fn hex32(d: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in d {
        let _ = core::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b));
    }
    s
}

/// The worst refusal this boot, for `tests wifi`: 0 none, 1 unpinned, 2 violates-layout, 3 pin-mismatch.
static REFUSAL: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// FWPIN: why a role was refused this boot (the worst one), or `None` when nothing was refused.
pub fn refusal() -> Option<&'static str> {
    match REFUSAL.load(core::sync::atomic::Ordering::Relaxed) {
        1 => Some("unpinned"),
        2 => Some("violates-layout"),
        3 => Some("pin-mismatch"),
        _ => None,
    }
}

fn refuse(code: u8) {
    REFUSAL.fetch_max(code, core::sync::atomic::Ordering::Relaxed);
}

/// A staged image and what the loader learned about it.
pub struct StagedImage {
    pub role: &'static str,
    pub path: String,
    pub bytes: Vec<u8>,
    pub digest: u32,
}

static STAGED: Mutex<Vec<StagedImage>> = Mutex::new(Vec::new());

/// How many members of the set are staged this boot. Arc 2 must see `FW_SET.len()`.
pub fn staged_count() -> usize {
    STAGED.lock().len()
}

/// Bytes staged for `role`, if that member was staged. Keeps the buffers inside the module.
pub fn with_staged<R>(role: &str, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
    STAGED.lock().iter().find(|s| s.role == role).map(|s| f(s.bytes.as_slice()))
}

/// One staged image's container verdict and geometry, for arc 2's SET-VALIDATION rung (WIFI-SETVAL).
///
/// Everything here is DERIVED from the staged bytes by this module's own `classify_header` and
/// `fnv1a32` — arc 2 re-derives nothing. W3 is ANSWERED (see the module note), but the one-source
/// rule stands for the same reason it was written: two independent implementations of the container
/// rule is exactly how the loader's `hdr=` verdict and a consumer's payload offset come to disagree
/// without either one looking wrong. One `classify_header`, every reader.
#[cfg(feature = "wifi2")]
pub(crate) struct StagedHeader {
    pub role: &'static str,
    pub layout: &'static str,
    pub kind: u8,
    pub ver: u8,
    /// The header's declared `size`: payload BYTES for `hdr=words`, payload RECORD count for
    /// `hdr=records`.
    pub declared: u32,
    pub len: usize,
    /// Records actually walked. Zero unless `layout == "records"`.
    pub records: u32,
    /// The payload satisfies its layout's shape rule: whole be32 words (`words`), or a clean record
    /// walk whose count equals `declared` (`records`).
    pub stream_ok: bool,
    pub digest: u32,
}

/// The header verdict of every staged member, in [`FW_SET`] order. A COMPLETE set yields
/// [`FW_SET_LEN`] entries; fewer while `staged_count()` says COMPLETE is itself a finding, and arc
/// 2's validation rung reports it as one.
///
/// `wifi2`-gated, along with [`FW_SET_LEN`], so that an arc-1-only build's item set is unchanged: the
/// census-and-staging image stays what it was, rather than "what it was, plus accessors nothing
/// calls".
#[cfg(feature = "wifi2")]
pub(crate) fn set_headers() -> Vec<StagedHeader> {
    let staged = STAGED.lock();
    let mut out = Vec::new();
    for spec in FW_SET {
        if let Some(s) = staged.iter().find(|s| s.role == spec.role) {
            let v = classify_header(&s.bytes);
            out.push(StagedHeader {
                role: s.role,
                layout: v.layout,
                kind: v.kind,
                ver: v.ver,
                declared: v.declared,
                len: s.bytes.len(),
                records: v.records,
                stream_ok: v.stream_ok,
                digest: s.digest,
            });
        }
    }
    out
}

/// Short human reason for a [`FatError`] in the witness lines. A local twin of `fat::fat_reason`.
///
/// The old justification — "which is `#[cfg(target_arch = "aarch64")]`", so an x86-only path would
/// have to widen a shared file's gate to reuse it — is FALSE and has been struck (2026-08-27):
/// VFSX86 (`fs/vfs.rs`, 2026-08-21) un-gated `fat_reason`, which is arch-neutral today. Two real
/// reasons keep the twin:
/// 1. EXHAUSTIVE BY CONSTRUCTION — every `FatError` variant is named here, so a new variant makes
///    this fail to compile rather than silently printing the wrong reason. `fat_reason` ends in a
///    catchall (`_ => "mount failed"`) that would absorb it, and today that catchall already
///    swallows `NoSpace`, `OutOfVolume` and `Busy` — of which `Busy` at least is a documented
///    outcome of a mount here (see the storage-loan note at the top of this file).
/// 2. DIVERGENT WORDING — "no block device" (not "no USB block device": `stage_volume` walks
///    several `BlockSource`s through `fat::mount_source`, so naming USB would be wrong),
///    "unsupported FAT variant/geometry", "entry is a directory".
///
/// Calling `fat_reason` here would change the witness strings AND give up the compile-time check.
fn reason(e: FatError) -> &'static str {
    match e {
        FatError::NoDisk => "no block device",
        FatError::Io => "block I/O error",
        FatError::NotFat => "no FAT partition/BPB found",
        FatError::Unsupported => "unsupported FAT variant/geometry",
        FatError::NotFound => "entry not found",
        FatError::IsDirectory => "entry is a directory",
        FatError::BadChain => "corrupt FAT chain",
        FatError::NoSpace => "no free space",
        FatError::OutOfVolume => "derived LBA outside the volume extent",
        FatError::Busy => "storage driver busy (controller loan held)",
    }
}

/// FNV-1a (32-bit). Not a security check — a boot-to-boot identity so two captures can be compared,
/// and so a re-flashed card that silently changed an image is visible.
fn fnv1a32(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for b in data {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// The `type` byte of a word-stream image (the microcode). Pinned on metal, rmbp1-boot1.
const HDR_TYPE_WORDS: u8 = 0x75;
/// The `type` byte of a record-stream image (both initvals). Pinned on metal, rmbp1-boot1.
const HDR_TYPE_RECORDS: u8 = 0x69;
/// Bit 15 of a record's `be16 offset` field flags a 32-bit value (6-byte record); clear = 16-bit
/// (4-byte record). Pinned by the exact-consume walk over the bench set (module note, PROVENANCE).
const REC_FLAG_32BIT: u16 = 0x8000;

/// Which container shape the file satisfies. See the module note: W3 is answered on metal, and the
/// `records` verdict is issued only when the walk it claims actually succeeds.
struct HdrVerdict {
    kind: u8,
    ver: u8,
    /// The header's declared `size`: payload BYTES (`words`) or payload RECORD count (`records`).
    declared: u32,
    layout: &'static str,
    /// Records actually walked. Zero unless `layout == "records"`.
    records: u32,
    /// The payload satisfies its layout's shape rule (whole be32 words, or a clean count-matching
    /// record walk).
    stream_ok: bool,
}

/// Walk a record-stream payload: `be16 offset` (bit 15 = 32-bit value) then `be16`/`be32` value.
/// Returns the record count iff the walk consumes the payload exactly; a trailing fragment is a
/// refusal, not a truncation.
fn walk_records(payload: &[u8]) -> Option<u32> {
    let mut off = 0usize;
    let mut recs: u32 = 0;
    while off < payload.len() {
        if off + 2 > payload.len() {
            return None;
        }
        let osz = u16::from_be_bytes([payload[off], payload[off + 1]]);
        let need = 2 + if osz & REC_FLAG_32BIT != 0 { 4 } else { 2 };
        if off + need > payload.len() {
            return None;
        }
        off += need;
        recs += 1;
    }
    Some(recs)
}

fn classify_header(data: &[u8]) -> HdrVerdict {
    if data.len() < 8 {
        return HdrVerdict { kind: 0, ver: 0, declared: 0, layout: "shorter-than-header", records: 0, stream_ok: false };
    }
    let kind = data[0];
    let ver = data[1];
    let declared = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let payload = &data[8..];
    match kind {
        // Word-stream: `size` is the payload byte count and the payload is whole be32 words.
        HDR_TYPE_WORDS => {
            let ok = declared as usize == payload.len() && payload.len() % 4 == 0;
            HdrVerdict { kind, ver, declared, layout: "words", records: 0, stream_ok: ok }
        }
        // Record-stream: `size` is the record count; verify by walking, never by arithmetic alone.
        HDR_TYPE_RECORDS => match walk_records(payload) {
            Some(recs) => {
                let ok = recs == declared;
                HdrVerdict { kind, ver, declared, layout: "records", records: recs, stream_ok: ok }
            }
            None => HdrVerdict { kind, ver, declared, layout: "records", records: 0, stream_ok: false },
        },
        _ => HdrVerdict { kind, ver, declared, layout: "unrecognized", records: 0, stream_ok: false },
    }
}

/// Human-readable description of the names searched for one role, built from the spec so the message
/// can never drift from the actual search order.
fn names_description(spec: &FwSpec) -> String {
    let mut s = String::new();
    for (i, name) in spec.names.iter().enumerate() {
        if i > 0 {
            s.push('|');
        }
        s.push_str(name);
    }
    s
}

/// Human-readable description of the directories searched.
fn dirs_description() -> String {
    let mut s = String::new();
    for (i, d) in SEARCH_DIRS.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        match d {
            Some(d) => {
                s.push('/');
                s.push_str(d);
                s.push('/');
            }
            None => s.push('/'),
        }
    }
    s
}

/// Read one directory's entries. `None` = the volume root. A missing directory yields `None` (not an
/// error — that candidate simply cannot match); an unreadable one prints a NON-TERMINAL note and
/// yields `None` so the remaining directories are still tried.
fn entries_in(fs: &FatFs, root: &[DirEntry], dir: Option<&str>) -> Option<Vec<DirEntry>> {
    let Some(d) = dir else {
        return Some(root.to_vec());
    };
    let de = root.iter().find(|e| e.is_dir && e.name().eq_ignore_ascii_case(d))?;
    let first = de.first_cluster();
    if first == 0 {
        // A directory entry whose first cluster is 0 has no chain to walk (an empty or malformed
        // entry). `read_dir` would treat it as an invalid cluster; say so plainly instead.
        serial_println!(":: wifi: /{}/ has no cluster chain (first_cluster=0) — skipping that directory ::", d);
        return None;
    }
    match fs.read_dir(first) {
        Ok(v) => Some(v),
        Err(e) => {
            serial_println!(
                ":: wifi: /{}/ unreadable ({}) — skipping that directory (non-terminal) ::",
                d, reason(e)
            );
            None
        }
    }
}

/// Outcome of one role's search.
enum RoleResult {
    Staged,
    Absent,
    Rejected,
}

/// Locate + validate + stage one member of the set. First existing name wins: a present-but-invalid
/// file REJECTS the role rather than falling through to the next alias.
fn stage_role(fs: &FatFs, root: &[DirEntry], spec: &FwSpec, vol: &str) -> RoleResult {
    for dir in SEARCH_DIRS {
        let Some(entries) = entries_in(fs, root, *dir) else { continue };
        for name in spec.names {
            let Some(de) = entries.iter().find(|e| !e.is_dir && e.name().eq_ignore_ascii_case(name))
            else {
                continue;
            };
            let path = match dir {
                None => alloc::format!("/{}", name),
                Some(d) => alloc::format!("/{}/{}", d, name),
            };

            // Bounds gate. Refuse before allocating, so a bogus size can never commit the heap.
            if de.size < MIN_BLOB_BYTES {
                serial_println!(
                    ":: wifi: {} REJECTED {} size={} on {} — reason=too-small (min {}) ::",
                    spec.role, path, de.size, vol, MIN_BLOB_BYTES
                );
                return RoleResult::Rejected;
            }
            if de.size > MAX_BLOB_BYTES {
                serial_println!(
                    ":: wifi: {} REJECTED {} size={} on {} — reason=too-large (max {}) ::",
                    spec.role, path, de.size, vol, MAX_BLOB_BYTES
                );
                return RoleResult::Rejected;
            }

            // Exact-size reservation: `read_file` extends the Vec sector by sector, and letting it
            // grow by doubling would put a 4 MiB image in up to 8 MiB of capacity and spike a ~12 MiB
            // transient against a 48 MiB heap — retained for the rest of the boot.
            let mut data: Vec<u8> = Vec::new();
            data.reserve_exact(de.size as usize);
            if let Err(e) = fs.read_file(de, &mut data, de.size as usize) {
                serial_println!(
                    ":: wifi: {} REJECTED {} size={} on {} — reason=read-failed ({}) ::",
                    spec.role, path, de.size, vol, reason(e)
                );
                return RoleResult::Rejected;
            }
            if data.len() != de.size as usize {
                // `read_file` returns a short read for a chain that ends early. Refuse it: half a
                // microcode image pushed into the core is worse than no radio.
                serial_println!(
                    ":: wifi: {} REJECTED {} size={} on {} — reason=short-read (got {} bytes) ::",
                    spec.role, path, de.size, vol, data.len()
                );
                return RoleResult::Rejected;
            }

            return admit(spec, path, vol, data);
        }
    }
    RoleResult::Absent
}

/// FWPIN: the one gate every candidate passes, whichever volume it was read from. Layout first (a
/// malformed container is refused outright), then the SHA-256 pin; only a pinned match enters `STAGED`.
fn admit(spec: &FwSpec, path: String, vol: &str, data: Vec<u8>) -> RoleResult {
    let v = classify_header(&data);
    let digest = fnv1a32(&data);
    let sha = crypto_core::sha2::sha256(&data);
    let len = data.len();
    if !v.stream_ok {
        refuse(2); refused_ui(spec.role, "violates-layout"); // REFUSALUI (B468): the refusal is shown, not only printed
        serial_println!(
            ":: wifi: {} REJECTED {} bytes={} on {} — reason=violates-layout hdr={} type={:#04x} declared={} records={} sha256={} ::",
            spec.role, path, len, vol, v.layout, v.kind, v.declared, v.records, hex32(&sha)
        );
        return RoleResult::Rejected;
    }
    match pin_for(spec.role) {
        None => {
            refuse(1); refused_ui(spec.role, "unpinned"); // REFUSALUI (B468): the refusal is shown, not only printed
            serial_println!(
                ":: wifi: {} REJECTED {} bytes={} on {} — reason=unpinned sha256={} (no `{}` row in unaos/firmware/b43.pins; pin it from the WIFI-FW build line) ::",
                spec.role, path, len, vol, hex32(&sha), spec.role
            );
            return RoleResult::Rejected;
        }
        Some(pin) if pin != sha => {
            refuse(3); refused_ui(spec.role, "pin-mismatch"); // REFUSALUI (B468): the refusal is shown, not only printed
            serial_println!(
                ":: wifi: {} REJECTED {} bytes={} on {} — reason=pin-mismatch sha256={} pin={} ::",
                spec.role, path, len, vol, hex32(&sha), hex32(&pin)
            );
            return RoleResult::Rejected;
        }
        Some(_) => (),
    }
    serial_println!(
        ":: wifi: {} STAGED {} bytes={} on {} fnv1a={:#010x} hdr={} type={:#04x} ver={:#04x} declared={} records={} stream=ok sha256={} pin=match ::",
        spec.role, path, len, vol, digest, v.layout, v.kind, v.ver, v.declared, v.records, hex32(&sha),
    );
    STAGED.lock().push(StagedImage { role: spec.role, path, bytes: data, digest });
    RoleResult::Staged
}

/// FWPIN pass 2: the boot card's own native UnaFS root, read through the mount table — where arroyo's
/// WIFI-FW block stages the set (`/FIRMWARE/`). Searched ONLY when `/` is the native volume AND that
/// volume rides the program-source handle: a native volume bound on any other handle is not the boot
/// card, and R95 makes a removable volume data, never a microcode source.
#[cfg(feature = "unafs")]
fn stage_boot_root(dirs: &str, boot: crate::drivers::block::BlockHandle) -> VolOutcome {
    use crate::fs::vfs::{NodeKind, VfsError};
    let mt = crate::shell::vfs_mount_table();
    // `stat("/")` binds the native volume lazily if it is not bound yet; a backend error is transport.
    match mt.stat("/") {
        Ok(_) => (),
        Err(VfsError::Backend(why)) => {
            serial_println!(
                ":: wifi: staging attempt DEFERRED at boot-root — the root volume did not answer ({}); nothing staged from it, re-attempting ::",
                why
            );
            return VolOutcome::Deferred("boot-root");
        }
        Err(_) => {
            serial_println!(":: wifi: boot-root pass skipped — no root volume bound; searched nothing ::");
            return VolOutcome::Unusable(String::from("boot-root=unbound"));
        }
    }
    let name = mt.volume_name("/").unwrap_or_default();
    if name != "native" {
        serial_println!(
            ":: wifi: boot-root pass skipped — / is volume '{}' (not the native UnaFS root; the program-source FAT pass already searched the card); searched nothing ::",
            name
        );
        return VolOutcome::Unusable(alloc::format!("boot-root=skipped(volume={})", name));
    }
    let bound = crate::fs::unafs::mount_bound_handle();
    if bound != Some(boot) {
        serial_println!(
            ":: wifi: boot-root pass REFUSED — the native volume rides handle={:?}, the boot card is handle={:?}; a removable volume is data, never a microcode source (R95); searched nothing ::",
            bound, boot
        );
        return VolOutcome::Unusable(alloc::format!("boot-root=refused(handle={:?})", bound));
    }
    let vol = alloc::format!("source=boot-root volume=native handle={:?}", boot);
    let mut rejected = 0usize;
    for spec in FW_SET {
        if with_staged(spec.role, |_| ()).is_some() {
            continue;
        }
        let mut res = RoleResult::Absent;
        'search: for dir in SEARCH_DIRS {
            for nm in spec.names {
                let path = match dir {
                    None => alloc::format!("/{}", nm),
                    Some(d) => alloc::format!("/{}/{}", d, nm),
                };
                let Ok(st) = mt.stat(&path) else { continue };
                if st.kind != NodeKind::File {
                    continue;
                }
                // Bounds gate before the read, exactly as the FAT pass: a bogus size never commits the heap.
                if st.size < MIN_BLOB_BYTES as u64 || st.size > MAX_BLOB_BYTES as u64 {
                    serial_println!(
                        ":: wifi: {} REJECTED {} size={} on {} — reason=out-of-bounds (min {} max {}) ::",
                        spec.role, path, st.size, vol, MIN_BLOB_BYTES, MAX_BLOB_BYTES
                    );
                    res = RoleResult::Rejected;
                    break 'search;
                }
                res = match mt.read(&path, 0, st.size as usize) {
                    Ok(data) if data.len() == st.size as usize => admit(spec, path, &vol, data),
                    Ok(data) => {
                        serial_println!(
                            ":: wifi: {} REJECTED {} size={} on {} — reason=short-read (got {} bytes) ::",
                            spec.role, path, st.size, vol, data.len()
                        );
                        RoleResult::Rejected
                    }
                    Err(e) => {
                        serial_println!(
                            ":: wifi: {} REJECTED {} size={} on {} — reason=read-failed ({:?}) ::",
                            spec.role, path, st.size, vol, e
                        );
                        RoleResult::Rejected
                    }
                };
                break 'search; // first existing name wins, per role
            }
        }
        match res {
            RoleResult::Rejected => rejected += 1,
            RoleResult::Staged => (),
            RoleResult::Absent => serial_println!(
                ":: wifi: {} ABSENT — none of {} in {} on {} ::",
                spec.role, names_description(spec), dirs, vol
            ),
        }
    }
    VolOutcome::Searched(rejected, vol)
}

/// WIFI-REARM: whether a staging attempt produced this boot's answer, or only a "not yet".
///
/// Carries no data beyond that distinction on purpose: the DETAIL of what happened is already on the
/// wire, printed by the attempt itself. A verdict enum that also carried the reason would be a second
/// place for the same fact to be written, and the two spellings would drift.
pub enum StageOutcome {
    /// A terminal verdict was printed. The caller must not attempt again this boot.
    Settled,
    /// Nothing was staged and nothing terminal was printed, for a reason a later pass can change.
    /// The `&'static str` names the stage that deferred, for the caller's retry line.
    Retry(&'static str),
    /// WIFI-REACH: the set is INCOMPLETE after searching every volume PRESENT this attempt, and no
    /// second populated handle existed to search — so a volume that has not yet enumerated (the
    /// classic case: a USB stick carrying the b43 blobs, published many main-loop passes after the
    /// internal card that is the program source) could still carry the missing roles. NOTHING
    /// terminal was printed. The caller holds arc 2 and the terminal verdict, and re-attempts when
    /// the USB storage-ready edge fires or a bounded deadline expires (`crate::wifi`'s second-handle
    /// wait). Returned ONLY when the caller passed `commit = false`: on the committing attempt the
    /// verdict is forced, so `Pending` can never be the last word.
    ///
    /// FWPIN (B455): RETIRED — no longer returned (the alternate-handle search it waited for is gone, R95);
    /// kept so `wifi/mod.rs`'s `S_WAIT_ALT` arms compile until the integrator retires that state.
    Pending,
}

/// A [`FatError`] that a later pass could plausibly see differently.
///
/// `NoDisk`/`Io`/`Busy` are all statements about the TRANSPORT at this instant — the handle was
/// registered, the read did not land. Every other variant is a statement about the MEDIA (not FAT,
/// an unsupported geometry, a corrupt chain), and re-reading identical sectors cannot change it.
/// Retrying those would be a spin dressed up as diligence.
fn retryable(e: FatError) -> bool {
    matches!(e, FatError::NoDisk | FatError::Io | FatError::Busy)
}

/// One volume's worth of the search. Returns `(staged, rejected)` for the roles it filled on THIS
/// volume; roles already staged by an earlier pass — OR an earlier ATTEMPT — are skipped, so a
/// second volume (or a re-attempt after a deferral) can only ever ADD to the set and can never
/// re-stage or shadow an image already held. That per-role skip is the no-double-stage guarantee
/// after PSRC widened the search past a single volume — see the module note.
///
/// `vol` is the fingerprint string the witness lines quote, built by the caller so both passes name
/// their volume in the same vocabulary.
fn stage_pass(fs: &FatFs, root: &[DirEntry], vol: &str, dirs: &str) -> (usize, usize) {
    let mut staged = 0usize;
    let mut rejected = 0usize;
    for spec in FW_SET {
        if with_staged(spec.role, |_| ()).is_some() {
            continue; // filled by an earlier volume/attempt — first volume wins, per role
        }
        match stage_role(fs, root, spec, vol) {
            RoleResult::Staged => staged += 1,
            RoleResult::Rejected => rejected += 1,
            RoleResult::Absent => serial_println!(
                ":: wifi: {} ABSENT — none of {} in {} on {} ::",
                spec.role, names_description(spec), dirs, vol
            ),
        }
    }
    (staged, rejected)
}

/// The outcome of mounting and searching ONE volume — the reconciliation of PSRC's `(usize, usize,
/// String)` tuple with WIFI-REARM's `StageOutcome`.
enum VolOutcome {
    /// Mounted, root read, and searched. `(rejected_here, volume-description)`.
    Searched(usize, String),
    /// The mount or the root-directory read failed for a reason a later pass CAN change
    /// (`NoDisk`/`Io`/`Busy`). Nothing was staged from this volume and nothing terminal was printed;
    /// the attempt is a "not yet". Carries the stage name for the caller's retry line. A deferred
    /// mount is a `Retry` whether it is the program source or the alternate: the set is not settled
    /// until every volume it could reach has been genuinely tried.
    Deferred(&'static str),
    /// The mount or root read failed for a reason a later pass CANNOT change (`NotFat`,
    /// `Unsupported`, a corrupt chain). A NON-TERMINAL per-volume line was printed; this volume
    /// contributes nothing, but the OTHER volume may still carry files, so the search continues and
    /// the caller still prints exactly one terminal verdict. Carries a volume-description for the
    /// searched-list.
    Unusable(String),
}

/// Mount one source and run [`stage_pass`] on it, naming the volume for the witnesses.
fn stage_volume(src: fat::BlockSource, dirs: &str, label: &str) -> VolOutcome {
    let fs = match fat::mount_source(src) {
        Ok(fs) => fs,
        Err(e) if retryable(e) => {
            // NOT a terminal line. The handle exists and the transport did not answer — the
            // signature of a device published a moment before its bring-up settled. Nothing was
            // staged from this volume, so re-attempting costs one mount.
            serial_println!(
                ":: wifi: staging attempt DEFERRED at mount — {} volume present but would not mount ({}); nothing staged from it, re-attempting ::",
                label, reason(e)
            );
            return VolOutcome::Deferred("mount");
        }
        Err(e) => {
            serial_println!(
                ":: wifi: firmware NOT staged from the {} volume — would not mount ({}); searched {} ::",
                label, reason(e), dirs
            );
            return VolOutcome::Unusable(alloc::format!("source={} (unmountable)", src.name()));
        }
    };
    let (vol_id, clusters) = fs.volume_fingerprint();
    let vol = alloc::format!(
        "source={} label='{}' fp={:#010x}:{:#010x}",
        fs.source_name(), fs.label(), vol_id, clusters
    );
    let root = match fs.read_root() {
        Ok(r) => r,
        Err(e) if retryable(e) => {
            serial_println!(
                ":: wifi: staging attempt DEFERRED at root-dir — mounted the {} volume {} but the root directory did not read ({}); nothing staged from it, re-attempting ::",
                label, vol, reason(e)
            );
            return VolOutcome::Deferred("root-dir");
        }
        Err(e) => {
            serial_println!(
                ":: wifi: firmware NOT staged from the {} volume — root directory unreadable ({}) on {}; searched {} ::",
                label, reason(e), vol, dirs
            );
            return VolOutcome::Unusable(vol);
        }
    };
    let (_staged, rejected) = stage_pass(&fs, &root, &vol, dirs);
    VolOutcome::Searched(rejected, vol)
}

/// Locate + validate + stage the firmware set. Called only after a program-source block device is
/// present, and at most [`crate::wifi`]'s bounded attempt budget of times per boot — see
/// [`StageOutcome`] and the module note. The witness IS the result.
///
/// ## PSRC (GR26): two volumes, in preference order — and why this read is the exception
///
/// The block layer's [`crate::drivers::block::program_source`] now prefers the handle carrying the
/// BOOT VOLUME serial, so on the bench machine (boot volume = the internal SD card) a USB stick
/// inserted merely to carry files no longer becomes the program source. That is the right rule for
/// `/boot`, for every exec, and for every write — and it would be the WRONG rule applied alone here,
/// because the b43 blobs are USER-SUPPLIED files that the user carries on exactly such a stick.
/// UnaOS ships no firmware (`CLEAN_ROOM_POLICY.md` §4); the honest reading of "the user placed the
/// files on the media" is that the media is whichever one they plugged in.
///
/// So this pass searches the program source FIRST and, only if the set is still incomplete, the
/// other populated handle. It is safe to widen precisely here and nowhere else:
///
///   * **Read-only.** This module never writes a sector, so the substitution FRGUARD refuses —
///     writing this system's data to a medium it did not boot from — cannot arise.
///   * **Additive, per role.** [`stage_pass`] skips a role already staged, so the preferred volume
///     always wins any file that exists on both. A second volume can add a missing image; it can
///     never replace one.
///   * **Same shape as the search this loader already does.** It already tries three directories per
///     volume and three names per directory for the same reason. A second volume is one more rung on
///     a ladder that exists because firmware is a "wherever I can find it" read.
///
/// ## PSRC meets WIFI-REARM
///
/// The two-volume search runs INSIDE the retry-aware entry. A `Deferred` from EITHER volume becomes
/// a `Retry` (the transport for that handle had not settled); the caller re-attempts and, because
/// [`stage_pass`] skips roles already in `STAGED`, the re-attempt resumes the half-built set without
/// double-staging. A mounted-but-incomplete volume is NOT terminal on its own — the terminal verdict
/// below is printed only after BOTH the program source and (if the set is still short) the alternate
/// have been tried on this attempt.
///
/// The witness names the volume each image came from (`on source=… label=… fp=…`), so a capture
/// always says which medium fed the radio — the widening is never silent.
///
/// **FWPIN (B455) supersedes the two sections below:** pass 2 is now the boot card's own native UnaFS
/// root ([`stage_boot_root`]), never another handle, and `Pending` is never returned — `commit` is
/// kept in the signature for the caller and ignored. The text below is the history.
///
/// ## WIFI-REACH (GR26): `commit`, and the `Pending` third outcome
///
/// The two-volume search above only helps when BOTH handles are present at attempt time. The bench
/// timing (`sdhc.md` §13.7) is card-early / stick-late: on the first attempt the program source (the
/// internal card) is present but the USB stick carrying the blobs has not enumerated, so
/// [`crate::drivers::block::alternate_program_source`] is `None` and pass 2 finds nothing to search.
/// Printing the terminal INCOMPLETE verdict there — and letting arc 2 run on that count — is exactly
/// the premature verdict WIFI-REARM was built to prevent, one main-loop epoch earlier than the stick.
///
/// So this pass takes `commit`:
///   * `commit == false` — the caller can still wait. If the set is incomplete AND no alternate
///     handle was present to search, return [`StageOutcome::Pending`] and print NOTHING terminal;
///     the caller holds arc 2 and re-attempts when the stick's storage-ready edge fires or its
///     deadline expires. If an alternate WAS present and searched and the set is still incomplete,
///     both handles have been genuinely tried and the verdict IS terminal — printed here, `Settled`.
///   * `commit == true` — the second-handle deadline has expired; force the terminal verdict whether
///     or not an alternate ever appeared. `Pending` is never returned on a committing attempt.
pub fn stage_attempt(_commit: bool) -> StageOutcome {
    let dirs = dirs_description();

    // Pass 1 — the program source. FAT-verb law: reads follow it. See the module note.
    let (vol, rejected, boot) = match crate::drivers::block::program_source() {
        Some((_, h)) => match stage_volume(source_of_handle(h), &dirs, "program-source") {
            VolOutcome::Deferred(stage) => return StageOutcome::Retry(stage),
            VolOutcome::Searched(rej, vol) => (vol, rej, h),
            VolOutcome::Unusable(vol) => (vol, 0, h),
        },
        None => {
            // The caller gates on `program_source().is_some()`, so this arm is defensive.
            serial_println!(
                ":: wifi: firmware NOT staged — no program-source block device; searched {} ::", dirs
            );
            return StageOutcome::Settled;
        }
    };

    // Pass 2 (FWPIN) — the SAME card's native UnaFS root, for roles still missing. Never another
    // handle: the alternate-handle (USB stick) pass is gone (R95, SEC-2026-10-06 F5).
    #[cfg(feature = "unafs")]
    let (alt_rej, alt_vol) = if staged_count() < FW_SET.len() {
        match stage_boot_root(&dirs, boot) {
            VolOutcome::Deferred(stage) => return StageOutcome::Retry(stage),
            VolOutcome::Searched(rej, v) => (rej, v),
            VolOutcome::Unusable(v) => (0, v),
        }
    } else {
        (0, String::new())
    };
    #[cfg(not(feature = "unafs"))]
    let (alt_rej, alt_vol) = {
        let _ = boot; // no native volume in this build: the program-source FAT pass is the whole search
        (0usize, String::new())
    };
    let rejected = rejected + alt_rej;

    let searched = if alt_vol.is_empty() {
        vol
    } else {
        alloc::format!("{} + {}", vol, alt_vol)
    };

    // Exactly one terminal verdict, read from the authoritative `STAGED` set (not a per-attempt
    // counter — the set may have been built across the two passes above and across earlier attempts).
    let staged = staged_count();
    if staged == FW_SET.len() {
        serial_println!(
            ":: wifi: firmware set COMPLETE {}/{} staged on {} — held in kernel memory, NOT pushed to the core (no MMIO, no device write); arc 2 owns bcma core bring-up ::",
            staged, FW_SET.len(), searched
        );
    } else {
        let mut missing = String::new();
        for spec in FW_SET {
            if with_staged(spec.role, |_| ()).is_some() {
                continue;
            }
            if !missing.is_empty() {
                missing.push_str(", ");
            }
            missing.push_str(spec.role);
            missing.push('(');
            missing.push_str(&names_description(spec));
            missing.push(')');
        }
        serial_println!(
            ":: wifi: firmware set INCOMPLETE {}/{} staged ({} rejected) on {} — missing: {} — parked, radio stays down ::",
            staged, FW_SET.len(), rejected, searched,
            if missing.is_empty() { "none (see REJECTED above)" } else { missing.as_str() },
        );
    }
    StageOutcome::Settled
}

/// PSRC: the [`fat::BlockSource`] that reads a given block-layer handle.
///
/// `fs::fat` keeps a private `source_of` doing exactly this for `mount_program_source`. It is
/// deliberately NOT reused here: `fs/fat.rs` is outside this arc's lane, and publishing its private
/// mapping is a change that belongs to the FAT layer's own arc. **Flagged for the integrator:** the
/// two must stay in step, and the compiler enforces the harder half — both matches are total over
/// `BlockHandle`, so a fourth handle is a build error in both places, not a silent mis-route.
fn source_of_handle(handle: crate::drivers::block::BlockHandle) -> fat::BlockSource {
    match handle {
        crate::drivers::block::BlockHandle::Global => fat::BlockSource::Default,
        crate::drivers::block::BlockHandle::Usb => fat::BlockSource::Usb,
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        crate::drivers::block::BlockHandle::Sdhc => fat::BlockSource::Sdhc,
        // TEGRA-SDBLK: the fourth handle predicted above, arrived. Mapped only — neither
        // `program_source` nor `alternate_program_source` returns it, so no firmware search ever
        // reaches this arm; nothing else in this file changes.
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        crate::drivers::block::BlockHandle::SdMmc => fat::BlockSource::SdMmc, #[cfg(all(target_arch = "x86_64", feature = "ahci"))] crate::drivers::block::BlockHandle::Ahci { port } => fat::BlockSource::Ahci(port), // AHCIBOOT: mapped only — neither `program_source` nor `alternate_program_source` returns a SATA handle, so no firmware search reaches this arm; nothing else in this file changes.
    }
}

/// REFUSALUI (B468): FWPIN's refusal on the glass — information (a toast), in the wire's words.
fn refused_ui(role: &str, why: &str) {
    let w = alloc::format!("{} reason={}", role, why);
    #[cfg(feature = "wc")] let _ = crate::video::dialog::refused(crate::video::dialog::WHAT_FIRMWARE, w.as_bytes());
    let _ = w;
}
