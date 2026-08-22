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
//! whose unafs volume arrives on any OTHER handle (the orin's TegraSd card,
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

use spin::Mutex;

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
/// `BlockHandle::TegraSd` reads the card AND is sized from the card; one opened on
/// `BlockHandle::Global` reads the stick AND is sized from the stick. Neither can be built wrong.
/// See [`handle_info`] / [`handle_read`] / [`handle_write`] for the arm each handle contributes.
pub struct SdSectorDevice {
    /// Which registry handle this device reads, writes and is sized from — the single name.
    handle: block::BlockHandle,
    sectors: u64,
}

/// SDSEAM: the live geometry row for `handle`.
///
/// Exhaustive on purpose (see [`SdSectorDevice`]): every handle the block layer defines contributes
/// exactly one arm, and the arm names the same registry slot its read/write arms below route to.
///
/// ### MERGE NOTE — the `TegraSd` arms, WRITTEN AT THE TRUNK LANDING
/// `BlockHandle::TegraSd` and its entry points (`tegra_sd_info`, `read_block_tegra_sd`,
/// `write_block_tegra_sd`) live in `drivers/block.rs` and arrived with the orin track's TEGRASD
/// commit; they did not exist on the pi branch, and `drivers/block.rs` was not that arc's lane, so
/// the arms could not be written there. They were not guesswork either — the totality of these
/// matches made the merge report each missing arm as an E0004, and each is the single line named
/// below under the TEGRASD cfg triple
/// `#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]`. All four are now
/// applied verbatim, exactly as written here:
///
/// * [`handle_info`] — `BlockHandle::TegraSd => block::tegra_sd_info(),`
/// * [`handle_read`] — `BlockHandle::TegraSd => block::read_block_tegra_sd(lba, buf),`
/// * [`handle_write`] — `BlockHandle::TegraSd => block::write_block_tegra_sd(lba, buf),`
///   (which refuses in every cfg — the card is read-only outside `sdmmc_arm`, so a unafs write
///   attempt on it fails closed rather than reaching the medium)
/// * [`SdSectorDevice::open_on`] — `BlockHandle::TegraSd => dev.num_blocks,` (a dedicated slot;
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
        block::BlockHandle::TegraSd => block::tegra_sd_info(),
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
        block::BlockHandle::TegraSd => block::read_block_tegra_sd(lba, buf),
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
        block::BlockHandle::TegraSd => block::write_block_tegra_sd(lba, buf),
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
            block::BlockHandle::TegraSd => dev.num_blocks,
        };
        Ok(Self { handle, sectors })
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
        match handle_write(self.handle, lba, buf) {
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
    let span = locate_on(handle)?;
    partition_witness(handle, &span);
    let dev = SdSectorDevice::open_on(handle)?;
    let adapter = BlockAdapter::for_partition(dev, &span);
    UnaFS::mount(adapter).map_err(MountError::Fs)
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

    // Does the mounted volume fit inside that partition? `block_count` is 4096 B blocks; the
    // partition is counted in 512 B sectors, so eight sectors per block. Checked, not assumed.
    let fits = span
        .block_count
        .checked_mul(8)
        .map(|s| s <= p.sector_count)
        .unwrap_or(false);

    // Re-read the superblock magic through the BOUNDED, partition-relative path.
    let range = block::PartitionRange::new(handle, &p);
    let mut b0 = [0u8; 512];
    let magic_ok = range.read_block(0, &mut b0).is_ok()
        && b0[..::unafs::superblock::MAGIC.len()] == ::unafs::superblock::MAGIC;

    serial_println!(
        ":: PART: unafs span check — slot={} type=0x{:02x} part=[{}..{}) span_base={} span_blocks={} fits={} magic={} ::",
        p.slot,
        p.type_byte,
        p.start_lba,
        p.end_lba(),
        span.base_lba,
        span.block_count,
        if fits { "yes" } else { "NO" },
        if magic_ok { "ok" } else { "MISSING" }
    );
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
/// ### MERGE NOTE — the TegraSd arm this seam is waiting for
/// When `BlockHandle::TegraSd` lands (orin's TEGRASD, `drivers/block.rs`), the totality of this
/// match reports it as an E0004 alongside [`handle_info`]'s trio. Its arm here is one line under the
/// TEGRASD cfg triple, and it is the whole point of this arc:
///
/// * [`bind_probe_admitted`] — `BlockHandle::TegraSd => true,` (the orin's unafs volume rides the
///   card's dedicated handle while `Global` is the USB stick; admitting the probe is what lets the
///   shell's `with_unafs` find the card's volume with no tegra-side shell wiring at all)
/// * [`bind_probe_candidates`] — add `block::BlockHandle::TegraSd` to the array (and grow its
///   length), in enum order; the array mirrors the enum and [`handle_kind_name`] gains
///   `BlockHandle::TegraSd => "tegra-sd",`.
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
        block::BlockHandle::TegraSd => true,
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
        block::BlockHandle::TegraSd,
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
        block::BlockHandle::TegraSd => "tegra-sd",
    }
}

/// UNAFSBIND: the handle the live cached mount is riding, or `None` if no mount is currently bound.
/// A read-only peek (masked, like every `MOUNT` touch); it never triggers the lazy bind.
pub fn mount_bound_handle() -> Option<block::BlockHandle> {
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
fn bind_mount() -> Result<BoundMount, MountError> {
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
    crate::arch::without_interrupts(|| {
        let mut guard = MOUNT.lock();
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
            match bind_mount() {
                Ok(m) => *guard = Some(m),
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
        match with_unafs_attempt(&mut f) {
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
fn request_mount_discard() {
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

// =====================================================================
// K8c: the snapshot READ path under CURRENT-ACL enforcement.
//
// Ruling of record (Peter, 2026-07-16, "we want high security"): snapshot
// reads are governed by the LIVE object's CURRENT ACL, re-evaluated at read
// time. Revocation is total — a principal that cannot read the live object
// cannot read ANY snapshot of it. Snapshots preserve bytes, never authority.
//
// ONE evaluator at this layer ([`read_authz`]): every snapshot-read surface
// (`usnapcat`, `usnapls`, [`snapshot_read`]) defers to it, keyed on the LIVE
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
        Some(AttributeValue::String(s)) => s.clone(),
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
        if fs.superblock.version == ::unafs::superblock::VERSION
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
