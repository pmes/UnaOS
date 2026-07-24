// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// aarch64 SMP bring-up — releasing the parked Cortex-A72 secondary cores on the Raspberry Pi 4.
//
// The GPU firmware (default armstub, no `armstub=` line) starts only core 0 at our kernel; cores
// 1-3 sit in the firmware's spin-table, each in `wfe; ldr xN,[release_addr]; cbz xN,loop; br xN`,
// watching a per-core 64-bit slot in the first page of RAM (core1=0xE0, core2=0xE8, core3=0xF0 —
// from the BCM2711 DTB `cpu-release-addr`). To release a core we write our entry point into its
// slot and `SEV`; it wakes, reads a now-non-zero slot, and branches to us.
//
// Two things make this correct on real silicon (both no-ops in QEMU, which QEMU raspi4b models with
// the *same* spin-table so this exact code is testable there):
//   * The parked core reads its slot with the **MMU off / non-cacheable**, while we write it with
//     the MMU on and RAM cacheable — so our store can sit in cache and never be seen. We must clean
//     the slot to the Point of Coherency (`DC CVAC` + `DSB`) before `SEV`.
//   * A released core arrives in the *same* raw state core 0 did: EL2, MMU OFF, caches off, no
//     stack. So each secondary re-runs the per-core setup (stack, `enable_mmu`, exception vectors)
//     before it can touch a lock or atomic. That's what `_secondary_start` + `__secondary_rust` do.

use core::sync::atomic::{AtomicBool, Ordering};

use super::{boot, cache, exceptions, gic, percpu, sched, timer};

/// The Pi 4 is a single cluster of 4 Cortex-A72s (MPIDR Aff0 = 0..3).
const NUM_CORES: usize = 4;

/// SGI 0 — the inter-processor wake/reschedule ping. M2 uses it only to prove cross-core delivery;
/// M3 will hang the scheduler reschedule off it (the handler is `gic::handle_irq`'s SGI branch).
pub const IPI_RESCHED: u32 = 0;

/// Per-core spin-table release slots (physical addresses in the first page), from the BCM2711 DTB
/// `cpu-release-addr`. Index by MPIDR Aff0; slot 0 (0xD8) is core 0's and unused (it's already up).
const RELEASE_ADDR: [usize; NUM_CORES] = [0xD8, 0xE0, 0xE8, 0xF0];

/// Set true by each secondary once its MMU + vectors + per-CPU + GIC + IPI are up and IRQs are
/// unmasked — the BSP waits on this before it sends any SGI (so the ping isn't lost pre-init).
static CORE_READY: [AtomicBool; NUM_CORES] = [const { AtomicBool::new(false) }; NUM_CORES];

/// One secondary's boot/idle stack (16 KiB… 64 KiB; AArch64 SP must stay 16-aligned, which the
/// type alignment + power-of-two size guarantee). Lives in BSS (zeroed by core 0's `_start`).
const SEC_STACK_SIZE: usize = 0x1_0000; // 64 KiB
#[repr(C, align(16))]
struct SecStack([u8; SEC_STACK_SIZE]);
/// One stack per core, indexed by Aff0. Slot 0 is unused (core 0 has the linker `__stack_top`); the
/// secondaries take slots 1-3. `_secondary_start` computes its own top: base + (core+1)*size.
static mut SECONDARY_STACKS: [SecStack; NUM_CORES] =
    [const { SecStack([0; SEC_STACK_SIZE]) }; NUM_CORES];

