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
use crate::vug;
use crate::pal::TargetPal;

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
// the kernel shell runs at EL1 as ASID 0 (on the tegra post-drop core TTBR0_EL1[63:48] = 0 — it
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
// (A future arc that runs EL0 tasks and returns to the shell must re-establish ASID 0 before shell
// FAT ops — today the shell is cooperative EL1 and never installs a user-slot TTBR0.)
//
// SCOPE (JD6): the WHOLE tree the shell can `cd` into. `resolve_write_target` normalizes the path
// against the cwd and walks to the PARENT directory via the read-only `resolve_path`, then the
// writes ride the dir-aware `create_in_dir`/`locate_in_dir` twins (`first_cluster == 0` ⇒ root).
// A parent that is a plain file is `-ENOTDIR`; a missing parent `-ENOENT`; a FULL directory
// `-ENOSPC` (extending a subdir's cluster chain is out of scope — the twins add a slot but never grow
// the directory chain). JD7 layers `mkdir`/`rmdir` on top via the `fat::create_dir`/`remove_dir`
// FATDIRS seam (call-never-edit, like JD6's write path): `rm` stays file-only (`-EISDIR` on a
// directory — use `rmdir`), and `rmdir` removes an EMPTY directory (non-empty ⇒ `-ENOTEMPTY`, the
// root refused). The seam's internal F3 locking is sound for these EL1 callers without the syscall
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
// reads — a dead transfer times out, never WFI-parks the timerless EL1 core). Writes are
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

/// JD6 `touch`: ensure a 0-length file exists at `path` in ANY directory the shell can reach
/// (create if absent; idempotent no-op if present). Rides the dir-aware `locate_in_dir` /
/// `create_in_dir` twins — the parent may be the root or any subdirectory.
fn fs_touch(console: &mut Console, arg: &str) {
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("touch: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("write: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("append: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("rm: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("mkdir: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("rmdir: no FAT filesystem ({:?})", e)),
    };
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
/// rides the JD3 wall-clock BOT pump — a stalled transfer is `-EIO`, never a hang on the timerless EL1
/// core); a future single-pass primitive could tighten that, tracked as a JD9 note.
fn fs_cp(console: &mut Console, src: &str, dst: &str, force: bool) {
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("cp: no FAT filesystem ({:?})", e)),
    };
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
/// rides the JD3 wall-clock BOT pump — a stalled transfer is `-EIO`, never a hang on the timerless EL1 core.
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("cp: no FAT filesystem ({:?})", e)),
    };
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
/// EL1 core). Depth is capped at `CP_MAX_DEPTH` (honest `-ELOOP`) — the JD9 belt-and-braces backstop
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
/// PRINCIPAL — unchanged. The shell is EL1 ASID 0, the PUBLIC principal; `rm -r` consults no U6/K-line
/// `OWNED_FILES` ACL and composes the same F3-locked `read_dir`/`locate_in_dir`/`delete_located`/
/// `remove_dir` primitives JD6/JD7/JD9 already exercise and ledger, so it inherits their locking
/// analysis unchanged (no new fat.rs surface, no new lock, no new namespace interaction).
fn fs_rm_recursive(console: &mut Console, arg: &str, force: bool) {
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("rm: no FAT filesystem ({:?})", e)),
    };
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
/// ACL NOTE: this shell is EL1 ASID 0 = the PUBLIC principal, so a panel `mv` consults no U6/K-line
/// `OWNED_FILES` ACL and is ACL-neutral by construction (the row re-key for a moved EL0-owned file is
/// a future K-line seam, ledgered in the pi4 FATMOVE SECURITY note). CRASH SAFETY is the seam's job:
/// `move_entry` publishes the destination BEFORE `0xE5`ing the source, so a power-cut mid-move leaves
/// a benign duplicate (two names, one chain), never a lost chain.
fn fs_mv(console: &mut Console, src: &str, dst: &str, force: bool) {
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("mv: no FAT filesystem ({:?})", e)),
    };
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
/// wall-clock BOT pump — a stalled transfer is `-EIO`, never a hang on the timerless EL1 core.
fn fs_head(console: &mut Console, arg: &str, n: u32) {
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("head: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("tail: no FAT filesystem ({:?})", e)),
    };
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
// The shared `ls`/`dir` arm rides `fat::mount()` — the x86 USB-storage backend. The Pi has no FAT
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
    match crate::fs::fat::mount() {
        Ok(fs) => ls_resolved(console, &fs, &normalize_path(&cwd_path(), arg), long),
        Err(e) => console.println(&alloc::format!("ls: no FAT filesystem ({:?})", e)),
    }
}

