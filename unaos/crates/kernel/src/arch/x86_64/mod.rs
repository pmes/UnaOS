#[macro_use]
pub mod serial;
pub mod gdt;
pub mod interrupts;
pub mod pci;
pub mod memory;

pub fn init() {
    gdt::init();
    interrupts::init_idt();
    interrupts::init_pics();
    x86_64::instructions::interrupts::enable();
}

pub fn hlt_loop() -> ! {
    loop {
        hlt();
    }
}

pub fn hlt() {
    x86_64::instructions::hlt();
}

pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    x86_64::instructions::interrupts::without_interrupts(f)
}
