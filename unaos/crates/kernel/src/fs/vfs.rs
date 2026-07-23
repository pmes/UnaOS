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

//! VFS-1 — the unifying virtual-filesystem layer (spine).
//!
//! Design of record: [`docs/dev/OS/09_FILESYSTEM/vfs.md`]. This module is the
//! spine that document specifies — the mount table, the path resolver, the
//! backend trait, and thin adapters over the two backends that already exist
//! (native UnaFS and FAT). It is deliberately **unconsumed** this arc: no shell
//! command, syscall, or EL0 path routes through it yet. The spine + doc land
//! alone so the design can be reviewed before consumers move onto it.
//!
//! ## Why this exists
//!
//! Three filesystems now coexist on the same machine: the native UnaFS volume,
//! the on-SD FAT volume, and a hot-plugged USB FAT stick. Each was reached
//! today by an ad-hoc `fat::mount()` / `unafs::with_unafs()` call at the call
//! site, with the namespace mapping (`/` vs `/usb`) hand-rolled in the shell.
//! The VFS replaces that with ONE namespace: a mount table maps a volume prefix
//! to a backend, a resolver splits a path into `(backend, volume-relative
//! path)`, and every backend answers the same small read-side trait.
//!
//! ## Shape (not POSIX)
//!
//! We do not ape the POSIX inode/dentry VFS. UnaOS owns the whole stack, so the
//! trait is exactly the surface `SYS_OPEN`-for-read needs today —
//! [`read_dir`](VfsBackend::read_dir) / [`stat`](VfsBackend::stat) /
//! [`read`](VfsBackend::read) / [`open_read`](VfsBackend::open_read) — plus the
//! one thing that makes a capability OS different from a Unix: the ACL check is
//! part of the open contract, not a bolt-on. Write verbs are shaped (the
//! `&self` receiver leaves room) but deferred: no backend exposes a mutation
//! through the trait this arc.
//!
//! ## Foreign-volume ACL posture (the load-bearing ruling)
//!
//! The native volume carries a per-object owner/grants ACL (U6); FAT carries no
//! owners at all. Rather than invent fake per-file owners for FAT, the VFS
//! authorizes a **foreign** volume at MOUNT time: the mount carries a single
//! *volume principal* (and an optional world-readable posture), and every
//! object on that volume inherits it. Revocation granularity for a foreign
//! volume is therefore the **unmount**, not a per-file grant. See the doc §5.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The kernel authority principal — mirrors [`crate::fs::unafs::KERNEL_PRINCIPAL`]
/// but is defined here so the neutral spine (and its x86 build, which has no
/// unafs module) does not depend on the aarch64-only native backend.
pub const KERNEL_PRINCIPAL: &str = "kernel";

/// A VFS-level failure. Backends map their own error types into these; the
/// variants are the ones a resolver/consumer actually distinguishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VfsError {
    /// No mount's prefix claims this path.
    NoSuchVolume,
    /// The path resolves to a volume but names nothing within it.
    NoSuchPath,
    /// A path component that must be a directory was a file.
    NotADirectory,
    /// A read/stat targeted a directory where a file was required.
    IsADirectory,
    /// The ACL refused this principal (native per-object, or the foreign
    /// volume's mount capability).
    Denied,
    /// The backend failed for a reason the VFS does not model finely; the
    /// static string is the backend's own reason, for tracing only.
    Backend(&'static str),
}

/// What a resolved node is. Deliberately two-valued: the VFS does not surface
/// FAT volume-label pseudo-entries or unafs symlinks yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Dir,
}

/// The metadata `stat`/`open_read` return. Kept to what a descriptor needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub kind: NodeKind,
    pub size: u64,
}

/// One directory entry as the VFS presents it — a display name plus a kind.
/// Backend-specific identity (FAT first-cluster, unafs inode id) is NOT exposed;
/// the VFS is name-addressed, and re-resolution from the name is the contract
/// (the same discipline the shell's FAT path cache already follows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEnt {
    pub name: String,
    pub kind: NodeKind,
}

