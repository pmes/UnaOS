// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use x86_64::instructions::port::Port;


pub unsafe fn read_config_32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    let address_port: u16 = 0xCF8;
    let data_port: u16 = 0xCFC;

    let address = 0x8000_0000
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);

    let mut addr_port = Port::<u32>::new(address_port);
    addr_port.write(address);

    let mut data = Port::<u32>::new(data_port);
    data.read()
}

pub unsafe fn read_config_16(bus: u8, slot: u8, func: u8, offset: u8) -> u16 {
    let address_port: u16 = 0xCF8;
    let data_port: u16 = 0xCFC;

    let address = 0x8000_0000
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);

    let mut addr_port = Port::<u32>::new(address_port);
    addr_port.write(address);

    let port_offset = (offset & 2) as u16;
    let mut data = Port::<u16>::new(data_port + port_offset);
    data.read()
}

pub unsafe fn write_config_16(bus: u8, slot: u8, func: u8, offset: u8, value: u16) {
    let address_port: u16 = 0xCF8;
    let data_port: u16 = 0xCFC;

    let address = 0x8000_0000
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);

    let mut addr_port = Port::<u32>::new(address_port);
    addr_port.write(address);

    let port_offset = (offset & 2) as u16;
    let mut data = Port::<u16>::new(data_port + port_offset);
    data.write(value);
}

pub unsafe fn write_config_32(bus: u8, slot: u8, func: u8, offset: u8, value: u32) {
    let address = 0x8000_0000
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);

    let mut addr_port = Port::<u32>::new(0xCF8);
    addr_port.write(address);
    let mut data = Port::<u32>::new(0xCFC);
    data.write(value);
}

/// Intel PCH xHCI port routing. On Intel chipsets (e.g. Panther Point in the 2012 MacBook Pro
/// Retina) the USB2 ports are physically shared with an EHCI companion controller and default to
/// EHCI; the SuperSpeed lanes likewise need enabling. We route every *switchable* port to xHCI by
/// copying the routing-MASK registers (USB2PRM @ 0xD4 / USB3PRM @ 0xDC = "which ports CAN switch")
/// into the routing-SELECT registers (XUSB2PR @ 0xD0 / USB3_PSSEN @ 0xD8). Without this the xHCI
/// sees no devices on the shared ports. Gated to the specific Intel xHCI device ids that have an
/// EHCI companion; a clean no-op on everything else (QEMU's qemu-xhci is vendor 0x1b36, not Intel).
/// Mirrors Linux's `usb_enable_intel_xhci_ports`. Must run while the controller is halted, before
/// it starts.
fn enable_intel_xhci_ports(bus: u8, dev: u8, func: u8) {
    const XUSB2PR: u8 = 0xD0;
    const USB2PRM: u8 = 0xD4;
    const USB3_PSSEN: u8 = 0xD8;
    const USB3PRM: u8 = 0xDC;
    // Intel xHCI controllers whose USB2 ports are shared with an EHCI companion.
    const SWITCHABLE: &[u16] = &[
        0x1E31, // Panther Point (7-series — 2012 MacBook Pro Retina)
        0x8C31, // Lynx Point
        0x9C31, // Lynx Point-LP
        0x9CB1, // Wildcat Point-LP
        0x22B5, // Cherry View / Braswell
    ];

    unsafe {
        if read_config_16(bus, dev, func, 0x00) != 0x8086 {
            return; // not Intel
        }
        let device = read_config_16(bus, dev, func, 0x02);
        if !SWITCHABLE.contains(&device) {
            return; // Intel, but not a shared-port xHCI
        }

        let ss = read_config_32(bus, dev, func, USB3PRM);
        write_config_32(bus, dev, func, USB3_PSSEN, ss); // enable SuperSpeed on supported ports
        let usb2 = read_config_32(bus, dev, func, USB2PRM);
        write_config_32(bus, dev, func, XUSB2PR, usb2); // route USB2 ports to xHCI

        serial_println!(
            ":: Intel xHCI port routing applied (dev {:#06x}): USB3_PSSEN={:#010x} XUSB2PR={:#010x} ::",
            device, ss, usb2
        );
    }
}

