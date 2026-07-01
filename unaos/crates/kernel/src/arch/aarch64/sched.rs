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
//   * M3a — the COOPERATIVE core: a task runs until it `yield_now`s (round-robin) or returns
//     (`exit`). No interrupt delivery needed, so it's fully exercisable in QEMU raspi4b (which
//     withholds Group-1 IRQs on the `pi` build — same reason the timer poll-spins there).
//   * M3b — PREEMPTION + the APs: each secondary enters its own `run()` loop; the per-core generic
//     timer preempts the running task at quantum expiry (`timer_preempt`, from the IRQ handler
//     AFTER EOI). Preemption from an interrupt requires the IRQ stub to now save ELR/SPSR
//     (exceptions.rs) — those are per-core system registers, not a per-context stacked frame.
//     Like the SGI IPIs, timer delivery is metal-only (QEMU won't deliver it), so on QEMU the APs
//     run their demo tasks to completion sequentially and on the Pi they interleave (preemption).

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

use super::percpu::{self, NUM_CPUS};
use super::timer;

/// Per-thread kernel stack. 16 KiB matches the x86 scheduler.
const TASK_STACK_SIZE: usize = 16 * 1024;

/// Timer ticks a task runs before preemption (~4 ms/tick × 3 = 12 ms quantum).
const QUANTUM_TICKS: u32 = 3;

// Task lifecycle. A `u8` behind an atomic: the running task writes it (yield/exit/preempt) and the
// scheduler reads it after the switch-back to decide requeue-vs-free.
const STATE_READY: u8 = 0;
const STATE_RUNNING: u8 = 1;
const STATE_FINISHED: u8 = 2;
/// Parked: off every run queue, waiting on the per-CPU sleeper list (`sleep_ticks`) until a
/// deadline passes. The scheduler parks the Box per the blocking task's "park action" (below)
/// instead of requeuing it, and re-readies it when the wake condition holds. Mirrors x86's
/// STATE_BLOCKED; M4b extends it to semaphore wait queues.
const STATE_BLOCKED: u8 = 3;

// What the scheduler should do with a task that switched back in the BLOCKED state — set by the
// blocking primitive (under IRQ-masked) just before it switches, read-and-cleared by the scheduler
// after the switch (same CPU, sequential; `switch_context` is the memory barrier between them).
const PARK_NONE: u8 = 0;
const PARK_WAITQ: u8 = 1; // hand the Box to a wait queue, then release that queue's lock (M4b+)
const PARK_SLEEP: u8 = 2; // push the Box onto this CPU's sleeper list with a wake deadline

/// Pre-reserved waiter capacity per `Semaphore`. The scheduler pushes a blocked task's Box into the
/// waiter list WHILE holding the semaphore's spinlock (the lock-handoff); a reallocating push there
/// would take the heap lock UNDER the semaphore lock. So the list is pre-reserved (`Semaphore::init`)
/// and `wait()` asserts it never exceeds this, making the park-side push provably allocation-free.
const WAIT_CAPACITY: usize = 32;

/// Initial DAIF planted in a fresh task's frame: IRQ masked (I bit). The task starts masked so the
/// switch-in is atomic; `task_trampoline` unmasks before running the body (so the timer can preempt
/// it). Matches x86's INITIAL_RFLAGS-with-IF=0.
const INITIAL_DAIF: u64 = 1 << 7;

/// Turns preemption on. Gated so the BSP's cooperative demo (which runs BEFORE this is set) is
/// provably un-preempted, matching x86's SCHED_ACTIVE. `timer_preempt` no-ops until it's true.
static SCHED_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Releases the APs from their wait loop into `run()`. Set once the AP run queues are populated.
static SCHED_GO: AtomicBool = AtomicBool::new(false);

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
    /// Logical CPU this task is pinned to. Tasks never migrate, so a woken task always returns to
    /// this core's run queue — which keeps its TPIDR_EL2 (per-CPU) view correct on resume, exactly
    /// as x86 relies on the GS base staying put. Read by `make_ready` when re-readying a woken task.
    cpu: u32,
}

