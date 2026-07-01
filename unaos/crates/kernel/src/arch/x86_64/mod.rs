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
/// point used by drivers and the scheduler for coarse timing (e.g. TCP retransmission RTO,
/// `sleep_ticks`). Once the timer is calibrated against the ACPI PM timer (see `apic::calibrate`)
/// the tick runs at a real 1 kHz — i.e. one tick per millisecond — on any machine; before
/// calibration it is steady but uncalibrated (~0.8 ms/tick under QEMU).
pub fn ticks() -> u64 {
    apic::ticks()
}

/// Milliseconds since boot. The calibrated APIC tick is 1 kHz, so this is the tick count directly;
/// before calibration it degrades to the raw (~1 ms) tick count. Arch-neutral (mirrors aarch64).
#[inline]
pub fn ms() -> u64 {
    apic::ticks()
}

/// Free-running CPU cycle counter (rdtsc). Invariant on Nehalem and later (incl. the Ivy Bridge
/// MacBookPro10,1): a constant rate across P-/C-/T-states, and — unlike `ticks()` — it advances
/// regardless of EFLAGS.IF or whether the APIC-timer ISR runs. `apic::calibrate` measures its
/// absolute rate against the ACPI PM timer (there is no CPUID leaf 0x15/0x16 before Skylake); use
/// `hw_wait_budget()` for a wall-clock deadline in these units.
#[inline]
pub fn now_cycles() -> u64 {
    // SAFETY: RDTSC has no preconditions at ring 0; it is gated only by CR4.TSD (a ring-3
    // restriction we never set), never by the interrupt flag.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Wall-clock seconds a hardware busy-wait may burn before it is treated as wedged.
const HW_WAIT_SECONDS: u64 = 2;

/// Fixed fallback busy-wait budget in `now_cycles()` (rdtsc) units, used before/without calibration.
/// 2.5e9 cycles ≈ [0.5 s, 2.5 s] across 1–5 GHz parts (~1.1 s at a 2.3 GHz Ivy Bridge base) and
/// ≈2.5 s under QEMU/TCG: long enough that a healthy controller's µs-scale handshakes never trip
/// it, short enough that a wedged status bit fails fast instead of looking frozen on a serial-less
/// laptop.
pub const HW_WAIT_BUDGET: u64 = 2_500_000_000;

/// Busy-wait budget in `now_cycles()` (rdtsc) units. Once the TSC is calibrated this is an honest
/// wall-clock `HW_WAIT_SECONDS` (`tsc_hz * seconds`); before calibration (or if it failed / no PM
/// timer) it falls back to the fixed `HW_WAIT_BUDGET` guess. Callers pass this to `wait_until`.
#[inline]
pub fn hw_wait_budget() -> u64 {
    let hz = apic::tsc_hz();
    if hz != 0 {
        hz.saturating_mul(HW_WAIT_SECONDS)
    } else {
        HW_WAIT_BUDGET
    }
}

pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    x86_64::instructions::interrupts::without_interrupts(f)
}
