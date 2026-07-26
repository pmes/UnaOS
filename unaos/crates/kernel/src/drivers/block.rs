// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Minimal block-device abstraction over the USB Mass Storage (xHCI BOT) driver.
// A single device is supported (the QEMU usb-storage target); geometry is published
// here after SCSI bring-up, and read/write are serviced by locking the xHCI controller.

use spin::Mutex;
use crate::drivers::xhci::{XHCI_CONTROLLER, CswStatus};

// M6g backend selector (aarch64 bare-metal only). The block layer dispatches over a registered
// backend: the default is the xHCI USB-MSC path this file has always served; `register_sd` flips it
// to the EMMC2/SDHCI microSD driver (`drivers::emmc2`). Everything SD-related is cfg-gated so the x86
// build compiles the pre-M6g file verbatim (no dead static, no behavioural change) — the seam is
// invisible to any target that never calls `register_sd`.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
use core::sync::atomic::{AtomicU8, Ordering};
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
const BACKEND_XHCI: u8 = 0;
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
const BACKEND_SD: u8 = 1;
/// Which backend `read_block`/`write_block` dispatch to. Only `register_sd` ever flips it to SD; the
/// xHCI enumeration path (which populates `BLOCK_DEVICE` from the xHCI driver) never touches it.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static BACKEND: AtomicU8 = AtomicU8::new(BACKEND_XHCI);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    NotReady,
    Io,
    BadLba,
}

/// Geometry + identity of the enumerated mass-storage device.
#[derive(Clone, Copy)]
pub struct BlockDeviceInfo {
    pub slot_id: u8,
    pub block_size: u32,
    pub num_blocks: u64,
    pub vendor: [u8; 8],
    pub product: [u8; 16],
}

/// Generic block device interface (single-device for now; see [`read_block`]/[`write_block`]).
pub trait BlockDevice {
    fn block_size(&self) -> u32;
    fn num_blocks(&self) -> u64;
    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError>;
    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError>;
}

/// The one registered USB block device (populated by xHCI storage bring-up).
pub static BLOCK_DEVICE: Mutex<Option<BlockDeviceInfo>> = Mutex::new(None);

/// Snapshot of the current block device geometry, if any.
pub fn info() -> Option<BlockDeviceInfo> {
    *BLOCK_DEVICE.lock()
}

/// PIUSB-27: geometry of the USB mass-storage stick, published by the xHCI storage bring-up ALONGSIDE
/// `BLOCK_DEVICE`. Kept separate so it survives `register_sd` flipping the global device to the microSD:
/// on the Pi the SD backend owns `BLOCK_DEVICE`, but the USB stick's geometry stays available here so it
/// can be mounted read-only through [`read_block_usb`]. `None` until a USB stick enumerates.
pub static USB_BLOCK_DEVICE: Mutex<Option<BlockDeviceInfo>> = Mutex::new(None);

/// PIUSB-27: snapshot of the USB stick geometry, if one enumerated.
pub fn usb_info() -> Option<BlockDeviceInfo> {
    *USB_BLOCK_DEVICE.lock()
}

/// PIUSB-27: storage-ready edge for the USB FAT mount. `set_usb_ready` is raised by the xHCI storage
/// bring-up (once per enumeration, so it re-arms on hot-plug); `take_usb_ready` is the main loop's
/// consume-once read (swaps it back to false). The mount runs from the main loop rather than the
/// bring-up path because the FAT mount re-locks the xHCI controller via `read_block_usb`, and the
/// bring-up already holds that lock — so the mount+witness must fire with the lock released.
static USB_STORAGE_READY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// PIUSB-27: raise the USB storage-ready edge (called from the xHCI bring-up on each enumeration).
pub fn set_usb_ready() {
    USB_STORAGE_READY.store(true, core::sync::atomic::Ordering::Release);
}

/// PIUSB-27: consume the USB storage-ready edge — returns true exactly once per raised edge.
pub fn take_usb_ready() -> bool {
    USB_STORAGE_READY.swap(false, core::sync::atomic::Ordering::AcqRel)
}

/// PIUSB-27: read one block (`lba`) from the USB mass-storage stick DIRECTLY through the xHCI controller,
/// bypassing the backend selector — so the stick is readable even when the global block device is the
/// microSD (BACKEND_SD on the Pi). Strictly read-only. Geometry (block size, bound) comes from
/// [`USB_BLOCK_DEVICE`]; the transfer is the same xHCI BOT READ(10) the default path has always used, and
/// it takes only the xHCI controller lock (the SD/emmc2 path is untouched). Returns bytes copied.
pub fn read_block_usb(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let dev = usb_info().ok_or(BlockError::NotReady)?;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
    }
    let mut guard = XHCI_CONTROLLER.lock();
    let xhci = guard.as_mut().ok_or(BlockError::NotReady)?;
    match xhci.storage_read10(lba as u32, 1) {
        Ok(res) if res.status == CswStatus::Passed => {}
        _ => return Err(BlockError::Io),
    }
    let src = xhci.storage_data_ptr().ok_or(BlockError::Io)?;
    let n = (dev.block_size as usize).min(buf.len());
    unsafe {
        core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), n);
    }
    Ok(n)
}

