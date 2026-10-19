// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Geode — CODEX §2's **Archives** (the Vault). GEODEMETA (LEDGER SR148): a `.geode` archive carries UnaFS
//! records whole, so UnaFS → `.geode` → UnaFS gives back every entry's kind, size, bytes, attribute set (owner,
//! mode, every `una:*` key, small and spilled alike) and its three §R7 times, byte for byte. Extracting onto a
//! volume that keeps no UnaFS records (a FAT stick, any host directory) says ONCE, before the first write, what
//! the target will not keep.
//!
//! # The format, `.geode` v1 (all integers little-endian, the §R1 codec of
//! `docs/dev/OS/09_FILESYSTEM/unafs-records.md`)
//!
//! ```text
//! header   "GEODEAR1" (8) ‖ version u32 (= 1) ‖ entries u64
//! entry    path   String          relative, '/'-joined; no empty, "." or ".." component
//!          kind   FileKind §R4    0 File · 1 Directory (2 Symlink, 3 System: refused)
//!          ctime u64 ‖ mtime u64 ‖ atime u64      the §R7 meta trailer's three, unix seconds
//!          attrs  Map<String, AttributeValue §R5>  the inode's `attributes` ∪ its resolved
//!                                                   `large_attributes`, keys ascending
//!          data   len u64 ‖ bytes                   a File's contents; a Directory's is empty
//! trailer  "GEODEEND" (8) ‖ FNV-1a 64 (unafs::hash) of every byte before the trailer
//! ```
//!
//! Entries are in pre-order (a directory before what it holds, siblings in name order, the UnaFS listing's
//! order), so an unpack can create each entry under a parent it has already made. What a record holds that
//! is a LOCATION on one volume — the inode id, the parent id, the extents and the spilled values' extents —
//! is not carried: the target volume allocates its own, and the small/large split is re-decided by the
//! same `set_attribute` rule, so it lands the same. An archive is parsed and checked whole (checksum, every
//! path, every parent) before either unpack writes anything.
//!
//! The encoder and decoder are `geode_core`'s (GEODEMOD SR157, `unaos/libs/fs/geode_core`): ONE codec, which the
//! kernel's backup (`unaos/crates/kernel/src/fs/geodew.rs`) compiles from the same file.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use unafs::fs::FileSystemError;
use unafs::{BlockDevice, FileKind, UnaFS};

// GEODEMOD (LEDGER SR157): the format's ONE encoder and decoder is `geode_core` (no_std), which the kernel's
// `fs/geodew.rs` compiles too; this crate keeps what needs std and a volume (pack, unpack, plain extraction).
pub use geode_core::{END, Entry, MAGIC, VERSION, leaf, parent_of, write_archive};

/// Deepest directory nesting a pack walks before calling the volume corrupt (unafs's own path bound).
pub const MAX_DEPTH: usize = unafs::fs::MAX_PATH_DEPTH;

/// What can go wrong.
#[derive(Debug)]
pub enum GeodeError {
    /// The volume refused.
    Fs(FileSystemError),
    /// The archive is not a valid `.geode` v1 (the reason names the field or the check).
    Format(String),
    /// A record kind the format does not carry (symlink, system object), named by path.
    Unsupported { path: String, kind: FileKind },
    /// The target already holds this name; nothing was written.
    Exists(String),
    /// The host filesystem refused (plain extraction).
    Io(std::io::Error),
}

impl fmt::Display for GeodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeodeError::Fs(e) => write!(f, "geode: unafs: {e}"),
            GeodeError::Format(why) => write!(f, "geode: not a .geode v1 archive: {why}"),
            GeodeError::Unsupported { path, kind } => write!(f, "geode: {path}: a {kind:?} is not archived"),
            GeodeError::Exists(p) => write!(f, "geode: {p} already exists in the target; nothing written"),
            GeodeError::Io(e) => write!(f, "geode: {e}"),
        }
    }
}

impl std::error::Error for GeodeError {}

impl From<FileSystemError> for GeodeError {
    fn from(e: FileSystemError) -> Self {
        GeodeError::Fs(e)
    }
}

impl From<std::io::Error> for GeodeError {
    fn from(e: std::io::Error) -> Self {
        GeodeError::Io(e)
    }
}

fn bad(why: impl Into<String>) -> GeodeError {
    GeodeError::Format(why.into())
}

// ---------------------------------------------------------------------------
// Decode (the encoder is `geode_core::write_archive`, re-exported above)
// ---------------------------------------------------------------------------

