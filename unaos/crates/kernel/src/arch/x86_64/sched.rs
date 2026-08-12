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
///
/// `pub` so a fixture that must observe N PREEMPTIONS sizes its window from the scheduler's own
/// quantum rather than restating it (U3.5's property-(c) window, `syscall::U3_5_OBS_IRQS`). A second
/// copy of this number is exactly the kind of constant that survives a change to this one.
pub const QUANTUM_TICKS: u32 = 4;

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
///
/// TEARDOWN-1 — "at a preemption boundary" was the whole defect. A kill was ARMED and then delivered
/// only from `run()`'s READY arm, i.e. only to a task that switched back yielded-or-preempted. A task
/// PARKED in a kernel wait switches back BLOCKED, never reaches that arm, and — if nothing ever wakes
/// it — never reaches any arm at all: `bg_kill` reported "kill armed — the task retires at its next
/// preemption" forever, and the row, the address-space slot and the compositor window it owned were
/// immortal. That is what the WINX-2 kill leg timed out on.
///
/// The kill is now delivered at THREE boundaries, and arming REACHES a parked target rather than
/// waiting for one:
///   * [`kill_check_current`] — the syscall boundary. A killed task retires at its next syscall
///     return, which for any program that talks to the kernel at all is the prompt case.
///   * `run()`'s READY arm — unchanged, and still the only boundary a never-syscalling spinner has.
///   * [`kill_wake_parked`] — called from `request` itself, so a target already parked on a futex or
///     a sleep is EVICTED through that park kind's own wake path (`make_ready`, exactly as
///     `futex_wake` / `drain_due_sleepers` do) and so arrives at the syscall boundary above. Nothing
///     mutates scheduler state behind a park's back: the eviction takes precisely the lock the
///     matching wake already takes, in the same order, and re-readies outside it.
/// The converse race — arm, THEN park — is closed at the two park sites (`futex_wait`, and the
/// PARK_SLEEP arm of `park_blocked`) by testing the flag under the very lock that publishes the Box
/// into its wait structure. That is what makes the pair lossless rather than merely likely: the
/// arming store precedes the eviction scan, and the scan takes that same lock, so a parker whose
/// under-lock test reads `false` is guaranteed to have published before the scan can look.
pub struct KillSwitch {
    requested: AtomicBool,
    reaped: AtomicBool,
}

impl KillSwitch {
    /// A fresh, un-requested kill switch. `const` so a fixture can build one before the Arc.
    pub const fn new() -> Self {
        KillSwitch { requested: AtomicBool::new(false), reaped: AtomicBool::new(false) }
    }
    /// Request termination at the target's next kill boundary (idempotent).
    ///
    /// TEARDOWN-1: arming also EVICTS a target that is already parked in a kernel wait, strictly after
    /// the publishing store above (so the eviction scan's under-lock test cannot read a stale `false`).
    /// Arming and evicting are one operation deliberately — there is no way for a caller to arm a kill
    /// without reaching a parked target, which is exactly the hole this closes.
    pub fn request(&self) {
        self.requested.store(true, Ordering::Release);
        let evicted = kill_wake_parked();
        if evicted > 0 {
            serial_println!(
                "[killbound-x86] kill armed — {} task(s) evicted from a kernel wait to reach a kill boundary",
                evicted
            );
        }
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
    /// SMPBAL-X86: `true` = an idle core may STEAL this task out of its run queue ([`try_steal`]);
    /// `false` = it is PINNED to `cpu` and never migrates. The PIN CONTRACT: set once at spawn as
    /// `requested_cpu == CPU_AUTO`, so a caller that named a core gets exactly the core it named,
    /// forever, and only a caller that said "place me" is eligible to be re-placed later. Every
    /// pre-existing spawn site in the tree names a core, so all of them stay pinned and behave as
    /// before this arc — including render / input / usb-pump, which is why `SCHED-X86 PLACE-CHECK`
    /// needs no exemption and must not be given one.
    ///
    /// Read only under the owning run-queue's lock (`RunQueue::steal_one`); never mutated — the
    /// stealer re-homes `cpu`, not this. Unlike aarch64, ring-3 tasks are NOT categorically excluded:
    /// x86 has no ASID and no per-`(core, slot)` residency table, so `cpu` is a pure policy field on
    /// this arch. See `steal_one` for the one class that IS excluded, and `AS_GEN` in `memory.rs` for
    /// the TLB obligation migration creates here.
    steal_ok: bool,
    /// VUGSPREAD: how many times [`try_steal`] has MIGRATED this task since it was spawned.
    ///
    /// Written only by the thief, at step 3 of the steal protocol, where it owns the popped Box
    /// exclusively — so it needs no atomicity and no lock beyond the pop's own.
    ///
    /// It exists because a moves COUNT cannot tell balancing from ping-pong, and that distinction is
    /// the whole lesson aarch64 paid three arcs for. A fleet that SETTLES shows several distinct
    /// tasks each migrating once; a fleet that THRASHES shows one task's counter climbing. The
    /// per-steal witness prints it, and [`STEAL_REMIGS`] counts the steals that found it already
    /// non-zero — so the churn question survives past `STEAL_LOG_MAX`, on the rollup, rather than
    /// going quiet exactly when a long run would start to answer it.
    migrations: u32,
    /// VUGSPREAD-COOL: `arch::ms()` at this task's LAST migration (`0` = never migrated). The
    /// ping-pong brake for the RUNNING-victim floor reads it: a task stolen less than
    /// [`STEAL_COOLDOWN_MS`] ago is left where it is, so a freshly-stolen vug runs at least one
    /// scheduling quantum on its new home before another idle core may yank it back.
    ///
    /// Denominated in `ms()`, NOT `now_cycles()`, on purpose — the stamp is written by one thief core
    /// and read by another, and `rdtsc` is per-core (the same cross-core hazard the `busy_pct` contract
    /// and `pick_cpu`'s `live=0` note guard against); `ms()` (`APIC_TICKS`) is globally coherent, so
    /// `ms() - migrate_ms` is a sound cross-core elapsed. Written by the thief at step 3 of the steal,
    /// under the pop's own lock where it owns the Box exclusively (same discipline as `migrations`);
    /// read under the victim's lock in [`RunQueue::steal_one`]. A NEVER-migrated task carries `0`, and
    /// `ms() - 0` is always past the cooldown, so the FIRST corrective steal of any task is never
    /// delayed — only a re-steal within the window is.
    migrate_ms: u64,
    /// VUGSPREAD (review F16): this task's core came from a RING-3 PLACEMENT HINT — the `place`
    /// argument of `SYS_THREAD_SPAWN`, resolved by the kernel to an index. True for exactly the
    /// population that the pin contract used to freeze and this arc released; false for every kernel
    /// spawn site and for `CPU_AUTO` processes, which were already movable.
    ///
    /// Attribution only. It is never read on a scheduling path — `steal_ok` alone governs
    /// eligibility, and giving this field any authority would quietly recreate the two-class
    /// distinction the arc removed.
    hint_placed: bool,

    // ── R1 / rtpi: PRIORITY-INHERITANCE state (feature-gated so a knob-off `Task` is byte-identical) ──
    //
    // RECLAMATION-SAFE BY DESIGN (review BLOCKER fix): the DONATION does NOT live on this Task — a
    // Task box is freed by `run()` the instant it exits, so a donor on another CPU that held a raw
    // pointer to it would use-after-free. Instead the donation lives on the LONG-LIVED lock (a
    // blockable `Mutex` is `'static`): each held lock's [`PiCtl::boost`] carries the inherited floor,
    // and this task's EFFECTIVE priority folds in the boost of every lock it holds (see `sched_prio`).
    // A donor therefore only ever touches `PiCtl`s (never a `Task`), and this task only ever reads
    // `PiCtl`s of locks IT holds — both live for the whole access. See the "R1 / rtpi" block.
    //
    /// `PiCtl` addresses of the PI locks this task currently HOLDS (0 = empty slot). `sched_prio`
    /// folds in each held lock's `boost`. Written only by this task on its own CPU (acquire adds,
    /// release removes). Bounded: a task holding more than `PI_HELD_MAX` PI locks at once drops the
    /// overflow from this set, which loses TWO things for the overflowed lock — its `boost` is not
    /// aggregated into this task's own priority, and (because `pi_held_set_waits` iterates only this
    /// set) its `owner_waits` uplink is never published, so TRANSITIVE propagation through it is
    /// severed. Direct donation to the lock itself still lands. A documented cap, never a safety
    /// issue.
    #[cfg(feature = "rtpi")]
    held: [AtomicU64; PI_HELD_MAX],
}

impl Task {
    /// SMPBAL-X86: a COOPERATIVE ring-3 task — one that drops to ring 3 with RFLAGS.IF clear and
    /// therefore runs to completion with the timer masked. `user_entry != 0` is what makes a task
    /// ring-3 (a kernel task leaves it 0 and is `preemptible == false` too, which is why both terms
    /// are needed). Such a task must never run on core 0: `timer_interrupt_handler`'s `cpu_index == 0`
    /// arm is the sole advancer of `APIC_TICKS`, so masking the timer there freezes `arch::ms()` for
    /// the machine.
    #[inline]
    fn is_cooperative_user(&self) -> bool {
        self.user_entry != 0 && !self.preemptible
    }
}

/// R1 / rtpi — a task's EFFECTIVE scheduling priority: the base `priority` raised by the inherited
/// floor of every PI lock it holds. This is the number every placement / preemption decision
/// consumes, so a task that holds a lock a higher-priority task is blocked on is enqueued, dispatched
/// and protected-from-preemption at the inherited level, not its base.
///
/// RECLAMATION-SAFE: the boost is read from the HELD LOCKS, not written onto the task. Each `held`
/// slot is a `PiCtl` address inside a lock this task currently holds; a blockable `Mutex` is
/// `'static`, so the `PiCtl` outlives every access — there is no freed-Task hazard here (this runs on
/// tasks the caller owns: a `Box` under the run-queue lock, or `current`). Owning-task reads its own
/// `held`; a donor never calls this.
///
/// KNOB-OFF BYTE-IDENTITY: this exists ONLY in the `rtpi` build. Each of its four call sites is
/// `#[cfg]`-paired with the original `task.priority` expression the site used before this arc, so a
/// knob-off build's source — and therefore its codegen — is token-for-token the pre-arc scheduler.
#[cfg(feature = "rtpi")]
#[inline]
fn sched_prio(task: &Task) -> u8 {
    let mut p = task.priority;
    for slot in task.held.iter() {
        let ctl = slot.load(Ordering::Relaxed);
        if ctl != 0 {
            // SAFETY: `ctl` addresses a `PiCtl` inside a lock this task holds; a blockable `Mutex` is
            // `'static`, so the `PiCtl` is live for this read.
            let b = unsafe { (*(ctl as *const PiCtl)).boost.load(Ordering::Relaxed) };
            if b > p {
                p = b;
            }
        }
    }
    p
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
    /// SMPBAL-X86: the `memory::as_gen()` value at which this core last UNCONDITIONALLY reloaded CR3,
    /// i.e. the generation of user page-table state its TLB is known to be consistent with. Written
    /// and read only by this core's `run()` at IF=0, so `Relaxed` is sufficient; the ordering that
    /// matters is on `AS_GEN` itself, which the mutating side publishes `AcqRel`.
    ///
    /// The initial 0 is deliberately BELOW the initial `AS_GEN` of 1, so a core's very first dispatch
    /// always takes the reload arm. That costs one `mov cr3` per core per boot and removes the need to
    /// reason about whether the pre-scheduler CR3 was ever validated.
    cr3_gen: AtomicU64,
    /// VUGSPREAD: the CR3 this core's SCHEDULER last installed — a shadow of the live register, kept
    /// so the dispatch site can count the address-space switches a migration causes without reading
    /// CR0/CR3 a second time per dispatch.
    ///
    /// INTROSPECTION ONLY, and that is load-bearing: the real skip decision is still
    /// `switch_cr3_if_needed`'s HARDWARE compare against the live register. This shadow only decides
    /// whether a counter is bumped, so a drift costs a miscount and can never install a wrong root.
    ///
    /// It is nevertheless exact, because every CR3 mutation on this arch is either a sched.rs site
    /// that maintains it (the two dispatch arms, `exit`'s teardown restore, `reap_killed`'s) or the
    /// `syscall.rs` probe pair, which saves and restores around itself and so is net-zero across any
    /// point the scheduler can observe. Written and read only by the owning core at IF=0, hence
    /// `Relaxed`. The initial 0 is not a real CR3, so a core's first dispatch always counts — one
    /// per core per boot, which is the truth.
    cr3_live: AtomicU64,
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
            cr3_gen: AtomicU64::new(0),
            cr3_live: AtomicU64::new(0),
        }
    }
}

static SCHED: [SchedCpu; MAX_CPUS] = [const { SchedCpu::new() }; MAX_CPUS];

/// VUG-1 M3b — per-CPU load counters for the demo's "CPU pulse" meter (BeOS-Pulse style). Additive,
/// lock-free, relaxed: `run()` bumps `CPU_BUSY[cpu]` each time it dispatches a task and `CPU_IDLE[cpu]`
/// each time it idles (`hlt`). The demo samples both once per frame and shows busy/(busy+idle) over
/// the window as a per-core bar. Introspection only — never read on any scheduling path. This is the
/// SEAM a real per-core utilization feed would replace.
///
/// SCHEDLOAD-X86 — the replacement now exists alongside (`ACCT` / [`core_load`]) and these two are
/// DELIBERATELY LEFT INTACT. They are an EVENT-COUNT feed with five live consumers that read them as
/// such (`SYS_CPUPULSE` -> `PULSE.ELF`, `vug.rs`'s meter, `ui_status`'s fallback, `selftest`, the
/// `sched` shell verb); silently redefining their currency to time would break every one of them and
/// would also destroy the arc's own cross-check, which is that the TIME feed and the EVENT feed must
/// agree about WHICH cores are idle while disagreeing about the magnitudes. Two instruments, one
/// question: that is the point, not a duplication to be tidied away.
static CPU_BUSY: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static CPU_IDLE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

// ── SCHEDLOAD-X86 ───────────────────────────────────────────────────────────────────────────────
// Per-core busy-TIME accounting: what fraction of the last ~250 ms each core spent EXECUTING a task,
// as opposed to how many times it was handed one.
//
// WHY A SECOND FEED. `CPU_BUSY`/`CPU_IDLE` count EVENTS — one increment per dispatch, one per `hlt`.
// The ratio they yield is a cadence proxy, not a duty cycle, and on the metal capture it reads (a)
// `c1 = 3/243 -> 1%` for the core that owns every pixel on the machine, because the render task
// blocks in `recv` and is therefore dispatch-SPARSE however expensive each pass is, and (b)
// `c3 = 1897489/0 -> 100%` for a ring-3 spinner, four orders of magnitude away from `c7`'s `348/157`
// while both are simply "the core was busy". Neither number can be compared to the other and neither
// can be compared across cores, so no balancer can be built on them. Exactly ONE reading from that
// feed is unambiguous — `busy == 0` means zero tasks ran — and it is that reading, and only that
// reading, which the new feed must reproduce (see [`emit_load_witness`]).
//
// DENOMINATED IN TSC, NOT IN APIC TICKS. `arch::now_cycles()` is `rdtsc`, invariant on Nehalem+ and
// calibrated against the ACPI PM timer at `t~116ms` (`apic::tsc_hz()`); the APIC TIMER's rate is a
// separate quantity and is NOT calibrated the same way (`scheduler.md:1766`). Spans are therefore
// measured in TSC, and the window budget is `tsc_hz()/4`.
//
// ...EXCEPT FRESHNESS, WHICH IS DENOMINATED IN MILLISECONDS, AND THAT DIVERGENCE FROM THE AARCH64
// TWIN IS THE ONE LOAD-BEARING DESIGN DECISION HERE. On aarch64 `CNTPCT` is a SYSTEM-global counter:
// core A can subtract a timestamp core B wrote and get a real elapsed span. `rdtsc` is a PER-CORE
// counter. Synchronization across packages/cores is a firmware property (the reset-time TSC sync,
// `IA32_TSC_ADJUST`) that this kernel neither programs nor verifies, so a cross-core `now_cycles() -
// their_timestamp` is NOT a sound elapsed time — a modest negative skew wraps to ~2^64 and a modest
// positive skew fabricates a full window of activity. Every quantity a REMOTE reader evaluates
// therefore uses `arch::ms()`, which is globally coherent by construction (`APIC_TICKS` is advanced
// by core 0 alone; see `run_bsp`'s clock note). Every quantity a core evaluates about ITSELF — the
// busy and idle spans — stays in TSC, where `rdtsc` is exact. Nothing in this module ever subtracts
// two TSC readings taken on different cores.
//
// AGE-ON-READ (aarch64's PULSE-5) FOR THE SELF ROW ONLY, AND A DIFFERENT MECHANISM FOR THE REST.
// PULSE-5 exists because the Pi's window can only close at a dispatch boundary, so one compute-bound
// task freezes the percent for an unbounded time; its remedy is to add `now - run_t0` into the window
// at READ time. Across cores that is exactly the unsound TSC subtraction ruled out above — but it is
// perfectly sound when the reader IS the owning core, and that case is not a corner: the witness is
// emitted from the render task, about the render core, at the end of its own pass. `core_load` tests
// `cpu == meter_current_cpu()` and takes the live span only there. See `live_span_cyc`, whose
// soundness argument is much simpler than the aarch64 twin's — a core's scheduler loop runs only when
// no task is running on that core, so there is no concurrency to order and no fence is needed.
//
// For REMOTE cores the freeze is still real — the arc's own QEMU smoke printed `c2=64%` and then
// `c2=--` on the next line, a core that had stopped folding because it was inside a task rather than
// because it had stopped working. That case is handled by a different and cheaper route: `core_load`
// reads `SCHED[cpu].current != 0` (a task is executing) together with `fold_age_ms >= LOAD_WINDOW_MS`
// (the span has already outlasted a whole window) and reports 100%. Both inputs are globally coherent
// — an atomic pointer and the core-0 ms clock — so this reaches PULSE-5's case-1 conclusion with no
// cross-core cycle arithmetic at all. It is an INFERENCE, not a measurement, and the witness marks it
// as such on the wire (`100%*`); see `CoreLoad::pegged` for why that distinction is not optional.
//
// The finer-grained half of PULSE-5 (a partial in-flight span shorter than one window, on a REMOTE
// core) is deliberately not reproduced: it is precisely the part that needs a live cross-core cycle
// delta. Its absence under-reports such a core for at most one window, which is the safe direction —
// an inflated percent would send a future balancer AWAY from a core that is actually free. That is a
// statement about THIS omission and must not be read as a claim that the instrument cannot
// over-report at all: it can, by exactly one mechanism, `busy_pct`'s partial-window blend, bounded by
// one decaying window and documented there. The omission also matters far less here than on the Pi:
// x86 has a live 1 kHz LVT timer on every core and `QUANTUM_TICKS = 4`, so an ordinary preemptible
// task's span is broken, and folded, every ~4 ms.
//
// NO `PaddedUsize`. The aarch64 padding fix is justified there by the A72 having no LSE atomics, so
// an LL/SC reservation broken by a same-cache-line store from a neighbour can livelock; x86 has no
// LL/SC and that argument does not exist here. `CoreAccount` IS cache-line aligned below, on its own
// (much weaker) merits, which are spelled out at the `repr(align)` attribute — do not read that as
// the A72 fix having been ported.

/// The rolling load window. ONE constant, expressed in milliseconds, from which both denominations
/// are derived — the TSC budget a core folds spans against ([`load_window_cyc`]) and the ms-clock
/// bounds a remote reader thresholds against ([`LOAD_STALE_MS`], and the pegged-core test in
/// [`core_load`]). Keeping it single-sourced is what makes "the window has elapsed" mean the same
/// thing to the writer and to the reader despite their using different clocks.
const LOAD_WINDOW_MS: u64 = 250;

/// The rolling load window in TSC cycles. Read from the calibrated `apic::tsc_hz()` on every call
/// rather than cached: unlike the aarch64 twin's `CNTFRQ_EL0` sysreg read this is one relaxed atomic
/// load, so a cache would buy nothing and would risk latching the pre-calibration fallback forever.
///
/// Before calibration (`tsc_hz() == 0`) it falls back to a nominal 2.3 GHz — the Ivy Bridge
/// MacBookPro10,1 base clock. That only stretches or shrinks the SMOOTHING span; it cannot bias the
/// percent, which is a ratio of two spans measured in the same units. Calibration completes at
/// `t~116ms` and the scheduler is not enabled until `t~243ms`, so on a real boot the fallback is
/// never actually the number in force.
#[inline]
fn load_window_cyc() -> u64 {
    let hz = apic::tsc_hz();
    let hz = if hz == 0 { 2_300_000_000 } else { hz };
    hz / 1000 * LOAD_WINDOW_MS
}

/// Sentinel `recent_pct` meaning "no window has completed yet" — fall back to the partial window.
const LOAD_PCT_NONE: u32 = u32::MAX;

/// Sentinel `last_acct_ms` meaning "this core has NEVER folded a span", i.e. it has never been inside
/// `run()`. A dedicated out-of-band value rather than `0`, because `arch::ms()` legitimately reads 0
/// during early boot and "folded at ms 0" must not be confusable with "never folded" — that
/// confusion is precisely what would make a never-scheduled core print a fabricated `0%`.
const ACCT_MS_NEVER: u64 = u64::MAX;

/// One core's busy-time accounting slot.
///
/// SINGLE-WRITER: every field is written ONLY by the owning core's `run()` loop, so `Relaxed` is
/// sufficient for the accounting arithmetic. Read cross-core by introspection ([`core_load`]) only —
/// never consulted on any scheduling path in this arc.
///
/// R1/§10 — the writes are NOT uniformly IF=0, and the distinction is worth stating precisely rather
/// than glossing. The busy-arm fold runs at the loop top with interrupts masked; the IDLE-arm fold
/// runs immediately after `enable_and_hlt` returns, i.e. with IF=1, so its load-add-store sequence is
/// technically interruptible. It is still sound, for a reason stronger than masking: **no interrupt
/// path anywhere touches `ACCT`.** The timer/IPI handlers write `SCHED[cpu]`, `percpu`, and the APIC,
/// and `timer_preempt` returns early on `current == 0` — which is exactly the state of a core running
/// that fold. There is therefore no writer to race with and no update that can be lost.
///
/// CACHE-LINE ALIGNED, on x86's own merits and NOT as a port of the aarch64 `PaddedUsize` fix (whose
/// justification is A72 LL/SC livelock and does not exist on this arch). The merit here is plain
/// write-write false sharing: the slot is ~72 bytes of atomics that the owning core STORES to on
/// every dispatch pass — thousands of times a second, per core — and eight unaligned slots would pack
/// two or three cores' write sets into one 64-byte line, so core 3's fold would steal the line from
/// core 4's in a loop that is on the dispatch path. 128 rather than 64: Sandy/Ivy Bridge L2 fetches
/// adjacent line pairs, making the effective sharing granularity 128 bytes on the bench machine. The
/// cost is 1 KiB of `.bss` for `MAX_CPUS = 8`.
#[repr(align(128))]
struct CoreAccount {
    /// Cumulative context switches INTO a task on this core (one per busy dispatch). This is the
    /// SAME quantity `CPU_BUSY[cpu]` counts; it is carried here as well so the witness line and the
    /// `CoreLoad` contract need not reach across to the other feed, and so that a divergence between
    /// the two is itself observable.
    ctx_switches: AtomicU64,
    /// TSC cycles spent EXECUTING tasks in the CURRENT (incomplete) window.
    win_busy_cyc: AtomicU64,
    /// TSC cycles spent IDLE (`hlt`, empty queue) in the current window. The window rolls over when
    /// `win_busy_cyc + win_idle_cyc` reaches [`load_window_cyc`].
    win_idle_cyc: AtomicU64,
    /// Busy percent (0..=100) of the last COMPLETED window, or [`LOAD_PCT_NONE`] before the first.
    recent_pct: AtomicU32,
    /// `arch::ms()` at the most recent fold, or [`ACCT_MS_NEVER`]. Milliseconds, not cycles — this is
    /// the one quantity a REMOTE core evaluates, and `rdtsc` is per-core (see the module note above).
    /// A core going round the dispatch loop folds a span every pass, so this stays fresh; a core that
    /// left `run()`, or that never entered it, stops touching it and reads STALE.
    last_acct_ms: AtomicU64,
    /// R1/M3 — the TSC instant at which the CURRENTLY-EXECUTING task's span began on this core, or 0
    /// when this core is not inside a task. Published immediately before `switch_context` and cleared
    /// by the fold in [`account`](Self::account), so while it is non-zero the span it anchors is
    /// provably NOT yet in `win_busy_cyc` and adding `now - run_t0` cannot double-count.
    ///
    /// READ BY THE OWNING CORE ONLY. This is the one place the aarch64 twin's age-on-read is adopted,
    /// and it is adopted for exactly the case where the cross-core `rdtsc` objection does not apply.
    /// See [`live_span_cyc`](Self::live_span_cyc) for the soundness argument, which is much simpler
    /// here than on aarch64 — it needs no fences at all.
    run_t0: AtomicU64,
    /// Seqlock sequence for the last-task pair: even = stable, odd = write in progress.
    last_seq: AtomicU64,
    /// Id of the last task dispatched here (0 = none yet).
    last_id: AtomicU64,
    /// `&'static str` name of the last task, split into `.as_ptr()` + `.len()`, both published under
    /// the seqlock. A `&'static str` is a 16-byte fat pointer no single atomic can hold, so the
    /// seqlock is what makes the cross-core read of the pair sound (never a torn ptr-from-A /
    /// len-from-B).
    last_name_ptr: AtomicUsize,
    last_name_len: AtomicUsize,
}

impl CoreAccount {
    const fn new() -> Self {
        CoreAccount {
            ctx_switches: AtomicU64::new(0),
            win_busy_cyc: AtomicU64::new(0),
            win_idle_cyc: AtomicU64::new(0),
            recent_pct: AtomicU32::new(LOAD_PCT_NONE),
            last_acct_ms: AtomicU64::new(ACCT_MS_NEVER),
            run_t0: AtomicU64::new(0),
            last_seq: AtomicU64::new(0),
            last_id: AtomicU64::new(0),
            last_name_ptr: AtomicUsize::new(0),
            last_name_len: AtomicUsize::new(0),
        }
    }

    /// Fold one measured span into the rolling window (owning core only). Exactly one of
    /// `busy_cyc` / `idle_cyc` is non-zero per call: `account(delta, 0)` after a task's execution span
    /// (measured around `switch_context`), `account(0, delta)` after an idle `hlt`. On window
    /// completion — busy+idle reaching the ~250 ms budget — it snapshots the busy TIME fraction and
    /// resets.
    ///
    /// Marking the slot fresh is the FIRST thing it does, so a core that is dispatching is provably
    /// tracked even if the span it just measured was zero-length.
    #[inline]
    fn account(&self, busy_cyc: u64, idle_cyc: u64) {
        self.last_acct_ms.store(crate::arch::ms(), Ordering::Relaxed);
        // R1/M3: ANY fold ends the in-flight span — whatever was executing has now been measured and
        // is about to land in `win_busy_cyc`, so `live_span_cyc` must stop aging it. Cleared here
        // rather than in the busy arm alone so both call sites are covered with no branch.
        self.run_t0.store(0, Ordering::Relaxed);
        let busy = self.win_busy_cyc.load(Ordering::Relaxed) + busy_cyc;
        let idle = self.win_idle_cyc.load(Ordering::Relaxed) + idle_cyc;
        let total = busy + idle;
        if total >= load_window_cyc() {
            // `total >= budget > 0`, so the division is safe and the result is already 0..=100.
            self.recent_pct.store((busy * 100 / total) as u32, Ordering::Relaxed);
            self.win_idle_cyc.store(0, Ordering::Relaxed);
            self.win_busy_cyc.store(0, Ordering::Relaxed);
        } else {
            self.win_idle_cyc.store(idle, Ordering::Relaxed);
            self.win_busy_cyc.store(busy, Ordering::Relaxed);
        }
    }

    /// Publish the last-dispatched task's id + name under the seqlock (owning core only): bump the
    /// sequence ODD, write the pair, bump it EVEN. A reader that sees a stable even sequence on both
    /// sides of its loads therefore has a consistent snapshot.
    #[inline]
    fn note_last(&self, id: u64, name: &'static str) {
        let seq = self.last_seq.load(Ordering::Relaxed);
        self.last_seq.store(seq + 1, Ordering::Release); // odd: write in progress
        self.last_id.store(id, Ordering::Relaxed);
        self.last_name_ptr.store(name.as_ptr() as usize, Ordering::Relaxed);
        self.last_name_len.store(name.len(), Ordering::Relaxed);
        self.last_seq.store(seq + 2, Ordering::Release); // even: stable
    }

    /// R1/M3 — TSC cycles this core has ALREADY spent inside the task it is executing RIGHT NOW, or 0
    /// if it is not inside one.
    ///
    /// **CALLABLE ONLY ON THE OWNING CORE.** `rdtsc` is per-core, so `now_cycles() - run_t0` is an
    /// elapsed time only when both readings come from the same counter. [`core_load`] enforces this
    /// with an explicit `cpu == meter_current_cpu()` test and passes 0 otherwise; a remote caller
    /// simply loses the in-flight term, which is the pre-R1 behaviour.
    ///
    /// SOUNDNESS, and it is much simpler than the aarch64 twin's — it needs no fences and no
    /// double-count argument. `run_t0` is written only by THIS core's scheduler loop, and that loop
    /// runs only when no task is running on this core. So while the caller (a task, or the scheduler
    /// itself) is executing here, the writer provably is not: there is no concurrency to order. And
    /// the span cannot be counted twice, because `account()` clears `run_t0` BEFORE the busy total it
    /// folds becomes visible to anyone — while `run_t0` is non-zero its span is by construction not
    /// yet in `win_busy_cyc`. `emit_load_witness` additionally takes its snapshot with interrupts
    /// masked, so not even a preemption can land between the two reads.
    #[inline]
    fn live_span_cyc(&self) -> u64 {
        let t0 = self.run_t0.load(Ordering::Relaxed);
        if t0 == 0 { 0 } else { crate::arch::now_cycles().wrapping_sub(t0) }
    }

