// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Minimal block-device abstraction over the USB Mass Storage (xHCI BOT) driver.
// A single device is supported (the QEMU usb-storage target); geometry is published
// here after SCSI bring-up, and read/write are serviced by locking the xHCI controller.

use spin::Mutex;
use crate::drivers::xhci::{self, CswStatus, XhciClaimError, XhciLoan};

/// MULTIBLK: the largest number of 512-byte blocks ONE `read_blocks`/`write_blocks` call may carry.
/// It is the xHCI SCSI staging buffer's capacity (`xhci::STORAGE_MAX_BLOCKS`), re-exported here so
/// filesystem code chunks against the block layer's own published limit instead of hard-coding a
/// number that belongs to a driver two layers down. A caller that hands in more is refused, never
/// silently truncated — a short write that reported success is the one failure mode a filesystem
/// cannot detect.
pub const MAX_BLOCKS_PER_OP: usize = crate::drivers::xhci::STORAGE_MAX_BLOCKS as usize;

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
    /// WEDGE-8 (F3) / WEDGE-10 (F2): the storage device is loaned out to another context and this
    /// call refused to wait for it — the xHCI controller mid-BOT-transaction or mid-service-pass,
    /// or the microSD card mid-CMD17/CMD24 sector transfer. Callers that CAN wait — unmasked,
    /// schedulable contexts — never see this from a healthy device: `claim_xhci_for_io` and
    /// `emmc2::claim_for_io` each retry with a bounded `hlt` wait first. A caller running with IRQs
    /// MASKED gets it immediately, because a masked wait on a driver lock is exactly the deadlock
    /// this family is named for; the FAT layer retries OUTSIDE its masked span and surfaces
    /// `-EAGAIN` on exhaustion.
    Busy,
}

/// WEDGE-8 (F3): claim the xHCI controller for one block transaction.
///
/// Masked callers (the FAT/dir RMW spans under `without_interrupts`) get an INSTANT
/// `BlockError::Busy` when the controller is loaned out — the whole point of F3 is that a masked
/// context must never wait on a driver lock, because the loan holder is preemptible and, once
/// preempted on this core, can only run again if this core takes timer IRQs.
///
/// Unmasked callers keep the old effectively-blocking semantics, honestly bounded: retry the claim
/// with a `hlt` between attempts (each wakes on the next IRQ, letting the scheduler run the loan
/// holder) up to `hw_wait_budget()` wall-clock — long enough for any healthy service pass or BOT
/// transaction, short enough that a wedged 25 s failing-transfer hold surfaces as `Busy` instead of
/// hanging the caller forever.
fn claim_xhci_for_io() -> Result<XhciLoan, BlockError> {
    match xhci::claim() {
        Ok(l) => return Ok(l),
        Err(XhciClaimError::NotReady) => return Err(BlockError::NotReady),
        Err(XhciClaimError::Busy) => {}
    }
    if crate::arch::irqs_masked() {
        return Err(BlockError::Busy);
    }
    let start = crate::arch::now_cycles();
    let budget = crate::arch::hw_wait_budget();
    loop {
        crate::hlt();
        match xhci::claim() {
            Ok(l) => return Ok(l),
            Err(XhciClaimError::NotReady) => return Err(BlockError::NotReady),
            Err(XhciClaimError::Busy) => {}
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= budget {
            return Err(BlockError::Busy);
        }
    }
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
    /// SDHC-4b (x86, `sdhcblk` knob): the dedicated [`SDHC_BLOCK_DEVICE`] entry — the card in the
    /// machine's INTERNAL SD slot, read through [`read_block_sdhc`] / [`read_blocks_sdhc`], which
    /// bypass the backend selector exactly as the `Usb` pair does. See the SDHC-4b section below for
    /// why it is a third handle and not a third value of the `BACKEND` selector.
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    Sdhc,
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
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        BlockHandle::Sdhc => sdhc_info(),
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
    // WEDGE-8 (F3): a claimed LOAN, not a held lock — the BOT pump below runs with no lock held.
    let mut xhci = claim_xhci_for_io()?;
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
    // WEDGE-8 (F3): a claimed LOAN, not a held lock — the BOT pump below runs with no lock held.
    let mut xhci = claim_xhci_for_io()?;
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
///
/// FRGUARD (GR21): `hotplug` says HOW this claim arrived — `false` for a device found by the initial
/// port scan (the boot volume), `true` for one queued by a live connect-change edge. It is recorded,
/// never acted on, here; only [`default_writable`] reads it, and only on `x86_64 + sdhcblk`. This arm
/// ignores it entirely, so the Pi's decisions are unchanged.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn publish_usb_geometry(dev: BlockDeviceInfo, hotplug: bool) {
    let _ = hotplug; // aarch64 keys its substitution guard on the BACKEND selector, not on claim origin
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
///
/// FRGUARD (GR21): the claim ORIGIN is recorded here — see [`GLOBAL_CLAIM_HOTPLUG`]. Recording is all
/// that happens; the registry writes below are byte-identical to the pre-FRGUARD ones, so a build
/// without `sdhcblk` is unchanged in both code and decisions.
#[cfg(not(all(target_arch = "aarch64", feature = "baremetal")))]
pub fn publish_usb_geometry(dev: BlockDeviceInfo, hotplug: bool) {
    // Ordered BEFORE the registry write, so no reader can observe a populated global slot whose
    // origin flag still describes the PREVIOUS claimer.
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    GLOBAL_CLAIM_HOTPLUG.store(hotplug, core::sync::atomic::Ordering::Release);
    let _ = hotplug;
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
/// FRGUARD (GR21): **that hazard also exists on x86, and a guard for it now exists there too.** SDHC-4b gave
/// x86 an internal SD backend that registers as handle `Sdhc` and deliberately leaves `BLOCK_DEVICE` empty,
/// so an x86 machine can boot with a canonical volume that is not the `Default` target — and Boot AI-2 proved
/// the consequence on metal: the flight recorder created and wrote a 262 656-byte `/UNAOS.LOG` onto a card
/// the operator hot-plugged to read, because that card was the first thing to claim the global slot. The x86
/// arm of `default_writable` (below) closes it, keyed on claim ORIGIN rather than on this file's `BACKEND`
/// selector, which x86 does not compile.
///
/// Deliberately WRITES ONLY. Reads may still fall through to the BOT path — a read cannot corrupt the wrong
/// device, and the USB mount has its own dedicated `read_block_usb` handle regardless.
///
/// Deliberately gated to platforms that HAVE a canonical backend, NOT a blanket "xHCI writes are suspect"
/// rule. On QEMU-virt aarch64 (`test-arm`) and on an x86 build without `sdhcblk` the SD backend is never
/// compiled, xHCI IS the legitimate sole backend, and the refusal does not exist — those builds keep their
/// pre-USBFALL write path byte-for-byte. The refusal is about substitution on a platform that has a
/// canonical backend, not about xHCI.
///
/// FRGUARD (GR21): x86 + `sdhcblk` is now such a platform and has its own arm below. The line above used to
/// say "and on x86", full stop; Boot AI-2 proved that premise dead.
///
/// Byte-inert on a healthy SD boot: `BACKEND_SD` is set before the first FAT write, so this returns `Ok`
/// without touching the console.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn default_writable() -> bool {
    BACKEND.load(Ordering::Acquire) == BACKEND_SD
}

/// FRGUARD (GR21): how the CURRENT occupant of the global [`BLOCK_DEVICE`] slot got there — `true` if it
/// was queued by a live connect-change (hot-plug) edge, `false` if the initial port scan found it. Written
/// by [`publish_usb_geometry`] on every claim, so it always describes the device the slot holds now.
///
/// This is the whole discriminator, and it is the one the metal capture forces. Boots AH and AI-1 ALSO had
/// an `Sdhc` handle registered (the operator's 29 MiB Panasonic sat in the internal slot, `blocks=60800`)
/// while the machine booted from a 60 GB card in a USB reader, and the flight recorder's writes to that
/// boot volume were entirely correct. A guard keyed on "an `Sdhc` handle exists" alone would have refused
/// them. What is different about Boot AI-2 is not that a canonical backend existed — it is that the global
/// slot was still EMPTY when the boot finished and was then claimed, 296 seconds in, by a medium the
/// operator plugged in to read.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
static GLOBAL_CLAIM_HOTPLUG: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// USBFALL F1 / FRGUARD (GR21): the x86 arm of the substitution guard — the same policy as the Pi's,
/// re-keyed onto the predicate that is actually true on this target.
///
/// x86 has no `BACKEND` selector to read, because SDHC-4b deliberately gave the internal SD card its own
/// THIRD registry handle and left the global slot untouched (see the SDHC-4b section below for why). So
/// "is the global slot holding the canonical backend?" is asked here as its two-part equivalent:
///
///   1. **Is there a canonical backend outside the global slot at all?** `sdhc_info().is_some()` — a card
///      that `register_sdhc` accepted after its read witnesses ran. If not, nothing can be substituted
///      away from and this is `true`, exactly as the pre-FRGUARD constant was.
///   2. **Did the global slot's occupant arrive as a HOT-PLUG?** If it was found by the boot port scan it
///      IS the boot volume and its writes are the rightful ones (Boots AA..AI-1). If it arrived later, on
///      a live connect-change edge, on a machine that already had a canonical volume it never claimed,
///      then a `Default` WRITE is a substitution of one physical medium for another — Boot AI-2, where
///      the recorder created a 262 656-byte `/UNAOS.LOG` on the operator's card at 312070 ms.
///
/// Deliberately WRITES ONLY, exactly as the aarch64 arm: reads still fall through to the BOT path, because
/// a read cannot corrupt the wrong device, and the stick keeps its dedicated `read_block_usb` handle.
///
/// Byte-inert on every boot with no internal card, and on every boot whose boot volume claimed the global
/// during the port scan — i.e. it returns `true` without touching the console on the entire AA..AI-1 series.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
pub fn default_writable() -> bool {
    if sdhc_info().is_none() {
        return true; // no canonical backend outside the global slot — nothing to substitute away from
    }
    !GLOBAL_CLAIM_HOTPLUG.load(core::sync::atomic::Ordering::Acquire)
}

/// USBFALL F1: targets without ANY canonical backend outside the global slot (x86 without `sdhcblk`,
/// QEMU-virt aarch64) have no substitution to refuse — the enumerated device IS the canonical `Default`
/// backend, so `Default` writes are available whenever the block layer has one at all. Constant `true`
/// keeps `write_block` and every `read_only()` consumer byte-identical to pre-USBFALL there.
#[cfg(not(any(
    all(target_arch = "aarch64", feature = "baremetal"),
    all(target_arch = "x86_64", feature = "sdhcblk")
)))]
pub fn default_writable() -> bool {
    true
}

