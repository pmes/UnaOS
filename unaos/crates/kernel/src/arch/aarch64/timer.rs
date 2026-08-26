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

/// The counter frequency (CNTFRQ_EL0), in Hz — read at runtime, never assumed: QEMU `virt` reads
/// 62.5 MHz, the Pi 4's firmware programs 54 MHz (its crystal; 19.2 MHz is the Pi 3's), and the Orin
/// (Tegra234) reads 31.25 MHz (capture-proven; see `mod.rs` `HW_WAIT_BUDGET`). Zero => `init` substitutes.
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
    } #[cfg(all(feature = "tegra", feature = "bsptick"))] bsptick_witness(); // ORIN-BSPTICK (appended to this line per the Location-shift convention; see file tail)
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

// ── IRQEL-RT EL1 one-shot proof (tail-defined per the Location-shift convention; everything below
// is `tegra`-gated so the pi and QEMU-virt images stay byte-identical). M1 item 4: 0a60e260 made
// the tegra `__vec_irq` bank ELR/SPSR by RUNTIME CurrentEL so one table serves EL2 and EL1 in one
// boot — but only the EL2 arm is metal-proven (JM4's `verify_live`, every boot). The EL1 arm had
// NEVER executed: the JM6 drop disables CNTP and the post-drop core runs cooperatively, so no
// interrupt was ever TAKEN at EL1. This pair arms exactly ONE CNTP tick inside a bounded,
// self-disarming IRQ-unmask window right after the drop (the `self_sgi_smoke` shape), witnesses
// the first IRQ taken at EL1 from inside the handler — i.e. on the far side of the runtime
// `irq_bank!`/`irq_unbank!` machinery this exists to prove — and leaves the machine in byte-exactly
// the pre-existing post-drop state: timer off, IRQ masked, `LIVE` false. The cooperative scheduler
// gains NO periodic tick (the intercept consumes the delivery INSTEAD of `on_tick`'s re-arm). ─────

/// IRQEL-CORE sentinel for [`EL1_PROOF_CORE`]: no core owns the one-shot window.
#[cfg(feature = "tegra")]
const EL1_PROOF_NO_CORE: u32 = u32::MAX;

/// The one-shot proof window is open ON EXACTLY ONE CORE: this is that core's `cpu_index`
/// ([`EL1_PROOF_NO_CORE`] = disarmed). While it is armed, `el1_proof_intercept` consumes the next
/// timer PPI **on that core only** instead of letting `on_tick` re-arm a periodic tick. Set/cleared
/// only by `el1_oneshot_proof` + the intercept.
///
/// IRQEL-CORE — why this is a core id and not a bool. It was `static EL1_PROOF_ARMED: AtomicBool`,
/// i.e. MACHINE-GLOBAL, and that is the defect boot 5c convicted. ORIN-SMP-3 (`UNAOS_TEGRASMP=1`,
/// `main.rs` `start_secondaries_tegra`, called BEFORE the JM6 drop) brings five Orin secondaries
/// online that (a) stay at EL2 — `smp_virt::secondary_entry` calls `exceptions::install()` there,
/// which sets HCR_EL2.IMO|FMO|AMO so their physical IRQs target EL2 — and (b) each arm their OWN
/// periodic 250 Hz PPI 30 via `arm_this_core_ap`. That is ~1250 timer IRQs/s entering this very
/// dispatch from cores that never dropped. Against a global flag the first AP tick after the arm
/// consumed the window: each AP's next tick is uniform in [0, 4 ms) while the boot core's one-shot
/// is a full 4 ms out, so an AP won with probability ~1 — deterministically, not flakily. The
/// intercept then read `CurrentEL` **on that AP** and honestly reported EL2. The instrument was
/// right; the window's scope was wrong. Scoping it to the arming core makes every other core's tick
/// fall through to `on_tick` unchanged — which also stops the old code from writing `CNTP_CTL=0` on
/// a SECONDARY, permanently killing that core's only wake source while `AP_LOCAL_TICK` still told
/// `hlt` it had one (a wake-less WFI park).
#[cfg(feature = "tegra")]
static EL1_PROOF_CORE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(EL1_PROOF_NO_CORE);
/// Latched by the intercept once the proof IRQ was taken; the arming spin watches it.
#[cfg(feature = "tegra")]
static EL1_PROOF_TAKEN: AtomicBool = AtomicBool::new(false);

