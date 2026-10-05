// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Amber Bytes — shared-core
//!
//! SELFINSTALL2 M2 (rmbp-ledger B310, ROADMAP SH-3): the UnaFS leg of `install ssd --write` — the
//! card's p2 (the UnaFS system volume UNAFSX86 laid) is mirrored onto the SSD's p2 by a SECTOR COPY of
//! the volume span described by an `amber_core::ClonePlan`, then the copy is mounted through the write
//! grant and checked with the unafs crate's own `fsck` (no_std, the same pass `tools/unafs fsck` runs).
//!
//! The partition arithmetic, the clone plan and its ETA live in the shared core (`amber_core`); this
//! file is only the kernel's I/O for them: reading the running volume through the very
//! `SdSectorDevice` the live mount reads through, and writing through the caller's granted
//! `InstallTarget`.
//!
//! CONSISTENCY OF A LIVE COPY. The source is the mounted root, so it may commit while the copy runs.
//! UnaFS is copy-on-write: a transaction writes only FREE blocks and its single atomic point is the
//! root-slot flip in block 1 (`unafs::root`, slots A/B = the first two sectors of that block). So if
//! block 1's bytes are identical before and after the copy, no commit happened in between and every
//! block reachable from the copied root was copied unchanged (writes to free blocks are unreachable
//! residue). A changed fence FAILS the clone — it is never reported as a mirror.

use super::{InstallError, InstallTarget};
use crate::drivers::block;
use crate::fs::unafs as kunafs;
use ::unafs::adapter::{BlockAdapter, PartitionSpan, SectorDevice, SectorError};
use amber_core::ClonePlan;
use alloc::format;

/// The running system's UnaFS volume: the disk the live mount rides and the span on it.
#[derive(Clone, Copy)]
pub struct Source {
    pub handle: block::BlockHandle,
    pub span: PartitionSpan,
}

impl Source {
    /// The volume's extent in 512-byte sectors (whole 4 KiB blocks).
    pub fn sectors(&self) -> u64 {
        self.span.block_count * 8
    }
    pub fn is_ahci(&self) -> bool {
        matches!(self.handle, block::BlockHandle::Ahci { .. })
    }
}

/// Short wire name of a handle (`sdhc`, `usb`, `ahci`, `global`).
pub fn handle_name(h: block::BlockHandle) -> &'static str {
    match h {
        block::BlockHandle::Global => "global",
        block::BlockHandle::Usb => "usb",
        block::BlockHandle::Ahci { .. } => "ahci",
        #[allow(unreachable_patterns)]
        _ => "sdhc",
    }
}

/// The running UnaFS volume, if the kernel has one bound (binding it lazily if nothing has yet).
pub fn source() -> Option<Source> {
    let h = match kunafs::mount_bound_handle() {
        Some(h) => h,
        None => {
            let _ = kunafs::with_unafs(|_| ());
            kunafs::mount_bound_handle()?
        }
    };
    let span = kunafs::locate_on(h).ok()?;
    Some(Source { handle: h, span })
}

/// Measured read rate of the source medium in bytes/s, over `sectors` sectors from the volume start.
pub fn measure_rate(src: &Source, sectors: u64) -> Option<u64> {
    let mut dev = kunafs::SdSectorDevice::open_on(src.handle).ok()?;
    let mut buf = [0u8; 512];
    let t0 = crate::arch::ticks();
    for i in 0..sectors {
        dev.read_sector(src.span.base_lba + i, &mut buf).ok()?;
    }
    let ms = crate::arch::ticks().wrapping_sub(t0).max(1);
    Some(sectors * 512 * 1000 / ms)
}

fn fence(dev: &mut kunafs::SdSectorDevice, base: u64) -> Result<[u8; 1024], InstallError> {
    let mut f = [0u8; 1024];
    let root = base + ::unafs::root::ROOT_BLOCK * 8;
    dev.read_sector(root, &mut f[..512]).map_err(|_| InstallError::Io)?;
    dev.read_sector(root + 1, &mut f[512..]).map_err(|_| InstallError::Io)?;
    Ok(f)
}

/// Why a clone did not complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneError {
    /// A read or write failed (no claim about what landed).
    Io,
    /// The source committed during the copy (the root-slot fence moved): the copy is not a snapshot.
    SourceMoved,
}

