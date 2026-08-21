// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// ARM generic timer — the aarch64 heartbeat, the analogue of the x86 local-APIC timer. It is the
// system's only interrupt source today: it wakes the CPU from WFI (so the idle loop isn't a busy
// spin) and drives the monotonic `ticks()` counter that arch-neutral code reads for coarse timing.
//
// We use the EL1 physical timer registers (CNTP_*_EL0). They are accessible — and the timer they
// control fires — at both EL1 and EL2, so this one register set works whichever EL UEFI hands us
// off at (EL2 on QEMU `virt`); the timer's PPI is INTID 30 on both QEMU virt and the Pi 4. The
// virtual (CNTV) and hypervisor (CNTHP) timers are deliberately left alone.
//
// Periodic tick via TVAL reload: CNTP_TVAL_EL0 is a 32-bit *down-counter*; the timer condition
// fires when it reaches 0. Re-loading it with `interval` inside the handler restarts the countdown,
// giving a periodic-ish tick. (It drifts by the handler latency vs an absolute CVAL schedule, which
// is fine for a heartbeat / wake source — exactness isn't needed, the same simplification the x86
// APIC-timer path makes.)

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Generic-timer PPI. INTID 30 = the non-secure EL1 physical timer (PPI 14, +16 base) on both the
/// QEMU `virt` irqmap and the Pi 4 `arm,armv8-timer` binding.
pub const TIMER_INTID: u32 = 30;

/// Set once `verify_live` has confirmed the timer IRQ is actually being *delivered* (ticks advance
/// after unmasking), not merely that the timer counts. The idle path (`arch::hlt`) consults this:
/// true => WFI (park until the next tick); false => poll-spin (no wake source, so WFI would hang).
static LIVE: AtomicBool = AtomicBool::new(false);

/// Target tick rate. 250 Hz gives a 4 ms heartbeat — frequent enough to be a responsive wake
/// source, coarse enough that the per-tick handler cost is negligible.
///
/// PUBLIC because `SYS_GETINFO` hands this tick count to ring 3 raw, so the rate is ABI: the
/// `const _: () = assert!(una_abi::GETINFO_TICK_HZ == super::timer::TICK_HZ)` beside `sys_getinfo`
/// in `syscall.rs` is what stops a retune here from silently re-introducing the 4x unit bug in
/// `user-vug`/`user-pulse` (una-abi's divergence ledger, D1). Retuning it is fine — retuning it
/// WITHOUT moving `GETINFO_TICK_HZ` is now a compile error, which is the whole point.
pub const TICK_HZ: u64 = 250;

/// Monotonic timer ticks since the heartbeat started. Read via `arch::ticks()`.
static TICKS: AtomicU64 = AtomicU64::new(0);
/// Down-counter reload value (CNTFRQ / TICK_HZ), computed once in `init`.
static INTERVAL: AtomicU64 = AtomicU64::new(0);

/// The counter frequency (CNTFRQ_EL0), in Hz. On QEMU `virt` this is 62.5 MHz; on the Pi 4 the
/// firmware programs it (19.2 MHz crystal-derived). Reading it instead of assuming keeps the tick
/// rate correct on both.
#[inline]
pub fn cntfrq() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

#[inline]
fn write_tval(interval: u64) {
    unsafe { core::arch::asm!("msr CNTP_TVAL_EL0, {}", in(reg) interval, options(nomem, nostack, preserves_flags)) };
}