/// Called from `gic::handle_irq_v3` for `TIMER_INTID`, INSIDE the IRQ handler. Returns `true` iff
/// the armed one-shot was consumed here: the timer is DISARMED first (CNTP_CTL=0 + isb, deasserting
/// the level-sensitive PPI before the caller's EOI — the same ordering rule `on_tick` documents)
/// and the witness printed. `false` (window not armed, or armed for a DIFFERENT core — every
/// periodic tick on every other path) leaves the pre-existing `on_tick` flow untouched.
#[cfg(feature = "tegra")]
pub fn el1_proof_intercept() -> bool {
    // IRQEL-CORE (see `EL1_PROOF_CORE`): the window belongs to exactly ONE core — the one that armed
    // it. The five ORIN-SMP-3 secondaries reach this same dispatch ~1250x/s from EL2 with their own
    // periodic PPI 30, and against the old machine-global flag one of them ALWAYS won the race and
    // printed the EL2 verdict. This guard decides only WHOSE delivery is the proof; the EL test below
    // is untouched, so the proof can still FAIL honestly — if the ARMING core's own IRQ lands at EL2
    // it is reported exactly as before (`this_cpu()` resolves to the same block at either EL: the
    // boot core's TPIDR_EL2 was seeded by the pre-drop `percpu::init(0)` and its TPIDR_EL1 by the
    // post-drop one, both with `cpu_index` 0).
    let armed = EL1_PROOF_CORE.load(Ordering::Acquire);
    if armed == EL1_PROOF_NO_CORE {
        return false;
    }
    let cpu = super::percpu::this_cpu().cpu_index;
    if cpu != armed {
        // Another core's periodic tick. Fall through to `on_tick` exactly as before this instrument
        // existed — and, load-bearing, do NOT write CNTP_CTL=0 here: the old code disarmed the
        // SECONDARY's timer, permanently removing that core's only wake source while
        // `this_core_has_local_tick()` kept telling `hlt` to WFI-park on it.
        return false;
    }
    unsafe {
        core::arch::asm!("msr CNTP_CTL_EL0, xzr", options(nomem, nostack, preserves_flags));
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
    }
    EL1_PROOF_CORE.store(EL1_PROOF_NO_CORE, Ordering::Release);
    // Verify-don't-assume: name the EL this IRQ was actually TAKEN at (CurrentEL — the same runtime
    // test `irq_bank!` ran moments ago on the way in), not the EL we hope it was. The core id is on
    // the line too: it is what makes an "EL2" verdict self-diagnosing (boot 5c's line could not
    // distinguish "the boot core's IRQ was routed to EL2" from "another core answered"). Printing
    // from IRQ context is safe here: the interrupted context is the lock-free arming spin below,
    // which holds no serial (or any other) lock.
    let el = super::exceptions::current_el();
    if el == 1 {
        serial_println!(
            ":: IRQEL-RT: first IRQ taken at EL1 on cpu {} — banked vector path live (ELR_EL1 bank) ::",
            cpu
        );
    } else {
        serial_println!(
            ":: IRQEL-RT: one-shot proof IRQ taken at EL{} on cpu {} (the ARMING core) — NOT the EL1 proof; HCR_EL2.IMO routed it up, see the [irqel2a] EL2 latch above (investigate) ::",
            el, cpu
        );
    }
    EL1_PROOF_TAKEN.store(true, Ordering::Release);
    true
}

