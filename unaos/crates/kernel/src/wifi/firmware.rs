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
// The loader reads through `fat::mount_program_source()`: the global block device when one is
// registered, else (x86 + `sdhcblk`) the card in the internal SD reader. That is the FAT-verb law —
// a "wherever I can find it" read FOLLOWS THE PROGRAM SOURCE — and firmware is exactly that class of
// read. Reaching for `mount()` instead would find nothing on a machine booted from the internal
// reader, where the program-bearing volume is mounted and the global handle is empty at the same
// time.
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
//   * **Word alignment** — pinned by `bcm4331.md` §S4: "the payload is a stream of big-endian 32-bit
//     words". So the payload length must be a multiple of 4 whatever the header turns out to be.
//     Reported, not enforced, because it is a property of the header layout question below.
//   * **Container header — a DOCUMENTED DISAGREEMENT, reported rather than resolved by fiat.**
//     `bcm4331.md` §S4 records "a 4-byte header (`type`, `ver`, reserved, big-endian `size`)". Those
//     four fields cannot fit in four bytes — a `be32` size alone is four — so the record is internally
//     inconsistent and cannot be implemented as written. Rather than pick a reading and call it
//     pinned, `classify_header` tries BOTH self-consistent candidates and reports which one (if
//     either) the file actually satisfies:
//
//       - **Layout A (8-byte):** `type` u8, `ver` u8, 2 reserved, `be32 size`; consistent iff
//         `size == len - 8`.
//       - **Layout B (4-byte):** `type` u8, `ver` u8, 2 reserved, no declared size; consistent iff
//         `(len - 4) % 4 == 0` (payload is whole 32-bit words) and layout A does not hold.
//
//     The ACCEPT decision rests only on presence and bounds, so a file in either container — or in
//     neither — still stages, and says which. **This is the metal probe for the disagreement:** the
//     first boot with the real set prints `hdr=A|B|unrecognized`, `type=`, `ver=`, `declared=` and an
//     FNV-1a digest per file, and that capture is what corrects `bcm4331.md` §S4 and gates arc 2.
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
//   * **`Settled`** — the volume mounted and each role got its verdict, OR the mount failed for a
//     reason a later pass cannot change (`NotFat`, `Unsupported`, a corrupt chain): a volume that is
//     not FAT now will not be FAT in two seconds. One terminal verdict, printed here.
//   * **`Retry(stage)`** — the mount, or the root-directory read, failed for a reason that CAN
//     change (`NoDisk`, `Io`, `Busy`). Nothing was staged and nothing was printed as terminal; the
//     caller re-attempts under its own bounded budget and prints the exhaustion line if it runs out.
//
// The `Retry` arms are reachable only BEFORE any role has been staged — both sit above the
// `for spec in FW_SET` loop — so a retry can never double-stage a role or resume a half-built set.
// That is a property of where the arms are, and it is why `STAGED` needs no reset between attempts.
//
// ## Sourcing (see the note in `mod.rs`)
// File names, core revision and PHY type come from `bcm4331.md` §S4, which states its own sourcing
// separately and differs from this file's — see the cross-reference in `mod.rs`. Nothing here was
// derived from Linux driver source.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

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
/// §S4: the initvals are "a few KB each", so the floor must clear a plausible small table.
const MIN_BLOB_BYTES: u32 = 256;
/// Above this we refuse rather than commit the kernel heap. §S4: the ucode is "tens of KB"; 4 MiB is
/// ~two orders of magnitude of headroom and still bounded.
const MAX_BLOB_BYTES: u32 = 4 * 1024 * 1024;

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

/// The staged microcode's container verdict, as `(layout, type, ver, declared, len)`, or `None` when
/// the `ucode` role did not stage. `None` while the set is COMPLETE is itself a finding, and arc 2
/// prints it as one.
///
/// This exists so arc 2 does not re-derive the payload offset. W3 — `bcm4331.md` §S4's internally
/// inconsistent header record — is still an open UNKNOWN, and two independent implementations of an
/// unresolved rule is exactly how the loader's `hdr=` verdict and an uploader's payload offset come
/// to disagree without either one looking wrong. One `classify_header`, both readers.
///
/// `wifi2`-gated, along with [`FW_SET_LEN`], so that an arc-1-only build's item set is unchanged: the
/// census-and-staging image stays what it was, rather than "what it was, plus two accessors nothing
/// calls".
#[cfg(feature = "wifi2")]
pub(crate) fn ucode_header() -> Option<(&'static str, u8, u8, u32, usize)> {
    with_staged("ucode", |b| {
        let v = classify_header(b);
        (v.layout, v.kind, v.ver, v.declared, b.len())
    })
}

/// Short human reason for a [`FatError`] in the witness lines. A local twin of `fat::fat_reason`,
/// which is `#[cfg(target_arch = "aarch64")]` — this path is x86-only, and widening that gate would
/// be an edit to a shared file for no functional gain. Exhaustive by construction: a new `FatError`
/// variant makes this fail to compile rather than silently printing the wrong reason.
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

