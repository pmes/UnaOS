// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Minimal block-device abstraction over the USB Mass Storage (xHCI BOT) driver.
// USBREG (SO33): the USB side is a REGISTRY ARRAY keyed on (xHCI slot, LUN) — see the USBREG section
// below — so a multi-slot card reader publishes one disk per card instead of one disk per reader.
// Geometry is published here after SCSI bring-up, and read/write are serviced by locking the xHCI
// controller.

use crate::sync::Mutex;
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
    /// MASKED gets it at once (x86, PREEMPTOFFXHCILOAN Z62: after a <= 10 ms spin only on a RUNNING block holder on another CPU), because a masked wait on a preempted holder is exactly the deadlock
    /// this family is named for; the FAT layer retries OUTSIDE its masked span and surfaces
    /// `-EAGAIN` on exhaustion.
    Busy,
    /// AHCIROOT (rmbp-ledger B332): -EPERM — a write to a SATA port with no live kernel-held grant
    /// covering that LBA (`AHCI_LIVE_GRANT`, drivers/block.rs tail). Nothing was written.
    Denied,
}

/// WEDGE-8 (F3): claim the xHCI controller for one block transaction.
///
/// Masked callers (the FAT/dir RMW spans, `with_unafs`) get `BlockError::Busy` at once unless the holder is RUNNING:
/// PREEMPTOFFXHCILOAN (zenbook-ledger Z62, x86) holds the block layer's loan preempt-off and publishes its CPU (`BlockLoan`,
/// file tail), and a masked caller spins <= `LOAN_MASKED_WAIT_MS` only on a published holder on ANOTHER CPU. F3 kept: a masked
/// context never waits on a preemptible or same-core holder, which could only run again if this core took timer IRQs.
///
/// Unmasked callers keep the old effectively-blocking semantics, honestly bounded: retry the claim
/// with a `hlt` between attempts (each wakes on the next IRQ, letting the scheduler run the loan
/// holder) up to `hw_wait_budget()` wall-clock — long enough for any healthy SERVICE PASS, but NOT for
/// every healthy BOT transaction: the driver itself budgets a first attempt at 3× this wait (`BOT_BUDGET_SCALE_FIRST` in `xhci/mod.rs`), so a slow-but-healthy transfer can hold the loan past this whole wait, and `Busy` is then a NORMAL, RETRYABLE outcome — not a wedge verdict (`usb_xhci.md` §32.3).
/// Still short enough that a wedged 25 s failing-transfer hold surfaces as `Busy` instead of hanging the caller forever.
fn claim_xhci_for_io() -> Result<BlockLoan, BlockError> {
    match take_block_loan() { // PREEMPTOFFXHCILOAN (Z62): the take holds this CPU's preempt-off count and publishes the holder
        Ok(l) => return Ok(l),
        Err(XhciClaimError::NotReady) => return Err(BlockError::NotReady),
        Err(XhciClaimError::Busy) => {}
    }
    if crate::arch::irqs_masked() || (cfg!(target_arch = "aarch64") && crate::arch::sched::preemptoff::witness().0) { // PREEMPTOFFARM (zenbook-ledger Z67): an aarch64 preempt-off holder (the FAT/DIR spans) must not hlt-wait on a preemptible holder either (WEDGE-8 F3); x86 unchanged. LINE-NEUTRAL fold
        return masked_loan_wait(); // PREEMPTOFFXHCILOAN (Z62): a bounded spin on a RUNNING holder only (was: Busy at once)
    }
    let start = crate::arch::now_cycles();
    let budget = crate::arch::hw_wait_budget();
    loop {
        crate::hlt();
        match take_block_loan() {
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

// ===================== USBREG (SO33) — the USB block registry is an ARRAY =====================
//
// PIUSB-27 gave the USB mass-storage path ONE slot, `USB_BLOCK_DEVICE`, and that slot is what this
// section replaces. The defect it carried is not subtle and it was measured on Peter's bench
// (orin-ledger A56, trunk QUEUE §2): a multi-slot card reader is ONE BOT device whose card slots are
// LOGICAL UNITS, USBLUN taught the driver to census every LUN — and then the bring-up published
// exactly one of them, because there was exactly one place to put it. A second card in the same
// reader was NAMED on the wire (`USBLUN: 2 present, publishing lun=0`) and then dropped: no
// geometry, no `/volumes/` point, invisible to `fs::bootdisk`'s walk.
//
// ### The key is (xHCI slot, LUN), and it comes from the ENUMERATOR
//
// Each entry is keyed on the pair the enumerator hands us — the xHCI slot id the device addressed on,
// and the logical unit inside it. Never on bytes: two cards imaged off each other carry the same
// `BS_VolID` and the same size, and rmbp-ledger B98 records what content-keyed identity costs (a real
// card merged into its clone and hidden from `/volumes`). A publish for a key already held REPLACES
// that entry; a publish for a new key takes the LOWEST FREE index; a retraction clears every entry
// whose SLOT matches the torn-down slot, because a disconnect takes all of a device's units with it.
//
// ### Indices are STABLE, and holes are honest
//
// The index is what [`crate::fs::fat::BlockSource::UsbN`] names, so it is an ADDRESS: compacting the
// array on a retraction would silently re-point a mounted volume at a different card, which is the
// mis-route this arc exists to stop. A retraction therefore leaves a HOLE, and the next publish fills
// the lowest free index. Entry 0 is the PRIMARY disk: it is what [`usb_info`], [`read_block_usb`],
// [`BlockHandle::Usb`] and `BlockSource::Usb` have always meant and still mean, so every consumer that
// never learned about the array keeps its exact pre-USBREG behaviour on the one-disk machine that is
// every x86 bench, every Pi bench and every QEMU leg.
//
// ### Why the global slot is no longer a coin flip
//
// `publish_usb_geometry` used to claim [`BLOCK_DEVICE`] unconditionally on every non-baremetal target,
// so with two mass-storage disks the LAST one to configure won the boot volume. The global is now
// claimed only by the disk that took INDEX 0 — the first USB disk published since the slot was last
// empty. On a one-disk machine that is the same device claiming it by the same call in the same order:
// byte-identical behaviour. On a two-card reader the second card can no longer take the boot disk's
// global handle away from it.
pub const MAX_USB_DISKS: usize = 4;

/// USBREG: one USB disk in the registry — its geometry plus the LOGICAL UNIT it lives on.
///
/// The LUN is registry state, not a field of [`BlockDeviceInfo`], and that is deliberate: a
/// `BlockDeviceInfo` is a GEOMETRY record that four unrelated backends construct (the Pi's microSD,
/// the rMBP's internal reader, the Orin's card, the stick), and only one of them has logical units.
/// Keeping the LUN here means the one publisher that has it carries it, and the block entry points
/// read it back out of the registry when they address the device — which is also what makes
/// `(slot, lun)` the key rather than something every caller must remember and re-supply.
#[derive(Clone, Copy)]
pub struct UsbDisk {
    pub info: BlockDeviceInfo,
    pub lun: u8,
}

/// USBREG: the USB block registry. `MAX_USB_DISKS` entries, index-addressed, holes permitted.
pub static USB_DISKS: Mutex<[Option<UsbDisk>; MAX_USB_DISKS]> = Mutex::new([None; MAX_USB_DISKS]);

/// USBREG: the registry entry at `ix`, if any.
pub fn usb_disk(ix: usize) -> Option<UsbDisk> {
    if ix >= MAX_USB_DISKS {
        return None;
    }
    USB_DISKS.lock()[ix]
}

/// USBREG: geometry of the USB disk at `ix` — the indexed form of [`usb_info`].
pub fn usb_info_ix(ix: usize) -> Option<BlockDeviceInfo> {
    usb_disk(ix).map(|d| d.info)
}

/// USBREG: how many registry entries hold a disk. Counts OCCUPANCY, not the highest index — a hole
/// left by a retraction is not a disk, and a census that said otherwise would count addresses.
pub fn usb_disk_count() -> usize {
    USB_DISKS.lock().iter().filter(|e| e.is_some()).count()
}

/// USBREG: is `ix` a live registry entry? The predicate `fs::fat::live_sources` walks.
pub fn usb_disk_present(ix: usize) -> bool {
    usb_disk(ix).is_some()
}

/// PIUSB-27 / USBREG: snapshot of the PRIMARY USB disk's geometry — registry entry 0.
///
/// Unchanged in meaning from the single-slot `USB_BLOCK_DEVICE` it replaces: on every machine with one
/// USB disk, entry 0 IS that disk and every pre-USBREG caller reads exactly what it read before. A
/// machine with a second card reaches it through [`usb_info_ix`].
pub fn usb_info() -> Option<BlockDeviceInfo> {
    usb_info_ix(0)
}

// ===================== APPLOAD — "is there storage I can load a PROGRAM from?" =====================
//
// SDHC-4b gave the internal SD slot a THIRD handle ([`SDHC_BLOCK_DEVICE`]) and `register_sdhc` says
// so verbatim: "(global BLOCK_DEVICE untouched)". That was deliberate and it stays — the point of
// the third handle is that a card in the internal reader can never disturb the transport the boot
// volume is being read through.
//
// The DEFECT that motivates this section is not in that decision; it is that every consumer asking a
// DIFFERENT question — "is there any storage at all that I could load a program off?" — was written
// against [`info`], which reads the GLOBAL handle and nothing else. So a machine booted from the
// INTERNAL reader with no USB stick printed
//
//     :: SDHCBLK: 8472 STAT.ELF / 12568 VUG.ELF / 12568 PULSE.ELF ::
//
// (the card mounted, the programs listed, by name and by size) and then, in the same capture,
//
//     [wc-x] desktop-app DECLINE reason=no-storage … no block device ever enumerated
//
// while that card sat mounted. The registry was right; the question was asked of the wrong slot.
//
// [`program_source`] below is that question, asked ONCE, in the block layer, so no consumer has to
// carry a `#[cfg]` to get it right.
//
// ### PRECEDENCE — PSRC (GR26): the BOOT VOLUME wins; the handle ORDER is only the fallback
//
// The original rule was "GLOBAL first, Sdhc only as a fallback", justified by the premise that this
// machine boots from a USB card reader, so the global slot IS the boot volume. `sdhc.md` §12.3
// recorded the review finding that killed that premise: the bench rMBP now boots from the INTERNAL
// slot, `sdhcblk` is default-on, and `publish_usb_geometry` claims the global UNCONDITIONALLY on
// x86. So a USB stick inserted merely to CARRY files — the b43 firmware blobs, this week, for real —
// would claim the global mid-boot and, under the old rule, silently become the program source:
// `/boot` would stop meaning the volume the machine booted from and `bg /apps/VUG.ELF` would resolve
// against the stick.
//
// FRGUARD already refuses the WRITE half of exactly that substitution (`BM_SUBSTITUTED`, Boot AI-2,
// `docs/SECURITY.md`). The kernel therefore already HOLDS the evidence — `BootInfo::boot_volume_serial`,
// published here by `set_boot_volume_serial` — and the old `program_source` simply did not consult it.
// This is that consultation.
//
// The rule, in order:
//
//   1. **boot-serial.** If the FRGUARD verdict POSITIVELY locates the boot volume on a handle, and
//      that handle is still populated, that handle wins. `BM_MATCH` -> `Global`, `BM_SUBSTITUTED` ->
//      `Sdhc`. This is the only arm that can differ from the pre-PSRC answer, and it differs in
//      exactly one direction: away from a medium this kernel did not boot from.
//   2. **fallback-order / serial-unproven.** Everything else keeps the historical ladder — global,
//      then `Sdhc`. That covers a boot serial of 0 (the loader could not identify the medium; the
//      disarmed sentinel), a serial found on NEITHER handle (`BM_UNPROVEN` — the QEMU harness, and
//      any machine booting from a device the block layer has no driver for), a verdict not yet
//      derived, and a verdict whose named handle has since been retracted. Same fail-open direction
//      FRGUARD takes, for the same reason: an unidentified or unreachable boot volume is a reason to
//      SAY SO, not a reason to guess.
//
// ### Why this is cheap on the hot path, and why it adds no cache
//
// `program_source` is consulted per open — the shell's twenty-five FAT verbs, every exec probe, the
// desktop-app storage gate on each main-loop pass while it waits, the wifi staging poll. It must not
// read a sector, and it must not be callable into a masked deadlock.
//
// It does neither, because **the cache it needs already exists**: `BOOT_MEDIUM_VERDICT`. FRGUARD
// derives it ONCE, from an unmasked main-loop context (`flight_recorder::service`), precisely because
// deriving costs sector reads that cannot happen inside `write_block`; and `publish_usb_geometry`
// resets it to `BM_UNKNOWN` whenever a new device claims the global slot, precisely so a hot-plug
// cannot inherit the previous occupant's verdict. That is the same freshness property a preference
// cache would need, already paid for and already metal-exercised. Adding a SECOND cache here would
// duplicate the invalidation and create the stale-handle hazard it was meant to avoid — two caches
// that can disagree about which disk is in the slot is strictly worse than one.
//
// So the whole decision is two relaxed-cost atomic loads plus the two `Option` reads the function
// already performed. No sector I/O, no new lock, no new state.
//
// The one residual staleness — a verdict naming a handle that has since been RETRACTED
// (`unpublish_usb_geometry` deliberately does not reset the verdict, because resetting it would turn
// a `BM_SUBSTITUTED` refusal into a fail-open and WEAKEN FRGUARD) — is handled structurally rather
// than by invalidation: arm 1 only wins if the named handle is still `Some`. A verdict that names an
// empty slot falls through to arm 2 and says `verdict=stale-handle` on the wire.
//
// ### Is the verdict derived in time?
//
// It is derived in exactly the case that needs it, and the race that looks available is empty.
// `flight_recorder::service` returns early while `info()` is `None`, so on a card-ONLY boot the
// verdict is never derived at all — and that costs nothing, because with the global slot empty the
// fallback ladder already lands on `Sdhc`. The verdict only matters when BOTH handles are populated,
// which is precisely the state in which the flight recorder's gate passes and the derivation runs.
//
// There IS a residual window, and it is stated rather than argued away (review, GR26). Ordering on
// x86 is fixed: `register_sdhc` runs synchronously inside `pci::init` (`sdhc::probe` -> `bring_up`),
// while `publish_usb_geometry` runs from the main loop's deferred SCSI bring-up
// (`xhci::service_storage`, held until the enumeration queue drains). So the card is always
// registered FIRST, and every poll-until-present consumer — `wifi::service`, `desktop_uefi::desktop_app_service`,
// `shell::fatverb_storage_witness` — resolved and parked on `Sdhc` at the first main-loop pass, long
// before any stick arrives. What remains is the INTRA-PASS window on the one pass where a stick
// claims the global: `service_storage` resets the verdict to `BM_UNKNOWN` near the top of the pass
// and `flight_recorder::service` re-derives it at the bottom, so between those two points arm 2 runs
// with `verdict=undecided` and the ladder answers `Global` — the stick. A per-call consumer that
// resolves inside that window (a shell verb or an exec dispatched on exactly that pass) gets the
// stick for that one call. It is bounded to a single main-loop pass, it requires a hot-plug
// concurrent with the call, it is read-side only (FRGUARD still refuses the write), and the `PSRC`
// witness prints the transient `psrc=global … verdict=undecided` line, so the capture shows it. It
// is NOT closed by construction, and closing it would cost either a sector read on the hot path or a
// second cache — both of which this section rejects for stronger reasons.
//
// ### The `Usb` handle is still deliberately NOT in the ladder
//
// It exists so the Pi can reach a stick while the microSD owns the global; on x86 the stick IS the
// global, so including it here would either duplicate the global arm or let a Pi program load
// silently retarget.

/// PSRC: handle codes for the change-only witness latch.
const PSRC_H_NONE: u8 = 0;
const PSRC_H_GLOBAL: u8 = 1;
const PSRC_H_SDHC: u8 = 2;
/// PSRC: reason codes — the three-token vocabulary `sdhc.md` §12.3 named.
const PSRC_R_BOOT_SERIAL: u8 = 0;
const PSRC_R_FALLBACK_ORDER: u8 = 1;
const PSRC_R_SERIAL_UNPROVEN: u8 = 2;
/// PSRC: the underlying FRGUARD state, so `reason=fallback-order` and `reason=serial-unproven` each
/// stay falsifiable instead of collapsing four distinct causes into one word.
const PSRC_V_MATCH: u8 = 0;
const PSRC_V_SUBSTITUTED: u8 = 1;
const PSRC_V_UNPROVEN: u8 = 2;
const PSRC_V_UNDECIDED: u8 = 3;
const PSRC_V_DISARMED: u8 = 4;
const PSRC_V_STALE_HANDLE: u8 = 5;
const PSRC_V_UNBUILT: u8 = 6;

/// PSRC: the last decision this boot PRINTED, packed as `handle | reason<<2 | verdict<<4`, plus a
/// `has-printed` bit so the very first decision is never mistaken for a repeat of code 0.
///
/// Change-only rather than once-only on purpose. `program_source` runs per open, so a per-call line
/// would flood; but a once-only line would hide the event this arc exists for — the moment a stick
/// claims the global and the preference has to REFUSE it. Latching on the decision means the wire
/// carries one line per distinct answer, which is the falsifiable minimum: a boot where the answer
/// never changes prints once, and a boot where a stick arrives prints the flip.
static PSRC_LAST: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);

/// PSRC: emit the decision line if (and only if) it differs from the last one printed.
///
/// Pure logging: it takes each registry slot's lock briefly through [`source_census`] and changes
/// nothing, so it is safe from any context `program_source` itself is safe from.
fn psrc_witness(handle: u8, reason: u8, verdict: u8, serial: u32) {
    let code: u16 = 0x100 | (handle as u16) | ((reason as u16) << 2) | ((verdict as u16) << 4);
    if PSRC_LAST.swap(code, core::sync::atomic::Ordering::Relaxed) == code {
        return;
    }
    let psrc = match handle {
        PSRC_H_GLOBAL => "global",
        PSRC_H_SDHC => "sdhc",
        _ => "none",
    };
    let reason_s = match reason {
        PSRC_R_BOOT_SERIAL => "boot-serial",
        PSRC_R_SERIAL_UNPROVEN => "serial-unproven",
        _ => "fallback-order",
    };
    let verdict_s = match verdict {
        PSRC_V_MATCH => "match",
        PSRC_V_SUBSTITUTED => "substituted",
        PSRC_V_UNPROVEN => "unproven",
        PSRC_V_UNDECIDED => "undecided",
        PSRC_V_DISARMED => "disarmed",
        PSRC_V_STALE_HANDLE => "stale-handle",
        PSRC_V_UNBUILT => "unbuilt",
        // Not constructible: every caller passes one of the seven constants above. Rendered rather
        // than `unreachable!()` so a witness can never be the thing that panics a boot.
        _ => "?",
    };
    serial_println!(
        ":: PSRC: psrc={} reason={} verdict={} boot_serial=0x{:08x} handles={} ::",
        psrc, reason_s, verdict_s, serial, source_census()
    );
}

/// APPLOAD / PSRC: the block device a program should be loaded from, with the HANDLE it was found
/// under — `None` only when there is genuinely nowhere to look.
///
/// Precedence: the handle the FRGUARD verdict positively locates the BOOT VOLUME on, if it is still
/// populated; else the historical ladder — the global [`BLOCK_DEVICE`], then (x86 + `sdhcblk`) the
/// internal SD card under [`SDHC_BLOCK_DEVICE`]. See the PSRC section note above for the rule, its
/// cost argument, and why it adds no cache of its own.
///
/// The returned handle is not decoration: the three handles are served by DIFFERENT read paths
/// (`read_block` goes through the backend selector, `read_block_sdhc` bypasses it into the SDHCI
/// driver), so a caller that reads without honouring the handle would issue the wrong transport's
/// commands. `fs::fat::mount_program_source` is the intended consumer and it maps this handle
/// straight onto the matching `fs::fat::BlockSource`.
pub fn program_source() -> Option<(BlockDeviceInfo, BlockHandle)> {
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    {
        use core::sync::atomic::Ordering;
        let glob = info();
        let card = sdhc_info();
        let serial = BOOT_VOLUME_SERIAL.load(Ordering::Acquire);
        let verdict = BOOT_MEDIUM_VERDICT.load(Ordering::Acquire);

        // --- Arm 1: the boot volume was POSITIVELY located, and its handle is still populated.
        if serial != 0 {
            if verdict == BM_MATCH {
                if let Some(dev) = glob {
                    psrc_witness(PSRC_H_GLOBAL, PSRC_R_BOOT_SERIAL, PSRC_V_MATCH, serial);
                    return Some((dev, BlockHandle::Global));
                }
            } else if verdict == BM_SUBSTITUTED {
                if let Some(dev) = card {
                    psrc_witness(PSRC_H_SDHC, PSRC_R_BOOT_SERIAL, PSRC_V_SUBSTITUTED, serial);
                    return Some((dev, BlockHandle::Sdhc));
                }
            }
        }

        // --- Arm 2: the historical global-then-Sdhc ladder, with the cause named.
        let (reason, vstate) = if serial == 0 {
            (PSRC_R_FALLBACK_ORDER, PSRC_V_DISARMED)
        } else {
            match verdict {
                BM_UNPROVEN => (PSRC_R_SERIAL_UNPROVEN, PSRC_V_UNPROVEN),
                BM_UNKNOWN => (PSRC_R_SERIAL_UNPROVEN, PSRC_V_UNDECIDED),
                // BM_MATCH / BM_SUBSTITUTED that fell through arm 1: the verdict names a handle the
                // registry no longer holds (a disconnect retracted it without resetting the verdict —
                // see the note above on why FRGUARD must not reset it there).
                _ => (PSRC_R_FALLBACK_ORDER, PSRC_V_STALE_HANDLE),
            }
        };
        if let Some(dev) = glob {
            psrc_witness(PSRC_H_GLOBAL, reason, vstate, serial);
            return Some((dev, BlockHandle::Global));
        }
        if let Some(dev) = card {
            psrc_witness(PSRC_H_SDHC, reason, vstate, serial);
            return Some((dev, BlockHandle::Sdhc));
        }
        psrc_witness(PSRC_H_NONE, reason, vstate, serial);
        None
    }
    // No `Sdhc` handle exists in this image, so there is no precedence to decide and no boot serial
    // published to decide it with: the ladder is one rung long. The witness still fires — a build
    // that cannot substitute should SAY that it cannot, rather than leaving a reader to infer it
    // from the absence of a line (the exact inference `sdhc.md` §12.4 records as falsified).
    #[cfg(not(all(target_arch = "x86_64", feature = "sdhcblk")))]
    {
        let glob = info();
        psrc_witness(
            if glob.is_some() { PSRC_H_GLOBAL } else { PSRC_H_NONE },
            PSRC_R_FALLBACK_ORDER,
            PSRC_V_UNBUILT,
            0,
        );
        glob.map(|dev| (dev, BlockHandle::Global))
    }
}

/// APPLOAD: what [`program_source`] LOOKED AT, for the witness lines that report its `None`.
///
/// The defect this arc fixes was invisible for exactly one reason: the decline line asserted "no
/// block device ever enumerated" when it had only ever consulted one of three slots. A refusal that
/// cannot name what it inspected cannot be falsified by the capture it appears in. So every consumer
/// that declines on an absent program source now prints this census beside the reason, and a future
/// no-apps boot names its own cause instead of a boot-wide claim it never checked.
///
/// Every field can read either way on a real boot, and the three renderings are distinguishable:
/// `sdhc=present` (a card registered), `sdhc=absent` (the feature is in, nothing registered),
/// `sdhc=unbuilt` (no `sdhcblk` in this image, so the handle does not exist to be consulted).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SourceCensus {
    /// The global [`BLOCK_DEVICE`] slot holds a device.
    pub global: bool,
    /// The internal-SD slot: `Some(true/false)` when this image carries the handle, `None` when it
    /// does not — which is a different fact from "the handle is empty" and is printed differently.
    pub sdhc: Option<bool>,
    /// TEGRA-SDBLK: the Orin microSD slot, read the same three ways as `sdhc` above
    /// (`Some(true)` registered / `Some(false)` handle built but empty / `None` not in this image).
    ///
    /// It is censused but NOT a program-source rung: `program_source` does not consult it, so this
    /// field never explains a `psrc=` verdict. It is here because the census's job is to name every
    /// handle that EXISTS, and a reader debugging "why did nothing mount the card" is owed the
    /// difference between "no tegra handle in this build" and "the handle is there and empty".
    pub tegra_sd: Option<bool>,
}

impl core::fmt::Display for SourceCensus {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "global={}", if self.global { "present" } else { "absent" })?;
        match self.sdhc {
            Some(true) => write!(f, " sdhc=present"),
            Some(false) => write!(f, " sdhc=absent"),
            None => write!(f, " sdhc=unbuilt"),
        }?;
        // TEGRA-SDBLK: appended, never interleaved — the existing two fields keep their exact
        // spelling and order, so every capture that greps `global=` / `sdhc=` still reads the same.
        //
        // Printed ONLY on a build that carries the handle. `sdhc` prints its `unbuilt` case because
        // on x86 "is `sdhcblk` in this image" is the live question a decline line has to answer; on
        // an rMBP or a Pi "is the ORIN's card handle in this image" is not a question anyone is
        // asking, and answering it would rewrite a witness line three docs quote verbatim. So a
        // non-tegra image renders exactly the string it rendered before this variant existed.
        match self.tegra_sd {
            Some(true) => write!(f, " tegra_sd=present"),
            Some(false) => write!(f, " tegra_sd=absent"),
            None => Ok(()),
        }
    }
}

/// APPLOAD: snapshot the handles [`program_source`] consults. Pure query — takes each slot's lock
/// briefly and changes nothing, so it is safe to call from a decline path.
pub fn source_census() -> SourceCensus {
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    let sdhc = Some(sdhc_info().is_some());
    #[cfg(not(all(target_arch = "x86_64", feature = "sdhcblk")))]
    let sdhc = None;
    #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
    let tegra_sd = Some(tegra_sd_info().is_some());
    #[cfg(not(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc")))]
    let tegra_sd = None;
    SourceCensus { global: info().is_some(), sdhc, tegra_sd }
}

/// INSTALL-SEL: which of the two registry handles a device row was read from. The block layer keeps
/// two `Option<BlockDeviceInfo>` slots — the GLOBAL [`BLOCK_DEVICE`] (whatever the active backend is:
/// the xHCI stick on x86, the microSD once `register_sd` has run on the Pi) and the dedicated USB
/// handle [`USB_DISKS`] — and the graphical installer's chooser can show BOTH as separate rows
/// when they name different devices. "Row 1" is therefore not a property of the device; it is a
/// property of one frame's list. Naming the handle makes the row's identity independent of the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockHandle {
    /// The global [`BLOCK_DEVICE`] entry — read/written through [`read_block`] / [`write_block`].
    Global,
    /// The dedicated [`USB_DISKS`] entry — read/written through [`read_block_usb`] /
    /// [`write_block_usb`], which bypass the backend selector.
    Usb,
    /// SDHC-4b (x86, `sdhcblk` knob): the dedicated [`SDHC_BLOCK_DEVICE`] entry — the card in the
    /// machine's INTERNAL SD slot, read through [`read_block_sdhc`] / [`read_blocks_sdhc`], which
    /// bypass the backend selector exactly as the `Usb` pair does. See the SDHC-4b section below for
    /// why it is a third handle and not a third value of the `BACKEND` selector.
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    Sdhc,
    /// TEGRA-SDBLK (aarch64, `tegra` + `sdmmc`): the dedicated [`TEGRA_SD_BLOCK_DEVICE`] entry — the
    /// card in the Orin devkit's microSD slot, read through [`read_block_tegra_sd`] /
    /// [`read_blocks_tegra_sd`], which bypass the backend selector exactly as the `Usb` and `Sdhc`
    /// pairs do. See the TEGRA-SDBLK section below for why it is its own handle rather than a value
    /// of the `BACKEND` selector (which is `baremetal`-gated and does not exist on a tegra build).
    ///
    /// Gated on the same triple as the entry points it names, i.e. on EXACTLY the builds that have a
    /// read path to dispatch to — the `Sdhc` precedent, and the reason a non-tegra image is
    /// byte-identical to its pre-variant self (no arm of this variant is compiled there at all).
    ///
    /// **Writes through this handle are refused in every cfg.** The card's only writer is the armed
    /// `sdmmc_arm` ladder in `sdmmc_tegra.rs`; [`write_block_tegra_sd`] exists so the dispatch below
    /// is total and fails CLOSED, not so that anything writes.
    #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
    SdMmc, #[cfg(all(target_arch = "x86_64", feature = "ahci"))] /** AHCIBOOT: one published SATA disk, keyed on the HBA PORT — the enumerator's key (`drivers::ahci`'s `PI` mask), never anything read off the medium. Reads go through [`read_block_ahci_port`]; every write through it is REFUSED, because `drivers/ahci.rs` compiles no ATA write opcode at all. The installer refuses it by construction (`install/mod.rs`, rmbp-ledger B91). */ Ahci { port: u8 }, #[cfg(all(target_arch = "x86_64", feature = "nvme"))] /** NVMEBOOT (zenbook-ledger Z9): the NVMe namespace 1 that `drivers::nvme` published into the NVMe registry (block.rs tail). Reads go through [`read_block_nvme`]; a write is admitted only inside the boot's NVMe root grant (one UnaFS extent), -EPERM elsewhere. */ Nvme,
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
    // USBREG: the USB handle is now an ARRAY, so resolving an identity against it means asking every
    // live entry rather than only entry 0. This is strictly more truthful than what it replaces: an
    // identity captured while a second card was the chooser's row used to resolve to `None` (entry 0
    // is a different disk) and the installer would refuse a target that is in fact still present.
    // The match clauses below are unchanged, so a resolution that succeeded before still succeeds.
    if id.handle == BlockHandle::Usb {
        let reg = *USB_DISKS.lock();
        return reg
            .iter()
            .flatten()
            .map(|d| d.info)
            .find(|d| d.slot_id == id.slot_id && d.num_blocks == id.num_blocks);
    }
    let cur = match id.handle {
        BlockHandle::Global => info(),
        BlockHandle::Usb => usb_info(),
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        BlockHandle::Sdhc => sdhc_info(),
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        BlockHandle::SdMmc => tegra_sd_info(), #[cfg(all(target_arch = "x86_64", feature = "ahci"))] BlockHandle::Ahci { port } => ahci_info_port(port), #[cfg(all(target_arch = "x86_64", feature = "nvme"))] BlockHandle::Nvme => nvme_info(), // AHCIBOOT: the registry row for that port, so an identity captured on a SATA disk re-resolves to the same disk.
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
/// [`USB_DISKS`]; the transfer is the same xHCI BOT READ(10) the default path has always used, and
/// it takes only the xHCI controller lock (the SD/emmc2 path is untouched). Returns bytes copied.
pub fn read_block_usb(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    read_block_usb_ix(0, lba, buf)
}

/// USBREG: read one block from the USB disk at registry index `ix`.
///
/// Every USB read in this file funnels through here, [`read_block_usb`] included, and that is the
/// point: the transfer is addressed at the entry's OWN `(slot_id, lun)` rather than at whichever LUN
/// the driver's per-slot `bot_lun` field happens to hold. Before USBREG that field was written once
/// by the bring-up and read by every later transfer, which is sound for one disk and a silent
/// mis-route for two — a read of disk 1 would leave `bot_lun` pointing at disk 1's card and the next
/// read of disk 0 would return disk 1's sectors, with no error anywhere. Addressing per call removes
/// the ordering hazard instead of documenting it.
pub fn read_block_usb_ix(ix: usize, lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let disk = usb_disk(ix).ok_or(BlockError::NotReady)?;
    let dev = disk.info;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
    }
    // WEDGE-8 (F3): a claimed LOAN, not a held lock — the BOT pump below runs with no lock held.
    let mut xhci = claim_xhci_for_io()?;
    match xhci.storage_read10_on(dev.slot_id, disk.lun, read10_lba32("read-usb", lba)?, 1) { // LBA32 (SR15): refuse, never truncate — see `read10_lba32` at the file tail.
        Ok(res) if res.status == CswStatus::Passed => {}
        other => {
            io_cause_witness("read-usb", lba, other);
            return Err(BlockError::Io);
        }
    }
    let src = xhci.storage_data_ptr_on(dev.slot_id).ok_or(BlockError::Io)?;
    let n = (dev.block_size as usize).min(buf.len());
    unsafe {
        core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), n);
    }
    Ok(n)
}

