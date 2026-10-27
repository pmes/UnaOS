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
//! [`unafs::adapter::SectorDevice`] over the block layer,
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
//!
//! **SDSEAM — the device names its disk.** [`SdSectorDevice`] carries the
//! [`crate::drivers::block::BlockHandle`] it was opened on, and its reads, its
//! writes and its `sector_count` all dispatch on that one value; [`locate_on`]
//! and [`mount_on`] are the handle-named forms of [`locate`] / [`mount`], which
//! are now thin `BlockHandle::Global` wrappers. Before this, reads went to the
//! ambient backend and the size came from the ambient registry slot — two
//! independent answers to "which disk is this", and PI-FS-2 was the boot where
//! they disagreed. Every pi path still opens `Global`, whose arms are the
//! identical calls, so pi behaviour is unchanged; what changed is that a mount
//! can no longer be assembled from two devices.
//!
//! **UNAFSBIND — the mount cache names its disk too.** SDSEAM made a *device*
//! carry its handle; the process-wide [`MOUNT`] cache still assumed one: its
//! lazy bind called [`mount`] — a `BlockHandle::Global` wrapper — so a machine
//! whose unafs volume arrives on any OTHER handle (the orin's SdMmc card,
//! bound via SDSEAM's handle routing, is the motivating case) had a shell whose
//! [`with_unafs`] could never see its own volume. The cache entry is now a
//! [`BoundMount`] that STORES the [`block::BlockHandle`] its volume was mounted
//! from, and the lazy bind ([`bind_mount`]) discovers that handle: `Global` is
//! attempted first and wins whenever it holds a volume — every pi path is
//! byte-for-byte the old behaviour — and only when the global path demonstrably
//! has no unafs volume are the other handles probed, in enum order, gated by
//! the exhaustive [`bind_probe_admitted`]. One `[unafsbind]` witness line at
//! bind time names the handle the mount rode.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use ::unafs::inode::AttributeValue;

use crate::sync::Mutex;

use crate::drivers::block::{self, BlockError};
use ::unafs::adapter::{
    BlockAdapter, PartError, PartitionSpan, SectorDevice, SectorError, locate_unafs,
};
use ::unafs::fs::FileSystemError;
use ::unafs::UnaFS;

/// The kernel block layer as a 512 B [`SectorDevice`], bound to ONE named block handle.
///
/// Constructed by [`SdSectorDevice::open_on`] only when the named handle holds a registered device
/// whose logical block size is exactly 512 B, so `read_sector`'s LBA space is that device's native
/// one with no scaling.
///
/// ### SDSEAM: the handle is carried, not assumed
/// Before this arc the struct held only `sectors`, its reads went to the ambient
/// [`block::read_block`], and its SIZE came from the ambient [`block::info`] (plus the PI-FS-2
/// emmc2 override). That is two independent guesses at "which disk is this", and the PI-FS-2 bug
/// was exactly the moment they disagreed: the global registry slot had been clobbered by a USB
/// mass-storage enumeration while `read_block` still routed to the microSD, so the SIZE GUARD and
/// the DATA PATH named different devices and the SD's own partition was rejected as out of bounds.
///
/// PI-FS-2 fixed that by pinning the size to the card. This arc fixes the CLASS: the device carries
/// the [`block::BlockHandle`] it was opened on, and reads, writes AND sizing all dispatch on that
/// one value. There is no longer any way for the three to name different disks, because there is
/// only one name.
///
/// The dispatch below is EXHAUSTIVE — no wildcard arm anywhere. `BlockHandle` is
/// total-by-construction in the block layer, so a handle added there is a compile error here (E0004)
/// rather than a silent mis-route into whatever the ambient backend happened to be. That forcing
/// function is the whole point: it is what makes a future handle correct at this seam BY
/// CONSTRUCTION instead of by someone remembering.
///
/// **Tegra note (`orin-unafs-root.md` §3 item 4).** The reason the tegra sizing arm was skipped
/// during TEGRASD is that on a tegra build the ambient `read_block` reaches the USB stick, so sizing
/// from `tegra_sd_info()` would have guarded STICK reads with the CARD's capacity — the inverse of
/// PI-FS-2. With the handle carried, that premise is gone: a device opened on
/// `BlockHandle::SdMmc` reads the card AND is sized from the card; one opened on
/// `BlockHandle::Global` reads the stick AND is sized from the stick. Neither can be built wrong.
/// See [`handle_info`] / [`handle_read`] / [`handle_write`] for the arm each handle contributes.
pub struct SdSectorDevice {
    /// Which registry handle this device reads, writes and is sized from — the single name.
    handle: block::BlockHandle,
    sectors: u64,
    /// BOOT80 (B350): the read-ahead window (one multi-block command's worth of sectors) — armed on
    /// the SD card's handle only, where a command costs what a sector does; window 0 elsewhere.
    ra: RaSet, // VFSTXCOST (B522): sixteen such windows, LRU, write-through — see `RaSet` at this file's tail
}

/// SDSEAM: the live geometry row for `handle`.
///
/// Exhaustive on purpose (see [`SdSectorDevice`]): every handle the block layer defines contributes
/// exactly one arm, and the arm names the same registry slot its read/write arms below route to.
///
/// ### MERGE NOTE — the `SdMmc` arms, WRITTEN AT THE TRUNK LANDING
/// `BlockHandle::SdMmc` and its entry points (`tegra_sd_info`, `read_block_tegra_sd`,
/// `write_block_tegra_sd`) live in `drivers/block.rs` and arrived with the orin track's TEGRASD
/// commit; they did not exist on the pi branch, and `drivers/block.rs` was not that arc's lane, so
/// the arms could not be written there. They were not guesswork either — the totality of these
/// matches made the merge report each missing arm as an E0004, and each is the single line named
/// below under the TEGRASD cfg triple
/// `#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]`. All four are now
/// applied verbatim, exactly as written here:
///
/// * [`handle_info`] — `BlockHandle::SdMmc => block::tegra_sd_info(),`
/// * [`handle_read`] — `BlockHandle::SdMmc => block::read_block_tegra_sd(lba, buf),`
/// * [`handle_write`] — `BlockHandle::SdMmc => block::write_block_tegra_sd(lba, buf),`
///   (which refuses in every cfg — the card is read-only outside `sdmmc_arm`, so a unafs write
///   attempt on it fails closed rather than reaching the medium)
/// * [`SdSectorDevice::open_on`] — `BlockHandle::SdMmc => dev.num_blocks,` (a dedicated slot;
///   no PI-FS-2 override, for the reason given there)
///
/// With those four lines the tegra sizing arm skipped during TEGRASD is correct BY CONSTRUCTION:
/// `dev` came from `tegra_sd_info()` and the reads it guards go to `read_block_tegra_sd`, so the
/// capacity and the bytes are the same card. That is precisely the property the ambient path could
/// not offer, and the reason the arm was right to be skipped until this seam existed.
fn handle_info(handle: block::BlockHandle) -> Option<block::BlockDeviceInfo> {
    match handle {
        block::BlockHandle::Global => block::info(),
        block::BlockHandle::Usb => block::usb_info(),
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        block::BlockHandle::Sdhc => block::sdhc_info(),
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        block::BlockHandle::SdMmc => block::tegra_sd_info(), #[cfg(all(target_arch = "x86_64", feature = "ahci"))] block::BlockHandle::Ahci { port } => block::ahci_info_port(port), // UNAFSX86: the SATA disk's own row
    }
}

/// SDSEAM: one absolute 512 B sector read, routed by handle.
///
/// `Global` keeps calling [`block::read_block`] — the ambient backend dispatcher — because that IS
/// what the global handle means: on the bare-metal Pi it routes to emmc2 for as long as the SD
/// backend is active, which is the behaviour every pi path depends on and which this arc preserves
/// unchanged. The other arms bypass the backend selector exactly as their block-layer twins do.
fn handle_read(
    handle: block::BlockHandle,
    lba: u64,
    buf: &mut [u8],
) -> Result<usize, BlockError> {
    match handle {
        block::BlockHandle::Global => block::read_block(lba, buf),
        block::BlockHandle::Usb => block::read_block_usb(lba, buf),
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        block::BlockHandle::Sdhc => block::read_block_sdhc(lba, buf),
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        block::BlockHandle::SdMmc => block::read_block_tegra_sd(lba, buf), #[cfg(all(target_arch = "x86_64", feature = "ahci"))] block::BlockHandle::Ahci { port } => block::read_block_ahci_port(port, lba, buf), // UNAFSX86
    }
}

/// SDSEAM: one absolute 512 B sector write, routed by handle. The twin of [`handle_read`], and it
/// must stay the twin: a read arm and a write arm that reached different devices would be the
/// PI-FS-2 class again, one layer down.
fn handle_write(handle: block::BlockHandle, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    match handle {
        block::BlockHandle::Global => block::write_block(lba, buf),
        block::BlockHandle::Usb => block::write_block_usb(lba, buf),
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        block::BlockHandle::Sdhc => block::write_block_sdhc(lba, buf),
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        block::BlockHandle::SdMmc => block::write_block_tegra_sd(lba, buf), #[cfg(all(target_arch = "x86_64", feature = "ahci"))] block::BlockHandle::Ahci { port } => block::write_block_ahci_port(port, lba, buf), // UNAFSX86: refuses unless ahci-write's grant path — the twin of the read arm
    }
}

impl SdSectorDevice {
    /// Open the GLOBAL block handle — the historical behaviour of this constructor, and what every
    /// in-tree caller means: on the Pi the global slot is the microSD once `register_sd` has run,
    /// and on x86 it is the boot stick. Kept as a named wrapper so the pi paths read exactly as
    /// before and the handle they mean is written down rather than inferred.
    pub fn open() -> Result<Self, MountError> {
        Self::open_on(block::BlockHandle::Global)
    }

    /// SDSEAM: open a specific block handle, if its geometry fits the seam.
    ///
    /// PI-FS-2: on the bare-metal Pi the native unafs store is the microSD (emmc2), and
    /// [`block::read_block`] routes SD reads to the emmc2 backend for as long as the SD backend is
    /// active — a state the USB storage bring-up never clears. But that bring-up DOES overwrite the
    /// shared `block::BLOCK_DEVICE` global with the USB stick's geometry, so sizing this device from
    /// the global would guard SD reads against the stick's block count. A 14 MiB "USB SD Reader"
    /// (num_blocks 29120) enumerated behind the shell then made `locate` reject the SD's own FAT
    /// partition (LBA 63, extent 109439 > 29120) as `Part(OutOfBounds(63))`, even though the reads
    /// themselves would have come off the SD. Bind the sector count to the SD card itself whenever the
    /// SD supplies the bytes, so the size guard and the data path name the same device.
    ///
    /// That override belongs to the GLOBAL handle specifically — it exists because the global slot's
    /// row can be clobbered while the global READ path still reaches the card. A handle with its own
    /// dedicated registry slot (`Usb`, `Sdhc`, and the tegra card when it arrives) cannot be
    /// clobbered by another device's enumeration, so its own row is already the right answer and it
    /// takes no override.
    pub fn open_on(handle: block::BlockHandle) -> Result<Self, MountError> {
        let dev = handle_info(handle).ok_or(MountError::NoStorage)?;
        if dev.block_size != 512 {
            return Err(MountError::BadSectorSize(dev.block_size));
        }
        let sectors = match handle {
            // Prefer the SD card's own block count when the SD backend is live (emmc2 answers the
            // reads); the global `dev.num_blocks` may have been clobbered by a USB mass-storage
            // enumeration. Byte-for-byte the pre-SDSEAM computation, now scoped to the one handle
            // whose read path it describes.
            block::BlockHandle::Global => {
                #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
                {
                    crate::drivers::emmc2::card_num_blocks().unwrap_or(dev.num_blocks)
                }
                #[cfg(not(all(target_arch = "aarch64", feature = "baremetal")))]
                {
                    dev.num_blocks
                }
            }
            // A dedicated slot is sized from itself: `handle_info` read the same row that
            // `handle_read`/`handle_write` will address, so guard and data path agree by
            // construction with no override to get right.
            block::BlockHandle::Usb => dev.num_blocks,
            #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
            block::BlockHandle::Sdhc => dev.num_blocks,
            #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
            block::BlockHandle::SdMmc => dev.num_blocks, #[cfg(all(target_arch = "x86_64", feature = "ahci"))] block::BlockHandle::Ahci { .. } => dev.num_blocks, // UNAFSX86: a dedicated per-port row
        };
        Ok(Self { handle, sectors, ra: RaSet::new(ra_window(handle)) })
    }

    /// The handle this device reads, writes and was sized from. Exposed so a caller that built a
    /// device can label its witnesses with the disk it actually names, rather than re-deciding.
    pub fn handle(&self) -> block::BlockHandle {
        self.handle
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
        if self.ra.window_sectors() > 1 {
            return self.read_sectors(lba, buf); // BOOT80: through the read-ahead window
        }
        match handle_read(self.handle, lba, buf) {
            Ok(512) => Ok(()),
            Ok(n) => Err(SectorError::Io(format!("short sector read: {n} bytes"))),
            Err(BlockError::BadLba) => Err(SectorError::OutOfBounds(lba)),
            Err(e) => {
                // UNAFSTXN: a `Busy` is TRANSIENT contention, not a medium fault — latch it for the
                // enclosing [`with_unafs`] hold before it collapses into the crate's opaque
                // `SectorError::Io`. See [`note_sector_busy`].
                note_sector_busy(&e);
                Err(SectorError::Io(format!("block layer: {e:?}")))
            }
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
        let r = handle_write(self.handle, lba, buf);
        match &r { Ok(()) => self.ra.write_through(lba, buf), Err(_) => self.ra.invalidate() } // BOOT80: the window stays true to the medium
        match r {
            Ok(()) => Ok(()),
            Err(BlockError::BadLba) => Err(SectorError::OutOfBounds(lba)),
            Err(e) => {
                // UNAFSTXN: as on the read twin — latch a transient `Busy` for the enclosing
                // [`with_unafs`] hold. A `Busy` here means the CMD24 ladder never STARTED (the
                // claim was refused before any command issued), so this sector is untouched.
                note_sector_busy(&e);
                Err(SectorError::Io(format!("block layer: {e:?}")))
            }
        }
    }

    fn sector_count(&self) -> u64 {
        self.sectors
    }

    /// BOOT80 (B350): a 4 KiB block in one call. On the SD card: served from the read-ahead window, or
    /// ONE multi-block read (CMD18) of the window that holds it; elsewhere the per-sector loop.
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), SectorError> {
        if self.ra.window_sectors() <= 1 { TX_FILLS.fetch_add(1, Ordering::Relaxed);
            for (i, c) in buf.chunks_mut(512).enumerate() {
                let at = lba.checked_add(i as u64).ok_or(SectorError::OutOfBounds(lba))?;
                match handle_read(self.handle, at, c) {
                    Ok(512) => {}
                    Ok(n) => return Err(SectorError::Io(format!("short sector read: {n} bytes"))),
                    Err(BlockError::BadLba) => return Err(SectorError::OutOfBounds(at)),
                    Err(e) => {
                        note_sector_busy(&e);
                        return Err(SectorError::Io(format!("block layer: {e:?}")));
                    }
                }
            }
            return Ok(());
        }
        let handle = self.handle;
        let sectors = self.sectors;
        self.ra.read(lba, buf, sectors, |start, b| handle_read_multi(handle, start, b))
    }

    /// BOOT80 (B350): the write twin — ONE multi-block write (CMD25) on the SD card, synchronous to the
    /// medium like the single-sector path (the K8a ordering note on `flush` below holds unchanged);
    /// the read-ahead window is refreshed with what landed, and dropped if the write failed part-way.
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), SectorError> {
        let r = handle_write_multi(self.handle, lba, buf);
        match &r {
            Ok(()) => self.ra.write_through(lba, buf),
            Err(_) => self.ra.invalidate(),
        }
        r
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
        #[cfg(all(target_arch = "x86_64", feature = "ahciroot"))] if let block::BlockHandle::Ahci { port } = self.handle { return block::flush_ahci_port(port).map_err(|_| SectorError::Io(alloc::string::String::from("ahci: FLUSH CACHE EXT failed"))); } // AHCIROOT (B332): a SATA SSD HAS a write cache — this is the real drain the note above demands
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
    /// UNAFSTXN: the storage device stayed loaned out to another context for the whole of
    /// [`with_unafs`]'s bounded restart budget, so the transaction could not be run to completion.
    ///
    /// TRANSIENT, and it says so honestly: nothing was mutated (the restart precondition is that no
    /// root ever flipped — see [`with_unafs_attempt`]'s durable-progress fence), the committed tree
    /// is exactly what it was, and the caller may simply try again. This is the unafs twin of
    /// `FatError::Busy`/`BlockError::Busy`, and the reason it is a variant rather than a `Fs(..)`
    /// or an `Io`: the WEDGE family's standing rule is that a refusal from CONTENTION must never be
    /// indistinguishable on the wire from a refusal by dead hardware.
    Busy,
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

/// Locate the UnaFS partition on the GLOBAL block handle — the volume this kernel mounts.
pub fn locate() -> Result<PartitionSpan, MountError> {
    locate_on(block::BlockHandle::Global)
}

/// SDSEAM: locate a UnaFS partition on a NAMED block handle.
///
/// The scan runs through the device's own routed reads, so the partition table read, the superblock
/// probe and the bound the span is checked against all come off the same disk. That is the property
/// [`locate`] used to get only by luck of the ambient backend agreeing with the ambient registry.
pub fn locate_on(handle: block::BlockHandle) -> Result<PartitionSpan, MountError> {
    let mut dev = SdSectorDevice::open_on(handle)?;
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
    mount_on(block::BlockHandle::Global)
}

/// SDSEAM: the handle-named form of [`mount`]. Locate and mount on ONE disk end to end — the
/// partition scan, the span witness and the live adapter every subsequent read/write flows through
/// are all built on the same handle, so a mount can no longer be assembled from two disks.
pub fn mount_on(handle: block::BlockHandle) -> Result<KernelUnaFS, MountError> {
    install_warn_hook();
    crate::fs::unafstime::install_clock_hook(); // UNAFSTIME (B308): the kernel clock stamps inodes
    let span = locate_on(handle)?;
    partition_witness(handle, &span); #[cfg(all(target_arch = "x86_64", feature = "wc"))] splitread::note_span(handle, &span); // HIDSTALL2 (B670): the span this handle's volume sits at, for the split read (LINE-NEUTRAL append, before this comment)
    let dev = SdSectorDevice::open_on(handle)?;
    let adapter = BlockAdapter::for_partition(dev, &span);
    keyboot_mount(handle, adapter).map(versions_armed).map_err(|e| { if let FileSystemError::Incompatible(bits) = &e { serial_println!("[unafs] mount refused incompat={:#x}", bits); } MountError::Fs(e) }) // ROOTINCOMPAT (SR159): an unknown INCOMPAT root bit is refused and said (LINE-NEUTRAL fold, before this comment). KEYBOOT (B688): the unlocked keyed root re-opens with the key its open mount carries (body at the tail); every other volume is `UnaFS::mount` as before
}

/// PARTITION (GR9): cross-check the located UnaFS span against the kernel block layer's own MBR
/// decode, once per boot.
///
/// There are TWO independent partition-table readers in this tree: the unafs crate's
/// `parse_partitions` (which `locate_unafs` uses to find this volume by superblock magic) and the
/// kernel block layer's [`block::decode_mbr`] (which the FAT mount uses to bind the ESP). They were
/// written at different times against the same spec, so "they agree on this medium" is a real,
/// falsifiable claim about the disk in the machine — and the one that matters, because if they
/// disagreed the ESP and the native volume could be bound to overlapping extents. This is the check
/// that makes the layout of record (p1 ESP, p2 UnaFS) an observed fact rather than an assumption.
///
/// Strictly read-only and strictly advisory: every outcome is a printed line, never an error. It
/// cannot change what gets mounted, so a wrong witness can mislead a reader but can never corrupt a
/// volume. The magic re-read goes through a [`block::PartitionRange`], i.e. through the bounded,
/// partition-RELATIVE addressing path — so a PASS also proves that path maps sector 0 of the
/// partition to the same bytes the crate's adapter reached by its own arithmetic.
///
/// INSTRUMENT NOTE (healthy-but-idle): the latch reads false until the first UnaFS mount of the boot
/// and true forever after; it gates printing only. On the layout of record the line reads
/// `slot=2 ... magic=ok fits=yes`, from the very first mount, and nothing about an idle system can
/// change any field on it — every value is a property of the medium, read at a moment when the
/// volume has just been located and is therefore defined.
fn partition_witness(handle: block::BlockHandle, span: &PartitionSpan) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // SDSEAM: the witness reads the disk the mount named, not the ambient one. On the Global handle
    // this is the identical call it made before (`block::info` / `block::read_block`), so the pi
    // line is unchanged; on any other handle it now describes the volume that was actually mounted
    // instead of silently describing the global slot's disk.
    let Some(dev) = handle_info(handle) else {
        serial_println!(":: PART: unafs span check — no block device ::");
        return;
    };
    let mut sec = [0u8; 512];
    if handle_read(handle, 0, &mut sec).is_err() {
        serial_println!(":: PART: unafs span check — LBA 0 unreadable ::");
        return;
    }
    // The census may already have printed for this handle (the FAT mount runs first on most boots);
    // calling it again is harmless and guarantees the raw bytes are in the log even on a boot where
    // no FAT volume was ever mounted.
    let Some(table) = block::mbr_census(handle, &sec, dev.num_blocks) else {
        serial_println!(
            ":: PART: unafs span check — no MBR at LBA 0; unafs span base={} blocks={} ::",
            span.base_lba, span.block_count
        );
        return;
    };

    // Which accepted primary contains the located span's base sector?
    let Some(p) = table.iter().find(|p| p.start_lba == span.base_lba) else {
        serial_println!(
            ":: PART: unafs span check — base LBA {} is NOT the start of any accepted MBR partition (blocks={}) ::",
            span.base_lba, span.block_count
        );
        return;
    };

    // PARTITION-FITS: re-read block 0 through the BOUNDED, partition-relative path — the magic AND
    // the volume's OWN declared block count, out of the same 512 bytes. The verdict, the witness
    // line and the overrun scream live in [`span_fit_report`] at EOF, because `panic::Location`
    // embeds source line numbers: growing a body HERE would rewrite every location string in the
    // ~2100 lines below it in this arch-neutral module, moving bytes in images (x86 included) that
    // see no behavioural change at all. This block is line-for-line the size of the one it replaced.
    //
    // WHY THE OLD `fits=` COULD NOT READ `NO`. It tested `span.block_count * 8 <= p.sector_count`.
    // `span.block_count` is set by the unafs crate's `locate_unafs` to `p.sector_count /
    // SECTORS_PER_BLOCK` off the SAME MBR entry (`libs/fs/unafs/src/adapter.rs`), so the test
    // reduced to `(n / 8) * 8 <= n` — true for every `n` under floor division. It could fire only
    // if the crate's `parse_mbr` and the kernel's `decode_mbr` returned different sector counts for
    // one slot, and both decode the same little-endian u32 at entry offset 12 of the same LBA 0. A
    // field named for a containment property was reporting a tautology, and it has been read as
    // evidence the volume was sound. [`span_fit_report`] compares the SUPERBLOCK's declared
    // `block_count` instead — written at format time, read back off the medium, an INDEPENDENT
    // source — which is the property `k3_mount_selftest` bit3 asserts and the only one this field's
    // name ever promised.
    let range = block::PartitionRange::new(handle, &p);
    let mut b0 = [0u8; 512];
    let read_ok = range.read_block(0, &mut b0).is_ok();
    let magic_ok =
        read_ok && b0[..::unafs::superblock::MAGIC.len()] == ::unafs::superblock::MAGIC;
    let sb0 = if magic_ok { Some(&b0) } else { None };
    span_fit_report(&p, span, sb0, magic_ok);
}

/// UNAFSBIND: the mount cache entry — the live mount PLUS the handle it was mounted from.
///
/// The handle is stored, not re-derived, for the same reason [`SdSectorDevice`] carries one: a cache
/// that answers "which disk is this mount" by assumption is the PI-FS-2 class one layer up. Every
/// sector the mount moves already routes through the adapter's carried handle (SDSEAM); this field
/// makes the BINDING itself inspectable — the `[unafsbind]` witness prints it, and
/// [`mount_bound_handle`] exposes it so any caller can label its own witnesses with the disk the
/// shared mount actually rides instead of re-deciding.
struct BoundMount {
    /// The [`block::BlockHandle`] every sector of `fs` reads and writes — the adapter inside `fs`
    /// was built by [`mount_on`] on exactly this value.
    handle: block::BlockHandle,
    fs: KernelUnaFS,
}

/// The single, process-wide UnaFS mount — the K4 coherence keystone.
///
/// Exactly ONE live mount for the volume, so there is one authoritative in-RAM
/// allocation bitmap and one journal cursor. Populated lazily on first use and
/// then kept (never dropped in steady state; `Drop`'s metadata write-back would
/// otherwise fire on a mere read). [`force_remount`] is the only thing that
/// clears it.
///
/// UNAFSBIND: the entry is a [`BoundMount`], and the lazy bind is [`bind_mount`] —
/// handle-DISCOVERING, not `Global`-assuming.
static MOUNT: Mutex<Option<BoundMount>> = Mutex::new(None);

