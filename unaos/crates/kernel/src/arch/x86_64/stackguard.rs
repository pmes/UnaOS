// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//! SMALLFIX M6 — STACKGUARD (rmbp-ledger B380; flight 23's MP3HANG). An UNMAPPED 4 KiB page under every x86
//! kernel task stack, so an overflow FAULTS at its first touch instead of writing through the poisoned span
//! into the heap below (flight 23: audio_core's 92,504-byte open frame on the render task's 32 KiB stack —
//! its stack probes wrote zeros every 4 KiB through the guard; no fault, the console and input gone).
//!
//! Layout of a slab (`sched.rs`): `[base, base + STACK_GUARD)` is the guard span (8 KiB), the usable stack
//! above it. The unmapped page is `[align_up(base, 4K), +4K)` — always whole inside the span, whatever the
//! allocator's alignment; the 1..4096 bytes between it and the usable floor stay the poisoned ABSORBER that
//! RENDSTACK's `guard_state` reads (a shallow dip is still a sizing alarm, not a kill).
//!
//! The fault side: a kernel-mode #PF whose CR2 is inside the CURRENT task's guard page — or the #DF it
//! escalates to when the CPU cannot push the #PF frame on the exhausted stack (#DF runs on its own IST) —
//! prints `[stack] OVERFLOW task=<name> stack=<base>..<top> fault=<addr> rip=<addr> via=<pf|df>` and halts
//! THAT TASK (`sched::exit` on a fresh frame at the top of its own slab), not the machine. R83: the first
//! fault on the wire. A task that overflowed while holding a lock still holds it — the line says which task.
//!
//! Boot: one arming line `[stack] guards armed tasks=<n> page=4096` (R80: an arming decision, one line), and
//! `tests stackroom` — each live guarded task's high mark (the `witness` paint) against its size.
//! STACKGUARD2 (B403) covers the rest: every AP stack, every IST stack and the BSP's firmware stack get a
//! guard page ([`arm_cpu`], before the AP runs), the remote flush is an IPI ([`shootdown`]), and a halted task
//! releases the locks `lockowner` names it the holder of. Design: docs/dev/evidence/rmbp-1005/stackguard2.md.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::memory;

pub const PAGE: u64 = 4096;

