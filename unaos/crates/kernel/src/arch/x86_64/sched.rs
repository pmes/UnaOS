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
//
// KERNEL-CLOCK layer (wall-clock timing): once `apic::calibrate` arms the local-APIC heartbeat at a
// real 1 kHz (`apic::TICK_HZ`), a tick is a millisecond, so `sleep_ms` is just `sleep_ticks` fed
// through `arch::ms_to_ticks`, and `JoinHandle::join_timeout` bounds a join by polling the
// completion `Semaphore` with the new non-blocking `Semaphore::try_wait` between `sleep_ticks` naps.
// `join_timeout` deliberately reuses ONLY the existing sleeper machinery (each nap is an ordinary
// `sleep_ticks`) — it adds no new park kind, no dual-deadline, and no lock-handoff, so none of the
// invariants above are touched. A timed-out joiner drops its handle while the joined task may still
// hold its own `done_sem` `Arc` clone, so a later `post()` into an empty waiter list stays sound
// (bumps the count on a soon-to-be-freed semaphore; no dangle, no leak).
//
// SCHED-POLISH layer (two §4 refinements, invariants above untouched):
//   * `effective_level` aging refinement (M1). A ready task carries a transient EFFECTIVE level
//     (`priority..NUM_PRIORITIES`) and always occupies `levels[effective_level]`. A FRESH enqueue
//     (`RunQueue::push`: spawn / wake) re-bases it to `priority`; the aging sweep (`RunQueue::age`)
//     bumps it up one on promotion; a RE-ENQUEUE after a dispatch (`RunQueue::requeue`: yield /
//     preempt) DECAYS it toward base by ONE level instead of resetting. So a task dispatched while
//     climbing under bursty load re-climbs at most one level, holding the starvation bound at
//     ~`AGE_TICKS` per level even when intermediate levels drain (the pre-refinement blow-up). The
//     field is owning-CPU-only + lock-protected exactly like `wait_ticks`; base `priority` stays the
//     immutable lock-free read for `poke_for`/`make_ready`/the dispatch publish.
//   * `Condvar::init_with_capacity(n)` (M2). The `WAIT_CAPACITY` (32) waiter-list reservation is now
//     per-`Condvar` (a `capacity` field, default 32, still exactly reserved by `init()`); `wait()`'s
//     alloc-free-park assert tracks the per-instance value. `RwLock::init_with_reader_capacity(n)`
//     threads it to the reader condvar (the writer queue + inner mutex keep the default), so a
//     `>32`-simultaneously-blocked-reader `RwLock` is constructible. `Condvar::new()` / `RwLock::new`
//     / `init()` behaviour is byte-identical to before.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
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
/// the load and runs; on dispatch it DECAYS one effective level (not all the way to base — the
/// `current_level`/`effective_level` refinement, see `RunQueue::requeue`/`age`), so it re-climbs at
/// most one level per turn. This holds the starvation bound at ~`AGE_TICKS` per level the task must
/// climb EVEN under bursty mixed load where intermediate levels drain (before the refinement a
/// dispatch at an intermediate level re-based the whole climb, making the bound finite-but-larger).
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

/// RFLAGS.IF (interrupt-enable, bit 9). U3.5: OR'd into a PREEMPTIBLE ring-3 task's `iretq` RFLAGS so
/// the timer can evict it; a cooperative (default) ring-3 task keeps IF clear and runs to completion.
const RFLAGS_IF: u64 = 1 << 9;

/// U3.5: external-termination handshake for a PREEMPTIBLE ring-3 task that never yields (a spinner).
/// A cooperative task exits via `sys_exit`; a never-syscalling one can only be stopped by the
/// scheduler REAPING it at a preemption boundary. The requester sets `requested`; the scheduler, on
/// the task's next switch-back, tears its address space down, drops it, and sets `reaped`. Shared by
/// `Arc` (exactly like `Task::done_sem`) so it outlives the dropped `Task` — the requester keeps a
/// clone across the request/reap window.
pub struct KillSwitch {
    requested: AtomicBool,
    reaped: AtomicBool,
}

impl KillSwitch {
    /// A fresh, un-requested kill switch. `const` so a fixture can build one before the Arc.
    pub const fn new() -> Self {
        KillSwitch { requested: AtomicBool::new(false), reaped: AtomicBool::new(false) }
    }
    /// Request termination at the task's next preemption (idempotent).
    pub fn request(&self) {
        self.requested.store(true, Ordering::Release);
    }
    /// True once the scheduler has reaped the task (torn its address space down + dropped it).
    pub fn is_reaped(&self) -> bool {
        self.reaped.load(Ordering::Acquire)
    }
    fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
    fn mark_reaped(&self) {
        self.reaped.store(true, Ordering::Release);
    }
}

impl Default for KillSwitch {
    fn default() -> Self {
        Self::new()
    }
}

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
    /// ONLY under the owning CPU's run-queue spinlock (zeroed by `RunQueue::push`/`requeue` on every
    /// enqueue, accrued + consumed by `RunQueue::age` on that CPU). NEVER read cross-CPU — unlike
    /// `priority`, it is mutable and lock-protected, so it must not be read off the owning CPU.
    wait_ticks: u32,
    /// Transient EFFECTIVE level this task currently sits at in the run queue (`priority..NUM_PRIORITIES`).
    /// The refinement over plain reset-on-dispatch aging: a task always occupies `levels[effective_level]`,
    /// and the run-queue placement operations keep this field == that index. A FRESH enqueue
    /// (`RunQueue::push`, i.e. spawn / wake) re-bases it to `priority`; the aging sweep (`RunQueue::age`)
    /// bumps it up one on promotion; a RE-ENQUEUE after a dispatch (`RunQueue::requeue`, i.e. yield /
    /// preempt) DECAYS it toward base by one level rather than all the way — so an intermediate dispatch
    /// no longer erases a multi-level promotion, tightening the starvation bound (see `RunQueue::age`).
    /// Same discipline as `wait_ticks`: lock-protected, owning-CPU-only, NEVER read cross-CPU (the base
    /// `priority` — not this — is what `poke_for`/`make_ready`/the dispatch publish read lock-free).
    effective_level: u8,
    /// Completion signal for `join()`, or `None` for a fire-and-forget task. A joinable task carries
    /// a clone of the same `Arc<Semaphore>` (0 permits) held by its `JoinHandle`; the trampoline
    /// `post()`s it after `entry` returns. The Arc — not a `'static` lifetime — keeps the semaphore
    /// alive across the park/post window (see `JoinHandle`). Read (never moved) by the trampoline
    /// through the raw `current` pointer; dropped when `run()` drops the Box on the Finished path.
    done_sem: Option<Arc<Semaphore>>,
    /// U1a: ring-3 entry point + initial user rsp. Non-zero only for `is_user` tasks made via
    /// `spawn_user`: such a task's initial frame lands in `user_task_trampoline`, which `iretq`s to
    /// ring 3 at `user_entry` with rsp = `user_rsp`. 0 / unused for ordinary kernel tasks.
    user_entry: u64,
    user_rsp: u64,
    /// U3: this task's private CR3 (per-process PML4 physical base), or 0 to run in the SHARED kernel
    /// address space (the default — U1a/U1b/U2 keep 0). When non-zero, `user_task_trampoline` installs
    /// it before the `iretq` to ring 3, and `exit` restores the kernel CR3 + frees the slot on
    /// teardown. Ring 3 is cooperative (IF-masked), so one CR3 covers the task's whole ring-3 run.
    user_cr3: u64,
    /// U3.5: preemptible ring 3. When true, `user_task_trampoline` drops to ring 3 with RFLAGS.IF
    /// SET, so the timer can preempt this task and other work shares its core (the DoS fix). The
    /// default `false` (U1a/U1b/U2/U2.5/U3) keeps IF clear — cooperative, run-to-completion FIFO — so
    /// those fixtures stay byte-identical. Preemptible: the U3.5 spinner, and — knob-on
    /// (`s4_sync_storage()`) — the u6gx cooperative-spin fixtures (a non-preemptible spinner on the
    /// storage service task's core would starve a cross-core live created read; see STOR-1 S5).
    preemptible: bool,
    /// U3.5: external-kill handshake for a `preemptible` task that never yields, or `None` (every
    /// other task). The scheduler checks it on switch-back and REAPS the task (address-space teardown
    /// + drop + mark reaped) instead of requeuing. An `Arc` (like `done_sem`) so the requester's clone
    /// keeps it alive after the Box is dropped.
    kill: Option<Arc<KillSwitch>>,
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