/// UNAFSBIND: may the lazy mount bind PROBE this handle for a unafs volume when the GLOBAL path has
/// none? `Global` answers `false` because it is not a probe — it is the bind's first, default
/// attempt, and it wins outright whenever it holds a volume (the pi/x86 behaviour of record).
///
/// EXHAUSTIVE on purpose — no wildcard arm, the SDSEAM discipline applied to the BIND seam: a handle
/// added in the block layer is an E0004 here, forcing the semantic decision "should a shell whose
/// global path has no unafs volume discover one riding this handle?" to be made in the same commit
/// that adds the handle, instead of the handle silently staying invisible to [`with_unafs`] —
/// which is exactly the defect this arc removes.
///
/// ### MERGE NOTE — the SdMmc arm this seam is waiting for
/// When `BlockHandle::SdMmc` lands (orin's TEGRASD, `drivers/block.rs`), the totality of this
/// match reports it as an E0004 alongside [`handle_info`]'s trio. Its arm here is one line under the
/// TEGRASD cfg triple, and it is the whole point of this arc:
///
/// * [`bind_probe_admitted`] — `BlockHandle::SdMmc => true,` (the orin's unafs volume rides the
///   card's dedicated handle while `Global` is the USB stick; admitting the probe is what lets the
///   shell's `with_unafs` find the card's volume with no tegra-side shell wiring at all)
/// * [`bind_probe_candidates`] — add `block::BlockHandle::SdMmc` to the array (and grow its
///   length), in enum order; the array mirrors the enum and [`handle_kind_name`] gains
///   `BlockHandle::SdMmc => "tegra-sd",`.
fn bind_probe_admitted(handle: block::BlockHandle) -> bool {
    match handle {
        // Not a probe: the default first attempt of every bind (see above).
        block::BlockHandle::Global => false,
        // A unafs volume on a dedicated-handle USB device whose machine boots with a volume-less
        // global path is exactly the "volume arrives via a different handle" class; discover it.
        block::BlockHandle::Usb => true,
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        block::BlockHandle::Sdhc => true,
        // TEGRASD merge arm — prescribed by the MERGE NOTE above: the orin's unafs volume rides
        // the card's dedicated handle while `Global` is the USB stick; admit the probe.
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        block::BlockHandle::SdMmc => true, #[cfg(all(target_arch = "x86_64", feature = "ahci"))] block::BlockHandle::Ahci { .. } => false, // UNAFSX86: NOT probed — the shared mount is read-write and this transport refuses ordinary writes; a SATA volume is reached by `mount_on` read-only, never by the lazy bind
    }
}

/// UNAFSBIND: every handle the block layer defines, in enum order — the bind's probe walk. The array
/// cannot itself be forced complete by the compiler; [`bind_probe_admitted`]'s exhaustive match is
/// the E0004 tripwire that drags a reader here (its MERGE NOTE names this array), and
/// [`handle_kind_name`] is a second, independent one.
fn bind_probe_candidates() -> impl Iterator<Item = block::BlockHandle> {
    [
        block::BlockHandle::Global,
        block::BlockHandle::Usb,
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        block::BlockHandle::Sdhc,
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        block::BlockHandle::SdMmc,
    ]
    .into_iter()
}

/// UNAFSBIND: the wire name of a handle kind, for the `[unafsbind]` witness. Exhaustive (E0004 on a
/// new handle); values match the block layer's `mbr-raw handle=` names so one `awk` finds a handle's
/// whole story across both layers.
fn handle_kind_name(handle: block::BlockHandle) -> &'static str {
    match handle {
        block::BlockHandle::Global => "global",
        block::BlockHandle::Usb => "usb",
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        block::BlockHandle::Sdhc => "sdhc",
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        block::BlockHandle::SdMmc => "tegra-sd", #[cfg(all(target_arch = "x86_64", feature = "ahci"))] block::BlockHandle::Ahci { .. } => "ahci", // UNAFSX86
    }
}

/// UNAFSBIND: the handle the live cached mount is riding, or `None` if no mount is currently bound.
/// A read-only peek (masked, like every `MOUNT` touch); it never triggers the lazy bind.
pub fn mount_bound_handle() -> Option<block::BlockHandle> { #[cfg(all(target_arch = "x86_64", feature = "wc"))] { if !crate::video::renderwedge::wait_unlocked(&MOUNT, crate::video::renderwedge::W_UNAFS) { return None; } } // TRASHLOCK (B523): RENDERWEDGE's bounded pre-wait (B520 :793's shape) — the render task never spins unbounded on MOUNT here; past MOUNT_BOUND_MS `[render] wait-timeout what=unafs-mount` and "none bound". ⚠ SAME-LINE fold: :639's `MOUNT.lock()` keeps its line and column
    crate::arch::without_interrupts(|| MOUNT.lock().as_ref().map(|b| b.handle))
}

/// UNAFSBIND: the lazy bind — locate, mount and NAME the volume [`with_unafs`] will serve.
///
/// Order of proof, not of preference:
///
/// 1. **`Global` first, and it wins whenever it holds a volume.** On the pi the global handle IS the
///    SD/USB path and the volume of record lives there; this arm is call-for-call the pre-UNAFSBIND
///    `mount()`, so every machine whose volume rides `Global` binds exactly as before and the probes
///    below never run.
/// 2. **Probing happens only on proof of ABSENCE, never on ambiguity.** The probe walk runs only
///    when the global attempt failed with a verdict that says "no unafs volume can be reached
///    through the global path" (`NoStorage`, `BadSectorSize`, `Part`, `NoVolume`) AND no transient
///    `Busy` was latched during the attempt. A `Busy`-tainted or `Fs(_)`-refused global attempt
///    returns its error unprobed: the global volume may exist (mid-loan, or mount-refused), and
///    binding some OTHER handle's volume behind it would shadow the volume of record with a
///    plausible impostor — a mis-bind, which is worse than the honest error.
/// 3. **Probes walk the handles in enum order**, taking only those [`bind_probe_admitted`] admits,
///    and the first handle that mounts is the binding. A probe's own failure is discarded (a probe
///    that latched `Busy` also stops the walk — same taint rule); if nothing binds, the GLOBAL
///    error is returned, so a machine with no volume anywhere reports today's exact verdict.
///
/// The successful bind prints the `[unafsbind]` witness: `mount=native` (the native volume's VFS
/// name of record — vfs.rs mounts `NativeBackend::new("native")`), `handle=` the disk it rode.
fn bind_mount() -> Result<BoundMount, MountError> { #[cfg(all(target_arch = "x86_64", feature = "wc"))] debug_assert!(crate::arch::sched::current_id().map_or(true, |t| !crate::lockowner::held_by(crate::lockowner::UNAFS, t)), "MOUNTSTACK (B846): bind_mount under the MOUNT hold"); // POISON2 (B846) MOUNTSTACK: with the ticket the bind runs BEFORE the masked hold (`prebind`, file tail); a bind under it is a bug. LINE-NEUTRAL append
    #[cfg(all(target_arch = "x86_64", feature = "ahciroot"))] if let Some(port) = crate::install::ahciroot::root_port() { if let Ok(fs) = mount_on(block::BlockHandle::Ahci { port }) { return Ok(BoundMount { handle: block::BlockHandle::Ahci { port }, fs }); } } // AHCIROOT (B332) M4: the root-granted SSD binds FIRST — SSD root over card root; silent (the UNAFSX86 line names it)
    let global_err = match mount_on(block::BlockHandle::Global) {
        Ok(fs) => {
            serial_println!(
                ":: [unafsbind] mount=native handle={} ::",
                handle_kind_name(block::BlockHandle::Global)
            );
            return Ok(BoundMount { handle: block::BlockHandle::Global, fs });
        }
        Err(e) => e,
    };
    // Busy taint: the global attempt was refused CONTENTION, not absence — the enclosing
    // `with_unafs_attempt` reads the latch and restarts; do not probe past a maybe-present volume.
    if TXN_BUSY.load(Ordering::Relaxed) {
        return Err(global_err);
    }
    // Only proven absence admits the probe walk (see the doc's rule 2). `Fs(_)` means a unafs
    // volume WAS located on Global and refused the mount; `Busy` is handled above.
    if !matches!(
        global_err,
        MountError::NoStorage
            | MountError::BadSectorSize(_)
            | MountError::Part(_)
            | MountError::NoVolume
    ) {
        return Err(global_err);
    }
    for handle in bind_probe_candidates() {
        if !bind_probe_admitted(handle) {
            continue;
        }
        match mount_on(handle) {
            Ok(fs) => {
                serial_println!(
                    ":: [unafsbind] mount=native handle={} ::",
                    handle_kind_name(handle)
                );
                return Ok(BoundMount { handle, fs });
            }
            // A probe that hit contention stops the walk untainted-result-first: the latch is set,
            // the enclosing attempt will restart, and the restart re-runs the whole discovery.
            Err(_) if TXN_BUSY.load(Ordering::Relaxed) => return Err(global_err),
            Err(_) => {}
        }
    }
    // Nothing anywhere: the global path's verdict is the machine's verdict, exactly as before.
    Err(global_err)
}

/// UNAFSTXN: a transient `BlockError::Busy` was refused to THIS `with_unafs` attempt by the block
/// layer — the storage device is loaned out to another context (WEDGE-8's xHCI controller mid-BOT,
/// WEDGE-10's microSD card mid-CMD17/CMD24 ladder) and the claim gave up rather than wait longer.
///
/// Set by [`note_sector_busy`] at the [`SdSectorDevice`] seam, and read/cleared by
/// [`with_unafs_attempt`] strictly INSIDE the same masked `MOUNT` hold that ran the attempt — the
/// [`MOUNT_DISCARD`] idiom exactly. Every producer (`read_sector`/`write_sector`) runs beneath a
/// caller that holds `MOUNT`, so this flag is serialized by that lock and can never leak from one
/// transaction into another.
///
/// WHY THE FLAG AND NOT THE ERROR VALUE. The `unafs` crate's seam is `SectorError`, which carries
/// no contention variant — a `Busy` therefore arrives at [`with_unafs`] already collapsed into
/// `SectorError::Io("block layer: Busy")` and, one layer further up, into whatever the closure
/// chose to make of a failed op (`FileSystemError::Storage`, a bare `false`, an `is_err()` that
/// silently reads as "absent"). Latching at the seam is what preserves the distinction the whole
/// WEDGE family exists to preserve: a refusal from CONTENTION must never be indistinguishable
/// from a refusal by DEAD HARDWARE.
static TXN_BUSY: AtomicBool = AtomicBool::new(false);

/// UNAFSTXN: latch a transient contention refusal for the enclosing transaction. Anything that is
/// not `Busy` is a real fault and passes through untouched.
#[inline]
fn note_sector_busy(e: &BlockError) {
    if matches!(e, BlockError::Busy) {
        TXN_BUSY.store(true, Ordering::Relaxed);
    }
}

/// UNAFSTXN: how many times one [`with_unafs`] call may run its closure before giving up with
/// [`MountError::Busy`]. The twin of `fs::fat`'s `RMW_BUSY_ATTEMPTS`, and paired the same way with a
/// wall-clock cap (`hw_wait_budget()`); both bounds are needed because the two backends refuse on
/// completely different time scales, and each bound is the binding one on exactly one of them:
///
/// * **The wall-clock cap binds the microSD backend.** Since WEDGE-10 a MASKED claimant on emmc2 —
///   and `with_unafs` is *always* masked, it runs the whole transaction inside `without_interrupts`
///   — does not get an instant refusal: it spins re-claiming for `MASKED_CLAIM_BUDGET_MS`, 2× the
///   driver's worst legitimate hold (~2.6 s), before the `Busy` is surfaced at all. So every attempt
///   that ends in `Busy` here has ALREADY absorbed a full bounded wait, and multiplying that by an
///   attempt count is the thing to prevent — the deadline stops us after roughly one such wait
///   rather than eight.
/// * **The attempt cap binds every instant-refusal backend.** WEDGE-8's xHCI keeps the original
///   policy (a masked claimant is told `Busy` immediately, `drivers/block.rs::claim_xhci_for_io`),
///   so on a USB-backed volume the attempts cost nothing but the inter-attempt yield and the
///   deadline would never fire. The count is what terminates the loop there.
///
/// EIGHT, not `fat.rs`'s sixty-four, and the difference is deliberate: a restarted FAT RMW re-runs
/// ONE sector read-modify-write, whereas a restarted unafs transaction re-runs a whole CoW
/// transaction — fresh data extents, a re-serialized inode map and refcount map, and a root flip —
/// off a mount that the abort has just discarded and must therefore re-read from the medium. The
/// retry is a NET for a handover race (the loan changing hands between the holder's release and our
/// claim), not a mechanism for out-waiting a wedged card; the block layer's own bounded wait is the
/// mechanism, exactly as WEDGE-10 states it. Eight is enough to survive several consecutive lost
/// handovers and small enough that the pathological case is bounded by the deadline instead.
const TXN_BUSY_ATTEMPTS: u32 = 8;

/// One attempt of [`with_unafs`]: the original masked `MOUNT` hold, plus the verdict on whether the
/// transaction may be restarted. See [`Attempt`].
enum Attempt<R> {
    /// The transaction reached an outcome that must be reported as-is.
    Settled(Result<R, MountError>),
    /// The transaction hit a transient `Busy` and left NOTHING durable behind; the cached mount has
    /// already been discarded inside the hold, so a fresh attempt starts from the committed root.
    Restart,
}

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
///
/// UNAFSTXN: `f` is `FnMut` rather than `FnOnce` because a transaction that dies on a transient
/// `Busy` is RESTARTED — see [`with_unafs`]'s restart loop and [`TXN_BUSY_ATTEMPTS`].
fn with_unafs_attempt<R>(f: &mut impl FnMut(&mut KernelUnaFS) -> R) -> Attempt<R> {
    crate::arch::without_interrupts(|| { #[cfg(all(target_arch = "x86_64", feature = "wc"))] { if !crate::video::renderwedge::wait_unlocked(&MOUNT, crate::video::renderwedge::W_UNAFS) { return Attempt::Settled(Err(MountError::Busy)); } } // RENDERWEDGE (B520): the RENDER task's acquire of the mount is bounded (MOUNT_BOUND_MS) — f28's wedge was this masked spin with no bound; past it `[render] wait-timeout what=unafs-mount` and a clean Busy (nothing was touched). Every other caller is unchanged. ⚠ SAME-LINE fold: line :794's `MOUNT.lock()` keeps its line and column (its `#[track_caller]` site)
        let mut guard = MOUNT.lock(); let _own = crate::lockowner::own(crate::lockowner::UNAFS); #[cfg(all(target_arch = "x86_64", feature = "wc"))] if splitwrite::ticket_foreign() { return Attempt::Restart; } #[cfg(target_arch = "x86_64")] mount_poison_check(&mut *guard); // POISON2 (B846) MOUNTPOISON: a dead holder's mark discards the in-RAM instance before anything reads it (file tail; LINE-NEUTRAL, before this comment). UNAFSWRITE (B825): another task's commit is writing with MOUNT released — leave MOUNT untouched and restart (LINE-NEUTRAL, before this comment). STACKGUARD2 (B403): the holder, released if it overflows mid-transaction
        // UNAFSTXN: open the attempt's contention window, INSIDE the hold. Clearing it before the
        // acquire would be an SMP bug: a core waiting on `MOUNT` would wipe the latch belonging to
        // the transaction currently running under it. Under the lock the flag is serialized with
        // every producer that matters, because every `read_sector`/`write_sector` of a transaction
        // runs beneath this guard. (The two lock-free probes that also read sectors —
        // `locate().is_err()` in `vfs`/`syscall` — can still set it from outside; the cost of that
        // race is at worst one wasted restart, never a wrong answer, because an abort is clean.)
        TXN_BUSY.store(false, Ordering::Relaxed);
        if guard.is_none() {
            // UNAFSBIND: the lazy bind DISCOVERS its handle (Global first and unchanged; probes only
            // on proven global absence) instead of assuming `Global` — see [`bind_mount`].
            match bind_in_hold() {
                Ok(m) => { #[cfg(all(target_arch = "x86_64", feature = "wc"))] splitread::note_bound(m.handle); *guard = Some(m) } // HIDSTALL2 (B670): a new bound mount = a new epoch; a split read planned on the old one re-runs under the hold (LINE-NEUTRAL append, before this comment)
                Err(e) => {
                    // UNAFSTXN: the MOUNT itself reads sectors (`locate`, the superblock, the
                    // reclaim drain), so contention can defeat it before the closure ever runs.
                    // That failure is the same transient and gets the same restart; the cache is
                    // already empty, so there is nothing to discard.
                    return if TXN_BUSY.swap(false, Ordering::Relaxed) {
                        Attempt::Restart
                    } else {
                        Attempt::Settled(Err(e))
                    };
                }
            }
        }
        let fs = &mut guard.as_mut().expect("mount just populated").fs;
        // UNAFSTXN: the durable-progress fence. `commits` counts ROOT FLIPS — the single atomic
        // point of a CoW transaction — so comparing it across the closure answers exactly one
        // question: did anything this closure did reach the medium irrevocably? Read from the same
        // mount instance on both sides, inside one hold, so the comparison is meaningful.
        let commits_before = fs.commit_stats().commits;
        let r = f(fs);
        let busy = TXN_BUSY.swap(false, Ordering::Relaxed);
        let durable = fs.commit_stats().commits != commits_before;
        // K9-PARITY: mid-staging FAILURE discard (SECURITY.md §K1 K9). A staged ACL persist that
        // fails partway (`native_acl_write_on` -> `request_mount_discard`) leaves UNCOMMITTED in-flight
        // transaction state on this shared, cached mount — the root never flipped (K3 durable-first: the
        // committed tree is untouched), but a LATER persist's `commit()` would otherwise flush that
        // orphaned residue alongside its own row. Drop the cached mount HERE, still inside the same
        // uninterrupted MOUNT hold that ran the failing op, so the discard is atomic w.r.t. every other
        // persister (no SMP window in which another core could observe or commit the dirty mount — a
        // discard AFTER releasing the hold, e.g. a bare `force_remount`, would race). The next
        // `with_unafs` re-mounts fresh from the committed root; under CoW the orphaned blocks are a
        // power-cut-equivalent LEAK (never a dangle), the same residue class a real power cut mid-persist
        // would leave. This is the in-lane closure of the K9 lens-B deferred residual; it touches neither
        // the K5B fusion, the K4 IRQ-mask keystone, nor any committed durable state.
        if MOUNT_DISCARD.swap(false, core::sync::atomic::Ordering::Relaxed) {
            *guard = None;
        }
        if busy && !durable {
            // UNAFSTXN — CLEAN ABORT. Nothing this transaction touched can be on the medium in a
            // half-applied form, and that is a property of the FORMAT, not of this code:
            //   * a `Busy` is produced ONLY by a refused claim (`emmc2::claim_for_io`,
            //     `block::claim_xhci_for_io`), which returns BEFORE any command is issued — so the
            //     sector that took the `Busy` was never written, not partially written;
            //   * under K8a copy-on-write no committed block is ever overwritten in place, and the
            //     transaction's single atomic point is one 512 B root-slot flip that only happens
            //     inside `commit()` — which the fence above has just proven did not run;
            //   * therefore the committed tree on disk is byte-for-byte what it was at entry, and
            //     any fresh blocks the dead transaction wrote are unreachable residue — the same
            //     power-cut-equivalent LEAK class the crate's own `txn_unwind` is built around
            //     ("the root never flipped on any failing path, so the on-disk committed tree is
            //     ground truth BY DEFINITION").
            // Discarding the whole cached mount is the strictly stronger form of that unwind: the
            // next attempt re-derives root, inode map and refcount map from the medium and trusts
            // nothing left in RAM. Done HERE, inside the same uninterrupted hold, for the K9 reason
            // above. `Drop` on the discarded mount only calls `flush()`, a no-op on this device —
            // it cannot write the residue back out.
            *guard = None;
            return Attempt::Restart;
        }
        if busy {
            // UNAFSTXN: contention DID strike, but the closure also flipped a root — some of what it
            // did is durable. Re-running it could double-apply, so the restart is DECLINED and the
            // closure's own result stands. Counted for the census rather than silently ignored.
            note_txn_busy_durable();
        }
        Attempt::Settled(Ok(r))
    })
}

/// Run `f` against the one coherent mount, restarting the transaction on transient contention.
///
/// UNAFSTXN: `with_unafs` joins the WEDGE-8/WEDGE-10 `Busy`-aware callers. Before this, a `Busy`
/// raised inside a unafs transaction reached the closure as an opaque `SectorError::Io` and was
/// reported upward as a hard failure — a permanent verdict on a temporary condition, and on the
/// read paths that phrase "the op failed" as `is_err()` it could read as "the object is absent".
/// Now a `Busy` that left nothing durable behind ABORTS the transaction cleanly (the cached mount is
/// discarded inside the hold, so the next attempt re-derives everything from the committed root) and
/// the whole closure is RUN AGAIN — bounded by [`TXN_BUSY_ATTEMPTS`] and by `hw_wait_budget()` of
/// wall clock, whichever comes first. Exhaustion returns [`MountError::Busy`], which is the honest
/// answer: nothing was mutated and the caller may retry.
///
/// The inter-attempt yield is the same one `fs::fat`'s RMW retry uses, with the one guard that
/// matters here: `with_unafs` may itself be called from an already-masked context, and a `hlt` with
/// interrupts masked never wakes. So we `hlt` (schedulable — it lets the loan holder run) only when
/// the caller left us unmasked, and spin-hint otherwise. Either way the wait is OUTSIDE the
/// attempt's own `without_interrupts` span: the F1–F4 rule that no masked span may wait on a driver
/// lock is not weakened by this loop, it is what the loop is built out of.
pub fn with_unafs<R>(mut f: impl FnMut(&mut KernelUnaFS) -> R) -> Result<R, MountError> {
    let start = crate::arch::now_cycles();
    let budget = crate::arch::hw_wait_budget();
    let mut restarts: u32 = 0;
    for _ in 0..TXN_BUSY_ATTEMPTS {
        #[cfg(all(target_arch = "x86_64", feature = "wc"))] if !splitwrite::ticket_wait() { break; } if let Err(e) = prebind() { note_txn_restarts(restarts, true); return Err(e); } match { let t_mask = crate::hidstall::now_us(); let a = with_unafs_attempt(&mut f); crate::hidstall::note_masked_since(t_mask); a } { // HIDSTALL (B485): the attempt's IRQ-masked span, on the stall line as `masked_ms=`
            Attempt::Settled(out) => {
                if restarts > 0 {
                    note_txn_restarts(restarts, true);
                }
                return out;
            }
            Attempt::Restart => {}
        }
        restarts += 1;
        if crate::arch::now_cycles().wrapping_sub(start) >= budget {
            break;
        }
        if crate::arch::irqs_masked() {
            core::hint::spin_loop();
        } else {
            crate::hlt(); // unmasked here — the attempt's mask ended with it; let the holder run
        }
    }
    note_txn_restarts(restarts, false);
    Err(MountError::Busy)
}

// -----------------------------------------------------------------------------------------------
// UNAFSTXN — the restart census.
//
// Behind `feature = "witness"` (UNAOS_WITNESS), the family's DEFAULT-QUIET gate: a boot/media build
// compiles the whole census away and the restart loop above is byte-identical without it. QUIET AT
// ZERO by construction — the line is emitted per `with_unafs` call that actually restarted, so a
// healthy boot (the expected reading, and the one WEDGE-10's gate produced: "the requeue arm fired
// zero times — the bounded wait is the mechanism, the requeue the net") prints nothing at all. When
// it does print, it prints the whole census: this call's restarts and verdict, plus the boot-
// cumulative totals, so one line is enough to tell a single unlucky handover from a contended run.
// COST WHEN ON: three relaxed atomics on a path that has already spent seconds inside the block
// layer's bounded wait. COST WHEN OFF: none.
// -----------------------------------------------------------------------------------------------

/// UNAFSTXN: `with_unafs` calls that restarted at least once this boot.
#[cfg(feature = "witness")]
static TXN_CALLS_RESTARTED: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
/// UNAFSTXN: total transaction restarts this boot (a call may contribute several).
#[cfg(feature = "witness")]
static TXN_RESTART_TOTAL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// UNAFSTXN: transactions that took a `Busy` but had ALREADY flipped a root, so the restart was
/// declined to avoid double-applying durable work. Non-zero here is the honest admission that a
/// closure's result was built partly on a refused sector — the number worth watching, because it is
/// the one case this arc cannot repair from inside `with_unafs`.
#[cfg(feature = "witness")]
static TXN_DECLINED_DURABLE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// UNAFSTXN: census one `with_unafs` call that restarted. `recovered` distinguishes a transaction
/// that went on to settle from one that exhausted the bound and returned [`MountError::Busy`].
#[cfg(feature = "witness")]
fn note_txn_restarts(restarts: u32, recovered: bool) {
    if restarts == 0 {
        return;
    }
    let calls = TXN_CALLS_RESTARTED.fetch_add(1, Ordering::Relaxed) + 1;
    let total = TXN_RESTART_TOTAL.fetch_add(restarts, Ordering::Relaxed) + restarts;
    serial_println!(
        ":: UNAFSTXN: unafs txn restarted on Busy — restarts={} verdict={} boot_txns_restarted={} boot_restarts={} declined_durable={} ::",
        restarts,
        if recovered { "recovered" } else { "EXHAUSTED" },
        calls,
        total,
        TXN_DECLINED_DURABLE.load(Ordering::Relaxed),
    );
}

