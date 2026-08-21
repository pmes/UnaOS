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
//! command, syscall, or user path routes through it yet. The spine + doc land
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
    /// volume's mount capability). On a write op this is the *write*-side ACL
    /// refusal (VFS-2): the principal lacks the write right on this object.
    Denied,
    /// The requested mutation is not expressible through this backend's write
    /// surface (VFS-2). Examples: an in-place *shrink* truncate (neither the FAT
    /// nor the UnaFS backend carries a shrink primitive this arc), a name that
    /// is not a representable FAT 8.3 short name (LFN write is out of scope this
    /// arc), or a write op on a backend that exposes no mutating surface at all
    /// (the default trait bodies). Distinct from [`VfsError::Denied`] — the
    /// caller is authorized, the operation itself has no sound implementation.
    Unsupported,
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

    // --- Write surface (VFS-2) -------------------------------------------------
    //
    // The mutating half of the open contract the design doc (§2) shaped and
    // deferred. It mirrors the read side's discipline: a `authorize_write` ACL
    // check that the mutating verbs compose FIRST, then act — so a principal
    // without the write right never mutates (and, for `create`, never learns
    // whether the name was free). The default bodies make the surface *opt-in*:
    // a backend that exposes no mutation (the witness mock, a future read-only
    // adapter) inherits [`VfsError::Unsupported`] for every verb and needs no
    // change. The doc's op naming is honored — `create` / `write` / `unlink`,
    // plus `truncate` (VFS-2's brief) — and no new shape is invented.

    /// The WRITE-side ACL check, the mutating twin of [`authorize_read`]. Permit
    /// → `Ok(())`; the principal lacks the write right → [`VfsError::Denied`].
    ///
    /// Native volumes consult the per-object owner/grants ACL for the WRITE
    /// right (`CAP_WRITE`); foreign (FAT) volumes apply the volume-level mount
    /// capability — but write is **never** granted by the world-**readable**
    /// posture (a stick mounted for reading is not thereby writable): only the
    /// volume principal and kernel authority may write a foreign volume.
    ///
    /// [`authorize_read`]: VfsBackend::authorize_read
    fn authorize_write(&self, _rel: &str, _principal: &str) -> Result<(), VfsError> {
        Err(VfsError::Unsupported)
    }

    /// Create a new node (`NodeKind::File` or `NodeKind::Dir`) at `rel`. The
    /// parent directory must exist; the leaf must not (a backend may reject an
    /// existing name with [`VfsError::Backend`]). Returns the new node's stat.
    /// Implementors authorize the write FIRST (via [`authorize_write`]).
    ///
    /// [`authorize_write`]: VfsBackend::authorize_write
    fn create(&self, _rel: &str, _kind: NodeKind, _principal: &str) -> Result<Stat, VfsError> {
        Err(VfsError::Unsupported)
    }

    /// Write `data` to the file at `rel` starting at `offset`, growing the file
    /// (allocating storage) as needed. `offset` may not exceed the current size
    /// (no sparse holes). Returns the number of bytes written. Implementors
    /// authorize the write FIRST.
    fn write(&self, _rel: &str, _offset: u64, _data: &[u8], _principal: &str) -> Result<usize, VfsError> {
        Err(VfsError::Unsupported)
    }

    /// Set the file at `rel` to exactly `size` bytes. Growing zero-extends;
    /// `size == current` is a no-op; truncation to `0` is supported. An in-place
    /// *shrink* to a non-zero size is [`VfsError::Unsupported`] this arc (neither
    /// backend carries a shrink primitive). Implementors authorize FIRST.
    fn truncate(&self, _rel: &str, _size: u64, _principal: &str) -> Result<(), VfsError> {
        Err(VfsError::Unsupported)
    }

    /// Remove the file at `rel`. A directory is refused with
    /// [`VfsError::IsADirectory`] (directory removal is a separate verb).
    /// Implementors authorize the write FIRST.
    fn unlink(&self, _rel: &str, _principal: &str) -> Result<(), VfsError> {
        Err(VfsError::Unsupported)
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

    // --- write-side resolve-then-dispatch conveniences (VFS-2) ---

    pub fn create(&self, path: &str, kind: NodeKind, principal: &str) -> Result<Stat, VfsError> {
        let (b, rel) = self.resolve(path)?;
        b.create(rel, kind, principal)
    }

    pub fn write(&self, path: &str, offset: u64, data: &[u8], principal: &str) -> Result<usize, VfsError> {
        let (b, rel) = self.resolve(path)?;
        b.write(rel, offset, data, principal)
    }

    pub fn truncate(&self, path: &str, size: u64, principal: &str) -> Result<(), VfsError> {
        let (b, rel) = self.resolve(path)?;
        b.truncate(rel, size, principal)
    }

    pub fn unlink(&self, path: &str, principal: &str) -> Result<(), VfsError> {
        let (b, rel) = self.resolve(path)?;
        b.unlink(rel, principal)
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
/// re-mounts through [`crate::fs::fat::mount_source`] per call (the same
/// stateless posture the shell's FAT commands already use — a swapped card is
/// picked up on the next access).
///
/// VFS-3: the adapter is parametrized by the block [`crate::fs::fat::BlockSource`]
/// it mounts through, so ONE `MountTable` can carry BOTH FAT volumes the Pi
/// exposes at once — the SD boot partition ([`Default`](crate::fs::fat::BlockSource::Default),
/// at `/fat`) and the hot-plugged USB stick ([`Usb`](crate::fs::fat::BlockSource::Usb),
/// at `/usb`) — each reaching its own device.
///
/// USBFALL F3 (was PIUSB-27): a `Usb`-sourced mount is **no longer read-only by
/// construction**. USB-WRITE routed `fat::write_sector`'s `Usb` arm to the verified
/// BOT WRITE(10) path, and [`FatBackend::read_only`] reports `false` for the `Usb`
/// source on aarch64 (and for `Default` whenever the block layer will accept its
/// writes — see USBFALL F1 there) — so FAT/dir/data writes DO reach the stick and the
/// adapter passes write verbs through rather than refusing them. The residual cost
/// is documented on `fat::with_fat_lock` (a `Usb` sector RMW is held under masked
/// IRQs for the BOT deadline, not for a polled transfer).
pub struct FatBackend {
    volume: String,
    /// The volume principal every object on this foreign volume inherits.
    principal: String,
    /// When true, any principal may read (a world-readable mount, e.g. the boot
    /// USB stick); else only the volume principal and kernel authority.
    world_readable: bool,
    /// VFS-3: which block device this volume mounts through. `Default` = the
    /// globally-registered device (SD on the Pi); `Usb` = the USB stick read
    /// directly through xHCI (read-only, PIUSB-27).
    source: crate::fs::fat::BlockSource,
}

impl FatBackend {
    /// Mount a FAT volume into the VFS with an explicit ACL posture, reading
    /// through the globally-registered block device
    /// ([`Default`](crate::fs::fat::BlockSource::Default) — the SD boot partition
    /// on the Pi). Writable (the boot FAT).
    pub fn new(volume: &str, principal: &str, world_readable: bool) -> Self {
        Self {
            volume: volume.to_string(),
            principal: principal.to_string(),
            world_readable,
            source: crate::fs::fat::BlockSource::Default,
        }
    }

    /// VFS-3: mount the hot-plugged USB FAT stick into the VFS, read through the
    /// xHCI [`Usb`](crate::fs::fat::BlockSource::Usb) source — the same mount
    /// `ls /usb` and the `/fs/usb` HTTP route already use. World-readable (its
    /// contents are meant to be read) and **writable** since USB-WRITE: the
    /// write verbs route to the verified BOT WRITE(10) path (`write_block_usb`,
    /// MISSION RMW+restore witnessed), which superseded the PIUSB-27 guard.
    pub fn new_usb(volume: &str, principal: &str) -> Self {
        Self {
            volume: volume.to_string(),
            principal: principal.to_string(),
            world_readable: true,
            source: crate::fs::fat::BlockSource::Usb,
        }
    }

    /// VFS-3/USB-WRITE: is this a read-only mount? Both current sources have a
    /// verified write path (`Default` = SD, `Usb` = BOT WRITE(10) with the
    /// MISSION RMW+restore witness), so neither is read-only *by construction*;
    /// a future source without a verified write path returns true here.
    ///
    /// USBFALL F1: a `Default` mount is additionally read-only *by condition* when
    /// the block layer would refuse its writes — i.e. on Pi bare-metal with no SD
    /// registered, where `write_block` fails closed rather than substituting the
    /// USB stick. Without this the boot LOOKED writable and every write failed
    /// late with an opaque `Io`; now the mount answers honestly up front and the
    /// VFS write verbs return `Unsupported` ("read-only volume") before touching
    /// the block path. Byte-inert on a healthy SD boot (`default_writable()` is
    /// true before the first mount) and on every non-`baremetal` target, where
    /// `default_writable()` is a constant `true`. `Usb` is unaffected: it reaches
    /// the stick through its own `write_block_usb` handle, which the F1 guard
    /// deliberately does not gate.
    ///
    /// FATVERB: this is now a FORWARD, not a second copy. It used to carry its own `match` over
    /// the source, and the shell's write gate carried another — two predicates for one question,
    /// free to drift, on a target where the `Default` arm is the difference between a write that
    /// lands and a write that fails closed several sectors in. `BlockSource::write_veto` is the
    /// single definition; the VFS reports its presence as a boolean and the shell prints its text.
    fn read_only(&self) -> bool {
        self.source.write_veto().is_some()
    }

    /// Resolve a volume-relative path to its FAT directory entry by walking the
    /// directory tree from the root through the public `FatFs` surface. Returns
    /// the entry, or `None` for the volume root (which has no entry of its own).
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

    /// VFS-2: resolve `rel` to `(parent_first_cluster, leaf_name)` — the shape
    /// the dir-aware fat.rs write twins (`create_in_dir`/`locate_in_dir`) take.
    /// The parent directory is walked read-only; `first_cluster == 0` is the
    /// volume root. `rel` must name a leaf under a directory: the bare root
    /// (`""`/`"/"`) has no leaf and is refused [`VfsError::IsADirectory`]; a
    /// parent that is a file is [`VfsError::NotADirectory`]; an absent parent is
    /// [`VfsError::NoSuchPath`]. The leaf itself need NOT exist (it is what the
    /// caller creates/locates).
    fn resolve_parent(
        fs: &crate::fs::fat::FatFs,
        rel: &str,
    ) -> Result<(u32, String), VfsError> {
        let comps: Vec<&str> = components(rel).collect();
        let (leaf, parents) = comps.split_last().ok_or(VfsError::IsADirectory)?;
        let leaf = leaf.to_string();
        if parents.is_empty() {
            return Ok((0, leaf)); // parent is the volume root
        }
        // Walk to the parent directory and take its first cluster.
        let parent_rel = {
            let mut s = String::new();
            for c in parents {
                s.push('/');
                s.push_str(c);
            }
            s
        };
        match Self::resolve_entry(fs, &parent_rel)? {
            None => Ok((0, leaf)), // parent resolved to the root
            Some(e) if e.is_dir => Ok((e.first_cluster(), leaf)),
            Some(_) => Err(VfsError::NotADirectory),
        }
    }
}

/// Map a FAT error into the VFS error space.
fn fat_err(e: crate::fs::fat::FatError) -> VfsError {
    use crate::fs::fat::FatError;
    match e {
        FatError::NotFound => VfsError::NoSuchPath,
        FatError::IsDirectory => VfsError::IsADirectory,
        _ => VfsError::Backend(crate::fs::fat::fat_reason(e)),
    }
}

/// VFS-2: map a FAT *create*-path error. `Unsupported` here means the name is
/// not a representable 8.3 short name — VFAT LFN write is out of scope this arc
/// (documented bound), so it surfaces as [`VfsError::Unsupported`] rather than
/// an opaque backend string. Everything else maps as [`fat_err`].
fn fat_create_err(e: crate::fs::fat::FatError) -> VfsError {
    match e {
        crate::fs::fat::FatError::Unsupported => VfsError::Unsupported,
        other => fat_err(other),
    }
}

/// VFSX86 (2026-08-21): this impl was `#[cfg(target_arch = "aarch64")]` from VFS-1/VFS-2. The gate
/// recorded WHERE THE WORK WAS DONE (the Pi came first), not a hardware constraint — there is no
/// arch-specific line in the body. Every primitive it calls is compiled unconditionally on x86_64
/// today and always was: `fat::mount_source` (fat.rs — arch-neutral, and its `match` even carries an
/// x86-only `Sdhc` arm), `read_root`/`read_dir`/`read_at`, and the whole write half —
/// `locate_in_dir`, `create_in_dir`, `create_dir`, `write_grow`, `delete_located`. The one genuinely
/// gated dependency was `fat::fat_reason`, itself gated only by its first caller; it is now
/// arch-neutral too (see its note). aarch64 behaviour is unchanged by construction: no arm of this
/// impl, and no callee, was edited — the gate was deleted, nothing else.
///
/// ⚠ WHAT THIS DOES **NOT** DO. Widening the seam does not enroll anyone in the x86 write
/// discipline. x86 has no in-`fat.rs` FAT/directory mutation lock — `fat::with_fat_lock` and
/// `with_dir_lock` are `#[inline(always)]` passthroughs there, and consistency is held CALLER-SIDE
/// by the "X86 FAT-MUTATOR ROSTER" documented on `fat::with_fat_lock`. This impl is a seam, not a
/// caller: on x86 it has ZERO callers as landed, so it mutates nothing and joins no row. The roster
/// rule binds whoever calls it — an x86 consumer migrating onto these verbs must either submit
/// through the storage-service task or run in program order on the BSP main loop ahead of the
/// launchers, and must add itself to that roster. Migrating the three existing direct callers
/// (`shell.rs`, `fs/flight_recorder.rs`, the Holocron bond store) is deliberately NOT part of this
/// change.
///
/// The write verbs' ACL is unchanged and is what the direct callers today do NOT have: each verb
/// calls `authorize_write` FIRST, which refuses a read-only volume (`Unsupported`, via
/// `FatBackend::read_only` -> `BlockSource::write_veto`) and then enforces the volume-principal ACL
/// (`principal == self.principal || principal == KERNEL_PRINCIPAL`; the `world_readable` posture is
/// deliberately not consulted for writes). The direct callers reproduce the first check and not the
/// second.
impl VfsBackend for FatBackend {
    fn volume_name(&self) -> &str {
        &self.volume
    }

    fn read_dir(&self, rel: &str) -> Result<Vec<DirEnt>, VfsError> {
        let fs = crate::fs::fat::mount_source(self.source).map_err(fat_err)?;
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
        let fs = crate::fs::fat::mount_source(self.source).map_err(fat_err)?;
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
        let fs = crate::fs::fat::mount_source(self.source).map_err(fat_err)?;
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

    fn authorize_write(&self, _rel: &str, principal: &str) -> Result<(), VfsError> {
        // VFS-3: a non-Default source (the USB stick) is read-only BY
        // CONSTRUCTION — PIUSB-27's `write_sector` refuses it, so no write could
        // ever reach the medium. Refuse here so the caller gets a clean
        // "read-only volume" (`Unsupported`) rather than a block I/O error
        // surfacing from deep in `write_grow`. This is not an ACL refusal
        // (`Denied`): the principal may be perfectly authorized; the VOLUME has
        // no writable surface. A future world-writable USB flag would relax this.
        if self.read_only() {
            return Err(VfsError::Unsupported);
        }
        // FOREIGN-VOLUME WRITE POSTURE (doc §5, extended by VFS-2): the volume's
        // one mount capability governs writes too — permit the volume principal
        // and kernel authority. The `world_readable` posture is deliberately NOT
        // consulted: a stick mounted for reading is not thereby writable. A
        // future world-WRITABLE posture would be a separate mount flag; until
        // then a foreign write is authorized exactly to the mounting principal.
        if principal == self.principal || principal == KERNEL_PRINCIPAL {
            Ok(())
        } else {
            Err(VfsError::Denied)
        }
    }

    fn create(&self, rel: &str, kind: NodeKind, principal: &str) -> Result<Stat, VfsError> {
        self.authorize_write(rel, principal)?;
        let fs = crate::fs::fat::mount_source(self.source).map_err(fat_err)?;
        let (parent, leaf) = Self::resolve_parent(&fs, rel)?;
        // Reject an existing name (create is not idempotent-overwrite).
        match fs.locate_in_dir(parent, &leaf) {
            Ok(_) => return Err(VfsError::Backend("exists")),
            Err(crate::fs::fat::FatError::NotFound) => {}
            Err(e) => return Err(fat_err(e)),
        }
        match kind {
            // 0x20 = plain file; create_in_dir yields a 0-length entry.
            NodeKind::File => {
                fs.create_in_dir(parent, &leaf, 0x20).map_err(fat_create_err)?;
                Ok(Stat { kind: NodeKind::File, size: 0 })
            }
            // create_dir allocates the child cluster + `.`/`..` and publishes it.
            NodeKind::Dir => {
                fs.create_dir(parent, &leaf).map_err(fat_create_err)?;
                Ok(Stat { kind: NodeKind::Dir, size: 0 })
            }
        }
    }

    fn write(&self, rel: &str, offset: u64, data: &[u8], principal: &str) -> Result<usize, VfsError> {
        self.authorize_write(rel, principal)?;
        let fs = crate::fs::fat::mount_source(self.source).map_err(fat_err)?;
        let (parent, leaf) = Self::resolve_parent(&fs, rel)?;
        let off32: u32 = offset.try_into().map_err(|_| VfsError::Unsupported)?;
        let (de, dir_lba, dir_off) = fs.locate_in_dir(parent, &leaf).map_err(fat_err)?;
        if de.is_dir {
            return Err(VfsError::IsADirectory);
        }
        // write_grow handles the whole span the brief scopes: overwrite-in-place
        // (offset < size), append at EOF (offset == size), and free-cluster
        // allocation when the write runs past the last cluster — publishing the
        // grown `size` / new first_cluster to the directory entry LAST (data +
        // FAT already durable). offset > size (a sparse hole) is rejected by
        // write_grow with BadChain, which we surface as Unsupported.
        match fs.write_grow(de.first_cluster(), de.size, dir_lba, dir_off, off32, data) {
            Ok((written, _new_size, _new_first)) => Ok(written),
            Err(crate::fs::fat::FatError::BadChain) if off32 > de.size => Err(VfsError::Unsupported),
            Err(e) => Err(fat_err(e)),
        }
    }

    fn truncate(&self, rel: &str, size: u64, principal: &str) -> Result<(), VfsError> {
        self.authorize_write(rel, principal)?;
        let fs = crate::fs::fat::mount_source(self.source).map_err(fat_err)?;
        let (parent, leaf) = Self::resolve_parent(&fs, rel)?;
        let (de, dir_lba, dir_off) = fs.locate_in_dir(parent, &leaf).map_err(fat_err)?;
        if de.is_dir {
            return Err(VfsError::IsADirectory);
        }
        let cur = de.size as u64;
        if size == cur {
            return Ok(()); // no-op
        }
        if size == 0 {
            // Truncate-to-zero = free the chain + a fresh 0-length entry (the
            // only shrink fat.rs's PUBLIC surface expresses; there is no in-place
            // shrink primitive). delete_located marks the slot 0xE5 FIRST, THEN
            // frees the chain (crash-safe), then a fresh entry reclaims the name.
            fs.delete_located(dir_lba, dir_off, de.first_cluster()).map_err(fat_err)?;
            fs.create_in_dir(parent, &leaf, 0x20).map_err(fat_create_err)?;
            return Ok(());
        }
        if size > cur {
            // Zero-extend: grow with a run of zero bytes from the old EOF.
            let add = (size - cur) as usize;
            let zeros = alloc::vec![0u8; add];
            let start: u32 = cur.try_into().map_err(|_| VfsError::Unsupported)?;
            fs.write_grow(de.first_cluster(), de.size, dir_lba, dir_off, start, &zeros)
                .map_err(fat_err)?;
            return Ok(());
        }
        // 0 < size < cur: an in-place shrink to a non-zero size. No primitive.
        Err(VfsError::Unsupported)
    }

    fn unlink(&self, rel: &str, principal: &str) -> Result<(), VfsError> {
        self.authorize_write(rel, principal)?;
        let fs = crate::fs::fat::mount_source(self.source).map_err(fat_err)?;
        let (parent, leaf) = Self::resolve_parent(&fs, rel)?;
        let (de, dir_lba, dir_off) = fs.locate_in_dir(parent, &leaf).map_err(fat_err)?;
        if de.is_dir {
            return Err(VfsError::IsADirectory); // directory removal is a separate verb
        }
        fs.delete_located(dir_lba, dir_off, de.first_cluster()).map_err(fat_err)?;
        Ok(())
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

/// VFS-2: the native WRITE-side ACL evaluator for a resolved live inode `id` —
/// the write twin of [`crate::fs::unafs::read_authz`], consulting the SAME
/// per-object `owner`/`grants:<principal>` attributes but testing the WRITE
/// right. It reuses the ONE grant-rights decoder
/// ([`crate::fs::unafs::rights_from_native`]) and the `CAP_WRITE`-equal
/// [`crate::fs::unafs::RIGHT_WRITE`] bit, so a VFS write and the syscall layer's
/// grant machinery agree on what a `w`/`rw` grant admits.
///
/// Semantics mirror the read evaluator's ordering: a GONE inode fails closed for
/// everyone (deletion is total revocation); then kernel authority permits; a
/// public object (no `owner`) permits (unchanged public semantics — a public
/// native object is writable, as it is readable); the owner permits; a
/// `grants:<principal>` row permits IFF it carries the write right; else denied.
///
/// This write evaluator lives in the VFS adapter (not beside `read_authz` in
/// `unafs.rs`) to keep VFS-2 within the VFS lane; a follow-up may hoist it into
/// `unafs.rs` as a `write_authz` sibling the way the read side already defers.
#[cfg(target_arch = "aarch64")]
fn native_write_authz(
    fs: &mut crate::fs::unafs::KernelUnaFS,
    id: u64,
    principal: &str,
) -> Result<(), VfsError> {
    use ::unafs::inode::AttributeValue;
    let ino = match fs.read_inode(id) {
        Ok(i) => i,
        Err(_) => return Err(VfsError::Denied), // gone -> fail closed for everyone
    };
    if principal == KERNEL_PRINCIPAL {
        return Ok(());
    }
    let owner = match ino.attributes.get("owner") {
        Some(AttributeValue::String(s)) => s.clone(),
        _ => return Ok(()), // no owner row -> public object, writable
    };
    if principal == owner {
        return Ok(());
    }
    if let Some(AttributeValue::String(rights)) =
        ino.attributes.get(&alloc::format!("grants:{}", principal))
    {
        if crate::fs::unafs::rights_from_native(rights.as_bytes()) & crate::fs::unafs::RIGHT_WRITE
            != 0
        {
            return Ok(());
        }
    }
    Err(VfsError::Denied)
}

/// VFS-2: resolve a volume-relative `rel` to `(parent_inode_id, leaf_name)` for
/// the native create/unlink path. `rel` must name a leaf under a directory: the
/// bare root has no leaf ([`VfsError::IsADirectory`]); a missing parent is
/// [`VfsError::NoSuchPath`].
#[cfg(target_arch = "aarch64")]
fn native_parent(
    fs: &mut crate::fs::unafs::KernelUnaFS,
    rel: &str,
) -> Result<(u64, String), VfsError> {
    let comps: Vec<&str> = components(rel).collect();
    let (leaf, parents) = comps.split_last().ok_or(VfsError::IsADirectory)?;
    let leaf = leaf.to_string();
    let mut parent_path = String::from("/");
    for (i, c) in parents.iter().enumerate() {
        if i > 0 {
            parent_path.push('/');
        }
        parent_path.push_str(c);
    }
    let parent_id = fs
        .resolve_path(&parent_path)
        .map_err(|_| VfsError::NoSuchPath)?;
    Ok((parent_id, leaf))
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

    fn authorize_write(&self, rel: &str, principal: &str) -> Result<(), VfsError> {
        // NATIVE WRITE POSTURE (VFS-2): per-object owner/grants ACL, WRITE right.
        let path = native_abs(rel);
        crate::fs::unafs::with_unafs(|fs| {
            let id = fs.resolve_path(&path).map_err(|_| VfsError::NoSuchPath)?;
            native_write_authz(fs, id, principal)
        })
        .map_err(unafs_err)?
    }

    fn create(&self, rel: &str, kind: NodeKind, principal: &str) -> Result<Stat, VfsError> {
        crate::fs::unafs::with_unafs(|fs| {
            // Create authorizes against the PARENT directory's write ACL (the
            // leaf does not exist yet), then plants the node under it.
            let (parent_id, leaf) = native_parent(fs, rel)?;
            native_write_authz(fs, parent_id, principal)?;
            let id = match kind {
                NodeKind::File => fs
                    .create_file(parent_id, leaf)
                    .map_err(|_| VfsError::Backend("unafs-create"))?,
                NodeKind::Dir => fs
                    .mkdir(parent_id, leaf)
                    .map_err(|_| VfsError::Backend("unafs-mkdir"))?,
            };
            // The new object ACQUIRES A REAL OWNER — the creating principal — so
            // it is not left world-writable (do-it-right: a native object carries
            // its own per-object ACL, unlike the foreign volume it may be copied
            // from). Kernel-created objects stay public (no owner row).
            if principal != KERNEL_PRINCIPAL {
                fs.set_attribute(
                    id,
                    alloc::string::String::from("owner"),
                    ::unafs::inode::AttributeValue::String(principal.to_string()),
                )
                .map_err(|_| VfsError::Backend("unafs-owner"))?;
            }
            Ok(Stat {
                kind,
                size: 0,
            })
        })
        .map_err(unafs_err)?
    }

    fn write(&self, rel: &str, offset: u64, data: &[u8], principal: &str) -> Result<usize, VfsError> {
        let path = native_abs(rel);
        crate::fs::unafs::with_unafs(|fs| {
            let id = fs.resolve_path(&path).map_err(|_| VfsError::NoSuchPath)?;
            native_write_authz(fs, id, principal)?;
            let ino = fs.read_inode(id).map_err(|_| VfsError::NoSuchPath)?;
            if matches!(native_kind(ino.kind), NodeKind::Dir) {
                return Err(VfsError::IsADirectory);
            }
            fs.write_data(id, offset, data)
                .map_err(|_| VfsError::Backend("unafs-write"))?;
            Ok(data.len())
        })
        .map_err(unafs_err)?
    }

    fn truncate(&self, rel: &str, size: u64, principal: &str) -> Result<(), VfsError> {
        let path = native_abs(rel);
        crate::fs::unafs::with_unafs(|fs| {
            let id = fs.resolve_path(&path).map_err(|_| VfsError::NoSuchPath)?;
            native_write_authz(fs, id, principal)?;
            let ino = fs.read_inode(id).map_err(|_| VfsError::NoSuchPath)?;
            if matches!(native_kind(ino.kind), NodeKind::Dir) {
                return Err(VfsError::IsADirectory);
            }
            if size == ino.size {
                return Ok(()); // no-op
            }
            if size > ino.size {
                // Zero-extend from the old EOF.
                let add = (size - ino.size) as usize;
                let zeros = alloc::vec![0u8; add];
                fs.write_data(id, ino.size, &zeros)
                    .map_err(|_| VfsError::Backend("unafs-write"))?;
                return Ok(());
            }
            // A shrink (including to 0) would DROP the per-object ACL if done by
            // unlink+recreate — the one thing the native volume must never lose.
            // UnaFS carries no in-place shrink primitive this arc, so a native
            // shrink is honestly Unsupported (a caller that wants smaller content
            // creates a fresh object and writes it). This is a DELIBERATE
            // asymmetry with the FAT backend (which truncates-to-0 by
            // delete+recreate — FAT has no per-object ACL to preserve).
            Err(VfsError::Unsupported)
        })
        .map_err(unafs_err)?
    }

    fn unlink(&self, rel: &str, principal: &str) -> Result<(), VfsError> {
        let path = native_abs(rel);
        crate::fs::unafs::with_unafs(|fs| {
            let id = fs.resolve_path(&path).map_err(|_| VfsError::NoSuchPath)?;
            native_write_authz(fs, id, principal)?;
            let ino = fs.read_inode(id).map_err(|_| VfsError::NoSuchPath)?;
            if matches!(native_kind(ino.kind), NodeKind::Dir) {
                return Err(VfsError::IsADirectory); // directory removal is a separate verb
            }
            let (parent_id, leaf) = native_parent(fs, rel)?;
            fs.unlink(parent_id, &leaf)
                .map_err(|_| VfsError::Backend("unafs-unlink"))?;
            Ok(())
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

// =========================================================================================
// VFS-2 write witnesses — self-verifying, self-cleaning proofs that a CREATE + WRITE + READ
// BACK round-trips through the mount table to each real backend on the live QEMU card. Unlike
// the arch-neutral `vfs1_resolution_witness` (in-RAM mocks), these mount the actual FAT /
// UnaFS adapter and exercise the disk write path, so they are aarch64-only and honest-skip on
// media that lacks the volume. Each runs once, cleans up its scratch file, and prints its own
// uncounted witness line. Wired into the storage battery in `syscall.rs` (VFS-2 adoption).
// =========================================================================================

/// VFS-2 FAT write witness: create → write → read-back → checksum on the FAT volume, routed
/// entirely through the `MountTable`/`FatBackend` write surface. Self-cleaning.
#[cfg(target_arch = "aarch64")]
pub fn vfs2_fat_write_witness() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::fs::fat::mount().is_err() {
        serial_println!(":: VFS2-fat: no FAT filesystem — skipped ::");
        return;
    }
    // Foreign volume mounted to the kernel principal (world_readable is a READ posture and does
    // not confer write — the witness writes as the volume principal `kernel`).
    let mut mt = MountTable::new();
    mt.mount("/", Box::new(FatBackend::new("fat", KERNEL_PRINCIPAL, true)));

    let path = "/VFS2TST.TXT";
    let payload: &[u8] = b"VFS-2 FAT write-path witness bytes\n";
    let _ = mt.unlink(path, KERNEL_PRINCIPAL); // clear stale scratch from an interrupted run

    if let Err(e) = mt.create(path, NodeKind::File, KERNEL_PRINCIPAL) {
        serial_println!(":: VFS2-fat: create {} failed {:?} :: FAIL ::", path, e);
        return;
    }
    let wrote = match mt.write(path, 0, payload, KERNEL_PRINCIPAL) {
        Ok(n) => n,
        Err(e) => {
            serial_println!(":: VFS2-fat: write {} failed {:?} :: FAIL ::", path, e);
            let _ = mt.unlink(path, KERNEL_PRINCIPAL);
            return;
        }
    };
    let back = match mt.read(path, 0, payload.len()) {
        Ok(b) => b,
        Err(e) => {
            serial_println!(":: VFS2-fat: readback {} failed {:?} :: FAIL ::", path, e);
            let _ = mt.unlink(path, KERNEL_PRINCIPAL);
            return;
        }
    };
    let ok = wrote == payload.len() && back == payload;
    let got = checksum(&back);
    let want = checksum(payload);
    let _ = mt.unlink(path, KERNEL_PRINCIPAL); // self-clean, whatever the verdict

    if ok && got == want {
        serial_println!(
            ":: VFS2: write test — created {}, wrote {}, readback OK :: PASS ::",
            path, wrote
        );
    } else {
        serial_println!(
            ":: VFS2-fat: readback mismatch on {} (wrote {}, sum {:#x} want {:#x}) :: FAIL ::",
            path, wrote, got, want
        );
    }
}

/// VFS-2 native write witness: create → write → read-back → checksum on the native UnaFS
/// volume, routed through the `MountTable`/`NativeBackend` write surface. Self-cleaning; honest
/// skip on media without a unafs partition.
#[cfg(target_arch = "aarch64")]
pub fn vfs2_native_write_witness() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::fs::unafs::locate().is_err() {
        serial_println!(":: VFS2-native: no unafs volume — skipped ::");
        return;
    }
    let mut mt = MountTable::new();
    mt.mount("/", Box::new(NativeBackend::new("native")));

    let path = "/vfs2ntv.txt";
    let payload: &[u8] = b"VFS-2 native write-path witness bytes\n";
    let _ = mt.unlink(path, KERNEL_PRINCIPAL); // clear stale scratch

    if let Err(e) = mt.create(path, NodeKind::File, KERNEL_PRINCIPAL) {
        serial_println!(":: VFS2-native: create {} failed {:?} :: FAIL ::", path, e);
        return;
    }
    let wrote = match mt.write(path, 0, payload, KERNEL_PRINCIPAL) {
        Ok(n) => n,
        Err(e) => {
            serial_println!(":: VFS2-native: write {} failed {:?} :: FAIL ::", path, e);
            let _ = mt.unlink(path, KERNEL_PRINCIPAL);
            return;
        }
    };
    let back = match mt.read(path, 0, payload.len()) {
        Ok(b) => b,
        Err(e) => {
            serial_println!(":: VFS2-native: readback {} failed {:?} :: FAIL ::", path, e);
            let _ = mt.unlink(path, KERNEL_PRINCIPAL);
            return;
        }
    };
    let ok = wrote == payload.len() && back == payload;
    let got = checksum(&back);
    let want = checksum(payload);
    let _ = mt.unlink(path, KERNEL_PRINCIPAL); // self-clean

    if ok && got == want {
        serial_println!(
            ":: VFS2: write test — created {}, wrote {}, readback OK :: PASS ::",
            path, wrote
        );
    } else {
        serial_println!(
            ":: VFS2-native: readback mismatch on {} (wrote {}, sum {:#x} want {:#x}) :: FAIL ::",
            path, wrote, got, want
        );
    }
}

/// A trivial additive byte checksum for the witnesses' read-back verification.
#[cfg(target_arch = "aarch64")]
fn checksum(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0u32, |a, &b| a.wrapping_add(b as u32))
}

// =========================================================================================
// VFS-3 USB-mount witness — proves the hot-plugged USB FAT stick lives in the VFS namespace
// (at `/usb`) ALONGSIDE the SD boot FAT (at `/fat`), each routing to its own block device, and
// that the USB volume is WRITABLE through the table (USB-WRITE cleared the old read-only guard:
// a `create` at `/usb/...` now lands a real entry rather than being refused).
//
// This is a METAL proof, honest-skip under QEMU: the USB stick is reached through the xHCI
// `Usb` source (PIUSB-27), which needs the BCM2711 PCIe RC + VL805 xHCI. QEMU raspi4b models no
// PCIe RC and attaches no usb-storage, so `mount_source(Usb)` finds no device and the witness
// prints the skip line there. The positive PASS line is the attended-metal evidence, the same
// posture the whole piusb line already carries ("attended-metal for positive verify").
// =========================================================================================

/// VFS-3 USB-mount witness: build a `MountTable` carrying the SD boot FAT at `/fat` and the USB
/// FAT stick at `/usb` (each on its own block source), then prove through the table that (a) the
/// USB volume is reachable — its root lists and a file reads back — and (b) the USB volume is
/// now WRITABLE: a `create` at `/usb/...` succeeds through the table (USB-WRITE made
/// `FatBackend::read_only()` false — the `Usb` source carries a verified BOT WRITE(10) path), so
/// the mount no longer refuses writes with `Unsupported`. Honest-skip when no USB volume is
/// present (always, under QEMU raspi4b).
#[cfg(target_arch = "aarch64")]
pub fn vfs3_usb_mount_witness() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // Presence check at MountTable build time (doc §6 hot-mount): only bind /usb when the stick
    // is actually enumerated. Absent -> honest skip, no panic.
    if crate::fs::fat::mount_source(crate::fs::fat::BlockSource::Usb).is_err() {
        serial_println!(":: VFS3: usb-mount — no USB volume — skipped ::");
        return;
    }
    let mut mt = MountTable::new();
    // Both FAT volumes in ONE namespace, each on its own device — the SHELL-WRITE flag's core
    // concern (before VFS-3 the FatBackend could only ever reach the Default/boot device).
    mt.mount("/fat", Box::new(FatBackend::new("fat", KERNEL_PRINCIPAL, true)));
    mt.mount("/usb", Box::new(FatBackend::new_usb("usb", KERNEL_PRINCIPAL)));

    // (a) the USB root lists through the table.
    let entries = match mt.read_dir("/usb") {
        Ok(e) => e,
        Err(e) => {
            serial_println!(":: VFS3: usb-mount — read_dir /usb failed {:?} :: FAIL ::", e);
            return;
        }
    };
    // (b) read a real file back through the table (the first regular file in the root, if any).
    let mut read_bytes = 0usize;
    if let Some(f) = entries.iter().find(|e| matches!(e.kind, NodeKind::File)) {
        let path = alloc::format!("/usb/{}", f.name);
        match mt.read(&path, 0, 64) {
            Ok(b) => read_bytes = b.len(),
            Err(e) => {
                serial_println!(":: VFS3: usb-mount — read {} failed {:?} :: FAIL ::", path, e);
                return;
            }
        }
    }
    // (c) the USB volume is now WRITABLE through the table: a `create` at `/usb/...` SUCCEEDS
    // (USB-WRITE cleared the by-construction read-only guard — the `Usb` source has a verified
    // BOT WRITE(10) path), landing a real directory entry rather than being refused with
    // `Unsupported`. A failure here means the write path regressed, not that a guard held.
    let create_ok = match mt.create("/usb/VFS3W.TXT", NodeKind::File, KERNEL_PRINCIPAL) {
        Ok(_) => true,
        Err(e) => {
            serial_println!(
                ":: VFS3: usb-mount — writable-mount check failed (create /usb returned {:?}) :: FAIL ::",
                e
            );
            return;
        }
    };
    // (d) the two mounts are independent: /fat still resolves to the Default-source backend and
    // its root lists (coexistence — /usb did not displace the boot FAT).
    let fat_ok = mt.read_dir("/fat").is_ok();

    serial_println!(
        ":: VFS3: usb-mount test — /usb root {} entries, read {} bytes, create-ok={}, /fat coexists={} :: PASS ::",
        entries.len(),
        read_bytes,
        create_ok,
        fat_ok
    );
}