/// FRGUARD (GR21): one-shot latch for the x86 refusal witness — same shape as `SUBST_REFUSED`, so a
/// retrying writer names the refusal once rather than per sector.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
static X86_SUBST_REFUSED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// FRGUARD (GR21): the x86 twin of [`guard_default_write_backend`]. Fails CLOSED with `NotReady` and one
/// falsifiable line naming both media, so a capture can tell "the guard fired" from "nothing tried to
/// write". The witness prints the two geometries because that is what identified the substitution in the
/// Boot AI-2 forensics: `blocks=124735488` internal vs `dev_blocks=60800` in the global slot.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
fn guard_default_write_backend() -> Result<(), BlockError> {
    if default_writable() {
        return Ok(());
    }
    if !X86_SUBST_REFUSED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        let canon = sdhc_info().map(|d| d.num_blocks).unwrap_or(0);
        let glob = info().map(|d| d.num_blocks).unwrap_or(0);
        serial_println!(
            ":: BLOCK: Default WRITE refused — canonical backend is Sdhc (blocks={}), global slot is a \
             HOT-PLUGGED device (blocks={}); refusing to write the boot volume's data to somebody else's \
             medium (first, once) ::",
            canon, glob
        );
    }
    Err(BlockError::NotReady)
}

#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn guard_default_write_backend() -> Result<(), BlockError> {
    if default_writable() {
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
    // WEDGE-10 (F2): this arm carries the same masked-instant-`Busy` / unmasked-bounded-wait split as
    // `claim_xhci_for_io` below — implemented inside `emmc2::claim_for_io`, one level down, because the
    // INSTALL-PI target calls `read_block_512`/`write_block_512` directly and must get the policy too.
    // `Busy` therefore propagates from here exactly as it does from the xHCI arm, and `fat.rs` retries
    // it outside its masked span.
    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    if BACKEND.load(Ordering::Acquire) == BACKEND_SD {
        return crate::drivers::emmc2::read_block_512(lba, buf);
    }

    // WEDGE-8 (F3): a claimed LOAN, not a held lock — the BOT pump below runs with no lock held.
    let mut xhci = claim_xhci_for_io()?;

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
    // USBFALL F1: fail CLOSED rather than substituting the USB stick for a missing SD card. See
    // `guard_default_write_backend` — compiled only where SD is the canonical backend (Pi bare-metal).
    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    guard_default_write_backend()?;
    // FRGUARD (GR21): the same refusal on x86 + `sdhcblk`, keyed on claim origin instead of the BACKEND
    // selector. Not compiled at all without the knob, so the plain x86 write path is untouched.
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    guard_default_write_backend()?;

    let dev = info().ok_or(BlockError::NotReady)?;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
    }

    // U9: the SD backend now services in-place block WRITES (polled CMD24), routing to the EMMC2/SDHCI
    // driver just as reads route to `read_block_512`. write_block_512 re-guards the lba against its own
    // num_blocks. The xHCI body below is unchanged (and the only path an x86 build compiles).
    // WEDGE-10 (F2): see the `read_block` arm — `emmc2::claim_for_io` applies the masked-instant-`Busy`
    // split under this call, and the ~1.3 s CMD24 + programming-busy + CMD13 ladder now runs on a
    // claimed loan with no driver lock held.
    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    if BACKEND.load(Ordering::Acquire) == BACKEND_SD {
        return crate::drivers::emmc2::write_block_512(lba, buf);
    }

    // WEDGE-8 (F3): a claimed LOAN, not a held lock — the BOT pump below runs with no lock held.
    let mut xhci = claim_xhci_for_io()?;

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

