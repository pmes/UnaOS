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

//! CHARTER: Kernel — fs-core
//!
//! EXFAT (rmbp-ledger B392, R79, R95) — a removable exFAT medium is a read-only volume while it is
//! inserted.
//!
//! Flight 25: Peter's 64 GB card in the hub's USB reader was claimed and refused
//! (`[volumes] not mounted … fs=exfat part_type=7 reason=no-exfat-reader`): every SD card over
//! 32 GB is exFAT by the SD Association's rule, and the tree had no exFAT reader.
//!
//! The seam: the on-disk format lives ONCE, in `unaos/libs/fs/exfat_core` (no_std, the shared core
//! the host KATs also link). This file is only the kernel's half: the [`SectorRead`] adapter over a
//! USB registry entry, a per-disk mount cache (the up-case table is read once per insert, not once
//! per verb — the mount table is rebuilt per verb), the [`ExfatBackend`] the VFS mounts, and
//! `tests exfat`. `fs/removable.rs` calls [`probe`] when a disk carries no FAT volume and its boot
//! sector says `EXFAT`. Read-only: every write verb answers the trait's `Unsupported`, and
//! [`VfsBackend::write_veto`] names why (exFAT write is owed — docs/dev/evidence/rmbp-1005/exfat.md).

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use exfat_core::{Error, SectorRead, Volume};
use crate::sync::Mutex;

use crate::fs::fat::BlockSource;
use crate::fs::vfs::{volid_mix, DirEnt, NodeKind, Stat, VfsBackend, VfsError, VfsTime, VOLID_SEED};

/// One USB registry entry, read through xHCI in chunks the staging buffer holds.
struct UsbDev {
    ix: usize,
}

impl SectorRead for UsbDev {
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> exfat_core::Result<()> {
        let max = crate::drivers::block::MAX_BLOCKS_PER_OP.max(1) * exfat_core::SECTOR;
        for (k, chunk) in buf.chunks_mut(max).enumerate() {
            let at = lba + (k * (max / exfat_core::SECTOR)) as u64;
            match crate::drivers::block::read_blocks_usb_ix(self.ix, at, chunk) {
                Ok(_) => {}
                Err(crate::drivers::block::BlockError::Busy) => return Err(Error::Busy),
                Err(_) => return Err(Error::Io),
            }
        }
        Ok(())
    }
}

/// A mounted volume, keyed by the registry entry AND the enumerator's identity, so a replug (a new
/// slot) or a different disk on a reused index never reads through a stale geometry.
struct Cached {
    ix: usize,
    slot_id: u8,
    num_blocks: u64,
    vol: Arc<Volume>,
}

static CACHE: Mutex<Vec<Cached>> = Mutex::new(Vec::new());

fn ix_of(source: BlockSource) -> Option<usize> {
    match source {
        BlockSource::Usb => Some(0),
        BlockSource::UsbN(n) => Some(n as usize),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

/// The volume on USB registry entry `ix`: from the cache when the same disk is still there, else
/// located (superfloppy, GPT, MBR) and mounted. No lock is held across a sector read.
fn mount_ix(ix: usize) -> Result<Arc<Volume>, Error> {
    let d = crate::drivers::block::usb_info_ix(ix).ok_or(Error::Io)?;
    if d.block_size != exfat_core::SECTOR as u32 {
        return Err(Error::Unsupported);
    }
    if let Some(c) = CACHE.lock().iter().find(|c| c.ix == ix && c.slot_id == d.slot_id && c.num_blocks == d.num_blocks) {
        return Ok(c.vol.clone());
    }
    let dev = UsbDev { ix };
    let (lba, blocks, _slot) = exfat_core::locate(&dev, d.num_blocks)?;
    let vol = Arc::new(Volume::mount(&dev, lba, blocks)?);
    let mut c = CACHE.lock();
    c.retain(|c| c.ix != ix);
    c.push(Cached { ix, slot_id: d.slot_id, num_blocks: d.num_blocks, vol: vol.clone() });
    Ok(vol)
}

/// The wire's name for a refusal (`reason=` on `[volumes] not mounted`).
pub fn reason(e: Error) -> &'static str {
    match e {
        Error::Io => "exfat-io",
        Error::Busy => "exfat-busy",
        Error::NotExfat => "exfat-not-found",
        Error::BootChecksum => "exfat-boot-checksum",
        Error::UpcaseChecksum => "exfat-upcase-checksum",
        Error::Corrupt(r) => r,
        Error::NotFound => "not-found",
        Error::NotADirectory => "not-a-directory",
        Error::IsADirectory => "is-a-directory",
        Error::Unsupported => "exfat-unsupported",
    }
}

/// The name a label gives under `/volumes`: the label entry's UTF-16 characters (§7.3), with the
/// path separator and control characters substituted; an empty or all-substituted label is
/// `Untitled` (the FAT rule, `bootdisk::sanitize_label`).
fn volume_name(label: &str) -> String {
    let mut out = String::new();
    let mut kept = false;
    for c in label.chars() {
        if c == '/' || c == '\\' || c.is_control() {
            out.push('_');
        } else {
            out.push(c);
            kept = true;
        }
    }
    let t = out.trim();
    if t.is_empty() || !kept || t == "." || t == ".." {
        return String::from(crate::fs::bootdisk::UNTITLED);
    }
    String::from(t)
}

/// `fs/removable.rs`'s exFAT arm: mount the disk at registry entry `ix` and return the name its
/// label gives (made unique by the caller), or the refusal's wire reason.
pub fn probe(ix: usize) -> Result<String, &'static str> {
    mount_ix(ix).map(|v| volume_name(v.label())).map_err(reason)
}

