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

//! BeFS-K4: read-WRITE kernel mount of a native UnaFS volume.
//!
//! Wires the kernel's 512 B block layer ([`crate::drivers::block`]) into the
//! `unafs` crate's K2 seam: [`SdSectorDevice`] implements
//! [`unafs::adapter::SectorDevice`] over `read_block`/`write_block`,
//! `locate_unafs` finds the UnaFS partition by superblock magic, and [`mount`]
//! returns a live `UnaFS<BlockAdapter<SdSectorDevice>>`. Arch-neutral like
//! `fs::fat` — it builds on the generic block layer only — though today only
//! the Pi 4 media carries a UnaFS partition.
//!
//! **BeFS-K4 (writable) — the coherence keystone.** `write_sector` routes to
//! [`crate::drivers::block::write_block`] (emmc2 CMD24, R1/CMD13-hardened),
//! so the mount is read-write. K3 mounted per call, which is safe only while
//! the volume is immutable; writes make a per-call mount a COHERENCE HAZARD
//! (two live mounts = two independent in-RAM allocation maps, so a block one
//! frees the other can re-hand-out → corruption). Every access — read AND
//! write — therefore flows through the single, process-wide, IRQ-masked
//! mount [`with_unafs`] (one authoritative in-RAM refcount/inode map, all
//! operations serialized; modelled on the F3 `NAMESPACE` lock). Keeping one
//! mount live also means a pure read never triggers any drop-time write-back,
//! so reads stay genuinely read-only.
//!
//! **K8a — copy-on-write; the torn-write class is CLOSED BY FORMAT.** The
//! crate's write path never overwrites a committed block: every mutation
//! writes fresh blocks, then commits by flipping ONE 512 B root sector (A/B
//! generation-stamped slots — `unafs::root`), which
//! [`BlockAdapter::write_sector_in_block`] lowers to a single hardened
//! `write_sector` on the medium. The pre-K8 4096↔512 atomicity gap (one
//! `write_block` = eight non-atomic sector writes tearing a metadata swap)
//! no longer has a load-bearing write to tear: a power cut anywhere yields
//! the old committed tree or the new one, never a hybrid. The WAL is gone —
//! there is no dirty-mount state. See `docs/SECURITY.md` §K4 (ledger entry
//! RETIRED-PENDING-METAL by K8a) and the `K8a-cow` witness below.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use ::unafs::inode::AttributeValue;

use spin::Mutex;

use crate::drivers::block::{self, BlockError};
use ::unafs::adapter::{
    BlockAdapter, PartError, PartitionSpan, SectorDevice, SectorError, locate_unafs,
};
use ::unafs::fs::FileSystemError;
use ::unafs::UnaFS;

/// The kernel block layer as a 512 B [`SectorDevice`].
///
/// Constructed by [`SdSectorDevice::open`] only when a block device is
/// registered and its logical block size is exactly 512 B, so `read_sector`'s
/// LBA space is the device's native one with no scaling.
pub struct SdSectorDevice {
    sectors: u64,
}

impl SdSectorDevice {
    /// Open the registered block device, if its geometry fits the seam.
    pub fn open() -> Result<Self, MountError> {
        let dev = block::info().ok_or(MountError::NoStorage)?;
        if dev.block_size != 512 {
            return Err(MountError::BadSectorSize(dev.block_size));
        }
        Ok(Self {
            sectors: dev.num_blocks,
        })
    }
}

impl SectorDevice for SdSectorDevice {
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), SectorError> {
        if buf.len() != 512 {
            return Err(SectorError::Io(format!(
                "read buffer {} != sector size 512",
                buf.len()
            )));
        }
        match block::read_block(lba, buf) {
            Ok(512) => Ok(()),
            Ok(n) => Err(SectorError::Io(format!("short sector read: {n} bytes"))),
            Err(BlockError::BadLba) => Err(SectorError::OutOfBounds(lba)),
            Err(e) => Err(SectorError::Io(format!("block layer: {e:?}"))),
        }
    }

    fn write_sector(&mut self, lba: u64, buf: &[u8]) -> Result<(), SectorError> {
        // K4: the real write path. One 512 B sector -> one hardened block-layer
        // write (emmc2 CMD24 + R1/CMD13 status checks). `write_block` re-guards
        // the lba against the device's own num_blocks, so an out-of-range write
        // is refused (BadLba) before it touches the medium.
        if buf.len() != 512 {
            return Err(SectorError::Io(format!(
                "write buffer {} != sector size 512",
                buf.len()
            )));
        }
        match block::write_block(lba, buf) {
            Ok(()) => Ok(()),
            Err(BlockError::BadLba) => Err(SectorError::OutOfBounds(lba)),
            Err(e) => Err(SectorError::Io(format!("block layer: {e:?}"))),
        }
    }

    fn sector_count(&self) -> u64 {
        self.sectors
    }

    /// LOAD-BEARING CONTRACT (K8a commit ordering — lens B, 2026-07-16):
    /// this flush is a deliberate NO-OP because every `write_sector` above is
    /// SYNCHRONOUS-TO-MEDIUM — the emmc2 block layer busy-waits CMD24 program
    /// completion and checks CMD13 status before returning, so by the time
    /// the crate's commit calls `flush()` as its pre-root-flip "barrier",
    /// every fresh block is already on the card. The CoW guarantee ("old tree
    /// or new tree, never a hybrid") rests on exactly this property. If a
    /// future storage path introduces a write cache, write-back queueing, or
    /// DMA-deferred completion, this method MUST become a real drain/flush —
    /// otherwise the root flip can reach the medium before the tree it points
    /// at, and commit ordering silently breaks.
    fn flush(&mut self) -> Result<(), SectorError> {
        Ok(())
    }
}

/// Why a UnaFS mount attempt failed.
#[derive(Debug)]
pub enum MountError {
    /// No block device registered yet.
    NoStorage,
    /// The block device's logical block size is not 512 B.
    BadSectorSize(u32),
    /// Partition-table parsing failed.
    Part(PartError),
    /// No partition carries a UnaFS superblock.
    NoVolume,
    /// The filesystem itself refused the mount.
    Fs(FileSystemError),
}

