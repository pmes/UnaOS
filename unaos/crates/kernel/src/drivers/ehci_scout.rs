// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! EHCI-1 scout — read-only EHCI reconnaissance (UNAOS_EHCISCOUT=1).
//!
//! The 2012 rMBP's internal keyboard + trackpad are invisible to our xHCI-only driver. PORTSW-1
//! established (metal-log evidence) that the Panther Point USB2 port-routing write (`XUSB2PR`) does
//! not surface them — they sit outside the 4-port switchable mask, i.e. almost certainly on
//! EHCI-only ports behind the companion controllers. Before anyone designs an EHCI driver arc, this
//! scout answers what that driver would actually face.
//!
//! STRICTLY READ-ONLY (Maestro tripwire-grade condition). Every access in this module is a PCI
//! config-space READ or an MMIO READ off the EHCI BAR. There is not a single write to any
//! controller register, PCI config register, or port: no port reset, no ownership handoff (the
//! BIOS/OS semaphore and CONFIGFLAG are READ and reported, never written), no run/stop change, no
//! doorbell. A scout that pokes is a STOP tripwire; a register that cannot be read without a side
//! effect is skipped and reported as skipped.
//!
//! Output is a single bounded, awk-friendly block bracketed by `:: EHCI-SCOUT: begin ::` …
//! `:: EHCI-SCOUT: end (N controllers, M ports, K connected) ::`. x86_64 only; compiled solely
//! under the `ehciscout` feature (knob OFF => this module does not exist and no code runs, media
//! byte-identical). Inert-but-honest on QEMU: reports whatever the machine exposes, or the honest
//! `no EHCI controller found` line if none.

use crate::arch::pci::{read_config_16, read_config_32};

/// EHCI class code: base 0x0C (Serial Bus), subclass 0x03 (USB), prog-IF 0x20 (EHCI).
const EHCI_CLASS: u8 = 0x0C;
const EHCI_SUBCLASS: u8 = 0x03;
const EHCI_PROGIF: u8 = 0x20;

/// PCI config offsets.
const CFG_STATUS_COMMAND: u8 = 0x04; // command (lo16) + status (hi16)
const CFG_CLASS: u8 = 0x08; // rev/prog-if/subclass/class
const CFG_HEADER_TYPE: u8 = 0x0C;
const CFG_BAR0: u8 = 0x10;
const CFG_CAP_PTR: u8 = 0x34;
const CFG_INTR: u8 = 0x3C; // int line (byte0) + int pin (byte1)

/// EHCI capability-register offsets off BAR0.
const CAP_CAPLENGTH: u64 = 0x00; // u8
const CAP_HCIVERSION: u64 = 0x02; // u16
const CAP_HCSPARAMS: u64 = 0x04; // u32
const CAP_HCCPARAMS: u64 = 0x08; // u32

/// EHCI operational-register offsets off (BAR0 + CAPLENGTH). pub(crate): the EHCI-3 driver layer
/// (`drivers/ehci`) programs the same operational block the scout reads.
pub(crate) const OP_USBCMD: u64 = 0x00;
pub(crate) const OP_USBSTS: u64 = 0x04;
pub(crate) const OP_CONFIGFLAG: u64 = 0x40;
pub(crate) const OP_PORTSC0: u64 = 0x44;

/// Read a u32 from an identity-mapped MMIO physical address, but ONLY after proving the page is
/// present in the live page tables (firmware identity map). Returns `None` if unmapped — so a metal
/// boot where the EHCI BAR is outside the firmware identity map reports honestly instead of taking
/// an unhandled #PF. Read-only: `read_volatile` of device MMIO has no side effect on EHCI cap/op
/// status registers we touch here.
pub(crate) unsafe fn mmio_read32(phys: u64) -> Option<u32> {
    // translate() walks CR3 and honors huge leaves; None => not present on the path.
    if crate::arch::x86_64::memory::translate(phys).is_none() {
        return None;
    }
    Some(core::ptr::read_volatile(phys as *const u32))
}

/// Walk the standard PCI capability list and return the byte offset of the first capability whose
/// id matches `want`, or 0 if absent. Read-only.
unsafe fn find_cap(bus: u8, dev: u8, func: u8, want: u8) -> u8 {
    let status = (read_config_32(bus, dev, func, CFG_STATUS_COMMAND) >> 16) as u16;
    if status & (1 << 4) == 0 {
        return 0; // no capability list
    }
    let mut ptr = (read_config_32(bus, dev, func, CFG_CAP_PTR) & 0xFF) as u8;
    let mut guard = 0u8;
    while ptr != 0 && ptr != 0xFF && guard < 48 {
        let cap = read_config_32(bus, dev, func, ptr);
        if (cap & 0xFF) as u8 == want {
            return ptr;
        }
        ptr = ((cap >> 8) & 0xFF) as u8;
        guard += 1;
    }
    0
}

