// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Minimal block-device abstraction over the USB Mass Storage (xHCI BOT) driver.
// A single device is supported (the QEMU usb-storage target); geometry is published
// here after SCSI bring-up, and read/write are serviced by locking the xHCI controller.

use spin::Mutex;
use crate::drivers::xhci::{XHCI_CONTROLLER, CswStatus};

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

/// Read one block (`lba`) into `buf`. Locks BLOCK_DEVICE only briefly (to read geometry),
/// then locks the xHCI controller — never both at once — so there is no nested-lock deadlock.
/// Returns the number of bytes copied.
pub fn read_block(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let dev = info().ok_or(BlockError::NotReady)?;
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
    unsafe { core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), n); }
    Ok(n)
}

/// Write one block (`lba`) from `buf` (zero-padded to the block size).
pub fn write_block(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let dev = info().ok_or(BlockError::NotReady)?;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
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