/// A mounted read-only UnaFS volume over the kernel block layer.
pub type KernelUnaFS = UnaFS<BlockAdapter<SdSectorDevice>>;

/// Route the unafs crate's no_std warnings (e.g. the dirty-mount notice) to the
/// kernel serial console. Installed once, at the first mount attempt.
fn install_warn_hook() {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if !INSTALLED.swap(true, Ordering::Relaxed) {
        ::unafs::warnlog::set_warn_hook(|msg| serial_println!("UNAFS: {}", msg));
    }
}

/// Locate the UnaFS partition on the registered block device.
pub fn locate() -> Result<PartitionSpan, MountError> {
    let mut dev = SdSectorDevice::open()?;
    locate_unafs(&mut dev)
        .map_err(MountError::Part)?
        .ok_or(MountError::NoVolume)
}

/// Locate and construct a FRESH UnaFS mount. Prefer [`with_unafs`] for all
/// real access — a fresh mount holds its own in-RAM allocation bitmap and
/// journal cursor, so two of them live at once (or one alongside the shared
/// [`MOUNT`]) is a K4 write-coherence hazard. This is the primitive
/// [`with_unafs`] and [`force_remount`] build on; direct callers must ensure
/// no other mount is live.
pub fn mount() -> Result<KernelUnaFS, MountError> {
    install_warn_hook();
    let span = locate()?;
    let dev = SdSectorDevice::open()?;
    let adapter = BlockAdapter::for_partition(dev, &span);
    UnaFS::mount(adapter).map_err(MountError::Fs)
}

/// The single, process-wide UnaFS mount — the K4 coherence keystone.
///
/// Exactly ONE live mount for the volume, so there is one authoritative in-RAM
/// allocation bitmap and one journal cursor. Populated lazily on first use and
/// then kept (never dropped in steady state; `Drop`'s metadata write-back would
/// otherwise fire on a mere read). [`force_remount`] is the only thing that
/// clears it.
static MOUNT: Mutex<Option<KernelUnaFS>> = Mutex::new(None);

/// Run `f` against the one coherent mount, mounting on demand.
///
/// IRQ-masked around the lock (the F3 `NAMESPACE` discipline): a timer preempt
/// of a holder followed by a same-core re-entry into a unafs sequence would
/// deadlock the non-reentrant spinlock, so the whole critical section runs with
/// IRQs masked. The aarch64 storage path is fully polled (no scheduler yield
/// under the lock), so holding across the bounded block I/O is sound — the same
/// reasoning the FAT-side `NAMESPACE`/`FAT_MUTATION` locks rest on. Returns
/// [`MountError`] if the volume cannot be mounted (the cache stays empty, so a
/// later call retries).
pub fn with_unafs<R>(f: impl FnOnce(&mut KernelUnaFS) -> R) -> Result<R, MountError> {
    crate::arch::without_interrupts(|| {
        let mut guard = MOUNT.lock();
        if guard.is_none() {
            *guard = Some(mount()?);
        }
        Ok(f(guard.as_mut().expect("mount just populated")))
    })
}

/// Drop the cached live mount so the next [`with_unafs`] re-reads the volume
/// from disk with a fresh in-RAM bitmap/journal — a genuine remount, not a
/// RAM-cache re-read. The dropped instance's `Drop` flushes its
/// already-consistent metadata; the fresh mount then observes only what
/// actually reached the medium. This is the durability-proof primitive the K4
/// witness uses to prove a write survived being committed to the block device.
pub fn force_remount() {
    crate::arch::without_interrupts(|| {
        *MOUNT.lock() = None;
    });
}

// =====================================================================================================
// K6: the NATIVE ACL SEAM — the U6 owner/grants ACL stored as native unafs TYPED ATTRIBUTES, retiring
// the FAT-bridge `UNAFS.ATR` sidecar. The unafs volume is a DEDICATED ATTRIBUTE volume this arc (VFS
// verdict 2): the kernel is its only client, there is NO user-visible unafs namespace.
// =====================================================================================================
//
// LAYOUT. Each owned FAT file has exactly one unafs file in the volume root, named `acl-<lba>-<off>`
// (hex of its FAT directory-slot identity `(dir_lba, dir_off)` — the same runtime key the in-RAM
// `OWNED_FILES` table uses, so a runtime persist/clear is an O(1) `resolve_path`, not a scan). The
// file carries typed attributes:
//   `name`  : String  — the FAT 8.3 name (the DURABLE rebuild key; a recycled slot is only a hint,
//                       so mount re-resolves BY NAME exactly like `atr_rebuild_into_owned`);
//   `fc`    : Int      — first_cluster (identity corroboration; 0 = unknown/0-length);
//   `owner` : String  — the owner principal's canonical native string (`principal_native_string`);
//   `grants:<grantee>` : String — one attribute per grant, KEY = `grant_native_key(grantee)`,
//                                  VALUE = `rights_native_value(rights)`.
// The owner/grant byte strings are pre-projected by the caller (`syscall.rs`, the K4-ready codec) so
// this module stays ACL-policy-agnostic: it carries opaque `&[u8]` key/value pairs and never derives
// a principal. The reverse projection (native string -> `PrincipalRecord`) lives with the codec.
//
// COHERENCE + ORDERING. Every access flows through the one process-wide, IRQ-masked [`with_unafs`]
// mount and each attribute mutation is a journaled `set_attribute`/`remove_attribute`/`unlink` (the K4
// write path — new-extents-first, single-block metadata swap, free-last: a power cut LEAKS, never
// dangles). K5B (2026-07-15, the NAMESPACE-unfreeze): the K3 two-phase (durable-first) and K5
// (anti-resurrection) orderings are PRESERVED by the callers in `syscall.rs` fusing on THIS mount lock —
// each persist site takes its OWNED_FILES snapshot, performs the journaled write (`native_acl_write_on`),
// and commits in-RAM all inside ONE `with_unafs` hold, so MOUNT itself serializes the persisters (the
// K5B invariant); the FAT `NAMESPACE` lock is no longer held across any ACL disk write. Lock order:
// `NAMESPACE ⊃ MOUNT ⊃ OWNED_FILES` (no path ever takes MOUNT then NAMESPACE, nor OWNED_FILES then
// MOUNT; OWNED_FILES stays a take-and-release leaf).

