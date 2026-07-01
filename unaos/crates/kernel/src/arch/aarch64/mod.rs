#[macro_use]
pub mod serial;
pub mod cache;
pub mod percpu;
pub mod memory;
pub mod pci;
pub mod exceptions;
pub mod gic;
pub mod timer;
#[cfg(feature = "baremetal")]
pub mod boot;
#[cfg(feature = "baremetal")]
pub mod mailbox;
#[cfg(feature = "baremetal")]
pub mod smp;
#[cfg(feature = "baremetal")]
pub mod sched;

pub fn init() {
    serial_println!(":: AARCH64 Core Hardware Init ::");
    boot_diagnostics();
    // Per-CPU data for the boot core (TPIDR_EL2) before anything can take an interrupt, so an IRQ
    // handler that resolves `percpu::this_cpu()` (the SGI path) always has a valid block. Secondary
    // cores do their own `percpu::init` in `smp::__secondary_rust`.
    percpu::init(0);
    // Bring up interrupts, mirroring the x86 init order (IDT -> APIC -> timer -> sti):
    //   1. install the exception vectors (and, at EL2, route async exceptions to EL2);
    //   2. bring up the GICv2 (distributor + this core's CPU interface);
    //   3. arm the generic timer (enables its PPI at the GIC);
    //   4. unmask IRQ in PSTATE — the heartbeat starts here.
    exceptions::install();
    gic::init();
    timer::init();
    // One-shot self-test: confirm the timer asserts and the GIC latches its PPI before we unmask
    // (the analogue of the x86 APIC/SMP smoke tests; ~one tick period, invaluable on the
    // serial-less Pi where this rides the fbcon boot log).
    timer::diagnose();
    exceptions::enable_irq();
    // Confirm the timer IRQ actually delivers before committing the idle path to WFI. On hardware
    // where it doesn't (an unverified GIC routing), this leaves `hlt()` on its poll-spin fallback so
    // the GUI stays responsive instead of freezing in a wake-less WFI.
    timer::verify_live();
}

/// Read-only boot probe: dump the Exception Level we were handed off at, the generic-timer
/// frequency, the MMU state, and DAIF — all from system registers (zero MMIO, so it cannot fault
/// before exception vectors exist). This grounded the GIC/timer bring-up. Firmware hands every core
/// off at EL2; the bare-metal build then drops to EL1 (see `boot::drop_to_el1`) before this runs, so
/// it prints EL=1, while the UEFI/QEMU-virt build stays at EL2. CNTFRQ differs by board (QEMU
/// 62.5 MHz vs Pi 4 54 MHz), so the tick interval is computed from it at runtime.
fn boot_diagnostics() {
    let current_el: u64;
    let cntfrq: u64;
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) current_el, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) cntfrq, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, DAIF", out(reg) daif, options(nomem, nostack, preserves_flags));
    }
    // CurrentEL holds the EL in bits [3:2]. The MMU/cacheability is governed by SCTLR for the EL we
    // actually run at, so read SCTLR_EL2 when at EL2 (reading SCTLR_EL1 there would be misleading —
    // it describes a translation regime we aren't using).
    let el = (current_el >> 2) & 0b11;
    let sctlr: u64;
    unsafe {
        if el == 2 {
            core::arch::asm!("mrs {}, SCTLR_EL2", out(reg) sctlr, options(nomem, nostack, preserves_flags));
        } else {
            core::arch::asm!("mrs {}, SCTLR_EL1", out(reg) sctlr, options(nomem, nostack, preserves_flags));
        }
    }
    serial_println!(
        ":: AARCH64 boot diag: EL={}  CNTFRQ={} Hz  MMU={}  DAIF(DAIF)={:#06b} ::",
        el,
        cntfrq,
        if sctlr & 1 != 0 { "on" } else { "off" },
        (daif >> 6) & 0b1111,
    );
}

pub fn hlt_loop() -> ! {
    loop {
        hlt();
    }
}

pub fn hlt() {
    // Interrupt-driven idle WHEN the timer IRQ is confirmed delivering (`timer::verify_live`): WFI
    // parks the core until the next tick (it wakes on a pending physical interrupt even with
    // PSTATE.I set, so a panic-time hlt_loop still halts cleanly). If liveness was NOT confirmed —
    // an untested GIC path where the PPI never reaches the CPU — WFI would have no wake source and
    // sleep forever, freezing the polled main loop; fall back to a light poll-spin so input keeps
    // being serviced (the pre-interrupt behavior, trading idle power for liveness).
    if timer::is_live() {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack, preserves_flags)) };
    } else {
        core::hint::spin_loop();
    }
}

pub fn poll_input() -> Option<u8> {
    serial::SERIAL_PORT.lock().read_byte()
}

/// Make CPU writes to `[addr, addr+len)` visible to a non-coherent scan-out engine — the Pi 4's
/// VideoCore HVS reads the framebuffer straight from RAM and does not snoop the A72 data cache, so
/// after the GUI flushes pixels into a (cacheable) framebuffer they must be cleaned to the Point of
/// Coherency or the display shows stale rows. Called from the shared video path (`FrameBuffer`/
/// `fbcon`); a `DC CVAC` sweep on aarch64, a no-op on the cache-coherent x86 framebuffer and inert
/// in QEMU. See arch/aarch64/cache.rs for why this is mandatory on metal but invisible in QEMU.
#[inline]
pub fn flush_framebuffer_range(addr: usize, len: usize) {
    cache::clean_range(addr, len);
}

/// Monotonic tick counter since boot. Arch-neutral entry point (mirrors x86_64); now backed by the
/// generic-timer heartbeat (~250 Hz) rather than the old 0 stub.
pub fn ticks() -> u64 {
    timer::ticks()
}

/// Milliseconds since boot. Arch-neutral mirror of x86_64's `ms`, derived from the generic-timer
/// heartbeat: `ticks()` advances at `timer::TICK_HZ` (250 Hz = 4 ms/tick), so ms = ticks * 4.
#[inline]
pub fn ms() -> u64 {
    ticks() * 4
}

/// Free-running virtual cycle counter (CNTVCT_EL0). Monotonic and interrupt-flag-independent, like
/// x86 rdtsc — the portable timebase for bounding hardware busy-waits (see `now_cycles` on x86_64).
/// Runs at CNTFRQ_EL0 (~62.5 MHz under QEMU virt, 54 MHz on the Pi 4), NOT GHz, so its budget is in
/// its own units. The generic timer is up and delivery-confirmed on both paths (`timer::verify_live`),
/// and the bare-metal EL1 drop sets CNTVOFF_EL2=0 so CNTVCT shares the physical timebase (CNTPCT).
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
