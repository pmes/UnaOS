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

## 4a. aarch64 SMP scheduler and the Orin work-stealing balancer

The aarch64 port now runs a real preemptive multi-core scheduler
(`arch/aarch64/{sched,smp_virt}.rs`). Secondaries brought up by PSCI `CPU_ON`
enter `run()` (via `secondary_run`), which sets `ONLINE[cpu]` and makes the core
a **SCHED-BAL** load-balancing participant: migratable (plain kernel) tasks are
placed on the least-loaded online core at spawn/wake (`place_cpu`) and an idle
core pulls work off a busier one (`try_steal`). Per-core `STEALS`/`CPU_BUSY`
counters back the one-line `sched_bal_witness`.

On the Jetson Orin Nano the boot core does **not** enter `run()` — it drives the
cooperative M4 CAPSTONE from `run_capstone_boot_core`. The `burst` shell verb
(and the `sched_demo` boot trigger) stages `BURST_TASKS` migratable `PRIO_LOW`
tasks across every online core and reports the balance. Two facts about this path
are load-bearing and were the subject of the **SCHED-BURST-FIX** arc:

- The cooperative boot core is an online scheduler participant too, but it marks
  itself `ONLINE` inside `run_burst` (it never runs the `run()` seam that does so
  for secondaries) — otherwise the witness under-counts the online cores by one.
- **JC3 — per-core AP timer tick.** Each GICv3 secondary now arms its OWN
  periodic generic-timer tick (`timer::arm_this_core_ap`) before entering
  `run()`, so it re-polls its run queue / attempts a steal every ~4 ms — a
  self-driven scheduler participant rather than reschedule-SGI-dependent. This
  promotes the JC2-deferred "AP timer PPI stretch": the deferral existed because
  `on_tick` bumps the shared monotonic `TICKS` that feeds `ticks()`/`ms()`, so a
  second ticking core would inflate the wall-clock budget. JC3 contains that with
  a **local-only** tick: `arm_this_core_ap` registers the core in `AP_LOCAL_TICK`,
  and such a core advances only its per-CPU `percpu.ticks` (its scheduler clock,
  which also drives `sleep_ticks`), never the shared `TICKS`. The boot core stays
  the sole global-clock owner. Each AP prints one witness line on its first tick
  (`:: AARCH64 SMP: cN timer PPI live (tick 1) ::`), quiet after. The tick does
  not preempt on this EL2 `run()` path (`SCHED_ACTIVE` stays false; `handle_irq_v3`
  calls only `on_tick`) — it just breaks the idle WFI so the loop re-polls.
- **SGI audit (JC3).** The reschedule poke (`poke_cpu`) targeted `gic::send_sgi`
  with the LINEAR core index, but the GICv3 CPU interface routes SGIs by MPIDR
  **affinity** (`ICC_SGI1R_EL1`). On QEMU `virt` affinity == index so it worked;
  on multi-cluster Tegra234 (Aff0 = 0, cluster in Aff2/Aff1) the raw index is not
  a valid target, so a poke never woke the AP — the boot-11 metal symptom. Fixed:
  `poke_cpu` maps the index to the core's published affinity
  (`smp_virt::sgi_target_for_index`) first. The JC3 tick is the belt-and-braces
  second wake, so placed/stealable work is picked up within a tick even if a poke
  is slow or lost.
- Belt and braces, the cooperative burst driver still **steal-drains** while it
  waits — it pulls any stranded work back to itself and runs it — so the burst
  always drains (no teardown wedge) and the steal is recorded (witness shows
  steals > 0). A lost-progress spin ceiling emits an explicit timeout witness
  rather than hanging silently.

## 5. Status and limitations

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
