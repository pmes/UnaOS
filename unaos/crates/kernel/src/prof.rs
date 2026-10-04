// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! CHARTER: Kernel — kernel-by-ruling (R83: built in UnaOS; rmbp-ledger B331 PROFILE)
//!
//! PROFILE — the sampling profiler. Peter (2026-10-04): "do we have some kind of profiling so we can
//! see where things are congesting?" The kernel had stage timings and rollups, each answering a
//! question asked in advance at one site; nothing said WHERE the machine spends its time. This does:
//! the timer interrupt both arches already take records, when armed, what it interrupted.
//!
//! * **Sampler (M1).** [`on_tick_x86`] (folded onto the x86 timer ISR's `note_tick()` line) and
//!   [`on_tick_arm`] (folded onto aarch64 `timer::on_tick`'s tail) record `(rip, task id, ring, cpu)`
//!   into THIS CPU's ring of [`CAP`] samples every `div`-th tick. One writer per ring (the owning CPU,
//!   interrupts masked), published through `LEN` with Release; a full ring counts `dropped` and never
//!   overwrites. The ISR never allocates: the rings are heap-allocated by the FIRST `prof start` and
//!   kept for every later run. Disarmed cost: one relaxed load per tick.
//! * **Symbols (M2).** The kernel has no symbol table at runtime and embeds none. arroyo writes
//!   `target/kernel.syms` (`llvm-nm -n -C --defined-only`) beside the ELF it links; the kernel is PIE,
//!   so every dump carries `anchor=` — the runtime address of [`unaos_prof_anchor`] — and
//!   `tools/flame` derives the slide from that symbol's syms line. `prof top` prints 256-byte RIP
//!   buckets; `prof dump` prints every sample to serial as `[prof] cpu= task= ring= rip=`.
//! * **Views (M3).** Per-syscall log2 latency histograms ([`sys_t0`]/[`sys_note`], armed with the
//!   sampler) behind `prof sys`; per-task CPU share from the samples behind `prof tasks`.
//! * **Compositor + fixture (M4).** `prof top` closes with the `[comp2]` present/compose/blit split
//!   (per-pass means over the profiling window, `witness` images); `tests prof` is the witness.
//!
//! Off by default; nothing prints at boot (R80). Design: `docs/dev/evidence/rmbp-1005/PROFILE.md`.
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::{Acquire, Relaxed, Release}};

use crate::console::Console;

/// Samples per CPU ring.
pub const CAP: usize = 4096;
/// CPU slots (the larger of the two arches' `MAX_CPUS`/`NUM_CPUS`).
pub const CPUS: usize = 8;
/// RIP bucket granularity for `prof top` (log2 bytes): 256-byte buckets.
pub const BUCKET_SHIFT: u32 = 8;

static ARMED: AtomicBool = AtomicBool::new(false);
static DIV: AtomicU32 = AtomicU32::new(1);
static T_START: AtomicU64 = AtomicU64::new(0);
static T_STOP: AtomicU64 = AtomicU64::new(0);
static COUNTDOWN: [AtomicU32; CPUS] = [const { AtomicU32::new(1) }; CPUS];
static LEN: [AtomicUsize; CPUS] = [const { AtomicUsize::new(0) }; CPUS];
static DROPPED: [AtomicU64; CPUS] = [const { AtomicU64::new(0) }; CPUS];
/// Per-CPU sample ring: `2 * CAP` words, `[rip, (tid << 16) | (ring << 8) | cpu]` per sample.
static RING: [spin::Once<&'static [AtomicU64]>; CPUS] = [const { spin::Once::new() }; CPUS];

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

/// The one recording path both arches' ISR hooks share. Interrupts are masked here.
#[inline]
fn record(rip: u64, ring: u8) {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    if cpu >= CPUS {
        return;
    }
    let c = COUNTDOWN[cpu].load(Relaxed);
    if c > 1 {
        COUNTDOWN[cpu].store(c - 1, Relaxed);
        return;
    }
    COUNTDOWN[cpu].store(DIV.load(Relaxed).max(1), Relaxed);
    let Some(buf) = RING[cpu].get() else { return };
    let n = LEN[cpu].load(Relaxed);
    if n >= CAP || 2 * n + 1 >= buf.len() {
        DROPPED[cpu].fetch_add(1, Relaxed);
        return;
    }
    let tid = crate::arch::sched::current_task_id(cpu).unwrap_or(0) & 0xFFFF_FFFF_FFFF;
    buf[2 * n].store(rip, Relaxed);
    buf[2 * n + 1].store((tid << 16) | ((ring as u64) << 8) | cpu as u64, Relaxed);
    LEN[cpu].store(n + 1, Release);
}

/// x86 timer ISR hook: `rip` of the interrupted context, `user` = it was ring 3.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn on_tick_x86(rip: u64, user: bool) {
    if ARMED.load(Relaxed) {
        record(rip, if user { 3 } else { 0 });
    }
}

/// aarch64 timer hook (`timer::on_tick`, inside the IRQ exception, IRQs masked): the interrupted
/// context is `ELR_ELx`/`SPSR_ELx` at the EL this core takes its IRQs at. EL0 reports as ring 3.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn on_tick_arm() {
    if !ARMED.load(Relaxed) {
        return;
    }
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
    record(elr, if spsr & 0b1100 == 0 { 3 } else { 0 });
}

/// One decoded sample.
#[derive(Clone, Copy)]
pub struct Sample {
    pub rip: u64,
    pub tid: u64,
    pub ring: u8,
    pub cpu: u8,
}

