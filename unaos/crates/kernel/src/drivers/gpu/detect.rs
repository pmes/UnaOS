use alloc::vec::Vec;
use crate::drivers::pci::PciScanner;

pub struct GpuInfo {
    pub vendor: u16,
    pub device: u16,
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
    pub bar0_phys: u64,
    pub bar0_size: u64,
    pub gpu_type: GpuType,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GpuType {
    NvidiaKepler,
    IntelIvyBridge,
    Unknown,
}

pub fn detect_gpus() -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    serial_println!("[GPU] Scanning for Display Controllers (Class 0x03)...");

    for bus in 0u16..256 {
        for slot in 0u8..32 {
            for func in 0u8..8 {
                let vendor_id = unsafe { crate::arch::pci::read_config_16(bus as u8, slot, func, 0x00) };
                if vendor_id == 0xFFFF {
                    if func == 0 {
                        break; // No device at this slot
                    } else {
                        continue;
                    }
                }

                let class_reg = unsafe { crate::arch::pci::read_config_32(bus as u8, slot, func, 0x08) };
                let class_code = ((class_reg >> 24) & 0xFF) as u8;
                let _subclass = ((class_reg >> 16) & 0xFF) as u8;

                if class_code == 0x03 {
                    let device_id = unsafe { crate::arch::pci::read_config_16(bus as u8, slot, func, 0x02) };
                    
                    let mut gpu_type = GpuType::Unknown;
                    if vendor_id == 0x10DE && device_id == 0x0FD5 {
                        gpu_type = GpuType::NvidiaKepler;
                    } else if vendor_id == 0x8086 && device_id == 0x0166 {
                        gpu_type = GpuType::IntelIvyBridge;
                    }

                    // Get BAR0 address
                    let bar0_phys = PciScanner::get_bar_address(bus as u8, slot, func);
                    let bar0_size = get_bar_size(bus as u8, slot, func, 0x10);

                    serial_println!("[GPU] Found: {:?} (Vendor: {:04X}, Device: {:04X}) at bus {} slot {} func {}", 
                        gpu_type, vendor_id, device_id, bus, slot, func);
                    serial_println!("[GPU] BAR0: 0x{:X} (Size: {} bytes)", bar0_phys, bar0_size);

                    gpus.push(GpuInfo {
                        vendor: vendor_id,
                        device: device_id,
                        bus: bus as u8,
                        slot,
                        func,
                        bar0_phys,
                        bar0_size,
                        gpu_type,
                    });

                    // If it's a single-function device, stop checking other functions
                    let header_type_reg = unsafe { crate::arch::pci::read_config_32(bus as u8, slot, 0, 0x0C) };
                    let header_type = ((header_type_reg >> 16) & 0xFF) as u8;
                    let is_multi_function = (header_type & 0x80) != 0;
                    if !is_multi_function {
                        break;
                    }
                }
            }
        }
    }
    
    gpus
}

fn get_bar_size(bus: u8, slot: u8, func: u8, offset: u8) -> u64 {
    unsafe {
        // Read original BAR
        let original_bar = crate::arch::pci::read_config_32(bus, slot, func, offset);
        
        // Write all 1s
        crate::arch::pci::write_config_32(bus, slot, func, offset, 0xFFFFFFFF);
        
        // Read size mask
        let size_mask = crate::arch::pci::read_config_32(bus, slot, func, offset);
        
        // Restore original BAR
        crate::arch::pci::write_config_32(bus, slot, func, offset, original_bar);
        
        // Check Type (Bits 1-2). 0x00 = 32-bit, 0x04 = 64-bit
        let is_64bit = (original_bar & 0x06) == 0x04;
        
        if is_64bit {
            let offset_high = offset + 4;
            let original_bar_high = crate::arch::pci::read_config_32(bus, slot, func, offset_high);
            crate::arch::pci::write_config_32(bus, slot, func, offset_high, 0xFFFFFFFF);
            let size_mask_high = crate::arch::pci::read_config_32(bus, slot, func, offset_high);
            crate::arch::pci::write_config_32(bus, slot, func, offset_high, original_bar_high);
            
            let full_mask = ((size_mask_high as u64) << 32) | ((size_mask & 0xFFFFFFF0) as u64);
            (!full_mask).wrapping_add(1)
        } else {
            let mask = size_mask & 0xFFFFFFF0;
            (!mask).wrapping_add(1) as u64
        }
    }
}