/// Parse and check a whole archive: magic, version, checksum, every record, every path and its parent — the shared
/// core's decoder (`geode_core::read_archive`); its refusal word is the `Format` reason.
pub fn read_archive(bytes: &[u8]) -> Result<Vec<Entry>, GeodeError> {
    geode_core::read_archive(bytes).map_err(bad)
}

// ---------------------------------------------------------------------------
// UnaFS → entries → UnaFS
// ---------------------------------------------------------------------------

/// The entries under directory `dir` of a UnaFS volume, in archive (pre-)order: each inode's kind, times,
/// every attribute (spilled values read back whole) and, for a file, its bytes.
pub fn records<D: BlockDevice>(fs: &mut UnaFS<D>, dir: &str) -> Result<Vec<Entry>, GeodeError> {
    let top = fs.resolve_path(dir)?;
    if fs.read_inode(top)?.kind != FileKind::Directory {
        return Err(GeodeError::Fs(FileSystemError::NotADirectory));
    }
    let mut out = Vec::new();
    // (directory inode, its relative path, depth); children pushed in reverse so they pop in name order.
    let mut stack: Vec<(u64, String, usize)> = vec![(top, String::new(), 0)];
    while let Some((id, rel, depth)) = stack.pop() {
        if !rel.is_empty() {
            out.push(entry_of(fs, id, &rel)?);
        }
        if fs.read_inode(id)?.kind != FileKind::Directory {
            continue;
        }
        if depth >= MAX_DEPTH {
            return Err(GeodeError::Fs(FileSystemError::CorruptVolume("directory nesting past MAX_DEPTH")));
        }
        let children = fs.ls(id)?;
        for c in children.into_iter().rev() {
            let p = if rel.is_empty() { c.name.clone() } else { format!("{rel}/{}", c.name) };
            stack.push((c.inode_id, p, depth + 1));
        }
    }
    Ok(out)
}

fn entry_of<D: BlockDevice>(fs: &mut UnaFS<D>, id: u64, path: &str) -> Result<Entry, GeodeError> {
    let inode = fs.read_inode(id)?;
    if !matches!(inode.kind, FileKind::File | FileKind::Directory) {
        return Err(GeodeError::Unsupported { path: path.into(), kind: inode.kind });
    }
    let mut attributes = inode.attributes.clone();
    for key in inode.large_attributes.keys() {
        match fs.get_attribute(id, key)? {
            Some(v) => {
                attributes.insert(key.clone(), v);
            }
            None => return Err(GeodeError::Fs(FileSystemError::InvalidAttributeData)),
        }
    }
    let data = if inode.kind == FileKind::File && inode.size > 0 {
        fs.read_data(id, 0, inode.size)?
    } else {
        Vec::new()
    };
    Ok(Entry {
        path: path.into(),
        kind: inode.kind,
        ctime: inode.ctime,
        mtime: inode.mtime,
        atime: inode.atime,
        attributes,
        data,
    })
}

/// Pack directory `dir` of a UnaFS volume (what it holds, not the directory itself) into a `.geode` archive.
pub fn pack<D: BlockDevice>(fs: &mut UnaFS<D>, dir: &str) -> Result<Vec<u8>, GeodeError> {
    Ok(write_archive(&records(fs, dir)?))
}

/// What an unpack did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackReport {
    pub files: usize,
    pub dirs: usize,
    pub attributes: usize,
    pub bytes: u64,
}