/// Arm the generic timer as a periodic heartbeat and enable its PPI at the GIC. Must run after
/// `gic::init` and before IRQs are unmasked.
pub fn init() {
    let freq = cntfrq();
    // Guard against a firmware that left CNTFRQ unset (0): fall back to the QEMU virt value so the
    // tick is merely wrong-rate rather than a divide-by-zero / never-firing timer.
    let freq = if freq == 0 { 62_500_000 } else { freq };
    let interval = freq / TICK_HZ;
    INTERVAL.store(interval, Ordering::Relaxed);

    arm_this_core();
    serial_println!(
        ":: AARCH64 generic timer armed (CNTFRQ={} Hz, {} Hz tick, INTID {}) ::",
        freq, TICK_HZ, TIMER_INTID
    );
    // UVUG-7 (P52): `arch::ms()` is derived directly from CNTVCT/CNTFRQ, NOT `ticks()*4`. The global
    // tick counter is summed across every core's timer IRQ, so on 4-core BCM2711 metal `ticks()*4`
    // ran ms ~4x fast and typematic repeated ~4x too fast. CNTFRQ is the true, frequency-independent
    // tick rate the clock now uses: 1 CNTVCT tick = 1/CNTFRQ s, so ms = CNTVCT/(CNTFRQ/1000).
    #[cfg(feature = "witness")]
    serial_println!(
        // VUGSLOMO adds the ABI leg as FIELDS on this line and not as a line of its own — the
        // line-neutral rule (d18d37c7) — because it is the same clock answering the same question one
        // consumer further out. `abi_per_tick` is the divisor [`abi_ticks`] applies, and it is the
        // FALSIFIER for this arc on either host: a boot whose `[el0live]`/`[vugfps]` pair still reads
        // 4x apart with this divisor printed is a fault somewhere other than the unit.
        "[uvug7] ms clock: CNTFRQ={} Hz (={} kHz per ms); ms=CNTVCT/(CNTFRQ/1000), core-count-independent; \
         sys_getinfo ticks=CNTVCT/(CNTFRQ/{})={} (NOT the per-core-summed tick counter)",
        freq, freq / 1000, TICK_HZ, freq / TICK_HZ
    );
}

/// Arm THIS core's generic timer + enable its (banked) PPI at the GIC. The BSP reaches this via
/// `init` (which first computes the shared `INTERVAL`); each secondary calls it directly once its
/// GIC CPU interface is up, so every core gets its own periodic tick for scheduler preemption.
/// `INTERVAL` must already be set (the BSP's `init` ran first).
pub fn arm_this_core() {
    super::gic::enable_ppi(TIMER_INTID);
    write_tval(INTERVAL.load(Ordering::Relaxed));
    unsafe {
        // CNTP_CTL_EL0: ENABLE=bit0=1, IMASK=bit1=0 (unmasked). The timer now counts down and will
        // assert its PPI when TVAL hits 0.
        core::arch::asm!("msr CNTP_CTL_EL0, {}", in(reg) 1u64, options(nomem, nostack, preserves_flags));
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
    }
}

/// Per-tick handler, called from the GIC IRQ dispatch when INTID 30 fires. Re-arm first (which
/// clears the timer's level-sensitive output so it doesn't immediately re-fire after EOI), then
/// bump the tick counter. Lock-free; safe from interrupt context.
///
/// The `isb` after the TVAL write is load-bearing on real hardware: the PPI is level-sensitive, so
/// `gic::handle_irq` must not write `GICC_EOIR` until this re-arm has actually deasserted the timer
/// line. Without the context-synchronization barrier the comparator write may not have taken effect
/// by the time EOI is processed, and the GIC re-pends a spurious tick. QEMU re-evaluates the timer
/// synchronously so it never shows there — this is purely for the Pi 4 / baseline-Armv8.0 path.
pub fn on_tick() {
    write_tval(INTERVAL.load(Ordering::Relaxed));
    unsafe { core::arch::asm!("isb", options(nomem, nostack, preserves_flags)) };
    let prev = TICKS.fetch_add(1, Ordering::Relaxed);
    // Per-CPU tick, bumped by THIS core's timer only (each core arms its own periodic tick). It is
    // the scheduler's local clock: `sched::sleep_ticks` computes a wake deadline against this core's
    // count and the scheduler drains due sleepers against it, so a sleeper wakes on the core it
    // parked on regardless of the other cores' tick pace. Advances only on metal (QEMU raspi4b never
    // delivers the timer IRQ, so `on_tick` never runs there — hence tick-driven sleep is metal-only).
    super::percpu::this_cpu().ticks.fetch_add(1, Ordering::Relaxed);
    if prev == 0 {
        serial_println!("AARCH64: timer heartbeat live (first tick).");
    }
}

