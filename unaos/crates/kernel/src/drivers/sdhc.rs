// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! SDHC — SD Host Controller (SDHCI) on x86: the 2012 rMBP's BUILT-IN PCIe card reader.
//!
//! ## Why this driver exists
//!
//! Every storage failure this project has fought lives on the **USB BOT** path: a card in an
//! external USB reader, behind xHCI, behind Bulk-Only Transport. The rMBP's own reader is not on
//! that path at all — it is a PCIe SD Host Controller (class 0x08 / subclass 0x05). Driving it
//! reaches a card WITHOUT xHCI, WITHOUT BOT, and without any of the ring/phase machinery those
//! rounds were spent on. That is the whole point of this file.
//!
//! ## Milestones
//!
//! * **Milestone 1 (SDHC-1, shipped)** — discovery only, and STRICTLY read-only: a storage-class
//!   PCI census (`PciScanner::storage_inventory`) plus a version/capability/present-state read out
//!   of BAR0. It programmed nothing, so the serial log was evidence about *the machine*.
//! * **Milestone 2 (SDHC-2, shipped)** — bring-up. This driver now WRITES to the controller:
//!   it claims the function's memory decode, resets the host controller, programs bus power and
//!   the SD clock, runs the card identification ladder, and reads one block. Milestone 1's witness
//!   lines are all still emitted, in the same order, before anything is programmed — so a boot log
//!   from this build is still directly comparable with a milestone-1 log up to the `claim` line.
//! * **Milestone 3 (SDHC-3, this file)** — multi-block and ADMA2, layered so a failure can be
//!   attributed. 3a adds CMD18 READ_MULTIPLE_BLOCK still on PIO, so new COMMAND semantics are
//!   proved against milestone 2's single-block path with no new risk surface at all; only then do
//!   3b/3c grant Bus Master and put a DMA engine underneath the same command. Milestone 2's
//!   witness lines are unchanged and still emitted first.
//!
//! ## Witness discipline (the rule this project pays for six times over)
//!
//! Every register read below is accompanied by a statement of what the hardware guarantees about
//! that field **at the moment it is read**, and the raw word is printed before anything is decoded
//! from it. Concretely, the two traps this controller sets:
//!
//! * **Card Inserted** (Present State bit 16) is only defined once **Card State Stable** (bit 17)
//!   reads 1. Sampled before debounce completes it is meaningless — and a freshly reset controller
//!   has *just* restarted that debounce, so the naive "reset, then look" ordering reads the field
//!   in exactly its undefined window.
//! * **Internal Clock Stable** (Clock Control bit 1) reads 0 for a healthy controller whose
//!   internal clock has not been enabled yet, which is bit-for-bit what a dead controller reads.
//!   It only carries information *after* Internal Clock Enable has been written 1.
//!
//! ## Scope limits held on purpose
//!
//! * **Bus Master is granted late, narrowly, and only when it will be used.** Milestones 1 and 2
//!   left the bit exactly as the firmware set it, because they issued no DMA. Milestone 3 enables
//!   it in [`adma2_init`] — never in [`claim`] — and only after the controller has advertised
//!   ADMA2 and every address the engine could reach has passed a refusal check. If any of those
//!   refuse, the function keeps exactly the capabilities milestone 2 gave it. Keeping the grant out
//!   of `claim` also keeps every log line up to the claim line comparable across all three
//!   milestones.
//! * **DMA lands only in driver-owned memory.** ADMA2 writes a bounce buffer this driver allocated
//!   and never frees, which is then copied to the caller. A DMA write that straggles in after a
//!   transfer was declared timed out therefore lands somewhere harmless, forever.
//! * **32-bit ADMA2 unconditionally.** Never the 64-bit mode: the kernel heap is below 4 GiB by
//!   construction on this platform, so 64-bit addressing would buy nothing while making an
//!   unverified capability claim load-bearing.
//! * **1-bit bus, no high-speed.** Identification-clock then 25 MHz default speed, DAT0 only. A
//!   4-bit bus needs ACMD6 and a Host Control 1 change; not this milestone.
//! * **No block-layer registration.** `drivers::block`'s x86 path has no backend selector — its
//!   `publish_usb_geometry` claims the global `BLOCK_DEVICE` unconditionally — so registering a
//!   card here would silently fight the USB stick for that slot (the exact PI-FS-2 clobber the
//!   aarch64 side already documents). Geometry is published through this module's own
//!   [`card_num_blocks`] / [`read_block_512`] / [`read_blocks_512`] instead, and wiring it into the
//!   block layer waits for the arc that gives x86 a real backend selector. Milestone 3 holds this
//!   line exactly where milestone 2 drew it: a faster read path is not a reason to claim a global
//!   another driver is already using.

#![allow(dead_code)]

use spin::Mutex;

use crate::drivers::block::BlockError;
use crate::drivers::pci::PciScanner;

// ===================================================================================
// Register map — SD Host Controller Simplified Specification 3.00, Table 2-1.
//
// Offsets are named at their SPEC-DEFINED WIDTH and accessed at that width (8/16/32).  The Pi's
// EMMC2 driver (`drivers::emmc2`) uses Broadcom's 32-bit "combined" views of the same block; that
// works there because the VideoCore wrapper serves them, but a generic SDHCI part is only required
// to honour the widths the spec assigns, so this driver uses those.
// ===================================================================================

const REG_SDMA_ADDR: u64 = 0x00; // 32
const REG_BLOCK_SIZE: u64 = 0x04; // 16
const REG_BLOCK_COUNT: u64 = 0x06; // 16
const REG_ARGUMENT: u64 = 0x08; // 32
const REG_TRANSFER_MODE: u64 = 0x0C; // 16
const REG_COMMAND: u64 = 0x0E; // 16 — writing THIS issues the command
const REG_RESPONSE0: u64 = 0x10; // 32 (0x10/0x14/0x18/0x1C)
const REG_BUFFER_DATA: u64 = 0x20; // 32 — PIO FIFO
const REG_PRESENT_STATE: u64 = 0x24; // 32
const REG_HOST_CONTROL1: u64 = 0x28; // 8
const REG_POWER_CONTROL: u64 = 0x29; // 8
const REG_BLOCK_GAP: u64 = 0x2A; // 8
const REG_WAKEUP_CONTROL: u64 = 0x2B; // 8
const REG_CLOCK_CONTROL: u64 = 0x2C; // 16
const REG_TIMEOUT_CONTROL: u64 = 0x2E; // 8
const REG_SOFTWARE_RESET: u64 = 0x2F; // 8
const REG_INT_STATUS: u64 = 0x30; // 16 normal + 16 error at 0x32; read/written as one 32-bit word
const REG_INT_STATUS_EN: u64 = 0x34; // 16 normal + 16 error at 0x36
const REG_INT_SIGNAL_EN: u64 = 0x38; // 16 normal + 16 error at 0x3A
const REG_AUTO_CMD_ERR: u64 = 0x3C; // 16 — why the controller's own Auto CMD12 failed
const REG_HOST_CONTROL2: u64 = 0x3E; // 16
const REG_CAPABILITIES: u64 = 0x40; // 32
const REG_CAPABILITIES_2: u64 = 0x44; // 32
const REG_ADMA_ERROR: u64 = 0x54; // 8 — ADMA Error Status: which state the engine faulted in
const REG_ADMA_ADDR: u64 = 0x58; // 32 (low half; the high half at 0x5C exists only in 64-bit mode)
const REG_HOST_CTRL_VERSION: u64 = 0xFE; // 16

/// The SDHCI register block is 256 bytes; map one page of it Uncacheable.
const SDHCI_MMIO_LEN: usize = 0x1000;

// --- Present State (0x24) ---------------------------------------------------------
/// Command Inhibit (CMD): 1 = the CMD line is busy, a new command may not be issued.
const PS_CMD_INHIBIT: u32 = 1 << 0;
/// Command Inhibit (DAT): 1 = the DAT line is busy (a data transfer, or an R1b busy, is running).
const PS_DAT_INHIBIT: u32 = 1 << 1;
/// Card Inserted. **Only defined while `PS_CARD_STABLE` reads 1.**
const PS_CARD_INSERTED: u32 = 1 << 16;
/// Card State Stable: 1 = the debounce circuit has settled and bit 16 may be believed.
const PS_CARD_STABLE: u32 = 1 << 17;
/// Card Detect Pin Level (raw, undebounced — informational only).
const PS_CARD_DETECT_PIN: u32 = 1 << 18;
/// Write Protect Switch Pin Level: **1 = write ENABLED**, 0 = write protected (inverted sense).
const PS_WRITE_PROTECT: u32 = 1 << 19;

// --- Power Control (0x29, 8-bit) --------------------------------------------------
const PWR_ON: u8 = 1 << 0;
const PWR_V33: u8 = 0b111 << 1;
const PWR_V30: u8 = 0b110 << 1;
const PWR_V18: u8 = 0b101 << 1;

// --- Host Control 1 (0x28, 8-bit) -------------------------------------------------
/// DMA Select, bits [4:3]: 00 = SDMA, 01 = 32-bit ADMA1, 10 = 32-bit ADMA2, 11 = 64-bit ADMA2.
const HC1_DMA_SELECT_MASK: u8 = 0b11 << 3;
/// 32-bit ADMA2. Chosen **unconditionally**, never 64-bit: the only evidence that this controller
/// supports 64-bit addressing is an uncommitted bench observation, and the kernel heap this driver
/// allocates from is below 4 GiB by construction (`arch::x86_64::memory`), so the 64-bit mode would
/// buy nothing while making an unverified claim load-bearing.
const HC1_DMA_ADMA2_32: u8 = 0b10 << 3;

// --- Clock Control (0x2C, 16-bit) -------------------------------------------------
const CLK_INTERNAL_EN: u16 = 1 << 0;
/// Internal Clock Stable — **read-only, and meaningless until `CLK_INTERNAL_EN` has been set.**
const CLK_INTERNAL_STABLE: u16 = 1 << 1;
const CLK_SD_EN: u16 = 1 << 2;
/// Clock Generator Select (spec 3.00+): 0 = divided clock mode, 1 = programmable clock mode.
const CLK_GEN_PROGRAMMABLE: u16 = 1 << 5;

// --- Software Reset (0x2F, 8-bit) -------------------------------------------------
const SRST_ALL: u8 = 1 << 0;
const SRST_CMD: u8 = 1 << 1;
const SRST_DAT: u8 = 1 << 2;

// --- Interrupt Status (0x30 normal | 0x32 error, read as one 32-bit word) ---------
const INT_CMD_COMPLETE: u32 = 1 << 0;
const INT_XFER_COMPLETE: u32 = 1 << 1;
const INT_DMA: u32 = 1 << 3;
const INT_BUF_WRITE_READY: u32 = 1 << 4;
const INT_BUF_READ_READY: u32 = 1 << 5;
const INT_CARD_INSERT: u32 = 1 << 6;
const INT_CARD_REMOVE: u32 = 1 << 7;
/// Error Interrupt summary (read-only): set whenever any bit of the error half is set.
const INT_ERROR_SUMMARY: u32 = 1 << 15;
const INT_ERR_CMD_TIMEOUT: u32 = 1 << 16;
const INT_ERR_CMD_CRC: u32 = 1 << 17;
const INT_ERR_CMD_END_BIT: u32 = 1 << 18;
const INT_ERR_CMD_INDEX: u32 = 1 << 19;
const INT_ERR_DATA_TIMEOUT: u32 = 1 << 20;
const INT_ERR_DATA_CRC: u32 = 1 << 21;
const INT_ERR_DATA_END_BIT: u32 = 1 << 22;
/// Auto CMD Error (error-half bit 8): the STOP the controller issued on the driver's behalf at the
/// end of a multi-block transfer failed. Already inside [`INT_ERR_ANY`]'s `0xFFFF_0000` half — it is
/// named here only so a failure line says *which* error, instead of "other-error".
const INT_ERR_AUTO_CMD: u32 = 1 << 24;
/// ADMA Error (error-half bit 9): the descriptor engine faulted — a bad descriptor, a length
/// mismatch, or a system-address error. Also already inside [`INT_ERR_ANY`], so no existing error
/// path is weakened by naming it; `REG_ADMA_ERROR` says *which* of the three it was.
const INT_ERR_ADMA: u32 = 1 << 25;
/// Any error at all: the summary bit, or any bit of the error half.
const INT_ERR_ANY: u32 = INT_ERROR_SUMMARY | 0xFFFF_0000;

// --- Capabilities (0x40) ----------------------------------------------------------
const CAP_BASE_CLK_SHIFT: u32 = 8; // [15:8], MHz; 0 = "not reported, use another source"
const CAP_MAX_BLK_SHIFT: u32 = 16; // [17:16]: 0=512, 1=1024, 2=2048
const CAP_ADMA2: u32 = 1 << 19;
const CAP_HIGH_SPEED: u32 = 1 << 21;
const CAP_SDMA: u32 = 1 << 22;
const CAP_V33: u32 = 1 << 24;
const CAP_V30: u32 = 1 << 25;
const CAP_V18: u32 = 1 << 26;
const CAP_64BIT_V3: u32 = 1 << 28; // 64-bit System Bus Support (spec 3.00 position)
const CAP_SLOT_TYPE_SHIFT: u32 = 30; // [31:30]: 0=removable, 1=embedded, 2=shared-bus

// --- Bounded-wait budgets (milliseconds) ------------------------------------------
// Generous against the microseconds a healthy controller takes; the point is that a dead or absent
// controller fails in bounded time and logs one line, instead of freezing a serial-less laptop.
const RESET_TIMEOUT_MS: u64 = 100;
const CLK_STABLE_TIMEOUT_MS: u64 = 100;
/// Card-detect debounce. The spec gives no absolute figure (it is a controller-internal RC), but a
/// settled reading is universally sub-millisecond; 100 ms is pure margin.
const CD_STABLE_TIMEOUT_MS: u64 = 100;

/// One command's completion. A healthy card answers in microseconds.
const CMD_TIMEOUT_MS: u64 = 100;
/// The ACMD41 power-up polling loop. The SD Physical Layer spec allows a card up to 1 second to
/// finish its internal power-up, and cards routinely take tens of milliseconds.
const ACMD41_TIMEOUT_MS: u64 = 1000;
/// One block's data phase. The SD read-access-time ceiling is 100 ms; doubled for margin.
const DATA_TIMEOUT_MS: u64 = 200;