/// USB-WRITE: write one block (`lba`) to the USB mass-storage stick DIRECTLY through the xHCI
/// controller — the write twin of [`read_block_usb`]. Bypasses the backend selector, so the stick
/// is writable even when the global block device is the microSD (BACKEND_SD on the Pi). Geometry
/// (block size, bound) comes from [`USB_DISKS`]; the transfer is the same xHCI BOT WRITE(10)
/// the default path uses, taking only the xHCI controller lock (the SD/emmc2 path is untouched).
/// The caller's `buf` is staged (zero-padded to the block size) into the controller's DMA buffer,
/// then WRITE(10) is issued; a non-`Passed` CSW propagates as [`BlockError::Io`] — the write NEVER
/// reports a false success. This is the block-layer half of a writable `/usb`: the FAT layer's
/// `write_sector` routes its `Usb` source here in place of the PIUSB-27 read-only refusal.
pub fn write_block_usb(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    write_block_usb_ix(0, lba, buf)
}

/// USBREG: write one block to the USB disk at registry index `ix`. The write twin of
/// [`read_block_usb_ix`], addressed at the entry's own `(slot_id, lun)` for the same reason.
pub fn write_block_usb_ix(ix: usize, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let disk = usb_disk(ix).ok_or(BlockError::NotReady)?;
    let dev = disk.info;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
    }
    // WEDGE-8 (F3): a claimed LOAN, not a held lock — the BOT pump below runs with no lock held.
    let mut xhci = claim_xhci_for_io()?;
    let dst = xhci.storage_data_ptr_on(dev.slot_id).ok_or(BlockError::Io)?;
    let n = (dev.block_size as usize).min(buf.len());
    unsafe {
        core::ptr::write_bytes(dst, 0, dev.block_size as usize);
        core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, n);
    }
    match xhci.storage_write10_on(dev.slot_id, disk.lun, read10_lba32("write-usb", lba)?, 1) { // LBA32 (SR15): refuse, never truncate — a wrapped WRITE lands on someone else's sector.
        Ok(res) if res.status == CswStatus::Passed => Ok(()),
        other => {
            io_cause_witness("write-usb", lba, other);
            Err(BlockError::Io)
        }
    }
}

/// PIUSB-28: publish the geometry of a freshly enumerated USB mass-storage device. Always records it
/// under the dedicated [`USB_DISKS`] handle (so `read_block_usb`/`/fs/usb` can reach the stick),
/// then raises the storage-ready edge. The global [`BLOCK_DEVICE`] is only claimed when USB is the ACTIVE
/// backend — i.e. no SD card has flipped the selector to `BACKEND_SD`. On the Pi the microSD registers at
/// BSP probe (long before xHCI enum), so a later-enumerated USB stick must NOT overwrite the SD geometry:
/// PI-FS-2 traced a 14 MiB USB card reader's `num_blocks` clobbering the SD's global, which bounded fresh
/// unafs mounts to the reader's size → `PartError::OutOfBounds(63)`. On x86 (no `register_sd`, no BACKEND
/// selector) the stick IS the default/boot backend, so this always claims the global — byte-identical to
/// the pre-PIUSB-28 behavior there. Must run OUTSIDE the xHCI controller lock's storage callers as before.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn publish_usb_geometry(dev: BlockDeviceInfo) {
    publish_usb_geometry_lun(dev, 0);
}

/// USBREG: the LUN-carrying form. `publish_usb_geometry` is this call with `lun = 0`, which is what
/// every non-LUN publisher means and what a `nousblun` build always means.
///
/// Returns the registry INDEX the disk landed on — the xHCI bring-up prints it, and the caller needs
/// it to know whether this disk is the primary (index 0) or one of the extra cards.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn publish_usb_geometry_lun(dev: BlockDeviceInfo, lun: u8) -> usize {
    let ix = registry_place(dev, lun);
    // Claim the global only while USB is still the active backend; once the SD has registered, leave
    // BLOCK_DEVICE (the SD's geometry) untouched — the stick stays reachable through the registry.
    // USBREG: and only for the PRIMARY disk (index 0). A second card in the same reader is a disk,
    // not a candidate for the global slot.
    if ix == 0 && BACKEND.load(Ordering::Acquire) != BACKEND_SD {
        *BLOCK_DEVICE.lock() = Some(dev);
    }
    USB_PUBLISH_GEN.fetch_add(1, core::sync::atomic::Ordering::AcqRel); // PA35 race: every publish is a new generation
    set_usb_ready();
    ix
}

/// PIUSB-28: x86 / non-SD-capable targets — the USB stick is the default (boot) block backend, so it
/// always claims the global `BLOCK_DEVICE` alongside the dedicated USB handle. Preserves the historical
/// behavior on any target that never compiles the SD backend / BACKEND selector.
///
/// FRGUARD (GR21): a claim of the global slot INVALIDATES the boot-medium verdict — the next reader
/// re-derives it against whatever now occupies the slot. See [`BOOT_MEDIUM_VERDICT`].
#[cfg(not(all(target_arch = "aarch64", feature = "baremetal")))]
pub fn publish_usb_geometry(dev: BlockDeviceInfo) {
    publish_usb_geometry_lun(dev, 0);
}

/// USBREG: the LUN-carrying form — see the `baremetal` twin above. Returns the registry index.
#[cfg(not(all(target_arch = "aarch64", feature = "baremetal")))]
pub fn publish_usb_geometry_lun(dev: BlockDeviceInfo, lun: u8) -> usize {
    let ix = registry_place(dev, lun);
    // USBREG: the global is the PRIMARY disk's, and only the primary's. Before this, every publish
    // claimed it, so the last mass-storage unit to configure owned the boot volume — the coin flip
    // trunk QUEUE §2 records. Index 0 is the first disk published since the slot was last empty, so
    // on a one-disk machine this is the same claim by the same call in the same order.
    if ix == 0 && !usb_global_closed(dev.slot_id) { // USBSTOR (B384, R95): once the boot medium is pinned on another handle, a USB disk is a SEPARATE disk and never the global slot — see `pin_boot_medium_once` at the file tail.
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        BOOT_MEDIUM_VERDICT.store(BM_UNKNOWN, core::sync::atomic::Ordering::Release);
        *BLOCK_DEVICE.lock() = Some(dev);
    }
    USB_PUBLISH_GEN.fetch_add(1, core::sync::atomic::Ordering::AcqRel); // PA35 race: every publish is a new generation
    set_usb_ready();
    ix
}

/// USBREG: put `dev` in the registry under the key `(dev.slot_id, lun)` and return its index.
///
/// Three cases, in order, and the order is the whole rule:
///  1. **the key is already held** — REPLACE it in place. A re-enumeration of the same unit must not
///     consume a second address, and a mounted volume that named this index keeps naming the same
///     card. This is also what a `publish` after a failed-and-retried bring-up does.
///  2. **a free index exists** — take the LOWEST one, so a machine that has retracted everything
///     refills index 0 and the primary-disk meaning of entry 0 is restored.
///  3. **full** — refuse, loudly, and answer `MAX_USB_DISKS` (which no `usb_disk` call resolves). A
///     registry that silently evicted a live disk to make room would be the single-slot defect again
///     with more steps.
fn registry_place(dev: BlockDeviceInfo, lun: u8) -> usize {
    let mut reg = USB_DISKS.lock();
    for (i, e) in reg.iter_mut().enumerate() {
        if let Some(cur) = e {
            if cur.info.slot_id == dev.slot_id && cur.lun == lun {
                *e = Some(UsbDisk { info: dev, lun });
                return i;
            }
        }
    }
    for (i, e) in reg.iter_mut().enumerate() {
        if e.is_none() {
            *e = Some(UsbDisk { info: dev, lun });
            return i;
        }
    }
    drop(reg);
    serial_println!(
        ":: BLK: usb registry FULL ({} disks) — slot={} lun={} not published; the disks already \
         registered are untouched ::",
        MAX_USB_DISKS, dev.slot_id, lun
    );
    MAX_USB_DISKS
}

/// USB-UNPLUG: retract the geometry a USB mass-storage device published, when its xHCI slot is torn
/// down on a physical disconnect. This is the missing half of [`publish_usb_geometry`]: before it, the
/// xHCI layer handled removal correctly at ITS level (slot bindings cleared, DISABLE_SLOT queued —
/// the metal wire shows `[Port 1] slot 1 torn down on disconnect` / `DISABLE_SLOT slot 1 -> code 1`)
/// but nothing downstream ever heard about it, so `BLOCK_DEVICE` / the USB registry kept a dead
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
/// PA35's storage race, witnessed live: a rescue ladder that began before a replug ran its own
/// port-cycle, the re-enumeration it caused PUBLISHED a fresh healthy disk mid-ladder, and the
/// ladder's final surrender then RETRACTED that fresh publish — same slot id, nothing to tell the
/// generations apart (`Disk …` then `:: BLK: removed …` one line later on the wire). Every publish
/// now advances this generation; a retractor captures the generation it is working against and
/// [`unpublish_usb_geometry`] refuses to remove a NEWER publish than the one the retraction was
/// queued/earned against.
pub static USB_PUBLISH_GEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The current publish generation — retractors capture this at the moment their teardown/ladder
/// BEGINS, and pass it to [`unpublish_usb_geometry`] so a publish that lands in between survives.
pub fn usb_publish_gen() -> u64 {
    USB_PUBLISH_GEN.load(core::sync::atomic::Ordering::Acquire)
}

/// Returns true if a registry entry was actually retracted (i.e. this slot WAS the storage device).
/// `captured_gen` is the publish generation the caller captured when its retraction became owed
/// (ladder start / disconnect detection); a registry holding a newer generation is a DIFFERENT
/// device on a recycled slot number and must survive.
pub fn unpublish_usb_geometry(slot_id: u8, captured_gen: u64) -> bool {
    if USB_PUBLISH_GEN.load(core::sync::atomic::Ordering::Acquire) != captured_gen {
        crate::bootlog_println!(
            ":: BLK: retraction SKIPPED slot={} — a newer publish (gen {}) superseded the one this retraction was earned against (gen {}); the live disk survives ::",
            slot_id, USB_PUBLISH_GEN.load(core::sync::atomic::Ordering::Acquire), captured_gen
        );
        return false;
    }
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
    // USBREG: a disconnect takes EVERY logical unit of the device with it, so the retraction clears
    // every registry entry whose SLOT matches — the same `(slot, lun)` key the publish used, asked
    // with the lun wild. Indices are NOT compacted: an index is the address
    // `fs::fat::BlockSource::UsbN` names, and shuffling live disks down to fill a hole would
    // re-point a mounted volume at a different card, which is the mis-route this arc removes.
    let mut units = 0u8;
    {
        let mut reg = USB_DISKS.lock();
        for e in reg.iter_mut() {
            if (*e).map(|d| d.info.slot_id) == Some(slot_id) {
                if departing.is_none() {
                    departing = (*e).map(|d| d.info);
                }
                units = units.saturating_add(1);
                *e = None;
            }
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
    USB_STORAGE_READY.store(false, core::sync::atomic::Ordering::Release); USB_PUBLISH_GEN.fetch_add(1, core::sync::atomic::Ordering::AcqRel); // USBUNPUB: A RETRACTION IS A GENERATION TOO. Until this call the counter moved only on PUBLISH, so `video/quarry/live.rs::volume_gen` — which ADDS this number to `fs::NS_GEN` — could not see a REMOVAL, and Quarry went on serving a listing of a volume that had physically left until the next ARRIVAL happened to bump it. SR3 covered every mutation UnaOS itself performs; a retraction performed by a human hand was still unheard. Placed HERE, not at the head: AFTER the `departing` unwrap, so a retraction that removed NOTHING moves nothing (a generation is a CHANGE, and every no-op arm above returns `false` having changed no registry), and AFTER both handle clears, so a reader that observes the new generation observes the emptied registry behind it — AcqRel, the ordering both publish sites already use. The gen GUARD at the head of this function compares against the generation the retraction was EARNED against, captured by each caller before its teardown begins, so a tail bump cannot make a retraction refuse itself; the two loop callers in `drivers/xhci/mod.rs` (`:6258`, `:14654`) re-read `usb_publish_gen()` inline on every iteration and the ladder caller (`:11147`) makes one call. Line-neutral append — see the FILE TAIL note below: no `core::panic::Location` in this file moves.

    // The removal witness. QEMU cannot hot-unplug a device here, so this line is what proves on the
    // next attended metal boot that the disconnect actually reached the block registry — the exact
    // evidence whose absence defined this defect.
    let product = core::str::from_utf8(&dev.product).unwrap_or("?").trim_end();
    let vendor = core::str::from_utf8(&dev.vendor).unwrap_or("?").trim_end();
    serial_println!(
        ":: BLK: removed '{}' '{}' (xhci slot {} disconnect) units={} remaining={} ::",
        vendor, product, slot_id, units, usb_disk_count()
    );
    true
}

/// M6g: register the microSD (EMMC2/SDHCI) as the block backend — publish its geometry AND flip the
/// selector so `read_block` (and, since U9, `write_block`) route to `drivers::emmc2`. Called once, from
/// the bare-metal BSP probe after a successful card init (`emmc2::probe`). aarch64 bare-metal only; x86
/// never calls it, so its read/write path is byte-identical to the pre-M6g xHCI-only code.
///
/// SDHCRW (rmbp-ledger B166, R59, 2026-09-22): this is where the Pi's card SAYS its posture, and the
/// site is chosen because it is the one call in this file that runs EXACTLY ONCE on a Pi boot and
/// runs before any consumer — the same "at the mount" property the x86 truth table has, obtained
/// from the call graph rather than from a latch. **The emmc2 write path itself did not change and
/// needed no change: it is behind NO feature at all** (`write_block`'s `BACKEND_SD` arm routes
/// straight to `emmc2::write_block_512`, and `emmc2` has issued CMD24 since U9), so R59's
/// default-on half was already true on this board — worth saying out loud, because the rule "if the
/// write path is behind a feature, that feature becomes default-on" has no work to do here. What
/// was missing was the OTHER half of LAWS §3: a NAMED OPT-OUT. `sdw-ro` is it, and this line is how
/// a `kernel8-test` capture reads which posture the image carries.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn register_sd(dev: BlockDeviceInfo) {
    *BLOCK_DEVICE.lock() = Some(dev);
    BACKEND.store(BACKEND_SD, Ordering::Release);
    serial_println!(
        ":: SDHCPOST: posture sdw-ro={} driver=emmc2 write-path=live -> sd={} reason={} (the Pi \
         card's write path is behind no feature and never was; R59's opt-out is the only thing \
         that refuses it \u{2014} SDHCRW, at the registration, once) ::",
        if cfg!(feature = "sdw-ro") { 1 } else { 0 },
        if cfg!(feature = "sdw-ro") { "ro" } else { "rw" },
        if cfg!(feature = "sdw-ro") { "opt-out" } else { "none" }
    );
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
/// arm of `default_writable` (below) closes it, keyed on the boot volume's identity as the UEFI loader
/// reported it, rather than on this file's `BACKEND` selector, which x86 does not compile.
///
/// **The aarch64 arm has a gap this arc does NOT close, recorded in `docs/SECURITY.md`'s USBFALL row and
/// in `docs/dev/OS/07_USB_STORAGE/usb_xhci.md` §11 so the Pi lane sees it:** the guard below is called
/// only from [`write_block`], never from [`write_blocks`], so a `Default` MULTI-SECTOR FAT write
/// (`fs/fat.rs::write_sectors`) bypasses USBFALL F1 entirely on `aarch64 + baremetal`. Pre-existing, real,
/// and out of this arc's lane — the x86 arm guards both entry points.
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
///
/// SDHCRW (B166, R59): `sdw-ro` is a SECOND, independent reason this can be `false`, and the two are
/// deliberately not merged into one message — [`guard_default_write_backend`] below tests the
/// opt-out FIRST and says `reason=opt-out`, because printing USBFALL's "no SD backend registered"
/// over a boot whose card registered perfectly well and was simply asked to stay read-only is
/// exactly the kind of misattributed refusal this file's one-definition rule exists to stop.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn default_writable() -> bool {
    !cfg!(feature = "sdw-ro") && BACKEND.load(Ordering::Acquire) == BACKEND_SD
}

// ===================== FRGUARD (GR21) — is the Default slot the medium we BOOTED from? ==============
//
// R1 of this arc keyed the guard on CLAIM ORIGIN: boot port scan vs hot-plug edge. That predicate is
// REFUTED by the bench's own capture and the kernel's own instrument. On the rMBP the mass-storage
// device sits on port 5, a USB3 port whose link finishes training ~1.6 s after the 100 ms pre-scan
// settle, so it MISSES the initial CCS scan and is recovered through the hot-plug path — on 30 boots
// out of 30, without one exception:
//
//     xHCI: !! ccs-margin LATE port=5 t_seen_ms=1659 settle_ms=100 short_by_ms<=1559
//           (missed the initial CCS scan; recovered via CSC; ...) result=CCSMARGIN-LATE
//     xHCI: [Port 5] device connected (hot-plug); queuing for enumeration.
//
// The boot volume and the intruder arrive down the SAME path on this machine. The axis carried no
// signal, and a guard built on it would have refused the flight recorder on every normal boot.
//
// The axis that DOES separate them is which volume the kernel booted FROM, and the handoff for it
// already exists and is metal-proven: the UEFI loader reads `BS_VolID` off LBA 0 of its own
// loaded-image device (`read_boot_volume_serial`, bootloader crate — LoadedImage -> DeviceHandle ->
// BlockIO) and passes it in `BootInfo::boot_volume_serial`. INSTALL-SELF already consumes it to stop
// the installer erasing the running system; this is the same identity asked a different question.
//
// So: a `Default` claimant may be WRITTEN only if it carries the boot volume's serial.
//   * AH / AI-1 — booted from the 60 GB card in the USB reader, which IS the Default claimant. Match,
//     writes allowed, byte-identical to trunk.
//   * AI-2 — booted from that same 60 GB card moved to the INTERNAL slot; the Default claimant is the
//     operator's 29 MiB Panasonic. Mismatch, writes refused. This is the defect.
//   * The re-enumeration case R1 had to charge as a fail-closed cost now costs nothing: a boot stick
//     that chirps and re-enumerates is still the same volume and still matches.
//
// The refusal is keyed on a PROOF, not on a failed identity test, and that distinction was forced by
// running the gate rather than reasoning about it. The first version of this predicate refused
// whenever the `Default` slot did not carry the boot serial. `UNAOS_SDHCBLK=1 ./arroyo test-fat`
// showed what that costs:
//
//     :: FRGUARD: Default slot (blocks=196608) does NOT carry the boot volume serial 0xfabe1afd …
//     :: FR: UNAOS.LOG NOT reserved …            <- 47 PASS -> 40 PASS, exit 1
//
// In the QEMU harness the boot ESP is deliberately a SEPARATE `ide-hd` and the kernel's `Default` is
// the `usb-storage` stick ("The boot ESP stays on the separate ide-hd", builder/src/main.rs), so the
// two can never match — and the kernel has no driver for the ESP, so the boot medium is not reachable
// through the block layer at all. "Default is not the boot medium" is therefore a perfectly normal
// state, not evidence of anything. (The same run also killed the assumption that QEMU has no `Sdhc`
// handle: it registers an emulated 16 MiB card, so `sdhc_info()` is `Some` there too.)
//
// So the guard refuses only in the state where it can POSITIVELY locate the boot volume on the other
// handle — `BM_SUBSTITUTED`. Everything else fails open:
//
//   * `sdhc_info().is_none()` — no canonical backend outside the global slot, nothing to substitute
//     away from. Byte-identical to the pre-FRGUARD constant.
//   * boot serial 0 — the loader could not identify the medium it booted from (a pre-guard loader, a
//     non-FAT boot path, an unstamped formatter, aarch64). Announced once, then trunk behaviour.
//   * `BM_UNPROVEN` — the serial is on neither handle. The QEMU case, and any machine booting from a
//     device the block layer cannot see.
//
// The guard must never be the thing that silently kills the flight recorder; an unidentified or
// unreachable boot volume is a reason to say so, not a reason to refuse. Same disarm direction
// INSTALL-SELF takes on the same sentinel.

/// FRGUARD: `BS_VolID` of the volume the kernel was loaded from, as the UEFI loader read it. 0 = absent
/// (the disarmed sentinel). Published once from the kernel entry path before any storage exists.
///
/// BOOTROOT (orin 22): **arch-neutral, on every target that has a loader** — the gate was
/// `cfg(x86_64 + sdhcblk)` when its only consumer was the FRGUARD verdict below.
///
/// ⚠ **It is NOT how the root is chosen, and nothing here should be read as saying it is.** Peter's
/// direction (2026-09-08) is that the kernel is told NOTHING about where it came from: "WTF does it
/// matter what method I choose to boot? You are assuming too much." `fs::bootdisk` decides what `/`
/// is by CONTENT — it enumerates every disk the board has and finds the one carrying the file whose
/// bytes are this kernel's own `.text` window — and a loader-supplied serial is exactly the kind of
/// hint that walk refuses to take. What this static is, and all it is: a SEAM. INSTALL-SELF and
/// FRGUARD read it to refuse the medium they booted from as an erase/substitute target, and
/// [`crate::fs::fat::locate_boot_volume`] offers the same datum to any caller that wants the volume
/// a loader NAMED (as opposed to the volume the kernel was FOUND on). Un-gated because that seam has
/// no arch in it, not because the root grew a dependency on it. The FRGUARD machinery around it
/// (`BM_*`, `evaluate_boot_medium_once`, `default_writable`) stays gated exactly as it was.
static BOOT_VOLUME_SERIAL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Not yet derived for the current occupant of the global slot.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
const BM_UNKNOWN: u8 = 0;
/// The global slot carries the boot volume's serial: it IS the medium we booted from.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
const BM_MATCH: u8 = 1;
/// The global slot does NOT carry it, and the `Sdhc` handle DOES — the boot medium is positively
/// located somewhere else, so a `Default` write is a proven substitution. The only refusing state.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
const BM_SUBSTITUTED: u8 = 2;
/// Neither handle carries the boot serial — the boot medium is not reachable through the block layer
/// at all, so nothing here can prove a substitution. FAILS OPEN. This is the normal state for a
/// machine that boots from a device the kernel has no driver for, which is exactly the QEMU harness:
/// the boot ESP is a separate `ide-hd` and the kernel's `Default` is the `usb-storage` stick.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
const BM_UNPROVEN: u8 = 3;

/// FRGUARD: the cached verdict on the CURRENT occupant of the global slot. Deriving it costs sector
/// READS (`fat::volume_serials` walks LBA 0, the GPT spans and every MBR partition start), which must
/// never happen from inside `write_block` — that call can arrive masked, mid-FAT-RMW, with the xHCI
/// loan already claimed. So the verdict is derived once from an unmasked main-loop context by
/// [`evaluate_boot_medium_once`] and only READ on the write path. Reset to `BM_UNKNOWN` by
/// [`publish_usb_geometry`], so a hot-plug that replaces the occupant forces a fresh derivation
/// instead of inheriting the previous device's verdict.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
static BOOT_MEDIUM_VERDICT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(BM_UNKNOWN);

/// FRGUARD: publish the boot volume serial from `BootInfo`. Called once from the kernel entry path,
/// before storage is up, so nothing can read a half-initialised guard. Deliberately separate from
/// `install::selfguard::set_boot_volume_serial`, which consumes the same field: that one is gated on
/// the installer features and is absent from every bench/boot build, which is exactly why its
/// `:: install: boot volume serial …` line appears ZERO times in the 30-boot capture. This one is not
/// gated on the installer, so the guard's own input is on the wire on every boot it can affect.
///
/// BOOTROOT (orin 22): un-gated, for the reason on [`BOOT_VOLUME_SERIAL`] — the seam it publishes is
/// arch-neutral, so the publication is too. It does NOT decide the root; `fs::bootdisk` does that by
/// content, and takes no hint from here. The witness text below is unchanged; on a target that
/// compiles no FRGUARD the "guard ARMED/DISARMED" clause reads as what it has always been, a
/// statement about the x86 substitution guard.
pub fn set_boot_volume_serial(serial: u32) {
    BOOT_VOLUME_SERIAL.store(serial, core::sync::atomic::Ordering::Release);
    if serial == 0 {
        serial_println!(
            ":: FRGUARD: boot volume serial ABSENT (0) — Default-write substitution guard DISARMED \
             (the loader could not identify the medium it booted from; writes behave exactly as trunk) ::"
        );
    } else {
        serial_println!(
            ":: FRGUARD: boot volume serial=0x{:08x} — Default-write substitution guard ARMED ::",
            serial
        );
    }
}

/// FRGUARD: the boot volume's `BS_VolID` as published by [`set_boot_volume_serial`], or 0 when the
/// loader could not identify the medium it booted from (the disarmed sentinel).
///
/// SDHC-4c reads it for its reserve witness ONLY — the line names the boot serial and the card's
/// volume serial side by side, so a capture shows whether the reserved extent lives on the medium
/// this kernel booted from, and the FRGUARD verdict above can be read together with it.
///
/// BOOTROOT (orin 22): un-gated on every arch, and STILL not a behavioural input to the root.
/// [`crate::fs::fat::locate_boot_volume`] is the one seam over it — "which enumerated source carries
/// the volume the loader named?" — and `shell::vfs_mount_table` deliberately does not call it:
/// `fs::bootdisk` finds the disk by comparing this kernel's `.text` against candidate files, so a
/// board that boots off USB one flight and the onboard slot the next needs no hint and gets none.
/// The sentence that used to end this doc ("Nothing keys behaviour off this getter") is therefore
/// still true, and is kept: SDHC-4c's bound is the LBA extent, not an identity test, and the root's
/// bound is a byte comparison, not a serial.
pub fn boot_volume_serial() -> u32 {
    BOOT_VOLUME_SERIAL.load(core::sync::atomic::Ordering::Acquire)
}

/// FRGUARD: derive the boot-medium verdict for the current global-slot occupant, at most once per
/// claim. MUST be called from an unmasked, lock-free context — it issues sector reads. The x86 main
/// loop's flight-recorder pass is the site that does so, ahead of every `Default` writer on that pass.
///
/// Idempotent and cheap after the first call (one relaxed load). Prints ONE line per derivation naming
/// the serials on both sides, so "the guard armed and agreed" is a fact in the capture rather than an
/// inference from silence.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
pub fn evaluate_boot_medium_once() {
    use core::sync::atomic::Ordering;
    if BOOT_MEDIUM_VERDICT.load(Ordering::Acquire) != BM_UNKNOWN {
        return; // already derived for this occupant
    }
    let boot_serial = BOOT_VOLUME_SERIAL.load(Ordering::Acquire);
    if boot_serial == 0 || sdhc_info().is_none() || info().is_none() {
        return; // disarmed, or nothing to judge yet — `default_writable` handles these directly
    }
    let glob = info().map(|d| d.num_blocks).unwrap_or(0);
    let canon = sdhc_info().map(|d| d.num_blocks).unwrap_or(0);
    let on_default = crate::fs::fat::volume_serials(crate::fs::fat::BlockSource::Default);
    if on_default.contains(&boot_serial) {
        BOOT_MEDIUM_VERDICT.store(BM_MATCH, Ordering::Release);
        serial_println!(
            ":: FRGUARD: Default slot (blocks={}) CARRIES the boot volume serial 0x{:08x} — it is the \
             medium this kernel booted from; writes ALLOWED (canonical Sdhc handle blocks={}) ::",
            glob, boot_serial, canon
        );
        return;
    }
    // The global slot is not the boot medium. That alone is NOT enough to refuse: a machine can
    // legitimately boot from a device the block layer cannot reach (no driver for it), in which case
    // nothing here is a substitution and refusing would be a guess. Refuse only when the boot volume
    // is POSITIVELY located on the other handle — that is a proof, not an absence of one.
    let on_sdhc = crate::fs::fat::volume_serials(crate::fs::fat::BlockSource::Sdhc);
    if on_sdhc.contains(&boot_serial) {
        BOOT_MEDIUM_VERDICT.store(BM_SUBSTITUTED, Ordering::Release);
        serial_println!(
            ":: FRGUARD: SUBSTITUTION — the boot volume serial 0x{:08x} is on the INTERNAL Sdhc card \
             (blocks={}), not in the Default slot (blocks={}, {} FAT volume(s), none of them ours); \
             Default writes REFUSED ::",
            boot_serial, canon, glob, on_default.len()
        );
    } else {
        BOOT_MEDIUM_VERDICT.store(BM_UNPROVEN, Ordering::Release);
        serial_println!(
            ":: FRGUARD: boot volume serial 0x{:08x} found on NEITHER handle (Default blocks={} has \
             {} FAT volume(s), Sdhc blocks={} has {}) — the boot medium is not reachable through the \
             block layer, so no substitution can be proven; writes ALLOWED (guard DISARMED) ::",
            boot_serial, glob, on_default.len(), canon, on_sdhc.len()
        );
    }
}

/// USBFALL F1 / FRGUARD (GR21): the x86 arm of the substitution guard. See the section comment above
/// for why the predicate is what it is, and for the refuted one it replaces.
///
/// Deliberately WRITES ONLY, exactly as the aarch64 arm: reads still fall through to the BOT path,
/// because a read cannot corrupt the wrong device, and the stick keeps its dedicated `read_block_usb`
/// handle. Pure reads of two atomics — no locks, no sector I/O — so it is safe from any context a
/// `write_block` can arrive in.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
pub fn default_writable() -> bool {
    use core::sync::atomic::Ordering;
    if sdhc_info().is_none() {
        return true; // no canonical backend outside the global slot — nothing to substitute away from
    }
    if BOOT_VOLUME_SERIAL.load(Ordering::Acquire) == 0 {
        return true; // boot medium unidentifiable — FAIL OPEN, announced once at publish time
    }
    // Exactly one state refuses: BM_SUBSTITUTED, where the boot volume was positively found on the
    // other handle. BM_UNKNOWN (not yet derived) and BM_UNPROVEN (boot medium unreachable) both fail
    // OPEN — the arc's law is that this guard never silently kills the recorder, and neither state is
    // evidence of a substitution.
    BOOT_MEDIUM_VERDICT.load(Ordering::Acquire) != BM_SUBSTITUTED
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
            ":: BLOCK: Default WRITE refused — the boot volume serial 0x{:08x} is on the internal \
             Sdhc card (blocks={}), not in the Default slot (blocks={}); refusing to write this \
             system's data to a medium it did not boot from (first, once) ::",
            BOOT_VOLUME_SERIAL.load(core::sync::atomic::Ordering::Acquire),
            canon,
            glob
        );
    }
    Err(BlockError::NotReady)
}

