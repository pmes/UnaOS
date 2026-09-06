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
// VFSROUTE: the three FAT types this file used to name everywhere are down to ONE consumer — the
// x86 exec probe (`fat_path_is_file` + `open_read_volume`), which must bind the program source
// itself so `fatverb_storage_witness` has two independent producers to compare. Every verb sees
// `crate::fs::vfs::{DirEnt, Stat, VfsError}` instead, which is the point: a listing row in this
// shell is a NAMESPACE fact, not a FAT structure. aarch64 names none of them at all.
#[cfg(target_arch = "x86_64")]
use crate::fs::fat::{DirEntry, FatError, FatFs};
// The in-kernel `vug` demo (the `vug` and `pulse` verbs) is an aarch64 module, and since DECRUD-1 a
// knob-gated one — see the `pub mod vug` note in `lib.rs`. The verbs that drive it carry the identical
// gate, so wherever the module is not compiled they are not registered at all and the words fall
// through to the normal unknown-command reply: on x86 always, and on the Pi unless `UNAOS_VUGDEMO=1`.
#[cfg(all(target_arch = "aarch64", feature = "vugdemo"))]
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
// SCOPE (JD6): the WHOLE tree the shell can `cd` into. VFSROUTE (orin 17) replaced this paragraph's
// machinery without changing its scope: the path is normalized against the cwd by `vfs_path` and the
// PARENT walk now happens inside whichever backend the mount table resolved to — `FatBackend`'s
// `resolve_parent` for a FAT volume (which rides the same dir-aware `create_in_dir`/`locate_in_dir`
// twins, `first_cluster == 0` ⇒ root), `NativeBackend`'s `native_parent` for the native one.
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




/// PI-UI-3: print a verb's output line to the panel AND mirror it to the serial console as a
/// `:: ui3:<verb>: <line> ::` witness. On the Pi bench the verb output renders panel-only, so a
/// headless capture cannot see it; the witness gives the same content on the wire so `date`/`time`/
/// `ifconfig` are verifiable from serial alone. Same content on both sinks, byte-for-byte.
fn ui3_say(console: &mut Console, verb: &str, line: &str) {
    console.println(line);
    serial_println!(":: ui3:{}: {} ::", verb, line);
}

/// PI-FS-5: panel-line + `:: fs5: <line> ::` serial mirror (the `ui3_say` idiom, dedicated tag) — the
/// `fdisk -l` verb renders panel-only on the bench, so the witness gives a headless capture the same content.
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
    // VFSROUTE: the three HANDLE codes are stamped only where a program source is bound — the x86
    // arm of `vfs_mount_table` and the x86 exec probe. aarch64 mounts NAMED volumes (`/`, `/fat`,
    // `/usb`) and never asks the block layer "which handle holds the programs", so it stamps only
    // the write gate's ADMITTED/REFUSED_RO/DECLINED.
    #[cfg(target_arch = "x86_64")]
    pub const GLOBAL: u8 = 2;
    #[cfg(target_arch = "x86_64")]
    pub const USB: u8 = 3;
    #[cfg(target_arch = "x86_64")]
    pub const SDHC: u8 = 4;
    /// The write gate ADMITTED the volume.
    pub const ADMITTED: u8 = 5;
    /// The write gate REFUSED it as read-only.
    pub const REFUSED_RO: u8 = 6;

    #[cfg(target_arch = "x86_64")]
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
/// VFSROUTE: that site is now `vfs_mount_table`'s x86 arm — the ONE place a verb path binds a
/// program source — so the instrument is x86-only, like the exec probe it is compared against.
#[cfg(target_arch = "x86_64")]
static READ_BIND: AtomicU8 = AtomicU8::new(bind::NONE);
#[cfg(target_arch = "x86_64")]
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


/// FATVERB: mount the volume a READ verb should act on — the program source, so `ls` and a bare
/// name are looking at the same card. Stamps [`READ_BIND`] either way, including on the decline:
/// "the read verbs asked and got nothing" is a different fact from "the read verbs never asked",
/// and Boot AR's symptom was the first one.
#[cfg(target_arch = "x86_64")]
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


// FATVERB: TWO SINKS, TWO LENGTHS — and that is deliberate, not laziness.
//
// The first cut printed one ~235-character line to both. The panel is 128–180 columns depending on
// the scale metrics, so the census tail — the part that says WHICH handles existed, i.e. the whole
// diagnostic — was clipped off the right edge and never reached the eye it was written for. Serial
// has no such limit and a bench capture wants everything. So the operator gets a sentence and the
// capture gets the forensics, and they carry the same verdict word (`REFUSED READ-ONLY`) so one is
// greppable from the other.



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
// RELICS (orin 17): the scan is O(len) over this file, and this file grew past the default
// `long_running_const_eval` step budget with this arc. The lint is a guard against a const that may
// never terminate; this one provably does (a single forward walk bounded by the file length), and
// the lint's own help says an allow is the right answer for a const that is merely long. The ASSERT
// is untouched — the law still fails the build, which is the protection.
#[allow(long_running_const_eval)]
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
        "FATVERB source law: shell.rs must not bind the default FAT handle. VFSROUTE (orin 17) \
         tightened this: a file verb does not bind a FAT handle AT ALL any more — it resolves its \
         path through vfs_mount_table() and calls the VfsBackend trait, and the ONE call that still \
         names a source is that builder's open_read_volume (the PROGRAM SOURCE, x86) plus the exec \
         probe. If you are only naming the function in prose, spell it without parentheses."
    );
};

// ===================== VFSROUTE (orin 17) — EVERY FILE VERB ASKS THE MOUNT TABLE ==================
//
// Peter, 2026-09-06: *"should UnaFS not support ls? I'm confused that ls has to be made to list a
// dir. Should a mounted file system not be listable? Sounds like we'll be adding each filesystem to
// ls so it lists."*
//
// He is describing the defect exactly. Before this arc a file verb CHOSE a filesystem: `ls` picked
// unafs on aarch64 and FAT on x86, `cat` had two bodies, and every mutating verb ran a `/fat`/`/usb`
// prefix test (RELICS's `native_target`) to decide whether to call `unafs_verb_*` or walk `fat.rs`.
// That is "adding each filesystem to ls", and the next volume would have added a third arm to every
// one of them.
//
// THE RULE THIS SECTION IMPLEMENTS. A verb resolves its argument to an absolute path in the ONE
// namespace, hands that path to the mount table, and calls the trait. Which filesystem answers is
// [`crate::fs::vfs::MountTable::resolve`]'s longest-prefix decision and nothing else. No verb names
// a filesystem, no verb has a `target_arch` gate, and a backend that cannot perform an operation
// returns a TYPED error the verb prints — never a silent fall-through to some other volume. `rmdir`
// on the native volume is the worked example: the UnaFS crate has no directory removal, so
// `NativeBackend` inherits the trait's default and the operator sees
// `rmdir: /D: operation not supported on this volume (-ENOTSUP)` instead of a directory quietly
// disappearing off the FAT boot partition, which is what the pre-VFSROUTE verb did.
//
// WHAT MOVED, AND WHAT THAT COST. The FAT-direct bodies these functions used to carry are gone from
// this file, not deleted from the tree: `fat.rs`'s public API is untouched and the VFS's
// `FatBackend` calls exactly the same primitives (`locate_in_dir`, `create_in_dir`, `write_grow`,
// `delete_located`, `remove_dir`, `rename_entry`, `move_entry`) — the walk simply happens behind the
// trait now. Three FAT-SPECIFIC facts stopped being printable, because a trait that could print them
// would be a FAT trait: `rm`'s freed-cluster count, `stat`'s attr byte / first cluster / directory
// slot LBA, and `ls`'s canonical 8.3 re-spelling of the typed name. The forensic pair is still
// reachable — `hexdump` for bytes, `dd if=<lba>` for sectors — and the two verbs that answered
// "which device is this" (`fdisk -l`, `mount`) are unchanged.
//
// THE ONE THING A VERB STILL ASKS THE VOLUME: whether it accepts mutation at all. That question is
// `VfsBackend::write_veto`, which forwards to `fat::BlockSource::write_veto` — the single definition
// FATVERB established. It is asked BEFORE a multi-step verb starts, so a read-only volume yields a
// whole refusal instead of a half-finished mutation and an opaque I/O error several sectors in.

/// The principal every shell verb acts as. The shell runs at kernel authority — it is the console
/// of the machine, not a tenant — so it passes [`crate::fs::vfs::KERNEL_PRINCIPAL`] to every ACL
/// check. Named once here rather than spelled at forty call sites.
const SHELL_PRINCIPAL: &str = crate::fs::vfs::KERNEL_PRINCIPAL;

/// VFSROUTE: build the live namespace and resolve ONE typed argument in it, for a READ verb.
///
/// Returns the table (the caller keeps it, so a recursive walk builds it once) and the absolute
/// path. `Err` is the operator line, already tagged with the reason: an unbound reserved volume
/// (VFS-4's `-ENODEV`, the P44 misdirection) or a namespace with nothing mounted at all.
fn vfs_read_target(verb: &str, arg: &str) -> Result<(crate::fs::vfs::MountTable, String), String> {
    let mt = vfs_mount_table();
    let path = vfs_path(arg);
    if let Some(vol) = unmounted_reserved_volume(&mt.prefixes(), &path) {
        return Err(alloc::format!("{}: {}: volume {} not mounted (-ENODEV)", verb, path, vol));
    }
    if mt.prefixes().is_empty() {
        // FATVERB's decline line, kept verbatim in content: a bench capture must be able to tell
        // "the verb asked and there was no volume" from "the keystroke never arrived".
        serial_println!(
            ":: [fatverb] {} -> NO VOLUME (namespace empty; handles={}) ::",
            verb, crate::drivers::block::source_census()
        );
        return Err(alloc::format!("{}: no filesystem mounted (-ENODEV)", verb));
    }
    Ok((mt, path))
}

/// VFSROUTE: the READ-verb front door — resolve, or print the refusal and give up.
fn vfs_read_open(console: &mut Console, verb: &str, arg: &str)
    -> Option<(crate::fs::vfs::MountTable, String)>
{
    match vfs_read_target(verb, arg) {
        Ok(t) => Some(t),
        Err(msg) => {
            console.println(&msg);
            None
        }
    }
}

/// VFSROUTE: the WRITE-verb front door — resolve, then ASK THE VOLUME whether it accepts mutation
/// before anything is touched.
///
/// This is FATVERB's write gate, moved to the layer that owns the question. It still stamps
/// [`WRITE_GATE`] and still writes the two-sink refusal (`REFUSED READ-ONLY` on the panel, the
/// census on serial), because `fatverb_storage_witness` reads that instrument and a bench capture
/// reads that line — but the predicate is now `VfsBackend::write_veto` on the volume the PATH
/// resolves to, not `mount_program_source().write_veto()` on whatever the program source happens to
/// be. On a machine with two writable volumes in one namespace that is the difference between
/// gating the volume you are about to write and gating a different one.
fn vfs_write_open(console: &mut Console, verb: &str, arg: &str)
    -> Option<(crate::fs::vfs::MountTable, String)>
{
    let (mt, path) = match vfs_read_target(verb, arg) {
        Ok(t) => t,
        Err(msg) => {
            stamp(&WRITE_GATE, &WRITE_GATE_SEQ, bind::DECLINED);
            console.println(&msg);
            return None;
        }
    };
    match mt.write_veto(&path) {
        Ok(None) => {
            stamp(&WRITE_GATE, &WRITE_GATE_SEQ, bind::ADMITTED);
            Some((mt, path))
        }
        Ok(Some(why)) => {
            stamp(&WRITE_GATE, &WRITE_GATE_SEQ, bind::REFUSED_RO);
            let vol = mt.volume_name(&path).unwrap_or_else(|_| String::from("?"));
            console.println(&alloc::format!("{}: REFUSED READ-ONLY ({})", verb, vol));
            serial_println!(
                ":: [fatverb] {} -> REFUSED READ-ONLY (volume={} path={} reason={}; handles={}) ::",
                verb, vol, path, why, crate::drivers::block::source_census()
            );
            None
        }
        Err(e) => {
            stamp(&WRITE_GATE, &WRITE_GATE_SEQ, bind::DECLINED);
            console.println(&alloc::format!("{}: {}: {}", verb, path, vfs_err(e)));
            None
        }
    }
}

/// VFSROUTE: the one refusal renderer — `verb: path: reason`, the shell's errno house style.
fn vfs_fail(console: &mut Console, verb: &str, path: &str, e: crate::fs::vfs::VfsError) {
    vfs_say(console, &alloc::format!("{}: {}: {}", verb, path, vfs_err(e)));
}

/// VFSROUTE: the leaf of an absolute namespace path (`/A/B.TXT` -> `B.TXT`; the root -> `""`).
fn vfs_leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or("")
}

/// VFSROUTE: the parent of an absolute namespace path (`/A/B.TXT` -> `/A`; a root child -> `/`).
fn vfs_parent(path: &str) -> String {
    match path.rfind('/') {
        None | Some(0) => String::from("/"),
        Some(i) => String::from(&path[..i]),
    }
}

/// VFSROUTE: join a directory path and a leaf into an absolute namespace path.
fn vfs_join(dir: &str, leaf: &str) -> String {
    if dir == "/" {
        alloc::format!("/{}", leaf)
    } else {
        alloc::format!("{}/{}", dir, leaf)
    }
}

/// VFSROUTE: is `path` a directory in the namespace? (`false` for anything that does not resolve —
/// the callers that ask this are deciding a destination shape, not reporting an error.)
fn vfs_is_dir(mt: &crate::fs::vfs::MountTable, path: &str) -> bool {
    matches!(mt.stat(path), Ok(st) if matches!(st.kind, crate::fs::vfs::NodeKind::Dir))
}

/// JD6 `touch`: ensure a 0-length file exists at `path` (create if absent; idempotent no-op if
/// present) — on WHICHEVER volume the namespace says `path` lives on. `touch <path>`.
fn fs_touch(console: &mut Console, arg: &str) {
    use crate::fs::vfs::{NodeKind, VfsError};
    let Some((mt, path)) = vfs_write_open(console, "touch", arg) else { return };
    match mt.stat(&path) {
        Ok(_) => vfs_say(console, &path), // already exists — idempotent, print the path
        Err(VfsError::NoSuchPath) => match mt.create(&path, NodeKind::File, SHELL_PRINCIPAL) {
            Ok(_) => vfs_say(console, &path),
            Err(e) => vfs_fail(console, "touch", &path, e),
        },
        Err(e) => vfs_fail(console, "touch", &path, e),
    }
}

/// JD6 `write`: create-or-REPLACE the file at `path` with exactly `data`. A directory target is
/// refused (`-EISDIR`).
///
/// Replace is unlink-then-create, not truncate-to-0: the native backend has no in-place shrink
/// primitive (a native `truncate` to 0 on a non-empty file is `Unsupported` BY DESIGN, because
/// shrink-by-recreate would drop the per-object ACL), so unlink+create is the one shape that works
/// on both volumes. Said here rather than discovered later.
fn fs_write(console: &mut Console, arg: &str, data: &[u8]) {
    use crate::fs::vfs::NodeKind;
    let Some((mt, path)) = vfs_write_open(console, "write", arg) else { return };
    if vfs_is_dir(&mt, &path) {
        return console.println(&alloc::format!("write: {}: is a directory (-EISDIR)", path));
    }
    let _ = mt.unlink(&path, SHELL_PRINCIPAL); // drop the old contents if the name was taken
    if let Err(e) = mt.create(&path, NodeKind::File, SHELL_PRINCIPAL) {
        return vfs_fail(console, "write", &path, e);
    }
    if data.is_empty() {
        return vfs_say(console, &alloc::format!("wrote 0 bytes to {}", path));
    }
    match mt.write(&path, 0, data, SHELL_PRINCIPAL) {
        Ok(n) => vfs_say(console, &alloc::format!("wrote {} bytes to {} ({} bytes)", n, path, n)),
        Err(e) => vfs_fail(console, "write", &path, e),
    }
}

/// JD5 `append`: append `data` at EOF, creating the file if absent (like `>>`). A directory target
/// is refused (`-EISDIR`).
fn fs_append(console: &mut Console, arg: &str, data: &[u8]) {
    use crate::fs::vfs::{NodeKind, VfsError};
    let Some((mt, path)) = vfs_write_open(console, "append", arg) else { return };
    let offset = match mt.stat(&path) {
        Ok(st) if matches!(st.kind, NodeKind::Dir) =>
            return console.println(&alloc::format!("append: {}: is a directory (-EISDIR)", path)),
        Ok(st) => st.size,
        Err(VfsError::NoSuchPath) => {
            if let Err(e) = mt.create(&path, NodeKind::File, SHELL_PRINCIPAL) {
                return vfs_fail(console, "append", &path, e);
            }
            0
        }
        Err(e) => return vfs_fail(console, "append", &path, e),
    };
    if data.is_empty() {
        return vfs_say(console, &alloc::format!(
            "appended 0 bytes to {} ({} bytes)", path, offset));
    }
    match mt.write(&path, offset, data, SHELL_PRINCIPAL) {
        Ok(n) => vfs_say(console, &alloc::format!(
            "appended {} bytes to {} ({} bytes)", n, path, offset + n as u64)),
        Err(e) => vfs_fail(console, "append", &path, e),
    }
}

/// JD6 `rm`: delete a FILE. A directory is `-EISDIR` (use `rmdir`, or `rm -r`); an absent name is
/// `-ENOENT` EXCEPT under `force` (JD14 `-f`), which is quiet the way POSIX `rm -f` is. A
/// wrong-usage `-EISDIR` is still shown under `-f`, exactly as POSIX `rm -f DIR` still complains.
fn fs_rm(console: &mut Console, arg: &str, force: bool) {
    use crate::fs::vfs::VfsError;
    let Some((mt, path)) = vfs_write_open(console, "rm", arg) else { return };
    match mt.unlink(&path, SHELL_PRINCIPAL) {
        Ok(()) => vfs_say(console, &alloc::format!("removed {}", path)),
        Err(VfsError::NoSuchPath) => {
            if !force {
                console.println(&alloc::format!("rm: {}: not found (-ENOENT)", path));
            }
        }
        Err(e) => vfs_fail(console, "rm", &path, e),
    }
}

/// JD7 `mkdir`: create a directory. An existing name (file OR directory) is `-EEXIST`; a missing
/// parent `-ENOENT`; a parent that is a file `-ENOTDIR`.
fn fs_mkdir(console: &mut Console, arg: &str) {
    use crate::fs::vfs::{NodeKind, VfsError};
    let Some((mt, path)) = vfs_write_open(console, "mkdir", arg) else { return };
    match mt.create(&path, NodeKind::Dir, SHELL_PRINCIPAL) {
        Ok(_) => vfs_say(console, &alloc::format!("created directory {}", path)),
        Err(VfsError::Backend("exists")) =>
            console.println(&alloc::format!("mkdir: {}: file exists (-EEXIST)", path)),
        Err(e) => vfs_fail(console, "mkdir", &path, e),
    }
}

/// JD7 `rmdir`: remove an EMPTY directory. The root is refused locally (`-EBUSY` — it is unnameable
/// on every volume); a file target is `-ENOTDIR`; a non-empty directory is `-ENOTEMPTY`.
///
/// **This is the verb that shows the rule working.** The native UnaFS backend implements no
/// `remove_dir` (the crate has none), so `rmdir /SOMEDIR` on the native volume prints
/// `-ENOTSUP` — the volume's own honest answer. Before VFSROUTE the verb was FAT-direct on both
/// arches, so the same keystroke walked the FAT boot partition looking for a name that lives on a
/// different volume: at best `-ENOENT`, at worst a directory removed off the wrong card.
fn fs_rmdir(console: &mut Console, arg: &str) {
    use crate::fs::vfs::VfsError;
    let Some((mt, path)) = vfs_write_open(console, "rmdir", arg) else { return };
    if path == "/" {
        return console.println("rmdir: /: cannot remove the root directory (-EBUSY)");
    }
    match mt.remove_dir(&path, SHELL_PRINCIPAL) {
        Ok(()) => vfs_say(console, &alloc::format!("removed directory {}", path)),
        Err(VfsError::Backend("not-empty")) =>
            console.println(&alloc::format!("rmdir: {}: directory not empty (-ENOTEMPTY)", path)),
        Err(e) => vfs_fail(console, "rmdir", &path, e),
    }
}

/// VFSROUTE: copy one FILE's bytes from `src` to a freshly-created `dst`, streaming in fixed
/// `CP_WINDOW` windows so a file of ANY size copies with a bounded heap footprint and no truncation.
/// Returns the byte count, or a formatted error line.
///
/// **A cross-volume copy now works**, and it works for free: the read side asks whichever backend
/// owns `src` and the write side asks whichever owns `dst`, so `cp /K3HELLO.TXT /fat/HELLO.TXT`
/// moves bytes between the native volume and the FAT card. The pre-VFSROUTE verb could not — it held
/// ONE `FatFs` and both ends had to be on it.
fn vfs_copy_bytes(
    mt: &crate::fs::vfs::MountTable,
    src: &str,
    dst: &str,
    size: u64,
) -> Result<u64, String> {
    const CP_WINDOW: usize = 32 * 1024;
    let mut off: u64 = 0;
    while off < size {
        let want = core::cmp::min(CP_WINDOW as u64, size - off) as usize;
        let buf = mt
            .read(src, off, want)
            .map_err(|e| alloc::format!("{}: {}", src, vfs_err(e)))?;
        if buf.is_empty() {
            break; // the source ended early (malformed) — copy what it holds, honestly
        }
        let wrote = mt
            .write(dst, off, &buf, SHELL_PRINCIPAL)
            .map_err(|e| alloc::format!("{}: {}", dst, vfs_err(e)))?;
        off += wrote as u64;
        if wrote < buf.len() {
            break; // short write — report what landed rather than looping forever
        }
    }
    Ok(off)
}

/// JD8 `cp <src> <dst>`: copy a FILE. `cp FILE DIR/` lands as `DIR/<leaf>`. JD14: no-clobber is the
/// DEFAULT — an existing destination FILE is `-EEXIST` unless `-f`; `-n` reasserts the default.
/// A directory source is `-EISDIR` (use `cp -r`).
fn fs_cp(console: &mut Console, src: &str, dst: &str, force: bool) {
    use crate::fs::vfs::{NodeKind, VfsError};
    let Some((mt, spath)) = vfs_read_open(console, "cp", src) else { return };
    let dpath_arg = match vfs_read_target("cp", dst) {
        Ok((_, p)) => p,
        Err(msg) => return console.println(&msg),
    };
    let st = match mt.stat(&spath) {
        Ok(s) => s,
        Err(e) => return vfs_fail(console, "cp", &spath, e),
    };
    if matches!(st.kind, NodeKind::Dir) {
        return console.println(&alloc::format!("cp: {}: is a directory (-EISDIR)", spath));
    }
    // The `cp FILE DIR/` idiom: an existing directory receives the copy under the source's leaf.
    let dpath = if vfs_is_dir(&mt, &dpath_arg) {
        vfs_join(&dpath_arg, vfs_leaf(&spath))
    } else {
        dpath_arg
    };
    if dpath.eq_ignore_ascii_case(&spath) {
        return console.println(&alloc::format!(
            "cp: {}: cannot copy a file onto itself (-EINVAL)", spath));
    }
    // The write gate is asked about the DESTINATION volume — the one that is about to be mutated.
    if !vfs_gate_ok(console, &mt, "cp", &dpath) {
        return;
    }
    match mt.stat(&dpath) {
        Ok(d) if matches!(d.kind, NodeKind::Dir) =>
            return console.println(&alloc::format!("cp: {}: is a directory (-EISDIR)", dpath)),
        Ok(_) => {
            if !force {
                return console.println(&alloc::format!(
                    "cp: {}: file exists (-EEXIST); use cp -f to overwrite", dpath));
            }
            if let Err(e) = mt.unlink(&dpath, SHELL_PRINCIPAL) {
                return vfs_fail(console, "cp", &dpath, e);
            }
        }
        Err(VfsError::NoSuchPath) => {}
        Err(e) => return vfs_fail(console, "cp", &dpath, e),
    }
    if let Err(e) = mt.create(&dpath, NodeKind::File, SHELL_PRINCIPAL) {
        return vfs_fail(console, "cp", &dpath, e);
    }
    match vfs_copy_bytes(&mt, &spath, &dpath, st.size) {
        Ok(n) => vfs_say(console, &alloc::format!("copied {} -> {} ({} bytes)", spath, dpath, n)),
        Err(msg) => console.println(&alloc::format!("cp: {}", msg)),
    }
}

/// VFSROUTE: the write gate asked about an already-resolved path (the multi-path verbs `cp`/`mv`
/// gate their DESTINATION, which `vfs_write_open` cannot do because it resolves a typed argument).
fn vfs_gate_ok(
    console: &mut Console,
    mt: &crate::fs::vfs::MountTable,
    verb: &str,
    path: &str,
) -> bool {
    match mt.write_veto(path) {
        Ok(None) => {
            stamp(&WRITE_GATE, &WRITE_GATE_SEQ, bind::ADMITTED);
            true
        }
        Ok(Some(why)) => {
            stamp(&WRITE_GATE, &WRITE_GATE_SEQ, bind::REFUSED_RO);
            let vol = mt.volume_name(path).unwrap_or_else(|_| String::from("?"));
            console.println(&alloc::format!("{}: REFUSED READ-ONLY ({})", verb, vol));
            serial_println!(
                ":: [fatverb] {} -> REFUSED READ-ONLY (volume={} path={} reason={}; handles={}) ::",
                verb, vol, path, why, crate::drivers::block::source_census()
            );
            false
        }
        Err(e) => {
            stamp(&WRITE_GATE, &WRITE_GATE_SEQ, bind::DECLINED);
            vfs_fail(console, verb, path, e);
            false
        }
    }
}

