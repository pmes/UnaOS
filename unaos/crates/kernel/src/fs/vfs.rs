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

/// A wall-clock stamp as the VFS presents it — the fields a listing renders, and
/// nothing else. VFS-owned on purpose: the FAT backend decodes its on-disk
/// `DirEntry::mtime()` into this, so no FAT type crosses the trait boundary, and a
/// backend whose medium carries no timestamp (native UnaFS) answers `None` rather
/// than fabricating one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VfsTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub min: u8,
    pub sec: u8,
}

/// One directory entry as the VFS presents it — a display name, a kind, and the
/// two listing facts a caller would otherwise have to go BEHIND the VFS to get.
/// Backend-specific identity (FAT first-cluster, unafs inode id) is NOT exposed;
/// the VFS is name-addressed, and re-resolution from the name is the contract
/// (the same discipline the shell's FAT path cache already follows).
///
/// VFS-1 (adoption): `size`/`mtime` were added because the shell's `ls` could not
/// be routed through the mount table without them — it would have had to keep its
/// per-volume `pi_usb_ls_collect` / `pi_ls_collect` collectors purely to recover a
/// size column, which is exactly the per-verb dispatch this layer exists to
/// delete. `mtime` is `None` where the medium carries no stamp (native UnaFS),
/// which the renderer shows as a dash rather than a fabricated date.
///
/// The `.`/`..` pseudo-entries are NOT presented: the mount table resolves paths
/// lexically (`normalize_path` collapses `.` and pops `..` before a backend is
/// ever consulted), so a dot entry in a listing is a FAT on-disk artifact with no
/// meaning in this namespace. Both real backends filter them, and both of the
/// shell collectors this arc deleted filtered them too — the behaviour is
/// preserved, just moved to the one place that owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEnt {
    pub name: String,
    pub kind: NodeKind,
    pub size: u64,
    pub mtime: Option<VfsTime>,
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

    /// LAYOUT (orin 18): the VOLUME-relative directory this mount is rooted at, or `""` when the
    /// mount IS the volume root.
    ///
    /// **This is the fact that makes a `rel` MOUNT-relative rather than volume-relative, and it is
    /// on the trait because the one caller that holds two mounts at once — [`MountTable::rename`] —
    /// cannot be correct without it.** Before LAYOUT every mount was rooted at the volume root, so
    /// the two spaces coincided and a remainder produced by one mount was a valid address on any
    /// other mount of the same volume. A rooted mount breaks exactly that: `/boot` and `/apps` can
    /// be ONE VOLUME and still be TWO ADDRESS SPACES.
    ///
    /// The default is `""` — a backend with no notion of a sub-root (native UnaFS, the witness
    /// mock) is addressed in its volume's own space and overrides nothing, so this places no
    /// obligation on any implementor.
    fn mount_root(&self) -> &str {
        ""
    }

    /// LAYOUT: the VOLUME-relative path that the mount-relative `rel` names on the medium —
    /// [`mount_root`](VfsBackend::mount_root) followed by `rel`'s components.
    ///
    /// The SINGLE definition of mount-space → volume-space: the FAT walk and
    /// [`MountTable::rename`]'s cross-mount translation both go through it, so the two cannot
    /// drift apart. The volume root is `""` and prefixes nothing, which makes every pre-LAYOUT
    /// mount byte-for-byte the old walk.
    fn on_volume(&self, rel: &str) -> String {
        let root = self.mount_root();
        let mut s = String::with_capacity(root.len() + rel.len() + 1);
        s.push_str(root);
        for c in components(rel) {
            s.push('/');
            s.push_str(c);
        }
        s
    }

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

    // --- VFSROUTE (orin 17) ----------------------------------------------------
    //
    // The six operations a SHELL FILE VERB needs that VFS-2 did not shape, added
    // because the verbs that needed them were reaching around the trait to get
    // them: `mv` called `fat::rename_entry`/`move_entry` and `unafs::rename`
    // directly, `rmdir`/`rm -r` called `fat::remove_dir`, `setfattr` called
    // `unafs::remove_attribute`, and every write verb re-derived the read-only
    // question from `fat::BlockSource::write_veto` at the call site. Each is a
    // question about A VOLUME, so each belongs to the volume's backend.
    //
    // All six default to a refusal, so a backend that cannot do one says so in
    // the type system and the verb PRINTS that refusal — the rule this arc is
    // built on: a backend that cannot perform an operation returns a typed error,
    // never a silent fall-through to some other filesystem.

    /// Why this volume refuses ordinary file mutation, or `None` if it accepts it.
    ///
    /// The trait-level twin of [`crate::fs::fat::BlockSource::write_veto`], hoisted so a caller can
    /// ask BEFORE it starts a multi-step verb (`write`'s delete-then-recreate, `mv`'s relink) and
    /// get a whole answer instead of a half-finished mutation and an opaque I/O error several
    /// sectors in. The default is a refusal: a backend exposes a write surface by opting in.
    fn write_veto(&self) -> Option<&'static str> {
        Some("this volume exposes no write surface")
    }

    /// Rename or move the node at `from_rel` to `to_rel` WITHIN this volume (both
    /// paths are already volume-relative, so a cross-volume move never reaches a
    /// backend — the mount table refuses it). An existing `to_rel` is refused with
    /// [`VfsError::Backend`]`("exists")`, the spelling [`create`](VfsBackend::create)
    /// already uses. Implementors authorize the write FIRST.
    fn rename(&self, _from_rel: &str, _to_rel: &str, _principal: &str) -> Result<(), VfsError> {
        Err(VfsError::Unsupported)
    }

    /// Remove the EMPTY directory at `rel`. A file is [`VfsError::NotADirectory`];
    /// a non-empty directory is [`VfsError::Backend`]`("not-empty")`; the volume
    /// root is never removable ([`VfsError::IsADirectory`]). Implementors
    /// authorize the write FIRST.
    ///
    /// The native UnaFS backend deliberately does NOT implement this: the crate
    /// has no directory removal at all (`unlink` returns `IsADirectory`
    /// unconditionally), so it inherits the default and `rmdir /SOMEDIR` on the
    /// native volume prints an honest `-ENOTSUP` instead of silently deleting
    /// something on the FAT volume — which is exactly what the pre-VFSROUTE verb
    /// did.
    fn remove_dir(&self, _rel: &str, _principal: &str) -> Result<(), VfsError> {
        Err(VfsError::Unsupported)
    }

    /// Drop one typed attribute (`key`) from the object at `rel`. FAT carries no
    /// typed attributes, so the FAT backend inherits the default refusal and
    /// `setfattr -x k /boot/F` says so rather than pretending to succeed.
    fn remove_attr(&self, _rel: &str, _key: &str, _principal: &str) -> Result<(), VfsError> {
        Err(VfsError::Unsupported)
    }

    /// The volume's total capacity in bytes, when the medium publishes one.
    /// `None` is an honest "this backend does not know", which `df` renders as a
    /// dash — never a fabricated figure.
    fn volume_bytes(&self) -> Option<u64> {
        None
    }

    /// One line of the volume's own geometry/identity, for the `mount` listing —
    /// the backend describing ITSELF, which is the only layer that can. `None`
    /// means the backend publishes no description and the listing prints the
    /// prefix + name + access it already knows.
    fn describe(&self) -> Option<String> {
        None
    }

    // --- VOLID (orin 18) -------------------------------------------------------

    /// This volume's IDENTITY — the answer to "are these two mounts the same storage?",
    /// which is the only question [`MountTable::same_volume`] and [`MountTable::rename`]
    /// ever wanted.
    ///
    /// **Deliberately NOT the volume name.** A name is an argument the mount site typed;
    /// identity is a fact about the medium. One machine binds ONE volume at SEVERAL
    /// prefixes — BOOTROOT's `shell::vfs_mount_table` binds the disk the kernel was found
    /// on at `/`, `/boot` and `/apps` — and the retired ROOTFS bind that first exposed this
    /// typed the same card `"card"` at `/` and `"fat"` at `/boot`. A name comparison calls
    /// one medium two volumes (and, worse, two media one volume),
    /// refusing `mv /A.TXT /boot/B.TXT` as "cross-volume" when it is a plain in-volume
    /// relink (rmbp 15, condition C1). That is the inverse of the pointer comparison the
    /// name replaced, arriving through the other door: a pointer is too FINE (two adapters
    /// over one medium split), and a name is wrong in BOTH directions (one medium may be
    /// typed two names; two media may be typed one). The pointer test survives as a FLOOR
    /// inside [`same_storage`] — never as the answer — because it is at least never wrong
    /// when it says YES.
    ///
    /// A backend answers with something ALIASING CANNOT BREAK: whatever it actually reads
    /// through, and WHICH FILESYSTEM it reads it as.
    ///
    /// # IDENTITY IS OVER (DEVICE, FILESYSTEM) — NEVER THE DEVICE ALONE
    ///
    /// The device alone is not identity, and the case that proves it is the Pi: it mounts
    /// the native UnaFS volume at `/` and the FAT program volume at `/boot`, and BOTH live on
    /// the one physical card — `drivers/emmc2.rs` sizes the UnaFS volume from the same card
    /// that [`crate::fs::fat::BlockSource::Default`] reads. An id derived from the block
    /// source alone would call those two mounts one volume and admit `mv /X.TXT /boot/X.TXT`,
    /// relinking an UnaFS inode into a FAT directory — the C1 failure reproduced one layer
    /// further down. So every implementor mixes a DOMAIN TAG naming its filesystem BEFORE it
    /// mixes anything about the medium, and two different filesystems are unequal by
    /// construction whatever they sit on. [`volid_mix`] and [`VOLID_SEED`] are the shared
    /// derivation.
    ///
    /// # WHY IT IS REQUIRED, AND WHY IT RETURNS `Option`
    ///
    /// **No default body.** For a MUTATING method a default of `Err(Unsupported)` is the
    /// safe direction, and this trait takes it six times above. For an IDENTITY the
    /// instinct inverts: a constant default would make every backend that did not override
    /// it the SAME volume as every other — the corrupting direction, and the one answer a
    /// rename must never get wrong. A default derived from the name is no better, since
    /// that comparison IS C1. So the compiler asks each implementor the question instead.
    ///
    /// `None` is the honest "I cannot establish this volume's identity", and it means
    /// **NEVER EQUAL TO ANYTHING, INCLUDING ANOTHER `None`** — see [`same_storage`]. It is
    /// therefore always safe to answer: refusing a legal move is an inconvenience,
    /// performing an illegal one relinks a directory entry to a name that means nothing on
    /// its volume. `Some(id)` is a CLAIM, and equal ids must mean the same bytes.
    ///
    /// A backend still compares equal to ITSELF when it answers `None`: [`same_storage`]
    /// settles object identity first, so `mv /A.TXT /B.TXT` inside one mount never turns
    /// into a spurious cross-volume refusal because the medium was unreadable that instant.
    fn volume_id(&self) -> Option<u64>;
}

