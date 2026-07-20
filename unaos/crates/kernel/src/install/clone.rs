// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// INSTALL — the buffered self-clone payload primitive (the installer engine's payload seam for a
// same-device clone).
//
// The Pi self-clone (INSTALL-PI-2) reproduces the running system's own boot media onto the target card.
// Unlike the Orin flow (installer_engine.md §INSTALL-2), where the clone SOURCE (the USB boot stick) is a
// DIFFERENT block device from the target (the microSD) and the copy can stream read-source→write-target,
// the Pi has ONE block device: the seated `emmc2` card is BOTH the boot media it reads its source from
// AND the install target it writes onto. A streaming copy (the Orin `copy_dir`) would read the source
// AFTER the GPT write had already destroyed it. So the same-device clone is TWO PHASES:
//   1. SNAPSHOT — read the WHOLE source boot tree into memory (bounded), through the in-tree FAT reader
//      (`fs::fat`), BEFORE any destructive write.
//   2. WRITE — lay the fresh GPT + FAT32 (the caller does that), then mirror the buffered tree onto the
//      freshly-formatted ESP via the engine's `TreeWriter`, recording each file's extents so the caller
//      sha-content-verifies every cloned file off the card.
//
// Both phases are engine-level and target-agnostic (any `InstallTarget`, any `fs::fat` source), so this
// is the engine's payload seam — NOT a Pi-only fork. The Orin's streaming clone could adopt the buffered
// path later without changing this module; two targets, one engine.
//
// SAFETY / BOUNDS. The snapshot materializes the source tree in the kernel heap, so it is bounded on
// every axis a malformed or hostile source could exploit: per-file size, TOTAL bytes (kept well under the
// heap), and recursion depth. A `.`/`..` self/parent entry is skipped (it is structure, not payload). A
// short read (chain ended before the directory-recorded size) is a malformed source — refused, never
// cloned partially.

use super::fat32::{
    dir_clusters_for_slots, put_dir_entry, Extent, FatGeom, TreeWriter, ATTR_ARCHIVE, ATTR_DIR,
    DIR_SLOTS_PER_CLUSTER,
};
use super::{hash, InstallError, InstallTarget};
use crate::fs::fat::FatFs;
use alloc::string::String;
use alloc::vec::Vec;

/// The boot ESP tree is shallow (root → EFI → BOOT); refuse a pathological or looping source rather than
/// exhaust the kernel stack.
const MAX_DEPTH: u32 = 8;
/// Per-file cap — the Pi boot payload's largest file (the GPU firmware / the kernel image) is a few MB;
/// refuse an implausibly large file rather than exhaust the heap reading it whole.
const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;
/// Total-tree cap — the whole snapshot is buffered before the destructive write, so keep it well under
/// the kernel heap. A boot tree is a handful of MB; 40 MiB is a generous ceiling.
const MAX_TOTAL_BYTES: usize = 40 * 1024 * 1024;

/// One file materialized off the source: its short (8.3) name and its whole content.
pub struct SnapFile {
    pub name: String,
    pub data: Vec<u8>,
}

/// One directory materialized off the source: its files and its subdirectories (each a `SnapDir`). The
/// `.`/`..` entries are NOT stored — they are re-emitted by the writer from the actual cluster layout.
pub struct SnapDir {
    pub files: Vec<SnapFile>,
    pub subdirs: Vec<(String, SnapDir)>,
}

/// A whole source boot tree read into memory before any destructive write.
pub struct SnapTree {
    pub root: SnapDir,
    pub file_count: usize,
    pub total_bytes: usize,
}

/// A cloned file's verification record: its path on the boot tree, content SHA-256, the exact device
/// extents the writer recorded (what `verify_extents` re-reads off the card), and its size.
pub struct FileRec {
    pub path: String,
    pub sha: [u8; 32],
    pub extents: Vec<Extent>,
    pub size: usize,
}

/// PHASE 1 — read the whole source boot tree into memory through the in-tree FAT reader, BEFORE any
/// destructive write. `src` is a mounted source volume (on the Pi, the seated card's own FAT boot
/// partition). Every file is materialized whole; bounds guard the heap. Returns the buffered tree.
pub fn snapshot(src: &FatFs) -> Result<SnapTree, InstallError> {
    let root_entries = src.read_root().map_err(|_| InstallError::Io)?;
    let mut file_count = 0usize;
    let mut total_bytes = 0usize;
    let root = snap_dir(src, &root_entries, 0, &mut file_count, &mut total_bytes)?;
    Ok(SnapTree { root, file_count, total_bytes })
}

