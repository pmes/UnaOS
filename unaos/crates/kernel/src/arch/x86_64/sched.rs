// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Per-CPU, preemptive, round-robin scheduler for kernel threads (x86_64).
//
// Built on the SMP foundation already in place: per-CPU GDT/TSS/IST, x2APIC with a per-CPU LVT
// timer (vector `TIMER_VECTOR`), GS-based per-CPU data (`percpu::this_cpu()`), and the
// reschedule IPI (vector `IPI_VECTOR`). Each application processor (AP) runs its OWN scheduler
// loop and drains its OWN run queue; the BSP is deliberately *not* scheduled — it stays the
// hardware-service core (xHCI / console / storage), exactly as before. APs run scheduled work.
//
// Model (xv6-style). Every CPU has a private "scheduler context" — the stack it was already
// running on when it entered `run()`. A runnable kernel thread (a `Task`) is switched *into* by
// `switch_context`, runs until it yields (`yield_now`), is preempted (the timer tick), or
// finishes (`exit`), at which point control switches *back* to that CPU's scheduler context,
// which requeues or frees the task and picks the next one. Tasks are CPU-pinned: a task only
// ever runs on the CPU whose run queue it sits in, so the per-CPU GS base stays correct across a
// switch with no save/restore (there is no task migration yet).
//
// Correctness invariants (these were pinned by an adversarial design review; do not weaken them
// without re-deriving the proofs):
//   * `switch_context` saves/restores RFLAGS (pushfq/popfq), so the interrupt flag travels WITH
//     each context. Every switch-away happens with interrupts disabled, so every resume restores
//     IF=0; a task's "running with interrupts on" state is re-established explicitly (the
//     trampoline's `sti`, `yield_now`'s post-switch `sti`) or, for a preempted task, by the
//     timer handler's deferred `iretq`.
//   * `current` (the running task's raw pointer) is owned SOLELY by the scheduler loop. Interrupt
//     handlers may only READ it and flip the task's atomic `state`; they never requeue or free.
//     Exactly one `Box::from_raw` per `Box::into_raw`, performed only in `run()` after the switch
//     returns. The IPI handler is wake-only — it never context-switches.
//   * The run-queue spin lock is held only briefly, with IF=0, and never across a context switch.
//     A lock holder therefore cannot be preempted (IF=0 masks the timer/IPI) — no self-deadlock.
//   * The idle sleep is the atomic `sti; hlt` pair (`enable_and_hlt`); a wake (timer or IPI) that
//     was latched before the `sti` still fires the handler and returns past the `hlt`, and the
//     loop then re-checks the queue — so a `spawn`+IPI landing in the idle window is never lost.
//   * `Condvar::wait` holds the ONE sanctioned "release a sibling lock while holding a wait-queue
//     lock": it calls the user `Mutex`'s `Semaphore::post` while still holding the condvar's own
//     spin lock, immediately before the switch — inherent to atomically releasing the mutex and
//     blocking. The lock order is therefore `cv.locked -> mutex.sem.locked -> run-queue lock ->
//     heap`, and it is acyclic (nothing ever takes a wait-queue lock while holding a run-queue
//     lock). `notify_one`/`notify_all` must NOT add a second such nesting: they pop a waiter under
//     the condvar lock but call `make_ready` only AFTER releasing it (the `Semaphore::post`
//     discipline), so `notify_all` drains one waiter per lock acquisition.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use lazy_static::lazy_static;
// Aliased so the public `Mutex<T>` (a sleeping mutex, below) can own the nicer name. This spin
// lock guards the internal run/sleeper queues only.
use spin::Mutex as SpinMutex;

use crate::arch::gdt::MAX_CPUS;
use crate::arch::{apic, percpu};

/// Per-task kernel stack. 16 KiB is generous for kernel threads (the deepest thing they do is
/// `serial_println!` formatting); bump if a workload needs more.
const TASK_STACK_SIZE: usize = 16 * 1024;

/// Preemption quantum, in local-APIC timer ticks. After this many ticks a running task is
/// preempted and rotated to the back of its run queue. Small so round-robin sharing is visible.
const QUANTUM_TICKS: u32 = 4;

/// Priority aging (anti-starvation). A ready task that has WAITED in the run queue this many local
/// ticks without being dispatched is promoted one effective level toward the top (its base priority
/// is unchanged). Repeated, a low task under continuous higher-priority load climbs to parity with
/// the load and runs, then drops back to base on dispatch — bounding starvation to ~`AGE_TICKS` per
/// level it must climb. (Bound is exact when no level between base and the contended level drains;
/// finite-but-larger under bursty mixed load, since a dispatch at an intermediate level re-bases.)
const AGE_TICKS: u32 = 16;

/// How often the scheduler runs the aging sweep, in local ticks. Kept well below `AGE_TICKS` so the
/// one-promotion-per-sweep cap never binds in steady state (a sweep accrues elapsed credit and
/// carries any surplus past `AGE_TICKS` to the next sweep, so coarse/late sweeps lose no credit).
const AGING_INTERVAL: u64 = 4;

/// Pre-reserved run-queue capacity per CPU. Sized so steady-state `push_back` never reallocates
/// (which would take the heap lock while the run-queue lock is held). Exceeding it is still
/// correct — the heap lock is always innermost — but avoid it.
const RUNQ_CAPACITY: usize = 64;

/// Pre-reserved waiter capacity per `Semaphore`. The scheduler pushes a blocked task's Box into the
/// waiter list WHILE holding the semaphore's spinlock (the lock-handoff); if that push reallocated
/// it would take the heap lock UNDER the semaphore lock — a lock-ordering inversion that can
/// deadlock a concurrent `post()`. So the waiter list is pre-reserved and `wait()` asserts it never
/// exceeds this, making the park-side push provably allocation-free.
const WAIT_CAPACITY: usize = 32;

/// RFLAGS planted in a fresh task's initial frame: reserved bit 1 set (always 1), IF=0, DF=0.
/// The task starts masked; its trampoline enables interrupts explicitly.
const INITIAL_RFLAGS: u64 = 0x0000_0002;

// ---------------------------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------------------------

// Task state, as a `u8` behind an atomic (a handler on this CPU writes it while the scheduler
// reads it; making it atomic keeps that well-defined and future-proofs migration).
const STATE_READY: u8 = 0;
const STATE_RUNNING: u8 = 1;
const STATE_FINISHED: u8 = 2;
/// Parked: off every run queue, waiting in a semaphore's waiter list or the per-CPU sleeper list
/// until woken (`Semaphore::post` / a sleep deadline). The scheduler parks the Box per the
/// blocking task's "park action" (below) instead of requeuing it.
const STATE_BLOCKED: u8 = 3;

// What the scheduler should do with a task that switched back in the BLOCKED state — set by the
// blocking primitive (under IF=0) just before it switches, read-and-cleared by `run()` after the
// switch (same CPU, sequential; `switch_context` is the barrier).
const PARK_NONE: u8 = 0;
const PARK_WAITQ: u8 = 1; // hand the Box to a wait queue, then release that queue's lock
const PARK_SLEEP: u8 = 2; // push the Box onto this CPU's sleeper list with a wake deadline

/// Number of scheduling priority levels. A CPU always runs a ready task of the HIGHEST non-empty
/// level; within a level scheduling is round-robin (FIFO). Higher number = higher priority.
pub const NUM_PRIORITIES: usize = 4;
/// Convenience priority levels (any `0..NUM_PRIORITIES` is valid; out-of-range is clamped).
pub const PRIO_LOW: u8 = 0;
pub const PRIO_NORMAL: u8 = 1;
pub const PRIO_HIGH: u8 = 2;
pub const PRIO_RT: u8 = 3;
/// Sentinel for `SchedCpu::current_prio` meaning "no task running" (CPU idle). Compares below any
/// real priority so a newly-ready task always wakes an idle core.
const PRIO_IDLE: u8 = u8::MAX;

