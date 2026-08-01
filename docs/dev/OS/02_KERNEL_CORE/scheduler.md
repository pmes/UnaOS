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
runs application work on the APs. (aarch64 SMP-BAL changes this on the Pi: once
the BSP finishes its boot duties it enters the scheduler via `run_bsp` and becomes
a schedulable, steal-eligible core — see §2, *BSP scheduling + work stealing*.)

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
  to preempt on a tick. `wait_and_run()` is the AP idle/dispatch loop, and
  `run_bsp(cpu)` is the BSP's (see SCHED-X86 below).
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

- **Rolling-window busy fraction — TIME, not passes (SCHED-5).** `busy_pct_recent`
  is the fraction of **CPU time** this core spent executing tasks over a rolling
  **~250 ms** window. It measures CNTPCT (free-running system counter) cycles spent
  running tasks — bracketed around `switch_context` in `dispatch_next` — versus
  cycles spent idle (the WFI / poll-spin in `run()`); when their sum reaches the
  window budget (derived from `CNTFRQ_EL0`) the busy time fraction is snapshotted
  and the accumulators reset. This replaced the original **dispatch-pass** counting,
  which measured scheduler *activity*, not time: a task waking once per 4 ms tick and
  running for microseconds read ~50% "load" (one busy pass, one idle pass per tick)
  while consuming almost no CPU, and could not tell a busy-loop from a bare cadence
  (it misled the NET-19 arc off a spurious `c1=50%`). CNTPCT is chosen because it is
  always running even where the Group-1 timer IRQ is withheld (QEMU raspi4b), so the
  accounting advances on every path — cooperative and preemptive, QEMU and metal.
  The counter is read only at the dispatch entry/exit boundary and the idle WFI, so
  there is no per-instruction cost. The `SCHED: load cN=..%` line format is
  unchanged; only the meaning of the percentage moved from pass-fraction to
  time-fraction (idle cores now report near 0%, a continuously-running core ~100%).
- **Idle = wall-minus-busy (SCHED-7).** The idle side is measured as the whole pass
  span (`t_prev`→now in `run()`) minus the busy span `dispatch_next` folded in that
  pass (`PASS_BUSY_CYC`), so **every** non-task cycle counts as idle — the WFI *and*
  the sleeper drain, empty-queue dispatch passes, and any poll-spin where WFI returns
  immediately. The original SCHED-5 code bracketed only the explicit WFI, leaving
  those overheads unaccounted; on the input/render cores — whose per-tick peripheral
  IRQ breaks WFI near-instantly, so only the micro busy spans ever landed in the
  window — that read a steady phantom `c0=100% c1=100%` while the workloads were
  provably idle (render parked in `recv`, usb-pump ~60 cyc / 4 ms). Folding
  wall-minus-busy makes busy% == task-execution / wall-time regardless of whether
  WFI sleeps, so those cores now read their true near-0%.
- **Untracked cores read `--`, not a frozen percent (SCHED-8).** SCHED-7 fixed
  cores *inside* `run()`; a core that *leaves* the scheduler loop is a separate
  hole. The Pi/tegra boot core spawns its services then `hlt_loop`s (it never runs
  `run()` on the fb path); the virt boot core spin-loops after CAPSTONE drains.
  Such a core stops folding spans, so its `recent_pct` freezes at the last window
  it completed — on the Pi that was ~100% (the BSP was busy cooperatively draining
  boot tasks), which then printed a permanent `c0=100%` even though the BSP was
  idle in WFI. The fix stamps `last_acct_cyc` on every `account()` fold and adds
  `CoreLoad::tracked`: a slot untouched for >2 windows (~500 ms), or never touched,
  is **stale** — the core is not being accounted right now. Honest views (the
  `SCHED: load` heartbeat and the `top`/`load_table` verb) render an untracked
  core as `--` rather than its frozen snapshot; the liveness gate still reads the
  raw `busy_pct_recent`/`ctx_switches`, so `load-accounting PASS` is unaffected.
  Self-healing: a core re-entering `run()` refreshes the stamp and reports live
  numbers again. General across boards — the same guard covers the x86 BSP inline
  loop and any parked/never-scheduled core.
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

### BSP scheduling + work stealing (aarch64, SMP-BAL)

SCHED-3 balances at **spawn**, but a task's cost is unknown then and wake bursts
pile work unevenly *after* placement. On metal (P45) cores 1–3 "sort of" worked
but never balanced, and core 0 showed `--` forever because the Pi BSP finished its
boot duties and `hlt_loop`ed **outside** the scheduler. SMP-BAL closes both gaps.