/// Decode the two-bit EHCI/USB port line state to a short label (for interrupt-IN HID planning).
fn line_state(bits: u32) -> &'static str {
    match bits & 0x3 {
        0b00 => "SE0",
        0b01 => "K-state(low-speed)",
        0b10 => "J-state",
        _ => "undefined",
    }
}

/// Probe a single EHCI function fully (PCI config + MMIO cap/op/port reads). Returns
/// (ports_seen, ports_connected) so the caller can total the block trailer.
unsafe fn probe_controller(bus: u8, dev: u8, func: u8, idx: usize) -> (u32, u32) {
    let vendor = read_config_16(bus, dev, func, 0x00);
    let device = read_config_16(bus, dev, func, 0x02);

    // BAR0 (EHCI BARs are always 32-bit memory BARs, but decode the type defensively).
    let bar0_raw = read_config_32(bus, dev, func, CFG_BAR0);
    let is_io = bar0_raw & 0x1 != 0;
    let is_64 = (bar0_raw & 0x06) == 0x04;
    let bar0 = if is_64 {
        let hi = read_config_32(bus, dev, func, CFG_BAR0 + 4) as u64;
        (bar0_raw as u64 & 0xFFFF_FFF0) | (hi << 32)
    } else {
        (bar0_raw & 0xFFFF_FFF0) as u64
    };

    let intr = read_config_32(bus, dev, func, CFG_INTR);
    let int_line = (intr & 0xFF) as u8;
    let int_pin = ((intr >> 8) & 0xFF) as u8;

    serial_println!(
        ":: EHCI-SCOUT: [{}] bdf {}:{}.{} id {:04x}:{:04x} BAR0={:#x} ({}) IRQ={} INT{} ::",
        idx, bus, dev, func, vendor, device, bar0,
        if is_io { "IO?!" } else if is_64 { "mem64" } else { "mem32" },
        int_line,
        if int_pin >= 1 && int_pin <= 4 { (b'A' + int_pin - 1) as char } else { '?' }
    );

    // Power state via the PCI Power Management capability (id 0x01), PMCSR at cap+4, bits[1:0].
    let pm_cap = find_cap(bus, dev, func, 0x01);
    if pm_cap != 0 {
        let pmcsr = read_config_32(bus, dev, func, pm_cap + 4);
        serial_println!(
            ":: EHCI-SCOUT: [{}] PM cap@{:#04x} PMCSR={:#06x} power-state=D{} ::",
            idx, pm_cap, pmcsr & 0xFFFF, pmcsr & 0x3
        );
    } else {
        serial_println!(":: EHCI-SCOUT: [{}] no PCI PM capability (power state unknown) ::", idx);
    }

    // Intel RMH note. Panther Point EHCI ports sit behind an integrated rate-matching hub (RMH):
    // devices enumerate as downstream of that hub rather than on a bare root port. There is no
    // standard config register that names the RMH; report the identity + what the config space
    // shows so the reader can correlate on metal.
    if vendor == 0x8086 {
        serial_println!(
            ":: EHCI-SCOUT: [{}] Intel EHCI ({:04x}:{:04x}) — Panther Point #1=8086:1e26 #2=8086:1e2d; \
ports sit behind the integrated rate-matching hub (RMH), devices appear hub-downstream ::",
            idx, vendor, device
        );
    }

    // MMIO capability registers off BAR0 (guarded by translate()).
    let caplength_word = match mmio_read32(bar0 + CAP_CAPLENGTH) {
        Some(v) => v,
        None => {
            serial_println!(
                ":: EHCI-SCOUT: [{}] BAR0 {:#x} not present in firmware identity map — skipping MMIO reads (no fault, no writes) ::",
                idx, bar0
            );
            return (0, 0);
        }
    };
    let caplength = (caplength_word & 0xFF) as u64;
    let hciversion = match mmio_read32(bar0 + (CAP_HCIVERSION & !0x3)) {
        Some(v) => ((v >> 16) & 0xFFFF) as u16, // HCIVERSION is the high half of the 0x00 dword
        None => 0,
    };
    let hcsparams = mmio_read32(bar0 + CAP_HCSPARAMS).unwrap_or(0);
    let hccparams = mmio_read32(bar0 + CAP_HCCPARAMS).unwrap_or(0);

    let n_ports = hcsparams & 0xF;
    let ppc = (hcsparams >> 4) & 0x1; // Port Power Control
    let prr = (hcsparams >> 7) & 0x1; // Port Routing Rules
    let n_cc = (hcsparams >> 12) & 0xF; // number of companion controllers
    let n_pcc = (hcsparams >> 8) & 0xF; // ports per companion controller
    let addr64 = hccparams & 0x1;
    let eecp = ((hccparams >> 8) & 0xFF) as u8; // EHCI Extended Capabilities Pointer (into config space)

    serial_println!(
        ":: EHCI-SCOUT: [{}] CAPLENGTH={:#x} HCIVERSION={:#06x} HCSPARAMS={:#010x} (N_PORTS={} PPC={} PRR={} N_CC={} N_PCC={}) ::",
        idx, caplength, hciversion, hcsparams, n_ports, ppc, prr, n_cc, n_pcc
    );
    serial_println!(
        ":: EHCI-SCOUT: [{}] HCCPARAMS={:#010x} (64bit-addr={} EECP={:#04x}) ::",
        idx, hccparams, addr64, eecp
    );

    // Extended capabilities live in PCI config space (EECP is a config offset). Report the
    // BIOS/OS ownership semaphore — READ ONLY, never written (that would be an ownership handoff,
    // a STOP tripwire). USBLEGSUP: id=byte0, next=byte1, bit16=HC BIOS Owned, bit24=HC OS Owned.
    if eecp >= 0x40 {
        let legsup = read_config_32(bus, dev, func, eecp);
        let bios_owned = (legsup >> 16) & 0x1;
        let os_owned = (legsup >> 24) & 0x1;
        serial_println!(
            ":: EHCI-SCOUT: [{}] USBLEGSUP@{:#04x}={:#010x} BIOS-owned={} OS-owned={} (READ-ONLY, semaphore NOT touched) ::",
            idx, eecp, legsup, bios_owned, os_owned
        );
    } else {
        serial_println!(":: EHCI-SCOUT: [{}] no EHCI extended-cap (EECP=0) — no BIOS/OS handoff semaphore ::", idx);
    }

    // Operational registers off (BAR0 + CAPLENGTH). All reads.
    let op = bar0 + caplength;
    let usbcmd = mmio_read32(op + OP_USBCMD).unwrap_or(0);
    let usbsts = mmio_read32(op + OP_USBSTS).unwrap_or(0);
    let configflag = mmio_read32(op + OP_CONFIGFLAG).unwrap_or(0);
    let rs = usbcmd & 0x1; // Run/Stop — is firmware still running the controller?
    let hchalted = (usbsts >> 12) & 0x1;
    let cf = configflag & 0x1; // 1 => ports routed to THIS EHCI; 0 => routed to companion (cHC)

    serial_println!(
        ":: EHCI-SCOUT: [{}] USBCMD={:#010x} (RS/run={}) USBSTS={:#010x} (HCHalted={}) CONFIGFLAG={:#x} (CF/route-to-EHCI={}) ::",
        idx, usbcmd, rs, usbsts, hchalted, configflag, cf
    );

    // PORTSC[i] — the heart of the scout. Each: bit0 CCS (connect), bit1 CSC, bit2 PED (enabled),
    // bit8 PR (reset), bits[11:10] line state, bit12 PP (port power), bit13 PO (port owner:
    // 1 => a companion/handoff owns this port, not EHCI). READ ONLY.
    let mut connected = 0u32;
    let ports = n_ports.min(15); // HCSPARAMS N_PORTS is 4 bits; clamp defensively
    for i in 0..ports {
        let portsc = match mmio_read32(op + OP_PORTSC0 + 4 * i as u64) {
            Some(v) => v,
            None => break,
        };
        let ccs = portsc & 0x1;
        let ped = (portsc >> 2) & 0x1;
        let pr = (portsc >> 8) & 0x1;
        let ls = (portsc >> 10) & 0x3;
        let pp = (portsc >> 12) & 0x1;
        let owner = (portsc >> 13) & 0x1;
        if ccs != 0 {
            connected += 1;
        }
        serial_println!(
            ":: EHCI-SCOUT: [{}] PORTSC[{}]={:#010x} connect={} enabled={} reset={} power={} owner={} line={} ::",
            idx, i, portsc, ccs, ped, pr, pp,
            if owner != 0 { "companion" } else { "EHCI" },
            line_state(ls)
        );
        let _ = ls; // line_state already consumed it for the label
    }

    (ports, connected)
}

