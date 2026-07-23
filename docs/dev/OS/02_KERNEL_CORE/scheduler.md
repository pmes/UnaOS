# SMP and the Kernel Scheduler

The kernel runs a symmetric-multiprocessing (SMP), per-CPU, preemptive,
fixed-priority scheduler with a set of composable blocking primitives. It is
implemented for **x86_64** in `unaos/crates/kernel/src/arch/x86_64/`
(`sched.rs`, `smp.rs`, `percpu.rs`, `acpi.rs`). aarch64 currently runs a single
polled core with no scheduler.

> **Branch note.** This subsystem lives on the USB/scheduler track and the
> combined integration branch (`c01-int_combined`). The boot evidence quoted
> below is from the combined headless boot.

---

## 1. SMP bring-up

The boot processor (BSP) brings up the application processors (APs) before any
device interrupts are enabled:

1. **CPU discovery — ACPI MADT** (`acpi.rs`). The bootloader passes the ACPI
   RSDP; `acpi::init(rsdp_addr)` parses the MADT to enumerate local APIC IDs.
   Degrades gracefully to uniprocessor if ACPI is absent.
2. **Local APIC — x2APIC with xAPIC fallback** (`arch/x86_64/apic.rs`). The
   kernel software-enables x2APIC when the CPU advertises it (the QEMU model is
   launched with `+x2apic`; `UNAOS_CPU=qemu64` forces the xAPIC fallback path for
   testing). LINT0 is masked (no legacy PIC ExtINT); LINT1 is wired as NMI.
3. **AP startup — INIT-SIPI-SIPI** (`smp.rs`). `smp::start_aps()` wakes each AP.
   Each AP brings up its own **per-CPU GDT/TSS** and local APIC, then parks until
   the scheduler turns on. `smp::online_aps()` returns the IDs that came online.
4. **Per-CPU data** (`percpu.rs`). Each CPU has its own GS-based per-CPU block
   (current task, run queues, etc.). Memory is identity-mapped (offset 0), so all
   CPUs share CR3.