/// Guards armed since boot / armed now.
static ARMED: AtomicU32 = AtomicU32::new(0);
static LIVE: AtomicU32 = AtomicU32::new(0);
static ARM_FAIL: AtomicU32 = AtomicU32::new(0);
static ARM_WHY: crate::sync::Mutex<&'static str> = crate::sync::Mutex::new("");
/// Serialises the page-table edits (two cores may split the same leaf). Taken with IF=0 only.
static PT_LOCK: crate::sync::Mutex<()> = crate::sync::Mutex::new(());
/// The live guarded slabs `(base, len, name)`, for `tests stackroom`. A slab leaves it (under this lock)
/// before it is freed, so a scan holding the lock never reads freed memory. Full = guarded but unlisted.
const REG_CAP: usize = 256;
static REG: crate::sync::Mutex<[(u64, usize, &'static str); REG_CAP]> = crate::sync::Mutex::new([(0, 0, ""); REG_CAP]);
/// Remote TLB generation: bumped after every unmap; each core drops its global entries when it sees a change.
static TLB_GEN: AtomicU64 = AtomicU64::new(0);
static SEEN_GEN: [AtomicU64; crate::arch::gdt::MAX_CPUS] = [const { AtomicU64::new(0) }; crate::arch::gdt::MAX_CPUS];

/// The guard page of a slab starting at `base` (inside `[base, base + 8 KiB)`).
#[inline]
pub fn guard_page(base: u64) -> u64 {
    (base + PAGE - 1) & !(PAGE - 1)
}

/// Offset (from the slab base) of the first byte ABOVE the guard page: the absorber starts here.
#[inline]
pub fn absorber_lo(base: u64) -> usize {
    (guard_page(base) + PAGE - base) as usize
}

fn set(base: u64, present: bool) -> Result<u32, &'static str> {
    let mut spare = [0u64; 2];
    if !present {
        spare = [memory::stack_guard_frame(), memory::stack_guard_frame()]; // before PT_LOCK: the heap lock stays innermost
    }
    let r = crate::arch::without_interrupts(|| {
        let _g = PT_LOCK.lock();
        memory::stack_guard_page(guard_page(base), present, &mut spare)
    });
    for f in spare {
        memory::stack_guard_frame_free(f);
    }
    r
}

/// Unmap the guard page of a freshly painted slab (`sched.rs` spawn paths, after `paint_stack`).
pub fn arm(base: u64, len: usize, name: &'static str) {
    match set(base, false) {
        Ok(_) => {
            ARMED.fetch_add(1, Ordering::Relaxed);
            LIVE.fetch_add(1, Ordering::Relaxed);
            crate::arch::without_interrupts(|| {
                let mut r = REG.lock();
                if let Some(e) = r.iter_mut().find(|e| e.0 == 0) {
                    *e = (base, len, name);
                }
            });
            shootdown(); // STACKGUARD2 M2: live on every core now, not at its next tick
        }
        Err(why) => {
            ARM_FAIL.fetch_add(1, Ordering::Relaxed);
            *ARM_WHY.lock() = why;
        }
    }
}

/// Map the guard page back before the slab is freed (the allocator writes its free-list node at the
/// block's start). Called from `Task`'s drop; a page that was never armed is already present (no-op).
pub fn disarm(base: u64) {
    crate::arch::without_interrupts(|| {
        let mut r = REG.lock();
        if let Some(e) = r.iter_mut().find(|e| e.0 == base) {
            *e = (0, 0, "");
        }
    });
    if memory::translate(guard_page(base)).is_none() && set(base, true).is_ok() {
        if LIVE.load(Ordering::Relaxed) > 0 {
            LIVE.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// Timer tick (every core) — STACKGUARD2: now the BACKSTOP behind the [`shootdown`] IPI (a core that was
/// masked past the IPI's bound catches up here), and the place a core first says it is online (it ticks,
/// so it is IPI-able). One relaxed load per tick otherwise.
#[inline]
pub fn tlb_sync() {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    if cpu < 32 && ONLINE.load(Ordering::Relaxed) & (1 << cpu) == 0 {
        ONLINE.fetch_or(1 << cpu, Ordering::Relaxed);
    }
    tlb_catch_up(cpu);
}

/// Drop this core's global TLB entries if a guard was unmapped since it last did (CR4.PGE toggle; a CR3
/// reload when PGE is off), THEN ack the generation — the ack means "flushed", never "about to".
#[inline]
fn tlb_catch_up(cpu: usize) {
    let g = TLB_GEN.load(Ordering::Acquire);
    if cpu >= SEEN_GEN.len() || SEEN_GEN[cpu].load(Ordering::Relaxed) >= g {
        return;
    }
    use x86_64::registers::control::{Cr3, Cr4};
    const PGE: u64 = 1 << 7;
    unsafe {
        let cr4 = Cr4::read_raw();
        if cr4 & PGE != 0 {
            Cr4::write_raw(cr4 & !PGE);
            Cr4::write_raw(cr4);
        } else {
            let (f, fl) = Cr3::read_raw();
            Cr3::write_raw(f, fl);
        }
    }
    SEEN_GEN[cpu].fetch_max(g, Ordering::Release);
}

/// The one boot line, on the BSP just before it joins the scheduler; registers `tests stackroom`.
pub fn boot_line() {
    arm_cpu(0); // STACKGUARD2: a no-op when `smp::start_aps` already armed the BSP (a uniprocessor boot did not)
    let fail = ARM_FAIL.load(Ordering::Relaxed);
    let (aps, ist, bsp) = (APS_ARMED.load(Ordering::Relaxed).count_ones(), IST_ARMED.load(Ordering::Relaxed).count_ones(), if BSP_GUARD.load(Ordering::Relaxed) != 0 { "armed" } else { *BSP_WHY.lock() });
    if fail == 0 {
        serial_println!("[stack] guards armed tasks={} aps={} ist={} bsp={} page={}", ARMED.load(Ordering::Relaxed), aps, ist, bsp, PAGE);
    } else {
        serial_println!("[stack] guards armed tasks={} aps={} ist={} bsp={} page={} failed={} why={}", ARMED.load(Ordering::Relaxed), aps, ist, bsp, PAGE, fail, *ARM_WHY.lock());
    }
    crate::tests::register("stackroom", stackroom);
}

/// `tests stackroom`: each live task's high mark against its size (the `witness` paint), and the guards.
pub fn stackroom() {
    let st = super::sched::STACK_GUARD;
    let n = crate::arch::without_interrupts(|| {
        let r = REG.lock();
        let mut n = 0u32;
        for &(base, len, name) in r.iter().filter(|e| e.0 != 0) {
            n += 1;
            let usable = len.saturating_sub(st);
            match super::sched::slab_high(base, len) {
                Some(h) => serial_println!("[stack] room task={} high={} of {} left={} guard={:#x}", name, h, usable, usable.saturating_sub(h), guard_page(base)),
                None => serial_println!("[stack] room task={} high=? of {} (no paint: a build without witness) guard={:#x}", name, usable, guard_page(base)),
            }
        }
        n
    });
    stackroom2(n);
}

// ── The fault side ─────────────────────────────────────────────────────────────────────────────────────

static OVF_FAULT: [AtomicU64; crate::arch::gdt::MAX_CPUS] = [const { AtomicU64::new(0) }; crate::arch::gdt::MAX_CPUS];
static OVF_RIP: [AtomicU64; crate::arch::gdt::MAX_CPUS] = [const { AtomicU64::new(0) }; crate::arch::gdt::MAX_CPUS];
static OVF_DF: [AtomicU64; crate::arch::gdt::MAX_CPUS] = [const { AtomicU64::new(0) }; crate::arch::gdt::MAX_CPUS];

/// Called by the CPL-0 #PF handler and the #DF handler (GS is the kernel's: a CPL-0 fault). If `cr2` is in
/// the current task's guard page, abandon the task — never returns. Otherwise returns (the old fatal path).
pub fn on_kernel_fault(cr2: u64, rip: u64, df: bool) {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    if cpu >= OVF_FAULT.len() {
        return;
    }
    if df && DF_DEPTH[cpu].fetch_add(1, Ordering::Relaxed) > 0 {
        df_reentry(cpu, cr2, rip); // STACKGUARD2: a #DF inside this core's #DF path — never the panic path twice
    }
    let task_hit = super::sched::current_slab(cpu).filter(|&(base, _)| cr2 >= guard_page(base) && cr2 < guard_page(base) + PAGE);
    let Some((base, len)) = task_hit else {
        static_fault(cpu, cr2, rip, df); // STACKGUARD2: an AP / BSP / IST guard is fatal, named first; else returns
        return;
    };
    OVF_FAULT[cpu].store(cr2, Ordering::Relaxed);
    OVF_RIP[cpu].store(rip, Ordering::Relaxed);
    OVF_DF[cpu].store(df as u64, Ordering::Relaxed);
    // A fresh frame at the top of the task's own slab: every frame on it is being abandoned.
    let rsp = ((base + len as u64) & !0xF) - 64;
    unsafe {
        core::arch::asm!(
            "mov rsp, {0}",
            "xor ebp, ebp",
            "call {1}",
            "ud2",
            in(reg) rsp,
            sym overflow_exit,
            options(noreturn)
        );
    }
}

extern "C" fn overflow_exit() -> ! {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    DF_DEPTH[cpu].store(0, Ordering::Relaxed); // this core left its #DF path for good (we run on the dead task's slab)
    let (fault, rip, df) = (OVF_FAULT[cpu].load(Ordering::Relaxed), OVF_RIP[cpu].load(Ordering::Relaxed), OVF_DF[cpu].load(Ordering::Relaxed));
    // STACKGUARD2 M3: release what the dead task is KNOWN to hold BEFORE its line, so the line reaches the
    // wire (the FTDI ring is the rMBP's wire). Replaces the 20M-spin wait on SERIAL1 + machine-wide panic mode.
    let released = release_held(super::sched::current_task_id(cpu).unwrap_or(0));
    if let Some((name, base, len)) = super::sched::current_named_slab(cpu) {
        serial_println!(
            "[stack] OVERFLOW task={} stack={:#x}..{:#x} fault={:#x} rip={:#x} via={} -> task halted",
            name,
            base + super::sched::STACK_GUARD as u64,
            base + len as u64,
            fault,
            rip,
            if df != 0 { "df" } else { "pf" }
        );
        serial_println!("[stack] overflow task={} released=[{}]", name, Released(released));
        OVF_LAST.store(((cpu as u64) << 8) | df, Ordering::Relaxed);
        OVF_COUNT.fetch_add(1, Ordering::Release);
        // Re-lay the absorber so RENDSTACK's switch-out check does not read the abandoned overflow as a
        // TRAVERSED guard and panic the machine this path exists to keep.
        super::sched::stack_repaint_absorber(cpu);
    }
    super::sched::exit()
}

// ══ STACKGUARD2 (rmbp-ledger B403) ══════════════════════════════════════════════════════════════════════
// Design and the #DF IST reasoning: docs/dev/evidence/rmbp-1005/stackguard2.md. In short: a guard under the
// #DF IST cannot triple-fault (delivery loads the mapped IST TOP); a #DF handler deep enough to reach it
// re-enters #DF, and the per-core depth below halts that core with one line instead of looping.

use core::sync::atomic::AtomicU8;
use crate::arch::gdt::MAX_CPUS;

/// Bit `cpu` = that AP's stack guard is unmapped.
static APS_ARMED: AtomicU32 = AtomicU32::new(0);
/// Bit `cpu * 4 + k` = that CPU's IST slot `k` guard is unmapped (MAX_CPUS = 8 -> 32 bits).
static IST_ARMED: AtomicU32 = AtomicU32::new(0);
/// Bit `cpu` = `arm_cpu(cpu)` ran (idempotence).
static CPU_DONE: AtomicU32 = AtomicU32::new(0);
/// The BSP firmware stack: its guard page (0 = not armed), the top `_start` saw, the painted floor.
static BSP_GUARD: AtomicU64 = AtomicU64::new(0);
static BSP_TOP: AtomicU64 = AtomicU64::new(0);
static BSP_PAINT_LO: AtomicU64 = AtomicU64::new(0);
static BSP_WHY: crate::sync::Mutex<&'static str> = crate::sync::Mutex::new("owed(not-reached)");
/// Cores that have ticked (IPI-able).
static ONLINE: AtomicU32 = AtomicU32::new(0);
/// Per-core #DF depth (reset when a task overflow leaves the #DF path for the dead task's slab).
static DF_DEPTH: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];
/// A static-stack overflow's kind, for the fresh-frame exit: (kind << 8) | index.
static OVF_STATIC: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Task overflows halted since boot, and the last one's `(cpu << 8) | df` (for `tests stackroom overflow`).
static OVF_COUNT: AtomicU32 = AtomicU32::new(0);
static OVF_LAST: AtomicU64 = AtomicU64::new(0);
/// The shootdown IPI's vector (0 = not allocated: the tick backstop only) and its tallies.
static TLB_VEC: AtomicU8 = AtomicU8::new(0);
static SHOOTS: AtomicU32 = AtomicU32::new(0);
static SHOOT_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static SHOOT_SAID: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The paint a static stack is laid with at arm time (its high mark is the lowest byte that differs).
const PAINT: u8 = 0xC5;
/// The UEFI minimum boot-services stack (spec §2.3.6): the BSP guard is armed only below this much room.
const BSP_MIN_ROOM: u64 = 128 * 1024;
/// How much of the BSP stack below `_start`'s RSP is painted (inside the UEFI minimum, so it is the stack's).
const BSP_PAINT: u64 = 120 * 1024;
/// The shootdown's bound per core-set.
const SHOOT_BOUND_US: u64 = 2000;

const K_BSP: u64 = 1;
const K_AP: u64 = 2;
const K_IST: u64 = 3; // + slot

fn kind_name(code: u64) -> (&'static str, usize) {
    let (k, i) = (code >> 8, (code & 0xFF) as usize);
    match k {
        K_BSP => ("bsp", 0),
        K_AP => ("ap", i),
        _ => (["ist-df", "ist-nmi", "ist-db", "ist-mc"][((k - K_IST) as usize).min(3)], i),
    }
}

fn paint(lo: u64, hi: u64) {
    if hi > lo {
        // SAFETY: callers pass a span of a stack nothing is running on (an AP before its SIPI, an IST below
        // its top 2 KiB, the BSP's stack below its live frame).
        unsafe { core::ptr::write_bytes(lo as *mut u8, PAINT, (hi - lo) as usize) };
    }
}

/// Bytes used of `[lo, top)` by its paint: `top - lowest byte that differs` (`None` = saturated: the floor differs).
fn high_mark(lo: u64, top: u64) -> Option<u64> {
    let mut a = lo;
    while a < top {
        // SAFETY: a mapped static stack span.
        if unsafe { core::ptr::read_volatile(a as *const u8) } != PAINT {
            return if a == lo { None } else { Some(top - a) };
        }
        a += 1;
    }
    Some(0)
}

fn arm_static(guard: u64) -> Result<(), &'static str> {
    set(guard, false).map(|_| {
        shootdown();
    })
}

/// Arm `cpu`'s static stack guards. AP: called by `smp::start_aps` BEFORE its SIPI — the stack and the four
/// IST stacks are painted whole and their guard pages unmapped before the AP ever runs on them. BSP (0): its
/// IST stacks (painted below their top 2 KiB: they are live) and its firmware stack; turns `lockowner` on and
/// allocates the shootdown vector. Idempotent.
pub fn arm_cpu(cpu: usize) {
    if cpu >= MAX_CPUS || CPU_DONE.fetch_or(1 << cpu, Ordering::AcqRel) & (1 << cpu) != 0 {
        return;
    }
    if cpu == 0 {
        crate::lockowner::enable();
        if TLB_VEC.load(Ordering::Relaxed) == 0 {
            if let Some(v) = super::interrupts::vectors::alloc("tlb", tlb_ipi_handler) {
                TLB_VEC.store(v, Ordering::Release);
            }
        }
        arm_bsp();
    } else if let Some((g, lo, top)) = super::smp::ap_guard_bounds(cpu) {
        paint(lo, top);
        match arm_static(g) {
            Ok(()) => { APS_ARMED.fetch_or(1 << cpu, Ordering::Relaxed); }
            Err(why) => { ARM_FAIL.fetch_add(1, Ordering::Relaxed); *ARM_WHY.lock() = why; }
        }
    }
    for k in 0..crate::arch::gdt::IST_SLOTS {
        let Some((g, lo, top)) = crate::arch::gdt::ist_bounds(cpu, k) else { continue };
        crate::arch::without_interrupts(|| paint(lo, if cpu == 0 { top - 2048 } else { top }));
        match arm_static(g) {
            Ok(()) => { IST_ARMED.fetch_or(1 << (cpu * 4 + k), Ordering::Relaxed); }
            Err(why) => { ARM_FAIL.fetch_add(1, Ordering::Relaxed); *ARM_WHY.lock() = why; }
        }
    }
}

/// The BSP runs on the firmware's stack (UEFI BootServicesData, `Reserved` in our map; the kernel makes no
/// runtime-services call). Its floor is the floor of the UEFI descriptor holding `_start`'s RSP; armed only
/// when that descriptor is `Reserved` and leaves at least the UEFI minimum (128 KiB) below the top.
fn arm_bsp() {
    let top = crate::prof::boot_stack_top();
    let Some((start, _end)) = super::memory::stackguard_reserved_region(top) else {
        *BSP_WHY.lock() = "owed(no-reserved-region)";
        return;
    };
    let g = (start + PAGE - 1) & !(PAGE - 1);
    if top < g + PAGE + BSP_MIN_ROOM {
        *BSP_WHY.lock() = "owed(region-under-128k)";
        return;
    }
    let sp: u64;
    unsafe { core::arch::asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack, preserves_flags)) };
    let lo = (top - BSP_PAINT).max(g + PAGE);
    crate::arch::without_interrupts(|| paint(lo, sp.saturating_sub(4096)));
    BSP_PAINT_LO.store(lo, Ordering::Relaxed);
    BSP_TOP.store(top, Ordering::Relaxed);
    match arm_static(g) {
        Ok(()) => BSP_GUARD.store(g, Ordering::Release),
        Err(why) => *BSP_WHY.lock() = why,
    }
}