#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn guard_default_write_backend() -> Result<(), BlockError> {
    if default_writable() {
        return Ok(());
    }
    // SDHCRW (B166, R59): the opt-out is tested BEFORE the substitution message, and the two reasons
    // never share a line. They are not the same event and they do not send a reader to the same
    // place: `opt-out` is a build the operator asked for, `no-sd-backend` is a card that failed to
    // identify. The latch is shared because the rule ("say it once") is the same for both.
    if !SUBST_REFUSED.swap(true, Ordering::Relaxed) {
        if cfg!(feature = "sdw-ro") {
            serial_println!(
                ":: SDHCPOST: posture sdw-ro=1 driver=emmc2 -> sd=ro reason=opt-out \u{2014} \
                 refusing Default WRITE; the card registered and this build was asked to keep it \
                 read-only (SDHCRW, first, once) ::"
            );
        } else {
            serial_println!(
                ":: USBFALL: no SD backend registered — refusing Default WRITE (would land on the USB stick) ::"
            );
        }
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

    match xhci.storage_read10(read10_lba32("read", lba)?, 1) { // LBA32 (SR15): refuse, never truncate — see `read10_lba32` at the file tail.
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
    match xhci.storage_write10(read10_lba32("write", lba)?, 1) { // LBA32 (SR15): refuse, never truncate — a wrapped WRITE lands on someone else's sector.
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
    match xhci.storage_read10(read10_lba32("read", lba)?, count as u16) { // LBA32 (SR15): the COUNTED form truncated the head argument, so a wrap walked `count` sectors of the wrong disk.
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
    // left the Boot AI-2 write path wide open. The matching aarch64 gap is recorded in
    // `docs/SECURITY.md` and `usb_xhci.md` §11 rather than here, so the Pi lane can find it.
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
    match xhci.storage_write10(read10_lba32("write", lba)?, count as u16) { // LBA32 (SR15): the COUNTED form truncated the head argument, so a wrap OVERWROTE `count` sectors of the wrong disk.
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
    read_blocks_usb_ix(0, lba, buf)
}

/// USBREG: the counted read, addressed at registry entry `ix`'s own `(slot_id, lun)`.
pub fn read_blocks_usb_ix(ix: usize, lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let disk = usb_disk(ix).ok_or(BlockError::NotReady)?;
    let dev = disk.info;
    let count = span_blocks(&dev, lba, buf.len())?;
    // WEDGE-8: claim the controller as a LOAN — no lock is held across the BOT transaction below.
    // `claim_xhci_for_io` returns `Busy` immediately to a masked caller (a masked wait on a driver
    // lock IS the F3 deadlock) and otherwise waits its own bounded, unmasked budget.
    let mut loan = claim_xhci_for_io()?;
    let xhci = &mut *loan;
    match xhci.storage_read10_on(dev.slot_id, disk.lun, read10_lba32("read-usb", lba)?, count as u16) { // LBA32 (SR15): counted form, same rule as the single-block twin above.
        Ok(res) if res.status == CswStatus::Passed => {}
        other => {
            io_cause_witness("read-usb", lba, other);
            return Err(BlockError::Io);
        }
    }
    let src = xhci.storage_data_ptr_on(dev.slot_id).ok_or(BlockError::Io)?;
    unsafe { core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), buf.len()); }
    Ok(buf.len())
}

/// MULTIBLK: the counted twin of [`write_block_usb`]. Same guard ladder as the single-block form —
/// geometry re-read on every call, span bounded against `num_blocks`, a non-`Passed` CSW propagated
/// as [`BlockError::Io`] so a write NEVER reports a false success.
pub fn write_blocks_usb(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    write_blocks_usb_ix(0, lba, buf)
}

/// USBREG: the counted write, addressed at registry entry `ix`'s own `(slot_id, lun)`.
pub fn write_blocks_usb_ix(ix: usize, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let disk = usb_disk(ix).ok_or(BlockError::NotReady)?;
    let dev = disk.info;
    let count = span_blocks(&dev, lba, buf.len())?;
    // WEDGE-8: claim the controller as a LOAN — no lock is held across the BOT transaction below.
    // `claim_xhci_for_io` returns `Busy` immediately to a masked caller (a masked wait on a driver
    // lock IS the F3 deadlock) and otherwise waits its own bounded, unmasked budget.
    let mut loan = claim_xhci_for_io()?;
    let xhci = &mut *loan;
    let dst = xhci.storage_data_ptr_on(dev.slot_id).ok_or(BlockError::Io)?;
    unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, buf.len()); }
    match xhci.storage_write10_on(dev.slot_id, disk.lun, read10_lba32("write-usb", lba)?, count as u16) { // LBA32 (SR15): counted form, same rule as the single-block twin above.
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
/// alongside [`BLOCK_DEVICE`] and [`USB_DISKS`]: nothing here ever claims the global, so the
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
    crate::bootlog_println!(
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
    crate::fs::bootstep::note_read(1); // BOOT80 (B350): the step lines count what reached the card
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
    #[cfg(feature = "sdw")] crate::drivers::sdhc::wr_census_idle(); // SDHCMULTI M1: a read after a quiet gap closes the write burst
    crate::fs::bootstep::note_read(count as u64); // BOOT80 (B350)
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
/// **SDHC-4c (supersedes 4b's "not reachable from the FAT layer"):** `fs::fat::write_sector` /
/// `write_sectors` now route a `Sdhc` source here — but ONLY after `fs::sdhc4c::permit_write` has
/// admitted the span, i.e. only for LBAs inside the one reserved extent published at boot. Every
/// other LBA on the card is refused above this function, before any card lock is taken. See
/// `fs/sdhc4c.rs` for the writable set and the argument that it is closed. The other route,
/// [`PartitionRange::write_block`] with an explicitly `Sdhc`-handled range, is still called by
/// nothing in `fs/fat.rs`.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", feature = "sdw"))]
pub fn write_block_sdhc(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let dev = sdhc_info().ok_or(BlockError::NotReady)?;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
    }
    crate::fs::bootstep::note_write(1); // BOOT80 (B350)
    let t0 = crate::drivers::sdhc::wr_census_begin(); // SDHCMULTI M1: the census times the whole call
    let r = crate::drivers::sdhc::write_block_512(lba, buf);
    crate::drivers::sdhc::wr_census_note(t0, 1, false, 0, r.is_ok());
    r
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
/// for the Pi's `emmc2`). SDHCPOST: CMD25 now DOES lift it, under `sdw`, and the seam above is unchanged.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
pub fn write_blocks_sdhc(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let dev = sdhc_info().ok_or(BlockError::NotReady)?;
    let count = span_blocks(&dev, lba, buf.len())?;
    let bs = dev.block_size as usize; #[cfg(all(feature = "sdw", not(feature = "sdw-ro")))] if count > 1 && bs == SECTOR_BYTES { return write_blocks_sdhc_mb(lba, count, &buf[..count * bs]); } // SDHCPOST (B155): a COUNTED write is ONE CMD25, not `count` CMD24s — §SDHCPOST at this file's TAIL carries the measurement that decided it (a 15.5 MB capture is ~31,700 single-block writes against ~484 multi-block ones, because `fat::write_sectors` chunks at MAX_BLOCKS_PER_OP=64 and `sdhc::MB_MAX_BLOCKS` is 64 too). SDHCRW (B166, R59): this route now rides the DEFAULT x86 image — `sdw` is default-on and `sdw-ro` is the opt-out — so the counted write a file verb issues IS one CMD25 on a plain `./arroyo test`. The loop below remains the single-sector path AND the whole of the opt-out shape.
    for i in 0..count {
        write_block_sdhc(lba + i as u64, &buf[i * bs..(i + 1) * bs])?;
    }
    Ok(())
}

// ============ TEGRA-SDBLK — the Orin's microSD as its own registry slot (read-only) ============
//
// `arch::aarch64::sdmmc_tegra` has, since ORIN-SDMMC-1, identified the card in the Orin devkit's
// microSD slot and read sectors from it — and told nobody. Every SD arm in this file is
// `#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]`, i.e. the Pi's emmc2, so on a tegra
// build `block::read_block` has always meant the USB stick and nothing else, and `fs::unafs`'s
// `SdSectorDevice::open()` could only ever answer `NoStorage`. A proven driver with no consumer —
// the same shape SDHC-4b found on x86, and this section is its aarch64 twin
// (docs/dev/OS/10_INSTALL/orin-unafs-root.md §3 item 1).
//
// ### Its own slot, NOT the global, and NOT the `BACKEND` selector
// The Orin boots from a USB stick, so on this board `BLOCK_DEVICE` is the stick and `block::info()`
// is what the installer, `fat::probe_once`, the shell and unafs all read. Publishing the card into
// the global would silently re-point every one of them at a different disk — PI-FS-2, exactly. The
// `BACKEND` selector is not an option either: it is `baremetal`-gated (it exists to route the Pi's
// emmc2) and does not compile on a tegra build at all. So the card gets its own entry, reached only
// by naming it, precisely as `Usb` (PIUSB-27) and `Sdhc` (SDHC-4b) are — and `read_block`,
// `write_block`, `read_blocks`, `write_blocks` and `publish_usb_geometry` gain not one statement.
//
// ### The follow-up step: `BlockHandle::SdMmc` (LANDED — see the variant above)
// SDHC-4b's precedent is a new HANDLE variant, and it is now here. It was held back one commit
// because `BlockHandle` is total-by-construction, and MEASURED (variant added,
// `UNAOS_TEGRA=1 ./arroyo check`, variant removed again) the arm-tegra leg reports E0004 at twelve
// sites: nine in this file (`alternate_program_source`, `lookup`, `mbr_census`'s two, `PartitionRange`'s
// five) and three OUTSIDE that arc's lane — `fs::fat::source_of` and `install/mod.rs`'s two handle
// matches, plus `wifi::firmware::source_of_handle` as a thirteenth. That totality is a feature, not
// an obstacle: it is the forcing function those files document ("a fourth handle is a build error in
// both places, not a silent mis-route"), and it fired exactly when the variant landed. Every one of
// those arms is mechanical — the reads route here, the writes refuse here, and nothing that could
// already serve a source changed.
//
// Nothing about the read path below changed when the variant arrived. The variant only lets a
// `PartitionRange` CARRY the card, so a partition on it is addressed through the bounded,
// partition-relative gate instead of by a caller doing `start + rel` for itself. A caller may still
// name `tegra_sd_info()` and `read_block(s)_tegra_sd` directly; those are unchanged.
//
// ### What the variant deliberately does NOT do: it is not a program-source rung
// `program_source` still returns the global slot and nothing else on this board, and
// `alternate_program_source` maps `SdMmc` to `None`. The Orin boots from a USB stick; making the
// card a program source would re-point `mount_program_source`, the shell's file verbs and the exec
// path at a different disk — the PI-FS-2 hazard this whole section is written to avoid. The card is
// censused (`SourceCensus::tegra_sd`) and readable by NAME; binding a filesystem to it is
// `orin-unafs-root.md` §3 item 4's job, behind its own knob.
//
// ### WRITES ARE REFUSED HERE, in every cfg
// The Orin's card-write path is the `sdmmc_arm` ladder in `sdmmc_tegra.rs`, behind three explicit
// gates (`sdmmc` + `sdmmc_arm` + `install_target` for the destructive flow), each of which the
// operator arms deliberately. Registering the card as a block backend must not open a fourth door
// past that ladder, so the write entry points below exist ONLY to refuse — fail-closed, with a
// one-shot witness that names the reason. There is no cfg in which they write.

/// TEGRA-SDBLK: geometry of the card in the Orin's microSD slot, published by [`register_tegra_sd`]
/// once `sdmmc_census` has identified it AND read sector 0 from it. A THIRD slot alongside
/// [`BLOCK_DEVICE`] and [`USB_DISKS`]: nothing here ever claims the global, so the boot volume
/// every existing caller reads through `info()` is untouched. `None` until registration.
#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
pub static TEGRA_SD_BLOCK_DEVICE: Mutex<Option<BlockDeviceInfo>> = Mutex::new(None);

/// TEGRA-SDBLK: snapshot of the Orin microSD's geometry, if one registered this boot.
#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
pub fn tegra_sd_info() -> Option<BlockDeviceInfo> {
    *TEGRA_SD_BLOCK_DEVICE.lock()
}

/// TEGRA-SDBLK: one-shot latch so the publish witness is printed exactly once per boot even if the
/// census were ever to run twice (it is called from two boot paths today — the JC3 early site and the
/// main aarch64 entry — and only the first one to succeed may claim the line).
#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
static TEGRA_SD_PUBLISHED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// TEGRA-SDBLK: publish the Orin microSD's geometry under the [`TEGRA_SD_BLOCK_DEVICE`] slot.
///
/// Called once, from the end of `sdmmc_tegra::sdmmc_census`, after M1 (mapped window, live SDHCI),
/// M2 (card identified) and M3 (sector 0 read) have all succeeded — so a registered card is one whose
/// read path this boot has already exercised and reported on, not merely one that answered CMD2/CMD3.
///
/// Returns whether the card was registered. It refuses `num_blocks == 0` rather than publishing a
/// device every subsequent bound check would reject one call later — an instrument that can say NO.
///
/// `slot_id` is 0, the sentinel `register_sd` stamps on the Pi's microSD: 0 is never a live xHCI
/// device slot, so [`unpublish_usb_geometry`] can never retract this entry on a USB disconnect (belt
/// and braces — that function only touches the `Global` and `Usb` slots anyway).
#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
pub fn register_tegra_sd(num_blocks: u64, block_addressing: bool) -> bool {
    if num_blocks == 0 {
        serial_println!(
            ":: SDMMC: REFUSED to publish the microSD block backend — num_blocks=0 (nothing to address) ::"
        );
        return false;
    }
    let dev = BlockDeviceInfo {
        slot_id: 0,
        block_size: SECTOR_BYTES as u32,
        num_blocks,
        vendor: *b"TEGRA-SD",
        product: if block_addressing { *b"MICROSD SDHC/XC " } else { *b"MICROSD SDSC    " },
    };
    *TEGRA_SD_BLOCK_DEVICE.lock() = Some(dev);
    if !TEGRA_SD_PUBLISHED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        #[cfg(feature = "sdwrite")] serial_println!(":: SDMMC: block backend published — {} sectors (read-write — the block layer admits ordinary writes; SDWRITE) ::", num_blocks); #[cfg(not(feature = "sdwrite"))] serial_println!(
            ":: SDMMC: block backend published — {} sectors (read-only) ::",
            num_blocks
        );
    }
    true
}

/// TEGRA-SDBLK: read one sector (`lba`) from the Orin microSD DIRECTLY through the SDMMC driver's
/// polled CMD17, bypassing the backend selector — the read twin of [`read_block_usb`]. Geometry comes
/// from [`TEGRA_SD_BLOCK_DEVICE`], re-read on every call as every other entry point does, and the
/// driver re-guards the LBA against the card's own capacity one layer down. Takes no xHCI lock at all,
/// so it cannot interact with the boot stick's transport.
#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
pub fn read_block_tegra_sd(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let dev = tegra_sd_info().ok_or(BlockError::NotReady)?;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
    }
    crate::arch::aarch64::sdmmc_tegra::tegra_sd_read_block_512(lba, buf)
}

/// TEGRA-SDBLK: the counted twin of [`read_block_tegra_sd`] — `buf.len() / block_size` consecutive
/// sectors in one call, bounded by the shared [`span_blocks`] rules so the bound cannot drift between
/// handles. The driver loops CMD17 underneath (it has no unarmed CMD18 primitive); the cap applied
/// here is [`MAX_BLOCKS_PER_OP`], the same number `fs/fat.rs` already chunks every source against —
/// one cap for all slots is one rule to get right instead of three.
#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
pub fn read_blocks_tegra_sd(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let dev = tegra_sd_info().ok_or(BlockError::NotReady)?;
    let count = span_blocks(&dev, lba, buf.len())?;
    let n = crate::arch::aarch64::sdmmc_tegra::tegra_sd_read_blocks_512(lba, count, buf)?;
    // A short counted read is an error, never a silent prefix — the same rule the other counted forms
    // enforce, and the one failure mode a filesystem above has no way to notice.
    if n < buf.len() {
        return Err(BlockError::Io);
    }
    Ok(buf.len())
}

/// TEGRA-SDBLK: one-shot latch so the write refusal names itself once instead of per retry.
#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
static TEGRA_SD_WRITE_REFUSED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// TEGRA-SDBLK: the block layer REFUSES to write the Orin microSD — in every cfg, including a fully
/// armed one. This is not a stub awaiting a driver: `sdmmc_tegra` HAS a metal-proven CMD24/CMD25 write
/// path, and it is reachable only through the `sdmmc_arm` paranoia ladder and the `install_target`
/// flow, each of which stashes, writes, verifies and restores under its own gates. Publishing the card
/// as a readable block device must not become a fourth way past that ladder, so this entry point exists
/// so the seam is total and fails CLOSED, not so that anything writes through it.
#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
pub fn write_block_tegra_sd(_lba: u64, _buf: &[u8]) -> Result<(), BlockError> { #[cfg(all(feature = "sdwrite", not(feature = "sdw-ro")))] { return tegra_sd_write_through(_lba, _buf); } #[cfg(any(not(feature = "sdwrite"), feature = "sdw-ro"))] { // SDWRITE (A60): the knob-off arm below is the pre-A60 body, byte-for-byte; the route above is the only addition. SDHCRW (B166, R59): the OPT-OUT takes the same arm, and it has to be HERE and not only in `tegra_sd_writes_admitted` — a posture that reported `ro` at the mount while this entry point still wrote would be two policies again, the exact drift A60's one-definition rule exists to stop. The refusal line below names which of the two it is.
    if !TEGRA_SD_WRITE_REFUSED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        serial_println!(
            ":: SDMMC: posture sdw-ro={} -> sd=ro reason={} \u{2014} WRITE refused at the block \
             layer; the Orin card's only writer is the armed sdmmc_arm ladder in sdmmc_tegra.rs \
             (first, once) ::",
            if cfg!(feature = "sdw-ro") { 1 } else { 0 },
            if cfg!(feature = "sdw-ro") { "opt-out" } else { "no-sdwrite" }
        );
    }
    // `NotReady` is the refusal code SDHC-4b's disarmed write twin returns for the same situation —
    // "this build has no writer for this device" — and `BlockError` gains no variant here, so no match
    // anywhere in the tree changes shape.
    Err(BlockError::NotReady)
} }

/// TEGRA-SDBLK: the counted twin of [`write_block_tegra_sd`], refusing for the same reason. Present so
/// that a future `BlockHandle::SdMmc` dispatch has a total set of four entry points to name.
#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
pub fn write_blocks_tegra_sd(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    #[cfg(feature = "sdwrite")] { return tegra_sd_write_blocks_through(lba, buf); } #[cfg(not(feature = "sdwrite"))] { write_block_tegra_sd(lba, buf) }
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
        // TEGRA-SDBLK: its own latch bit for the same reason `Sdhc` has one — the Orin card's table
        // is censused once on its own terms and can neither suppress nor be suppressed by the boot
        // stick's census. Disjoint from bit 4 by construction: no build carries both handles.
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        BlockHandle::SdMmc => 8, #[cfg(all(target_arch = "x86_64", feature = "ahci"))] BlockHandle::Ahci { .. } => 16, #[cfg(all(target_arch = "x86_64", feature = "nvme"))] BlockHandle::Nvme => 32, // AHCIBOOT: one latch bit for the whole SATA handle, so a two-disk rMBP censuses once rather than per port.
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
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        BlockHandle::SdMmc => "tegra-sd", #[cfg(all(target_arch = "x86_64", feature = "ahci"))] BlockHandle::Ahci { .. } => "ahci", #[cfg(all(target_arch = "x86_64", feature = "nvme"))] BlockHandle::Nvme => "nvme",
    };

    // --- RAW, before decoding anything: the signature word and the four 16-byte entries verbatim.
    crate::census_println!(
        ":: PART: mbr-raw handle={} dev_blocks={} sig={:02x}{:02x} ::",
        name, dev_blocks, sec[MBR_SIG_OFF], sec[MBR_SIG_OFF + 1]
    );
    for i in 0..4 {
        let o = MBR_TABLE_OFF + i * MBR_ENTRY_LEN;
        let e = &sec[o..o + MBR_ENTRY_LEN];
        crate::census_println!(
            ":: PART: mbr-raw handle={} e{} = {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
            name, i + 1,
            e[0], e[1], e[2], e[3], e[4], e[5], e[6], e[7],
            e[8], e[9], e[10], e[11], e[12], e[13], e[14], e[15]
        );
    }

    // --- Decoded verdict, per slot, then the summary.
    let Some(t) = table else {
        crate::census_println!(":: PART: mbr census handle={} sig=absent — not an MBR ::", name);
        return table;
    };
    for slot in 1..=4u8 {
        match (t.entry(slot), t.reject(slot)) {
            (Some(p), _) => crate::census_println!(
                ":: PART: mbr handle={} slot={} type=0x{:02x} boot=0x{:02x} start={} count={} end={} ACCEPT ::",
                name, slot, p.type_byte, p.boot_flag, p.start_lba, p.sector_count, p.end_lba()
            ),
            (None, Some(r)) => crate::census_println!(
                ":: PART: mbr handle={} slot={} REJECT {:?} ::", name, slot, r
            ),
            (None, None) => {}
        }
    }
    crate::census_println!(
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
            // TEGRA-SDBLK: the point of the variant. A `PartitionRange` built on the Orin card now
            // reaches the sdmmc driver's polled CMD17 through the same bounded, partition-relative
            // gate every other handle goes through — no second addressing path, no absolute LBA
            // computed by a caller.
            #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
            BlockHandle::SdMmc => read_block_tegra_sd(abs, buf), #[cfg(all(target_arch = "x86_64", feature = "ahci"))] BlockHandle::Ahci { port } => read_block_ahci_port(port, abs, buf), #[cfg(all(target_arch = "x86_64", feature = "nvme"))] BlockHandle::Nvme => read_block_nvme(abs, buf),
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
            // TEGRA-SDBLK: REFUSES, in every cfg. `write_block_tegra_sd` is the refusal itself (a
            // one-shot witness + `NotReady`), so the variant cannot become a fourth door past the
            // armed `sdmmc_arm` ladder — a range that carries the card is readable and nothing more.
            #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
            BlockHandle::SdMmc => write_block_tegra_sd(abs, buf), #[cfg(all(target_arch = "x86_64", feature = "ahci"))] BlockHandle::Ahci { port } => write_block_ahci_port(port, abs, buf), #[cfg(all(target_arch = "x86_64", feature = "nvme"))] BlockHandle::Nvme => write_block_nvme(abs, buf), // AHCIBOOT: REFUSES, in every cfg — `write_block_ahci` is the refusal and the image carries no ATA write opcode.
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
            #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
            BlockHandle::SdMmc => read_blocks_tegra_sd(abs, buf), #[cfg(all(target_arch = "x86_64", feature = "ahci"))] BlockHandle::Ahci { port } => read_blocks_ahci_port(port, abs, buf), #[cfg(all(target_arch = "x86_64", feature = "nvme"))] BlockHandle::Nvme => read_blocks_nvme(abs, buf),
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
            // TEGRA-SDBLK: refuses, as the single-sector twin above does and for the same reason.
            #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
            BlockHandle::SdMmc => write_blocks_tegra_sd(abs, buf), #[cfg(all(target_arch = "x86_64", feature = "ahci"))] BlockHandle::Ahci { port } => write_blocks_ahci_port(port, abs, buf), #[cfg(all(target_arch = "x86_64", feature = "nvme"))] BlockHandle::Nvme => write_blocks_nvme(abs, buf), // AHCIBOOT: refuses, as the single-sector twin does.
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
            #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
            BlockHandle::SdMmc => tegra_sd_info(), #[cfg(all(target_arch = "x86_64", feature = "ahci"))] BlockHandle::Ahci { port } => ahci_info_port(port), #[cfg(all(target_arch = "x86_64", feature = "nvme"))] BlockHandle::Nvme => nvme_info(),
        }
        .ok_or(BlockError::NotReady)?;
        let bs = dev.block_size as usize;
        if bs == 0 || len == 0 || len % bs != 0 {
            return Err(BlockError::Io);
        }
        Ok((len / bs) as u64)
    }
}

// ===================== SDWRITE (A60, 2026-09-12) — the card's write POSTURE, in one place =====================
//
// WHAT THIS SECTION IS FOR. Until A60 the Orin microSD had no writer outside the armed `sdmmc_arm`
// ladder, and `write_block_tegra_sd` above said so by refusing in EVERY cfg. `fs/fat.rs`'s
// `BlockSource::SdMmc` veto and `fs/bootdisk.rs`'s `rw=` witness only REPORTED that refusal; neither
// was the refusal. LOGIN M2 needs a writable `/` on a slot-booted Orin, so the refusal has to be
// liftable — and the one thing that must not happen while lifting it is the wire saying `rw=yes` over
// a path that still cannot write a byte. Hence: ONE posture, decided here, forwarded everywhere.
//
// THE POLARITY, and why it is this way round. `sdwrite` is DEFAULT-ON (`unaos/arroyo` composes it into
// every build unless `UNAOS_NOSDWRITE=1`), so the SHIPPED configuration is the one that writes and the
// knob-off configuration is the pre-A60 refusal kept verbatim. LAWS §5's default-quiet rule is about
// the polarity a board actually boots: the shipped polarity here is ON, and `KERNEL_CFG_MATRIX` gains
// `arm-tegra-sdwrite` / `arm-tegra-sdwritefx` so the shipped polarity is type-checked WITH the board
// features, while every pre-existing `arm-tegra-*` leg already type-checks the OFF polarity.
//
// THE APPEND IS AT THE FILE TAIL, not in the middle of the file, so no `core::panic::Location` line in
// `block.rs` moves and `./arroyo knoboff sdwrite <baseline>` can come out byte-identical. Every edit
// this arc made ABOVE this line is line-for-line in place, for the same reason.

/// SDWRITE: does the block layer admit an ORDINARY write (a file mutation, not the armed ladder) to
/// the Orin microSD? The single definition of that answer. `fs/fat.rs`'s `BlockSource::SdMmc` veto
/// arm forwards to it instead of stating a second policy, so the report and the behaviour cannot
/// drift — which is exactly how the pre-A60 tree came to have a veto in `fat.rs` that only ECHOED a
/// refusal living two layers down.
///
/// SDHCRW (rmbp-ledger B166, R59, 2026-09-22): the board-wide OPT-OUT reaches here too. R59 is
/// *"read write"* **on every board**, and the corollary is that the one named escape must be the
/// same name on every board — an operator arming a cold-witness boot should not have to learn a
/// third spelling for the Orin. `sdwrite` stays default-on (A60, unchanged) and `sdw-ro`
/// (`UNAOS_SDW_RO=1`) now vetoes on top of it. Still a `const fn` and still only `cfg!`: this
/// driver's posture has no pin to read, so nothing here became a runtime question. UNFLOWN on the
/// armed side and it stays that way — R39, no Orin QEMU exists; the leg that proves it is a compile.
#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
pub const fn tegra_sd_writes_admitted() -> bool {
    cfg!(feature = "sdwrite") && !cfg!(feature = "sdw-ro")
}