/// Identification clock: the SD Physical Layer spec requires the card be clocked at 100–400 kHz
/// until it leaves identification state.
const CLK_IDENT_HZ: u32 = 400_000;
/// Default-speed transfer clock. 25 MHz is the SD default-speed ceiling; high speed (50 MHz) needs
/// CMD6 plus Host Control 1's high-speed enable and is not attempted this milestone.
const CLK_TRANSFER_HZ: u32 = 25_000_000;

// ===================================================================================
// MMIO accessors. `base` is the identity-mapped physical BAR0; the window is mapped
// Uncacheable, so no fence is needed between accesses on x86 (UC is strongly ordered).
// ===================================================================================

#[inline]
fn r8(base: u64, off: u64) -> u8 {
    unsafe { core::ptr::read_volatile((base + off) as *const u8) }
}
#[inline]
fn r16(base: u64, off: u64) -> u16 {
    unsafe { core::ptr::read_volatile((base + off) as *const u16) }
}
#[inline]
fn r32(base: u64, off: u64) -> u32 {
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}
#[inline]
fn w8(base: u64, off: u64, val: u8) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u8, val) }
}
#[inline]
fn w16(base: u64, off: u64, val: u16) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u16, val) }
}
#[inline]
fn w32(base: u64, off: u64, val: u32) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u32, val) }
}

// ===================================================================================
// Bounded waits, in rdtsc units.
// ===================================================================================

/// `ms` milliseconds expressed in `arch::now_cycles()` (rdtsc) units. The TSC is calibrated against
/// the ACPI PM timer by `apic::calibrate`, which runs long before `pci::init` (and therefore before
/// this probe), so the calibrated leg is the one that executes on both QEMU and metal. The fallback
/// derives from `arch::HW_WAIT_BUDGET`, which is defined as the ~2-second pre-calibration guess.
#[inline]
fn cycles_ms(ms: u64) -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    if hz != 0 {
        hz.saturating_mul(ms) / 1000
    } else {
        crate::arch::HW_WAIT_BUDGET.saturating_mul(ms) / 2000
    }
}

/// Spin until `pred()` holds or `ms` milliseconds elapse. Returns true if the predicate held.
/// `wrapping_sub` so a 64-bit TSC wrap mid-wait cannot trip the deadline early.
fn wait_ms<F: Fn() -> bool>(ms: u64, pred: F) -> bool {
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

// ===================================================================================
// Decoded controller identity
// ===================================================================================

/// What the controller told us about itself, decoded once, before anything is programmed.
#[derive(Clone, Copy)]
struct HostCaps {
    /// Specification Version Number (Host Controller Version bits 7:0): 0x02 = 3.00.
    spec: u8,
    caps: u32,
    caps2: u32,
    /// Base clock for the SD clock divider, in Hz. 0 = the controller did not report one.
    base_hz: u32,
}

impl HostCaps {
    /// Spec 3.00 or later — the 10-bit divided-clock encoding and Capabilities_2 only exist here.
    #[inline]
    fn is_v3(&self) -> bool {
        self.spec >= 0x02
    }
}

/// Decode the Specification Version Number into the spec's own version string. Unknown encodings
/// are reported honestly rather than guessed.
fn spec_version_name(sver: u8) -> &'static str {
    match sver {
        0x00 => "1.00",
        0x01 => "2.00",
        0x02 => "3.00",
        0x03 => "4.00",
        0x04 => "4.10",
        0x05 => "4.20",
        _ => "unknown",
    }
}

// ===================================================================================
// Step 1 — claim and map
// ===================================================================================

/// Claim one SDHCI function: report its config-space state raw, enable memory decode if the
/// firmware left it off, and map BAR0 Uncacheable. Returns the mapped base, or `None` with a named
/// reason on serial.
///
/// **Bus Master is deliberately NOT enabled.** This milestone is PIO end to end, so the controller
/// never needs to master the bus; leaving the bit as the firmware set it is the smaller grant.
fn claim(bus: u8, slot: u8, func: u8) -> Option<u64> {
    // Config-space reads. Vendor/Device/BAR0/COMMAND are architecturally valid at any time on a
    // present function; a non-present function reads 0xFFFF as vendor, which the census already
    // filtered before handing us this bdf.
    let bar0_raw = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x10) };
    let command = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x04) };
    let vendor = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x00) };
    let device = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x02) };
    let bar0 = PciScanner::get_bar_address(bus, slot, func);

    // Milestone-1's line, unchanged, so a milestone-2 log is comparable with a milestone-1 log.
    serial_println!(
        "[sdhc] bdf {}:{}.{} {:04x}:{:04x} bar0={:#x} cmd={:#06x} mem-decode={}",
        bus, slot, func, vendor, device, bar0, command, (command & 0x0002 != 0) as u8
    );

    if (bar0_raw & 0x1) != 0 {
        serial_println!(
            "[sdhc] bdf {}:{}.{} bar0 is an I/O BAR (io={:#x}) — MMIO probe skipped",
            bus, slot, func, bar0_raw & 0xFFFF_FFFC
        );
        return None;
    }
    if bar0 == 0 {
        serial_println!(
            "[sdhc] bdf {}:{}.{} bar0 unassigned by firmware — no MMIO probe",
            bus, slot, func
        );
        return None;
    }

    // Milestone 2's first WRITE anywhere in this driver: memory decode. Milestone 1 refused here on
    // purpose (it existed to observe a quiescent controller); a driver that must reach BAR0 has to
    // turn decode on. Only bit 1 is set — Bus Master (bit 2) and everything else are left alone.
    if command & 0x0002 == 0 {
        unsafe { crate::arch::pci::write_config_16(bus, slot, func, 0x04, command | 0x0002) };
        let after = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x04) };
        serial_println!(
            "[sdhc] claim bdf {}:{}.{} mem-decode was OFF: cmd {:#06x} -> {:#06x} bus-master={} (PIO only)",
            bus, slot, func, command, after, (after >> 2) & 1
        );
        if after & 0x0002 == 0 {
            serial_println!(
                "[sdhc] claim bdf {}:{}.{} memory decode did not stick — controller not claimable",
                bus, slot, func
            );
            return None;
        }
    } else {
        serial_println!(
            "[sdhc] claim bdf {}:{}.{} mem-decode already ON (cmd={:#06x}) bus-master={} (PIO only)",
            bus, slot, func, command, (command >> 2) & 1
        );
    }

    // Identity-map the register block Uncacheable (PCD|PWT), the same seam the GPU drivers use for
    // their BARs. Creating a mapping is not a device access; the device sees nothing here.
    crate::arch::memory::map_mmio_window(bar0, SDHCI_MMIO_LEN);
    serial_println!(
        "[sdhc] map bdf {}:{}.{} bar0={:#x} len={:#x} uncacheable",
        bus, slot, func, bar0, SDHCI_MMIO_LEN
    );
    Some(bar0)
}

// ===================================================================================
// Step 1b — survey: raw first, then decoded
// ===================================================================================

/// Read the controller's self-description and print it — RAW WORDS FIRST, then the decode.
///
/// Validity of each field at this instant: Host Controller Version (0xFE), Capabilities (0x40) and
/// Capabilities_2 (0x44) are hardwired read-only descriptors, valid from the moment the function
/// decodes — they do not depend on reset, clock, power, or a card. Host Control 1 / Power Control /
/// Clock Control are read here only to record **what the firmware left behind**, which is why they
/// are printed before the reset that erases them.
fn survey(base: u64, bus: u8, slot: u8, func: u8) -> HostCaps {
    let hcver = r16(base, REG_HOST_CTRL_VERSION);
    let caps = r32(base, REG_CAPABILITIES);
    let caps2 = r32(base, REG_CAPABILITIES_2);
    let present = r32(base, REG_PRESENT_STATE);
    let hostctl1 = r8(base, REG_HOST_CONTROL1);
    let pwrctl = r8(base, REG_POWER_CONTROL);
    let clkctl = r16(base, REG_CLOCK_CONTROL);

    // RAW, before any decoding — so a decode bug can never hide the evidence it was derived from.
    serial_println!(
        "[sdhc] raw bdf {}:{}.{} hcver={:#06x} caps={:#010x} caps2={:#010x} present={:#010x} hostctl1={:#04x} pwrctl={:#04x} clkctl={:#06x}",
        bus, slot, func, hcver, caps, caps2, present, hostctl1, pwrctl, clkctl
    );

    let sver = (hcver & 0xFF) as u8;
    let vver = (hcver >> 8) as u8;
    serial_println!(
        "[sdhc] bdf {}:{}.{} hcver={:#06x} spec={} vendor-ver={:#04x}",
        bus, slot, func, hcver, spec_version_name(sver), vver
    );

    let base_mhz = (caps >> CAP_BASE_CLK_SHIFT) & 0xFF;
    let max_blk = match (caps >> CAP_MAX_BLK_SHIFT) & 0x3 {
        0 => 512u32,
        1 => 1024,
        2 => 2048,
        _ => 0,
    };
    let slot_type = match (caps >> CAP_SLOT_TYPE_SHIFT) & 0x3 {
        0 => "removable",
        1 => "embedded",
        2 => "shared-bus",
        _ => "reserved",
    };
    serial_println!(
        "[sdhc] bdf {}:{}.{} caps={:#010x} caps2={:#010x} base-clk={}MHz max-blk={} v3.3={} v3.0={} v1.8={} sdma={} adma2={} hispeed={} 64bit={} slot={}",
        bus, slot, func, caps, caps2, base_mhz, max_blk,
        (caps & CAP_V33 != 0) as u8, (caps & CAP_V30 != 0) as u8, (caps & CAP_V18 != 0) as u8,
        (caps & CAP_SDMA != 0) as u8, (caps & CAP_ADMA2 != 0) as u8,
        (caps & CAP_HIGH_SPEED != 0) as u8, (caps & CAP_64BIT_V3 != 0) as u8,
        slot_type
    );

    // Milestone-1's present-state line. NOTE the ordering trap it does NOT fall into: this reading
    // is taken BEFORE the reset, i.e. of a controller the firmware has had settled for a long time,
    // so Card State Stable is expected 1 here. The post-reset reading is taken separately, after an
    // explicit wait for the debounce to re-settle (see `wait_card_detect`).
    serial_println!(
        "[sdhc] bdf {}:{}.{} present={:#010x} card-inserted={} cd-stable={} write-protected={}",
        bus, slot, func, present,
        (present & PS_CARD_INSERTED != 0) as u8,
        (present & PS_CARD_STABLE != 0) as u8,
        (present & PS_WRITE_PROTECT == 0) as u8
    );

    HostCaps { spec: sver, caps, caps2, base_hz: base_mhz * 1_000_000 }
}

// ===================================================================================
// Step 2 — reset, power, clock
// ===================================================================================

/// Software Reset For All, then wait for the controller to self-clear the bit.
///
/// Validity: `SRST_ALL` is write-1-to-start / read-1-while-in-progress, and the spec requires the
/// controller to clear it when the reset completes — so this bit IS meaningful the instant after
/// the write, which is what makes it a legitimate completion witness (unlike the clock-stable bit
/// below). A healthy-but-idle controller reads 0 here; so does one that never started the reset,
/// which is why the raw word is printed after the wait rather than assumed.
///
/// Reset All clears the clock, the power, and every interrupt enable — so everything programmed
/// after this point must be programmed *after* this point, in this order.
fn reset_all(base: u64) -> bool {
    w8(base, REG_SOFTWARE_RESET, SRST_ALL);
    let cleared = wait_ms(RESET_TIMEOUT_MS, || r8(base, REG_SOFTWARE_RESET) & SRST_ALL == 0);
    let srst = r8(base, REG_SOFTWARE_RESET);
    serial_println!(
        "[sdhc] reset-all srst={:#04x} cleared={} (bound {}ms)",
        srst, cleared as u8, RESET_TIMEOUT_MS
    );
    if !cleared {
        serial_println!("[sdhc] reset-all did NOT complete — controller unresponsive, stopping");
    }
    cleared
}

/// Reset only the CMD and DAT circuits. Required by the spec after a command timeout: the CMD line
/// is left in an undefined state and the next command would be issued into it.
fn reset_cmd_dat(base: u64) {
    w8(base, REG_SOFTWARE_RESET, SRST_CMD | SRST_DAT);
    let _ = wait_ms(RESET_TIMEOUT_MS, || {
        r8(base, REG_SOFTWARE_RESET) & (SRST_CMD | SRST_DAT) == 0
    });
}

/// Latch interrupt STATUS for polling, and keep interrupt SIGNALLING off.
///
/// This distinction is load-bearing: a status bit only appears in the Interrupt Status register if
/// the corresponding Status Enable bit is set, so with Status Enable at 0 every poll below would
/// read a permanent 0 — a controller that works exactly as indistinguishable from one that is dead.
/// Signal Enable stays 0 because this driver is polled; enabling it would route these to the IDT.
fn arm_polled_status(base: u64) {
    w32(base, REG_INT_STATUS_EN, 0xFFFF_FFFF);
    w32(base, REG_INT_SIGNAL_EN, 0x0000_0000);
    w32(base, REG_INT_STATUS, 0xFFFF_FFFF); // W1C anything stale from before the reset
}

/// Turn on bus power at the highest voltage BOTH the controller's Capabilities and this driver
/// support. The voltage select field must be programmed before (or with) the power-on bit; a
/// controller may ignore a power-on written against an unsupported voltage, which then presents as
/// a card that never answers CMD0.
///
/// Returns the selected millivolts, or `None` if the controller advertises no supported voltage.
fn set_power(base: u64, caps: &HostCaps) -> Option<u32> {
    let (sel, mv) = if caps.caps & CAP_V33 != 0 {
        (PWR_V33, 3300)
    } else if caps.caps & CAP_V30 != 0 {
        (PWR_V30, 3000)
    } else if caps.caps & CAP_V18 != 0 {
        (PWR_V18, 1800)
    } else {
        serial_println!(
            "[sdhc] power: caps {:#010x} advertise NO supported bus voltage (v3.3/v3.0/v1.8 all 0) — stopping",
            caps.caps
        );
        return None;
    };

    // Voltage first with power off, then power on: the spec's own ordering.
    w8(base, REG_POWER_CONTROL, sel);
    w8(base, REG_POWER_CONTROL, sel | PWR_ON);
    let readback = r8(base, REG_POWER_CONTROL);
    serial_println!(
        "[sdhc] power sel={:#04x} pwrctl={:#04x} v={}mV on={} (healthy-idle pre-power reading is 0x00)",
        sel, readback, mv, (readback & PWR_ON != 0) as u8
    );
    if readback & PWR_ON == 0 {
        serial_println!("[sdhc] power: SD Bus Power did not latch — stopping");
        return None;
    }
    Some(mv)
}