// The secondary entry stub. Runs with the MMU OFF at EL2 (x0-x3 = 0 per the spin-table protocol).
// It reads its Aff0 into x0 and sets SP to this core's stack top, then tail-calls the Rust entry.
// Position-independent absolute symbol references (adrp/add, and `sym`) resolve to the correct
// address because the kernel is identity-mapped (VA == PA). Kept out of `.text.boot` on purpose —
// it's reached by address, not by being first in the image.
//
// NOTE: the Aff0 passed in x0 is ADVISORY. `__secondary_rust` ignores it and re-derives the core id
// from MPIDR_EL1 after `enable_mmu` — the MMU-off read here can be spilled to the stack and reloaded
// stale-cacheable on >1 MiB BCM2711 images (the CORE3-SMP hazard; see arch_arm64.md §CORE3-SMP FIX).
// The asm below is unchanged; only the id's *consumer* moved past the MMU turn-on.
#[cfg(not(feature = "core3probe"))]
core::arch::global_asm!(
    r#"
    .globl _secondary_start
    _secondary_start:
        mrs   x0, mpidr_el1
        and   x0, x0, #0xff          // x0 = core id (Aff0)
        adrp  x1, {stacks}
        add   x1, x1, #:lo12:{stacks}
        mov   x2, #({size} >> 12)
        lsl   x2, x2, #12            // x2 = SEC_STACK_SIZE
        madd  x3, x0, x2, x1         // x3 = &SECONDARY_STACKS + core*size
        add   x3, x3, x2            // + size  => top of this core's stack
        mov   sp, x3
        bl    {entry}               // __secondary_rust(core)  — never returns
    1:  wfe
        b     1b
    "#,
    stacks = sym SECONDARY_STACKS,
    entry = sym __secondary_rust,
    size = const SEC_STACK_SIZE,
);