/// The read-side surface every backend answers. `rel` is always a
/// **volume-relative** path (the resolver has already stripped the mount
/// prefix): `""` or `"/"` is the volume root, `"/a/b"` a nested name.
///
/// Path-resolution posture (case/LFN) is the **backend's**, not the VFS's — the
/// doc §3 fixes this per backend: FAT is case-insensitive with VFAT long names,
/// native UnaFS is case-sensitive exact-byte. The VFS does not normalize case;
/// it forwards `rel` verbatim so each backend applies its own on-disk rule.
pub trait VfsBackend {
    /// The volume's own name (`"native"`, `"usb"`, …) — for tracing/listing.
    fn volume_name(&self) -> &str;

    /// List the directory at `rel`. Errors: [`VfsError::NoSuchPath`] (absent),
    /// [`VfsError::NotADirectory`] (a file).
    fn read_dir(&self, rel: &str) -> Result<Vec<DirEnt>, VfsError>;

    /// Metadata for the node at `rel`.
    fn stat(&self, rel: &str) -> Result<Stat, VfsError>;

    /// Read up to `len` bytes from the file at `rel` starting at `offset`.
    /// [`VfsError::IsADirectory`] if `rel` is a directory.
    fn read(&self, rel: &str, offset: u64, len: usize) -> Result<Vec<u8>, VfsError>;

    /// The ACL check that composes into `SYS_OPEN`. Permit → `Ok(())`; refuse →
    /// [`VfsError::Denied`]. Native volumes consult the per-object owner/grants
    /// ACL; foreign volumes apply the volume-level mount capability uniformly
    /// (see the module note and doc §5).
    fn authorize_read(&self, rel: &str, principal: &str) -> Result<(), VfsError>;

    /// open-for-read = authorize, then hand back the stat the caller turns into
    /// a file descriptor. The default composition is the whole open contract:
    /// **authorize first**, then stat — so a denied principal never learns a
    /// file's size or even its existence-as-file. Backends should not override
    /// this ordering.
    fn open_read(&self, rel: &str, principal: &str) -> Result<Stat, VfsError> {
        self.authorize_read(rel, principal)?;
        self.stat(rel)
    }
}

/// One mount: a namespace prefix bound to a backend. The prefix is canonical —
/// it starts with `/`, and (except the root `/`) carries no trailing slash.
struct Mount {
    prefix: String,
    backend: Box<dyn VfsBackend>,
}

/// The process-wide namespace: an ordered set of mounts, longest-prefix wins.
///
/// Lifecycle (doc §6): [`mount`](Self::mount) binds a prefix at hot-plug time
/// (the USB stick tonight proves the need), [`unmount`](Self::unmount) removes
/// it on eject. A foreign volume's whole authority is its mount entry, so an
/// unmount is also the revocation of every capability that volume conferred.
pub struct MountTable {
    mounts: Vec<Mount>,
}

impl Default for MountTable {
    fn default() -> Self {
        Self::new()
    }
}

impl MountTable {
    pub const fn new() -> Self {
        Self { mounts: Vec::new() }
    }

    /// Bind `backend` at namespace `prefix`. A prefix already present is
    /// replaced (a re-mount). `prefix` is canonicalized: a missing leading `/`
    /// is added and a trailing `/` (other than the bare root) is dropped.
    pub fn mount(&mut self, prefix: &str, backend: Box<dyn VfsBackend>) {
        let prefix = canonical_prefix(prefix);
        self.mounts.retain(|m| m.prefix != prefix);
        self.mounts.push(Mount { prefix, backend });
    }

    /// Remove the mount at `prefix` (hot-unmount / eject). Returns whether one
    /// was present.
    pub fn unmount(&mut self, prefix: &str) -> bool {
        let prefix = canonical_prefix(prefix);
        let before = self.mounts.len();
        self.mounts.retain(|m| m.prefix != prefix);
        self.mounts.len() != before
    }

    /// The names of the mounted volumes' prefixes, for `mount`-style listing.
    pub fn prefixes(&self) -> Vec<&str> {
        self.mounts.iter().map(|m| m.prefix.as_str()).collect()
    }