// ===================== MULTIBLK — counted (multi-block) transfers =====================
//
// Everything above this line moves exactly one sector per USB round trip, because that is all the
// xHCI SCSI staging buffer could hold. usb_xhci.md §12.1 priced what that costs: a single flight-
// recorder reservation is ~730 BOT transactions and ~1460 awaited completion events, and since each
// awaited event is an independent chance to hit the unexplained lost-completion wedge (M2), the
// amplification (M1) is what converts a low per-transaction hazard into a boot-killing certainty.
//
// The entry points below carry a real block COUNT. They are additive on purpose: `read_block` /
// `write_block` / `read_block_usb` / `write_block_usb` are left byte-identical, so every existing
// caller — including the installer engine's verify ladder and the shell's raw `write <lba> <byte>` —
// keeps the exact code path it was audited on. Nothing is rerouted; `fs/fat.rs` opts in where it can
// prove the extent is contiguous.
//
// ### The contract, and why it refuses rather than truncates
// `buf.len()` must be a non-zero exact multiple of the device block size, and at most
// [`MAX_BLOCKS_PER_OP`] blocks. Every violation is an error. A block layer that quietly served a
// prefix of the request would hand a filesystem a successful-looking short write, which is precisely
// the failure a filesystem has no way to notice — so the refusal is the safe direction and the only
// honest one. Callers that want more than the cap chunk against `MAX_BLOCKS_PER_OP`, which is
// published for exactly that purpose.
//
// ### aarch64 impact — deliberately behaviour-inert on the microSD path
// The SD backend (`drivers::emmc2`, Pi bare-metal) exposes only the single-block CMD17/CMD24
// primitives, so `read_blocks`/`write_blocks` LOOP those, sector by sector: byte-for-byte the same
// card traffic the per-sector callers produced before this arc, in the same order. The aarch64 seat
// therefore inherits no risk and no win from this change today. The win is available to it whenever
// it wants to spend the arc: implementing CMD18 (READ_MULTIPLE_BLOCK) / CMD25 (WRITE_MULTIPLE_BLOCK)
// behind these two functions lifts the whole `fs/fat.rs` coalescing layer onto the microSD with no
// filesystem change at all, because the coalescing lives above this seam, not below it.

/// MULTIBLK: validate a counted request against one device's geometry, returning the block count.
/// Shared by all four counted entry points so the bound rules cannot drift between them.
fn span_blocks(dev: &BlockDeviceInfo, lba: u64, len: usize) -> Result<usize, BlockError> {
    let bs = dev.block_size as usize;
    if bs == 0 || len == 0 || len % bs != 0 {
        return Err(BlockError::Io); // not a whole number of blocks — a caller bug, not an I/O fault
    }
    let count = len / bs;
    if count > MAX_BLOCKS_PER_OP {
        return Err(BlockError::Io); // larger than the staging buffer; chunk against MAX_BLOCKS_PER_OP
    }
    let end = lba.checked_add(count as u64).ok_or(BlockError::BadLba)?;
    if end > dev.num_blocks {
        return Err(BlockError::BadLba); // the LAST block must be in range, not just the first
    }
    Ok(count)
}

/// MULTIBLK: read `buf.len() / block_size` consecutive blocks starting at `lba` into `buf`, in ONE
/// transfer where the backend supports it. Returns the number of bytes read (always `buf.len()` on
/// success — a partial read is an error, see the contract note above).
pub fn read_blocks(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let dev = info().ok_or(BlockError::NotReady)?;
    let count = span_blocks(&dev, lba, buf.len())?;

    // SD backend: no multi-block primitive yet — loop the proven single-block CMD17. Identical card
    // traffic to the pre-MULTIBLK per-sector callers; see the aarch64 note above.
    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    if BACKEND.load(Ordering::Acquire) == BACKEND_SD {
        let bs = dev.block_size as usize;
        for i in 0..count {
            crate::drivers::emmc2::read_block_512(lba + i as u64, &mut buf[i * bs..(i + 1) * bs])?;
        }
        return Ok(buf.len());
    }

    // WEDGE-8: claim the controller as a LOAN — no lock is held across the BOT transaction below.
    // `claim_xhci_for_io` returns `Busy` immediately to a masked caller (a masked wait on a driver
    // lock IS the F3 deadlock) and otherwise waits its own bounded, unmasked budget.
    let mut loan = claim_xhci_for_io()?;
    let xhci = &mut *loan;
    match xhci.storage_read10(lba as u32, count as u16) {
        Ok(res) if res.status == CswStatus::Passed => {}
        other => {
            io_cause_witness("read", lba, other);
            return Err(BlockError::Io);
        }
    }
    let src = xhci.storage_data_ptr().ok_or(BlockError::Io)?;
    unsafe { core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), buf.len()); }
    Ok(buf.len())
}

/// MULTIBLK: write `buf.len() / block_size` consecutive blocks starting at `lba` from `buf`, in ONE
/// transfer where the backend supports it. `buf` covers the whole span, so — unlike the single-block
/// path's callers — there is nothing to read first: this is the point where a full-sector overwrite
/// stops costing a READ(10) it never used.
pub fn write_blocks(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    // FRGUARD (GR21): the counted path needs the guard as much as the single-block one — `fs/fat.rs`'s
    // `write_sectors` routes here, and that is the entry point the flight recorder's `write_grow` /
    // `write_at` actually use. Guarding only `write_block` (as the aarch64 arm does today) would have
    // left the Boot AI-2 write path wide open. Deliberately NOT added to the aarch64 arm in this arc:
    // that gap is real but it is the Pi lane's to close, and this arc's contract is that aarch64 makes
    // byte-identical decisions.
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    guard_default_write_backend()?;

    let dev = info().ok_or(BlockError::NotReady)?;
    let count = span_blocks(&dev, lba, buf.len())?;

    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    if BACKEND.load(Ordering::Acquire) == BACKEND_SD {
        let bs = dev.block_size as usize;
        for i in 0..count {
            crate::drivers::emmc2::write_block_512(lba + i as u64, &buf[i * bs..(i + 1) * bs])?;
        }
        return Ok(());
    }

    // WEDGE-8: claim the controller as a LOAN — no lock is held across the BOT transaction below.
    // `claim_xhci_for_io` returns `Busy` immediately to a masked caller (a masked wait on a driver
    // lock IS the F3 deadlock) and otherwise waits its own bounded, unmasked budget.
    let mut loan = claim_xhci_for_io()?;
    let xhci = &mut *loan;
    let dst = xhci.storage_data_ptr().ok_or(BlockError::Io)?;
    unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, buf.len()); }
    match xhci.storage_write10(lba as u32, count as u16) {
        Ok(res) if res.status == CswStatus::Passed => Ok(()),
        other => {
            io_cause_witness("write", lba, other);
            Err(BlockError::Io)
        }
    }
}

/// MULTIBLK: the counted twin of [`read_block_usb`] — reads the USB stick DIRECTLY through the xHCI
/// controller, bypassing the backend selector, so a `Usb`-sourced FAT mount coalesces on the Pi too
/// (where the global block device is the microSD). Strictly read-only, as that path has always been.
pub fn read_blocks_usb(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let dev = usb_info().ok_or(BlockError::NotReady)?;
    let count = span_blocks(&dev, lba, buf.len())?;
    // WEDGE-8: claim the controller as a LOAN — no lock is held across the BOT transaction below.
    // `claim_xhci_for_io` returns `Busy` immediately to a masked caller (a masked wait on a driver
    // lock IS the F3 deadlock) and otherwise waits its own bounded, unmasked budget.
    let mut loan = claim_xhci_for_io()?;
    let xhci = &mut *loan;
    match xhci.storage_read10(lba as u32, count as u16) {
        Ok(res) if res.status == CswStatus::Passed => {}
        other => {
            io_cause_witness("read-usb", lba, other);
            return Err(BlockError::Io);
        }
    }
    let src = xhci.storage_data_ptr().ok_or(BlockError::Io)?;
    unsafe { core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), buf.len()); }
    Ok(buf.len())
}