/// SDWRITE: one 512-byte sector to the card, through the driver's unarmed CMD24 primitive.
///
/// Bounds are checked HERE against the published geometry (as [`read_block_tegra_sd`] does) and again
/// in the driver against the card's own capacity, because this function names an LBA to a medium and
/// must not depend on a caller's care. The one-shot witness uses the SAME latch the refusal used, so a
/// boot says once, on the wire, which posture it is running — and an operator reading a capture never
/// has to infer the posture from the absence of a line.
#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc", feature = "sdwrite"))]
pub fn tegra_sd_write_through(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    if !TEGRA_SD_WRITE_REFUSED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        serial_println!(
            ":: SDMMC: WRITE admitted at the block layer — ordinary file mutation routes to the \
             unarmed CMD24 primitive (SDWRITE; first, once) ::"
        );
    }
    let dev = tegra_sd_info().ok_or(BlockError::NotReady)?;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
    }
    if buf.len() < SECTOR_BYTES {
        return Err(BlockError::Io);
    }
    crate::arch::aarch64::sdmmc_tegra::tegra_sd_write_block_512(lba, &buf[..SECTOR_BYTES])
}

/// SDWRITE: the counted twin — `buf.len() / block_size` consecutive sectors, bounded by the shared
/// [`span_blocks`] rules so the cap cannot drift from the read side's. The driver has no unarmed CMD25
/// primitive (the counted write in `sdmmc_tegra.rs` is `sdmmc_arm`-gated and stays there), so this
/// LOOPS the proven CMD24 exactly as [`read_blocks_tegra_sd`] loops CMD17 — same order, same traffic a
/// per-sector caller would produce. A partial run returns the FIRST error: the sectors before it are
/// on the medium, which is the same truth a per-sector caller would have had.
#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc", feature = "sdwrite"))]
pub fn tegra_sd_write_blocks_through(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let dev = tegra_sd_info().ok_or(BlockError::NotReady)?;
    let count = span_blocks(&dev, lba, buf.len())?;
    for i in 0..count {
        let off = i * SECTOR_BYTES;
        tegra_sd_write_through(lba + i as u64, &buf[off..off + SECTOR_BYTES])?;
    }
    Ok(())
}

/// SDWRITE: why the NATIVE root refuses ordinary file mutation, or `None` if it accepts it — the
/// answer [`crate::fs::vfs::NativeBackend::write_veto`] forwards.
///
/// **The second finding of ledger row A60, in code.** `fs/bootdisk.rs`'s native arm used to sample
/// `BlockSource::write_veto()` — a question about a `BlockSource` — for a mount it binds as a
/// `NativeBackend`, two different objects; and `NativeBackend::write_veto` itself returned a flat
/// `None`, i.e. "this volume is always writable", which is a claim no layer had checked. This function
/// is the check: it asks the block layer about the handle the shared unafs mount is ACTUALLY riding
/// (`fs::unafs::mount_bound_handle`, the UNAFSBIND discovery's own answer), so `rw=yes` on the root
/// mount is a statement about the disk under it.
///
/// EXHAUSTIVE on `BlockHandle` on purpose: a new handle is an E0004 here, which drags whoever adds it
/// to the one place the root's posture is decided.
///
/// THE UNBOUND CASE, stated rather than hidden. `None` from `mount_bound_handle` means nothing has
/// mounted the native volume yet, so no disk has been chosen and there is no handle to ask about. It
/// answers `None` (admit), which is EXACTLY what this code path answered before A60 in every state —
/// so the unbound answer cannot be a regression — and the block layer's own refusal still stands one
/// layer down, where the write would meet it. It is reachable only for a handle whose bound answer
/// would have been `Some`, and in every configuration that compiles this function the only such
/// handles are `Global` under a false `default_writable()` (FRGUARD / USBFALL F1, which the Pi's
/// bare-metal media do not carry `sdwrite` to reach) and x86's `Sdhc` (where `NativeBackend` does not
/// exist at all). Forcing the bind here to close it would run `with_unafs` from inside a predicate —
/// a side effect, and a lock this function has no business taking on a caller's behalf.
/// `target_arch = "aarch64"`-gated because `fs::unafs` is (`fs/mod.rs:43`), and `NativeBackend` —
/// the only caller — is gated the same way.
#[cfg(all(any(target_arch = "aarch64", feature = "unafs"), feature = "sdwrite"))]
pub fn native_mount_write_veto() -> Option<&'static str> {
    let Some(handle) = crate::fs::unafs::mount_bound_handle() else {
        return None;
    };
    #[cfg(all(target_arch = "x86_64", feature = "ahciroot"))] if let BlockHandle::Ahci { port } = handle { if crate::install::ahciroot::root_port() == Some(port) { return None; } } #[cfg(all(target_arch = "x86_64", feature = "nvme"))] if handle == BlockHandle::Nvme && nvme_root_grant().is_some() { return None; } // AHCIROOT (B332): the native root on a root-granted SSD is writable (the grant bounds it to p2); the per-HANDLE answer below is unchanged, so leg 8's FAT/handle agreement is untouched
    handle_write_veto(handle)
}

/// SDWRITE: may an ORDINARY file mutation reach the medium behind `handle`? The block layer's own
/// answer, per handle, with no volume and no filesystem in it — so it can be driven over every handle
/// by a fixture instead of only observed on whichever one a boot happens to have bound.
///
/// This is the predicate `fs/fat.rs`'s `BlockSource::write_veto` is a per-SOURCE view of, and leg 8
/// (`fs::bootdisk::sdwrite_posture_selftest`) asserts the two agree for every source in
/// `fat::ALL_SOURCES`. Two views of one answer is the shape that drifts; the fixture is what stops it.
#[cfg(feature = "sdwrite")]
pub fn handle_write_veto(handle: BlockHandle) -> Option<&'static str> {
    match handle {
        // USBFALL F1 / FRGUARD: the same predicate `write_block` itself enforces, asked of the same
        // slot — never a second copy of the policy.
        BlockHandle::Global => {
            if default_writable() {
                None
            } else {
                Some(NATIVE_GLOBAL_VETO)
            }
        }
        // USB-WRITE (F3): the stick's BOT WRITE(10) path is verified and routed, for every registry
        // index (`write_block_usb_ix`), so there is one answer and it is yes.
        BlockHandle::Usb => None,
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        BlockHandle::Sdhc => sdhc_write_veto(NATIVE_SDHC_VETO), // SDHCPOST (B155): a FORWARD to §SDHCPOST's one definition, never a second policy — `fs/fat.rs`'s `BlockSource::Sdhc` arm forwards to the SAME function, which is what makes leg 8 of `fs::bootdisk::sdwrite_posture_selftest` agree BY CONSTRUCTION instead of by two authors keeping two arms in step. It was `Some(NATIVE_SDHC_VETO)` — a second copy of the policy, and exactly the drift this arc's own fixture is built to catch.
        // A60: the posture this arc exists to make liftable, forwarded from its one definition.
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        BlockHandle::SdMmc => {
            if tegra_sd_writes_admitted() {
                None
            } else {
                Some(NATIVE_TEGRA_SD_VETO)
            }
        } #[cfg(all(target_arch = "x86_64", feature = "ahci"))] BlockHandle::Ahci { .. } => Some(NATIVE_AHCI_VETO), #[cfg(all(target_arch = "x86_64", feature = "nvme"))] BlockHandle::Nvme => Some(NATIVE_NVME_VETO), // AHCIBOOT: a flat NO with no exception to carve — `write_block_ahci` refuses in every cfg and the image compiles no ATA write opcode. A forward to that standing answer, not a second policy.
    }
}

/// SDWRITE: the native root's refusal when the global slot is not writable. Named separately from
/// `fs/fat.rs`'s `DEFAULT_VETO` because it is printed about a DIFFERENT mount (the native volume, not
/// a FAT one) and a reader chasing it needs to land on this function, not on that one.
#[cfg(feature = "sdwrite")]
const NATIVE_GLOBAL_VETO: &str = "the native root rides the global slot and the block layer refuses \
                                  writes through it (FRGUARD / USBFALL F1)";
#[cfg(all(feature = "sdwrite", target_arch = "x86_64", feature = "sdhcblk"))]
const NATIVE_SDHC_VETO: &str = "the native root rides the internal SD reader, which admits a write \
                                only inside the reserved flight-recorder extent (SDHC-4c)";
#[cfg(all(feature = "sdwrite", target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
const NATIVE_TEGRA_SD_VETO: &str = "the native root rides the microSD and this build refuses every \
                                    write to it (UNAOS_SDW_RO=1 opt-out, or UNAOS_NOSDWRITE=1 / no \
                                    `sdwrite` feature)";

// ══════════ AHCI (rmbp-ledger B89, first rung) — the SATA disks as their own registry ════════════
//
// `drivers/ahci.rs` brings up every implemented AHCI port that holds a plain SATA disk, IDENTIFYs
// it and reads sectors from it. This section is where those disks become BLOCK DEVICES.
//
// ### It is an ARRAY, keyed on the AHCI PORT, and the key comes from the ENUMERATOR
//
// USBREG's rule, applied one controller over: the key is the port index the HBA's `PI` mask handed
// us, never anything read off the medium. A machine with two SATA disks (which the bench rMBP is —
// B89 names both) publishes two entries, and two disks imaged off each other cannot merge, because
// nothing here ever compares content. The registry INDEX is an address and is stable for the boot;
// this arc has no hot-plug and therefore no retraction, so it also has no holes.
//
// ### It is READ-ONLY, and that is a property of the image, not a policy
//
// `write_block_ahci` refuses unconditionally with a one-shot witness, and it can afford to be that
// blunt because `drivers/ahci.rs` compiles no WRITE opcode at all — the whole file can issue
// exactly `IDENTIFY DEVICE` (0xEC) and `READ DMA EXT` (0x25). The refusal is the honest answer a
// total dispatch needs, in the same shape and for the same reason as the pre-`sdw` SDHC stub, not a
// switch someone can flip. Writes are the next arc.
//
// ### It deliberately does NOT claim the global slot, and no installer can name it
//
// `register_ahci` never touches [`BLOCK_DEVICE`]: the boot volume every existing caller reads
// through [`info`] is untouched, exactly as SDHC-4b's third handle leaves it untouched. And nothing
// in `install/` can reach these entries — rmbp-ledger B91 is why: the internal SSD this driver
// enumerates carries a live Catalina install, and a target list that could name it is one operator
// slip from erasing it. The write arc brings a partition-mode installer; until then the device is
// invisible to every write path in the tree.
//
// ### ⚠ WHAT IS NOT HERE YET, and what it costs
//
// There is no `BlockHandle::Ahci` and no `fs::fat::BlockSource::Ahci`, so `fs::bootdisk`'s walk
// cannot yet see these disks. That is not an oversight — see the module doc of `drivers/ahci.rs`
// and this arc's report. Adding the handle variant makes four EXHAUSTIVE matches outside this file
// non-exhaustive (`install/mod.rs` x2, `wifi/firmware.rs`, `fs/unafs.rs` x4), and `install/mod.rs`
// is the one file this arc is forbidden to touch. The entry points below are the whole seam those
// arms would dispatch to when the handle lands.

/// AHCI: one published SATA disk — its geometry plus the HBA PORT it lives on.
///
/// The port is registry state rather than a field of [`BlockDeviceInfo`], for USBREG's reason: a
/// `BlockDeviceInfo` is a GEOMETRY record that five unrelated backends construct, and only this one
/// has AHCI ports. `info.slot_id` carries the port number too, so an identity built from the
/// geometry record alone still names the right disk.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
#[derive(Clone, Copy)]
pub struct AhciDisk {
    pub info: BlockDeviceInfo,
    pub port: u8,
}

/// AHCI: how many SATA disks the registry can hold. Mirrors [`MAX_USB_DISKS`]; the bench rMBP has
/// two and QEMU q35 exposes six ports of which the fixture uses two.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub const MAX_AHCI_DISKS: usize = crate::drivers::ahci::MAX_AHCI_DISKS;

/// AHCI: the SATA block registry. Index-addressed, stable for the boot.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub static AHCI_DISKS: Mutex<[Option<AhciDisk>; MAX_AHCI_DISKS]> =
    Mutex::new([None; MAX_AHCI_DISKS]);

/// AHCI: the registry entry at `ix`, if any.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn ahci_disk(ix: usize) -> Option<AhciDisk> {
    if ix >= MAX_AHCI_DISKS {
        return None;
    }
    AHCI_DISKS.lock()[ix]
}

/// AHCI: geometry of the SATA disk at `ix`.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn ahci_info_ix(ix: usize) -> Option<BlockDeviceInfo> {
    ahci_disk(ix).map(|d| d.info)
}

/// AHCI: geometry of the PRIMARY SATA disk — registry entry 0.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn ahci_info() -> Option<BlockDeviceInfo> {
    ahci_info_ix(0)
}

/// AHCI: how many registry entries hold a disk. Counts OCCUPANCY, not the highest index.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn ahci_disk_count() -> usize {
    AHCI_DISKS.lock().iter().filter(|e| e.is_some()).count()
}

/// AHCI: publish one SATA disk's geometry at registry index `ix`.
///
/// Called once per port from `ahci::bring_up_port`, AFTER IDENTIFY DEVICE has decoded — so a
/// registered disk is one the log has already said something true about, which is the ordering
/// `register_sdhc` established and the reason the registry is worth trusting.
///
/// Refuses a zero-sector disk rather than publishing a device every bound check would reject one
/// call later: an instrument that can say NO. The global [`BLOCK_DEVICE`] is never touched.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn register_ahci(ix: usize, port: u8, num_blocks: u64, model: &[u8; 40]) -> bool {
    if ix >= MAX_AHCI_DISKS {
        serial_println!(":: AHCI: REFUSED to register port={} — registry index {} is out of range ::", port, ix);
        return false;
    }
    if num_blocks == 0 {
        serial_println!(":: AHCI: REFUSED to register port={} — num_blocks=0 (nothing to address) ::", port);
        return false;
    }
    let mut product = [b' '; 16];
    product.copy_from_slice(&model[..16]);
    let dev = BlockDeviceInfo {
        // The AHCI port IS the slot key. It is not an xHCI slot id, and it never collides with one
        // in any consumer: `unpublish_usb_geometry` only ever walks `USB_DISKS` and `BLOCK_DEVICE`,
        // neither of which this function writes.
        slot_id: port,
        block_size: SECTOR_BYTES as u32,
        num_blocks,
        vendor: *b"SATA    ",
        product,
    };
    AHCI_DISKS.lock()[ix] = Some(AhciDisk { info: dev, port });
    let mib = num_blocks.saturating_mul(SECTOR_BYTES as u64) / (1024 * 1024);
    crate::bootlog_println!(
        ":: AHCI: registered port={} as registry index {} — blocks={} ({} MiB) READ-ONLY \
         (global BLOCK_DEVICE untouched, installer not told) ::",
        port, ix, num_blocks, mib
    );
    true
}

/// AHCI: read one block (`lba`) from the SATA disk at registry index `ix`, DIRECTLY through the
/// AHCI driver, bypassing the backend selector — the read twin of [`read_block_usb_ix`]. Geometry
/// is re-read from the registry on every call, as every other entry point here does, and
/// `ahci::read_block_at` re-guards the LBA against the port's own sector count one layer down.
/// Takes no xHCI lock at all, so it cannot interact with the boot volume's transport.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn read_block_ahci_ix(ix: usize, lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let dev = ahci_info_ix(ix).ok_or(BlockError::NotReady)?;
    if lba >= dev.num_blocks {
        return Err(BlockError::BadLba);
    }
    crate::drivers::ahci::read_block_at(ix, lba, buf).map_err(|_| BlockError::Io)?;
    Ok(dev.block_size as usize)
}

/// AHCI: the primary-disk form of [`read_block_ahci_ix`] — registry entry 0.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn read_block_ahci(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    read_block_ahci_ix(0, lba, buf)
}

/// AHCI: the counted twin of [`read_block_ahci_ix`] — `buf.len() / block_size` consecutive blocks in
/// one call. Bounded by the shared [`span_blocks`] rules so they cannot drift between handles.
///
/// The driver's primitive is a SINGLE-sector READ DMA EXT, so this LOOPS it — byte-for-byte the same
/// disk traffic a per-sector caller produces, in the same order, exactly the shape
/// `read_blocks_sdhc`'s write twin uses for the same reason. A multi-sector PRDT would lift this
/// with no change above the seam.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn read_blocks_ahci_ix(ix: usize, lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let dev = ahci_info_ix(ix).ok_or(BlockError::NotReady)?;
    let count = span_blocks(&dev, lba, buf.len())?;
    let bs = dev.block_size as usize;
    for i in 0..count {
        read_block_ahci_ix(ix, lba + i as u64, &mut buf[i * bs..(i + 1) * bs])?;
    }
    Ok(buf.len())
}

/// AHCI: the primary-disk form of [`read_blocks_ahci_ix`].
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn read_blocks_ahci(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    read_blocks_ahci_ix(0, lba, buf)
}

/// AHCI: one-shot latch so the refusal below names itself once instead of per retry.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
static AHCI_WRITE_REFUSED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// AHCI: every write is refused, in every cfg this arc can be built in.
///
/// This is not a policy that could be configured away: `drivers/ahci.rs` contains no WRITE opcode,
/// so there is no ladder for this function to call even if it wanted to. It exists so a future total
/// dispatch fails CLOSED and says why, which is the same direction USBFALL F1's
/// `guard_default_write_backend` and the pre-`sdw` `write_block_sdhc` stub fail in.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn write_block_ahci(_lba: u64, _buf: &[u8]) -> Result<(), BlockError> {
    if !AHCI_WRITE_REFUSED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        serial_println!(
            ":: AHCI: WRITE refused — this arc is READ-ONLY and the image carries no ATA write \
             opcode for the SATA disks (first, once) ::"
        );
    }
    Err(BlockError::NotReady)
}

// ══════════ AHCIBOOT (rmbp-ledger B89, second rung) — the SATA disks as BLOCK HANDLES ════════════
//
// AHCI's first rung published the SATA disks into the registry above and stopped there, because
// `BlockHandle` is matched EXHAUSTIVELY outside this file and one of the files that matches it is
// `install/`, which B91 forbade that arc to touch. The stranger guard's ordering constraint is
// satisfied a different way here: the handle arrives and the installer's two arms REFUSE it by
// construction (`install/mod.rs`), so the internal SSD gains a READ path and no write path at all.
//
// ### The key is the PORT, and the port comes from the enumerator
//
// `BlockHandle::Ahci { port }` carries the HBA port — the same key `AhciDisk` is published under
// and the same key `drivers::ahci`'s `PI` mask handed us. It is NOT the registry index: an index is
// an address into an array this file owns, and a handle that travelled through `BlockDeviceId`,
// `PartitionRange` and `fs::fat::BlockSource` and came back meaning a different row would be the
// identity-from-bytes defect (rmbp 17) in a new spelling. The port is a fact about the wire.
// `fs::fat::BlockSource::Ahci(port)` carries the SAME number, so `handle_of` / `source_of` is a
// lossless bijection in both directions and neither enum can name a disk the other cannot.
//
// The `_port` entry points below are therefore the whole seam: they resolve port -> registry index
// once, through [`ahci_ix_of_port`], and then call the `_ix` entry points the first rung landed.
// One resolution site, so a stale port answers `NotReady` rather than reading a neighbour's disk.
//
// ### The write twins refuse, and the refusal is not a policy
//
// `write_block_ahci_port` / `write_blocks_ahci_port` forward to [`write_block_ahci`], which refuses
// unconditionally with a one-shot witness. `drivers/ahci.rs` still compiles exactly two ATA opcodes
// (IDENTIFY DEVICE 0xEC and READ DMA EXT 0x25), so there is no ladder for a write to descend even
// if something above asked for one. Read-only stays a property of the image.

/// AHCIBOOT: the HBA port the registry entry at `ix` was published under, if that entry is live.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn ahci_port_at(ix: usize) -> Option<u8> {
    ahci_disk(ix).map(|d| d.port)
}

/// AHCIBOOT: which registry index holds the disk on HBA `port`? `None` when no port answers.
///
/// The ONE place a port becomes an index. Everything below goes through it, so "the disk on port 5"
/// means the same row to a `PartitionRange`, to a `BlockSource` and to a `BlockDeviceId`.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn ahci_ix_of_port(port: u8) -> Option<usize> {
    AHCI_DISKS.lock().iter().position(|e| matches!(e, Some(d) if d.port == port))
}

/// AHCIBOOT: geometry of the SATA disk on HBA `port`.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn ahci_info_port(port: u8) -> Option<BlockDeviceInfo> {
    ahci_info_ix(ahci_ix_of_port(port)?)
}

/// AHCIBOOT: read one block from the SATA disk on HBA `port`. `NotReady` when no port answers —
/// never a read of whichever disk happens to occupy that index now.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn read_block_ahci_port(port: u8, lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    read_block_ahci_ix(ahci_ix_of_port(port).ok_or(BlockError::NotReady)?, lba, buf)
}

/// AHCIBOOT: the counted twin of [`read_block_ahci_port`].
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn read_blocks_ahci_port(port: u8, lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    read_blocks_ahci_ix(ahci_ix_of_port(port).ok_or(BlockError::NotReady)?, lba, buf)
}

/// AHCIBOOT: every write to a SATA disk is refused. Forwards to [`write_block_ahci`] rather than
/// returning a local `Err` so there is exactly ONE place in the tree that decides this answer and
/// exactly one witness that names it.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn write_block_ahci_port(_port: u8, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    #[cfg(feature = "ahciroot")] let r = ahci_port_write(_port, lba, buf); #[cfg(not(feature = "ahciroot"))] let r = write_block_ahci(lba, buf); r // AHCIROOT (B332): the live grant decides; -EPERM without one
}

/// AHCIBOOT: the counted twin of [`write_block_ahci_port`] — refuses before touching the medium.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub fn write_blocks_ahci_port(_port: u8, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    #[cfg(feature = "ahciroot")] let r = ahci_port_write(_port, lba, buf); #[cfg(not(feature = "ahciroot"))] let r = write_block_ahci(lba, buf); r // AHCIROOT (B332): the live grant decides; -EPERM without one
}