/// M1 item 4 — called ONCE from `main.rs`, immediately after the post-drop `exceptions::install()`
/// (EL1, DAIF fully masked, timer off per `set_not_live`): arm CNTP for a SINGLE tick (~4 ms) and
/// open a bounded ~100 ms IRQ-unmask window (save DAIF, unmask I, spin on the flag off the
/// free-running CNTPCT, restore DAIF — the `self_sgi_smoke` shape) so exactly one interrupt is
/// taken AT EL1 through the runtime-banked `__vec_irq`. Self-disarming on EVERY path: delivered =>
/// the intercept already wrote CNTP_CTL=0 (and printed the proof witness); not delivered => the
/// window expiry disarms and prints the INCONCLUSIVE line. Never stalls boot beyond the window,
/// and the post-window machine state is exactly today's (timer off, IRQ masked, `LIVE` false).
#[cfg(feature = "tegra")]
pub fn el1_oneshot_proof() {
    let freq = cntfrq();
    let freq = if freq == 0 { 62_500_000 } else { freq };
    let cpu = super::percpu::this_cpu().cpu_index;
    // Armed witness BEFORE the unmask, so the serial lock is free again when the handler prints.
    serial_println!(
        ":: IRQEL-RT: EL1 one-shot proof — arming CNTP for a single tick at EL1 on cpu {} (~100 ms window; CORE-LOCAL, other cores' periodic PPI{} is NOT this proof) ::",
        cpu, TIMER_INTID
    );
    // Adjudicating pair (IRQEL-RT2): the machine state IMMEDIATELY BEFORE the arm — including the
    // GICR PPI enable, which answers the "the EL2-era enable persists across the drop" assumption
    // below that has never actually been read back — and again immediately after it.
    el1_proof_snapshot("pre-arm");
    // Re-assert the (banked) timer PPI enable at this core's redistributor. The EL2-era enable
    // persists across the JM6 drop (GICR state is EL-independent), but the re-enable is idempotent
    // and cheap — verify-don't-assume.
    super::gic::enable_ppi(TIMER_INTID);
    EL1_PROOF_TAKEN.store(false, Ordering::Relaxed);
    EL1_PROOF_CORE.store(cpu, Ordering::Release);
    write_tval(freq / TICK_HZ);
    unsafe {
        core::arch::asm!("msr CNTP_CTL_EL0, {}", in(reg) 1u64, options(nomem, nostack, preserves_flags));
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
    }
    // Still masked here, so this print cannot deadlock against the handler's own.
    el1_proof_snapshot("armed");
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, DAIF", out(reg) daif, options(nomem, nostack, preserves_flags));
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags));
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
    }
    let budget = freq / 10; // ~100 ms — the verify_live/self_sgi_smoke window
    let start = cntpct();
    while cntpct().wrapping_sub(start) < budget {
        if EL1_PROOF_TAKEN.load(Ordering::Acquire) {
            break;
        }
        core::hint::spin_loop();
    }
    unsafe {
        // Restore the entry DAIF (post-drop: fully masked) FIRST, then disarm unconditionally (a
        // no-op when the intercept already did): the one-shot must not outlive its window whatever
        // happened inside it, and no further IRQ can land once I is re-masked.
        core::arch::asm!("msr daif, {}", in(reg) daif, options(nomem, nostack, preserves_flags));
        core::arch::asm!("msr CNTP_CTL_EL0, xzr", options(nomem, nostack, preserves_flags));
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
    }
    EL1_PROOF_CORE.store(EL1_PROOF_NO_CORE, Ordering::Release);
    if !EL1_PROOF_TAKEN.load(Ordering::Acquire) {
        // Bounded, non-blocking miss path (the instrument-exists law): note it with the same
        // diagnostics `diagnose` reads and PROCEED — boot must not stall on an unproven vector arm.
        serial_println!(
            ":: IRQEL-RT: EL1 one-shot NOT delivered in ~100 ms — proof INCONCLUSIVE (CNTP_CTL={:#x}, GICR PPI{} pending={}); timer disarmed, boot proceeds ::",
            cntp_ctl(),
            TIMER_INTID,
            super::gic::ppi_pending(TIMER_INTID)
        );
        // The miss path gets the same adjudicating snapshot as the arm, so an INCONCLUSIVE capture
        // still names WHICH precondition moved (PPI enable dropped, PMR masked, RPR stuck on an
        // un-EOI'd interrupt, comparator never reached) instead of only that nothing arrived.
        el1_proof_snapshot("miss");
    }
}

