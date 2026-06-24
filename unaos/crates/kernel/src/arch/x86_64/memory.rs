// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.


use unaos_boot_info::{BootInfo, MemoryRegionKind};

pub fn init(boot_info: &'static mut BootInfo) {
    serial_println!(":: X86_64 Memory Init ::");
    
    let regions = unsafe {
        core::slice::from_raw_parts(
            boot_info.memory_regions_addr as *const unaos_boot_info::MemoryRegion,
            boot_info.memory_regions_len,
        )
    };
    
    let mut heap_start = 0;
    let mut heap_size = 0;
    
    for region in regions {
        if region.kind == MemoryRegionKind::Usable && region.phys_start > 0x100000 {
            let size = (region.page_count * 4096) as usize;
            if size >= crate::allocator::HEAP_SIZE {
                heap_start = region.phys_start;
                heap_size = crate::allocator::HEAP_SIZE;
                break;
            }
        }
    }
    
    if heap_size > 0 {
        unsafe {
            crate::allocator::init_heap_raw(heap_start as *mut u8, heap_size);
        }
    } else {
        serial_println!("Available memory regions: {}", regions.len());
        for region in regions.iter().take(15) {
            serial_println!("Kind: {:?}, Start: {:#x}, Pages: {}", region.kind, region.phys_start, region.page_count);
        }
        panic!("Failed to find usable memory for heap");
    }
}