// ── M2: the shootdown IPI ─────────────────────────────────────────────────────────────────────────────

extern "x86-interrupt" fn tlb_ipi_handler(frame: x86_64::structures::idt::InterruptStackFrame) {
    // Maskable, like the reschedule IPI: from ring 3 the GS is the user's — swap around the per-CPU read.
    let user = frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;
    if user {
        unsafe { core::arch::asm!("swapgs", options(nostack, preserves_flags)) };
    }
    tlb_catch_up(crate::arch::percpu::this_cpu().cpu_index as usize);
    super::apic::eoi();
    if user {
        unsafe { core::arch::asm!("swapgs", options(nostack, preserves_flags)) };
    }
}

/// After an unmap (this core already `invlpg`'d): every other ONLINE core drops its global entries NOW — a
/// fixed IPI, then a bounded wait for each core's ack generation. Independent of the tick (CLOCKCORE B397):
/// it needs only the LAPIC ICR and the TSC. While it waits it acks any concurrent shootdown itself, so two
/// masked senders cannot wait on each other. Returns `(cpus, acked)`.
pub fn shootdown() -> (u32, u32) {
    let g = TLB_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    let me = crate::arch::percpu::this_cpu().cpu_index as usize;
    if me < SEEN_GEN.len() {
        SEEN_GEN[me].fetch_max(g, Ordering::Release);
    }
    let v = TLB_VEC.load(Ordering::Acquire);
    let mask = ONLINE.load(Ordering::Acquire) & !(1u32 << me);
    if v == 0 || mask == 0 {
        return (0, 0);
    }
    let t0 = crate::arch::now_cycles();
    for c in (0..MAX_CPUS).filter(|c| mask & (1 << c) != 0) {
        if let Some(p) = super::percpu::cpu(c) {
            let id = p.apic_id;
            crate::arch::without_interrupts(|| super::apic::send_ipi(id, 0x0000_4000 | v as u32));
        }
    }
    let hz = super::apic::tsc_hz();
    let bound = if hz == 0 { 0 } else { hz / 1_000_000 * SHOOT_BOUND_US };
    let cpus = mask.count_ones();
    let mut spins = 0u64;
    let acked = loop {
        let acked = (0..MAX_CPUS).filter(|&c| mask & (1 << c) != 0 && SEEN_GEN[c].load(Ordering::Acquire) >= g).count() as u32;
        if acked == cpus {
            break acked;
        }
        let late = if hz == 0 { spins > 2_000_000 } else { crate::arch::now_cycles().wrapping_sub(t0) > bound };
        if late {
            break acked;
        }
        tlb_catch_up(me);
        spins += 1;
        core::hint::spin_loop();
    };
    let us = if hz == 0 { 0 } else { crate::arch::now_cycles().wrapping_sub(t0) / (hz / 1_000_000).max(1) };
    SHOOTS.fetch_add(1, Ordering::Relaxed);
    if acked < cpus {
        let t = SHOOT_TIMEOUTS.fetch_add(1, Ordering::Relaxed) + 1;
        if t > 3 && t % 256 != 0 {
            return (cpus, acked); // the first three, then one in 256: a masked core is a fact, not a flood (QUIETBOOT)
        }
        let missing = (0..MAX_CPUS).filter(|&c| mask & (1 << c) != 0 && SEEN_GEN[c].load(Ordering::Acquire) < g).fold(0u32, |m, c| m | (1 << c));
        serial_println!("[tlb] shootdown pages=1 cpus={} acked={} us={} timeout=1 timeouts={} missing_mask={} (the tick catches them up)", cpus, acked, us, t, missing);
    } else if !SHOOT_SAID.swap(true, Ordering::Relaxed) {
        serial_println!("[tlb] shootdown pages=1 cpus={} acked={} us={}", cpus, acked, us);
    }
    (cpus, acked)
}