// CORE3-PROBE (UNAOS_CORE3PROBE=1). Identical to the stub above, but with a raw-PL011 dump prologue
// that runs FIRST — MMU off, no stack, physical addressing, before the `mrs`/stack setup and any
// Rust. For each core it emits one record `[<mpidr_lo>E<el>X<x0_lo>]` to the Pi 4 PL011 data
// register (0xFE201000), polling the FR TXFF bit (base+0x18, bit 5) with a bounded spin so no core
// can wedge another; interleaving between cores is acceptable — the hex fields disambiguate. This
// answers the CORE3-SMP paradox: a core-3 record reading `[3E2X0]` means it arrived correctly (bug
// is in Rust/x0 spill); `[0E2X0]` means MPIDR_EL1 genuinely reads 0 at EL2 (firmware delivery); a
// missing/garbled record means a wrong branch target. The `.Lc3p_*` helpers are leaf (single-level
// `bl`, `ret` via x30, no stack) and use only x9-x17 scratch, all overwritten by the real stub.
#[cfg(feature = "core3probe")]
core::arch::global_asm!(
    r#"
    .globl _secondary_start
    _secondary_start:
        // --- core3probe raw dump (MMU off, no stack) ---
        mov   x9,  #0x1000
        movk  x9,  #0xFE20, lsl #16   // x9 = PL011 base 0xFE201000
        mrs   x10, mpidr_el1          // x10 = MPIDR (low byte = Aff0)
        mrs   x13, currentel          // x13 bits[3:2] = EL
        mov   x14, x0                 // x14 = arrival x0 (spin-table sets x0..x3 = 0)
        mov   w11, #0x5B              // '['
        bl    .Lc3p_putc
        lsr   x11, x10, #4
        bl    .Lc3p_hex               // MPIDR low byte, high nibble
        mov   x11, x10
        bl    .Lc3p_hex               // MPIDR low byte, low nibble
        mov   w11, #0x45              // 'E'
        bl    .Lc3p_putc
        lsr   x11, x13, #2
        bl    .Lc3p_hex               // CurrentEL (0..3)
        mov   w11, #0x58              // 'X'
        bl    .Lc3p_putc
        mov   x11, x14
        bl    .Lc3p_hex               // arrival x0, low nibble
        mov   w11, #0x5D              // ']'
        bl    .Lc3p_putc
        b     .Lc3p_done
    .Lc3p_hex:
        and   x11, x11, #0xf
        cmp   x11, #9
        add   x12, x11, #0x30         // '0'..'9'
        add   x15, x11, #0x37         // 'A'..'F'
        csel  x11, x15, x12, gt
        // fall through to putc (x30 still = the .Lc3p_hex caller's link)
    .Lc3p_putc:
        mov   x16, #0x100000          // bounded TXFF spin
    2:  ldr   w17, [x9, #0x18]        // PL011 FR
        tbz   w17, #5, 3f             // TXFF (bit 5) clear => room
        subs  x16, x16, #1
        b.ne  2b
    3:  strb  w11, [x9]               // PL011 DR
        ret
    .Lc3p_done:
        // --- original stub (unchanged) ---
        mrs   x0, mpidr_el1
        and   x0, x0, #0xff          // x0 = core id (Aff0)
        adrp  x1, {stacks}
        add   x1, x1, #:lo12:{stacks}
        mov   x2, #({size} >> 12)
        lsl   x2, x2, #12            // x2 = SEC_STACK_SIZE
        madd  x3, x0, x2, x1         // x3 = &SECONDARY_STACKS + core*size
        add   x3, x3, x2            // + size  => top of this core's stack
        mov   sp, x3
        bl    {entry}               // __secondary_rust(core)  — never returns
    1:  wfe
        b     1b
    "#,
    stacks = sym SECONDARY_STACKS,
    entry = sym __secondary_rust,
    size = const SEC_STACK_SIZE,
);

unsafe extern "C" {
    fn _secondary_start();
}

// ---------------------------------------------------------------------------------------------
// SMP-7 telemetry (feature `smp7`, armed by `UNAOS_SMP7=1`; DEFAULT OFF — every call site below is
// `#[cfg]`-gated, so a default image is byte-identical to the pre-arc one).
//
// Metal observation 2026-07-24 (P53): "only procs 2 & 4 running regularly" — 2 of 4 cores online on a
// real Pi 4, worse than the previously documented intermittent 3/4. The pre-arc kernel prints exactly
// one bit of evidence per missing core ("WARNING core N did not come online"), which cannot tell
// "never left the firmware spin-table" from "arrived and died inside its own bring-up". This module
// records, per AP, when it was released, whether/when it checked in, the MPIDR it actually derived,
// how far through bring-up it got, and how many re-releases it took — then prints one summary line.
//
// Everything here is written by a core with its MMU already ON (the check-in point is placed after
// `enable_mmu`, deliberately: an MMU-off store to a cacheable-mapped address is the CORE3-SMP hazard
// class, and telemetry must not re-open it). Pre-MMU evidence remains the `core3probe` raw-PL011 stub
// above — the only sound way to observe that window.
#[cfg(feature = "smp7")]
mod smp7 {
    use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    /// Bring-up stages an AP passes through, published in order. A core that never checks in stays at
    /// `NONE`; a core stuck at `N` died between stage N and N+1 — that is the diagnostic.
    pub const STAGE_NONE: u32 = 0;
    pub const STAGE_RUST: u32 = 1; // MMU on, id re-derived from MPIDR_EL1, bounds-checked
    pub const STAGE_VECTORS: u32 = 2; // VBAR installed
    pub const STAGE_PERCPU: u32 = 3; // TPIDR per-CPU block live
    pub const STAGE_GIC: u32 = 4; // CPU interface + IPI SGI + IRQ unmasked
    pub const STAGE_TIMER: u32 = 5; // this core's periodic tick armed
    pub const STAGE_READY: u32 = 6; // CORE_READY published

    pub fn stage_name(s: u32) -> &'static str {
        match s {
            STAGE_NONE => "none",
            STAGE_RUST => "rust",
            STAGE_VECTORS => "vectors",
            STAGE_PERCPU => "percpu",
            STAGE_GIC => "gic",
            STAGE_TIMER => "timer",
            _ => "ready",
        }
    }

    pub struct CoreEvidence {
        /// CNTPCT when the BSP wrote this core's release slot (0 = never released).
        pub released_at: AtomicU64,
        /// CNTPCT at the AP's first MMU-on instruction in `__secondary_rust` (0 = never arrived).
        pub checkin_at: AtomicU64,
        /// CNTPCT when the AP published `CORE_READY` (0 = never).
        pub ready_at: AtomicU64,
        /// The full MPIDR_EL1 the AP read for itself, MMU on (0 = never checked in).
        pub mpidr: AtomicU64,
        /// Highest stage reached.
        pub stage: AtomicU32,
        /// Re-releases the BSP issued for this core (`smp7_retry` only).
        pub retries: AtomicU32,
    }

    impl CoreEvidence {
        pub const fn new() -> Self {
            Self {
                released_at: AtomicU64::new(0),
                checkin_at: AtomicU64::new(0),
                ready_at: AtomicU64::new(0),
                mpidr: AtomicU64::new(0),
                stage: AtomicU32::new(STAGE_NONE),
                retries: AtomicU32::new(0),
            }
        }
    }

    pub static EV: [CoreEvidence; super::NUM_CORES] =
        [const { CoreEvidence::new() }; super::NUM_CORES];

    /// An AP records that it arrived, with the id it derived for itself. Relaxed is enough: the BSP
    /// only reads these after its own bounded wait, and `CORE_READY`'s Release/Acquire pair is the
    /// real ordering edge for everything that matters.
    pub fn checkin(core: usize, mpidr: u64) {
        EV[core].mpidr.store(mpidr, Ordering::Relaxed);
        EV[core].checkin_at.store(super::timer::cntpct(), Ordering::Relaxed);
        EV[core].stage.store(STAGE_RUST, Ordering::Relaxed);
    }

    pub fn stage(core: usize, s: u32) {
        EV[core].stage.store(s, Ordering::Relaxed);
        if s == STAGE_READY {
            EV[core].ready_at.store(super::timer::cntpct(), Ordering::Relaxed);
        }
    }

    /// Milliseconds from `t0` to `t` at the counter frequency (0 if `t` is unset/earlier). Integer
    /// math only — no FP on the boot path.
    pub fn ms_since(t0: u64, t: u64, freq: u64) -> u64 {
        if t == 0 || t <= t0 || freq == 0 {
            0
        } else {
            (t - t0).saturating_mul(1000) / freq
        }
    }
}