/// Program the SD clock to at most `target_hz`, then enable it.
///
/// Two encodings exist and the controller's spec version decides which:
/// * **spec 3.00+ (10-bit divided clock):** SDCLK = base / (2·N), N a 10-bit value in
///   ClockControl[15:8] (low) and [7:6] (high). N = 0 selects the base clock undivided.
/// * **spec ≤ 2.00 (8-bit divided clock):** the field is ONE-HOT — 0x00 = base, 0x01 = base/2,
///   0x02 = base/4, 0x04 = /8, … 0x80 = /256. Writing a 3.00-style N here selects a wildly wrong
///   frequency, which is exactly the kind of silent misprogramming that shows up as "the card never
///   answers". QEMU's `sdhci-pci` reports spec 2.00, so this leg is not hypothetical.
///
/// Returns the actual SDCLK in Hz on success.
fn set_clock(base: u64, caps: &HostCaps, target_hz: u32) -> Option<u32> {
    if caps.base_hz == 0 {
        serial_println!(
            "[sdhc] clock: Capabilities base-clock field is 0 — the controller does not report its\
             clock and x86 has no second source (no VideoCore mailbox); stopping"
        );
        return None;
    }

    // Stop the SD clock before touching the divider (spec: the divider may only change while
    // SD Clock Enable is 0).
    let cc = r16(base, REG_CLOCK_CONTROL) & !CLK_SD_EN;
    w16(base, REG_CLOCK_CONTROL, cc);

    let (field, actual, mode) = if caps.is_v3() {
        // N = ceil(base / (2·target)), clamped into 10 bits. N >= 1 so we never select the
        // undivided base clock by accident on a slow-identification step.
        let n = caps.base_hz.div_ceil(target_hz.saturating_mul(2).max(1)).clamp(1, 0x3FF);
        let field = (((n & 0xFF) as u16) << 8) | ((((n >> 8) & 0x3) as u16) << 6);
        (field, caps.base_hz / (2 * n), "10bit")
    } else {
        // One-hot: find the smallest shift s (divisor 2^s) with base / 2^s <= target. s = 0 means
        // the undivided base clock (field 0x00); s >= 1 means field = 1 << (s - 1), up to 0x80.
        let mut s: u32 = 0;
        while s < 9 && caps.base_hz >> s > target_hz {
            s += 1;
        }
        let field_val: u16 = if s == 0 { 0 } else { 1u16 << (s - 1) };
        (field_val << 8, caps.base_hz >> s, "8bit")
    };

    // Rebuild Clock Control: clear the frequency-select field ([15:8] and, on 3.00, [7:6]) and the
    // generator-select bit (divided clock mode), then set the new divider + Internal Clock Enable.
    let mut cc = r16(base, REG_CLOCK_CONTROL);
    cc &= !0xFFC0; // frequency select, both halves
    cc &= !CLK_GEN_PROGRAMMABLE; // divided-clock mode
    cc |= field;
    cc |= CLK_INTERNAL_EN;
    w16(base, REG_CLOCK_CONTROL, cc);

    // Data Timeout Counter: 0x0E is the maximum (TMCLK · 2^27). Set to max so a slow card cannot be
    // failed by the controller's own timeout before our millisecond budget has had its say.
    w8(base, REG_TIMEOUT_CONTROL, 0x0E);

    // NOW the stable bit means something. Before the write above it reads 0 on a perfectly healthy
    // controller — identical to a dead one — which is why it is only polled from here.
    let stable = wait_ms(CLK_STABLE_TIMEOUT_MS, || {
        r16(base, REG_CLOCK_CONTROL) & CLK_INTERNAL_STABLE != 0
    });
    let cc_now = r16(base, REG_CLOCK_CONTROL);
    if !stable {
        serial_println!(
            "[sdhc] clock: internal clock NOT stable after {}ms (clkctl={:#06x}) — stopping",
            CLK_STABLE_TIMEOUT_MS, cc_now
        );
        return None;
    }

    w16(base, REG_CLOCK_CONTROL, cc_now | CLK_SD_EN);
    let cc_final = r16(base, REG_CLOCK_CONTROL);
    serial_println!(
        "[sdhc] clock mode={} base={}Hz target={}Hz actual={}Hz clkctl={:#06x} stable=1 sd-clk-en={}",
        mode, caps.base_hz, target_hz, actual, cc_final, (cc_final & CLK_SD_EN != 0) as u8
    );
    Some(actual)
}

/// Wait for the card-detect debounce to settle, THEN read Card Inserted.
///
/// This is the ordering the witness rule exists to enforce. `PS_CARD_INSERTED` is undefined while
/// `PS_CARD_STABLE` is 0, and a Software Reset For All restarts the debounce — so the reading taken
/// immediately after the reset is exactly the meaningless one. A healthy controller with no card
/// settles to stable=1/inserted=0; a healthy controller with a card settles to stable=1/inserted=1;
/// a controller whose debounce never settles reads stable=0, which this reports as its own outcome
/// rather than collapsing into "no card".
fn wait_card_detect(base: u64) -> Option<bool> {
    let settled = wait_ms(CD_STABLE_TIMEOUT_MS, || {
        r32(base, REG_PRESENT_STATE) & PS_CARD_STABLE != 0
    });
    let present = r32(base, REG_PRESENT_STATE);
    serial_println!(
        "[sdhc] card-detect present={:#010x} cd-stable={} card-inserted={} cd-pin={} wp-switch={} (bound {}ms)",
        present,
        (present & PS_CARD_STABLE != 0) as u8,
        (present & PS_CARD_INSERTED != 0) as u8,
        (present & PS_CARD_DETECT_PIN != 0) as u8,
        (present & PS_WRITE_PROTECT != 0) as u8,
        CD_STABLE_TIMEOUT_MS
    );
    if !settled {
        serial_println!(
            "[sdhc] card-detect: debounce never settled — 'card inserted' is UNDEFINED here, not 0; stopping"
        );
        return None;
    }
    Some(present & PS_CARD_INSERTED != 0)
}

// ===================================================================================
// Step 3 — the command layer
// ===================================================================================

// --- Command register (0x0E, 16-bit) fields ---------------------------------------
const RSP_NONE: u16 = 0b00;
const RSP_136: u16 = 0b01;
const RSP_48: u16 = 0b10;
const RSP_48_BUSY: u16 = 0b11;
const CMD_CRC_CHECK: u16 = 1 << 3;
const CMD_INDEX_CHECK: u16 = 1 << 4;
const CMD_DATA_PRESENT: u16 = 1 << 5;

/// Assemble the Command register word: index in bits 13:8, flags in the low byte.
#[inline]
const fn cmd_word(index: u8, flags: u16) -> u16 {
    ((index as u16) << 8) | flags
}

// --- Transfer Mode (0x0C, 16-bit) fields ------------------------------------------
/// DMA Enable: the data phase is moved by whichever engine Host Control 1's DMA Select names,
/// instead of through the PIO FIFO. Left 0 by every milestone-2 path and by the PIO path in
/// milestone 3, so those transfers are unchanged.
const TM_DMA_EN: u16 = 1 << 0;
const TM_BLOCK_COUNT_EN: u16 = 1 << 1;
/// Auto CMD12 Enable: the controller issues STOP_TRANSMISSION itself at the end of the transfer.
/// Only meaningful together with Block Count Enable — the block counter is what tells the
/// controller *when* the transfer ended.
const TM_AUTO_CMD12: u16 = 1 << 2;
const TM_DIR_READ: u16 = 1 << 4; // 1 = card -> host
const TM_MULTI_BLOCK: u16 = 1 << 5;

/// R1 card-status error bits (SD Physical Layer §4.10.1): OUT_OF_RANGE(31), ADDRESS_ERROR(30),
/// BLOCK_LEN_ERROR(29), ERASE_SEQ_ERROR(28), ERASE_PARAM(27), WP_VIOLATION(26), CARD_IS_LOCKED(25),
/// LOCK_UNLOCK_FAILED(24), COM_CRC_ERROR(23), ILLEGAL_COMMAND(22), CARD_ECC_FAILED(21),
/// CC_ERROR(20), ERROR(19), CSD_OVERWRITE(16), WP_ERASE_SKIP(15), AKE_SEQ_ERROR(3).
///
/// This is the CARD's verdict and is disjoint from the controller's error-interrupt bits, which
/// only see the LINK (CRC, timeout, index). A card can complete a command cleanly on the wire while
/// rejecting it here; without this check that rejection is silently swallowed.
const R1_ERROR_MASK: u32 = 0xFFF9_8008;

/// One command's outcome. The error carries the RAW Interrupt Status word that convicted it, so a
/// failure line reports the evidence rather than a summary of it.
type CmdResult = Result<(), u32>;

/// Issue one command and wait for it to complete.
///
/// Ordering is fixed by the spec: both Command Inhibits must be clear, then Argument, then Transfer
/// Mode, then Command — writing the Command register is what issues it, so it must be last.
///
/// Waiting on **Command Inhibit (DAT)** and not merely (CMD) is load-bearing, and the Pi driver
/// learned it on metal: a preceding R1b (CMD7 SELECT) leaves the card asserting busy on DAT0, which
/// the controller reflects here until it clears. Issuing the next command into that window is
/// rejected by real silicon while QEMU, which deasserts busy instantly, never shows it.
///
/// On any error the CMD and DAT circuits are reset before returning. The spec requires this after a
/// command timeout — the line is left in an undefined state, and the next command would be issued
/// into it, turning one failure into a cascade that indicts the wrong step.
fn send_command(base: u64, command: u16, arg: u32, data: bool) -> CmdResult {
    if !wait_ms(CMD_TIMEOUT_MS, || {
        r32(base, REG_PRESENT_STATE) & (PS_CMD_INHIBIT | PS_DAT_INHIBIT) == 0
    }) {
        let ps = r32(base, REG_PRESENT_STATE);
        serial_println!("[sdhc] cmd{}: line still busy, present={:#010x}", (command >> 8) & 0x3F, ps);
        reset_cmd_dat(base);
        return Err(0);
    }

    w32(base, REG_INT_STATUS, 0xFFFF_FFFF); // W1C anything stale
    w32(base, REG_ARGUMENT, arg);
    if !data {
        // No data phase: clear Transfer Mode outright, so no auto-CMD or DMA bit can survive from a
        // previous transfer and attach itself to this command.
        w16(base, REG_TRANSFER_MODE, 0);
    }
    w16(base, REG_COMMAND, command); // ISSUES

    // Command Complete OR any error. A card that is not there never sets Command Complete; the
    // controller flags Command Timeout instead, and may set BOTH — so the error half is tested
    // regardless of Command Complete rather than after it.
    if !wait_ms(CMD_TIMEOUT_MS, || {
        r32(base, REG_INT_STATUS) & (INT_CMD_COMPLETE | INT_ERR_ANY) != 0
    }) {
        let int = r32(base, REG_INT_STATUS);
        reset_cmd_dat(base);
        return Err(int);
    }
    let int = r32(base, REG_INT_STATUS);
    if int & INT_ERR_ANY != 0 {
        w32(base, REG_INT_STATUS, int); // W1C exactly what we saw
        reset_cmd_dat(base);
        return Err(int);
    }
    w32(base, REG_INT_STATUS, INT_CMD_COMPLETE);
    Ok(())
}

/// The four response registers. Valid only once Command Complete has been observed — before that
/// they hold the PREVIOUS command's response, which is the most convincing kind of wrong answer.
#[inline]
fn read_response(base: u64) -> [u32; 4] {
    [
        r32(base, REG_RESPONSE0),
        r32(base, REG_RESPONSE0 + 4),
        r32(base, REG_RESPONSE0 + 8),
        r32(base, REG_RESPONSE0 + 12),
    ]
}

/// Check the R1 card-status word (Response0 after any R1 command) for card-reported errors.
fn r1_check(base: u64, who: &str) -> Result<(), u32> {
    let r1 = r32(base, REG_RESPONSE0);
    if r1 & R1_ERROR_MASK != 0 {
        serial_println!("[sdhc] {} rejected by the CARD: R1={:#010x}", who, r1);
        return Err(r1);
    }
    Ok(())
}

/// Name the error bits in a raw Interrupt Status word, for the failure lines.
fn int_error_name(int: u32) -> &'static str {
    if int & INT_ERR_CMD_TIMEOUT != 0 {
        "cmd-timeout"
    } else if int & INT_ERR_CMD_CRC != 0 {
        "cmd-crc"
    } else if int & INT_ERR_CMD_END_BIT != 0 {
        "cmd-end-bit"
    } else if int & INT_ERR_CMD_INDEX != 0 {
        "cmd-index"
    } else if int & INT_ERR_DATA_TIMEOUT != 0 {
        "data-timeout"
    } else if int & INT_ERR_DATA_CRC != 0 {
        "data-crc"
    } else if int & INT_ERR_DATA_END_BIT != 0 {
        "data-end-bit"
    } else if int & INT_ERR_AUTO_CMD != 0 {
        "auto-cmd12"
    } else if int & INT_ERR_ADMA != 0 {
        "adma"
    } else if int & INT_ERR_ANY != 0 {
        "other-error"
    } else {
        "no-response-within-budget"
    }
}

/// Extract bit range `[hi:lo]` from a 136-bit R2 response (CID or CSD).
///
/// The controller strips the CRC byte and shifts the 120-bit content right by 8, so register bit
/// `b-8` carries CSD/CID bit `b`. This off-by-8 is the classic way to mis-read a capacity by orders
/// of magnitude, which is why it lives in one place. `lo` must be >= 8 (bits 7:0 are the CRC the
/// controller consumed).
fn r2_bits(resp: &[u32; 4], hi: u32, lo: u32) -> u64 {
    let mut val = 0u64;
    let mut b = hi;
    loop {
        let r = b - 8;
        let bit = (resp[(r / 32) as usize] >> (r % 32)) & 1;
        val = (val << 1) | bit as u64;
        if b == lo {
            break;
        }
        b -= 1;
    }
    val
}

// ===================================================================================
// The identified card
// ===================================================================================

/// Whether this card's controller has a usable ADMA2 engine, and — when it has not — the reason,
/// which is carried so it can be NAMED on serial instead of degrading into a silent PIO fallback.
///
/// `Unavailable` is decided once at engine init and never changes. `Faulted` is latched the first
/// time a live transfer fails, so one bad transfer disables the engine for the rest of the boot
/// rather than re-failing on every read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Adma2State {
    Ready,
    Unavailable(&'static str),
    Faulted(&'static str),
}

impl Adma2State {
    /// The reason string, for the lines that must name why DMA is not being used.
    fn reason(&self) -> &'static str {
        match self {
            Adma2State::Ready => "ready",
            Adma2State::Unavailable(r) | Adma2State::Faulted(r) => r,
        }
    }
}