/// UNAFSTXN: default-quiet build — the census is compiled out.
#[cfg(not(feature = "witness"))]
#[inline(always)]
fn note_txn_restarts(_restarts: u32, _recovered: bool) {}

/// UNAFSTXN: census a `Busy` whose transaction had already committed, so the restart was declined.
#[cfg(feature = "witness")]
#[inline]
fn note_txn_busy_durable() {
    TXN_DECLINED_DURABLE.fetch_add(1, Ordering::Relaxed);
}

/// UNAFSTXN: default-quiet build — the census is compiled out.
#[cfg(not(feature = "witness"))]
#[inline(always)]
fn note_txn_busy_durable() {}

/// K9-PARITY: a staged ACL persist under an already-held [`with_unafs`] mount sets this to ask the
/// enclosing hold to DISCARD the cached mount (drop the dirty in-flight transaction, re-mount fresh from
/// the committed root on the next access). Set only on the FAILURE path of [`native_acl_write_on`]; the
/// enclosing [`with_unafs`] consumes it before releasing the lock, so it is set and cleared strictly
/// within one serialized hold and can never leak into a later or read-only hold.
static MOUNT_DISCARD: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// K9-PARITY: request the enclosing [`with_unafs`] hold discard the cached mount (see [`MOUNT_DISCARD`]).
pub(crate) fn request_mount_discard() {
    MOUNT_DISCARD.store(true, core::sync::atomic::Ordering::Relaxed);
}

/// K9-PARITY (test only): when set, [`native_acl_stage_row`] aborts PARTWAY through a row — after the
/// inode + name/fc/owner attributes have staged but before the grants — to exercise the mid-staging-
/// failure discard path. Default off, set transiently by the `k9_parity_check` witness only; NO
/// production caller touches it (an always-false atomic load in the staging body, the K3_TEST_FAIL_PERSIST
/// idiom).
#[doc(hidden)]
pub static TEST_FAIL_MIDSTAGE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Drop the cached live mount so the next [`with_unafs`] re-reads the volume
/// from disk with a fresh in-RAM bitmap/journal — a genuine remount, not a
/// RAM-cache re-read. The dropped instance's `Drop` flushes its
/// already-consistent metadata; the fresh mount then observes only what
/// actually reached the medium. This is the durability-proof primitive the K4
/// witness uses to prove a write survived being committed to the block device.
pub fn force_remount() {
    crate::arch::without_interrupts(|| { #[cfg(all(target_arch = "x86_64", feature = "wc"))] { if !crate::video::renderwedge::wait_unlocked(&MOUNT, crate::video::renderwedge::W_UNAFS) { return ; } } // TRASHLOCK (B523): RENDERWEDGE's bounded pre-wait (B520 :793's shape); past the bound the remount is skipped (nothing was touched). ⚠ SAME-LINE fold: :1017's `MOUNT.lock()` keeps its line and column
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
    with_unafs_write(|fs| native_acl_write_on(fs, dir_lba, dir_off, name, first_cluster, owner, grants))
        .unwrap_or(false)
}

/// K5B: the body of [`native_acl_write`], against an ALREADY-HELD coherent mount. The persist sites
/// (`syscall.rs`) call this inside their single fused `with_unafs` hold — snapshot, THIS journaled write,
/// and the in-RAM commit all under one MOUNT hold (the K5B invariant) — so they must not re-enter
/// `with_unafs` (the MOUNT spinlock is non-reentrant). Same contract as the wrapper: `true` iff every
/// journaled step committed.
///
/// K9-MASKCUT (SECURITY.md §K1 K9): adopt the UNAFS-BATCH staged-transaction shape at this ACL-persist
/// choke point — stage the ENTIRE row (create/resolve + stale-grant removal + name/fc/owner/grant
/// rewrites) with autocommit OFF and land it in ONE root flip, instead of one flip per attribute. This
/// cuts the sector/flip count inside the K5B per-core IRQ-masked `with_unafs` window (the crate's own
/// batched-sync win, applied to the kernel ACL path). No change to WHAT is written, nor to the K5B fusion:
/// the caller's snapshot -> THIS write -> in-RAM commit still all run under the one MOUNT hold; only this
/// write's internal flip count shrinks. All three fused persist sites (`sys_fgrant_revoke_2phase`,
/// `native_persist_grants`, `native_persist_grow`, plus create/rename) funnel through here, so the cut is
/// uniform.
///
/// SCOPE-GUARD (the M1 requirement): the staging body ([`native_acl_stage_row`]) owns every early return;
/// this wrapper has NO early return between `set_autocommit(false)` and the unconditional
/// `set_autocommit(true)`, so a staging failure can never leak the autocommit-OFF state onto the
/// process-wide cached mount (which would silently drop a later writer's commit). Production always enters
/// autocommit-ON (the K8a witness is the only other toggler and never nests a persist inside its hold), so
/// restoring to ON is the invariant, not a guess.
///
/// DURABLE-FIRST (K3, preserved — strengthened on the success path): on staging success we commit ONCE, so
/// a crash lands EITHER the old row OR the whole new row, never a partial row (as the per-op path could).
/// On staging FAILURE we do NOT commit — no root flip, nothing durable changes, the old row stands, the
/// caller sees `false` -> `-EIO`, in-RAM intact. RESIDUAL (pre-existing and equal to the autocommit-ON
/// path, documented not introduced): a mid-op I/O/`NoSpace` failure leaves uncommitted in-flight blocks on
/// the shared cached mount that a later persist's commit would flush — the crate exposes no PUBLIC in-place
/// unwind (`txn_unwind` is private; `create_files_batch` cannot express create-or-replace-with-removal), so
/// the brief's "reload from committed root" is not expressible here. The autocommit-ON path shares this
/// exact class (a failed op's writes are also left uncommitted-in-flight). True closure = a crate-side
/// public rollback, out of the pi lane (see SECURITY.md §K1 K9 and the landing report).
pub fn native_acl_write_on(
    fs: &mut KernelUnaFS,
    dir_lba: u64,
    dir_off: u32,
    name: &str,
    first_cluster: u32,
    owner: &[u8],
    grants: &[(&[u8], &[u8])],
) -> bool {
    #[cfg(feature = "nsspan")] let _cs0 = fs.commit_stats(); fs.set_autocommit(false);
    let staged = native_acl_stage_row(fs, dir_lba, dir_off, name, first_cluster, owner, grants);
    // Commit ONLY on full staging success: a failed stage flips no root (durable-first — old row intact).
    let ok = staged && fs.commit().is_ok();
    // K9-PARITY: on FAILURE (staging aborted OR the single commit errored) the mount carries uncommitted
    // in-flight blocks — ask the enclosing `with_unafs` hold to discard the cached mount so no later
    // persist's commit flushes this orphaned residue. The root never flipped, so nothing durable changed
    // (K3); this only reloads the committed root. See `with_unafs` and SECURITY.md §K1 K9.
    if !ok {
        request_mount_discard();
    }
    fs.set_autocommit(true); #[cfg(feature = "nsspan")] { let cs1 = fs.commit_stats(); ACL_PERSIST_FLIPS.fetch_max(cs1.commits.wrapping_sub(_cs0.commits), core::sync::atomic::Ordering::Relaxed); ACL_PERSIST_BLOCKS.fetch_max(cs1.blocks_written.wrapping_sub(_cs0.blocks_written), core::sync::atomic::Ordering::Relaxed); }
    ok
}

/// K9-MASKCUT: the STAGING body — create-or-resolve the ACL file and (re)write its typed attributes as a
/// sequence of journaled `set_attribute`/`remove_attribute` ops. The caller ([`native_acl_write_on`]) holds
/// autocommit OFF around this and issues the SINGLE commit, so these ops stage into one transaction (one
/// root flip) rather than one flip apiece. Owns every early return; returns `true` iff every op staged
/// cleanly. Body is the pre-K9 `native_acl_write_on` verbatim — no change to what is written.
fn native_acl_stage_row(
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
        // K9-PARITY (test only): mid-staging failure injection. Fires HERE — after a fresh inode + the
        // name/fc/owner attributes have already staged into the autocommit-OFF transaction — so the abort
        // leaves a near-complete, UNCOMMITTED row (the worst-case residue the discard must swallow). Never
        // set in production; the `k9_parity_check` witness sets it transiently. See `with_unafs`.
        if TEST_FAIL_MIDSTAGE.load(core::sync::atomic::Ordering::Relaxed) {
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
    match with_unafs_write(|fs| {
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

// =====================================================================
// K8c: the snapshot READ path under CURRENT-ACL enforcement.
//
// Ruling of record (Peter, 2026-07-16, "we want high security"): snapshot
// reads are governed by the LIVE object's CURRENT ACL, re-evaluated at read
// time. Revocation is total — a principal that cannot read the live object
// cannot read ANY snapshot of it. Snapshots preserve bytes, never authority.
//
// ONE evaluator at this layer ([`read_authz`]): every snapshot-read surface
// (`snap cat`, `snap ls`, [`snapshot_read`]) defers to it, keyed on the LIVE
// inode identity. HONESTY NOTE (lens A fold, 2026-07-16): this enforces the
// same SEMANTICS as the live syscall path — current-ACL, CAP_READ-equivalent
// grant rights (ONE decoder, [`rights_from_native`] below, which the syscall
// layer's grant machinery delegates to; the bit is const-asserted equal to
// CAP_READ — never a lookalike), fail-closed on a deleted object — but it is a
// kernel-verb-layer evaluator DISTINCT from the syscall layer's
// OwnedFile/FileGrant machinery. Unifying the two evaluators is a ledgered
// follow-up (SECURITY.md K8c entry). A native unafs object carries its ACL as
// its own typed attributes — `owner` (String) and one `grants:<principal>` per
// grantee holding an `rw`/`r`/`w` rights value (the K6 convention
// `rights_native_value` writes).
// =====================================================================

/// The principal a kernel-authority surface (the shell) presents. Kernel
/// authority reads any LIVE object; it is NOT a bypass of the deleted-object
/// fail-closed edge (see [`read_authz`]).
pub const KERNEL_PRINCIPAL: &str = "kernel";

/// The READ right bit of the capability model — BY DEFINITION equal to the
/// syscall layer's `CAP_READ` (1 << 0), const-asserted there so the two can
/// never drift (this module compiles in every aarch64 config; the syscall
/// layer is baremetal-gated, hence the bit lives here and the assert there).
pub(crate) const RIGHT_READ: u32 = 1 << 0;
/// The WRITE right bit — equal to the syscall layer's `CAP_WRITE` (1 << 1),
/// const-asserted there.
pub(crate) const RIGHT_WRITE: u32 = 1 << 1;

/// THE canonical decoder for a grant's native rights value (the K6 `rw`/`r`/
/// `w`/`-` encoding `rights_native_value` writes): `rw`->R|W, `r`->R, `w`->W;
/// anything else -> 0 (no rights = not a live grant). ONE implementation:
/// the syscall layer's mount-time ACL rebuild (`rights_from_native` in
/// `arch/aarch64/syscall.rs`) DELEGATES here, so the verb-layer authz and the
/// syscall-layer grant machinery decode rights through the same function.
pub(crate) fn rights_from_native(s: &[u8]) -> u32 {
    match s {
        b"rw" => RIGHT_READ | RIGHT_WRITE,
        b"r" => RIGHT_READ,
        b"w" => RIGHT_WRITE,
        _ => 0,
    }
}

/// A current-ACL read decision, traced honestly on the serial log so the
/// deleted-from-live fail-closed edge is visible, never silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadAuthz {
    /// The principal may read (owner, grantee, kernel authority, or a public
    /// live object with no owner row).
    Permit,
    /// The live object exists but its current ACL admits neither this principal
    /// nor kernel authority — revocation of a formerly-granted principal lands
    /// here, and it withholds the snapshot bytes too (revocation reaches the past).
    DenyAcl,
    /// The live object is GONE (deleted from the live tree): there is no live
    /// ACL row, so the fail-closed default refuses — for EVERY principal,
    /// kernel authority included. Deletion is the ultimate revocation; the
    /// retained bytes survive on disk but no one reads them through this path.
    DenyNoLiveObject,
}

/// The current-ACL READ evaluator every snapshot-read surface defers to (K8c).
/// It consults ONLY the LIVE inode `live_id` (its `owner` + `grants:<principal>`
/// attributes), never a snapshot's, so a snapshot read inherits exactly the live
/// object's present-day authority. Same SEMANTICS as the syscall layer's live
/// read check; a distinct evaluator (see the section note above).
///
/// Order matters: the deleted-object check comes FIRST, before the kernel-
/// authority shortcut, so a deleted live object fails closed uniformly (the
/// brief's explicit ruling — no owner row = refuse; documented consequence:
/// even the kernel shell cannot read a snapshot of a deleted object through
/// this path). Then kernel authority permits; then a public object (no `owner`)
/// permits; then the owner permits; then a `grants:<principal>` row permits
/// ONLY if its rights value carries the READ right — decoded by
/// [`rights_from_native`], the ONE decoder the syscall layer's grant machinery
/// also uses (it delegates here), tested against [`RIGHT_READ`] == the syscall
/// layer's `CAP_READ` bit, const-asserted (lens A fix: a write-only `w` grant
/// reads NEITHER live NOR snapshot — the grant model's exact CAP_READ
/// semantics, not key-presence). Else DenyAcl.
pub fn read_authz(fs: &mut KernelUnaFS, live_id: u64, principal: &str) -> ReadAuthz {
    // Fail closed on a live object that no longer exists — checked before the
    // kernel shortcut so deletion is a total, uniform revocation.
    let ino = match fs.read_inode(live_id) {
        Ok(i) => i,
        Err(_) => return ReadAuthz::DenyNoLiveObject,
    };
    if principal == KERNEL_PRINCIPAL {
        return ReadAuthz::Permit;
    }
    let owner = match ino.attributes.get("owner") {
        Some(AttributeValue::String(s)) if s != crate::fs::rootacl::SYSTEM_OWNER => s.clone(), // ROOTACL (B456): `system` reads for everyone
        // No owner row: a public live object, readable by all (unchanged public
        // semantics). Only ABSENCE OF THE OBJECT (above) fails closed.
        _ => return ReadAuthz::Permit,
    };
    if principal == owner {
        return ReadAuthz::Permit;
    }
    if let Some(AttributeValue::String(rights)) = ino
        .attributes
        .get(&alloc::format!("grants:{}", principal))
    {
        // Rights-aware (lens A): the grant admits a READ iff it carries the
        // read right — the same decoder + bit (CAP_READ, const-asserted) the
        // syscall layer uses.
        if rights_from_native(rights.as_bytes()) & RIGHT_READ != 0 {
            return ReadAuthz::Permit;
        }
    }
    ReadAuthz::DenyAcl
}

/// The result of a current-ACL snapshot read.
#[derive(Debug, Clone, PartialEq)]
pub enum SnapReadResult {
    /// Permitted — the retained bytes AS OF the snapshot.
    Ok(Vec<u8>),
    /// The path does not resolve within the snapshot (never existed there).
    NotInSnapshot,
    /// The current ACL refused the read; the decision (`DenyAcl` /
    /// `DenyNoLiveObject`) is carried so the caller can trace WHY.
    Refused(ReadAuthz),
    /// No retained root carries this generation (dropped or never taken).
    SnapshotMissing,
}

/// Read a file from a retained root under CURRENT-ACL enforcement (K8c). The
/// object is resolved in the SNAPSHOT (frozen bytes), but the authority is the
/// LIVE object of the same stable logical id, decided by [`read_authz`]
/// (live-read semantics: current-ACL, CAP_READ grant rights, fail-closed on a
/// deleted object). Permitted → the snapshot's bytes; refused → the traced
/// reason (an impostor or rights-lacking grantee is `DenyAcl`, a live-deleted
/// object is `DenyNoLiveObject`, fail-closed).
pub fn snapshot_read(
    generation: u64,
    path: &str,
    principal: &str,
) -> Result<SnapReadResult, MountError> {
    with_unafs(|fs| {
        // Phase 1: resolve the object's logical id in the snapshot (frozen view
        // borrows fs; scope it so the ACL check below can re-borrow fs).
        let sid = {
            let mut view = match fs.open_snapshot(generation) {
                Ok(v) => v,
                Err(FileSystemError::SnapshotNotFound(_)) => {
                    return SnapReadResult::SnapshotMissing
                }
                Err(_) => return SnapReadResult::NotInSnapshot,
            };
            match view.resolve_path(path) {
                Ok(id) => id,
                Err(_) => return SnapReadResult::NotInSnapshot,
            }
        };

        // Phase 2: CURRENT-ACL — authorize against the LIVE inode of that id.
        match read_authz(fs, sid, principal) {
            ReadAuthz::Permit => {}
            deny => return SnapReadResult::Refused(deny),
        }

        // Phase 3: permitted — hand back the retained bytes (reopen the view;
        // the phase-1 borrow is released).
        let mut view = match fs.open_snapshot(generation) {
            Ok(v) => v,
            Err(FileSystemError::SnapshotNotFound(_)) => {
                return SnapReadResult::SnapshotMissing
            }
            Err(_) => return SnapReadResult::NotInSnapshot,
        };
        let size = match view.read_inode(sid) {
            Ok(i) => i.size,
            Err(_) => return SnapReadResult::NotInSnapshot,
        };
        match view.read_data(sid, 0, size) {
            Ok(bytes) => SnapReadResult::Ok(bytes),
            Err(_) => SnapReadResult::NotInSnapshot,
        }
    })
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
/// bit5 (root `ls`) requires the two staged fixture files present + readable and
/// every other root entry to be a legitimate native ACL row file (`acl-*`, the
/// on-disk sidecar successor that boot migration + `native_acl_write` place in
/// this root) — NOT an exact count of two, which dropped the bit (`w=0x1df`) on
/// any metal card carrying a live owner row. A leaked scratch fixture still fails
/// the bit, so the self-clean discipline stays protected.
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
        if (::unafs::superblock::MIN_SUPPORTED_VERSION..=::unafs::superblock::VERSION).contains(&fs.superblock.version)
            && fs.superblock.block_size as u64 == ::unafs::BLOCK_SIZE
        {
            r |= 1 << 2;
        }
        // bit3: the volume fits the partition that carries it.
        if fs.superblock.block_count <= span.block_count {
            r |= 1 << 3;
        }

        // bit5: `ls /` finds the two staged fixture FILES, and every OTHER root entry
        // is a legitimate native ACL row file (`acl-<lba>-<off>`, see `acl_file_name`).
        //
        // The native ACL store — the sidecar's on-disk successor (K6/K7) — writes its
        // rows AS FILES into THIS root: boot-time `native_migrate_from_sidecar` (run at
        // the head of `u7_launcher`, BEFORE this witness) materialises a real owned
        // file's committed row into an `acl-*` file the moment a metal card carries one.
        // That row is durable, by-design security state (never residue), so an exact
        // `entries.len() == 2` was over-strict against legitimate volumes and dropped
        // this bit (`w=0x1df`) on any card with a live owner row. The honest assertion:
        // both fixtures present + readable, and no UNEXPECTED entry — a leaked scratch
        // fixture (K4TEST/K8CUT/…, none `acl-`-prefixed) still fails this bit, so the
        // fixtures' self-clean discipline stays genuinely protected.
        if let Ok(entries) = fs.ls(fs.superblock.root_inode) {
            let hello = entries
                .iter()
                .find(|e| e.name == "K3HELLO.TXT" && e.kind == ::unafs::FileKind::File);
            let pat = entries
                .iter()
                .find(|e| e.name == "K3PAT.BIN" && e.kind == ::unafs::FileKind::File);
            let only_fixtures_and_acl = entries.iter().all(|e| {
                e.name == "K3HELLO.TXT" || e.name == "K3PAT.BIN" || e.name.starts_with("acl-")
            });
            if hello.is_some() && pat.is_some() && only_fixtures_and_acl {
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
/// Tick source: `CNTPCT_EL0` on aarch64; on x86 (UNAFSX86) the TSC through
/// `arch::now_cycles()` — a real counter, never a 0 that reads as "free".
fn bench_ticks() -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch::timer::cntpct()
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        crate::arch::now_cycles()
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

/// K8c scratch fixture — an OWNED native object the witness snapshots, revokes,
/// and deletes; self-cleaning (K2 idiom) so the STATEFUL card never accrues.
const K8C_NAME: &str = "K8CSNAP.TXT";
const K8C_PATH: &str = "/K8CSNAP.TXT";
/// Multi-block payloads: OLD is what the snapshot must keep serving to the
/// permitted principals; NEW is the live divergence the snapshot never shows.
const K8C_OLD: &[u8] = &[0xC3u8; 3 * 4096];
const K8C_NEW: &[u8] = &[0xD4u8; 3 * 4096];

/// K8c-snapread witness (uncounted): prove the snapshot READ path enforces the
/// LIVE object's CURRENT ACL — the "high security" ruling (revocation reaches
/// the past). Snapshots preserve bytes, never authority.
///
/// A native object carries its ACL as its own attributes (`owner` + one
/// `grants:<p>` per grantee holding an `rw`/`r`/`w` rights value). The witness
/// sets owner=alice, grants bob `r` and carol `w` (write-only), retains the
/// tree, overwrites the live file, then reads the SNAPSHOT as several
/// principals — every decision routed through the SAME [`read_authz`] every
/// snapshot-read surface uses.
///
/// Sequence (8 bits):
///   bit0 setup: create the owned scratch (owner=alice, grants:bob=r,
///        grants:carol=w) with OLD bytes, retain it, overwrite the LIVE file
///        with NEW — snapshot present;
///   bit1 the OWNER (alice) reads the snapshot -> the OLD bytes (permit + faithful);
///   bit2 the READ-GRANTEE (bob) reads the snapshot -> the OLD bytes (grant honored);
///   bit3 an IMPOSTOR (mallory) is REFUSED from the snapshot (DenyAcl) AND
///        refused the LIVE object by the same evaluator (refused live <=> refused snapshot);
///   bit4 RIGHTS-AWARE (lens A): the WRITE-ONLY grantee (carol, `w`) is REFUSED
///        the snapshot read (DenyAcl) AND refused live — a grant without the
///        READ right (CAP_READ) reads neither, matching the syscall layer;
///   bit5 REVOCATION: drop bob's live grant; bob now REFUSED from the snapshot
///        he could read a moment ago (DenyAcl) — the money line: revocation
///        reaches the past;
///   bit6 DELETED-OBJECT EDGE: unlink the live object; even the OWNER (alice)
///        is REFUSED (DenyNoLiveObject) — no live ACL row, fail closed, traced;
///   bit7 self-clean: drop the snapshot + remount, no leak, fsck clean.
///
/// Skips honestly on media without a unafs partition. Self-cleaning.
pub fn k8c_snapread_selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    if let Err(e) = locate() {
        serial_println!(":: K8c-snapread: no unafs volume ({:?}) — skipped ::", e);
        return;
    }

    let mut w = 0u32;
    let t0 = bench_ticks();

    // Start clean: drop stray snapshots + scratch from a prior run.
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
        let _ = fs.unlink(root, K8C_NAME);
    });

    // bit0: create the owned scratch (owner=alice, grants:bob) with OLD bytes,
    // retain, then diverge the live file to NEW.
    let (created_gen, live_id) = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let id = match fs.create_file(root, String::from(K8C_NAME)) {
            Ok(id) => id,
            Err(_) => return (None, 0),
        };
        if fs.write_data(id, 0, K8C_OLD).is_err() {
            return (None, 0);
        }
        if fs
            .set_attribute(id, String::from("owner"), AttributeValue::String(String::from("alice")))
            .is_err()
        {
            return (None, 0);
        }
        if fs
            .set_attribute(id, String::from("grants:bob"), AttributeValue::String(String::from("r")))
            .is_err()
        {
            return (None, 0);
        }
        // carol: WRITE-ONLY grant — the rights-aware negative (bit4).
        if fs
            .set_attribute(id, String::from("grants:carol"), AttributeValue::String(String::from("w")))
            .is_err()
        {
            return (None, 0);
        }
        let g = match fs.snapshot_create(String::from("k8c-before"), String::from("alice"), bench_ticks()) {
            Ok(g) => g,
            Err(_) => return (None, 0),
        };
        if fs.write_data(id, 0, K8C_NEW).is_err() {
            return (None, 0);
        }
        (Some(g), id)
    })
    .unwrap_or((None, 0));
    let snap_gen = match created_gen {
        Some(g) => g,
        None => {
            serial_println!(":: K8c-snapread: setup failed (create/own/retain) FAIL [w=0x00] ::");
            return;
        }
    };
    if with_unafs(|fs| {
        fs.snapshot_index()
            .map(|s| s.iter().any(|e| e.generation == snap_gen))
            .unwrap_or(false)
    })
    .unwrap_or(false)
    {
        w |= 1 << 0;
    }

    // bit1: the OWNER reads the snapshot -> OLD bytes.
    if matches!(
        snapshot_read(snap_gen, K8C_PATH, "alice"),
        Ok(SnapReadResult::Ok(ref b)) if b.as_slice() == K8C_OLD
    ) {
        w |= 1 << 1;
    }

    // bit2: the GRANTEE reads the snapshot -> OLD bytes.
    if matches!(
        snapshot_read(snap_gen, K8C_PATH, "bob"),
        Ok(SnapReadResult::Ok(ref b)) if b.as_slice() == K8C_OLD
    ) {
        w |= 1 << 2;
    }

    // bit3: an IMPOSTOR is refused from the snapshot AND refused the LIVE object
    // by the same predicate (refused live <=> refused snapshot).
    let refused_snapshot = matches!(
        snapshot_read(snap_gen, K8C_PATH, "mallory"),
        Ok(SnapReadResult::Refused(ReadAuthz::DenyAcl))
    );
    let refused_live = with_unafs(|fs| read_authz(fs, live_id, "mallory") == ReadAuthz::DenyAcl)
        .unwrap_or(false);
    if refused_snapshot && refused_live {
        w |= 1 << 3;
    }

    // bit4: RIGHTS-AWARE (lens A fix) — the WRITE-ONLY grantee (carol, `w`)
    // holds a grant row but NOT the READ right: refused from the snapshot AND
    // refused live, by the same evaluator (CAP_READ semantics, not key-presence).
    let carol_refused_snapshot = matches!(
        snapshot_read(snap_gen, K8C_PATH, "carol"),
        Ok(SnapReadResult::Refused(ReadAuthz::DenyAcl))
    );
    let carol_refused_live = with_unafs(|fs| read_authz(fs, live_id, "carol") == ReadAuthz::DenyAcl)
        .unwrap_or(false);
    if carol_refused_snapshot && carol_refused_live {
        w |= 1 << 4;
    }

    // bit5: REVOCATION reaches the past — drop bob's live grant; bob, who read
    // the snapshot at bit2, is now refused from the very same snapshot.
    let revoked = with_unafs(|fs| fs.remove_attribute(live_id, "grants:bob").is_ok())
        .unwrap_or(false);
    if revoked
        && matches!(
            snapshot_read(snap_gen, K8C_PATH, "bob"),
            Ok(SnapReadResult::Refused(ReadAuthz::DenyAcl))
        )
    {
        w |= 1 << 5;
    }

    // bit6: DELETED-OBJECT EDGE — unlink the live object; even the OWNER is
    // refused (no live ACL row -> DenyNoLiveObject, fail closed, traced).
    let deleted = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        fs.unlink(root, K8C_NAME).is_ok()
    })
    .unwrap_or(false);
    let owner_refused_after_delete = matches!(
        snapshot_read(snap_gen, K8C_PATH, "alice"),
        Ok(SnapReadResult::Refused(ReadAuthz::DenyNoLiveObject))
    );
    if deleted && owner_refused_after_delete {
        w |= 1 << 6;
    } else if deleted {
        // Trace the edge honestly whether or not it scored.
        serial_println!(":: K8c-snapread: deleted-object edge did not fail closed as expected ::");
    }

    // bit7: self-clean — drop the snapshot + remount; no leak, fsck clean.
    let _ = with_unafs(|fs| fs.snapshot_drop(snap_gen));
    force_remount();
    let cleaned = with_unafs(|fs| {
        fs.resolve_path(K8C_PATH).is_err()
            && fs.snapshot_index().map(|s| s.is_empty()).unwrap_or(false)
            && fs.fsck(false).map(|r| r.is_clean()).unwrap_or(false)
    })
    .unwrap_or(false);
    if cleaned {
        w |= 1 << 7;
    }

    let ticks = bench_ticks().wrapping_sub(t0);
    let verdict = if w == 0xff { "PASS" } else { "FAIL" };
    serial_println!(
        ":: K8c-snapread: current-ACL snapshot reads (owner+grantee read OLD, impostor refused live<->snap, write-only grant refused, revocation reaches the past, deleted-object fails closed) + self-clean {} [w={:#04x}] ticks={} ::",
        verdict,
        w,
        ticks
    );
}

/// F2 scratch fixtures: created, renamed, stripped, unlinked — the volume is
/// left exactly as found (self-cleaning, K2 idiom).
const F2_SRC: &str = "F2SRC.TXT";
const F2_DST: &str = "F2DST.TXT";
const F2_OTHER: &str = "F2OTHER.TXT";
const F2_SRC_PATH: &str = "/F2SRC.TXT";
const F2_DST_PATH: &str = "/F2DST.TXT";
const F2_PAYLOAD: &[u8] = b"F2: a rename moves a name, never the bytes.\n";

/// F2-mutations witness (uncounted): prove the full mutation set — `rename`,
/// `remove_attribute`, `unlink` — on the LIVE kernel mount, each a single
/// atomic CoW transaction through the one IRQ-masked `with_unafs` hold, and
/// each DURABLE across a genuine remount.
///
/// Sequence (7 bits):
///   bit0 stage: create the scratch file with bytes + two typed attributes;
///   bit1 RENAME durability: rename src -> dst, genuine remount, the OLD name
///        is negative, the NEW name resolves to the SAME inode id, and the
///        bytes are byte-identical (no data copy — the name moved, not the
///        extents);
///   bit2 collision REFUSED cleanly: a rename onto an existing name returns
///        FileExists and disturbs NOTHING (both names still resolve);
///   bit3 REMOVE_ATTRIBUTE: the targeted attribute is gone after a remount
///        while the sibling attribute on the same inode is untouched;
///   bit4 UNLINK durability: unlink + genuine remount — the name is negative
///        and the stale inode id fails NotFound (never aliases another file);
///   bit5 negative paths: a missing source refuses BOTH rename and unlink
///        (NotFound) — refusals, not silent successes. (The IsADirectory
///        refusal is pinned host-side, not here: the crate has no `rmdir`, so
///        a directory made by this witness could never be cleaned up.)
///   bit6 self-clean: the fresh mount is refcount-consistent (fsck) and no F2
///        scratch name survives in the root.
///
/// Skips honestly on media without a unafs partition. Runs AFTER the K8
/// witnesses, so its churn can never perturb their block accounting.
pub fn f2_mutations_selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    if let Err(e) = locate() {
        serial_println!(":: F2-mutations: no unafs volume ({:?}) — skipped ::", e);
        return;
    }

    let t0 = bench_ticks();
    let mut w = 0u32;

    // bit0: stage. Clear any stale scratch from an interrupted run first.
    let staged = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let _ = fs.unlink(root, F2_SRC);
        let _ = fs.unlink(root, F2_DST);
        let _ = fs.unlink(root, F2_OTHER);
        let id = match fs.create_file(root, String::from(F2_SRC)) {
            Ok(id) => id,
            Err(_) => return 0,
        };
        if fs.write_data(id, 0, F2_PAYLOAD).is_err() {
            return 0;
        }
        if fs
            .set_attribute(id, String::from("f2kind"), AttributeValue::String(String::from("scratch")))
            .is_err()
        {
            return 0;
        }
        if fs
            .set_attribute(id, String::from("f2keep"), AttributeValue::Int(1))
            .is_err()
        {
            return 0;
        }
        // A second file, so the collision refusal below has a real target.
        if fs.create_file(root, String::from(F2_OTHER)).is_err() {
            return 0;
        }
        id
    })
    .unwrap_or(0);
    if staged != 0 {
        w |= 1 << 0;
    }

    // bit1: rename + genuine remount — same inode, same bytes, old name gone.
    let renamed = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        fs.rename(root, F2_SRC, root, F2_DST).is_ok()
    });
    force_remount();
    let rename_durable = matches!(renamed, Ok(true))
        && with_unafs(|fs| {
            let moved = match fs.resolve_path(F2_DST_PATH) {
                Ok(id) => id,
                Err(_) => return false,
            };
            let bytes_ok = match (fs.read_inode(moved), fs.read_data(moved, 0, F2_PAYLOAD.len() as u64)) {
                (Ok(inode), Ok(data)) => inode.size == F2_PAYLOAD.len() as u64 && data == F2_PAYLOAD,
                _ => false,
            };
            // The name moved, NOT the inode: the id is the one we staged.
            moved == staged && bytes_ok && fs.resolve_path(F2_SRC_PATH).is_err()
        })
        .unwrap_or(false);
    if rename_durable {
        w |= 1 << 1;
    }

    // bit2: a rename onto an EXISTING name is refused, and nothing moves.
    let collision_refused = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let refused = matches!(
            fs.rename(root, F2_DST, root, F2_OTHER),
            Err(FileSystemError::FileExists)
        );
        // Both names survive the refusal, untouched.
        refused
            && fs.resolve_path(F2_DST_PATH).is_ok()
            && fs.resolve_path("/F2OTHER.TXT").is_ok()
    })
    .unwrap_or(false);
    if collision_refused {
        w |= 1 << 2;
    }

    // bit3: remove_attribute — the target goes, the sibling stays, durably.
    let attr_removed = with_unafs(|fs| {
        let id = match fs.resolve_path(F2_DST_PATH) {
            Ok(id) => id,
            Err(_) => return false,
        };
        fs.remove_attribute(id, "f2kind").is_ok()
    })
    .unwrap_or(false);
    force_remount();
    let attr_durable = attr_removed
        && with_unafs(|fs| {
            let id = match fs.resolve_path(F2_DST_PATH) {
                Ok(id) => id,
                Err(_) => return false,
            };
            let gone = matches!(fs.get_attribute(id, "f2kind"), Ok(None));
            let kept = matches!(
                fs.get_attribute(id, "f2keep"),
                Ok(Some(AttributeValue::Int(1)))
            );
            gone && kept
        })
        .unwrap_or(false);
    if attr_durable {
        w |= 1 << 3;
    }

    // bit4: unlink + genuine remount — the name is negative and the STALE
    // inode id fails NotFound (logical ids are never recycled).
    let unlinked = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        fs.unlink(root, F2_DST).is_ok() && fs.unlink(root, F2_OTHER).is_ok()
    });
    force_remount();
    let unlink_durable = matches!(unlinked, Ok(true))
        && with_unafs(|fs| {
            fs.resolve_path(F2_DST_PATH).is_err()
                && fs.resolve_path(F2_SRC_PATH).is_err()
                && fs.read_inode(staged).is_err()
        })
        .unwrap_or(false);
    if unlink_durable {
        w |= 1 << 4;
    }

    // bit5: negative paths — a missing source refuses BOTH rename and unlink.
    // Refusals, never silent successes.
    //
    // The IsADirectory refusal is deliberately NOT exercised here: the crate
    // has no `rmdir`, so any directory this witness created would leak into
    // the root forever and break `k3_mount_selftest`'s no-unexpected-entry
    // bit on every later boot. Self-cleaning outranks coverage — that refusal
    // is pinned host-side instead, by the crate's
    // `unlink_refuses_directories_and_missing_names`.
    let negatives = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let rename_refused = matches!(
            fs.rename(root, "F2GHOST.TXT", root, "F2REAL.TXT"),
            Err(FileSystemError::NotFound)
        );
        let unlink_refused = matches!(fs.unlink(root, "F2GHOST.TXT"), Err(FileSystemError::NotFound));
        rename_refused && unlink_refused
    })
    .unwrap_or(false);
    if negatives {
        w |= 1 << 5;
    }

    // bit6: self-clean — fsck consistent and no F2 scratch name in the root.
    force_remount();
    let clean = with_unafs(|fs| {
        let root = fs.superblock.root_inode;
        let consistent = fs.fsck(false).map(|r| r.is_clean()).unwrap_or(false);
        let no_leak = fs
            .ls(root)
            .map(|entries| {
                entries
                    .iter()
                    .all(|d| d.name != F2_SRC && d.name != F2_DST && d.name != F2_OTHER)
            })
            .unwrap_or(false);
        consistent && no_leak
    })
    .unwrap_or(false);
    if clean {
        w |= 1 << 6;
    }

    let ticks = bench_ticks().wrapping_sub(t0);
    let verdict = if w == 0x7f { "PASS" } else { "FAIL" };
    serial_println!(
        ":: F2-mutations: rename (durable, same inode, bytes intact) + collision refused + remove_attribute (sibling intact) + unlink (stale id NotFound) + negatives + self-clean {} [w={:#04x}] ticks={} ::",
        verdict,
        w,
        ticks
    );
}