**1. BSP into the scheduler.** `run_bsp(cpu)` is the BSP's entry into the
scheduler loop, called after it finishes its one-time boot duties (service spawn +
deferred USB enumerate) in place of the historical `hlt_loop()`. It mirrors the
APs' `wait_and_run` → `run` (minus the `SCHED_GO` wait — the BSP is the core that
*set* `SCHED_GO`), and it `mark_online`s core 0 so placement and stealing may use
it. The "BSP never schedules" invariants were audited and hold: MMU/percpu/vectors
are up long before; GIC distributor config is BSP-only and already done (PL011 RX
is routed to the input core, not core 0); the periodic timer IRQ still runs its
ms-clock tick *before* `timer_preempt`, so the global clock keeps advancing whether
or not a task is preempted on core 0; and only steal-eligible **kernel** tasks can
land on core 0, so no EL0/ASID-0 window assumption is disturbed. Once core 0 folds
a load span every pass, `tracked()` flips true and the `SCHED: load` line / `top`
show core 0's **real** utilization — the P45 `c0 = --` artifact heals with no meter
change (it was already honest; the BSP simply now produces live data).

> The one call-site that swaps `hlt_loop()` → `run_bsp(0)` lives in `main.rs`
> (`kernel_main`, the Pi GUI+AP path), which is outside the scheduler lane. SMP-BAL
> lands the `run_bsp` entry + the invariant audit; the one-line wire-up is a
> deferred `main.rs` diff (see the arc's landing report). Until it lands, core 0
> stays honestly `--`; stealing among the APs is unaffected.

**2. Work stealing.** The `Task.steal_ok` flag is the no-migrate control: a
`CPU_AUTO` kernel task is steal-eligible; a task pinned to an explicit core
(render/input/pump/backstop/capstone) and **every** EL0/slot task (which carry
per-core TTBR0/ASID state) are pinned and never move. Whenever a core finds its own
run queue empty in `run`, `try_steal` runs before it parks:

1. **Victim select** (lock-free length peek): the online core ≠ self with the
   deepest run queue, provided its depth ≥ `STEAL_MIN_DEPTH = 2` (the floor leaves
   the last task at home, so two idle cores never ping-pong a lone task).
2. **Steal** under the victim's run-queue lock *only*: re-check depth (it may have
   drained), then `steal_one` — remove the first steal-eligible ready task,
   scanning **low → high** priority (take background work first, never rob a core
   of its most-urgent task). Pinned tasks are skipped.
3. **Re-home**: retarget `task.cpu` to self (preserving the no-migrate invariant
   for its new home) and push onto the local queue; the loop then dispatches it.

**Race-freedom.** Every task in a run queue is `STATE_READY` (running → out in
`current`, blocked → in a wait/sleeper list), so a stolen task is always safe to
move; both queues are touched only under their own lock with IRQ masked; and **at
most one** run-queue lock is held at a time (victim lock released before the local
push), so there is no lock-ordering hazard. Only an idle core steals, at most one
task per empty pass, so it self-limits and never competes with useful local work.
Capstone/render/input tasks are pinned, so stealing cannot perturb them — CAPSTONE
6/6 stays green with stealing active.

**Witnesses.** A rate-limited (`STEAL_LOG_MAX = 12`) `:: [smpbal] steal '<task>'
c<from>-><to> ::` line per steal, and the spread test `smpbal_steal_witness`
(last in `start_aps`): it piles `SMPBAL_N = 4` steal-eligible tasks on one core
while the others idle in `run`, then asserts they ran on ≥ 2 distinct cores —
`:: SMPBAL: spread test — tasks=4 cores-used=2 :: PASS ::`. A single-core spawn can
only reach two cores if idle cores stole the backlog, so this directly proves the
steal path (not just placement). Both are `all(feature = "pi", feature =
"witness")` gated — armed for `kernel8-test`, byte-identical for the default boot
and the jetson/virt builds. The `SCHED: task ... -> core N (policy: ...)` line is
the spawn-placement decision witness (already present from SCHED-3).

### The BSP joins the scheduler + the GUI handoff (x86_64, SCHED-X86)

x86 had the whole toolkit — run queues, preemption, `Channel`, `spawn` — and used
none of it for the interactive OS. `kernel_main` ended in an inline GUI/shell loop
on the BSP, so core 0 **never popped a run queue**. Two consequences, both on the
metal capture:

