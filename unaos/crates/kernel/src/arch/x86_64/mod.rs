#[macro_use]
pub mod serial;
pub mod gdt;
pub mod interrupts;
pub mod apic;
pub mod acpi;
pub mod percpu;
pub mod smp;
pub mod sched;
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

/// Monotonic tick counter since boot (the local-APIC timer heartbeat). Arch-neutral entry
/// point used by drivers for coarse timing (e.g. TCP retransmission RTO); the absolute rate
/// is uncalibrated but steady.
pub fn ticks() -> u64 {
    apic::ticks()
}

/// Free-running CPU cycle counter (rdtsc). Invariant on Nehalem and later (incl. the Ivy Bridge
/// MacBookPro10,1): a constant rate across P-/C-/T-states, and — unlike `ticks()` — it advances
/// regardless of EFLAGS.IF or whether the APIC-timer ISR runs. The absolute rate is unknown (no
/// CPUID leaf 0x15/0x16 before Skylake), so callers compare against a fixed cycle budget
/// (`HW_WAIT_BUDGET`) instead of converting to seconds. Used to bound hardware busy-waits with a
/// real wall-clock deadline rather than an iteration count.
#[inline]
pub fn now_cycles() -> u64 {
    // SAFETY: RDTSC has no preconditions at ring 0; it is gated only by CR4.TSD (a ring-3
    // restriction we never set), never by the interrupt flag.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Busy-wait budget in `now_cycles()` (rdtsc) units. 2.5e9 cycles ≈ [0.5 s, 2.5 s] across 1–5 GHz
/// parts (~1.1 s at a 2.3 GHz Ivy Bridge base) and ≈2.5 s under QEMU/TCG (default 1 GHz vCPU TSC):
/// long enough that a healthy controller's µs-scale handshakes never trip it, short enough that a
/// wedged status bit fails fast instead of looking frozen on a serial-less laptop.
pub const HW_WAIT_BUDGET: u64 = 2_500_000_000;

pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    x86_64::instructions::interrupts::without_interrupts(f)
}