// K9-MASKCUT WATCH (nsspan-gated, EOF so knob-off adds/moves nothing above): worst-case flip + block
// count of a SINGLE ACL row persist through `native_acl_write_on`, captured across the K3/K5 fixtures.
// `nsspan_report` (syscall.rs) emits these next to the per-site tick spans. Post-K9 `flips` = 1 (the
// staged batch's one commit) vs the pre-K9 per-op regime's ~(4 + stale + grants) flips — the sector/flip
// reduction the arc exists to prove, QEMU-observable (unlike the TCG-blind tick number). No lock, no heap.
#[cfg(feature = "nsspan")]
pub static ACL_PERSIST_FLIPS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "nsspan")]
pub static ACL_PERSIST_BLOCKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// PARTITION-FITS: the containment verdict, the `unafs span check` witness line, and the overrun
/// scream — the tail half of [`partition_witness`].
///
/// AT EOF ON PURPOSE. `panic::Location` embeds source line numbers, so a body grown mid-file
/// rewrites every location string below it in this arch-neutral module and moves bytes in images
/// that see no behavioural change at all. Appending here moves nothing above it — the same
/// discipline the K9-MASKCUT statics above follow.
///
/// WHAT IT COMPARES, AND WHY THAT IS THE FIX. `span.block_count` is the unafs crate's own
/// `p.sector_count / SECTORS_PER_BLOCK`, so testing it back against `p.sector_count` was a
/// tautology (see [`partition_witness`]). The superblock's `block_count` is a SECOND, INDEPENDENT
/// source: it is written at format time and read back off the medium here, through the bounded
/// partition-relative path. Comparing the two is exactly `k3_mount_selftest` bit3 — which runs on
/// NEITHER board's flashed image, for two DIFFERENT reasons: on tegra its spawn site sits below
/// `tegra_early_stop`'s `-> !`; on the Pi it is `witness`-gated and the media `K8_FEATS` omits it.
/// So on ANY card that boots THIS is the only check; every board, first mount attempt, pass or fail.
///
/// `sb0` is `Some` only when the magic just verified on that same read, and that gate is
/// load-bearing: the `Superblock` record (unafs-records.md §R2) is all fixed-width, so a decode of
/// arbitrary bytes SUCCEEDS with garbage. A non-UnaFS or unreadable block 0 therefore yields
/// `sb_blocks=? fits=?` and never a scream. The decode is prefix-only (trailing block padding is
/// ignored) and deliberately skips `Superblock::from_bytes`, whose `validate` would also reject a
/// version this build does not know — a format bump must not be reported as a containment breach.
///
/// `sb_blocks=` prints on AGREEMENT too, not only on failure. A guard whose input never appears in
/// the log cannot be read, only trusted, and a trusted field is what this replaced.
///
/// SCREAM, NOT REFUSE. `BlockAdapter::for_partition` already bounds every access at
/// `span.block_count`, so an over-declaring volume is enforced against STRUCTURALLY — it gets
/// `OutOfBounds`, never a cross-partition access. What was missing was visibility, so an overrun
/// gets its own line ending `=> FAIL ::`, a builtin mbench FORBID (`scripts/mbench.py`,
/// `DEFAULT_FORBIDS`): a violation reddens every spec on every board with no spec edit anywhere.
/// The witness stays advisory about the mount itself, exactly as its own docs promise.
fn span_fit_report(
    p: &block::PartitionEntry,
    span: &PartitionSpan,
    sb0: Option<&[u8; 512]>,
    magic_ok: bool,
) {
    let declared = sb0.and_then(|b| {
        ::unafs::codec::deserialize_block::<::unafs::Superblock>(b)
            .ok()
            .map(|sb| sb.block_count)
    });
    match declared {
        Some(n) => serial_println!(
            ":: PART: unafs span check — slot={} type=0x{:02x} part=[{}..{}) span_base={} span_blocks={} sb_blocks={} fits={} magic={} ::",
            p.slot,
            p.type_byte,
            p.start_lba,
            p.end_lba(),
            span.base_lba,
            span.block_count,
            n,
            if n <= span.block_count { "yes" } else { "NO" },
            if magic_ok { "ok" } else { "MISSING" }
        ),
        None => serial_println!(
            ":: PART: unafs span check — slot={} type=0x{:02x} part=[{}..{}) span_base={} span_blocks={} sb_blocks=? fits=? magic={} ::",
            p.slot,
            p.type_byte,
            p.start_lba,
            p.end_lba(),
            span.base_lba,
            span.block_count,
            if magic_ok { "ok" } else { "MISSING" }
        ),
    }
    if let Some(n) = declared {
        if n > span.block_count {
            serial_println!(
                ":: PART: unafs span check — volume declares {} blocks but MBR slot {} carries only {} => FAIL ::",
                n,
                p.slot,
                span.block_count
            );
        }
    }
}

/// UNAFSX86 M4 (rmbp-ledger B298): `tests unafs` — the whole seam on the ROOT the boot bound.
///
/// Walks the path a real program's file takes and then checks it from underneath: (1) `create` a file
/// under `/home` through the MOUNT TABLE (so it lands on whichever backend owns `/`), (2) `write` it
/// through the table, (3) `force_remount` and re-read it through [`with_unafs`] (the bytes reached the
/// medium, not a RAM cache), (4) set and get the `owner` attribute through the crate's existing API (no
/// VFS attribute method — ATTRSURF owns that surface), (5) `unlink` through the table and confirm it is
/// gone. SKIP when `/` is not the native volume (a FAT-only card) — the fixture never invents a volume.
#[cfg(feature = "unafs")]
pub fn unafsx86_selftest() {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL};
    const DIR: &str = "/home";
    const PATH: &str = "/home/unafsx86.tst";
    const BODY: &[u8] = b"UNAFSX86 seam: create, write, remount, attr, unlink";
    const OWNER: &str = "user:unafsx86";
    let mt = crate::shell::vfs_mount_table();
    let root = mt.volume_name("/").unwrap_or_else(|_| String::from("-"));
    if root != "native" {
        serial_println!(":: UNAFSX86-T: root={} (not unafs) -> SKIP ::", root);
        return;
    }
    let _ = mt.stat(DIR).or_else(|_| mt.create(DIR, NodeKind::Dir, KERNEL_PRINCIPAL));
    let _ = mt.unlink(PATH, KERNEL_PRINCIPAL); // a stale scratch from an interrupted run
    let create = mt.create(PATH, NodeKind::File, KERNEL_PRINCIPAL).is_ok();
    let write = create && mt.write(PATH, 0, BODY, KERNEL_PRINCIPAL).map(|n| n == BODY.len()).unwrap_or(false);
    force_remount();
    let reread = matches!(
        with_unafs(|fs| match fs.resolve_path(PATH) {
            Ok(id) => fs.read_data(id, 0, BODY.len() as u64).map(|d| d == BODY).unwrap_or(false),
            Err(_) => false,
        }),
        Ok(true)
    );
    let owner = matches!(
        with_unafs(|fs| {
            let id = fs.resolve_path(PATH).ok()?;
            fs.set_attribute(id, String::from("owner"), AttributeValue::String(String::from(OWNER))).ok()?;
            match fs.get_attribute(id, "owner").ok()? {
                Some(AttributeValue::String(v)) => Some(v == OWNER),
                _ => Some(false),
            }
        }),
        Ok(Some(true))
    );
    let unlink = mt.unlink(PATH, KERNEL_PRINCIPAL).is_ok() && mt.stat(PATH).is_err();
    let w = |b: bool| if b { "ok" } else { "no" };
    let pass = create && write && reread && owner && unlink;
    serial_println!(
        ":: UNAFSX86-T: root=unafs create={} write={} remount-read={} owner={} unlink={} -> {} ::",
        w(create), w(write), w(reread), w(owner), w(unlink),
        if pass { "PASS" } else { "FAIL" }
    );
}

// =================================================================================================
// BOOT80 (rmbp-ledger B350) — multi-block I/O and the read-ahead window for the one mount's device.
// Boot 21's 60 s: every 4 KiB block was eight single-sector commands, and every commit rewrote the
// whole refcount map. The crate now hands the device a block per call (`read_sectors` /
// `write_sectors`) and writes only dirty map leaves; this is the device's half.
// =================================================================================================

/// The read-ahead window for `handle`, in sectors: the SD card's multi-block bound, clamped to the block
/// layer's per-op cap; 0 (off) on every other handle.
fn ra_window(handle: block::BlockHandle) -> u64 {
    match handle {
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        block::BlockHandle::Sdhc => (crate::drivers::sdhc::MB_MAX_BLOCKS as u64).min(block::MAX_BLOCKS_PER_OP as u64),
        _ => 0,
    }
}

fn map_block_err(at: u64, e: BlockError) -> SectorError {
    match e {
        BlockError::BadLba => SectorError::OutOfBounds(at),
        e => {
            note_sector_busy(&e);
            SectorError::Io(format!("block layer: {e:?}"))
        }
    }
}

/// `buf.len() / 512` consecutive sectors from `lba`: one counted read per block-layer op on the SD card,
/// the per-sector loop elsewhere.
fn handle_read_multi(handle: block::BlockHandle, lba: u64, buf: &mut [u8]) -> Result<(), SectorError> {
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    if let block::BlockHandle::Sdhc = handle {
        let per = block::MAX_BLOCKS_PER_OP * 512;
        for (i, c) in buf.chunks_mut(per).enumerate() {
            let at = lba + (i * block::MAX_BLOCKS_PER_OP) as u64;
            block::read_blocks_sdhc(at, c).map_err(|e| map_block_err(at, e))?;
        }
        return Ok(());
    }
    for (i, c) in buf.chunks_mut(512).enumerate() {
        let at = lba + i as u64;
        match handle_read(handle, at, c) {
            Ok(512) => {}
            Ok(n) => return Err(SectorError::Io(format!("short sector read: {n} bytes"))),
            Err(e) => return Err(map_block_err(at, e)),
        }
    }
    Ok(())
}

/// The write twin of [`handle_read_multi`]: `write_blocks_sdhc` (one CMD25 under `sdw`) on the SD card.
fn handle_write_multi(handle: block::BlockHandle, lba: u64, buf: &[u8]) -> Result<(), SectorError> {
    if buf.len() % 512 != 0 {
        return Err(SectorError::Io(format!("write span {} not whole sectors", buf.len())));
    }
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    if let block::BlockHandle::Sdhc = handle {
        let per = block::MAX_BLOCKS_PER_OP * 512;
        for (i, c) in buf.chunks(per).enumerate() {
            let at = lba + (i * block::MAX_BLOCKS_PER_OP) as u64;
            block::write_blocks_sdhc(at, c).map_err(|e| map_block_err(at, e))?;
        }
        return Ok(());
    }
    for (i, c) in buf.chunks(512).enumerate() {
        let at = lba + i as u64;
        handle_write(handle, at, c).map_err(|e| map_block_err(at, e))?;
    }
    Ok(())
}

/// BOOT80: the read-ahead window the bound handle's device runs with (0 = none; the `tests boot80` line).
pub fn ra_window_bound() -> u64 {
    mount_bound_handle().map(ra_window).unwrap_or(0)
}

// ── STACKGUARD2 (rmbp-ledger B403): the mount a stack-overflowed transaction took with it ─────────────────

/// Force-release `MOUNT` and LEAK the bound instance — only from the overflow path, when `lockowner` names the
/// dead task its holder. Not dropped: `Drop` writes the in-RAM metadata back, and a transaction that died mid-way
/// leaves that metadata half-done. Copy-on-write: the medium is consistent at the last root flip, so the next
/// `with_unafs` re-binds from it.
pub fn lockowner_release() -> bool { let poisoned = MOUNT.mark_poison(); // POISON2 (B846) MOUNTPOISON: MARKED before the force-unlock, so a taker that wins the window below sees it (`true` = marked)
    unsafe { MOUNT.force_unlock() };
    crate::arch::without_interrupts(|| { #[cfg(all(target_arch = "x86_64", feature = "wc"))] { if !crate::video::renderwedge::wait_unlocked(&MOUNT, crate::video::renderwedge::W_UNAFS) { return ; } } // TRASHLOCK (B523): RENDERWEDGE's bounded pre-wait (B520 :793's shape); past the bound the leak is left to the next `with_unafs` (the lock was force-released above). ⚠ SAME-LINE fold: :2881's `MOUNT.lock()` keeps its line and column
        if let Some(m) = MOUNT.lock().take() {
            core::mem::forget(m);
        }
    });
    TXN_BUSY.store(false, Ordering::Relaxed); poisoned
}

// =================================================================================================
// VFSTXCOST (rmbp-ledger B522) — the read-ahead window becomes a SET of windows.
//
// SMALLFIX8 (B516) measured a FIXED 76–103 ms per JOBSCAN chunk (one `with_unafs` transaction:
// `resolve_path` + `ls`) whatever the record count. The cost is the device, not the walk: every
// `ls_via` step reads the directory's INODE block and then its DATA blocks (`unafs/src/fs.rs`
// `ls_via` -> `read_inode_via` + `read_data_via`), and `resolve_path` runs one `ls_via` per path
// component from the root — so `/jobs/ledger/<track>` touches root inode, root data, `/jobs` inode,
// `/jobs` data, `ledger` inode, `ledger` data, the target's inode and (non-empty) its data: 7–8
// distinct metadata blocks, scattered by copy-on-write. BOOT80's cache held ONE 64-sector window, so
// every one of those blocks was a miss and every miss a whole 32 KiB CMD18 on a 1-bit, 25 MHz PIO bus
// (`drivers/sdhc.rs` §"1-bit bus, no high-speed"): ~11 ms each — BOOT80's own line on flight 28 reads
// `mount_ms=120 mount_cmds=11`, 10.9 ms per window. 7 × 10.9 = 76 ms, 9 × 10.9 = 98 ms: the measured
// band. And nothing survived to the next transaction, so the NEXT chunk re-read the same root and
// `/jobs` blocks from the card again.
//
// The fix keeps sixteen windows (512 KiB at most, grown only as touched), most recent first; a hit in
// any of them is served from RAM and moves it to the front, a miss refills the least recent. The
// shared ancestors a listing walks (root, `/jobs`, `/jobs/ledger`) stay resident across transactions,
// so a chunk costs its own directory's blocks and nothing else.
//
// COHERENCE — the same rule as BOOT80's one window, unchanged: the set lives inside the ONE mount's
// device (two live mounts are already forbidden), every write the device issues is mirrored into
// every window it overlaps (`write_through`), a failed write drops all of them, a failed fetch caches
// nothing, and a remount (K9 discard, UNAFSTXN abort, `force_remount`) builds a fresh device. It is a
// mirror of the MEDIUM, so it is never a second writer and never older than the card; under K8a
// copy-on-write a committed block is never rewritten in place, so the only blocks whose bytes change
// under it are the ones this device itself writes (fresh CoW blocks, the 512 B root-slot flip) — and
// those pass through `write_through`. A generation key would invalidate strictly MORE (every commit,
// i.e. every bootlog flush) and cover nothing this does not.
// =================================================================================================