/// AHCIBOOT: the SATA root's standing write refusal, as [`handle_write_veto`] reports it.
#[cfg(all(feature = "sdwrite", target_arch = "x86_64", feature = "ahci"))]
const NATIVE_AHCI_VETO: &str = "the volume rides an internal SATA disk and the block layer refuses \
                                every write through this handle — the only SATA write path there \
                                is belongs to the installer's grant (AHCI / AHCIBOOT / AHCIWRITE)";

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// BLOCKSMALL — LBA32 (LEDGER SR15) and the USBUNPUB fixture.
//
// APPENDED AT THE FILE TAIL, for the reason the SDWRITE block above already states: no
// `core::panic::Location` line in `block.rs` moves, so `./arroyo knoboff <knob> <baseline>` can come
// out byte-identical. The eight `lba as u32` replacements this arc made ABOVE this line are
// line-for-line in place, same rule. Nothing here is behind a new knob: the refusal is CORRECTNESS in
// the shared block layer and rides every build; only the two fixtures are behind `witness`.
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// USBUNPUB fixture: **[`unpublish_usb_geometry`] advances [`USB_PUBLISH_GEN`], and only when it
/// actually retracted something.**
///
/// # Why a synthetic pair and not the real stick
///
/// QEMU cannot hot-unplug the `usb-storage` device on either the `test` or the `test-arm` leg — the
/// removal witness in `unpublish_usb_geometry` says so in its own comment, and that is precisely why
/// this half of the registry shipped with no counter for months. So the leg drives a PUBLISH and a
/// RETRACTION of a synthetic disk through the real `publish_usb_geometry_lun` /
/// `unpublish_usb_geometry` entry points — the real registry, the real generation counter, the real
/// slot-matching rule — exactly as `fs/bootdisk.rs`'s USBREG leg drives `admit`/`plan` with synthetic
/// disks for the two-card reader no machine in this fleet can present.
///
/// # Why it cannot disturb the boot volume
///
/// Its entry point is `fs::bootdisk::unafsroot_selftest`, which `main.rs` calls on the heap-up line —
/// long before xHCI enumeration, so the registry is empty and the fixture's disk takes index 0 and
/// puts itself back. Belt and braces on top of that ordering: `BLOCK_DEVICE` and `USB_STORAGE_READY`
/// are SNAPSHOT before and RESTORED after (the publish claims the global at index 0 on the x86 arm and
/// raises the ready edge; the retraction clears both), and the leg asserts the registry index it took
/// is empty again. `BOOT_MEDIUM_VERDICT`'s reset is a no-op here because `BM_UNKNOWN` is its initial
/// value. The synthetic slot id is `0x7f`, and nothing on the wire at heap-up holds any slot at all.
///
/// # The legs
///
/// 1. **publish steps the counter** — `+1`, and the disk is in the registry under its own slot.
/// 2. **a STALE retraction is refused and QUIET** — retracting against the generation captured BEFORE
///    the publish returns `false` (PA35's rule: a newer publish supersedes the retraction) and must
///    move nothing. This is the control that stops the counter being bumped by a call that changed no
///    registry; without it a `fetch_add` at the head of the function would pass leg 3 just as well.
/// 3. **the retraction steps the counter** — `+1`, `true`, and the entry is gone.
/// 4. **a NO-OP retraction is quiet** — the same slot again, against the current generation: nothing
///    to remove, `false`, counter unmoved. Together with leg 2 this makes "a generation is a CHANGE" a
///    claim rather than an implementation detail.
///
/// **Go-red:** delete the `fetch_add` from `unpublish_usb_geometry` — `gen_after` equals the
/// post-publish generation, `retract_step=false`, `-> FAIL`.
#[cfg(feature = "witness")]
pub fn usbunpub_selftest() {
    use core::sync::atomic::{AtomicBool, Ordering as O};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, O::Relaxed) {
        return;
    }
    const FX_SLOT: u8 = 0x7f;
    let saved_global = *BLOCK_DEVICE.lock();
    let saved_ready = USB_STORAGE_READY.load(O::Acquire);
    let dev = BlockDeviceInfo {
        slot_id: FX_SLOT,
        block_size: SECTOR_BYTES as u32,
        num_blocks: 64,
        vendor: *b"UNAOS-FX",
        product: *b"USBUNPUB FIXTURE",
    };
    let g0 = usb_publish_gen();
    let ix = publish_usb_geometry_lun(dev, 0);
    let g1 = usb_publish_gen();
    let placed = ix < MAX_USB_DISKS && usb_disk(ix).map(|d| d.info.slot_id) == Some(FX_SLOT);
    let publish_step = g1 == g0 + 1;
    // Leg 2: earned against a generation the publish has already superseded.
    let stale_refused = !unpublish_usb_geometry(FX_SLOT, g0);
    let stale_quiet = usb_publish_gen() == g1;
    // Leg 3: the retraction proper.
    let retracted = unpublish_usb_geometry(FX_SLOT, g1);
    let g2 = usb_publish_gen();
    let retract_step = g2 == g1 + 1;
    let cleared = usb_disk(ix).is_none();
    // Leg 4: nothing left to remove.
    let noop_refused = !unpublish_usb_geometry(FX_SLOT, g2);
    let noop_quiet = usb_publish_gen() == g2;
    // Put the two handles back exactly as they were found.
    *BLOCK_DEVICE.lock() = saved_global;
    USB_STORAGE_READY.store(saved_ready, O::Release);
    let pass = placed
        && publish_step
        && stale_refused
        && stale_quiet
        && retracted
        && retract_step
        && cleared
        && noop_refused
        && noop_quiet;
    serial_println!(
        ":: USBUNPUB: gen_before={} gen_after={} ix={} placed={} publish_step={} \
         stale_refused={} stale_quiet={} retracted={} retract_step={} cleared={} \
         noop_refused={} noop_quiet={} -> {} ::",
        g0, g2, ix, placed, publish_step, stale_refused, stale_quiet, retracted, retract_step,
        cleared, noop_refused, noop_quiet,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// LBA32 (SR15): the 32-bit sector number a SCSI **READ(10) / WRITE(10)** CDB carries, or a refusal.
///
/// # What was wrong, stated exactly
///
/// Eight call sites in this file handed `lba as u32` to the BOT pump. `as` on an out-of-range value
/// does not fail, it TRUNCATES: LBA `0x1_0000_0000` becomes `0`, which is **the boot sector** — a read
/// that returns the wrong sector as if it were right, and a write that destroys the partition table
/// with no error anywhere. That is the identical shape `sd_block_arg` (orin ledger A57) removed from
/// `arch/aarch64/sdmmc_tegra.rs`'s six sites, and this is that shape for this transport.
///
/// # Which ceiling, and why it is NOT 4 GiB here
///
/// A57's four sites truncated a BYTE OFFSET (`(lba * 512) as u32`), so their ceiling was 4 GiB of
/// medium. **These eight never multiply.** READ(10)'s CDB bytes 2..5 are a sector number, so the
/// ceiling is `u32::MAX` **SECTORS** — 2 TiB at 512 B/sector, 4096× further out. The brief's rule is
/// therefore satisfied by construction and is worth writing down because it is the trap: *the byte
/// offset must never be truncated before the divide*, and in this file it is never formed at all.
/// A caller that ever needs a byte offset here must compute it in `u64` and divide first.
///
/// # Reachability today — an ACCIDENT, which is why this is a fix and not a comment
///
/// The USB geometry that bounds every one of the eight sites comes from `scsi_read_capacity10`
/// (`drivers/xhci/mod.rs:11864`), whose `last_lba` is itself a `u32`, so `num_blocks` cannot exceed
/// `0x1_0000_0000` and `lba >= dev.num_blocks` already keeps the argument inside 32 bits. The wrap is
/// thus UNREACHABLE on this tree — guarded by the width of a reply field, not by anything this layer
/// decides. The day a READ CAPACITY(16) lands (the command a >2 TiB device requires, and the reason
/// `last_lba == 0xffff_ffff` is a "ask me again with the 16-byte form" sentinel) the guard evaporates
/// silently and every site wraps. A refusal costs one compare against a path that costs a USB round
/// trip, and it converts a future silent mis-address into a stop on the wire.
///
/// `op` names the entry point for the witness; the refusal prints and returns
/// [`BlockError::BadLba`], the same error the geometry bound returns, so no caller learns a new shape.
fn read10_lba32(op: &str, lba: u64) -> Result<u32, BlockError> {
    match u32::try_from(lba) {
        Ok(v) => Ok(v),
        Err(_) => {
            serial_println!(
                ":: BLK: {} REFUSED lba={} — SCSI READ(10)/WRITE(10) addresses a SECTOR with 32 bits \
                 (ceiling {} sectors = 2 TiB at 512 B); the transfer is refused, never wrapped \
                 (LBA32, SR15) ::",
                op, lba, u32::MAX as u64 + 1
            );
            Err(BlockError::BadLba)
        }
    }
}

/// LBA32 (SR15) fixture: [`read10_lba32`]'s known-answer test, in the shape `sd_block_arg_selftest`
/// established for the same defect class on the Orin.
///
/// The kernel crate is `no_std` and has no `#[cfg(test)]` idiom, so this is a boot-path witness rather
/// than a host unit test. It is PURE — no MMIO, no allocation, no disk, no registry — so it runs
/// wherever `witness` is compiled and disturbs nothing beside it.
///
/// **Two positives, each the last value its arm can express**, because an off-by-one at the ceiling is
/// exactly the defect class: LBA 0 (`0x0000_0000`) and LBA 4,294,967,295 (`0xffff_ffff`).
///
/// **Three negative controls**, and they are the point rather than ballast: `1 << 32` — the first
/// unaddressable sector, and the one whose old value was `0`, i.e. THE BOOT SECTOR — one past it, and
/// `u64::MAX`. Without them a builder that answered with a number for every input would pass.
///
/// **`wrapto=` SHOWS the defect rather than claiming it**: it is `(1u64 << 32) as u32` computed here,
/// so the wire carries the literal value the eight sites used to hand the CDB. **`byteoff=`** is the
/// same sector's BYTE offset computed in `u64` (2 TiB), the number that must never be narrowed before
/// the divide — printed so a reader can see the two ceilings are 4096 apart and not confuse this row
/// with orin ledger A57's 4 GiB one.
///
/// **Go-red, and this fixture's BOUND, stated rather than oversold:** mutate [`read10_lba32`]'s
/// `u32::try_from` to `Ok(lba as u32)` and the leg reads `refused=0/3 -> FAIL`. Reverting ONE CALL
/// SITE to `lba as u32` is INVISIBLE here — this is a known-answer test on the helper — and no
/// behavioural leg can close that gap: the `lba >= dev.num_blocks` geometry bound refuses an
/// out-of-range LBA with the same [`BlockError::BadLba`] one layer earlier, so a wrapped site and a
/// refusing site are indistinguishable from any caller on any reachable input. What guards the eight
/// is the TYPE — this function returns a `Result`, so a site that drops it does not compile into the
/// same expression — plus LEDGER SR15. A grep gate over the call shape would make it mechanical.
#[cfg(feature = "witness")]
pub fn lba32_selftest() {
    use core::sync::atomic::{AtomicBool, Ordering as O};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, O::Relaxed) {
        return;
    }
    let first = read10_lba32("fixture", 0);
    let last = read10_lba32("fixture", u32::MAX as u64);
    let refused = read10_lba32("fixture", 1u64 << 32).is_err() as u32
        + read10_lba32("fixture", (1u64 << 32) + 1).is_err() as u32
        + read10_lba32("fixture", u64::MAX).is_err() as u32;
    // What the eight sites used to compute for the first refused vector: 0 — LBA 0, the boot sector.
    let wrapto = (1u64 << 32) as u32;
    // The byte offset of that same sector, in u64 and never narrowed: 2 TiB. A57's ceiling is 4 GiB;
    // these two rows are the same defect class at ceilings 4096 apart, and this number says so.
    let byteoff = (1u64 << 32).saturating_mul(SECTOR_BYTES as u64);
    let pass = first == Ok(0) && last == Ok(0xffff_ffff) && refused == 3 && wrapto == 0;
    serial_println!(
        ":: LBA32: first={:#010x} last={:#010x} refused={}/3 wrapto={:#010x} byteoff={} -> {} ::",
        first.unwrap_or(0xdead_beef),
        last.unwrap_or(0xdead_beef),
        refused,
        wrapto,
        byteoff,
        if pass { "PASS" } else { "FAIL" }
    );
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// AHCIWRITE (rmbp-ledger B89, SATA write half) — the WRITE CAPABILITY, behind `ahci-write`.
//
// APPENDED AT THE FILE TAIL, for the reason the BLOCKSMALL and SDWRITE blocks above already state:
// `block.rs` is compiled into `kernel8.img`, a `#[cfg]`'d-off block still moves every
// `core::panic::Location` below it (LAWS §5), and at the tail there is nothing below to move. Knob
// off, this whole section is not lexed as code and the Pi image is byte-for-byte what it was.
//
// ### WHY A CAPABILITY AND NOT A FLAG
//
// The disk this path can reach is the rMBP's internal SSD, and Catalina is on it (rmbp-ledger B91,
// RULINGS R25: a non-UnaOS disk is a STRANGER). A boolean "writes are allowed now" would be a
// statement about the code that exists today; a VALUE that has to be produced, carried and shown at
// the wire is a statement about every caller there will ever be. So:
//
//   * [`WriteGrant`]'s fields are PRIVATE to this module. There is no literal-struct construction of
//     one anywhere in the tree, in any cfg — `WriteGrant { .. }` outside `drivers/block.rs` is a
//     compile error, not a review finding.
//   * Its one constructor is [`WriteGrant::new`], and the tree contains exactly ONE call to it:
//     `install::partition::mint_grant`, which runs only after `census()` has read the whole GPT and
//     `check_partition()` has passed every refusal in the ladder (boot device, transport, existence,
//     ESP, content, foreign type, size).
//   * [`write_sectors_granted`] is the ONE caller of `drivers::ahci::write_block_at`, which is
//     `pub(crate)` precisely so that stays true. Two greps are the whole audit:
//         grep -rn 'write_block_at'  unaos/crates/kernel/src   -> declaration + this call
//         grep -rn 'WriteGrant::new' unaos/crates/kernel/src   -> declaration + mint_grant
//   * The PLAIN path is UNCHANGED. [`write_block_ahci`], [`write_block_ahci_port`] and
//     [`write_blocks_ahci_port`] still return `NotReady` with the same one-shot witness, in this cfg
//     as in every other, so `BlockHandle::Ahci`'s arm in `handle_dispatch`, the FAT writer on a
//     SATA-mounted volume, every shell verb and the whole-disk installer engine all still refuse.
//     Arming this knob does not widen any door that was already there; it adds one narrow new one.

/// AHCIWRITE: permission to write ONE inclusive LBA range on ONE AHCI port, and nothing else.
///
/// Opaque by construction — the fields are private and the only constructor is [`WriteGrant::new`].
/// `Copy` so the installer can hand it down through `PartitionTarget` without lifetime ceremony;
/// that costs nothing, because possessing one is already the permission and copying a permission
/// grants no range a caller did not already hold.
///
/// The LBAs are ABSOLUTE disk LBAs, not partition-relative ones. That is deliberate and it is the
/// half that makes the check meaningful: `PartitionTarget::map()` has already translated, so the
/// number this type sees is the number the HBA will see, and a translation bug upstream shows up
/// here as an out-of-grant refusal instead of passing a plausible-looking relative LBA through.
#[cfg(all(target_arch = "x86_64", feature = "ahci-write"))]
#[derive(Clone, Copy)]
pub struct WriteGrant {
    port: u8,
    first_lba: u64,
    last_lba: u64,
}

#[cfg(all(target_arch = "x86_64", feature = "ahci-write"))]
impl WriteGrant {
    /// THE ONE CONSTRUCTOR. Called from exactly one place in the tree —
    /// `install::partition::mint_grant` — after the census and the refusal ladder.
    ///
    /// ⚠ **DO NOT ADD A SECOND CALL SITE.** The refusal ladder is what makes a grant mean anything;
    /// a grant minted anywhere else is a write to a stranger's disk with the guards skipped. A new
    /// caller belongs behind `mint_grant`, not beside it.
    pub fn new(port: u8, first_lba: u64, last_lba: u64) -> Self {
        Self { port, first_lba, last_lba }
    }
    pub fn port(&self) -> u8 {
        self.port
    }
    pub fn first_lba(&self) -> u64 {
        self.first_lba
    }
    pub fn last_lba(&self) -> u64 {
        self.last_lba
    }
    /// Is `lba` inside this grant? Inclusive at both ends, and it is the ONLY predicate
    /// [`write_sectors_granted`] consults.
    pub fn allows(&self, lba: u64) -> bool {
        lba >= self.first_lba && lba <= self.last_lba
    }
}

/// AHCIWRITE: write `buf.len()/block_size` consecutive sectors at ABSOLUTE `lba` on the port the
/// grant names — the ONLY path in this tree that reaches `drivers::ahci::write_block_at`.
///
/// Every sector is checked against the grant INDIVIDUALLY before it is issued, so a run that starts
/// inside the grant and walks off the end is refused at the first offending sector and the sectors
/// before it are the only bytes that moved. The refusal prints
/// `[ahci] write REFUSED lba=<n> outside grant port=<p> <first>..<last>` — a witness, not a silent
/// `Err`, because a guard nobody can see fire is an absent one (LAWS §5), and this one has a go-red
/// fixture (`install::partition::grant_go_red`).
///
/// Three independent bounds guard one write and they live in three files on purpose: the
/// `PartitionTarget`'s `map()` (install/partition.rs) refuses anything past the partition's length,
/// this range check (drivers/block.rs) refuses anything outside the granted extent, and
/// `ahci::write_block_at` (drivers/ahci.rs) refuses anything past the device's own sector count. A
/// single mistake has to be made three times, in three files, to reach the medium.
#[cfg(all(target_arch = "x86_64", feature = "ahci-write"))]
pub fn write_sectors_granted(grant: &WriteGrant, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let ix = ahci_ix_of_port(grant.port()).ok_or(BlockError::NotReady)?;
    let dev = ahci_info_ix(ix).ok_or(BlockError::NotReady)?;
    let bs = dev.block_size as usize;
    if bs == 0 || buf.is_empty() || buf.len() % bs != 0 {
        return Err(BlockError::BadLba);
    }
    let count = buf.len() / bs;
    for i in 0..count {
        let abs = lba.checked_add(i as u64).ok_or(BlockError::BadLba)?;
        if !grant.allows(abs) {
            serial_println!(
                ":: [ahci] write REFUSED lba={} outside grant port={} {}..{} — nothing written ::",
                abs,
                grant.port(),
                grant.first_lba(),
                grant.last_lba()
            );
            return Err(BlockError::BadLba);
        }
        if abs >= dev.num_blocks {
            return Err(BlockError::BadLba);
        }
        crate::drivers::ahci::write_block_at(ix, abs, &buf[i * bs..(i + 1) * bs])
            .map_err(|_| BlockError::Io)?;
    }
    Ok(())
}

// ══════ SDHCPOST (rmbp-ledger B155, 2026-09-22) — the internal SD reader's WRITE POSTURE ═══════
//
// WHAT THIS SECTION IS FOR. Flight 11 asked the machine for a screenshot and got
// `:: PRTSCR: REFUSED READ-ONLY (source=sdhc … SDHC-4c)`. SDHCWRITE then found why, and the finding
// is the shape of this section: the card's posture had exactly ONE definition —
// `fat::BlockSource::write_veto`'s `Sdhc` arm — and it returned `Some(…)` unconditionally, so no
// knob, no pin and no driver primitive anywhere below it could change the answer. CMD24 (4a), CMD25
// (4b) and the reserved-extent permit (4c) were all in the image and all unreachable from a file
// verb. This is the lift, and it is opt-in.
//
// THE POLARITY — INVERTED BY SDHCRW (rmbp-ledger B166, R59, 2026-09-22), and this paragraph is the
// whole of the change. B155 shipped `sdw-rw` DEFAULT OFF and said in as many words that the decision
// was Peter's and not a build flag. He took it: RULINGS R59, verbatim, *"read write."* So the
// shipped configuration is now the one that WRITES — `sdw` (4a/4b's ladder) rides the default x86
// image, the posture is `rw`, and the refusal moved behind a NAMED OPT-OUT, `sdw-ro`
// (`UNAOS_SDW_RO=1`), which is LAWS §3 read straight: default-on with a named opt-out, never opt-in.
// The cost B155 priced — a journal-less FAT under a machine that loses power — is unchanged and is
// now a cost the ruling accepts; the opt-out is what an operator arms for a cold-witness boot, and
// it is the ONLY thing that restores the pre-SDHCPOST refusal.
//
// A NOTE ON WHAT PROVES IT, because the old proof inverts too. B155's claim was `./arroyo knoboff
// sdw-rw` = "the knob-OFF image equals baseline". Under R59 the knob-OFF image IS the new rw default
// and MUST differ from baseline — that is the ruling — so a knoboff verdict is not the proof here
// and must not be quoted as one (LAWS §5). The proof is the REPLAY: the default image prints
// `sdhc=rw` and writes, the `UNAOS_SDW_RO=1` image prints `sdhc=ro reason=opt-out` and refuses.
//
// THE THREE CONDITIONS, and why each is separate. `sdhc=rw` requires all three, and the reason a
// refusal carries names WHICH one failed, because they fail for unrelated causes and send a reader
// to three different places:
//
//   1. NO OPT-OUT, PATH BUILT — `sdw-ro` NOT built and `sdw` built. Both halves are condition 1
//                             because both are build-time and both mean the same thing to a caller:
//                             this image will not write the card. Either one compiles the refusing
//                             twin below, which returns the CALLER'S OWN string and names itself
//                             `opt-out` (the knob) or `no-write-path` (no 4a/4b ladder) on the wire.
//                             `sdw` is default-on for x86 now, so in practice condition 1 is the
//                             opt-out and nothing else.
//   2. WP PIN says ENABLED  — Present State bit 19, read ONCE, AT MOUNT. This is deliberately NOT
//                             the same read 4a/4b do: `write_block_512` and `write_blocks_512`
//                             re-read the pin at the moment of EVERY write and keep doing so
//                             (§14.4 gate 2). The posture's read answers a different question —
//                             "may this VOLUME be mounted writable" — and a mount-time answer is
//                             what `write_veto` exists to give IN ADVANCE, before a multi-step file
//                             verb gets part-way. The two reads cannot disagree dangerously: the
//                             posture is the LOOSER of the pair and the per-write read still
//                             refuses underneath it.
//   3. WRITE PATH LIVE      — `sdhc_info()` is published, i.e. the card registered. Without it
//                             there is no geometry to bound an LBA against, and admitting a write
//                             to a handle with no device would be a `NotReady` surfaced as a
//                             per-sector I/O error on a volume that was never writable — the exact
//                             failure mode `BlockSource::write_veto`'s doc was written against.
//
// WHY THE ONE DEFINITION LIVES HERE AND NOT IN `fs/fat.rs`. Because there are TWO readers, not one,
// and A60 already learned this on the Orin: `handle_write_veto` above answers per HANDLE and
// `fat::BlockSource::write_veto` answers per SOURCE, and `fs::bootdisk::sdwrite_posture_selftest`
// leg 8 compares the two for every source in `fat::ALL_SOURCES`. Two views of one answer is the
// shape that drifts. Both are now forwards to [`sdhc_write_veto`], so the fixture cannot find a
// disagreement — not because the two arms were kept in step, but because there is only one arm.
//
// THE APPEND IS AT THE FILE TAIL, for the reason the SDWRITE section above gives: no
// `core::panic::Location` line in `block.rs` moves, and the two in-file sites this section needed
// (`write_blocks_sdhc`'s CMD25 route and `handle_write_veto`'s `Sdhc` arm) are LINE-NEUTRAL. SDHCRW
// keeps the append at the tail for the same reason, and keeps both sites line-neutral too — but see
// the polarity note above: byte-identity is no longer what this section is measured by.

/// SDHCPOST/SDHCRW: the REFUSING twin — condition 1 failed, so the answer is the pre-SDHCPOST
/// refusal, returned from the CALLER'S OWN string.
///
/// SDHCRW (B166, R59): this is now the EXCEPTIONAL build, not the shipped one. It compiles when
/// `sdw-ro` is armed (the named opt-out) or when `sdw` is absent (no 4a/4b ladder, so there is no
/// write path to admit anything to). It prints the truth table exactly as the admitting twin does —
/// the same line, the same fields — because an operator who armed the opt-out and an operator whose
/// image simply has no ladder need to read WHICH of the two they are looking at, and silence is not
/// a reading. That is a deliberate departure from B155, where this twin was byte-silent to protect a
/// byte-identity claim R59 retires.
///
/// The string is a parameter rather than a constant here for an honesty reason: the two callers
/// print about two DIFFERENT mounts (`NATIVE_SDHC_VETO` is about the native root, `fs/fat.rs`'s is
/// about a FAT volume) and a reader chasing a refusal must land on the layer that refused. Passing
/// the string in also keeps ONE signature for both polarities, so neither caller carries a `cfg`.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", any(feature = "sdw-ro", not(feature = "sdw"))))]
pub fn sdhc_write_veto(ro_reason: &'static str) -> Option<&'static str> {
    // SDHCRW (B166): the SAME truth-table line the admitting twin prints, once per boot, so the two
    // polarities are read off one vocabulary and the opt-out is a fact on the wire rather than the
    // absence of one. `wp-pin=` and `write-path=` are `unread` here and say so: this twin refuses
    // BEFORE condition 2, so claiming a pin reading it never took would be the lie the line exists
    // to prevent.
    if !SDHC_POSTURE_SAID.swap(true, core::sync::atomic::Ordering::Relaxed) {
        serial_println!(":: SDHCPOST: posture {} ::", SDHC_RO_TWIN_ROW);
    }
    Some(ro_reason)
}

/// SDHCRW (B166): the refusing twin's row as ONE CONTIGUOUS LITERAL per polarity, and the shape is
/// forced by a GATE rather than chosen for looks.
///
/// `scripts/banner-cert.sh` certifies a feature by finding its witness token IN THE ARTIFACT with
/// `LC_ALL=C grep -a -o -F` — a fixed string, not a regex — which is the whole reason it can catch a
/// banner that names a feature the media does not carry. A `serial_println!` with `{}` in it does
/// not put its rendered line anywhere: rustc splits the format string at each placeholder, so
/// `sdw-ro=1` never exists as bytes and the row is uncertifiable. MEASURED on this arc: the first
/// cut printed the two fields through `cfg!` and `./arroyo esp-x86` with `UNAOS_SDW_RO=1` came back
/// `feature=sdw-ro witness=:: SDHCPOST: posture sdw-ro=1 wp-pin=unread hits=0 -> MISSING` — the
/// gate correctly refusing to certify a line that is not in the binary.
///
/// `#[cfg]` and not `cfg!`, for the other half of the same gate: `cfg!` is a runtime false and
/// compiles BOTH arms, so the opt-out row's bytes would sit in every default image and banner-cert
/// would report a LEAK — a control string for a feature the banner did not name. One literal per
/// polarity means the artifact carries exactly the row it can print.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", feature = "sdw-ro"))]
const SDHC_RO_TWIN_ROW: &str = "sdw-ro=1 wp-pin=unread write-path=unread -> sdhc=ro \
                                reason=opt-out (condition 1 refused before the pin was read; R59 \
                                made rw the default and THIS is the exception — SDHCPOST/SDHCRW, \
                                first, once)";
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", not(feature = "sdw"), not(feature = "sdw-ro")))]
const SDHC_RO_TWIN_ROW: &str = "sdw-ro=0 wp-pin=unread write-path=unread -> sdhc=ro \
                                reason=no-write-path (this image carries no `sdw` CMD24/CMD25 \
                                ladder at all, so there is nothing to admit a mutation to — \
                                SDHCPOST/SDHCRW, first, once)";

/// SDHCPOST: the ADMITTING twin — conditions 2 and 3, asked at the mount. `None` = an ordinary file
/// mutation may reach the card; `Some(reason)` names which condition said no.
///
/// SDHCRW (B166, R59): this is the SHIPPED twin now. `_ro_reason` is unused on purpose and is not a
/// mistake to clean up: it is condition 1's answer, and condition 1 cannot fail in a build that
/// compiled THIS twin (`sdw` built, `sdw-ro` not). Keeping the parameter keeps ONE signature for
/// both polarities, so neither caller carries a `cfg`.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", feature = "sdw", not(feature = "sdw-ro")))]
pub fn sdhc_write_veto(_ro_reason: &'static str) -> Option<&'static str> {
    sdhc_rw_gate()
}

/// SDHCPOST: `true` iff an ordinary write is admitted — the boolean face of [`sdhc_write_veto`], for
/// callers that want the posture without a reason. Kept beside `tegra_sd_writes_admitted` in
/// PURPOSE if not in position (position is the file tail, for byte-identity); unlike that one it
/// cannot be `const fn`, because condition 2 is a PIN on a live controller and not a `cfg!`.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", feature = "sdw", not(feature = "sdw-ro")))]
#[inline(always)]
pub fn sdhc_writes_admitted() -> bool {
    sdhc_write_veto("").is_none()
}

/// SDHCPOST: the write-protect pin AS THE POSTURE READ IT — 0 unread, 1 enabled (writable),
/// 2 protected. Written exactly once per boot, by [`sdhc_rw_gate`].
///
/// A latch rather than a live read, and that is the whole point of condition 2: a volume's posture
/// must not change under an open file verb because someone brushed the slider mid-write. The LIVE
/// read still exists and still refuses, one layer down, at every single CMD24 and CMD25.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", feature = "sdw", not(feature = "sdw-ro")))]
static SDHC_WP_AT_MOUNT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// SDHCPOST: one-shot latch for the truth-table witness. SDHCRW (B166): shared by BOTH twins now —
/// the refusing one prints the same line — so its cfg is the module's, not the posture's.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
static SDHC_POSTURE_SAID: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// SDHCRW (B166): a SECOND, independent one-shot for the pre-card question — see [`sdhc_rw_gate`]'s
/// PRE-CARD note. It must not be the same latch as [`SDHC_POSTURE_SAID`]: if one question could
/// consume the other's line, the boot that mattered would be the one with no row.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", feature = "sdw", not(feature = "sdw-ro")))]
static SDHC_NOPATH_SAID: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// SDHCPOST: condition 2's refusal.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", feature = "sdw", not(feature = "sdw-ro")))]
const SDHC_RW_WP_VETO: &str = "the internal SD reader is mounted READ-ONLY (reason=wp-pin) \
                               \u{2014} the write-protect pin read at MOUNT says the card is \
                               locked (Present State bit 19 clear), and R59's rw default does not \
                               overrule the slider";
/// SDHCPOST: condition 3's refusal.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", feature = "sdw", not(feature = "sdw-ro")))]
const SDHC_RW_NOPATH_VETO: &str = "the internal SD reader is mounted READ-ONLY \
                                   (reason=no-write-path) \u{2014} no card geometry is published \
                                   on this handle, so there is nothing to bound an LBA against and \
                                   the block layer has no live write path to admit a mutation to";

/// SDHCPOST: THE DECISION, and the only place conditions 2 and 3 are stated.
///
/// The pin is read on the first posture question of the boot THAT HAS A LIVE WRITE PATH, which IS
/// the mount: `FatBackend::new_source` asks `read_only()` while binding, and `fs::bootdisk`'s
/// HOMESOIL census asks `write_veto()` for every source as the volume walk runs.
///
/// ### PRE-CARD: the qualifier above is SDHCRW's, and a measured boot is why it is there
///
/// B155's doc said "the FIRST posture question of the boot, which IS the mount — nothing asks
/// earlier". **Something does, and it is a FIXTURE.** `fs::bootdisk::sdwrite_posture_selftest` (leg
/// 8, the `:: SDWRITE-POSTURE:` line) drives `write_veto` over EVERY source in `fat::ALL_SOURCES`
/// deliberately — that is its whole value — and on x86 it runs before `drivers::sdhc` registers the
/// card. B155 never saw it because with `sdw-rw` default OFF this function did not exist in a
/// shipped build; R59's flip ran it for the first time, and the first default replay read:
///
/// ```text
/// :: SDHCPOST: posture sdw-ro=0 wp-pin=PROTECTED write-path=ABSENT -> sdhc=ro reason=wp-pin ::
/// :: SDWRITE-POSTURE: posture=on … sdhc=ro card=absent … :: PASS ::
/// … (14 lines later) :: SDHCBLK: registered internal SD card as block handle Sdhc … ::
/// ```
///
/// `card_write_protected()` answers `None` with no mapped controller, the gate fails CLOSED and
/// reads that as PROTECTED — correct in isolation, and the latch then froze it for the boot. The
/// card's pin in that same boot says ENABLED (`:: sdhc: w1 … wp_sw=1 … -> PASS ::`), so the volume
/// would have stayed read-only on every default image for a reason the hardware never gave.
///
/// THE FIX IS AN ORDERING FIX, NOT A LOOSENING, and it is condition 3 doing its job: with no live
/// write path the answer is `no-write-path` — which was always the honest name for "there is no
/// card here yet" — and the function takes NEITHER latch on that path. Nothing is admitted that was
/// not admitted before: the pre-card question still refuses. What changes is that it no longer
/// ANSWERS FOR the mount that has not happened. The pin's fail-closed reading is untouched and
/// still applies the moment a controller is mapped.
///
/// The truth table is printed ONCE, whatever the verdict — including when the verdict is `ro`,
/// because an operator on a DEFAULT image who still got a read-only card needs the row that says
/// which condition refused, not the absence of a line. SDHCRW (B166): the refusing twin above prints
/// the same line for the same reason, so the vocabulary is one and `sdhc=rw` / `sdhc=ro` is a token
/// an artifact grep finds in either polarity. The pre-card question gets its own row and its own
/// one-shot, so a boot whose card never registers still says so exactly once.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", feature = "sdw", not(feature = "sdw-ro")))]
fn sdhc_rw_gate() -> Option<&'static str> {
    use core::sync::atomic::Ordering;
    // SDHCRW (B166): condition 3 is tested FIRST and is now a hard early return — see the PRE-CARD
    // note above. Before the card registers there is no pin to read and no mount to answer for, so
    // this arm reads no pin, takes no latch, and cannot decide the boot.
    if sdhc_info().is_none() {
        if !SDHC_NOPATH_SAID.swap(true, Ordering::Relaxed) {
            serial_println!(
                ":: SDHCPOST: posture sdw-ro=0 wp-pin=unread write-path=ABSENT -> sdhc=ro \
                 reason=no-write-path (asked BEFORE the card registered — typically \
                 `sdwrite_posture_selftest` leg 8, which drives every source on purpose; NOTHING \
                 is latched here and the mount answers for itself — SDHCRW, first, once) ::"
            );
        }
        return Some(SDHC_RW_NOPATH_VETO);
    }
    let path_live = true;
    let mut wp = SDHC_WP_AT_MOUNT.load(Ordering::Acquire);
    if wp == 0 {
        // `None` (no mapped controller) is PROTECTED, not unknown: this gate fails closed, in the
        // same direction as USBFALL F1's `guard_default_write_backend` and for the same reason.
        wp = match crate::drivers::sdhc::card_write_protected() {
            Some(true) => 2,
            Some(false) => 1,
            None => 2,
        };
        SDHC_WP_AT_MOUNT.store(wp, Ordering::Release);
    }
    let verdict = if wp != 1 {
        Some(SDHC_RW_WP_VETO)
    } else if !path_live {
        Some(SDHC_RW_NOPATH_VETO)
    } else {
        None
    };
    if !SDHC_POSTURE_SAID.swap(true, Ordering::Relaxed) {
        serial_println!(
            ":: SDHCPOST: posture sdw-ro=0 wp-pin={} write-path={} -> {} reason={} (the pin is \
             read ONCE, at the mount; 4a/4b re-read it at every write \u{2014} SDHCPOST/SDHCRW, \
             first, once) ::",
            match wp {
                1 => "enabled",
                _ => "PROTECTED",
            },
            if path_live { "live" } else { "ABSENT" },
            // The verdict is a WHOLE literal, `sdhc=rw` / `sdhc=ro`, not a `sdhc={}` with two
            // fragments: `sdhc=rw` is then a string an artifact grep can find, so "is this image
            // capable of a writable card at all" is answered off the ARTIFACT
            // (`LC_ALL=C grep -a -o -F 'sdhc=rw'`) and never only off a boot that may not have run.
            if verdict.is_none() { "sdhc=rw" } else { "sdhc=ro" },
            match (wp, path_live) {
                (1, true) => "none",
                (1, false) => "no-write-path",
                _ => "wp-pin",
            }
        );
    }
    verdict
}

/// SDHCPOST: the counted write as ONE CMD25, the route [`write_blocks_sdhc`] takes for every span
/// longer than a sector.
///
/// THE NUMBER THAT DECIDED IT, because "faster" is not a reason and this file does not take one.
/// `video/prtscr.rs` hands `SLICE_WRITE = 32 KiB` per `write_grow`; `write_grow` step 3 pushes the
/// whole-sector interior through `write_span` -> `fat::write_sectors`, which chunks at
/// `MAX_BLOCKS_PER_OP` — `STORAGE_DATA_BYTES / 512` = 64. `sdhc::MB_MAX_BLOCKS` is also 64. So one
/// capture slice is EXACTLY one CMD25, and the rMBP's 15.5 MB screenshot is ~484 of them where the
/// CMD24 loop issued ~31,700 single-block writes: ~64x fewer card transactions, each one a saved
/// command, response, Buffer-Write-Ready re-arm and DAT0 busy poll. The premise SDHCWRITE left this
/// path on — "one sector per file-verb write is the common case" — is false for the workload that
/// asked for the posture in the first place.
///
/// It could not be done until now for a reason that was correct: `fs::sdhc4c` published
/// `cmd24 == bytes / 512` as a falsifiable prediction, and re-pointing this path would have made a
/// shipped witness name a command the card never saw. That counter is now `sectors=` and the
/// prediction is `sectors == bytes / 512`, which is true of BOTH commands and names neither.
///
/// Bounds are re-checked here against the published geometry even though the driver checks them
/// again against the card's own capacity, for the reason [`tegra_sd_write_through`] gives: this
/// function names an LBA to a medium and must not depend on a caller's care.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk", feature = "sdw", not(feature = "sdw-ro")))]
pub fn write_blocks_sdhc_mb(lba: u64, count: usize, buf: &[u8]) -> Result<(), BlockError> {
    let dev = sdhc_info().ok_or(BlockError::NotReady)?;
    if count == 0 || count > crate::drivers::sdhc::MB_MAX_BLOCKS as usize {
        return Err(BlockError::Io); // larger than the driver's bound; the caller chunks, as `write_sectors` does
    }
    if lba.checked_add(count as u64).is_none_or(|end| end > dev.num_blocks) {
        return Err(BlockError::BadLba);
    }
    if buf.len() < count * SECTOR_BYTES {
        return Err(BlockError::Io);
    }
    crate::fs::bootstep::note_write(count as u64); // BOOT80 (B350)
    let t0 = crate::drivers::sdhc::wr_census_begin(); // SDHCMULTI M1
    let r = crate::drivers::sdhc::write_blocks_512(lba, count as u16, &buf[..count * SECTOR_BYTES]);
    crate::drivers::sdhc::wr_census_note(t0, count as u64, true, *r.as_ref().unwrap_or(&0), r.is_ok());
    r.map(|_| ())
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// AHCIROOT (rmbp-ledger B332, R82) — THE KERNEL-HELD SATA WRITE GRANT and the port write path.
//
// AHCIWRITE gave the installer a `WriteGrant` VALUE it carries inside its own `InstallTarget`; the
// PLAIN port path (`write_block_ahci_port`, which UnaFS's sector seam and the FAT writer reach) kept
// refusing, so the SSD's UnaFS could never be the read-write `/`. This section is the one door that
// path gains, and it is a SLOT, not a flag:
//
//   * ONE grant at a time, for ONE port and ONE inclusive LBA range, plus who holds it (`install`,
//     `root`, `test`). The slot is filled only with a `WriteGrant` that `install::partition` minted —
//     `WriteGrant::new` keeps its single call site, so the AHCIWRITE audit greps are unchanged.
//   * [`hold_ahci_grant`] returns a [`GrantHold`]; dropping it FLUSHES the drive and empties the slot.
//     `install ssd --write` and `tests ahciw` hold it for the verb's lifetime only. The boot's root
//     bind keeps its hold for the boot ([`hold_ahci_grant_for_boot`]) — and an occupied slot is why
//     `install ssd --write` cannot run over the live root.
//   * With no live grant covering the sector, [`ahci_port_write`] returns [`BlockError::Denied`]
//     (-EPERM) and prints `:: [ahci] write -EPERM … ::` (first few per boot, then counted).
//   * A granted write goes through `write_sectors_granted` (the one caller of `ahci::write_block_at`)
//     and every sector is READ BACK and compared. The flush is the commit barrier: UnaFS's
//     `SdSectorDevice::flush` (its pre-root-flip barrier) calls [`flush_ahci_port`] on an AHCI handle.
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// AHCIROOT: who holds the live SATA write grant.
#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GrantKind {
    /// `install ssd --write`, for the verb's lifetime, on a disk the self-guard judged.
    Install,
    /// The boot's root bind, over an OURS disk's UnaFS partition, for the boot.
    Root,
    /// `tests ahciw`, over the 64-sector scratch tail of an OURS disk's UnaFS partition.
    Test,
}

#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
impl GrantKind {
    pub fn tag(self) -> &'static str {
        match self {
            GrantKind::Install => "install",
            GrantKind::Root => "root",
            GrantKind::Test => "test",
        }
    }
}

/// AHCIROOT: THE slot. `None` = every SATA port write through the plain path is -EPERM.
#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
static AHCI_LIVE_GRANT: Mutex<Option<(WriteGrant, GrantKind)>> = Mutex::new(None);

/// AHCIROOT: refusal lines printed so far (the first [`DENY_LINES`] print; the rest are counted).
#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
static AHCI_DENIED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
const DENY_LINES: u32 = 4;

/// AHCIROOT: the live grant, if any — `(port, first_lba, last_lba, kind)`.
#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
pub fn ahci_live_grant() -> Option<(u8, u64, u64, GrantKind)> {
    crate::arch::without_interrupts(|| AHCI_LIVE_GRANT.lock().map(|(g, k)| (g.port(), g.first_lba(), g.last_lba(), k)))
}

/// AHCIROOT: how many port writes have been refused -EPERM this boot.
#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
pub fn ahci_denied_count() -> u32 {
    AHCI_DENIED.load(core::sync::atomic::Ordering::Relaxed)
}

/// AHCIROOT: the RAII face of a live grant. Dropping it flushes the drive and empties the slot.
#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
#[must_use = "the grant is dropped (and the slot emptied) the moment the hold is"]
pub struct GrantHold {
    port: u8,
    kind: GrantKind,
}

#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
impl GrantHold {
    pub fn port(&self) -> u8 {
        self.port
    }
}

#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
impl Drop for GrantHold {
    fn drop(&mut self) {
        let flushed = flush_ahci_port(self.port).is_ok();
        crate::arch::without_interrupts(|| *AHCI_LIVE_GRANT.lock() = None);
        if self.kind != GrantKind::Root {
            serial_println!(
                "[ahci] grant dropped port={} kind={} flush={}",
                self.port,
                self.kind.tag(),
                if flushed { "ok" } else { "FAIL" }
            );
        }
    }
}

/// AHCIROOT: put a minted grant in the slot. `None` when the slot is already held (by anyone) — the
/// caller refuses; it never waits and never replaces another holder's grant.
#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
pub fn hold_ahci_grant(g: WriteGrant, kind: GrantKind) -> Option<GrantHold> {
    let prior = crate::arch::without_interrupts(|| {
        let mut s = AHCI_LIVE_GRANT.lock();
        match *s {
            Some((h, hk)) => Some((h.port(), hk)),
            None => {
                *s = Some((g, kind));
                None
            }
        }
    });
    if let Some((p, hk)) = prior {
        if kind != GrantKind::Root {
            serial_println!("[ahci] grant REFUSED port={} kind={} — the slot is held (port={} kind={})", g.port(), kind.tag(), p, hk.tag());
        }
        return None;
    }
    if kind != GrantKind::Root {
        serial_println!("[ahci] grant held port={} kind={} lba={}..{}", g.port(), kind.tag(), g.first_lba(), g.last_lba());
    }
    Some(GrantHold { port: g.port(), kind })
}

/// AHCIROOT: the boot's root hold — kept for the life of the boot (the hold is leaked on purpose:
/// the root volume is written until power-off). Silent (R80: the root line is the boot's one line).
#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
pub fn hold_ahci_grant_for_boot(g: WriteGrant) -> bool {
    match hold_ahci_grant(g, GrantKind::Root) {
        Some(h) => {
            core::mem::forget(h);
            true
        }
        None => false,
    }
}

/// AHCIROOT: ATA `FLUSH CACHE EXT` on the disk at HBA `port`.
#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
pub fn flush_ahci_port(port: u8) -> Result<(), BlockError> {
    let ix = ahci_ix_of_port(port).ok_or(BlockError::NotReady)?;
    crate::drivers::ahci::flush_at(ix).map_err(|_| BlockError::Io)
}

/// AHCIROOT: THE plain SATA port write. -EPERM unless the live grant names this port and covers
/// every sector of the run; otherwise written through the grant and read back sector by sector.
#[cfg(all(target_arch = "x86_64", feature = "ahciroot"))]
pub fn ahci_port_write(port: u8, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    if buf.is_empty() || buf.len() % SECTOR_BYTES != 0 {
        return Err(BlockError::BadLba);
    }
    let n = (buf.len() / SECTOR_BYTES) as u64;
    let last = lba.checked_add(n - 1).ok_or(BlockError::BadLba)?;
    let live = crate::arch::without_interrupts(|| *AHCI_LIVE_GRANT.lock());
    let g = match live {
        Some((g, _)) if g.port() == port && g.allows(lba) && g.allows(last) => g,
        other => {
            let k = AHCI_DENIED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if k < DENY_LINES {
                match other {
                    None => serial_println!(":: [ahci] write -EPERM port={} lba={} grant=none — nothing written ::", port, lba),
                    Some((g, gk)) => serial_println!(
                        ":: [ahci] write -EPERM port={} lba={} grant=port {} {}..{} kind={} — outside it, nothing written ::",
                        port, lba, g.port(), g.first_lba(), g.last_lba(), gk.tag()
                    ),
                }
            }
            return Err(BlockError::Denied);
        }
    };
    write_sectors_granted(&g, lba, buf)?;
    let mut rb = [0u8; SECTOR_BYTES];
    for i in 0..n as usize {
        read_block_ahci_port(port, lba + i as u64, &mut rb)?;
        if rb[..] != buf[i * SECTOR_BYTES..(i + 1) * SECTOR_BYTES] {
            serial_println!(":: [ahci] write readback MISMATCH port={} lba={} — the medium does not hold what was written ::", port, lba + i as u64);
            return Err(BlockError::Io);
        }
    }
    Ok(())
}

// ===================== USBSTOR (rmbp-ledger B384, R95) — the boot medium is never displaced ==========
//
// Flight 25 (f25-boots.log :327–:628): a 64 GB card in the hub's USB reader enumerated at boot, took
// registry index 0 and with it the GLOBAL slot (`publish_usb_geometry_lun`, x86 arm). The rMBP boots
// from the INTERNAL Sdhc card, and the users store's ladder (`fs/users.rs` `store_via_from`) prefers
// `Global` whenever it exists, so the store mounted the STICK and `set-password … NOT written
// reason=volume` twice; FRGUARD refused Default writes, correctly, against a slot the stick should
// never have held. The shape :523 already names is the rule here: the boot medium keeps its handle
// and its write path for the whole boot, and a USB disk that claims is a SEPARATE disk.
//
// `pin_boot_medium_once` proves ONCE, from the main loop (it reads sectors), whether the boot volume
// serial the loader handed over lives on the Sdhc card. Pinned there: a later USB publish declines the
// global slot (`usb_global_closed`), and a stick that claimed it before the pin is evicted — it stays
// registry index 0, `BlockSource::Usb`, where Volumes mounts it. Not pinned there (booted from a card
// in a USB reader, no Sdhc card, no serial): byte-identical to before — index 0 claims the global.

/// USBSTOR: 0 = not yet proven, 1 = the boot volume is on the Sdhc card (global slot closed to USB),
/// 2 = it is not (the legacy claim stands).
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
static BOOT_PIN: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// USBSTOR: is the boot medium pinned on a handle other than the global slot?
pub fn boot_medium_kept() -> bool {
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    {
        return BOOT_PIN.load(core::sync::atomic::Ordering::Acquire) == 1;
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "sdhcblk")))]
    false
}