    /// Busy percent (0..=100) for the window AS IT STANDS, with `live` the in-flight execution span
    /// (see [`live_span_cyc`](Self::live_span_cyc)); pass 0 when the caller is not the owning core.
    /// Three cases, in the order tested:
    ///
    ///   1. The in-flight span alone covers a whole window: the last ~250 ms were, in their entirety,
    ///      this core executing one task. That is 100 %, MEASURED, and no other term can change it.
    ///   2. The current window already holds a full budget's worth of measured time (or no window has
    ///      ever completed, so there is no history to consult): report the measured occupancy alone.
    ///   3. The window is still short: report the measured part at FULL weight and fill only the
    ///      REMAINDER from the last completed window's rate. That is what keeps the number continuous
    ///      — a core that just rolled its window does not drop to a noisy two-millisecond sample —
    ///      while bounding how much of the answer can be historical: the stale term's weight is
    ///      exactly the fraction of the window not yet measured, and it decays to zero as the window
    ///      fills.
    ///
    /// R1/M1 — CASE 3 IS THE ONE WAY THIS INSTRUMENT CAN OVER-REPORT, and the bound is worth stating
    /// because a previous revision of these comments claimed it could never happen. A core that was
    /// 100 % busy in window N-1 and goes fully idle in window N reads its OLD percent at the start of
    /// N and decays linearly to 0 across ~250 ms, rather than dropping instantly. It is bounded by one
    /// window, it is always decaying, and it cannot fire for a core that has been idle for a whole
    /// window — but a core that finished work < 250 ms before a witness line WILL print a non-zero
    /// percent. Any refutation criterion phrased as "a percent on a core PULSE-A shows at `busy=0`"
    /// must tolerate that, or it is a false-refutation trigger.
    ///
    /// The alternative — dropping the blend and reporting `busy/elapsed` alone — is strictly worse:
    /// immediately after a roll `elapsed` is one span, so a single 2 ms busy sample would print 100 %
    /// outright. The blend trades an unbounded sampling error for a bounded, decaying one.
    ///
    /// Lock-free, allocation-free, no CROSS-core TSC subtraction.
    fn busy_pct(&self, live: u64) -> u32 {
        let budget = load_window_cyc();
        if live >= budget {
            return 100; // case 1 — the whole window is one uninterrupted execution span
        }
        let busy = self.win_busy_cyc.load(Ordering::Relaxed) + live;
        let idle = self.win_idle_cyc.load(Ordering::Relaxed);
        let elapsed = busy + idle;
        let recent = self.recent_pct.load(Ordering::Relaxed);
        if elapsed >= budget || recent == LOAD_PCT_NONE {
            if elapsed == 0 {
                0
            } else {
                ((busy * 100 / elapsed) as u32).min(100)
            }
        } else {
            // `busy` < 2*budget and `recent` <= 100, so both products stay far inside u64.
            let rem = budget - elapsed;
            (((busy * 100 + recent as u64 * rem) / budget) as u32).min(100)
        }
    }

    /// Is this core's load being accounted RIGHT NOW — i.e. is `busy_pct` a LIVE number?
    ///
    /// True when the owning core folded a span within [`LOAD_STALE_MS`]. False when the slot has never
    /// been touched (a core that has not entered `run()` — the BSP before its handoff, an AP not yet
    /// released) or has gone stale (a core that LEFT `run()`, or one masking its own timer under a
    /// cooperative ring-3 task). An untracked core is reported `--` by every honest view rather than
    /// as its frozen last percent, and never as `0%` — a fabricated zero on a core that is actually
    /// pegged is the exact failure this arc exists to make impossible.
    ///
    /// This is the fold-age half of the question only. [`core_load`] ORs in the pegged-core arm, which
    /// re-admits a core that stopped folding because it is inside one long execution span; a caller
    /// wanting the raw staleness reads `fold_age_ms`.
    fn tracked(&self) -> bool {
        self.fold_age_ms() < LOAD_STALE_MS
    }

    /// Milliseconds since this core last folded a span, or [`ACCT_MS_NEVER`] if it never has. The raw
    /// quantity [`tracked`](Self::tracked) thresholds.
    ///
    /// `saturating_sub` rather than `wrapping_sub`: `arch::ms()` is monotone, so a "negative" age can
    /// only come from a torn or racing read, and clamping it to 0 (freshest) is the reading that
    /// cannot invent staleness out of a race.
    fn fold_age_ms(&self) -> u64 {
        let last = self.last_acct_ms.load(Ordering::Relaxed);
        if last == ACCT_MS_NEVER {
            return ACCT_MS_NEVER;
        }
        crate::arch::ms().saturating_sub(last)
    }

    /// Read the last-task pair with a bounded seqlock retry. The `&'static str` reconstruction is
    /// sound: the writer only ever publishes a live `'static` name's `(ptr, len)`, and the seqlock
    /// guarantees the reader sees a MATCHING pair.
    ///
    /// R1/L1 — the `fence(Acquire)` before the second sequence read is the canonical seqlock shape and
    /// is not decoration. An Acquire *load* orders what follows it; what this reader needs is for the
    /// data loads ABOVE to be ordered before the validating load BELOW, which is a fence's job. On
    /// x86-TSO loads are never reordered with loads, so the code was already correct on this machine —
    /// the fence buys conformance to the memory model rather than to the microarchitecture, and costs
    /// nothing (it compiles to no instruction on x86-64).
    fn last_task(&self) -> (u64, &'static str) {
        for _ in 0..8 {
            let s1 = self.last_seq.load(Ordering::Acquire);
            if s1 & 1 != 0 {
                continue; // write in progress; retry
            }
            let id = self.last_id.load(Ordering::Relaxed);
            let ptr = self.last_name_ptr.load(Ordering::Relaxed);
            let len = self.last_name_len.load(Ordering::Relaxed);
            core::sync::atomic::fence(Ordering::Acquire);
            if self.last_seq.load(Ordering::Relaxed) != s1 {
                continue; // changed under us; retry
            }
            if ptr == 0 || len == 0 {
                return (id, "-");
            }
            // SAFETY: `ptr`/`len` were published together (seqlock-consistent) from a live
            // `&'static str`, so this reconstructs that exact, still-live string slice.
            let name = unsafe {
                core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr as *const u8, len))
            };
            return (id, name);
        }
        (self.last_id.load(Ordering::Relaxed), "?")
    }
}

/// How long a slot may go unfolded before its percent stops being reported as live. Two load windows
/// of slack, so a slow rollover never mislabels a genuinely-scheduled core as stale, while a core that
/// has actually stopped dispatching is disqualified within half a second.
const LOAD_STALE_MS: u64 = 2 * LOAD_WINDOW_MS;

static ACCT: [CoreAccount; MAX_CPUS] = [const { CoreAccount::new() }; MAX_CPUS];

/// A snapshot of one core's live scheduler load. Returned by [`core_load`]; consumed by the
/// `[schedx86] load` witness. Every field is a point-in-time read — introspection only, never
/// consulted on a scheduling path in this arc.
///
/// Mirrors the aarch64 `CoreLoad` contract field-for-field with three deliberate divergences:
///   * there is no `busy_pct_recent` EXCLUDING a service band, because x86 has no `PRIO_SERVICE`
///     (`spawn_prio` and the band are Arc 1's business);
///   * the freshness field is `fold_age_ms`, not `fold_age_cyc`. Same quantity, different
///     denomination, and the rename is deliberate rather than cosmetic: on this arch that age MUST be
///     measured in the globally-coherent ms clock, because `rdtsc` is per-core (see the module note
///     at `CoreAccount`). A field named `..._cyc` holding milliseconds would be the kind of quiet lie
///     this instrument exists to prevent;
///   * there is an extra field, `pegged`, which aarch64 has no need of. See it.
pub struct CoreLoad {
    /// Busy TIME fraction (0..=100). Only meaningful when `tracked` is true, and its PROVENANCE is
    /// `pegged` — read the two together or not at all.
    pub busy_pct_recent: u32,
    /// Cumulative context switches into a task on this core since boot.
    pub ctx_switches: u64,
    /// Id of the last task dispatched on this core (0 = none yet).
    pub last_task_id: u64,
    /// Name of the last task dispatched on this core ("-" = none yet). While `pegged` is true this is
    /// ALMOST CERTAINLY also the task the core is executing right now — but "almost" is the honest
    /// word and R2/L1 is why. [`core_load`] loads `current` before this name precisely so the name is
    /// as-new-or-newer than the `current` that set `pegged`, which closes the stale-name direction;
    /// the remaining window is a remote dispatch landing between the two loads, which the emit-side
    /// interrupt mask cannot help with because the racing writer is another core. Best-effort and
    /// stale-by-one-able, not proven. See the note at the `current_raw` load.
    pub last_task: &'static str,
    /// Is `busy_pct_recent` a LIVE number? False for a core that has never entered `run()` or has
    /// stopped folding spans WITHOUT a task executing; such a core renders `--`, never a percent.
    pub tracked: bool,
    /// R1/H2 — WAS `busy_pct_recent` MEASURED, OR INFERRED? True means inferred: no span was folded
    /// for this window, and the 100 % was DEDUCED from a live `current` plus a fold age past a full
    /// window (see [`core_load`]). False means every cycle in the number was measured and folded.
    ///
    /// This field exists because the arc's own thesis demands it. The module argues at length that
    /// `--` must not collapse into `0%` — that the absence of a measurement must not print as one.
    /// An INFERENCE printed as a measurement is the same category error one level up, and worse,
    /// because the inferred value is the extreme of the scale: an inferred `100%` is byte-identical to
    /// a core that folded a full window of real busy spans. Every renderer of this struct MUST make
    /// the two distinguishable; `emit_load_witness` does it with a trailing `*`.
    ///
    /// It also decides whether the arc's headline cross-check can be scored at all. If the core
    /// PULSE-A reports at 100 % reads 100 % here via the pegged arm, the two feeds are not agreeing
    /// for independent reasons — `current != 0` and "this core is dispatching" are near-identical
    /// facts, so the agreement would be structural rather than evidential.
    pub pegged: bool,
    /// Milliseconds since this core last folded a load span; [`ACCT_MS_NEVER`] if it never has.
    /// `tracked` answers "is this worth PRINTING" with ~500 ms of slack; this is the raw measurement,
    /// so a future caller asking the much tighter question "may I hand this core work only it can
    /// run" can pick its own bound.
    pub fold_age_ms: u64,
}

/// Read a core's live load: busy-TIME percent over the rolling ~250 ms window, whether that percent
/// was measured or inferred, cumulative context switches, the last task dispatched, and the freshness
/// of all of it. Allocation-free and lock-free; callable from ANY core. Introspection only.
///
/// An out-of-range core reads as never-tracked rather than as zero load — the same distinction the
/// `tracked` flag draws for a real core.
pub fn core_load(cpu: usize) -> CoreLoad {
    if cpu >= MAX_CPUS {
        return CoreLoad {
            busy_pct_recent: 0,
            ctx_switches: 0,
            last_task_id: 0,
            last_task: "-",
            tracked: false,
            pegged: false,
            fold_age_ms: ACCT_MS_NEVER,
        };
    }
    let acct = &ACCT[cpu];
    // R2/L1 — `current` is loaded BEFORE the task name, not after, and the reason is an ordering the
    // previous revision claimed but did not establish. The writer's order per dispatch is
    // `note_last(T)` then `current = T`; a reader that takes the NAME first and `current` second can
    // therefore observe a name from the previous dispatch beside a `current` from the new one, and the
    // writer's ordering buys it nothing. Reading `current` first inverts that: the `note_last(T)` that
    // precedes the `current = T` we observe has already happened, so the name we read afterwards is T
    // or NEWER.
    //
    // "Or newer" is the honest residue, and it is not closed here. If the core dispatches again
    // between the two loads the name advances past the `current` we tested — but that requires the
    // core to have folded a span, which makes `fold_age_ms` fresh and `pegged` false, so the row is
    // not printed with a name at all. What remains is a genuine tens-of-nanoseconds race against a
    // <=4/s dispatch rate on a core that is pegged by definition (order 1e-7 per emit). The mask in
    // `emit_load_witness` does NOT help, because the racing writer is a remote core. So: the pegged
    // attribution is BEST-EFFORT and stale-by-one-able, not proven — treat a `100%*(name)` as strong
    // evidence about the core and good-but-unwarranted evidence about the name.
    let current_raw = SCHED[cpu].current.load(Ordering::Acquire);
    let (last_task_id, last_task) = acct.last_task();
    let fold_age_ms = acct.fold_age_ms();
    // R1/M3 — SELF AGE-ON-READ. The module declines aarch64's PULSE-5 wholesale because
    // `now_cycles() - their_t0` is unsound across cores. That argument is correct and it does NOT
    // apply when the reader IS the owning core, where the subtraction is the same same-core rdtsc
    // pair the fold already performs twice per dispatch. Declining it there was a real loss of
    // resolution on the one core the survey most wants a number for: the witness is emitted from the
    // render task itself, at the END of its pass, and that task's span is folded only when it later
    // blocks in `recv` — so before this arm c1 reported its own load missing most of the current pass,
    // every single time, biased low by roughly 2x at exactly the sample point.
    let live = if cpu == meter_current_cpu() { acct.live_span_cyc() } else { 0 };
    // A CORE PEGGED BY ONE LONG SPAN MUST READ PEGGED, NOT `--`. This was found by the arc's own QEMU
    // smoke, which printed `c2=64%` and then `c2=--` on the next 5 s line: a core that had stopped
    // folding because it was INSIDE a task, not because it had stopped working. `--` is the right
    // answer for "no measurement"; it is the wrong answer for "one measurement that has not finished",
    // and on a balancer's input those are opposite readings.
    //
    // The test is sound on this arch, which is why it is worth having:
    //   * `SCHED[cpu].current != 0` says a task is executing here. It is published Release before the
    //     switch and cleared AFTER the fold that closes the span, so a reader can never see a live
    //     `current` paired with an already-banked span.
    //   * `fold_age_ms >= LOAD_WINDOW_MS` says the span it is executing has already outlasted a whole
    //     window. The two together mean the last window was, in its entirety, this core running one
    //     task — which is 100%, and no other term can change it.
    // Both quantities are globally coherent (an atomic pointer and the core-0 ms clock), so this
    // reaches aarch64 PULSE-5's case-1 conclusion WITHOUT the cross-core `rdtsc` subtraction that
    // mechanism would otherwise require and that this arch cannot honestly perform.
    //
    // `live == 0` gates it: when we DO have the in-flight span (the self row) the number is measured
    // and needs no inference, and `busy_pct`'s case 1 reaches the identical 100% by measurement. So
    // the pegged arm is, by construction, a REMOTE-core fallback — which is also why marking it on the
    // wire matters (R1/H2): everything it produces is deduced, not counted.
    //
    // Below `LOAD_WINDOW_MS` a remote core's in-flight span is simply not counted yet, so the
    // instrument under-reports it for at most one window. That direction is the safe one — an inflated
    // percent would send a future balancer AWAY from a core that is actually free. It is NOT, however,
    // a claim that the instrument can never over-report: `busy_pct`'s stale blend can, by up to one
    // decaying window, and that bound is documented there (R1/M1).
    let pegged = live == 0
        && fold_age_ms != ACCT_MS_NEVER
        && fold_age_ms >= LOAD_WINDOW_MS
        && current_raw != 0;
    // R2/H1 — THERE ARE THREE INDEPENDENT REASONS A ROW IS LIVE, AND ALL THREE MUST BE NAMED HERE.
    // This predicate was `pegged || acct.tracked()`, which was correct only until `pegged` was gated
    // on `live == 0` to make it a remote-only fallback. That gate silently removed the self row's
    // rescue: the witness is emitted from INSIDE the render task, so for c1 `run_t0` is always set,
    // `live > 0`, and `pegged` is therefore always FALSE — collapsing `tracked` to
    // `fold_age_ms < LOAD_STALE_MS`, which for the self row IS THE CURRENT PASS'S DURATION. Any render
    // pass reaching the emit >= 500 ms after its dispatch would have printed `c1=--`, the absence
    // token, for the core holding the longest, cleanest, same-core-measured span on the machine —
    // while `busy_pct`'s case 1 was sitting right there ready to return a MEASURED 100%. The emitter
    // tests `!tracked` first, so the absence token would have won over the measurement. Boot AH's
    // first render pass is 237 ms (dispatch 2553 ms -> first depth line 2790 ms) against that 500 ms
    // threshold: a 2.1x margin on a pass that builds a ~28 MiB back buffer, which a heavier first
    // paint (UNAOS_WC compositor path, 2880x1800) crosses. It would also have self-tripped the arc's
    // own refutation criterion, which declares any steady-line `--` a defect.
    //
    // The three reasons, none of which implies another:
    //   1. `live > 0`  — we HOLD a measured in-flight span for this core (self row only). A core
    //      executing a task is being accounted by definition; this is the most direct evidence of
    //      liveness there is, and it is exactly what the previous predicate omitted.
    //   2. `pegged`    — we can INFER the core is inside a long span (remote row).
    //   3. `acct.tracked()` — it folded a span recently (the fold-age test, which governs every core
    //      that is neither executing nor inferable: idle cores, and cores that left `run()`).
    // Anything added later that produces a percent must extend this list too, or it will be computed
    // and then thrown away behind a `--`.
    let tracked = live > 0 || pegged || acct.tracked();
    CoreLoad {
        busy_pct_recent: if pegged { 100 } else { acct.busy_pct(live) },
        ctx_switches: acct.ctx_switches.load(Ordering::Relaxed),
        last_task_id,
        last_task,
        tracked,
        pegged,
        fold_age_ms,
    }
}

/// A bounded, allocation-free line buffer for [`emit_load_witness`], with an explicit overflow flag.
/// Truncation is recorded rather than silent: a witness that quietly loses its last two cores would
/// read as a shorter machine, which is exactly the class of lie this arc is built to exclude.
struct LineBuf {
    buf: [u8; LINEBUF_CAP],
    len: usize,
    overflow: bool,
}

/// Capacity of [`LineBuf`], with the bound proved rather than guessed — and R2/L2 corrected the proof
/// after the first version got the per-core term and the cap suffix wrong. It held, but a comment that
/// says "proved" and then miscounts is the exact class this arc exists to police, so the arithmetic is
/// spelled out term by term. Worst case for `MAX_CPUS = 8`:
///
/// | term | bytes |
/// | --- | --- |
/// | `"[schedx86] load"` | 15 |
/// | tag (`"-prejoin"`, budgeted) | 16 |
/// | 8 x `" cN=100%*(<16-byte name>+)"` — space, `cN=`, `100`, `%`, `*`, `(`, name, clip `+`, `)` = 28 | 224 |
/// | `" sw=["` + 8 x 20 digits of `u64::MAX` + 7 commas + `"]"` | 173 |
/// | `" q=["` + 8 x 10 digits + 7 commas + `"]"` | 92 |
/// | `" steal="` + 2 x 20 digits of `u64::MAX` + `"/"` (SMPBAL-X86) | 48 |
/// | `" asgen="` + 2 x 20 digits of `u64::MAX` + `"/"` (SMPBAL-X86) | 48 |
/// | `" cores=8/NNN <CAPPED>"` | 21 |
/// | **total** | **637** |
///
/// 768 carried ~131 bytes of headroom against that 637.
///
/// VUGSPREAD added a SECOND user, [`emit_spread_witness`], and the review round that followed added
/// five fields to it, so the buffer is now sized on THAT line rather than the load line. Worst case
/// for `MAX_CPUS = 8`, term by term:
///
/// | term | bytes |
/// | --- | --- |
/// | `"[spread] pack=8 spare=8 rqp=["` | 29 |
/// | 8 x `",1/<20 digits>/<20 digits>"` — comma, `1`, `/`, ready, `/`, pinned = 44 | 352 |
/// | `"] steal=<20>/<20> m1=<20> mh=<20> remig=<20> cool=<20> packseen=<20> cr3sw=<20>"` | 206 |
/// | `" decl=t:<20> e:<20> f:<20> p:<20> d:<20> i:<20>"` | 144 |
/// | `" cores=8/NNN <CAPPED>"` | 21 |
/// | **total** | **752** |
///
/// 768 would have left 16 bytes — thinner still (VUGSPREAD-COOL added `cool=<20>`), which is exactly
/// why the cap sits at **1024**, leaving ~272 bytes of headroom.
/// The cost is 256 more bytes of a 16 KiB kernel stack, on a witness path that allocates nothing and
/// is never nested. [`LineBuf`] reports overflow on the wire regardless, so the bound is belt and
/// braces either way — but a bound that is quietly one field from binding is not a bound.
const LINEBUF_CAP: usize = 1024;

/// Longest task name the witness will print for a pegged core, in bytes. Bounds the line (see
/// [`LINEBUF_CAP`]); truncation is at a UTF-8 character boundary and is marked with a trailing `+`
/// (R2/L3 — an earlier revision of this line said `…`, which the code never emitted).
const PEG_NAME_CAP: usize = 16;

impl LineBuf {
    const fn new() -> Self {
        LineBuf { buf: [0; LINEBUF_CAP], len: 0, overflow: false }
    }
    /// The accumulated bytes as a `&str`. Everything written is ASCII produced by this module's own
    /// `write!`s, plus (for pegged cores only) a task name — which is a `&'static str` literal from a
    /// spawn site, so the slice is valid UTF-8; the `from_utf8` is checked rather than assumed anyway,
    /// because an instrument that can emit garbage bytes on a bad read is worse than one that says so.
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("<witness: non-utf8>")
    }
}

/// One core's row in the witness snapshot. Taken ONCE per core under a single interrupt mask (see
/// [`emit_load_witness`]) so that a row's percent, its dispatch count and its queue depth all describe
/// the same instant — a reader diffing `sw` against a percent on the same line is exactly what the
/// cross-check criterion asks for, and two reads at two instants would quietly break it.
#[derive(Clone, Copy)]
struct LoadRow {
    pct: u32,
    tracked: bool,
    pegged: bool,
    name: &'static str,
    sw: u64,
    q: usize,
}

impl LoadRow {
    const fn blank() -> Self {
        LoadRow { pct: 0, tracked: false, pegged: false, name: "-", sw: 0, q: 0 }
    }
}

impl core::fmt::Write for LineBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        if self.len + b.len() > LINEBUF_CAP {
            self.overflow = true;
            return Err(core::fmt::Error);
        }
        self.buf[self.len..self.len + b.len()].copy_from_slice(b);
        self.len += b.len();
        Ok(())
    }
}

/// SCHEDLOAD-X86 — the always-on per-core load witness. Emits ONE serial line:
///
/// ```text
/// [schedx86] load c0=0% c1=3% c2=-- c3=100%*(pulse) … sw=[..] q=[..] steal=M/P asgen=G/R
/// ```
///
/// THE PER-CORE TOKEN HAS EXACTLY THREE FORMS, and the distinction between them is the instrument:
///
/// | form | meaning |
/// | --- | --- |
/// | `cN=NN%` | **MEASURED.** Every cycle in the number was folded from a real span over the window. |
/// | `cN=100%*(name)` | **INFERRED.** No span was folded for this window; the value is deduced from a live `current` plus a fold age past a whole window. `name` is the task holding the core. |
/// | `cN=--` | **ABSENT.** No live measurement at all — the core never entered `run()`, or stopped folding with nothing executing. |
///
/// `sw` is cumulative context switches per core; `q` is the instantaneous ready-queue depth;
/// `steal=M/P` is SMPBAL-X86's cumulative migrations over the idle passes that attempted one (see
/// `STEAL_PASSES` for why both terms are printed); `asgen=G/R` is the live address-space generation
/// over the dispatches that had to re-validate CR3 against it (see `CR3_RELOADS`). `tag` is appended
/// to the `load` token (`""` for the steady heartbeat, `"-prejoin"` for the boot one-shot).
///
/// WHY THREE FORMS AND NOT TWO. `0%` is a MEASUREMENT — this core folded spans and none were busy.
/// `--` is the ABSENCE of one. Collapsing them would make an unaccounted core indistinguishable from a
/// provably idle one, which is how an instrument ends up certifying the very imbalance it was built to
/// detect. `100%*` is the same argument one level up (R1/H2): an inferred value printed identically to
/// a measured one is the same category error, and worse here because the inferred value is the extreme
/// of the scale. Without the marker the arc's headline PULSE-A cross-check cannot be scored — a core
/// agreeing at 100 % via the pegged arm agrees for a reason that is near-identical to PULSE-A's own
/// ("this core is dispatching"), i.e. structurally rather than evidentially.
///
/// WHY THE SNAPSHOT IS TAKEN WITH INTERRUPTS MASKED (R1/H1) — this is a correctness requirement, not
/// tidiness. `run_queue_len` acquires `RUN_QUEUES[c]`, a plain `spin::Mutex` with NO IRQ masking, and
/// its own doc names it a WEDGE-4 `<W1>` hazard site. This witness runs from `x86_render_service` — a
/// PREEMPTIBLE task on the core that owns the panel — so unmasked it would be a permanent, silent
/// self-deadlock waiting to happen: preempt the render task while it holds `RUN_QUEUES[1]`, and `run()`
/// on the same core then needs that identical lock at IF=0 to requeue the very task that holds it.
/// Nothing breaks the cycle, nothing panics, and because this witness is emitted FROM the dead task,
/// no instrument on the machine would report it. Masking removes the preemption that forms the cycle.
///
/// THE MASKED SECTION IS BOUNDED AND TAKES NOTHING ELSE. It is `n <= MAX_CPUS = 8` iterations of:
/// a handful of relaxed atomic loads, one bounded (8-retry) seqlock read, and ONE `RUN_QUEUES[c]`
/// acquisition held only across `len()` (a sum over `NUM_PRIORITIES = 4` `VecDeque::len`s). The locks
/// are taken and released one at a time — never nested, so no lock-order inversion is possible — and
/// no allocation, no UART and no other lock is touched inside. Order of tens of nanoseconds per core,
/// sub-microsecond total.
///
/// R2/M2 — WHY THE PRINT IS OUTSIDE THE MASK, corrected. A previous revision of this comment argued
/// that keeping `serial_println!` outside `without_interrupts` avoids "a guaranteed 8 ms of masked
/// interrupts on the render core every 5 seconds". **That was a false premise**: `serial::_print`
/// wraps its ENTIRE write in `interrupts::without_interrupts` itself (`serial.rs:105`), so the masked
/// wire time happens either way and nesting would change nothing measurable.
///
/// The real reasons, both of which survive checking:
///   * The snapshot is masked because `run_queue_len` NEEDS it (above). The print does not need it,
///     and extending a mask past its justification is how a mask stops being reviewable.
///   * The unmasked print cannot re-open the H1 shape, for a structural reason rather than a
///     probabilistic one: `_print` takes `SERIAL1` with **`try_lock()`, never `lock()`**, defers to
///     `serial_ring` when contended, and carries a lock-free `raw_byte` panic hatch. No core can
///     block on the UART lock, so no holder can be preempted into a cycle. The serial path is
///     immune to the wedge by construction, which is exactly why it is safe to leave alone.
///
/// Call from a rate-limited path only; it is introspection, never a scheduling decision.
pub fn emit_load_witness(tag: &str) {
    use core::fmt::Write;
    let seen = crate::arch::acpi::cpu_count().max(1);
    let n = meter_cpu_count();

    // ── snapshot (IF=0) ─────────────────────────────────────────────────────────────────────────
    let mut rows = [LoadRow::blank(); MAX_CPUS];
    x86_64::instructions::interrupts::without_interrupts(|| {
        for (c, row) in rows.iter_mut().enumerate().take(n) {
            let ld = core_load(c);
            *row = LoadRow {
                pct: ld.busy_pct_recent,
                tracked: ld.tracked,
                pegged: ld.pegged,
                name: ld.last_task,
                sw: ld.ctx_switches,
                q: run_queue_len(c),
            };
        }
    });

    // ── format + emit (IF restored) ─────────────────────────────────────────────────────────────
    let mut w = LineBuf::new();
    let _ = write!(w, "[schedx86] load{}", tag);
    for (c, row) in rows.iter().enumerate().take(n) {
        if !row.tracked {
            let _ = write!(w, " c{}=--", c);
        } else if row.pegged {
            // R1/H2: the `*` says DEDUCED, not counted. The name is worth attributing here and only
            // here — a pegged core is by definition executing something — but it is BEST-EFFORT, not
            // proven: see R2/L1 at `core_load`'s `current_raw` load for the remaining stale-by-one
            // window. Read the `*` as evidence about the core and the name as a strong hint.
            let (nm, clip) = peg_name(row.name);
            let _ = write!(w, " c{}={}%*({}{})", c, row.pct, nm, clip);
        } else {
            let _ = write!(w, " c{}={}%", c, row.pct);
        }
    }
    let _ = write!(w, " sw=[");
    for (c, row) in rows.iter().enumerate().take(n) {
        let _ = write!(w, "{}{}", if c == 0 { "" } else { "," }, row.sw);
    }
    let _ = write!(w, "] q=[");
    for (c, row) in rows.iter().enumerate().take(n) {
        let _ = write!(w, "{}{}", if c == 0 { "" } else { "," }, row.q);
    }
    // SMPBAL-X86: cumulative migrations / idle passes that attempted one. Both numbers, never just
    // the first: a steal count alone cannot distinguish balancing from churn, and the ratio is the
    // arc's own falsifier (see `STEAL_PASSES`). It rides this line rather than a line of its own so
    // the moves can be read against the per-core percents they are supposed to have flattened.
    let (moves, passes) = steal_counters();
    let _ = write!(w, "] steal={}/{}", moves, passes);
    // SMPBAL-X86: the CR3-generation fix's only falsifiable reading — the live address-space
    // generation and the number of dispatches that had to re-validate against it. See `CR3_RELOADS`.
    let _ = write!(
        w,
        " asgen={}/{}",
        crate::arch::memory::as_gen(),
        CR3_RELOADS.load(Ordering::Relaxed)
    );
    // R1/L4: the column count is capped at `MAX_CPUS`. Say so when it BINDS, for the same reason
    // `LineBuf` reports truncation — a witness that quietly drops its last two cores reads as a
    // shorter machine, and the bench rMBP is exactly 8 logical cores, i.e. zero headroom.
    if seen > n {
        let _ = write!(w, " cores={}/{} <CAPPED>", n, seen);
    }
    if w.overflow {
        // Say so on the wire rather than shipping a line that merely LOOKS complete.
        serial_println!("{} <TRUNCATED>", w.as_str());
    } else {
        serial_println!("{}", w.as_str());
    }
    // VUGSPREAD: the placement half of the same question, on the same clock. `seen` rides along so
    // the column cap is reported on BOTH lines — review F13: a capped `[spread]` would under-count
    // `pack` and over-state `spare` for the invisible cores, which is the one direction that makes
    // this witness certify headroom the machine does not have.
    emit_spread_witness(n, seen);
}