/// VFSTXCOST: windows kept per device.
const RA_SLOTS: usize = 16;

/// VFSTXCOST: medium fetches by the mount's device (a read-ahead window fill on the SD card; a whole
/// block read on a windowless handle). Read as deltas by `fs::vfs`'s `[vfs] tx` line.
static TX_FILLS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// VFSTXCOST: block reads served from a resident window without touching the medium.
static TX_HITS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// VFSTXCOST: `(fills, hits)` so far this boot — the `blocks=` / `hits=` of `[vfs] tx`.
pub fn tx_io() -> (u64, u64) {
    (TX_FILLS.load(Ordering::Relaxed), TX_HITS.load(Ordering::Relaxed))
}

/// VFSTXCOST: up to [`RA_SLOTS`] read-ahead windows, most recently used first. The API is
/// `ReadAheadCache`'s, so `SdSectorDevice`'s BOOT80 call sites are unchanged.
pub struct RaSet {
    window: u64,
    slots: alloc::vec::Vec<unafs::readahead::ReadAheadCache>, #[cfg(all(target_arch = "x86_64", feature = "wc"))] seen: u64, // UNAFSWRITE (B825): the `splitwrite::RA_GEN` these windows were filled under
}

impl RaSet {
    fn new(window: u64) -> Self {
        Self { window, slots: alloc::vec::Vec::new(), #[cfg(all(target_arch = "x86_64", feature = "wc"))] seen: 0 }
    }

    fn window_sectors(&self) -> u64 {
        self.window
    }

    fn invalidate(&mut self) {
        self.slots.iter_mut().for_each(|s| s.invalidate());
    }

    fn write_through(&mut self, lba: u64, data: &[u8]) {
        self.slots.iter_mut().for_each(|s| s.write_through(lba, data));
    }

    fn read<F>(&mut self, lba: u64, out: &mut [u8], dev_sectors: u64, mut fetch: F) -> Result<(), SectorError>
    where
        F: FnMut(u64, &mut [u8]) -> Result<(), SectorError>,
    {
        #[cfg(all(target_arch = "x86_64", feature = "wc"))] splitwrite::ra_sync(self); if let Some(i) = self.slots.iter().position(|s| s.lookup(lba, out)) { // UNAFSWRITE (B825): a split commit wrote blocks past this device — its windows drop first
            TX_HITS.fetch_add(1, Ordering::Relaxed);
            let s = self.slots.remove(i);
            self.slots.insert(0, s);
            return Ok(());
        }
        let mut s = if self.slots.len() >= RA_SLOTS {
            self.slots.pop().expect("RA_SLOTS > 0")
        } else {
            unafs::readahead::ReadAheadCache::new(self.window)
        };
        let r = s.read(lba, out, dev_sectors, |start, b| {
            TX_FILLS.fetch_add(1, Ordering::Relaxed);
            fetch(start, b)
        });
        self.slots.insert(0, s);
        r
    }
}

// =================================================================================================
// HIDSTALL2 (rmbp-ledger B670) — MOUNT guards the bound mount; a file's DATA is read outside it.
// TAIL-APPENDED (this file's panic Locations are line-pinned). The two hooks are same-line folds at
// :449 (`note_span`, in `mount_on`) and :807 (`note_bound`, the lazy bind in `with_unafs_attempt`),
// both under this module's cfg, so an image without `wc` is byte-identical.
//
// THE WIRE (flight 36 boot 3): `[vfs] tx what=read path=/apps/AETHER.ELF list_ms=1092 blocks=101`
// (f36-boot3.log:1945). `NativeBackend::read` (fs/vfs.rs:1813-1824) runs `resolve_path`, `read_inode`
// and `read_data` (3.2 MB: 101 32 KiB windows on the 1-bit SD) inside ONE `with_unafs` closure, i.e.
// MOUNT (:567) taken at :794 with IRQs masked (:793) for 1092 ms. LOCKREG: `held_max_ms=1091 …
// held_max_at=unafs.rs:567`; HIDSTALL: `masked_max_ms=1092` (the attempt's masked span, :900). The
// waiter the capture names is the render task (`spin-wait name=unafs.rs:567 at=unafs.rs:639
// waiter=9@cpu7`, f36-boot3.log:219).
//
// THE SPLIT. A committed block is never rewritten in place (K8a, this file's header): the bytes of a
// committed file change only after a commit that unreferences them flips the root (generation + 1),
// and the inode's extents name every byte a read needs. So:
//   1. PLAN under MOUNT: resolve, read the inode, copy its extents; note the root generation and the
//      bound mount's identity (epoch, handle, partition span).
//   2. READ the data blocks with MOUNT released, through the block-layer calls the mount's own device
//      makes (`handle_read`, `read_blocks_sdhc` on the card), unmasked unless the caller is masked.
//   3. CHECK under MOUNT that the generation and the epoch are unchanged. Then no commit ran and no
//      other volume was bound, the extents still name those bytes, and the result is what the one
//      long hold would have returned.
// A changed generation or epoch, an unaligned extent, a block past the volume or any device error
// (a `Busy` loan included) runs the read again the old way, in one hold (`fallbacks=` counts these).
// Writes stay whole transactions under MOUNT: the allocator and the refcount map sit behind the
// crate's `&mut self`, and K5B's persisters serialise on this lock.
// =================================================================================================
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub mod splitread {
    use super::*;
    use core::sync::atomic::AtomicU64;

    /// The bound mount's identity, as the split read plans against it.
    #[derive(Clone, Copy)]
    struct Bound {
        epoch: u64,
        handle: block::BlockHandle,
        span: PartitionSpan,
    }

    /// The span `mount_on` located per handle (a property of the medium, not of one mount).
    static SPANS: Mutex<Vec<(block::BlockHandle, PartitionSpan)>> = Mutex::new(Vec::new());
    /// The live bound mount's identity; written only inside the MOUNT hold (the :807 fold).
    static BOUND: Mutex<Option<Bound>> = Mutex::new(None);
    static EPOCH: AtomicU64 = AtomicU64::new(0); pub(super) fn bound_now() -> Option<(block::BlockHandle, PartitionSpan)> { BOUND.lock().as_ref().map(|b| (b.handle, b.span)) } // UNAFSWRITE (B825): the bound mount's medium, for the split commit's write phase (LINE-NEUTRAL)

    /// Split reads this boot, the longest MOUNT hold one of them took (us), the medium fetches made
    /// under MOUNT by a fallback, and the fallbacks.
    static OPS: AtomicU64 = AtomicU64::new(0);
    static HELD_MAX_US: AtomicU64 = AtomicU64::new(0);
    static IO_UNDER_LOCK: AtomicU64 = AtomicU64::new(0);
    static FALLBACKS: AtomicU64 = AtomicU64::new(0);

    /// One block-layer op per this many 4 KiB blocks of a contiguous run (64 KiB).
    const RUN_BLOCKS: u64 = 16;
    /// A split read slower than this prints its `[unafslock]` line.
    const LINE_MS: u64 = 100;

    /// :449 fold — `mount_on` located `span` on `handle`.
    pub fn note_span(handle: block::BlockHandle, span: &PartitionSpan) {
        crate::arch::without_interrupts(|| {
            let mut t = SPANS.lock();
            match t.iter_mut().find(|(h, _)| *h == handle) {
                Some(e) => e.1 = *span,
                None => t.push((handle, *span)),
            }
        });
    }

    /// :807 fold — the lazy bind just bound a mount on `handle` (inside the masked MOUNT hold).
    pub fn note_bound(handle: block::BlockHandle) {
        let span = SPANS.lock().iter().find(|(h, _)| *h == handle).map(|e| e.1);
        let epoch = EPOCH.fetch_add(1, Ordering::Relaxed) + 1;
        *BOUND.lock() = span.map(|span| Bound { epoch, handle, span });
    }

    /// Why a split read returned no bytes (the caller maps these to its own error type).
    #[derive(Debug)]
    pub enum ReadMiss {
        NoSuchPath,
        IsADirectory,
        Failed,
        Mount(MountError),
    }

    struct Plan {
        root_gen: u64,
        bound: Bound,
        chunks: ::unafs::ExtentList,
        size: u64,
    }

    /// Read `len` bytes at `offset` of the file at absolute unafs `path`: the plan and the check hold
    /// MOUNT, the data blocks are read without it. Same bytes and same refusals as the one-hold
    /// `resolve_path` + `read_inode` + `read_data` it replaces.
    pub fn read(path: &str, offset: u64, len: u64) -> Result<Vec<u8>, ReadMiss> {
        OPS.fetch_add(1, Ordering::Relaxed);
        let t0 = crate::hidstall::now_us();
        let (plan, held_plan) = with_unafs(|fs| {
            let h0 = crate::hidstall::now_us();
            let r = (|| -> Result<Option<Plan>, ReadMiss> {
                let id = fs.resolve_path(path).map_err(|_| ReadMiss::NoSuchPath)?;
                let ino = fs.read_inode(id).map_err(|_| ReadMiss::NoSuchPath)?;
                if ino.kind == ::unafs::FileKind::Directory {
                    return Err(ReadMiss::IsADirectory);
                }
                if fs.key_slot().is_some_and(|k| k.cipher == ::unafs::keyslot::CIPHER_AES256_XTS) { return Ok(None); } let root_gen = fs.root_generation(); // crawl B 1: a CIPHERED root's raw sectors are XTS ciphertext — the one-hold read goes through the mount's VolumeCipher (LINE-NEUTRAL fold, code first)
                Ok(BOUND.lock().as_ref().copied().map(|bound| Plan { root_gen, bound, chunks: ino.chunks, size: ino.size }))
            })();
            (r, crate::hidstall::now_us().saturating_sub(h0))
        })
        .map_err(ReadMiss::Mount)?;
        HELD_MAX_US.fetch_max(held_plan, Ordering::Relaxed);
        let Some(plan) = plan? else {
            return locked(path, offset, len); // no bound identity noted: the one-hold read
        };
        let t1 = crate::hidstall::now_us();
        let data = read_extents(&plan, offset, len);
        let t2 = crate::hidstall::now_us();
        let (same, held_check) = with_unafs(|fs| {
            let h0 = crate::hidstall::now_us();
            let same = fs.root_generation() == plan.root_gen && BOUND.lock().as_ref().map(|b| b.epoch) == Some(plan.bound.epoch);
            (same, crate::hidstall::now_us().saturating_sub(h0))
        })
        .map_err(ReadMiss::Mount)?;
        HELD_MAX_US.fetch_max(held_check, Ordering::Relaxed);
        let (out, how) = match data {
            Some(d) if same => (Ok(d), "split"),
            _ => (locked(path, offset, len), "fallback"),
        };
        let t3 = crate::hidstall::now_us();
        if t3.saturating_sub(t0) >= LINE_MS * 1000 {
            serial_println!(
                "[unafslock] read path={} bytes={} held_us={} data_ms={} total_ms={} -> {}",
                path,
                out.as_ref().map(|d| d.len()).unwrap_or(0),
                held_plan.max(held_check),
                t2.saturating_sub(t1) / 1000,
                t3.saturating_sub(t0) / 1000,
                how
            );
        }
        out
    }

    /// The pre-split read: one `with_unafs` hold, data I/O under MOUNT. Counted.
    fn locked(path: &str, offset: u64, len: u64) -> Result<Vec<u8>, ReadMiss> {
        FALLBACKS.fetch_add(1, Ordering::Relaxed);
        let (f0, _) = tx_io();
        let r = with_unafs(|fs| -> Result<Vec<u8>, ReadMiss> {
            let id = fs.resolve_path(path).map_err(|_| ReadMiss::NoSuchPath)?;
            let ino = fs.read_inode(id).map_err(|_| ReadMiss::NoSuchPath)?;
            if ino.kind == ::unafs::FileKind::Directory {
                return Err(ReadMiss::IsADirectory);
            }
            fs.read_data(id, offset, len).map_err(|_| ReadMiss::Failed)
        });
        IO_UNDER_LOCK.fetch_add(tx_io().0.wrapping_sub(f0), Ordering::Relaxed);
        r.map_err(ReadMiss::Mount)?
    }

    /// The crate's extent walk (`unafs::fs::read_from_extents_via`), step for step, with the block
    /// fetches batched into contiguous runs and issued with MOUNT released. `None` = take the one-hold
    /// path (it reproduces the crate's own refusal, if there is one).
    fn read_extents(p: &Plan, offset: u64, len: u64) -> Option<Vec<u8>> {
        let bs = ::unafs::BLOCK_SIZE;
        if p.size > p.bound.span.block_count.saturating_mul(bs) {
            return None;
        }
        let total = len.min(p.size.saturating_sub(offset));
        let end = offset.checked_add(total)?;
        let mut out = Vec::new();
        out.try_reserve_exact(usize::try_from(total).ok()?).ok()?;
        let mut buf = alloc::vec![0u8; (RUN_BLOCKS * bs) as usize];
        let mut cur = offset;
        while cur < end {
            let mut hit = None;
            let mut next = u64::MAX;
            for e in &p.chunks {
                let e_end = e.logical_offset.checked_add(e.length)?;
                if cur >= e.logical_offset && cur < e_end {
                    hit = Some((e, e_end));
                    break;
                }
                if e.logical_offset > cur && e.logical_offset < next {
                    next = e.logical_offset;
                }
            }
            let Some((e, e_end)) = hit else {
                let gap = (end - cur).min(next.saturating_sub(cur));
                out.resize(out.len() + gap as usize, 0);
                cur += gap;
                continue;
            };
            if e.logical_offset % bs != 0 {
                return None;
            }
            let first = e.physical_block.checked_add((cur - e.logical_offset) / bs)?;
            // The run: blocks first.. while the walk stays in this extent and under `end`.
            let mut n = 0u64;
            let mut c = cur;
            while c < end && c < e_end && n < RUN_BLOCKS {
                c = (c / bs + 1) * bs;
                n += 1;
            }
            if first.checked_add(n)? > p.bound.span.block_count {
                return None;
            }
            let lba = p.bound.span.base_lba.checked_add(first.checked_mul(8)?)?;
            let run = &mut buf[..(n * bs) as usize];
            if !read_run(p.bound.handle, lba, run) {
                return None;
            }
            for i in 0..n {
                let off = (cur % bs) as usize;
                let take = (bs as usize - off).min((end - cur) as usize);
                let at = (i * bs) as usize + off;
                out.extend_from_slice(&run[at..at + take]);
                cur += take as u64;
            }
        }
        Some(out)
    }

    /// `buf.len() / 512` sectors from `lba` on `handle`, the device's own calls, no `TXN_BUSY` latch
    /// (no transaction is open: a `Busy` here only sends this read to the one-hold path).
    fn read_run(handle: block::BlockHandle, lba: u64, buf: &mut [u8]) -> bool {
        #[cfg(feature = "sdhcblk")]
        if let block::BlockHandle::Sdhc = handle {
            let per = block::MAX_BLOCKS_PER_OP * 512;
            for (i, c) in buf.chunks_mut(per).enumerate() {
                if block::read_blocks_sdhc(lba + (i * block::MAX_BLOCKS_PER_OP) as u64, c).is_err() {
                    return false;
                }
            }
            return true;
        }
        buf.chunks_mut(512).enumerate().all(|(i, c)| matches!(handle_read(handle, lba + i as u64, c), Ok(512)))
    }

    /// `(ops, held_max_us, io_under_lock, fallbacks)` this boot.
    pub fn census() -> (u64, u64, u64, u64) {
        (
            OPS.load(Ordering::Relaxed),
            HELD_MAX_US.load(Ordering::Relaxed),
            IO_UNDER_LOCK.load(Ordering::Relaxed),
            FALLBACKS.load(Ordering::Relaxed),
        )
    }

    // ── `tests unafslock`: a ≤ 4 MiB split read on a worker core against a metadata loop here ────────

    static T_PATH: Mutex<String> = Mutex::new(String::new());
    static T_LEN: AtomicU64 = AtomicU64::new(0);
    static T_DONE: AtomicBool = AtomicBool::new(false);
    static T_OK: AtomicBool = AtomicBool::new(false);
    static T_HASH: AtomicU64 = AtomicU64::new(0);
    static T_BYTES: AtomicU64 = AtomicU64::new(0);
    static T_MS: AtomicU64 = AtomicU64::new(0);

    fn fnv(h: u64, d: &[u8]) -> u64 {
        d.iter().fold(h, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
    }
    const FNV0: u64 = 0xcbf2_9ce4_8422_2325;

    fn worker(_: usize) {
        let path = crate::arch::without_interrupts(|| T_PATH.lock().clone());
        let t0 = crate::hidstall::now_us();
        let r = read(&path, 0, T_LEN.load(Ordering::Relaxed));
        T_MS.store(crate::hidstall::now_us().saturating_sub(t0) / 1000, Ordering::Relaxed);
        if let Ok(d) = r {
            T_BYTES.store(d.len() as u64, Ordering::Relaxed);
            T_HASH.store(fnv(FNV0, &d), Ordering::Relaxed);
            T_OK.store(true, Ordering::Relaxed);
        }
        T_DONE.store(true, Ordering::Release);
    }

    /// `tests unafslock` (HIDSTALL2, B670): the largest file in `/apps`, up to 4 MiB, read split on a
    /// worker core while this task runs metadata ops (`resolve_path` + `read_inode`) on the same mount.
    /// PASS = every MOUNT hold of both ≤ 5 ms, no fetch under MOUNT by the read, at least one
    /// metadata op completed while the read ran, and the bytes equal a reference read taken first.
    pub fn selftest() { super::splitwrite::write_leg(); super::bind_leg(); // HEAPREGION (B854) BINDSTACK: the sized bind task's leg (file tail). UNAFSWRITE (B825): the write leg runs first and prints its own line
        const CAP: u64 = 4 << 20;
        let pick = with_unafs(|fs| -> Option<(String, u64)> {
            let dir = fs.resolve_path("/apps").ok()?;
            let mut best: Option<(String, u64)> = None;
            for e in fs.ls(dir).ok()? {
                if e.kind != ::unafs::FileKind::File {
                    continue;
                }
                if let Ok(i) = fs.read_inode(e.inode_id) {
                    if best.as_ref().map_or(true, |b| i.size > b.1) {
                        best = Some((e.name, i.size));
                    }
                }
            }
            best
        });
        let Ok(Some((name, size))) = pick else {
            serial_println!(":: UNAFSLOCK: SKIP (no native unafs volume, or no file in /apps) ::");
            return;
        };
        let path = format!("/apps/{}", name);
        let len = size.min(CAP);
        // The reference: the same bytes in 32 KiB holds (also warms the metadata windows the plan walks).
        let mut want = FNV0;
        let mut at = 0u64;
        while at < len {
            let n = (len - at).min(32 << 10);
            let p = path.as_str();
            match with_unafs(|fs| fs.resolve_path(p).and_then(|id| fs.read_data(id, at, n))) {
                Ok(Ok(d)) if d.len() as u64 == n => want = fnv(want, &d),
                _ => {
                    serial_println!(":: UNAFSLOCK: ops=0 held_max_ms=0 io_under_lock=0 -> FAIL :: why=reference-read path={} at={}", path, at);
                    return;
                }
            }
            at += n;
        }
        let (_, _, io0, fb0) = census();
        HELD_MAX_US.store(0, Ordering::Relaxed);
        crate::arch::without_interrupts(|| *T_PATH.lock() = path.clone());
        T_LEN.store(len, Ordering::Relaxed);
        T_OK.store(false, Ordering::Relaxed);
        T_DONE.store(false, Ordering::Release);
        let here = crate::arch::percpu::this_cpu().cpu_index as usize;
        let cpu = crate::arch::smp::worker_cpu(0).unwrap_or(here);
        crate::arch::sched::spawn("unafslock-read", worker, 0, cpu, crate::arch::sched::PRIO_NORMAL);
        let (mut ops, mut meta_held_max, mut meta_wait_max) = (0u64, 0u64, 0u64);
        let t0 = crate::arch::ms();
        while !T_DONE.load(Ordering::Acquire) && crate::arch::ms().saturating_sub(t0) < 30_000 {
            let s = crate::hidstall::now_us();
            let h = with_unafs(|fs| {
                let h0 = crate::hidstall::now_us();
                let _ = fs.resolve_path("/apps").and_then(|d| fs.read_inode(d));
                crate::hidstall::now_us().saturating_sub(h0)
            });
            let total = crate::hidstall::now_us().saturating_sub(s);
            if let Ok(h) = h {
                if !T_DONE.load(Ordering::Acquire) {
                    ops += 1;
                }
                meta_held_max = meta_held_max.max(h);
                meta_wait_max = meta_wait_max.max(total.saturating_sub(h));
            }
            let until = crate::hidstall::now_us() + 2000;
            while crate::hidstall::now_us() < until {
                if crate::arch::irqs_masked() { core::hint::spin_loop() } else { crate::hlt() }
            }
        }
        let done = T_DONE.load(Ordering::Acquire);
        let (_, read_held, io1, fb1) = census();
        let held_us = read_held.max(meta_held_max);
        let io_under = io1.wrapping_sub(io0);
        let fallbacks = fb1.wrapping_sub(fb0);
        let same = T_OK.load(Ordering::Relaxed) && T_BYTES.load(Ordering::Relaxed) == len && T_HASH.load(Ordering::Relaxed) == want;
        let pass = done && same && held_us <= 5000 && io_under == 0 && fallbacks == 0 && ops >= 1;
        serial_println!(
            ":: UNAFSLOCK: ops={} held_max_ms={} io_under_lock={} -> {} :: path={} bytes={} read_ms={} read_held_us={} meta_held_us={} meta_wait_max_ms={} fallbacks={} match={} done={}",
            ops,
            held_us.div_ceil(1000),
            io_under,
            if pass { "PASS" } else { "FAIL" },
            path,
            len,
            T_MS.load(Ordering::Relaxed),
            read_held,
            meta_held_max,
            meta_wait_max / 1000,
            fallbacks,
            if same { "yes" } else { "no" },
            if done { "yes" } else { "no" }
        );
    }
}

// =================================================================================================
// KEYBOOT (rmbp-ledger B688) — the PRE-BOOT UNLOCK of a keyed root. TAIL-APPENDED: this module's panic
// Locations are line-pinned (see `partition_witness`'s note), and nothing above changes. `mount_on` (:452)
// already refuses a keyed volume with `Fs(KeyRequired)` through `UnaFS::mount` (the crate's gate reads block 0
// and stops before `load_committed`), so every lazy bind of a locked root refuses and binds nothing.
//
// [`keyboot_probe`] names the ROOT CANDIDATE — the first handle, in the bind's own order (the AHCI root grant,
// then `Global`, then the admitted probes), whose partition table carries a UnaFS volume — and says whether its
// block 0 carries a key slot (§R2a), reading two sectors and mounting nothing. [`keyboot_unwrap`] + [`keyboot_bind`] (KEYBOOT2) open THAT volume
// with the typed secret — `KeySlot::unwrap` (the Argon2id, UNMASKED, on the login worker), then, on the render task,
// `UnaFS::mount_with_key` — and only then takes the masked `MOUNT` hold to install it as the bound mount. Building the
// mount outside the hold is sound for exactly this volume: while it is keyed and locked every other bind stops at
// block 0, so no second live mount of it exists (K4). The unwrapped data key stays with the open root (below).
//
// THE REBIND (seat, wave 36). A `force_remount`, a K9 discard or a contention restart drops the bound mount and the
// next `with_unafs` rebinds through `mount_on` (:452), which for THE unlocked root hands the crate the unwrapped data
// key (`UnaFS::mount_with_key`, [`keyboot_mount`]) instead of refusing `KeyRequired`. That key ([`KB_KEY`]) is the
// open root's: it exists only from the unlock while the root is open, in RAM, wiped on drop — never in a store, on
// a disk or on the wire (BP-security-11 as security.md now states it; M2's cipher needs the same key in the mount).
// `bootdisk::survey` does not cache an answer while [`root_locked`] (the hold) is true.
// =================================================================================================

/// KEYBOOT: the root candidate [`keyboot_probe`] named (the volume [`keyboot_unwrap`] opens).
static KB_HANDLE: Mutex<Option<block::BlockHandle>> = Mutex::new(None);
/// KEYBOOT: the probe saw a key slot on the root candidate.
static KB_KEYED: AtomicBool = AtomicBool::new(false);
/// KEYBOOT: the keyed root was opened with its key this boot.
static KB_UNLOCKED: AtomicBool = AtomicBool::new(false);
/// KEYBOOT: the OPEN root's unwrapped data key (wiped on drop) — what a rebind of that root opens it with.
static KB_KEY: Mutex<Option<::unafs::VolumeKey>> = Mutex::new(None);

/// KEYBOOT: `mount_on`'s open — with the open root's key for THE unlocked root's handle, `UnaFS::mount` otherwise.
fn keyboot_mount(handle: block::BlockHandle, adapter: BlockAdapter<SdSectorDevice>) -> Result<KernelUnaFS, FileSystemError> {
    if *KB_HANDLE.lock() == Some(handle) {
        if let Some(k) = KB_KEY.lock().as_ref() {
            return UnaFS::mount_with_key(adapter, k);
        }
    }
    UnaFS::mount(adapter)
}

/// KEYBOOT: every handle the bind may take the root from, in the bind's order ([`bind_mount`]).
fn keyboot_candidates() -> Vec<block::BlockHandle> {
    let mut v = Vec::new();
    #[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
    if let Some(port) = crate::install::ahciroot::ensure_root_grant() {
        v.push(block::BlockHandle::Ahci { port });
    }
    for h in bind_probe_candidates() {
        if h == block::BlockHandle::Global || bind_probe_admitted(h) {
            v.push(h);
        }
    }
    v
}

/// KEYBOOT: the root candidate and whether it is keyed — `Some((handle, true))` keyed, `Some((handle, false))`
/// unkeyed, `None` when no handle carries a UnaFS volume. A slot this build cannot read counts as keyed (the mount
/// refuses it too). Reads the partition table and sectors 0..2 of the volume; mounts nothing.
pub fn keyboot_probe() -> Option<(block::BlockHandle, bool)> {
    for h in keyboot_candidates() {
        let Ok(span) = locate_on(h) else {
            continue;
        };
        let mut b = [0u8; 1024];
        let read = handle_read(h, span.base_lba, &mut b[..512]).is_ok() && handle_read(h, span.base_lba + 1, &mut b[512..]).is_ok();
        let keyed = read && !matches!(::unafs::KeySlot::parse(&b), Ok(None));
        kb_masked(|| *KB_HANDLE.lock() = Some(h)); // crawl B 7: IRQ-masked, as `keyboot_mount` takes it inside the masked MOUNT hold (a holder preempted here would deadlock it)
        KB_KEYED.store(keyed, Ordering::Release);
        return Some((h, keyed));
    }
    None
}

// KEYBOOT2 (rmbp-ledger B810) — the unlock SPLIT IN PLACE (it was `unlock_root`, one fn on the login worker). FLIGHT 37
// §6: Enter on the metal form hung the machine twice. The boot's FIRST mount (`locate_on`, the first
// `partition_witness`, `mount_with_key`) ran on the 16 KiB `login-submit` stack (sched.rs `TASK_STACK_SIZE`), where an
// overflow inside a masked driver hold leaves CARD or the heap lock held (stackguard.rs: the task halts, its locks stay),
// and the bind's MOUNT take had no bound off the render task (RENDERWEDGE's wait is a no-op there). Now TWO halves:
// [`keyboot_unwrap`] — the WORKER half: the slot read off the medium through the block layer's normal path (no masked
// hold of ours), then the ONE Argon2id with interrupts live; the key parked in [`KB_PENDING`] (wiped on drop);
// [`keyboot_bind`] — the RENDER half (`login::kb_apply`): the mount built outside any hold of ours on the render stack,
// where every bind ran before, then RENDERWEDGE's bounded wait and a short masked MOUNT install. Every masked span the
// two halves take goes through [`kb_masked`] (`masked_max_us=`). `tests keyboot` lends a RAM volume
// ([`keyboot_lend_ram`]) and drives the same two halves through the form's own dispatch (`login::kb_leg_submit`).

/// KEYBOOT2: what the unlock opens — the probed root, or a RAM volume a fixture lent ([`keyboot_lend_ram`]).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KbTarget {
    Root(block::BlockHandle),
    Ram,
}