    /// Resolve an absolute `path` to `(backend, volume-relative path)` by
    /// **longest matching prefix**. A prefix matches only at a path boundary:
    /// `/usb` claims `/usb` and `/usb/...` but never `/usbfoo`. The bare root
    /// `/` claims everything (with the full path as the relative remainder), so
    /// as long as a root mount exists this never returns
    /// [`VfsError::NoSuchVolume`].
    pub fn resolve<'a>(&'a self, path: &'a str) -> Result<(&'a dyn VfsBackend, &'a str), VfsError> {
        let path = if path.is_empty() { "/" } else { path };
        let mut best: Option<&Mount> = None;
        for m in &self.mounts {
            if prefix_claims(&m.prefix, path) {
                match best {
                    Some(b) if b.prefix.len() >= m.prefix.len() => {}
                    _ => best = Some(m),
                }
            }
        }
        let m = best.ok_or(VfsError::NoSuchVolume)?;
        // The volume-relative remainder: strip the prefix. Root ("/") keeps the
        // whole path; a named prefix strips it, leaving "" for the mount point
        // itself or "/rest" below it.
        let rel = if m.prefix == "/" {
            path
        } else {
            &path[m.prefix.len()..]
        };
        Ok((m.backend.as_ref(), rel))
    }

    // --- resolve-then-dispatch conveniences (a consumer's one-call surface) ---

    pub fn read_dir(&self, path: &str) -> Result<Vec<DirEnt>, VfsError> {
        let (b, rel) = self.resolve(path)?;
        b.read_dir(rel)
    }

    pub fn stat(&self, path: &str) -> Result<Stat, VfsError> {
        let (b, rel) = self.resolve(path)?;
        b.stat(rel)
    }

    pub fn read(&self, path: &str, offset: u64, len: usize) -> Result<Vec<u8>, VfsError> {
        let (b, rel) = self.resolve(path)?;
        b.read(rel, offset, len)
    }

    /// The unified open-for-read: resolve the namespace, then run the resolved
    /// backend's ACL-composing open. This is the single entry `SYS_OPEN` will
    /// call in a follow-up arc.
    pub fn open_read(&self, path: &str, principal: &str) -> Result<Stat, VfsError> {
        let (b, rel) = self.resolve(path)?;
        b.open_read(rel, principal)
    }
}

/// Canonicalize a mount prefix: ensure a leading `/`, drop a trailing `/`
/// (keeping the bare root `/`).
fn canonical_prefix(prefix: &str) -> String {
    let mut p = if prefix.starts_with('/') {
        prefix.to_string()
    } else {
        let mut s = String::from("/");
        s.push_str(prefix);
        s
    };
    while p.len() > 1 && p.ends_with('/') {
        p.pop();
    }
    p
}

/// Does `prefix` claim `path`, matching only at a component boundary? The root
/// `/` claims everything; a named prefix claims itself and its descendants.
fn prefix_claims(prefix: &str, path: &str) -> bool {
    if prefix == "/" {
        return true;
    }
    if !path.starts_with(prefix) {
        return false;
    }
    // Boundary: the char after the prefix must be a separator or end-of-path,
    // so "/usb" does not claim "/usbfoo".
    match path.as_bytes().get(prefix.len()) {
        None => true,
        Some(b'/') => true,
        _ => false,
    }
}

/// Split a volume-relative path into its non-empty components. `""`/`"/"` give
/// an empty iterator (the volume root). Shared by the backend adapters so their
/// walk logic is identical.
fn components(rel: &str) -> impl Iterator<Item = &str> {
    rel.split('/').filter(|c| !c.is_empty())
}

// =========================================================================================
// Adapters — thin glue over the EXISTING backends. Neither FS is rewritten; each adapter
// wraps that backend's public mount API and translates its DirEntry/Inode/error into the
// VFS trait's neutral types.
// =========================================================================================