The **BSP is never scheduled** — it remains the hardware-service core (it drives
xHCI/storage, the NIC, and the console in `kernel_main`'s loop). The scheduler
runs application work on the APs.

Boot evidence: `APIC: x2APIC software-enabled (id=0…3)`, `SMP: AP 1/2/3 online`.

---

## 2. The scheduler

`sched.rs` implements per-CPU preemptive scheduling.

- **Run queues.** Each CPU has multilevel run queues — `NUM_PRIORITIES = 4`:
  `PRIO_LOW`, `PRIO_NORMAL`, `PRIO_HIGH`, `PRIO_RT`. The scheduler always
  dispatches the highest-priority ready task; equal priority round-robins.
- **Tasks.** A `Task` holds the saved context, stack, priority, and state. The
  context switch is a hand-written routine that saves/restores callee-saved
  registers and the flags.
- **Spawning.** `spawn(name, entry, arg, target_cpu, priority)` creates a
  fire-and-forget task pinned to a CPU. `spawn_joinable(...) -> JoinHandle`
  additionally returns a handle (see §3).
- **Cooperative + preemptive.** `yield_now()` cooperatively reschedules;
  `exit()` terminates the current task; the APIC timer calls `timer_preempt()`
  to preempt on a tick. `wait_and_run()` is the AP idle/dispatch loop.
- **Sleeping.** `sleep_ticks(n)` blocks the current task on a timer-driven,
  per-CPU sleeper list.
- **Priority aging (anti-starvation).** Ready tasks accrue wait-credit; a
  periodic sweep promotes long-waiting tasks one level so a continuously-ready
  high-priority task cannot starve lower ones forever. A promoted task runs at
  its base priority once dispatched.

### Per-core load accounting (aarch64, SCHED-2)

The aarch64 `sched.rs` maintains a live per-core load view alongside the older
cumulative `CPU_BUSY`/`CPU_IDLE` pulse counters (which feed the demo's CPU-pulse
meter and converge to a since-boot average). Accounting is **per-core,
single-writer, lock-free, `Relaxed`** — updated from the tick/switch path that is
already running, adding no new lock to the scheduler hot path and no cross-core
contention. Each `dispatch_next` pass folds one sample into the core's slot:
a **busy** pass (a task was dispatched) or an **idle** pass (empty run queue).

- **Rolling-window busy fraction.** Samples accumulate over `LOAD_WINDOW = 64`
  dispatch passes; on window completion the busy percent is snapshotted and the
  accumulators reset, so `busy_pct_recent` tracks *recent* activity rather than
  the since-boot average. The window is measured in **dispatch passes, not timer
  ticks** — same reasoning as priority aging (`AGE_TICKS`): the cooperative paths
  and QEMU raspi4b have no live periodic tick, and a pass advances on every path.
- **Context-switch count.** A cumulative `ctx_switches` per core (one per busy
  dispatch).
- **Last-scheduled task.** Id + `&'static str` name of the last task dispatched
  on the core, published under a per-core **seqlock** so a cross-core reader never
  reconstructs a torn `(ptr, len)` pair from the fat string pointer.

Read API — `core_load(core) -> CoreLoad { busy_pct_recent, ctx_switches,
last_task_id, last_task }` — is allocation-free and callable from any core;
strictly introspection (never consulted on a scheduling decision).

**Surfaces.** The `top` shell verb prints the per-core table on demand (recent
busy %, ctx-switches, last task). On metal, `timer_preempt` drives a periodic,
change-only serial heartbeat
`:: SCHED: load c0=..% c1=..% c2=..% c3=..% (ctx +N/win) ::` (one emitter per
window via an atomic boundary election; metal-only, since QEMU delivers no timer
IRQ). A non-degeneracy witness (`load_accounting_witness`) runs in the pi
`kernel8-test` battery after the cooperative demo and asserts the accounting is
live — at least one core busy and the context-switch counter advanced — catching
a regression that freezes it: `:: AARCH64 SCHED: load-accounting PASS (...) ::`.

### Load-balanced placement (aarch64, SCHED-3)

The scheduler is **no-migrate**: a task lives for its whole life on the core its
`Task.cpu` names, chosen once at spawn (a woken task always returns to that
core's run queue). Historically *every* caller named that core explicitly, so
placement was 100% caller-pinned. That is correct for tasks that **must** be
single-core (the GUI render loop, the input service), but it also meant several
independent hot services that each pass a hand-picked "off the render core" pin
could pile onto the **same** core. On metal (R23s1f/R23s1h) `net9` (a 4 ms poll)
and `orphan-reaper` both landed on core 2, saturating it (`c2=100%`) while core 1
sat near-idle (`c1=0%`) — the `SCHED: load` heartbeat made the imbalance visible.

SCHED-3 keeps explicit pins **verbatim** and adds an opt-in load-balanced
placement for tasks with no core affinity:

- **`CPU_AUTO` sentinel + `spawn_auto`.** A caller that passes `CPU_AUTO`
  (or uses `spawn_auto(name, entry, arg)`) asks the scheduler to place the task
  on the **least-loaded online core** instead of a fixed pin. Any real core index
  still passes through unchanged, so all existing spawns are byte-identical.
- **`pick_cpu`.** Among the cores registered online (`mark_online`, called by
  `start_aps` as it releases the APs — the BSP never schedules on the Pi, so it is
  never a candidate), pick the minimum **ready-queue depth** first, tie-broken by
  the lower rolling-window busy fraction (SCHED-2), then a rotating cursor so
  equal-load cores fill round-robin. Placement runs on the spawn path only (never
  the switch hot path), so the brief per-queue depth read is free.

The `SCHED: task ... -> core N` placement witness line now reports the policy
(`load-balanced` vs `caller-pinned`). The `SCHED: load` heartbeat format is
unchanged (bench parses it).

**Witness.** `placement_spread_witness` runs at the top of `start_aps` (all
online run queues still empty), spawns `SPREAD_N = 12` `CPU_AUTO` tasks, and
asserts they land on **≥ 3 distinct cores**: `:: AARCH64 SCHED: placement-spread
PASS (12 unpinned tasks -> 3 distinct cores, mask 0b1110) ::`. After the APs are
released, `placement_spread_epilogue` reports the cores those workers actually ran
on as non-gating corroboration. Both are gated behind `all(feature = "pi",
feature = "witness")` — armed for the `kernel8-test` battery, **off** for a
default `kernel8` boot (default-quiet), byte-identical for the jetson/virt builds
that share this module.

> Adopting `CPU_AUTO` at the real service call sites (e.g. `net9` in `genet.rs`,
> `orphan-reaper` in `main.rs`) is the follow-up that spreads those services in
> practice; those files are outside the scheduler lane, so SCHED-3 lands the
> mechanism + witness and leaves the call-site conversion to a net/main arc.

### Orphan-reaper wake on enqueue (aarch64, SCHED-4b)

**SCHED-4 sleep_ticks regression** (U11-reap FAIL, timer never ticks in QEMU) bisected and fixed by SCHED-4b (`d7631117`): semaphore wake on orphan enqueue — ~0% idle duty metal-confirmed (c2=0% P31b), U11-reap PASS restored.

---

## 3. Blocking and synchronization primitives

All primitives live in `sched.rs` and are built to be **lost-wakeup-safe** (the
xv6 lock-handoff discipline: the waker hands ownership to a specific waiter under
the lock, so a wakeup can never be missed) and **cross-CPU** (a post on one core
wakes a task blocked on another).

| Primitive | API | Notes |
| --- | --- | --- |
| `Semaphore` | `new(initial)`, `init()`, `wait() -> bool`, `post()` | Counting; the foundation the others compose from. `wait()` returns a bool so callers can detect an off-task wake. |
| `Mutex<T>` | `new(v)`, `lock() -> MutexGuard` | Sleeping mutex, RAII guard, binary-semaphore-backed. |
| `Condvar` | `new()`, `wait(guard)`, `notify_one()`, `notify_all()` | Mesa semantics; `wait` atomically releases the mutex, blocks, and re-acquires. Reuses the same lock-handoff proof as the semaphore. |
| `Channel<T>` | `new(capacity)`, `send(v)`, `recv() -> T` | Bounded MPMC buffer, composed from a `Mutex` + two `Semaphore`s (no new unsafe). |
| `RwLock<T>` | `new(v)`, `read() -> guard`, `write() -> guard` | Writer-preferring, composed from an inner `Mutex` + two `Condvar`s. |
| `JoinHandle` | `spawn_joinable(...)`, `handle.join()` | Single-shot, `!Clone`; `join()` blocks until the task's trampoline posts a completion semaphore. |

These require a one-time `init()` before first use (they hold internal wait
queues that are set up lazily).

---

## 4. Verification

The scheduler ships a self-test (`start_demo(online_aps)`) that spawns workloads
across the APs and prints `PASS` lines. The toolkit has been verified in **both**
APIC modes (x2APIC and the xAPIC fallback): counting mutex (2×1000 = 2000),
bounded channel (sum 210), condvar `notify_all` (3/3), priority aging
(base-priority task makes progress under continuous higher-priority load), join
(4/4 same-CPU and cross-CPU), and the reader-writer lock.

Combined-boot evidence (one kernel running SMP + USB + net + video):
`RWLOCK: [cpu3] done 5/5, torn=false, max_concurrent_readers=4 => PASS`.

---

## 5. Status and limitations

- **x86_64 only.** aarch64 runs a single polled core; it has no GIC-driven
  preemption or scheduler yet.
- **No priority inheritance** on `Mutex` (assessed as large/thorny under CPU
  pinning; deliberately deferred).
- **APIC timer is uncalibrated** (~1 ms/tick on QEMU); a CPUID 0x15 / TSC-
  deadline calibration is future work.
- **`RwLock` reader starvation is unbounded** (condvar-blocked tasks do not age),
  and each condvar has a documented capacity precondition. These are recorded as
  preconditions rather than fixed.

---

## See also
- `unaos/crates/kernel/src/arch/x86_64/{sched,smp,percpu,acpi}.rs` — the implementation.
- [`network_stack.md`](../06_NETWORK_STACK/network_stack.md) and the USB/video docs — the other subsystems the BSP services while the scheduler runs the APs.