// ── M1: the static stacks' fault side ─────────────────────────────────────────────────────────────────

fn static_hit(cr2: u64) -> Option<(u64, u64, u64)> {
    let pg = cr2 & !(PAGE - 1);
    let b = BSP_GUARD.load(Ordering::Acquire);
    if b != 0 && pg == b {
        return Some((K_BSP << 8, BSP_PAINT_LO.load(Ordering::Relaxed), BSP_TOP.load(Ordering::Relaxed)));
    }
    let aps = APS_ARMED.load(Ordering::Acquire);
    for c in (1..MAX_CPUS).filter(|c| aps & (1 << c) != 0) {
        if let Some((g, lo, top)) = super::smp::ap_guard_bounds(c) {
            if pg == g {
                return Some(((K_AP << 8) | c as u64, lo, top));
            }
        }
    }
    let ist = IST_ARMED.load(Ordering::Acquire);
    for c in 0..MAX_CPUS {
        for k in 0..crate::arch::gdt::IST_SLOTS {
            if ist & (1 << (c * 4 + k)) == 0 {
                continue;
            }
            if let Some((g, lo, top)) = crate::arch::gdt::ist_bounds(c, k) {
                if pg == g {
                    return Some((((K_IST + k as u64) << 8) | c as u64, lo, top));
                }
            }
        }
    }
    None
}

