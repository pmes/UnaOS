// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una



pub struct PciScanner;

impl PciScanner {
    /// Scan for the xHCI controller. Returns `(bar_phys_addr, bus, device, function)`
    /// so callers can both map the BAR and reach the device's config space (e.g. to
    /// enable Bus Master).
    pub fn scan() -> Option<(u64, u8, u8, u8)> {
        if let Some(found) = Self::enumerate_buses() {
            serial_println!("[PCI] FOUND XHCI CONTROLLER AT PHYSICAL ADDRESS: 0x{:X} (bus {} dev {} fn {})",
                found.0, found.1, found.2, found.3);
            Some(found)
        } else {
            serial_println!("[PCI] WARNING: XHCI CONTROLLER NOT FOUND");
            None
        }
    }

    pub fn enumerate_buses() -> Option<(u64, u8, u8, u8)> {
        serial_println!("PCI: Commencing motherboard scan...");

        for bus in 0..=255 {
            for device in 0..=31 {
                let vendor_id = unsafe { crate::arch::pci::read_config_16(bus, device, 0, 0x00) };

                if vendor_id == 0xFFFF {
                    continue;
                }

                let header_type_reg = unsafe { crate::arch::pci::read_config_32(bus, device, 0, 0x0C) };
                let header_type = ((header_type_reg >> 16) & 0xFF) as u8;
                let is_multi_function = (header_type & 0x80) != 0;

                let max_func = if is_multi_function { 7 } else { 0 };

                for func in 0..=max_func {
                    if func != 0 {
                        let vendor_id_reg = unsafe { crate::arch::pci::read_config_16(bus, device, func, 0x00) };
                        if vendor_id_reg == 0xFFFF {
                            continue;
                        }
                    }

                    let class_reg = unsafe { crate::arch::pci::read_config_32(bus, device, func, 0x08) };
                    let class_code = ((class_reg >> 24) & 0xFF) as u8;
                    let subclass = ((class_reg >> 16) & 0xFF) as u8;
                    let prog_if = ((class_reg >> 8) & 0xFF) as u8;

                    if class_code == 0x0C && subclass == 0x03 && prog_if == 0x30 {
                        // Found XHCI
                        return Some((Self::get_bar_address(bus, device, func), bus, device, func));
                    }
                }
            }
        }

        None
    }

    pub fn find_device(target_class: u8, target_subclass: u8) -> Option<(u8, u8, u8)> {
        for bus in 0u16..256 {
            for slot in 0u8..32 {
                let vendor_id = unsafe { crate::arch::pci::read_config_16(bus as u8, slot, 0, 0x00) };
                if vendor_id == 0xFFFF {
                    continue;
                }
                let class_word = unsafe { crate::arch::pci::read_config_16(bus as u8, slot, 0, 0x0A) };
                let class_code = (class_word >> 8) as u8;
                let subclass = (class_word & 0xFF) as u8;

                if class_code == target_class && subclass == target_subclass {
                    return Some((bus as u8, slot, 0));
                }
            }
        }
        None
    }

