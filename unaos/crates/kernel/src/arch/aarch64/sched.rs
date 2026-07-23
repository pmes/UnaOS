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
            last_seq: AtomicU64::new(0),
            last_id: AtomicU64::new(0),
            last_name_ptr: AtomicUsize::new(0),
            last_name_len: AtomicUsize::new(0),
        }
    }

    /// SCHED-5 — fold a measured span into the rolling window (single-writer, owning core). Exactly one
    /// of `busy_cyc` / `idle_cyc` is non-zero per call: `account(delta, 0)` after a task's execution
    /// span (around `switch_context`), `account(0, delta)` after an idle WFI. On window completion
    /// (busy+idle cycles reaching the ~250 ms budget) it snapshots the busy TIME fraction and resets.
    /// Relaxed loads/stores are sound because only this core's scheduler loop ever writes its slot.
    #[inline]
    fn account(&self, busy_cyc: u64, idle_cyc: u64) {
        let busy = self.win_busy_cyc.load(Ordering::Relaxed) + busy_cyc;
        let idle = self.win_idle_cyc.load(Ordering::Relaxed) + idle_cyc;
        let total = busy + idle;
        if total >= load_window_cyc() {
            self.recent_pct.store((busy * 100 / total) as u32, Ordering::Relaxed); // total>=budget>0
            self.win_busy_cyc.store(0, Ordering::Relaxed);
            self.win_idle_cyc.store(0, Ordering::Relaxed);
        } else {
            self.win_busy_cyc.store(busy, Ordering::Relaxed);
            self.win_idle_cyc.store(idle, Ordering::Relaxed);
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

    /// Busy percent (0..=100) for `core_load`: the last completed window, or the partial window if
    /// none has completed yet (0 when the core has never run a pass).
    fn busy_pct(&self) -> u32 {
        let recent = self.recent_pct.load(Ordering::Relaxed);
        if recent != LOAD_PCT_NONE {
            return recent;
        }
        let busy = self.win_busy_cyc.load(Ordering::Relaxed);
        let total = busy + self.win_idle_cyc.load(Ordering::Relaxed);
        if total == 0 {
            0
        } else {
            (busy * 100 / total) as u32
        }
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
    pub busy_pct_recent: u32,
    /// Cumulative context switches into a task on this core since boot.
    pub ctx_switches: u64,
    /// Id of the last task dispatched on this core (0 = none yet).
    pub last_task_id: u64,
    /// Name of the last task dispatched on this core ("-" = none yet).
    pub last_task: &'static str,
}

/// SCHED-2 — read this core's live load: recent busy percent (rolling window), cumulative context
/// switches, and the last task dispatched. Allocation-free and lock-free; callable from ANY core
/// (the shell `top` verb, the witnesses). Introspection only — never consulted on a scheduling path.
pub fn core_load(core: usize) -> CoreLoad {
    if core >= NUM_CPUS {
        return CoreLoad { busy_pct_recent: 0, ctx_switches: 0, last_task_id: 0, last_task: "-" };
    }
    let acct = &ACCT[core];
    let (last_task_id, last_task) = acct.last_task();
    CoreLoad {
        busy_pct_recent: acct.busy_pct(),
        ctx_switches: acct.ctx_switches.load(Ordering::Relaxed),
        last_task_id,
        last_task,
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
    // Signal completion to any joiner. `post()` cross-core-wakes a parked joiner if there is one.
    // BORROW `done_sem` (never move it): the Box's own Arc clone is this `post()`'s liveness anchor
    // and must remain in the Box until the scheduler drops it on the Finished path — strictly AFTER
    // this post (exit() switches away; the scheduler then reclaims and drops the Box).
    unsafe {
        if let Some(sem) = &(*raw).done_sem {
            sem.post();
        }
    }
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
            "mov x8, xzr",  "mov x9, xzr",  "mov x10, xzr", "mov x11, xzr",
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
            "isb",
            "eret",
            in("x0") sp,
            in("x1") entry,
            in("x2") 0x240u64,
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

/// Register `cpu` as an online, scheduling core — a candidate for `CPU_AUTO` load-balanced placement.
/// Called by the BSP as it releases the APs (`start_aps`); idempotent, introspection-only bookkeeping
/// (no effect on any existing caller-pinned spawn, so boot behavior is byte-identical without CPU_AUTO).
pub fn mark_online(cpu: usize) {
    if cpu < NUM_CPUS {
        ONLINE_MASK[cpu].store(true, Ordering::Release);
    }
}

/// Resolve a requested `cpu` to a concrete core. An explicit index passes through verbatim (the
/// no-migrate pin contract). `CPU_AUTO` selects the least-loaded online core: minimum ready-queue
/// depth first, then the lower rolling-window busy fraction, then a rotating cursor for round-robin
/// spread on ties. Falls back to core 0 only if no core is online yet (early BSP staging).
fn pick_cpu(requested: usize) -> usize {
    if requested != CPU_AUTO {
        return requested;
    }
    let rot = AUTO_ROTATE.fetch_add(1, Ordering::Relaxed);
    let mut best: Option<usize> = None;
    let mut best_depth = usize::MAX;
    let mut best_pct = u32::MAX;
    for i in 0..NUM_CPUS {
        let cpu = (rot + i) % NUM_CPUS; // rotating start => equal-load cores fill round-robin
        if !ONLINE_MASK[cpu].load(Ordering::Acquire) {
            continue;
        }
        let depth = RUN_QUEUES[cpu].lock().len();
        let pct = ACCT[cpu].busy_pct();
        if depth < best_depth || (depth == best_depth && pct < best_pct) {
            best = Some(cpu);
            best_depth = depth;
            best_pct = pct;
        }
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
    });
    RUN_QUEUES[cpu].lock().push(task);
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
    let cpu = pick_cpu(requested_cpu);
    assert!(cpu < NUM_CPUS, "spawn_user: cpu out of range");
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
    });
    RUN_QUEUES[cpu].lock().push(task);
    // PI-SCHED-1 — placement witness for EL0 tasks (the vug/midden GUI-app loads land here — the
    // "all vug load on core 2" sighting). Same `pi`-gating + rationale as `spawn_inner`.
    #[cfg(feature = "pi")]
    serial_println!(
        ":: SCHED: task '{}' -> core {} (policy: {} EL0, no-migrate) ::",
        name,
        cpu,
        if requested_cpu == CPU_AUTO { "load-balanced" } else { "caller-pinned" }
    );
    poke_cpu(cpu);
    id
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

/// Mark a parked/just-woken task READY, push it onto its PINNED CPU's run queue, and poke that CPU.
/// Used by the sleeper drain (same CPU) and, from M4b, `Semaphore::post` (cross-CPU wake). The task
/// always returns to `task.cpu`, so its per-CPU (TPIDR_EL2) view stays correct on resume — tasks do
/// not migrate. Caller runs with IRQ masked.
fn make_ready(task: Box<Task>) {
    let target = task.cpu as usize;
    debug_assert!(target < NUM_CPUS, "make_ready: cpu out of range");
    task.state.store(STATE_READY, Ordering::Release);
    RUN_QUEUES[target].lock().push(task);
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
    // AARCH64-PRIO — age then pick, under ONE run-queue lock acquisition. Count this dispatch pass
    // (the aging clock) and, ~every AGING_INTERVAL passes, run the anti-starvation sweep BEFORE the
    // pop so a long-waiting task cannot be dispatched before it is aged in the same pass. The sweep
    // and pop share the lock; `age` carries surplus credit past `AGE_TICKS`, so a coarse cadence loses
    // nothing. Owning-CPU-only counters (Relaxed). See `AGE_TICKS` for why the clock is passes, not ticks.
    let next = {
        let mut q = RUN_QUEUES[cpu].lock();
        let passes = SCHED[cpu].age_passes.fetch_add(1, Ordering::Relaxed) + 1;
        let elapsed = passes - SCHED[cpu].age_last_sweep.load(Ordering::Relaxed);
        if elapsed >= AGING_INTERVAL {
            q.age(elapsed.min(u32::MAX as u64) as u32);
            SCHED[cpu].age_last_sweep.store(passes, Ordering::Relaxed);
        }
        q.pop_highest() // highest-priority ready task; lock dropped here
    };
    let Some(task) = next else {
        CPU_IDLE[cpu].fetch_add(1, Ordering::Relaxed); // M3b CPU-pulse meter (introspection)
        // SCHED-5: idle TIME is measured at the WFI in `run()` (the span the core actually sleeps), not
        // here — this empty pass itself takes negligible time and returns straight to the idle loop.
        unmask_irq();
        return false;
    };
    CPU_BUSY[cpu].fetch_add(1, Ordering::Relaxed); // M3b CPU-pulse meter (introspection)
    // SCHED-2/SCHED-5: busy dispatch — one context switch into a task; record it + the last-task
    // identity. The task's EXECUTION TIME is folded into the window AFTER `switch_context` returns
    // (below), when the elapsed CNTPCT span is known. Single-writer (this core), relaxed; no lock added.
    ACCT[cpu].ctx_switches.fetch_add(1, Ordering::Relaxed);
    ACCT[cpu].note_last(task.id, task.name);
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
    unsafe {
        switch_context(SCHED[cpu].scheduler_sp.as_ptr(), entry_sp);
    }
    let busy_cyc = now_cyc().wrapping_sub(busy_t0);
    // The switch-back always lands IRQ-masked (yield_now/exit mask first; timer_preempt runs in the
    // auto-masked IRQ handler), so the Box reclaim below can't race a re-entrant preempt on this
    // core. Re-assert the mask explicitly so that safety doesn't rest on an inherited DAIF that a
    // future switch-in path could leave enabled.
    mask_irq();
    ACCT[cpu].account(busy_cyc, 0); // fold this task's execution span into the rolling load window
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
            // READY (yielded or preempted): re-enqueue at its BASE priority level (round-robin within),
            // which also re-zeroes its aging clock — a task only ages while it sits WAITING.
            debug_assert_eq!(park, PARK_NONE, "non-blocked task carried a park action");
            task.state.store(STATE_READY, Ordering::Release);
            RUN_QUEUES[cpu].lock().push(task);
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
            // SCHED-5: this is where an idle core actually spends time — WFI parks it until the next
            // tick/IPI (metal) or a light poll-spin (QEMU, no Group-1 timer). Bracket that span with a
            // CNTPCT read and fold it in as IDLE time, so a core that mostly sleeps reports near 0%.
            let idle_t0 = now_cyc();
            crate::arch::hlt();
            ACCT[cpu].account(0, now_cyc().wrapping_sub(idle_t0));
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
        packed |= (ld.busy_pct_recent.min(255) as u64) << (cpu * 8);
        ctx_now += ld.ctx_switches;
    }
    if packed == LOAD_WITNESS_LAST.swap(packed, Ordering::Relaxed) {
        return; // unchanged since the last window — stay quiet
    }
    let ctx_delta = ctx_now.saturating_sub(LOAD_WITNESS_CTX.swap(ctx_now, Ordering::Relaxed));
    let c = |i: usize| core_load(i).busy_pct_recent;
    serial_println!(
        ":: SCHED: load c0={}% c1={}% c2={}% c3={}% (ctx +{}/win) ::",
        c(0), c(1), c(2), c(3), ctx_delta
    );
}

/// SCHED-2 on-demand per-core load table (the `top` shell verb's body, and a serial witness). Prints
/// one row per core: recent busy percent (rolling window), cumulative context switches, and the last
/// task (id + name). Reads `core_load` per core — introspection only, safe from any core.
pub fn load_table(mut line: impl FnMut(&str)) {
    line("core  busy%  ctx-switches  last-task");
    for cpu in 0..NUM_CPUS {
        let ld = core_load(cpu);
        line(&alloc::format!(
            "{:>3}   {:>4}   {:>11}   {} (tid {})",
            cpu, ld.busy_pct_recent, ld.ctx_switches, ld.last_task, ld.last_task_id
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
    // (core 0) never enters `run` on the Pi, so it stays off the candidate set — a CPU_AUTO task only
    // ever lands on a core that actually drains its run queue. No effect on the caller-pinned spawns
    // below (they name their core), so the workload placement here is byte-identical.
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