/// JD9: recursively copy the CONTENTS of directory `src` INTO the already-created directory `dst`.
/// `stats` accumulates across the whole tree so the caller can report an honest partial count.
/// Depth-capped at [`CP_MAX_DEPTH`] (`-ELOOP`). SNAPSHOT per level: the listing is captured before
/// any mutation, so copying as we go never invalidates the walk.
fn cp_tree(
    mt: &crate::fs::vfs::MountTable,
    src: &str,
    dst: &str,
    depth: u32,
    stats: &mut CpStats,
) -> Result<(), String> {
    use crate::fs::vfs::NodeKind;
    if depth > CP_MAX_DEPTH {
        return Err(alloc::format!(
            "{}: maximum directory depth {} exceeded (-ELOOP)", src, CP_MAX_DEPTH));
    }
    let rows = mt
        .read_dir(src)
        .map_err(|e| alloc::format!("{}: {}", src, vfs_err(e)))?;
    for r in &rows {
        let child_src = vfs_join(src, &r.name);
        let child_dst = vfs_join(dst, &r.name);
        match r.kind {
            NodeKind::Dir => {
                mt.create(&child_dst, NodeKind::Dir, SHELL_PRINCIPAL)
                    .map_err(|e| alloc::format!("{}: {}", child_dst, vfs_err(e)))?;
                stats.dirs += 1;
                cp_tree(mt, &child_src, &child_dst, depth + 1, stats)?;
            }
            NodeKind::File => {
                mt.create(&child_dst, NodeKind::File, SHELL_PRINCIPAL)
                    .map_err(|e| alloc::format!("{}: {}", child_dst, vfs_err(e)))?;
                let n = vfs_copy_bytes(mt, &child_src, &child_dst, r.size)?;
                stats.files += 1;
                stats.bytes += n;
            }
        }
    }
    Ok(())
}

/// JD9 `cp -r <src> <dst>`: recursively copy a directory tree. `cp -r DIR DEST/` lands the tree as
/// `DEST/<leaf>`. A FILE source degrades to a plain `cp`. Refuses to copy a directory into its own
/// subtree (`-EINVAL`). One summary line, or an honest partial count on the first failure.
fn fs_cp_recursive(console: &mut Console, src: &str, dst: &str, force: bool) {
    use crate::fs::vfs::{NodeKind, VfsError};
    let Some((mt, spath)) = vfs_read_open(console, "cp", src) else { return };
    let st = match mt.stat(&spath) {
        Ok(s) => s,
        Err(e) => return vfs_fail(console, "cp", &spath, e),
    };
    if matches!(st.kind, NodeKind::File) {
        return fs_cp(console, src, dst, force); // `-r` on a file is a plain copy
    }
    let dpath_arg = match vfs_read_target("cp", dst) {
        Ok((_, p)) => p,
        Err(msg) => return console.println(&msg),
    };
    let dpath = if vfs_is_dir(&mt, &dpath_arg) {
        vfs_join(&dpath_arg, vfs_leaf(&spath))
    } else {
        dpath_arg
    };
    if dpath.eq_ignore_ascii_case(&spath) || is_descendant(&dpath, &spath) {
        return console.println(&alloc::format!(
            "cp: cannot copy directory {} into itself or its own subtree ({}) (-EINVAL)",
            spath, dpath));
    }
    if !vfs_gate_ok(console, &mt, "cp", &dpath) {
        return;
    }
    match mt.stat(&dpath) {
        Ok(_) => return console.println(&alloc::format!(
            "cp: {}: file exists (-EEXIST); remove it first", dpath)),
        Err(VfsError::NoSuchPath) => {}
        Err(e) => return vfs_fail(console, "cp", &dpath, e),
    }
    if let Err(e) = mt.create(&dpath, NodeKind::Dir, SHELL_PRINCIPAL) {
        return vfs_fail(console, "cp", &dpath, e);
    }
    let mut stats = CpStats { dirs: 1, files: 0, bytes: 0 };
    match cp_tree(&mt, &spath, &dpath, 1, &mut stats) {
        Ok(()) => vfs_say(console, &alloc::format!(
            "copied {} -> {} ({} file(s), {} dir(s), {} bytes)",
            spath, dpath, stats.files, stats.dirs, stats.bytes)),
        Err(msg) => {
            console.println(&alloc::format!("cp: {}", msg));
            console.println(&alloc::format!(
                "cp: partial — {} file(s), {} dir(s), {} bytes copied",
                stats.files, stats.dirs, stats.bytes));
        }
    }
}

/// JD13: recursively delete the CONTENTS of directory `dir` — child FILES then child DIRECTORIES,
/// depth-first, so a directory is emptied before it is removed. Depth-capped at [`CP_MAX_DEPTH`].
/// SNAPSHOT-then-delete: the listing is captured before any mutation and each child is addressed BY
/// NAME, so deleting as we go never invalidates the walk.
fn rm_tree(
    mt: &crate::fs::vfs::MountTable,
    dir: &str,
    depth: u32,
    stats: &mut RmStats,
) -> Result<(), String> {
    use crate::fs::vfs::NodeKind;
    if depth > CP_MAX_DEPTH {
        return Err(alloc::format!(
            "{}: maximum directory depth {} exceeded (-ELOOP)", dir, CP_MAX_DEPTH));
    }
    let rows = mt
        .read_dir(dir)
        .map_err(|e| alloc::format!("{}: {}", dir, vfs_err(e)))?;
    for r in rows.iter().filter(|r| matches!(r.kind, NodeKind::File)) {
        let child = vfs_join(dir, &r.name);
        mt.unlink(&child, SHELL_PRINCIPAL)
            .map_err(|e| alloc::format!("{}: {}", child, vfs_err(e)))?;
        stats.files += 1;
    }
    for r in rows.iter().filter(|r| matches!(r.kind, NodeKind::Dir)) {
        let child = vfs_join(dir, &r.name);
        rm_tree(mt, &child, depth + 1, stats)?;
        mt.remove_dir(&child, SHELL_PRINCIPAL)
            .map_err(|e| alloc::format!("{}: {}", child, vfs_err(e)))?;
        stats.dirs += 1;
    }
    Ok(())
}

/// JD13 `rm -r <path>`: recursively delete a directory tree (files then directories, depth-first).
/// The volume root is refused (`-EBUSY`); a FILE target degrades to a plain delete; a missing target
/// is quiet under `-f`. One summary line, or an honest partial count on the first failure.
fn fs_rm_recursive(console: &mut Console, arg: &str, force: bool) {
    use crate::fs::vfs::{NodeKind, VfsError};
    let Some((mt, path)) = vfs_write_open(console, "rm", arg) else { return };
    if path == "/" {
        return console.println("rm: /: cannot remove the volume root (-EBUSY)");
    }
    let st = match mt.stat(&path) {
        Ok(s) => s,
        Err(VfsError::NoSuchPath) => {
            if !force {
                console.println(&alloc::format!("rm: {}: not found (-ENOENT)", path));
            }
            return;
        }
        Err(e) => return vfs_fail(console, "rm", &path, e),
    };
    if matches!(st.kind, NodeKind::File) {
        return match mt.unlink(&path, SHELL_PRINCIPAL) {
            Ok(()) => vfs_say(console, &alloc::format!("removed {}", path)),
            Err(e) => vfs_fail(console, "rm", &path, e),
        };
    }
    let mut stats = RmStats { dirs: 0, files: 0 };
    match rm_tree(&mt, &path, 1, &mut stats) {
        Ok(()) => match mt.remove_dir(&path, SHELL_PRINCIPAL) {
            Ok(()) => {
                stats.dirs += 1;
                vfs_say(console, &alloc::format!(
                    "removed {} ({} file(s), {} dir(s))", path, stats.files, stats.dirs));
            }
            Err(e) => {
                vfs_fail(console, "rm", &path, e);
                console.println(&alloc::format!(
                    "rm: partial — {} file(s), {} dir(s) removed", stats.files, stats.dirs));
            }
        },
        Err(msg) => {
            console.println(&alloc::format!("rm: {}", msg));
            console.println(&alloc::format!(
                "rm: partial — {} file(s), {} dir(s) removed", stats.files, stats.dirs));
        }
    }
}

/// JD10 `mv`: move OR rename by relinking one directory entry — O(1), by reference, no data copy
/// (so `mv DIR NEWNAME` needs no `-r`). The `mv SRC DIR/` idiom lands the entry under DIR as the
/// source leaf. JD14: no-clobber is the DEFAULT — an existing destination is `-EEXIST` unless `-f`.
///
/// **A cross-volume move is refused by name, and the refusal comes from the mount table**, not from
/// a prefix test in this verb: relinking an entry is a within-volume operation, so
/// [`crate::fs::vfs::MountTable::same_volume`] is asked and the operator is told to `cp` then `rm`.
/// That is the same answer the pre-VFSROUTE verb gave for the native/FAT pair — but it now holds for
/// every pair of volumes the machine ever mounts, including two it has not met yet.
fn fs_mv(console: &mut Console, src: &str, dst: &str, force: bool) {
    use crate::fs::vfs::{NodeKind, VfsError};
    let Some((mt, spath)) = vfs_read_open(console, "mv", src) else { return };
    if spath == "/" {
        return console.println("mv: /: cannot move the volume root (-EBUSY)");
    }
    let dpath_arg = match vfs_read_target("mv", dst) {
        Ok((_, p)) => p,
        Err(msg) => return console.println(&msg),
    };
    let src_st = match mt.stat(&spath) {
        Ok(s) => s,
        Err(e) => return vfs_fail(console, "mv", &spath, e),
    };
    let dpath = if vfs_is_dir(&mt, &dpath_arg) {
        vfs_join(&dpath_arg, vfs_leaf(&spath))
    } else {
        dpath_arg
    };
    match mt.same_volume(&spath, &dpath) {
        Ok(true) => {}
        Ok(false) => return console.println(
            "mv: cross-volume move is not supported (copy with `cp`, then `rm`)"),
        Err(e) => return vfs_fail(console, "mv", &dpath, e),
    }
    if matches!(src_st.kind, NodeKind::Dir)
        && (dpath.eq_ignore_ascii_case(&spath) || is_descendant(&dpath, &spath))
    {
        return console.println(&alloc::format!(
            "mv: cannot move directory {} into itself or its own subtree ({}) (-EINVAL)",
            spath, dpath));
    }
    if !vfs_gate_ok(console, &mt, "mv", &dpath) {
        return;
    }
    if !dpath.eq_ignore_ascii_case(&spath) {
        match mt.stat(&dpath) {
            Ok(d) => {
                if !force {
                    return console.println(&alloc::format!(
                        "mv: {}: file exists (-EEXIST); use mv -f to overwrite", dpath));
                }
                // `-f`: remove the existing destination first, then relink into the freed name.
                // A DIRECTORY destination is tree-replaced (JD15), which is only possible on a
                // volume whose backend implements `remove_dir` — one that does not says so.
                let removed = match d.kind {
                    NodeKind::File => mt.unlink(&dpath, SHELL_PRINCIPAL),
                    NodeKind::Dir => {
                        let mut st = RmStats { dirs: 0, files: 0 };
                        match rm_tree(&mt, &dpath, 1, &mut st) {
                            Ok(()) => mt.remove_dir(&dpath, SHELL_PRINCIPAL),
                            Err(msg) => {
                                return console.println(&alloc::format!(
                                    "mv: -f: overwrite (remove existing) failed: {}", msg));
                            }
                        }
                    }
                };
                if let Err(e) = removed {
                    return console.println(&alloc::format!(
                        "mv: -f: overwrite (remove existing) failed: {}: {}", dpath, vfs_err(e)));
                }
            }
            Err(VfsError::NoSuchPath) => {}
            Err(e) => return vfs_fail(console, "mv", &dpath, e),
        }
    }
    match mt.rename(&spath, &dpath, SHELL_PRINCIPAL) {
        Ok(()) => vfs_say(console, &alloc::format!("moved {} -> {}", spath, dpath)),
        Err(VfsError::Backend("exists")) =>
            console.println(&alloc::format!("mv: {}: file exists (-EEXIST)", dpath)),
        Err(e) => vfs_fail(console, "mv", &dpath, e),
    }
}

/// VFSROUTE: `cat <path>` through the mount table. Bounded to `CAP` bytes so a huge file cannot
/// flood the console, with the honest `[... n of m bytes shown]` tail note.
///
/// One body for every volume and both arches. Before VFSROUTE there were two: an aarch64 routed one
/// and an x86 FAT-direct one, printing the same text from two places — the duplication Peter's
/// ruling names.
fn vfs_cat(console: &mut Console, arg: &str) {
    use crate::fs::vfs::NodeKind;
    const CAP: u64 = 8192;
    let Some((mt, path)) = vfs_read_open(console, "cat", arg) else { return };
    let st = match mt.stat(&path) {
        Ok(s) => s,
        Err(e) => return vfs_fail(console, "cat", &path, e),
    };
    if matches!(st.kind, NodeKind::Dir) {
        return console.println(&alloc::format!("cat: {}: is a directory (-EISDIR)", path));
    }
    let want = core::cmp::min(st.size, CAP);
    match mt.read(&path, 0, want as usize) {
        Ok(data) => {
            for line in render_text(&data).split('\n') {
                console.println(line);
            }
            if st.size > data.len() as u64 {
                console.println(&alloc::format!(
                    "[... {} of {} bytes shown]", data.len(), st.size));
            }
        }
        Err(e) => vfs_fail(console, "cat", &path, e),
    }
}

/// VFSROUTE: resolve one argument to a readable FILE — the shared front half of `head`, `tail`,
/// `hexdump`, `wc` and `grep`, wearing the caller's verb name so the error lines stay in house
/// style. Returns the table, the absolute path and its size.
fn vfs_file_target(console: &mut Console, verb: &str, arg: &str)
    -> Option<(crate::fs::vfs::MountTable, String, u64)>
{
    use crate::fs::vfs::NodeKind;
    let (mt, path) = vfs_read_open(console, verb, arg)?;
    match mt.stat(&path) {
        Ok(st) if matches!(st.kind, NodeKind::Dir) => {
            console.println(&alloc::format!("{}: {}: is a directory (-EISDIR)", verb, path));
            None
        }
        Ok(st) => Some((mt, path, st.size)),
        Err(e) => {
            vfs_fail(console, verb, &path, e);
            None
        }
    }
}

/// JD12 `head <path> [n]`: print the FIRST `n` lines (default 10). Streams from offset 0 in bounded
/// windows and STOPS as soon as `n` newlines are seen, so `head` of a huge file reads only the first
/// window(s). A byte ceiling (`HEAD_MAX`) backstops a file with no newlines.
fn fs_head(console: &mut Console, arg: &str, n: u32) {
    const WINDOW: usize = 4096;
    const HEAD_MAX: u64 = 64 * 1024;
    let Some((mt, path, size)) = vfs_file_target(console, "head", arg) else { return };
    let (mut off, mut lines) = (0u64, 0u32);
    let mut cur = String::new();
    let mut more = false;
    'outer: while off < size && off < HEAD_MAX && lines < n {
        let want = core::cmp::min(WINDOW as u64, size - off) as usize;
        let buf = match mt.read(&path, off, want) {
            Ok(b) => b,
            Err(e) => return vfs_fail(console, "head", &path, e),
        };
        if buf.is_empty() {
            break;
        }
        for (i, &b) in buf.iter().enumerate() {
            match b {
                b'\n' => {
                    console.println(&cur);
                    cur.clear();
                    lines += 1;
                    if lines >= n {
                        more = i + 1 < buf.len() || off + (buf.len() as u64) < size;
                        break 'outer;
                    }
                }
                b'\r' => {}
                0x20..=0x7e => cur.push(b as char),
                _ => cur.push('.'),
            }
        }
        off += buf.len() as u64;
    }
    if lines < n && !cur.is_empty() {
        console.println(&cur);
        lines += 1;
    }
    if more || (lines < n && off < size) {
        console.println(&alloc::format!("[... first {} line(s) shown]", lines));
    }
}

/// JD12 `tail <path> [n]`: print the LAST `n` lines (default 10). Reads a bounded window ending at
/// EOF, renders it, and prints the last `n` lines. A window that began mid-file drops its first
/// (cut) line and notes the bound. An empty file prints nothing.
fn fs_tail(console: &mut Console, arg: &str, n: u32) {
    const TAIL_MAX: u64 = 64 * 1024;
    let Some((mt, path, size)) = vfs_file_target(console, "tail", arg) else { return };
    if size == 0 {
        return;
    }
    let start = size.saturating_sub(TAIL_MAX);
    let buf = match mt.read(&path, start, (size - start) as usize) {
        Ok(b) => b,
        Err(e) => return vfs_fail(console, "tail", &path, e),
    };
    let text = render_text(&buf);
    if text.is_empty() {
        return;
    }
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop(); // a trailing '\n' yields an empty final element, not a real line
    }
    let windowed = start > 0;
    if windowed {
        // The first line is a partial iff the byte just before `start` is not a newline — one extra
        // byte read, and only when windowed, so essentially never in practice.
        let cut = match mt.read(&path, start - 1, 1) {
            Ok(p) => p.first() != Some(&b'\n'),
            Err(_) => true,
        };
        if cut && !lines.is_empty() {
            lines.remove(0);
        }
    }
    let from = lines.len().saturating_sub(n as usize);
    for l in &lines[from..] {
        console.println(l);
    }
    if windowed {
        console.println(&alloc::format!(
            "[... last {} byte(s) scanned]", size - start));
    }
}

/// JD19 `hexdump <path> [off] [len]`: bounded dump of a file's bytes. Default off=0, len=256; `len`
/// is hard-capped at `HEXDUMP_MAX` (4096). Rows carry the absolute file offset. An `off` at or past
/// EOF is an honest empty note; more bytes past the window get an honest tail note.
fn fs_hexdump(console: &mut Console, arg: &str, off: u32, len: usize) {
    const HEXDUMP_MAX: usize = 4096;
    let Some((mt, path, size)) = vfs_file_target(console, "hexdump", arg) else { return };
    if off as u64 >= size {
        return console.println(&alloc::format!(
            "hexdump: {}: offset {} at/past EOF ({} byte(s)) — nothing to dump", path, off, size));
    }
    let want = core::cmp::min(
        core::cmp::min(len, HEXDUMP_MAX) as u64,
        size - off as u64,
    ) as usize;
    let data = match mt.read(&path, off as u64, want) {
        Ok(d) => d,
        Err(e) => return vfs_fail(console, "hexdump", &path, e),
    };
    if data.is_empty() {
        return console.println(&alloc::format!("hexdump: {}: 0 byte(s) read", path));
    }
    xd_rows(console, off as usize, &data);
    let shown_end = off as u64 + data.len() as u64;
    if size > shown_end {
        console.println(&alloc::format!("[... {} more byte(s)]", size - shown_end));
    }
}

/// JD19 `stat <path>`: one entry's detail, AS THE NAMESPACE KNOWS IT — the canonical absolute path,
/// the VOLUME that answered, kind, size, and the last-write stamp when the medium carries one.
///
/// **What it no longer prints, and why that is the arc working rather than a loss.** The pre-VFSROUTE
/// verb printed the FAT attribute byte, the first cluster and the directory-entry LBA + slot offset.
/// Those are FAT on-disk facts: a `stat` that printed them for every volume would either be a FAT
/// verb wearing a general name, or would need the trait to carry FAT's own on-disk layout into every backend.
/// The forensic pair that answers the same questions is unchanged and volume-honest — `hexdump` for
/// a file's bytes, `dd if=<lba>` for a raw sector.
fn fs_stat(console: &mut Console, arg: &str) {
    use crate::fs::vfs::NodeKind;
    let Some((mt, path)) = vfs_read_open(console, "stat", arg) else { return };
    let st = match mt.stat(&path) {
        Ok(s) => s,
        Err(e) => return vfs_fail(console, "stat", &path, e),
    };
    let volume = mt.volume_name(&path).unwrap_or_else(|_| String::from("?"));
    // The stamp lives on the LISTING row, not on `Stat` (a medium with no timestamp answers `None`
    // rather than fabricating one), so read the parent directory and find this leaf in it.
    let mtime = if path == "/" {
        None
    } else {
        mt.read_dir(&vfs_parent(&path)).ok().and_then(|rows| {
            rows.into_iter()
                .find(|r| r.name.eq_ignore_ascii_case(vfs_leaf(&path)))
                .and_then(|r| r.mtime)
        })
    };
    console.println(&alloc::format!("  path:   {}", path));
    console.println(&alloc::format!("  volume: {}", volume));
    console.println(&alloc::format!(
        "  kind:   {}", if matches!(st.kind, NodeKind::Dir) { "dir" } else { "file" }));
    console.println(&alloc::format!("  size:   {} byte(s)", st.size));
    console.println(&alloc::format!("  mtime:  {}", vfs_mtime_field(mtime.as_ref()).trim()));
    if path == "/" {
        console.println("  entry:  the volume root has no directory entry of its own");
    }
}

/// JD18 running tally for `find`: hits printed, and directories scanned.
struct FindStats {
    matches: u32,
    dirs: u32,
}

/// JD18: recursively walk `dir`, matching each entry's name against `pat` with the JD12
/// `glob_match`. A hit prints its full path — a directory with a trailing `/`. Depth-capped at
/// [`CP_MAX_DEPTH`] (`-ELOOP`); a read error stops with an errno-tagged path and leaves the
/// already-printed hits standing.
fn find_walk(
    console: &mut Console,
    mt: &crate::fs::vfs::MountTable,
    dir: &str,
    pat: &str,
    depth: u32,
    stats: &mut FindStats,
) -> Result<(), String> {
    use crate::fs::vfs::NodeKind;
    if depth > CP_MAX_DEPTH {
        return Err(alloc::format!(
            "{}: maximum directory depth {} exceeded (-ELOOP)", dir, CP_MAX_DEPTH));
    }
    stats.dirs += 1;
    let rows = mt
        .read_dir(dir)
        .map_err(|e| alloc::format!("{}: {}", dir, vfs_err(e)))?;
    for r in &rows {
        let child = vfs_join(dir, &r.name);
        if glob_match(pat, &r.name) {
            stats.matches += 1;
            if matches!(r.kind, NodeKind::Dir) {
                console.println(&alloc::format!("{}/", child));
            } else {
                console.println(&child);
            }
        }
        if matches!(r.kind, NodeKind::Dir) {
            find_walk(console, mt, &child, pat, depth + 1, stats)?;
        }
    }
    Ok(())
}

/// JD18 `find [root] <pattern>`: recursively search the tree under `<root>` (default the cwd) for
/// entries whose name matches `<pattern>`, then an honest `N match(es), M dir(s) scanned` tally. A
/// FILE root degrades to a single self-match test (the POSIX shape).
fn fs_find(console: &mut Console, root_arg: &str, pat: &str) {
    use crate::fs::vfs::NodeKind;
    let Some((mt, path)) = vfs_read_open(console, "find", root_arg) else { return };
    let st = match mt.stat(&path) {
        Ok(s) => s,
        Err(e) => return vfs_fail(console, "find", &path, e),
    };
    let mut stats = FindStats { matches: 0, dirs: 0 };
    if matches!(st.kind, NodeKind::File) {
        if glob_match(pat, vfs_leaf(&path)) {
            console.println(&path);
            stats.matches += 1;
        }
        return console.println(&alloc::format!(
            "{} match(es), 0 dir(s) scanned", stats.matches));
    }
    if let Err(msg) = find_walk(console, &mt, &path, pat, 1, &mut stats) {
        console.println(&alloc::format!("find: {}", msg));
    }
    console.println(&alloc::format!(
        "{} match(es), {} dir(s) scanned", stats.matches, stats.dirs));
}

/// JD18 running tally for `du`: files and directories counted across the whole subtree.
struct DuStats {
    files: u32,
    dirs: u32,
}

/// JD18: total bytes of the subtree rooted at `dir` — the sum of every descendant FILE's size.
/// Depth-capped at [`CP_MAX_DEPTH`]; a read error stops with an errno-tagged path.
fn du_subtree(
    mt: &crate::fs::vfs::MountTable,
    dir: &str,
    depth: u32,
    stats: &mut DuStats,
) -> Result<u64, String> {
    use crate::fs::vfs::NodeKind;
    if depth > CP_MAX_DEPTH {
        return Err(alloc::format!(
            "{}: maximum directory depth {} exceeded (-ELOOP)", dir, CP_MAX_DEPTH));
    }
    let rows = mt
        .read_dir(dir)
        .map_err(|e| alloc::format!("{}: {}", dir, vfs_err(e)))?;
    let mut total: u64 = 0;
    for r in &rows {
        if matches!(r.kind, NodeKind::Dir) {
            stats.dirs += 1;
            total += du_subtree(mt, &vfs_join(dir, &r.name), depth + 1, stats)?;
        } else {
            stats.files += 1;
            total += r.size;
        }
    }
    Ok(total)
}

