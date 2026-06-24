#[macro_use]
pub mod serial;
pub mod memory;
pub mod pci;

pub fn init() {
    serial_println!(":: AARCH64 Core Hardware Init ::");
}

pub fn hlt_loop() -> ! {
    loop {
        hlt();
    }
}

pub fn hlt() {
    unsafe {
        // Polling mode for now, no WFE so we don't hang without interrupts
        core::arch::asm!("nop");
    }
}

pub fn poll_input() -> Option<u8> {
    serial::SERIAL_PORT.lock().read_byte()
}

pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    // Save DAIF, mask IRQ (daifset #2 = the I bit), run, then RESTORE the saved state. Restoring
    // (rather than blindly `daifclr`) keeps nested calls correct: an inner call must not re-enable
    // interrupts that an outer call had masked. Harmless today (aarch64 runs polled with no
    // interrupt sources) but correct for when a GIC/timer lands.
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags));
        core::arch::asm!("msr daifset, #2", options(nomem, nostack, preserves_flags));
    }
    let ret = f();
    unsafe {
        core::arch::asm!("msr daif, {}", in(reg) daif, options(nomem, nostack, preserves_flags));
    }
    ret
}
