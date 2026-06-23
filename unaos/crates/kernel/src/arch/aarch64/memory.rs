use unaos_boot_info::BootInfo;

pub fn init(_boot_info: &'static mut BootInfo) {
    serial_println!(":: AARCH64 Memory Init ::");
    // TODO: Parse the UEFI memory map
    // TODO: Set up TTBR0, TCR, MAIR, and enable MMU
    // TODO: Initialize the global heap allocator
}
