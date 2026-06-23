#[macro_use]
pub mod serial;
pub mod memory;
pub mod pci;

pub fn init() {
    // TODO: AArch64 initialization (GIC, etc.)
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
    // TODO: Implement actual interrupt disabling for aarch64
    f()
}