/// USBSTOR: the x86 publish asks this before claiming the global slot for registry index 0. One line
/// per declined claim, naming the slot — the `[block] … separate (R95)` witness.
#[cfg(not(all(target_arch = "aarch64", feature = "baremetal")))]
fn usb_global_closed(slot: u8) -> bool {
    if !boot_medium_kept() {
        return false;
    }
    serial_println!("[block] boot medium=sdhc kept; usb slot={} separate (R95)", slot);
    true
}

/// USBSTOR: derive the pin. MUST run from an unmasked, lock-free main-loop context (sector reads) —
/// `fs::removable::service` is its caller, on every x86 service loop. One acquire load once decided.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
pub fn pin_boot_medium_once() {
    if BOOT_PIN.load(core::sync::atomic::Ordering::Acquire) != 0 {
        return;
    }
    let serial = BOOT_VOLUME_SERIAL.load(core::sync::atomic::Ordering::Acquire);
    if serial == 0 {
        BOOT_PIN.store(2, core::sync::atomic::Ordering::Release);
        return; // the loader named nothing: no proof, the legacy claim stands (FRGUARD says DISARMED)
    }
    if sdhc_info().is_none() {
        return; // no card registered yet; asked again next pass
    }
    if !crate::fs::fat::volume_serials(crate::fs::fat::BlockSource::Sdhc).contains(&serial) {
        BOOT_PIN.store(2, core::sync::atomic::Ordering::Release);
        serial_println!("[block] boot medium=not-sdhc serial=0x{:08x}; usb index 0 may hold the global slot (legacy)", serial);
        return;
    }
    BOOT_PIN.store(1, core::sync::atomic::Ordering::Release);
    let evicted = {
        let mut g = BLOCK_DEVICE.lock();
        match *g {
            Some(d) if d.slot_id != 0 => {
                *g = None;
                Some(d.slot_id)
            }
            _ => None,
        }
    };
    if let Some(slot) = evicted {
        BOOT_MEDIUM_VERDICT.store(BM_UNKNOWN, core::sync::atomic::Ordering::Release);
        USB_PUBLISH_GEN.fetch_add(1, core::sync::atomic::Ordering::AcqRel); // a registry change: Quarry and Volumes re-read
        serial_println!("[block] boot medium=sdhc kept; usb slot={} separate (R95) evicted=global", slot);
    } else {
        serial_println!("[block] boot medium=sdhc kept; usb slot=- separate (R95)");
    }
}

/// USBSTOR: no Sdhc handle in this image — nothing to pin.
#[cfg(not(all(target_arch = "x86_64", feature = "sdhcblk")))]
pub fn pin_boot_medium_once() {}

// QEMUCLASS1 (rmbp-ledger B604) — TAIL-APPENDED (no line above moves). QEMUIMAGE26 booted from the AHCI ESP with
// QEMU's Sdhc card present: `pin_boot_medium_once` read the card, did not find the loader's serial on it and stored 2
// (`[block] boot medium=not-sdhc serial=0xfabe1afd`), and USBSTOR printed `boot_medium_kept=0 … -> FAIL
// reason=boot-medium-not-pinned` about a pin whose subject (a boot from the Sdhc card, R95) was never there.

/// USBSTOR: `true` only when the pin was decided AGAINST the card on a named serial — the loader handed a boot
/// volume serial and the Sdhc card's FAT volumes do not carry it. `false` while undecided, when kept, and when the
/// loader named nothing (serial 0: no proof either way, the fixture stays red).
pub fn boot_medium_not_sdhc() -> bool {
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    {
        return BOOT_PIN.load(core::sync::atomic::Ordering::Acquire) == 2
            && BOOT_VOLUME_SERIAL.load(core::sync::atomic::Ordering::Acquire) != 0;
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "sdhcblk")))]
    false
}

// ══════════ NVME registry (zenbook-ledger Z2, ZENBOOKNVME) — the NVMe namespace as its own registry ══════════
//
// TAIL-APPENDED (no line above moves). The AHCI first rung's shape (`AHCI_DISKS` / `register_ahci` /
// `read_blocks_ahci_ix` / the refusing `write_block_ahci`), for ONE namespace: `drivers/nvme.rs`
// publishes namespace 1 here after IDENTIFY. NVMEBOOT (zenbook-ledger Z9) made it a handle
// (`BlockHandle::Nvme`, `fs::fat::BlockSource::Nvme`) so the root walk sees it, and its writes granted
// (the NVMEBOOT section below). The global [`BLOCK_DEVICE`] is never touched.

/// NVME: the published namespace's geometry. Never held across I/O.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub static NVME_DISK: Mutex<Option<BlockDeviceInfo>> = Mutex::new(None);

/// NVME: publish namespace 1. Refuses a zero-block namespace or a block size outside 512..4096.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn register_nvme(num_blocks: u64, block_size: u32, model: &[u8; 40]) -> bool {
    if num_blocks == 0 || !(512..=4096).contains(&block_size) {
        serial_println!(":: [nvme] REFUSED to register ns=1 — blocks={} lba={} ::", num_blocks, block_size);
        return false;
    }
    let mut product = [b' '; 16];
    product.copy_from_slice(&model[..16]);
    let dev = BlockDeviceInfo { slot_id: 0, block_size, num_blocks, vendor: *b"NVMe    ", product };
    crate::arch::without_interrupts(|| *NVME_DISK.lock() = Some(dev));
    true
}

/// NVME: geometry of the published namespace, if any.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn nvme_info() -> Option<BlockDeviceInfo> {
    crate::arch::without_interrupts(|| *NVME_DISK.lock())
}

/// NVME: read `buf.len() / block_size` whole blocks at `lba`, DIRECTLY through the NVMe driver (it
/// chunks by its own MDTS and re-guards the range against the namespace size one layer down).
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn read_blocks_nvme(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let dev = nvme_info().ok_or(BlockError::NotReady)?;
    let count = span_blocks(&dev, lba, buf.len())?;
    let bs = dev.block_size as usize;
    crate::drivers::nvme::read_at(lba, &mut buf[..count * bs]).map_err(|_| BlockError::Io)?;
    Ok(buf.len())
}

// ══════════ NVMEBOOT (zenbook-ledger Z9) — the NVMe root grant and the granted write ══════════
//
// TAIL-APPENDED (no line above moves). AHCIROOT's slot shape (B332, the AHCIROOT section above) for the
// NVMe namespace: ONE grant, ONE inclusive absolute-LBA range, held for the boot (the root is written
// until power-off), never cleared. Its ONE minter is `fs::unafs::ensure_nvme_root_grant`, which grants
// exactly the one UnaFS volume `fs::unafs::locate_on(BlockHandle::Nvme)` finds — the extent the mount
// rides — so a foreign partition beside it (the Zenbook's Windows) is outside the grant by construction.
// The audit is `grep -rn hold_nvme_root_grant unaos/crates/kernel/src` -> this declaration + that call.
//
// [`write_blocks_nvme`] is the ONLY block-layer path to `drivers::nvme::write_at`: -EPERM unless the
// live grant covers every block of the run, else written and READ BACK as one run (AHCIROOT's
// `ahci_port_write` rule; NVMEBATCH zenbook-ledger Z22: one WRITE and one read-back per run, not per block). [`flush_nvme`] is the K8a commit barrier `fs::unafs::SdSectorDevice::flush`
// calls on an NVMe handle. The slot's lock is taken IRQ-masked, copied out, never held across I/O.

/// NVMEBOOT: THE slot — `(first_lba, last_lba)`, inclusive. `None` = every NVMe write is -EPERM.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
static NVME_ROOT_GRANT: Mutex<Option<(u64, u64)>> = Mutex::new(None);

/// NVMEBOOT: refusal lines printed so far (the first [`NVME_DENY_LINES`] print; the rest are counted).
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
static NVME_DENIED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
const NVME_DENY_LINES: u32 = 4;

/// NVMEBOOT: put the boot's root grant in the slot. `false` (and nothing changes) when the slot is already
/// held, when the range is empty or reversed, starts at LBA 0 (the partition table is never ours to write)
/// or runs past the namespace. ⚠ ONE caller: `fs::unafs::ensure_nvme_root_grant` — do not add a second.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn hold_nvme_root_grant(first: u64, last: u64) -> bool {
    let Some(dev) = nvme_info() else { return false };
    if first == 0 || last < first || last >= dev.num_blocks {
        return false;
    }
    crate::arch::without_interrupts(|| {
        let mut s = NVME_ROOT_GRANT.lock();
        if s.is_some() {
            return false;
        }
        *s = Some((first, last));
        true
    })
}

// ══════════ BOUNDEDWAIT (rmbp-ledger B981) — ONE bounded wait and ONE masked bounded hold for the block drivers ══════════
//
// TAIL-APPENDED (no line above moves). `drivers/ahci.rs`, `drivers/sdhc.rs` and `drivers/nvme.rs` each carried a
// byte-identical `cycles_ms` + `wait_ms`, and ahci's `dongle::with_port` and nvme's `with_ctrl` the same masked
// bounded try-lock skeleton; the copies are lifted here, and the drivers keep only their own lines and counters.
// The clock is the TSC (`arch::now_cycles()` against `arch::apic::tsc_hz()`), NOT `arch::ms()`: the probes run inside
// `pci::init` with interrupts masked, where the APIC tick does not advance. x86_64 only (every user is).

/// BOUNDEDWAIT: the TSC-unit bounded wait every x86 block driver spins on.
#[cfg(target_arch = "x86_64")]
pub mod wait {
    /// `ms` milliseconds in `arch::now_cycles()` (rdtsc) units. The TSC is calibrated against the ACPI PM timer by
    /// `apic::calibrate`, long before `pci::init`; the uncalibrated fallback derives from `arch::HW_WAIT_BUDGET`
    /// (the ~2 s pre-calibration guess).
    #[inline]
    pub fn cycles_ms(ms: u64) -> u64 {
        let hz = crate::arch::apic::tsc_hz();
        if hz != 0 {
            hz.saturating_mul(ms) / 1000
        } else {
            crate::arch::HW_WAIT_BUDGET.saturating_mul(ms) / 2000
        }
    }

    /// Spin until `pred()` holds or `ms` milliseconds elapse. Returns true if the predicate held.
    /// `wrapping_sub` so a 64-bit TSC wrap mid-wait cannot trip the deadline early.
    pub fn bounded<F: Fn() -> bool>(ms: u64, pred: F) -> bool {
        let start = crate::arch::now_cycles();
        let budget = cycles_ms(ms);
        loop {
            if pred() {
                return true;
            }
            if crate::arch::now_cycles().wrapping_sub(start) >= budget {
                return false;
            }
            core::hint::spin_loop();
        }
    }
}

/// BOUNDEDWAIT (DISKHOLD): the ONE masked bounded hold (the DONGLEBOOT B658 shape) a block driver takes its
/// controller lock through.
#[cfg(all(target_arch = "x86_64", any(feature = "ahci", feature = "nvme")))]
pub mod hold {
    /// Preempt-off for the whole hold (no preemption inside it; interrupts as the caller had them, PREEMPTOFFAUDIT Z57); `take` (the caller's own `try_lock`, so
    /// LOCKREG names the driver's acquire site) is retried until it yields a guard or `bound_ms` elapses —
    /// `at_once` refuses on the first miss (a caller that never waits, e.g. the render task). A refusal runs
    /// `refused(<TSC cycles spun>)` inside the hold (the driver's own line and counters); a taken guard runs
    /// `held(guard)`. `#[track_caller]` so the masked span (CLOCKCORE) is named at the driver's call.
    #[track_caller]
    pub fn with<G, R>(
        bound_ms: u64,
        at_once: bool,
        mut take: impl FnMut() -> Option<G>,
        refused: impl FnOnce(u64) -> R,
        held: impl FnOnce(G) -> R,
    ) -> R {
        crate::arch::sched::preemptoff::scoped(move || { // PREEMPTOFFAUDIT (zenbook-ledger Z57): preempt-off, interrupts LEFT ON (was without_interrupts; the census: no ISR takes CTRL/PORTS)
            let t0 = crate::arch::now_cycles();
            let bound = super::wait::cycles_ms(bound_ms);
            let guard = loop {
                if let Some(g) = take() {
                    break g;
                }
                let spun = crate::arch::now_cycles().wrapping_sub(t0);
                if at_once || spun >= bound {
                    return refused(spun);
                }
                core::hint::spin_loop();
            };
            held(guard)
        })
    }
}

/// NVMEBOOT: the live root grant, if any — `(first_lba, last_lba)`.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn nvme_root_grant() -> Option<(u64, u64)> {
    crate::arch::without_interrupts(|| *NVME_ROOT_GRANT.lock())
}

/// NVMEBOOT: read ONE block at `lba` into the front of `buf` (the single-block twin the handle and source
/// dispatchers call). Returns the block size read.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn read_block_nvme(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let bs = nvme_info().ok_or(BlockError::NotReady)?.block_size as usize;
    if buf.len() < bs {
        return Err(BlockError::Io);
    }
    read_blocks_nvme(lba, &mut buf[..bs])?;
    Ok(bs)
}

/// NVMEBOOT: write ONE block at `lba` from the front of `buf` — through [`write_blocks_nvme`]'s grant check.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn write_block_nvme(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let bs = nvme_info().ok_or(BlockError::NotReady)?.block_size as usize;
    if buf.len() < bs {
        return Err(BlockError::Io);
    }
    write_blocks_nvme(lba, &buf[..bs])
}

/// NVMEBOOT (NVMEWRITE): THE NVMe block-layer write. -EPERM unless the live root grant covers every block
/// of the run; otherwise written through `drivers::nvme::write_at` and read back with ONE `read_at` over the
/// run, compared block by block in memory (NVMEBATCH, zenbook-ledger Z22: `fs::unafs::handle_write_multi` hands a
/// whole 4 KiB UnaFS block here as one run; the driver chunks by its own MDTS). NVME4KN (zenbook-ledger Z31): `lba` and
/// the grant are the namespace's OWN LBAs on every LBA size; [`write_sectors_nvme`] maps the 512 B sector view onto them.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn write_blocks_nvme(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let dev = nvme_info().ok_or(BlockError::NotReady)?;
    let count = span_blocks(&dev, lba, buf.len())?;
    let last = lba + count as u64 - 1;
    match nvme_root_grant() {
        Some((a, b)) if lba >= a && last <= b => {}
        other => {
            let k = NVME_DENIED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if k < NVME_DENY_LINES {
                match other {
                    None => serial_println!(":: [nvme] write -EPERM lba={} grant=none — nothing written ::", lba),
                    Some((a, b)) => serial_println!(":: [nvme] write -EPERM lba={} grant={}..{} — nothing written ::", lba, a, b),
                }
            }
            return Err(BlockError::Denied);
        }
    }
    let bs = dev.block_size as usize;
    crate::drivers::nvme::write_at(lba, buf).map_err(|_| BlockError::Io)?;
    let mut rb = alloc::vec![0u8; count * bs];
    crate::drivers::nvme::read_at(lba, &mut rb).map_err(|_| BlockError::Io)?;
    if let Some(i) = (0..count).find(|&i| rb[i * bs..(i + 1) * bs] != buf[i * bs..(i + 1) * bs]) {
        serial_println!(":: [nvme] write readback MISMATCH lba={} — the medium does not hold what was written ::", lba + i as u64);
        return Err(BlockError::Io);
    }
    NVME_RUNS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    NVME_RUN_BLOCKS.fetch_add(count as u32, core::sync::atomic::Ordering::Relaxed);
    Ok(())
}

/// NVMEBATCH (zenbook-ledger Z22): granted runs written and verified this boot, and the blocks they carried — the
/// `tests nvme` line's `runs=` / `run_blocks=` (a UnaFS root's run is 8 blocks; before Z22 every run was 1).
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
static NVME_RUNS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
static NVME_RUN_BLOCKS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// NVMEBATCH: `(runs, blocks)` written through [`write_blocks_nvme`] this boot.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn nvme_run_census() -> (u32, u32) {
    (NVME_RUNS.load(core::sync::atomic::Ordering::Relaxed), NVME_RUN_BLOCKS.load(core::sync::atomic::Ordering::Relaxed))
}

/// NVMEBOOT: NVM Flush on namespace 1 — the commit barrier (a controller with no volatile write cache
/// completes it as a no-op, NVMe 1.4).
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn flush_nvme() -> Result<(), BlockError> {
    crate::drivers::nvme::flush().map_err(|_| BlockError::Io)
}

/// NVMEBOOT: the NVMe root's standing write refusal, as [`handle_write_veto`] reports it (the root-granted
/// namespace is lifted by `native_mount_write_veto`, AHCIROOT's shape).
#[cfg(all(feature = "sdwrite", target_arch = "x86_64", feature = "nvme"))]
const NATIVE_NVME_VETO: &str = "the volume rides the NVMe namespace and the block layer refuses every \
                                write outside the boot's NVMe root grant (one UnaFS extent, NVMEBOOT)";

// ══════════ NVME4KN (zenbook-ledger Z31) — the NVMe namespace as a 512 B sector view, on any LBA size ══════════
//
// TAIL-APPENDED. `fs::unafs::SdSectorDevice` (the unafs crate's `SectorDevice`) addresses 512 B sectors on every handle;
// on the NVMe that space is a VIEW over the namespace: sector `s` is byte `s * 512`, LBA `s >> k` with
// k = log2(lba_bytes) - 9 (0 on a 512 B namespace, 3 on 4Kn). This section is the ONE place the mapping lives. Every
// entry point issues only [`read_blocks_nvme`] / [`write_blocks_nvme`] (the grant check and the read-back stay theirs);
// on k = 0 each one is exactly the call it replaced. A 512 B write on k > 0 (the unafs K8a root flip, the keyslot) is a
// read-modify-write of its ONE LBA through the granted write: a one-LBA NVMe write is power-fail atomic (AWUPF is
// 0's-based) and the sibling bytes are rewritten as just read, so the flip stays one device write. The pair runs under
// the UnaFS MOUNT lock that holds every write of the bound mount (BP-storage-8); `CTRL` is held per command only.
// Any other sub-LBA write is refused by name, nothing written.

