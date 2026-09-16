// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AHCI — the Serial ATA host controller (`UNAOS_AHCI=1`, default OFF; writes need `ahci-write`).
//!
//! # SCOPE — what this arc does, and the three things it deliberately does NOT do
//!
//! **Does:** find the AHCI controller by PCI class (0x01 mass-storage / 0x06 SATA / progif 0x01),
//! take it from firmware through the BIOS/OS handoff when the controller advertises one, map its
//! ABAR uncacheable, read `CAP`/`CAP2`/`PI`/`VS`, and for every implemented port that reports a
//! plain SATA disk (`PxSSTS.DET == 3` and `PxSIG == 0x0000_0101`) bring the port up, run
//! IDENTIFY DEVICE (ATA `0xEC`), read LBA 0 with READ DMA EXT (ATA `0x25`), and publish the disk
//! into `drivers::block`'s registry as a READ source.
//!
//! **Does NOT — 1: the opcode census is EXACTLY TWO without `ahci-write`, EXACTLY THREE with it.**
//! Off: `IDENTIFY DEVICE` (0xEC) and `READ DMA EXT` (0x25), and no writing opcode exists anywhere in
//! the image. On (AHCIWRITE): `WRITE DMA EXT` (0x35) joins them in [`write_block_at`] at this file's
//! tail — and its ONLY caller is `block::write_sectors_granted`, which demands a `block::WriteGrant`
//! the installer's `PartitionTarget` mints after its census. The plain block-layer write twin in
//! `drivers/block.rs` keeps refusing in BOTH polarities. Prove the polarity on the artifact, never
//! here: `LC_ALL=C grep -a -o -F 'WRITE-DMA-EXT-0x35' <elf>` is 1 hit armed and 0 hits unarmed.
//!
//! **Does NOT — 2: no installer reaches this device except through a grant.** Unarmed, `install/`'s
//! two `Ahci` arms both refuse (rmbp-ledger B91: the bench rMBP's internal SSD carries a live
//! Catalina). Armed, the READ arm opens so the pre-flight census can look before anything writes,
//! the WRITE arm stays a refusal, and the only write path is `PartitionTarget`'s granted one, bound
//! to one partition's LBA range by a capability minted after every refusal in the ladder passed.
//!
//! **Does NOT — 3: no interrupts.** `GHC.IE` and every `PxIE` stay 0; command completion is polled
//! on `PxCI`/`PxIS` against a TSC deadline, the same bounded-budget discipline the xHCI BOT pump
//! uses. Everything this module does happens inside `pci::init`, so it adds no work to any service
//! pass and takes no lock in an input band.
//!
//! # Why this exists
//!
//! rmbp-ledger B89, first rung: the rMBP announces both of its disks at every boot — the PCI
//! census has printed `class=01 sub=06 progif=01 (sata)` on every capture this project has taken —
//! and the OS binds to neither, because there is no SATA driver. The loader can already boot from
//! an internal FAT partition; the kernel then cannot find its root, because `fs/bootdisk.rs` walks
//! block sources and there was never a source on that controller. This arc gives it one.
//!
//! # Arch neutrality
//!
//! The module is declared `#[cfg(all(target_arch = "x86_64", feature = "ahci"))]` in `drivers/mod.rs`
//! because its enumeration hook is PCI config space, which on this tree is an x86 seam
//! (`arch::pci::read_config_*`). The FILE is deliberately arch-neutral in name and shape — AHCI
//! exists on aarch64 boards too, and a future aarch64 arm needs only a different way to reach the
//! ABAR: everything from [`hba_take`] downwards is MMIO and ATA, not x86. Nothing here is named for
//! a board (LAWS §3, ONE OS).
//!
//! # Byte identity
//!
//! Knob off, this file is not lexed at all (the `pub mod` declaration is `#[cfg]`-erased, the one
//! case LAWS §5 names as safe for a module), the block-layer arms do not exist, and the call site in
//! `arch/x86_64/pci.rs` is a LINE-NEUTRAL append that vanishes. `arroyo`'s `arm_features` strips
//! `ahci` from aarch64 media so the feature cannot shift an aarch64 `-Cmetadata` fingerprint.
//!
//! # Registers touched
//!
//! Generic host control: `CAP` (0x00), `GHC` (0x04, AE/HR bits), `IS` (0x08), `PI` (0x0C),
//! `VS` (0x10), `CAP2` (0x24), `BOHC` (0x28). Per port at `0x100 + port*0x80`: `PxCLB`/`PxCLBU`
//! (0x00/0x04), `PxFB`/`PxFBU` (0x08/0x0C), `PxIS` (0x10), `PxIE` (0x14, written 0 only),
//! `PxCMD` (0x18), `PxTFD` (0x20), `PxSIG` (0x24), `PxSSTS` (0x28), `PxSERR` (0x30),
//! `PxSACT` (0x34), `PxCI` (0x38). Source: the AHCI 1.3.1 specification and the Serial ATA
//! specification — cleanroom, no driver code from any other operating system was consulted.
//!
//! # The witness lines
//!
//! ```text
//! :: AHCI: port=<p> model="<m>" sectors=<n> lba48=<0|1> ::
//! :: AHCI: port=<p> sector0 sig=<hex> kind=MBR|GPT|none ::
//! :: AHCI: selfcheck port=<p> identify=<ok|bad> sector0=<GPT|MBR|none> -> PASS|FAIL ::
//! ```
//!
//! The selfcheck line is printed ONLY for a port that reached IDENTIFY — it can execute in every
//! state it reports on, so its silence means "no SATA disk answered", never "the check passed". A
//! `-> FAIL` on it reds any spec replay by `mbench`'s `DEFAULT_FORBIDS`, which is what makes the
//! go-red mutation (corrupt the `PxSIG` gate or the IDENTIFY decode) a measured leg rather than a
//! read of the source.
//!
//! # Next arc
//!
//! Landed: PARTINSTALL (the partition-mode installer) and AHCIWRITE (`WRITE DMA EXT` behind
//! `ahci-write`). Next is multi-sector PRDT runs and the attended operator flow on real metal.

use spin::Mutex;

// ── AHCI 1.3.1 §3.1: Generic Host Control ───────────────────────────────────────────────────────
const REG_CAP: u64 = 0x00;
const REG_GHC: u64 = 0x04;
const REG_PI: u64 = 0x0C;
const REG_VS: u64 = 0x10;
const REG_CAP2: u64 = 0x24;
const REG_BOHC: u64 = 0x28;

const GHC_HR: u32 = 1 << 0;
const GHC_AE: u32 = 1 << 31;

const CAP_SSS: u32 = 1 << 27; // Supports Staggered Spin-up
const CAP2_BOH: u32 = 1 << 0; // Supports BIOS/OS Handoff