pub fn init(_dtb_addr: u64, _dtb_size: usize) {
    if let Some((xhci_phys_addr, bus, dev, func)) = crate::drivers::pci::PciScanner::scan() {
        serial_println!(":: x86_64 PCI Init: Found xHCI at {:#x} ::", xhci_phys_addr);

        // DIAGNOSTIC (read-only): dump interrupt line/pin + capability list to plan
        // interrupt-driven bring-up (INTx IRQ vs MSI-X).
        crate::drivers::pci::PciScanner::probe_irq_caps(bus, dev, func);

        // Enable PCI Memory Space + Bus Master (DMA) for the controller. Without Bus
        // Master the xHCI can never fetch command TRBs or write event TRBs.
        crate::drivers::pci::PciScanner::enable_bus_master(bus, dev, func);

        // Intel PCH quirk: route USB2/USB3 ports from the EHCI companion to xHCI before the
        // controller starts (else it sees no devices on shared ports). No-op on non-Intel (QEMU).
        enable_intel_xhci_ports(bus, dev, func);

        // Initialize xHCI
        crate::drivers::xhci::init(xhci_phys_addr); // Reset and command ring

        unsafe {
            let mut xhci = crate::drivers::xhci::XhciController::new(xhci_phys_addr as usize);

            // Allocate the global command + event rings, capture their physical
            // addresses, then DROP the guards before start()/poll so nothing can
            // deadlock by re-locking them later.
            let (event_ring_phys, command_ring_phys) = {
                let mut cmd_ring_guard = crate::drivers::xhci::COMMAND_RING.lock();
                let mut evt_ring_guard = crate::drivers::xhci::EVENT_RING.lock();

                *cmd_ring_guard = Some(crate::drivers::xhci::ring::TransferRing::new(256));
                *evt_ring_guard = Some(crate::drivers::xhci::event::EventRing::new());

                (evt_ring_guard.as_mut().unwrap().get_ptr(),
                 cmd_ring_guard.as_mut().unwrap().get_ptr())
            };

            let erst_table_phys = &raw mut crate::drivers::xhci::ERST_TABLE as u64;
            xhci.init_interrupter(event_ring_phys, erst_table_phys);

            // Route the controller's interrupts via MSI-X straight to the local APIC (no
            // 8259, no I/O APIC). init_interrupter just published the IR0/OP MMIO bases the
            // handler needs; IMAN.IE is set there and USBCMD.INTE in start(). The MSI message
            // targets the BSP local APIC (0xFEE00000 | dest_id<<12) at IDT vector 0x40.
            let msg_addr = 0xFEE0_0000u32 | ((crate::arch::apic::apic_id() as u32) << 12);
            crate::drivers::pci::PciScanner::enable_msix(
                bus, dev, func, xhci_phys_addr, msg_addr,
                crate::arch::interrupts::XHCI_MSI_VECTOR as u32,
            );

            xhci.init_pointers(command_ring_phys);
            xhci.start();

            // Store globally
            *crate::drivers::xhci::XHCI_CONTROLLER.lock() = Some(xhci);
        }
    }

    // Network controller (PCI class 0x02 = Network, subclass 0x00 = Ethernet).
    // QEMU's e1000 (82540EM) lands here; bring it up for polled RX.
    if let Some((bus, slot, func)) = crate::drivers::pci::PciScanner::find_device(0x02, 0x00) {
        serial_println!(
            ":: x86_64 PCI: Found network controller (class 0x02) at {}:{}.{} ::",
            bus, slot, func
        );
        crate::drivers::e1000::init(bus, slot, func);
        // Route the NIC's RX interrupt to the BSP local APIC via MSI (IDT vector 0x41),
        // the same local-APIC delivery the xHCI uses. The e1000e keeps its MSI-X table in
        // BAR3 (not mappable by enable_msix), so plain MSI is used.
        let msg_addr = 0xFEE0_0000u32 | ((crate::arch::apic::apic_id() as u32) << 12);
        crate::drivers::e1000::enable_interrupts(
            bus, slot, func, msg_addr,
            crate::arch::interrupts::NIC_MSI_VECTOR as u32,
        );
    } else {
        serial_println!(":: x86_64 PCI: No network controller (class 0x02) found ::");
    }
}