/// Unpack an archive under directory `dir` of a UnaFS volume, keeping every record whole. The archive is
/// checked whole and every top-level name is checked free in `dir` before the first write. Each entry is
/// created, written and given its attributes in archive order; then the times are set from the last entry
/// back to the first (a child's create edits its parent directory, which the clock would otherwise stamp).
/// Not one transaction: a volume error mid-way leaves the entries made so far (each one committed whole).
pub fn unpack<D: BlockDevice>(fs: &mut UnaFS<D>, archive: &[u8], dir: &str) -> Result<UnpackReport, GeodeError> {
    let entries = read_archive(archive)?;
    let top = fs.resolve_path(dir)?;
    if fs.read_inode(top)?.kind != FileKind::Directory {
        return Err(GeodeError::Fs(FileSystemError::NotADirectory));
    }
    let present: BTreeSet<String> = fs.ls(top)?.into_iter().map(|e| e.name).collect();
    if let Some(e) = entries.iter().find(|e| !e.path.contains('/') && present.contains(&e.path)) {
        return Err(GeodeError::Exists(e.path.clone()));
    }
    let mut ids: BTreeMap<&str, u64> = BTreeMap::new();
    let mut made: Vec<(u64, &Entry)> = Vec::with_capacity(entries.len());
    let mut report = UnpackReport { files: 0, dirs: 0, attributes: 0, bytes: 0 };
    for e in &entries {
        let parent = match parent_of(&e.path) {
            "" => top,
            p => *ids.get(p).ok_or_else(|| bad(format!("{}: parent not made", e.path)))?,
        };
        let id = match e.kind {
            FileKind::Directory => {
                report.dirs += 1;
                fs.mkdir(parent, leaf(&e.path).into())?
            }
            _ => {
                report.files += 1;
                let id = fs.create_file(parent, leaf(&e.path).into())?;
                if !e.data.is_empty() {
                    fs.write_data(id, 0, &e.data)?;
                    report.bytes += e.data.len() as u64;
                }
                id
            }
        };
        for (k, v) in &e.attributes {
            fs.set_attribute(id, k.clone(), v.clone())?;
            report.attributes += 1;
        }
        ids.insert(e.path.as_str(), id);
        made.push((id, e));
    }
    for (id, e) in made.iter().rev() {
        fs.set_times(*id, e.ctime, e.mtime, e.atime)?;
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// Extraction onto a volume that keeps no UnaFS records (FAT, a host directory)
// ---------------------------------------------------------------------------

/// What a plain extraction did, and the one warning it gave before writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainReport {
    pub files: usize,
    pub dirs: usize,
    /// Attributes the target could not keep (every one in the archive).
    pub dropped_attributes: usize,
    /// The distinct attribute keys dropped, ascending.
    pub dropped_keys: Vec<String>,
    /// The warning, exactly as given.
    pub warning: String,
}

/// The one warning a plain extraction gives: what the target will not keep, said before anything is written.
pub fn drop_warning(entries: &[Entry], target: &Path) -> (String, usize, Vec<String>) {
    let keys: BTreeSet<&str> = entries.iter().flat_map(|e| e.attributes.keys().map(String::as_str)).collect();
    let n: usize = entries.iter().map(|e| e.attributes.len()).sum();
    let holders = entries.iter().filter(|e| !e.attributes.is_empty()).count();
    let list: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
    let warning = format!(
        "[geode] warn: {} keeps no UnaFS records; dropping attrs={n} on entries={holders}/{} keys={} and every ctime (kept: names, bytes, mtime, atime)",
        target.display(),
        entries.len(),
        if list.is_empty() { "none".to_string() } else { list.join(",") },
    );
    (warning, n, list)
}

/// Extract an archive into host directory `target` (a mounted FAT volume, or any directory), printing the one
/// drop warning to stderr before the first write.
pub fn unpack_plain(archive: &[u8], target: &Path) -> Result<PlainReport, GeodeError> {
    unpack_plain_with(archive, target, &mut |w| eprintln!("{w}"))
}

/// [`unpack_plain`] with the warning handed to `warn` (called exactly once, before any write). Names, bytes,
/// mtime and atime are kept; every attribute and the ctime are dropped. Every top-level name is checked free
/// first; nothing is overwritten (`create_new`).
pub fn unpack_plain_with(
    archive: &[u8],
    target: &Path,
    warn: &mut dyn FnMut(&str),
) -> Result<PlainReport, GeodeError> {
    let entries = read_archive(archive)?;
    if !target.is_dir() {
        return Err(GeodeError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} is not a directory", target.display()),
        )));
    }
    if let Some(e) = entries.iter().find(|e| !e.path.contains('/') && target.join(&e.path).exists()) {
        return Err(GeodeError::Exists(e.path.clone()));
    }
    let (warning, dropped_attributes, dropped_keys) = drop_warning(&entries, target);
    warn(&warning);
    let mut report = PlainReport { files: 0, dirs: 0, dropped_attributes, dropped_keys, warning };
    for e in &entries {
        let p = target.join(&e.path);
        if e.kind == FileKind::Directory {
            std::fs::create_dir(&p)?;
            report.dirs += 1;
        } else {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&p)?;
            f.write_all(&e.data)?;
            report.files += 1;
        }
    }
    // Times last, deepest first (a create in a directory moves its mtime).
    for e in entries.iter().rev() {
        let p = target.join(&e.path);
        let t = |s: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s);
        let times = std::fs::FileTimes::new().set_modified(t(e.mtime)).set_accessed(t(e.atime));
        let f = if e.kind == FileKind::Directory {
            std::fs::File::open(&p)?
        } else {
            std::fs::OpenOptions::new().write(true).open(&p)?
        };
        f.set_times(times)?;
    }
    Ok(report)
}
