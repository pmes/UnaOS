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

/// Monotonic timer ticks since the heartbeat started. Read via `arch::ticks()` — and, through `ms()`
/// (= `ticks() * 4`), the wall-clock budget arch-neutral code times against. So EXACTLY ONE core may
/// advance it: the boot core (the global-clock owner). Every other GICv3 secondary that arms its own
/// periodic tick (JC3, `arm_this_core_ap`) is registered LOCAL-ONLY in `AP_LOCAL_TICK` and bumps only
/// its per-CPU `percpu.ticks`, never this — so N ticking APs do not inflate `ticks()`/`ms()` N×.
static TICKS: AtomicU64 = AtomicU64::new(0);
/// Down-counter reload value (CNTFRQ / TICK_HZ), computed once in `init`.
static INTERVAL: AtomicU64 = AtomicU64::new(0);

/// JC3 — bitmask (by linear `cpu_index`) of cores whose periodic tick is LOCAL-ONLY: they advance
/// their own `percpu.ticks` (this core's scheduler clock, drives `sleep_ticks`/idle-wake) but NEVER the
/// shared monotonic `TICKS`. Set by `arm_this_core_ap` when a GICv3 secondary arms its own tick; the
/// boot core (global-clock owner) is never in it. GICv3/`virt`+tegra only — the Pi is GICv2 with every
/// core bumping the shared clock as before, and this whole path is compiled out there, so the Pi image
/// (and its `on_tick`) is byte-identical.
#[cfg(not(feature = "pi"))]
static AP_LOCAL_TICK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

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
    //
    // ...but SAY SO. The banner below prints the SUBSTITUTED value, so a board whose firmware never
    // programmed CNTFRQ produced a line reading `CNTFRQ=62500000 Hz` — indistinguishable from a
    // correctly-programmed QEMU virt, while every wall-clock budget derived from it (verify_live's
    // ~100 ms, the xHCI pump's timeouts, busy_delay_ms) is silently wrong by whatever ratio the real
    // counter runs at. A substituted default that is never witnessed is a fabricated measurement.
    // `tegra`-gated so the pi/virt images stay byte-identical.
    #[cfg(feature = "tegra")]
    if freq == 0 {
        cntfrq_substituted(62_500_000);
    }
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
        "[uvug7] ms clock: CNTFRQ={} Hz (={} kHz per ms); ms=CNTVCT/(CNTFRQ/1000), core-count-independent",
        freq, freq / 1000
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

/// JC3 — arm a GICv3 SECONDARY's own periodic tick. Identical to `arm_this_core` (enable this core's
/// timer PPI at its redistributor + start the periodic down-counter) but FIRST registers this core as a
/// LOCAL-ONLY ticker in `AP_LOCAL_TICK`, so its `on_tick` advances only its per-CPU `percpu.ticks` and
/// never the shared monotonic `TICKS`. This is the JC3 containment of the deferred double-count (the
/// reason the AP tick was held back in JC2): several APs may now run their own idle-wake/preemptible
/// tick — making each a self-driven scheduler participant instead of reschedule-SGI-dependent — without
/// inflating the `ticks()`/`ms()` wall-clock budget. `INTERVAL` must already be set (the BSP's `init`
/// ran first). GICv3 only; the Pi's per-core arm stays `arm_this_core` (shared clock, unchanged).
#[cfg(not(feature = "pi"))]
pub fn arm_this_core_ap() {
    let cpu = super::percpu::this_cpu().cpu_index;
    if cpu < 32 {
        AP_LOCAL_TICK.fetch_or(1 << cpu, Ordering::Relaxed);
    }
    arm_this_core();
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
    // Per-CPU tick, bumped by THIS core's timer only (each core arms its own periodic tick). It is
    // the scheduler's local clock: `sched::sleep_ticks` computes a wake deadline against this core's
    // count and the scheduler drains due sleepers against it, so a sleeper wakes on the core it
    // parked on regardless of the other cores' tick pace. Advances only on metal (QEMU raspi4b never
    // delivers the timer IRQ, so `on_tick` never runs there — hence tick-driven sleep is metal-only).
    let local = super::percpu::this_cpu().ticks.fetch_add(1, Ordering::Relaxed);
    // Shared monotonic `TICKS` (feeds `ticks()`/`ms()` wall-clock budgets): advanced ONLY by cores that
    // are NOT registered LOCAL-ONLY. On the Pi (GICv2) that is every core, unchanged. On the GICv3
    // `virt`/tegra path (JC3) the boot core owns it and each secondary armed via `arm_this_core_ap` is
    // local-only, so multiple ticking APs do not advance the shared clock N×.
    #[cfg(not(feature = "pi"))]
    let local_only = {
        let cpu = super::percpu::this_cpu().cpu_index;
        cpu < 32 && (AP_LOCAL_TICK.load(Ordering::Relaxed) & (1 << cpu)) != 0
    };
    #[cfg(feature = "pi")]
    let local_only = false;
    if !local_only {
        let prev = TICKS.fetch_add(1, Ordering::Relaxed);
        if prev == 0 {
            serial_println!("AARCH64: timer heartbeat live (first tick).");
        }
    } else if local == 0 {
        // JC3 witness — one line the first time THIS secondary's per-core timer PPI delivers, quiet
        // after (bounded to one line per AP: this branch runs only on that core's first tick). Proof
        // the AP is now self-driven (its own tick re-polls the run queue) rather than SGI-dependent.
        #[cfg(not(feature = "pi"))]
        serial_println!(
            ":: AARCH64 SMP: c{} timer PPI live (tick {}) ::",
            super::percpu::this_cpu().cpu_index,
            local + 1
        );
    }
}