/// MULTIBLK: the counted twin of [`write_block_usb`]. Same guard ladder as the single-block form —
/// geometry re-read on every call, span bounded against `num_blocks`, a non-`Passed` CSW propagated
/// as [`BlockError::Io`] so a write NEVER reports a false success.
pub fn write_blocks_usb(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let dev = usb_info().ok_or(BlockError::NotReady)?;
    let count = span_blocks(&dev, lba, buf.len())?;
    // WEDGE-8: claim the controller as a LOAN — no lock is held across the BOT transaction below.
    // `claim_xhci_for_io` returns `Busy` immediately to a masked caller (a masked wait on a driver
    // lock IS the F3 deadlock) and otherwise waits its own bounded, unmasked budget.
    let mut loan = claim_xhci_for_io()?;
    let xhci = &mut *loan;
    let dst = xhci.storage_data_ptr().ok_or(BlockError::Io)?;
    unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, buf.len()); }
    match xhci.storage_write10(lba as u32, count as u16) {
        Ok(res) if res.status == CswStatus::Passed => Ok(()),
        other => {
            io_cause_witness("write-usb", lba, other);
            Err(BlockError::Io)
        }
    }
}

// ===================== SDHC-4b — the internal SD card as a THIRD registry handle =====================
//
// Boot AC identified the card in the 2012 rMBP's built-in PCIe SD reader (`drivers/sdhc.rs`) and read
// it three ways; Boot AD wrote one sector to it and restored it byte-for-byte. The driver therefore
// has proven `read_block_512` / `read_blocks_512` (and, behind `sdw`, `write_block_512`) — but nothing
// above the driver could reach them, because the block layer had exactly two backends and every SD arm
// in it is `#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]`, i.e. the Pi's EMMC2. On x86
// `block::read_block` has always meant the USB stick and nothing else. This section is the seam that
// lets FAT mount the internal card, and every decision in it is about NOT disturbing that stick.
//
// ### Why a third HANDLE and not a third value of `BACKEND`
// Widening the selector was the obvious shape and it is the wrong one, for three independent reasons:
//   1. **It would steal the boot volume.** On x86 the machine boots from a USB card reader, so the
//      stick IS `BLOCK_DEVICE` and `block::info()` is what the flight recorder, `fs/fat.rs`'s
//      `probe_once`, the shell, `fs/unafs.rs` and the installer all read. Flipping the selector to the
//      internal card would silently re-point every one of them at a different disk — the failure the
//      Pi paid for as PI-FS-2 (a 14 MiB reader's `num_blocks` clobbering the SD's global), in reverse.
//   2. **The publish order makes it a race, not a choice.** `sdhc::probe` runs inside the x86 PCI
//      probe; the xHCI stick enumerates later and `publish_usb_geometry` on this target claims the
//      global UNCONDITIONALLY. A card registered into the global would simply be overwritten a second
//      later, so the "selector" would decide nothing and would hide that it decided nothing.
//   3. **Refusal beats precedence.** With a separate handle there is no precedence rule to get wrong:
//      a caller that wants the internal card must NAME it. Nothing in the tree names it today except
//      the read-only mount witness added by this arc, so every existing caller is untouched by
//      construction rather than by a policy that has to keep being right.
// The `Usb` handle is the proven precedent for exactly this (PIUSB-27 added it so the Pi could reach
// the stick while the microSD owned the global), and this section mirrors it line for line.
//
// ### x86 `read_block` / `write_block` are not merely unchanged — they are untouched
// There is no new statement anywhere in `read_block`, `write_block`, `read_blocks`, `write_blocks` or
// `publish_usb_geometry`. Not one `#[cfg]`, not one branch. The whole SDHC path is additive entry
// points plus one extra arm in the handle/source matches, and with the knob off the variant does not
// exist and those matches are byte-identical to today's.
//
// ### SINGLE FAT WRITER (see `flight_recorder.rs`) — what this arc does and does not do about it
// `fs/fat.rs`'s `with_fat_lock` / `with_dir_lock` are deliberately INERT on x86, so on this target the
// tree's rule is "at most one FAT/directory MUTATOR". That rule was paid for in A/B-proven cross-linked
// chains and stolen delete-witness snapshots, and it is why the flight recorder reserves `UNAOS.LOG`
// once and thereafter only writes IN PLACE.
//
// This arc therefore adds a READER, not a writer:
//   * the SDHC volume is mounted as a SEPARATE, by-value `FatFs` (`fs::fat::BlockSource::Sdhc`) with no
//     state shared with the boot volume's mount — the `FatFs` is a plain value, and `ALLOC_HINT` (the
//     only cross-volume global in `fs/fat.rs`) is documented advisory-only and is read by the
//     ALLOCATOR, which a read-only mount never enters;
//   * `fs::fat::write_sector` / `write_sectors` REFUSE a `Sdhc` source unconditionally, in every cfg,
//     with a one-shot witness — the same blanket refusal PIUSB-27 shipped for `Usb`, which USB-WRITE
//     later lifted in its own arc with its own witness ladder;
//   * so no FAT entry, no directory sector and no data cluster of the internal card is ever written by
//     this build, and the count of x86 FAT mutators is unchanged at one.
// What would have to be true before anything may write through the FAT layer to this volume: either
// `with_fat_lock`/`with_dir_lock` become REAL on x86 (they cannot mask IRQs across the `hlt`-driven BOT
// pump, so this needs a different mechanism, not a flag flip), or the SDHC writer adopts the recorder's
// reserve-once-then-write-in-place shape, which is not a FAT mutation after bootstrap. Neither is in
// this arc, and until one of them holds, the refusal below is the whole safety argument.
//
// The BLOCK-layer write pair below is a different question and is answered differently: it is
// `sdw`-gated (so a default image contains no CMD24 command word at all — SDHC-4a's property is
// preserved), it is reachable only through `PartitionRange::write_block(s)` with an explicitly
// `Sdhc`-handled range, and `fs/fat.rs` never calls `PartitionRange::write_block` (verified: the FAT
// writers go straight to `write_sector`/`write_sectors`). It exists so the primitive is type-checked
// through the block seam and available to the next arc, not so that anything in this one uses it.

/// SDHC-4b: geometry of the card in the machine's internal SD slot, published by [`register_sdhc`]
/// once `sdhc::bring_up` has identified it AND its read witnesses have run. Deliberately a THIRD slot
/// alongside [`BLOCK_DEVICE`] and [`USB_BLOCK_DEVICE`]: nothing here ever claims the global, so the
/// boot volume every existing caller reads through `info()` is untouched. `None` until registration.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
pub static SDHC_BLOCK_DEVICE: Mutex<Option<BlockDeviceInfo>> = Mutex::new(None);

/// SDHC-4b: snapshot of the internal SD card's geometry, if one registered.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
pub fn sdhc_info() -> Option<BlockDeviceInfo> {
    *SDHC_BLOCK_DEVICE.lock()
}