/// VUG-1 M3b — per-CPU load counters for the demo's "CPU pulse" meter (BeOS-Pulse style). Additive,
/// lock-free, relaxed: `run()` bumps `CPU_BUSY[cpu]` each time it dispatches a task and `CPU_IDLE[cpu]`
/// each time it idles (`hlt`). The demo samples both once per frame and shows busy/(busy+idle) over
/// the window as a per-core bar. Introspection only — never read on any scheduling path. This is the
/// SEAM a real per-core utilization feed would replace.
static CPU_BUSY: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static CPU_IDLE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

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
    /// FRESH ENQUEUE at the task's BASE priority level (FIFO within the level), clamped in range, and
    /// reset its aging state. Used on spawn and on WAKE (`make_ready`) — a task that just became
    /// runnable (newly created, or unblocked after doing its work) starts over at base, so strict
    /// priority is preserved for genuinely new work. Zeroes `wait_ticks` (it only ages while WAITING)
    /// and re-bases `effective_level` to the base level. NOT used for a mere yield/preempt re-enqueue
    /// (that is `requeue`, which preserves promotion progress).
    fn push(&mut self, mut task: Box<Task>) {
        task.wait_ticks = 0;
        let level = (task.priority as usize).min(NUM_PRIORITIES - 1);
        task.effective_level = level as u8;
        self.levels[level].push_back(task);
    }
    /// RE-ENQUEUE a task that switched back READY from a DISPATCH (yield or timer preempt), decaying
    /// its transient effective level by ONE toward base rather than resetting all the way (as `push`
    /// would). This is the `current_level` refinement: an intermediate dispatch — the task got a slice
    /// while climbing, e.g. because a contended level momentarily drained under bursty load — no longer
    /// erases a multi-level promotion, so the task re-climbs at most one level instead of from base.
    /// Base `priority` is untouched (immutable); the new level is clamped to `>= base`, so a task never
    /// decays below its own priority and, absent contention, settles back at base within a few
    /// dispatches (strict priority restored). Resets the aging clock for the new level.
    fn requeue(&mut self, mut task: Box<Task>) {
        let base = (task.priority as usize).min(NUM_PRIORITIES - 1);
        let cur = (task.effective_level as usize).min(NUM_PRIORITIES - 1);
        let level = cur.saturating_sub(1).max(base);
        task.wait_ticks = 0;
        task.effective_level = level as u8;
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
    ///
    /// `effective_level` is kept == the level the task actually occupies (bumped on promotion), so a
    /// later `requeue` after a dispatch can decay it by ONE level instead of dropping it to base. This
    /// is the refinement: without it, reset-on-dispatch made the starvation bound `~2*AGE_TICKS` per
    /// level only when NO intermediate level drains, blowing up under bursty mixed load (a dispatch at
    /// an intermediate level re-based the climb); preserving the level across a dispatch caps the
    /// re-climb at one level, so the bound holds at `~2*AGE_TICKS` per level regardless of intermediate
    /// drains.
    fn age(&mut self, elapsed: u32) {
        for level in (0..NUM_PRIORITIES - 1).rev() {
            let n = self.levels[level].len();
            for _ in 0..n {
                let mut task = self.levels[level].pop_front().expect("age: len/pop mismatch");
                task.wait_ticks = task.wait_ticks.saturating_add(elapsed);
                if task.wait_ticks >= AGE_TICKS {
                    task.wait_ticks -= AGE_TICKS; // carry surplus credit, don't discard it
                    debug_assert!(level + 1 < NUM_PRIORITIES, "age: promotion above top level");
                    task.effective_level = (level + 1) as u8; // track the RELOCATE destination
                    self.levels[level + 1].push_back(task); // RELOCATE up one level (base unchanged)
                } else {
                    debug_assert_eq!(
                        task.effective_level as usize, level,
                        "age: effective_level out of sync with occupied level"
                    );
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

    // Signal completion to any joiner. `post()` runs with IF=1 and self-masks (Semaphore::post),
    // and cross-CPU-wakes a parked joiner if any. The task's OWN `done_sem` Arc clone is the
    // liveness anchor for this `post()` — it MUST remain in the Box until `run()` drops it on the
    // Finished path, so we BORROW it here (never take/move the Arc out before `exit()`).
    unsafe {
        debug_assert!(
            (*raw).state.load(Ordering::Acquire) == STATE_RUNNING && (*raw).cpu as usize == cpu,
            "task_trampoline: task not running on its own CPU at completion"
        );
        if let Some(sem) = &(*raw).done_sem {
            sem.post();
        }
    }
    exit();
}

/// Build a fresh task's initial stack frame so the first `switch_context` into it lands in
/// `trampoline` (`task_trampoline` for kernel threads, `user_task_trampoline` for ring-3 tasks)
/// with an ABI-correct stack. Returns the value to store in `ctx_rsp`.
///
/// SysV requires rsp ≡ 8 (mod 16) at a function's first instruction (a `call` pushes an 8-byte
/// return address onto a 16-aligned rsp). After `switch_context` pops 6 regs + RFLAGS and `ret`s,
/// the trampoline sees rsp = (rip slot) + 8, so the rip slot must be 16-aligned — equivalently
/// `new_rsp ≡ 8 (mod 16)`. We CONSTRUCT that (don't merely assume it) and assert it.
fn build_initial_frame(stack: &mut [u8], trampoline: extern "C" fn() -> !) -> u64 {
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
        p.add(7).write(trampoline as *const () as u64); // consumed by ret
    }
    new_rsp as u64
}

// ---------------------------------------------------------------------------------------------
// Public API: spawn / yield / exit
// ---------------------------------------------------------------------------------------------

/// Shared spawn path: build a kernel thread at `priority` (optionally carrying a `done_sem`
/// completion signal for `join`), enqueue it on `target_cpu`'s run queue, and poke that CPU. Returns
/// the new task's id. `target_cpu` must be an online AP.
fn spawn_inner(
    name: &'static str,
    entry: fn(usize),
    arg: usize,
    target_cpu: usize,
    priority: u8,
    done_sem: Option<Arc<Semaphore>>,
) -> u64 {
    assert!(target_cpu < MAX_CPUS, "spawn: target_cpu out of range");

    let mut stack: Box<[u8]> = alloc::vec![0u8; TASK_STACK_SIZE].into_boxed_slice();
    let ctx_rsp = build_initial_frame(&mut stack, task_trampoline);

    let id = NEXT_TID.fetch_add(1, Ordering::Relaxed);
    let task = Box::new(Task {
        id,
        name,
        state: AtomicU8::new(STATE_READY),
        ctx_rsp,
        stack,
        entry,
        arg,
        cpu: target_cpu as u32,
        priority,
        wait_ticks: 0, // re-zeroed by push() on every enqueue; this satisfies the struct literal
        effective_level: 0, // re-based to `priority` by push() on enqueue (just below)
        done_sem,
        user_entry: 0,
        user_rsp: 0,
        user_cr3: 0,
        preemptible: false,
        kill: None,
    });

    RUN_QUEUES[target_cpu].lock().push(task);
    poke_for(target_cpu, priority);
    id
}

/// Create a fire-and-forget kernel thread at `priority` on `target_cpu`. The task runs `entry(arg)`
/// and is freed when `entry` returns; there is no way to wait for it (use `spawn_joinable` for that).
pub fn spawn(name: &'static str, entry: fn(usize), arg: usize, target_cpu: usize, priority: u8) {
    spawn_inner(name, entry, arg, target_cpu, priority, None);
}

/// Placeholder `entry` for ring-3 tasks: `spawn_user` stores this in `Task.entry`, but
/// `user_task_trampoline` never calls it (it `iretq`s to ring 3 instead). Panics if ever reached.
fn user_never(_: usize) {
    unreachable!("user task's kernel `entry` was called");
}

/// First code a ring-3 task runs at ring 0, reached when `switch_context` `ret`s into its freshly
/// built frame (IF masked, from INITIAL_RFLAGS). Unlike `task_trampoline` it does NOT enable
/// interrupts: ring 3 stays non-preemptible for U1a (the `iretq` frame carries IF clear — the M6a
/// mirror; preemptible ring 3 is a later arc). It records this task's kernel-stack top for the
/// syscall + fault paths, then drops to ring 3.
///
/// The task's Box kernel stack is abandoned across the `iretq`; the SYSCALL stub re-enters on it at
/// `syscall_kernel_rsp` = the stack TOP, so the shallow frame this trampoline used is simply
/// overwritten (control never returns to it — ring 3 leaves only via `syscall` or a fault, both of
/// which reset rsp to the top). The Box therefore needs headroom for the syscall handler; it has it.
extern "C" fn user_task_trampoline() -> ! {
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *const Task;
    debug_assert!(!raw.is_null(), "user_task_trampoline: current is null");
    let (entry, user_rsp, preemptible) =
        unsafe { ((*raw).user_entry, (*raw).user_rsp, (*raw).preemptible) };
    // U4x: TSS.RSP0 (used by the CPU on a ring-3 FAULT or timer preemption) and the per-CPU SYSCALL
    // kernel-rsp anchor (used by the SYSCALL stub — SYSCALL does not switch stacks itself) are BOTH
    // installed at the scheduler DISPATCH site (`run`) — NOT here — so, exactly like the CR3 install
    // U3.5 moved there, they are correct for BOTH this first entry AND a task RESUMED after a block or
    // preemption (which never re-enters this trampoline). That single dispatch install is what makes a
    // SECOND concurrent user task per core safe (U4x's parent + children): a syscall or fault from a
    // resumed task lands on ITS OWN kernel stack, never a just-freed sibling's. See `run`.

    // U3/U3.5: this task's private CR3 was installed by the scheduler DISPATCH path (`run`) before it
    // switched into this trampoline — NOT here — so the same CR3 install covers a RESUMED preempted
    // task too (which never re-enters this trampoline). `exit` restores the kernel CR3 + frees the
    // slot on teardown. The kernel half (kernel code, this Box kernel stack, GDT/TSS/IDT/percpu, the
    // iretq frame below) is shared into every per-process PML4, so running under the process CR3 pulls
    // nothing out from under us — only PML4[2] (the USER_BASE window) is private.

    // U3.5: a PREEMPTIBLE task drops to ring 3 with RFLAGS.IF SET so the timer can evict it; the
    // default cooperative task keeps IF clear (INITIAL_RFLAGS), running to completion FIFO.
    let user_rflags = if preemptible { INITIAL_RFLAGS | RFLAGS_IF } else { INITIAL_RFLAGS };

    // Drop to ring 3. `swapgs` parks this CPU's PerCpuData pointer in the GS shadow for the syscall
    // path; the `iretq` frame is [SS, RSP, RFLAGS, CS, RIP] (RIP pushed LAST so `iretq` pops it
    // first). Interrupts are masked HERE, so the swapgs/iretq window can't take an IRQ; a preemptible
    // task only becomes interruptible once `iretq` loads its IF=1 RFLAGS in ring 3.
    //
    // U2 Part-0b: FIRST-ENTRY GPR SCRUB — the x86 twin of the aarch64 M6d first-`eret` scrub (and of
    // the U1b SYSRET-return scrub, which only covers the return half). Before this, the trampoline's
    // live kernel values reached ring 3 in registers at first entry (the Task Box pointer `raw`, the
    // kernel-stack top, the entry VA, ...). Zero every GPR except `rsp` (which must keep pointing at
    // the iretq frame) AFTER the five frame words are pushed and BEFORE `iretq`, so the user program
    // starts from an all-zero register file. The user entry ABI takes no register arguments — the
    // U1a blob and the U2 loaded HELLO.BIN both load their own registers — so this leaks nothing the
    // program needs.
    unsafe {
        core::arch::asm!(
            "swapgs",
            "push {ss}",
            "push {ursp}",
            "push {rflags}",
            "push {cs}",
            "push {entry}",
            // U2.5 Part 0-i: FIRST-ENTRY x87/MMX SCRUB — the x86 twin of the aarch64 M6f FP/SIMD
            // first-entry scrub. x87/MMX state is ring-3-reachable (CR0.EM/TS are 0 out of INIT
            // reset, and neither the AP trampoline nor UEFI sets them), yet nothing zeros it, so
            // firmware/kernel-era residue would reach ring 3 in the FP register file. `fninit`
            // empties only the x87 TAGS; the physical mantissa bits survive and an MMX move reads
            // them regardless of tag. So `fninit` first, then push eight 0.0's (fldz) — overwriting
            // all eight physical registers R0..R7 — then pop them all (fstp st(0)), which stores 0.0
            // and re-marks every register empty. Safe at CPL 0: the kernel target is `+soft-float`
            // with SIMD/MMX disabled, so no kernel value lives in the FP file. SSE stays fenced
            // separately by CR4.OSFXSR=0 (#UD in ring 3) — untouched here. These touch no GPR/rsp,
            // so their placement before the GPR scrub is immaterial to the frame.
            "fninit",
            "fldz", "fldz", "fldz", "fldz", "fldz", "fldz", "fldz", "fldz",
            "fstp st(0)", "fstp st(0)", "fstp st(0)", "fstp st(0)",
            "fstp st(0)", "fstp st(0)", "fstp st(0)", "fstp st(0)",
            // Scrub every GPR but rsp. The five inputs above are dead after their pushes, so zeroing
            // the registers that carried them is safe; iretq reads only the pushed frame.
            "xor eax, eax",
            "xor ebx, ebx",
            "xor ecx, ecx",
            "xor edx, edx",
            "xor esi, esi",
            "xor edi, edi",
            "xor ebp, ebp",
            "xor r8d, r8d",
            "xor r9d, r9d",
            "xor r10d, r10d",
            "xor r11d, r11d",
            "xor r12d, r12d",
            "xor r13d, r13d",
            "xor r14d, r14d",
            "xor r15d, r15d",
            "iretq",
            ss = in(reg) crate::arch::gdt::USER_DATA_SEL as u64,
            ursp = in(reg) user_rsp,
            rflags = in(reg) user_rflags, // reserved bit set; IF clear (cooperative) or set (preemptible)
            cs = in(reg) crate::arch::gdt::USER_CODE_SEL as u64,
            entry = in(reg) entry,
            options(noreturn),
        );
    }
}

/// Create a ready ring-3 (user-mode) task on `target_cpu`'s run queue (U1a): when dispatched it
/// drops to ring 3 at `user_entry` with rsp = `user_rsp` (both from `syscall::setup`) and calls
/// back into the kernel via `syscall`. MUST be spawned on a SCHEDULED core (an AP), never the
/// unscheduled BSP — `user_task_trampoline` reads `SCHED[cpu].current`, which is null on a core
/// that never runs the scheduler loop. Fire-and-forget: `sys_exit` marks it FINISHED and the
/// scheduler reclaims it. Returns the task id.
pub fn spawn_user(name: &'static str, user_entry: u64, user_rsp: u64, target_cpu: usize) -> u64 {
    spawn_user_in_space(name, user_entry, user_rsp, target_cpu, 0)
}

/// U3: like `spawn_user`, but the task runs in a PRIVATE address space `user_cr3` (a per-process
/// PML4 physical base from `memory::alloc_user_space`). `user_task_trampoline` installs that CR3
/// before dropping to ring 3, and `exit` restores the kernel CR3 + frees the slot on teardown.
/// `user_cr3 == 0` is exactly `spawn_user` (the shared kernel window — U1a/U1b/U2).
pub fn spawn_user_in_space(
    name: &'static str,
    user_entry: u64,
    user_rsp: u64,
    target_cpu: usize,
    user_cr3: u64,
) -> u64 {
    spawn_user_inner(name, user_entry, user_rsp, target_cpu, user_cr3, false, None)
}

/// U3.5: like `spawn_user_in_space`, but the task drops to ring 3 PREEMPTIBLE (RFLAGS.IF set, so the
/// timer can evict it) and carries a `KillSwitch` the watchdog uses to reap it — it never yields, so
/// the scheduler is the only thing that can stop it. The x86 twin of aarch64 M6e's I-unmasked
/// `spawn_user`. `user_cr3` must be a real private address space (a preemptible task can be preempted
/// mid-run, and the CR3 is re-installed at every dispatch — including its resume). Returns the id.
pub fn spawn_user_preemptible(
    name: &'static str,
    user_entry: u64,
    user_rsp: u64,
    target_cpu: usize,
    user_cr3: u64,
    kill: Arc<KillSwitch>,
) -> u64 {
    spawn_user_inner(name, user_entry, user_rsp, target_cpu, user_cr3, true, Some(kill))
}

/// Shared ring-3 spawn: build the user task (cooperative or `preemptible`, with an optional external
/// `kill` handshake), enqueue it on `target_cpu`, and poke that CPU. `spawn_user_in_space` (hence
/// `spawn_user` and every U1a/U1b/U2/U2.5/U3 caller) passes `preemptible=false, kill=None`, so those
/// tasks are built byte-identically to before U3.5.
fn spawn_user_inner(
    name: &'static str,
    user_entry: u64,
    user_rsp: u64,
    target_cpu: usize,
    user_cr3: u64,
    preemptible: bool,
    kill: Option<Arc<KillSwitch>>,
) -> u64 {
    assert!(target_cpu < MAX_CPUS, "spawn_user: target_cpu out of range");
    let mut stack: Box<[u8]> = alloc::vec![0u8; TASK_STACK_SIZE].into_boxed_slice();
    let ctx_rsp = build_initial_frame(&mut stack, user_task_trampoline);
    let id = NEXT_TID.fetch_add(1, Ordering::Relaxed);
    let task = Box::new(Task {
        id,
        name,
        state: AtomicU8::new(STATE_READY),
        ctx_rsp,
        stack,
        entry: user_never, // never called — the trampoline iretq's to ring 3 instead
        arg: 0,
        cpu: target_cpu as u32,
        priority: PRIO_NORMAL,
        wait_ticks: 0,
        effective_level: 0, // re-based to `priority` by push() on enqueue (just below)
        done_sem: None,
        user_entry,
        user_rsp,
        user_cr3,
        preemptible,
        kill,
    });
    RUN_QUEUES[target_cpu].lock().push(task);
    poke_for(target_cpu, PRIO_NORMAL);
    id
}

/// Turn scheduling on (idempotent): release the APs from their post-online wait loop into `run()`
/// and enable timer preemption. `start_demo` does the same, but only under the `sched_demo`
/// feature; the default build's U1a ring-3 demo needs scheduling live too, so it calls this. Safe
/// to call in addition to `start_demo` — both merely set these two release flags.
pub fn enable() {
    SCHED_ACTIVE.store(true, Ordering::Release);
    SCHED_GO.store(true, Ordering::Release);
}

/// Like `spawn`, but returns a `JoinHandle` a scheduled task can `join()` to block until this task
/// finishes. Allocates an `Arc<Semaphore>` (0 permits) shared between the new task and the handle;
/// the task's trampoline posts it on completion. Costs one heap alloc + a reserved waiter list, so
/// only pay it when you actually need to join.
pub fn spawn_joinable(
    name: &'static str,
    entry: fn(usize),
    arg: usize,
    target_cpu: usize,
    priority: u8,
) -> JoinHandle {
    let done = Arc::new(Semaphore::new(0));
    done.init(); // reserve the waiter list BEFORE the task can run + post (alloc-free park)
    let id = spawn_inner(name, entry, arg, target_cpu, priority, Some(done.clone()));
    JoinHandle { done, id }
}

/// Outcome of `JoinHandle::join_timeout`: the joined task finished within the deadline, or the
/// deadline elapsed first (the joiner gave up; the task may still be running).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[must_use]
pub enum JoinResult {
    /// The joined task's trampoline posted its completion permit before the deadline.
    Completed,
    /// The deadline elapsed with the task still unfinished. The handle is consumed either way, but
    /// the joined task keeps running (and will free normally when it eventually returns).
    TimedOut,
}

/// A handle to wait for a `spawn_joinable` task to finish. Holds a clone of the task's completion
/// `Arc<Semaphore>`; `join()` blocks until the task's trampoline posts it. Single-shot: `join`
/// consumes the handle (one join per task), and the handle is intentionally NOT `Clone` (the
/// completion semaphore carries exactly one permit — a second joiner would block forever).
pub struct JoinHandle {
    done: Arc<Semaphore>,
    /// Id of the joined task, for diagnostics.
    #[allow(dead_code)]
    id: u64,
}

impl JoinHandle {
    /// Id of the task this handle joins.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Block the current task until the joined task finishes. MUST be called from a scheduled task
    /// (it blocks); the `assert` rejects a call off the scheduler — e.g. the unscheduled BSP — loudly
    /// rather than silently returning as if the task had finished.
    ///
    /// No timeout: a joined task that PANICS or never returns leaves the joiner blocked forever (the
    /// completion permit is posted only on a normal `entry` return; this no-`std` kernel has no
    /// unwinding and no backstop). This matches the kernel's panic-halts policy.
    ///
    /// `self` (the Arc clone) deliberately stays bound for the whole body: it is the refcount anchor
    /// that keeps the completion semaphore alive while this task is parked in its waiter list. Do not
    /// destructure or move the Arc out before `wait()` returns.
    pub fn join(self) {
        assert!(
            self.done.wait(),
            "JoinHandle::join() must be called from a scheduled task"
        );
    }

    /// Bounded join: block the current task until the joined task finishes OR `timeout_ticks` of this
    /// CPU's local-APIC timer elapse, whichever comes first. Unlike `join`, a hung or never-returning
    /// task can NOT trap the joiner forever — the deadline always releases it (`TimedOut`).
    ///
    /// Implementation: poll the completion `Semaphore` with `try_wait` between short `sleep_ticks`
    /// naps, rather than parking on the semaphore's waiter list. This is what lets it be time-bounded
    /// at all — a `wait()`-parked joiner has no deadline — and it deliberately reuses ONLY the
    /// existing, invariant-preserving sleeper machinery (each nap is an ordinary `sleep_ticks`), so
    /// it introduces no new park kind, no dual-deadline, and no lock-handoff. The trade is a wake
    /// every `JOIN_POLL_TICKS` ticks while waiting; joins are rare and the timeout paths short, so the
    /// cost is negligible. Poll granularity (and thus worst-case overshoot past a just-missed
    /// completion) is one `JOIN_POLL_TICKS` window.
    ///
    /// MUST be called from a scheduled task, like `join`: it relies on `sleep_ticks` actually
    /// blocking to advance the deadline. The assert rejects a call off the scheduler (e.g. the
    /// unscheduled BSP) loudly — there `sleep_ticks` is a no-op, which would busy-spin the poll.
    ///
    /// `self` stays bound for the whole body: its `Arc` clone is what keeps the completion semaphore
    /// alive while we poll it, exactly as `join` keeps it alive across the park. On `TimedOut` the
    /// handle is dropped while the task may still hold its own `done_sem` clone — the semaphore stays
    /// live until the task finishes and drops it, so a later `post()` into an empty waiter list is
    /// sound (it just bumps the count on a soon-to-be-freed semaphore); no dangle, no leak.
    #[must_use]
    pub fn join_timeout(self, timeout_ticks: u64) -> JoinResult {
        /// Poll cadence: how many ticks the joiner sleeps between completion checks.
        const JOIN_POLL_TICKS: u64 = 2;

        let cpu = percpu::this_cpu().cpu_index as usize;
        assert!(
            SCHED[cpu].current.load(Ordering::Acquire) != 0,
            "JoinHandle::join_timeout() must be called from a scheduled task"
        );

        let mut remaining = timeout_ticks;
        loop {
            if self.done.try_wait() {
                return JoinResult::Completed;
            }
            if remaining == 0 {
                return JoinResult::TimedOut; // final check above already ran, so this is authoritative
            }
            let nap = JOIN_POLL_TICKS.min(remaining);
            sleep_ticks(nap);
            remaining -= nap;
        }
    }
}

// Compile-time guard for the `Task` <-> `Arc<Semaphore>` Send cycle introduced by `done_sem`. We do
// NOT add an `unsafe impl Send for Semaphore` (that would be a second unsafe to audit, and could
// mask a future `!Send` field): `Semaphore`'s Send auto-derives so long as every field is `Send`,
// and the cycle resolves co-inductively to Send while no leaf is `!Send`. This probe locks that in —
// if a future field breaks it, fix that field rather than papering over it with `unsafe impl`.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Task>();
    assert_send::<Arc<Semaphore>>();
    assert_send::<JoinHandle>();
    // Smoke-test that `RwLock<T>: Sync` holds for a `Send + Sync` T (i.e. the unsafe impl exists and
    // compiles with the intended bound). The correctness of REQUIRING `T: Sync` — not Mutex's weaker
    // `T: Send` — is upheld by the explicit `where T: Send + Sync` on the impl + its SAFETY comment;
    // stable Rust can't mechanically negative-assert that `RwLock<NotSync>` is `!Sync`.
    const fn assert_sync<T: Sync>() {}
    assert_sync::<RwLock<u64>>();
};

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
        // U3: if this task owned a private address space, tear it down HERE — restore the shared
        // kernel CR3 (that `mov cr3` full-flush retires this process's user TLB entries), THEN free
        // the slot. Order matters: free-after-restore, so no core is left on the dead root. We run on
        // this task's own kernel stack + scheduler code, both Global in the kernel half (shared into
        // every process root), so restoring the kernel CR3 doesn't pull the stack out from under us.
        let user_cr3 = (*raw).user_cr3;
        if user_cr3 != 0 {
            crate::arch::memory::restore_kernel_cr3();
            crate::arch::memory::free_user_space_by_cr3(user_cr3);
        }
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

/// Block the current task for approximately `ms` milliseconds — `sleep_ticks` expressed in real time
/// via the calibrated timebase. The local-APIC heartbeat is armed at `apic::TICK_HZ` (1 kHz once
/// `apic::calibrate` has run), so `arch::ms_to_ticks` maps ms to ticks and the wake lands within one
/// tick of the requested wall-clock delay on any machine. Before calibration the tick is ~0.8 ms
/// under QEMU, so the sleep runs proportionally short (documented degradation, not a bug). Like
/// `sleep_ticks`, a no-op outside a scheduled task.
pub fn sleep_ms(ms: u64) {
    sleep_ticks(crate::arch::ms_to_ticks(ms));
}

// ---------------------------------------------------------------------------------------------
// Semaphore — the inter-thread blocking primitive (counting; FIFO waiters)
// ---------------------------------------------------------------------------------------------

/// A counting semaphore for kernel threads. `wait()` blocks when the count is zero; `post()` wakes
/// one waiter (or bumps the count). Waking is cross-CPU aware: a task blocked on CPU B is woken
/// from CPU A by moving it to B's run queue and sending the reschedule IPI.
///
/// MUST outlive every task that can block on it, because `wait()` hands raw pointers to
/// `waiters`/`locked` to the scheduler to be dereferenced after the context switch. Two ways to
/// guarantee that: (1) be `'static` (e.g. a `static SEM`); or (2) be kept alive behind an `Arc`
/// whose clones are held by every party across the park/post window — a blocked waiter holds one on
/// its parked stack, and the poster holds one until it is done posting (this is how `JoinHandle` /
/// `Task::done_sem` use a non-`'static` completion semaphore soundly). Dropping one with parked
/// waiters would leak those tasks and dangle.
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

    /// Non-blocking permit acquire: take a permit and return `true` if one is available RIGHT NOW,
    /// else return `false` without blocking. This is exactly `wait()`'s fast path with the park
    /// removed, so it is safe from ANY context (scheduled task, BSP, idle) — it never switches. Used
    /// by `JoinHandle::join_timeout` to poll a completion semaphore between timed sleeps.
    #[must_use]
    pub fn try_wait(&self) -> bool {
        let was_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
        self.lock_raw();
        let got = if self.count.load(Ordering::Relaxed) > 0 {
            self.count.fetch_sub(1, Ordering::Relaxed);
            true
        } else {
            false
        };
        self.unlock_raw();
        if was_enabled {
            x86_64::instructions::interrupts::enable();
        }
        got
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
    /// FIFO waiter list; touched only under `locked`. Pre-reserved to `capacity` by `init()` /
    /// `init_with_capacity()` so the scheduler's park-side `push_back` never reallocates under the
    /// held lock.
    waiters: UnsafeCell<VecDeque<Box<Task>>>,
    /// Per-instance waiter-list reservation (the number `wait()` asserts the queue never reaches).
    /// Defaults to `WAIT_CAPACITY`; `init_with_capacity(n)` raises it so a `>WAIT_CAPACITY`-reader
    /// `RwLock` is possible (see `RwLock::init_with_reader_capacity`). Set ONCE at init before any
    /// task can block, then only read — `Relaxed` suffices.
    capacity: AtomicUsize,
}

// SAFETY: every access to `waiters` is serialized by `locked`; the park-side push happens while the
// blocker's lock is still held (released by the scheduler after the push), establishing
// happens-before with the next notify — identical to `Semaphore`.
unsafe impl Sync for Condvar {}

impl Condvar {
    /// Construct an empty condition variable with the default `WAIT_CAPACITY` waiter reservation.
    /// `const` so it can initialise a `static`. Behaviour is unchanged from before the capacity
    /// parameter existed — `new()` + `init()` still reserves exactly `WAIT_CAPACITY`.
    pub const fn new() -> Self {
        Condvar {
            locked: AtomicBool::new(false),
            waiters: UnsafeCell::new(VecDeque::new()),
            capacity: AtomicUsize::new(WAIT_CAPACITY),
        }
    }

    /// Reserve the default `WAIT_CAPACITY` waiter slots so the scheduler's park-side push never
    /// reallocates under the held lock. Call once on the BSP before any task can block on this condvar.
    pub fn init(&self) {
        self.init_with_capacity(WAIT_CAPACITY);
    }

    /// Reserve `capacity` waiter slots (in place of the default `WAIT_CAPACITY`) and record that
    /// reservation so `wait()`'s alloc-free-park assert tracks THIS instance's real ceiling. Lifts the
    /// 32-waiter cap for a condvar that must hold more blocked tasks — e.g. the reader queue of a
    /// `>WAIT_CAPACITY`-reader `RwLock`. Call once on the BSP before any task can block. `capacity`
    /// MUST be `>= WAIT_CAPACITY` is NOT required, but it must be `>= 1` and large enough for the peak
    /// simultaneous blocked population, or `wait()` will assert.
    pub fn init_with_capacity(&self, capacity: usize) {
        debug_assert!(capacity >= 1, "Condvar capacity must be >= 1");
        self.capacity.store(capacity, Ordering::Relaxed);
        self.lock_raw();
        unsafe { (*self.waiters.get()).reserve(capacity) };
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
            unsafe { (*self.waiters.get()).len() } < self.capacity.load(Ordering::Relaxed),
            "Condvar waiter overflow (raise this condvar's init_with_capacity)"
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
// RwLock<T> — a writer-preferring reader-writer lock, composed from a Mutex + two Condvars
// ---------------------------------------------------------------------------------------------

/// Internal counters of an `RwLock`, guarded by its inner `Mutex`. The invariant
/// `(readers > 0) XOR writer` is maintained under that mutex; while it holds, readers share `&T` and
/// a writer has an exclusive `&mut T`, so no `&mut T` ever aliases another reference.
struct RwState {
    /// Number of read locks currently held.
    readers: u32,
    /// True while the single write lock is held.
    writer: bool,
    /// Writers currently blocked in `write()` (drives writer-preference: new readers yield to them).
    waiting_writers: u32,
}

/// A sleeping reader-writer lock: many concurrent readers OR one exclusive writer. Composed from the
/// existing primitives (no new unsafe blocking machinery): an inner `Mutex<RwState>` serialises the
/// counters and two `Condvar`s park waiters — exactly the way `Channel` composes a Mutex + Semaphores.
///
/// WRITER-PREFERRING: a new reader yields to any waiting writer, so writers as a CLASS are not
/// starved by readers. This is NOT strict per-writer FIFO, though — a writer woken from `writer_ok`
/// can be leapfrogged by another writer that barges in (takes the just-freed lock before the woken
/// one re-acquires `inner`, sending it back to the queue tail), so an individual writer's progress
/// is guaranteed only against a FINITE writer population, not an unbounded barging stream. On the
/// reader side the cost is heavier: **reader starvation is UNBOUNDED** under sustained write load —
/// a reader blocked on the condvar is `STATE_BLOCKED`, off every run queue, so it accrues NO
/// priority-aging credit (the `AGE_TICKS` anti-starvation mechanism bounds only run-queue waiting,
/// not condvar blocking). Do not use this lock where readers must make progress against a continuous
/// writer stream.
///
/// MUST be `'static` (or Arc-kept-alive) like its parts; call `init()` once on the BSP before use.
///
/// PRECONDITION (load-bearing): at most a condvar's reserved capacity of tasks may be simultaneously
/// blocked on the reader condvar — or on the writer condvar — of a single `RwLock`. The underlying
/// `Condvar` asserts its waiter list never reaches that reservation and PANICS otherwise. Unlike the
/// other primitives (whose waiter counts are naturally bounded by producer/consumer/holder count), an
/// `RwLock`'s reader queue is unbounded BY DESIGN, so a caller MUST bound its concurrent-reader
/// population. With `init()` both queues reserve the default `WAIT_CAPACITY` (32); a lock needing >32
/// simultaneously-blocked readers is constructed by reserving the reader queue for more via
/// `init_with_reader_capacity(n)` (the writer queue stays at the default).
///
/// NOT REENTRANT. A task must hold AT MOST ONE guard (read or write) on a given `RwLock` at a time,
/// and must never call `read()`/`write()` while already holding a guard on it. All four re-entries
/// DEADLOCK PERMANENTLY (the condvars have no timer backstop — a never-coming notify sleeps forever):
///   * read-then-read — the 2nd `read()` yields to a queued writer that waits for `readers == 0`,
///     which the 1st guard prevents. DANGEROUS: this deadlocks ONLY when a writer also happens to be
///     waiting, so it passes tests and then hangs in production.
///   * read-then-write — `write()` waits for `readers == 0`, but the caller is that reader.
///   * write-then-read — `read()` blocks on `writer == true`, which the caller holds.
///   * write-then-write — the 2nd `write()` blocks on `writer == true`.
pub struct RwLock<T> {
    inner: Mutex<RwState>,
    /// Parks readers waiting out a writer; woken (all at once) when the lock clears with no writer
    /// queued.
    readers_ok: Condvar,
    /// Parks writers waiting out readers/a writer; woken one-at-a-time on each release (FIFO within
    /// the condvar queue, though a barging writer may still grab the freed lock first).
    writer_ok: Condvar,
    data: UnsafeCell<T>,
}

// SAFETY: `&T` is handed to MULTIPLE readers on different CPUs at once, so `T: Sync` is REQUIRED
// (this is the key difference from `Mutex<T>`, which needs only `T: Send` because it hands out one
// `&mut T`); a writer's `&mut T` is the data's sole accessor but migrates across CPUs over time, so
// `T: Send`. The `(readers > 0) XOR writer` invariant under `inner` guarantees no `&mut T` aliases.
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    /// Construct an unlocked `RwLock`. `const` so it can initialise a `static`.
    pub const fn new(value: T) -> Self {
        RwLock {
            inner: Mutex::new(RwState { readers: 0, writer: false, waiting_writers: 0 }),
            readers_ok: Condvar::new(),
            writer_ok: Condvar::new(),
            data: UnsafeCell::new(value),
        }
    }

    /// Reserve all three sub-primitives' waiter capacity at the default `WAIT_CAPACITY` (32). Call
    /// once on the BSP before use. Behaviour is unchanged from before the capacity parameter existed.
    pub fn init(&self) {
        self.inner.init();
        self.readers_ok.init();
        self.writer_ok.init();
    }

    /// Like `init`, but reserve the READER condvar for up to `readers` simultaneously-blocked readers
    /// instead of the default 32 — the way to build a `>WAIT_CAPACITY`-reader `RwLock` (the reader
    /// queue is unbounded by design; the writer queue stays at the default, bounded by the writer
    /// population). Call once on the BSP before use. The inner mutex keeps the default reservation:
    /// tasks pass through it only transiently (they end up blocked on a condvar, not the mutex), and
    /// CPU-pinning serialises per-core contenders, so at most ~one-per-CPU ever parks on it at once.
    pub fn init_with_reader_capacity(&self, readers: usize) {
        self.inner.init();
        self.readers_ok.init_with_capacity(readers);
        self.writer_ok.init();
    }

    /// Acquire a shared read lock, blocking until no writer holds OR is waiting (writer-preference).
    /// Returns an RAII guard giving `&T`; releases on drop. MUST be called from a scheduled task (it
    /// may block). NOT reentrant (see the type docs).
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        {
            let mut g = self.inner.lock();
            while g.writer || g.waiting_writers > 0 {
                g = self.readers_ok.wait(g); // re-checks the predicate on every (possibly spurious) wake
            }
            g.readers += 1;
        } // inner guard dropped; our read lock is now recorded in `readers`
        RwLockReadGuard { lock: self, _not_send: PhantomData }
    }

    /// Acquire the exclusive write lock, blocking until no readers and no writer. Returns an RAII
    /// guard giving `&mut T`; releases on drop. MUST be called from a scheduled task. NOT reentrant.
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        {
            let mut g = self.inner.lock();
            g.waiting_writers += 1; // announce ourselves so new readers yield to us (writer-preference)
            while g.writer || g.readers > 0 {
                g = self.writer_ok.wait(g);
            }
            g.waiting_writers -= 1;
            g.writer = true;
        } // inner guard dropped; our write lock is now recorded in `writer`
        RwLockWriteGuard { lock: self, _not_send: PhantomData }
    }
}

/// Shared-read RAII guard from `RwLock::read`. `Deref`s to `&T`; on drop, decrements the reader
/// count and (when it reaches zero) hands the lock to a waiting writer.
pub struct RwLockReadGuard<'a, T> {
    lock: &'a RwLock<T>,
    /// `!Send` like `MutexGuard`: a held lock belongs to the task that took it.
    _not_send: PhantomData<*const ()>,
}