/// Monotonic count of timer ticks since boot.
#[inline]
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// VUGSLOMO — the [`TICK_HZ`]-rate tick count `sys_getinfo` hands EL0, derived from CNTVCT/CNTFRQ
/// and NOT from [`ticks`]. Core-count-independent, and live even where the tick IRQ is not.
///
/// THE BUG THIS EXISTS TO CLOSE, and it is UVUG-7 (P52) again, one caller further out. [`ticks`] is
/// the GLOBAL counter and [`on_tick`] bumps it from EVERY core's periodic timer IRQ, so on 4-core
/// BCM2711 metal it advances at `4 * TICK_HZ` = ~1000 Hz. `sys_getinfo` handed that counter to EL0
/// RAW while `una_abi::GETINFO_TICK_HZ` told ring 3 to divide by 250 — so every ring-3 program that
/// reads the field measured time 4x FAST and any rate it computed came out 4x LOW.
///
/// The ABIFREEZE assert beside `sys_getinfo` could not catch this and still cannot: it checks
/// `GETINFO_TICK_HZ == TICK_HZ`, which is the rate each core ARMS its timer at, and both sides of
/// that equation were always 250. The divergence was never in the constant — it was that the
/// quantity being published was a SUM over cores rather than a clock. So the fix is on the
/// publishing side, and the assert keeps its meaning: this function's unit IS `TICK_HZ`.
///
/// MEASURED (R24 boot6, PA43 metal, hw-pi4@6de03c87): `[vugfps] wf=` alternated 1,2,1,2 for 818
/// consecutive samples while `[wcn] win=6 asid=0x1` reported the SAME window presenting at 5.8-6.0/s
/// with `gap=144..231ms` — the exact 4x, and the alternation is a ~6 fps rate sampled over the
/// 0.25 s window the wrong divisor opens. PA42 (boot5) reads the same way: `wf=25..41` against
/// `[wcn] rate=96..164/s`. The vug's own meter doc names this disagreement as the finding it was
/// built to make; this is that finding's other end.
///
/// IT IS NOT ONLY A READOUT. `user-vug`'s LOD ladder consumes the meter's number: `rate < LOD_DOWN`
/// (24) drops a rung and `rate * 4 < LOD_DOWN` drops TWO, `rate > LOD_UP` (55) climbs one. Against a
/// 4x-low rate those thresholds sit at 96 and 220 REAL fps, so on the Pi the ladder ran with its
/// step-up unreachable and its double-step-down permanently armed (boot6 `[vuglod] lvl=1001` then
/// `lvl=1`: level 3 -> 1 -> 0 inside two windows), and PA42's 25..41 straddled the 24 edge — which is
/// the fps SWING that boot's operator reported. One divisor, both complaints.
///
/// CNTVCT is immune to both faults CNTFRQ-derived clocks were reached for before: it counts at
/// CNTFRQ_EL0 no matter how many cores tick, and it counts on QEMU raspi4b where the tick IRQ is
/// never delivered at all (there [`ticks`] stays 0 forever, so the meter's `now > *ticks` gate never
/// opened and the readout never refreshed — the same silence the old `ms()` had).
///
/// `cntfrq() / TICK_HZ` is exact on both paths (54 MHz / 250 = 216 000; QEMU virt 62.5 MHz / 250 =
/// 250 000). A zero CNTFRQ — no generic timer — yields 0 rather than dividing by it; ring 3 reads a
/// frozen clock and skips its update, which is the same shape as the pre-heartbeat case.
#[inline]
pub fn abi_ticks() -> u64 {
    let per_tick = cntfrq() / TICK_HZ;
    if per_tick == 0 { 0 } else { super::now_cycles() / per_tick }
}

/// Whether the timer IRQ was confirmed delivering (see `verify_live`). The idle path branches on it.
#[inline]
pub fn is_live() -> bool {
    LIVE.load(Ordering::Relaxed)
}

