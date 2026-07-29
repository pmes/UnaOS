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

/// BOTEV: one-shot latches for the "concrete cause" witness below. `BlockError::Io` is the coarse
/// verdict every FAT-layer error collapses to (`FatError::Io`), which is why the flight recorder
/// could only report `:: FR: UNAOS.LOG reservation failed (Io) ::` on metal — the SCSI/BOT reason
/// (a `BotError` variant, or a non-`Passed` CSW with its residue) was discarded one frame below.
/// Latched per direction so the FIRST read failure and the FIRST write failure each get exactly one
/// line for the whole boot: enough to name the cause, incapable of flooding a wedged pipe's log.
static IO_WITNESS_READ: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static IO_WITNESS_WRITE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// BOTEV: print the concrete SCSI/BOT cause of ONE failed block operation, at most once per
/// direction per boot, then let the caller collapse it to `BlockError::Io` exactly as before. A
/// `bot_err` names the transport fault (`Timeout` = the pipe the BOT recovery witnesses describe);
/// a `csw_status` names a completed transaction the DEVICE rejected, with its residue. Pure
/// logging: no retry, no state change, no bearing on the returned error.
fn io_cause_witness(
    op: &str,
    lba: u64,
    outcome: Result<crate::drivers::xhci::BotResult, crate::drivers::xhci::BotError>,
) {
    let latch = if op.as_bytes().first() == Some(&b'r') { &IO_WITNESS_READ } else { &IO_WITNESS_WRITE };
    if latch.swap(true, core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    match outcome {
        Ok(res) => serial_println!(
            ":: BLK: io-cause op={} lba={} csw_status={:?} residue={} (first, once) ::",
            op, lba, res.status, res.residue),
        Err(e) => serial_println!(
            ":: BLK: io-cause op={} lba={} bot_err={:?} (first, once) ::", op, lba, e),
    }
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

/// INSTALL-SEL: which of the two registry handles a device row was read from. The block layer keeps
/// two `Option<BlockDeviceInfo>` slots — the GLOBAL [`BLOCK_DEVICE`] (whatever the active backend is:
/// the xHCI stick on x86, the microSD once `register_sd` has run on the Pi) and the dedicated USB
/// handle [`USB_BLOCK_DEVICE`] — and the graphical installer's chooser can show BOTH as separate rows
/// when they name different devices. "Row 1" is therefore not a property of the device; it is a
/// property of one frame's list. Naming the handle makes the row's identity independent of the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockHandle {
    /// The global [`BLOCK_DEVICE`] entry — read/written through [`read_block`] / [`write_block`].
    Global,
    /// The dedicated [`USB_BLOCK_DEVICE`] entry — read/written through [`read_block_usb`] /
    /// [`write_block_usb`], which bypass the backend selector.
    Usb,
}

/// INSTALL-SEL: a durable name for ONE block device, good across frames and across a registry change.
///
/// The graphical installer captures this when the operator commits to a row, and the installer engine
/// re-resolves it at go-time through [`lookup`]. The point is that a list that changes between the
/// operator's choice and the engine's bind — which it now can, since `unpublish_usb_geometry` lets the
/// list SHRINK on a physical disconnect — can never silently retarget the install: an identity that no
/// longer resolves is a refusal, not a fallback to whatever disk happens to occupy the row now.
///
/// ### Why these three fields
/// * `handle` — as above, it decides WHICH registry slot (and therefore which read/write path) the
///   row named, so the Pi's microSD-in-the-global and stick-in-the-USB-handle case cannot be confused.
/// * `slot_id` — the xHCI slot the device enumerated on. This is the same key
///   [`unpublish_usb_geometry`] matches on for retraction, so the two agree by construction. A replug
///   lands on a NEW slot id, so "same disk, physically re-seated" correctly reads as a different
///   device to a dialog that was opened before the replug.
/// * `num_blocks` — a cheap geometry witness that closes the residual slot-id-reuse window: a freed
///   slot handed to some other, differently sized device will not match. Comparing more is possible
///   but not more honest; a mismatch here already forces the refusal path, which is the safe direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockDeviceId {
    pub handle: BlockHandle,
    pub slot_id: u8,
    pub num_blocks: u64,
}

impl BlockDeviceInfo {
    /// INSTALL-SEL: the durable identity of this device as read from `handle`.
    pub fn id(&self, handle: BlockHandle) -> BlockDeviceId {
        BlockDeviceId { handle, slot_id: self.slot_id, num_blocks: self.num_blocks }
    }
}