/// A kernel thread. Owned as `Box<Task>`: it lives in exactly one place at a time — a run queue
/// (`Box` in the `VecDeque`), or "running" (the `Box` leaked to a raw pointer in `current`).
pub struct Task {
    id: u64,
    /// Human-readable label, for the `sched` shell command and logs.
    #[allow(dead_code)]
    name: &'static str,
    /// `STATE_*`. Written by the owning CPU's handlers/scheduler only.
    state: AtomicU8,
    /// Saved stack pointer (the whole register context lives on the stack `switch_context` built).
    ctx_rsp: u64,
    /// Owns the stack memory; freed when the task is dropped (on the `Finished` path).
    #[allow(dead_code)]
    stack: Box<[u8]>,
    entry: fn(usize),
    arg: usize,
    /// Logical CPU this task is pinned to (only read by a `debug_assert`, hence the allow).
    #[allow(dead_code)]
    cpu: u32,
    /// BASE scheduling priority (`0..NUM_PRIORITIES`, higher = more urgent). IMMUTABLE after spawn,
    /// so it is safe to read lock-free from any CPU (the preemption decision when waking/spawning).
    /// Aging never touches this — it only transiently raises the *level* a task sits in (below).
    priority: u8,
    /// Ticks this task has WAITED in a run queue since its last enqueue, for priority aging. Touched
    /// ONLY under the owning CPU's run-queue spinlock (zeroed by `RunQueue::push` on every enqueue,
    /// accrued + consumed by `RunQueue::age` on that CPU). NEVER read cross-CPU — unlike `priority`,
    /// it is mutable and lock-protected, so it must not be read off the owning CPU.
    wait_ticks: u32,
}

// ---------------------------------------------------------------------------------------------
// Per-CPU scheduler state
// ---------------------------------------------------------------------------------------------

/// One CPU's scheduler bookkeeping. All interior-mutable (atomics), so the array is a plain
/// immutable `static` — no `static mut`, no lock for the fields themselves.
struct SchedCpu {
    /// Saved rsp of this CPU's scheduler/idle context (written by `switch_context` when the
    /// scheduler switches into a task; read to switch back). Write-before-read: the first switch
    /// targeting it is always the save side, so the initial 0 is never loaded into rsp.
    scheduler_rsp: AtomicU64,
    /// Raw `*mut Task` of the task currently running on this CPU, or 0. Owned by `run()`.
    current: AtomicU64,
    /// Ticks remaining in the current task's quantum.
    quantum: AtomicU32,
    /// Set when the running task should be preempted at the next safe point (quantum expiry, or
    /// an explicit reschedule request — e.g. a higher-priority task became ready). Single signal.
    need_resched: AtomicBool,
    /// Priority of the task currently running here, or `PRIO_IDLE` if none. Set on dispatch, reset
    /// on switch-back. Read (best-effort, Acquire) by a waker/spawner on another CPU to decide
    /// whether the newly-ready task outranks what's running and should preempt it.
    current_prio: AtomicU8,
    /// "Park action" for a task switching back BLOCKED: `PARK_*`. Set by the blocking primitive
    /// before its switch, read-and-cleared by `run()` after. Same-CPU sequential, so `Relaxed`
    /// is sufficient (`switch_context` is the memory barrier between writer and reader).
    park_kind: AtomicU8,
    /// PARK_WAITQ: the wait queue's `*mut VecDeque<Box<Task>>` (the scheduler pushes the Box here).
    park_waiters: AtomicU64,
    /// PARK_WAITQ: the wait queue's `*const AtomicBool` lock (the scheduler releases it after the
    /// push — the lock-handoff that makes the wakeup lost-proof).
    park_lock: AtomicU64,
    /// PARK_SLEEP: the wake deadline, in this CPU's local timer ticks.
    park_deadline: AtomicU64,
}

impl SchedCpu {
    const fn new() -> Self {
        SchedCpu {
            scheduler_rsp: AtomicU64::new(0),
            current: AtomicU64::new(0),
            quantum: AtomicU32::new(0),
            need_resched: AtomicBool::new(false),
            current_prio: AtomicU8::new(PRIO_IDLE),
            park_kind: AtomicU8::new(PARK_NONE),
            park_waiters: AtomicU64::new(0),
            park_lock: AtomicU64::new(0),
            park_deadline: AtomicU64::new(0),
        }
    }
}

static SCHED: [SchedCpu; MAX_CPUS] = [const { SchedCpu::new() }; MAX_CPUS];

/// True once the BSP has finished SMP verification and turned scheduling on. Gates the timer
/// handler's preempt branch so the pre-scheduler smoke test (`smp::verify_smp`) is provably
/// identical to before (no context switches during it).
static SCHED_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Releases the APs from their post-online wait loop into `run()`. Set by the BSP only after
/// `verify_smp` has run against idle APs.
static SCHED_GO: AtomicBool = AtomicBool::new(false);

/// Monotonic task-id source.
static NEXT_TID: AtomicU64 = AtomicU64::new(1);

/// A CPU's ready tasks, bucketed by EFFECTIVE level. A task normally sits at its base priority
/// level, but the aging sweep (`age`) may transiently lift a long-waiting task to a higher level so
/// strict priority does not starve it; on its next enqueue it re-bases. One spinlock (in
/// `RUN_QUEUES`) guards all levels; held only briefly (push/pop are O(NUM_PRIORITIES); `age` is
/// O(ready tasks)) and always with IF=0.
///
/// Two distinct placement operations share these levels: ENQUEUE (`push`) re-bases a task to its
/// base-priority level and ZEROES its aging clock; RELOCATE (`age`) moves a task one level UP
/// without touching its base priority. They must not be confused (relocating via `push` would be a
/// no-op promotion that leaves starvation intact).
struct RunQueue {
    levels: [VecDeque<Box<Task>>; NUM_PRIORITIES],
}

impl RunQueue {
    fn with_capacity(cap: usize) -> Self {
        RunQueue { levels: core::array::from_fn(|_| VecDeque::with_capacity(cap)) }
    }
    /// ENQUEUE a task at its BASE priority level (FIFO within the level), clamped in range, and
    /// reset its aging clock — every enqueue (spawn / wake / re-enqueue after preempt/yield) zeroes
    /// `wait_ticks`, so a task only ages while it sits WAITING and re-bases the moment it is requeued.
    fn push(&mut self, mut task: Box<Task>) {
        task.wait_ticks = 0;
        let level = (task.priority as usize).min(NUM_PRIORITIES - 1);
        self.levels[level].push_back(task);
    }
    /// Dequeue the front of the HIGHEST non-empty level (strict priority over the effective level,
    /// round-robin within).
    fn pop_highest(&mut self) -> Option<Box<Task>> {
        for level in self.levels.iter_mut().rev() {
            if let Some(task) = level.pop_front() {
                return Some(task);
            }
        }
        None
    }
    /// Priority-aging sweep (anti-starvation): RELOCATE every ready task that has now waited at
    /// least `AGE_TICKS` one level UP, carrying any surplus credit to the next sweep. `elapsed` is
    /// the local ticks since the previous sweep. Run on the OWNING CPU under the run-queue lock.
    ///
    /// Iterating HIGH→LOW is load-bearing: a task promoted from `level` into `level + 1` lands in a
    /// level that was ALREADY processed this sweep, so it is never revisited (exactly-once per
    /// sweep, no runaway multi-level jump). Within a level, popping exactly `n = len()` from the
    /// front and pushing kept tasks to the back rotates the deque full-circle, preserving FIFO.
    /// Relocation is a raw `VecDeque` move that leaves `priority` (base) untouched — NOT `push`.
    fn age(&mut self, elapsed: u32) {
        for level in (0..NUM_PRIORITIES - 1).rev() {
            let n = self.levels[level].len();
            for _ in 0..n {
                let mut task = self.levels[level].pop_front().expect("age: len/pop mismatch");
                task.wait_ticks = task.wait_ticks.saturating_add(elapsed);
                if task.wait_ticks >= AGE_TICKS {
                    task.wait_ticks -= AGE_TICKS; // carry surplus credit, don't discard it
                    debug_assert!(level + 1 < NUM_PRIORITIES, "age: promotion above top level");
                    self.levels[level + 1].push_back(task); // RELOCATE up one level (base unchanged)
                } else {
                    self.levels[level].push_back(task);
                }
            }
        }
    }
    fn len(&self) -> usize {
        self.levels.iter().map(VecDeque::len).sum()
    }
}