/// JD18 `du [dir]`: for each DIRECT child of `<dir>` print its total bytes (a file = its own size, a
/// directory = the recursive sum of its subtree), then a `total:` line. `du FILE` is that file's
/// single line. A directory entry itself contributes no bytes — only file bytes are real.
fn fs_du(console: &mut Console, arg: &str) {
    use crate::fs::vfs::NodeKind;
    let Some((mt, path)) = vfs_read_open(console, "du", arg) else { return };
    let st = match mt.stat(&path) {
        Ok(s) => s,
        Err(e) => return vfs_fail(console, "du", &path, e),
    };
    if matches!(st.kind, NodeKind::File) {
        console.println(&alloc::format!("  {:>10}  {}", st.size, path));
        return console.println(&alloc::format!(
            "total: {} byte(s) in 1 file(s), 0 dir(s)", st.size));
    }
    let rows = match mt.read_dir(&path) {
        Ok(r) => r,
        Err(e) => return vfs_fail(console, "du", &path, e),
    };
    let mut stats = DuStats { files: 0, dirs: 0 };
    let mut grand: u64 = 0;
    for r in &rows {
        let child = vfs_join(&path, &r.name);
        if matches!(r.kind, NodeKind::Dir) {
            stats.dirs += 1;
            match du_subtree(&mt, &child, 1, &mut stats) {
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
            grand += r.size;
            console.println(&alloc::format!("  {:>10}  {}", r.size, child));
        }
    }
    console.println(&alloc::format!(
        "total: {} byte(s) in {} file(s), {} dir(s)", grand, stats.files, stats.dirs));
}

/// VFSROUTE: read a bounded run of a file in windows, feeding `sink` — the streaming core `wc` and
/// `grep` share, so neither holds a whole file. Returns the number of bytes scanned.
fn scan_file(
    mt: &crate::fs::vfs::MountTable,
    path: &str,
    size: u64,
    mut sink: impl FnMut(&[u8]),
) -> Result<u64, crate::fs::vfs::VfsError> {
    const WINDOW: usize = 4096;
    let cap = core::cmp::min(size, SCAN_MAX as u64);
    let mut off: u64 = 0;
    while off < cap {
        let want = core::cmp::min(WINDOW as u64, cap - off) as usize;
        let buf = mt.read(path, off, want)?;
        if buf.is_empty() {
            break;
        }
        sink(&buf);
        off += buf.len() as u64;
    }
    Ok(off)
}

/// BASICS: resolve one path argument to a readable FILE for `wc`/`grep`, wearing the caller's verb
/// name so the error lines stay in house style.
fn scan_target(console: &mut Console, verb: &str, arg: &str)
    -> Option<(crate::fs::vfs::MountTable, String, u64)>
{
    vfs_file_target(console, verb, arg)
}

/// RELICS/VFSROUTE `df` and `mount`: what is attached, and how full it is.
///
/// **One row per MOUNT, and every field comes off the backend.** `df` used to mount the FAT program
/// source and print that one volume's BPB fields; a machine with three volumes in one namespace got
/// a report about whichever the program source happened to be. Now the table itself is the report:
/// the prefix and volume name off [`crate::fs::vfs::MountTable::rows`], the capacity off
/// `VfsBackend::volume_bytes` (a backend whose medium publishes none answers `None` and the column
/// is a dash — never a fabricated figure), the access posture off `VfsBackend::write_veto`, and the
/// per-volume geometry line, under `mount` only, off `VfsBackend::describe`.
///
/// **`used` is a file-byte tally, and the line says so.** No backend publishes a free-block count,
/// so `used` is the recursive sum of every file's size — the number `du` prints for that volume's
/// root, computed by the same walker — and `free` is derived from it. That undercounts by per-file
/// slack, so the footer names the method rather than letting a reader assume a block-accurate figure.
fn df_report(console: &mut Console, verb: &str) {
    let mt = vfs_mount_table();
    let rows = mt.rows();
    if rows.is_empty() {
        console.println(&alloc::format!("{}: no filesystem mounted (-ENODEV)", verb));
        serial_println!(
            ":: [fatverb] {} -> NO VOLUME (namespace empty; handles={}) ::",
            verb, crate::drivers::block::source_census()
        );
        return;
    }
    console.println("Volume      Prefix      Size(KiB)  Used(KiB)  Free(KiB)  Access");
    let mut descriptions: Vec<String> = Vec::new();
    // One TALLY per distinct VOLUME, not per mount: a machine that binds one volume at two prefixes
    // (x86's `/` + `/fat`, the Orin's ROOTFS pair) would otherwise walk the whole card twice to
    // print the same number twice. The rows still list every mount — a mount point is a fact — they
    // just share the walk.
    let mut tallied: Vec<(String, u64, u32, u32)> = Vec::new();
    for (prefix, name, veto, total, describe) in &rows {
        let (used, files, dirs) = match tallied.iter().find(|(n, _, _, _)| n == name) {
            Some((_, u, f, d)) => (*u, *f, *d),
            None => {
                let mut st = DuStats { files: 0, dirs: 0 };
                let u = du_subtree(&mt, prefix, 0, &mut st).unwrap_or(0);
                tallied.push((String::from(*name), u, st.files, st.dirs));
                (u, st.files, st.dirs)
            }
        };
        let stats = DuStats { files, dirs };
        let size_col = match total {
            Some(t) => alloc::format!("{}", t / 1024),
            None => String::from("-"),
        };
        let free_col = match total {
            Some(t) => alloc::format!("{}", t.saturating_sub(used) / 1024),
            None => String::from("-"),
        };
        console.println(&alloc::format!(
            "{:<11} {:<11} {:>9}  {:>9}  {:>9}  {}",
            name, prefix, size_col, used / 1024, free_col,
            veto.unwrap_or("read-write")));
        console.println(&alloc::format!(
            "            {} file(s), {} dir(s)", stats.files, stats.dirs));
        // RELICS (R26 clause 1): the retired `fatinfo` verb was one line of volume GEOMETRY, and
        // geometry is a property of a MOUNT — so it prints under `mount`, not under `df`, and it
        // comes from the backend describing ITSELF. A volume that publishes no description simply
        // contributes no line.
        if verb == "mount" {
            if let Some(d) = describe {
                descriptions.push(alloc::format!("{}: {}", name, d));
            }
        }
    }
    console.println("used = recursive file-byte tally, slack not counted");
    for d in &descriptions {
        console.println(d);
    }
}

/// VFSROUTE: expand ONE path argument's trailing glob against the namespace. `Literal` = no
/// metacharacter in the leaf (the arg passes through unchanged); `Matched` = the leaf resolved
/// against its parent DIRECTORY to zero or more rows, sorted so a listing or transcript is
/// deterministic. A metacharacter in an earlier component is treated literally.
///
/// It walks [`crate::fs::vfs::MountTable::read_dir`], so a glob works on EVERY mounted volume. The
/// pre-VFSROUTE expander took a `&FatFs`, which is why `cat *.TXT` was FAT-only and silently found
/// nothing on the native volume.
enum Glob {
    Literal(String),
    Matched { parent: String, rows: Vec<crate::fs::vfs::DirEnt> },
}

/// VFSROUTE: the glob expander (see [`Glob`]). `path` is already absolute and normalized.
fn vfs_glob(mt: &crate::fs::vfs::MountTable, path: &str) -> Glob {
    let leaf = vfs_leaf(path);
    if leaf.is_empty() || !has_glob(leaf) {
        return Glob::Literal(String::from(path));
    }
    let parent = vfs_parent(path);
    let mut rows: Vec<crate::fs::vfs::DirEnt> = match mt.read_dir(&parent) {
        Ok(rs) => rs.into_iter().filter(|r| glob_match(leaf, &r.name)).collect(),
        Err(_) => Vec::new(),
    };
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Glob::Matched { parent, rows }
}

/// VFSROUTE: render listing rows in the shell's table shape — size + name, `<DIR>` for a directory,
/// and under `-l` the date column [`vfs_mtime_field`] renders. Returns `(files, dirs)`.
fn vfs_print_rows(console: &mut Console, rows: &[crate::fs::vfs::DirEnt], long: bool) -> (u32, u32) {
    use crate::fs::vfs::NodeKind;
    let (mut files, mut dirs) = (0u32, 0u32);
    for de in rows {
        let date = vfs_mtime_field(de.mtime.as_ref());
        if matches!(de.kind, NodeKind::Dir) {
            dirs += 1;
            if long {
                console.println(&alloc::format!("  <DIR>        {}  {}/", date, de.name));
            } else {
                console.println(&alloc::format!("  <DIR>         {}", de.name));
            }
        } else {
            files += 1;
            if long {
                console.println(&alloc::format!("  {:>10}  {}  {}", de.size, date, de.name));
            } else {
                console.println(&alloc::format!("  {:>10}  {}", de.size, de.name));
            }
        }
    }
    (files, dirs)
}

/// VFSROUTE: `ls`/`dir` — ONE body, every volume, both arches.
///
/// Peter's question was about exactly this function: a mounted filesystem is listable because it
/// implements [`crate::fs::vfs::VfsBackend::read_dir`], and `ls` walks whatever the mount table
/// hands it. There is no unafs arm, no FAT arm and no `/usb` prefix test in here; `ls /` lists the
/// root volume, `ls /fat` the boot FAT, `ls /usb` the stick, and a volume mounted tomorrow lists
/// with no edit to this file at all.
///
/// Emits the per-invocation `:: ls1: <path>: <names> (N file, M dir) ::` serial witness unchanged —
/// the verb renders panel-only on the bench, so a headless capture gets the same content.
fn vfs_ls(console: &mut Console, arg: &str, long: bool) {
    let path = vfs_path(arg);
    match vfs_ls_collect(&path) {
        Ok((is_dir, rows)) => {
            let (files, dirs) = vfs_print_rows(console, &rows, long);
            if is_dir {
                console.println(&alloc::format!("{} file(s), {} dir(s)", files, dirs));
            }
            let names: Vec<&str> = rows.iter().map(|d| d.name.as_str()).collect();
            if long {
                let sizes: Vec<String> = rows.iter().map(|d| alloc::format!("{}", d.size)).collect();
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

/// VFSROUTE: the single-path `ls` core under its FATVERB name — kept because
/// `fatverb_storage_witness` drives THIS function as "the real read verb the `ls`/`dir` arm calls",
/// and that leg's whole value is that it exercises the verb rather than a copy of it.
#[cfg(target_arch = "x86_64")]
fn ls_path(console: &mut Console, arg: &str, long: bool) {
    vfs_ls(console, arg, long);
}

/// JD12 `ls *.EXT`: list every entry matching a wildcard, one table line each (sorted), with the
/// file/dir tally. No match is an honest "no match".
fn ls_globbed(console: &mut Console, arg: &str, long: bool) {
    let Some((mt, path)) = vfs_read_open(console, "ls", arg) else { return };
    match vfs_glob(&mt, &path) {
        Glob::Literal(_) => vfs_ls(console, arg, long),
        Glob::Matched { rows, .. } if rows.is_empty() =>
            console.println(&alloc::format!("ls: {}: no match", arg)),
        Glob::Matched { rows, .. } => {
            let (files, dirs) = vfs_print_rows(console, &rows, long);
            console.println(&alloc::format!("{} file(s), {} dir(s)", files, dirs));
        }
    }
}

/// JD12 `cat *.EXT`: cat every FILE matching a wildcard (concatenate), in sorted order — reusing
/// [`vfs_cat`] per file so the rendering and truncation note are identical to a single-path `cat`.
fn cat_globbed(console: &mut Console, arg: &str) {
    use crate::fs::vfs::NodeKind;
    let Some((mt, path)) = vfs_read_open(console, "cat", arg) else { return };
    match vfs_glob(&mt, &path) {
        Glob::Literal(_) => vfs_cat(console, arg),
        Glob::Matched { rows, .. } if rows.is_empty() =>
            console.println(&alloc::format!("cat: {}: no match", arg)),
        Glob::Matched { parent, rows } => {
            for r in &rows {
                let child = vfs_join(&parent, &r.name);
                if matches!(r.kind, NodeKind::Dir) {
                    console.println(&alloc::format!("cat: {}: is a directory (-EISDIR)", child));
                } else {
                    vfs_cat(console, &child);
                }
            }
        }
    }
}

/// JD12: expand the SOURCE args of a `cp`/`mv` into concrete absolute paths, printing a per-pattern
/// "no match" note for any wildcard that matched nothing. SNAPSHOT: the whole list is taken before
/// any mutation runs, so a wildcard operation never invalidates its own list.
fn expand_sources(
    console: &mut Console,
    mt: &crate::fs::vfs::MountTable,
    verb: &str,
    sources: &[&str],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in sources {
        match vfs_glob(mt, &vfs_path(s)) {
            Glob::Literal(p) => out.push(p),
            Glob::Matched { rows, .. } if rows.is_empty() =>
                console.println(&alloc::format!("{}: {}: no match", verb, s)),
            Glob::Matched { parent, rows } => {
                for r in &rows {
                    out.push(vfs_join(&parent, &r.name));
                }
            }
        }
    }
    out
}

/// JD12 `rm` with wildcards: each arg a target or a trailing glob. SNAPSHOT-then-delete. A
/// no-match wildcard is quiet under `-f` (POSIX `rm -f *.none` is silent).
fn rm_globbed(console: &mut Console, args: &[&str], recursive: bool, force: bool) {
    let mt = vfs_mount_table();
    for a in args {
        match vfs_glob(&mt, &vfs_path(a)) {
            Glob::Literal(p) =>
                if recursive { fs_rm_recursive(console, &p, force) } else { fs_rm(console, &p, force) },
            Glob::Matched { rows, .. } if rows.is_empty() => {
                if !force {
                    console.println(&alloc::format!("rm: {}: no match", a));
                }
            }
            Glob::Matched { parent, rows } => {
                for r in &rows {
                    let path = vfs_join(&parent, &r.name);
                    if recursive { fs_rm_recursive(console, &path, force) } else { fs_rm(console, &path, force) }
                }
            }
        }
    }
}

/// JD12 `cp` with wildcards / multiple sources. With more than one source the destination MUST be
/// an existing directory (several files can only land INTO a directory). SNAPSHOT-then-copy.
fn cp_globbed(console: &mut Console, sources: &[&str], dst: &str, recursive: bool, force: bool) {
    let mt = vfs_mount_table();
    let srcs = expand_sources(console, &mt, "cp", sources);
    if srcs.is_empty() {
        return; // every pattern was empty (each already reported "no match")
    }
    if srcs.len() > 1 && !vfs_is_dir(&mt, &vfs_path(dst)) {
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

/// JD12 `mv` with wildcards / multiple sources. With more than one source the destination MUST be
/// an existing directory. SNAPSHOT-then-move.
fn mv_globbed(console: &mut Console, sources: &[&str], dst: &str, force: bool) {
    let mt = vfs_mount_table();
    let srcs = expand_sources(console, &mt, "mv", sources);
    if srcs.is_empty() {
        return;
    }
    if srcs.len() > 1 && !vfs_is_dir(&mt, &vfs_path(dst)) {
        return console.println(&alloc::format!("mv: target {}: not a directory (-ENOTDIR)", dst));
    }
    for s in &srcs {
        fs_mv(console, s, dst, force);
    }
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



/// JD13: a running tally of a `rm -r` for the summary / partial-failure report (the delete twin of
/// `CpStats`, no byte count — a delete moves no data).
struct RmStats {
    dirs: u32,
    files: u32,
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





/// JD17: parse the `date -s` argument pair — `YYYY-MM-DD HH:MM[:SS]`. Strict shapes (dash- and
/// colon-separated decimal fields, seconds optional and defaulting to 0); range validation is
/// `clock::set`'s (`WallTime::is_valid`), so this only has to produce the numbers honestly.
fn parse_wallclock(args: &[&str]) -> Option<crate::clock::WallTime> {
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



// ---------------------------------------------------------------------------------------------
// THE LISTING SEAM — `ls` lists WHATEVER IS MOUNTED, and knows nothing about filesystems.
//
// History, because it is the whole lesson. PI-SHELL-LS pointed the Pi's `ls` at the native unafs
// volume while x86's stayed on FAT, which fixed the board and entrenched the defect: one verb, two
// bodies, each naming a filesystem. VFS-1 replaced the aarch64 half's two per-volume collectors with
// the mount table. VFSROUTE (orin 17) finished it — Peter: *"Should a mounted file system not be
// listable? Sounds like we'll be adding each filesystem to ls so it lists."* There is now ONE
// collector, on both arches, and it asks `MountTable::read_dir`; a volume is listable because its
// backend implements the trait, and a volume mounted tomorrow lists with no edit here.
// ---------------------------------------------------------------------------------------------

/// VFS-1 (adoption): list `path` **through the mount table** — the ONE collector behind every
/// `ls` on this arch, replacing the two per-volume collectors (`pi_ls_collect` against unafs and
/// `pi_usb_ls_collect` against the USB FAT) that the verb used to choose between with a hand-rolled
/// `/usb` prefix test. The volume is now decided by [`MountTable::resolve`] — the same longest-prefix
/// rule `run`, `bg` and `mount` already obey — so `ls /` lists native UnaFS, `ls /fat` the SD boot FAT
/// and `ls /usb` the stick, with no verb-side dispatch and no volume the verb has to know about.
///
/// Returns `(is_dir, rows)` sorted by name. A directory yields its entries; a plain file yields its
/// own single row (the DOS `ls <file>` idiom), synthesized from `stat` since a file has no listing.
/// Mount points that sit immediately below `path` are appended as directory rows, so `ls /` still
/// advertises `usb/` (and now `fat/`) the way the `/fs/` HTTP listing does — but as a fact READ OFF
/// THE MOUNT TABLE rather than a `usb_info()` probe wired only for that one volume.
///
/// Any resolve/mount failure surfaces as the errno-tagged message [`vfs_err`] renders, so the three
/// volumes report failures in one vocabulary instead of three.
#[allow(clippy::type_complexity)]
pub(crate) fn vfs_ls_collect(path: &str) -> Result<(bool, Vec<crate::fs::vfs::DirEnt>), String> {
    use crate::fs::vfs::{DirEnt, NodeKind};
    let mt = vfs_mount_table();
    // VFS-4: a path naming a reserved volume that is not currently bound reports the VOLUME as
    // missing, not a bare -ENOENT off the native root. `ls` shares the guard the mutating `mount`
    // verb has had since VFS-4 rather than re-deriving it.
    if let Some(vol) = unmounted_reserved_volume(&mt.prefixes(), path) {
        return Err(alloc::format!("volume {} not mounted (-ENODEV)", vol));
    }
    let st = mt.stat(path).map_err(|e| alloc::format!("{}: {}", path, vfs_err(e)))?;
    if !matches!(st.kind, NodeKind::Dir) {
        let leaf = String::from(path.rsplit('/').next().unwrap_or(path));
        return Ok((false, alloc::vec![DirEnt {
            name: leaf,
            kind: NodeKind::File,
            size: st.size,
            mtime: None,
        }]));
    }
    let mut rows = mt
        .read_dir(path)
        .map_err(|e| alloc::format!("{}: {}", path, vfs_err(e)))?;
    // Mount points immediately below `path` — `/fat` and `/usb` when listing `/`. Boundary-matched
    // the way the resolver matches, and only for prefixes that are actually bound, so an absent
    // stick contributes no row (honest hot-plug, doc §6).
    let base = if path == "/" { "" } else { path };
    for pfx in mt.prefixes() {
        if pfx == "/" {
            continue;
        }
        if let Some(tail) = pfx.strip_prefix(base) {
            let name = tail.trim_start_matches('/');
            if !name.is_empty() && !name.contains('/') && !rows.iter().any(|r| r.name == name) {
                rows.push(DirEnt {
                    name: String::from(name),
                    kind: NodeKind::Dir,
                    size: 0,
                    mtime: None,
                });
            }
        }
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((true, rows))
}

/// VFS-1 (adoption): render a [`DirEnt`](crate::fs::vfs::DirEnt)'s last-write stamp as the
/// fixed-width `YYYY-MM-DD HH:MM:SS` field the `ls -l` date column uses (mirrors genet's
/// `fmt_fat_mtime` so the shell and `/fs/usb/` never disagree). `None` — a medium with no stamp
/// (native UnaFS) or a FAT entry whose on-disk field was all-zero — renders as a 19-char dashed
/// placeholder rather than a fabricated 1980 date. ONE formatter for all three volumes: before this
/// arc the FAT path had `fat_mtime_field` and the unafs path had a separate `UNAFS_NO_MTIME`
/// constant, and the verb picked between them by knowing which volume it was on.
fn vfs_mtime_field(ts: Option<&crate::fs::vfs::VfsTime>) -> String {
    match ts {
        None => String::from("       -           "),
        Some(t) => alloc::format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            t.year, t.month, t.day, t.hour, t.min, t.sec
        ),
    }
}


/// PI-SHELL-LS boot witness (`witness` battery only): exercise the exact `vfs_ls_collect` listing
/// the shell verb uses, against the native root, and emit the `:: ls1: ... ::` line headlessly — so
/// `UNAOS_PI=1 ./arroyo kernel8-test` proves `ls` works without a serial-console injection path.
/// Quiet default boots never compile this. Baremetal-gated to match the emmc2 backend the volume
/// rides. VFS-1 (adoption): the collector is now the mount-table one, so this witness proves the
/// ROUTED listing rather than a unafs-direct call the verb no longer makes.
#[cfg(all(target_arch = "aarch64", feature = "baremetal", feature = "witness"))]
pub fn pi_ls_witness() {
    vfs_ls_say("/");
}

/// PI-FS-5 boot/hot-plug witness: exercise the EXACT `vfs_ls_collect` listing the shell's `ls /usb`
/// verb uses, against the live USB FAT mount, and emit the `:: ls1: /usb... ::` line headlessly — so a
/// capture proves the shell sees the same volume `/fs/usb` serves, without a serial-console injection
/// path. Called from `fat::piusb27_mount_witness` (which fires once per bring-up + every hot-plug), so it
/// rides the same USB feature gate as the mount witness — NOT the baremetal/witness battery (the USB FAT
/// volume is present in `UNAOS_FATIMG=1 ./arroyo test-arm`, where the emmc2-backed unafs volume is not).
/// Lists the `/usb` root then descends one named subdir to prove the LFN-aware subpath walk.
#[cfg(target_arch = "aarch64")]
pub fn pi_usb_ls_witness() {
    vfs_ls_say("/usb");
    vfs_ls_say("/usb/SUBDIR");
}

/// VFS-1 (adoption): the ONE headless `:: ls1: ... ::` emitter both listing witnesses now share —
/// collect through the mount table, render names (a directory shown with a trailing `/`) and the
/// file/dir tally. Both witnesses previously carried their own copy of this loop against their own
/// volume's collector, which is precisely the duplication the routing seam removes.
#[cfg(target_arch = "aarch64")]
fn vfs_ls_say(path: &str) {
    use crate::fs::vfs::NodeKind;
    match vfs_ls_collect(path) {
        Ok((_, rows)) => {
            let names: Vec<String> = rows
                .iter()
                .map(|d| {
                    if matches!(d.kind, NodeKind::Dir) {
                        alloc::format!("{}/", d.name)
                    } else {
                        d.name.clone()
                    }
                })
                .collect();
            let dirs = rows.iter().filter(|d| matches!(d.kind, NodeKind::Dir)).count();
            let files = rows.len() - dirs;
            serial_println!(
                ":: ls1: {}: {} ({} file, {} dir) ::",
                path, names.join(" "), files, dirs
            );
        }
        Err(msg) => serial_println!(":: ls1: {}: ERR {} ::", path, msg),
    }
}

// ---------------------------------------------------------------------------------------------
// JD12 — wildcard globbing (`ls *.C`, `cat *.MD`, `rm *.TXT`, `cp *.TXT DIR/`, `mv *.LOG ARCH/`).
//
// A single TRAILING glob in a path's LAST component is expanded against the parent directory via the
// read-only `read_dir` (case-insensitive 8.3 matching, already proven for `cd`/`cat`). Expansion is
// invoked ONLY inside the fs-verb arms below — the shared arg-split at the top of `dispatch_command`
// is unchanged, and the NET command region (ifconfig/ping/arp/nc/curl — a sockets-arc
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







// ---------------------------------------------------------------------------------------------
// JD19 — read-only forensic verbs: `stat` (one entry's full on-disk detail) and `hexdump`
// (bounded
// hexdump). Both are `shell.rs`-only, ride the existing public fat.rs API call-never-edit, and never
// mutate: `stat` composes resolve_path/locate_in_dir plus one raw `block::read_block` of the on-disk
// directory sector for the true attr byte (the parsed DirEntry keeps only `is_dir`); `hexdump` streams a
// bounded window through the offset-aware `read_at`. Neither is glob-wired (single path) — a
// metacharacter resolves literally, an honest `-ENOENT`, the same as a mid-path glob today.



/// JD19: hexdump `data` with each row labelled by its ABSOLUTE file offset (`base` + row start), in
/// the canonical `OFFSET: <16 hex bytes> | <ascii> |` layout (non-printables render as `.`). Distinct
/// from the `dd`-verb `hexdump` (which labels rows from 0 and dumps a fixed 128 bytes): the file
/// dump needs
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
    #[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
    let (proc_verbs, proc_rows) = (true, crate::arch::syscall::proc_table_rows());
    #[cfg(not(any(all(any(feature = "baremetal", feature = "tegra_el0", feature = "virt_el0"), target_arch = "aarch64"), target_arch = "x86_64")))] // EL0-NAMING: NEGATED/RUNTIME — KEPT LONGHAND ON PURPOSE. Cargo feature implication is ONE-WAY: `baremetal`/`tegra_el0` imply `aarch64_el0`, not the reverse, so `not(aarch64_el0)` would diverge from this predicate for anyone who enabled `aarch64_el0` ALONE. No gate leg builds that combination, which is the trap — a byte-identity check over the legs would PASS while the hazard shipped. Positive sites are safe because implication runs their way; these are not.
    let (proc_verbs, proc_rows) = (false, 0usize);
    midden_core::Facts {
        aarch64: cfg!(target_arch = "aarch64"),
        x86: cfg!(target_arch = "x86_64"),
        v3d: cfg!(all(target_arch = "aarch64", feature = "v3d")),
        // BARENAME (§6.6a): the knob the `vug`/`pulse` match arms actually carry. It is a pure
        // `feature` read — `Avail::VugDemo` composes it with `aarch64` — so this line says one
        // thing and says it truthfully: DEFAULT OFF, and off means the words are not verbs.
        vugdemo: cfg!(feature = "vugdemo"),
        proc_verbs,
        proc_rows,
        // BARENAME (PARITY §6.6a): bare-name launch exists exactly where the PROCESS VERBS exist.
        // `spawn_user_image_bg` + the shell job table are the whole dependency and they carry that
        // very gate, so this reads the flag off `proc_verbs` instead of re-deriving a second
        // `cfg!(target_arch = ...)` beside it — the ONE-OS law forbids an arch gate in the
        // experience layer, and there is only one fact here: "this build can start a ring-3
        // program". A build with no process table still leaves it false, and there the core never
        // probes the volume and never returns `Plan::Exec`.
        exec: proc_verbs,
    }
}



/// VFSROUTE (orin 17): does `path` name a plain FILE on the exec probe's OWN FAT mount?
///
/// **The one FAT-direct walk left in this file, and it is not a verb.** Every file verb goes through
/// the mount table now; this predicate exists solely for [`FatVolume::is_file`], the x86 exec probe,
/// which must bind `mount_program_source` ITSELF and stamp `EXEC_BIND` from that binding.
/// `fatverb_storage_witness` compares that stamp with the one a real read verb leaves, and the leg's
/// whole value is that the two are INDEPENDENT producers — route the probe through
/// `vfs_mount_table()` as well and the comparison becomes an expression compared with itself, which
/// is the defect FATVERB's own note records the first cut of that witness making.
///
/// Case-insensitive 8.3 matching, component by component, exactly as the resolver it replaces did;
/// it returns a bool because a bool is the entire question `Volume::is_file` asks.
#[cfg(target_arch = "x86_64")]
fn fat_path_is_file(fs: &FatFs, path: &str) -> bool {
    let mut cluster = 0u32; // 0 = the root (read_dir's convention)
    let mut cur: Option<DirEntry> = None;
    for comp in path.split('/').filter(|c| !c.is_empty()) {
        if let Some(de) = &cur {
            if !de.is_dir {
                return false; // a component below a plain file is not a path
            }
            cluster = de.first_cluster();
        }
        let entries = match fs.read_dir(cluster) {
            Ok(e) => e,
            Err(_) => return false,
        };
        match entries.iter().find(|de| de.name().eq_ignore_ascii_case(comp)) {
            Some(de) => cur = Some(*de),
            None => return false,
        }
    }
    matches!(cur, Some(de) if !de.is_dir) // the volume root is not a file
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
        fat_path_is_file(&fs, &normalize_path(&cwd_path(), name))
    }
    /// BARENAME (PARITY §6.6a): the aarch64 twin — the SAME question, asked of the namespace this
    /// arch actually has.
    ///
    /// aarch64 is not x86 with a different mnemonic set here: x86 has no VFS, so its whole path
    /// universe IS the program-source FAT and "resolve from the cwd" already means "resolve on the
    /// volume executables live on". On the Pi those are two different statements — `/` is native
    /// UnaFS and the executables are on `/fat` — so the faithful port is not the x86 code with the
    /// mount swapped, it is [`exec_resolve`]: the cwd first (so `ls`/`cat`/`run` and a bare name
    /// agree about what a name means, VFS-1's whole point), then the program-source root. See
    /// `exec_resolve` for the order and why it is the same order.
    ///
    /// No [`EXEC_BIND`] stamp: that instrument and its `fatverb_storage_witness` reader are x86-only
    /// (they compare FAT *handles*, and this arch binds a mount table, not a handle).
    #[cfg(all(feature = "aarch64_el0", target_arch = "aarch64"))]
    fn is_file(&mut self, name: &str) -> bool {
        exec_resolve(name).is_some()
    }
    // No process table, no loader, so `Facts::exec` is false and the core never calls this.
    // Answering `false` keeps the promise: no behaviour change on a build that cannot launch.
    #[cfg(not(any(target_arch = "x86_64", all(feature = "aarch64_el0", target_arch = "aarch64"))))]
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

// ==================== BASICS — the everyday verbs, back in the table ==============================
//
// Peter, 2026-09-06: *"what about the basic shell commands that were removed"* / *"some of the
// commands were 1 off tests we need basic commands back"*.
//
// WHAT WAS ACTUALLY REMOVED, precisely, because it changes what the fix is. `c7b12e23` ("shell: one
// interpreter, and it is midden's") did NOT delete `help`, `echo`, `ver` or `gneiss` — it MOVED them
// out of this file's `match` and into `midden_core`, where the command table lives, and they have
// answered from there ever since. What the shell has never had is the other half of a usable
// prompt: no way to search a file, count it, see how full the volume is, ask what a word means, see
// what was typed, or wait. Those are added here, and two verbs that WERE lost are restored: `umv`
// and `urmattr` shipped `#[cfg(aarch64)]` match arms that no table entry ever pointed at, so the
// operator got "Unknown command" about commands this kernel carries (MIDDEN_CONVERGENCE's
// one-for-one contract, violated in the direction nobody was checking).
//
// WHERE EACH VERB LIVES, per MIDDEN_CONVERGENCE §1(c). A verb belongs in `midden_core` when the
// core can answer it in full — `exit` is there, because the honest answer is a sentence about what
// the shell IS. Everything added below needs the volume, the block layer, the timebase or the
// shell's own state, so each is `Plan::Host`: midden owns the parse and the wording, this file
// performs the call. That is the same split `ls`/`cat`/`head` already sit on.
//
// ARCH-NEUTRAL BY DEFAULT (LAWS), AND SINCE VFSROUTE THERE IS NO EXCEPTION LEFT. Not one of these
// helpers carries a `target_arch` gate: they ride the MOUNT TABLE (`vfs_mount_table` — built on both
// arches now), `crate::arch::ms()` (the arch-neutral timebase both arches publish), and
// `midden_core` itself. The note that used to stand here claimed `crate::fs::vfs` was aarch64-only
// and gated `df`'s namespace line on that; it was wrong about the module (`fs/mod.rs` declares
// `pub mod vfs;` unconditionally — only `NativeBackend` is aarch64) and the gate is gone.

/// BASICS: what the shell was told, oldest first. Bounded, drop-oldest, heap-only.
///
/// Recorded at the TOP of `dispatch_command`, before the line is planned, so the record is of what
/// the operator typed and not of what the shell made of it — a typo is history too, which is the
/// whole reason anyone reads a history. Empty lines are not recorded (they are not commands).
///
/// This is deliberately NOT `Console::history`, which is the SCROLLBACK (every output line the view
/// is holding). Two different questions — "what did I type" vs "what is on screen" — and conflating
/// them is why `history` could not simply read the console.
static CMD_HISTORY: spin::Mutex<Vec<String>> = spin::Mutex::new(Vec::new());

/// BASICS: how many command lines `history` retains. Small and fixed: this is a bench console, the
/// store is heap, and a run-away paste must not be able to grow it without bound.
const HISTORY_CAP: usize = 64;

/// BASICS: the shell's variable store — `set` writes it, `env` reads it.
///
/// **Inert on purpose, and said so in `help`.** Nothing expands `$NAME` yet: expansion is a change
/// to `midden_core`'s parser (M2), not to this file, and a store that silently did nothing while
/// the help text implied substitution would be worse than no store. What it is good for today is
/// what an operator at a bench actually uses it for — writing down a path, an address or a pid
/// between commands, on a machine with no notepad.
static ENV_VARS: spin::Mutex<Vec<(String, String)>> = spin::Mutex::new(Vec::new());

/// BASICS: caps on the variable store. A shell variable is a convenience, not a database.
const ENV_MAX_VARS: usize = 32;
const ENV_MAX_NAME: usize = 32;
const ENV_MAX_VALUE: usize = 256;

/// BASICS: record one typed line in [`CMD_HISTORY`]. Called from `dispatch_command`'s first act.
fn history_record(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let mut h = CMD_HISTORY.lock();
    if h.len() >= HISTORY_CAP {
        h.remove(0);
    }
    h.push(String::from(line));
}

/// BASICS `history [n] [-c]`: the last `n` command lines (default: all retained), numbered from the
/// oldest retained line so the numbers are stable while the ring is not full.
///
/// `history -c` clears the store and says how many it dropped — a count, not silence, because a
/// clear that prints nothing is indistinguishable from a clear that did not run.
fn history_cmd(console: &mut Console, args: &[&str]) {
    if args.first() == Some(&"-c") {
        let mut h = CMD_HISTORY.lock();
        let n = h.len();
        h.clear();
        drop(h);
        return console.println(&alloc::format!("history: cleared {} line(s)", n));
    }
    let want = args.first().and_then(|s| s.parse::<usize>().ok());
    let h = CMD_HISTORY.lock().clone();
    if h.is_empty() {
        return console.println("history: nothing recorded yet");
    }
    let from = match want {
        Some(n) => h.len().saturating_sub(n),
        None => 0,
    };
    for (i, line) in h.iter().enumerate().skip(from) {
        console.println(&alloc::format!("{:>5}  {}", i + 1, line));
    }
}

/// BASICS `env`: the build's own facts first, then the shell variables.
///
/// The facts are READ LIVE at every call (cwd, uptime, the process cap) rather than snapshotted at
/// boot, so `env` after a `cd` says where you are. The version string is not re-typed here — it is
/// asked of `midden_core`, the one place that owns the shell's identity, so `ver` and `env` can
/// never drift into printing two different versions of the same kernel.
fn env_report(console: &mut Console) {
    let facts = midden_facts();
    let empty: &[&str] = &[];
    let mut vol = midden_core::NameList(empty);
    let ver = match midden_core::plan("ver", &facts, &mut vol) {
        midden_core::Plan::Say(m) => String::from(m.text()),
        _ => String::from("(unavailable)"),
    };
    let arch = if facts.aarch64 {
        "aarch64"
    } else if facts.x86 {
        "x86_64"
    } else {
        "unknown"
    };
    console.println(&alloc::format!("ARCH={}", arch));
    console.println(&alloc::format!("VER={}", ver));
    console.println("SHELL=midden_core");
    console.println(&alloc::format!("CWD={}", cwd_path()));
    console.println(&alloc::format!("UPTIME_MS={}", crate::arch::ms()));
    console.println(&alloc::format!("EXEC={}", if facts.exec { "on" } else { "off" }));
    console.println(&alloc::format!(
        "PROCS={}",
        if facts.proc_verbs { facts.proc_rows } else { 0 }
    ));
    console.println(&alloc::format!("V3D={}", if facts.v3d { "on" } else { "off" }));
    console.println(&alloc::format!("VUGDEMO={}", if facts.vugdemo { "on" } else { "off" }));
    let vars = ENV_VARS.lock().clone();
    if vars.is_empty() {
        console.println("(no shell variables set — `set NAME VALUE` to add one)");
        return;
    }
    for (k, v) in &vars {
        console.println(&alloc::format!("{}={}", k, v));
    }
}

/// BASICS `set [NAME [VALUE...]]` / `set -u NAME`: read, write and drop shell variables.
///
/// Shapes, all of them printing what happened rather than succeeding in silence:
/// * `set` — list the variables (not the build facts; `env` is the one that shows both).
/// * `set NAME` — print `NAME=value`, or say it is unset.
/// * `set NAME VALUE...` — set it; the value is the rest of the line, spaces and all.
/// * `set -u NAME` — remove it.
///
/// A name is restricted to ASCII alphanumerics and `_`, which is not fussiness: a name containing
/// `=` or whitespace could never be read back unambiguously by the `$NAME` expansion M2 will add,
/// so accepting one now would be writing a value nobody can ever reference.
fn set_cmd(console: &mut Console, args: &[&str], rest: &str) {
    if args.is_empty() {
        let vars = ENV_VARS.lock().clone();
        if vars.is_empty() {
            return console.println("set: no shell variables (usage: set NAME VALUE)");
        }
        for (k, v) in &vars {
            console.println(&alloc::format!("{}={}", k, v));
        }
        return;
    }
    if args[0] == "-u" {
        let Some(name) = args.get(1) else {
            return console.println("usage: set -u <NAME>");
        };
        let mut vars = ENV_VARS.lock();
        let before = vars.len();
        vars.retain(|(k, _)| k != name);
        let gone = vars.len() != before;
        drop(vars);
        return console.println(&if gone {
            alloc::format!("set: {} removed", name)
        } else {
            alloc::format!("set: {}: not set", name)
        });
    }
    let name = args[0];
    if name.len() > ENV_MAX_NAME
        || name.is_empty()
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return console.println(&alloc::format!(
            "set: {}: bad name (ASCII letters, digits and _ only, max {})", name, ENV_MAX_NAME));
    }
    if args.len() == 1 {
        let vars = ENV_VARS.lock().clone();
        return match vars.iter().find(|(k, _)| k == name) {
            Some((k, v)) => console.println(&alloc::format!("{}={}", k, v)),
            None => console.println(&alloc::format!("set: {}: not set", name)),
        };
    }
    // The value is the remainder of the line after the name, with its interior spacing preserved —
    // the same rule `write`/`append` follow, so `set MSG hello  world` stores what was typed.
    // `rest` arrives from `Plan::Host` already left-trimmed and `name` is its first token, so the
    // slice below is exactly "everything after the name" and needs no search to find it.
    let value = rest[core::cmp::min(name.len(), rest.len())..].trim();
    if value.len() > ENV_MAX_VALUE {
        return console.println(&alloc::format!(
            "set: {}: value too long ({} > {} bytes)", name, value.len(), ENV_MAX_VALUE));
    }
    let mut vars = ENV_VARS.lock();
    if let Some(slot) = vars.iter_mut().find(|(k, _)| k == name) {
        slot.1 = String::from(value);
    } else {
        if vars.len() >= ENV_MAX_VARS {
            drop(vars);
            return console.println(&alloc::format!(
                "set: refused — the variable store is full ({} max); `set -u NAME` frees a slot",
                ENV_MAX_VARS));
        }
        vars.push((String::from(name), String::from(value)));
    }
    drop(vars);
    console.println(&alloc::format!("{}={}", name, value));
}

/// BASICS `which <word>`: what does this word mean to the shell, right now, on THIS build?
///
/// It answers through `midden_core` rather than by looking at this file's `match`, and that is the
/// point: verb-ness is per-build (`vug` is a verb on a `vugdemo` Pi and a program name everywhere
/// else), so any second opinion assembled here would eventually disagree with the interpreter and
/// send an operator hunting for a fault that is only in the help. Same table, same
/// `Avail` filter, same resolver, same precedence — `which` cannot lie unless `plan` lies too.
fn which_report(console: &mut Console, word: &str) {
    let facts = midden_facts();
    let canon = midden_core::canon_verb(word);
    if midden_core::CORE_VERBS.contains(&canon.as_str()) {
        return console.println(&alloc::format!("{}: shell built-in (answered by midden_core)", canon));
    }
    if midden_core::is_verb(&canon, &facts) {
        return console.println(&alloc::format!("{}: kernel verb (serviced by the shell)", canon));
    }
    if facts.exec {
        let mut vol = FatVolume;
        if let Some(name) = midden_core::resolve_exec(word, &mut vol) {
            return console.println(&alloc::format!("{}: program {}", word, name));
        }
        return console.println(&alloc::format!("{}: not found (no verb, no program)", word));
    }
    console.println(&alloc::format!(
        "{}: not a verb on this build (and this build cannot launch programs)", word));
}

/// BASICS `sleep <ms>`: wait, bounded twice over.
///
/// Two independent bounds, because either one alone is a hang. The DEADLINE (`arch::ms()`) is the
/// one that normally ends the wait. The SPIN CAP is what ends it when the deadline never arrives:
/// on x86 `ms()` is the local-APIC tick count, so a caller reached with interrupts masked would
/// watch a frozen clock forever. When the cap is what fired, the verb SAYS SO rather than printing
/// a plausible "slept 500 ms" — a sleep that silently did not sleep is the kind of thing a later
/// timing bug gets blamed on for a week.
///
/// `core::hint::spin_loop()` and not `hlt`/`yield_now`: this runs in three different contexts
/// (the x86 GUI inline loop, which is not a scheduled task; the Orin console pump; the Pi GUI
/// channel task — see `selftest.rs` §context safety), and the spin hint is the only one of the
/// three that is correct in all of them and needs no arch gate to say so.
fn shell_sleep(console: &mut Console, ms: u64) {
    const MAX_MS: u64 = 10_000;
    const SPIN_CAP: u64 = 50_000_000;
    let want = core::cmp::min(ms, MAX_MS);
    if want != ms {
        console.println(&alloc::format!("sleep: {} ms capped to {} ms", ms, MAX_MS));
    }
    let start = crate::arch::ms();
    let mut spins: u64 = 0;
    while crate::arch::ms().saturating_sub(start) < want && spins < SPIN_CAP {
        spins += 1;
        core::hint::spin_loop();
    }
    let elapsed = crate::arch::ms().saturating_sub(start);
    if elapsed < want {
        return console.println(&alloc::format!(
            "sleep: clock did not advance ({} of {} ms after {} spins) — no calibrated timebase here",
            elapsed, want, spins));
    }
    console.println(&alloc::format!("slept {} ms", elapsed));
}

/// BASICS: how many bytes `grep` and `wc` will scan of one file before they stop and say so.
///
/// Sixteen times `head`'s ceiling: those verbs page a window, these ones answer a question ABOUT
/// the whole file, and a count that quietly described the first 64 KiB would be a wrong answer
/// rather than a short one. It is still a ceiling, and both verbs print the truncation note, so the
/// reader is never left to assume.
const SCAN_MAX: u32 = 1024 * 1024;



/// BASICS: count lines, words and bytes over a byte run, carrying the word-boundary state across
/// window edges.
///
/// `in_word` is an in/out parameter and not a local, which is the only interesting thing here: a
/// word split across two 4 KiB reads must be counted ONCE, and a per-window counter would count it
/// twice on every file larger than the window — a bug that never shows up on a small test file.
///
/// Definitions, matching `wc`: a LINE is a `\n` (so a file with no trailing newline reports one
/// fewer line than it has visible rows, exactly as `wc` does); a WORD is a maximal run of
/// non-whitespace; a BYTE is a byte, counted raw and never through `render_text` — the point of
/// `wc -c` is the on-disk size, so a non-printable must not be normalised into a `.` first.
fn wc_accumulate(data: &[u8], in_word: &mut bool, lines: &mut u64, words: &mut u64, bytes: &mut u64) {
    for &b in data {
        *bytes += 1;
        if b == b'\n' {
            *lines += 1;
        }
        let space = matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
        if space {
            *in_word = false;
        } else if !*in_word {
            *in_word = true;
            *words += 1;
        }
    }
}

/// BASICS `wc [-l|-w|-c] <path>`: lines, words and bytes of one file.
///
/// With no flag all three are printed followed by the canonical path, the familiar layout. A single
/// selector prints that one number and the path. Multiple selectors are accepted and print in the
/// fixed l/w/c order rather than in the order typed — `wc` has never promised otherwise, and a
/// column order that depended on the argument order would be unreadable in a capture.
fn fs_wc(console: &mut Console, args: &[&str]) {
    let mut want_l = false;
    let mut want_w = false;
    let mut want_c = false;
    let mut path: Option<&str> = None;
    for a in args {
        if a.starts_with('-') && a.len() > 1 {
            for ch in a[1..].chars() {
                match ch {
                    'l' => want_l = true,
                    'w' => want_w = true,
                    'c' => want_c = true,
                    other => {
                        return console.println(&alloc::format!(
                            "wc: unknown flag -{} (usage: wc [-l|-w|-c] <path>)", other));
                    }
                }
            }
        } else if path.is_none() {
            path = Some(a);
        } else {
            return console.println("usage: wc [-l|-w|-c] <path>  (one path)");
        }
    }
    let Some(path) = path else {
        return console.println("usage: wc [-l|-w|-c] <path>");
    };
    if !want_l && !want_w && !want_c {
        want_l = true;
        want_w = true;
        want_c = true;
    }
    let Some((mt, canon, size)) = scan_target(console, "wc", path) else { return };
    let (mut lines, mut words, mut bytes, mut in_word) = (0u64, 0u64, 0u64, false);
    let res = scan_file(&mt, &canon, size, |chunk| {
        wc_accumulate(chunk, &mut in_word, &mut lines, &mut words, &mut bytes)
    });
    let scanned = match res {
        Ok(n) => n,
        Err(e) => return vfs_fail(console, "wc", &canon, e),
    };
    let mut out = String::new();
    for (on, n) in [(want_l, lines), (want_w, words), (want_c, bytes)] {
        if on {
            out.push_str(&alloc::format!("{:>8}", n));
        }
    }
    console.println(&alloc::format!("{}  {}", out, canon));
    if scanned < size {
        console.println(&alloc::format!(
            "[... counted {} of {} bytes; scan ceiling {}]", scanned, size, SCAN_MAX));
    }
}

/// BASICS: does `line` match `pat`? FIXED STRING, with `^` and `$` as the only metacharacters.
///
/// **No regex, deliberately, and `help` says so in as many words.** There is no matcher in the tree
/// to borrow (`glob_match` is a FILENAME glob — `*` there does not cross a `/`, and its semantics
/// are wrong for line text), and a hand-rolled engine is not a shell verb, it is its own arc. The
/// failure mode of pretending otherwise is silent and awful: a user types `grep 'foo.*bar'`, the
/// shell matches it literally, finds nothing, and reports the file does not contain what it does
/// contain. Two anchors cost four lines and cover most of what a bench operator wants; everything
/// else is honestly absent.
///
/// A bare `^` is start-anchored-empty (every line matches, as `grep '^'` does); `^$` matches only
/// empty lines. `ci` lower-cases both sides in ASCII only — the same rule `canon_verb` follows, and
/// for the same reason: case folding must not change a string's length.
fn grep_match(pat: &str, line: &str, ci: bool) -> bool {
    let (pat, line) = if ci {
        (pat.to_ascii_lowercase(), line.to_ascii_lowercase())
    } else {
        (String::from(pat), String::from(line))
    };
    let anchored_start = pat.starts_with('^');
    let anchored_end = pat.ends_with('$') && pat.len() > 1;
    let body = &pat[usize::from(anchored_start)..pat.len() - usize::from(anchored_end)];
    match (anchored_start, anchored_end) {
        (true, true) => line == body,
        (true, false) => line.starts_with(body),
        (false, true) => line.ends_with(body),
        (false, false) => body.is_empty() || line.contains(body),
    }
}

/// BASICS: the four `grep` flags, carried as one value so the emitter's signature stays readable
/// and adding a fifth is one field rather than one more positional `bool` nobody can tell apart at
/// the call site.
struct GrepOpts {
    ci: bool,
    numbered: bool,
    invert: bool,
    count_only: bool,
}

/// BASICS: test one assembled line and print it if it counts. Split out of [`fs_grep`] so the
/// streaming closure can call it while holding the console borrow — a closure that captured the
/// console AND was called from inside another closure that captured it would not borrow-check.
fn grep_emit(
    console: &mut Console,
    pat: &str,
    line: &str,
    lineno: u64,
    opts: &GrepOpts,
    hits: &mut u64,
) {
    if grep_match(pat, line, opts.ci) == opts.invert {
        return;
    }
    *hits += 1;
    if opts.count_only {
        return;
    }
    if opts.numbered {
        console.println(&alloc::format!("{}:{}", lineno, line));
    } else {
        console.println(line);
    }
}

/// BASICS `grep [-i] [-n] [-v] [-c] <pattern> <path>`: print the lines of one file that match.
///
/// Flags are the four that earn their place at a bench: `-i` fold case, `-n` number the lines,
/// `-v` invert the sense, `-c` print only the count. They combine (`-in`), and an unknown flag is a
/// refusal rather than a silent literal — `grep -r foo F` must not quietly search for `foo` in one
/// file and let the operator believe it recursed.
///
/// Lines are rendered by `head`'s rules (printable ASCII kept, everything else a `.`), so grepping
/// a binary cannot corrupt the console and what is printed is what `cat` would have shown.
fn fs_grep(console: &mut Console, args: &[&str]) {
    let (mut ci, mut numbered, mut invert, mut count_only) = (false, false, false, false);
    let mut positional: Vec<&str> = Vec::new();
    for a in args {
        if a.starts_with('-') && a.len() > 1 && positional.is_empty() {
            for ch in a[1..].chars() {
                match ch {
                    'i' => ci = true,
                    'n' => numbered = true,
                    'v' => invert = true,
                    'c' => count_only = true,
                    other => {
                        return console.println(&alloc::format!(
                            "grep: unknown flag -{} (usage: grep [-i] [-n] [-v] [-c] <pattern> <path>)",
                            other));
                    }
                }
            }
        } else {
            positional.push(a);
        }
    }
    if positional.len() != 2 {
        return console.println("usage: grep [-i] [-n] [-v] [-c] <pattern> <path>");
    }
    let (pat, path) = (positional[0], positional[1]);
    let opts = GrepOpts { ci, numbered, invert, count_only };
    let Some((mt, canon, size)) = scan_target(console, "grep", path) else { return };
    let mut cur = String::new();
    let mut lineno: u64 = 0;
    let mut hits: u64 = 0;
    // STREAMED, not buffered. The obvious shape — collect every window into a `Vec<Vec<u8>>` and
    // walk it afterwards — would hold a megabyte of file in the heap to print a handful of lines,
    // on a kernel whose heap budget is the reason the console has a 256-line scrollback cap. The
    // line assembly lives in this closure rather than in `scan_file` so `wc`, which never wants a
    // `String`, does not pay for one.
    let res = scan_file(&mt, &canon, size, |chunk| {
        for &b in chunk {
            match b {
                b'\n' => {
                    lineno += 1;
                    grep_emit(console, pat, &cur, lineno, &opts, &mut hits);
                    cur.clear();
                }
                b'\r' => {}
                0x20..=0x7e => cur.push(b as char),
                _ => cur.push('.'),
            }
        }
    });
    let scanned = match res {
        Ok(n) => n,
        Err(e) => return vfs_fail(console, "grep", &canon, e),
    };
    // A last line with no trailing newline is a line; `cat` shows it, so `grep` must consider it.
    if !cur.is_empty() {
        lineno += 1;
        grep_emit(console, pat, &cur, lineno, &opts, &mut hits);
    }
    if count_only {
        console.println(&alloc::format!("{}", hits));
    } else if hits == 0 {
        console.println(&alloc::format!("grep: {}: no match in {} line(s)", canon, lineno));
    }
    if scanned < size {
        console.println(&alloc::format!(
            "[... searched {} of {} bytes; scan ceiling {}]", scanned, size, SCAN_MAX));
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
        vugdemo: false,
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

    // BASICS (orin 17): the everyday verbs get their own legs, in the same battery, on both arches.
    shell_basics_witness();
    // RELICS (orin 17, R26): the rename/retire legs, and — where there is a native volume — the
    // transcript that proves each retired verb's plain replacement covers it.
    shell_relics_witness();
}

// ==================== BASICS — the witness battery for the everyday verbs =========================
//
// WHY A FIXTURE AND NOT A TYPED TRANSCRIPT. The headless QEMU gates type nothing (`./arroyo test`,
// `test-arm`, and `kernel8-test` without `UNAOS_K8_SCRIPT`), so a claim like "`help` prints" that
// depends on a keystroke is not gated on any arch. These legs drive the SAME code the prompt drives
// — `midden_core::plan`, `render_message`, and the verbs' own helpers — and assert what came out.
//
// WHAT MAKES THEM FALSIFIABLE. Not one leg compares an expression with itself. `basics.help` and
// `basics.echo` capture REAL console output through a real `Console::set_output_sink`, so deleting
// the `SHELL:` help line or breaking `render_message` reds them. `basics.wc` asserts counts against
// literals AND asserts that a two-window split gives the same answer as one pass, which is the one
// property a per-window counter would silently break. `basics.grep` asserts both senses of every
// case, so a matcher stuck on `true` fails as loudly as one stuck on `false`. `basics.table`
// compares the verb table against the words this file's `match` actually carries — the check whose
// absence let `umv` and `urmattr` ship unreachable before they retired.

/// BASICS: where a captured console's lines land. `Console::set_output_sink` takes a bare `fn`
/// pointer (no captured state, by its own contract), so the buffer has to be a static.
#[cfg(feature = "witness")]
static WITNESS_CAPTURE: spin::Mutex<Vec<String>> = spin::Mutex::new(Vec::new());

/// BASICS: the sink itself. Touches nothing but its own lock, which satisfies the sink contract's
/// "must not call back into this `Console` and must hold no lock the call site could already hold".
#[cfg(feature = "witness")]
fn witness_sink(line: &str) {
    WITNESS_CAPTURE.lock().push(String::from(line));
}

/// BASICS: run `f` against a throwaway console and return every line it printed.
///
/// The console is heap-only and dropped on return, exactly as `fatverb_storage_witness` does it, so
/// the fixture's own output never reaches the operator's panel.
#[cfg(feature = "witness")]
fn witness_capture(f: impl FnOnce(&mut Console)) -> Vec<String> {
    WITNESS_CAPTURE.lock().clear();
    let mut c = Console::new();
    c.set_output_sink(witness_sink);
    f(&mut c);
    let out = WITNESS_CAPTURE.lock().clone();
    out
}

/// BASICS: the five legs. Called from `midden_witness`, so it runs wherever that runs — both
/// arches, every headless gate — without a new call site in `main.rs`.
#[cfg(feature = "witness")]
fn shell_basics_witness() {
    fn verdict(name: &str, ok: bool, got: &str) {
        if ok {
            serial_println!(":: TSTE: {} -> PASS ::", name);
        } else {
            serial_println!(":: TSTE: {} -> FAIL (got {}) ::", name, got);
        }
    }

    // --- table: the words this build's `match` carries are the words the table registers ---------
    //
    // Both directions. The basics must be verbs HERE (a table that forgot one sends the line to
    // bare-name resolution and answers "Unknown command" about a verb the kernel carries), and the
    // two unafs write verbs must be verbs on an aarch64 build (the defect this arc found: arms
    // shipped, table silent). The aarch64 half is asserted against a SYNTHETIC fact set so the x86
    // gate proves it too — the same trick `midden.resolve` uses to test elision on the Pi.
    let facts = midden_facts();
    let empty: &[&str] = &[];
    let basics = ["grep", "wc", "df", "mount", "env", "set", "history", "sleep", "which"];
    let mut missing = String::new();
    for w in basics {
        let mut vol = midden_core::NameList(empty);
        if !matches!(midden_core::plan(w, &facts, &mut vol), midden_core::Plan::Host { .. }) {
            missing.push(' ');
            missing.push_str(w);
        }
    }
    // RELICS (R26 clause 2): `umv`/`urmattr` were asserted here by BASICS, which had just restored
    // their table entries. They are retired now — `mv` reaches the native volume itself and the
    // attribute verb is `setfattr` — so what this half asserts is the SURVIVORS, on the same
    // SYNTHETIC aarch64 fact set, so the x86 gate proves the aarch64 shape too.
    let arm = midden_core::Facts { aarch64: true, ..midden_core::Facts::bare() };
    for w in ["setfattr", "snap", "mv", "mount"] {
        if !midden_core::is_verb(w, &arm) {
            missing.push(' ');
            missing.push_str(w);
        }
    }
    verdict(
        "shell.basics.table",
        missing.is_empty(),
        &alloc::format!("unregistered:{}", if missing.is_empty() { " none" } else { &missing }),
    );

    // --- help: the command table describes itself, and the new classes are in the description ----
    let lines = witness_capture(|c| {
        let mut vol = midden_core::NameList(empty);
        match midden_core::plan("help", &facts, &mut vol) {
            midden_core::Plan::Say(m) => render_message(c, &m),
            other => c.println(&alloc::format!("help did not reach the core: {:?}", other)),
        }
    });
    let has_shell = lines.iter().any(|l| l.starts_with("SHELL:"));
    let has_text = lines.iter().any(|l| l.starts_with("TEXT:"));
    let has_df = lines.iter().any(|l| l.contains("df | mount"));
    verdict(
        "shell.basics.help",
        lines.len() > 20 && has_shell && has_text && has_df,
        &alloc::format!("lines={} shell={} text={} df={}", lines.len(), has_shell, has_text, has_df),
    );

    // --- echo: the core answers, and the ring renders exactly what it answered ------------------
    let lines = witness_capture(|c| {
        let mut vol = midden_core::NameList(empty);
        match midden_core::plan("echo hi there", &facts, &mut vol) {
            midden_core::Plan::Say(m) => render_message(c, &m),
            other => c.println(&alloc::format!("echo did not reach the core: {:?}", other)),
        }
    });
    verdict(
        "shell.basics.echo",
        lines.len() == 1 && lines[0] == "hi there",
        &alloc::format!("{:?}", lines),
    );

    // --- wc: the counts, and the window-split invariant ----------------------------------------
    //
    // `SAMPLE` is chosen so every field is a different number (2 lines, 4 words, 19 bytes) — three
    // counters that all read 4 would let a wire-crossing pass. The split point falls INSIDE the
    // word `two`, which is the case a per-window counter double-counts.
    const SAMPLE: &[u8] = b"one two\nthree four\n";
    let (mut l1, mut w1, mut b1, mut iw1) = (0u64, 0u64, 0u64, false);
    wc_accumulate(SAMPLE, &mut iw1, &mut l1, &mut w1, &mut b1);
    let (mut l2, mut w2, mut b2, mut iw2) = (0u64, 0u64, 0u64, false);
    wc_accumulate(&SAMPLE[..5], &mut iw2, &mut l2, &mut w2, &mut b2);
    wc_accumulate(&SAMPLE[5..], &mut iw2, &mut l2, &mut w2, &mut b2);
    verdict(
        "shell.basics.wc",
        (l1, w1, b1) == (2, 4, 19) && (l2, w2, b2) == (l1, w1, b1),
        &alloc::format!("one_pass={:?} split={:?}", (l1, w1, b1), (l2, w2, b2)),
    );

    // --- grep: substring, both anchors, case folding, and every one of them in both senses ------
    let g = [
        (grep_match("two", "one two three", false), true),
        (grep_match("TWO", "one two three", false), false),
        (grep_match("TWO", "one two three", true), true),
        (grep_match("^one", "one two three", false), true),
        (grep_match("^two", "one two three", false), false),
        (grep_match("three$", "one two three", false), true),
        (grep_match("two$", "one two three", false), false),
        (grep_match("^one two three$", "one two three", false), true),
        (grep_match("^one two$", "one two three", false), false),
        (grep_match("", "anything", false), true),
    ];
    let bad = g.iter().filter(|(got, want)| got != want).count();
    verdict("shell.basics.grep", bad == 0, &alloc::format!("{} of {} cases wrong", bad, g.len()));
}

// ==================== RELICS — the rename/retire transcript ======================================
//
// WHAT THIS BATTERY HAS TO PROVE, and why it is two halves.
//
// Peter's R26 has two claims that can rot in opposite directions. Clause 1 ("standard names REPLACE
// ours") rots if a retired spelling creeps back as an alias — a table-level fact, provable on any
// build, so `shell.relics.renamed` runs everywhere. Clause 2 ("the duplicated unafs verbs RETIRE
// once a transcript proves the plain verb covers the unafs volume") rots if a plain verb quietly
// stops reaching the native volume — a RUNTIME fact about a real volume, so `shell.relics.native`
// runs only where there is one, which is the `kernel8-test` aarch64 gate.
//
// WHAT MAKES THEM FALSIFIABLE. `renamed` asserts BOTH senses of every pair: the old word must be a
// non-verb AND the new word must be a verb, on three different fact sets, so a table stuck on
// "yes" fails exactly as loudly as one stuck on "no". `native` never compares an expression with
// itself: every leg WRITES through the plain verb's own helper and READS BACK through the mount
// table — the seam `ls` and `cat` use — so pointing a write helper at the wrong volume reds it
// even though the write itself succeeded.

/// RELICS: the pairs R26 clause 1 renamed. `(retired spelling, the standard word that replaced it)`.
///
/// `read` is in this list and its entry is `dd` for the reason the `dd` arm records: POSIX `read`
/// is a shell builtin that reads a line of INPUT, so ours was a standard word wearing a foreign
/// meaning, which is the same defect clause 1 names — not merely a house spelling.
#[cfg(feature = "witness")]
const RELIC_RENAMES: &[(&str, &str)] = &[
    ("bootlog", "dmesg"), ("vfs", "mount"), ("xd", "hexdump"), ("usbinfo", "lsusb"),
    ("netinfo", "ifconfig"), ("diskinfo", "fdisk"), ("fatinfo", "mount"), ("setdate", "date"),
    ("sched", "ps"), ("connect", "nc"), ("udpsend", "nc"), ("get", "curl"), ("read", "dd"),
    ("uls", "ls"), ("ucat", "cat"), ("utouch", "touch"), ("uwrite", "write"),
    ("umkdir", "mkdir"), ("urm", "rm"), ("umv", "mv"), ("urmattr", "setfattr"),
    ("usnaps", "snap"), ("usnap", "snap"), ("usnapdrop", "snap"), ("usnapls", "snap"),
    ("usnapcat", "snap"),
];

/// RELICS: the two arch-neutral legs plus, on a build with a native volume, the subsumption
/// transcript. Called from [`midden_witness`], so it runs wherever that runs.
#[cfg(feature = "witness")]
fn shell_relics_witness() {
    fn verdict(name: &str, ok: bool, got: &str) {
        if ok {
            serial_println!(":: TSTE: {} -> PASS ::", name);
        } else {
            serial_println!(":: TSTE: {} -> FAIL (got {}) ::", name, got);
        }
    }

    // --- renamed: the old word is gone, the new word is here, on three builds ------------------
    let builds = [
        midden_core::Facts::bare(),
        midden_core::Facts { aarch64: true, ..midden_core::Facts::bare() },
        midden_core::Facts { x86: true, exec: true, proc_verbs: true, ..midden_core::Facts::bare() },
    ];
    let mut bad = String::new();
    for f in builds {
        for (old, new) in RELIC_RENAMES {
            if midden_core::is_verb(old, &f) {
                bad.push_str(" alias:");
                bad.push_str(old);
            }
            if !midden_core::is_verb(new, &f) {
                bad.push_str(" missing:");
                bad.push_str(new);
            }
        }
    }
    // PER-PAIR EVIDENCE. The leg above is one verdict over 26 pairs, which is the right shape for a
    // gate but the wrong shape for a capture: a reader asking "was `usbinfo` really replaced, and by
    // what?" should not have to trust an aggregate. One line per pair, on the strictest fact set (a
    // bare build, where only `Avail::Always` verbs exist), so the line says both halves.
    {
        let f = midden_core::Facts::bare();
        for (old, new) in RELIC_RENAMES {
            serial_println!(
                ":: [relics] {} -> {} :: retired={} registered={} ::",
                old, new, !midden_core::is_verb(old, &f), midden_core::is_verb(new, &f)
            );
        }
    }
    verdict(
        "shell.relics.renamed",
        bad.is_empty(),
        &alloc::format!("{}", if bad.is_empty() { " none" } else { &bad }),
    );

    // --- help: the description names the survivors and none of the relics, and has a BENCH class -
    let facts = midden_facts();
    let empty: &[&str] = &[];
    let lines = witness_capture(|c| {
        let mut vol = midden_core::NameList(empty);
        match midden_core::plan("help", &facts, &mut vol) {
            midden_core::Plan::Say(m) => render_message(c, &m),
            other => c.println(&alloc::format!("help did not reach the core: {:?}", other)),
        }
    });
    let text = lines.join("\n");
    let has_bench = lines.iter().any(|l| l.starts_with("BENCH:"));
    // Word-boundary-free `contains` would let `mount` match inside `unmounted`; the relics are
    // checked as whole words the way the operator would type them, one per space-split token.
    let stale = ["bootlog", "vfs", "xd", "usbinfo", "netinfo", "diskinfo", "fatinfo", "setdate",
                 "sched", "udpsend", "uls", "ucat", "utouch", "uwrite", "umkdir", "urmattr",
                 "usnap", "usnaps", "usnapls", "usnapcat", "usnapdrop"];
    let mut leaked = String::new();
    for w in stale {
        if text.split(|c: char| !c.is_ascii_alphanumeric()).any(|t| t == w) {
            leaked.push(' ');
            leaked.push_str(w);
        }
    }
    let names_new = ["dmesg", "hexdump", "lsusb", "ifconfig", "fdisk", "dd", "nc", "curl",
                     "snap", "setfattr", "ps"]
        .iter()
        .all(|w| text.split(|c: char| !c.is_ascii_alphanumeric()).any(|t| t == *w));
    verdict(
        "shell.relics.help",
        has_bench && leaked.is_empty() && names_new,
        &alloc::format!("bench={} new={} leaked:{}", has_bench, names_new,
            if leaked.is_empty() { " none" } else { &leaked }),
    );

    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    shell_relics_native_witness();
    #[cfg(all(target_arch = "aarch64", feature = "baremetal", feature = "witness"))]
    vfsroute_native_witness();
    // VFSROUTE: the arch-neutral routing transcript, here on aarch64 because this is the call site
    // that runs AFTER `emmc2::probe()` — i.e. the first moment this board has volumes to route to.
    #[cfg(all(target_arch = "aarch64", feature = "baremetal", feature = "witness"))]
    vfsroute_witness();
}

/// RELICS: THE SUBSUMPTION TRANSCRIPT — the leg R26 clause 2 makes the retirement conditional on.
///
/// One leg per retired pair, each in the `:: TSTE: shell.relics.<verb> -> PASS ::` shape, and each
/// is a WRITE through the plain verb's own helper followed by a READ BACK through the mount table —
/// `vfs_ls_collect` / `MountTable::read`, the seam `ls` and `cat` resolve through. That is what
/// makes them a transcript of subsumption rather than a self-test of one helper: if `touch` were
/// still FAT-direct on aarch64 the write would SUCCEED (on the FAT volume) and the read back off
/// the native root would still find nothing, so the leg reds.
///
/// # IT SELF-CLEANS, AND THAT CONSTRAINT SHAPED THE `mkdir` LEG
///
/// `fs/unafs.rs`'s `k3_mount_selftest` bit5 requires the native root to hold the two staged
/// fixtures and `acl-*` rows and NOTHING ELSE — its own doc: *"a leaked scratch fixture still fails
/// the bit, so the fixtures' self-clean discipline stays protected."* Every file this transcript
/// creates is deleted before it returns.
///
/// A DIRECTORY cannot be. The UnaFS crate has no directory removal at all (`unlink` returns
/// `IsADirectory` unconditionally; ROADMAP §F2's own scope note: *"the crate has no `rmdir`"*), so a
/// leg that created `/RELICD` would leave it there — invisible in QEMU, where the card is re-staged
/// every run, and a permanent K3-mount red on any metal Pi from its second boot onward. So the
/// `mkdir` leg proves the ROUTING without mutating: it aims `mkdir` at a name that exists ONLY on
/// the native volume (the file the `write` leg just made) and requires the refusal to be the NATIVE
/// crate's `FileExists`. A `mkdir` still riding fat.rs would not find that name on the FAT boot
/// partition at all — it would CREATE a directory and report success — so the leg is two-sided
/// against exactly the failure it exists to catch. **Residual, stated rather than hidden:** there is
/// no POSITIVE native `mkdir` transcript, and there cannot be one until the crate can remove a
/// directory. What carries the positive half meanwhile is `touch`/`write`, which reach the volume
/// through the same `unafs_split` + `resolve_path(parent)` + create-in-parent path.
///
/// Runs after `emmc2::probe()` at the aarch64 `midden_witness` call site, so the volume is mounted —
/// unlike the x86 call site, which is why the native half is gated to this arch and this feature.
#[cfg(all(target_arch = "aarch64", feature = "baremetal", feature = "witness"))]
fn shell_relics_native_witness() {
    fn verdict(name: &str, ok: bool, got: &str) {
        if ok {
            serial_println!(":: TSTE: {} -> PASS ::", name);
        } else {
            serial_println!(":: TSTE: {} -> FAIL (got {}) ::", name, got);
        }
    }
    /// Read a native path back through the MOUNT TABLE (never through the writer's own helper).
    fn read_back(path: &str) -> Option<Vec<u8>> {
        let mt = vfs_mount_table();
        let st = mt.stat(path).ok()?;
        mt.read(path, 0, st.size as usize).ok()
    }
    fn listed(dir: &str, name: &str) -> bool {
        match vfs_ls_collect(dir) {
            Ok((_, rows)) => rows.iter().any(|d| d.name == name),
            Err(_) => false,
        }
    }

    // Every leg drives the PLAIN verb's helper. `witness_capture` swallows the console line the
    // verb prints (the fixture must not paint the operator's panel) and hands it back for the FAIL
    // text, so a failure names what the verb actually said.
    let say = |f: &dyn Fn(&mut Console)| -> String { witness_capture(|c| f(c)).join(" | ") };

    const A: &str = "/RELIC1.TXT";
    const B: &str = "/RELIC2.TXT";
    const C: &str = "/RELIC3.TXT";

    // 1. `write` (was `uwrite`) — read back through the mount table, byte for byte.
    let got = say(&|c: &mut Console| fs_write(c, A, b"relics-one"));
    let ok = read_back(A).as_deref() == Some(b"relics-one".as_slice());
    verdict("shell.relics.write", ok, &alloc::format!("{} read={:?}", got, read_back(A)));

    // 2. `append` (the native primitive the `u*` family never had) — EOF extend, not overwrite.
    let got = say(&|c: &mut Console| fs_append(c, A, b"-two"));
    let ok = read_back(A).as_deref() == Some(b"relics-one-two".as_slice());
    verdict("shell.relics.append", ok, &alloc::format!("{} read={:?}", got, read_back(A)));

    // 3. `cat` (was `ucat`) — the READ verb's own path, asserted against the same bytes.
    let lines = witness_capture(|c| vfs_cat(c, A));
    let ok = lines.iter().any(|l| l.contains("relics-one-two"));
    verdict("shell.relics.cat", ok, &alloc::format!("{:?}", lines));

    // 4. `touch` (was `utouch`) — and, through `vfs_ls_collect`, `ls` (was `uls`) as well.
    let got = say(&|c: &mut Console| fs_touch(c, B));
    verdict("shell.relics.touch", listed("/", "RELIC2.TXT"), &got);

    // 5. `mkdir` (was `umkdir`) — the routing, proven without leaving a directory behind. See the
    //    doc above for why this leg is shaped as a refusal. `A` exists on the NATIVE root only,
    //    because leg 1 put it there, so a FAT-direct `mkdir` would create and report success.
    let got = say(&|c: &mut Console| fs_mkdir(c, A));
    let native_refusal = got.contains("-EEXIST");
    let still_a_file = read_back(A).is_some();
    verdict(
        "shell.relics.mkdir",
        native_refusal && still_a_file,
        &alloc::format!("said={} native_refusal={} still_a_file={}", got, native_refusal, still_a_file),
    );

    // 6. `mv` (was `umv`) — the old name gone from the listing AND the new one in it, which a copy
    //    would fail and a no-op would fail differently.
    let got = say(&|c: &mut Console| fs_mv(c, B, C, false));
    let ok = !listed("/", "RELIC2.TXT") && listed("/", "RELIC3.TXT");
    verdict("shell.relics.mv", ok, &got);

    // 7. `setfattr -x` (was `urmattr`) — plant a typed attribute through the crate, drop it through
    //    the VERB's helper, and prove it is gone by asking the crate again. Both ends are real.
    let planted = crate::fs::unafs::with_unafs(|fs| {
        let id = fs.resolve_path(C).ok()?;
        fs.set_attribute(id, String::from("relic:tag"),
                         ::unafs::AttributeValue::String(String::from("keep"))).ok()?;
        fs.get_attribute(id, "relic:tag").ok().flatten().map(|_| ())
    }).ok().flatten().is_some();
    let got = say(&|c: &mut Console| setfattr_x(c, "relic:tag", C));
    let gone = crate::fs::unafs::with_unafs(|fs| {
        let id = fs.resolve_path(C).ok()?;
        fs.get_attribute(id, "relic:tag").ok().flatten()
    }).ok().flatten().is_none();
    verdict("shell.relics.setfattr", planted && gone, &alloc::format!(
        "planted={} gone={} said={}", planted, gone, got));

    // 8. `snap` (was `usnaps` / `usnap` / `usnapdrop` / `usnapls` / `usnapcat`) — ALL FIVE
    //    subcommands, through the one verb, in one round trip: create, see it in the index, list
    //    and read AS OF it, then drop it and see it leave the index.
    //
    //    The `cat` half is asserted against the LIVE bytes of the same file rather than against the
    //    absence of an error prefix: every one of `unafs_verb_snapcat`'s returns starts `snap cat:`,
    //    successes included, so a prefix test would have been a check that cannot fail — and was,
    //    in this leg's first cut. Snapshot-read == live-read is the property that actually matters
    //    for a file no one modified between the two.
    let live = read_back("/K3HELLO.TXT")
        .and_then(|b| core::str::from_utf8(&b).ok().map(String::from))
        .unwrap_or_default();
    let created = say(&|c: &mut Console| snap_cmd(c, &["create", "relics"]));
    let index = unafs_verb_snaps().join(" | ");
    let snap_gen = crate::fs::unafs::with_unafs(|fs| {
        fs.snapshot_index().ok().and_then(|v| v.iter().find(|s| s.name == "relics").map(|s| s.generation))
    }).ok().flatten();
    let (ok, why) = match snap_gen {
        Some(g) => {
            let in_index = index.contains("relics");
            let ls_ok = unafs_verb_snapls(g, "/").iter().any(|l| l.contains("K3HELLO.TXT"));
            let cat_ok = !live.is_empty() && unafs_verb_snapcat(g, "/K3HELLO.TXT").contains(&live);
            let _ = say(&|c: &mut Console| snap_cmd(c, &["drop", &alloc::format!("{}", g)]));
            let dropped = crate::fs::unafs::with_unafs(|fs| {
                fs.snapshot_index().ok().map(|v| !v.iter().any(|s| s.generation == g))
            }).ok().flatten().unwrap_or(false);
            (in_index && ls_ok && cat_ok && dropped,
             alloc::format!("gen={} index={} ls={} cat={} dropped={}", g, in_index, ls_ok, cat_ok, dropped))
        }
        None => (false, alloc::format!("no generation created ({})", created)),
    };
    verdict("shell.relics.snap", ok, &why);

    // 9. `rm` (was `urm`) — and the SELF-CLEAN. Both files go; the root must be back to exactly what
    //    `k3_mount_selftest` bit5 requires, and this leg asserts that rather than assuming it.
    let got_a = say(&|c: &mut Console| fs_rm(c, A, false));
    let got_c = say(&|c: &mut Console| fs_rm(c, C, false));
    let clean = match vfs_ls_collect("/") {
        Ok((_, rows)) => !rows.iter().any(|d| d.name.starts_with("RELIC")),
        Err(_) => false,
    };
    verdict(
        "shell.relics.rm",
        !listed("/", "RELIC1.TXT") && !listed("/", "RELIC3.TXT") && clean,
        &alloc::format!("{} | {} | root_clean={}", got_a, got_c, clean),
    );
}

// ===================== VFSROUTE — THE TRANSCRIPT ==================================================
//
// The legs Peter's ruling asks for, in the `:: TSTE: <name> -> PASS/FAIL ::` shape the boot-replay
// ring and `tste` already read. Every one drives THE REAL VERB through `witness_capture` (a
// heap-only console, dropped on return, so the fixture never paints the operator's panel) and
// asserts against a fact read off the MOUNT TABLE — never against the verb's own return value, so
// no leg compares an expression with itself.
//
// WHAT MAKES THEM TWO-SIDED. The failure this arc exists to prevent is a verb that ignores the
// namespace and answers off whichever volume it happens to hold. So the legs are written against a
// PROBE NAME taken off the root volume's own listing, and then require that name to appear under a
// DIFFERENT volume's prefix if and only if the two prefixes resolve to the same volume. On aarch64
// `/` is native UnaFS and `/fat` is the boot FAT, so a `ls`/`cat`/`stat` that had kept its old
// FAT-direct body would show a native file under `/fat` and the leg reds. On x86 the two prefixes
// ARE one volume, and the same expression requires the opposite answer — so neither arch's leg can
// pass by being stuck.
//
// ARCH-NEUTRAL BY CONSTRUCTION: there is no `target_arch` in the assertions, only in the call sites
// (aarch64 runs it after `emmc2::probe()`, x86 after the storage-ready pass — the two moments each
// board actually has a volume).

/// VFSROUTE: one-shot latch — the service loops call the x86 site every pass; it must speak once.
#[cfg(feature = "witness")]
static VFSROUTE_WITNESS_DONE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// VFSROUTE (orin 17): the routing transcript — `ls`, `cat`, `stat` and a typed refusal, proven to
/// go through the mount table on every mounted volume.
#[cfg(feature = "witness")]
pub fn vfsroute_witness() {
    use core::sync::atomic::Ordering;
    use crate::fs::vfs::{NodeKind, VfsError};
    if VFSROUTE_WITNESS_DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    fn verdict(name: &str, ok: bool, got: &str) {
        if ok {
            serial_println!(":: TSTE: {} -> PASS ::", name);
        } else {
            serial_println!(":: TSTE: {} -> FAIL (got {}) ::", name, got);
        }
    }
    let mt = vfs_mount_table();

    // --- leg 1: ROUTING. Every mounted prefix resolves to its own volume, and a name that merely
    //     STARTS WITH a prefix does not (the boundary rule — a naive `starts_with`, which is what
    //     the deleted per-verb prefix tests used, gets `/fatty.bin` wrong and sends a root-volume
    //     file to the FAT card).
    let rows = mt.rows();
    let mut route_ok = !rows.is_empty();
    let mut route_why = String::new();
    for (prefix, name, _, _, _) in &rows {
        let got = mt.volume_name(prefix).unwrap_or_else(|_| String::from("?"));
        route_why.push_str(&alloc::format!("{}={} ", prefix, got));
        if got != *name {
            route_ok = false;
        }
    }
    let root_vol = mt.volume_name("/").unwrap_or_else(|_| String::from("?"));
    for probe in ["/fatty.bin", "/usbfoo"] {
        let got = mt.volume_name(probe).unwrap_or_else(|_| String::from("?"));
        route_why.push_str(&alloc::format!("{}={} ", probe, got));
        if got != root_vol {
            route_ok = false; // a prefix claimed a name that only shares its first bytes
        }
    }
    verdict("vfsroute.route", route_ok, &route_why);

    // --- the probe: a real FILE on the root volume, read off the TABLE (not off a verb), plus
    //     whether `/fat` is the same volume as `/` on this board. Both legs below hang off these.
    let probe = mt
        .read_dir("/")
        .ok()
        .and_then(|rs| rs.into_iter().find(|r| matches!(r.kind, NodeKind::File)));
    let same_as_fat = mt.same_volume("/", "/fat").unwrap_or(false);

    match probe {
        None => serial_println!(
            ":: vfsroute: the root volume ({}) lists no file — ls/cat/stat legs skipped ::", root_vol),
        Some(p) => {
            let root_path = vfs_join("/", &p.name);
            let fat_path = vfs_join("/fat", &p.name);

            // --- leg 2: `ls`. The probe appears under `/`, and under `/fat` IFF the two prefixes
            //     are the same volume. A verb that ignored the prefix would show it under both.
            let ls_root = witness_capture(|c| vfs_ls(c, "/", false)).join(" | ");
            let ls_fat = witness_capture(|c| vfs_ls(c, "/fat", false)).join(" | ");
            let in_root = ls_root.contains(p.name.as_str());
            let in_fat = ls_fat.contains(p.name.as_str());
            verdict(
                "vfsroute.ls",
                in_root && (in_fat == same_as_fat),
                &alloc::format!(
                    "probe={} in_root={} in_fat={} same_volume={} root=[{}] fat=[{}]",
                    p.name, in_root, in_fat, same_as_fat, ls_root, ls_fat),
            );

            // --- leg 3: `cat`. The same shape on the read path: the probe reads under `/`, and
            //     under `/fat` IFF one volume. "Reads" is asserted as "did not print an error
            //     line", and the negative half as "did", so both directions are convictable.
            let cat_root = witness_capture(|c| vfs_cat(c, &root_path)).join(" | ");
            let cat_fat = witness_capture(|c| vfs_cat(c, &fat_path)).join(" | ");
            let root_read = !cat_root.starts_with("cat: ");
            let fat_read = !cat_fat.starts_with("cat: ");
            verdict(
                "vfsroute.cat",
                root_read && (fat_read == same_as_fat),
                &alloc::format!(
                    "root_read={} fat_read={} same_volume={} fat_said=[{}]",
                    root_read, fat_read, same_as_fat,
                    // Bounded: a FAIL on a binary probe file would otherwise pour the whole 8 KiB
                    // console cap onto the wire, and the serial transport is the evidence channel.
                    &cat_fat[..core::cmp::min(cat_fat.len(), 96)]),
            );

            // --- leg 4: `stat`. It must NAME the volume that answered, and the size it reports
            //     must equal the size the LISTING reported for the same object — two verbs, one
            //     fact, so a `stat` reading a different volume disagrees with `ls`.
            let st = witness_capture(|c| fs_stat(c, &root_path)).join(" | ");
            let names_volume = st.contains(&alloc::format!("volume: {}", root_vol));
            let names_size = st.contains(&alloc::format!("size:   {} byte(s)", p.size));
            let st_root = witness_capture(|c| fs_stat(c, "/")).join(" | ");
            let root_is_dir = st_root.contains("kind:   dir");
            verdict(
                "vfsroute.stat",
                names_volume && names_size && root_is_dir,
                &alloc::format!(
                    "volume={} size={} root_dir={} said=[{}]",
                    names_volume, names_size, root_is_dir, st),
            );
        }
    }

    // --- leg 5: THE TYPED REFUSAL, and no fallback.
    //
    // `remove_attr` is the clean case: typed attributes are a UnaFS feature, the FAT backend
    // implements none, and it therefore inherits the trait's default. Asked THROUGH THE TABLE about
    // a FAT path it must answer exactly `Unsupported` — which the verb renders `-ENOTSUP` — and it
    // must NOT be answered by some other volume that does have attributes. Deterministic on every
    // board: the refusal is a property of the backend, not of the media in the slot.
    //
    // The aarch64 half adds the one an operator meets: `rmdir` on the native volume. The UnaFS crate
    // has no directory removal, so `NativeBackend` inherits the default there too. Before VFSROUTE
    // that keystroke walked fat.rs looking for the name on the BOOT PARTITION — the silent
    // fall-through this arc exists to delete.
    let fat_attr = mt.remove_attr("/fat/VFSROUTE.TXT", "k", SHELL_PRINCIPAL);
    let fat_refused = fat_attr == Err(VfsError::Unsupported);
    let native_rmdir = mt.remove_dir("/VFSROUTE.DIR", SHELL_PRINCIPAL);
    // On a board whose root volume IS a FAT mount, `remove_dir` is implemented and answers about the
    // object (`NoSuchPath`) rather than about the capability; on a native root it is the capability
    // refusal. Both are typed errors from the volume that owns the path, which is the property.
    let native_typed = matches!(
        native_rmdir,
        Err(VfsError::Unsupported) | Err(VfsError::NoSuchPath) | Err(VfsError::NotADirectory)
    );
    verdict(
        "vfsroute.refuse",
        fat_refused && native_typed,
        &alloc::format!(
            "fat_remove_attr={:?} root_remove_dir={:?} root_vol={}",
            fat_attr, native_rmdir, root_vol),
    );
}

/// VFSROUTE (orin 17): the NATIVE-volume mutation transcript — `touch`, `ls`, `mkdir`, `rmdir`, `rm`
/// through the plain verbs, on the volume the pre-VFSROUTE mutating verbs could not reach.
///
/// # IT SELF-CLEANS, AND THAT CONSTRAINT SHAPED TWO OF THE LEGS
///
/// `fs/unafs.rs`'s `k3_mount_selftest` bit5 requires the native root to hold the staged fixtures and
/// `acl-*` rows and NOTHING ELSE. Every file this transcript creates is deleted before it returns; a
/// DIRECTORY cannot be, because the UnaFS crate has no directory removal at all. So there is no
/// positive `mkdir` leg here — instead `mkdir` is aimed at the file `touch` just made and must be
/// refused `-EEXIST`, which is a ROUTING proof (a `mkdir` still riding fat.rs would not find that
/// name on the boot partition and would create a directory and report success), and `rmdir` is aimed
/// at the same name and must be refused `-ENOTSUP`, which is the CAPABILITY proof: the native
/// backend implements no directory removal, so the verb prints the volume's own answer instead of
/// falling through to the FAT walker — the exact silent fallback this arc deletes.
#[cfg(all(target_arch = "aarch64", feature = "baremetal", feature = "witness"))]
fn vfsroute_native_witness() {
    fn verdict(name: &str, ok: bool, got: &str) {
        if ok {
            serial_println!(":: TSTE: {} -> PASS ::", name);
        } else {
            serial_println!(":: TSTE: {} -> FAIL (got {}) ::", name, got);
        }
    }
    fn listed(name: &str) -> bool {
        match vfs_ls_collect("/") {
            Ok((_, rows)) => rows.iter().any(|d| d.name == name),
            Err(_) => false,
        }
    }
    let say = |f: &dyn Fn(&mut Console)| -> String { witness_capture(|c| f(c)).join(" | ") };
    const F: &str = "/VFSR1.TXT";

    // 1. `touch` lands on the NATIVE volume, and `ls` — the read verb, through the mount table —
    //    sees it. Two different verbs, one fact.
    let said = say(&|c: &mut Console| fs_touch(c, F));
    let after = listed("VFSR1.TXT");
    verdict("vfsroute.touch", after, &alloc::format!("said={} listed={}", said, after));

    // 2. `mkdir` at the same name: refused -EEXIST, and the name is STILL A FILE afterwards.
    let said = say(&|c: &mut Console| fs_mkdir(c, F));
    let eexist = said.contains("-EEXIST");
    let still_file = listed("VFSR1.TXT");
    verdict("vfsroute.mkdir", eexist && still_file,
        &alloc::format!("said={} eexist={} still_file={}", said, eexist, still_file));

    // 3. `rmdir`: the CAPABILITY refusal — the native backend implements no directory removal, so
    //    the verb prints `-ENOTSUP` and nothing anywhere is removed.
    let said = say(&|c: &mut Console| fs_rmdir(c, F));
    let enotsup = said.contains("-ENOTSUP");
    let untouched = listed("VFSR1.TXT");
    verdict("vfsroute.rmdir", enotsup && untouched,
        &alloc::format!("said={} enotsup={} untouched={}", said, enotsup, untouched));

    // 4. `rm`, and the SELF-CLEAN — asserted, not assumed (bit5 fails on a leaked fixture).
    let said = say(&|c: &mut Console| fs_rm(c, F, false));
    let gone = !listed("VFSR1.TXT");
    let clean = match vfs_ls_collect("/") {
        Ok((_, rows)) => !rows.iter().any(|d| d.name.starts_with("VFSR")),
        Err(_) => false,
    };
    verdict("vfsroute.rm", gone && clean,
        &alloc::format!("said={} gone={} root_clean={}", said, gone, clean));
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
/// value and same reasoning as `video::desktop_uefi::STORAGE_WAIT_MS` — generous against the deferred SCSI
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
    // The shape is `desktop_uefi::desktop_app_service`'s, deliberately — including its law that the wait
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

    // VFSROUTE: the routing transcript rides THIS site on x86, and for the same reason the FATVERB
    // legs do — `midden_witness` fires at main.rs step 5, before `pci::init` and the storage
    // publish, so a routing leg there would assert against an empty namespace and be dead rather
    // than quiet. By here `fat::probe_once()` / `sdhc_probe_once()` have run and the mount table has
    // something in it.
    #[cfg(feature = "witness")]
    vfsroute_witness();
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
    // BASICS: the history record is the FIRST act, ahead of the planner, so what is retained is
    // what the operator typed — including the line the shell is about to refuse. A record taken
    // after `plan` would hold only the commands that worked, which is the opposite of what anyone
    // reads a history for.
    history_record(cmd_line);
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
            #[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
            bare_exec(console, &typed, &name);
            // BARENAME (§6.6a): the day a loader arrived on aarch64 the compiler pointed here, as
            // this comment used to promise. What is left is the build with no process table at all,
            // which never sets `Facts::exec` and so is never handed this arm; the branch stays so
            // the match is total.
            #[cfg(not(any(all(any(feature = "baremetal", feature = "tegra_el0", feature = "virt_el0"), target_arch = "aarch64"), target_arch = "x86_64")))] // EL0-NAMING: NEGATED/RUNTIME — KEPT LONGHAND ON PURPOSE. Cargo feature implication is ONE-WAY: `baremetal`/`tegra_el0` imply `aarch64_el0`, not the reverse, so `not(aarch64_el0)` would diverge from this predicate for anyone who enabled `aarch64_el0` ALONE. No gate leg builds that combination, which is the trap — a byte-identity check over the legs would PASS while the hazard shipped. Positive sites are safe because implication runs their way; these are not.
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
    // DECRUD-1 adds the `vugdemo` half of exactly that rule: knob-off the two arms below are not
    // registered on aarch64 either, so the words must stop claiming the screen there too.
    #[cfg(all(target_arch = "aarch64", feature = "v3d"))]
    let took_screen = (cfg!(feature = "vugdemo") && (command == "vug" || command == "pulse")) || command == "v3d";
    #[cfg(all(target_arch = "aarch64", not(feature = "v3d")))]
    let took_screen = cfg!(feature = "vugdemo") && (command == "vug" || command == "pulse");
    #[cfg(not(target_arch = "aarch64"))]
    let took_screen = false;

    match command {
        "date" => {
            // RELICS (R26 clause 1): `setdate` was ours; `date -s` is the standard, so the SEED is
            // a flag on the verb that shows the clock and the second word is gone. Everything after
            // `-s` is the old seed argument list, parsed by the same `parse_wallclock`.
            if args.first().copied() == Some("-s") {
                match parse_wallclock(&args[1..]) {
                    Some(t) if crate::clock::set(t).is_ok() => {
                        console.println(&alloc::format!(
                            "clock set: {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                            t.year, t.month, t.day, t.hour, t.min, t.sec));
                    }
                    _ => console.println(
                        "date: usage: date -s YYYY-MM-DD HH:MM[:SS]  (year 1980-2107)"),
                }
                return took_screen;
            }
            // JD17/CLOCK-3/PI-UI-3: show the kernel wall clock. The UNIFIED civil clock is the source of
            // truth: prefer the Unix anchor (an SNTP sync on the Pi — PI-NET-16 — or a `date -s` seed), so a
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
                None => ui3_say(console, "date", "date: clock not set (date -s YYYY-MM-DD HH:MM:SS)"),
            }
        },
        "time" => {
            // CLOCK-1: the shared kernel civil clock — ISO-8601 UTC plus the source that set it.
            // UNSET is first-class and honest: `unsynced` until an SNTP sync (pi/genet PI-NET-16) or a
            // `date -s` seeds it. x86 has no SNTP client yet, so `time` there reads `unsynced` until a
            // manual `date -s` — the seam is what this arc delivers; x86 SNTP is a future rmbp arc.
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
                None => ui3_say(console, "time", "time: unsynced (no SNTP sync or date -s yet)"),
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
        // RELICS (R26 clause 1): `usbinfo` was ours; `lsusb` is the standard word for exactly
        // this output — the list of USB devices this bus enumerated.
        "lsusb" => {
            for line in crate::drivers::xhci::usb_summary() {
                console.println(&line);
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
            // VFSROUTE: ONE body. The arm used to split on `target_arch` — `pi_ls` (unafs) on
            // aarch64, `ls_path`/`ls_globbed` (FAT) on x86 — which is precisely "adding each
            // filesystem to ls". It now calls the routed lister for every volume on every board,
            // and the wildcard path expands through `MountTable::read_dir`, so `ls *.TXT` works on
            // the native volume too (it silently found nothing there before).
            match path {
                Some(a) if has_glob(a) => ls_globbed(console, a, long),
                other => vfs_ls(console, other.unwrap_or("."), long),
            }
        },
        "cd" => {
            // JD4: change the shell's working directory. No argument (or `/`) returns to the root.
            // VFSROUTE: resolved through the mount table, so `cd /fat` and `cd /usb` are ordinary
            // directory changes rather than names that only one volume's walker understands. The
            // stored cwd is the normalized namespace path — a path string, not a cached chain head,
            // so a swapped card can only ever produce an honest `-ENOENT`.
            let path = vfs_path(args.first().copied().unwrap_or("/"));
            let mt = vfs_mount_table();
            if let Some(vol) = unmounted_reserved_volume(&mt.prefixes(), &path) {
                console.println(&alloc::format!(
                    "cd: {}: volume {} not mounted (-ENODEV)", path, vol));
            } else {
                match mt.stat(&path) {
                    Ok(st) if matches!(st.kind, crate::fs::vfs::NodeKind::Dir) => {
                        console.println(&path);
                        *CWD.lock() = if path == "/" { None } else { Some(path) };
                    }
                    Ok(_) => console.println(&alloc::format!(
                        "cd: {}: not a directory (-ENOTDIR)", path)),
                    Err(e) => vfs_fail(console, "cd", &path, e),
                }
            }
        },
        "pwd" => {
            console.println(&cwd_path());
        },
        "cat" | "type" => {
            // JD4: `cat` takes a path (absolute or cwd-relative), e.g. `cat DOCS/README.TXT`.
            // VFSROUTE: ONE body for every volume and both arches (aarch64 had the routed one, x86 a
            // FAT-direct twin printing the same text), and JD12 glob expansion now walks the mount
            // table too — `cat *.TXT` reaches whichever volume the pattern's parent lives on.
            match args.first() {
                None => console.println("usage: cat <path>"),
                Some(name) if has_glob(name) => cat_globbed(console, name),
                Some(name) => vfs_cat(console, name),
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
        // ---- BASICS (orin 17) ------------------------------------------------------------------
        // Peter, 2026-09-06: "we need basic commands back". Nine service arms, every one of them
        // `Avail::Always` in `midden_core::HOST_VERBS` and free of a `target_arch` gate, so the
        // words mean the same thing on the rMBP, the Pi and the Orin. The helpers they call live
        // in the BASICS section above `midden_witness`.
        "grep" => {
            // Fixed-string search over one file, with ^/$ anchors and -i/-n/-v/-c. Deliberately
            // not a regex — see `grep_match` for why a literal match of `.*` is worse than none.
            fs_grep(console, &args);
        },
        "wc" => {
            // Lines, words and bytes of one file; -l/-w/-c select, and they combine.
            fs_wc(console, &args);
        },
        // RELICS (R26 clause 1): `vfs` folded in here. `mount` with no arguments answers "what is
        // attached, and how full is it" (with the FAT geometry the retired `fatinfo` printed);
        // `mount <op>` performs the op over the ONE namespace, which is the surface `vfs` was.
        // `df` is the same table without the geometry — the capacity question, plainly.
        "df" | "mount" => {
            if verb == "mount" && !args.is_empty() {
                vfs_cmd(console, &args);
            } else {
                df_report(console, command);
            }
        },
        "env" => {
            // The build's live facts, then the shell variables. No `$VAR` expansion yet (M2).
            env_report(console);
        },
        "set" => {
            // `set` / `set NAME` / `set NAME VALUE...` / `set -u NAME`.
            set_cmd(console, &args, &rest);
        },
        "history" => {
            // The last n typed lines, numbered; `history -c` clears and says how many it dropped.
            history_cmd(console, &args);
        },
        "sleep" => {
            // Bounded twice: a deadline on `arch::ms()` and a spin cap for a clock that is frozen.
            match args.first().and_then(|s| parse_num(s)) {
                Some(ms) => shell_sleep(console, ms),
                None => console.println("usage: sleep <ms>  (decimal or 0x-hex, capped at 10000)"),
            }
        },
        "which" => {
            // Verb, program, or neither — answered through `midden_core`, never re-derived here.
            match args.first() {
                Some(word) => which_report(console, word),
                None => console.println("usage: which <word>"),
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
        // RELICS (R26 clause 1): `xd` was ours; the standard spelling of a bounded file dump is
        // `hexdump`, and it is what an operator coming from any other system will type.
        "hexdump" => {
            // JD19: bounded hexdump — `hexdump <path> [off] [len]` (default off=0, len=256; len
            // capped at 4096). off/len accept decimal or 0x-hex. off past EOF = honest empty; a
            // directory = -EISDIR; an honest `[... n more byte(s)]` tail note when the file is larger.
            match args.first() {
                None => console.println("usage: hexdump <path> [off] [len]"),
                Some(path) => {
                    let off = args.get(1).and_then(|s| parse_num(s)).unwrap_or(0) as u32;
                    let len = args.get(2).and_then(|s| parse_num(s)).map(|n| n as usize).unwrap_or(256);
                    fs_hexdump(console, path, off, len);
                }
            }
        },
        // RELICS (R26 clause 2): `urmattr` had no plain twin to retire into — nothing else in the
        // shell removes a typed attribute — so it takes the STANDARD name for the job. Argument
        // order follows `setfattr(1)`: the flag and its key first, the path last (`urmattr` had it
        // the other way round, which is one more thing an operator had to remember).
        // Always-registered (clause 3): the ring decides the answer, the word exists everywhere.
        "setfattr" => {
            match (args.first().copied(), args.get(1).copied(), args.get(2).copied()) {
                (Some("-x"), Some(key), Some(path)) => setfattr_x(console, key, path),
                _ => console.println("usage: setfattr -x <key> <path>  (drop one typed attribute)"),
            }
        },
        // RELICS (R26 clause 2): five spellings (`usnaps` `usnap` `usnapdrop` `usnapls` `usnapcat`)
        // become ONE verb with subcommands. They were never five commands: they were one noun with
        // five operations, which is what a subcommand is for, and the `u` prefix said only "the
        // native volume" — the volume every file verb now reaches by prefix.
        "snap" => {
            snap_cmd(console, &args);
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
        #[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
        "run" => {
            // EXEC-1: load an ELF64 user program off the VFS namespace and execute it in user mode, reporting its
            // exit status. Rides the SAME `MountTable` the `mount` verb uses (`/fat` = FAT boot partition,
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
        "screenshot" => {
            // PRTSCR: capture the panel to `SCREEN<n>.PNG` at the volume root. The whole mechanism
            // lives in `video::prtscr` because the Print Screen KEY reaches the same function from
            // the device-service pass — a verb that reimplemented any of it would be a second
            // capture path to keep in step, which is exactly what MIDDEN-M1 removed from this file.
            //
            // Two sinks, two lengths, one verdict word — FATVERB's rule. The operator gets a
            // sentence; the capture gets the census line `Refusal::report` writes to serial. Both
            // are emitted for a refusal, and the OK line is emitted here for the same reason: a
            // headless `test-fat` boot must be able to prove the write from the log alone.
            match crate::video::prtscr::capture() {
                Ok(shot) => {
                    serial_println!(
                        ":: PRTSCR: {} {}x{} {} bytes -> OK ::",
                        shot.name, shot.width, shot.height, shot.bytes
                    );
                    console.println(&alloc::format!(
                        "wrote {} ({}x{}, {} bytes)", shot.name, shot.width, shot.height, shot.bytes
                    ));
                }
                Err(why) => {
                    why.report();
                    console.println(&why.sentence());
                }
            }
        },
        // RELICS (R26 clause 1): `diskinfo` was ours. Of the two spellings Peter offered, this
        // output is `fdisk -l` and not `df -h`: every field is DEVICE geometry (vendor, product,
        // block size, block count, capacity), and not one of them is about how full a filesystem
        // is, which is what `df` answers and what `df`/`mount` already print. A bare `fdisk` is a
        // usage line, not a listing — there is no partition EDITOR here, and a verb that silently
        // did the read-only half of an interactive tool would be teaching the wrong reflex.
        "fdisk" => {
            if args.first().copied() != Some("-l") {
                console.println("usage: fdisk -l  (list block devices; no partition editor here)");
                return took_screen;
            }
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
                                // `fdisk -l` had been telling the operator the stick could not be
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
        // RELICS (R26): THE SHELLBASICS HAZARD, CLOSED. `write` carried a JD5 overload — two args
        // that both parsed as `<u64 lba> <byte>` meant a RAW BLOCK WRITE rather than a file write,
        // so `write 12 0` was not a file called `12` containing `0`, it was 512 bytes of zeros over
        // logical block 12. The two operations now have two verbs, and the raw one is `dd`, which
        // is what raw block I/O is called everywhere else.
        //
        // ARGUMENT SHAPE, and why it is `key=value` and not positional: `dd`'s whole convention is
        // `if=`/`of=`, and the convention is load-bearing here — a positional `dd 12 0` would be
        // exactly the ambiguity this arm exists to remove, because nothing in it says which way the
        // bytes travel. So:
        //
        //   dd if=<lba>                  read ONE 512-byte block and dump the first 128 bytes
        //   dd of=<lba> byte=<0xNN|n>    fill ONE 512-byte block with that byte value
        //
        // One block, always: `count=` is deliberately not accepted rather than accepted and capped,
        // because a shell that takes `count=` from a bench operator and quietly does something else
        // is worse than one that refuses the word. The retired `read <lba>` spelling is gone with
        // the same reasoning as the rest of clause 1 — POSIX `read` is a shell builtin that reads a
        // LINE OF INPUT into a variable, so our `read` was not merely non-standard, it was a
        // standard word wearing someone else's meaning.
        "dd" => {
            let field = |k: &str| -> Option<&str> {
                args.iter().find_map(|a| a.strip_prefix(k))
            };
            let lba_in = field("if=").and_then(|v| parse_num(v));
            let lba_out = field("of=").and_then(|v| parse_num(v));
            let byte = field("byte=").and_then(parse_byte);
            match (lba_in, lba_out, byte) {
                (Some(lba), None, _) => {
                    let mut buf = [0u8; 512];
                    match crate::drivers::block::read_block(lba, &mut buf) {
                        Ok(_) => {
                            console.println(&alloc::format!("LBA {}:", lba));
                            hexdump(console, &buf[0..128]);
                        }
                        Err(e) => console.println(&alloc::format!("dd: read error: {:?}", e)),
                    }
                }
                (None, Some(lba), Some(b)) => {
                    let buf = [b; 512];
                    match crate::drivers::block::write_block(lba, &buf) {
                        Ok(()) => console.println(&alloc::format!(
                            "dd: wrote LBA {} (0x{:02x} x512)", lba, b)),
                        Err(e) => console.println(&alloc::format!("dd: write error: {:?}", e)),
                    }
                }
                (None, Some(_), None) => console.println("dd: of= needs byte=<0xNN>  (no source given)"),
                (Some(_), Some(_), _) => console.println("dd: if= and of= together are not supported"),
                _ => console.println("usage: dd if=<lba> | dd of=<lba> byte=<0xNN>  (ONE 512-byte block)"),
            }
        },
        "write" => {
            // RELICS (R26): a FILE write, and only ever a file write. `write <path> <text...>`
            // (create-or-truncate; text = the rest of the line, whitespace-collapsed like `echo`).
            // The raw-block overload that used to live here is `dd` — see that arm.
            match args.first() {
                None => console.println("usage: write <path> <text>"),
                Some(name) => fs_write(console, name, args[1..].join(" ").as_bytes()),
            }
        },
        // RELICS (R26 clause 1): `netinfo` was ours, and of the two standard spellings Peter
        // offered this is `ifconfig`, not `ip`. The reason is the OUTPUT: every line describes ONE
        // interface's state — its MAC, whether the link is up, and this NIC's frame/IRQ counters —
        // which is `ifconfig`'s shape exactly. `ip` prints ADDRESS OBJECTS over a set of links
        // (`ip addr`, `ip route`, `ip link`), and this shell has neither a link set nor a routing
        // table to print, so `ip` would be a name promising a subcommand tree that does not exist.
        "ifconfig" => {
            // PI-UI-3: the Pi (GENET) has no e1000, so the x86 path below reports "no device" there. Give
            // the Pi shell an equivalent that reads the GENET interface snapshot — MAC / IP / gateway /
            // lease state — plus the civil-clock sync state, matching the x86 verb's line shape.
            #[cfg(all(target_arch = "aarch64", not(feature = "genet")))]
            ui3_say(console, "ifconfig", "No network device ready.");
            #[cfg(all(target_arch = "aarch64", feature = "genet"))]
            {
                match crate::arch::aarch64::genet::netinfo() {
                    Some(n) => {
                        ui3_say(console, "ifconfig", &alloc::format!(
                            "NIC: MAC {}  link {}",
                            crate::drivers::e1000::fmt_mac(&n.mac),
                            if n.link_up { "UP" } else { "DOWN" }
                        ));
                        ui3_say(console, "ifconfig", &alloc::format!(
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
                        ui3_say(console, "ifconfig", &alloc::format!("time: {}", sync));
                    }
                    None => ui3_say(console, "ifconfig", "No network device ready."),
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
        // RELICS (R26 clause 1): `connect` and `udpsend` were TWO verbs for one job — open a
        // socket to a host:port and exchange a message — differing only in the transport, which is
        // exactly what `nc`'s `-u` flag selects. So they are one `nc`, and the semantics do match:
        // `nc <ip> <port> [message]` connects, sends, reads, closes; `nc -u` sends one datagram and
        // waits briefly for a reply. What `nc` does elsewhere and does NOT do here is stream stdin
        // — there is no pipeline in this shell to stream from — so the message is an argument, and
        // the usage line says so rather than implying a stdin that would hang.
        "nc" => {
            let udp = args.first().copied() == Some("-u");
            let rest: &[&str] = if udp { &args[1..] } else { &args[..] };
            let ip = rest.first().and_then(|s| parse_ipv4(s));
            let port = rest.get(1).and_then(|s| s.parse::<u16>().ok());
            if udp {
                match (ip, port) {
                    (Some(ip), Some(port)) => {
                        let msg = if rest.len() > 2 { rest[2..].join(" ") } else { String::from("unaos-udp") };
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
                    _ => console.println("usage: nc -u <a.b.c.d> <port> [message]"),
                }
                return took_screen;
            }
            let args = rest;
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
                _ => console.println("usage: nc [-u] <a.b.c.d> <port> [message]"),
            }
        },
        // RELICS (R26 clause 1): `get` was ours; an HTTP/1.0 GET over a socket, printed to the
        // terminal, is `curl` everywhere else. The semantics match the standard verb's DEFAULT
        // behaviour exactly (fetch and print; no follow, no upload), so the name is honest. The
        // argument becomes a URL rather than three positionals for the same reason `dd` takes
        // `if=`: the URL IS the standard's argument, and `curl 10.0.2.2 8000 /x` would be a verb
        // wearing a standard name over a private argument grammar.
        "curl" => {
            // Minimal HTTP/1.0 GET over the streaming TCP client: connect, send the request,
            // read the whole response until the server closes, and print it.
            let url = args.first().copied().unwrap_or("");
            let url = url.strip_prefix("http://").unwrap_or(url);
            let (hostport, path) = match url.find('/') {
                Some(i) => (&url[..i], String::from(&url[i..])),
                None => (url, String::from("/")),
            };
            let (host, port_str) = match hostport.rfind(':') {
                Some(i) => (&hostport[..i], Some(&hostport[i + 1..])),
                None => (hostport, None),
            };
            let port = match port_str {
                Some(v) => match v.parse::<u16>().ok() { Some(n) => Some(n), None => None },
                None => Some(80),
            };
            match (parse_ipv4(host), port) {
                (Some(ip), Some(port)) => {
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
                _ => console.println("usage: curl [http://]<a.b.c.d>[:port][/path]  (HTTP/1.0 GET)"),
            }
        },
        // The in-kernel 3D sculptor. Aarch64 only, matching `crate::vug`: the whole arm vanishes on
        // x86, where the verb is therefore an ordinary unrecognised word. DECRUD-1: and on the
        // `vugdemo` knob, DEFAULT OFF — knob-off the verb is that same unrecognised word on the Pi,
        // and `VUG.ELF` (`run vug`) is the program that does this for real, in ring 3.
        #[cfg(all(target_arch = "aarch64", feature = "vugdemo"))]
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
        // way — arch AND (DECRUD-1) the `vugdemo` knob. x86 keeps the per-core instrument it always
        // had — the `ui_status` strip and `sched` — and this verb is an unrecognised word there;
        // knob-off the Pi is in the same position, with `PULSE.ELF` (`run pulse`) as the real one.
        #[cfg(all(target_arch = "aarch64", feature = "vugdemo"))]
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
        // RELICS (R26 clause 1): `sched` retired. `ps` was already an alias for it, and Peter's
        // ruling is that the standard word is THE name — so the alias became the verb and the
        // house spelling is gone. (The scheduler MODULE is still `arch::sched`; a subsystem name
        // is not a verb name, and clause 1 is about what the operator types.)
        "ps" => {
            // SCHEDPAR: one table body for both arches. `current_task_id`/`run_queue_len` are
            // signature-matched twins (the aarch64 pair authored by the orin seat, folded under
            // the 2026-08-27 cross-lane grant); only the core census and the demo line stay
            // per-arch. The aarch64 census walks the full percpu range rather than
            // `online_cpu_count()` — the online set is a MASK and may be sparse (metal has
            // brought up 3 of 4), and a count-bounded loop would silently drop the highest core.
            #[cfg(target_arch = "x86_64")]
            let count = core::cmp::min(
                crate::arch::acpi::cpu_count().max(1),
                crate::arch::gdt::MAX_CPUS,
            );
            #[cfg(target_arch = "aarch64")]
            let count = crate::arch::percpu::NUM_CPUS;
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
            #[cfg(target_arch = "x86_64")]
            console.println(&alloc::format!(
                "demo tasks finished: {}", crate::arch::sched::demo_done()));
            // Not an apology: the aarch64 demo paths are the wrong shape for a counter. The
            // cooperative demo completes synchronously before preemption enables and before this
            // shell exists (a count here would read the same constant forever), and the
            // `sched_demo` burst is a balancer exercise with its own serial witness.
            #[cfg(target_arch = "aarch64")]
            console.println(
                "demo tasks: n/a — the cooperative demo completes before this shell exists; \
                 the sched_demo burst reports on serial as ':: AARCH64 SCHED-BAL:'");
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
        // RELICS (R26 clause 1): the verb is `dmesg`. What it prints IS the kernel's boot message
        // ring, which is dmesg's subject on every other system. The RING itself stays
        // `crate::bootlog` and so does the `UNAOS_BOOTLOG` knob: a module and an env knob are not
        // words an operator types at a prompt, and clause 1 is about the verb table.
        "dmesg" => {
            // GUI-WITNESS M2b: print the boot-milestone ring with timestamps — the operator's eyes at
            // the bench. On a GUI (non-usbdebug) build serial is silent and fbcon detached at the GUI
            // handoff, so this verb is the ONLY witness surface for whether PORTSW flipped, the FTDI
            // console armed (vs. failed), the EHCI HID / trackpad armed, and the block device came up.
            // Reads the same ring the serial dump shows. Snapshots under the ring lock then prints, so
            // console I/O never runs while the ring is held.
            let mut buf = [(0u64, ""); 32]; // matches bootlog::capacity()
            let n = crate::bootlog::snapshot(&mut buf);
            if n == 0 {
                console.println("dmesg: no boot milestones recorded");
            } else {
                console.println(&alloc::format!("dmesg: {} milestone(s) (oldest first):", n));
                for (ms, tag) in &buf[..n] {
                    console.println(&alloc::format!("  [{:>8} ms] {}", ms, tag));
                }
            }
        },
        // ORIN-REBOOT (baton orin-6 §5.1 + Peter's cold-boot ruling 2026-08-25): the arch-neutral
        // POWER VERBS' service arms — thin hooks only. The mechanisms live in `power::reboot` /
        // `power::shutdown` (aarch64: PSCI SYSTEM_RESET / SYSTEM_OFF via SMC — the firmware owns
        // the machine; x86 shutdown routes to the real ACPI S5; unwired platform slots refuse with
        // honest witnesses + hlt park). This retires the old `shutdown` TODO stub, which "shut
        // down" by double-parking in `hlt` — a machine that idles when asked to power off is
        // neither cold-boot-ready nor honest. The console line goes out BEFORE each call because a
        // successful reset/off kills the machine mid-instruction — same last-line discipline as
        // `acpi_power::poweroff`.
        "shutdown" | "off" => {
            console.println("shutting down: invoking the platform firmware mechanism...");
            crate::power::shutdown();
        },
        "reboot" => {
            console.println("rebooting: invoking the platform firmware mechanism...");
            crate::power::reboot();
        },
        #[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
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
        #[cfg(any(all(feature = "baremetal", target_arch = "aarch64"), target_arch = "x86_64"))] // NOT widened to `tegra_el0` with its neighbours, DELIBERATELY — the one process verb that is not. `storm` is the only arm reaching past the process table into board hardware: it reads `storm_slots` (= `arch::boot`, the BCM2711 slot pool; `arch::uslots` is the facade an Orin port would use) and spawns `storm_fat_writer`, which drives `BlockSource::Usb` and is `#[cfg(feature = "baremetal")]` with no arch arm at all. Whether the Orin gets a FAT-writer leg under storm is a HW-JETSON question about that board's storage, not a gate typo — left to that seat rather than guessed at here. Folded onto this line to stay line-neutral (PARITY.md 5.3).
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
            // BGREAP-CLOSE: `dead` counts occupied rows whose pid the kernel no longer knows — the
            // close-box residue. It is the term that made the metal reading unreadable: `job rows
            // free=4/12` next to `proc rows free=9 of 10` looked like a leak with no name, and the
            // count says how much of the shortfall the next launch will reclaim by itself.
            let (jobs_free, jobs_rows, jobs_dead) = {
                let j = BG_JOBS.lock();
                let dead = j
                    .iter()
                    .flatten()
                    .filter(|job| {
                        matches!(
                            crate::arch::syscall::bg_poll(job.pid, false),
                            crate::arch::syscall::BgPoll::Gone
                        )
                    })
                    .count();
                (j.iter().filter(|s| s.is_none()).count(), j.len(), dead)
            };
            let slots_free = storm_slots::user_slots_free();
            console.println(&alloc::format!(
                "storm: n={} — {}/{} process rows free, {}/{} job rows, {}/{} user slots",
                n, rows_free, rows, jobs_free, jobs_rows,
                slots_free, storm_slots::USER_SLOTS
            ));
            serial_println!(
                ":: STORM: begin n={} | proc rows free={} running={} exited={} porphaned={} of {} | job rows free={}/{} dead={} | user slots free={}/{} ::",
                n, rows_free, rows_running, rows_exited, rows_orphaned, rows,
                jobs_free, jobs_rows, jobs_dead, slots_free, storm_slots::USER_SLOTS
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
        #[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
        "jobs" => {
            // BGRUN-1: list background programs and REAP the exited ones (this verb is the reaper — a
            // PEXITED row stays claimed until it is polled here, and the table is bounded). `jobs`.
            bg_jobs(console);
        },
        #[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
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

/// JD19: parse a `u64` offset/length accepting decimal or `0x`-hex (the `hexdump` off/len args).
fn parse_num(s: &str) -> Option<u64> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u64>().ok()
    }
}

// --- SHELL-WRITE: the one namespace's write surface (`mount <op> <path> [text]`) ---------
//
// The FIRST consumer of the VFS-2 write surface. Each invocation builds the
// process namespace fresh (stateless, like the FAT verbs — a swapped card is
// picked up on the next command) and drives the `MountTable` create / write /
// truncate / unlink verbs. The shell is the trusted operator console, so it
// writes as the kernel-authority principal (`KERNEL_PRINCIPAL`) — the same
// posture the retired `u*` native verbs recorded. Namespace: native UnaFS at `/`, the FAT
// boot partition at `/fat`, and the hot-plugged USB FAT stick at `/usb` when
// present (VFS-3, read-only). Both real backends are aarch64-only (the x86 build
// has neither the unafs module nor a `VfsBackend for FatBackend` impl), so the
// x86 arm is an honest "unsupported on this arch" line.

/// EXEC-1 / X86RUN / VFSROUTE: read an ELF64 user image off the VFS namespace, or explain why not.
///
/// **One body, both arches.** It used to be two: an aarch64 twin that went through the mount table
/// and an x86 twin that mounted the program source and walked `fat.rs` itself, with the `/fat`
/// prefix hand-rolled as a string rewrite because x86 "has no VFS namespace". x86 has one now — the
/// mount table binds the program source at `/` and `/fat` — so the prefix is a real mount and both
/// arches ask the same resolver. Every check that could say NO still says it, and each in the same
/// words as before.
///
/// Two things genuinely differ and stay `cfg`-split, because they are hardware facts and not
/// preferences: the size ceiling (the ring-3 window each arch maps) and the expected `e_machine`
/// (183 = EM_AARCH64, 62 = EM_X86_64). The loader re-checks the machine from scratch either way —
/// this pre-check only sharpens the operator's error text.
///
/// LAUNCHPACE (x86): the storage-phase breakdown is preserved, re-pointed at the routed phases —
/// `mount_us` is now the namespace build (which still does the per-launch FAT re-mount inside
/// `open_read_volume`), `dirwalk_us` the `stat` that resolves the entry, `read_us` the image read.
#[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
fn read_el0_image(console: &mut Console, verb: &str, path: &str) -> Option<alloc::vec::Vec<u8>> {
    use crate::fs::vfs::NodeKind;
    // The hard read ceiling: a file at or under it may still be rejected by the loader, but we never
    // read past it. JETSON-EL0: the aarch64 side goes through the `uslots` facade.
    #[cfg(target_arch = "aarch64")]
    let cap: u64 = crate::arch::aarch64::uslots::USER_REGION_SIZE as u64;
    #[cfg(not(target_arch = "aarch64"))]
    let cap: u64 = crate::arch::syscall::user_window_size() as u64;
    #[cfg(target_arch = "x86_64")]
    let t_entry = crate::arch::now_cycles();

    let path = &vfs_path(path)[..];
    let mt = vfs_mount_table();
    #[cfg(target_arch = "x86_64")]
    let t_mount = crate::arch::now_cycles();
    if let Some(vol) = unmounted_reserved_volume(&mt.prefixes(), path) {
        console.println(&alloc::format!(
            "{}: {}: volume {} not mounted (-ENODEV)", verb, path, vol));
        return None;
    }
    let st = match mt.stat(path) {
        Ok(s) => s,
        Err(e) => {
            console.println(&alloc::format!("{}: {}: {}", verb, path, vfs_err(e)));
            return None;
        }
    };
    #[cfg(target_arch = "x86_64")]
    let t_resolve = crate::arch::now_cycles();
    if matches!(st.kind, NodeKind::Dir) {
        console.println(&alloc::format!("{}: {}: is a directory (-EISDIR)", verb, path));
        return None;
    }
    if st.size == 0 {
        console.println(&alloc::format!("{}: {}: empty file", verb, path));
        return None;
    }
    if st.size > cap {
        console.println(&alloc::format!(
            "{}: {}: {} bytes exceeds the {}-byte user window (-E2BIG)",
            verb, path, st.size, cap
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
    #[cfg(target_arch = "x86_64")]
    let t_read = crate::arch::now_cycles();
    if bytes.len() as u64 != st.size {
        // FATREAD-1 was exactly this class of silent mismatch (a doubled read that pushed
        // STAT.ELF/VUG.ELF past the window). Say NO out loud rather than hand the loader a short or
        // long image and let it report an unrelated reason.
        console.println(&alloc::format!(
            "{}: {}: short read — {} of {} bytes (-EIO)", verb, path, bytes.len(), st.size
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
        #[cfg(target_arch = "aarch64")]
        const WANT_MACHINE: u16 = 183; // EM_AARCH64
        #[cfg(not(target_arch = "aarch64"))]
        const WANT_MACHINE: u16 = 62; // EM_X86_64
        #[cfg(target_arch = "aarch64")]
        const WANT_NAME: &str = "aarch64";
        #[cfg(not(target_arch = "aarch64"))]
        const WANT_NAME: &str = "x86-64";
        let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
        if machine != WANT_MACHINE {
            // An image staged for the other architecture lands here with a reason an operator can
            // act on, instead of the loader's bare "wrong e_machine".
            console.println(&alloc::format!(
                "{}: {}: not an {} image (e_machine {} != {})",
                verb, path, WANT_NAME, machine, WANT_MACHINE
            ));
            return None;
        }
    }
    #[cfg(target_arch = "x86_64")]
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

#[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
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
#[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
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
/// `MAX_PROCS` says.** `video::desktop_uefi::desktop_app_service` launches the desktop app (`STAT.ELF`) at
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
#[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
static BG_JOBS: spin::Mutex<[Option<BgJob>; 12]> = spin::Mutex::new([None; 12]);

/// BGREAP-CLOSE: claim a row in [`BG_JOBS`], reclaiming provably-finished rows when the table is
/// full. **Every insert into that table must come through here** — `jobs` is the only path that
/// drops an entry, and there are launches whose job never reaches a `jobs` sweep.
///
/// ### The defect this closes
/// The kernel `Proc` row has a scavenger (`proc_reserve`'s BGRUN-SCAV) and the thread table has one
/// (WINX-7's `sys_thread_spawn` sweep). The SHELL's table had neither, and a close-box press is
/// exactly the event that needs one: `wc_close_click` -> `bg_kill` reaps the Proc row IN PLACE
/// (PROCREAP) without ever touching `BG_JOBS`, because it runs from the input pump and cannot take
/// the shell's lock across a compositor repaint. So every window the operator closes retires a Proc
/// row and a user slot while leaving a `BG_JOBS` row pointing at a pid that no longer exists.
/// Metal, 2026-08-18: `proc rows free=9 of 10 | job rows free=4/12 | user slots free=11/12` — the
/// job table was the only census that did not recover, and it is a hard ceiling on the next storm.
///
/// ### Shape: lazy reclaim, not eager free
/// The WINX-7 idiom, for the same reason: the eager site (`wc_close_click`) runs on the input pump
/// and taking this lock there would put a compositor repaint inside the shell's critical section.
/// Under pressure the lock is already held by the one caller that needs the row.
///
/// Two tiers, so job history stays readable for as long as it can be:
///  * **Gone** — the kernel does not know the pid. Killed by the close box, or scavenged by
///    BGRUN-SCAV. The exit status is ALREADY unrecoverable, so reclaiming loses nothing.
///  * **finished** — exited/faulted/closed and never reaped. Reclaimed only when tier 1 freed
///    nothing, and the witness PRINTS THE OUTCOME, so the status the operator would have seen from
///    `jobs` is on the wire rather than dropped (the BGRUN-SCAV "never silent" rule).
///
/// A `Running` row is never touched: a full table of live jobs is a genuine refusal, and the caller
/// still kills the pid it just spawned rather than leave it untrackable.
#[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
fn bg_jobs_claim(jobs: &mut [Option<BgJob>; 12]) -> Option<usize> {
    use crate::arch::syscall::BgPoll;
    if let Some(i) = jobs.iter().position(|s| s.is_none()) {
        return Some(i);
    }
    let mut freed: Option<usize> = None;
    // Tier 1 — rows the kernel has already forgotten. Lossless.
    for i in 0..jobs.len() {
        let Some(job) = jobs[i] else { continue };
        if matches!(crate::arch::syscall::bg_poll(job.pid, false), BgPoll::Gone) {
            jobs[i] = None;
            serial_println!(
                ":: BGREAP: job table full — reclaimed row {} from dead pid {} ({}) (the kernel row is already gone: closed, killed or scavenged) ::",
                i,
                job.pid,
                core::str::from_utf8(&job.name[..job.nlen as usize]).unwrap_or("?")
            );
            if freed.is_none() {
                freed = Some(i);
            }
        }
    }
    if freed.is_some() {
        return freed;
    }
    // Tier 2 — finished but unreaped. `bg_poll(reap = true)` is safe under this lock for the same
    // reason `bg_jobs` relies on (a PEXITED row's `done` permit is posted before the state is
    // published, and the reap arm uses `try_wait`); a `Running` row is unaffected by the flag.
    for i in 0..jobs.len() {
        let Some(job) = jobs[i] else { continue };
        let verdict = crate::arch::syscall::bg_poll(job.pid, true);
        if matches!(verdict, BgPoll::Running) {
            continue;
        }
        match verdict {
            BgPoll::Exited(code) => serial_println!(
                ":: BGREAP: job table full — reclaimed row {} from pid {} (exit={} DISCARDED; `jobs` never read it) ::",
                i, job.pid, code
            ),
            _ => serial_println!(
                ":: BGREAP: job table full — reclaimed row {} from finished pid {} (no exit status to report; `jobs` never read it) ::",
                i, job.pid
            ),
        }
        jobs[i] = None;
        return Some(i);
    }
    None
}

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
#[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
fn bg_program(console: &mut Console, path: &str) -> bool {
    let Some(bytes) = read_el0_image(console, "bg", path) else {
        return false;
    };
    let n = bytes.len();
    match crate::arch::syscall::spawn_user_image_bg(&bytes) {
        Ok((pid, asid, entry)) => {
            let mut jobs = BG_JOBS.lock();
            // BGREAP-CLOSE: `bg_jobs_claim` reclaims rows whose job is provably finished before it
            // reports the table full — a close-box press retires the kernel row without telling this
            // table, and `jobs` is the only other path that drops an entry.
            let Some(idx) = bg_jobs_claim(&mut jobs) else {
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
            jobs[idx] = Some(BgJob { pid, asid, name, nlen: nlen as u8 });
            console.println(&alloc::format!("bg: {} started — pid {} (see `jobs`)", path, pid));
            serial_println!(
                ":: BGRUN: bg {} — loaded {} bytes, entry {:#x}, pid={} slot={} (window layer arms asid=slot+1) DETACHED ::",
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
#[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
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
#[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
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
/// [`crate::video::desktop_uefi::desktop_app_service`], which launches the desktop app at boot with nobody at
/// the prompt to type `bg`. Registering it here is what keeps `jobs` and `kill` TRUTHFUL —
/// `bg_kill_cmd` resolves a pid through this table and REFUSES one it cannot find, so an
/// unregistered launch would be a running ring-3 program the operator can neither list nor stop.
#[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
pub(crate) fn adopt_bg_job(pid: u64, slot: u64, name: &str) -> bool {
    let mut jobs = BG_JOBS.lock();
    // BGREAP-CLOSE: same claim as `bg_program`'s — see `bg_jobs_claim`.
    let Some(idx) = bg_jobs_claim(&mut jobs) else {
        return false;
    };
    let mut buf = [0u8; 32];
    let nlen = name.len().min(32);
    buf[..nlen].copy_from_slice(&name.as_bytes()[..nlen]);
    jobs[idx] = Some(BgJob { pid, asid: slot, name: buf, nlen: nlen as u8 });
    true
}

/// BARENAME (PARITY §6.6a): the aarch64 program-source root — the namespace spelling of x86's
/// "the volume executables live on".
///
/// x86 has no VFS: its whole path universe is the program-source FAT, so there "resolve from the
/// cwd" and "resolve on the volume executables live on" are the same sentence, and `/fat` is
/// carried only as an alias for that one volume's root. On the Pi the two come apart — `/` is
/// native UnaFS and `arroyo`'s `kernel8` FAT staging puts `VUG.ELF`/`VUGC.ELF`/`VUGX.ELF`/
/// `STAT.ELF`/`PULSE.ELF` on the SD FAT, which `vfs_mount_table` binds at `/fat`. This constant is
/// that half of the x86 sentence, named rather than inlined.
#[cfg(all(feature = "aarch64_el0", target_arch = "aarch64"))]
const EXEC_ROOT: &str = "/fat";

/// BARENAME (PARITY §6.6a): resolve a bare-name candidate to an absolute VFS path, or `None`.
///
/// **Through the VFS seam, not a private path scheme.** [`vfs_path`] is what `ls`, `cat`, `run`,
/// `bg` and `mount` resolve through, so a bare name means exactly what those verbs say it means —
/// `cd /fat` then `vug` works for the same reason `cd /fat` then `cat VUG.ELF` works, and a name
/// that `ls` cannot show is a name this cannot launch.
///
/// Order, and it is x86's order transposed rather than a new policy:
///
/// 1. **cwd-relative**, via `vfs_path` — the x86 first (and only) probe, verbatim in intent.
/// 2. **The program-source root**, [`EXEC_ROOT`] — only for a RELATIVE token, and skipped when it
///    would repeat probe 1. On x86 this step is not absent, it is *implied*: its cwd already sits
///    on the program source, so its single probe covers both. Dropping it on the Pi would mean the
///    operator at `/` still could not type `vug` — the exact defect §6.6a names, with `bg
///    /fat/VUG.ELF` still the only way in — so it is the step that makes the port a port.
///
/// A directory never resolves: a bare name launches a program.
#[cfg(all(feature = "aarch64_el0", target_arch = "aarch64"))]
fn exec_resolve(name: &str) -> Option<String> {
    use crate::fs::vfs::NodeKind;
    let mt = vfs_mount_table();
    let probe = |p: String| -> Option<String> {
        match mt.stat(&p) {
            Ok(st) if !matches!(st.kind, NodeKind::Dir) => Some(p),
            _ => None,
        }
    };
    let from_cwd = vfs_path(name);
    if let Some(hit) = probe(from_cwd.clone()) {
        return Some(hit);
    }
    if name.starts_with('/') {
        return None;
    }
    let from_root = normalize_path(EXEC_ROOT, name);
    if from_root == from_cwd {
        return None;
    }
    probe(from_root)
}

/// BARENAME (PARITY §6.6a): the on-disk spelling of an already-resolved path, for the messages.
///
/// x86 reads `canon` out of the FAT directory entry its re-resolve walked, so `jobs` shows
/// `/VUG.ELF` after the operator typed `vug`. The VFS `stat` this arch resolves through returns no
/// name at all, so the spelling is recovered the only honest way available: list the parent and
/// take the entry that matches case-insensitively. A miss (or an unreadable parent) falls back to
/// the resolved path unchanged — a display name is never worth a refusal.
#[cfg(all(feature = "aarch64_el0", target_arch = "aarch64"))]
fn exec_canon(path: &str) -> String {
    let (dir, leaf) = match path.rfind('/') {
        Some(0) => ("/", &path[1..]),
        Some(i) => (&path[..i], &path[i + 1..]),
        None => return String::from(path),
    };
    if let Ok(rows) = vfs_mount_table().read_dir(dir) {
        if let Some(row) = rows.iter().find(|r| r.name.eq_ignore_ascii_case(leaf)) {
            return normalize_path(dir, &row.name);
        }
    }
    String::from(path)
}

/// BARE-EXEC: re-resolve the core's answer over the live volume — the one genuinely per-arch step
/// of a bare-name launch, split out so everything after it is ONE body on both arches.
///
/// Returns `(load_path, canon)`: what to hand [`read_el0_image`], and the spelling the operator and
/// the capture are shown. On x86 `load_path` is the token the core returned, untouched, because
/// that arch's loader resolves it identically and quoting anything else would change lines a
/// shipping spec anchors on. On aarch64 it is the ABSOLUTE VFS path [`exec_resolve`] found, because
/// there the token alone is ambiguous between the native root and the program source.
///
/// Both twins OWN their refusal: a miss here is a volume that changed under us between the core's
/// probe and this read (a card pulled mid-command) — an honest race, reported as one rather than as
/// a typo — so each prints its panel line and its serial mirror before returning `None`.
///
/// LAUNCH-AR (x86): the PROGRAM SOURCE, matching `FatVolume::is_file` above and `read_el0_image`
/// below. All three legs of a bare-name launch — probe, re-resolve, read — bind the same handle;
/// the Boot AR failure was exactly what happens when they do not.
#[cfg(target_arch = "x86_64")]
fn bare_exec_reresolve(console: &mut Console, typed: &str, name: &str) -> Option<(String, String)> {
    // VFSROUTE: the re-resolve asks the NAMESPACE, like every other verb path. The independent
    // handle the FATVERB witness compares against is the exec PROBE (`FatVolume::is_file`), which is
    // deliberately left binding `mount_program_source` directly — see its note.
    let mt = vfs_mount_table();
    if mt.prefixes().is_empty() {
        console.println(&alloc::format!("{}: the volume went away before it could be started", typed));
        serial_println!(":: BAREXEC: {} (typed '{}') — REFUSED: volume vanished after resolution ::", name, typed);
        return None;
    }
    let canon = vfs_path(name);
    match mt.stat(&canon) {
        Ok(st) if !matches!(st.kind, crate::fs::vfs::NodeKind::Dir) =>
            Some((String::from(name), canon)),
        _ => {
            console.println(&alloc::format!("{}: {} went away before it could be started", typed, name));
            serial_println!(":: BAREXEC: {} (typed '{}') — REFUSED: resolved name no longer a file ::", name, typed);
            None
        }
    }
}

/// BARENAME (PARITY §6.6a): the aarch64 twin — [`exec_resolve`] again (the same walk the probe
/// made, one command ago, closing the same race x86's re-mount closes) plus [`exec_canon`] for the
/// display spelling. Same two message shapes, so a Pi capture and an rMBP capture read alike.
#[cfg(all(feature = "aarch64_el0", target_arch = "aarch64"))]
fn bare_exec_reresolve(console: &mut Console, typed: &str, name: &str) -> Option<(String, String)> {
    let Some(path) = exec_resolve(name) else {
        console.println(&alloc::format!("{}: {} went away before it could be started", typed, name));
        serial_println!(":: BAREXEC: {} (typed '{}') — REFUSED: resolved name no longer a file ::", name, typed);
        return None;
    };
    let canon = exec_canon(&path);
    Some((path, canon))
}

/// BARE-EXEC (GR20; aarch64 since PARITY §6.6a): run a program by TYPING ITS NAME — `vug.elf` at
/// the prompt starts
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
/// **On aarch64 (PARITY §6.6a)** the same two sentences hold with one substitution: the resolution
/// is `exec_resolve` — the VFS seam `ls`/`cat`/`run`/`bg` share, cwd first and then the
/// program-source root `/fat` — and `canon` is recovered by [`exec_canon`] from the parent listing
/// rather than from a FAT directory entry. Case behaves the same way for the same reason: the FAT
/// backend behind `/fat` matches components case-insensitively, so arm 2 of the core's resolver
/// (`vug` → `vug.elf`) already hits the on-disk `VUG.ELF` and the upper-cased arm 3 stays latent
/// here too.
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
#[cfg(any(all(feature = "aarch64_el0", target_arch = "aarch64"), target_arch = "x86_64"))]
fn bare_exec(console: &mut Console, typed: &str, name: &str) -> bool {
    // --- re-resolve the core's answer over the live volume ---------------------------------------
    // The core probed a moment ago; re-resolving costs one walk and closes the window where the
    // volume changed underneath. A miss is a RACE, not a typo, and `bare_exec_reresolve` says so —
    // it is the ONE per-arch step, and it has already printed if it returns `None`.
    let Some((load_path, canon)) = bare_exec_reresolve(console, typed, name) else {
        return false;
    };
    // --- loud from here: the name is a real file, so we owe an outcome --------------------------
    // Every refusal below is mirrored to serial as well as the panel. The panel line is what the
    // operator reads; the serial line is what a headless capture reads, and without it a bench log
    // could not tell "refused, and here is why" from "the keystroke never arrived".
    let Some(bytes) = read_el0_image(console, typed, &load_path) else {
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
    // The ELF64 / little-endian / e_machine pre-checks already ran inside `read_el0_image` (the
    // arch's own twin, so EM_X86_64 there and EM_AARCH64 here), which named any of them; the kernel
    // loader re-validates from scratch regardless.
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

/// VFS-1 (adoption): **the seam** — the ONE place a shell verb turns an operator-typed argument into
/// an absolute VFS path. Every routed verb (`ls`, `cat`, `run`, `bg`, `mount`, and since RELICS the
/// plain mutating verbs) calls this and nothing
/// else; which volume the path lands on is then decided solely by `MountTable::resolve`'s
/// longest-prefix rule, never by the verb.
///
/// It is `normalize_path` against the cwd — purely lexical, so `.` collapses and `..` pops before any
/// backend is consulted, and a relative `VUG.ELF` means what `pwd` says it means. That last part is a
/// FIX, not just a move: `run`, `bg` and `mount` used their argument VERBATIM, so after a `cd` they
/// silently resolved against the root while every other verb honoured the cwd. One seam means one
/// answer to "what does this path name", which is the point of the layer.
fn vfs_path(arg: &str) -> String {
    normalize_path(&cwd_path(), arg)
}

/// VFSROUTE (orin 17): **the namespace**, built once per verb, on BOTH arches.
///
/// It was `#[cfg(target_arch = "aarch64")]` from VFS-1 — not because a mount table is an aarch64
/// idea, but because the Pi came first and x86's verbs were still FAT-direct. That gate is what made
/// `ls` and `cat` carry two bodies, and Peter's ruling is that they should carry none: a mounted
/// filesystem is listable because it implements the backend trait, whatever the board.
///
/// **aarch64** binds `/` = native UnaFS, `/fat` = the SD boot partition, and `/usb` = the stick when
/// it is actually enumerated (honest hot-plug, doc §6). The Orin's ROOTFS knob re-points both `/` and
/// `/fat` at the Tegra card, since this machine has neither of the first two volumes.
///
/// **x86** binds THE PROGRAM SOURCE — `crate::drivers::block::program_source`, resolved through
/// [`open_read_volume`] so the READ_BIND instrument is stamped exactly as it was when each verb
/// mounted for itself. That is FATVERB's law, unchanged: the verbs and the exec probe must bind the
/// same handle, and on a machine booted from the internal SD reader the global slot is the wrong
/// one. It is bound at BOTH `/` and `/fat`, because `/fat` is the spelling the packaging text, the
/// staged-image script and `exec_resolve`'s second probe all use for that one volume — the same
/// two-prefix shape `sdmmc_root_bind` already uses on the Orin, and honest for the same reason
/// (`/fat` IS a mount point, so `ls /` showing it is a fact, not decoration).
///
/// An arch with no volume at all returns an EMPTY table, and the verbs report "no filesystem
/// mounted (-ENODEV)" — which is a better answer than the pre-VFSROUTE `no FAT filesystem (NoDisk)`
/// because it does not name a filesystem the operator never asked about.
pub(crate) fn vfs_mount_table() -> crate::fs::vfs::MountTable {
    use crate::fs::vfs::{FatBackend, MountTable, KERNEL_PRINCIPAL};
    #[allow(unused_mut)]
    let mut mt = MountTable::new();
    #[cfg(target_arch = "aarch64")]
    {
        use crate::fs::vfs::NativeBackend;
        mt.mount("/", alloc::boxed::Box::new(NativeBackend::new("native")));
        mt.mount("/fat", alloc::boxed::Box::new(FatBackend::new("fat", KERNEL_PRINCIPAL, true))); #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmcroot"))] crate::arch::aarch64::sdmmc_tegra::sdmmc_root_bind(&mut mt); // ROOTFS (orin 16, A28): on the Orin `/` (native UnaFS) and `/fat` (BlockSource::Default) BOTH name volumes this machine does not have, so `ls /` answered `backend error: unafs-mount`; this re-points BOTH at the card's FAT through BlockSource::TegraSd (`/fat` too, because it is EXEC_ROOT and the literal prefix of /fat/VUG.ELF etc). See arch/aarch64/sdmmc_tegra.rs §ROOTFS.
        // VFS-3: bind the USB stick at /usb only when it is present (honest hot-plug).
        if crate::fs::fat::mount_source(crate::fs::fat::BlockSource::Usb).is_ok() {
            mt.mount("/usb", alloc::boxed::Box::new(FatBackend::new_usb("usb", KERNEL_PRINCIPAL)));
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        // FATVERB: the program source, and its READ_BIND stamp, in the one place a verb now binds.
        if let Ok(fs) = open_read_volume() {
            let src = fs.source();
            mt.mount("/", alloc::boxed::Box::new(
                FatBackend::new_source("fat", KERNEL_PRINCIPAL, true, src)));
            mt.mount("/fat", alloc::boxed::Box::new(
                FatBackend::new_source("fat", KERNEL_PRINCIPAL, true, src)));
        }
    }
    mt
}

/// Render a `VfsError` as an errno-style operator line, matching the shell's
/// `-ENOENT`/`-EISDIR` house style.
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
fn vfs_say(console: &mut Console, line: &str) {
    console.println(line);
    serial_println!(":: vfsw: {} ::", line);
}

/// VFS-4: the namespace prefixes the shell's `mount` verb reserves for DISTINCT
/// backing volumes that may be absent. A mutating verb aimed at one of these when
/// it is NOT currently mounted must report "volume not mounted" — never fall
/// through to the native root, which mis-reports a bare `-ENOENT`. On the P44
/// sitting a `mount write /usb/…` with the stick's FAT unreadable (its READ(10)
/// LBA0 returned all-zeros with a passing CSW, so `mount_source(Usb)` honestly
/// found no FAT and `/usb` never bound) fell through to native-root create,
/// which failed resolving the parent `/usb` as a native path and said
/// "no such file or directory (-ENOENT)". That misdirection cost bench time; the
/// honest answer is that the *volume* is not mounted. `/` (native) is excluded —
/// it is always mounted and is the legitimate fall-through for un-prefixed paths.
const RESERVED_VOLUME_PREFIXES: &[&str] = &["/usb", "/fat"];

/// VFS-4: if `path` targets a reserved volume prefix (see
/// [`RESERVED_VOLUME_PREFIXES`]) that is not present in the live `mounted`
/// prefix set, return that prefix. Boundary-matched exactly as the resolver is
/// (§4): `/usb` and `/usb/…` name the volume, but `/usbfoo` does NOT (it is a
/// native-root name and legitimately resolves there).
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

/// RELICS (R26 clause 1) / VFSROUTE (orin 17): `mount <write|append|rm|mkdir> <path> [text ...]`.
///
/// **It is now an ALIAS, and that is the point.** Before VFSROUTE this was the ONLY write surface
/// that spoke to the mount table — the plain verbs were FAT-direct, so `mount write /X hi` and
/// `write /X hi` did different things on different volumes. Now the plain verbs ARE the routed
/// surface, so keeping a second implementation here would be exactly the duplication Peter's ruling
/// is about. The subcommands stay because the bench playbooks and `docs/dev/OS/09_FILESYSTEM/vfs.md`
/// spell them, and they cost one line each to forward.
///
/// A bare `mount` (no arguments) is the TABLE — see [`df_report`], which renders one row per mount
/// with each volume's own capacity, access posture and description.
fn vfs_cmd(console: &mut Console, args: &[&str]) {
    let op = match args.first() {
        Some(&o) => o,
        None => {
            console.println("usage: mount <write|append|rm|mkdir> <path> [text ...]");
            console.println("  bare `mount` lists the namespace: prefix, volume, capacity, access");
            return;
        }
    };
    let path = match args.get(1) {
        Some(&p) => p,
        None => {
            console.println(&alloc::format!("usage: mount {} <path> [text ...]", op));
            return;
        }
    };
    match op {
        "write" => {
            let mut data = args[2..].join(" ").into_bytes();
            data.push(b'\n');
            fs_write(console, path, &data);
        }
        "append" => {
            let mut data = args[2..].join(" ").into_bytes();
            data.push(b'\n');
            fs_append(console, path, &data);
        }
        "rm" => fs_rm(console, path, false),
        "mkdir" => fs_mkdir(console, path),
        other => console.println(&alloc::format!(
            "mount: unknown op '{}' (write|append|rm|mkdir)", other)),
    }
}

// --- RELICS (R26 clause 2): the two survivors of the `u*` family --------------
//
// `setfattr` and `snap` are the only members with no plain file verb to retire into: nothing else
// in the shell drops a typed attribute, and nothing else retains a tree. Both take the STANDARD
// spelling for the job rather than a `u`-prefixed one, and both are registered on EVERY build
// (R26 clause 3) — the ring arm below says what THIS platform can do, which is what a platform is
// allowed to decide. The x86 twins are honest refusals, the shape `vfs_cmd` already used.

/// `setfattr -x <key> <path>` — drop one typed attribute from the object at `<path>`.
///
/// VFSROUTE: routed, and therefore ONE body on both arches. `remove_attr` is a backend capability:
/// the native UnaFS backend implements it, the FAT backend inherits the trait's refusal, so
/// `setfattr -x k /fat/F` prints `-ENOTSUP` — FAT's own honest answer — instead of the verb knowing
/// in advance which volume has typed attributes. The previous shape was a `target_arch` split with
/// an x86 body that refused by name; that refusal was right about x86 for the wrong reason (it is
/// the VOLUME that has no attributes, not the architecture).
fn setfattr_x(console: &mut Console, key: &str, path: &str) {
    let Some((mt, path)) = vfs_write_open(console, "setfattr", path) else { return };
    match mt.remove_attr(&path, key, SHELL_PRINCIPAL) {
        Ok(()) => vfs_say(console, &alloc::format!("setfattr: removed '{}' from {}", key, path)),
        Err(e) => vfs_say(console, &alloc::format!(
            "setfattr: {}: {}: {}", path, key, vfs_err(e))),
    }
}

/// `snap list|create|drop|ls|cat` — the retained-root family, one verb.
///
/// Subcommand shapes, and why each is what it is: `list` takes nothing (it is the index); `create
/// <name>` names the new retained root; `drop <gen>` takes the GENERATION stamp `list` prints, not
/// the name, because names are not unique and a generation is; `ls <gen> [path]` and `cat <gen>
/// <path>` read AS OF a snapshot and enforce the LIVE object's current ACL (the K8c ruling — a file
/// deleted from the live tree has no current ACL row and therefore fails closed).
#[cfg(target_arch = "aarch64")]
fn snap_cmd(console: &mut Console, args: &[&str]) {
    let usage = "usage: snap list | snap create <name> | snap drop <gen> | snap ls <gen> [path] | snap cat <gen> <path>";
    match args.first().copied() {
        None | Some("list") => {
            for line in &unafs_verb_snaps() {
                console.println(line);
            }
        }
        Some("create") => match args.get(1).copied() {
            None => console.println("usage: snap create <name>"),
            Some(name) => console.println(&unafs_verb_snap(name)),
        },
        Some("drop") => match args.get(1).copied().and_then(|s| s.parse::<u64>().ok()) {
            None => console.println("usage: snap drop <generation>"),
            Some(generation) => console.println(&unafs_verb_snapdrop(generation)),
        },
        Some("ls") => match args.get(1).copied().and_then(|s| s.parse::<u64>().ok()) {
            None => console.println("usage: snap ls <generation> [path]"),
            Some(generation) => {
                let path = args.get(2).copied().unwrap_or("/");
                for line in &unafs_verb_snapls(generation, path) {
                    console.println(line);
                }
            }
        },
        Some("cat") => match (
            args.get(1).copied().and_then(|s| s.parse::<u64>().ok()),
            args.get(2).copied(),
        ) {
            (Some(generation), Some(path)) => console.println(&unafs_verb_snapcat(generation, path)),
            _ => console.println("usage: snap cat <generation> <path>"),
        },
        Some(other) => {
            console.println(&alloc::format!("snap: unknown subcommand '{}'", other));
            console.println(usage);
        }
    }
}

/// x86 has no native volume, so there is nothing to retain. Honest refusal, by name.
#[cfg(not(target_arch = "aarch64"))]
fn snap_cmd(console: &mut Console, _args: &[&str]) {
    console.println("snap: no native volume on this build (retained roots are a UnaFS feature)");
}

// --- BeFS-K4 native unafs write verbs -----------------------------------------
// The native unafs volume uses ABSOLUTE, case-sensitive paths (no shell cwd).
// Every verb routes through the single coherent mount (`crate::fs::unafs::
// with_unafs`), so the in-RAM allocation bitmap/journal stay authoritative and
// each mutation is write-through + durable. Kept in this dedicated region (not
// among the FAT-verb helpers above) so the pi4 unafs lane stays trivially
// separable from the concurrent jetson FAT-verb work.









/// RELICS (R26 clause 2) / K8b: list retained snapshots (the on-disk snapshot index) on the native
/// volume — the body the retired `usnaps` arm carried inline, lifted so `snap list` can call it and
/// so the whole snapshot family sits together with its siblings.
#[cfg(target_arch = "aarch64")]
fn unafs_verb_snaps() -> alloc::vec::Vec<String> {
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
        Err(e) => alloc::vec![alloc::format!("snap list: {:?}", e)],
    });
    match out {
        Ok(lines) => lines,
        Err(e) => alloc::vec![alloc::format!("snap list: no unafs volume ({:?})", e)],
    }
}

/// `snap create <name>`: retain the current committed tree as a snapshot. The shell
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
        Ok(Ok(generation)) => alloc::format!("snap create: retained '{}' (generation {})", name, generation),
        Ok(Err(msg)) => alloc::format!("snap create: {}: {}", name, msg),
        Err(e) => alloc::format!("snap create: no unafs volume ({:?})", e),
    }
}

/// `snap drop <generation>`: drop a retained snapshot; reclamation drains
/// eagerly, freeing only blocks no live/retained root still reaches.
#[cfg(target_arch = "aarch64")]
fn unafs_verb_snapdrop(generation: u64) -> String {
    match crate::fs::unafs::with_unafs(|fs| {
        fs.snapshot_drop(generation)
            .map_err(|e| alloc::format!("{:?}", e))
    }) {
        Ok(Ok(())) => alloc::format!("snap drop: dropped generation {} (blocks reclaimed)", generation),
        Ok(Err(msg)) => alloc::format!("snap drop: generation {}: {}", generation, msg),
        Err(e) => alloc::format!("snap drop: no unafs volume ({:?})", e),
    }
}

/// `snap ls <gen> [path]`: list a retained snapshot's directory AS OF the
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
                    return alloc::vec![alloc::format!("snap ls: no such snapshot generation {}", generation)];
                }
                Err(e) => return alloc::vec![alloc::format!("snap ls: {:?}", e)],
            };
            match view.resolve_path(path) {
                Ok(id) => id,
                Err(_) => return alloc::vec![alloc::format!("snap ls: {}: not in snapshot", path)],
            }
        };
        // CURRENT-ACL on the live directory — the same evaluator as usnapcat.
        match read_authz(fs, dir_id, KERNEL_PRINCIPAL) {
            ReadAuthz::Permit => {}
            ReadAuthz::DenyNoLiveObject => {
                return alloc::vec![alloc::format!(
                    "snap ls: {}: refused — directory deleted from live tree (no current ACL; fail-closed)",
                    path
                )];
            }
            ReadAuthz::DenyAcl => {
                return alloc::vec![alloc::format!(
                    "snap ls: {}: refused — current ACL denies this principal",
                    path
                )];
            }
        }
        let mut view = match fs.open_snapshot(generation) {
            Ok(v) => v,
            Err(e) => return alloc::vec![alloc::format!("snap ls: {:?}", e)],
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
            Err(e) => alloc::vec![alloc::format!("snap ls: {:?}", e)],
        }
    });
    match out {
        Ok(lines) => lines,
        Err(e) => alloc::vec![alloc::format!("snap ls: no unafs volume ({:?})", e)],
    }
}

/// `snap cat <gen> <path>`: read a file from a retained snapshot under the LIVE
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
                Ok(s) => alloc::format!("snap cat: gen {} {} ({} bytes)\n{}", generation, path, bytes.len(), s),
                Err(_) => alloc::format!("snap cat: gen {} {} ({} bytes, binary)", generation, path, bytes.len()),
            }
        }
        Ok(SnapReadResult::NotInSnapshot) => {
            alloc::format!("snap cat: {}: not in snapshot gen {}", path, generation)
        }
        Ok(SnapReadResult::SnapshotMissing) => {
            alloc::format!("snap cat: no such snapshot generation {}", generation)
        }
        Ok(SnapReadResult::Refused(ReadAuthz::DenyNoLiveObject)) => alloc::format!(
            "snap cat: {}: refused — object deleted from live tree (no current ACL; fail-closed)",
            path
        ),
        Ok(SnapReadResult::Refused(ReadAuthz::DenyAcl)) => {
            alloc::format!("snap cat: {}: refused — current ACL denies this principal", path)
        }
        Ok(SnapReadResult::Refused(ReadAuthz::Permit)) => {
            // Unreachable (Permit is not a refusal) — reported rather than panicked.
            alloc::format!("snap cat: {}: internal: permit reported as refusal", path)
        }
        Err(e) => alloc::format!("snap cat: no unafs volume ({:?})", e),
    }
}