impl<T> Deref for RwLockReadGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: we hold a read lock, so `readers > 0` hence `writer == false`: no `&mut T` exists.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        // Dropping re-acquires the inner sleeping Mutex and therefore MAY BLOCK (park + switch) if it
        // is momentarily contended — sound because the dropping task is a normal scheduled task; but
        // it means a guard must be dropped only from a scheduled task (interrupts on, no spinlock
        // held), the same contexts where `read()`/`write()` may be called.
        debug_assert!(
            SCHED[percpu::this_cpu().cpu_index as usize].current.load(Ordering::Acquire) != 0,
            "RwLockReadGuard dropped off a scheduled task (Drop re-acquires a sleeping Mutex)"
        );
        let wake = {
            let mut g = self.lock.inner.lock();
            g.readers -= 1;
            g.readers == 0
        }; // inner guard dropped BEFORE the notify (keeps the condvar lock strictly inside the inner
           // mutex's critical section and avoids waking readers that would just re-park on `inner`)
        if wake {
            self.lock.writer_ok.notify_one(); // hand off to one waiting writer (no-op if none)
        }
    }
}

/// Exclusive-write RAII guard from `RwLock::write`. `Deref`/`DerefMut` to `&mut T`; on drop, clears
/// the writer flag and wakes the next writer (FIFO) or, if none waits, all parked readers.
pub struct RwLockWriteGuard<'a, T> {
    lock: &'a RwLock<T>,
    _not_send: PhantomData<*const ()>,
}

