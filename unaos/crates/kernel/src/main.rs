#![no_std]
#![no_main]

extern crate alloc;

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
// Removed VirtAddr for now to simplify the boot path
use unaos_kernel::serial_println;

// ENTRY POINT
entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    // 1. Core Hardware Init (GDT, IDT, PICS)
    unaos_kernel::init();

    // 2. Serial Verification
    // We skip complex memory mapping for this specific heartbeat test
    // because bootloader 0.9.x fields vary by environment.
    serial_println!(":: UnaOS Kernel Initialized ::");
    serial_println!(":: Status: ONLINE ::");
    serial_println!(":: Architect Verified ::");

    // 3. Memory Map Telemetry (Optional Diagnostic)
    serial_println!(
        ":: Memory Regions Detected: {} ::",
        boot_info.memory_map.iter().count()
    );

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