/// Rust entry for a released secondary core. Called from `_secondary_start` with the **MMU still
/// off** and this core's stack set. Turns the MMU on FIRST (before any lock/atomic — `serial_println`
/// takes a spinlock), then brings up this core's private state: exception vectors, per-CPU block
/// (TPIDR_EL2), GIC CPU interface, the IPI SGI, and IRQ unmask. It then idles in WFE, waking to
/// service inter-processor interrupts (M3 will replace the idle with the scheduler).
///
/// The incoming `_advisory` argument (Aff0 that the asm stub read MMU-off) is **advisory only** and
/// deliberately ignored: on real BCM2711 silicon, when the loaded image crosses 1 MiB the compiler's
/// MMU-off stack spill of that argument (`stp x0,[sp,#..]`) is reloaded cacheable after `enable_mmu`
/// and can hit a stale (zero) L2 line — the mismatched-attributes coherency hazard that produced the
/// deterministic "phantom core 0" (id 3 → 0) regression (see §CORE3-SMP FIX, arch_arm64.md; metal
/// probe verdict `[03E2X0]` proved delivery correct + kernel-side corruption). We re-derive the id
/// fresh from `MPIDR_EL1` AFTER the MMU is on, so every store/load of the id is cacheable-coherent.
#[unsafe(no_mangle)]
extern "C" fn __secondary_rust(_advisory: u64) -> ! {
    // Drop this core EL2 -> EL1 (matching the BSP) BEFORE the MMU, so its locks/atomics run at EL1.
    // Each core must do its own drop: VMPIDR/CNTHCTL/CPTR/CPACR/SP_EL1 are all banked per-core.
    unsafe { boot::drop_to_el1() };
    // MMU on, using the L1 table the BSP already built (`enable_mmu` touches no per-core state).
    unsafe { boot::enable_mmu() };

    // Re-derive this core's id from MPIDR_EL1 with the MMU ON — the structural fix for CORE3-SMP.
    // `drop_to_el1` seeds VMPIDR_EL2 with the real MPIDR (boot.rs), so this EL1 read returns the
    // physical Aff0. Both this `mrs` and any spill of the derived value execute cacheable, so there
    // is no MMU-off store / MMU-on reload pair for a stale line to poison (unlike the asm stub's
    // argument, which we ignore). Idiom mirrors gic::mpidr_affinity.
    let mpidr: u64;
    unsafe {
        core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr, options(nomem, nostack, preserves_flags))
    };
    let core = (mpidr & 0xff) as usize;
    // Bounds-check before indexing CORE_READY/percpu/etc. On a garbage affinity, park this core in a
    // low-power WFE loop rather than panic through a possibly-unsound path: a parked core is exactly
    // the pre-fix failure mode (the BSP times it out and runs on the cores it has), never worse.
    if core >= NUM_CORES {
        loop {
            unsafe { core::arch::asm!("wfe", options(nomem, nostack, preserves_flags)) };
        }
    }
    // SMP-7: first evidence this core exists at all, taken with the MMU on (see the module comment).
    // From here every stage bump narrows a lost core to the step it died in.
    #[cfg(feature = "smp7")]
    smp7::checkin(core, mpidr);
    // Per-core VBAR_EL2 so a fault on this core is caught rather than jumping to a stale vector.
    exceptions::install();
    #[cfg(feature = "smp7")]
    smp7::stage(core, smp7::STAGE_VECTORS);
    // Per-CPU data reachable via TPIDR_EL2 (the IPI handler resolves its counter through this).
    percpu::init(core);
    #[cfg(feature = "smp7")]
    smp7::stage(core, smp7::STAGE_PERCPU);
    // This core's GIC CPU interface + the IPI SGI (both banked per-core), then unmask IRQ.
    gic::init_cpu_interface();
    gic::enable_sgi(IPI_RESCHED);
    exceptions::enable_irq();
    #[cfg(feature = "smp7")]
    smp7::stage(core, smp7::STAGE_GIC);
    // This core's own periodic tick, for scheduler preemption (`INTERVAL` was set by the BSP's
    // timer::init). Also wakes this core's WFI in the scheduler's wait/idle loops.
    timer::arm_this_core();
    #[cfg(feature = "smp7")]
    smp7::stage(core, smp7::STAGE_TIMER);

    serial_println!(":: AARCH64 SMP: core {} online (EL1, MMU on, GIC+IPI+timer ready) ::", core);
    // SMP-7: stamp READY *before* the publish, so a core that is counted online always has a ready
    // timestamp (the BSP reads the evidence only after observing CORE_READY, so it cannot see the
    // stage without the stamp — the reverse order could).
    #[cfg(feature = "smp7")]
    smp7::stage(core, smp7::STAGE_READY);
    // Publish readiness AFTER everything above (Release pairs with the BSP's Acquire), so the BSP
    // only sends an SGI once this core can actually take it.
    CORE_READY[core].store(true, Ordering::Release);

    // Enter the scheduler: wait for the BSP to turn scheduling on, then run this core's run queue
    // forever (idling in WFI when empty). Never returns.
    sched::wait_and_run(core)
}