/// Read-only EHCI reconnaissance entry point. Called once at PCI-init time (x86_64, knob-on only).
pub fn scout() {
    serial_println!(":: EHCI-SCOUT: begin ::");
    serial_println!(":: EHCI-SCOUT: read-only reconnaissance — PCI-config + MMIO READS only; zero writes to any register/port ::");

    let mut controllers = 0usize;
    let mut total_ports = 0u32;
    let mut total_connected = 0u32;

    for bus in 0u8..=255 {
        for dev in 0u8..=31 {
            let vendor = unsafe { read_config_16(bus, dev, 0, 0x00) };
            if vendor == 0xFFFF {
                continue;
            }
            let ht = ((unsafe { read_config_32(bus, dev, 0, CFG_HEADER_TYPE) } >> 16) & 0xFF) as u8;
            let max_func = if ht & 0x80 != 0 { 7 } else { 0 };
            for func in 0..=max_func {
                if func != 0 && unsafe { read_config_16(bus, dev, func, 0x00) } == 0xFFFF {
                    continue;
                }
                let class_reg = unsafe { read_config_32(bus, dev, func, CFG_CLASS) };
                let class = ((class_reg >> 24) & 0xFF) as u8;
                let subclass = ((class_reg >> 16) & 0xFF) as u8;
                let progif = ((class_reg >> 8) & 0xFF) as u8;
                if class == EHCI_CLASS && subclass == EHCI_SUBCLASS && progif == EHCI_PROGIF {
                    let (p, c) = unsafe { probe_controller(bus, dev, func, controllers) };
                    controllers += 1;
                    total_ports += p;
                    total_connected += c;
                }
            }
        }
    }

    if controllers == 0 {
        serial_println!(":: EHCI-SCOUT: no EHCI controller found (class 0x0C0320) — expected on q35 QEMU without -device usb-ehci ::");
    }

    serial_println!(
        ":: EHCI-SCOUT: end ({} controllers, {} ports, {} connected) ::",
        controllers, total_ports, total_connected
    );
}