/// VUGSPREAD — the PLACEMENT witness. One serial line, emitted from [`emit_load_witness`] so it
/// rides that instrument's existing rate limit and introduces no clock of its own:
///
/// ```text
/// [spread] pack=0 spare=3 rqp=[0/0/0,1/0/0,--,1/1/1,…] steal=4/812331 m1=3 mh=4 remig=0 packseen=12 cr3sw=91204 decl=t:0 e:812327 f:0 p:0 d:0 i:0
/// ```
///
/// `rqp` is one token per core, `running/ready/pinned`:
///   * `running` — 1 if a task is dispatched here right now (`SCHED[c].current != 0`), else 0. The
///     pointer is TESTED, never dereferenced: the Box is owned by that core's `run()` and a remote
///     read of its contents would be a use-after-free waiting for a teardown to line up.
///   * `ready` — tasks in the ready queue. Excludes the running one, which is the arithmetic the
///     old steal floor got wrong (see `steal_floor`).
///   * `pinned` — how many of those `ready` tasks `steal_one` may never take.
///   * `--` — this core never entered `run()`. Not an idle core, and not counted in `spare`.
///
/// `pack` counts DISPATCHING cores with `running + ready >= 2` — cores carrying more runnable work
/// than they can execute. `spare` counts dispatching cores with nothing running and nothing ready.
/// **`pack >= 1` together with `spare >= 1` is the defect this arc exists to remove**, and it is a
/// machine-wide statement rather than a per-core one because that is the shape a reader can score
/// against `[wpace]` on the same wire.
///
/// `m1` / `mh` attribute the moves (see `STEAL_M_DEPTH1` / `STEAL_M_HINT`); `packseen` is the
/// high-rate companion to `pack` (see `PACK_SEEN`); `cr3sw` is what the moves COST (see
/// `CR3_SWITCHES`); `decl` names every declined attempt (see `STEAL_D_*`).
///
/// **THE CONSERVATION LAW, and its tolerance (review F11).** `e + f + p + d + i + moves == passes`,
/// with `t` outside `passes` because `STEAL_PASSES` is bumped after the thief exclusion. The terms
/// are read at slightly different instants from a machine that keeps incrementing them, so a
/// residual of a FEW is sampling skew and means nothing. A residual in the THOUSANDS, or one that
/// grows with the capture, means a return path was added without a counter — at which point the
/// `decl` breakdown is incomplete and every attribution below it is suspect. Score the magnitude,
/// not the equality.
///
/// WHAT THE NEXT BOOT CAN ACTUALLY PRODUCE — review F16 rewrote this table, because the first
/// version listed PRE-fix diagnoses that a POST-fix boot cannot reach, which is the same defect as a
/// witness that cannot fail. All three repairs are in force on the next capture; the rows below are
/// what that machine can print.
///
/// | the next boot shows | reading |
/// | --- | --- |
/// | `pack -> 0`, `moves` a handful then flat, `remig 0`, `win=1` off 19.1/s | **PASS.** Spread, and settled rather than oscillating. |
/// | `pack=0` and `packseen` near 0 at every sample, `win=1` still at 19.1/s | **REFUTED.** No packing existed at the 5 s census OR at the millions of steal-pass observations between them, so the frame time was never a placement problem. Go to the yield-spin barrier: the TIME feed already reads c0/c5 at 2–3 % where the EVENT feed reads 99 %. |
/// | `pack=0` but `packseen/passes` materially non-zero, rate unchanged | the packing is real and TRANSIENT — sub-census, forming and clearing inside a frame. Neither the floor nor the pin can hold a queue that is empty whenever it is looked at; this is a barrier/wake-latency story, not a placement one. |
/// | `pack>=1`, `spare>=1`, packed core's `pinned` > 0 | **THE FIX FAILED**, and this is the row that says so — not, as an earlier draft had it, a vindication of the pin repair. Post-fix a ring-3 thread is steal-eligible, so a pinned task still sitting on a packed core is either a kernel task that legitimately named that core (check the `[schedx86] load` name) or a ring-3 path that does not go through `spawn_user_thread`. Find which before touching anything else. |
/// | `pack>=1`, `spare>=1`, `pinned=0`, `decl i:` climbing | the idle-floor GUARD is what is holding the packing: every ready-holding core is going idle between the peek and the lock and re-raising its floor to 2. That is the ping-pong brake working as specified and declining a move that would in fact have helped. It is a tuning question, not a defect, and the change would be to let a depth-1 steal through when the thief has been idle for more than one pass. |
/// | `decl f:` climbing post-fix | narrower than it was: with the per-victim floor in force, `f` can only fire when EVERY ready-holding core is at `PRIO_IDLE` with depth 1. Same guard as the row above, observed one step earlier. It does NOT mean "the old floor hid it" — that diagnosis is unreachable now. |
/// | `moves` climbing with `remig` beside it, or `cr3sw` delta outrunning the `steal=` delta | churn. See `scheduler.md` for the numeric revert criterion; "climbing" against a baseline of one move in four and a half million passes is not a threshold. |
///
/// **THE TWO REPAIRS ARE NOT FULLY SEPARABLE, and `m1`/`mh` say how far separation goes.** A vug
/// worker packed behind its parent needed BOTH the pin release and the floor change, and it scores
/// on both counters — that is the honest answer, not a shortcoming. What the pair does settle is the
/// one-sided cases: `mh > 0` with `m1 == 0` means the pin was the whole story, `m1 > 0` with
/// `mh == 0` means the floor was. Anything else is a joint result and must be reported as one.
///
/// SNAPSHOT DISCIPLINE is `emit_load_witness`'s, verbatim and for its reasons: the scan takes
/// `RUN_QUEUES[c]` (a plain `spin::Mutex` with no IRQ masking) from a preemptible task on the render
/// core, so it runs inside `without_interrupts` or it is a latent self-deadlock. Bounded: `n <=
/// MAX_CPUS` iterations of one relaxed load plus one lock held across a walk of `NUM_PRIORITIES`
/// short deques, taken and released one at a time, no allocation and no UART inside. The census walks
/// the queue rather than calling `len()` because `pinned` cannot be counted any other way; the queues
/// it walks are the same ones `len()` already sums over.
fn emit_spread_witness(n: usize, seen: usize) {
    use core::fmt::Write;

    /// One core's placement row. `Copy` and plain, so the masked snapshot allocates nothing.
    #[derive(Clone, Copy)]
    struct SpreadRow {
        dispatching: bool,
        running: bool,
        ready: usize,
        pinned: usize,
    }

    let mut rows =
        [SpreadRow { dispatching: false, running: false, ready: 0, pinned: 0 }; MAX_CPUS];
    x86_64::instructions::interrupts::without_interrupts(|| {
        for (c, row) in rows.iter_mut().enumerate().take(n) {
            row.dispatching = cpu_dispatching(c);
            // TESTED, not dereferenced — see the doc above.
            row.running = SCHED[c].current.load(Ordering::Acquire) != 0;
            let (ready, pinned) = RUN_QUEUES[c].lock().census();
            row.ready = ready;
            row.pinned = pinned;
        }
    });

    let mut pack = 0usize;
    let mut spare = 0usize;
    for row in rows.iter().take(n) {
        if !row.dispatching {
            continue;
        }
        let runnable = usize::from(row.running) + row.ready;
        if runnable >= 2 {
            pack += 1;
        } else if runnable == 0 {
            spare += 1;
        }
    }

    let mut w = LineBuf::new();
    let _ = write!(w, "[spread] pack={} spare={} rqp=[", pack, spare);
    for (c, row) in rows.iter().enumerate().take(n) {
        // A core that is not dispatching gets `--` rather than `0/0/0`, the same distinction
        // `[schedx86] load` draws between a measured zero and an absent measurement: a core that
        // never entered `run()` is not an idle core, and printing it as one would let the witness
        // certify spare capacity the machine does not have.
        let sep = if c == 0 { "" } else { "," };
        if row.dispatching {
            let _ = write!(
                w,
                "{}{}/{}/{}",
                sep,
                u8::from(row.running),
                row.ready,
                row.pinned
            );
        } else {
            let _ = write!(w, "{}--", sep);
        }
    }
    let (moves, passes) = steal_counters();
    let _ = write!(
        w,
        "] steal={}/{} m1={} mh={} remig={} cool={} packseen={} cr3sw={}",
        moves,
        passes,
        STEAL_M_DEPTH1.load(Ordering::Relaxed),
        STEAL_M_HINT.load(Ordering::Relaxed),
        STEAL_REMIGS.load(Ordering::Relaxed),
        STEAL_COOL_SKIP.load(Ordering::Relaxed),
        PACK_SEEN.load(Ordering::Relaxed),
        CR3_SWITCHES.load(Ordering::Relaxed),
    );
    let _ = write!(
        w,
        " decl=t:{} e:{} f:{} p:{} d:{} i:{}",
        STEAL_D_THIEF.load(Ordering::Relaxed),
        STEAL_D_EMPTY.load(Ordering::Relaxed),
        STEAL_D_FLOOR.load(Ordering::Relaxed),
        STEAL_D_PINNED.load(Ordering::Relaxed),
        STEAL_D_DRAIN.load(Ordering::Relaxed),
        STEAL_D_IDLEFLOOR.load(Ordering::Relaxed),
    );
    // Review F13: carry the column cap onto THIS line too. `pack` and `spare` are sums over the
    // columns printed, so a capped machine under-reports packing and over-reports spare capacity —
    // the one direction in which this witness could certify headroom that is not there.
    if seen > n {
        let _ = write!(w, " cores={}/{} <CAPPED>", n, seen);
    }
    if w.overflow {
        serial_println!("{} <TRUNCATED>", w.as_str());
    } else {
        serial_println!("{}", w.as_str());
    }
}

// ── STORM-X86: the launch-boundary headroom probe ────────────────────────────────────────────────
//
// WHY IT EXISTS ON THIS ARCH. The `storm` verb raises a vug fleet in one command; on aarch64 it has
// always carried a census at the launch boundary, and the argument for that census is arch-neutral:
// every OTHER scheduler quantity is sampled on a clock of its own — `[schedx86] load` rides
// `x86_render_service`'s multi-second heartbeat, the vug CPU-pulse meter rides frames — and the few
// seconds in which a fleet is being BUILT are shorter than one of those windows. The launch boundary
// is the only clock that samples the machine at the instant its size changes. A `storm` that printed
// no census would be a load generator with no instrument, which is not what the verb is for.
//
// WHAT ITS SILENCE MEANS — the same rule as aarch64, for the same structural reason. Every `[storm]`
// line is emitted from the SHELL task, so it can only run while the shell is dispatched, and a fleet
// that starves the shell is one of the outcomes the probe is hunting. Its silence therefore refutes
// nothing. Two properties make that silence READABLE: `pre` is emitted before the first launch and
// one line after EACH successful launch, so the last `[storm] k=` on the wire names the launch after
// which the shell stopped reporting. On x86 NO instrument survives a starved shell — the
// `[schedx86] load` heartbeat is emitted from `x86_render_service`, the SAME task that dispatches
// shell commands, so both stop together (review-corrected: the aarch64 census can lean on its
// timer-driven train; this one cannot). A truncated `[storm]` tail plus a stopped `load` train is
// ONE silence, not two witnesses — never read a missing `post` as a clean run.
//
// WHAT IS DELIBERATELY DIFFERENT FROM THE aarch64 CENSUS, stated so a two-arch capture is not
// mis-read as a regression on one of them:
//   * no EL0 `runnable/committed` pair. x86 keeps no per-core ring-3 residency counters (aarch64's
//     `EL0_RESIDENTS` has no twin here), and the honest report of a quantity this arch does not
//     measure is its ABSENCE, not a zero. The ring-3 population is reported instead by the process
//     rows and user slots the verb prints around these lines.
//   * no `below-band` run-queue depth. There is no service priority band on x86 (`spawn_prio` and
//     `PRIO_SERVICE` are aarch64's), so `q` is the whole ready queue and there is nothing to split
//     it against.
//   * `busy` uses the THREE-form rendering of [`emit_load_witness`] verbatim — `NN%` measured,
//     `NN%*(name)` inferred, `--` absent. That distinction is the load instrument's whole thesis on
//     this arch, and a boundary sample that flattened it back into a bare percent would be the one
//     place a stale or deduced number is most likely to be read as a measurement.
//
// COST. One `RUN_QUEUES[c]` acquisition per core, held across `len()` only, taken with interrupts
// masked — the same bounded, non-nested snapshot `emit_load_witness` takes and for the identical
// WEDGE-4 `<W1>` reason (`run_queue_len` locks without masking; this runs from a preemptible task).
// The formatting and the UART write happen with IF restored. Once per launch, never per frame.

/// STORM-X86 — one boundary sample: per-core saturation, ready-queue depth, and cumulative context
/// switches, under the caller's `phase` label so a capture reads back in launch order.
///
/// Arch-neutral name-and-shape mirror of aarch64's `storm_probe`; see the block above for what the
/// fields mean on THIS arch, which two aarch64 columns are deliberately absent, and what this line's
/// absence does not prove.
pub fn storm_probe(phase: &str) {
    use core::fmt::Write;
    let seen = crate::arch::acpi::cpu_count().max(1);
    let n = meter_cpu_count();

    // ── snapshot (IF=0) — R1/H1's requirement, not tidiness: see the block above ────────────────
    let mut rows = [LoadRow::blank(); MAX_CPUS];
    x86_64::instructions::interrupts::without_interrupts(|| {
        for (c, row) in rows.iter_mut().enumerate().take(n) {
            let ld = core_load(c);
            *row = LoadRow {
                pct: ld.busy_pct_recent,
                tracked: ld.tracked,
                pegged: ld.pegged,
                name: ld.last_task,
                sw: ld.ctx_switches,
                q: run_queue_len(c),
            };
        }
    });

    // ── format + emit (IF restored) ─────────────────────────────────────────────────────────────
    let mut w = LineBuf::new();
    let _ = write!(w, "[storm] {} | busy", phase);
    for (c, row) in rows.iter().enumerate().take(n) {
        if !row.tracked {
            let _ = write!(w, " c{}=--", c);
        } else if row.pegged {
            let (nm, clip) = peg_name(row.name);
            let _ = write!(w, " c{}={}%*({}{})", c, row.pct, nm, clip);
        } else {
            let _ = write!(w, " c{}={}%", c, row.pct);
        }
    }
    let mut ctx = 0u64;
    let _ = write!(w, " | rq(ready)");
    for (c, row) in rows.iter().enumerate().take(n) {
        let _ = write!(w, " c{}={}", c, row.q);
        ctx += row.sw;
    }
    let _ = write!(w, " | ctx={}", ctx);
    if seen > n {
        let _ = write!(w, " cores={}/{} <CAPPED>", n, seen);
    }
    if w.overflow {
        serial_println!("{} <TRUNCATED>", w.as_str());
    } else {
        serial_println!("{}", w.as_str());
    }
}

/// STORM-X86 — the FULL boundary block, emitted at the two ends of a storm (the per-launch lines in
/// between are [`storm_probe`] alone, which is the cheap half).
///
/// It is [`storm_probe`] followed by the standing load witness re-emitted at THIS instant instead of
/// on its own heartbeat, tagged with the phase so a capture can tell a boundary emission from a
/// periodic one. That pairing is the point: the `[storm]` line carries the boundary-specific totals,
/// and the `[schedx86] load` line beside it is in the exact wording the rest of the capture uses, so
/// a storm run and a steady-state run are read with one vocabulary.
///
/// aarch64's census additionally chains `[pulse5]`/`[spread4]`/`[prio]`; those instruments have no
/// x86 twin in this arc, and re-emitting a line this arch does not produce is not available to be
/// got wrong. Nothing here CONSUMES a periodic instrument's state — `emit_load_witness` only reads
/// the per-core accounts, so calling it at a boundary does not shorten any window the heartbeat is
/// about to report (the defect aarch64's census documents at `prio_witness`).
pub fn storm_census(phase: &str) {
    storm_probe(phase);
    // VUGSPREAD (review F17/F7c): re-arm the per-steal witness at the launch boundary.
    //
    // `STEAL_LOG_MAX` names the first 24 migrations of a boot and then goes quiet forever. Before
    // this arc that cap was generous — Boot AS spent its whole 24 on nothing, because there was ONE
    // steal in ten minutes. With the corrector actually able to see the packing it is the opposite
    // problem: a settling fleet burns the cap during early boot on migrations nobody is asking
    // about, and the storm that the arc is *for* then reports its moves as an uninterpretable
    // increment on `steal=`, with no names, no source cores and no `m=` churn counts. The boot that
    // is supposed to settle this question would be the one boot that could not answer it.
    //
    // Reset at the boundary rather than raising the cap: the cap is what keeps a thrashing fleet
    // from flooding the wire, and raising it would trade a bounded silence for an unbounded flood.
    // Zeroing here re-arms exactly 24 named lines per storm phase, which is a launch's worth. The
    // cumulative `steal=`/`m1`/`mh`/`remig` totals on `[spread]` are untouched by this and keep
    // counting across the reset, so nothing measured is lost — only the naming is renewed.
    STEAL_LOG_COUNT.store(0, Ordering::Relaxed);
    emit_load_witness(&alloc::format!("-storm-{}", phase));
}

/// A task name clipped to [`PEG_NAME_CAP`] bytes at a UTF-8 character boundary, so the pegged-core
/// suffix cannot blow the line bound derived at [`LINEBUF_CAP`]. Returns the slice and a marker that
/// is `"+"` when clipping occurred — a silently shortened name is a name the reader would mis-match
/// against a spawn site, which is the same lie class `<TRUNCATED>` and `<CAPPED>` exist to prevent.
fn peg_name(name: &'static str) -> (&'static str, &'static str) {
    if name.len() <= PEG_NAME_CAP {
        return (name, "");
    }
    let mut end = PEG_NAME_CAP;
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    (&name[..end], "+")
}

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
        // R1 / rtpi: enqueue at the EFFECTIVE (inheritance-aware) priority, so a boosted holder that
        // becomes runnable lands above the mid-priority tasks it must not sit behind. The knob-off arm
        // is the pre-arc expression verbatim (`sched_prio` exists only in the `rtpi` build), so an
        // unarmed build is byte-identical here.
        #[cfg(feature = "rtpi")]
        let level = (sched_prio(&task) as usize).min(NUM_PRIORITIES - 1);
        #[cfg(not(feature = "rtpi"))]
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
        // R1 / rtpi: the decay floor is the EFFECTIVE priority, so a boosted holder preempted
        // mid-critical-section re-enqueues at (at least) its inherited level rather than decaying
        // toward its base and back under the mid-priority tasks. Knob-off arm is the pre-arc verbatim.
        #[cfg(feature = "rtpi")]
        let base = (sched_prio(&task) as usize).min(NUM_PRIORITIES - 1);
        #[cfg(not(feature = "rtpi"))]
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
    /// SMPBAL-X86 — remove and return the first STEAL-ELIGIBLE ready task for a thief on `thief_cpu`,
    /// scanning LOW→HIGH priority (take a core's BACKGROUND work first, never rob it of its most
    /// urgent task — note `.iter_mut()`, NOT the `.rev()` `pop_highest` uses) and front-first within a
    /// level (oldest waiter). Returns `None` when the queue holds only pinned or excluded work.
    ///
    /// Runs on the THIEF's core under the VICTIM's run-queue lock. Every task in a run queue is
    /// `STATE_READY` — a running task is out of the queue in `current`, a blocked one is in a wait
    /// queue or a sleeper list — so anything reachable here is safe to re-home. `wait_ticks` and
    /// `effective_level` are read/written under this same lock, satisfying their "never cross-CPU"
    /// discipline; the thief's `push` re-bases both — which is a BEHAVIOUR CHANGE, not an
    /// assurance (review C6): a stolen task loses its aging promotion and restarts at base
    /// priority on the thief. Benign for the ring-3 fleet this arc places (they re-age in ms),
    /// stated so nobody reads a stolen task's level reset as a scheduler bug.
    ///
    /// THREE filters, and all three are load-bearing:
    ///   * `steal_ok` — the pin contract (`Task::steal_ok`). Render / input / usb-pump and every
    ///     fixture named a core, so they are skipped and left exactly where they were placed.
    ///   * cooperative ring-3 onto core 0 — a task with RFLAGS.IF clear in ring 3 masks the timer for
    ///     its lifetime, and core 0 is the sole advancer of the global ms-clock. `pick_cpu` encodes
    ///     the same rule for PLACEMENT; it has to be repeated here because a later migration is a
    ///     placement decision that `pick_cpu` never sees.
    ///   * VUGSPREAD-COOL — a task that migrated within `STEAL_COOLDOWN_MS` is left where it is, so it
    ///     runs a few quanta on its new home before another idle core yanks it back. This is the
    ///     RUNNING-victim ping-pong brake; each skip bumps [`STEAL_COOL_SKIP`]. A task whose only
    ///     obstacle was the cooldown is passed over exactly like a pinned one, so an empty return still
    ///     reads as `p`/[`STEAL_D_PINNED`] at the pass level — see that counter's doc.
    ///
    /// `now_ms` is `arch::ms()`, read ONCE by the caller and threaded in: it is globally coherent
    /// (`APIC_TICKS`), so comparing it against each task's `migrate_ms` is a sound cross-core elapsed,
    /// and reading it once keeps every candidate in this walk judged against one instant.
    ///
    /// O(ready tasks) worst case, and off the switch hot path — only an idle core with an empty queue
    /// ever calls it.
    fn steal_one(&mut self, thief_cpu: usize, now_ms: u64) -> Option<Box<Task>> {
        for level in self.levels.iter_mut() {
            let pos = level.iter().position(|t| {
                if !t.steal_ok || (thief_cpu == 0 && t.is_cooperative_user()) {
                    return false;
                }
                // VUGSPREAD-COOL — refuse a re-steal inside the cooldown window. `migrate_ms == 0`
                // (never migrated) always clears it, so the first corrective steal is never delayed.
                #[cfg(not(feature = "rtpi"))]
                if t.migrate_ms != 0 && now_ms.saturating_sub(t.migrate_ms) < STEAL_COOLDOWN_MS {
                    STEAL_COOL_SKIP.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
                // R1 / rtpi — the same cooldown, with ONE exemption: a PRIORITY-BOOSTED holder
                // (`sched_prio > priority` — it holds a lock a strictly-higher task is blocked on) is
                // ALWAYS stealable, cooldown or not. Holding such a task on a saturated home for up to
                // `STEAL_COOLDOWN_MS` is a bounded priority-inversion window — precisely what this arc
                // exists to close — so an idle core must be free to pull it and run the critical
                // section out, releasing the high-priority waiter. VUGSPREAD-COOL's ping-pong brake and
                // priority inheritance thus compose without reopening the inversion the cooldown would
                // otherwise create. (Knob-off, the `not(rtpi)` branch above is vug-storm's exact code.)
                #[cfg(feature = "rtpi")]
                if t.migrate_ms != 0
                    && now_ms.saturating_sub(t.migrate_ms) < STEAL_COOLDOWN_MS
                    && sched_prio(t.as_ref()) <= t.priority
                {
                    STEAL_COOL_SKIP.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
                true
            });
            if let Some(pos) = pos {
                return level.remove(pos);
            }
        }
        None
    }
    /// VUGSPREAD: `(ready, pinned)` for this queue — how many READY tasks it holds, and how many of
    /// those [`steal_one`](Self::steal_one) may never take (`!steal_ok`). Taken under the queue's own
    /// lock, by the `[spread]` witness only; never consulted on a scheduling path.
    ///
    /// The PAIR is the instrument. `ready` alone cannot separate "this core is packed and no idle
    /// core has looked yet" from "this core is packed with work no idle core is ALLOWED to take",
    /// and those two diagnoses want opposite repairs — the first a corrector that runs, the second a
    /// pin contract that is too wide. A census that printed one number would read identically under
    /// both, which is the class of witness this tree does not accept.
    ///
    /// `pinned` counts the `steal_ok` filter ONLY, not `steal_one`'s second filter (a cooperative
    /// ring-3 task that a core-0 thief must skip). Those two differ: this census is a property of the
    /// QUEUE, while the second filter is a property of the (queue, thief) pair and would print a
    /// different number for every core that read it. Review F8-doc — say the bound rather than imply
    /// it: this is O(ready tasks) over `NUM_PRIORITIES` deques, i.e. the same walk `len()` sums over,
    /// and it is bounded in practice by `RUNQ_CAPACITY` (64) per level rather than by anything
    /// structural. It runs inside the witness's masked section, at the witness's rate, and nowhere
    /// else.
    fn census(&self) -> (usize, usize) {
        let mut ready = 0;
        let mut pinned = 0;
        for level in self.levels.iter() {
            for t in level.iter() {
                ready += 1;
                if !t.steal_ok {
                    pinned += 1;
                }
            }
        }
        (ready, pinned)
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

    /// R1 / rtpi — RELOCATE a READY task UP to `level` for priority inheritance. Finds the `Box` whose
    /// ADDRESS equals the identity token `owner` across all levels; if it sits BELOW `level`, removes
    /// and re-inserts it at `level` (updating `effective_level` to match, the invariant `age` keeps).
    /// Returns `true` iff a match was found (whether or not it needed moving).
    ///
    /// Runs under this queue's own lock (the DONOR may be on another CPU — the same cross-CPU-under-
    /// the-owner's-lock discipline `try_steal` uses), so touching `effective_level` here is sound
    /// exactly as the thief's `push` is. `owner` is COMPARED by address, never dereferenced, and the
    /// task it matches is dereferenced only through the queue's OWN live `Box` — so a stale/reused
    /// `owner` at worst relocates a wrong ready task upward; the unearned elevation then decays one
    /// level per requeue (`requeue` steps `effective_level` down a single level per dispatch, it does
    /// not snap back to `sched_prio` in one step) — never a use-after-free. A task NOT found raced out
    /// of the queue; its held locks' boosts re-level it on its next enqueue. O(ready tasks), off the
    /// hot path.
    #[cfg(feature = "rtpi")]
    fn pi_relocate(&mut self, owner: u64, level: usize) -> bool {
        let level = level.min(NUM_PRIORITIES - 1);
        for l in 0..NUM_PRIORITIES {
            if let Some(pos) = self.levels[l]
                .iter()
                .position(|t| t.as_ref() as *const Task as u64 == owner)
            {
                if l >= level {
                    return true; // already at or above the target level — nothing to do
                }
                let mut task = self.levels[l].remove(pos).expect("pi_relocate: pos/remove mismatch");
                task.effective_level = level as u8;
                self.levels[level].push_back(task);
                return true;
            }
        }
        false
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

/// WEDGE-4 probe (x86 half of the cross-arch W4 candidate, relayed r23s1q). `RUN_QUEUES`' doc says
/// the lock is "held only briefly" — but briefly is not the same as ATOMICALLY. The dispatcher
/// (`run`) and `timer_preempt` both operate with IF=0, while the spawn/wake paths (`spawn_inner`,
/// `spawn_user_inner`, `make_ready`) acquire a queue lock with IF possibly 1. A quantum expiry
/// landing inside one of those unmasked critical sections switches the holder out MID-HOLD; the
/// scheduler context then takes `RUN_QUEUES[cpu].lock()` IRQ-masked and spins on a lock whose
/// holder can only ever run again through this very dispatcher. Permanent, silent, no panic —
/// the P66/P68/s44 death shape, needing no Pi hardware.
///
/// Two probes, both using the WEDGE-2 raw-byte primitive (no lock, bounded poll) so the instrument
/// cannot itself block on anything the dying chain holds:
/// * `<W1>` — `timer_preempt` fired while this CPU was inside an unmasked run-queue critical
///   section AND is about to context-switch away from it. The window exists; capped at 16 emits.
/// * `<W2>` — the dispatcher's own run-queue acquisition exceeded its spin bound (~seconds). The
///   wedge is HAPPENING on this core, named at wedge time instead of spinning silently. Capped at
///   4 emits, then falls back to the ordinary blocking acquisition (behaviour unchanged).
///
/// Knob-gated with the rest of the wedge instrumentation family (`UNAOS_WEDGE2=1`): default-off
/// builds carry no flag writes, no tokens, no extra branch in the dispatch loop.
#[cfg(feature = "wedge2")]
mod wedge4 {
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    /// Per-CPU: "this CPU is inside a run-queue critical section it entered with IRQs unmasked."
    /// Set BEFORE the acquisition begins (the spin is part of the window) and cleared after the
    /// guard drops. Relaxed everywhere: it gates a diagnostic token and orders nothing.
    pub static IN_RQ: [AtomicBool; super::MAX_CPUS] =
        [const { AtomicBool::new(false) }; super::MAX_CPUS];
    static W1_EMITS: AtomicU32 = AtomicU32::new(0);
    static W2_EMITS: AtomicU32 = AtomicU32::new(0);

    /// Mark this CPU inside the window. Caller clears with [`leave`] on the SAME INDEX it entered
    /// with, and every call site captures that index into a stack local before `enter` and passes
    /// the same local to `leave` — so the flag it sets is the flag it clears.
    ///
    /// SMPBAL-X86 CORRECTION. The justification used to be "a preempted task resumes on the
    /// queue-owning CPU it was pinned to". That is no longer true — `try_steal` migrates tasks — but
    /// the mechanism survives unchanged, because it never depended on the pin: it depends on the
    /// captured local, which travels with the task. What DOES change is the meaning of the `<W1>`
    /// token on the wire: read it as "some core was mid-enqueue", not "this core was". A task that
    /// migrates between `enter` and `leave` leaves the flag set on the core it entered on, which is
    /// the core whose lock it actually took, so the attribution is still the honest one.
    pub fn enter(cpu: usize) {
        IN_RQ[cpu].store(true, Ordering::Relaxed);
    }
    pub fn leave(cpu: usize) {
        IN_RQ[cpu].store(false, Ordering::Relaxed);
    }

    // Through `wedge2::mark`, not a local loop: `mark` is `#[inline(never)]`, which is what keeps
    // the token a real string in the image — the strings census depends on that (a local loop gets
    // unrolled into byte constants and the census reads 0 for a token that emits fine).
    fn emit(counter: &AtomicU32, cap: u32, tok: &str) {
        if counter.fetch_add(1, Ordering::Relaxed) < cap {
            crate::wedge2::mark(tok);
        }
    }

    /// W4-A: called from `timer_preempt` (IF=0) just before it switches away from the current task.
    pub fn note_preempt_in_rq(cpu: usize) {
        if IN_RQ[cpu].load(Ordering::Relaxed) {
            emit(&W1_EMITS, 16, "<W1>");
        }
    }

    /// W4-B: acquire `m` with a bounded spin; name the stall on the wire if the bound trips, then
    /// block exactly as the un-instrumented path would. The bound (~2e8 polls) is seconds of wall
    /// clock — three orders of magnitude past any legitimate hold of this lock.
    pub fn lock_or_squawk<'a, T>(m: &'a super::SpinMutex<T>) -> spin::MutexGuard<'a, T, spin::Spin> {
        let mut spins: u64 = 0;
        loop {
            if let Some(g) = m.try_lock() {
                return g;
            }
            spins += 1;
            if spins == 200_000_000 {
                emit(&W2_EMITS, 4, "<W2>");
            }
            core::hint::spin_loop();
        }
    }
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

    // WINX-7: the completion `post()` moved DOWN into `exit()`, which this call reaches immediately.
    // It used to live here, which was correct while only KERNEL threads were joinable — a kernel
    // thread's one completion edge is "entry returned". A ring-3 THREAD (`spawn_user_thread`) never
    // runs `entry` at all: it `iretq`s to ring 3 and finishes at `SYS_THREAD_EXIT`, a fault-kill, or
    // the scheduler's `KillSwitch` reap, none of which pass through here. Putting the post at the
    // single terminus every task funnels through means one implementation covers all four edges and
    // a joiner can never be stranded on a thread that died some way its author did not anticipate.
    unsafe {
        debug_assert!(
            (*raw).state.load(Ordering::Acquire) == STATE_RUNNING && (*raw).cpu as usize == cpu,
            "task_trampoline: task not running on its own CPU at completion"
        );
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
/// the new task's id. `target_cpu` must be an online AP, or [`CPU_AUTO`] to let `pick_cpu` place it
/// (which also makes it steal-eligible — see `Task::steal_ok`).
fn spawn_inner(
    name: &'static str,
    entry: fn(usize),
    arg: usize,
    target_cpu: usize,
    priority: u8,
    done_sem: Option<Arc<Semaphore>>,
) -> u64 {
    // SMPBAL-X86: the pin contract is decided from the REQUESTED value, before placement resolves it.
    // A kernel task is never a cooperative ring-3 task, so the core-0 exclusion does not apply.
    let steal_ok = target_cpu == CPU_AUTO;
    let target_cpu = pick_cpu(target_cpu, false, name);
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
        steal_ok,
        migrations: 0,
        migrate_ms: 0,
        hint_placed: false,
        #[cfg(feature = "rtpi")]
        held: [const { AtomicU64::new(0) }; PI_HELD_MAX],
    });

    // WEDGE-4 `<W1>` window: this acquisition can run with IF=1; see `wedge4`.
    #[cfg(feature = "wedge2")]
    let w4cpu = percpu::this_cpu().cpu_index as usize;
    #[cfg(feature = "wedge2")]
    wedge4::enter(w4cpu);
    RUN_QUEUES[target_cpu].lock().push(task);
    #[cfg(feature = "wedge2")]
    wedge4::leave(w4cpu);
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
    let (entry, user_rsp, preemptible, thread_arg) =
        unsafe { ((*raw).user_entry, (*raw).user_rsp, (*raw).preemptible, (*raw).arg) };
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
    //
    // WINX-7 — the ONE EXCEPTION, and it is a DELIBERATE ABI VALUE rather than kernel residue: `rdi`
    // carries this task's `arg`, so a `SYS_THREAD_SPAWN`ed ring-3 thread enters its `extern "C"` worker
    // entry with the SysV first argument already in place (the aarch64 twin puts the same word in x0;
    // see `spawn_user_thread`). It is bound as an explicit `in("rdi")` operand — NOT left to the
    // register allocator — precisely so the scrub below can drop its `xor edi, edi` without opening a
    // hole: every task that is not a thread carries `arg == 0`, so rdi is provably zero for the U1a /
    // U2 / U3 / `run` / `bg` paths and the scrub property is preserved bit for bit. The value is one
    // the CALLER of `SYS_THREAD_SPAWN` chose from inside its own address space, so it discloses
    // nothing the program did not already know.
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
            // NO `xor edi, edi` — rdi is the thread-argument ABI register (see the note above). It is
            // an explicit `in("rdi")` operand and is 0 for every non-thread task.
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
            in("rdi") thread_arg as u64, // WINX-7: SysV arg0 for a SYS_THREAD_SPAWNed worker; 0 otherwise
            options(noreturn),
        );
    }
}

