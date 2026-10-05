// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! CHARTER: Kernel — kernel-by-ruling (R83: built in UnaOS; rmbp-ledger B331 PROFILE, B340 PROFILE2)
//!
//! PROFILE — the sampling profiler. Peter (2026-10-04): "do we have some kind of profiling so we can
//! see where things are congesting?" The kernel had stage timings and rollups, each answering a
//! question asked in advance at one site; nothing said WHERE the machine spends its time. This does:
//! the timer interrupt both arches already take records, when armed, what it interrupted.
//!
//! * **Sampler (B331 M1, B340 M1).** [`sample_tick`] (folded onto the x86 timer ISR's `note_tick()`
//!   line) and [`on_tick_arm`] (folded onto aarch64 `timer::on_tick`'s tail) record a sample — header
//!   `(tid, program id, depth, ring, cpu)`, the interrupted PC, then up to [`MAX_DEPTH`] caller return
//!   addresses from a frame-pointer walk — into THIS CPU's ring of [`RING_WORDS`] words every `div`-th
//!   tick. One writer per ring (the owning CPU, interrupts masked), published through `LEN` with
//!   Release; a full ring counts `dropped` and never overwrites. The ISR never allocates: the rings are
//!   heap-allocated by the FIRST `prof start` and kept for every later run. Disarmed cost: one relaxed
//!   load per tick. The walk validates every frame pointer against the CURRENT stack's bounds before it
//!   dereferences it and every return address against the kernel image's executable range, so it cannot
//!   fault (see [`walk_ok`]).
//! * **Symbols (B331 M2, B340 M3).** The kernel has no symbol table at runtime and embeds none. arroyo
//!   writes `target/kernel.syms` (`llvm-nm -n -C --defined-only`) beside the ELF it links and
//!   `target/APPS/<NAME>.syms` beside every staged ring-3 ELF; the kernel is PIE, so every dump carries
//!   `anchor=` — the runtime address of [`unaos_prof_anchor`] — and `tools/flame` derives the slide from
//!   that symbol's syms line. Ring-3 samples carry the task's program id; `prof dump` prints the table.
//! * **Names (B340 M2).** A lock-free per-task name table written at spawn ([`note_task`]), and a
//!   per-CPU pending program name a launch arms and the next ring-3 spawn consumes ([`launching`]).
//! * **Views (B331 M3, B340 M4).** Per-syscall log2 latency histograms ([`sys_t0`]/[`sys_note`], and
//!   [`sys_note_linux`] for the Linux-ABI dispatcher) behind `prof sys`; per-task CPU share behind
//!   `prof tasks`.
//! * **Compositor + fixtures.** `prof top` closes with the `[comp2]` present/compose/blit split;
//!   `tests prof` (B331) and `tests prof2` (B340) are the witnesses; [`sys_prof`] is `SYS_PROF` (B340 M5).
//!
//! Off by default; nothing prints at boot (R80). Design: `docs/dev/evidence/rmbp-1005/PROFILE.md`,
//! `PROFILE2.md`.
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{fence, AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::{AcqRel, Acquire, Relaxed, Release}};

use crate::console::Console;

/// Words per CPU ring. A sample is `2 + depth` words (header, PC, callers).
pub const RING_WORDS: usize = 32768;
/// Deepest caller chain one sample records.
pub const MAX_DEPTH: usize = 32;
/// CPU slots (the larger of the two arches' `MAX_CPUS`/`NUM_CPUS`).
pub const CPUS: usize = 8;
/// RIP bucket granularity for `prof top` (log2 bytes): 256-byte buckets.
pub const BUCKET_SHIFT: u32 = 8;

static ARMED: AtomicBool = AtomicBool::new(false);
static DIV: AtomicU32 = AtomicU32::new(1);
static T_START: AtomicU64 = AtomicU64::new(0);
static T_STOP: AtomicU64 = AtomicU64::new(0);
static COUNTDOWN: [AtomicU32; CPUS] = [const { AtomicU32::new(1) }; CPUS];
/// Words published in each ring.
static LEN: [AtomicUsize; CPUS] = [const { AtomicUsize::new(0) }; CPUS];
/// Samples published in each ring.
static NSAMP: [AtomicUsize; CPUS] = [const { AtomicUsize::new(0) }; CPUS];
static DROPPED: [AtomicU64; CPUS] = [const { AtomicU64::new(0) }; CPUS];
/// Per-CPU sample ring of [`RING_WORDS`] words: `[header, pc, caller0 .. caller(depth-1)]` per sample,
/// header = `(tid << 32) | (prog << 24) | (depth << 16) | (ring << 8) | cpu`.
static RING: [spin::Once<&'static [AtomicU64]>; CPUS] = [const { spin::Once::new() }; CPUS];
/// The kernel image's executable range `[lo, hi)` (computed at the first `prof start`; 0/0 = unknown,
/// and then no walk runs).
static TEXT_LO: AtomicU64 = AtomicU64::new(0);
static TEXT_HI: AtomicU64 = AtomicU64::new(0);
/// The BSP's boot-stack top as `_start` saw it (x86; the BSP keeps its UEFI stack).
#[cfg(target_arch = "x86_64")]
static BOOT_TOP: AtomicU64 = AtomicU64::new(0);
/// Ring-0 samples whose walk found no interrupt boundary (an image built without frame pointers).
static WALK_MISS: AtomicU64 = AtomicU64::new(0);

/// The timer interrupt's rate on this arch: the sampler's ceiling.
pub fn tick_hz() -> u32 {
    #[cfg(target_arch = "x86_64")]
    return crate::arch::apic::TICK_HZ as u32;
    #[cfg(target_arch = "aarch64")]
    return crate::arch::timer::TICK_HZ as u32;
}

/// `now_cycles()` rate in Hz (calibrated TSC on x86, CNTFRQ on aarch64); 0 if not yet known.
pub fn cycle_hz() -> u64 {
    #[cfg(target_arch = "x86_64")]
    return crate::arch::apic::tsc_hz();
    #[cfg(target_arch = "aarch64")]
    return crate::arch::timer::cntfrq();
}

/// `now_cycles()` units to nanoseconds (cycles verbatim when the rate is unknown).
pub fn cyc_to_ns(cyc: u64) -> u64 {
    let hz = cycle_hz();
    if hz == 0 { cyc } else { ((cyc as u128 * 1_000_000_000) / hz as u128) as u64 }
}

/// The symbol `tools/flame` anchors the PIE slide on: its runtime address is printed beside every
/// dump, its link address is its line in `target/kernel.syms`.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn unaos_prof_anchor() -> u64 {
    0x5052_4f46 // "PROF"
}

/// Runtime address of [`unaos_prof_anchor`].
pub fn anchor() -> u64 {
    unaos_prof_anchor as *const () as usize as u64
}

/// Is the sampler armed?
#[inline]
pub fn armed() -> bool {
    ARMED.load(Relaxed)
}