/// A fault in an AP / BSP / IST guard page: fatal by nature (an idle/scheduler or handler context has no task
/// to halt). Moves to a fresh frame at the top of this core's #DF IST (marking the #DF path taken), names the
/// overflow, panics. Returns when `cr2` is in no static guard (the old fatal path follows).
fn static_fault(cpu: usize, cr2: u64, rip: u64, df: bool) {
    let Some((code, _, _)) = static_hit(cr2) else { return };
    OVF_FAULT[cpu].store(cr2, Ordering::Relaxed);
    OVF_RIP[cpu].store(rip, Ordering::Relaxed);
    OVF_DF[cpu].store(df as u64, Ordering::Relaxed);
    OVF_STATIC[cpu].store(code, Ordering::Relaxed);
    DF_DEPTH[cpu].store(1, Ordering::Relaxed);
    let Some((_, _, top)) = crate::arch::gdt::ist_bounds(cpu, 0) else { return };
    let rsp = (top & !0xF) - 64;
    unsafe {
        core::arch::asm!("mov rsp, {0}", "xor ebp, ebp", "call {1}", "ud2", in(reg) rsp, sym static_exit, options(noreturn));
    }
}

extern "C" fn static_exit() -> ! {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    crate::serial_ring::enter_panic_mode(); // the machine is about to panic: the lock-free wire, as the panic handler does
    let code = OVF_STATIC[cpu].load(Ordering::Relaxed);
    let (name, idx) = kind_name(code);
    let (lo, top) = static_hit(OVF_FAULT[cpu].load(Ordering::Relaxed)).map(|(_, l, t)| (l, t)).unwrap_or((0, 0));
    serial_println!(
        "[stack] OVERFLOW stack={}{} cpu={} stack={:#x}..{:#x} fault={:#x} rip={:#x} via={} -> panic (no task to halt)",
        name,
        idx,
        cpu,
        lo,
        top,
        OVF_FAULT[cpu].load(Ordering::Relaxed),
        OVF_RIP[cpu].load(Ordering::Relaxed),
        if OVF_DF[cpu].load(Ordering::Relaxed) != 0 { "df" } else { "pf" }
    );
    panic!("stack overflow: {}{} on cpu {}", name, idx, cpu);
}