/// NVME4KN: sub-LBA writes served by read-modify-write this boot (`tests nvme`'s `rmw=`).
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
static NVME_RMW: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// NVME4KN: unaligned-write refusals so far (the first [`NVME_DENY_LINES`] print).
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
static NVME_UNALIGNED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// NVME4KN: `k` = log2(LBA size) - 9 of the published namespace (0 for 512 B .. 3 for 4 KiB); `None` with no namespace
/// or a size that is not a power of two in 512..=4096.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn nvme_sector_shift() -> Option<u32> {
    let bs = nvme_info()?.block_size;
    if bs.is_power_of_two() && (512..=4096).contains(&bs) { Some(bs.trailing_zeros() - 9) } else { None }
}

/// NVME4KN: `(k, block size)` of the published namespace, or the refusal the block layer gives.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
fn nvme_view() -> Result<(u32, usize), BlockError> {
    let dev = nvme_info().ok_or(BlockError::NotReady)?;
    let k = nvme_sector_shift().ok_or(BlockError::Io)?;
    Ok((k, dev.block_size as usize))
}

/// NVME4KN: read `buf.len() / 512` view sectors from `sector`. Whole LBAs go straight to [`read_blocks_nvme`]
/// (chunked by `MAX_BLOCKS_PER_OP`, as `fs::unafs::nvme_read_multi` did); a span that is not whole LBAs is served LBA
/// by LBA through a one-LBA bounce.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn read_sectors_nvme(sector: u64, buf: &mut [u8]) -> Result<(), BlockError> {
    if buf.is_empty() || buf.len() % SECTOR_BYTES != 0 {
        return Err(BlockError::Io);
    }
    let (k, bs) = nvme_view()?;
    let per = 1u64 << k;
    if sector % per == 0 && buf.len() % bs == 0 {
        for (i, c) in buf.chunks_mut(MAX_BLOCKS_PER_OP * bs).enumerate() {
            read_blocks_nvme((sector >> k) + (i * MAX_BLOCKS_PER_OP) as u64, c)?;
        }
        return Ok(());
    }
    let mut b = alloc::vec![0u8; bs];
    let mut done = 0usize;
    while done < buf.len() {
        let s = sector.checked_add((done / SECTOR_BYTES) as u64).ok_or(BlockError::BadLba)?;
        read_blocks_nvme(s >> k, &mut b)?;
        let off = ((s & (per - 1)) as usize) * SECTOR_BYTES;
        let n = core::cmp::min(bs - off, buf.len() - done);
        buf[done..done + n].copy_from_slice(&b[off..off + n]);
        done += n;
    }
    Ok(())
}

/// NVME4KN: write `buf.len() / 512` view sectors at `sector`. Whole LBAs -> ONE [`write_blocks_nvme`] run (as
/// `fs::unafs::handle_write_multi` did); exactly one 512 B sector inside a larger LBA -> read-modify-write of that LBA
/// through the same granted write; anything else is refused by name and nothing is written.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn write_sectors_nvme(sector: u64, buf: &[u8]) -> Result<(), BlockError> {
    if buf.is_empty() || buf.len() % SECTOR_BYTES != 0 {
        return Err(BlockError::Io);
    }
    let (k, bs) = nvme_view()?;
    let per = 1u64 << k;
    if sector % per == 0 && buf.len() % bs == 0 {
        return write_blocks_nvme(sector >> k, buf);
    }
    if buf.len() == SECTOR_BYTES {
        let mut b = alloc::vec![0u8; bs];
        read_blocks_nvme(sector >> k, &mut b)?;
        let off = ((sector & (per - 1)) as usize) * SECTOR_BYTES;
        b[off..off + SECTOR_BYTES].copy_from_slice(buf);
        write_blocks_nvme(sector >> k, &b)?;
        NVME_RMW.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        return Ok(());
    }
    if NVME_UNALIGNED.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < NVME_DENY_LINES {
        serial_println!(":: [nvme] write refused sector={} len={} lba={} — not whole LBAs, nothing written ::", sector, buf.len(), bs);
    }
    Err(BlockError::Io)
}

/// NVME4KN: ONE 512 B view sector at `sector` into the front of `buf` (the `fs::unafs::handle_read` arm). Returns 512.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn read_sector_nvme(sector: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    if buf.len() < SECTOR_BYTES {
        return Err(BlockError::Io);
    }
    read_sectors_nvme(sector, &mut buf[..SECTOR_BYTES])?;
    Ok(SECTOR_BYTES)
}

/// NVME4KN: ONE 512 B view sector at `sector` from the front of `buf` (the `fs::unafs::handle_write` arm).
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn write_sector_nvme(sector: u64, buf: &[u8]) -> Result<(), BlockError> {
    if buf.len() < SECTOR_BYTES {
        return Err(BlockError::Io);
    }
    write_sectors_nvme(sector, &buf[..SECTOR_BYTES])
}

/// NVME4KN: sub-LBA writes served by read-modify-write this boot.
#[cfg(all(target_arch = "x86_64", feature = "nvme"))]
pub fn nvme_rmw_count() -> u32 {
    NVME_RMW.load(core::sync::atomic::Ordering::Relaxed)
}

// ══════════ PREEMPTOFFXHCILOAN (zenbook-ledger Z62) — the block layer's xHCI loan held preempt-off; a masked claimant waits on a RUNNING holder only ══════════
//
// TAIL-APPENDED (no line above moves; `claim_xhci_for_io` changed in place, line-neutral). WEDGE-8 F3 gave a MASKED
// `claim_xhci_for_io` caller (the FAT RMW spans, `with_unafs`) `Busy` at once, because the loan holder was preemptible:
// a masked spinner could wait on a holder preempted on its own core. The block layer's holders (eight, each a local loan
// for one BOT transaction) never sleep under the loan (their waits are `crate::hlt()` at IF=1), and no ISR touches the
// loan's state (`xhci_msi_handler` is MMIO and one atomic), so the take now holds this CPU's preempt-off count
// (BP-cores-23) from before the take to after the put and publishes the holder's CPU in `BLOCK_LOAN_CPU`. A masked
// claimant spins (`wait::bounded`, `LOAN_MASKED_WAIT_MS`) only while the published holder is on ANOTHER CPU — a running
// holder — and answers `Busy` at once for an unpublished holder (the pump, xhci-poll, ftdi, usbnet, ptp: preemptible as
// before) or a same-CPU one. aarch64 keeps the at-once refusal (PREEMPTOFF-ARM owed). Owed: XHCILOAN-FAILHOLD (a failing
// BOT ladder holds preempt-off up to 3x `hw_wait_budget`), XHCILOAN-POLL, XHCILOAN-PUMP (refused), XHCILOAN-PTPCOPY.

/// PREEMPTOFFXHCILOAN: the block layer's loan of the xHCI controller. Derefs to [`XhciLoan`]; on x86 it holds this CPU's
/// preempt-off count from before the take (`take_block_loan`) to after the put (its Drop), and `tag` is the published
/// holder word (cpu + 1; 0 on aarch64).
pub struct BlockLoan {
    loan: Option<XhciLoan>,
    tag: u32,
}

impl core::ops::Deref for BlockLoan {
    type Target = XhciLoan;
    #[inline]
    fn deref(&self) -> &XhciLoan {
        self.loan.as_ref().expect("BlockLoan invariant: Some until drop")
    }
}

impl core::ops::DerefMut for BlockLoan {
    #[inline]
    fn deref_mut(&mut self) -> &mut XhciLoan {
        self.loan.as_mut().expect("BlockLoan invariant: Some until drop")
    }
}

impl Drop for BlockLoan {
    fn drop(&mut self) {
        // The put first (XhciLoan's masked O(1) put), while this CPU is still preempt-off; then the publication is
        // withdrawn (only if it is still ours) and the count released.
        #[cfg(target_arch = "x86_64")] let shed = loan_shed_take(); drop(self.loan.take()); // XHCILOANFAILHOLD (Z66): the shed mark is read BEFORE the put (still held: no other block loan can exist to clear it)
        #[cfg(target_arch = "x86_64")]
        { if self.tag == 0 { return; } if shed { return; } // XHCILOANPTPCOPY (Z69): a demoted (claim-only) loan holds no count and no publication. XHCILOANFAILHOLD (Z66): a shed loan already withdrew its publication and released its count (`xhci_loan_shed`)
            let _ = BLOCK_LOAN_CPU.compare_exchange(
                self.tag,
                0,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Relaxed,
            );
            crate::arch::sched::preemptoff::enable();
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = self.tag;
    }
}

/// PREEMPTOFFXHCILOAN: the CPU (+1) of the block-layer holder of controller 0's loan, held preempt-off; 0 = none
/// published. Written by the holder (after its take, cleared by CAS after its put), read by masked claimants. Lock-free.
#[cfg(target_arch = "x86_64")]
static BLOCK_LOAN_CPU: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// PREEMPTOFFXHCILOAN: the longest a MASKED claimant spins on a running holder: past a healthy 32 KiB BOT transaction
/// (~1-2 ms at USB2), and inside the 20 ms masked goal (CLOCKCORE / HIDSTALL `masked_max_ms`).
pub const LOAN_MASKED_WAIT_MS: u64 = 10;

/// PREEMPTOFFXHCILOAN: masked waits that got the loan, and masked claims refused, this boot; the last refusal's why.
#[cfg(target_arch = "x86_64")]
static LOAN_MASKED_OK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(target_arch = "x86_64")]
static LOAN_MASKED_BUSY: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(target_arch = "x86_64")]
static LOAN_LAST_WHY: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
#[cfg(target_arch = "x86_64")]
static LOAN_REFUSAL_SAID: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[cfg(target_arch = "x86_64")]
fn loan_why_word(code: u8) -> &'static str {
    match code {
        1 => "holder-preemptible",
        2 => "same-cpu",
        3 => "masked-timeout(10ms)",
        4 => "claimant-long-hold", // XHCILOANPTPCOPY (Z69): a masked PTP/MTP claimant (its transaction hlt-waits) is refused at once
        _ => "none",
    }
}

/// PREEMPTOFFXHCILOAN: take controller 0's loan for the block layer — this CPU's preempt-off count held first, the holder
/// published on success, the count released on a refusal.
#[cfg(target_arch = "x86_64")]
fn take_block_loan() -> Result<BlockLoan, XhciClaimError> {
    crate::arch::sched::preemptoff::disable();
    match xhci::claim() {
        Ok(l) => {
            if loan_poll_yield_skip() { drop(l); crate::arch::sched::preemptoff::enable(); return Err(XhciClaimError::Busy); } let tag = crate::arch::percpu::this_cpu().cpu_index.saturating_add(1); loan_shed_arm(); loan_yield_close(); // XHCILOANPTPYIELDPOLLSKIP (Z86): the xhci-poll take inside an open PTP yield window goes straight back (unpublished, the closer left OPEN). XHCILOANFAILHOLD (Z66): the loan's age starts here, its shed mark cleared. XHCILOANPTPYIELDPOLLTAKE (Z82): an open PTP yield window records its closer BEFORE the publish below
            BLOCK_LOAN_CPU.store(tag, core::sync::atomic::Ordering::Release);
            Ok(BlockLoan { loan: Some(l), tag })
        }
        Err(e) => {
            crate::arch::sched::preemptoff::enable();
            Err(e)
        }
    }
}

/// PREEMPTOFFXHCILOAN: the aarch64 take is the plain claim (no preempt-off count there yet: PREEMPTOFF-ARM).
#[cfg(not(target_arch = "x86_64"))]
fn take_block_loan() -> Result<BlockLoan, XhciClaimError> {
    xhci::claim().map(|l| BlockLoan { loan: Some(l), tag: 0 })
}

/// PREEMPTOFFXHCILOAN: a MASKED claimant met a held loan. Spin re-taking it for at most [`LOAN_MASKED_WAIT_MS`], but only
/// while the published holder is a preempt-off block holder on ANOTHER CPU (running, so it returns the loan); an
/// unpublished holder (preemptible) or one on this CPU answers `Busy` at once, as WEDGE-8 F3 did. The first refusal is
/// said once.
#[cfg(target_arch = "x86_64")]
fn masked_loan_wait() -> Result<BlockLoan, BlockError> {
    let me = crate::arch::percpu::this_cpu().cpu_index.saturating_add(1);
    let got: core::cell::Cell<Option<Result<BlockLoan, BlockError>>> = core::cell::Cell::new(None);
    let why = core::cell::Cell::new(0u8);
    let t0 = crate::arch::now_cycles();
    let _ = wait::bounded(LOAN_MASKED_WAIT_MS, || match take_block_loan() {
        Ok(l) => {
            got.set(Some(Ok(l)));
            true
        }
        Err(XhciClaimError::NotReady) => {
            got.set(Some(Err(BlockError::NotReady)));
            true
        }
        Err(XhciClaimError::Busy) => {
            let h = BLOCK_LOAN_CPU.load(core::sync::atomic::Ordering::Acquire);
            if h == 0 {
                why.set(1);
                true
            } else if h == me {
                why.set(2);
                true
            } else {
                false
            }
        }
    });
    if let Some(r) = got.into_inner() {
        if r.is_ok() {
            LOAN_MASKED_OK.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        return r;
    }
    let code = if why.get() == 0 { 3 } else { why.get() };
    LOAN_LAST_WHY.store(code, core::sync::atomic::Ordering::Relaxed);
    LOAN_MASKED_BUSY.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    say_loan_refusal("block", code, t0); // XHCILOANPTPCOPY (Z69): the ONE refusal line and latch, `who=` named
    Err(BlockError::Busy)
}

/// PREEMPTOFFXHCILOAN: aarch64 keeps WEDGE-8's at-once refusal for a masked claimant (PREEMPTOFF-ARM owed).
#[cfg(not(target_arch = "x86_64"))]
fn masked_loan_wait() -> Result<BlockLoan, BlockError> {
    Err(BlockError::Busy)
}

/// PREEMPTOFFXHCILOAN: `(masked waits that got the loan, masked claims refused)` this boot.
#[cfg(target_arch = "x86_64")]
pub fn xhci_loan_masked_census() -> (u32, u32) {
    (
        LOAN_MASKED_OK.load(core::sync::atomic::Ordering::Relaxed),
        LOAN_MASKED_BUSY.load(core::sync::atomic::Ordering::Relaxed),
    )
}

/// PREEMPTOFFXHCILOAN KAT handshake: 0 idle, 1 the holder task holds the loan, 2 the masked claimant is spinning,
/// 3 the holder put the loan back, 9 the holder never got it.
#[cfg(target_arch = "x86_64")]
static LOAN_KAT_STATE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// PREEMPTOFFXHCILOAN KAT holder (a task on a sibling CPU): take the block loan unmasked, hold it until the claimant
/// spins (<= 200 ms) and 2 ms more, put it back.
#[cfg(target_arch = "x86_64")]
fn loan_kat_holder(_: usize) {
    use core::sync::atomic::Ordering;
    let Ok(l) = claim_xhci_for_io() else {
        LOAN_KAT_STATE.store(9, Ordering::Release);
        return;
    };
    LOAN_KAT_STATE.store(1, Ordering::Release);
    let _ = wait::bounded(200, || LOAN_KAT_STATE.load(Ordering::Acquire) == 2);
    let _ = wait::bounded(2, || false);
    drop(l);
    LOAN_KAT_STATE.store(3, Ordering::Release);
}

/// PREEMPTOFFXHCILOAN: `tests preemptaudit`'s loan leg — a task on a sibling CPU holds the block loan (preempt-off,
/// published), and a MASKED claimant on this CPU must get it inside [`LOAN_MASKED_WAIT_MS`]. Returns
/// `(loan_wait word, µs the masked claim took)`: `ok` / `refused-<why>` / `notready` / `err`, or
/// `skip(masked|no-xhci|one-cpu|holder-late)`.
#[cfg(target_arch = "x86_64")]
pub fn xhci_loan_wait_kat() -> (&'static str, u64) {
    use core::sync::atomic::Ordering;
    if crate::arch::irqs_masked() {
        return ("skip(masked)", 0);
    }
    if matches!(xhci::claim(), Err(XhciClaimError::NotReady)) {
        return ("skip(no-xhci)", 0);
    }
    let me = crate::arch::percpu::this_cpu().cpu_index as usize;
    let cpu = crate::arch::sched::sibling_online_cpu(me);
    if cpu == me {
        return ("skip(one-cpu)", 0);
    }
    LOAN_KAT_STATE.store(0, Ordering::Release);
    crate::arch::sched::spawn("loanwait-kat", loan_kat_holder, 0, cpu, crate::arch::sched::PRIO_HIGH);
    if !wait::bounded(1000, || LOAN_KAT_STATE.load(Ordering::Acquire) == 1) {
        return ("skip(holder-late)", 0);
    }
    let (word, cycles) = crate::arch::without_interrupts(|| {
        LOAN_KAT_STATE.store(2, Ordering::Release);
        let t0 = crate::arch::now_cycles();
        let r = claim_xhci_for_io();
        let c = crate::arch::now_cycles().wrapping_sub(t0);
        let w = match &r {
            Ok(_) => "ok",
            Err(BlockError::Busy) => match LOAN_LAST_WHY.load(Ordering::Relaxed) {
                1 => "refused-holder-preemptible",
                2 => "refused-same-cpu",
                3 => "refused-masked-timeout",
                _ => "refused",
            },
            Err(BlockError::NotReady) => "notready",
            Err(_) => "err",
        };
        drop(r);
        (w, c)
    });
    let _ = wait::bounded(200, || LOAN_KAT_STATE.load(Ordering::Acquire) >= 3);
    (word, cycles.saturating_mul(1000) / wait::cycles_ms(1).max(1))
}

// ══════════ XHCILOANPTPCOPY (zenbook-ledger Z69) — the PTP/MTP controller claim takes the ONE block-loan path, demoted to claim-only ══════════
//
// TAIL-APPENDED. `drivers/xhci/ptp.rs` `with_controller` copied `claim_xhci_for_io`'s hlt loop on the same controller-0
// claim word. A PTP/MTP hold is measured at seconds to minutes (`mtp::READ_CHUNK` 64 MiB, GetObject to `WHOLE_CAP`
// 128 MiB, the shutter's >= 1 s ObjectAdded watch, a whole-file SendObject), far past the loan's one-BOT-transaction
// expectation, so it must hold neither the preempt-off count (BP-cores-23) nor the publication: `claim_only` takes the
// ONE path (`claim_xhci_for_io`) and demotes the loan at once. A masked PTP claimant is refused at once (its transaction
// `hlt`-waits, which at IF=0 never wakes), counted and said by the one refusal line (`who=ptp`). Owed: XHCILOAN-PTPYIELD.

/// XHCILOANPTPCOPY: PTP/MTP claims that took the loan through [`BlockLoan::claim_only`] this boot.
static LOAN_PTP_CLAIMS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

impl BlockLoan {
    /// XHCILOANPTPCOPY: the controller loan for a LONG holder (the PTP/MTP transport): the ONE claim path
    /// (`claim_xhci_for_io`: its unmasked bounded hlt wait), then demoted — no preempt-off count, no publication — so a
    /// masked block claimant meets an unpublished holder and is told `Busy` at once (`why=holder-preemptible`). A masked
    /// caller is refused at once (`why=claimant-long-hold`).
    pub fn claim_only() -> Result<BlockLoan, BlockError> {
        if crate::arch::irqs_masked() {
            ptp_masked_refusal();
            return Err(BlockError::Busy);
        }
        let _ptp = LoanPtpMark::set(); let mut l = claim_xhci_for_io()?; // XHCILOANPTPYIELDPTPTAKE (Z87): this task is marked a PTP claimant until claim_only returns
        l.demote();
        LOAN_PTP_CLAIMS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        Ok(l)
    }

    /// XHCILOANPTPCOPY: withdraw this loan's publication (only if still ours) and release the count its take holds;
    /// `tag` 0 then tells the Drop there is nothing left to release.
    fn demote(&mut self) {
        #[cfg(target_arch = "x86_64")]
        {
            if self.tag != 0 {
                let _ = BLOCK_LOAN_CPU.compare_exchange(
                    self.tag,
                    0,
                    core::sync::atomic::Ordering::AcqRel,
                    core::sync::atomic::Ordering::Relaxed,
                );
                self.tag = 0;
                crate::arch::sched::preemptoff::enable();
            }
        }
    }
}

// ══════════ XHCILOANPTPYIELD (zenbook-ledger Z73) — a PTP/MTP data phase hands the loan back to a block claimant between TDs ══════════
//
// A claim-only PTP hold (Z69) keeps controller 0 for a whole data phase: a 64 MiB GetPartialObject (`mtp::READ_CHUNK`) is
// 2048 TDs of 32 KiB (`STORAGE_DATA_BYTES`), seconds at USB2, and a masked block claimant (FAT RMW, `with_unafs`) is told
// `Busy` at once (why=holder-preemptible) and gives up after its 64 / 8 tick-paced retries. `ptp::loan_yield` calls
// `yield_now` from the ONE bulk-in loop (`ptp_txn_inner`'s Phase::In) between two TDs (nothing of PTP armed; a bulk
// transfer has no inter-TD timing): the PUT (`XhciLoan`'s masked O(1) put), a bounded WINDOW in which a block claimant
// takes it (its take publishes `BLOCK_LOAN_CPU`), the RE-TAKE through `claim_only` again (its bounded wait is the ONE
// path's), pointer-checked to be the same heap controller. Only a demoted loan (tag 0) yields. Owed: XHCILOAN-PTPYIELD-ARM.

/// XHCILOANPTPYIELD: a periodic yield every this many TDs (4 MiB of 32 KiB TDs) ...
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
pub const LOAN_YIELD_TDS: u32 = 128;
/// XHCILOANPTPYIELD: ... and no sooner than this many ms after the last (a no-claimant window then costs <= 3/50 of a pull).
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
pub const LOAN_YIELD_MIN_MS: u64 = 50;
/// XHCILOANPTPYIELD: a masked claimant refused since the last yield makes one due after this many TDs (256 KiB).
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
pub const LOAN_YIELD_DEMAND_TDS: u32 = 8;
/// XHCILOANPTPYIELD: the window on the ms clock (>= 2 real ms: two 1 kHz ticks on every CPU, so a hlt-waiter retries).
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
pub const LOAN_YIELD_WINDOW_MS: u64 = 3;
/// XHCILOANPTPYIELD: `claim_only` attempts for the re-take (~8 s at x86's 2 s budget: past a failing BOT ladder's 3x first attempt).
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
const LOAN_YIELD_RETAKES: u32 = 4;

/// XHCILOANPTPYIELD: TDs since the last yield, `arch::ms` at it, `LOAN_MASKED_BUSY` at it (one PTP hold at a time: written
/// only by the holder).
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
static LOAN_YIELD_TDS_SINCE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
static LOAN_YIELD_LAST_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
static LOAN_YIELD_SEEN_BUSY: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// XHCILOANPTPYIELD: yields this boot, and windows a block claimant took.
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
static LOAN_YIELDS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
static LOAN_YIELD_TAKEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
impl BlockLoan {
    /// XHCILOANPTPYIELD: one TD of a PTP data phase passed; a yield is due at >= `LOAN_YIELD_TDS` TDs and
    /// >= `LOAN_YIELD_MIN_MS` since the last, or at >= `LOAN_YIELD_DEMAND_TDS` when a masked claimant was refused since.
    pub fn yield_due() -> bool {
        use core::sync::atomic::Ordering;
        let n = LOAN_YIELD_TDS_SINCE.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let periodic = n >= LOAN_YIELD_TDS
            && crate::arch::ms().saturating_sub(LOAN_YIELD_LAST_MS.load(Ordering::Relaxed)) >= LOAN_YIELD_MIN_MS;
        let demand = n >= LOAN_YIELD_DEMAND_TDS
            && LOAN_MASKED_BUSY.load(Ordering::Relaxed) != LOAN_YIELD_SEEN_BUSY.load(Ordering::Relaxed);
        periodic || demand
    }

    /// XHCILOANPTPYIELD: put the loan back, give a block claimant a bounded window, re-take it through `claim_only`.
    /// `Ok((µs put -> re-take, a block claimant took the window))`; `Ok((0, false))` with nothing moved for a loan that must
    /// not yield (masked, preempt-off held, published, absent). `Err` leaves the loan EMPTY: the caller must not touch the
    /// controller again (the Drop of an empty demoted loan does nothing).
    pub fn yield_now(&mut self) -> Result<(u64, bool), BlockError> {
        use core::sync::atomic::Ordering;
        if crate::arch::irqs_masked() || crate::arch::sched::preemptoff::witness().0 || self.tag != 0 || self.loan.is_none() {
            return Ok((0, false));
        }
        let before: *const crate::drivers::xhci::XhciController = &***self;
        let t0 = crate::arch::now_cycles();
        LOAN_YIELD_CLOSER.store(LOAN_CLOSER_OPEN, Ordering::Release); drop(self.loan.take()); // the PUT: XhciLoan's masked O(1) put. XHCILOANPTPYIELDPOLLTAKE (Z82): the window is marked open while this hold still has the loan
        let m0 = crate::arch::ms();
        let mut taken = false;
        loop {
            if BLOCK_LOAN_CPU.load(Ordering::Acquire) != 0 {
                taken = true; // a block take published: the re-take below waits out its transaction
                break;
            }
            if crate::arch::ms().saturating_sub(m0) >= LOAN_YIELD_WINDOW_MS {
                break;
            }
            crate::arch::sched::yield_now(); // a same-CPU claimant runs
        }
        let mut tries = 0u32;
        let mut l = loop {
            match BlockLoan::claim_only() {
                Ok(l) => break l,
                Err(BlockError::Busy) => {
                    tries += 1;
                    if tries >= LOAN_YIELD_RETAKES {
                        return Err(BlockError::Busy);
                    }
                }
                Err(e) => return Err(e),
            }
        };
        let after: *const crate::drivers::xhci::XhciController = &**l;
        if !core::ptr::eq(before, after) {
            return Err(BlockError::Io); // `l` drops: the controller goes back
        }
        self.loan = l.loan.take(); // the demoted temporary drops empty (tag 0)
        LOAN_YIELD_TDS_SINCE.store(0, Ordering::Relaxed);
        LOAN_YIELD_LAST_MS.store(crate::arch::ms(), Ordering::Relaxed);
        LOAN_YIELD_SEEN_BUSY.store(LOAN_MASKED_BUSY.load(Ordering::Relaxed), Ordering::Relaxed);
        LOAN_YIELDS.fetch_add(1, Ordering::Relaxed);
        if taken {
            LOAN_YIELD_TAKEN.fetch_add(1, Ordering::Relaxed);
        }
        let us = crate::arch::now_cycles().wrapping_sub(t0).saturating_mul(1000) / wait::cycles_ms(1).max(1);
        Ok((us, taken))
    }
}

/// XHCILOANPTPYIELD: aarch64 never yields (no `BLOCK_LOAN_CPU` publication there: XHCILOAN-PTPYIELD-ARM owed).
#[cfg(all(feature = "uvc", not(target_arch = "x86_64")))]
impl BlockLoan {
    pub fn yield_due() -> bool {
        false
    }
    pub fn yield_now(&mut self) -> Result<(u64, bool), BlockError> {
        Ok((0, false))
    }
}

/// XHCILOANPTPYIELD: `(yields, windows a block claimant took)` this boot.
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
pub fn xhci_loan_yields() -> (u32, u32) {
    (
        LOAN_YIELDS.load(core::sync::atomic::Ordering::Relaxed),
        LOAN_YIELD_TAKEN.load(core::sync::atomic::Ordering::Relaxed),
    )
}

/// XHCILOANPTPYIELD KAT handshake: 0 idle, 1 the claimant is about to wait, 2 it holds the loan, 3 it put it back, 9 refused.
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
static LOAN_YIELD_KAT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// XHCILOANPTPYIELD KAT claimant (a task on a sibling CPU, Z62's `loan_kat_holder` shape): an unmasked block claim that
/// hlt-waits while the KAT's PTP claim holds the loan, takes it in the yield window, holds 1 ms, puts it back.
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
fn loan_yield_kat_claimant(_: usize) {
    use core::sync::atomic::Ordering;
    LOAN_YIELD_KAT.store(1, Ordering::Release);
    match claim_xhci_for_io() {
        Ok(l) => {
            LOAN_YIELD_KAT.store(2, Ordering::Release);
            let _ = wait::bounded(1, || false);
            drop(l);
            LOAN_YIELD_KAT.store(3, Ordering::Release);
        }
        Err(_) => LOAN_YIELD_KAT.store(9, Ordering::Release),
    }
}

/// XHCILOANPTPYIELD: arm the KAT claimant on a sibling CPU and wait until it is in its claim wait (called while the
/// PTP claim holds the loan). `Err` = `skip(one-cpu)` / `skip(claimant-late)`.
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
pub fn xhci_loan_yield_kat_arm() -> Result<(), &'static str> {
    use core::sync::atomic::Ordering;
    let me = crate::arch::percpu::this_cpu().cpu_index as usize;
    let cpu = crate::arch::sched::sibling_online_cpu(me);
    if cpu == me {
        return Err("skip(one-cpu)");
    }
    LOAN_YIELD_KAT.store(0, Ordering::Release);
    crate::arch::sched::spawn("ptpyield-kat", loan_yield_kat_claimant, 0, cpu, crate::arch::sched::PRIO_HIGH);
    if !wait::bounded(1000, || LOAN_YIELD_KAT.load(Ordering::Acquire) == 1) {
        return Err("skip(claimant-late)");
    }
    let _ = wait::bounded(2, || false); // it is in `claim_xhci_for_io`'s hlt wait now
    Ok(())
}

/// XHCILOANPTPYIELD: after the KAT's PTP claim is put back, wait (<= 2500 ms) for the claimant to finish.
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
pub fn xhci_loan_yield_kat_settle() {
    let _ = wait::bounded(2500, || LOAN_YIELD_KAT.load(core::sync::atomic::Ordering::Acquire) >= 3);
}

/// XHCILOANPTPCOPY: PTP/MTP claims through the one path this boot (read by `ptp::loan_path_kat`).
pub fn xhci_loan_ptp_claims() -> u32 {
    LOAN_PTP_CLAIMS.load(core::sync::atomic::Ordering::Relaxed)
}

