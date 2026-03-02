#![no_std]
#![no_main]

extern crate alloc;

use bootloader_api::{entry_point, BootInfo};
use core::panic::PanicInfo;
use unaos_kernel::serial_println;

// ENTRY POINT
entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // 1. Core Hardware Init (GDT, IDT, PICS)
    unaos_kernel::init();

    // 2. Serial Verification
    serial_println!(":: UNAOS KERNEL AWAKE ::");

    // 3. Framebuffer Init
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