/// A #DF while this core is already in its #DF path (the panic path ran its IST into the guard): one
/// lock-free line, then this core stops with IF=0. Never the panic path a second time.
fn df_reentry(cpu: usize, cr2: u64, rip: u64) -> ! {
    crate::serial_ring::enter_panic_mode();
    serial_println!("[stack] OVERFLOW stack=ist-df{} fault={:#x} rip={:#x} via=df -> core halted (a #DF inside the #DF path)", cpu, cr2, rip);
    loop {
        unsafe { core::arch::asm!("cli; hlt", options(nomem, nostack)) };
    }
}

// ── M3: what a halted task is known to hold ───────────────────────────────────────────────────────────

/// Force-release every `lockowner` lock whose recorded holder is `tid`; the bit set of what was released.
fn release_held(tid: u64) -> u32 {
    use crate::lockowner as lo;
    let mut out = 0u32;
    if lo::held_by(lo::SINK, tid) {
        lo::clear(lo::SINK);
        crate::drivers::xhci::ftdi::lockowner_release();
        out |= 1 << lo::SINK;
    }
    if lo::held_by(lo::UART, tid) {
        lo::clear(lo::UART);
        super::serial::lockowner_release();
        out |= 1 << lo::UART;
    }
    #[cfg(feature = "unafs")]
    if lo::held_by(lo::UNAFS, tid) {
        lo::clear(lo::UNAFS);
        crate::fs::unafs::lockowner_release();
        out |= 1 << lo::UNAFS;
    }
    out
}