/// SDHC-4b: publish the internal SD card's geometry under the [`SDHC_BLOCK_DEVICE`] handle.
///
/// Called once, from `sdhc::bring_up`, AFTER the read witnesses (`verify_read`, `verify_multiblock`,
/// `adma2_smoke`, `verify_adma_ab`) have run — so a registered card is one whose read path has already
/// been exercised and reported on this boot, not merely one that answered CMD2/CMD3. That ordering is
/// the point: the registry is the thing FAT trusts, so it should only ever name a card the log has
/// already said something true about.
///
/// Returns whether the card was registered. It refuses a zero-block card rather than publishing a
/// device every subsequent bound check would reject one call later — an instrument that can say NO.
///
/// `slot_id` is 0, the same sentinel `register_sd` stamps on the Pi's microSD: 0 is never a live xHCI
/// device slot, so [`unpublish_usb_geometry`] (which matches on slot id and returns early on 0) can
/// never retract this entry on a USB disconnect. Belt and braces — that function only ever touches the
/// `Global` and `Usb` slots anyway.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
pub fn register_sdhc(num_blocks: u64, block_addressed: bool) -> bool {
    if num_blocks == 0 {
        serial_println!(
            ":: SDHCBLK: REFUSED to register the internal SD card — num_blocks=0 (nothing to address) ::"
        );
        return false;
    }
    let dev = BlockDeviceInfo {
        slot_id: 0,
        block_size: SECTOR_BYTES as u32,
        num_blocks,
        vendor: *b"SD-SLOT ",
        product: if block_addressed { *b"INTERNAL SDHC/XC" } else { *b"INTERNAL SDSC   " },
    };
    *SDHC_BLOCK_DEVICE.lock() = Some(dev);
    let mib = num_blocks.saturating_mul(SECTOR_BYTES as u64) / (1024 * 1024);
    serial_println!(
        ":: SDHCBLK: registered internal SD card as block handle Sdhc — blocks={} ({} MiB) addressing={} \
         (global BLOCK_DEVICE untouched) ::",
        num_blocks, mib, if block_addressed { "block" } else { "byte" }
    );
    true
}

/// SDHC-4b: read one block (`lba`) from the internal SD card DIRECTLY through the SDHCI driver,
/// bypassing the backend selector — the read twin of [`read_block_usb`]. Geometry (block size, bound)
/// comes from [`SDHC_BLOCK_DEVICE`], re-read on every call as every other entry point does, and
/// `sdhc::read_block_512` re-guards the LBA against the card's own `num_blocks` one layer down.
/// Takes no xHCI lock at all, so it cannot interact with the boot volume's transport.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
pub fn read_block_sdhc(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let dev = sdhc_info().ok_or(BlockError::NotReady)?;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
    }
    crate::drivers::sdhc::read_block_512(lba, buf)
}

/// SDHC-4b: the counted twin of [`read_block_sdhc`] — `buf.len() / block_size` consecutive blocks in
/// one call. Bounded by the shared [`span_blocks`] rules so they cannot drift between handles.
///
/// The cap those rules apply is [`MAX_BLOCKS_PER_OP`], which is the xHCI staging buffer's capacity and
/// has nothing to do with the SDHCI path's own limit (`sdhc::MB_MAX_BLOCKS`, which `read_blocks_512`
/// chunks against internally). Using the published block-layer cap here is deliberate and conservative:
/// it is the number `fs/fat.rs` already chunks every source against, it is never larger than what the
/// SD path can serve, and one cap for all handles is one rule to get right instead of three.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
pub fn read_blocks_sdhc(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let dev = sdhc_info().ok_or(BlockError::NotReady)?;
    let count = span_blocks(&dev, lba, buf.len())?;
    let n = crate::drivers::sdhc::read_blocks_512(lba, count, buf)?;
    // A short counted read is an error, never a silent prefix — the same rule the xHCI counted forms
    // enforce, and the one failure mode a filesystem above has no way to notice.
    if n < buf.len() {
        return Err(BlockError::Io);
    }
    Ok(buf.len())
}

/// SDHC-4b: one-shot latch so the refusal below names itself once instead of per retry.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", not(feature = "sdw")))]
static SDHC_WRITE_REFUSED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// SDHC-4b: write one block (`lba`) to the internal SD card through the SDHCI driver's polled
/// single-block CMD24 (`sdhc::write_block_512`, SDHC-4a — metal-proven on Boot AD: `verify=IDENTICAL
/// restore=IDENTICAL -> PASS`). The driver keeps its own three refusals (WP switch read live, CSD
/// PERM_WRITE_PROTECT, CSD TMP_WRITE_PROTECT), so this entry point inherits them rather than
/// re-implementing them.
///
/// **Not reachable from the FAT layer in this arc.** `fs::fat::write_sector`/`write_sectors` refuse a
/// `Sdhc` source unconditionally (see the SINGLE FAT WRITER note above); the only route here is
/// [`PartitionRange::write_block`] with an explicitly `Sdhc`-handled range, which nothing in `fs/fat.rs`
/// calls. It is compiled and type-checked so the seam exists and the `sdw` polarity is covered by the
/// `x86-all` gate leg, not so that this build writes to the card.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", feature = "sdw"))]
pub fn write_block_sdhc(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let dev = sdhc_info().ok_or(BlockError::NotReady)?;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
    }
    crate::drivers::sdhc::write_block_512(lba, buf)
}

/// SDHC-4b: without `sdw` there is no CMD24 command word in this image at all (SDHC-4a's property),
/// so the honest answer is a refusal that says why — not a silent `Ok`, and not a call that could not
/// link. Fails CLOSED, in the same direction as USBFALL F1's `guard_default_write_backend`.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", not(feature = "sdw")))]
pub fn write_block_sdhc(_lba: u64, _buf: &[u8]) -> Result<(), BlockError> {
    if !SDHC_WRITE_REFUSED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        serial_println!(
            ":: SDHCBLK: WRITE refused — this build carries no `sdw` feature, so it contains no CMD24 \
             ladder for the internal SD card (first, once) ::"
        );
    }
    Err(BlockError::NotReady)
}

/// SDHC-4b: the counted twin of [`write_block_sdhc`]. The SDHCI driver exposes only the single-block
/// CMD24 primitive, so this LOOPS it, sector by sector — byte-for-byte the same card traffic a
/// per-sector caller produces, in the same order (exactly the shape `read_blocks`/`write_blocks` use
/// for the Pi's `emmc2`). CMD25 (WRITE_MULTIPLE_BLOCK) would lift this with no change above the seam.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
pub fn write_blocks_sdhc(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let dev = sdhc_info().ok_or(BlockError::NotReady)?;
    let count = span_blocks(&dev, lba, buf.len())?;
    let bs = dev.block_size as usize;
    for i in 0..count {
        write_block_sdhc(lba + i as u64, &buf[i * bs..(i + 1) * bs])?;
    }
    Ok(())
}