- `bg`/`run` place a ring-3 task on the CALLER's core (`bg_place_cpu()` =
  `meter_current_cpu()`), and the caller was the BSP. Result: 2 BGRUN spawns,
  **zero** `wc-x86: SYS_WIN_CREATE`, **zero** `:: SYSCALL:` witnesses, and both
  kills burning the full `KILL_CONFIRM_MS` — programs spawned, never dispatched,
  never reaped.
- The pulse meter read `c0:0/0` while every other core read `0/263`.

SCHED-X86 mirrors the Pi's structure exactly: the loop becomes three scheduled
kernel services and the BSP calls `sched::run_bsp(0)`.

| Task | Core | Owns |
| --- | --- | --- |
| `x86_usb_pump` | `online_aps().last()` | The xHCI service family, `fat::probe_once`, the boot-ledger / flight-recorder pumps, the witness probe ladder, `e1000::service_net`. Touches no pixel. `sleep_ticks(1)` ≈ 1 ms — the same floor the old `hlt()`-paced loop had. |
| `x86_input_service` | `online_aps().last()` | Drains `pal::EVENT_QUEUE` and forwards every event over `GUI_CHANNEL_X86`. Paints nothing, routes nothing. Also emits the 250 ms `Event::Timer` pulse. |
| `x86_render_service` | `online_aps().first()` | Every pixel: builds its own `Screen`/`TargetPal`/`Console`, runs `wc_click_route` → `el0_input_route` → `handle_key`, the cursor sprite, CURSOR-HIDE, `pal.render()`. The shell runs here. |

**`run_bsp(cpu)`** takes the caller's index for parity with the aarch64 twin and
asserts it against `percpu::this_cpu()`; the body is `run()` (x86's `run` derives
the index itself). It has no `mark_online` because x86 has **no work-stealing and
no `CPU_AUTO`** — `make_ready` enqueues on `task.cpu` unconditionally, so nothing
can migrate onto core 0 and core 0 runs only what is pinned there. The invariant
audit, with the x86 evidence, is in the function's doc block; the load-bearing one
is the ms-clock: `timer_interrupt_handler` banks `percpu::note_tick()` and the
cpu-0-only `APIC_TICKS.fetch_add`, and issues the EOI, **before** calling
`timer_preempt()`, so the global clock advances whether or not core 0 is preempted.

**Two placement rules are load-bearing.**

1. **The pump and the render task must be on different cores.** `XHCI_CONTROLLER`
   is a raw `spin::Mutex`, not the sleeping one, and both take it (the render side
   through `fat` block reads, `pal::pump_and_poll` inside a full-screen app, and
   `usbinfo`). Kernel tasks are preempted like any other, so two preemptible takers
   of a raw spinlock on one core hard-deadlock it: the spinner cannot yield, so the
   holder it displaced is never redispatched. The handoff therefore **requires two
   distinct dispatching APs** and declines (staying on the inline loop, with a
   witness) otherwise. No future task that touches xHCI may join the render core.
2. **Never place a COOPERATIVE ring-3 task on core 0.** `spawn_user`'s
   `INITIAL_RFLAGS` carries IF=0, and core 0 is the sole advancer of the global
   ms-clock, so such a task would freeze `arch::ms()` for its lifetime.
   `spawn_user_preemptible` (IF=1 — what `run`/`bg` use) is unrestricted.