/// One identified SD card behind one SDHCI function.
pub struct SdCard {
    base: u64,
    bdf: (u8, u8, u8),
    /// From ACMD41 bit 30 (CCS). **This — not the capacity — is what governs addressing:**
    /// true = block-addressed (SDHC/SDXC), false = byte-addressed (SDSC).
    block_addressing: bool,
    num_blocks: u64,
    /// CMD3's relative card address, already shifted into bits 31:16 for use as a command argument.
    rca_arg: u32,
    csd_structure: u8,
    /// ADMA2 engine state — `Unavailable` until [`adma2_init`] has run.
    adma: Adma2State,
    /// Physical address of the one-page ADMA2 descriptor table, or 0. Held as an address rather
    /// than a pointer so `SdCard` stays `Send` and can live in the `CARD` static; on x86_64 the
    /// kernel heap is identity-mapped, so this value is simultaneously the CPU address and the bus
    /// address (see `arch::x86_64::memory`).
    adma_table: u64,
    /// Physical address of the driver-owned DMA bounce buffer, or 0.
    adma_bounce: u64,
}

static CARD: Mutex<Option<SdCard>> = Mutex::new(None);

/// The identified card's 512-byte block count, or `None` until [`probe`] has identified one.
///
/// Deliberately NOT published into `drivers::block::BLOCK_DEVICE`: the x86 block layer has no
/// backend selector, so `publish_usb_geometry` claims that global unconditionally and a card
/// registered there would silently fight the USB stick for it — the PI-FS-2 clobber in reverse.
pub fn card_num_blocks() -> Option<u64> {
    CARD.lock().as_ref().map(|c| c.num_blocks)
}

/// Whether the identified card is block-addressed (SDHC/SDXC) rather than byte-addressed (SDSC).
pub fn card_block_addressed() -> Option<bool> {
    CARD.lock().as_ref().map(|c| c.block_addressing)
}

// ===================================================================================
// Step 3 — card identification
// ===================================================================================

/// Render up to 8 CID name bytes as printable ASCII, substituting '.' for anything else, so a
/// garbled CID reads as garbled rather than corrupting the serial stream.
fn ascii_field(buf: &mut [u8], val: u64, chars: usize) -> usize {
    for i in 0..chars {
        let byte = ((val >> (8 * (chars - 1 - i))) & 0xFF) as u8;
        buf[i] = if (0x20..0x7F).contains(&byte) { byte } else { b'.' };
    }
    chars
}