impl<T> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: we hold the exclusive write lock; no other accessor exists.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: we hold the exclusive write lock; this is the only live reference to the data.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        debug_assert!(
            SCHED[percpu::this_cpu().cpu_index as usize].current.load(Ordering::Acquire) != 0,
            "RwLockWriteGuard dropped off a scheduled task (Drop re-acquires a sleeping Mutex)"
        );
        let wake_writer = {
            let mut g = self.lock.inner.lock();
            g.writer = false;
            g.waiting_writers > 0
        }; // inner guard dropped BEFORE the notify (see RwLockReadGuard::drop)
        if wake_writer {
            self.lock.writer_ok.notify_one(); // FIFO hand-off to the next writer (writer-preference)
        } else {
            self.lock.readers_ok.notify_all(); // no writer waiting: release every parked reader
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
                CPU_BUSY[cpu].fetch_add(1, Ordering::Relaxed); // M3b CPU-pulse meter (introspection)
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

                // U3.5: install the incoming task's ADDRESS SPACE (CR3) here — the single dispatch
                // site that covers BOTH a first entry (was the trampoline's job) AND a resume after
                // preemption (which never re-enters the trampoline). A user task runs under its
                // private `user_cr3`; a kernel task and the cooperative shared-window fixtures
                // (`user_cr3 == 0`) run under the kernel CR3 — so a kernel task never inherits a
                // just-preempted user task's CR3 (which could be freed on that task's teardown). We
                // are IF=0 here (loop top), and "only if different" skips the redundant full-flush
                // `mov cr3` on the common no-switch case.
                let target_cr3 = {
                    let uc = unsafe { (*raw).user_cr3 };
                    if uc != 0 { uc } else { crate::arch::memory::kernel_cr3() }
                };
                unsafe { crate::arch::memory::switch_cr3_if_needed(target_cr3) };

                // U4x: install the incoming task's KERNEL-STACK anchors here too — the M7-twin of the
                // CR3-at-dispatch above. TSS.RSP0 is the stack the CPU switches to on a ring-3
                // fault/IRQ; `syscall_kernel_rsp` is the stack the SYSCALL stub switches to (SYSCALL
                // does not switch stacks itself). Both must name the INCOMING task's own kernel stack.
                // Doing it HERE — the single dispatch site — covers first entry AND resume-after-
                // block/preempt (which never re-enters the trampoline), which is what makes a SECOND
                // concurrent user task per core safe: a syscall/fault from a resumed task lands on its
                // OWN kernel stack, never a just-freed sibling's (the U4x parent/children hazard the
                // trampoline-only install could not cover). We are IF=0 here. Set unconditionally: for
                // a kernel task both anchors are simply never consulted (it never enters from ring 3).
                let ktop = {
                    let s = unsafe { &(*raw).stack };
                    ((s.as_ptr() as usize + s.len()) & !0xF) as u64
                };
                crate::arch::gdt::set_privilege_stack0(cpu, ktop);
                percpu::set_syscall_kernel_rsp(ktop);

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
                    // Frees the stack; for a joinable task this also drops its `done_sem` Arc clone
                    // (possibly the last, freeing the join-sem) — sound: the trampoline already
                    // posted it, no lock is held here, and the heap lock is innermost.
                    STATE_FINISHED => drop(task),
                    STATE_BLOCKED => park_blocked(cpu, park, task),
                    _ => {
                        // READY (yielded or preempted). U3.5: a preemptible never-yielding task whose
                        // KillSwitch has been requested is REAPED here instead of requeued — the only
                        // way to stop a task that never calls `sys_exit`. Teardown MIRRORS `exit`:
                        // restore the kernel CR3 (this task ran under its own CR3, still live here;
                        // that mov-cr3 full-flush retires the slot's user TLB entries) BEFORE freeing
                        // the slot, so no core is left on a dead/reused root. We are IF=0.
                        debug_assert_eq!(park, PARK_NONE, "non-blocked task carried a park action");
                        let reap = task.kill.as_ref().is_some_and(|k| k.is_requested());
                        if reap {
                            if task.user_cr3 != 0 {
                                crate::arch::memory::restore_kernel_cr3();
                                crate::arch::memory::free_user_space_by_cr3(task.user_cr3);
                            }
                            if let Some(k) = &task.kill {
                                k.mark_reaped(); // through the Arc — the requester's clone keeps it live
                            }
                            drop(task); // frees the kstack; the interrupt frame on it is abandoned
                        } else {
                            // Rotate to the back of its (decayed) effective level: `requeue` steps the
                            // transient promotion down by one toward base rather than re-basing, so a
                            // task dispatched mid-climb re-climbs at most one level (the aging refinement).
                            task.state.store(STATE_READY, Ordering::Release);
                            RUN_QUEUES[cpu].lock().requeue(task);
                        }
                    }
                }
            }
            None => {
                // Nothing to run: sleep until an interrupt (timer or a `spawn`/wake IPI). The `sti;
                // hlt` pair is atomic — an interrupt latched before the `sti` still fires and
                // returns past the `hlt`, so a wake that arrived in the empty-check window is not
                // lost. On wake we loop back to the top (which `cli`s) and re-check the queue +
                // sleepers.
                CPU_IDLE[cpu].fetch_add(1, Ordering::Relaxed); // M3b CPU-pulse meter (introspection)
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

/// VUG-1 M3b: number of CPUs the "CPU pulse" meter should show (online cores, capped at MAX_CPUS).
pub fn meter_cpu_count() -> usize {
    core::cmp::min(crate::arch::acpi::cpu_count().max(1), MAX_CPUS)
}

/// VUG-1 M3b: cumulative `(busy, idle)` dispatch/idle counts for `cpu` (see `CPU_BUSY`/`CPU_IDLE`).
/// The demo diffs these across a frame window to derive a per-core load fraction. Introspection only.
pub fn meter_cpu_ticks(cpu: usize) -> (u64, u64) {
    if cpu >= MAX_CPUS {
        return (0, 0);
    }
    (CPU_BUSY[cpu].load(Ordering::Relaxed), CPU_IDLE[cpu].load(Ordering::Relaxed))
}

/// VUG-HONESTY: linear index of the calling core (the vug/pulse "demo core"). Arch-neutral mirror of
/// the aarch64 accessor, same `gs:[0]` self-lookup shape as `current_name`. The shared `vug` CPU-pulse
/// display credits its render load only to this core; other frozen-counter cores read parked, never a
/// fabricated bar. Introspection only — no scheduling-path effect.
pub fn meter_current_cpu() -> usize {
    percpu::this_cpu().cpu_index as usize
}

/// Name of the task currently running on THIS CPU, or `None` if the CPU is idle. `name` is a
/// `&'static str`, so it stays valid even after the Box is reclaimed. Used by the U1b ring-3
/// fault-kill log (`interrupts::ring3_fault_kill`), which runs on this CPU with GS already
/// restored to `PerCpuData` — the same lookup shape as `current_task_id`, keyed to the current CPU.
pub fn current_name() -> Option<&'static str> {
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *const Task;
    if raw.is_null() {
        None
    } else {
        Some(unsafe { (*raw).name })
    }
}