/// VOLID (orin 18): are these two mounts THE SAME STORAGE? The one place the identity
/// contract is interpreted — [`MountTable::same_volume`] (what `mv` prints) and
/// [`MountTable::rename`] (what `mv` enforces) both call it, so the two cannot drift.
///
/// Two rules, in order:
///
/// 1. **The same backend object is the same storage**, unconditionally. This is the old
///    pointer comparison kept as a FLOOR rather than as the rule: a pointer test is too
///    fine to BE the answer (two adapters over one medium are one volume and compare
///    unequal), but it is never wrong in the TRUE direction — one object reads one medium.
///    It makes the relation reflexive, and it is what keeps a backend that answers `None`
///    usable within its own mount.
/// 2. Otherwise `Some(a) == Some(b)`. `None` on either side is NOT equal — including
///    `None` vs `None`, which is two backends that each said "I cannot tell", the weakest
///    possible ground on which to relink a directory entry.
pub fn same_storage(a: &dyn VfsBackend, b: &dyn VfsBackend) -> bool {
    if core::ptr::addr_eq(a as *const dyn VfsBackend, b as *const dyn VfsBackend) {
        return true;
    }
    match (a.volume_id(), b.volume_id()) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// VOLID (orin 18): FNV-1a (64-bit) — the whole of the id derivation. No allocation, no
/// state, `const`-evaluable, and total over any byte string, so a backend can mix its
/// domain tag and its own identity bytes without pulling in a hasher.
pub const fn volid_mix(mut h: u64, bytes: &[u8]) -> u64 {
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

/// VOLID: the FNV-1a offset basis — the seed every domain tag starts from.
pub const VOLID_SEED: u64 = 0xcbf2_9ce4_8422_2325;

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
        crate::fs::perf_op("list", path, || b.read_dir(rel)) // FATFIX M2: measured, not guessed
    }

    pub fn stat(&self, path: &str) -> Result<Stat, VfsError> {
        let (b, rel) = self.resolve(path)?;
        b.stat(rel)
    }

    pub fn read(&self, path: &str, offset: u64, len: usize) -> Result<Vec<u8>, VfsError> {
        let (b, rel) = self.resolve(path)?;
        crate::fs::perf_op("read", path, || b.read(rel, offset, len)) // FATFIX M2: the launch delay
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

    // --- VFSROUTE (orin 17): the resolve-then-dispatch surface the shell verbs ask ---

    /// The name of the volume that claims `path` (`"native"`, `"fat"`, `"usb"`, …).
    /// A verb uses it to say WHICH volume answered without knowing what kind of
    /// filesystem that volume is.
    pub fn volume_name(&self, path: &str) -> Result<String, VfsError> {
        // Owned, not borrowed: `resolve` ties the returned reference's lifetime to the PATH's (they
        // share one lifetime parameter so the volume-relative remainder can borrow from it), so a
        // `&str` here would outlive nothing useful. One small allocation per call, at a call site
        // that is about to format a line anyway.
        Ok(self.resolve(path)?.0.volume_name().to_string())
    }

    /// Why the volume claiming `path` refuses mutation, or `None` if it accepts it.
    pub fn write_veto(&self, path: &str) -> Result<Option<&'static str>, VfsError> {
        Ok(self.resolve(path)?.0.write_veto())
    }

    /// Rename or move `from` to `to`. **A cross-volume move is refused here**, in
    /// the ONE place that can see both ends: a backend is handed volume-relative
    /// paths and could not tell that the other end lives on a different volume, so
    /// letting the call through would relink an entry on one volume to a name that
    /// means nothing there. The refusal is [`VfsError::Unsupported`] — the caller
    /// is authorized; the operation has no sound implementation across volumes
    /// (copy-then-delete is a different operation and the operator asks for it by
    /// typing `cp` then `rm`).
    pub fn rename(&self, from: &str, to: &str, principal: &str) -> Result<(), VfsError> {
        let (bf, relf) = self.resolve(from)?;
        let (bt, relt) = self.resolve(to)?;
        if !same_storage(bf, bt) {
            return Err(VfsError::Unsupported);
        }
        // ONE VOLUME IS NOT ONE ADDRESS SPACE (LAYOUT, orin 18 — rmbp 15's finding B66).
        //
        // This block used to rest on the sentence "both remainders are VOLUME-ROOT-relative by
        // construction (each mount strips its own prefix)" and hand `relt` straight to `bf`. LAYOUT
        // falsified that sentence: [`VfsBackend::mount_root`] lets a mount be rooted at a
        // DIRECTORY, so a remainder is mount-relative, and `/boot` (root `""`) and `/apps`
        // (root `/APPS`) are one volume addressed two ways. Passing the destination's remainder to
        // the source's backend then produced a SILENT WRONG LOCATION that reported success:
        // `mv /boot/A.TXT /apps/B.TXT` wrote `B.TXT` to the volume root (never inside `APPS/`) and
        // `mv /apps/X.ELF /boot/Y.ELF` renamed `APPS/X.ELF` to `APPS/Y.ELF` (the program never left
        // `/apps`). Both ends exist afterwards, so a presence-only check passes on the bug — which
        // is why `layout.mv` asserts ABSENCE from the source as well.
        //
        // The fix is to put both ends in ONE space before the backend is called. Each remainder is
        // lifted to the volume through its OWN mount (`on_volume`), and the pair is then expressed
        // inside whichever of the two mounts can address both: the source's if it can reach the
        // destination (`/boot` → `/apps`), otherwise the destination's if it can reach the source
        // (`/apps` → `/boot`). Both mounts name one volume, so either is a sound executor, and which
        // one executes says NOTHING about who may write: both postures are asked either way, below.
        //
        // Neither can address both (two sibling rooted mounts, `/apps` and a future `/lib`) → the
        // move is genuinely cross-space and is REFUSED BY NAME. Refusing is never corrupting;
        // silently relinking into the wrong directory is.
        //
        // Two shapes were rejected. Refusing whenever the two roots differ is a capability
        // regression — `mv /boot/X /apps/Y` is a legitimate same-volume move that worked before
        // LAYOUT. Making the two-argument backend ops take volume-absolute paths is the structural
        // answer, and it changes the `VfsBackend` contract for every implementor; that is not a
        // change to make under a boot deadline.
        let src_abs = bf.on_volume(relf);
        let dst_abs = bt.on_volume(relt);
        let (b, rf, rt) = if let (Some(rf), Some(rt)) = (
            under_mount_root(bf.mount_root(), &src_abs),
            under_mount_root(bf.mount_root(), &dst_abs),
        ) {
            (bf, rf, rt)
        } else if let (Some(rf), Some(rt)) = (
            under_mount_root(bt.mount_root(), &src_abs),
            under_mount_root(bt.mount_root(), &dst_abs),
        ) {
            (bt, rf, rt)
        } else {
            return Err(VfsError::Backend("cross-mount-root"));
        };
        // TWO MOUNTS, TWO DIFFERENT QUESTIONS — not one question asked twice (rmbp 15's B67, and
        // their withdrawal of the shape that preceded it; orin 19 holds the falsifier).
        //
        // A mount is a POSTURE as much as an address space, so a move that crosses two mounts of one
        // volume must satisfy both. The symmetric-LOOKING form — `bt.authorize_write(relt)` — was
        // implemented and MEASURED RED (`exec-orin18-aclsym`, `MBENCH FAIL 118/119`,
        // `mv: /RELIC3.TXT: -ENOENT`): a rename's DESTINATION DOES NOT EXIST YET, so authorizing the
        // destination PATH asks a backend about an object that is not there. It is backend-specific
        // besides — `FatBackend::authorize_write` ignores its `rel` (posture only), so the call was a
        // no-op on FAT, while `NativeBackend::authorize_write` resolves the path and maps the miss to
        // `NoSuchPath`. That red is kept as the falsifier this shape had to survive.
        //
        // The two questions, stated apart:
        //   - the SOURCE mount is asked about the object that LEAVES it — which exists;
        //   - the DESTINATION mount is asked about the directory that RECEIVES the leaf — because the
        //     leaf does not exist yet. That is already this file's own convention: `NativeBackend::
        //     create` authorizes against the PARENT with the reason written there verbatim.
        //
        // Asked UNCONDITIONALLY, in both directions. Gating them on which mount executes is what let
        // the hole open in the first place, and the executor is an implementation detail of where the
        // pair can be addressed — never a statement about who may write.
        //
        // ⚠ WHAT THE SECOND CALL DOES *TODAY*, stated so a reader does not credit it with more
        // (rmbp 15's B68, measured, and it corrects this commit's own first claim). This function has
        // exactly ONE caller — `shell.rs`'s `mv` — and it passes `SHELL_PRINCIPAL`, which IS
        // `KERNEL_PRINCIPAL`: the shell is the console of the machine, not a tenant. Both authorizers
        // short-circuit on that principal, so for every invocation REACHABLE TODAY the destination
        // call reduces to the existence check `NativeBackend::authorize_write` runs before its ACL.
        // It is a destination-parent EXISTENCE check now and an ACL check the day a non-kernel
        // principal reaches this surface. Both halves are worth having; only one of them can fire,
        // and no fixture driven through the shell verb can exercise the other — a green run through
        // `mv` would prove nothing about the ACL. A witness has to call this seam directly with a
        // non-kernel principal.
        //
        // ⚠ AND IT CAN REFUSE FOR A REASON THAT IS NOT AN ACL (pi 8): `native_write_authz` reads the
        // inode BEFORE it short-circuits on the kernel principal, and a read miss returns `Denied` —
        // so a destination parent that resolves to an id that will not read is INDISTINGUISHABLE
        // from an ACL refusal, kernel principal or not. The root case is the one that would bite
        // (`receiving_dir` yields `""` for a leaf at the mount root, which `native_abs` renders `/`),
        // and it is MEASURED GREEN rather than argued: on the Pi `kernel8-test` leg `/` IS the native
        // UnaFS volume (`:: ls1: /: K3HELLO.TXT K3PAT.BIN apps/ boot/ ::`), `shell.relics.mv` moves a
        // file whose DESTINATION is at that root, and it passes — while `c8eb4038`, which asked about
        // the destination LEAF instead of its parent, is exactly where that same path returned
        // `-ENOENT`. The resolution half is live and proven on the leg that convicted the other shape.
        //
        // ⚠ FAT's new refusal point (rmbp 15): `FatBackend::authorize_write` ignores its `rel` but
        // consults `read_only()` FIRST, so this call refuses a move INTO a read-only mount whose
        // SOURCE is writable. Correct, and unreachable today only because `/boot` and `/apps` are one
        // volume with one posture — which is the same unstated "both mounts share a posture"
        // invariant that produced this defect, now load-bearing in one more place.
        bf.authorize_write(relf, principal)?;
        bt.authorize_write(&receiving_dir(relt), principal)?;
        b.rename(rf, rt, principal)
    }

    /// Do `from` and `to` land on the SAME volume? The question `mv` asks before it decides between
    /// a rename and an honest cross-volume refusal.
    ///
    /// **Compared by [`VfsBackend::volume_id`] — the volume's STORAGE identity — and the difference
    /// is load-bearing (VOLID, orin 18, rmbp 15 condition C1).** A machine may bind ONE volume at
    /// several prefixes, and may name them differently: BOOTROOT binds the found boot disk at `/`,
    /// `/boot` and `/apps`, and the retired ROOTFS bind that exposed this defect typed one Orin card
    /// `"card"` at `/` and `"fat"` at `/boot`. This compared the two NAMES until VOLID and answered
    /// `false` for one physical card, refusing `mv /A.TXT /boot/B.TXT` on exactly the configuration
    /// the render9 boot disk flies. A pointer comparison (the shape before that) fails the same way;
    /// the name merely moved the failure one door over. Identity is a fact about the medium, so the
    /// backend answers it off what it reads through, not off what its mount was called.
    ///
    /// It still errs CONSERVATIVELY: a backend that cannot establish its storage identity answers
    /// `None`, the two mounts read as different volumes, and the rename is refused (`cp` then `rm`
    /// still works). Refusing is never corrupting; the reverse is. [`same_storage`] holds the whole
    /// rule, and [`MountTable::rename`] enforces the same predicate this one reports.
    pub fn same_volume(&self, from: &str, to: &str) -> Result<bool, VfsError> {
        let (bf, _) = self.resolve(from)?;
        let (bt, _) = self.resolve(to)?;
        Ok(same_storage(bf, bt))
    }

    /// VOLID: the storage identity of the volume claiming `path` — the value [`same_storage`]
    /// compares. Exposed so a transcript can PRINT the two ids it is asserting over instead of only
    /// their equality; `None` is the backend's honest "I cannot establish it", never a zero.
    pub fn volume_id(&self, path: &str) -> Result<Option<u64>, VfsError> {
        Ok(self.resolve(path)?.0.volume_id())
    }

    pub fn remove_dir(&self, path: &str, principal: &str) -> Result<(), VfsError> {
        let (b, rel) = self.resolve(path)?;
        b.remove_dir(rel, principal)
    }

    pub fn remove_attr(&self, path: &str, key: &str, principal: &str) -> Result<(), VfsError> {
        let (b, rel) = self.resolve(path)?;
        b.remove_attr(rel, key, principal)
    }

    /// One row per mount, for the `mount`/`df` listing: `(prefix, volume name,
    /// write veto, capacity, description)`. The verb renders these five facts and
    /// knows nothing else about any volume — which is the whole point.
    #[allow(clippy::type_complexity)]
    pub fn rows(&self) -> Vec<(&str, &str, Option<&'static str>, Option<u64>, Option<String>)> {
        self.mounts
            .iter()
            .map(|m| {
                (
                    m.prefix.as_str(),
                    m.backend.volume_name(),
                    m.backend.write_veto(),
                    m.backend.volume_bytes(),
                    m.backend.describe(),
                )
            })
            .collect()
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

/// LAYOUT (orin 18): express the VOLUME-relative `abs` as a remainder inside a mount rooted at
/// `root`, or `None` when `abs` lies OUTSIDE that mount's address space. The inverse of
/// [`VfsBackend::on_volume`], and the second half of [`MountTable::rename`]'s translation.
///
/// The root is matched case-insensitively, like every other FAT lookup here (`/APPS` and `/apps`
/// name one directory), and at a PATH BOUNDARY — a mount rooted at `/APPS` claims `/APPS` and
/// `/APPS/…` but never `/APPSTORE/…`, the same boundary rule [`prefix_claims`] applies to namespace
/// prefixes. `abs` equal to the root is the mount point itself and yields `""`, which every write
/// verb already refuses as [`VfsError::IsADirectory`].
fn under_mount_root<'a>(root: &str, abs: &'a str) -> Option<&'a str> {
    if root.is_empty() {
        return Some(abs); // the volume root addresses the whole volume
    }
    if abs.len() < root.len()
        || !abs.is_char_boundary(root.len())
        || !abs[..root.len()].eq_ignore_ascii_case(root)
    {
        return None;
    }
    match abs.as_bytes().get(root.len()) {
        None => Some(""),                       // `abs` IS the mount point
        Some(b'/') => Some(&abs[root.len()..]), // a name beneath it
        Some(_) => None,                        // `/APPSTORE` merely starts with `/APPS`
    }
}