const BOHC_BOS: u32 = 1 << 0; // BIOS Owned Semaphore
const BOHC_OOS: u32 = 1 << 1; // OS Owned Semaphore
const BOHC_BB: u32 = 1 << 4; // BIOS Busy

// ── AHCI 1.3.1 §3.3: Port registers, at 0x100 + port * 0x80 ─────────────────────────────────────
const PORT_BASE: u64 = 0x100;
const PORT_STRIDE: u64 = 0x80;

const P_CLB: u64 = 0x00;
const P_CLBU: u64 = 0x04;
const P_FB: u64 = 0x08;
const P_FBU: u64 = 0x0C;
const P_IS: u64 = 0x10;
const P_IE: u64 = 0x14;
const P_CMD: u64 = 0x18;
const P_TFD: u64 = 0x20;
const P_SIG: u64 = 0x24;
const P_SSTS: u64 = 0x28;
const P_SERR: u64 = 0x30;
const P_SACT: u64 = 0x34;
const P_CI: u64 = 0x38;

const PCMD_ST: u32 = 1 << 0;
const PCMD_SUD: u32 = 1 << 1;
const PCMD_FRE: u32 = 1 << 4;
const PCMD_FR: u32 = 1 << 14;
const PCMD_CR: u32 = 1 << 15;

const TFD_ERR: u32 = 1 << 0;
const TFD_DRQ: u32 = 1 << 3;
const TFD_BSY: u32 = 1 << 7;

const PIS_TFES: u32 = 1 << 30; // Task File Error Status

/// Serial ATA: `PxSIG` for a plain SATA disk. An ATAPI device signs `0xEB14_0101`, an enclosure
/// service `0xC33C_0101`, a port multiplier `0x9669_0101` — this arc claims only the first.
const SIG_SATA_DISK: u32 = 0x0000_0101;

/// `PxSSTS.DET` value 3 — device present AND phy communication established. 1 is "presence detected
/// but no communication", which is a link that never trained and is NOT a disk we can talk to.
const DET_PRESENT: u32 = 3;

// ── ATA opcodes. THE COMPLETE SET THIS FILE CAN ISSUE — see the SCOPE block. ─────────────────────
const ATA_IDENTIFY_DEVICE: u8 = 0xEC;
const ATA_READ_DMA_EXT: u8 = 0x25;

/// AHCI 1.3.1 §3.3.1: `PxCLB` must be 1 KiB aligned and the list is 32 headers of 32 bytes.
const CMDLIST_BYTES: usize = 32 * 32;
const CMDLIST_ALIGN: usize = 1024;
/// AHCI 1.3.1 §3.3.3: the received-FIS structure is 256 bytes, 256-byte aligned.
const FIS_BYTES: usize = 256;
const FIS_ALIGN: usize = 256;
/// One command table: 64-byte CFIS + 16-byte ACMD + 48 reserved = 0x80, then the PRDT. One PRDT
/// entry (16 bytes) is all a single-sector transfer needs; 256 bytes is the aligned round-up and
/// leaves room for the eight entries a later multi-sector arc would want.
const CMDTBL_BYTES: usize = 256;
const CMDTBL_ALIGN: usize = 128;

const SECTOR_BYTES: usize = 512;

/// The AHCI port index space is 32 wide (`PI` is a 32-bit mask).
const MAX_PORTS: usize = 32;

/// How many SATA disks this arc will bring up and publish. Four mirrors `block::MAX_USB_DISKS`; the
/// bench rMBP has two (the internal SSD and the optical-bay disk B89 names), and QEMU q35 exposes
/// six AHCI ports of which the fixture uses two.
pub const MAX_AHCI_DISKS: usize = 4;

// ── Bounded waits, in TSC units. Same construction as `drivers/sdhc.rs`: `arch::apic::tsc_hz()` is
// calibrated against the ACPI PM timer long before `pci::init` runs, and the uncalibrated fallback
// derives from `arch::HW_WAIT_BUDGET` (the ~2 s pre-calibration guess). `arch::ms()` is NOT usable
// here — the APIC tick does not advance with interrupts masked, and this runs inside `pci::init`.
#[inline]
fn cycles_ms(ms: u64) -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    if hz != 0 {
        hz.saturating_mul(ms) / 1000
    } else {
        crate::arch::HW_WAIT_BUDGET.saturating_mul(ms) / 2000
    }
}

/// Spin until `pred()` holds or `ms` milliseconds elapse. `wrapping_sub` so a 64-bit TSC wrap
/// mid-wait cannot trip the deadline early.
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

/// Port state-machine transitions (`CR`/`FR` following `ST`/`FRE`) are specified to complete in
/// 500 ms (AHCI 1.3.1 §10.1.2).
const T_PORT_MS: u64 = 500;
/// A command's completion budget. IDENTIFY and a single-sector read are microsecond-scale on any
/// healthy device; a second is a deadline, not an expectation.
const T_CMD_MS: u64 = 1000;
/// How long to wait for a link to train after a staggered spin-up.
const T_LINK_MS: u64 = 1000;

// ── MMIO. `abar` is the identity-mapped physical ABAR; the window is mapped Uncacheable, so no
// fence is needed between accesses on x86 (UC is strongly ordered).
#[inline]
fn r32(abar: u64, off: u64) -> u32 {
    unsafe { core::ptr::read_volatile((abar + off) as *const u32) }
}
#[inline]
fn w32(abar: u64, off: u64, val: u32) {
    unsafe { core::ptr::write_volatile((abar + off) as *mut u32, val) }
}
#[inline]
fn pr32(abar: u64, port: u8, off: u64) -> u32 {
    r32(abar, PORT_BASE + (port as u64) * PORT_STRIDE + off)
}
#[inline]
fn pw32(abar: u64, port: u8, off: u64, val: u32) {
    w32(abar, PORT_BASE + (port as u64) * PORT_STRIDE + off, val)
}

/// One brought-up SATA port's DMA structures and geometry.
///
/// Every address here is a HEAP address used directly as a bus address. That is this kernel's
/// standing x86 property, not an assumption this driver introduces: the identity map means VA == PA
/// and every DMA structure in `drivers/xhci/mod.rs` (DCBAA, the scratchpad array, every ring and
/// every BOT staging buffer) is programmed into a controller the same way. [`bus_addr`] is the one
/// place that conversion happens, so a future aarch64 arm has one function to change.
#[derive(Clone, Copy)]
struct AhciPort {
    abar: u64,
    port: u8,
    /// Command list base — 32 headers of 32 bytes, 1 KiB aligned.
    clb: u64,
    /// Command table for slot 0 — CFIS at +0x00, PRDT at +0x80.
    ctba: u64,
    /// The single-sector DMA landing buffer this port's reads target.
    dma: u64,
    num_sectors: u64,
    lba48: bool,
}