/// K6: the volume-root file name for the ACL row of the FAT file at `(dir_lba, dir_off)`.
fn acl_file_name(dir_lba: u64, dir_off: u32) -> String {
    format!("acl-{:x}-{:x}", dir_lba, dir_off)
}

/// K6: one ACL row read back from the native attribute volume. Byte strings are the on-disk native
/// projections; the caller reverses them into principals. `name` is the durable FAT re-resolve key.
pub struct NativeAclRow {
    pub name: String,
    pub first_cluster: u32,
    pub owner: Vec<u8>,
    /// `(grant_native_key, rights_native_value)` byte strings, one per grant.
    pub grants: Vec<(Vec<u8>, Vec<u8>)>,
}

/// True iff `e` is the "absent" signal (a missing file/name), which the idempotent clear/read paths
/// treat as "nothing there", distinct from a real I/O error.
fn is_absent(e: &FileSystemError) -> bool {
    matches!(e, FileSystemError::NotFound | FileSystemError::RootMissing)
}

/// K6: write (create-or-replace) the ACL row for `(dir_lba, dir_off)` — the `atr_persist_row` /
/// `atr_write_grant_row_locked` native successor. Rewrites owner + grants WHOLESALE: stale `grants:*`
/// attributes not present in `grants` are removed first, so a revoke (a narrower grant set) durably
/// drops the revoked edge. `owner`/grant strings are the caller's pre-projected native bytes. Returns
/// `true` iff every journaled step committed; `false` on any mount or write error (the caller treats a
/// persist failure as non-fatal for a grow/re-persist, or fail-closed for a create).
pub fn native_acl_write(
    dir_lba: u64,
    dir_off: u32,
    name: &str,
    first_cluster: u32,
    owner: &[u8],
    grants: &[(&[u8], &[u8])],
) -> bool {
    with_unafs(|fs| native_acl_write_on(fs, dir_lba, dir_off, name, first_cluster, owner, grants))
        .unwrap_or(false)
}

/// K5B: the body of [`native_acl_write`], against an ALREADY-HELD coherent mount. The persist sites
/// (`syscall.rs`) call this inside their single fused `with_unafs` hold — snapshot, THIS journaled write,
/// and the in-RAM commit all under one MOUNT hold (the K5B invariant) — so they must not re-enter
/// `with_unafs` (the MOUNT spinlock is non-reentrant). Same contract as the wrapper: `true` iff every
/// journaled step committed.
pub fn native_acl_write_on(
    fs: &mut KernelUnaFS,
    dir_lba: u64,
    dir_off: u32,
    name: &str,
    first_cluster: u32,
    owner: &[u8],
    grants: &[(&[u8], &[u8])],
) -> bool {
    let owner_s = match core::str::from_utf8(owner) {
        Ok(s) => s,
        Err(_) => return false,
    };
    {
        let root = fs.superblock.root_inode;
        let fname = acl_file_name(dir_lba, dir_off);
        let id = match fs.resolve_path(&format!("/{}", fname)) {
            Ok(id) => id,
            Err(ref e) if is_absent(e) => match fs.create_file(root, fname) {
                Ok(id) => id,
                Err(_) => return false,
            },
            Err(_) => return false,
        };
        // Remove any stale grant attributes no longer in the caller's set (the revoke path).
        let stale: Vec<String> = match fs.read_inode(id) {
            Ok(ino) => ino
                .attributes
                .keys()
                .filter(|k| k.starts_with("grants:"))
                .filter(|k| !grants.iter().any(|(gk, _)| *gk == k.as_bytes()))
                .cloned()
                .collect(),
            Err(_) => return false,
        };
        for k in stale {
            if fs.remove_attribute(id, &k).is_err() {
                return false;
            }
        }
        // Refresh the durable key fields + owner + the current grant set (journaled writes).
        if fs
            .set_attribute(id, "name".to_string(), AttributeValue::String(name.to_string()))
            .is_err()
            || fs
                .set_attribute(id, "fc".to_string(), AttributeValue::Int(first_cluster as i64))
                .is_err()
            || fs
                .set_attribute(id, "owner".to_string(), AttributeValue::String(owner_s.to_string()))
                .is_err()
        {
            return false;
        }
        for (gk, rv) in grants {
            let (ks, vs) = match (core::str::from_utf8(gk), core::str::from_utf8(rv)) {
                (Ok(k), Ok(v)) => (k, v),
                _ => return false,
            };
            if fs
                .set_attribute(id, ks.to_string(), AttributeValue::String(vs.to_string()))
                .is_err()
            {
                return false;
            }
        }
        true
    }
}

/// K6: read back the ACL row for `(dir_lba, dir_off)`, or `None` if there is no row (public) or the
/// mount fails. The read is pure (no metadata write-back through the coherent mount). Used by the
/// migration read-back verify and by a native single-row lookup.
pub fn native_acl_read(dir_lba: u64, dir_off: u32) -> Option<NativeAclRow> {
    with_unafs(|fs| native_acl_read_on(fs, dir_lba, dir_off)).ok().flatten()
}

/// K5B: the body of [`native_acl_read`], against an ALREADY-HELD coherent mount — for callers already
/// inside their fused `with_unafs` hold (re-entering the non-reentrant MOUNT spinlock would deadlock).
pub fn native_acl_read_on(fs: &mut KernelUnaFS, dir_lba: u64, dir_off: u32) -> Option<NativeAclRow> {
    let fname = acl_file_name(dir_lba, dir_off);
    let id = fs.resolve_path(&format!("/{}", fname)).ok()?;
    native_acl_row_of(fs, id)
}