/// x86 `_start` (main.rs): record the BSP's boot-stack top — the stack pointer of the entry frame, above
/// which no kernel frame lives — so a walk outside any task (the BSP's main loop) has a proven-mapped
/// `[sp, top)`. Inlined, so it reads `_start`'s own stack pointer.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub fn note_boot_stack() {
    let sp: u64;
    unsafe { core::arch::asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack, preserves_flags)) };
    BOOT_TOP.store(sp, Relaxed);
}

/// Tick bookkeeping: `Some(cpu)` when THIS tick is due a sample on this CPU.
#[inline]
fn due() -> Option<usize> {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    if cpu >= CPUS {
        return None;
    }
    let c = COUNTDOWN[cpu].load(Relaxed);
    if c > 1 {
        COUNTDOWN[cpu].store(c - 1, Relaxed);
        return None;
    }
    COUNTDOWN[cpu].store(DIV.load(Relaxed).max(1), Relaxed);
    Some(cpu)
}

/// The one recording path both arches' ISR hooks share. Interrupts are masked here.
#[inline]
fn push(cpu: usize, pc: u64, ring: u8, callers: &[u64]) {
    let Some(buf) = RING[cpu].get() else { return };
    let n = LEN[cpu].load(Relaxed);
    let d = callers.len().min(MAX_DEPTH);
    if n + 2 + d > buf.len() {
        DROPPED[cpu].fetch_add(1, Relaxed);
        return;
    }
    let tid = crate::arch::sched::current_task_id(cpu).unwrap_or(0) & 0xFFFF_FFFF;
    let prog = prog_of(tid) as u64;
    buf[n].store((tid << 32) | (prog << 24) | ((d as u64) << 16) | ((ring as u64) << 8) | cpu as u64, Relaxed);
    buf[n + 1].store(pc, Relaxed);
    for (i, &c) in callers[..d].iter().enumerate() {
        buf[n + 2 + i].store(c, Relaxed);
    }
    NSAMP[cpu].fetch_add(1, Relaxed);
    LEN[cpu].store(n + 2 + d, Release);
}

/// x86 timer ISR hook: `rip` of the interrupted context, `user` = it was ring 3.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn sample_tick(rip: u64, user: bool) {
    if ARMED.load(Relaxed) {
        sample_x86(rip, user);
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(never)]
fn sample_x86(rip: u64, user: bool) {
    let Some(cpu) = due() else { return };
    let mut fr = [0u64; MAX_DEPTH];
    let d = if user { 0 } else { walk(rip, cpu, &mut fr) };
    push(cpu, rip, if user { 3 } else { 0 }, &fr[..d]);
}

/// aarch64 timer hook (`timer::on_tick`, inside the IRQ exception, IRQs masked): the interrupted
/// context is `ELR_ELx`/`SPSR_ELx` at the EL this core takes its IRQs at. EL0 reports as ring 3.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn on_tick_arm() {
    if ARMED.load(Relaxed) {
        sample_arm();
    }
}

#[cfg(target_arch = "aarch64")]
#[inline(never)]
fn sample_arm() {
    let Some(cpu) = due() else { return };
    let el: u64;
    unsafe { core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack, preserves_flags)) };
    let (elr, spsr): (u64, u64);
    if (el >> 2) & 3 == 2 {
        unsafe {
            core::arch::asm!("mrs {}, elr_el2", out(reg) elr, options(nomem, nostack, preserves_flags));
            core::arch::asm!("mrs {}, spsr_el2", out(reg) spsr, options(nomem, nostack, preserves_flags));
        }
    } else {
        unsafe {
            core::arch::asm!("mrs {}, elr_el1", out(reg) elr, options(nomem, nostack, preserves_flags));
            core::arch::asm!("mrs {}, spsr_el1", out(reg) spsr, options(nomem, nostack, preserves_flags));
        }
    }
    let user = spsr & 0b1100 == 0;
    let mut fr = [0u64; MAX_DEPTH];
    let d = if user { 0 } else { walk(elr, cpu, &mut fr) };
    push(cpu, elr, if user { 3 } else { 0 }, &fr[..d]);
}

// ---- B340 M1 — the frame-pointer walk ----------------------------------------------------------------

/// `[lo, hi)` the walk may dereference on this CPU right now: from the live stack pointer `sp` up to the
/// top of the stack `sp` is on — the current task's stack, else (x86) the BSP's boot stack or this AP's
/// static boot stack. `None` (no walk) when the stack is unknown or `sp` is not inside it. Everything in
/// `[sp, top)` is mapped: it is the part of the stack this context has already pushed through.
#[inline]
fn stack_window(sp: u64, cpu: usize) -> Option<(u64, u64)> {
    let (lo, hi) = match crate::arch::sched::current_stack_bounds() {
        Some(b) => b,
        None => {
            #[cfg(target_arch = "x86_64")]
            {
                if cpu == 0 {
                    let t = BOOT_TOP.load(Relaxed);
                    if t == 0 {
                        return None;
                    }
                    (t.saturating_sub(4 << 20), t)
                } else {
                    crate::arch::smp::ap_stack_bounds(cpu)?
                }
            }
            #[cfg(target_arch = "aarch64")]
            {
                let _ = cpu;
                return None;
            }
        }
    };
    if sp < lo || sp >= hi { None } else { Some((sp, hi)) }
}

/// A frame pointer the walk may dereference: 8-aligned, its 16-byte record inside `[lo, hi)`, and above
/// the previous one (the chain climbs toward the stack top; a loop or a step down ends the walk).
#[inline]
fn walk_ok(fp: u64, prev: u64, lo: u64, hi: u64) -> bool {
    fp & 7 == 0 && fp >= lo && fp > prev && fp.checked_add(16).is_some_and(|e| e <= hi)
}

/// Read the two words of a frame record `[fp] = caller's fp, [fp + 8] = return address`.
/// SAFETY: the caller proved `walk_ok(fp, ..)` against the current stack window.
#[inline]
unsafe fn frame(fp: u64) -> (u64, u64) {
    unsafe { (core::ptr::read_volatile(fp as *const u64), core::ptr::read_volatile((fp + 8) as *const u64)) }
}

/// Is `ret` a plausible kernel return address (inside the image's executable range)?
#[inline]
fn in_text(ret: u64) -> bool {
    ret >= TEXT_LO.load(Relaxed) && ret < TEXT_HI.load(Relaxed)
}

/// Is `ret` the return slot that marks the interrupted context's boundary? x86: the `x86-interrupt`
/// handler's frame record sits directly on the hardware frame, so its return slot IS the interrupted
/// RIP. aarch64: the IRQ stub `__vec_irq` leaves `x29` untouched and `bl`s the Rust handler, so the
/// handler's record returns into the stub and holds the interrupted `x29`.
#[inline]
fn boundary(ret: u64, pc: u64) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        ret == pc
    }
    #[cfg(target_arch = "aarch64")]
    {
        let _ = pc;
        unsafe extern "C" {
            static __vec_irq: u8;
        }
        let v = &raw const __vec_irq as u64;
        ret > v && ret < v + 0x100
    }
}