/// Create a ready ring-3 (user-mode) task on `target_cpu`'s run queue (U1a): when dispatched it
/// drops to ring 3 at `user_entry` with rsp = `user_rsp` (both from `syscall::setup`) and calls
/// back into the kernel via `syscall`. MUST be spawned on a core that is RUNNING THE SCHEDULER LOOP
/// — `user_task_trampoline` reads `SCHED[cpu].current`, which is null on a core that never dispatches.
/// Since SCHED-X86 that includes core 0 (the BSP calls `run_bsp` at the GUI handoff), so the
/// constraint is "a dispatching core", not "an AP"; a core still parked in `wait_and_run` is still
/// illegal. One placement rule survives and is SHARPER than the old one: this entry point builds a
/// COOPERATIVE ring-3 task (`INITIAL_RFLAGS` has IF=0), and core 0 is the sole advancer of the global
/// ms-clock, so a cooperative user task on core 0 would freeze `arch::ms()` for its lifetime — never
/// place one there. `spawn_user_preemptible` (IF=1) has no such restriction.
/// Fire-and-forget: `sys_exit` marks it FINISHED and the scheduler reclaims it. Returns the task id.
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
    // SMPBAL-X86: pin contract from the REQUESTED value; `!preemptible` here means a COOPERATIVE
    // ring-3 task, which `pick_cpu` must keep off core 0 (the global ms-clock) — and which
    // `steal_one` must likewise never move onto core 0 later.
    let steal_ok = target_cpu == CPU_AUTO;
    let target_cpu = pick_cpu(target_cpu, !preemptible, name);
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
        steal_ok,
        migrations: 0,
        migrate_ms: 0,
        hint_placed: false,
        #[cfg(feature = "rtpi")]
        held: [const { AtomicU64::new(0) }; PI_HELD_MAX],
    });
    // WEDGE-4 `<W1>` window: this acquisition can run with IF=1; see `wedge4`.
    #[cfg(feature = "wedge2")]
    let w4cpu = percpu::this_cpu().cpu_index as usize;
    #[cfg(feature = "wedge2")]
    wedge4::enter(w4cpu);
    RUN_QUEUES[target_cpu].lock().push(task);
    #[cfg(feature = "wedge2")]
    wedge4::leave(w4cpu);
    poke_for(target_cpu, PRIO_NORMAL);
    id
}

// ---------------------------------------------------------------------------------------------
// WINX-7: ring-3 THREADS — several ring-3 tasks sharing ONE address space.
//
// `spawn_user_in_space` / `spawn_user_preemptible` already put a ring-3 task in a private CR3, and
// U4x's dispatch-site install of CR3 + TSS.RSP0 + `syscall_kernel_rsp` already made a SECOND
// concurrent user task per core safe (each task's syscalls and faults land on its OWN kernel stack,
// never a sibling's). What was missing for threads is not scheduling at all — it is LIFETIME. Two
// things follow from several tasks sharing one `user_cr3`:
//
//   1. TEARDOWN MUST BE REFCOUNTED. The pre-WINX-7 `exit`/reap paths called
//      `memory::free_user_space_by_cr3` unconditionally, so the first thread to finish freed the
//      slot — retiring its siblings' compositor windows, wiping the shared handle row, and handing
//      the backing frames to the next `alloc_user_space` while live ring-3 code was still executing
//      out of them. `user_space_retain`/`user_space_release` below make the LAST holder the one that
//      frees, which is the same rule aarch64 gets from `boot::slot_thread_retain` +
//      `teardown_user_slot`'s per-thread refcount.
//   2. A THREAD MUST BE JOINABLE. `spawn_user_thread` gives the task a `done_sem` (the same
//      `Arc<Semaphore>` machinery `spawn_joinable` uses for kernel threads) and hands the caller a
//      `JoinHandle`; the completion is posted at `exit()`, which is the single terminus every task
//      reaches however it dies.
//
// Threads are spawned PREEMPTIBLE (RFLAGS.IF set in ring 3). That is not a convenience: a worker
// thread's whole job is to run concurrently with its parent, and a cooperative (IF-masked) ring-3
// task runs to completion FIFO — so a co-located cooperative worker would monopolise its core until
// it made a syscall, and a `user-vug` worker's inner rasterisation loop makes none. The U3.5
// preemptible path is exactly the primitive this needs and it is already proven on the wire
// (`:: U3.5: ring-3 preemption — IRQs-at-ring3=…, co-task ran, spinner resumed -> PASS ::`).
// ---------------------------------------------------------------------------------------------

/// WINX-7: per-slot EXTRA-HOLDER counts for the user address spaces. `USER_SPACE_REFS[s]` is the
/// number of live tasks in slot `s` BEYOND the first — i.e. the number of `SYS_THREAD_SPAWN`ed ring-3
/// threads that have not yet retired. A single-task process keeps this at 0, so its first (and only)
/// `user_space_release` frees the slot and every pre-WINX-7 path behaves byte-identically.
///
/// Relaxed-free accounting is deliberately NOT used: `retain` and `release` can run on different
/// cores (a parent spawning on core A while a worker exits on core B), and the decision the counter
/// drives is "may I free this address space?", so both sides are `AcqRel`.
static USER_SPACE_REFS: [AtomicU32; crate::arch::memory::USER_SLOTS] =
    [const { AtomicU32::new(0) }; crate::arch::memory::USER_SLOTS];

/// TEARDOWN-1: per-slot ADDRESS-SPACE DOOM — "every ring-3 task under this slot is owed its death".
///
/// WHY A KILL MUST BE ADDRESS-SPACE SCOPED, not task scoped. `SYS_THREAD_SPAWN` gives a ring-3 program
/// several tasks under one slot, and only the LAST of them to leave releases the address space (see
/// `USER_SPACE_REFS`). A `KillSwitch` names ONE task — the leader `run`/`bg` spawned — so killing it
/// reaped the leader and left its workers running: the refcount never reached zero, so
/// `free_user_space_by_cr3` never ran, so the slot's compositor windows were never retired. That is
/// exactly what the WINX-8 teardown leg read as `cleared=false` — a killed vug whose window stayed on the
/// panel, owned by a process the operator had been told was dead, with the slot unrecyclable and the
/// workers burning their cores against a parent that no longer exists.
///
/// A worker thread carries no `KillSwitch` of its own (nothing hands one out per thread, and sharing the
/// leader's Arc would let the first thread out publish `reaped` for the whole process). So the scope lives
/// here instead, in the one place that is genuinely shared: the slot. `reap_killed` arms it as the last
/// act of retiring a task that owned an address space, and every sibling then matches the same armed
/// predicate at its own next kill boundary.
///
/// QUIESCENCE IS PRESERVED, NOT WEAKENED. Nothing is reclaimed here. Each sibling still retires through
/// `reap_killed` and decrements `USER_SPACE_REFS` itself; only when that reaches zero does the slot free.
/// This makes that edge REACHABLE — it does not move it earlier.
static SLOT_DOOMED: [AtomicBool; crate::arch::memory::USER_SLOTS] =
    [const { AtomicBool::new(false) }; crate::arch::memory::USER_SLOTS];

/// TEARDOWN-1: arm the address-space doom for the slot rooted at `cr3`. Idempotent; a `cr3` that is not a
/// live slot root is ignored (there is no address space to scope to). Disarmed on the slot's real free
/// edge in [`user_space_release`].
fn doom_address_space(cr3: u64) {
    if let Some(s) = cr3_slot(cr3) {
        SLOT_DOOMED[s].store(true, Ordering::Release);
    }
}

/// TEARDOWN-1: is the address space rooted at `cr3` doomed?
fn cr3_doomed(cr3: u64) -> bool {
    cr3 != 0 && cr3_slot(cr3).is_some_and(|s| SLOT_DOOMED[s].load(Ordering::Acquire))
}

/// TEARDOWN-1: THE kill predicate — is this task owed its death? Either its own `KillSwitch` is armed
/// (the leader a requester named) or its address space is doomed (a sibling thread of that leader).
/// Every kill boundary and every eviction sweep tests exactly this, so the two scopes can never drift.
fn task_kill_armed(task: &Task) -> bool {
    task.kill.as_ref().is_some_and(|k| k.is_requested()) || cr3_doomed(task.user_cr3)
}

/// WINX-7: the slot index whose page-table root is `cr3`, or `None` for the shared kernel window.
/// The `memory::current_slot` shape, but keyed off a STORED cr3 rather than the live one — a task's
/// `user_cr3` field is the authority on which address space it belongs to, and at reap time the live
/// CR3 has already been restored to the kernel's.
fn cr3_slot(cr3: u64) -> Option<usize> {
    (0..crate::arch::memory::USER_SLOTS).find(|&s| crate::arch::memory::slot_cr3(s) == cr3)
}

/// WINX-7: claim one extra hold on the address space `cr3` — called by the syscall layer BEFORE it
/// spawns a thread into that space, so the slot cannot be torn down in the window between the
/// decision to spawn and the new task's first dispatch. Idempotent-free and unbounded in principle;
/// bounded in practice by the syscall layer's fixed thread table.
pub fn user_space_retain(cr3: u64) {
    if let Some(s) = cr3_slot(cr3) {
        USER_SPACE_REFS[s].fetch_add(1, Ordering::AcqRel);
    }
}

/// WINX-7: drop one hold on the address space `cr3`, freeing it only when the LAST holder leaves.
/// The caller MUST already have restored the kernel CR3 (that `mov cr3` full-flush is what retires
/// this slot's user TLB entries) — the same precondition `free_user_space_by_cr3` has always had.
///
/// The count is decremented with a saturating CAS loop rather than `fetch_sub`, so a stray extra
/// release (a path that retained nothing) cannot wrap the counter to `u32::MAX` and make the slot
/// immortal — the fail-closed direction here is "free it", not "leak it forever".
fn user_space_release(cr3: u64) {
    let Some(s) = cr3_slot(cr3) else {
        // Not one of ours (a CR3 that is not a live slot root — nothing to free, nothing to count).
        return;
    };
    let mut cur = USER_SPACE_REFS[s].load(Ordering::Acquire);
    loop {
        if cur == 0 {
            // TEARDOWN-1: the slot is going away, so DISARM its doom before the root can be recycled.
            // Cleared here — on the real 1->0 free edge, under the same "last holder" test that frees —
            // because that is the only moment at which no task can still be looking for it. Leaving it
            // set would hand the doom to the NEXT tenant of this slot, which would kill a brand-new
            // program at its first syscall.
            SLOT_DOOMED[s].store(false, Ordering::Release);
            // Last holder: free the slot for real. `free_user_space_by_cr3` retires the slot's
            // compositor windows, drops its FB leaves, clears its handle/file rows and releases the
            // used-flag — the clear-before-release discipline it already documents.
            crate::arch::memory::free_user_space_by_cr3(cr3);
            return;
        }
        match USER_SPACE_REFS[s].compare_exchange_weak(
            cur,
            cur - 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return, // a sibling is still live — the slot stays
            Err(observed) => cur = observed,
        }
    }
}

/// WINX-7: create a ring-3 THREAD inside the EXISTING address space `user_cr3` — a second (third, …)
/// task under the same page-table root as its parent, so a memory word is coherent across them and a
/// futex on that word is a real cross-thread wait/wake.
///
/// `user_entry` and `user_rsp` are VAs the CALLER carved out of its OWN window; the syscall layer
/// validates them against that window before we are called (this function performs no validation and
/// must not be reached from anywhere else). `arg` is delivered in `rdi` — SysV arg0 — by
/// `user_task_trampoline`, so a worker written as `extern "C" fn(usize)` receives it directly.
///
/// The returned `JoinHandle` is the caller's ONLY way to wait for this thread; the completion permit
/// is posted at the thread's `exit()` (whatever ends it). The caller MUST have called
/// `user_space_retain(user_cr3)` first — the retain has to precede the enqueue, because a preemptible
/// task can be dispatched on another core the instant it is pushed.
pub fn spawn_user_thread(
    name: &'static str,
    user_entry: u64,
    user_rsp: u64,
    arg: usize,
    target_cpu: usize,
    user_cr3: u64,
) -> JoinHandle {
    // SMPBAL-X86: a thread is spawned PREEMPTIBLE, so the core-0 exclusion does not apply.
    //
    // VUGSPREAD — this used to read `let steal_ok = target_cpu == CPU_AUTO;`, and the note beside it
    // observed, without alarm, that `sys_thread_spawn` always names a core so `steal_ok` is always
    // false. That was the defect, stated in the code and not recognised as one.
    //
    // Read the pin contract for what it says: a caller that NAMED a core gets that core forever. It
    // exists so that render, input, usb-pump and the fixtures — kernel spawn sites that name a core
    // because the core is part of their correctness — are never migrated. A `SYS_THREAD_SPAWN` core
    // is not that. Ring 3 passes `place` ∈ {0 = my core, 1 = a sibling}, which is a HINT about
    // locality expressed in the only vocabulary the syscall has; the kernel then resolves it to an
    // index. Marking the result a PIN promoted a user-space hint into a kernel guarantee, and the
    // consequence was measured on Boot AS: a vug is a parent plus two workers, `place=0` puts one
    // worker on the parent's own core BY REQUEST, and nothing in the system could ever undo it — two
    // ring-3 threads time-sharing one core at 19 fps while c3 and c4 sat at 0 %, for ten minutes.
    //
    // So a ring-3 thread is steal-eligible, full stop. Its placement is still honoured — it starts
    // exactly where `sys_thread_spawn` asked — and an idle core may correct it later, which is the
    // arch's whole stated model ("placement is a one-shot hint and `try_steal` is the correction").
    // Nothing about the migration is new or unproven here: a thread is preemptible, `steal_one`
    // already excludes the one class that must not move (a cooperative ring-3 task onto core 0, and
    // a thread is not cooperative), and the TLB obligation is discharged by `AS_GEN` exactly as it is
    // for the `bg-user` PROCESS this thread shares an address space with — a process that has been
    // steal-eligible since SMPBAL-X86 landed. Making the parent movable and its threads immovable was
    // never a considered position.
    //
    // REVIEW F2 — THE ARGUMENT ABOVE IS SCOPED TO THIS FUNCTION'S ONE CALLER, and that scope is now
    // asserted rather than assumed. Everything that makes the release safe is a property of a
    // `SYS_THREAD_SPAWN` thread: it is preemptible (so it is not `steal_one`'s excluded class), it
    // has a private address space whose TLB obligation `AS_GEN` discharges, and its core is a ring-3
    // HINT rather than a correctness requirement. A future KERNEL caller reaching this function with
    // a core that is part of its correctness would be handed `steal_ok = true` and silently migrated
    // — the exact failure the pin contract exists to prevent, arriving from the other direction.
    //
    // **STOP if you are about to add a second caller.** The unconditional `steal_ok` is not a
    // property of this function; it is a property of ring-3 threads. A caller that needs a real pin
    // needs a parameter here, not a comment.
    debug_assert!(
        user_cr3 != 0 && user_entry != 0,
        "spawn_user_thread: steal_ok is unconditional here on the strength of this being a ring-3 \
         thread — a kernel caller must not reach it"
    );
    let steal_ok = true;
    let target_cpu = pick_cpu(target_cpu, false, name);
    assert!(target_cpu < MAX_CPUS, "spawn_user_thread: target_cpu out of range");
    assert!(user_cr3 != 0, "spawn_user_thread: a thread needs a real private address space");
    let done = Arc::new(Semaphore::new(0));
    done.init(); // reserve the waiter list BEFORE the thread can run + post (alloc-free park)

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
        arg,               // -> rdi at first entry (the thread ABI; see `user_task_trampoline`)
        cpu: target_cpu as u32,
        priority: PRIO_NORMAL,
        wait_ticks: 0,
        effective_level: 0, // re-based to `priority` by push() on enqueue (just below)
        done_sem: Some(done.clone()),
        user_entry,
        user_rsp,
        user_cr3,
        preemptible: true, // see the section header: a worker must share its core, not own it
        kill: None,
        steal_ok,
        migrations: 0,
        migrate_ms: 0,
        // VUGSPREAD (review F16): this core came from ring 3's `place` argument. Attribution only.
        hint_placed: true,
        #[cfg(feature = "rtpi")]
        held: [const { AtomicU64::new(0) }; PI_HELD_MAX],
    });
    // WEDGE-4 `<W1>` window: this acquisition can run with IF=1; see `wedge4`. Same idiom as every
    // other enqueue site (`spawn_inner` / `spawn_user_inner` / `make_ready`) — no new lock ORDER is
    // introduced here, only one more instance of the existing one.
    #[cfg(feature = "wedge2")]
    let w4cpu = percpu::this_cpu().cpu_index as usize;
    #[cfg(feature = "wedge2")]
    wedge4::enter(w4cpu);
    RUN_QUEUES[target_cpu].lock().push(task);
    #[cfg(feature = "wedge2")]
    wedge4::leave(w4cpu);
    poke_for(target_cpu, PRIO_NORMAL);
    JoinHandle { done, id }
}