/// XHCILOANPTPCOPY: whether a preempt-off block holder of controller 0's loan is published right now.
pub fn xhci_loan_published() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        BLOCK_LOAN_CPU.load(core::sync::atomic::Ordering::Acquire) != 0
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// XHCILOANPTPCOPY: a MASKED PTP/MTP claimant, refused at once: counted with the masked refusals, said once.
#[cfg(target_arch = "x86_64")]
fn ptp_masked_refusal() {
    LOAN_LAST_WHY.store(4, core::sync::atomic::Ordering::Relaxed);
    LOAN_MASKED_BUSY.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    say_loan_refusal("ptp", 4, crate::arch::now_cycles());
}

/// XHCILOANPTPCOPY: aarch64 keeps the silent at-once refusal (PREEMPTOFF-ARM owed).
#[cfg(not(target_arch = "x86_64"))]
fn ptp_masked_refusal() {}

/// XHCILOANPTPCOPY: the ONE `[xhci] loan` first-refusal line (the boot's first masked refusal, once), naming the
/// refused claimant (`block` | `ptp`).
#[cfg(target_arch = "x86_64")]
fn say_loan_refusal(who: &str, code: u8, t0: u64) {
    if !LOAN_REFUSAL_SAID.swap(true, core::sync::atomic::Ordering::AcqRel) {
        let us = crate::arch::now_cycles().wrapping_sub(t0).saturating_mul(1000) / wait::cycles_ms(1).max(1);
        serial_println!(
            "[xhci] loan masked claim -> Busy who={} why={} waited_us={} (PREEMPTOFFXHCILOAN Z62, first refusal, once)",
            who,
            loan_why_word(code),
            us
        );
    }
}

// ══════════ XHCILOANFAILHOLD (zenbook-ledger Z66) — a block loan in a BOT wait sheds its non-preemption ══════════
//
// TAIL-APPENDED after Z62's KAT (Z62's `Drop` and `take_block_loan` gain one same-line token each, line-neutral). Z62 held
// the block loan preempt-off from take to put; a FAILING BOT transfer holds that loan through the whole ladder inside one
// `storage_*10` call (`xhci/mod.rs` `bot_transfer_body`): a 3x `hw_wait_budget` first attempt (6 s on x86) plus a
// recovery and its rungs, every other task on the holder's CPU stopped with it — on the service core, 3x the pump
// watch's `WEDGE_MS`. The xHCI layer's hook (`XhciController::loan_shed_hook`, its file tail) calls `xhci_loan_shed` from
// the holder's own call chain at each stage-wait wake (gate: loan age >= `LOAN_MASKED_WAIT_MS`) and at the ladder entry
// (gate 0: the recovery's EP0/command waits are unhooked). The shed is Z69's demotion shape on the static path: the
// publication withdrawn (CAS tag -> 0), the count released (`preemptoff::enable`), then the mark (`LOAN_SHED`) that tells
// `Drop` not to do either again. The holder KEEPS the claim and is preemptible as before Z62; a masked claimant answers
// `Busy` at once (unpublished holder), WEDGE-8 F3 restated. All state is lock-free and written only by the holder while
// it holds the loan (controller 0's single loan: at most one block loan is live). aarch64: no count, nothing to shed.

/// XHCILOANFAILHOLD: the live block loan was shed (set by `xhci_loan_shed`, cleared at the take, read-and-cleared by
/// `Drop` before the put).
#[cfg(target_arch = "x86_64")]
static LOAN_SHED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// XHCILOANFAILHOLD: `now_cycles` at the live block loan's take (its age).
#[cfg(target_arch = "x86_64")]
static LOAN_TAKEN_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// XHCILOANFAILHOLD: block loans shed this boot.
#[cfg(target_arch = "x86_64")]
static LOAN_SHED_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// XHCILOANFAILHOLD: at the take (loan held, preempt-off): stamp the loan's age, clear its shed mark.
#[cfg(target_arch = "x86_64")]
#[inline]
fn loan_shed_arm() {
    LOAN_TAKEN_AT.store(crate::arch::now_cycles(), core::sync::atomic::Ordering::Relaxed);
    LOAN_SHED.store(false, core::sync::atomic::Ordering::Relaxed);
}

/// XHCILOANFAILHOLD: `Drop`'s read of the shed mark, taken (cleared) while the loan is still held.
#[cfg(target_arch = "x86_64")]
#[inline]
fn loan_shed_take() -> bool {
    LOAN_SHED.swap(false, core::sync::atomic::Ordering::AcqRel)
}

/// XHCILOANFAILHOLD: shed THIS CPU's live block loan if it has been held at least `gate_ms` — the publication withdrawn,
/// the preempt-off count released, the mark set (Z69's `demote` shape). A no-op unless the published block holder is
/// this CPU (so a non-block xHCI holder, or a loan already shed, never matches). Called only from the holder's own call
/// chain (`xhci::XhciController::loan_shed_hook`) while it is still preempt-off, so the check and the release cannot
/// interleave with a switch. The first shed of the boot is said once.
#[cfg(target_arch = "x86_64")]
pub fn xhci_loan_shed(rung: &'static str, gate_ms: u64) {
    use core::sync::atomic::Ordering;
    let me = crate::arch::percpu::this_cpu().cpu_index.saturating_add(1);
    if BLOCK_LOAN_CPU.load(Ordering::Acquire) != me || LOAN_SHED.load(Ordering::Relaxed) {
        return;
    }
    let waited_ms = crate::arch::now_cycles().wrapping_sub(LOAN_TAKEN_AT.load(Ordering::Relaxed)) / wait::cycles_ms(1).max(1);
    if waited_ms < gate_ms {
        return;
    }
    if BLOCK_LOAN_CPU.compare_exchange(me, 0, Ordering::AcqRel, Ordering::Relaxed).is_err() {
        return;
    }
    crate::arch::sched::preemptoff::enable();
    LOAN_SHED.store(true, Ordering::Release);
    if LOAN_SHED_N.fetch_add(1, Ordering::Relaxed) == 0 {
        serial_println!(
            "[xhci] loan shed cpu={} rung={} waited_ms={} (XHCILOANFAILHOLD Z66, first shed, once)",
            me - 1,
            rung,
            waited_ms
        );
    }
}

/// XHCILOANFAILHOLD: block loans shed this boot (`loan_shed=` on the PREEMPTAUDIT line).
#[cfg(target_arch = "x86_64")]
pub fn xhci_loan_shed_count() -> u32 {
    LOAN_SHED_N.load(core::sync::atomic::Ordering::Relaxed)
}

// ══════════ XHCILOANSHEDKAT (zenbook-ledger Z71) — `tests preemptaudit` drives the block loan's shed ══════════
//
// TAIL-APPENDED after Z66 (no line above moves). `loan_shed=` reads 0 on every healthy boot, so Z66's shed had never run
// under a test. The honest unit: the KAT takes the block loan through the ONE claim path (`claim_xhci_for_io`) and calls
// `xhci_loan_shed` on it directly (`XhciController::loan_shed_hook` is a private rung/gate picker over exactly this call
// and needs a live mid-BOT controller: owed XHCILOAN-SHEDHOOK-KAT). Legs: the gate (a 60 s gate changes nothing), the
// shed (count back to its pre-take value — 0, read by `preemptoff::witness` false -> true -> false — publication 0, the
// claim kept, `LOAN_SHED_N` +1), a MASKED contender on a sibling CPU answered Busy why=holder-preemptible inside
// `LOAN_MASKED_WAIT_MS` while the holder waits preemptible, and the shed-aware Drop under the KAT's own extra `disable`
// (a double release would read the count free). The KAT gives back what it spent (B1034 `full_kat`'s shape): its
// `LOAN_SHED_N`, the contender's `LOAN_MASKED_BUSY`, `LOAN_LAST_WHY`, and neither once-line (`[xhci] loan shed`,
// `[xhci] loan masked claim`) is said by it. The shed statics are written only by the ONE block-loan holder — the KAT
// for the whole window. A real masked refusal inside the contender's ~ms window is still counted, but its once-line is
// silenced if it was the boot's first. XHCILOANSHEDKATCONTROL (zenbook-ledger Z78) builds the go-red control: the same
// contender, BEFORE the shed (the loan still published preempt-off), must spin Z62's bounded wait out and answer Busy
// why=masked-timeout in [8, 20] ms (`loan_shed_control`); both legs share ONE bookkeeping helper (`loan_shed_contend`).
// XHCILOANSHEDKATCONTROLUS (zenbook-ledger Z80): the control's µs reaches the line as `loan_shed_us=` (`LOAN_SHED_CONTROL_US`).

/// XHCILOANSHEDKAT contender handshake: 0 waiting, then the masked claim's outcome — 1 Ok, 2 Busy, 3 NotReady, 4 error.
#[cfg(target_arch = "x86_64")]
static LOAN_SHEDKAT_STATE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// XHCILOANSHEDKAT: the contender's `LOAN_LAST_WHY` read right after its claim.
#[cfg(target_arch = "x86_64")]
static LOAN_SHEDKAT_WHY: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// XHCILOANSHEDKAT: µs the contender's masked claim took.
#[cfg(target_arch = "x86_64")]
static LOAN_SHEDKAT_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// XHCILOANSHEDKATCONTROLUS (zenbook-ledger Z80): µs the pre-shed control's masked contender took (`loan_shed_us=` on the
/// PREEMPTAUDIT line); written only by the KAT task — 0 at the KAT's first statement, the control's reading once its
/// contender came — so 0 means the control did not run.
#[cfg(target_arch = "x86_64")]
static LOAN_SHED_CONTROL_US: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// XHCILOANSHEDKAT contender (a task on a sibling CPU): one MASKED `claim_xhci_for_io` against the KAT's shed loan,
/// timed; the outcome, the refusal's why and the µs published for the KAT.
#[cfg(target_arch = "x86_64")]
fn loan_shed_contender(_: usize) {
    use core::sync::atomic::Ordering;
    let (code, why, cycles) = crate::arch::without_interrupts(|| {
        let t0 = crate::arch::now_cycles();
        let r = claim_xhci_for_io();
        let c = crate::arch::now_cycles().wrapping_sub(t0);
        let code = match &r {
            Ok(_) => 1u8,
            Err(BlockError::Busy) => 2,
            Err(BlockError::NotReady) => 3,
            Err(_) => 4,
        };
        let why = LOAN_LAST_WHY.load(Ordering::Relaxed);
        drop(r);
        (code, why, c)
    });
    LOAN_SHEDKAT_WHY.store(why, Ordering::Relaxed);
    LOAN_SHEDKAT_US.store(cycles.saturating_mul(1000) / wait::cycles_ms(1).max(1), Ordering::Relaxed);
    LOAN_SHEDKAT_STATE.store(code, Ordering::Release);
}

/// XHCILOANSHEDKATCONTROL: ONE contender run for both KAT legs — spawn [`loan_shed_contender`] on `cpu`, wait for its
/// outcome (<= 1000 ms), and give back what it spent: its `LOAN_MASKED_BUSY` (Busy) or `LOAN_MASKED_OK` (Ok),
/// `LOAN_LAST_WHY`, and the once-line latch (its refusal is never the boot's said line). Returns
/// `(came, code, why, us)` as the contender published them.
#[cfg(target_arch = "x86_64")]
fn loan_shed_contend(cpu: usize) -> (bool, u8, u8, u64) {
    use core::sync::atomic::Ordering;
    let prior_said = LOAN_REFUSAL_SAID.swap(true, Ordering::AcqRel);
    let prior_why = LOAN_LAST_WHY.load(Ordering::Relaxed);
    LOAN_SHEDKAT_STATE.store(0, Ordering::Release);
    crate::arch::sched::spawn("loanshed-kat", loan_shed_contender, 0, cpu, crate::arch::sched::PRIO_HIGH);
    let came = wait::bounded(1000, || LOAN_SHEDKAT_STATE.load(Ordering::Acquire) != 0);
    let code = LOAN_SHEDKAT_STATE.load(Ordering::Acquire);
    if code == 2 {
        LOAN_MASKED_BUSY.fetch_sub(1, Ordering::Relaxed);
    } else if code == 1 {
        LOAN_MASKED_OK.fetch_sub(1, Ordering::Relaxed);
    }
    LOAN_LAST_WHY.store(prior_why, Ordering::Relaxed);
    if !prior_said {
        LOAN_REFUSAL_SAID.store(false, Ordering::Release);
    }
    let why = LOAN_SHEDKAT_WHY.load(Ordering::Relaxed);
    let us = LOAN_SHEDKAT_US.load(Ordering::Relaxed);
    (came, code, why, us)
}

/// XHCILOANSHEDKATCONTROL: the pre-shed go-red leg. Called while the KAT holds the block loan published preempt-off
/// on this CPU (no migration: the CPU read here is stable); a MASKED contender on a sibling CPU must spin Z62's
/// bounded wait out (`masked_loan_wait`, [`LOAN_MASKED_WAIT_MS`]) and answer Busy why=masked-timeout inside
/// [8, 20] ms. The first failed check: `control-same-cpu` / `control-late` / `control-got` / `control-why` /
/// `control-early` (the wait path dead) / `control-slow`. XHCILOANSHEDKATCONTROLUS (Z80): a contender that came leaves
/// its µs in `LOAN_SHED_CONTROL_US` on every outcome.
#[cfg(target_arch = "x86_64")]
fn loan_shed_control() -> Result<(), &'static str> {
    let here = crate::arch::percpu::this_cpu().cpu_index as usize;
    let cpu = crate::arch::sched::sibling_online_cpu(here);
    if cpu == here {
        return Err("control-same-cpu");
    }
    let (came, code, why, us) = loan_shed_contend(cpu);
    if !came {
        return Err("control-late");
    }
    LOAN_SHED_CONTROL_US.store(us.min(u32::MAX as u64) as u32, core::sync::atomic::Ordering::Relaxed);
    if code == 1 {
        return Err("control-got");
    }
    if code != 2 || why != 3 {
        return Err("control-why");
    }
    if us < 8_000 {
        return Err("control-early");
    }
    if us > 20_000 {
        return Err("control-slow");
    }
    Ok(())
}

/// XHCILOANSHEDKAT: `tests preemptaudit`'s shed leg (`loan_shed_kat=`): `ok`, `skip(masked|no-xhci|one-cpu)`, or the
/// first failed check — `claim-busy` / `claim-notready` / `held-before` / `not-held` / `gate-ignored` / `not-released` /
/// `still-published` / `claim-lost` / `not-counted` / `contender-got` / `contender-waited` / `contender-late` /
/// `double-release` / `mark-kept`; the pre-shed control (XHCILOANSHEDKATCONTROL Z78) first: `control-same-cpu` /
/// `control-late` / `control-got` / `control-why` / `control-early` / `control-slow`. XHCILOANSHEDKATCONTROLUS (Z80): the
/// control's µs reset to 0 first, so `loan_shed_us=` reads 0 on every return before the control's contender came.
#[cfg(target_arch = "x86_64")]
pub fn xhci_loan_shed_kat() -> &'static str {
    use core::sync::atomic::Ordering;
    use crate::arch::sched::preemptoff;
    LOAN_SHED_CONTROL_US.store(0, Ordering::Relaxed);
    if crate::arch::irqs_masked() {
        return "skip(masked)";
    }
    if matches!(xhci::claim(), Err(XhciClaimError::NotReady)) {
        return "skip(no-xhci)";
    }
    let me = crate::arch::percpu::this_cpu().cpu_index as usize;
    let cpu = crate::arch::sched::sibling_online_cpu(me);
    if cpu == me {
        return "skip(one-cpu)";
    }
    if preemptoff::witness().0 {
        return "held-before";
    }
    let loan = match claim_xhci_for_io() {
        Ok(l) => l,
        Err(BlockError::NotReady) => return "claim-notready",
        Err(_) => return "claim-busy",
    };
    // XHCILOANSHEDKATCONTROL (Z78): the go-red control while the loan is still published; a failure returns its word and
    // the loan goes back through the normal (unshed) Drop.
    if let Err(w) = loan_shed_control() {
        return w;
    }
    // The take holds the count (no switch, no migration until the shed); the mask keeps the shed and its reads on
    // one CPU. The pre-bump keeps the boot's "first shed, once" line for a real shed; every count is given back.
    let shed: Result<(), &'static str> = crate::arch::without_interrupts(|| {
        let tag = crate::arch::percpu::this_cpu().cpu_index.saturating_add(1);
        if !preemptoff::witness().0 || BLOCK_LOAN_CPU.load(Ordering::Acquire) != tag {
            return Err("not-held");
        }
        let n0 = LOAN_SHED_N.fetch_add(1, Ordering::Relaxed);
        let legs = || -> Result<(), &'static str> {
            xhci_loan_shed("kat", 60_000);
            if !preemptoff::witness().0
                || BLOCK_LOAN_CPU.load(Ordering::Acquire) != tag
                || LOAN_SHED.load(Ordering::Acquire)
                || LOAN_SHED_N.load(Ordering::Relaxed) != n0.wrapping_add(1)
            {
                return Err("gate-ignored");
            }
            xhci_loan_shed("kat", 0);
            if preemptoff::witness().0 {
                return Err("not-released");
            }
            if BLOCK_LOAN_CPU.load(Ordering::Acquire) != 0 {
                return Err("still-published");
            }
            if !matches!(xhci::claim(), Err(XhciClaimError::Busy)) {
                return Err("claim-lost");
            }
            if LOAN_SHED_N.load(Ordering::Relaxed) != n0.wrapping_add(2) || !LOAN_SHED.load(Ordering::Acquire) {
                return Err("not-counted");
            }
            Ok(())
        };
        let r = legs();
        let n1 = LOAN_SHED_N.load(Ordering::Relaxed);
        LOAN_SHED_N.fetch_sub(n1.wrapping_sub(n0), Ordering::Relaxed);
        r
    });
    if let Err(w) = shed {
        return w;
    }
    // The masked contender on the sibling CPU; this holder waits PREEMPTIBLE (the shed's point).
    let (came, code, why, us) = loan_shed_contend(cpu);
    if !came {
        return "contender-late";
    }
    if code == 1 {
        return "contender-got";
    }
    if !(code == 2 && why == 1 && us < LOAN_MASKED_WAIT_MS.saturating_mul(1000)) {
        return "contender-waited";
    }
    // The shed-aware Drop under this KAT's own hold: the put, and no second release (a double one reads the count free).
    preemptoff::disable();
    drop(loan);
    let (held, _) = preemptoff::witness();
    let mark = LOAN_SHED.load(Ordering::Acquire);
    preemptoff::enable();
    if !held {
        return "double-release";
    }
    if mark {
        return "mark-kept";
    }
    "ok"
}

/// XHCILOANSHEDKATCONTROLUS (zenbook-ledger Z80): the pre-shed control's measured µs (`loan_shed_us=` after
/// `loan_shed_kat=` on the PREEMPTAUDIT line): ~10000 on a live Z62 wait, 0 when the control did not run.
#[cfg(target_arch = "x86_64")]
pub fn xhci_loan_shed_control_us() -> u32 {
    LOAN_SHED_CONTROL_US.load(core::sync::atomic::Ordering::Relaxed)
}

// ══════════ XHCILOANPOLLCAP (zenbook-ledger Z74) — the xhci-poll task is the block loan's ninth holder ══════════
//
// TAIL-APPENDED (no line above moves). `xhci::poll_events` now dispatches at most `POLL_DRAIN_CAP` events a pass (6 on
// x86: 48 µs of MMIO), so the poller's hold is short and never sleeps; it takes controller 0 through Z62's ONE take
// (`take_block_loan`: this CPU's preempt-off count, the holder published in `BLOCK_LOAN_CPU`, the shed mark armed) and
// puts it back through `BlockLoan`'s Drop. A masked claimant on another CPU then waits a running holder out
// (`masked_loan_wait`) instead of `Busy` why=holder-preemptible. No wait here: the poller skips a Busy pass.

/// XHCILOANPOLLCAP: the xhci-poll task's take of controller 0 as a block loan — `take_block_loan`, nothing else.
#[cfg(target_arch = "x86_64")]
pub fn xhci_poll_loan() -> Result<BlockLoan, XhciClaimError> {
    take_block_loan()
}

// ══════════ XHCILOANPTPYIELDPOLLTAKE (zenbook-ledger Z82) — a PTP yield window records WHO closed it: a block claimant or the xhci-poll take ══════════
//
// TAIL-APPENDED (two LINE-NEUTRAL folds: `take_block_loan`'s x86 Ok arm and `yield_now`'s PUT line). Z73's window ends at
// the first published take (`BLOCK_LOAN_CPU`), and `take_block_loan` is its ONLY publisher; the xhci-poll task (Z74's
// `xhci_poll_loan`) takes through it every tick, so `taken=` on the `[xhci] loan yield` line counted the poller's 48 µs
// hold as a served claimant. The holder marks the window OPEN before its put; the FIRST take after the put swaps OPEN for
// its class BEFORE it publishes (one CAS: every later take, the re-take included, fails it), so `yield_now` seeing the
// publish sees the closer. The poll take is told apart by its task, `xhci-poll` (the identity Z76's skip line names).
// Owed: XHCILOAN-PTPYIELD-POLLSKIP, XHCILOAN-PTPYIELD-KATCLOSER, XHCILOAN-PTPYIELD-PTPTAKE (BUILT by XHCILOANPTPYIELDPTPTAKE,
// zenbook-ledger Z87: a third class, PTP — the take of a task `claim_only` marked, the ONE PTP/MTP entry — in the SAME one CAS).

/// XHCILOANPTPYIELDPOLLTAKE: the closer word's states — a window open, closed by a block take, closed by the xhci-poll take.
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
const LOAN_CLOSER_OPEN: u8 = 1;
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
const LOAN_CLOSER_BLOCK: u8 = 2;
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
const LOAN_CLOSER_POLL: u8 = 3;
/// XHCILOANPTPYIELDPTPTAKE (Z87): closed by a second PTP/MTP claim's take (`claim_only`; refused by `with_controller` after it).
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
const LOAN_CLOSER_PTP: u8 = 4;

/// XHCILOANPTPYIELDPOLLTAKE: who closed the current / last PTP yield window (written OPEN by the PTP holder before its put,
/// swapped by the first take after it; read by `ptp` after the re-take). Lock-free.
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
static LOAN_YIELD_CLOSER: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// XHCILOANPTPYIELDPTPTAKE (Z87): the task id of a `claim_only` caller while it is inside its claim (0 = none). Written only
/// by that task (`LoanPtpMark`: CAS 0 -> tid, so the holder's own re-take does not overwrite a waiting second claimant; CAS
/// tid -> 0 on drop); read by `loan_yield_close` while a window is open.
#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
static LOAN_PTP_TAKER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// XHCILOANPTPYIELDPTPTAKE (Z87): marks the current task as a PTP/MTP claimant for the span of `claim_only`'s take (the
/// guard's drop clears it, the `?` return included). A ZST: without `uvc` or off x86 it compiles to nothing.
struct LoanPtpMark;

impl LoanPtpMark {
    #[inline(always)]
    fn set() -> LoanPtpMark {
        #[cfg(all(feature = "uvc", target_arch = "x86_64"))]
        {
            if let Some(t) = crate::arch::sched::current_id() {
                let _ = LOAN_PTP_TAKER.compare_exchange(0, t, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Relaxed);
            }
        }
        LoanPtpMark
    }
}

#[cfg(all(feature = "uvc", target_arch = "x86_64"))]
impl Drop for LoanPtpMark {
    fn drop(&mut self) {
        if let Some(t) = crate::arch::sched::current_id() {
            let _ = LOAN_PTP_TAKER.compare_exchange(t, 0, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// XHCILOANPTPYIELDPOLLTAKE: called by `take_block_loan` after a successful take and BEFORE its publish: an open window
/// gets its closer (one load and out for every take outside a window).
#[cfg(target_arch = "x86_64")]
#[inline]
fn loan_yield_close() {
    #[cfg(feature = "uvc")]
    {
        use core::sync::atomic::Ordering;
        if LOAN_YIELD_CLOSER.load(Ordering::Acquire) != LOAN_CLOSER_OPEN {
            return;
        }
        let ptp = crate::arch::sched::current_id().is_some_and(|t| t == LOAN_PTP_TAKER.load(Ordering::Acquire)); // XHCILOANPTPYIELDPTPTAKE (Z87): a task `claim_only` marked
        let who = if crate::arch::sched::current_name() == Some("xhci-poll") { LOAN_CLOSER_POLL } else if ptp { LOAN_CLOSER_PTP } else { LOAN_CLOSER_BLOCK };
        let _ = LOAN_YIELD_CLOSER.compare_exchange(LOAN_CLOSER_OPEN, who, Ordering::AcqRel, Ordering::Relaxed);
    }
}

/// XHCILOANPTPYIELDPOLLTAKE: whether the last PTP yield window was closed by the xhci-poll task's take (read by
/// `ptp_txn_inner` right after a taken yield; one PTP hold at a time). aarch64: never (no publication there).
#[cfg(feature = "uvc")]
pub fn xhci_loan_yield_closed_by_poll() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        LOAN_YIELD_CLOSER.load(core::sync::atomic::Ordering::Acquire) == LOAN_CLOSER_POLL
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// XHCILOANPTPYIELDPTPTAKE (Z87): whether the last PTP yield window was closed by a second PTP/MTP claim's take (read by
/// `ptp_txn_inner` right after a taken yield, beside `xhci_loan_yield_closed_by_poll`). aarch64: never.
#[cfg(feature = "uvc")]
pub fn xhci_loan_yield_closed_by_ptp() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        LOAN_YIELD_CLOSER.load(core::sync::atomic::Ordering::Acquire) == LOAN_CLOSER_PTP
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

// ══════════ XHCILOANPTPYIELDPOLLSKIP (zenbook-ledger Z86) — the xhci-poll take is refused inside a PTP yield window ══════════
//
// TAIL-APPENDED (one LINE-NEUTRAL fold: `take_block_loan`'s x86 Ok arm; one in `xhci::poll_loan`). The xhci-poll task takes
// controller 0 every 1 ms tick through `take_block_loan` (Z74), so it closed a PTP yield window (`ptp::TXN_YIELDED`, Z73;
// <= 3 ms) before the block claimant the window exists for (Z82's `taken_poll=`). Z76's claim-first-then-refuse rule, moved
// to where it can still refuse: AFTER `claim()` succeeds (a take after the PTP put sees the flag the holder set before that
// put) and BEFORE `loan_yield_close` and the publish, so the closer stays OPEN and `yield_now` sees no `BLOCK_LOAN_CPU`: the
// window serves a block claimant or nobody. Only the task the closer names `xhci-poll` is refused; the loan goes straight
// back (`XhciLoan`'s Drop), the preempt-off count is released, `Busy`; counted (`poll_skips=`), the first said once, not
// stamped claim-busy. The cost: <= 3 ms of input latency per window (the PTP holder drains the ring itself during its hold).
// Owed: XHCILOAN-PTPYIELD-POLLSKIP-KATRULE (Z84's `skip(poll-closed)` arm deleted: ok-or-FAIL).

/// XHCILOANPTPYIELDPOLLSKIP: the xhci-poll take refused inside a PTP yield window (written and read by that one task:
/// `xhci::poll_loan` takes it to leave the refusal unstamped).
#[cfg(all(target_arch = "x86_64", feature = "uvc"))]
static LOAN_POLL_SKIPPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// XHCILOANPTPYIELDPOLLSKIP: called by `take_block_loan` after a successful `claim()` and BEFORE the closer and the publish:
/// `true` = this is the xhci-poll take and a PTP yield window is open (the caller puts the loan back and answers `Busy`).
/// One load and out for every take outside a window; `false`, inlined away, without `uvc` (no window exists).
#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn loan_poll_yield_skip() -> bool {
    #[cfg(feature = "uvc")]
    {
        use core::sync::atomic::Ordering;
        if xhci::ptp::TXN_YIELDED.load(Ordering::Acquire) && crate::arch::sched::current_name() == Some("xhci-poll") {
            LOAN_POLL_SKIPPED.store(true, Ordering::Relaxed);
            if POLL_YIELD_SKIPS.fetch_add(1, Ordering::Relaxed) == 0 {
                serial_println!("[xhci] poll skip why=ptp-yield who=xhci-poll (XHCILOANPTPYIELDPOLLSKIP Z86, first skip, once)");
            }
            return true;
        }
    }
    false
}

/// XHCILOANPTPYIELDPOLLSKIP: whether the xhci-poll task's last refused take was the window skip (cleared by the read).
#[cfg(all(target_arch = "x86_64", feature = "uvc"))]
pub fn xhci_poll_skip_take() -> bool {
    LOAN_POLL_SKIPPED.swap(false, core::sync::atomic::Ordering::Relaxed)
}

/// XHCILOANPTPYIELDPOLLSKIP: no `uvc`, no window, never a skip (the knob-off image keeps `poll_loan`'s old code).
#[cfg(all(target_arch = "x86_64", not(feature = "uvc")))]
#[inline(always)]
pub fn xhci_poll_skip_take() -> bool {
    false
}

/// XHCILOANPTPYIELDPOLLSKIP: xhci-poll takes refused inside a PTP yield window this boot (`poll_skips=`). A twin of Z76's
/// `PUMP_YIELD_SKIPS`, which stays the pump's alone (Z81's `pump_kat=` reads it as the pump gate's proof).
#[cfg(feature = "uvc")]
static POLL_YIELD_SKIPS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// XHCILOANPTPYIELDPOLLSKIP: ` poll_skips=<n>` on the `[xhci] loan yield` line (ptp.rs), NOTHING while 0 (Z73's line
/// byte-identical on a boot the gate never fired).
#[cfg(feature = "uvc")]
pub struct PollSkips;

#[cfg(feature = "uvc")]
impl core::fmt::Display for PollSkips {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let n = POLL_YIELD_SKIPS.load(core::sync::atomic::Ordering::Relaxed);
        if n == 0 { Ok(()) } else { write!(f, " poll_skips={}", n) }
    }
}