/// JD12 `ls *.EXT`: list every entry matching a wildcard, one `ls`-table line each (sorted), with the
/// file/dir tally. A directory match shows as `<DIR>` (its contents are not expanded — that mirrors
/// how a shell hands matched names to `ls`); no match is an honest "no match".
#[cfg(not(target_arch = "aarch64"))]
fn ls_globbed(console: &mut Console, arg: &str, long: bool) {
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("ls: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("cat: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("rm: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("cp: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("mv: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("find: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("du: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("stat: no FAT filesystem ({:?})", e)),
    };
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
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("xd: no FAT filesystem ({:?})", e)),
    };
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

/// Run one command. Returns `true` if the command took over the whole screen with its own
/// graphics (e.g. `vug`), so the caller should NOT repaint the console over it.
pub fn dispatch_command(cmd_line: &str, console: &mut Console, pal: &mut TargetPal) -> bool {
    // Split command and args (simple whitespace split)
    let mut parts = cmd_line.trim().split_whitespace();
    let command = parts.next().unwrap_or("");
    let args: Vec<&str> = parts.collect();

    // The `vug` and `pulse` commands paint full-screen views; everything else leaves the
    // console visible. PI-APP-1: `v3d` blits the visible battery onto the live scanout, so it too
    // keeps the console off (a repaint would overwrite the replayed tiles). Aarch64+v3d only; the
    // knob-off build never registers the command, so this OR-clause is constant-folded away there.
    #[cfg(all(target_arch = "aarch64", feature = "v3d"))]
    let took_screen = command == "vug" || command == "pulse" || command == "v3d";
    #[cfg(not(all(target_arch = "aarch64", feature = "v3d")))]
    let took_screen = command == "vug" || command == "pulse";

    match command {
        "ver" | "version" => {
            console.println("unaOS v0.1.0 (Kernel: Jules 1 / Cortex: Jules 6)");
        },
        "help" => {
            console.println("COMMANDS: ver, help, clear, echo, panic, gneiss");
            console.println("STORAGE:  diskinfo, usbinfo, read <lba>, write <lba> <byte>");
            console.println("FILES:    fatinfo (FAT geometry), ls [-l] [dir], cd [dir], pwd, cat <path>");
            console.println("PAGING:   head <path> [n], tail <path> [n]  (first / last n lines, default 10)");
            console.println("WRITE:    touch <path>, write <path> <text>, append <path> <text>, rm [-r] [-f] <path>");
            console.println("DIRS:     mkdir <path>, rmdir <path>  (create / remove empty directories)");
            console.println("VFS:      vfs write|append|rm|mkdir <path> [text]  (unified namespace: / native, /fat FAT)");
            console.println("          rm -r <dir>  (recursively delete a directory tree; root refused)");
            console.println("COPY:     cp [-f] <src> <dst>  (copy a file; cp FILE DIR/ lands as DIR/<leaf>)");
            console.println("          cp -r <srcdir> <dst>  (recursively copy a directory tree)");
            console.println("MOVE:     mv [-f] <src...> <dst>  (move/rename a file or dir, O(1); mv SRC DIR/ lands as DIR/<leaf>)");
            console.println("FLAGS:    default is no-clobber; -f = force overwrite (rm: quiet on missing), -n = no-clobber");
            console.println("WILDCARD: * / ? in the last path component — ls/cat/rm/cp/mv expand it (e.g. rm *.TMP, cp *.TXT DOCS/)");
            console.println("          (create/edit/delete/copy/move files & dirs anywhere in the tree; sync = write-through, durable)");
            console.println("TREE:     find [root] <pattern>  (recursive glob search), du [dir]  (recursive size tally)");
            console.println("          uptime  (seconds since boot; shows the wall clock when set)");
            console.println("INSPECT:  stat <path>  (full on-disk detail), xd <path> [off] [len]  (bounded hexdump)");
            #[cfg(target_arch = "aarch64")]
            console.println("UNAFS:    uls [path], ucat <path>  (native unafs volume, absolute paths)");
            #[cfg(target_arch = "aarch64")]
            console.println("          utouch <path>, uwrite <path> <text>, umkdir <path>, urm <path>  (write-through)");
            console.println("          usnaps, usnap <name>, usnapdrop <gen>  (retained roots / snapshots)");
            #[cfg(target_arch = "aarch64")]
            console.println("          usnapls <gen> [path], usnapcat <gen> <path>  (read a snapshot; current-ACL enforced)");
            console.println("CLOCK:    date, setdate YYYY-MM-DD HH:MM[:SS]  (seeds mtime stamps; unset = honest dashes)");
            console.println("          time  (shared civil clock: ISO-8601 UTC + source; unsynced until SNTP/setdate)");
            console.println("SMP:      sched (per-CPU run queues), pulse (full-screen CPU monitor)");
            #[cfg(target_arch = "aarch64")]
            console.println("          top  (per-core load: recent busy%, ctx-switches, last task)");
            // PI-APP-1: v3d-gated so the knob-off build's help text stays byte-identical to baseline.
            #[cfg(all(target_arch = "aarch64", feature = "v3d"))]
            console.println("APPS:     vug (3D sculptor), v3d (replay the visible GPU graphics battery)");
            console.println("POWER:    batmon (SMC battery snapshot; x86 UNAOS_SMC=1 only)");
            console.println("WITNESS:  bootlog (boot-milestone ring: PORTSW / FTDI console / EHCI HID / block / GUI handoff)");
            console.println("TEST:     tste (in-OS self-test suite: boot-replay + live checks)");
            console.println("NETWORK:  netinfo, ping <ip> [count], arp <ip>");
            console.println("          connect <ip> <port> [message], udpsend <ip> <port> [message]");
            console.println("          get <ip> [port] [path]  (HTTP/1.0 GET)");
        },
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
        "echo" => {
            let content = args.join(" ");
            console.println(&content);
        },
        "panic" => {
            // Test the Exception Handler
            panic!("Manual Panic Requested by Architect!");
        },
        "gneiss" => {
             console.println("Gneiss is Home.");
        },
        "usbinfo" => {
            for line in crate::drivers::xhci::usb_summary() {
                console.println(&line);
            }
        },
        "fatinfo" => {
            match crate::fs::fat::mount() {
                Ok(fs) => console.println(&fs.describe()),
                Err(e) => console.println(&alloc::format!("fatinfo: no FAT filesystem ({:?})", e)),
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
            match crate::fs::fat::mount() {
                Ok(fs) => match resolve_path(&fs, &path) {
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
                },
                Err(e) => console.println(&alloc::format!("cd: no FAT filesystem ({:?})", e)),
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
                Some(name) => match crate::fs::fat::mount() {
                    Ok(fs) => match resolve_path(&fs, &normalize_path(&cwd_path(), name)) {
                        Ok(Resolved::Root) =>
                            console.println("cat: /: is a directory (-EISDIR)"),
                        Ok(Resolved::Entry(de, canon)) => cat_render(console, &fs, &de, &canon),
                        Err(msg) => console.println(&alloc::format!("cat: {}", msg)),
                    },
                    Err(e) => console.println(&alloc::format!("cat: no FAT filesystem ({:?})", e)),
                },
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
        #[cfg(feature = "baremetal")]
        "run" => {
            // EXEC-1: load an ELF64 EL0 program off the VFS namespace and execute it at EL0, reporting its
            // exit status. Rides the SAME `MountTable` the `vfs` verb uses (`/fat` = FAT boot partition,
            // `/usb` = USB stick, `/` = native UnaFS), so `run /fat/ELFHELLO.ELF` loads the boot-partition
            // fixture. The bytes are read here (EL1/ASID 0) and handed to the kernel loader
            // (`run_user_image`), which maps them into a fresh per-task slot with per-segment W^X pages and
            // runs them under EL0 + the fault-kill net. `run <path>`.
            match args.first() {
                None => console.println("usage: run <path>   (load + execute an ELF64 EL0 program)"),
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
                                fs5_say(console, &alloc::format!(
                                    "USB FAT: {}  label {}  volume {} MiB  mounted /usb (read-only)",
                                    kind, label, vol_mib));
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
        "" => {}, // Ignore empty enter
        _ => {
            console.println("Unknown command. Type 'help' for assistance.");
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
/// and is **read-only by construction** (PIUSB-27): a `vfs write|append|rm|mkdir`
/// at `/usb` returns `-ENOTSUP` (the FatBackend refuses writes on a non-Default
/// source before touching the block layer). Rebuilt per invocation, so a stick
/// hot-plugged (or ejected) between commands is picked up on the next `vfs`.
/// EXEC-1: `run <path>` — load an ELF64 (or flat) EL0 program off the VFS namespace and execute it at EL0,
/// reporting its exit status. Reads the whole file through the same `MountTable` the `vfs` verb uses,
/// bounds it to the kernel's 16 KiB user window (an oversize file is rejected with a clear message — never
/// silently truncated), pre-checks the ELF64 magic + aarch64 machine for an early operator-friendly reason,
/// then hands the bytes to the kernel loader `run_user_image`, which maps them into a fresh per-task slot
/// (per-segment W^X pages) and runs them under EL0 + the fault-kill net. The kernel is the security
/// authority: this pre-check only sharpens the error text; `run_user_image` re-validates from scratch.
///
/// Witness (headless-capturable): `:: EXEC: run <path> — loaded <n> bytes, entry 0x<..>, exit=<code> ::`.
#[cfg(feature = "baremetal")]
fn run_program(console: &mut Console, path: &str) {
    use crate::fs::vfs::NodeKind;
    // Cap = the kernel user window; a file at or under it may still be rejected by the loader (a flat blob
    // is re-bounded to one code page), but this is the hard read ceiling — we never read past it.
    const CAP: u64 = crate::arch::aarch64::boot::USER_REGION_SIZE as u64;
    let mt = vfs_mount_table();
    let st = match mt.stat(path) {
        Ok(s) => s,
        Err(e) => {
            console.println(&alloc::format!("run: {}: {}", path, vfs_err(e)));
            return;
        }
    };
    if matches!(st.kind, NodeKind::Dir) {
        console.println(&alloc::format!("run: {}: is a directory (-EISDIR)", path));
        return;
    }
    if st.size == 0 {
        console.println(&alloc::format!("run: {}: empty file", path));
        return;
    }
    if st.size > CAP {
        console.println(&alloc::format!(
            "run: {}: {} bytes exceeds the {}-byte EL0 user window (-E2BIG)",
            path, st.size, CAP
        ));
        return;
    }
    let bytes = match mt.read(path, 0, st.size as usize) {
        Ok(b) => b,
        Err(e) => {
            console.println(&alloc::format!("run: {}: {}", path, vfs_err(e)));
            return;
        }
    };
    // Early ELF64/aarch64 pre-check for a friendly reason (the kernel loader is the real gate). A flat blob
    // (no ELF magic) is allowed through — the loader routes it to the position-independent flat path.
    if bytes.len() >= 20 && bytes[0..4] == [0x7F, b'E', b'L', b'F'] {
        if bytes[4] != 2 {
            console.println(&alloc::format!("run: {}: not an ELF64 image (EI_CLASS != 2)", path));
            return;
        }
        if bytes[5] != 1 {
            console.println(&alloc::format!("run: {}: not little-endian (EI_DATA != 1)", path));
            return;
        }
        let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
        if machine != 183 {
            console.println(&alloc::format!(
                "run: {}: not an aarch64 image (e_machine {} != 183)", path, machine
            ));
            return;
        }
    }
    // Hand the bytes to the kernel loader: map into a fresh EL0 slot, run co-located, wait (bounded 5 s) for
    // the program to exit or fault. The image length + entry are reported for the witness.
    let n = bytes.len();
    let deadline = 5 * crate::arch::aarch64::timer::cntfrq();
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

/// SHELL-WRITE dispatcher: `vfs <write|append|rm|mkdir> <path> [text ...]`.
#[cfg(target_arch = "aarch64")]
fn vfs_cmd(console: &mut Console, args: &[&str]) {
    use crate::fs::vfs::{NodeKind, VfsError, KERNEL_PRINCIPAL};
    let op = match args.first() {
        Some(&o) => o,
        None => {
            console.println("usage: vfs <write|append|rm|mkdir> <path> [text ...]");
            console.println("  namespace: / = native UnaFS, /fat = FAT boot partition, /usb = USB stick (read-only)");
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