// ======================================================================================
// EHCI-2 — configure-and-relook mode (UNAOS_EHCICONFIG=1, feature `ehciconfig`).
//
// The EHCI-1 metal survey found both Panther Point EHCI functions ASLEEP (RS=0, HCHalted=1,
// CONFIGFLAG=0, 0 connected). Asleep, "no device on the port" and "device not visible while the
// controller is unconfigured" are indistinguishable. This mode runs the MINIMAL wake sequence and
// re-censuses the ports twice (before AND after CONFIGFLAG=1), producing the evidence that
// distinguishes asleep-until-configured USB internals from not-USB (SPI). It is EVIDENCE ONLY: no
// enumeration, no transfers, no reset of any port, no async/periodic schedule, no driver.
//
// WRITE SURFACE (tripwire-grade): the ONLY registers written are, per EHCI function, its own PMCSR
// (D0 wake, PME_Status masked), its USBLEGSUP OS-owned bit (ownership handshake — never forced past
// a stuck BIOS bit), its USBLEGCTLSTS (eecp+4: SMI enables cleared + RW1C status acked after OS
// ownership — Maestro-granted extension, quirk_usb_handoff_ehci discipline), USBCMD.RS (run),
// CONFIGFLAG (route ports to EHCI), and PORTSC port-power bits (PP, only when PPC honors software
// control). Nothing else — never XUSB2PR / USB3_PSSEN / any xHCI register.
// ======================================================================================

/// Translate-guarded 32-bit MMIO write. Returns `false` (writing nothing) if the page is not present
/// in the live page tables, so a BAR outside the firmware identity map is reported as skipped rather
/// than faulting. Mirrors `mmio_read32`'s guard.
#[cfg(feature = "ehciconfig")]
pub(crate) unsafe fn mmio_write32(phys: u64, val: u32) -> bool {
    if crate::arch::x86_64::memory::translate(phys).is_none() {
        return false;
    }
    core::ptr::write_volatile(phys as *mut u32, val);
    true
}

/// Cycle count (in `now_cycles()`/rdtsc units) approximating `ms` milliseconds of wall clock. Uses
/// the calibrated TSC rate when available; falls back to a conservative fixed estimate otherwise
/// (~2.3 GHz Ivy Bridge base). Used for the connect-debounce settle between the two censuses.
#[cfg(feature = "ehciconfig")]
fn ms_cycles(ms: u64) -> u64 {
    let hz = crate::arch::x86_64::apic::tsc_hz();
    if hz != 0 {
        hz.saturating_mul(ms) / 1000
    } else {
        2_300_000u64.saturating_mul(ms) // ~2.3e6 cycles/ms fallback
    }
}

/// Busy-spin for approximately `ms` milliseconds against the free-running TSC (advances regardless of
/// EFLAGS.IF). Bounded and side-effect free — no MMIO, no writes.
#[cfg(feature = "ehciconfig")]
pub(crate) fn settle_ms(ms: u64) {
    let start = crate::arch::now_cycles();
    let budget = ms_cycles(ms);
    while crate::arch::now_cycles().wrapping_sub(start) < budget {
        core::hint::spin_loop();
    }
}