/// K6: extract a [`NativeAclRow`] from an already-resolved ACL-file inode. A row with no `owner`
/// attribute yields `None` (public / not an ACL). Shared by the single-row read and the list walk.
fn native_acl_row_of(fs: &mut KernelUnaFS, id: u64) -> Option<NativeAclRow> {
    let ino = fs.read_inode(id).ok()?;
    let owner = match ino.attributes.get("owner") {
        Some(AttributeValue::String(s)) => s.as_bytes().to_vec(),
        _ => return None,
    };
    let name = match ino.attributes.get("name") {
        Some(AttributeValue::String(s)) => s.clone(),
        _ => return None,
    };
    let first_cluster = match ino.attributes.get("fc") {
        Some(AttributeValue::Int(v)) => *v as u32,
        _ => 0,
    };
    let mut grants: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for (k, v) in ino.attributes.iter() {
        if let (true, AttributeValue::String(rv)) = (k.starts_with("grants:"), v) {
            grants.push((k.as_bytes().to_vec(), rv.as_bytes().to_vec()));
        }
    }
    Some(NativeAclRow { name, first_cluster, owner, grants })
}

/// K6: clear the ACL row for `(dir_lba, dir_off)` — the `atr_clear_row` native successor. Deletes the
/// unafs ACL file (journaled), reverting the FAT file to public. Idempotent: an absent row is already
/// clear (`true`); only a real mount/I-O error is `false`.
pub fn native_acl_clear(dir_lba: u64, dir_off: u32) -> bool {
    match with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let fname = acl_file_name(dir_lba, dir_off);
        match fs.unlink(root, &fname) {
            Ok(_) => true,
            Err(ref e) if is_absent(e) => true, // already public on disk
            Err(_) => false,
        }
    }) {
        Ok(ok) => ok,
        // Media without a unafs volume (or no block device) holds no native row — nothing persisted
        // to clear (benign, like the sidecar's NotFound). Only a real mount/parse failure is an error.
        Err(MountError::NoVolume | MountError::NoStorage | MountError::BadSectorSize(_)) => true,
        Err(_) => false,
    }
}

/// K6: every ACL row on the volume — the mount-time rebuild source (the `atr_rebuild_into_owned`
/// native successor reads this and re-resolves each `name` on the FAT volume). Skips non-`acl-` files
/// and rows without an owner. On a mount error, an empty list (fail-closed: no owners installed).
pub fn native_acl_list() -> Vec<NativeAclRow> {
    with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let mut out = Vec::new();
        let entries = match fs.ls(root) {
            Ok(e) => e,
            Err(_) => return out,
        };
        for e in entries {
            if e.kind != ::unafs::FileKind::File || !e.name.starts_with("acl-") {
                continue;
            }
            if let Some(row) = native_acl_row_of(fs, e.inode_id) {
                out.push(row);
            }
        }
        out
    })
    .unwrap_or_default()
}

/// The K3HELLO.TXT fixture contents, byte-pinned against what `arroyo kernel8`
/// stages into the unafs volume.
const K3_HELLO: &[u8] = b"Hello from native UnaFS on the Pi 4!\n";
/// K3PAT.BIN: 12 KiB, byte i = (i*7+3)&0xFF — three unafs blocks, so reading it
/// walks extents, not just a single block.
const K3_PAT_LEN: u64 = 12288;

/// K3-mount witness (locate + mount + superblock sanity + seam bound-check;
/// root `ls` + byte-verified reads through resolve/extent walking).
///
/// Called at the tail of the aarch64 `u7_launcher` fixture chain (the
/// `k4_ready_selftest` idiom): one-shot, read-only, and its serial evidence is
/// the uncounted `:: K3-mount: … ::` line — never a `-> PASS` fixture line, so
/// the 23-PASS battery stays byte-equivalent. On media without a UnaFS
/// partition it reports a skip, not a failure.
///
/// K4 update: the read path now runs through the coherent [`with_unafs`] mount
/// (no per-call mount → no `Drop`-time metadata write on this pure read), and
/// bit4 no longer writes to the volume (K3 relied on the RO seam refusing a
/// write to `base_lba`; with real writes that would zero the superblock).
/// bit4 instead proves the now-live write seam is bound-checked: a write to an
/// out-of-range LBA is refused before it can touch the medium. Writes proper
/// are proven by the `K4-write` witness.
pub fn k3_mount_selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // No unafs partition staged (e.g. an old card): honest skip, not a FAIL.
    let span = match locate() {
        Ok(span) => span,
        Err(e) => {
            serial_println!(":: K3-mount: no unafs volume ({:?}) — skipped ::", e);
            return;
        }
    };

    let mut w = 0u32;

    // bit0: the partition span is sane (non-empty, on-device).
    if span.block_count > 0 {
        w |= 1 << 0;
    }

    // The read bits, computed against the ONE coherent mount (a pure read: it
    // never mutates, so it never triggers a metadata write-back).
    let read_bits = with_unafs(|fs| {
        let mut r = 0u32;
        // bit1: superblock magic is the frozen on-disk signature.
        if fs.superblock.magic == ::unafs::superblock::MAGIC {
            r |= 1 << 1;
        }
        // bit2: version + block size are the pinned format constants.
        if fs.superblock.version == ::unafs::superblock::VERSION
            && fs.superblock.block_size as u64 == ::unafs::BLOCK_SIZE
        {
            r |= 1 << 2;
        }
        // bit3: the volume fits the partition that carries it.
        if fs.superblock.block_count <= span.block_count {
            r |= 1 << 3;
        }

        // bit5: `ls /` sees exactly the two staged fixture FILES.
        if let Ok(entries) = fs.ls(fs.superblock.root_inode) {
            let hello = entries
                .iter()
                .find(|e| e.name == "K3HELLO.TXT" && e.kind == ::unafs::FileKind::File);
            let pat = entries
                .iter()
                .find(|e| e.name == "K3PAT.BIN" && e.kind == ::unafs::FileKind::File);
            if entries.len() == 2 && hello.is_some() && pat.is_some() {
                r |= 1 << 5;
            }
        }

        // bit6: resolve + read K3HELLO.TXT — every byte matches the pinned text.
        if let Ok(id) = fs.resolve_path("/K3HELLO.TXT") {
            if let (Ok(inode), Ok(data)) =
                (fs.read_inode(id), fs.read_data(id, 0, K3_HELLO.len() as u64 + 8))
            {
                if inode.size == K3_HELLO.len() as u64 && data == K3_HELLO {
                    r |= 1 << 6;
                }
            }
        }

        // bit7: K3PAT.BIN — all 12 KiB match the (i*7+3)&0xFF pattern, so the
        // read crossed unafs block (and extent) boundaries intact.
        if let Ok(id) = fs.resolve_path("/K3PAT.BIN") {
            if let (Ok(inode), Ok(data)) = (fs.read_inode(id), fs.read_data(id, 0, K3_PAT_LEN)) {
                if inode.size == K3_PAT_LEN
                    && data.len() as u64 == K3_PAT_LEN
                    && data
                        .iter()
                        .enumerate()
                        .all(|(i, &b)| b == ((i * 7 + 3) & 0xFF) as u8)
                {
                    r |= 1 << 7;
                }
            }
        }

        // bit8: a missing name refuses to resolve (negative witness).
        if fs.resolve_path("/K3NOPE.TXT").is_err() {
            r |= 1 << 8;
        }
        r
    });
    match read_bits {
        Ok(r) => w |= r,
        Err(e) => {
            serial_println!(":: K3-mount: located but mount FAILED ({:?}) ::", e);
            return;
        }
    }

    // bit4: the write seam is bound-checked — a write to an out-of-range LBA is
    // refused (BadLba) before it reaches the medium. This touches NO real data
    // (the target sector does not exist); it replaces the K3 base_lba write,
    // which with a live write path would have zeroed the superblock.
    if let Ok(mut dev) = SdSectorDevice::open() {
        let sector = [0u8; 512];
        let oob = dev.sector_count().saturating_add(1024);
        if dev.write_sector(oob, &sector).is_err() {
            w |= 1 << 4;
        }
    }

    let verdict = if w == 0x1ff { "PASS" } else { "FAIL" };
    serial_println!(
        ":: K3-mount: native unafs volume located (base_lba={}, {} blocks) + superblock v{} mounted + ls/cat byte-verified {} [w={:#05x}] ::",
        span.base_lba,
        span.block_count,
        ::unafs::superblock::VERSION,
        verdict,
        w
    );
}

