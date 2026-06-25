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

/// Monotonic tick counter. Arch-neutral entry point (mirrors x86_64). aarch64 has no timer
/// heartbeat wired yet, so this is a stub returning 0 — the NIC (its only consumer) never runs
/// on aarch64 anyway (NET_DEVICE stays None), so no timing depends on it here.
pub fn ticks() -> u64 {
    0
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
