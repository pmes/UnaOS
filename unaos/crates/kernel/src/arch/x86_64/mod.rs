#[macro_use]
pub mod serial;
pub mod gdt;
pub mod interrupts;
pub mod apic;
pub mod acpi;
pub mod percpu;
pub mod smp;
pub mod pci;
pub mod memory;

pub fn init() {
    gdt::init();
    interrupts::init_idt();
    // Pure local-APIC system: silence the legacy 8259 PIC, then software-enable the local
    // APIC (timer heartbeat + spurious vector; LINT0 masked, LINT1=NMI) before enabling
    // interrupts. Input is USB-HID via the xHCI MSI-X path — no PS/2, no PIT, no I/O APIC.
    interrupts::disable_legacy_pic();
    apic::init();
    // Per-CPU data for the BSP (logical CPU 0). Must precede `sti` so the timer/IPI handlers
    // can resolve `this_cpu()` via the GS base.
    percpu::init_cpu(0, apic::apic_id_u32());
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