    pub fn get_bar_address(bus: u8, slot: u8, func: u8) -> u64 {
        let bar0 = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x10) };
        // Check Type (Bits 1-2). 0x00 = 32-bit, 0x04 = 64-bit
        let is_64bit = (bar0 & 0x06) == 0x04;
        let addr_low = bar0 & 0xFFFFFFF0;

        if is_64bit {
            let bar1 = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x14) };
            (addr_low as u64) | ((bar1 as u64) << 32)
        } else {
            addr_low as u64
        }
    }

    pub fn enable_bus_master(bus: u8, slot: u8, func: u8) {
        let command_reg_offset = 0x04;
        let current_val = unsafe { crate::arch::pci::read_config_16(bus, slot, func, command_reg_offset) };
        // Bit 2 (0x4) = Bus Master, Bit 1 (0x2) = Memory Space
        unsafe { crate::arch::pci::write_config_16(bus, slot, func, command_reg_offset, current_val | 0x06) };
    }

    /// Read-only diagnostic: dump command/status, interrupt line+pin, and the PCI
    /// capability list (IDs) for a device. Used to plan interrupt-driven xHCI bring-up
    /// (legacy INTx IRQ number vs. MSI/MSI-X capability presence). No side effects.
    pub fn probe_irq_caps(bus: u8, slot: u8, func: u8) {
        unsafe {
            let cmd_sts = crate::arch::pci::read_config_32(bus, slot, func, 0x04);
            let command = (cmd_sts & 0xFFFF) as u16;
            let status = (cmd_sts >> 16) as u16;
            // 0x3C: byte0 = Interrupt Line (IRQ), byte1 = Interrupt Pin (1=INTA..4=INTD)
            let intr = crate::arch::pci::read_config_32(bus, slot, func, 0x3C);
            let int_line = (intr & 0xFF) as u8;
            let int_pin = ((intr >> 8) & 0xFF) as u8;
            serial_println!(
                "[PCI-PROBE] bdf {}:{}.{} COMMAND={:#06x} (IntxDisable bit10={}) STATUS={:#06x} (CapList bit4={})",
                bus, slot, func, command, (command >> 10) & 1, status, (status >> 4) & 1
            );
            serial_println!(
                "[PCI-PROBE] Interrupt Line (IRQ)={} ({:#x}), Interrupt Pin=INT{}",
                int_line, int_line, (b'A' + int_pin.saturating_sub(1)) as char
            );

            // Walk the capability list if present (Status bit 4).
            if (status & (1 << 4)) != 0 {
                let mut cap_ptr = (crate::arch::pci::read_config_32(bus, slot, func, 0x34) & 0xFF) as u8;
                let mut guard = 0u8;
                while cap_ptr != 0 && cap_ptr != 0xFF && guard < 32 {
                    let cap = crate::arch::pci::read_config_32(bus, slot, func, cap_ptr);
                    let cap_id = (cap & 0xFF) as u8;
                    let next = ((cap >> 8) & 0xFF) as u8;
                    let name = match cap_id {
                        0x01 => "PowerMgmt",
                        0x05 => "MSI",
                        0x10 => "PCIe",
                        0x11 => "MSI-X",
                        _ => "?",
                    };
                    serial_println!(
                        "[PCI-PROBE]   cap@{:#04x} id={:#04x} ({}) next={:#04x} word0={:#010x}",
                        cap_ptr, cap_id, name, next, cap
                    );
                    if cap_id == 0x11 {
                        // MSI-X: Message Control (cap+2), Table Offset/BIR (cap+4), PBA (cap+8)
                        let mc = ((cap >> 16) & 0xFFFF) as u16;
                        let table = crate::arch::pci::read_config_32(bus, slot, func, cap_ptr + 4);
                        let pba = crate::arch::pci::read_config_32(bus, slot, func, cap_ptr + 8);
                        serial_println!(
                            "[PCI-PROBE]     MSI-X TableSize={} (entries) Enable={} FuncMask={} TableOff/BIR={:#x} PBA={:#x}",
                            (mc & 0x7FF) + 1, (mc >> 15) & 1, (mc >> 14) & 1, table, pba
                        );
                    }
                    cap_ptr = next;
                    guard += 1;
                }
            }
        }
    }

    /// Enable MSI-X on a device and program table entry 0 to deliver interrupt `vector`
    /// to the interrupt controller at `msg_addr`. The message address is supplied by the
    /// caller (rather than hard-coded) so this stays architecture-neutral — on x86 it is the
    /// local APIC at `0xFEE00000 | (dest_id << 12)`; this code is also compiled on aarch64.
    ///
    /// The MSI-X table lives in MMIO at `bar0_phys + (TableOffset & !7)`; we require the
    /// table BIR to be 0 (the xHCI BAR0, already identity-mapped). Entry 0 is fully
    /// programmed (MsgAddrLo/Hi, MsgData, VectorControl=unmasked) BEFORE the MSI-X Enable bit
    /// is set, so the function can never emit a message from a half-written entry. Returns
    /// true on success. Lock-free: only PCI config-space and MMIO accesses.
    pub fn enable_msix(bus: u8, slot: u8, func: u8, bar0_phys: u64, msg_addr: u32, vector: u32) -> bool {
        unsafe {
            // Capability list must be present (Status bit 4).
            let status = (crate::arch::pci::read_config_32(bus, slot, func, 0x04) >> 16) as u16;
            if (status & (1 << 4)) == 0 {
                serial_println!("[MSI-X] {}:{}.{} has no capability list; cannot enable MSI-X.", bus, slot, func);
                return false;
            }

            // Walk the capability list for the MSI-X capability (id 0x11).
            let mut cap_ptr = (crate::arch::pci::read_config_32(bus, slot, func, 0x34) & 0xFF) as u8;
            let mut msix_cap = 0u8;
            let mut guard = 0u8;
            while cap_ptr != 0 && cap_ptr != 0xFF && guard < 32 {
                let cap = crate::arch::pci::read_config_32(bus, slot, func, cap_ptr);
                if (cap & 0xFF) as u8 == 0x11 {
                    msix_cap = cap_ptr;
                    break;
                }
                cap_ptr = ((cap >> 8) & 0xFF) as u8;
                guard += 1;
            }
            if msix_cap == 0 {
                serial_println!("[MSI-X] {}:{}.{} has no MSI-X capability (id 0x11).", bus, slot, func);
                return false;
            }

            // Table Offset / BIR at cap+0x04: BIR = bits 2:0, byte offset = bits 31:3.
            let table_off_bir = crate::arch::pci::read_config_32(bus, slot, func, msix_cap + 4);
            let bir = (table_off_bir & 0x7) as u8;
            let table_off = (table_off_bir & !0x7u32) as u64;
            if bir != 0 {
                serial_println!("[MSI-X] Table BIR is {} (only BAR0 supported); aborting.", bir);
                return false;
            }
            let table = bar0_phys + table_off;

            // Program entry 0 (16 bytes). Done while MSI-X Enable is still 0, so no message
            // is generated from these partial writes.
            core::ptr::write_volatile((table + 0x00) as *mut u32, msg_addr); // Message Address (low)
            core::ptr::write_volatile((table + 0x04) as *mut u32, 0);        // Message Address (high)
            core::ptr::write_volatile((table + 0x08) as *mut u32, vector);   // Message Data
            core::ptr::write_volatile((table + 0x0C) as *mut u32, 0);        // Vector Control: bit0=0 -> unmasked

            // Ensure all four table writes are committed before the Enable bit is set, so the
            // function can never latch a half-written entry (required ordering on weakly-
            // ordered arches; a no-op fence on x86's strongly-ordered UC MMIO).
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

            // Message Control (cap+0x02): set Enable (bit 15), clear Function Mask (bit 14).
            let mc = crate::arch::pci::read_config_16(bus, slot, func, msix_cap + 2);
            let mc = (mc | (1u16 << 15)) & !(1u16 << 14);
            crate::arch::pci::write_config_16(bus, slot, func, msix_cap + 2, mc);

            serial_println!(
                "[MSI-X] Enabled on {}:{}.{}: cap@{:#04x} table@{:#x} entry0(addr={:#x} data={:#x}) MsgCtl={:#06x}",
                bus, slot, func, msix_cap, table, msg_addr, vector, mc
            );
            true
        }
    }

    pub fn disable_interrupts(bus: u8, slot: u8, func: u8) {
        let command_reg_offset = 0x04;
        let current_val = unsafe { crate::arch::pci::read_config_16(bus, slot, func, command_reg_offset) };
        // Bit 10 (0x400) = Interrupt Disable (1 = Disabled)
        unsafe { crate::arch::pci::write_config_16(bus, slot, func, command_reg_offset, current_val | (1 << 10)) };
        serial_println!("xHCI: PCI Interrupts DISABLED (Bit 10 Set).");
    }
}