/// KEYBOOT2: one unlock's measures — the `[keyboot] unlock …` line and the fixture's.
#[derive(Clone, Copy)]
pub struct KbReport {
    pub target: &'static str,
    pub via: &'static str,
    pub read_ms: u64,
    pub kdf_ms: u64,
    pub bind_ms: u64,
    pub masked_max_us: u64,
    pub kdf_if: &'static str,
    /// `unwrapped` / `mac` / `words` / `error` from the worker half, then `ok` / `busy` / `disk` from the bind.
    pub outcome: &'static str,
}

const KB_REPORT0: KbReport = KbReport { target: "-", via: "-", read_ms: 0, kdf_ms: 0, bind_ms: 0, masked_max_us: 0, kdf_if: "na", outcome: "-" };
/// KEYBOOT2: the key the worker unwrapped, until the render task binds with it (or drops it: wiped).
static KB_PENDING: Mutex<Option<(KbTarget, ::unafs::VolumeKey)>> = Mutex::new(None);
/// KEYBOOT2: the last unlock's measures.
static KB_REPORT: Mutex<KbReport> = Mutex::new(KB_REPORT0);
/// KEYBOOT2: a RAM volume `tests keyboot` lent as the unlock target (never set on a real boot).
static KB_RAM: Mutex<Option<Vec<u8>>> = Mutex::new(None);
/// KEYBOOT2: where the lent RAM volume's bind lands (the fixture takes it back).
static KB_RAM_MOUNT: Mutex<Option<UnaFS<::unafs::MemDevice>>> = Mutex::new(None);
/// KEYBOOT2: the longest masked span the unlock's two halves took (µs), since the last unwrap began.
static KB_MASKED_MAX_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// KEYBOOT2: `without_interrupts` for the unlock path, its span folded into [`KB_MASKED_MAX_US`].
fn kb_masked<R>(f: impl FnOnce() -> R) -> R {
    let t0 = crate::hidstall::now_us();
    let r = crate::arch::without_interrupts(f);
    KB_MASKED_MAX_US.fetch_max(crate::hidstall::now_us().saturating_sub(t0), Ordering::Relaxed);
    r
}

/// KEYBOOT2: interrupts live at this instant (`1`/`0`; `na` where the arch has no cheap read).
fn kb_if_word() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        if x86_64::instructions::interrupts::are_enabled() { "1" } else { "0" }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        "na"
    }
}

fn kb_report(f: impl FnOnce(&mut KbReport)) {
    kb_masked(|| f(&mut *KB_REPORT.lock()));
}

/// KEYBOOT2: the last unlock's measures (the wire line's and the fixture's).
pub fn keyboot_report() -> KbReport {
    let mut r = kb_masked(|| *KB_REPORT.lock());
    r.masked_max_us = KB_MASKED_MAX_US.load(Ordering::Relaxed);
    r
}

/// KEYBOOT2 — the WORKER half (`login::kb_compute`, on `login-submit`): read the target's key slot (the root's two
/// sectors through `handle_read`, before the KDF; the lent RAM volume's first 1 KiB), run the ONE Argon2id (or the
/// words' HKDF) with interrupts live, and park the unwrapped key for [`keyboot_bind`]. `Fs(KeySlot(KeyRefused))` for a
/// wrong key (the slot's MAC), `Fs(KeySlot(BadRecovery))` for words that fail their checksum.
pub fn keyboot_unwrap(u: &::unafs::Unlock<'_>) -> Result<(), MountError> {
    KB_MASKED_MAX_US.store(0, Ordering::Relaxed);
    kb_report(|r| *r = KB_REPORT0);
    let stale = kb_masked(|| KB_PENDING.lock().take());
    drop(stale); // a key a released submit never bound: wiped
    let t = crate::arch::ms();
    let ram = kb_masked(|| {
        KB_RAM.lock().as_ref().map(|b| {
            let mut s = [0u8; 1024];
            let n = b.len().min(1024);
            s[..n].copy_from_slice(&b[..n]);
            s
        })
    });
    let (target, b0) = match ram {
        Some(s) => (KbTarget::Ram, s),
        None => {
            let handle = kb_masked(|| *KB_HANDLE.lock()).ok_or(MountError::NoVolume)?;
            let span = locate_on(handle)?;
            let mut b = [0u8; 1024];
            if handle_read(handle, span.base_lba, &mut b[..512]).is_err() || handle_read(handle, span.base_lba + 1, &mut b[512..]).is_err() {
                return Err(MountError::NoVolume);
            }
            (KbTarget::Root(handle), b)
        }
    };
    let read_ms = crate::arch::ms().saturating_sub(t);
    let via = match u {
        ::unafs::Unlock::Password(_) => "password",
        ::unafs::Unlock::Recovery(_) => "words",
    };
    let tw = match target {
        KbTarget::Root(h) => handle_kind_name(h),
        KbTarget::Ram => "ram",
    };
    kb_report(|r| {
        r.target = tw;
        r.via = via;
        r.read_ms = read_ms;
    });
    let slot = ::unafs::KeySlot::parse(&b0).map_err(|e| MountError::Fs(FileSystemError::KeySlot(e)))?.ok_or(MountError::NoVolume)?;
    let kdf_if = kb_if_word();
    let t = crate::arch::ms();
    let key = slot.unwrap(u);
    let kdf_ms = crate::arch::ms().saturating_sub(t);
    let outcome = match &key {
        Ok(_) => "unwrapped",
        Err(::unafs::KeySlotError::KeyRefused) => "mac",
        Err(::unafs::KeySlotError::BadRecovery) => "words",
        Err(_) => "error",
    };
    kb_report(|r| {
        r.kdf_ms = kdf_ms;
        r.kdf_if = kdf_if;
        r.outcome = outcome;
    });
    let key = key.map_err(|e| MountError::Fs(FileSystemError::KeySlot(e)))?;
    kb_masked(move || *KB_PENDING.lock() = Some((target, key)));
    Ok(())
}

/// KEYBOOT2 — the RENDER half (`login::kb_apply`): bind the target with the parked key. The root: the mount is built
/// first (its sector reads outside any hold of ours), then RENDERWEDGE's bounded wait and ONE short masked MOUNT
/// install. `Ok(true)` bound now, `Ok(false)` already bound (moot), `Busy` when MOUNT stayed held past the bound or
/// ANOTHER volume holds it (the key goes, wiped; the hold stays).
pub fn keyboot_bind() -> Result<bool, MountError> {
    let t = crate::arch::ms();
    let Some((target, key)) = kb_masked(|| KB_PENDING.lock().take()) else {
        return Err(MountError::NoVolume);
    };
    let r = match target {
        KbTarget::Ram => kb_bind_ram(key),
        KbTarget::Root(h) => kb_bind_root(h, key),
    };
    let bind_ms = crate::arch::ms().saturating_sub(t);
    let outcome = match &r {
        Ok(_) => "ok",
        Err(MountError::Busy) => "busy",
        Err(_) => "disk",
    };
    kb_report(|rep| {
        rep.bind_ms = bind_ms;
        rep.outcome = outcome;
    });
    r
}

/// KEYBOOT2: the root's bind (the bind half of the former `unlock_root`, moved, not copied).
fn kb_bind_root(handle: block::BlockHandle, key: ::unafs::VolumeKey) -> Result<bool, MountError> {
    install_warn_hook();
    crate::fs::unafstime::install_clock_hook();
    let span = locate_on(handle)?;
    partition_witness(handle, &span);
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    splitread::note_span(handle, &span);
    let dev = SdSectorDevice::open_on(handle)?;
    let mut fs = Some(UnaFS::mount_with_key(BlockAdapter::for_partition(dev, &span), &key).map(versions_armed).map_err(|e| { if let FileSystemError::Incompatible(bits) = &e { serial_println!("[unafs] mount refused incompat={:#x}", bits); } MountError::Fs(e) })?); // ROOTINCOMPAT (SR159): the keyed mount says the refusal too
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    if !crate::video::renderwedge::wait_unlocked(&MOUNT, crate::video::renderwedge::W_UNAFS) {
        return Err(MountError::Busy); // RENDERWEDGE's bounded pre-wait: this half runs on the render task, where it is armed
    }
    let prev = kb_masked(move || KB_KEY.lock().replace(key)); // crawl B 8 + 7: the open root's key parked BEFORE the bind (a rebind right after it opens WITH the key, `keyboot_mount`)
    let bound = kb_masked(|| {
        let mut g = MOUNT.lock();
        if let Some(b) = g.as_ref() {
            return if b.handle == handle { Some(false) } else { None }; // crawl B 4: this root already bound = moot; ANOTHER volume bound = refused below
        }
        if let Some(m) = fs.take() {
            #[cfg(all(target_arch = "x86_64", feature = "wc"))]
            splitread::note_bound(handle);
            *g = Some(BoundMount { handle, fs: m });
        }
        Some(true)
    });
    drop(fs);
    let Some(bound) = bound else {
        let k = kb_masked(|| core::mem::replace(&mut *KB_KEY.lock(), prev));
        drop(k); // crawl B 4: another volume holds MOUNT — the key goes (wiped, BP-security-11); the boot never releases onto the wrong root
        return Err(MountError::Busy);
    };
    drop(prev);
    KB_UNLOCKED.store(true, Ordering::Release);
    serial_println!(
        "[keyboot] root unlocked handle={} via={} bound={} (the data key lives only with the open root, in RAM, wiped on drop)",
        handle_kind_name(handle),
        keyboot_report().via,
        if bound { "now" } else { "already" }
    );
    Ok(bound)
}

/// KEYBOOT2: the lent RAM volume's bind — the same key, `UnaFS::mount_with_key` on its bytes, the mount kept for the
/// fixture ([`keyboot_ram_take`]).
fn kb_bind_ram(key: ::unafs::VolumeKey) -> Result<bool, MountError> {
    let Some(bytes) = kb_masked(|| KB_RAM.lock().take()) else {
        return Err(MountError::NoVolume);
    };
    let m = UnaFS::mount_with_key(::unafs::MemDevice::from_bytes(bytes), &key).map_err(MountError::Fs)?;
    let old = kb_masked(move || KB_RAM_MOUNT.lock().replace(m));
    drop(old);
    Ok(true)
}

/// KEYBOOT2: drop a parked key no bind will take (INPUTSTALL's 15 s release of a submit): wiped.
pub fn keyboot_pending_drop() {
    let k = kb_masked(|| KB_PENDING.lock().take());
    drop(k);
}

/// KEYBOOT2 (`tests keyboot`): lend a keyed RAM volume as the unlock target. `false` (nothing lent) while the root's
/// hold is open or a volume is already lent.
pub fn keyboot_lend_ram(bytes: Vec<u8>) -> bool {
    if root_locked() {
        return false;
    }
    let refused = kb_masked(move || {
        let mut g = KB_RAM.lock();
        if g.is_some() {
            return Some(bytes);
        }
        *g = Some(bytes);
        None
    });
    let ok = refused.is_none();
    drop(refused);
    ok
}

/// KEYBOOT2 (`tests keyboot`): take back the lent volume — its bound mount, if the unlock bound it — and end the lend.
pub fn keyboot_ram_take() -> Option<UnaFS<::unafs::MemDevice>> {
    let (m, b) = kb_masked(|| (KB_RAM_MOUNT.lock().take(), KB_RAM.lock().take()));
    drop(b);
    m
}

/// KEYBOOT: the keyed root was opened with its key this boot.
pub fn root_unlocked() -> bool {
    KB_UNLOCKED.load(Ordering::Acquire)
}

/// KEYBOOT: the root candidate is keyed and not opened yet — a bind now would refuse it (`KeyRequired`).
pub fn root_locked() -> bool {
    KB_KEYED.load(Ordering::Acquire) && !KB_UNLOCKED.load(Ordering::Acquire)
}

/// DISKFULL (rmbp-ledger B699; BP-storage-21, BP-bar-26; research B §6: a full disk must never wedge the OS or lose the
/// file being written) — the ONE place a UnaFS write that found the volume full becomes `-ENOSPC`.
///
/// - **The mapping.** [`write_or_full`] is the only caller of `UnaFS::write_data` on the VFS write path: the crate's
///   `FileSystemError::NoSpace` (`unaos/libs/fs/unafs/src/fs.rs:94`, raised by `alloc_block` at `fs.rs:1104` and the
///   span check at `fs.rs:1540-1541`) becomes `VfsError::Backend(NOSPACE)`, which `attrsys::errno_of` — the one VFS
///   refusal -> errno table every path syscall reads (`SYS_PATH_WRITE`'s `path_write_as`) — renders `-ENOSPC`. Every
///   other write failure keeps its old spelling (`unafs-write` -> `-EIO`). Never a panic: no unwrap on the path.
/// - **The rollback, read in the crate.** A failed `write_data` (`fs.rs:1594`) returns `write_data_inner`'s error
///   BEFORE `maybe_commit`: the root never flips, so the committed tree is untouched (K8a CoW: `write_data_inner`
///   writes only fresh blocks, `fs.rs:1569-1570`, and the inode last, `fs.rs:1584`) — but unlike `create_files_batch`
///   (`fs.rs:1941`/`:1950`) and `commit_inner` (`fs.rs:968-971`) it does NOT call `txn_unwind` (only `nospace_room`'s versioned retry does), so the allocations
///   it made stay in the mount's in-RAM refcount view. A later commit on the same mount would persist that residue
///   as leaked blocks. So the KERNEL rolls it back: on the root mount [`write_root`] asks the enclosing `with_unafs`
///   hold to discard the cached mount ([`request_mount_discard`], consumed inside the same hold above), which
///   re-mounts from the committed root — the crate's own `txn_unwind` semantics (reload from disk, trust nothing in
///   RAM), done one layer up. The fixture does the same on its RAM volume by re-mounting the device.
/// - **The user sees it once.** [`after_hold`] (outside any hold) posts `Disk full: <volume> - <n> MB free` with an
///   `Open Trash` action at most once per [`TOAST_EVERY_MS`]; [`low_pct`] is the status model's disk fact (free under
///   [`LOW_PCT`] % of the root volume, published by every root write).
///
/// Wire: `[fs] full vol=<name> free_kb=<n> op=write -> ENOSPC`, `[fs] full toast vol=<name> free_mb=<n> ->
/// posted|dropped|headless|suppressed`, `tests diskfull` -> `:: DISKFULL: enospc=1 panics=0 tx_rolled=1
/// after_delete=ok toasts=1 -> PASS :: …`.
pub mod diskfull {
    use crate::fs::vfs::VfsError;
    use ::unafs::fs::FileSystemError;
    use core::sync::atomic::{AtomicU64, Ordering};

    /// The `VfsError::Backend` spelling of a full volume (`attrsys::errno_of` -> `-ENOSPC`).
    pub const NOSPACE: &str = "nospace";
    /// The toast's floor: one per minute.
    pub const TOAST_EVERY_MS: u64 = 60_000;
    /// The status bar's disk item shows below this much free (percent of the volume).
    pub const LOW_PCT: u64 = 5;
    const KB_PER_BLOCK: u64 = ::unafs::storage::BLOCK_SIZE / 1024;

    static ENOSPC: AtomicU64 = AtomicU64::new(0);
    static TOASTS: AtomicU64 = AtomicU64::new(0);
    static SUPPRESSED: AtomicU64 = AtomicU64::new(0);
    /// `crate::arch::ms()` of the last toast; `u64::MAX` = never.
    static LAST_TOAST_MS: AtomicU64 = AtomicU64::new(u64::MAX);
    /// A full event waiting for [`after_hold`]: its `free_kb`, `u64::MAX` = none.
    static PEND_KB: AtomicU64 = AtomicU64::new(u64::MAX);
    static PEND_VOL: crate::sync::Mutex<([u8; 24], usize)> = crate::sync::Mutex::new(([0; 24], 0));
    /// The root volume's free/total blocks as the last root write left them (0 total = not yet measured).
    static ROOT_FREE: AtomicU64 = AtomicU64::new(0);
    static ROOT_TOTAL: AtomicU64 = AtomicU64::new(0);

    /// `Some(free %)` when `free` is under [`LOW_PCT`] % of `total` (pure: the fixture and [`low_pct`] share it).
    pub fn low_of(free: u64, total: u64) -> Option<u8> {
        if total == 0 || free.saturating_mul(100) >= total.saturating_mul(LOW_PCT) {
            return None;
        }
        Some((free.saturating_mul(100) / total) as u8)
    }

    /// The status model's read: the root volume is under [`LOW_PCT`] % free (relaxed loads only — paint-path legal).
    pub fn low_pct() -> Option<u8> {
        low_of(ROOT_FREE.load(Ordering::Relaxed), ROOT_TOTAL.load(Ordering::Relaxed))
    }

    /// `(enospc, toasts, suppressed)` since boot (or the fixture's reset).
    pub fn counts() -> (u64, u64, u64) {
        (ENOSPC.load(Ordering::Relaxed), TOASTS.load(Ordering::Relaxed), SUPPRESSED.load(Ordering::Relaxed))
    }

    /// Fixture hook: zero the counters and forget the last toast (so the once-per-minute floor starts fresh).
    pub fn reset() {
        ENOSPC.store(0, Ordering::Relaxed);
        TOASTS.store(0, Ordering::Relaxed);
        SUPPRESSED.store(0, Ordering::Relaxed);
        LAST_TOAST_MS.store(u64::MAX, Ordering::Relaxed);
        PEND_KB.store(u64::MAX, Ordering::Relaxed);
    }

    /// THE write: `write_data`, with a full volume mapped to `Backend(NOSPACE)` and said on the wire. The caller
    /// rolls the failed transaction back (see the module docs): [`write_root`] on the root, a re-mount in the fixture.
    pub fn write_or_full<D: ::unafs::BlockDevice>(fs: &mut ::unafs::UnaFS<D>, vol: &str, id: u64, off: u64, data: &[u8]) -> Result<(), VfsError> {
        let free_kb = fs.free_blocks().saturating_mul(KB_PER_BLOCK);
        match fs.write_data(id, off, data) {
            Ok(()) => Ok(()),
            Err(FileSystemError::NoSpace) => {
                ENOSPC.fetch_add(1, Ordering::Relaxed);
                serial_println!("[fs] full vol={} free_kb={} op=write -> ENOSPC", vol, free_kb);
                {
                    let mut p = PEND_VOL.lock();
                    let n = vol.len().min(p.0.len());
                    p.0[..n].copy_from_slice(&vol.as_bytes()[..n]);
                    p.1 = n;
                }
                PEND_KB.store(free_kb, Ordering::Relaxed);
                Err(VfsError::Backend(NOSPACE))
            }
            Err(_) => Err(VfsError::Backend("unafs-write")),
        }
    }

    /// The root mount's write (`NativeBackend::write`, inside the `with_unafs` hold): [`write_or_full`], then ANY
    /// failure discards the cached mount inside this same hold (the crate left the failed write's allocations in
    /// RAM; the discard re-mounts from the committed root), and the free count is published for the status bar.
    pub fn write_root(fs: &mut super::KernelUnaFS, vol: &str, id: u64, off: u64, data: &[u8]) -> Result<(), VfsError> {
        let free0 = fs.free_blocks();
        let r = write_or_full(fs, vol, id, off, data);
        let free = if r.is_err() {
            super::request_mount_discard();
            free0 // the committed free count the discard restores
        } else {
            fs.free_blocks()
        };
        ROOT_FREE.store(free, Ordering::Relaxed);
        ROOT_TOTAL.store(fs.superblock.block_count, Ordering::Relaxed);
        r
    }

    /// After the hold (never inside it: the toast reads the session's Trash path): post the pending full event's
    /// toast, at most once per [`TOAST_EVERY_MS`]. `true` when a toast was posted by this call.
    pub fn after_hold() -> bool {
        let kb = PEND_KB.swap(u64::MAX, Ordering::Relaxed);
        if kb == u64::MAX {
            return false;
        }
        let (buf, n) = *PEND_VOL.lock();
        let vol = core::str::from_utf8(&buf[..n]).unwrap_or("volume");
        let now = crate::arch::ms();
        let last = LAST_TOAST_MS.load(Ordering::Relaxed);
        if last != u64::MAX && now.saturating_sub(last) < TOAST_EVERY_MS {
            SUPPRESSED.fetch_add(1, Ordering::Relaxed);
            serial_println!("[fs] full toast vol={} free_mb={} -> suppressed last_ms_ago={}", vol, kb / 1024, now.saturating_sub(last));
            return false;
        }
        LAST_TOAST_MS.store(now, Ordering::Relaxed);
        TOASTS.fetch_add(1, Ordering::Relaxed);
        #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
        let how = if crate::video::notify::disk_full(vol, kb / 1024, &crate::fs::trash::trash_dir()) { "posted" } else { "dropped" };
        #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
        let how = "headless";
        serial_println!("[fs] full toast vol={} free_mb={} -> {}", vol, kb / 1024, how);
        true
    }
}

// =================================================================================================
// FILEVERSIONS (rmbp-ledger B714) — TAIL-APPENDED. The file system keeps the previous contents of a file
// when a write replaces them (the unafs crate's `fs::versions`, unafs-records.md §R17): BP-bar-14 / research B
// §6 — an overwrite is never the only copy; local, no cloud, no app support. The kernel's part is the policy
// and three calls:
//
// * the policy: every mount this file builds — `mount_on` (the lazy bind and every rebind) and the keyed
//   root's `keyboot_bind` (KEYBOOT2) — keeps versions ([`versions_armed`], folded onto those lines);
// * [`versions_on`] lists a file's versions newest first and prints `[fs] version inode=<n> kept=<n>
//   bytes=<n>` (bytes = the blocks they cost x 4096); [`read_version_on`] reads one whole;
//   [`restore_version_on`] makes one the contents again (itself a write that keeps the current contents).
//
// The `_on` forms take any mount (the RAM volume of `tests fileversions`); the bare forms run on THE mount
// under [`with_unafs`]. Paths are the native volume's (the VFS strips its mount prefix).
// =================================================================================================