/// Clear the liveness flag. Used by the Jetson (tegra) EL2->EL1 drop, which disables the physical
/// timer (CNTP_CTL=0) but leaves the core with no interrupt source at EL1 — so from there
/// `arch::hlt()` must fall back to a busy spin instead of a wake-less WFI-park (`verify_live` set
/// LIVE true at EL2, and that reading is stale once the timer is off). The free-running counter used
/// for wall-clock timeouts (CNTPCT/CNTVCT) keeps counting regardless, so polled paths still bound
/// themselves correctly. `Relaxed` matches the other `LIVE` accesses.
pub fn set_not_live() {
    LIVE.store(false, Ordering::Relaxed);
}

/// Post-unmask liveness gate: watch the tick counter actually advance over a bounded wall-clock
/// window (timed off the always-running CNTPCT, which doesn't depend on the IRQ). Call ONCE right
/// after IRQs are unmasked.
///
/// This guards a serial-less metal failure mode: if the timer counts and the GIC latches its PPI
/// but the CPU interface never *delivers* the IRQ — e.g. the Pi's security-extensions GIC routes the
/// PPI differently than the Non-secure Group-1 model assumes — then `ticks()` stays frozen, and a
/// WFI idle would sleep forever with no wake source, leaving the GUI painted-but-frozen (input never
/// re-polled). Confirming delivery here lets `arch::hlt` fall back to a poll-spin so the system stays
/// responsive (the pre-interrupt behavior) instead of hanging. On QEMU / a correctly-wired Pi the
/// first tick lands within one period (~4 ms) and this returns immediately.
pub fn verify_live() {
    let freq = cntfrq();
    let budget = (if freq == 0 { 62_500_000 } else { freq }) / 10; // ~100 ms ceiling
    let start_ticks = ticks();
    let start_ct = cntpct();
    let mut live = false;
    while cntpct().wrapping_sub(start_ct) < budget {
        if ticks() != start_ticks {
            live = true;
            break;
        }
        core::hint::spin_loop();
    }
    LIVE.store(live, Ordering::Relaxed);
    if live {
        serial_println!(":: AARCH64 timer LIVE: IRQ delivery confirmed; idle = WFI ::");
    } else {
        serial_println!(":: AARCH64 timer NOT live: no IRQ in ~100 ms; idle = poll-spin fallback ::");
    }
}

/// Raw CNTP_CTL_EL0 (ENABLE=0, IMASK=1, ISTATUS=2). Diagnostic only.
#[inline]
pub fn cntp_ctl() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, CNTP_CTL_EL0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// Raw CNTP_CVAL_EL0 — the comparator the down-counter (TVAL) writes resolve to. Diagnostic only
/// (the `[wedgeprobe]` witness): `cval <= cntpct()` with ENABLE=1/IMASK=0 means the timer's output
/// line SHOULD be asserted right now. Per-core banked, like every CNTP_* register.
#[inline]
pub fn cntp_cval() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, CNTP_CVAL_EL0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// Free-running physical counter CNTPCT_EL0. Diagnostic / busy-delay only.
#[inline]
pub fn cntpct() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// One-shot probe (IRQ still masked): spin until the timer's compare condition fires (ISTATUS=1) or
/// a bounded counter window elapses, then report whether the timer asserted and whether the GIC
/// distributor latched its PPI as pending. This isolates the three failure modes — timer not
/// counting/asserting, GIC not forwarding the PPI, or CPU-interface delivery — without needing IRQs.
pub fn diagnose() {
    let freq = cntfrq();
    let start = cntpct();
    // Wait up to ~20 ms (well over one 4 ms period) for ISTATUS.
    let budget = freq / 50;
    let mut istatus = false;
    while cntpct().wrapping_sub(start) < budget {
        if cntp_ctl() & (1 << 2) != 0 {
            istatus = true;
            break;
        }
        core::hint::spin_loop();
    }
    serial_println!(
        ":: AARCH64 timer diag: CNTP_CTL={:#x} (ISTATUS={})  GICD PPI{} pending={} ::",
        cntp_ctl(),
        istatus as u8,
        TIMER_INTID,
        super::gic::ppi_pending(TIMER_INTID),
    );
}
