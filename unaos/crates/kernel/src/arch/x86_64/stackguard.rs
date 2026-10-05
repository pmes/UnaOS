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
//! Not covered: the AP boot stacks (`smp.rs`, a static array — no page boundary of their own) and the BSP's
//! firmware stack; the IST stacks (`gdt.rs`).

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::memory;

pub const PAGE: u64 = 4096;

/// Guards armed since boot / armed now.
static ARMED: AtomicU32 = AtomicU32::new(0);
static LIVE: AtomicU32 = AtomicU32::new(0);
static ARM_FAIL: AtomicU32 = AtomicU32::new(0);
static ARM_WHY: spin::Mutex<&'static str> = spin::Mutex::new("");
/// Serialises the page-table edits (two cores may split the same leaf). Taken with IF=0 only.
static PT_LOCK: spin::Mutex<()> = spin::Mutex::new(());
/// The live guarded slabs `(base, len, name)`, for `tests stackroom`. A slab leaves it (under this lock)
/// before it is freed, so a scan holding the lock never reads freed memory. Full = guarded but unlisted.
const REG_CAP: usize = 256;
static REG: spin::Mutex<[(u64, usize, &'static str); REG_CAP]> = spin::Mutex::new([(0, 0, ""); REG_CAP]);
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
            TLB_GEN.fetch_add(1, Ordering::Release);
            crate::arch::without_interrupts(|| {
                let mut r = REG.lock();
                if let Some(e) = r.iter_mut().find(|e| e.0 == 0) {
                    *e = (base, len, name);
                }
            });
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

/// Timer tick (every core): when another core unmapped a guard since this core last looked, drop this
/// core's global TLB entries (CR4.PGE toggle; a CR3 reload when PGE is off) so its stale 2 MiB / 1 GiB
/// translation of that page goes. One relaxed load per tick otherwise.
#[inline]
pub fn tlb_sync() {
    let g = TLB_GEN.load(Ordering::Acquire);
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    if cpu >= SEEN_GEN.len() || SEEN_GEN[cpu].load(Ordering::Relaxed) == g {
        return;
    }
    SEEN_GEN[cpu].store(g, Ordering::Relaxed);
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
}

/// The one boot line, on the BSP just before it joins the scheduler; registers `tests stackroom`.
pub fn boot_line() {
    let fail = ARM_FAIL.load(Ordering::Relaxed);
    if fail == 0 {
        serial_println!("[stack] guards armed tasks={} page={}", ARMED.load(Ordering::Relaxed), PAGE);
    } else {
        serial_println!("[stack] guards armed tasks={} page={} failed={} why={}", ARMED.load(Ordering::Relaxed), PAGE, fail, *ARM_WHY.lock());
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
    serial_println!(
        ":: STACKROOM: tasks={} armed={} live={} failed={} page={} -> {} ::",
        n,
        ARMED.load(Ordering::Relaxed),
        LIVE.load(Ordering::Relaxed),
        ARM_FAIL.load(Ordering::Relaxed),
        PAGE,
        if ARM_FAIL.load(Ordering::Relaxed) == 0 && ARMED.load(Ordering::Relaxed) > 0 { "PASS" } else { "FAIL" }
    );
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
    let Some((base, len)) = super::sched::current_slab(cpu) else { return };
    let g = guard_page(base);
    if cr2 < g || cr2 >= g + PAGE {
        return;
    }
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
    let (fault, rip, df) = (OVF_FAULT[cpu].load(Ordering::Relaxed), OVF_RIP[cpu].load(Ordering::Relaxed), OVF_DF[cpu].load(Ordering::Relaxed));
    // The dead task may have held the serial lock: wait a bounded while, then take the lock-free path.
    let mut spins = 0u32;
    while super::serial::SERIAL1.is_locked() && spins < 20_000_000 {
        core::hint::spin_loop();
        spins += 1;
    }
    if super::serial::SERIAL1.is_locked() {
        crate::serial_ring::enter_panic_mode();
    }
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
        // Re-lay the absorber so RENDSTACK's switch-out check does not read the abandoned overflow as a
        // TRAVERSED guard and panic the machine this path exists to keep.
        super::sched::stack_repaint_absorber(cpu);
    }
    super::sched::exit()
}