**PAT / write-combining — the one x86-specific step the Pi gets for free.** The
framebuffer is WC via PAT slot 4, and `ensure_pat_wc` is per-core MSR state that
only the BSP used to program. Pinning render to an AP would have made that AP's fb
PTE select PA4=WB, which under the firmware's UC var-range MTRR is **effective-UC**
— every blit uncombined, on a panel whose flush is already most of a frame.
`smp::ap_entry` now calls `arch::memory::ensure_pat_wc()` immediately after
`apic::init()` and before the `AP_ONLINE` handshake, on **every** AP — so the
placement decision is not load-bearing and a later re-pin cannot regress the panel.
The leaf retype is not per-core (APs run on the BSP's CR3), and PAT=WC wins over
any MTRR type, so the MSR was the only missing piece.

**Witnesses** (this is how metal proves it, `awk '/SCHED-X86/'`):

```
:: SCHED-X86: RENDER on core 1 + INPUT/usb-pump on core 7 (7 AP(s) dispatching) — OS on its own scheduler ::
:: SCHED-X86 PLACE: aps=7 render=c1 svc=c7 worker=[c2,c3,c4] xhci=[c2,c3] tier=exclusive pool=5 ::
:: SCHED-X86: BSP entered run loop cpu=0 ::
:: SCHED-X86: usb-pump task dispatched on core 7 ::
:: SCHED-X86: input task dispatched on core 7 ::
:: SCHED-X86: render task dispatched on core 1 — panel owned by the scheduler ::
:: SCHED-X86 PLACE-CHECK: actual=c1 arg=c1 published=c1 pool=5 collide=0 tier=exclusive verdict=PASS ::
[schedx86] depth sent=… recv=… inflight=… (render core 1)
```

The spawn line and the three *dispatched* lines are deliberately separate: spawned
is not dispatched (the WINX-2/WINX-3 lesson), and a spawn line with no dispatch
line names a run queue nobody pops. `inflight` is the live `GUI_CHANNEL_X86`
occupancy — the number that separates "render is keeping up" from "render is wedged
and the input task is one burst from blocking in `send`".

### Core placement (WITCORE) — the two extra lines

SCHED-X86 reserved two APs by name but left every *other* placement site in the tree
saying `online_aps().first()`, which since that arc has meant **the render core**.
`arch::x86_64::smp` now owns placement — `worker_cpu(n)` (never render) and
`xhci_worker_cpu(n)` (never render or service, because both hold the raw
`XHCI_CONTROLLER` spinlock and two preemptible holders on one core deadlock it) —
and the two lines above are how a bench operator checks it.

`PLACE` is the **map**, printed at the handoff before any task is spawned. It
carries no verdict on purpose: a check made at publish time against the publisher's
own arguments is a tautology. `worker=[…]` are the cores the ring-3 fixture ladder
will take; `xhci[0]` is `storage-svc` and `xhci[1]` is `bx-blockreq`. A `-` means
that consumer got **no core and will skip**.

`PLACE-CHECK` is the **verdict**, printed by the render task itself once it is
running, comparing the core the hardware reports (`percpu::this_cpu().cpu_index`),
the core the spawn site asked for, and the split read back out of `smp::SPLIT`:

| verdict | meaning | operator action |
| --- | --- | --- |
| `PASS` | all three agree and every consumer class got its cores | none |
| `PARTIAL` | rule intact, **coverage lost** — `pool < 3` (U7x/SOCK-4/U6gx skip, and U6gx is the only automated exercise of the STOR-1 S5 mitigation) or an `xhci=` slot is `-` (storage service and/or `bx-blockreq` skip) | raise core count; on QEMU `UNAOS_SMP=8` |
| `FAIL` | the renderer is not on the core that was published/requested — a mis-set GS base, a spawn enqueued on the wrong index, a run loop popping another core's queue, or a torn publish | stop; this is the defect the arc exists to prevent |

`awk '/SCHED-X86 PLACE/'` gets both lines; `awk '/verdict=FAIL/'` is the alarm.
Note the QEMU default `-smp 6` (5 APs) meets the 3-core pool requirement with **zero
slack** — one AP failing INIT-SIPI-SIPI yields `PARTIAL`, not `FAIL`.

Sites that DECLINE print the measured pool rather than a description of it, e.g.
`:: U6gx: placement pool too small (aps=4 pool=2, need 3) — owner/grants demo skipped ::`.

One placement site is deliberately **not** under this owner: `syscall::bg_place_cpu`,
which starts `bg`/`run` programs on the caller's core — since SCHED-X86, the render
core. That is not a rule-1 deadlock (the storage syscall handler is IF-masked and
cannot be preempted holding `XHCI_CONTROLLER`); it is an operator-facing placement
question — a foreground `run` degrades the panel for its duration — and it is open.

**Expected at the bench, not fixed here:** the shell now runs inside
`x86_render_service`, so `bg_place_cpu()`'s premise ("the caller's core is
definitionally scheduling") finally holds and programs start — but they start on
the **render** core, so a foreground `run` degrades the panel for its duration.
That is a syscall-layer placement question. `sibling_online_cpu` also changes
answer, by design: core 0 publishes `scheduler_rsp` from this loop, so WINX-7
sibling placement may now pick it.

The `rast` knob keeps the inline loop (its demo drives the BSP's local `screen`).

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
- `unaos/crates/kernel/src/main.rs` — `x86_usb_pump` / `x86_input_service` /
  `x86_render_service` and the SCHED-X86 handoff block in `kernel_main`.
- `unaos/crates/kernel/src/arch/x86_64/memory.rs` — `ensure_pat_wc`, and why every
  core that can blit must program PA4=WC.
- [`network_stack.md`](../06_NETWORK_STACK/network_stack.md) and the USB/video docs — the other subsystems the BSP services while the scheduler runs the APs.