/// INSTALL-SEL: resolve an identity against the LIVE registry — `Some(info)` only if the named handle
/// still holds exactly that device, `None` if it is gone or has been replaced. Purely a query: it
/// takes one handle's lock briefly and changes nothing, so it is safe to call from a repaint as well
/// as from the engine's bind. Both callers going through this one function is what makes the erase
/// warning on glass and the device the engine actually binds provably the same device.
pub fn lookup(id: BlockDeviceId) -> Option<BlockDeviceInfo> {
    let cur = match id.handle {
        BlockHandle::Global => info(),
        BlockHandle::Usb => usb_info(),
    };
    match cur {
        Some(d) if d.slot_id == id.slot_id && d.num_blocks == id.num_blocks => Some(d),
        _ => None,
    }
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
        other => {
            io_cause_witness("read-usb", lba, other);
            return Err(BlockError::Io);
        }
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
        other => {
            io_cause_witness("write-usb", lba, other);
            Err(BlockError::Io)
        }
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

/// USB-UNPLUG: retract the geometry a USB mass-storage device published, when its xHCI slot is torn
/// down on a physical disconnect. This is the missing half of [`publish_usb_geometry`]: before it, the
/// xHCI layer handled removal correctly at ITS level (slot bindings cleared, DISABLE_SLOT queued —
/// the metal wire shows `[Port 1] slot 1 torn down on disconnect` / `DISABLE_SLOT slot 1 -> code 1`)
/// but nothing downstream ever heard about it, so `BLOCK_DEVICE` / `USB_BLOCK_DEVICE` kept a dead
/// device forever. Every consumer that re-reads the registry each pass — the graphical installer's
/// per-frame disk list (`video::instgui::devices`), the shell's `df`, the FAT mounts — therefore went
/// on offering an unplugged disk as a live install target, and a replug (which lands on a NEW slot
/// number) simply overwrote the entry rather than adding a second one, hiding the leak.
///
/// ### Matching is by slot id, never by "the USB backend"
/// The retraction fires only for a registry entry whose `slot_id` EQUALS the torn-down xHCI slot. That
/// single rule gets three cases right at once:
/// - **x86 (this track):** the stick owns both handles, both carry its slot, both clear.
/// - **Pi / aarch64 with a microSD:** `register_sd` publishes the card with `slot_id: 0` and xHCI slot
///   0 is never a live device slot, so a USB disconnect can never retract the SD's global geometry —
///   without this function needing to know the `BACKEND` selector at all.
/// - **Slot-id reuse:** an id freed by DISABLE_SLOT and later handed to some other device cannot
///   retract a disk that has since republished under a different slot, because the stored id is
///   compared, not merely the fact that *a* slot went away.
///
/// ### In-flight I/O
/// There is no dangling handle to chase. Every block entry point ([`read_block`], [`write_block`],
/// [`read_block_usb`], [`write_block_usb`]) re-reads the registry through `info()` / `usb_info()` on
/// EVERY call and geometry-bounds the LBA against that snapshot, so the first operation issued after
/// the retraction fails honestly with [`BlockError::NotReady`] instead of transferring against a dead
/// slot — the same shape as the BOT timeout path, which likewise reports the failure rather than
/// pretending a transfer completed. A FAT mount is a by-value `FatFs` re-derived from a fresh
/// `mount()`, so an existing mount does not keep the geometry alive either; its next sector read is
/// what surfaces the loss. Callers holding a long synchronous job (the installer engine) see the same
/// error on their next block op and abort with it.
///
/// The pending storage-ready edge is dropped as part of the retraction: an edge raised by the attach
/// but not yet consumed by the main loop would otherwise drive a FAT mount against a disk that is no
/// longer there. A replug re-raises it from `publish_usb_geometry` in the normal way.
///
/// Returns true if a registry entry was actually retracted (i.e. this slot WAS the storage device).
pub fn unpublish_usb_geometry(slot_id: u8) -> bool {
    // Slot 0 is the xHCI "no slot" sentinel and also the id `register_sd` stamps on the microSD.
    // Neither is ever a device whose disconnect we are being told about.
    if slot_id == 0 {
        return false;
    }

    // Snapshot the identity BEFORE clearing, so the witness below can name the disk that left.
    // Each handle is taken on its own — never both at once — so this adds no lock ordering to the
    // block layer. Clearing is guarded per handle by its own slot match, because on the Pi the global
    // may legitimately be the microSD while the USB handle is the stick, and only the latter must go.
    let mut departing: Option<BlockDeviceInfo> = None;
    {
        let mut usb = USB_BLOCK_DEVICE.lock();
        if (*usb).map(|d| d.slot_id) == Some(slot_id) {
            departing = *usb;
            *usb = None;
        }
    }
    {
        let mut glob = BLOCK_DEVICE.lock();
        if (*glob).map(|d| d.slot_id) == Some(slot_id) {
            if departing.is_none() {
                departing = *glob;
            }
            *glob = None;
        }
    }
    let Some(dev) = departing else {
        return false;
    };
    // Drop an unconsumed attach edge: there is nothing left to mount.
    USB_STORAGE_READY.store(false, core::sync::atomic::Ordering::Release);

    // The removal witness. QEMU cannot hot-unplug a device here, so this line is what proves on the
    // next attended metal boot that the disconnect actually reached the block registry — the exact
    // evidence whose absence defined this defect.
    let product = core::str::from_utf8(&dev.product).unwrap_or("?").trim_end();
    let vendor = core::str::from_utf8(&dev.vendor).unwrap_or("?").trim_end();
    serial_println!(
        ":: BLK: removed '{}' '{}' (xhci slot {} disconnect) ::",
        vendor, product, slot_id
    );
    true
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
        other => {
            // BOTEV: name the concrete SCSI/BOT cause once before it collapses into
            // `BlockError::Io` / `FatError::Io`.
            io_cause_witness("read", lba, other);
            return Err(BlockError::Io);
        }
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
        other => {
            // BOTEV: this is the line that turns the flight recorder's `(Io)` into a diagnosis —
            // `/UNAOS.LOG`'s reservation writes come through here.
            io_cause_witness("write", lba, other);
            Err(BlockError::Io)
        }
    }
}