static PORTS: Mutex<[Option<AhciPort>; MAX_AHCI_DISKS]> = Mutex::new([None; MAX_AHCI_DISKS]);

/// The one place a heap pointer becomes a bus address. See [`AhciPort`].
#[inline]
fn bus_addr(p: u64) -> u64 {
    p
}

/// Allocate zeroed, aligned DMA-visible memory. Returns 0 on allocation failure, which every caller
/// treats as "this port does not come up" rather than panicking — a boot that cannot spare 1.5 KiB
/// has a bigger problem than a missing disk, and refusing loudly is better than aborting the boot.
///
/// Deliberately never freed: the structures stay live for the whole boot because the controller
/// keeps DMAing into the FIS receive area as long as `PxCMD.FRE` is set. Handing this memory back
/// to the allocator while the HBA still owns it is the one class of bug a port teardown must get
/// right, and this arc has no teardown at all.
fn dma_alloc(size: usize, align: usize) -> u64 {
    let layout = match core::alloc::Layout::from_size_align(size, align) {
        Ok(l) => l,
        Err(_) => return 0,
    };
    let p = unsafe { alloc::alloc::alloc_zeroed(layout) };
    p as u64
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// Step 1 — find the controller and claim it
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// Walk PCI for the first function reporting class 0x01 / subclass 0x06 / progif 0x01 (AHCI 1.0).
///
/// Class-based, never bdf-based: LAWS §3 forbids a bus or slot literal in kernel source, and the
/// same walk therefore finds the controller on the bench rMBP, on QEMU's q35 and on any other
/// machine. Bounded by the same 256x32 sweep the other targeted walks in this kernel run, and
/// multi-function-gated on function 0's header type so a single-function device is never probed at
/// functions 1..7 (which is architecturally undefined and, on some silicon, aliases function 0).
fn find_controller() -> Option<(u8, u8, u8, u16, u16)> {
    for bus in 0u16..256 {
        for slot in 0u8..32 {
            let v0 = unsafe { crate::arch::pci::read_config_16(bus as u8, slot, 0, 0x00) };
            if v0 == 0xFFFF {
                continue;
            }
            let hdr0 = unsafe { crate::arch::pci::read_config_32(bus as u8, slot, 0, 0x0C) };
            let max_func: u8 = if ((hdr0 >> 16) & 0x80) != 0 { 7 } else { 0 };
            for func in 0..=max_func {
                let vend = unsafe { crate::arch::pci::read_config_16(bus as u8, slot, func, 0x00) };
                if vend == 0xFFFF {
                    continue;
                }
                let class_reg = unsafe { crate::arch::pci::read_config_32(bus as u8, slot, func, 0x08) };
                let class = ((class_reg >> 24) & 0xFF) as u8;
                let sub = ((class_reg >> 16) & 0xFF) as u8;
                let progif = ((class_reg >> 8) & 0xFF) as u8;
                if class == 0x01 && sub == 0x06 && progif == 0x01 {
                    let devid = unsafe { crate::arch::pci::read_config_16(bus as u8, slot, func, 0x02) };
                    return Some((bus as u8, slot, func, vend, devid));
                }
            }
        }
    }
    None
}

/// Claim the function: report its config state raw, enable memory decode AND bus master, then map
/// ABAR uncacheable. Returns the mapped base, or `None` with a named reason on serial.
///
/// **Bus Master IS enabled here**, unlike `sdhc::probe`'s PIO-only claim — AHCI has no PIO data
/// path at all; the controller masters the bus to fetch the command list, the command table and the
/// PRDT and to land the data. A driver that mapped ABAR without bus master would program everything
/// correctly and then watch `PxCI` never clear.
///
/// **ABAR is BAR5** (config offset 0x24), not BAR0. On the bench rMBP BAR0 is an I/O BAR at
/// `0x3080` — the legacy IDE task-file window the same function also decodes — and a driver that
/// read BAR0 would map sixteen bytes of I/O space as if it were a 4 KiB register block. AHCI 1.3.1
/// §2.1.11 places the AHCI register block at BAR5 and nowhere else.
fn hba_take(bus: u8, slot: u8, func: u8, vend: u16, devid: u16) -> Option<u64> {
    let bar5_raw = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x24) };
    let command = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x04) };

    serial_println!(
        "[ahci] bdf {}:{}.{} {:04x}:{:04x} bar5={:#010x} cmd={:#06x} mem-decode={} bus-master={}",
        bus, slot, func, vend, devid, bar5_raw, command,
        (command & 0x0002 != 0) as u8, (command >> 2) & 1
    );

    if (bar5_raw & 0x1) != 0 {
        serial_println!(
            "[ahci] bdf {}:{}.{} ABAR (BAR5) is an I/O BAR (io={:#x}) — not an AHCI register block, skipped",
            bus, slot, func, bar5_raw & 0xFFFF_FFFC
        );
        return None;
    }
    // BAR type bits 2:1 — 0b10 (value 0x4) is a 64-bit BAR, whose upper half lives in the NEXT
    // dword. BAR5 is the LAST base address register of a type-0 header, so there is no next dword:
    // config offset 0x28 is the Cardbus CIS Pointer, and a driver that "decoded" a 64-bit BAR5 would
    // be splicing an unrelated register into an address. AHCI 1.3.1 §2.1.11 specifies ABAR as a
    // 32-bit memory BAR, so this cannot legally happen — and the honest response to a controller
    // that reports it anyway is a named refusal, not a silently wrong base.
    if (bar5_raw & 0x06) == 0x04 {
        serial_println!(
            "[ahci] bdf {}:{}.{} ABAR (BAR5) reports 64-bit type (raw={:#010x}) — impossible for the last \
             BAR of a type-0 header (0x28 is the Cardbus CIS pointer, not a BAR high half); refused",
            bus, slot, func, bar5_raw
        );
        return None;
    }
    let abar = (bar5_raw & 0xFFFF_FFF0) as u64;
    if abar == 0 {
        serial_println!(
            "[ahci] bdf {}:{}.{} ABAR unassigned by firmware — no MMIO probe", bus, slot, func
        );
        return None;
    }

    // Memory decode + bus master. Only bits 1 and 2 are set; everything else is carried through.
    if command & 0x0006 != 0x0006 {
        unsafe { crate::arch::pci::write_config_16(bus, slot, func, 0x04, command | 0x0006) };
        let after = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x04) };
        serial_println!(
            "[ahci] claim bdf {}:{}.{} cmd {:#06x} -> {:#06x} (mem-decode + bus-master; DMA is the only data path AHCI has)",
            bus, slot, func, command, after
        );
        if after & 0x0006 != 0x0006 {
            serial_println!(
                "[ahci] claim bdf {}:{}.{} decode/bus-master did not stick (cmd={:#06x}) — controller not claimable",
                bus, slot, func, after
            );
            return None;
        }
    }

    // Identity-map the register block Uncacheable — the same seam `sdhc::probe` and the GPU drivers
    // use. Creating a mapping is a page-table edit, not a device access; the controller sees nothing.
    // 0x1100 covers generic host control (0x00..0x100) plus all 32 ports (0x100 + 32*0x80); the
    // identity map's leaves are 2 MiB, so this types the containing leaf UC either way.
    crate::arch::memory::map_mmio_window(abar, 0x1100);
    serial_println!("[ahci] map bdf {}:{}.{} abar={:#x} len={:#x} uncacheable", bus, slot, func, abar, 0x1100);
    Some(abar)
}