/// USB-WRITE: write one block (`lba`) to the USB mass-storage stick DIRECTLY through the xHCI
/// controller — the write twin of [`read_block_usb`]. Bypasses the backend selector, so the stick
/// is writable even when the global block device is the microSD (BACKEND_SD on the Pi). Geometry
/// (block size, bound) comes from [`USB_BLOCK_DEVICE`]; the transfer is the same xHCI BOT WRITE(10)
/// the default path uses, taking only the xHCI controller lock (the SD/emmc2 path is untouched).
/// The caller's `buf` is staged (zero-padded to the block size) into the controller's DMA buffer,
/// then WRITE(10) is issued; a non-`Passed` CSW propagates as [`BlockError::Io`] — the write NEVER
/// reports a false success. This is the block-layer half of a writable `/usb`: the FAT layer's
/// `write_sector` routes its `Usb` source here in place of the PIUSB-27 read-only refusal.
pub fn write_block_usb(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let dev = usb_info().ok_or(BlockError::NotReady)?;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
    }
    let mut guard = XHCI_CONTROLLER.lock();
    let xhci = guard.as_mut().ok_or(BlockError::NotReady)?;
    let dst = xhci.storage_data_ptr().ok_or(BlockError::Io)?;
    let n = (dev.block_size as usize).min(buf.len());
    unsafe {
        core::ptr::write_bytes(dst, 0, dev.block_size as usize);
        core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, n);
    }
    match xhci.storage_write10(lba as u32, 1) {
        Ok(res) if res.status == CswStatus::Passed => Ok(()),
        _ => Err(BlockError::Io),
    }
}

/// PIUSB-28: publish the geometry of a freshly enumerated USB mass-storage device. Always records it
/// under the dedicated [`USB_BLOCK_DEVICE`] handle (so `read_block_usb`/`/fs/usb` can reach the stick),
/// then raises the storage-ready edge. The global [`BLOCK_DEVICE`] is only claimed when USB is the ACTIVE
/// backend — i.e. no SD card has flipped the selector to `BACKEND_SD`. On the Pi the microSD registers at
/// BSP probe (long before xHCI enum), so a later-enumerated USB stick must NOT overwrite the SD geometry:
/// PI-FS-2 traced a 14 MiB USB card reader's `num_blocks` clobbering the SD's global, which bounded fresh
/// unafs mounts to the reader's size → `PartError::OutOfBounds(63)`. On x86 (no `register_sd`, no BACKEND
/// selector) the stick IS the default/boot backend, so this always claims the global — byte-identical to
/// the pre-PIUSB-28 behavior there. Must run OUTSIDE the xHCI controller lock's storage callers as before.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn publish_usb_geometry(dev: BlockDeviceInfo) {
    *USB_BLOCK_DEVICE.lock() = Some(dev);
    // Claim the global only while USB is still the active backend; once the SD has registered, leave
    // BLOCK_DEVICE (the SD's geometry) untouched — the stick stays reachable via USB_BLOCK_DEVICE.
    if BACKEND.load(Ordering::Acquire) != BACKEND_SD {
        *BLOCK_DEVICE.lock() = Some(dev);
    }
    set_usb_ready();
}

/// PIUSB-28: x86 / non-SD-capable targets — the USB stick is the default (boot) block backend, so it
/// always claims the global `BLOCK_DEVICE` alongside the dedicated USB handle. Preserves the historical
/// behavior on any target that never compiles the SD backend / BACKEND selector.
#[cfg(not(all(target_arch = "aarch64", feature = "baremetal")))]
pub fn publish_usb_geometry(dev: BlockDeviceInfo) {
    *BLOCK_DEVICE.lock() = Some(dev);
    *USB_BLOCK_DEVICE.lock() = Some(dev);
    set_usb_ready();
}

/// M6g: register the microSD (EMMC2/SDHCI) as the block backend — publish its geometry AND flip the
/// selector so `read_block` (and, since U9, `write_block`) route to `drivers::emmc2`. Called once, from
/// the bare-metal BSP probe after a successful card init (`emmc2::probe`). aarch64 bare-metal only; x86
/// never calls it, so its read/write path is byte-identical to the pre-M6g xHCI-only code.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn register_sd(dev: BlockDeviceInfo) {
    *BLOCK_DEVICE.lock() = Some(dev);
    BACKEND.store(BACKEND_SD, Ordering::Release);
}