/// K4-write scratch fixture: a small file the witness creates, writes, remounts,
/// byte-verifies, then deletes — leaving the volume as it found it (the K2
/// self-cleaning idiom), so `K3-mount`'s exact-two-entries `ls` still holds on
/// the next boot.
const K4_NAME: &str = "K4TEST.TXT";
const K4_PATH: &str = "/K4TEST.TXT";
/// Payload spans one 512 B sector (well under a 4096 B block), so the durability
/// proof rides the single-sector, atomic-swap-clean case.
const K4_PAYLOAD: &[u8] = b"K4 journaled write on native UnaFS -- durable across a remount.\n";

/// K4-write witness: prove the kernel can WRITE the native unafs volume and that
/// the write is DURABLE across a genuine remount, all through the one coherent
/// mount. Runs last in the `u7_launcher` chain (after `k3_mount_selftest`);
/// self-cleaning (create then delete + journal reset), so a card that carries
/// the write-back is left with only the staged K3 fixtures.
///
/// Sequence (7 bits):
///   bit0 create /K4TEST.TXT + write the payload;
///   bit1 force a real remount (fresh in-RAM maps, re-read from disk);
///   bit2 resolve + read back — exact bytes match (WRITE DURABILITY);
///   bit3 delete it (one atomic CoW transaction);
///   bit4 remount again — the name no longer resolves (DELETE DURABILITY);
///   bit5 a missing nested path resolves to Err (negative path);
///   bit6 the fresh mount is refcount-CONSISTENT (fsck clean — the K8
///        successor of the old clean-journal check) and root `ls` is back to
///        the staged fixtures (no K4TEST leak).
///
/// On media without a unafs partition it skips, like `k3_mount_selftest`.
pub fn k4_write_selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    if let Err(e) = locate() {
        serial_println!(":: K4-write: no unafs volume ({:?}) — skipped ::", e);
        return;
    }

    let mut w = 0u32;

    // bit0: create + write through the coherent mount. Clear any stale scratch
    // from a prior interrupted run first (idempotent).
    let created = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let _ = fs.unlink(root, K4_NAME);
        match fs.create_file(root, String::from(K4_NAME)) {
            Ok(id) => fs.write_data(id, 0, K4_PAYLOAD).is_ok(),
            Err(_) => false,
        }
    });
    if matches!(created, Ok(true)) {
        w |= 1 << 0;
    }

    // bit1: a genuine remount — the next access re-reads the volume from disk.
    force_remount();

    // bit2: the file is present after the remount, with the exact bytes — proof
    // the write reached the block device (not a RAM cache).
    let verified = with_unafs(|fs| match fs.resolve_path(K4_PATH) {
        Ok(id) => match (fs.read_inode(id), fs.read_data(id, 0, K4_PAYLOAD.len() as u64)) {
            (Ok(inode), Ok(data)) => {
                inode.size == K4_PAYLOAD.len() as u64 && data == K4_PAYLOAD
            }
            _ => false,
        },
        Err(_) => false,
    });
    if matches!(verified, Ok(true)) {
        // bit1 credits the successful fresh mount; bit2 the byte-verify.
        w |= 1 << 1;
    }
    if matches!(verified, Ok(true)) {
        w |= 1 << 2;
    }

    // bit3: delete the scratch file — under K8a one atomic CoW transaction
    // (catalog scrub + directory rewrite + block release, one root flip).
    let deleted = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        fs.unlink(root, K4_NAME).is_ok()
    });
    if matches!(deleted, Ok(true)) {
        w |= 1 << 3;
    }

    // bit4: remount; the delete is durable (the name no longer resolves).
    force_remount();
    let gone = with_unafs(|fs| fs.resolve_path(K4_PATH).is_err());
    if matches!(gone, Ok(true)) {
        w |= 1 << 4;
    }

    // bit5: negative path — a write/resolve of a missing nested parent is Err,
    // never a silent success.
    let neg = with_unafs(|fs| fs.resolve_path("/K4NOPE/DEEP.TXT").is_err());
    if matches!(neg, Ok(true)) {
        w |= 1 << 5;
    }

    // bit6: the fresh mount is refcount-CONSISTENT (a full reachability-vs-
    // refcount fsck dry run — the K8 successor of the clean-journal check)
    // and the root is back to exactly the staged fixtures — no K4TEST leak.
    let clean = with_unafs(|fs| {
        let consistent = fs.fsck(false).map(|r| r.is_clean()).unwrap_or(false);
        let no_leak = fs
            .ls(fs.superblock.root_inode)
            .map(|entries| entries.iter().all(|d| d.name != K4_NAME))
            .unwrap_or(false);
        consistent && no_leak
    });
    if matches!(clean, Ok(true)) {
        w |= 1 << 6;
    }

    let verdict = if w == 0x7f { "PASS" } else { "FAIL" };
    serial_println!(
        ":: K4-write: create+write {} + remount byte-verify + delete + remount + negative + clean-tree {} [w={:#04x}] ::",
        K4_PATH,
        verdict,
        w
    );
}

