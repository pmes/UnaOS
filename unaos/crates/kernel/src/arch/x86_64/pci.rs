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

pub fn init(_dtb_addr: u64, _dtb_size: usize) {
    if let Some(xhci_phys_addr) = crate::drivers::pci::PciScanner::scan() {
        serial_println!(":: x86_64 PCI Init: Found xHCI at {:#x} ::", xhci_phys_addr);
        
        // Initialize xHCI
        crate::drivers::xhci::init(xhci_phys_addr); // Reset and command ring
        
        unsafe {
            let mut xhci = crate::drivers::xhci::XhciController::new(xhci_phys_addr as usize);
            
            // Initialize globals
            let mut cmd_ring_guard = crate::drivers::xhci::COMMAND_RING.lock();
            let mut evt_ring_guard = crate::drivers::xhci::EVENT_RING.lock();

            *cmd_ring_guard = Some(crate::drivers::xhci::ring::TransferRing::new(256));
            *evt_ring_guard = Some(crate::drivers::xhci::event::EventRing::new());

            let event_ring_phys = evt_ring_guard.as_mut().unwrap().get_ptr();
            let erst_table_phys = &raw mut crate::drivers::xhci::ERST_TABLE as u64;
            xhci.init_interrupter(event_ring_phys, erst_table_phys);
            
            let command_ring_phys = cmd_ring_guard.as_mut().unwrap().get_ptr();
            xhci.init_pointers(command_ring_phys);
            xhci.start();
            
            // Store globally
            *crate::drivers::xhci::XHCI_CONTROLLER.lock() = Some(xhci);
        }
    }
}
