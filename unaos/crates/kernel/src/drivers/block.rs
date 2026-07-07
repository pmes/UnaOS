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

/// M6g: register the microSD (EMMC2/SDHCI) as the block backend — publish its geometry AND flip the
/// selector so `read_block` (and, since U9, `write_block`) route to `drivers::emmc2`. Called once, from
/// the bare-metal BSP probe after a successful card init (`emmc2::probe`). aarch64 bare-metal only; x86
/// never calls it, so its read/write path is byte-identical to the pre-M6g xHCI-only code.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn register_sd(dev: BlockDeviceInfo) {
    *BLOCK_DEVICE.lock() = Some(dev);
    BACKEND.store(BACKEND_SD, Ordering::Release);
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
