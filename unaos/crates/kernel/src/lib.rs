#![no_std]
#![cfg_attr(test, no_main)]
#![feature(abi_x86_interrupt)]

extern crate alloc;

#[macro_use]
pub mod serial;

pub mod allocator;
pub mod gdt;
pub mod interrupts;
pub mod memory;
pub mod pal;
pub mod writer;

pub mod console;
pub mod user;
pub mod vug;

// Stubs
pub mod pci {
    pub struct PciScanner;
    impl PciScanner {
        pub fn scan() {}
    }
}
pub mod xhci {}

pub fn init() {
    gdt::init();
    interrupts::init_idt();
    unsafe { interrupts::PICS.lock().initialize() };

    enable_sse();

    x86_64::instructions::interrupts::enable();
}

fn enable_sse() {
    use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};
    unsafe {
        let mut cr0 = Cr0::read();
        // FIX: Correct flag name
        cr0.remove(Cr0Flags::EMULATE_COPROCESSOR);
        cr0.insert(Cr0Flags::MONITOR_COPROCESSOR);
        Cr0::write(cr0);

        let mut cr4 = Cr4::read();
        cr4.insert(Cr4Flags::OSFXSR);
        cr4.insert(Cr4Flags::OSXMMEXCPT_ENABLE);
        Cr4::write(cr4);
    }
}

pub fn hlt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}