/// FAT backend adapter (arch-neutral: FAT runs on both x86 and aarch64).
///
/// FAT carries no owners, so this adapter holds the volume's ACL posture: the
/// `principal` a mount conferred and whether the volume is world-readable. It
/// re-mounts through [`crate::fs::fat::mount`] per call (the same stateless
/// posture the shell's FAT commands already use — a swapped card is picked up
/// on the next access).
pub struct FatBackend {
    volume: String,
    /// The volume principal every object on this foreign volume inherits.
    principal: String,
    /// When true, any principal may read (a world-readable mount, e.g. the boot
    /// USB stick); else only the volume principal and kernel authority.
    world_readable: bool,
}

impl FatBackend {
    /// Mount a FAT volume into the VFS with an explicit ACL posture.
    pub fn new(volume: &str, principal: &str, world_readable: bool) -> Self {
        Self {
            volume: volume.to_string(),
            principal: principal.to_string(),
            world_readable,
        }
    }

    /// Resolve a volume-relative path to its FAT directory entry by walking the
    /// directory tree from the root through the public `FatFs` surface. Returns
    /// the entry, or `None` for the volume root (which has no entry of its own).
    #[cfg(target_arch = "aarch64")]
    fn resolve_entry(
        fs: &crate::fs::fat::FatFs,
        rel: &str,
    ) -> Result<Option<crate::fs::fat::DirEntry>, VfsError> {
        let mut dir = fs.read_root().map_err(fat_err)?;
        let mut found: Option<crate::fs::fat::DirEntry> = None;
        let mut prev_was_file = false;
        for comp in components(rel) {
            // A file consumed on a previous component cannot be descended into:
            // a further component makes it a "directory" that is a file.
            if prev_was_file {
                return Err(VfsError::NotADirectory);
            }
            let entry = dir
                .into_iter()
                .find(|e| e.name().eq_ignore_ascii_case(comp))
                .ok_or(VfsError::NoSuchPath)?;
            if entry.is_dir {
                dir = fs.read_dir(entry.first_cluster()).map_err(fat_err)?;
            } else {
                dir = Vec::new();
                prev_was_file = true;
            }
            found = Some(entry);
        }
        Ok(found)
    }
}

/// Map a FAT error into the VFS error space.
#[cfg(target_arch = "aarch64")]
fn fat_err(e: crate::fs::fat::FatError) -> VfsError {
    use crate::fs::fat::FatError;
    match e {
        FatError::NotFound => VfsError::NoSuchPath,
        FatError::IsDirectory => VfsError::IsADirectory,
        _ => VfsError::Backend(crate::fs::fat::fat_reason(e)),
    }
}

#[cfg(target_arch = "aarch64")]
impl VfsBackend for FatBackend {
    fn volume_name(&self) -> &str {
        &self.volume
    }

    fn read_dir(&self, rel: &str) -> Result<Vec<DirEnt>, VfsError> {
        let fs = crate::fs::fat::mount().map_err(fat_err)?;
        let entries = match Self::resolve_entry(&fs, rel)? {
            None => fs.read_root().map_err(fat_err)?, // volume root
            Some(e) if e.is_dir => fs.read_dir(e.first_cluster()).map_err(fat_err)?,
            Some(_) => return Err(VfsError::NotADirectory),
        };
        Ok(entries
            .into_iter()
            .map(|e| DirEnt {
                name: e.name().to_string(),
                kind: if e.is_dir { NodeKind::Dir } else { NodeKind::File },
            })
            .collect())
    }

    fn stat(&self, rel: &str) -> Result<Stat, VfsError> {
        let fs = crate::fs::fat::mount().map_err(fat_err)?;
        match Self::resolve_entry(&fs, rel)? {
            None => Ok(Stat {
                kind: NodeKind::Dir,
                size: 0,
            }), // the volume root is a directory
            Some(e) => Ok(Stat {
                kind: if e.is_dir { NodeKind::Dir } else { NodeKind::File },
                size: e.size as u64,
            }),
        }
    }

    fn read(&self, rel: &str, offset: u64, len: usize) -> Result<Vec<u8>, VfsError> {
        let fs = crate::fs::fat::mount().map_err(fat_err)?;
        let entry = Self::resolve_entry(&fs, rel)?.ok_or(VfsError::IsADirectory)?;
        if entry.is_dir {
            return Err(VfsError::IsADirectory);
        }
        let mut out = Vec::new();
        fs.read_at(
            entry.first_cluster(),
            entry.size,
            offset as u32,
            &mut out,
            len,
        )
        .map_err(fat_err)?;
        Ok(out)
    }