/// IRQEL-RT2 — one adjudicating state dump for the EL1 one-shot proof, taken on the arming core at
/// `when` ∈ {`pre-arm`, `armed`, `miss`}. Every value here is EL1-legal, and the two lines are
/// SPLIT deliberately:
///
///  * `[irqel2a]` carries the CPU-side state, including the four **EL2-only** registers that decide
///    where a physical IRQ goes. HCR_EL2 / CNTHCTL_EL2 / ICC_SRE_EL2 cannot be read at EL1 (an
///    `mrs` of an EL2 register from EL1 is UNDEFINED — it would take a sync exception into the very
///    vectors under test), so they are latched by the JM6 drop asm at the last instant they are
///    readable and read back here from RAM: `boot_tegra::jm6_el2_latch`. HCR_EL2.IMO is THE bit
///    that answers "EL1 or EL2" — 0 means a physical IRQ taken at EL1 targets EL1.
///  * `[irqel2b]` carries the GIC view, and is second on purpose: it is the only part that performs
///    an `ICC_*_EL1` system-register access at EL1, which is UNDEFINED unless ICC_SRE_EL1.SRE == 1.
///    That bit is printed by `[irqel2a]`, which is therefore already on the wire if `[irqel2b]`
///    faults — and when the latch says SRE == 0 the snapshot is skipped and SAID so rather than
///    taken. (`gic::wedgeprobe_snapshot` is reused rather than duplicated: it is read-only, takes no
///    lock and enters no wait, and its banked-state fields are exactly this proof's preconditions.)
///
/// The CNTP_* reads need CNTHCTL_EL2.EL1PCTEN/EL1PCEN, which the drop sets and boot 5c already
/// proved effective — that boot's `write_tval` (`msr CNTP_TVAL_EL0`) and `cntpct()` both executed at
/// EL1 with no trap. Cost is four serial lines on one tegra boot, all after `tegra_early_stop` has
/// armed the UARTC latch (DARKWIN), and the whole block is `tegra`-gated.
#[cfg(feature = "tegra")]
#[inline(never)]
fn el1_proof_snapshot(when: &str) {
    let daif: u64;
    unsafe { core::arch::asm!("mrs {}, DAIF", out(reg) daif, options(nomem, nostack, preserves_flags)) };
    let (hcr, cnthctl, sre1, sre2) = super::boot_tegra::jm6_el2_latch();
    let ctl = cntp_ctl();
    let (cval, now) = (cntp_cval(), cntpct());
    serial_println!(
        ":: [irqel2a] {} cpu={} CurrentEL={} DAIF={:#x} (I={}) | JM6 EL2 latch: HCR_EL2={:#x} \
         IMO={} FMO={} AMO={} TGE={} E2H={} RW={} | CNTHCTL_EL2={:#x} EL1PCTEN={} EL1PCEN={} | \
         ICC_SRE_EL1={:#x} ICC_SRE_EL2={:#x} | CNTP_CTL={:#x} ENABLE={} IMASK={} ISTATUS={} \
         CVAL-CNTPCT={}cyc CNTFRQ={} :: (IMO=0 is what makes an IRQ taken at EL1 target EL1)",
        when,
        super::percpu::this_cpu().cpu_index,
        super::exceptions::current_el(),
        daif,
        (daif >> 7) & 1,
        hcr,
        (hcr >> 4) & 1,
        (hcr >> 3) & 1,
        (hcr >> 5) & 1,
        (hcr >> 27) & 1,
        (hcr >> 34) & 1,
        (hcr >> 31) & 1,
        cnthctl,
        cnthctl & 1,
        (cnthctl >> 1) & 1,
        sre1,
        sre2,
        ctl,
        ctl & 1,
        (ctl >> 1) & 1,
        (ctl >> 2) & 1,
        cval.wrapping_sub(now) as i64,
        cntfrq(),
    );
    if sre1 & 1 == 0 {
        serial_println!(
            ":: [irqel2b] {} SKIPPED — the JM6 latch says ICC_SRE_EL1.SRE=0, so any ICC_*_EL1 access at EL1 is UNDEFINED; the GIC view is NOT read (and the handler's own ICC_IAR1_EL1 would fault) ::",
            when
        );
        return;
    }
    let g = super::gic::wedgeprobe_snapshot();
    serial_println!(
        ":: [irqel2b] {} GICD_CTLR={:#x} | this core's banked PPI{}: enab={} pend={} act={} | \
         ICC_IGRPEN1_EL1={:#x} ICC_PMR_EL1={:#x} ICC_RPR_EL1={:#x} ICC_HPPIR1_EL1={} ::",
        when,
        g.gicd_ctlr,
        TIMER_INTID,
        g.timer_enabled as u8,
        g.timer_pending as u8,
        g.timer_active as u8,
        g.cpuif_ctlr,
        g.pmr,
        g.rpr,
        g.hppir,
    );
}