/// Run the SD card identification ladder on a powered, clocked bus with a card present.
///
/// CMD0 → CMD8 → (CMD55 + ACMD41)* → CMD2 → CMD3 → CMD9 → CMD7 → CMD16, then raise the clock from
/// the identification rate to default speed. Each step logs its own outcome; the first failure
/// stops the ladder and returns `None`.
fn identify(base: u64, caps: &HostCaps, bdf: (u8, u8, u8)) -> Option<SdCard> {
    // --- CMD0 GO_IDLE_STATE. No response, so there is nothing to check but the absence of a
    // controller-level error; a card that is present but wedged still accepts it silently.
    if let Err(int) = send_command(base, cmd_word(0, RSP_NONE), 0, false) {
        serial_println!("[sdhc] cmd0 go-idle FAILED int={:#010x} ({})", int, int_error_name(int));
        return None;
    }
    serial_println!("[sdhc] cmd0 go-idle ok");

    // --- CMD8 SEND_IF_COND (R7). Argument 0x1AA = supply-voltage class 1 (2.7–3.6 V) plus the
    // check pattern 0xAA. The card must ECHO both back; that echo is the discriminator between a
    // v2.00+ card and a v1.x card / no card at all, and it is the first proof that data flows in
    // BOTH directions on this bus.
    if let Err(int) = send_command(base, cmd_word(8, RSP_48 | CMD_CRC_CHECK | CMD_INDEX_CHECK), 0x1AA, false) {
        serial_println!(
            "[sdhc] cmd8 send-if-cond FAILED int={:#010x} ({}) — card is pre-v2.00 or absent; \
             this milestone identifies v2.00+ cards only",
            int, int_error_name(int)
        );
        return None;
    }
    let r7 = r32(base, REG_RESPONSE0);
    if r7 & 0xFFF != 0x1AA {
        serial_println!(
            "[sdhc] cmd8 echo MISMATCH: sent 0x1aa, resp0={:#010x} (echo={:#05x}) — the bus is not \
             carrying the card's answer intact; stopping",
            r7, r7 & 0xFFF
        );
        return None;
    }
    serial_println!("[sdhc] cmd8 send-if-cond resp0={:#010x} echo=0x1aa ok (v2.00+ card)", r7);

    // --- ACMD41 SD_SEND_OP_COND, bounded. Each iteration is CMD55 (APP_CMD, R1, argument 0 because
    // the card has no RCA yet) then ACMD41 (R3). R3 carries the OCR, which has NO CRC and NO command
    // index — so CRC and index checking must be OFF for it, or a healthy card is convicted of a link
    // error. Argument 0x40FF8000 = HCS (host supports high capacity) plus the 2.7–3.6 V window.
    //
    // Bit 31 is "power-up complete" and reads 0 for as long as the card is still initialising; that
    // 0 is a legitimate in-progress answer, not a failure, and only the elapsed-time bound can tell
    // the two apart.
    let start = crate::arch::now_cycles();
    let budget = cycles_ms(ACMD41_TIMEOUT_MS);
    let mut polls: u32 = 0;
    let ocr = loop {
        polls += 1;
        if let Err(int) = send_command(base, cmd_word(55, RSP_48 | CMD_CRC_CHECK | CMD_INDEX_CHECK), 0, false) {
            serial_println!("[sdhc] cmd55 app-cmd FAILED int={:#010x} ({})", int, int_error_name(int));
            return None;
        }
        if let Err(int) = send_command(base, cmd_word(41, RSP_48), 0x40FF_8000, false) {
            serial_println!("[sdhc] acmd41 FAILED int={:#010x} ({})", int, int_error_name(int));
            return None;
        }
        let ocr = r32(base, REG_RESPONSE0);
        if ocr & (1 << 31) != 0 {
            break ocr;
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= budget {
            serial_println!(
                "[sdhc] acmd41 still busy after {}ms and {} polls (ocr={:#010x}) — card never \
                 finished power-up; stopping",
                ACMD41_TIMEOUT_MS, polls, ocr
            );
            return None;
        }
        core::hint::spin_loop();
    };
    let ccs = ocr & (1 << 30) != 0;
    serial_println!(
        "[sdhc] acmd41 ocr={:#010x} powered-up=1 ccs={} ({}-addressed) after {} polls",
        ocr, ccs as u8,
        if ccs { "block" } else { "byte" },
        polls
    );

    // --- CMD2 ALL_SEND_CID (R2). Moves the card to identification state and returns the CID. R2 has
    // no command index to check, so index checking is off; CRC stays on.
    if let Err(int) = send_command(base, cmd_word(2, RSP_136 | CMD_CRC_CHECK), 0, false) {
        serial_println!("[sdhc] cmd2 all-send-cid FAILED int={:#010x} ({})", int, int_error_name(int));
        return None;
    }
    let cid = read_response(base);
    serial_println!(
        "[sdhc] cmd2 cid raw=[{:#010x},{:#010x},{:#010x},{:#010x}]",
        cid[0], cid[1], cid[2], cid[3]
    );
    {
        let mid = r2_bits(&cid, 127, 120);
        let oid = r2_bits(&cid, 119, 104);
        let pnm = r2_bits(&cid, 103, 64);
        let prv = r2_bits(&cid, 63, 56);
        let psn = r2_bits(&cid, 55, 24);
        let mdt = r2_bits(&cid, 19, 8);
        let mut oid_buf = [0u8; 2];
        let mut pnm_buf = [0u8; 5];
        ascii_field(&mut oid_buf, oid, 2);
        ascii_field(&mut pnm_buf, pnm, 5);
        serial_println!(
            "[sdhc] cid mid={:#04x} oid={} pnm={} prv={}.{} psn={:#010x} mdt={}-{:02}",
            mid,
            core::str::from_utf8(&oid_buf).unwrap_or("??"),
            core::str::from_utf8(&pnm_buf).unwrap_or("?????"),
            (prv >> 4) & 0xF, prv & 0xF,
            psn,
            2000 + ((mdt >> 4) & 0xFF), mdt & 0xF
        );
    }

    // --- CMD3 SEND_RELATIVE_ADDR (R6). The card publishes its RCA in response bits 31:16; every
    // addressed command from here on carries it.
    if let Err(int) = send_command(base, cmd_word(3, RSP_48 | CMD_CRC_CHECK | CMD_INDEX_CHECK), 0, false) {
        serial_println!("[sdhc] cmd3 send-rca FAILED int={:#010x} ({})", int, int_error_name(int));
        return None;
    }
    let r6 = r32(base, REG_RESPONSE0);
    let rca = r6 >> 16;
    let rca_arg = rca << 16;
    if rca == 0 {
        serial_println!("[sdhc] cmd3 returned RCA 0 (resp0={:#010x}) — RCA 0 deselects; stopping", r6);
        return None;
    }
    serial_println!("[sdhc] cmd3 resp0={:#010x} rca={:#06x}", r6, rca);

    // --- CMD9 SEND_CSD (R2). Must be issued while the card is in stand-by — after CMD3 and BEFORE
    // CMD7 SELECT. Issued after CMD7 the card is in transfer state and rejects it, which is why the
    // ordering here is not cosmetic.
    if let Err(int) = send_command(base, cmd_word(9, RSP_136 | CMD_CRC_CHECK), rca_arg, false) {
        serial_println!("[sdhc] cmd9 send-csd FAILED int={:#010x} ({})", int, int_error_name(int));
        return None;
    }
    let csd = read_response(base);
    serial_println!(
        "[sdhc] cmd9 csd raw=[{:#010x},{:#010x},{:#010x},{:#010x}]",
        csd[0], csd[1], csd[2], csd[3]
    );
    let csd_structure = r2_bits(&csd, 127, 126) as u8;
    let num_blocks = if csd_structure == 1 {
        // CSD v2 (SDHC/SDXC): C_SIZE = CSD[69:48]; capacity = (C_SIZE+1) · 512 KiB.
        let c_size = r2_bits(&csd, 69, 48);
        serial_println!("[sdhc] csd v2 c_size={} -> blocks=(c_size+1)*1024", c_size);
        (c_size + 1) * 1024
    } else if csd_structure == 0 {
        // CSD v1 (SDSC): blocks = (C_SIZE+1) · 2^(C_SIZE_MULT+2) · 2^READ_BL_LEN / 512.
        let read_bl_len = r2_bits(&csd, 83, 80) as u32;
        let c_size = r2_bits(&csd, 73, 62);
        let c_size_mult = r2_bits(&csd, 49, 47) as u32;
        serial_println!(
            "[sdhc] csd v1 c_size={} c_size_mult={} read_bl_len={}",
            c_size, c_size_mult, read_bl_len
        );
        ((c_size + 1) << (c_size_mult + 2)) * (1u64 << read_bl_len) / 512
    } else {
        serial_println!(
            "[sdhc] csd structure {} is not 0 (v1) or 1 (v2) — capacity cannot be derived; stopping",
            csd_structure
        );
        return None;
    };
    if num_blocks == 0 {
        serial_println!("[sdhc] csd yields a zero-block card — refusing to believe it; stopping");
        return None;
    }

    // CROSS-CHECK. CCS (from ACMD41) and the CSD structure version are set by the card from the same
    // fact — a high-capacity card reports CCS=1 and CSD v2, a standard-capacity card CCS=0 and v1.
    // If they disagree, one of the two reads is wrong, and since CCS decides whether every later LBA
    // is a block index or a byte offset, believing the wrong one corrupts every access. Two
    // independent registers agreeing is what makes either of them evidence.
    let expected_ccs = csd_structure == 1;
    if expected_ccs != ccs {
        serial_println!(
            "[sdhc] MISMATCH: acmd41 ccs={} but csd structure={} (implies ccs={}) — addressing mode \
             is unresolved and a wrong guess corrupts every access; stopping",
            ccs as u8, csd_structure, expected_ccs as u8
        );
        return None;
    }

    let mib = num_blocks * 512 / (1024 * 1024);
    let class = if !ccs {
        "SDSC (standard capacity)"
    } else if mib <= 32 * 1024 {
        "SDHC"
    } else {
        "SDXC"
    };
    serial_println!(
        "[sdhc] card {} blocks x512 = {}MiB class={} addressing={} (ccs governs, csd v{} agrees)",
        num_blocks, mib, class,
        if ccs { "block" } else { "byte" },
        if csd_structure == 1 { 2 } else { 1 }
    );

    // --- CMD7 SELECT_CARD (R1b) → transfer state. R1b means the card asserts busy on DAT0 after the
    // response; the next command's Command-Inhibit(DAT) wait is what absorbs it.
    if let Err(int) = send_command(base, cmd_word(7, RSP_48_BUSY | CMD_CRC_CHECK | CMD_INDEX_CHECK), rca_arg, false) {
        serial_println!("[sdhc] cmd7 select FAILED int={:#010x} ({})", int, int_error_name(int));
        return None;
    }
    if r1_check(base, "cmd7 select").is_err() {
        return None;
    }
    serial_println!("[sdhc] cmd7 select ok (transfer state)");

    // --- CMD16 SET_BLOCKLEN 512 (R1). Meaningful for SDSC; on a block-addressed card 512 is fixed
    // and this is a harmless confirmation.
    if let Err(int) = send_command(base, cmd_word(16, RSP_48 | CMD_CRC_CHECK | CMD_INDEX_CHECK), 512, false) {
        serial_println!("[sdhc] cmd16 set-blocklen FAILED int={:#010x} ({})", int, int_error_name(int));
        return None;
    }
    if r1_check(base, "cmd16 set-blocklen").is_err() {
        return None;
    }
    serial_println!("[sdhc] cmd16 set-blocklen=512 ok");

    // --- Identification is over; raise the clock from 400 kHz to default speed.
    let actual = set_clock(base, caps, CLK_TRANSFER_HZ)?;
    serial_println!("[sdhc] transfer clock {}Hz engaged", actual);

    Some(SdCard {
        base,
        bdf,
        block_addressing: ccs,
        num_blocks,
        rca_arg,
        csd_structure,
        // The engine starts refused and is only granted by `adma2_init`. A default of "ready"
        // would make a forgotten init call read as a working DMA path.
        adma: Adma2State::Unavailable("engine init has not run"),
        adma_table: 0,
        adma_bounce: 0,
    })
}

// ===================================================================================
// Step 4 — a single-block read (CMD17), PIO
// ===================================================================================

/// Read one 512-byte block at `lba` into `buf` via a polled single-block CMD17. Returns the number
/// of bytes copied. No DMA and no cache maintenance: the FIFO is drained through the uncacheable
/// BAR window into a normal kernel buffer.
///
/// The addressing mode is taken from the card's CCS, which `identify` refused to guess at — an LBA
/// handed to a byte-addressed card as if it were a block index reads the wrong 512 bytes without
/// erroring, which is why that cross-check had to pass before this function could exist.
pub fn read_block_512(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let guard = CARD.lock();
    let card = guard.as_ref().ok_or(BlockError::NotReady)?;
    if lba >= card.num_blocks {
        return Err(BlockError::BadLba);
    }
    let base = card.base;

    // The Argument register is 32 bits, so both addressing modes have a reachability limit and both
    // are checked rather than truncated: a silently wrapped address reads a real but wrong block.
    let arg = if card.block_addressing {
        u32::try_from(lba).map_err(|_| BlockError::BadLba)?
    } else {
        let byte_off = lba.checked_mul(512).ok_or(BlockError::BadLba)?;
        u32::try_from(byte_off).map_err(|_| BlockError::BadLba)?
    };

    w32(base, REG_INT_STATUS, 0xFFFF_FFFF); // W1C stale status
    w16(base, REG_BLOCK_SIZE, 512); // bits 11:0 = block length; SDMA boundary bits stay 0 (PIO)
    w16(base, REG_BLOCK_COUNT, 1);
    // Single block, card -> host, block count enabled, DMA disabled, no auto-CMD. Written BEFORE the
    // Command register, which is what issues the transfer.
    w16(base, REG_TRANSFER_MODE, TM_BLOCK_COUNT_EN | TM_DIR_READ);

    send_command(
        base,
        cmd_word(17, RSP_48 | CMD_CRC_CHECK | CMD_INDEX_CHECK | CMD_DATA_PRESENT),
        arg,
        true,
    )
    .map_err(|_| BlockError::Io)?;
    // The card answered on the wire; now its own verdict, BEFORE touching the FIFO. A card that
    // rejected CMD17 never fills the buffer, and draining it anyway would return stale bytes with
    // an Ok beside them.
    r1_check(base, "cmd17 read").map_err(|_| BlockError::Io)?;

    // Buffer Read Ready means the controller has a full block staged for PIO. Reading the data port
    // before it is set returns undefined data — this bit is the only thing that makes the FIFO
    // meaningful, and it reads 0 both for a healthy controller mid-transfer and for one that will
    // never deliver, so only the bounded wait separates them.
    if !wait_ms(DATA_TIMEOUT_MS, || {
        r32(base, REG_INT_STATUS) & (INT_BUF_READ_READY | INT_ERR_ANY) != 0
    }) {
        return Err(BlockError::Io);
    }
    let int = r32(base, REG_INT_STATUS);
    if int & INT_ERR_ANY != 0 {
        w32(base, REG_INT_STATUS, int);
        reset_cmd_dat(base);
        return Err(BlockError::Io);
    }
    w32(base, REG_INT_STATUS, INT_BUF_READ_READY);

    // Drain exactly 128 little-endian words. The controller expects the whole block to be consumed;
    // a short drain leaves the FIFO loaded and desynchronises the next transfer, so all 128 are
    // always read even when the caller's buffer is shorter.
    let n = buf.len().min(512);
    for i in 0..128usize {
        let word = r32(base, REG_BUFFER_DATA);
        let bytes = word.to_le_bytes();
        let off = i * 4;
        for (k, &b) in bytes.iter().enumerate() {
            if off + k < n {
                buf[off + k] = b;
            }
        }
    }

    if !wait_ms(DATA_TIMEOUT_MS, || {
        r32(base, REG_INT_STATUS) & (INT_XFER_COMPLETE | INT_ERR_ANY) != 0
    }) {
        return Err(BlockError::Io);
    }
    let int = r32(base, REG_INT_STATUS);
    w32(base, REG_INT_STATUS, int); // W1C everything we saw
    if int & INT_ERR_ANY != 0 {
        reset_cmd_dat(base);
        return Err(BlockError::Io);
    }
    Ok(n)
}

// ===================================================================================
// Step 5 — a multi-block read (CMD18), still PIO
//
// Milestone 3's first rung, and it deliberately adds NO new risk surface: no DMA, no Bus Master,
// the same FIFO drain `read_block_512` already proved on metal. What is new is only the COMMAND
// SEMANTICS — CMD18 with Block Count Enable and Auto CMD12 — so that when ADMA2 is layered on top
// in the next rung and something breaks, the failure indicts the DMA engine rather than leaving
// "multi-block" and "DMA" jointly suspect. That is §6.6's own layering argument, applied.
// ===================================================================================

/// The most 512-byte blocks one CMD18 transfer moves: 64 blocks = 32 KiB. Bounds one transfer's
/// drain and one ADMA2 descriptor run, and matches the chunk the aarch64 Tegra driver already runs
/// on metal (`arch::aarch64::sdmmc_tegra`'s `MULTIBLOCK_CHUNK_BLOCKS`).
const MB_MAX_BLOCKS: u16 = 64;

/// Translate an LBA into the Argument-register value this card expects.
///
/// The Argument register is 32 bits, so BOTH addressing modes have a reachability limit and both
/// are checked rather than truncated: a silently wrapped address reads a real but wrong block. This
/// mirrors the check inside [`read_block_512`] rather than refactoring it — that function is the
/// control instrument every comparison below is measured against, and it stays byte-for-byte as
/// metal verified it.
fn lba_arg(card: &SdCard, lba: u64) -> Result<u32, BlockError> {
    if card.block_addressing {
        u32::try_from(lba).map_err(|_| BlockError::BadLba)
    } else {
        let byte_off = lba.checked_mul(512).ok_or(BlockError::BadLba)?;
        u32::try_from(byte_off).map_err(|_| BlockError::BadLba)
    }
}

/// The (command word, transfer-mode bits, log name) for a counted read of `count` blocks.
///
/// `count == 1` issues **CMD17 READ_SINGLE_BLOCK with Multi/Single Block Select clear**, not CMD18
/// with a block count of one. Both are spec-legal, but the single-block form is the one every card
/// and controller in the field is exercised on — and it is what keeps the two arms of §7.3's A/B
/// comparison issuing the *same* command for the same window. An A/B test whose arms differ in more
/// than the variable under test measures nothing.
///
/// Auto CMD12 appears only on the multi-block leg, and only alongside Block Count Enable: the block
/// counter is what tells the controller when to issue the closing STOP.
fn counted_read_cmd(count: u16) -> (u16, u16, &'static str) {
    if count == 1 {
        (
            cmd_word(17, RSP_48 | CMD_CRC_CHECK | CMD_INDEX_CHECK | CMD_DATA_PRESENT),
            TM_BLOCK_COUNT_EN | TM_DIR_READ,
            "cmd17",
        )
    } else {
        (
            cmd_word(18, RSP_48 | CMD_CRC_CHECK | CMD_INDEX_CHECK | CMD_DATA_PRESENT),
            TM_BLOCK_COUNT_EN | TM_DIR_READ | TM_MULTI_BLOCK | TM_AUTO_CMD12,
            "cmd18",
        )
    }
}

/// Close a multi-block read that ended badly: CMD12 STOP_TRANSMISSION, then reset the CMD and DAT
/// circuits, then clear the status word.
///
/// The CMD12 is the part that is easy to skip and expensive to have skipped. An aborted CMD18
/// leaves the CARD in data state, streaming or waiting to stream; the controller-side reset clears
/// only the HOST's circuits. The next command issued to a card still in data state is rejected for
/// a reason that has nothing to do with that command — one failure becomes a cascade that indicts
/// the wrong step, which is exactly the class of misdiagnosis this driver exists to avoid.
///
/// CMD12 is R1b (the card asserts busy on DAT0 while it finishes), and `send_command`'s own
/// Command-Inhibit(DAT) wait on the *next* command absorbs that busy. Every leg is bounded:
/// `send_command` twice at `CMD_TIMEOUT_MS`, `reset_cmd_dat` at `RESET_TIMEOUT_MS`.
fn abort_data_transfer(base: u64) {
    let _ = send_command(base, cmd_word(12, RSP_48_BUSY | CMD_CRC_CHECK | CMD_INDEX_CHECK), 0, false);
    reset_cmd_dat(base);
    w32(base, REG_INT_STATUS, 0xFFFF_FFFF);
}

/// Read `count` contiguous 512-byte blocks starting at `lba` into `buf` via one polled CMD18
/// READ_MULTIPLE_BLOCK, draining the FIFO block by block. Returns the number of bytes written.
///
/// Auto CMD12 is enabled, so the controller issues the closing STOP itself the moment the block
/// counter reaches zero — which is why Block Count Enable is not optional here: the counter is what
/// tells the controller when the transfer ended. An Auto-CMD12 failure sets error-half bit 8, which
/// [`INT_ERR_ANY`] already covers and [`int_error_name`] now names.
///
/// Every failure path prints the RAW Interrupt Status word that convicted it and then closes the
/// card's side with [`abort_data_transfer`], so a failure here cannot poison the next command.
fn read_blocks_512_pio(lba: u64, count: u16, buf: &mut [u8]) -> Result<usize, BlockError> {
    let guard = CARD.lock();
    let card = guard.as_ref().ok_or(BlockError::NotReady)?;
    read_blocks_pio_on(card, lba, count, buf)
}

/// [`read_blocks_512_pio`]'s body, against an already-borrowed card. Split out so the dispatcher in
/// milestone 3c can fall back from ADMA2 to PIO without dropping and retaking `CARD`.
fn read_blocks_pio_on(
    card: &SdCard,
    lba: u64,
    count: u16,
    buf: &mut [u8],
) -> Result<usize, BlockError> {
    if count == 0 || count > MB_MAX_BLOCKS {
        serial_println!(
            "[sdhc-mb] refusing count={} (bound is 1..={}) — a transfer this driver cannot bound \
             is not issued",
            count, MB_MAX_BLOCKS
        );
        return Err(BlockError::Io);
    }
    let n = count as usize * 512;
    if buf.len() < n {
        serial_println!(
            "[sdhc-mb] refusing lba={} blocks={}: caller buffer is {} bytes, {} needed — a short \
             drain desynchronises the FIFO for every later transfer",
            lba, count, buf.len(), n
        );
        return Err(BlockError::Io);
    }
    if lba.checked_add(count as u64).is_none_or(|end| end > card.num_blocks) {
        return Err(BlockError::BadLba);
    }

    let base = card.base;
    let arg = lba_arg(card, lba)?;
    let (command, tm, name) = counted_read_cmd(count);

    w32(base, REG_INT_STATUS, 0xFFFF_FFFF); // W1C stale status
    w16(base, REG_BLOCK_SIZE, 512); // bits 11:0 = block length; SDMA boundary bits stay 0 (PIO)
    w16(base, REG_BLOCK_COUNT, count);
    // Card -> host, block count enabled, DMA still DISABLED. Written BEFORE the Command register,
    // which is what issues the transfer.
    w16(base, REG_TRANSFER_MODE, tm);

    if let Err(int) = send_command(base, command, arg, true) {
        serial_println!(
            "[sdhc-mb] {} lba={} blocks={} FAILED int={:#010x} ({})",
            name, lba, count, int, int_error_name(int)
        );
        abort_data_transfer(base);
        return Err(BlockError::Io);
    }
    // The card answered on the wire; now its own verdict, BEFORE touching the FIFO. A card that
    // rejected the read never fills the buffer, and draining it anyway would return stale bytes with
    // an Ok beside them.
    if let Err(r1) = r1_check(base, "multi-block read") {
        serial_println!("[sdhc-mb] {} lba={} blocks={} rejected by the CARD r1={:#010x}", name, lba, count, r1);
        abort_data_transfer(base);
        return Err(BlockError::Io);
    }

    for blk in 0..count as usize {
        // Buffer Read Ready is re-armed by the controller once per block. It reads 0 both for a
        // healthy controller mid-block and for one that will never deliver, so only the bounded
        // wait separates them — and the error half is tested in the same predicate, because a
        // failing transfer sets an error bit instead of, not before, Buffer Read Ready.
        if !wait_ms(DATA_TIMEOUT_MS, || {
            r32(base, REG_INT_STATUS) & (INT_BUF_READ_READY | INT_ERR_ANY) != 0
        }) {
            let int = r32(base, REG_INT_STATUS);
            serial_println!(
                "[sdhc-mb] {} lba={} blocks={} block {} buffer-read-ready TIMEOUT after {}ms \
                 int={:#010x} ({})",
                name, lba, count, blk, DATA_TIMEOUT_MS, int, int_error_name(int)
            );
            abort_data_transfer(base);
            return Err(BlockError::Io);
        }
        let int = r32(base, REG_INT_STATUS);
        if int & INT_ERR_ANY != 0 {
            serial_println!(
                "[sdhc-mb] {} lba={} blocks={} block {} ERROR int={:#010x} ({})",
                name, lba, count, blk, int, int_error_name(int)
            );
            w32(base, REG_INT_STATUS, int);
            abort_data_transfer(base);
            return Err(BlockError::Io);
        }
        w32(base, REG_INT_STATUS, INT_BUF_READ_READY); // W1C, re-arm for the next block

        let bo = blk * 512;
        for i in 0..128usize {
            let word = r32(base, REG_BUFFER_DATA);
            let off = bo + i * 4;
            buf[off..off + 4].copy_from_slice(&word.to_le_bytes());
        }
    }

    // Transfer Complete: the controller's Auto CMD12 has closed the card's read.
    if !wait_ms(DATA_TIMEOUT_MS, || {
        r32(base, REG_INT_STATUS) & (INT_XFER_COMPLETE | INT_ERR_ANY) != 0
    }) {
        let int = r32(base, REG_INT_STATUS);
        serial_println!(
            "[sdhc-mb] {} lba={} blocks={} transfer-complete TIMEOUT after {}ms int={:#010x} ({})",
            name, lba, count, DATA_TIMEOUT_MS, int, int_error_name(int)
        );
        abort_data_transfer(base);
        return Err(BlockError::Io);
    }
    let int = r32(base, REG_INT_STATUS);
    w32(base, REG_INT_STATUS, int); // W1C everything we saw
    if int & INT_ERR_ANY != 0 {
        serial_println!(
            "[sdhc-mb] {} lba={} blocks={} transfer ERROR int={:#010x} ({})",
            name, lba, count, int, int_error_name(int)
        );
        abort_data_transfer(base);
        return Err(BlockError::Io);
    }
    Ok(n)
}

// ===================================================================================
// Step 6 — the ADMA2 engine (SDHC-3b)
//
// The first DMA in this driver, and the first time this function is ever granted Bus Master. Every
// address the engine can reach is checked before that grant is made, and every buffer it can write
// is allocated and owned by this driver.
// ===================================================================================

/// One 4 KiB page of descriptors: 512 slots of 8 bytes. Three is the most any transfer below uses.
///
/// A 4 KiB-aligned 4 KiB region cannot cross a 64 KiB boundary, so the table satisfies ADMA2's
/// boundary rule **by construction** rather than by a check that could be got wrong.
const ADMA2_TABLE_BYTES: usize = 4096;
const ADMA2_DESC_BYTES: usize = 8;
const ADMA2_MAX_DESCS: usize = ADMA2_TABLE_BYTES / ADMA2_DESC_BYTES;

/// The driver-owned DMA landing zone: 64 × 512 = 32 KiB, one full [`MB_MAX_BLOCKS`] chunk.
///
/// DMA lands **only** here and is then copied to the caller. Two things follow, and together they
/// are why it exists rather than DMA-ing into the caller's buffer directly:
/// * every caller-alignment and caller-lifetime constraint disappears; and
/// * a straggling DMA write that arrives *after* a transfer was declared timed out lands inside
///   memory this driver owns forever — never in a caller's buffer that has since been freed or
///   reused. The table and the bounce are allocated once at engine init and never freed.
const ADMA2_BOUNCE_BYTES: usize = MB_MAX_BLOCKS as usize * 512;

/// Largest byte count in one descriptor. The length field is 16 bits, and spec 3.00 encodes 65536
/// as a length of 0 — a version-dependent special case this driver simply never relies on. 32 KiB
/// leaves margin on both sides and still covers a whole chunk in one descriptor when the buffer is
/// contiguous and boundary-free.
const ADMA2_MAX_DESC_LEN: usize = 32768;

/// ADMA2 descriptor attributes (spec 3.00 §1.13).
const ADMA2_ATTR_VALID: u16 = 1 << 0;
const ADMA2_ATTR_END: u16 = 1 << 1;
/// Act = 10b: Tran — transfer `length` bytes to/from `address`. No Int attribute is ever set: this
/// driver is polled, and a descriptor interrupt would raise a status bit nothing consumes.
const ADMA2_ATTR_ACT_TRAN: u16 = 0b10 << 4;

/// The 32-bit address ceiling. The ADMA2-32 descriptor address field and `REG_ADMA_ADDR` are both
/// 32 bits wide, so anything at or above this is simply unreachable by the engine — a fact this
/// driver refuses on rather than truncates into a wrong-but-plausible address.
const DMA_CEILING: u64 = 0x1_0000_0000;

/// The byte a DMA landing zone is pre-filled with, so "the engine wrote nothing" is distinguishable
/// from "the engine wrote what was already there".
const DMA_SENTINEL: u8 = 0xA5;

/// Decode ADMA Error Status bits [1:0] — the state the engine was in when it faulted, which is what
/// separates "the descriptor table itself is unreadable" from "a descriptor's data transfer failed".
fn adma_error_state_name(err: u8) -> &'static str {
    match err & 0b11 {
        0b00 => "ST_STOP (stopped between descriptors)",
        0b01 => "ST_FDS (faulted fetching a descriptor)",
        0b10 => "reserved",
        _ => "ST_TFR (faulted transferring a descriptor's data)",
    }
}