/// Every published sample, all CPUs (shell context; allocates).
pub fn samples() -> Vec<Sample> {
    let mut v = Vec::new();
    for cpu in 0..CPUS {
        let Some(buf) = RING[cpu].get() else { continue };
        let n = LEN[cpu].load(Acquire).min(CAP);
        for i in 0..n {
            let rip = buf[2 * i].load(Relaxed);
            let w = buf[2 * i + 1].load(Relaxed);
            v.push(Sample { rip, tid: w >> 16, ring: (w >> 8) as u8, cpu: w as u8 });
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
    LEN.iter().map(|l| l.load(Acquire).min(CAP)).sum()
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
    for cpu in 0..CPUS {
        RING[cpu].call_once(|| {
            let v: Vec<AtomicU64> = (0..2 * CAP).map(|_| AtomicU64::new(0)).collect();
            &*alloc::boxed::Box::leak(v.into_boxed_slice())
        });
        LEN[cpu].store(0, Relaxed);
        DROPPED[cpu].store(0, Relaxed);
        COUNTDOWN[cpu].store(1, Relaxed);
    }
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
    for h in SYS_H.iter() {
        for b in h.iter() {
            b.store(0, Relaxed);
        }
    }
    for i in 0..SYS_SLOTS {
        SYS_SUM_NS[i].store(0, Relaxed);
        SYS_MAX_NS[i].store(0, Relaxed);
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
        "[prof] summary armed={} hz={} samples={} dropped={} span_ms={} cap={} anchor={:#x}",
        if armed() { "yes" } else { "no" },
        hz(),
        count(),
        dropped(),
        span_ms(),
        CAP,
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
            out(console, &format!("[prof] start hz={} tick_hz={} cap={} cpus={} anchor={:#x}", hz, tick_hz(), CAP, CPUS, anchor()));
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

/// `prof dump` — every sample to serial (the console gets the count).
fn dump(console: &mut Console) {
    let v = samples();
    serial_println!("{}", summary_line());
    for s in &v {
        serial_println!("[prof] cpu={} task={} ring={} rip={:#x}", s.cpu, s.tid, s.ring, s.rip);
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

/// M4 slot: the compositor split rows (filled by M4).
fn comp_rows(_console: &mut Console) {}

// ---- M3 — per-syscall latency histograms + per-task CPU share ----------------------------------------

/// Syscall numbers tracked one-for-one; a number at or past the last slot is pooled into it.
pub const SYS_SLOTS: usize = 64;
/// log2 buckets of nanoseconds: bucket `b` holds `[2^(b-1), 2^b)` ns (bucket 0 = 0 ns); the last is open.
pub const SYS_BUCKETS: usize = 32;
static SYS_H: [[AtomicU32; SYS_BUCKETS]; SYS_SLOTS] = [const { [const { AtomicU32::new(0) }; SYS_BUCKETS] }; SYS_SLOTS];
static SYS_SUM_NS: [AtomicU64; SYS_SLOTS] = [const { AtomicU64::new(0) }; SYS_SLOTS];
static SYS_MAX_NS: [AtomicU64; SYS_SLOTS] = [const { AtomicU64::new(0) }; SYS_SLOTS];

/// Syscall entry: the start stamp when armed (never 0), else 0. One relaxed load disarmed.
#[inline]
pub fn sys_t0() -> u64 {
    if ARMED.load(Relaxed) { crate::arch::now_cycles() | 1 } else { 0 }
}

/// Syscall exit: charge `nr`'s histogram with the time since `t0` (a 0 stamp = not armed at entry).
/// A blocking verb (sleep, wait) is charged its whole blocked time: this is latency as the caller
/// saw it, not CPU.
#[inline]
pub fn sys_note(nr: u64, t0: u64) {
    if t0 == 0 {
        return;
    }
    let ns = cyc_to_ns(crate::arch::now_cycles().wrapping_sub(t0));
    let slot = (nr as usize).min(SYS_SLOTS - 1);
    let b = ((64 - ns.leading_zeros()) as usize).min(SYS_BUCKETS - 1);
    SYS_H[slot][b].fetch_add(1, Relaxed);
    SYS_SUM_NS[slot].fetch_add(ns, Relaxed);
    SYS_MAX_NS[slot].fetch_max(ns, Relaxed);
}

/// The upper bound (ns) of bucket `b`.
fn bucket_hi(b: usize) -> u64 {
    if b == 0 { 0 } else { 1u64 << b }
}

/// `prof sys` — one row per syscall number seen in the run.
fn sys(console: &mut Console) {
    out(console, &summary_line());
    let mut rows = 0;
    for nr in 0..SYS_SLOTS {
        let h: Vec<u32> = SYS_H[nr].iter().map(|b| b.load(Relaxed)).collect();
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
            "[prof] sys nr={}{} calls={} mean_ns={} p50_ns<={} p99_ns<={} max_ns={} hist={}",
            nr,
            if nr == SYS_SLOTS - 1 { "+" } else { "" },
            calls,
            SYS_SUM_NS[nr].load(Relaxed) / calls,
            quant(50),
            quant(99),
            SYS_MAX_NS[nr].load(Relaxed),
            hist
        ));
    }
    if rows == 0 {
        out(console, "[prof] sys: no syscalls in this run (arm with `prof start`)");
    }
}

/// `prof tasks` — per-task CPU share from the samples (task 0 = no task: idle / scheduler context).
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
        out(console, &format!(
            "[prof] task tid={}{} samples={} pct={} ring3={}",
            tid,
            if tid == 0 { " (idle)" } else { "" },
            n,
            pct(n, total),
            pct(u, n.max(1))
        ));
    }
}

/// M4 slot: the compositor baseline (filled by M4).
fn comp_baseline() {}
