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
//     AFTER EOI). Preemption from an interrupt requires the IRQ stub to bank ELR/SPSR — and, once EL0
//     is preemptible (M6e), SP_EL0 — (exceptions.rs); those are per-core system registers, not a
//     per-context stacked frame.
//     Like the SGI IPIs, timer delivery is metal-only (QEMU won't deliver it), so on QEMU the APs
//     run their demo tasks to completion sequentially and on the Pi they interleave (preemption).

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{
    AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering,
};
// spin's Mutex is the low-level SPINLOCK guarding the run queues / sleeper lists; alias it so this
// module's own sleeping `Mutex<T>` (below) owns the bare name — same split as x86's sched.rs.
use spin::Mutex as SpinMutex;

use super::percpu::{self, NUM_CPUS};
use super::timer;

/// Per-thread kernel stack. 16 KiB matches the x86 scheduler.
const TASK_STACK_SIZE: usize = 16 * 1024;

/// Timer ticks a task runs before preemption (~4 ms/tick × 3 = 12 ms quantum).
const QUANTUM_TICKS: u32 = 3;

/// AARCH64-PRIO — fixed-priority multilevel run queues. A CPU always runs a ready task of the HIGHEST
/// non-empty level; within a level scheduling is round-robin (FIFO). Higher number = more urgent.
/// This mirrors the proven x86 design (`arch/x86_64/sched.rs`), adapted to this module's structures.
pub const NUM_PRIORITIES: usize = 4;
/// Convenience priority levels (any `0..NUM_PRIORITIES` is valid; out-of-range is clamped by `push`).
pub const PRIO_LOW: u8 = 0;
/// The DEFAULT level every existing caller lands at (`spawn`/`spawn_user`/`spawn_joinable`) — a single
/// level, so those paths stay byte-identical to the pre-priority flat round-robin.
pub const PRIO_NORMAL: u8 = 1;
pub const PRIO_HIGH: u8 = 2;
pub const PRIO_RT: u8 = 3;

/// SCHED-PRIO — the INTERACTIVE SERVICE BAND: the level the panel's own latency-critical tasks run
/// at, one step above the `PRIO_NORMAL` every EL0 program (and every ordinary kernel worker) lands
/// at. An alias for [`PRIO_HIGH`], named for its meaning so the spawn sites read as policy rather
/// than as a magic number, and so this band and the storage service (`irqstorage`, already
/// `PRIO_HIGH`) can be told apart in the source even though they share a level today.
///
/// ### What the band is for (P73/P75)
/// The compositor pass owner (`main.rs::render_service` — `Screen::flush` → `wm::service_damage`,
/// plus the cursor bracket), the input router (`input_service`) and the HID report pump (`usb_pump`)
/// were ORDINARY `PRIO_NORMAL` tasks, i.e. exact round-robin peers of six EL0 vug render fleets.
/// Under fleet load the triage measured composites collapsing 0.99→0.43/s and WM-lock erase defers
/// going 29%→76%: the panel's own service work was queued behind every vug that happened to be ready.
/// The band makes those three win a contested dispatch.
///
/// ### Why this is not real time, and cannot starve EL0
///   * The band is ONE level up, not the top; nothing here is `PRIO_RT`.
///   * The anti-starvation sweep is untouched. An EL0 task that waits `AGE_TICKS` dispatch passes is
///     RELOCATED into this very band and then shares it round-robin, so the worst case for a vug is
///     bounded exactly as before — the band buys ordering, not exclusion. `[prio] agedin=` counts
///     precisely those promotions, so the escape valve is visible rather than assumed.
///   * All three tasks BLOCK for a living (`GUI_CHANNEL.recv`, `RX_READY.wait`, `sleep_ticks`). None
///     of them can hold a core: they are off the run queue entirely whenever there is no work, which
///     is what makes "highest ready level" cheap to grant.
///   * The EL0 task a wake preempts loses at most the REMAINDER of its quantum, never a whole one —
///     see `preempt_hint`, which trims to one tick rather than switching anything out by force.
///
/// ### Priority inversion
/// The band's paths take three locks, and none of them can invert unboundedly:
///   * `wm::STAGE` and `cursor::OVERLAY` are taken with `try_lock` on every path a service task
///     reaches (`stage_window`, `stage_rows`, `compose_into`, the `DEFER` drain probe) — a service
///     task never waits on a lock an EL0 present holds; it declines and falls back.
///   * `wm::TABLE` is a blocking `lock()`, but every holder takes NO further scheduler-visible lock
///     while holding it (the compositor snapshots rows and releases before any framebuffer write),
///     so a hold is a bounded straight-line section, and an EL0 holder that is descheduled mid-hold
///     is aged into this same band within `AGE_TICKS` and runs it out.
///   * `RqGuard` sections are IRQ-masked and O(ready tasks) at worst, so no run-queue section is
///     ever preemptible at all.
pub const PRIO_SERVICE: u8 = PRIO_HIGH;

/// AARCH64-PRIO — anti-starvation aging. A ready task that has WAITED in a run queue this many aging
/// units without being dispatched is RELOCATED one effective level UP (its BASE `priority` is
/// unchanged); repeated, a low task under continuous higher-priority load climbs to parity, runs, then
/// re-bases on its next enqueue — bounding starvation to ~`AGE_TICKS` per level it must climb.
///
/// UNIT NOTE (the aarch64 adaptation): x86 measures the wait in its always-live LVT `percpu.ticks`.
/// The aarch64 cooperative dispatch paths (BSP `demo_cooperative`, the `virt` secondaries, the `virt`
/// CAPSTONE driver, QEMU raspi4b) have NO live periodic tick — QEMU delivers no Group-1 timer IRQ, so
/// `percpu.ticks` is frozen at 0 there. So this module ages by SCHEDULER ACTIVITY — one unit per
/// `dispatch_next` pass on the owning CPU (see `SchedCpu::age_passes`) — which advances on EVERY path
/// (cooperative and preemptive, QEMU and metal). A pass IS the starvation measure: it counts every
/// time this core dispatched SOMEONE ELSE while a waiter sat. Every other aging invariant (owning-CPU-
/// only `wait_ticks`, ENQUEUE zeroes, RELOCATE promotes via a raw HIGH→LOW move, the sweep under the
/// same run-queue lock as the pop, promoted-then-dispatched runs at base) matches x86 exactly.
const AGE_TICKS: u32 = 16;
/// How often the aging sweep runs, in dispatch passes. Kept well below `AGE_TICKS` so the
/// one-promotion-per-sweep cap never binds; a sweep accrues elapsed credit and carries any surplus
/// past `AGE_TICKS` to the next sweep, so a coarse/late sweep loses no credit.
const AGING_INTERVAL: u64 = 4;

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

/// SCHED-NEXT (virt busy-heartbeat): the `virt` BSP sets this ONCE, BEFORE `CPU_ON`, to declare "I
/// will stage cooperative work — secondaries, wait for my release." It is the clean discriminator
/// between the paths that share `__secondary_rust_virt`: only `virt` (`start_secondaries`) arms it;
/// the real Orin (`start_secondaries_tegra`) and the SMP-probe legs stage no work and never arm, so
/// their secondaries skip the wait entirely and park as before (no added latency, and — the point —
/// no hang). Set before `CPU_ON` so every secondary that comes online is guaranteed to observe it.
static SECWORK_ARMED: AtomicBool = AtomicBool::new(false);
/// Released by the BSP once every secondary's run queue is staged. An armed secondary waits on this
/// (with a GENEROUS finite backstop, NOT a tight timing ceiling — the fragile 20 ms ceiling flaked
/// under host load) so it drains REAL work; a non-armed secondary never reads it. The `virt`
/// secondaries run at EL2 and never enter the full `run()` loop (no per-core timer — see the module
/// header + `smp_virt`); this flag releases a single cooperative pass (`run_secondary_work`) so an
/// online core publishes honest BUSY telemetry — the other half of the idle-heartbeat.
static SECWORK_GO: AtomicBool = AtomicBool::new(false);
/// Per-core "cooperative pass drained" flag, set by each secondary after `run_secondary_work` empties
/// its queue; the BSP waits on it before reading the busy-heartbeat witness so it never races an
/// un-run core. Introspection/handshake only — never read on a scheduling path.
static SECWORK_DONE: [AtomicBool; NUM_CPUS] = [const { AtomicBool::new(false) }; NUM_CPUS];

/// Monotonic task-id source.
static NEXT_TID: AtomicU64 = AtomicU64::new(1);

/// A kernel thread. Owned as `Box<Task>`: it lives in exactly one place — a run queue, or "running"
/// (the Box leaked to a raw pointer in `SchedCpu::current`).
pub struct Task {
    id: u64, // read by `current_id` (M6f SYS_GETPID/GETINFO) + join handles
    name: &'static str, // read by `current_name` (the M6b EL0 fault-kill log)
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
    /// AARCH64-PRIO — BASE scheduling priority (`0..NUM_PRIORITIES`, higher = more urgent). IMMUTABLE
    /// after spawn, so it is safe to read lock-free from any CPU. Aging never touches this — it only
    /// transiently raises the *level* a task sits in (see `wait_ticks`); a promoted task re-bases here
    /// on its next enqueue.
    priority: u8,
    /// AARCH64-PRIO — aging units this task has WAITED in a run queue since its last enqueue. Touched
    /// ONLY under the owning CPU's run-queue spinlock (zeroed by `RunQueue::push` on every enqueue,
    /// accrued + consumed by `RunQueue::age` on that CPU). NEVER read cross-CPU — unlike `priority` it
    /// is mutable and lock-protected, so it must not be read off the owning CPU.
    wait_ticks: u32,
    /// Completion signal for `join()`, or `None` for a fire-and-forget task. A joinable task carries
    /// a clone of the same `Arc<Semaphore>` (0 permits) held by its `JoinHandle`; the trampoline
    /// `post()`s it after `entry` returns. The Arc — not a `'static` lifetime — keeps the semaphore
    /// alive across the park/post window (see `JoinHandle`). Read (never moved) by the trampoline
    /// through the raw `current` pointer; dropped when the scheduler drops the Box on the Finished path.
    done_sem: Option<Arc<Semaphore>>,
    /// EL0 user-mode entry point + initial SP_EL0 (M6a). Non-zero only for `is_user` tasks made via
    /// `spawn_user`: such a task's initial frame lands in `user_task_trampoline`, which `eret`s to EL0
    /// at `user_entry` with SP_EL0 = `user_sp`. 0 / unused for ordinary kernel tasks. Read only by
    /// `user_task_trampoline` (baremetal-only); on the `virt` build every task is a kernel thread, so
    /// these stay 0 and unread (`spawn_inner` still initialises them for the shared struct layout).
    #[cfg_attr(not(feature = "baremetal"), allow(dead_code))]
    user_entry: u64,
    #[cfg_attr(not(feature = "baremetal"), allow(dead_code))]
    user_sp: u64,
    /// M6d: the TTBR0_EL1 value (`root_pa | asid << 48`) that installs this task's address space, or 0
    /// for a kernel task (no switch — kernel mappings are Global and byte-identical in every root, so a
    /// kernel task runs correctly on whatever root is live). A shared-window EL0 task (M6b/M6e) carries
    /// the boot root `&L1 | ASID 0`; a per-task-slot EL0 task carries its slot root `slot_l1 | asid<<48`.
    /// `dispatch_next` installs it (only if it differs from the live TTBR0); `exit` tears the slot down
    /// (when `asid = user_ttbr0 >> 48` is non-zero).
    user_ttbr0: u64,
    /// SMP-BAL — the no-migrate flag. `true` = this task has NO hard core affinity and an idle core may
    /// STEAL it from its current run queue (`try_steal`); `false` = it is PINNED and never migrates. Set
    /// once at spawn: a `CPU_AUTO` (load-balanced) kernel task is steal-eligible; a task spawned with an
    /// explicit core index (render/input/pump/backstop/capstone) and EVERY EL0/slot task (which carry
    /// per-core TTBR0/ASID state) are pinned. Read ONLY under the owning run-queue lock (in `steal_one`),
    /// mutated ONLY by the stealer while it exclusively owns the popped `Box` (retargeting `cpu`), so it
    /// never races the owning core's dispatch. Honors the brief's "pinned tasks stay pinned" contract.
    steal_ok: bool,
    /// SPREAD-5 — CNTPCT timestamp at which this task was last PARKED, or 0 if it has never parked.
    /// Written by `park_blocked` (the sole park funnel) while it exclusively owns the Box; read by
    /// `make_ready` (the sole wake funnel) while it exclusively owns the Box. No other code touches
    /// it, so it needs no atomic and no lock despite park and wake happening on different cores —
    /// the Box handoff through the wait queue / sleeper list is the synchronisation.
    ///
    /// CNTPCT rather than the per-CPU `ticks`: park and wake are frequently on different cores, and
    /// the per-CPU tick counters are independent (a core that idles at WFI accrues them at its own
    /// pace), so a difference across cores would not be a duration. CNTPCT is system-global,
    /// fixed-frequency and always advancing — the same property the load accounting relies on.
    park_cyc: u64,
    /// SPREAD-6 (VUG-PACE-2) — CNTPCT timestamp at which the PLACEMENT QUESTION was last asked for
    /// this task (stamped at spawn, where placement is first decided, and by every `rewake_place`
    /// call thereafter). Same Box-handoff synchronisation argument as `park_cyc`: written only by
    /// whoever exclusively owns the Box.
    ///
    /// Why it exists: SPREAD-5 gates re-placement on a >= 100 ms park, and a frame-paced task NEVER
    /// parks that long — so whatever core assignment contention-era wakes left it with was PERMANENT.
    /// The s1q wire shows the cost as the residual "predestined fps": win1 pinned at 30.9/s for tens
    /// of seconds with two runnable EL0 tasks time-sharing c2 (99% busy) while c0/c1/c3 sat idle and
    /// `rewake=` never moved. This stamp lets a micro-park wake ask the question again on a slow
    /// clock (see `PLACE_REFRESH_MS`) without restoring SPREAD-4's per-frame churn.
    place_cyc: u64,
    /// SPREAD-7 — CNTPCT timestamp at which this task was last made READY by `make_ready` (EL0 tasks
    /// only; 0 = not currently priced). Consumed (read + zeroed) by the `dispatch_next` that first
    /// runs the task, yielding the wake-to-dispatch latency the `[spread7]` witness aggregates. Same
    /// Box-handoff synchronisation argument as `park_cyc`: written only by whoever exclusively owns
    /// the Box (the wake funnel on write, the dispatching core on consume).
    wake_cyc: u64,
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
    /// AARCH64-PRIO — monotonic count of `dispatch_next` passes on this CPU; the aging clock (see
    /// `AGE_TICKS`). Touched ONLY by this CPU's scheduler loop (sequential), so `Relaxed` suffices.
    age_passes: AtomicU64,
    /// AARCH64-PRIO — `age_passes` value at the last aging sweep; the elapsed since is what
    /// `RunQueue::age` accrues. Owning-CPU-only, `Relaxed`.
    age_last_sweep: AtomicU64,
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
            age_passes: AtomicU64::new(0),
            age_last_sweep: AtomicU64::new(0),
        }
    }
}

static SCHED: [SchedCpu; NUM_CPUS] = [const { SchedCpu::new() }; NUM_CPUS];

/// VUG-1 M3b — per-CPU load counters for the demo's "CPU pulse" meter (BeOS-Pulse style). Additive,
/// lock-free, relaxed: `dispatch_next` bumps `CPU_BUSY[cpu]` when it dispatches a task and
/// `CPU_IDLE[cpu]` when the run queue is empty (the core idles). The demo samples both once per frame
/// and shows busy/(busy+idle) over the window as a per-core bar. Introspection only — never read on
/// any scheduling path. This is the SEAM a real per-core utilization feed would replace.
static CPU_BUSY: [AtomicU64; NUM_CPUS] = [const { AtomicU64::new(0) }; NUM_CPUS];
static CPU_IDLE: [AtomicU64; NUM_CPUS] = [const { AtomicU64::new(0) }; NUM_CPUS];

/// SCHED-PRIO — the BASE priority of the task currently dispatched on each core, or [`PRIO_NONE`]
/// when the core is in its scheduler/idle context (or has never run the loop). Published by
/// `dispatch_next` around the switch, and the ONLY cross-core-readable fact about a running task
/// this module exposes: `SCHED[cpu].current` is a raw `*mut Task` whose Box the owning core may
/// reclaim at any moment, so it may never be dereferenced from another core. One relaxed byte can.
///
/// Read by [`preempt_hint`]; never read on the switch path itself.
static CUR_PRIO: [AtomicU8; NUM_CPUS] = [const { AtomicU8::new(PRIO_NONE) }; NUM_CPUS];

/// SCHED-PRIO — "no task running here". `u8::MAX` outranks every real level, so an idle core is
/// never a preemption candidate: the poke/reschedule SGI already wakes it, and there is nothing to
/// take the CPU away from.
const PRIO_NONE: u8 = u8::MAX;

/// SCHED-PRIO — the `[prio]` witness counters. All monotonic, relaxed, per core; read only by
/// `prio_witness`, which reports per-window DELTAS against its own last snapshot.
///
///   * `PRIO_SVC_DISPATCH` — dispatches whose task's BASE priority is at or above `PRIO_SERVICE`
///     (an aged-up EL0 task counts as EL0, which is the honest reading: the band did not win that
///     dispatch, the anti-starvation sweep did).
///   * `PRIO_EL0_DISPATCH` — dispatches of an EL0/user task (`user_entry != 0`), the population the
///     band is being ranked against.
///   * `PRIO_DEFER` — service-band wakes that landed on a core running a LOWER-band task, i.e. the
///     times the compositor/router was ready but had to wait out (a trimmed remainder of) somebody
///     else's quantum. This is the residual latency the band does NOT remove, kept visible on
///     purpose — a `defer` that stays small beside a large `svc` is the band working.
///   * `PRIO_AGED_IN` — `RunQueue::age` relocations that lifted a below-band task INTO the band.
static PRIO_SVC_DISPATCH: [AtomicU64; NUM_CPUS] = [const { AtomicU64::new(0) }; NUM_CPUS];
static PRIO_EL0_DISPATCH: [AtomicU64; NUM_CPUS] = [const { AtomicU64::new(0) }; NUM_CPUS];
static PRIO_DEFER: [AtomicU64; NUM_CPUS] = [const { AtomicU64::new(0) }; NUM_CPUS];
static PRIO_AGED_IN: [AtomicU64; NUM_CPUS] = [const { AtomicU64::new(0) }; NUM_CPUS];
/// Last-window snapshots, so `[prio]` can print per-window deltas beside the running totals.
static PRIO_LAST_SVC: AtomicU64 = AtomicU64::new(0);
static PRIO_LAST_EL0: AtomicU64 = AtomicU64::new(0);
static PRIO_LAST_DEFER: AtomicU64 = AtomicU64::new(0);
static PRIO_LAST_AGED: AtomicU64 = AtomicU64::new(0);

/// SCHED-7 — the CNTPCT busy span `dispatch_next` folded into the load window on the CURRENT pass
/// (0 when the run queue was empty). `run()` reads+clears it after every pass and folds the REST of
/// the pass's wall span in as IDLE, so no wall time is left unaccounted. Single-writer (the owning
/// core's scheduler loop writes it in `dispatch_next`; the same loop consumes it in `run()`); relaxed.
static PASS_BUSY_CYC: [AtomicU64; NUM_CPUS] = [const { AtomicU64::new(0) }; NUM_CPUS];

// ---------------------------------------------------------------------------------------------
// SCHED-2 — per-core load accounting (rolling-window utilization, ctx-switch count, last task)
// ---------------------------------------------------------------------------------------------
//
// The `CPU_BUSY`/`CPU_IDLE` pulse counters above are CUMULATIVE-since-boot pass counts — good for a
// meter that diffs across its own frame window, but they never answer "what is this core doing NOW"
// on demand (busy% since boot converges to a flat average). This adds a LIVE per-core view: a
// busy fraction over the most-recent WINDOW dispatch passes, a running context-switch count, and the
// id+name of the last task dispatched on the core.
//
// Cost & concurrency: strictly per-core, single-writer (only the owning core's scheduler loop writes
// its slot, from the tick/switch path that is already running), lock-free, Relaxed atomics — it adds
// NO new lock to the scheduler hot path and no cross-core contention. Each dispatch pass does a
// handful of relaxed loads/stores. Reads (the `top` shell verb, the witnesses) may run from ANY core;
// the counters are plain relaxed reads and the last-task (id + &'static str) is published under a
// per-core SEQLOCK so a cross-core reader never observes a torn id/name/len triple. Introspection
// only: `core_load` and the witnesses NEVER run on a scheduling decision path.
//
// SCHED-5 — TIME, NOT PASSES. `busy_pct_recent` was originally "fraction of the last WINDOW dispatch
// PASSES that dispatched work". That counts scheduler activity, not CPU time: a task that wakes once
// per 4 ms tick, runs for microseconds, then blocks looks 50% "loaded" (one busy pass, one idle pass
// per tick) while consuming almost no CPU — it cannot tell a busy-loop apart from a bare cadence, and
// it misled a whole downstream arc (NET-19 was briefed off a spurious `c1=50%`). SCHED-5 makes the
// metric time-based: it measures the CYCLES (free-running CNTPCT, see `now_cyc`) this core spends
// EXECUTING tasks vs sitting IDLE (WFI / empty-queue), and reports the busy time fraction over a
// rolling ~250 ms window. The counter is read only at the dispatch entry/exit boundary (around
// `switch_context`) and at the idle WFI in `run()` — no per-instruction cost. CNTPCT (unlike the
// Group-1 timer IRQ, which QEMU withholds) is always running, so the accounting advances on every
// path — cooperative and preemptive, QEMU and metal. The LINE FORMAT is unchanged
// (`SCHED: load cN=..%`); only the meaning of the percentage moves from pass-fraction to time-fraction.

/// Rolling-window size, in CNTPCT cycles, over which `busy_pct_recent` is computed. A completed window
/// (busy+idle cycles reaching this budget) snapshots its busy TIME fraction and resets, so the reported
/// percentage tracks recent utilization rather than the since-boot average. Derived from the counter
/// frequency (`CNTFRQ_EL0`) as ~250 ms, so the window is a fixed wall-clock span on any board (the
/// frequency differs — ~54 MHz on the BCM2711, ~62.5 MHz on QEMU virt — but the percentage is a ratio,
/// so only the smoothing span is frequency-derived, never the value).
static CNTFRQ_HZ: AtomicU64 = AtomicU64::new(0);

/// Free-running system counter (CNTPCT_EL0) — the time base for time-based load accounting. Mirrors
/// genet.rs's `now_cyc` probe but kept self-contained here (the scheduler pulls in no net code). A
/// single non-trapping EL2/EL1 sysreg read; NOT CPU clock cycles (CNTPCT is a fixed-frequency counter),
/// but a stable monotonic tick that is ALWAYS advancing, so it works even where the timer IRQ does not.
#[inline]
fn now_cyc() -> u64 {
    let cnt: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntpct_el0", out(reg) cnt, options(nomem, nostack, preserves_flags));
    }
    cnt
}

/// The load window budget in CNTPCT cycles (~250 ms). Reads `CNTFRQ_EL0` once and caches it (some
/// firmwares report it per-boot but not per-pass-cheap, so cache to keep the accounting fold O(1)).
/// Falls back to a nominal 54 MHz if firmware leaves CNTFRQ_EL0 at 0 (only affects the smoothing span).
#[inline]
fn load_window_cyc() -> u64 {
    let f = CNTFRQ_HZ.load(Ordering::Relaxed);
    let frq = if f != 0 {
        f
    } else {
        let v: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntfrq_el0", out(reg) v, options(nomem, nostack, preserves_flags));
        }
        let v = if v == 0 { 54_000_000 } else { v };
        CNTFRQ_HZ.store(v, Ordering::Relaxed);
        v
    };
    frq / 4
}

/// Sentinel `recent_pct` meaning "no window has completed yet" (fall back to the partial window).
const LOAD_PCT_NONE: u32 = u32::MAX;

// PULSE-5 — A WINDOW THAT CLOSES ONLY AT A DISPATCH BOUNDARY CANNOT REPORT A CORE THAT NEVER
// REACHES ONE.
//
// SCHED-5/SCHED-7 fold a span into the window at exactly two points: after `switch_context` returns
// (busy) and at the bottom of `run()`'s pass (idle). Both are DISPATCH BOUNDARIES. So the reported
// number — `recent_pct`, the last COMPLETED window — is only as fresh as the core's last boundary:
//
//   * best case (a core cycling the loop): the value is a *previous* window, i.e. 250-500 ms old
//     by construction. That is the P69 shape — "45% unused" printed while vugs were starving.
//   * worst case (one compute-bound task holding the core): the busy span is UNBOUNDED — nothing is
//     folded and nothing rolls, so `recent_pct` freezes at its pre-storm value for as long as the
//     task runs. In the QEMU raspi4b gate there is no Group-1 timer delivery at all, so there is no
//     preemption to break the span and the freeze is total; on metal the quantum (3 ticks, ~12 ms)
//     does break it, but the value still lags a whole window, and the core trips SCHED-8's
//     `tracked()` staleness bound whenever a fold gap exceeds ~500 ms — at which point every honest
//     consumer (`SCHED: load`, `top`, `ui_status::live_permille`) drops to `--`/no-live-number and
//     the status strip falls back to the coarse dispatch-pass classifier.
//
// Everything downstream of that number inherits the lag, and two of the consumers are DECISION
// paths, not displays: `pick_cpu`'s tie-break key 4 (placement) and `video::screen::flush_parallel`'s
// helper ranking + headroom weights (which core gets a render band). A stale-low percent on a
// saturated core therefore attracts MORE work to the core that is already the problem.
//
// THE FIX IS AGE-ON-READ, not account-at-tick. `busy_pct()` now adds the CURRENTLY-EXECUTING span
// (`now - run_t0`) into the window's busy total at READ time, so a long-running task's core reads
// honestly busy within a single read, with no dispatch boundary required. See `run_t0` and
// `busy_pct` for the mechanism and the ordering argument.
//
// Why not account-at-tick (close/roll the window from the 250 Hz timer IRQ)? Three reasons, in
// order of weight:
//   1. The tick does not exist where the bug is worst. `timer::on_tick` / `timer_preempt` are
//      METAL-ONLY — QEMU raspi4b never delivers the timer PPI — so a tick-driven window would be
//      dead code in the gate that has to prove it, and would leave the total-freeze case unfixed on
//      exactly the platform where the span is never broken at all.
//   2. It cannot see the in-flight span without moving state. The busy anchor (`busy_t0`) is a
//      LOCAL in `dispatch_next`, live on the scheduler stack across `switch_context`; a tick
//      handler firing on top of the running task would have to take ownership of that anchor and
//      re-publish a partial span — i.e. push state the dispatch path exclusively owns into a
//      shared, interrupt-reentrant location, on the switch path, to buy a COARSER answer.
//   3. The cost lands on the wrong path. Account-at-tick pays on every tick of every core forever;
//      age-on-read pays only when someone asks, and readers are the rare path (a 4 Hz strip, a
//      1024-tick witness, placement).
// Age-on-read costs ONE relaxed store on the dispatch path, one relaxed store in the fold, and one
// extra sysreg read + two loads per READ. It is strictly cheaper and strictly fresher.

/// One core's load accounting slot. All fields written ONLY by the owning core's scheduler loop
/// (single-writer, Relaxed); read cross-core by introspection. The last-task triple is seqlock-
/// protected (odd `last_seq` = write in progress) so a reader never reconstructs a torn `&str`.
struct CoreAccount {
    /// Cumulative context switches INTO a task on this core (one per busy dispatch).
    ctx_switches: AtomicU64,
    /// SCHED-5 — CNTPCT cycles spent EXECUTING tasks in the CURRENT (incomplete) window.
    win_busy_cyc: AtomicU64,
    /// SCHED-5 — CNTPCT cycles spent IDLE (WFI / empty queue) in the current window. The window rolls
    /// over when `win_busy_cyc + win_idle_cyc` reaches `load_window_cyc()`.
    win_idle_cyc: AtomicU64,
    /// Busy percent (0..=100) of the last COMPLETED window, or `LOAD_PCT_NONE` before the first.
    recent_pct: AtomicU32,
    /// SPREAD-9 — the SERVICE-BAND share of `win_busy_cyc`: CNTPCT cycles of the current window
    /// spent executing tasks whose BASE priority is `>= PRIO_SERVICE`. Always `<= win_busy_cyc`
    /// (folded by the same call, same single writer, same window roll). Exists so EL0 placement can
    /// subtract the time that no longer competes with EL0: with services preempting at IPI receipt
    /// (`ipi_preempt`), service execution costs an EL0 co-resident latency measured in
    /// microseconds, and letting it inflate the placement percent kept a hole in the fleet's
    /// spread wherever the service band currently lived.
    win_svc_cyc: AtomicU64,
    /// SPREAD-9 — service-band busy percent (0..=100) of the last COMPLETED window (0 before the
    /// first — with no history the honest default is "no service load", which reduces to the
    /// pre-arc reading). Companion of `recent_pct`, filled by the same window roll.
    recent_svc_pct: AtomicU32,
    /// PULSE-5 — CNTPCT timestamp at which the CURRENTLY-EXECUTING task's span began on this core,
    /// or 0 when this core is not inside a task (scheduler overhead, idle, or it left `run()`).
    /// Published by the owning core immediately before `switch_context` and cleared by the fold in
    /// `account()`; it is the ONLY thing a reader needs to age the in-flight span (`now - run_t0`)
    /// into the current window, which is what makes `busy_pct()` independent of dispatch boundaries.
    ///
    /// ORDERING (the one subtle part). A cross-core reader must never count the same span twice —
    /// once as `now - run_t0` and again inside `win_busy_cyc` after the fold folded it. The writer's
    /// order in `account()` is: clear `run_t0` (Relaxed) THEN publish `win_busy_cyc` (Release). The
    /// reader's order in `busy_pct()` is: load `win_busy_cyc` (Acquire) THEN load `run_t0`
    /// (Relaxed). If the reader's Acquire load observes the post-fold busy total it
    /// synchronizes-with that Release store, so the preceding `run_t0 = 0` is visible to its later
    /// load and the span cannot be added twice. The converse skew (reader sees the pre-fold total
    /// and a freshly-cleared `run_t0`) merely UNDER-counts by one span for one read — which is
    /// exactly the pre-PULSE-5 behaviour, so it is never worse than the code it replaces.
    run_t0: AtomicU64,
    /// SCHED-8 — CNTPCT timestamp of the most recent `account()` fold (0 = never accounted). A core
    /// running `run()` folds a busy OR idle span every dispatch pass, so this stays fresh; a core that
    /// left `run()` (the Pi/tegra BSP `hlt_loop`s after spawning services; the virt boot core spin-loops
    /// after CAPSTONE drains) stops folding, so this freezes and `tracked()` reports the slot STALE. That
    /// is what makes the `SCHED: load` line print `--` for an untracked core instead of the last window's
    /// frozen percent (the c0=100% artifact): honest "no current data" over a fabricated number.
    last_acct_cyc: AtomicU64,
    /// Seqlock sequence for the last-task triple: even = stable, odd = write in progress.
    last_seq: AtomicU64,
    /// Id of the last task dispatched here (0 = none yet).
    last_id: AtomicU64,
    /// `&'static str` name of the last task, split into `.as_ptr()` + `.len()` (both published under
    /// the seqlock). A `&'static str` is a 16-byte fat pointer that no single atomic can hold, so the
    /// seqlock is what makes the cross-core read of the pair sound.
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
            win_svc_cyc: AtomicU64::new(0),
            recent_svc_pct: AtomicU32::new(0),
            run_t0: AtomicU64::new(0),
            last_acct_cyc: AtomicU64::new(0),
            last_seq: AtomicU64::new(0),
            last_id: AtomicU64::new(0),
            last_name_ptr: AtomicUsize::new(0),
            last_name_len: AtomicUsize::new(0),
        }
    }

    /// SCHED-5 — fold a measured span into the rolling window (single-writer, owning core). Exactly one
    /// of `busy_cyc` / `idle_cyc` is non-zero per call: `account(delta, 0, ..)` after a task's execution
    /// span (around `switch_context`), `account(0, delta, 0)` after an idle WFI. On window completion
    /// (busy+idle cycles reaching the ~250 ms budget) it snapshots the busy TIME fraction and resets.
    /// Relaxed loads/stores are sound because only this core's scheduler loop ever writes its slot.
    ///
    /// SPREAD-9 — `svc_cyc` is the service-band portion of `busy_cyc`: equal to it when the span just
    /// measured belonged to a task of BASE priority `>= PRIO_SERVICE`, else 0 (and always 0 on the
    /// idle call). It rides the same fold so `win_svc_cyc`/`recent_svc_pct` can never skew against
    /// the totals they are subtracted from.
    #[inline]
    fn account(&self, busy_cyc: u64, idle_cyc: u64, svc_cyc: u64) {
        // SCHED-8: mark the slot fresh — every dispatch pass in `run()` folds a span, so a core still
        // inside the scheduler keeps this current; a core that left `run()` stops touching it (goes STALE).
        self.last_acct_cyc.store(now_cyc(), Ordering::Relaxed);
        // PULSE-5: any fold ENDS the in-flight span — whatever was executing has now been measured
        // and is about to land in `win_busy_cyc`, so a reader must stop aging it. Cleared BEFORE the
        // busy total is published (Release, below) so the pair can never be read as a double count;
        // see `run_t0`'s ordering note. One relaxed store; no branch, both call sites covered.
        self.run_t0.store(0, Ordering::Relaxed);
        let busy = self.win_busy_cyc.load(Ordering::Relaxed) + busy_cyc;
        let idle = self.win_idle_cyc.load(Ordering::Relaxed) + idle_cyc;
        let svc = self.win_svc_cyc.load(Ordering::Relaxed) + svc_cyc;
        let total = busy + idle;
        if total >= load_window_cyc() {
            self.recent_pct.store((busy * 100 / total) as u32, Ordering::Relaxed); // total>=budget>0
            self.recent_svc_pct.store((svc * 100 / total) as u32, Ordering::Relaxed); // SPREAD-9
            self.win_svc_cyc.store(0, Ordering::Relaxed);
            self.win_idle_cyc.store(0, Ordering::Relaxed);
            self.win_busy_cyc.store(0, Ordering::Release); // PULSE-5: publishes the `run_t0` clear
            PULSE5_FOLD_WINDOWS.fetch_add(1, Ordering::Relaxed);
        } else {
            self.win_svc_cyc.store(svc, Ordering::Relaxed);
            self.win_idle_cyc.store(idle, Ordering::Relaxed);
            self.win_busy_cyc.store(busy, Ordering::Release); // PULSE-5: publishes the `run_t0` clear
        }
    }

    /// Publish the last-dispatched task's id + name under the seqlock (owning core only). Bump the
    /// sequence odd, write the triple, bump it even — a reader spinning on an even/matching sequence
    /// then sees a consistent snapshot. Release fences pair with the reader's Acquire loads.
    #[inline]
    fn note_last(&self, id: u64, name: &'static str) {
        let seq = self.last_seq.load(Ordering::Relaxed);
        self.last_seq.store(seq + 1, Ordering::Release); // odd: write in progress
        self.last_id.store(id, Ordering::Relaxed);
        self.last_name_ptr.store(name.as_ptr() as usize, Ordering::Relaxed);
        self.last_name_len.store(name.len(), Ordering::Relaxed);
        self.last_seq.store(seq + 2, Ordering::Release); // even: stable
    }

    /// PULSE-5 — CNTPCT cycles the core has ALREADY spent inside the task it is executing right now,
    /// or 0 if it is not inside one. This is the quantity SCHED-5's fold cannot see until the task
    /// switches back, and therefore the quantity whose absence froze `busy_pct` under a compute-bound
    /// task. One relaxed load + one sysreg read; safe from any core (CNTPCT is system-global, so
    /// `now - run_t0` is a real elapsed span whichever core evaluates it).
    #[inline]
    fn live_span_cyc(&self) -> u64 {
        let t0 = self.run_t0.load(Ordering::Relaxed);
        if t0 == 0 {
            0
        } else {
            now_cyc().wrapping_sub(t0)
        }
    }

    /// PULSE-5 — busy percent (0..=100) for `core_load`, computed for the window AS IT STANDS NOW
    /// rather than as of the last completed one. Three cases, in the order they are tested:
    ///
    ///   1. The in-flight span alone covers a whole window: the last ~250 ms were, in their
    ///      entirety, this core executing one task. That is 100%, and no other term can change it —
    ///      this is the compute-bound case the arc exists for, and it is now a single read away
    ///      from the truth instead of an unbounded wait for a dispatch boundary.
    ///   2. The current (partial) window plus the in-flight span already spans a full window's
    ///      worth of measured time: report that occupancy directly. No stale term is consulted.
    ///   3. The window is still short: report the measured part at full weight and fill only the
    ///      REMAINDER of the window from the last completed window's rate. This is what keeps the
    ///      number continuous — a core that has just rolled its window does not drop to a noisy
    ///      two-millisecond sample — while bounding how much of the answer can be historical: the
    ///      stale term's weight is exactly the fraction of the window not yet measured, and it
    ///      decays to zero as the window fills. Before any window has completed (`LOAD_PCT_NONE`)
    ///      there is no historical rate, so it falls through to the measured part alone.
    ///
    /// Read order (`win_busy_cyc` Acquire, then `run_t0`) is load-bearing against double-counting a
    /// span that the fold has just banked; see `run_t0`. Lock-free, allocation-free, callable from
    /// any core, and cheap enough for `pick_cpu`: two atomic loads, one sysreg read, integer math.
    fn busy_pct(&self) -> u32 {
        let budget = load_window_cyc();
        // Acquire pairs with `account`'s Release store; it must be read BEFORE `run_t0`.
        let win_busy = self.win_busy_cyc.load(Ordering::Acquire);
        let win_idle = self.win_idle_cyc.load(Ordering::Relaxed);
        let live = self.live_span_cyc();
        if live >= budget {
            return 100; // case 1 — the whole window is one uninterrupted execution span
        }
        let busy = win_busy + live;
        let elapsed = busy + win_idle;
        let recent = self.recent_pct.load(Ordering::Relaxed);
        if elapsed >= budget || recent == LOAD_PCT_NONE {
            // case 2 (and the pre-first-window fallback): measured time only.
            if elapsed == 0 {
                0
            } else {
                ((busy * 100 / elapsed) as u32).min(100)
            }
        } else {
            // case 3 — measured part at full weight, the unmeasured remainder at the last window's
            // rate. `busy` < 2*budget and `recent` <= 100, so both products stay far inside u64.
            let rem = budget - elapsed;
            (((busy * 100 + recent as u64 * rem) / budget) as u32).min(100)
        }
    }

    /// SPREAD-9 — busy percent EXCLUDING the service band: the load figure EL0 placement weighs now
    /// that services preempt at IPI receipt. Same three-case shape and read ordering as `busy_pct`
    /// (which see), with the service share removed from each term:
    ///
    ///   * the windowed part subtracts `win_svc_cyc` from `win_busy_cyc` (same fold, so never skewed
    ///     the wrong way; `saturating_sub` covers the one-call read race between the two loads);
    ///   * the in-flight span counts only if the task executing RIGHT NOW is below the band —
    ///     `live_is_svc` is the caller's `CUR_PRIO` read for this core, the same published word
    ///     `preempt_hint` keys on;
    ///   * the historical fill uses `recent_pct - recent_svc_pct` (snapshotted by the same roll).
    ///
    /// ELAPSED time keeps the FULL busy (service included): the answer is "what fraction of this
    /// core's time went to work an EL0 arrival would actually wait behind", not a renormalization
    /// that would inflate the EL0 share on a service-heavy core.
    fn el0_busy_pct(&self, live_is_svc: bool) -> u32 {
        let budget = load_window_cyc();
        // Acquire pairs with `account`'s Release store; it must be read BEFORE `run_t0` (see `run_t0`).
        let win_busy = self.win_busy_cyc.load(Ordering::Acquire);
        let win_svc = self.win_svc_cyc.load(Ordering::Relaxed);
        let win_idle = self.win_idle_cyc.load(Ordering::Relaxed);
        let live = self.live_span_cyc();
        let live_el0 = if live_is_svc { 0 } else { live };
        if live_el0 >= budget {
            return 100; // case 1 — the whole window is one uninterrupted below-band execution span
        }
        let el0_busy = win_busy.saturating_sub(win_svc) + live_el0;
        let elapsed = win_busy + live + win_idle;
        let recent = self.recent_pct.load(Ordering::Relaxed);
        if elapsed >= budget || recent == LOAD_PCT_NONE {
            // case 2 (and the pre-first-window fallback): measured time only.
            if elapsed == 0 {
                0
            } else {
                ((el0_busy * 100 / elapsed) as u32).min(100)
            }
        } else {
            // case 3 — measured part at full weight, the unmeasured remainder at the last window's
            // below-band rate.
            let recent_el0 =
                recent.saturating_sub(self.recent_svc_pct.load(Ordering::Relaxed)) as u64;
            let rem = budget - elapsed;
            (((el0_busy * 100 + recent_el0 * rem) / budget) as u32).min(100)
        }
    }

    /// SCHED-8 — is this core's load being accounted RIGHT NOW? True when the owning core folded a span
    /// within the last ~2 load windows (~500 ms): i.e. it is inside `run()`, folding a busy or idle span
    /// every dispatch pass. False when the slot has never been touched (never-scheduled core) or has gone
    /// stale — the core left the scheduler loop (the Pi/tegra BSP `hlt_loop`; the virt boot core's post-
    /// CAPSTONE spin) and its `recent_pct` is a frozen snapshot, not a live number. Untracked cores are
    /// reported `--` rather than that frozen percent. Two windows of slack absorbs a slow rollover so a
    /// genuinely-scheduled core is never mislabeled stale.
    fn tracked(&self) -> bool {
        let last = self.last_acct_cyc.load(Ordering::Relaxed);
        if last == 0 {
            return false; // never accounted — this core has not run the scheduler loop
        }
        // PULSE-5: a core with an IN-FLIGHT execution span is being accounted by definition — it is
        // inside `run()`, executing a dispatched task, and `busy_pct()` now measures that span
        // live. Before this arm, a core holding one compute-bound task for longer than two windows
        // went `--` at exactly the moment it was the busiest thing on the machine: the fold gap
        // aged past the staleness bound, `SCHED: load` printed `--`, and `ui_status::live_permille`
        // returned None so the status strip fell back to the dispatch-pass classifier — the
        // "45% unused while vugs starved" reading. The fold-age bound below still governs every
        // core that is NOT executing (BSP `hlt_loop`, post-CAPSTONE spin, never-scheduled), which
        // is the case SCHED-8 introduced it for; `fold_age_cyc` is deliberately left untouched, so
        // WEDGE-1's much tighter "may I pin work here" gate keeps reading the raw fold age and
        // still disqualifies a core that is not going round the dispatch loop.
        if self.run_t0.load(Ordering::Relaxed) != 0 {
            return true;
        }
        now_cyc().wrapping_sub(last) < load_window_cyc().saturating_mul(2)
    }

    /// WEDGE-1 — cycles since the last fold, or `u64::MAX` if this core has never folded a span.
    /// The raw quantity `tracked()` thresholds; see `CoreLoad::fold_age_cyc`.
    fn fold_age_cyc(&self) -> u64 {
        let last = self.last_acct_cyc.load(Ordering::Relaxed);
        if last == 0 {
            return u64::MAX;
        }
        now_cyc().wrapping_sub(last)
    }

    /// Read the last-task triple with a bounded seqlock retry. `&'static str` reconstruction is sound:
    /// the writer only ever publishes a live `'static` name's `(ptr, len)` pair, and the seqlock
    /// guarantees the reader sees a matching pair (never a torn ptr-from-A / len-from-B).
    fn last_task(&self) -> (u64, &'static str) {
        for _ in 0..8 {
            let s1 = self.last_seq.load(Ordering::Acquire);
            if s1 & 1 != 0 {
                continue; // write in progress; retry
            }
            let id = self.last_id.load(Ordering::Relaxed);
            let ptr = self.last_name_ptr.load(Ordering::Relaxed);
            let len = self.last_name_len.load(Ordering::Relaxed);
            if self.last_seq.load(Ordering::Acquire) != s1 {
                continue; // changed under us; retry
            }
            if ptr == 0 || len == 0 {
                return (id, "-");
            }
            // SAFETY: `ptr`/`len` were published together (seqlock-consistent) from a live `&'static
            // str`, so this reconstructs that exact still-live string slice.
            let name = unsafe {
                core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr as *const u8, len))
            };
            return (id, name);
        }
        (self.last_id.load(Ordering::Relaxed), "?")
    }
}

static ACCT: [CoreAccount; NUM_CPUS] = [const { CoreAccount::new() }; NUM_CPUS];

/// A snapshot of one core's live scheduler load (SCHED-2). Returned by `core_load`; consumed by the
/// `top` shell verb and the load witnesses. All fields are a point-in-time read — introspection only.
pub struct CoreLoad {
    /// SCHED-5 — busy TIME fraction (0..=100): CNTPCT cycles spent executing tasks over the most recent
    /// ~250 ms window (was, pre-SCHED-5, the fraction of dispatch PASSES that ran work — a cadence proxy
    /// that read ~50% for a task waking once per tick; now it is real CPU utilization).
    ///
    /// PULSE-5 — and it is CURRENT, not last-window. The value includes the span the core is executing
    /// at this instant, so a core inside a multi-second compute-bound task reads ~100% on the first
    /// read rather than reporting its pre-storm percent until the task ends. See `CoreAccount::busy_pct`.
    pub busy_pct_recent: u32,
    /// Cumulative context switches into a task on this core since boot.
    pub ctx_switches: u64,
    /// Id of the last task dispatched on this core (0 = none yet).
    pub last_task_id: u64,
    /// Name of the last task dispatched on this core ("-" = none yet).
    pub last_task: &'static str,
    /// SCHED-8 — is `busy_pct_recent` a LIVE number? True when this core is currently inside `run()`
    /// (folding a span every dispatch pass); false when its accounting slot is stale — the core left the
    /// scheduler loop (BSP `hlt_loop` / post-CAPSTONE spin) or never ran it, so `busy_pct_recent` is a
    /// frozen last-window snapshot. Honest views (`SCHED: load` line, `top`) render an untracked core as
    /// `--`; the liveness gate still reads the raw `busy_pct_recent`/`ctx_switches` so it stays green.
    pub tracked: bool,
    /// WEDGE-1 — CNTPCT cycles since this core last folded a load span, i.e. how long since it was
    /// provably going round the dispatch loop. `u64::MAX` when it has never folded one.
    ///
    /// `tracked` answers "is this number worth PRINTING", and its ~2-window (~500 ms) slack is right
    /// for that: it keeps a genuinely-scheduled core off the `--` display during a slow rollover.
    /// That slack is wrong for a caller asking "may I hand this core work it alone can run" — for
    /// 500 ms after a core stops dispatching, `tracked` still says yes. `fold_age_cyc` is the raw
    /// measurement under both questions, so such a caller can pick its own, much tighter bound.
    /// See `video::screen::flush_parallel`'s helper gate, which uses it to avoid pinning a
    /// non-stealable band onto a core that is not dispatching. (Hardening: that hazard is real on its
    /// own terms and is NOT a diagnosed cause of the P66 wedge, whose mechanism is unknown.)
    pub fold_age_cyc: u64,
}

/// WEDGE-1 — the freshness bound a caller should use before handing a core work that ONLY that core
/// can run (a pinned, non-stealable task with an untimed join). One eighth of a load window, ~30 ms:
/// a core inside `run()` folds a span every dispatch pass — orders of magnitude more often than this
/// — so a live core never trips it, while a core that has stopped dispatching is disqualified within
/// a frame or two rather than the ~500 ms `tracked()` allows.
///
/// Deliberately conservative in the safe direction: a false "not fresh" costs one band of parallelism
/// for one frame, a false "fresh" costs a task parked forever.
pub fn dispatch_fresh_cyc() -> u64 {
    (load_window_cyc() / 8).max(1)
}

/// SCHED-2 — read this core's live load: recent busy percent (rolling window), cumulative context
/// switches, and the last task dispatched. Allocation-free and lock-free; callable from ANY core
/// (the shell `top` verb, the witnesses). Introspection only — never consulted on a scheduling path.
pub fn core_load(core: usize) -> CoreLoad {
    if core >= NUM_CPUS {
        return CoreLoad {
            busy_pct_recent: 0,
            ctx_switches: 0,
            last_task_id: 0,
            last_task: "-",
            tracked: false,
            fold_age_cyc: u64::MAX,
        };
    }
    let acct = &ACCT[core];
    let (last_task_id, last_task) = acct.last_task();
    CoreLoad {
        busy_pct_recent: acct.busy_pct(),
        ctx_switches: acct.ctx_switches.load(Ordering::Relaxed),
        last_task_id,
        last_task,
        tracked: acct.tracked(),
        fold_age_cyc: acct.fold_age_cyc(),
    }
}

/// AARCH64-PRIO — a CPU's ready tasks, bucketed by EFFECTIVE level. A task normally sits at its base
/// `priority` level, but the aging sweep (`age`) may transiently lift a long-waiting task to a higher
/// level so strict priority does not starve it; on its next enqueue it re-bases. One spinlock (in
/// `RUN_QUEUES`) guards all levels; held only briefly (push/pop are O(NUM_PRIORITIES); `age` is
/// O(ready tasks)) and always with IRQ masked.
///
/// Two distinct placement operations share these levels: ENQUEUE (`push`) re-bases a task to its
/// base-priority level and ZEROES its aging clock; RELOCATE (`age`) moves a task one level UP without
/// touching its base priority. They must not be confused (relocating via `push` would be a no-op
/// promotion that leaves starvation intact).
struct RunQueue {
    levels: [VecDeque<Box<Task>>; NUM_PRIORITIES],
}

impl RunQueue {
    /// `const` (each level a const-`new` `VecDeque`) so `RUN_QUEUES` stays a plain const static, no
    /// lazy_static — matching this module's existing run-queue construction.
    const fn new() -> Self {
        RunQueue { levels: [const { VecDeque::new() }; NUM_PRIORITIES] }
    }
    /// ENQUEUE a task at its BASE priority level (FIFO within the level), clamped in range, and reset
    /// its aging clock — every enqueue (spawn / wake / re-enqueue after preempt/yield) zeroes
    /// `wait_ticks`, so a task only ages while it sits WAITING and re-bases the moment it is requeued.
    fn push(&mut self, mut task: Box<Task>) {
        task.wait_ticks = 0;
        let level = (task.priority as usize).min(NUM_PRIORITIES - 1);
        self.levels[level].push_back(task);
    }
    /// Total ready tasks across all levels — the depth signal SCHED-3 load-balanced placement reads
    /// to pick the least-loaded core (introspection under the run-queue lock, never on the switch path).
    fn len(&self) -> usize {
        self.levels.iter().map(VecDeque::len).sum()
    }
    /// SPREAD-9 — ready tasks BELOW the service band: the depth EL0 placement weighs. A queued
    /// service-band task is not competition an EL0 task will ever wait a quantum behind — it
    /// preempts at IPI receipt, runs a micro pass and blocks — so counting it made the core hosting
    /// the services read deeper than the load an EL0 arrival would actually contend with (the
    /// placement half of the dissolved service-core reserve). Effective level is the right key here:
    /// an aged-up EL0 task sitting IN the band is transiently excluded, which errs toward the old
    /// (conservative) reading for exactly the population the anti-starvation valve is about to run.
    fn len_below_band(&self) -> usize {
        self.levels[..PRIO_SERVICE as usize].iter().map(VecDeque::len).sum()
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
    /// SMP-BAL — remove and return the first STEAL-ELIGIBLE (`steal_ok`) ready task, scanning LOW→HIGH
    /// priority (take a core's BACKGROUND work first, never rob it of its most-urgent task) and front-
    /// first within a level (oldest waiter). Pinned tasks (render/input/pump/capstone/EL0) are skipped
    /// and left in place. Returns `None` if the queue holds only pinned work. Runs on the STEALER's core
    /// under the VICTIM's run-queue lock; every task here is `STATE_READY` (a running task is out of the
    /// queue in `current`, a blocked one is in a wait/sleeper list), so a stolen task is always safe to
    /// re-home. O(ready tasks) worst case, off the switch hot path (only an idle core with an empty queue).
    fn steal_one(&mut self) -> Option<Box<Task>> {
        for level in self.levels.iter_mut() {
            if let Some(pos) = level.iter().position(|t| t.steal_ok) {
                return level.remove(pos);
            }
        }
        None
    }
    /// Priority-aging sweep (anti-starvation): RELOCATE every ready task that has now waited at least
    /// `AGE_TICKS` one level UP, carrying any surplus credit to the next sweep. `elapsed` is the aging
    /// units (dispatch passes) since the previous sweep. Run on the OWNING CPU under the run-queue lock.
    ///
    /// Iterating HIGH→LOW is load-bearing: a task promoted from `level` into `level + 1` lands in a
    /// level that was ALREADY processed this sweep, so it is never revisited (exactly-once per sweep,
    /// no runaway multi-level jump). Within a level, popping exactly `n = len()` from the front and
    /// pushing kept tasks to the back rotates the deque full-circle, preserving FIFO. Relocation is a
    /// raw `VecDeque` move that leaves `priority` (base) untouched — NOT `push`. A `push_back` into
    /// `level + 1` may reallocate under the run-queue lock; that is benign here exactly as at `spawn`
    /// (the heap lock is always innermost — run-queue → heap is the only ordering, never inverted).
    ///
    /// SCHED-PRIO — returns how many of this sweep's relocations landed a task INTO the interactive
    /// service band (from below `PRIO_SERVICE` to at or above it). That number is the fairness escape
    /// valve made countable: it is exactly how often a below-band task (an EL0 vug) had waited long
    /// enough to be lifted to parity with the compositor, and `[prio] agedin=` reports it. Counting
    /// here costs one comparison on a path that is already O(ready tasks) and off the switch.
    fn age(&mut self, elapsed: u32) -> u32 {
        let mut into_band = 0u32;
        for level in (0..NUM_PRIORITIES - 1).rev() {
            let n = self.levels[level].len();
            for _ in 0..n {
                let mut task = self.levels[level].pop_front().expect("age: len/pop mismatch");
                task.wait_ticks = task.wait_ticks.saturating_add(elapsed);
                if task.wait_ticks >= AGE_TICKS {
                    task.wait_ticks -= AGE_TICKS; // carry surplus credit, don't discard it
                    debug_assert!(level + 1 < NUM_PRIORITIES, "age: promotion above top level");
                    if level + 1 >= PRIO_SERVICE as usize && level < PRIO_SERVICE as usize {
                        into_band += 1;
                    }
                    self.levels[level + 1].push_back(task); // RELOCATE up one level (base unchanged)
                } else {
                    self.levels[level].push_back(task);
                }
            }
        }
        into_band
    }
}

/// Per-CPU ready queues. `RunQueue::new` is const, so no lazy_static; a `push` may allocate, but only
/// at `spawn` / an aging relocate (never under the switch), so the brief lock is realloc-free on the
/// switch hot path.
static RUN_QUEUES: [SpinMutex<RunQueue>; NUM_CPUS] =
    [const { SpinMutex::new(RunQueue::new()) }; NUM_CPUS];

/// Per-CPU sleeper lists: tasks blocked in `sleep_ticks`, tagged with their wake deadline (this
/// CPU's `percpu.ticks`). Touched ONLY by the scheduler on the OWNING CPU (parked there on the
/// switch-back, drained at the loop top), so the lock is always uncontended — it exists solely to
/// make the field interior-mutable, not for cross-CPU synchronisation. Being single-CPU, a
/// `push_back` that reallocates only ever nests the (innermost) heap lock, never another sched lock.
static SLEEPERS: [SpinMutex<VecDeque<Sleeper>>; NUM_CPUS] =
    [const { SpinMutex::new(VecDeque::new()) }; NUM_CPUS];

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

// --- WEDGE-4 — the run-queue lock discipline, and the instruments that witness a breach. ---

/// WEDGE-4 — which run-queue section (if any) the core at this index is currently inside.
/// `0` = none; otherwise `((queue + 1) << 32) | (owner tid as u32)`.
///
/// Two consumers, both diagnostic: `timer_preempt` reads THIS core's word to catch a preempt landing
/// inside a section (probe W4-A — the precondition for the wedge, which the masking below makes
/// impossible, so a line from it means the discipline has been breached again), and `wedge4_rq_stall`
/// scans all cores to name the holder of a queue it could not acquire (probe W4-B). Sections never
/// nest — every acquisition below takes exactly one queue and drops it before taking another — so a
/// single word per core is enough.
static IN_RQ_SECTION: [AtomicU64; NUM_CPUS] = [const { AtomicU64::new(0) }; NUM_CPUS];

/// WEDGE-4 W4-B — try_lock attempts before a run-queue acquisition is declared stalled. Same order as
/// WEDGE-1's `DRAIN_STALL_SPINS`: far beyond any legitimate hold (push/pop are O(NUM_PRIORITIES), the
/// aging sweep O(ready tasks)), so reaching it means the holder is off-CPU and never coming back.
const RQ_STALL_SPINS: u64 = 1 << 26;

/// WEDGE-4 W4-A — cap on the preempt-in-section witness, so a breach reports itself without flooding.
const W4A_PRINT_MAX: u32 = 8;
static W4A_PRINTS: AtomicU32 = AtomicU32::new(0);

type RqLockGuard = spin::MutexGuard<'static, RunQueue, spin::Spin>;

/// WEDGE-4 — one raw byte at the UART, taking NO lock. Same seam as WEDGE-2's breadcrumbs
/// (`crate::arch::serial::wedge2_raw_byte` is this call): a bounded volatile poll of the PL011 TX-full
/// bit and one volatile store. `serial_println!` cannot be used here — it masks IRQ and takes the
/// serial lock, and this instrument exists precisely for a core that is spinning IRQ-masked forever.
#[inline(never)]
fn w4_str(s: &str) {
    for b in s.as_bytes() {
        super::serial::SerialPort.write_byte(*b);
    }
}

/// WEDGE-4 — a decimal integer through the same lock-free seam (no formatter, no allocation).
#[inline(never)]
fn w4_dec(v: u64) {
    let mut buf = [0u8; 20];
    let mut n = 0;
    let mut v = v;
    loop {
        buf[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    while n > 0 {
        n -= 1;
        super::serial::SerialPort.write_byte(buf[n]);
    }
}

/// WEDGE-4 W4-B — the wedge namer: one line identifying the queue that could not be acquired and, if
/// some core is inside a section on it, that section's owner. Emitted ONCE per stalled acquisition
/// (at exactly `RQ_STALL_SPINS`); the caller then keeps spinning, so behaviour is unchanged and this
/// only makes a silent wedge legible.
#[inline(never)]
fn wedge4_rq_stall(core: usize, queue: usize) {
    w4_str("\r\n[wedge4] RQ STALL core=");
    w4_dec(core as u64);
    w4_str(" queue=");
    w4_dec(queue as u64);
    for c in 0..NUM_CPUS {
        let s = IN_RQ_SECTION[c].load(Ordering::Acquire);
        if s != 0 && (s >> 32) == queue as u64 + 1 {
            w4_str(" owner_core=");
            w4_dec(c as u64);
            w4_str(" owner_tid=");
            w4_dec(s & 0xffff_ffff);
            break;
        }
    }
    w4_str("\r\n");
}

/// WEDGE-4 W4-B — acquire `queue`'s lock, spinning as before, but bounded well enough to say so once.
#[inline]
fn rq_lock_witnessed(queue: usize, core: usize) -> RqLockGuard {
    let mut spins: u64 = 0;
    loop {
        if let Some(guard) = RUN_QUEUES[queue].try_lock() {
            return guard;
        }
        spins = spins.wrapping_add(1);
        if spins == RQ_STALL_SPINS {
            wedge4_rq_stall(core, queue);
        }
        core::hint::spin_loop();
    }
}

/// WEDGE-4 — the tid dispatched on `cpu` right now, or 0 outside a scheduled task. Owner attribution
/// for the probes only; relaxed, and the `&'static`-lifetime Box is live for as long as it is
/// `current`.
#[inline]
fn current_tid_relaxed(cpu: usize) -> u64 {
    let raw = SCHED[cpu].current.load(Ordering::Relaxed) as *const Task;
    if raw.is_null() { 0 } else { unsafe { (*raw).id } }
}

/// WEDGE-4 — a held run-queue lock together with the IRQ state it masked. See [`rq`].
struct RqGuard {
    daif: u64,
    core: usize,
    guard: Option<RqLockGuard>,
}

impl Drop for RqGuard {
    fn drop(&mut self) {
        self.guard = None; // release the spinlock FIRST
        IN_RQ_SECTION[self.core].store(0, Ordering::Release);
        irq_restore(self.daif); // then restore the caller's IRQ state (nested masks stay masked)
    }
}

impl Deref for RqGuard {
    type Target = RunQueue;
    fn deref(&self) -> &RunQueue {
        self.guard.as_ref().expect("rq: guard released while borrowed")
    }
}

impl DerefMut for RqGuard {
    fn deref_mut(&mut self) -> &mut RunQueue {
        self.guard.as_mut().expect("rq: guard released while borrowed")
    }
}

/// WEDGE-4 — take `queue`'s run-queue lock with IRQ MASKED for exactly the length of the hold. This is
/// the only admissible way to acquire `RUN_QUEUES`.
///
/// `RUN_QUEUES` is a bare `spin::Mutex` with no interrupt discipline of its own, while the scheduler
/// side (`dispatch_next`, `make_ready`, `try_steal`) takes it IRQ-masked. Before this, the spawn and
/// placement paths took the same lock from ordinary preemptible task context: a timer preempt landing
/// inside one of those sections froze the holder, and every masked acquisition of that queue then span
/// forever — a 100%-busy core that dispatches nothing, panics nothing and prints nothing, cascading to
/// its siblings through `try_steal`'s all-queues peek. Masking closes the window; nothing else changes,
/// and no protection is weakened.
///
/// The hold is the whole span, so keep it as short as it already was (`RunQueue::push` may reallocate
/// under it — pre-existing, and the heap lock stays innermost).
#[inline]
fn rq(queue: usize) -> RqGuard {
    let daif = irq_save_mask();
    let core = percpu::this_cpu().cpu_index as usize;
    let guard = rq_lock_witnessed(queue, core);
    debug_assert_eq!(
        IN_RQ_SECTION[core].load(Ordering::Relaxed),
        0,
        "rq: run-queue sections must not nest"
    );
    IN_RQ_SECTION[core].store(
        ((queue as u64 + 1) << 32) | (current_tid_relaxed(core) & 0xffff_ffff),
        Ordering::Release,
    );
    RqGuard { daif, core, guard: Some(guard) }
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
    // Completion is signalled in `exit()` now (the single post point — see the note there), which covers
    // this kernel-thread return AND the EL0-thread `SYS_THREAD_EXIT` path (ELF-2), whose task erets to EL0
    // and never runs this trampoline tail.
    exit();
}

/// Placeholder `entry` for user tasks: `spawn_user` sets `Task.entry` to this, but `user_task_trampoline`
/// never calls it (it `eret`s to EL0 instead). Panics loudly if a path ever reaches it. EL0/user machinery
/// is baremetal-only (the `virt` JC3 path runs kernel-thread CAPSTONE, no EL0 — see the module gate).
#[cfg(feature = "baremetal")]
fn user_never(_: usize) {
    unreachable!("user task's kernel `entry` was called");
}

/// First code an EL0 (user) task runs at EL1, reached when `switch_context` `ret`s into its freshly
/// built frame (with I MASKED, from INITIAL_DAIF). The EL1 prologue stays I-masked (no preempt during
/// the drop); the eret plants an EL0 PSTATE with I UNMASKED (M6e: SPSR 0x240), so the generic timer
/// preempts the running EL0 task. That is sound because `__vec_irq` now banks SP_EL0 (M6e) — a timer
/// preempt of EL0 saves/restores the user stack pointer across the scheduler switch. (This is a
/// metal-only effect: QEMU raspi4b delivers no Group-1 timer IRQ, so EL0 there runs to its next
/// syscall/fault uninterrupted.) It reads the task's EL0 entry/SP, then drops to EL0.
///
/// The current SP_EL1 (this task's Box kernel stack) is retained across the `eret` to EL0 and becomes
/// the stack the later `svc`/IRQ re-entry (`__vec_svc`/`__vec_irq`) runs on, so the 16 KiB Box must
/// have headroom for the SVC/IRQ frame (256 GPR + 528 FP + 32 banked + the Rust handler) — it does;
/// this trampoline uses only a shallow prologue. The msr+eret are one `noreturn` asm block so nothing
/// runs after the drop. Baremetal-only (EL0/user machinery — the `virt` path has no EL0 this arc).
#[cfg(feature = "baremetal")]
extern "C" fn user_task_trampoline() -> ! {
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *const Task;
    debug_assert!(!raw.is_null(), "user_task_trampoline: current is null");
    let (entry, sp) = unsafe { ((*raw).user_entry, (*raw).user_sp) };
    // ELF-2: the one u64 argument the EL0 task starts with in x0 (the thread ABI's single arg). Regular EL0
    // tasks (`spawn_user`/`spawn_user_slot`) leave `arg` 0, so x0 stays 0 — byte-identical to the pre-ELF-2
    // scrub. A `spawn_user_thread` sets it. This is a DELIBERATE ABI value, not kernel residue: it is placed
    // in x0 AFTER the full GPR/FP scrub below, so the scrub's no-leak property is unweakened (every other
    // register is still zeroed; x0 alone carries the intended argument, exactly as a syscall ABI arg would).
    let arg = unsafe { (*raw).arg as u64 };
    // SPSR_EL1 = 0x240 (M6e): M[3:0]=0b0000 (EL0t — a dedicated SP_EL0; EL0h/0b0001 is an illegal
    // return from EL1 -> PSTATE.IL), M[4]=0 (AArch64), DAIF = D,F masked with A (SError) and I (IRQ)
    // CLEAR. I unmasked => the generic timer preempts a running EL0 task (M6e; safe now that
    // `__vec_irq` banks SP_EL0). A stays clear (baremetal SError policy: an EL0-provoked external
    // abort is delivered promptly at the Lower-EL SError vector, not held pending into kernel
    // context). Was 0x2C0 (I masked, non-preemptible) through M6a–M6c.
    //
    // M6d hardening — GPR scrub at the FIRST eret to EL0. Zero x0-x30 after the three msr consume their
    // inputs, so no live kernel value (the raw `Task` pointer, kernel x29/x30, the entry/SP/SPSR just
    // loaded, ...) is architecturally readable at EL0 on entry. The three inputs are PINNED to x0/x1/x2
    // and already written to their system registers before the scrub, so zeroing x0-x30 is safe. This is
    // the aarch64 twin of the x86 SYSRET GPR scrub; the preempt-RESUME path is already clean (it restores
    // from the task's own saved frame in `__vec_irq`, not through this trampoline).
    //
    // M6f hardening (Part 0) — FP/SIMD + thread-pointer scrub, immediately after the GPR scrub and before
    // the eret. `CPACR_EL1.FPEN=0b11` (drop_to_el1) makes the WHOLE v0-v31/FPSR/FPCR file EL0-readable, and
    // the `+neon` kernel autovectorizes (memcpy/fmt/GUI blits) so it leaves live kernel data in the vector
    // file; the GPR scrub alone is not the no-leak property the ledger wanted, so zero v0-v31 (`movi
    // vN.2d,#0` needs no GPR) and reset FPSR/FPCR. Also zero-init TPIDR_EL0/TPIDRRO_EL0 (EL0-readable thread
    // pointers, firmware-residue UNKNOWN; the kernel uses TPIDR_EL2 for per-CPU, never these). All use only
    // `movi`/xzr, so they run after x0-x30 are already zeroed. FP is enabled at EL1 here (the GUI does NEON),
    // so `movi` executes without trapping. Like the GPR scrub this covers only FIRST entry; the preempt-
    // RESUME path restores the task's own saved v0-v31/FPSR/FPCR from its `__vec_irq` frame.
    unsafe {
        core::arch::asm!(
            "msr SP_EL0, x0",
            "msr ELR_EL1, x1",
            "msr SPSR_EL1, x2",
            "mov x0, xzr",  "mov x1, xzr",  "mov x2, xzr",  "mov x3, xzr",
            "mov x4, xzr",  "mov x5, xzr",  "mov x6, xzr",  "mov x7, xzr",
            "mov x8, xzr",  /* x9 holds the ELF-2 thread arg; scrubbed at the tail */ "mov x10, xzr", "mov x11, xzr",
            "mov x12, xzr", "mov x13, xzr", "mov x14, xzr", "mov x15, xzr",
            "mov x16, xzr", "mov x17, xzr", "mov x18, xzr", "mov x19, xzr",
            "mov x20, xzr", "mov x21, xzr", "mov x22, xzr", "mov x23, xzr",
            "mov x24, xzr", "mov x25, xzr", "mov x26, xzr", "mov x27, xzr",
            "mov x28, xzr", "mov x29, xzr", "mov x30, xzr",
            // FP/SIMD file: zero all 32 vector registers (each `.2d,#0` clears both 64-bit lanes).
            "movi v0.2d, #0",  "movi v1.2d, #0",  "movi v2.2d, #0",  "movi v3.2d, #0",
            "movi v4.2d, #0",  "movi v5.2d, #0",  "movi v6.2d, #0",  "movi v7.2d, #0",
            "movi v8.2d, #0",  "movi v9.2d, #0",  "movi v10.2d, #0", "movi v11.2d, #0",
            "movi v12.2d, #0", "movi v13.2d, #0", "movi v14.2d, #0", "movi v15.2d, #0",
            "movi v16.2d, #0", "movi v17.2d, #0", "movi v18.2d, #0", "movi v19.2d, #0",
            "movi v20.2d, #0", "movi v21.2d, #0", "movi v22.2d, #0", "movi v23.2d, #0",
            "movi v24.2d, #0", "movi v25.2d, #0", "movi v26.2d, #0", "movi v27.2d, #0",
            "movi v28.2d, #0", "movi v29.2d, #0", "movi v30.2d, #0", "movi v31.2d, #0",
            "msr FPSR, xzr",        // clear cumulative FP exception flags
            "msr FPCR, xzr",        // default control (round-to-nearest, no FTZ, no traps)
            "msr TPIDR_EL0, xzr",   // EL0 RW thread pointer — no kernel residue reaches EL0
            "msr TPIDRRO_EL0, xzr", // EL0 RO thread pointer (EL1-writable)
            // ELF-2: plant the thread argument in x0 AFTER the scrub (deliberate ABI value), then scrub the
            // scratch x9 that carried it. For a 0-arg task this writes x0=0 — identical to the scrub result.
            "mov x0, x9",
            "mov x9, xzr",
            "isb",
            "eret",
            in("x0") sp,
            in("x1") entry,
            in("x2") 0x240u64,
            in("x9") arg,
            options(noreturn, nostack),
        );
    }
}

/// Build a fresh task's initial stack frame so the first `switch_context` into it lands in
/// `trampoline` (`task_trampoline` for kernel threads, `user_task_trampoline` for EL0 tasks). Returns
/// the value to store in `ctx_sp`. The 176-byte frame (matching `switch_context`) sits below a
/// 16-aligned top; x30's slot holds the trampoline, DAIF's slot holds IRQ-masked, the rest (x19-x29,
/// pad, d8-d15) zero.
fn build_initial_frame(stack: &mut [u8], trampoline: extern "C" fn() -> !) -> u64 {
    let base = stack.as_mut_ptr() as usize;
    let top = (base + stack.len()) & !0xF;
    let sp = top - 176;
    unsafe {
        let p = sp as *mut u64;
        for i in 0..22 {
            p.add(i).write(0); // x19..x29, pad, d8..d15
        }
        p.add(11).write(trampoline as usize as u64); // x30 (lr) slot -> ret lands in the trampoline
        p.add(12).write(INITIAL_DAIF); // DAIF slot (offset 96)
    }
    sp as u64
}

// ---------------------------------------------------------------------------------------------
// SCHED-3 — load-balanced placement for UNPINNED spawns
// ---------------------------------------------------------------------------------------------
//
// The scheduler is no-migrate: a task lives on the core its `Task.cpu` names, decided once at spawn.
// Historically EVERY caller named that core explicitly, so placement was 100% caller-pinned and hot
// services that all pass a hand-picked "off the render core" pin cluster on the SAME core (the metal
// R23s1f/R23s1h sightings: net9 + orphan-reaper both land on core 2 -> c2 saturates while core 1 sits
// near-idle). SCHED-3 keeps explicit pins verbatim (render/input MUST stay single-core) but adds an
// opt-in placement: a caller that passes `CPU_AUTO` lets the scheduler drop the task on the LEAST-
// loaded online core, so unpinned work spreads by actual load instead of piling onto one pin.
//
// The candidate set is the cores that are actually running the scheduler loop, registered via
// `mark_online` when the BSP releases the APs (`start_aps`). The load metric is the ready-queue DEPTH
// (the most direct "will this task wait" signal), tie-broken by the rolling-window busy fraction and
// then a rotating cursor so equal-load cores fill round-robin rather than all landing on the lowest
// index. Placement runs off the hot path (spawn only), so the brief per-queue depth read is free.

/// Sentinel `cpu` for `spawn`/`spawn_prio`/`spawn_auto`: "don't pin me — place me on the least-loaded
/// online core by actual load". Any real core index (`< NUM_CPUS`) is honored verbatim (no-migrate).
pub const CPU_AUTO: usize = usize::MAX;

/// Cores registered as online + scheduling (the `CPU_AUTO` placement candidate set). Set by
/// `mark_online` when the BSP releases an AP into `run`; never cleared (a core stays a candidate).
static ONLINE_MASK: [AtomicBool; NUM_CPUS] = [const { AtomicBool::new(false) }; NUM_CPUS];

/// Round-robin cursor for `CPU_AUTO` tie-breaking: when several online cores share the minimum load,
/// the scan starts at a rotating offset so consecutive auto-placements fan out instead of stacking.
static AUTO_ROTATE: AtomicUsize = AtomicUsize::new(0);

// SPREAD-3 — COMMITTED LOAD, NOT AN INSTANTANEOUS SNAPSHOT.
//
// SCHED-3's two placement signals are both blind to the thing EL0 spawns actually create: a long-lived,
// compute-bound resident that is RUNNING rather than QUEUED.
//
//   * ready-queue DEPTH (`RunQueue::len`) counts tasks WAITING in the queue. The task a core is
//     currently executing lives in `SCHED[cpu].current`, NOT in `RUN_QUEUES[cpu]` — so a core spinning
//     flat-out inside one compute-bound vug reads depth 0, exactly like a genuinely idle core.
//   * the rolling busy fraction (`CoreAccount::busy_pct`) is a ~250 ms LAGGING window. A burst of
//     spawns issued inside one window all read the SAME pre-burst percentage, so they all agree on the
//     same "least busy" core and all land on it. (PULSE-5 has since removed the WORST of that lag —
//     `busy_pct` now ages the in-flight execution span in at read time, so a core running one
//     compute-bound task no longer reports its pre-storm percent indefinitely. It is still a rolling
//     window and still cannot see a spawn that has not started executing, which is precisely why the
//     committed-residents key below remains key 1 and is NOT superseded by the fresher percent.)
//
// Together those produce the P68 measurement (27 bg-el0 -> c3, 18 -> c0, 8 -> c1, ~0 -> c2 while
// c0/c3 sat at 99% and c1/c2 at ~80%): placement keeps re-reading a signal that has not yet caught up
// with the placements it already made, and because the scheduler is no-migrate, nothing ever corrects
// it. Operator-visible as stagger inversion — vugs launched early run slower than their replacements.
//
// The fix is to make the placement decision account for what has ALREADY been committed to a core:
// a per-core count of LIVE EL0 residents, incremented at the moment of placement (before the enqueue,
// so it is visible to the very next spawn) and decremented when the task is reaped. It is O(1), needs
// no new Task field (an EL0 task is exactly one with a non-zero `user_entry`), carries no migration
// machinery, and — unlike depth and busy_pct — it cannot lag the decisions it is meant to inform.

/// SPREAD-3 — live EL0 residents committed to each core. Bumped by every EL0 spawn path
/// (`spawn_user_inner`, `spawn_user_thread`) at placement time and dropped on every reap path
/// (`exit`, `retire_killed`). This is the COMMITTED-load signal `pick_cpu` reads first: unlike
/// ready-queue depth it counts a task that is currently RUNNING, and unlike the rolling busy window
/// it updates synchronously with the placement rather than ~250 ms later.
/// SPIN-3 (2026-07-30, the P96 exoneration cascade): the BCM2711's A72 cores have NO LSE atomics —
/// every RMW is an LL/SC retry loop, and an exclusive reservation broken by another core's store to
/// the SAME CACHE LINE retries forever under sustained contention. The per-cpu accounting atomics
/// were adjacent (4-8 to a 64-byte line): the yield storm hammering its own counters at MHz rates
/// starved rx-backstop's `make_ready` fetch_add on a NEIGHBORING counter for 20-200 s, IRQ-masked —
/// with every lock witness reading clean, because the locks live on other lines. One padded slot per
/// core ends the false sharing; the reservation granule is the line.
#[repr(align(64))]
struct PaddedUsize(AtomicUsize);
#[repr(align(64))]
struct PaddedSlotRow([AtomicU32; KILL_ASID_SLOTS]);

static EL0_RESIDENTS: [PaddedUsize; NUM_CPUS] =
    [const { PaddedUsize(AtomicUsize::new(0)) }; NUM_CPUS];

/// SPREAD-3 — commit one EL0 resident to `cpu`. Called BEFORE the run-queue push so a concurrent
/// `pick_cpu` on another core can never place a second resident against a stale count. Returns the
/// new (inclusive) resident count, which the placement witness prints.
///
/// `allow(dead_code)`: the only callers are the two EL0 spawn paths, which are `baremetal`-gated (the
/// `virt`/JC3 aarch64 build runs kernel threads only and creates no EL0 task). The RELEASE side stays
/// ungated because `exit()` is shared by both worlds — it simply never fires there (`user_entry == 0`).
#[allow(dead_code)]
#[inline]
fn el0_resident_enter(cpu: usize) -> usize {
    if cpu >= NUM_CPUS {
        return 0;
    }
    EL0_RESIDENTS[cpu].0.fetch_add(1, Ordering::AcqRel) + 1
}

/// SPREAD-3 — release one EL0 resident from `cpu` (task reaped). Saturating at zero: an accounting
/// slip must never underflow into `usize::MAX` and permanently exclude a healthy core from placement.
/// EL0 tasks are `steal_ok = false` (never migrated), so the `cpu` recorded at spawn is still the
/// core being released here.
#[inline]
fn el0_resident_leave(cpu: usize) {
    if cpu >= NUM_CPUS {
        return;
    }
    let _ = EL0_RESIDENTS[cpu].0.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
        if n == 0 { None } else { Some(n - 1) }
    });
}

// SPREAD-4 — A COMMITTED RESIDENT IS NOT THE SAME THING AS A RUNNABLE ONE.
//
// SPREAD-3's residents key fixed the burst problem (N spawns in one window no longer agree on one
// core), but it counts HEADS, not LOAD: a vug spinning flat out and a vug parked on its input futex
// weigh exactly the same. On a live fleet that is the dominant error, because a windowed EL0 app
// spends most of its life PARKED — VUGPAUSE-2 made the idle vug block on `SYS_INPUT_WAIT` and its
// workers block on the phase futex, so a four-vug desktop with one active window reads
// `residents = 4` spread over the cores that happened to be least loaded at spawn time, and every
// later placement steers around load that is not there. The P73 wire is the shape that produces:
// per-core busy swinging 3-5x while the residents counts stay flat and even.
//
// The second half is that the count is only ever consulted at SPAWN. EL0 tasks are `steal_ok = false`
// (they carry per-core address-space state), so `try_steal` can never correct them — and neither can
// anything else, because `make_ready` returns a woken task to `task.cpu` unconditionally. A vug that
// was placed on c1 when c1 was idle re-runs on c1 forever, however crowded c1 later becomes. That is
// the "parked-then-resumed vug re-runs on its original core regardless of current load" half, and it
// is also the sched half of the vug speed-up delay: when its peers park, the surviving vug's own
// worker threads stay bunched on the cores they were spawned onto and the fleet-of-one never spreads.
//
// SPREAD-4 fixes both with one counter and one decision point:
//
//   1. LIVE RESIDENTS. `EL0_PARKED` tracks how many of a core's committed residents are currently
//      BLOCKED. `el0_active` = committed - parked is the RUNNABLE resident count, and that is what
//      `pick_cpu` keys on. A parked vug stops anchoring load to its core the moment it blocks and
//      starts counting again the moment it wakes. Nothing else about SPREAD-3's accounting moves:
//      `EL0_RESIDENTS` keeps its exact enter/leave sites and its exact meaning.
//   2. RE-PLACE AT WAKE. `make_ready` is the single funnel every wake goes through (sleeper drain,
//      `Semaphore::post`, `futex_wake`, the kill sweeps), and a task arriving there is parked — it
//      owns no core, holds no per-core register state, and is about to be pushed onto SOME queue.
//      That is the one instant at which an EL0 task can be moved for free, so that is where the
//      committed-load signal gets a second look. See `rewake_place`.

/// SPREAD-4 — how many of each core's committed EL0 residents are currently PARKED (blocked in a wait
/// queue or on the sleeper list). Incremented by `park_blocked` and decremented by `make_ready`, which
/// are the sole park/wake funnels: every blocking primitive an EL0 task can reach (`Semaphore::wait`,
/// `futex_wait`, `sleep_ticks`) parks by setting `park_kind` and switching back, and every wake path
/// re-readies through `make_ready`. The pair is therefore exactly balanced.
///
/// `retire_killed` needs no arm here: the task it reaps was just POPPED FROM A RUN QUEUE, so it was
/// READY (already un-parked by the `make_ready` that the kill sweep performed) and only its
/// `EL0_RESIDENTS` credit is outstanding. `exit()` likewise runs on a RUNNING task.
static EL0_PARKED: [PaddedUsize; NUM_CPUS] =
    [const { PaddedUsize(AtomicUsize::new(0)) }; NUM_CPUS];

/// SPREAD-4 — note that a committed EL0 resident of `cpu` has gone to sleep.
#[inline]
fn el0_parked_enter(cpu: usize) {
    if cpu < NUM_CPUS {
        EL0_PARKED[cpu].0.fetch_add(1, Ordering::AcqRel);
    }
}

/// SPREAD-4 — note that a parked EL0 resident of `cpu` is runnable again. Saturating at zero for the
/// same reason `el0_resident_leave` is: an accounting slip must never underflow into `usize::MAX` and
/// make a busy core look permanently empty (the failure mode here is the DANGEROUS direction —
/// `el0_active` would saturate to 0 and the core would attract every placement on the machine).
#[inline]
fn el0_parked_leave(cpu: usize) {
    if cpu >= NUM_CPUS {
        return;
    }
    let _ = EL0_PARKED[cpu].0.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
        if n == 0 { None } else { Some(n - 1) }
    });
}

/// SPREAD-4 — RUNNABLE EL0 residents on `cpu`: committed minus parked. This is the committed-load
/// signal `pick_cpu` keys on and the one `rewake_place` compares cores by. Saturating rather than
/// wrapping: the two counters are updated by different cores at different instants, so a reader can
/// legitimately observe `parked > residents` for a few cycles mid-wake (the parked decrement lands
/// before the resident transfer). Saturation makes that transient read 0 — one placement decision
/// made against a count that is one low — instead of a huge number that would exclude the core.
#[inline]
fn el0_active(cpu: usize) -> usize {
    if cpu >= NUM_CPUS {
        return 0;
    }
    EL0_RESIDENTS[cpu].0
        .load(Ordering::Acquire)
        .saturating_sub(EL0_PARKED[cpu].0.load(Ordering::Acquire))
}

// SPREAD-10 — A VUG'S TRIPLE LIVES TOGETHER; THE RENDEZVOUS COMES HOME.
//
// FLUID-3 closed the present-path hypothesis: presents run inline (~2.5 ms) on the caller's core,
// there is no queue and no consumer, and the aggregate rate is conserved. The remaining pace-setter
// for a vug's settled fps is the FUTEX PARK at its frame barrier — the parent parks on `DONE` behind
// its two workers (workers on `PHASE`), and when the three tasks of one vug sit scattered across
// saturated cores, every frame pays a cross-core rendezvous: wake SGI, queue wait behind foreign
// residents, then the parent's own re-dispatch. The [fluid3] millisecond park buckets ARE that price,
// per frame, and the wildly-unequal per-vug rates (19..80/s on the same binary) are a pure function
// of WHERE each triple landed. The fix is placement, not pacing: bias the members of one
// address-space slot toward the same core, so the rendezvous resolves against LOCAL wakes — the
// worker's `PHASE` wake and the parent's `DONE` wake land on a queue the target core is already
// dispatching, the spin-then-park windows catch them, and the cross-core IPI + foreign-queue wait
// drops out of the frame loop.
//
// The identification costs nothing new: a triple is exactly the tasks sharing one address-space slot
// (`user_ttbr0 >> 48` — the ASID `spawn_user_thread` propagates from the parent, the same key the
// PHASE futex hashes under). What placement lacked was a per-core view of it; `SLOT_CORE_RES` below
// is that view, maintained at the exact sites the SPREAD-3 committed-residents counter already
// occupies.
//
// The weight (the whole tuning argument, in one place): one cross-core rendezvous costs ~1-3 ms per
// frame ([fluid3] p50..p90 millisecond buckets under storm; [spread7] wd_mean prices one run-queue
// position at ~4-6 ms saturated). The co-residency bonus must therefore be worth LESS than one
// runnable resident (or triples would pile onto saturated cores and regress SPREAD-4's margin
// discipline) and MORE than the depth/pct tie-breaks (or nothing would ever converge). Half a
// runnable resident — ~2-3 ms-equivalent, squarely the price of the rendezvous it buys back — is the
// bonus, applied in doubled-load units wherever placement compares cores. Concretely:
//
//   * at SPAWN (`pick_cpu`): a core hosting a same-slot sibling wins every tie on runnable residents
//     but can never beat a core with one FEWER resident. Pure preference, zero pile-up risk.
//   * at REWAKE (`rewake_place`): a second qualifying lane beside the margin lane — a candidate
//     hosting MORE same-slot siblings than home qualifies at margin 0 (equal load allowed, heavier
//     never), so triples actually converge on a balanced fleet where the margin-2 lane would hold
//     every member exactly where it is. Sequential such moves strictly increase the slot's
//     co-residency (each move is toward strictly-more siblings), so convergence terminates; the
//     SPREAD-6 refresh clock (~4 asks/s per task, per-task and unsynchronized) is what carries a
//     shifted fleet to the new answer within ~250 ms.
//   * RETENTION: home hosting a sibling is half a resident harder to leave via the margin lane (on
//     the integer lattice that bites as one extra resident: a triple is broken up by pure load only
//     for a >= margin+1 win). A core that saturates still sheds: both lanes compare RUNNABLE load,
//     and a big enough delta clears the retention — co-residency is a preference, never an anchor.
//
// Siblings are counted COMMITTED (not runnable-adjusted): a parked parent still names the core its
// workers should rendezvous on — that is precisely the anchor this arc exists to create.

/// SPREAD-10 — committed EL0 residents per (core, address-space slot). Slot index is the ASID
/// (1..=`boot::USER_SLOTS`; index 0 — kernel tasks and the shared window — is never counted and never
/// biases). Enter/leave sites mirror `EL0_RESIDENTS` exactly: both EL0 spawn paths, the `make_ready`
/// move (transfer home -> target), and every reap path. Lock-free; same saturating-leave discipline.
static SLOT_CORE_RES: [PaddedSlotRow; NUM_CPUS] =
    [const { PaddedSlotRow([const { AtomicU32::new(0) }; KILL_ASID_SLOTS]) }; NUM_CPUS];

/// SPREAD-10 — placements the co-residency bonus DECIDED: rewake moves that qualified only through
/// the sibling lane, plus spawns whose winner differs from what the bonus-free key would have picked.
/// The `[spread10]` witness prints it; climbing without bound beside a flat `cores_per_slot`
/// histogram would be the thrash signature (the convergence argument above says sequential moves
/// terminate, so it should settle to the fleet's churn rate).
static SPREAD10_CO_MOVES: AtomicU64 = AtomicU64::new(0);

/// SPREAD-10 — the address-space slot of a task, or 0 for "no slot, no bias" (kernel tasks, the
/// shared window, and any out-of-range ASID). The same `>> 48` extraction every ASID consumer uses.
#[inline]
fn slot_of(user_ttbr0: u64) -> usize {
    let asid = (user_ttbr0 >> 48) as usize;
    if asid != 0 && asid < KILL_ASID_SLOTS { asid } else { 0 }
}

/// SPREAD-10 — commit one slot resident to `cpu`. Called beside every `el0_resident_enter`.
///
/// `allow(dead_code)`: `el0_resident_enter`'s reason verbatim — the only callers are the two EL0
/// spawn paths, which are `baremetal`-gated; the release side stays ungated because `exit()` is
/// shared by both worlds.
#[allow(dead_code)]
#[inline]
fn slot_res_enter(cpu: usize, user_ttbr0: u64) {
    let slot = slot_of(user_ttbr0);
    if slot != 0 && cpu < NUM_CPUS {
        SLOT_CORE_RES[cpu].0[slot].fetch_add(1, Ordering::AcqRel);
    }
}

/// SPREAD-10 — release one slot resident from `cpu`. Called beside every `el0_resident_leave`.
/// Saturating for `el0_resident_leave`'s reason: an accounting slip must never underflow and turn a
/// core into a permanent phantom sibling magnet.
#[inline]
fn slot_res_leave(cpu: usize, user_ttbr0: u64) {
    let slot = slot_of(user_ttbr0);
    if slot != 0 && cpu < NUM_CPUS {
        let _ = SLOT_CORE_RES[cpu].0[slot].fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            if n == 0 { None } else { Some(n - 1) }
        });
    }
}

/// SPREAD-10 — committed same-slot residents on `cpu` (0 for the no-slot sentinel).
#[inline]
fn slot_res(cpu: usize, slot: usize) -> u32 {
    if slot == 0 || cpu >= NUM_CPUS {
        return 0;
    }
    SLOT_CORE_RES[cpu].0[slot].load(Ordering::Acquire)
}

/// SPREAD-4 — how much less loaded another core must be before a waking EL0 task is moved onto it,
/// in RUNNABLE residents. Two, not one, and the gap is the whole stability argument: with a margin of
/// one, two cores carrying `n` and `n-1` would trade a task back and forth on every wake (each move
/// makes the destination the loaded one), and a windowed vug wakes on every frame. A margin of two
/// cannot oscillate — moving a task across a gap of two leaves the two cores at `n-1` and `n`, which
/// is a gap of one, which is below the threshold — so each imbalance is corrected at most once.
const REWAKE_MARGIN: usize = 2;

// SPREAD-5 — A PER-FRAME PARK IS NOT A BACKGROUND→FOREGROUND TRANSITION.
//
// SPREAD-4 re-placed an EL0 task on EVERY wake. That was right about WHERE a long-parked task should
// land and wrong about HOW OFTEN the question is worth asking, because in this fleet parking is not a
// rare event: VUGPAUSE-2 parks the idle vug on `SYS_INPUT_WAIT` and VUG-PACE parks each worker on the
// phase futex after 64 spin passes, so every vug task in the fleet parks and wakes ONCE PER FRAME. The
// P75 metal wire is what that costs: `rewake=3256 and climbing` on a six-vug fleet — thousands of
// placement decisions per minute, each one a potential migration, against a load signal that swings
// within a frame. The measured symptoms are migration churn, not imbalance: per-window rates that
// diverge 5x (win5 125/s, win1 23/s, win2/3 frozen), a stuttery mouse under the full fleet, and the
// paradox that the fifth and sixth vug beat a lone vug's frame rate.
//
// The fix is to separate the two shapes of park that share one funnel:
//
//   * a MICRO-PARK — the frame-loop park. The task is coming back within a frame, onto a core whose
//     load has not meaningfully changed, and it still owns its warm caches there. Nothing about the
//     placement question has moved since the last time it was asked. Return it to `task.cpu`, which is
//     exactly the pre-SPREAD-4 behaviour, and the one that was correct for this case all along.
//   * a REAL TRANSITION — a window that was idle/background for a human interval and is now getting
//     work again. Its core assignment IS stale, the load has genuinely moved, and the cache footprint
//     is cold anyway, so the move is close to free. This is the case SPREAD-4 was built for, and it
//     keeps SPREAD-4's machinery unchanged: the margin-2 threshold and the WEDGE-1 freshness guard.
//
// The discriminator is how long the task was parked, and nothing else — no notion of focus, no window
// identity, no new coupling from the scheduler up into the compositor.

/// SPREAD-5 — how long a task must have been parked for its wake to count as a real background→
/// foreground transition rather than a frame-loop micro-park, in milliseconds.
///
/// 100 ms sits in a wide gap between the two populations, which is why the number is not delicate:
///
///   * a 60 fps frame is 16.7 ms and a 30 fps frame is 33 ms, so a frame-loop park is at most a few
///     tens of milliseconds. 100 ms is THREE 30 fps frames — a task that parks and wakes on the frame
///     clock cannot reach it even if the fleet drops to 10 fps under load.
///   * VUGPAUSE-2's backstop wake is 256 ms, so a genuinely idle window — one parked with no input and
///     no frame to draw — crosses 100 ms on its very first backstop period and every one after. The
///     long-park path stays reachable for exactly the population it is meant to serve.
///
/// The gap is ~3x on the near side and ~2.5x on the far side, so neither population lands near the
/// boundary. Erring low costs some churn back; erring high costs a slower correction after a focus
/// change (one extra backstop period). 100 ms is comfortably inside both margins.
const REWAKE_MIN_PARK_MS: u64 = 100;

/// SPREAD-5 — [`REWAKE_MIN_PARK_MS`] in CNTPCT cycles. Frequency-derived like `load_window_cyc` (which
/// is `CNTFRQ/4`), so the threshold is the same wall-clock span on the BCM2711's ~54 MHz counter and on
/// QEMU virt's ~62.5 MHz one. `.max(1)` so a nonsense CNTFRQ can never make the threshold zero, which
/// would silently restore SPREAD-4's re-place-on-every-wake behaviour.
// SPREAD-6 (VUG-PACE-2) — THE PLACEMENT LATCH, AND ITS ESCAPEMENT.
//
// SPREAD-5's damping was right about churn and silently wrong about one population: a task that never
// stops. A frame-paced vug parks and wakes every frame, each park tens of milliseconds, so under
// SPREAD-5 the placement question is never asked again for as long as the vug keeps rendering — its
// core assignment is frozen at whatever the LAST long-park wake (or the spawn) decided, under whatever
// load existed at that instant. When the surrounding fleet then pauses or exits, the survivor keeps the
// contention-era packing forever. The s1q wire is the measurement: win1 held 30.7-30.9/s across ten
// straight rollups — two runnable EL0 tasks time-sharing c2 at 99% busy while three cores sat idle —
// with [spread4] rewake frozen at 26 and short= climbing by hundreds per window. A stable rate that is
// a pure function of stale packing is exactly Peter's "vug wants to go back to what it thinks its fps
// is supposed to be even though it could run faster": the fps was never a target, it was a latch.
//
// The escapement: a micro-park wake MAY still ask the placement question, at most once per
// `PLACE_REFRESH_MS` per task. That bounds the asking rate at ~4/s per task (a six-vug fleet is ~72
// asks/s of lock-free arithmetic, against SPREAD-4's measured ~540 placement calls/s), and asking is
// not moving: `rewake_place`'s margin-2 threshold and freshness gate still decide, so a balanced fleet
// answers "stay" and the counters show it. What changes is only that a pile-up now comes apart within
// a quarter second of the load leaving, instead of never.
const PLACE_REFRESH_MS: u64 = 250;

/// SPREAD-6 — [`PLACE_REFRESH_MS`] in CNTPCT cycles, frequency-derived exactly like
/// [`rewake_min_park_cyc`] and for the same reason.
#[inline]
fn place_refresh_cyc() -> u64 {
    let frq = load_window_cyc().saturating_mul(4); // == CNTFRQ_EL0, cached
    (frq / 1000).saturating_mul(PLACE_REFRESH_MS).max(1)
}

#[inline]
fn rewake_min_park_cyc() -> u64 {
    let frq = load_window_cyc().saturating_mul(4); // == CNTFRQ_EL0, cached
    (frq / 1000).saturating_mul(REWAKE_MIN_PARK_MS).max(1)
}

/// SPREAD-4 — choose the core a waking EL0 task should run on, given the core it was parked from.
/// Returns `home` unless some other online core is at least `REWAKE_MARGIN` runnable residents
/// lighter, in which case it returns the lightest such core (rolling busy fraction breaking ties —
/// PULSE-5 made that number current rather than a lagging window, which is what makes it usable on a
/// decision path at all).
///
/// Called with the waking task ALREADY un-parked from `home`'s parked count, so `home`'s figure is
/// the load the task would be joining, not including itself — the comparison is apples to apples.
///
/// FRESHNESS GATE (WEDGE-1's, for WEDGE-1's reason). An EL0 task is `steal_ok = false`: whichever
/// queue it lands on is the only core that will ever run it, so handing it to a core that has stopped
/// going round its dispatch loop parks it forever. A candidate must therefore have folded a load span
/// within `dispatch_fresh_cyc()`. This is deliberately conservative in the safe direction — declining
/// every candidate simply leaves the task at home, which is the pre-SPREAD-4 behaviour.
///
/// Lock-free by construction (atomics and one sysreg read; no run-queue lock), which is required:
/// `make_ready` calls this INSIDE the IRQ-masked wake path and the `rq()` discipline forbids nesting
/// a second run-queue section under the push that follows.
///
/// SPREAD-10 — `slot` (0 = no bias) adds the co-residency term. Loads are compared in DOUBLED units
/// (`2*act + 1`, the +1 a uniform shift so the half-resident bonus never underflows at act 0): a core
/// hosting more same-slot siblings than home is half a resident lighter than it reads, home hosting a
/// sibling is half a resident harder to leave, and a SECOND qualifying lane admits a sibling-bound
/// move at margin 0 (candidate hosts strictly MORE siblings AND is no more loaded than home) — the
/// lane that lets a scattered triple converge on a balanced fleet, where the margin lane would never
/// move anyone. Rationale + weight tuning: the SPREAD-10 block above `SLOT_CORE_RES`.
fn rewake_place(home: usize, slot: usize) -> usize {
    if home >= NUM_CPUS {
        return home;
    }
    let home_act = el0_active(home);
    // SPREAD-10 — home's committed same-slot siblings, EXCLUDING the waking task itself (it is still
    // committed at home; the transfer happens after the decision).
    let home_sibs = slot_res(home, slot).saturating_sub(1);
    // SPREAD-4's cheap early-out, kept — unless the task is slot-scattered (home hosts none of its
    // siblings), in which case the sibling lane must still get its scan: at low load a sibling-bound
    // move is exactly the cheap, convergence-carrying case.
    if home_act < REWAKE_MARGIN && (slot == 0 || home_sibs > 0) {
        return home; // home is not carrying enough to be worth correcting
    }
    // Doubled-unit effective load of home: the half-resident retention bonus when staying preserves
    // an existing co-residency.
    let home_eff = 2 * home_act + 1 - usize::from(home_sibs > 0);
    let fresh = dispatch_fresh_cyc();
    let mut best = home;
    let mut best_eff = home_eff;
    let mut best_sibs = home_sibs;
    let mut best_pct = u32::MAX;
    let mut best_sib_lane_only = false; // did the winner qualify ONLY via the sibling lane?
    for cpu in 0..NUM_CPUS {
        if cpu == home || !ONLINE_MASK[cpu].load(Ordering::Acquire) {
            continue;
        }
        let act = el0_active(cpu);
        let sibs = if slot != 0 { slot_res(cpu, slot) } else { 0 };
        let toward = sibs > home_sibs; // moving here strictly increases the triple's co-residency
        let eff = 2 * act + 1 - usize::from(toward);
        // Lane 1 (SPREAD-4, unchanged for slotless tasks): enough of a load win to pay for the move.
        let margin_lane = eff + 2 * REWAKE_MARGIN <= home_eff;
        // Lane 2 (SPREAD-10): sibling-bound, margin 0 — equal load allowed, heavier never.
        let sib_lane = toward && act <= home_act;
        if !margin_lane && !sib_lane {
            continue; // not enough of a win to pay for the move
        }
        if ACCT[cpu].fold_age_cyc() >= fresh {
            continue; // not provably dispatching — never hand it a task only it can run
        }
        // SPREAD-9: the tie-break percent excludes service-band time, exactly as in `pick_cpu` — a
        // waking task must not decline the core the service band lives on for load that would
        // preempt-and-vanish rather than compete (the rewake half of the dissolved reserve).
        let pct = ACCT[cpu].el0_busy_pct(CUR_PRIO[cpu].load(Ordering::Relaxed) >= PRIO_SERVICE);
        // Choose by effective load; ties fall to MORE siblings (all members of a slot then rank the
        // same target core, which is what keeps concurrent asks pointing one way), then lower pct.
        if eff < best_eff
            || (eff == best_eff && (sibs > best_sibs || (sibs == best_sibs && pct < best_pct)))
        {
            best = cpu;
            best_eff = eff;
            best_sibs = sibs;
            best_pct = pct;
            best_sib_lane_only = sib_lane && !margin_lane;
        }
    }
    // SPREAD-10 — a move only the sibling lane admitted is a placement the bonus decided.
    if best != home && best_sib_lane_only {
        SPREAD10_CO_MOVES.fetch_add(1, Ordering::Relaxed);
    }
    best
}

/// SPREAD-4 — wakes that moved an EL0 task to a lighter core, and wakes that left it where it was.
/// The ratio is the arc's own honesty check: a fleet in balance should be nearly all `stay`.
///
/// SPREAD-5 narrows the population both counters describe: they now count only wakes that ASKED the
/// placement question (parked longer than [`REWAKE_MIN_PARK_MS`]). `rewake` should therefore climb by
/// roughly one per real focus change rather than once per frame per task.
static SPREAD4_REWAKE: AtomicU64 = AtomicU64::new(0);
/// SPREAD-11 — yield-path slot re-placements (the P94 idle-desktop livelock fix). Counts moves made
/// at the READY re-enqueue (yield/preempt) refresh, the path that never passes `make_ready` and so
/// never saw SPREAD-10's co-placement at all.
static SPREAD11_YIELD_MOVES: AtomicU64 = AtomicU64::new(0);
static SPREAD4_STAY: AtomicU64 = AtomicU64::new(0);

/// SPREAD-5 — EL0 wakes that skipped placement entirely because the park was a frame-loop micro-park.
/// This is the damping made visible: it is the count of `rewake_place` calls SPREAD-4 would have made
/// and SPREAD-5 does not. On a running fleet it should dwarf `rewake` + `stay` by orders of magnitude;
/// if it does not, the fleet is not parking per frame and this arc's premise needs re-checking.
static SPREAD5_SHORT_STAY: AtomicU64 = AtomicU64::new(0);

/// SPREAD-6 — micro-park wakes that asked the placement question anyway because the last ask was more
/// than [`PLACE_REFRESH_MS`] ago. Climbs at ~4/s per continuously-running EL0 task; the OUTCOME of each
/// ask still lands in `rewake`/`stay`, so `refresh` large with `rewake` flat is a fleet that keeps
/// asking and keeps being told it is already in the right place — the escapement idling, as designed.
static SPREAD6_REFRESH: AtomicU64 = AtomicU64::new(0);

/// SPREAD-7 — EL0 wakes that landed in the TICK-QUANTIZED arm of the wake path: the woken task is an
/// equal-or-higher-band peer (below the service band) of the task running on its target core, so
/// nothing preempts for it — it waits in the run queue for the incumbent's next dispatch boundary,
/// up to a full quantum (~12 ms) before SPREAD-8's same-band trim (see `preempt_hint`'s SPREAD-7 and
/// SPREAD-8 sections). On a frame-barrier fleet this climbs at roughly the fleet's park rate;
/// `quant` flat while the fleet stutters means wakes are landing on idle cores or being absorbed by
/// the spin windows, and the ceiling is elsewhere.
static SPREAD7_QUANT: AtomicU64 = AtomicU64::new(0);

/// SPREAD-8 — equal-band EL0 wakes that TRIMMED the incumbent's quantum (the policy SPREAD-7's
/// diagnosis proposed, now implemented in `preempt_hint`'s same-band arm). Counted only when the
/// `swap(1)` actually LOWERED the countdown (previous value > 1) — a wake landing when the incumbent
/// was already on its final tick changed nothing and is not counted, so `trim <= quant` by
/// construction and the gap is the already-about-to-yield population. On a storm this climbs at
/// roughly the fleet's park rate, and `wd_mean` on the same line drops from half-quantum scale
/// (~6000 us) to at most one tick (~4000 us worst, less typically). The counter moves under QEMU
/// too (`dispatch_next` stores `QUANTUM_TICKS` on every dispatch, so the swap reads > 1), but with
/// no live timer IRQ the shortened countdown is never consumed — the latency effect is metal-only,
/// as with every preemption behaviour in this module.
static SPREAD8_TRIM: AtomicU64 = AtomicU64::new(0);

/// SPREAD-7 — wake-to-dispatch latency for EL0 wakes: CNTPCT cycles from `make_ready`'s stamp to the
/// `dispatch_next` that first runs the woken task, summed / counted / max'd. This prices the
/// quantization `SPREAD7_QUANT` counts: on an idle fleet `wd_mean` is IPI-scale (microseconds); on a
/// saturated one it converges on the half-quantum (~6 ms) plus queue depth. Cumulative, lock-free,
/// owner-core writes only for SUM/N (dispatch), MAX via `fetch_max`.
static SPREAD7_WD_SUM: AtomicU64 = AtomicU64::new(0);
static SPREAD7_WD_N: AtomicU64 = AtomicU64::new(0);
static SPREAD7_WD_MAX: AtomicU64 = AtomicU64::new(0);

/// SPREAD-9 — the pending IPI-receipt preemption, per CPU: the highest service BAND whose wake is
/// queued on this core and found a LOWER-band incumbent running (0 = none pending; every real band
/// is `>= 1`, and only service bands `>= PRIO_SERVICE` are ever stored). Set by `preempt_hint`'s
/// service arm BEFORE `make_ready` sends the wake SGI (so the flag is visible by the time the SGI
/// lands), consumed exactly once (`swap(0)`) by `ipi_preempt` on the target core — one preemption
/// per IPI, by construction. `fetch_max` rather than `store` so two concurrent wakes of different
/// service bands leave the higher one pending.
static KICK_BAND: [AtomicU8; NUM_CPUS] = [const { AtomicU8::new(0) }; NUM_CPUS];

/// SPREAD-9 — IPI-receipt preemptions performed: `ipi_preempt` found a pending kick band above the
/// running task's and dispatched from the IRQ-exit path instead of returning to the incumbent. On
/// metal under fleet load this climbs with the service wake rate; the wakes it serves are the ones
/// whose `svc_lat` collapses from tick scale to IPI scale.
static SPREAD9_KICK: AtomicU64 = AtomicU64::new(0);

/// SPREAD-9 — service-band wake-to-dispatch latency (CNTPCT cycles): `make_ready` stamp to first
/// dispatch, for wakes of BASE priority `>= PRIO_SERVICE` (the population `preempt_hint`'s service
/// arm and `ipi_preempt` serve). Split from the SPREAD-7 aggregates so the EL0 `wake2disp` pricing
/// keeps its exact population and the service band's number is readable on its own: this is the
/// figure that should sit at IPI scale (mean < 100 us) once services preempt at IPI receipt.
static SPREAD9_SVC_SUM: AtomicU64 = AtomicU64::new(0);
static SPREAD9_SVC_N: AtomicU64 = AtomicU64::new(0);
static SPREAD9_SVC_MAX: AtomicU64 = AtomicU64::new(0);

/// SPREAD-4 — rate limit for the per-event `[spread4] rewake` trace, on the same terms as
/// `[smpbal] steal`: name the first few moves, then go quiet so a steadily-rebalancing desktop cannot
/// flood the serial log. The cumulative counters in `spread4_witness` carry the steady state.
#[cfg(feature = "pi")]
const SPREAD4_LOG_MAX: u32 = 16;
#[cfg(feature = "pi")]
static SPREAD4_LOG_COUNT: AtomicU32 = AtomicU32::new(0);

/// Register `cpu` as an online, scheduling core — a candidate for `CPU_AUTO` load-balanced placement.
/// Called by the BSP as it releases the APs (`start_aps`); idempotent, introspection-only bookkeeping
/// (no effect on any existing caller-pinned spawn, so boot behavior is byte-identical without CPU_AUTO).
pub fn mark_online(cpu: usize) {
    if cpu < NUM_CPUS {
        ONLINE_MASK[cpu].store(true, Ordering::Release);
    }
}

/// Resolve a requested `cpu` to a concrete core. An explicit index passes through verbatim (the
/// no-migrate pin contract). `CPU_AUTO` selects the least-loaded online core, keyed in this order:
///
///   1. SPREAD-3/SPREAD-4 — fewest RUNNABLE EL0 residents (`el0_active` = committed minus parked).
///      This is the only signal that is already true at the instant of the decision, so N spawns in a
///      burst spread instead of all agreeing on one core. It is the primary key precisely because a
///      running compute-bound EL0 task is invisible to both keys below. SPREAD-4 subtracts the PARKED
///      residents: a vug blocked on its input futex owes its core nothing, and counting it kept
///      placement steering around load that was not there (see `EL0_PARKED`).
///   2. minimum ready-queue DEPTH — the classic "will this task wait" signal, still the right
///      discriminator between cores carrying equal resident counts.
///   3. lower rolling-window busy fraction, then
///   4. a rotating cursor, so cores that tie on every measurable signal fill round-robin.
///
/// Keys 2-4 are SCHED-3's chain, unchanged; SPREAD-3 only puts committed load ahead of them. When no
/// EL0 task exists (the `virt`/JC3 kernel-thread builds, and the placement-spread witness that runs
/// at the top of `start_aps`) every core reads 0 residents, key 1 is a universal tie, and placement
/// is byte-identical to SCHED-3. Falls back to core 0 only if no core is online yet (early BSP staging).
fn pick_cpu(requested: usize) -> usize {
    pick_cpu_slot(requested, 0)
}

/// SPREAD-10 — `pick_cpu` with the co-residency term. `slot` 0 (every kernel spawn, the shared
/// window) reduces EXACTLY to the SCHED-3/SPREAD-9 key chain: the primary key is compared in doubled
/// units (`2*res + 1`), which is order-isomorphic to `res` when no bonus applies. A nonzero slot
/// discounts a core already holding same-slot siblings by half a resident — enough to win every
/// runnable-resident tie (ahead of the depth/pct tie-breaks), never enough to beat a core with one
/// fewer runnable resident. Weight rationale: the SPREAD-10 block above `SLOT_CORE_RES`.
fn pick_cpu_slot(requested: usize, slot: usize) -> usize {
    if requested != CPU_AUTO {
        return requested;
    }
    let rot = AUTO_ROTATE.fetch_add(1, Ordering::Relaxed);
    let mut best: Option<usize> = None;
    let mut best_eff = usize::MAX;
    let mut best_depth = usize::MAX;
    let mut best_pct = u32::MAX;
    // SPREAD-10 — the bonus-free winner, tracked in parallel so `co_moves` can count exactly the
    // spawns the bonus DECIDED (winner differs from what the plain key chain would have picked).
    let mut plain: Option<usize> = None;
    let mut plain_res = usize::MAX;
    let mut plain_depth = usize::MAX;
    let mut plain_pct = u32::MAX;
    for i in 0..NUM_CPUS {
        let cpu = (rot + i) % NUM_CPUS; // rotating start => fully-tied cores fill round-robin
        if !ONLINE_MASK[cpu].load(Ordering::Acquire) {
            continue;
        }
        let res = el0_active(cpu); // SPREAD-4: runnable residents, not merely committed ones
        // SPREAD-10: half-resident discount for a core already hosting this task's siblings; the +1
        // shift keeps the subtraction above zero at res 0.
        let eff = 2 * res + 1 - usize::from(slot_res(cpu, slot) > 0);
        // SPREAD-9 — the dissolved service-core reserve: keys 2 and 3 now weigh only work that
        // actually COMPETES with a new arrival. A queued/running service-band task preempts at IPI
        // receipt, runs a micro pass and blocks — it is latency-invisible to a co-resident — but the
        // full depth and the service-inclusive busy percent made whichever core currently hosted the
        // band read loaded, so placement steered the fleet around a hole that followed the services
        // (the ~40%-busy core beside three 99% ones under an 18-task storm). Below-band depth and
        // the service-subtracted percent are the same signals minus exactly that time; the service
        // tasks themselves keep their pins and their placement freedom untouched.
        let depth = rq(cpu).len_below_band();
        let pct = ACCT[cpu].el0_busy_pct(CUR_PRIO[cpu].load(Ordering::Relaxed) >= PRIO_SERVICE);
        let better = eff < best_eff
            || (eff == best_eff
                && (depth < best_depth || (depth == best_depth && pct < best_pct)));
        if better {
            best = Some(cpu);
            best_eff = eff;
            best_depth = depth;
            best_pct = pct;
        }
        let plain_better = res < plain_res
            || (res == plain_res
                && (depth < plain_depth || (depth == plain_depth && pct < plain_pct)));
        if plain_better {
            plain = Some(cpu);
            plain_res = res;
            plain_depth = depth;
            plain_pct = pct;
        }
    }
    // SPREAD-10 — a spawn steered off the plain winner is a placement the bonus decided.
    if slot != 0 && best.is_some() && best != plain {
        SPREAD10_CO_MOVES.fetch_add(1, Ordering::Relaxed);
    }
    best.unwrap_or(0)
}

/// Shared spawn path: build a kernel thread on `cpu`'s run queue (optionally carrying a `done_sem`
/// completion signal for `join`), enqueue it, and poke that CPU. `cpu` may be `CPU_AUTO` for
/// load-balanced placement (see `pick_cpu`); any real index is a verbatim no-migrate pin. Returns id.
fn spawn_inner(
    name: &'static str,
    entry: fn(usize),
    arg: usize,
    requested_cpu: usize,
    priority: u8,
    done_sem: Option<Arc<Semaphore>>,
) -> u64 {
    let cpu = pick_cpu(requested_cpu);
    assert!(cpu < NUM_CPUS, "spawn: cpu out of range");
    let mut stack: Box<[u8]> = alloc::vec![0u8; TASK_STACK_SIZE].into_boxed_slice();
    let ctx_sp = build_initial_frame(&mut stack, task_trampoline);
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
        priority,
        wait_ticks: 0, // re-zeroed by RunQueue::push on every enqueue; satisfies the struct literal
        done_sem,
        user_entry: 0,
        user_sp: 0,
        user_ttbr0: 0, // kernel task: no root switch (kernel mappings are Global in every root)
        // SMP-BAL: a load-balanced (CPU_AUTO) kernel task has no core affinity → steal-eligible. A task
        // spawned onto an explicit core is pinned there (no-migrate), so stealing never touches it.
        steal_ok: requested_cpu == CPU_AUTO,
        park_cyc: 0, // SPREAD-5: never parked yet; the first park_blocked stamps it.
        place_cyc: now_cyc(), // SPREAD-6: spawn placement counts as the first ask of the question
        wake_cyc: 0, // SPREAD-7: never woken yet; `make_ready` stamps it
    });
    rq(cpu).push(task);
    // PI-SCHED-1 — placement witness. The scheduler PINS a task to the caller-chosen core and never
    // migrates it (see `Task.cpu` / `make_ready`), so the placement is decided entirely at the spawn
    // site; this line makes that decision auditable (the probe's core deliverable). Gated behind the
    // `pi` feature so it fires on the Raspberry Pi target (where the two core-placement sightings that
    // motivated this arc were observed) and in the `kernel8-test` gate, while staying BYTE-IDENTICAL for
    // the jetson/tegra + virt builds that share this aarch64 module. FLAG: this file is shared across the
    // aarch64 sub-tracks — the addition is log-only and behind `pi`, so it cannot alter scheduling on any
    // track. (The Pi `kernel8` build never sets `sched_demo`, so `pi` is the gate that makes the probe
    // visible on the actual target.)
    #[cfg(feature = "pi")]
    serial_println!(
        ":: SCHED: task '{}' -> core {} (policy: {}, no-migrate; prio {}) ::",
        name,
        cpu,
        if requested_cpu == CPU_AUTO { "load-balanced" } else { "caller-pinned" },
        priority
    );
    // Wake the target if it's a different, possibly-idle core (same-core needs no poke).
    poke_cpu(cpu);
    id
}

/// Create a ready, fire-and-forget kernel thread on `cpu`'s run queue at the DEFAULT priority
/// (`PRIO_NORMAL` — the single level, so this stays behaviourally identical to the pre-priority flat
/// round-robin): it runs `entry(arg)` and is freed when `entry` returns, with no way to wait for it
/// (use `spawn_joinable` for that). Returns the task id. Use `spawn_prio` to pick a level.
pub fn spawn(name: &'static str, entry: fn(usize), arg: usize, cpu: usize) -> u64 {
    spawn_inner(name, entry, arg, cpu, PRIO_NORMAL, None)
}

/// Like `spawn`, but at an explicit scheduling `priority` (`0..NUM_PRIORITIES`; higher = more urgent,
/// clamped in range). The CPU always runs a ready task of the highest non-empty level; a lower task
/// is protected from indefinite starvation by aging (see `AGE_TICKS`). Returns the task id.
pub fn spawn_prio(name: &'static str, entry: fn(usize), arg: usize, cpu: usize, priority: u8) -> u64 {
    spawn_inner(name, entry, arg, cpu, priority, None)
}

/// SCHED-3: like `spawn`, but LOAD-BALANCED — the scheduler places the task on the least-loaded online
/// core (see `pick_cpu`) instead of a caller-named pin. For fire-and-forget service/worker tasks that
/// have no core affinity; use `spawn` (explicit `cpu`) when a task MUST run on a specific core (e.g. the
/// single-core render loop). Equivalent to `spawn(.., CPU_AUTO)`. Returns the task id.
pub fn spawn_auto(name: &'static str, entry: fn(usize), arg: usize) -> u64 {
    spawn_inner(name, entry, arg, CPU_AUTO, PRIO_NORMAL, None)
}

/// Create a ready EL0 (user-mode) task on `cpu`'s run queue (M6a): when dispatched it drops to EL0 at
/// `user_entry` with SP_EL0 = `user_sp` (both from `syscall::setup`) and calls back into the kernel via
/// `svc`. MUST be spawned on a SCHEDULED core (an AP), never the unscheduled BSP — `user_task_trampoline`
/// reads `SCHED[cpu].current`, which is null on a core that never runs the scheduler loop. Fire-and-
/// forget: `sys_exit` marks it FINISHED and the scheduler reclaims it. Returns the task id.
///
/// Since M6e EVERY EL0 task starts I-unmasked (preemptible) — `user_task_trampoline` plants SPSR 0x240
/// — so on metal the timer may preempt any of them mid-EL0; `__vec_irq` banks SP_EL0 so that is safe.
///
/// M6d: this variant runs the task on the SHARED user window under the boot root (`&L1 | ASID 0`) — the
/// path the M6b/M6e demo tasks take (they don't write their shared stack). A task carrying its OWN
/// address space (a private slot) is created with `spawn_user_slot` instead. Setting `user_ttbr0` to the
/// boot root (rather than 0) is load-bearing: if a per-slot task last ran on this core, `dispatch_next`
/// must switch TTBR0 back to the boot root before this task's EL0 access hits the shared window VA.
///
/// EL0/user spawn machinery (this and `spawn_user_slot`/`spawn_user_inner`) is baremetal-only: it reaches
/// into `super::boot` (the Pi-gated user MMU), and the `virt` JC3 path runs kernel-thread CAPSTONE only.
#[cfg(feature = "baremetal")]
pub fn spawn_user(name: &'static str, user_entry: u64, user_sp: u64, cpu: usize) -> u64 {
    spawn_user_inner(name, user_entry, user_sp, super::boot::boot_ttbr0(), cpu)
}

/// Like `spawn_user`, but the task runs in its OWN per-task address space (M6d): `user_ttbr0` is the
/// slot root `slot_l1_pa | (asid << 48)` from `boot::slot_ttbr0`. `dispatch_next` installs it on
/// dispatch; `exit` tears the slot down. This is what lets an EL0 program write its own (slot-private)
/// stack without disturbing any other task.
#[cfg(feature = "baremetal")]
pub fn spawn_user_slot(
    name: &'static str,
    user_entry: u64,
    user_sp: u64,
    user_ttbr0: u64,
    cpu: usize,
) -> u64 {
    spawn_user_inner(name, user_entry, user_sp, user_ttbr0, cpu)
}

#[cfg(feature = "baremetal")]
fn spawn_user_inner(
    name: &'static str,
    user_entry: u64,
    user_sp: u64,
    user_ttbr0: u64,
    requested_cpu: usize,
) -> u64 {
    // SPREAD-10: bias toward the core(s) already holding this address space's tasks. For a fresh
    // slot the count is zero everywhere and this is byte-identical to the plain key chain.
    let cpu = pick_cpu_slot(requested_cpu, slot_of(user_ttbr0));
    assert!(cpu < NUM_CPUS, "spawn_user: cpu out of range");
    // BG-SPREAD witness aid — record the decision so the caller can read back where its task landed.
    LAST_USER_PLACEMENT.store(cpu, Ordering::Release);
    let mut stack: Box<[u8]> = alloc::vec![0u8; TASK_STACK_SIZE].into_boxed_slice();
    let ctx_sp = build_initial_frame(&mut stack, user_task_trampoline);
    let id = NEXT_TID.fetch_add(1, Ordering::Relaxed);
    let task = Box::new(Task {
        id,
        name,
        state: AtomicU8::new(STATE_READY),
        ctx_sp,
        stack,
        entry: user_never, // never called — the user trampoline erets to EL0 instead
        arg: 0,
        cpu: cpu as u32,
        priority: PRIO_NORMAL, // EL0 tasks run at the default level (unchanged from the pre-priority path)
        wait_ticks: 0,
        done_sem: None,
        user_entry,
        user_sp,
        user_ttbr0,
        // SMP-BAL: EL0/slot tasks carry per-core TTBR0/ASID state — always pinned, never stolen.
        steal_ok: false,
        park_cyc: 0, // SPREAD-5: never parked yet; the first park_blocked stamps it.
        place_cyc: now_cyc(), // SPREAD-6: spawn placement counts as the first ask of the question
        wake_cyc: 0, // SPREAD-7: never woken yet; `make_ready` stamps it
    });
    // SKILL-1: count this task against its address-space slot BEFORE the push, so it is countable before
    // it can ever be dispatched (and thus before any ASID-scoped kill could observe a short count).
    asid_thread_enter(user_ttbr0);
    // SPREAD-3: commit this EL0 resident to its core BEFORE the enqueue, so the very next `pick_cpu`
    // already sees it. PINNED EL0 spawns are counted too — the pin is honored verbatim (placement is
    // untouched), but the residents it parks on that core are real committed load and a later
    // `CPU_AUTO` placement must see them.
    let residents = el0_resident_enter(cpu);
    slot_res_enter(cpu, user_ttbr0); // SPREAD-10: same instant, same core — the sibling map stays true
    rq(cpu).push(task);
    // PI-SCHED-1 — placement witness for EL0 tasks (the vug/midden GUI-app loads land here — the
    // "all vug load on core 2" sighting). Same `pi`-gating + rationale as `spawn_inner`. SPREAD-3 folds
    // the counted value into the existing `policy:` field (shape unchanged, still one parseable line)
    // so the next attended boot can check the accounting against the observed spread. `residents` is
    // INCLUSIVE of this task: it is the committed count on that core after this placement.
    #[cfg(feature = "pi")]
    serial_println!(
        ":: SCHED: task '{}' -> core {} (policy: {} EL0 residents={}, no-migrate) ::",
        name,
        cpu,
        if requested_cpu == CPU_AUTO { "load-balanced" } else { "caller-pinned" },
        residents
    );
    #[cfg(not(feature = "pi"))]
    let _ = residents;
    poke_cpu(cpu);
    id
}

/// ELF-2 — spawn an EL0 THREAD: a new user task that SHARES the address space named by `user_ttbr0` (the
/// parent's slot root `slot_l1_pa | asid<<48`), rather than a fresh private slot. This is the primitive
/// behind `SYS_THREAD_SPAWN`: the caller carves `user_sp` (a stack) from its own 16 KiB window, names an
/// `user_entry` PC inside the shared window, and the new thread runs concurrently under the SAME ASID —
/// possibly on a different core (`cpu`). It carries one u64 `arg` (delivered in x0 by `user_task_trampoline`)
/// and a `JoinHandle` completion semaphore (posted in `exit()` — the single post point).
///
/// SLOT REFCOUNT: the caller MUST have already `slot_thread_retain`ed the slot BEFORE calling this, so the
/// shared address space cannot be torn down between here and the thread's first dispatch. Teardown happens
/// on the LAST thread's exit (`teardown_user_slot`'s 1->0 edge).
///
/// Multi-core soundness (shared ASID on two cores): `dispatch_next` installs `user_ttbr0` per-core, so both
/// cores run under the same root/ASID; nG user leaves are ASID-tagged and TLB maintenance is Inner-Shareable
/// broadcast; and `teardown_user_slot` now repoints EACH exiting thread's core off the slot root, so the
/// final ASID flush races no live root on any other core. Baremetal-only (reaches the Pi-gated user MMU).
#[cfg(feature = "baremetal")]
pub fn spawn_user_thread(
    name: &'static str,
    user_entry: u64,
    user_sp: u64,
    arg: u64,
    user_ttbr0: u64,
    requested_cpu: usize,
) -> JoinHandle {
    // SPREAD-10: THE co-placement site — a worker spawns under its parent's slot root, so the bias
    // lands it beside the parent (or its earlier-spawned sibling) whenever the load terms tie.
    let cpu = pick_cpu_slot(requested_cpu, slot_of(user_ttbr0));
    assert!(cpu < NUM_CPUS, "spawn_user_thread: cpu out of range");
    let done = Arc::new(Semaphore::new(0));
    done.init(); // reserve the waiter list BEFORE the thread can run + post (alloc-free park)
    let mut stack: Box<[u8]> = alloc::vec![0u8; TASK_STACK_SIZE].into_boxed_slice();
    let ctx_sp = build_initial_frame(&mut stack, user_task_trampoline);
    let id = NEXT_TID.fetch_add(1, Ordering::Relaxed);
    let task = Box::new(Task {
        id,
        name,
        state: AtomicU8::new(STATE_READY),
        ctx_sp,
        stack,
        entry: user_never,
        arg: arg as usize, // delivered to EL0 in x0 by user_task_trampoline (the thread's single argument)
        cpu: cpu as u32,
        priority: PRIO_NORMAL,
        wait_ticks: 0,
        done_sem: Some(done.clone()),
        user_entry,
        user_sp,
        user_ttbr0,
        // SMP-BAL: shared-ASID EL0 thread — per-core address-space state, always pinned.
        steal_ok: false,
        park_cyc: 0, // SPREAD-5: never parked yet; the first park_blocked stamps it.
        place_cyc: now_cyc(), // SPREAD-6: spawn placement counts as the first ask of the question
        wake_cyc: 0, // SPREAD-7: never woken yet; `make_ready` stamps it
    });
    // SKILL-1: this is the path that MAKES a slot multi-threaded, so it is the one the ASID-scoped kill
    // exists for — count the sibling before it can be dispatched.
    asid_thread_enter(user_ttbr0);
    // SPREAD-3: a shared-ASID EL0 thread burns a core exactly like a slot task does — count it as a
    // committed resident on the same terms (before the enqueue; released on its `exit()`).
    let residents = el0_resident_enter(cpu);
    slot_res_enter(cpu, user_ttbr0); // SPREAD-10: same instant, same core — the sibling map stays true
    rq(cpu).push(task);
    #[cfg(feature = "pi")]
    serial_println!(
        ":: SCHED: thread '{}' -> core {} (ELF-2 shared-ASID EL0 thread residents={}, no-migrate) ::",
        name,
        cpu,
        residents
    );
    #[cfg(not(feature = "pi"))]
    let _ = residents;
    poke_cpu(cpu);
    JoinHandle { done, id }
}

/// BG-SPREAD — the core chosen by the most recent EL0-SLOT spawn (`spawn_user_slot`). Witness aid
/// only: it makes the placement decision readable by the caller that just made it, which the
/// `pid`-returning signature cannot express. Read it IMMEDIATELY after your own `spawn_user_slot`
/// call, from the same task — it is a single global, so an interleaved slot spawn on another core
/// would overwrite it. Never a scheduling input; nothing reads it but the BGSPREAD witness.
static LAST_USER_PLACEMENT: AtomicUsize = AtomicUsize::new(0);

/// BG-SPREAD — read the core the last `spawn_user_slot` placed its task on. See
/// [`LAST_USER_PLACEMENT`] for the (deliberately narrow) contract.
pub fn last_user_placement() -> usize {
    LAST_USER_PLACEMENT.load(Ordering::Acquire)
}

/// BG-SPREAD — how many cores are registered online + scheduling (the `CPU_AUTO` candidate set).
/// Lets a witness skip honestly on a boot that brought up fewer cores than it needs to observe a
/// spread (the metal 3-of-4-cores variance documented in the pi4 regression spec).
pub fn online_cpu_count() -> usize {
    (0..NUM_CPUS).filter(|&c| ONLINE_MASK[c].load(Ordering::Acquire)).count()
}

/// ELF-2 — pick an online scheduling core DIFFERENT from `not`, for placing a thread on a sibling core (the
/// `SYS_THREAD_SPAWN` "spread" hint). Returns the least-loaded such core, or `not` itself if no other core is
/// online (uniprocessor / early staging). Lets a thread demonstrate genuine cross-core parallelism without
/// the EL0 program knowing the CPU topology.
pub fn other_online_cpu(not: usize) -> usize {
    let mut best: Option<usize> = None;
    let mut best_depth = usize::MAX;
    for c in 0..NUM_CPUS {
        if c == not || !ONLINE_MASK[c].load(Ordering::Acquire) {
            continue;
        }
        let depth = rq(c).len();
        if depth < best_depth {
            best_depth = depth;
            best = Some(c);
        }
    }
    best.unwrap_or(not)
}

/// Like `spawn`, but returns a `JoinHandle` a scheduled task can `join()` to block until this task
/// finishes. Allocates an `Arc<Semaphore>` (0 permits) shared between the new task and the handle;
/// the task's trampoline posts it on completion. Costs one heap alloc + a reserved waiter list, so
/// only pay it when you actually need to join.
pub fn spawn_joinable(name: &'static str, entry: fn(usize), arg: usize, cpu: usize) -> JoinHandle {
    let done = Arc::new(Semaphore::new(0));
    done.init(); // reserve the waiter list BEFORE the task can run + post (alloc-free park)
    let id = spawn_inner(name, entry, arg, cpu, PRIO_NORMAL, Some(done.clone()));
    JoinHandle { done, id }
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

    /// KILLBOUND: evict any killed task parked on THIS completion semaphore (a `SYS_THREAD_JOIN`er).
    /// Delegates to `Semaphore::wake_killed`; see there for why no permit is handed over.
    pub fn wake_killed(&self) -> u32 {
        self.done.wake_killed()
    }

    /// Block the current task until the joined task finishes. MUST be called from a scheduled task
    /// (it blocks); the `assert` rejects a call off the scheduler (e.g. the unscheduled BSP) loudly
    /// rather than silently returning as if the task had finished.
    ///
    /// No timeout: a joined task that PANICS or never returns leaves the joiner blocked forever (the
    /// completion permit is posted only on a normal `entry` return; this no-`std` kernel has no
    /// unwinding). This matches the kernel's panic-halts policy.
    ///
    /// `self` (the Arc clone) deliberately stays bound for the whole body: it is the refcount anchor
    /// that keeps the completion semaphore alive while this task is parked in its waiter list. In
    /// every window where a raw pointer to the semaphore internals is live (the parked joiner's
    /// park_waiters/park_lock, or a waiter Box in the list), >= 1 Arc clone is provably held — the
    /// parked joiner holds its handle on its frozen stack, and the posting task holds its `done_sem`
    /// clone until the Box is dropped on the Finished path, strictly after the post. So the Arc
    /// refcount substitutes for the `Semaphore`'s `'static` requirement: no UAF, no leak.
    pub fn join(self) {
        assert!(
            self.done.wait(),
            "JoinHandle::join() must be called from a scheduled task"
        );
    }
}

// Compile-time guard for the `Task` <-> `Arc<Semaphore>` Send cycle introduced by `done_sem`. No
// `unsafe impl Send` is added: `Semaphore`'s Send auto-derives while every field is `Send`, and the
// cycle resolves co-inductively. This probe locks that in — if a future field breaks it, fix that
// field rather than papering over it with an `unsafe impl`.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Task>();
    assert_send::<Arc<Semaphore>>();
    assert_send::<JoinHandle>();
};

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
        // The reschedule SGI: `smp::IPI_RESCHED` on the Pi (spin-table SMP), SGI 0 on the `virt` path
        // (`smp_virt::IPI_SGI`; `smp` is baremetal-only there). On the JC3 boot-core-only CAPSTONE this
        // branch is never taken (self-poke is skipped), so the `virt` value is only ever a compile target.
        #[cfg(feature = "baremetal")]
        super::gic::send_sgi(target, super::smp::IPI_RESCHED);
        #[cfg(not(feature = "baremetal"))]
        super::gic::send_sgi(target, 0);
    }
}

/// SCHED-PRIO — shorten the running task's quantum on `target` when a HIGHER-band task has just been
/// made ready there, so the band's ordering preference actually turns into latency.
///
/// ### Why a hint and not a switch
/// Strict priority is applied at the DISPATCH BOUNDARY (`pop_highest`), which means a woken service
/// task waits for the running task to reach one: a yield, a block, or quantum expiry. With
/// `QUANTUM_TICKS = 3` at ~4 ms/tick, an EL0 vug that never yields (they spin) can hold the core for
/// up to ~12 ms past the wake — per wake, on the cursor's path. Trimming the countdown to ONE tick
/// caps that at ~4 ms without any cross-core register surgery, without an extra IPI, and without
/// touching the switch path.
///
/// ### Why it is safe
///   * It only ever LOWERS a countdown, and never below 1 — the interrupted task always gets at least
///     one more tick, so there is no way to livelock a task out of making progress, and no way to
///     preempt one that is mid-anything: `timer_preempt` is the sole consumer and it already runs
///     only at a legitimate boundary (post-EOI, with the run-queue guard's `IN_RQ_SECTION` tripwire
///     watching for a section breach).
///   * The two atomics are a cross-core relaxed load (`CUR_PRIO`) and a cross-core relaxed store
///     (`quantum`), both racy by construction and both benign: the worst outcome of losing the race
///     with the owning core's own `quantum.store(QUANTUM_TICKS)` is that this wake does not get its
///     trim and the service task waits the ordinary quantum — exactly the pre-arc behaviour.
///   * `CUR_PRIO` is the ONLY thing read cross-core. `SCHED[target].current` is deliberately not
///     touched: it is a raw pointer to a Box the owning core can free at any instant.
///   * On QEMU raspi4b there is no live timer IRQ, so there is no quantum countdown and this is inert
///     — the gate exercises the counter, and the trim itself is a metal-only effect (as with every
///     other preemption behaviour in this module).
/// ### SPREAD-7 — the same-band quantization made countable (the ~35 fps ceiling)
/// SCHED-PRIO built the trim for the service band only, and P79's storm-6 wire (pi4-r23s1r) showed
/// what that leaves on the table for everyone below it. A vug frame is a THREE-TASK RENDEZVOUS
/// (parent + two workers on the PHASE futex), and every rendezvous the spin window does not catch
/// becomes a park followed by a wake. The wake's SGI only breaks WFI — `gic::handle_irq` counts SGIs
/// and returns — so a woken PRIO_NORMAL task landing on a BUSY core sits in the run queue until the
/// running task reaches a dispatch boundary: yield, block, or quantum expiry, up to a full quantum
/// (~12 ms, mean ~6 ms) per park. A frame with two or three uncaught rendezvous is 15-35 ms of pure
/// queue wait, which is the metal reading exactly: per-window rates pinned at 21-39/s (26-48 ms
/// frames, in quantum-sized steps), `gap` minima of 15 ms on the lucky windows and 32-71 ms on the
/// crowded ones, and the rare ~50/s escape being a stretch where the spin windows caught every
/// rendezvous and nothing parked. The ceiling was never a target or a cap (VUG-PACE) — it is wake
/// latency quantized by the co-resident's quantum.
///
/// CROSS-ARCH CONVERGENCE. x86's `make_ready` has the same three-arm shape: idle target ->
/// IPI-paced (our `poke_cpu` SGI, already present and metal-validated); higher-band wake -> preempt
/// (our service trim above); equal-or-lower-band wake onto a busy core -> waits for the tick, on
/// both arches, as SCHED-PRIO designed it. SPREAD-7 landed the instrument for that last arm:
/// `SPREAD7_QUANT` counts exactly the wakes it quantizes (a woken EL0 task, below the service band,
/// equal-or-higher than the target core's running band — an idle core reads `CUR_PRIO == PRIO_NONE`
/// (`u8::MAX`) and never matches), and the wake-to-dispatch stamps in `dispatch_next` price what
/// each one cost. `quant` climbing at the fleet's park rate with `wd_mean` in the half-quantum
/// range IS the ceiling, live on the wire.
///
/// ### SPREAD-8 — the same-band trim (the policy, implemented)
/// POLICY: an equal-band wake is worth at most ONE tick of the incumbent's time, not a full
/// quantum. SPREAD-7's wire pricing (pi4-r23s1r) is the motivation: every uncaught rendezvous paid
/// a mean ~6 ms / worst ~12 ms of pure run-queue wait behind an incumbent of the SAME band, and
/// that wait — not rendering, not compositing — was the whole fps ceiling. So the same-band arm now
/// gets the SCHED-PRIO trim: `prio >= cur` => the incumbent's countdown drops to 1. The TICK BOUND
/// is the churn guard: the incumbent still finishes its current tick (nothing is switched out by
/// force, exactly as the service trim above), so the worst case is one extra dispatch boundary per
/// tick per core — structurally incapable of re-running SPREAD-4's ~540-moves/s disease, whose
/// engine was per-frame MIGRATION, not dispatch. Same safety argument as the service trim: the swap
/// only ever lowers the countdown toward 1 (a raced `quantum.store(QUANTUM_TICKS)` from the owning
/// core loses at most this wake's trim — the pre-SPREAD-8 behaviour), `timer_preempt` remains the
/// sole consumer, and `CUR_PRIO` is still the only cross-core read. `SPREAD8_TRIM` counts the wakes
/// whose swap actually lowered a countdown, so the wire can prove the policy fires (`trim=` beside
/// `quant=` on the `[spread7]` line, climbing together at the park rate).
fn preempt_hint(target: usize, prio: u8, el0: bool) {
    let cur = CUR_PRIO[target].load(Ordering::Relaxed);
    if prio >= PRIO_SERVICE {
        if prio > cur {
            SCHED[target].quantum.store(1, Ordering::Relaxed);
            PRIO_DEFER[target].fetch_add(1, Ordering::Relaxed);
            // SPREAD-9 — arm the IPI-receipt preemption: a HIGHER-band service wake no longer waits
            // for the next timer tick. `make_ready` calls this hint BEFORE it sends the wake SGI, so
            // by the time `gic::handle_irq` acks that SGI on the target the band is pending there and
            // `ipi_preempt` (IRQ-exit, post-EOI — the same boundary `timer_preempt` uses) dispatches
            // instead of returning to the incumbent. The quantum trim above is kept as the fallback
            // for the races (kick consumed by an earlier in-flight SGI, IRQ landing inside a masked
            // section): the wake then costs at most one tick, the SCHED-PRIO behaviour. Equal-band
            // wakes (`prio == cur`) deliberately take neither arm here and keep SPREAD-8's one-tick
            // policy — immediate preemption is the service band's alone, so the fleet cannot churn
            // itself with it.
            KICK_BAND[target].fetch_max(prio, Ordering::Relaxed);
        }
        return;
    }
    // SPREAD-7 count + SPREAD-8 trim: `prio >= cur` with `prio < PRIO_SERVICE` implies `cur` is a
    // real below-service level, so PRIO_NONE (idle) and any service-band runner are excluded for
    // free. An equal-band wake is worth at most ONE tick of the incumbent's time, not a full
    // quantum (see SPREAD-8 above): trim the countdown, tick-bounded, and count both the quantized
    // population (quant=) and the trims that actually shortened a countdown (trim=).
    if el0 && prio >= cur {
        SPREAD7_QUANT.fetch_add(1, Ordering::Relaxed);
        if SCHED[target].quantum.swap(1, Ordering::Relaxed) > 1 {
            SPREAD8_TRIM.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Mark a parked/just-woken task READY, push it onto a run queue, and poke that CPU. Used by the
/// sleeper drain (same CPU) and, from M4b, `Semaphore::post` (cross-CPU wake). Caller runs with IRQ
/// masked.
///
/// A KERNEL task always returns to `task.cpu` exactly as before — that is the no-migrate contract the
/// kernel blocking primitives are written against (`Condvar::wait` rebuilds its `!Send` guard on the
/// resuming core; `spawn_inner`'s caller-pinned tasks are pinned on purpose), and nothing here
/// touches it.
///
/// SPREAD-4 — AN EL0 TASK GETS ITS PLACEMENT RE-EXAMINED HERE, AND ONLY HERE. Two things have to be
/// true at once for a move to be sound, and this is the only point in a task's life where both are:
///
///   * the task owns no core state. It is parked: not `current` anywhere, not in any run queue, no
///     live register context beyond the saved frame on its own kernel stack (which is a Global
///     identity mapping, valid under every root). `dispatch_next` installs `user_ttbr0` on whichever
///     core dispatches it, so the address space follows the task rather than the core.
///   * we already hold the decision. The wake is going to push onto SOME queue in the next few
///     instructions, so choosing a different one costs one comparison and no extra machinery.
///
/// The residual is the old core's TTBR0, which keeps pointing at the moved task's slot root until
/// that core next dispatches a user task. That is benign and stays benign: the slot L1 tables are a
/// STATIC array (`boot::SLOT_L1`), never freed to the heap, and `teardown_user_slot` broadcasts
/// `tlbi aside1is` for the ASID on the last release — so a core left holding the value has no stale
/// translations and no dangling page. It runs EL1 code, which never touches a low VA.
///
/// The counters are kept consistent across the move in one order: un-park at home (so `home`'s figure
/// excludes the waker itself), pick, then transfer the RESIDENT credit home -> target and retarget
/// `task.cpu`. Both reap paths (`exit`, `retire_killed`) release against `task.cpu`, so the transfer
/// is what keeps them releasing the core that is actually carrying the task.
fn make_ready(mut task: Box<Task>) {
    let home = task.cpu as usize;
    let mut target = home;
    // SPREAD-4: `user_entry != 0` is the same EL0 test SPREAD-3's enter/leave sites use, so the parked
    // accounting covers exactly the population the resident accounting does. A kernel task falls
    // straight through to the unchanged push below.
    if task.user_entry != 0 {
        el0_parked_leave(home);
        // SPREAD-5: ask the placement question only for a wake that follows a real absence. `park_cyc`
        // is stamped by `park_blocked`, the sole park funnel, so a zero here means this task reached
        // `make_ready` without ever having parked — the kill sweeps can re-ready a task that was never
        // blocked. Treat that as a short stay: it declines the move, which is the pre-SPREAD-4
        // behaviour and the conservative direction (a task is never moved on a duration we did not
        // measure). `saturating_sub` covers the same case defensively; CNTPCT itself is monotonic.
        let now = now_cyc();
        let parked_cyc = now.saturating_sub(task.park_cyc);
        let long_park = task.park_cyc != 0 && parked_cyc >= rewake_min_park_cyc();
        task.park_cyc = 0; // consumed — the next park stamps it afresh
        // SPREAD-6: a micro-park may still ask, on a slow clock. Without this, a task that never stops
        // never re-asks and its packing latches forever — the residual "predestined fps" (see
        // PLACE_REFRESH_MS). `place_cyc == 0` (a Task literal that predates the stamp discipline —
        // none today, but cheap to be honest about) counts as due: asking is safe, moving is gated.
        let refresh = !long_park && now.saturating_sub(task.place_cyc) >= place_refresh_cyc();
        if !long_park && !refresh {
            // The micro-park path: no placement call, no counters but this one, and `target` stays
            // `home`. `stay` is deliberately NOT incremented — it means "asked and declined", and
            // folding thousands of unasked frame wakes into it would bury the signal it carries.
            SPREAD5_SHORT_STAY.fetch_add(1, Ordering::Relaxed);
        } else {
            if refresh {
                SPREAD6_REFRESH.fetch_add(1, Ordering::Relaxed);
            }
            task.place_cyc = now; // the question is being asked NOW — re-arm the refresh clock
            // SPREAD-10: the slot names the task's futex-coupled siblings; `rewake_place` biases
            // toward the core(s) already holding them (0 = unslotted, no bias).
            target = rewake_place(home, slot_of(task.user_ttbr0));
            if target != home {
                el0_resident_leave(home);
                let act = el0_resident_enter(target);
                // SPREAD-10: the slot-residency credit moves with the resident credit, same order,
                // so a concurrent sibling's placement ask never sees this task counted twice.
                slot_res_leave(home, task.user_ttbr0);
                slot_res_enter(target, task.user_ttbr0);
                task.cpu = target as u32;
                SPREAD4_REWAKE.fetch_add(1, Ordering::Relaxed);
                #[cfg(feature = "pi")]
                if SPREAD4_LOG_COUNT.fetch_add(1, Ordering::Relaxed) < SPREAD4_LOG_MAX {
                    serial_println!(
                        ":: [spread4] rewake asid={} tid={} from=c{} to=c{} act={} parked={}ms ::",
                        task.user_ttbr0 >> 48,
                        task.id,
                        home,
                        target,
                        act,
                        cyc_to_ms(parked_cyc)
                    );
                }
                #[cfg(not(feature = "pi"))]
                let _ = act;
            } else {
                SPREAD4_STAY.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    debug_assert!(target < NUM_CPUS, "make_ready: cpu out of range");
    task.state.store(STATE_READY, Ordering::Release);
    // SCHED-PRIO: read the base priority BEFORE the Box is moved into the queue (after the push it
    // belongs to whichever core dispatches it next and must not be touched from here).
    let prio = task.priority;
    let el0 = task.user_entry != 0;
    if el0 || prio >= PRIO_SERVICE {
        // SPREAD-7: stamp the wake so the dispatching core can price the run-queue wait. EL0 (the
        // population whose frame rendezvous the [spread7] witness is instrumenting) — and, SPREAD-9,
        // the service band, whose wait lands in the separate `svc_lat` aggregates. Ordinary
        // kernel-worker wake traffic (sleeper drain, semaphores) stays out of both means.
        task.wake_cyc = now_cyc();
    }
    rq(target).push(task);
    // SCHED-PRIO: the wake path is where interactive latency is actually decided — `GUI_CHANNEL.recv`
    // (compositor), `RX_READY.wait` (input router) and `sleep_ticks` (HID pump) all come back through
    // here. Called AFTER the push (the task must already be queued when the target looks) and —
    // SPREAD-9 — BEFORE the poke: the hint's service arm arms `KICK_BAND`, and the flag has to be
    // set before the SGI it answers is sent, or the target's `ipi_preempt` could ack the SGI, find
    // no pending band, and return to the incumbent it was meant to preempt.
    preempt_hint(target, prio, el0);
    poke_cpu(target);
}

/// Cooperatively give up the CPU: mark this task ready and switch back to the scheduler, which
/// requeues us and runs the next task. We resume here (IRQ masked, carried by the switch) when
/// re-dispatched. No-op if called outside a scheduled task.
pub fn yield_now() {
    let cpu = percpu::this_cpu().cpu_index as usize;
    mask_irq();
    // SKILL-1 on-CPU kill boundary: a cooperative yield is as good a retirement point as a preemption.
    kill_check_current(); // never returns if this task has been killed
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

/// The name of the task currently dispatched on THIS core, or None outside a scheduled task (the
/// scheduler/idle context, or the unscheduled BSP). Reads the same `current` slot the trampolines
/// use; the `&'static str` stays valid even after the Box is reclaimed. Used by the M6b EL0
/// fault-kill path to label its log line.
pub fn current_name() -> Option<&'static str> {
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *const Task;
    if raw.is_null() { None } else { Some(unsafe { (*raw).name }) }
}

/// The id (pid) of the task currently dispatched on THIS core, or None outside a scheduled task.
/// The aarch64 twin of `current_name`; backs the M6f `SYS_GETPID`/`SYS_GETINFO` syscalls (a syscall
/// always runs with its EL0 task current, so it always resolves to a real id there).
pub fn current_id() -> Option<u64> {
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *const Task;
    if raw.is_null() { None } else { Some(unsafe { (*raw).id }) }
}

// =============================================================================================
// SKILL-1 — the ASYNCHRONOUS KILL primitive.
//
// Until this arc `exit()` was the only way a task could die, and it retires the CALLING task only.
// A timed-out `run_user_image` therefore had no way to stop the EL0 task it had given up on: the
// task kept running (observed on metal spinning a core at 100% forever, still rendering over the
// screen, starving every later run), and the only containment was parking its `Proc` row PORPHANED.
//
// The design deliberately does NO cross-CPU register surgery. A kill is a REQUEST published in a
// tiny fixed table keyed by task id; the target retires ITSELF, on its own core, at a boundary where
// switching away is already proven safe:
//
//   (a) OFF-CPU — `dispatch_next` checks the request table after popping a task and BEFORE switching
//       into it. A killed task is never dispatched again: it is torn down and dropped right there,
//       on the scheduler's own stack. This covers every ready, sleeping, or wait-queued task (they
//       all re-enter through a dispatch) and needs no cooperation from the target at all.
//   (b) ON-CPU — `timer_preempt` (the per-core quantum tick, the only involuntary boundary a spinning
//       EL0 task ever reaches), `yield_now`, and the SVC dispatcher (`aarch64_svc_handler`) call
//       `kill_check_current`, which routes a killed current task straight into `exit()`. Both call
//       sites already run IRQ-masked on the task's own kernel stack and already tolerate a
//       never-returning `exit()` — that is exactly what the M6b EL0 fault-kill path does.
//
// Task ids are monotonic (`NEXT_TID`) and never reused, so a request can never be mis-delivered to a
// later task that happened to inherit a number.
//
// CONFIRMATION is the point of the ticket: the requester needs to know the task is provably off every
// CPU and will never run again before it reclaims resources the task owns (its `Proc` row). The
// retiring side settles the slot as its LAST act before switching away for good / before its Box is
// dropped, so `KILL_DONE` means "torn down; nothing of this task will execute again".
//
// FAIL-CLOSED throughout: no free slot, or a kill that does not confirm inside the caller's bounded
// wait, degrades to exactly the pre-arc PORPHANED behaviour — never to freeing a row under a live task.
// =============================================================================================

/// How many kills may be in flight at once. Small on purpose: the requesters are the `run_user_image`
/// deadline path and the shell's `kill`, and the `Proc` table bounds how many rows can be at risk.
/// Exhaustion returns `None` and the caller falls back to PORPHANED — it never grows the table on demand
/// (a STOP tripwire).
///
/// PROCS-6 — this is now DERIVED from `MAX_PROCS` rather than written down beside it, because the old
/// literal 4 was not an independent choice: it was the process table's size, and the comment said so.
/// Once the cap rose to 6 the two numbers silently decoupled, and the decoupling is not benign — the
/// tables are COUPLED in the failure direction. Six killable rows against four kill slots means an
/// operator hammering `kill` across a full panel of vugs exhausts the kill table first; every request
/// past the fourth arms nothing, falls back to PORPHANED, and parks a row. Those parked rows are then
/// reclaimable only through KILLBOUND's quiescence witness, which is the narrower path (it needs the
/// victim's ASID drained to zero live EL0 tasks) — so the shortfall converts confirmable kills into
/// rows that wedge until their tasks happen to retire. That is the exact shape of the P60 wedge this
/// machinery exists to prevent, re-introduced by a capacity change one file away.
///
/// Keeping the slots `>=` the rows makes the shortfall unrepresentable: every row that can be killed
/// has a slot to be killed through, so `KILL_EXHAUSTED` can only ever be reached by concurrent
/// requesters racing on the SAME rows, never by capacity.
///
/// It is a literal rather than `= syscall::MAX_PROCS` for a boring reason: `syscall` is gated behind
/// `baremetal` and this module is not, so the constant cannot NAME the process table in every build
/// that compiles it. The coupling is enforced instead by a `const` assert in `syscall.rs` — the one
/// module that can see both constants, and the only configuration in which a `Proc` table exists at
/// all, hence the only one where a shortfall could mean anything. Drift fails the build there. The
/// assert is an inequality rather than an equality on purpose: a future arc may want kill headroom
/// ABOVE the row count, but never below it.
pub const MAX_KILL_REQS: usize = 6;

const KILL_FREE: u8 = 0;
/// Armed and owned by a live requester; the retiring task publishes `KILL_DONE` for it to observe.
const KILL_PENDING: u8 = 1;
/// The target retired. Terminal until the OWNER releases the slot (`kill_release`).
const KILL_DONE: u8 = 2;
/// The requester's bounded wait expired and it walked away, but the kill STAYS ARMED — the task is
/// still owed a death, and its row is parked PORPHANED meanwhile. Whoever finally retires the task
/// frees the slot directly (no owner is left to observe a `KILL_DONE`).
const KILL_DETACHED: u8 = 3;

struct KillReq {
    /// The target task id. Only meaningful while `state != KILL_FREE`.
    tid: AtomicU64,
    /// The target's address-space ASID, or 0 for a kernel task / a tid-only request. NON-ZERO makes the
    /// request ADDRESS-SPACE SCOPED: it names every EL0 thread sharing that ASID, not just `tid`. See
    /// `kill` for why that is the correct unit.
    asid: AtomicU64,
    /// `KILL_*`. The ownership token: a requester CASes FREE->PENDING to claim.
    state: AtomicU8,
}

static KILLS: [KillReq; MAX_KILL_REQS] = [const {
    KillReq {
        tid: AtomicU64::new(0),
        asid: AtomicU64::new(0),
        state: AtomicU8::new(KILL_FREE),
    }
}; MAX_KILL_REQS];

/// SKILL-1 — LIVE EL0 THREADS PER ADDRESS-SPACE SLOT, indexed by ASID.
///
/// An ELF-2 process can hold several tasks under one ASID (`spawn_user_thread`), and a kill must be scoped
/// to the ADDRESS SPACE, not to one thread: retiring the tid that `run_user_image` happens to know while
/// its siblings keep running would leave the program alive — still spinning, still rendering — under a row
/// we had just reported reaped. (There is no memory-unsafety in that: `teardown_user_slot` is refcounted
/// and only the last thread out retires the slot. The lie is in the accounting, which is worse for being
/// silent.) This counter is what lets the ASID-scoped request know when the LAST of the set has retired,
/// and it is the reason a confirmation can be trusted for a multi-threaded target.
///
/// Sized for ASID 0 (never counted — kernel tasks and the shared window) plus `boot::USER_SLOTS` (8).
/// Kept here rather than reading `boot::SLOT_REFCOUNT` because that is private, is baremetal-only, and
/// counts slot RETAINS (which include the pre-spawn retain `SYS_THREAD_SPAWN` takes before a task exists);
/// this counts TASKS, which is exactly the set a kill has to drain.
const KILL_ASID_SLOTS: usize = 9;
static ASID_THREADS: [AtomicU32; KILL_ASID_SLOTS] =
    [const { AtomicU32::new(0) }; KILL_ASID_SLOTS];

/// Count a freshly spawned EL0 task against its slot. Called from both user-task spawn paths BEFORE the
/// run-queue push, so the task is countable before it can ever be dispatched. Kernel tasks (ASID 0) and
/// out-of-range ASIDs are ignored, which makes this inert on `virt` (every task there is a kernel thread).
#[cfg_attr(not(feature = "baremetal"), allow(dead_code))] // both user-spawn paths are baremetal-only
fn asid_thread_enter(user_ttbr0: u64) {
    let asid = (user_ttbr0 >> 48) as usize;
    if asid != 0 && asid < KILL_ASID_SLOTS {
        ASID_THREADS[asid].fetch_add(1, Ordering::AcqRel);
    }
}

/// Retire an EL0 task from its slot's live count, returning how many threads REMAIN under that ASID.
/// Called from every task-death path (`exit` and the off-CPU `retire_killed`) beside the slot teardown, so
/// the count tracks live tasks exactly. Returns 0 for kernel tasks, which is also the right answer for a
/// tid-scoped request: "no siblings outstanding".
fn asid_thread_leave(user_ttbr0: u64) -> u32 {
    let asid = (user_ttbr0 >> 48) as usize;
    if asid == 0 || asid >= KILL_ASID_SLOTS {
        return 0;
    }
    let prev = ASID_THREADS[asid].fetch_sub(1, Ordering::AcqRel);
    debug_assert!(prev > 0, "asid_thread_leave: underflow (a task retired twice?)");
    prev.saturating_sub(1)
}

/// How many EL0 tasks are still live under `asid`. Introspection for the kill witness.
pub fn asid_live_threads(asid: u64) -> u32 {
    let a = asid as usize;
    if a == 0 || a >= KILL_ASID_SLOTS {
        return 0;
    }
    ASID_THREADS[a].load(Ordering::Acquire)
}

/// A requester's claim on one `KILLS` slot. Not `Copy`: it must be surrendered exactly once, via
/// `kill_release` (confirmed) or `kill_detach` (gave up), so a slot can never be double-freed.
pub struct KillTicket {
    idx: usize,
}

/// The index of the ARMED kill request naming this task, if any — matched by TID, or by ASID when the
/// request is address-space scoped (so every sibling thread of a killed process is caught by the same
/// request). Lock-free scan of a 4-entry table: in the common case every entry is `KILL_FREE` and this is
/// four acquire loads, cheap enough for the dispatch fast path.
fn kill_slot_for(tid: u64, user_ttbr0: u64) -> Option<usize> {
    let asid = user_ttbr0 >> 48;
    (0..MAX_KILL_REQS).find(|&i| {
        let st = KILLS[i].state.load(Ordering::Acquire);
        if st != KILL_PENDING && st != KILL_DETACHED {
            return false;
        }
        KILLS[i].tid.load(Ordering::Acquire) == tid
            || (asid != 0 && KILLS[i].asid.load(Ordering::Acquire) == asid)
    })
}

/// Settle slot `idx` from the RETIRING side — the last act of a killed task's teardown.
///
/// `remaining` is how many sibling threads still live under the request's ASID. A non-zero count means the
/// address space is NOT yet drained, so the request stays ARMED and un-settled: the siblings are caught by
/// the same request at their own next boundaries, and only the last one out settles the slot. That is what
/// makes a confirmation on a multi-threaded target honest rather than a claim about one thread of several.
///
/// The state transition is a **compare_exchange, not a load-then-store**. It races `kill_detach`, and does
/// so at precisely the worst moment — the requester's bounded wait expiring is exactly when a slow kill
/// lands. A load-then-store would read PENDING, lose to the detach's CAS, then stamp `KILL_DONE` over
/// `KILL_DETACHED`, stranding the slot in a terminal state with no owner left to release it: a permanently
/// consumed entry, four of which retire the primitive for the rest of the boot. With the CAS, losing the
/// race means the state is `KILL_DETACHED` and the slot is freed inline here — the owner-less path.
///
/// The `Proc`-row hook runs FIRST, while the slot is still un-settled: it reclaims a row that was parked
/// PORPHANED because this very kill did not confirm in time. Restricted to PORPHANED rows, so it can
/// never race the confirmed path (where the row is still PRUNNING and the requester frees it itself).
fn kill_settle(idx: usize, tid: u64, remaining: u32) {
    if remaining > 0 {
        return; // siblings still live under this ASID — the request stays armed for them
    }
    #[cfg(feature = "baremetal")]
    super::syscall::note_killed_task_retired(tid);
    #[cfg(not(feature = "baremetal"))]
    let _ = tid;
    if KILLS[idx]
        .state
        .compare_exchange(KILL_PENDING, KILL_DONE, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        // Not PENDING. Free the slot ONLY if it is genuinely owner-less (`KILL_DETACHED`) — and do that
        // with a CAS too, so a slot some other requester has meanwhile re-armed can never be clobbered by
        // a late settle. Any other state (already FREE because the owner retracted, already DONE) is
        // someone else's to dispose of; leave it alone.
        let _ = KILLS[idx].state.compare_exchange(
            KILL_DETACHED,
            KILL_FREE,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// Request that a task be killed. Returns a ticket to confirm against, or `None` if the request table is
/// full (the caller MUST fail closed — see `KILL_EXHAUSTED`). Pokes every online core so a killed task
/// sitting in an idle core's run queue is dispatched — and thus reaped — promptly rather than at that
/// core's next tick.
///
/// SCOPE. `tid` names the task the caller knows about; `asid` (non-zero) widens the request to EVERY EL0
/// task sharing that address space. Pass the ASID whenever the target is a user process: an ELF-2 program
/// may hold several tasks under one slot (`SYS_THREAD_SPAWN`), and killing only the tid the caller happens
/// to hold would leave the program running under a row the caller is about to reclaim. With the ASID set,
/// each sibling is matched by this same request at its own boundary and `kill_settle` withholds the
/// confirmation until the LAST of them has retired — so a confirmation means the PROCESS is gone, not one
/// thread of it. Pass 0 for a kernel task (no address space to scope to).
pub fn kill(tid: u64, asid: u64) -> Option<KillTicket> {
    if tid == 0 {
        return None;
    }
    for i in 0..MAX_KILL_REQS {
        if KILLS[i]
            .state
            .compare_exchange(KILL_FREE, KILL_PENDING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // Publish the target BEFORE any reader can match it: the CAS made the slot non-FREE, so
            // `kill_slot_for` could already be scanning it — it compares tid/asid, and a stale 0 simply
            // means "no match yet", never a match on the wrong task.
            KILLS[i].asid.store(asid, Ordering::Release);
            KILLS[i].tid.store(tid, Ordering::Release);
            // KILLBOUND — evict any target already PARKED in a kernel wait, strictly AFTER the publish
            // above (so `kill_slot_for` matches) and BEFORE the pokes (so a core woken by the poke finds
            // the evicted task already on its run queue and retires it in that same dispatch pass). A
            // parked task reaches neither SKILL-1 boundary on its own; without this the request stays
            // armed forever and the row it owns is immortal. See `futex_wake_killed`.
            let evicted = kill_wake_parked();
            if evicted > 0 {
                serial_println!(
                    "[killbound] kill tid={} asid={} — {} task(s) evicted from a kernel wait to reach the boundary",
                    tid, asid, evicted
                );
            }
            for cpu in 0..NUM_CPUS {
                if ONLINE_MASK[cpu].load(Ordering::Acquire) {
                    poke_cpu(cpu);
                }
            }
            return Some(KillTicket { idx: i });
        }
    }
    None
}

/// Has the target of `ticket` provably retired? `true` means it is off every CPU, its address-space
/// slot is torn down, and it will never execute again — the precondition for reclaiming its resources.
pub fn kill_confirmed(ticket: &KillTicket) -> bool {
    KILLS[ticket.idx].state.load(Ordering::Acquire) == KILL_DONE
}

/// Surrender a CONFIRMED ticket, returning the slot to the pool.
pub fn kill_release(ticket: KillTicket) {
    debug_assert_eq!(
        KILLS[ticket.idx].state.load(Ordering::Acquire),
        KILL_DONE,
        "kill_release on an unconfirmed ticket"
    );
    KILLS[ticket.idx].tid.store(0, Ordering::Release);
    KILLS[ticket.idx].asid.store(0, Ordering::Release);
    KILLS[ticket.idx].state.store(KILL_FREE, Ordering::Release);
}

/// RETRACT a request whose target turns out to be already gone — the requester has independently observed
/// that nothing is left alive to kill (for `run_user_image`: the `Proc` row reached `PEXITED` and the slot
/// holds no live threads). Without this, a request armed in the sampling gap between "row reads PRUNNING"
/// and `kill` — during which the task can complete its entire `SYS_EXIT` — could never be settled by
/// anyone, and four such leaks would exhaust the table for the rest of the boot.
///
/// CAS from `KILL_PENDING`, never a blind store: a retirement may be settling this very slot concurrently.
/// If the CAS fails the state is `KILL_DONE` (the kill landed after all) and the slot is freed anyway —
/// either way it returns to the pool exactly once.
pub fn kill_retract(ticket: KillTicket) {
    if KILLS[ticket.idx]
        .state
        .compare_exchange(KILL_PENDING, KILL_FREE, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        let _ = KILLS[ticket.idx].state.compare_exchange(
            KILL_DONE,
            KILL_FREE,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// Surrender an UNCONFIRMED ticket: the requester's bounded wait expired. The request stays ARMED — the
/// task is still owed its death and will take it at its next boundary — and the slot is freed by
/// `kill_settle` when that finally happens. The caller has already fallen back to PORPHANED.
///
/// WHICH TARGETS CAN HOLD A SLOT FOREVER — the honest limits of a boundary-driven kill:
///   * ~~A task blocked on a wake that never comes~~ — **CLOSED by KILLBOUND.** This was the real bug,
///     not a theoretical limit: a task parked on a futex nobody wakes is in neither a run queue nor on a
///     CPU, so neither arm saw it, and the operator's `kill` reported "armed but unconfirmed" forever
///     (P60, a `bg`'d vug whose worker threads never spawned and whose frame barrier therefore blocked in
///     `futex_wait` before its first present). It is now closed from BOTH sides: `Semaphore::wait` and
///     `futex_wait` test the armed-kill flag before parking, and `kill` evicts already-parked targets via
///     `kill_wake_parked`. The lock-order objection recorded here was WRONG — the eviction takes exactly
///     the lock `futex_wake`/`Semaphore::post` already take, in the same order, and calls `make_ready`
///     outside it; the park-side handoff releases that lock from the scheduler, so a requester spinning
///     on it always makes progress.
///   * A task that **never reaches any boundary at all** — no yield, no syscall, and no timer (the QEMU
///     case; on metal the quantum tick always arrives). This one is genuine and remains.
/// Both are bounded and visible rather than silent: the table is four entries, and the exhaustion that
/// results is witnessed once per boot by the caller (`KILL_EXHAUSTED`). Every OTHER path returns its slot
/// — a confirmed kill via `kill_release`, a detached-then-landed kill inline in `kill_settle`, and a
/// request armed against an already-dead task via the requester's own post-arm re-check.
pub fn kill_detach(ticket: KillTicket) {
    let _ = KILLS[ticket.idx].state.compare_exchange(
        KILL_PENDING,
        KILL_DETACHED,
        Ordering::AcqRel,
        Ordering::Acquire,
    ); // a DONE slot lost the race (the task died just now) — leave it for the owner-less free below
    if KILLS[ticket.idx].state.load(Ordering::Acquire) == KILL_DONE {
        KILLS[ticket.idx].tid.store(0, Ordering::Release);
        KILLS[ticket.idx].asid.store(0, Ordering::Release);
        KILLS[ticket.idx].state.store(KILL_FREE, Ordering::Release);
    }
}

/// SPINHUNT — the sentinel `tid` an ORPHAN-REAP request is armed with. Task ids are handed out by a
/// monotone counter that starts at 1, so `u64::MAX` can never name a live task: the request therefore
/// matches purely on its ASID, which is exactly its scope (every sibling thread of a dead leader).
const ORPHAN_KILL_TID: u64 = u64::MAX;

/// SPINHUNT — arm an OWNER-LESS, address-space-scoped kill against everything still alive under `asid`.
///
/// THE HOLE THIS CLOSES. `SYS_THREAD_SPAWN` gives an EL0 process several tasks under one slot, and
/// `boot::slot_thread_retain` keeps that slot alive until the LAST of them exits. Nothing in the kernel
/// ever made the last one exit. When a leader called `SYS_EXIT` without joining its workers — which is
/// not a userspace bug but the DOCUMENTED VUGGUARD path, since joining a worker that is not answering
/// would park the parent forever — the workers simply kept running. A worker whose release signal is a
/// yield-poll (`user-vug`'s `uvug_worker`, and any barrier of that shape) then burns its pinned core at
/// 100% for the rest of the boot against a parent that no longer exists.
///
/// AND IT IS SELF-SEALING WITHOUT THIS. The orphan holds a `THREAD_TABLE` row, and KILLBOUND's scavenge
/// of that table is gated on `ASID_GEN[owner]` having been bumped — which happens on the slot's 1->0
/// teardown edge, which requires the last thread under the slot to exit, which is the very thing the
/// orphan will never do. So the leak is permanent by construction: the row is unreclaimable, the slot is
/// unrecyclable, and the core stays pegged. Exactly the sustained one-core load with a clean `jobs`
/// table (the leader's `Proc` row IS reaped; an orphaned worker never had one) that P61 observed.
///
/// WHY THE EXISTING MACHINERY, AND WHY DETACHED. `kill(_, asid)` is already address-space scoped: every
/// sibling is matched by the one request at its own next boundary (an EL0 syscall — a yield-polling
/// worker reaches one every pass — or a preemption on metal), and `kill_settle` withholds settlement
/// while any sibling is still live. But there is no requester here to confirm to and none to hand the
/// slot back: the leader is on its way into `exit()`. So the ticket is DETACHED immediately, which is
/// the owner-less path `kill_settle` already implements — `kill_slot_for` still matches a `KILL_DETACHED`
/// request, and the LAST orphan out CASes the slot `KILL_DETACHED -> KILL_FREE` inline. The request
/// therefore returns to the four-entry pool exactly once, with nobody holding it.
///
/// QUIESCENCE IS PRESERVED, NOT WEAKENED. Nothing is reclaimed here. Each orphan retires through its own
/// `exit()`/`retire_killed`, decrementing `SLOT_REFCOUNT` itself; only when that reaches zero does
/// `teardown_user_slot` bump `ASID_GEN`, and only then does the KILLBOUND scavenge consider the row dead.
/// This fix makes that edge REACHABLE; it does not move it earlier.
///
/// Returns `true` if a request was armed. `false` means the four-entry table is full — the honest,
/// bounded failure the caller must WITNESS rather than paper over (the orphans then live on until some
/// later kill happens to name their ASID, exactly as before this fix).
pub fn orphan_kill(asid: u64) -> bool {
    if asid == 0 {
        return false;
    }
    match kill(ORPHAN_KILL_TID, asid) {
        Some(ticket) => {
            // Surrender it at once: the request stays ARMED (`KILL_DETACHED` matches in
            // `kill_slot_for`) and frees itself when the last orphan under this ASID retires.
            kill_detach(ticket);
            true
        }
        None => false,
    }
}

/// ON-CPU kill boundary. If the task running on THIS core has been killed, retire it here and now —
/// `exit()` never returns, which is exactly what the M6b EL0 fault-kill path already does from the
/// synchronous exception handler. No-op (and just four relaxed loads) otherwise.
///
/// Callers must be at a point where switching away for good is safe: IRQ-masked, on the task's own
/// kernel stack, holding no scheduler lock. `timer_preempt`, `yield_now` and the SVC dispatcher all
/// qualify — each already tolerates a `switch_context` that may never come back.
pub fn kill_check_current() {
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *const Task;
    if raw.is_null() {
        return;
    }
    let (id, ttbr0) = unsafe { ((*raw).id, (*raw).user_ttbr0) };
    if kill_slot_for(id, ttbr0).is_some() {
        exit();
    }
}

/// OFF-CPU kill boundary: retire `task` from the scheduler's own context, without ever switching into
/// it. Mirrors `exit()`'s teardown — slot teardown first (it repoints THIS core's TTBR0 off the dead root
/// before the ASID is flushed), then the joiner signal, then the Box drop that frees the kernel stack —
/// and settles the kill slot LAST.
///
/// ORDERING NOTE (the two arms differ, deliberately): here the kernel stack is already freed when
/// `KILL_DONE` is published, because the scheduler owns the Box and can drop it outright. The `exit()` arm
/// cannot — it is still executing ON that stack — so there the confirmation is published with the state
/// store and the final `switch_context` still to come, and the stack is freed a moment later by whichever
/// core reclaims the Box. The guarantee both arms actually make, and the only one a requester may rely on,
/// is: **the address-space slot is torn down, the joiner is released, and the task will never execute
/// again.** Kernel-stack reclamation is scheduler-internal and is NOT part of the confirmation contract.
fn retire_killed(idx: usize, task: Box<Task>) {
    let tid = task.id;
    let ttbr0 = task.user_ttbr0;
    // SPREAD-3: the off-CPU reap arm — release this task's committed EL0 residency. Mirrors the
    // `exit()` arm exactly (same `user_entry != 0` EL0 test, same recorded `cpu`), because a killed
    // vug frees its core just as thoroughly as one that returned, and a resident that is never
    // released would permanently bias placement away from a core that is in fact idle.
    if task.user_entry != 0 {
        el0_resident_leave(task.cpu as usize);
        // SPREAD-10: and its slot-residency credit — a dead sibling must stop attracting its triple.
        slot_res_leave(task.cpu as usize, task.user_ttbr0);
    }
    // M6d: the same slot retirement `exit()` performs, executed on the scheduler stack. Legal here for
    // the same reason it is legal there — the kernel half of every root is Global and identical, so
    // repointing TTBR0 to the boot root pulls nothing out from under the running (scheduler) context.
    #[cfg(feature = "baremetal")]
    {
        let asid = task.user_ttbr0 >> 48;
        if asid != 0 {
            unsafe { super::boot::teardown_user_slot(asid) };
        }
    }
    // Release any joiner: a killed task must not leave a `JoinHandle` blocked forever. Same single-post
    // discipline as `exit()` — borrow, never move, so the Box's own Arc keeps the semaphore alive.
    if let Some(sem) = &task.done_sem {
        sem.post();
    }
    task.state.store(STATE_FINISHED, Ordering::Release);
    drop(task); // frees the kernel stack
    // Drop this task out of its slot's live count, then settle — which withholds the confirmation while
    // sibling threads of the same address space are still alive (they are caught by this same request).
    let remaining = asid_thread_leave(ttbr0);
    kill_settle(idx, tid, remaining);
}

/// Terminate the current task: mark it finished and switch to the scheduler for good (which frees
/// its stack). Never returns; called automatically when a task's entry returns.
pub fn exit() -> ! {
    let cpu = percpu::this_cpu().cpu_index as usize;
    mask_irq();
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    assert!(!raw.is_null(), "exit: no current task");
    unsafe {
        // M6d (baremetal only): if this task owns a private address-space slot (ASID != 0), tear it down
        // HERE — repoint this core's TTBR0 off the dead slot root and broadcast-invalidate its ASID (see
        // `boot::teardown_user_slot`). Done before the switch-away, on the task's own kernel stack, which
        // is a Global identity mapping present in the boot root too — so repointing TTBR0 to the boot
        // root does not pull the stack out from under us. Shared-window (ASID 0) and kernel (ttbr0 = 0)
        // tasks skip it. On `virt` (JC3) every task is a kernel thread (user_ttbr0 == 0), so there is no
        // slot to retire and the whole block is compiled out (it reaches into the Pi-gated `super::boot`).
        #[cfg(feature = "baremetal")]
        {
            let asid = (*raw).user_ttbr0 >> 48;
            if asid != 0 {
                super::boot::teardown_user_slot(asid);
            }
        }
        // Signal completion to any joiner — the SINGLE post point for `done_sem`, covering BOTH a kernel
        // thread's normal `entry` return (via `task_trampoline`) and an EL0 thread's `SYS_THREAD_EXIT`
        // (ELF-2), which reaches `exit()` directly from the syscall handler. BORROW `done_sem` (never move
        // it): the Box's own Arc clone is this `post()`'s liveness anchor and must stay in the Box until the
        // scheduler drops it on the Finished path — strictly AFTER this post (we `switch_context` away just
        // below; the scheduler then reclaims and drops the Box). Only joinable spawns set `done_sem`; every
        // other task carries `None`, so this is a no-op for them (byte-identical to the pre-ELF-2 path).
        if let Some(sem) = &(*raw).done_sem {
            sem.post();
        }
        // SKILL-1: drop this task out of its address space's live-thread count — on EVERY exit path, not
        // just the killed one, or the count drifts and a later ASID-scoped kill would never confirm.
        let remaining = asid_thread_leave((*raw).user_ttbr0);
        // SPREAD-3: the on-CPU reap arm — release this task's committed EL0 residency. `user_entry != 0`
        // is exactly the EL0 test used at placement (kernel threads are constructed with `user_entry: 0`),
        // so increment and decrement cover the same population. `exit()` is the single funnel for every
        // non-killed retirement — `sys_exit`, `SYS_THREAD_EXIT`, the M6b fault-kill, `kill_check_current`
        // and a kernel entry's return all land here — so one decrement here covers them all.
        if (*raw).user_entry != 0 {
            el0_resident_leave((*raw).cpu as usize);
            // SPREAD-10: release the slot-residency credit on the same funnel, or a retired triple
            // member would keep pulling its siblings toward a core it no longer runs on.
            slot_res_leave((*raw).cpu as usize, (*raw).user_ttbr0);
        }
        // Settle any kill request naming this task (by tid, or by ASID for a whole-process kill). Placed
        // HERE — after the slot teardown and the joiner post, with only the state store and the final
        // switch left.
        //
        // ORDERING (asymmetric with the off-CPU arm, deliberately — see `retire_killed`): this task is
        // still executing on its own kernel stack, so that stack is NOT yet freed when `KILL_DONE` is
        // published; the scheduler reclaims the Box a moment later, after the `switch_context` below.
        // What IS guaranteed at this point — and all the requester's contract claims — is that the
        // address-space slot is retired, the joiner is released, and this task will never execute again.
        if let Some(idx) = kill_slot_for((*raw).id, (*raw).user_ttbr0) {
            kill_settle(idx, (*raw).id, remaining);
        }
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
    // SCHED-2: drive the periodic load heartbeat off the per-core timer tick (metal-only — QEMU never
    // reaches here). Placed before the current-null return so an idle-but-ticking core still advances
    // the aggregate cadence; the emit itself is change-only and single-core per window (see the fn).
    load_witness_tick();
    let cpu = percpu::this_cpu().cpu_index as usize;
    // WEDGE-4 W4-A — the fix's own tripwire. Every run-queue section now runs IRQ-masked (`rq`), so a
    // timer IRQ can no longer land inside one and this word must read 0 here. A line from it means the
    // discipline has been breached again — i.e. some acquisition reached the lock without masking.
    let section = IN_RQ_SECTION[cpu].load(Ordering::Acquire);
    if section != 0 && W4A_PRINTS.fetch_add(1, Ordering::Relaxed) < W4A_PRINT_MAX {
        w4_str("\r\n[wedge4] preempt-in-section core=");
        w4_dec(cpu as u64);
        w4_str(" queue=");
        w4_dec((section >> 32) - 1);
        w4_str(" tid=");
        w4_dec(section & 0xffff_ffff);
        w4_str("\r\n");
    }
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    if raw.is_null() {
        return; // scheduler/idle context, or an unscheduled core (the BSP)
    }
    // SKILL-1 on-CPU kill boundary — the load-bearing one. The quantum tick is the ONLY involuntary
    // boundary a spinning EL0 task ever reaches (it makes no syscalls and never yields), so this is what
    // turns "parked PORPHANED but still burning a core at 100%" into an actual death. Checked BEFORE the
    // quantum countdown so a killed task dies at the very next tick rather than at its next full quantum.
    // Never returns if this task has been killed; the abandoned IRQ frame is harmless — EOI already ran
    // (see `gic::handle_irq`) and the stack it sits on is freed with the task's Box.
    kill_check_current();
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

/// SPREAD-9 — preempt at IPI RECEIPT: the wake SGI's answer to `preempt_hint`'s service arm. Called
/// from `gic::handle_irq` AFTER EOI when the acked INTID was an SGI, on the core the SGI targeted —
/// the exact position `timer_preempt` occupies for the timer PPI, and deliberately so: it reuses
/// `timer_preempt`'s context-switch machinery verbatim (mark the incumbent READY, `switch_context`
/// to the scheduler from the IRQ frame; the task resumes here IRQ-masked when re-dispatched and
/// unwinds back through `__vec_irq`'s epilogue, which restores its banked ELR/SPSR/SP_EL0). No new
/// switch path exists.
///
/// The policy, and the bound: ONE preemption per IPI, service band only. The pending band is
/// consumed with a single `swap(0)`, so an SGI can trigger at most one dispatch; the band is only
/// ever armed by a service-band wake that found a LOWER-band incumbent, so the fleet (PRIO_NORMAL)
/// can never preempt itself and no wake-storm churn regression is possible — an EL0 wake takes
/// exactly the SPREAD-7/8 path it took before this arc. Equal-band service wakes never arm the kick
/// (`prio > cur` in the hint), keeping SPREAD-8's approved one-tick policy for that arm.
///
/// Why dispatching here is sound, point by point:
///   * RqGuard/WEDGE-4: every run-queue section runs IRQ-masked (`rq`), so this IRQ cannot have
///     landed inside one; the `IN_RQ_SECTION` check below is the same tripwire `timer_preempt`
///     carries, made load-bearing — on a breach we DECLINE the switch (the armed quantum trim
///     still bounds the wake at one tick) rather than dispatch over a torn queue.
///   * Nesting: the IRQ vector runs with IRQ masked end to end and `switch_context` banks DAIF, so
///     there is no nested-IRQ context to dispatch from — exactly the `timer_preempt` situation.
///   * The re-check against `CUR_PRIO` is this core's OWN word (published by `dispatch_next`), so
///     a stale hint — the incumbent already switched to a service task by the time the SGI lands —
///     declines instead of preempting the band with itself.
///
/// On QEMU raspi4b the SGIs are live (unlike the timer PPI), so the gate exercises this path end to
/// end; the LATENCY effect it exists for is metal-only, as with every preemption behaviour here.
pub fn ipi_preempt() {
    if !SCHED_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let cpu = percpu::this_cpu().cpu_index as usize;
    let band = KICK_BAND[cpu].swap(0, Ordering::Relaxed);
    if band == 0 {
        return; // ordinary wake ping — breaking the WFI was the whole job
    }
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    if raw.is_null() {
        return; // scheduler/idle context — the interrupted dispatch loop picks the wake up itself
    }
    if CUR_PRIO[cpu].load(Ordering::Relaxed) >= band {
        return; // incumbent is already at/above the woken band — nothing to reclaim
    }
    if IN_RQ_SECTION[cpu].load(Ordering::Acquire) != 0 {
        return; // discipline breach (never expected): decline; the quantum trim still bounds the wake
    }
    // SKILL-1 on-CPU kill boundary, exactly as at the top of `timer_preempt`'s switch arm: this is a
    // legitimate involuntary boundary, so a killed incumbent dies here rather than running on.
    kill_check_current();
    SPREAD9_KICK.fetch_add(1, Ordering::Relaxed);
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
    // SCHED-7: start each pass with no busy span recorded; the busy branch below overwrites this with
    // the task's measured execution span. An empty pass leaves it 0, so `run()` folds the whole pass
    // (drain + this empty dispatch + the following WFI/poll-spin) in as idle.
    PASS_BUSY_CYC[cpu].store(0, Ordering::Relaxed);
    // AARCH64-PRIO — age then pick, under ONE run-queue lock acquisition. Count this dispatch pass
    // (the aging clock) and, ~every AGING_INTERVAL passes, run the anti-starvation sweep BEFORE the
    // pop so a long-waiting task cannot be dispatched before it is aged in the same pass. The sweep
    // and pop share the lock; `age` carries surplus credit past `AGE_TICKS`, so a coarse cadence loses
    // nothing. Owning-CPU-only counters (Relaxed). See `AGE_TICKS` for why the clock is passes, not ticks.
    let next = {
        let mut q = rq(cpu);
        let passes = SCHED[cpu].age_passes.fetch_add(1, Ordering::Relaxed) + 1;
        let elapsed = passes - SCHED[cpu].age_last_sweep.load(Ordering::Relaxed);
        if elapsed >= AGING_INTERVAL {
            // SCHED-PRIO: the sweep now reports how many relocations reached the interactive service
            // band. Folded into the witness counter here (still under the run-queue lock, one relaxed
            // add on a path that already took the lock) rather than inside `age`, which has no `cpu`.
            let into_band = q.age(elapsed.min(u32::MAX as u64) as u32);
            if into_band != 0 {
                PRIO_AGED_IN[cpu].fetch_add(into_band as u64, Ordering::Relaxed);
            }
            SCHED[cpu].age_last_sweep.store(passes, Ordering::Relaxed);
        }
        q.pop_highest() // highest-priority ready task; lock dropped here
    };
    let Some(mut task) = next else {
        CPU_IDLE[cpu].fetch_add(1, Ordering::Relaxed); // M3b CPU-pulse meter (introspection)
        // SCHED-5: idle TIME is measured at the WFI in `run()` (the span the core actually sleeps), not
        // here — this empty pass itself takes negligible time and returns straight to the idle loop.
        unmask_irq();
        return false;
    };
    // SKILL-1 OFF-CPU kill boundary. A killed task is retired HERE — after the pop (so it is off every
    // run queue), before the switch (so it never executes another instruction) and before any accounting
    // (a task that does not run is not a dispatch). This is the arm that needs no cooperation from the
    // target at all, and it covers ready, sleeping and wait-queued tasks alike: they all come back
    // through a dispatch. IRQ is masked and the run-queue lock was dropped above, so the teardown inside
    // is on the same footing as `exit()`'s. Returning `true` re-enters the loop for a real pass.
    if let Some(idx) = kill_slot_for(task.id, task.user_ttbr0) {
        retire_killed(idx, task);
        return true; // leaves IRQ MASKED, exactly like the ordinary busy return below
    }
    CPU_BUSY[cpu].fetch_add(1, Ordering::Relaxed); // M3b CPU-pulse meter (introspection)
    // SCHED-2/SCHED-5: busy dispatch — one context switch into a task; record it + the last-task
    // identity. The task's EXECUTION TIME is folded into the window AFTER `switch_context` returns
    // (below), when the elapsed CNTPCT span is known. Single-writer (this core), relaxed; no lock added.
    ACCT[cpu].ctx_switches.fetch_add(1, Ordering::Relaxed);
    ACCT[cpu].note_last(task.id, task.name);
    // SCHED-PRIO — the dispatch-share witness, and the running-priority publication that makes
    // `preempt_hint` possible. BASE priority is what is counted and published: a task the aging sweep
    // lifted is a below-band task that the ANTI-STARVATION path is running, not a band win, and it
    // must not be able to shield itself from a real service wake by wearing the band's number.
    let svc_band = task.priority >= PRIO_SERVICE;
    if svc_band {
        PRIO_SVC_DISPATCH[cpu].fetch_add(1, Ordering::Relaxed);
    }
    if task.user_entry != 0 {
        PRIO_EL0_DISPATCH[cpu].fetch_add(1, Ordering::Relaxed);
    }
    CUR_PRIO[cpu].store(task.priority, Ordering::Relaxed);
    // SPREAD-7: consume the wake stamp — this dispatch is the woken task first RUNNING, so the span
    // since `make_ready` is exactly the run-queue wait the wake paid (the quantity the quantized arm
    // of `preempt_hint` leaves unbounded below one quantum). Zeroed so a later preempt/requeue cycle
    // of the same task is not double-priced: this measures wake latency, not scheduling in general.
    if task.wake_cyc != 0 {
        let wait = now_cyc().saturating_sub(task.wake_cyc);
        task.wake_cyc = 0;
        if task.priority >= PRIO_SERVICE {
            // SPREAD-9: a service-band wake — price it in the svc_lat aggregates, NOT the SPREAD-7
            // ones, so the EL0 wake2disp population is exactly what it was before this arc. BASE
            // priority decides, mirroring the stamp site (`make_ready`) — an aged-up EL0 task was
            // stamped as EL0 and is priced as EL0.
            SPREAD9_SVC_SUM.fetch_add(wait, Ordering::Relaxed);
            SPREAD9_SVC_N.fetch_add(1, Ordering::Relaxed);
            SPREAD9_SVC_MAX.fetch_max(wait, Ordering::Relaxed);
        } else {
            SPREAD7_WD_SUM.fetch_add(wait, Ordering::Relaxed);
            SPREAD7_WD_N.fetch_add(1, Ordering::Relaxed);
            SPREAD7_WD_MAX.fetch_max(wait, Ordering::Relaxed);
        }
    }
    task.state.store(STATE_RUNNING, Ordering::Release);
    SCHED[cpu].quantum.store(QUANTUM_TICKS, Ordering::Relaxed);
    let raw = Box::into_raw(task);
    let entry_sp = unsafe { (*raw).ctx_sp };
    // Publish `current` (Release) strictly before switching in — the trampoline reads it Acquire.
    SCHED[cpu].current.store(raw as u64, Ordering::Release);
    // M6d: install the incoming task's address space. A user task carries `user_ttbr0` (`root | asid`);
    // a kernel task carries 0 (no switch — kernel mappings are Global and byte-identical in every root).
    // Read the live TTBR0 and write only if the FULL 64-bit value (root AND ASID) differs, so an ASID-0
    // shared-window task always switches away from a live non-zero slot ASID even if a base coincided.
    // Runs I-masked (dispatch masks IRQ) and BEFORE the eret, so EL0 always executes under the right
    // root. It is here — not in `user_task_trampoline` — because a preempted user task RESUMED through
    // `__vec_irq`'s restore tail (not the trampoline) must also get its root reinstalled every dispatch.
    let want_ttbr0 = unsafe { (*raw).user_ttbr0 };
    if want_ttbr0 != 0 {
        unsafe {
            let live: u64;
            core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) live, options(nomem, nostack, preserves_flags));
            if live != want_ttbr0 {
                core::arch::asm!(
                    "msr TTBR0_EL1, {}",
                    "isb",
                    in(reg) want_ttbr0,
                    options(nostack, preserves_flags),
                );
            }
        }
    }
    // SCHED-5: bracket the task's execution with a CNTPCT read. The span from here to the switch-back
    // is exactly the wall-clock time this core spent running the dispatched task (plus any IRQ handling
    // that fired while it ran) — the "busy" time for time-based load accounting. Two sysreg reads per
    // dispatch, off the per-instruction path.
    let busy_t0 = now_cyc();
    // PULSE-5: publish that anchor before switching in, so the span is READABLE while it is still
    // running instead of only after it ends. One relaxed store, no sysreg read of its own (it
    // reuses the `busy_t0` this path already took), no ordering constraint on the switch. It is
    // cleared by the fold below (`account`). This single store is the whole cost of the fix on the
    // context-switch path.
    ACCT[cpu].run_t0.store(busy_t0, Ordering::Relaxed);
    unsafe {
        switch_context(SCHED[cpu].scheduler_sp.as_ptr(), entry_sp);
    }
    let busy_cyc = now_cyc().wrapping_sub(busy_t0);
    // The switch-back always lands IRQ-masked (yield_now/exit mask first; timer_preempt runs in the
    // auto-masked IRQ handler), so the Box reclaim below can't race a re-entrant preempt on this
    // core. Re-assert the mask explicitly so that safety doesn't rest on an inherited DAIF that a
    // future switch-in path could leave enabled.
    mask_irq();
    // Fold this task's execution span into the rolling load window; SPREAD-9 — tag the span with its
    // band so `el0_busy_pct` can subtract service time from what EL0 placement weighs.
    ACCT[cpu].account(busy_cyc, 0, if svc_band { busy_cyc } else { 0 });
    // SCHED-7: publish this pass's busy span so `run()` can subtract it from the pass's total wall
    // span and fold the remainder (scheduler overhead, then the WFI/poll-spin) in as idle.
    PASS_BUSY_CYC[cpu].store(busy_cyc, Ordering::Relaxed);
    // SCHED-PRIO: back on the scheduler stack — nothing is running here, so nothing is preemptible.
    // Published alongside the `current` clear it mirrors, and for the same reason.
    CUR_PRIO[cpu].store(PRIO_NONE, Ordering::Relaxed);
    SCHED[cpu].current.store(0, Ordering::Release);
    // Consume the park action exactly once: read it and immediately reset to NONE, so a stale action
    // can never leak into the next task's switch-back. Only a task that switched back BLOCKED carries
    // a meaningful action.
    let park = SCHED[cpu].park_kind.swap(PARK_NONE, Ordering::Relaxed);
    let mut task = unsafe { Box::from_raw(raw) };
    match task.state.load(Ordering::Acquire) {
        STATE_FINISHED => drop(task), // free the stack
        STATE_BLOCKED => park_blocked(cpu, park, task), // sleeper list / (M4b) a wait queue
        _ => {
            // READY (yielded or preempted): re-enqueue at its BASE priority level (round-robin within),
            // which also re-zeroes its aging clock — a task only ages while it sits WAITING.
            debug_assert_eq!(park, PARK_NONE, "non-blocked task carried a park action");
            task.state.store(STATE_READY, Ordering::Release);
            // SPREAD-11 (2026-07-30, the P87/P92/P93/P94 idle-desktop livelock): a task that YIELDS
            // instead of parking never passes `make_ready`, so SPREAD-10's co-placement never sees
            // it — a spread vug triple that yield-spins its rendezvous ([spread10] 3c+=1 co_moves=0)
            // storms the run-queue locks at wake-speed (P94 measured ctx +1.3M/win, svc=0, and
            // rx-backstop starved 199 s inside make_ready). Give the READY re-enqueue the SAME
            // refresh clock the park path has: at most once per PLACE_REFRESH_MS, ask rewake_place
            // and move toward the slot's residents. Credits move exactly as SPREAD-10's rewake does.
            let mut dest = cpu;
            if task.user_entry != 0 {
                let now = now_cyc();
                if task.place_cyc != 0 && now.saturating_sub(task.place_cyc) >= place_refresh_cyc() {
                    task.place_cyc = now;
                    let target = rewake_place(cpu, slot_of(task.user_ttbr0));
                    if target != cpu {
                        el0_resident_leave(cpu);
                        let _ = el0_resident_enter(target);
                        slot_res_leave(cpu, task.user_ttbr0);
                        slot_res_enter(target, task.user_ttbr0);
                        task.cpu = target as u32;
                        SPREAD11_YIELD_MOVES.fetch_add(1, Ordering::Relaxed);
                        dest = target;
                    }
                }
            }
            rq(dest).push(task);
        }
    }
    true
}

/// VUG-1 M3b: number of CPUs the "CPU pulse" meter should show. Arch-neutral mirror of the x86 accessor.
pub fn meter_cpu_count() -> usize {
    { #[cfg(feature = "tegra")] { percpu::METER_CPU_COUNT } #[cfg(not(feature = "tegra"))] { NUM_CPUS } } // VUGFIX: tegra DISPLAYS 6 (DTB /cpus), not the 8-slot array bound; one line => pi/virt byte-identical
}

/// VUG-1 M3b: cumulative `(busy, idle)` dispatch/idle counts for `cpu` (see `CPU_BUSY`/`CPU_IDLE`).
/// The demo diffs these across a frame window to derive a per-core load fraction. Introspection only.
pub fn meter_cpu_ticks(cpu: usize) -> (u64, u64) {
    if cpu >= NUM_CPUS {
        return (0, 0);
    }
    (CPU_BUSY[cpu].load(Ordering::Relaxed), CPU_IDLE[cpu].load(Ordering::Relaxed))
}

/// VUG-HONESTY: the linear index of the core CALLING this — the "demo core" the vug/pulse render loop
/// runs on. The CPU-pulse display credits its own render load ONLY to this core; every other core whose
/// counters are frozen this window is parked (reads parked, never the demo core's load). Introspection
/// only — a `TPIDR_EL1/EL2` self-lookup, no scheduling-path effect.
pub fn meter_current_cpu() -> usize {
    percpu::this_cpu().cpu_index as usize
}

/// VUG-1 M3b: register `cpu` as idle for the CPU-pulse meter — the seam a core that parks WITHOUT
/// running this scheduler (`smp_virt::__secondary_rust_virt`'s WFI park: comes online, publishes
/// `CORE_READY`, never calls `dispatch_next`) uses to bump its own `CPU_IDLE`. Without it such a core
/// stays `(CPU_BUSY, CPU_IDLE) == (0, 0)` and the meter shows a pinned/undefined bar for a
/// demonstrably-online-idle core; one heartbeat makes it read honest 0% busy. Same contract as the
/// other pulse counters: introspection only, lock-free relaxed, never read on any scheduling path.
pub fn note_core_idle(cpu: usize) {
    if cpu >= NUM_CPUS {
        return;
    }
    CPU_IDLE[cpu].fetch_add(1, Ordering::Relaxed);
}

/// Park a task that switched back BLOCKED, per the action it set before switching. Runs in the
/// scheduler context with IRQ masked and owns `task`.
fn park_blocked(cpu: usize, park: u8, mut task: Box<Task>) {
    // SPREAD-5: stamp the park instant while we still exclusively own the Box and BEFORE it is handed
    // to a wait queue or the sleeper list (after which another core may take it at any moment). This is
    // the sole park funnel, so this is the only writer; `make_ready` is the sole reader. Stamped for
    // every task, not just EL0 — one register write on a path that is already switching contexts, and
    // it keeps the field's meaning unconditional rather than "valid only for some tasks".
    task.park_cyc = now_cyc();
    // SPREAD-4: this is the SOLE park funnel — every blocking primitive an EL0 task can reach
    // (`Semaphore::wait`, `futex_wait`, `sleep_ticks`) marks itself BLOCKED, sets `park_kind` and
    // switches back into `dispatch_next`, which lands here. A parked resident owes its core no CPU, so
    // it stops counting towards `el0_active` for as long as it sleeps; `make_ready` puts it back.
    let el0_home = if task.user_entry != 0 { Some(task.cpu as usize) } else { None };
    if let Some(home) = el0_home {
        el0_parked_enter(home);
    }
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
            // SPREAD-4: it is never coming back through `make_ready`, so undo the park credit AND
            // release its residency, or the core would carry a phantom resident for the rest of the boot.
            if let Some(home) = el0_home {
                el0_parked_leave(home);
                el0_resident_leave(home);
                slot_res_leave(home, task.user_ttbr0); // SPREAD-10: same phantom-credit reasoning
            }
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

/// VUGPAUSE-2: timer ticks between two runs of the input-wait backstop. A tick is ~4 ms, so 64 is ~256 ms
/// — four wake/poll/re-park cycles per second for an idle vug. That is two decimal orders inside the
/// tightest liveness bound it has to satisfy (UVUG-8r2's 2 s takeover heartbeat) and far below what a load
/// meter can resolve, which is the whole point: the vug keeps its old "I am still polling" contract with
/// the watchdogs while costing effectively nothing.
#[cfg(feature = "baremetal")]
const INPUT_WAIT_BACKSTOP_TICKS: u64 = 64;

/// VUGPAUSE-2: the tick at which the next backstop pass is due. Global rather than per-CPU, and claimed by
/// CAS, so the cadence is ONE pass per period across the whole machine and not one per core — six cores
/// each waking every parked vug would be six times the work for exactly the same effect.
#[cfg(feature = "baremetal")]
static INPUT_WAIT_BACKSTOP_DUE: AtomicU64 = AtomicU64::new(0);

/// VUGPAUSE-2: run the input-wait backstop if its period has elapsed. Called from the scheduler loop top,
/// beside `drain_due_sleepers` and for the same structural reason — it is a periodic re-ready pass, it
/// needs `make_ready`, and this is the one place in the kernel that runs forever on every core with IRQs
/// masked and no lock held.
///
/// Metal-only by construction, and deliberately so: `timer::ticks()` advances from the timer IRQ, which
/// QEMU raspi4b never delivers, so under QEMU this is a load of an atomic that never changes. Nothing is
/// lost there — the headless run has no HID, so no vug ever freezes, so nothing ever parks.
///
/// Gated on `baremetal` because the thing it wakes is: `arch::aarch64::syscall` — which owns the input
/// rings and therefore `SYS_INPUT_WAIT` — is itself a `baremetal`-only module. Without EL0 there is no
/// input ring, nothing can park on one, and the backstop has nothing to do.
#[inline]
fn input_wait_backstop() {
    #[cfg(not(feature = "baremetal"))]
    return;
    #[cfg(feature = "baremetal")]
    {
        input_wait_backstop_inner();
    }
}

#[cfg(feature = "baremetal")]
#[inline]
fn input_wait_backstop_inner() {
    let now = super::timer::ticks();
    let due = INPUT_WAIT_BACKSTOP_DUE.load(Ordering::Relaxed);
    if now < due {
        return;
    }
    // Claim the period. A loser simply skips; it does not spin or retry.
    if INPUT_WAIT_BACKSTOP_DUE
        .compare_exchange(
            due,
            now.wrapping_add(INPUT_WAIT_BACKSTOP_TICKS),
            Ordering::AcqRel,
            Ordering::Relaxed,
        )
        .is_err()
    {
        return;
    }
    super::syscall::el0_input_wake_backstop();
}

/// Run `cpu`'s run queue to completion, cooperatively (the M3a demo driver on the BSP): dispatch
/// tasks until the queue drains, then return. Used before preemption is enabled, so tasks only
/// switch via `yield_now`/`exit`. It does NOT drain the sleeper list, so a task that `sleep_ticks`
/// (or otherwise blocks) here would be parked and never re-dispatched — the blocking primitives are
/// exercised on the APs' `run()` loop, which does service sleepers.
pub fn run_until_empty(cpu: usize) {
    while dispatch_next(cpu) {}
}

// ---------------------------------------------------------------------------------------------
// SMP-BAL — work stealing: an idle core pulls a steal-eligible task off the most-loaded core
// ---------------------------------------------------------------------------------------------
//
// Placement (`pick_cpu`, SCHED-3) balances at SPAWN, but a task's cost is not known then and wake
// bursts pile work unevenly AFTER placement (the metal P45 sighting: cores 1-3 "sort of" work but
// never balance). Stealing is the runtime correction: whenever a core finds its own run queue empty
// (in `run`), it looks for the most-loaded OTHER core and pulls ONE of its steal-eligible ready tasks
// over, so backlog drains onto idle silicon instead of waiting behind a saturated core's queue.
//
// Protocol (provably race-free against the existing per-core run-queue spinlocks):
//   1. VICTIM SELECT (lock-free peek): scan online cores != self, pick the one with the deepest run
//      queue whose depth is >= STEAL_MIN_DEPTH. The floor leaves the last task at home (no ping-pong: a
//      core with a single task keeps it) and means we only ever move genuine backlog.
//   2. STEAL (under the victim's lock ONLY): re-read depth (it may have drained since the peek) and, if
//      still >= the floor, `steal_one()` — remove the first steal-eligible ready task. Pinned tasks are
//      skipped. The lock is released before we touch our own queue, so only ONE run-queue lock is ever
//      held at a time — no lock-ordering hazard, no deadlock.
//   3. RE-HOME (we exclusively own the popped Box): retarget `task.cpu` to self so a later wake returns
//      it here (the no-migrate invariant is preserved for its NEW home), then push onto our own queue.
// Soundness: a task sitting in a run queue is always `STATE_READY` (running → out in `current`; blocked
// → in a wait/sleeper list), and both queues are touched only under their own lock with IRQ masked, so
// the steal can never race the victim's own `dispatch_next`/`push` or observe a half-built task. Only an
// idle core steals, and it steals at most one task per empty pass, so it self-limits.

/// Minimum victim run-queue depth to steal from. `2` leaves the last ready task at its home core (a core
/// with one task is not "loaded"), which prevents two idle cores from ping-ponging a lone task.
const STEAL_MIN_DEPTH: usize = 2;

/// SMP-BAL — rate limit for the `[smpbal] steal` witness: emit the first `STEAL_LOG_MAX` steals then go
/// quiet, so a steady rebalancing workload cannot flood the serial log. Introspection only.
#[cfg(feature = "pi")]
const STEAL_LOG_MAX: u32 = 12;
#[cfg(feature = "pi")]
static STEAL_LOG_COUNT: AtomicU32 = AtomicU32::new(0);

/// SMP-BAL — an idle `cpu` (empty local queue) tries to steal ONE steal-eligible ready task from the
/// most-loaded online core. Returns `true` if a task was moved onto `cpu`'s queue (the caller then loops
/// and dispatches it), `false` if there was nothing worth stealing. IRQ-masked throughout (matching the
/// run-queue lock contract); acquires at most one run-queue lock at a time.
fn try_steal(cpu: usize) -> bool {
    mask_irq();
    // 1. Peek for the deepest OTHER online queue at/above the floor (lock-free length reads).
    let mut victim: Option<usize> = None;
    let mut best_depth = STEAL_MIN_DEPTH - 1;
    for c in 0..NUM_CPUS {
        if c == cpu || !ONLINE_MASK[c].load(Ordering::Acquire) {
            continue;
        }
        let depth = rq(c).len();
        if depth > best_depth {
            best_depth = depth;
            victim = Some(c);
        }
    }
    let Some(v) = victim else {
        unmask_irq();
        return false;
    };
    // 2. Steal under the victim's lock only (re-check depth — it may have drained since the peek).
    let stolen = {
        let mut vq = rq(v);
        if vq.len() < STEAL_MIN_DEPTH {
            None
        } else {
            vq.steal_one()
        }
    };
    let Some(mut task) = stolen else {
        unmask_irq();
        return false;
    };
    // 3. Re-home onto this core and enqueue (we exclusively own the Box here).
    let name = task.name;
    task.cpu = cpu as u32;
    rq(cpu).push(task);
    // Rate-limited steal witness (pi-gated: fires on the target + kernel8-test, byte-identical elsewhere).
    #[cfg(feature = "pi")]
    if STEAL_LOG_COUNT.fetch_add(1, Ordering::Relaxed) < STEAL_LOG_MAX {
        serial_println!(":: [smpbal] steal '{}' c{}->c{} ::", name, v, cpu);
    }
    #[cfg(not(feature = "pi"))]
    let _ = name;
    unmask_irq();
    true
}

/// SMP-BAL — the BSP's entry into the scheduler, after it finishes its one-time boot duties (service
/// spawn + deferred USB enumerate). Mirrors the APs' `wait_and_run`→`run`, minus the `SCHED_GO` wait (the
/// BSP is the core that SET `SCHED_GO` in `start_aps`, so scheduling is already live). Registers core 0
/// as an online, scheduling core (so `CPU_AUTO` placement and stealing may use it) and enters the loop;
/// never returns. Replaces the historical `hlt_loop()` the Pi/tegra BSP parked in.
///
/// Why this is safe (the "BSP never schedules" invariants audited):
///   * MMU/percpu/vectors: all installed on the BSP long before it reaches here (boot path).
///   * GIC/IRQ routing: distributor config is BSP-only and already done; the PL011 RX SPI is routed to
///     the input core, not core 0. The BSP running scheduled tasks adds no distributor state.
///   * ms-clock: the BSP's periodic timer IRQ still runs its tick bookkeeping BEFORE `timer_preempt`, so
///     the global clock keeps advancing whether or not a task is preempted on core 0.
///   * meter sentinel: the `c0 = --` display is `tracked()` returning false because core 0 never folded a
///     load span. Once core 0 runs this loop it folds a busy/idle span every pass → `tracked()` true →
///     the `SCHED: load` line and `top` show core 0's REAL utilization (the P45 `c0 = --` artifact heals).
///   * only steal-eligible KERNEL tasks (`CPU_AUTO`) can land on core 0 (via placement or steal); pinned
///     EL0/render/input tasks never do, so no ASID-0/per-core-window assumption is disturbed.
pub fn run_bsp(cpu: usize) -> ! {
    mark_online(cpu);
    run(cpu)
}

/// The APs' scheduler loop: dispatch ready tasks forever, idling (WFI on metal / poll in QEMU, via
/// `arch::hlt`) when the queue is empty until the timer/an IPI makes work. Never returns.
fn run(cpu: usize) -> ! {
    // SCHED-7: the wall-clock anchor for time-based load accounting. Every cycle between one pass's
    // anchor and the next is either a task's EXECUTION span (folded in as busy by `dispatch_next`) or
    // the core AWAITING work (WFI, poll-spin, sleeper drain, run-queue management) — which is idle.
    let mut t_prev = now_cyc();
    loop {
        // Wake any sleepers whose deadline has passed (IRQ masked, matching the switch-back critical
        // section); `make_ready` pushes them onto THIS CPU's own run queue so the dispatch below
        // picks them up. The wake source is the free-running periodic timer — each tick breaks the
        // idle WFI and re-enters this loop — so an idle core with only a pending sleeper still makes
        // progress; worst-case wake latency is one tick. `dispatch_next` re-masks (redundant here),
        // then either switches into a task or, on an empty queue, unmasks and returns false to idle.
        mask_irq();
        drain_due_sleepers(cpu);
        input_wait_backstop();
        if !dispatch_next(cpu) {
            // SMP-BAL: local queue is empty — before parking, try to pull a steal-eligible task off the
            // MOST-loaded core (work redistribution). A successful steal enqueues onto THIS core, so we
            // loop straight back and `dispatch_next` runs it; nothing is parked. This is what lets an
            // idle core (incl. the BSP once it runs `run_bsp`) drain a saturated core's backlog. Only an
            // idle core ever steals, so it never competes with useful local work.
            if !try_steal(cpu) {
                // Nothing to steal either: park until the next tick/IPI — WFI on metal, a light poll-spin
                // in QEMU (no Group-1 timer). This span is idle; it is folded in below along with the
                // rest of the pass, so it does not matter whether WFI actually sleeps or returns at once.
                crate::arch::hlt();
            }
        }
        // SCHED-7: fold EVERY cycle this pass did NOT spend executing a task in as IDLE — the whole
        // pass span (`t_prev`→now) minus the busy span `dispatch_next` already accounted. This closes
        // the phantom-100% hole: the old code bracketed ONLY the explicit WFI, so on a core whose WFI
        // returns near-instantly (a peripheral IRQ pending on that core — the input/render cores) or
        // that spins the empty-queue loop between infrequent micro-wakes, the drain + empty passes +
        // instant-return WFI were UNACCOUNTED, and the only cycles ever landing in the window were the
        // micro busy spans → a steady phantom 100%. Measuring wall-minus-busy makes busy% ==
        // (task-execution / wall-time): ~0 for a provably-idle workload, ~100 for a CPU-bound one.
        let t_now = now_cyc();
        let busy = PASS_BUSY_CYC[cpu].swap(0, Ordering::Relaxed);
        ACCT[cpu].account(0, t_now.wrapping_sub(t_prev).saturating_sub(busy), 0);
        t_prev = t_now;
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
// SCHED-NEXT — one-shot cooperative scheduled work on the `virt` GICv3 secondaries (busy-heartbeat)
// ---------------------------------------------------------------------------------------------
//
// The idle-heartbeat proved a parked-online secondary reads honest *idle*. This proves its other
// half: an online secondary can actually RUN scheduled work and read *busy*. It is the QEMU-testable
// (cooperative) slice of "SMP scheduling on virt" — preemptive multi-core stays the metal-only proof.
// A `virt` secondary runs at EL2 and has no per-core timer, so this is a single bounded pass over a
// finite pre-staged queue of run-to-completion tasks (yield/exit only): `run_until_empty` needs no
// timer and never WFIs, and `switch_context` is EL-neutral — exactly how the boot-core CAPSTONE runs
// cooperatively at EL2/EL1 under QEMU. Each dispatch bumps `CPU_BUSY[cpu]` (the busy telemetry).

/// Cooperative secondary-probe body: yield a few times, then return (freeing the task). Every
/// dispatch/redispatch of it bumps `CPU_BUSY[cpu]` via `dispatch_next`, so a secondary that drains a
/// small queue of these publishes non-zero busy telemetry. `arg` = the core id (for the log line).
fn secondary_probe_body(arg: usize) {
    let core = arg;
    for _ in 0..3 {
        yield_now();
    }
    serial_println!(":: SCHED: core {} cooperative probe ran (busy telemetry) ::", core);
}

/// BSP-side: declare that this boot WILL stage cooperative secondary work — call ONCE, BEFORE
/// `CPU_ON`, so every secondary that comes online observes it and waits for the release. Only the
/// `virt` `start_secondaries` calls this; the tegra/probe paths that share `__secondary_rust_virt`
/// never do, so their secondaries skip the wait (see `run_secondary_work`).
pub fn arm_secondary_work() {
    SECWORK_ARMED.store(true, Ordering::Release);
}

/// BSP-side: stage `n` cooperative probe tasks onto secondary `cpu`'s run queue. Call BEFORE
/// `secondary_work_go`; the target core drains them in `run_secondary_work`. Pinned to `cpu` (like
/// every task) so they dispatch on that core and its `CPU_BUSY` is what moves.
pub fn stage_secondary_work(cpu: usize, n: usize) {
    for _ in 0..n {
        spawn("sec-probe", secondary_probe_body, cpu, cpu);
    }
}

/// BSP-side: release every secondary spinning in `run_secondary_work` to drain its staged queue.
pub fn secondary_work_go() {
    SECWORK_GO.store(true, Ordering::Release);
}

/// BSP-side: has secondary `cpu` finished its cooperative drain pass? (The busy-heartbeat completion
/// gate — the BSP waits on this before reading `meter_cpu_ticks`.)
pub fn secondary_work_done(cpu: usize) -> bool {
    cpu < NUM_CPUS && SECWORK_DONE[cpu].load(Ordering::Acquire)
}

/// Secondary-side: if this boot staged cooperative work (armed), wait for the BSP's release then
/// drain THIS core's staged queue to completion and mark done; otherwise return immediately. Called
/// by `__secondary_rust_virt` once, BEFORE its idle park. The queue is finite and pre-staged, so
/// `run_until_empty` drains it and returns (no timer / no WFI); `dispatch_next` bumps `CPU_BUSY[cpu]`
/// per dispatch. The wait spins with IRQ unmasked (the caller's state), so a BSP→AP SGI landing during
/// it is still serviced.
///
/// TWO clean paths (both provably non-hanging), keyed off `SECWORK_ARMED` — which the BSP sets before
/// `CPU_ON`, so a secondary always observes the true value:
///   * NOT armed → the tegra (`start_secondaries_tegra`) and SMP-probe callers of this shared tail
///     stage no work: return at once, no wait, no drain. Park exactly as before — zero added latency,
///     zero hang risk.
///   * armed → the `virt` `start_secondaries` WILL stage + release. Wait for `SECWORK_GO` with a
///     GENEROUS finite backstop (~1 s). This is deterministic under host load: the release lands in
///     microseconds when idle and still well inside the backstop when the host is saturated (the BSP's
///     ping proofs — which run between arming and release — are themselves bounded to a few hundred ms
///     of guest time). The backstop is a SAFETY net (a release that never comes = a BSP bug), never
///     the normal-case timing, so it can never hang and never flakes. This replaces the original
///     ~20 ms one-shot ceiling, which doubled as the timing bound and spuriously FAILed the busy
///     witness under load (APs that hadn't reached this pass before 20 ms parked idle with `busy=0`).
pub fn run_secondary_work(cpu: usize) {
    if !SECWORK_ARMED.load(Ordering::Acquire) {
        return; // tegra / probe: no work staged, no wait, park as before
    }
    let freq = timer::cntfrq();
    let freq = if freq == 0 { 62_500_000 } else { freq };
    let deadline = timer::cntpct() + freq; // ~1 s generous backstop (never the normal path)
    while !SECWORK_GO.load(Ordering::Acquire) && timer::cntpct() < deadline {
        core::hint::spin_loop();
    }
    run_until_empty(cpu);
    if cpu < NUM_CPUS {
        SECWORK_DONE[cpu].store(true, Ordering::Release);
    }
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

/// WEDGE-5 (2026-07-30, the P94/P95 rx-backstop starvation): the semaphore raw lock was the ONLY
/// unwitnessed masked spin on the starved task's path (the run-queue lock has WEDGE-4 and never
/// fired). Episodes crossing the stall threshold count here; the max spin ever seen rides beside
/// it. Printed by [spin1] from the witness core — never from inside the spin (the serial lock must
/// not nest under a stalled acquisition).
static SEM_STALL_EPISODES: AtomicU64 = AtomicU64::new(0);
static SEM_SPIN_MAX: AtomicU64 = AtomicU64::new(0);
const SEM_STALL_SPINS: u64 = 50_000_000; // ~seconds at spin_loop speed — far past any honest hold

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
        // WEDGE-5: witnessed like the run-queue lock — count the stall, never print in here (the
        // serial lock must not nest under a stalled acquisition).
        let mut spins: u64 = 0;
        while self.locked.swap(true, Ordering::Acquire) {
            spins = spins.wrapping_add(1);
            if spins == SEM_STALL_SPINS {
                SEM_STALL_EPISODES.fetch_add(1, Ordering::Relaxed);
            }
            core::hint::spin_loop();
        }
        SEM_SPIN_MAX.fetch_max(spins, Ordering::Relaxed);
    }

    /// WEDGE-5 introspection for the [spin1] witness: (locked, count, waiters_len). The waiters
    /// length is read UNLOCKED — witness-only; a torn read is acceptable and never dereferenced.
    pub fn debug_state(&self) -> (bool, i64, usize) {
        (
            self.locked.load(Ordering::Relaxed),
            self.count.load(Ordering::Relaxed),
            unsafe { (*self.waiters.get()).len() },
        )
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

        // KILLBOUND — the PRE-PARK kill boundary. A parked task is in neither a run queue nor any
        // core's `current`, so NEITHER of SKILL-1's boundaries can reach it: parking here with a kill
        // already armed would make the task permanently unkillable (see `kill_detach`'s limits note).
        // The wait's own predicate is the right place to test the flag, and this call site qualifies as
        // a kill boundary for exactly the reasons `kill_check_current`'s do — IRQ-masked, on the task's
        // own kernel stack. RELEASE the raw lock first: `exit()` never returns, and a lock held across
        // it would wedge every future waiter/poster on this semaphore.
        let (id, ttbr0) = unsafe { ((*raw).id, (*raw).user_ttbr0) };
        if kill_slot_for(id, ttbr0).is_some() {
            self.unlock_raw();
            exit(); // diverges
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

    /// KILLBOUND — the POST-PARK half: evict every waiter on this semaphore that an ARMED kill names,
    /// and re-ready it so it reaches the off-CPU dispatch boundary (where `dispatch_next` retires it
    /// before it can execute another instruction). Returns how many were evicted.
    ///
    /// The pre-park check in `wait` closes "kill armed, then park"; this closes "park, then kill" —
    /// the operator case, where the app has been sitting at its barrier for minutes before anyone
    /// types `kill`. Together they make the property total: an armed kill always reaches its target.
    ///
    /// NO PERMIT IS HANDED OVER (unlike `post`), and that is sound only because an evicted task
    /// provably never resumes: the request that matched it cannot be cleared while it is alive
    /// (`kill_release` needs `KILL_DONE`, i.e. the target retired; `kill_settle` withholds that while
    /// `asid_thread_leave` still counts this task; `kill_retract` is only reachable once the requester
    /// has independently observed the target dead). So the task is retired at its next dispatch,
    /// having executed nothing — it never returns from `wait` to act on a permit it does not hold.
    ///
    /// `make_ready` is called with the raw lock RELEASED, the same rule `post` follows (the run-queue
    /// lock must never nest under this one).
    pub fn wake_killed(&self) -> u32 {
        let daif = irq_save_mask();
        let mut evicted = 0u32;
        loop {
            self.lock_raw();
            let found = unsafe {
                let q = &mut *self.waiters.get();
                q.iter()
                    .position(|t| kill_slot_for(t.id, t.user_ttbr0).is_some())
                    .and_then(|i| q.remove(i))
            };
            self.unlock_raw();
            match found {
                Some(task) => {
                    make_ready(task);
                    evicted += 1;
                }
                None => break,
            }
        }
        irq_restore(daif);
        evicted
    }
}

// ---------------------------------------------------------------------------------------------
// ELF-3: futex — a physical-address-keyed EL0 wait/wake primitive (backs SYS_FUTEX)
// ---------------------------------------------------------------------------------------------
//
// A futex lets EL0 build a userspace mutex/condvar: block on a u32 user word iff it still holds an
// expected value, and wake N blocked waiters. The KEY is the word's PHYSICAL address — globally unique,
// so the threads of one process (same slot backing frame) that share a word key on the same bucket, and
// two different processes never collide. Bucket selection reuses the exact Semaphore lock-handoff park
// (PARK_WAITQ): the blocking task holds the bucket lock across the switch, the scheduler pushes its Box
// then releases the lock — lost-wakeup-safe. A small fixed pool of buckets (one per live key); the ELF-3
// test uses ONE. `-EAGAIN`-equivalent (TableFull) when every bucket is serving a different live key.

/// Distinct futex keys the kernel can have waiters parked on at once (never grown — same discipline as
/// the thread/proc tables).
///
/// VUGPAUSE-2 raised this from 16, and the raise is load-bearing rather than defensive. A vug used to hold
/// ONE live key (its `DONE` barrier word), so a six-vug fleet fit in 16 with room to spare. It now holds
/// THREE while idle — the barrier word, the `PHASE` release word both workers park on, and its input ring
/// — which is 18 for the same fleet: over the old pool, and the overflow does not fail loudly. It returns
/// `TableFull`, every caller degrades to a spin, and the arc's whole benefit quietly evaporates on exactly
/// the workload it was built for. Sized to 64 so a full `USER_SLOTS` fleet cannot reach it; a bucket is a
/// lock, a key and a `VecDeque` header, so the pool is small even at this width.
const NFUTEX: usize = 64;

/// One futex wait bucket: a keyed FIFO wait queue with a Semaphore-style raw lock handed to the scheduler.
struct FutexBucket {
    /// Raw spinlock guarding `key` + `waiters` (Acquire on lock, Release on unlock; the PARK_WAITQ
    /// lock-handoff releases it AFTER the scheduler pushes the blocking Box).
    locked: AtomicBool,
    /// The physical-address key this bucket serves, or 0 = free. Claimed on the first waiter for a key,
    /// released back to 0 when its last waiter leaves.
    key: AtomicU64,
    /// FIFO waiter list; touched only under `locked`, pre-reserved by `futex_init`.
    waiters: UnsafeCell<VecDeque<Box<Task>>>,
}

// SAFETY: every access to `key`/`waiters` is serialised by `locked`; identical argument to `Semaphore`.
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

/// Reserve every futex bucket's waiter capacity on the BSP before any task can park on one (so the
/// scheduler's park-side `push_back` never reallocates under the held lock). Call once at boot / before
/// the first `futex_wait`.
pub fn futex_init() {
    for b in FUTEX.iter() {
        b.lock_raw();
        unsafe { (*b.waiters.get()).reserve(WAIT_CAPACITY) };
        b.unlock_raw();
    }
}

/// Outcome of `futex_wait`.
pub enum FutexWait {
    /// Was blocked, then woken by a `futex_wake` on the same key.
    Woken,
    /// `*uaddr != expected` at the compare — the caller must re-check and loop (no sleep happened).
    Mismatch,
    /// Every bucket is busy with a different live key — the futex pool is exhausted.
    TableFull,
    /// Called off a scheduled task (no `current` to park) — cannot block.
    NoTask,
}

/// FUTEX_WAIT: block the current task on `key` iff the u32 at `uaddr` still equals `expected`. The compare
/// is performed UNDER the bucket lock, and any `futex_wake(key, ..)` must take that same lock, so a wake
/// can never slip between the compare and the park being enqueued (the classic race-free compare-and-block).
/// `key` MUST be the physical address of `uaddr`. `uaddr` must already be validated (in the caller's user
/// window) by the syscall layer; this reads it at EL1 (the syscall runs with the caller's TTBR0 live).
pub fn futex_wait(key: u64, uaddr: u64, expected: u32) -> FutexWait {
    debug_assert!(key != 0, "futex key must be non-zero");
    let daif = irq_save_mask();
    // Select the bucket: an existing one for this key, else claim a free one. Left LOCKED on success.
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
                // FUTEX-DUP (VUG-PACE-2) — accept a bucket ANOTHER waiter keyed to `key` between our
                // existence scan above and this claim pass. Two waiters entering together on a key with
                // no standing bucket (the ONLY two-concurrent-waiter key in the system is user-vug's
                // PHASE word: both workers park on it in the same instant, once per frame) could each
                // complete the existence scan before either had stored the key, and the old claim loop
                // — which tested `== 0` alone — then minted TWO buckets for one key. `futex_wake`
                // stopped at the first, and the second bucket's waiter slept forever: the s1q win1
                // lockup (att=0, no fault, parent parked at the frame barrier behind its stranded
                // worker). This check closes the common window; the wake-side full scan below is the
                // correctness backstop for the sliver it cannot (a foreign bucket freeing mid-scan).
                let k = b.key.load(Ordering::Relaxed);
                if k == key {
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
                    irq_restore(daif);
                    return FutexWait::TableFull;
                }
            }
        }
    };
    // b is locked and serves `key`. Compare-and-block under the lock.
    let cur = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
    if cur != expected {
        if b.waiters_empty() {
            b.key.store(0, Ordering::Relaxed); // release a bucket we claimed but won't park on
        }
        b.unlock_raw();
        irq_restore(daif);
        return FutexWait::Mismatch;
    }
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    if raw.is_null() {
        if b.waiters_empty() {
            b.key.store(0, Ordering::Relaxed);
        }
        b.unlock_raw();
        irq_restore(daif);
        return FutexWait::NoTask;
    }
    // KILLBOUND — the PRE-PARK kill boundary; the `Semaphore::wait` twin, and the one that matters
    // most in anger: the frame barrier of a windowed EL0 app (`user-vug`) blocks HERE, and a task
    // parked in a futex bucket is invisible to both SKILL-1 boundaries. Release the bucket (dropping
    // the key if we claimed it and are not going to park on it) before the never-returning `exit()`.
    if kill_slot_for(unsafe { (*raw).id }, unsafe { (*raw).user_ttbr0 }).is_some() {
        if b.waiters_empty() {
            b.key.store(0, Ordering::Relaxed);
        }
        b.unlock_raw();
        exit(); // diverges
    }
    assert!(
        unsafe { (*b.waiters.get()).len() } < WAIT_CAPACITY,
        "futex waiter overflow (raise WAIT_CAPACITY)"
    );
    // FLUID-3 — the park clock. Opens here (the last instruction before the switch out) and closes
    // on the first instruction after resume, so it prices the WHOLE invisible interval: blocked in
    // the bucket, plus `make_ready`-to-dispatch. This is the vug-side wait the P83 idle reserve is
    // made of; see the ledger above `fluid3_note_park`.
    let fl3_t0 = super::now_cycles();
    unsafe {
        debug_assert_eq!((*raw).cpu as usize, cpu, "futex_wait: task on the wrong CPU");
        (*raw).state.store(STATE_BLOCKED, Ordering::Release);
        // Hand the scheduler this bucket's waiter list + lock (PARK_WAITQ lock-handoff — see Semaphore::wait).
        SCHED[cpu].park_waiters.store(b.waiters.get() as u64, Ordering::Relaxed);
        SCHED[cpu].park_lock.store(&b.locked as *const AtomicBool as u64, Ordering::Relaxed);
        SCHED[cpu].park_kind.store(PARK_WAITQ, Ordering::Relaxed);
        switch_context(&raw mut (*raw).ctx_sp, SCHED[cpu].scheduler_sp.load(Ordering::Acquire));
    }
    // Resumed once `futex_wake` moved us back to our run queue (it released the lock).
    fluid3_note_park(super::now_cycles().saturating_sub(fl3_t0));
    irq_restore(daif);
    FutexWait::Woken
}

/// FUTEX-DUP (VUG-PACE-2) — wakes that found MORE THAN ONE bucket serving their key. Every count here
/// is one occurrence of the double-claim race `futex_wait`'s claim loop can still lose (see the note
/// there); before the full-scan fix each one was a permanently stranded waiter — the s1q win1 lockup.
/// Expected to stay at 0 on almost every boot; nonzero is the race observed AND survived.
static FUTEX_DUP: AtomicU64 = AtomicU64::new(0);
/// FUTEX-DUP — rate limit for the per-event witness line, `[spread4] rewake`-style: the first few
/// occurrences name themselves, the cumulative counter carries the rest.
static FUTEX_DUP_LOG: AtomicU32 = AtomicU32::new(0);
const FUTEX_DUP_LOG_MAX: u32 = 8;

/// FUTEX_WAKE: wake up to `n` waiters parked on `key`; returns the number actually woken. Releases a
/// bucket back to free once its last waiter leaves. Waiters are re-readied OUTSIDE the bucket lock (the
/// run-queue lock must never nest under it — same rule as `Semaphore::post`).
///
/// FUTEX-DUP (VUG-PACE-2): the scan visits EVERY bucket serving `key`, not just the first. The claim
/// race in `futex_wait` can leave two buckets keyed alike (two waiters entering together on a key with
/// no standing bucket), and the old `break` after the first match stranded the second bucket's waiter
/// on a key nothing would ever name again — with user-vug's PHASE futex that was a worker asleep
/// forever, the parent parked at the frame barrier behind it, and a window that stopped presenting with
/// no fault anywhere (the s1q win1 signature: att=0, parked=0ms, composited by neighbors only). The
/// early exit now happens only once `n` waiters are woken; the extra cost on the common single-bucket
/// wake is one pass over the remaining bucket keys, each a lock/load/unlock with no waiter traffic.
pub fn futex_wake(key: u64, n: usize) -> usize {
    debug_assert!(key != 0, "futex key must be non-zero");
    let daif = irq_save_mask();
    let mut woken = 0usize;
    let mut buckets_hit = 0u32;
    for b in FUTEX.iter() {
        b.lock_raw();
        if b.key.load(Ordering::Relaxed) != key {
            b.unlock_raw();
            continue;
        }
        buckets_hit += 1;
        while woken < n {
            let next = unsafe { (*b.waiters.get()).pop_front() };
            match next {
                Some(task) => {
                    b.unlock_raw();
                    make_ready(task);
                    woken += 1;
                    b.lock_raw();
                    // A concurrent drain may have freed + reclaimed this bucket for another key; stop.
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
            break; // the wake's budget is spent — semantics unchanged from the single-bucket scan
        }
    }
    if buckets_hit > 1 {
        // WITSWEEP — the witness's own blind spot, stated: `[futexdup]` CANNOT fire for an n==1 wake.
        // The scan `break`s the moment the budget is met (`woken >= n` above), so a single-waiter wake
        // that finds its waiter in the FIRST matching bucket never visits a second one — a duplicate
        // pair keyed alike is then both UNWITNESSED (buckets_hit stays 1) and left UNDRAINED (the
        // second bucket keeps its key and its waiter until some later wake on that key scans past the
        // first). This arc's target case (user-vug's PHASE futex) wakes n==2, so the scan runs past
        // the first bucket and the duplicate is both drained and counted; the blind spot is real only
        // for keys woken strictly one-at-a-time, where the strand it could hide is also the one the
        // full-scan fix exists to absorb. A zero FUTEX_DUP therefore means "no duplicate SEEN", not
        // "no duplicate happened".
        //
        // The race happened and the full scan absorbed it. Witness it: this exact shape was a silent
        // permanent strand before the fix, so each early occurrence is worth a line on the wire.
        FUTEX_DUP.fetch_add(1, Ordering::Relaxed);
        if FUTEX_DUP_LOG.fetch_add(1, Ordering::Relaxed) < FUTEX_DUP_LOG_MAX {
            serial_println!(
                ":: [futexdup] key={:#x} buckets={} woken={} (double-claim absorbed) ::",
                key, buckets_hit, woken
            );
        }
    }
    irq_restore(daif);
    woken
}

/// KILLBOUND — the futex twin of `Semaphore::wake_killed`: evict every futex waiter that an ARMED kill
/// names, across every bucket, and re-ready it so the off-CPU dispatch boundary retires it. Returns how
/// many were evicted.
///
/// This is the load-bearing one. `user-vug`'s per-frame barrier blocks in `futex_wait` (main.rs:799-810),
/// and when its workers are absent — the THREAD_TABLE-exhaustion chain — it blocks there FOREVER, before
/// its first present. That task is in no run queue and is no core's `current`, so both SKILL-1 boundaries
/// miss it, `asid_thread_leave` never runs, the ASID-scoped request is never settled, and the operator's
/// `kill` reports "armed but unconfirmed" for the rest of the boot. Evicting it here is what turns the
/// armed request into an actual death.
///
/// Same lock discipline as `futex_wake`: scan+remove under the bucket lock, `make_ready` outside it, and
/// release a bucket whose last waiter has left so the key does not strand a pool entry.
fn futex_wake_killed() -> u32 {
    let daif = irq_save_mask();
    let mut evicted = 0u32;
    for b in FUTEX.iter() {
        loop {
            b.lock_raw();
            let found = unsafe {
                let q = &mut *b.waiters.get();
                q.iter()
                    .position(|t| kill_slot_for(t.id, t.user_ttbr0).is_some())
                    .and_then(|i| q.remove(i))
            };
            if found.is_some() && b.waiters_empty() {
                b.key.store(0, Ordering::Relaxed); // last waiter gone — release the bucket
            }
            b.unlock_raw();
            match found {
                Some(task) => {
                    make_ready(task);
                    evicted += 1;
                }
                None => break,
            }
        }
    }
    irq_restore(daif);
    evicted
}

/// KILLBOUND introspection: how many tasks are parked across every futex bucket right now. Used by the
/// `[killbound]` regression leg as a POSITIVE witness that its fixture really reached the futex park (the
/// condition under test) before the kill is issued — so a leg that passes cannot be passing vacuously.
pub fn futex_parked_total() -> usize {
    let daif = irq_save_mask();
    let mut n = 0usize;
    for b in FUTEX.iter() {
        b.lock_raw();
        n += unsafe { (*b.waiters.get()).len() };
        b.unlock_raw();
    }
    irq_restore(daif);
    n
}

// ---- FLUID-3 — price the futex park (the vug-side wait the load meter proves is real idle) -------
//
// P83 bench observation (Peter, live): pointer motion INCREASES a fleet core's idle, and each vug
// SETTLES to a characteristic fps below available capacity. The SCHED load meter counts service time
// as busy (`CoreAccount::busy_pct`), so the growing reserve is genuine idle: fleet tasks stop being
// runnable. The only place a live (non-paused) vug leaves the run queues is `futex_wait` — the frame
// barrier (`DONE`), the worker release (`PHASE`) and the idle input ring are all futex parks — so the
// park duration distribution IS the invisible wait this arc exists to price. Measured from just
// before the context switch out to the first instruction after resume, so it includes wake-to-
// dispatch latency: exactly the interval the parked core may sit idle while the meter shows reserve.
//
// Drained on the `[wcn]`/`[comp2]` cadence by `video::wm`'s `[fluid3]` emit, which pairs it with the
// present-side concurrency figures. The histogram is log2 in microseconds (bucket b holds parks in
// [2^(b-1), 2^b) us, top bucket open-ended) — enough to read the modes: a barrier park behind a
// healthy worker is tens-to-hundreds of us; a park behind a starved worker on a saturated core is
// milliseconds; an idle vug's input-ring park is seconds and lands in the top bucket.
const FL3_BUCKETS: usize = 16;
static FL3_PARK_N: AtomicU64 = AtomicU64::new(0);
static FL3_PARK_CYC: AtomicU64 = AtomicU64::new(0);
static FL3_PARK_MAX_CYC: AtomicU64 = AtomicU64::new(0);
static FL3_HIST: [AtomicU32; FL3_BUCKETS] = [const { AtomicU32::new(0) }; FL3_BUCKETS];

/// FLUID-3 — fold one completed futex park into the window's ledger. Called on the resumed task's
/// own path with IRQs still masked; three relaxed RMWs and one shift, no locks.
#[inline]
fn fluid3_note_park(cyc: u64) {
    FL3_PARK_N.fetch_add(1, Ordering::Relaxed);
    FL3_PARK_CYC.fetch_add(cyc, Ordering::Relaxed);
    FL3_PARK_MAX_CYC.fetch_max(cyc, Ordering::Relaxed);
    let cyc_per_us = (load_window_cyc().saturating_mul(4) / 1_000_000).max(1);
    let us = (cyc / cyc_per_us).max(1);
    let b = ((64 - us.leading_zeros()) as usize).min(FL3_BUCKETS - 1);
    FL3_HIST[b].fetch_add(1, Ordering::Relaxed);
}

/// FLUID-3 — drain the park ledger: `(parks, mean_us, max_us, hist)`. The histogram is log2-us as
/// documented on the statics; the caller (`video::wm::fluid3_emit`) derives percentiles from it.
pub fn fluid3_drain() -> (u64, u64, u64, [u32; FL3_BUCKETS]) {
    let n = FL3_PARK_N.swap(0, Ordering::Relaxed);
    let cyc = FL3_PARK_CYC.swap(0, Ordering::Relaxed);
    let max = FL3_PARK_MAX_CYC.swap(0, Ordering::Relaxed);
    let mut hist = [0u32; FL3_BUCKETS];
    for (i, h) in FL3_HIST.iter().enumerate() {
        hist[i] = h.swap(0, Ordering::Relaxed);
    }
    let cyc_per_us = (load_window_cyc().saturating_mul(4) / 1_000_000).max(1);
    (
        n,
        cyc.checked_div(n).unwrap_or(0) / cyc_per_us,
        max / cyc_per_us,
        hist,
    )
}

/// KILLBOUND — sweep every wait an EL0 task can be parked in and evict the ones an armed kill names.
/// Called by `kill` once the request is published, so the predicate (`kill_slot_for`) already matches.
///
/// The set is enumerable because every blocking wait an EL0 program can reach is one of exactly two
/// shapes: a futex bucket (`SYS_FUTEX`), or a `Semaphore` the syscall layer owns (`SYS_THREAD_JOIN`'s
/// join handle, `SYS_WAIT`'s `Proc::done`, `SYS_MRECV`'s per-ASID bus mailbox). The futex half lives
/// here; the semaphore half is delegated to `syscall`, which owns those tables — the same hook shape
/// `note_killed_task_retired` already uses.
fn kill_wake_parked() -> u32 {
    let mut n = futex_wake_killed();
    #[cfg(feature = "baremetal")]
    {
        n += super::syscall::kill_wake_parked_semaphores();
    }
    n
}

// ---------------------------------------------------------------------------------------------
// Mutex<T> — a sleeping mutual-exclusion lock (a binary semaphore guarding owned data)
// ---------------------------------------------------------------------------------------------

/// A sleeping mutex: `lock()` BLOCKS the calling task (it does not spin) until the lock is free,
/// then hands back exclusive access to the protected data through an RAII guard that unlocks on
/// drop. Built on `Semaphore` with a single permit, so it inherits the lost-wakeup-safe, cross-CPU
/// block/wake — and, unlike the run-queue spinlock, a task may safely hold it across preemption and
/// yields.
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
// one live `&mut T` — at a time, across all cores. `T: Send` because the data is accessed from
// whichever core currently holds the lock (which varies over time).
unsafe impl<T: Send> Sync for Mutex<T> {}
unsafe impl<T: Send> Send for Mutex<T> {}

impl<T> Mutex<T> {
    /// Construct an unlocked mutex (one permit). `const` so it can initialise a `static`.
    pub const fn new(value: T) -> Self {
        Mutex { sem: Semaphore::new(1), data: UnsafeCell::new(value) }
    }

    /// Reserve the underlying semaphore's waiter capacity. Call once on the BSP before use.
    pub fn init(&self) {
        self.sem.init();
    }

    /// Acquire the lock, blocking the current task until it is free. Returns a guard that unlocks on
    /// drop.
    ///
    /// MUST be called from a scheduled task. A guard may be issued ONLY when a real permit was taken
    /// — otherwise two callers could hold guards at once (aliased `&mut T` = UB). Off a scheduled
    /// task `wait()` cannot block and so does not acquire, so we panic rather than hand out an
    /// unbacked guard (a sleeping mutex is meaningless off a scheduler context anyway).
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
    /// be moved to another task/core (which would unlock from the wrong context).
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
// Channel<T> — a bounded blocking channel, composed from a Mutex + two Semaphores
// ---------------------------------------------------------------------------------------------

/// A fixed-capacity blocking channel (the classic slots/items bounded buffer). `send` blocks while
/// full, `recv` blocks while empty; both can cross-CPU-wake the other side. Multiple producers and
/// multiple consumers are safe — the semaphores and the buffer mutex serialise all of them.
/// Demonstrates that the scheduler's `Mutex` and `Semaphore` COMPOSE into higher-level concurrency
/// with no new unsafe: `slots` counts free slots (send waits on it), `items` counts buffered values
/// (recv waits on it), and a `Mutex` serialises the buffer. Each `wait()` corresponds to a real
/// produced/consumed item, so the buffer push/pop is never starved or raced (bounded-buffer invariant).
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
// Condvar — a Mesa-semantics condition variable (the count-less companion to Semaphore)
// ---------------------------------------------------------------------------------------------

/// A condition variable for kernel threads, paired with the sleeping `Mutex<T>`. `wait(guard)`
/// atomically releases the mutex and blocks the calling task; `notify_one`/`notify_all` wake parked
/// waiter(s). It is the count-LESS sibling of `Semaphore`: a notification with no waiter is a no-op
/// (it is NOT stored), which is the defining condition-variable semantics.
///
/// Callers MUST re-check their predicate in a `while` loop. Wakeups are permitted to be spurious,
/// and — unlike `sleep_ticks` — a cv waiter has NO timer backstop: it sits only in this queue with
/// no deadline, so a `notify` is its SOLE wake source and a missed notify is a permanent hang, not a
/// one-tick latency blip. That raises the stakes on the lost-wakeup proof below; it is the same
/// lock-handoff that makes `Semaphore` safe.
///
/// MUST be `'static` (e.g. a `static CV`): like `Semaphore`, `wait()` hands raw pointers to
/// `waiters`/`locked` to the scheduler to be dereferenced after the context switch. Call `init()`
/// once on the BSP before use.
///
/// Lost-wakeup safety (Mesa semantics). A correct notifier changes the predicate under the mutex,
/// then notifies. `wait()` acquires the condvar's `locked` BEFORE releasing the mutex, and keeps it
/// held — handed off to the scheduler, released only after the blocked Box is enqueued — across the
/// switch. A notifier must take `locked` to pop a waiter, so it cannot run a notify between the
/// mutex-release and the enqueue; it always observes the waiter. This is exactly `Semaphore`'s
/// lock-handoff, with the protected resource (the mutex) released explicitly inside the handoff.
pub struct Condvar {
    /// Raw spinlock guarding `waiters`. Same role as `Semaphore::locked`; no count (notifications
    /// are not stored).
    locked: AtomicBool,
    /// FIFO waiter list; touched only under `locked`. Pre-reserved to `WAIT_CAPACITY` by `init()`.
    waiters: UnsafeCell<VecDeque<Box<Task>>>,
}

// SAFETY: every access to `waiters` is serialized by `locked`; the park-side push happens while the
// blocker's lock is still held (released by the scheduler after the push), establishing
// happens-before with the next notify — identical to `Semaphore`.
unsafe impl Sync for Condvar {}

impl Condvar {
    /// Construct an empty condition variable. `const` so it can initialise a `static`.
    pub const fn new() -> Self {
        Condvar { locked: AtomicBool::new(false), waiters: UnsafeCell::new(VecDeque::new()) }
    }

    /// Reserve the waiter list so the scheduler's park-side push never reallocates under the held
    /// lock. Call once on the BSP before any task can block on this condvar.
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
    /// notified, re-acquire the mutex and return a fresh guard. MUST be called from a scheduled task
    /// holding `guard`. Spurious wakeups are allowed — the caller must re-test its predicate.
    ///
    /// The lettered step order below is load-bearing for lost-wakeup safety and the lock-ordering
    /// invariant; do not reorder it.
    pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        // (a) Mask IRQ for the whole critical section; remember the caller's DAIF to restore.
        let daif = irq_save_mask();

        // (b) Must be on a scheduled task. Assert BEFORE consuming the guard: were we to forget the
        //     guard first and then panic, the mutex would be permanently locked (its explicit
        //     release in (h) never runs, and the guard's Drop is already gone).
        let cpu = percpu::this_cpu().cpu_index as usize;
        let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
        assert!(
            !raw.is_null(),
            "Condvar::wait() called off a scheduled task (needs a scheduler context)"
        );

        // (c) Extract the mutex and DISARM the guard's Drop. `forget` is the deliberate dual of the
        //     explicit `mutex.sem.post()` in (h): exactly one release per acquire.
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
            //     `Semaphore::post` runs with IRQ masked; `post` snapshots+restores DAIF, so it
            //     STAYS masked — preserving "every switch-away is IRQ-masked" into (j). It is the one
            //     sanctioned post-under-another-primitive-lock (cv.locked -> mutex.sem.locked ->
            //     run-queue -> heap, acyclic across this module).
            mutex.sem.post();

            // (j) Switch to the scheduler; park_blocked(PARK_WAITQ) pushes our Box into self.waiters
            //     and releases self.locked LAST (the lock-handoff).
            switch_context(
                &raw mut (*raw).ctx_sp,
                SCHED[cpu].scheduler_sp.load(Ordering::Acquire),
            );
        }

        // Resumed (IRQ masked, carried) by a notify that moved us back to our pinned run queue; the
        // condvar lock was already released by the scheduler that parked us. Restore the caller's
        // DAIF FIRST, then re-acquire the mutex: the inner `lock()`'s `Semaphore::wait` snapshots the
        // CURRENT DAIF — the carried masked one, not the caller's — so a bare re-acquire would strand
        // the task IRQ-masked. The re-acquire may legitimately block again on a contended mutex via
        // the mutex's own disjoint PARK_WAITQ handoff; the task stays CPU-pinned, so rebuilding the
        // `!Send` guard on the same core upholds its unlock-from-owner intent.
        irq_restore(daif);
        mutex.lock()
    }

    /// Wake one waiter if any; a no-op if none are waiting (the notification is NOT stored — the
    /// defining difference from `Semaphore::post`). May be called from any context (a task or the
    /// unscheduled BSP). `make_ready` runs only AFTER releasing the condvar lock, so the lock is
    /// never nested over a run-queue lock.
    pub fn notify_one(&self) {
        let daif = irq_save_mask();
        self.lock_raw();
        let waiter = unsafe { (*self.waiters.get()).pop_front() };
        self.unlock_raw();
        if let Some(task) = waiter {
            make_ready(task);
        }
        irq_restore(daif);
    }

    /// Wake EVERY currently-queued waiter. Drains one waiter per lock acquisition and calls
    /// `make_ready` outside the lock (the `Semaphore::post` discipline), so the condvar lock is never
    /// held across a run-queue lock. May be called from any context. A waiter that arrives mid-drain
    /// may also be woken — harmless under Mesa semantics (the caller re-tests its predicate); a
    /// correct notifier holds the mutex across the notify, so in practice no new waiter arrives
    /// during the drain, and each iteration removes exactly one Box (no livelock).
    pub fn notify_all(&self) {
        let daif = irq_save_mask();
        loop {
            self.lock_raw();
            let waiter = unsafe { (*self.waiters.get()).pop_front() };
            self.unlock_raw();
            match waiter {
                Some(task) => make_ready(task),
                None => break,
            }
        }
        irq_restore(daif);
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
/// starved by readers. This is NOT strict per-writer FIFO — a writer woken from `writer_ok` can be
/// leapfrogged by another writer that barges in (takes the just-freed lock before the woken one
/// re-acquires `inner`), so an individual writer's progress is guaranteed only against a FINITE
/// writer population, not an unbounded barging stream. On the reader side the cost is heavier:
/// **reader starvation is UNBOUNDED** under sustained write load — a reader blocked on the condvar is
/// `STATE_BLOCKED`, off every run queue. (This aarch64 scheduler has NO priority aging at all — the
/// run queue is flat round-robin — so, even more than on x86, nothing bounds condvar-blocked
/// waiting.) Do not use this lock where readers must make progress against a continuous writer stream.
///
/// MUST be `'static` (or Arc-kept-alive) like its parts; call `init()` once on the BSP before use.
///
/// PRECONDITION (load-bearing): at most `WAIT_CAPACITY` (32) tasks may be simultaneously blocked on
/// the reader condvar — or on the writer condvar — of a single `RwLock`. Unlike the other primitives
/// (whose waiter counts are naturally bounded by producer/consumer/holder count), an `RwLock`'s
/// reader queue is unbounded BY DESIGN, so a caller MUST bound its concurrent-reader population.
///
/// NOT REENTRANT. A task must hold AT MOST ONE guard (read or write) on a given `RwLock` at a time.
/// All four re-entries DEADLOCK PERMANENTLY (the condvars have no timer backstop):
///   * read-then-read — the 2nd `read()` yields to a queued writer that waits for `readers == 0`,
///     which the 1st guard prevents. DANGEROUS: deadlocks ONLY when a writer is also waiting, so it
///     passes tests and then hangs in production.
///   * read-then-write — `write()` waits for `readers == 0`, but the caller is that reader.
///   * write-then-read — `read()` blocks on `writer == true`, which the caller holds.
///   * write-then-write — the 2nd `write()` blocks on `writer == true`.
pub struct RwLock<T> {
    inner: Mutex<RwState>,
    /// Parks readers waiting out a writer; woken (all at once) when the lock clears with no writer queued.
    readers_ok: Condvar,
    /// Parks writers waiting out readers/a writer; woken one-at-a-time on each release.
    writer_ok: Condvar,
    data: UnsafeCell<T>,
}

// SAFETY: `&T` is handed to MULTIPLE readers on different cores at once, so `T: Sync` is REQUIRED
// (the key difference from `Mutex<T>`, which needs only `T: Send` because it hands out one `&mut T`);
// a writer's `&mut T` is the data's sole accessor but migrates across cores over time, so `T: Send`.
// The `(readers > 0) XOR writer` invariant under `inner` guarantees no `&mut T` aliases.
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

    /// Reserve all three sub-primitives' waiter capacity. Call once on the BSP before use.
    pub fn init(&self) {
        self.inner.init();
        self.readers_ok.init();
        self.writer_ok.init();
    }

    /// Acquire a shared read lock, blocking until no writer holds OR is waiting (writer-preference).
    /// Returns an RAII guard giving `&T`; releases on drop. MUST be called from a scheduled task.
    /// NOT reentrant (see the type docs).
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

/// Shared-read RAII guard from `RwLock::read`. `Deref`s to `&T`; on drop, decrements the reader count
/// and (when it reaches zero) hands the lock to a waiting writer.
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
        // is momentarily contended — sound because the dropping task is a normal scheduled task, but
        // it means a guard must be dropped only from a scheduled task (the same contexts where
        // `read()`/`write()` may be called).
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
/// the writer flag and wakes the next writer or, if none waits, all parked readers.
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
            self.lock.writer_ok.notify_one(); // FIFO-ish hand-off to the next writer (writer-preference)
        } else {
            self.lock.readers_ok.notify_all(); // no writer waiting: release every parked reader
        }
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

    // PRIO-MIX M1 (the Pi kernel8-battery accrual the AARCH64-PRIO landing deferred): run the dedicated
    // priority-mix stress witness here on the boot core, still COOPERATIVELY — this runs BEFORE
    // `start_aps` flips `SCHED_ACTIVE`, so preemption is provably off and the strict-ordering sub-scenario
    // is validly asserted (its aged-rescue half is a bounded-rescue claim that also stays honest once
    // preemption is on). Self-contained + bounded, so it leaves the queue empty for the workload below.
    // On the `virt`/Orin paths the boot core diverges into `run_capstone_boot_core` before reaching here,
    // so those get the witness there instead (no double run).
    prio_mix_witness(0);

    // SCHED-2: non-degeneracy proof for the per-core load accounting. The cooperative demo + prio-mix
    // above drove dozens of dispatches on core 0, so the rolling-window busy fraction and the
    // context-switch counter are both provably non-zero here — this witness catches a regression that
    // freezes the accounting (a stuck-at-0 counter, a window that never advances). Runs in the QEMU
    // `kernel8-test` gate (no timer needed — the accounting rides the cooperative dispatch path).
    // `pi`-gated exactly like `core_load_report`: fires on the target + gate, byte-identical for the
    // jetson/virt builds that diverge into `run_capstone_boot_core` before reaching here.
    #[cfg(feature = "pi")]
    load_accounting_witness();

    // SKILL-1: prove the asynchronous kill primitive. Runs here for the same reason the two witnesses
    // above do — cooperatively on the boot core, before `start_aps` flips `SCHED_ACTIVE`, self-contained
    // and bounded, leaving the run queue empty. `pi`-gated, so the virt/Orin builds (which diverge into
    // `run_capstone_boot_core` before reaching here) stay byte-identical.
    #[cfg(feature = "pi")]
    skill_kill_witness(0);
}

// --- SKILL-1 witness: the asynchronous kill primitive, exercised deterministically in QEMU ----------
/// Dispatch passes the witness allows a kill to confirm in. Generous: the expected cost is ONE pass.
#[cfg(feature = "pi")]
const SKILL_MAX_PASSES: u32 = 32;
/// The off-CPU victim's liveness counter — must ADVANCE before the kill and FREEZE after it.
#[cfg(feature = "pi")]
static SKILL_VICTIM_TICKS: AtomicU32 = AtomicU32::new(0);
/// The on-CPU victim's liveness counter, same contract.
#[cfg(feature = "pi")]
static SKILL_SELF_TICKS: AtomicU32 = AtomicU32::new(0);
/// The on-CPU victim publishes its own kill slot here (`idx + 1`; 0 = not yet armed) so the witness can
/// release the ticket the victim could not carry across its own retirement.
#[cfg(feature = "pi")]
static SKILL_SELF_SLOT: AtomicU32 = AtomicU32::new(0);
/// Set by the post-kill task: proves the scheduler still runs ordinary work after two reaps.
#[cfg(feature = "pi")]
static SKILL_RERUN_DONE: AtomicBool = AtomicBool::new(false);

/// The OFF-CPU victim: an immortal cooperative loop. It never exits on its own, so the only thing that
/// can retire it is the kill — which is precisely the property under test.
#[cfg(feature = "pi")]
fn skill_victim_body(_: usize) {
    loop {
        SKILL_VICTIM_TICKS.fetch_add(1, Ordering::AcqRel);
        yield_now();
    }
}

/// The ON-CPU victim: arms its OWN kill while it is the running task, then hits a boundary. It cannot be
/// caught by the off-CPU dispatch arm (the request did not exist when it was dispatched), so the retire
/// must come from `kill_check_current` — the same code path `timer_preempt` uses against a spinning EL0
/// task on metal. This is the closest a timer-less QEMU can get to the metal case: the ARM is identical,
/// only the trigger differs (a cooperative `yield_now` here, the quantum tick there).
#[cfg(feature = "pi")]
fn skill_self_body(_: usize) {
    SKILL_SELF_TICKS.fetch_add(1, Ordering::AcqRel);
    let id = current_id().expect("skill: self victim has no id");
    let ticket = kill(id, 0).expect("skill: no kill slot for the self victim");
    // Hand the slot index to the witness and forget the ticket: this task is about to stop existing
    // mid-function, so it cannot be the one to surrender it. (The index is all a ticket ever was.)
    SKILL_SELF_SLOT.store(ticket.idx as u32 + 1, Ordering::Release);
    core::mem::forget(ticket);
    SKILL_SELF_TICKS.fetch_add(1, Ordering::AcqRel);
    yield_now(); // never returns — `kill_check_current` retires this task here
    SKILL_SELF_TICKS.fetch_add(100, Ordering::AcqRel); // MUST NOT run; a huge step makes a leak obvious
}

/// A plain task spawned after both kills: it must run and exit normally.
#[cfg(feature = "pi")]
fn skill_rerun_body(_: usize) {
    SKILL_RERUN_DONE.store(true, Ordering::Release);
}

/// SKILL-1 — the asynchronous-kill witness. Cooperative and fully deterministic under QEMU raspi4b (no
/// timer, no IRQ, no SMP required): it drives `dispatch_next` by hand, so every step is observed rather
/// than raced.
///
/// What it proves:
///   1. LIVENESS — the victim really is running before the kill (its counter advances).
///   2. OFF-CPU REAP — after `kill`, the victim is never dispatched again (counter frozen), the request
///      confirms, and the run queue drains. This is the arm that needs no cooperation from the target.
///   3. ON-CPU RETIRE — a task that is killed while it is the CURRENT task retires at its next boundary
///      via `kill_check_current` (the same call `timer_preempt` makes on metal) and confirms.
///   4. REUSE — both tickets release, every kill slot returns to the pool, and ordinary scheduling
///      continues (a fresh task spawns, runs and exits). This is the "rerun after a kill" half.
///
/// What it CANNOT prove here: the TIMER-driven trigger. QEMU raspi4b delivers no Group-1 timer IRQ, so
/// `timer_preempt` never fires and a genuinely spinning task (one that neither yields nor syscalls) can
/// never be interrupted — it would wedge this single cooperative core. That trigger is metal-only.
#[cfg(feature = "pi")]
pub fn skill_kill_witness(cpu: usize) {
    let queue_empty = || rq(cpu).len() == 0;
    let slots_free = || {
        (0..MAX_KILL_REQS).all(|i| KILLS[i].state.load(Ordering::Acquire) == KILL_FREE)
    };

    // --- 1 + 2: the OFF-CPU arm -----------------------------------------------------------------
    let vid = spawn("skill-victim", skill_victim_body, 0, cpu);
    for _ in 0..3 {
        dispatch_next(cpu); // let it run a few cooperative rounds
    }
    let alive_ticks = SKILL_VICTIM_TICKS.load(Ordering::Acquire);
    let was_alive = alive_ticks >= 3;

    let ticket = kill(vid, 0);
    let mut off_passes = 0u32;
    let off_confirmed = match &ticket {
        None => false,
        Some(t) => {
            let mut ok = false;
            while off_passes < SKILL_MAX_PASSES {
                off_passes += 1;
                dispatch_next(cpu);
                if kill_confirmed(t) {
                    ok = true;
                    break;
                }
            }
            ok
        }
    };
    // FROZEN is the real claim: "never scheduled again", not merely "eventually stopped".
    let off_frozen = SKILL_VICTIM_TICKS.load(Ordering::Acquire) == alive_ticks;
    let off_drained = queue_empty();
    if let Some(t) = ticket {
        if off_confirmed {
            kill_release(t);
        } else {
            kill_detach(t);
        }
    }

    // --- 3: the ON-CPU arm ----------------------------------------------------------------------
    let _ = spawn("skill-self", skill_self_body, 0, cpu);
    let mut self_passes = 0u32;
    let mut self_confirmed = false;
    while self_passes < SKILL_MAX_PASSES {
        self_passes += 1;
        dispatch_next(cpu);
        let slot = SKILL_SELF_SLOT.load(Ordering::Acquire);
        if slot != 0 && KILLS[(slot - 1) as usize].state.load(Ordering::Acquire) == KILL_DONE {
            self_confirmed = true;
            break;
        }
    }
    // Exactly 2 = it reached the arm-and-yield point once and never came back past the yield.
    let self_frozen = SKILL_SELF_TICKS.load(Ordering::Acquire) == 2;
    let self_slot = SKILL_SELF_SLOT.load(Ordering::Acquire);
    if self_confirmed && self_slot != 0 {
        kill_release(KillTicket { idx: (self_slot - 1) as usize });
    }
    let self_drained = queue_empty();

    // --- 3b: the DETACH/SETTLE INTERLEAVE (the F1 hazard, made deterministic) --------------------
    // The cooperative harness can express the exact ordering that used to strand a slot: arm a kill, let
    // the requester's bounded wait "expire" (detach) while the victim is still off-CPU and un-reaped, and
    // only THEN let the retirement land. Under the old load-then-store settle, the retiring side read
    // PENDING, lost to the detach's CAS, and stamped KILL_DONE over KILL_DETACHED — a terminal state with
    // no owner left to release it, i.e. a permanently consumed slot. With the CAS settle the retirement
    // discovers the detach and frees the slot inline, so the pool comes back whole. Asserting the slot is
    // FREE (not DONE, not still DETACHED) after the reap is exactly that distinction.
    SKILL_VICTIM_TICKS.store(0, Ordering::Release);
    let did = spawn("skill-detach", skill_victim_body, 0, cpu);
    dispatch_next(cpu); // victim runs once, then parks back in the run queue (off-CPU, un-reaped)
    let dticket = kill(did, 0);
    let detach_armed = dticket.is_some();
    if let Some(t) = dticket {
        kill_detach(t); // the requester gives up BEFORE the kill lands — the F1 interleave
    }
    let mut detach_passes = 0u32;
    while detach_passes < SKILL_MAX_PASSES {
        detach_passes += 1;
        dispatch_next(cpu); // the retirement lands here, against a DETACHED slot
        if queue_empty() {
            break;
        }
    }
    let detach_reaped = queue_empty();
    // The whole point: an owner-less settle must FREE the slot, not strand it in DONE.
    let detach_slot_freed = slots_free();

    // --- 4: reuse -------------------------------------------------------------------------------
    let pool_clean = slots_free();
    let _ = spawn("skill-rerun", skill_rerun_body, 0, cpu);
    run_until_empty(cpu);
    let rerun_ok = SKILL_RERUN_DONE.load(Ordering::Acquire);

    let pass = was_alive
        && off_confirmed
        && off_frozen
        && off_drained
        && self_confirmed
        && self_frozen
        && self_drained
        && detach_armed
        && detach_reaped
        && detach_slot_freed
        && pool_clean
        && rerun_ok;
    if pass {
        serial_println!(
            ":: SKILL-1: async kill — victim ran {} rounds then froze; off-CPU reap confirmed in {} pass(es), on-CPU boundary retire confirmed in {}; detach-then-settle interleave freed its slot inline; queue drained, kill slots returned, rerun OK :: PASS ::",
            alive_ticks,
            off_passes,
            self_passes
        );
    } else {
        serial_println!(
            ":: SKILL-1: async kill — alive={} off_conf={} off_frozen={} off_drained={} self_conf={} self_frozen={} self_drained={} det_armed={} det_reaped={} det_freed={} pool={} rerun={} :: FAIL ::",
            was_alive,
            off_confirmed,
            off_frozen,
            off_drained,
            self_confirmed,
            self_frozen,
            self_drained,
            detach_armed,
            detach_reaped,
            detach_slot_freed,
            pool_clean,
            rerun_ok
        );
    }
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

// ---- M4 CAPSTONE: one coordinator drives every sync primitive's cross-core self-test in sequence,
// so a SINGLE boot (one metal reflash) verifies the whole toolkit. Each step spawns worker task(s)
// on OTHER APs and `join()`s them (so `join`/`spawn_joinable` are exercised throughout), and prints
// a PASS/FAIL line. All state is `static` (the primitives require `'static`); `init()`'d on the BSP.
static CAP_SEM: Semaphore = Semaphore::new(0);
static CAP_MTX: Mutex<u64> = Mutex::new(0);
static CAP_CHAN: Channel<u64> = Channel::new(4);
static CAP_CV_PRED: Mutex<bool> = Mutex::new(false);
static CAP_CV: Condvar = Condvar::new();
static CAP_CV_RELEASED: AtomicBool = AtomicBool::new(false);
static CAP_RW: RwLock<(u64, u64)> = RwLock::new((0, 0));
static CAP_RW_TORN: AtomicBool = AtomicBool::new(false);
static CAP_CORES: SpinMutex<[usize; 2]> = SpinMutex::new([0, 0]); // the two worker APs (online[1], online[2])
const CAP_SEM_ITEMS: usize = 5;
const CAP_MTX_INCRS: u64 = 500;
const CAP_CHAN_N: u64 = 20;
const CAP_RW_WRITES: u64 = 6;
const CAP_RW_READS: usize = 8;

fn cap_sem_producer(_: usize) {
    for _ in 0..CAP_SEM_ITEMS {
        busy_delay_ms(1); // let the coordinator block on the empty semaphore first
        CAP_SEM.post();
    }
}
fn cap_mtx_incr(_: usize) {
    for _ in 0..CAP_MTX_INCRS {
        *CAP_MTX.lock() += 1;
    }
}
fn cap_chan_producer(_: usize) {
    for v in 1..=CAP_CHAN_N {
        CAP_CHAN.send(v);
    }
}
fn cap_cv_worker(_: usize) {
    {
        let mut go = CAP_CV_PRED.lock();
        while !*go {
            go = CAP_CV.wait(go);
        }
    }
    CAP_CV_RELEASED.store(true, Ordering::Relaxed);
}
fn cap_rw_writer(_: usize) {
    for v in 1..=CAP_RW_WRITES {
        {
            let mut w = CAP_RW.write();
            w.0 = v;
            busy_delay_ms(1); // widen the non-atomic update window
            w.1 = v;
        }
        busy_delay_ms(1);
    }
}
fn cap_rw_reader(_: usize) {
    for _ in 0..CAP_RW_READS {
        let r = CAP_RW.read();
        let a = r.0;
        busy_delay_ms(1);
        if a != r.1 {
            CAP_RW_TORN.store(true, Ordering::Relaxed); // a writer's half-update leaked in
        }
    }
}

fn cap_report(name: &str, pass: bool) {
    serial_println!(":: CAPSTONE {}: {} ::", name, if pass { "PASS" } else { "FAIL" });
}

/// The capstone coordinator: run each primitive's cross-core self-test in sequence and report. Runs
/// on the first online AP; workers land on the two other APs (`CAP_CORES`). Every step blocks the
/// coordinator (on a wait/recv/join) and is woken cross-core — the metal-only path in one place.
fn capstone_body(_: usize) {
    let [b, c] = *CAP_CORES.lock();
    serial_println!(":: CAPSTONE: verifying all 6 sync primitives (workers on cores {} + {}) ::", b, c);

    // 1. Semaphore — a producer on core b posts; we consume (cross-core wake TO the coordinator).
    let h = spawn_joinable("cap-sem", cap_sem_producer, 0, b);
    let mut got = 0usize;
    for _ in 0..CAP_SEM_ITEMS {
        assert!(CAP_SEM.wait(), "capstone ran off a scheduled task");
        got += 1;
    }
    h.join();
    cap_report("Semaphore", got == CAP_SEM_ITEMS);

    // 2. Mutex — two incrementers contend the same Mutex<u64> across cores b + c (no lost RMW).
    let h1 = spawn_joinable("cap-mtx-a", cap_mtx_incr, 0, b);
    let h2 = spawn_joinable("cap-mtx-b", cap_mtx_incr, 0, c);
    h1.join();
    h2.join();
    cap_report("Mutex", *CAP_MTX.lock() == 2 * CAP_MTX_INCRS);

    // 3. Channel — a producer on core b sends 1..=N through a cap-4 buffer; we recv + sum.
    let h = spawn_joinable("cap-chan", cap_chan_producer, 0, b);
    let mut sum = 0u64;
    for _ in 0..CAP_CHAN_N {
        sum += CAP_CHAN.recv();
    }
    h.join();
    cap_report("Channel", sum == CAP_CHAN_N * (CAP_CHAN_N + 1) / 2);

    // 4. Condvar — a worker on core b blocks on the predicate; we flip it under the mutex + notify.
    let h = spawn_joinable("cap-cv", cap_cv_worker, 0, b);
    busy_delay_ms(5); // let the worker reach cv.wait()
    {
        let mut go = CAP_CV_PRED.lock();
        *go = true;
        CAP_CV.notify_all();
    }
    h.join();
    cap_report("Condvar", CAP_CV_RELEASED.load(Ordering::Relaxed));

    // 5. RwLock — a writer + two readers across cores b + c; a torn read would mean the write lock
    //    failed to exclude the readers.
    let hw = spawn_joinable("cap-rw-w", cap_rw_writer, 0, b);
    let hr1 = spawn_joinable("cap-rw-r1", cap_rw_reader, 0, b);
    let hr2 = spawn_joinable("cap-rw-r2", cap_rw_reader, 0, c);
    hw.join();
    hr1.join();
    hr2.join();
    cap_report("RwLock", !CAP_RW_TORN.load(Ordering::Relaxed));

    // 6. join() — exercised in EVERY step above (each spawn_joinable + join blocked the coordinator
    //    until the worker's completion post, at least one cross-core).
    cap_report("join", true);
    serial_println!(":: CAPSTONE COMPLETE — all 6 sync primitives verified in one boot ::");
    // PI-SCHED-1 — one-shot per-core load snapshot at a steady state (after the capstone has driven
    // real cross-core dispatch on every AP). `pi`-gated (fires on the target + `kernel8-test`, byte-
    // identical for jetson/virt); `core_load_report()` itself is callable on demand for future paths.
    #[cfg(feature = "pi")]
    core_load_report();
}

/// PI-SCHED-1 — per-core task-slice activation snapshot (introspection only). Prints each core's
/// dispatch BUSY vs IDLE pass counts, taken from the `CPU_BUSY`/`CPU_IDLE` pulse counters that
/// `dispatch_next` already maintains (BUSY bumped when a task is dispatched, IDLE when the run queue is
/// empty). A one-shot witness — call it on demand (e.g. a future shell command) or once at steady
/// state. NEVER read on a scheduling path; it only observes the counters. Left `pub` (no dead-code
/// warning) so the call site can stay behind `sched_demo` without gating the function itself.
pub fn core_load_report() {
    for cpu in 0..NUM_CPUS {
        let busy = CPU_BUSY[cpu].load(Ordering::Relaxed);
        let idle = CPU_IDLE[cpu].load(Ordering::Relaxed);
        serial_println!(":: SCHED-LOAD: core {} busy {} idle {} ::", cpu, busy, idle);
    }
}

// ---------------------------------------------------------------------------------------------
// SCHED-2 — load witnesses (periodic serial line + on-demand `top` table + a non-degeneracy proof)
// ---------------------------------------------------------------------------------------------

/// Periodic-witness cadence, in aggregate timer ticks across all cores. At the ~4 ms/tick Pi quantum
/// this fires roughly every few seconds. Metal-only — `timer_preempt` (its only driver) never runs in
/// QEMU raspi4b (no Group-1 timer delivery), so this line appears on the real Pi, not in the gate.
const LOAD_WITNESS_INTERVAL: u64 = 1024;
/// Aggregate tick accumulator across cores (whichever core lands on the interval boundary emits).
static LOAD_WITNESS_TICKS: AtomicU64 = AtomicU64::new(0);
/// Packed busy-percents of the last emitted line (`c0 | c1<<8 | c2<<16 | c3<<24`), for change-only
/// suppression — a steady-state system stops re-printing an unchanged load line.
static LOAD_WITNESS_LAST: AtomicU64 = AtomicU64::new(u64::MAX);
/// `ctx_switches` sum snapshot at the last emission, to derive the per-window context-switch delta.
static LOAD_WITNESS_CTX: AtomicU64 = AtomicU64::new(0);

/// PULSE-5 — count of load windows closed by a FOLD (`account` reaching the budget at a dispatch
/// boundary). It is the denominator of the arc's claim: reads are no longer waiting on these. Bumped
/// once per window per core (~4/s/core), never per dispatch, so it adds nothing measurable to the
/// switch path.
static PULSE5_FOLD_WINDOWS: AtomicU64 = AtomicU64::new(0);
/// PULSE-5 — high-water in-flight execution span (CNTPCT cycles) seen at witness sample time. This is
/// the staleness that used to be invisible: pre-PULSE-5 a span this long contributed NOTHING to the
/// reported percent until it ended. Sampled only by the witness (~1 s cadence, metal-only), so it is
/// a floor on the true maximum, never an inflation of it.
static PULSE5_SPAN_MAX_CYC: AtomicU64 = AtomicU64::new(0);

/// CNTPCT cycles → milliseconds, for witness output only. Uses the same cached CNTFRQ the load window
/// is derived from, so the two numbers are always expressed against one clock.
fn cyc_to_ms(cyc: u64) -> u64 {
    let frq = load_window_cyc().saturating_mul(4).max(1); // load_window_cyc() == CNTFRQ/4
    cyc / (frq / 1000).max(1)
}

/// SCHED-2 periodic load heartbeat: called once per `timer_preempt` (per core, per tick, metal-only).
/// The core whose atomic increment lands exactly on the `LOAD_WITNESS_INTERVAL` boundary is the sole
/// emitter for that window (fetch_add hands each multiple to exactly one core), so there is no
/// double-print and no reader lock. Change-only: it prints only when the packed per-core busy-percents
/// differ from the last emission. Cheap on the non-boundary passes (one relaxed fetch_add + a modulo).
fn load_witness_tick() {
    let n = LOAD_WITNESS_TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    if n % LOAD_WITNESS_INTERVAL != 0 {
        return;
    }
    let mut packed = 0u64;
    let mut ctx_now = 0u64;
    for cpu in 0..NUM_CPUS.min(8) {
        let ld = core_load(cpu);
        // SCHED-8: pack 255 for an untracked (stale/never-run) core so a tracked→untracked transition
        // changes the signature and re-emits the line; real percents are 0..100 so 255 never collides.
        let byte = if ld.tracked { ld.busy_pct_recent.min(254) as u64 } else { 255 };
        packed |= byte << (cpu * 8);
        ctx_now += ld.ctx_switches;
    }
    if packed == LOAD_WITNESS_LAST.swap(packed, Ordering::Relaxed) {
        return; // unchanged since the last window — stay quiet
    }
    let ctx_delta = ctx_now.saturating_sub(LOAD_WITNESS_CTX.swap(ctx_now, Ordering::Relaxed));
    // SCHED-8: an untracked core prints `--` (no live number) rather than its frozen last-window percent.
    let c = |i: usize| {
        let ld = core_load(i);
        if ld.tracked {
            alloc::format!("{}%", ld.busy_pct_recent)
        } else {
            alloc::string::String::from("--")
        }
    };
    serial_println!(
        ":: SCHED: load c0={} c1={} c2={} c3={} (ctx +{}/win) ::",
        c(0), c(1), c(2), c(3), ctx_delta
    );
    pulse5_witness();
    spread4_witness(); // SPREAD-4: the placement signal beside the load it is derived from
    prio_witness(); // SCHED-PRIO: who WON those dispatches, beside where they were placed
}

/// PULSE-5 — the proof line for age-on-read. It says three things and nothing else: how long each
/// core has been inside its current task RIGHT NOW (`live cN=..ms` — the term the reported percents
/// now include and previously omitted entirely), the worst such span seen so far (`span_max` — the
/// staleness the old `recent_pct` would have been carrying, invisibly, for that whole span), and how
/// many windows the FOLD path closed (`folds` — unchanged machinery, kept so the line shows the
/// dispatch-boundary path still working rather than replaced; reads simply no longer wait on it).
/// A `span_max` well past `window` beside a `SCHED: load` line that still reads honestly busy is the
/// whole arc in one line.
///
/// Two callers, deliberately: `load_witness_tick` (metal, the attended-boot proof, inheriting that
/// line's change-only suppression so it adds no steady-state chatter) and `load_accounting_witness`
/// (once, in the QEMU battery — otherwise nothing in the gate exercises the aged read at all, since
/// `timer_preempt` never runs on raspi4b). Reads only; safe from any core.
fn pulse5_witness() {
    let mut live_ms = [0u64; 4];
    for cpu in 0..NUM_CPUS.min(4) {
        let span = ACCT[cpu].live_span_cyc();
        PULSE5_SPAN_MAX_CYC.fetch_max(span, Ordering::Relaxed);
        live_ms[cpu] = cyc_to_ms(span);
    }
    // SPIN-1 (2026-07-30, the P87/P92/P93 desktop lockup): a core inside ONE task for >10 s while
    // the witness still runs is the wedge signature ([prio] el0 huge, svc=0, comp2 rate collapsed)
    // — and until now the line never NAMED the task. Cross-CPU current read: the pointer load is
    // atomic; deref is safe in practice because a task that has been current for 10 s is
    // definitionally not mid-drop. Prints every witness pass while the condition holds.
    for cpu in 0..NUM_CPUS.min(4) {
        if cyc_to_ms(ACCT[cpu].live_span_cyc()) > 10_000 {
            let raw = SCHED[cpu].current.load(Ordering::Acquire) as *const Task;
            if !raw.is_null() {
                let (id, name, st) = unsafe { ((*raw).id, (*raw).name, (*raw).state.load(Ordering::Relaxed)) };
                let (rxl, rxc, rxw) = {
                    #[cfg(feature = "baremetal")]
                    { crate::arch::serial::RX_READY.debug_state() }
                    #[cfg(not(feature = "baremetal"))]
                    { (false, 0i64, 0usize) }
                };
                serial_println!(
                    "[spin1] cpu={} span={}ms task={}:{} state={} park={} | rx_ready locked={} count={} waiters={} | sem_stalls={} sem_spin_max={} — one task has owned this core the whole span; the [prio]/[comp2] lines beside this name the starvation",
                    cpu, cyc_to_ms(ACCT[cpu].live_span_cyc()), id, name, st,
                    SCHED[cpu].park_kind.load(Ordering::Relaxed),
                    rxl as u32, rxc, rxw,
                    SEM_STALL_EPISODES.load(Ordering::Relaxed),
                    SEM_SPIN_MAX.load(Ordering::Relaxed)
                );
            }
        }
    }
    serial_println!(
        "[pulse5] live c0={}ms c1={}ms c2={}ms c3={}ms span_max={}ms window={}ms folds={}",
        live_ms[0], live_ms[1], live_ms[2], live_ms[3],
        cyc_to_ms(PULSE5_SPAN_MAX_CYC.load(Ordering::Relaxed)),
        cyc_to_ms(load_window_cyc()),
        PULSE5_FOLD_WINDOWS.load(Ordering::Relaxed),
    );
}

/// SCHED-PRIO — the proof line for the interactive service band, in the `[pulse5]`/`[spread4]` mould
/// and emitted from the same two sites, so the dispatch share reads beside the load it explains.
///
/// It says four things, all as PER-WINDOW deltas (the running totals follow, so a single line is
/// still interpretable if the previous one was suppressed):
///
///   * `svc` — dispatches won by a task whose BASE priority is in the band. On a busy panel this is
///     the compositor + router + HID pump getting the core the moment they are ready.
///   * `el0` — dispatches of EL0/user tasks over the same window. `svc` and `el0` are not a partition
///     (ordinary `PRIO_NORMAL` kernel workers are in neither); they are the two populations the arc
///     is about, and their RATIO is the reading. The band is working when `svc` is a small, steady
///     share and `el0` stays LARGE — a collapsing `el0` would mean the fleet is being starved, which
///     is the failure this arc must not cause.
///   * `defer` — service-band wakes that found a lower-band task running on the target core, i.e. the
///     residual "the compositor was ready and still had to wait" (bounded to one tick by
///     `preempt_hint`). Large `defer` beside large `svc` is contention being resolved; large `defer`
///     with `svc` near zero would mean the band is not being granted and something is wrong.
///   * `agedin` — anti-starvation relocations that lifted a below-band task INTO the band. This is
///     the fairness valve; a nonzero figure under load is the proof that EL0 cannot be excluded.
///
/// Reads only, lock-free, safe from any core, no `pi` gate (matching its two neighbours): the
/// counters exist on every aarch64 build and simply stay zero where there is no EL0 and no panel.
///
/// THREE callers, and the third is why this is `pub`. `load_witness_tick` and
/// `load_accounting_witness` are the two `[pulse5]`/`[spread4]` sites; the first is metal-only
/// (`timer_preempt` never runs on raspi4b) and the second fires exactly ONCE, early, before the panel
/// tasks are spawned at all — so between them nothing would ever print this line at the moment it is
/// about: a live compositor contending with a live fleet. The third caller is the render task's own
/// rate-limited `[sched6]` block (`main.rs::render_service`), which ticks on both QEMU and metal for
/// as long as the panel is up, and which reports the composites/s figure this line explains.
pub fn prio_witness() {
    let mut svc = 0u64;
    let mut el0 = 0u64;
    let mut defer = 0u64;
    let mut aged = 0u64;
    for cpu in 0..NUM_CPUS {
        svc += PRIO_SVC_DISPATCH[cpu].load(Ordering::Relaxed);
        el0 += PRIO_EL0_DISPATCH[cpu].load(Ordering::Relaxed);
        defer += PRIO_DEFER[cpu].load(Ordering::Relaxed);
        aged += PRIO_AGED_IN[cpu].load(Ordering::Relaxed);
    }
    let d_svc = svc.saturating_sub(PRIO_LAST_SVC.swap(svc, Ordering::Relaxed));
    let d_el0 = el0.saturating_sub(PRIO_LAST_EL0.swap(el0, Ordering::Relaxed));
    let d_defer = defer.saturating_sub(PRIO_LAST_DEFER.swap(defer, Ordering::Relaxed));
    let d_aged = aged.saturating_sub(PRIO_LAST_AGED.swap(aged, Ordering::Relaxed));
    serial_println!(
        "[prio] svc={} el0={} defer={} agedin={} /win (band>={}, totals svc={} el0={})",
        d_svc, d_el0, d_defer, d_aged, PRIO_SERVICE, svc, el0,
    );
}

/// SPREAD-4 — the proof line for live residents + re-placement, in the `[pulse5]` mould and emitted
/// from the same two sites, so the placement signal is readable beside the load numbers it is derived
/// from. It says exactly three things:
///
///   * `cN=active/committed` — per core, the runnable resident count `pick_cpu` now keys on, over the
///     SPREAD-3 committed count it used to. `2/5` is the arc in one field: three of that core's five
///     EL0 residents are parked and were, until now, steering placement away from a core with room.
///   * `rewake` / `stay` — how many EL0 wakes moved to a lighter core and how many did not, counted
///     over the wakes that ASKED (SPREAD-5: long parks only). A fleet in balance is nearly all `stay`;
///     a burst of `rewake` is a pile-up being taken apart. Post-SPREAD-5 `rewake` should track real
///     focus changes — single digits over a session, not the thousands P75 measured.
///   * `short` — SPREAD-5: wakes that skipped placement because the park was a frame-loop micro-park.
///     Expected to dominate `rewake + stay` by orders of magnitude on a windowed fleet; that ratio IS
///     the damping. `short` climbing while `rewake` stays flat is the arc working.
///   * `refresh` — SPREAD-6: micro-park wakes that asked anyway because the task's last placement ask
///     was over `PLACE_REFRESH_MS` ago (the escapement that unlatches stale packing — the residual
///     "predestined fps"). ~4/s per continuously-running EL0 task; its OUTCOMES land in
///     `rewake`/`stay`, so `refresh` climbing with `rewake` flat is a fleet already in place.
///   * `margin` / `minpark` — the two thresholds those decisions were made against (runnable-resident
///     gap, and minimum park duration), so a reading is interpretable without the source.
///
/// Reads only, lock-free, safe from any core. Not `pi`-gated, matching `pulse5_witness` beside it: it
/// is one introspection line on a path that already prints one, and the counters it reads exist on
/// every aarch64 build (they simply stay zero where there is no EL0).
fn spread4_witness() {
    serial_println!(
        "[spread4] live c0={}/{} c1={}/{} c2={}/{} c3={}/{} rewake={} stay={} short={} refresh={} margin={} minpark={}ms",
        el0_active(0),
        EL0_RESIDENTS[0].0.load(Ordering::Relaxed),
        el0_active(1),
        EL0_RESIDENTS[1].0.load(Ordering::Relaxed),
        el0_active(2),
        EL0_RESIDENTS[2].0.load(Ordering::Relaxed),
        el0_active(3),
        EL0_RESIDENTS[3].0.load(Ordering::Relaxed),
        SPREAD4_REWAKE.load(Ordering::Relaxed),
        SPREAD4_STAY.load(Ordering::Relaxed),
        SPREAD5_SHORT_STAY.load(Ordering::Relaxed),
        SPREAD6_REFRESH.load(Ordering::Relaxed),
        REWAKE_MARGIN,
        REWAKE_MIN_PARK_MS,
    );
    spread7_witness();
    spread10_witness();
}

/// SPREAD-10 — the co-placement proof line, beside `[spread4]` (same emit sites, so the sibling map
/// is readable next to the per-core load it biases). `slots 1c/2c/3c+` is the cores-per-slot
/// histogram over live slots (slots whose committed count is zero everywhere are not counted): the
/// arc's expected metal signature is the population collapsing into `1c`/`2c` under storm, with the
/// `[fluid3]` park percentiles on the same wire dropping out of the millisecond buckets and the
/// per-vug `[wcn]` rates converging upward. `co_moves` is cumulative placements the co-residency
/// bonus DECIDED (sibling-lane rewakes + bonus-steered spawns): it should step at convergence edges
/// (spawn bursts, load shifts) and go flat between them — climbing steadily beside a static
/// histogram would be thrash, which the strictly-increasing-co-residency move rule is built to
/// exclude. Reads only, lock-free, safe from any core; all-zero (no live slot) on the QEMU battery
/// before EL0 exists, which proves the wiring exactly as `[spread4]`'s zero baseline does.
fn spread10_witness() {
    let mut on1 = 0u32;
    let mut on2 = 0u32;
    let mut on3 = 0u32;
    for slot in 1..KILL_ASID_SLOTS {
        let mut cores = 0u32;
        for cpu in 0..NUM_CPUS {
            if SLOT_CORE_RES[cpu].0[slot].load(Ordering::Relaxed) > 0 {
                cores += 1;
            }
        }
        match cores {
            0 => {}
            1 => on1 += 1,
            2 => on2 += 1,
            _ => on3 += 1,
        }
    }
    serial_println!(
        "[spread10] slots 1c={} 2c={} 3c+={} co_moves={} ymoves={}",
        on1,
        on2,
        on3,
        SPREAD10_CO_MOVES.load(Ordering::Relaxed),
        SPREAD11_YIELD_MOVES.load(Ordering::Relaxed),
    );
}

/// SPREAD-7 — the wake-quantization proof line, emitted beside `[spread4]` from the same sites.
/// `quant` is how many EL0 wakes landed in the tick-quantized arm (equal-or-higher band than the
/// target's running task, below the service band; see `preempt_hint`); `trim` (SPREAD-8) is how
/// many of those wakes actually shortened the incumbent's countdown — the same-band trim policy
/// firing; `wake2disp` prices ALL EL0 wakes: mean and max CNTPCT-derived microseconds from
/// `make_ready` to first dispatch, over `n` wakes. The P79 ceiling read as `quant` climbing at the
/// fleet park rate with `wd_mean` in the thousands of microseconds (half-quantum scale); with
/// SPREAD-8 in force the expected signature is `trim` climbing beside `quant` and `wd_mean`
/// bounded by one tick (~4000 us worst). A healthy fleet reads `wd_mean` in the tens (SGI +
/// dispatch pass). Cumulative counters, reads only, safe from any core.
fn spread7_witness() {
    let n = SPREAD7_WD_N.load(Ordering::Relaxed);
    let sum = SPREAD7_WD_SUM.load(Ordering::Relaxed);
    let frq = load_window_cyc().saturating_mul(4).max(1); // == CNTFRQ_EL0, cached
    let cyc_per_us = (frq / 1_000_000).max(1);
    serial_println!(
        "[spread7] quant={} trim={} wake2disp n={} mean={}us max={}us",
        SPREAD7_QUANT.load(Ordering::Relaxed),
        SPREAD8_TRIM.load(Ordering::Relaxed),
        n,
        sum.checked_div(n).unwrap_or(0) / cyc_per_us,
        SPREAD7_WD_MAX.load(Ordering::Relaxed) / cyc_per_us,
    );
    // SPREAD-9 — the immediate-preemption proof line, beside [spread7] so the before/after is one
    // wire read. `kick` = IPI-receipt preemptions performed (`ipi_preempt` dispatched instead of
    // returning to the incumbent); `svc_lat` = service-band wake-to-dispatch, the number the kicks
    // exist to collapse. Metal expectation under fleet load: `kick` climbing with the service wake
    // rate and `svc_lat` mean < 100 us (IPI + dispatch pass — down from tick scale). In QEMU the
    // SGIs are live but timer-driven service wakes are not, so the gate proves the counters and the
    // switch path, not the latency.
    let sn = SPREAD9_SVC_N.load(Ordering::Relaxed);
    serial_println!(
        "[spread9] kick={} svc_lat n={} mean={}us max={}us",
        SPREAD9_KICK.load(Ordering::Relaxed),
        sn,
        SPREAD9_SVC_SUM.load(Ordering::Relaxed).checked_div(sn).unwrap_or(0) / cyc_per_us,
        SPREAD9_SVC_MAX.load(Ordering::Relaxed) / cyc_per_us,
    );
}

/// SCHED-2 on-demand per-core load table (the `top` shell verb's body, and a serial witness). Prints
/// one row per core: recent busy percent (rolling window), cumulative context switches, and the last
/// task (id + name). Reads `core_load` per core — introspection only, safe from any core.
pub fn load_table(mut line: impl FnMut(&str)) {
    line("core  busy%  ctx-switches  last-task");
    for cpu in 0..NUM_CPUS {
        let ld = core_load(cpu);
        // SCHED-8: `--` for a core not being accounted right now (left `run()` / never ran it), so the
        // busy% column never shows a frozen snapshot as if it were live.
        let busy = if ld.tracked {
            alloc::format!("{}", ld.busy_pct_recent)
        } else {
            alloc::string::String::from("--")
        };
        line(&alloc::format!(
            "{:>3}   {:>4}   {:>11}   {} (tid {})",
            cpu, busy, ld.ctx_switches, ld.last_task, ld.last_task_id
        ));
    }
}

/// SCHED-2 non-degeneracy witness (the QEMU-gate proof): assert the per-core accounting is LIVE — at
/// least one core shows busy activity and its context-switch counter has advanced past zero. A
/// regression that freezes the accounting (counters stuck at 0, no window ever completes) FAILs this
/// loudly. Uncounted (not part of the timed battery); prints a single PASS/FAIL line. Reads only.
pub fn load_accounting_witness() {
    let mut any_busy = false;
    let mut any_ctx = false;
    let mut max_pct = 0u32;
    let mut total_ctx = 0u64;
    for cpu in 0..NUM_CPUS {
        let ld = core_load(cpu);
        if ld.busy_pct_recent > 0 {
            any_busy = true;
        }
        if ld.ctx_switches > 0 {
            any_ctx = true;
        }
        max_pct = max_pct.max(ld.busy_pct_recent);
        total_ctx += ld.ctx_switches;
    }
    if any_busy && any_ctx {
        serial_println!(
            ":: AARCH64 SCHED: load-accounting PASS (max busy {}%, {} ctx-switches total) ::",
            max_pct, total_ctx
        );
    } else {
        serial_println!(
            ":: AARCH64 SCHED: load-accounting FAIL (any_busy={}, any_ctx={}) ::",
            any_busy, any_ctx
        );
    }
    // PULSE-5: the aged-read state behind the percents just asserted, on the one path the QEMU
    // battery actually reaches. Separate line, so the PASS/FAIL line the gate matches is untouched.
    pulse5_witness();
    // SPREAD-4: same reasoning, same site. In the QEMU battery this runs before any EL0 task exists,
    // so it reads all-zero — which is the honest baseline, and it proves the counters are wired and
    // the line is emitted. The numbers that matter are the ones `load_witness_tick` prints on metal.
    spread4_witness();
    // SCHED-PRIO: same reasoning again. `timer_preempt` never runs on raspi4b, so `load_witness_tick`
    // is unreachable there and this is the ONLY site that emits `[prio]` in the QEMU battery. The
    // dispatch counters are live on that path (the cooperative loop dispatches through the same
    // `dispatch_next`), so the line carries real numbers, not a wired-and-zero placeholder.
    prio_witness();
}

/// M3b/M4a/M4-capstone: turn on preemptive scheduling and put a workload on the APs, then flip
/// SCHED_ACTIVE (enables `timer_preempt`) and SCHED_GO (releases the APs into `run`). The workload:
///   * first online AP — a busy-pair (preemption regression) + the M4a `sleep_ticks` sleeper;
///   * (with >= 3 online APs) the M4 CAPSTONE: one coordinator on the first AP runs every sync
///     primitive's cross-core self-test in sequence, with workers on the two other APs.
/// On metal the busy-pair INTERLEAVES, the sleeper parks-then-wakes on its own tick, and every
/// capstone step is woken cross-core by a real reschedule SGI. In QEMU (no Group-1 IRQ delivery) the
/// busy tasks run sequentially, the sleeper self-skips, and every block/wake is carried by the run()
/// busy-poll (SGI delivery being the metal-only half). Call AFTER the BSP's cooperative demo.
pub fn start_aps(online: &[usize]) {
    // SCHED-3: register every online AP as a candidate for CPU_AUTO load-balanced placement. The BSP
    // (core 0) is registered SEPARATELY by `run_bsp` if/when it enters the scheduler after its boot
    // duties (SMP-BAL); until then it stays off the candidate set — a CPU_AUTO task only ever lands on a
    // core that actually drains its run queue. No effect on the caller-pinned spawns below (they name
    // their core), so the workload placement here is byte-identical.
    for &c in online {
        mark_online(c);
    }
    // SCHED-3: prove load-balanced placement spreads unpinned work across the online cores. Run HERE,
    // BEFORE the caller-pinned workload below is staged, so every online core's run queue is empty and
    // the depth-balancer fans the unpinned tasks evenly across ALL of them (a proof that once a core is
    // loaded the balancer correctly steers away from it belongs to the accounting witness, not this
    // one). Gated behind `witness` (armed for `kernel8-test`, OFF for a default `kernel8` boot) so quiet
    // boot stays quiet, and `pi` so it fires only on the target/gate — byte-identical for jetson/virt.
    #[cfg(all(feature = "pi", feature = "witness"))]
    placement_spread_witness();
    if let Some(&c) = online.first() {
        spawn("busy-a", preempt_body, c * 10, c);
        spawn("busy-b", preempt_body, c * 10 + 1, c);
        spawn("sleeper", sleep_demo_body, c, c);
    }
    if online.len() >= 3 {
        // Reserve every primitive's waiter capacity on the BSP before any task can block on it.
        CAP_SEM.init();
        CAP_MTX.init();
        CAP_CHAN.init();
        CAP_CV_PRED.init();
        CAP_CV.init();
        CAP_RW.init();
        *CAP_CORES.lock() = [online[1], online[2]];
        spawn("capstone", capstone_body, 0, online[0]);
    } else {
        serial_println!(":: AARCH64 SCHED: capstone skipped (needs >= 3 online APs) ::");
    }
    SCHED_ACTIVE.store(true, Ordering::Release);
    SCHED_GO.store(true, Ordering::Release);
    serial_println!(
        ":: AARCH64 SCHED: preemption ON; busy-pair + sleep + full M4 capstone on APs {:?} ::",
        online
    );

    // SCHED-3: with the APs now released, the unpinned workers placed at the top of this fn drain and
    // record the core they actually RAN on — corroboration that the placement spread is real execution,
    // not just an enqueue decision. Informational (non-gating: cross-core drain timing under QEMU is not
    // guaranteed); the PASS gate was the deterministic placement spread above.
    #[cfg(all(feature = "pi", feature = "witness"))]
    placement_spread_epilogue();

    // SMP-BAL: with the placement/capstone workload drained and the APs idle-polling in `run`, exercise
    // WORK STEALING end to end — pile movable tasks on one core and prove idle siblings pull them over.
    #[cfg(all(feature = "pi", feature = "witness"))]
    smpbal_steal_witness();
}

// ---------------------------------------------------------------------------------------------
// SCHED-3 — load-balanced-placement spread witness (QEMU-testable, default-quiet)
// ---------------------------------------------------------------------------------------------
//
// Asserts that N `CPU_AUTO` (unpinned) spawns land on >= 3 DISTINCT cores — the direct inverse of the
// metal imbalance (all unpinned work piling onto one caller-picked pin). Run with every online core's
// run queue empty (top of `start_aps`), so the depth-balancer fans the tasks evenly; the LANDING core
// of each auto-placement (deterministic — decided at enqueue by `pick_cpu`) is folded into a bitset and
// the distinct-core count is the PASS gate. The workers also record the core they actually RAN on
// (`placement_spread_epilogue`, after the APs are released) as non-gating corroboration.
#[cfg(all(feature = "pi", feature = "witness"))]
const SPREAD_N: usize = 12;
#[cfg(all(feature = "pi", feature = "witness"))]
static SPREAD_RAN_MASK: AtomicU32 = AtomicU32::new(0);

#[cfg(all(feature = "pi", feature = "witness"))]
fn spread_worker(_: usize) {
    let cpu = percpu::this_cpu().cpu_index as usize;
    SPREAD_RAN_MASK.fetch_or(1u32 << (cpu & 31), Ordering::Relaxed);
}

#[cfg(all(feature = "pi", feature = "witness"))]
pub fn placement_spread_witness() {
    let online_n = (0..NUM_CPUS)
        .filter(|&c| ONLINE_MASK[c].load(Ordering::Acquire))
        .count();
    if online_n < 3 {
        serial_println!(
            ":: AARCH64 SCHED: placement-spread witness SKIP (needs >= 3 online cores, have {}) ::",
            online_n
        );
        return;
    }
    SPREAD_RAN_MASK.store(0, Ordering::Relaxed);
    let mut placed_mask: u32 = 0;
    for _ in 0..SPREAD_N {
        // Balance across the online cores, then pin to the chosen core so the landing is observable.
        // Each spawn pushes onto its core's queue, so the depth signal `pick_cpu` reads climbs there and
        // the next placement moves on — genuine load spread across the (empty) cores, not a fixed rotation.
        let cpu = pick_cpu(CPU_AUTO);
        spawn_prio("spread", spread_worker, 0, cpu, PRIO_NORMAL);
        placed_mask |= 1u32 << (cpu & 31);
    }
    let distinct = placed_mask.count_ones();
    if distinct >= 3 {
        serial_println!(
            ":: AARCH64 SCHED: placement-spread PASS ({} unpinned tasks -> {} distinct cores, mask {:#06b}) ::",
            SPREAD_N, distinct, placed_mask
        );
    } else {
        serial_println!(
            ":: AARCH64 SCHED: placement-spread FAIL ({} unpinned tasks -> only {} distinct cores, mask {:#06b}) ::",
            SPREAD_N, distinct, placed_mask
        );
    }
}

/// SCHED-3 corroboration: after the APs are released, give the unpinned spread workers a bounded window
/// to drain and report the cores they actually RAN on. Informational only (never a PASS/FAIL gate) —
/// cross-core drain timing under QEMU is not guaranteed, so this confirms live execution without gating.
#[cfg(all(feature = "pi", feature = "witness"))]
pub fn placement_spread_epilogue() {
    busy_delay_ms(20);
    let ran_mask = SPREAD_RAN_MASK.load(Ordering::Relaxed);
    serial_println!(
        ":: AARCH64 SCHED: placement-spread ran-mask {:#06b} ({} cores observed running) ::",
        ran_mask,
        ran_mask.count_ones()
    );
}

// ---------------------------------------------------------------------------------------------
// SMP-BAL — work-stealing spread witness (QEMU-testable, default-quiet)
// ---------------------------------------------------------------------------------------------
//
// Directly exercises STEALING (not just placement): concentrate N steal-eligible tasks on ONE core with
// every OTHER online core idle in `run`, then assert the tasks actually RAN on >= 2 distinct cores — the
// only way that can happen with a single-core spawn is that idle cores pulled the backlog over via
// `try_steal`. Runs at the END of `start_aps`, after the capstone/placement workload has drained (so the
// APs are idle-polling in `run`), and stages tasks that each spin briefly so the home core cannot drain
// the whole pile before its idle siblings steal. `pi`+`witness` gated → byte-identical off the gate.

#[cfg(all(feature = "pi", feature = "witness"))]
const SMPBAL_N: usize = 4;
#[cfg(all(feature = "pi", feature = "witness"))]
static SMPBAL_RAN_MASK: AtomicU32 = AtomicU32::new(0);

#[cfg(all(feature = "pi", feature = "witness"))]
fn smpbal_worker(_: usize) {
    let cpu = percpu::this_cpu().cpu_index as usize;
    SMPBAL_RAN_MASK.fetch_or(1u32 << (cpu & 31), Ordering::Relaxed);
    busy_delay_ms(3); // hold the core long enough that idle siblings steal the rest of the pile
}

/// SMP-BAL witness-only spawn: enqueue a STEAL-ELIGIBLE task PINNED to `cpu` (i.e. `steal_ok = true` while
/// starting concentrated on one core) — the exact runtime state "a movable task is currently queued on a
/// loaded core". Mirrors `spawn_inner` minus the placement policy/log; used solely by `smpbal_steal_witness`.
#[cfg(all(feature = "pi", feature = "witness"))]
fn spawn_stealable_on(name: &'static str, entry: fn(usize), arg: usize, cpu: usize) {
    let mut stack: Box<[u8]> = alloc::vec![0u8; TASK_STACK_SIZE].into_boxed_slice();
    let ctx_sp = build_initial_frame(&mut stack, task_trampoline);
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
        priority: PRIO_NORMAL,
        wait_ticks: 0,
        done_sem: None,
        user_entry: 0,
        user_sp: 0,
        user_ttbr0: 0,
        steal_ok: true, // the point of the fixture: movable, but staged on one core
        park_cyc: 0, // SPREAD-5: never parked yet; the first park_blocked stamps it.
        place_cyc: now_cyc(), // SPREAD-6: spawn placement counts as the first ask of the question
        wake_cyc: 0, // SPREAD-7: never woken yet; `make_ready` stamps it
    });
    rq(cpu).push(task);
    poke_cpu(cpu);
}

/// SMP-BAL — the deliverable-4 spread test. Pile `SMPBAL_N` steal-eligible tasks on ONE online core while
/// the others idle in `run`, wait a bounded window, and PASS iff they ran on >= 2 distinct cores (proving
/// idle cores stole the backlog). Runs LAST in `start_aps` (APs already idle post-capstone). Emits
/// `:: SMPBAL: spread test — tasks=N cores-used=M :: PASS ::`.
#[cfg(all(feature = "pi", feature = "witness"))]
pub fn smpbal_steal_witness() {
    let online: alloc::vec::Vec<usize> = (0..NUM_CPUS)
        .filter(|&c| ONLINE_MASK[c].load(Ordering::Acquire))
        .collect();
    if online.len() < 2 {
        serial_println!(
            ":: SMPBAL: spread test SKIP (needs >= 2 online cores, have {}) ::",
            online.len()
        );
        return;
    }
    let home = online[0];
    SMPBAL_RAN_MASK.store(0, Ordering::Relaxed);
    for _ in 0..SMPBAL_N {
        spawn_stealable_on("smpbal", smpbal_worker, 0, home);
    }
    // Give the idle siblings time to steal + everyone to drain (each worker spins ~3 ms).
    busy_delay_ms(60);
    let mask = SMPBAL_RAN_MASK.load(Ordering::Relaxed);
    let used = mask.count_ones();
    if used >= 2 {
        serial_println!(
            ":: SMPBAL: spread test — tasks={} cores-used={} :: PASS ::",
            SMPBAL_N, used
        );
    } else {
        serial_println!(
            ":: SMPBAL: spread test — tasks={} cores-used={} (mask {:#06b}) :: FAIL ::",
            SMPBAL_N, used, mask
        );
    }
}

// ---------------------------------------------------------------------------------------------
// AARCH64-PRIO M3 — priority + anti-starvation aging witness (cooperative, self-checking, bounded)
// ---------------------------------------------------------------------------------------------
//
// Proves the two halves of the multilevel scheduler in one cooperative pass on a single core:
//   1. FIXED PRIORITY — a CPU runs the highest non-empty level first (a low task is NOT picked while
//      higher-priority work is ready).
//   2. AGING — that low task is nonetheless NOT starved: under CONTINUOUS high-priority load it is
//      relocated up, level by level, until it becomes dispatchable, and runs BEFORE the load drains.
//
// The load is `PW_HIGH_TASKS` PRIO_HIGH tasks that each yield `PW_HIGH_ITERS` times (so PRIO_HIGH is
// continuously non-empty for many dispatch passes), plus ONE PRIO_LOW task that runs to completion in
// a single dispatch (no yield — so once aging lifts it into a dispatchable level it finishes at once,
// without re-basing). WITHOUT aging the low task could only run after every high task exited; the
// witness asserts it ran WHILE at least one high task was still active (`PW_LOW_UNDER_LOAD`), which is
// only possible via aging. BOUNDED + never hangs: every task does finite work and the low task never
// yields, so `run_until_empty` always drains — a broken aging path FAILs loudly (low runs after the
// load drained → `under_load == false`), it never wedges the core. All cooperative (yield/exit only),
// so it needs no timer and runs identically on the `virt` GICv3 boot core (test-arm 40) and on metal.
const PW_HIGH_TASKS: usize = 2;
const PW_HIGH_ITERS: usize = 40;
static PW_HIGH_ACTIVE: AtomicU32 = AtomicU32::new(0);
static PW_LOW_RAN: AtomicBool = AtomicBool::new(false);
static PW_LOW_UNDER_LOAD: AtomicBool = AtomicBool::new(false);

/// High-priority load: yield many times (keeping PRIO_HIGH continuously ready), then retire.
fn pw_high_body(_: usize) {
    for _ in 0..PW_HIGH_ITERS {
        yield_now();
    }
    PW_HIGH_ACTIVE.fetch_sub(1, Ordering::Relaxed);
}

/// The starvation candidate: runs to completion in ONE dispatch (no yield). Records that it ran and
/// whether high-priority load was still active at that moment — the aging proof.
fn pw_low_body(_: usize) {
    PW_LOW_RAN.store(true, Ordering::Relaxed);
    if PW_HIGH_ACTIVE.load(Ordering::Relaxed) > 0 {
        PW_LOW_UNDER_LOAD.store(true, Ordering::Relaxed);
    }
}

/// Run the AARCH64-PRIO M3 witness cooperatively on `cpu` and print the PASS/FAIL line. Self-contained:
/// it stages its own tasks and drains them via `run_until_empty`, leaving the queue empty for the
/// caller. Emits `:: AARCH64 SCHED: priority+aging PASS ::` on success.
pub fn priority_aging_witness(cpu: usize) {
    PW_HIGH_ACTIVE.store(PW_HIGH_TASKS as u32, Ordering::Relaxed);
    PW_LOW_RAN.store(false, Ordering::Relaxed);
    PW_LOW_UNDER_LOAD.store(false, Ordering::Relaxed);
    serial_println!(
        ":: AARCH64 SCHED: priority+aging witness — {} PRIO_HIGH loaders vs 1 PRIO_LOW candidate on cpu {} ::",
        PW_HIGH_TASKS,
        cpu
    );
    // Stage the load first (ahead of the candidate in FIFO), then the low candidate.
    for _ in 0..PW_HIGH_TASKS {
        spawn_prio("pw-high", pw_high_body, 0, cpu, PRIO_HIGH);
    }
    spawn_prio("pw-low", pw_low_body, 0, cpu, PRIO_LOW);
    run_until_empty(cpu);
    let ran = PW_LOW_RAN.load(Ordering::Relaxed);
    let under_load = PW_LOW_UNDER_LOAD.load(Ordering::Relaxed);
    if ran && under_load {
        serial_println!(":: AARCH64 SCHED: priority+aging PASS ::");
    } else {
        serial_println!(
            ":: AARCH64 SCHED: priority+aging FAIL (low_ran={}, under_load={}) ::",
            ran,
            under_load
        );
    }
}

// ---------------------------------------------------------------------------------------------
// PRIO-MIX M1 — dedicated priority-mix stress witness (cooperative, self-checking, bounded)
// ---------------------------------------------------------------------------------------------
//
// The AARCH64-PRIO landing proved priority + aging in ONE combined scenario (`priority_aging_witness`)
// and DEFERRED a dedicated *mix* witness (the Pi metal ledger records it as "mix witness deferred").
// This closes it. Under a genuine mixed-priority load it proves BOTH halves of the multilevel
// scheduler on one core, back to back, and reports each half INDEPENDENTLY:
//
//   * STRICT — from a DRAINED queue seeded with `PM_STRICT_HIGH` PRIO_HIGH short tasks (each runs to
//     completion in one dispatch, no yield) + 1 PRIO_LOW short task, the CPU dispatches the whole
//     PRIO_HIGH level before the PRIO_LOW task. A monotonic completion-ORDER counter records the
//     finish order: strict holds iff every high task finished before the low one (the low task's
//     completion index is last). This is an ORDERING claim — valid ONLY on a cooperative drained
//     start, which is how the witness ALWAYS runs (both call sites run it before preemption is on);
//     it is deliberately NOT asserted under preemption.
//   * AGED-RESCUE — from a drained queue seeded with `PM_AGE_HIGH` PRIO_HIGH loaders that each yield
//     `PM_AGE_ITERS` times (keeping PRIO_HIGH continuously ready) + 1 PRIO_LOW no-yield canary, the
//     low task is nonetheless rescued by aging and completes WHILE high load is still active
//     (`under_load`) — the anti-starvation proof. This is a BOUNDED-RESCUE claim (the low task
//     completes before the finite load drains), NOT an ordering claim, so it stays honest under real
//     preemption on Pi metal: the aging clock is dispatch-PASSES (`SchedCpu::age_passes` — it advances
//     on cooperative AND preemptive dispatch alike), so a rescued-before-drain low is bounded in either
//     regime. Same 2-loaders x 40-iters shape as the proven `priority_aging_witness`.
//
// BOUNDED + never hangs a battery: every task does finite work and NEITHER low task ever yields, so
// `run_until_empty` always drains — a broken scheduler FAILs loudly (strict: low not last;
// aged-rescue: low ran only after the load drained), it never wedges the core. That finite-work
// guarantee IS the watchdog bound; no timer is needed, so it runs identically on the `virt` GICv3
// boot core (test-arm 40) and in the Pi kernel8 battery (`demo_cooperative`, before preemption).
// Telemetry statics are lock-free relaxed (owning-core-only within a cooperative drain).
const PM_STRICT_HIGH: usize = 3;
const PM_AGE_HIGH: usize = 2;
const PM_AGE_ITERS: usize = 40;

// STRICT sub-scenario: a monotonic completion-order source + the low task's finish index + a done count.
static PM_SEQ: AtomicU32 = AtomicU32::new(0);
static PM_LOW_ORDER: AtomicU32 = AtomicU32::new(0);
static PM_HIGH_DONE: AtomicU32 = AtomicU32::new(0);
// AGED-RESCUE sub-scenario: live loader count + the canary's ran/under-load flags (mirrors the proven
// `priority_aging_witness` discriminator).
static PM_AGE_ACTIVE: AtomicU32 = AtomicU32::new(0);
static PM_AGE_LOW_RAN: AtomicBool = AtomicBool::new(false);
static PM_AGE_LOW_UNDER_LOAD: AtomicBool = AtomicBool::new(false);

/// STRICT high task: run to completion in one dispatch (no yield), stamping its completion order.
fn pm_strict_high_body(_: usize) {
    let _ = PM_SEQ.fetch_add(1, Ordering::Relaxed);
    PM_HIGH_DONE.fetch_add(1, Ordering::Relaxed);
}

/// STRICT low task: run to completion in one dispatch (no yield), recording its completion order. It
/// must be the LAST to finish (index == `PM_STRICT_HIGH`) for strict priority to hold.
fn pm_strict_low_body(_: usize) {
    let order = PM_SEQ.fetch_add(1, Ordering::Relaxed);
    PM_LOW_ORDER.store(order, Ordering::Relaxed);
}

/// AGED-RESCUE loader: yield many times (keeping PRIO_HIGH continuously ready), then retire.
fn pm_age_high_body(_: usize) {
    for _ in 0..PM_AGE_ITERS {
        yield_now();
    }
    PM_AGE_ACTIVE.fetch_sub(1, Ordering::Relaxed);
}

/// AGED-RESCUE canary: run to completion in ONE dispatch (no yield). Records that it ran and whether
/// high load was still active at that moment — the aging (bounded-rescue) proof.
fn pm_age_low_body(_: usize) {
    PM_AGE_LOW_RAN.store(true, Ordering::Relaxed);
    if PM_AGE_ACTIVE.load(Ordering::Relaxed) > 0 {
        PM_AGE_LOW_UNDER_LOAD.store(true, Ordering::Relaxed);
    }
}

/// Run the PRIO-MIX M1 witness cooperatively on `cpu` and print the self-checking line. Two bounded,
/// self-contained sub-scenarios drained via `run_until_empty` (each leaves the queue empty for the
/// caller). Emits `:: AARCH64 SCHED: prio-mix witness (strict=..., aged-rescue=...) => PASS/FAIL ::`.
pub fn prio_mix_witness(cpu: usize) {
    serial_println!(
        ":: AARCH64 SCHED: prio-mix witness — {} PRIO_HIGH short + 1 PRIO_LOW (strict), then {} PRIO_HIGH loaders + 1 PRIO_LOW (aged-rescue) on cpu {} ::",
        PM_STRICT_HIGH,
        PM_AGE_HIGH,
        cpu
    );

    // --- Sub-scenario 1: STRICT priority from a drained queue. Load first (ahead in FIFO), then low. ---
    PM_SEQ.store(0, Ordering::Relaxed);
    PM_LOW_ORDER.store(0, Ordering::Relaxed);
    PM_HIGH_DONE.store(0, Ordering::Relaxed);
    for _ in 0..PM_STRICT_HIGH {
        spawn_prio("pm-hi", pm_strict_high_body, 0, cpu, PRIO_HIGH);
    }
    spawn_prio("pm-lo", pm_strict_low_body, 0, cpu, PRIO_LOW);
    run_until_empty(cpu);
    let high_done = PM_HIGH_DONE.load(Ordering::Relaxed) as usize;
    let low_order = PM_LOW_ORDER.load(Ordering::Relaxed);
    // Every high task finished AND the low task finished last (its completion index == the high count).
    let strict = high_done == PM_STRICT_HIGH && low_order == PM_STRICT_HIGH as u32;

    // --- Sub-scenario 2: AGED-RESCUE under sustained high-priority pressure (drained queue). ---
    PM_AGE_ACTIVE.store(PM_AGE_HIGH as u32, Ordering::Relaxed);
    PM_AGE_LOW_RAN.store(false, Ordering::Relaxed);
    PM_AGE_LOW_UNDER_LOAD.store(false, Ordering::Relaxed);
    for _ in 0..PM_AGE_HIGH {
        spawn_prio("pm-load", pm_age_high_body, 0, cpu, PRIO_HIGH);
    }
    spawn_prio("pm-canary", pm_age_low_body, 0, cpu, PRIO_LOW);
    run_until_empty(cpu);
    // Bounded rescue: the canary ran WHILE the finite load was still active (only possible via aging).
    let aged_rescue =
        PM_AGE_LOW_RAN.load(Ordering::Relaxed) && PM_AGE_LOW_UNDER_LOAD.load(Ordering::Relaxed);

    let pass = strict && aged_rescue;
    serial_println!(
        ":: AARCH64 SCHED: prio-mix witness (strict={}, aged-rescue={}) => {} ::",
        if strict { "PASS" } else { "FAIL" },
        if aged_rescue { "PASS" } else { "FAIL" },
        if pass { "PASS" } else { "FAIL" }
    );
}

/// JC3: run the M4 CAPSTONE on the `virt` boot core ALONE, cooperatively, and never return. Called by
/// `main.rs` right after the boot core drops EL2 -> EL1 (`boot_virt::drop_to_el1`) with its per-CPU
/// (TPIDR_EL1) and EL1 vectors installed. This is the QEMU-testable proof that the scheduler + all six
/// sync primitives run at EL1 on the GICv3 `virt` path.
///
/// Boot-core-only (no SMP): the coordinator AND both "worker" cores are the boot core itself
/// (`CAP_CORES = [cpu, cpu]`), so every cross-core wake in `capstone_body` degrades to a same-core
/// cooperative reschedule — exactly how the Pi CAPSTONE already runs under QEMU (no Group-1 IRQ delivery;
/// "every block/wake carried by the run() busy-poll"). It exercises the SEMANTICS of all six primitives
/// (a real park + switch + wake for each Semaphore/Mutex/Channel/Condvar/RwLock/join), proving they
/// function at EL1; true cross-core contention timing remains the Pi/metal proof. The single-core queue
/// is never transiently empty while CAPSTONE is in flight (each worker is queued before the coordinator
/// blocks and runs to completion between the coordinator's blocks), so the driver makes progress to the
/// final `CAPSTONE COMPLETE` line, then idles.
///
/// No preemption: the drop disabled the physical timer (the shared `__vec_irq` stub banks EL2 state), so
/// `SCHED_ACTIVE` stays FALSE (`timer_preempt` no-ops) and the drive loop busy-polls rather than WFI —
/// never wedging the single core in a wake-less WFI (`crate::arch::hlt` would WFI while `is_live()` still
/// reads true from the pre-drop EL2 `verify_live`). Task bodies still run IRQ-unmasked (`task_trampoline`
/// clears I), but with the timer disabled and the JC2 SMP SGIs long quiescent there is no IRQ source.
pub fn run_capstone_boot_core(cpu: usize) -> ! {
    assert!(cpu < NUM_CPUS, "run_capstone_boot_core: cpu out of range");
    // Reserve every primitive's waiter capacity on the boot core before any task can block on it.
    CAP_SEM.init();
    CAP_MTX.init();
    CAP_CHAN.init();
    CAP_CV_PRED.init();
    CAP_CV.init();
    CAP_RW.init();
    // Coordinator and both workers co-located on the boot core: cross-core wakes become same-core
    // cooperative reschedules.
    *CAP_CORES.lock() = [cpu, cpu];
    serial_println!(
        ":: AARCH64 SCHED (virt): boot core {} at EL1 — running the full M4 CAPSTONE cooperatively ::",
        cpu
    );
    // VUG-HONESTY: witness the parked-core display rule on this virt boot (deterministic, no framebuffer)
    // — a frozen non-demo core reads PARKED, never the demo core's fabricated load. The GICv3/test-arm
    // capture proves the display-honesty fix that completes the merged idle/busy-heartbeat counters.
    let _ = crate::vug::parked_display_witness();
    // AARCH64-PRIO M3: prove fixed-priority + anti-starvation aging before the CAPSTONE. Self-contained
    // and bounded (stages its own tasks, drains them, leaves the queue empty), so it never perturbs the
    // CAPSTONE that follows — it just adds the `priority+aging PASS` line to this cooperative boot.
    priority_aging_witness(cpu);
    // PRIO-MIX M1: the dedicated priority-mix stress witness (strict ordering + aged rescue), reported
    // alongside the line above. Equally self-contained + bounded (stages, drains, leaves the queue
    // empty), so it too never perturbs the CAPSTONE. (The AARCH64-PRIO landing deferred this one.)
    prio_mix_witness(cpu);
    spawn("capstone", capstone_body, 0, cpu);
    SCHED_GO.store(true, Ordering::Release); // harmless (no APs on this path)
    // Cooperative dispatch loop: drain the run queue, then busy-poll (never WFI). `dispatch_next` returns
    // false only once the queue drains — after CAPSTONE has fully completed — at which point the core just
    // idle-spins (a headless regression captures the log within its timeout).
    loop {
        while dispatch_next(cpu) {}
        core::hint::spin_loop();
    }
}