struct Released(u32);
impl core::fmt::Display for Released {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut first = true;
        for (i, n) in crate::lockowner::NAMES.iter().enumerate() {
            if self.0 & (1 << i) != 0 {
                if !first {
                    f.write_str(",")?;
                }
                f.write_str(n)?;
                first = false;
            }
        }
        Ok(())
    }
}

// ── M4/M5: `tests stackroom` over every kind, and the witness ─────────────────────────────────────────

fn room_line(kind: &str, idx: usize, lo: u64, top: u64, guard: u64) {
    let size = top - lo;
    match high_mark(lo, top) {
        Some(h) => serial_println!("[stack] room kind={}{} high={} of {} left={} guard={:#x}", kind, idx, h, size, size - h, guard),
        None => serial_println!("[stack] room kind={}{} high>={} of {} left=? guard={:#x} (the painted floor was reached)", kind, idx, size, size, guard),
    }
}

/// The static kinds' high marks and the witness (`n` = the live tasks `stackroom` already listed).
fn stackroom2(n: u32) {
    let b = BSP_GUARD.load(Ordering::Acquire);
    if b != 0 {
        room_line("bsp", 0, BSP_PAINT_LO.load(Ordering::Relaxed), BSP_TOP.load(Ordering::Relaxed), b);
    } else {
        serial_println!("[stack] room kind=bsp0 guard=none reason={}", *BSP_WHY.lock());
    }
    let aps = APS_ARMED.load(Ordering::Acquire);
    for c in (1..MAX_CPUS).filter(|c| aps & (1 << c) != 0) {
        if let Some((g, lo, top)) = super::smp::ap_guard_bounds(c) {
            room_line("ap", c, lo, top, g);
        }
    }
    let ist = IST_ARMED.load(Ordering::Acquire);
    for c in 0..MAX_CPUS {
        for k in 0..crate::arch::gdt::IST_SLOTS {
            if ist & (1 << (c * 4 + k)) != 0 {
                if let Some((g, lo, top)) = crate::arch::gdt::ist_bounds(c, k) {
                    room_line(kind_name((K_IST + k as u64) << 8).0, c, lo, top, g);
                }
            }
        }
    }
    // The flush, proven now (a test-time probe, R80): one shootdown to every online core, every ack back.
    let (cpus, acked) = shootdown();
    let ipi = if TLB_VEC.load(Ordering::Relaxed) == 0 {
        "novector"
    } else if cpus == 0 {
        "solo"
    } else if acked == cpus && SHOOT_TIMEOUTS.load(Ordering::Relaxed) == 0 {
        "ok"
    } else {
        "timeout"
    };
    let fail = ARM_FAIL.load(Ordering::Relaxed);
    let pass = fail == 0 && ARMED.load(Ordering::Relaxed) > 0 && ist != 0 && (ipi == "ok" || ipi == "solo");
    serial_println!(
        ":: STACKROOM: tasks={} aps={} ist={} armed={} live={} failed={} ipi_flush={} shootdowns={} timeouts={} bsp={} page={} -> {} ::",
        n,
        aps.count_ones(),
        ist.count_ones(),
        ARMED.load(Ordering::Relaxed),
        LIVE.load(Ordering::Relaxed),
        fail,
        ipi,
        SHOOTS.load(Ordering::Relaxed),
        SHOOT_TIMEOUTS.load(Ordering::Relaxed),
        if b != 0 { "armed" } else { "owed" },
        PAGE,
        if pass { "PASS" } else { "FAIL" }
    );
    if crate::tests::arg().as_deref().map(str::trim) == Some("overflow") {
        overflow_test();
    }
}