lazy_static! {
    /// Per-CPU multilevel run queues. The lock protects only the queue structure; a `Task`'s own
    /// fields are touched solely by the CPU that owns it. Cross-CPU `spawn`/wake pushes under the
    /// lock. Each level is pre-reserved so `push` never reallocates under the lock.
    static ref RUN_QUEUES: [SpinMutex<RunQueue>; MAX_CPUS] =
        core::array::from_fn(|_| SpinMutex::new(RunQueue::with_capacity(RUNQ_CAPACITY)));

    /// Per-CPU sleeper lists: tasks blocked in `sleep_ticks`, with their wake deadline (this CPU's
    /// tick count). Touched ONLY by `run()` on the owning CPU (parked there on switch-back, drained
    /// at the loop top), so the lock is always uncontended — it exists only so the field is
    /// interior-mutable, not for cross-CPU synchronisation.
    static ref SLEEPERS: [SpinMutex<VecDeque<Sleeper>>; MAX_CPUS] =
        core::array::from_fn(|_| SpinMutex::new(VecDeque::with_capacity(RUNQ_CAPACITY)));
}

/// A parked sleeper: its wake deadline (owning CPU's tick count) and the task.
struct Sleeper {
    deadline: u64,
    task: Box<Task>,
}

// ---------------------------------------------------------------------------------------------
// Context switch (the only assembly in the scheduler)
// ---------------------------------------------------------------------------------------------

// `switch_context(old_rsp: *mut u64, new_rsp: u64)` — SysV: rdi = old_rsp, rsi = new_rsp.
// Saves RFLAGS + the callee-saved registers of the current context onto the current stack, stores
// the resulting rsp through `old_rsp`, loads `new_rsp`, restores that context's saved registers +
// RFLAGS, and `ret`s into it. Caller-saved registers need no saving — `switch_context` is a normal
// C-ABI call, so the compiler already treats them as clobbered around it. A true naked symbol (no
// compiler prologue/epilogue): the first instruction is `pushfq`, the last is `ret`.
//
// Stack frame it builds/consumes, from the saved rsp upward:
//   [rsp+ 0] r15  [+ 8] r14  [+16] r13  [+24] r12  [+32] rbx  [+40] rbp  [+48] RFLAGS  [+56] rip
core::arch::global_asm!(
    "
    .globl switch_context
    switch_context:
        pushfq
        push rbp
        push rbx
        push r12
        push r13
        push r14
        push r15
        mov [rdi], rsp
        mov rsp, rsi
        pop r15
        pop r14
        pop r13
        pop r12
        pop rbx
        pop rbp
        popfq
        ret
    "
);

unsafe extern "C" {
    fn switch_context(old_rsp: *mut u64, new_rsp: u64);
}

/// First code every kernel thread runs. Reached when `switch_context` `ret`s into a freshly-built
/// initial frame. Establishes an ABI-clean, interrupts-on context, then runs the task body.
extern "C" fn task_trampoline() -> ! {
    // The switch carried RFLAGS, but be belt-and-suspenders ABI-clean: DF=0 for rep-string ops.
    unsafe { core::arch::asm!("cld", options(nomem, nostack, preserves_flags)) };
    // Kernel threads run with interrupts ENABLED (so the timer can preempt them). The fresh frame
    // started masked (INITIAL_RFLAGS has IF=0); turn them on now.
    x86_64::instructions::interrupts::enable();

    // `current` was published (Release) by `run()` strictly before the switch into us.
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *const Task;
    debug_assert!(!raw.is_null(), "task_trampoline: current is null");
    let (entry, arg) = unsafe { ((*raw).entry, (*raw).arg) };

    entry(arg);
    exit();
}

/// Build a fresh task's initial stack frame so the first `switch_context` into it lands in
/// `task_trampoline` with an ABI-correct stack. Returns the value to store in `ctx_rsp`.
///
/// SysV requires rsp ≡ 8 (mod 16) at a function's first instruction (a `call` pushes an 8-byte
/// return address onto a 16-aligned rsp). After `switch_context` pops 6 regs + RFLAGS and `ret`s,
/// the trampoline sees rsp = (rip slot) + 8, so the rip slot must be 16-aligned — equivalently
/// `new_rsp ≡ 8 (mod 16)`. We CONSTRUCT that (don't merely assume it) and assert it.
fn build_initial_frame(stack: &mut [u8]) -> u64 {
    let base = stack.as_mut_ptr() as usize;
    // Align the frame top DOWN to 16. The 8 frame qwords sit below it; the slot at `top` itself
    // is left unused padding.
    let top = (base + stack.len()) & !0xF;
    // new_rsp points at the lowest frame slot (r15). 9 qwords below `top` (one pad + rip + RFLAGS
    // + 6 regs) ⇒ new_rsp = top - 72, which is ≡ 8 (mod 16).
    let new_rsp = top - 72;
    assert_eq!(new_rsp % 16, 8, "build_initial_frame: misaligned task stack");

    unsafe {
        let p = new_rsp as *mut u64;
        // Callee-saved registers (zeroed) consumed by the 6 pops.
        for i in 0..6 {
            p.add(i).write(0);
        }
        p.add(6).write(INITIAL_RFLAGS); // consumed by popfq
        p.add(7).write(task_trampoline as *const () as u64); // consumed by ret
    }
    new_rsp as u64
}

// ---------------------------------------------------------------------------------------------
// Public API: spawn / yield / exit
// ---------------------------------------------------------------------------------------------

/// Create a kernel thread at `priority` and enqueue it on `target_cpu`'s run queue, then poke that
/// CPU so it promptly picks the work up (wakes it from idle, or preempts a lower-priority task
/// running there). The task runs `entry(arg)` and is freed when `entry` returns. `target_cpu` must
/// be an online AP.
pub fn spawn(name: &'static str, entry: fn(usize), arg: usize, target_cpu: usize, priority: u8) {
    assert!(target_cpu < MAX_CPUS, "spawn: target_cpu out of range");

    let mut stack: Box<[u8]> = alloc::vec![0u8; TASK_STACK_SIZE].into_boxed_slice();
    let ctx_rsp = build_initial_frame(&mut stack);

    let task = Box::new(Task {
        id: NEXT_TID.fetch_add(1, Ordering::Relaxed),
        name,
        state: AtomicU8::new(STATE_READY),
        ctx_rsp,
        stack,
        entry,
        arg,
        cpu: target_cpu as u32,
        priority,
        wait_ticks: 0, // re-zeroed by push() on every enqueue; this satisfies the struct literal
    });

    RUN_QUEUES[target_cpu].lock().push(task);
    poke_for(target_cpu, priority);
}

/// Send the wake-only reschedule IPI (vector `IPI_VECTOR`) to `target` so an idle CPU breaks its
/// `hlt` and re-checks its run queue. Skips a self-poke (no point interrupting ourselves). The IPI
/// never context-switches; the target's scheduler loop picks the work up.
fn poke_cpu(target: usize) {
    let this = percpu::this_cpu().cpu_index as usize;
    if target != this {
        if let Some(c) = percpu::cpu(target) {
            let icr_low = 0x0000_4000 | crate::arch::interrupts::IPI_VECTOR as u32; // fixed, assert
            apic::send_ipi(c.apic_id, icr_low);
        }
    }
}

/// After enqueuing a newly-ready task of `prio` on `target`, decide whether to disturb that CPU:
/// wake it if idle, or ask it to reschedule (so it preempts a strictly-lower-priority running task
/// at its next tick). If the new task ranks at or below what's running, it simply waits its turn.
/// Best-effort: the read of `current_prio` may be momentarily stale, but the free-running periodic
/// timer always re-evaluates the run queue, so no work is lost — only the preemption latency varies.
fn poke_for(target: usize, prio: u8) {
    let running = SCHED[target].current_prio.load(Ordering::Acquire);
    if running == PRIO_IDLE {
        poke_cpu(target); // idle core: wake it to pick up the task (self-poke is a no-op)
    } else if prio > running {
        SCHED[target].need_resched.store(true, Ordering::Release);
        poke_cpu(target); // running a lower-priority task: preempt it at the next tick
    }
}

/// Mark a parked/just-woken task READY, push it onto its PINNED CPU's run queue, and poke that CPU
/// (waking it or preempting a lower-priority task). Used by the sleeper drain (same CPU) and
/// `Semaphore::post` (cross-CPU wake). The task always returns to `task.cpu`, so its GS base stays
/// correct on resume (tasks don't migrate). Caller runs with IF=0.
fn make_ready(task: Box<Task>) {
    let target = task.cpu as usize;
    let prio = task.priority;
    debug_assert!(target < MAX_CPUS, "make_ready: cpu out of range");
    task.state.store(STATE_READY, Ordering::Release);
    RUN_QUEUES[target].lock().push(task);
    poke_for(target, prio);
}