// ── ORIN-BSPTICK (Candidate B arc 1; tail-defined per the Location-shift convention; everything
// below is `tegra`+`bsptick`-gated so the knob-off image is byte-identical to baseline). A PERIODIC
// EL1 generic-timer tick on the Orin boot core, armed from `tegra_early_stop`'s terminus line right
// after the IRQEL-RT one-shot proof self-disarms — the first standing periodic interrupt the
// post-drop boot core has ever had.
//
// THE JM6 DISARM, RE-DERIVED RATHER THAN REUSED. The drop asm (`boot_tegra.rs`) writes
// `msr cntp_ctl_el0, xzr` for two stated reasons: (1) at the time, the shared `__vec_irq` stub read
// ELR_EL2/SPSR_EL2 unconditionally and an IRQ taken at EL1 would have FAULTED in the stub; (2) the
// post-drop CAPSTONE loop is cooperative and needs no tick. Reason (1) is dead: 0a60e260 made the
// tegra `__vec_irq` bank ELR/SPSR by RUNTIME CurrentEL, and the IRQEL-RT one-shot exists precisely
// to prove that bank live at EL1 — this periodic tick rides the same runtime-banked vector, armed
// only AFTER the post-drop `exceptions::install()` has VBAR_EL1 pointing at it. Reason (2) was a
// preference, not a constraint, and it SURVIVES this arc: on the tegra (not-`baremetal`) build the
// v3 dispatch's timer arm calls ONLY `on_tick` (`gic::handle_irq_v3` — `timer_preempt` is
// `baremetal`-gated and `SCHED_ACTIVE` is false on this board), so a tick re-arms TVAL and bumps
// counters but can never context-switch: the terminus stays cooperative, now with a heartbeat.
// The remaining hazards, one by one: SERIAL — `_print` masks IRQs around the port lock and an
// IRQ-context print goes `try_lock` + staging ring, so a tick landing mid-print cannot deadlock;
// EL0 — the 0x480 lower-EL IRQ vector routes to the same banked `__vec_irq` (metal-proven on the
// Pi's preemptive EL0 path); IDLE — `LIVE` stays false, so `arch::hlt`'s timerless-core poll-spin
// decision (JD3: what keeps the pump's synchronous USB reads progressing) is untouched; the tick
// does not license WFI. AP CROSS-TALK is the IRQEL-CORE lesson re-applied below.
//
// WITNESS RULE: the `[orinbsptick]` token is 13 bytes — LLVM immediate-encodes tokens <= 8 bytes
// (invisible to artifact grep), so this one is provable by `strings` on the objcopy'd image. ──────

/// ORIN-BSPTICK sentinel for [`BSPTICK_CORE`]: no core owns the periodic tick.
#[cfg(all(feature = "tegra", feature = "bsptick"))]
const BSPTICK_NO_CORE: u32 = u32::MAX;

/// The `cpu_index` of the ONE core whose periodic EL1 tick this instrument witnesses
/// ([`BSPTICK_NO_CORE`] = disarmed). A core id and not a bool for the IRQEL-CORE reason
/// (`EL1_PROOF_CORE` above): the five ORIN-SMP-3 secondaries each arm their OWN 250 Hz PPI 30 at
/// EL2 and reach the SAME `on_tick` dispatch ~1250x/s — against machine-global state an AP consumes
/// the boot core's window with probability ~1 (that exact bug burned the one-shot proof's first
/// shape). Scoped to the arming core, every AP tick falls through this instrument unchanged.
#[cfg(all(feature = "tegra", feature = "bsptick"))]
static BSPTICK_CORE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(BSPTICK_NO_CORE);