/// WINX-7: pick a core for a thread whose caller asked for a SIBLING (`place == 1`) — an online core
/// that is not `caller`, or `caller` itself when this machine is scheduling only one core. The
/// aarch64 twin is `sched::other_online_cpu`.
///
/// "Online" here means DISPATCHING, and that distinction is the WINX-2 `bg_place_cpu` lesson written
/// down: `meter_cpu_count()` reports how many cores the meter knows about, which is not the same as
/// how many were released into `run()`. A thread placed on a core that never dispatches is spawned,
/// never run, and never joined — a silent hang at the parent's frame barrier. So the probe is a core
/// that has actually PUBLISHED a scheduler context (`scheduler_rsp != 0`, written by the first
/// `switch_context` that core's `run()` performed); a core still sitting in `wait_and_run` has not.
/// VUGSPREAD — IT WAS "A SIBLING", AND IT MEANT "THE SAME SIBLING, EVERY TIME".
///
/// The scan above returned the FIRST core matching the probe, in index order, with no reference to
/// what that core was already doing. On an eight-core machine every `place=1` thread in the system
/// therefore landed on the same low-numbered core, and a second vug's worker joined the first vug's
/// worker there while the high-numbered cores stayed empty. Boot AS shows the shape from the other
/// end: the CPU-pulse census reads c0 and c5 pegged, c3 and c4 at exactly `busy/idle=0/250`.
///
/// "A sibling" was always a policy, not a constraint — the caller asked for "not my core", nothing
/// more — so the choice among the eligible cores is free, and it now uses the SAME key chain
/// [`pick_cpu`] uses: shallowest ready queue first (an exact instantaneous count), lowest rolling
/// busy percent as the tie-break (a ~250 ms lagging window), then the rotating cursor so full ties
/// fill round-robin rather than all landing on the lowest index. Sharing the cursor with `pick_cpu`
/// is deliberate: a process and the threads it spawns are placed against one rotation, not two that
/// can synchronise.
///
/// The ELIGIBILITY probe is unchanged on purpose — still `scheduler_rsp != 0`, still the WINX-2
/// `bg_place_cpu` lesson (a thread placed on a core that never dispatches is spawned, never run, and
/// never joined: a silent hang at the parent's frame barrier). This arc changes WHICH eligible core
/// is chosen, and nothing about which cores are eligible.
///
/// Render and service are DEPRIORITISED, not excluded — preferred away from first, accepted if they
/// are all that is dispatching. Excluding them outright would reintroduce the hang this function's
/// probe exists to prevent, on a machine small enough that they are the only siblings.
///
/// The ladder here is TWO tiers — `{render, service}` excluded, then nothing excluded — and review
/// F21 is right that this is NOT `pick_cpu`'s ladder, which is THREE (`{render, service}`, then
/// `{render}`, then nothing). The difference is deliberate, and is written down so the two are not
/// "unified" later by someone reading a looser earlier wording. `pick_cpu`'s middle rung keeps a
/// long-lived program off the SERVICE core one rung longer than off the render core, because that
/// rung is where the `xhci_worker_cpu` storage-latency preference lives. Here the caller's own core
/// is already excluded, so by the time the first rung fails the machine has at most two dispatching
/// cores left and there is no third candidate for a middle rung to choose between. Ordering
/// render-before-service at that point would be a distinction with no set to apply it to.
///
/// LOCKING: `run_queue_len`'s WEDGE-4 `<W1>` shape. The caller is `sys_thread_spawn`, i.e. syscall
/// context at IF possibly 1, so the whole scan is taken inside `without_interrupts` for the identical
/// reason `pick_cpu` and `emit_load_witness` do — at most `MAX_CPUS` iterations, one run-queue lock
/// at a time, never nested, no allocation and no UART inside.
pub fn sibling_online_cpu(caller: usize) -> usize {
    let render = crate::arch::smp::render_cpu();
    let service = crate::arch::smp::service_cpu();
    let rot = AUTO_ROTATE.fetch_add(1, Ordering::Relaxed);
    let mut best: Option<(usize, usize, u32)> = None; // (cpu, depth, pct)
    x86_64::instructions::interrupts::without_interrupts(|| {
        for tier in 0..2u8 {
            for i in 0..MAX_CPUS {
                let c = (rot + i) % MAX_CPUS;
                if c == caller || SCHED[c].scheduler_rsp.load(Ordering::Acquire) == 0 {
                    continue;
                }
                if tier < 1 && (render == Some(c) || service == Some(c)) {
                    continue;
                }
                let depth = RUN_QUEUES[c].lock().len();
                // Cross-core read: `busy_pct`'s own contract says pass 0 for `live` here, because
                // `live_span_cyc` would subtract another core's `rdtsc` anchor from ours.
                let pct = ACCT[c].busy_pct(0);
                let better = match best {
                    None => true,
                    Some((_, bd, bp)) => depth < bd || (depth == bd && pct < bp),
                };
                if better {
                    best = Some((c, depth, pct));
                }
            }
            if best.is_some() {
                return;
            }
        }
    });
    best.map_or(caller, |(c, _, _)| c)
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
    /// (it blocks); the `assert` rejects a call off the scheduler — e.g. a boot-path caller that is
    /// not itself a task, or a core still parked in `wait_and_run` — loudly rather than silently
    /// returning as if the task had finished.
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
    /// blocking to advance the deadline. The assert rejects a call off the scheduler (a boot-path
    /// caller that is not a task) loudly — there `sleep_ticks` is a no-op, which would busy-spin the
    /// poll.
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

// ── SMPBAL-X86: PLACEMENT (`CPU_AUTO`) ──────────────────────────────────────────────────────────
//
// Until this arc every x86 spawn site named a core and that decision was final: `make_ready`
// enqueued on `task.cpu` unconditionally, nothing re-placed at wake, and `run()`'s empty-queue arm
// went straight to `enable_and_hlt`. The measured consequence on the bench rMBP was six ring-3 vugs
// on ONE core with five cores at 0 %, because every `bg`/`run` launch inherits the shell's core and
// the shell is the render task.
//
// `CPU_AUTO` is the opt-in half of the fix (the corrective half is `try_steal`). A caller that names
// a core still gets exactly that core — the PIN CONTRACT, checked first in `pick_cpu` — so every
// pre-existing spawn site is unchanged. A caller that passes `CPU_AUTO` is placed on the least-loaded
// DISPATCHING core and is marked `steal_ok`, which is what makes it correctable later.
//
// x86 deliberately does NOT get the aarch64 SPREAD-4…15 apparatus: no re-place at wake, no margin,
// no freshness gate, no co-placement or recruit lanes. That layer exists on aarch64 solely because
// EL0 tasks there cannot be stolen, and its own file records that three successive arcs of it were
// "correct on a saturated board and wrong on an idle one". Here, placement is a one-shot hint and
// `try_steal` is the correction; if placement guesses wrong an idle core fixes it within one idle
// pass, which is a far shorter feedback loop than any tuning constant.

/// SMPBAL-X86: sentinel `target_cpu` meaning "don't pin me — place me on the least-loaded dispatching
/// core, and let an idle core steal me later". The aarch64 twin is `aarch64::sched::CPU_AUTO`.
pub const CPU_AUTO: usize = usize::MAX;

/// SMPBAL-X86: cores that have entered `run()` and are therefore actually DISPATCHING — the candidate
/// set for `CPU_AUTO` placement and the victim/thief set for stealing.
///
/// "Online" must mean dispatching, not merely brought up. That is the WINX-2 `bg_place_cpu` lesson
/// written into a data structure: `meter_cpu_count()` reports how many cores the METER knows about,
/// and a task placed on a core that is online but still sitting in `wait_and_run` is spawned, never
/// dispatched, never joined — a silent hang with no witness. `sibling_online_cpu` probes
/// `scheduler_rsp != 0` for the same reason; this flag is the explicit version, set at the top of
/// `run()` (which is the one function every core — APs via `wait_and_run`, the BSP via `run_bsp` —
/// must pass through), so it is true strictly before that core can pop its first task.
static ONLINE_MASK: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

/// SMPBAL-X86: rotating start index for `pick_cpu`'s scan, so fully-tied cores fill round-robin
/// instead of every tie landing on the lowest index. Introspection-grade ordering (`Relaxed`) — a
/// racing read at worst re-uses a start position, which costs nothing but a tie broken the same way
/// twice.
static AUTO_ROTATE: AtomicUsize = AtomicUsize::new(0);

/// SMPBAL-X86: register `cpu` as dispatching. Called from `run()` before its first pop.
fn mark_online(cpu: usize) {
    if cpu < MAX_CPUS {
        ONLINE_MASK[cpu].store(true, Ordering::Release);
    }
}

/// SMPBAL-X86: is `cpu` dispatching? Introspection + the placement/steal candidate test.
#[inline]
fn cpu_dispatching(cpu: usize) -> bool {
    cpu < MAX_CPUS && ONLINE_MASK[cpu].load(Ordering::Acquire)
}

/// SMPBAL-X86: rate limit for the one-shot `SCHEDPLACE-X86` witness — the first `PLACE_LOG_MAX`
/// auto-placements are named on the wire, then it goes quiet. Enough to cover a six-vug launch at the
/// bench without a busy desktop flooding the log.
const PLACE_LOG_MAX: u32 = 24;
static PLACE_LOG_COUNT: AtomicU32 = AtomicU32::new(0);

/// SMPBAL-X86: choose the core a `CPU_AUTO` spawn lands on. Returns `requested` unchanged for every
/// other value — the PIN CONTRACT, and it is checked FIRST so a named core can never be second-guessed.
///
/// THREE exclusions, and they are not the same kind of rule.
///
///   1. **Core 0 for a COOPERATIVE ring-3 task** (`cooperative_user`, i.e.
///      `Task::is_cooperative_user`). HARD — it is a correctness rule, never relaxed at any tier: a
///      ring-3 task with IF=0 on core 0 masks the timer for its lifetime and freezes the global
///      ms-clock, because `timer_interrupt_handler`'s `cpu_index == 0` arm is the sole advancer of
///      `APIC_TICKS`.
///   2. **The SERVICE core** (`smp::service_cpu`). This is the `xhci_worker_cpu` rule. It was a
///      DEADLOCK rule before WEDGE-8; today it is a SERIALISATION/LATENCY rule (review-corrected):
///      WEDGE-8 made `XHCI_CONTROLLER` a masked O(1) loan — `claim()` and `Drop` take the mutex
///      only under `IrqMask`, so no holder can be preempted mid-hold and the preempt-the-holder
///      deadlock is structurally impossible. What remains true: a ring-3 program placed there
///      contends the service core's storage latency, and `xhci_worker_cpu` still DECLINES to
///      co-locate. The tier-1 relaxation needs a 2-dispatching-core machine to fire — unreachable
///      on the 8-core bench — and is safe when it does, merely slower.
///   3. **The RENDER core.** Performance: it owns the panel and hosts the shell, it is the core the
///      measured imbalance piled onto, and putting a fresh program back on it restores the defect
///      this arc exists to remove.
///
/// Rules 2 and 3 RELAX in tiers (both, then render only, then neither) so a machine with too few
/// dispatching cores still places somewhere real instead of failing; rule 1 never relaxes. The tier
/// ladder deliberately mirrors `smp::worker_pool`'s `Exclusive` / `SvcShared` / `RenderShared`.
///
/// Key chain, best first: (1) shallowest ready queue, (2) lowest rolling busy percent from the
/// SCHEDLOAD-X86 feed (an UNTRACKED core scores 0 — it has folded no span, and a core that just
/// entered `run()` genuinely has no load), (3) the rotating cursor. Depth leads because it is an
/// exact instantaneous count while the percent is a ~250 ms lagging window; the percent breaks ties
/// between equally-shallow queues, which is the common case on an idle desktop.
///
/// LOCKING. `run_queue_len` takes a run-queue lock with no IRQ masking of its own, and this runs from
/// the spawn paths at IF possibly 1 — the WEDGE-4 `<W1>` shape. The whole scan is therefore taken
/// inside `without_interrupts`, exactly as `emit_load_witness` does and for the same reason: preempt
/// a task while it holds `RUN_QUEUES[c]` and that core's `run()` then spins on it at IF=0 forever.
/// The section is bounded — at most `MAX_CPUS` iterations of one lock acquisition held across
/// `len()`, taken and released one at a time, never nested, no allocation and no UART inside.
fn pick_cpu(requested: usize, cooperative_user: bool, name: &'static str) -> usize {
    if requested != CPU_AUTO {
        return requested;
    }
    let here = percpu::this_cpu().cpu_index as usize;
    let render = crate::arch::smp::render_cpu();
    let service = crate::arch::smp::service_cpu();
    let rot = AUTO_ROTATE.fetch_add(1, Ordering::Relaxed);

    // Tier ladder: exclude {render, service}, then {render}, then nothing. Rule 1 (core 0 for a
    // cooperative ring-3 task) is outside the ladder — it never relaxes.
    let mut best: Option<(usize, usize, u32)> = None; // (cpu, depth, pct)
    x86_64::instructions::interrupts::without_interrupts(|| {
        for tier in 0..3u8 {
            for i in 0..MAX_CPUS {
                let c = (rot + i) % MAX_CPUS;
                if !cpu_dispatching(c) {
                    continue;
                }
                if cooperative_user && c == 0 {
                    continue; // hard rule: never a cooperative ring-3 task on the clock core
                }
                if tier < 2 && render == Some(c) {
                    continue;
                }
                if tier < 1 && service == Some(c) {
                    continue;
                }
                let depth = RUN_QUEUES[c].lock().len();
                // `live = 0`: this is a CROSS-core read, and `live_span_cyc` would subtract another
                // core's `rdtsc` anchor from ours. `busy_pct`'s own contract says pass 0 here. A core
                // that has never folded a span scores 0, which is the truth for a core that just
                // entered `run()`.
                let pct = ACCT[c].busy_pct(0);
                let better = match best {
                    None => true,
                    Some((_, bd, bp)) => depth < bd || (depth == bd && pct < bp),
                };
                if better {
                    best = Some((c, depth, pct));
                }
            }
            if best.is_some() {
                return;
            }
        }
    });

    let Some((cpu, depth, pct)) = best else {
        // Nothing is dispatching yet (pre-`enable()` spawn). Fall back to the caller's core, which is
        // the pre-arc behaviour and definitionally reachable — the caller is executing on it.
        // Review C1: the fallback must not silently relax rule 1 (cooperative ring-3 on the clock
        // core freezes the ms-clock). Latent today — both CPU_AUTO producers spawn preemptible —
        // but the guard is one spawn site away from live, so it is asserted, not assumed.
        debug_assert!(
            !(cooperative_user && here == 0),
            "pick_cpu fallback would place a cooperative ring-3 task on the clock core"
        );
        return here;
    };
    if PLACE_LOG_COUNT.fetch_add(1, Ordering::Relaxed) < PLACE_LOG_MAX {
        serial_println!(
            ":: SCHEDPLACE-X86: '{}' -> c{} (q={} load={}% from c{}) ::",
            name,
            cpu,
            depth,
            pct,
            here
        );
    }
    cpu
}

/// Mark a parked/just-woken task READY, push it onto its CURRENT HOME CPU's run queue, and poke that
/// CPU (waking it or preempting a lower-priority task). Used by the sleeper drain (same CPU) and
/// `Semaphore::post` (cross-CPU wake). Caller runs with IF=0.
///
/// SMPBAL-X86 — this doc used to read "the task always returns to `task.cpu`, so its GS base stays
/// correct on resume (tasks don't migrate)". The MECHANISM is unchanged and still correct; the reason
/// given for it was wrong twice over. Tasks now migrate (`try_steal`), and `task.cpu` is simply the
/// task's CURRENT home — re-homed by the stealer before the push, so a later wake delivers the task
/// to where it now lives. And GS base was never a per-task property: it is pure per-core state
/// programmed in `percpu::init_cpu`, so a migrated task reading the NEW core's per-CPU block is the
/// correct behaviour, not a hazard. What actually had to be repaired for migration is the TLB — see
/// `AS_GEN` in `memory.rs`.
///
/// x86 deliberately has no `rewake_place` twin: re-placing on every wake is what drove aarch64's
/// SPREAD-4/5/6 churn (`rewake=3256 and climbing` on a six-vug fleet). Correction happens on the idle
/// side instead, where it costs an idle core's spare cycles rather than a waking task's latency.
fn make_ready(task: Box<Task>) {
    let target = task.cpu as usize;
    // R1 / rtpi: poke at the EFFECTIVE priority so a woken boosted holder preempts a mid-priority
    // task on its target core. Knob-off arm is the pre-arc `task.priority` verbatim — byte-identical.
    #[cfg(feature = "rtpi")]
    let prio = sched_prio(&task);
    #[cfg(not(feature = "rtpi"))]
    let prio = task.priority;
    debug_assert!(target < MAX_CPUS, "make_ready: cpu out of range");
    task.state.store(STATE_READY, Ordering::Release);
    // WEDGE-4 `<W1>` window: this acquisition can run with IF=1; see `wedge4`.
    #[cfg(feature = "wedge2")]
    let w4cpu = percpu::this_cpu().cpu_index as usize;
    #[cfg(feature = "wedge2")]
    wedge4::enter(w4cpu);
    RUN_QUEUES[target].lock().push(task);
    #[cfg(feature = "wedge2")]
    wedge4::leave(w4cpu);
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
        // WINX-7: signal completion to any joiner FIRST — before the address space is torn down and
        // before the state flip. This is the single terminus every task reaches (a kernel thread via
        // `task_trampoline`, a ring-3 process via `SYS_EXIT`, a ring-3 thread via `SYS_THREAD_EXIT`, a
        // faulting ring-3 task via `ring3_fault_kill`), which is exactly why the post belongs here
        // and not in the trampoline. The task's OWN `done_sem` Arc clone is the liveness anchor for
        // this `post()` — it MUST remain in the Box until `run()` drops it on the Finished path, so
        // we BORROW it (never take/move the Arc out). `post()` may `make_ready` a joiner parked on
        // another core; we are IF=0 and hold no lock, so that is the ordinary cross-CPU wake.
        if let Some(sem) = &(*raw).done_sem {
            sem.post();
        }
        // U3: if this task owned a private address space, tear it down HERE — restore the shared
        // kernel CR3 (that `mov cr3` full-flush retires this process's user TLB entries), THEN free
        // the slot. Order matters: free-after-restore, so no core is left on the dead root. We run on
        // this task's own kernel stack + scheduler code, both Global in the kernel half (shared into
        // every process root), so restoring the kernel CR3 doesn't pull the stack out from under us.
        //
        // WINX-7: the free is now REFCOUNTED (`user_space_release`) because an address space can hold
        // several tasks — a process plus its `SYS_THREAD_SPAWN`ed ring-3 threads. The pre-WINX-7 code
        // freed unconditionally, which for a threaded program meant the FIRST thread to finish pulled
        // the slot (and its window surfaces, and its handle row) out from under its still-running
        // siblings. `user_space_release` frees only on the LAST holder's exit; every earlier exit
        // still restores the kernel CR3, which is both harmless (the next dispatch installs whatever
        // the incoming task needs) and necessary (this core must not sit on a root it no longer has
        // a task for). Single-task programs are unchanged: their refcount is 0, so the first release
        // IS the last one.
        let user_cr3 = (*raw).user_cr3;
        if user_cr3 != 0 {
            crate::arch::memory::restore_kernel_cr3();
            // VUGSPREAD: keep the `cr3_live` shadow honest across a teardown restore — see `SchedCpu::cr3_live`.
            SCHED[cpu].cr3_live.store(crate::arch::memory::kernel_cr3(), Ordering::Relaxed);
            user_space_release(user_cr3);
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
/// No-op outside a scheduled task (a boot-path caller that is not itself a task), like `yield_now`.
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
    /// permit is held. Returns `false` WITHOUT acquiring if called off a scheduled task (a boot-path
    /// caller / the scheduler's own idle context) — there is no `current` to block, so it cannot
    /// wait. A
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
// WINX-7: FUTEX — a keyed ring-3 wait/wake primitive (backs SYS_FUTEX(26))
// ---------------------------------------------------------------------------------------------
//
// A futex lets ring 3 build a userspace mutex/condvar out of one shared u32: block on that word iff it
// still holds an expected value, and wake N blocked waiters. It is what makes a ring-3 thread pool
// possible at all — `user-vug`'s per-frame barrier is exactly "the parent blocks until both workers
// have bumped `done`", and without a real park that is a spin that burns a core per waiting thread.
//
// THE KEY IS `(slot, uaddr)`, NOT A PHYSICAL ADDRESS — the one deliberate divergence from the
// aarch64 twin, and it is a simplification rather than a weakening. aarch64 keys on the word's PA
// because a PA is globally unique across address spaces. On x86 the same uniqueness comes free from
// the pair: two threads share a key iff they are in the same slot AND naming the same VA, which is
// precisely "the same word", because threads of one process share one page-table root and distinct
// processes share no user memory at all. Deriving a PA would mean a page-table walk from the
// scheduler (a `memory` API this layer has no business reaching for) to re-establish a property the
// slot index already carries. The cost is honest and stated: if x86 ever grows genuine cross-process
// shared memory, a futex on it will need the PA key back.
//
// Everything else is the aarch64 shape verbatim, because the shape is the correctness argument: the
// compare-and-block happens UNDER the bucket lock that any `futex_wake` on the same key must also
// take, so a wake can never slip between the compare and the park being enqueued (the classic
// race-free compare-and-block). The park itself reuses the `Semaphore` PARK_WAITQ lock-handoff
// unchanged — the blocking task holds the bucket lock across the switch and the SCHEDULER pushes its
// Box and releases the lock — so the lost-wakeup proof is the one already written for `Semaphore`.
//
// LOCK ORDER: a bucket lock is a WAIT-QUEUE lock and sits exactly where `Semaphore::locked` sits —
// `futex_wake` pops a waiter under the bucket lock but calls `make_ready` only AFTER releasing it, so
// the run-queue lock is never nested under a bucket lock and the existing acyclic order holds.

/// Distinct futex keys the kernel can have waiters parked on at once (the same fixed discipline as
/// `USER_SLOTS` / the process table).
///
/// HEADROOM — raised 16 -> 64, catching up to the identical raise aarch64 made at VUGPAUSE-2 and
/// which x86 never took. The doc line that used to sit here ("`user-vug` uses ONE key per process")
/// is why the audit had to reach this constant at all, and it is the pre-VUGPAUSE-2 claim: a vug
/// holds THREE live keys while idle — its `DONE` barrier word, the `PHASE` release word BOTH workers
/// park on, and its input ring.
///
/// THE ARITHMETIC THIS HAD TO SATISFY. At 3 keys per program and `MAX_PROCS` = 10, a full fleet
/// wants 30 buckets; 16 could not seat even six. And the overflow does not fail loudly — that is the
/// whole danger. `futex_wait` returns `TableFull`, `sys_futex` degrades it to `-EAGAIN` and
/// `SYS_INPUT_WAIT` degrades it to a yield, so the programs that lost the race quietly stop parking
/// and start SPINNING: a fleet of identical programs running at two different speeds with nothing in
/// any program to explain it, which is precisely the symptom HEADROOM exists to remove. It would
/// have replaced the `NTHREAD` cliff with a futex cliff and looked the same from the panel.
///
/// 64 rather than a tighter fit, for the reason aarch64 states: a full `USER_SLOTS` fleet must not
/// be able to reach it, and a bucket is a lock, a key and a `VecDeque` header. Matching aarch64's
/// width also means the two arches' vug frame loops now pay the SAME bucket-scan cost, which is the
/// one real price here (`futex_wait`'s selection is a linear scan under each bucket's raw lock, on
/// the barrier hot path) — and it is a price aarch64 has been paying, measured, since VUGPAUSE-2.
const NFUTEX: usize = 64;

/// One futex wait bucket: a keyed FIFO wait queue with a `Semaphore`-style raw lock handed to the
/// scheduler at park time.
struct FutexBucket {
    /// Raw spinlock guarding `key` + `waiters` (Acquire on lock, Release on unlock; the PARK_WAITQ
    /// lock-handoff releases it AFTER the scheduler has pushed the blocking Box).
    locked: AtomicBool,
    /// The key this bucket serves, or 0 = free. Claimed by the first waiter for a key, released back
    /// to 0 when its last waiter leaves.
    key: AtomicU64,
    /// FIFO waiter list; touched only under `locked`, pre-reserved by `futex_init`.
    waiters: UnsafeCell<VecDeque<Box<Task>>>,
}

// SAFETY: every access to `key`/`waiters` is serialised by `locked` — identical argument to
// `Semaphore`'s, including the park-side push performed by the scheduler while the lock is still
// held, which establishes happens-before with the next `futex_wake` that acquires it.
unsafe impl Sync for FutexBucket {}

impl FutexBucket {
    const fn new() -> Self {
        FutexBucket {
            locked: AtomicBool::new(false),
            key: AtomicU64::new(0),
            waiters: UnsafeCell::new(VecDeque::new()),
        }
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
    #[inline]
    fn waiters_empty(&self) -> bool {
        unsafe { (*self.waiters.get()).is_empty() }
    }
}

static FUTEX: [FutexBucket; NFUTEX] = [const { FutexBucket::new() }; NFUTEX];

/// Reserve every futex bucket's waiter capacity on the BSP before any task can park on one, so the
/// scheduler's park-side `push_back` never reallocates (and so never takes the heap lock) under the
/// handed-off bucket lock. Called from [`init`].
pub fn futex_init() {
    for b in FUTEX.iter() {
        b.lock_raw();
        unsafe { (*b.waiters.get()).reserve(WAIT_CAPACITY) };
        b.unlock_raw();
    }
}

/// Outcome of [`futex_wait`].
pub enum FutexWait {
    /// Was blocked, then woken by a `futex_wake` on the same key.
    Woken,
    /// `*uaddr != expected` at the compare — the caller must re-check and loop (no sleep happened).
    Mismatch,
    /// Every bucket is busy with a DIFFERENT live key — the fixed futex pool is exhausted.
    TableFull,
    /// Called off a scheduled task (no `current` to park) — cannot block.
    NoTask,
    /// TEARDOWN-1: the caller has an ARMED kill and so was refused the park. No sleep happened; the
    /// caller must not loop, because it is about to be retired at the syscall boundary on the way out.
    Killed,
}

/// Build the futex key for `uaddr` in address-space `slot`. Non-zero by construction (the slot index
/// is biased by 1, and the top byte of a user VA is 0 — `USER_BASE` is 1 TiB and the window is
/// 16 KiB + the FB region, so bits [63:56] are provably clear and cannot collide with the tag).
pub fn futex_key(slot: usize, uaddr: u64) -> u64 {
    debug_assert!(uaddr >> 56 == 0, "futex_key: user VA overlaps the slot tag");
    (((slot as u64) + 1) << 56) | (uaddr & 0x00FF_FFFF_FFFF_FFFF)
}

/// FUTEX_WAIT: block the current task on `key` iff the u32 at `uaddr` still equals `expected`.
///
/// `uaddr` must ALREADY be validated by the syscall layer as a 4-aligned, writable address inside the
/// CALLER's own ring-3 window, and `key` must be `futex_key(caller_slot, uaddr)`; this function
/// dereferences `uaddr` at CPL 0 under the caller's still-live CR3 (a syscall runs in the caller's
/// address space) and performs no validation of its own.
pub fn futex_wait(key: u64, uaddr: u64, expected: u32) -> FutexWait {
    debug_assert!(key != 0, "futex key must be non-zero");
    let was_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable(); // IF=0 for the whole critical section

    // Select the bucket: an existing one serving this key, else claim a free one. Left LOCKED on
    // success — the compare and the park both happen under that same hold.
    let mut chosen: Option<&FutexBucket> = None;
    for b in FUTEX.iter() {
        b.lock_raw();
        if b.key.load(Ordering::Relaxed) == key {
            chosen = Some(b);
            break;
        }
        b.unlock_raw();
    }
    let b = match chosen {
        Some(b) => b,
        None => {
            let mut claimed = None;
            for b in FUTEX.iter() {
                b.lock_raw();
                // FUTEX-DUP — the claim pass JOINS a bucket another waiter keyed to `key` between our
                // find scan above and this pass. The find scan holds each bucket lock only while it
                // inspects that bucket, so it is not atomic across the table, and IF=0 buys nothing
                // here: this is SMP with a scheduler per CPU. Two waiters entering together on a key
                // with no standing bucket could therefore both finish the find scan empty, and a
                // claim pass testing `== 0` ALONE would mint a SECOND bucket for the same key.
                //
                // ONE BUCKET PER KEY IS NO LONGER ASSUMED. This check closes the wide window; the
                // full scan in `futex_wake` is what makes duplicates harmless rather than merely
                // rare, and is the correctness argument (a bucket freed by a concurrent drain and
                // reclaimed for `key` mid-scan is a sliver no claim-side test can close).
                let k = b.key.load(Ordering::Relaxed);
                if k == key {
                    FUTEX_DUP.fetch_add(1, Ordering::Relaxed);
                    claimed = Some(b);
                    break;
                }
                if k == 0 {
                    b.key.store(key, Ordering::Relaxed);
                    claimed = Some(b);
                    break;
                }
                b.unlock_raw();
            }
            match claimed {
                Some(b) => b,
                None => {
                    if was_enabled {
                        x86_64::instructions::interrupts::enable();
                    }
                    return FutexWait::TableFull;
                }
            }
        }
    };

    // `b` is locked and serves `key`. Compare-and-block under that lock.
    let cur = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
    if cur != expected {
        if b.waiters_empty() {
            b.key.store(0, Ordering::Relaxed); // release a bucket we claimed but will not park on
        }
        b.unlock_raw();
        if was_enabled {
            x86_64::instructions::interrupts::enable();
        }
        return FutexWait::Mismatch;
    }
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    if raw.is_null() {
        if b.waiters_empty() {
            b.key.store(0, Ordering::Relaxed);
        }
        b.unlock_raw();
        if was_enabled {
            x86_64::instructions::interrupts::enable();
        }
        return FutexWait::NoTask;
    }
    // TEARDOWN-1: the "arm, THEN park" half of the futex leg. `b` has been locked continuously since it
    // was found/claimed and stays locked until the SCHEDULER pushes our Box and releases it, so this
    // test and the publish are inside one hold of the very lock `kill_wake_parked`'s sweep takes — the
    // same losslessness argument the PARK_SLEEP arm of `park_blocked` states. Refuse to park: a task
    // owed its death must not enter a wait nobody may be left to signal. The bucket we may have just
    // claimed is released here on the same terms as the `Mismatch` path above, so a refused park never
    // strands a bucket on a key with no waiters.
    if unsafe { task_kill_armed(&*raw) } {
        if b.waiters_empty() {
            b.key.store(0, Ordering::Relaxed);
        }
        b.unlock_raw();
        if was_enabled {
            x86_64::instructions::interrupts::enable();
        }
        return FutexWait::Killed;
    }
    // Capacity guard: the lock is held continuously until the scheduler pushes, so the length cannot
    // change before then — asserting here proves the park-side push will not reallocate.
    assert!(
        unsafe { (*b.waiters.get()).len() } < WAIT_CAPACITY,
        "futex waiter overflow (raise WAIT_CAPACITY)"
    );
    unsafe {
        debug_assert_eq!((*raw).cpu as usize, cpu, "futex_wait: task on the wrong CPU");
        (*raw).state.store(STATE_BLOCKED, Ordering::Release);
        // Hand the scheduler this bucket's waiter list + lock (the PARK_WAITQ lock-handoff — see
        // `Semaphore::wait` for the full lost-wakeup argument).
        SCHED[cpu].park_waiters.store(b.waiters.get() as u64, Ordering::Relaxed);
        SCHED[cpu].park_lock.store(&b.locked as *const AtomicBool as u64, Ordering::Relaxed);
        SCHED[cpu].park_kind.store(PARK_WAITQ, Ordering::Relaxed);
        switch_context(&raw mut (*raw).ctx_rsp, SCHED[cpu].scheduler_rsp.load(Ordering::Acquire));
    }
    // Resumed (IF=0, carried) once a `futex_wake` moved us back to our run queue; it released the
    // bucket lock, so we must not touch it here.
    if was_enabled {
        x86_64::instructions::interrupts::enable();
    }
    FutexWait::Woken
}

/// FUTEX-DUP — times either side OBSERVED a second bucket serving one key: the claim pass finding the
/// key already claimed since its own find scan, or a wake finding waiters in more than one bucket for
/// its key. Expected to read 0 on a healthy boot; nonzero is the double-claim race observed AND
/// absorbed (before this arc the wake side's `break`-after-first-match made the second bucket's waiter
/// a permanent strand — a parked ring-3 worker nothing would ever name again).
static FUTEX_DUP: AtomicU64 = AtomicU64::new(0);
/// FUTEX-DUP — per-event line budget, on the same terms as every other rate-limited witness here: the
/// first few occurrences name themselves on the wire, [`futex_dup_witness`] carries the steady state.
/// A wake/claim path must never print unconditionally — `user-vug` reaches both once per frame.
static FUTEX_DUP_LOG: AtomicU32 = AtomicU32::new(0);
const FUTEX_DUP_LOG_MAX: u32 = 8;

/// FUTEX-DUP — read the absorbed-race count (0 on a healthy boot).
pub fn futex_dup_count() -> u64 {
    FUTEX_DUP.load(Ordering::Relaxed)
}

/// FUTEX-DUP — the rollup line, emitted once from the WINX-7 futex verdict rather than from any futex
/// path, so the count is on the wire in every headless boot without a hot path ever printing.
pub fn futex_dup_witness() {
    serial_println!(
        "[futexdup] observed={} (duplicate same-key buckets absorbed; 0 = the race never happened)",
        FUTEX_DUP.load(Ordering::Relaxed)
    );
}

/// FUTEX_WAKE: wake up to `n` waiters parked on `key`; returns how many were actually woken. Releases
/// the bucket back to free once its last waiter leaves. Waiters are re-readied OUTSIDE the bucket
/// lock (the run-queue lock must never nest under a wait-queue lock — the `Semaphore::post` rule).
///
/// FUTEX-DUP — the scan visits EVERY bucket serving `key`, and exits early only once `n` waiters have
/// been woken. One bucket per key is an invariant `futex_wait`'s claim pass now defends but cannot
/// guarantee, so THIS is where correctness lives: a wake that stopped at the first match would leave
/// a duplicate bucket's waiters parked on a key no later wake would ever reach. The added cost on the
/// ordinary single-bucket wake is one pass over the remaining bucket keys — a lock, a load, an unlock
/// each, with no waiter traffic.
pub fn futex_wake(key: u64, n: usize) -> usize {
    debug_assert!(key != 0, "futex key must be non-zero");
    let was_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();
    let mut woken = 0usize;
    // Buckets that held at least one waiter for `key` — more than one IS the race, seen.
    let mut buckets_served = 0u32;
    for b in FUTEX.iter() {
        b.lock_raw();
        if b.key.load(Ordering::Relaxed) != key {
            b.unlock_raw();
            continue;
        }
        if !b.waiters_empty() {
            buckets_served += 1;
        }
        while woken < n {
            let next = unsafe { (*b.waiters.get()).pop_front() };
            match next {
                Some(task) => {
                    b.unlock_raw();
                    make_ready(task);
                    woken += 1;
                    b.lock_raw();
                    // A concurrent drain may have freed and reclaimed this bucket for another key
                    // while we were outside the lock; stop rather than pop a stranger's waiter.
                    if b.key.load(Ordering::Relaxed) != key {
                        break;
                    }
                }
                None => break,
            }
        }
        if b.key.load(Ordering::Relaxed) == key && b.waiters_empty() {
            b.key.store(0, Ordering::Relaxed); // last waiter gone — release the bucket
        }
        b.unlock_raw();
        if woken >= n {
            // The wake's budget is spent; semantics are unchanged from the first-match scan. KNOWN
            // BLIND SPOT (cross-seat, 2026-07-29): exiting here means an n==1 wake that finds its
            // waiter in the FIRST bucket serving the key never scans further — a duplicate bucket
            // beyond it is neither counted by [futexdup] nor drained. Only n>=2 wakes can witness a
            // duplicate. The trigger pattern in the tree (vug's PHASE barrier) wakes 2, so it is
            // covered; a future n==1 caller inherits detection-by-luck, not by construction.
            break;
        }
    }
    if buckets_served > 1 {
        // Seen and absorbed. Worth a line while the count is small, because this exact shape used to
        // be a silent permanent strand rather than an event.
        FUTEX_DUP.fetch_add(1, Ordering::Relaxed);
        if FUTEX_DUP_LOG.fetch_add(1, Ordering::Relaxed) < FUTEX_DUP_LOG_MAX {
            serial_println!(
                "[futexdup] wake key={:#x} buckets={} woken={} n={} (double-claim absorbed)",
                key,
                buckets_served,
                woken,
                n
            );
        }
    }
    if was_enabled {
        x86_64::instructions::interrupts::enable();
    }
    woken
}

/// WINX-7 introspection: how many tasks are parked across every futex bucket RIGHT NOW.
///
/// A point-in-time sample, and that is the whole caveat: it answers "is anything parked at this
/// instant", not "did anything park". The WINX-7 witness originally gated on this and was flaky for
/// exactly that reason — a park that begins and ends between two samples is invisible — so the
/// verdict now uses `syscall::futex_park_count()`, a monotonic count of `FUTEX_WAIT`s that blocked
/// and were woken. This function survives as the DEBUGGING view (`is the system parked on a futex
/// right now?`), which the counter cannot answer.
pub fn futex_parked_total() -> usize {
    let was_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();
    let mut n = 0usize;
    for b in FUTEX.iter() {
        b.lock_raw();
        n += unsafe { (*b.waiters.get()).len() };
        b.unlock_raw();
    }
    if was_enabled {
        x86_64::instructions::interrupts::enable();
    }
    n
}

/// WINX7-GO introspection: how many tasks are parked on ONE futex key right now.
///
/// The keyed twin of [`futex_parked_total`], and the difference is the whole reason it exists. The
/// global gauge answers "is ANYTHING parked", which makes any handshake built on it depend on
/// nothing else in the system happening to park at the same moment — a coupling that is invisible
/// in the source and that a placement change elsewhere can break silently. This one answers "is the
/// task I am waiting for parked on the word I am about to write", which is the question a launcher
/// actually has, and it cannot be satisfied by a stranger.
///
/// The same point-in-time caveat applies and the same escape from it: as EVIDENCE that a park
/// happened this is useless (`syscall::futex_park_count()` is the monotonic counter for that), but
/// as a HANDSHAKE it is exact whenever the parked task cannot leave until the caller releases it —
/// the state is level-triggered, so there is no window for the sample to miss.
///
/// FUTEX-DUP: every bucket serving `key` is counted, not just the first, for the same reason
/// `futex_wake` scans them all — one bucket per key is an invariant the claim pass defends but
/// cannot guarantee, and a handshake that undercounted would release its gate early.
pub fn futex_waiters_on(key: u64) -> usize {
    debug_assert!(key != 0, "futex key must be non-zero");
    let was_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();
    let mut n = 0usize;
    for b in FUTEX.iter() {
        b.lock_raw();
        if b.key.load(Ordering::Relaxed) == key {
            n += unsafe { (*b.waiters.get()).len() };
        }
        b.unlock_raw();
    }
    if was_enabled {
        x86_64::instructions::interrupts::enable();
    }
    n
}

// ---------------------------------------------------------------------------------------------
// R1 / rtpi — PRIORITY INHERITANCE on the sleeping Mutex (RECLAMATION-SAFE)
// ---------------------------------------------------------------------------------------------
//
// THE INVERSION x86 HAD. The scheduler runs strict priority + round-robin + anti-starvation aging,
// but NO priority inheritance (`scheduler.md` §5). So a LOW-priority task holding a sleeping `Mutex`
// a HIGH-priority task needs can be preempted by ANY number of MID-priority tasks: the high task then
// waits for the low holder PLUS every mid task in front of it — UNBOUNDED priority inversion.
// `[rtwit]` (R0) measures exactly that lock-hold tail; this rung bounds it.
//
// WHY THE DONATION LIVES ON THE LOCK, NOT THE TASK (the review BLOCKER fix). A first cut wrote the
// boost onto the HOLDER's `Task` and had the donor deref a raw `*mut Task` read from the lock's owner
// field. That is a cross-CPU USE-AFTER-FREE: `owner != 0` proves the holder is alive only
// INSTANTANEOUSLY at the load, not across the load->deref window. Interleaving — donor T on CPU A
// loads `owner = H`; A is preempted (IF=1, a full quantum); H on CPU B drops its guard, `exit()`s,
// and `run()` does `Box::from_raw; drop` — H's Task box is FREED (nothing keeps it alive); A resumes
// and derefs freed memory. A second, free-less manifestation: H merely runs its release
// (`owner=0`, revert) in that window, and the donor's `fetch_max` RESURRECTS a boost on a task that
// no longer holds anything — a permanent leak.
//
// The fix is to make the donation target the LONG-LIVED lock. A blockable `Mutex` is `'static`
// (its own contract), so its embedded [`PiCtl`] never dangles. The donor touches ONLY `PiCtl`s
// (`boost`/`owner`/`owner_waits`), never a `Task`. The holder's `owner` field is used purely as an
// IDENTITY TOKEN — compared for equality (never dereferenced) to find the running core / run-queue
// slot — so a stale-or-reused value is a benign wrong-target boost that self-corrects, never UB.
//
// THE PROTOCOL (minimal, soft-RT — the BEOS-SMP-FLOW R3 shape). Only the sleeping `Mutex`
// participates (the counting `Semaphore`/futex have no single owner; the IRQ-masked `spin::Mutex`es
// in `video/wm.rs` cannot be preempted mid-hold — both out of scope):
//
//   1. ACQUIRE-TIME DONATION (`pi_donate`, from a blocker in `Mutex::lock`). A task blocking on a
//      held lock raises the lock's `PiCtl::boost` to its own effective priority, then ACCELERATES
//      the current holder (found by identity: bump its core's `current_prio` if running, relocate it
//      up if ready) so it out-ranks the mid tasks NOW. If the holder is itself blocked on another PI
//      lock, the boost PROPAGATES down that chain via `PiCtl::owner_waits` (TRANSITIVE), bounded by
//      `PI_CHAIN_MAX`. The holder's own `sched_prio` folds in `boost` whenever it (re)enters a queue.
//   2. HANDOFF INHERITANCE. `boost` stays on the lock across a FIFO handoff, so whoever holds is
//      boosted by whoever waits — a handoff to a lower waiter over a higher one cannot reopen the
//      inversion. `PiCtl::nwait` counts blocked waiters; when it falls to 0 the boost resets.
//   3. REVERT is STRUCTURAL: releasing a lock removes its `PiCtl` from the holder's `held` set, so
//      the holder's `sched_prio` stops folding in that boost — no `Task` field to leak, no
//      resurrection possible. `boost` itself is cleared when the last waiter leaves (`nwait == 0`).
//
// EFFECT ON THE SCHEDULER. `sched_prio` = max(base, max over held locks of `boost`). A boosted holder
// is enqueued/decayed at the inherited level (`push`/`requeue`), dispatched publishing it
// (`current_prio`), relocated up in place if READY (`pi_relocate_scan`), and shielded from a mid wake
// if RUNNING (`pi_bump_running`) — all under the existing best-effort `current_prio` contract.
//
// KNOB-OFF: the entire block is `#[cfg(feature = "rtpi")]`; the `Task`/`Mutex` PI fields do not exist
// and `Mutex::lock` takes its original single-`wait()` path, so an unarmed build is byte-identical.

/// R1 / rtpi — cap on transitive-inheritance chain length, so a (buggy) lock cycle terminates rather
/// than spinning. A real holder chain is a handful deep; the sleeping `Mutex` backs a few kernel
/// structures (`Channel`, `RwLock`), so this is comfortably over any legitimate depth.
#[cfg(feature = "rtpi")]
const PI_CHAIN_MAX: u32 = 8;

/// R1 / rtpi — how many PI locks one task can hold at once with full PI semantics. An overflowed
/// lock (dropped from `held`) still receives direct donations, but the holder stops folding its
/// `boost` into `sched_prio` AND stops publishing its `owner_waits` uplink (`pi_held_set_waits`
/// iterates only `held`), so transitive propagation THROUGH the overflowed lock is severed too — a
/// documented cap, not merely lost self-aggregation. Sleeping mutexes nest shallowly here, so 4 is
/// comfortably over the real maximum.
#[cfg(feature = "rtpi")]
const PI_HELD_MAX: usize = 4;

/// R1 / rtpi — the per-lock PRIORITY-INHERITANCE control block embedded in every sleeping `Mutex`.
/// NON-GENERIC so a donor can walk a chain of it across different `Mutex<T>` types, and — the whole
/// point — LONG-LIVED: it lives inside a `'static` lock, so a donor holding a `*const PiCtl` never
/// dangles, unlike a `*mut Task` (which `run()` frees on exit). This is what makes the walk
/// reclamation-safe.
#[cfg(feature = "rtpi")]
pub(crate) struct PiCtl {
    /// Current holder's `Task` pointer as an IDENTITY TOKEN only (0 = free). NEVER dereferenced by a
    /// donor — only compared for equality (`pi_bump_running` / `pi_relocate_scan`). Comparing a stale
    /// or reused value is a `u64` compare, so a freed holder is at worst a benign wrong-target boost
    /// that decays one level per requeue (not a one-step snap-back), never a use-after-free.
    owner: AtomicU64,
    /// Max effective priority of tasks currently blocked on this lock, or 0 for none. The DONATION
    /// lives here, on the long-lived lock. A holder's `sched_prio` folds in the `boost` of every lock
    /// it holds. Reset to 0 when the last waiter leaves (`nwait == 0`).
    boost: AtomicU8,
    /// If the current owner is itself blocked on another PI lock, that lock's `PiCtl` address (the
    /// TRANSITIVE uplink); else 0. Maintained by the owner on its own CPU (set on block, cleared on
    /// wake/release) across every lock it holds.
    owner_waits: AtomicU64,
    /// Count of tasks currently blocked on this lock. Incremented before a waiter parks, decremented
    /// when it wakes with the permit; when it reaches 0 the boost is reset (so a stale-high boost
    /// cannot outlive the contention that raised it — the leak witness `active` returns to 0).
    nwait: AtomicU32,
}

#[cfg(feature = "rtpi")]
impl PiCtl {
    const fn new() -> Self {
        PiCtl {
            owner: AtomicU64::new(0),
            boost: AtomicU8::new(0),
            owner_waits: AtomicU64::new(0),
            nwait: AtomicU32::new(0),
        }
    }
    #[inline]
    fn addr(&self) -> u64 {
        self as *const PiCtl as u64
    }
}

/// R1 / rtpi — the current task on this CPU as a raw pointer, or null off a scheduled context (the
/// BSP/idle path). MUST be called with IF=0 (review B1): only under a masked section is
/// `cpu_index` + `SCHED[cpu].current` guaranteed to name the CALLER — at IF=1 a timer preempt plus a
/// cross-CPU steal between the two reads makes a task identify as a DIFFERENT task (freed-Task deref
/// via `sched_prio`, wrong-task `held` writes, asymmetric `nwait` accounting). Mirrors the invariant
/// `Semaphore::wait` relies on (it too reads `current` only after `interrupts::disable()`). Every
/// caller wraps its whole per-task PI bookkeeping in `without_interrupts`, capturing `me` inside.
#[cfg(feature = "rtpi")]
#[inline]
fn pi_current() -> *mut Task {
    debug_assert!(!x86_64::instructions::interrupts::are_enabled());
    let cpu = percpu::this_cpu().cpu_index as usize;
    SCHED[cpu].current.load(Ordering::Acquire) as *mut Task
}

/// R1 / rtpi — add `ctl` to a task's held-lock set (first empty slot). Overflow is dropped: the lock
/// is still boosted globally, this task just under-aggregates it (the `PI_HELD_MAX` cap). Own-CPU.
#[cfg(feature = "rtpi")]
#[inline]
fn pi_held_add(me: *mut Task, ctl: u64) {
    let held = unsafe { &(*me).held };
    for slot in held.iter() {
        if slot
            .compare_exchange(0, ctl, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
    }
}

/// R1 / rtpi — remove `ctl` from a task's held-lock set (structural revert of the inheritance: the
/// task's `sched_prio` stops folding in that lock's boost). Own-CPU.
#[cfg(feature = "rtpi")]
#[inline]
fn pi_held_remove(me: *mut Task, ctl: u64) {
    let held = unsafe { &(*me).held };
    for slot in held.iter() {
        if slot
            .compare_exchange(ctl, 0, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
    }
}

/// R1 / rtpi — publish (or clear) the TRANSITIVE uplink on every lock this task holds: while the task
/// is blocked on `wait_ctl`, each lock it holds records "my owner waits on `wait_ctl`", so a donor
/// walking one of those locks follows the chain onward. `wait_ctl == 0` clears it on wake. Own-CPU.
#[cfg(feature = "rtpi")]
#[inline]
fn pi_held_set_waits(me: *mut Task, wait_ctl: u64) {
    let held = unsafe { &(*me).held };
    for slot in held.iter() {
        let ctl = slot.load(Ordering::Relaxed);
        if ctl != 0 {
            // SAFETY: `ctl` is a `PiCtl` in a `'static` lock this task holds — live for this write.
            unsafe { (*(ctl as *const PiCtl)).owner_waits.store(wait_ctl, Ordering::Release) };
        }
    }
}

/// R1 / rtpi — reset a lock's boost to 0 once no task is blocked on it (`nwait == 0`), keeping the
/// `active` leak gauge honest: a lock carries `boost > 0` only while it actually has blocked waiters.
#[cfg(feature = "rtpi")]
#[inline]
fn pi_boost_reset(ctl: &PiCtl) {
    if ctl.boost.swap(0, Ordering::AcqRel) > 0 {
        crate::rtpi::note_boost_end();
    }
}

/// R1 / rtpi — DONATE `prio` down a chain of lock control blocks, starting at `ctl_addr` (a
/// `*const PiCtl`). Raises each lock's `boost` to at least `prio`, ACCELERATES that lock's current
/// holder by IDENTITY (bump-if-running / relocate-if-ready — never dereferencing the holder), and —
/// if the holder is itself blocked on another PI lock — follows `owner_waits` and repeats
/// (transitive), bounded by `PI_CHAIN_MAX`. A no-op once a lock already carries `>= prio` (the boost
/// already propagated).
///
/// RECLAMATION SAFETY: every pointer dereferenced here is a `PiCtl` inside a `'static` lock, never a
/// `Task`. The holder is reached only as an identity token (`owner`, compared for equality by the two
/// helpers, never dereferenced). So no load->use window can straddle a `Task` free: the exact
/// interleaving that was UB before (donor loads holder, holder exits+frees, donor derefs) cannot
/// occur because the donor never holds or derefs a `Task` pointer. The caller's IF may be 1, so the
/// run-queue-lock scan masks interrupts (the WEDGE-4 `<W1>` hazard, as `pick_cpu` does).
#[cfg(feature = "rtpi")]
fn pi_donate(mut ctl_addr: u64, prio: u8, mut depth: u32) {
    while ctl_addr != 0 && depth <= PI_CHAIN_MAX {
        // SAFETY: `ctl_addr` is a `PiCtl` inside a `'static` lock — always live.
        let ctl = unsafe { &*(ctl_addr as *const PiCtl) };
        let old = ctl.boost.fetch_max(prio, Ordering::AcqRel);
        if prio <= old {
            return; // this lock already carries a boost >= prio — the chain past here is done.
        }
        if old == 0 {
            crate::rtpi::note_boost_begin(); // 0 -> boosted: the `active` gauge counts this lock.
        }
        crate::rtpi::note_inherit(old, prio, depth);

        // Accelerate the CURRENT holder (identity only — never a `Task` deref). Both helpers no-op if
        // the holder isn't in the state they handle, so calling both is safe and needs no state read.
        let owner = ctl.owner.load(Ordering::Acquire);
        if owner != 0 {
            pi_bump_running(owner, prio);
            pi_relocate_scan(owner, prio);
        }

        // Transitive: if the holder is itself blocked on another PI lock, follow the uplink. Reading
        // `owner_waits` is a `PiCtl` load (safe); it names another `'static` lock's `PiCtl`.
        let next = ctl.owner_waits.load(Ordering::Acquire);
        if next == 0 {
            return;
        }
        ctl_addr = next;
        depth += 1;
    }
}

/// R1 / rtpi — if `owner` (an identity token) is the task currently RUNNING on some core, raise that
/// core's published `current_prio` to `prio` so a remote mid-priority wake declines to preempt the
/// boosted holder. Found by IDENTITY (`SCHED[c].current == owner`) — `owner` is COMPARED, never
/// dereferenced, so a stale/reused value at worst matches nothing (no-op) or a wrong task (a benign
/// one-shot bump that its next dispatch corrects). Pure atomic ops; no lock, no mask.
#[cfg(feature = "rtpi")]
#[inline]
fn pi_bump_running(owner: u64, prio: u8) {
    for c in 0..MAX_CPUS {
        if SCHED[c].current.load(Ordering::Acquire) == owner {
            SCHED[c].current_prio.fetch_max(prio, Ordering::AcqRel);
            return;
        }
    }
}

/// R1 / rtpi — find `owner` (an identity token) in whatever run queue it sits in and relocate it UP
/// to `prio`'s level. Scans by IDENTITY under each queue's own lock, one at a time, inside
/// `without_interrupts` (the caller's IF may be 1; holding a run-queue lock while preempted would
/// wedge that core's `run()` at IF=0 — the WEDGE-4 `<W1>` hazard). The match derefs the QUEUE's own
/// live `Box`, never `owner` (compared by address); a stale/reused `owner` at worst relocates a wrong
/// ready task one level, which its next requeue (`sched_prio` from ITS own held set) corrects — never
/// a use-after-free, never a persistent boost. Not-found = the task raced out of the queue; its held
/// locks' boosts re-level it on its next enqueue.
#[cfg(feature = "rtpi")]
#[inline]
fn pi_relocate_scan(owner: u64, prio: u8) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        for c in 0..MAX_CPUS {
            if RUN_QUEUES[c].lock().pi_relocate(owner, prio as usize) {
                return;
            }
        }
    });
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
    /// R1 / rtpi: the per-lock priority-inheritance control block (owner identity, inherited `boost`,
    /// transitive uplink, blocked-waiter count). Lives inside this `'static` lock, so a donor holding
    /// its address never dangles — the whole reason the walk is reclamation-safe. See [`PiCtl`].
    #[cfg(feature = "rtpi")]
    pi: PiCtl,
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
            #[cfg(feature = "rtpi")]
            pi: PiCtl::new(),
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
    /// out an unbacked guard. (A sleeping mutex is meaningless off a scheduler context anyway — a
    /// boot-path caller that is not a task must not block.)
    #[cfg(not(feature = "rtpi"))]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        assert!(
            self.sem.wait(),
            "Mutex::lock() called off a scheduled task (a sleeping mutex needs a scheduler context)"
        );
        MutexGuard { mutex: self, _not_send: PhantomData }
    }

    /// R1 / rtpi — the priority-inheritance-aware acquire. Same contract and same guard as the
    /// knob-off `lock` above; the difference is entirely in what happens on CONTENTION:
    ///   * uncontested (`try_wait` takes the permit): record ownership, add this lock to our held set.
    ///   * contested (about to block): raise THIS lock's `boost` and donate down the holder chain
    ///     (`pi_donate`), publish the transitive uplink on the locks we hold, count ourselves in
    ///     `nwait`, then park in `sem.wait()`.
    ///   * on waking with the permit: uncount `nwait` (resetting `boost` if we were the last waiter),
    ///     clear the uplink, take ownership, and add this lock to our held set — from which our
    ///     `sched_prio` inherits whatever `boost` the remaining waiters still impose (handoff).
    /// The `sem.wait()`/permit semantics are IDENTICAL to the knob-off path; only the surrounding
    /// bookkeeping differs, so lost-wakeup safety is exactly as before.
    #[cfg(feature = "rtpi")]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        if self.sem.try_wait() {
            // Uncontested: took the permit without blocking. Record ownership; no inversion here.
            self.pi_acquire_uncontended();
            return MutexGuard { mutex: self, _not_send: PhantomData };
        }
        // Contested: we are going to block. Raise this lock's boost + donate down the holder chain
        // BEFORE parking so the holder out-ranks the mid-priority tasks and runs the section out.
        // `me` is captured under IF=0 inside `pi_on_block` and handed to `pi_acquire_after_block`,
        // so both sides of the park account the SAME task (review B1: symmetric `nwait`).
        let me = self.pi_on_block();
        assert!(
            self.sem.wait(),
            "Mutex::lock() called off a scheduled task (a sleeping mutex needs a scheduler context)"
        );
        // Woken with the permit: we are the new holder. Still the same task `me` — a park does not
        // change the caller's identity, only where/when it runs.
        self.pi_acquire_after_block(me);
        MutexGuard { mutex: self, _not_send: PhantomData }
    }

    /// R1 / rtpi — record ownership after an UNCONTESTED acquire (the `try_wait` fast path). Null
    /// `me` (an off-task boot-path caller taking a free permit — permitted, as in the knob-off path)
    /// just records the owner identity. The whole body runs masked (review B1): under IF=0 the
    /// `current` read names the caller, and the `held` write cannot hit a wrong task.
    ///
    /// The stale-`boost` reset is GATED on `nwait == 0` (review M2): a third task can snatch the
    /// permit here between a holder's `post` and a waiter's park — that waiter's donation is LIVE
    /// (it counted itself in `nwait` before donating), and unconditionally resetting would destroy
    /// it and reopen the inversion. Only a truly waiterless lock gets its residue cleared.
    #[cfg(feature = "rtpi")]
    #[inline]
    fn pi_acquire_uncontended(&self) {
        x86_64::instructions::interrupts::without_interrupts(|| {
            let me = pi_current();
            if self.pi.nwait.load(Ordering::Acquire) == 0 {
                pi_boost_reset(&self.pi); // no waiters ⇒ no inherited floor
            }
            self.pi.owner.store(me as u64, Ordering::Release);
            if !me.is_null() {
                pi_held_add(me, self.pi.addr());
            }
        });
    }

    /// R1 / rtpi — before parking on a contended lock: count ourselves in `nwait`, publish the
    /// transitive uplink on the locks WE hold (so a donor to us walks onward), then raise this lock's
    /// `boost` to our effective priority and donate down the holder chain. Returns the captured `me`
    /// (null for an off-task caller, which cannot block anyway — the `sem.wait()` assert catches it,
    /// and `pi_acquire_after_block` skips the same bookkeeping on the same null).
    ///
    /// The bookkeeping runs masked (review B1): `me` is captured at IF=0 (so it IS the caller) and
    /// the `nwait`/uplink writes happen as that task. The donated priority is computed AFTER
    /// `pi_held_set_waits` publishes the uplink (review M3): a donor that arrives in the
    /// snapshot→publish window stops its walk at us without seeing the uplink, so re-reading
    /// `sched_prio` after publication folds that donor's boost into what WE forward down the chain —
    /// the transitive donation is never lost.
    #[cfg(feature = "rtpi")]
    #[inline]
    fn pi_on_block(&self) -> *mut Task {
        let (me, p) = x86_64::instructions::interrupts::without_interrupts(|| {
            let me = pi_current();
            if me.is_null() {
                return (me, 0u8);
            }
            self.pi.nwait.fetch_add(1, Ordering::AcqRel);
            // Publish "I (a holder of these locks) am now blocked on `self.pi`" so a transitive
            // donor to one of the locks I hold follows the chain to the lock I wait on.
            pi_held_set_waits(me, self.pi.addr());
            // Recompute AFTER the uplink is visible (review M3), so a donation that landed on our
            // held locks before publication is folded into the priority we donate onward.
            (me, sched_prio(unsafe { &*me }))
        });
        if !me.is_null() {
            // Boost THIS lock and its holder chain. Outside the mask: `pi_donate` touches only
            // `'static` `PiCtl`s (never `me`) and masks its own run-queue scans.
            pi_donate(self.pi.addr(), p, 1);
        }
        me
    }

    /// R1 / rtpi — after waking with the permit: uncount `nwait` (resetting `boost` to 0 if we were
    /// the last blocked waiter, so a stale boost never outlives its contention), clear our transitive
    /// uplink, take ownership, and add this lock to our held set. Handoff inheritance is then
    /// AUTOMATIC: `sched_prio` folds in whatever `boost` the still-blocked waiters keep on this lock.
    ///
    /// `me` is the SAME pointer `pi_on_block` captured (review B1): the null check runs BEFORE the
    /// `nwait.fetch_sub`, so a caller that never incremented (null `me`) never decrements — no `u32`
    /// wrap, last-waiter detection stays exact. The per-task writes and the `current_prio` publish
    /// run masked with a FRESH `cpu_index` read, so the store hits the CPU we are actually on.
    #[cfg(feature = "rtpi")]
    #[inline]
    fn pi_acquire_after_block(&self, me: *mut Task) {
        if me.is_null() {
            // Never counted in `nwait` (pi_on_block skipped it on the same null) — do not decrement.
            self.pi.owner.store(0, Ordering::Release);
            return;
        }
        x86_64::instructions::interrupts::without_interrupts(|| {
            // We are no longer a blocked waiter. If we were the last, drop the lock's boost.
            if self.pi.nwait.fetch_sub(1, Ordering::AcqRel) <= 1 {
                pi_boost_reset(&self.pi);
            }
            unsafe {
                pi_held_set_waits(me, 0); // we no longer wait on anything — clear our uplink
                self.pi.owner.store(me as u64, Ordering::Release);
                pi_held_add(me, self.pi.addr());
                // Publish our (possibly inherited) effective priority so a mid wake declines to
                // preempt us. IF=0 ⇒ `cpu` is the core running `me` right now.
                let cpu = percpu::this_cpu().cpu_index as usize;
                if cpu < MAX_CPUS {
                    SCHED[cpu].current_prio.store(sched_prio(&*me), Ordering::Release);
                }
            }
        });
    }

    /// R1 / rtpi — the release-side bookkeeping, run just BEFORE the permit is posted (from the guard
    /// `Drop`, and from `Condvar::wait`, which `forget`s the guard and posts raw). Clears ownership
    /// and removes this lock from the holder's held set — a STRUCTURAL revert: the holder's
    /// `sched_prio` simply stops folding in this lock's `boost`, so there is no `Task` field to leak
    /// and no resurrection is possible.
    /// The whole body runs masked (review B1): `me` is captured at IF=0 so the `held` removal hits
    /// the RELEASING task (a preempt+steal between an unmasked `cpu_index` read and the `current`
    /// read would strip a held slot from — and republish the priority of — a different task), and the
    /// fresh `cpu_index` read inside the mask makes the `current_prio` store same-CPU by construction.
    #[cfg(feature = "rtpi")]
    #[inline]
    fn pi_release(&self) {
        x86_64::instructions::interrupts::without_interrupts(|| {
            let me = pi_current();
            self.pi.owner.store(0, Ordering::Release);
            if me.is_null() {
                return;
            }
            unsafe {
                pi_held_remove(me, self.pi.addr());
                // Republish our now-current effective priority (this lock's boost no longer counts).
                // IF=0 ⇒ `cpu` is the core running `me` right now, so this is a same-CPU write.
                let cpu = percpu::this_cpu().cpu_index as usize;
                if cpu < MAX_CPUS {
                    SCHED[cpu].current_prio.store(sched_prio(&*me), Ordering::Release);
                }
            }
        });
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
        // R1 / rtpi: revert any inherited priority (and clear ownership) BEFORE handing off the
        // permit, so the woken next holder observes a consistent lock. `#[cfg]`-gated (not a shim
        // call) so a knob-off build carries none of the PI symbols and stays bit-identical.
        #[cfg(feature = "rtpi")]
        self.mutex.pi_release();
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
            // R1 / rtpi: this `forget`+raw-post is the ONE mutex release that bypasses the guard
            // `Drop`, so the PI revert must run here too or the boost/ownership would leak across a
            // condvar wait. Atomics only (no UART), safe under the condvar lock. `#[cfg]`-gated so a
            // knob-off build stays bit-identical.
            #[cfg(feature = "rtpi")]
            mutex.pi_release();
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
    /// defining difference from `Semaphore::post`). May be called from any context (a task, an
    /// interrupt handler, or the boot path). `make_ready` is called only AFTER releasing the condvar
    /// lock, so the lock
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

    /// Reserve the underlying primitives' waiter capacity. Call once on the BSP, from the boot path,
    /// BEFORE any task can block on the channel — which since SCHED-X86 means before the BSP itself
    /// enters `run_bsp`. (Does not lock the buffer: `Mutex::lock` requires a scheduler context, and
    /// the boot path is not one.)
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

    /// Non-blocking `recv`: take a buffered value if one is already there, else `None`.
    ///
    /// SCHED-X86 DRAIN: this exists so a consumer can empty a burst before doing expensive
    /// per-batch work. The x86 render task presents ONE frame per drained burst, which is the
    /// semantic the dismantled BSP loop had — it drained every queued event and then presented
    /// once. Presenting per event instead is the exact regression main.rs documents as having
    /// bitten before ("at native resolution that flush is slow, so processing a single event per
    /// loop made input lag badly — the cursor never caught up; typed text appeared seconds late").
    /// A 2880x1800 present is ~50 ms; a fast typist or one trackpad sweep queues dozens of events.
    ///
    /// Unlike `recv` this never parks, so it is safe to call where blocking would be wrong — but it
    /// still takes the buffer mutex, so it must run on a scheduled task like the rest of the type.
    /// `try_wait` takes a permit only when it succeeds, so the `items`/`slots` accounting is
    /// identical to `recv`'s on the `Some` path and untouched on the `None` path.
    pub fn try_recv(&self) -> Option<T> {
        if !self.items.try_wait() {
            return None;
        }
        // Same invariant as `recv`: an `items` permit means a value was pushed before its `post`.
        let value = self.buffer.lock().pop_front().expect("channel buffer empty after items.try_wait");
        self.slots.post();
        Some(value)
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
/// No-op unless scheduling is active AND a task is actually running on this CPU — so it does nothing
/// during the pre-scheduler smoke test, nothing on a core still parked in `wait_and_run`, and nothing
/// on the BSP before it reaches `run_bsp` (after which core 0 is preempted like any other core).
pub fn timer_preempt() {
    if !SCHED_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    if raw.is_null() {
        return; // scheduler/idle context, or a CPU that is not dispatching yet
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

    // Preempt. We are already IF=0 (interrupt gate) and hold no lock — but the task we are about
    // to switch AWAY from may hold one. WEDGE-4 `<W1>`: if it is inside an unmasked run-queue
    // critical section, this switch is the wedge candidate's trigger, named on the wire.
    #[cfg(feature = "wedge2")]
    wedge4::note_preempt_in_rq(cpu);
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

/// SCHED-X86 — the BSP's entry into the scheduler, after it finishes its one-time boot duties
/// (framebuffer bring-up, PCI/xHCI publish, service-task spawn). Mirrors the APs'
/// `wait_and_run`→`run`, minus the `SCHED_GO` wait: the BSP is the core that SET `SCHED_GO` in
/// `enable()`, so scheduling is already live by the time it calls this. Never returns; it replaces
/// the inline GUI/shell loop the x86 BSP used to fall into at the end of `kernel_main`.
///
/// `cpu` is the caller's own logical index, taken as a parameter for signature parity with the
/// aarch64 twin and CHECKED against the per-CPU block rather than used — x86's `run()` derives the
/// index itself from `percpu::this_cpu()`, so a mismatch would mean the caller's model of which core
/// it is on is wrong, which is worth failing loudly on.
///
/// Why this is safe — the "the BSP is never scheduled" invariants, re-audited against the x86
/// evidence rather than inherited from the aarch64 doc:
///
///   * **GDT / TSS / IST / IDT / percpu / SYSCALL MSRs.** The BSP goes through the SAME `init_cpu`
///     an AP does: `arch::init()` calls `gdt::init()` (which is exactly `init_cpu(0)` — a full
///     per-CPU TSS with its four IST stacks), `interrupts::init_idt()`, `percpu::init_cpu(0, ..)`
///     and `syscall::init()`, all long before `kernel_main` reaches its GUI handoff. So the two
///     things `run()` writes per dispatch — `gdt::set_privilege_stack0(cpu, ktop)` and
///     `percpu::set_syscall_kernel_rsp(ktop)` — address a real TSS and a real per-CPU block on core
///     0, not the null state an unbrought-up core would have.
///
///   * **APIC routing.** There is nothing to re-route. x86 has no interrupt distributor to
///     reconfigure and no routed input line: keyboard/pointer arrive as USB HID through the xHCI
///     MSI-X path, not a per-core SPI. All local-APIC setup is per-core and already done (`apic::init`
///     on the BSP, and on each AP in `ap_entry`); the only globally-scoped acts are `apic::calibrate`
///     and `report_tick_rate`, both BSP-only and both complete before this call. The BSP running
///     scheduled tasks adds no interrupt-controller state at all.
///
///   * **The global ms-clock keeps advancing.** This is the one that would bite silently, so it was
///     checked by reading the handler rather than by analogy. `interrupts::timer_interrupt_handler`
///     runs, in this order: `percpu::note_tick()` (the per-core tick), then the `cpu_index == 0`
///     arm that does `APIC_TICKS.fetch_add(1, ..)` — core 0 is the SOLE advancer of the counter
///     behind `arch::ticks()` / `arch::ms()` — then `apic::eoi()`, and only THEN
///     `sched::timer_preempt()`. Both clocks are banked and the EOI is issued BEFORE any preemption
///     can switch core 0 away, so the system's wall clock advances whether or not a task is running
///     here. The residual hazard is NOT this function's: a COOPERATIVE ring-3 task (`spawn_user`,
///     whose `INITIAL_RFLAGS` carries IF=0) placed on core 0 would mask the timer for its lifetime
///     and freeze the global clock. Kernel tasks are safe (`task_trampoline` enables interrupts on
///     entry) and the live shell paths are safe (`run_user_image` / `spawn_user_image_bg` both use
///     `spawn_user_preemptible`, IF=1). The rule, written down: never place a COOPERATIVE ring-3
///     task on core 0.
///
///   * **Placement (SMPBAL-X86 — this bullet is the one the arc rewrote).** It used to read
///     "placement can never surprise us, because x86 tasks do not migrate": there was no `CPU_AUTO`
///     and no steal predicate, `make_ready` enqueued on `task.cpu` unconditionally, and core 0 ran
///     only what was explicitly pinned to it — which is to say it idled, and no imbalance anywhere
///     could ever heal. Both halves now exist. `run()` calls `mark_online(cpu)` before its first pop
///     (the x86 twin of the `mark_online` aarch64 does here), so from this call on core 0 is a
///     `CPU_AUTO` placement candidate and a steal thief/victim like any other.
///
///     The residual hazard named two bullets up is unchanged and is now ENCODED rather than merely
///     documented: a COOPERATIVE (IF=0) ring-3 task must never reach core 0, because it would mask
///     the timer for its lifetime and freeze the global ms-clock. `pick_cpu` excludes core 0 for that
///     task class at placement and `RunQueue::steal_one` excludes it again at migration — two sites,
///     because a steal is a placement decision `pick_cpu` never sees. Kernel tasks are safe
///     (`task_trampoline` enables interrupts on entry) and the live shell paths are safe
///     (`run_user_image` / `spawn_user_image_bg` both use `spawn_user_preemptible`, IF=1).
///
///   * **The `c0:0/0` meter reading heals, and that is a display change, not load moving.**
///     `CPU_BUSY[0]`/`CPU_IDLE[0]` are frozen at (0,0) today because core 0 never enters this loop;
///     from here it folds a busy or idle span every pass, so `meter_cpu_ticks(0)` starts reporting.
///     Do not read the meter coming alive as evidence that work was rebalanced onto core 0.
///
///   * **`sibling_online_cpu` changes behaviour, by design.** It probes `SCHED[c].scheduler_rsp != 0`
///     as "is this core dispatching". Core 0 publishes that word on its first `switch_context` from
///     this loop, so WINX-7 `SYS_THREAD_SPAWN` sibling placement may now answer 0. That is correct —
///     core 0 really is dispatching now — and it is the whole point: the same predicate is what
///     `bg_place_cpu` relies on being true of the caller.
pub fn run_bsp(cpu: usize) -> ! {
    let this = percpu::this_cpu().cpu_index as usize;
    assert_eq!(cpu, this, "run_bsp: caller named cpu {} but is running on cpu {}", cpu, this);
    // SCHED-X86 witness — the falsifiable moment. On the metal capture this line is what separates
    // "the BSP reached the handoff" from "the BSP is dispatching": everything after it is scheduler
    // work, and its absence with the spawn line present means the handoff block ran but `run()` was
    // never entered.
    serial_println!(":: SCHED-X86: BSP entered run loop cpu={} ::", cpu);
    // SCHEDLOAD-X86 ANTI-WITNESS, and the only instant on the whole boot at which it can be taken.
    // Core 0 has not yet folded a single span — it is one statement away from `run()` — while the APs
    // have been dispatching since the scheduler was enabled. So this line MUST read `c0=--` with at
    // least one other core carrying a percent. If it reads `c0=0%` the accounting is fabricating a
    // measurement for a core it has never measured, and every "idle core" this arc reports downstream
    // is worthless; if it reads `c0=--` and so does everything else, the APs are not being accounted
    // either. Cheap enough to be unconditional (one line, once per boot) and it is the assertion the
    // survey's Arc-0 anti-witness names.
    emit_load_witness("-prejoin");
    run()
}

/// The per-CPU scheduler/idle loop. Runs on the CPU's original stack, which becomes its
/// "scheduler context". Never returns. Pops a task, switches into it, and — when it switches back
/// (yield / preempt / exit) — requeues or frees it, then repeats; idles in an atomic `sti; hlt`
/// when the queue is empty.
/// VUGPAUSE-2/x86: milliseconds between two runs of the input-wait backstop. The global timebase is
/// 1 kHz (`apic::ticks()` is ms), so 256 is ~4 wake/poll/re-park cycles per second for an idle app.
/// Far below what a load meter can resolve, and far inside any liveness bound measured in polls: the
/// app keeps its old "I am still polling" contract with the watchdogs while costing effectively nothing.
const INPUT_WAIT_BACKSTOP_MS: u64 = 256;

/// VUGPAUSE-2/x86: the ms at which the next backstop pass is due. GLOBAL rather than per-CPU, and
/// claimed by CAS, so the cadence is ONE pass per period across the whole machine and not one per core
/// — six cores each waking every parked app would be six times the work for exactly the same effect.
static INPUT_WAIT_BACKSTOP_DUE: AtomicU64 = AtomicU64::new(0);

/// VUGPAUSE-2/x86: run the input-wait backstop if its period has elapsed. Called from the scheduler
/// loop top. A loser of the CAS simply skips; it does not spin or retry.
///
/// `apic::ticks()` advances from the BSP's timer IRQ, so this is a load of an atomic that barely moves
/// before the APIC heartbeat is armed. Nothing is lost there — nothing can be parked in
/// `SYS_INPUT_WAIT` before ring 3 exists.
#[inline]
fn input_wait_backstop() {
    let now = apic::ticks();
    let due = INPUT_WAIT_BACKSTOP_DUE.load(Ordering::Relaxed);
    if now < due {
        return;
    }
    if INPUT_WAIT_BACKSTOP_DUE
        .compare_exchange(
            due,
            now.wrapping_add(INPUT_WAIT_BACKSTOP_MS),
            Ordering::AcqRel,
            Ordering::Relaxed,
        )
        .is_err()
    {
        return;
    }
    crate::arch::x86_64::syscall::user_input_wake_backstop();
}

// ── SMPBAL-X86: WORK STEALING ───────────────────────────────────────────────────────────────────
//
// Placement (`CPU_AUTO`) is a one-shot guess made at spawn; this is the correction. An idle core —
// one whose own ready queue came up empty — pulls ONE steal-eligible task off the most-loaded
// dispatching core instead of going straight to `hlt`, so backlog drains onto idle silicon rather
// than queueing behind a saturated core.
//
// Protocol, race-free against the existing per-core run-queue spinlocks:
//   1. VICTIM SELECT (advisory peek): scan dispatching cores != self for the deepest queue at or
//      above `STEAL_MIN_DEPTH`, taking and releasing each core's lock one at a time.
//   2. STEAL, under the VICTIM's lock ONLY: re-read the depth (it may have drained since the peek)
//      and, if still at the floor, `steal_one()`. The lock is released before this core's own queue
//      is touched, so exactly ONE run-queue lock is held at a time — no lock ordering, no deadlock.
//   3. RE-HOME (we exclusively own the popped Box): `task.cpu = cpu`, then push locally. A later wake
//      through `make_ready` now delivers the task to its new home.
//
// WHY THIS IS SOUND ON x86, stated because it is the arch-specific half:
//   * There is no per-task hardware state to strand. VUGSPREAD/review F3 corrects the reason given
//     for the FP half, which was false as written ("no FPU/XSAVE context exists anywhere in this
//     kernel"): x87/MMX is ring-3-REACHABLE on this machine — CR0.EM/TS are 0 out of INIT and the
//     U2.5 first-entry scrub twenty lines into `user_task_trampoline` exists precisely because a
//     ring-3 program can leave live mantissa bits in the FP file. The correct statement is narrower
//     and survives that: **x87 is not per-TASK state in this scheduler at all.** Nothing here saves,
//     restores, or attributes the FP file to a task, migrated or not — a ring-3 task that dirties it
//     already loses that state to the next task on its OWN core, and a migration is not a new
//     hazard, only the same one on a different core. What makes the omission tolerable rather than a
//     latent bug is a CONVENTION, not an enforcement: the userspace targets are built `+soft-float`,
//     so no program in the tree keeps a value there across a preemption. **A future ring-3 program
//     compiled with hardware FP would need per-task save/restore — and would need it with or without
//     stealing.** The kernel's own `+soft-float` is what makes the scrub safe at CPL 0; it is not
//     what makes ring 3 safe. There is no FS base, and GS
//     base is pure per-core state — a migrated task reading the new core's per-CPU block is correct.
//   * Everything else per-task is re-derived at the single dispatch site: CR3, TSS.RSP0 and the
//     SYSCALL kernel rsp are all installed there from the incoming `Task`, covering first entry and
//     resume alike.
//   * The four `debug_assert!((*raw).cpu as usize == cpu)` invariants are PRESERVED, not exempted,
//     because step 3 re-homes before the push.
//   * `SLEEPERS` is NOT touched, deliberately. A sleep deadline is in the parking core's local APIC
//     tick domain and is not portable; a sleeping task is not in a run queue, so it is not reachable
//     from here. Do not "helpfully" migrate sleepers.
//   * The TLB obligation migration creates — x86 has no shootdown IPI and `slot_cr3(s)` is a fixed
//     address reused by every tenant of a slot — is discharged by the address-space generation, see
//     `AS_GEN` in `memory.rs` and the dispatch site below. That fix is not separable from this one.

/// SMPBAL-X86: minimum victim queue depth to steal from. `2` leaves the last ready task at its home
/// core (a core with one task is not "loaded"), which is what stops two idle cores ping-ponging a
/// lone task between them.
const STEAL_MIN_DEPTH: usize = 2;

// ── VUGSPREAD: THE FLOOR WAS COUNTING THE WRONG POPULATION ──────────────────────────────────────
//
// `STEAL_MIN_DEPTH`'s justification above — "a core with one task is not loaded" — is a true
// sentence attached to the wrong quantity. A run queue holds only READY tasks; the task a core is
// EXECUTING lives in `SCHED[cpu].current`, not in `levels`. So a floor of 2 ON THE QUEUE means a
// core needs THREE runnable tasks before an idle core will judge it loaded, and the packing this arc
// was opened on — a vug's parent and one of its workers time-sharing one core with two cores at 0 %
// — sits at queue depth ONE. It was not missed by the corrector; it was BELOW the corrector's floor
// by construction, which is why Boot AS reads `steal=1/4574483`: one migration in four and a half
// million idle passes, over ten minutes, on a visibly lopsided machine.
//
// The floor is therefore asked of the VICTIM rather than read off a constant:
//   * a victim that is RUNNING something carries that task PLUS its queue, so depth 1 already means
//     two runnable tasks and an idle core should take one;
//   * a victim at `PRIO_IDLE` is between tasks and is about to dispatch the very task we would take.
//     Stealing its only ready task is exactly how two idle cores start ping-ponging one task — the
//     hazard the constant was reaching for — so that case keeps the floor of 2, unchanged.
//
// This does not widen the steal in the direction that hurt aarch64. The rate is still bounded by the
// idle-pass rate at one task per pass, the thief still must have nothing of its own, and the pin
// contract is untouched. What changes is only WHICH victims are visible: cores that are genuinely
// running one task while another waits behind it.

/// VUGSPREAD: the ready-queue depth at which an idle core may take one of `victim`'s tasks. See the
/// block above.
///
/// Best-effort by design: `current_prio` may be a tick stale. Being wrong costs at most one extra
/// migration — the thief re-homes the Box it exclusively owns and the task runs there — and can
/// never lose or duplicate a task, because the decision is re-taken under the victim's own lock.
///
/// Called from the LOCK site. `try_steal`'s peek loop INLINES this rule rather than calling it, so
/// that the `current_prio` load it needs for the floor is the same one review F15's `PACK_SEEN`
/// observation reads — one load answering both questions instead of two loads that could disagree
/// with each other within a single iteration. Keep the two in step if either changes.
#[inline]
fn steal_floor(victim: usize) -> usize {
    if SCHED[victim].current_prio.load(Ordering::Acquire) == PRIO_IDLE {
        STEAL_MIN_DEPTH
    } else {
        1
    }
}

// ── VUGSPREAD: WHY A STEAL DID NOT HAPPEN ───────────────────────────────────────────────────────
//
// `steal=M/P` says how often the corrector ran and how rarely it moved anything. It does NOT say
// why, and on Boot AS the two readings that matter — "there was nothing to take" and "there was
// something to take and the rules forbade it" — are indistinguishable in it. They are opposite
// diagnoses: the first refutes the placement hypothesis outright, the second convicts a specific
// rule. So every declined attempt now names its reason, and the reasons are DISJOINT:
//
// | counter | meaning | which mechanism it fingerprints |
// | --- | --- | --- |
// | `t` [`STEAL_D_THIEF`] | the thief is the render or service core | neither — an expected refusal |
// | `e` [`STEAL_D_EMPTY`] | no other dispatching core held ANY ready task | the machine really was unpacked |
// | `f` [`STEAL_D_FLOOR`] | ready tasks existed, none reached its victim's floor | the floor (fixed above) |
// | `p` [`STEAL_D_PINNED`] | a victim qualified, every ready task on it was pinned, cooperative-excluded, or COOLING | the pin contract / VUGSPREAD-COOL |
// | `d` [`STEAL_D_DRAIN`] | the victim drained between the peek and the lock | benign raciness |
// | `i` [`STEAL_D_IDLEFLOOR`] | the victim held work but had gone IDLE, raising its own floor | the idle-floor ping-pong guard (Review F10) |
//
// VUGSPREAD-COOL folds into `p` on purpose: a task passed over only because it migrated within
// `STEAL_COOLDOWN_MS` leaves `steal_one` empty-handed exactly as a pinned one does, so the pass-level
// tally cannot separate them and the conservation law stays intact. The per-task SKIP is counted
// separately by [`STEAL_COOL_SKIP`] (`cool=` on `[spread]`), which is the reading that DOES distinguish
// "refused a re-steal" from "nothing was stealable" — and the term whose rise, against a flat `remig`,
// is this brake working.
//
// The arithmetic is checkable ON THE WIRE, which is what stops the set from quietly losing a case:
// `e + f + p + d + i + moves == passes` (`i` = `STEAL_D_IDLEFLOOR`, the Review-F10 idle-floor path,
// emitted as `i:` on the `decl=` line — see below), and `t` sits outside `passes` because
// `STEAL_PASSES` is bumped after the thief exclusion. A capture where those do not add up means a
// path was added without a counter, and the witness is lying rather than merely incomplete.
/// Review F10 — `d` was ONE counter covering two different events, which contradicted the
/// disjointness the table claims. A victim whose queue is EMPTY when the lock is taken drained under
/// a race (benign, and says nothing about policy); a victim holding `0 < len < floor` did NOT drain
/// — it went IDLE between the peek and the lock, raising its own floor from 1 to 2. The second is
/// **the new idle-floor guard firing**, i.e. this arc's own ping-pong brake doing its job, and
/// reading it as a race would hide exactly the mechanism a reviewer would want to audit.
static STEAL_D_THIEF: AtomicU64 = AtomicU64::new(0);
static STEAL_D_EMPTY: AtomicU64 = AtomicU64::new(0);
static STEAL_D_FLOOR: AtomicU64 = AtomicU64::new(0);
static STEAL_D_PINNED: AtomicU64 = AtomicU64::new(0);
/// The victim's queue was EMPTY under the lock — a true drain, a benign race.
static STEAL_D_DRAIN: AtomicU64 = AtomicU64::new(0);
/// The victim still held work but had gone IDLE, raising its floor to `STEAL_MIN_DEPTH`. The guard.
static STEAL_D_IDLEFLOOR: AtomicU64 = AtomicU64::new(0);

// ── VUGSPREAD: WHICH REPAIR DID THE WORK ────────────────────────────────────────────────────────
//
// Review F16. Three repairs landed together, and "the fleet spread out" attributes to none of them.
// Two of the three are separable by a counter, and both counters are read at the moment of a move —
// the only instant at which the question has an answer:
//
//   * `STEAL_M_DEPTH1` — moves taken from a victim whose ready queue held exactly ONE task. That is
//     precisely the population the old constant floor of 2 excluded, so this counts moves that could
//     not have happened before `steal_floor`.
//   * `STEAL_M_HINT` — moves of a task whose core came from a ring-3 placement hint
//     ([`Task::hint_placed`]). That is precisely the population the old pin contract froze, so this
//     counts moves that could not have happened before the `spawn_user_thread` change.
//
// They are independent, not exclusive: a vug worker packed behind its parent scores BOTH, which is
// the honest answer — that move needed both repairs and neither alone would have produced it. The
// third repair (`sibling_online_cpu`) is a PLACEMENT change and leaves no trace in the steal path at
// all; it is read off `[spread] rqp=` at launch (workers landing on distinct low-load cores) and off
// `SCHEDPLACE-X86`, never off these.
static STEAL_M_DEPTH1: AtomicU64 = AtomicU64::new(0);
static STEAL_M_HINT: AtomicU64 = AtomicU64::new(0);

/// VUGSPREAD (review F15) — idle passes on which at least one OTHER dispatching core was PACKED
/// (running a task with at least one more ready behind it).
///
/// The `[spread]` census samples every ~5 s from the render service. Packing that forms and clears
/// between two samples is invisible to it, so a refutation read off `pack=0` alone would be reading
/// a sampling artefact as a fact. This is the high-rate companion: it is evaluated on every steal
/// pass — millions per capture — inside the peek loop that already holds the depth, so it costs one
/// comparison and no extra lock. `packseen/passes` is a duty cycle, and a `pack=0` census standing
/// beside a `packseen` near zero is a REAL refutation, where `pack=0` alone is only a quiet sample.
static PACK_SEEN: AtomicU64 = AtomicU64::new(0);

/// VUGSPREAD (review F7) — dispatches that took `switch_cr3_if_needed`'s reload, i.e. installed a
/// DIFFERENT address space than the one this core was standing on. Distinct from [`CR3_RELOADS`],
/// which counts only the generation-behind revalidation arm.
///
/// This is the price tag on a migration, and it exists so the revert criterion in `scheduler.md` can
/// be a number instead of an adjective: on this hardware nothing kernel-mapped carries `PTE_GLOBAL`
/// and firmware leaves CR4.PGE clear, so every one of these is a WHOLE-TLB flush. Note it is NOT a
/// migration counter — an ordinary two-program alternation on one core takes this arm constantly —
/// so read its DELTA against the `steal=` delta over the same interval, never its absolute value.
static CR3_SWITCHES: AtomicU64 = AtomicU64::new(0);

/// VUGSPREAD: steals whose task had ALREADY migrated at least once (`Task::migrations > 0`). The
/// churn term. A balancing fleet drives `STEAL_MOVES` up a handful of times and leaves this at or
/// near zero; a thrashing one drives the two up together. Without it, "moves went up" is not
/// evidence of anything.
static STEAL_REMIGS: AtomicU64 = AtomicU64::new(0);

// ── VUGSPREAD-COOL: THE RUNNING-VICTIM PING-PONG BRAKE ──────────────────────────────────────────
//
// `steal_floor` lowered the RUNNING victim's floor to 1 so an idle core takes one task off a core
// that is running one and queueing another. That is the correct one-shot on an UNDER-loaded board
// (a vug parent time-sharing with idle cores). On a SATURATED board it is a churn engine, and Boot C
// measured exactly that: `remig=750397/750418` — 99.997% of steals were RE-migrations of the same
// handful of vugs — driving `cr3sw=14073209` whole-TLB flushes, while six cores already sat at 99%.
// Stealing cannot improve an all-busy board; every one of those moves was work thrown away, and it
// smeared each vug's compose across a rotating set of cores (the uneven `[wcpar]`/pulse reading).
//
// The mechanism the VUGSPREAD idle-floor guard does NOT reach: a vug BLOCKS briefly inside
// `SYS_WIN_PRESENT`, its home core's queue empties, that core steals a neighbour, the vug unblocks
// and re-queues home — now home is over-subscribed and a third idle core steals it back. The guard
// covers an IDLE victim; here the victim is RUNNING (floor 1) and the churn is driven by THIEVES
// going transiently idle. The brake is therefore per-TASK recency, not per-victim depth: a task
// stolen less than `STEAL_COOLDOWN_MS` ago is left where it is, so it runs at least a few quanta on
// its new home before another core may take it. The FIRST steal of any task is never delayed
// (`migrate_ms == 0`), so genuine spreading is untouched; only the immediate re-steal is refused,
// which is the ping-pong and nothing else. A settled fleet reaches one-vug-per-core and STAYS there,
// so a later vug CLOSE frees exactly one core with nothing left to steal — the survivors keep their
// homes instead of being re-grabbed, which is the close-spike's scheduler half.
//
// `STEAL_COOLDOWN_MS` is bench-tuned to a handful of scheduling quanta (the calibrated tick is 1 kHz,
// so this is ~16 quanta): long enough to break the ~0.5 ms re-steal cadence Boot C measured, short
// enough that a one-time post-close rebalance is delayed imperceptibly (and post-close there is
// usually nothing to steal at all). It is deliberately NOT a rate cap on the renderer — the vug still
// presents unbounded; only the SCHEDULER's re-placement of its task is damped.
const STEAL_COOLDOWN_MS: u64 = 16;

/// VUGSPREAD-COOL: per-task steal candidates SKIPPED because they migrated within `STEAL_COOLDOWN_MS`.
/// The brake's activity, and the direct counterweight to `STEAL_REMIGS`: a healthy post-fix capture
/// reads this CLIMBING while `remig` stays near flat — the ping-pong being refused rather than served.
/// A side counter like `STEAL_REMIGS`, outside the `e+f+p+d+i+moves == passes` invariant (a cooled
/// task that leaves `steal_one` empty-handed still lands on `p`/[`STEAL_D_PINNED`], whose doc now names
/// this case); this counts the per-task SKIPS inside the walk, which that pass-level tally cannot see.
static STEAL_COOL_SKIP: AtomicU64 = AtomicU64::new(0);

/// SMPBAL-X86: rate limit for the per-steal witness — the first `STEAL_LOG_MAX` migrations are named
/// on the wire, then it goes quiet. The cumulative `steal=` field on the `[schedx86] load` line keeps
/// counting after that, so the log stays bounded without the measurement stopping.
const STEAL_LOG_MAX: u32 = 24;
static STEAL_LOG_COUNT: AtomicU32 = AtomicU32::new(0);

/// SMPBAL-X86: total tasks MOVED by stealing, machine-wide.
static STEAL_MOVES: AtomicU64 = AtomicU64::new(0);

/// SMPBAL-X86: total idle passes that RAN the steal attempt, machine-wide. Reported alongside
/// `STEAL_MOVES` because the ratio is the falsifier: a steal count climbing at DISPATCH rate rather
/// than at idle-pass rate is churn, not balance (aarch64's own lesson, paid for over three arcs). A
/// fleet in balance shows moves ≪ passes and moves going flat while passes keep climbing.
static STEAL_PASSES: AtomicU64 = AtomicU64::new(0);

/// SMPBAL-X86: cumulative `(moves, passes)` for the load witness. Introspection only.
fn steal_counters() -> (u64, u64) {
    (STEAL_MOVES.load(Ordering::Relaxed), STEAL_PASSES.load(Ordering::Relaxed))
}

/// SMPBAL-X86: dispatches that took the UNCONDITIONAL CR3 reload arm — i.e. this core's `cr3_gen` was
/// behind `memory::as_gen()` and its TLB had to be re-validated. Bumped on the RARE arm only, so the
/// common no-mutation dispatch pays one relaxed load and nothing else.
///
/// This exists because the CR3-generation fix is otherwise INVISIBLE. It has no failure mode a
/// witness could catch after the fact — a stale-TLB cross-tenant read is silent by construction —
/// so the only falsifiable thing about it is whether it FIRES. Reported against the live generation:
/// `asgen=<gen>/<reloads>`. A generation climbing with window activity while reloads stays at 0 means
/// the dispatch site is not consulting it and the whole mechanism is dead.
static CR3_RELOADS: AtomicU64 = AtomicU64::new(0);

/// SMPBAL-X86: an idle `cpu` (its own ready queue came up empty) tries to steal ONE eligible task
/// from the most-loaded dispatching core. Returns `true` if a task landed on this core's queue — the
/// caller then loops back to the top of `run()` and dispatches it rather than halting.
///
/// Called from `run()`'s empty-queue arm with IF ALREADY 0 (the loop top masked), which is the
/// run-queue lock contract; no extra masking is needed and none is taken. At most one run-queue lock
/// is held at any instant.
///
/// NEITHER THE RENDER CORE NOR THE SERVICE CORE STEALS — the same two exclusions `pick_cpu` applies,
/// and they have to be repeated here because a steal is a placement decision `pick_cpu` never sees.
/// The service one is a DEADLOCK rule (`x86_usb_pump` holds the raw `XHCI_CONTROLLER` spinlock there;
/// a co-located preemptible task that also takes it can preempt the holder and spin forever — the
/// rule `smp::xhci_worker_cpu` DECLINES rather than break); the render one is the panel's latency
/// budget, which its idleness between frames represents rather than spare capacity. Both remain valid
/// VICTIMS: anything steal-eligible sitting on them is precisely what should be drained away. The
/// tier relaxation `pick_cpu` does has no analogue here and needs none — refusing to steal always
/// leaves the task where it already runs correctly.
fn try_steal(cpu: usize) -> bool {
    if crate::arch::smp::render_cpu() == Some(cpu) || crate::arch::smp::service_cpu() == Some(cpu) {
        STEAL_D_THIEF.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    STEAL_PASSES.fetch_add(1, Ordering::Relaxed);

    // 1. PEEK for the deepest OTHER dispatching queue at or above the floor. Each depth read takes
    //    that core's lock and releases it immediately — one at a time, never two at once, and the
    //    result is treated as advisory (step 2 re-checks under the lock it actually steals from).
    //    Say "peek", not "lock-free": these ARE acquisitions, and they go through the same bounded
    //    wrapper as everything else so the wedge probe can see them.
    //    VUGSPREAD: the floor is now per-victim (`steal_floor`), and `any_ready` remembers whether
    //    ANY other core held a ready task at all — that is what separates "nothing to take" from
    //    "something to take, below the floor", the two readings `steal=M/P` alone conflates.
    let mut victim: Option<usize> = None;
    let mut best_depth = 0usize;
    let mut any_ready = false;
    let mut saw_pack = false;
    for c in 0..MAX_CPUS {
        if c == cpu || !cpu_dispatching(c) {
            continue;
        }
        #[cfg(feature = "wedge2")]
        let depth = wedge4::lock_or_squawk(&RUN_QUEUES[c]).len();
        #[cfg(not(feature = "wedge2"))]
        let depth = RUN_QUEUES[c].lock().len();
        if depth == 0 {
            continue;
        }
        any_ready = true;
        // The two questions the floor asks, asked once: is this core RUNNING something (so its
        // queue depth understates its runnable count by one), and does its depth clear the floor
        // that answer implies. `running` doubles as review F15's high-rate packing observation —
        // running + at least one ready IS a packed core, seen here millions of times per capture
        // rather than at the census's ~5 s sample.
        let running = SCHED[c].current_prio.load(Ordering::Acquire) != PRIO_IDLE;
        if running {
            saw_pack = true;
        }
        let floor = if running { 1 } else { STEAL_MIN_DEPTH };
        if depth >= floor && depth > best_depth {
            best_depth = depth;
            victim = Some(c);
        }
    }
    if saw_pack {
        PACK_SEEN.fetch_add(1, Ordering::Relaxed);
    }
    let Some(v) = victim else {
        if any_ready { &STEAL_D_FLOOR } else { &STEAL_D_EMPTY }.fetch_add(1, Ordering::Relaxed);
        return false;
    };

    // 2. Steal under the victim's lock ONLY, re-checking the depth (it may have drained since the
    //    peek). The guard is scoped so it is dropped before this core's own queue is touched.
    //    VUGSPREAD: the floor is re-ASKED here, not carried from the peek — the victim may have gone
    //    idle in between, in which case the stricter idle floor is the one that should apply.
    //
    // Review F10: the short-read is a THREE-way answer, not a flag — `Some(true)` the queue was empty (a
    // true race), `Some(false)` it still held work but the victim had gone idle and raised its own
    // floor (this arc's ping-pong guard firing), `None` the queue cleared the floor and `steal_one`
    // itself declined. Collapsing the first two would file the guard's every firing as a race.
    let mut short: Option<bool> = None;
    // The victim's depth AS MEASURED UNDER ITS OWN LOCK — the peek's `best_depth` is advisory and
    // must not be the number attribution is scored on (review F16 wants moves the OLD floor would
    // have refused, and only the locked read can say whether this was one).
    let victim_depth: usize;
    // VUGSPREAD-COOL — one coherent `ms()` reading for both the cooldown test inside `steal_one` and
    // the migration stamp at step 3, so the task this steal takes is judged and stamped at one instant.
    let now_ms = crate::arch::ms();
    let stolen = {
        // WEDGE-4: this is a REMOTE run-queue acquisition, new on this arch, so it goes through the
        // same bounded-spin wrapper as the dispatcher's own — otherwise it would be the one
        // acquisition the wedge probe cannot see.
        #[cfg(feature = "wedge2")]
        let mut vq = wedge4::lock_or_squawk(&RUN_QUEUES[v]);
        #[cfg(not(feature = "wedge2"))]
        let mut vq = RUN_QUEUES[v].lock();
        let len = vq.len();
        victim_depth = len;
        if len < steal_floor(v) {
            short = Some(len == 0);
            None
        } else {
            vq.steal_one(cpu, now_ms)
        }
    };
    let Some(mut task) = stolen else {
        match short {
            Some(true) => &STEAL_D_DRAIN,
            Some(false) => &STEAL_D_IDLEFLOOR,
            None => &STEAL_D_PINNED,
        }
        .fetch_add(1, Ordering::Relaxed);
        return false;
    };

    // 3. Re-home and enqueue locally. We exclusively own the Box here, so this write races nothing.
    //    `name` is copied out BEFORE the push — the Box moves.
    let name = task.name;
    task.cpu = cpu as u32;
    // VUGSPREAD: the per-task migration count, bumped where the Box is exclusively owned. `m=1` on
    // every line is a fleet settling; the same name coming back with `m=2,3,4…` is churn, and that
    // is the reading the constant-floor change above has to be judged against.
    task.migrations = task.migrations.saturating_add(1);
    let mig = task.migrations;
    if mig > 1 {
        STEAL_REMIGS.fetch_add(1, Ordering::Relaxed);
    }
    // VUGSPREAD-COOL — stamp the migration so the next idle core's `steal_one` leaves this task on its
    // new home for `STEAL_COOLDOWN_MS`. Same `now_ms` the cooldown test above read, under the pop's
    // own lock where the Box is exclusively owned (the `migrations` discipline).
    task.migrate_ms = now_ms;
    // Review F16 — attribute the move to the repair(s) that made it possible. Both, or neither, or
    // one: a vug worker packed behind its parent scores both, and that is the honest answer.
    if victim_depth == 1 {
        STEAL_M_DEPTH1.fetch_add(1, Ordering::Relaxed); // the old constant floor would have refused
    }
    if task.hint_placed {
        STEAL_M_HINT.fetch_add(1, Ordering::Relaxed); // the old pin contract would have frozen it
    }
    #[cfg(feature = "wedge2")]
    wedge4::lock_or_squawk(&RUN_QUEUES[cpu]).push(task);
    #[cfg(not(feature = "wedge2"))]
    RUN_QUEUES[cpu].lock().push(task);
    STEAL_MOVES.fetch_add(1, Ordering::Relaxed);
    if STEAL_LOG_COUNT.fetch_add(1, Ordering::Relaxed) < STEAL_LOG_MAX {
        serial_println!(":: [smpbal] steal '{}' c{}->c{} (m={}) ::", name, v, cpu, mig);
    }
    true
}

fn run() -> ! {
    let cpu = percpu::this_cpu().cpu_index as usize;
    // SMPBAL-X86: register as DISPATCHING before the first pop, so `pick_cpu` and `try_steal` may use
    // this core — and, just as important, so neither can use a core that has not reached here.
    mark_online(cpu);
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

        // VUGPAUSE-2/x86: release every task parked in `SYS_INPUT_WAIT`, on a coarse cadence. Here for
        // the same structural reason as the sleeper drain above — it is a periodic re-ready pass, it
        // needs `make_ready`, and this is the one place in the kernel that runs forever on every core
        // with IRQs masked and no lock held.
        input_wait_backstop();

        // Age then pick, under ONE run-queue lock acquisition: run the anti-starvation sweep (gated
        // to ~every AGING_INTERVAL ticks) AFTER the sleeper drain (so freshly-woken sleepers, pushed
        // at base with a fresh clock, are visible to it) and BEFORE the pop, so a task cannot be
        // dispatched before it is aged in the same pass.
        let next = {
            // WEDGE-4 `<W2>`: this is THE acquisition the candidate mechanism wedges — IRQ-masked,
            // spinning on a lock whose holder this dispatcher itself preempted. Bounded, so the
            // wire names the stall instead of the core dying silently.
            #[cfg(feature = "wedge2")]
            let mut q = wedge4::lock_or_squawk(&RUN_QUEUES[cpu]);
            #[cfg(not(feature = "wedge2"))]
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
                // SCHEDLOAD-X86: the busy-TIME feed's dispatch-side bookkeeping, taken here — the same
                // site and the same instant as the event counter above, so the two instruments can
                // never disagree about WHETHER a dispatch happened while disagreeing (as they must)
                // about what it cost. `name`/`id` are read while `task` is still a `Box`, before it is
                // handed to `into_raw`.
                ACCT[cpu].ctx_switches.fetch_add(1, Ordering::Relaxed);
                ACCT[cpu].note_last(task.id, task.name);
                task.state.store(STATE_RUNNING, Ordering::Release);
                // Fresh quantum + clear the reschedule signal, and publish the running priority so a
                // remote waker/spawner can tell whether a newly-ready task should preempt us.
                SCHED[cpu].quantum.store(QUANTUM_TICKS, Ordering::Relaxed);
                SCHED[cpu].need_resched.store(false, Ordering::Relaxed);
                // R1 / rtpi: publish the EFFECTIVE priority so a remote waker cannot preempt a running
                // boosted holder with a mid-priority task. Knob-off arm is the pre-arc verbatim.
                #[cfg(feature = "rtpi")]
                SCHED[cpu].current_prio.store(sched_prio(&task), Ordering::Release);
                #[cfg(not(feature = "rtpi"))]
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
                //
                // SMPBAL-X86: and this is where the CR3 install stops being a pure optimization
                // question. `switch_cr3_if_needed` skips the reload when this core's LIVE CR3 already
                // equals the target, inferring "then my TLB is already correct for it". That
                // inference held only because tasks did not migrate: every core that ever installed a
                // given user CR3 was a core on which a task holding it would later exit and restore
                // the kernel CR3. Stealing falsifies it, and because `slot_cr3(s)` is a FIXED physical
                // address shared by every tenant of that slot over the SAME backing frames, the
                // failure is silent and cross-tenant — a new program dispatched on a core still
                // standing on the recycled root would run under the previous tenant's cached leaves
                // (stale W bits against the new ELF's W^X layout, plus reach into window-surface pages
                // that `clear_slot_fb` unmapped with a core-local `invlpg` only).
                //
                // So the skip is now conditional on this core having validated its TLB at the CURRENT
                // address-space generation. `AS_GEN` (`memory.rs`) is bumped by every user-leaf
                // mutation anywhere; a core whose stamp is behind reloads unconditionally — a full
                // non-global flush on this hardware, since nothing the kernel maps carries
                // `PTE_GLOBAL` and firmware leaves CR4.PGE clear — and restamps. One relaxed load per
                // dispatch in the steady state; at most one extra `mov cr3` per core per mutation.
                //
                // Note the ordering: `as_gen()` is read BEFORE the reload, so a mutation that lands
                // between the read and the reload leaves this core stamped one generation short and
                // it reloads again next dispatch. Erring stale-low is the safe direction; erring
                // stale-high would be the bug.
                let target_cr3 = {
                    let uc = unsafe { (*raw).user_cr3 };
                    if uc != 0 { uc } else { crate::arch::memory::kernel_cr3() }
                };
                //
                // VUGSPREAD adds one counter and no branch. `CR3_RELOADS` already counted the RARE
                // arm — the generation-behind revalidation — but the arm that a MIGRATION actually
                // pays was uncounted: a task arriving on a thief core is, by construction, arriving
                // on a core standing on some other root, so `switch_cr3_if_needed` takes its reload
                // and this machine flushes the entire TLB (nothing the kernel maps is `PTE_GLOBAL`
                // and firmware leaves CR4.PGE clear). Widening the steal without a price tag on the
                // move would leave the revert criterion unfalsifiable, so the switch is counted here
                // against `cr3_live`. The hardware compare inside `switch_cr3_if_needed` is still the
                // thing that DECIDES; this only decides whether a number goes up.
                let as_gen = crate::arch::memory::as_gen();
                if SCHED[cpu].cr3_gen.load(Ordering::Relaxed) == as_gen {
                    if SCHED[cpu].cr3_live.load(Ordering::Relaxed) != target_cr3 {
                        CR3_SWITCHES.fetch_add(1, Ordering::Relaxed);
                    }
                    unsafe { crate::arch::memory::switch_cr3_if_needed(target_cr3) };
                } else {
                    unsafe { crate::arch::memory::load_cr3(target_cr3) };
                    SCHED[cpu].cr3_gen.store(as_gen, Ordering::Relaxed);
                    CR3_RELOADS.fetch_add(1, Ordering::Relaxed); // rare arm only — see `CR3_RELOADS`
                }
                SCHED[cpu].cr3_live.store(target_cr3, Ordering::Relaxed);

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
                //
                // U4y: note there is NO user-rsp twin to install here, by construction. The SYSCALL
                // stub pushes the user rsp onto the task's own kernel stack rather than leaving it in
                // `PerCpuData`, so it rides `ctx_rsp` through the switch with everything else on that
                // stack and needs no re-install. It could not be re-installed from here in any case:
                // this site never sees the incoming task's ring-3 rsp. Do not "restore symmetry" by
                // adding a per-CPU user-rsp write — that shared slot is exactly the bug U4y removed.
                let ktop = {
                    let s = unsafe { &(*raw).stack };
                    ((s.as_ptr() as usize + s.len()) & !0xF) as u64
                };
                crate::arch::gdt::set_privilege_stack0(cpu, ktop);
                percpu::set_syscall_kernel_rsp(ktop);

                // SCHEDLOAD-X86: anchor the EXECUTION span. Taken as late as possible — the last
                // statement before the switch — so the CR3 / TSS.RSP0 / syscall-rsp installs above are
                // scheduler overhead and are charged to neither busy nor idle, which is the same
                // accounting boundary the aarch64 twin draws. One `rdtsc`, no memory traffic.
                //
                // R1/M3: the same reading is also PUBLISHED (`run_t0`), which is what lets this core
                // read its own in-flight span before the switch back closes it. Cleared by the fold
                // below, so while it is set the span it anchors is provably not yet banked.
                let busy_t0 = crate::arch::now_cycles();
                ACCT[cpu].run_t0.store(busy_t0, Ordering::Relaxed);
                unsafe {
                    switch_context(SCHED[cpu].scheduler_rsp.as_ptr(), entry_rsp);
                }
                // SCHEDLOAD-X86: ...and close it here, the first statement after the switch returns.
                // Both readings are taken on THIS core, so the subtraction is a sound elapsed time
                // even though `rdtsc` is per-core. `wrapping_sub` because the TSC is a free-running
                // 64-bit counter.
                //
                // The span INCLUDES any interrupt handler that fired while the task was running: that
                // time is the core being busy on this task's behalf. R1/M2 — but read that narrowly,
                // because it is only true WHILE A TASK IS RUNNING. The idle arm below charges the
                // waking handler to IDLE, so a core with an empty run queue that is saturated
                // servicing device IRQs reads 0%, not busy. That is a real blind spot in what this
                // instrument can see, not an arithmetic bug, and it is not hypothetical: core 0 is the
                // sole advancer of `APIC_TICKS`, so it carries a strictly larger ISR share than any AP
                // while having nothing pinned to it, and will report 0% on every line regardless.
                // Anything that balances against these numbers must know that ISR load is invisible
                // here. Documented in `scheduler.md` under the SCHEDLOAD-X86 limits.
                ACCT[cpu].account(crate::arch::now_cycles().wrapping_sub(busy_t0), 0);

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
                        let reap = task_kill_armed(&task);
                        if reap {
                            reap_killed(task);
                        } else {
                            // Rotate to the back of its (decayed) effective level: `requeue` steps the
                            // transient promotion down by one toward base rather than re-basing, so a
                            // task dispatched mid-climb re-climbs at most one level (the aging refinement).
                            task.state.store(STATE_READY, Ordering::Release);
                            // WEDGE-4 `<W2>`: same masked-context acquisition as the loop top.
                            #[cfg(feature = "wedge2")]
                            wedge4::lock_or_squawk(&RUN_QUEUES[cpu]).requeue(task);
                            #[cfg(not(feature = "wedge2"))]
                            RUN_QUEUES[cpu].lock().requeue(task);
                        }
                    }
                }
            }
            None => {
                // SMPBAL-X86: before halting, try to pull one task off the most-loaded core. This is
                // the exact slot the aarch64 twin uses, and the placement is deliberate: it runs only
                // when this core has genuinely nothing of its own, so the steal rate is bounded by the
                // idle-pass rate and one steal is taken per pass at most. A success falls through to
                // the loop top (which re-pops under the lock) rather than dispatching inline, so the
                // stolen task goes through the ordinary aging + pick path with no second code path.
                // IF is 0 here — masked at the loop top and not yet re-enabled by the `hlt` below.
                if try_steal(cpu) {
                    continue;
                }
                // Nothing to run: sleep until an interrupt (timer or a `spawn`/wake IPI). The `sti;
                // hlt` pair is atomic — an interrupt latched before the `sti` still fires and
                // returns past the `hlt`, so a wake that arrived in the empty-check window is not
                // lost. On wake we loop back to the top (which `cli`s) and re-check the queue +
                // sleepers.
                CPU_IDLE[cpu].fetch_add(1, Ordering::Relaxed); // M3b CPU-pulse meter (introspection)
                // SCHEDLOAD-X86: the IDLE span, measured the same way and on the same core. It runs
                // from just before the `sti; hlt` to just after the waking interrupt returns past it,
                // so it counts the handler that woke us as idle time. That is a deliberate and bounded
                // over-count of idleness — one interrupt's worth per idle pass, microseconds against a
                // ~1 ms tick — and it errs in the safe direction: this instrument can under-report a
                // core's load, never invent load that is not there.
                let idle_t0 = crate::arch::now_cycles();
                x86_64::instructions::interrupts::enable_and_hlt();
                ACCT[cpu].account(0, crate::arch::now_cycles().wrapping_sub(idle_t0));
            }
        }
    }
}

/// U3.5/TEARDOWN-1: RETIRE a task whose `KillSwitch` is armed, from the scheduler's own context.
///
/// Owns `task` and drops it, so the task never executes again. Extracted from `run()`'s READY arm
/// because TEARDOWN-1 gives it a second caller — the PARK_SLEEP arm of `park_blocked`, where a task
/// that would otherwise commit itself to a sleeper list is retired instead. Both callers are the
/// scheduler context with IF=0, holding no scheduler, wait-queue or heap lock, which is what makes the
/// teardown below (a `mov cr3`, a slot free, a heap drop) legal here.
///
/// Teardown MIRRORS `exit`: restore the kernel CR3 first (this task ran under its own CR3, which is
/// still live on this core; that `mov cr3` full-flush retires the slot's user TLB entries) BEFORE the
/// slot is freed, so no core is left standing on a dead or reused root.
fn reap_killed(task: Box<Task>) {
    // WINX-7: post the completion here too. A REAPED task never runs `exit()` — the scheduler drops
    // it where it stands — so without this a joiner parked on a killed thread would wait forever,
    // which is the one way a `kill` could leave a process MORE stuck than before it was killed.
    if let Some(sem) = &task.done_sem {
        sem.post();
    }
    // WINX-7: refcounted, for the reason `exit()` documents — a killed THREAD must not tear the
    // address space out from under its siblings. (A killed PROCESS is the common case and still frees
    // here: its refcount is 0 unless it spawned threads.)
    if task.user_cr3 != 0 {
        // TEARDOWN-1: DOOM the address space before dropping our hold on it, so every sibling ring-3 thread
        // follows this task out at its own next kill boundary and the refcount can actually reach zero.
        // Armed here rather than at the requester — a `bg_kill` names a pid, and the set of tasks that
        // pid's program owns is knowable only from the slot. Idempotent, so a sibling reaping after us
        // simply re-arms the same flag; the flag is cleared on the slot's real free edge.
        doom_address_space(task.user_cr3);
        crate::arch::memory::restore_kernel_cr3();
        // VUGSPREAD: keep the `cr3_live` shadow honest across a teardown restore — see `SchedCpu::cr3_live`.
        SCHED[percpu::this_cpu().cpu_index as usize]
            .cr3_live
            .store(crate::arch::memory::kernel_cr3(), Ordering::Relaxed);
        user_space_release(task.user_cr3);
    }
    if let Some(k) = &task.kill {
        k.mark_reaped(); // through the Arc — the requester's clone keeps it live
    }
    drop(task); // frees the kstack; the interrupt frame on it is abandoned
    // TEARDOWN-1: reach the siblings the doom above just named. A worker parked on the frame barrier its
    // now-dead parent will never release is the whole point: it is woken here, through the futex's own
    // wake path, and retires at its next syscall return. Called AFTER the drop so no Box and no lock is
    // held; legal in this context for the same reason `park_blocked`/`drain_due_sleepers` are — the
    // scheduler context takes the sleeper/bucket locks and re-readies outside them, in that order.
    let _ = kill_wake_parked();
}

/// TEARDOWN-1: is the task running on THIS core carrying an ARMED kill? Two atomic loads on the fast
/// path (the `current` pointer and the flag), and nothing at all for a task with no kill switch — cheap
/// enough for the syscall return path, which is where [`kill_check_current`] calls it.
fn current_kill_requested() -> bool {
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *const Task;
    if raw.is_null() {
        return false;
    }
    // SAFETY: `current` is owned by `run()` on this CPU and we are on that CPU with IF=0 (every caller
    // is a syscall/interrupt-masked context), so the Box cannot be reclaimed under us. We only read.
    unsafe { task_kill_armed(&*raw) }
}

/// TEARDOWN-1: the SYSCALL kill boundary. If the task running on this core has been killed, retire it
/// here — this call does not return in that case.
///
/// Retirement is expressed as `yield_now()`, deliberately, rather than as a second copy of the teardown:
/// the task switches back to the scheduler in STATE_READY, and the scheduler's existing reap arm — the
/// one U3.5 wrote and WINX-7 fixed — sees the armed switch and runs [`reap_killed`]. So the completion
/// post, the refcounted address-space release, the `mark_reaped` handshake and the kernel-stack free all
/// stay in exactly ONE place, and a killed task's teardown is byte-identical whether it was delivered at
/// a preemption or at a syscall.
///
/// Callers must be at a point where switching away FOR GOOD is safe: on the task's own kernel stack,
/// holding no lock, with every guard dropped. The syscall dispatcher's tail qualifies — it already
/// documents that a blocking/exiting syscall may `switch_context` from there.
pub fn kill_check_current() {
    if !current_kill_requested() {
        return;
    }
    yield_now(); // -> scheduler -> reap_killed; never comes back
}

/// TEARDOWN-1: sweep the kernel waits a ring-3 task can be parked in and EVICT the ones an armed kill
/// names. Called from [`KillSwitch::request`] once the arming store is published, so the predicate
/// below (the task's own `kill` flag) already matches. Returns how many tasks were evicted.
///
/// THE SET IS ENUMERABLE, and that is the whole reason this is a finite function rather than a hope.
/// Every park on this arch routes through one of the three `PARK_*` actions, and only two of them can
/// hold a ring-3 task indefinitely:
///   * PARK_SLEEP — the per-CPU `SLEEPERS` lists (`SYS_SLEEP_MS`). Swept here.
///   * PARK_WAITQ on a futex bucket (`SYS_FUTEX`). Swept here. This is the one that could park
///     FOREVER — a barrier whose other side is gone is woken by nobody — and so the one that made a
///     kill unconfirmable rather than merely late.
///   * PARK_WAITQ on a `Semaphore` the syscall layer owns (`SYS_THREAD_JOIN`'s join handle,
///     `SYS_WAIT`'s `Proc::done`). NOT swept: those tables live in `syscall`, not here, and there is no
///     registry of live semaphores to walk. The honest bound this leaves is stated at the bottom of
///     this doc rather than papered over.
///
/// Each half evicts through the park kind's OWN wake path — `make_ready`, called outside the wait
/// structure's lock, which is precisely what `drain_due_sleepers` and `futex_wake` do. So the lock order
/// is unchanged (the run-queue lock is never nested under a sleeper or bucket lock), the woken task
/// resumes inside its own `sleep_ticks`/`futex_wait` exactly as a legitimate wake would leave it, and
/// nothing reaches around a park to mutate scheduler state behind its back. A futex waiter evicted this
/// way returns `FutexWait::Woken` and its `SYS_FUTEX` returns 0 — a spurious wake, which is a legal
/// futex outcome the ring-3 loop must already tolerate, and it does not matter here anyway: the task
/// retires at the syscall boundary on the way out.
///
/// THE REMAINING BOUND, stated: a killed task already parked on a kernel `Semaphore` is not evicted. It
/// retires at the syscall boundary when that semaphore is posted, so it is LATE, not immortal — and both
/// ring-3 semaphore parks wait on something that is itself killable (a thread, a child process), so the
/// post is reachable. Closing it properly needs a `kill_wake_parked_semaphores` hook in `syscall` (the
/// shape aarch64 uses); that is a separate arc, not a silent gap.
fn kill_wake_parked() -> u32 {
    let was_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();
    let mut evicted = 0u32;

    // (a) PARK_SLEEP — the per-CPU sleeper lists. Same lock-then-release-then-`make_ready` shape as
    // `drain_due_sleepers`, one victim per pass so the lock is never held across the re-ready.
    for cpu in 0..MAX_CPUS {
        loop {
            let doomed = {
                let mut sleepers = SLEEPERS[cpu].lock();
                match sleepers
                    .iter()
                    .position(|s| task_kill_armed(&s.task))
                {
                    Some(i) => sleepers.remove(i).map(|s| s.task),
                    None => None,
                }
            }; // sleeper lock dropped here
            match doomed {
                Some(task) => {
                    make_ready(task);
                    evicted += 1;
                }
                None => break,
            }
        }
    }

    // (b) PARK_WAITQ on a futex bucket. The bucket lock is the SAME lock `futex_wake` takes, held
    // across the removal only; the key is released when the removal drains the bucket, exactly as a
    // wake would — otherwise a bucket emptied here would stay claimed for a key with no waiters.
    for b in FUTEX.iter() {
        loop {
            b.lock_raw();
            let doomed = unsafe {
                let w = &mut *b.waiters.get();
                match w.iter().position(|t| task_kill_armed(t)) {
                    Some(i) => w.remove(i),
                    None => None,
                }
            };
            if doomed.is_some() && b.waiters_empty() {
                b.key.store(0, Ordering::Relaxed);
            }
            b.unlock_raw();
            match doomed {
                Some(task) => {
                    make_ready(task);
                    evicted += 1;
                }
                None => break,
            }
        }
    }

    if was_enabled {
        x86_64::instructions::interrupts::enable();
    }
    evicted
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
            // TEARDOWN-1: the "arm, THEN park" half of the sleep leg, closed at the PUBLISH point and
            // UNDER the publish lock. The test and the `push_back` share one continuous hold of
            // `SLEEPERS[cpu]`, which is the same lock `kill_wake_parked`'s sweep takes — so either the
            // sweep runs before this hold (and its arming store, which precedes it, is what this test
            // reads) or after the push (and it finds the sleeper). There is no third ordering, which is
            // why this is lossless rather than merely narrow. The lock is dropped before `reap_killed`,
            // which frees a slot and touches the heap.
            let mut sleepers = SLEEPERS[cpu].lock();
            if task_kill_armed(&task) {
                drop(sleepers);
                reap_killed(task);
                return;
            }
            sleepers.push_back(Sleeper { deadline, task });
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
    // WINX-7: reserve every futex bucket's waiter capacity here, on the BSP, before any AP can reach
    // `run()` and therefore before any task can park on one. Doing it at the first `futex_wait`
    // instead would put a `VecDeque` growth (and the heap lock) inside the handed-off bucket lock,
    // which is exactly the allocation the park-side push is proven not to perform.
    futex_init();
}

/// Number of tasks currently queued on a CPU (best-effort snapshot; for the `sched` shell command).
pub fn run_queue_len(cpu: usize) -> usize {
    if cpu >= MAX_CPUS {
        return 0;
    }
    // WEDGE-4 `<W1>` window: the shell's status readout and the sched selftest call this from task
    // context (IF possibly 1) — the same unmasked-acquisition class as the spawn paths, found by the
    // pi seat's seven-site sweep (their two extras were also fixture/witness acquisitions).
    #[cfg(feature = "wedge2")]
    let w4cpu = percpu::this_cpu().cpu_index as usize;
    #[cfg(feature = "wedge2")]
    wedge4::enter(w4cpu);
    let n = RUN_QUEUES[cpu].lock().len();
    #[cfg(feature = "wedge2")]
    wedge4::leave(w4cpu);
    n
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
/// (the scheduler-reaper context — `current` is `0` there — or a core not yet dispatching). STOR-1 S4c uses
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