/// Cooperatively give up the CPU. The current task is marked ready and rotated to the back of its
/// run queue (by the scheduler), and another runnable task runs. Returns when this task is later
/// rescheduled. No-op if called outside a scheduled task (e.g. on the BSP main loop).
pub fn yield_now() {
    // Critical section with IF=0: nothing may preempt us between marking ready and switching.
    // Save the caller's IF and restore exactly that on exit (don't blindly re-enable — a caller
    // could be in its own interrupts-off region).
    let was_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    if !raw.is_null() {
        unsafe {
            debug_assert_eq!((*raw).cpu as usize, cpu, "task ran on the wrong CPU");
            (*raw).state.store(STATE_READY, Ordering::Release);
            // Switch back to the scheduler; it requeues us and runs the next task. We resume here
            // (IF=0, carried by popfq) when rescheduled. `was_enabled` lives on our stack and
            // survives the switch.
            switch_context(
                &raw mut (*raw).ctx_rsp,
                SCHED[cpu].scheduler_rsp.load(Ordering::Acquire),
            );
        }
    }
    if was_enabled {
        x86_64::instructions::interrupts::enable();
    }
}

/// Terminate the current task. Marks it finished and switches to the scheduler, which frees its
/// stack. Never returns. Called automatically when a task's entry function returns.
pub fn exit() -> ! {
    // IF=0 first (M11): no timer/IPI may observe this half-dead task between the state flip and
    // the switch.
    x86_64::instructions::interrupts::disable();
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    assert!(!raw.is_null(), "exit: no current task");
    unsafe {
        (*raw).state.store(STATE_FINISHED, Ordering::Release);
        // Switch away for good. `old_rsp` is a throwaway slot on the dying stack (the scheduler
        // never reads it back, and never switches into a Finished task).
        let mut discard: u64 = 0;
        switch_context(&raw mut discard, SCHED[cpu].scheduler_rsp.load(Ordering::Acquire));
    }
    unreachable!("scheduler resumed a finished task")
}

/// Block the current task for `ticks` of THIS CPU's local-APIC timer, then become runnable again.
/// Timer-driven (no waker), so it cannot lose a wakeup: the scheduler drains due sleepers at its
/// loop top, and the free-running periodic timer re-enters that loop every tick (granularity and
/// worst-case wake latency are one tick). `ticks == 0` wakes on the next loop pass (~0–1 tick).
/// No-op outside a scheduled task (e.g. the unscheduled BSP), like `yield_now`.
pub fn sleep_ticks(ticks: u64) {
    let was_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    if !raw.is_null() {
        let deadline = percpu::this_cpu().ticks.load(Ordering::Relaxed) + ticks;
        unsafe {
            debug_assert_eq!((*raw).cpu as usize, cpu, "task ran on the wrong CPU");
            (*raw).state.store(STATE_BLOCKED, Ordering::Release);
            // Tell the scheduler to park us on this CPU's sleeper list with this deadline.
            SCHED[cpu].park_deadline.store(deadline, Ordering::Relaxed);
            SCHED[cpu].park_kind.store(PARK_SLEEP, Ordering::Relaxed);
            switch_context(
                &raw mut (*raw).ctx_rsp,
                SCHED[cpu].scheduler_rsp.load(Ordering::Acquire),
            );
        }
        // Resumed (IF=0, carried) once the deadline passed and the scheduler re-dispatched us.
    }
    if was_enabled {
        x86_64::instructions::interrupts::enable();
    }
}

// ---------------------------------------------------------------------------------------------
// Semaphore — the inter-thread blocking primitive (counting; FIFO waiters)
// ---------------------------------------------------------------------------------------------

/// A counting semaphore for kernel threads. `wait()` blocks when the count is zero; `post()` wakes
/// one waiter (or bumps the count). Waking is cross-CPU aware: a task blocked on CPU B is woken
/// from CPU A by moving it to B's run queue and sending the reschedule IPI.
///
/// MUST be `'static` (e.g. a `static SEM`): `wait()` hands raw pointers to `waiters`/`locked` to
/// the scheduler to be dereferenced after the context switch, so the Semaphore must outlive every
/// task that can block on it. Dropping one with parked waiters would leak those tasks and dangle.
///
/// Soundness of the `UnsafeCell<VecDeque>` + `unsafe impl Sync`: EVERY access to `waiters` (and the
/// count) is gated by the `locked` spinlock, and the park-side push is performed by the scheduler
/// while the blocker's lock is still held — before the scheduler releases it — which establishes
/// happens-before with the next `post()` that acquires the lock. The lock-handoff (hold `locked`
/// across the switch into the scheduler; the scheduler pushes the Box then releases it) is what
/// makes the wakeup lost-proof: a `post()` on another CPU spins on `locked` and so cannot observe
/// the waiter list until the blocked Box is in it.
pub struct Semaphore {
    /// Raw spinlock guarding `count` and `waiters`. Acquire on lock, Release on unlock.
    locked: AtomicBool,
    /// Permit count; touched only under `locked` (Relaxed — the lock provides ordering). Always >= 0.
    count: AtomicI64,
    /// FIFO waiter list; touched only under `locked`. Pre-reserved to `WAIT_CAPACITY` by `init()`.
    waiters: UnsafeCell<VecDeque<Box<Task>>>,
}