/// Which container layout, if any, the leading bytes satisfy. See the module note: this REPORTS the
/// documented disagreement, it does not resolve it.
struct HdrVerdict {
    kind: u8,
    ver: u8,
    /// Layout A's declared payload length. Meaningless unless `layout == "A"`.
    declared: u32,
    layout: &'static str,
    /// Payload is a whole number of big-endian 32-bit words under the chosen layout (§S4).
    words_ok: bool,
}

fn classify_header(data: &[u8]) -> HdrVerdict {
    if data.len() < 8 {
        return HdrVerdict { kind: 0, ver: 0, declared: 0, layout: "shorter-than-header", words_ok: false };
    }
    let kind = data[0];
    let ver = data[1];
    let declared = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let len = data.len() as u64;
    // Layout A: 8-byte header carrying an explicit big-endian payload length.
    if declared as u64 == len - 8 {
        return HdrVerdict { kind, ver, declared, layout: "A", words_ok: (len - 8) % 4 == 0 };
    }
    // Layout B: 4-byte header, payload length implied by the file size.
    if (len - 4) % 4 == 0 {
        return HdrVerdict { kind, ver, declared, layout: "B", words_ok: true };
    }
    HdrVerdict { kind, ver, declared, layout: "unrecognized", words_ok: false }
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

            let v = classify_header(&data);
            let digest = fnv1a32(&data);
            let len = data.len();
            serial_println!(
                ":: wifi: {} STAGED {} bytes={} on {} fnv1a={:#010x} hdr={} type={:#04x} ver={:#04x} declared={} words={} ::",
                spec.role, path, len, vol, digest, v.layout, v.kind, v.ver, v.declared,
                if v.words_ok { "ok" } else { "not-32bit-aligned" },
            );
            STAGED.lock().push(StagedImage { role: spec.role, path, bytes: data, digest });
            return RoleResult::Staged;
        }
    }
    RoleResult::Absent
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

/// Locate + validate + stage the firmware set. Called only after a program-source block device is
/// present, and at most [`crate::wifi`]'s bounded attempt budget of times per boot — see
/// [`StageOutcome`] and the module note. The witness IS the result.
pub fn stage_attempt() -> StageOutcome {
    let dirs = dirs_description();

    // FAT-verb law: reads follow the program source. See the module note.
    let fs = match fat::mount_program_source() {
        Ok(fs) => fs,
        Err(e) if retryable(e) => {
            // NOT the terminal line. The handle exists (the caller's gate just said so) and the
            // transport did not answer, which is the signature of a device published a moment before
            // its bring-up settled. Nothing has been staged, so re-attempting costs one mount.
            serial_println!(
                ":: wifi: staging attempt DEFERRED at mount — program-source volume present but would not mount ({}); nothing staged, re-attempting ::",
                reason(e)
            );
            return StageOutcome::Retry("mount");
        }
        Err(e) => {
            serial_println!(
                ":: wifi: firmware NOT staged — program-source FAT volume would not mount ({}); searched {} ::",
                reason(e), dirs
            );
            return StageOutcome::Settled;
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
                ":: wifi: staging attempt DEFERRED at root-dir — mounted {} but the root directory did not read ({}); nothing staged, re-attempting ::",
                vol, reason(e)
            );
            return StageOutcome::Retry("root-dir");
        }
        Err(e) => {
            serial_println!(
                ":: wifi: firmware NOT staged — root directory unreadable ({}) on {}; searched {} ::",
                reason(e), vol, dirs
            );
            return StageOutcome::Settled;
        }
    };

    let mut staged = 0usize;
    let mut rejected = 0usize;
    let mut missing = String::new();
    for spec in FW_SET {
        match stage_role(&fs, &root, spec, &vol) {
            RoleResult::Staged => staged += 1,
            RoleResult::Rejected => rejected += 1,
            RoleResult::Absent => {
                if !missing.is_empty() {
                    missing.push_str(", ");
                }
                missing.push_str(spec.role);
                missing.push('(');
                missing.push_str(&names_description(spec));
                missing.push(')');
                serial_println!(
                    ":: wifi: {} ABSENT — none of {} in {} on {} ::",
                    spec.role, names_description(spec), dirs, vol
                );
            }
        }
    }

    // Exactly one terminal verdict, whatever happened above.
    if staged == FW_SET.len() {
        serial_println!(
            ":: wifi: firmware set COMPLETE {}/{} staged on {} — held in kernel memory, NOT pushed to the core (no MMIO, no device write); arc 2 owns bcma core bring-up ::",
            staged, FW_SET.len(), vol
        );
    } else {
        serial_println!(
            ":: wifi: firmware set INCOMPLETE {}/{} staged ({} rejected) on {} — missing: {} — parked, radio stays down ::",
            staged, FW_SET.len(), rejected, vol,
            if missing.is_empty() { "none (see REJECTED above)" } else { missing.as_str() },
        );
    }
    StageOutcome::Settled
}