// ===================== PARTITION — MBR decode + bounded partition ranges =====================
//
// PARTITION (GR9): everything above this line addresses a WHOLE device. A real card carries a
// partition table, and the UnaOS layout of record is MBR partition 1 = ESP (FAT) + partition 2 =
// UnaFS (the layout `make-pi-img.sh` already stamps and `locate_unafs` already finds by superblock
// magic). This section is the missing middle: ONE decoder for the classic MBR, and ONE bounded
// window type ([`PartitionRange`]) that the layers above address a partition through.
//
// ### Every field here comes off the medium — nothing guarantees it
// LBA 0 of an arbitrary disk is attacker-shaped input: an unformatted stick, a foreign OS's table,
// or a deliberately crafted sector. NOTHING about a partition entry is guaranteed by the medium —
// not the type byte, not the start LBA, not the length, not that two entries are disjoint. The only
// thing the hardware guarantees is that we read 512 bytes from LBA 0 (and the block layer's own
// `num_blocks` guard guarantees that read was in range). So the decoder treats every field as a
// claim to be checked, and a claim that fails is REJECTED — never clamped, never repaired, never
// trusted "because it is probably fine". [`decode_mbr`] documents each rule at its check.
//
// ### Reading, not writing
// This arc reads and bounds. It writes NO partition table: laying an MBR down is a medium-destroying
// operation and belongs to its own arc behind its own approval. The installer's existing GPT writer
// (`install::gpt`) is untouched.

/// Bytes in one logical sector on every device this layer supports (the block layer already refuses
/// a device whose `block_size` is anything else at mount time).
pub const SECTOR_BYTES: usize = 512;

/// Byte offset of the 0x55AA boot signature inside LBA 0.
const MBR_SIG_OFF: usize = 510;
/// Byte offset of the first of the four 16-byte primary partition entries inside LBA 0.
const MBR_TABLE_OFF: usize = 446;
/// Size of one MBR primary partition entry.
const MBR_ENTRY_LEN: usize = 16;

/// MBR type byte reserved by UEFI for the protective partition of a GPT disk.
pub const MBR_TYPE_GPT_PROTECTIVE: u8 = 0xEE;
/// MBR type bytes for extended-partition containers (CHS / LBA forms). We do not walk EBR chains.
pub const MBR_TYPE_EXTENDED_CHS: u8 = 0x05;
/// See [`MBR_TYPE_EXTENDED_CHS`].
pub const MBR_TYPE_EXTENDED_LBA: u8 = 0x0F;
/// The MBR type byte the UnaOS card layout stamps on its UnaFS data partition (partition 2).
/// Advisory ONLY: [`crate::fs::unafs`] locates the volume by its `UNAFS` superblock magic, never by
/// this byte, so a card carved by a foreign tool still works. It exists so a census line reads
/// meaningfully to a human.
pub const MBR_TYPE_UNAFS: u8 = 0x7F;

/// Why [`decode_mbr`] refused one primary entry. Every variant is a claim the entry made that the
/// medium does not back; the entry is dropped, never clamped into range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionReject {
    /// Type byte 0x00 — an unused slot. The only "reject" that is not a defect.
    Empty,
    /// Length field is 0: the entry addresses nothing, so it can only ever mislead.
    ZeroLength,
    /// Start LBA 0: the entry claims the partition table's own sector. A partition that contains
    /// LBA 0 could rewrite the table that describes it, and it aliases the superfloppy case.
    StartsAtZero,
    /// `start + count` overflows u64 — a crafted extent that would wrap back into low LBAs.
    ExtentOverflow,
    /// The extent's LAST sector is past the end of the device. Accepting it would let a filesystem
    /// address sectors the medium does not have (and, on a wrapping controller, sectors it does).
    PastDevice,
    /// The extent intersects an earlier ACCEPTED entry. Two filesystems sharing sectors is a
    /// corruption generator: each would consider the overlap its own to write.
    Overlap,
    /// An extended-partition container (0x05 / 0x0F). We do not walk EBR chains, so the container
    /// itself is not an addressable volume.
    Extended,
    /// A GPT protective entry (0xEE) was present, so this disk's real table is the GPT at LBA 1 and
    /// the MBR describes nothing. See [`MbrTable::protective`].
    Protective,
}

/// One accepted MBR primary partition, normalized to LBAs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionEntry {
    /// 1..=4 — the slot number as a human names it ("partition 1 = ESP"). Slot order is table order.
    pub slot: u8,
    /// The 0x80 boot-indicator byte, verbatim. Advisory; nothing in UnaOS keys off it.
    pub boot_flag: u8,
    /// The partition type byte, verbatim. Advisory (see [`MBR_TYPE_UNAFS`]).
    pub type_byte: u8,
    /// First sector of the partition, absolute on the device.
    pub start_lba: u64,
    /// Length of the partition in sectors. `start_lba + sector_count` is checked `<= num_blocks`.
    pub sector_count: u64,
}

impl PartitionEntry {
    /// One past the last sector of this partition (always computable: `decode_mbr` already proved
    /// the sum does not overflow and does not exceed the device).
    pub fn end_lba(&self) -> u64 {
        self.start_lba + self.sector_count
    }
}

/// A decoded classic MBR partition table. Produced only from a sector that carried 0x55AA.
#[derive(Debug, Clone, Copy)]
pub struct MbrTable {
    /// True when a 0xEE protective entry was present. The disk's real table is then the GPT at
    /// LBA 1 and this table exposes NO partitions: a hybrid MBR (real entries alongside 0xEE) is
    /// exactly the ambiguity an attacker wants, and resolving it in favour of "some entries are
    /// real" would let a crafted MBR entry shadow a GPT partition. GPT disks go through the GPT path.
    pub protective: bool,
    /// Slot 1..=4 in table order; `None` where the entry was rejected.
    entries: [Option<PartitionEntry>; 4],
    /// Why each slot was rejected (`None` for an accepted slot).
    rejects: [Option<PartitionReject>; 4],
}

impl MbrTable {
    /// The entry in `slot` (1..=4), if it was accepted.
    pub fn entry(&self, slot: u8) -> Option<PartitionEntry> {
        if slot == 0 || slot > 4 {
            return None;
        }
        self.entries[(slot - 1) as usize]
    }

    /// Why `slot` (1..=4) was rejected, if it was.
    pub fn reject(&self, slot: u8) -> Option<PartitionReject> {
        if slot == 0 || slot > 4 {
            return None;
        }
        self.rejects[(slot - 1) as usize]
    }

    /// Every accepted entry, in slot order.
    pub fn iter(&self) -> impl Iterator<Item = PartitionEntry> + '_ {
        self.entries.iter().filter_map(|e| *e)
    }

    /// How many slots were accepted.
    ///
    /// INSTRUMENT NOTE (healthy-but-idle): on the UnaOS layout of record — MBR p1 ESP + p2 UnaFS —
    /// this reads **2** on every boot, from the very first read, and never changes: the table is a
    /// property of the medium, not of any activity. A value that is not 2 on a UnaOS card is a real
    /// finding about the medium, not a sampling artifact. On a GPT disk it reads 0 (`protective`).
    pub fn accepted(&self) -> u8 {
        self.entries.iter().filter(|e| e.is_some()).count() as u8
    }

    /// How many slots were rejected for a reason OTHER than being an empty slot.
    ///
    /// INSTRUMENT NOTE (healthy-but-idle): on the UnaOS layout this reads **0** on every boot —
    /// slots 3 and 4 are empty (`PartitionReject::Empty`, which this deliberately does not count),
    /// and slots 1 and 2 are accepted. A non-zero value means the table itself carried an entry we
    /// refused; the per-slot census line names which rule it broke. There is no idle path that can
    /// make this drift, because nothing but a re-read of LBA 0 can change it.
    pub fn rejected(&self) -> u8 {
        self.rejects
            .iter()
            .filter(|r| matches!(r, Some(x) if *x != PartitionReject::Empty))
            .count() as u8
    }
}