fn snap_dir(
    src: &FatFs,
    entries: &[crate::fs::fat::DirEntry],
    depth: u32,
    file_count: &mut usize,
    total_bytes: &mut usize,
) -> Result<SnapDir, InstallError> {
    if depth > MAX_DEPTH {
        return Err(InstallError::BadArg);
    }
    let mut dir = SnapDir { files: Vec::new(), subdirs: Vec::new() };
    for e in entries {
        let name = e.name();
        if name == "." || name == ".." {
            continue;
        }
        if e.is_dir {
            let child_entries = src.read_dir(e.first_cluster()).map_err(|_| InstallError::Io)?;
            let sub = snap_dir(src, &child_entries, depth + 1, file_count, total_bytes)?;
            dir.subdirs.push((String::from(name), sub));
        } else {
            let size = e.size as usize;
            if size > MAX_FILE_BYTES {
                return Err(InstallError::BadArg);
            }
            if total_bytes.saturating_add(size) > MAX_TOTAL_BYTES {
                return Err(InstallError::NoSpace);
            }
            let mut data: Vec<u8> = Vec::new();
            src.read_file(e, &mut data, size).map_err(|_| InstallError::Io)?;
            if data.len() != size {
                // Short read (chain ended before the recorded size) — a malformed source; do not clone it.
                return Err(InstallError::Io);
            }
            *file_count += 1;
            *total_bytes += size;
            dir.files.push(SnapFile { name: String::from(name), data });
        }
    }
    Ok(dir)
}

/// PHASE 2 — mirror the buffered tree onto a freshly-`format_esp`'d ESP via the engine's `TreeWriter`.
/// The caller has already written the GPT, zeroed the ESP metadata, and formatted the ESP. Returns a
/// `FileRec` per cloned file (its extents + content SHA) for the caller's sha-content-verify. The root
/// directory is sized to its entry count and reserved BEFORE any file/subdir allocation, so its cluster
/// chain stays contiguous.
pub fn write_snapshot<T: InstallTarget>(
    t: &mut T,
    geom: FatGeom,
    tree: &SnapTree,
) -> Result<Vec<FileRec>, InstallError> {
    let mut recs: Vec<FileRec> = Vec::new();
    let mut w = TreeWriter::new(t, geom);
    let root_slots = tree.root.files.len() + tree.root.subdirs.len();
    let root_clusters = dir_clusters_for_slots(root_slots.max(1));
    let root_cluster = w.reserve_root(root_clusters)?;
    write_dir(&mut w, &tree.root, root_cluster, 0, true, "", &mut recs, 0, root_clusters)?;
    Ok(recs)
}

#[allow(clippy::too_many_arguments)]
fn write_dir<T: InstallTarget>(
    w: &mut TreeWriter<'_, T>,
    dir: &SnapDir,
    this_cluster: u32,
    parent_cluster: u32,
    is_root: bool,
    path_prefix: &str,
    recs: &mut Vec<FileRec>,
    depth: u32,
    this_clusters: u32,
) -> Result<(), InstallError> {
    if depth > MAX_DEPTH {
        return Err(InstallError::BadArg);
    }
    // The directory image is built WHOLLY in memory across its whole cluster chain, then written once, so
    // a possibly-stale data cluster on a non-blank card never leaks bytes into a directory.
    let capacity = this_clusters as usize * DIR_SLOTS_PER_CLUSTER;
    let mut image = alloc::vec![0u8; this_clusters as usize * 512];
    let mut slot = 0usize;
    if !is_root {
        if !put_dir_entry(&mut image, slot, ".", ATTR_DIR, this_cluster, 0)
            || !put_dir_entry(&mut image, slot + 1, "..", ATTR_DIR, parent_cluster, 0)
        {
            return Err(InstallError::BadArg);
        }
        slot += 2;
    }

    // Subdirectories first: recurse (children must exist to know their first cluster) before writing the
    // parent entry. Each child is sized from its own `.`/`..` + entry count and given its own chain.
    for (name, sub) in &dir.subdirs {
        if slot >= capacity {
            return Err(InstallError::NoSpace);
        }
        let child_slots = 2 + sub.files.len() + sub.subdirs.len();
        let child_clusters = dir_clusters_for_slots(child_slots);
        let child = w.alloc_dir_clusters(child_clusters)?;
        let child_prefix = alloc::format!("{}{}/", path_prefix, name);
        let child_dotdot = if is_root { 0 } else { this_cluster };
        write_dir(w, sub, child, child_dotdot, false, &child_prefix, recs, depth + 1, child_clusters)?;
        if !put_dir_entry(&mut image, slot, name, ATTR_DIR, child, 0) {
            return Err(InstallError::BadArg);
        }
        slot += 1;
    }

    // Files: allocate + write each whole (from the buffered bytes), record its extents for the caller's
    // sha-content-verify.
    for f in &dir.files {
        if slot >= capacity {
            return Err(InstallError::NoSpace);
        }
        let sha = hash::sha256(&f.data);
        let (first, extents) = w.write_file(&f.data)?;
        if !put_dir_entry(&mut image, slot, &f.name, ATTR_ARCHIVE, first, f.data.len() as u32) {
            return Err(InstallError::BadArg);
        }
        slot += 1;
        recs.push(FileRec {
            path: alloc::format!("{}{}", path_prefix, f.name),
            sha,
            extents,
            size: f.data.len(),
        });
    }

    w.write_dir_image(this_cluster, &image)?;
    Ok(())
}

/// Lower-hex a 32-byte digest for the per-file clone manifest line.
pub fn sha_hex(d: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in d {
        s.push(core::char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(core::char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}
