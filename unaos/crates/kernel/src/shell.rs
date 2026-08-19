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

use alloc::vec::Vec;
use alloc::string::String;
use crate::console::Console;
use crate::fs::fat::{DirEntry, FatError, FatFs};
// The in-kernel `vug` demo (the `vug` and `pulse` verbs) is an aarch64 module — see the `pub mod vug`
// note in `lib.rs`. The verbs that drive it are gated to match, so on x86 they are not registered at
// all and fall through to the normal unknown-command reply.
#[cfg(target_arch = "aarch64")]
use crate::vug;
use crate::pal::TargetPal;

// STORM-X86: the module that owns the user address-space slot POOL, aliased per-arch so the `storm`
// verb's census can name the same fact on both. On aarch64 the pool (`SLOT_USED` + `USER_SLOTS`) is
// built by the EL0 bring-up in `arch::boot`; on x86 it is the static PML4/PDPT/PD/PT + backing pool
// in `arch::memory`, claimed by `alloc_user_space`. Same quantity, same denominator, different
// owning module — an alias rather than a cfg-split at each use site, so the census line stays ONE
// piece of source and the two arches cannot drift into printing different things.
#[cfg(all(feature = "baremetal", target_arch = "aarch64"))]
use crate::arch::boot as storm_slots;
#[cfg(target_arch = "x86_64")]
use crate::arch::memory as storm_slots;

/// JD4: the shell's current working directory as a NORMALIZED, CANONICAL absolute path
/// ("/" = root, else "/DIR/SUB" in the on-disk 8.3 spelling). A path string, not a cached
/// cluster: every command re-resolves it from the root, so a swapped or remounted card can
/// never leave the shell holding a stale chain head — the worst case is an honest `-ENOENT`.
/// `None` means the root (no heap touched until the first `cd`).
static CWD: spin::Mutex<Option<String>> = spin::Mutex::new(None);

/// The current working directory as a display/join-ready absolute path.
fn cwd_path() -> String {
    CWD.lock().clone().unwrap_or_else(|| String::from("/"))
}

/// Join `arg` onto `base` and normalize lexically: absolute `arg` replaces `base`, `.` and empty
/// components collapse, `..` pops (never above the root). Purely textual — resolution against the
/// volume happens in [`resolve_path`].
fn normalize_path(base: &str, arg: &str) -> String {
    let mut comps: Vec<&str> = Vec::new();
    let prefix = if arg.starts_with('/') { "" } else { base };
    for part in prefix.split('/').chain(arg.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                comps.pop();
            }
            p => comps.push(p),
        }
    }
    if comps.is_empty() {
        return String::from("/");
    }
    let mut out = String::new();
    for c in comps {
        out.push('/');
        out.push_str(c);
    }
    out
}

/// A resolved absolute path: the root itself, or a concrete directory entry (file or subdir)
/// plus the CANONICAL absolute path it was found at (on-disk 8.3 spelling).
enum Resolved {
    Root,
    Entry(DirEntry, String),
}

/// Walk a normalized absolute path from the root, component by component, via the read-only
/// `FatFs::read_dir`. Case-insensitive 8.3 matching (short names are stored uppercase on disk).
/// Errors carry the errno-style tag the caller prints — nothing is swallowed.
fn resolve_path(fs: &FatFs, path: &str) -> Result<Resolved, String> {
    let mut cluster = 0u32; // 0 = the root (read_dir's convention)
    let mut cur: Option<(DirEntry, String)> = None;
    let mut canon = String::new();
    for comp in path.split('/').filter(|c| !c.is_empty()) {
        if let Some((de, _)) = &cur {
            if !de.is_dir {
                return Err(alloc::format!("{}: not a directory (-ENOTDIR)", canon));
            }
            cluster = de.first_cluster();
        }
        let entries = fs
            .read_dir(cluster)
            .map_err(|e| alloc::format!("{}: read failed ({:?}, -EIO)", canon, e))?;
        match entries.iter().find(|de| de.name().eq_ignore_ascii_case(comp)) {
            Some(de) => {
                canon.push('/');
                canon.push_str(de.name());
                cur = Some((*de, canon.clone()));
            }
            None => {
                return Err(alloc::format!("{}/{}: not found (-ENOENT)", canon, comp));
            }
        }
    }
    Ok(match cur {
        None => Resolved::Root,
        Some((de, canon)) => Resolved::Entry(de, canon),
    })
}

// ---------------------------------------------------------------------------------------------
// JD5/JD6 — the WRITE path: create / edit / delete files from the panel shell (JD6 extends it to
// any subdirectory the shell can `cd` into; JD5 was root-directory only).
//
// DESIGN NOTE (why this rides fat.rs directly, not the SYS_OPEN/WRITE syscall layer):
// The SVC syscall path is EL0/ASID-keyed and lives in the out-of-lane `arch/aarch64/syscall.rs`;
// the kernel shell runs in kernel mode as ASID 0 (on the tegra post-drop core TTBR0_EL1[63:48] = 0 — it
// never switches TTBR0 to a user slot), so it cannot invoke the SVC path. The shell therefore rides
// the SAME F3-locked fat.rs PUBLIC entry points the U9/U10/U11 syscalls call — the dir-aware
// `create_in_dir`/`locate_in_dir` twins (JD6; `first_cluster == 0` ⇒ the root twins
// `create_in_root`/`find_located`), plus `write_grow`, `delete_located` — never editing fat.rs
// (call-never-edit this arc; the two JD6 twins are a seat-granted narrow ADDITIVE exception).
//
// PRINCIPAL: the shell IS ASID 0, the shared/public principal. By U6's existing rule an ASID-0
// create is PUBLIC (no owner row), so shell-created files are plain public FAT files. The shell does
// not — and cannot without an out-of-lane pub accessor — consult the U6 `OWNED_FILES` ACL; that is
// correct: the panel shell is the local trusted operator console, the same trust as the boot window.
// (A future arc that runs user tasks and returns to the shell must re-establish ASID 0 before shell
// FAT ops — today the shell is cooperative kernel and never installs a user-slot TTBR0.)
//
// SCOPE (JD6): the WHOLE tree the shell can `cd` into. `resolve_write_target` normalizes the path
// against the cwd and walks to the PARENT directory via the read-only `resolve_path`, then the
// writes ride the dir-aware `create_in_dir`/`locate_in_dir` twins (`first_cluster == 0` ⇒ root).
// A parent that is a plain file is `-ENOTDIR`; a missing parent `-ENOENT`; a FULL directory
// `-ENOSPC` (extending a subdir's cluster chain is out of scope — the twins add a slot but never grow
// the directory chain). JD7 layers `mkdir`/`rmdir` on top via the `fat::create_dir`/`remove_dir`
// FATDIRS seam (call-never-edit, like JD6's write path): `rm` stays file-only (`-EISDIR` on a
// directory — use `rmdir`), and `rmdir` removes an EMPTY directory (non-empty ⇒ `-ENOTEMPTY`, the
// root refused). The seam's internal F3 locking is sound for these kernel callers without the syscall
// NAMESPACE lock (it reaches fat.rs directly) — see fat.rs's FATDIRS block + docs/SECURITY.md.
// JD8 layers `cp <src> <dst>` (file copy) on the SAME primitives — no new fat.rs surface: it STREAMS
// the source through the offset-aware `read_at` into the create-or-truncate `create_in_dir`/
// `write_grow` write path (the `cp FILE DIR/` idiom lands the copy under the source leaf; a plain-`cp`
// directory source is `-EISDIR`). JD9 adds `cp -r <srcdir> <dst>` — recursive directory copy — by
// COMPOSING those same primitives: `read_dir` walks the source, the FATDIRS `create_dir` seam rebuilds
// the tree (call-never-edit, like `mkdir`), and the JD8 per-file streaming leg (`copy_file_into`) copies
// each file. It creates a FRESH destination tree (top-level target pre-existing ⇒ `-EEXIST`; `.`/`..`
// filtered at every level; self-into-descendant refused `-EINVAL`; depth bounded `-ELOOP`; a mid-tree
// failure reports the honest partial count) — still no new fat.rs surface.
//
// SAFETY (M3): every fat.rs write returns a `Result`; a stalled USB write surfaces as
// `FatError::Io` (the block layer's `write_block` rides the SAME JD3 wall-clock-bounded BOT pump as
// reads — a dead transfer times out, never WFI-parks the timerless kernel core). Writes are
// WRITE-THROUGH (BOT WRITE(10) / polled SD CMD24 complete before the command returns — no write-back
// cache), and each fat.rs step is atomic under F3's FAT_MUTATION/DIR_MUTATION locks, so a mid-
// sequence failure leaves the volume consistent: a failed grow keeps the OLD (smaller) size (size is
// published last); a failed `rm`/truncate leaves lost clusters (benign, chkdsk-reclaimable), never
// an aliasing or torn volume.

/// JD5: errno tag for a `FatError`, for the write-command console messages.
fn fat_errno(e: FatError) -> &'static str {
    match e {
        FatError::NotFound => "-ENOENT",
        FatError::IsDirectory => "-EISDIR",
        FatError::NoSpace => "-ENOSPC",
        FatError::Unsupported => "-EINVAL", // not a representable 8.3 name
        FatError::NoDisk | FatError::NotFat => "-ENODEV",
        FatError::Io | FatError::BadChain => "-EIO",
        // WEDGE-8 (F3): the storage driver's controller loan was busy past the bounded retry —
        // the operation never started; running the command again is the honest remedy.
        FatError::Busy => "-EAGAIN",
        // Merge seam: the x86 trunk's fat.rs added OutOfVolume (a range check past the volume
        // end); it is an addressing error, not a device fault, but -EIO is the closest errno
        // this tagger's callers understand. Grafted at assembly.
        FatError::OutOfVolume => "-EIO",
    }
}

/// JD6: resolve a shell path argument to its write target `(parent_first_cluster, leaf_name,
/// parent_canon)`. `.`/`..` normalize lexically against the cwd, then the read-only `resolve_path`
/// walks to the PARENT directory (the root ⇒ `first_cluster` 0 and `parent_canon` ""). The final
/// component is the leaf to create/locate — it need NOT exist yet. The root itself is not a writable
/// target (`-EISDIR`); a parent that is a plain file is `-ENOTDIR`; a missing parent is `-ENOENT`
/// (both surface from `resolve_path`). The dir-aware fat.rs twins (`create_in_dir`/`locate_in_dir`)
/// take `parent_first_cluster` directly, so this reaches the whole tree the shell can `cd` into.
fn resolve_write_target(fs: &FatFs, arg: &str) -> Result<(u32, String, String), String> {
    let path = normalize_path(&cwd_path(), arg);
    let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    let (leaf, parent_comps) = match comps.split_last() {
        Some((last, rest)) => (String::from(*last), rest),
        None => return Err(String::from("/: is a directory (-EISDIR)")), // path == "/"
    };
    if parent_comps.is_empty() {
        return Ok((0, leaf, String::new())); // parent is the volume root
    }
    let mut parent_path = String::new();
    for c in parent_comps {
        parent_path.push('/');
        parent_path.push_str(c);
    }
    match resolve_path(fs, &parent_path)? {
        Resolved::Root => Ok((0, leaf, String::new())), // unreachable for a non-empty parent, but honest
        Resolved::Entry(de, canon) => {
            if de.is_dir {
                Ok((de.first_cluster(), leaf, canon))
            } else {
                Err(alloc::format!("{}: not a directory (-ENOTDIR)", canon))
            }
        }
    }
}

/// JD6: an absolute display path for a write target's leaf under `parent_canon` ("" ⇒ the root),
/// e.g. `("", "NOTE.TXT") → "/NOTE.TXT"`, `("/DOCS", "NOTE.TXT") → "/DOCS/NOTE.TXT"`.
fn joined(parent_canon: &str, leaf: &str) -> String {
    alloc::format!("{}/{}", parent_canon, leaf)
}

/// PI-UI-3: print a verb's output line to the panel AND mirror it to the serial console as a
/// `:: ui3:<verb>: <line> ::` witness. On the Pi bench the verb output renders panel-only, so a
/// headless capture cannot see it; the witness gives the same content on the wire so `date`/`time`/
/// `netinfo` are verifiable from serial alone. Same content on both sinks, byte-for-byte.
fn ui3_say(console: &mut Console, verb: &str, line: &str) {
    console.println(line);
    serial_println!(":: ui3:{}: {} ::", verb, line);
}

/// PI-FS-5: panel-line + `:: fs5: <line> ::` serial mirror (the `ui3_say` idiom, dedicated tag) — the
/// `diskinfo` verb renders panel-only on the bench, so the witness gives a headless capture the same content.
#[cfg(target_arch = "aarch64")]
fn fs5_say(console: &mut Console, line: &str) {
    console.println(line);
    serial_println!(":: fs5: {} ::", line);
}

// ===================== FATVERB — the verbs find their volume ========================================
//
// Boot AR, second half. LAUNCH-AR moved the three EXEC legs (the `FatVolume::is_file` probe, the
// `bare_exec` re-resolve, the `read_el0_image` load) from the default-handle `fat::mount` to
// `fat::mount_program_source()`, because on a machine booted from the internal SD reader the global
// `BLOCK_DEVICE` slot is EMPTY — `SDHCBLK` registers the card under its own handle and says so on
// the wire. That fixed launching. It did not fix looking: Peter typed `ls` twice on that same boot
// and got nothing, because the twenty-five FAT verbs in this file were all still asking the handle
// that does not exist there. A shell where `ls` and `vug` disagree about which volume is the volume
// is not a shell.
//
// So every verb binds the SAME handle the exec legs bind. The two helpers below are the only place
// that binding is expressed, which is the point: the failure mode this arc closes is call sites
// drifting apart, and twenty-five copies of `match fat::mount` is how they drifted.
//
// READ and WRITE are split because they are not the same question.
//   * A read verb needs a volume. If it cannot get one it says so, naming the handles it was
//     offered — the `exec-probe ... NO VOLUME (...; handles=...)` idiom, because a decline that
//     cannot name what it inspected cannot be falsified by the capture it appears in.
//   * A write verb needs a volume that ADMITS WRITES, and on the internal reader it will not get
//     one: `Sdhc` is a read-only mount outside the reserved flight-recorder extent (SDHC-4c). Left
//     ungated, `rm FOO` on that boot would walk a real directory mutation down to `write_sector`,
//     collect `Unsupported` from the permit, and surface as a per-sector I/O error — or, worse for
//     a multi-step verb like `write` (delete-then-recreate) or `mv`, get part-way. So the gate runs
//     BEFORE the first mutation, refuses LOUDLY on both sinks, and names the volume, the reason and
//     the census. It never silently no-ops.
//
// `mount_program_source()` is IDENTICAL to `mount()` on every configuration that already worked:
// aarch64 and x86-without-`sdhcblk` have no second handle, and where a global device is registered
// it still wins the precedence ladder. The behaviour only differs on the one boot shape that was
// already broken.

/// FATVERB: WHICH HANDLE A BINDING SITE ACTUALLY BOUND — recorded as a fact, not recomputed.
///
/// This exists because the first cut of the FATVERB witness was A==A and the adoption review said
/// so: it compared `mount_program_source() + resolve` against `mount_program_source() + resolve`
/// and would have passed with every read verb reverted to the old handle. The only way a fixture
/// can tell "the verbs bind the program source" from "this expression binds the program source" is
/// to observe what the VERBS DID, so each binding site stamps its answer here and the witness reads
/// the stamps after driving a real verb.
///
/// The SEQ counters are the half that makes a revert visible. A leg that only compared the stamps
/// would still pass against a stale pair left by some earlier call; requiring the counter to ADVANCE
/// across the driven verb means a verb that no longer routes through these helpers fails the leg
/// instead of inheriting somebody else's answer.
///
/// Plain relaxed atomics: this is an instrument, it is read only by the one-shot witness, and it is
/// on the path of every file verb — it must cost nothing and must never introduce an ordering that
/// the verbs would not otherwise have.
mod bind {
    /// Never asked.
    pub const NONE: u8 = 0;
    /// Asked; nothing mounted (the decline path).
    pub const DECLINED: u8 = 1;
    pub const GLOBAL: u8 = 2;
    pub const USB: u8 = 3;
    pub const SDHC: u8 = 4;
    /// The write gate ADMITTED the volume.
    pub const ADMITTED: u8 = 5;
    /// The write gate REFUSED it as read-only.
    pub const REFUSED_RO: u8 = 6;

    pub fn of(fs: &crate::fs::fat::FatFs) -> u8 {
        match fs.source_name() {
            "global" => GLOBAL,
            "usb" => USB,
            _ => SDHC,
        }
    }

    /// Only the x86 storage witness renders these; the verbs stamp and never read.
    #[cfg(target_arch = "x86_64")]
    pub fn name(code: u8) -> &'static str {
        match code {
            NONE => "never",
            DECLINED => "declined",
            GLOBAL => "global",
            USB => "usb",
            SDHC => "sdhc",
            ADMITTED => "admitted",
            REFUSED_RO => "refused-ro",
            _ => "?",
        }
    }
}

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering as BindOrd};

/// The handle the READ verbs' binding site last bound, and how many times it has bound anything.
static READ_BIND: AtomicU8 = AtomicU8::new(bind::NONE);
static READ_BIND_SEQ: AtomicU32 = AtomicU32::new(0);
/// The handle the EXEC PROBE (`FatVolume::is_file`) last bound. Independent producer, independent
/// call path — this is the pair `fatverb_storage_witness` compares. x86 only: aarch64 never sets
/// `Facts::exec`, so the probe is a constant `false` there and has no handle to stamp.
#[cfg(target_arch = "x86_64")]
static EXEC_BIND: AtomicU8 = AtomicU8::new(bind::NONE);
#[cfg(target_arch = "x86_64")]
static EXEC_BIND_SEQ: AtomicU32 = AtomicU32::new(0);
/// What the WRITE gate last answered, and how many times it has answered.
static WRITE_GATE: AtomicU8 = AtomicU8::new(bind::NONE);
static WRITE_GATE_SEQ: AtomicU32 = AtomicU32::new(0);

fn stamp(cell: &AtomicU8, seq: &AtomicU32, code: u8) {
    cell.store(code, BindOrd::Relaxed);
    seq.fetch_add(1, BindOrd::Relaxed);
}

/// FATVERB: the write gate's answer, decided but not yet rendered. Split out from
/// [`mount_write_volume`] so the witness can drive a real write verb and read the gate's recorded
/// answer without a console, and so the rendering lives in exactly one place.
enum WriteVolume {
    /// The program source mounted and admits ordinary file mutation.
    Admitted(FatFs),
    /// It mounted and REFUSES: the handle's name, the volume label, the reason.
    ReadOnly(&'static str, String, &'static str),
    /// Nothing mounted at all.
    NoVolume(FatError),
}

/// FATVERB: mount the volume a READ verb should act on — the program source, so `ls` and a bare
/// name are looking at the same card. Stamps [`READ_BIND`] either way, including on the decline:
/// "the read verbs asked and got nothing" is a different fact from "the read verbs never asked",
/// and Boot AR's symptom was the first one.
fn open_read_volume() -> Result<FatFs, FatError> {
    match crate::fs::fat::mount_program_source() {
        Ok(fs) => {
            stamp(&READ_BIND, &READ_BIND_SEQ, bind::of(&fs));
            Ok(fs)
        }
        Err(e) => {
            stamp(&READ_BIND, &READ_BIND_SEQ, bind::DECLINED);
            Err(e)
        }
    }
}

/// FATVERB: the WRITE gate. Mount the program source, then ask the BLOCK LAYER — through the one
/// predicate `crate::fs::fat::BlockSource::write_veto`, which the VFS's `FatBackend::read_only`
/// also forwards to — whether that source can be mutated at all. Runs before any directory entry,
/// cluster chain or FAT sector has been touched.
fn open_write_volume() -> WriteVolume {
    let fs = match open_read_volume() {
        Ok(fs) => fs,
        Err(e) => {
            stamp(&WRITE_GATE, &WRITE_GATE_SEQ, bind::DECLINED);
            return WriteVolume::NoVolume(e);
        }
    };
    match fs.write_veto() {
        None => {
            stamp(&WRITE_GATE, &WRITE_GATE_SEQ, bind::ADMITTED);
            WriteVolume::Admitted(fs)
        }
        Some(why) => {
            stamp(&WRITE_GATE, &WRITE_GATE_SEQ, bind::REFUSED_RO);
            WriteVolume::ReadOnly(fs.source_name(), fs.label(), why)
        }
    }
}

// FATVERB: TWO SINKS, TWO LENGTHS — and that is deliberate, not laziness.
//
// The first cut printed one ~235-character line to both. The panel is 128–180 columns depending on
// the scale metrics, so the census tail — the part that says WHICH handles existed, i.e. the whole
// diagnostic — was clipped off the right edge and never reached the eye it was written for. Serial
// has no such limit and a bench capture wants everything. So the operator gets a sentence and the
// capture gets the forensics, and they carry the same verdict word (`REFUSED READ-ONLY`) so one is
// greppable from the other.

/// FATVERB: the read verbs' decline, on both sinks. Boot AR's symptom WAS a read decline — `ls`
/// twice, nothing back — so the headless capture must carry it. Without the mirror a bench log
/// cannot tell "the verb declined, and here is what it was offered" from "the keystroke never
/// arrived", which is exactly the ambiguity that cost the first diagnosis.
fn mount_read_volume(console: &mut Console, verb: &str) -> Option<FatFs> {
    match open_read_volume() {
        Ok(fs) => Some(fs),
        Err(e) => {
            console.println(&alloc::format!("{}: no FAT filesystem ({:?})", verb, e));
            serial_println!(
                ":: [fatverb] {} -> NO VOLUME ({:?}; handles={}) ::",
                verb, e, crate::drivers::block::source_census()
            );
            None
        }
    }
}

/// FATVERB: the write verbs' gate, on both sinks. `None` means the caller has already been
/// explained to and must return WITHOUT mutating anything.
fn mount_write_volume(console: &mut Console, verb: &str) -> Option<FatFs> {
    match open_write_volume() {
        WriteVolume::Admitted(fs) => Some(fs),
        WriteVolume::ReadOnly(source, label, why) => {
            console.println(&alloc::format!("{}: REFUSED READ-ONLY ({})", verb, source));
            serial_println!(
                ":: [fatverb] {} -> REFUSED READ-ONLY (source={} label={} reason={}; handles={}) ::",
                verb,
                source,
                if label.is_empty() { "-" } else { &label },
                why,
                crate::drivers::block::source_census()
            );
            None
        }
        WriteVolume::NoVolume(e) => {
            console.println(&alloc::format!("{}: no FAT filesystem ({:?})", verb, e));
            serial_println!(
                ":: [fatverb] {} -> NO VOLUME ({:?}; handles={}) ::",
                verb, e, crate::drivers::block::source_census()
            );
            None
        }
    }
}

// ===================== FATVERB — THE SOURCE LAW, ENFORCED BY THE COMPILER ==========================
//
// The arc's claim is "no FAT verb in this file binds the default handle any more". That was true the
// day it landed and grep-true ever since, which is worth exactly nothing: the Boot AR defect WAS a
// call site left behind, and the next one will arrive the same way — someone adds a verb, copies the
// nearest neighbour, and the neighbour they copy is from a different file. A runtime witness cannot
// catch that; it can only observe the verbs it happens to drive.
//
// So the law is checked where it can actually be violated — at compile time, over this file's own
// source, on every `arroyo check` leg and every cfg combination. A re-introduced default-handle
// mount here is a BUILD ERROR naming this comment, not a silent regression that waits for a bench.
//
// SCOPE, honestly. The needle is the path spelling `::mount` immediately followed by an empty
// argument list — assembled below byte by byte, so that neither the needle nor this paragraph trips
// the law it defines. That covers `crate::fs::fat::mount(…)` with no arguments and every
// abbreviation of it that keeps the path separator. It does NOT catch a `use` import of the function
// followed by a bare call: a determined evasion defeats it. This is a tripwire against the failure
// that actually happens — a copied call site — not a sandbox. `mount_source` and
// `mount_program_source` do not match, which is the point: those are the callers we want.
//
// Prose elsewhere in this file deliberately names the function without parentheses for the same
// reason.
const _: () = {
    const SRC: &[u8] = include_bytes!("shell.rs");
    const NEEDLE: [u8; 9] = [b':', b':', b'm', b'o', b'u', b'n', b't', b'(', b')'];
    let mut i = 0usize;
    let mut hits = 0usize;
    while i + NEEDLE.len() <= SRC.len() {
        // Cheap first-byte reject, then the full compare — keeps const-eval linear and fast over a
        // file this size.
        if SRC[i] == NEEDLE[0] {
            let mut k = 0usize;
            while k < NEEDLE.len() && SRC[i + k] == NEEDLE[k] {
                k += 1;
            }
            if k == NEEDLE.len() {
                hits += 1;
            }
        }
        i += 1;
    }
    assert!(
        hits == 0,
        "FATVERB source law: shell.rs must not bind the default FAT handle. A file verb reads and \
         writes the PROGRAM SOURCE, through mount_read_volume / mount_write_volume — see the FATVERB \
         section. If you are only naming the function in prose, spell it without parentheses."
    );
};