/// Execute `cp`: copy the source span onto `t` in 64-sector chunks, fenced on the source's root block.
/// Returns the elapsed milliseconds.
pub fn clone_span<T: InstallTarget>(t: &mut T, src: &Source, cp: &ClonePlan) -> Result<u64, CloneError> {
    const CHUNK: u64 = 64;
    let mut dev = kunafs::SdSectorDevice::open_on(src.handle).map_err(|_| CloneError::Io)?;
    let before = fence(&mut dev, src.span.base_lba).map_err(|_| CloneError::Io)?;
    let t0 = crate::arch::ticks();
    let mut buf = alloc::vec![0u8; (CHUNK as usize) * 512];
    let mut done = 0u64;
    let mut next_report = cp.sectors / 8;
    for (s, d, n) in cp.chunks(CHUNK) {
        let b = &mut buf[..(n as usize) * 512];
        for i in 0..n as usize {
            dev.read_sector(s + i as u64, &mut b[i * 512..(i + 1) * 512]).map_err(|_| CloneError::Io)?;
        }
        t.write_sectors(d, b).map_err(|_| CloneError::Io)?;
        done += n;
        if done >= next_report {
            serial_println!("[install] unafs mirror {}/{} MiB", amber_core::sectors_mib(done), amber_core::sectors_mib(cp.sectors));
            next_report = next_report.saturating_add(cp.sectors / 8).max(done + 1);
        }
    }
    let after = fence(&mut dev, src.span.base_lba).map_err(|_| CloneError::Io)?;
    if before != after {
        return Err(CloneError::SourceMoved);
    }
    Ok(crate::arch::ticks().wrapping_sub(t0))
}

/// A granted `InstallTarget` seen as the unafs crate's `SectorDevice`, so the crate can mount the copy.
struct TargetSectors<'a, T: InstallTarget>(&'a mut T);

impl<T: InstallTarget> SectorDevice for TargetSectors<'_, T> {
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), SectorError> {
        self.0.read_sectors(lba, buf).map_err(|e| SectorError::Io(format!("install target: {e:?}")))
    }
    fn write_sector(&mut self, lba: u64, buf: &[u8]) -> Result<(), SectorError> {
        self.0.write_sectors(lba, buf).map_err(|e| SectorError::Io(format!("install target: {e:?}")))
    }
    fn sector_count(&self) -> u64 {
        self.0.capacity_sectors()
    }
}

/// Mount the copy at `first_lba` (`block_count` 4 KiB blocks) on the target and run `fsck(false)`.
/// `Ok(true)` = clean. The mount is the target's own (another disk than the live mount's), dropped here.
pub fn fsck_target<T: InstallTarget>(t: &mut T, first_lba: u64, block_count: u64) -> Result<bool, &'static str> {
    let span = PartitionSpan { base_lba: first_lba, block_count };
    let adapter = BlockAdapter::for_partition(TargetSectors(t), &span);
    let mut fs = ::unafs::UnaFS::mount(adapter).map_err(|_| "the copy did not mount")?;
    let r = fs.fsck(false).map_err(|_| "fsck errored")?;
    Ok(r.is_clean())
}

/// UNAFSGROW (rmbp-ledger B347) M2: grow the volume at `first_lba` on the target — inside a partition
/// span of `span_blocks` 4 KiB blocks — to `want` blocks (never below its current size: a volume
/// already that large is left as it is), through the unafs crate's own `UnaFS::grow` (the function
/// `tools/unafs grow` calls), then `fsck(false)` the result. Returns `(from, to, clean)`.
pub fn grow_target<T: InstallTarget>(t: &mut T, first_lba: u64, span_blocks: u64, want: u64) -> Result<(u64, u64, bool), &'static str> {
    let span = PartitionSpan { base_lba: first_lba, block_count: span_blocks };
    let adapter = BlockAdapter::for_partition(TargetSectors(t), &span);
    let mut fs = ::unafs::UnaFS::mount(adapter).map_err(|_| "the volume did not mount")?;
    let from = fs.superblock.block_count;
    let r = fs.grow(want.max(from)).map_err(|_| "the grow was refused")?;
    let clean = fs.fsck(false).map(|rep| rep.is_clean()).unwrap_or(false);
    Ok((from, r.to, clean))
}
