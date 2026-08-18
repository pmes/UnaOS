// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una



// Fallback logic for ECAM base. We try Highmem first, then Lowmem.
const ECAM_BASE_HIGH: usize = 0x40_1000_0000;
const ECAM_BASE_LOW: usize = 0x3f00_0000;

static mut ACTIVE_ECAM_BASE: usize = 0;

fn get_ecam_address(bus: u8, slot: u8, func: u8, offset: u8) -> usize {
    unsafe {
        ACTIVE_ECAM_BASE
            | ((bus as usize) << 20)
            | ((slot as usize) << 15)
            | ((func as usize) << 12)
            | (offset as usize)
    }
}

pub unsafe fn read_config_32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    let addr = get_ecam_address(bus, slot, func, offset);
    core::ptr::read_volatile(addr as *const u32)
}

pub unsafe fn read_config_16(bus: u8, slot: u8, func: u8, offset: u8) -> u16 {
    let addr = get_ecam_address(bus, slot, func, offset);
    core::ptr::read_volatile(addr as *const u16)
}

pub unsafe fn write_config_16(bus: u8, slot: u8, func: u8, offset: u8, value: u16) {
    let addr = get_ecam_address(bus, slot, func, offset);
    core::ptr::write_volatile(addr as *mut u16, value);
}

pub fn init(dtb_addr: u64, dtb_size: usize) {
    serial_println!(":: AARCH64 PCI Init ::");

    let mut found_ecam = false;

    if dtb_addr != 0 {
        unsafe {
            let dtb_slice = core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size);
            if let Ok(fdt) = fdt::Fdt::new(dtb_slice) {
                // Find node with compatible = "pci-host-ecam-generic"
                for node in fdt.all_nodes() {
                    if let Some(compatible) = node.property("compatible") {
                        let comp_str = core::str::from_utf8(compatible.value).unwrap_or("");
                        if comp_str.contains("pci-host-ecam-generic") {
                            if let Some(reg) = node.property("reg") {
                                // "reg" contains (base_addr, size) based on address/size cells.
                                // In QEMU virt, typically 2 cells for addr, 2 for size.
                                // We can just read the first 8 bytes if #address-cells = 2.
                                // fdt crate provides reg iterator.
                                if let Some(first_reg) = node.reg().and_then(|mut r| r.next()) {
                                    ACTIVE_ECAM_BASE = first_reg.starting_address as usize;
                                    let base = ACTIVE_ECAM_BASE;
                                    serial_println!(":: DTB Parsed: Found PCIe ECAM at {:#x} ::", base);
                                    found_ecam = true;
                                    break;
                                }
                            }
                        }
                    }
                }
            } else {
                serial_println!(":: WARNING: Failed to parse DTB at {:#x} ::", dtb_addr);
            }
        }
    }

    if !found_ecam {
        // Fallback to hardcoded addresses
        unsafe {
            ACTIVE_ECAM_BASE = ECAM_BASE_HIGH;
            let vendor_id = read_config_16(0, 0, 0, 0);
            if vendor_id == 0xFFFF || vendor_id == 0 {
                ACTIVE_ECAM_BASE = ECAM_BASE_LOW;
                let vendor_id_low = read_config_16(0, 0, 0, 0);
                let base = ACTIVE_ECAM_BASE;
                if vendor_id_low == 0xFFFF || vendor_id_low == 0 {
                    serial_println!(":: WARNING: Could not find valid PCIe ECAM ::");
                } else {
                    serial_println!(":: Using Lowmem ECAM base: {:#x} ::", base);
                }
            } else {
                let base = ACTIVE_ECAM_BASE;
                serial_println!(":: Using Highmem ECAM base: {:#x} ::", base);
            }
        }
    }

    if let Some((xhci_phys_addr, bus, dev, func)) = crate::drivers::pci::PciScanner::scan() {
        serial_println!(":: AARCH64 PCI Init: Found xHCI at {:#x} ::", xhci_phys_addr);

        // Enable PCI Memory Space + Bus Master (DMA) for the controller.
        crate::drivers::pci::PciScanner::enable_bus_master(bus, dev, func);

        // Initialize xHCI
        crate::drivers::xhci::init(xhci_phys_addr); // Reset and command ring

        unsafe {
            let mut xhci = crate::drivers::xhci::XhciController::new(xhci_phys_addr as usize);

            // Allocate the global rings, capture their physical addresses, then DROP
            // the guards before start()/poll to avoid any re-lock deadlock.
            let (event_ring_phys, command_ring_phys) = {
                let mut cmd_ring_guard = crate::drivers::xhci::COMMAND_RING.lock();
                let mut evt_ring_guard = crate::drivers::xhci::EVENT_RING.lock();

                *cmd_ring_guard = Some(crate::drivers::xhci::ring::TransferRing::new(256));
                *evt_ring_guard = Some(crate::drivers::xhci::event::EventRing::new());

                (evt_ring_guard.as_mut().unwrap().get_ptr(),
                 cmd_ring_guard.as_mut().unwrap().get_ptr())
            };

            // ERST is heap-allocated inside init_interrupter (JETSON-XCARVE: no xHC DMA structure in .bss).
            xhci.init_interrupter(event_ring_phys);
            xhci.init_pointers(command_ring_phys);
            xhci.start();

            // Store globally
            crate::drivers::xhci::install(xhci);
        }
    }
}