// --- DMA-visible memory access ----------------------------------------------------
//
// The bounce buffer and the descriptor table are ordinary Write-Back kernel heap. Everything the
// CPU writes there must actually reach memory, and everything the DEVICE writes there must actually
// be re-read — and the compiler knows about neither party. It sees a region this driver filled with
// a sentinel and no subsequent writer, so a plain read of it is free to be folded back to the
// sentinel. The two helpers below therefore use volatile accesses, which may be neither elided nor
// forwarded across.
//
// CACHE MAINTENANCE: none, and none is needed. x86_64 is I/O-coherent — the CPU caches and the PCIe
// DMA path snoop each other — which is the same contract `drivers::xhci::dma_coherency` documents
// and ships on for every xHCI ring and data buffer in this kernel (on x86_64 its clean/invalidate
// functions are empty bodies that compile to nothing).
//
// ORDERING: zero fence instructions, and this is the reason. Descriptor stores go to WB memory; the
// MMIO writes that follow them (the ADMA System Address, then the Command register that issues the
// transfer) go to the UC BAR window. x86-TSO does not reorder stores with stores, and a UC store
// may not pass earlier stores — so the descriptors are globally visible before the write that makes
// the controller fetch them. That is byte-for-byte the seam the xHCI doorbell already relies on. In
// the other direction the UC read of Interrupt Status is the ordering point: PCIe producer/consumer
// ordering requires a read returning from the device to push the DMA writes it posted ahead of
// itself, so once that read reports Transfer Complete, every byte the engine wrote is in memory.

/// Fill `len` bytes at DMA-visible physical address `phys` with `byte`, volatile.
fn dma_fill(phys: u64, len: usize, byte: u8) {
    let pattern = u64::from_ne_bytes([byte; 8]);
    let words = len / 8;
    for i in 0..words {
        unsafe { core::ptr::write_volatile((phys as *mut u64).add(i), pattern) };
    }
    for i in words * 8..len {
        unsafe { core::ptr::write_volatile((phys as *mut u8).add(i), byte) };
    }
}

/// Copy `dst.len()` bytes out of the DMA-visible region at `phys`, volatile.
fn dma_copy_out(phys: u64, dst: &mut [u8]) {
    let words = dst.len() / 8;
    for i in 0..words {
        let w = unsafe { core::ptr::read_volatile((phys as *const u64).add(i)) };
        dst[i * 8..i * 8 + 8].copy_from_slice(&w.to_ne_bytes());
    }
    for (i, d) in dst.iter_mut().enumerate().skip(words * 8) {
        *d = unsafe { core::ptr::read_volatile((phys as *const u8).add(i)) };
    }
}

/// Write one 8-byte ADMA2-32 descriptor: attr:u16 | length:u16 | address:u32.
fn adma2_write_desc(table: u64, index: usize, attr: u16, len: u16, addr: u32) {
    let p = table + (index * ADMA2_DESC_BYTES) as u64;
    unsafe {
        core::ptr::write_volatile(p as *mut u16, attr);
        core::ptr::write_volatile((p + 2) as *mut u16, len);
        core::ptr::write_volatile((p + 4) as *mut u32, addr);
    }
}

/// Build the descriptor chain covering `total` bytes starting at physical `phys`. Returns the
/// number of descriptors written, or `None` — having printed why — if the span cannot be described.
///
/// The span is split at every 64 KiB physical boundary **and** capped at [`ADMA2_MAX_DESC_LEN`].
/// The first satisfies ADMA2's boundary constraint; the second keeps every length inside the 16-bit
/// field without ever touching the version-dependent "length 0 means 65536" encoding.
///
/// The lengths are summed and checked against `total` before returning. A chain describing fewer
/// bytes than the block count demands is exactly what raises ADMA Length Mismatch on real silicon,
/// and catching it here names the bug instead of reading it back off the wire.
fn adma2_build(table: u64, phys: u64, total: usize) -> Option<usize> {
    if total == 0 {
        return None;
    }
    let mut at = phys;
    let mut left = total;
    let mut n = 0usize;
    let mut sum = 0usize;

    while left > 0 {
        if n >= ADMA2_MAX_DESCS {
            serial_println!(
                "[sdhc-adma] descriptor table would overflow ({} slots) describing {} bytes at \
                 {:#x} — refusing",
                ADMA2_MAX_DESCS, total, phys
            );
            return None;
        }
        // Bytes from `at` up to the next 64 KiB boundary.
        let to_boundary = (0x1_0000 - (at & 0xFFFF)) as usize;
        let this = left.min(to_boundary).min(ADMA2_MAX_DESC_LEN);
        if at + this as u64 > DMA_CEILING {
            serial_println!(
                "[sdhc-adma] descriptor {} would reach {:#x}, past the 32-bit ceiling — refusing",
                n, at + this as u64
            );
            return None;
        }
        let last = this == left;
        let attr = ADMA2_ATTR_VALID | ADMA2_ATTR_ACT_TRAN | if last { ADMA2_ATTR_END } else { 0 };
        adma2_write_desc(table, n, attr, this as u16, at as u32);
        sum += this;
        at += this as u64;
        left -= this;
        n += 1;
    }

    if sum != total {
        serial_println!(
            "[sdhc-adma] descriptor lengths sum to {} but {} were required — refusing",
            sum, total
        );
        return None;
    }
    Some(n)
}

/// Bring the ADMA2 engine up for one identified card, or refuse with a named, printed reason.
///
/// Ordering is deliberate: capability, then memory, then the address refusals, then the DMA-select
/// latch, and **only then** Bus Master. The grant comes last because it is the one step that hands
/// the device a new capability — if anything earlier refuses, the function is left exactly as
/// milestone 2 left it, with no bus-mastering it does not use.
fn adma2_init(card: &mut SdCard, caps: &HostCaps) {
    let (bus, slot, func) = card.bdf;
    let base = card.base;

    fn refuse(card: &mut SdCard, why: &'static str) {
        card.adma = Adma2State::Unavailable(why);
        serial_println!(
            "[sdhc-adma] UNAVAILABLE ({}) — every read is served by the PIO path; a named \
             limitation, never a silent one",
            why
        );
    }

    // --- Capability, read FRESH from this boot's controller. Nothing about ADMA2 is carried over
    // from a previous observation of this machine: the only evidence for the bench part's ADMA2
    // support is an uncommitted bench note, so the register decides, every boot.
    if caps.caps & CAP_ADMA2 == 0 {
        serial_println!("[sdhc-adma] caps={:#010x} adma2-bit=0", caps.caps);
        refuse(card, "controller advertises no ADMA2");
        return;
    }

    // --- Memory. Allocated once, never freed, for the lifetime of the kernel.
    let Ok(table_layout) = core::alloc::Layout::from_size_align(ADMA2_TABLE_BYTES, 4096) else {
        refuse(card, "descriptor-table layout invalid");
        return;
    };
    let Ok(bounce_layout) = core::alloc::Layout::from_size_align(ADMA2_BOUNCE_BYTES, 4096) else {
        refuse(card, "bounce-buffer layout invalid");
        return;
    };
    let table = unsafe { alloc::alloc::alloc_zeroed(table_layout) } as u64;
    let bounce = unsafe { alloc::alloc::alloc_zeroed(bounce_layout) } as u64;
    if table == 0 || bounce == 0 {
        refuse(card, "dma allocation failed");
        return;
    }

    // --- The two address refusals, before the engine can ever be called Ready.
    //
    // (a) Reachability. On x86_64 the kernel heap is identity-mapped physical RAM handed to devices
    //     as physical==bus addresses (`arch::x86_64::memory`, which also prints a HEAP WARNING on
    //     serial if the heap window itself ends above 4 GiB), so `ptr as u64` IS the bus address —
    //     the same seam the xHCI rings use. In ADMA2-32 that address field is 32 bits, so anything
    //     at or past the ceiling is unreachable, and is refused BY NAME rather than truncated into
    //     a valid-looking address pointing at the wrong RAM.
    // (b) Alignment. The descriptor address field's low bits are address bits, not flags, so a
    //     misaligned buffer is a WRONG address, not a slow one. The 4096-byte layouts guarantee it;
    //     it is asserted anyway, because a guarantee that is never checked is an assumption.
    if table + ADMA2_TABLE_BYTES as u64 > DMA_CEILING
        || bounce + ADMA2_BOUNCE_BYTES as u64 > DMA_CEILING
    {
        serial_println!("[sdhc-adma] table={:#x} buf={:#x} ceiling={:#x}", table, bounce, DMA_CEILING);
        refuse(card, "dma window above 4GiB");
        return;
    }
    if table & 0x3 != 0 || bounce & 0x3 != 0 {
        serial_println!("[sdhc-adma] table={:#x} buf={:#x} not 4-byte aligned", table, bounce);
        refuse(card, "dma window misaligned");
        return;
    }

    // --- Host Control 1 DMA Select -> 32-bit ADMA2, read-modify-write so the bus-width, high-speed
    // and LED bits the earlier steps left there are preserved.
    let hc1 = r8(base, REG_HOST_CONTROL1);
    let hc1_want = (hc1 & !HC1_DMA_SELECT_MASK) | HC1_DMA_ADMA2_32;
    w8(base, REG_HOST_CONTROL1, hc1_want);
    let hc1_back = r8(base, REG_HOST_CONTROL1);
    serial_println!(
        "[sdhc-adma] host-control1 {:#04x} -> {:#04x} readback={:#04x} dma-select={:#03b}",
        hc1, hc1_want, hc1_back, (hc1_back & HC1_DMA_SELECT_MASK) >> 3
    );
    if hc1_back & HC1_DMA_SELECT_MASK != HC1_DMA_ADMA2_32 {
        refuse(card, "dma-select did not latch");
        return;
    }

    // --- Bus Master. The first grant this kernel has ever made to this function: milestones 1 and
    // 2 left the bit exactly as the firmware set it because they issued no DMA. It is enabled HERE,
    // at engine init, and deliberately NOT in `claim` — so every log line up to and including the
    // claim line stays directly comparable with a milestone-1 or milestone-2 boot log.
    let cmd_before = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x04) };
    unsafe { crate::arch::pci::write_config_16(bus, slot, func, 0x04, cmd_before | 0x0004) };
    let cmd_after = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x04) };
    serial_println!(
        "[sdhc-adma] bus-master grant bdf {}:{}.{} cmd {:#06x} -> {:#06x} bus-master {} -> {}",
        bus, slot, func, cmd_before, cmd_after,
        (cmd_before >> 2) & 1, (cmd_after >> 2) & 1
    );
    if cmd_after & 0x0004 == 0 {
        refuse(card, "bus-master did not latch");
        return;
    }

    card.adma_table = table;
    card.adma_bounce = bounce;
    card.adma = Adma2State::Ready;
    serial_println!(
        "[sdhc-adma] engine ready bm=1 table={:#x} buf={:#x} descs-max={} mode=adma2-32",
        table, bounce, ADMA2_MAX_DESCS
    );
}