/// JD6 `touch`: ensure a 0-length file exists at `path` in ANY directory the shell can reach
/// (create if absent; idempotent no-op if present). Rides the dir-aware `locate_in_dir` /
/// `create_in_dir` twins — the parent may be the root or any subdirectory.
fn fs_touch(console: &mut Console, arg: &str) {
    let Some(fs) = mount_write_volume(console, "touch") else { return };
    let (parent, name, canon) = match resolve_write_target(&fs, arg) {
        Ok(t) => t,
        Err(msg) => return console.println(&alloc::format!("touch: {}", msg)),
    };
    match fs.locate_in_dir(parent, &name) {
        Ok((de, _, _)) => console.println(&joined(&canon, de.name())), // already exists (canonical name)
        Err(FatError::NotFound) => match fs.create_in_dir(parent, &name, 0x20) {
            Ok((de, _, _)) => console.println(&joined(&canon, de.name())),
            Err(e) => console.println(&alloc::format!(
                "touch: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
        },
        Err(e) => console.println(&alloc::format!(
            "touch: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
    }
}

/// JD6 `write`: create-or-TRUNCATE a file at `path` in ANY reachable directory and store `data`
/// (the exact given bytes). Truncate of an existing file = free its chain + a fresh 0-length entry,
/// then grow-write — the only create-or-truncate reachable through fat.rs's PUBLIC API (there is no
/// in-place shrink primitive, and the directory-field publisher is private). A directory target is
/// refused (`-EISDIR`). Rides the dir-aware create_in_dir/locate_in_dir twins.
fn fs_write(console: &mut Console, arg: &str, data: &[u8]) {
    let Some(fs) = mount_write_volume(console, "write") else { return };
    let (parent, name, canon) = match resolve_write_target(&fs, arg) {
        Ok(t) => t,
        Err(msg) => return console.println(&alloc::format!("write: {}", msg)),
    };
    // Locate in the parent dir. Present-as-file -> TRUNCATE (delete then recreate); directory ->
    // refuse; absent -> create fresh. The result is the fresh entry's on-disk (dir_lba, dir_off).
    let (dir_lba, dir_off) = match fs.locate_in_dir(parent, &name) {
        Ok((de, dl, doff)) => {
            if de.is_dir {
                return console.println(&alloc::format!(
                    "write: {}: is a directory (-EISDIR)", joined(&canon, de.name())));
            }
            if let Err(e) = fs.delete_located(dl, doff, de.first_cluster()) {
                return console.println(&alloc::format!(
                    "write: {}: truncate failed {} ({:?})", joined(&canon, &name), fat_errno(e), e));
            }
            match fs.create_in_dir(parent, &name, 0x20) {
                Ok((_, dl2, doff2)) => (dl2, doff2),
                Err(e) => return console.println(&alloc::format!(
                    "write: {}: recreate failed {} ({:?}) — old file removed", joined(&canon, &name), fat_errno(e), e)),
            }
        }
        Err(FatError::NotFound) => match fs.create_in_dir(parent, &name, 0x20) {
            Ok((_, dl, doff)) => (dl, doff),
            Err(e) => return console.println(&alloc::format!(
                "write: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
        },
        Err(e) => return console.println(&alloc::format!(
            "write: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
    };
    if data.is_empty() {
        return console.println(&alloc::format!("wrote 0 bytes to {}", joined(&canon, &name)));
    }
    // The entry is a fresh 0-length file (first_cluster = 0, size = 0): grow from offset 0.
    match fs.write_grow(0, 0, dir_lba, dir_off, 0, data) {
        Ok((written, new_size, _)) => console.println(&alloc::format!(
            "wrote {} bytes to {} ({} bytes)", written, joined(&canon, &name), new_size)),
        Err(e) => console.println(&alloc::format!(
            "write: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
    }
}

/// JD6 `append`: append `data` at the end of a file at `path` in ANY reachable directory, creating
/// it if absent (like `>>`). The append grows the file from its current EOF via `write_grow`
/// (allocate + zero-fill + chain new clusters, directory `size` published LAST). A directory target
/// is refused (`-EISDIR`). Rides the dir-aware create_in_dir/locate_in_dir twins.
fn fs_append(console: &mut Console, arg: &str, data: &[u8]) {
    let Some(fs) = mount_write_volume(console, "append") else { return };
    let (parent, name, canon) = match resolve_write_target(&fs, arg) {
        Ok(t) => t,
        Err(msg) => return console.println(&alloc::format!("append: {}", msg)),
    };
    let (first_cluster, size, dir_lba, dir_off) = match fs.locate_in_dir(parent, &name) {
        Ok((de, dl, doff)) => {
            if de.is_dir {
                return console.println(&alloc::format!(
                    "append: {}: is a directory (-EISDIR)", joined(&canon, de.name())));
            }
            (de.first_cluster(), de.size, dl, doff)
        }
        Err(FatError::NotFound) => match fs.create_in_dir(parent, &name, 0x20) {
            Ok((de, dl, doff)) => (de.first_cluster(), de.size, dl, doff), // fresh: 0, 0
            Err(e) => return console.println(&alloc::format!(
                "append: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
        },
        Err(e) => return console.println(&alloc::format!(
            "append: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
    };
    if data.is_empty() {
        return console.println(&alloc::format!(
            "appended 0 bytes to {} ({} bytes)", joined(&canon, &name), size));
    }
    // Seek to EOF (`start = size`) and grow: write_grow appends the new bytes past the current end.
    match fs.write_grow(first_cluster, size, dir_lba, dir_off, size, data) {
        Ok((written, new_size, _)) => console.println(&alloc::format!(
            "appended {} bytes to {} ({} bytes)", written, joined(&canon, &name), new_size)),
        Err(e) => console.println(&alloc::format!(
            "append: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
    }
}

/// JD6 `rm`: delete a file at `path` in ANY reachable directory — `delete_located` marks the
/// directory slot `0xE5` FIRST, then frees the cluster chain (all FAT copies), the crash-safe order
/// fat.rs guarantees. A directory target is refused (`-EISDIR` — use `rmdir` for directories, JD7);
/// an absent name is `-ENOENT`, EXCEPT under `force` (the JD14 `-f` flag), which suppresses the
/// missing-target error quietly (POSIX `rm -f`); a wrong-usage `-EISDIR` is still shown under `-f`.
/// Rides the dir-aware locate_in_dir twin.
fn fs_rm(console: &mut Console, arg: &str, force: bool) {
    let Some(fs) = mount_write_volume(console, "rm") else { return };
    let (parent, name, canon) = match resolve_write_target(&fs, arg) {
        Ok(t) => t,
        // JD14: `-f` is lenient about a missing target (POSIX `rm -f NOSUCH` is quiet). A missing
        // parent component means the target does not exist, so force suppresses the message.
        Err(msg) => { if !force { console.println(&alloc::format!("rm: {}", msg)); } return; }
    };
    match fs.locate_in_dir(parent, &name) {
        Ok((de, dl, doff)) => {
            if de.is_dir {
                // A wrong-usage error (a directory without `-r`), NOT a "missing target" — shown even
                // under `-f`, exactly as POSIX `rm -f DIR` still complains.
                return console.println(&alloc::format!(
                    "rm: {}: is a directory (-EISDIR)", joined(&canon, de.name())));
            }
            match fs.delete_located(dl, doff, de.first_cluster()) {
                Ok(freed) => console.println(&alloc::format!(
                    "removed {} ({} cluster(s) freed)", joined(&canon, de.name()), freed.len())),
                Err(e) => console.println(&alloc::format!(
                    "rm: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
            }
        }
        // JD14: a missing leaf is quiet under `-f`.
        Err(FatError::NotFound) => { if !force { console.println(&alloc::format!(
            "rm: {}: not found (-ENOENT)", joined(&canon, &name))); } }
        Err(e) => console.println(&alloc::format!(
            "rm: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
    }
}

/// JD7 `mkdir`: create a new directory `path` in ANY reachable parent. Walks to the parent via
/// `resolve_write_target` (JD6), locates the leaf FIRST (`fat::create_dir` does NOT de-duplicate —
/// the inherited `create_in_dir` contract), then calls the `fat::create_dir` FATDIRS seam, which
/// allocates + `.`/`..`-initializes a fresh directory cluster and links a DIR-attr entry in the
/// parent. Honest errors: name already taken (file OR dir) → `-EEXIST`; parent missing → `-ENOENT`;
/// parent is a plain file → `-ENOTDIR` (both from `resolve_write_target`); volume or parent-dir full
/// → `-ENOSPC`; a non-8.3 name → `-EINVAL`. The root itself as a target → `-EISDIR` (it always exists).
fn fs_mkdir(console: &mut Console, arg: &str) {
    let Some(fs) = mount_write_volume(console, "mkdir") else { return };
    let (parent, name, canon) = match resolve_write_target(&fs, arg) {
        Ok(t) => t,
        Err(msg) => return console.println(&alloc::format!("mkdir: {}", msg)),
    };
    // create_dir does NOT de-duplicate — locate first so an existing name (file OR directory) is an
    // honest -EEXIST, never a duplicate directory slot in the parent.
    match fs.locate_in_dir(parent, &name) {
        Ok((de, _, _)) => console.println(&alloc::format!(
            "mkdir: {}: file exists (-EEXIST)", joined(&canon, de.name()))),
        Err(FatError::NotFound) => match fs.create_dir(parent, &name) {
            Ok((de, _, _)) => console.println(&alloc::format!(
                "created directory {}", joined(&canon, de.name()))),
            Err(e) => console.println(&alloc::format!(
                "mkdir: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
        },
        Err(e) => console.println(&alloc::format!(
            "mkdir: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
    }
}

/// JD7 `rmdir`: remove an EMPTY directory `path` from ANY reachable parent. Walks to the parent via
/// `resolve_write_target` (JD6), then calls the `fat::remove_dir` FATDIRS seam, which verifies the
/// target holds only `.`/`..` and frees its single cluster. Errno fidelity is shell-side (the seam
/// reuses existing `FatError` variants — see FATDIRS): the root is refused LOCALLY (`-EBUSY` — it is
/// never nameable and cluster 0 is not freeable); a FILE target is `-ENOTDIR` (resolved from the
/// parent walk BEFORE the call, so the seam's `Unsupported`-for-non-dir never surfaces here); an
/// absent name is `-ENOENT`; a NON-EMPTY directory maps the seam's `IsDirectory` → `-ENOTEMPTY`.
/// (`rm` stays file-only — a directory there is still `-EISDIR`; use `rmdir`.)
///
/// Note: removing the current working directory (e.g. `rmdir .` in an empty cwd, which normalizes to
/// the cwd path) succeeds and leaves the JD4 cwd stale — the very next cwd-relative command re-resolves
/// it and gets an honest `-ENOENT`, exactly the JD4 stale-cwd worst case (the cwd is a re-resolved path
/// string, not a cached chain head). No corruption — `delete_located` is crash-safe.
fn fs_rmdir(console: &mut Console, arg: &str) {
    let Some(fs) = mount_write_volume(console, "rmdir") else { return };
    // Refuse the root explicitly, with the honest errno. `resolve_write_target` would report the "/"
    // path as `-EISDIR`, but the volume root is never a removable directory (it is unnameable and
    // cluster 0 is not freeable). This also covers `rmdir .` at the root and `rmdir ..` that pops to it.
    if normalize_path(&cwd_path(), arg) == "/" {
        return console.println("rmdir: /: cannot remove the root directory (-EBUSY)");
    }
    let (parent, name, canon) = match resolve_write_target(&fs, arg) {
        Ok(t) => t,
        Err(msg) => return console.println(&alloc::format!("rmdir: {}", msg)),
    };
    match fs.locate_in_dir(parent, &name) {
        Ok((de, _, _)) => {
            if !de.is_dir {
                return console.println(&alloc::format!(
                    "rmdir: {}: not a directory (-ENOTDIR)", joined(&canon, de.name())));
            }
            match fs.remove_dir(parent, &name) {
                Ok(freed) => console.println(&alloc::format!(
                    "removed directory {} ({} cluster(s) freed)", joined(&canon, de.name()), freed.len())),
                // The seam maps a NON-EMPTY directory to `IsDirectory`; the shell owns the -ENOTEMPTY tag.
                Err(FatError::IsDirectory) => console.println(&alloc::format!(
                    "rmdir: {}: directory not empty (-ENOTEMPTY)", joined(&canon, de.name()))),
                Err(e) => console.println(&alloc::format!(
                    "rmdir: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
            }
        }
        Err(FatError::NotFound) => console.println(&alloc::format!(
            "rmdir: {}: not found (-ENOENT)", joined(&canon, &name))),
        Err(e) => console.println(&alloc::format!(
            "rmdir: {}: {} ({:?})", joined(&canon, &name), fat_errno(e), e)),
    }
}

/// JD8 `cp <src> <dst>`: copy a FILE to a new location, composing the read path (`read_at`) with the
/// JD6 create-or-truncate write path (`create_in_dir` + `write_grow`) — NO fat.rs mutation, exactly
/// JD7's nature (it only CALLS existing public API). `src` must be a file (a directory source is
/// `-EISDIR` — recursive `cp -r` is a JD9 candidate, out of scope this arc). If `dst` resolves to an
/// existing DIRECTORY the copy lands as `dst/<src-leaf>` (the `cp FILE DIR/` idiom); otherwise `dst`
/// names the destination file. JD14: no-clobber is the PANEL DEFAULT — an existing destination FILE is
/// refused (`-EEXIST`) unless `force` (`-f`) is set, which opts into truncate-in-place overwrite; `-n`
/// reasserts the default. (This aligns `cp` with `mv`'s pre-existing no-clobber default — a deliberate
/// divergence from POSIX `cp`, which overwrites silently; the panel favours safety + cp/mv symmetry.)
/// Honest errors:
/// src missing → `-ENOENT`; src is a dir → `-EISDIR`; dst exists (no `-f`) → `-EEXIST`; dst parent
/// missing → `-ENOENT`; dst parent is a file → `-ENOTDIR`; the volume/dir full → `-ENOSPC`; copying a
/// file onto itself (same canonical path) → `-EINVAL`.
///
/// SIZE HANDLING (JD8-M2 decision): the copy STREAMS the bytes in fixed windows via the offset-aware
/// `read_at` (existing public fat.rs API — the U9/read-path twin of `read_file`, so this reaches for
/// no NEW primitive) feeding `write_grow`, so a file of ANY size copies with a bounded
/// (`CP_WINDOW`-byte) heap footprint and NO truncation. There is deliberately no size ceiling. The
/// per-window `write_grow` re-walks the growing destination chain (bounded, and every FAT/data access
/// rides the JD3 wall-clock BOT pump — a stalled transfer is `-EIO`, never a hang on the timerless kernel
/// core); a future single-pass primitive could tighten that, tracked as a JD9 note.
fn fs_cp(console: &mut Console, src: &str, dst: &str, force: bool) {
    let Some(fs) = mount_write_volume(console, "cp") else { return };
    // --- Resolve the SOURCE to a concrete file (a directory source is out of scope). ---
    let src_norm = normalize_path(&cwd_path(), src);
    let (de_src, src_canon) = match resolve_path(&fs, &src_norm) {
        Ok(Resolved::Root) => return console.println("cp: /: is a directory (-EISDIR)"),
        Ok(Resolved::Entry(de, canon)) => {
            if de.is_dir {
                return console.println(&alloc::format!("cp: {}: is a directory (-EISDIR)", canon));
            }
            (de, canon)
        }
        Err(msg) => return console.println(&alloc::format!("cp: {}", msg)),
    };
    // --- Decide the DESTINATION path (the `cp FILE DIR/` idiom): an existing directory receives the
    //     copy under the source's canonical leaf name; anything else is the destination file itself. ---
    let dst_norm = normalize_path(&cwd_path(), dst);
    let dst_final = match resolve_path(&fs, &dst_norm) {
        Ok(Resolved::Root) => normalize_path("/", de_src.name()), // into the volume root
        Ok(Resolved::Entry(ref de, _)) if de.is_dir => normalize_path(&dst_norm, de_src.name()), // into a dir
        _ => dst_norm.clone(), // an existing file (overwrite) or a new name — resolve_write_target validates the parent
    };
    let (dparent, dleaf, dcanon) = match resolve_write_target(&fs, &dst_final) {
        Ok(t) => t,
        Err(msg) => return console.println(&alloc::format!("cp: {}", msg)),
    };
    let dest_disp = joined(&dcanon, &dleaf);
    // --- Refuse copying a file onto itself. Canonical paths are unique per file, so a case-insensitive
    //     full-path compare is complete (FAT 8.3 names are case-insensitive). ---
    if src_canon.eq_ignore_ascii_case(&dest_disp) {
        return console.println(&alloc::format!(
            "cp: {} and {} are the same file (-EINVAL)", src_canon, dest_disp));
    }
    // --- Create-or-truncate the destination and stream the bytes (shared with the JD9 `cp -r` per-file
    //     leg, so the streaming/errno logic lives in exactly one place). ---
    match copy_file_into(&fs, &de_src, &src_canon, dparent, &dleaf, &dcanon, force) {
        Ok(bytes) => console.println(&alloc::format!(
            "copied {} -> {} ({} bytes)", src_canon, dest_disp, bytes)),
        Err(msg) => console.println(&alloc::format!("cp: {}", msg)),
    }
}

/// JD8/JD9: copy ONE file `de_src` into directory `dparent` under leaf name `dleaf`, create-or-truncating
/// the destination. Returns the byte count copied, or a fully-formatted (path + errno) error string the
/// caller prefixes with its command name. This is the streaming core shared by `fs_cp` (the file `cp`) and
/// the JD9 `cp_tree` recursion — NO fat.rs mutation: it composes the offset-aware read `read_at` with the
/// JD6 create-or-truncate write path (`locate_in_dir`/`delete_located`/`create_in_dir` + `write_grow`),
/// all call-never-edit. `dcanon` is the destination parent's canonical path (for messages / the display).
///
/// SIZE HANDLING (the JD8-M2 decision, unchanged): STREAMS in fixed `CP_WINDOW`-byte windows, so a file of
/// ANY size copies with a bounded heap footprint and NO truncation (no size ceiling). Every FAT/data access
/// rides the JD3 wall-clock BOT pump — a stalled transfer is `-EIO`, never a hang on the timerless kernel core.
fn copy_file_into(
    fs: &FatFs,
    de_src: &DirEntry,
    src_canon: &str,
    dparent: u32,
    dleaf: &str,
    dcanon: &str,
    force: bool,
) -> Result<u64, String> {
    let dest_disp = joined(dcanon, dleaf);
    // --- Create-or-truncate the destination as a fresh 0-length file (the JD6 write prologue). ---
    let (dir_lba, dir_off) = match fs.locate_in_dir(dparent, dleaf) {
        Ok((de, dl, doff)) => {
            if de.is_dir {
                return Err(alloc::format!("{}: is a directory (-EISDIR)", joined(dcanon, de.name())));
            }
            // JD14: no-clobber is the panel default — an existing FILE destination is refused unless
            // `-f` was given (which opts into the overwrite/truncate below). The `cp -r` recursion
            // always writes into a freshly-created (empty) tree, so it passes `force = true` and this
            // guard never trips there.
            if !force {
                return Err(alloc::format!(
                    "{}: file exists (-EEXIST); use cp -f to overwrite", dest_disp));
            }
            fs.delete_located(dl, doff, de.first_cluster()).map_err(|e| {
                alloc::format!("{}: truncate failed {} ({:?})", dest_disp, fat_errno(e), e)
            })?;
            match fs.create_in_dir(dparent, dleaf, 0x20) {
                Ok((_, dl2, doff2)) => (dl2, doff2),
                Err(e) => return Err(alloc::format!(
                    "{}: recreate failed {} ({:?}) — old file removed", dest_disp, fat_errno(e), e)),
            }
        }
        Err(FatError::NotFound) => match fs.create_in_dir(dparent, dleaf, 0x20) {
            Ok((_, dl, doff)) => (dl, doff),
            Err(e) => return Err(alloc::format!("{}: {} ({:?})", dest_disp, fat_errno(e), e)),
        },
        Err(e) => return Err(alloc::format!("{}: {} ({:?})", dest_disp, fat_errno(e), e)),
    };
    // --- Stream the bytes: read_at windows -> write_grow appends. The destination entry starts as a
    //     fresh 0-length file (first_cluster 0, size 0); write_grow allocates + publishes as it grows. ---
    const CP_WINDOW: usize = 32 * 1024;
    let src_fc = de_src.first_cluster();
    let src_size = de_src.size;
    let (mut dst_first, mut dst_size, mut off) = (0u32, 0u32, 0u32);
    let mut buf: Vec<u8> = Vec::new();
    while off < src_size {
        buf.clear();
        fs.read_at(src_fc, src_size, off, &mut buf, CP_WINDOW).map_err(|e| {
            alloc::format!("{}: read failed {} ({:?})", src_canon, fat_errno(e), e)
        })?;
        if buf.is_empty() {
            break; // the source chain ended before de.size (malformed) — copy what it holds, honestly
        }
        match fs.write_grow(dst_first, dst_size, dir_lba, dir_off, off, &buf) {
            Ok((_, new_size, new_first)) => {
                dst_first = new_first;
                dst_size = new_size;
                off += buf.len() as u32;
            }
            Err(e) => return Err(alloc::format!(
                "{}: write failed {} ({:?})", dest_disp, fat_errno(e), e)),
        }
    }
    Ok(dst_size as u64)
}

/// JD9: true if `path` lies strictly INSIDE `ancestor` — both are canonical absolute 8.3 paths, compared
/// case-insensitively (short names are stored uppercase). `is_descendant("/DOCS/SUB", "/DOCS") == true`;
/// `is_descendant("/DOCS", "/DOCS") == false`; `is_descendant("/DOCSX", "/DOCS") == false` (the `/` guard
/// blocks a false prefix match). Paths are pure ASCII, so byte-indexing at `ancestor.len()` is
/// char-boundary-safe. Used to refuse `cp -r` of a directory into itself or one of its own descendants.
fn is_descendant(path: &str, ancestor: &str) -> bool {
    path.len() > ancestor.len()
        && path.as_bytes()[ancestor.len()] == b'/'
        && path[..ancestor.len()].eq_ignore_ascii_case(ancestor)
}

/// JD9: a running tally of a `cp -r` for the summary / partial-failure report.
struct CpStats {
    dirs: u32,
    files: u32,
    bytes: u64,
}

/// JD9: the maximum directory nesting `cp -r` will descend before refusing with `-ELOOP`. A sane bound so a
/// pathologically deep (or, on a malformed volume, self-referential — though `read_dir`'s own chain-loop
/// guard already backstops that) tree yields an honest error, never a stack blow-out. FAT paths are shallow
/// in practice; 32 is far past any real console tree.
const CP_MAX_DEPTH: u32 = 32;

/// JD9: recursively copy the CONTENTS of the source directory (cluster `src_cluster`, canonical path
/// `src_canon`) INTO the already-created destination directory (cluster `dst_cluster`, canonical path
/// `dst_canon`). `.`/`..` are filtered at every level; a child file rides `copy_file_into`, a child
/// directory is freshly `create_dir`'d and recursed into. `stats` accumulates across the whole tree so the
/// caller can report an honest partial count if a mid-tree op fails. Returns a fully-formatted error string
/// (path + errno) on the FIRST failure — the copy stops there, no silent truncation. Every op rides the JD3
/// BOT pump (bounded, never a hang). The destination subtree is created fresh by the caller's `-EEXIST`
/// pre-check, so no child name can pre-exist — each `create_dir`/`copy_file_into` writes into empty space.
fn cp_tree(
    fs: &FatFs,
    src_cluster: u32,
    src_canon: &str,
    dst_cluster: u32,
    dst_canon: &str,
    depth: u32,
    stats: &mut CpStats,
) -> Result<(), String> {
    if depth > CP_MAX_DEPTH {
        return Err(alloc::format!(
            "{}: maximum directory depth {} exceeded (-ELOOP)", dst_canon, CP_MAX_DEPTH));
    }
    let entries = fs
        .read_dir(src_cluster)
        .map_err(|e| alloc::format!("{}: read failed ({:?}, -EIO)", src_canon, e))?;
    for de in &entries {
        let nm = de.name();
        if nm == "." || nm == ".." {
            continue; // skip the self/parent links a subdirectory cluster carries
        }
        let child_src = joined(src_canon, nm);
        let child_dst = joined(dst_canon, nm);
        if de.is_dir {
            let (cde, _, _) = fs.create_dir(dst_cluster, nm).map_err(|e| {
                alloc::format!("{}: {} ({:?})", child_dst, fat_errno(e), e)
            })?;
            stats.dirs += 1;
            cp_tree(fs, de.first_cluster(), &child_src, cde.first_cluster(), &child_dst, depth + 1, stats)?;
        } else {
            // The destination subtree is freshly created (empty), so no child name can pre-exist —
            // pass `force = true` so the JD14 no-clobber guard never trips inside a fresh `cp -r` tree.
            let bytes = copy_file_into(fs, de, &child_src, dst_cluster, nm, dst_canon, true)?;
            stats.files += 1;
            stats.bytes += bytes;
        }
    }
    Ok(())
}

/// JD9 `cp -r <srcdir> <dstdir>`: recursively copy a directory tree. Composes the read walk (`read_dir`),
/// directory creation (the FATDIRS `create_dir` seam, via the JD7 idiom), and the JD8 per-file streaming
/// copy — all `shell.rs`-only, NO fat.rs mutation (call-never-edit). Guards, in order:
///   * a ROOT source is refused (`-EINVAL`) — the volume root has no leaf name to copy AS, and any
///     in-volume destination is a descendant of it (the next guard would refuse it anyway);
///   * a FILE source degrades to a plain file copy (`fs_cp`) — POSIX-friendly, honest;
///   * the destination path follows the `cp DIR DEST` idiom: an existing directory (or root) receives the
///     tree AS `DEST/<src-leaf>`; a not-yet-existing DEST becomes the new tree; an existing FILE is
///     `-ENOTDIR`;
///   * copying a directory into itself or one of its own descendants is refused (`-EINVAL`,
///     canonical-path prefix compare) — this is what stops an infinite `cp -r DOCS DOCS/SUB`;
///   * the top-level target must NOT already exist — `-EEXIST` under the no-clobber default, so `cp -r`
///     creates a FRESH tree and never silently merges into an existing one. JD15: `cp -rf` opts into
///     TREE-REPLACE — the existing target (file or whole directory tree) is deleted first
///     (delete-dst-first, crash-safe-partial per `force_remove_existing`) and the fresh tree is then
///     built as normal. Because the top-level target is always fresh at build time, every directory
///     `cp_tree` creates below it is inside freshly-created (so empty) parents — no child can collide.
/// A mid-tree failure stops and reports the honest partial count (dirs/files/bytes copied so far) plus the
/// failing path + errno; nothing is rolled back (a partial tree is left on disk, crash-safe per the
/// FATDIRS/JD6 ordering — the operator can `rmdir`/`rm` it).
fn fs_cp_recursive(console: &mut Console, src: &str, dst: &str, force: bool) {
    let Some(fs) = mount_write_volume(console, "cp") else { return };
    // --- Resolve the SOURCE. Root is refused; a file degrades to the plain file copy. ---
    let src_norm = normalize_path(&cwd_path(), src);
    let (de_src, src_canon) = match resolve_path(&fs, &src_norm) {
        Ok(Resolved::Root) => return console.println("cp: -r /: cannot copy the volume root (-EINVAL)"),
        Ok(Resolved::Entry(de, canon)) => (de, canon),
        Err(msg) => return console.println(&alloc::format!("cp: {}", msg)),
    };
    if !de_src.is_dir {
        return fs_cp(console, src, dst, force); // `cp -r FILE DST` == `cp FILE DST` (JD14: -f honoured)
    }
    // --- Decide the TARGET directory path (the `cp DIR DEST` idiom). ---
    let dst_norm = normalize_path(&cwd_path(), dst);
    let target = match resolve_path(&fs, &dst_norm) {
        Ok(Resolved::Root) => normalize_path("/", de_src.name()), // into the volume root
        Ok(Resolved::Entry(ref de, _)) if de.is_dir => normalize_path(&dst_norm, de_src.name()), // into a dir
        Ok(Resolved::Entry(_, canon)) =>
            return console.println(&alloc::format!("cp: {}: not a directory (-ENOTDIR)", canon)),
        Err(_) => dst_norm.clone(), // does not exist yet — DEST itself becomes the new tree
    };
    // --- Guard: refuse copying a directory into itself or one of its own descendants. ---
    if target.eq_ignore_ascii_case(&src_canon) || is_descendant(&target, &src_canon) {
        return console.println(&alloc::format!(
            "cp: cannot copy directory {} into itself or its own subtree ({}) (-EINVAL)",
            src_canon, target));
    }
    // --- The top-level target must not already exist (fresh-tree rule → honest -EEXIST). JD15: `-f`
    //     (`cp -rf`) now opts into TREE-REPLACE — delete whatever exists at the target first
    //     (delete-dst-first, crash-safe-partial per `force_remove_existing`), then fall through to
    //     create a FRESH tree. Without `-f` an existing target stays -EEXIST (no-clobber default). ---
    if let Ok(existing) = resolve_path(&fs, &target) {
        if !force {
            return console.println(&alloc::format!(
                "cp: {}: already exists (-EEXIST); use cp -rf to replace it, or rm -r it first", target));
        }
        let de_existing = match existing {
            Resolved::Entry(de, _) => de,
            // `target` is never the volume root (it is always a leaf under some parent), so this arm
            // is unreachable — refuse defensively rather than clobber.
            Resolved::Root => return console.println("cp: -rf /: refusing to replace the volume root (-EBUSY)"),
        };
        let (rp, rl, _rc) = match resolve_write_target(&fs, &target) {
            Ok(t) => t,
            Err(msg) => return console.println(&alloc::format!("cp: {}", msg)),
        };
        if let Err(msg) = force_remove_existing(&fs, &de_existing, rp, &rl, &target) {
            return console.println(&alloc::format!("cp: -rf: replace failed: {}", msg));
        }
    }
    // --- Create the top-level target directory (its parent must exist), then recurse into it. ---
    let (tparent, tleaf, tcanon) = match resolve_write_target(&fs, &target) {
        Ok(t) => t,
        Err(msg) => return console.println(&alloc::format!("cp: {}", msg)),
    };
    let top = match fs.create_dir(tparent, &tleaf) {
        Ok((de, _, _)) => de,
        Err(e) => return console.println(&alloc::format!(
            "cp: {}: {} ({:?})", joined(&tcanon, &tleaf), fat_errno(e), e)),
    };
    let target_canon = joined(&tcanon, top.name());
    let mut stats = CpStats { dirs: 1, files: 0, bytes: 0 }; // the top dir counts
    match cp_tree(&fs, de_src.first_cluster(), &src_canon, top.first_cluster(), &target_canon, 1, &mut stats) {
        Ok(()) => console.println(&alloc::format!(
            "copied {}/ -> {}/ ({} dir(s), {} file(s), {} bytes)",
            src_canon, target_canon, stats.dirs, stats.files, stats.bytes)),
        Err(msg) => console.println(&alloc::format!(
            "cp: {} [partial: {} dir(s), {} file(s), {} bytes copied before the error]",
            msg, stats.dirs, stats.files, stats.bytes)),
    }
}

/// JD13: a running tally of a `rm -r` for the summary / partial-failure report (the delete twin of
/// `CpStats`, no byte count — a delete moves no data).
struct RmStats {
    dirs: u32,
    files: u32,
}

/// JD13: recursively delete the CONTENTS of the directory (cluster `dir_cluster`, canonical path
/// `dir_canon`) — child FILES then child DIRECTORIES, depth-first, so a directory is emptied before it
/// is removed. `.`/`..` are filtered at every level (the JD9 `cp_tree` walk shape, inverted for delete).
/// A child file is unlinked via `locate_in_dir` + `delete_located` (the `fs_rm` primitives, run QUIET —
/// no per-file console line, so a whole tree yields ONE summary like `cp -r`); a child directory is
/// recursed into and then `remove_dir`'d (it now holds only `.`/`..`). `stats` accumulates across the
/// whole tree so the caller reports an honest partial count if a mid-tree op fails. Returns a
/// fully-formatted error string (path + errno) on the FIRST failure — the delete stops there, nothing
/// is rolled back (crash-safe per the U10 `0xE5`-then-free ordering; the operator can re-run `rm -r`).
///
/// SNAPSHOT-then-delete (the JD12 glob-safety property, carried into the recursion): `read_dir` captures
/// the entry list before any mutation, and each child is re-located BY NAME (`0xE5`-marking one sibling
/// never moves another entry's slot), so deleting as we go never invalidates the walk. Every op rides
/// the JD3 wall-clock BOT pump (bounded — a stalled transfer is `-EIO`, never a hang on the timerless
/// kernel core). Depth is capped at `CP_MAX_DEPTH` (honest `-ELOOP`) — the JD9 belt-and-braces backstop
/// against a malformed self-referential volume (`read_dir`'s own chain-loop guard is the first line).
fn rm_tree(
    fs: &FatFs,
    dir_cluster: u32,
    dir_canon: &str,
    depth: u32,
    stats: &mut RmStats,
) -> Result<(), String> {
    if depth > CP_MAX_DEPTH {
        return Err(alloc::format!(
            "{}: maximum directory depth {} exceeded (-ELOOP)", dir_canon, CP_MAX_DEPTH));
    }
    // SNAPSHOT the directory contents before any deletion (so a delete never invalidates the walk).
    let entries = fs
        .read_dir(dir_cluster)
        .map_err(|e| alloc::format!("{}: read failed ({:?}, -EIO)", dir_canon, e))?;
    for de in &entries {
        let nm = de.name();
        if nm == "." || nm == ".." {
            continue; // skip the self/parent links a subdirectory cluster carries
        }
        let child = joined(dir_canon, nm);
        if de.is_dir {
            // Empty the child directory first, THEN remove it (it now holds only `.`/`..`).
            rm_tree(fs, de.first_cluster(), &child, depth + 1, stats)?;
            fs.remove_dir(dir_cluster, nm)
                .map_err(|e| alloc::format!("{}: {} ({:?})", child, fat_errno(e), e))?;
            stats.dirs += 1;
        } else {
            // Unlink the child file BY NAME (re-locate its slot, then delete) — the `fs_rm` primitives.
            let (fde, dl, doff) = fs
                .locate_in_dir(dir_cluster, nm)
                .map_err(|e| alloc::format!("{}: {} ({:?})", child, fat_errno(e), e))?;
            fs.delete_located(dl, doff, fde.first_cluster())
                .map_err(|e| alloc::format!("{}: {} ({:?})", child, fat_errno(e), e))?;
            stats.files += 1;
        }
    }
    Ok(())
}

/// JD15 `-f` tree-replace primitive — remove WHATEVER currently occupies a destination so a forced
/// copy/move can then create a FRESH one. A FILE is unlinked via the `fs_rm` primitives
/// (`locate_in_dir` + `delete_located`); a DIRECTORY is emptied by the JD13 `rm_tree` and then
/// `remove_dir`'d (the same call-never-edit composition `rm -r` uses — zero fat.rs mutation). `de` is
/// the already-resolved destination entry; `parent`/`leaf` locate its slot in the parent directory;
/// `canon` is its canonical path (for honest error text). Returns Ok when the destination is now
/// absent, or a formatted `path: reason (errno)` string on the first failure.
///
/// ⚠ CRASH-SAFE-PARTIAL (the JD13 honest-count discipline): this deletes the destination BEFORE the
/// caller's fresh copy/move. A power cut in the delete→recreate window therefore leaves the
/// destination ABSENT — never a half-overwritten or silently-merged tree. Nothing is rolled back; the
/// operator re-runs the `cp -rf`/`mv -f` to complete it. `-f` tree-replace trades the plain `-EEXIST`
/// refusal for this bounded, honest window; no-clobber stays the panel DEFAULT (only `-f` opts in).
fn force_remove_existing(
    fs: &FatFs,
    de: &DirEntry,
    parent: u32,
    leaf: &str,
    canon: &str,
) -> Result<(), String> {
    if de.is_dir {
        // Empty the destination subtree (child files then child dirs, depth-first), then remove the
        // now-empty directory itself — exactly the `fs_rm_recursive` composition, run QUIET here.
        let mut stats = RmStats { dirs: 0, files: 0 };
        rm_tree(fs, de.first_cluster(), canon, 1, &mut stats)?;
        fs.remove_dir(parent, leaf)
            .map_err(|e| alloc::format!("{}: {} ({:?})", canon, fat_errno(e), e))?;
    } else {
        // A plain FILE destination — re-locate its slot BY NAME (DirEntry carries no slot coords) and
        // unlink it, the same `fs_rm` primitive pair.
        let (fde, dl, doff) = fs
            .locate_in_dir(parent, leaf)
            .map_err(|e| alloc::format!("{}: {} ({:?})", canon, fat_errno(e), e))?;
        fs.delete_located(dl, doff, fde.first_cluster())
            .map_err(|e| alloc::format!("{}: {} ({:?})", canon, fat_errno(e), e))?;
    }
    Ok(())
}

/// JD13 `rm -r <path>` (also `rm -R`): recursively delete a directory tree — files then directories,
/// depth-first, so every directory is emptied before it is removed. `shell.rs`-only, NO fat.rs mutation:
/// it composes `read_dir` (the walk), the `fs_rm` file-delete primitives (`locate_in_dir` +
/// `delete_located`), and the `rmdir` primitive (`remove_dir`) — all call-never-edit. Guards, in order:
///   * the ROOT is refused (`-EBUSY`) — a recursive delete of the whole volume is a footgun, and the
///     volume root is never a removable directory (cluster 0 is not freeable). Checked LOCALLY before any
///     walk, mirroring `fs_rmdir`'s root refusal; also catches `rm -r .` at the root and `rm -r ..` that
///     pops to it;
///   * a FILE target degrades to a plain file delete (`fs_rm`) — POSIX-friendly (`rm -r FILE` == `rm FILE`);
///   * a DIRECTORY target is emptied by `rm_tree`, then the now-empty top directory itself is removed
///     (counted in the summary).
/// A mid-tree failure stops and reports the honest partial count (dirs/files removed so far) plus the
/// failing path + errno; nothing is rolled back (crash-safe per the U10 `0xE5`-then-free ordering — the
/// operator can re-run `rm -r` to clear the remainder). Recursion is depth-capped (`CP_MAX_DEPTH`,
/// honest `-ELOOP`).
///
/// PRINCIPAL — unchanged. The shell is kernel ASID 0, the PUBLIC principal; `rm -r` consults no U6/K-line
/// `OWNED_FILES` ACL and composes the same F3-locked `read_dir`/`locate_in_dir`/`delete_located`/
/// `remove_dir` primitives JD6/JD7/JD9 already exercise and ledger, so it inherits their locking
/// analysis unchanged (no new fat.rs surface, no new lock, no new namespace interaction).
fn fs_rm_recursive(console: &mut Console, arg: &str, force: bool) {
    let Some(fs) = mount_write_volume(console, "rm") else { return };
    // Refuse the root explicitly, with the honest errno, BEFORE any walk. `normalize_path` folds
    // `rm -r .` at the root and `rm -r ..` that pops to it into "/". The root refusal stands even
    // under `-f` — `rm -rf /` is a footgun the panel never honours (cluster 0 is unremovable).
    let norm = normalize_path(&cwd_path(), arg);
    if norm == "/" {
        return console.println("rm: -r /: cannot remove the root directory (-EBUSY)");
    }
    // Resolve the target. A FILE degrades to a plain `rm`; a DIRECTORY is the recursive case. Under
    // `-f`, a missing target is quiet (POSIX `rm -rf NOSUCH`).
    let (de_src, src_canon) = match resolve_path(&fs, &norm) {
        Ok(Resolved::Root) =>
            return console.println("rm: -r /: cannot remove the root directory (-EBUSY)"),
        Ok(Resolved::Entry(de, canon)) => (de, canon),
        Err(msg) => { if !force { console.println(&alloc::format!("rm: {}", msg)); } return; }
    };
    if !de_src.is_dir {
        return fs_rm(console, arg, force); // `rm -r FILE` == `rm FILE` (JD14: -f honoured)
    }
    // A directory: walk to its parent so the now-empty top directory can be removed after `rm_tree`.
    let (parent, leaf, parent_canon) = match resolve_write_target(&fs, &norm) {
        Ok(t) => t,
        Err(msg) => { if !force { console.println(&alloc::format!("rm: {}", msg)); } return; }
    };
    let mut stats = RmStats { dirs: 0, files: 0 };
    match rm_tree(&fs, de_src.first_cluster(), &src_canon, 1, &mut stats) {
        Ok(()) => match fs.remove_dir(parent, &leaf) {
            Ok(_) => {
                stats.dirs += 1; // the top directory itself
                console.println(&alloc::format!(
                    "removed {}/ ({} dir(s), {} file(s))", src_canon, stats.dirs, stats.files));
            }
            Err(e) => console.println(&alloc::format!(
                "rm: {}: {} ({:?}) [partial: {} dir(s), {} file(s) removed before the error]",
                joined(&parent_canon, &leaf), fat_errno(e), e, stats.dirs, stats.files)),
        },
        Err(msg) => console.println(&alloc::format!(
            "rm: {} [partial: {} dir(s), {} file(s) removed before the error]",
            msg, stats.dirs, stats.files)),
    }
}

/// JD10 `mv <src> <dst>` (aliases `move`/`ren`/`rename`): move OR rename a file or directory by
/// RELINKING its directory entry — the file's data never moves (O(1), by reference), composing the
/// FATMOVE `rename_entry`/`move_entry` seam with the JD6 path-resolution idioms (call-never-edit; no
/// fat.rs mutation of our own). Two dispatches, decided by whether source and destination share a
/// parent directory:
///   * SAME parent → `rename_entry` (rewrites the 8.3 name in the existing directory entry in place;
///     works on files AND directories — an in-place rename leaves `first_cluster` untouched, so a
///     renamed directory's own `.`/`..` and its children's `..` stay correct: `mv DIR NEWNAME` is
///     O(1), no `mv -r` needed unlike `cp -r`);
///   * DIFFERENT parents → `move_entry` (re-publishes the entry over the SAME `first_cluster` in the
///     new parent, then `0xE5`s the old name WITHOUT freeing the chain — the data moves by reference).
///     FILES only: a directory across parents needs its `..` rewritten to the new parent (out of the
///     seam's scope) → honest `-EISDIR` (rename it in place, or `cp -r` + `rm -r`).
/// The `mv SRC DIR/` idiom lands the entry under DIR as the source's own leaf name; otherwise DST
/// names the target directly (rename / move-with-new-name). Guards, in order: a DIRECTORY moved onto
/// itself or into its own subtree is refused (`-EINVAL`, the JD9 `is_descendant` canonical-prefix
/// compare); the destination must not already exist (no-clobber panel default → `-EEXIST`, mirroring
/// the FATMOVE seam's own dest-exists refusal shell-side) — EXCEPT a rename to the source's own
/// canonical name (same parent + same leaf), which the seam treats as a no-op success. JD14: `-f`
/// (force) opts into overwriting an existing destination — the existing file (JD14) OR directory
/// TREE (JD15: emptied via `rm_tree` + `remove_dir`) is deleted first (delete-dst-first,
/// crash-safe-partial), then the entry is relinked into the freed slot. Honest errno
/// surface: src missing → `-ENOENT`; root as src → `-EBUSY`; dst parent missing → `-ENOENT`; dst
/// parent is a file → `-ENOTDIR`; dst dir full → `-ENOSPC`; a non-8.3 dst name → `-EINVAL`; a
/// directory across parents → `-EISDIR`.
///
/// ACL NOTE: this shell is kernel ASID 0 = the PUBLIC principal, so a panel `mv` consults no U6/K-line
/// `OWNED_FILES` ACL and is ACL-neutral by construction (the row re-key for a moved user-owned file is
/// a future K-line seam, ledgered in the pi4 FATMOVE SECURITY note). CRASH SAFETY is the seam's job:
/// `move_entry` publishes the destination BEFORE `0xE5`ing the source, so a power-cut mid-move leaves
/// a benign duplicate (two names, one chain), never a lost chain.
fn fs_mv(console: &mut Console, src: &str, dst: &str, force: bool) {
    let Some(fs) = mount_write_volume(console, "mv") else { return };
    // --- Resolve the SOURCE to a concrete entry (file or dir). The volume root has no leaf name to
    //     move AS, so it is refused. ---
    let src_norm = normalize_path(&cwd_path(), src);
    let (de_src, src_canon) = match resolve_path(&fs, &src_norm) {
        Ok(Resolved::Root) => return console.println("mv: /: cannot move the volume root (-EBUSY)"),
        Ok(Resolved::Entry(de, canon)) => (de, canon),
        Err(msg) => return console.println(&alloc::format!("mv: {}", msg)),
    };
    // The source's parent directory (first-cluster id; 0 ⇒ root). Since SRC exists, its parent walk
    // succeeds; the returned leaf is the user-typed spelling — we use the canonical `de_src.name()`.
    let (src_parent, _src_leaf_typed, _src_parent_canon) = match resolve_write_target(&fs, &src_norm) {
        Ok(t) => t,
        Err(msg) => return console.println(&alloc::format!("mv: {}", msg)),
    };
    let src_leaf = de_src.name(); // the canonical on-disk 8.3 leaf
    // --- Decide the DESTINATION (the `mv SRC DIR/` idiom): an existing directory (or the root)
    //     receives the entry under the source's own leaf; anything else names the destination itself. ---
    let dst_norm = normalize_path(&cwd_path(), dst);
    let dst_final = match resolve_path(&fs, &dst_norm) {
        Ok(Resolved::Root) => normalize_path("/", src_leaf), // into the volume root
        Ok(Resolved::Entry(ref de, _)) if de.is_dir => normalize_path(&dst_norm, src_leaf), // into a dir
        _ => dst_norm.clone(), // an existing file (→ -EEXIST below) or a new name — validated next
    };
    let (dparent, dleaf, dcanon) = match resolve_write_target(&fs, &dst_final) {
        Ok(t) => t,
        Err(msg) => return console.println(&alloc::format!("mv: {}", msg)),
    };
    let dest_disp = joined(&dcanon, &dleaf);
    // --- Guard: refuse moving a DIRECTORY onto itself or into its own subtree (the JD9 prefix compare;
    //     also the right message before the seam would otherwise refuse a cross-parent dir move). ---
    if de_src.is_dir && (dest_disp.eq_ignore_ascii_case(&src_canon) || is_descendant(&dest_disp, &src_canon)) {
        return console.println(&alloc::format!(
            "mv: cannot move directory {} into itself or its own subtree ({}) (-EINVAL)",
            src_canon, dest_disp));
    }
    // --- Guard: a DIRECTORY source that would cross parents cannot be moved (its `..` needs rewriting —
    //     the seam refuses with IsDirectory). Surface that BEFORE any `-f` delete-dst-first below, so
    //     force never removes the destination for a move that is going to fail anyway. ---
    if de_src.is_dir && src_parent != dparent {
        return console.println(&alloc::format!(
            "mv: {}: cannot move a directory across directories (-EISDIR); rename it in place or use cp -r + rm -r",
            src_canon));
    }
    // --- Dest pre-check (no-clobber). Skip it only when the destination IS the source (same parent +
    //     same canonical leaf) — a rename to the same name, which `rename_entry` treats as a no-op. ---
    let same_target = src_parent == dparent && dleaf.eq_ignore_ascii_case(src_leaf);
    if !same_target {
        match fs.locate_in_dir(dparent, &dleaf) {
            Ok((de, dl, doff)) => {
                // JD14: no-clobber is the default — an existing destination is `-EEXIST` unless `-f`.
                if !force {
                    return console.println(&alloc::format!(
                        "mv: {}: file exists (-EEXIST); use mv -f to overwrite", joined(&dcanon, de.name())));
                }
                // `-f`: overwrite the existing destination by removing it first (delete-dst-first),
                // then the rename/move below re-publishes the entry into the freed slot. JD15: a
                // DIRECTORY destination is now TREE-REPLACED too (emptied via `rm_tree` + `remove_dir`
                // by `force_remove_existing`), not just a plain FILE — crash-safe-partial per JD13
                // (a power cut in the window leaves the destination absent, never merged). The `_ = dl,
                // doff` slot coords are re-derived inside the helper by name.
                let _ = (dl, doff);
                let dest_leaf = de.name();
                let dest_canon = joined(&dcanon, dest_leaf);
                if let Err(msg) = force_remove_existing(&fs, &de, dparent, dest_leaf, &dest_canon) {
                    return console.println(&alloc::format!(
                        "mv: -f: overwrite (remove existing) failed: {}", msg));
                }
            }
            Err(FatError::NotFound) => {}
            Err(e) => return console.println(&alloc::format!(
                "mv: {}: {} ({:?})", dest_disp, fat_errno(e), e)),
        }
    }
    // --- Dispatch by parent: same dir → rename in place (files AND dirs); across dirs → move (files
    //     only — the seam refuses a directory source with IsDirectory). Both are O(1) entry relinks. ---
    let (verb, result) = if src_parent == dparent {
        ("renamed", fs.rename_entry(src_parent, src_leaf, &dleaf))
    } else {
        ("moved", fs.move_entry(src_parent, src_leaf, dparent, &dleaf))
    };
    match result {
        Ok((de, _, _)) => console.println(&alloc::format!(
            "{} {} -> {}", verb, src_canon, joined(&dcanon, de.name()))),
        // move_entry returns IsDirectory when the SOURCE is a directory crossing parents.
        Err(FatError::IsDirectory) => console.println(&alloc::format!(
            "mv: {}: cannot move a directory across directories (-EISDIR); rename it in place or use cp -r + rm -r",
            src_canon)),
        Err(e) => console.println(&alloc::format!(
            "mv: {}: {} ({:?})", dest_disp, fat_errno(e), e)),
    }
}

/// JD12: render bytes as printable text for the console — LF is kept as a line break, CR is dropped,
/// and any other non-printing byte renders as `.`. The single rendering rule shared by `cat`/`head`/
/// `tail` so a file reads identically however it is viewed (and mirrors identically into the JD11
/// serial transcript). Returns the whole rendered string; the caller splits on `'\n'` to print lines.
fn render_text(data: &[u8]) -> String {
    data.iter().filter_map(|&b| match b {
        b'\n' => Some('\n'),
        b'\r' => None,
        0x20..=0x7e => Some(b as char),
        _ => Some('.'),
    }).collect()
}

/// JD12: the `cat` core — read a resolved FILE entry (bounded to `CAP` bytes so a huge file can't
/// flood the console) and print it as printable text, noting a byte-bounded short read. Shared by the
/// single-path `cat <file>` and the wildcard `cat *.EXT` (JD12-M2), so the rendering + truncation note
/// live in exactly one place. `de` must be a file — a directory surfaces `-EISDIR` from `read_file`.
fn cat_render(console: &mut Console, fs: &FatFs, de: &DirEntry, canon: &str) {
    const CAP: usize = 8192;
    let mut data: Vec<u8> = Vec::new();
    match fs.read_file(de, &mut data, CAP) {
        Ok(()) => {
            for line in render_text(&data).split('\n') {
                console.println(line);
            }
            // Bound the read so a huge file (e.g. kernel.elf) can't flood the console.
            if (de.size as usize) > data.len() {
                console.println(&alloc::format!(
                    "[... {} of {} bytes shown]", data.len(), de.size));
            }
        }
        Err(FatError::IsDirectory) =>
            console.println(&alloc::format!("cat: {}: is a directory (-EISDIR)", canon)),
        Err(e) => console.println(&alloc::format!("cat: {}: {:?}", canon, e)),
    }
}

/// JD12 `head <path> [n]`: print the FIRST `n` lines of a file (default 10). Streams from offset 0 via
/// the offset-aware `read_at` in bounded windows and STOPS as soon as `n` newlines are seen — so
/// `head 10` of a huge file reads only the first window(s), never the whole file. A byte ceiling
/// (`HEAD_MAX`) backstops a file with no (or too few) newlines so an unterminated giant line still
/// bounds the read and the heap. A directory or the root is `-EISDIR`. Every access rides the JD3
/// wall-clock BOT pump — a stalled transfer is `-EIO`, never a hang on the timerless kernel core.
fn fs_head(console: &mut Console, arg: &str, n: u32) {
    let Some(fs) = mount_read_volume(console, "head") else { return };
    let (de, canon) = match resolve_path(&fs, &normalize_path(&cwd_path(), arg)) {
        Ok(Resolved::Root) => return console.println("head: /: is a directory (-EISDIR)"),
        Ok(Resolved::Entry(de, canon)) => {
            if de.is_dir {
                return console.println(&alloc::format!("head: {}: is a directory (-EISDIR)", canon));
            }
            (de, canon)
        }
        Err(msg) => return console.println(&alloc::format!("head: {}", msg)),
    };
    const WINDOW: usize = 4096;
    const HEAD_MAX: u32 = 64 * 1024; // ceiling: an unterminated giant line still bounds the read
    let (fc, size) = (de.first_cluster(), de.size);
    let (mut off, mut lines) = (0u32, 0u32);
    let mut cur = String::new(); // the line under construction (rendered, per `render_text`'s rules)
    let mut buf: Vec<u8> = Vec::new();
    let mut more = false; // does content remain AFTER the nth line? — drives the truncation note
    'outer: while off < size && off < HEAD_MAX && lines < n {
        buf.clear();
        if let Err(e) = fs.read_at(fc, size, off, &mut buf, WINDOW) {
            return console.println(&alloc::format!("head: {}: {} ({:?})", canon, fat_errno(e), e));
        }
        if buf.is_empty() {
            break; // chain ended before de.size (malformed) — show what we have, honestly
        }
        for (i, &b) in buf.iter().enumerate() {
            match b {
                b'\n' => {
                    console.println(&cur);
                    cur.clear();
                    lines += 1;
                    if lines >= n {
                        // More remains iff any byte follows this newline — in this window, or the
                        // file continues past it. (`off` still holds this window's start here.)
                        more = i + 1 < buf.len() || off + (buf.len() as u32) < size;
                        break 'outer;
                    }
                }
                b'\r' => {}
                0x20..=0x7e => cur.push(b as char),
                _ => cur.push('.'),
            }
        }
        off += buf.len() as u32;
    }
    // A final line with no trailing newline (we stopped before `n` full lines): print it.
    if lines < n && !cur.is_empty() {
        console.println(&cur);
        lines += 1;
    }
    // Note when more lines exist than shown: content followed the nth line (`more`), or the byte
    // ceiling cut a still-growing file before we reached `n` lines.
    if more || (lines < n && off < size) {
        console.println(&alloc::format!("[... first {} line(s) shown]", lines));
    }
}

/// JD12 `tail <path> [n]`: print the LAST `n` lines of a file (default 10). Reads a bounded window at
/// the END of the file (`TAIL_MAX` bytes ending at EOF) via the offset-aware `read_at`, renders it,
/// and prints the last `n` lines. If the window began mid-file, its first (cut) line is dropped and a
/// note records the bound. A directory or the root is `-EISDIR`; an empty file prints nothing. Every
/// access rides the JD3 wall-clock BOT pump — a stalled transfer is `-EIO`, never a hang.
fn fs_tail(console: &mut Console, arg: &str, n: u32) {
    let Some(fs) = mount_read_volume(console, "tail") else { return };
    let (de, canon) = match resolve_path(&fs, &normalize_path(&cwd_path(), arg)) {
        Ok(Resolved::Root) => return console.println("tail: /: is a directory (-EISDIR)"),
        Ok(Resolved::Entry(de, canon)) => {
            if de.is_dir {
                return console.println(&alloc::format!("tail: {}: is a directory (-EISDIR)", canon));
            }
            (de, canon)
        }
        Err(msg) => return console.println(&alloc::format!("tail: {}", msg)),
    };
    const TAIL_MAX: u32 = 64 * 1024; // bounded tail window: only the last TAIL_MAX bytes are scanned
    let (fc, size) = (de.first_cluster(), de.size);
    if size == 0 {
        return; // empty file — tail shows nothing
    }
    let start = size.saturating_sub(TAIL_MAX);
    let mut buf: Vec<u8> = Vec::new();
    if let Err(e) = fs.read_at(fc, size, start, &mut buf, (size - start) as usize) {
        return console.println(&alloc::format!("tail: {}: {} ({:?})", canon, fat_errno(e), e));
    }
    let text = render_text(&buf);
    if text.is_empty() {
        return;
    }
    let mut lines: Vec<&str> = text.split('\n').collect();
    // A file ending in '\n' yields a trailing "" element — it is not a real last line.
    if text.ends_with('\n') {
        lines.pop();
    }
    // A window that began mid-file usually cuts its first line — but not if it happens to start on a
    // line boundary. Decide precisely: the first line is a partial iff the byte just before `start`
    // is not a newline (one extra byte read; only when windowed, so essentially never in practice).
    let windowed = start > 0;
    if windowed {
        let mut probe: Vec<u8> = Vec::new();
        let cut = fs.read_at(fc, size, start - 1, &mut probe, 1).is_err()
            || probe.first() != Some(&b'\n');
        if cut && !lines.is_empty() {
            lines.remove(0);
        }
    }
    let from = lines.len().saturating_sub(n as usize);
    if windowed {
        console.println(&alloc::format!(
            "[... tail of {} bytes; last {} line(s)]", size, lines.len() - from));
    }
    for line in &lines[from..] {
        console.println(line);
    }
}

/// JD17: parse the `setdate` argument pair — `YYYY-MM-DD HH:MM[:SS]`. Strict shapes (dash- and
/// colon-separated decimal fields, seconds optional and defaulting to 0); range validation is
/// `clock::set`'s (`WallTime::is_valid`), so this only has to produce the numbers honestly.
fn parse_setdate(args: &[&str]) -> Option<crate::clock::WallTime> {
    if args.len() != 2 {
        return None;
    }
    let num = |s: &str| -> Option<u32> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse::<u32>().ok()
    };
    let mut d = args[0].split('-');
    let (year, month, day) = (num(d.next()?)?, num(d.next()?)?, num(d.next()?)?);
    if d.next().is_some() {
        return None;
    }
    let mut t = args[1].split(':');
    let (hour, min) = (num(t.next()?)?, num(t.next()?)?);
    let sec = match t.next() {
        Some(s) => num(s)?,
        None => 0,
    };
    if t.next().is_some() {
        return None;
    }
    Some(crate::clock::WallTime { year, month, day, hour, min, sec })
}

/// JD16: format one entry's FAT last-write timestamp as a fixed-width `YYYY-MM-DD HH:MM:SS` field for
/// the `ls -l` long listing. A zeroed on-disk stamp (a host tool that left it 0, or a kernel-written
/// entry — the kernel has no RTC to stamp with; see §JD16) is shown honestly as a dashed placeholder of
/// the same width rather than a bogus 1980 date. Precision is 2 seconds, no timezone (FAT stores local
/// wall-clock; there is no offset to correct by).
fn fmt_mtime(de: &DirEntry) -> String {
    let ts = de.mtime();
    if ts.is_zero() {
        // 19 chars, same width as "YYYY-MM-DD HH:MM:SS"
        return String::from("       -           ");
    }
    alloc::format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        ts.year, ts.month, ts.day, ts.hour, ts.min, ts.sec
    )
}

/// Print one directory's entries in the `ls` table format, with the file/dir tally. `long` selects the
/// JD16 `-l` long format (size + FAT last-write timestamp + name), otherwise the classic short table.
/// PI-SHELL-LS: FAT-only (the x86 storage path); the Pi routes `ls` to unafs via `pi_ls`.
#[cfg(not(target_arch = "aarch64"))]
fn print_dir_listing(console: &mut Console, entries: &[DirEntry], long: bool) {
    let (mut files, mut dirs) = (0u32, 0u32);
    for de in entries {
        if de.is_dir {
            dirs += 1;
            if long {
                console.println(&alloc::format!(
                    "  <DIR>        {}  {}/", fmt_mtime(de), de.name()));
            } else {
                console.println(&alloc::format!("  <DIR>         {}", de.name()));
            }
        } else {
            files += 1;
            if long {
                console.println(&alloc::format!(
                    "  {:>10}  {}  {}", de.size, fmt_mtime(de), de.name()));
            } else {
                console.println(&alloc::format!("  {:>10}  {}", de.size, de.name()));
            }
        }
    }
    console.println(&alloc::format!("{} file(s), {} dir(s)", files, dirs));
}

// ---------------------------------------------------------------------------------------------
// PI-SHELL-LS — `ls` on the Pi shell lists the NATIVE unafs volume (the SD-card partition), not FAT.
//
// The shared `ls`/`dir` arm rides the FAT program source (FATVERB: `mount_read_volume`; before that
// the default-handle `fat::mount`) — the x86 USB-storage backend, or the internal SD card when that is what booted us.
// The Pi has no FAT
// volume mounted (its native store is unafs; FAT on the SD card is only the firmware boot partition),
// so on the board `ls` printed "ls: no FAT filesystem (...)". The unafs volume DOES work — it is the
// very volume PI-NET-15 serves at `/fs/` (what Safari sees, K3HELLO.TXT et al.) via the same
// `with_unafs` + `resolve_path`/`read_inode`/`ls` calls used here. So on aarch64 we route `ls` to
// unafs and it lists exactly what `/fs/` shows. (x86 keeps the FAT path, unchanged.)
// ---------------------------------------------------------------------------------------------

/// PI-SHELL-LS: list `path` off the native unafs volume under ONE `with_unafs` hold, returning
/// `(is_dir, rows)` where each row is `(name, size, is_dir)` sorted by name (`.`/`..`/System entries
/// filtered — the same subset `/fs/` shows). A directory yields its entries; a plain file yields its
/// own single row (the DOS `ls <file>` idiom). Any resolve/mount failure surfaces as an errno-tagged
/// message string. Mirrors `genet::fs_read_dir` so the shell and `/fs/` never disagree.
#[cfg(target_arch = "aarch64")]
#[allow(clippy::type_complexity)]
fn pi_ls_collect(path: &str) -> Result<(bool, Vec<(String, u64, bool)>), String> {
    let listed = crate::fs::unafs::with_unafs(|fs| {
        let id = fs
            .resolve_path(path)
            .map_err(|e| alloc::format!("{}: not found ({:?}, -ENOENT)", path, e))?;
        let inode = fs
            .read_inode(id)
            .map_err(|e| alloc::format!("{}: stat failed ({:?}, -EIO)", path, e))?;
        if inode.kind == ::unafs::FileKind::Directory {
            let entries = fs
                .ls(id)
                .map_err(|e| alloc::format!("{}: read failed ({:?}, -EIO)", path, e))?;
            let mut rows: Vec<(String, u64, bool)> = Vec::new();
            for de in &entries {
                if de.name == "." || de.name == ".." || de.kind == ::unafs::FileKind::System {
                    continue;
                }
                let sz = fs.read_inode(de.inode_id).map(|i| i.size).unwrap_or(0);
                rows.push((de.name.clone(), sz, de.kind == ::unafs::FileKind::Directory));
            }
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            Ok::<_, String>((true, rows))
        } else {
            let leaf = String::from(path.rsplit('/').next().unwrap_or(path));
            Ok((false, alloc::vec![(leaf, inode.size, false)]))
        }
    });
    match listed {
        Ok(inner) => inner,
        Err(e) => Err(alloc::format!("no unafs volume ({:?})", e)),
    }
}

/// PI-SHELL-LS: the Pi `ls`/`dir` core. Resolves `arg` against the cwd, prints the unafs listing in the
/// same table shape as the FAT path (size + name; a dir shows `<DIR>`), then a per-invocation
/// `:: ls1: <path>: <names> (N file, M dir) ::` serial witness — the verb renders panel-only on the
/// bench, so the witness gives a headless capture the same content (the PI-UI-3 `ui3_say` idiom).
/// PI-FS-4: unafs inodes carry a size but no last-write time, so `ls -l` shows the size plus a dashed
/// date column (`UNAFS_NO_MTIME`), and the short `ls` is unchanged. The `-l` serial mirror keeps the
/// `:: ls1:` shape and appends the per-entry sizes so a headless capture can witness the long form.
/// PI-FS-5: format a FAT last-write stamp as the fixed-width `YYYY-MM-DD HH:MM:SS` field the FAT `ls -l`
/// column uses (mirrors genet's `fmt_fat_mtime` so the shell and `/fs/usb/` never disagree). An all-zero
/// on-disk stamp renders as the dashed placeholder — the same 19-char width — rather than a bogus 1980 date.
#[cfg(target_arch = "aarch64")]
fn fat_mtime_field(ts: &crate::fs::fat::FatTimestamp) -> String {
    if ts.is_zero() {
        return String::from("       -           ");
    }
    alloc::format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        ts.year, ts.month, ts.day, ts.hour, ts.min, ts.sec
    )
}

/// PI-FS-5: collect a listing off the LIVE USB FAT mount at `sub` (relative to the USB root — `""` or `"/"`
/// is the root; `"/DIR"` / `"/DIR/SUB"` descend). Mounts read-only through the same `fat::mount_source(Usb)`
/// API genet's `/fs/usb` route uses, walks each path component by its display name (LFN-aware, PI-FS-3), and
/// returns `(is_dir, rows)` where each row is `(name, size, is_dir, mtime_field)`. `.`/`..` are filtered.
/// A file leaf yields its own single row (the DOS `ls <file>` idiom). Any mount/resolve failure is an
/// errno-tagged message string, matching the unafs path's shape.
#[cfg(target_arch = "aarch64")]
#[allow(clippy::type_complexity)]
fn pi_usb_ls_collect(sub: &str) -> Result<(bool, Vec<(String, u64, bool, String)>), String> {
    let fs = crate::fs::fat::mount_source(crate::fs::fat::BlockSource::Usb)
        .map_err(|e| alloc::format!("no USB FAT mount ({}, -ENODEV)", crate::fs::fat::fat_reason(e)))?;
    let comps: Vec<&str> = sub.split('/').filter(|c| !c.is_empty()).collect();
    let mut entries = fs
        .read_root()
        .map_err(|e| alloc::format!("/usb: read failed ({}, -EIO)", crate::fs::fat::fat_reason(e)))?;
    for (i, comp) in comps.iter().enumerate() {
        let here = alloc::format!("/usb/{}", comps[..=i].join("/"));
        let de = entries
            .iter()
            .find(|d| d.name().eq_ignore_ascii_case(comp))
            .ok_or_else(|| alloc::format!("{}: not found (-ENOENT)", here))?;
        if de.is_dir {
            entries = fs
                .read_dir(de.first_cluster())
                .map_err(|e| alloc::format!("{}: read failed ({}, -EIO)", here, crate::fs::fat::fat_reason(e)))?;
        } else if i == comps.len() - 1 {
            // A file leaf named as the final component — list its own single row (DOS idiom).
            return Ok((false, alloc::vec![(String::from(de.name()), de.size as u64, false, fat_mtime_field(&de.mtime()))]));
        } else {
            return Err(alloc::format!("{}: not a directory (-ENOTDIR)", here));
        }
    }
    let mut rows: Vec<(String, u64, bool, String)> = Vec::new();
    for de in &entries {
        let name = de.name();
        if name == "." || name == ".." {
            continue;
        }
        rows.push((String::from(name), de.size as u64, de.is_dir, fat_mtime_field(&de.mtime())));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((true, rows))
}

/// PI-FS-5: the `/usb[...]` arm of the Pi `ls`. `full` is the normalized `/usb...` path (for the table
/// header and the `:: ls1: /usb...` witness); `sub` is the part past `/usb`. Prints the same table shape as
/// the unafs/FAT paths — size + name, `<DIR>` for directories — and, under `-l`, the FAT last-write date the
/// `/fs/usb/` HTTP listing shows (PI-FS-4). Emits the `:: ls1:` witness so a headless capture sees the rows.
#[cfg(target_arch = "aarch64")]
fn pi_usb_ls(console: &mut Console, full: &str, sub: &str, long: bool) {
    match pi_usb_ls_collect(sub) {
        Ok((is_dir, rows)) => {
            let (mut files, mut dirs) = (0u32, 0u32);
            for (name, size, row_is_dir, date) in &rows {
                if *row_is_dir {
                    dirs += 1;
                    if long {
                        console.println(&alloc::format!("  <DIR>        {}  {}/", date, name));
                    } else {
                        console.println(&alloc::format!("  <DIR>         {}", name));
                    }
                } else {
                    files += 1;
                    if long {
                        console.println(&alloc::format!("  {:>10}  {}  {}", size, date, name));
                    } else {
                        console.println(&alloc::format!("  {:>10}  {}", size, name));
                    }
                }
            }
            if is_dir {
                console.println(&alloc::format!("{} file(s), {} dir(s)", files, dirs));
            }
            let names: Vec<&str> = rows.iter().map(|(n, _, _, _)| n.as_str()).collect();
            if long {
                let sizes: Vec<String> = rows.iter().map(|(_, s, _, _)| alloc::format!("{}", s)).collect();
                serial_println!(
                    ":: ls1: {}: {} ({} file, {} dir) sizes: {} ::",
                    full, names.join(" "), files, dirs, sizes.join(" ")
                );
            } else {
                serial_println!(":: ls1: {}: {} ({} file, {} dir) ::", full, names.join(" "), files, dirs);
            }
        }
        Err(msg) => {
            console.println(&alloc::format!("ls: {}", msg));
            serial_println!(":: ls1: {}: ERR {} ::", full, msg);
        }
    }
}

#[cfg(target_arch = "aarch64")]
fn pi_ls(console: &mut Console, arg: &str, long: bool) {
    // 19-char dashed placeholder, the same width as the FAT `YYYY-MM-DD HH:MM:SS` field — unafs has no
    // last-write time to render, so `ls -l` shows a `-` here honestly rather than a fabricated date.
    const UNAFS_NO_MTIME: &str = "       -           ";
    let path = normalize_path(&cwd_path(), arg);
    // PI-FS-5: the `/usb` mount lives in the SAME path namespace the HTTP server exposes at `/fs/usb`.
    // `ls /usb` (and `/usb/<sub>`, LFN-aware) lists the live USB FAT volume via the genet mount API rather
    // than unafs. Everything else stays on the native unafs volume below.
    if path == "/usb" || path.starts_with("/usb/") {
        let sub = path.strip_prefix("/usb").unwrap_or("");
        pi_usb_ls(console, &path, sub, long);
        return;
    }
    match pi_ls_collect(&path) {
        Ok((is_dir, rows)) => {
            // PI-FS-5: at the unafs root, append a `usb/` pseudo-entry when the USB stick is mounted — mirroring
            // the `/fs/` HTTP listing's `usb/` link, so `ls /` advertises the drive the same way the browser does.
            let show_usb = path == "/" && crate::drivers::block::usb_info().is_some();
            let (mut files, mut dirs) = (0u32, 0u32);
            for (name, size, row_is_dir) in &rows {
                if *row_is_dir {
                    dirs += 1;
                    if long {
                        console.println(&alloc::format!("  <DIR>        {}  {}/", UNAFS_NO_MTIME, name));
                    } else {
                        console.println(&alloc::format!("  <DIR>         {}", name));
                    }
                } else {
                    files += 1;
                    if long {
                        console.println(&alloc::format!("  {:>10}  {}  {}", size, UNAFS_NO_MTIME, name));
                    } else {
                        console.println(&alloc::format!("  {:>10}  {}", size, name));
                    }
                }
            }
            // PI-FS-5: the mounted-USB pseudo-entry — a `usb/` dir row at the unafs root, counted as a dir.
            if show_usb {
                dirs += 1;
                if long {
                    console.println(&alloc::format!("  <DIR>        {}  usb/", UNAFS_NO_MTIME));
                } else {
                    console.println("  <DIR>         usb");
                }
            }
            if is_dir {
                console.println(&alloc::format!("{} file(s), {} dir(s)", files, dirs));
            }
            let mut names: Vec<&str> = rows.iter().map(|(n, _, _)| n.as_str()).collect();
            if show_usb {
                names.push("usb");
            }
            if long {
                let sizes: Vec<String> = rows.iter().map(|(_, s, _)| alloc::format!("{}", s)).collect();
                serial_println!(
                    ":: ls1: {}: {} ({} file, {} dir) sizes: {} ::",
                    path, names.join(" "), files, dirs, sizes.join(" ")
                );
            } else {
                serial_println!(
                    ":: ls1: {}: {} ({} file, {} dir) ::",
                    path, names.join(" "), files, dirs
                );
            }
        }
        Err(msg) => {
            console.println(&alloc::format!("ls: {}", msg));
            serial_println!(":: ls1: {}: ERR {} ::", path, msg);
        }
    }
}

/// PI-SHELL-LS boot witness (`witness` battery only): exercise the exact `pi_ls_collect` listing the
/// shell verb uses, against the unafs root, and emit the `:: ls1: ... ::` line headlessly — so
/// `UNAOS_PI=1 ./arroyo kernel8-test` proves `ls` works without a serial-console injection path. Quiet
/// default boots never compile this. Baremetal-gated to match the emmc2 backend the volume rides.
#[cfg(all(target_arch = "aarch64", feature = "baremetal", feature = "witness"))]
pub fn pi_ls_witness() {
    match pi_ls_collect("/") {
        Ok((_, rows)) => {
            let names: Vec<&str> = rows.iter().map(|(n, _, _)| n.as_str()).collect();
            let dirs = rows.iter().filter(|(_, _, d)| *d).count();
            let files = rows.len() - dirs;
            serial_println!(
                ":: ls1: /: {} ({} file, {} dir) ::",
                names.join(" "), files, dirs
            );
        }
        Err(msg) => serial_println!(":: ls1: /: ERR {} ::", msg),
    }
}

/// PI-FS-5 boot/hot-plug witness: exercise the EXACT `pi_usb_ls_collect` listing the shell's `ls /usb`
/// verb uses, against the live USB FAT mount, and emit the `:: ls1: /usb... ::` line headlessly — so a
/// capture proves the shell sees the same volume `/fs/usb` serves, without a serial-console injection
/// path. Called from `fat::piusb27_mount_witness` (which fires once per bring-up + every hot-plug), so it
/// rides the same USB feature gate as the mount witness — NOT the baremetal/witness battery (the USB FAT
/// volume is present in `UNAOS_FATIMG=1 ./arroyo test-arm`, where the emmc2-backed unafs volume is not).
/// Lists the `/usb` root then descends one named subdir to prove the LFN-aware subpath walk.
#[cfg(target_arch = "aarch64")]
pub fn pi_usb_ls_witness() {
    for (full, sub) in [("/usb", ""), ("/usb/SUBDIR", "/SUBDIR")] {
        match pi_usb_ls_collect(sub) {
            Ok((_, rows)) => {
                let names: Vec<String> = rows
                    .iter()
                    .map(|(n, _, d, _)| if *d { alloc::format!("{}/", n) } else { n.clone() })
                    .collect();
                let dirs = rows.iter().filter(|(_, _, d, _)| *d).count();
                let files = rows.len() - dirs;
                serial_println!(
                    ":: ls1: {}: {} ({} file, {} dir) ::",
                    full, names.join(" "), files, dirs
                );
            }
            Err(msg) => serial_println!(":: ls1: {}: ERR {} ::", full, msg),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// JD12 — wildcard globbing (`ls *.C`, `cat *.MD`, `rm *.TXT`, `cp *.TXT DIR/`, `mv *.LOG ARCH/`).
//
// A single TRAILING glob in a path's LAST component is expanded against the parent directory via the
// read-only `read_dir` (case-insensitive 8.3 matching, already proven for `cd`/`cat`). Expansion is
// invoked ONLY inside the fs-verb arms below — the shared arg-split at the top of `dispatch_command`
// is unchanged, and the NET command region (netinfo/ping/arp/connect/udpsend/get — a sockets-arc
// lane) never sees a glob. A verb loops over the matches (SNAPSHOT-then-act: the match list is
// captured before any mutation, so a `rm *.TXT` that deletes as it goes never invalidates its own
// list). A glob with no match is an honest per-pattern "no match" note; a name with no metacharacter
// passes through literally (byte-identical to pre-JD12). Only the leaf is a pattern — a metacharacter
// in an earlier component resolves literally (an honest `-ENOENT`), never a mid-path wildcard walk.

/// True if `s` carries a glob metacharacter (`*` = any run, `?` = exactly one char).
fn has_glob(s: &str) -> bool {
    s.bytes().any(|b| b == b'*' || b == b'?')
}

/// Case-insensitive 8.3 wildcard match: `*` matches any run (including empty), `?` exactly one
/// character; every other byte is literal. Iterative with star-backtrack — no recursion, no
/// allocation. `name` is a canonical on-disk 8.3 name (e.g. `README.TXT`), `pat` the leaf pattern.
fn glob_match(pat: &str, name: &str) -> bool {
    let p = pat.as_bytes();
    let n = name.as_bytes();
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut resume) = (usize::MAX, 0usize); // last '*' seen in `p`, and where to resume `n`
    while ni < n.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi].eq_ignore_ascii_case(&n[ni])) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = pi;
            resume = ni;
            pi += 1; // let '*' match zero chars first; a later mismatch backtracks and extends it
        } else if star != usize::MAX {
            pi = star + 1;
            resume += 1;
            ni = resume; // '*' swallows one more char of `n`, then retry the rest of the pattern
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1; // any trailing '*'s match the (now empty) tail of `n`
    }
    pi == p.len()
}

/// The result of expanding ONE path argument. `Literal` = no metacharacter in the leaf — the arg
/// passed through unchanged (byte-identical to pre-JD12; also covers a glob confined to a NON-trailing
/// component, which resolves literally). `Matched` = a trailing-glob leaf resolved against
/// `parent_canon`'s directory to zero or more entries; an empty `entries` (including a parent that
/// does not resolve to a directory) is an honest "no match".
enum Glob {
    Literal(String),
    Matched { parent_canon: String, entries: Vec<DirEntry> },
}

/// Expand a shell path arg for the fs-verb glob (JD12). A metacharacter is honored ONLY in the LAST
/// path component; a glob in an earlier component is treated literally (its parent resolve fails ⇒ no
/// match, an honest error at the verb). Case-insensitive; `.`/`..` filtered; matches sorted so a
/// listing / serial transcript is deterministic.
fn glob_expand(fs: &FatFs, arg: &str) -> Glob {
    let norm = normalize_path(&cwd_path(), arg);
    let comps: Vec<&str> = norm.split('/').filter(|c| !c.is_empty()).collect();
    let leaf = match comps.last() {
        Some(l) => *l,
        None => return Glob::Literal(norm), // arg normalized to "/" — nothing to glob
    };
    if !has_glob(leaf) {
        return Glob::Literal(String::from(arg)); // pass the ORIGINAL typed arg through unchanged
    }
    // Resolve the PARENT (everything but the leaf) to a directory cluster + its canonical path.
    let (parent_cluster, parent_canon) = if comps.len() == 1 {
        (0u32, String::new()) // the volume root
    } else {
        let mut parent_path = String::new();
        for c in &comps[..comps.len() - 1] {
            parent_path.push('/');
            parent_path.push_str(c);
        }
        match resolve_path(fs, &parent_path) {
            Ok(Resolved::Root) => (0u32, String::new()),
            Ok(Resolved::Entry(de, canon)) if de.is_dir => (de.first_cluster(), canon),
            _ => return Glob::Matched { parent_canon: parent_path, entries: Vec::new() }, // no dir → no match
        }
    };
    let mut entries: Vec<DirEntry> = match fs.read_dir(parent_cluster) {
        Ok(es) => es
            .into_iter()
            .filter(|de| {
                let nm = de.name();
                nm != "." && nm != ".." && glob_match(leaf, nm)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    entries.sort_by(|a, b| a.name().cmp(b.name()));
    Glob::Matched { parent_canon, entries }
}

/// Resolve `path` and print it in the `ls` table format (the single-path `ls` core, shared by the
/// `ls` arm and the wildcard `ls *.EXT` Literal fall-through). A directory lists its entries; a plain
/// file prints its one table line (the DOS idiom); errors are errno-tagged.
/// PI-SHELL-LS: FAT-only (x86); the Pi lists unafs via `pi_ls`.
#[cfg(not(target_arch = "aarch64"))]
fn ls_resolved(console: &mut Console, fs: &FatFs, path: &str, long: bool) {
    match resolve_path(fs, path) {
        Ok(Resolved::Root) => match fs.read_dir(0) {
            Ok(entries) => print_dir_listing(console, &entries, long),
            Err(e) => console.println(&alloc::format!("ls: /: read failed ({:?}, -EIO)", e)),
        },
        Ok(Resolved::Entry(de, canon)) => {
            if de.is_dir {
                match fs.read_dir(de.first_cluster()) {
                    Ok(entries) => print_dir_listing(console, &entries, long),
                    Err(e) => console.println(&alloc::format!(
                        "ls: {}: read failed ({:?}, -EIO)", canon, e)),
                }
            } else if long {
                console.println(&alloc::format!(
                    "  {:>10}  {}  {}", de.size, fmt_mtime(&de), de.name()));
            } else {
                console.println(&alloc::format!("  {:>10}  {}", de.size, de.name()));
            }
        }
        Err(msg) => console.println(&alloc::format!("ls: {}", msg)),
    }
}

/// JD4 `ls`/`ls <dir>` (single path): mount + resolve + print. Extracted so the wildcard `ls *.EXT`
/// can share the exact resolve/print behaviour for a non-trailing-glob fall-through.
#[cfg(not(target_arch = "aarch64"))]
fn ls_path(console: &mut Console, arg: &str, long: bool) {
    if let Some(fs) = mount_read_volume(console, "ls") {
        ls_resolved(console, &fs, &normalize_path(&cwd_path(), arg), long);
    }
}

/// JD12 `ls *.EXT`: list every entry matching a wildcard, one `ls`-table line each (sorted), with the
/// file/dir tally. A directory match shows as `<DIR>` (its contents are not expanded — that mirrors
/// how a shell hands matched names to `ls`); no match is an honest "no match".
#[cfg(not(target_arch = "aarch64"))]
fn ls_globbed(console: &mut Console, arg: &str, long: bool) {
    let Some(fs) = mount_read_volume(console, "ls") else { return };
    match glob_expand(&fs, arg) {
        Glob::Literal(p) => ls_resolved(console, &fs, &normalize_path(&cwd_path(), &p), long),
        Glob::Matched { entries, .. } if entries.is_empty() =>
            console.println(&alloc::format!("ls: {}: no match", arg)),
        Glob::Matched { entries, .. } => print_dir_listing(console, &entries, long),
    }
}

/// JD12 `cat *.EXT`: cat every FILE matching a wildcard (concatenate), in sorted order — reusing
/// `cat_render` per file so the rendering + truncation note are identical to a single-path `cat`. A
/// directory match is skipped with the classic `-EISDIR` note; no match is an honest "no match". A
/// glob confined to a non-trailing component falls through to a literal resolve (honest error).
fn cat_globbed(console: &mut Console, arg: &str) {
    let Some(fs) = mount_read_volume(console, "cat") else { return };
    match glob_expand(&fs, arg) {
        Glob::Literal(p) => match resolve_path(&fs, &normalize_path(&cwd_path(), &p)) {
            Ok(Resolved::Root) => console.println("cat: /: is a directory (-EISDIR)"),
            Ok(Resolved::Entry(de, canon)) => cat_render(console, &fs, &de, &canon),
            Err(msg) => console.println(&alloc::format!("cat: {}", msg)),
        },
        Glob::Matched { entries, .. } if entries.is_empty() =>
            console.println(&alloc::format!("cat: {}: no match", arg)),
        Glob::Matched { parent_canon, entries } => {
            for de in &entries {
                let canon = joined(&parent_canon, de.name());
                if de.is_dir {
                    console.println(&alloc::format!("cat: {}: is a directory (-EISDIR)", canon));
                } else {
                    cat_render(console, &fs, de, &canon);
                }
            }
        }
    }
}

/// JD12: does `dst` (a shell path arg) resolve to an existing directory (or the root)? A multi-source
/// `cp`/`mv` requires it — several sources can only land INTO a directory, not onto one target name.
fn dst_is_dir(fs: &FatFs, dst: &str) -> bool {
    match resolve_path(fs, &normalize_path(&cwd_path(), dst)) {
        Ok(Resolved::Root) => true,
        Ok(Resolved::Entry(de, _)) => de.is_dir,
        Err(_) => false,
    }
}

/// JD12: expand the SOURCE args of a `cp`/`mv` into concrete source paths, printing a per-pattern "no
/// match" note (tagged with `verb`) for any wildcard that matched nothing. Literal args pass through
/// unchanged. Returns the flattened, ordered list (SNAPSHOT: taken before any mutation runs).
fn expand_sources(console: &mut Console, fs: &FatFs, verb: &str, sources: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in sources {
        match glob_expand(fs, s) {
            Glob::Literal(p) => out.push(p),
            Glob::Matched { entries, .. } if entries.is_empty() =>
                console.println(&alloc::format!("{}: {}: no match", verb, s)),
            Glob::Matched { parent_canon, entries } => {
                for de in &entries {
                    out.push(joined(&parent_canon, de.name()));
                }
            }
        }
    }
    out
}

/// JD12 `rm` with wildcards: `rm [-r] <path...>` — each arg a file/dir target or a trailing glob.
/// SNAPSHOT-then-delete (`glob_expand` captures the match list before any delete), so a wildcard
/// delete never invalidates its own list. A wildcard with no match is an honest per-pattern note;
/// each concrete target rides the existing per-target handler — `fs_rm` (file-only; a directory is
/// `-EISDIR`, use `rmdir`) or, when `recursive` (JD13 `rm -r *`), `fs_rm_recursive` (a directory tree,
/// a file degrades to a plain delete). SNAPSHOT-safety holds through the recursion too: each concrete
/// match is re-resolved by its canonical path, and a completed `rm -r` never touches a sibling's slot.
fn rm_globbed(console: &mut Console, args: &[&str], recursive: bool, force: bool) {
    let Some(fs) = mount_write_volume(console, "rm") else { return };
    for a in args {
        match glob_expand(&fs, a) {
            Glob::Literal(p) =>
                if recursive { fs_rm_recursive(console, &p, force) } else { fs_rm(console, &p, force) },
            // JD14: a no-match wildcard is quiet under `-f` (POSIX `rm -f *.none` is silent).
            Glob::Matched { entries, .. } if entries.is_empty() =>
                { if !force { console.println(&alloc::format!("rm: {}: no match", a)); } }
            Glob::Matched { parent_canon, entries } => {
                for de in &entries {
                    let path = joined(&parent_canon, de.name());
                    if recursive { fs_rm_recursive(console, &path, force) } else { fs_rm(console, &path, force) }
                }
            }
        }
    }
}

/// JD12 `cp` with wildcards / multiple sources: `cp [-r] <src...> <dst>`. Sources expand (globs +
/// literals); with more than one source the destination MUST be an existing directory (several files
/// can only land INTO a directory). Each source rides the existing `fs_cp` / `fs_cp_recursive` (the
/// `FILE DIR/` idiom lands each under `dst/<leaf>`). SNAPSHOT-then-copy.
fn cp_globbed(console: &mut Console, sources: &[&str], dst: &str, recursive: bool, force: bool) {
    let Some(fs) = mount_write_volume(console, "cp") else { return };
    let srcs = expand_sources(console, &fs, "cp", sources);
    if srcs.is_empty() {
        return; // every pattern was empty (each already reported "no match")
    }
    if srcs.len() > 1 && !dst_is_dir(&fs, dst) {
        return console.println(&alloc::format!("cp: target {}: not a directory (-ENOTDIR)", dst));
    }
    for s in &srcs {
        if recursive {
            fs_cp_recursive(console, s, dst, force);
        } else {
            fs_cp(console, s, dst, force);
        }
    }
}

/// JD12 `mv` with wildcards / multiple sources: `mv <src...> <dst>`. Sources expand; with more than
/// one source the destination MUST be an existing directory. Each source rides the existing `fs_mv`
/// (the `SRC DIR/` idiom lands each under `dst/<leaf>`). SNAPSHOT-then-move — a wildcard move never
/// invalidates its own list.
fn mv_globbed(console: &mut Console, sources: &[&str], dst: &str, force: bool) {
    let Some(fs) = mount_write_volume(console, "mv") else { return };
    let srcs = expand_sources(console, &fs, "mv", sources);
    if srcs.is_empty() {
        return;
    }
    if srcs.len() > 1 && !dst_is_dir(&fs, dst) {
        return console.println(&alloc::format!("mv: target {}: not a directory (-ENOTDIR)", dst));
    }
    for s in &srcs {
        fs_mv(console, s, dst, force);
    }
}

/// JD14: split a `cp`/`mv`/`rm` argument list into `(recursive, force, no_clobber, positional paths)`.
/// A FLAG arg is `-` followed by one or more ASCII letters — bundled short flags, so `-rf` == `-r -f`;
/// `r`/`R` set recursive, `f` sets force (`-f`), `n` sets no-clobber (`-n`), any other letter is an
/// ignored unknown flag. Everything else is a positional path (this also fixes the pre-JD14 exact-token
/// `-r` match, which never recognized a bundled `rm -rf DIR`). Consistent with the established
/// convention that a leading-`-` arg is a flag — a file literally named `-x` is still reachable as
/// `./-x` (its letters parse as unknown/ignored flags, exactly the pre-JD14 behaviour). `-` alone, or
/// an arg with a non-letter after the dash (e.g. `-2`), is treated as a path.
fn split_flags<'a>(args: &[&'a str]) -> (bool, bool, bool, Vec<&'a str>) {
    let (mut recursive, mut force, mut no_clobber) = (false, false, false);
    let mut paths: Vec<&'a str> = Vec::new();
    for &a in args {
        if a.len() > 1 && a.starts_with('-') && a[1..].bytes().all(|b| b.is_ascii_alphabetic()) {
            for c in a[1..].chars() {
                match c {
                    'r' | 'R' => recursive = true,
                    'f' => force = true,
                    'n' => no_clobber = true,
                    _ => {} // unknown flag — ignored (keeps a file named `-x` reachable as `./-x`)
                }
            }
        } else {
            paths.push(a);
        }
    }
    (recursive, force, no_clobber, paths)
}

// ---------------------------------------------------------------------------------------------
// JD18 — read-only TREE TOOLS: `find` (recursive glob search), `du` (subtree size tally),
// `uptime` (seconds since boot from the arch counter). All THREE are pure reads — ZERO mutation,
// no fat.rs edit (call-never-edit): they compose the same `read_dir` SNAPSHOT walk as JD9 `cp_tree`
// and JD13 `rm_tree`, the JD12 `glob_match`, and (for `uptime`) the JD17 clock's additive
// `uptime_secs()` helper. `.`/`..` are filtered at every level and recursion is bounded by the
// shared `CP_MAX_DEPTH` (honest `-ELOOP`); a mid-walk read error stops with an honest path + errno
// and the partial results already printed (nothing invented). FAT directory ENTRIES report size 0,
// so only file sizes contribute real bytes to a `du` tally — a directory's size is the sum of its
// files, recursively.

/// JD18 running tally for `find`: hits printed, and directories scanned (each `read_dir` level,
/// the root included) — the honest denominator for the closing summary.
struct FindStats {
    matches: u32,
    dirs: u32,
}

/// JD18: recursively walk the directory (cluster `dir_cluster`, canonical `dir_canon`), matching each
/// entry's 8.3 name against `pat` with the JD12 `glob_match` (case-insensitive; a literal pattern is an
/// exact-name match). A hit prints its full canonical path — a directory with a trailing `/`. `.`/`..`
/// are skipped; every subdirectory is recursed into (whether or not its own name matched). SNAPSHOT
/// per level (`read_dir` before any descent — a pure read never mutates, but the idiom stays uniform
/// with the JD9/JD13 walkers). Depth-capped at `CP_MAX_DEPTH` (honest `-ELOOP`); a read error stops
/// with a formatted `path: reason (-EIO)` and leaves the already-printed hits standing.
fn find_walk(
    console: &mut Console,
    fs: &FatFs,
    dir_cluster: u32,
    dir_canon: &str,
    pat: &str,
    depth: u32,
    stats: &mut FindStats,
) -> Result<(), String> {
    if depth > CP_MAX_DEPTH {
        return Err(alloc::format!(
            "{}: maximum directory depth {} exceeded (-ELOOP)", dir_canon, CP_MAX_DEPTH));
    }
    stats.dirs += 1; // this directory level is being scanned
    let entries = fs
        .read_dir(dir_cluster)
        .map_err(|e| alloc::format!("{}: read failed ({:?}, -EIO)", dir_canon, e))?;
    for de in &entries {
        let nm = de.name();
        if nm == "." || nm == ".." {
            continue;
        }
        let child = joined(dir_canon, nm);
        if glob_match(pat, nm) {
            stats.matches += 1;
            if de.is_dir {
                console.println(&alloc::format!("{}/", child));
            } else {
                console.println(&child);
            }
        }
        if de.is_dir {
            find_walk(console, fs, de.first_cluster(), &child, pat, depth + 1, stats)?;
        }
    }
    Ok(())
}

/// JD18 `find <root> <pattern>`: recursively search the tree under `<root>` (a directory path;
/// default `.` when only a pattern is given) for entries whose 8.3 name matches `<pattern>` (the JD12
/// glob engine — `*`/`?`, case-insensitive; a literal is an exact match). Prints each hit as its full
/// canonical path, then an honest `N match(es), M dir(s) scanned` tally. A missing root is `-ENOENT`;
/// a FILE root degrades to a single self-match test (the POSIX shape — `find` a file tests that file);
/// a mid-walk I/O error reports the path + errno with the partial hits/count already shown.
fn fs_find(console: &mut Console, root_arg: &str, pat: &str) {
    let Some(fs) = mount_read_volume(console, "find") else { return };
    let norm = normalize_path(&cwd_path(), root_arg);
    let mut stats = FindStats { matches: 0, dirs: 0 };
    match resolve_path(&fs, &norm) {
        Ok(Resolved::Root) => {
            if let Err(msg) = find_walk(console, &fs, 0, "", pat, 1, &mut stats) {
                console.println(&alloc::format!("find: {}", msg));
            }
            console.println(&alloc::format!(
                "{} match(es), {} dir(s) scanned", stats.matches, stats.dirs));
        }
        Ok(Resolved::Entry(de, canon)) => {
            if de.is_dir {
                if let Err(msg) = find_walk(console, &fs, de.first_cluster(), &canon, pat, 1, &mut stats) {
                    console.println(&alloc::format!("find: {}", msg));
                }
                console.println(&alloc::format!(
                    "{} match(es), {} dir(s) scanned", stats.matches, stats.dirs));
            } else {
                // A file root: the POSIX self-match test — the root itself is the only candidate.
                if glob_match(pat, de.name()) {
                    console.println(&canon);
                    stats.matches += 1;
                }
                console.println(&alloc::format!(
                    "{} match(es), 0 dir(s) scanned", stats.matches));
            }
        }
        Err(msg) => console.println(&alloc::format!("find: {}", msg)),
    }
}

/// JD18 running tally for `du`: files and directories counted across the whole subtree.
struct DuStats {
    files: u32,
    dirs: u32,
}

/// JD18: total bytes of the subtree rooted at (cluster `dir_cluster`, canonical `dir_canon`) — the
/// sum of every descendant FILE's size (FAT directory entries report size 0, so directories add no
/// bytes of their own). Accumulates file/dir counts into `stats`. `.`/`..` filtered; depth-capped at
/// `CP_MAX_DEPTH` (honest `-ELOOP`); a read error stops with a formatted `path: reason (-EIO)`.
fn du_subtree(
    fs: &FatFs,
    dir_cluster: u32,
    dir_canon: &str,
    depth: u32,
    stats: &mut DuStats,
) -> Result<u64, String> {
    if depth > CP_MAX_DEPTH {
        return Err(alloc::format!(
            "{}: maximum directory depth {} exceeded (-ELOOP)", dir_canon, CP_MAX_DEPTH));
    }
    let entries = fs
        .read_dir(dir_cluster)
        .map_err(|e| alloc::format!("{}: read failed ({:?}, -EIO)", dir_canon, e))?;
    let mut total: u64 = 0;
    for de in &entries {
        let nm = de.name();
        if nm == "." || nm == ".." {
            continue;
        }
        if de.is_dir {
            stats.dirs += 1;
            total += du_subtree(fs, de.first_cluster(), &joined(dir_canon, nm), depth + 1, stats)?;
        } else {
            stats.files += 1;
            total += de.size as u64;
        }
    }
    Ok(total)
}

/// JD18 `du <dir>`: for each DIRECT child of `<dir>` print its total bytes (a file = its own size, a
/// directory = the recursive sum of its subtree), then a `total: N byte(s) in M file(s), K dir(s)`
/// line. `du FILE` is that file's single line. FAT directory entries themselves report size 0 — only
/// file bytes are real. A missing path is `-ENOENT`; a mid-walk read error reports the path + errno
/// with the partial per-child lines and a total of what was tallied (honest partial).
fn fs_du(console: &mut Console, arg: &str) {
    let Some(fs) = mount_read_volume(console, "du") else { return };
    let norm = normalize_path(&cwd_path(), arg);
    let (cluster, canon) = match resolve_path(&fs, &norm) {
        Ok(Resolved::Root) => (0u32, String::new()),
        Ok(Resolved::Entry(de, canon)) => {
            if de.is_dir {
                (de.first_cluster(), canon)
            } else {
                // A plain file: its one line, then a total of one file.
                console.println(&alloc::format!("  {:>10}  {}", de.size, canon));
                return console.println(&alloc::format!(
                    "total: {} byte(s) in 1 file(s), 0 dir(s)", de.size));
            }
        }
        Err(msg) => return console.println(&alloc::format!("du: {}", msg)),
    };
    let entries = match fs.read_dir(cluster) {
        Ok(es) => es,
        Err(e) => return console.println(&alloc::format!(
            "du: {}: read failed ({:?}, -EIO)", if canon.is_empty() { "/" } else { &canon }, e)),
    };
    let mut stats = DuStats { files: 0, dirs: 0 };
    let mut grand: u64 = 0;
    for de in &entries {
        let nm = de.name();
        if nm == "." || nm == ".." {
            continue;
        }
        let child = joined(&canon, nm);
        if de.is_dir {
            stats.dirs += 1;
            match du_subtree(&fs, de.first_cluster(), &child, 1, &mut stats) {
                Ok(sz) => {
                    grand += sz;
                    console.println(&alloc::format!("  {:>10}  {}/", sz, child));
                }
                Err(msg) => {
                    console.println(&alloc::format!("du: {}", msg));
                    break; // stop the walk; the total below is honest for what was scanned
                }
            }
        } else {
            stats.files += 1;
            grand += de.size as u64;
            console.println(&alloc::format!("  {:>10}  {}", de.size, child));
        }
    }
    console.println(&alloc::format!(
        "total: {} byte(s) in {} file(s), {} dir(s)", grand, stats.files, stats.dirs));
}

// ---------------------------------------------------------------------------------------------
// JD19 — read-only forensic verbs: `stat` (one entry's full on-disk detail) and `xd` (bounded
// hexdump). Both are `shell.rs`-only, ride the existing public fat.rs API call-never-edit, and never
// mutate: `stat` composes resolve_path/locate_in_dir plus one raw `block::read_block` of the on-disk
// directory sector for the true attr byte (the parsed DirEntry keeps only `is_dir`); `xd` streams a
// bounded window through the offset-aware `read_at`. Neither is glob-wired (single path) — a
// metacharacter resolves literally, an honest `-ENOENT`, the same as a mid-path glob today.

/// JD19: decode a FAT attribute byte into its flag names, space-joined (RO/HIDDEN/SYS/DIR/ARCHIVE),
/// or `-` when none are set. Bits per the FAT short-entry spec: 0x01 read-only, 0x02 hidden, 0x04
/// system, 0x10 directory, 0x20 archive (0x08 volume-label / 0x0F long-file-name components never
/// reach a parsed entry — `classify_dir_slot` skips them).
fn decode_attr(a: u8) -> String {
    let mut s = String::new();
    for (bit, name) in [
        (0x01u8, "RO"),
        (0x02, "HIDDEN"),
        (0x04, "SYS"),
        (0x10, "DIR"),
        (0x20, "ARCHIVE"),
    ] {
        if a & bit != 0 {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(name);
        }
    }
    if s.is_empty() {
        s.push('-');
    }
    s
}

/// JD19 `stat <path>`: one entry's full on-disk detail — the forensic view. Prints the canonical
/// absolute path, kind (file/dir), size in bytes, the raw attr byte (hex + decoded flags), first
/// cluster (hex; `0x0` honest for a 0-length file), the FAT last-write stamp (dash when zeroed, via
/// `fmt_mtime`), and the on-disk location (directory-entry LBA + 32-byte slot offset). `stat /`
/// reports the root honestly — it is a directory with NO directory entry of its own. Missing path is
/// `-ENOENT`. Read-only; no glob (a metacharacter resolves literally → `-ENOENT`).
fn fs_stat(console: &mut Console, arg: &str) {
    let Some(fs) = mount_read_volume(console, "stat") else { return };
    // The volume root has no directory entry of its own — report it honestly, no slot.
    if normalize_path(&cwd_path(), arg) == "/" {
        console.println("  path:  /");
        console.println("  kind:  dir");
        console.println("  size:  0 byte(s)");
        console.println("  entry: root has no directory entry");
        return;
    }
    // Walk to the parent, then locate the leaf's on-disk slot (for the attr byte + forensic LBA/offset).
    let (parent, leaf, parent_canon) = match resolve_write_target(&fs, arg) {
        Ok(t) => t,
        Err(msg) => return console.println(&alloc::format!("stat: {}", msg)),
    };
    let (de, dir_lba, dir_off) = match fs.locate_in_dir(parent, &leaf) {
        Ok(t) => t,
        Err(FatError::NotFound) => return console.println(&alloc::format!(
            "stat: {}: not found (-ENOENT)", joined(&parent_canon, &leaf))),
        Err(e) => return console.println(&alloc::format!(
            "stat: {}: {} ({:?})", joined(&parent_canon, &leaf), fat_errno(e), e)),
    };
    let canon = joined(&parent_canon, de.name());
    // The raw attr byte lives at slot offset +11; the parsed DirEntry keeps only `is_dir`, so read the
    // on-disk directory sector for the true byte via the same raw block path the `read` verb uses.
    let attr = {
        let mut buf = [0u8; 512];
        match crate::drivers::block::read_block(dir_lba, &mut buf) {
            Ok(_) if dir_off + 12 <= buf.len() => Some(buf[dir_off + 11]),
            _ => None,
        }
    };
    console.println(&alloc::format!("  path:  {}", canon));
    console.println(&alloc::format!("  kind:  {}", if de.is_dir { "dir" } else { "file" }));
    console.println(&alloc::format!("  size:  {} byte(s)", de.size));
    match attr {
        Some(a) => console.println(&alloc::format!("  attr:  0x{:02x}  [{}]", a, decode_attr(a))),
        None => console.println("  attr:  (directory sector unreadable, -EIO)"),
    }
    console.println(&alloc::format!("  clus:  0x{:x}", de.first_cluster()));
    // fmt_mtime pads a zeroed stamp to a dash within a 19-char field; trim to a bare `-` here.
    console.println(&alloc::format!("  mtime: {}", fmt_mtime(&de).trim()));
    console.println(&alloc::format!("  entry: LBA {} slot +{}", dir_lba, dir_off));
}

/// JD19: hexdump `data` with each row labelled by its ABSOLUTE file offset (`base` + row start), in
/// the canonical `OFFSET: <16 hex bytes> | <ascii> |` layout (non-printables render as `.`). Distinct
/// from the `read`-verb `hexdump` (which labels rows from 0 and dumps a fixed 128 bytes): `xd` needs
/// the true file offset and a variable length, and pads a short final row so the ASCII gutter aligns.
fn xd_rows(console: &mut Console, base: usize, data: &[u8]) {
    for (i, chunk) in data.chunks(16).enumerate() {
        let mut line = alloc::format!("{:08x}: ", base + i * 16);
        for b in chunk {
            line.push_str(&alloc::format!("{:02x} ", b));
        }
        for _ in chunk.len()..16 {
            line.push_str("   "); // pad the short final row to keep the ASCII gutter aligned
        }
        line.push_str(" |");
        for b in chunk {
            let c = if *b >= 32 && *b < 127 { *b as char } else { '.' };
            line.push(c);
        }
        line.push('|');
        console.println(&line);
    }
}

/// JD19 `xd <path> [off] [len]`: bounded hexdump of a file's bytes via the offset-aware `read_at`.
/// Default off=0, len=256; `len` is hard-capped at `XD_MAX` (4096). Rows carry the absolute file
/// offset. An `off` at or past EOF is an honest empty note; a directory target is `-EISDIR`; the root
/// is `-EISDIR`. When more bytes remain past the dumped window an honest `[... n more byte(s)]` tail
/// note is printed. off/len are parsed decimal or `0x`-hex by the caller.
fn fs_xd(console: &mut Console, arg: &str, off: u32, len: usize) {
    const XD_MAX: usize = 4096;
    let Some(fs) = mount_read_volume(console, "xd") else { return };
    let (de, canon) = match resolve_path(&fs, &normalize_path(&cwd_path(), arg)) {
        Ok(Resolved::Root) => return console.println("xd: /: is a directory (-EISDIR)"),
        Ok(Resolved::Entry(de, canon)) => (de, canon),
        Err(msg) => return console.println(&alloc::format!("xd: {}", msg)),
    };
    if de.is_dir {
        return console.println(&alloc::format!("xd: {}: is a directory (-EISDIR)", canon));
    }
    let want = core::cmp::min(len, XD_MAX);
    let mut data: Vec<u8> = Vec::new();
    if let Err(e) = fs.read_at(de.first_cluster(), de.size, off, &mut data, want) {
        return console.println(&alloc::format!("xd: {}: {} ({:?})", canon, fat_errno(e), e));
    }
    if data.is_empty() {
        if off >= de.size {
            return console.println(&alloc::format!(
                "xd: {}: offset {} at/past EOF ({} byte(s)) — nothing to dump", canon, off, de.size));
        }
        return console.println(&alloc::format!("xd: {}: 0 byte(s) read", canon));
    }
    xd_rows(console, off as usize, &data);
    // Honest tail note whenever the file holds more bytes past the dumped window (a cap hit, a short
    // `len`, or both). `off + data.len()` never overflows the file: read_at delivered within `size`.
    let shown_end = off as usize + data.len();
    if (de.size as usize) > shown_end {
        console.println(&alloc::format!("[... {} more byte(s)]", de.size as usize - shown_end));
    }
}

pub struct History {
    entries: Vec<String>,
    position: usize,
}

impl History {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            position: 0,
        }
    }

    pub fn push(&mut self, cmd: String) {
        if !cmd.trim().is_empty() {
            self.entries.push(cmd);
            self.position = self.entries.len();
        }
    }
}

// --- MIDDEN-M1: the seam between the kernel console and the shared shell core -----------------
//
// Three small pieces, and deliberately only three: what this build can do (`midden_facts`), how
// the core asks "is that a file?" (`FatVolume`), and how a terminal message reaches the panel
// (`render_message`). Everything else about what a command MEANS now lives in
// `unaos/libs/sys/midden_core`, shared with the Ring 3 `midden` handler.

/// Describe this build to the core.
///
/// The core carries no `#[cfg(target_arch)]` — a shell core that only tells the truth on one arch
/// is not a shared core — so the facts are built here, at the single call site, with `cfg!` and
/// `#[cfg]`. `proc_rows` is READ from the process table rather than written down (HEADROOM split
/// the cap 10 on x86 / 6 on aarch64, and a help line naming a stale ceiling is worse than none).
fn midden_facts() -> midden_core::Facts {
    // The process verbs (`run`/`bg`/`jobs`/`kill`/`storm`) are registered on aarch64-baremetal and
    // on x86; this mirrors the `#[cfg]` on their match arms exactly.
    // `arch::syscall` itself is `baremetal`-gated on aarch64, so the cap must be read under the
    // SAME condition that decides whether the verbs exist — a build with no process table has no
    // storm cap to name, and 0 is the honest stand-in because `proc_verbs` is false beside it.
    #[cfg(any(all(feature = "baremetal", target_arch = "aarch64"), target_arch = "x86_64"))]
    let (proc_verbs, proc_rows) = (true, crate::arch::syscall::proc_table_rows());
    #[cfg(not(any(all(feature = "baremetal", target_arch = "aarch64"), target_arch = "x86_64")))]
    let (proc_verbs, proc_rows) = (false, 0usize);
    midden_core::Facts {
        aarch64: cfg!(target_arch = "aarch64"),
        x86: cfg!(target_arch = "x86_64"),
        v3d: cfg!(all(target_arch = "aarch64", feature = "v3d")),
        proc_verbs,
        proc_rows,
        // BARE-NAME LAUNCH is x86-only today: `bare_exec` + `spawn_user_image_bg` off the FAT
        // volume. With this false the core never probes the volume and never returns `Plan::Exec`,
        // so an aarch64 build does exactly what it did before this crate existed.
        exec: cfg!(target_arch = "x86_64"),
    }
}

/// The core's one filesystem question, answered over the shell's real volume.
///
/// Not a filesystem trait: `is_file` is true iff the name resolves to a regular (non-directory)
/// file from the shell's cwd, and that is all the resolver is allowed to know. Stateless like
/// every other FAT verb — the volume is mounted per question, so a swapped card is picked up on
/// the next command.
struct FatVolume;

impl midden_core::Volume for FatVolume {
    /// APPLOAD/LAUNCH-AR: probe the PROGRAM SOURCE, not the default handle.
    ///
    /// This is the half of APPLOAD that was missed. `read_el0_image` was moved to
    /// `mount_program_source()` because the global `BLOCK_DEVICE` cannot answer "where do
    /// executables live" on a machine booted from the internal SD reader — `SDHCBLK` registers the
    /// card as handle `Sdhc` and says so on the wire ("global BLOCK_DEVICE untouched"). But the
    /// LOADER is not the first thing a bare name meets: the RESOLVER is, and it was left on
    /// `mount()`. On Boot AR that asymmetry cost the operator the whole feature — `SDHCBLK` listed
    /// `12568 VUG.ELF`, the operator typed `vug`, and this probe answered `false` through a handle
    /// that does not exist, so the core never built `Plan::Exec` and the console said "Unknown
    /// command" about a file the same boot had just printed.
    ///
    /// A probe and a load that disagree about WHICH volume is the volume can only ever produce
    /// that shape, so they are now the same question asked of the same handle.
    ///
    /// FATVERB: it also STAMPS what it bound into [`EXEC_BIND`]. `fatverb_storage_witness` compares
    /// that stamp against the one the read verbs leave, which is the only version of "the probe and
    /// `ls` agree" that a reverted read verb can fail.
    #[cfg(target_arch = "x86_64")]
    fn is_file(&mut self, name: &str) -> bool {
        let fs = match crate::fs::fat::mount_program_source() {
            Ok(fs) => {
                stamp(&EXEC_BIND, &EXEC_BIND_SEQ, bind::of(&fs));
                fs
            }
            Err(e) => {
                stamp(&EXEC_BIND, &EXEC_BIND_SEQ, bind::DECLINED);
                // Loud, and able to fail: a resolver that answers "no such program" because it
                // could not mount anything is indistinguishable, from the panel, from a typo. This
                // line is the difference, and it names the handles that WERE available so the
                // capture shows what the probe was offered rather than only what it concluded.
                serial_println!(
                    ":: [midden] exec-probe \"{}\" -> NO VOLUME ({:?}; handles={}) ::",
                    name, e, crate::drivers::block::source_census()
                );
                return false;
            }
        };
        matches!(
            resolve_path(&fs, &normalize_path(&cwd_path(), name)),
            Ok(Resolved::Entry(de, _)) if !de.is_dir
        )
    }
    // aarch64 never sets `Facts::exec`, so the core never calls this. Answering `false` rather
    // than reaching for unafs keeps the promise: no behaviour change on a build with no loader.
    #[cfg(not(target_arch = "x86_64"))]
    fn is_file(&mut self, _name: &str) -> bool {
        false
    }
}

/// Render one `midden_core::Message` to the console — the Ring 0 half of "views are the only path
/// to the user".
///
/// The core returns ONE string; the console is line-oriented, so this splits on `\n` and prints
/// each line, which is byte-for-byte what the old per-line `console.println` calls did. `NoOp`
/// prints nothing at all (not a blank line): silence is the message.
fn render_message(console: &mut Console, msg: &midden_core::Message) {
    if matches!(msg, midden_core::Message::NoOp) {
        return;
    }
    for line in msg.text().split('\n') {
        console.println(line);
    }
}

/// MIDDEN-M1 WITNESS (witness battery): prove, headlessly and on BOTH arches, that the console's
/// interpreter is the shared core and that extension-elided resolution works.
///
/// Why a fixture and not just the live line: the live `:: [midden] ... ::` witness needs a
/// keystroke, and the headless QEMU gates type nothing. This drives the same `midden_core::plan`
/// the prompt drives, over a synthetic volume, and asserts the four properties that would silently
/// rot: a core verb is answered in-core, a host verb is routed (not swallowed), a bare name elides
/// `.elf` to the on-disk spelling, and a verb still beats a program of the same stem. Each check
/// prints in the uniform `:: TSTE: <name> -> PASS/FAIL ::` shape, so the boot-replay ring picks
/// them up and `tste` lists them.
///
/// It can fail: every assertion compares against a literal expected value, and a FAIL line names
/// what it got.
#[cfg(feature = "witness")]
pub fn midden_witness() {
    fn verdict(name: &str, ok: bool, got: &str) {
        if ok {
            serial_println!(":: TSTE: {} -> PASS ::", name);
        } else {
            serial_println!(":: TSTE: {} -> FAIL (got {}) ::", name, got);
        }
    }

    // A volume that is NOT the real one, and a build description that is NOT this build's: the
    // fixture must prove the RESOLVER, on every arch, not the card in the slot or the `cfg` this
    // kernel happens to carry. (`vug` is a verb on aarch64 and a program name on x86 — the fixture
    // pins the x86 shape so the `.elf` elision is exercised on the Pi gate too, which is the only
    // headless gate that runs today.)
    const NAMES: &[&str] = &["VUG.ELF", "STAT.ELF", "README.TXT"];
    let facts = midden_core::Facts {
        x86: true,
        exec: true,
        proc_verbs: true,
        proc_rows: midden_facts().proc_rows,
        aarch64: false,
        v3d: false,
    };

    // 1. dispatch — a core verb is answered by the core, with real text.
    let mut vol = midden_core::NameList(NAMES);
    let p = midden_core::plan("echo midden m1", &facts, &mut vol);
    let ok = matches!(&p, midden_core::Plan::Say(m)
        if m.kind() == "TerminalOutput" && m.text() == "midden m1");
    verdict("midden.dispatch", ok, &alloc::format!("{:?}", p));

    // 2. routing — a host verb comes back as Host with its args intact, never swallowed.
    let mut vol = midden_core::NameList(NAMES);
    let p = midden_core::plan("cat DOCS/README.TXT", &facts, &mut vol);
    let ok = matches!(&p, midden_core::Plan::Host { verb, rest }
        if verb == "cat" && rest == "DOCS/README.TXT");
    verdict("midden.route", ok, &alloc::format!("{:?}", p));

    // 3. resolve — the `.elf` the user did not type, elided against the on-disk name.
    let mut vol = midden_core::NameList(NAMES);
    let p = midden_core::plan("vug", &facts, &mut vol);
    let ok = matches!(&p, midden_core::Plan::Exec { typed, name }
        if typed == "vug" && name == "VUG.ELF");
    verdict("midden.resolve", ok, &alloc::format!("{:?}", p));
    if let midden_core::Plan::Exec { typed, name } = &p {
        serial_println!(":: [midden] resolve \"{}\" -> {} ::", typed, name);
    }

    // 4. precedence — `stat` is a verb and STAT.ELF is on the volume; the verb wins. This is the
    //    collision MIDDEN_CONVERGENCE §5 records, pinned here so a later "improvement" that lets a
    //    program shadow a verb cannot land quietly.
    let mut vol = midden_core::NameList(NAMES);
    let p = midden_core::plan("stat FOO", &facts, &mut vol);
    let ok = matches!(&p, midden_core::Plan::Host { verb, .. } if verb == "stat");
    verdict("midden.precedence", ok, &alloc::format!("{:?}", p));

    // The STORAGE legs used to sit here, and that was the defect the adoption review convicted.
    // `midden_witness` runs at main.rs step 5, BEFORE `pci::init` and before the USB storage
    // publish, so every storage leg read `handles=global=absent sdhc=absent` and passed on all-false
    // inputs — vacuous on QEMU and on metal, forever. They now live in `fatverb_storage_witness`
    // below, called from the storage-ready service pass beside `fat::probe_once`.
}

// ===================== FATVERB — the storage witness, after storage exists =========================

/// FATVERB: one-shot latch. The service loop calls the witness every pass; it must speak once.
#[cfg(target_arch = "x86_64")]
static FATVERB_WITNESS_DONE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// FATVERB: when the wait for a program source began (`arch::ticks()` ms, 0 = not yet waiting).
#[cfg(target_arch = "x86_64")]
static WITNESS_WAIT_SINCE_MS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// FATVERB: how long to wait for a block device before witnessing an empty census anyway. Same
/// value and same reasoning as `video::wcx::STORAGE_WAIT_MS` — generous against the deferred SCSI
/// bring-up on this bench, and the number matters far less than the wait terminating in a line.
#[cfg(target_arch = "x86_64")]
const STORAGE_WAIT_MS: u64 = 30_000;

/// FATVERB: THE VERBS AND THE PROBE MUST BIND THE SAME HANDLE, AND A WRITE VERB MUST CONSULT THE GATE.
///
/// **Where this runs, and why that is the whole point.** The first cut put these legs in
/// `midden_witness`, which fires at main.rs step 5 — before `pci::init`, before the xHCI storage
/// publish, before `sdhc::bring_up`. The review's capture settled it: all three legs passed with
/// `handles=global=absent sdhc=absent`, i.e. with every input false. A fixture whose inputs cannot
/// be non-trivial is not quiet, it is dead. This one is called from the storage-ready service pass,
/// immediately after `fat::probe_once()` / `sdhc_probe_once()` have run, so by the time it speaks
/// the census names real handles and the questions have content.
///
/// **What makes each leg falsifiable.** Not comparing an expression with itself — the other half of
/// the review. Each leg DRIVES A REAL VERB and reads the stamp that verb left behind:
///
/// * `fatverb.readvol` runs `ls_path`, the function the `ls`/`dir` dispatch arm calls, and then
///   requires (a) that the read-verb binding counter ADVANCED across that call, and (b) that the
///   handle it stamped equals the handle the exec probe stamped. Revert `ls_path` to the old
///   default-handle `fat::mount` and (a) fails, because a reverted verb never reaches the recorder. Point the
///   read helper at a different handle and (b) fails. Neither half can be satisfied by evaluating
///   one expression twice, which is precisely what the first cut did.
///
/// * `fatverb.writegate` runs `fs_rm` against a name that cannot exist, and requires that the write
///   gate's counter advanced and that its recorded answer matches the mounted source's veto. The
///   probe name is deliberately unresolvable, so on a WRITABLE volume the verb gates through and
///   then stops at `NotFound` — the fixture drives the real refusal path without ever creating,
///   truncating or unlinking anything. On a READ-ONLY source the gate fires and the verb never
///   reaches the directory at all, which is the property the arc exists to guarantee.
///
/// The verbs print to a THROWAWAY `Console` (heap-only, no panel, dropped on return), so the
/// fixture's own output never lands on the operator's screen; the verbs' serial mirrors still reach
/// the capture, which is where a bench reader wants them.
///
/// **Quiet, not vacuous, in QEMU.** Where the handles agree the stamps agree and the legs pass —
/// but they pass on inputs that were free to differ, and the counters prove the verbs ran.
#[cfg(target_arch = "x86_64")]
pub fn fatverb_storage_witness() {
    use core::sync::atomic::Ordering;
    if FATVERB_WITNESS_DONE.load(Ordering::Acquire) {
        return;
    }
    // BOUNDED WAIT FOR A PROGRAM SOURCE — the second half of "not vacuous", and it is not optional.
    //
    // Being called from the storage-ready pass is necessary but not sufficient: the pass begins
    // running long before the deferred SCSI bring-up behind `service_storage` finishes, so a witness
    // that latched on its FIRST call would fire with an empty census on exactly the configurations
    // where storage is slow — which is the `midden_witness` failure again, one screen further down
    // the boot. Measured on this host: the plain `./arroyo test` shape had `sdhc=present` on the
    // first pass, and the `test-fat sf` shape had nothing at all, so the two differ and neither can
    // be assumed.
    //
    // The shape is `wcx::desktop_app_service`'s, deliberately — including its law that the wait
    // TERMINATES IN A LINE rather than in silence. A boot that genuinely never gets a block device
    // must still emit these legs (a read verb with no volume is Boot AR's own symptom, and a spec
    // REQUIRE must not go red because the machine had no card in it), so the deadline expires into
    // the witness rather than out of it, and the census on the line says which it was.
    if crate::drivers::block::program_source().is_none() {
        let now = crate::arch::ticks();
        let started = WITNESS_WAIT_SINCE_MS.load(Ordering::Relaxed);
        if started == 0 {
            WITNESS_WAIT_SINCE_MS.store(now.max(1), Ordering::Relaxed);
            return;
        }
        if now.saturating_sub(started) < STORAGE_WAIT_MS {
            return;
        }
        // Fall through: speak on an empty census, and say so below.
    }
    if FATVERB_WITNESS_DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let waited = match WITNESS_WAIT_SINCE_MS.load(Ordering::Relaxed) {
        0 => 0,
        t => crate::arch::ticks().saturating_sub(t),
    };

    fn verdict(name: &str, ok: bool, got: &str) {
        if ok {
            serial_println!(":: TSTE: {} -> PASS ::", name);
        } else {
            serial_println!(":: TSTE: {} -> FAIL (got {}) ::", name, got);
        }
    }

    // A name that resolves on no volume anyone will ever stage: the write leg must be able to run
    // on a WRITABLE boot volume without mutating it, so the gate is what it exercises, not the
    // directory. `$` is legal in a FAT 8.3 name, so this reaches the resolver rather than dying in
    // path validation — the leg must test the gate, not the parser.
    const NOSUCH: &str = "$FATVERB.$$$";
    // The read probe: any name will do, because the leg asserts AGREEMENT about the handle, never
    // presence of the file. `VUG.ELF` is used only so the exec probe's stamp comes from the same
    // question `bare_exec` asks in anger.
    const PROBE: &str = "VUG.ELF";

    let census = crate::drivers::block::source_census();

    // --- readvol -------------------------------------------------------------------------------
    let exec_seq0 = EXEC_BIND_SEQ.load(BindOrd::Relaxed);
    let mut v = FatVolume;
    let _ = midden_core::Volume::is_file(&mut v, PROBE);
    let exec_ran = EXEC_BIND_SEQ.load(BindOrd::Relaxed) > exec_seq0;
    let exec_bound = EXEC_BIND.load(BindOrd::Relaxed);

    let read_seq0 = READ_BIND_SEQ.load(BindOrd::Relaxed);
    {
        // The REAL read verb, exactly as the `ls`/`dir` dispatch arm invokes it.
        let mut sink = Console::new();
        ls_path(&mut sink, ".", false);
    }
    let read_ran = READ_BIND_SEQ.load(BindOrd::Relaxed) > read_seq0;
    let read_bound = READ_BIND.load(BindOrd::Relaxed);

    verdict(
        "fatverb.readvol",
        exec_ran && read_ran && exec_bound == read_bound,
        &alloc::format!(
            "exec_ran={} read_ran={} exec={} read={} handles={}",
            exec_ran, read_ran, bind::name(exec_bound), bind::name(read_bound), census
        ),
    );

    // --- writegate -----------------------------------------------------------------------------
    // The independent side of the comparison: what the SOURCE says, read straight off the block
    // layer's own predicate, before the verb runs.
    let veto = crate::fs::fat::mount_program_source().ok().and_then(|fs| fs.write_veto());
    let gate_seq0 = WRITE_GATE_SEQ.load(BindOrd::Relaxed);
    {
        // The REAL write verb. `force = true` so a hypothetical prompt cannot stall the boot.
        let mut sink = Console::new();
        fs_rm(&mut sink, NOSUCH, true);
    }
    let gate_ran = WRITE_GATE_SEQ.load(BindOrd::Relaxed) > gate_seq0;
    let gate = WRITE_GATE.load(BindOrd::Relaxed);
    let gate_agrees = match veto {
        Some(_) => gate == bind::REFUSED_RO,
        None => gate == bind::ADMITTED || gate == bind::DECLINED,
    };
    verdict(
        "fatverb.writegate",
        gate_ran && gate_agrees,
        &alloc::format!(
            "gate_ran={} gate={} veto={} handles={}",
            gate_ran, bind::name(gate), veto.unwrap_or("none"), census
        ),
    );

    // `waited` is the MEASUREMENT, not the threshold: a reader can tell "storage was there on the
    // first pass" (0 ms) from "the deadline expired and this census is empty because nothing ever
    // arrived" (>= the threshold), which is the difference between a quiet leg and a dead one.
    serial_println!(
        ":: [fatverb] storage witness: exec={} read={} gate={} waited={}ms handles={} ::",
        bind::name(exec_bound), bind::name(read_bound), bind::name(gate), waited, census
    );
}

/// Run one command. Returns `true` if the command took over the whole screen with its own
/// graphics (e.g. `vug`), so the caller should NOT repaint the console over it.
pub fn dispatch_command(cmd_line: &str, console: &mut Console, pal: &mut TargetPal) -> bool {
    // MIDDEN-M1 — ONE INTERPRETER, AND IT IS MIDDEN'S.
    //
    // This function used to be a rival shell: it split the line itself, decided by `match` arm
    // what counted as a command, wrote its own help text, and invented "Unknown command" in a
    // `_ =>` fallthrough — none of it shared with `handlers/midden`, the shell handler that is
    // supposed to BE the UnaOS shell. Two interpreters, two command tables, no shared line.
    //
    // Now the line goes to `midden_core::plan` first (unaos/libs/sys/midden_core — no_std,
    // forbid(unsafe_code), the same shared-core convention as libs/fs/unafs and libs/sys/helm),
    // and this function services what comes back:
    //
    //   Nothing  — empty line, nothing printed (the old `"" => {}` arm).
    //   Say(msg) — the core answered in full (help/echo/ver/gneiss, and every refusal). We render
    //              the `TerminalOutput`/`TerminalError` and return. THIS is the console rendering
    //              a bandy-shaped terminal message, in Ring 0, off the shared core.
    //   Exec     — a bare name resolved to a program on disk, `.elf` elided if the user left it
    //              off (x86; see `bare_exec`). `ls` still shows STAT.ELF — elision is a lookup
    //              rule, never a storage or display rule.
    //   Host     — a verb the core knows but only the kernel can perform. The giant match below
    //              is that service layer, and ONLY that: it no longer decides what a word means.
    //
    // What is still deferred is stated plainly rather than implied: the FAT/VFS/net/process verbs
    // remain kernel-side implementations reached through `Plan::Host`. See
    // docs/dev/USERLAND/MIDDEN_CONVERGENCE.md §2 for the split and what M2 moves.
    let facts = midden_facts();
    let mut vol = FatVolume;
    let plan = midden_core::plan(cmd_line, &facts, &mut vol);

    // WITNESS (must be able to fail): one line per dispatched line, naming the message the core
    // produced. A line that never reached the core cannot print this, and a core that produced
    // nothing prints `len=0` rather than a plausible number.
    match &plan {
        midden_core::Plan::Nothing => {}
        midden_core::Plan::Say(msg) => serial_println!(
            ":: [midden] cmd=\"{}\" -> {} len={} ::",
            cmd_line.trim(), msg.kind(), msg.len()
        ),
        // LAUNCH-AR: `cmd=`-shaped, like its three siblings, and NOT the fixture's wording.
        //
        // This arm used to print `resolve "vug" -> VUG.ELF`, which is byte-for-byte the line
        // `midden_witness` prints from its synthetic `NameList`. Reading Boot AR's capture, that
        // made the ONE `resolve "vug" -> VUG.ELF` in the log look like proof the operator's launch
        // had resolved, when it was the boot fixture talking and the operator's own line was the
        // `TerminalError` two thousand lines later. A witness that cannot be told apart from a
        // fixture is not a witness. Same `cmd="<line>" ->` prefix as `Say`/`Host` now, so the
        // operator's dispatch is greppable as one family and the disposition is the tail.
        midden_core::Plan::Exec { name, .. } => serial_println!(
            ":: [midden] cmd=\"{}\" -> Exec {} ::", cmd_line.trim(), name
        ),
        midden_core::Plan::Host { verb, .. } => serial_println!(
            ":: [midden] cmd=\"{}\" -> Host verb={} ::", cmd_line.trim(), verb
        ),
    }

    let (verb, rest) = match plan {
        midden_core::Plan::Nothing => return false,
        midden_core::Plan::Say(msg) => {
            render_message(console, &msg);
            return false;
        }
        midden_core::Plan::Exec { typed, name } => {
            #[cfg(target_arch = "x86_64")]
            bare_exec(console, &typed, &name);
            // No build outside x86 sets `Facts::exec`, so the core never hands this arm a plan
            // there; the branch exists so the match is total and the day a loader arrives on
            // another arch the compiler points here.
            #[cfg(not(target_arch = "x86_64"))]
            {
                let _ = (&typed, &name);
                console.println("Unknown command. Type 'help' for assistance.");
            }
            return false;
        }
        midden_core::Plan::Host { verb, rest } => (verb, rest),
    };
    let command: &str = &verb;
    let args: Vec<&str> = rest.split_whitespace().collect();

    // The `vug` and `pulse` commands paint full-screen views; everything else leaves the
    // console visible. PI-APP-1: `v3d` blits the visible battery onto the live scanout, so it too
    // keeps the console off (a repaint would overwrite the replayed tiles). Aarch64+v3d only; the
    // knob-off build never registers the command, so this OR-clause is constant-folded away there.
    //
    // This predicate must track which verbs are actually REGISTERED below, not which words exist. On
    // x86 neither `vug` nor `pulse` is registered (the demo module is aarch64-only), so nothing here
    // takes the screen: claiming otherwise would suppress the console repaint that carries the
    // "Unknown command" reply, and typing `vug` would present as a hang instead of a refusal.
    #[cfg(all(target_arch = "aarch64", feature = "v3d"))]
    let took_screen = command == "vug" || command == "pulse" || command == "v3d";
    #[cfg(all(target_arch = "aarch64", not(feature = "v3d")))]
    let took_screen = command == "vug" || command == "pulse";
    #[cfg(not(target_arch = "aarch64"))]
    let took_screen = false;

    match command {
        "date" => {
            // JD17/CLOCK-3/PI-UI-3: show the kernel wall clock. The UNIFIED civil clock is the source of
            // truth: prefer the Unix anchor (an SNTP sync on the Pi — PI-NET-16 — or a `setdate` seed), so a
            // networked board shows the REAL date with no operator action. Historically `date` read only the
            // JD17 FAT anchor (`now()`), which the SNTP path never plants (it anchors the civil clock via
            // `set_anchor`), so a synced Pi still printed "clock not set" — the bug behind PI-UI-3. Fall back
            // to the FAT anchor, then to the honest UNSET state. `unix_now()` + `civil_from_unix` mirror the
            // `time` verb's path so `date` and `time` never disagree.
            let ymd = match crate::clock::unix_now() {
                Some(secs) => {
                    let (y, mo, d, h, mi, s) = crate::clock::civil_from_unix(secs);
                    Some((y as u32, mo, d, h, mi, s))
                }
                None => crate::clock::now()
                    .map(|t| (t.year, t.month, t.day, t.hour, t.min, t.sec)),
            };
            match ymd {
                Some((y, mo, d, h, mi, s)) => ui3_say(console, "date", &alloc::format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, mi, s)),
                None => ui3_say(console, "date", "date: clock not set (setdate YYYY-MM-DD HH:MM:SS)"),
            }
        },
        "setdate" => {
            // JD17: seed the wall clock — `setdate YYYY-MM-DD HH:MM:SS` (seconds optional). The
            // architectural counter extends it forward from this moment; new/rewritten FAT
            // entries are mtime-stamped from it. Re-seeding replaces the anchor.
            match parse_setdate(&args) {
                Some(t) if crate::clock::set(t).is_ok() => {
                    console.println(&alloc::format!(
                        "clock set: {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                        t.year, t.month, t.day, t.hour, t.min, t.sec));
                }
                _ => console.println(
                    "setdate: usage: setdate YYYY-MM-DD HH:MM[:SS]  (year 1980-2107)"),
            }
        },
        "time" => {
            // CLOCK-1: the shared kernel civil clock — ISO-8601 UTC plus the source that set it.
            // UNSET is first-class and honest: `unsynced` until an SNTP sync (pi/genet PI-NET-16) or a
            // `setdate` seeds it. x86 has no SNTP client yet, so `time` there reads `unsynced` until a
            // manual `setdate` — the seam is what this arc delivers; x86 SNTP is a future rmbp arc.
            let mut buf = [0u8; 24];
            match crate::clock::iso8601_now(&mut buf) {
                Some(n) => {
                    let iso = core::str::from_utf8(&buf[..n]).unwrap_or("<iso>");
                    let src = match crate::clock::source() {
                        crate::clock::ClockSource::Sntp { stratum } =>
                            alloc::format!("sntp, stratum {}", stratum),
                        crate::clock::ClockSource::Manual => alloc::format!("manual"),
                        crate::clock::ClockSource::Unset => alloc::format!("unsynced"),
                    };
                    // PI-UI-3: mirror to serial (verb output is panel-only on the bench).
                    ui3_say(console, "time", &alloc::format!("{} ({})", iso, src));
                }
                None => ui3_say(console, "time", "time: unsynced (no SNTP sync or setdate yet)"),
            }
        },
        "clear" => {
            // Clear both the screen and the console buffer?
            // Usually 'clear' clears the visible screen.
            // For now, we will rely on console.draw() to repaint.
            // To effectively clear, we might want to clear the lines in console?
            // Or just clear screen. But draw() repaints lines.
            // Let's implement a 'clear' on console if needed, or just let draw handle it.
            // If the user wants a blank slate, we should probably clear the history buffer.
            // BUT, the prompt said "Reset cursor logic here".
            // Let's implement a clear method on Console.
            console.clear();
        },
        "panic" => {
            // Test the Exception Handler
            panic!("Manual Panic Requested by Architect!");
        },
        "usbinfo" => {
            for line in crate::drivers::xhci::usb_summary() {
                console.println(&line);
            }
        },
        "fatinfo" => {
            if let Some(fs) = mount_read_volume(console, "fatinfo") {
                console.println(&fs.describe());
            }
        },
        "ls" | "dir" => {
            // JD4: `ls` lists the cwd; `ls <dir>` any path (absolute or cwd-relative). An `ls` of
            // a plain file prints its one table line (the DOS idiom), not an error. JD12: `ls *.EXT`
            // lists the wildcard matches (sorted, with the file/dir tally). JD16: `-l` selects the
            // long format (size + FAT last-write timestamp + name); flags are filtered from the paths
            // (a `-`+letters arg — same convention as cp/rm/mv), so a file literally named `-l` is
            // reachable as `./-l`. `l`/`L` set long; other flag letters are ignored.
            let long = args.iter().any(|&a|
                a.len() > 1 && a.starts_with('-')
                && a[1..].bytes().all(|b| b.is_ascii_alphabetic())
                && a[1..].bytes().any(|b| b == b'l' || b == b'L'));
            let path = args.iter().copied().find(|a|
                !(a.len() > 1 && a.starts_with('-')
                  && a[1..].bytes().all(|b| b.is_ascii_alphabetic())));
            // PI-SHELL-LS: on the Pi the native store is unafs (the volume `/fs/` serves), not FAT, so
            // route `ls` there. `ls <path>` resolves subpaths; wildcards fall back to a literal resolve
            // (unafs has no glob layer — an honest not-found if it isn't a real name). x86 keeps FAT.
            #[cfg(target_arch = "aarch64")]
            {
                pi_ls(console, path.unwrap_or("."), long);
            }
            #[cfg(not(target_arch = "aarch64"))]
            match path {
                Some(a) if has_glob(a) => ls_globbed(console, a, long),
                other => ls_path(console, other.unwrap_or("."), long),
            }
        },
        "cd" => {
            // JD4: change the shell's working directory. No argument (or `/`) returns to the
            // root. The stored cwd is the CANONICAL on-disk spelling of the resolved path.
            let path = normalize_path(&cwd_path(), args.first().copied().unwrap_or("/"));
            if let Some(fs) = mount_read_volume(console, "cd") {
                match resolve_path(&fs, &path) {
                    Ok(Resolved::Root) => {
                        *CWD.lock() = None;
                        console.println("/");
                    }
                    Ok(Resolved::Entry(de, canon)) => {
                        if de.is_dir {
                            console.println(&canon);
                            *CWD.lock() = Some(canon);
                        } else {
                            console.println(&alloc::format!(
                                "cd: {}: not a directory (-ENOTDIR)", canon));
                        }
                    }
                    Err(msg) => console.println(&alloc::format!("cd: {}", msg)),
                }
            }
        },
        "pwd" => {
            console.println(&cwd_path());
        },
        "cat" | "type" => {
            // JD4: `cat` takes a path (absolute or cwd-relative), e.g. `cat DOCS/README.TXT`.
            match args.first() {
                None => console.println("usage: cat <path>"),
                Some(name) if has_glob(name) => cat_globbed(console, name),
                Some(name) => {
                    if let Some(fs) = mount_read_volume(console, "cat") {
                        match resolve_path(&fs, &normalize_path(&cwd_path(), name)) {
                            Ok(Resolved::Root) =>
                                console.println("cat: /: is a directory (-EISDIR)"),
                            Ok(Resolved::Entry(de, canon)) => cat_render(console, &fs, &de, &canon),
                            Err(msg) => console.println(&alloc::format!("cat: {}", msg)),
                        }
                    }
                }
            }
        },
        "head" => {
            // JD12: print the FIRST n lines of a file (default 10). `head <path> [n]`.
            match args.first() {
                None => console.println("usage: head <path> [lines]"),
                Some(path) => {
                    let n = args.get(1).and_then(|s| s.parse::<u32>().ok()).unwrap_or(10);
                    fs_head(console, path, n);
                }
            }
        },
        "tail" => {
            // JD12: print the LAST n lines of a file (default 10). `tail <path> [n]`.
            match args.first() {
                None => console.println("usage: tail <path> [lines]"),
                Some(path) => {
                    let n = args.get(1).and_then(|s| s.parse::<u32>().ok()).unwrap_or(10);
                    fs_tail(console, path, n);
                }
            }
        },
        "find" => {
            // JD18: recursive glob search over the tree — `find <root> <pattern>` (one arg = the
            // pattern, root defaults to `.`). Read-only walk; prints each hit's canonical path then
            // an honest `N match(es), M dir(s) scanned` tally. Missing root → -ENOENT; a file root
            // degrades to a self-match test; a mid-walk read error reports the partial results.
            match args.len() {
                0 => console.println("usage: find [root] <pattern>"),
                1 => fs_find(console, ".", args[0]),
                _ => fs_find(console, args[0], args[1]),
            }
        },
        "du" => {
            // JD18: recursive subtree size tally — `du <dir>` (default the cwd). Per direct child a
            // total-bytes line, then a `total: ...` line. FAT directory entries report size 0, so
            // only file bytes are real; a directory's size is the recursive sum of its files.
            fs_du(console, args.first().copied().unwrap_or("."));
        },
        "uptime" => {
            // JD18: seconds since boot from the architectural counter (aarch64 CNTPCT/CNTFRQ),
            // rendered `up HH:MM:SS`; when the JD17 wall clock is set, the current time is appended.
            // x86 has no calibrated counter plumbed → an honest note.
            match crate::clock::uptime_secs() {
                Some(secs) => {
                    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
                    match crate::clock::now() {
                        Some(t) => console.println(&alloc::format!(
                            "up {:02}:{:02}:{:02} (clock: {:04}-{:02}-{:02} {:02}:{:02}:{:02})",
                            h, m, s, t.year, t.month, t.day, t.hour, t.min, t.sec)),
                        None => console.println(&alloc::format!("up {:02}:{:02}:{:02}", h, m, s)),
                    }
                }
                None => console.println("uptime: no calibrated counter on this arch"),
            }
        },
        "stat" => {
            // JD19: one entry's full on-disk detail (canonical path, kind, size, attr byte + flags,
            // first cluster, FAT mtime, and the forensic dir-entry LBA + slot offset). Read-only; no
            // glob (a metacharacter resolves literally → -ENOENT). `stat /` reports the root honestly.
            match args.first() {
                None => console.println("usage: stat <path>"),
                Some(path) => fs_stat(console, path),
            }
        },
        "xd" => {
            // JD19: bounded hexdump — `xd <path> [off] [len]` (default off=0, len=256; len capped at
            // 4096). off/len accept decimal or 0x-hex. off past EOF = honest empty; a directory =
            // -EISDIR; an honest `[... n more byte(s)]` tail note when the file is larger.
            match args.first() {
                None => console.println("usage: xd <path> [off] [len]"),
                Some(path) => {
                    let off = args.get(1).and_then(|s| parse_num(s)).unwrap_or(0) as u32;
                    let len = args.get(2).and_then(|s| parse_num(s)).map(|n| n as usize).unwrap_or(256);
                    fs_xd(console, path, off, len);
                }
            }
        },
        #[cfg(target_arch = "aarch64")]
        "uls" => {
            // BeFS-K3/K4: list a directory on the native unafs volume (absolute paths,
            // case-sensitive names — unafs has no shell cwd). `uls` lists the root. Routes through
            // the single coherent mount (`with_unafs`); a pure read never writes.
            let path = args.first().copied().unwrap_or("/");
            let out = crate::fs::unafs::with_unafs(|fs| match fs.resolve_path(path) {
                Ok(id) => match fs.ls(id) {
                    Ok(entries) => {
                        let mut lines = alloc::vec::Vec::new();
                        for de in &entries {
                            let size = fs.read_inode(de.inode_id).map(|i| i.size).unwrap_or(0);
                            if de.kind == ::unafs::FileKind::Directory {
                                lines.push(alloc::format!("  <DIR>              {}", de.name));
                            } else {
                                lines.push(alloc::format!("  {:>10}  {}", size, de.name));
                            }
                        }
                        lines.push(alloc::format!("  ({} entries)", entries.len()));
                        lines
                    }
                    Err(e) => alloc::vec![alloc::format!("uls: {}: {:?}", path, e)],
                },
                Err(e) => alloc::vec![alloc::format!("uls: {}: {:?}", path, e)],
            });
            match out {
                Ok(lines) => for line in &lines { console.println(line); },
                Err(e) => console.println(&alloc::format!("uls: no unafs volume ({:?})", e)),
            }
        },
        #[cfg(target_arch = "aarch64")]
        "ucat" => {
            // BeFS-K3/K4: print a file off the native unafs volume (bounded like `cat`).
            match args.first() {
                None => console.println("usage: ucat <path>"),
                Some(path) => {
                    let out = crate::fs::unafs::with_unafs(|fs| match fs.resolve_path(path) {
                        Ok(id) => match fs.read_inode(id) {
                            Ok(inode) if inode.kind == ::unafs::FileKind::Directory =>
                                alloc::vec![alloc::format!("ucat: {}: is a directory (-EISDIR)", path)],
                            Ok(inode) => {
                                const CAP: u64 = 8192;
                                let want = inode.size.min(CAP);
                                match fs.read_data(id, 0, want) {
                                    Ok(data) => {
                                        let text: String = data.iter().filter_map(|&b| match b {
                                            b'\n' => Some('\n'),
                                            b'\r' => None,
                                            0x20..=0x7e => Some(b as char),
                                            _ => Some('.'),
                                        }).collect();
                                        let mut lines: alloc::vec::Vec<String> =
                                            text.split('\n').map(|s| s.into()).collect();
                                        if inode.size > want {
                                            lines.push(alloc::format!(
                                                "[... {} of {} bytes shown]", want, inode.size));
                                        }
                                        lines
                                    }
                                    Err(e) => alloc::vec![alloc::format!("ucat: {}: {:?}", path, e)],
                                }
                            }
                            Err(e) => alloc::vec![alloc::format!("ucat: {}: {:?}", path, e)],
                        },
                        Err(e) => alloc::vec![alloc::format!("ucat: {}: {:?}", path, e)],
                    });
                    match out {
                        Ok(lines) => for line in &lines { console.println(line); },
                        Err(e) => console.println(&alloc::format!("ucat: no unafs volume ({:?})", e)),
                    }
                }
            }
        },
        #[cfg(target_arch = "aarch64")]
        "utouch" => {
            // BeFS-K4: create a 0-length file on the native unafs volume (error if it exists or the
            // parent is missing). Absolute paths. Write-through + durable via the coherent mount.
            match args.first().copied() {
                None => console.println("usage: utouch <path>"),
                Some(path) => console.println(&unafs_verb_touch(path)),
            }
        },
        #[cfg(target_arch = "aarch64")]
        "uwrite" => {
            // BeFS-K4: create-or-replace a file on the native unafs volume with the given text
            // (`uwrite <path> <text...>`). Durable write-through.
            match args.first().copied() {
                None => console.println("usage: uwrite <path> <text...>"),
                Some(path) => {
                    let text = args[1..].join(" ");
                    console.println(&unafs_verb_write(path, text.as_bytes()));
                }
            }
        },
        #[cfg(target_arch = "aarch64")]
        "umkdir" => {
            // BeFS-K4: create a directory on the native unafs volume (`umkdir <path>`).
            match args.first().copied() {
                None => console.println("usage: umkdir <path>"),
                Some(path) => console.println(&unafs_verb_mkdir(path)),
            }
        },
        #[cfg(target_arch = "aarch64")]
        "urm" => {
            // BeFS-K4: delete a file on the native unafs volume (`urm <path>`). A directory is
            // refused (the crate's `unlink` returns IsADirectory) — mirrors POSIX `rm` without -r.
            match args.first().copied() {
                None => console.println("usage: urm <path>"),
                Some(path) => console.println(&unafs_verb_rm(path)),
            }
        },
        #[cfg(target_arch = "aarch64")]
        "usnaps" => {
            // K8b: list retained snapshots (the on-disk snapshot index) on the native unafs volume.
            let out = crate::fs::unafs::with_unafs(|fs| match fs.snapshot_index() {
                Ok(snaps) => {
                    let mut lines = alloc::vec::Vec::new();
                    if snaps.is_empty() {
                        lines.push(String::from("  (no retained snapshots)"));
                    } else {
                        for s in &snaps {
                            lines.push(alloc::format!(
                                "  gen {:>6}  {:<16}  by {:<12}  @{}",
                                s.generation, s.name, s.creator, s.timestamp
                            ));
                        }
                        lines.push(alloc::format!("  ({} of {} snapshots)", snaps.len(),
                            ::unafs::SNAPSHOT_CAP));
                    }
                    lines
                }
                Err(e) => alloc::vec![alloc::format!("usnaps: {:?}", e)],
            });
            match out {
                Ok(lines) => for line in &lines { console.println(line); },
                Err(e) => console.println(&alloc::format!("usnaps: no unafs volume ({:?})", e)),
            }
        },
        #[cfg(target_arch = "aarch64")]
        "usnap" => {
            // K8b: retain the current committed tree as a snapshot (`usnap <name>`). The shell runs
            // at kernel authority, so the creator principal recorded is "kernel" (owner-or-kernel
            // drop authority then admits any later usnapdrop from this surface).
            match args.first().copied() {
                None => console.println("usage: usnap <name>"),
                Some(name) => console.println(&unafs_verb_snap(name)),
            }
        },
        #[cfg(target_arch = "aarch64")]
        "usnapdrop" => {
            // K8b: drop a retained snapshot by its generation stamp (`usnapdrop <generation>`).
            // Reclamation drains eagerly; only blocks no live/retained root still reaches are freed.
            match args.first().copied().and_then(|s| s.parse::<u64>().ok()) {
                None => console.println("usage: usnapdrop <generation>"),
                Some(generation) => console.println(&unafs_verb_snapdrop(generation)),
            }
        },
        #[cfg(target_arch = "aarch64")]
        "usnapls" => {
            // K8c: list a retained snapshot's directory AS OF the snapshot (`usnapls <gen> [path]`).
            match args.first().copied().and_then(|s| s.parse::<u64>().ok()) {
                None => console.println("usage: usnapls <generation> [path]"),
                Some(generation) => {
                    let path = args.get(1).copied().unwrap_or("/");
                    for line in &unafs_verb_snapls(generation, path) {
                        console.println(line);
                    }
                }
            }
        },
        #[cfg(target_arch = "aarch64")]
        "usnapcat" => {
            // K8c: read a file from a retained snapshot under the LIVE object's CURRENT ACL
            // (`usnapcat <gen> <path>`). The shell is a kernel-authority surface, so it reads any
            // LIVE object — but a file DELETED from the live tree fails closed (no current ACL row),
            // the deleted-object edge of the high-security ruling.
            match args.first().copied().and_then(|s| s.parse::<u64>().ok()) {
                None => console.println("usage: usnapcat <generation> <path>"),
                Some(generation) => match args.get(1).copied() {
                    None => console.println("usage: usnapcat <generation> <path>"),
                    Some(path) => console.println(&unafs_verb_snapcat(generation, path)),
                },
            }
        },
        "touch" => {
            // JD6: create a 0-length file if absent (idempotent), in any reachable dir. `touch <path>`.
            match args.first() {
                None => console.println("usage: touch <path>"),
                Some(name) => fs_touch(console, name),
            }
        },
        "append" => {
            // JD5: append text at EOF, creating the file if absent (like `>>`). `append <path> <text>`.
            match args.first() {
                None => console.println("usage: append <path> <text>"),
                Some(name) => fs_append(console, name, args[1..].join(" ").as_bytes()),
            }
        },
        "rm" | "del" => {
            // JD6: delete a file in any reachable dir (a directory is -EISDIR; use `rmdir`). JD12: takes
            // multiple targets and expands trailing wildcards — `rm <path...>`, `rm *.TMP`. JD13: `-r`/`-R`
            // recursively deletes a directory tree (files then directories, depth-first; the root is
            // refused; an honest partial count on a mid-tree failure). Flags (leading `-`) are filtered
            // from the paths exactly as `cp`/`mv` do — a name literally beginning with `-` is reachable as
            // `./-name`. Without `-r`, a directory target stays -EISDIR (byte-identical to pre-JD13). JD14:
            // `-f`/force suppresses the missing-target error (POSIX `rm -f NOSUCH`/`rm -rf NOSUCH` are
            // quiet, and a no-match wildcard is quiet); bundled short flags parse (`rm -rf DIR`).
            let (recursive, force, _no_clobber, paths) = split_flags(&args);
            if paths.is_empty() {
                console.println("usage: rm [-r] [-f] <path> [path ...]");
            } else if !paths.iter().any(|a| has_glob(a)) {
                // No wildcard: delete each literal target (a single non-recursive/non-force arg is
                // byte-identical to pre-JD14).
                for &a in &paths {
                    if recursive { fs_rm_recursive(console, a, force); } else { fs_rm(console, a, force); }
                }
            } else {
                rm_globbed(console, &paths, recursive, force);
            }
        },
        "mkdir" | "md" => {
            // JD7: create a directory in any reachable parent (name exists → -EEXIST). `mkdir <path>`.
            match args.first() {
                None => console.println("usage: mkdir <path>"),
                Some(name) => fs_mkdir(console, name),
            }
        },
        "rmdir" | "rd" => {
            // JD7: remove an EMPTY directory (non-empty → -ENOTEMPTY; root refused). `rmdir <path>`.
            match args.first() {
                None => console.println("usage: rmdir <path>"),
                Some(name) => fs_rmdir(console, name),
            }
        },
        "vfs" => {
            // SHELL-WRITE: the unified VFS write surface. Unlike the FAT-direct
            // verbs above (`write`/`append`/`rm`/`mkdir`, which ride fat.rs on the
            // boot partition at cwd-relative paths), this routes create / write /
            // truncate / unlink through the VFS-2 `MountTable` over ONE namespace —
            // the native UnaFS volume at `/`, the FAT boot partition at `/fat` — so
            // a panel operator can exercise the per-object native ACL path and the
            // foreign volume-level path from the same surface. `vfs <op> <path>`.
            vfs_cmd(console, &args);
        },
        #[cfg(any(all(feature = "baremetal", target_arch = "aarch64"), all(feature = "tegra_el0", target_arch = "aarch64"), target_arch = "x86_64"))] // EXECGATE: tegra_el0 joins (JETSON-EL0 widened the vectors/sched/syscall, never the shell)
        "run" => {
            // EXEC-1: load an ELF64 user program off the VFS namespace and execute it in user mode, reporting its
            // exit status. Rides the SAME `MountTable` the `vfs` verb uses (`/fat` = FAT boot partition,
            // `/usb` = USB stick, `/` = native UnaFS), so `run /fat/ELFHELLO.ELF` loads the boot-partition
            // fixture. The bytes are read here (kernel mode/ASID 0) and handed to the kernel loader
            // (`run_user_image`), which maps them into a fresh per-task slot with per-segment W^X pages and
            // runs them under user mode + the fault-kill net. `run <path>`.
            //
            // X86RUN (GR20): also on x86, where the read side differs (there is no VFS namespace on
            // this arch) but nothing else does — see `read_el0_image`'s x86 twin for the path rules.
            // `run /fat/VUG.ELF` and `run VUG.ELF` both reach the DATA volume's root there.
            match args.first() {
                None => console.println("usage: run <path>   (load + execute an ELF64 user program)"),
                Some(&path) => run_program(console, path),
            }
        },
        "cp" | "copy" => {
            // JD8: copy a file (`cp FILE DIR/` lands as DIR/<leaf>). JD9: `-r`/`-R` recursively copies a
            // directory tree. Flags (leading `-`) precede the paths; only `-r`/`-R` are recognized (any
            // other leading-`-` arg is ignored as an unknown flag — a name literally beginning with `-` is
            // reachable as `./-name`). JD12: sources expand trailing wildcards and there may be several,
            // the LAST path being the destination (into a directory). JD14: no-clobber is the DEFAULT —
            // an existing destination FILE is `-EEXIST` unless `-f` (which overwrites); `-n` reasserts
            // the default (and overrides `-f`). `cp [-r] [-f|-n] <src...> <dst>`.
            let (recursive, force_raw, no_clobber, paths) = split_flags(&args);
            let force = force_raw && !no_clobber; // `-n` (no-clobber) overrides `-f` for safety
            if paths.len() < 2 {
                console.println("usage: cp [-r] [-f|-n] <src...> <dst>");
            } else if paths.len() == 2 && !has_glob(paths[0]) && !has_glob(paths[1]) {
                // No wildcard, one src: byte-identical to pre-JD12 cp (plus the JD14 no-clobber default).
                if recursive {
                    fs_cp_recursive(console, paths[0], paths[1], force);
                } else {
                    fs_cp(console, paths[0], paths[1], force);
                }
            } else {
                let dst = paths[paths.len() - 1];
                cp_globbed(console, &paths[..paths.len() - 1], dst, recursive, force);
            }
        },
        "mv" | "move" | "ren" | "rename" => {
            // JD10: move OR rename a file or directory by relinking one directory entry (O(1), by
            // reference — no data copy, so `mv DIR NEWNAME` needs no `-r`). Same parent → rename in
            // place (files AND dirs); across parents → move (files only, a dir there is -EISDIR). The
            // `mv SRC DIR/` idiom lands the entry under DIR as the source leaf. JD12: sources expand
            // trailing wildcards and there may be several, the LAST path being the destination
            // directory. JD14: no-clobber is the DEFAULT — an existing destination is `-EEXIST` unless
            // `-f` (which overwrites a FILE dest via delete-dst-first; a directory dest is still refused);
            // `-n` reasserts the default. Flags are filtered from the paths like `cp`/`rm`.
            // `mv [-f|-n] <src...> <dst>`.
            let (_recursive, force_raw, no_clobber, paths) = split_flags(&args);
            let force = force_raw && !no_clobber; // `-n` (no-clobber) overrides `-f` for safety
            if paths.len() < 2 {
                console.println("usage: mv [-f|-n] <src...> <dst>");
            } else if paths.len() == 2 && !has_glob(paths[0]) && !has_glob(paths[1]) {
                // No wildcard, one src: byte-identical to pre-JD12 mv (plus the JD14 flag parse).
                fs_mv(console, paths[0], paths[1], force);
            } else {
                let dst = paths[paths.len() - 1];
                mv_globbed(console, &paths[..paths.len() - 1], dst, force);
            }
        },
        "sync" => {
            // JD5-M3: storage is WRITE-THROUGH — block::write_block issues a synchronous BOT WRITE(10)
            // (USB) / polled CMD24 (SD) that completes before the command returns, so there is no
            // write-back cache to flush. `sync` is the honest confirmation of that (a no-op by design).
            console.println("sync: write-through storage — every write is already durable on the card");
        },
        "diskinfo" => {
            // PI-FS-5: on the Pi report BOTH storage devices — the SD card (emmc2, the global block device that
            // hosts unafs + the FAT boot partition) AND, when present, the USB stick (its own geometry from
            // `USB_BLOCK_DEVICE`, plus the FAT type/size/label read from the live read-only mount). x86 keeps the
            // single-device report below (its one block device IS the USB stick).
            #[cfg(target_arch = "aarch64")]
            {
                match crate::drivers::block::info() {
                    Some(d) => {
                        let vendor = core::str::from_utf8(&d.vendor).unwrap_or("?");
                        let product = core::str::from_utf8(&d.product).unwrap_or("?");
                        let cap_mib = (d.num_blocks * d.block_size as u64) / (1024 * 1024);
                        fs5_say(console, &alloc::format!(
                            "SD: {} {}  block {}  blocks {}  {} MiB (unafs + FAT boot)",
                            vendor.trim_end(), product.trim_end(), d.block_size, d.num_blocks, cap_mib));
                    }
                    None => fs5_say(console, "SD: no card block device ready."),
                }
                match crate::drivers::block::usb_info() {
                    Some(u) => {
                        let vendor = core::str::from_utf8(&u.vendor).unwrap_or("?");
                        let product = core::str::from_utf8(&u.product).unwrap_or("?");
                        let cap_mib = (u.num_blocks * u.block_size as u64) / (1024 * 1024);
                        fs5_say(console, &alloc::format!(
                            "USB: {} {}  block {}  blocks {}  {} MiB",
                            vendor.trim_end(), product.trim_end(), u.block_size, u.num_blocks, cap_mib));
                        // FAT type/size/label off the LIVE read-only mount (the volume `/fs/usb` and `ls /usb` serve).
                        match crate::fs::fat::mount_source(crate::fs::fat::BlockSource::Usb) {
                            Ok(fs) => {
                                let kind = match fs.kind() {
                                    crate::fs::fat::FatKind::Fat16 => "FAT16",
                                    crate::fs::fat::FatKind::Fat32 => "FAT32",
                                };
                                let label = fs.label();
                                let label = if label.is_empty() { String::from("-") } else { label };
                                let vol_mib = fs.volume_bytes() / (1024 * 1024);
                                // FATVERB: the posture is ASKED, not remembered. This line claimed
                                // "(read-only)" from PIUSB-27, which USB-WRITE F3 retired when it
                                // routed the `Usb` arm to the verified BOT WRITE(10) path — so
                                // `diskinfo` had been telling the operator the stick could not be
                                // written for as long as it could. One predicate now answers for the
                                // VFS, the shell's write gate and this line.
                                let posture = match fs.write_veto() {
                                    None => "read-write",
                                    Some(_) => "read-only",
                                };
                                fs5_say(console, &alloc::format!(
                                    "USB FAT: {}  label {}  volume {} MiB  mounted /usb ({})",
                                    kind, label, vol_mib, posture));
                            }
                            Err(e) => fs5_say(console, &alloc::format!(
                                "USB FAT: unmounted ({})", crate::fs::fat::fat_reason(e))),
                        }
                    }
                    None => fs5_say(console, "USB: no stick present."),
                }
            }
            #[cfg(not(target_arch = "aarch64"))]
            match crate::drivers::block::info() {
                Some(d) => {
                    let vendor = core::str::from_utf8(&d.vendor).unwrap_or("?");
                    let product = core::str::from_utf8(&d.product).unwrap_or("?");
                    let cap_mib = (d.num_blocks * d.block_size as u64) / (1024 * 1024);
                    console.println(&alloc::format!("Disk: {} {}", vendor.trim_end(), product.trim_end()));
                    console.println(&alloc::format!("Block size: {}  Blocks: {}  Capacity: {} MiB",
                        d.block_size, d.num_blocks, cap_mib));
                }
                None => {
                    console.println("No block device ready.");
                    // Surface how far USB mass-storage enumeration/bring-up got (metal diagnosis).
                    console.println(&crate::drivers::xhci::storage_diag());
                }
            }
        },
        "read" => {
            match args.first().and_then(|s| s.parse::<u64>().ok()) {
                Some(lba) => {
                    let mut buf = [0u8; 512];
                    match crate::drivers::block::read_block(lba, &mut buf) {
                        Ok(_) => {
                            console.println(&alloc::format!("LBA {}:", lba));
                            hexdump(console, &buf[0..128]);
                        }
                        Err(e) => console.println(&alloc::format!("read error: {:?}", e)),
                    }
                }
                None => console.println("usage: read <lba>"),
            }
        },
        "write" => {
            // JD5 overload: `write <lba> <byte>` (raw block write) IFF exactly two args parse as a
            // <u64 lba> <byte 0..=255> pair — byte-identical to the pre-JD5 behaviour. Any other
            // shape is a FILE write `write <path> <text...>` (create-or-truncate; text = the rest of
            // the line, whitespace-collapsed like `echo`). A numeric filename can still be reached as
            // `/NAME` (an absolute path never parses as an LBA).
            let raw = if args.len() == 2 {
                match (args[0].parse::<u64>().ok(), parse_byte(args[1])) {
                    (Some(lba), Some(b)) => Some((lba, b)),
                    _ => None,
                }
            } else {
                None
            };
            match raw {
                Some((lba, b)) => {
                    let buf = [b; 512];
                    match crate::drivers::block::write_block(lba, &buf) {
                        Ok(()) => console.println(&alloc::format!("wrote LBA {} (0x{:02x} x512)", lba, b)),
                        Err(e) => console.println(&alloc::format!("write error: {:?}", e)),
                    }
                }
                None if args.is_empty() =>
                    console.println("usage: write <path> <text>  |  write <lba> <byte>"),
                None => fs_write(console, args[0], args[1..].join(" ").as_bytes()),
            }
        },
        "netinfo" => {
            // PI-UI-3: the Pi (GENET) has no e1000, so the x86 path below reports "no device" there. Give
            // the Pi shell an equivalent that reads the GENET interface snapshot — MAC / IP / gateway /
            // lease state — plus the civil-clock sync state, matching the x86 verb's line shape.
            #[cfg(all(target_arch = "aarch64", not(feature = "genet")))]
            ui3_say(console, "netinfo", "No network device ready.");
            #[cfg(all(target_arch = "aarch64", feature = "genet"))]
            {
                match crate::arch::aarch64::genet::netinfo() {
                    Some(n) => {
                        ui3_say(console, "netinfo", &alloc::format!(
                            "NIC: MAC {}  link {}",
                            crate::drivers::e1000::fmt_mac(&n.mac),
                            if n.link_up { "UP" } else { "DOWN" }
                        ));
                        ui3_say(console, "netinfo", &alloc::format!(
                            "IP {}.{}.{}.{} ({})  GW {}.{}.{}.{}",
                            n.ip[0], n.ip[1], n.ip[2], n.ip[3],
                            if n.leased { "dhcp" } else { "static" },
                            n.gw[0], n.gw[1], n.gw[2], n.gw[3]
                        ));
                        let mut buf = [0u8; 24];
                        let sync = match crate::clock::iso8601_now(&mut buf) {
                            Some(len) => {
                                let iso = core::str::from_utf8(&buf[..len]).unwrap_or("<iso>");
                                match crate::clock::source() {
                                    crate::clock::ClockSource::Sntp { stratum } =>
                                        alloc::format!("{} (sntp, stratum {})", iso, stratum),
                                    crate::clock::ClockSource::Manual =>
                                        alloc::format!("{} (manual)", iso),
                                    crate::clock::ClockSource::Unset =>
                                        alloc::format!("unsynced"),
                                }
                            }
                            None => alloc::format!("unsynced"),
                        };
                        ui3_say(console, "netinfo", &alloc::format!("time: {}", sync));
                    }
                    None => ui3_say(console, "netinfo", "No network device ready."),
                }
            }
            #[cfg(not(target_arch = "aarch64"))]
            match crate::drivers::e1000::info() {
                Some(n) => {
                    console.println(&alloc::format!(
                        "NIC: MAC {}  link {}",
                        crate::drivers::e1000::fmt_mac(&n.mac),
                        if n.link_up { "UP" } else { "DOWN" }
                    ));
                    console.println(&alloc::format!(
                        "BAR0 {:#x}  RX frames: {}  TX frames: {}  IRQs: {}",
                        n.mmio_base, n.rx_count, n.tx_count, n.irq_count
                    ));
                    console.println(&alloc::format!("TCP listener (:7) active conns: {}", n.tcp_conns));
                    // SOCK-1 (knob-on): report the smoltcp interface riding the same NIC.
                    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
                    console.println(&crate::smolnet::info_line());
                }
                None => console.println("No network device ready."),
            }
        },
        "ping" => {
            match args.first().and_then(|s| parse_ipv4(s)) {
                Some(ip) => {
                    let count = args.get(1)
                        .and_then(|s| s.parse::<u16>().ok())
                        .unwrap_or(4)
                        .clamp(1, 16);
                    console.println(&alloc::format!(
                        "PING {}.{}.{}.{} ({} requests)", ip[0], ip[1], ip[2], ip[3], count));
                    // Blocks while it ARP-resolves the target and waits for each reply.
                    // SOCK-1 (knob-on): route through the smoltcp ICMP socket instead of the
                    // hand-rolled engine; the outcome shape + renderer below are unchanged.
                    let outcome = {
                        #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
                        { crate::smolnet::ping(ip, count) }
                        #[cfg(not(all(feature = "smolnet", target_arch = "x86_64")))]
                        { crate::drivers::e1000::ping(ip, count) }
                    };
                    match outcome {
                        Some(o) if o.resolved => {
                            let peer = o.mac
                                .map(|m| crate::drivers::e1000::fmt_mac(&m))
                                .unwrap_or_default();
                            console.println(&alloc::format!(
                                "{}/{} replies received (peer {})", o.received, o.sent, peer));
                        }
                        Some(_) => console.println("host unreachable (no ARP reply)"),
                        None => console.println("No network device ready."),
                    }
                }
                None => console.println("usage: ping <a.b.c.d> [count]"),
            }
        },
        "arp" => {
            match args.first().and_then(|s| parse_ipv4(s)) {
                Some(ip) => {
                    // SOCK-1 (knob-on): resolve via smoltcp's neighbor discovery (ARP-triggering
                    // poll) instead of the hand-rolled cache; the rendering below is unchanged.
                    let resolved = {
                        #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
                        { crate::smolnet::arp_resolve(ip) }
                        #[cfg(not(all(feature = "smolnet", target_arch = "x86_64")))]
                        { crate::drivers::e1000::arp_resolve(ip) }
                    };
                    match resolved {
                        Some(mac) => console.println(&alloc::format!(
                            "{}.{}.{}.{} is-at {}",
                            ip[0], ip[1], ip[2], ip[3], crate::drivers::e1000::fmt_mac(&mac))),
                        None => console.println("no ARP reply (host unreachable / no NIC)"),
                    }
                }
                None => console.println("usage: arp <a.b.c.d>"),
            }
        },
        "connect" => {
            let ip = args.first().and_then(|s| parse_ipv4(s));
            let port = args.get(1).and_then(|s| s.parse::<u16>().ok());
            match (ip, port) {
                (Some(ip), Some(port)) => {
                    // Optional message; if omitted, just open and immediately close.
                    let msg = if args.len() > 2 { args[2..].join(" ") } else { String::new() };
                    console.println(&alloc::format!(
                        "CONNECT {}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], port));
                    // Blocks while it ARP-resolves, handshakes, exchanges, and closes.
                    match crate::drivers::e1000::connect(ip, port, msg.as_bytes()) {
                        Some(o) if o.established => {
                            console.println(&alloc::format!(
                                "established; {} bytes received; closed={}", o.rx_len, o.closed));
                        }
                        Some(o) if !o.resolved => console.println("host unreachable (no ARP reply)"),
                        Some(_) => console.println("connection refused / no response"),
                        None => console.println("No network device ready."),
                    }
                }
                _ => console.println("usage: connect <a.b.c.d> <port> [message]"),
            }
        },
        "udpsend" => {
            let ip = args.first().and_then(|s| parse_ipv4(s));
            let port = args.get(1).and_then(|s| s.parse::<u16>().ok());
            match (ip, port) {
                (Some(ip), Some(port)) => {
                    let msg = if args.len() > 2 { args[2..].join(" ") } else { String::from("unaos-udp") };
                    console.println(&alloc::format!(
                        "UDP {}.{}.{}.{}:{} <- {:?}", ip[0], ip[1], ip[2], ip[3], port, msg));
                    match crate::drivers::e1000::udp_send(ip, port, msg.as_bytes()) {
                        Some(o) if o.sent => {
                            if o.replied {
                                console.println(&alloc::format!("reply: {} bytes", o.rx_len));
                            } else {
                                console.println("sent; no reply (UDP is best-effort)");
                            }
                        }
                        Some(_) => console.println("host unreachable (no ARP reply)"),
                        None => console.println("No network device ready."),
                    }
                }
                _ => console.println("usage: udpsend <a.b.c.d> <port> [message]"),
            }
        },
        "get" => {
            // Minimal HTTP/1.0 GET over the streaming TCP client: connect, send the request,
            // read the whole response until the server closes, and print it.
            match args.first().and_then(|s| parse_ipv4(s)) {
                Some(ip) => {
                    let port = args.get(1).and_then(|s| s.parse::<u16>().ok()).unwrap_or(80);
                    let path = if args.len() > 2 { String::from(args[2]) } else { String::from("/") };
                    let req = alloc::format!(
                        "GET {} HTTP/1.0\r\nHost: {}.{}.{}.{}\r\nConnection: close\r\n\r\n",
                        path, ip[0], ip[1], ip[2], ip[3]);
                    console.println(&alloc::format!(
                        "GET http://{}.{}.{}.{}:{}{}", ip[0], ip[1], ip[2], ip[3], port, path));
                    match crate::drivers::e1000::fetch(ip, port, req.as_bytes()) {
                        Some((o, body)) if o.established => {
                            console.println(&alloc::format!(
                                "--- {} bytes received; closed={} ---", o.rx_len, o.closed));
                            // Render printable ASCII; drop CR, keep LF as line breaks.
                            let text: String = body.iter().filter_map(|&b| match b {
                                b'\n' => Some('\n'),
                                b'\r' => None,
                                0x20..=0x7e => Some(b as char),
                                _ => Some('.'),
                            }).collect();
                            for line in text.split('\n') {
                                console.println(line);
                            }
                        }
                        Some((o, _)) if !o.resolved => console.println("host unreachable (no ARP reply)"),
                        Some(_) => console.println("connection refused / no response"),
                        None => console.println("No network device ready."),
                    }
                }
                None => console.println("usage: get <a.b.c.d> [port] [path]"),
            }
        },
        // The in-kernel 3D sculptor. Aarch64 only, matching `crate::vug`: the whole arm vanishes on
        // x86, where the verb is therefore an ordinary unrecognised word.
        #[cfg(target_arch = "aarch64")]
        "vug" => {
             match args.first().copied() {
                 Some("bebox") => {
                     console.println("Vug: BeBox tribute (press any key)...");
                     vug::run_bebox_mode(pal);
                     // Tribute screen stays up; `took_screen` keeps the console off it.
                 }
                 Some("wire") => {
                     console.println("Vug: sculpting the quartz (wireframe)...");
                     vug::run_crystal(pal, vug::Mode::Wire);
                     console.draw(pal); // clean exit: restore the shell over the demo
                 }
                 _ => {
                     console.println("Vug: sculpting the quartz (solid)...");
                     vug::run_crystal(pal, vug::Mode::Solid);
                     console.draw(pal); // clean exit: restore the shell over the demo
                 }
             }
        },
        // PI-APP-1: the V3D visible-battery app. Registered in the same launcher path as `vug` (a
        // shell-command match arm — the kernel's convention for a launchable UI program). It replays
        // the four visible V3D stages (gradient / animate / multi-primitive / blit) onto the live
        // framebuffer so the graphics are watchable while the system is up — the boot-time battery
        // flashes past before the monitor wakes. Reuses the state boot established (no GPU re-init);
        // the `:: V3D: app replay ... ::` witnesses go to serial for the bench. Aarch64+v3d only, so
        // the knob-off build is byte-identical (the whole arm vanishes with the feature).
        #[cfg(all(target_arch = "aarch64", feature = "v3d"))]
        "v3d" => {
            console.println("V3D: replaying the visible graphics battery (press any key)...");
            let n = crate::arch::aarch64::v3d::run_visible_battery_again();
            if n == 0 {
                // Block never came up this boot (QEMU / fail-closed) — nothing landed on screen, so
                // restore the console rather than leaving a blank take-screen.
                console.println("V3D: not available on this boot (GPU absent or bring-up skipped).");
                console.draw(pal);
            }
            // On a real replay `took_screen` keeps the console off the freshly-blitted tiles.
        },
        // UI1-M3's full-screen monitor draws through the same `vug` module, so it is gated the same
        // way. x86 keeps the per-core instrument it always had — the `ui_status` strip and `sched` —
        // and this verb is an unrecognised word there.
        #[cfg(target_arch = "aarch64")]
        "pulse" => {
            // UI1-M3: the full-screen system monitor (BeOS Pulse homage). Any key exits; the
            // console repaints over it on the way out (same contract as the vug crystal).
            console.println("Pulse: system monitor (press any key)...");
            vug::run_pulse(pal);
            console.draw(pal); // clean exit: restore the shell over the monitor
        },
        "tste" | "selftest" => {
            // The in-OS self-test suite (TSTE-1). Prints a three-section PASS/FAIL/SKIP table in the
            // console (like `ps` — it does NOT take the screen) and mirrors every line to serial.
            crate::selftest::run(console, pal);
        },
        "sched" | "ps" => {
            #[cfg(target_arch = "x86_64")]
            {
                let count = core::cmp::min(
                    crate::arch::acpi::cpu_count().max(1),
                    crate::arch::gdt::MAX_CPUS,
                );
                console.println("CPU  role  current  run-queue");
                for cpu in 0..count {
                    let role = if cpu == 0 { "bsp" } else { "ap " };
                    let cur = match crate::arch::sched::current_task_id(cpu) {
                        Some(id) => alloc::format!("tid {}", id),
                        None => "-".into(),
                    };
                    console.println(&alloc::format!(
                        "{:>3}  {}   {:<8} {}",
                        cpu, role, cur, crate::arch::sched::run_queue_len(cpu)
                    ));
                }
                console.println(&alloc::format!(
                    "demo tasks finished: {}", crate::arch::sched::demo_done()));
            }
            #[cfg(not(target_arch = "x86_64"))]
            console.println("sched: x86_64 only");
        },
        "burst" => {
            // ORIN-BURST: fire the SCHED-BAL multi-hot-thread burst live from the tegra shell so the
            // operator can watch vug/pulse light every Orin core, and repeat it at will. Runs inside the
            // jd2_console_pump task (pinned to the boot core), so it drives the burst from core 0: the
            // balancer PLACES the migratable PRIO_LOW busy tasks across the online cores and idle cores
            // steal the residual. PRIO_LOW keeps it below the console/render, so the shell stays live. The
            // descriptive per-core witness goes to serial (":: AARCH64 SCHED-BAL: ...") — the verb does
            // NOT take the screen, so `vug`/`pulse` can be watched in parallel and `burst` re-fired.
            #[cfg(target_arch = "aarch64")]
            {
                console.println("burst: staging 8 migratable busy tasks across the online cores...");
                crate::arch::sched::run_burst(0);
                console.println("burst: done (per-core witness on serial: ':: AARCH64 SCHED-BAL: ...')");
            }
            #[cfg(not(target_arch = "aarch64"))]
            console.println("burst: SCHED-BAL burst is aarch64 only");
        },
        "simmer" => {
            // SIMMER (R23s1): a per-core load animator. Stage one PINNED PRIO_LOW duty-cycling task on
            // every online core EXCEPT this (boot/console) core, each breathing on its own id-seeded
            // rhythm, so the vug per-core meter shows the cores rising and falling independently — "like
            // a moderately busy computer." Runs inside jd2_console_pump (the boot core); the animators
            // live on the secondary cores, which run the preemptive scheduler (so their sleeps cycle).
            // Toggle: bare `simmer` flips it on/off; `simmer off` stops it explicitly. The verb does NOT
            // take the screen, so start `simmer` then `vug` to watch the bars wander. Start/stop witness
            // on serial only (":: SIMMER: ... ::") — the visual is the product, so no per-cycle spam.
            #[cfg(target_arch = "aarch64")]
            {
                let off = args
                    .first()
                    .map(|a| *a == "off" || *a == "stop")
                    .unwrap_or(false);
                if off {
                    crate::arch::sched::simmer_stop();
                    console.println("simmer: stopped.");
                } else if crate::arch::sched::simmer_active() {
                    crate::arch::sched::simmer_stop();
                    console.println("simmer: stopped (toggle). Type 'simmer' to start it again.");
                } else {
                    crate::arch::sched::simmer_start(0);
                    console.println("simmer: per-core animators staged. Now type 'vug' to watch the cores breathe.");
                }
            }
            #[cfg(not(target_arch = "aarch64"))]
            console.println("simmer: per-core load animator is aarch64 only");
        },
        "top" => {
            // SCHED-2: per-core scheduler load table (aarch64). Recent busy% (rolling window),
            // cumulative context switches, and the last task dispatched on each core. On-demand read
            // of `sched::core_load` — introspection only, no scheduling-path effect.
            #[cfg(target_arch = "aarch64")]
            crate::arch::sched::load_table(|row| console.println(row));
            #[cfg(not(target_arch = "aarch64"))]
            console.println("top: aarch64 only");
        },
        "batmon" => {
            // NATIVE-MIDDEN M1b: one honest SMC battery line. A one-shot human command, so it does a
            // FRESH port-I/O read (snapshot(), unthrottled) rather than the cached boot-time snapshot
            // the shell never refreshes. Prints and returns — takes no screen, does no seam work.
            #[cfg(all(target_arch = "x86_64", feature = "smc"))]
            {
                // Honesty rule: every absent field prints the "-" sentinel; a `None` must NEVER render
                // as a number a reader could mistake for a real value (0 mA is a plausible amperage).
                // Mirrors the M2 witness sentinel shape at smc.rs:498-499.
                let snap = crate::drivers::smc::battery::snapshot();
                let fu = |o: Option<u16>| o.map(|v| alloc::format!("{}", v)).unwrap_or_else(|| "-".into());
                let fi = |o: Option<i16>| o.map(|v| alloc::format!("{}", v)).unwrap_or_else(|| "-".into());
                console.println(&alloc::format!(
                    "batt: present={} soc={}% volt={}mV amp={}mA full={}mAh rem={}mAh ac={}",
                    snap.present,
                    fu(snap.soc_pct),
                    fu(snap.volt_mv),
                    fi(snap.amp_ma),
                    fu(snap.full_mah),
                    fu(snap.rem_mah),
                    match snap.ac_present { Some(true) => "yes", Some(false) => "no", None => "-" },
                ));
            }
            #[cfg(not(all(target_arch = "x86_64", feature = "smc")))]
            console.println("batmon: SMC battery monitor is x86 UNAOS_SMC=1 only");
        },
        "bootlog" => {
            // GUI-WITNESS M2b: print the boot-milestone ring with timestamps — the operator's eyes at
            // the bench. On a GUI (non-usbdebug) build serial is silent and fbcon detached at the GUI
            // handoff, so this verb is the ONLY witness surface for whether PORTSW flipped, the FTDI
            // console armed (vs. failed), the EHCI HID / trackpad armed, and the block device came up.
            // Reads the same ring the serial dump shows. Snapshots under the ring lock then prints, so
            // console I/O never runs while the ring is held.
            let mut buf = [(0u64, ""); 32]; // matches bootlog::capacity()
            let n = crate::bootlog::snapshot(&mut buf);
            if n == 0 {
                console.println("bootlog: no boot milestones recorded");
            } else {
                console.println(&alloc::format!("bootlog: {} milestone(s) (oldest first):", n));
                for (ms, tag) in &buf[..n] {
                    console.println(&alloc::format!("  [{:>8} ms] {}", ms, tag));
                }
            }
        },
        "shutdown" | "off" => {
             // TODO: Create arch::shutdown()
             serial_println!("Shutdown requested");
             crate::hlt_loop();
             crate::hlt_loop();
        },
        #[cfg(any(all(feature = "baremetal", target_arch = "aarch64"), all(feature = "tegra_el0", target_arch = "aarch64"), target_arch = "x86_64"))] // EXECGATE: tegra_el0 joins (JETSON-EL0 widened the vectors/sched/syscall, never the shell)
        "bg" => {
            // BGRUN-1: run a user program in the BACKGROUND — the shell returns to its prompt at once and
            // the program keeps running (and, if windowed, its window stays OPEN, so TAB has a ring to
            // walk — this is what turns the WC-TAB binding into a workflow: `run` blocks until its app
            // dies, so real windows never coexisted before this verb). `bg <path>`.
            match args.first() {
                None => console.println("usage: bg <path>   (run an ELF64 user program in the background)"),
                Some(&path) => {
                    bg_program(console, path);
                }
            }
        },
        // STORM-X86 (Peter, Boot AL, rMBP): this arm was `#[cfg(feature = "baremetal")]` — a Pi-only
        // gate — while the `bg`/`jobs`/`kill` arms on either side of it had already been widened to
        // include x86. So `storm` at the x86 prompt fell through to "Unknown command" even though
        // every mechanism it drives (the bg path, the process table, the slot pool) is live there.
        // The gate now matches its neighbours exactly. `all(baremetal, aarch64)` is not a narrowing:
        // `baremetal` is only ever set by the Pi legs (see `arroyo`), so the aarch64 build selects
        // the identical arm it always did.
        #[cfg(any(all(feature = "baremetal", target_arch = "aarch64"), target_arch = "x86_64"))]
        "storm" => {
            // STORM-VERB (Peter, P77 sitting): launch a whole vug fleet in one command — `storm [n]`,
            // default 6, so an operator can raise a load storm without typing `bg /fat/VUG.ELF` six
            // times. Each launch is EXACTLY the bg path (same spawn, same job table, same messages);
            // this verb adds the loop and, since STORM-HEADROOM, the MEASUREMENT around the loop. It
            // still decides nothing about how a vug is spawned or where it is placed. Stops honestly
            // at the first failure — a partial fleet is reported as such, never rounded up.
            //
            // STORM-HEADROOM — WHAT ACTUALLY BOUNDS `n`. The sentence this replaces ("the job table
            // (8 slots) and PROCS-6's bg cap bound n") was true and useless: it named both bounds
            // without saying which one BITES. The one that bites is the PROCESS TABLE, always: the
            // job table is kept strictly above it and the user-slot pool keeps a 2-slot reserve
            // above THAT, so `syscall::proc_table_rows()` is the first ceiling a growing fleet
            // reaches, on either arch.
            //
            // THE CLAMP IS DERIVED, NOT WRITTEN DOWN. It admits `proc_table_rows() + 2` — two past
            // the ceiling, deliberately, and that margin is the whole design: an operator who asks
            // for more than the machine has must get a REFUSAL that names the resource, not a
            // silently-lowered request that reads as if it were granted. It used to be the literal
            // `8`, which was `6 + 2` at the time and became a lie the moment HEADROOM moved x86's
            // `MAX_PROCS` to 10 (a `storm 9` would have been quietly served as `storm 8`). Deriving
            // it means the margin, not the number, is what is maintained.
            //
            // WHAT THAT MEANS PER ARCH, recomputed:
            //   * **aarch64** — `MAX_PROCS = 6`, so the clamp admits 8 and the arithmetic is
            //     unchanged from STORM-X86: `storm 7`/`storm 8` cannot succeed as asked on ANY boot,
            //     the SEVENTH launch is refused by the process table, and the fleet that remains is
            //     the same fleet `storm 6` builds.
            //   * **x86** — `MAX_PROCS = 10`, so the clamp admits 12 and `storm 11`/`storm 12` are
            //     the requests that cannot succeed on an empty boot; the ELEVENTH launch is the one
            //     the process table refuses. On a `wc` boot the desktop app holds a row permanently,
            //     so the refusal arrives one earlier: the TENTH launch, i.e. `storm 9` is the
            //     largest fleet that completes as asked and `storm 8` is the largest that still
            //     leaves a row for a foreground `run`.
            // The cap is not a knob — see the `MAX_PROCS` block in `arch::syscall` for why 10 is
            // where the user-slot reserve puts it on x86, and treat moving it as an arc, not a
            // tuning step. (HEADROOM was that arc.)
            //
            // WHY THE CENSUS SITS HERE. Every scheduler quantity that could name a ceiling already
            // exists, but on other clocks: the `:: SCHED: load ::` train is timer-driven (~1 s
            // windows, metal-only) and `[fluid3]`/`[comp2]` ride the compositor. The seconds in which
            // a fleet is being BUILT are shorter than one of those windows, so the launch boundary is
            // the only clock that samples the machine at the instant its size changes. `pre` is taken
            // before the first launch, one `[storm] k=` line after each successful one, `post` after
            // the burst. That layout is what makes a WEDGE readable as well as a refusal: if the
            // fleet starves the shell, this verb stops printing, and the last `k=` on the wire names
            // the launch it stopped after. Its silence proves nothing on its own — and on x86 there
            // is NO instrument that survives a starved shell (the `[schedx86] load` heartbeat runs
            // on the same render-service task as this dispatch; on aarch64 the timer-driven
            // `:: SCHED: load ::` / `[pulse5]` / `[spin1]` train does survive). A silent x86 tail
            // is settled only by the next boot's slice, honestly.
            let n = args
                .first()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(6)
                .clamp(1, crate::arch::syscall::proc_table_rows() + 2);
            // STORM-FATW: `fat` anywhere in the args arms the USB-traffic writer (below). A bare
            // `storm fat` keeps the default fleet of 6 (the non-numeric arg falls through the parse).
            let fatw = args.iter().any(|a| *a == "fat");
            // Pre-flight resource census. The refusal string already names what ran out AT a refusal;
            // this names the headroom when there is NO refusal, which is the reading "storm 6 launched
            // 6/6" silently withholds. Taken before the burst so a partial fleet can be attributed to
            // rows that were already spoken for (live work, corpses awaiting `jobs`, or the PORPHANED
            // rows `jobs` can never reap) rather than to the cap.
            let rows = crate::arch::syscall::proc_table_rows();
            let (rows_free, rows_running, rows_exited, rows_orphaned) =
                crate::arch::syscall::proc_table_headroom();
            let (jobs_free, jobs_rows) = {
                let j = BG_JOBS.lock();
                (j.iter().filter(|s| s.is_none()).count(), j.len())
            };
            let slots_free = storm_slots::user_slots_free();
            console.println(&alloc::format!(
                "storm: n={} — {}/{} process rows free, {}/{} job rows, {}/{} user slots",
                n, rows_free, rows, jobs_free, jobs_rows,
                slots_free, storm_slots::USER_SLOTS
            ));
            serial_println!(
                ":: STORM: begin n={} | proc rows free={} running={} exited={} porphaned={} of {} | job rows free={}/{} | user slots free={}/{} ::",
                n, rows_free, rows_running, rows_exited, rows_orphaned, rows,
                jobs_free, jobs_rows, slots_free, storm_slots::USER_SLOTS
            );
            crate::arch::sched::storm_census("pre");
            let mut launched = 0usize;
            for _ in 0..n {
                if !bg_program(console, "/fat/VUG.ELF") {
                    // `bg_program` has already said WHY, but not uniformly on this wire: a SPAWN
                    // refusal also prints `:: BGRUN: bg … rejected (…)` to serial, while an
                    // IMAGE-READ failure (missing, empty or oversized /fat/VUG.ELF) is console-only.
                    // A serial-only capture would therefore be unable to tell the fleet ceiling from
                    // a bad card, which is exactly the confusion this arc exists to remove — so this
                    // line re-reads the census rather than pointing at a neighbour that may not be
                    // there. `free > 0` here means the table was NOT the limit.
                    let (f, r, e, o) = crate::arch::syscall::proc_table_headroom();
                    serial_println!(
                        ":: STORM: REFUSED at launch {} of {} — fleet stands at {} | proc rows free={} running={} exited={} porphaned={} | user slots free={} ::",
                        launched + 1, n, launched, f, r, e, o,
                        storm_slots::user_slots_free()
                    );
                    break;
                }
                launched += 1;
                crate::arch::sched::storm_probe(&alloc::format!("k={}/{}", launched, n));
            }
            console.println(&alloc::format!("storm: launched {}/{} vugs", launched, n));
            serial_println!(":: STORM: launched {}/{} vugs ::", launched, n);
            // Review must-fix (R23S1T): re-read the resource census UNCONDITIONALLY after the burst,
            // not only on refusal. `user_slots_free`'s whole reason to exist is "did the 2-slot
            // reserve survive a FULL fleet" — and the success path (storm 6, six launches, no
            // refusal) is exactly the state that question is about. Nor is it derivable:
            // `rows_free_before - launched` breaks the moment a vug faults mid-burst and leaves a
            // PEXITED row, and `[spread10]`'s slot histogram counts residency, not SLOT_USED.
            {
                let (f, r, e, o) = crate::arch::syscall::proc_table_headroom();
                serial_println!(
                    ":: STORM: end | proc rows free={} running={} exited={} porphaned={} of {} | user slots free={}/{} ::",
                    f, r, e, o, crate::arch::syscall::proc_table_rows(),
                    storm_slots::user_slots_free(), storm_slots::USER_SLOTS
                );
            }
            // `post` is taken immediately, so its busy percents still carry the pre-burst window —
            // that is intended: it is the ZERO MARK for the cumulative counters, against which the
            // next few timer-driven windows are read. The settled fleet is described by those lines,
            // not by this one.
            crate::arch::sched::storm_census("post");
            // STORM-FATW (Peter, R23 boot1 sitting): `storm [n] fat` also arms a kernel writer task
            // that drives bounded USB storage traffic UNDER the fleet — the missing half of the
            // WEDGE-8/F3 metal provocation (the fleet supplies preemption pressure; this supplies
            // the driver-claim traffic). See `storm_fat_writer` for the two legs and their honesty
            // rules. Armed AFTER the burst so the census lines above describe the fleet alone.
            #[cfg(target_arch = "aarch64")]
            if fatw {
                crate::arch::sched::spawn_auto("stormfatw", storm_fat_writer, 0);
                console.println("storm: fat writer armed — watch :: STORM: fatw lines on serial");
            }
            // STORM-X86: `fat` is PARSED on x86 and then REFUSED OUT LOUD. It is not silently
            // dropped, because a silently-ignored arg is the worst of the three options — the
            // operator gets the fleet, sees no `fatw` lines, and cannot tell "the writer ran and
            // found nothing" from "the writer never existed". The refusal names the reason and the
            // arch. `storm_fat_writer` stays where it is: it drives `BlockSource::Usb` through the
            // Pi's `fs::fat` masked-RMW path against the xHCI loan, an aarch64+`baremetal` provocation
            // (WEDGE-8/F3) with no x86 counterpart in this arc. Porting it is an arc of its own, not
            // a cfg widening — dragging it over here would compile a writer aimed at a driver claim
            // this arch does not hold, which is a fake instrument, not a feature.
            #[cfg(target_arch = "x86_64")]
            if fatw {
                console.println("storm: `fat` refused — the USB-writer provocation is aarch64/baremetal-only (fleet launched without it)");
                serial_println!(":: STORM: fatw REFUSED — aarch64/baremetal-only provocation, not ported to x86 ::");
            }
        },
        #[cfg(any(all(feature = "baremetal", target_arch = "aarch64"), target_arch = "x86_64"))]
        "jobs" => {
            // BGRUN-1: list background programs and REAP the exited ones (this verb is the reaper — a
            // PEXITED row stays claimed until it is polled here, and the table is bounded). `jobs`.
            bg_jobs(console);
        },
        #[cfg(any(all(feature = "baremetal", target_arch = "aarch64"), target_arch = "x86_64"))]
        "kill" => {
            // BGRUN-1: kill a background program by pid (SKILL-1 underneath — ASID-scoped, so ELF-2
            // sibling threads die with it; unconfirmed kills park the row PORPHANED and settle at the
            // task's next boundary). `kill <pid>`.
            match args.first().and_then(|s| s.parse::<u64>().ok()) {
                None => console.println("usage: kill <pid>   (see `jobs` for pids)"),
                Some(pid) => bg_kill_cmd(console, pid),
            }
        },
        // MIDDEN-M1: this arm is a DRIFT NET, and as of this arc it is unreachable — deliberately.
        //
        // It no longer means "unknown word": the core already ruled on that, and an unknown word
        // comes back as `Plan::Say(TerminalError)` and never arrives here. What could arrive is a
        // word midden's table calls a verb that THIS build does not compile the machinery for —
        // and that set is empty today, because every `Avail` in `HOST_VERBS` mirrors the `#[cfg]`
        // on its arm below exactly (the review checked all 78 spellings arm by arm). So the table
        // and the match agree, and nothing reaches this arm.
        //
        // It is kept because that agreement is a HAND-MAINTAINED invariant across two files. Add a
        // verb to `HOST_VERBS` and forget its arm, or `#[cfg]`-narrow an arm without narrowing its
        // `Avail`, and the drift lands HERE — as a sentence naming the verb, on the panel — rather
        // than as a word that silently does nothing. Deleting the arm would make that same drift a
        // non-exhaustive-match compile error only if the match were over an enum, and it is over
        // `&str`; there is no compiler check to fall back on. Hence: unreachable by construction,
        // retained as the net for the construction breaking.
        //
        // (No example is given on purpose. Every plausible one is wrong: `uls`/`top` on x86 are
        // `Avail::Aarch64`/an arm that prints its own aarch64-only message, and `vug`'s `Avail`
        // tracks the v3d cfg. If a real reachable case ever appears, it is a BUG in the table.)
        other => {
            console.println(&alloc::format!(
                "{}: not available on this build (the verb exists; this kernel does not carry it)",
                other
            ));
        }
    }

    took_screen
}

/// Print `data` as a classic hex dump (offset, 16 hex bytes, ASCII gutter) to the console.
fn hexdump(console: &mut Console, data: &[u8]) {
    for (i, chunk) in data.chunks(16).enumerate() {
        let mut line = alloc::format!("{:04x}: ", i * 16);
        for b in chunk {
            line.push_str(&alloc::format!("{:02x} ", b));
        }
        line.push_str(" |");
        for b in chunk {
            let c = if *b >= 32 && *b < 127 { *b as char } else { '.' };
            line.push(c);
        }
        line.push('|');
        console.println(&line);
    }
}

/// Parse a dotted-quad IPv4 address (`a.b.c.d`) into 4 octets. Rejects anything that
/// isn't exactly four decimal octets in 0..=255.
fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut count = 0usize;
    for part in s.split('.') {
        if count >= 4 {
            return None;
        }
        octets[count] = part.parse::<u8>().ok()?;
        count += 1;
    }
    if count == 4 {
        Some(octets)
    } else {
        None
    }
}

/// Parse a byte literal in decimal or `0x..` hex form.
fn parse_byte(s: &str) -> Option<u8> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u8::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u8>().ok()
    }
}

/// JD19: parse a `u64` offset/length accepting decimal or `0x`-hex (the `xd` off/len args).
fn parse_num(s: &str) -> Option<u64> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u64>().ok()
    }
}

// --- SHELL-WRITE: unified VFS write surface (`vfs <op> <path> [text]`) ---------
//
// The FIRST consumer of the VFS-2 write surface. Each invocation builds the
// process namespace fresh (stateless, like the FAT verbs — a swapped card is
// picked up on the next command) and drives the `MountTable` create / write /
// truncate / unlink verbs. The shell is the trusted operator console, so it
// writes as the kernel-authority principal (`KERNEL_PRINCIPAL`) — the same
// posture the `u*` native verbs record. Namespace: native UnaFS at `/`, the FAT
// boot partition at `/fat`, and the hot-plugged USB FAT stick at `/usb` when
// present (VFS-3, read-only). Both real backends are aarch64-only (the x86 build
// has neither the unafs module nor a `VfsBackend for FatBackend` impl), so the
// x86 arm is an honest "unsupported on this arch" line.

/// Build the shell's VFS namespace: native UnaFS at `/`, FAT boot partition at
/// `/fat`, and — when a USB stick is enumerated — the hot-plugged USB FAT at
/// `/usb` (VFS-3). World-readable is a READ posture only (it does not confer
/// write); the shell writes as `KERNEL_PRINCIPAL`, which both backends authorize.
///
/// VFS-3: the `/usb` mount is bound only when the stick is actually present
/// (`mount_source(Usb)` succeeds) — the presence check at build time is the
/// honest hot-plug posture (doc §6): absent → `/usb` is simply not in the table,
/// so a `/usb/...` path falls through to the native root and resolves to a clean
/// `-ENOENT`, never a panic. The USB volume is read through the xHCI `Usb` source
/// and is **writable** since USB-WRITE F3, which routed the `Usb` arm to the
/// verified BOT WRITE(10) path and retired PIUSB-27's blanket refusal. Whether a
/// `vfs write|append|rm|mkdir` at `/usb` is admitted is decided by exactly one
/// predicate — `fat::BlockSource::write_veto`, which `FatBackend::read_only`
/// forwards to — so this note cannot drift from the code again the way its
/// PIUSB-27 predecessor did. Rebuilt per invocation, so a stick
/// hot-plugged (or ejected) between commands is picked up on the next `vfs`.
/// EXEC-1: `run <path>` — load an ELF64 (or flat) user program off the VFS namespace and execute it in user mode,
/// reporting its exit status. Reads the whole file through the same `MountTable` the `vfs` verb uses,
/// bounds it to the kernel's 16 KiB user window (an oversize file is rejected with a clear message — never
/// silently truncated), pre-checks the ELF64 magic + aarch64 machine for an early operator-friendly reason,
/// then hands the bytes to the kernel loader `run_user_image`, which maps them into a fresh per-task slot
/// (per-segment W^X pages) and runs them under user mode + the fault-kill net. The kernel is the security
/// authority: this pre-check only sharpens the error text; `run_user_image` re-validates from scratch.
///
/// Witness (headless-capturable): `:: EXEC: run <path> — loaded <n> bytes, entry 0x<..>, exit=<code> ::`.
/// EXEC-1/BGRUN-1: the shared read-and-precheck front of `run` and `bg` — stat + bound + read off the
/// VFS, then the friendly ELF64/aarch64 pre-check (the kernel loader is the real gate; a flat blob
/// passes through to the position-independent flat path). `verb` names the caller in every message.
///
/// # X86RUN (GR20) — why this is `baremetal`-OR-x86 rather than `baremetal`
///
/// `run`/`bg`/`jobs`/`kill` were written during the Pi arcs and their whole family — verbs, helpers,
/// job table — carried `#[cfg(feature = "baremetal")]`, the **Pi-4 bare-metal** feature. On an x86
/// build they were therefore absent from `dispatch_command`'s match and fell through to "Unknown
/// command", so there was no operator-facing way to start a ring-3 program on the rMBP at all.
///
/// Nothing about that gate was load-bearing. The x86 kernel half was BUILT for these verbs and has
/// been shipping for arcs: `arch::x86_64::syscall` carries `run_user_image`, `spawn_user_image_bg`,
/// `bg_poll` and `bg_kill` (the WINX-2 twins of the aarch64 entries), whose own doc comments name
/// "the synchronous shell `run <path>` entry", "the shell's `bg <path>` entry" and "run `jobs` to
/// reap exited jobs". `arroyo` and `scripts/make-fat-img.sh` stage `STAT.ELF` / `VUG.ELF` /
/// `PULSE.ELF` onto the x86 DATA volume with the comment "for `run`/`bg`", and the `esp-x86`
/// operator text already warns that a mis-staged stick makes ``bg /fat/VUG.ELF`` report `-ENOENT`.
/// The gate was an oversight of provenance, not a dependency.
///
/// TWO things genuinely differ on x86, and both are handled by the twin below rather than by
/// forcing the aarch64 body to compile:
///
/// * **There is no VFS namespace.** `impl VfsBackend for FatBackend` and `NativeBackend` are
///   `#[cfg(target_arch = "aarch64")]` (see `fs/vfs.rs`), which is why `vfs_cmd` already refuses on
///   x86. So the x86 twin reads through the FAT-direct path the `cat` verb uses
///   (FATVERB: `mount_read_volume` + `resolve_path` + the JD4 cwd — the same program-source handle
///   this loader binds), which is the path Peter's `cat hello.txt`
///   demonstrably exercises. `/fat/NAME` is accepted as an alias for that volume's root, because it
///   is the form the packaging text tells the operator to type and the only FAT volume x86 mounts
///   IS the DATA volume.
/// * **The machine check.** The aarch64 body pre-checks `e_machine == 183`; x86 wants
///   `EM_X86_64 = 62`. The kernel loader (`arch::x86_64::elf::validate_elf`) re-checks from scratch
///   either way — this pre-check only sharpens the operator's error text.
#[cfg(all(any(feature = "baremetal", feature = "tegra_el0"), target_arch = "aarch64"))] // EXECGATE: the aarch64 twin serves tegra_el0 too
fn read_el0_image(console: &mut Console, verb: &str, path: &str) -> Option<alloc::vec::Vec<u8>> {
    use crate::fs::vfs::NodeKind;
    // Cap = the kernel user window; a file at or under it may still be rejected by the loader (a flat blob
    // is re-bounded to one code page), but this is the hard read ceiling — we never read past it.
    const CAP: u64 = crate::arch::aarch64::boot::USER_REGION_SIZE as u64;
    let mt = vfs_mount_table();
    let st = match mt.stat(path) {
        Ok(s) => s,
        Err(e) => {
            console.println(&alloc::format!("{}: {}: {}", verb, path, vfs_err(e)));
            return None;
        }
    };
    if matches!(st.kind, NodeKind::Dir) {
        console.println(&alloc::format!("{}: {}: is a directory (-EISDIR)", verb, path));
        return None;
    }
    if st.size == 0 {
        console.println(&alloc::format!("{}: {}: empty file", verb, path));
        return None;
    }
    if st.size > CAP {
        console.println(&alloc::format!(
            "{}: {}: {} bytes exceeds the {}-byte user window (-E2BIG)",
            verb, path, st.size, CAP
        ));
        return None;
    }
    let bytes = match mt.read(path, 0, st.size as usize) {
        Ok(b) => b,
        Err(e) => {
            console.println(&alloc::format!("{}: {}: {}", verb, path, vfs_err(e)));
            return None;
        }
    };
    if bytes.len() >= 20 && bytes[0..4] == [0x7F, b'E', b'L', b'F'] {
        if bytes[4] != 2 {
            console.println(&alloc::format!("{}: {}: not an ELF64 image (EI_CLASS != 2)", verb, path));
            return None;
        }
        if bytes[5] != 1 {
            console.println(&alloc::format!("{}: {}: not little-endian (EI_DATA != 1)", verb, path));
            return None;
        }
        let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
        if machine != 183 {
            console.println(&alloc::format!(
                "{}: {}: not an aarch64 image (e_machine {} != 183)", verb, path, machine
            ));
            return None;
        }
    }
    Some(bytes)
}

/// X86RUN (GR20): the x86 twin of `read_el0_image` — same contract, same message shapes, same
/// "every check can say NO" discipline, over the only file surface this arch has.
///
/// x86 has no VFS namespace (`fs/vfs.rs` gates both backend impls to aarch64, which is why `vfs_cmd`
/// refuses here), so this reads through the FAT-direct path `cat` uses: the PROGRAM SOURCE
/// (FATVERB — `cat` now binds `mount_program_source` through `mount_read_volume`, exactly as this
/// loader does), which on x86 is the USB mass-storage DATA volume, or the internal SD card on a
/// machine booted from the reader — never the UEFI boot volume the kernel cannot reach — resolved
/// through the JD4 cwd exactly like every other file verb.
///
/// **`/fat` is accepted as an alias for that volume's root.** It is the form the packaging text tells
/// the operator to type (`esp-x86` prints "…or `bg /fat/VUG.ELF` reports -ENOENT"), the form
/// `scripts/make-fat-img.sh` documents for the staged `STAT.ELF`/`VUG.ELF`, and it costs nothing to
/// honour because x86 mounts exactly one FAT volume. Both `run /fat/VUG.ELF` and `run VUG.ELF` (or
/// `run /VUG.ELF`) therefore reach the same file, and the witness reports the CANONICAL on-disk path
/// so a capture never has to guess which spelling was typed.
#[cfg(target_arch = "x86_64")]
fn read_el0_image(console: &mut Console, verb: &str, path: &str) -> Option<alloc::vec::Vec<u8>> {
    // LAUNCHPACE: time each storage phase of a program launch (mount → directory walk → cluster-chain
    // read) so a bench capture can CONVICT where a launch stall actually lives, rather than infer it
    // from the coarse `BGRUN`/`BAREXEC` "loaded N bytes" line — that line prints only after all three
    // phases and breaks out none of them. The operator's "~1 s pause when I start vug or pulse" runs
    // exactly this path, and the three sub-costs have very different fixes (a re-mount cache, a
    // directory-index cache, a batched read), so the witness has to separate them before any of them
    // is touched. Emitted once, on the SUCCESSFUL read, in `now_cycles()` (rdtsc) units converted to µs.
    let t_entry = crate::arch::now_cycles();
    // Cap = the ring-3 window the loader will map into; a file at or under it may still be rejected by
    // the loader, but this is the hard read ceiling — we never read past it.
    let cap = crate::arch::syscall::user_window_size();
    // `/fat/NAME` -> `/NAME` (see the doc note). Case-insensitive, because FAT short names are.
    let rel = {
        let p = path;
        if p.len() >= 4 && p[..4].eq_ignore_ascii_case("/fat") {
            match p.as_bytes().get(4) {
                None => "/",              // a bare `/fat` IS the volume root
                Some(b'/') => &p[4..],    // `/fat/VUG.ELF` -> `/VUG.ELF`
                Some(_) => p,             // `/fatty.bin` is a real name, not the alias
            }
        } else {
            p
        }
    };
    // APPLOAD: `mount_program_source` — this function's ENTIRE job is finding an executable, which is
    // the one question the global handle alone cannot answer on a machine booted from the internal SD
    // reader. `bg /fat/VUG.ELF` reported `-ENOENT` there while the card was mounted and the file
    // listed. `/fat` stays the alias for "the one FAT volume this arch mounts"; which handle serves
    // that volume is now the block layer's decision, not this call site's assumption.
    let fs = match crate::fs::fat::mount_program_source() {
        Ok(fs) => fs,
        Err(e) => {
            console.println(&alloc::format!(
                "{}: no FAT filesystem ({:?}; handles={})",
                verb, e, crate::drivers::block::source_census()
            ));
            return None;
        }
    };
    let t_mount = crate::arch::now_cycles();
    let de = match resolve_path(&fs, &normalize_path(&cwd_path(), rel)) {
        Ok(Resolved::Root) => {
            console.println(&alloc::format!("{}: {}: is a directory (-EISDIR)", verb, path));
            return None;
        }
        Ok(Resolved::Entry(de, _canon)) => de,
        Err(msg) => {
            console.println(&alloc::format!("{}: {}", verb, msg));
            return None;
        }
    };
    let t_resolve = crate::arch::now_cycles();
    if de.is_dir {
        console.println(&alloc::format!("{}: {}: is a directory (-EISDIR)", verb, path));
        return None;
    }
    if de.size == 0 {
        console.println(&alloc::format!("{}: {}: empty file", verb, path));
        return None;
    }
    if de.size as usize > cap {
        console.println(&alloc::format!(
            "{}: {}: {} bytes exceeds the {}-byte user window (-E2BIG)",
            verb, path, de.size, cap
        ));
        return None;
    }
    let mut bytes = alloc::vec::Vec::new();
    if let Err(e) = fs.read_file(&de, &mut bytes, cap) {
        console.println(&alloc::format!("{}: {}: read failed ({:?}, -EIO)", verb, path, e));
        return None;
    }
    let t_read = crate::arch::now_cycles();
    if bytes.len() != de.size as usize {
        // FATREAD-1 was exactly this class of silent mismatch (a doubled read that pushed
        // STAT.ELF/VUG.ELF past the window). Say NO out loud rather than hand the loader a short or
        // long image and let it report an unrelated reason.
        console.println(&alloc::format!(
            "{}: {}: short read — {} of {} bytes (-EIO)", verb, path, bytes.len(), de.size
        ));
        return None;
    }
    if bytes.len() >= 20 && bytes[0..4] == [0x7F, b'E', b'L', b'F'] {
        if bytes[4] != 2 {
            console.println(&alloc::format!("{}: {}: not an ELF64 image (EI_CLASS != 2)", verb, path));
            return None;
        }
        if bytes[5] != 1 {
            console.println(&alloc::format!("{}: {}: not little-endian (EI_DATA != 1)", verb, path));
            return None;
        }
        let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
        if machine != 62 {
            // 62 = EM_X86_64. An aarch64 image (183) staged on x86 media lands here with a reason an
            // operator can act on, instead of the loader's bare "not EM_X86_64".
            console.println(&alloc::format!(
                "{}: {}: not an x86-64 image (e_machine {} != 62)", verb, path, machine
            ));
            return None;
        }
    }
    // LAUNCHPACE: the storage-phase breakdown for this launch. `mount_us` is the per-launch FAT re-mount
    // (sector-0 read + MBR/GPT decode + BPB parse — suspect: repeated every launch, never cached);
    // `dirwalk_us` is the root-directory scan that `resolve_path` runs to find the entry; `read_us` is
    // the cluster-chain read of the image itself (the MULTIBLK batched path). `total_us` is the whole
    // synchronous cost this launch imposes on its caller's core — and on x86 the shell runs on the
    // RENDER core (`x86_render_service`), so this total is time the panel is not composing. The window
    // create and first present that follow are on the app's own core and carry their own `wc-a`/`wc-h`
    // witnesses; this line owns the storage half.
    serial_println!(
        "[launchpace] verb={} bytes={} mount_us={} dirwalk_us={} read_us={} total_us={}",
        verb,
        bytes.len(),
        cyc_to_us(t_mount.saturating_sub(t_entry)),
        cyc_to_us(t_resolve.saturating_sub(t_mount)),
        cyc_to_us(t_read.saturating_sub(t_resolve)),
        cyc_to_us(t_read.saturating_sub(t_entry)),
    );
    Some(bytes)
}

/// LAUNCHPACE: rdtsc cycle delta → microseconds, at the rate `apic::calibrate` measured against the
/// ACPI PM timer. Mirrors `video::wcg::cycles_to_us` (private there); the fallback rate matches, so a
/// witness printed before calibration is honest-but-approximate rather than a divide-by-zero. x86 only,
/// because the only caller ([`read_el0_image`]'s x86 twin) is.
#[cfg(target_arch = "x86_64")]
fn cyc_to_us(dt: u64) -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    let hz = if hz == 0 { 1_250_000_000 } else { hz };
    dt.saturating_mul(1_000_000) / hz
}

#[cfg(any(all(feature = "baremetal", target_arch = "aarch64"), target_arch = "x86_64"))]
fn run_program(console: &mut Console, path: &str) {
    let Some(bytes) = read_el0_image(console, "run", path) else {
        return;
    };
    // Hand the bytes to the kernel loader: map into a fresh user slot, run co-located, wait (bounded 5 s) for
    // the program to exit or fault. The image length + entry are reported for the witness.
    let n = bytes.len();
    // The two `run_user_image`s take the SAME 5-second bound in different units, deliberately and
    // documented on both sides: aarch64's twin takes a CNTPCT span (`timer::cntfrq()` = 1 s), x86's
    // takes plain milliseconds ("the arch-neutral unit the shell now passes").
    #[cfg(target_arch = "aarch64")]
    let deadline = 5 * crate::arch::aarch64::timer::cntfrq();
    #[cfg(target_arch = "x86_64")]
    let deadline: u64 = 5_000;
    match crate::arch::syscall::run_user_image("shell-run", &bytes, deadline) {
        Ok((outcome, entry)) => {
            use crate::arch::syscall::RunOutcome;
            match outcome {
                RunOutcome::Exited(code) => {
                    console.println(&alloc::format!("run: {}: exited with status {}", path, code));
                    serial_println!(
                        ":: EXEC: run {} — loaded {} bytes, entry {:#x}, exit={} ::",
                        path, n, entry, code
                    );
                }
                RunOutcome::Faulted => {
                    console.println(&alloc::format!(
                        "run: {}: killed by the fault-kill net (contained fault)", path
                    ));
                    serial_println!(
                        ":: EXEC: run {} — loaded {} bytes, entry {:#x}, exit=FAULT ::",
                        path, n, entry
                    );
                }
                // CLOSE-CLEAN exists only on aarch64: the x86 `RunOutcome` (WINX-2) has three
                // variants, with no window-close arm, so this one is arch-gated rather than faked.
                #[cfg(target_arch = "aarch64")]
                RunOutcome::Closed => {
                    // CLOSE-CLEAN: the operator closed the window; the program's exit is clean.
                    console.println(&alloc::format!("run: {}: closed (window close box)", path));
                    serial_println!(
                        ":: EXEC: run {} — loaded {} bytes, entry {:#x}, exit=CLOSED ::",
                        path, n, entry
                    );
                }
                RunOutcome::Timeout => {
                    console.println(&alloc::format!("run: {}: did not exit within the deadline", path));
                    serial_println!(
                        ":: EXEC: run {} — loaded {} bytes, entry {:#x}, exit=TIMEOUT ::",
                        path, n, entry
                    );
                }
            }
        }
        Err(why) => {
            console.println(&alloc::format!("run: {}: {}", path, why));
            serial_println!(":: EXEC: run {} — rejected ({}) ::", path, why);
        }
    }
}

/// BGRUN-1: one shell-side background job. The PATH is copied (bounded) so `jobs` can name it — the
/// kernel row carries only the fixed task name. The pid is the durable key; asid rides for `kill`.
#[cfg(any(all(feature = "baremetal", target_arch = "aarch64"), target_arch = "x86_64"))]
#[derive(Clone, Copy)]
struct BgJob {
    pid: u64,
    asid: u64,
    name: [u8; 32],
    nlen: u8,
}

/// BGRUN-1: the shell's job table. Bounded like every table in this kernel. LENS CORRECTION
/// (round 1): the binding resource is the Proc table (`MAX_PROCS`), not the user address-space
/// slots — so the real ceiling is `MAX_PROCS - 1` bg jobs alongside one foreground `run`, and this
/// table's full arm is reachable only if MAX_PROCS grows past it. Rows are kept strictly ABOVE the
/// Proc cap on purpose (harmless headroom): it is what makes an untrackable job impossible rather
/// than merely unlikely.
///
/// HEADROOM — 8 -> 12 rows, because x86's `MAX_PROCS` went 6 -> 10 and 8 rows would no longer have
/// been strictly above it. That is the whole reason for the raise; the arm still cannot be reached.
/// The table is arch-neutral and so is the raise: aarch64 keeps `MAX_PROCS = 6`, so there it simply
/// adds four rows that can never be claimed. The per-arch ceilings are now:
///   * **x86** — 9 bg jobs with no foreground `run` (8 with one), see the eviction note below.
///   * **aarch64** — 6 bg jobs with no foreground `run` (5 with one), unchanged by this arc.
///
/// ⚠ KERNEL-APPS EVICTION — **on a `wc` x86 boot the ceiling is one lower than a bare reading of
/// `MAX_PROCS` says.** `video::wcx::desktop_app_service` launches the desktop app (`STAT.ELF`) at
/// boot and it never exits, so it permanently holds one Proc row, one user slot, one `wm` window and
/// one row of THIS table. With `MAX_PROCS = 10` the operator's budget on such a boot is therefore
/// **9** bg jobs with no foreground `run` (8 with one) — which is exactly the fleet HEADROOM was
/// sized for: `storm 8` plus the desktop app, with a row still free for a foreground `run`.
///
/// ⚠ **Shell access is NO LONGER single-task.** It used to be, and this note used to say so while
/// keeping the Mutex "explicit rather than relying on it". The eviction added a second caller on a
/// DIFFERENT core: `adopt_bg_job` runs from the device-service task (`x86_usb_pump`, service core)
/// while the shell runs in `x86_render_service` (render core). The Mutex is now load-bearing — do
/// not drop it. No live hazard today (the service-core call happens once, at boot, before an
/// operator can type `jobs`), and cross-core contention on a raw `spin::Mutex` is bounded spin that
/// progresses — the SCHED-X86 deadlock rule is about two preemptible takers on ONE core. But note
/// that `bg_jobs` holds this lock across `console.println`, which on a `wc` build routes through the
/// compositor: a future second cross-core caller could spin for the length of a repaint.
#[cfg(any(all(feature = "baremetal", target_arch = "aarch64"), target_arch = "x86_64"))]
static BG_JOBS: spin::Mutex<[Option<BgJob>; 12]> = spin::Mutex::new([None; 12]);

/// STORM-FATW: the bounded USB-traffic writer `storm [n] fat` arms — the driver-claim half of the
/// WEDGE-8/F3 metal provocation (the vug fleet is the preemption half). Two legs, decided once at
/// start by whether the stick carries a mountable FAT volume:
///
/// * **usb-fat leg** (a FAT16/32 stick is in): the FULL F3 chain — `create_in_root` /
///   `write_grow` appends / periodic `delete_located`, all on `BlockSource::Usb`, so every round
///   runs the masked FAT/dir RMW spans against the xHCI loan exactly as user `SYS_WRITE` does.
/// * **raw-scratch leg** (no FAT volume — the honest fallback, and it SAYS so): bounded
///   read/write/verify rounds on the last-but-one LBA via `read_block_usb`/`write_block_usb`,
///   original sector restored at the end (the `mission_write_selftest` RMW-restore discipline).
///   This still contends for the claim under the fleet, but the masked-RMW leg is NOT exercised —
///   the begin line names that, so a green run cannot be over-read.
///
/// Honesty rules: `Busy`/`-EAGAIN` outcomes are counted and reported as `busy=` — under WEDGE-8
/// they are the fix WORKING (pre-fix, that contention was the silent dead core). Ten consecutive
/// hard I/O errors abort the run rather than burn the remaining rounds on a dead device. Bounded
/// (300 rounds, one `hlt` breather each) so an armed writer always ends and reports. Re-arming
/// while one runs just adds a second contender — noisy but bounded, and contention is the point.
#[cfg(feature = "baremetal")]
fn storm_fat_writer(_: usize) {
    use crate::fs::fat::BlockSource;
    const ROUNDS: u32 = 300;
    const REDELETE_EVERY: u32 = 64; // exercise the dir-RMW delete/create cycle, and cap file growth
    let mut ok = 0u32;
    let mut busy = 0u32;
    let mut io = 0u32;
    let mut io_run = 0u32; // consecutive hard errors — the abort counter
    let fat_leg = crate::fs::fat::mount_source(BlockSource::Usb);
    serial_println!(
        ":: STORM: fatw begin rounds={} leg={} ::",
        ROUNDS,
        if fat_leg.is_ok() { "usb-fat (full F3 chain: masked RMW vs xHCI loan)" }
        else { "raw-scratch (no FAT volume on the stick — masked-RMW leg NOT exercised)" }
    );
    let mut pattern = [0u8; 512];
    match fat_leg {
        Ok(fs) => {
            const NAME: &str = "STORMW.TMP";
            // (de-fields we carry between rounds: dir slot + chain head + size)
            let mut cur: Option<(u64, usize, u32, u32)> = None; // (dir_lba, dir_off, first_cluster, size)
            for r in 0..ROUNDS {
                for (i, b) in pattern.iter_mut().enumerate() {
                    *b = (r as u8) ^ (i as u8);
                }
                let step: Result<(), crate::fs::fat::FatError> = (|| {
                    if cur.is_none() {
                        let (de, lba, off) = match fs.find_located(NAME) {
                            Ok(t) => t,
                            Err(crate::fs::fat::FatError::NotFound) => fs.create_in_root(NAME, 0x20)?,
                            Err(e) => return Err(e),
                        };
                        cur = Some((lba, off, de.first_cluster(), de.size));
                    }
                    let (lba, off, first, size) = cur.unwrap();
                    if r % REDELETE_EVERY == REDELETE_EVERY - 1 {
                        fs.delete_located(lba, off, first)?;
                        cur = None;
                        return Ok(());
                    }
                    let (_w, new_size, new_first) = fs.write_grow(first, size, lba, off, size, &pattern)?;
                    cur = Some((lba, off, new_first, new_size));
                    Ok(())
                })();
                match step {
                    Ok(()) => { ok += 1; io_run = 0; }
                    Err(crate::fs::fat::FatError::Busy) => { busy += 1; io_run = 0; }
                    Err(_) => {
                        io += 1;
                        io_run += 1;
                        cur = None; // re-resolve the slot next round — the volume may have moved under us
                        if io_run >= 10 {
                            serial_println!(":: STORM: fatw ABORT at r={} — 10 consecutive I/O errors ::", r);
                            break;
                        }
                    }
                }
                if r % 50 == 49 {
                    serial_println!(":: STORM: fatw r={}/{} ok={} busy={} io={} ::", r + 1, ROUNDS, ok, busy, io);
                }
                crate::hlt();
            }
            // Leave the volume clean: best-effort delete of the scratch file.
            if let Some((lba, off, first, _)) = cur {
                let _ = fs.delete_located(lba, off, first);
            }
            serial_println!(
                ":: STORM: fatw done leg=usb-fat ok={} busy={} io={} (busy>0 with no freeze = WEDGE-8 witnessed live) ::",
                ok, busy, io
            );
        }
        Err(_) => {
            let Some(dev) = crate::drivers::block::usb_info() else {
                serial_println!(":: STORM: fatw done leg=none — no USB block device enumerated; nothing exercised ::");
                return;
            };
            if dev.num_blocks < 4 {
                serial_println!(":: STORM: fatw done leg=none — device too small for a scratch sector ::");
                return;
            }
            let scratch = dev.num_blocks - 2;
            let mut orig = [0u8; 512];
            if crate::drivers::block::read_block_usb(scratch, &mut orig).is_err() {
                serial_println!(":: STORM: fatw done leg=none — scratch LBA {} unreadable; nothing exercised ::", scratch);
                return;
            }
            let mut verify = [0u8; 512];
            for r in 0..ROUNDS {
                for (i, b) in pattern.iter_mut().enumerate() {
                    *b = (r as u8) ^ (i as u8);
                }
                let step: Result<(), crate::drivers::block::BlockError> = (|| {
                    crate::drivers::block::write_block_usb(scratch, &pattern)?;
                    let n = crate::drivers::block::read_block_usb(scratch, &mut verify)?;
                    if n < verify.len() || verify != pattern {
                        return Err(crate::drivers::block::BlockError::Io);
                    }
                    Ok(())
                })();
                match step {
                    Ok(()) => { ok += 1; io_run = 0; }
                    Err(crate::drivers::block::BlockError::Busy) => { busy += 1; io_run = 0; }
                    Err(_) => {
                        io += 1;
                        io_run += 1;
                        if io_run >= 10 {
                            serial_println!(":: STORM: fatw ABORT at r={} — 10 consecutive I/O errors ::", r);
                            break;
                        }
                    }
                }
                if r % 50 == 49 {
                    serial_println!(":: STORM: fatw r={}/{} ok={} busy={} io={} ::", r + 1, ROUNDS, ok, busy, io);
                }
                crate::hlt();
            }
            // RMW-restore: put the original sector back, and say whether that succeeded.
            let restored = crate::drivers::block::write_block_usb(scratch, &orig).is_ok();
            serial_println!(
                ":: STORM: fatw done leg=raw-scratch lba={} ok={} busy={} io={} restored={} (masked-RMW leg NOT exercised — no FAT volume) ::",
                scratch, ok, busy, io, restored
            );
        }
    }
}

/// BGRUN-1: `bg <path>` — read the image, spawn it detached, record the job. The shell prompt is
/// back the moment this returns; the program (and its window, if it creates one) keeps running.
#[cfg(any(all(feature = "baremetal", target_arch = "aarch64"), target_arch = "x86_64"))]
fn bg_program(console: &mut Console, path: &str) -> bool {
    let Some(bytes) = read_el0_image(console, "bg", path) else {
        return false;
    };
    let n = bytes.len();
    match crate::arch::syscall::spawn_user_image_bg(&bytes) {
        Ok((pid, asid, entry)) => {
            let mut jobs = BG_JOBS.lock();
            let Some(slot) = jobs.iter_mut().find(|s| s.is_none()) else {
                // The kernel row exists but the shell can no longer track it; kill it rather than
                // leak an untrackable job (`jobs` could never reap what it never recorded).
                drop(jobs);
                let why = crate::arch::syscall::bg_kill(pid, asid);
                console.println(&alloc::format!(
                    "bg: {}: job table full — spawned pid {} was killed ({})",
                    path, pid, why
                ));
                return false;
            };
            let mut name = [0u8; 32];
            let nlen = path.len().min(32);
            name[..nlen].copy_from_slice(&path.as_bytes()[..nlen]);
            *slot = Some(BgJob { pid, asid, name, nlen: nlen as u8 });
            console.println(&alloc::format!("bg: {} started — pid {} (see `jobs`)", path, pid));
            serial_println!(
                ":: BGRUN: bg {} — loaded {} bytes, entry {:#x}, pid={} asid={} DETACHED ::",
                path, n, entry, pid, asid
            );
            true
        }
        Err(why) => {
            console.println(&alloc::format!("bg: {}: {}", path, why));
            serial_println!(":: BGRUN: bg {} — rejected ({}) ::", path, why);
            false
        }
    }
}

/// BGRUN-1: `jobs` — list background jobs and reap the exited ones. This is the SOLE reaper for
/// bg rows: an exited job's kernel row stays claimed (PEXITED) until it is polled here.
#[cfg(any(all(feature = "baremetal", target_arch = "aarch64"), target_arch = "x86_64"))]
fn bg_jobs(console: &mut Console) {
    use crate::arch::syscall::BgPoll;
    let mut jobs = BG_JOBS.lock();
    let mut any = false;
    for slot in jobs.iter_mut() {
        let Some(job) = *slot else { continue };
        any = true;
        let name = core::str::from_utf8(&job.name[..job.nlen as usize]).unwrap_or("?");
        // LOCK-ACROSS-REAP (lens should-fix, written down): this holds the BG_JOBS spinlock across
        // `bg_poll(reap=true)` -> `done.wait()`. Safe because the reap arm runs ONLY on a row observed
        // PEXITED, and SYS_EXIT posts the `done` permit strictly before publishing PEXITED — so the
        // wait takes the count>0 fast path and cannot park under the held lock. The one state where
        // that permit assumption fails (PORPHANED, the round-1 deadlock) never reaches the reap arm.
        match crate::arch::syscall::bg_poll(job.pid, true) {
            BgPoll::Running => {
                console.println(&alloc::format!("  pid {:>3}  running  {}", job.pid, name));
            }
            BgPoll::Exited(code) => {
                console.println(&alloc::format!(
                    "  pid {:>3}  exited {} (reaped)  {}", job.pid, code, name
                ));
                serial_println!(":: BGRUN: jobs — pid={} exit={} reaped ::", job.pid, code);
                *slot = None;
            }
            BgPoll::Faulted => {
                console.println(&alloc::format!(
                    "  pid {:>3}  FAULTED (contained; reaped)  {}", job.pid, name
                ));
                serial_println!(":: BGRUN: jobs — pid={} exit=FAULT reaped ::", job.pid);
                *slot = None;
            }
            // CLOSE-CLEAN is an aarch64-only `BgPoll` variant (the x86 WINX-2 enum has four:
            // Running / Exited / Faulted / Gone), so this arm is arch-gated rather than invented.
            #[cfg(target_arch = "aarch64")]
            BgPoll::Closed => {
                // CLOSE-CLEAN: the operator clicked the window's close box — a clean, asked-for
                // exit. Reads like a normal completed job, never like a fault.
                console.println(&alloc::format!(
                    "  pid {:>3}  closed (reaped)  {}", job.pid, name
                ));
                serial_println!(":: BGRUN: jobs — pid={} exit=CLOSED reaped ::", job.pid);
                *slot = None;
            }
            BgPoll::Gone => {
                // Row already gone (e.g. a kill reaped it, or PORPHANED settled and was reclaimed by
                // the exit path). Drop the stale entry honestly.
                console.println(&alloc::format!("  pid {:>3}  gone  {}", job.pid, name));
                *slot = None;
            }
        }
    }
    if !any {
        console.println("jobs: none");
    }
    // The per-job lines above are panel-only except when they REAP (those already carry a witness).
    // This one line makes the verb itself visible in a headless capture — otherwise a `jobs` with
    // nothing to reap is indistinguishable on the wire from a keystroke that never arrived. Counted
    // off the guard already held: `BG_JOBS` is a plain spinlock and re-locking here would deadlock.
    let remaining = jobs.iter().flatten().count();
    drop(jobs);
    serial_println!(":: BGRUN: jobs — {} tracked job(s) after the sweep ::", remaining);
}

/// BGRUN-1: `kill <pid>` — kill a recorded background job (SKILL-1 underneath).
///
/// PROCREAP — this doc used to say "the row is reaped by the next `jobs`; the table entry stays until
/// then", which contradicted the code three lines below it (which drops the entry) and, on x86, was the
/// live half of the Boot AJ lockout: the x86 `bg_kill` only MARKED the row `PEXITED`, `jobs` is the
/// sole reaper and reaps only what is still in `BG_JOBS`, so dropping the entry here orphaned the row
/// for the rest of the boot. Three kills, three rows gone, no recovery short of reboot.
///
/// The premise is now TRUE on both arches: a CONFIRMED kill reaps the row IN PLACE inside `bg_kill`
/// (aarch64 has since round 1; x86 as of this arc), so the entry dropped here names a row that is
/// already free, and the operator's record of the outcome is THIS line rather than an uninformative
/// `gone` from a later `jobs`. The witness carries the table accounting so a boot PROVES the transition
/// instead of implying it.
#[cfg(any(all(feature = "baremetal", target_arch = "aarch64"), target_arch = "x86_64"))]
fn bg_kill_cmd(console: &mut Console, pid: u64) {
    let jobs = BG_JOBS.lock();
    let Some(job) = jobs.iter().flatten().find(|j| j.pid == pid).copied() else {
        console.println(&alloc::format!("kill: no background job with pid {} (see `jobs`)", pid));
        // Mirrored so a headless capture can tell a REFUSED kill from a keystroke that never landed.
        serial_println!(":: BGRUN: kill pid={} — no such background job ::", pid);
        return;
    };
    drop(jobs); // bg_kill yields while confirming; never hold the table lock across that.
    let verdict = crate::arch::syscall::bg_kill(job.pid, job.asid);
    console.println(&alloc::format!("kill: pid {}: {}", pid, verdict));
    if verdict.starts_with("killed") {
        // The kernel reaped the row; the shell's entry is now the only stale handle. Drop it.
        let mut jobs = BG_JOBS.lock();
        for slot in jobs.iter_mut() {
            if matches!(slot, Some(j) if j.pid == pid) {
                *slot = None;
            }
        }
        drop(jobs);
        // PROCREAP: the accounting is the point. A kill that claims to have freed a row and did not is
        // exactly the defect this arc closes, and the old line ("kill pid=N — killed") could not tell
        // the two apart on the wire. Read the table AFTER the reap: the free count must rise by one per
        // kill, and a boot that launches and kills repeatedly must never see it drift down.
        let (free, _running, _exited, _orphaned) = crate::arch::syscall::proc_table_headroom();
        serial_println!(
            ":: BGRUN: kill pid={} — {} ({}/{} free) ::",
            pid, verdict, free, crate::arch::syscall::proc_table_rows()
        );
    } else {
        serial_println!(":: BGRUN: kill pid={} — {} ::", pid, verdict);
    }
}

/// BARE-EXEC (GR20): record a job in the shell's table under an arbitrary display name.
///
/// `bg_program` records the path it was handed; `bare_exec` records the CANONICAL on-disk path it
/// resolved (so `jobs` shows `/VUG.ELF` after the operator typed `vug.elf`), which is why the table
/// insert lives here rather than inline in one of them.
///
/// Returns false if the table is full; the caller must then say so rather than imply a live handle —
/// an untracked job is one `jobs` could never reap and `kill` could never name.
///
/// Note what this does NOT change: the kernel `Proc` row and its `KillSwitch` are registered by
/// `spawn_user_image_bg` itself, so `bg_kill`/`bg_poll` can always reach the pid. Only the shell's
/// NAME for it comes from here.
///
/// `pub(crate)` since the kernel-apps eviction, for ONE second caller:
/// [`crate::video::wcx::desktop_app_service`], which launches the desktop app at boot with nobody at
/// the prompt to type `bg`. Registering it here is what keeps `jobs` and `kill` TRUTHFUL —
/// `bg_kill_cmd` resolves a pid through this table and REFUSES one it cannot find, so an
/// unregistered launch would be a running ring-3 program the operator can neither list nor stop.
#[cfg(target_arch = "x86_64")]
pub(crate) fn adopt_bg_job(pid: u64, slot: u64, name: &str) -> bool {
    let mut jobs = BG_JOBS.lock();
    let Some(free) = jobs.iter_mut().find(|s| s.is_none()) else {
        return false;
    };
    let mut buf = [0u8; 32];
    let nlen = name.len().min(32);
    buf[..nlen].copy_from_slice(&name.as_bytes()[..nlen]);
    *free = Some(BgJob { pid, asid: slot, name: buf, nlen: nlen as u8 });
    true
}

/// BARE-EXEC (GR20, x86): run a program by TYPING ITS NAME — `vug.elf` at the prompt starts
/// `VUG.ELF` off the FAT volume, in a window, with the prompt back immediately.
///
/// MIDDEN-M1: called from `dispatch_command`'s `Plan::Exec` arm, which `midden_core` produces only
/// after `is_verb` said no. **Precedence is therefore absolute and needs no tie-break rule — a known
/// verb always wins.** A file called `LS` on the volume is unreachable this way and that is correct;
/// `run ./LS` names it explicitly.
///
/// # Resolution
///
/// `midden_core::resolve_exec` decides which SPELLING to load and hands it here as `name`; `typed`
/// is what the user actually wrote, and every message quotes the one the reader recognises. The
/// core tries the exact token first, then — and only for a bare leaf carrying no extension — the
/// `.elf` suffix, so **`vug` finds `VUG.ELF`** with nobody typing the extension and without `ls`
/// hiding it.
///
/// **On x86, `name` is NOT the on-disk spelling** and no caller may assume it is. `FatVolume`'s
/// `is_file` walks FAT with `eq_ignore_ascii_case`, so the core's `vug.elf` probe matches the
/// on-disk `VUG.ELF` and the core returns the string `"vug.elf"` — the resolver's upper-cased arm
/// never fires here (it is there for the fixture's exact-match `NameList` and for a future
/// case-sensitive `Volume`). The genuine on-disk 8.3 spelling appears one step below, as `canon`,
/// which the re-resolve reads out of the directory entry; that is the name the serial refusal
/// lines quote. Beneath that, this function performs the SAME resolution `cat` uses, verbatim:
/// `fs::fat::mount_program_source()` (the handle the block layer names as the one executables live
/// on — the USB mass-storage DATA volume, or the internal SD card when that is what booted us) +
/// `normalize_path` against
/// the JD4 cwd + `resolve_path`. That walk matches components with `eq_ignore_ascii_case`, so
/// `vug.elf` already found `VUG.ELF` before the elision existed; the elision is what makes the
/// *bare* `vug` work. `cd DOCS` then a bare name works for the same reason, since the cwd is
/// applied first.
///
/// # Why background, and how the operator stops it
///
/// Launched through `spawn_user_image_bg` — the same call `winx8_launcher` makes — so a windowed
/// program's window STAYS OPEN and the prompt returns. Unlike that witness, nothing kills it: a
/// user-typed launch runs until it exits or the operator stops it with `kill <pid>`. The pid is
/// printed on the launch line and the job is recorded in the shell's table, so `jobs` lists it and
/// `kill` can reach it.
///
/// # Silence vs. refusal — where the boundary is
///
/// The core established that `name` is a real file before building `Plan::Exec`, so from here on
/// this function OWNS the reply and every exit says why: not-an-ELF, wrong-arch, too-big,
/// unreadable and spawn-rejected are all named. The one remaining `false` path is a volume that
/// changed under us between the core's probe and this read (a card pulled mid-command) — an honest
/// race, reported as such rather than as a typo. Nothing may end in silence.
///
/// The loader is untouched: the bytes go to `spawn_user_image_bg` exactly as `bg` sends them, so the
/// per-segment W^X mapping, the ring-3 window bound and the fault-kill net are the same ones CFU-2's
/// write gate is built on. This adds a way to CALL the loader, never a way to relax it.
#[cfg(target_arch = "x86_64")]
fn bare_exec(console: &mut Console, typed: &str, name: &str) -> bool {
    // --- re-resolve the core's answer over the live volume ---------------------------------------
    // The core probed through this same mount a moment ago; re-resolving costs one walk and closes
    // the window where the card changed underneath. A miss here is a RACE, not a typo, and says so.
    // LAUNCH-AR: the PROGRAM SOURCE, matching `FatVolume::is_file` above and `read_el0_image`
    // below. All three legs of a bare-name launch — probe, re-resolve, read — now bind the same
    // handle; the Boot AR failure was exactly what happens when they do not.
    let Ok(fs) = crate::fs::fat::mount_program_source() else {
        console.println(&alloc::format!("{}: the volume went away before it could be started", typed));
        serial_println!(":: BAREXEC: {} (typed '{}') — REFUSED: volume vanished after resolution ::", name, typed);
        return false;
    };
    let canon = match resolve_path(&fs, &normalize_path(&cwd_path(), name)) {
        Ok(Resolved::Entry(de, canon)) if !de.is_dir => canon,
        _ => {
            console.println(&alloc::format!("{}: {} went away before it could be started", typed, name));
            serial_println!(":: BAREXEC: {} (typed '{}') — REFUSED: resolved name no longer a file ::", name, typed);
            return false;
        }
    };
    // --- loud from here: the name is a real file, so we owe an outcome --------------------------
    // Every refusal below is mirrored to serial as well as the panel. The panel line is what the
    // operator reads; the serial line is what a headless capture reads, and without it a bench log
    // could not tell "refused, and here is why" from "the keystroke never arrived".
    let Some(bytes) = read_el0_image(console, typed, name) else {
        // read_el0_image named the reason on the panel (size / arch / read error).
        serial_println!(":: BAREXEC: {} — REFUSED at the image read/pre-check (see the panel line) ::", canon);
        return true;
    };
    if !(bytes.len() >= 4 && bytes[0..4] == [0x7F, b'E', b'L', b'F']) {
        // The loader would otherwise take this down its FLAT-blob path and try to execute a text
        // file as code. Contained (ring 3, fault-kill net) but useless and confusing, so refuse
        // here: a bare name launches ELF programs, nothing else. `run <path>` still reaches the flat
        // path for anyone who means it.
        console.println(&alloc::format!(
            "{}: not an executable (no ELF64 magic) — a bare name runs ELF programs only", typed
        ));
        serial_println!(
            ":: BAREXEC: {} (typed '{}') — REFUSED: not an ELF64 image (no magic); {} bytes read ::",
            canon, typed, bytes.len()
        );
        return true;
    }
    // The ELF64 / little-endian / EM_X86_64 pre-checks already ran inside `read_el0_image`, which
    // named any of them; the kernel loader re-validates from scratch regardless.
    let n = bytes.len();
    match crate::arch::syscall::spawn_user_image_bg(&bytes) {
        Ok((pid, slot, entry)) => {
            if !adopt_bg_job(pid, slot, &canon) {
                // Spawned but untrackable — kill it rather than leave a job `jobs` could never reap
                // and `kill` could never name. Same rule `bg` follows, same reason.
                let why = crate::arch::syscall::bg_kill(pid, slot);
                console.println(&alloc::format!(
                    "{}: job table full — spawned pid {} was killed ({})", typed, pid, why
                ));
                serial_println!(
                    ":: BAREXEC: {} — job table full, pid={} killed ({}) ::", canon, pid, why
                );
                return true;
            }
            console.println(&alloc::format!(
                "{}: started — pid {} (`jobs` lists it, `kill {}` stops it)", canon, pid, pid
            ));
            serial_println!(
                ":: BAREXEC: {} (typed '{}') — loaded {} bytes, entry {:#x}, pid={} slot={} DETACHED, left RUNNING ::",
                canon, typed, n, entry, pid, slot
            );
            true
        }
        Err(why) => {
            console.println(&alloc::format!("{}: {}", typed, why));
            serial_println!(":: BAREXEC: {} — rejected ({}) ::", canon, why);
            true
        }
    }
}

#[cfg(target_arch = "aarch64")]
fn vfs_mount_table() -> crate::fs::vfs::MountTable {
    use crate::fs::vfs::{FatBackend, MountTable, NativeBackend, KERNEL_PRINCIPAL};
    let mut mt = MountTable::new();
    mt.mount("/", alloc::boxed::Box::new(NativeBackend::new("native")));
    mt.mount("/fat", alloc::boxed::Box::new(FatBackend::new("fat", KERNEL_PRINCIPAL, true)));
    // VFS-3: bind the USB stick at /usb only when it is present (honest hot-plug).
    if crate::fs::fat::mount_source(crate::fs::fat::BlockSource::Usb).is_ok() {
        mt.mount("/usb", alloc::boxed::Box::new(FatBackend::new_usb("usb", KERNEL_PRINCIPAL)));
    }
    mt
}

/// Render a `VfsError` as an errno-style operator line, matching the shell's
/// `-ENOENT`/`-EISDIR` house style.
#[cfg(target_arch = "aarch64")]
fn vfs_err(e: crate::fs::vfs::VfsError) -> String {
    use crate::fs::vfs::VfsError::*;
    match e {
        NoSuchVolume => String::from("no such volume"),
        NoSuchPath => String::from("no such file or directory (-ENOENT)"),
        NotADirectory => String::from("not a directory (-ENOTDIR)"),
        IsADirectory => String::from("is a directory (-EISDIR)"),
        Denied => String::from("permission denied (-EACCES)"),
        Unsupported => String::from("operation not supported on this volume (-ENOTSUP)"),
        Backend(s) => alloc::format!("backend error: {}", s),
    }
}

/// Panel-line + `:: vfsw: <line> ::` serial mirror (the `ui3_say` idiom, dedicated
/// tag) — the verb renders panel-only on the bench, so the witness gives a headless
/// capture the same content.
#[cfg(target_arch = "aarch64")]
fn vfs_say(console: &mut Console, line: &str) {
    console.println(line);
    serial_println!(":: vfsw: {} ::", line);
}

/// VFS-4: the namespace prefixes the shell's `vfs` verb reserves for DISTINCT
/// backing volumes that may be absent. A mutating verb aimed at one of these when
/// it is NOT currently mounted must report "volume not mounted" — never fall
/// through to the native root, which mis-reports a bare `-ENOENT`. On the P44
/// sitting a `vfs write /usb/…` with the stick's FAT unreadable (its READ(10)
/// LBA0 returned all-zeros with a passing CSW, so `mount_source(Usb)` honestly
/// found no FAT and `/usb` never bound) fell through to native-root create,
/// which failed resolving the parent `/usb` as a native path and said
/// "no such file or directory (-ENOENT)". That misdirection cost bench time; the
/// honest answer is that the *volume* is not mounted. `/` (native) is excluded —
/// it is always mounted and is the legitimate fall-through for un-prefixed paths.
#[cfg(target_arch = "aarch64")]
const RESERVED_VOLUME_PREFIXES: &[&str] = &["/usb", "/fat"];

/// VFS-4: if `path` targets a reserved volume prefix (see
/// [`RESERVED_VOLUME_PREFIXES`]) that is not present in the live `mounted`
/// prefix set, return that prefix. Boundary-matched exactly as the resolver is
/// (§4): `/usb` and `/usb/…` name the volume, but `/usbfoo` does NOT (it is a
/// native-root name and legitimately resolves there).
#[cfg(target_arch = "aarch64")]
fn unmounted_reserved_volume(mounted: &[&str], path: &str) -> Option<&'static str> {
    for &pfx in RESERVED_VOLUME_PREFIXES {
        let claims = path == pfx
            || (path.len() > pfx.len()
                && path.starts_with(pfx)
                && path.as_bytes()[pfx.len()] == b'/');
        if claims && !mounted.iter().any(|m| *m == pfx) {
            return Some(pfx);
        }
    }
    None
}

/// SHELL-WRITE dispatcher: `vfs <write|append|rm|mkdir> <path> [text ...]`.
#[cfg(target_arch = "aarch64")]
fn vfs_cmd(console: &mut Console, args: &[&str]) {
    use crate::fs::vfs::{NodeKind, VfsError, KERNEL_PRINCIPAL};
    let op = match args.first() {
        Some(&o) => o,
        None => {
            console.println("usage: vfs <write|append|rm|mkdir> <path> [text ...]");
            // FATVERB: not "(read-only)" — USB-WRITE F3 made the stick writable; see `diskinfo`.
            console.println("  namespace: / = native UnaFS, /fat = FAT boot partition, /usb = USB stick");
            return;
        }
    };
    let path = match args.get(1) {
        Some(&p) => p,
        None => {
            console.println(&alloc::format!("usage: vfs {} <path> [text ...]", op));
            return;
        }
    };
    let mt = vfs_mount_table();
    // VFS-4: a mutating verb aimed at a reserved volume that is not mounted (the
    // USB stick absent, or its FAT unreadable so `mount_source(Usb)` failed and
    // `/usb` never bound) must say so plainly — NOT fall through to the native
    // root and mis-report `-ENOENT` (the P44 misdirection). Applies to every op
    // uniformly, before dispatch.
    if let Some(vol) = unmounted_reserved_volume(&mt.prefixes(), path) {
        vfs_say(console, &alloc::format!(
            "vfs {}: {}: volume {} not mounted (-ENODEV)", op, path, vol));
        return;
    }
    let principal = KERNEL_PRINCIPAL;
    match op {
        "write" => {
            // Create-or-overwrite: replace an existing file's contents wholesale.
            // We unlink-then-create rather than truncate-to-0 because the native
            // backend has no in-place shrink primitive (truncate to 0 on a
            // non-empty native file is `Unsupported` by design) — replace works on
            // both backends. A directory target is refused up front.
            if let Ok(st) = mt.stat(path) {
                if matches!(st.kind, NodeKind::Dir) {
                    vfs_say(console, &alloc::format!("vfs write: {}: is a directory (-EISDIR)", path));
                    return;
                }
            }
            let mut data = args[2..].join(" ").into_bytes();
            data.push(b'\n');
            let _ = mt.unlink(path, principal); // drop the old file if present
            if let Err(e) = mt.create(path, NodeKind::File, principal) {
                vfs_say(console, &alloc::format!("vfs write: {}: {}", path, vfs_err(e)));
                return;
            }
            match mt.write(path, 0, &data, principal) {
                Ok(n) => vfs_say(console, &alloc::format!("vfs write: {}: wrote {} bytes", path, n)),
                Err(e) => vfs_say(console, &alloc::format!("vfs write: {}: {}", path, vfs_err(e))),
            }
        }
        "append" => {
            let offset = match mt.stat(path) {
                Ok(st) if matches!(st.kind, NodeKind::Dir) => {
                    vfs_say(console, &alloc::format!("vfs append: {}: is a directory (-EISDIR)", path));
                    return;
                }
                Ok(st) => st.size, // append at the current EOF
                Err(VfsError::NoSuchPath) => {
                    if let Err(e) = mt.create(path, NodeKind::File, principal) {
                        vfs_say(console, &alloc::format!("vfs append: {}: {}", path, vfs_err(e)));
                        return;
                    }
                    0
                }
                Err(e) => {
                    vfs_say(console, &alloc::format!("vfs append: {}: {}", path, vfs_err(e)));
                    return;
                }
            };
            let mut data = args[2..].join(" ").into_bytes();
            data.push(b'\n');
            match mt.write(path, offset, &data, principal) {
                Ok(n) => vfs_say(console, &alloc::format!(
                    "vfs append: {}: wrote {} bytes at offset {}", path, n, offset)),
                Err(e) => vfs_say(console, &alloc::format!("vfs append: {}: {}", path, vfs_err(e))),
            }
        }
        "rm" => match mt.unlink(path, principal) {
            Ok(()) => vfs_say(console, &alloc::format!("vfs rm: {}: removed", path)),
            Err(e) => vfs_say(console, &alloc::format!("vfs rm: {}: {}", path, vfs_err(e))),
        },
        "mkdir" => match mt.create(path, NodeKind::Dir, principal) {
            Ok(_) => vfs_say(console, &alloc::format!("vfs mkdir: {}: created", path)),
            Err(VfsError::Backend("exists")) =>
                vfs_say(console, &alloc::format!("vfs mkdir: {}: already exists (-EEXIST)", path)),
            Err(e) => vfs_say(console, &alloc::format!("vfs mkdir: {}: {}", path, vfs_err(e))),
        },
        other => console.println(&alloc::format!(
            "vfs: unknown op '{}' (write|append|rm|mkdir)", other)),
    }
}

/// x86 has no writable VFS backend (no unafs module, no `VfsBackend for FatBackend`
/// impl), so the unified write surface is aarch64-only. Honest refusal on x86.
#[cfg(not(target_arch = "aarch64"))]
fn vfs_cmd(console: &mut Console, _args: &[&str]) {
    console.println("vfs: unified VFS write surface is aarch64-only (no writable backend on this arch)");
}

// --- BeFS-K4 native unafs write verbs -----------------------------------------
// The native unafs volume uses ABSOLUTE, case-sensitive paths (no shell cwd).
// Every verb routes through the single coherent mount (`crate::fs::unafs::
// with_unafs`), so the in-RAM allocation bitmap/journal stay authoritative and
// each mutation is write-through + durable. Kept in this dedicated region (not
// among the FAT-verb helpers above) so the pi4 unafs lane stays trivially
// separable from the concurrent jetson FAT-verb work.

/// Split an absolute unafs path into `(parent_dir, leaf)`. Rejects the bare
/// root (nothing to create/remove). `"/A.TXT"` -> `("/", "A.TXT")`; `"/D/A"` ->
/// `("/D", "A")`; a bare `"A"` is treated as root-relative -> `("/", "A")`.
#[cfg(target_arch = "aarch64")]
fn unafs_split(path: &str) -> Option<(&str, &str)> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some(("/", &trimmed[1..])),
        Some(i) => Some((&trimmed[..i], &trimmed[i + 1..])),
        None => Some(("/", trimmed)),
    }
}

/// `utouch <path>`: create a 0-length file (error if it exists / parent missing).
#[cfg(target_arch = "aarch64")]
fn unafs_verb_touch(path: &str) -> String {
    let (parent, leaf) = match unafs_split(path) {
        Some(pl) => pl,
        None => return alloc::format!("utouch: {}: invalid path", path),
    };
    match crate::fs::unafs::with_unafs(|fs| {
        let pid = fs.resolve_path(parent).map_err(|e| alloc::format!("{:?}", e))?;
        fs.create_file(pid, leaf.into())
            .map(|_| ())
            .map_err(|e| alloc::format!("{:?}", e))
    }) {
        Ok(Ok(())) => alloc::format!("utouch: created {}", path),
        Ok(Err(msg)) => alloc::format!("utouch: {}: {}", path, msg),
        Err(e) => alloc::format!("utouch: no unafs volume ({:?})", e),
    }
}

/// `uwrite <path> <text>`: create-or-replace a file with `bytes` (durable).
#[cfg(target_arch = "aarch64")]
fn unafs_verb_write(path: &str, bytes: &[u8]) -> String {
    let (parent, leaf) = match unafs_split(path) {
        Some(pl) => pl,
        None => return alloc::format!("uwrite: {}: invalid path", path),
    };
    let n = bytes.len();
    match crate::fs::unafs::with_unafs(|fs| {
        let pid = fs.resolve_path(parent).map_err(|e| alloc::format!("{:?}", e))?;
        // Replace semantics: drop an existing FILE of this name first (a
        // directory is left intact, and create_file then reports FileExists).
        let _ = fs.unlink(pid, leaf);
        let id = fs
            .create_file(pid, leaf.into())
            .map_err(|e| alloc::format!("{:?}", e))?;
        fs.write_data(id, 0, bytes)
            .map_err(|e| alloc::format!("{:?}", e))
    }) {
        Ok(Ok(())) => alloc::format!("uwrite: wrote {} bytes to {}", n, path),
        Ok(Err(msg)) => alloc::format!("uwrite: {}: {}", path, msg),
        Err(e) => alloc::format!("uwrite: no unafs volume ({:?})", e),
    }
}

/// `umkdir <path>`: create a directory.
#[cfg(target_arch = "aarch64")]
fn unafs_verb_mkdir(path: &str) -> String {
    let (parent, leaf) = match unafs_split(path) {
        Some(pl) => pl,
        None => return alloc::format!("umkdir: {}: invalid path", path),
    };
    match crate::fs::unafs::with_unafs(|fs| {
        let pid = fs.resolve_path(parent).map_err(|e| alloc::format!("{:?}", e))?;
        fs.mkdir(pid, leaf.into())
            .map(|_| ())
            .map_err(|e| alloc::format!("{:?}", e))
    }) {
        Ok(Ok(())) => alloc::format!("umkdir: created {}/", path),
        Ok(Err(msg)) => alloc::format!("umkdir: {}: {}", path, msg),
        Err(e) => alloc::format!("umkdir: no unafs volume ({:?})", e),
    }
}

/// `urm <path>`: delete a file (a directory is refused with IsADirectory).
#[cfg(target_arch = "aarch64")]
fn unafs_verb_rm(path: &str) -> String {
    let (parent, leaf) = match unafs_split(path) {
        Some(pl) => pl,
        None => return alloc::format!("urm: {}: invalid path", path),
    };
    match crate::fs::unafs::with_unafs(|fs| {
        let pid = fs.resolve_path(parent).map_err(|e| alloc::format!("{:?}", e))?;
        fs.unlink(pid, leaf)
            .map(|_| ())
            .map_err(|e| alloc::format!("{:?}", e))
    }) {
        Ok(Ok(())) => alloc::format!("urm: removed {}", path),
        Ok(Err(msg)) => alloc::format!("urm: {}: {}", path, msg),
        Err(e) => alloc::format!("urm: no unafs volume ({:?})", e),
    }
}

/// `usnap <name>`: retain the current committed tree as a snapshot. The shell
/// is a kernel-authority surface, so the creator principal is "kernel"
/// (owner-or-kernel destructive authority — a later `usnapdrop` from this
/// surface is always permitted). Returns the generation stamp.
#[cfg(target_arch = "aarch64")]
fn unafs_verb_snap(name: &str) -> String {
    let ts = crate::arch::timer::cntpct();
    match crate::fs::unafs::with_unafs(|fs| {
        fs.snapshot_create(name.into(), "kernel".into(), ts)
            .map_err(|e| alloc::format!("{:?}", e))
    }) {
        Ok(Ok(generation)) => alloc::format!("usnap: retained '{}' (generation {})", name, generation),
        Ok(Err(msg)) => alloc::format!("usnap: {}: {}", name, msg),
        Err(e) => alloc::format!("usnap: no unafs volume ({:?})", e),
    }
}

/// `usnapdrop <generation>`: drop a retained snapshot; reclamation drains
/// eagerly, freeing only blocks no live/retained root still reaches.
#[cfg(target_arch = "aarch64")]
fn unafs_verb_snapdrop(generation: u64) -> String {
    match crate::fs::unafs::with_unafs(|fs| {
        fs.snapshot_drop(generation)
            .map_err(|e| alloc::format!("{:?}", e))
    }) {
        Ok(Ok(())) => alloc::format!("usnapdrop: dropped generation {} (blocks reclaimed)", generation),
        Ok(Err(msg)) => alloc::format!("usnapdrop: generation {}: {}", generation, msg),
        Err(e) => alloc::format!("usnapdrop: no unafs volume ({:?})", e),
    }
}

/// `usnapls <gen> [path]`: list a retained snapshot's directory AS OF the
/// snapshot (K8c) — a read-only [`SnapshotView`] listing; never perturbs the
/// live tree, refcounts, or the reclaim queue. Gated by the SAME current-ACL
/// evaluator as `usnapcat` ([`read_authz`] on the target directory's live id) —
/// no snapshot-read surface bypasses it: a directory deleted from the live tree
/// fails closed, symmetrically with the file read (lens A fold).
#[cfg(target_arch = "aarch64")]
fn unafs_verb_snapls(generation: u64, path: &str) -> alloc::vec::Vec<String> {
    use crate::fs::unafs::{read_authz, ReadAuthz, KERNEL_PRINCIPAL};
    let out = crate::fs::unafs::with_unafs(|fs| {
        // Resolve the directory's logical id in the snapshot (scoped: the view
        // borrows fs; release it so the ACL check can re-borrow).
        let dir_id = {
            let mut view = match fs.open_snapshot(generation) {
                Ok(v) => v,
                Err(::unafs::fs::FileSystemError::SnapshotNotFound(_)) => {
                    return alloc::vec![alloc::format!("usnapls: no such snapshot generation {}", generation)];
                }
                Err(e) => return alloc::vec![alloc::format!("usnapls: {:?}", e)],
            };
            match view.resolve_path(path) {
                Ok(id) => id,
                Err(_) => return alloc::vec![alloc::format!("usnapls: {}: not in snapshot", path)],
            }
        };
        // CURRENT-ACL on the live directory — the same evaluator as usnapcat.
        match read_authz(fs, dir_id, KERNEL_PRINCIPAL) {
            ReadAuthz::Permit => {}
            ReadAuthz::DenyNoLiveObject => {
                return alloc::vec![alloc::format!(
                    "usnapls: {}: refused — directory deleted from live tree (no current ACL; fail-closed)",
                    path
                )];
            }
            ReadAuthz::DenyAcl => {
                return alloc::vec![alloc::format!(
                    "usnapls: {}: refused — current ACL denies this principal",
                    path
                )];
            }
        }
        let mut view = match fs.open_snapshot(generation) {
            Ok(v) => v,
            Err(e) => return alloc::vec![alloc::format!("usnapls: {:?}", e)],
        };
        match view.ls(dir_id) {
            Ok(entries) => {
                let mut lines = alloc::vec![alloc::format!("snapshot gen {} : {}", generation, path)];
                for e in &entries {
                    lines.push(alloc::format!("  {:<24}  id {:>5}  {:?}", e.name, e.inode_id, e.kind));
                }
                if entries.is_empty() {
                    lines.push(String::from("  (empty)"));
                }
                lines
            }
            Err(e) => alloc::vec![alloc::format!("usnapls: {:?}", e)],
        }
    });
    match out {
        Ok(lines) => lines,
        Err(e) => alloc::vec![alloc::format!("usnapls: no unafs volume ({:?})", e)],
    }
}

/// `usnapcat <gen> <path>`: read a file from a retained snapshot under the LIVE
/// object's CURRENT ACL (K8c high-security ruling). The shell runs at kernel
/// authority, so it reads any LIVE object — but a file DELETED from the live
/// tree fails closed (no current ACL row), and that refusal is reported plainly.
#[cfg(target_arch = "aarch64")]
fn unafs_verb_snapcat(generation: u64, path: &str) -> String {
    use crate::fs::unafs::{ReadAuthz, SnapReadResult, KERNEL_PRINCIPAL};
    match crate::fs::unafs::snapshot_read(generation, path, KERNEL_PRINCIPAL) {
        Ok(SnapReadResult::Ok(bytes)) => {
            // Print the retained bytes as UTF-8 where possible, else a byte count.
            match core::str::from_utf8(&bytes) {
                Ok(s) => alloc::format!("usnapcat: gen {} {} ({} bytes)\n{}", generation, path, bytes.len(), s),
                Err(_) => alloc::format!("usnapcat: gen {} {} ({} bytes, binary)", generation, path, bytes.len()),
            }
        }
        Ok(SnapReadResult::NotInSnapshot) => {
            alloc::format!("usnapcat: {}: not in snapshot gen {}", path, generation)
        }
        Ok(SnapReadResult::SnapshotMissing) => {
            alloc::format!("usnapcat: no such snapshot generation {}", generation)
        }
        Ok(SnapReadResult::Refused(ReadAuthz::DenyNoLiveObject)) => alloc::format!(
            "usnapcat: {}: refused — object deleted from live tree (no current ACL; fail-closed)",
            path
        ),
        Ok(SnapReadResult::Refused(ReadAuthz::DenyAcl)) => {
            alloc::format!("usnapcat: {}: refused — current ACL denies this principal", path)
        }
        Ok(SnapReadResult::Refused(ReadAuthz::Permit)) => {
            // Unreachable (Permit is not a refusal) — reported rather than panicked.
            alloc::format!("usnapcat: {}: internal: permit reported as refusal", path)
        }
        Err(e) => alloc::format!("usnapcat: no unafs volume ({:?})", e),
    }
}
