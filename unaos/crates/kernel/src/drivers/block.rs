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
/// holder) up to `hw_wait_budget()` wall-clock — long enough for any healthy SERVICE PASS, but NOT for
/// every healthy BOT transaction: the driver itself budgets a first attempt at 3× this wait (`BOT_BUDGET_SCALE_FIRST` in `xhci/mod.rs`), so a slow-but-healthy transfer can hold the loan past this whole wait, and `Busy` is then a NORMAL, RETRYABLE outcome — not a wedge verdict (`usb_xhci.md` §32.3).
/// Still short enough that a wedged 25 s failing-transfer hold surfaces as `Busy` instead of hanging the caller forever.
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
// `/fat` would stop meaning the volume the machine booted from and `bg /fat/VUG.ELF` would resolve
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

/// PSRC: the OTHER populated program-source handle — the one [`program_source`] did NOT choose.
///
/// `None` when there is no second populated handle (the overwhelmingly common case: one disk).
///
/// This exists for exactly one caller and one class of read. `wifi::firmware` stages user-supplied
/// b43 blobs, and those blobs live wherever the OPERATOR put them — which, in the workflow this arc
/// was written during, is a USB stick carried to a machine whose boot volume is the internal card.
/// Making the preference above authoritative for that read would mean "to try a firmware blob,
/// re-flash your boot card", which is a worse OS. The firmware search is READ-ONLY, its whole job is
/// "find a user-supplied file wherever it is" (it already walks three directories per volume for the
/// same reason), and none of the hazards the preference closes — `/fat` meaning the wrong volume for
/// an exec, a write landing on a foreign medium — apply to it.
///
/// Nothing else should use this. A caller that means "the volume this system is bound to" wants
/// [`program_source`], and a caller that means a SPECIFIC disk wants [`lookup`] with a
/// [`BlockDeviceId`].
pub fn alternate_program_source() -> Option<(BlockDeviceInfo, BlockHandle)> {
    match program_source()?.1 {
        BlockHandle::Global => {
            #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
            {
                sdhc_info().map(|d| (d, BlockHandle::Sdhc))
            }
            #[cfg(not(all(target_arch = "x86_64", feature = "sdhcblk")))]
            {
                None
            }
        }
        // `program_source` never returns `Usb` (see its precedence note); mapped for totality.
        BlockHandle::Usb => None,
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        BlockHandle::Sdhc => info().map(|d| (d, BlockHandle::Global)),
        // TEGRA-SDBLK: `program_source` never returns `TegraSd` either — the Orin's program volume is
        // the boot medium in the global slot, and the microSD is a SEPARATE disk that this arc gives a
        // read path, not a program-loading precedence (see the census note on `source_census`). Mapped
        // for totality, exactly as `Usb` is, and it means the same thing: this arm cannot be reached.
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        BlockHandle::TegraSd => None,
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
    TegraSd,
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
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        BlockHandle::TegraSd => tegra_sd_info(),
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
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn publish_usb_geometry(dev: BlockDeviceInfo) {
    *USB_BLOCK_DEVICE.lock() = Some(dev);
    // Claim the global only while USB is still the active backend; once the SD has registered, leave
    // BLOCK_DEVICE (the SD's geometry) untouched — the stick stays reachable via USB_BLOCK_DEVICE.
    if BACKEND.load(Ordering::Acquire) != BACKEND_SD {
        *BLOCK_DEVICE.lock() = Some(dev);
    }
    USB_PUBLISH_GEN.fetch_add(1, core::sync::atomic::Ordering::AcqRel); // PA35 race: every publish is a new generation
    set_usb_ready();
}

/// PIUSB-28: x86 / non-SD-capable targets — the USB stick is the default (boot) block backend, so it
/// always claims the global `BLOCK_DEVICE` alongside the dedicated USB handle. Preserves the historical
/// behavior on any target that never compiles the SD backend / BACKEND selector.
///
/// FRGUARD (GR21): a claim of the global slot INVALIDATES the boot-medium verdict — the next reader
/// re-derives it against whatever now occupies the slot. See [`BOOT_MEDIUM_VERDICT`].
#[cfg(not(all(target_arch = "aarch64", feature = "baremetal")))]
pub fn publish_usb_geometry(dev: BlockDeviceInfo) {
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    BOOT_MEDIUM_VERDICT.store(BM_UNKNOWN, core::sync::atomic::Ordering::Release);
    *BLOCK_DEVICE.lock() = Some(dev);
    *USB_BLOCK_DEVICE.lock() = Some(dev);
    USB_PUBLISH_GEN.fetch_add(1, core::sync::atomic::Ordering::AcqRel); // PA35 race: every publish is a new generation
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
        serial_println!(
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
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub fn default_writable() -> bool {
    BACKEND.load(Ordering::Acquire) == BACKEND_SD
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
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
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
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
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
/// this kernel booted from, and the FRGUARD verdict above can be read together with it. Nothing
/// keys behaviour off this getter: SDHC-4c's bound is the LBA extent, not an identity test.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
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
// ### The follow-up step: `BlockHandle::TegraSd` (LANDED — see the variant above)
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
// `alternate_program_source` maps `TegraSd` to `None`. The Orin boots from a USB stick; making the
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
/// [`BLOCK_DEVICE`] and [`USB_BLOCK_DEVICE`]: nothing here ever claims the global, so the boot volume
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
            ":: TEGRA-SD: REFUSED to publish the microSD block backend — num_blocks=0 (nothing to address) ::"
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
        serial_println!(
            ":: TEGRA-SD: block backend published — {} sectors (read-only) ::",
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
pub fn write_block_tegra_sd(_lba: u64, _buf: &[u8]) -> Result<(), BlockError> {
    if !TEGRA_SD_WRITE_REFUSED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        serial_println!(
            ":: TEGRA-SD: WRITE refused at the block layer — the Orin card's only writer is the armed \
             sdmmc_arm ladder in sdmmc_tegra.rs (first, once) ::"
        );
    }
    // `NotReady` is the refusal code SDHC-4b's disarmed write twin returns for the same situation —
    // "this build has no writer for this device" — and `BlockError` gains no variant here, so no match
    // anywhere in the tree changes shape.
    Err(BlockError::NotReady)
}

/// TEGRA-SDBLK: the counted twin of [`write_block_tegra_sd`], refusing for the same reason. Present so
/// that a future `BlockHandle::TegraSd` dispatch has a total set of four entry points to name.
#[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
pub fn write_blocks_tegra_sd(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    write_block_tegra_sd(lba, buf)
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
        BlockHandle::TegraSd => 8,
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
        BlockHandle::TegraSd => "tegra-sd",
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
            // TEGRA-SDBLK: the point of the variant. A `PartitionRange` built on the Orin card now
            // reaches the sdmmc driver's polled CMD17 through the same bounded, partition-relative
            // gate every other handle goes through — no second addressing path, no absolute LBA
            // computed by a caller.
            #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
            BlockHandle::TegraSd => read_block_tegra_sd(abs, buf),
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
            BlockHandle::TegraSd => write_block_tegra_sd(abs, buf),
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
            BlockHandle::TegraSd => read_blocks_tegra_sd(abs, buf),
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
            BlockHandle::TegraSd => write_blocks_tegra_sd(abs, buf),
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
            BlockHandle::TegraSd => tegra_sd_info(),
        }
        .ok_or(BlockError::NotReady)?;
        let bs = dev.block_size as usize;
        if bs == 0 || len == 0 || len % bs != 0 {
            return Err(BlockError::Io);
        }
        Ok((len / bs) as u64)
    }
}