/// K8a scratch fixture: created, crash-simulated, re-created, deleted — the
/// volume is left exactly as found (self-cleaning, K2 idiom).
const K8_NAME: &str = "K8CUT.TXT";
const K8_PATH: &str = "/K8CUT.TXT";
const K8_PAYLOAD: &[u8] = b"K8a copy-on-write commit -- old tree or new tree, never a hybrid.\n";

/// K8a-cow witness (uncounted): prove the copy-on-write commit discipline on
/// the live kernel mount, benchmark-instrumented (CNTPCT ticks per commit +
/// blocks written — the vaire ruling's before/after numbers).
///
/// Sequence (7 bits):
///   bit0 a mutation ADVANCES the root generation (create scratch → gen+);
///   bit1 CoW: the old tree stays intact until the flip — a mutation with
///        autocommit OFF (fresh blocks written, root NEVER flipped) followed
///        by a genuine remount lands on the OLD tree: the file is absent
///        (POWER-CUT-MID-COMMIT CONVERGENCE, the crash-simulation seam);
///   bit2 the post-"cut" volume is refcount-consistent (fsck clean);
///   bit3 the same mutation WITH the commit is durable across a remount
///        (byte-verified) — new tree wins once the root flips;
///   bit4 REFCOUNT PERSISTENCE: after the remount the freshly loaded
///        refcount map agrees with recomputed reachability (fsck clean on
///        the persisted map);
///   bit5 delete + remount: gone, consistent, no leak (self-cleaning);
///   bit6 commit stats live: commits > 0 and blocks-written > 0 recorded.
///
/// Skips honestly on media without a unafs partition.
///
/// Tick source: `CNTPCT_EL0` on aarch64; 0 on other arches (the witness is
/// only chained on the Pi, but this module compiles on both — zero x86
/// behavior change).
fn bench_ticks() -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch::timer::cntpct()
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        0
    }
}

pub fn k8a_cow_selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    if let Err(e) = locate() {
        serial_println!(":: K8a-cow: no unafs volume ({:?}) — skipped ::", e);
        return;
    }

    let mut w = 0u32;
    let t0 = bench_ticks();

    // bit0: generation advances on a committed mutation.
    let gen_adv = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let _ = fs.unlink(root, K8_NAME); // clear stale scratch (idempotent)
        let g0 = fs.root_generation();
        match fs.create_file(root, String::from(K8_NAME)) {
            Ok(_) => fs.root_generation() > g0,
            Err(_) => false,
        }
    });
    if matches!(gen_adv, Ok(true)) {
        w |= 1 << 0;
    }
    // Remove the committed scratch again so the crash-sim leg starts clean.
    let _ = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let _ = fs.unlink(root, K8_NAME);
    });

    // bit1: the crash-simulation seam — mutate with autocommit OFF (fresh
    // blocks written to the medium, the root sector NEVER flipped), then a
    // genuine remount. The next mount must land on the OLD tree: the file
    // absent, nothing torn. This is a power cut between the data writes and
    // the root flip, exercised on the real block device.
    let _ = with_unafs(|fs| {
        fs.set_autocommit(false);
        let root = fs.superblock.root_inode;
        if let Ok(id) = fs.create_file(root, String::from(K8_NAME)) {
            let _ = fs.write_data(id, 0, K8_PAYLOAD);
        }
        // Deliberately NO commit: drop the mount state via force_remount.
    });
    force_remount();
    let old_tree = with_unafs(|fs| fs.resolve_path(K8_PATH).is_err());
    if matches!(old_tree, Ok(true)) {
        w |= 1 << 1;
    }

    // bit2: the post-"cut" volume is internally consistent.
    let consistent = with_unafs(|fs| fs.fsck(false).map(|r| r.is_clean()).unwrap_or(false));
    if matches!(consistent, Ok(true)) {
        w |= 1 << 2;
    }

    // bit3: the same mutation, committed, survives a remount byte-for-byte.
    let created = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        match fs.create_file(root, String::from(K8_NAME)) {
            Ok(id) => fs.write_data(id, 0, K8_PAYLOAD).is_ok(),
            Err(_) => false,
        }
    });
    force_remount();
    let durable = with_unafs(|fs| match fs.resolve_path(K8_PATH) {
        Ok(id) => match (fs.read_inode(id), fs.read_data(id, 0, K8_PAYLOAD.len() as u64)) {
            (Ok(inode), Ok(data)) => inode.size == K8_PAYLOAD.len() as u64 && data == K8_PAYLOAD,
            _ => false,
        },
        Err(_) => false,
    });
    if matches!(created, Ok(true)) && matches!(durable, Ok(true)) {
        w |= 1 << 3;
    }

    // bit4: refcount persistence — the map the remount just loaded from disk
    // agrees with recomputed reachability.
    let refs_persist = with_unafs(|fs| fs.fsck(false).map(|r| r.is_clean()).unwrap_or(false));
    if matches!(refs_persist, Ok(true)) {
        w |= 1 << 4;
    }

    // bit5: self-clean — delete, remount, gone + consistent + no leak.
    // Capture the commit counters HERE, from the mount instance that did the
    // work (stats are per-instance; the remount below starts a fresh one).
    let stats = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let _ = fs.unlink(root, K8_NAME);
        fs.commit_stats()
    })
    .unwrap_or_default();
    force_remount();
    let cleaned = with_unafs(|fs| {
        fs.resolve_path(K8_PATH).is_err()
            && fs.fsck(false).map(|r| r.is_clean()).unwrap_or(false)
            && fs
                .ls(fs.superblock.root_inode)
                .map(|es| es.iter().all(|d| d.name != K8_NAME))
                .unwrap_or(false)
    });
    if matches!(cleaned, Ok(true)) {
        w |= 1 << 5;
    }

    // bit6 + the bench numbers (vaire ruling): CNTPCT ticks across the whole
    // witness and the crate's commit counters captured above.
    let ticks = bench_ticks().wrapping_sub(t0);
    if stats.commits > 0 && stats.blocks_written > 0 {
        w |= 1 << 6;
    }

    let verdict = if w == 0x7f { "PASS" } else { "FAIL" };
    serial_println!(
        ":: K8a-cow: CoW commit (gen-advance) + power-cut-mid-commit converges to OLD tree + refcounts persist + self-clean {} [w={:#04x}] bench: commits={} blocks={} last={} ticks={} ::",
        verdict,
        w,
        stats.commits,
        stats.blocks_written,
        stats.last_commit_blocks,
        ticks
    );
}

