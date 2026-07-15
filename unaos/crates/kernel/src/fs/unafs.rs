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
//! **BeFS-K4 (writable) — the coherence keystone.** `write_sector` now routes
//! to [`crate::drivers::block::write_block`] (emmc2 CMD24, R1/CMD13-hardened),
//! so the mount is read-write. K3 mounted per call, which is safe only while
//! the volume is immutable; writes make a per-call mount a COHERENCE HAZARD
//! (two live mounts = two independent in-RAM allocation bitmaps + journal
//! cursors, so a block one frees the other can re-hand-out → corruption).
//! Every access — read AND write — therefore flows through the single,
//! process-wide, IRQ-masked mount [`with_unafs`] (one authoritative in-RAM
//! bitmap/journal, all operations serialized; modelled on the F3 `NAMESPACE`
//! lock). Keeping one mount live also means a pure read never triggers the
//! crate's `Drop`-time `sync_metadata` write-back, so reads stay genuinely
//! read-only.
//!
//! **Torn-write scope (honest).** Crash safety comes from write ORDERING
//! inside the `unafs` crate (new-extents-first, single-block metadata swap,
//! free-last → a power cut LEAKS blocks, never dangles a reference), NOT from
//! journal replay: the WAL is intent-logging only (dirty detection, no
//! rollback). The "single-block swap is atomic" claim holds at the crate's
//! 4096 B block granularity; the medium writes 512 B sectors, so one
//! `write_block` is eight non-atomic `write_sector`s — a swap is truly atomic
//! only when the live record fits the first 512 B sector. See
//! `docs/SECURITY.md` §K4 for the full scope.

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
// dangles). The K3 two-phase (durable-first) and K5 (ns-spanned snapshot+write) orderings are
// PRESERVED by the caller in `syscall.rs`: those callers hold the FAT `NAMESPACE` lock across their
// OWNED_FILES snapshot AND this native write, so the lock still fuses the two exactly as it did for the
// sidecar; the store the write targets is immaterial to that serialization. Lock order gains one edge:
// `NAMESPACE ⊃ MOUNT` (no path ever takes MOUNT then NAMESPACE).

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
    let owner_s = match core::str::from_utf8(owner) {
        Ok(s) => s,
        Err(_) => return false,
    };
    with_unafs(|fs| {
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
    })
    .unwrap_or(false)
}

/// K6: read back the ACL row for `(dir_lba, dir_off)`, or `None` if there is no row (public) or the
/// mount fails. The read is pure (no metadata write-back through the coherent mount). Used by the
/// migration read-back verify and by a native single-row lookup.
pub fn native_acl_read(dir_lba: u64, dir_off: u32) -> Option<NativeAclRow> {
    with_unafs(|fs| {
        let fname = acl_file_name(dir_lba, dir_off);
        let id = fs.resolve_path(&format!("/{}", fname)).ok()?;
        native_acl_row_of(fs, id)
    })
    .ok()
    .flatten()
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
    with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let fname = acl_file_name(dir_lba, dir_off);
        match fs.unlink(root, &fname) {
            Ok(_) => true,
            Err(ref e) if is_absent(e) => true, // already public on disk
            Err(_) => false,
        }
    })
    .unwrap_or(false)
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
///   bit1 force a real remount (fresh in-RAM bitmap/journal, re-read from disk);
///   bit2 resolve + read back — exact bytes match (WRITE DURABILITY);
///   bit3 delete it, then reset the (now-clean) journal head;
///   bit4 remount again — the name no longer resolves (DELETE DURABILITY);
///   bit5 a missing nested path resolves to Err (negative path);
///   bit6 the fresh mount reports a CLEAN journal and root `ls` is back to the
///        staged fixtures (no K4TEST leak).
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

    // bit3: delete the scratch file, then reset the (now-clean) journal head so
    // the next mount's dirty-detection is unambiguously clean. `journal.reset`
    // and `device` are disjoint fields, so the borrow splits cleanly.
    let deleted = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        if fs.unlink(root, K4_NAME).is_err() {
            return false;
        }
        fs.journal.reset(&mut fs.device).is_ok()
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

    // bit6: the fresh mount is CLEAN (balanced/reset journal) and the root is
    // back to exactly the staged fixtures — no K4TEST leak.
    let clean = with_unafs(|fs| {
        let dirty = fs.journal.check_recovery(&mut fs.device).unwrap_or(true);
        let no_leak = fs
            .ls(fs.superblock.root_inode)
            .map(|entries| entries.iter().all(|d| d.name != K4_NAME))
            .unwrap_or(false);
        !dirty && no_leak
    });
    if matches!(clean, Ok(true)) {
        w |= 1 << 6;
    }

    let verdict = if w == 0x7f { "PASS" } else { "FAIL" };
    serial_println!(
        ":: K4-write: create+write {} + remount byte-verify + delete + remount + negative + clean-journal {} [w={:#04x}] ::",
        K4_PATH,
        verdict,
        w
    );
}