// SAFETY: every access to the interior `waiters` is serialized by the `locked` spinlock; see the
// type's doc comment for the full happens-before argument.
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
    /// permit is held. Returns `false` WITHOUT acquiring if called off a scheduled task (the
    /// unscheduled BSP / idle context) — there is no `current` to block, so it cannot wait. A
    /// caller that issues a resource on success (e.g. `Mutex::lock`) MUST check the return value:
    /// a `false` means NO permit was taken, so no resource may be handed out.
    #[must_use]
    pub fn wait(&self) -> bool {
        let was_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable(); // IF=0 for the whole critical section
        self.lock_raw();

        if self.count.load(Ordering::Relaxed) > 0 {
            self.count.fetch_sub(1, Ordering::Relaxed);
            self.unlock_raw();
            if was_enabled {
                x86_64::instructions::interrupts::enable();
            }
            return true; // acquired a permit on the fast path
        }

        // No permit: block. Only a scheduled task can park; on the BSP/idle context just bail (the
        // lock-handoff requires a real `current` to switch away from) — and crucially WITHOUT
        // acquiring, so the count stays consistent (no permit was taken).
        let cpu = percpu::this_cpu().cpu_index as usize;
        let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
        if raw.is_null() {
            self.unlock_raw();
            if was_enabled {
                x86_64::instructions::interrupts::enable();
            }
            return false; // off a scheduled task: did NOT acquire
        }

        // Capacity guard: the lock is held continuously until the scheduler pushes, so the length
        // cannot change before then; asserting it here proves the park-side push won't reallocate.
        assert!(
            unsafe { (*self.waiters.get()).len() } < WAIT_CAPACITY,
            "Semaphore waiter overflow (raise WAIT_CAPACITY)"
        );

        unsafe {
            (*raw).state.store(STATE_BLOCKED, Ordering::Release);
            // Tell the scheduler to push us into THIS semaphore's waiter list and release THIS
            // semaphore's lock after the push (the lock-handoff). We keep `locked` held across the
            // switch; the scheduler releases it.
            SCHED[cpu].park_waiters.store(self.waiters.get() as u64, Ordering::Relaxed);
            SCHED[cpu].park_lock.store(&self.locked as *const AtomicBool as u64, Ordering::Relaxed);
            SCHED[cpu].park_kind.store(PARK_WAITQ, Ordering::Relaxed);
            switch_context(
                &raw mut (*raw).ctx_rsp,
                SCHED[cpu].scheduler_rsp.load(Ordering::Acquire),
            );
        }
        // Resumed (IF=0, carried) once `post()` moved us back to our run queue. The lock was
        // already released by the scheduler that parked us — we must not touch it here. We were
        // woken by a `post()` that handed us the permit (it did NOT increment the count), so we
        // now hold one.
        if was_enabled {
            x86_64::instructions::interrupts::enable();
        }
        true
    }

    /// Release a permit: wake one FIFO waiter if any, else increment the count. Wakes across CPUs.
    pub fn post(&self) {
        let was_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
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
        if was_enabled {
            x86_64::instructions::interrupts::enable();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Mutex<T> — a sleeping mutual-exclusion lock (a binary semaphore guarding owned data)
// ---------------------------------------------------------------------------------------------

/// A sleeping mutex: `lock()` BLOCKS the calling task (it does not spin) until the lock is free,
/// then hands back exclusive access to the protected data through an RAII guard that unlocks on
/// drop. Built on `Semaphore` with a single permit, so it inherits the lost-wakeup-safe, cross-CPU
/// block/wake — and, unlike a spinlock, a task may safely hold it across preemption and yields.
///
/// Like `Semaphore`, a Mutex that tasks block on must be `'static` (the underlying semaphore hands
/// raw pointers to its internals across the context switch). Call `init()` once before use.
///
/// NOT re-entrant: a task that locks the same Mutex twice deadlocks (it blocks waiting on itself).
pub struct Mutex<T> {
    sem: Semaphore,
    data: UnsafeCell<T>,
}

// SAFETY: the single semaphore permit guarantees at most one task holds the guard — hence at most
// one live `&mut T` — at a time, across all CPUs. `T: Send` because the data is accessed from
// whichever CPU currently holds the lock (which varies over time).
unsafe impl<T: Send> Sync for Mutex<T> {}
unsafe impl<T: Send> Send for Mutex<T> {}

impl<T> Mutex<T> {
    /// Construct an unlocked mutex (one permit). `const` so it can initialise a `static`.
    pub const fn new(value: T) -> Self {
        Mutex {
            sem: Semaphore::new(1),
            data: UnsafeCell::new(value),
        }
    }

    /// Reserve the underlying semaphore's waiter capacity. Call once on the BSP before use.
    pub fn init(&self) {
        self.sem.init();
    }

    /// Acquire the lock, blocking the current task until it is free. Returns a guard that unlocks
    /// on drop.
    ///
    /// MUST be called from a scheduled task. A guard may be issued ONLY when a real permit was
    /// taken — otherwise two callers could hold guards at once (aliased `&mut T` = UB). Off a
    /// scheduled task `wait()` cannot block and so does not acquire, so we panic rather than hand
    /// out an unbacked guard. (A sleeping mutex is meaningless off a scheduler context anyway —
    /// the unscheduled BSP must not block.)
    pub fn lock(&self) -> MutexGuard<'_, T> {
        assert!(
            self.sem.wait(),
            "Mutex::lock() called off a scheduled task (a sleeping mutex needs a scheduler context)"
        );
        MutexGuard { mutex: self, _not_send: PhantomData }
    }
}

/// RAII guard returned by `Mutex::lock`: dereferences to the protected data and releases the lock
/// (`sem.post()`) when dropped.
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
    /// Make the guard `!Send`: the lock is owned by the task that took it, so a held guard must not
    /// be moved to another task/CPU (which would unlock from the wrong context).
    _not_send: PhantomData<*const ()>,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: we hold the only permit, so no other task aliases the data.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: we hold the only permit, so this is the only live reference to the data.
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.sem.post();
    }
}

// ---------------------------------------------------------------------------------------------
// Condvar — a Mesa-semantics condition variable (the count-less companion to Semaphore)
// ---------------------------------------------------------------------------------------------

/// A condition variable for kernel threads, paired with the sleeping `Mutex<T>`. `wait(guard)`
/// atomically releases the mutex and blocks the calling task; `notify_one`/`notify_all` wake parked
/// waiter(s). It is the count-LESS sibling of `Semaphore`: a notification with no waiter is a no-op
/// (it is NOT stored), which is the defining condition-variable semantics.
///
/// Callers MUST re-check their predicate in a `while` loop. Wakeups are permitted to be spurious,
/// and — unlike `sleep_ticks` — a cv waiter has NO timer backstop: it sits only in this queue with
/// no deadline, so a `notify` is its SOLE wake source and a missed notify is a permanent hang, not
/// a one-tick latency blip. That raises the stakes on the lost-wakeup proof below; it is the same
/// lock-handoff that makes `Semaphore` safe.
///
/// MUST be `'static` (e.g. a `static CV`): like `Semaphore`, `wait()` hands raw pointers to
/// `waiters`/`locked` to the scheduler to be dereferenced after the context switch, so the Condvar
/// must outlive every task that can block on it. Call `init()` once on the BSP before use.
///
/// Lost-wakeup safety (Mesa semantics). A correct notifier changes the predicate under the mutex,
/// then notifies. `wait()` acquires the condvar's `locked` BEFORE releasing the mutex, and keeps it
/// held — handed off to the scheduler, released only after the blocked Box is enqueued — across the
/// switch. A notifier must take `locked` to pop a waiter, so it cannot run a notify between the
/// mutex-release and the enqueue; it always observes the waiter. This is exactly `Semaphore`'s
/// lock-handoff with the protected resource (the mutex) released explicitly inside the handoff.
pub struct Condvar {
    /// Raw spinlock guarding `waiters`. Acquire on lock, Release on unlock. Same role as
    /// `Semaphore::locked`; there is no count (notifications are not stored).
    locked: AtomicBool,
    /// FIFO waiter list; touched only under `locked`. Pre-reserved to `WAIT_CAPACITY` by `init()`
    /// so the scheduler's park-side `push_back` never reallocates under the held lock.
    waiters: UnsafeCell<VecDeque<Box<Task>>>,
}

// SAFETY: every access to `waiters` is serialized by `locked`; the park-side push happens while the
// blocker's lock is still held (released by the scheduler after the push), establishing
// happens-before with the next notify — identical to `Semaphore`.
unsafe impl Sync for Condvar {}

impl Condvar {
    /// Construct an empty condition variable. `const` so it can initialise a `static`.
    pub const fn new() -> Self {
        Condvar {
            locked: AtomicBool::new(false),
            waiters: UnsafeCell::new(VecDeque::new()),
        }
    }

    /// Reserve the waiter list's capacity so the scheduler's park-side push never reallocates under
    /// the held lock. Call once on the BSP before any task can block on this condvar.
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

    /// Atomically release `guard`'s mutex and block the current task on this condvar; when later
    /// notified, re-acquire the mutex and return a fresh guard. MUST be called from a scheduled
    /// task holding `guard`. Spurious wakeups are allowed — the caller must re-test its predicate.
    ///
    /// The lettered step order below is load-bearing for lost-wakeup safety and the lock-ordering
    /// invariant; do not reorder it (see the type doc and the module header).
    pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        // (a) Mask interrupts for the whole critical section; remember the caller's IF to restore.
        let was_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();

        // (b) Must be on a scheduled task. Assert BEFORE consuming the guard: were we to forget the
        //     guard first and then panic, the mutex would be permanently locked — the explicit
        //     release in (h) never runs, and the guard's Drop is already gone.
        let cpu = percpu::this_cpu().cpu_index as usize;
        let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
        assert!(
            !raw.is_null(),
            "Condvar::wait() called off a scheduled task (needs a scheduler context)"
        );

        // (c) Extract the mutex and DISARM the guard's Drop. `forget` is the deliberate dual of the
        //     explicit `mutex.sem.post()` in (h): exactly one release per acquire, mirroring the
        //     mutex's single-permit invariant.
        let mutex = guard.mutex;
        core::mem::forget(guard);

        // (d) Acquire the condvar lock BEFORE releasing the mutex — closing the lost-wakeup window.
        //     A notifier that flips the predicate after our release still cannot notify an empty
        //     queue, because notify needs this lock, which we now hold across the switch.
        self.lock_raw();

        // (e) Prove the park-side push stays allocation-free: the lock is held continuously through
        //     the switch, so this length cannot change before park_blocked pushes.
        assert!(
            unsafe { (*self.waiters.get()).len() } < WAIT_CAPACITY,
            "Condvar waiter overflow (raise WAIT_CAPACITY)"
        );

