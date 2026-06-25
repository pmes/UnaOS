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

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use lazy_static::lazy_static;
use spin::Mutex;

use crate::arch::gdt::MAX_CPUS;
use crate::arch::{apic, percpu};

/// Per-task kernel stack. 16 KiB is generous for kernel threads (the deepest thing they do is
/// `serial_println!` formatting); bump if a workload needs more.
const TASK_STACK_SIZE: usize = 16 * 1024;

/// Preemption quantum, in local-APIC timer ticks. After this many ticks a running task is
/// preempted and rotated to the back of its run queue. Small so round-robin sharing is visible.
const QUANTUM_TICKS: u32 = 4;

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
    /// an explicit reschedule request). Single preempt signal.
    need_resched: AtomicBool,
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

lazy_static! {
    /// Per-CPU run queues. The lock protects only the `VecDeque` structure; a `Task`'s own fields
    /// are touched solely by the CPU that owns it. Cross-CPU `spawn` pushes here under the lock.
    static ref RUN_QUEUES: [Mutex<VecDeque<Box<Task>>>; MAX_CPUS] =
        core::array::from_fn(|_| Mutex::new(VecDeque::with_capacity(RUNQ_CAPACITY)));

    /// Per-CPU sleeper lists: tasks blocked in `sleep_ticks`, with their wake deadline (this CPU's
    /// tick count). Touched ONLY by `run()` on the owning CPU (parked there on switch-back, drained
    /// at the loop top), so the lock is always uncontended — it exists only so the field is
    /// interior-mutable, not for cross-CPU synchronisation.
    static ref SLEEPERS: [Mutex<VecDeque<Sleeper>>; MAX_CPUS] =
        core::array::from_fn(|_| Mutex::new(VecDeque::with_capacity(RUNQ_CAPACITY)));
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

/// Create a kernel thread and enqueue it on `target_cpu`'s run queue, then poke that CPU so it
/// promptly picks the work up (wakes it from idle `hlt`; harmless if it is already running). The
/// task runs `entry(arg)` and is freed when `entry` returns. `target_cpu` must be an online AP.
pub fn spawn(name: &'static str, entry: fn(usize), arg: usize, target_cpu: usize) {
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
    });

    RUN_QUEUES[target_cpu].lock().push_back(task);
    poke_cpu(target_cpu);
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