#[inline]
fn le_u32(b: &[u8], off: usize) -> u32 {
    (b[off] as u32) | ((b[off + 1] as u32) << 8) | ((b[off + 2] as u32) << 16) | ((b[off + 3] as u32) << 24)
}

/// Decode a classic MBR partition table out of `sec` (which must be LBA 0 of a device holding
/// `dev_blocks` sectors). Pure: it reads no device and prints nothing, so it is the one place the
/// validation rules live and the one thing a witness or a test can exercise directly.
///
/// Returns `None` when `sec` carries no 0x55AA signature — that is "this is not an MBR", not an
/// error: LBA 0 may legitimately be a FAT superfloppy BPB or blank media, and the caller falls
/// through to its other candidates.
///
/// ### What a malformed table does
/// Nothing is repaired and nothing is clamped. Each slot is judged on its own and a slot that fails
/// any rule is dropped with a [`PartitionReject`] naming the rule. A table where every slot fails
/// decodes successfully to a table with **zero** partitions, which every caller treats as "no
/// partition here" — the safe direction, because a caller can only ever mount FEWER volumes, never
/// a volume in a place the table did not honestly describe. The rules, in order of application:
///
/// 1. type byte 0x00 → `Empty` (an unused slot; not counted as a defect);
/// 2. type byte 0xEE anywhere in the table → the WHOLE table is `protective` and exposes nothing;
/// 3. type byte 0x05 / 0x0F → `Extended` (we do not walk EBR chains);
/// 4. `sector_count == 0` → `ZeroLength`;
/// 5. `start_lba == 0` → `StartsAtZero` (it would contain the table describing it);
/// 6. `start + count` overflowing → `ExtentOverflow`;
/// 7. `start + count > dev_blocks` → `PastDevice`;
/// 8. intersecting an already-ACCEPTED slot → `Overlap`.
///
/// Rule 8 is what makes the accepted set pairwise disjoint, which is in turn what makes a
/// [`PartitionRange`]'s bound check a real isolation guarantee rather than a formality: two
/// overlapping partitions would each pass their own bound check while writing the same sectors.
/// Table order decides the winner so the outcome is deterministic for a given medium.
pub fn decode_mbr(sec: &[u8], dev_blocks: u64) -> Option<MbrTable> {
    if sec.len() < SECTOR_BYTES {
        return None;
    }
    if sec[MBR_SIG_OFF] != 0x55 || sec[MBR_SIG_OFF + 1] != 0xAA {
        return None;
    }

    // Rule 2 first: one 0xEE anywhere makes the entire MBR a protective shell for a GPT.
    let protective = (0..4).any(|i| sec[MBR_TABLE_OFF + i * MBR_ENTRY_LEN + 4] == MBR_TYPE_GPT_PROTECTIVE);

    let mut entries: [Option<PartitionEntry>; 4] = [None; 4];
    let mut rejects: [Option<PartitionReject>; 4] = [None; 4];

    for i in 0..4 {
        let off = MBR_TABLE_OFF + i * MBR_ENTRY_LEN;
        let boot_flag = sec[off];
        let type_byte = sec[off + 4];
        let start_lba = le_u32(sec, off + 8) as u64;
        let sector_count = le_u32(sec, off + 12) as u64;

        let reject = if type_byte == 0x00 {
            Some(PartitionReject::Empty)
        } else if protective {
            Some(PartitionReject::Protective)
        } else if type_byte == MBR_TYPE_EXTENDED_CHS || type_byte == MBR_TYPE_EXTENDED_LBA {
            Some(PartitionReject::Extended)
        } else if sector_count == 0 {
            Some(PartitionReject::ZeroLength)
        } else if start_lba == 0 {
            Some(PartitionReject::StartsAtZero)
        } else {
            match start_lba.checked_add(sector_count) {
                None => Some(PartitionReject::ExtentOverflow),
                Some(end) if end > dev_blocks => Some(PartitionReject::PastDevice),
                Some(end) => {
                    // Rule 8: disjointness against everything already accepted.
                    let clash = entries.iter().flatten().any(|p| {
                        start_lba < p.end_lba() && p.start_lba < end
                    });
                    if clash { Some(PartitionReject::Overlap) } else { None }
                }
            }
        };

        match reject {
            Some(r) => rejects[i] = Some(r),
            None => {
                entries[i] = Some(PartitionEntry {
                    slot: (i + 1) as u8,
                    boot_flag,
                    type_byte,
                    start_lba,
                    sector_count,
                })
            }
        }
    }

    Some(MbrTable { protective, entries, rejects })
}

/// PARTITION: one-shot latch per handle for [`mbr_census`] — bit 0 = `Global`, bit 1 = `Usb`.
///
/// INSTRUMENT NOTE (healthy-but-idle): reads 0 before any FAT mount is attempted and stops changing
/// once each present handle has been censused (1, 2, or 3). It gates PRINTING only; the decode it
/// guards is pure and is re-run on every mount, so a latched census can never make a later mount
/// see a different table than the one it printed.
static MBR_CENSUS_LATCH: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// PARTITION: print the RAW LBA-0 partition area first, then the decoded verdict — at most once per
/// handle per boot. Returns the decoded table (or `None` if `sec` is not an MBR) whether or not it
/// printed, so the caller's control flow does not depend on the latch.
///
/// The raw line comes BEFORE any interpretation on purpose. Every decoded line below it is this
/// code's opinion; the raw line is the medium's bytes, and it is what lets a bench sitting decide,
/// from the serial log alone, whether a surprising verdict came from a surprising disk or from a
/// decoder bug. It is the same discipline the block layer's `io-cause` witness follows: name the
/// evidence before naming the conclusion.
pub fn mbr_census(handle: BlockHandle, sec: &[u8], dev_blocks: u64) -> Option<MbrTable> {
    let table = decode_mbr(sec, dev_blocks);

    let bit: u8 = match handle {
        BlockHandle::Global => 1,
        BlockHandle::Usb => 2,
        // SDHC-4b: its own latch bit, so the internal card's table is censused once on its own terms
        // and can never be suppressed by (or suppress) the boot volume's census.
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        BlockHandle::Sdhc => 4,
    };
    let prev = MBR_CENSUS_LATCH.fetch_or(bit, core::sync::atomic::Ordering::Relaxed);
    if prev & bit != 0 || sec.len() < SECTOR_BYTES {
        return table;
    }
    let name = match handle {
        BlockHandle::Global => "global",
        BlockHandle::Usb => "usb",
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        BlockHandle::Sdhc => "sdhc",
    };

    // --- RAW, before decoding anything: the signature word and the four 16-byte entries verbatim.
    serial_println!(
        ":: PART: mbr-raw handle={} dev_blocks={} sig={:02x}{:02x} ::",
        name, dev_blocks, sec[MBR_SIG_OFF], sec[MBR_SIG_OFF + 1]
    );
    for i in 0..4 {
        let o = MBR_TABLE_OFF + i * MBR_ENTRY_LEN;
        let e = &sec[o..o + MBR_ENTRY_LEN];
        serial_println!(
            ":: PART: mbr-raw handle={} e{} = {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
            name, i + 1,
            e[0], e[1], e[2], e[3], e[4], e[5], e[6], e[7],
            e[8], e[9], e[10], e[11], e[12], e[13], e[14], e[15]
        );
    }

    // --- Decoded verdict, per slot, then the summary.
    let Some(t) = table else {
        serial_println!(":: PART: mbr census handle={} sig=absent — not an MBR ::", name);
        return table;
    };
    for slot in 1..=4u8 {
        match (t.entry(slot), t.reject(slot)) {
            (Some(p), _) => serial_println!(
                ":: PART: mbr handle={} slot={} type=0x{:02x} boot=0x{:02x} start={} count={} end={} ACCEPT ::",
                name, slot, p.type_byte, p.boot_flag, p.start_lba, p.sector_count, p.end_lba()
            ),
            (None, Some(r)) => serial_println!(
                ":: PART: mbr handle={} slot={} REJECT {:?} ::", name, slot, r
            ),
            (None, None) => {}
        }
    }
    serial_println!(
        ":: PART: mbr census handle={} protective={} accepted={} rejected={} ::",
        name, t.protective as u8, t.accepted(), t.rejected()
    );
    table
}

