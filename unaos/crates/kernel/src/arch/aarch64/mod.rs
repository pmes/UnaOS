#[macro_use]
pub mod serial;
pub mod memory;
pub mod pci;
pub mod exceptions;
pub mod gic;
pub mod timer;

pub fn init() {
    serial_println!(":: AARCH64 Core Hardware Init ::");
    boot_diagnostics();
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
}

/// Read-only boot probe: dump the Exception Level we were handed off at, the generic-timer
/// frequency, the MMU state, and DAIF — all from system registers (zero MMIO, so it cannot fault
/// before exception vectors exist). This grounded the GIC/timer bring-up: UEFI hands us off at EL2
/// (so vectors install at VBAR_EL2 and async exceptions are routed to EL2), and CNTFRQ differs by
/// board (QEMU 62.5 MHz vs Pi 4 54 MHz), so the tick interval is computed from it at runtime.
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
    unsafe {
        // Interrupt-driven idle: wait for an interrupt. The generic-timer heartbeat (and any future
        // GIC source) wakes us; WFI wakes on a pending physical interrupt even if PSTATE.I is set,
        // so a panic-time hlt_loop still halts cleanly. This replaces the old busy `nop` spin now
        // that there's a real interrupt source.
        core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
    }
}

pub fn poll_input() -> Option<u8> {
    serial::SERIAL_PORT.lock().read_byte()
}

/// Monotonic tick counter since boot. Arch-neutral entry point (mirrors x86_64); now backed by the
/// generic-timer heartbeat (~250 Hz) rather than the old 0 stub.
pub fn ticks() -> u64 {
    timer::ticks()
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