/// The walk: climb from this function's own frame to the interrupt boundary (at most 12 records — the
/// sampler and handler frames), then follow the interrupted context's chain, recording at most
/// [`MAX_DEPTH`] return addresses. Every record is `walk_ok` before it is read; every recorded address is
/// `in_text`. Returns the depth recorded (0 = no chain: no frame pointers, unknown stack, or the
/// interrupted code had no frame).
#[inline(never)]
fn walk(pc: u64, cpu: usize, out: &mut [u64; MAX_DEPTH]) -> usize {
    if TEXT_HI.load(Relaxed) == 0 {
        return 0;
    }
    let (mut fp, sp): (u64, u64);
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("mov {}, rbp", out(reg) fp, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("mov {}, x29", out(reg) fp, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {}, sp", out(reg) sp, options(nomem, nostack, preserves_flags));
    }
    let Some((lo, hi)) = stack_window(sp, cpu) else { return 0 };
    let mut prev = 0u64;
    let mut found = false;
    for _ in 0..12 {
        if !walk_ok(fp, prev, lo, hi) {
            break;
        }
        // SAFETY: `walk_ok` just proved the record inside the mapped window.
        let (next, ret) = unsafe { frame(fp) };
        prev = fp;
        fp = next;
        if boundary(ret, pc) {
            found = true;
            break;
        }
    }
    if !found {
        WALK_MISS.fetch_add(1, Relaxed);
        return 0;
    }
    // `fp` is now the interrupted context's frame pointer; `prev` (a sampler-side record) is below it.
    let mut n = 0;
    while n < MAX_DEPTH && walk_ok(fp, prev, lo, hi) {
        // SAFETY: as above.
        let (next, ret) = unsafe { frame(fp) };
        if !in_text(ret) {
            break;
        }
        out[n] = ret;
        n += 1;
        prev = fp;
        fp = next;
    }
    n
}

#[cfg(not(all(target_arch = "aarch64", feature = "baremetal")))]
unsafe extern "C" {
    static __ehdr_start: u8;
}

/// The kernel image's executable range, from its own ELF header and the `PF_X` `PT_LOAD` phdrs through
/// `__ehdr_start` (the image is PIE with `min_vaddr == 0`, so runtime = `ehdr + p_vaddr`). `(0, 0)` when
/// the header does not validate (no walk then).
#[cfg(not(all(target_arch = "aarch64", feature = "baremetal")))]
fn image_text() -> (u64, u64) {
    let ehdr = &raw const __ehdr_start as u64;
    // SAFETY: reads only, through the linker-resolved image base; the magic / class / phentsize checks
    // gate every phdr read (the x86 WXN sweep's `wxn_image_bounds` shape).
    let ok = unsafe {
        let p = ehdr as *const u8;
        core::ptr::read_unaligned(p.cast::<u32>()) == 0x464C_457F
            && *p.add(4) == 2
            && core::ptr::read_unaligned(p.add(54).cast::<u16>()) == 56
    };
    if !ok {
        return (0, 0);
    }
    let (phoff, phnum) = unsafe {
        let p = ehdr as *const u8;
        (core::ptr::read_unaligned(p.add(32).cast::<u64>()), core::ptr::read_unaligned(p.add(56).cast::<u16>()) as usize)
    };
    let (mut lo, mut hi) = (u64::MAX, 0u64);
    for i in 0..phnum.min(64) {
        let (ptype, flags, vaddr, memsz) = unsafe {
            let ph = (ehdr + phoff + (i as u64) * 56) as *const u8;
            (
                core::ptr::read_unaligned(ph.cast::<u32>()),
                core::ptr::read_unaligned(ph.add(4).cast::<u32>()),
                core::ptr::read_unaligned(ph.add(16).cast::<u64>()),
                core::ptr::read_unaligned(ph.add(40).cast::<u64>()),
            )
        };
        if ptype != 1 || flags & 1 == 0 || memsz == 0 {
            continue;
        }
        lo = lo.min(ehdr + vaddr);
        hi = hi.max(ehdr + vaddr + memsz);
    }
    if hi <= lo { (0, 0) } else { (lo, hi) }
}

/// Bare-metal Pi: the image is linked at a fixed address by `pi-baremetal.ld` — `[_start, __bss_start)`
/// covers text (and the read-only data after it, a harmless over-approximation for a return-address test).
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn image_text() -> (u64, u64) {
    unsafe extern "C" {
        static _start: u8;
        static __bss_start: u8;
    }
    (&raw const _start as u64, &raw const __bss_start as u64)
}

/// Ring-0 samples whose walk found no interrupt boundary since the last `prof start`.
pub fn walk_misses() -> u64 {
    WALK_MISS.load(Relaxed)
}

// ---- B340 M2 — the lock-free task name table + program ids -------------------------------------------

/// Name-table slots (keyed `tid % NAME_SLOTS`; a newer tid overwrites an older one in its slot).
pub const NAME_SLOTS: usize = 256;
/// Bytes of a stored name.
pub const NAME_LEN: usize = 16;
/// Interned program names (program id = index + 1; 0 = no program).
pub const PROG_SLOTS: usize = 63;

struct NameSlot {
    tid: AtomicU64,
    w: [AtomicU64; 2],
    prog: AtomicU32,
}

static NAMES: [NameSlot; NAME_SLOTS] =
    [const { NameSlot { tid: AtomicU64::new(0), w: [const { AtomicU64::new(0) }; 2], prog: AtomicU32::new(0) } }; NAME_SLOTS];
static PROGS: [[AtomicU64; 2]; PROG_SLOTS] = [const { [const { AtomicU64::new(0) }; 2] }; PROG_SLOTS];
static NPROG: AtomicU32 = AtomicU32::new(0);
/// Per-CPU pending program id a launch armed, and when (ms), for the next ring-3 spawn on that CPU.
static PENDING: [AtomicU32; CPUS] = [const { AtomicU32::new(0) }; CPUS];
static PENDING_MS: [AtomicU64; CPUS] = [const { AtomicU64::new(0) }; CPUS];
/// A pending program name older than this is stale (the launch failed before it spawned).
const PENDING_TTL_MS: u64 = 2000;

fn pack(s: &str) -> [u64; 2] {
    let mut b = [0u8; NAME_LEN];
    for (d, c) in b.iter_mut().zip(s.bytes()) {
        *d = c;
    }
    [u64::from_le_bytes(b[..8].try_into().unwrap()), u64::from_le_bytes(b[8..].try_into().unwrap())]
}

fn unpack(w: [u64; 2]) -> String {
    let mut b = [0u8; NAME_LEN];
    b[..8].copy_from_slice(&w[0].to_le_bytes());
    b[8..].copy_from_slice(&w[1].to_le_bytes());
    let n = b.iter().position(|&c| c == 0).unwrap_or(NAME_LEN);
    b[..n].iter().map(|&c| if c.is_ascii_graphic() { c as char } else { '_' }).collect()
}

/// Seqlock write: invalidate the slot's tid, write the payload, publish the tid (Release).
fn write_slot(tid: u64, w: [u64; 2], prog: u32) {
    if tid == 0 {
        return;
    }
    let s = &NAMES[tid as usize % NAME_SLOTS];
    s.tid.store(0, Relaxed);
    fence(Release);
    s.w[0].store(w[0], Relaxed);
    s.w[1].store(w[1], Relaxed);
    s.prog.store(prog, Relaxed);
    s.tid.store(tid, Release);
}

/// Seqlock read: `(name words, prog)` if `tid`'s slot holds `tid` before and after the payload read.
fn read_slot(tid: u64) -> Option<([u64; 2], u32)> {
    if tid == 0 {
        return None;
    }
    let s = &NAMES[tid as usize % NAME_SLOTS];
    if s.tid.load(Acquire) != tid {
        return None;
    }
    let w = [s.w[0].load(Relaxed), s.w[1].load(Relaxed)];
    let p = s.prog.load(Relaxed);
    fence(Acquire);
    if s.tid.load(Relaxed) != tid { None } else { Some((w, p)) }
}

/// ISR-safe: the program id `tid` carries (0 = none / unknown).
#[inline]
fn prog_of(tid: u64) -> u8 {
    if tid == 0 {
        return 0;
    }
    let s = &NAMES[tid as usize % NAME_SLOTS];
    if s.tid.load(Acquire) == tid { s.prog.load(Relaxed).min(255) as u8 } else { 0 }
}

/// Spawn hook (every kernel-task spawn path, both arches): `tid` is `name`.
pub fn note_task(tid: u64, name: &str) {
    write_slot(tid, pack(name), 0);
}

/// Ring-3 spawn hook: `tid` takes the program name a launch on this CPU armed (fresh), else `name`.
pub fn note_user_task(tid: u64, name: &str) {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    if cpu < CPUS {
        let p = PENDING[cpu].swap(0, Relaxed);
        if p != 0 && crate::arch::ms().saturating_sub(PENDING_MS[cpu].load(Relaxed)) < PENDING_TTL_MS {
            return write_slot(tid, prog_words(p), p);
        }
    }
    note_task(tid, name);
}

/// Thread spawn hook: a thread of a named program carries that program's name and id.
pub fn note_thread(tid: u64, name: &str) {
    if let Some((w, p)) = crate::arch::sched::current_id().and_then(read_slot) {
        if p != 0 {
            return write_slot(tid, w, p);
        }
    }
    note_task(tid, name);
}

/// Launch hook (`run`, `bg`, a bare name): the next ring-3 spawn on this CPU is the program at `path`.
/// Its name is the path's basename without extension, upper-cased (`/apps/LUMEN.ELF` -> `LUMEN`), the
/// stem of its `target/APPS/<NAME>.syms`.
pub fn launching(path: &str) {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    if cpu >= CPUS {
        return;
    }
    let base = path.rsplit('/').next().unwrap_or(path);
    let stem = base.split('.').next().unwrap_or(base);
    let mut up = [0u8; NAME_LEN];
    let mut n = 0;
    for c in stem.bytes().take(NAME_LEN) {
        up[n] = c.to_ascii_uppercase();
        n += 1;
    }
    let name = core::str::from_utf8(&up[..n]).unwrap_or("");
    let p = intern(name);
    PENDING_MS[cpu].store(crate::arch::ms(), Relaxed);
    PENDING[cpu].store(p, Relaxed);
}

/// The id of program `name`, interned on first sight (0 when empty or the table is full).
fn intern(name: &str) -> u32 {
    if name.is_empty() {
        return 0;
    }
    let w = pack(name);
    let n = (NPROG.load(Acquire) as usize).min(PROG_SLOTS);
    for (i, slot) in PROGS[..n].iter().enumerate() {
        if slot[0].load(Relaxed) == w[0] && slot[1].load(Relaxed) == w[1] {
            return i as u32 + 1;
        }
    }
    let i = NPROG.fetch_add(1, AcqRel) as usize;
    if i >= PROG_SLOTS {
        NPROG.store(PROG_SLOTS as u32, Relaxed);
        return 0;
    }
    PROGS[i][0].store(w[0], Relaxed);
    PROGS[i][1].store(w[1], Release);
    i as u32 + 1
}

fn prog_words(p: u32) -> [u64; 2] {
    match (p as usize).checked_sub(1).filter(|&i| i < PROG_SLOTS) {
        Some(i) => [PROGS[i][0].load(Relaxed), PROGS[i][1].load(Acquire)],
        None => [0, 0],
    }
}

/// Program `p`'s name (empty for 0 / unknown).
pub fn prog_name(p: u32) -> String {
    unpack(prog_words(p))
}

/// `tid`'s name, if the table still holds it.
pub fn task_name(tid: u64) -> Option<String> {
    read_slot(tid).map(|(w, _)| unpack(w))
}

// ---- samples, runs, the shell views --------------------------------------------------------------------

/// One decoded sample. Its callers stay in the ring: [`caller`] reads them.
#[derive(Clone, Copy)]
pub struct Sample {
    pub rip: u64,
    pub tid: u64,
    pub ring: u8,
    pub cpu: u8,
    pub prog: u8,
    pub depth: u8,
    /// Word offset of the sample's header in its CPU's ring.
    pub off: u32,
}

/// Caller `i` (0 = the interrupted function's caller) of sample `s`, or 0.
pub fn caller(s: &Sample, i: usize) -> u64 {
    if i >= s.depth as usize {
        return 0;
    }
    RING.get(s.cpu as usize).and_then(|r| r.get()).and_then(|b| b.get(s.off as usize + 2 + i)).map_or(0, |w| w.load(Relaxed))
}

/// Every published sample, all CPUs (shell context; allocates).
pub fn samples() -> Vec<Sample> {
    let mut v = Vec::new();
    for cpu in 0..CPUS {
        let Some(buf) = RING[cpu].get() else { continue };
        let len = LEN[cpu].load(Acquire).min(buf.len());
        let mut i = 0;
        while i + 2 <= len {
            let h = buf[i].load(Relaxed);
            let depth = ((h >> 16) & 0xFF) as usize;
            if i + 2 + depth > len {
                break;
            }
            v.push(Sample {
                rip: buf[i + 1].load(Relaxed),
                tid: h >> 32,
                prog: (h >> 24) as u8,
                depth: depth as u8,
                ring: (h >> 8) as u8,
                cpu: h as u8,
                off: i as u32,
            });
            i += 2 + depth;
        }
    }
    v
}

/// Samples dropped (ring full), all CPUs.
pub fn dropped() -> u64 {
    DROPPED.iter().map(|d| d.load(Relaxed)).sum()
}

/// Samples published, all CPUs.
pub fn count() -> usize {
    NSAMP.iter().map(|l| l.load(Acquire)).sum()
}

/// The effective sample rate (`tick_hz / div`).
pub fn hz() -> u32 {
    tick_hz() / DIV.load(Relaxed).max(1)
}

/// Disarm, let any in-flight ISR on another core finish, clear every ring and counter, then arm at
/// `want_hz` (clamped to the tick rate). Allocates the rings on first use (shell context only).
pub fn start(want_hz: u32) -> u32 {
    ARMED.store(false, Relaxed);
    settle();
    if TEXT_HI.load(Relaxed) == 0 {
        let (lo, hi) = image_text();
        TEXT_LO.store(lo, Relaxed);
        TEXT_HI.store(hi, Relaxed);
    }
    for cpu in 0..CPUS {
        RING[cpu].call_once(|| {
            let v: Vec<AtomicU64> = (0..RING_WORDS).map(|_| AtomicU64::new(0)).collect();
            &*alloc::boxed::Box::leak(v.into_boxed_slice())
        });
        LEN[cpu].store(0, Relaxed);
        NSAMP[cpu].store(0, Relaxed);
        DROPPED[cpu].store(0, Relaxed);
        COUNTDOWN[cpu].store(1, Relaxed);
    }
    WALK_MISS.store(0, Relaxed);
    let th = tick_hz().max(1);
    let hz = want_hz.clamp(1, th);
    DIV.store((th / hz).max(1), Relaxed);
    on_start();
    T_STOP.store(0, Relaxed);
    T_START.store(crate::arch::now_cycles(), Relaxed);
    ARMED.store(true, Release);
    self::hz()
}

/// Disarm. Samples stay readable until the next `start`.
pub fn stop() {
    if ARMED.swap(false, Relaxed) {
        T_STOP.store(crate::arch::now_cycles(), Relaxed);
    }
    settle();
}

/// Wait ~2 ms (bounded on the free-running counter) so an ISR that read `ARMED` just before a
/// disarm has published or abandoned its sample before the rings are read or cleared.
fn settle() {
    let hz = cycle_hz();
    let wait = if hz == 0 { 4_000_000 } else { hz / 500 };
    let t0 = crate::arch::now_cycles();
    while crate::arch::now_cycles().wrapping_sub(t0) < wait {
        core::hint::spin_loop();
    }
}

/// Wall time of the run so far (or of the finished run), in milliseconds.
pub fn span_ms() -> u64 {
    let t0 = T_START.load(Relaxed);
    if t0 == 0 {
        return 0;
    }
    let t1 = match T_STOP.load(Relaxed) { 0 => crate::arch::now_cycles(), t => t };
    cyc_to_ns(t1.wrapping_sub(t0)) / 1_000_000
}

/// Per-run state the views keep: syscall histograms cleared, compositor baseline taken.
fn on_start() {
    for h in SYS_H.iter().chain(LSYS_H.iter()) {
        for b in h.iter() {
            b.store(0, Relaxed);
        }
    }
    for i in 0..SYS_SLOTS {
        SYS_SUM_NS[i].store(0, Relaxed);
        SYS_MAX_NS[i].store(0, Relaxed);
    }
    for i in 0..LINUX_SLOTS {
        LSYS_SUM_NS[i].store(0, Relaxed);
        LSYS_MAX_NS[i].store(0, Relaxed);
    }
    comp_baseline();
}

/// One line to the console AND the serial wire (`tools/flame` reads serial).
fn out(console: &mut Console, s: &str) {
    console.println(s);
    serial_println!("{}", s);
}

fn summary_line() -> String {
    format!(
        "[prof] summary armed={} hz={} samples={} dropped={} span_ms={} cap_words={} depth_max={} walk_miss={} anchor={:#x}",
        if armed() { "yes" } else { "no" },
        hz(),
        count(),
        dropped(),
        span_ms(),
        RING_WORDS,
        MAX_DEPTH,
        walk_misses(),
        anchor()
    )
}

/// The `prof` shell verb.
pub fn shell_verb(args: &[&str], console: &mut Console) {
    let num = |i: usize| args.get(i).and_then(|s| s.parse::<u32>().ok());
    match args.first().copied() {
        None | Some("status") => out(console, &summary_line()),
        Some("start") => {
            let hz = start(num(1).unwrap_or(tick_hz()));
            out(console, &format!("[prof] start hz={} tick_hz={} cap_words={} cpus={} anchor={:#x}", hz, tick_hz(), RING_WORDS, CPUS, anchor()));
        }
        Some("stop") => {
            stop();
            out(console, &summary_line());
        }
        Some("dump") => dump(console),
        Some("top") => top(console, num(1).unwrap_or(10) as usize),
        Some("sys") => sys(console),
        Some("tasks") => tasks(console),
        _ => console.println("usage: prof [status] | prof start [hz] | prof stop | prof top [n] | prof dump | prof sys | prof tasks"),
    }
}

/// `prof dump` — the program table, the sampled tasks' names, then every sample (with its caller chain)
/// to serial (the console gets the count).
fn dump(console: &mut Console) {
    let v = samples();
    serial_println!("{}", summary_line());
    let np = (NPROG.load(Acquire) as usize).min(PROG_SLOTS);
    for p in 1..=np {
        serial_println!("[prof] prog id={} name={}", p, prog_name(p as u32));
    }
    let mut tids: Vec<u64> = v.iter().map(|s| s.tid).collect();
    tids.sort_unstable();
    tids.dedup();
    for &t in &tids {
        if let Some(n) = task_name(t) {
            serial_println!("[prof] name tid={} name={}", t, n);
        }
    }
    for s in &v {
        if s.depth == 0 {
            serial_println!("[prof] cpu={} task={} ring={} rip={:#x} prog={}", s.cpu, s.tid, s.ring, s.rip, s.prog);
        } else {
            let mut fp = String::new();
            for i in 0..s.depth as usize {
                if i != 0 {
                    fp.push(',');
                }
                fp.push_str(&format!("{:#x}", caller(s, i)));
            }
            serial_println!("[prof] cpu={} task={} ring={} rip={:#x} prog={} fp={}", s.cpu, s.tid, s.ring, s.rip, s.prog, fp);
        }
    }
    serial_println!("[prof] dump end samples={}", v.len());
    console.println(&format!("[prof] dump: {} samples written to serial", v.len()));
}

/// `(ring, bucket)` -> samples, hottest first. A bucket is `rip >> BUCKET_SHIFT`.
pub fn buckets(v: &[Sample]) -> Vec<((u8, u64), usize)> {
    let mut keys: Vec<(u8, u64)> = v.iter().map(|s| (s.ring, s.rip >> BUCKET_SHIFT)).collect();
    keys.sort_unstable();
    let mut out: Vec<((u8, u64), usize)> = Vec::new();
    for k in keys {
        match out.last_mut() {
            Some((lk, n)) if *lk == k => *n += 1,
            _ => out.push((k, 1)),
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// Per-mille as `x.y%`.
fn pct(n: usize, total: usize) -> String {
    let pm = if total == 0 { 0 } else { n * 1000 / total };
    format!("{}.{}%", pm / 10, pm % 10)
}

/// A bucket's start address, and its offset from the anchor (link-address independent: add it to the
/// anchor's `kernel.syms` address to land in the symbol table). Ring-3 buckets are user addresses.
pub fn bucket_name(ring: u8, b: u64) -> String {
    let a = b << BUCKET_SHIFT;
    if ring == 3 {
        return format!("{:#x} [ring3]", a);
    }
    let off = a as i64 - anchor() as i64;
    if off < 0 { format!("{:#x} anchor-{:#x}", a, -off) } else { format!("{:#x} anchor+{:#x}", a, off) }
}

/// `prof top [n]` — the hottest N 256-byte RIP buckets with sample %.
fn top(console: &mut Console, n: usize) {
    let v = samples();
    out(console, &summary_line());
    let total = v.len();
    for (rank, ((ring, b), c)) in buckets(&v).into_iter().take(n.max(1)).enumerate() {
        out(console, &format!("[prof] top rank={} samples={} pct={} ring={} bucket={}", rank + 1, c, pct(c, total), ring, bucket_name(ring, b)));
    }
    comp_rows(console);
}


// ---- M3 — per-syscall latency histograms + per-task CPU share (B340 M4: + the Linux-ABI table) ---------

/// Syscall numbers tracked one-for-one; a number at or past the last slot is pooled into it.
pub const SYS_SLOTS: usize = 64;
/// Linux-ABI syscall numbers tracked one-for-one (x86_64 Linux numbers reach ~335); the last is pooled.
pub const LINUX_SLOTS: usize = 336;
/// log2 buckets of nanoseconds: bucket `b` holds `[2^(b-1), 2^b)` ns (bucket 0 = 0 ns); the last is open.
pub const SYS_BUCKETS: usize = 32;
static SYS_H: [[AtomicU32; SYS_BUCKETS]; SYS_SLOTS] = [const { [const { AtomicU32::new(0) }; SYS_BUCKETS] }; SYS_SLOTS];
static SYS_SUM_NS: [AtomicU64; SYS_SLOTS] = [const { AtomicU64::new(0) }; SYS_SLOTS];
static SYS_MAX_NS: [AtomicU64; SYS_SLOTS] = [const { AtomicU64::new(0) }; SYS_SLOTS];
static LSYS_H: [[AtomicU32; SYS_BUCKETS]; LINUX_SLOTS] = [const { [const { AtomicU32::new(0) }; SYS_BUCKETS] }; LINUX_SLOTS];
static LSYS_SUM_NS: [AtomicU64; LINUX_SLOTS] = [const { AtomicU64::new(0) }; LINUX_SLOTS];
static LSYS_MAX_NS: [AtomicU64; LINUX_SLOTS] = [const { AtomicU64::new(0) }; LINUX_SLOTS];

/// Syscall entry: the start stamp when armed (never 0), else 0. One relaxed load disarmed.
#[inline]
pub fn sys_t0() -> u64 {
    if ARMED.load(Relaxed) { crate::arch::now_cycles() | 1 } else { 0 }
}

#[inline]
fn charge(h: &[[AtomicU32; SYS_BUCKETS]], sum: &[AtomicU64], max: &[AtomicU64], nr: u64, t0: u64) {
    if t0 == 0 {
        return;
    }
    let ns = cyc_to_ns(crate::arch::now_cycles().wrapping_sub(t0));
    let slot = (nr as usize).min(h.len() - 1);
    let b = ((64 - ns.leading_zeros()) as usize).min(SYS_BUCKETS - 1);
    h[slot][b].fetch_add(1, Relaxed);
    sum[slot].fetch_add(ns, Relaxed);
    max[slot].fetch_max(ns, Relaxed);
}

/// Syscall exit: charge `nr`'s histogram with the time since `t0` (a 0 stamp = not armed at entry).
/// A blocking verb (sleep, wait) is charged its whole blocked time: this is latency as the caller
/// saw it, not CPU.
#[inline]
pub fn sys_note(nr: u64, t0: u64) {
    charge(&SYS_H, &SYS_SUM_NS, &SYS_MAX_NS, nr, t0);
}

/// B340 M4: the Linux-ABI dispatcher's exit (folded onto `syscall_dispatch`'s Linux branch) — the same
/// charge into the Linux-numbered table.
#[inline]
pub fn sys_note_linux(nr: u64, t0: u64) {
    charge(&LSYS_H, &LSYS_SUM_NS, &LSYS_MAX_NS, nr, t0);
}

/// Calls charged to Linux number `nr` this run.
pub fn linux_calls(nr: usize) -> u64 {
    LSYS_H.get(nr).map_or(0, |h| h.iter().map(|b| b.load(Relaxed) as u64).sum())
}

/// The upper bound (ns) of bucket `b`.
fn bucket_hi(b: usize) -> u64 {
    if b == 0 { 0 } else { 1u64 << b }
}

/// `prof sys` — one row per syscall number seen in the run (native, then `abi=linux`).
fn sys(console: &mut Console) {
    out(console, &summary_line());
    let rows = sys_rows(console, "", &SYS_H, &SYS_SUM_NS, &SYS_MAX_NS) + sys_rows(console, " abi=linux", &LSYS_H, &LSYS_SUM_NS, &LSYS_MAX_NS);
    if rows == 0 {
        out(console, "[prof] sys: no syscalls in this run (arm with `prof start`)");
    }
}

fn sys_rows(console: &mut Console, abi: &str, hs: &[[AtomicU32; SYS_BUCKETS]], sum: &[AtomicU64], max: &[AtomicU64]) -> usize {
    let mut rows = 0;
    for nr in 0..hs.len() {
        let h: Vec<u32> = hs[nr].iter().map(|b| b.load(Relaxed)).collect();
        let calls: u64 = h.iter().map(|&c| c as u64).sum();
        if calls == 0 {
            continue;
        }
        rows += 1;
        let quant = |q: u64| {
            let want = (calls * q).div_ceil(100).max(1);
            let mut acc = 0u64;
            for (b, &c) in h.iter().enumerate() {
                acc += c as u64;
                if acc >= want {
                    return bucket_hi(b);
                }
            }
            bucket_hi(SYS_BUCKETS - 1)
        };
        let mut hist = String::new();
        for (b, &c) in h.iter().enumerate() {
            if c != 0 {
                if !hist.is_empty() {
                    hist.push(',');
                }
                hist.push_str(&format!("<{}:{}", bucket_hi(b), c));
            }
        }
        out(console, &format!(
            "[prof] sys{} nr={}{} calls={} mean_ns={} p50_ns<={} p99_ns<={} max_ns={} hist={}",
            abi,
            nr,
            if nr == hs.len() - 1 { "+" } else { "" },
            calls,
            sum[nr].load(Relaxed) / calls,
            quant(50),
            quant(99),
            max[nr].load(Relaxed),
            hist
        ));
    }
    rows
}

/// `prof tasks` — per-task CPU share from the samples, with the name table's name (task 0 = no task:
/// idle / scheduler context).
fn tasks(console: &mut Console) {
    let v = samples();
    out(console, &summary_line());
    let total = v.len();
    let mut keys: Vec<(u64, u8)> = v.iter().map(|s| (s.tid, s.ring)).collect();
    keys.sort_unstable();
    // (tid, samples, ring-3 samples)
    let mut rows: Vec<(u64, usize, usize)> = Vec::new();
    for (tid, ring) in keys {
        match rows.last_mut() {
            Some(r) if r.0 == tid => {
                r.1 += 1;
                r.2 += (ring == 3) as usize;
            }
            _ => rows.push((tid, 1, (ring == 3) as usize)),
        }
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (tid, n, u) in rows {
        let name = if tid == 0 { String::from("(idle)") } else { task_name(tid).unwrap_or_else(|| String::from("?")) };
        out(console, &format!("[prof] task tid={} name={} samples={} pct={} ring3={}", tid, name, n, pct(n, total), pct(u, n.max(1))));
    }
}

// ---- B340 M5 — SYS_PROF: a program profiles itself --------------------------------------------------

/// The tid that armed the current run through `SYS_PROF` (0 = the operator's `prof start`, or idle).
static SYS_OWNER: AtomicU64 = AtomicU64::new(0);

/// The calling program's own samples (its tid, or — when it carries one — its program id) as
/// `una_abi::prof::Sample` records, at most `max` (shell or syscall context; allocates).
pub fn own_records(tid: u64, max: usize) -> Vec<una_abi::prof::Sample> {
    let prog = prog_of(tid);
    samples()
        .into_iter()
        .filter(|s| s.tid == tid || (prog != 0 && s.prog == prog))
        .take(max.min(una_abi::prof::READ_MAX))
        .map(|s| una_abi::prof::Sample { rip: s.rip, caller: caller(&s, 0), tid: s.tid as u32, ring: s.ring, cpu: s.cpu, depth: s.depth, prog: s.prog })
        .collect()
}

/// `SYS_PROF(op, buf, len)` — the body both arches' dispatchers call; `copy(ptr, bytes)` is the arch's
/// validated `copy_to_user` (false = fault). Returns per `una_abi::prof`: START -> the armed rate in Hz,
/// STOP -> 0 (stopped) or 1 (the run is not the caller's: left armed), READ -> records copied,
/// STATUS -> the caller's sample count; `-22` for an unknown op, `-14` for a bad buffer.
pub fn sys_prof(op: u64, buf: u64, len: u64, copy: impl FnOnce(u64, &[u8]) -> bool) -> i64 {
    use una_abi::prof as p;
    let me = crate::arch::sched::current_id().unwrap_or(0);
    match op {
        p::OP_START => {
            if !armed() {
                let hz = start(tick_hz());
                SYS_OWNER.store(me, Relaxed);
                hz as i64
            } else {
                hz() as i64
            }
        }
        p::OP_STOP => {
            if armed() && SYS_OWNER.load(Relaxed) == me && me != 0 {
                stop();
                SYS_OWNER.store(0, Relaxed);
                0
            } else {
                1
            }
        }
        p::OP_READ => {
            let max = (len as usize) / p::SAMPLE_BYTES;
            let recs = own_records(me, max);
            if recs.is_empty() {
                return 0;
            }
            let mut bytes: Vec<u8> = Vec::with_capacity(recs.len() * p::SAMPLE_BYTES);
            for r in &recs {
                bytes.extend_from_slice(&r.to_bytes());
            }
            if copy(buf, &bytes) { recs.len() as i64 } else { -14 }
        }
        p::OP_STATUS => own_records(me, usize::MAX).len() as i64,
        _ => -22,
    }
}


// ---- M4 — the compositor split + `tests prof` --------------------------------------------------------

#[cfg(feature = "witness")]
static COMP0: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

/// Take the `[comp2]` counters' baseline at `prof start`.
fn comp_baseline() {
    #[cfg(feature = "witness")]
    for (b, v) in COMP0.iter().zip(crate::video::wm::prof_comp2()) {
        b.store(v, Relaxed);
    }
}

/// The compositor's present / compose / blit per-pass means over the profiling window, as three
/// `prof top` rows. `src=window` is a true delta; `src=since-rollup` means a `[comp2]` rollup drained
/// the counters mid-run and the means cover the span since that drain.
fn comp_rows(console: &mut Console) {
    #[cfg(feature = "witness")]
    {
        let cur = crate::video::wm::prof_comp2();
        let base: Vec<u64> = COMP0.iter().map(|b| b.load(Relaxed)).collect();
        let window = cur.iter().zip(base.iter()).all(|(c, b)| c >= b);
        let d: Vec<u64> = if window { cur.iter().zip(base.iter()).map(|(c, b)| c - b).collect() } else { cur.to_vec() };
        let passes = d[0];
        let us = |cyc: u64| if passes == 0 { 0 } else { cyc_to_ns(cyc / passes) / 1000 };
        let src = if window { "window" } else { "since-rollup" };
        for (name, cyc) in [("present", d[4]), ("compose", d[3]), ("blit", d[1].saturating_sub(d[2]))] {
            out(console, &format!("[prof] top row=wc:{} mean_us={} passes={} src={}", name, us(cyc), passes, src));
        }
    }
    #[cfg(not(feature = "witness"))]
    out(console, "[prof] top row=wc: (the [comp2] split needs a witness image)");
}

/// The 1 s window `tests prof` profiles.
pub const TEST_MS: u64 = 1000;

/// The synthetic load `tests prof` runs on this CPU: an FNV-1a churn over a stack buffer until the
/// deadline. Never inlined, so its bucket is its own.
#[inline(never)]
pub fn synthetic_load(deadline: u64) -> u64 {
    let mut buf = [0u8; 1024];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0usize;
    while crate::arch::now_cycles() < deadline {
        for b in buf.iter_mut() {
            h = (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3);
            *b = (h >> 7) as u8 ^ i as u8;
        }
        i = i.wrapping_add(1);
        core::hint::black_box(&buf);
    }
    h
}

/// `tests prof` registration, once (never at boot, R80).
pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, core::sync::atomic::Ordering::AcqRel) {
        crate::tests::register("prof", selftest);
        crate::tests::register("prof2", selftest2);
    }
}

/// `tests prof`: arm at the tick rate, run the synthetic load for [`TEST_MS`], stop, and assert
/// `samples > 0` and `dropped == 0`. A run already in progress is restarted (its samples are lost).
pub fn selftest() {
    let hz = start(tick_hz());
    let chz = cycle_hz();
    let span = if chz == 0 { 2_500_000_000 } else { chz / 1000 * TEST_MS };
    let h = synthetic_load(crate::arch::now_cycles().wrapping_add(span));
    core::hint::black_box(h);
    stop();
    let v = samples();
    let (n, d) = (v.len(), dropped());
    let load = synthetic_load as *const () as usize as u64;
    let me = crate::arch::percpu::this_cpu().cpu_index as u8;
    let in_load = v.iter().filter(|s| s.cpu == me && s.ring == 0 && s.rip >= load && s.rip < load + 512).count();
    let mine = v.iter().filter(|s| s.cpu == me).count();
    let (top, top_pct) = match buckets(&v).first() {
        Some(((ring, b), c)) => (format!("{:#x}{}", b << BUCKET_SHIFT, if *ring == 3 { "/ring3" } else { "" }), pct(*c, n)),
        None => (String::from("none"), pct(0, 0)),
    };
    serial_println!(
        "[prof] test top_pct={} cpu={} cpu_samples={} in_load={} load={:#x} anchor={:#x} span_ms={} cpus_sampled={}",
        top_pct,
        me,
        mine,
        in_load,
        load,
        anchor(),
        span_ms(),
        (0..CPUS).filter(|&c| v.iter().any(|s| s.cpu as usize == c)).count()
    );
    let pass = n > 0 && d == 0;
    serial_println!(":: PROFILE: hz={} samples={} dropped={} top={} -> {} ::", hz, n, d, top, if pass { "PASS" } else { "FAIL" });
    crate::tests::tally(pass);
}


// ---- B340 — `tests prof2` ------------------------------------------------------------------------------

/// The load task's run, ms.
pub const P2_MS: u64 = 600;
static P2_TID: AtomicU64 = AtomicU64::new(0);
static P2_DONE: AtomicBool = AtomicBool::new(false);
static P2_DEADLINE: AtomicU64 = AtomicU64::new(0);

/// The `prof2-load` task: the synthetic load three frames deep (task entry -> `prof2_a` -> `prof2_b` ->
/// `synthetic_load`), each step `#[inline(never)]` and used after the call so no tail call folds a frame.
fn prof2_task(_: usize) {
    P2_TID.store(crate::arch::sched::current_id().unwrap_or(0), Relaxed);
    core::hint::black_box(prof2_a(P2_DEADLINE.load(Relaxed)));
    P2_DONE.store(true, Release);
}

#[inline(never)]
fn prof2_a(deadline: u64) -> u64 {
    core::hint::black_box(prof2_b(deadline)) ^ 1
}

#[inline(never)]
fn prof2_b(deadline: u64) -> u64 {
    core::hint::black_box(synthetic_load(deadline)) ^ 2
}

/// `tests prof2`: arm, run `prof2-load` on another core for [`P2_MS`] (the shell waits, bounded), stop,
/// and assert stacks were walked, names resolve, the Linux table takes a note and `SYS_PROF`'s read core
/// returns only the load task's own samples.
pub fn selftest2() {
    let hz = start(tick_hz());
    let chz = cycle_hz();
    let ms_cyc = if chz == 0 { 2_500_000 } else { chz / 1000 };
    P2_DONE.store(false, Relaxed);
    P2_TID.store(0, Relaxed);
    P2_DEADLINE.store(crate::arch::now_cycles().wrapping_add(ms_cyc * P2_MS), Release);
    let target = crate::arch::sched::other_dispatching_cpu();
    #[cfg(target_arch = "x86_64")]
    crate::arch::sched::spawn("prof2-load", prof2_task, 0, target, crate::arch::sched::PRIO_NORMAL);
    #[cfg(target_arch = "aarch64")]
    let _ = crate::arch::sched::spawn("prof2-load", prof2_task, 0, target);
    #[cfg(all(feature = "linuxabi", target_arch = "x86_64"))]
    {
        let t = sys_t0();
        sys_note_linux(39, t); // getpid's number: the table the dispatcher's fold charges
    }
    let limit = crate::arch::now_cycles().wrapping_add(ms_cyc * (P2_MS + 2000));
    while !P2_DONE.load(Acquire) && crate::arch::now_cycles() < limit {
        core::hint::spin_loop();
    }
    let ran = P2_DONE.load(Acquire);
    stop();
    let v = samples();
    let (n, d) = (v.len(), dropped());
    let tid = P2_TID.load(Relaxed);
    let stacks = v.iter().filter(|s| s.depth > 0).count();
    let depth_max = v.iter().map(|s| s.depth).max().unwrap_or(0);
    let mut tids: Vec<u64> = v.iter().map(|s| s.tid).filter(|&t| t != 0).collect();
    tids.sort_unstable();
    tids.dedup();
    let names = tids.iter().filter(|&&t| task_name(t).is_some()).count();
    let load_named = tid != 0 && task_name(tid).as_deref() == Some("prof2-load");
    let load = synthetic_load as *const () as usize as u64;
    let b = prof2_b as *const () as usize as u64;
    let mine: Vec<&Sample> = v.iter().filter(|s| tid != 0 && s.tid == tid).collect();
    let in_load = mine.iter().filter(|s| s.rip >= load && s.rip < load + 512).count();
    let chain = mine.iter().filter(|s| s.rip >= load && s.rip < load + 512 && caller(s, 0) > b && caller(s, 0) < b + 256).count();
    let r3: Vec<&Sample> = v.iter().filter(|s| s.ring == 3).collect();
    let r3_syms = if r3.is_empty() { "skip" } else if r3.iter().any(|s| s.prog != 0) { "ok" } else { "noprog" };
    #[cfg(all(feature = "linuxabi", target_arch = "x86_64"))]
    let linux_hook = if linux_calls(39) >= 1 { "1" } else { "0" };
    #[cfg(not(all(feature = "linuxabi", target_arch = "x86_64")))]
    let linux_hook = "skip";
    let recs = own_records(tid, una_abi::prof::READ_MAX);
    let sys_ok = tid != 0
        && !recs.is_empty()
        && recs.iter().all(|r| r.tid as u64 == tid && una_abi::prof::Sample::from_bytes(&r.to_bytes()) == Some(*r));
    serial_println!(
        "[prof2] detail ran={} load_tid={} load_named={} load_samples={} in_load={} chain={} recs={} walk_miss={} text={:#x}..{:#x} hz={} samples={} dropped={} span_ms={} r3_samples={}",
        ran as u8,
        tid,
        load_named as u8,
        mine.len(),
        in_load,
        chain,
        recs.len(),
        walk_misses(),
        TEXT_LO.load(Relaxed),
        TEXT_HI.load(Relaxed),
        hz,
        n,
        d,
        span_ms(),
        r3.len()
    );
    let pass = n > 0 && d == 0 && ran && stacks > 0 && names > 0 && load_named && sys_ok && linux_hook != "0";
    serial_println!(
        ":: PROFILE2: stacks={} depth_max={} names={} r3_syms={} linux_hook={} sys_prof={} -> {} ::",
        stacks,
        depth_max,
        names,
        r3_syms,
        linux_hook,
        sys_ok as u8,
        if pass { "PASS" } else { "FAIL" }
    );
    crate::tests::tally(pass);
}