/// AHCI 1.3.1 §10.6.3 — BIOS/OS Handoff. Only meaningful when `CAP2.BOH` says the controller
/// implements it; on a controller that does not, `BOHC` is reserved and writing it is wrong.
///
/// Set `OOS`, then wait for the BIOS to drop `BOS`. The spec gives the BIOS 25 ms to respond and up
/// to two seconds when it sets `BB` (it is still moving data); this waits the two seconds only in
/// the `BB` case, which is the one where waiting is the point.
fn bios_handoff(abar: u64) {
    let cap2 = r32(abar, REG_CAP2);
    if cap2 & CAP2_BOH == 0 {
        serial_println!("[ahci] handoff: CAP2.BOH=0 — controller implements no BIOS/OS handoff, nothing to take");
        return;
    }
    let before = r32(abar, REG_BOHC);
    w32(abar, REG_BOHC, before | BOHC_OOS);
    let dropped = wait_ms(25, || r32(abar, REG_BOHC) & BOHC_BOS == 0);
    let mid = r32(abar, REG_BOHC);
    if !dropped && (mid & BOHC_BB) != 0 {
        // BIOS Busy: it acknowledged and is finishing a transfer. Two seconds is the spec's ceiling.
        wait_ms(2000, || r32(abar, REG_BOHC) & BOHC_BOS == 0);
    }
    let after = r32(abar, REG_BOHC);
    serial_println!(
        "[ahci] handoff: BOHC {:#010x} -> {:#010x} (BOS={} OOS={} BB={})",
        before, after, after & BOHC_BOS, (after & BOHC_OOS) >> 1, (after & BOHC_BB) >> 4
    );
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// Step 2 — port bring-up
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// AHCI 1.3.1 §10.1.2: stop the port's command engine and its FIS receive engine, in that order,
/// and wait for each to acknowledge. Returns false if either engine refused to stop inside its
/// specified 500 ms, which means the port must not be reprogrammed — its DMA pointers are live.
fn port_stop(abar: u64, port: u8) -> bool {
    let cmd = pr32(abar, port, P_CMD);
    if cmd & PCMD_ST != 0 {
        pw32(abar, port, P_CMD, cmd & !PCMD_ST);
    }
    if !wait_ms(T_PORT_MS, || pr32(abar, port, P_CMD) & PCMD_CR == 0) {
        serial_println!("[ahci] port {} CR never cleared after ST=0 (PxCMD={:#010x}) — port left alone",
            port, pr32(abar, port, P_CMD));
        return false;
    }
    let cmd = pr32(abar, port, P_CMD);
    if cmd & PCMD_FRE != 0 {
        pw32(abar, port, P_CMD, cmd & !PCMD_FRE);
    }
    if !wait_ms(T_PORT_MS, || pr32(abar, port, P_CMD) & PCMD_FR == 0) {
        serial_println!("[ahci] port {} FR never cleared after FRE=0 (PxCMD={:#010x}) — port left alone",
            port, pr32(abar, port, P_CMD));
        return false;
    }
    true
}

/// AHCI 1.3.1 §10.1.2: start the FIS receive engine, then the command engine — the ClearBusy
/// discipline. `ST` is set ONLY when the task file is idle (`BSY` and `DRQ` both clear) and the
/// link is up; setting it over a busy task file is how a port ends up permanently wedged.
fn port_start(abar: u64, port: u8) -> bool {
    let cmd = pr32(abar, port, P_CMD);
    pw32(abar, port, P_CMD, cmd | PCMD_FRE);
    if !wait_ms(T_PORT_MS, || pr32(abar, port, P_CMD) & PCMD_FR != 0) {
        serial_println!("[ahci] port {} FR never set after FRE=1 — receive engine did not start", port);
        return false;
    }
    if !wait_ms(T_PORT_MS, || {
        let tfd = pr32(abar, port, P_TFD);
        tfd & (TFD_BSY | TFD_DRQ) == 0
    }) {
        serial_println!("[ahci] port {} task file still BSY/DRQ (PxTFD={:#010x}) — ST not set", port,
            pr32(abar, port, P_TFD));
        return false;
    }
    let cmd = pr32(abar, port, P_CMD);
    pw32(abar, port, P_CMD, cmd | PCMD_ST);
    true
}

/// Build the Register Host-to-Device FIS for one command directly into the command table's CFIS
/// area, program the single PRDT entry, fill command header slot 0, issue it on `PxCI` bit 0 and
/// poll to completion.
///
/// Returns `Ok(())` when the controller cleared `PxCI` bit 0 with no task-file error. Every failure
/// path names what it saw; none of them leaves the port running a command (a timeout stops and
/// restarts the port so the next call starts from a known state).
///
/// `count` is in SECTORS and is capped at 1 by every caller in this arc. `lba` is a 48-bit LBA;
/// `IDENTIFY DEVICE` ignores both and passes zero.
fn issue(p: &AhciPort, cmd: u8, lba: u64, count: u16, bytes: usize, write: bool) -> Result<(), ()> {
    let abar = p.abar;
    let port = p.port;

    // Only one command slot is ever used, so a busy PxCI bit 0 means the previous command never
    // finished — refuse rather than overwrite a table the controller may still be reading.
    if pr32(abar, port, P_CI) & 1 != 0 {
        serial_println!("[ahci] port {} slot 0 still busy (PxCI={:#010x}) — command {:#04x} refused",
            port, pr32(abar, port, P_CI), cmd);
        return Err(());
    }

    unsafe {
        // ── Command table: CFIS at +0x00 (AHCI 1.3.1 §4.2.3) ────────────────────────────────────
        let ct = p.ctba as *mut u8;
        core::ptr::write_bytes(ct, 0, CMDTBL_BYTES);
        // Register H2D FIS, Serial ATA §10.3.4: type 0x27, C bit (bit 7 of byte 1) set = this FIS
        // carries a command rather than a control update.
        ct.add(0).write_volatile(0x27);
        ct.add(1).write_volatile(0x80);
        ct.add(2).write_volatile(cmd);
        ct.add(3).write_volatile(0); // features (low)
        ct.add(4).write_volatile((lba & 0xFF) as u8);
        ct.add(5).write_volatile(((lba >> 8) & 0xFF) as u8);
        ct.add(6).write_volatile(((lba >> 16) & 0xFF) as u8);
        // Device register. Bit 6 = LBA mode. IDENTIFY DEVICE takes a zero device register; every
        // data command in this arc is LBA-addressed.
        ct.add(7).write_volatile(if cmd == ATA_IDENTIFY_DEVICE { 0 } else { 1 << 6 });
        ct.add(8).write_volatile(((lba >> 24) & 0xFF) as u8);
        ct.add(9).write_volatile(((lba >> 32) & 0xFF) as u8);
        ct.add(10).write_volatile(((lba >> 40) & 0xFF) as u8);
        ct.add(11).write_volatile(0); // features (high)
        ct.add(12).write_volatile((count & 0xFF) as u8);
        ct.add(13).write_volatile(((count >> 8) & 0xFF) as u8);
        ct.add(14).write_volatile(0); // ICC
        ct.add(15).write_volatile(0); // control

        // ── PRDT entry 0 at +0x80 (AHCI 1.3.1 §4.2.3.3). DBC is byte count MINUS ONE and must be
        // odd (i.e. an even byte count); interrupt-on-completion is left clear — nothing in this
        // arc uses interrupts.
        let prdt = (p.ctba + 0x80) as *mut u32;
        let dba = bus_addr(p.dma);
        prdt.add(0).write_volatile((dba & 0xFFFF_FFFF) as u32);
        prdt.add(1).write_volatile((dba >> 32) as u32);
        prdt.add(2).write_volatile(0);
        prdt.add(3).write_volatile((bytes as u32).saturating_sub(1) & 0x003F_FFFF);

        // ── Command header slot 0 (AHCI 1.3.1 §4.2.2) ───────────────────────────────────────────
        // DW0: CFL in DWORDs (bits 4:0) — a Register H2D FIS is 20 bytes = 5 DWORDs; W (bit 6) is
        // the direction, 0 for a device-to-host transfer; PRDTL in bits 31:16.
        let hdr = p.clb as *mut u32;
        let dw0: u32 = 5 | (if write { 1 << 6 } else { 0 }) | (1u32 << 16);
        hdr.add(0).write_volatile(dw0);
        hdr.add(1).write_volatile(0); // PRDBC — the controller writes the byte count back here
        let ctba = bus_addr(p.ctba);
        hdr.add(2).write_volatile((ctba & 0xFFFF_FFFF) as u32);
        hdr.add(3).write_volatile((ctba >> 32) as u32);
        hdr.add(4).write_volatile(0);
        hdr.add(5).write_volatile(0);
        hdr.add(6).write_volatile(0);
        hdr.add(7).write_volatile(0);
    }

    // Clear stale status before issuing so a poll cannot read a previous command's error.
    pw32(abar, port, P_SERR, pr32(abar, port, P_SERR));
    pw32(abar, port, P_IS, pr32(abar, port, P_IS));

    pw32(abar, port, P_CI, 1);

    let done = wait_ms(T_CMD_MS, || {
        pr32(abar, port, P_CI) & 1 == 0 || pr32(abar, port, P_IS) & PIS_TFES != 0
    });

    let is = pr32(abar, port, P_IS);
    let tfd = pr32(abar, port, P_TFD);
    if !done {
        serial_println!(
            "[ahci] port {} command {:#04x} TIMED OUT after {} ms (PxCI={:#010x} PxIS={:#010x} PxTFD={:#010x} PxSERR={:#010x})",
            port, cmd, T_CMD_MS, pr32(abar, port, P_CI), is, tfd, pr32(abar, port, P_SERR)
        );
        // Leave the port in a known state rather than with a command outstanding.
        port_stop(abar, port);
        port_start(abar, port);
        return Err(());
    }
    if is & PIS_TFES != 0 || tfd & TFD_ERR != 0 {
        serial_println!(
            "[ahci] port {} command {:#04x} task-file error (PxIS={:#010x} PxTFD={:#010x} err={:#04x} PxSERR={:#010x})",
            port, cmd, is, tfd, (tfd >> 8) & 0xFF, pr32(abar, port, P_SERR)
        );
        return Err(());
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// Step 3 — IDENTIFY DEVICE decode
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// One IDENTIFY DEVICE response, decoded.
struct Identity {
    model: [u8; 40],
    serial: [u8; 20],
    sectors: u64,
    lba48: bool,
}

/// Read a 16-bit word out of the 512-byte IDENTIFY buffer. The data is little-endian words; the
/// TEXT fields inside it are big-endian byte pairs, which is why [`ata_string`] swaps and the
/// numeric readers do not.
#[inline]
fn id_word(buf: &[u8; SECTOR_BYTES], w: usize) -> u16 {
    u16::from_le_bytes([buf[w * 2], buf[w * 2 + 1]])
}

/// ATA/ATAPI: a string field is a run of words whose two bytes are stored HIGH byte first. Copy
/// them out swapped, replace anything outside printable ASCII with `?` (a device that returns
/// garbage here must not be able to inject control characters into the serial log), and trim
/// trailing spaces — the spec pads with 0x20.
fn ata_string(buf: &[u8; SECTOR_BYTES], first_word: usize, out: &mut [u8]) {
    let words = out.len() / 2;
    for i in 0..words {
        let w = id_word(buf, first_word + i);
        let hi = (w >> 8) as u8;
        let lo = (w & 0xFF) as u8;
        out[i * 2] = if (0x20..0x7F).contains(&hi) { hi } else { b'?' };
        out[i * 2 + 1] = if (0x20..0x7F).contains(&lo) { lo } else { b'?' };
    }
}

/// The printable prefix of an ATA string field, trailing spaces removed.
fn trimmed(s: &[u8]) -> &str {
    let mut end = s.len();
    while end > 0 && s[end - 1] == b' ' {
        end -= 1;
    }
    // Every byte was forced into printable ASCII by `ata_string`, so this cannot fail; the fallback
    // is there so a future caller cannot turn a decode bug into a panic in `pci::init`.
    core::str::from_utf8(&s[..end]).unwrap_or("?")
}

/// Decode the fields this arc needs out of a 512-byte IDENTIFY DEVICE response.
///
/// * words 10..19 — serial number (20 bytes)
/// * words 27..46 — model number (40 bytes)
/// * word 83 bit 10 — the 48-bit Address feature set is SUPPORTED
/// * words 100..103 — the 48-bit "Maximum LBA + 1", i.e. the sector count
/// * words 60..61 — the 28-bit sector count, used when 48-bit addressing is absent
///
/// A device claiming LBA48 but reporting a zero 48-bit count falls back to the 28-bit field rather
/// than publishing a zero-sector disk: `register_ahci` refuses zero, so the honest fallback is what
/// keeps a slightly-wrong device usable instead of invisible.
fn decode_identity(buf: &[u8; SECTOR_BYTES]) -> Identity {
    let mut model = [b' '; 40];
    let mut serial = [b' '; 20];
    ata_string(buf, 27, &mut model);
    ata_string(buf, 10, &mut serial);

    let lba48 = id_word(buf, 83) & (1 << 10) != 0;
    let s48 = (id_word(buf, 100) as u64)
        | ((id_word(buf, 101) as u64) << 16)
        | ((id_word(buf, 102) as u64) << 32)
        | ((id_word(buf, 103) as u64) << 48);
    let s28 = (id_word(buf, 60) as u64) | ((id_word(buf, 61) as u64) << 16);

    let sectors = if lba48 && s48 != 0 { s48 } else { s28 };
    Identity { model, serial, sectors, lba48: lba48 && s48 != 0 }
}

/// Classify LBA 0 by its signature bytes.
///
/// `MBR` — the 0x55AA boot signature at offset 510. `GPT` — the same, plus a protective MBR whose
/// first partition entry has type 0xEE, OR (the superfloppy-GPT case) the `EFI PART` magic that
/// really lives at LBA 1. This function only ever sees LBA 0, so it decides GPT from the protective
/// MBR's type byte, which is what a GPT disk is required to carry.
fn sector0_kind(buf: &[u8; SECTOR_BYTES]) -> &'static str {
    if buf[510] != 0x55 || buf[511] != 0xAA {
        return "none";
    }
    // MBR partition table: four 16-byte entries at 0x1BE; the type byte is at entry offset 4 and the
    // sector count is the little-endian u32 at entry offset 12.
    //
    // BOTH fields are read, and that is not belt-and-braces — it is what stops a FAT SUPERFLOPPY
    // from being called an MBR. A superfloppy's LBA 0 is a BPB, which carries the same 0x55AA at
    // offset 510 and has ordinary boot code or padding where the partition table would be, so a
    // signature-only test reports `MBR` on a disk that has no partition table at all. Requiring one
    // entry with a non-zero TYPE and a non-zero SIZE makes the verdict a statement about a table
    // that exists. A superfloppy then reads `none`, which is the honest answer this arc can give:
    // "LBA 0 carries a boot signature and no partition table".
    let mut real_entry = false;
    for e in 0..4 {
        let base = 0x1BE + e * 16;
        let ptype = buf[base + 4];
        let psize = u32::from_le_bytes([buf[base + 12], buf[base + 13], buf[base + 14], buf[base + 15]]);
        if ptype == 0xEE && psize != 0 {
            return "GPT";
        }
        if ptype != 0 && psize != 0 {
            real_entry = true;
        }
    }
    if real_entry { "MBR" } else { "none" }
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// Step 4 — the probe, and the public read entry point
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// Read one 512-byte sector from `port` into `buf`. This is what `drivers::block`'s AHCI handle
/// calls, and it is the ONLY way out of this module to the medium.
///
/// Bounded and synchronous: one READ DMA EXT, polled against [`T_CMD_MS`]. The port lock is held
/// across the command, so two callers serialise rather than interleaving command tables — with one
/// command slot in play that is a correctness requirement, not an optimisation.
pub fn read_block_at(port_ix: usize, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    if buf.len() < SECTOR_BYTES || port_ix >= MAX_AHCI_DISKS {
        return Err(());
    }
    let guard = PORTS.lock();
    let p = match guard[port_ix] {
        Some(p) => p,
        None => return Err(()),
    };
    if lba >= p.num_sectors {
        return Err(());
    }
    // READ DMA EXT is a 48-bit command; a device without 48-bit addressing cannot serve it. This
    // arc issues no 28-bit read command, so such a device is enumerated and reported and simply has
    // no read path — an honest gap, named here, rather than a command the device would abort.
    if !p.lba48 {
        return Err(());
    }
    issue(&p, ATA_READ_DMA_EXT, lba, 1, SECTOR_BYTES, false)?;
    unsafe {
        core::ptr::copy_nonoverlapping(p.dma as *const u8, buf.as_mut_ptr(), SECTOR_BYTES);
    }
    Ok(())
}

/// How many ports this driver brought up and published.
pub fn disk_count() -> usize {
    PORTS.lock().iter().filter(|e| e.is_some()).count()
}

/// Bring up one implemented port, if it holds a plain SATA disk. Returns the registry index it took,
/// or `None`.
fn bring_up_port(abar: u64, port: u8, cap: u32, next_ix: usize) -> Option<usize> {
    let ssts = pr32(abar, port, P_SSTS);
    let det = ssts & 0xF;
    let ipm = (ssts >> 8) & 0xF;

    // Staggered spin-up: when the controller supports it the port may be parked with the device
    // unspun, and DET reads 0 until SUD is set. Three conditions, and the third is what keeps this
    // off the boot's critical path: the controller must advertise SSS, the link must not already be
    // up, and `PxCMD.SUD` must still be CLEAR. If firmware already spun this port up and DET is
    // still 0, the port is genuinely empty and waiting a second time buys nothing — without that
    // term an empty six-port HBA would pay [`T_LINK_MS`] six times over on every armed boot.
    let det = if det != DET_PRESENT && cap & CAP_SSS != 0 && pr32(abar, port, P_CMD) & PCMD_SUD == 0 {
        let cmd = pr32(abar, port, P_CMD);
        pw32(abar, port, P_CMD, cmd | PCMD_SUD);
        wait_ms(T_LINK_MS, || pr32(abar, port, P_SSTS) & 0xF == DET_PRESENT);
        pr32(abar, port, P_SSTS) & 0xF
    } else {
        det
    };

    if det != DET_PRESENT {
        serial_println!("[ahci] port {} no device (PxSSTS={:#010x} DET={} IPM={})",
            port, pr32(abar, port, P_SSTS), det, ipm);
        return None;
    }

    let sig = pr32(abar, port, P_SIG);
    if sig != SIG_SATA_DISK {
        serial_println!(
            "[ahci] port {} device present but PxSIG={:#010x} is not a plain SATA disk ({:#010x}) — skipped",
            port, sig, SIG_SATA_DISK
        );
        return None;
    }

    if next_ix >= MAX_AHCI_DISKS {
        serial_println!("[ahci] port {} SATA disk present but the registry is full ({} entries) — not published",
            port, MAX_AHCI_DISKS);
        return None;
    }

    if !port_stop(abar, port) {
        return None;
    }

    let clb = dma_alloc(CMDLIST_BYTES, CMDLIST_ALIGN);
    let fb = dma_alloc(FIS_BYTES, FIS_ALIGN);
    let ctba = dma_alloc(CMDTBL_BYTES, CMDTBL_ALIGN);
    let dma = dma_alloc(SECTOR_BYTES, 4096);
    if clb == 0 || fb == 0 || ctba == 0 || dma == 0 {
        serial_println!("[ahci] port {} DMA allocation failed (clb={:#x} fb={:#x} ctba={:#x} dma={:#x}) — port not brought up",
            port, clb, fb, ctba, dma);
        return None;
    }

    let clb_b = bus_addr(clb);
    let fb_b = bus_addr(fb);
    pw32(abar, port, P_CLB, (clb_b & 0xFFFF_FFFF) as u32);
    pw32(abar, port, P_CLBU, (clb_b >> 32) as u32);
    pw32(abar, port, P_FB, (fb_b & 0xFFFF_FFFF) as u32);
    pw32(abar, port, P_FBU, (fb_b >> 32) as u32);
    // Interrupts stay masked for the life of this driver — see the SCOPE block.
    pw32(abar, port, P_IE, 0);
    pw32(abar, port, P_SERR, pr32(abar, port, P_SERR));
    pw32(abar, port, P_IS, pr32(abar, port, P_IS));
    pw32(abar, port, P_SACT, 0);

    if !port_start(abar, port) {
        return None;
    }

    let p = AhciPort { abar, port, clb, ctba, dma, num_sectors: 0, lba48: false };

    // ── IDENTIFY DEVICE ─────────────────────────────────────────────────────────────────────────
    if issue(&p, ATA_IDENTIFY_DEVICE, 0, 0, SECTOR_BYTES, false).is_err() {
        serial_println!(":: AHCI: selfcheck port={} identify=bad sector0=none -> FAIL ::", port);
        return None;
    }
    let mut idbuf = [0u8; SECTOR_BYTES];
    unsafe { core::ptr::copy_nonoverlapping(p.dma as *const u8, idbuf.as_mut_ptr(), SECTOR_BYTES) };
    let id = decode_identity(&idbuf);

    let p = AhciPort { num_sectors: id.sectors, lba48: id.lba48, ..p };
    PORTS.lock()[next_ix] = Some(p);

    serial_println!(
        ":: AHCI: port={} model=\"{}\" sectors={} lba48={} ::",
        port, trimmed(&id.model), id.sectors, id.lba48 as u8
    );
    serial_println!(
        "[ahci] port {} serial=\"{}\" capacity={} MiB registry-index={}",
        port, trimmed(&id.serial), id.sectors.saturating_mul(SECTOR_BYTES as u64) / (1024 * 1024), next_ix
    );

    let identify_ok = id.sectors != 0;

    // ── Sector 0 ────────────────────────────────────────────────────────────────────────────────
    let mut s0 = [0u8; SECTOR_BYTES];
    let kind = if identify_ok && read_block_at(next_ix, 0, &mut s0).is_ok() {
        let k = sector0_kind(&s0);
        let sig16 = u16::from_le_bytes([s0[510], s0[511]]);
        serial_println!(":: AHCI: port={} sector0 sig={:#06x} kind={} ::", port, sig16, k);
        k
    } else {
        serial_println!(":: AHCI: port={} sector0 sig=0x0000 kind=none ::", port);
        "none"
    };

    // ── Publish ─────────────────────────────────────────────────────────────────────────────────
    if identify_ok {
        crate::drivers::block::register_ahci(next_ix, port, id.sectors, &id.model);
    } else {
        // Roll the entry back: a disk whose IDENTIFY decoded to zero sectors has no addressable
        // range, so an entry for it would be a device every bound check rejects one call later.
        PORTS.lock()[next_ix] = None;
    }

    serial_println!(
        ":: AHCI: selfcheck port={} identify={} sector0={} -> {} ::",
        port,
        if identify_ok { "ok" } else { "bad" },
        kind,
        if identify_ok && kind != "none" { "PASS" } else { "FAIL" }
    );

    if identify_ok { Some(next_ix) } else { None }
}

/// The one entry point. Called from `arch::x86_64::pci::init`, once, behind the `ahci` feature.
///
/// Every step names itself on serial, and every refusal says which register convicted it — a probe
/// that silently did not run is indistinguishable on the wire from a machine with no SATA
/// controller, which is the exact failure shape LAWS §5 calls out.
pub fn probe() {
    let (bus, slot, func, vend, devid) = match find_controller() {
        Some(t) => t,
        None => {
            serial_println!("[ahci] no AHCI controller (class 0x01/0x06 progif 0x01) on this machine");
            return;
        }
    };

    let abar = match hba_take(bus, slot, func, vend, devid) {
        Some(a) => a,
        None => return,
    };

    bios_handoff(abar);

    // AHCI 1.3.1 §10.1.2: the HBA must be in AHCI mode before any AHCI register other than GHC is
    // meaningful. Set AE and leave it set; this arc never issues HBA Reset (GHC.HR), which would
    // throw away whatever state firmware left on ports we are not claiming.
    let ghc = r32(abar, REG_GHC);
    if ghc & GHC_AE == 0 {
        w32(abar, REG_GHC, ghc | GHC_AE);
    }
    let ghc_after = r32(abar, REG_GHC);

    let cap = r32(abar, REG_CAP);
    let pi = r32(abar, REG_PI);
    let vs = r32(abar, REG_VS);
    let n_ports = (cap & 0x1F) + 1;
    let n_slots = ((cap >> 8) & 0x1F) + 1;

    serial_println!(
        "[ahci] hba VS={}.{}.{} CAP={:#010x} (np={} ncs={} s64a={} sss={} sncq={}) PI={:#010x} GHC={:#010x} (AE={} HR={})",
        (vs >> 16) & 0xFFFF, (vs >> 8) & 0xFF, vs & 0xFF,
        cap, n_ports, n_slots, (cap >> 31) & 1, (cap >> 27) & 1, (cap >> 30) & 1,
        pi, ghc_after, (ghc_after >> 31) & 1, ghc_after & GHC_HR
    );

    if pi == 0 {
        serial_println!("[ahci] PI=0 — the controller implements no ports; nothing to enumerate");
        return;
    }

    let mut next_ix = 0usize;
    let mut seen = 0u32;
    for port in 0..MAX_PORTS as u8 {
        if pi & (1u32 << port) == 0 {
            continue;
        }
        seen += 1;
        if let Some(ix) = bring_up_port(abar, port, cap, next_ix) {
            next_ix = ix + 1;
        }
    }

    serial_println!(
        "[ahci] done: implemented-ports={} published={} (READ-ONLY arc — no WRITE opcode is compiled into this image)",
        seen, next_ix
    ); #[cfg(feature = "witness")] crate::fs::bootdisk::ahciboot_selftest(); // AHCIBOOT (B89 second rung): the wire fixture, folded onto this line so knob-off byte identity is untouched. It runs HERE because this is the last statement of the one enumeration pass — registry populated, every HBA and port lock released, heap up — and because on x86 nothing else on a headless boot runs after it: `shell::vfs_mount_table`'s `bootdisk::bind` arm is `target_arch = "aarch64"`. Default-quiet (`witness`), like `homesoil_selftest`.
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// AHCIWRITE (rmbp-ledger B89, SATA write half) — `WRITE DMA EXT`, behind `ahci-write`.
//
// APPENDED AT THE FILE TAIL, and that is a byte-identity requirement rather than a style: a
// `#[cfg]`'d-off block still shifts every `core::panic::Location` line below it (LAWS §5), and this
// file has plenty — every slice index in `sector0_kind` and `id_word` carries one. At the tail,
// nothing is below it to move, so `ahci-write` OFF is byte-for-byte the image AHCI shipped.
//
// ### THE ONE THING THIS SECTION IS FOR, stated before the code
//
// Peter's internal SSD has Catalina on it. The arc's whole design premise is that the wrong write
// must be impossible BY CONSTRUCTION, not by care, so this function is deliberately NOT a peer of
// `read_block_at`: it is private to the crate's write capability and has exactly ONE caller in the
// tree, `drivers::block::write_sectors_granted`, which will not call it without a `WriteGrant`
// minted by `install::partition::mint_grant` after the census and the whole refusal ladder passed.
// `grep -rn 'write_block_at' unaos/crates/kernel/src` is the audit, and it prints two lines: this
// declaration and that one call.
//
// ### The spec, cited
//
// ATA8-ACS `WRITE DMA EXT` (0x35) is the 48-bit DMA write: the same Register H2D FIS shape as
// `READ DMA EXT` (0x25) with the opposite data direction, LBA in the six LBA bytes, sector count in
// the two count bytes, device register bit 6 (LBA mode) set. AHCI 1.3.1 §5.5 issues it exactly as
// the read is issued — command header slot 0, one PRDT entry, `PxCI` bit 0 — with §4.2.2's `W` bit
// (DW0 bit 6) set, which is what tells the HBA the PRDT is a SOURCE of data rather than a sink.
// [`issue`] already carries that `write` parameter and already sets that bit; this arc adds no new
// register discipline, no new wait, and no second command slot.

/// ATA8-ACS `WRITE DMA EXT`. THE THIRD AND LAST OPCODE THIS FILE CAN ISSUE, and only with
/// `ahci-write` compiled in — see the SCOPE block at the head of the module.
#[cfg(feature = "ahci-write")]
const ATA_WRITE_DMA_EXT: u8 = 0x35;

/// AHCIWRITE: one-shot arming witness. It carries the opcode census token the DONE gate counts on
/// the ELF (`WRITE-DMA-EXT-0x35`), and that token appears in EXACTLY ONE string literal in the whole
/// tree, so a census of the artifact is a census of this code path's presence — an instrument proven
/// in the artifact and not in the diff (LAWS §5).
#[cfg(feature = "ahci-write")]
static WRITE_ARMED_SAID: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Write one 512-byte sector to `port_ix` (a REGISTRY INDEX, the same key [`read_block_at`] takes)
/// from `buf`. Bounded and synchronous: one `WRITE DMA EXT`, polled against [`T_CMD_MS`], D2H status
/// and error decoded by [`issue`] and printed on any failure.
///
/// ⚠ **DO NOT GIVE THIS A SECOND CALLER.** It is `pub(crate)`, not `pub`, and the crate's single call
/// site is `drivers::block::write_sectors_granted`, which holds a `WriteGrant` naming the only LBA
/// range an install was cleared to touch. A direct caller would be a write to Peter's live SSD with
/// no census, no refusal ladder and no bound — the exact thing rmbp-ledger B91 and RULINGS R25 are
/// about. If a second caller is ever genuinely needed, it goes through the grant too.
///
/// Symmetric with the read in every other respect: the same port lock across the command (one
/// command slot means two callers must serialise, which is correctness and not tuning), the same
/// LBA48 requirement (this arc issues no 28-bit write), and the same per-port bound on
/// `num_sectors`, which is the LAST of three independent bounds — the grant's range check and the
/// `PartitionTarget`'s `map()` are the other two, and they live in different files on purpose.
#[cfg(feature = "ahci-write")]
pub(crate) fn write_block_at(port_ix: usize, lba: u64, buf: &[u8]) -> Result<(), ()> {
    if buf.len() < SECTOR_BYTES || port_ix >= MAX_AHCI_DISKS {
        return Err(());
    }
    let guard = PORTS.lock();
    let p = match guard[port_ix] {
        Some(p) => p,
        None => return Err(()),
    };
    if lba >= p.num_sectors {
        serial_println!(
            "[ahci] port {} WRITE refused lba={} beyond device capacity {} sectors",
            p.port, lba, p.num_sectors
        );
        return Err(());
    }
    // WRITE DMA EXT is a 48-bit command. A device without the 48-bit Address feature set cannot
    // serve it, and this arc issues no 28-bit write, so such a device stays read-only — named here
    // rather than discovered as a task-file abort three layers up.
    if !p.lba48 {
        serial_println!("[ahci] port {} WRITE refused — device reports no LBA48 and no 28-bit write is compiled", p.port);
        return Err(());
    }
    if !WRITE_ARMED_SAID.swap(true, core::sync::atomic::Ordering::Relaxed) {
        serial_println!(
            ":: AHCI: write path ARMED — opcode WRITE-DMA-EXT-0x35 (ATA8-ACS, LBA48) compiled and reachable only through a WriteGrant (first, once) ::"
        );
    }
    // Stage the payload into the port's DMA buffer, which the HBA will now READ FROM rather than
    // write into. Same buffer, same bus address, opposite direction — the direction lives entirely
    // in the command header's W bit (AHCI 1.3.1 §4.2.2) and in the ATA opcode.
    unsafe {
        core::ptr::copy_nonoverlapping(buf.as_ptr(), p.dma as *mut u8, SECTOR_BYTES);
    }
    issue(&p, ATA_WRITE_DMA_EXT, lba, 1, SECTOR_BYTES, true)
}
