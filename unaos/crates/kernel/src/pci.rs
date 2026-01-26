use x86_64::instructions::port::Port;
use crate::serial_println;

pub struct PciScanner;

impl PciScanner {
    pub fn scan() {
        serial_println!("PCI: Scanning...");
        // Bus 0..256 (u8 covers 0..255, loop should be 0..=255 or 0..256 with larger type)
        // bus is u16 in loop to reach 256, cast to u8
        for bus in 0u16..256 {
            for slot in 0u8..32 {
                // Check if device exists (Vendor ID at offset 0x00)
                let vendor_id = unsafe { Self::read_config_16(bus as u8, slot, 0, 0x00) };
                if vendor_id == 0xFFFF {
                    continue;
                }

                // Read Class/Subclass at offset 0x0A
                let class_word = unsafe { Self::read_config_16(bus as u8, slot, 0, 0x0A) };

                // High byte is Class, Low byte is Subclass
                let class_code = (class_word >> 8) as u8;
                let subclass = (class_word & 0xFF) as u8;

                serial_println!("CONTACT: [{:02x}:{:02x}:00] Class: {:02x} Sub: {:02x}",
                    bus, slot, class_code, subclass);

                if class_code == 0x0C && subclass == 0x03 {
                    serial_println!("TARGET LOCKED: USB xHCI");
                }
            }
        }
        serial_println!("PCI: Scan Complete.");
    }

    pub fn find_device(target_class: u8, target_subclass: u8) -> Option<(u8, u8, u8)> {
        for bus in 0u16..256 {
            for slot in 0u8..32 {
                let vendor_id = unsafe { Self::read_config_16(bus as u8, slot, 0, 0x00) };
                if vendor_id == 0xFFFF {
                    continue;
                }
                let class_word = unsafe { Self::read_config_16(bus as u8, slot, 0, 0x0A) };
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
        let bar0 = unsafe { Self::read_config_32(bus, slot, func, 0x10) };
        // Check Type (Bits 1-2). 0x00 = 32-bit, 0x02 = 64-bit
        let is_64bit = ((bar0 >> 1) & 0x03) == 0x02;
        let addr_low = bar0 & 0xFFFFFFF0;

        if is_64bit {
            let bar1 = unsafe { Self::read_config_32(bus, slot, func, 0x14) };
            (addr_low as u64) | ((bar1 as u64) << 32)
        } else {
            addr_low as u64
        }
    }

    pub fn enable_bus_master(bus: u8, slot: u8, func: u8) {
        let command_reg_offset = 0x04;
        let current_val = unsafe { Self::read_config_16(bus, slot, func, command_reg_offset) };
        // Bit 2 (0x4) = Bus Master, Bit 1 (0x2) = Memory Space
        unsafe { Self::write_config_16(bus, slot, func, command_reg_offset, current_val | 0x06) };
    }

    pub fn disable_interrupts(bus: u8, slot: u8, func: u8) {
        let command_reg_offset = 0x04;
        let current_val = unsafe { Self::read_config_16(bus, slot, func, command_reg_offset) };
        // Bit 10 (0x400) = Interrupt Disable (1 = Disabled)
        unsafe { Self::write_config_16(bus, slot, func, command_reg_offset, current_val | (1 << 10)) };
        serial_println!("xHCI: PCI Interrupts DISABLED (Bit 10 Set).");
    }

    unsafe fn read_config_32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
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

    /// Reads a 16-bit word from PCI Config Space.
    unsafe fn read_config_16(bus: u8, slot: u8, func: u8, offset: u8) -> u16 {
        let address_port: u16 = 0xCF8;
        let data_port: u16 = 0xCFC;

        // Construct Address
        let address = 0x8000_0000
            | ((bus as u32) << 16)
            | ((slot as u32) << 11)
            | ((func as u32) << 8)
            | ((offset as u32) & 0xFC);

        let mut addr_port = Port::<u32>::new(address_port);
        addr_port.write(address);

        // Read from Data Port
        let port_offset = (offset & 2) as u16;
        let mut data = Port::<u16>::new(data_port + port_offset);
        data.read()
    }

    unsafe fn write_config_16(bus: u8, slot: u8, func: u8, offset: u8, value: u16) {
        let address_port: u16 = 0xCF8;
        let data_port: u16 = 0xCFC;

        // Construct Address
        let address = 0x8000_0000
            | ((bus as u32) << 16)
            | ((slot as u32) << 11)
            | ((func as u32) << 8)
            | ((offset as u32) & 0xFC);

        let mut addr_port = Port::<u32>::new(address_port);
        addr_port.write(address);

        // Write to Data Port
        let port_offset = (offset & 2) as u16;
        let mut data = Port::<u16>::new(data_port + port_offset);
        data.write(value);
    }
}