/// Monotonic count of timer ticks since boot.
#[inline]
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Whether the timer IRQ was confirmed delivering (see `verify_live`). The idle path branches on it.
#[inline]
pub fn is_live() -> bool {
    LIVE.load(Ordering::Relaxed)
}

/// HID-REGRESS-B12 — does THIS core have its OWN periodic timer PPI armed (JC3 `arm_this_core_ap`)?
///
/// The global `LIVE` flag reflects the BOOT CORE's timer only: the Jetson JM6 EL2->EL1 drop clears it
/// (`set_not_live`) because the boot core's physical timer is switched off there. But a GICv3 SECONDARY
/// that armed its own local-only tick (registered in `AP_LOCAL_TICK`) still has a live 250 Hz wake
/// source of its OWN, independent of the boot core. Without this distinction such a secondary falls into
/// the `LIVE == false` poll-spin branch of `arch::hlt` post-drop and BUSY-SPINS its `run()` idle loop
/// (re-attempting a work-steal every iteration) instead of parking in WFI until its next tick — and five
/// cores hammering the shared run-queue spinlocks/atomics saturate the interconnect, starving the boot
/// core's cooperative xHCI HID poll (the boot-12 "keyboard+mouse armed but ZERO deliveries" regression).
/// Reporting the local tick here lets `hlt` WFI-park such a core (bounded to one ~4 ms tick), so it stays
/// self-scheduling yet idle-quiet, and input coexists. The boot core is never in `AP_LOCAL_TICK`, so its
/// timerless post-drop poll-spin is unchanged. Pi (GICv2, no `AP_LOCAL_TICK`) always reads false — the
/// Pi idle path is byte-identical.
#[inline]
pub fn this_core_has_local_tick() -> bool {
    #[cfg(not(feature = "pi"))]
    {
        let cpu = super::percpu::this_cpu().cpu_index;
        cpu < 32 && (AP_LOCAL_TICK.load(Ordering::Relaxed) & (1 << cpu)) != 0
    }
    #[cfg(feature = "pi")]
    {
        false
    }
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

// ── CNTFRQ-SUB witness (tail-defined per the Location-shift convention; `tegra`-gated so the pi and
// QEMU-virt images stay byte-identical). See `init`: the fallback is correct, its silence was not.
#[cfg(feature = "tegra")]
#[inline(never)]
fn cntfrq_substituted(used: u64) {
    serial_println!(
        ":: tegra: CNTFRQ-SUB — CNTFRQ_EL0 reads 0 (firmware never programmed it); SUBSTITUTING {} Hz for the tick + EVERY wall-clock budget derived from it — the banner's CNTFRQ below is this substitute, NOT silicon ::",
        used
    );
}
