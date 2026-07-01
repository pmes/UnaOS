// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// aarch64 preemptive kernel-thread scheduler — the ARM counterpart of arch/x86_64/sched.rs.
//
// It mirrors the proven x86 design (per-CPU run queues, a stack-swapping `switch_context`, an
// initial frame that lands a fresh task in a trampoline) but is written fresh for aarch64 rather
// than sharing the mature 1700-line x86 module — the only genuinely arch-specific pieces are the
// context switch, the per-CPU access (TPIDR_EL2, via `percpu`), and the interrupt-state save
// (DAIF instead of RFLAGS); the rest is small enough to keep parallel and leaves the x86 code
// untouched.
//
// M3a (this file's first cut) is the COOPERATIVE core: a task runs until it calls `yield_now`
// (round-robin) or returns (`exit`). That's fully exercisable in QEMU raspi4b — it needs no
// interrupt delivery, which QEMU's GIC model withholds on the `pi` build (same reason the timer
// poll-spins there). M3b adds the APs entering their own scheduler loops and timer-driven
// PREEMPTION, which — like the SGI IPIs — only the real Pi 4 can deliver.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

use super::percpu::{self, NUM_CPUS};

/// Per-thread kernel stack. 16 KiB matches the x86 scheduler.
const TASK_STACK_SIZE: usize = 16 * 1024;

// Task lifecycle. A `u8` behind an atomic: the running task writes it (yield/exit) and the
// scheduler reads it after the switch-back to decide requeue-vs-free.
const STATE_READY: u8 = 0;
const STATE_RUNNING: u8 = 1;
const STATE_FINISHED: u8 = 2;

/// Initial DAIF planted in a fresh task's frame: IRQ masked (I bit). The task starts masked so the
/// switch-in is atomic; `task_trampoline` unmasks before running the body (so on metal the timer
/// can preempt it). Matches x86's INITIAL_RFLAGS-with-IF=0.
const INITIAL_DAIF: u64 = 1 << 7;

/// Monotonic task-id source.
static NEXT_TID: AtomicU64 = AtomicU64::new(1);

/// A kernel thread. Owned as `Box<Task>`: it lives in exactly one place — a run queue, or "running"
/// (the Box leaked to a raw pointer in `SchedCpu::current`).
pub struct Task {
    #[allow(dead_code)] // read by the future `sched`/`ps` command + join handles
    id: u64,
    #[allow(dead_code)]
    name: &'static str,
    /// `STATE_*`, written by the owning CPU only.
    state: AtomicU8,
    /// Saved stack pointer — the whole callee-saved context `switch_context` built lives on the stack.
    ctx_sp: u64,
    /// Owns the stack memory; freed when the Box is dropped on the Finished path.
    #[allow(dead_code)]
    stack: Box<[u8]>,
    entry: fn(usize),
    arg: usize,
}

/// One CPU's scheduler bookkeeping (interior-mutable atomics, so the array is a plain static).
struct SchedCpu {
    /// Saved SP of this CPU's scheduler/idle context — written by the first `switch_context` INTO a
    /// task (the save side), read to switch back. Write-before-read, so the initial 0 is never loaded.
    scheduler_sp: AtomicU64,
    /// Raw `*mut Task` currently running here, or 0. Owned by the scheduler loop.
    current: AtomicU64,
}

impl SchedCpu {
    const fn new() -> Self {
        SchedCpu { scheduler_sp: AtomicU64::new(0), current: AtomicU64::new(0) }
    }
}

static SCHED: [SchedCpu; NUM_CPUS] = [const { SchedCpu::new() }; NUM_CPUS];

/// Per-CPU ready queues. `VecDeque::new` is const, so no lazy_static; a `push` may allocate, but
/// only at `spawn` (never under the switch), so the brief lock is realloc-free in the hot path.
static RUN_QUEUES: [Mutex<VecDeque<Box<Task>>>; NUM_CPUS] =
    [const { Mutex::new(VecDeque::new()) }; NUM_CPUS];

// --- Interrupt-state helpers (the scheduler's critical sections run with IRQ masked). ---
#[inline]
fn mask_irq() {
    unsafe { core::arch::asm!("msr daifset, #2", options(nomem, nostack, preserves_flags)) };
}
#[inline]
fn unmask_irq() {
    unsafe { core::arch::asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags)) };
}