/// The CURRENT task's user CR3 (`0` for a kernel task), or `None` if no task is current on this CPU
/// (the scheduler-reaper context — `current` is `0` there — or the unscheduled BSP). STOR-1 S4c uses
/// this in the teardown path to tell a SELF-teardown (`exit`/reap of the current ring-3 task, which
/// runs IF=0 mid-death and MUST NOT block on the storage service task) from a launcher tearing down
/// ANOTHER slot (a live scheduled task that MAY block): a launcher is a kernel task, so its
/// `user_cr3` is `0` and never equals the slot's CR3 it is freeing, whereas `exit`'s current task IS
/// that slot. Keyed to the current CPU (GS already restored in every teardown caller).
pub fn current_user_cr3() -> Option<u64> {
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *const Task;
    if raw.is_null() {
        None
    } else {
        Some(unsafe { (*raw).user_cr3 })
    }
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

// RwLock demo state. A writer bumps a shared (u64,u64) NON-atomically (store .0, gap, store .1)
// while readers across APs read-share it and assert the two halves match — a torn (.0 != .1) read
// would prove the lock let a reader in during a write. Writer-preference; RW_READERS is small so at
// most 4 readers ever block on the reader condvar (<< WAIT_CAPACITY=32).
/// Shared data with the invariant .0 == .1, mutated only under the write lock.
static RWL: RwLock<(u64, u64)> = RwLock::new((0, 0));
/// Set true if any reader ever observed a torn (.0 != .1) value — the lock failing to exclude a writer.
static RW_TORN: AtomicBool = AtomicBool::new(false);
/// Readers currently inside the read section (used to witness that reads actually OVERLAP = sharing).
static RW_CUR_READERS: AtomicUsize = AtomicUsize::new(0);
/// High-water mark of `RW_CUR_READERS` (a soft witness that read-sharing happened).
static RW_MAX_READERS: AtomicUsize = AtomicUsize::new(0);
/// Count of reader + writer tasks that have finished (the verifier waits for all of them).
static RW_DONE: AtomicUsize = AtomicUsize::new(0);
/// Number of reader tasks. Kept small so blocked readers stay well under `WAIT_CAPACITY`.
const RW_READERS: usize = 4;
/// Read iterations per reader; write iterations by the single writer.
const RW_READS: u32 = 8;
const RW_WRITES: u64 = 8;

/// Turn scheduling on and spawn the demo workload across the online APs: equal-priority round-robin
/// + timer preemption (the busy pair on the first AP), and the RWLOCK showcase — a writer mutates a
/// shared value under the write lock while readers across other APs read-share it and check for torn
/// reads, with a verifier bounding it. Called once on the BSP after `smp::start_aps` (hence after
/// `verify_smp`). No-op with no APs; the rwlock showcase needs >=2 APs (else SKIPPED).
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

    // KERNEL-CLOCK witnesses (M2 sleep_ms, M3 join_timeout). Both are sleep-driven and self-checking,
    // so they run on ANY topology (even a single AP — unlike the RwLock showcase). Pinned to the LAST
    // AP to keep them off the busy pair on AP[0], so the busy tasks don't perturb the ms measurement;
    // with one AP they share it, which the 2x tolerance absorbs.
    let cpu_clock = online_aps[online_aps.len() - 1];
    spawn("sleep-ms", demo_sleep_ms, 0, cpu_clock, PRIO_NORMAL);
    spawn("join-tmo", demo_join_timeout, 0, cpu_clock, PRIO_NORMAL);

    // RwLock (writer-preferring, composed from Mutex + 2 Condvars): a writer bumps a shared (u64,u64)
    // non-atomically while RW_READERS readers read-share it and assert the two halves match. A torn
    // read would prove the lock let a reader in during a write. Each reader holds the read section
    // across a short sleep, so co-located OR cross-AP readers get admitted concurrently (reader
    // overlap, max>1, is the soft sharing witness; readers spread across the non-busy APs when >=3
    // exist). PASS = all done AND no torn read. A verifier on the last AP polls with a
    // timeout, so a self-deadlock / lost wakeup is a printed FAIL, never a silent hang. Readers stay
    // OFF the busy AP and are few (<=4 ever blocked << WAIT_CAPACITY=32). Needs >=2 APs.
    if online_aps.len() >= 2 {
        RWL.init(); // reserve the inner mutex + both condvars' waiter lists before anyone can block
        let cpu_w = online_aps[1];
        spawn("rw-writer", demo_rw_writer, cpu_w, cpu_w, PRIO_NORMAL);
        let n = online_aps.len();
        for i in 0..RW_READERS {
            let cpu_r = online_aps[1 + (i % (n - 1))]; // spread readers across the non-busy APs
            spawn("rw-reader", demo_rw_reader, i, cpu_r, PRIO_NORMAL);
        }
        let cpu_verify = online_aps[n - 1];
        spawn("rw-verify", demo_rw_verifier, cpu_verify, cpu_verify, PRIO_NORMAL);
    } else {
        serial_println!("RWLOCK: SKIPPED (needs >=2 APs; have {})", online_aps.len());
    }

    // AGEREF (M1): witness that the `effective_level` refinement tightens the aging bound. One LOW
    // victim runs the SAME work twice under continuous HIGH-priority load on one AP: phase CTL blocks
    // between iterations (wake re-enqueues at BASE — the old reset-on-dispatch behavior), phase REF
    // yields between iterations (re-enqueue DECAYS one level — the refinement). The refined phase must
    // be measurably faster (re-climbs one level, not from base). Self-calibrating (both phases share
    // the same load/jitter), so it is robust to QEMU timing; a verifier bounds it (watchdog). Pinned
    // to a NON-clock AP so it never perturbs the SLEEPMS/JOINTMO wall-clock witnesses: the middle AP
    // when >=3 exist (only the blocked-heavy RwLock tasks live there — delaying them is watchdog-safe),
    // else the busy AP (its round-robin regression has no timing assert).
    let ageref_ap = if online_aps.len() >= 3 { online_aps[1] } else { online_aps[0] };
    let ageref_verify_ap = online_aps[online_aps.len() - 1];
    for i in 0..AGEREF_LOAD {
        spawn("ageref-load", demo_ageref_load, i, ageref_ap, PRIO_HIGH);
    }
    spawn("ageref-victim", demo_ageref_victim, 0, ageref_ap, PRIO_LOW);
    spawn("ageref-verify", demo_ageref_verifier, ageref_verify_ap, ageref_verify_ap, PRIO_NORMAL);

    // CVCAP (M2): witness a >WAIT_CAPACITY (32) reader RwLock. A writer holds the write lock while
    // CV_READERS (40) readers all pile up blocked on the reader condvar — only possible because the
    // lock reserved its reader queue past 32 via `init_with_reader_capacity`. When the writer releases,
    // notify_all wakes them all; PASS = all finish, no torn read, and the high-water blocked-reader
    // count exceeded 32 (proving the raised capacity was genuinely exercised). Needs >=2 APs.
    if online_aps.len() >= 2 {
        RWL2.init_with_reader_capacity(CV_READER_CAP);
        let n = online_aps.len();
        let cpu_cv = online_aps[n - 1];
        spawn("cv-writer", demo_cvcap_writer, 0, cpu_cv, PRIO_NORMAL);
        for i in 0..CV_READERS {
            let cpu_r = online_aps[i % n]; // spread the 40 readers across every AP
            spawn("cv-reader", demo_cvcap_reader, i, cpu_r, PRIO_NORMAL);
        }
        spawn("cv-verify", demo_cvcap_verifier, cpu_cv, cpu_cv, PRIO_NORMAL);
    } else {
        serial_println!("CVCAP: SKIPPED (needs >=2 APs; have {})", online_aps.len());
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

/// RwLock writer: take the exclusive write lock and bump the shared (u64,u64) to an incrementing
/// value NON-atomically — store .0, sleep one tick, then store .1 — so a reader admitted mid-write
/// (a broken lock) would observe a torn .0 != .1. Holding the write lock across the sleep keeps it
/// exclusive throughout, so a correct lock lets NO reader see the torn state.
fn demo_rw_writer(_arg: usize) {
    let cpu = percpu::this_cpu().cpu_index as usize;
    for v in 1..=RW_WRITES {
        {
            let mut g = RWL.write();
            g.0 = v;
            sleep_ticks(1); // straddle the two stores, still holding the exclusive write lock
            g.1 = v;
        } // write guard dropped -> wakes the next writer, or all parked readers
        sleep_ticks(1); // give readers a turn between writes
    }
    serial_println!("SCHED: [cpu{} rw-writer] {} writes done", cpu, RW_WRITES);
    RW_DONE.fetch_add(1, Ordering::Release);
}

/// RwLock reader: repeatedly take a shared read lock, witness reader overlap (CUR/MAX), hold the read
/// section across a short sleep so a co-located reader is admitted concurrently, and assert the two
/// halves match (a torn read => the lock failed to exclude the writer). Read-sharing lets many of
/// these be in their section at once.
fn demo_rw_reader(idx: usize) {
    let cpu = percpu::this_cpu().cpu_index as usize;
    for _ in 0..RW_READS {
        {
            let g = RWL.read();
            let cur = RW_CUR_READERS.fetch_add(1, Ordering::AcqRel) + 1;
            RW_MAX_READERS.fetch_max(cur, Ordering::AcqRel);
            sleep_ticks(2); // hold the read section so another reader overlaps (the sharing witness)
            if g.0 != g.1 {
                RW_TORN.store(true, Ordering::Release); // torn read: the lock admitted us mid-write
            }
            RW_CUR_READERS.fetch_sub(1, Ordering::AcqRel);
        } // read guard dropped
        sleep_ticks(1);
    }
    serial_println!("SCHED: [cpu{} rw-reader {}] {} reads done", cpu, idx, RW_READS);
    RW_DONE.fetch_add(1, Ordering::Release);
}

/// RwLock verifier on the last AP: poll until all readers + the writer finish (or a generous timeout
/// = the watchdog), then print one self-checking line. PASS = all done AND no torn read ever seen; a
/// timeout (self-deadlock / lost wakeup) => FAIL. Reader overlap (max>1) is a soft witness that
/// read-sharing happened — its absence is reported as INCONCLUSIVE, not a FAIL, since CPU-pinned
/// scheduling can serialise readers without the lock being wrong.
fn demo_rw_verifier(cpu: usize) {
    let total = RW_READERS + 1; // RW_READERS readers + the one writer
    let mut waited = 0u64;
    while RW_DONE.load(Ordering::Acquire) < total && waited < 8_000 {
        sleep_ticks(8);
        waited += 8;
    }
    let done = RW_DONE.load(Ordering::Acquire);
    let torn = RW_TORN.load(Ordering::Acquire);
    let maxr = RW_MAX_READERS.load(Ordering::Acquire);
    let pass = done == total && !torn;
    let witness = if pass && maxr <= 1 {
        " (INCONCLUSIVE overlap: readers did not visibly overlap)"
    } else {
        ""
    };
    serial_println!(
        "RWLOCK: [cpu{}] done {}/{}, torn={}, max_concurrent_readers={} => {}{}",
        cpu,
        done,
        total,
        torn,
        maxr,
        if pass { "PASS" } else { "FAIL" },
        witness
    );
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------------------------
// AGEREF demo witness (M1: the `effective_level` aging refinement)
// ---------------------------------------------------------------------------------------------

/// Number of continuous HIGH-priority load tasks that starve the LOW victim on the AGEREF AP.
const AGEREF_LOAD: usize = 2;
/// Iterations the victim runs per phase. Each iteration it gives up the CPU (block in CTL, yield in
/// REF) and must age back up to be redispatched — so this many aging climbs are timed per phase.
const AGEREF_ITERS: u32 = 12;
/// Safety cap (local ticks) on the load spinners, in case the victim ever wedged — they self-stop so
/// the AP is never held forever even if `AGEREF_STOP` were somehow never set.
const AGEREF_LOAD_MAX_TICKS: u64 = 20_000;
/// PASS threshold: the refined (yield/decay) phase must be at most this percent of the control
/// (block/reset) phase. Well below 100 (the refinement roughly halves the re-climb), yet clear of the
/// few-tick block-vs-yield overhead that would make CTL trivially slower even with no refinement.
const AGEREF_MAX_PCT: u64 = 75;

/// Set by the victim when both phases are measured; the load spinners exit and the verifier proceeds.
static AGEREF_STOP: AtomicBool = AtomicBool::new(false);
/// Ticks the CONTROL phase took (block-between-iters → wake re-enqueues at BASE = the old behavior).
static AGEREF_CTL_ELAPSED: AtomicU64 = AtomicU64::new(0);
/// Ticks the REFINED phase took (yield-between-iters → re-enqueue DECAYS one level = the refinement).
static AGEREF_REF_ELAPSED: AtomicU64 = AtomicU64::new(0);
/// Set once the victim has recorded both elapsed measurements (distinguishes "done" from a timeout).
static AGEREF_DONE: AtomicBool = AtomicBool::new(false);

/// Continuous HIGH-priority load: spin so the LOW victim only ever runs by aging up to parity. Never
/// sleeps/yields (that would relieve the starvation pressure); the timer preempts it and `requeue`
/// keeps it at its base HIGH level. Exits when the victim signals `AGEREF_STOP` (or a safety cap).
fn demo_ageref_load(_idx: usize) {
    let start = percpu::this_cpu().ticks.load(Ordering::Relaxed);
    let mut acc: u64 = 0;
    while !AGEREF_STOP.load(Ordering::Acquire) {
        for i in 0..200_000u64 {
            acc = acc.wrapping_add(i);
        }
        core::hint::black_box(acc);
        if percpu::this_cpu().ticks.load(Ordering::Relaxed).wrapping_sub(start) > AGEREF_LOAD_MAX_TICKS {
            break; // safety: never hold the AP forever
        }
    }
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

/// AGEREF victim (PRIO_LOW). Runs the same tiny workload twice under the HIGH load, differing only in
/// how it yields the CPU between iterations — which selects the re-enqueue path being compared:
///   * CONTROL: `sleep_ticks(0)` → the task BLOCKS and is woken via `make_ready`/`push` → re-enqueued
///     at BASE (level 0). Each iteration it must re-climb the FULL distance to parity (the old
///     reset-on-dispatch behavior).
///   * REFINED: `yield_now()` → the task re-enqueues via `requeue` → its effective level DECAYS by one
///     rather than resetting, so it re-climbs at most one level (the refinement).
/// Both phases run back-to-back under identical load, so absolute QEMU timing jitter cancels in the
/// ratio; the refined phase should be markedly faster. Local ticks (this task is CPU-pinned) time each
/// phase. Aging guarantees progress, so it cannot hang — but the verifier still watchdogs it in case a
/// regression broke aging entirely (then it would starve and never set `AGEREF_DONE`).
fn demo_ageref_victim(_arg: usize) {
    let cpu = percpu::this_cpu().cpu_index as usize;

    // CONTROL phase: block between iterations (re-base to base on each wake).
    let t0 = percpu::this_cpu().ticks.load(Ordering::Relaxed);
    for _ in 0..AGEREF_ITERS {
        core::hint::black_box(cpu);
        sleep_ticks(0); // block + immediate wake → push() re-bases to level 0
    }
    let ctl = percpu::this_cpu().ticks.load(Ordering::Relaxed).wrapping_sub(t0);

    // REFINED phase: yield between iterations (decay one level per dispatch).
    let t1 = percpu::this_cpu().ticks.load(Ordering::Relaxed);
    for _ in 0..AGEREF_ITERS {
        core::hint::black_box(cpu);
        yield_now(); // re-enqueue via requeue() → effective_level decays by one
    }
    let refined = percpu::this_cpu().ticks.load(Ordering::Relaxed).wrapping_sub(t1);

    AGEREF_CTL_ELAPSED.store(ctl, Ordering::Release);
    AGEREF_REF_ELAPSED.store(refined, Ordering::Release);
    AGEREF_DONE.store(true, Ordering::Release);
    AGEREF_STOP.store(true, Ordering::Release); // release the load spinners
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

/// AGEREF verifier: poll until the victim finishes both phases (or a generous timeout = the watchdog),
/// then print one self-checking line. PASS = the victim finished AND the refined phase was <=
/// `AGEREF_MAX_PCT`% of the control phase (the refinement measurably tightened the re-climb). A
/// timeout (aging broke → the LOW victim starved forever) => FAIL.
fn demo_ageref_verifier(cpu: usize) {
    let mut waited = 0u64;
    while !AGEREF_DONE.load(Ordering::Acquire) && waited < 12_000 {
        sleep_ticks(16);
        waited += 16;
    }
    let done = AGEREF_DONE.load(Ordering::Acquire);
    let ctl = AGEREF_CTL_ELAPSED.load(Ordering::Acquire);
    let refined = AGEREF_REF_ELAPSED.load(Ordering::Acquire);
    // Refined must be at most AGEREF_MAX_PCT% of control (guard against ctl==0 divide/degenerate).
    let pass = done && ctl > 0 && refined > 0 && refined * 100 <= ctl * AGEREF_MAX_PCT;
    serial_println!(
        "AGEREF: [cpu{}] refined={} ctl={} ticks (refined<={}% ctl), done={} => {}",
        cpu,
        refined,
        ctl,
        AGEREF_MAX_PCT,
        done,
        if pass { "PASS" } else { "FAIL" }
    );
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------------------------
// CVCAP demo witness (M2: Condvar::init_with_capacity → a >32-reader RwLock)
// ---------------------------------------------------------------------------------------------

/// Reader tasks that pile up blocked on ONE RwLock's reader condvar simultaneously — chosen > the
/// default `WAIT_CAPACITY` (32) so the run REQUIRES the raised reservation (else `Condvar::wait`
/// asserts). The whole point of `init_with_capacity`.
const CV_READERS: usize = 40;
/// Reader-condvar reservation for `RWL2` (> `CV_READERS`, since `wait()` asserts `len < capacity`).
const CV_READER_CAP: usize = 48;

/// A second RwLock, reserved past 32 readers via `init_with_reader_capacity`. Same torn-read invariant
/// as `RWL`: `.0 == .1` under the write lock.
static RWL2: RwLock<(u64, u64)> = RwLock::new((0, 0));
/// Set true if any reader saw a torn (.0 != .1) value.
static CV_TORN: AtomicBool = AtomicBool::new(false);
/// Readers that have begun their `read()` attempt (the writer waits for all of them before releasing).
static CV_STARTED: AtomicUsize = AtomicUsize::new(0);
/// Readers currently INSIDE `read()` (i.e. blocked on the reader condvar while the writer holds it).
static CV_WAITING: AtomicUsize = AtomicUsize::new(0);
/// High-water mark of `CV_WAITING` — the witness that >32 readers were simultaneously blocked.
static CV_MAX_WAITING: AtomicUsize = AtomicUsize::new(0);
/// Readers that have finished (the verifier waits for all of them).
static CV_DONE: AtomicUsize = AtomicUsize::new(0);
/// Set by the writer once it HOLDS the write lock — readers wait for this so they are guaranteed to
/// block (rather than racing in before the writer and never contributing to the blocked high-water).
static CV_WRITER_READY: AtomicBool = AtomicBool::new(false);

/// CVCAP writer: take the write lock, publish `CV_WRITER_READY`, then HOLD it (sleeping, so it never
/// hogs a core) until every reader has piled up blocked on the reader condvar — at which point >32 are
/// parked on one condvar, which only the raised reservation permits. Releasing wakes them all at once.
fn demo_cvcap_writer(_arg: usize) {
    let cpu = percpu::this_cpu().cpu_index as usize;
    let mut g = RWL2.write();
    g.0 = 1;
    g.1 = 1; // a consistent value; any torn read a reader sees would mean the lock failed
    CV_WRITER_READY.store(true, Ordering::Release);
    // Hold until all readers have entered read() (bounded, so a lost reader can't hang the writer).
    let mut waited = 0u64;
    while CV_STARTED.load(Ordering::Acquire) < CV_READERS && waited < 8_000 {
        sleep_ticks(4);
        waited += 4;
    }
    sleep_ticks(24); // settle: let the last few readers actually park on the condvar
    drop(g); // release the write lock → notify_all wakes every parked reader
    serial_println!("SCHED: [cpu{} cv-writer] released; {} readers started", cpu, CV_STARTED.load(Ordering::Acquire));
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

/// CVCAP reader: wait for the writer to hold the lock, then take a read lock — which BLOCKS on the
/// reader condvar until the writer releases. `CV_WAITING` tracks how many are blocked at once (its
/// high-water is the >32 witness). On wake, check the value is not torn, then finish.
fn demo_cvcap_reader(_idx: usize) {
    // Wait until the writer holds the lock so we are guaranteed to block (bounded).
    let mut waited = 0u64;
    while !CV_WRITER_READY.load(Ordering::Acquire) && waited < 8_000 {
        sleep_ticks(2);
        waited += 2;
    }
    CV_STARTED.fetch_add(1, Ordering::AcqRel);
    let cur = CV_WAITING.fetch_add(1, Ordering::AcqRel) + 1;
    CV_MAX_WAITING.fetch_max(cur, Ordering::AcqRel);
    let g = RWL2.read(); // blocks on the reader condvar until the writer releases
    CV_WAITING.fetch_sub(1, Ordering::AcqRel);
    if g.0 != g.1 {
        CV_TORN.store(true, Ordering::Release);
    }
    drop(g);
    CV_DONE.fetch_add(1, Ordering::Release);
}

/// CVCAP verifier: poll until all readers finish (or a generous timeout = the watchdog), then print
/// one self-checking line. PASS = all readers finished AND no torn read AND the blocked high-water
/// exceeded 32 (proving the >WAIT_CAPACITY reservation was actually exercised). A timeout => FAIL.
fn demo_cvcap_verifier(cpu: usize) {
    let mut waited = 0u64;
    while CV_DONE.load(Ordering::Acquire) < CV_READERS && waited < 12_000 {
        sleep_ticks(16);
        waited += 16;
    }
    let done = CV_DONE.load(Ordering::Acquire);
    let torn = CV_TORN.load(Ordering::Acquire);
    let maxw = CV_MAX_WAITING.load(Ordering::Acquire);
    let pass = done == CV_READERS && !torn && maxw > WAIT_CAPACITY;
    serial_println!(
        "CVCAP: [cpu{}] done {}/{}, torn={}, max_blocked_readers={} (cap {}, need >{}) => {}",
        cpu,
        done,
        CV_READERS,
        torn,
        maxw,
        CV_READER_CAP,
        WAIT_CAPACITY,
        if pass { "PASS" } else { "FAIL" }
    );
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------------------------
// KERNEL-CLOCK demo witnesses (M2 sleep_ms, M3 join_timeout)
// ---------------------------------------------------------------------------------------------

/// M2 target: sleep this many ms and measure the actual delay against the invariant TSC.
const SLEEP_MS_TARGET: u64 = 100;

/// M2 self-test: sleep a known wall-clock interval via `sleep_ms`, measure the ACTUAL elapsed time
/// against an INDEPENDENT reference (the invariant TSC, read with `now_cycles`, which advances
/// regardless of interrupt delivery), and print a self-checking `=> PASS/FAIL`. The TSC is the right
/// reference precisely because it does NOT derive from the APIC-timer ISR the sleep depends on, so a
/// broken calibration (wrong tick rate) or a lost wake shows up as a wildly wrong measured duration.
///
/// Tolerance is a generous 2x band: under QEMU/TCG the timer-delivery cadence is loose, but a gross
/// miss (uncalibrated tick ~0.8 ms, or a hang) falls well outside it. Cannot hang — the `sleep_ms`
/// bounds it — so no watchdog is needed. If the TSC never calibrated (no PM timer), there is no wall
/// reference, so it reports SKIPPED rather than a meaningless verdict.
fn demo_sleep_ms(_arg: usize) {
    let cpu = percpu::this_cpu().cpu_index as usize;
    let hz = apic::tsc_hz();
    if hz == 0 {
        serial_println!("SLEEPMS: [cpu{}] SKIPPED (TSC uncalibrated; no wall-clock reference)", cpu);
        DEMO_DONE.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let t0 = crate::arch::now_cycles();
    sleep_ms(SLEEP_MS_TARGET);
    let elapsed_ms = (crate::arch::now_cycles().wrapping_sub(t0) as u128 * 1000 / hz as u128) as u64;
    let (lo, hi) = (SLEEP_MS_TARGET / 2, SLEEP_MS_TARGET * 2);
    let pass = (lo..=hi).contains(&elapsed_ms);
    serial_println!(
        "SLEEPMS: [cpu{}] slept {} ms (target {}, tol [{},{}], TSC ref) => {}",
        cpu,
        elapsed_ms,
        SLEEP_MS_TARGET,
        lo,
        hi,
        if pass { "PASS" } else { "FAIL" }
    );
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

// M3 (join_timeout) demo tuning. A "hung" stand-in sleeps far past the joiner's short timeout (so the
// joiner must give up => TimedOut); a "quick" task finishes well within a long timeout (=> Completed).
const JT_TIMEOUT_MS: u64 = 40; // joiner's patience for the hung task
const JT_HANG_MS: u64 = 400; // hung task's lifetime (>> JT_TIMEOUT_MS, so the joiner times out)
const JT_WAIT_MS: u64 = 400; // joiner's patience for the quick task (>> JT_QUICK_MS)
const JT_QUICK_MS: u64 = 15; // quick task's lifetime (<< JT_WAIT_MS, so the joiner sees Completed)

/// A stand-in for a hung / never-returning task: it simply outlives the joiner's timeout. (It does
/// eventually return so it frees within the test window rather than truly leaking — the join side has
/// already observed `TimedOut` long before.)
fn demo_jt_hung(_arg: usize) {
    sleep_ms(JT_HANG_MS);
}

/// A task that finishes promptly, so a join with a generous timeout observes `Completed`.
fn demo_jt_quick(_arg: usize) {
    sleep_ms(JT_QUICK_MS);
}

/// M3 self-test: exercise BOTH `join_timeout` outcomes from one coordinator task. (a) join a hung
/// task with a short timeout — must return `TimedOut` (the whole point: a hung task can't trap the
/// joiner). (b) join a quick task with a generous timeout — must return `Completed`. Both joined
/// tasks are pinned to this CPU, so the coordinator's timed sleeps yield the core to them and the
/// scheduler interleaves all three. PASS = TimedOut AND Completed, exactly. Bounded by both timeouts,
/// so it cannot hang — no watchdog needed.
fn demo_join_timeout(_arg: usize) {
    let cpu = percpu::this_cpu().cpu_index as usize;

    let hung = spawn_joinable("jt-hung", demo_jt_hung, 0, cpu, PRIO_NORMAL);
    let r_hung = hung.join_timeout(crate::arch::ms_to_ticks(JT_TIMEOUT_MS));

    let quick = spawn_joinable("jt-quick", demo_jt_quick, 0, cpu, PRIO_NORMAL);
    let r_quick = quick.join_timeout(crate::arch::ms_to_ticks(JT_WAIT_MS));

    let pass = r_hung == JoinResult::TimedOut && r_quick == JoinResult::Completed;
    serial_println!(
        "JOINTMO: [cpu{}] hung(t/o {}ms)=>{:?}, quick(t/o {}ms)=>{:?} => {}",
        cpu,
        JT_TIMEOUT_MS,
        r_hung,
        JT_WAIT_MS,
        r_quick,
        if pass { "PASS" } else { "FAIL" }
    );
    DEMO_DONE.fetch_add(1, Ordering::Relaxed);
}

/// How many demo tasks have finished (for headless verification).
pub fn demo_done() -> usize {
    DEMO_DONE.load(Ordering::Relaxed)
}