/// PARTITION: a bounded, addressable window onto ONE partition.
///
/// This is the type the layers above address a partition through. Its whole reason to exist is the
/// bound: every entry point takes a **partition-relative** LBA and refuses anything whose span does
/// not lie entirely inside `[0, sector_count)` BEFORE translating it to an absolute LBA. A caller
/// therefore cannot express an access outside its own partition — not by arithmetic error, not by a
/// corrupt on-disk field, not by a hostile one. That is the property a filesystem mounted on
/// partition 1 needs in order to be incapable of writing into partition 2.
///
/// ### Two independent bounds, deliberately
/// 1. HERE, against the partition's own extent — the isolation guarantee.
/// 2. Below, in [`read_block`] / [`write_block`] (and the counted forms), against the device's
///    `num_blocks` — the medium guarantee, re-read from the live registry on every call.
///
/// Neither is redundant: (1) cannot see a disk that shrank or was unplugged, and (2) cannot see a
/// partition boundary. A range is a plain value copied from a decoded entry, so it does not keep a
/// device alive; if the disk goes away, (2) fails the next access with `NotReady`, exactly as every
/// other block caller does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionRange {
    /// Which registry handle (and therefore which read/write path) this partition lives behind.
    pub handle: BlockHandle,
    /// Absolute LBA of partition-relative sector 0.
    pub start_lba: u64,
    /// Length of the partition in sectors — the bound every access is checked against.
    pub sector_count: u64,
}

impl PartitionRange {
    /// Build a range for `entry` as read from `handle`.
    pub fn new(handle: BlockHandle, entry: &PartitionEntry) -> Self {
        Self { handle, start_lba: entry.start_lba, sector_count: entry.sector_count }
    }

    /// Translate a partition-relative span to an absolute LBA, or refuse.
    ///
    /// `sectors` is the WHOLE span, so the check covers the last sector, not merely the first — the
    /// same rule `span_blocks` applies at the device level, and for the same reason: a span whose
    /// head is in range and whose tail is not is precisely the shape of a cross-partition write.
    fn map(&self, rel_lba: u64, sectors: u64) -> Result<u64, BlockError> {
        if sectors == 0 {
            return Err(BlockError::Io); // a caller bug, not an I/O fault
        }
        let end = rel_lba.checked_add(sectors).ok_or(BlockError::BadLba)?;
        if end > self.sector_count {
            return Err(BlockError::BadLba);
        }
        self.start_lba.checked_add(rel_lba).ok_or(BlockError::BadLba)
    }

    /// Absolute LBA of partition-relative `rel_lba`, bound-checked. Exposed so a filesystem that
    /// must speak absolute LBAs (the in-tree FAT reader does) can still get every address through
    /// this one gate instead of computing `start + rel` for itself.
    pub fn absolute(&self, rel_lba: u64, sectors: u64) -> Result<u64, BlockError> {
        self.map(rel_lba, sectors)
    }

    /// Does the ABSOLUTE span `[lba, lba+sectors)` lie entirely inside this partition? The check a
    /// caller holding absolute addresses uses before handing them to the device layer.
    pub fn contains_absolute(&self, lba: u64, sectors: u64) -> bool {
        let Some(end) = lba.checked_add(sectors) else { return false };
        let Some(part_end) = self.start_lba.checked_add(self.sector_count) else { return false };
        sectors != 0 && lba >= self.start_lba && end <= part_end
    }

    /// Read one partition-relative sector.
    pub fn read_block(&self, rel_lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
        let abs = self.map(rel_lba, 1)?;
        match self.handle {
            BlockHandle::Global => read_block(abs, buf),
            BlockHandle::Usb => read_block_usb(abs, buf),
            #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
            BlockHandle::Sdhc => read_block_sdhc(abs, buf),
        }
    }

    /// Write one partition-relative sector.
    pub fn write_block(&self, rel_lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        let abs = self.map(rel_lba, 1)?;
        match self.handle {
            BlockHandle::Global => write_block(abs, buf),
            BlockHandle::Usb => write_block_usb(abs, buf),
            // SDHC-4b: the ONLY route to the internal card's write primitive in this arc. `fs/fat.rs`
            // does not call `PartitionRange::write_block` (its writers go straight to `write_sector`),
            // so no FAT mutation can arrive here; a deliberate caller holding an `Sdhc` range can.
            #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
            BlockHandle::Sdhc => write_block_sdhc(abs, buf),
        }
    }

    /// Read a counted run of partition-relative sectors (`buf.len()` must be a whole multiple of the
    /// device block size; the device layer re-checks that and the `MAX_BLOCKS_PER_OP` cap).
    pub fn read_blocks(&self, rel_lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
        let abs = self.map(rel_lba, self.span_sectors(buf.len())?)?;
        match self.handle {
            BlockHandle::Global => read_blocks(abs, buf),
            BlockHandle::Usb => read_blocks_usb(abs, buf),
            #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
            BlockHandle::Sdhc => read_blocks_sdhc(abs, buf),
        }
    }

    /// Write a counted run of partition-relative sectors.
    pub fn write_blocks(&self, rel_lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        let abs = self.map(rel_lba, self.span_sectors(buf.len())?)?;
        match self.handle {
            BlockHandle::Global => write_blocks(abs, buf),
            BlockHandle::Usb => write_blocks_usb(abs, buf),
            #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
            BlockHandle::Sdhc => write_blocks_sdhc(abs, buf),
        }
    }

    /// Sectors covered by a `len`-byte buffer, using the LIVE device block size for the handle so
    /// the bound above is computed in the same units the device layer will use. A `len` that is not
    /// a whole number of sectors is refused here rather than rounded — rounding down would let the
    /// tail sector escape the bound check that the device layer then happily performs.
    fn span_sectors(&self, len: usize) -> Result<u64, BlockError> {
        let dev = match self.handle {
            BlockHandle::Global => info(),
            BlockHandle::Usb => usb_info(),
            #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
            BlockHandle::Sdhc => sdhc_info(),
        }
        .ok_or(BlockError::NotReady)?;
        let bs = dev.block_size as usize;
        if bs == 0 || len == 0 || len % bs != 0 {
            return Err(BlockError::Io);
        }
        Ok((len / bs) as u64)
    }
}
