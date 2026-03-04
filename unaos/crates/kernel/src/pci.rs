use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::instructions::port::Port;
use crate::serial_println;

pub struct PciPort {
    address_port: Port<u32>,
    data_port: Port<u32>,
}

impl PciPort {
    pub const fn new() -> Self {
        PciPort {
            address_port: Port::new(0xCF8),
            data_port: Port::new(0xCFC),
        }
    }
}

lazy_static! {
    static ref PCI_CONFIG: Mutex<PciPort> = Mutex::new(PciPort::new());
}

pub struct PciScanner;

impl PciScanner {
    pub fn read_word(bus: u8, device: u8, func: u8, offset: u8) -> u32 {
        let address = 0x8000_0000
            | ((bus as u32) << 16)
            | ((device as u32) << 11)
            | ((func as u32) << 8)
            | ((offset as u32) & 0xFC);

        x86_64::instructions::interrupts::without_interrupts(|| {
            let mut ports = PCI_CONFIG.lock();
            unsafe {
                ports.address_port.write(address);
                ports.data_port.read()
            }
        })
    }

    pub fn enumerate_buses() -> Option<u64> {
        serial_println!("PCI: Commencing motherboard scan...");

        for bus in 0..=255 {
            for device in 0..=31 {
                let vendor_id_reg = Self::read_word(bus, device, 0, 0x00);
                let vendor_id = (vendor_id_reg & 0xFFFF) as u16;

                if vendor_id == 0xFFFF {
                    continue;
                }

                let header_type_reg = Self::read_word(bus, device, 0, 0x0C);
                let header_type = ((header_type_reg >> 16) & 0xFF) as u8;
                let is_multi_function = (header_type & 0x80) != 0;

                let max_func = if is_multi_function { 7 } else { 0 };

                for func in 0..=max_func {
                    if func != 0 {
                        let vendor_id_reg = Self::read_word(bus, device, func, 0x00);
                        if (vendor_id_reg & 0xFFFF) as u16 == 0xFFFF {
                            continue;
                        }
                    }

                    let class_reg = Self::read_word(bus, device, func, 0x08);
                    let class_code = ((class_reg >> 24) & 0xFF) as u8;
                    let subclass = ((class_reg >> 16) & 0xFF) as u8;
                    let prog_if = ((class_reg >> 8) & 0xFF) as u8;

                    if class_code == 0x0C && subclass == 0x03 && prog_if == 0x30 {
                        // Found XHCI
                        return Some(Self::read_bar0(bus, device, func));
                    }
                }
            }
        }

        None
    }

    pub fn scan() -> Option<u64> {
        if let Some(addr) = Self::enumerate_buses() {
            serial_println!("[PCI] FOUND XHCI CONTROLLER AT PHYSICAL ADDRESS: 0x{:X}", addr);
            Some(addr)
        } else {
            serial_println!("[PCI] WARNING: XHCI CONTROLLER NOT FOUND");
            None
        }
    }

    fn read_bar0(bus: u8, device: u8, func: u8) -> u64 {
        let bar0 = Self::read_word(bus, device, func, 0x10);
        let is_64bit = (bar0 & 0x06) == 0x04; // Bits 1-2. 0x00 = 32-bit, 0x04 = 64-bit (Type 02 = 10b => bit 1 is 0, bit 2 is 1 => 4) Wait, 0x2 memory type is bits 1-2. 0x2 << 1 is 0x4.
        let addr_low = bar0 & 0xFFFFFFF0;

        if is_64bit {
            let bar1 = Self::read_word(bus, device, func, 0x14);
            (addr_low as u64) | ((bar1 as u64) << 32)
        } else {
            addr_low as u64
        }
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
