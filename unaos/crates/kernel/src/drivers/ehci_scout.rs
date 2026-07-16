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

/// EHCI operational-register offsets off (BAR0 + CAPLENGTH).
const OP_USBCMD: u64 = 0x00;
const OP_USBSTS: u64 = 0x04;
const OP_CONFIGFLAG: u64 = 0x40;
const OP_PORTSC0: u64 = 0x44;

/// Read a u32 from an identity-mapped MMIO physical address, but ONLY after proving the page is
/// present in the live page tables (firmware identity map). Returns `None` if unmapped — so a metal
/// boot where the EHCI BAR is outside the firmware identity map reports honestly instead of taking
/// an unhandled #PF. Read-only: `read_volatile` of device MMIO has no side effect on EHCI cap/op
/// status registers we touch here.
unsafe fn mmio_read32(phys: u64) -> Option<u32> {
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