/// Ticks taken on the arming core since [`el1_bsptick_start`]. Written ONLY by that core (the
/// [`BSPTICK_CORE`] guard in [`bsptick_witness`] runs before every bump), so it is boot-core-scoped
/// by construction even though the cell itself is a static.
#[cfg(all(feature = "tegra", feature = "bsptick"))]
static BSPTICK_COUNT: AtomicU64 = AtomicU64::new(0);

/// ORIN-BSPTICK — arm THIS core's generic timer as a standing PERIODIC tick at EL1 and leave IRQs
/// unmasked, permanently: every statement of the terminus line after this call, and the
/// `run_capstone_boot_core` drive loop itself, now executes with a 250 Hz heartbeat landing through
/// the runtime-banked `__vec_irq`. Call ONCE from `main.rs`, on the boot core, after the post-drop
/// `exceptions::install()` (VBAR_EL1 must already be live) — in practice right after
/// `el1_oneshot_proof` returns, whose self-disarm leaves exactly the state this function expects
/// (timer off, IRQ masked, `EL1_PROOF_CORE` = none, GICR PPI enable persisting from its arm).
#[cfg(all(feature = "tegra", feature = "bsptick"))]
pub fn el1_bsptick_start() {
    let freq = cntfrq();
    let freq = if freq == 0 { 62_500_000 } else { freq };
    let cpu = super::percpu::this_cpu().cpu_index;
    // Re-seed INTERVAL (idempotent — JM4's `init` computed the same value at EL2; `on_tick` reloads
    // TVAL from it on every tick, and a zero there would be an instant re-fire storm).
    INTERVAL.store(freq / TICK_HZ, Ordering::Relaxed);
    BSPTICK_COUNT.store(0, Ordering::Relaxed);
    BSPTICK_CORE.store(cpu, Ordering::Release);
    // Armed banner BEFORE the unmask, so the serial lock is free when the first tick's witness
    // prints (the same ordering `el1_oneshot_proof` uses for the same reason).
    serial_println!(
        ":: [orinbsptick] arming PERIODIC CNTP at EL{} on cpu {} ({} Hz, PPI{}) — IRQs stay UNMASKED across the terminus; dispatch is on_tick ONLY (timer_preempt is baremetal-gated, SCHED_ACTIVE false): no preemption, the capstone loop stays cooperative ::",
        super::exceptions::current_el(), cpu, TICK_HZ, TIMER_INTID
    );
    // The one-shot's own arm path, reused: banked GICR PPI enable + TVAL + ENABLE=1 + isb. NOT
    // `arm_this_core_ap` — the boot core stays the global-clock owner, so `ticks()`/`ms()` resume
    // advancing post-drop exactly as they did at EL2 (frozen since the JM6 disarm until now).
    arm_this_core();
    unsafe {
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags));
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
    }
}

/// ORIN-BSPTICK per-tick witness, called from `on_tick` (IRQ context — safe: `_print` is
/// `try_lock` + staging, see the block comment above). Counts ONLY the arming core's ticks (the
/// IRQEL-CORE guard — the five EL2 APs reach this same dispatch with their own PPI 30 and must fall
/// through unchanged) and emits at a low rate: tick 1 (the one-hit proof the arm delivered), then
/// every [`TICK_HZ`]th tick (~1 line/s). The line carries count + CurrentEL + cpu: the count
/// advancing proves the tick is PERIODIC (the one-shot could only ever say "once"), and the EL is
/// re-measured per emission rather than assumed — an EL2 reading here would mean HCR_EL2.IMO
/// regressed, reported instead of hidden.
#[cfg(all(feature = "tegra", feature = "bsptick"))]
#[inline(never)]
fn bsptick_witness() {
    let armed = BSPTICK_CORE.load(Ordering::Acquire);
    if armed == BSPTICK_NO_CORE {
        return;
    }
    let cpu = super::percpu::this_cpu().cpu_index;
    if cpu != armed {
        return;
    }
    let n = BSPTICK_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 || n % TICK_HZ == 0 {
        serial_println!(
            ":: [orinbsptick] tick {} taken at EL{} on cpu {} — periodic CNTP live across the terminus ::",
            n,
            super::exceptions::current_el(),
            cpu
        );
    }
}
