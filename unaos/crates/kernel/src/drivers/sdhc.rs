// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! SDHC-1 — SD Host Controller (SDHCI) DISCOVERY, milestone 1: make the reader visible.
//!
//! The 2012 rMBP has a built-in SD card reader; the x86 kernel has never seen it, because
//! `drivers::pci::PciScanner::enumerate_buses` matches exactly one class triple (xHCI) and the
//! only block device this arch could reach was a USB mass-storage LUN behind it. Milestone 1
//! answers two questions off one boot's serial, and nothing else:
//!
//!   1. does the machine expose its card reader as a PCI/PCIe SD Host Controller
//!      (class 0x08 / subclass 0x05)? — `drivers::pci::PciScanner::storage_inventory`
//!   2. if so, WHICH SDHCI specification version, and what does it claim it can do?
//!      — the Host Controller Version register (0xFE) and Capabilities (0x40/0x44), here.
//!
//! **STRICTLY READ-ONLY.** This module issues no MMIO write of any kind: no software reset, no
//! clock or bus-power programming, no voltage switch, no command, no DMA, no interrupt enable.
//! It does not even size BAR0 (that needs the write-all-ones / restore dance on the BAR) nor
//! enable the function's memory decode — if the firmware left decode off or BAR0 unassigned, the
//! honest thing is to say so and stop, which is what this does. A controller the firmware left
//! quiescent is therefore observed exactly as quiescent; the next sitting's serial log is
//! evidence about the machine, not about anything we did to it.
//!
//! Witness format (one block per SDHCI function; `[sdhc]` prefix, grep-able):
//!
//! ```text
//! [sdhc] bdf B:S.F VVVV:DDDD bar0=0xADDR cmd=0xCCCC mem-decode=1
//! [sdhc] bdf B:S.F hcver=0xVVVV spec=3.00 vendor-ver=0x00
//! [sdhc] bdf B:S.F caps=0x........ caps2=0x........ base-clk=NNMHz max-blk=512 \
//!                  v3.3=1 v3.0=0 v1.8=0 sdma=1 adma2=1 hispeed=1 64bit=0 slot=removable
//! [sdhc] bdf B:S.F present=0x........ card-inserted=1 cd-stable=1 write-protected=0
//! ```
//!
//! and, when the probe cannot proceed, exactly one of:
//!
//! ```text
//! [sdhc] no SD host controller found (class 0x08/0x05)
//! [sdhc] bdf B:S.F bar0 unassigned by firmware — no MMIO probe
//! [sdhc] bdf B:S.F bar0 is an I/O BAR (io=0xADDR) — MMIO probe skipped
//! [sdhc] bdf B:S.F memory decode OFF (cmd=0xCCCC) — MMIO probe skipped (read-only: we do not enable it)
//! ```

#![allow(dead_code)]

use crate::drivers::pci::PciScanner;

/// SDHCI register offsets used by the milestone-1 probe. All read-only reads.
const REG_PRESENT_STATE: u64 = 0x24;
const REG_CAPABILITIES: u64 = 0x40;
const REG_CAPABILITIES_2: u64 = 0x44;
const REG_HOST_CTRL_VERSION: u64 = 0xFE;

/// The SDHCI register block is 256 bytes; map one page of it Uncacheable.
const SDHCI_MMIO_LEN: usize = 0x1000;

/// Decode the Specification Version Number (Host Controller Version register, bits 7:0) into the
/// spec's own version string. Unknown encodings are reported honestly rather than guessed.
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