pub use ::unafs::VersionInfo;

/// FILEVERSIONS (B714): the kernel's versioning policy on a mount (on).
pub fn versions_arm<D: ::unafs::BlockDevice>(fs: &mut UnaFS<D>) {
    fs.set_versioning(true);
}

/// FILEVERSIONS (B714): `mount_on` / `keyboot_bind`'s fold — the bound mount keeps versions.
fn versions_armed(mut fs: KernelUnaFS) -> KernelUnaFS {
    versions_arm(&mut fs);
    fs
}

/// FILEVERSIONS (B714): the versions of the file at `path`, newest first; prints the `[fs] version` line.
pub fn versions_on<D: ::unafs::BlockDevice>(fs: &mut UnaFS<D>, path: &str) -> Result<Vec<VersionInfo>, FileSystemError> {
    let id = fs.resolve_path(path)?;
    let v = fs.versions(id)?;
    let blocks = v.iter().fold(0u64, |a, i| a.saturating_add(i.kept_blocks));
    serial_println!("[fs] version inode={} kept={} bytes={}", id, v.len(), blocks.saturating_mul(::unafs::BLOCK_SIZE));
    Ok(v)
}

/// FILEVERSIONS (B714): the whole contents of version `n` (0 = newest) of the file at `path`.
pub fn read_version_on<D: ::unafs::BlockDevice>(fs: &mut UnaFS<D>, path: &str, n: usize) -> Result<Vec<u8>, FileSystemError> {
    let id = fs.resolve_path(path)?;
    let size = fs.versions(id)?.get(n).map(|i| i.size).ok_or(FileSystemError::NotFound)?;
    fs.read_version(id, n, 0, size)
}

/// FILEVERSIONS (B714): make version `n` (0 = newest) the contents of the file at `path` again.
pub fn restore_version_on<D: ::unafs::BlockDevice>(fs: &mut UnaFS<D>, path: &str, n: usize) -> Result<(), FileSystemError> {
    let id = fs.resolve_path(path)?;
    let r = fs.restore_version(id, n);
    serial_println!("[fs] version restore inode={} n={} ok={}", id, n, r.is_ok() as u8);
    r
}

/// FILEVERSIONS (B714): [`versions_on`] on THE mount (`None`: no volume, or no such file).
pub fn versions(path: &str) -> Option<Vec<VersionInfo>> {
    with_unafs(|fs| versions_on(fs, path).ok()).ok().flatten()
}

/// FILEVERSIONS (B714): [`read_version_on`] on THE mount.
pub fn read_version(path: &str, n: usize) -> Option<Vec<u8>> {
    with_unafs(|fs| read_version_on(fs, path, n).ok()).ok().flatten()
}

/// FILEVERSIONS (B714): [`restore_version_on`] on THE mount (`Ok` once committed). RESTORERESULT (SR161): the ONE
/// restore path, and it keeps the reason — the crate's refusal comes back whole (`ReadOnly`, `CorruptVolume`, ...)
/// for the caller's classifier (Quarry's `ops::restore_refusal`); a mount that never ran the restore (no volume, the
/// device `Busy` past the restart budget) is a `Storage(Io("mount: <MountError>"))`, never an `Ok`.
pub fn restore_version(path: &str, n: usize) -> Result<(), FileSystemError> {
    match with_unafs(|fs| restore_version_on(fs, path, n).map_err(|e| { request_mount_discard(); e })) { // crawl B 10: a failed restore's in-RAM references are discarded inside the hold, as `diskfull::write_root` does
        Ok(r) => r,
        Err(MountError::Fs(e)) => Err(e),
        Err(m) => Err(FileSystemError::Storage(::unafs::storage::Error::Io(format!("mount: {:?}", m)))),
    }
}

/// KEYBOOT (crawl B 2, TAIL-APPENDED): `users::keyboot_key_root`'s hand-off — the first administrator just keyed THE
/// bound root on `handle` (`set_key_slot`), so this boot's rebinds of it (`diskfull::write_root`'s discard, a
/// `force_remount`, a K9 discard) open it with `key` through [`keyboot_mount`] instead of refusing `KeyRequired`.
/// Parked BEFORE the slot is written (an unkeyed volume opens with a key as without one); `None` takes it back when
/// the write failed. IRQ-masked, as every KB_* write (crawl B 7).
pub fn keyboot_adopt(handle: block::BlockHandle, key: Option<::unafs::VolumeKey>) {
    let on = key.is_some();
    crate::arch::without_interrupts(|| {
        *KB_HANDLE.lock() = Some(handle);
        *KB_KEY.lock() = key;
    });
    KB_UNLOCKED.store(on, Ordering::Release);
}

// =================================================================================================
// GEODEBACKUP (rmbp-ledger B740, BP-storage-28) — TAIL-APPENDED, so no line above moves. The record read seam the
// kernel's `.geode` writer (`fs::geodew`) archives: an inode's WHOLE attribute set — its small `attributes` and
// every spilled `large_attributes` value read back (the host crate's `entry_of` rule, handlers/geode). GEODEMOD
// (SR157): the module is `fs::geodew`, declared in `fs/mod.rs` under this file's cfg (GEODEDEP SR160: no re-export).
// =================================================================================================

/// GEODEBACKUP (B740): every attribute of inode `id` (`ino` = its record), small and spilled, keys ascending.
pub fn record_attrs_on<D: ::unafs::BlockDevice>(fs: &mut UnaFS<D>, id: u64, ino: &::unafs::Inode) -> Result<alloc::collections::BTreeMap<String, AttributeValue>, FileSystemError> {
    let mut a = ino.attributes.clone();
    for key in ino.large_attributes.keys() {
        match fs.get_attribute(id, key)? {
            Some(v) => {
                a.insert(key.clone(), v);
            }
            None => return Err(FileSystemError::InvalidAttributeData),
        }
    }
    Ok(a)
}

// =================================================================================================
// UNAFSWRITE (rmbp-ledger B825; storage.md BP-storage-8, unafs-records.md §R16) — a commit's card WRITES leave the
// IRQ-masked MOUNT hold. TAIL-APPENDED (this file's panic Locations are line-pinned); the hooks are same-line folds
// under this module's cfg — :794 (`ticket_foreign`, inside the hold), :900 (`ticket_wait`, before each attempt),
// :2938/:2943/:2962 (`RaSet`'s generation), :3028 (`splitread::bound_now`), :3266 (the `tests unafslock` write leg)
// — so an image without `wc` is byte-identical and its write ops reach `with_unafs` through the alias below.
//
// THE WIRE THAT OWED IT (FILETYPES3 B820, flight 37 boot 3): every UnaFS commit's card writes ran inside the masked
// MOUNT hold (~40 ms per commit on the rMBP card, `[filetypes] … masked_ms=44` for one key), the floor under the
// registry build's `masked_max_ms`.
//
// THE SPLIT (the crate's `stage` / `StagedCommit::write_staged` / `flip`):
//   1. HOLD 1 (MOUNT): the op runs with autocommit off, then `stage` relocates the maps and captures their blocks.
//   2. WRITE (MOUNT released, IRQs as the caller left them): the captured blocks through a device of their own on
//      the bound mount's medium and partition, then the barrier. They are free in the committed tree (K8a), so a
//      cut here leaves the old tree whole. `RA_GEN` moves after it: the mount device's read-ahead never saw them.
//   3. HOLD 2 (MOUNT): `flip` writes the one root sector if nothing moved since the stage; else `Restage` (a whole
//      commit lands everything) or `Lost` (another mount: the op runs again on it, at most twice).
// THE TWO-WRITER RULE: from the ticket's take to its give the holder is the volume's only writer — every
// `with_unafs` waits for the ticket before an attempt (:900), and one that took MOUNT after the ticket was taken
// leaves it untouched and restarts (:794). The holder's own holds pass by task id. Lock order TICKET ⊃ MOUNT.
//
// TICKETOWNER (rmbp-ledger B844; cores.md BP-cores-14): the ticket dies with its holder. `sync::release_task` ->
// `lockowner::release_dead` -> [`splitwrite::release_dead`] (after lockowner's UNAFS case): the bound mount is LEAKED
// (lockowner_release's take + forget, without its force_unlock — in the write phase the dead holder does not hold MOUNT
// and another core may), then the ticket is given back; `[lock] released-on-death name=unafs-ticket …`. The dead op's
// staged RAM leaves with the mount — the next `with_unafs` re-binds a new instance, so a flip of the dead stage is
// `Lost` — and its written blocks are free space in the committed tree (K8a). MOUNT still held by another core past the
// 1500 ms bound: the ticket stays held and `[lock] kept-on-death name=unafs-ticket … why=mount-busy` says so.
// TICKETPREEMPT (B844): a caller with IRQs MASKED that meets a foreign ticket answers Busy at once (`pause`): the holder
// may be preempted on this very core and cannot run until it unmasks. Only an unmasked caller waits (bounded).
// UNAFSWRITE2 first half (B844; storage.md BP-storage-8): HOLD 1 runs the op under `FreshHold` (autocommit off +
// the crate's `hold_fresh`): the op's own write_fresh blocks (inode, extents, data) are held in RAM and travel in the
// `StagedCommit` (`fresh_blocks`), written with MOUNT released like the map blocks. UNAFSWRITE2 second half (B861): the
// btree store's node writes (catalog, name index, attribute index) are held too (`node_blocks`): an op writes nothing
// under MOUNT. `write`/`truncate` split too (vfs.rs): `nospace_room`'s version drop runs in a split op that is its own
// attempt, the drop's commit whole (a full versioned volume only; DROPSPLIT owed), the retried write staged.
// UNAFSWRITE3 (rmbp-ledger B872): a split op may return BEFORE it is done (`OpAgain`): the version drop on a full volume
// is a staged commit of its own (DROPSPLIT) and a write past `CHUNK_BLOCKS` lands in chunks (HELDCAP), each a staged
// commit; `run` flips each and reruns the op (the rerun-on-Lost extended), the ticket held across all of them.
// =================================================================================================
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub use splitwrite::with_unafs_write;
/// UNAFSWRITE (B825): without `wc` (and on aarch64) a write op's commit stays whole under MOUNT — the same call.
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub use with_unafs as with_unafs_write;

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub mod splitwrite {
    use super::*;
    use ::unafs::fs::{FlipOutcome, OpAgain, StagedCommit};
    use core::sync::atomic::AtomicU64;

    /// The commit ticket: the holder's task id, 0 = free.
    pub(super) static TICKET: AtomicU64 = AtomicU64::new(0);
    /// Moves after every write phase; a `RaSet` filled under an older value drops its windows (`ra_sync`).
    static RA_GEN: AtomicU64 = AtomicU64::new(0);

    /// Census: commits flipped after an off-MOUNT write; commits whose card writes ran under MOUNT (ciphered root,
    /// no bound span, a failed write phase, a restage); restages; lost stages; the longest hold (us); the longest
    /// write phase (ms).
    static SPLIT: AtomicU64 = AtomicU64::new(0);
    static WHOLE: AtomicU64 = AtomicU64::new(0);
    static RESTAGED: AtomicU64 = AtomicU64::new(0);
    static LOST: AtomicU64 = AtomicU64::new(0);
    static HELD_MAX_US: AtomicU64 = AtomicU64::new(0);
    static WRITE_MS_MAX: AtomicU64 = AtomicU64::new(0);
    /// UNAFSWRITE2 (B844): the op's own write_fresh blocks carried by flipped split commits (`fresh_blocks`).
    static DATA_STAGED: AtomicU64 = AtomicU64::new(0);
    /// UNAFSWRITE2 second half (B861): the op's btree node blocks carried by flipped split commits (`node_blocks`).
    static NODE_STAGED: AtomicU64 = AtomicU64::new(0);
    /// UNAFSWRITE3 (B872): version drops landed by a split op's own staged commit (DROPSPLIT) / landed whole (ciphered
    /// root, no bound span, a failed write phase, a restage); and chunk commits of a chunked write that owed more (HELDCAP).
    static DROPS_STAGED: AtomicU64 = AtomicU64::new(0);
    static DROPS_WHOLE: AtomicU64 = AtomicU64::new(0);
    static CHUNKS: AtomicU64 = AtomicU64::new(0);
    /// UNAFSWRITE3 (B872): the most chunk reruns one op may ask for (4096 x 4 MiB = 16 GiB); past it the op answers Busy.
    const CHUNK_RUNS: u32 = 4096;

    /// The bound on any wait for the ticket: RENDERWEDGE's `MOUNT_BOUND_MS` (`video/renderwedge.rs`, 1500 ms — not
    /// re-exported by that module, so its value is restated here), past which the caller answers `Busy`.
    const TICKET_BOUND_MS: u64 = 1_500;
    /// Runs of the op when its stage is `Lost` (another mount bound during the write phase).
    const LOST_RUNS: u32 = 2;
    /// A split commit whose write phase took this long prints its `[unafs] commit` line.
    const LINE_MS: u64 = 20;

    fn now_us() -> u64 {
        crate::hidstall::now_us()
    }

    /// TICKETPREEMPT (B844): `false` = IRQs are masked — the holder may be preempted on THIS core and cannot run
    /// until the caller unmasks, so the caller answers Busy at once instead of spinning its bound masked.
    fn pause() -> bool {
        if crate::arch::irqs_masked() {
            return false;
        }
        crate::hlt(); // unmasked: let the holder run
        true
    }

    /// :794 fold — the ticket is held by another task (read inside the MOUNT hold).
    pub(super) fn ticket_foreign() -> bool {
        let t = TICKET.load(Ordering::Acquire);
        t != 0 && crate::arch::sched::current_id() != Some(t)
    }

    /// :900 fold — wait (bounded) until no other task holds the ticket; `false` = the bound passed.
    pub(super) fn ticket_wait() -> bool {
        if !ticket_foreign() {
            return true;
        }
        let t0 = now_us();
        while ticket_foreign() {
            if now_us().saturating_sub(t0) >= TICKET_BOUND_MS * 1000 {
                return false;
            }
            if !pause() {
                return false; // TICKETPREEMPT (B844): masked — Busy now
            }
        }
        true
    }

    pub(super) fn ticket_take(me: u64) -> bool {
        let t0 = now_us();
        loop {
            if TICKET.compare_exchange(0, me, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                return true;
            }
            if now_us().saturating_sub(t0) >= TICKET_BOUND_MS * 1000 {
                return false;
            }
            if !pause() {
                return false; // TICKETPREEMPT (B844): masked — Busy now
            }
        }
    }

    /// TICKETOWNER (B844): `lockowner::release_dead`'s ticket case — `tid` will never run again. `true` = the ticket was
    /// its and is free now (named by `sync::release_task` as `unafs-ticket`). First, while the ticket still keeps every
    /// other writer out, the bound mount is LEAKED (taken and forgotten, never dropped: its drop frees heap memory and
    /// the heap's lock may still be the dead task's until the registry walk) — the dead op's staged RAM goes with it.
    /// MOUNT is taken by a masked `try_lock` spin bounded by `TICKET_BOUND_MS`; never force-unlocked (another core may
    /// hold it legitimately: a `with_unafs` that met the ticket and is restarting). Past the bound the ticket STAYS
    /// held — the dead op's RAM may not be handed to the next writer — and `why=mount-busy` is printed.
    pub fn release_dead(tid: u64) -> bool {
        if tid == 0 || TICKET.load(Ordering::Acquire) != tid {
            return false;
        }
        let t0 = now_us();
        let leaked = crate::arch::without_interrupts(|| loop {
            if let Some(mut g) = MOUNT.try_lock() {
                if let Some(m) = g.take() {
                    core::mem::forget(m);
                }
                break true;
            }
            if now_us().saturating_sub(t0) >= TICKET_BOUND_MS * 1000 {
                break false;
            }
            core::hint::spin_loop();
        });
        if !leaked {
            serial_println!(
                "[lock] kept-on-death name=unafs-ticket at=lockowner by={}@cpu? why=mount-busy ms={} (TICKETOWNER B844: MOUNT held past the bound — the ticket stays held, never a force_unlock)",
                tid,
                now_us().saturating_sub(t0) / 1000
            );
            return false;
        }
        TICKET.compare_exchange(tid, 0, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    /// UNAFSWRITE2 (B844): HOLD 1's op shape — autocommit off and the crate's `hold_fresh` on for exactly the op's
    /// run; both turned back on/off by `Drop`, so no exit of the closure leaves the hold armed (an armed hold would make
    /// the NEXT op's write_fresh blocks wait silently for a commit).
    struct FreshHold<'a>(&'a mut KernelUnaFS);

    impl<'a> FreshHold<'a> {
        fn on(fs: &'a mut KernelUnaFS) -> Self {
            fs.set_autocommit(false);
            fs.hold_fresh(true);
            FreshHold(fs)
        }
    }

    impl Drop for FreshHold<'_> {
        fn drop(&mut self) {
            self.0.hold_fresh(false);
            self.0.set_autocommit(true);
        }
    }

    /// :2962 fold — drop the windows of a `RaSet` filled before the last write phase.
    pub(super) fn ra_sync(ra: &mut RaSet) {
        let g = RA_GEN.load(Ordering::Acquire);
        if ra.seen != g {
            ra.invalidate();
            ra.seen = g;
        }
    }

    enum Stage {
        Done,
        Whole(Result<(), FileSystemError>),
        Split(StagedCommit, block::BlockHandle, PartitionSpan),
    }

    enum Flip {
        Ok,
        Restaged(Result<(), FileSystemError>),
        Whole(Result<(), FileSystemError>),
        Lost,
        Err(FileSystemError),
    }

    /// Run the write op `f` against the one coherent mount with its commit split (see the banner). Same contract
    /// as [`with_unafs`]; a commit that fails is `MountError::Fs`, and a `Busy` leaves nothing landed.
    pub fn with_unafs_write<R>(mut f: impl FnMut(&mut KernelUnaFS) -> R) -> Result<R, MountError> {
        let Some(me) = crate::arch::sched::current_id() else {
            return with_unafs(f); // no task identity to hold the ticket by: the whole commit
        };
        if TICKET.load(Ordering::Acquire) == me {
            return with_unafs(f); // already this task's (never nested in practice: `f` runs under MOUNT)
        }
        if !ticket_take(me) {
            return Err(MountError::Busy);
        }
        let out = run(&mut f);
        TICKET.store(0, Ordering::Release);
        out
    }

    /// UNAFSWRITE3 (rmbp-ledger B872): the op may return before it is done — `take_again` after it, `resume_again` before
    /// its rerun: a version drop (DROPSPLIT) or a chunk (HELDCAP) lands as its own split commit, then the op runs again.
    /// The ticket is held across ALL of the op's commits; a death between them is TICKETOWNER's release (`release_dead`:
    /// the staged RAM goes with the leaked instance; a drop or chunk already flipped stays landed — correct by K8a, and a
    /// landed chunk is a shorter file, never a torn one). Never an `Ok` for an op that owed more: past the bound, Busy.
    fn run<R>(f: &mut impl FnMut(&mut KernelUnaFS) -> R) -> Result<R, MountError> {
        let (mut lost, mut again, mut drops, mut chunks) = (0u32, None::<OpAgain>, 0usize, 0u32); // UNAFSWRITE3 (B872)
        while lost < LOST_RUNS {
            // HOLD 1: the op, then the stage.
            let (r, stage, held1, next) = with_unafs(|fs| {
                let h0 = now_us();
                let r = {
                    let mut hold = FreshHold::on(fs); // UNAFSWRITE2 (B844): autocommit off + hold_fresh on, both undone by Drop
                    if let Some(a) = again {
                        hold.0.resume_again(a); // UNAFSWRITE3 (B872): the cursor of the op's last landed chunk
                    }
                    f(&mut *hold.0)
                };
                let next = fs.take_again(); // UNAFSWRITE3 (B872): the op returned before it was done
                let stage = if MOUNT_DISCARD.load(Ordering::Relaxed) || !fs.has_staged() {
                    Stage::Done // a K9 discard drops the mount after this closure; or nothing to commit
                } else if fs.key_slot().is_some_and(|k| k.cipher == ::unafs::keyslot::CIPHER_AES256_XTS) {
                    Stage::Whole(fs.commit()) // the write device would need the mount's cipher
                } else {
                    match super::splitread::bound_now() {
                        None => Stage::Whole(fs.commit()),
                        Some((handle, span)) => match fs.stage() {
                            Ok(Some(s)) => Stage::Split(s, handle, span),
                            Ok(None) => Stage::Whole(Ok(())), // the v6 -> v7 migration committed whole
                            Err(e) => Stage::Whole(Err(e)),
                        },
                    }
                };
                (r, stage, now_us().saturating_sub(h0), next)
            })?;
            let (s, handle, span) = match stage {
                Stage::Done => return if next.is_some() { Err(MountError::Busy) } else { Ok(r) }, // UNAFSWRITE3 (B872): never a short Ok
                Stage::Whole(Ok(())) => {
                    WHOLE.fetch_add(1, Ordering::Relaxed);
                    match again_after(next, false, &mut drops, &mut chunks) {
                        Next::Done => return Ok(r),
                        Next::Again(a) => {
                            again = Some(a); // UNAFSWRITE3 (B872): landed whole — run the op again
                            continue;
                        }
                        Next::Bound => return Err(MountError::Busy),
                    }
                }
                Stage::Whole(Err(e)) => return Err(MountError::Fs(e)),
                Stage::Split(s, handle, span) => (s, handle, span),
            };

            // WRITE: MOUNT released; the ticket keeps every other writer out.
            let t1 = now_us();
            let wrote = write_phase(&s, handle, &span);
            RA_GEN.fetch_add(1, Ordering::Release);
            let write_ms = now_us().saturating_sub(t1) / 1000;

            // HOLD 2: the flip.
            let (flip, held2) = match with_unafs(|fs| {
                let h0 = now_us();
                let o = if !wrote {
                    if fs.holds_staged(&s) { Flip::Whole(fs.commit()) } else { Flip::Lost }
                } else {
                    match fs.flip(&s) {
                        Ok(FlipOutcome::Landed) => Flip::Ok,
                        Ok(FlipOutcome::Restage) => Flip::Restaged(fs.commit()),
                        Ok(FlipOutcome::Lost) => Flip::Lost,
                        Err(e) => Flip::Err(e),
                    }
                };
                (o, now_us().saturating_sub(h0))
            }) {
                Ok(v) => v,
                Err(e) => {
                    force_remount(); // the staged RAM goes with the mount: a Busy never lands later
                    return Err(e);
                }
            };
            let held = held1.max(held2);
            HELD_MAX_US.fetch_max(held, Ordering::Relaxed);
            WRITE_MS_MAX.fetch_max(write_ms, Ordering::Relaxed);
            let (name, out) = match flip {
                Flip::Ok => {
                    SPLIT.fetch_add(1, Ordering::Relaxed);
                    DATA_STAGED.fetch_add(s.fresh_blocks() as u64, Ordering::Relaxed);
                    NODE_STAGED.fetch_add(s.node_blocks() as u64, Ordering::Relaxed); // UNAFSWRITE2 (B861)
                    ("ok", Some(Ok(())))
                }
                Flip::Restaged(res) => {
                    RESTAGED.fetch_add(1, Ordering::Relaxed);
                    WHOLE.fetch_add(1, Ordering::Relaxed);
                    ("restaged", Some(res))
                }
                Flip::Whole(res) => {
                    WHOLE.fetch_add(1, Ordering::Relaxed);
                    ("whole", Some(res))
                }
                Flip::Lost => {
                    LOST.fetch_add(1, Ordering::Relaxed);
                    ("lost", None)
                }
                Flip::Err(e) => return Err(MountError::Fs(e)), // the flip failed and unwound: nothing landed
            };
            if write_ms >= LINE_MS || name != "ok" {
                serial_println!("[unafs] commit staged_blocks={} write_ms={} held_us={} flip={}", s.blocks(), write_ms, held, name);
            }
            match out {
                Some(Ok(())) => match again_after(next, name == "ok", &mut drops, &mut chunks) {
                    Next::Done => return Ok(r),
                    Next::Again(a) => again = Some(a), // UNAFSWRITE3 (B872): landed — run the op again
                    Next::Bound => return Err(MountError::Busy),
                },
                Some(Err(e)) => return Err(MountError::Fs(e)),
                None => lost += 1, // Lost: run the op again on the mount that is bound now (`again` stays the last landed answer)
            }
        }
        Err(MountError::Busy)
    }

    /// UNAFSWRITE3 (B872): what `run` does after a commit LANDED.
    enum Next {
        Done,
        Again(OpAgain),
        Bound,
    }

    /// UNAFSWRITE3 (B872): the op asked to run again (`take_again`) and its commit landed (`staged` = split): count it,
    /// say a drop on the wire, and bound the reruns — drops by the version ring (`RING_MAX`), chunks by `CHUNK_RUNS`.
    fn again_after(next: Option<OpAgain>, staged: bool, drops: &mut usize, chunks: &mut u32) -> Next {
        let Some(a) = next else {
            return Next::Done;
        };
        if a.drop {
            if *drops >= ::unafs::RING_MAX {
                return Next::Bound;
            }
            *drops += 1;
            if staged { DROPS_STAGED.fetch_add(1, Ordering::Relaxed) } else { DROPS_WHOLE.fetch_add(1, Ordering::Relaxed) };
            serial_println!("[unafs] drop staged={}", staged as u8);
        } else {
            if *chunks >= CHUNK_RUNS {
                return Next::Bound;
            }
            *chunks += 1;
            CHUNKS.fetch_add(1, Ordering::Relaxed);
        }
        Next::Again(a)
    }

    /// The staged blocks through a device of their own on the bound mount's medium — the constructors `mount_on`
    /// uses (`SdSectorDevice::open_on`, `BlockAdapter::for_partition`). `false` on any refusal.
    fn write_phase(s: &StagedCommit, handle: block::BlockHandle, span: &PartitionSpan) -> bool {
        match SdSectorDevice::open_on(handle) {
            Ok(dev) => s.write_staged(&mut BlockAdapter::for_partition(dev, span)).is_ok(),
            Err(_) => false,
        }
    }

    /// `(split, whole, restaged, lost, held_max_us, write_ms_max)` this boot.
    pub fn census() -> (u64, u64, u64, u64, u64, u64) {
        (
            SPLIT.load(Ordering::Relaxed),
            WHOLE.load(Ordering::Relaxed),
            RESTAGED.load(Ordering::Relaxed),
            LOST.load(Ordering::Relaxed),
            HELD_MAX_US.load(Ordering::Relaxed),
            WRITE_MS_MAX.load(Ordering::Relaxed),
        )
    }

    /// UNAFSWRITE2 second half (B861): `op` through `with_unafs_write` landed as exactly ONE split commit (none whole).
    fn staged_once(mut op: impl FnMut(&mut KernelUnaFS) -> bool) -> bool {
        let (s0, w0, _, _, _, _) = census();
        let r = with_unafs_write(|fs| op(fs));
        let (s1, w1, _, _, _, _) = census();
        matches!(r, Ok(true)) && s1.wrapping_sub(s0) == 1 && w1 == w0
    }

    /// `tests unafslock` write leg (:3266 fold): 8 attribute commits on a scratch file under `/var`, each through
    /// `with_unafs_write`. ok = 8 commits, all split, none under MOUNT, none lost, every hold <= 20 ms. UNAFSWRITE2 second
    /// half (B861): then a 3-block `write` and a `truncate` to 5000, each ONE split commit read back; ok also needs a node
    /// block per attribute commit in the stages and `under_mount_writes` (whole commits + held spills) 0. UNAFSWRITE3 (B872):
    /// then the file is extended at its EOF by 2 x `CHUNK_BLOCKS` + 1 blocks — ok also needs 3 chunks, each one split commit,
    /// none whole, none spilled, read back off MOUNT — and no version drop this boot landed whole (`drop_staged`).
    pub fn write_leg() {
        const LEAF: &str = "unafslock.tmp";
        const PATH: &str = "/var/unafslock.tmp";
        const N: u64 = 8;
        let made = with_unafs_write(|fs| -> Result<u64, (bool, &'static str)> {
            if fs.key_slot().is_some_and(|k| k.cipher == ::unafs::keyslot::CIPHER_AES256_XTS) {
                return Err((true, "ciphered root"));
            }
            let var = fs.resolve_path("/var").map_err(|_| (true, "no /var"))?;
            match fs.resolve_path(PATH) {
                Ok(id) => Ok(id),
                Err(_) => fs.create_file(var, String::from(LEAF)).map_err(|_| (false, "create")),
            }
        });
        let id = match made {
            Ok(Ok(id)) => id,
            Ok(Err((true, why))) => {
                serial_println!("[unafslock] write -> SKIP ({})", why);
                return;
            }
            Ok(Err((false, why))) => {
                serial_println!("[unafslock] write commits=0 -> FAIL why={}", why);
                return;
            }
            Err(_) => {
                serial_println!("[unafslock] write -> SKIP (no native unafs volume)");
                return;
            }
        };
        let (s0, w0, r0, l0, _, _) = census();
        let d0 = DATA_STAGED.load(Ordering::Relaxed);
        let n0 = NODE_STAGED.load(Ordering::Relaxed); // UNAFSWRITE2 (B861)
        let hd0 = with_unafs(|fs| fs.commit_stats().held_direct).unwrap_or(0);
        HELD_MAX_US.store(0, Ordering::Relaxed);
        WRITE_MS_MAX.store(0, Ordering::Relaxed);
        let mut commits = 0u64;
        for i in 0..N {
            let r = with_unafs_write(|fs| fs.set_attribute(id, String::from("unafslock"), AttributeValue::Int(i as i64)));
            if matches!(r, Ok(Ok(()))) {
                commits += 1;
            }
        }
        // UNAFSWRITE2 second half (B861): `write` and `truncate` through the same staged path, read back.
        const WBYTES: usize = 3 * 4096;
        let wbuf = alloc::vec![0x5au8; WBYTES]; // heap, not the 16 KiB task stack
        let write_ok = staged_once(|fs| fs.write_data(id, 0, &wbuf).is_ok())
            && with_unafs(|fs| fs.read_data(id, 0, WBYTES as u64)).ok().and_then(|r| r.ok()).is_some_and(|d| d == wbuf);
        let trunc_ok = staged_once(|fs| fs.truncate_data(id, 5000).is_ok())
            && with_unafs(|fs| (fs.read_inode(id).map(|i| i.size), fs.read_data(id, 0, 5000)))
                .ok()
                .is_some_and(|(size, d)| matches!(size, Ok(5000)) && d.is_ok_and(|d| d[..] == wbuf[..5000]));
        let (s1, w1, r1, l1, held, wms) = census();
        let data = DATA_STAGED.load(Ordering::Relaxed).wrapping_sub(d0); // UNAFSWRITE2 (B844): each attribute commit's inode travels in its stage
        let node = NODE_STAGED.load(Ordering::Relaxed).wrapping_sub(n0); // UNAFSWRITE2 (B861): each attribute commit's index path travels too
        let spilled = with_unafs(|fs| fs.commit_stats().held_direct).unwrap_or(0).wrapping_sub(hd0);
        // UNAFSWRITE3 HELDCAP (B872): a write past the hold's cap, extending the file at its EOF (5000), lands in chunks.
        const CBYTES: usize = (2 * ::unafs::fs::CHUNK_BLOCKS as usize + 1) * 4096;
        let cbuf = alloc::vec![0xa5u8; CBYTES]; // heap, not the task stack
        let (cs0, cw0, _, _, _, _) = census();
        let ch0 = CHUNKS.load(Ordering::Relaxed);
        let chd0 = with_unafs(|fs| fs.commit_stats().held_direct).unwrap_or(0);
        let cwrote = matches!(with_unafs_write(|fs| fs.write_data(id, 5000, &cbuf).is_ok()), Ok(true));
        let (cs1, cw1, _, _, _, _) = census();
        let chunks = CHUNKS.load(Ordering::Relaxed).wrapping_sub(ch0) + cwrote as u64; // the reruns, plus the last chunk
        let cspill = with_unafs(|fs| fs.commit_stats().held_direct).unwrap_or(0).wrapping_sub(chd0);
        let cread = super::splitread::read(PATH, 5000, CBYTES as u64).is_ok_and(|d| d == cbuf);
        let chunk_ok = cwrote && chunks == 3 && cs1.wrapping_sub(cs0) == 3 && cw1 == cw0 && cspill == 0 && cread;
        let drops = DROPS_STAGED.load(Ordering::Relaxed).wrapping_add(DROPS_WHOLE.load(Ordering::Relaxed));
        let drop_ok = DROPS_WHOLE.load(Ordering::Relaxed) == 0; // DROPSPLIT: no drop this boot landed whole
        let _ = with_unafs_write(|fs| fs.resolve_path("/var").and_then(|var| fs.unlink(var, LEAF)));
        let (split, whole, restaged, lost) = (s1.wrapping_sub(s0), w1.wrapping_sub(w0), r1.wrapping_sub(r0), l1.wrapping_sub(l0));
        let under = whole.wrapping_add(spilled); // UNAFSWRITE2 (B861): card writes under MOUNT — whole commits + held spills
        let ok = commits == N && split >= N && whole == 0 && lost == 0 && held <= 20_000 && data >= N && node >= N && under == 0 && write_ok && trunc_ok && chunk_ok && drop_ok;
        serial_println!(
            "[unafslock] write commits={} split={} write_held_us={} write_ms_max={} io_under_lock={} restaged={} lost={} data_blocks_staged={} node_blocks_staged={} under_mount_writes={} write_staged={} truncate_staged={} drop_staged={} drops={} chunks={} -> {}",
            commits,
            split,
            held,
            wms,
            whole,
            restaged,
            lost,
            data,
            node,
            under,
            if write_ok { "ok" } else { "FAIL" },
            if trunc_ok { "ok" } else { "FAIL" },
            if drop_ok { "ok" } else { "FAIL" },
            drops,
            chunks,
            if ok { "ok" } else { "FAIL" }
        );
    }
}