    fn authorize_read(&self, _rel: &str, principal: &str) -> Result<(), VfsError> {
        // FOREIGN-VOLUME POSTURE (doc §5): no per-file owners. The whole volume
        // shares one mount capability — permit the volume principal, kernel
        // authority always, and everyone iff the mount is world-readable.
        if self.world_readable
            || principal == self.principal
            || principal == KERNEL_PRINCIPAL
        {
            Ok(())
        } else {
            Err(VfsError::Denied)
        }
    }
}

/// Native UnaFS backend adapter (aarch64 only — the kernel `unafs` module is
/// aarch64-gated). Wraps the one coherent [`crate::fs::unafs::with_unafs`] mount
/// and defers the ACL to [`crate::fs::unafs::read_authz`] — the SAME per-object
/// owner/grants evaluator the syscall layer's live-read check uses, so the VFS
/// open and the existing SYS_OPEN authorize identically.
#[cfg(target_arch = "aarch64")]
pub struct NativeBackend {
    volume: String,
}

#[cfg(target_arch = "aarch64")]
impl NativeBackend {
    pub fn new(volume: &str) -> Self {
        Self {
            volume: volume.to_string(),
        }
    }
}

/// Map a unafs mount error into the VFS error space.
#[cfg(target_arch = "aarch64")]
fn unafs_err(_e: crate::fs::unafs::MountError) -> VfsError {
    VfsError::Backend("unafs-mount")
}

#[cfg(target_arch = "aarch64")]
impl VfsBackend for NativeBackend {
    fn volume_name(&self) -> &str {
        &self.volume
    }

    fn read_dir(&self, rel: &str) -> Result<Vec<DirEnt>, VfsError> {
        let path = native_abs(rel);
        crate::fs::unafs::with_unafs(|fs| {
            let id = fs.resolve_path(&path).map_err(|_| VfsError::NoSuchPath)?;
            let entries = fs.ls(id).map_err(|_| VfsError::NotADirectory)?;
            Ok(entries
                .into_iter()
                .map(|e| DirEnt {
                    name: e.name,
                    kind: native_kind(e.kind),
                })
                .collect())
        })
        .map_err(unafs_err)?
    }

    fn stat(&self, rel: &str) -> Result<Stat, VfsError> {
        let path = native_abs(rel);
        crate::fs::unafs::with_unafs(|fs| {
            let id = fs.resolve_path(&path).map_err(|_| VfsError::NoSuchPath)?;
            let ino = fs.read_inode(id).map_err(|_| VfsError::NoSuchPath)?;
            Ok(Stat {
                kind: native_kind(ino.kind),
                size: ino.size,
            })
        })
        .map_err(unafs_err)?
    }

    fn read(&self, rel: &str, offset: u64, len: usize) -> Result<Vec<u8>, VfsError> {
        let path = native_abs(rel);
        crate::fs::unafs::with_unafs(|fs| {
            let id = fs.resolve_path(&path).map_err(|_| VfsError::NoSuchPath)?;
            let ino = fs.read_inode(id).map_err(|_| VfsError::NoSuchPath)?;
            if matches!(native_kind(ino.kind), NodeKind::Dir) {
                return Err(VfsError::IsADirectory);
            }
            fs.read_data(id, offset, len as u64)
                .map_err(|_| VfsError::Backend("unafs-read"))
        })
        .map_err(unafs_err)?
    }

    fn authorize_read(&self, rel: &str, principal: &str) -> Result<(), VfsError> {
        // NATIVE POSTURE (doc §5): per-object owner/grants ACL. Defer to the one
        // read_authz evaluator (owner/grant-with-CAP_READ/public, fail-closed on
        // a deleted object) — identical semantics to the live SYS_OPEN check.
        let path = native_abs(rel);
        crate::fs::unafs::with_unafs(|fs| {
            let id = fs.resolve_path(&path).map_err(|_| VfsError::NoSuchPath)?;
            match crate::fs::unafs::read_authz(fs, id, principal) {
                crate::fs::unafs::ReadAuthz::Permit => Ok(()),
                _ => Err(VfsError::Denied),
            }
        })
        .map_err(unafs_err)?
    }
}

