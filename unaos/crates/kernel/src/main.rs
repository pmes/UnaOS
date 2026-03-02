#![no_std]
#![no_main]

extern crate alloc;

use bootloader_api::{entry_point, BootInfo};
use core::panic::PanicInfo;
use x86_64::VirtAddr;
use unaos_kernel::serial_println;

// ENTRY POINT
entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // 1. Core Hardware Init (GDT, IDT, PICS)
    unaos_kernel::init();

    // 2. Serial Verification
    serial_println!(":: UNAOS KERNEL AWAKE ::");

    // 3. Global Heap Allocation (Phase 3 Memory Translation)
    let physical_memory_offset = boot_info.physical_memory_offset.into_option().unwrap();
    let phys_offset = VirtAddr::new(physical_memory_offset);

    let mut mapper = unsafe { unaos_kernel::memory::init(phys_offset) };
    let mut frame_allocator = unsafe { unaos_kernel::memory::BootInfoFrameAllocator::init(&boot_info.memory_regions) };

    unaos_kernel::allocator::init_heap(&mut mapper, &mut frame_allocator)
        .expect("Heap initialization failed");
    serial_println!(":: KERNEL HEAP ALLOCATED ::");

    // 4. Motherboard Hardware Interconnects
    unaos_kernel::pci::PciScanner::scan();

    // 5. Framebuffer Init
    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        let info = framebuffer.info();
        let buffer = framebuffer.buffer_mut();
        unaos_kernel::vug::init(buffer, info);
    } else {
        serial_println!(":: WARNING: No framebuffer detected ::");
    }

    loop {
        // Halt the CPU until the next interrupt
        x86_64::instructions::hlt();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("{}", info);
    unaos_kernel::hlt_loop();
}