/// One CPU's scheduler bookkeeping (interior-mutable atomics, so the array is a plain static).
struct SchedCpu {
    /// Saved SP of this CPU's scheduler/idle context — written by the first `switch_context` INTO a
    /// task (the save side), read to switch back. Write-before-read, so the initial 0 is never loaded.
    scheduler_sp: AtomicU64,
    /// Raw `*mut Task` currently running here, or 0. Owned by the scheduler loop.
    current: AtomicU64,
    /// Timer ticks left in the running task's quantum (set on dispatch, counted down in `timer_preempt`).
    quantum: AtomicU32,
    /// "Park action" for a task switching back BLOCKED: `PARK_*`. Set by the blocking primitive
    /// before its switch, read-and-cleared by the scheduler after. Same-CPU sequential, so `Relaxed`
    /// suffices (`switch_context` is the barrier between the writer and the reader).
    park_kind: AtomicU8,
    /// PARK_SLEEP: the wake deadline, in THIS CPU's local timer ticks (`percpu.ticks`).
    park_deadline: AtomicU64,
    /// PARK_WAITQ: the wait queue's `*mut VecDeque<Box<Task>>` (the scheduler pushes the Box here).
    park_waiters: AtomicU64,
    /// PARK_WAITQ: the wait queue's `*const AtomicBool` lock (the scheduler releases it AFTER the
    /// push — the lock-handoff that makes the wakeup lost-proof).
    park_lock: AtomicU64,
}

impl SchedCpu {
    const fn new() -> Self {
        SchedCpu {
            scheduler_sp: AtomicU64::new(0),
            current: AtomicU64::new(0),
            quantum: AtomicU32::new(0),
            park_kind: AtomicU8::new(PARK_NONE),
            park_deadline: AtomicU64::new(0),
            park_waiters: AtomicU64::new(0),
            park_lock: AtomicU64::new(0),
        }
    }
}

static SCHED: [SchedCpu; NUM_CPUS] = [const { SchedCpu::new() }; NUM_CPUS];

/// Per-CPU ready queues. `VecDeque::new` is const, so no lazy_static; a `push` may allocate, but
/// only at `spawn` (never under the switch), so the brief lock is realloc-free in the hot path.
static RUN_QUEUES: [Mutex<VecDeque<Box<Task>>>; NUM_CPUS] =
    [const { Mutex::new(VecDeque::new()) }; NUM_CPUS];

/// Per-CPU sleeper lists: tasks blocked in `sleep_ticks`, tagged with their wake deadline (this
/// CPU's `percpu.ticks`). Touched ONLY by the scheduler on the OWNING CPU (parked there on the
/// switch-back, drained at the loop top), so the lock is always uncontended — it exists solely to
/// make the field interior-mutable, not for cross-CPU synchronisation. Being single-CPU, a
/// `push_back` that reallocates only ever nests the (innermost) heap lock, never another sched lock.
static SLEEPERS: [Mutex<VecDeque<Sleeper>>; NUM_CPUS] =
    [const { Mutex::new(VecDeque::new()) }; NUM_CPUS];

/// A parked sleeper: its wake deadline (owning CPU's `percpu.ticks`) and the task it belongs to.
struct Sleeper {
    deadline: u64,
    task: Box<Task>,
}

// --- Interrupt-state helpers (the scheduler's critical sections run with IRQ masked). ---
#[inline]
fn mask_irq() {
    unsafe { core::arch::asm!("msr daifset, #2", options(nomem, nostack, preserves_flags)) };
}
#[inline]
fn unmask_irq() {
    unsafe { core::arch::asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags)) };
}
/// Snapshot DAIF then mask IRQ. The blocking primitives (`Semaphore`, and from M4d the `Condvar`
/// that releases a mutex mid-critical-section) restore the SNAPSHOT rather than unconditionally
/// unmasking, so they nest correctly: a `post()` invoked with IRQ already masked leaves it masked.
/// (`yield_now`/`exit`/`sleep_ticks` are only ever entered from an unmasked task body, so they use
/// the simpler unconditional mask/unmask.)
#[inline]
fn irq_save_mask() -> u64 {
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags));
        core::arch::asm!("msr daifset, #2", options(nomem, nostack, preserves_flags));
    }
    daif
}
#[inline]
fn irq_restore(daif: u64) {
    unsafe { core::arch::asm!("msr daif, {}", in(reg) daif, options(nomem, nostack, preserves_flags)) };
}