/// A volume-relative path → the absolute path the unafs `resolve_path` expects
/// (it is rooted at `/`). `""` → `"/"`.
#[cfg(target_arch = "aarch64")]
fn native_abs(rel: &str) -> String {
    if rel.is_empty() || rel == "/" {
        "/".to_string()
    } else if rel.starts_with('/') {
        rel.to_string()
    } else {
        let mut s = String::from("/");
        s.push_str(rel);
        s
    }
}

#[cfg(target_arch = "aarch64")]
fn native_kind(k: ::unafs::inode::FileKind) -> NodeKind {
    match k {
        ::unafs::inode::FileKind::Directory => NodeKind::Dir,
        _ => NodeKind::File,
    }
}

// =========================================================================================
// VFS-1 witness — unit-shaped proof that resolution composes across TWO backends.
//
// Arch-neutral (no disk, no unafs/fat mount): two in-RAM mock backends stand in for the
// native and FAT volumes, mounted at "/" and "/usb". The witness asserts that (a) longest-
// prefix resolution routes each path to the right backend with the right volume-relative
// remainder, (b) the boundary rule keeps "/usbfoo" on the root volume, (c) read composes
// across both, and (d) the two ACL postures (per-object native vs volume-level foreign)
// each deny and permit as designed. It compiles on BOTH arches (proving the spine is
// arch-neutral) and is a pure function a follow-up may wire behind the `witness` feature.
// =========================================================================================

/// A minimal in-RAM backend for the witness: a flat name→bytes map plus a fixed
/// ACL posture. Not a production adapter — it exists only to exercise the
/// resolver and the trait's open contract without a mounted volume.
#[doc(hidden)]
pub struct MockBackend {
    name: String,
    files: Vec<(String, Vec<u8>)>,
    /// `None` = per-object native-style (owner == "alice", public otherwise);
    /// `Some((principal, world))` = foreign volume-level posture.
    foreign: Option<(String, bool)>,
}

#[doc(hidden)]
impl MockBackend {
    fn native(name: &str) -> Self {
        Self {
            name: name.to_string(),
            files: alloc::vec![("/a.txt".to_string(), alloc::vec![1u8, 2, 3])],
            foreign: None,
        }
    }
    fn foreign(name: &str, principal: &str, world: bool) -> Self {
        Self {
            name: name.to_string(),
            files: alloc::vec![("/b.txt".to_string(), alloc::vec![9u8, 8])],
            foreign: Some((principal.to_string(), world)),
        }
    }
}

#[doc(hidden)]
impl VfsBackend for MockBackend {
    fn volume_name(&self) -> &str {
        &self.name
    }
    fn read_dir(&self, _rel: &str) -> Result<Vec<DirEnt>, VfsError> {
        Ok(self
            .files
            .iter()
            .map(|(n, _)| DirEnt {
                name: n.trim_start_matches('/').to_string(),
                kind: NodeKind::File,
            })
            .collect())
    }
    fn stat(&self, rel: &str) -> Result<Stat, VfsError> {
        if rel.is_empty() || rel == "/" {
            return Ok(Stat {
                kind: NodeKind::Dir,
                size: 0,
            });
        }
        self.files
            .iter()
            .find(|(n, _)| n == rel)
            .map(|(_, d)| Stat {
                kind: NodeKind::File,
                size: d.len() as u64,
            })
            .ok_or(VfsError::NoSuchPath)
    }
    fn read(&self, rel: &str, offset: u64, len: usize) -> Result<Vec<u8>, VfsError> {
        let (_, d) = self
            .files
            .iter()
            .find(|(n, _)| n == rel)
            .ok_or(VfsError::NoSuchPath)?;
        let start = (offset as usize).min(d.len());
        let end = (start + len).min(d.len());
        Ok(d[start..end].to_vec())
    }
    fn authorize_read(&self, _rel: &str, principal: &str) -> Result<(), VfsError> {
        match &self.foreign {
            // Foreign: volume-level capability.
            Some((p, world)) => {
                if *world || principal == p || principal == KERNEL_PRINCIPAL {
                    Ok(())
                } else {
                    Err(VfsError::Denied)
                }
            }
            // Native-style: owner "alice", public to none else (kernel always).
            None => {
                if principal == "alice" || principal == KERNEL_PRINCIPAL {
                    Ok(())
                } else {
                    Err(VfsError::Denied)
                }
            }
        }
    }
}

