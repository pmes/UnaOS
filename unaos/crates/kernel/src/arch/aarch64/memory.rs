use unaos_boot_info::{BootInfo, MemoryRegionKind};

pub fn init(boot_info: &'static mut BootInfo) {
    serial_println!(":: AARCH64 Memory Init ::");
    
    let regions = unsafe {
        core::slice::from_raw_parts(
            boot_info.memory_regions_addr as *const unaos_boot_info::MemoryRegion,
            boot_info.memory_regions_len,
        )
    };
    
    let mut heap_start = 0;
    let mut heap_size = 0;
    
    for region in regions {
        if region.kind == MemoryRegionKind::Usable {
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
        panic!("Failed to find usable memory for heap");
    }
}