        unsafe {
            // (f) Block, and (g) install the PARK_WAITQ hand-off onto THIS condvar's queue + lock.
            (*raw).state.store(STATE_BLOCKED, Ordering::Release);
            SCHED[cpu].park_waiters.store(self.waiters.get() as u64, Ordering::Relaxed);
            SCHED[cpu].park_lock.store(&self.locked as *const AtomicBool as u64, Ordering::Relaxed);
            SCHED[cpu].park_kind.store(PARK_WAITQ, Ordering::Relaxed);

            // (h) Release the user mutex while still holding the condvar lock. This RAW
            //     `Semaphore::post`, called with IF=0, sees `was_enabled == false` and does NOT
            //     re-enable interrupts — preserving "every switch-away is IF=0" into (j). It is the
            //     one sanctioned post-under-another-primitive-lock (cv.locked -> mutex.sem.locked);
            //     see the module header.
            mutex.sem.post();
            debug_assert!(
                !x86_64::instructions::interrupts::are_enabled(),
                "Condvar::wait must switch with interrupts disabled"
            );

            // (j) Switch to the scheduler; park_blocked(PARK_WAITQ) pushes our Box into
            //     self.waiters and releases self.locked LAST (the lock-handoff).
            switch_context(
                &raw mut (*raw).ctx_rsp,
                SCHED[cpu].scheduler_rsp.load(Ordering::Acquire),
            );
        }