/// Mark a parked/just-woken task READY, push it onto its PINNED CPU's run queue, and poke that CPU.
/// Used by the sleeper drain (same CPU → no IPI) and `Semaphore::post` (cross-CPU wake). The task
/// always returns to `task.cpu`, so its GS base stays correct on resume (tasks don't migrate).
/// Caller runs with IF=0; mirrors `spawn`'s enqueue+poke, so the same lost-wake-free argument holds.
fn make_ready(task: Box<Task>) {
    let target = task.cpu as usize;
    debug_assert!(target < MAX_CPUS, "make_ready: cpu out of range");
    task.state.store(STATE_READY, Ordering::Release);
    RUN_QUEUES[target].lock().push_back(task);
    poke_cpu(target);
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

    /// Acquire a permit, blocking the current task until one is available. No-op (returns
    /// immediately) if called outside a scheduled task (e.g. the unscheduled BSP).
    pub fn wait(&self) {
        let was_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable(); // IF=0 for the whole critical section
        self.lock_raw();

        if self.count.load(Ordering::Relaxed) > 0 {
            self.count.fetch_sub(1, Ordering::Relaxed);
            self.unlock_raw();
            if was_enabled {
                x86_64::instructions::interrupts::enable();
            }
            return;
        }

        // No permit: block. Only a scheduled task can park; on the BSP/idle context just bail (the
        // lock-handoff requires a real `current` to switch away from).
        let cpu = percpu::this_cpu().cpu_index as usize;
        let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
        if raw.is_null() {
            self.unlock_raw();
            if was_enabled {
                x86_64::instructions::interrupts::enable();
            }
            return;
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
        // already released by the scheduler that parked us — we must not touch it here.
        if was_enabled {
            x86_64::instructions::interrupts::enable();
        }
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

        let next = RUN_QUEUES[cpu].lock().pop_front(); // lock dropped immediately
        match next {
            Some(task) => {
                task.state.store(STATE_RUNNING, Ordering::Release);
                // Fresh quantum + clear the reschedule signal for the task we're about to run.
                SCHED[cpu].quantum.store(QUANTUM_TICKS, Ordering::Relaxed);
                SCHED[cpu].need_resched.store(false, Ordering::Relaxed);

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
                // Consume the park action exactly once: read it and immediately reset to NONE, so a
                // stale action can never leak into the next task's switch-back. Only a task that
                // switched back BLOCKED has a meaningful action.
                let park = SCHED[cpu].park_kind.swap(PARK_NONE, Ordering::Relaxed);
                let task = unsafe { Box::from_raw(raw) };
                match task.state.load(Ordering::Acquire) {
                    STATE_FINISHED => drop(task), // frees the stack
                    STATE_BLOCKED => park_blocked(cpu, park, task),
                    _ => {
                        // READY (yielded or preempted): rotate to the back of the run queue.
                        debug_assert_eq!(park, PARK_NONE, "non-blocked task carried a park action");
                        task.state.store(STATE_READY, Ordering::Release);
                        RUN_QUEUES[cpu].lock().push_back(task);
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

/// Touch the run queues so they are initialised on the BSP before any AP can reach them, and
/// reserve their capacity. Call once on the BSP after the heap is up and after SMP verification.
pub fn init() {
    for q in RUN_QUEUES.iter() {
        q.lock().reserve(RUNQ_CAPACITY);
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

/// Semaphore the producer/consumer demo blocks on. `'static`, as a blocking semaphore must be.
static DEMO_SEM: Semaphore = Semaphore::new(0);

/// Number of items the producer/consumer demo exchanges (waits and posts must match).
const DEMO_ITEMS: u32 = 5;

/// Turn scheduling on and spawn a small demo workload across the online APs, exercising every
/// scheduler mechanism: timer preemption, cooperative yield, task exit/free, timer-driven sleep,
/// and semaphore block + CROSS-CPU wake. Called once on the BSP after `smp::start_aps` (hence after
/// `verify_smp`). No-op with no APs.
pub fn start_demo(online_aps: &[usize]) {
    if online_aps.is_empty() {
        serial_println!("SCHED: no application processors online; scheduler idle.");
        return;
    }

    // Reserve the semaphore's waiter capacity FIRST — before the APs are released or any task is
    // spawned — so no task can ever block on it before its waiter list is sized (B2's no-realloc
    // proof then holds unconditionally, not by spawn-ordering).
    DEMO_SEM.init();
    // Release the APs into their scheduler loops, and arm the timer-preempt path.
    SCHED_ACTIVE.store(true, Ordering::Release);
    SCHED_GO.store(true, Ordering::Release);

    serial_println!(
        "SCHED: scheduling enabled on {} AP(s) {:?}; spawning demo threads...",
        online_aps.len(),
        online_aps
    );

    // Pin two non-yielding "busy" threads to the first AP: with round-robin timer preemption their
    // output INTERLEAVES (without preemption the first would monopolise the core until it exits).
    let cpu_busy = online_aps[0];
    spawn("busy-A", demo_busy, encode(cpu_busy, b'A'), cpu_busy);
    spawn("busy-B", demo_busy, encode(cpu_busy, b'B'), cpu_busy);

    // Producer/consumer over DEMO_SEM. The consumer blocks in wait(); the producer sleeps (proving
    // sleep_ticks), then posts — waking the consumer. With a third AP the producer runs on a
    // DIFFERENT core than the consumer, so post() exercises the CROSS-CPU wake (move the blocked
    // task to its own core's run queue + reschedule IPI). The worker proves yield + exit/free.
    if let Some(&cpu_cons) = online_aps.get(1) {
        spawn("cons-C", demo_consumer, encode(cpu_cons, b'C'), cpu_cons);
        let cpu_prod = online_aps.get(2).copied().unwrap_or(cpu_cons);
        spawn("prod-P", demo_producer, encode(cpu_prod, b'P'), cpu_prod);
        spawn("worker-E", demo_worker, encode(cpu_prod, b'E'), cpu_prod);
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

/// Consumer: blocks on the semaphore for each item. It cannot make progress until the producer
/// posts — so its "GOT item N" lines prove the block actually suspended it and a `post()` woke it.
fn demo_consumer(arg: usize) {
    let (cpu, tag) = decode(arg);
    for round in 0..DEMO_ITEMS {
        serial_println!("SCHED: [cpu{cpu} cons-{tag}] waiting for item {round}");
        DEMO_SEM.wait();
        serial_println!("SCHED: [cpu{cpu} cons-{tag}] GOT item {round}");
    }
    serial_println!("SCHED: [cpu{cpu} cons-{tag}] done");
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

/// Producer: sleeps (timer-driven block), then posts an item — waking the consumer (cross-CPU when
/// they are on different cores).
fn demo_producer(arg: usize) {
    let (cpu, tag) = decode(arg);
    for round in 0..DEMO_ITEMS {
        sleep_ticks(12);
        serial_println!("SCHED: [cpu{cpu} prod-{tag}] posting item {round}");
        DEMO_SEM.post();
    }
    serial_println!("SCHED: [cpu{cpu} prod-{tag}] done");
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

/// Short-lived worker: a couple of rounds, then exits (returns) — exercising stack reclamation.
fn demo_worker(arg: usize) {
    let (cpu, tag) = decode(arg);
    for round in 0..3u32 {
        serial_println!("SCHED: [cpu{cpu} worker-{tag}] step {round}");
        yield_now();
    }
    serial_println!("SCHED: [cpu{cpu} worker-{tag}] exiting");
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

/// How many demo tasks have finished (for headless verification).
pub fn demo_done() -> usize {
    DEMO_DONE.load(Ordering::Relaxed)
}