/// The VFS-1 resolution witness. Returns `Ok(())` when every assertion holds;
/// `Err(reason)` names the first that failed. Pure and arch-neutral.
pub fn vfs1_resolution_witness() -> Result<(), &'static str> {
    let mut mt = MountTable::new();
    mt.mount("/", Box::new(MockBackend::native("native")));
    mt.mount("/usb", Box::new(MockBackend::foreign("usb", "installer", false)));

    // (a) longest-prefix routing + relative remainder.
    let (b, rel) = mt.resolve("/usb/b.txt").map_err(|_| "resolve /usb/b.txt")?;
    if b.volume_name() != "usb" || rel != "/b.txt" {
        return Err("routing: /usb/b.txt -> wrong backend/rel");
    }
    let (b, rel) = mt.resolve("/a.txt").map_err(|_| "resolve /a.txt")?;
    if b.volume_name() != "native" || rel != "/a.txt" {
        return Err("routing: /a.txt -> wrong backend/rel");
    }
    // The mount point itself resolves to its backend with an empty remainder.
    let (b, rel) = mt.resolve("/usb").map_err(|_| "resolve /usb")?;
    if b.volume_name() != "usb" || !rel.is_empty() {
        return Err("routing: /usb (mount point) -> wrong backend/rel");
    }

    // (b) boundary rule: "/usbfoo" belongs to the root volume, not "/usb".
    let (b, rel) = mt.resolve("/usbfoo").map_err(|_| "resolve /usbfoo")?;
    if b.volume_name() != "native" || rel != "/usbfoo" {
        return Err("boundary: /usbfoo leaked into /usb");
    }

    // (c) read composes across both backends.
    if mt.read("/a.txt", 0, 8) != Ok(alloc::vec![1u8, 2, 3]) {
        return Err("read: native /a.txt");
    }
    if mt.read("/usb/b.txt", 0, 8) != Ok(alloc::vec![9u8, 8]) {
        return Err("read: foreign /usb/b.txt");
    }

    // (d) the two ACL postures.
    //   Native per-object: owner permitted, stranger denied, kernel permitted.
    if mt.open_read("/a.txt", "alice").is_err() {
        return Err("acl native: owner denied");
    }
    if mt.open_read("/a.txt", "mallory") != Err(VfsError::Denied) {
        return Err("acl native: stranger not denied");
    }
    if mt.open_read("/a.txt", KERNEL_PRINCIPAL).is_err() {
        return Err("acl native: kernel denied");
    }
    //   Foreign volume-level: mount principal permitted, others denied (not
    //   world-readable), kernel permitted.
    if mt.open_read("/usb/b.txt", "installer").is_err() {
        return Err("acl foreign: volume principal denied");
    }
    if mt.open_read("/usb/b.txt", "alice") != Err(VfsError::Denied) {
        return Err("acl foreign: non-volume principal not denied");
    }
    if mt.open_read("/usb/b.txt", KERNEL_PRINCIPAL).is_err() {
        return Err("acl foreign: kernel denied");
    }

    // (e) unmount is the foreign volume's revocation: after eject, /usb/... no
    // longer resolves to the usb backend (falls through to root -> NoSuchPath).
    mt.unmount("/usb");
    match mt.resolve("/usb/b.txt") {
        Ok((b, _)) if b.volume_name() == "native" => {}
        _ => return Err("unmount: /usb still bound after eject"),
    }

    Ok(())
}