        // Resumed (IF=0, carried) by a notify that moved us back to our pinned run queue; the
        // condvar lock was already released by the scheduler that parked us. Restore the caller's
        // IF, then re-acquire the mutex and return a fresh guard. We restore IF ourselves rather
        // than leaning on the inner `lock()`: its `Semaphore::wait` snapshots the CURRENT IF — the
        // carried 0, not the caller's original — so it alone would strand the task interrupts-off.
        // The re-acquire may legitimately block again on a contended mutex via the mutex's own
        // disjoint PARK_WAITQ handoff; the task stays CPU-pinned throughout, so rebuilding the
        // `!Send` guard on the same CPU upholds its unlock-from-owner-context intent.
        if was_enabled {
            x86_64::instructions::interrupts::enable();
        }
        mutex.lock()
    }

    /// Wake one waiter if any; a no-op if none are waiting (the notification is NOT stored — the
    /// defining difference from `Semaphore::post`). May be called from any context (a task or the
    /// unscheduled BSP). `make_ready` is called only AFTER releasing the condvar lock, so the lock
    /// is never nested over a run-queue lock.
    pub fn notify_one(&self) {
        let was_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
        self.lock_raw();
        let waiter = unsafe { (*self.waiters.get()).pop_front() };
        self.unlock_raw();
        if let Some(task) = waiter {
            make_ready(task);
        }
        if was_enabled {
            x86_64::instructions::interrupts::enable();
        }
    }

    /// Wake EVERY currently-queued waiter. Drains one waiter per lock acquisition and calls
    /// `make_ready` outside the lock (the `Semaphore::post` discipline), so the condvar lock is
    /// never held across a run-queue lock. May be called from any context. A waiter that arrives
    /// mid-drain may also be woken — harmless under Mesa semantics (the caller re-tests its
    /// predicate); a correct notifier holds the mutex across the notify, so in practice no new
    /// waiter arrives during the drain, and each iteration removes exactly one Box (no livelock).
    pub fn notify_all(&self) {
        let was_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
        loop {
            self.lock_raw();
            let waiter = unsafe { (*self.waiters.get()).pop_front() };
            self.unlock_raw();
            match waiter {
                Some(task) => make_ready(task),
                None => break,
            }
        }
        if was_enabled {
            x86_64::instructions::interrupts::enable();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Channel<T> — a bounded blocking channel, composed from a Mutex + two Semaphores
// ---------------------------------------------------------------------------------------------

/// A fixed-capacity blocking channel (the classic slots/items bounded buffer). `send` blocks while
/// full, `recv` blocks while empty; both can cross-CPU-wake the other side. Multiple producers and
/// multiple consumers are safe — the semaphores and the buffer mutex serialise all of them — though
/// the demo only drives one of each. Demonstrates that the scheduler's `Mutex` and `Semaphore`
/// compose into higher-level concurrency without new unsafe:
/// `slots` counts free slots (send waits on it), `items` counts buffered values (recv waits on it),
/// and a `Mutex` serialises the buffer itself. Each `wait()` corresponds to a real produced/consumed
/// item, so the buffer push/pop is never starved or raced (standard bounded-buffer invariant).
///
/// Like the primitives it is built from, a Channel that tasks block on must be `'static`; call
/// `init()` once on the BSP before use. `Channel<T>` is `Sync` (auto-derived) exactly when `T: Send`.
pub struct Channel<T> {
    /// Free slots; initial permits = capacity. `send` waits (blocks when full).
    slots: Semaphore,
    /// Buffered items; initial 0. `recv` waits (blocks when empty).
    items: Semaphore,
    /// The buffer, serialised by a sleeping mutex (held only across a single push/pop).
    buffer: Mutex<VecDeque<T>>,
}

impl<T> Channel<T> {
    /// Construct an empty channel with `capacity` slots. `const` so it can initialise a `static`.
    pub const fn new(capacity: usize) -> Self {
        Channel {
            slots: Semaphore::new(capacity as i64),
            items: Semaphore::new(0),
            buffer: Mutex::new(VecDeque::new()),
        }
    }

    /// Reserve the underlying primitives' waiter capacity. Call once on the BSP before use. (Does
    /// not lock the buffer — `Mutex::lock` requires a scheduler context, which the BSP is not.)
    pub fn init(&self) {
        self.slots.init();
        self.items.init();
        self.buffer.init();
    }

    /// Send a value, blocking while the channel is full. Must be called from a scheduled task.
    pub fn send(&self, value: T) {
        assert!(self.slots.wait(), "Channel::send() called off a scheduled task");
        self.buffer.lock().push_back(value); // mutex held only across the push (never across a wait)
        self.items.post();
    }

    /// Receive a value, blocking while the channel is empty. Must be called from a scheduled task.
    pub fn recv(&self) -> T {
        assert!(self.items.wait(), "Channel::recv() called off a scheduled task");
        // A permit from `items` guarantees a value was pushed before its matching `post`, and pops
        // are serialised by the mutex, so the buffer is non-empty here.
        let value = self.buffer.lock().pop_front().expect("channel buffer empty after items.wait");
        self.slots.post();
        value
    }
}

// ---------------------------------------------------------------------------------------------
// Preemption hook (called from the timer interrupt) — the single involuntary switch site
// ---------------------------------------------------------------------------------------------

/// Called from the local-APIC timer handler on EVERY tick, on every CPU, AFTER the EOI. Decrements
/// the running task's quantum and, when it expires (or a reschedule was requested), preempts: the
/// task is marked ready and control switches to this CPU's scheduler. Runs in interrupt context
/// with IF=0; the task's own `iretq` (much later, when it is rescheduled) restores its IF=1.
///
/// No-op unless scheduling is active AND a task is actually running on this CPU — so it does
/// nothing on the BSP (never scheduled) and nothing during the pre-scheduler smoke test.
pub fn timer_preempt() {
    if !SCHED_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    if raw.is_null() {
        return; // scheduler/idle context, or an unscheduled CPU (BSP)
    }

    // Tick down the quantum; arm the reschedule signal when it runs out.
    let remaining = SCHED[cpu].quantum.load(Ordering::Relaxed);
    if remaining > 0 {
        SCHED[cpu].quantum.store(remaining - 1, Ordering::Relaxed);
    }
    if remaining <= 1 {
        SCHED[cpu].need_resched.store(true, Ordering::Relaxed);
    }
    if !SCHED[cpu].need_resched.load(Ordering::Relaxed) {
        return;
    }

    // Preempt. We are already IF=0 (interrupt gate) and hold no lock.
    unsafe {
        (*raw).state.store(STATE_READY, Ordering::Release);
        switch_context(&raw mut (*raw).ctx_rsp, SCHED[cpu].scheduler_rsp.load(Ordering::Acquire));
    }
    // Resumed (IF=0, carried). Falls back into the timer handler, which `iretq`s the task.
}

// ---------------------------------------------------------------------------------------------
// Per-CPU scheduler loop
// ---------------------------------------------------------------------------------------------

/// AP entry into scheduling. Waits until the BSP turns scheduling on (after SMP verification),
/// then runs the per-CPU scheduler loop forever. Called from `ap_entry` in place of the old idle
/// `hlt_loop`. Interrupts are already enabled by the caller, so the periodic timer wakes the
/// `hlt` below to re-check the release flag.
pub fn wait_and_run() -> ! {
    while !SCHED_GO.load(Ordering::Acquire) {
        x86_64::instructions::hlt();
    }
    run()
}

/// The per-CPU scheduler/idle loop. Runs on the CPU's original stack, which becomes its
/// "scheduler context". Never returns. Pops a task, switches into it, and — when it switches back
/// (yield / preempt / exit) — requeues or frees it, then repeats; idles in an atomic `sti; hlt`
/// when the queue is empty.
fn run() -> ! {
    let cpu = percpu::this_cpu().cpu_index as usize;
    // Aging sweep cadence, kept as a stack local (run() is `-> !`, entered once per CPU, so this
    // survives the CPU's whole lifetime — no SchedCpu field needed). Seed from the LIVE tick count:
    // by now the AP burned ticks in `wait_and_run`'s hlt loop / the BSP during verify_smp, so a 0
    // seed would make the first `elapsed` huge and promote everything on the first sweep.
    let mut last_age = percpu::this_cpu().ticks.load(Ordering::Relaxed);
    loop {
        // From here through the switch we keep IF=0 so no handler can re-enter the scheduler on
        // its own stack or be caught mid-requeue holding the run-queue lock.
        x86_64::instructions::interrupts::disable();
        // ABI-clean DF regardless of what the last switch inherited.
        unsafe { core::arch::asm!("cld", options(nomem, nostack, preserves_flags)) };

        // Wake any sleepers whose deadline has passed. The wake source is the free-running
        // periodic LVT timer (every tick breaks the idle `hlt` below and re-enters this loop), so
        // an idle CPU with only a pending sleeper makes progress; worst-case wake latency is one
        // tick. Same CPU, no IPI — `make_ready` pushes onto this CPU's own run queue.
        drain_due_sleepers(cpu);

        // Age then pick, under ONE run-queue lock acquisition: run the anti-starvation sweep (gated
        // to ~every AGING_INTERVAL ticks) AFTER the sleeper drain (so freshly-woken sleepers, pushed
        // at base with a fresh clock, are visible to it) and BEFORE the pop, so a task cannot be
        // dispatched before it is aged in the same pass.
        let next = {
            let mut q = RUN_QUEUES[cpu].lock();
            let now = percpu::this_cpu().ticks.load(Ordering::Relaxed);
            let elapsed = now.wrapping_sub(last_age);
            if elapsed >= AGING_INTERVAL {
                // Saturate the cast so even an absurd (>2^32-tick) inter-sweep gap loses no aging
                // credit (`age` carries surplus past AGE_TICKS forward). In steady state `elapsed`
                // ~= AGING_INTERVAL, so this min is a no-op.
                q.age(elapsed.min(u32::MAX as u64) as u32);
                last_age = now;
            }
            q.pop_highest() // highest-priority ready task; lock dropped here
        };
        match next {
            Some(task) => {
                task.state.store(STATE_RUNNING, Ordering::Release);
                // Fresh quantum + clear the reschedule signal, and publish the running priority so a
                // remote waker/spawner can tell whether a newly-ready task should preempt us.
                SCHED[cpu].quantum.store(QUANTUM_TICKS, Ordering::Relaxed);
                SCHED[cpu].need_resched.store(false, Ordering::Relaxed);
                SCHED[cpu].current_prio.store(task.priority, Ordering::Release);

                let raw = Box::into_raw(task);
                let entry_rsp = unsafe { (*raw).ctx_rsp };
                // Publish `current` (Release) strictly before switching in (the trampoline /
                // handlers read it Acquire).
                SCHED[cpu].current.store(raw as u64, Ordering::Release);

                unsafe {
                    switch_context(SCHED[cpu].scheduler_rsp.as_ptr(), entry_rsp);
                }

                // --- The task switched back to us (yield / preempt / block / exit). IF=0. ---
                SCHED[cpu].current.store(0, Ordering::Release);
                SCHED[cpu].current_prio.store(PRIO_IDLE, Ordering::Release);
                // Consume the park action exactly once: read it and immediately reset to NONE, so a
                // stale action can never leak into the next task's switch-back. Only a task that
                // switched back BLOCKED has a meaningful action.
                let park = SCHED[cpu].park_kind.swap(PARK_NONE, Ordering::Relaxed);
                let task = unsafe { Box::from_raw(raw) };
                match task.state.load(Ordering::Acquire) {
                    STATE_FINISHED => drop(task), // frees the stack
                    STATE_BLOCKED => park_blocked(cpu, park, task),
                    _ => {
                        // READY (yielded or preempted): rotate to the back of its priority level.
                        debug_assert_eq!(park, PARK_NONE, "non-blocked task carried a park action");
                        task.state.store(STATE_READY, Ordering::Release);
                        RUN_QUEUES[cpu].lock().push(task);
                    }
                }
            }
            None => {
                // Nothing to run: sleep until an interrupt (timer or a `spawn`/wake IPI). The `sti;
                // hlt` pair is atomic — an interrupt latched before the `sti` still fires and
                // returns past the `hlt`, so a wake that arrived in the empty-check window is not
                // lost. On wake we loop back to the top (which `cli`s) and re-check the queue +
                // sleepers.
                x86_64::instructions::interrupts::enable_and_hlt();
            }
        }
    }
}

/// Park a task that switched back in the BLOCKED state, per the action it set before switching.
/// Runs in the scheduler context with IF=0 (so it cannot be preempted) and owns `task`.
fn park_blocked(cpu: usize, park: u8, task: Box<Task>) {
    match park {
        PARK_WAITQ => {
            // Lock-handoff: the blocking task acquired the wait queue's lock and held it across the
            // switch; WE push its Box into the waiter list and then release that lock — strictly in
            // that order. Releasing only AFTER the push is what makes the wakeup lost-proof: a
            // `post()` on another CPU spins on the lock and so cannot observe the queue until the
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
            // A BLOCKED task with no valid park action is a bug; don't leak it — drop it.
            debug_assert!(false, "BLOCKED task with no park action");
            drop(task);
        }
    }
}

/// Move every sleeper on this CPU whose deadline has passed back onto the run queue. Called at the
/// scheduler loop top with IF=0. The sleeper lock is released before `make_ready` so we never nest
/// it under the run-queue lock.
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

// ---------------------------------------------------------------------------------------------
// Bring-up + introspection
// ---------------------------------------------------------------------------------------------

/// Touch the run queues so the lazy_static is initialised on the BSP (each level already
/// pre-reserved by `RunQueue::with_capacity`) before any AP can reach them. Call once on the BSP
/// after the heap is up and after SMP verification.
pub fn init() {
    for q in RUN_QUEUES.iter() {
        let _ = q.lock().len();
    }
}

/// Number of tasks currently queued on a CPU (best-effort snapshot; for the `sched` shell command).
pub fn run_queue_len(cpu: usize) -> usize {
    if cpu >= MAX_CPUS {
        return 0;
    }
    RUN_QUEUES[cpu].lock().len()
}

/// Id of the task currently running on a CPU, if any (best-effort; for introspection only).
pub fn current_task_id(cpu: usize) -> Option<u64> {
    if cpu >= MAX_CPUS {
        return None;
    }
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *const Task;
    if raw.is_null() {
        None
    } else {
        Some(unsafe { (*raw).id })
    }
}

// ---------------------------------------------------------------------------------------------
// Demo workload (proves the scheduler end to end; spawned by the BSP after verification)
// ---------------------------------------------------------------------------------------------

/// Count of demo tasks that have run to completion (so a test can confirm exit/free works).
static DEMO_DONE: AtomicUsize = AtomicUsize::new(0);

// Priority-aging demo state. A continuously-ready base-HIGH "hog" and a base-LOW "victim" share one
// AP; under strict priority the victim would never run until the hog finished, but aging promotes
// it so it completes WHILE the hog is still running. Release/Acquire so the cross-CPU verifier's
// reads are well-defined.
/// Set true when the hog finishes (it is sized NOT to, before the victim does).
static AGE_HIGH_DONE: AtomicBool = AtomicBool::new(false);
/// Set true when the victim finishes its rounds (the anti-starvation signal for the verifier).
static AGE_LOW_DONE: AtomicBool = AtomicBool::new(false);
/// Captured by the victim at completion: was the hog STILL running (i.e. did aging let the victim
/// through under continuous HIGH load)? This is the actual PASS condition.
static AGE_LOW_RAN_UNDER_HIGH: AtomicBool = AtomicBool::new(false);
/// Victim work: small, so a working ager finishes it quickly. The victim runs ~1 quantum per
/// ~2*AGE_TICKS of starvation (~1/9 duty under the HIGH hog). Hog work is sized so that, in the
/// wall-clock the victim needs to finish, the hog is only a fraction done — so AGE_HIGH_DONE is
/// still false when the victim reads it. Invariant to preserve on any retune:
///   AGE_HOG_ROUNDS*AGE_HOG_ITERS  >>  AGE_VICTIM_ROUNDS*AGE_VICTIM_ITERS * (2*AGE_TICKS/QUANTUM_TICKS)
/// (the ratio here is ~12x). Host-speed-dependent like `demo_busy`'s 40M loop.
const AGE_VICTIM_ROUNDS: u32 = 4;
const AGE_VICTIM_ITERS: u64 = 15_000_000;
const AGE_HOG_ROUNDS: u32 = 120;
const AGE_HOG_ITERS: u64 = 50_000_000;

/// Turn scheduling on and spawn the demo workload across the online APs: equal-priority round-robin
/// + timer preemption (the busy pair on the first AP), and the priority-AGING showcase — a
/// continuously-ready base-HIGH hog and a base-LOW victim on a second AP, where strict priority
/// alone would starve the victim forever but aging promotes it so it finishes under load; a verifier
/// on a third AP prints a self-checking PASS/FAIL. Called once on the BSP after `smp::start_aps`
/// (hence after `verify_smp`). No-op with no APs; the aging showcase needs >=3 APs (else SKIPPED).
pub fn start_demo(online_aps: &[usize]) {
    if online_aps.is_empty() {
        serial_println!("SCHED: no application processors online; scheduler idle.");
        return;
    }

    SCHED_ACTIVE.store(true, Ordering::Release);
    SCHED_GO.store(true, Ordering::Release);

    serial_println!(
        "SCHED: scheduling enabled on {} AP(s) {:?}; spawning demo threads...",
        online_aps.len(),
        online_aps
    );

    // Equal-priority round-robin + preemption regression: two non-yielding NORMAL threads on the
    // first AP must INTERLEAVE (without preemption the first would monopolise the core until exit).
    let cpu_busy = online_aps[0];
    spawn("busy-A", demo_busy, encode(cpu_busy, b'A'), cpu_busy, PRIO_NORMAL);
    spawn("busy-B", demo_busy, encode(cpu_busy, b'B'), cpu_busy, PRIO_NORMAL);

    // Priority aging (anti-starvation): a continuously-ready base-HIGH hog and a base-LOW victim on
    // a SECOND AP. Under strict priority the victim never runs until the hog finishes; aging lifts
    // its effective level until it reaches parity and runs, so it COMPLETES while the hog is still
    // going. The verifier runs on a THIRD AP — never co-located with the hog, which (being HIGH and
    // continuously ready) would starve a NORMAL verifier and cause a false FAIL. Needs >=3 distinct
    // APs; with fewer, the showcase is SKIPPED (a definitive non-FAIL the harness can tell apart).
    if online_aps.len() >= 3 {
        let cpu_age = online_aps[1];
        spawn("age-hog", demo_age_hog, cpu_age, cpu_age, PRIO_HIGH);
        spawn("age-victim", demo_age_victim, cpu_age, cpu_age, PRIO_LOW);
        let cpu_verify = online_aps[2];
        spawn("age-verify", demo_age_verifier, cpu_verify, cpu_verify, PRIO_NORMAL);
    } else {
        serial_println!("PRIOAGE: SKIPPED (needs >=3 APs; have {})", online_aps.len());
    }
}

/// Pack a (cpu, tag-letter) pair into the single `usize` arg the task entry receives.
fn encode(cpu: usize, tag: u8) -> usize {
    (cpu << 8) | tag as usize
}
fn decode(arg: usize) -> (usize, char) {
    ((arg >> 8) & 0xFF, (arg & 0xFF) as u8 as char)
}

/// CPU-bound thread that never yields. It is preempted by the timer; the interleaving of two of
/// these on one CPU is the visible proof that preemption works.
fn demo_busy(arg: usize) {
    let (cpu, tag) = decode(arg);
    for round in 0..6u32 {
        serial_println!("SCHED: [cpu{cpu} busy-{tag}] round {round}");
        // Burn enough cycles to span several quanta so a preemption lands mid-thread.
        let mut acc: u64 = 0;
        for i in 0..40_000_000u64 {
            acc = acc.wrapping_add(i ^ round as u64);
        }
        core::hint::black_box(acc);
    }
    serial_println!("SCHED: [cpu{cpu} busy-{tag}] done");
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

/// Aging HOG: a continuously-ready base-HIGH CPU burner. Under strict priority it would monopolise
/// its core until done; it is sized NOT to finish before the victim, so the victim's completion
/// provably happens under continuous HIGH load. Never yields or blocks.
fn demo_age_hog(cpu: usize) {
    for round in 0..AGE_HOG_ROUNDS {
        let mut acc: u64 = 0;
        for i in 0..AGE_HOG_ITERS {
            acc = acc.wrapping_add(i ^ round as u64);
        }
        core::hint::black_box(acc);
        if round % 20 == 0 {
            serial_println!("SCHED: [cpu{} age-hog] HIGH round {}/{}", cpu, round, AGE_HOG_ROUNDS);
        }
    }
    AGE_HIGH_DONE.store(true, Ordering::Release);
    serial_println!("SCHED: [cpu{} age-hog] done (expected AFTER the victim)", cpu);
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

/// Aging VICTIM: a base-LOW CPU task on the SAME AP as the hog. Without aging it never runs; with
/// aging it climbs to parity and completes. On finishing it captures whether the hog was still
/// running (the anti-starvation proof) BEFORE signalling, then signals the verifier.
fn demo_age_victim(cpu: usize) {
    for round in 0..AGE_VICTIM_ROUNDS {
        let mut acc: u64 = 0;
        for i in 0..AGE_VICTIM_ITERS {
            acc = acc.wrapping_add(i ^ round as u64);
        }
        core::hint::black_box(acc);
        serial_println!(
            "SCHED: [cpu{} age-victim] LOW round {}/{} ran under continuous HIGH load",
            cpu,
            round + 1,
            AGE_VICTIM_ROUNDS
        );
    }
    // Capture the proof BEFORE signalling done: the hog must still be running for this to be aging.
    AGE_LOW_RAN_UNDER_HIGH.store(!AGE_HIGH_DONE.load(Ordering::Acquire), Ordering::Release);
    AGE_LOW_DONE.store(true, Ordering::Release);
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

/// Aging VERIFIER on a DIFFERENT AP from the hog: poll until the victim finishes or a generous
/// timeout, then print one self-checking PASS/FAIL line. PASS iff the victim completed AND the hog
/// was still running when it did (= aging beat strict-priority starvation). A broken ager starves
/// the victim → timeout → definitive FAIL (never a hang); both exits print exactly one line.
fn demo_age_verifier(cpu: usize) {
    // Timeout is ~40x the observed victim completion (~150-200 ticks under a working ager on QEMU),
    // so it cannot false-FAIL a slow host yet still bounds a broken ager to a definitive FAIL.
    let mut waited = 0u64;
    while !AGE_LOW_DONE.load(Ordering::Acquire) && waited < 8_000 {
        sleep_ticks(8);
        waited += 8;
    }
    let done = AGE_LOW_DONE.load(Ordering::Acquire);
    let under_high = AGE_LOW_RAN_UNDER_HIGH.load(Ordering::Acquire);
    // Three outcomes, so a host-speed tuning miss is not mistaken for a real aging bug:
    //   PASS         - victim finished WHILE the hog was still running (aging beat starvation),
    //   FAIL         - victim never finished within the timeout (genuine starvation: aging broken),
    //   INCONCLUSIVE - victim finished but only after the hog already had (retune work sizes).
    let verdict = if done && under_high {
        "PASS"
    } else if !done {
        "FAIL"
    } else {
        "INCONCLUSIVE"
    };
    serial_println!(
        "PRIOAGE: [cpu{}] victim_done={} ran_under_high={} (waited {} ticks) => {}",
        cpu,
        done,
        under_high,
        waited,
        verdict
    );
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

/// How many demo tasks have finished (for headless verification).
pub fn demo_done() -> usize {
    DEMO_DONE.load(Ordering::Relaxed)
}