/// USBFALL F1: one-shot latch for the fail-closed refusal witness, so a write-heavy caller that keeps
/// retrying cannot flood the console with the same line.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static SUBST_REFUSED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// USBFALL F1: close the fail-OPEN backend substitution on the Pi bare-metal build.
///
/// On `aarch64 + baremetal` the canonical `Default` backend is the microSD: `emmc2::probe()` runs on the BSP
/// synchronously (`main.rs`), long before any FAT-writing consumer, and flips `BACKEND` to `BACKEND_SD` from
/// `register_sd`. If the card fails identify, `BACKEND` stays `BACKEND_XHCI` — and a later-enumerated USB
/// stick populates the global `BLOCK_DEVICE` via `publish_usb_geometry`, at which point every
/// `BlockSource::Default` WRITE silently lands on somebody's USB stick instead of the boot card. That is a
/// fail-open SUBSTITUTION of one physical device for another, and it is what this refuses: on a build whose
/// canonical backend is SD, a `Default` write with no registered SD returns `NotReady` and says so on serial
/// once, producing an honest "no writable volume" boot instead of misdirected writes.
///
/// Deliberately WRITES ONLY. Reads may still fall through to the BOT path — a read cannot corrupt the wrong
/// device, and the USB mount has its own dedicated `read_block_usb` handle regardless.
///
/// Deliberately `baremetal`-gated, NOT a blanket "xHCI writes are suspect" rule. On QEMU-virt aarch64
/// (`test-arm`) and on x86 the SD backend is never compiled, xHCI IS the legitimate sole backend, and this
/// function does not exist — those builds keep their pre-USBFALL write path byte-for-byte. The refusal is
/// about substitution on a platform that has a canonical backend, not about xHCI.
///
/// Byte-inert on a healthy SD boot: `BACKEND_SD` is set before the first FAT write, so this returns `Ok`
/// without touching the console.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn guard_default_write_backend() -> Result<(), BlockError> {
    if BACKEND.load(Ordering::Acquire) == BACKEND_SD {
        return Ok(());
    }
    if !SUBST_REFUSED.swap(true, Ordering::Relaxed) {
        serial_println!(
            ":: USBFALL: no SD backend registered — refusing Default WRITE (would land on the USB stick) ::"
        );
    }
    Err(BlockError::NotReady)
}

/// Read one block (`lba`) into `buf`. Locks BLOCK_DEVICE only briefly (to read geometry),
/// then locks the xHCI controller — never both at once — so there is no nested-lock deadlock.
/// Returns the number of bytes copied.
pub fn read_block(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let dev = info().ok_or(BlockError::NotReady)?;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
    }

    // M6g: SD backend routes to the EMMC2/SDHCI driver; the xHCI body below is unchanged (and the
    // only path an x86 build compiles). read_block_512 re-guards the lba against its own num_blocks.
    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    if BACKEND.load(Ordering::Acquire) == BACKEND_SD {
        return crate::drivers::emmc2::read_block_512(lba, buf);
    }

    let mut guard = XHCI_CONTROLLER.lock();
    let xhci = guard.as_mut().ok_or(BlockError::NotReady)?;

    match xhci.storage_read10(lba as u32, 1) {
        Ok(res) if res.status == CswStatus::Passed => {}
        _ => return Err(BlockError::Io),
    }
    let src = xhci.storage_data_ptr().ok_or(BlockError::Io)?;
    let n = (dev.block_size as usize).min(buf.len());
    unsafe { core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), n); }
    Ok(n)
}

/// Write one block (`lba`) from `buf` (zero-padded to the block size).
pub fn write_block(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    // USBFALL F1: fail CLOSED rather than substituting the USB stick for a missing SD card. See
    // `guard_default_write_backend` — compiled only where SD is the canonical backend (Pi bare-metal).
    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    guard_default_write_backend()?;

    let dev = info().ok_or(BlockError::NotReady)?;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
    }

    // U9: the SD backend now services in-place block WRITES (polled CMD24), routing to the EMMC2/SDHCI
    // driver just as reads route to `read_block_512`. write_block_512 re-guards the lba against its own
    // num_blocks. The xHCI body below is unchanged (and the only path an x86 build compiles).
    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    if BACKEND.load(Ordering::Acquire) == BACKEND_SD {
        return crate::drivers::emmc2::write_block_512(lba, buf);
    }

    let mut guard = XHCI_CONTROLLER.lock();
    let xhci = guard.as_mut().ok_or(BlockError::NotReady)?;

    // Stage the data into the controller's DMA buffer, then issue WRITE(10).
    let dst = xhci.storage_data_ptr().ok_or(BlockError::Io)?;
    let n = (dev.block_size as usize).min(buf.len());
    unsafe {
        core::ptr::write_bytes(dst, 0, dev.block_size as usize);
        core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, n);
    }
    match xhci.storage_write10(lba as u32, 1) {
        Ok(res) if res.status == CswStatus::Passed => Ok(()),
        _ => Err(BlockError::Io),
    }
}