fn vfs_err(e: Error) -> VfsError {
    match e {
        Error::NotFound => VfsError::NoSuchPath,
        Error::NotADirectory => VfsError::NotADirectory,
        Error::IsADirectory => VfsError::IsADirectory,
        other => VfsError::Backend(reason(other)),
    }
}

fn vfs_time(s: &exfat_core::Stamp) -> Option<VfsTime> {
    if s.is_unset() {
        return None;
    }
    Some(VfsTime { year: s.year, month: s.month, day: s.day, hour: s.hour, min: s.min, sec: s.sec })
}

/// A mounted exFAT volume. World-readable (a removable medium's contents are meant to be read),
/// read-only by construction.
pub struct ExfatBackend {
    volume: String,
    source: BlockSource,
}

impl ExfatBackend {
    pub fn new(volume: &str, source: BlockSource) -> Self {
        Self { volume: String::from(volume), source }
    }

    fn open(&self) -> Result<(Arc<Volume>, UsbDev), VfsError> {
        let ix = ix_of(self.source).ok_or(VfsError::Unsupported)?;
        let v = mount_ix(ix).map_err(vfs_err)?;
        Ok((v, UsbDev { ix }))
    }
}

impl VfsBackend for ExfatBackend {
    fn volume_name(&self) -> &str {
        &self.volume
    }

    /// VOLID: `(vfs:exfat:, BLOCK SOURCE, VolumeSerialNumber, ClusterCount)` — the filesystem tag
    /// first, so an exFAT volume never compares equal to a FAT one on the same device.
    fn volume_id(&self) -> Option<u64> {
        let (v, _) = self.open().ok()?;
        let h = volid_mix(VOLID_SEED, b"vfs:exfat:");
        let h = volid_mix(h, self.source.name().as_bytes());
        let h = volid_mix(h, &v.serial().to_le_bytes());
        Some(volid_mix(h, &v.cluster_count().to_le_bytes()))
    }

    fn read_dir(&self, rel: &str) -> Result<Vec<DirEnt>, VfsError> {
        let (v, dev) = self.open()?;
        let n = v.lookup(&dev, rel).map_err(vfs_err)?;
        if !n.is_dir() {
            return Err(VfsError::NotADirectory);
        }
        let l = v.read_dir(&dev, &n).map_err(vfs_err)?;
        Ok(l.nodes
            .iter()
            .map(|e| DirEnt {
                name: e.name.clone(),
                kind: if e.is_dir() { NodeKind::Dir } else { NodeKind::File },
                size: if e.is_dir() { 0 } else { e.data_len },
                mtime: vfs_time(&e.mtime),
            })
            .collect())
    }

    fn stat(&self, rel: &str) -> Result<Stat, VfsError> {
        let (v, dev) = self.open()?;
        let n = v.lookup(&dev, rel).map_err(vfs_err)?;
        Ok(Stat {
            kind: if n.is_dir() { NodeKind::Dir } else { NodeKind::File },
            size: if n.is_dir() { 0 } else { n.data_len },
            id: None,
            mtime: vfs_time(&n.mtime).map(|t| t.unix_secs()),
        })
    }

    fn read(&self, rel: &str, offset: u64, len: usize) -> Result<Vec<u8>, VfsError> {
        let (v, dev) = self.open()?;
        let n = v.lookup(&dev, rel).map_err(vfs_err)?;
        v.read(&dev, &n, offset, len).map_err(vfs_err)
    }

    fn authorize_read(&self, _rel: &str, _principal: &str) -> Result<(), VfsError> {
        Ok(())
    }

    fn write_veto(&self) -> Option<&'static str> {
        Some("exfat is read-only (write owed, B392)")
    }

    fn volume_bytes(&self) -> Option<u64> {
        self.open().ok().map(|(v, _)| v.volume_bytes())
    }

    fn describe(&self) -> Option<String> {
        let (v, _) = self.open().ok()?;
        Some(alloc::format!(
            "exfat label={} serial={:08x} clusters={} cluster_bytes={} backup_boot={}",
            v.label(),
            v.serial(),
            v.cluster_count(),
            v.cluster_bytes(),
            v.backup_boot as u8
        ))
    }
}

/// `tests exfat` — the shared core's spec checklist (`Volume::audit`, the same function the host
/// KATs run on the `mkfs.exfat` fixture) against the first mounted exFAT removable. Never at boot
/// (R80): registered by `fs/removable.rs`'s first service pass, fired only by the verb.
pub fn selftest() {
    let Some((ix, name)) = crate::fs::removable::exfat_mounts().into_iter().next() else {
        serial_println!(":: EXFAT: kat=0/0 mounted=- files=0 -> SKIP :: reason=no-exfat-medium ::");
        return;
    };
    let v = match mount_ix(ix) {
        Ok(v) => v,
        Err(e) => {
            serial_println!(":: EXFAT: kat=0/0 mounted={} files=0 -> FAIL :: reason={} ::", name, reason(e));
            return;
        }
    };
    let a = v.audit(&UsbDev { ix });
    let ok = a.passed() == a.total();
    serial_println!(
        ":: EXFAT: kat={}/{} mounted={} files={} -> {} :: fail={} used={} clusters={} cluster_bytes={} backup_boot={} ::",
        a.passed(),
        a.total(),
        name,
        a.files,
        if ok { "PASS" } else { "FAIL" },
        a.first_fail().unwrap_or("-"),
        a.used_clusters,
        v.cluster_count(),
        v.cluster_bytes(),
        v.backup_boot as u8
    );
}