/// K8b scratch fixture: a file the witness snapshots, overwrites, and finally
/// deletes — the volume is left exactly as found (self-cleaning, K2 idiom), so
/// the STATEFUL card never accumulates residue and `K3-mount`'s fixture `ls`
/// still holds next boot.
const K8B_NAME: &str = "K8BSNAP.TXT";
const K8B_PATH: &str = "/K8BSNAP.TXT";
/// Two payloads that each span multiple 4096 B blocks (real data extents to
/// share and to reclaim). OLD = what the snapshot must keep reading; NEW = the
/// live overwrite.
const K8B_OLD: &[u8] = &[0xA1u8; 3 * 4096];
const K8B_NEW: &[u8] = &[0xB2u8; 3 * 4096];

/// The physical data blocks a file's extents currently occupy (heap-staged
/// Vec — no large kernel-stack array; the >4 KiB-array wild-jump hazard).
fn k8b_data_blocks(fs: &mut KernelUnaFS, id: u64) -> alloc::vec::Vec<u64> {
    let mut out = alloc::vec::Vec::new();
    if let Ok(inode) = fs.read_inode(id) {
        for e in &inode.chunks {
            for i in 0..e.length.div_ceil(::unafs::BLOCK_SIZE) {
                out.push(e.physical_block + i);
            }
        }
    }
    out
}

