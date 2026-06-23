#[macro_use]
pub mod serial;
pub mod memory;
pub mod pci;

pub fn init() {
    serial_println!(":: AARCH64 Core Hardware Init ::");
}

pub fn hlt_loop() -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfe");
        }
    }
}

pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    unsafe { core::arch::asm!("msr daifset, #2"); }
    let ret = f();
    unsafe { core::arch::asm!("msr daifclr, #2"); }
    ret
}
