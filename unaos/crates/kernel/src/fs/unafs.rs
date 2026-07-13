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

//! BeFS-K3: read-only kernel mount of a native UnaFS volume.
//!
//! Wires the kernel's 512 B block layer ([`crate::drivers::block`]) into the
//! `unafs` crate's K2 seam: [`SdSectorDevice`] implements
//! [`unafs::adapter::SectorDevice`] over `read_block`, `locate_unafs` finds the
//! UnaFS partition by superblock magic, and [`mount`] returns a live
//! `UnaFS<BlockAdapter<SdSectorDevice>>`. Arch-neutral like `fs::fat` — it
//! builds on the generic block layer only — though today only the Pi 4 media
//! carries a UnaFS partition.
//!
//! **Read-only arc:** `write_sector` is a deliberate `Io` stub (K4 makes it
//! real), so no code path — not even a torn-journal recovery — can touch the
//! medium through this mount.

use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicBool, Ordering};

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

    fn write_sector(&mut self, _lba: u64, _buf: &[u8]) -> Result<(), SectorError> {
        // K3 is a read-only mount: refuse every write at the seam. K4 (journaled
        // writes) replaces this with the real write path.
        Err(SectorError::Io(String::from(
            "unafs mount is read-only (K3); writes land in K4",
        )))
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

/// Locate and mount the UnaFS volume, read-only.
///
/// Like `fs::fat::mount`, this constructs a fresh mount per call: the volume is
/// immutable through this path (the write seam is stubbed), so per-call mounts
/// cannot diverge from each other or the disk.
pub fn mount() -> Result<KernelUnaFS, MountError> {
    install_warn_hook();
    let span = locate()?;
    let dev = SdSectorDevice::open()?;
    let adapter = BlockAdapter::for_partition(dev, &span);
    UnaFS::mount(adapter).map_err(MountError::Fs)
}

/// The K3HELLO.TXT fixture contents, byte-pinned against what `arroyo kernel8`
/// stages into the unafs volume.
const K3_HELLO: &[u8] = b"Hello from native UnaFS on the Pi 4!\n";
/// K3PAT.BIN: 12 KiB, byte i = (i*7+3)&0xFF — three unafs blocks, so reading it
/// walks extents, not just a single block.
const K3_PAT_LEN: u64 = 12288;

/// K3-mount witness (M1: locate + mount + superblock sanity + RO seam proof;
/// M2: root `ls` + byte-verified reads through resolve/extent walking).
///
/// Called at the tail of the aarch64 `u7_launcher` fixture chain (the
/// `k4_ready_selftest` idiom): one-shot, read-only, and its serial evidence is
/// the uncounted `:: K3-mount: … ::` line — never a `-> PASS` fixture line, so
/// the 23-PASS battery stays byte-equivalent. On media without a UnaFS
/// partition it reports a skip, not a failure.
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

    match mount() {
        Ok(mut fs) => {
            // bit1: superblock magic is the frozen on-disk signature.
            if fs.superblock.magic == ::unafs::superblock::MAGIC {
                w |= 1 << 1;
            }
            // bit2: version + block size are the pinned format constants.
            if fs.superblock.version == ::unafs::superblock::VERSION
                && fs.superblock.block_size as u64 == ::unafs::BLOCK_SIZE
            {
                w |= 1 << 2;
            }
            // bit3: the volume fits the partition that carries it.
            if fs.superblock.block_count <= span.block_count {
                w |= 1 << 3;
            }

            // --- M2: read paths, byte-verified against the staged fixtures. ---

            // bit5: `ls /` sees exactly the two staged fixture FILES.
            if let Ok(entries) = fs.ls(fs.superblock.root_inode) {
                let hello = entries
                    .iter()
                    .find(|e| e.name == "K3HELLO.TXT" && e.kind == ::unafs::FileKind::File);
                let pat = entries
                    .iter()
                    .find(|e| e.name == "K3PAT.BIN" && e.kind == ::unafs::FileKind::File);
                if entries.len() == 2 && hello.is_some() && pat.is_some() {
                    w |= 1 << 5;
                }
            }

            // bit6: resolve + read K3HELLO.TXT — every byte matches the pinned text.
            if let Ok(id) = fs.resolve_path("/K3HELLO.TXT") {
                if let (Ok(inode), Ok(data)) =
                    (fs.read_inode(id), fs.read_data(id, 0, K3_HELLO.len() as u64 + 8))
                {
                    if inode.size == K3_HELLO.len() as u64 && data == K3_HELLO {
                        w |= 1 << 6;
                    }
                }
            }

            // bit7: K3PAT.BIN — all 12 KiB match the (i*7+3)&0xFF pattern, so the
            // read crossed unafs block (and extent) boundaries intact.
            if let Ok(id) = fs.resolve_path("/K3PAT.BIN") {
                if let (Ok(inode), Ok(data)) = (fs.read_inode(id), fs.read_data(id, 0, K3_PAT_LEN))
                {
                    if inode.size == K3_PAT_LEN
                        && data.len() as u64 == K3_PAT_LEN
                        && data
                            .iter()
                            .enumerate()
                            .all(|(i, &b)| b == ((i * 7 + 3) & 0xFF) as u8)
                    {
                        w |= 1 << 7;
                    }
                }
            }

            // bit8: a missing name refuses to resolve (negative witness).
            if fs.resolve_path("/K3NOPE.TXT").is_err() {
                w |= 1 << 8;
            }
        }
        Err(e) => {
            serial_println!(":: K3-mount: located but mount FAILED ({:?}) ::", e);
            return;
        }
    }

    // bit4: the RO seam holds — a raw write at the SectorDevice is refused.
    if let Ok(mut dev) = SdSectorDevice::open() {
        let sector = [0u8; 512];
        if matches!(
            dev.write_sector(span.base_lba, &sector),
            Err(SectorError::Io(_))
        ) {
            w |= 1 << 4;
        }
    }

    let verdict = if w == 0x1ff { "PASS" } else { "FAIL" };
    serial_println!(
        ":: K3-mount: native unafs volume located (base_lba={}, {} blocks) + superblock v{} mounted RO + ls/cat byte-verified {} [w={:#05x}] ::",
        span.base_lba,
        span.block_count,
        ::unafs::superblock::VERSION,
        verdict,
        w
    );
}
