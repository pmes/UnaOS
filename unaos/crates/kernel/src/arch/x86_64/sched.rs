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

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

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
}

impl SchedCpu {
    const fn new() -> Self {
        SchedCpu {
            scheduler_rsp: AtomicU64::new(0),
            current: AtomicU64::new(0),
            quantum: AtomicU32::new(0),
            need_resched: AtomicBool::new(false),
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

    // Poke the target so an idle CPU re-checks its queue. Skip a self-poke (we'd just be
    // interrupting ourselves). The IPI is wake-only — it never context-switches.
    let this = percpu::this_cpu().cpu_index as usize;
    if target_cpu != this {
        if let Some(c) = percpu::cpu(target_cpu) {
            let icr_low = 0x0000_4000 | crate::arch::interrupts::IPI_VECTOR as u32; // fixed, assert
            apic::send_ipi(c.apic_id, icr_low);
        }
    }
}

/// Cooperatively give up the CPU. The current task is marked ready and rotated to the back of its
/// run queue (by the scheduler), and another runnable task runs. Returns when this task is later
/// rescheduled. No-op if called outside a scheduled task (e.g. on the BSP main loop).
pub fn yield_now() {
    // Critical section with IF=0: nothing may preempt us between marking ready and switching.
    x86_64::instructions::interrupts::disable();
    let cpu = percpu::this_cpu().cpu_index as usize;
    let raw = SCHED[cpu].current.load(Ordering::Acquire) as *mut Task;
    if raw.is_null() {
        x86_64::instructions::interrupts::enable();
        return;
    }
    unsafe {
        debug_assert_eq!((*raw).cpu as usize, cpu, "task ran on the wrong CPU");
        (*raw).state.store(STATE_READY, Ordering::Release);
        // Switch back to the scheduler; it requeues us and runs the next task. We resume here
        // (IF=0, carried by popfq) when rescheduled.
        switch_context(&raw mut (*raw).ctx_rsp, SCHED[cpu].scheduler_rsp.load(Ordering::Acquire));
    }
    // Re-enable interrupts for the task body (we entered with them on; the fresh frame carried 0).
    x86_64::instructions::interrupts::enable();
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

                // --- The task switched back to us (yield / preempt / exit). IF=0 (carried). ---
                SCHED[cpu].current.store(0, Ordering::Release);
                let task = unsafe { Box::from_raw(raw) };
                match task.state.load(Ordering::Acquire) {
                    STATE_FINISHED => drop(task), // frees the stack
                    _ => {
                        task.state.store(STATE_READY, Ordering::Release);
                        RUN_QUEUES[cpu].lock().push_back(task);
                    }
                }
            }
            None => {
                // Nothing to run: sleep until an interrupt (timer or a `spawn` IPI). The `sti;
                // hlt` pair is atomic — an interrupt latched before the `sti` still fires and
                // returns past the `hlt`, so a wake that arrived in the empty-check window is not
                // lost. On wake we loop back to the top (which `cli`s) and re-check the queue.
                x86_64::instructions::interrupts::enable_and_hlt();
            }
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

/// Turn scheduling on and spawn a small demo workload across the online APs, proving the three
/// mechanisms a scheduler must have: timer preemption, cooperative yield, and task exit/free.
/// Called once on the BSP after `smp::start_aps` (hence after `verify_smp`). No-op with no APs.
pub fn start_demo(online_aps: &[usize]) {
    if online_aps.is_empty() {
        serial_println!("SCHED: no application processors online; scheduler idle.");
        return;
    }

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

    // Two cooperative (yielding) threads on the next AP, if present — proves voluntary yield.
    if let Some(&cpu_coop) = online_aps.get(1) {
        spawn("coop-C", demo_coop, encode(cpu_coop, b'C'), cpu_coop);
        spawn("coop-D", demo_coop, encode(cpu_coop, b'D'), cpu_coop);
    }

    // A short-lived worker on the next AP, if present — proves exit + stack reclamation, after
    // which that AP returns to idle.
    if let Some(&cpu_worker) = online_aps.get(2) {
        spawn("worker-E", demo_worker, encode(cpu_worker, b'E'), cpu_worker);
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

/// Cooperative thread: does a little work, then voluntarily yields each round.
fn demo_coop(arg: usize) {
    let (cpu, tag) = decode(arg);
    for round in 0..6u32 {
        serial_println!("SCHED: [cpu{cpu} coop-{tag}] round {round} (yields)");
        let mut acc: u64 = 0;
        for i in 0..5_000_000u64 {
            acc = acc.wrapping_add(i);
        }
        core::hint::black_box(acc);
        yield_now();
    }
    serial_println!("SCHED: [cpu{cpu} coop-{tag}] done");
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