/// Print everything the controller knows about a failed DMA transfer, on one line.
///
/// Raw Interrupt Status, raw ADMA Error Status, the ADMA System Address read BACK from the
/// controller — it advances as the engine walks the table, so it names the descriptor the engine
/// faulted on — and the Auto CMD Error word. Raw first, decoded second, as everywhere else here.
fn adma2_report_failure(base: u64, table: u64, what: &str, lba: u64, count: u16, int: u32) {
    let adma_err = r8(base, REG_ADMA_ERROR);
    let adma_at = r32(base, REG_ADMA_ADDR);
    let acmd_err = r16(base, REG_AUTO_CMD_ERR);
    let desc = if adma_at as u64 >= table {
        ((adma_at as u64 - table) / ADMA2_DESC_BYTES as u64) as i64
    } else {
        -1
    };
    serial_println!(
        "[sdhc-adma] {} lba={} blocks={} FAILED int={:#010x} ({}) adma-err={:#04x} ({}) \
         adma-addr={:#010x} table={:#x} desc={} auto-cmd-err={:#06x}",
        what, lba, count, int, int_error_name(int),
        adma_err, adma_error_state_name(adma_err),
        adma_at, table, desc, acmd_err
    );
}

/// Read `count` blocks at `lba` through the ADMA2 engine into `buf`. Returns (bytes copied, whether
/// the engine actually wrote the bounce buffer).
///
/// The caller must have established that `card.adma` is [`Adma2State::Ready`]. This function never
/// falls back to PIO on its own: a fallback that hides the failure it fell back from is exactly the
/// instrument lie §7.3 forbids, so the decision belongs to the dispatcher that can also print it.
fn read_blocks_adma_on(
    card: &SdCard,
    lba: u64,
    count: u16,
    buf: &mut [u8],
) -> Result<(usize, bool), BlockError> {
    if count == 0 || count > MB_MAX_BLOCKS {
        serial_println!("[sdhc-adma] refusing count={} (bound is 1..={})", count, MB_MAX_BLOCKS);
        return Err(BlockError::Io);
    }
    let n = count as usize * 512;
    if buf.len() < n {
        serial_println!(
            "[sdhc-adma] refusing lba={} blocks={}: caller buffer is {} bytes, {} needed",
            lba, count, buf.len(), n
        );
        return Err(BlockError::Io);
    }
    if lba.checked_add(count as u64).is_none_or(|end| end > card.num_blocks) {
        return Err(BlockError::BadLba);
    }
    if card.adma_table == 0 || card.adma_bounce == 0 {
        return Err(BlockError::NotReady);
    }

    let base = card.base;
    let table = card.adma_table;
    let bounce = card.adma_bounce;
    let arg = lba_arg(card, lba)?;
    let (command, tm, name) = counted_read_cmd(count);

    // Sentinel FIRST. Without it, a transfer where the engine wrote nothing at all would hand back
    // the previous transfer's bytes — or zeros that happen to match a zeroed control — with an `Ok`
    // beside them. With it, "the DMA engine did not write" is a reportable outcome (`wrote=0`)
    // instead of an invisible one.
    dma_fill(bounce, n, DMA_SENTINEL);

    let descs = adma2_build(table, bounce, n).ok_or(BlockError::Io)?;

    w32(base, REG_INT_STATUS, 0xFFFF_FFFF); // W1C stale status
    w32(base, REG_ADMA_ADDR, table as u32); // low half only: ADMA2-32 has no high half
    w16(base, REG_BLOCK_SIZE, 512);
    w16(base, REG_BLOCK_COUNT, count);
    w16(base, REG_TRANSFER_MODE, tm | TM_DMA_EN);

    if let Err(int) = send_command(base, command, arg, true) {
        adma2_report_failure(base, table, name, lba, count, int);
        abort_data_transfer(base);
        return Err(BlockError::Io);
    }
    if let Err(r1) = r1_check(base, "adma2 read") {
        serial_println!(
            "[sdhc-adma] {} lba={} blocks={} rejected by the CARD r1={:#010x}",
            name, lba, count, r1
        );
        abort_data_transfer(base);
        return Err(BlockError::Io);
    }

    // ONE wait for the whole transfer — with DMA there is no per-block CPU rendezvous, which is the
    // entire point of the engine. Budget: the single-block data ceiling plus 2 ms per block (≤328 ms
    // at 64 blocks), in the same rdtsc units as every other wait in this driver.
    let budget = DATA_TIMEOUT_MS + 2 * count as u64;
    if !wait_ms(budget, || {
        r32(base, REG_INT_STATUS) & (INT_XFER_COMPLETE | INT_ERR_ANY) != 0
    }) {
        let int = r32(base, REG_INT_STATUS);
        serial_println!("[sdhc-adma] transfer TIMEOUT after {}ms with {} descriptor(s)", budget, descs);
        adma2_report_failure(base, table, name, lba, count, int);
        abort_data_transfer(base);
        return Err(BlockError::Io);
    }
    // This UC read is also the ordering point in the device->CPU direction: PCIe requires it to push
    // the engine's posted writes ahead of itself, so once it reports Transfer Complete the bounce
    // buffer has settled and no fence is needed before reading it.
    let int = r32(base, REG_INT_STATUS);
    w32(base, REG_INT_STATUS, int); // W1C everything we saw
    if int & INT_ERR_ANY != 0 {
        adma2_report_failure(base, table, name, lba, count, int);
        abort_data_transfer(base);
        return Err(BlockError::Io);
    }

    dma_copy_out(bounce, &mut buf[..n]);
    // Whether the engine wrote anything at all. A card sector legitimately full of the sentinel byte
    // would read as `wrote=0`; that is a stated limit of this indicator, not a hidden one.
    let wrote = buf[..n].iter().any(|&b| b != DMA_SENTINEL);
    Ok((n, wrote))
}

/// [`read_blocks_adma_on`] against the published card.
fn read_blocks_512_adma(lba: u64, count: u16, buf: &mut [u8]) -> Result<(usize, bool), BlockError> {
    let guard = CARD.lock();
    let card = guard.as_ref().ok_or(BlockError::NotReady)?;
    if card.adma != Adma2State::Ready {
        return Err(BlockError::NotReady);
    }
    read_blocks_adma_on(card, lba, count, buf)
}

/// The engine's state, copied out from under the lock.
fn adma_state() -> Option<Adma2State> {
    CARD.lock().as_ref().map(|c| c.adma)
}

/// One in-boot DMA transfer, so a metal log says whether the engine ever ran at all.
///
/// Three readings are distinguishable in the log, which is why this prints a verdict and not a
/// status: `engine ready` + `read … ok wrote=1` (healthy); `UNAVAILABLE (reason)` with no `read`
/// line (the engine never ran, and why); `read … ok wrote=0` (the transfer completed but the engine
/// wrote nothing — the failure a bare `Ok` would have concealed).
fn adma2_smoke(num_blocks: u64) {
    if adma_state() != Some(Adma2State::Ready) {
        // The refusal has already printed its own named line at init; a second, weaker restatement
        // of it here would only compete with the real one.
        return;
    }
    let blocks = VERIFY_WINDOW_BLOCKS;
    if num_blocks < blocks as u64 {
        serial_println!("[sdhc-adma] smoke SKIPPED — card holds {} blocks", num_blocks);
        return;
    }
    let n = blocks * 512;
    let mut mb = MB_BUF.lock();
    match read_blocks_512_adma(0, blocks as u16, &mut mb[..n]) {
        Ok((bytes, wrote)) => serial_println!(
            "[sdhc-adma] read lba=0 blocks={} ok wrote={} bytes={}",
            blocks, wrote as u8, bytes
        ),
        Err(e) => serial_println!(
            "[sdhc-adma] read lba=0 blocks={} FAILED ({:?}) — reason named on the line above",
            blocks, e
        ),
    }
}

// ===================================================================================
// Step 4b — proving the block is really the card's
// ===================================================================================

/// Scratch sector for the bring-up verification. A static rather than a stack array so a 512-byte
/// buffer can never be a factor in an early-boot stack budget.
static VERIFY_BUF: Mutex<[u8; 512]> = Mutex::new([0u8; 512]);

/// FNV-1a over a sector — a compact content fingerprint for the comparisons below. Not a checksum
/// with any integrity claim; its only job is to make "same bytes" and "different bytes" printable.
fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// Read LBA 0 and prove, as far as one boot can, that the bytes came from the card.
///
/// Three independent claims, because no single one of them is sufficient:
///
/// 1. **LBA 0 read twice fingerprints identically.** Fails if the FIFO is returning residue or the
///    drain is desynchronised. Does NOT prove the bytes came from the addressed block.
/// 2. **LBA 1 fingerprints differently.** Fails if the address argument never reaches the card and
///    every read serves the same buffer — the failure mode claim (1) is blind to. (A card whose
///    first two sectors genuinely hold identical bytes would also report this; that is why it is
///    reported as an observation, not asserted as a pass.)
/// 3. **The MBR cross-check.** If LBA 0 carries the 55 AA boot signature, its partition table is
///    an independently-authored description of the SAME card, written by whatever formatted it. Its
///    partition extents must fit inside the capacity this driver derived from the CSD. An extent
///    that runs past that capacity convicts the CSD parse or the addressing mode — the two things
///    that are otherwise unfalsifiable from inside one boot.
///
/// A card with no MBR signature simply cannot supply claim 3, and that is reported as a limit of
/// the evidence rather than passed over.
fn verify_read(bus: u8, slot: u8, func: u8, num_blocks: u64) {
    let mut buf = VERIFY_BUF.lock();

    // --- Claim 1a: read LBA 0.
    match read_block_512(0, &mut buf[..]) {
        Ok(n) => serial_println!("[sdhc] read lba0 ok ({} bytes)", n),
        Err(e) => {
            serial_println!("[sdhc] read lba0 FAILED ({:?}) — no block was transferred", e);
            return;
        }
    }
    let h0 = fnv1a(&buf[..]);
    let sig = [buf[510], buf[511]];
    // RAW before decoded: the first 16 bytes of the sector as the card delivered them.
    serial_println!(
        "[sdhc] lba0 head={:02x}{:02x}{:02x}{:02x} {:02x}{:02x}{:02x}{:02x} {:02x}{:02x}{:02x}{:02x} {:02x}{:02x}{:02x}{:02x} sig=[{:#04x},{:#04x}] fnv={:#018x}",
        buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
        buf[8], buf[9], buf[10], buf[11], buf[12], buf[13], buf[14], buf[15],
        sig[0], sig[1], h0
    );

    // Snapshot the partition table before the buffer is reused by the repeat reads.
    let mut parts = [(0u8, 0u32, 0u32); 4];
    for (i, p) in parts.iter_mut().enumerate() {
        let o = 446 + i * 16;
        let ptype = buf[o + 4];
        let start = u32::from_le_bytes([buf[o + 8], buf[o + 9], buf[o + 10], buf[o + 11]]);
        let count = u32::from_le_bytes([buf[o + 12], buf[o + 13], buf[o + 14], buf[o + 15]]);
        *p = (ptype, start, count);
    }

    // --- Claim 1b: the same block again.
    let h0_again = match read_block_512(0, &mut buf[..]) {
        Ok(_) => fnv1a(&buf[..]),
        Err(e) => {
            serial_println!("[sdhc] re-read lba0 FAILED ({:?})", e);
            return;
        }
    };
    serial_println!(
        "[sdhc] verify repeat lba0 fnv={:#018x} match={} (a mismatch means FIFO residue or a \
         desynchronised drain, not a bad card)",
        h0_again, (h0_again == h0) as u8
    );

    // --- Claim 2: a different block.
    if num_blocks > 1 {
        match read_block_512(1, &mut buf[..]) {
            Ok(_) => {
                let h1 = fnv1a(&buf[..]);
                serial_println!(
                    "[sdhc] verify lba1 fnv={:#018x} differs-from-lba0={} (if 0, the address \
                     argument may not be reaching the card)",
                    h1, (h1 != h0) as u8
                );
            }
            Err(e) => serial_println!("[sdhc] read lba1 FAILED ({:?})", e),
        }
    }

    // --- Claim 3: the MBR cross-check.
    if sig != [0x55, 0xAA] {
        serial_println!(
            "[sdhc] verify mbr: no 55 aa boot signature at offset 510 (sig=[{:#04x},{:#04x}]) — the \
             card is unpartitioned or superfloppy-formatted, so the capacity cross-check is \
             UNAVAILABLE on this card, not failed",
            sig[0], sig[1]
        );
        return;
    }
    let mut any = false;
    let mut all_fit = true;
    for (i, &(ptype, start, count)) in parts.iter().enumerate() {
        if ptype == 0 || count == 0 {
            continue;
        }
        any = true;
        let end = start as u64 + count as u64;
        let fits = end <= num_blocks;
        all_fit &= fits;
        serial_println!(
            "[sdhc] verify mbr p{} type={:#04x} start={} count={} end={} fits-capacity={}",
            i, ptype, start, count, end, fits as u8
        );
    }
    if !any {
        serial_println!(
            "[sdhc] verify mbr: boot signature present but the partition table is empty — no \
             capacity cross-check available"
        );
        return;
    }
    serial_println!(
        "[sdhc] verify mbr bdf {}:{}.{}: partition extents vs CSD capacity {} blocks -> all-fit={} \
         ({})",
        bus, slot, func, num_blocks, all_fit as u8,
        if all_fit {
            "an independently-authored description of this card agrees with the CSD parse and the \
             addressing mode"
        } else {
            "an extent runs past the derived capacity — the CSD parse or the addressing mode is WRONG"
        }
    );
}

// ===================================================================================
// Step 5b — the multi-block witness
// ===================================================================================

/// Blocks per comparison window. 8 × 512 = 4 KiB: enough that a per-block desynchronisation shows
/// up, small enough that a failing window prints one line rather than a wall.
const VERIFY_WINDOW_BLOCKS: usize = 8;

/// The control buffer for a comparison window — filled block by block through the UNMODIFIED
/// single-block [`read_block_512`], which is what makes it a control.
static MB_CTL_BUF: Mutex<[u8; VERIFY_WINDOW_BLOCKS * 512]> =
    Mutex::new([0u8; VERIFY_WINDOW_BLOCKS * 512]);