/// Spin until `pred()` is true or the hardware wait budget elapses. Returns `true` on success,
/// `false` on timeout. Wall-clock bounded via `now_cycles()`, so a wedged status bit fails fast
/// instead of freezing a serial-less boot. Side-effect free apart from the caller's MMIO reads.
#[cfg(feature = "ehciconfig")]
pub(crate) fn wait_bounded<F: Fn() -> bool>(pred: F) -> bool {
    let start = crate::arch::now_cycles();
    let budget = crate::arch::hw_wait_budget();
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

/// Report a PORTSC census over `ports` ports off operational base `op`, tagged `label` ("A"/"B").
/// Returns the connected count. READ ONLY.
#[cfg(feature = "ehciconfig")]
unsafe fn census(op: u64, ports: u32, idx: usize, label: &str) -> u32 {
    let mut connected = 0u32;
    for i in 0..ports {
        let portsc = match mmio_read32(op + OP_PORTSC0 + 4 * i as u64) {
            Some(v) => v,
            None => break,
        };
        let ccs = portsc & 0x1;
        let ped = (portsc >> 2) & 0x1;
        let pr = (portsc >> 8) & 0x1;
        let ls = (portsc >> 10) & 0x3;
        let pp = (portsc >> 12) & 0x1;
        let owner = (portsc >> 13) & 0x1;
        if ccs != 0 {
            connected += 1;
        }
        serial_println!(
            ":: EHCI-CONFIG: [{}] census{} PORTSC[{}]={:#010x} connect={} enabled={} reset={} power={} owner={} line={} ::",
            idx, label, i, portsc, ccs, ped, pr, pp,
            if owner != 0 { "companion" } else { "EHCI" },
            line_state(ls)
        );
    }
    serial_println!(
        ":: EHCI-CONFIG: [{}] census{} = {} connected ::",
        idx, label, connected
    );
    connected
}

/// One woken EHCI function, as returned by `wake_run`: everything a caller needs to reach the
/// operational registers and ports. Shared between the EHCI-2 evidence mode (census between the
/// two wake halves) and the EHCI-3 driver (`drivers/ehci`) — ONE wake path, not two (EHCI-3 N3).
#[cfg(feature = "ehciconfig")]
pub(crate) struct EhciFnHandle {
    /// Operational-register base (BAR0 + CAPLENGTH), translate()-proven present.
    pub op: u64,
    pub n_ports: u32,
    /// HCSPARAMS.PPC — 1 => software controls PORTSC.PP.
    pub ppc: u32,
    /// HCCPARAMS bit0 — 64-bit addressing capability (0 on Panther Point: DMA must be < 4 GiB).
    pub addr64: u32,
}

/// Shared wake, first half: PMCSR->D0, USBLEGSUP OS-ownership handshake + USBLEGCTLSTS SMI
/// clear/ack, USBCMD.RS=1. Returns `None` (having written nothing to MMIO) when BAR0 is outside
/// the firmware identity map. Trace lines keep the `EHCI-CONFIG` tag — this IS the EHCI-2
/// metal-clean wake path, whichever layer calls it (EHCI-2 evidence mode or the EHCI-3 driver);
/// keeping the exact lines is what makes the EHCI-2-unchanged refactor gate assertable.
/// Idempotent: on an already-woken function every step reads as done and re-writes are no-ops.
#[cfg(feature = "ehciconfig")]
pub(crate) unsafe fn wake_run(bus: u8, dev: u8, func: u8, idx: usize) -> Option<EhciFnHandle> {
    let vendor = read_config_16(bus, dev, func, 0x00);
    let device = read_config_16(bus, dev, func, 0x02);
    serial_println!(
        ":: EHCI-CONFIG: [{}] bdf {}:{}.{} id {:04x}:{:04x} — begin wake sequence ::",
        idx, bus, dev, func, vendor, device
    );

    // BAR0 (EHCI BARs are 32-bit memory BARs; decode defensively like the read-only scout).
    let bar0_raw = read_config_32(bus, dev, func, CFG_BAR0);
    let is_64 = (bar0_raw & 0x06) == 0x04;
    let bar0 = if is_64 {
        let hi = read_config_32(bus, dev, func, CFG_BAR0 + 4) as u64;
        (bar0_raw as u64 & 0xFFFF_FFF0) | (hi << 32)
    } else {
        (bar0_raw & 0xFFFF_FFF0) as u64
    };

    // ---- Step 1: PCI power state -> D0 (PMCSR bits[1:0]). Report the transition honestly. ----
    let pm_cap = find_cap(bus, dev, func, 0x01);
    if pm_cap != 0 {
        let pmcsr = read_config_32(bus, dev, func, pm_cap + 4);
        let state_before = pmcsr & 0x3;
        if state_before != 0 {
            // D0, masking PME_Status (bit 15, RW1C) so a pending PME event isn't silently acked
            // by the write-back of the read value.
            let new = pmcsr & !0x3 & !(1 << 15);
            crate::arch::pci::write_config_32(bus, dev, func, pm_cap + 4, new);
            let after = read_config_32(bus, dev, func, pm_cap + 4);
            serial_println!(
                ":: EHCI-CONFIG: [{}] PMCSR D{}->D0: wrote {:#06x} read-back {:#06x} (power-state=D{}) ::",
                idx, state_before, new & 0xFFFF, after & 0xFFFF, after & 0x3
            );
            // A real D-state transition needs recovery time before the function's MMIO is
            // trustworthy (PCI PM spec allows up to 10ms for D3hot->D0). No-op when already D0.
            settle_ms(10);
        } else {
            serial_println!(":: EHCI-CONFIG: [{}] PMCSR already D0 (no transition) ::", idx);
        }
    } else {
        serial_println!(":: EHCI-CONFIG: [{}] no PCI PM capability — cannot set D0, continuing ::", idx);
    }

    // MMIO cap registers (guarded). If the BAR is unmapped we cannot configure — report + skip.
    let caplength_word = match mmio_read32(bar0 + CAP_CAPLENGTH) {
        Some(v) => v,
        None => {
            serial_println!(
                ":: EHCI-CONFIG: [{}] BAR0 {:#x} not present in firmware identity map — SKIPPED (no writes) ::",
                idx, bar0
            );
            return None;
        }
    };
    let caplength = (caplength_word & 0xFF) as u64;
    let hcsparams = mmio_read32(bar0 + CAP_HCSPARAMS).unwrap_or(0);
    let hccparams = mmio_read32(bar0 + CAP_HCCPARAMS).unwrap_or(0);
    let n_ports = (hcsparams & 0xF).min(15);
    let ppc = (hcsparams >> 4) & 0x1; // Port Power Control: 1 => software controls PORTSC.PP
    let eecp = ((hccparams >> 8) & 0xFF) as u8;
    let op = bar0 + caplength;

    // ---- Step 2: USBLEGSUP OS-ownership handshake (config-space extended cap). ----
    // Set the OS-owned bit; wait (bounded) for the firmware to drop BIOS-owned. NEVER force past a
    // stuck BIOS bit — a timeout is a STOP-and-report line in the trace, not a panic, and the
    // sequence continues so the census still runs.
    if eecp >= 0x40 {
        let legsup = read_config_32(bus, dev, func, eecp);
        let bios_before = (legsup >> 16) & 0x1;
        let os_before = (legsup >> 24) & 0x1;
        // USBLEGCTLSTS (eecp+4) — the SMI enable/status register. If the firmware left SMI
        // enables set, the OS-own write (and the later RS/CF/PP writes) can raise SMIs into a
        // BIOS handler — the classic EHCI BIOS-handoff hang. Per the Maestro-granted write-
        // surface extension (one register, inside the same extended cap as the handshake), the
        // standard quirk_usb_handoff_ehci discipline applies AFTER acquiring OS ownership:
        // clear the SMI-enable mask (write 0s to the enable half) and ACK the RW1C status bits
        // (bits 31:29, write-1-to-clear). Three reported values: pre (before OS-own),
        // post-own (after the semaphore write), cleared (after the enable-clear+ack).
        let legctl_pre = read_config_32(bus, dev, func, eecp + 4);
        if os_before == 0 {
            crate::arch::pci::write_config_32(bus, dev, func, eecp, legsup | (1 << 24));
        }
        // Bounded wait for BIOS-owned to clear.
        let cleared = wait_bounded(|| (read_config_32(bus, dev, func, eecp) >> 16) & 0x1 == 0);
        let after = read_config_32(bus, dev, func, eecp);
        let legctl_post_own = read_config_32(bus, dev, func, eecp + 4);
        // Enables = 0 (all SMI sources off), RW1C status (31:29) = 1s to acknowledge.
        crate::arch::pci::write_config_32(bus, dev, func, eecp + 4, 0xE000_0000);
        let legctl_cleared = read_config_32(bus, dev, func, eecp + 4);
        serial_println!(
            ":: EHCI-CONFIG: [{}] USBLEGCTLSTS@{:#04x}: pre={:#010x} post-own={:#010x} cleared->{:#010x} (SMI enables off + RW1C status acked, quirk_usb_handoff_ehci discipline) ::",
            idx, eecp + 4, legctl_pre, legctl_post_own, legctl_cleared
        );
        if cleared {
            serial_println!(
                ":: EHCI-CONFIG: [{}] USBLEGSUP@{:#04x}: OS-own set, BIOS-owned cleared (was BIOS={} OS={}, now {:#010x}) ::",
                idx, eecp, bios_before, os_before, after
            );
        } else {
            serial_println!(
                ":: EHCI-CONFIG: [{}] STOP-NOTE USBLEGSUP@{:#04x}: BIOS-owned bit STUCK after OS-own+bounded-wait (now {:#010x}) — not forcing, continuing to census ::",
                idx, eecp, after
            );
        }
    } else {
        serial_println!(
            ":: EHCI-CONFIG: [{}] no EHCI extended-cap (EECP=0) — no BIOS/OS handoff needed ::", idx
        );
    }

    // ---- Step 3: start the controller (RS=1), bounded wait for HCHalted=0. CF stays 0 here. ----
    let usbcmd_before = mmio_read32(op + OP_USBCMD).unwrap_or(0);
    let _ = mmio_write32(op + OP_USBCMD, usbcmd_before | 0x1); // RS=1
    let running = wait_bounded(|| (mmio_read32(op + OP_USBSTS).unwrap_or(0x1000) >> 12) & 0x1 == 0);
    let usbcmd_after = mmio_read32(op + OP_USBCMD).unwrap_or(0);
    let usbsts_after = mmio_read32(op + OP_USBSTS).unwrap_or(0);
    serial_println!(
        ":: EHCI-CONFIG: [{}] RS 0->1: USBCMD {:#010x}->{:#010x} (RS={}), USBSTS={:#010x} HCHalted={} {} ::",
        idx, usbcmd_before, usbcmd_after, usbcmd_after & 0x1, usbsts_after, (usbsts_after >> 12) & 0x1,
        if running { "(running)" } else { "(STOP-NOTE: HCHalted did not clear within budget)" }
    );

    Some(EhciFnHandle { op, n_ports, ppc, addr64: hccparams & 0x1 })
}

/// EPACE-TRIM M4 latch — set when `wake_route` took the trimmed (PPC=0) settle instead of the
/// historical unconditional 150 ms. Read by `drivers::ehci`'s root-port failure paths so that a
/// port which does not come up names the trim in the same line that reports the failure: the
/// trimmed value can never be the silent cause of a regression.
#[cfg(feature = "ehciconfig")]
pub(crate) static PWR_SETTLE_TRIMMED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Shared wake, second half: CONFIGFLAG=1 (route ports to this EHCI), port-power where PPC honors
/// software control, then the pre-look settle. Same one-wake-path property as `wake_run`; the
/// split exists so the EHCI-2 evidence mode can census between RS=1 and CF=1.
#[cfg(feature = "ehciconfig")]
pub(crate) unsafe fn wake_route(h: &EhciFnHandle, idx: usize) {
    let (op, n_ports, ppc) = (h.op, h.n_ports, h.ppc);
    // ---- Step 3b: CONFIGFLAG=1 (route ports to this EHCI), then port-power on (PPC honored). ----
    let _ = mmio_write32(op + OP_CONFIGFLAG, 0x1);
    let cf_after = mmio_read32(op + OP_CONFIGFLAG).unwrap_or(0) & 0x1;
    serial_println!(
        ":: EHCI-CONFIG: [{}] CONFIGFLAG 0->1: read-back CF={} {} ::",
        idx, cf_after,
        if cf_after == 1 { "(ports routed to EHCI)" } else { "(STOP-NOTE: CF did not stick)" }
    );

    // Port power: only when PPC=1 does software control PORTSC.PP; otherwise ports are always powered
    // and writing PP is meaningless (leave untouched). Set PP without disturbing other bits, and do
    // NOT set PR (reset) — that would be a transaction-initiating step, out of scope.
    if ppc == 1 {
        for i in 0..n_ports {
            let addr = op + OP_PORTSC0 + 4 * i as u64;
            if let Some(portsc) = mmio_read32(addr) {
                if (portsc >> 12) & 0x1 == 0 {
                    // Preserve RW1C bits by not writing 1s to them: PORTSC RW1C bits are CSC(1),
                    // PEC(3), OCC(5). Mask them off so we only add PP(12).
                    let rw1c = (1 << 1) | (1 << 3) | (1 << 5);
                    let _ = mmio_write32(addr, (portsc & !rw1c) | (1 << 12));
                }
            }
        }
        serial_println!(":: EHCI-CONFIG: [{}] port-power applied (PPC=1) on {} ports ::", idx, n_ports);
    } else {
        serial_println!(":: EHCI-CONFIG: [{}] PPC=0 — ports always-powered, no PP write ::", idx);
    }

    // EPACE-TRIM M4 (GR18) — the pre-look settle, priced by what actually happened above.
    //
    // This pause was one unconditional `settle_ms(150)` and it is 450 of the 461 ms EPACE
    // charges to `hcrst=` (307 ms on controller 0, which routes twice, + 154 ms on controller 1;
    // the HCRESET itself measures ~4 ms per call in the same capture). It bundled two different
    // debts:
    //
    //   * PORT-POWER-GOOD — owed only when this function actually applied port power. On PPC=0
    //     silicon the ports are always-powered and the PP write above is skipped entirely, which
    //     the capture witnesses in as many words. rmbp-gr16-s73 ttyUSB0.log boot 7 prints
    //     `PPC=0 — ports always-powered, no PP write` at L8192/L8205 (controller 0, both routes)
    //     and L8224 (controller 1) — three routes, zero power transitions, and 150 ms of settle
    //     charged to each of them for a transition that never happened. Same three lines in
    //     boots 2/3/4/8. There is no power edge to settle: 0 ms owed.
    //   * ATTACH DEBOUNCE (USB 2.0 §7.1.7.3, T_ATTDB = 100 ms) — owed either way, and already
    //     paid IN FULL by the one caller that looks at a port: `reset_root_port` opens with its
    //     own `settle_ms(100)`, explicitly labelled T_ATTDB, before it reads PORTSC. Paying it
    //     here as well was the redundancy; the debounce is unchanged by this trim.
    //
    // What remains on the PPC=0 path is the CONFIGFLAG 0->1 port-mux re-route two writes above.
    // 20 ms is already orders of magnitude over a register write, and the caller's 100 ms T_ATTDB
    // still lands on top of it. PPC=1 silicon keeps the full 150 ms untouched: this arc has no
    // measurement of a real power-on edge, and an undecomposed constant is not trimmed.
    let settle = if ppc == 1 { 150 } else { 20 };
    if ppc != 1 {
        PWR_SETTLE_TRIMMED.store(true, core::sync::atomic::Ordering::Relaxed);
    }
    serial_println!(
        ":: EHCI-CONFIG: [{}] pre-look settle {} ms ({}) — caller's 100 ms T_ATTDB debounce follows ::",
        idx, settle,
        if ppc == 1 { "PPC=1, port power applied here" } else { "EPACE-TRIM M4: PPC=0, no power edge to settle" }
    );
    settle_ms(settle);
}

/// EHCI-2 evidence mode for one function: the shared wake with the two PORTSC censuses around the
/// routing flip. Returns (censusA_connected, censusB_connected). Writes confined to this function's
/// PMCSR / USBLEGSUP OS-own bit / USBLEGCTLSTS (SMI-enable clear + status ack) / USBCMD.RS /
/// CONFIGFLAG / PORTSC PP — all inside `wake_run`/`wake_route`, the one shared wake path.
#[cfg(feature = "ehciconfig")]
unsafe fn configure_controller(bus: u8, dev: u8, func: u8, idx: usize) -> (u32, u32) {
    let Some(h) = wake_run(bus, dev, func, idx) else {
        return (0, 0);
    };
    let (op, n_ports, ppc) = (h.op, h.n_ports, h.ppc);

    // ---- Census A: RS=1, CONFIGFLAG=0 (ports still routed to the companion). ----
    let cf_before = mmio_read32(op + OP_CONFIGFLAG).unwrap_or(0) & 0x1;
    serial_println!(
        ":: EHCI-CONFIG: [{}] censusA context: CONFIGFLAG={} (route-to-EHCI={}), N_PORTS={} PPC={} ::",
        idx, cf_before, cf_before, n_ports, ppc
    );
    let census_a = census(op, n_ports, idx, "A");

    wake_route(&h, idx);

    // EPACE-TRIM M4 does not reach this artifact (GR18 review finding 2). censusB's whole meaning
    // is "PORTSC after the route flip AND the full connect debounce" — it is the canonical EHCI-2
    // evidence reading, and every earlier capture of it was taken 150 ms after the flip. M4 short-
    // ened `wake_route`'s settle on the strength of its ONE other caller paying T_ATTDB itself;
    // this evidence path has no such caller and would silently start sampling at 20 ms, changing
    // what the artifact means without changing its name. Pay the remainder explicitly, and only
    // where M4 actually took it (PPC=1 still gets the full 150 ms inside `wake_route`).
    if ppc != 1 {
        settle_ms(130);
    }

    // ---- Census B: RS=1, CONFIGFLAG=1, ports powered, after the full 150 ms settle. ----
    let census_b = census(op, n_ports, idx, "B");

    serial_println!(
        ":: EHCI-CONFIG: [{}] done — censusA={} connected, censusB={} connected ::",
        idx, census_a, census_b
    );
    (census_a, census_b)
}

/// EHCI configure-and-relook entry point (x86_64, `ehciconfig` knob-on only). Runs after the
/// read-only scout. Walks EHCI functions, wakes each minimally, and reports two PORTSC censuses.
#[cfg(feature = "ehciconfig")]
pub fn configure_and_relook() {
    serial_println!(":: EHCI-CONFIG: begin ::");
    serial_println!(":: EHCI-CONFIG: minimal wake + two PORTSC censuses (CF 0 then 1) — evidence only; NO enumeration/transfers/reset/driver; writes confined to EHCI PMCSR/USBLEGSUP-OS-own/USBLEGCTLSTS/RS/CONFIGFLAG/PORTSC-PP ::");

    let mut controllers = 0usize;
    let mut total_a = 0u32;
    let mut total_b = 0u32;

    for bus in 0u8..=255 {
        for dev in 0u8..=31 {
            let vendor = unsafe { read_config_16(bus, dev, 0, 0x00) };
            if vendor == 0xFFFF {
                continue;
            }
            let ht = ((unsafe { read_config_32(bus, dev, 0, CFG_HEADER_TYPE) } >> 16) & 0xFF) as u8;
            let max_func = if ht & 0x80 != 0 { 7 } else { 0 };
            for func in 0..=max_func {
                if func != 0 && unsafe { read_config_16(bus, dev, func, 0x00) } == 0xFFFF {
                    continue;
                }
                let class_reg = unsafe { read_config_32(bus, dev, func, CFG_CLASS) };
                let class = ((class_reg >> 24) & 0xFF) as u8;
                let subclass = ((class_reg >> 16) & 0xFF) as u8;
                let progif = ((class_reg >> 8) & 0xFF) as u8;
                if class == EHCI_CLASS && subclass == EHCI_SUBCLASS && progif == EHCI_PROGIF {
                    let (a, b) = unsafe { configure_controller(bus, dev, func, controllers) };
                    controllers += 1;
                    total_a += a;
                    total_b += b;
                }
            }
        }
    }

    if controllers == 0 {
        serial_println!(":: EHCI-CONFIG: no EHCI controller found (class 0x0C0320) ::");
    }

    serial_println!(
        ":: EHCI-CONFIG: end ({} controllers, censusA={} connected, censusB={} connected) ::",
        controllers, total_a, total_b
    );
}