/// `tests stackroom overflow` (test-only; R80: never at boot): a scratch task on a worker core recurses off
/// its 16 KiB stack; the chain is proven end to end when its OVERFLOW line lands, the task is halted, and the
/// #DF IST of the core it died on shows the recovery's footprint.
fn overflow_test() {
    const SCRATCH: usize = 16 * 1024;
    let before = OVF_COUNT.load(Ordering::Acquire);
    let here = crate::arch::percpu::this_cpu().cpu_index as usize;
    let cpu = (0..4).filter_map(super::smp::worker_cpu).find(|&c| c != here).unwrap_or(super::sched::CPU_AUTO);
    super::sched::spawn_stack("sg-scratch", scratch_entry, 0, cpu, super::sched::PRIO_NORMAL, SCRATCH);
    let mut waited = 0u64;
    while OVF_COUNT.load(Ordering::Acquire) == before && waited < 3000 {
        super::sched::sleep_ms(10);
        waited += 10;
    }
    let halted = OVF_COUNT.load(Ordering::Acquire) != before;
    let last = OVF_LAST.load(Ordering::Relaxed);
    let (dcpu, via) = ((last >> 8) as usize, if last & 1 != 0 { "df" } else { "pf" });
    let df_high = crate::arch::gdt::ist_bounds(dcpu, 0).and_then(|(_, lo, top)| high_mark(lo, top).map(|h| (h, top - lo)));
    let (h, size) = df_high.unwrap_or((0, 0));
    serial_println!(
        ":: STACKOVF: task=sg-scratch cpu={} via={} halted={} waited_ms={} df_ist_high={} of {} -> {} ::",
        dcpu,
        if halted { via } else { "none" },
        halted as u8,
        waited,
        h,
        size,
        if halted && (via == "pf" || h > 0) { "PASS" } else { "FAIL" }
    );
}

fn scratch_entry(_: usize) {
    let n = scratch_deep(0);
    serial_println!("[stack] sg-scratch returned depth={} (the guard did not fire) ", n);
}

#[inline(never)]
#[allow(unconditional_recursion)]
fn scratch_deep(d: u64) -> u64 {
    let mut b = [0u8; 1024];
    core::hint::black_box(&mut b);
    b[(d & 0x3FF) as usize] = d as u8;
    if d == u64::MAX {
        return 0;
    }
    scratch_deep(d + 1).wrapping_add(core::hint::black_box(&b)[7] as u64)
}