// `switch_context(old_sp: *mut u64, new_sp: u64)` — AAPCS64: x0 = old_sp, x1 = new_sp. Saves DAIF +
// the callee-saved registers (x19-x30) of the current context onto the current stack, stores the
// resulting SP through `old_sp`, loads `new_sp`, restores that context's registers + DAIF, and
// `ret`s into it (x30 = the restored return address). Caller-saved registers need no saving — this
// is a normal C-ABI call, so the compiler already treats them as clobbered. 112-byte frame (12
// GPRs + DAIF + pad), 16-aligned. Frame from the saved SP upward:
//   [+0] x19 [+8] x20 [+16] x21 [+24] x22 [+32] x23 [+40] x24 [+48] x25 [+56] x26
//   [+64] x27 [+72] x28 [+80] x29(fp) [+88] x30(lr) [+96] DAIF [+104] pad
core::arch::global_asm!(
    "
    .globl switch_context
    switch_context:
        mrs   x9, daif
        sub   sp, sp, #112
        stp   x19, x20, [sp, #0]
        stp   x21, x22, [sp, #16]
        stp   x23, x24, [sp, #32]
        stp   x25, x26, [sp, #48]
        stp   x27, x28, [sp, #64]
        stp   x29, x30, [sp, #80]
        str   x9, [sp, #96]
        mov   x9, sp
        str   x9, [x0]
        mov   sp, x1
        ldr   x9, [sp, #96]
        msr   daif, x9
        ldp   x19, x20, [sp, #0]
        ldp   x21, x22, [sp, #16]
        ldp   x23, x24, [sp, #32]
        ldp   x25, x26, [sp, #48]
        ldp   x27, x28, [sp, #64]
        ldp   x29, x30, [sp, #80]
        add   sp, sp, #112
        ret
    "
);

unsafe extern "C" {
    fn switch_context(old_sp: *mut u64, new_sp: u64);
}

/// First code every kernel thread runs, reached when `switch_context` `ret`s into a freshly-built
/// frame. Unmasks IRQ (the fresh frame started masked), runs the task body, then exits.
extern "C" fn task_trampoline() -> ! {
    unmask_irq();
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *const Task;
    debug_assert!(!raw.is_null(), "task_trampoline: current is null");
    let (entry, arg) = unsafe { ((*raw).entry, (*raw).arg) };
    entry(arg);
    exit();
}

/// Build a fresh task's initial stack frame so the first `switch_context` into it lands in
/// `task_trampoline`. Returns the value to store in `ctx_sp`. The 112-byte frame sits below a
/// 16-aligned top; x30's slot holds the trampoline, DAIF's slot holds IRQ-masked, the rest zero.
fn build_initial_frame(stack: &mut [u8]) -> u64 {
    let base = stack.as_mut_ptr() as usize;
    let top = (base + stack.len()) & !0xF;
    let sp = top - 112;
    unsafe {
        let p = sp as *mut u64;
        for i in 0..14 {
            p.add(i).write(0); // x19..x28, x29 and pad
        }
        p.add(11).write(task_trampoline as usize as u64); // x30 (lr) slot -> ret lands in trampoline
        p.add(12).write(INITIAL_DAIF); // DAIF slot
    }
    sp as u64
}

/// Create a ready kernel thread on `cpu`'s run queue. Fire-and-forget: it runs `entry(arg)` and is
/// freed when `entry` returns. Returns the task id.
pub fn spawn(name: &'static str, entry: fn(usize), arg: usize, cpu: usize) -> u64 {
    assert!(cpu < NUM_CPUS, "spawn: cpu out of range");
    let mut stack: Box<[u8]> = alloc::vec![0u8; TASK_STACK_SIZE].into_boxed_slice();
    let ctx_sp = build_initial_frame(&mut stack);
    let id = NEXT_TID.fetch_add(1, Ordering::Relaxed);
    let task = Box::new(Task {
        id,
        name,
        state: AtomicU8::new(STATE_READY),
        ctx_sp,
        stack,
        entry,
        arg,
    });
    RUN_QUEUES[cpu].lock().push_back(task);
    id
}

/// Cooperatively give up the CPU: mark this task ready and switch back to the scheduler, which
/// requeues us and runs the next task. We resume here (IRQ masked, carried by the switch) when
/// re-dispatched. No-op if called outside a scheduled task.
pub fn yield_now() {
    let cpu = percpu::this_cpu().cpu_index as usize;
    mask_irq();
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    if !raw.is_null() {
        unsafe {
            (*raw).state.store(STATE_READY, Ordering::Release);
            switch_context(
                &raw mut (*raw).ctx_sp,
                SCHED[cpu].scheduler_sp.load(Ordering::Acquire),
            );
        }
    }
    unmask_irq();
}

/// Terminate the current task: mark it finished and switch to the scheduler for good (which frees
/// its stack). Never returns; called automatically when a task's entry returns.
pub fn exit() -> ! {
    let cpu = percpu::this_cpu().cpu_index as usize;
    mask_irq();
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    assert!(!raw.is_null(), "exit: no current task");
    unsafe {
        (*raw).state.store(STATE_FINISHED, Ordering::Release);
        // `old_sp` is a throwaway on the dying stack — the scheduler never reads it back and never
        // switches into a finished task.
        let mut discard: u64 = 0;
        switch_context(&raw mut discard, SCHED[cpu].scheduler_sp.load(Ordering::Acquire));
    }
    unreachable!("scheduler resumed a finished task")
}

/// Run `cpu`'s run queue to completion, cooperatively: pop the front task, switch into it, and when
/// it switches back (yield / exit) requeue it (READY) or free it (FINISHED); return when the queue
/// drains. Runs on the caller's stack, which becomes this CPU's scheduler context. IRQ is masked
/// across pop+switch so nothing re-enters the scheduler on its own stack. This is the M3a demo
/// driver on the BSP; M3b turns it into the APs' forever-loop with a WFI idle instead of returning.
pub fn run_until_empty(cpu: usize) {
    loop {
        mask_irq();
        let next = RUN_QUEUES[cpu].lock().pop_front();
        let Some(task) = next else {
            unmask_irq();
            return;
        };
        task.state.store(STATE_RUNNING, Ordering::Release);
        let raw = Box::into_raw(task);
        let entry_sp = unsafe { (*raw).ctx_sp };
        // Publish `current` (Release) strictly before switching in — the trampoline reads it Acquire.
        SCHED[cpu].current.store(raw as u64, Ordering::Release);
        unsafe {
            switch_context(SCHED[cpu].scheduler_sp.as_ptr(), entry_sp);
        }
        // The task switched back (IRQ masked, carried). Reclaim the Box and requeue or free it.
        SCHED[cpu].current.store(0, Ordering::Release);
        let task = unsafe { Box::from_raw(raw) };
        match task.state.load(Ordering::Acquire) {
            STATE_READY => RUN_QUEUES[cpu].lock().push_back(task),
            _ => drop(task), // FINISHED (or unexpected) — free the stack
        }
    }
}

/// M3a smoke test: spawn a few cooperative kernel threads on the boot core and run them to
/// completion. Proves `switch_context` + the run queue + `spawn`/`yield_now`/`exit` — round-robin,
/// no interrupts required (so it runs identically in QEMU raspi4b and on metal). Called from
/// `kernel_main` on the BSP; returns once every demo task has exited.
pub fn demo_cooperative() {
    const ROUNDS: usize = 3;
    const NTASKS: usize = 3;

    fn body(arg: usize) {
        for round in 0..ROUNDS {
            serial_println!(":: SCHED: task {} round {} (cpu 0) ::", arg, round);
            yield_now();
        }
        serial_println!(":: SCHED: task {} done ::", arg);
    }

    serial_println!(":: AARCH64 SCHED: cooperative demo — {} tasks x {} rounds on cpu 0 ::", NTASKS, ROUNDS);
    for t in 0..NTASKS {
        spawn("demo", body, t, 0);
    }
    run_until_empty(0);
    serial_println!(":: AARCH64 SCHED: cooperative demo complete (context switch + round-robin OK) ::");
}