// `switch_context(old_sp: *mut u64, new_sp: u64)` — AAPCS64: x0 = old_sp, x1 = new_sp. Saves DAIF +
// the callee-saved registers (x19-x30 and d8-d15) of the current context onto the current stack,
// stores the resulting SP through `old_sp`, loads `new_sp`, restores that context's registers +
// DAIF, and `ret`s into it (x30 = the restored return address). Caller-saved registers need no
// saving — this is a normal C-ABI call, so the compiler already treats them as clobbered — but the
// AAPCS64 CALLEE-saved FP lanes d8-d15 must be preserved here, or a cooperative task holding a live
// float across `yield_now` would resume with it clobbered by the task it yielded to (the preemptive
// path additionally saves the FULL v0-v31 in the IRQ stub, since an async interrupt can catch any
// v-register live). 176-byte frame (12 GPRs + DAIF + pad + 8 doubles), 16-aligned:
//   [+0..88] x19..x30  [+96] DAIF  [+104] pad  [+112..168] d8..d15
core::arch::global_asm!(
    "
    .globl switch_context
    switch_context:
        mrs   x9, daif
        sub   sp, sp, #176
        stp   x19, x20, [sp, #0]
        stp   x21, x22, [sp, #16]
        stp   x23, x24, [sp, #32]
        stp   x25, x26, [sp, #48]
        stp   x27, x28, [sp, #64]
        stp   x29, x30, [sp, #80]
        str   x9, [sp, #96]
        stp   d8,  d9,  [sp, #112]
        stp   d10, d11, [sp, #128]
        stp   d12, d13, [sp, #144]
        stp   d14, d15, [sp, #160]
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
        ldp   d8,  d9,  [sp, #112]
        ldp   d10, d11, [sp, #128]
        ldp   d12, d13, [sp, #144]
        ldp   d14, d15, [sp, #160]
        add   sp, sp, #176
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
/// `task_trampoline`. Returns the value to store in `ctx_sp`. The 176-byte frame (matching
/// `switch_context`) sits below a 16-aligned top; x30's slot holds the trampoline, DAIF's slot holds
/// IRQ-masked, the rest (x19-x29, pad, d8-d15) zero.
fn build_initial_frame(stack: &mut [u8]) -> u64 {
    let base = stack.as_mut_ptr() as usize;
    let top = (base + stack.len()) & !0xF;
    let sp = top - 176;
    unsafe {
        let p = sp as *mut u64;
        for i in 0..22 {
            p.add(i).write(0); // x19..x29, pad, d8..d15
        }
        p.add(11).write(task_trampoline as usize as u64); // x30 (lr) slot -> ret lands in trampoline
        p.add(12).write(INITIAL_DAIF); // DAIF slot (offset 96)
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
        cpu: cpu as u32,
    });
    RUN_QUEUES[cpu].lock().push_back(task);
    // Wake the target if it's a different, possibly-idle core (same-core needs no poke).
    poke_cpu(cpu);
    id
}

/// Send the wake/reschedule SGI (IPI_RESCHED) to `target` so, if it is idle in WFI, it breaks out
/// and its scheduler loop re-polls its run queue. Skips a self-poke. The IPI never context-switches
/// — it just has to interrupt WFI; the target's `run()` picks the queued work up. On metal the SGI
/// turns cross-core spawn/wake latency from "up to one timer tick" into ~immediate and is the ONLY
/// wake source for a genuinely idle core; in QEMU raspi4b the SGI is not delivered, but the idle
/// path there is a poll-spin (the timer isn't live), so `run()` still re-polls the queue and picks
/// the task up — cross-core delivery is thus the QEMU-invisible half, validated on metal.
fn poke_cpu(target: usize) {
    let this = percpu::this_cpu().cpu_index as usize;
    if target != this {
        super::gic::send_sgi(target, super::smp::IPI_RESCHED);
    }
}

/// Mark a parked/just-woken task READY, push it onto its PINNED CPU's run queue, and poke that CPU.
/// Used by the sleeper drain (same CPU) and, from M4b, `Semaphore::post` (cross-CPU wake). The task
/// always returns to `task.cpu`, so its per-CPU (TPIDR_EL2) view stays correct on resume — tasks do
/// not migrate. Caller runs with IRQ masked.
fn make_ready(task: Box<Task>) {
    let target = task.cpu as usize;
    debug_assert!(target < NUM_CPUS, "make_ready: cpu out of range");
    task.state.store(STATE_READY, Ordering::Release);
    RUN_QUEUES[target].lock().push_back(task);
    poke_cpu(target);
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

/// Block the current task for `n` of THIS CPU's local timer ticks, then become runnable again.
/// Timer-driven with no waker, so it cannot lose a wakeup: the scheduler drains due sleepers at its
/// loop top and the free-running periodic timer re-enters that loop every tick, so granularity and
/// worst-case wake latency are one tick. `n == 0` wakes on the next scheduler pass. No-op outside a
/// scheduled task (the unscheduled BSP / idle context), like `yield_now`.
///
/// METAL-ONLY WAKE (QEMU-invisible): the wake source is the per-CPU generic-timer tick, and QEMU
/// raspi4b never delivers the timer IRQ (`percpu.ticks` stays frozen at 0 there), so a sleeper
/// parked in QEMU never wakes. The *parking* machinery is exercised in QEMU; the *wake* is validated
/// on the real Pi 4 (each core's timer is LIVE). Callers must not depend on it completing where
/// `timer::is_live()` is false. Interrupts are masked across the state flip + switch (like
/// `yield_now`) and unmasked on resume — task bodies always run with IRQ unmasked.
pub fn sleep_ticks(n: u64) {
    let cpu = percpu::this_cpu().cpu_index as usize;
    mask_irq();
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    if !raw.is_null() {
        let deadline = percpu::this_cpu().ticks.load(Ordering::Relaxed) + n;
        unsafe {
            debug_assert_eq!((*raw).cpu as usize, cpu, "sleep_ticks: task on the wrong CPU");
            (*raw).state.store(STATE_BLOCKED, Ordering::Release);
            // Ask the scheduler to park us on this CPU's sleeper list with this deadline (consumed in
            // `dispatch_next`'s switch-back via `park_blocked`).
            SCHED[cpu].park_deadline.store(deadline, Ordering::Relaxed);
            SCHED[cpu].park_kind.store(PARK_SLEEP, Ordering::Relaxed);
            switch_context(
                &raw mut (*raw).ctx_sp,
                SCHED[cpu].scheduler_sp.load(Ordering::Acquire),
            );
        }
        // Resumed (IRQ masked, carried by the switch) once the deadline passed and the scheduler
        // re-dispatched us.
    }
    unmask_irq();
}

/// Preempt the running task at quantum expiry. Called from the timer IRQ path (`gic::handle_irq`,
/// AFTER EOI) on the core that ticked. Counts the quantum down; when it runs out, marks the task
/// ready and switches to the scheduler, which requeues it and runs the next task. We resume here
/// (IRQ masked, carried) once re-dispatched, then unwind back through the IRQ stub — which restores
/// this task's ELR/SPSR and `eret`s it. No-op unless a scheduled task is running on this core.
pub fn timer_preempt() {
    if !SCHED_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    if raw.is_null() {
        return; // scheduler/idle context, or an unscheduled core (the BSP)
    }
    let remaining = SCHED[cpu].quantum.load(Ordering::Relaxed);
    if remaining > 1 {
        SCHED[cpu].quantum.store(remaining - 1, Ordering::Relaxed);
        return;
    }
    SCHED[cpu].quantum.store(0, Ordering::Relaxed);
    unsafe {
        (*raw).state.store(STATE_READY, Ordering::Release);
        switch_context(
            &raw mut (*raw).ctx_sp,
            SCHED[cpu].scheduler_sp.load(Ordering::Acquire),
        );
    }
}

/// Dispatch the front task of `cpu`'s queue: switch into it, and when it switches back (yield /
/// preempt / exit) requeue it (READY) or free it (FINISHED). Returns whether a task ran. The caller
/// runs on this CPU's scheduler stack. IRQ is masked across pop+switch (nothing may re-enter the
/// scheduler on its own stack); on an empty queue IRQ is left UNMASKED for the caller to idle.
fn dispatch_next(cpu: usize) -> bool {
    mask_irq();
    let Some(task) = RUN_QUEUES[cpu].lock().pop_front() else {
        unmask_irq();
        return false;
    };
    task.state.store(STATE_RUNNING, Ordering::Release);
    SCHED[cpu].quantum.store(QUANTUM_TICKS, Ordering::Relaxed);
    let raw = Box::into_raw(task);
    let entry_sp = unsafe { (*raw).ctx_sp };
    // Publish `current` (Release) strictly before switching in — the trampoline reads it Acquire.
    SCHED[cpu].current.store(raw as u64, Ordering::Release);
    unsafe {
        switch_context(SCHED[cpu].scheduler_sp.as_ptr(), entry_sp);
    }
    // The switch-back always lands IRQ-masked (yield_now/exit mask first; timer_preempt runs in the
    // auto-masked IRQ handler), so the Box reclaim below can't race a re-entrant preempt on this
    // core. Re-assert the mask explicitly so that safety doesn't rest on an inherited DAIF that a
    // future switch-in path could leave enabled.
    mask_irq();
    SCHED[cpu].current.store(0, Ordering::Release);
    // Consume the park action exactly once: read it and immediately reset to NONE, so a stale action
    // can never leak into the next task's switch-back. Only a task that switched back BLOCKED carries
    // a meaningful action.
    let park = SCHED[cpu].park_kind.swap(PARK_NONE, Ordering::Relaxed);
    let task = unsafe { Box::from_raw(raw) };
    match task.state.load(Ordering::Acquire) {
        STATE_FINISHED => drop(task), // free the stack
        STATE_BLOCKED => park_blocked(cpu, park, task), // sleeper list / (M4b) a wait queue
        _ => {
            // READY (yielded or preempted): rotate to the back of the run queue.
            debug_assert_eq!(park, PARK_NONE, "non-blocked task carried a park action");
            task.state.store(STATE_READY, Ordering::Release);
            RUN_QUEUES[cpu].lock().push_back(task);
        }
    }
    true
}

/// Park a task that switched back BLOCKED, per the action it set before switching. Runs in the
/// scheduler context with IRQ masked and owns `task`.
fn park_blocked(cpu: usize, park: u8, task: Box<Task>) {
    match park {
        PARK_WAITQ => {
            // Lock-handoff: the blocking task acquired the wait queue's lock and held it ACROSS the
            // switch; WE push its Box into the waiter list and THEN release that lock — strictly in
            // that order. Releasing only AFTER the push is what makes the wakeup lost-proof: a
            // `post()` on another core spins on the lock and so cannot observe the queue until the
            // Box is in it. The waiter list is pre-reserved (see `Semaphore::wait`), so this
            // `push_back` never reallocates and so never takes the heap lock under the held lock.
            let waiters = SCHED[cpu].park_waiters.load(Ordering::Relaxed) as *mut VecDeque<Box<Task>>;
            let lock = SCHED[cpu].park_lock.load(Ordering::Relaxed) as *const AtomicBool;
            unsafe {
                (*waiters).push_back(task);
                (*lock).store(false, Ordering::Release); // release the handed-off lock, LAST
            }
        }
        PARK_SLEEP => {
            let deadline = SCHED[cpu].park_deadline.load(Ordering::Relaxed);
            SLEEPERS[cpu].lock().push_back(Sleeper { deadline, task });
        }
        _ => {
            // A BLOCKED task with no valid park action is a bug; don't leak it — drop it (frees the stack).
            debug_assert!(false, "BLOCKED task with no park action");
            drop(task);
        }
    }
}

/// Move every sleeper on this CPU whose deadline has passed back onto the run queue. Called at the
/// scheduler loop top with IRQ masked. The sleeper lock is released before `make_ready` so its
/// run-queue lock is never nested under the sleeper lock.
fn drain_due_sleepers(cpu: usize) {
    let now = percpu::this_cpu().ticks.load(Ordering::Relaxed);
    loop {
        let due = {
            let mut sleepers = SLEEPERS[cpu].lock();
            match sleepers.iter().position(|s| s.deadline <= now) {
                Some(i) => sleepers.remove(i).map(|s| s.task),
                None => None,
            }
        }; // sleeper lock dropped here
        match due {
            Some(task) => make_ready(task),
            None => break,
        }
    }
}

/// Run `cpu`'s run queue to completion, cooperatively (the M3a demo driver on the BSP): dispatch
/// tasks until the queue drains, then return. Used before preemption is enabled, so tasks only
/// switch via `yield_now`/`exit`. It does NOT drain the sleeper list, so a task that `sleep_ticks`
/// (or otherwise blocks) here would be parked and never re-dispatched — the blocking primitives are
/// exercised on the APs' `run()` loop, which does service sleepers.
pub fn run_until_empty(cpu: usize) {
    while dispatch_next(cpu) {}
}

/// The APs' scheduler loop: dispatch ready tasks forever, idling (WFI on metal / poll in QEMU, via
/// `arch::hlt`) when the queue is empty until the timer/an IPI makes work. Never returns.
fn run(cpu: usize) -> ! {
    loop {
        // Wake any sleepers whose deadline has passed (IRQ masked, matching the switch-back critical
        // section); `make_ready` pushes them onto THIS CPU's own run queue so the dispatch below
        // picks them up. The wake source is the free-running periodic timer — each tick breaks the
        // idle WFI and re-enters this loop — so an idle core with only a pending sleeper still makes
        // progress; worst-case wake latency is one tick. `dispatch_next` re-masks (redundant here),
        // then either switches into a task or, on an empty queue, unmasks and returns false to idle.
        mask_irq();
        drain_due_sleepers(cpu);
        if !dispatch_next(cpu) {
            crate::arch::hlt();
        }
    }
}

/// AP entry into scheduling (called from `smp::__secondary_rust` in place of the idle park). Waits
/// until the BSP populates the run queues and flips `SCHED_GO`, then runs this core's scheduler
/// loop. `arch::hlt` in the wait loop wakes on this core's own timer (metal) or spins (QEMU) to
/// re-check the flag. Never returns.
pub fn wait_and_run(cpu: usize) -> ! {
    while !SCHED_GO.load(Ordering::Acquire) {
        crate::arch::hlt();
    }
    run(cpu)
}

// ---------------------------------------------------------------------------------------------
// Semaphore — the inter-thread blocking primitive (counting; FIFO waiters)
// ---------------------------------------------------------------------------------------------

/// A counting semaphore for kernel threads. `wait()` blocks when the count is zero; `post()` wakes
/// one FIFO waiter (or bumps the count). Waking is cross-CPU aware: a task blocked on core B is
/// woken from core A by moving it to B's run queue and sending the reschedule SGI (IPI_RESCHED).
///
/// MUST outlive every task that can block on it: `wait()` hands raw pointers to `waiters`/`locked`
/// to the scheduler to be dereferenced after the context switch. Be `'static` (a `static SEM`), or
/// (from the join work) kept alive behind an `Arc` whose clones every party holds across the
/// park/post window.
///
/// Soundness of `UnsafeCell<VecDeque>` + `unsafe impl Sync`: EVERY access to `waiters` (and `count`)
/// is gated by the `locked` spinlock, and the park-side push is performed by the scheduler while the
/// blocker's lock is still held — before it releases it — establishing happens-before with the next
/// `post()`. The lock-handoff (hold `locked` across the switch into the scheduler; the scheduler
/// pushes the Box then releases it) is what makes the wakeup lost-proof: a `post()` on another core
/// spins on `locked` and so cannot observe the waiter list until the blocked Box is in it.
pub struct Semaphore {
    /// Raw spinlock guarding `count` and `waiters`. Acquire on lock, Release on unlock.
    locked: AtomicBool,
    /// Permit count; touched only under `locked` (Relaxed — the lock provides ordering). Always >= 0.
    count: AtomicI64,
    /// FIFO waiter list; touched only under `locked`. Pre-reserved to `WAIT_CAPACITY` by `init()`.
    waiters: UnsafeCell<VecDeque<Box<Task>>>,
}

// SAFETY: every access to the interior `waiters`/`count` is serialised by the `locked` spinlock; see
// the type doc for the full happens-before argument.
unsafe impl Sync for Semaphore {}

impl Semaphore {
    /// Construct a semaphore with `initial` permits. `const` so it can initialise a `static`.
    pub const fn new(initial: i64) -> Self {
        Semaphore {
            locked: AtomicBool::new(false),
            count: AtomicI64::new(initial),
            waiters: UnsafeCell::new(VecDeque::new()),
        }
    }

    /// Reserve the waiter list's capacity so the scheduler's park-side push never reallocates under
    /// the held lock. Call once on the BSP before any task can block on this semaphore.
    pub fn init(&self) {
        self.lock_raw();
        unsafe { (*self.waiters.get()).reserve(WAIT_CAPACITY) };
        self.unlock_raw();
    }

    #[inline]
    fn lock_raw(&self) {
        while self.locked.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
    }

    #[inline]
    fn unlock_raw(&self) {
        self.locked.store(false, Ordering::Release);
    }

    /// Acquire a permit, blocking the current task until one is available. Returns `true` once a
    /// permit is held. Returns `false` WITHOUT acquiring if called off a scheduled task (there is no
    /// `current` to block). A caller that hands out a resource on success (e.g. `Mutex::lock`) MUST
    /// check the return: `false` means NO permit was taken, so no resource may be issued.
    #[must_use]
    pub fn wait(&self) -> bool {
        let daif = irq_save_mask(); // IRQ masked for the whole critical section; restored on exit
        self.lock_raw();

        if self.count.load(Ordering::Relaxed) > 0 {
            self.count.fetch_sub(1, Ordering::Relaxed);
            self.unlock_raw();
            irq_restore(daif);
            return true; // fast path: acquired a permit without blocking
        }

        // No permit: block — but only a scheduled task can park. Off a scheduled task (the
        // unscheduled BSP / idle context) bail WITHOUT acquiring, so the count stays consistent.
        let cpu = percpu::this_cpu().cpu_index as usize;
        let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
        if raw.is_null() {
            self.unlock_raw();
            irq_restore(daif);
            return false;
        }

        // The lock is held continuously until the scheduler pushes, so the length cannot change
        // before then; asserting it here proves the park-side push won't reallocate.
        assert!(
            unsafe { (*self.waiters.get()).len() } < WAIT_CAPACITY,
            "Semaphore waiter overflow (raise WAIT_CAPACITY)"
        );

        unsafe {
            debug_assert_eq!((*raw).cpu as usize, cpu, "Semaphore::wait: task on the wrong CPU");
            (*raw).state.store(STATE_BLOCKED, Ordering::Release);
            // Hand the scheduler this semaphore's waiter list + lock: it pushes our Box, then
            // releases the lock AFTER the push (the lock-handoff). We keep `locked` held across the
            // switch; the scheduler releases it.
            SCHED[cpu].park_waiters.store(self.waiters.get() as u64, Ordering::Relaxed);
            SCHED[cpu].park_lock.store(&self.locked as *const AtomicBool as u64, Ordering::Relaxed);
            SCHED[cpu].park_kind.store(PARK_WAITQ, Ordering::Relaxed);
            switch_context(
                &raw mut (*raw).ctx_sp,
                SCHED[cpu].scheduler_sp.load(Ordering::Acquire),
            );
        }
        // Resumed once `post()` moved us back to our run queue. The scheduler already released the
        // lock (we must not touch it). We were handed the permit (post did NOT re-increment count),
        // so we now hold one.
        irq_restore(daif);
        true
    }

    /// Release a permit: wake one FIFO waiter if any, else increment the count. Wakes across cores.
    pub fn post(&self) {
        let daif = irq_save_mask();
        self.lock_raw();
        let waiter = unsafe { (*self.waiters.get()).pop_front() };
        match waiter {
            // Release the lock BEFORE make_ready so its run-queue lock is never nested under ours.
            Some(task) => {
                self.unlock_raw();
                make_ready(task);
            }
            None => {
                self.count.fetch_add(1, Ordering::Relaxed);
                self.unlock_raw();
            }
        }
        irq_restore(daif);
    }
}

// ---------------------------------------------------------------------------------------------
// Demos
// ---------------------------------------------------------------------------------------------

/// M3a smoke test: spawn a few cooperative kernel threads on the boot core and run them to
/// completion. Proves `switch_context` + the run queue + `spawn`/`yield_now`/`exit` — round-robin,
/// no interrupts required (so it runs identically in QEMU raspi4b and on metal). Runs BEFORE
/// preemption is enabled, so it is provably cooperative. Returns once every demo task has exited.
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

/// Busy-wait `ms` milliseconds off the free-running CNTPCT (works even where the timer IRQ isn't
/// delivered, i.e. QEMU). Used by the preemption demo so each task holds the CPU long enough to be
/// preempted on metal.
fn busy_delay_ms(ms: u64) {
    let freq = timer::cntfrq();
    let deadline = timer::cntpct() + freq * ms / 1000;
    while timer::cntpct() < deadline {
        core::hint::spin_loop();
    }
}

/// The preemption-demo task body: print a few lines with a busy-delay between them and NO yield, so
/// the only thing that can switch away is the timer. `arg` encodes core*10 + task.
fn preempt_body(arg: usize) {
    let (core, task) = (arg / 10, arg % 10);
    for i in 0..5 {
        serial_println!(":: SCHED: core {} task {} iter {} ::", core, task, i);
        busy_delay_ms(3);
    }
    serial_println!(":: SCHED: core {} task {} done ::", core, task);
}

/// M4a demo: prove `sleep_ticks` parks a task and this core's timer tick wakes it. Metal-only wake —
/// in QEMU raspi4b the timer IRQ is never delivered (`timer::is_live()` is false), so a real sleep
/// would park forever; there we skip the sleep and just report, keeping the AP's queue draining.
fn sleep_demo_body(arg: usize) {
    let core = arg;
    if !timer::is_live() {
        serial_println!(
            ":: SCHED: core {} SLEEP demo skipped — timer not delivered (QEMU); sleep-wake is metal-only ::",
            core
        );
        return;
    }
    const NAP: u64 = 25; // ticks (~100 ms at the 250 Hz per-core tick)
    let before = percpu::this_cpu().ticks.load(Ordering::Relaxed);
    serial_println!(":: SCHED: core {} SLEEP {} ticks (at tick {}) ::", core, NAP, before);
    sleep_ticks(NAP);
    let after = percpu::this_cpu().ticks.load(Ordering::Relaxed);
    serial_println!(
        ":: SCHED: core {} WOKE at tick {} (slept {} ticks; sleep_ticks OK) ::",
        core,
        after,
        after.wrapping_sub(before),
    );
}

/// Shared demo semaphore for the M4b producer/consumer (initialised in `start_aps`). A `static`, so
/// it satisfies the `'static` requirement for a cross-thread semaphore.
static DEMO_SEM: Semaphore = Semaphore::new(0);
/// Items the producer posts / the consumer waits for.
const SEM_ITEMS: usize = 5;

/// M4b producer: post the demo semaphore `SEM_ITEMS` times, with a short busy-delay before each post
/// so the consumer reaches its blocking `wait()` first and is woken cross-core (rather than the
/// consumer racing ahead on the fast path). Runs on a different AP than the consumer.
fn sem_producer_body(core: usize) {
    for i in 0..SEM_ITEMS {
        busy_delay_ms(2); // give the consumer time to block on the empty semaphore
        DEMO_SEM.post();
        serial_println!(":: SCHED: core {} SEM post #{} ::", core, i + 1);
    }
    serial_println!(":: SCHED: core {} SEM producer done ({} posts) ::", core, SEM_ITEMS);
}

/// M4b consumer: `wait()` on the demo semaphore `SEM_ITEMS` times and report. Each `wait()` when the
/// semaphore is empty BLOCKS the task; the producer's `post()` on another AP re-readies it — on
/// metal via the reschedule SGI breaking WFI, in QEMU via this core's run() busy-poll. Receiving all
/// `SEM_ITEMS` proves no wakeup was lost.
fn sem_consumer_body(core: usize) {
    let mut got = 0usize;
    for _ in 0..SEM_ITEMS {
        assert!(DEMO_SEM.wait(), "SEM consumer ran off a scheduled task");
        got += 1;
    }
    serial_println!(
        ":: SCHED: core {} SEM consumer got {}/{} => {} ::",
        core,
        got,
        SEM_ITEMS,
        if got == SEM_ITEMS { "PASS" } else { "FAIL" },
    );
}

/// M3b/M4a/M4b: turn on preemptive scheduling and put a workload on the APs, then flip SCHED_ACTIVE
/// (enables `timer_preempt`) and SCHED_GO (releases the APs into `run`). The workload:
///   * first online AP — a busy-pair (preemption regression) + the M4a `sleep_ticks` sleeper;
///   * (with >= 3 online APs) a counting-`Semaphore` producer/consumer across two OTHER APs.
/// On metal the busy-pair INTERLEAVES under the timer, the sleeper parks-then-wakes on its own tick,
/// and the consumer is woken cross-core by the producer's reschedule SGI. In QEMU (no Group-1 IRQ
/// delivery) the busy tasks run sequentially, the sleeper self-skips, and the consumer is woken by
/// its core's run() busy-poll (SGI delivery being the metal-only half). Call AFTER the BSP's
/// cooperative demo so it stays un-preempted.
pub fn start_aps(online: &[usize]) {
    if let Some(&c) = online.first() {
        spawn("busy-a", preempt_body, c * 10, c);
        spawn("busy-b", preempt_body, c * 10 + 1, c);
        spawn("sleeper", sleep_demo_body, c, c);
    }
    if online.len() >= 3 {
        DEMO_SEM.init(); // reserve waiter capacity on the BSP before any task can block on it
        let (consumer, producer) = (online[1], online[2]);
        spawn("sem-consumer", sem_consumer_body, consumer, consumer);
        spawn("sem-producer", sem_producer_body, producer, producer);
    } else {
        serial_println!(":: AARCH64 SCHED: Semaphore demo skipped (needs >= 3 online APs) ::");
    }
    SCHED_ACTIVE.store(true, Ordering::Release);
    SCHED_GO.store(true, Ordering::Release);
    serial_println!(
        ":: AARCH64 SCHED: preemption ON; busy-pair + sleep + Semaphore demo on APs {:?} ::",
        online
    );
}
