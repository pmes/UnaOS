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

/// Milliseconds since boot. Arch-neutral mirror of x86_64's `ms`; aarch64 has no timer heartbeat
/// wired yet (like `ticks`), so it returns 0 for now.
#[inline]
pub fn ms() -> u64 {
    0
}

/// Free-running virtual cycle counter (CNTVCT_EL0). Monotonic and interrupt-flag-independent, like
/// x86 rdtsc — the portable timebase for bounding hardware busy-waits (see `now_cycles` on x86_64).
/// Runs at CNTFRQ_EL0 (~62.5 MHz under QEMU virt), NOT GHz, so its budget is in its own units.
/// NOTE: assumes the generic-timer counter is enabled at boot. That holds under QEMU virt (where
/// xHCI is exercised), but no GIC/timer is wired up for bare-metal ARM yet, so this must be
/// re-verified there before any metal xHCI path relies on it.
#[inline]
pub fn now_cycles() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack, preserves_flags));
    }
    v
}

/// Busy-wait budget in `now_cycles()` (CNTVCT) units. ~2.5 s at a ~60 MHz generic-timer rate.
pub const HW_WAIT_BUDGET: u64 = 150_000_000;

/// Busy-wait budget in `now_cycles()` (CNTVCT) units. Arch-neutral mirror of x86_64's
/// `hw_wait_budget`; aarch64 has no PM-timer calibration path, so it returns the fixed budget.
/// (CNTFRQ_EL0 gives the exact CNTVCT rate and could refine this later.)
#[inline]
pub fn hw_wait_budget() -> u64 {
    HW_WAIT_BUDGET
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