// ── POISON2 (rmbp-ledger B846, BP-storage-8) — the MOUNT's death mark CHECKED (MOUNTPOISON) and the lazy bind moved
// BEFORE the masked hold (MOUNTSTACK). STACKGUARD3 (B832) left the mount's death recovery to lockowner's leak: its
// `lockowner_release` force-unlocked MOUNT and only THEN took it to forget the instance, so a taker that won that window
// ran its transaction on the dead holder's half-applied in-RAM state. Now the release MARKS the lock first (the POISON
// table, `sync::Mutex::mark_poison`) and every `with_unafs` take checks it (`mount_poison_check`). And the lazy bind —
// which mounts, reads the root and COMMITS the reclaim drain — ran inside the masked MOUNT hold on whatever stack the
// caller had; with the commit ticket (x86 `wc`) it runs off the hold (`prebind`, `kb_bind_root`'s install shape), the
// ticket making it the volume's only writer. At the tail: no line above moves. ──

/// Poisoned MOUNT takes found (a dead holder's mark checked). Written under MOUNT.
#[cfg(target_arch = "x86_64")]
static MPOISON_CHECKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// MOUNTPOISON — run by every `with_unafs` take of MOUNT (the attempt's :794; the pre-bind's peek and install) with MOUNT
/// held: a dead holder's mark DISCARDS the in-RAM instance by `mem::forget`, never `Drop` (its write-back would flush the
/// half-applied transaction). The leak is deliberate (`dropped=1`; 0 = lockowner already leaked it): the medium is
/// consistent at the last root flip and the bind that follows re-reads it.
#[cfg(target_arch = "x86_64")]
fn mount_poison_check(g: &mut Option<BoundMount>) {
    if MOUNT.take_poison() {
        let dropped = g.take().map(core::mem::forget).is_some();
        MPOISON_CHECKS.fetch_add(1, Ordering::AcqRel);
        serial_println!("[unafs] mount poisoned -> rebind dropped={} (POISON2 B846: the dead holder's in-RAM instance leaked, never written back)", dropped as u8);
    }
}

/// MOUNTSTACK — the bind the attempt's hold would run (:806). With a task identity the bind belongs to [`prebind`] (off
/// the hold; the ticket held on x86 `wc`, the [`BIND`] latch elsewhere — HEAPREGION B854 extended it to every build), so a
/// hold that finds no mount latches contention and the attempt RESTARTS (the next attempt pre-binds). With no task identity
/// it binds here as before — where no ticket exists, only under the [`BIND`] latch (one binder at a time).
fn bind_in_hold() -> Result<BoundMount, MountError> {
    if crate::arch::sched::current_id().is_some() || BIND_LIVE.load(Ordering::Acquire) {
        TXN_BUSY.store(true, Ordering::Relaxed); // a task pre-binds; a bind task past its bound still runs: never a second bind
        return Err(MountError::Busy);
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    {
        let Some(_bind) = BIND.try_lock() else {
            TXN_BUSY.store(true, Ordering::Relaxed);
            return Err(MountError::Busy);
        };
        bind_mount()
    }
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        bind_mount()
    }
}

/// MOUNTSTACK — the lazy bind BEFORE the masked hold (`with_unafs`, each attempt): MOUNT unbound -> become THE binder (x86
/// `wc`: take the commit ticket, or it is already this task's inside `with_unafs_write`; elsewhere: win the [`BIND`]
/// latch), look again, bind UNMASKED with no MOUNT hold, install under one short masked take, give the ticket/latch back.
/// `Err` = the bind's own verdict (not contention), returned as the hold's bind returned it; contention (Busy, the ticket's
/// bound, a held MOUNT, another binder) answers `Ok` and the attempt restarts.
///
/// THE RACE RULE (HEAPREGION B854): the bind COMMITS the volume's reclaim drain, so two binds must never run at once. One
/// binder at a time — the ticket on x86 `wc`, [`BIND`] elsewhere, and on a no-ticket build the in-hold binder of a caller
/// with no task identity takes [`BIND`] too; a bind task that outlives its bounded join keeps the next bind out until it
/// ends ([`BIND_LIVE`]). The binder installs under one short masked MOUNT take; a mount found there wins and the binder's
/// instance is dropped.
fn prebind() -> Result<(), MountError> {
    let Some(me) = crate::arch::sched::current_id() else { return Ok(()) };
    if !mount_unbound() {
        return Ok(());
    }
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        let mine = splitwrite::TICKET.load(Ordering::Acquire) == me;
        if !mine && !splitwrite::ticket_take(me) {
            return Ok(());
        }
        let out = if mount_unbound() { prebind_now() } else { Ok(()) };
        if !mine {
            splitwrite::TICKET.store(0, Ordering::Release);
        }
        out
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    {
        let _ = me;
        let Some(_bind) = BIND.try_lock() else { return Ok(()) };
        if mount_unbound() { prebind_now() } else { Ok(()) }
    }
}

/// The pre-bind's peek: MOUNT free (`try_lock`, masked — it never spins) and empty after its poison check. A held MOUNT
/// answers `false` (its holder may be binding or bound; the attempt waits on it).
fn mount_unbound() -> bool {
    crate::arch::without_interrupts(|| match MOUNT.try_lock() {
        #[allow(unused_mut)]
        Some(mut g) => {
            #[cfg(target_arch = "x86_64")]
            mount_poison_check(&mut *g);
            g.is_none()
        }
        None => false,
    })
}

/// The binder's turn held: bind ([`bind_sized`] — on a sized task when the caller's own stack is small), then install
/// under one short masked take (x86 `wc`: RENDERWEDGE's bounded pre-wait first; a mount bound meanwhile wins and ours is
/// dropped — `kb_bind_root`'s shape).
fn prebind_now() -> Result<(), MountError> {
    match bind_sized() {
        Ok(m) => {
            #[cfg(all(target_arch = "x86_64", feature = "wc"))]
            if !crate::video::renderwedge::wait_unlocked(&MOUNT, crate::video::renderwedge::W_UNAFS) {
                drop(m);
                return Ok(());
            }
            let lost = crate::arch::without_interrupts(|| {
                let mut g = MOUNT.lock();
                #[cfg(target_arch = "x86_64")]
                mount_poison_check(&mut *g);
                if g.is_none() {
                    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
                    splitread::note_bound(m.handle);
                    *g = Some(m);
                    None
                } else {
                    Some(m)
                }
            });
            drop(lost);
            Ok(())
        }
        Err(e) => {
            if TXN_BUSY.load(Ordering::Relaxed) || matches!(e, MountError::Busy) {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

/// `tests lockreg`'s mount leg (sync.rs `mount_leg`): the bound mount is marked as a dead holder's release marks it (under
/// this task's own MOUNT hold, then a normal release), and one `with_unafs` must check it, discard the instance and re-bind.
/// `ok` = the check ran, the call answered, a mount is bound again. Costs one leaked mount per run, by design.
#[cfg(target_arch = "x86_64")]
pub fn poison_leg() -> &'static str {
    let bound = || crate::arch::without_interrupts(|| MOUNT.lock().is_some());
    if !bound() {
        return "skip(unbound)";
    }
    let c0 = MPOISON_CHECKS.load(Ordering::Acquire);
    let marked = crate::arch::without_interrupts(|| {
        let g = MOUNT.lock();
        let m = MOUNT.mark_poison();
        drop(g);
        m
    });
    if !marked {
        return "table-full";
    }
    let ok = with_unafs(|_| ()).is_ok();
    if MPOISON_CHECKS.load(Ordering::Acquire) <= c0 {
        "not-checked"
    } else if !ok || !bound() {
        "no-rebind"
    } else {
        "ok"
    }
}

// ── HEAPREGION (rmbp-ledger B854) BINDSTACK: the bind off a small caller's stack. `bind_mount` (the volume probe, the
// superblock, the reclaim drain) is deep; a 16 KiB task that reaches the pre-bind parks in a JOIN while the bind runs on a
// sized `unafs-bind` task (`sched::spawn_joinable_stack`), its result handed back through [`BIND_OUT`] (gen-checked). The
// caller keeps the ticket/latch across the join (the bind task holds no lock, so `bind_mount`'s MOUNTSTACK assert holds);
// the join is BOUNDED (`BIND_BOUND_MS`, splitwrite's TICKET_BOUND_MS figure): a bind task killed by its guard never posts,
// so past the bound the caller answers Busy `why=bind-died` and gives the ticket/latch back, and [`BIND_LIVE`] keeps any
// new bind out until that task ends (its late result is installed by the next pre-bind, never a second concurrent bind).
// At the tail: no line above moves. ──

/// The bind task's usable stack. The arc's figure, not a measured high-water (owed BINDSTACKHIGH).
const BIND_STACK: usize = 64 * 1024;
/// A caller whose usable stack is at most this binds through the task.
const BIND_SMALL: usize = 16 * 1024;
/// The join's bound: splitwrite's `TICKET_BOUND_MS` (the ticket's own bound), the same 1500 ms.
const BIND_BOUND_MS: u64 = 1_500;

/// The no-ticket builds' binder latch (non-`wc` x86, aarch64). TRY-locked only — never spun on — so holding it across the
/// unmasked bind and the join park is sound; `try_lock` records no order edge (no inversion under MOUNT), and x86's
/// registry releases it if its holder dies.
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
static BIND: Mutex<()> = Mutex::new(());
/// The last bind task's generation, and whether one is still running (set by the spawner, cleared by the task).
static BIND_GEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static BIND_LIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The bind task's result, tagged with its generation; a result of a superseded generation is dropped.
static BIND_OUT: Mutex<Option<(u64, Result<BoundMount, MountError>)>> = Mutex::new(None);

/// `Some(usable KiB)` when the caller should bind on the task: a scheduled task, IRQs unmasked (it must park in the
/// join), its usable stack span at most [`BIND_SMALL`].
fn small_caller() -> Option<usize> {
    crate::arch::sched::current_id()?;
    if crate::arch::irqs_masked() {
        return None;
    }
    let (lo, hi) = crate::arch::without_interrupts(crate::arch::sched::current_stack_bounds)?;
    let span = hi.saturating_sub(lo) as usize;
    (span <= BIND_SMALL).then_some(span >> 10)
}

/// Take the result of generation `bgen` (the generation) (masked short take; a stale result is dropped outside the hold).
fn bind_take(bgen: u64) -> Option<Result<BoundMount, MountError>> {
    let got = crate::arch::without_interrupts(|| BIND_OUT.lock().take());
    match got {
        Some((g, r)) if g == bgen => Some(r),
        other => {
            drop(other);
            None
        }
    }
}

/// Spawn `entry(arg)` on a [`BIND_STACK`] task and join it, bounded by [`BIND_BOUND_MS`]. `false` = the bound passed (the
/// task never posted: killed, or still running). The one spawn+join of the bind and of its `tests unafslock` leg.
fn run_sized(name: &'static str, entry: fn(usize), arg: usize) -> bool {
    let h = crate::arch::sched::spawn_joinable_stack(name, entry, arg, crate::arch::sched::CPU_AUTO, BIND_STACK);
    #[cfg(target_arch = "x86_64")]
    {
        h.join_timeout(crate::arch::ms_to_ticks(BIND_BOUND_MS)) == crate::arch::sched::JoinResult::Completed
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        // aarch64's JoinHandle has no bounded join: poll the task's own done flag (`RUN_DONE`, set as its last act) on the
        // clock that runs through a masked span, napping a tick between looks; the handle is dropped unjoined (its Arc
        // keeps the semaphore alive until the task posts).
        let t0 = crate::hidstall::now_us();
        loop {
            if RUN_DONE.swap(false, Ordering::AcqRel) {
                drop(h);
                return true;
            }
            if crate::hidstall::now_us().saturating_sub(t0) >= BIND_BOUND_MS * 1000 {
                drop(h);
                return false;
            }
            crate::arch::sched::sleep_ticks(1);
        }
    }
}

/// aarch64's bounded-join flag (see [`run_sized`]); set by the sized task's entry as its last act.
#[cfg(not(target_arch = "x86_64"))]
static RUN_DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[inline]
fn run_done() {
    #[cfg(not(target_arch = "x86_64"))]
    RUN_DONE.store(true, Ordering::Release);
}

/// The bind, on the caller or on a sized task. Wire once per bind: `[unafs] bind on=<task|caller> stack_kib=<n>`.
fn bind_sized() -> Result<BoundMount, MountError> {
    let bgen = BIND_GEN.load(Ordering::Acquire);
    if let Some(r) = bind_take(bgen) {
        return r; // a task that outlived its join finished meanwhile: its bind is this bind
    }
    if BIND_LIVE.load(Ordering::Acquire) {
        TXN_BUSY.store(true, Ordering::Relaxed);
        return Err(MountError::Busy); // that task still runs (or died): never a second concurrent bind
    }
    let Some(kib) = small_caller() else {
        serial_println!("[unafs] bind on=caller stack_kib={} (HEAPREGION B854)", crate::arch::without_interrupts(crate::arch::sched::current_stack_bounds).map_or(0, |(lo, hi)| (hi.saturating_sub(lo) >> 10) as usize));
        return bind_mount();
    };
    let bgen = BIND_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    BIND_LIVE.store(true, Ordering::Release);
    #[cfg(not(target_arch = "x86_64"))]
    RUN_DONE.store(false, Ordering::Release);
    serial_println!("[unafs] bind on=task stack_kib={} caller_kib={} (HEAPREGION B854)", BIND_STACK >> 10, kib);
    if !run_sized("unafs-bind", bind_task, bgen as usize) {
        serial_println!("[unafs] bind on=task why=bind-died bound_ms={} gen={} -> Busy (HEAPREGION B854: the ticket/latch given back; no new bind until the task ends)", BIND_BOUND_MS, bgen);
        TXN_BUSY.store(true, Ordering::Relaxed);
        return Err(MountError::Busy);
    }
    bind_take(bgen).unwrap_or(Err(MountError::Busy))
}

/// `unafs-bind`: one `bind_mount` on the sized stack, the result posted under its generation.
fn bind_task(bgen: usize) {
    let r = bind_mount();
    let stale = crate::arch::without_interrupts(|| BIND_OUT.lock().replace((bgen as u64, r)));
    drop(stale);
    BIND_LIVE.store(false, Ordering::Release);
    run_done();
}

// ── `tests unafslock`'s bind leg (x86 `wc`, folded on `splitread::selftest`'s first line): a 16 KiB `unafslock-bind`
// worker asks `small_caller` and runs a probe through the SAME `run_sized` on a `BIND_STACK` task. ──
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static LEG_CALLER_KIB: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static LEG_TASK_KIB: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static LEG_WORD: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn bind_leg() {
    use core::sync::atomic::Ordering::{Acquire, Release};
    let word = if crate::arch::sched::current_id().is_none() || crate::arch::irqs_masked() {
        "no-task"
    } else {
        LEG_WORD.store(0, Release);
        LEG_CALLER_KIB.store(0, Release);
        LEG_TASK_KIB.store(0, Release);
        let h = crate::arch::sched::spawn_joinable_stack("unafslock-bind", bind_leg_worker, 0, crate::arch::sched::CPU_AUTO, BIND_SMALL);
        if h.join_timeout(crate::arch::ms_to_ticks(2 * BIND_BOUND_MS)) != crate::arch::sched::JoinResult::Completed {
            "no-task"
        } else {
            match LEG_WORD.load(Acquire) {
                1 => "ok",
                2 => "caller-not-small",
                3 => "not-sized",
                _ => "no-task",
            }
        }
    };
    serial_println!(
        "[unafslock] bind_task={} on=task stack_kib={} caller_kib={}",
        word,
        LEG_TASK_KIB.load(Acquire),
        LEG_CALLER_KIB.load(Acquire)
    );
}

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn bind_leg_worker(_: usize) {
    use core::sync::atomic::Ordering::Release;
    let Some(kib) = small_caller() else {
        LEG_WORD.store(2, Release);
        return;
    };
    LEG_CALLER_KIB.store(kib, Release);
    let ran = run_sized("unafs-bind", bind_leg_probe, 0);
    let task_kib = LEG_TASK_KIB.load(core::sync::atomic::Ordering::Acquire);
    LEG_WORD.store(if ran && task_kib >= BIND_STACK >> 10 { 1 } else { 3 }, Release);
}

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn bind_leg_probe(_: usize) {
    let kib = crate::arch::without_interrupts(crate::arch::sched::current_stack_bounds).map_or(0, |(lo, hi)| (hi.saturating_sub(lo) >> 10) as usize);
    LEG_TASK_KIB.store(kib, core::sync::atomic::Ordering::Release);
    run_done();
}
