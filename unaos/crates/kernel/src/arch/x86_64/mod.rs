#[macro_use]
pub mod serial;
pub mod gdt;
pub mod interrupts;
pub mod apic;
pub mod pci;
pub mod memory;

pub fn init() {
    gdt::init();
    interrupts::init_idt();
    interrupts::init_pics();
    // Software-enable the local APIC. Must come after init_pics() (which sets up the 8259)
    // because enabling the APIC reroutes the PIC's INTR through LINT0 (ExtINT); apic::init()
    // programs that LVT entry so the legacy timer/keyboard keep working during transition.
    apic::init();
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