/// SMP-7: print the per-core bring-up evidence, then the one-line verdict. Called by the BSP once the
/// bring-up waits (and any retries) have settled, before the IPI smoke test — so the summary describes
/// *bring-up*, and the IPI lines that follow are read against it.
///
/// Times are milliseconds from the release write, computed from CNTPCT at the measured CNTFRQ (no
/// tick-count assumptions — the AP timers are only armed at stage `timer`).
#[cfg(feature = "smp7")]
fn report(t0: u64, freq: u64) {
    for core in 1..NUM_CORES {
        let e = &smp7::EV[core];
        let checkin = e.checkin_at.load(Ordering::Relaxed);
        let ready = e.ready_at.load(Ordering::Relaxed);
        serial_println!(
            "[smp7] core {} released=+{}ms checkin={} ready={} mpidr={:#x} stage={} retries={}",
            core,
            smp7::ms_since(t0, e.released_at.load(Ordering::Relaxed), freq),
            if checkin == 0 {
                alloc::format!("-")
            } else {
                alloc::format!("+{}ms", smp7::ms_since(t0, checkin, freq))
            },
            if ready == 0 {
                alloc::format!("-")
            } else {
                alloc::format!("+{}ms", smp7::ms_since(t0, ready, freq))
            },
            e.mpidr.load(Ordering::Relaxed),
            smp7::stage_name(e.stage.load(Ordering::Relaxed)),
            e.retries.load(Ordering::Relaxed),
        );
    }

    // Masks are by MPIDR Aff0: bit 0 is the BSP, which is online by construction.
    let mut online: u32 = 1;
    let mut missed: u32 = 0;
    let mut detail = alloc::string::String::new();
    for core in 1..NUM_CORES {
        let e = &smp7::EV[core];
        if CORE_READY[core].load(Ordering::Acquire) {
            online |= 1 << core;
            detail.push_str(&alloc::format!(
                "core{} ok +{}ms; ",
                core,
                smp7::ms_since(t0, e.ready_at.load(Ordering::Relaxed), freq)
            ));
        } else {
            missed |= 1 << core;
            detail.push_str(&alloc::format!(
                "core{} MISSED stage={} retries={}; ",
                core,
                smp7::stage_name(e.stage.load(Ordering::Relaxed)),
                e.retries.load(Ordering::Relaxed)
            ));
        }
    }
    serial_println!(
        "[smp7] cores online={:#x} missed={:#x} {}",
        online,
        missed,
        detail.trim_end()
    );
}