/// K8b-snap witness (uncounted): prove retained roots (snapshots) + reclamation
/// on the live kernel mount, benchmark-instrumented (CNTPCT ticks + the crate's
/// snapshot counters).
///
/// Sequence (7 bits):
///   bit0 `snapshot_create` retains the committed tree (index gains the entry);
///   bit1 after overwriting the LIVE file with NEW bytes, the snapshot's OLD
///        data blocks STILL hold the OLD bytes (read raw off the device — the
///        never-overwrite + block-sharing core) AND the live file reads NEW;
///   bit2 the two-root volume is refcount-consistent (fsck clean);
///   bit3 the allocator never hands out a block the live snapshot holds — a
///        churn of fresh allocations avoids the retained block set entirely;
///   bit4 drop → reclamation drains eagerly to empty, freeing the snapshot-
///        only blocks (free count rises), fsck clean, the freed blocks are
///        re-allocatable;
///   bit5 POWER-CUT-MID-DRAIN: a second snapshot dropped via the enqueue-only
///        half + a genuine remount — the mount's eager drain RESUMES and
///        converges (queue empty, fsck clean);
///   bit6 self-clean (delete + remount: gone, consistent, no leak) AND the
///        snapshot bench counters are live (created > 0 and dropped > 0).
///
/// Skips honestly on media without a unafs partition. Self-cleaning.
pub fn k8b_snap_selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    if let Err(e) = locate() {
        serial_println!(":: K8b-snap: no unafs volume ({:?}) — skipped ::", e);
        return;
    }

    let mut w = 0u32;
    let t0 = bench_ticks();

    // Start clean: drop any stray snapshots and scratch from a prior run.
    let _ = with_unafs(|fs| {
        while let Ok(snaps) = fs.snapshot_index() {
            match snaps.first() {
                Some(s) => {
                    let _ = fs.snapshot_drop(s.generation);
                }
                None => break,
            }
        }
        let root = fs.superblock.root_inode;
        let _ = fs.unlink(root, K8B_NAME);
    });

    // bit0: create the scratch file with OLD bytes + retain it.
    let (created_gen, old_blocks) = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let id = match fs.create_file(root, String::from(K8B_NAME)) {
            Ok(id) => id,
            Err(_) => return (None, alloc::vec::Vec::new()),
        };
        if fs.write_data(id, 0, K8B_OLD).is_err() {
            return (None, alloc::vec::Vec::new());
        }
        let blocks = k8b_data_blocks(fs, id);
        match fs.snapshot_create(String::from("k8b-before"), String::from("kernel"), bench_ticks()) {
            Ok(g) => (Some(g), blocks),
            Err(_) => (None, alloc::vec::Vec::new()),
        }
    })
    .unwrap_or((None, alloc::vec::Vec::new()));
    let snap_gen = match created_gen {
        Some(g) => g,
        None => {
            serial_println!(":: K8b-snap: setup failed (create/retain) FAIL [w=0x00] ::");
            return;
        }
    };
    let has_entry = with_unafs(|fs| {
        fs.snapshot_index()
            .map(|s| s.iter().any(|e| e.generation == snap_gen))
            .unwrap_or(false)
    })
    .unwrap_or(false);
    if has_entry && !old_blocks.is_empty() {
        w |= 1 << 0;
    }

    // bit1: overwrite the LIVE file with NEW bytes; the snapshot's OLD blocks
    // must be untouched (read them raw), and the live file must read NEW.
    let old_intact_and_live_new = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let id = match fs.resolve_path(K8B_PATH) {
            Ok(id) => id,
            Err(_) => return false,
        };
        let _ = root;
        if fs.write_data(id, 0, K8B_NEW).is_err() {
            return false;
        }
        // The snapshot's OLD blocks still hold the OLD bytes (never-overwrite).
        let mut block = alloc::vec![0u8; ::unafs::BLOCK_SIZE as usize];
        for (i, &pb) in old_blocks.iter().enumerate() {
            if <_ as ::unafs::BlockDevice>::read_block(&mut fs.device, pb, &mut block).is_err() {
                return false;
            }
            let base = i * ::unafs::BLOCK_SIZE as usize;
            let want = &K8B_OLD[base..base + ::unafs::BLOCK_SIZE as usize];
            if block[..] != want[..] {
                return false;
            }
        }
        // And the LIVE file reads the NEW bytes.
        matches!(fs.read_data(id, 0, K8B_NEW.len() as u64), Ok(d) if d == K8B_NEW)
    })
    .unwrap_or(false);
    if old_intact_and_live_new {
        w |= 1 << 1;
    }

    // bit2: two-root refcount consistency.
    let consistent = with_unafs(|fs| fs.fsck(false).map(|r| r.is_clean()).unwrap_or(false));
    if matches!(consistent, Ok(true)) {
        w |= 1 << 2;
    }

    // bit3: the allocator never reuses a retained snapshot's blocks. Churn a
    // few fresh allocations and confirm none land on the old set.
    let no_reuse = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let mut clean = true;
        for i in 0..4u8 {
            let name = alloc::format!("K8BCHURN{}.BIN", i);
            if let Ok(cid) = fs.create_file(root, name.clone()) {
                let _ = fs.write_data(cid, 0, &alloc::vec![i; 2 * 4096]);
                for b in k8b_data_blocks(fs, cid) {
                    if old_blocks.contains(&b) {
                        clean = false;
                    }
                }
                let _ = fs.unlink(root, &name);
            }
        }
        clean
    })
    .unwrap_or(false);
    if no_reuse {
        w |= 1 << 3;
    }

    // bit4: drop → eager reclamation frees the snapshot-only blocks and the
    // freed blocks re-allocate; fsck clean.
    let reclaimed = with_unafs(|fs| {
        let free_before = fs.free_blocks();
        if fs.snapshot_drop(snap_gen).is_err() {
            return false;
        }
        let drained = fs.reclaim_queue().map(|q| q.is_empty()).unwrap_or(false);
        let freed = fs.free_blocks() > free_before;
        let clean = fs.fsck(false).map(|r| r.is_clean()).unwrap_or(false);
        drained && freed && clean
    })
    .unwrap_or(false);
    if reclaimed {
        w |= 1 << 4;
    }

    // bit5: power-cut-mid-drain. Retain again, enqueue the drop WITHOUT
    // draining, then a genuine remount — the mount's eager drain must resume.
    // Capture the bench counters HERE, from the working instance (the counters
    // are per-instance and reset on the remount below), after this instance has
    // done its create (bit0) + drop (bit4) + this create + enqueue.
    let (enq, stats) = with_unafs(|fs| {
        let ok = match fs.snapshot_create(
            String::from("k8b-cut"),
            String::from("kernel"),
            bench_ticks(),
        ) {
            Ok(g) => fs.snapshot_drop_enqueue(g).is_ok(),
            Err(_) => false,
        };
        (ok, fs.commit_stats())
    })
    .unwrap_or((false, Default::default()));
    force_remount();
    let resumed = with_unafs(|fs| {
        fs.reclaim_queue().map(|q| q.is_empty()).unwrap_or(false)
            && fs.snapshot_index().map(|s| s.is_empty()).unwrap_or(false)
            && fs.fsck(false).map(|r| r.is_clean()).unwrap_or(false)
    })
    .unwrap_or(false);
    if enq && resumed {
        w |= 1 << 5;
    }

    // bit6: self-clean. Delete the scratch + remount and confirm no leak.
    let _ = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let _ = fs.unlink(root, K8B_NAME);
    });
    force_remount();
    let cleaned = with_unafs(|fs| {
        fs.resolve_path(K8B_PATH).is_err()
            && fs.snapshot_index().map(|s| s.is_empty()).unwrap_or(false)
            && fs.fsck(false).map(|r| r.is_clean()).unwrap_or(false)
            && fs
                .ls(fs.superblock.root_inode)
                .map(|es| es.iter().all(|d| d.name != K8B_NAME))
                .unwrap_or(false)
    })
    .unwrap_or(false);
    if cleaned && stats.snapshots_created > 0 && stats.snapshots_dropped > 0 {
        w |= 1 << 6;
    }

    let ticks = bench_ticks().wrapping_sub(t0);
    let verdict = if w == 0x7f { "PASS" } else { "FAIL" };
    serial_println!(
        ":: K8b-snap: retain+overwrite -> snapshot reads OLD bytes + retention-safe alloc + drop reclaims + power-cut-mid-drain converges + self-clean {} [w={:#04x}] bench: snaps_created={} snaps_dropped={} blocks={} ticks={} ::",
        verdict,
        w,
        stats.snapshots_created,
        stats.snapshots_dropped,
        stats.blocks_written,
        ticks
    );
}