/// SDHC-1 milestone 1: inventory the storage-class PCI functions, then for each SD Host
/// Controller read its version + capability + present-state registers and print the `[sdhc]`
/// witness. Read-only end to end; safe to run on any boot.
pub fn probe() {
    let controllers = PciScanner::storage_inventory();
    if controllers.is_empty() {
        serial_println!("[sdhc] no SD host controller found (class 0x08/0x05)");
        return;
    }

    for (bus, slot, func) in controllers {
        // Config space first: the raw BAR0 word (to distinguish an I/O BAR and an unassigned one)
        // and COMMAND (bit 1 = Memory Space Enable). Reads only.
        let bar0_raw = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x10) };
        let command = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x04) };
        let mem_decode = (command & 0x0002) != 0;
        let vendor = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x00) };
        let device = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x02) };
        let bar0 = PciScanner::get_bar_address(bus, slot, func);

        serial_println!(
            "[sdhc] bdf {}:{}.{} {:04x}:{:04x} bar0={:#x} cmd={:#06x} mem-decode={}",
            bus, slot, func, vendor, device, bar0, command, mem_decode as u8
        );

        if (bar0_raw & 0x1) != 0 {
            serial_println!(
                "[sdhc] bdf {}:{}.{} bar0 is an I/O BAR (io={:#x}) — MMIO probe skipped",
                bus, slot, func, bar0_raw & 0xFFFF_FFFC
            );
            continue;
        }
        if bar0 == 0 {
            serial_println!(
                "[sdhc] bdf {}:{}.{} bar0 unassigned by firmware — no MMIO probe",
                bus, slot, func
            );
            continue;
        }
        if !mem_decode {
            serial_println!(
                "[sdhc] bdf {}:{}.{} memory decode OFF (cmd={:#06x}) — MMIO probe skipped (read-only: we do not enable it)",
                bus, slot, func, command
            );
            continue;
        }

        // Identity-map the register block Uncacheable (PCD|PWT), the same way the GPU drivers map
        // their BARs. Creating a mapping is not a device access; the device sees nothing here.
        crate::arch::memory::map_mmio_window(bar0, SDHCI_MMIO_LEN);

        // Host Controller Version (0xFE, 16-bit): bits 7:0 = Specification Version Number,
        // bits 15:8 = Vendor Version Number.
        let hcver = unsafe { core::ptr::read_volatile((bar0 + REG_HOST_CTRL_VERSION) as *const u16) };
        let sver = (hcver & 0xFF) as u8;
        let vver = (hcver >> 8) as u8;
        serial_println!(
            "[sdhc] bdf {}:{}.{} hcver={:#06x} spec={} vendor-ver={:#04x}",
            bus, slot, func, hcver, spec_version_name(sver), vver
        );

        // Capabilities (0x40) and Capabilities_2 (0x44), 32-bit each.
        let caps = unsafe { core::ptr::read_volatile((bar0 + REG_CAPABILITIES) as *const u32) };
        let caps2 = unsafe { core::ptr::read_volatile((bar0 + REG_CAPABILITIES_2) as *const u32) };
        // Base Clock Frequency For SD Clock: bits 15:8 in MHz (0 = "get it from another source").
        let base_clk = (caps >> 8) & 0xFF;
        let max_blk = match (caps >> 16) & 0x3 {
            0 => 512u32,
            1 => 1024,
            2 => 2048,
            _ => 0,
        };
        let slot_type = match (caps >> 30) & 0x3 {
            0 => "removable",
            1 => "embedded",
            2 => "shared-bus",
            _ => "reserved",
        };
        serial_println!(
            "[sdhc] bdf {}:{}.{} caps={:#010x} caps2={:#010x} base-clk={}MHz max-blk={} v3.3={} v3.0={} v1.8={} sdma={} adma2={} hispeed={} 64bit={} slot={}",
            bus, slot, func, caps, caps2, base_clk, max_blk,
            (caps >> 24) & 1, (caps >> 25) & 1, (caps >> 26) & 1,
            (caps >> 22) & 1, (caps >> 19) & 1, (caps >> 21) & 1, (caps >> 28) & 1,
            slot_type
        );

        // Present State (0x24): is a card actually in the slot right now? bit 16 Card Inserted,
        // bit 17 Card State Stable, bit 19 Write Protect Switch Pin Level (0 = write protected).
        let present = unsafe { core::ptr::read_volatile((bar0 + REG_PRESENT_STATE) as *const u32) };
        serial_println!(
            "[sdhc] bdf {}:{}.{} present={:#010x} card-inserted={} cd-stable={} write-protected={}",
            bus, slot, func, present,
            (present >> 16) & 1, (present >> 17) & 1, ((present >> 19) & 1) ^ 1
        );
    }
}