/// The secondary cores that came online (MPIDR Aff0), for the BSP to place a scheduler workload on.
pub fn online_secondaries() -> alloc::vec::Vec<usize> {
    (1..NUM_CORES).filter(|&c| CORE_READY[c].load(Ordering::Acquire)).collect()
}

/// Busy-wait until `deadline` (a CNTPCT value) for `cond` to hold; returns whether it did. Bounds
/// the SMP handshakes so a wedged core can't hang boot (mirrors the mailbox/xHCI deadline pattern).
fn wait_until(deadline: u64, mut cond: impl FnMut() -> bool) -> bool {
    loop {
        if cond() {
            return true;
        }
        if timer::cntpct() >= deadline {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// BSP: release cores 1-3 from the spin-table, wait for each to come online, then prove the IPI path
/// by sending each an SGI and confirming its handler ran. Write our entry into each slot, clean it
/// to RAM (the parked cores read it MMU-off), one `DSB` + broadcast `SEV`, each core re-checks its
/// own slot and only the one now non-zero proceeds.
pub fn start_secondaries() {
    // The BSP already has its GIC CPU interface (gic::init) and IRQs unmasked; enable the IPI SGI on
    // it too so it can RECEIVE inter-processor pings (M3 has cores wake each other). Harmless in M2.
    gic::enable_sgi(IPI_RESCHED);

    let entry = _secondary_start as usize as u64;

    // SMP-7 HARDENING (always on, no knob — cache maintenance, not a behaviour change).
    //
    // Each AP arrives with its MMU and caches OFF and immediately uses its stack: the compiler's
    // prologue for `__secondary_rust` stores x30/x19/x20 there (`str x30,[sp,#0x10]` /
    // `stp x20,x19,[sp,#0x20]` in the P53 build) BEFORE `drop_to_el1`/`enable_mmu`. Those stores go
    // straight to DRAM, while every access to the same lines after `enable_mmu` is Normal-cacheable —
    // exactly the mismatched-attributes window that produced the CORE3-SMP phantom (arch_arm64.md
    // §CORE3-SMP FIX). The BSP has been running cacheable over all of RAM for the whole boot and can
    // have (speculatively or otherwise) allocated lines covering `SECONDARY_STACKS` into the
    // cluster-shared 1 MiB L2; a stale hit there poisons whatever the AP reloads. The CORE3 fix
    // deleted the one *known* load-bearing reload (the core id); disassembly of this build shows no
    // MMU-off spill is reloaded today, so this is hardening rather than a live-bug fix — but the
    // property currently rests on codegen, and codegen moves every time the image does. Clean +
    // invalidate the whole stack array to PoC here, immediately before the release, so no stale line
    // for any AP stack can survive into its MMU-off window. `DC CIVAC` is broadcast in the
    // inner-shareable domain, the range is 256 KiB (a few thousand ops, once, at boot), and the BSP
    // never touches these stacks again.
    unsafe {
        let stacks = &raw const SECONDARY_STACKS as usize;
        cache::clean_invalidate_range(stacks, SEC_STACK_SIZE * NUM_CORES);
    }

    // Write every AP's release slot, push it to the Point of Coherency (the parked cores read it with
    // their MMU off), then one broadcast SEV. Factored out so the retry path below re-issues the exact
    // same sequence — a retry that skipped the clean or the DSB would be a different experiment.
    let release_all = |cores: &[usize]| unsafe {
        for &core in cores {
            core::ptr::write_volatile(RELEASE_ADDR[core] as *mut u64, entry);
        }
        // All four slots (0xD8..0xF0) fall in one 64-byte cache line, so this single clean covers
        // cores 1-3; DSB completes it before the SEV makes the wake observable.
        let span = (RELEASE_ADDR[NUM_CORES - 1] + 8) - RELEASE_ADDR[1];
        cache::clean_range(RELEASE_ADDR[1], span);
        core::arch::asm!("dsb sy", "sev", options(nostack, preserves_flags));
    };

    #[cfg(feature = "smp7")]
    let t0 = timer::cntpct();
    release_all(&[1, 2, 3]);
    #[cfg(feature = "smp7")]
    for core in 1..NUM_CORES {
        smp7::EV[core].released_at.store(t0, Ordering::Relaxed);
    }
    serial_println!(":: AARCH64 SMP: released cores 1-3 via spin-table (0xE0/0xE8/0xF0) ::");

    let freq = timer::cntfrq();
    // Wait (≤ ~500 ms) for every secondary to publish readiness.
    for core in 1..NUM_CORES {
        let deadline = timer::cntpct() + freq / 2;
        if !wait_until(deadline, || CORE_READY[core].load(Ordering::Acquire)) {
            // SMP-7 RETRY (feature `smp7_retry`, armed by `UNAOS_SMP7_RETRY=1`; DEFAULT OFF, and a
            // strict superset of `smp7` so the retry never runs without the evidence that explains
            // it). A core that has not checked in at all may simply have missed the release; one that
            // checked in and then stopped is dead somewhere in its own bring-up and re-releasing it
            // would be actively wrong (it is not in the spin-table any more). So retry ONLY the
            // never-arrived case, re-issuing the identical write+clean+DSB+SEV. This is a diagnostic
            // instrument as much as a recovery: if a re-release recovers a core, the defect is in the
            // release/wake path; if it never does, the core is losing itself after arrival, and the
            // stage field says where.
            #[cfg(feature = "smp7_retry")]
            {
                const MAX_RETRIES: u32 = 3;
                let mut n = 0;
                while n < MAX_RETRIES
                    && !CORE_READY[core].load(Ordering::Acquire)
                    && smp7::EV[core].checkin_at.load(Ordering::Relaxed) == 0
                {
                    n += 1;
                    smp7::EV[core].retries.store(n, Ordering::Relaxed);
                    release_all(&[core]);
                    let d = timer::cntpct() + freq / 10; // ~100 ms per attempt
                    if wait_until(d, || CORE_READY[core].load(Ordering::Acquire)) {
                        break;
                    }
                }
            }
            if !CORE_READY[core].load(Ordering::Acquire) {
                serial_println!(":: AARCH64 SMP: WARNING core {} did not come online ::", core);
            }
        }
    }

    #[cfg(feature = "smp7")]
    report(t0, freq);

    // IPI smoke test: ping each online core with SGI 0 and confirm its per-CPU counter ticks — proof
    // the GIC delivers a software interrupt from the BSP to another core's CPU interface.
    for core in 1..NUM_CORES {
        if !CORE_READY[core].load(Ordering::Acquire) {
            continue;
        }
        let before = percpu::cpu(core).ipis.load(Ordering::Acquire);
        gic::send_sgi(core, IPI_RESCHED);
        let deadline = timer::cntpct() + freq / 10; // ~100 ms
        let took = wait_until(deadline, || {
            percpu::cpu(core).ipis.load(Ordering::Acquire) > before
        });
        let after = percpu::cpu(core).ipis.load(Ordering::Acquire);
        serial_println!(
            ":: AARCH64 SMP: core {} IPI {} (count {} -> {}) ::",
            core,
            if took { "OK" } else { "TIMEOUT" },
            before,
            after
        );
    }

    // The SGI travels the same Group-1 IRQ path as the generic timer. QEMU raspi4b's GIC model does
    // not deliver Group-1 interrupts (it's why the timer takes the poll-spin fallback there but is
    // LIVE on the real Pi 4), so the IPIs above TIME OUT in QEMU and are expected to be OK on metal.
    // Key the caveat off the timer's measured liveness so the log is self-explanatory.
    if !timer::is_live() {
        serial_println!(
            ":: AARCH64 SMP: (IPI delivery shares the timer's Group-1 path — not modeled by QEMU \
             raspi4b; validate on metal, where the timer is LIVE) ::"
        );
    }
}
