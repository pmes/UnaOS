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
/// an absent name is `-ENOENT`. Rides the dir-aware locate_in_dir twin.
fn fs_rm(console: &mut Console, arg: &str) {
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("rm: no FAT filesystem ({:?})", e)),
    };
    let (parent, name, canon) = match resolve_write_target(&fs, arg) {
        Ok(t) => t,
        Err(msg) => return console.println(&alloc::format!("rm: {}", msg)),
    };
    match fs.locate_in_dir(parent, &name) {
        Ok((de, dl, doff)) => {
            if de.is_dir {
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
        Err(FatError::NotFound) => console.println(&alloc::format!(
            "rm: {}: not found (-ENOENT)", joined(&canon, &name))),
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
/// names the destination file (created, or truncated in place if it already exists). Honest errors:
/// src missing → `-ENOENT`; src is a dir → `-EISDIR`; dst parent missing → `-ENOENT`; dst parent is a
/// file → `-ENOTDIR`; the volume/dir full → `-ENOSPC`; copying a file onto itself (same canonical
/// path) → `-EINVAL`.
///
/// SIZE HANDLING (JD8-M2 decision): the copy STREAMS the bytes in fixed windows via the offset-aware
/// `read_at` (existing public fat.rs API — the U9/read-path twin of `read_file`, so this reaches for
/// no NEW primitive) feeding `write_grow`, so a file of ANY size copies with a bounded
/// (`CP_WINDOW`-byte) heap footprint and NO truncation. There is deliberately no size ceiling. The
/// per-window `write_grow` re-walks the growing destination chain (bounded, and every FAT/data access
/// rides the JD3 wall-clock BOT pump — a stalled transfer is `-EIO`, never a hang on the timerless EL1
/// core); a future single-pass primitive could tighten that, tracked as a JD9 note.
fn fs_cp(console: &mut Console, src: &str, dst: &str) {
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
    match copy_file_into(&fs, &de_src, &src_canon, dparent, &dleaf, &dcanon) {
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
) -> Result<u64, String> {
    let dest_disp = joined(dcanon, dleaf);
    // --- Create-or-truncate the destination as a fresh 0-length file (the JD6 write prologue). ---
    let (dir_lba, dir_off) = match fs.locate_in_dir(dparent, dleaf) {
        Ok((de, dl, doff)) => {
            if de.is_dir {
                return Err(alloc::format!("{}: is a directory (-EISDIR)", joined(dcanon, de.name())));
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
            let bytes = copy_file_into(fs, de, &child_src, dst_cluster, nm, dst_canon)?;
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
///   * the top-level target must NOT already exist (`-EEXIST`). This is the simple, safe rule: `cp -r`
///     always creates a FRESH tree, never silently merging into or overwriting an existing one. Because the
///     top-level target is fresh, every directory `cp_tree` creates below it is inside freshly-created (so
///     empty) parents — no child can collide, and no existing file is ever clobbered.
/// A mid-tree failure stops and reports the honest partial count (dirs/files/bytes copied so far) plus the
/// failing path + errno; nothing is rolled back (a partial tree is left on disk, crash-safe per the
/// FATDIRS/JD6 ordering — the operator can `rmdir`/`rm` it).
fn fs_cp_recursive(console: &mut Console, src: &str, dst: &str) {
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
        return fs_cp(console, src, dst); // `cp -r FILE DST` == `cp FILE DST`
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
    // --- The top-level target must not already exist (fresh-tree rule → honest -EEXIST). ---
    if resolve_path(&fs, &target).is_ok() {
        return console.println(&alloc::format!("cp: {}: already exists (-EEXIST)", target));
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
/// canonical name (same parent + same leaf), which the seam treats as a no-op success. Honest errno
/// surface: src missing → `-ENOENT`; root as src → `-EBUSY`; dst parent missing → `-ENOENT`; dst
/// parent is a file → `-ENOTDIR`; dst dir full → `-ENOSPC`; a non-8.3 dst name → `-EINVAL`; a
/// directory across parents → `-EISDIR`.
///
/// ACL NOTE: this shell is EL1 ASID 0 = the PUBLIC principal, so a panel `mv` consults no U6/K-line
/// `OWNED_FILES` ACL and is ACL-neutral by construction (the row re-key for a moved EL0-owned file is
/// a future K-line seam, ledgered in the pi4 FATMOVE SECURITY note). CRASH SAFETY is the seam's job:
/// `move_entry` publishes the destination BEFORE `0xE5`ing the source, so a power-cut mid-move leaves
/// a benign duplicate (two names, one chain), never a lost chain.
fn fs_mv(console: &mut Console, src: &str, dst: &str) {
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
    // --- Dest pre-check (no-clobber). Skip it only when the destination IS the source (same parent +
    //     same canonical leaf) — a rename to the same name, which `rename_entry` treats as a no-op. ---
    let same_target = src_parent == dparent && dleaf.eq_ignore_ascii_case(src_leaf);
    if !same_target {
        match fs.locate_in_dir(dparent, &dleaf) {
            Ok((de, _, _)) => return console.println(&alloc::format!(
                "mv: {}: file exists (-EEXIST)", joined(&dcanon, de.name()))),
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

/// Print one directory's entries in the `ls` table format, with the file/dir tally.
fn print_dir_listing(console: &mut Console, entries: &[DirEntry]) {
    let (mut files, mut dirs) = (0u32, 0u32);
    for de in entries {
        if de.is_dir {
            dirs += 1;
            console.println(&alloc::format!("  <DIR>         {}", de.name()));
        } else {
            files += 1;
            console.println(&alloc::format!("  {:>10}  {}", de.size, de.name()));
        }
    }
    console.println(&alloc::format!("{} file(s), {} dir(s)", files, dirs));
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
fn ls_resolved(console: &mut Console, fs: &FatFs, path: &str) {
    match resolve_path(fs, path) {
        Ok(Resolved::Root) => match fs.read_dir(0) {
            Ok(entries) => print_dir_listing(console, &entries),
            Err(e) => console.println(&alloc::format!("ls: /: read failed ({:?}, -EIO)", e)),
        },
        Ok(Resolved::Entry(de, canon)) => {
            if de.is_dir {
                match fs.read_dir(de.first_cluster()) {
                    Ok(entries) => print_dir_listing(console, &entries),
                    Err(e) => console.println(&alloc::format!(
                        "ls: {}: read failed ({:?}, -EIO)", canon, e)),
                }
            } else {
                console.println(&alloc::format!("  {:>10}  {}", de.size, de.name()));
            }
        }
        Err(msg) => console.println(&alloc::format!("ls: {}", msg)),
    }
}

/// JD4 `ls`/`ls <dir>` (single path): mount + resolve + print. Extracted so the wildcard `ls *.EXT`
/// can share the exact resolve/print behaviour for a non-trailing-glob fall-through.
fn ls_path(console: &mut Console, arg: &str) {
    match crate::fs::fat::mount() {
        Ok(fs) => ls_resolved(console, &fs, &normalize_path(&cwd_path(), arg)),
        Err(e) => console.println(&alloc::format!("ls: no FAT filesystem ({:?})", e)),
    }
}

/// JD12 `ls *.EXT`: list every entry matching a wildcard, one `ls`-table line each (sorted), with the
/// file/dir tally. A directory match shows as `<DIR>` (its contents are not expanded — that mirrors
/// how a shell hands matched names to `ls`); no match is an honest "no match".
fn ls_globbed(console: &mut Console, arg: &str) {
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("ls: no FAT filesystem ({:?})", e)),
    };
    match glob_expand(&fs, arg) {
        Glob::Literal(p) => ls_resolved(console, &fs, &normalize_path(&cwd_path(), &p)),
        Glob::Matched { entries, .. } if entries.is_empty() =>
            console.println(&alloc::format!("ls: {}: no match", arg)),
        Glob::Matched { entries, .. } => print_dir_listing(console, &entries),
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

/// JD12 `rm` with wildcards: `rm <path...>` — each arg a file target or a trailing glob.
/// SNAPSHOT-then-delete (`glob_expand` captures the match list before any delete), so a wildcard
/// delete never invalidates its own list. A wildcard with no match is an honest per-pattern note;
/// each concrete target rides the existing per-file `fs_rm` (directory → `-EISDIR`, use `rmdir`).
fn rm_globbed(console: &mut Console, args: &[&str]) {
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(e) => return console.println(&alloc::format!("rm: no FAT filesystem ({:?})", e)),
    };
    for a in args {
        match glob_expand(&fs, a) {
            Glob::Literal(p) => fs_rm(console, &p),
            Glob::Matched { entries, .. } if entries.is_empty() =>
                console.println(&alloc::format!("rm: {}: no match", a)),
            Glob::Matched { parent_canon, entries } => {
                for de in &entries {
                    fs_rm(console, &joined(&parent_canon, de.name()));
                }
            }
        }
    }
}

/// JD12 `cp` with wildcards / multiple sources: `cp [-r] <src...> <dst>`. Sources expand (globs +
/// literals); with more than one source the destination MUST be an existing directory (several files
/// can only land INTO a directory). Each source rides the existing `fs_cp` / `fs_cp_recursive` (the
/// `FILE DIR/` idiom lands each under `dst/<leaf>`). SNAPSHOT-then-copy.
fn cp_globbed(console: &mut Console, sources: &[&str], dst: &str, recursive: bool) {
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
            fs_cp_recursive(console, s, dst);
        } else {
            fs_cp(console, s, dst);
        }
    }
}

/// JD12 `mv` with wildcards / multiple sources: `mv <src...> <dst>`. Sources expand; with more than
/// one source the destination MUST be an existing directory. Each source rides the existing `fs_mv`
/// (the `SRC DIR/` idiom lands each under `dst/<leaf>`). SNAPSHOT-then-move — a wildcard move never
/// invalidates its own list.
fn mv_globbed(console: &mut Console, sources: &[&str], dst: &str) {
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
        fs_mv(console, s, dst);
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
    // console visible.
    let took_screen = command == "vug" || command == "pulse";

    match command {
        "ver" | "version" => {
            console.println("unaOS v0.1.0 (Kernel: Jules 1 / Cortex: Jules 6)");
        },
        "help" => {
            console.println("COMMANDS: ver, help, clear, echo, panic, gneiss");
            console.println("STORAGE:  diskinfo, usbinfo, read <lba>, write <lba> <byte>");
            console.println("FILES:    fatinfo (FAT geometry), ls [dir], cd [dir], pwd, cat <path>");
            console.println("PAGING:   head <path> [n], tail <path> [n]  (first / last n lines, default 10)");
            console.println("WRITE:    touch <path>, write <path> <text>, append <path> <text>, rm <path>");
            console.println("DIRS:     mkdir <path>, rmdir <path>  (create / remove empty directories)");
            console.println("COPY:     cp <src> <dst>  (copy a file; cp FILE DIR/ lands as DIR/<leaf>)");
            console.println("          cp -r <srcdir> <dst>  (recursively copy a directory tree)");
            console.println("MOVE:     mv <src...> <dst>  (move/rename a file or dir, O(1); mv SRC DIR/ lands as DIR/<leaf>)");
            console.println("WILDCARD: * / ? in the last path component — ls/cat/rm/cp/mv expand it (e.g. rm *.TMP, cp *.TXT DOCS/)");
            console.println("          (create/edit/delete/copy/move files & dirs anywhere in the tree; sync = write-through, durable)");
            #[cfg(target_arch = "aarch64")]
            console.println("UNAFS:    uls [path], ucat <path>  (native unafs volume, absolute paths)");
            #[cfg(target_arch = "aarch64")]
            console.println("          utouch <path>, uwrite <path> <text>, umkdir <path>, urm <path>  (write-through)");
            console.println("SMP:      sched (per-CPU run queues), pulse (full-screen CPU monitor)");
            console.println("TEST:     tste (in-OS self-test suite: boot-replay + live checks)");
            console.println("NETWORK:  netinfo, ping <ip> [count], arp <ip>");
            console.println("          connect <ip> <port> [message], udpsend <ip> <port> [message]");
            console.println("          get <ip> [port] [path]  (HTTP/1.0 GET)");
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
            // lists the wildcard matches (sorted, with the file/dir tally).
            match args.first().copied() {
                Some(a) if has_glob(a) => ls_globbed(console, a),
                other => ls_path(console, other.unwrap_or(".")),
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
            // JD6: delete a file in any reachable dir (directory → -EISDIR; use `rmdir`). JD12: takes
            // multiple targets and expands trailing wildcards — `rm <path...>`, `rm *.TMP`.
            if args.is_empty() {
                console.println("usage: rm <path> [path ...]");
            } else if !args.iter().any(|a| has_glob(a)) {
                // No wildcard: delete each literal target (a single arg is byte-identical to pre-JD12).
                for &a in &args {
                    fs_rm(console, a);
                }
            } else {
                rm_globbed(console, &args);
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
        "cp" | "copy" => {
            // JD8: copy a file (`cp FILE DIR/` lands as DIR/<leaf>). JD9: `-r`/`-R` recursively copies a
            // directory tree. Flags (leading `-`) precede the paths; only `-r`/`-R` are recognized (any
            // other leading-`-` arg is ignored as an unknown flag — a name literally beginning with `-` is
            // reachable as `./-name`). JD12: sources expand trailing wildcards and there may be several,
            // the LAST path being the destination (into a directory). `cp [-r] <src...> <dst>`.
            let recursive = args.iter().any(|a| *a == "-r" || *a == "-R");
            let paths: Vec<&str> = args.iter().copied().filter(|a| !a.starts_with('-')).collect();
            if paths.len() < 2 {
                console.println("usage: cp [-r] <src...> <dst>");
            } else if paths.len() == 2 && !has_glob(paths[0]) && !has_glob(paths[1]) {
                // No wildcard, one src: byte-identical to pre-JD12 cp.
                if recursive {
                    fs_cp_recursive(console, paths[0], paths[1]);
                } else {
                    fs_cp(console, paths[0], paths[1]);
                }
            } else {
                let dst = paths[paths.len() - 1];
                cp_globbed(console, &paths[..paths.len() - 1], dst, recursive);
            }
        },
        "mv" | "move" | "ren" | "rename" => {
            // JD10: move OR rename a file or directory by relinking one directory entry (O(1), by
            // reference — no data copy, so `mv DIR NEWNAME` needs no `-r`). Same parent → rename in
            // place (files AND dirs); across parents → move (files only, a dir there is -EISDIR). The
            // `mv SRC DIR/` idiom lands the entry under DIR as the source leaf. JD12: sources expand
            // trailing wildcards and there may be several, the LAST path being the destination
            // directory. `mv <src...> <dst>`.
            if args.len() < 2 {
                console.println("usage: mv <src...> <dst>");
            } else if args.len() == 2 && !has_glob(args[0]) && !has_glob(args[1]) {
                // No wildcard, one src: byte-identical to pre-JD12 mv.
                fs_mv(console, args[0], args[1]);
            } else {
                let dst = args[args.len() - 1];
                mv_globbed(console, &args[..args.len() - 1], dst);
            }
        },
        "sync" => {
            // JD5-M3: storage is WRITE-THROUGH — block::write_block issues a synchronous BOT WRITE(10)
            // (USB) / polled CMD24 (SD) that completes before the command returns, so there is no
            // write-back cache to flush. `sync` is the honest confirmation of that (a no-op by design).
            console.println("sync: write-through storage — every write is already durable on the card");
        },
        "diskinfo" => {
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