/// The buffer a multi-block transfer lands in. Sized for a full [`MB_MAX_BLOCKS`] chunk so the
/// witness never has to be the reason a transfer is short. Static rather than stack, so 32 KiB can
/// never be a factor in an early-boot stack budget.
static MB_BUF: Mutex<[u8; MB_MAX_BLOCKS as usize * 512]> =
    Mutex::new([0u8; MB_MAX_BLOCKS as usize * 512]);

/// First differing (block index, byte offset within that block) between two equal-length windows,
/// or `None` if they are identical. The EVIDENCE, not a summary of it: a fingerprint mismatch says
/// only "different", while an offset says whether the transfer lost a block boundary (offset 0 of
/// some block), a word (offset ≡ 0 mod 4), or a byte.
fn first_difference(a: &[u8], b: &[u8]) -> Option<(usize, usize)> {
    for (i, (&x, &y)) in a.iter().zip(b.iter()).enumerate() {
        if x != y {
            return Some((i / 512, i % 512));
        }
    }
    None
}

/// Read one window twice — once as `VERIFY_WINDOW_BLOCKS` separate CMD17s, once as a single CMD18 —
/// and compare the two byte for byte.
///
/// The single-block path is the control, and it is the control precisely because nothing in
/// milestone 3 touches it: [`read_block_512`] is byte-for-byte the function metal verified in
/// milestone 2. So a `match=1` here is not "two runs of the same code agreed" — it is "a transfer
/// whose command semantics are new agreed with a transfer whose semantics are already trusted".
///
/// Reading the log: a healthy card prints `match=1`. If CMD18 never ran at all, the ladder stops at
/// a named `[sdhc-mb] cmd18 … FAILED` line and **no `match=` line exists** — the two outcomes are
/// distinguishable in the log, which is the whole point of printing a verdict rather than a status.
fn verify_multiblock(num_blocks: u64) {
    let blocks = VERIFY_WINDOW_BLOCKS;
    if num_blocks < blocks as u64 {
        serial_println!(
            "[sdhc-mb] SKIPPED — card holds {} blocks, fewer than the {}-block window",
            num_blocks, blocks
        );
        return;
    }
    let n = blocks * 512;
    let mut ctl = MB_CTL_BUF.lock();
    let mut mb = MB_BUF.lock();

    for b in 0..blocks {
        let off = b * 512;
        if let Err(e) = read_block_512(b as u64, &mut ctl[off..off + 512]) {
            serial_println!(
                "[sdhc-mb] control cmd17 lba={} FAILED ({:?}) — the control is unavailable, so no \
                 comparison is possible; NOT a multi-block verdict",
                b, e
            );
            return;
        }
    }
    let ctl_fnv = fnv1a(&ctl[..n]);

    // Sentinel-fill so "the transfer wrote nothing" cannot masquerade as "the transfer matched" —
    // a zeroed buffer compared against a zeroed control would.
    mb[..n].fill(0xA5);
    if let Err(e) = read_blocks_512_pio(0, blocks as u16, &mut mb[..n]) {
        serial_println!(
            "[sdhc-mb] window lba=0 blocks={} cmd18 FAILED ({:?}) — reason named on the line above",
            blocks, e
        );
        return;
    }
    let mb_fnv = fnv1a(&mb[..n]);

    match first_difference(&ctl[..n], &mb[..n]) {
        None => serial_println!(
            "[sdhc-mb] window lba=0 blocks={} match=1 first-diff=none ctl-fnv={:#018x} mb-fnv={:#018x}",
            blocks, ctl_fnv, mb_fnv
        ),
        Some((blk, off)) => serial_println!(
            "[sdhc-mb] window lba=0 blocks={} match=0 first-diff=blk{},off{} ctl={:#04x} mb={:#04x} \
             ctl-fnv={:#018x} mb-fnv={:#018x}",
            blocks, blk, off, ctl[blk * 512 + off], mb[blk * 512 + off], ctl_fnv, mb_fnv
        ),
    }
}

// ===================================================================================
// Step 7 — the A/B verdict and the fallback policy (SDHC-3c)
// ===================================================================================

/// Read one window twice — once through the untouched single-block PIO control, once through the
/// ADMA2 engine — and compare the two byte for byte. Returns (matched, the engine wrote), or `None`
/// if the comparison could not be made at all.
fn ab_window(lba: u64) -> Option<(bool, bool)> {
    let blocks = VERIFY_WINDOW_BLOCKS;
    let n = blocks * 512;
    let mut ctl = MB_CTL_BUF.lock();
    let mut mb = MB_BUF.lock();

    for b in 0..blocks {
        let off = b * 512;
        if let Err(e) = read_block_512(lba + b as u64, &mut ctl[off..off + 512]) {
            serial_println!(
                "[sdhc-ab] window lba={} control cmd17 lba={} FAILED ({:?}) — no control, so no \
                 comparison; this is NOT an adma2 verdict",
                lba, lba + b as u64, e
            );
            return None;
        }
    }

    let (_, wrote) = match read_blocks_512_adma(lba, blocks as u16, &mut mb[..n]) {
        Ok(v) => v,
        Err(e) => {
            serial_println!(
                "[sdhc-ab] window lba={} blocks={} adma2 read FAILED ({:?}) — reason named above",
                lba, blocks, e
            );
            return None;
        }
    };

    match first_difference(&ctl[..n], &mb[..n]) {
        None => {
            serial_println!(
                "[sdhc-ab] window lba={} blocks={} match=1 wrote={} first-diff=none",
                lba, blocks, wrote as u8
            );
            Some((true, wrote))
        }
        Some((blk, off)) => {
            // The evidence, not a summary of it. `off0` of a block means a lost block boundary; an
            // offset that is 0 mod 4 means a lost word; anything else, a lost byte.
            serial_println!(
                "[sdhc-ab] window lba={} blocks={} match=0 wrote={} first-diff=blk{},off{} \
                 (absolute lba={} byte={}) ctl={:#04x} adma={:#04x}",
                lba, blocks, wrote as u8, blk, off, lba + blk as u64, off,
                ctl[blk * 512 + off], mb[blk * 512 + off]
            );
            Some((false, wrote))
        }
    }
}

/// The A/B verdict: three windows across the card, each read by both paths and compared.
///
/// Three windows and not one, because a DMA engine that is wrong only for some addresses is a real
/// failure mode — a descriptor built from a truncated or mis-shifted address reads the right bytes
/// near the start of the card and the wrong ones further in. Head, middle and tail sample that.
///
/// If ADMA2 is unavailable this prints `SKIPPED` with the reason. It does not vanish: a witness that
/// disappears when the thing it measures is absent is indistinguishable from a witness that was
/// never called.
fn verify_adma_ab(num_blocks: u64) {
    let Some(state) = adma_state() else {
        return;
    };
    if state != Adma2State::Ready {
        serial_println!("[sdhc-ab] SKIPPED — adma2 unavailable ({})", state.reason());
        return;
    }
    let blocks = VERIFY_WINDOW_BLOCKS as u64;
    if num_blocks < blocks * 2 {
        serial_println!(
            "[sdhc-ab] SKIPPED — card holds {} blocks, too few for three distinct {}-block windows",
            num_blocks, blocks
        );
        return;
    }

    // Head, middle, tail. `num_blocks >= 2*blocks` makes the middle window fit by construction.
    let windows = [0u64, num_blocks / 2, num_blocks - blocks];
    let mut matched = 0usize;
    let mut compared = 0usize;
    for &w in windows.iter() {
        match ab_window(w) {
            Some((true, _)) => {
                compared += 1;
                matched += 1;
            }
            Some((false, _)) => compared += 1,
            None => {}
        }
    }

    serial_println!(
        "[sdhc-ab] verdict windows={} match={}/{} — {}",
        windows.len(), matched, windows.len(),
        if matched == windows.len() {
            "adma2 agrees with the pio control byte-for-byte"
        } else if compared == 0 {
            "NO window could be compared; this boot carries no adma2 verdict either way"
        } else {
            "adma2 DISAGREES with the pio control — the offsets above are the evidence"
        }
    );
}

/// Read `count` contiguous 512-byte blocks starting at `lba` into `buf`. The module's public
/// counted read: ADMA2 when the engine is Ready, PIO otherwise, chunked at [`MB_MAX_BLOCKS`].
///
/// **The fallback is loud, and that is the point.** When a live ADMA2 transfer fails, three things
/// are printed before this function can return success: the transfer's own failure line (raw status,
/// ADMA error state, faulting descriptor), the one-time `engine DISABLED` line naming the reason,
/// and the PIO retry's own result. A driver that quietly retried on PIO and returned `Ok` would
/// report a healthy card while its DMA engine was dead — the instrument lie this project has paid
/// for more than once. Success is never reported without the failure that preceded it on the wire.
///
/// The engine is latched `Faulted` on the first failure, so it is disabled for the rest of the boot
/// rather than re-failing (and re-printing) on every subsequent read.
///
/// Deliberately NOT published into `drivers::block::BLOCK_DEVICE` — see [`card_num_blocks`].
pub fn read_blocks_512(lba: u64, count: usize, buf: &mut [u8]) -> Result<usize, BlockError> {
    if count == 0 {
        return Ok(0);
    }
    let need = count.checked_mul(512).ok_or(BlockError::BadLba)?;
    if buf.len() < need {
        serial_println!(
            "[sdhc] read_blocks_512 lba={} count={}: caller buffer is {} bytes, {} needed",
            lba, count, buf.len(), need
        );
        return Err(BlockError::Io);
    }

    let mut guard = CARD.lock();
    let card = guard.as_mut().ok_or(BlockError::NotReady)?;
    if lba.checked_add(count as u64).is_none_or(|end| end > card.num_blocks) {
        return Err(BlockError::BadLba);
    }

    let mut done = 0usize;
    while done < count {
        let this = (count - done).min(MB_MAX_BLOCKS as usize) as u16;
        let at = lba + done as u64;
        let dst = &mut buf[done * 512..(done + this as usize) * 512];

        if card.adma == Adma2State::Ready {
            match read_blocks_adma_on(card, at, this, dst) {
                Ok(_) => {
                    done += this as usize;
                    continue;
                }
                Err(e) => {
                    // `read_blocks_adma_on` has already printed the raw evidence. Latch, announce,
                    // and fall through to the PIO retry below — never silently.
                    card.adma = Adma2State::Faulted("a live adma2 transfer failed");
                    serial_println!(
                        "[sdhc-adma] engine DISABLED — falling back to PIO ({}) after {:?} at \
                         lba={} blocks={}; every later read on this boot is PIO",
                        card.adma.reason(), e, at, this
                    );
                }
            }
        }

        match read_blocks_pio_on(card, at, this, dst) {
            Ok(_) => {
                if card.adma != Adma2State::Ready {
                    serial_println!("[sdhc-mb] pio lba={} blocks={} ok ({})", at, this, card.adma.reason());
                }
                done += this as usize;
            }
            Err(e) => {
                serial_println!("[sdhc-mb] pio lba={} blocks={} FAILED ({:?})", at, this, e);
                return Err(e);
            }
        }
    }
    Ok(need)
}

// ===================================================================================
// Bring-up
// ===================================================================================

/// Bring one claimed controller up to a clocked, powered bus with a debounced card-detect reading.
/// Every failure path has already logged its own named reason; this returns simply whether the
/// controller reached that state.
fn bring_up(base: u64, bus: u8, slot: u8, func: u8) -> bool {
    let caps = survey(base, bus, slot, func);

    if !reset_all(base) {
        return false;
    }
    arm_polled_status(base);
    if set_power(base, &caps).is_none() {
        return false;
    }
    if set_clock(base, &caps, CLK_IDENT_HZ).is_none() {
        return false;
    }
    let Some(inserted) = wait_card_detect(base) else {
        return false;
    };
    if !inserted {
        serial_println!(
            "[sdhc] bdf {}:{}.{} bus is up (powered, clocked) but NO CARD is inserted — nothing to identify",
            bus, slot, func
        );
        return false;
    }

    serial_println!(
        "[sdhc] bdf {}:{}.{} bus ready: powered, {}Hz identification clock, card present",
        bus, slot, func, CLK_IDENT_HZ
    );

    let Some(mut card) = identify(base, &caps, (bus, slot, func)) else {
        return false;
    };
    serial_println!(
        "[sdhc] bdf {}:{}.{} CARD IDENTIFIED — {} blocks, {}-addressed, csd v{}",
        bus, slot, func, card.num_blocks,
        if card.block_addressing { "block" } else { "byte" },
        if card.csd_structure == 1 { 2 } else { 1 }
    );
    let num_blocks = card.num_blocks;

    // SDHC-3b: the DMA engine, AFTER identification (it needs a card in transfer state to be worth
    // anything) and BEFORE publication (so the card is never visible with a half-built engine).
    // Refusal is not failure: an unavailable engine names itself and the PIO path serves every read.
    adma2_init(&mut card, &caps);
    *CARD.lock() = Some(card);

    // Verification goes through the PUBLIC read path, after the card is published, so what the
    // witness exercises is exactly what a later caller would get — not a private shortcut.
    verify_read(bus, slot, func, num_blocks);

    // SDHC-3a: multi-block command semantics, measured against that same public single-block path.
    verify_multiblock(num_blocks);
    // SDHC-3b: one DMA transfer, so the log says whether the engine ever ran at all...
    adma2_smoke(num_blocks);
    // ...and SDHC-3c: whether what it moved is what the card holds, at three places on the card.
    verify_adma_ab(num_blocks);
    true
}

/// SDHC-2: inventory the storage-class PCI functions, then claim and bring up the SD Host
/// Controller. Milestone 1's discovery witness is emitted first and unchanged.
///
/// Only the FIRST controller that claims successfully is driven; any further ones are inventoried
/// and named but left exactly as the firmware set them.
pub fn probe() {
    let controllers = PciScanner::storage_inventory();
    if controllers.is_empty() {
        serial_println!("[sdhc] no SD host controller found (class 0x08/0x05)");
        return;
    }

    let mut driven = false;
    for (bus, slot, func) in controllers {
        if driven {
            serial_println!(
                "[sdhc] bdf {}:{}.{} additional SD host controller — left untouched (milestone 2 drives one)",
                bus, slot, func
            );
            continue;
        }
        let Some(base) = claim(bus, slot, func) else {
            continue;
        };
        driven = true;
        bring_up(base, bus, slot, func);
    }
}