/// The mount-relative DIRECTORY that will RECEIVE the leaf named by `rel`: `rel` minus its last
/// component, and `""` — the mount point itself — when the leaf sits directly under the mount root.
///
/// **The destination side of [`MountTable::rename`]'s ACL question, and it is a DIFFERENT question
/// from the source side's.** The source names an object that exists and can be authorized directly;
/// the destination names one that does not exist yet, so the mount is asked about the directory it
/// will be planted in. This is not a new convention: `NativeBackend::create` authorizes against the
/// parent for exactly this reason, written there verbatim — the leaf does not exist yet. (Plain
/// backticks, not an intra-doc link: that type is `cfg(target_arch = "aarch64")` and the link would
/// dangle on the x86 build of this same file.)
///
/// The output form is [`VfsBackend::on_volume`]'s (`/`-led components, empty for the root), which
/// `native_abs` renders as `/` and the FAT adapter ignores entirely.
///
/// ACLSYM (orin 19): `pub(crate)` so the witness can assert this mapping as a VALUE. `crate::shell`'s
/// `vfs.aclsym.dir` leg pins all four shapes — `/APPS/X.ELF` → `/APPS`, `/X.TXT` → `""`, `""` → `""`,
/// `/A/B/C` → `/A/B` — which is what stops the "the leaf does not exist yet" decision above from
/// rotting into a silent identity function. It is pure and touches no medium, so that leg has no
/// skip branch on any board the TSTE battery reaches (the Pi bare-metal gate and the x86 gate —
/// measured; `./arroyo test-arm` reaches the battery on neither arc's watch).
pub(crate) fn receiving_dir(rel: &str) -> String {
    let mut comps: Vec<&str> = components(rel).collect();
    comps.pop();
    let mut s = String::new();
    for c in comps {
        s.push('/');
        s.push_str(c);
    }
    s
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
/// at `/boot`) and the hot-plugged USB stick ([`Usb`](crate::fs::fat::BlockSource::Usb),
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
    /// LAYOUT (orin 18): the volume-relative DIRECTORY this mount exposes as its root — `""` for
    /// the volume root (every mount before this arc), `"/APPS"` for the `/apps` program mount,
    /// which is the ESP's `APPS/` directory bound at its own prefix. Every `rel` the trait hands
    /// this backend is prefixed with it before the FAT walk (see [`VfsBackend::on_volume`], which
    /// reads it through [`VfsBackend::mount_root`]), so a consumer of `/apps/VUG.ELF` reaches
    /// `APPS/VUG.ELF` on the medium and can never reach above the directory. Set only by
    /// [`FatBackend::rooted`].
    root: String,
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
            root: String::new(),
        }
    }

    /// VFS-3: mount the hot-plugged USB FAT stick into the VFS, read through the
    /// xHCI [`Usb`](crate::fs::fat::BlockSource::Usb) source — the mount `ls /usb`
    /// and the `/fs/usb` HTTP route used. World-readable (its
    /// contents are meant to be read) and **writable** since USB-WRITE: the
    /// write verbs route to the verified BOT WRITE(10) path (`write_block_usb`,
    /// MISSION RMW+restore witnessed), which superseded the PIUSB-27 guard.
    ///
    /// ⚠ **HOMESOIL (orin 22): `vfs_mount_table` no longer calls this.** `/usb` is now one instance
    /// of the general home-soil rule — every enumerated non-root disk gets an indexed, bus-named
    /// point with its SOURCE's own posture — so `fs::bootdisk::bind` builds it through
    /// [`FatBackend::new_source`], which is this constructor with the source spelled out instead of
    /// baked in. The posture is identical (`Usb`'s `write_veto` is `None` either way), which is
    /// what made the fold safe on the Pi. Kept, not deleted: it is the honest one-argument spelling
    /// for a caller that means the stick and nothing else.
    pub fn new_usb(volume: &str, principal: &str) -> Self {
        Self {
            volume: volume.to_string(),
            principal: principal.to_string(),
            world_readable: true,
            source: crate::fs::fat::BlockSource::Usb,
            root: String::new(),
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
    pub(crate) fn read_only(&self) -> bool { // HOMESOIL (orin 22): `pub(crate)` on the SAME LINE (no line moves) so `fs::bootdisk` can print `rw=` from the SAME sample that built the mount, instead of re-deriving the posture at a second site — which is the drift `write_veto` was made the one definition to end.
        self.source.write_veto().is_some()
    }

    /// LAYOUT (orin 18): bind this mount's root at `dir` — a volume-relative directory such as
    /// `/APPS` — instead of the volume root. The directory need not exist at mount time (the
    /// table is rebuilt per verb and a missing directory answers `NoSuchPath` on first use, which
    /// is the honest answer for a card staged before this layout). Case-insensitive on the walk,
    /// like every other FAT lookup here, so `/APPS` and `/apps` name the same directory.
    pub fn rooted(mut self, dir: &str) -> Self {
        let mut root = String::new();
        for c in components(dir) {
            root.push('/');
            root.push_str(c);
        }
        self.root = root;
        self
    }

    /// LAYOUT: [`resolve_entry`](Self::resolve_entry) from this mount's root.
    fn entry(&self, fs: &crate::fs::fat::FatFs, rel: &str)
        -> Result<Option<crate::fs::fat::DirEntry>, VfsError> {
        Self::resolve_entry(fs, &self.on_volume(rel))
    }

    /// LAYOUT: [`resolve_parent`](Self::resolve_parent) from this mount's root. The mount point
    /// itself has no leaf from the consumer's point of view whatever the root is, so an empty
    /// `rel` is refused as [`VfsError::IsADirectory`] BEFORE the root is prepended — otherwise a
    /// `create("")` on `/apps` would resolve to "create `APPS` in the volume root".
    fn parent(&self, fs: &crate::fs::fat::FatFs, rel: &str) -> Result<(u32, String), VfsError> {
        if components(rel).next().is_none() {
            return Err(VfsError::IsADirectory);
        }
        Self::resolve_parent(fs, &self.on_volume(rel))
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

    /// VOLID (orin 18): identity = `(vfs:fat:, BLOCK SOURCE, VOLUME FINGERPRINT)` — the
    /// FILESYSTEM tag, the device this adapter reads through, and **the mounted volume's own**
    /// [`fingerprint`](crate::fs::fat::FatFs::volume_fingerprint), `(BS_VolID,
    /// count_of_clusters)`.
    ///
    /// All three terms carry weight:
    ///
    /// * The **tag** is the FILESYSTEM half of `(device, filesystem)`. Without it the Pi's
    ///   `/boot` (FAT on the card) and `/` (UnaFS on the same card) would collide.
    /// * The **source** is what `FatBackend` actually reads through: it carries no volume
    ///   selector at all (`volume`/`principal`/`world_readable` are a label and a posture),
    ///   and every method reaches the medium through `fat::mount_source(self.source)`. Two
    ///   adapters with equal `source` therefore address the same bytes whatever their mounts
    ///   were NAMED — which is exactly the C1 aliasing: BOOTROOT binds the found boot disk at
    ///   `/`, `/boot` and `/apps` over one source, and the retired ROOTFS bind typed one `TegraSd`
    ///   `"card"` at `/` and `"fat"` at `/boot`. `BlockSource::name()` is the
    ///   spelling `SourceCensus` publishes, so no second vocabulary is invented here.
    /// * The **fingerprint** is the volume's own identity, and it is what makes this an
    ///   identity rather than a device address: the serial is fixed at format time and the
    ///   cluster count by the geometry, so a REFORMAT or a swapped card under an unchanged
    ///   handle answers differently. It is the same primitive the aarch64 `UNAFS.ATR` ACL
    ///   store already binds to (`fs/fat.rs`, K1 M2.2), so the VFS and the ACL store agree on
    ///   what "this volume" means.
    ///
    /// **Why NOT `fat::volume_serials(source)`** (the shape reviewed and declined). That
    /// function is a DEVICE-WIDE CENSUS — superfloppy plus up to 128 GPT entries plus 4 MBR
    /// entries — and it is deliberately a SUPERSET, because its consumer is the boot-device
    /// guard, which must recognise a disk by ANY volume on it. Two different partitions on one
    /// device would hand back the same id set and read as one volume: C1 again, one layer
    /// down. `mount_source` is the right call precisely because it returns THE volume this
    /// adapter mounts, by the same first-match-wins rule every other method here uses.
    ///
    /// **`None` when the volume will not mount**, which is honest rather than conservative
    /// theatre: an unreadable medium has no established identity, and [`same_storage`] treats
    /// `None` as equal to nothing. The one case that would otherwise regress —
    /// `mv /A.TXT /B.TXT` inside a single mount whose card blipped — is answered by
    /// `same_storage`'s object-identity floor before this method is consulted, so the operator
    /// gets the real I/O error from the rename instead of a false cross-volume refusal.
    fn volume_id(&self) -> Option<u64> {
        let fs = crate::fs::fat::mount_source(self.source).ok()?;
        let (serial, clusters) = fs.volume_fingerprint();
        let h = volid_mix(VOLID_SEED, b"vfs:fat:");
        let h = volid_mix(h, self.source.name().as_bytes());
        let h = volid_mix(h, &serial.to_le_bytes());
        Some(volid_mix(h, &clusters.to_le_bytes()))
    }

    /// LAYOUT (orin 18) composed with VOLID: the ONE backend that can be rooted below the volume
    /// root — `""` for every mount before LAYOUT, `/APPS` for the program mount. The trait's
    /// default `on_volume` is what the walk below (`entry`/`parent`) and `MountTable::rename`
    /// both read it through.
    ///
    /// **The root is deliberately NOT part of [`volume_id`](FatBackend::volume_id) above, and the
    /// two answers must stay orthogonal.** Identity is a fact about the STORAGE; the root is a
    /// fact about this MOUNT's address space. `/boot` (root `""`) and `/apps` (root `/APPS`) are
    /// ONE volume over one `BlockSource` and must compare EQUAL, or `mv /boot/X /apps/Y` is
    /// refused as cross-volume — C1 all over again, which is the bug VOLID fixed. What makes that
    /// equality safe is the other half of the pair: `MountTable::rename` translates between the
    /// two roots before it calls a backend, so "same volume" no longer implies "same address
    /// space" anywhere that matters (B66). Mixing the root into the id would buy the translation's
    /// safety by reintroducing VOLID's defect, which is the wrong trade in both directions.
    fn mount_root(&self) -> &str {
        &self.root
    }

    fn read_dir(&self, rel: &str) -> Result<Vec<DirEnt>, VfsError> {
        let fs = crate::fs::fat::mount_source(self.source).map_err(fat_err)?;
        let entries = match self.entry(&fs, rel)? {
            None => fs.read_root().map_err(fat_err)?, // volume root
            Some(e) if e.is_dir => fs.read_dir(e.first_cluster()).map_err(fat_err)?,
            Some(_) => return Err(VfsError::NotADirectory),
        };
        Ok(entries
            .into_iter()
            .filter(|e| {
                // The `.`/`..` on-disk artifacts are not names in this namespace (see `DirEnt`).
                let n = e.name();
                n != "." && n != ".."
            })
            .map(|e| {
                let ts = e.mtime();
                DirEnt {
                    name: e.name().to_string(),
                    kind: if e.is_dir { NodeKind::Dir } else { NodeKind::File },
                    size: e.size as u64,
                    // An all-zero on-disk stamp is FAT's "unset", not the year 1980 —
                    // report absence honestly so the renderer dashes it.
                    mtime: if ts.is_zero() {
                        None
                    } else {
                        Some(VfsTime {
                            year: ts.year,
                            month: ts.month,
                            day: ts.day,
                            hour: ts.hour,
                            min: ts.min,
                            sec: ts.sec,
                        })
                    },
                }
            })
            .collect())
    }

    fn stat(&self, rel: &str) -> Result<Stat, VfsError> {
        let fs = crate::fs::fat::mount_source(self.source).map_err(fat_err)?;
        match self.entry(&fs, rel)? {
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
        let entry = self.entry(&fs, rel)?.ok_or(VfsError::IsADirectory)?;
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
        let (parent, leaf) = self.parent(&fs, rel)?;
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
        let (parent, leaf) = self.parent(&fs, rel)?;
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
        let (parent, leaf) = self.parent(&fs, rel)?;
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
        let (parent, leaf) = self.parent(&fs, rel)?;
        let (de, dir_lba, dir_off) = fs.locate_in_dir(parent, &leaf).map_err(fat_err)?;
        if de.is_dir {
            return Err(VfsError::IsADirectory); // directory removal is a separate verb
        }
        fs.delete_located(dir_lba, dir_off, de.first_cluster()).map_err(fat_err)?;
        Ok(())
    }

    // --- VFSROUTE (orin 17) ---------------------------------------------------------------

    /// FORWARD, not a second copy: [`FatBackend::read_only`] already forwards to
    /// [`crate::fs::fat::BlockSource::write_veto`], which is the single definition of "may an
    /// ordinary file mutation reach this handle". The trait method hands the shell's write verbs
    /// the same answer the block layer gives, with the REASON attached, so the operator line names
    /// the mechanism that said no instead of a bare `-ENOTSUP`.
    fn write_veto(&self) -> Option<&'static str> {
        self.source.write_veto()
    }

    fn rename(&self, from_rel: &str, to_rel: &str, principal: &str) -> Result<(), VfsError> {
        self.authorize_write(from_rel, principal)?;
        let fs = crate::fs::fat::mount_source(self.source).map_err(fat_err)?;
        let (sparent, sleaf) = self.parent(&fs, from_rel)?;
        let (dparent, dleaf) = self.parent(&fs, to_rel)?;
        // Locate-first destination check, the create discipline: an existing name is refused with
        // the SAME spelling `create` uses, so a caller has one string to test for.
        let same_slot = sparent == dparent && dleaf.eq_ignore_ascii_case(&sleaf);
        if !same_slot {
            match fs.locate_in_dir(dparent, &dleaf) {
                Ok(_) => return Err(VfsError::Backend("exists")),
                Err(crate::fs::fat::FatError::NotFound) => {}
                Err(e) => return Err(fat_err(e)),
            }
        }
        // Same parent -> rename in place (files AND dirs); across parents -> move (the seam refuses
        // a DIRECTORY source with IsDirectory, because its `..` would need rewriting).
        let r = if sparent == dparent {
            fs.rename_entry(sparent, &sleaf, &dleaf)
        } else {
            fs.move_entry(sparent, &sleaf, dparent, &dleaf)
        };
        r.map(|_| ()).map_err(fat_create_err)
    }

    fn volume_bytes(&self) -> Option<u64> {
        crate::fs::fat::mount_source(self.source).ok().map(|fs| fs.volume_bytes())
    }

    /// The FAT geometry line the retired `fatinfo` verb printed. It prints under `mount` because
    /// geometry is a property of a MOUNT — and it prints THROUGH THE BACKEND because the `mount`
    /// verb must not know that this volume happens to be FAT.
    fn describe(&self) -> Option<String> {
        crate::fs::fat::mount_source(self.source).ok().map(|fs| fs.describe())
    }

    fn remove_dir(&self, rel: &str, principal: &str) -> Result<(), VfsError> {
        self.authorize_write(rel, principal)?;
        let fs = crate::fs::fat::mount_source(self.source).map_err(fat_err)?;
        let (parent, leaf) = self.parent(&fs, rel)?;
        let (de, _, _) = fs.locate_in_dir(parent, &leaf).map_err(fat_err)?;
        if !de.is_dir {
            return Err(VfsError::NotADirectory);
        }
        match fs.remove_dir(parent, &leaf) {
            Ok(_) => Ok(()),
            // The fat.rs seam spells a NON-EMPTY directory `IsDirectory`; the VFS spells it
            // `Backend("not-empty")` so a caller can tell it from "you aimed a file verb at a
            // directory", which is what `IsADirectory` means everywhere else in this trait.
            Err(crate::fs::fat::FatError::IsDirectory) => Err(VfsError::Backend("not-empty")),
            Err(e) => Err(fat_err(e)),
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
        // FAIL CLOSED, BUT NOT IN AN ACL'S CLOTHES (rmbp 15 — the N1 class this round kept finding
        // in corners: `mv` reporting cross-volume where the truth was a write veto, quarry's stamp
        // claiming an invalidation it never performs, and this).
        //
        // The POSTURE is right and unchanged: an object whose inode will not read authorizes
        // nothing, for everyone. Note it fails closed BEFORE the kernel short-circuit below, which
        // is what makes it reachable at kernel authority — the reason `MountTable::rename`'s
        // destination-parent call is NOT inert on today's only caller, which passes
        // `KERNEL_PRINCIPAL`.
        //
        // The SPELLING was wrong. `Denied` renders as "permission denied (-EACCES)" in BOTH
        // renderers (`shell.rs`'s `vfs_err` and `video/quarry/live.rs`), so a STORAGE failure was
        // reported as a PERMISSIONS failure — sending an operator to look for an owner row that was
        // never the problem, and making a consistency fault indistinguishable from an ACL refusal
        // at exactly the seam where the two now meet. `Backend("inode-gone")` renders "backend
        // error: inode-gone" and names the mechanism instead.
        Err(_) => return Err(VfsError::Backend("inode-gone")),
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

    /// VOLID (orin 18): the DOMAIN TAG alone, and the name is not part of it — a constant that
    /// is EXACT here rather than a degradation, because there is exactly one native volume on
    /// this machine and it is not this struct.
    ///
    /// **This is the implementor `volume_fingerprint` cannot serve, and it is why identity is
    /// a `(device, filesystem)` pair rather than a device.** A fingerprint is a method on
    /// `FatFs`; the native UnaFS volume is not a `FatFs` and has none. Degrading it to the
    /// block source would be catastrophic on the Pi, which mounts UnaFS at `/` and the FAT
    /// program volume at `/boot` off THE SAME PHYSICAL CARD (`drivers/emmc2.rs` registers the
    /// card as `BlockSource::Default` and sizes the UnaFS volume from the same geometry): the
    /// two would collide and `mv /X.TXT /boot/X.TXT` would relink an inode across filesystems.
    /// The `vfs:unafs:` tag is what keeps them apart, and it does so BY CONSTRUCTION — no FAT
    /// id can equal it whatever the medium, because the tag is mixed before anything else.
    ///
    /// The device half is degenerate, and that is a fact about the subsystem, not an omission:
    /// `NativeBackend` holds nothing but a label and every method goes through
    /// `crate::fs::unafs::with_unafs`, the ONE coherent global mount. Two `NativeBackend`s are
    /// therefore the same storage no matter what their mounts were called — the same aliasing
    /// class as C1's FAT pair, found while fixing it and closed by the same rule. When UnaFS
    /// grows a second mountable volume this method is where its volume identity is mixed in,
    /// and the trait's `Option` is already the right shape for a volume that cannot supply one.
    fn volume_id(&self) -> Option<u64> {
        Some(volid_mix(VOLID_SEED, b"vfs:unafs:"))
    }

    fn read_dir(&self, rel: &str) -> Result<Vec<DirEnt>, VfsError> {
        let path = native_abs(rel);
        crate::fs::unafs::with_unafs(|fs| {
            let id = fs.resolve_path(&path).map_err(|_| VfsError::NoSuchPath)?;
            let entries = fs.ls(id).map_err(|_| VfsError::NotADirectory)?;
            Ok(entries
                .into_iter()
                .filter(|e| {
                    // `.`/`..` are not names in this namespace (see `DirEnt`); `System` objects are
                    // unafs's internal bookkeeping and were filtered by the shell collector this
                    // arc deleted — the filter moves here rather than disappearing.
                    e.name != "." && e.name != ".." && e.kind != ::unafs::FileKind::System
                })
                .map(|e| DirEnt {
                    name: e.name,
                    kind: native_kind(e.kind),
                    // unafs holds size on the inode, not the directory entry, so the listing costs
                    // one inode read per row — the same cost `pi_ls_collect` paid. A row whose
                    // inode cannot be read reports 0 rather than failing the whole listing.
                    size: fs.read_inode(e.inode_id).map(|i| i.size).unwrap_or(0),
                    // unafs records no last-write time; §3 says a backend answers for its own
                    // medium, so this is `None`, never a fabricated stamp.
                    mtime: None,
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
            // VFSROUTE: locate-first, the same create discipline the FAT twin uses, so an existing
            // name is refused with the ONE spelling every caller tests for (`Backend("exists")`)
            // instead of the crate's own `FileExists` arriving as an opaque backend string. The verb
            // renders it `-EEXIST` for every volume.
            if fs.resolve_path(&native_abs(rel)).is_ok() {
                return Err(VfsError::Backend("exists"));
            }
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

    // --- VFSROUTE (orin 17) ---------------------------------------------------------------

    /// The native volume is journaled and read-write since K4 — there is no block-layer veto on it
    /// (the ONE coherent mount is the write path itself), so it accepts mutation.
    fn write_veto(&self) -> Option<&'static str> {
        None
    }

    fn rename(&self, from_rel: &str, to_rel: &str, principal: &str) -> Result<(), VfsError> {
        crate::fs::unafs::with_unafs(|fs| {
            let from = native_abs(from_rel);
            let id = fs.resolve_path(&from).map_err(|_| VfsError::NoSuchPath)?;
            native_write_authz(fs, id, principal)?;
            let (sparent, sleaf) = native_parent(fs, from_rel)?;
            let (dparent, dleaf) = native_parent(fs, to_rel)?;
            // An existing destination is refused by the crate (`FileExists`) and never silently
            // overwritten. It is tested HERE, by name, rather than by matching the crate's error
            // variant: the caller needs to tell "the name was taken" from every other failure, and
            // a locate-first check is the same discipline the FAT twin above uses.
            if sparent != dparent || sleaf != dleaf {
                let to = native_abs(to_rel);
                if fs.resolve_path(&to).is_ok() {
                    return Err(VfsError::Backend("exists"));
                }
            }
            // Both directory rewrites land in ONE CoW transaction, so there is no "in neither
            // directory" window to crash into.
            fs.rename(sparent, &sleaf, dparent, &dleaf)
                .map_err(|_| VfsError::Backend("unafs-rename"))
        })
        .map_err(unafs_err)?
    }

    // NOTE: `remove_dir` is deliberately NOT implemented — see the trait's note. The UnaFS crate
    // carries no directory removal, so this backend inherits the default refusal and `rmdir` on a
    // native path prints `-ENOTSUP`. That is the honest answer and it is the negative leg the
    // VFSROUTE transcript asserts.

    fn remove_attr(&self, rel: &str, key: &str, principal: &str) -> Result<(), VfsError> {
        let path = native_abs(rel);
        crate::fs::unafs::with_unafs(|fs| {
            let id = fs.resolve_path(&path).map_err(|_| VfsError::NoSuchPath)?;
            native_write_authz(fs, id, principal)?;
            fs.remove_attribute(id, key)
                .map_err(|_| VfsError::Backend("unafs-rmattr"))
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

    /// VOLID (orin 18): `None`, and this is the implementor that shows why the trait returns
    /// an `Option` instead of merely being required.
    ///
    /// A mock's storage is its own `files` map, in this object's RAM. Nothing else in the
    /// system can address it, so the only mount that IS this storage is this very object —
    /// which [`same_storage`] settles on object identity before it ever calls this method. A
    /// name-derived id would be strictly WORSE than nothing: two `MockBackend::native("native")`
    /// values are two different `Vec`s, and an id off the name would call them one volume and
    /// admit a rename between them. That is the corrupting direction, from the exact instinct
    /// (fall back to the name) that produced C1.
    ///
    /// So the honest answer is "I have no identity anything else could share", which is what
    /// `None` means. The witness legs are unaffected: none of them asks `same_volume`, and a
    /// rename within one mock would still be admitted by the object-identity floor.
    fn volume_id(&self) -> Option<u64> {
        None
    }

    fn read_dir(&self, _rel: &str) -> Result<Vec<DirEnt>, VfsError> {
        Ok(self
            .files
            .iter()
            .map(|(n, d)| DirEnt {
                name: n.trim_start_matches('/').to_string(),
                kind: NodeKind::File,
                size: d.len() as u64,
                mtime: None,
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
    // FATGROW (2026-08-22): the FAT directory-chain growth witness rides this pass.
    //
    // WHY HERE. It needs exactly what this function already has — a live writable FAT volume on the
    // boot block device, reached once, from a task with a real stack — and this is the only pass in
    // the `fs/` layer that has it: `fat::probe_once` is NOT reached on the Pi bare-metal boot
    // (verified against `target/serial-pi.log`: no `FS: FAT mounted` line), and every other FAT
    // write fixture lives in `arch/aarch64/syscall.rs`'s u7 launcher chain. Hanging it here keeps
    // the whole arc inside `fs/`.
    //
    // It runs BEFORE this witness's own scratch file rather than after, because the two are strictly
    // sequential and each is self-cleaning, and a leading call cannot be skipped by one of the early
    // `return`s below. `fatgrow_witness_once` does its own mount, its own storage-ready check and its
    // own honest SKIPPED line, so it costs this witness nothing but the call.
    //
    // ⚠ `#[cfg]` FOLDED ONTO THE STATEMENT (PARITY §5.3): this function is compiled into every
    // aarch64 build while `fatgrow_witness_once` is `witness`-gated, so the gate has to sit on the
    // call, not around the function. Knob-off, the statement does not exist before MIR.
    #[cfg(feature = "witness")]
    crate::fs::fat::fatgrow_witness_once();
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
// (at `/usb`) ALONGSIDE the SD boot FAT (at `/boot`), each routing to its own block device, and
// that the USB volume is WRITABLE through the table (USB-WRITE cleared the old read-only guard:
// a `create` at `/usb/...` now lands a real entry rather than being refused).
//
// This is a METAL proof, honest-skip under QEMU: the USB stick is reached through the xHCI
// `Usb` source (PIUSB-27), which needs the BCM2711 PCIe RC + VL805 xHCI. QEMU raspi4b models no
// PCIe RC and attaches no usb-storage, so `mount_source(Usb)` finds no device and the witness
// prints the skip line there. The positive PASS line is the attended-metal evidence, the same
// posture the whole piusb line already carries ("attended-metal for positive verify").
// =========================================================================================

/// VFS-3 USB-mount witness: build a `MountTable` carrying the SD boot FAT at `/boot` and the USB
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
    mt.mount("/boot", Box::new(FatBackend::new("fat", KERNEL_PRINCIPAL, true)));
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
    // (d) the two mounts are independent: /boot still resolves to the Default-source backend and
    // its root lists (coexistence — /usb did not displace the boot FAT).
    let fat_ok = mt.read_dir("/boot").is_ok();

    serial_println!(
        ":: VFS3: usb-mount test — /usb root {} entries, read {} bytes, create-ok={}, /boot coexists={} :: PASS ::",
        entries.len(),
        read_bytes,
        create_ok,
        fat_ok
    );
}

// =========================================================================================
// VFS-1 (adoption) routing witness — the seam's own battery.
//
// VFS-1's original witness proved the mount table resolves correctly in isolation, against two
// in-RAM mocks. What it could not prove is the thing this adoption arc is actually about: that the
// LIVE table the shell builds routes a real path to the real backend that owns it, and refuses the
// paths it must refuse. These four legs are exactly the claims the routing seam makes, each
// asserted against `MountTable::resolve` — the one resolver `ls`, `cat`, `run`, `bg` and `vfs` now
// share.
//
// Uncounted `:: VFS-1: … ::` lines, in the idiom of the VFS-2/VFS-3 witnesses beside it. Every leg
// degrades to an honest skip rather than a failure when its volume is absent, so a board with no SD
// or no stick reports what it could not test instead of reporting a regression.
// =========================================================================================

/// VFS-1 (adoption): prove the LIVE mount table routes each namespace to the backend that owns it.
///
/// Legs:
/// * **fat** — `/boot/…` resolves to the FAT backend with the mount prefix stripped.
/// * **native** — a bare `/…` resolves to the native UnaFS backend, whole path intact.
/// * **boundary** — `/fatty.bin` and `/usbfoo` are NATIVE names, not volume names: a prefix claims a
///   path only at a component boundary (§3.1). This is the negative the seam most needs, because a
///   naive `starts_with` — which is precisely what the shell's deleted `/usb` special-case used —
///   gets it wrong and silently sends a native file to a FAT volume.
/// * **ro** — a read-only backend refuses every mutating verb THROUGH THE TABLE with `Unsupported`.
///   Asserted against a backend that implements no write methods, i.e. one that inherits the trait's
///   default bodies: this is the guarantee the RO seams rest on (a backend does not have to
///   remember to refuse — refusing is what it does unless it opts in), so it is proven at the seam
///   rather than per read-only volume.
#[cfg(target_arch = "aarch64")]
pub fn vfs1_routing_witness() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // The table the shell's `vfs_mount_table()` builds, rebuilt here so the witness asserts the
    // real namespace rather than a fixture of its own.
    let mut mt = MountTable::new();
    mt.mount("/", Box::new(NativeBackend::new("native")));
    mt.mount("/boot", Box::new(FatBackend::new("fat", KERNEL_PRINCIPAL, true)));
    let usb_present = crate::fs::fat::mount_source(crate::fs::fat::BlockSource::Usb).is_ok();
    if usb_present {
        mt.mount("/usb", Box::new(FatBackend::new_usb("usb", KERNEL_PRINCIPAL)));
    }

    // --- leg 1: a /boot path reaches the FAT backend, prefix stripped. -------------------------
    match mt.resolve("/boot/VUG.ELF") {
        Ok((b, rel)) if b.volume_name() == "fat" && rel == "/VUG.ELF" => {
            serial_println!(":: VFS-1: route /boot/VUG.ELF -> vol=fat rel=/VUG.ELF :: PASS ::");
        }
        Ok((b, rel)) => {
            serial_println!(
                ":: VFS-1: route /boot/VUG.ELF -> vol={} rel={} (want fat,/VUG.ELF) :: FAIL ::",
                b.volume_name(), rel);
            return;
        }
        Err(e) => {
            serial_println!(":: VFS-1: route /boot/VUG.ELF -> {:?} :: FAIL ::", e);
            return;
        }
    }

    // --- leg 2: a bare path reaches the native UnaFS backend, whole path intact. ---------------
    match mt.resolve("/K3HELLO.TXT") {
        Ok((b, rel)) if b.volume_name() == "native" && rel == "/K3HELLO.TXT" => {
            serial_println!(":: VFS-1: route /K3HELLO.TXT -> vol=native rel=/K3HELLO.TXT :: PASS ::");
        }
        Ok((b, rel)) => {
            serial_println!(
                ":: VFS-1: route /K3HELLO.TXT -> vol={} rel={} (want native,/K3HELLO.TXT) :: FAIL ::",
                b.volume_name(), rel);
            return;
        }
        Err(e) => {
            serial_println!(":: VFS-1: route /K3HELLO.TXT -> {:?} :: FAIL ::", e);
            return;
        }
    }

    // --- leg 3: the boundary negatives — a prefix is not a substring. --------------------------
    // `/usbfoo` is only meaningful to assert when /usb is actually bound; when the stick is absent
    // the name trivially lands on native and proves nothing, so say which case was tested.
    let mut boundary_ok = true;
    for name in ["/fatty.bin", "/usbfoo"] {
        match mt.resolve(name) {
            Ok((b, rel)) if b.volume_name() == "native" && rel == name => {}
            Ok((b, rel)) => {
                serial_println!(
                    ":: VFS-1: boundary {} -> vol={} rel={} (want native, verbatim) :: FAIL ::",
                    name, b.volume_name(), rel);
                boundary_ok = false;
            }
            Err(e) => {
                serial_println!(":: VFS-1: boundary {} -> {:?} :: FAIL ::", name, e);
                boundary_ok = false;
            }
        }
    }
    if !boundary_ok {
        return;
    }
    serial_println!(
        ":: VFS-1: boundary /fatty.bin,/usbfoo -> vol=native verbatim (usb bound={}) :: PASS ::",
        usb_present);

    // --- leg 4: the read-only seam refuses every mutating verb, through the table. -------------
    // MockBackend implements the read side only, so it inherits the trait's default write bodies.
    // That is the property under test: read-only is the DEFAULT posture, not something a backend
    // has to remember to assert.
    let mut ro = MountTable::new();
    ro.mount("/ro", Box::new(MockBackend::foreign("ro", KERNEL_PRINCIPAL, true)));
    let refusals = [
        ("create", ro.create("/ro/NEW.TXT", NodeKind::File, KERNEL_PRINCIPAL).err()),
        ("write", ro.write("/ro/a.txt", 0, b"x", KERNEL_PRINCIPAL).err()),
        ("truncate", ro.truncate("/ro/a.txt", 0, KERNEL_PRINCIPAL).err()),
        ("unlink", ro.unlink("/ro/a.txt", KERNEL_PRINCIPAL).err()),
    ];
    for (verb, err) in &refusals {
        if !matches!(err, Some(VfsError::Unsupported)) {
            serial_println!(
                ":: VFS-1: ro-seam {} -> {:?} (want Unsupported) :: FAIL ::", verb, err);
            return;
        }
    }
    // The same backend still READS — a read-only volume is refused for writes, not disabled.
    let read_ok = ro.read_dir("/ro").is_ok();
    serial_println!(
        ":: VFS-1: ro-seam create/write/truncate/unlink -> Unsupported, read still ok={} :: PASS ::",
        read_ok);
}

// =========================================================================================
// VFSROUTE (orin 17) — the source-parametrized constructor, appended at the FILE TAIL.
//
// A SECOND `impl FatBackend` block, appended at the file tail rather than inserted as a method in
// the first: this file is compiled into the knob-off `kernel8.img` and `panic::Location` embeds
// source line numbers, so a method inserted mid-file moves every panic site below it. A tail append
// moves nothing. (It was the THIRD such block until BOOTROOT (orin 22) deleted the ROOTFS one that
// stood above it: a constructor that hard-coded ONE source (the Orin card) and whose only caller
// was the retired per-board root knob. A caller that has resolved a source names it here instead,
// whatever the source is.)
// =========================================================================================

impl FatBackend {
    /// VFSROUTE: mount a FAT volume through an EXPLICIT block source.
    ///
    /// Its callers are the shell's `vfs_mount_table()` on BOTH arms. On x86 it must bind THE
    /// PROGRAM SOURCE —
    /// the handle `crate::drivers::block::program_source` names — and not the global slot. On a
    /// machine booted from the internal SD reader those are different devices, and FATVERB's whole
    /// argument is that a shell where `ls` and `run` disagree about which volume is the volume is
    /// not a shell. On aarch64 it binds the source `fs::bootdisk::locate` matched — the disk this
    /// kernel's own image was found on — which is `Default` on the Pi's card and `TegraSd` or `Usb`
    /// on an Orin, decided by content and never by a board cfg. [`FatBackend::new`] hard-codes
    /// `Default`, so a caller that has already resolved the handle needs this constructor to say so.
    ///
    /// Read-only posture is not decided here: [`FatBackend::read_only`] and the trait's
    /// `write_veto` both forward to [`crate::fs::fat::BlockSource::write_veto`], which answers for
    /// whatever source is passed in.
    pub fn new_source(
        volume: &str,
        principal: &str,
        world_readable: bool,
        source: crate::fs::fat::BlockSource,
    ) -> Self {
        Self {
            volume: volume.to_string(),
            principal: principal.to_string(),
            world_readable,
            source,
            root: String::new(),
        }
    }
}
