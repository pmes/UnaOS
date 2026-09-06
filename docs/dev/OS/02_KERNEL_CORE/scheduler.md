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
- **The window is aged at READ time, not only at dispatch boundaries (PULSE-5).**
  SCHED-5/SCHED-7/SCHED-8 all fold at the same two points — after `switch_context`
  returns, and at the bottom of `run()`'s pass — and both are **dispatch
  boundaries**. So the reported number was only ever as fresh as the core's last
  boundary. Best case that is a *previous* window, 250–500 ms old by construction
  (the P69 sighting: "45% unused" printed while vugs were starving). Worst case a
  single compute-bound task holds the core, nothing is folded, nothing rolls, and
  `recent_pct` freezes at its **pre-storm value for the whole span** — total on QEMU
  raspi4b, where no Group-1 timer delivery means no preemption to break the span at
  all; on metal the ~12 ms quantum breaks it, but the value still lags a window and
  the core trips SCHED-8's ~500 ms staleness bound whenever a fold gap exceeds it,
  at which point the honest views drop to `--` and `ui_status::live_permille`
  returns `None` — *the busiest core on the machine reporting "no live number"*.
  That matters because two consumers are **decision** paths, not displays:
  `pick_cpu`'s tie-break and `video::screen::flush_parallel`'s helper ranking +
  headroom weights, both of which a stale-low percent steers *toward* the saturated
  core. PULSE-5 publishes the execution span's start (`CoreAccount::run_t0`, one
  relaxed store before `switch_context`, cleared by the fold) and has `busy_pct()`
  add `now - run_t0` into the window when the value is **read**: a whole window
  covered by one span reads 100% outright; otherwise the measured part is reported
  at full weight and only the *unmeasured remainder* of the window is filled from
  the last completed window's rate, so the historical term's weight decays to zero
  as the window fills. `tracked()` gains the matching arm — a core with a live span
  is being accounted by definition — while `fold_age_cyc` is left untouched, so
  WEDGE-1's much tighter "may I pin work here" gate still reads the raw fold age.
  **Why not account-at-tick** (roll the window from the 250 Hz timer IRQ): the tick
  is metal-only, so it would be dead code in the very gate that has to prove it and
  would leave the total-freeze case unfixed on QEMU; it cannot see the in-flight
  span without moving the busy anchor out of `dispatch_next`'s stack frame into
  shared, interrupt-reentrant state *on the switch path*; and it would pay on every
  tick of every core forever, where age-on-read pays only when someone asks.
  Ordering: the fold clears `run_t0` **before** publishing `win_busy_cyc`
  (`Release`), the reader loads `win_busy_cyc` (`Acquire`) **before** `run_t0`, so a
  span can never be counted twice; the converse skew under-counts by one span for
  one read, which is exactly the pre-PULSE-5 behaviour. Hot-path cost: one relaxed
  store on the context-switch path, one relaxed store in the fold, one
  `Relaxed`→`Release` upgrade on an existing store; a read costs two atomic loads,
  one CNTPCT read and integer math. Nothing is locked and SPREAD-3's residents
  accounting is untouched.
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

PULSE-5 adds one line beside both of those, emitted by `pulse5_witness()`:

```
[pulse5] live c0=0ms c1=0ms c2=0ms c3=0ms span_max=0ms window=250ms folds=0
```

`live cN` is how long that core has been inside its *current* task at this
instant — the term the percents now include and previously omitted entirely;
`span_max` is the worst such span seen (i.e. how stale the old `recent_pct` would
have been, invisibly, for that long); `folds` counts windows still closed by the
dispatch-boundary path, which is unchanged — reads simply no longer wait on it.
It is emitted from the metal heartbeat (inheriting that line's change-only
suppression, so no steady-state chatter) and once from `load_accounting_witness`,
so the QEMU battery — where `timer_preempt` never runs — still exercises and shows
the aged read.

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
  the switch hot path), so the brief per-queue depth read is free. *(SPREAD-3,
  below, puts a committed-EL0-residents count ahead of depth as the primary key;
  the rest of this chain is unchanged and still decides every tie.)*

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

### Band-parallel work distribution (aarch64, SPREAD-2)

`Screen::flush_parallel` (VUG-PAR, `video/screen.rs`, feature `vugpar`) splits a
frame's damaged scanline extent into contiguous bands, runs band 0 inline and
dispatches the rest to helper APs via `sched::spawn_joinable`, then joins. The
join is a correctness barrier, and the band slots stay `steal_ok: false` — a
band must not migrate mid-frame, so **no-migrate is a real constraint and
stays**.

What was not real was the *distribution*. Two attended vugs (P65v2) sat at
`c0=99% c1=68% c2=99% c3=63%`: two cores pinned, two loafing. The helper set is
built as "every tracked core that is not mine", and with `MAX_BANDS == 4` on a
four-core Pi that cap equals the candidate count — so each vug independently
claimed all three of its peers, the cores claimed by *both* vugs saturated, and
each vug's own core stayed comparatively idle. This is BG-SPREAD's finding one
layer down: the placement was **inherited from the topology, not chosen** —
nothing consulted load. But unlike BG-SPREAD the cure is not re-placement:
when the helper set is forced, no reordering can change *which* cores work.

SPREAD-2 therefore fixes distribution in two parts:

- **Sizing (the part that moves the numbers).** Band widths are prefix sums of
  each executing core's *headroom* — `100 - busy_pct_recent`, floored at
  `HEADROOM_FLOOR` (25) — instead of an equal `span * b / nbands` split. A core
  already carrying the other vug's band is handed proportionally fewer rows.
  When all headrooms are equal the prefix sums reduce to `span * b / nbands`
  exactly, so an idle or calm boot keeps its former byte-identical partition.
- **Ranking (the general case).** Candidate helpers are insertion-sorted by
  `busy_pct_recent` ascending before the `MAX_BANDS - 1` cap applies, so a
  fan-out that *is* capped below the candidate count (more cores than bands, as
  on tegra) claims the idlest cores rather than the lowest-numbered ones. On a
  four-core Pi this is a no-op on the set and only decides which helper draws
  which band.

The floor matters: the blit is memory-bound and a saturated core is not a
stalled one, so a 100%-busy core still takes `25/100` of an equal share rather
than an empty band. It also bounds how far one bad reading can concentrate the
split. Feedback is stable because `busy_pct_recent` is a ~250 ms window while
frames land far inside it — the signal low-passes the per-frame response instead
of oscillating with it.

**Witness.** The per-spawn `SCHED: task 'vugband' -> core N` line remains the
trace; the number is a one-per-window rollup, every `SPREAD2_WINDOW` (60)
parallel frames:

```
:: [spread2] window 60 frames cores 4 bands 60,60,38,60 rows 3755,3149,1781,1939 rpb 6258,5248,4686,3231 ratio 193 ::
```

`cores` is how many cores drew bands, `bands`/`rows` are per-core totals for the
window, `rpb` is rows-per-band in hundredths, and `ratio` is max/min **`rpb`** in
hundredths (100 = perfectly even).

The normalization is the point. Raw row totals are not comparable across cores:
core 2 above was `tracked` for only 38 of the 60 frames, so it drew fewer bands
and its row total is low for a reason that has nothing to do with the split —
a raw max/min would have read 210 and blamed the weighting for participation.
Dividing by each core's own band count makes the ratio a statement about
weighting alone.

Normalized, the metric has a **structural bound**: headroom runs from 100 down to
`HEADROOM_FLOOR` (25), so the fattest average band can legitimately be 4x the
thinnest and no more. That makes 5x a real tripwire rather than a guess. A core
that draws bands but accumulates zero rows for an entire window reports a `9999`
sentinel — with `PAR_MIN_ROWS` at 64 and the floor holding the thinnest band well
above zero, that state is pathological, not an edge case, so it trips rather than
reporting a benign 0.

Because `vugpar` is off unless `UNAOS_VUGPAR=1`, the default regression log has
no `[spread2]` line at all, so the spec carries a **FORBID** rather than a
REQUIRE — `ratio` of 500 or more: zero hits on a default log, a real assertion on
a `UNAOS_VUGPAR=1` log. There is deliberately no `cores 1` tripwire; `nh == 0`
exits to the serial path before any rollup, so `nbands` is always >= 2 and such a
line cannot print. BG-SPREAD's `BGSPREAD` leg is unaffected — it is
ASID/parent-placement keyed and reads no band state.

### Committed-load EL0 placement (aarch64, SPREAD-3)

SPREAD-2 fixed how a *single* frame's work is divided. SPREAD-3 fixes the layer
above it: where a *newly spawned* EL0 task is put in the first place.

The measurement (P68, from the serial wire) was that `bg-user` launches placed by
`load-balanced EL0` did not spread at all — **27** landed on core 3, **18** on
core 0, **8** on core 1, and essentially none on core 2, while sustained load
read `c0=99% c3=99%` against `c1/c2 ≈ 80%`. Because the scheduler is no-migrate,
nothing could ever undo it. Operator-visible as **stagger inversion**: vugs
launched early, onto cores that were about to become crowded, ran visibly slower
than their later replacements, which landed on the cores the early ones had left
empty.

Neither of SCHED-3's two signals can see an EL0 vug:

- **Ready-queue depth** (`RunQueue::len`) counts tasks *waiting*. The task a core
  is *executing* lives in `SCHED[cpu].current`, not in `RUN_QUEUES[cpu]` — so a
  core spinning flat-out inside one compute-bound vug reads depth 0, exactly like
  a genuinely idle core. Depth is the wrong shape for long-lived compute.
- **`busy_pct_recent`** is a ~250 ms *lagging* window. That low-pass is a virtue
  for SPREAD-2's per-frame feedback and a defect here: a burst of spawns issued
  inside one window all read the same pre-burst percentage, all agree on the same
  "least busy" core, and all land on it. That is the 27-in-a-row shape.

So placement kept re-reading a signal that had not yet caught up with the
placements it had already made. The fix is to make the decision account for what
is already **committed**:

- **`EL0_RESIDENTS[NUM_CPUS]`** — a per-core count of live EL0 residents,
  incremented in `el0_resident_enter` at the moment of placement (**before** the
  run-queue push, so it is visible to the very next `pick_cpu`, including one
  running concurrently on another core) and released in `el0_resident_leave` on
  reap. Both EL0 spawn paths count (`spawn_user_inner` for slot tasks,
  `spawn_user_thread` for shared-ASID threads); both reap paths release (`exit`
  for every normal/fault/on-CPU-kill retirement, `retire_killed` for the off-CPU
  kill arm). An EL0 task is exactly one with a non-zero `user_entry`, so no new
  `Task` field was needed, and EL0 tasks are `steal_ok: false`, so the `cpu`
  recorded at spawn is still the core being released. The decrement saturates at
  zero — an accounting slip must not underflow to `usize::MAX` and permanently
  exclude a healthy core.
- **`pick_cpu` keys on residents first**, then falls through to SCHED-3's
  unchanged chain: depth, then busy fraction, then the rotating cursor. Explicit
  pins are still honored verbatim, and pinned EL0 spawns are *counted* even though
  their placement is untouched — a pinned vug is real committed load that a later
  `CPU_AUTO` placement must see.

The accounting is O(1), adds no migration machinery, and — unlike depth and
`busy_pct_recent` — cannot lag the decisions it exists to inform. (PULSE-5 has
since removed the worst of `busy_pct_recent`'s lag by aging the in-flight
execution span in at read time, so a core running one compute-bound task no
longer reports its pre-storm percent indefinitely. It is still a rolling window
and still cannot see a spawn that has not started executing, which is exactly why
committed residents remain **key 1** and are not superseded by the fresher
percent.) Where no EL0
task exists (the `virt`/JC3 kernel-thread builds; `placement_spread_witness`,
which runs at the top of `start_aps`) every core reads 0 residents, the new key
is a universal tie, and placement is byte-identical to SCHED-3.

**Witness.** The existing per-spawn line carries the counted value in its
existing `policy:` field, so it stays one parseable line and the
`SCHED: task '…' -> core N` prefix the pi4 spec matches on is unchanged:

```
:: SCHED: task 'bg-user' -> core 2 (policy: load-balanced EL0 residents=1, no-migrate) ::
```

`residents` is **inclusive** of the task just placed — the committed count on
that core after this placement. The next attended boot therefore proves the
accounting directly: successive `bg-user` launches should walk the cores rather
than repeating one, and `residents` should climb evenly across them instead of
running up on a single core.

### Live residents + re-placement at wake (aarch64, SPREAD-4)

SPREAD-3's residents key counts **heads, not load**, and it is only ever consulted
at **spawn**. Both halves matter on a live vug fleet, and together they are the
P73 shape (per-core busy swinging 3–5x while the resident counts sit flat).

- **A parked resident is not load.** A windowed EL0 app spends most of its life
  blocked — VUGPAUSE-2 parks the idle vug on `SYS_INPUT_WAIT` and its workers on
  the phase futex — so a four-vug desktop with one active window reads
  `residents = 4` and every later placement steers around load that is not there.
- **Placement is never revisited.** EL0 tasks are `steal_ok: false` (they carry
  per-core address-space state), so `try_steal` cannot correct them, and
  `make_ready` returned a woken task to `task.cpu` unconditionally. A vug placed
  on c1 while c1 was idle re-ran on c1 forever. This is also the scheduler half of
  the vug speed-up delay: when its peers park, the surviving vug's own worker
  threads stay bunched on the cores they were spawned onto.

SPREAD-4 adds one counter and one decision point.

- **`EL0_PARKED[cpu]`** — how many of a core's committed residents are currently
  blocked. `park_blocked` increments it and `make_ready` decrements it; those are
  the sole park/wake funnels (every EL0-reachable blocking primitive —
  `Semaphore::wait`, `futex_wait`, `sleep_ticks` — marks itself BLOCKED, sets
  `park_kind` and switches back into `dispatch_next`, which lands in
  `park_blocked`), so the pair is exactly balanced. `retire_killed` needs no arm:
  the task it reaps was popped from a run queue, i.e. already un-parked by the
  kill sweep's own `make_ready`. `exit()` runs on a RUNNING task.
- **`el0_active(cpu)` = committed − parked** is what `pick_cpu` now keys on.
  `EL0_RESIDENTS` keeps its exact enter/leave sites and its exact meaning; only
  the *reader* changes. The subtraction saturates, so a transient mid-wake read
  (parked decremented before the resident credit transfers) yields 0 rather than a
  huge number that would exclude the core.
- **`rewake_place(home)`** — on an EL0 wake, `make_ready` re-examines placement and
  moves the task to the lightest online core that is at least `REWAKE_MARGIN` (2)
  runnable residents lighter, breaking ties on the (PULSE-5-aged, therefore
  current) busy fraction. Kernel tasks are untouched and still return to
  `task.cpu` — that is the contract `Condvar::wait` and every caller-pinned spawn
  are written against.

**Why a wake is the sound moment to move an EL0 task.** It is parked: not
`current` anywhere, not in any run queue, holding no live register state beyond
the saved frame on its own kernel stack (a Global identity mapping valid under
every root). `dispatch_next` installs `user_ttbr0` on whichever core dispatches
it, so the address space follows the task. The residual is the old core's TTBR0,
which keeps pointing at the moved task's slot root until that core next dispatches
a user task — benign and staying benign, because the slot L1 tables are a static
array (`boot::SLOT_L1`, never freed to the heap) and `teardown_user_slot`
broadcasts `tlbi aside1is` for the ASID on the last release, so the core holds no
stale translation and no dangling page while running EL1 code that never touches a
low VA.

**Why the margin is 2.** With a margin of 1, two cores at `n` and `n−1` would
trade a task back and forth on every wake, and a windowed vug wakes every frame.
Moving across a gap of two leaves the cores at `n−1` and `n` — a gap of one, below
threshold — so each imbalance is corrected at most once and cannot oscillate.

**Freshness gate.** An EL0 task is non-stealable, so whichever queue it lands on is
the only core that will ever run it. A candidate must have folded a load span
within `dispatch_fresh_cyc()` (WEDGE-1's bound, for WEDGE-1's reason); declining
every candidate simply leaves the task at home, which is the pre-SPREAD-4
behaviour.

**Witness.** A rate-limited per-event trace plus a rollup emitted from the two
`pulse5_witness` sites:

```
:: [spread4] rewake asid=1 tid=109 from=c2 to=c1 act=1 parked=311ms ::
[spread4] live c0=2/5 c1=1/1 c2=0/3 c3=1/1 rewake=10 stay=884 short=52190 margin=2 minpark=100ms
```

`cN=active/committed` is the arc in one field: `0/3` is a core whose three EL0
residents are all parked and which was, until now, repelling placements from a
core with room. A fleet in balance is nearly all `stay`; a burst of `rewake` is a
pile-up being taken apart.

### Park duration gates re-placement (aarch64, SPREAD-5)

SPREAD-4 asked the placement question on **every** EL0 wake. That was right about
*where* a long-parked task belongs and wrong about *how often* the question is
worth asking, because in this fleet parking is not a rare event: VUGPAUSE-2 parks
the idle vug on `SYS_INPUT_WAIT` and VUG-PACE parks each worker on the phase futex
after 64 spin passes, so **every vug task parks and wakes once per frame**. P75
metal measured the cost: `rewake=3256 and climbing` on a six-vug fleet, with
per-window rates diverging 5x (win5 125/s, win1 23/s, win2/3 frozen), a stuttery
mouse, and the paradox that the fifth and sixth vug beat a lone vug's frame rate.
That is migration churn, not imbalance.

SPREAD-5 separates the two shapes of park that share one funnel:

- a **micro-park** — the frame-loop park. The task returns within a frame, to a
  core whose load has not meaningfully moved, with warm caches. Nothing about the
  placement question has changed since it was last asked. Return it to `task.cpu`
  — exactly the pre-SPREAD-4 behaviour, which was correct for this case all along.
- a **real transition** — a window idle for a human interval that is getting work
  again. Its assignment *is* stale, its caches are cold anyway, so the move is
  close to free. SPREAD-4's machinery is unchanged for this path: the margin-2
  threshold and the WEDGE-1 freshness gate both still apply.

**The discriminator is park duration and nothing else** — no notion of focus, no
window identity, no new coupling from the scheduler up into the compositor.
`park_blocked` (the sole park funnel) stamps `Task.park_cyc` from CNTPCT while it
still exclusively owns the Box; `make_ready` (the sole wake funnel) reads it while
it exclusively owns the Box, so the field needs neither atomic nor lock — the Box
handoff through the wait queue / sleeper list is the synchronisation. CNTPCT, not
the per-CPU `ticks`: park and wake are routinely on different cores and the tick
counters advance independently, so a cross-core tick difference would not be a
duration. A `park_cyc` of 0 (a task re-readied without ever having parked, as the
kill sweeps can do) counts as a short stay — a task is never moved on a duration
that was not measured.

**Why 100 ms** (`REWAKE_MIN_PARK_MS`, converted to cycles against the same cached
CNTFRQ the load window uses, so it is one wall-clock span on any board). It sits
in a wide gap between the two populations:

- a 60 fps frame is 16.7 ms and a 30 fps frame is 33 ms, so 100 ms is **three**
  30 fps frames — a frame-clock task cannot reach it even if the fleet drops to
  10 fps under load;
- VUGPAUSE-2's backstop wake is **256 ms**, so a genuinely idle window crosses
  100 ms on its first backstop period and every one after — the long-park path
  stays reachable for exactly the population it serves.

~3x margin on the near side, ~2.5x on the far side; neither population lands near
the boundary. Erring low costs churn back, erring high costs one extra backstop
period before a focus change is corrected.

**Witness.** `rewake` / `stay` keep their meaning but now describe only the wakes
that *asked* — post-SPREAD-5 `rewake` should track real focus changes (single
digits over a session, not thousands). The new `short` counts wakes that skipped
placement; it should dominate `rewake + stay` by orders of magnitude on a windowed
fleet, and that ratio *is* the damping. Micro-parks deliberately do **not**
increment `stay`, which means "asked and declined" and would otherwise be buried.
The per-event trace carries `parked=<n>ms`, so every move that does happen names
the absence that justified it.

### The placement latch and its escapement (aarch64, SPREAD-6 / VUG-PACE-2)

SPREAD-5's damping was right about churn and silently wrong about one population:
a task that never stops. A frame-paced vug parks and wakes every frame, each park
tens of milliseconds, so under SPREAD-5 the placement question was **never asked
again** for as long as the vug kept rendering — its core assignment stayed frozen
at whatever the last long-park wake (or the spawn) decided, under whatever load
existed at that instant. When the surrounding fleet paused or exited, the
survivor kept the contention-era packing forever.

The s1q metal wire is the measurement, and it is the residual half of P73's "vug
wants to go back to what it thinks its fps is supposed to be even though it could
run faster": `[wcn]` win1 pinned at 30.7–30.9/s across ten straight 5 s rollups
while win6 ran 88–93/s from the same binary, `SCHED: load` showing c2 at 99 % with
`[spread4] c2=2/4` (two runnable EL0 tasks time-sharing one core) while c0/c1/c3
sat near idle, and `rewake=` frozen at 26 while `short=` climbed by hundreds per
window — the question was simply never being asked. A frame rate that is a stable
function of stale packing *looks* like a target fps; there is none (VUG-PACE
established that), and the vug's idle tumble is frame-based (3 brads/rendered
frame), so the eye's report that the rotation "returned to its old speed" was a
true report of a real fps reversion, not a perceptual artifact.

SPREAD-6 adds the escapement: a micro-park wake may still ask the placement
question, at most once per `PLACE_REFRESH_MS` (250 ms) per task. `Task.place_cyc`
(stamped at spawn and at every `rewake_place` call, under the same Box-ownership
argument as `park_cyc`) is the clock. Asking is bounded at ~4/s per
continuously-running EL0 task — a six-vug fleet is ~72 lock-free asks/s against
SPREAD-4's measured ~540 placement calls/s — and asking is not moving: the
margin-2 threshold and the WEDGE-1 freshness gate still decide, so a balanced
fleet answers "stay". What changes is only that a pile-up now comes apart within
a quarter second of the load leaving, instead of never. The rollup gains
`refresh=` (asks from this path; outcomes still land in `rewake`/`stay`).

### Wake-latency quantization made countable (aarch64, SPREAD-7 / SMP-FLUID)

P79's storm-6 bench verdict ("it doesn't work fluidly") reduced to one structural
number: a ~35/s per-window ceiling that only rarely broke to ~50/s, with the
fleet-wide attach rate pinned at 123.8/s across dozens of rollups (pi4-r23s1r).
There is no fps target or cap anywhere in the code (VUG-PACE established that,
and the same run shows a lone fixture vug at 194.6/s). The ceiling is **wake
latency quantized by the co-resident's quantum**:

* A vug frame is a three-task rendezvous (parent + two workers on the `PHASE`
  futex). Every rendezvous the VUG-PACE spin window does not catch becomes a
  park followed by a wake through `make_ready`.
* The wake's SGI (`poke_cpu` → `IPI_RESCHED`) only breaks WFI — `gic::handle_irq`
  counts SGIs and returns. On an **idle** target core that is a prompt dispatch;
  on a **busy** one the woken task waits in the run queue until the running task
  reaches a dispatch boundary (yield, block, or quantum expiry — up to
  `QUANTUM_TICKS` × tick = ~12 ms, mean ~6 ms), and `preempt_hint` trims the
  incumbent's quantum only for a *service-band* wake (SCHED-PRIO).
* Under storm 6 three cores sit at 99 % busy, so nearly every uncaught
  rendezvous pays a quantum-scale wait, and a frame with 1–3 of them lands at
  20–48 ms. The wire agrees in quantum-sized steps: per-window rates 21–39/s,
  `[wcn] gap` minima of 15 ms on windows whose tasks own most of a core and
  32–71 ms on the crowded ones, and the rare ~50/s escape being a stretch where
  the spin windows caught every rendezvous and nothing parked at all
  (`parked=0ms` on exactly the fast windows).

The per-window *spread* (win4 at 39/s beside win2 at 9/s from the same binary)
is the same mechanism distributed unevenly: whichever vug's tasks share busy
cores pays more quantized wakes per frame. The SPREAD-4/5/6 machinery does not
take the packing apart because the tasks that would need to ask the placement
question either never park at all (spin-caught rendezvous produce no
`make_ready`, so SPREAD-6's escapement — which only fires *on a wake* — never
runs for them: the fast windows show `[wcn] parked=0ms` for whole rollups, and
`refresh` climbed at ~20–25/s fleet-wide against a designed ~4/s for *each* of
the ~14 committed EL0 tasks), or ask from a 3-runnable core against a
2-runnable one, which margin-2 correctly answers "stay".

**Cross-arch convergence.** x86's `make_ready` has the same three-arm shape:
idle target → IPI-paced; higher-band wake → preempt; equal-or-lower-band wake
onto a busy core → **waits for the tick, by design, on both arches**. aarch64 is
not missing the wake IPI. Changing the equal-band arm — the candidate fix is a
one-line extension of the existing SCHED-PRIO trim to same-band EL0 wakes
(`prio >= cur` ⇒ `quantum.store(1)`, capping the wait at ~one tick ≈ 4 ms;
bounded by the tick rate, so it cannot re-run SPREAD-4's ~540-moves/s churn) —
is a scheduling-policy decision reserved for review, not landed unilaterally.

**What SPREAD-7 lands** is the instrument that decision needs:

* `SPREAD7_QUANT` — EL0 wakes that hit the tick-quantized arm (woken task
  below the service band, equal-or-higher than the target core's running band;
  an idle core reads `PRIO_NONE` and never matches).
* `Task.wake_cyc` — stamped by `make_ready` (EL0 only), consumed by the first
  `dispatch_next` that runs the task: the exact run-queue wait each wake paid.
* `[spread7] quant=N wake2disp n=N mean=Xus max=Yus` — emitted beside
  `[spread4]` from the same sites. The ceiling reads as `quant` climbing at the
  fleet's park rate with `mean` at half-quantum scale (thousands of µs); a
  healthy fleet reads tens of µs. QEMU (no live timer IRQ) exercises the
  counters only, as with every preemption behaviour — the numbers are metal
  evidence.

#### SPREAD-8 — the same-band wake quantum trim (pre-staged; lands on approval)

SPREAD-8 implements the candidate fix SPREAD-7 reserved for review. **This
change lands only on Peter's approval** — it is built and gated on the track
branch so the fold is immediate once the policy is signed off, but it is not
part of the trunk until then.

**The policy.** An equal-band wake is worth at most **one tick** of the
incumbent's time, not a full quantum. `preempt_hint`'s same-band arm — the one
SPREAD-7 counted (`el0 && prio >= cur`, below the service band; an idle core
reads `PRIO_NONE` and never matches) — now applies the SCHED-PRIO trim:
`quantum.swap(1)`, dropping the incumbent's countdown to its final tick.

**The bound.** The trim is tick-bounded: the incumbent always finishes its
current tick (nothing is switched out by force — `timer_preempt` remains the
sole consumer of the countdown, exactly as with the service-band trim), so the
worst case is one extra dispatch boundary per tick per core. That is
structurally incapable of re-running SPREAD-4's ~540-moves/s churn, whose
engine was per-frame *migration*; this trims a countdown in place and moves
nothing. The cross-core race with the owning core's own
`quantum.store(QUANTUM_TICKS)` is benign as before: losing it costs one wake
its trim — the pre-SPREAD-8 behaviour.

**The wire signature.** The `[spread7]` line gains `trim=` — wakes whose swap
actually *lowered* a countdown (previous value > 1), so `trim ≤ quant` and the
gap is wakes that met an incumbent already on its final tick. Expected on a
storm run: `trim` climbing at roughly the fleet's park rate beside `quant`;
`wake2disp mean` dropping from half-quantum scale (~6000 µs) to at most one
tick (~4000 µs worst, less typically); the floor windows' fps roughly
tripling as their 26–48 ms frames lose the quantum-scale queue waits. QEMU
moves the counters (every dispatch stores `QUANTUM_TICKS`, so the swap reads
> 1) but has no live timer IRQ to consume the shortened countdown — the
latency effect, like every preemption behaviour, is metal evidence.

### Service-band IPI-receipt preemption + the dissolved reserve (aarch64, SPREAD-9)

Two coupled changes, from the bench observation that the service band's home
core sat at ~40% busy while three others ran 99% under an 18-task fleet storm:
the services must preempt *now*, and once they can, nothing needs to hold a
core open for them.

**The policy.**

* A **higher-band service wake preempts at IPI receipt**, not at the next
  timer tick. `preempt_hint`'s service arm (`prio >= PRIO_SERVICE`,
  `prio > cur`) keeps its SCHED-PRIO quantum trim (now the fallback) and
  additionally arms a per-CPU pending band, `KICK_BAND[target]`
  (`fetch_max`, so concurrent wakes leave the higher band pending).
  `make_ready` orders push → hint → `poke_cpu`, so the flag is set before the
  wake SGI is sent. On the target, `gic::handle_irq`'s SGI arm runs
  `sched::ipi_preempt` **after EOI** — the exact position and rationale of the
  `timer_preempt` call beside it — which consumes the band (`swap(0)`),
  re-checks it against this core's own `CUR_PRIO`, and dispatches via the
  existing `switch_context`-from-the-IRQ-frame machinery. No new switch path;
  the preempted task resumes through `__vec_irq`'s epilogue as with a timer
  preemption.
* **Equal-band wakes keep SPREAD-8's one-tick trim** (the approved policy).
  Immediate preemption is the service band's alone.
* **The service-core reserve is dissolved.** EL0 placement previously weighed
  two figures that included service load — total ready-queue depth and the
  service-inclusive rolling busy percent — so whichever core currently hosted
  the band (it migrates; the hole followed it) read loaded and repelled the
  fleet. `pick_cpu` now keys on `len_below_band()` (ready tasks below
  `PRIO_SERVICE`) and `el0_busy_pct()` (busy percent minus the service-band
  share, tracked by a parallel `win_svc_cyc`/`recent_svc_pct` fold in
  `CoreAccount::account`; the in-flight span is excluded when `CUR_PRIO` says
  the running task is in the band). `rewake_place`'s tie-break percent gets
  the same substitution. The service tasks themselves keep their band, their
  pins and their placement freedom — only the *fleet's* view of them changes.

**The bound.** One preemption per IPI (the single `swap(0)`), service band
only. The fleet (PRIO_NORMAL) can never arm the kick, so it cannot preempt
itself and no wake-storm churn regression is possible: an EL0 wake takes
exactly the SPREAD-7/8 path it took before. `ipi_preempt` declines when the
core is idle/in the scheduler, when the incumbent is already at/above the
woken band, and on an `IN_RQ_SECTION` breach (WEDGE-4's law — run-queue
sections are IRQ-masked, so the IRQ cannot have landed inside one; the check
is the same tripwire `timer_preempt` carries, made load-bearing). Every
decline leaves the armed quantum trim, so the wake still costs at most one
tick.

**The wire signature.** A `[spread9]` line beside `[spread7]` (same emit
sites, so before/after is one read):
`[spread9] kick=N svc_lat n=N mean=Xus max=Yus` — `kick` is IPI-receipt
preemptions performed; `svc_lat` prices service-band wake-to-dispatch
(stamped in `make_ready`, consumed at first dispatch, split from the EL0
`wake2disp` aggregates so that population is unchanged). Metal expectation
under storm: `kick` climbing with the service wake rate, `svc_lat` mean
< 100 µs (IPI + dispatch pass, down from tick scale), all four cores reaching
99% under fleet load with the mouse fluid. QEMU raspi4b delivers SGIs (so the
gate exercises `ipi_preempt` end to end and the counters/line shape), but the
timer-driven service cadences are absent — the latency collapse is metal
evidence, as with every preemption behaviour here.

### Worker co-placement — a vug's triple lives together (aarch64, SPREAD-10)

FLUID-3 (see `08_VIDEO/engine.md`) closed the present-path hypothesis for the
"predetermined fps": presents run inline (~2.5 ms) on the caller's core, there
is no queue and no single consumer, and the aggregate rate is conserved
(~258/s) while per-vug shares sit wildly unequal and stable (19..80/s, same
binary). The remaining pace-setter is the **futex park at the frame barrier**:
the parent parks on `DONE` behind its two workers (workers on `PHASE`), and
when the three tasks of one vug are scattered across saturated cores, every
frame pays a cross-core rendezvous — wake SGI, run-queue wait behind foreign
residents, the parent's own re-dispatch. That park duration, not any fps
target, is the settled rate, and it is the genuine idle the load meter shows.
SPREAD-10 is the scheduler-side fix FLUID-3 named as in reach: **bias the
members of one address-space slot toward the same core**, so the rendezvous
resolves against local wakes (the spin-then-park windows catch them; the
cross-core IPI + foreign-queue wait drops out of the frame loop). No
present-path or barrier-contract change.

**Identification.** Free: a triple is exactly the tasks sharing one
address-space slot — `user_ttbr0 >> 48`, the ASID `spawn_user_thread`
propagates from the parent and the key the `PHASE` futex hashes under. No new
task metadata. What placement lacked was a per-core view: `SLOT_CORE_RES`
(per core, per slot, committed residents), maintained at exactly the
SPREAD-3 `EL0_RESIDENTS` enter/leave sites — both EL0 spawn paths, the
`make_ready` move (transfer home → target, same order as the resident
transfer), every reap path. Committed, not runnable-adjusted, deliberately: a
parked parent still names the core its workers should rendezvous on.

**The bonus and its weight.** One cross-core rendezvous costs ~1–3 ms/frame
(`[fluid3]` millisecond park buckets under storm); one run-queue position
costs ~4–6 ms saturated (`[spread7]` wd_mean). The co-residency bonus must
therefore be worth **less than one runnable resident** (or triples pile onto
saturated cores and regress SPREAD-4's margin discipline) and **more than the
depth/pct tie-breaks** (or nothing converges). It is **half a runnable
resident** (~2–3 ms-equivalent — the price of the rendezvous it buys back),
applied in doubled-load units (`2*act + 1 − bonus`):

* **Spawn** (`pick_cpu_slot`): a core hosting a same-slot sibling wins every
  runnable-resident tie, ahead of depth/pct; it can never beat a core with
  one fewer runnable resident. Pure preference, no pile-up.
* **Rewake** (`rewake_place`): a second qualifying lane beside SPREAD-4's
  margin-2 lane — a candidate hosting **strictly more** same-slot siblings
  than home qualifies at margin 0 (equal load allowed, heavier never). This
  is what lets a scattered triple converge on a *balanced* fleet, where the
  margin lane holds every member in place. Sequential sibling-lane moves
  strictly increase the slot's co-residency, so convergence terminates (no
  oscillation by the same argument REWAKE_MARGIN makes); ties in the
  selection fall to more-siblings-first, so all members of a slot rank the
  same target core.
* **Retention**: home hosting a sibling is half a resident harder to leave
  via the margin lane (on the integer lattice this bites as one extra
  resident: pure load breaks up a triple only for a ≥ margin+1 win). Both
  lanes compare runnable load, so a saturating core still sheds — the
  co-residency term is a preference, never an anchor, and the SPREAD-6
  refresh escapement (~4 asks/s/task) re-asks with the new term, so triples
  converge within ~250 ms of a load shift and un-converge when a core
  saturates. Slotless tasks (kernel spawns, the shared window, `slot` 0)
  reduce byte-identically to the SCHED-3/SPREAD-9 key chains.

> **Corrected by SPREAD-13 (below), in scope rather than in weight.** Two claims
> in this section hold only while cores are contended. "No pile-up" is true
> against a core with one fewer *runnable* resident, which is what it says, and
> false against one with fewer *committed* residents — a core whose same-slot
> tasks are all momentarily parked scores `2·0+1−1 = 0`, below a core that owns
> nothing at all. "A preference, never an anchor" is true of the margin lane and
> false of `rewake_place`'s early-out, which returns home without scanning when
> `home_act < 2 && home_sibs > 0` — the reading a co-resident triple taking turns
> presents no matter how saturated its core is. Both follow from the asymmetry
> this section declares on purpose (siblings weighed committed, load weighed
> runnable), and SPREAD-13 fixes them by suspending this whole section when the
> machine has a spare core, not by re-tuning any of it.

**The wire signature.** `[spread10] slots 1c=N 2c=N 3c+=N co_moves=N` beside
`[spread4]` (same emit sites): the cores-per-slot histogram over live slots,
plus cumulative placements the bonus *decided* (sibling-lane rewakes + spawns
whose winner differs from the bonus-free key's). Expected metal: slots
collapsing into `1c`/`2c` under storm; `co_moves` stepping at convergence
edges and flat between (steady climb beside a static histogram would be
thrash, which the strictly-increasing rule excludes); on the same wire,
`[fluid3]` park p90 dropping out of the millisecond buckets, per-vug `[wcn]`
rates converging upward, and the reserve core's idle shrinking into
schedulable slack. QEMU battery baseline: all zeros before EL0 exists, which
proves the wiring — the verdict is the next bench boot's `[fluid3]` read.

### Recruiting genuinely idle cores (aarch64, SPREAD-12)

**The measurement.** One window open, on metal:
`:: SCHED: load c0=0% c1=54% c2=98% c3=0% ::`. Two cores flat idle while a
third saturates. On this board that is frame rate on the floor rather than a
tidiness complaint — V3D has never started a thread, so the desktop is entirely
software-rasterised and CPU scheduling *is* the frame-rate ceiling.

**Which predicate declined the move.** The vug's triple sits 1-on-c1,
2-on-c2. For the task on c2: `home_act` 2 with a sibling beside it, so
`home_eff` = 2·2+1−1 = 4, while idle c3 reads `act` 0, no siblings, `eff` = 1.
SPREAD-4's margin lane wants `1 + 2·REWAKE_MARGIN ≤ 4` — five into four — and
declines. SPREAD-10's sibling lane requires `sibs > home_sibs`, and an empty
core hosts no siblings, so the only lane that could still move something
structurally cannot apply to the only candidate that would help. The task on
c1 fares no better (`home_eff` 3 against `eff` 1). Both answer "stay", forever;
`[spread10] ymoves` stops at 1.

The track baton recorded this as *"`rewake_place` refuses equal-load moves"*.
That framing is wrong in a way that matters: the loads were **not** equal — 0
runnable residents against 2 is the widest win available on the machine. The
actual defect is that `REWAKE_MARGIN`, a hysteresis threshold calibrated for the
gap between `n` and `n−1`, was being charged unchanged against a gap between `n`
and **zero**. Spawn placement never had this bug: `pick_cpu_slot` compares
`eff < best_eff` with no margin at all, so a *new* task is sent to exactly the
idle core an *existing* task is forbidden to move to. The two halves of
placement disagreed about the same fleet.

**The fix — a third lane, not a new mechanism.** `rewake_place` gains an
empty-core lane beside the margin and sibling lanes: the candidate has **zero**
runnable EL0 residents and home carries at least `RECRUIT_MIN_HOME` (2). Both
halves are load-bearing.

* **Destination empty, not merely lighter.** This is what makes the margin
  unnecessary rather than merely inconvenient. Oscillation requires the
  destination to become the loaded side and hand the task back; a move onto an
  empty core leaves it at 1 and home at `home_act − 1 ≥ 1`, so a return move
  needs the *destination* to pick up a second runnable resident **and** home to
  fall to zero — a genuine reversal of the load, not the jitter the margin was
  written to damp. The margin keeps its exact behaviour for the population it
  was written about.
* **Home contended (≥ 2 runnable residents).** `home_act` *includes* the moving
  task on both call paths (`make_ready` un-parks it into the count before
  asking; a yielding task never left it), so ≥ 2 means precisely "this task is
  time-slicing against a peer" — there is a real queue wait to buy back.
  Without this half a task already alone on its core would chase whichever core
  reads emptier this microsecond and never settle. It must not be raised to 3:
  at `home_act ≥ 3` the margin lane already admits every `act == 0` candidate,
  so a 3 makes the idle lane a strict no-op and `recruit` reads a plausible 0
  forever. The dangerous re-tuning direction for this knob is *up*.

Because the lane lives in `rewake_place`, it serves the wake path and
SPREAD-11's yield path from one edit, and inherits the existing online check,
freshness gate and tie-breaks unchanged. No new lock, no new spin, no run-queue
section: the function stays lock-free (atomics and a handful of CNTPCT reads),
which is required — `make_ready` calls it inside the IRQ-masked wake path.

**Termination**, and what it rests on — which is *not* a monotone count of empty
cores, because `act == 0` does not mean "empty core". `el0_active` is
`EL0_RESIDENTS − EL0_PARKED`, a deliberate SPREAD-4 choice: it measures runnable
*contention*, not ownership. A core holding two committed residents that are both
parked on the frame futex — the fluid3 barrier shape, the commonest state in this
fleet — reads zero and is recruitable, so a core *rejoins* the zero-reading set
the instant its residents park and a fixed task population can re-offer the lane
indefinitely. Two things bound it instead:

* **Direction, per firing.** Resident credits move before the enqueue, so the
  destination is off zero for as long as its new resident stays runnable, while
  the source held ≥ 2 and keeps ≥ 1. No firing increases the zero-reading count,
  and the lane cannot fire twice onto the same core within one runnable interval.
  Against a *continuously runnable* population — the one the lane exists for, a
  triple time-slicing two cores while two sit at 0% — the set is monotone after
  all and the strong claim holds: at most one firing per initially-empty core,
  then quiet.
* **The clock, per task, for every other population.** Neither call path asks the
  placement question more often than once per `REWAKE_MIN_PARK_MS` of park (wake
  path) or once per `PLACE_REFRESH_MS` (yield/refresh path), and both re-arm
  `place_cyc` on the ask. A task whose core keeps flickering to zero beneath it
  migrates at single-digit moves per second, not once per dispatch: bounded
  churn, not a livelock — but churn, and `recruit` is what exposes it. The remedy
  if metal shows it is to qualify the lane on *committed* residents rather than
  runnable ones — a different reading of "empty", not a restored margin.

Two tasks refreshing in the same instant can both aim at one empty core; the
refresh clock is per-task and unsynchronised so it is unlikely, and it is
self-correcting — resident credits move before the enqueue, so the vacated core
is now the empty one and the next refresh recruits it.

**Relation to SPREAD-10, since it splits a triple.** It is supposed to.
SPREAD-10 fixed the co-residency bonus at *half* a runnable resident and stated
why in the same breath — it "can never beat a core with one fewer resident".
The idle lane fires only where the candidate has at least **two** fewer runnable
residents than home. It is therefore not an override of SPREAD-10's weight but
an enforcement of it: the margin lane had been suppressing the very comparison
SPREAD-10 declared the bonus must lose. On a loaded fleet no core reads zero,
the lane never fires, and co-placement behaves exactly as tuned. The triple
comes apart only on the one-window desktop — the case that is measurably broken.

**The hazard, and the whole safety story.** An idle core and a *wedged* core are
indistinguishable on load: both read `el0_active` 0 and 0% busy, and the wedged
one is the *more* attractive candidate by every tie-break in the function. An
EL0 task is `steal_ok = false` — the core it lands on is the only core that will
ever run it — so recruiting a wedged core parks that task forever, and this lane
would aim the whole fleet at it. `fold_age_cyc` is what tells them apart:
`run()` folds an idle span on **every** dispatch pass (SCHED-7's
wall-minus-busy fold), so a core idling *inside* the scheduler loop is
milliseconds fresh while a core that has left the loop is disqualified within
~30 ms. The lane sits ahead of that gate in the predicate chain so it inherits
it unconditionally.

**The wire signature.** Two fields appended to the existing `[spread10]` line
(deliberately beside `co_moves` — the two arcs pull in opposite directions by
design, and reading them apart would make either look like a bug):
`recruit=N rstale=N`.

* `recruit` — placements only the empty-core lane admitted. Expected: a small
  step, then settling. On a continuously-runnable population the termination
  argument bounds that step at one per initially-empty core; a *slow* climb after
  it is the park-flicker case the same argument admits, to be read against the
  `:: SCHED: load ::` line (still spread = churn that pays for itself; back to
  0%-beside-98% = churn that does not). A climb at dispatch rate would mean the
  placement clocks are not holding, which is a bug in this arc.
  **SPREAD-13 narrowed this field to `spare == 0` lines.** While co-placement is
  suspended, `recruit` is a structural zero — the lane still admits, but the
  margin lane co-admits with it and the exclusive attribution never holds. Read
  `recruit` only against `spare=0`; the derivation is in the SPREAD-13 wire
  signature below.
* `rstale` — empty cores refused by the freshness gate. Zero on a healthy fleet.
  It is not a performance number: a core that looks empty and is not dispatching
  is exactly the wedge SPIN-1…8 is hunting, and this is the placement path
  sighting it from outside. It is *also* the only way a contended task can be
  offered an empty core and still stay.

There is deliberately **no** "offered and declined" field, though the first cut
of this arc shipped one (`rdecl`). It could not fire in the state it claimed to
report: `best_eff` starts at `home_eff` and only decreases when the winner moves
off home, the lane requires `home_act ≥ 2` (so `home_eff ≥ 4`), and any candidate
with `act == 0` computes `eff ≤ 1` — an empty candidate that *reaches* the
comparison always wins it. The only `continue` between the lane test and the
comparison is the freshness gate, so "offered and declined" was identically
"stale-rejected", i.e. `rstale`. Its zero was structural while its documentation
told the operator to read that zero as convergence — the W4-A shape (an
instrument that cannot execute in the state it reports on), which is exactly the
failure this track has already paid for once.

Verdict pending the next bench boot: the target read is `recruit` stepping,
`rstale` at zero, and `:: SCHED: load ::` no longer showing a 0% core beside a
98% one. QEMU battery baseline is all zeros before EL0 exists, which proves the
wiring exactly as `[spread4]`'s zero baseline does.

**Metal verdict (PA3): the lane works and it was not sufficient.** `recruit=81`
on the wire — the lane fires — and the load line still read
`c0=0% c1=99% c2=0% c3=0%` with one window rendering. SPREAD-13 below is why:
the lane's own qualifier (`home_act ≥ RECRUIT_MIN_HOME`) cannot be met by the
state that actually pins the triple, because a co-resident triple taking turns
reads `home_act == 1` however busy its core is. The remedy this section
anticipated — "qualify the lane on *committed* residents rather than runnable
ones" — is the reading SPREAD-13 adopts, applied to the co-placement *policy*
rather than to this lane, whose predicate is untouched. Its **counter** is not:
`recruit` cannot fire while co-placement is suspended, so a re-run of the PA3
measurement now reads `recruit=0` for the same behaviour. See the SPREAD-13 wire
signature.

### Co-placement is conditional on contention (aarch64, SPREAD-13)

**The measurement (PA3, with SPREAD-12 in force).** One window rendering
steadily (`[comp2] rate=231/s`), `:: SCHED: load c0=0% c1=99% c2=0% c3=0% ::`,
`[spread10] co_moves=117 ymoves=185 recruit=81 rstale=4`, and `[fluid3] parks=0`
sustained across 60 s. The run-queue storm is gone, recruitment fires, nothing
parks — and a single vug still cannot use more than one core.

**What is still holding the triple together.** SPREAD-10, and specifically the
asymmetry it declares on purpose: **siblings are weighed committed, load is
weighed runnable.** That is right when cores are contended and is the pinning
mechanism when they are not.

* **Attraction uses committed weight.** A core hosting two same-slot tasks that
  are momentarily parked reads `act == 0` *and* `sibs == 2`, so
  `eff = 2·0 + 1 − 1 = 0` — the lowest value the doubled-load lattice can
  produce, strictly below a genuinely empty core's `2·0 + 1 = 1`. A scattered
  member therefore prefers the core its *parked* siblings live on over a core
  that owns nothing, and the sibling lane (margin 0) admits the move.
* **Repulsion uses runnable weight.** Once co-resident, whichever member is
  asking is usually the only *runnable* one on that core, so `home_act == 1`.
  The early-out (`home_act < 2 && home_sibs > 0`) then returns home without
  scanning at all; and even when it does scan, SPREAD-12's idle lane wants
  `home_act ≥ 2` and the margin lane wants a gap of two. A core at 99% busy
  with three committed EL0 residents is, to every lane in `rewake_place`, an
  uncontended core.

So the triple gathers under committed weight and cannot come apart under
runnable weight. That is not a threshold needing re-tuning; it is a policy whose
**premise** has failed. SPREAD-10's cost model is explicit that what
co-placement buys back is "run-queue wait behind *foreign* residents" on
*saturated* cores — 1–3 ms/frame, priced from `[spread7] wd_mean` under storm.
On a core that owns nothing there is no foreign resident and no queue: the wake
lands on a core sitting in WFI and the same `wd_mean` reads in the *tens* of
microseconds. Co-placement is buying back a cost that is not being charged, and
paying for it with three quarters of the machine.

**The pattern, now three-for-three on this track.** SPREAD-11's placement
predicate declined equal-load moves (right under contention, wrong when empty);
the vug frame barrier's spin budget was denominated in *yields*, so its
wall-clock coverage collapsed as the machine emptied and yields got cheaper
(right under contention, wrong when empty); and now co-placement. Each was tuned
against a saturated four-core board, each is correct there, and each misbehaves
on an idle one. The common error is not the tuning — it is that none of the three
re-asked whether its own premise still held.

**The fix — suspension, not deletion.** Under the six-window desktop the bonus is
a measured win and deleting it regresses the case the desktop actually ships. So
co-placement applies **exactly** as before whenever the machine has no spare
core, and is suspended entirely — bonus, `toward` discount, sibling lane,
retention and early-out together, at spawn (`pick_cpu_slot`) and at rewake
(`rewake_place`) alike — whenever it has one. Under suspension a slot task is
weighed precisely as a slotless one.

**"Spare" is committed-empty *and* dispatch-fresh** (`spare_cores()`), and both
halves are load-bearing. (SPREAD-14, below, added a third half — kernel-cold —
after the bench build line turned `UNAOS_VUGPAR=1` on and made a core saturated
by pinned kernel bands satisfy both of these.)

* **Committed, not runnable.** `el0_active == 0` is SPREAD-12's reading, and that
  section explains why the set it defines is not monotone: a core whose residents
  are all parked rejoins it once per frame. A predicate built on it would flip at
  frame rate and the triple would split and re-pack every few frames — exactly
  the flapping this arc has to bound. `EL0_RESIDENTS` moves only at spawn, at
  reap and at a placement move, so `spare` is a slow variable by construction.
* **Dispatch-fresh (WEDGE-1's gate).** A *wedged* core reads zero committed
  residents forever. Without this half, one wedged core would hold co-placement
  suspended fleet-wide and permanently — a silent machine-wide regression inside
  the very failure SPIN-1…8 is hunting. A core that has left the dispatch loop is
  not spare; it is broken.

**The fourth lane.** Suspension alone does not move anything, because the state
it opens up is invisible to the other three lanes: with the triple co-resident
and taking turns, `home_act == 1`, so the margin lane sees no gap and the idle
lane's `home_act ≥ 2` is unmet. So `rewake_place` gains a lane that fires only
while suspended — home hosts a **committed** same-slot sibling and the candidate
owns **no** committed EL0 resident. Requiring a sibling at home is what keeps
this a co-placement fix rather than a second, looser spreading policy: a task
with no sibling at home is not being held by SPREAD-10 and is already lanes 1
and 3's business.

**What bounds the flapping is a clock, not a structure.** Half of the predicate's
self-limiting story is true and is worth keeping:

* every split moves a task **onto** a core with zero committed residents, and the
  credit moves before the enqueue, so that core leaves the spare set immediately
  and `spare` strictly decreases. Home never becomes spare from its own split —
  the lane requires `home_sibs > 0`, so home keeps a committed resident besides
  the mover;
* no split ever increases `spare` directly.

The other half — that a *return* move therefore requires some **other** task to
have taken ownership of the destination, i.e. a real change in the committed
population — is **false**, and an earlier draft of this section shipped it as a
"strictly stronger termination argument than SPREAD-12's". It is not stronger; it
is the same class. The split's own destination is what drives `spare` to zero,
which re-arms the sibling lane on the next ask, whose move vacates that core
again and restores `spare`. The reachable 2-cycle, on a **two-window** desktop:

| | c0 | c1 | c2 | c3 | `spare` | the ask |
|---|---|---|---|---|---|---|
| **S1** | `a1,b3` | — | `a2,a3` | `b1,b2` | 1 | `a2` from c2: `home_act=1`, `home_sibs=1`, `home_eff=3`. Only the spread lane admits c1 (`eff=1`) ⇒ **`split++`**, `a2` → c1 |
| **S2** | `a1,b3` | `a2` | `a3` | `b1,b2` | 0 | `a2` from c1: `home_sibs=0`, so the early-out lets the scan run; c2 hosts `a3` ⇒ `toward` and `act ≤ home_act` ⇒ sibling lane ⇒ **`co_moves++`, `repack++`**, `a2` → c2 — which is S1 |

The committed population is identical at both ends. The precondition is not
exotic — any core whose committed population is exactly one, beside a
sibling-hosting core reading `act ≤ home_act` — and the six-window desktop
reaches it whenever a window closes and leaves a core singly owned.

**So the honest bound is the placement clock**, the same one SPREAD-12 fell back
on. The cycle's period is one placement *ask* per participating task, and asking
is exactly what SPREAD-6's escapement rate-limits: at most once per
`PLACE_REFRESH_MS` (250 ms) on the refresh path, or once per
`REWAKE_MIN_PARK_MS` (100 ms) on the wake path when a barrier park runs long.
Worst case, 4–10 migrations per second per participating task — below frame rate,
orders of magnitude below dispatch rate. SPREAD-6's latch-and-escapement is
therefore not superseded here; it is the thing doing the bounding. **`split` and
`repack` climbing together at ask rate is an expected reading, not a
falsification** (see the wire signature below).

The remedy, if the metal read says the churn is not paying for itself, is real
hysteresis rather than a threshold: refuse the sibling lane for a task whose last
placement was a spread-lane split within *N* ms — SPREAD-6's escapement applied
to the *lane* instead of to the ask. Deliberately not done in this arc: it adds a
second clock to tune ahead of any measurement saying the first is insufficient,
and `split`/`repack` exist to produce that measurement.

**Cost, stated rather than hidden.** An *idle* window (VUGPAUSE-2 parks its
triple on `SYS_INPUT_WAIT` for seconds) is indistinguishable here from a
barrier-synchronised one, so its triple is also spread and its next wake pays a
cross-core rendezvous it would not have paid. Bounded by the placement clock
above rather than by a move count — the 2-cycle applies to an idle window's
triple exactly as it does to a busy one — and the split fleet is *already* spread
when that window becomes active, which is the good case; the population it costs
is a window whose frame rate nobody is measuring. Accepted knowingly.

**Untouched:** explicit pins (`pick_cpu_slot` returns a non-`CPU_AUTO` request
verbatim before any of this arithmetic runs, so render and input stay
single-core); the `vugpar` band-parallel flush, which picks its own helper cores
from `core_load().tracked` and never consults EL0 placement at all — it was not
being denied cores by placement, and it is off unless `UNAOS_VUGPAR=1` (the
*converse* blindness — EL0 placement never consulting band load — turned out not
to be ignorable once the bench build line shipped that knob on, and is SPREAD-14
below); slotless
tasks, which never compute the predicate and so keep their exact former cost and
behaviour, `virt`/JC3 included. No new lock and no run-queue access: `spare_cores`
is two atomic loads per core over one CNTPCT read, which is required — the wake
path calls it IRQ-masked under WEDGE-4's `rq()` discipline.

**The wire signature.** Three fields appended to `[spread10]`:
`spare=N split=N repack=N` (SPREAD-14 later added `khot=N` between `spare` and
`split`; see its section below).

* `spare` — the predicate itself, sampled now. `spare=0` means co-placement is
  live and every other field on the line carries its pre-SPREAD-13 meaning;
  `spare>0` means it is suspended. It is a gauge, and trustworthy as one
  *because* it is built on committed residents: it moves at spawn/reap/move, not
  at frame rate, so one sample represents the window. Read against
  `1c`/`2c`/`3c+` it is the whole one-window story in one line — three tasks on
  three cores with a fourth spare reads `3c+=1 spare=1`, and the load line beside
  it should show three cores carrying work.
* `split` — triples this arc took apart (spread-lane-only moves, the same
  lane-only attribution `recruit` and `co_moves` use). It counts the
  `home_act == 1` case **only**: under suspension a spread-lane candidate has no
  committed residents, hence none runnable, hence `eff = 1`, and with the
  retention bonus zeroed both `margin_lane` and `idle_lane` reduce to
  `home_act ≥ 2`. So a split at `home_act ≥ 2` is co-admitted, and no lane-only
  counter records it — it lands in `[spread4] rewake` with every other move.
  `home_act == 1` is the state the arc was written for, which is why the field is
  still the one to read; it is not the whole delta.
* `repack` — the flap side: moves that put a slot task back onto a core hosting
  its siblings, from a core hosting none, counted on the raw sibling map with
  **no** lane attribution. So it catches margin-lane repacks under suspension
  (loaded home, lightly loaded sibling-hosting candidate) as well as the usual
  sibling-lane ones — a repack is not by itself evidence of `spare == 0`.

**Reading `split` against `repack`** — and read both against the placement clock:

| reading | meaning |
|---|---|
| `split` steps a few times, `repack` flat | the triple came apart and stayed apart — the good read |
| both climbing at the rate of placement *asks* (single digits/s per task) | the reachable 2-cycle above. **Expected and clock-bounded**, not a defect. Judge it on the `:: SCHED: load ::` line: work still spread = the churn buys the spread it was meant to buy; back to one core at 98% beside three at 0% = it does not, and the next arc is the split hysteresis named above |
| either counter climbing at *dispatch* rate | SPREAD-6's escapement is not holding. This is the real falsification, and it falsifies the bound rather than the lane |
| `repack` climbing, `split` flat | not this arc: SPREAD-10 gathering triples on a genuinely full machine, which is what it is for |

**Reading `split=0`** — check `spare` first. `spare=0` means the lane is
correctly dormant on a contended machine and says nothing about the arc.
`spare>0` with `rstale` climbing means the lane fired and the freshness gate
refused it, which is a wedge sighting rather than a placement result — SPREAD-13's
empty-core rejections are folded into SPREAD-12's `rstale` deliberately, because
the two lanes read "empty" differently (runnable vs committed) but a stale
rejection means the identical thing under both, and splitting the field would
make each lane look quiet for the other's reason. `spare>0` with `rstale=0` and
`3c+` still populated needs the load line before it is called a falsification: it
is one if the work is still piled on a single core, and it is the `home_act ≥ 2`
attribution gap above if the load line shows the work already spread.

**`recruit` becomes a structural zero while `spare>0`, and this arc is what did
it.** Same arithmetic as the `split` bullet, applied to SPREAD-12's counter:
suspension zeroes the retention bonus, so `home_eff = 2·home_act + 1`, a
candidate reading `act == 0` computes `eff = 1`, and `idle_lane` ⟹ `margin_lane`.
`best_idle_lane_only` demands exclusivity, so it can never be set. The lane
itself is untouched and still admits; it is the *attribution* that does not
survive. **`recruit=0` beside `spare>0` therefore says nothing at all about
recruitment** — not that it stopped, not that it is no longer needed. Read
`recruit` only on lines where `spare=0`. This is worth stating loudly because
PA3's `recruit=81` gives a reader a prior: the same bench boot with SPREAD-13 in
force prints `recruit=0`, and that is the suspension, not a regression.

The counter was left alone rather than made subsumption-aware (attributing to the
*narrowest* qualifying lane instead of demanding exclusivity), because that would
silently rewrite SPREAD-12's field for the `spare == 0` regime too — a change to
another arc's counter semantics, made for this arc's convenience.

The QEMU battery prints `spare=0`, and that is a *true* reading rather than a
broken one — worth writing down, because it is the shape this track sends arcs
back for. The battery's single emit comes from `load_accounting_witness`, which
runs once early, before EL0 exists and before the APs have folded a span recently
enough to clear `dispatch_fresh_cyc` (the same instant `[pulse5]` reports
`folds=0`). A core that has never provably dispatched is not spare, by the
definition above, so the field reports the state correctly and the battery proves
wiring only. On metal the emit comes from `load_witness_tick` inside
`timer_preempt`, where every core is going round `run()` folding a span every
pass, so the freshness half never suppresses a genuinely spare core there.

Verdict pending the next bench boot: the target read is `spare>0` with `split`
stepping a few times and settling, `[spread10]` slots moving off `1c` for the
single live window, and `:: SCHED: load ::` showing the vug's work across three
cores instead of one. `repack` flat is the *clean* outcome; `repack` tracking
`split` at ask rate is the documented 2-cycle and is acceptable provided the load
line shows the work spread — it is the *load line*, not `repack`, that decides
whether this arc worked. `recruit=0` is expected on that wire and is not a
finding.

### A core saturated by pinned kernel bands is not spare (aarch64, SPREAD-14)

**The flag (SPREAD-13's reviewer), and why it grew teeth.** Both halves of
SPREAD-13's spare predicate are EL0-shaped instruments, and a core saturated by
*pinned kernel* work is invisible to both. `vugband` tasks — the `vugpar`
band-parallel flush's per-frame workers — are kernel tasks (`user_ttbr0 = 0`),
`PRIO_NORMAL`, `steal_ok = false`, spawned onto up to three helper cores on
every full-screen present. A core fed that stream owns no committed EL0 resident
ever (`EL0_RESIDENTS` moves only on EL0 spawn/reap/move paths), and it is
dispatch-fresh *because of* the very load in question — it goes round `run()`
dispatching a band every frame, and WEDGE-1's own doc says a live core can never
trip the bound. When SPREAD-13 landed this was a latent shape behind a
default-off feature; the bench build line now ships `UNAOS_VUGPAR=1`, so it is
live on the bench.

**The mechanism, concretely.** With a full-screen present banding (the windowless
`occ.is_empty()` path, i.e. exactly the crystal the vugpar bench renders), each
helper core reads:

* `el0_committed == 0` → `spare_cores()` counts it → co-placement **suspended
  fleet-wide**, on the claim that a triple "has somewhere to go that costs nobody
  anything" — while the somewhere is a core P65v2 measured at **99% busy**
  (`c0=99 c1=68 c2=99 c3=63`, the 99s pure band load);
* `el0_active == 0` → the floor `eff = 1` in `rewake_place`'s scan, admitted by
  the spread lane (`el0_committed == 0`) and strictly below any
  `home_eff ≥ 3` — so the 99%-busy band core *wins* the comparison and a triple
  member is moved onto it, where at `PRIO_NORMAL` it time-slices against the
  band stream (and, symmetrically, stretches the flusher's untimed `join`, i.e.
  the frame).

The `pct` tie-break (`el0_busy_pct`) does see band time — bands run below the
service band — but a tie-break only orders equal-`eff` candidates; it never
stops a strict `eff` win.

**The fix — a third half, not a new instrument.** "Spare" gains *kernel-cold*,
read from a signal that already exists and already means exactly the right
thing: `el0_busy_pct` is documented (SPREAD-9) as "what fraction of this core's
time went to work an EL0 arrival would actually wait behind", and on a
committed-**empty** core every point of it is attributable to kernel tasks below
the service band — there is no EL0 resident to produce EL0 time. (The one
imprecision: EL0 time from a resident reaped mid-window lingers ~250 ms while
committed already reads 0 — erring toward "not spare", the safe direction.) A
committed-empty, dispatch-fresh core whose below-band busy is at or above
`SPARE_KBUSY_PCT_MAX` is *kernel-hot*: it is refused by `spare_cores()` and by
the spread lane's candidate test, the two places SPREAD-13's own arithmetic
decides. Nothing else moves: the margin/sibling/idle lanes, the `eff` lattice
and every `spare == 0` behaviour are untouched.

**The bound is derived from the lattice, not tuned.** The spread lane fires at
margin 0 on the claim that the destination is *free*, so "free" must mean the
hidden load is below the smallest quantum the doubled-load lattice
distinguishes — half a runnable resident, the co-residency bonus's own unit. One
time-slicing `PRIO_NORMAL` peer takes half the core, i.e. 50% of below-band
time; half a resident is therefore **25%**. At or above that, the core is
carrying at least the half-resident that separates a genuinely spare core's
`eff = 1` from a contended one, and the margin-0 claim would be dishonest. The
failure direction is asymmetric on purpose: a false "kernel-hot" merely keeps
co-placement live (the pre-SPREAD-13 behaviour, correct on a machine with no
free core); a false "spare" is the P65v2 state.

**Why a windowed percent and not a band counter.** Per-frame band tasks are
transient — spawned, run and reaped inside one flush — so any instantaneous "is
a band here now" flag (a committed-pinned-kernel-task count, say) would flicker
at frame rate: precisely the flapping SPREAD-13 rejected `el0_active` for. The
~250 ms low-passed percent moves on the same timescale as the placement clock.
And the instrument can execute in the state it reports on: the percent is folded
by the reported core's own dispatch loop, and both callers test freshness
*first*, so a wedged core's frozen percent is disqualified as stale before it
could be read as either hot or cold.

**PA3 is not regressed.** The SPREAD-13 measurement had `c0=0% c2=0% c3=0%`
beside the vug's `c1=99%` — presents run inline on the caller's core (FLUID-3),
so the idle cores read genuinely cold, `0 < 25`, and stay spare. Splits proceed
exactly as before on any machine whose free cores are actually free.

**The wire.** One field appended to `[spread10]`, from the same scan that
computes `spare` (one instrument, both fields): `khot=N` — committed-empty,
dispatch-fresh cores refused spare status *only* by the kernel-heat test, i.e.
cores the pre-SPREAD-14 build would have counted spare. Readings:

| reading | meaning |
|---|---|
| `vugpar` build, full-screen present running | `khot` ≈ the band helper count with `spare=0` — co-placement correctly live while the "free" cores blit; the pre-SPREAD-14 build read `spare=3` here |
| no-`vugpar` build | `khot=0` required; nonzero is a sighting of some *other* pinned kernel load saturating an unowned core — worth chasing, not noise |
| `khot>0` beside `spare>0` | mixed state: some cores blit, at least one is genuinely free — suspension and splits proceed, and the `pct` tie-break orders the spare set so the cold core is preferred |

The QEMU battery's `[spread10]` line gains the field reading `khot=0` (no core
is dispatch-fresh at that emit, so neither gauge counts anything), which proves
the wiring exactly as the battery's other zeros do.

### Fleet headroom at the launch boundary (aarch64, STORM-HEADROOM)

`storm [n]` is the shell verb that raises a load fleet — *n* background
`/fat/VUG.ELF` launches through the ordinary `bg` path, nothing else. The track
baton carried "storm 8 headroom probe" as the next reading to take, beyond the
`storm 6` fleet SPREAD-10 was measured against.

**That reading is not available, and the reason is a hard cap rather than a
tuning value.** The verb clamps *n* to 8; `syscall::MAX_PROCS` is **6**. The
seventh launch is refused by the process table, the loop stops, and the fleet
that remains is byte-for-byte the fleet `storm 6` builds — so `storm 7` and
`storm 8` cannot succeed as asked on any boot, empty or not. Six is not
arbitrary: each live row costs one of the eight `USER_SLOTS` EL0
address spaces, and the cap is set two below the pool precisely so a foreground
`run` and the launcher fixtures can still get an address space with a full
background fleet. Raising it makes the `Proc` table and the slot pool exhaust
together, which turns every slot-pressure failure into a table-full one. **Vug
count is therefore not the axis with headroom left on it.** Task count still is
— a vug is a triple (parent plus two ELF-2 workers), so a full fleet is ~18
tasks against four cores, and that is the load the placement arcs are actually
measured against.

The clamp is deliberately left at 8 rather than lowered to 6: an operator asking
for more than the machine has must get a *refusal that names the resource*, not
a silently-lowered request that reads as if it were granted.

### Fleet headroom, x86 (HEADROOM, Boot AL)

**Everything above still describes aarch64 exactly, and no longer describes
x86.** Boot AL took the reading the section above says is unavailable and found
the ceiling was not one cap but three, stacked so that raising any one alone
would have moved nothing:

| constant | was | now (x86) | aarch64 | what it bounds |
| --- | --- | --- | --- | --- |
| `memory::USER_SLOTS` | 8 | **12** | 8 | concurrent ring-3 address spaces |
| `syscall::MAX_PROCS` | 6 | **10** | 6 | live `Proc` rows (`<= USER_SLOTS - 2`) |
| `syscall::NTHREAD` | 8 | **24** | 8 | joinable ring-3 thread handles, machine-wide |
| `syscall::WIN_MAX` | 8 | **12** | 8 | global window ids |
| `wm::MAX_WINDOWS` | 8 | **12** | 12 | compositor rows (arch-neutral) |
| `sched::NFUTEX` | 16 | **64** | 64 | distinct futex keys with waiters parked |
| `shell::BG_JOBS` | 8 | **12** | 12 | shell job rows (arch-neutral) |
| `memory::FB_WIN_SLOTS` | 8 | 8 | 8 | windows ONE process may hold (`-EMFILE`) |

**`NTHREAD` was the cap the operator could see.** With six vugs up, vugs 5 and 6
logged `SYS_THREAD_SPAWN denied a=11 b=11 workers=0 -> inline raster`: eight rows
at two workers per vug seats four vugs, and the two that fell back to inline
raster ran at roughly **4x** the frame rate of the four with workers. A fleet of
identical programs running at two speeds, with nothing in any program to explain
it — the "predetermined fps" read off the panel for weeks was a table size.

**`MAX_PROCS` was the cap the operator hit.** On a `wc` boot the desktop app
(`STAT.ELF`) holds a row permanently and never exits, so six rows meant `storm`
fleets capped at **five**.

**`NFUTEX` was the cap that would have replaced them.** A vug holds *three* live
keys while idle (`DONE`, `PHASE`, its input ring), so a ten-program fleet wants
30 buckets against a pool of 16 — and the overflow is silent, because
`TableFull` degrades every caller to a spin. x86 had simply never taken the
raise aarch64 made at VUGPAUSE-2; without it, the `NTHREAD` cliff would have
been traded for a futex cliff that looks identical from the panel.

**The reserve did not move.** `MAX_PROCS <= USER_SLOTS - 2` now holds at
*equality* (10 ≤ 10): the two-slot margin a foreground `run` and the launcher
fixtures live on is exactly as wide as it was, and none of the new capacity was
bought from it. `FB_WIN_SLOTS` deliberately did **not** follow `WIN_MAX` — the
per-process window cap is unchanged at 8, which is what keeps the raise from
costing 12 slots × 4 extra 64 KiB surface reservations (~3 MiB) for capacity no
shipped program asks for. The `WIN_MAX == FB_WIN_SLOTS` assertion became
`FB_WIN_SLOTS <= WIN_MAX`, and `sys_win_create`'s region-slot search was
re-bounded to `FB_WIN_SLOTS` — left at `WIN_MAX` it would have handed out region
slots 8..11 and walked off the end of the slot's FB region.

**What the machine should now support**, and what the storm census grades it
against: **8 vugs** all with `workers=2`, alongside the resident desktop app,
with one `Proc` row still free for a foreground `run` and `user slots free >= 2`
at the end of the burst. `storm 9` is the largest fleet that completes as asked
on a `wc` boot; the tenth launch is the one the process table refuses.

**The clamp is now derived, not written down.** It admits
`proc_table_rows() + 2` — the same "two past the ceiling, so the refusal names
the resource" margin the section above argues for, maintained as a *margin*
rather than as the literal `8` it happened to equal when `MAX_PROCS` was 6. On
aarch64 that evaluates to 8 and the arithmetic above is unchanged; on x86 it is
12, so `storm 11`/`storm 12` are the requests that cannot succeed on an empty
boot. The `storm` help line is built from the same call, so it can no longer
quote a stale ceiling.

**Cost.** One address-space slot is `USER_STATIC_SIZE` (0x85000 = 544 KiB) of
`.bss` backing plus four 4 KiB page tables; 8 → 12 is **+2.14 MiB** of `.bss`
(4.28 → 6.42 MiB). `.bss` is NOBITS, so the boot image on the ESP does not grow.
Every other table in the list costs kilobytes or less. Against that: task count
is the axis the placement arcs are measured on, and a full 8-vug fleet is now
**24 tasks** (a vug is a triple) plus the desktop app, against four cores —
where the aarch64 section above tops out at ~18.

**The instrument.** What a storm run could not previously answer is "what breaks
first as *n* grows", because every quantity that would name a ceiling rides a
different clock — the `:: SCHED: load ::` train is timer-driven (~1 s windows,
metal-only) and `[fluid3]`/`[comp2]` ride the compositor, while the interval
that matters (the seconds in which the fleet is being built) is shorter than one
of those windows. The launch boundary is the only clock that samples the machine
at the instant its size changes, so the verb now takes a census there. It mints
**no new counter**: every number is read through the accessor that already owns
it. What is new is *when* they are sampled and that they are sampled together.

A run emits, on serial: a `:: STORM: begin ::` resource line (free/running/
exited/porphaned `Proc` rows, free job rows, free user slots), a `pre` census, one
`[storm] k=` line after each successful launch, a `:: STORM: REFUSED at launch
k ::` line if one is refused, and a `post` census. The refusal line **re-reads
the census** rather than referring to the message beside it, because those
messages are not uniform on this wire: a spawn refusal also prints
`:: BGRUN: bg … rejected (…)` to serial, while an image-read failure (missing or
oversized `/fat/VUG.ELF`) is console-only. A serial-only capture must be able to
tell the fleet ceiling from a bad card — `free > 0` at the refusal means the
process table was *not* the limit. The resource line is re-read **unconditionally
after the burst** as `:: STORM: end ::` (not only on refusal): the reserve
question — did two user slots survive a full fleet? — is asked by the *success*
path, and the answer is not derivable from the `begin` line (a vug that faults
mid-burst leaves a `PEXITED` row and breaks the arithmetic; `[spread10]`'s slot
histogram counts residency, not `SLOT_USED`). The census is `[storm]` plus
the standing witness train (`[pulse5]`, `[spread4]` → `[spread7]`/`[spread9]`/
`[spread10]`) re-emitted at that instant, in their existing wording — so a storm
capture and a steady-state capture are read with one vocabulary.

**Depth and saturation are printed as a pair, always**, because neither is
interpretable alone. A core spinning flat-out inside one compute-bound vug holds
that task in `current`, not in its run queue, so it reads depth 0 exactly like a
genuinely idle core. The pair separates three ceilings the load line conflates:

* `busy=99% rq=0/0` — saturated with nothing waiting; the ceiling is that core's
  throughput.
* `busy=99% rq=6/6` — saturated with a queue behind it; the ceiling is
  **placement**, and `[spread10] recruit`/`rstale` on the same block say whether
  placement was offered a way out and refused it.
* `busy=--` — the core is not dispatching at all; nothing about it is a load
  measurement (SCHED-8's untracked rendering).

**Two things are deliberately absent, and each absence is a correctness
property.** `[prio]`'s per-window *deltas* are not taken: `prio_witness` swaps
its snapshots as it prints, so calling it here would silently shorten the next
periodic `[prio]` window — a probe that alters what it measures. The cumulative
totals carry the same information at a boundary sample and cost the periodic
line nothing. `[fluid3]`'s park percentiles are not taken for the larger version
of the same reason: `fluid3_drain` *consumes* the buckets it reports, and they
belong to the compositor's `[comp2]` cadence. Park pressure is still represented,
through the cumulative `[spread4] short/rewake` and `[spread7] wake2disp`.

**What the silence means.** Apart from one `boot-baseline` line, every `[storm]`
line is emitted from the shell task, so the probe runs for exactly as long as the
shell is dispatched. That is the state a headroom probe is about — but a fleet
that starves the shell also silences the probe, and starving the shell is one of
the outcomes it is hunting. **A missing `post` is not a clean run.** Two
properties make the silence readable rather than mute: `pre` is emitted before
the first launch, and one line after *each* successful launch, so the last
`[storm] k=` on the wire names the launch after which the shell stopped
reporting, and a truncated tail is itself the measurement. The instruments that
survive a starved shell are the timer-driven ones — `load_witness_tick` and the
`[spin1]` block inside `pulse5_witness`, which run from the timer IRQ on every
core and depend on no task being schedulable. Read those beside a storm capture,
never instead of it.

The probe costs one `rq()` acquisition per core per sample — IRQ-masked for the
hold, WEDGE-4's law, byte-for-byte the hold `pick_cpu` already takes on every EL0
spawn, and once per launch rather than once per frame.

`load_accounting_witness` takes one `[storm] boot-baseline` line so the QEMU
battery *executes* the probe rather than merely compiling it; without it, code
reached only by an operator typing `storm` would run for the first time on an
attended bench. The gate reads
`busy c0=100% c1=-- c2=-- c3=-- | rq …=0/0 | el0 …=0/0` — the BSP inside the
cooperative demo, the APs not yet in `run()`, no EL0 anywhere — which is the
honest boot baseline and proves the run-queue reads and the pair rendering work.

### Futex duplicate-bucket lost wake (aarch64, FUTEX-DUP / VUG-PACE-2)

The other half of VUG-PACE-2, and the win1 lockup's root cause. `futex_wait`
selected its bucket in two passes — an existence scan for the key, then a claim
scan that tested **only** `key == 0`. Two waiters entering together on a key with
no standing bucket could each complete the existence scan before either had
stored the key, and the claim pass then minted **two buckets for one key**.
`futex_wake` stopped at the first matching bucket, so the second bucket's waiter
slept on a key nothing would ever name again.

The only two-concurrent-waiter key in the system is user-vug's `PHASE` word —
both workers park on it in the same instant, once per frame, thousands of
opportunities per minute under a fleet — which is why the victim was always a
vug. The s1q signature: `wake_phase` woke one worker, the stranded one never
rendered, `DONE` never reached `live`, and the parent parked at the frame
barrier's arrival futex making **no passes** — so `BARRIER_PASS_BUDGET`, which
counts returned passes, could never fire (UVUG-9's documented "parked forever"
limitation, observed in anger). On the wire: `[wcn] win=1 att=0 parked=0ms`,
composited by neighbors only, **no** fault, no `[uvug9]` stall, no `[vugpause2]
resume` on the restoring click (the parent was not parked on the *input* futex,
so the focus/unhide edges found the parked-hint clear), and no click ack (input
never drained again). Nothing recovers it — the input backstop wakes only input
keys — except `kill`, whose `futex_wake_killed` already scans every bucket.

The fix is two-sided. Claim side: the claim pass now joins a bucket another
waiter has keyed to the same key since the existence scan (closes the common
window). Wake side, the correctness backstop: `futex_wake` visits **every**
bucket serving the key, exiting early only once `n` waiters are woken, so a
duplicate that still slips through (a foreign bucket freeing mid-scan) costs
nothing. A wake that finds more than one bucket for its key increments
`FUTEX_DUP` and prints `[futexdup] key=... buckets=... woken=...` (first 8
occurrences) — each line is the race observed **and absorbed**; before the fix
each was a permanent strand. user-vug's barrier protocol was and is
lost-wakeup-safe against a correct futex; nothing in the program changed.

### A cooperative fixture cannot wait for its own launcher (x86, WINX7-GO)

Not a kernel defect — a **fixture** defect, and the reason it is recorded here is
that the shape generalises to every launcher/fixture pair on this arch.

`sched::spawn_user_in_space` builds a **cooperative** ring-3 task
(`preemptible = false`): the timer cannot evict it, and it releases its core only
by blocking. WINX-7 placed its fixture on the **launcher's own core**
(`meter_current_cpu()`), and the launcher then sat in a wait loop that had to
grant the fixture focus and inject an input event *while the fixture polled for
one*. The launcher could only run when the fixture blocked.

The fixture's only block was the frame barrier — `FUTEX_WAIT` on `done`, which
parks only when the parent's first load beats both workers' `lock xadd`. That is
a coin flip. Lose it and the fixture ran window → spawns → barrier → 2,000,000
non-yielding `SYS_INPUT_POLL`s → present → joins → exit without ever releasing
the core; the launcher got **exactly one** loop iteration, taken before the
window even existed, and then found `WINX7_DONE` already set. On the wire that is

```
:: WINX-7: … FAIL — witness=0xdf … parks=0 … injected=0 … done=1 ::
```

roughly **half of all runs** (measured: 11 FAIL / 23 verdict-bearing runs on
clean trunk). `parks=0` and `injected=0` were never two bugs; they are the same
missed interleave seen from both ends, and the verdict could not say so because
nothing reported the launcher's own scheduling. The `[winx7-go]` line does now —
`iters=1` is the starvation, `iters>=2 winseen_at=2 gorel=true` is health.

The repair is a pair of **GO gates** (the U7x/DMG-REFUSE go-word idiom), each a
word in the fixture's data page that only the launcher writes:

- **Gate 1 (`+0x100C`)** holds the *parent* immediately after `SYS_WIN_CREATE`.
  The blob never writes that word, so the `FUTEX_WAIT` is a park *by
  construction* rather than by timing — which hands the launcher the CPU. The
  launcher grants focus, injects, and only then plants `go = 1`, so the key is
  already in the ring when the fixture takes its first poll.
- **Gate 2 (`+0x1010`)** holds *both workers* just before their `lock xadd`. It
  exists because gate 1 alone made the verdict's `parks >= 1` self-satisfying:
  the GO park met it, so the *frame barrier* park — the assertion the test exists
  for — was still incidental, and did not happen on **6 of 28** passing runs
  (21%). With the workers gated, `done` is provably 0 when the parent reads it,
  so the barrier park is deterministic too.

The handshake that says "it is parked" is `sched::futex_waiters_on(key)`, the
keyed twin of `futex_parked_total()`. Keyed matters: the global gauge is only
correct here because of the same-core placement, so a placement change or a
stray parker anywhere else could re-open the flake silently. As *evidence* that
a park happened either gauge is useless (a park between two samples is
invisible; that is what `futex_park_count` is for), but as a *handshake* the
keyed one is exact, because the state is level-triggered — the fixture cannot
leave the gate until the launcher opens it. Gate 2 waits on **three** keys: two
waiters on gate 2 *and* one on `done`, so the parent has committed to the barrier
park before the workers are allowed to arrive.

The verdict gates on `barrier_parks = parks - 3` (the three gate parks are
structural and are named rather than quietly inflating the number), on both
gates' **provenance** being `handshake`, and on `iters >= 2 && winseen_at >= 1`.
That last pair is the disease asserted directly: the eight witness bits can line
up while the launcher was starved, and a failsafe-opened gate — the failsafe
exists so a gate can never wedge — was otherwise indistinguishable on the wire
from the healthy path, which made the insurance the one thing able to hide what
it insured against.

Two lessons for any future fixture on this arch: a cooperative ring-3 task must
`SYS_YIELD` inside any spin that waits on another task (WINX-7's poll loop now
yields every 4096 empty polls, at an unchanged 2,000,000-poll budget), and a
launcher must never assume it will be scheduled concurrently with a fixture it
placed on its own core.

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
| `x86_render_service` | `online_aps().first()` | Every pixel: builds its own `Screen`/`TargetPal`/`Console`, runs `wc_click_route` → `user_input_route` → `handle_key`, the cursor sprite, CURSOR-HIDE, `pal.render()`. The shell runs here. |

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
:: SCHED-X86 PLACE: aps=7 rsvc=c1 svc=c7 worker=[c2,c3,c4] xhci=[c2,c3] tier=exclusive pool=5 sched=all-cores ::
:: SCHED-X86: BSP entered run loop cpu=0 ::
:: SCHED-X86: usb-pump task dispatched on core 7 ::
:: SCHED-X86: input task dispatched on core 7 ::
:: SCHED-X86: render task dispatched on core 1 — panel owned by the scheduler ::
:: SCHED-X86 PLACE-CHECK: actual=c1 arg=c1 published=c1 pool=5 collide=0 tier=exclusive verdict=PASS ::
[schedx86] load-prejoin c0=-- c1=…% …                      (SCHEDLOAD-X86, once, at the handoff)
[schedx86] depth sent=… recv=… inflight=… (render core 1)
[schedx86] load c0=…% c1=…% c2=100%*(name) … sw=[…] q=[…]  (SCHEDLOAD-X86, every ~5 s)
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

One placement site was deliberately **not** under this owner: `syscall::bg_place_cpu`,
which started `bg`/`run` programs on the caller's core — since SCHED-X86, the render
core. That was never a rule-1 deadlock (the storage syscall handler is IF-masked and
cannot be preempted holding `XHCI_CONTROLLER`); it was an operator-facing placement
question — a foreground `run` degrades the panel for its duration — and it was open
until SMPBAL-X86 (below), which makes it `CPU_AUTO`.

`sibling_online_cpu` also changes answer, by design: core 0 publishes
`scheduler_rsp` from this loop, so WINX-7 sibling placement may now pick it.

The `rast` knob keeps the inline loop (its demo drives the BSP's local `screen`).

### Per-core busy-TIME accounting + the always-on load witness (x86_64, SCHEDLOAD-X86)

Until this arc, **no serial line on any x86 boot reported per-core load.** The only
feed was `CPU_BUSY`/`CPU_IDLE`, and reaching it needed an operator: the `sched`/`ps`
shell verb, or `PULSE.ELF` through `SYS_CPUPULSE(49)`. Every unattended capture in
the archive is therefore silent about which cores were working — which is the reason
this is the *first* arc of the SMP balancing campaign rather than a later one. A
balancer landed without a load witness is unfalsifiable.

`arch::x86_64::sched` now carries a per-core `CoreAccount` and a `core_load(cpu) ->
CoreLoad` accessor mirroring the aarch64 contract. `run()` folds a measured **span**
into a rolling ~250 ms window at the two sites that already bumped the event
counters: `now_cycles()` (rdtsc) taken either side of `switch_context` (busy) and
either side of `enable_and_hlt` (idle). `CPU_BUSY`/`CPU_IDLE` are untouched and keep
all five of their consumers — they are a second instrument in a different currency,
and the arc's cross-check is that the two agree about *which* cores are idle while
disagreeing about the magnitudes.

#### The per-core token has exactly three forms

This is the instrument. Reading the line without reading this table gets it wrong.

| form | meaning |
| --- | --- |
| `cN=NN%` | **MEASURED.** Every cycle in the number was folded from a real span over the window. |
| `cN=100%*(name)` | **INFERRED.** No span was folded for this window; the value is deduced from a live `current` plus a fold age past a whole window. `name` is the task holding the core (clipped to 16 bytes, `+` if clipped). |
| `cN=--` | **ABSENT.** No live measurement at all — the core never entered `run()`, or stopped folding with nothing executing. |

`0%` is a measurement (this core folded spans; none were busy); `--` is the *absence*
of one. Collapsing them would make an unaccounted core indistinguishable from a
provably idle one, which is how an instrument ends up certifying the imbalance it was
built to detect. `100%*` is that same argument one level up, and it matters more,
because the inferred value is the extreme of the scale: without the marker an
*inference* prints byte-for-byte as a *measurement*. The adversarial review caught the
collapse in the first revision, and the QEMU trace shows exactly why it matters —
`c2=100%*(zeolite-resolver)` (inferred, `sw[2]=20`) beside `c3=100%` (measured,
`sw[3]=34,481,377`) on the same line. It also decides whether the PULSE-A cross-check
can be scored at all: a core agreeing at 100 % via the pegged arm agrees for a reason
near-identical to PULSE-A's own ("this core is dispatching"), i.e. structurally rather
than evidentially.

The anti-witness is asserted on the wire, once per boot: `run_bsp` emits
`[schedx86] load-prejoin` one statement before entering `run()`, at the single instant
when core 0 has never folded a span and every AP has. That line **must** read `c0=--`
with at least one AP carrying a percent; `c0=0%` there refutes the instrument outright.
Note what it does *not* claim: the x86 BSP **does** enter `run()` (`main.rs:1362`
whenever `online.len() >= 2`), so `c0` reads `0%` on every steady line thereafter. The
prejoin `--` describes one instant, not a permanent state.

#### Design decisions, none of them ports

* **Spans in TSC, freshness in milliseconds.** `CNTPCT` is system-global, so on
  aarch64 one core may subtract another's timestamp. `rdtsc` is **per-core**, and
  cross-core synchronization is a firmware property this kernel neither programs nor
  verifies — a small negative skew wraps to ~2⁶⁴ and a small positive skew fabricates
  a window of activity. So every quantity a *remote* reader evaluates uses
  `arch::ms()` (globally coherent: core 0 alone advances `APIC_TICKS`), and only a
  core's own self-measured spans are in cycles. The `CoreLoad` field is
  `fold_age_ms`, not `fold_age_cyc`, for that reason.
* **Age-on-read for the SELF row only.** The cross-core objection above does not apply
  when the reader *is* the owning core, where the subtraction is the same same-core
  `rdtsc` pair the fold already performs twice per dispatch — and it needs no fences,
  because a core's scheduler loop runs only when no task is running on it, so there is
  no concurrency to order. This matters because the witness is emitted from the render
  task at the *end* of its pass, and that task's span is folded only when it later
  blocks in `recv`: without the self arm, c1 reported its own load missing most of the
  current pass every time, biased low by ~2× at exactly the sample point. Adding it
  moved c1 from `0–1%` to `2–3%` in the QEMU battery.
* **No cross-core age-on-read (PULSE-5); a pegged-core test instead.** For *remote*
  cores PULSE-5's remedy still needs the ruled-out subtraction. The freeze it fixes is
  real here too — the arc's own QEMU smoke caught a core printing `64%` then `--`,
  having stopped folding because it was *inside* a task — so `core_load` instead reads
  `SCHED[cpu].current != 0` together with a fold age past a full window and reports
  100 %, marked `*`. Both inputs are globally coherent.
* **No `PaddedUsize`.** Its justification is A72 LL/SC livelock, which does not exist
  on x86. `CoreAccount` *is* `repr(align(128))` on its own merits — plain write-write
  false sharing between neighbouring cores' slots on the dispatch path, at the 128-byte
  effective granularity of Sandy/Ivy Bridge's adjacent-line prefetch.

#### Documented limits — read these before balancing against these numbers

1. **ISR-only load is invisible; it reads `0%`.** A busy span includes interrupt
   handlers that fire *while a task is running*, but the idle arm charges the waking
   handler to **idle**. So a core with an empty run queue that is saturated servicing
   device IRQs reads `0%`. Not hypothetical: core 0 is the sole advancer of
   `APIC_TICKS`, carries a strictly larger ISR share than any AP, has nothing pinned to
   it, and will report `0%` on every line regardless of how much interrupt work it does.
2. **The instrument CAN over-report, by one mechanism and one only.** `busy_pct`'s
   partial-window blend fills the not-yet-elapsed remainder of the current window at the
   last completed window's rate, so a core that was busy and has just gone idle decays
   from its old percent to 0 across ~250 ms instead of dropping instantly. Bounded by
   one window, always decaying, and it cannot fire for a core idle for a full window —
   but **any refutation criterion of the form "a percent on a core PULSE-A shows at
   `busy=0`" must tolerate it**, or it is a false-refutation trigger. (The alternative,
   reporting `busy/elapsed` alone, is worse: right after a roll a single 2 ms busy
   sample would print 100 %.) The missing sub-window term for *remote* cores can only
   under-report, which is the safe direction; the blend is the exception.
3. **A foreground shell command silences the witness.** `handle_key` holds
   `SCREEN_APP_ACTIVE` for the whole of `dispatch_command`, during which
   `x86_input_service` sends nothing into `GUI_CHANNEL_X86` and the render task is
   blocked inside the command — so there is **no `depth` line and no `load` line** for
   the duration. Use `bg` to observe a program's load, not a foreground `run`.
4. **Two builds emit neither line.** Both require `run_bsp`, which the `rast` feature
   and the `online.len() < 2` fallback (`main.rs:1363`, "GUI stays inline on the BSP")
   both skip — and in those builds `x86_render_service` is never spawned either. A
   silent capture from such a build is not a regression.
5. **The column count is capped at `MAX_CPUS = 8`.** The bench rMBP is exactly 8
   logical cores, i.e. zero headroom. When the cap binds, the line says so
   (`cores=8/N <CAPPED>`) rather than silently reading as a shorter machine.

The steady-state line rides the existing `[schedx86] depth` clock gate (~5 s) in
`x86_render_service`, emitted after `pal.render()` — below the event-routing block,
which is the focus-trap seam and is not to be perturbed by an instrument. Every
`depth` line should be followed by exactly one `load` line.

Two properties of the emit are load-bearing rather than stylistic. The per-core
**snapshot is taken with interrupts masked**: `run_queue_len` acquires `RUN_QUEUES[c]`,
a plain `spin::Mutex` with no IRQ masking whose own doc names it a WEDGE-4 `<W1>`
hazard site, and this witness runs from a *preemptible* task on the render core.
Unmasked it is a permanent silent self-deadlock waiting to happen — preempt the render
task while it holds `RUN_QUEUES[1]`, and `run()` on the same core then needs that exact
lock at IF=0 to requeue the very task holding it; nothing breaks the cycle, nothing
panics, and since the witness is emitted *from* the dead task, nothing reports it. The
masked section is bounded: ≤ 8 iterations of lock-free reads plus one non-nested
run-queue acquisition each, no allocation, no UART, no other lock.

The `serial_println!` is left **outside** that mask, and the reason matters because an
earlier version of this paragraph gave a wrong one. It claimed the placement avoids
"~8 ms of masked interrupts every 5 s" — false: `serial::_print` already wraps its
entire write in `without_interrupts` (`serial.rs:105`), so that masked wire time is paid
either way and nesting would change nothing measurable. The true reasons are that the
snapshot is masked because `run_queue_len` *needs* it and the print does not, and that
the print cannot re-open the wedge for a structural reason: `_print` takes `SERIAL1`
with **`try_lock()`, never `lock()`**, defers to `serial_ring` when contended, and has a
lock-free `raw_byte` panic hatch — so no core can block on the UART lock and no holder
can be preempted into a cycle. The serial path is immune to this wedge by construction.

The whole line is then composed into one stack buffer and handed to a **single**
`serial_println!`: piecewise fragments would drop the UART lock between fields, and a
witness another core can cut in half is not evidence.

**A row counts as live for three independent reasons**, and the list is written out
because losing one of them nearly printed the `--` token for the best-measured core on
the machine. `tracked` is `live > 0 || pegged || fold_age_ms < 500 ms`: (1) we hold a
measured in-flight span for it (the self row); (2) we can infer one (`pegged`, remote
rows); (3) it folded a span recently. Gating `pegged` on `live == 0` — correct in
itself, to make inference a remote-only fallback — briefly removed reason (1) from the
predicate, and since `fold_age_ms` for the self row *is the current pass's duration*,
any render pass reaching the emit ≥ 500 ms after its dispatch would have printed
`c1=--` while a measured 100 % sat unused behind it. Boot AH's first render pass is
237 ms against that 500 ms threshold. Anything added later that produces a percent must
extend the list too.

The pegged token's `(name)` is **best-effort**, not proven. `core_load` loads `current`
before the name so the name is as-new-or-newer than the `current` that set `pegged`,
which closes the stale direction; a remote dispatch landing between the two loads can
still name a newer task, and the emit-side mask cannot help because the racing writer is
another core. Read the `*` as evidence about the core and the name as a strong hint.

Still open after this arc, in the order the campaign takes them: BSP-serial boot
probing and band-parallel compositor flush. Placement and work stealing are the next
section. `ui_status::live_permille`'s x86 arm still returns `None` and falls back to
the event meter — wiring it to this feed so the panel strip and the serial line read
one source is the survey's Arc-0 scope item 4, **deferred by the implementing session
and awaiting the integrator's explicit accept**, not silently dropped.

### Load-balanced placement + work stealing (x86_64, SMPBAL-X86)

The measurement SCHEDLOAD-X86 exists to make finally got taken at the bench: six vug
instances open on the rMBP, and the load line read one core pinned near 100 % with
several workers flat at 0 %. Two mechanisms produced that, and this arc lands both
halves plus the correctness fix migration forces.

**Why the pile-up happened.** Every x86 spawn site named a core, and that decision was
final — `make_ready` enqueued on `task.cpu` unconditionally, nothing re-placed at wake,
and `run()`'s empty-queue arm went straight to `enable_and_hlt`. `bg_place_cpu` returned
the caller's core, the caller is the shell, and since SCHED-X86 the shell *is*
`x86_render_service`. So every program launched landed on the render core and stayed
there, whatever the other seven cores were doing.

**Placement — `CPU_AUTO`.** `sched::CPU_AUTO` (`usize::MAX`) is a sentinel `target_cpu`
meaning "place me". `pick_cpu` checks the **pin contract first**: any other value is
returned unchanged, so every pre-existing spawn site — render, input, usb-pump, the
whole fixture ladder — is untouched and `SCHED-X86 PLACE-CHECK` keeps reading `PASS`
with no exemption (and must not be given one; a `PLACE-CHECK` taught to tolerate
migration cannot falsify what it was built for). For `CPU_AUTO` it scans the
**dispatching** cores — `ONLINE_MASK`, set at the top of `run()`, which is the "is this
core actually popping its queue" predicate `bg_place_cpu` has wanted since WINX-3 — and
picks by (1) shallowest ready queue, (2) lowest SCHEDLOAD-X86 busy percent, (3) a
rotating cursor. **Two** exclusions, of two different kinds:

| # | excluded | kind | relaxes? |
| --- | --- | --- | --- |
| 1 | core 0, for a **cooperative** (IF=0) ring-3 task | correctness — it masks the timer for its lifetime and freezes the global ms-clock | **never** |
| 2 | the **service** core | deadlock — `x86_usb_pump` holds the raw `XHCI_CONTROLLER` spinlock there, and a co-located preemptible task that also takes it (any ring-3 program touching storage) can preempt the holder then spin on a lock whose owner cannot run. This is `xhci_worker_cpu`'s rule, which DECLINES rather than co-locate | tier 2 |

Rule 2 relaxes in one tier so a machine with too few dispatching cores still places
somewhere real. Rule 1 has no tier. `run_user_image` and `bg_place_cpu` now pass
`CPU_AUTO`, which closes the open item above.

**NO RESERVING CORES — the third exclusion is gone (Peter, 2026-08-19).** A rule 3 used
to sit in that table: the **render** core, excluded on performance grounds ("it owns the
panel and hosts the shell, and it is the core the imbalance piled onto"), relaxing at
tier 3. The bench refuted it. Across a full six-vug sitting the metal capture read c1 at
**0–5 %** while every other core ran **64–82 %** — the machine holding 12.5 % of itself
idle for a latency budget that is never spent there, because the composite actually runs
on the *presenting task's* core. Peter's ruling is verbatim: **"THERE IS NO RESERVING
CORES."** The render core is now an ordinary member of the dispatch pool — placed onto by
`pick_cpu` and `sibling_online_cpu` on the same key chain as any other core, a steal thief
in `try_steal`, and a victim as it always was. The render *service task* is unchanged:
still spawned pinned to its core, where it blocks in `GUI_CHANNEL` `recv` and costs that
core nothing while it does.

The service-core exclusion (rule 2, and its `try_steal` twin) **survives the ruling** and
must never be cited as precedent for reviving a reservation: it prevents a deadlock, it
does not hold capacity. The same goes for `smp::worker_pool`'s render exclusion, which is
about WHERE a handful of named fixtures are spawned (a cooperative IF=0 fixture parked on
the render core stalls the panel; `xhci_worker_cpu`'s takers share a raw spinlock with the
shell) and holds no capacity idle.

**Correction — `try_steal`.** Placement is one guess made at spawn; stealing is what
makes a wrong guess cheap. An idle core — one whose own queue came up empty — takes one
eligible task off the deepest other queue instead of halting: peek depths, steal under
the **victim's lock only** (re-checking depth), release, re-home `task.cpu`, push
locally. One lock at a time, so no ordering hazard. `STEAL_MIN_DEPTH = 2` leaves the
last ready task at home, which is what stops two idle cores ping-ponging a lone task.
`steal_one` scans **LOW→HIGH** priority — take a core's background work, never its most
urgent task. The **service** core does not steal — exclusion 2 above, repeated here
because a steal is a placement decision `pick_cpu` never sees — but it remains a valid
*victim*, since draining eligible work off it is the point. (The render core used to be
excluded here too; the 2026-08-19 no-reservation ruling removed it, and it now steals like
any other core.)
Exclusion 1 is likewise repeated in `steal_one`'s predicate.

**Eligibility is the pin contract, and on x86 that includes ring-3 tasks.** `Task.steal_ok`
is set once at spawn as `requested_cpu == CPU_AUTO`. aarch64 excludes EL0 tasks
categorically; x86 does not need to, and the asymmetry is worth stating precisely
because it is the reverse of what was expected. aarch64 cannot steal an EL0 task
because three per-core residency tables keyed on `task.cpu` are mutated only at spawn,
re-place and reap while its `try_steal` re-homes without transferring them — x86 has no
such tables. There is no ASID, no per-`(core, slot)` table, no FPU/XSAVE state (the
target builds `+soft-float` with SSE/AVX off), no FS base, and GS base is pure per-core
state a migrated task correctly reads fresh. Everything per-task is re-derived at the
single dispatch site: CR3, TSS.RSP0 and the SYSCALL kernel rsp. So `task.cpu` is a pure
*policy* field here, and the whole aarch64 SPREAD-4…15 apparatus — margin, freshness
gate, co-placement, recruit — is deliberately **not** ported. That layer exists solely
because EL0 tasks there cannot be stolen, it ships a documented 4–10 migrations/sec/task
churn cost, and its own file records three successive arcs of it being "correct on a
saturated board and wrong on an idle one".

**The TLB obligation — and why it ships in the same commit.** x86 has no broadcast
invalidation and no shootdown IPI: `invlpg` and `mov cr3` are both core-local. Every
user-TLB argument in `memory.rs` rested on an unwritten premise — *the only core that
ever installs a given user CR3 is the core that will restore the kernel CR3 when that
address space is torn down* — which held **only because tasks did not migrate**.
Stealing falsifies it, and the failure is silent rather than a crash, because
`slot_cr3(s)` is a fixed physical address reused by every tenant of that slot over the
same backing frames:

* *intra-tenant* — a task stolen away from core B, changing its own leaves elsewhere,
  and stolen back, runs on B's stale entries if B dispatched nothing else meanwhile;
* *cross-tenant* — a task migrates off B and exits elsewhere, so only that core
  restores; the slot is freed and re-allocated at the same CR3 over the same frames, and
  the **new** tenant dispatched on B runs under the **previous** tenant's cached
  translations. That is stale W bits against the new ELF's W^X layout (a live W^X bypass
  of the GR19 shape) plus reach into window-surface pages `clear_slot_fb` unmapped with
  a local `invlpg` only.

Both need only that core B dispatched nothing with a different CR3 in the interval,
which is exactly the state `try_steal` runs in. The fix needs no IPI, because a core
idling on a stale root cannot *consume* a stale user translation — the kernel half is
identical in every slot PML4 and the kernel reaches user backing through identity
aliases, never `USER_BASE`. So the reload is deferred to the dispatch that would use it:
`memory::AS_GEN` is bumped by every user-leaf mutation (slot build, teardown/recycle,
the ELF permission pass, every window map/unmap), `SchedCpu.cr3_gen` records the
generation at which each core last validated its live CR3, and the dispatch site reloads
CR3 unconditionally when the two differ instead of taking `switch_cr3_if_needed`'s skip.
A full non-global flush on this hardware — nothing the kernel maps carries `PTE_GLOBAL`
and firmware leaves CR4.PGE clear. Cost: one relaxed load per dispatch, one atomic add
per mutation, at most one extra `mov cr3` per core per mutation. Deliberately *not*
"make the reload unconditional for user targets", which would regress the U3.5 fast path
the conditional exists for.

**Witnesses.**

```
:: SCHEDPLACE-X86: '<name>' -> c<N> (ld=<key> cred=<inflight> load=<pct>% from c<caller>) ::  (first 24 auto-placements)
:: [smpbal] steal '<name>' c<A>->c<B> ::                                       (first 24 migrations)
[schedx86] load c0=…% … sw=[…] q=[…] steal=<moves>/<passes> asgen=<gen>/<reloads>  (every ~5 s)
```

`steal=` carries **both** terms, never just the count, because the ratio is the
falsifier and the count alone is not. aarch64 paid for that lesson: a steal counter
climbing at *dispatch* rate rather than at *idle-pass* rate means churn, not balance. A
fleet in balance shows moves ≪ passes, with moves going flat while passes keep climbing.
**`steal=0/<large>` is a healthy reading**, not a dead instrument: it says placement got
it right and no core ever had backlog behind a running task. `passes = 0` is the dead
reading.

`asgen=` exists because the CR3-generation fix has no other falsifiable surface — a
stale-TLB cross-tenant read is silent by construction, so the only thing observable
about the mechanism is whether it *fires*. A generation climbing with window/launch
activity while `reloads` stays at 0 means the dispatch site is not consulting it.

**Limits, stated rather than discovered later.**

1. **Sleepers are never touched.** A sleep deadline lives in the parking core's local
   APIC tick domain and is not portable between cores. A sleeping task is not in a run
   queue, so `try_steal` cannot reach it — and must not be "helpfully" taught to.
2. **The steal takes a *remote* run-queue lock**, which is new on this arch. Both the
   peek and the steal go through the WEDGE-4 bounded-spin wrapper on `wedge2` builds, so
   `<W1>`/`<W2>` still see every acquisition. Land the first metal boots with
   `UNAOS_WEDGE2=1`.
3. **`<W1>` changes meaning slightly.** `wedge4`'s flag is set and cleared by the
   captured core index, so it is still mechanically correct across a migration, but read
   the token as "some core was mid-enqueue", not "this core was".
4. **The load percent cannot see ISR load** (SCHEDLOAD-X86 limit 1), so a core saturated
   servicing device interrupts with an empty queue reads 0 % and looks like a placement
   candidate. That is unchanged by this arc and bounded by the fact that placement is
   only a hint.
5. **Two witness structures under feature gates would under-report across a migration**
   and are left alone deliberately: `video/cursor.rs`'s overlay `owner_cpu` and
   `wedge2::CHAIN_CORE` both compare an owner-core claim recorded earlier. Neither is
   reachable in practice — the compositor task is explicitly placed, hence pinned — and
   weakening either would retire a real falsifier.

### The corrector that could not see the packing (x86_64, VUGSPREAD)

**Symptom, Boot AS.** A vug held `[wpace] win=1 pres=96 paced=0 slept=0ms rate=19.1/s`
for the whole capture — every present already late, never reaching the pacer's sleep —
while the desktop on `win=0` held a clean `60.0/s` on the same wire. Present syscalls
were not the cost (`[wc-h] maxpresent_us=4775` against a ~52 ms frame). The CPU-pulse
census read `c0 busy/idle=61455/249 -> 99%`, `c5 -> 99%`, and `c3`/`c4` at exactly
`0/250`: two cores pegged, two cores never dispatching a thing. And the corrective half
of SMPBAL-X86 read `steal=1/4574483` — **one migration in four and a half million idle
passes, over ten minutes, on a visibly lopsided machine.**

**Three defects, all convicted from the source before the next boot.**

1. **A ring-3 thread's core was treated as a pin.** `spawn_user_thread` set
   `steal_ok = target_cpu == CPU_AUTO`, and its own comment observed — without alarm —
   that `sys_thread_spawn` always names a core, so `steal_ok` was *always* false. The pin
   contract exists so that render / input / usb-pump / fixtures, which name a core because
   the core is part of their correctness, never migrate. A `SYS_THREAD_SPAWN` core is not
   that: ring 3 passes `place ∈ {0 = my core, 1 = a sibling}`, a locality *hint* in the only
   vocabulary the syscall has. Promoting it to a kernel guarantee made the parent process
   movable and its own threads immovable, which was never a considered position. A vug is a
   parent plus two workers and asks for one worker at `place=0` — so parent and worker
   shared one core **by request, permanently, with nothing in the system able to undo it.**
2. **"A sibling" meant "the same sibling, every time."** `sibling_online_cpu` returned the
   first core matching its probe, in index order, with no reference to load. Every `place=1`
   thread on the machine landed on the same low-numbered core.
3. **The steal floor counted the wrong population.** `STEAL_MIN_DEPTH = 2` was justified as
   "a core with one task is not loaded" — a true sentence attached to the wrong quantity. A
   run queue holds only READY tasks; the executing one lives in `SCHED[cpu].current`. So a
   floor of 2 *on the queue* means three runnable tasks before a core counts as loaded, and
   the 2-on-1 packing sits at queue depth **one**. It was not missed by the corrector; it
   was below the corrector's floor by construction.

**The repairs, all in `arch/x86_64/sched.rs`.** A ring-3 thread is steal-eligible, full
stop — it starts exactly where `sys_thread_spawn` asked and an idle core may correct it
later, which is this arch's whole stated model. `sibling_online_cpu` now chooses among the
eligible cores with `pick_cpu`'s key chain (shallowest queue, then lowest rolling busy
percent, then the shared rotating cursor), deprioritising the service core in a two-step
ladder rather than excluding it — excluding it outright would reintroduce the silent
`bg_place_cpu` hang its probe exists to prevent. (It deprioritised the render core on the
same rung until the 2026-08-19 no-reservation ruling; that core is now scored like any
other sibling.) And the floor is asked of the *victim*:
depth 1 suffices when the victim is running something (that is two runnable tasks), while a
victim at `PRIO_IDLE` keeps the floor of 2, because that core is between tasks and about to
dispatch the very task a thief would take — which is the ping-pong the constant was
actually reaching for. The eligibility probe, the pin contract for named *kernel* spawns,
the one-task-per-idle-pass rate bound and the `AS_GEN` TLB discharge are all unchanged.

**`[spread]` — the placement witness.** One line, emitted from `emit_load_witness` so it
rides that instrument's existing rate limit and adds no clock:

```text
[spread] pack=0 spare=3 rqp=[0/0/0,1/0/0,--,1/1/1,…] steal=4/812331 m1=3 mh=4 remig=0 packseen=12 cr3sw=91204 decl=t:0 e:812327 f:0 p:0 d:0 i:0
```

`rqp` is `running/ready/pinned` per core (`--` for a core that never entered `run()` — not
an idle core, and the distinction is the same one `[schedx86] load` draws between a measured
zero and an absent measurement). `pack` counts dispatching cores carrying `running + ready
>= 2`; `spare` counts dispatching cores with nothing at all. **`pack >= 1` together with
`spare >= 1` is the defect.** The column cap is carried onto this line too — a capped machine
under-reports `pack` and over-reports `spare`, the one direction in which the witness could
certify headroom that is not there.

`decl=` breaks declined steals into disjoint reasons: `t` thief excluded, `e` nothing ready
anywhere, `f` below floor at the peek, `p` all pinned, `d` a true drain (the victim's queue
was **empty** under the lock), `i` the victim went **idle** between peek and lock and raised
its own floor to 2. `d` and `i` are split deliberately: `i` is this arc's ping-pong guard
firing and folding it into a race counter would hide the mechanism a reviewer most wants to
audit.

**Conservation law and its tolerance.** `e + f + p + d + i + moves == passes`, with `t`
outside `passes`. The terms are sampled at slightly different instants from a live machine,
so a residual of a **few** is skew and means nothing; a residual in the **thousands**, or one
that grows with the capture, means a return path was added without a counter and every
attribution below it is suspect. Score the magnitude, not the equality.

**Attribution.** `m1` counts moves taken from a victim at ready-depth 1 — precisely what the
old constant floor refused. `mh` counts moves of a task whose core came from a ring-3 hint —
precisely what the old pin contract froze. They are independent, not exclusive: a vug worker
packed behind its parent scores **both**, and that is the honest answer, since neither repair
alone would have produced that move. What the pair does settle is the one-sided cases —
`mh > 0` with `m1 == 0` means the pin was the whole story, `m1 > 0` with `mh == 0` means the
floor was. Repair (2), `sibling_online_cpu`, leaves no trace in the steal path at all and is
read off `rqp=` at launch and off `SCHEDPLACE-X86`, never off these.

**`packseen`** is the high-rate companion to `pack`. The census samples every ~5 s from the
render service, so packing that forms and clears inside a frame is invisible to it; `packseen`
is evaluated on every steal pass — millions per capture — inside the peek loop that already
holds the depth. A `pack=0` census standing beside a near-zero `packseen` is a real
refutation; `pack=0` alone is only a quiet sample.

**`cr3sw`** is the price tag. A migration lands a task on a core standing on another root, so
the dispatch takes `switch_cr3_if_needed`'s reload — and on this hardware nothing kernel-mapped
carries `PTE_GLOBAL` and firmware leaves CR4.PGE clear, so every one is a whole-TLB flush. It
is not a migration counter (ordinary two-program alternation on one core takes that arm
constantly), so read its **delta** against the `steal=` delta over the same interval.

### Reading the next capture

All three repairs are in force on the next boot, so the table below is what *that* machine can
print. An earlier draft of it listed pre-fix diagnoses that a post-fix boot cannot reach, which
is the same defect as a witness that cannot fail.

| next boot shows | reading |
| --- | --- |
| `pack -> 0`, `moves` a handful then flat, `remig 0`, `win=1` off 19.1/s | **PASS** — spread, and settled rather than oscillating |
| `pack=0` **and** `packseen` near 0, `win=1` still 19.1/s | **REFUTED.** No packing at the census *or* across millions of pass observations. Go to the yield-spin barrier: the TIME feed already reads c0/c5 at 2–3 % where the EVENT feed reads 99 % |
| `pack=0` but `packseen/passes` materially non-zero, rate unchanged | the packing is real and **transient** — sub-census, forming and clearing inside a frame. Neither the floor nor the pin can hold a queue that is empty whenever it is looked at; this is a barrier/wake-latency story, **not a placement one** |
| `pack>=1`, `spare>=1`, packed core's `pinned` > 0 | **the fix FAILED.** Post-fix a ring-3 thread is steal-eligible, so a pinned task on a packed core is either a kernel task that legitimately named that core (check the name on `[schedx86] load`) or a ring-3 path that does not go through `spawn_user_thread`. Find which before touching anything else |
| `pack>=1`, `spare>=1`, `pinned=0`, `decl i:` climbing | the **idle-floor guard** is holding the packing: ready-holding cores keep going idle between peek and lock and re-raising their floor. Working as specified, and declining a move that would have helped. Tuning, not a defect — the change would be to admit a depth-1 steal once the thief has been idle more than one pass |
| `decl f:` climbing | the same guard one step earlier. With the per-victim floor in force `f` can only fire when *every* ready-holding core is at `PRIO_IDLE` with depth 1. It does **not** mean "the old floor hid it" — that diagnosis is unreachable now |

### The revert criterion, as a number

"Climbing" is not a threshold against a baseline of one move in 4.5 million passes, so the
criterion is numeric. Revert `steal_floor` to the constant `STEAL_MIN_DEPTH` — keeping the
`spawn_user_thread` pin release, which is not implicated in churn — if **any** of these holds
on a steady-state capture, measured as a delta over one `[spread]` interval and sustained
across three consecutive intervals:

- **`remig / moves > 0.5`** — more than half of all migrations are re-migrations, i.e. the
  fleet is passing tasks around rather than placing them.
- **`moves > 100/s`** — a settling fleet is a handful of moves *total*; a hundred a second is a
  balancer oscillating. (Boot AS's whole ten minutes produced one.)
- **`Δcr3sw / Δmoves` far above the `Δcr3sw / Δmoves` of the same workload before the change,
  or `Δcr3sw` itself rising by more than ~2 per move** — each move should buy about one extra
  whole-TLB flush; several means tasks are bouncing between cores that keep re-installing each
  other's roots.

None of these can be evaluated from a single sample, which is why all three are stated as
sustained deltas.

**⚠ Boot A: ALL THREE LEGS FIRE, and that is unread.** Measured over one 11.25 s steady-state window
(793 259 → 804 512 ms) of `~/unaos-bench/capture/gr25-bootA/ttyUSB0.log`: `remig/moves` ≈ **1.0**
(> 0.5), `moves` ≈ **157/s** (> 100/s), `Δcr3sw/Δmoves` ≈ **16.3** (≈ 2 expected). Ten desktop vugs is
not the "steady state" the criterion was written against and the sustained-across-three-intervals test
has not been applied, so this is **not** a revert call — but it does mean the churn criterion and the
transient-packing row above **select the same Boot A signature**, and the `[wpace]`-side arc that cited
that row (userspace.md § VUGSPIN) has since **withdrawn** its conviction on load-invariance grounds. Any
next reader scoring that row against this capture must score the churn criterion beside it; neither is
convicted, and treating row 3 as a unique selection is the specific mistake already made once.

### The churn is now convicted, and the fix is a cooldown, not the revert (x86_64, VUGSPREAD-COOL)

**Boot C settles the "unread" call.** `~/unaos-bench/capture/gr26-bootC/ttyUSB0.log`, a six-vug storm,
holds the churn signature at full strength and SUSTAINED across the whole storm, not one window:
`[spread]` reads `remig=750397/750418` (**≈ 1.0**, every steal a re-migration), `cr3sw` climbs to
**14 073 209** whole-TLB flushes, and it does this while `[schedx86] load` shows **six cores at 99 %** —
i.e. the fleet was ALREADY spread, and every one of those 750 k moves was work thrown away passing the
same handful of vugs around. The operator-visible cost is two-fold: each vug's compose smears across a
rotating set of cores (the uneven per-core reading), and a vug CLOSE frees a core whose idle pass
immediately steals a survivor, co-locating it and destabilising the survivors' rates (the "close makes
the others spike" report).

The mechanism is the one the idle-floor guard does **not** reach. `steal_floor`'s running-victim floor
of 1 is correct on an under-loaded board; on a saturated one the churn is driven by THIEVES, not
victims: a vug blocks briefly inside `SYS_WIN_PRESENT`, its home core's queue empties, that core steals
a neighbour, the vug unblocks and re-queues home — now home is over-subscribed and a third idle core
steals it back. The guard covers an IDLE victim keeping its lone task; it says nothing about a task that
was just moved being moved straight back.

**The fix is a per-task migration cooldown, and it is preferred over the documented revert.** The
revert criterion above prescribes restoring `steal_floor` to the constant `STEAL_MIN_DEPTH`. That would
stop the churn, but it also throws away the depth-1 rebalance this whole section exists to add — a
parent+worker packed on one core with idle cores beside it would once again sit unbalanced. Instead,
`Task::migrate_ms` stamps `arch::ms()` at each migration and `RunQueue::steal_one` refuses to take a
task that migrated less than its cooldown window ago (16 ms flat when this section was written —
escalated per-task since GR27 Boot B, 16→256 ms doubling per re-steal; see the GR27 section below):

- The **first** steal of any task is never delayed — a never-migrated task carries `migrate_ms == 0`,
  which always clears the window — so the depth-1 rebalance VUGSPREAD added fires exactly as before.
- Only the immediate RE-steal is refused, which is the ping-pong and nothing else. A storm settles to
  one-vug-per-core within a transient of ~1–2 moves per task and then STAYS there, because a settled
  fleet has depth 0 everywhere and nothing to steal.
- A later vug CLOSE therefore frees exactly one core with nothing left to steal: the survivors keep
  their homes instead of being re-grabbed. That is the scheduler half of the close-spike; it is not a
  rate cap on the renderer (the vug still presents unbounded), only a damp on re-placement.

**The new witness.** `steal_one`'s per-task skips are counted by `STEAL_COOL_SKIP` and printed as
`cool=` on `[spread]`. It is the direct counterweight to `remig`: a healthy post-fix capture reads
`cool=` CLIMBING while `remig` stays near flat — the ping-pong being REFUSED rather than served. A
cooled task that leaves `steal_one` empty-handed still lands on `p`/`STEAL_D_PINNED` at the pass level,
so the `e + f + p + d + i + moves == passes` conservation law is untouched; `cool=` is a side counter
like `remig`, outside it.

**What this fix does and does NOT even out.** With WCPAR step-3 (the parallel per-core compose) reverted
on trunk, `COMP_GATE` serialises the whole compose again, so the per-core COMPOSE census (`[wcpar]`
`c0..c7`) cannot be evened by any scheduler change — a serial compose lands on whichever core holds the
gate, wherever the vug task runs. What the cooldown evens is the PER-CORE CPU LOAD / pulse (`[schedx86]
load`, `sw=`): the vug TASKS settle one-per-core and stay, so the load reads even and a close leaves it
even. Evening the compose census itself is deferred with step-3 (see `engine.md` §8).

**Next boot's readings, as falsifiers.** On the next six-vug storm: `[spread]` `remig`/`moves` collapses
from ≈ 1.0 toward 0 and `cool=` climbs in its place; `moves` goes flat after the settling transient;
`cr3sw`'s delta per move falls back toward ~1; `[schedx86] load` shows the vug cores at a stable, even
occupancy that a close does not disturb (the freed core simply goes idle, the survivors hold their
homes). A boot where `remig` stays ≈ 1.0 with `cool=0` means the stamp or the read is dead — the brake
is not firing — and is the falsification this witness exists for.

### The flat cooldown was scored and failed: the brake now escalates (x86_64, GR27 Boot B)

**Boot B is the next boot, and the reading is mixed in the way that matters.** GR27 Boot B
(`~/unaos-bench/capture/gr27-bootA/ttyUSB0.log`, the vug-storm slice) shows the brake firing —
`cool=` climbed 25 920 → 631 339, ~3.3 k refused re-steals per second — while STILL failing the
settlement promise: `remig=92162` of `moves=92184` (99.98 % re-migrations, ~540/s sustained) and
`[spread]` held `pack=4-5` **with** `spare=1-2` for ~170 s (61 121 ms `pack=5 spare=1`; 169 727 ms
`pack=5 spare=2`; 223 758 ms `pack=4 spare=2`), with `pinned=0` throughout. So the stamp and the read
are alive, and the falsifier row above ("`remig` ≈ 1.0 with `cool=0`") does not fire — the defect is
in the WINDOW'S SHAPE. A flat 16 ms window does not settle a ping-pong; it stretches its period: every
task exits the window with its history erased, as stealable as a never-moved one, so the fleet replays
the same re-steal cycle at a 16 ms cadence indefinitely. "`cool=` climbing while `remig` stays flat"
turned out to be satisfiable by a brake that refuses six re-steals and then serves the seventh, forever.

**The fix is a per-task ESCALATING window.** `steal_cooldown_ms(migrations)` in
`arch/x86_64/sched.rs` computes `STEAL_COOLDOWN_MS << min(Task::migrations, STEAL_COOLDOWN_ESC_CAP)`
with base 16 ms and cap 4 — i.e. 32, 64, 128, 256, 256… ms for the 1st, 2nd, 3rd, 4th, 5th+
re-steal. Each re-steal a task suffers earns it an exponentially longer residency on its next home,
until at 256 ms (~256 quanta at the 1 kHz tick) the wake/block cadence that drives the ping-pong
(~0.5–16 ms) can no longer outrun the window and the task stays put. Preserved exactly: the FIRST
steal of any task is free (`migrate_ms == 0` clears the test before the window is computed), so the
depth-1 rebalance VUGSPREAD added is untouched; the rtpi priority-boost exemption (a boosted lock
holder is always stealable) is untouched; and knob-off the `not(rtpi)` branch remains the vug-storm
code with only the window widened.

**The decay question, decided: `Task::migrations` never resets.** *(SUPERSEDED 2026-08-19 by
SPREADSETTLE, below. The argument in this paragraph is sound about STRANDING and wrong about COST,
and the boot-4 capture measured the difference. Kept verbatim because the next person to reach for a
decay must know which half of it still holds.)* The gate is RECENCY-based — a
`migrate_ms` older than the capped 256 ms window clears it regardless of how large the count has
grown — so a long-settled task is always immediately stealable and history alone can never strand a
task on a bad core: a genuine topology change (a vug CLOSE freeing a core) sees every settled
survivor as stealable on the first pass, and only a *second* re-steal within a quarter second pays
the escalated window. Resetting the count on the wake-side push was considered and rejected as
actively harmful: the ping-pong cycle IS block → wake → push → re-steal, so every re-steal the
escalation exists to refuse is preceded by exactly such a push — a reset there would zero the
history each time around the loop and reproduce the flat window with extra steps.

**Expected wire signature next boot, as falsifiers.** On the next vug storm: `remig/moves` collapses
toward zero mid-workload (each task can be re-stolen at most ~4 times before its window outlasts the
cadence, so re-migrations are a bounded settling transient, not a sustained rate); `pack>=1` WITH
`spare>=1` closes after the transient instead of co-existing for minutes; `cool=` stops climbing
linearly — it rises during settling and then goes near-flat, because a settled fleet has depth 0
everywhere and `steal_one` is no longer even walking candidates to refuse. A boot where `remig`
keeps climbing at hundreds per second, or where `pack`+`spare` co-existence persists past a few
seconds of storm with `pinned=0`, refutes the escalation shape itself — the next lever is the cap or
the base, and the revert criterion above (restore `steal_floor` to `STEAL_MIN_DEPTH`) still stands
behind it, unchanged in its numbers.

### The escalation saturated, and the placement was blind (x86_64, SPREADSETTLE)

The boot-4 capture of 2026-08-19 answered the paragraph above with a refutation, on the escalation's
own terms. Over 350 s of a six-vug sitting: `remig/moves = 1.00` sustained, `cool=` climbing
6 600–11 500/s, `pack >= 1` **with** `spare >= 1` in every 5 s sample, ~184 k whole-TLB flushes per
5 s bought with it. That is the "refutes the escalation shape itself" row, and it convicts two
independent defects — one in the corrector, one in the placer.

#### Defect A — the cooldown ladder had no gear left

`steal_cooldown_ms` was keyed on `Task::migrations`, a **lifetime tally that only climbs**. Any task
surviving four re-steals is parked at the terminal 256 ms window permanently, so the corrector re-pays
a quarter-second refusal storm for every single move it makes, forever. The window did not terminate
the ping-pong; it set the ping-pong's period and then ran out of ladder.

**Repair 1 — the ladder decays.** Policy moves off `migrations` onto a new `Task::steal_esc`
(`0..=STEAL_COOLDOWN_ESC_CAP`), which goes **up** one per migration and **down** one per full
cooldown window served sitting still (`decayed_esc`). `migrations` keeps counting for the witness and
is no longer policy input at all. This is not a retreat from the never-reset argument, it is that
argument applied to the right quantity: what must not be erased is evidence of *recent* churn, and a
ping-ponging task is by construction re-stolen *inside* its window, so it serves no window and decays
nothing. What is erased is history the task has demonstrably stopped exhibiting — 496 ms of stillness
walks a level-4 task back to level 0. The wake-side-reset objection is untouched and still honoured:
decay is keyed on `migrate_ms` residency, not on queue events, so block → wake → push decays nothing.

`decayed_esc` cannot change the cooldown *test*'s answer — its first step requires
`resident >= steal_cooldown_ms(esc)`, the exact negation of the test — so the test stays a single
shift and the walk runs once, on the steal path, bounded to four iterations by the write-side clamp.

**Repair 2 — a sustained-idle thief is not a ping-pong thief.** Decay alone cannot collapse `cool`:
in a limit cycle whose period *is* the window, one level decayed and one earned cancel and the ladder
sits at equilibrium — correct for the ladder, useless for the machine, because the refusals in
between are the 11 500/s. The term that ends them is the one this file's own `[spread]` reading table
already prescribed ("let a depth-1 steal through when the thief has been idle for more than one
pass"). A thief that has been **continuously empty** for at least `STEAL_COOL_BYPASS_MS` (= the base
window, 16 ms) may take a cooling task from a victim that is genuinely packed — running, with ready
work behind it. Both conditions are required. The brake's premise is a thief that went idle for a few
hundred microseconds while a vug sat in `SYS_WIN_PRESENT`; a core that has held nothing for a whole
base window is not that thief, and the imbalance it is looking at is real. New per-core state:
`SchedCpu::idle_since_ms`, set on the empty-queue arm (only when unset, so a staying-empty core
accumulates one span) and cleared at the single dispatch site.

Rate-bounded by construction rather than by a counter: a bypass steal gives the thief work, so it
must re-earn the whole window before bypassing again — at most one cooldown-overriding move per idle
core per 16 ms, against the ~540/s boot-4 measured. The pin contract and the cooperative-ring-3 /
core-0 rule are **never** bypassed; only the cooldown is, because only the cooldown is tuning.

#### Defect B — spawn placement was blind on an idle machine

`pick_cpu` / `sibling_online_cpu` keyed on (queue depth, rolling busy percent, rotating cursor). On
an idle machine during a burst, **both measured keys go blind at the same time and for opposite
reasons**: a placed task is popped and running before the next spawn reads the queue, so depth reads
0 everywhere; and 47 ms of new work does not move a ~250 ms lagging window, so the percent reads 0
too. The whole decision fell to the cursor — and the cursor was itself collapsing, because it was
applied modulo `MAX_CPUS` while the choice was over the *eligible* cores: every cursor value landing
on an excluded core handed its turn to the same successor (2 of 8 values with the service core
excluded, 3 of 8 with core 0 also excluded for a cooperative spawn). Four of six vugs landed
together, every `SCHEDPLACE-X86` line printing `load=0%`.

**Repair 3 — an in-flight placement credit.** `PLACE_BURST` / `PLACE_LAST_MS` record how many
`CPU_AUTO` placements each core has been handed within `PLACE_BURST_MS` (pinned to `LOAD_WINDOW_MS`,
250 ms — the identity is the argument: the credit covers exactly the interval `busy_pct` is
structurally blind in). The primary key becomes `max(depth, credit)` — `max`, not `+`, so a task that
*is* still queued is never counted twice. The credit is the one number that is neither instantaneous
nor lagging: it is what the placer has already decided, which no measurement of the machine can
report yet. `pick_cpu` and `sibling_online_cpu` share it, for the same reason they share the cursor.

**Repair 4 — rotate over the eligible set.** Both functions now gather the eligible cores into a
stack array (`MAX_CPUS` triples, no allocation, lock discipline unchanged) and then choose with the
cursor taken modulo `n`. That is what round-robin means and what the cursor was always documented to
provide.

**Wire.** `[spread]` gains `byp=` (`STEAL_COOL_BYPASS` — cooldown overrides that took a task the
window refused) and `dcy=` (`STEAL_ESC_DECAYED` — escalation levels shed). `SCHEDPLACE-X86`'s `q=`
becomes `ld=` (the key is now `max(depth, credit)`, so a field labelled `q=` would print a queue
depth the queue does not have) and gains `cred=`.

**Falsifiers, and they are three-term readings — `cool` is never read alone again.**

| the next boot shows | reading |
| --- | --- |
| six-vug launch: six `SCHEDPLACE-X86` lines on **distinct** cores, `cred=` stepping up within the burst | **PASS** on Defect B. |
| a launch burst still clumping, with `cred=` climbing on the winner | the credit is live and being outvoted — the blindness is not in the keys this arc repaired. Look at `ld=`: if it is 0 on a core that just took a placement, `place_charge` is not reaching the path the spawn used. |
| steady state: `cool` / `byp` / `dcy` all flat after the launch transient, `remig/moves` off 1.00, `pack`+`spare` co-existence closed | **PASS** on Defect A. |
| `cool` climbing with `dcy=0` and `remig/moves` at 1.00 | the ladder is at equilibrium — every task re-stolen strictly inside its window, so nothing serves a window and nothing decays. Not a decay failure; decay has nothing to act on. The working term here is `byp`; if that is also flat the fleet is genuinely thrashing and the *base* window is the lever. |
| `pack>=1`, `spare>=1`, `decl p:` and `cool` climbing, `byp=0` | **unreachable post-fix.** A core that stays spare earns the bypass within 16 ms, so either `byp` climbs or the `spare` column is lying about an idle core. |
| `byp` climbing steadily rather than settling flat | **the bypass became the ping-pong.** 16 ms of continuous idleness is not separating a correcting thief from a `SYS_WIN_PRESENT` blip on this hardware. Raise `STEAL_COOL_BYPASS_MS`; do not remove the mechanism, which restores the boot-4 refusal storm verbatim. |

**Metal falsifier (this arc's DONE is QEMU + `strings`; the behaviour is metal).** Six-vug launch
spreads across distinct cores; steady-state `remig/moves` collapses from 1.00; no core sitting at 0 %
while another is packed.

**Tuning constants are bench-priced, not portable.** `STEAL_COOLDOWN_MS`, `STEAL_COOLDOWN_ESC_CAP`,
`STEAL_COOL_BYPASS_MS` and `PLACE_BURST_MS` are priced against the 8-core rMBP. The pi track carries
its own spreadtune numbers; both sets are to be re-priced together at the next trunk sync rather than
assumed to transfer.

**No capacity is held.** Nothing here reintroduces a reservation (Peter's NO-RESERVING-CORES ruling,
`f4bd5a73`): the bypass *widens* who may take work and the credit only reorders equally-idle
candidates. The service-core steal exclusion is untouched — it is a deadlock rule, not a reservation.

**Naming the moves.** `STEAL_LOG_COUNT` is reset at each `storm_census` boundary. The cap of
24 named migrations per boot was generous when a boot produced one steal; with the corrector
able to see the packing, early-boot settling would burn the whole cap and the storm the arc
exists for would report its moves as an uninterpretable increment on `steal=` — the one boot
that could answer the question would be the one that could not. The cumulative totals are
untouched by the reset; only the naming is renewed.

**A migration's FP obligation, stated correctly.** The migration-soundness argument's FP bullet
used to read "no FPU/XSAVE context exists anywhere in this kernel", which is false: x87/MMX is
ring-3-reachable here (CR0.EM/TS are 0 out of INIT, and the U2.5 first-entry scrub in
`user_task_trampoline` exists because of it). The correct, narrower statement is that **x87 is
not per-task state in this scheduler at all** — nothing saves, restores or attributes the FP
file to a task, so a ring-3 task already loses it to the next task on its own core and a
migration is the same hazard on a different core, not a new one. What makes the omission
tolerable is a **convention, not an enforcement**: userspace is built `+soft-float`, so no
program in the tree keeps a value there across a preemption. A future ring-3 program compiled
with hardware FP would need per-task save/restore — with or without stealing.

**QEMU cannot gate this.** The behaviour is a function of core count, real dispatch timing
and a frame-paced ring-3 fleet; `kernel8-test` is aarch64 and does not compile this file at
all. The x86 legs prove the code type-checks and the witness is bounded; the placement
behaviour itself is metal-only, and is stated so rather than dressed in a gate that cannot
fail.

### The steal half, ported to the Pi (aarch64, VUGSPREAD-PI)

`exec-vugspread`, closing PARITY.md §6.6c. The full write-up — including the piece-by-piece verdict
on each half of x86's VUGSPREAD and the corrected reading of the metal capture the arc was opened on
— is [PARITY.md §6.7](../08_VIDEO/PARITY.md). What belongs here is the scheduler mechanics.

**The gap was not "the Pi has a worse scheduler".** For PLACEMENT the Pi has a considerably more
elaborate one: SPREAD-3…SPREAD-14 above have no x86 twin. What it lacked is the correction x86 does
with `try_steal`, and the reason the two are not interchangeable is one sentence: **`make_ready` →
`rewake_place` can only correct a task that SLEEPS.** SPREAD-6's escapement already saw part of this
and put micro-park wakes on a 250 ms re-ask clock, but a thread that is genuinely CPU-bound between
presents — a vug worker ray-tracing its share of the shard — does not park at all, never reaches
`make_ready`, and its spawn-time core was therefore permanent.

**Three changes, and the third is the one a port would drop.**

1. **`spawn_user_thread` is `steal_ok = true`.** A ring-3 `place` argument is a locality HINT in the
   only vocabulary `SYS_THREAD_SPAWN` has; marking the resulting core a PIN promoted it into a kernel
   guarantee. The old justification — "EL0/slot tasks carry per-core TTBR0/ASID state" — had been
   false since SPREAD-4, which MOVES parked EL0 tasks between cores and writes out why that is sound
   (`user_ttbr0` is a value the task carries, `dispatch_next` installs it on whichever core runs the
   task, the old core's residual root is benign because slot L1s are a static array and teardown
   broadcasts `tlbi aside1is`). EL0 **slots** stay pinned deliberately: a windowed app parks on input
   every frame, so the rewake lane genuinely serves it.

2. **The floor is per-victim** (`sched_spread::steal_floor`), not the constant `STEAL_MIN_DEPTH = 2`.
   A run queue holds only READY tasks — the running task is in `current` — so a flat floor of 2 needs
   THREE runnable tasks before a core reads as loaded, and two-on-one packing sits at depth ONE.
   Victim running ⇒ floor 1; victim between tasks ⇒ floor 2, which is the ping-pong case the constant
   was actually reaching for. The floor is re-asked under the victim's own lock, not carried from the
   peek: the victim may have gone idle in between, and then the stricter floor is the right one.

3. **The SPREAD accounting is carried across the move.** `try_steal` retargets `task.cpu`; on x86
   that is the whole re-home. Here it is not. SPREAD-3's `EL0_RESIDENTS` and SPREAD-10's
   `SLOT_CORE_RES` are commitments released at exit **against `task.cpu`**, so a stolen EL0 thread
   whose credit stayed behind would grow the old core's count without bound and saturate the new
   core's at zero — and every subsequent `pick_cpu_slot` and `rewake_place` decision would steer
   around load that is not there, permanently. Both credits transfer at re-home in `make_ready`'s
   order (resident, then slot), and `place_cyc` is re-stamped because a steal IS a placement decision
   — without that, the task's next wake finds a stale refresh clock and re-asks immediately, fighting
   the move just made. `EL0_PARKED` needs no transfer: a task in a run queue is READY by construction.

**The brake is not optional.** VUGSPREAD-COOL (per-task `migrations` + `migrate_ms`, window
`16 ms << min(migrations, 4)`) is what makes the relaxed floor safe, and it is the half that matters
on an IDLE machine: with the floor at 1 there is almost always something to take, so without the
brake two empty cores trade one task forever. `migrate_ms == 0` always clears, so the FIRST corrective
steal — the repair itself — is never delayed; only re-steals are damped.

**What moved to shared code.** The policy above was `const`s and `fn`s inside `arch/x86_64/sched.rs`,
which is precisely why the Pi never had it. It is now `crates/kernel/src/sched_spread.rs`
(`STEAL_MIN_DEPTH`, `STEAL_COOLDOWN_MS`, `STEAL_COOLDOWN_ESC_CAP`, `steal_floor`, `steal_cooldown_ms`,
`steal_cooled`), with no `cfg(target_arch)` in the file. Both schedulers call in; each keeps only what
is genuinely arch-bound (how you ask "is this core running something", where milliseconds come from).
x86 behaviour is unchanged — its constants became re-exports and its functions one-line delegations.

**Witness.** `[spread4]` gains `steal= d1= remig= cool= pack=` on the same line as `rewake=`/`stay=`,
because `rewake` and `steal` are two lanes of one question and a spinning worker can only appear in
the second. `d1` is the floor repair's own attribution — moves taken from a victim at locked depth 1,
i.e. the ones the constant would have refused. Health is `steal` stepping at convergence edges with
`remig` near zero; the revert criterion is `remig` tracking `steal` while `cool` also climbs, which
is a brake refusing and serving the same oscillation. Per-move lines read
`:: [smpbal] steal 'name' cX->cY (m=N) ::`.

**QEMU gates this one, and decisively.** `VUGSPREAD: floor test` stages exactly TWO steal-eligible
tasks on one core with the rest idle and requires them to run on >= 2 distinct cores. On the pre-arc
floor that reads `cores-used=1` on every boot, not occasionally — the leg is an A/B, not a sample.
(The `[spread4]` census itself still reads all-zero in the battery, for the reason its own comment
gives: the only site that emits it on raspi4b runs before any EL0 task exists. The numbers that matter
are metal.)

**FLAKEHUNT — the leg asserted the move but measured the sibling's host timeslice (`exec-flakehunt`,
off `5d7ff0c8`).** The claim above is right and the staging is right; the *observation* was not. Both
ends of the window were blind wall-clock guesses, and one of them was three milliseconds wide: the
leg slept 2 ms assuming `home` had picked the first task up, and the worker spun 5 ms, so the state
the corrector must SEE — `home` running task one with exactly one ready behind it — existed for the
~3 ms remainder. A sibling can only convict inside that remainder, and only while its own vCPU is on
a host CPU. `busy_delay_ms` is CNTPCT, which under QEMU advances with the HOST clock whether or not
the guest core is executing, so on a loaded host that 3 ms could pass with an idle sibling running no
instructions at all — `cores-used=1`, and a FAIL that convicted the host's run queue rather than this
kernel's. Two independent Pi seats measured it at roughly **one red in four runs** of an otherwise
117/117 suite.

The repair changes neither the staging nor the pass condition (still "the pair ran on >= 2 distinct
cores", still reachable only through `try_steal` at victim depth 1). It changes how the leg *looks*:

1. **The second spawn is gated on an observation, not a delay.** Task two is queued only once task
   one has been seen running. A lone ready task at depth 1 on an idle victim is below
   `STEAL_MIN_DEPTH` under *both* floors, so a set bit can only mean `home` dispatched it. This
   strictly **sharpens** the A/B the blind 2 ms could lose in the other direction — a `home` that had
   not yet dispatched left the queue at depth 2 with the victim idle, which the pre-arc constant
   floor also cleared, so a stubbed build could have gone green by being slow.
2. **The window is ~25 ms, not ~3 ms** (the worker's spin *is* the window).
3. **The attempt is retried up to 4 times, and the count is published as `tries=`.** A retry cannot
   manufacture a pass: the pre-arc floor declines identically on every attempt.

Line shape, `tries=` inserted after `depth=1` and keyed by the spec's REQUIRE so it cannot silently
disappear:

```
:: VUGSPREAD: floor test — tasks=2 cores-used=2 depth=1 tries=1 :: PASS ::
```

**Go-red proven by stubbing**, not asserted: with `try_steal`'s two `steal_floor(running)` calls
replaced by the pre-arc constant `STEAL_MIN_DEPTH`, the retried leg reports
`cores-used=1 depth=1 tries=4 :: FAIL` — all four attempts refused — and reds both the REQUIRE and
the FORBID (116/117 required, 2 forbidden hits). On metal a `tries=` that starts climbing is itself a
spread reading this leg would otherwise have reported as a clean PASS.

### SPREADTUNE — the brake was measuring the wrong clock (aarch64, `exec-spreadtune`)

`exec-spreadtune`, off `180375ec`. The tune the §6.8f ruling called for, taken on the mechanism
rather than on a constant. **No number in `sched_spread.rs` changes, and neither does the floor.**

**The evidence, and why it was read wrong.** Two metal sittings meet §6.7's revert criterion for the
relaxed floor on their face — PA42 boot5 `steal=1600 d1=1251 remig=1585 cool=68598` and PA44
`steal=1121 remig=1107 d1=879`, i.e. `remig/steal ≈ 0.99` with `cool` climbing beside it. That is
also the convicted mechanism behind the vug fps swing (25–41) the bench reports as jerky: the worker
migrates, warms, is remigrated, repeats.

**First question asked, because the brief demanded it: is the cooldown actually consulted?** Yes.
`steal_one` calls `sched_spread::steal_cooled` on every candidate and bumps `cool` on every refusal;
the brake is wired and firing. The leak is elsewhere, and it is structural.

**The convicted defect: the governed population has TWO movers, and the brake only ever saw one.**
VUGSPREAD-PI's own change #1 made `spawn_user_thread` mark an EL0 thread `steal_ok = true`. Such a
thread also has `user_entry != 0`, so it is simultaneously:

* **steal-eligible** — `try_steal` may move it, and stamps `migrate_ms` when it does; and
* **an EL0 task** — so it also travels `make_ready`'s SPREAD-4 rewake lane, which retargeted
  `task.cpu` and stamped **nothing**.

So the residency window the brake enforces was measured from the last **steal**, never from the last
**move**. A worker stolen onto c1, rewake-placed back onto c2 five milliseconds later, then stolen
again the instant its stale steal-stamp aged out, pays three migrations while the brake accounts for
one residency. The two correctors were overruling each other inside the very window the brake exists
to guarantee, and neither could see the other. Before VUGSPREAD-PI the populations were disjoint
(every EL0 task was `steal_ok = false`), which is exactly why the brake was sound when written and
stopped being sound when the steal half landed.

**And `remig` cannot falsify anything on its own.** It counts steals of a task that has *ever* moved.
Once every steal-eligible worker has migrated once — which a small, long-lived worker pool reaches in
the first second — `remig/steal → 1` is arithmetically forced no matter how strong the brake is. The
ratio is a saturation artefact, not a measurement. `cool` compounds this: `steal_one`'s `position`
closure bumps it once per *cooled candidate inspected per scan*, over millions of idle passes, so
`cool = 42 × steal` says nothing about suppression pressure either. The revert criterion was built on
two fields that cannot distinguish a working brake from a broken one.

**The tune: both movers arm one clock.** `make_ready`'s rewake branch now calls `note_migration` and
stamps `task.migrate_ms`, gated on `task.steal_ok` — exactly the population `steal_cooled` governs (a
pinned EL0 slot task can never be stolen, so stamping it would only pollute the new statistics). A
task just placed anywhere, by either lane, owns its new core for its cooldown window before the other
lane may take it back. That is the residency contract `steal_one`'s doc-comment already claimed;
it is now true for the whole move population instead of half of it.

`migrations` is deliberately **not** bumped by a rewake move. The escalation ladder counts steal-lane
corrections of one task; a rewake move is a different corrector's decision, with its own escapement at
`PLACE_REFRESH_MS`/`REWAKE_MIN_PARK_MS`. A rewake move therefore can never drive a task to the 256 ms
terminal window on its own.

**The SMP-placement invariant is held, by construction.** The floor is untouched, no core becomes less
visible as a victim, and `d1` — the floor repair's own attribution, 879–1251 on metal — is unaffected.
More to the point, the change **cannot reach the population VUGSPREAD-PI was built for**: a worker
that spins flat out never parks, never enters `make_ready`, and behaves identically. The tasks the new
stamp governs are precisely the ones that already had a corrector.

**The price, stated.** A task whose *first* move came from the rewake lane now waits one base window —
16 ms, a single frame — before a corrective steal may take it, where before it was immediately
eligible. The "first corrective steal is never delayed" property is preserved for a task that has
never moved at all (`migrate_ms == 0`), which is the case the property was written for.

**x86 is not changed** (lane discipline: this seat owns `arch/aarch64`). `arch/x86_64/sched.rs` has
the same two-mover shape and the same leak; `sched_spread.rs` is untouched by this arc precisely so
that the rmbp seat's decision stays open.

#### The wire test

`[spread4]` gains four raw words after `pack=`, and they replace `remig`/`cool` as the reading:

```
… steal= d1= remig= cool= pack= rwstamp= residn= residmin=Nms residavg=Nms
```

* `rwstamp` — rewake-lane moves that armed the brake, i.e. **the size of the closed leak**. `rwstamp`
  comparable to `steal` is the direct measurement that the brake was seeing roughly half this
  population's moves.
* `residn` / `residmin` / `residavg` — residencies **ended across both movers**, in milliseconds: the
  ping-pong period itself, as a duration. `residn` is the sample count that makes the other two
  legible; a `residn` of zero prints `residmin=0ms residavg=0ms` rather than a sentinel.
* **Reading rule.** `residavg` well above `STEAL_COOLDOWN_MS` with `residmin` at or above it is a
  fleet that settles. `residavg` collapsing toward single-digit milliseconds while `steal` climbs is
  the ping-pong — now visible as a duration rather than inferred from a ratio that cannot fall. A
  rewake-lane move may legitimately end a short residency (its own escapement bounds it, not this
  one), so read `residmin` *with* `rwstamp`.

**The metal falsifier, and QEMU's honest limit.** The verdict is the next metal boot: `[vugfps] wf`
spread narrowing, with `residavg` risen and `[spread4]`'s shape changed. **QEMU raspi4b cannot supply
it** — no Group-1 IRQs or IPIs, so no real SMP contention shape — and on that host the `[spread4]`
census reads all-zero for the reason the VUGSPREAD-PI section above already records: the only site
that emits it on raspi4b runs before any EL0 task exists. A QEMU capture therefore proves the fields
are wired and the fleet did not regress. It does not judge the tune.

**Knob-off behaviour is NOT byte-identical, and that is deliberate.** `try_steal` and `make_ready` are
unconditional scheduler paths — the spread machinery rides no knob; only its `[smpbal]`/`[spread4]`
per-move witness lines are `pi`-gated. A scheduler tune that only applied under a video knob would be
the wrong shape.

### STARVE1 — the permanent starvation of dsktp boot 8 was a WEDGE, and SPREADTUNE is exonerated (aarch64, `exec-starve`)

`exec-starve`, off `0d865227`. The brief sent this seat after a NEW permanent-starvation class in the
dsktp boot-8 metal capture (`~/unaos-bench/capture/pi4-pi1-b1/ttyACM0.log`, boot-8 window from raw
line 56087). The conviction was reachable from capture + source, and it lands somewhere the brief's
suspect list did not: **the scheduler is not the defect, and today's own change is not implicated.**

**The verdict, and the two lines that carry it.** The terminal run reads

```
[el0live] verdict=STARVED el0 runnable/parked/committed=1/0/1 last_disp=234780ms ... kill_offcpu=18 corrupt=0 nopark=0
[spread4] live c0=0/0 c1=0/0 c2=0/0 c3=1/1 ... steal=19 d1=11 remig=6 cool=6 rwstamp=0 residn=6
```

(boot-8-window lines 2533 and 2519). **The brief's premise that `[spread4]` reports all four cores at
zero is a misreading of a single transitional line.** For the whole 250 s terminal run `[spread4]`
reports **`c3=1/1`** — one committed EL0 resident on core 3, unparked. There is no census divergence
to explain: `[el0live]` counts `el0_active` summed over cores and `[spread4]` prints the same
`el0_active` per core, so `runnable=1` and `c3=1/1` are the SAME word, and they agree. The task is
exactly where the scheduler thinks it is.

**What is wrong is core 3.** Every one of boot 8's twenty-one `[spin1]` prints — from `span=14874ms`
(line 1588) to `span=274869ms` (line 2517) — is **byte-identical apart from the span field**:

```
[spin1] cpu=3 span=…ms task=99:input state=1 park=0 | … | disp busy=197 idle=2663 | irq total=3081 last=30 unhandled=0 … | sched phase=6 passes=2860 futex_stalls=0
```

`sched passes=2860` frozen, `irq total=3081` frozen, `disp busy/idle` frozen, across 260 seconds.
Phase 6 is `SPIN8_TASK` — *switched into a task*. Core 3 entered task `99:input` and never completed
another scheduler pass, and took no interrupt, not even its own timer. That is the SPIN-1/SPIN-7/
SPIN-8 wedge signature exactly as those instruments were built to read it.

**Why the wedge is TERMINAL for the EL0 task, which is the genuinely new part.** An EL0 *slot* task is
created `steal_ok: false` (`arch/aarch64/sched.rs`, in `spawn_user_inner`'s `Task` initialiser — "left pinned
deliberately — VUGSPREAD releases THREADS, not slots"). `RunQueue::steal_one` skips every task whose
`steal_ok` is false. So the one mechanism that could rescue a runnable task from a dead core —
an idle sibling's `try_steal` — is by construction forbidden from touching it. While other EL0 tasks
lived, EL0 kept being dispatched on the surviving cores and the wedge was invisible. The operator's
vug open/close churn (`kill_oncpu=4 kill_offcpu=18`, exits 67→68) reaped them one at a time until the
**last survivor happened to be the one pinned to the dead core**, and at that instant EL0 stopped
forever. The kill churn is the *selector*, not the cause; the wedge predates it by ~250 s.

**SPREADTUNE (`cb628006`) is exonerated by the capture itself, not by argument.** Its entire new code
path is the `task.steal_ok`-gated stamp in `make_ready`'s rewake branch, and its wire field for that
path reads **`rwstamp=0`** on every `[spread4]` line in boot 8. The added code never executed. It also
could not have reached this task even in principle: the starved task is `steal_ok: false`, which is
the gate's own false arm. Suspect #1 is dismissed on evidence.

**Suspects #2 and #3 are likewise not reached.** `corrupt=0 nopark=0` for the whole run says the reap
path lost no task and left none unparked, and `DRAIN RESCUED (freed=1 pending=0)` fired earlier in the
same boot, so the retire/drain path demonstrably works. The task was not lost from a queue — the
census says it is still on core 3, which is precisely the problem.

**Root cause of the wedge itself is OUT OF THIS LANE and remains open.** Why `99:input` stops
returning with interrupts masked is a question for the input/serial-RX backstop path, not
`arch/aarch64/sched.rs`. This arc does not guess at it.

**The USB-storage EL0 starvation near boot (`rdcap=14562ms`, spin+hlt pump) is a DIFFERENT class** and
is not conflated here: it was transient and recovered, whereas this one never does.

#### The instrument: `[starve1]`

Rather than build an unvalidatable evacuation path into the scheduler on the strength of one capture,
this arc ships the discriminator that names the mechanism on the *next* boot, on one line, without a
source audit. `starve1_probe` is called from the tail of `el0live_witness` with the current verdict.

**It is a DELTA, not a snapshot** — the necessary design point. A frozen `passes` value proves nothing
read once, so the probe **arms** on the first STARVED window (snapshotting `SCHED_PASSES` and
`gic::IRQ_TOTAL` for every core) and **fires** on the second, printing the pair across the interval:

```
[starve1] episode=N cpu=3 act=1 res=1 phase=6 passes=2860->2860 irq=3081->3081 cur=99:input — WEDGE: …
[starve1] episode=N cores_named=1 verdict=WEDGE | machine steal= d1= cool= rwstamp= pack= — …
```

**The reading rule:**

* `passes=X->X` (frozen) on the core holding the runnable resident → **WEDGE**. The placement lane is
  blameless; go where the core died. `cur=` names the task that stopped returning.
* `passes=X->Y` (advancing) and the resident still not dispatched → **PLACEMENT**. The core is alive
  and refusing; the spread/run-queue/affinity lane has a case to answer, and the machine-wide
  `steal=/cool=/rwstamp=` on the second line are on the same wire to convict or clear it without
  hunting a nearby `[spread4]`.
* `cores_named=0` → **CENSUS-SPLIT**: `runnable>0` machine-wide while no core reports a runnable
  resident. That would be a third finding and a bug in the census itself.

**One line per episode.** The latch disarms the instant the verdict leaves STARVED, so a 250 s freeze
costs two lines, not the ~170 that a per-window print would have produced. `[el0live]` keeps printing
every window — it is the clock; `[starve1]` is the cause, stated once.

**Cost and safety.** Two relaxed loads per core per STARVED window, three relaxed stores per episode.
Nothing on a dispatch, wake, or steal path is touched. **No lock is taken** — a witness that can block
on the scheduler's own lock can hang the machine it is diagnosing — and the one `current` deref
follows `[spin1]`'s established rule (a task still current across two witness windows is not
mid-drop). Not knob-gated: it rides `el0live_witness`, which is unconditional.

**QEMU's honest limit, stated.** raspi4b **cannot reproduce this class and cannot judge the
instrument's verdict.** It has no Group-1 IRQs or IPIs, no real SMP contention shape, and its
`[spread4]`/`[el0live]` emit sites run before any EL0 task exists — so the census reads all-zero, the
STARVED verdict never fires, and `starve1_probe` never leaves `STARVE1_IDLE`. QEMU proves the code
compiles and the strings are linked into the image (`strings` on `kernel8.img`); it proves nothing
else. **The verdict is the next metal boot that starves.** If that boot prints `verdict=WEDGE` with a
frozen `passes` pair, this reading is confirmed and the next arc belongs in the input path; if it
prints `verdict=PLACEMENT`, this reading is falsified and the scheduler is back under suspicion.

### INWEDGE — what the wedge under STARVE1 actually was: the input router held the panel lock across a dispatch (aarch64, `exec-inwedge`)

STARVE1 above ends by naming the next question: *if the wedge is real, the next arc belongs in the
input path.* This is that arc, and the answer is yes — with a mechanism, a fix and a go-red.

**The prior was refuted first.** The suspect going in was the WHEEL/WHEELZOOM routing that landed
the same day: a focused vug dying mid-scroll, the input task delivering a wheel detent into a ring
whose owner was being torn down. The boot-8 wire refuses it outright, and does so three times over:

* `[wheel1]` — the wheel census, edge-triggered on `decoded + routed + nofocus` — **never printed**,
  in boot 8 or anywhere else in `pi4-pi1-b1/ttyACM0.log`. Not one wheel byte was decoded, routed, or
  dropped for want of focus.
* `[piusb26]` — the xHCI pump's cost line, rate-limited to one print per 5 s — printed **exactly
  once** in boot 8, at byte 6501 of the boot, and never again. `usb-pump` is spawned on `input_cpu`,
  the same core the wedge took: the pump made one pass and died with the core. After that instant no
  HID report of any kind — motion, button or wheel — was pumped at all.
* No `[piusb24]`, no `[el0in]`, no `[cursor] armed`, no `[vugzoom]`. Every `[clickroute]`, `[wm-act]`
  and `[drag]` line in the freeze window carries `settle=noproc-selftest` or `dragperf`: synthetic
  fixture events, not hardware.

Boot 8 never delivered a single real pointer report. There was no scroll to race with, and the
wheel arc is exonerated.

**What the capture does support.** `[spin1] cpu=3 task=99:input state=1 park=0 … sched phase=6
passes=2860` with `irq total=3081 last=30`, every counter byte-identical across 21 prints spanning
14.8 s to 465.3 s. Phase 6 is `SPIN8_TASK` — the core was inside a task, not inside its own scheduler
loop. INTID **30** is the generic-timer PPI: the last thing core 3 did was take a timer interrupt and
never leave it. A frozen IRQ total is the SPIN-7/SPIN-8 masked-spin signature. All three
already-witnessed masked spins read clean on that capture (`sem_stalls=0`, `futex_stalls=0`, no
`[wedge4] preempt-in-section` line), so the spin was on a raw `spin::Mutex` that none of
WEDGE-4/5/6 watches. Everything pinned to that core died with it: `rx-backstop` frozen at
`bs_phase=1 bs_loops=2`, `status-tick` and `usb-pump` never dispatched again.

**The interleaving, on one core.**

1. `usb-pump` (input core, `PRIO_SERVICE`) runs the FOCUS-VIS cursor keep-alive inside
   `route_input_to_active_el0`. It reads panel geometry out of `video::WRITER` (`pal_width_hint` /
   `pal_height_hint`) and repaints the sprite through `video::cursor::repaint`, whose `refresh_locked`
   takes `WRITER` again. All of it acquired **blocking, with interrupts enabled**, from a preemptible
   kernel task.
2. The quantum tick lands inside that section. `timer_preempt` marks the task READY and switches to
   the scheduler, which requeues it. The task is now off-CPU **still holding `WRITER`**.
3. The core dispatches its band peer, `input`.
4. The next timer tick lands with `input` current. `timer_preempt` calls `load_witness_tick`, whose
   emit is a `serial_println!` — and that print's panel mirror takes `WRITER` with a **blocking**
   acquire, inside the IRQ vector, masked.

   > **Correction (LOCKFIX).** This step originally named `video::fbcon::_print` as the blocking
   > acquirer. That is wrong: `fbcon::_print` takes `FBCON.`**`try_lock`** (`video/fbcon.rs:1244`)
   > and `serial::_print` takes `SERIAL_PORT.`**`try_lock`** — neither can block, so neither can be
   > the spinner. The blocking `WRITER.lock()` on the print path is in **`wm::composite`**
   > (`video/wm.rs:4622`), reached through `route_present_rows` → `wm::present` when the panel
   > console owns the glass, i.e. on `pidesk`/`wc` builds. Nothing about the mechanism, the
   > interleaving or the fix changes — only the callee's name, and a wrong name sends the next
   > reader to a file that is already safe. `main.rs`'s copy of this narrative carries the same
   > correction.
5. The acquire never returns. The core is masked, so the holder it displaced can never be
   redispatched to release the lock, and `SCHED[3].current` still names the interrupted task —
   `99:input`. That is the name on the `[spin1]` line, and it is not the culprit; it is the tenant.

**The rule, stated once.** *The input router may not hold a raw panel lock across a dispatch, and may
not block on one.* Every other raw-spinlock section on this arch already obeys it — `sched::rq`
masks, `Semaphore`/`FutexBucket` mask, `shell_inbox` masks, the global allocator masks. The input
router was the exception, and it is the one task set that shares a core with the kernel's only
IRQ-context printer.

**The fix (two parts, both in `main.rs`'s router).**

* `pal_width_hint` / `pal_height_hint` now go through `inwedge_panel_info`: **masked** (the section
  cannot be preempted) and **`try_lock`** (the router can never itself be the spinner). A refused read
  yields 0, which is the clamp-to-(0,0) degradation the FOCUS-VIS doc already sanctioned for an unset
  framebuffer.
* The whole keep-alive `match` runs inside `arch::without_interrupts`. Making only the two geometry
  reads safe would leave `cursor::repaint`'s own `WRITER` hold — the wider one — still preemptible on
  the input core, i.e. would leave the wedge where boot 8 found it. The masked span is bounded and
  small (two `info()` reads and one sprite restore+draw with its cache clean) and runs at most once
  per HID report on a ~250 Hz pump.

**The witness.** `[inwedge] panel-lock read=N refused=M` — edge-triggered on `refused`, so it is
silent on every boot that never contended (which is every automated gate). A **nonzero `refused` is
the boot-8 window entered and survived**: before this arc that same instant was a blocking acquire
from a preemptible task on the input core. It is the recurrence witness the arc owes, and it names
the wedge rather than leaving a reader to cross-reference `[spin1]`, `[spread4]` and `[el0live]`.

**Go-red.** `inwedge_selftest` holds `video::WRITER` and drives the router's own geometry read from
the same core. With the fix: `:: INWEDGE: router panel-lock — held: w=0 h=0 refused+2 | released:
w=640 real=640 read+1 :: PASS ::`, and `./arroyo kernel8-test 210` reaches 117/117 with 0 forbidden
(23833 lines). With `pal_width_hint` reverted to its pre-arc `WRITER.lock()` and nothing else
changed, the same 210 s window **stops the boot at 337 lines, 40/117 witnesses** — the deadlock, on a
QEMU that can deliver no HID at all. The metal capture needed a pointer report and a timer tick to
line up; the leg needs neither.

**Out of lane, reported not touched.** `video::cursor::refresh_locked` still takes `WRITER` with a
blocking acquire, and `arch::poll_input` still takes `SERIAL_PORT` blocking with interrupts enabled
from the preemptible `input` task (`arch/aarch64/serial.rs` names that hazard in its own comment).
Both are now covered on the input path — the first by the masked section above, the second by the
fact that its only masked counterparty (`sys_write`, `syscall.rs:7238`) holds `SERIAL_PORT` bounded —
but neither has been made safe at its own site. The general rule wants a `WRITER` discipline in the
video lane and a non-blocking `poll_input` in the syscall/serial lane.

### LOCKFIX — the rule, held everywhere it applies (aarch64 + x86, `exec-lockfix`)

INWEDGE stated the rule and fixed one pair of call sites. A landing panel then held the trunk on the
two things that statement left standing, and this arc is exactly that repair: **one door for the
input path, and a mask that contains nothing forbidden.**

**What was still wrong.**

* **The masked span was not safe to run masked.** `cursor::repaint`, called from inside
  `without_interrupts`, took `WRITER` with a **blocking** acquire three times over (`undraw_locked`,
  `draw_locked`, `refresh_locked`'s flush) and printed two `[cursor]` witness lines from in there.
  So the mask that stopped the router being *preempted* while holding turned it into the *spinner*
  instead: on a `pidesk` build the print mirrors to the panel through `wm::present` → `composite`,
  whose `WRITER.lock()` is blocking, taken masked. A recoverable stall became an unrecoverable
  wedge, live on every `pidesk` boot — the same interleaving as boot 8 with the roles exchanged.
* **Two more input-path call sites were never converted.** `quarry::live::wheel_route` (the QSCROLL
  seam, reached from `usb-pump` → `user_input_enqueue` → `key_route`) and
  `arch::aarch64::syscall::click_pointer_pos` both read the panel with a plain `WRITER.lock()` on
  the same preemptible band the rule is about.

**The fix, one shape.**

* **One door, in the module that owns the lock.** `video::panel_info_nonblocking()` — masked,
  `try_lock`, counted — is now the *only* way the input path reads panel geometry. Its census
  (`video::panel_census`) is the one `[inwedge]` reports; `main.rs`'s `inwedge_panel_info` /
  `inwedge_census` are thin wrappers over it, so the witness and the selftest read as before.
  Callers: the router's `pal_width_hint`/`pal_height_hint` (clamp to 0), `wheel_route` (**declines**
  the event — a wheel it cannot place is not consumed), `click_pointer_pos` (the (0,0) clamp an
  unset framebuffer has always given it). The `syscall.rs` change is line-neutral (PARITY §5.3).
* **`WEDGE-8`'s policy, extended from the sprite lock to the panel lock.** `video::panel_snapshot()`
  blocks when interrupts are ENABLED and `try_lock`s when they are MASKED — exactly what
  `cursor::claim_bounded` already does for `SPRITE`. Every `WRITER` acquisition in `video/cursor.rs`
  now goes through it, and every refusal takes the module's existing `owe_repaint` degradation, so
  the panel is one composite tail late and never wrong. `refresh_locked` additionally stops the pass
  when `sp.drawn` survives the undraw attempt: that flag is the refusal signal, and drawing on top
  of an arrow still on the glass would save the arrow as its own under-content.
* **The mask now contains pixels and atomics only.** `cursor::repaint` is split: `repaint_deferred()`
  is the pixel half (no blocking lock, no printing) and returns a `RepaintTail` the caller `finish()`es
  **outside** the mask — the two `[cursor]` lines and `repair`'s window-table damage. `repaint()`
  itself is unchanged for every other caller (`repaint_deferred().finish()`).
* **The go-red leg stops racing the APs.** `inwedge_selftest`'s released half asserted
  `read == read0 + 1` off a single non-blocking read, so any AP holding `WRITER` at that instant made
  a *correct* kernel print FAIL. It now retries the released read up to 64 times against the real
  geometry (`tries=` on the line records how many it took) and tolerates a concurrent refusal in the
  held half's delta (`>=`, not `==`). The held half's conviction is untouched: both reads taken
  against a lock this core holds must still come back clamped to 0.

**Witness shape**, unchanged apart from the new `tries=` field:
`:: INWEDGE: router panel-lock — held: w=0 h=0 refused+2 | released: w=640 real=640 read+1 tries=1
:: PASS ::`.

**Knob-off byte-identity moves, deliberately** (PARITY §5.3's §5.2 precedent): `main.rs` and
`video/cursor.rs` are compiled into the shipped `kernel8.img` and this is a correctness fix on a path
users run, not desktop furniture. `arch/aarch64/syscall.rs` is nonetheless kept line-neutral.

**Gate (2026-08-18, `exec-lockfix`).**

| gate | result |
| --- | --- |
| `./arroyo check` | green, both arches (rc=0) |
| `UNAOS_WC=1 UNAOS_PIDESK=1 UNAOS_QUARRY=1 ./arroyo check` | green (rc=0) |
| `./arroyo kernel8` | builds — `UnaOS-pi4-baremetal.img` 64M |
| `UNAOS_PIDESK=1 UNAOS_QUARRY=1 ./arroyo kernel8-test` | **117/117 required on all three runs** (8, 4, 8 forbidden), `:: INWEDGE: … tries=1 :: PASS ::` and `:: QUARRY: … +wheel … :: PASS ::` on every one |
| pre-arc control, `c4ee2280`, same host + knobs, four runs | 117/117 (3), 117/117 (4), **116/117 (10)**, 117/117 (6). The control is the one that dropped a required witness, and it produced the same classes: `[wc-g] COHER`/`RACE-BLIT`, `[dragperf] coalesced=0`, and `[wc-d] verify win=1 … -> FAIL` — the console-vs-compositor residue PARITY.md §6.9c assigns to `exec-shellport`'s pacing lane, and quarry.md §14.6's host-load lane. **No class appears armed that the control does not also produce, and the armed runs never lost a witness.** Honest note: this host is loaded (the runs are interleaved with other aarch64 QEMUs), which is why the arc is judged on paired runs rather than on an absolute count |
| **go-red retained** | `pal_width_hint` reverted to `WRITER.lock()` in a scratch build, nothing else changed: `✂️ MBENCH TRUNCATED — 40/117, 337 lines`. Byte-for-byte the same truncation INWEDGE recorded, so the leg still convicts a blocking acquire. Call site restored and re-verified |

### POSFIX — the last two blocking takes on the input path (aarch64 + x86, `exec-posfix`)

LOCKFIX closed the rule's `WRITER` population and, in the same breath, named two residuals of the
**same class** that it did not fix. This arc is exactly those two, and nothing else.

**Residual 1 — `pal::cursor::POS`, INWEDGE's interleaving with a different lock.** The cursor's
position `Mutex` had takers on both sides of the mask line: `move_rel`/`set_abs` reach `set_clamped`
from **inside** the router's masked keep-alive span, while `pos()` was a **blocking, unmasked,
preemptible** take from `quarry::live::wheel_route`, `syscall::click_pointer_pos` and the
compositor's hit-test helpers. That is boot 8 with the roles exchanged: a tick inside the unmasked
acquire parks the reader off-CPU holding `POS`, and the next masked pointer report on that core
spins on it forever.

**The fix is `sched::rq`'s, not LOCKFIX's, and the choice is a property of the object.** LOCKFIX gave
`WRITER` try-lock-and-decline because its critical section is a whole framebuffer handle held by
painters that copy megabytes — an unbounded wait, so refusing is the only bounded answer. `POS` is
the opposite: **both** of its critical sections are constant-time and call nothing (`pos` is a
`get_or_insert` of an `(i32, i32)`, `set_clamped` is a store of one). So **every taker is masked** —
no holder can be preempted, the hold is a handful of instructions, and a blocking acquire cannot
outlive a bounded spin. Mask-everywhere is also the only option available here: a decline has no
honest degradation for a position read (a cursor that does not move for this event, or a click
hit-tested at a stale point), and expressing one would have to change the eight `pos()` call sites in
`video/wm.rs` — a file this arc may not touch. The change is contained entirely in `pal.rs`; not one
call site moves. **The standing invariant** for anyone adding a taker is recorded on the static:
`POS` is acquired only inside `arch::without_interrupts`, and nothing but arithmetic runs inside it.

**Residual 2 — `quarry::live::open`'s panel read.** `open()` reads as boot furniture, but it has a
second caller and that one is an input event: the dock's pinned tile latches `request_open()` and
`service()` drains the latch from `syscall.rs`'s strip-press arm — the preemptible `usb-pump`/`input`
band. Its `WRITER.lock()` was blocking, on that band, in front of an allocation, a volume read and a
window mint. It now goes through `video::panel_info_nonblocking()`, **bounded-retried** (`PANEL_TRIES
= 64`, the bound LOCKFIX gave `inwedge_selftest`'s released read) rather than declined on the first
refusal: an open is a deliberate operator gesture where a lost event reads as a dead dock tile, and
panel geometry is static, so the answer a retry gets is the answer the first try wanted. Each try
masks and releases inside the door, so the holder can always run. If all 64 refuse, the request is
put **back** into `REOPEN` — the next `service()` pass reopens with no further operator action — and
one line says so: `[quarry] DECLINE reason=panel-busy tries=64 …`. A contended-but-successful open
prints `[quarry] open panel-contended tries=N panel=WxH`. What is deliberately not on the table is
proceeding on stale or zero geometry, which would size the window, the dock-strip check and the
surface allocation off a lie.

**Knob-off byte-identity is not claimed**, on PARITY.md §5.3's §5.2 precedent, exactly as LOCKFIX
did: `pal.rs` is compiled knob-off and this is a correctness fix on a path users run, not desktop
furniture. `video/quarry/live.rs` remains `feature = "quarry"`-only. `video/wm.rs` is untouched.

**Gate (2026-08-18, `exec-posfix`).**

| gate | result |
| --- | --- |
| `./arroyo check` | green, both arches (rc=0) |
| `UNAOS_WC=1 UNAOS_PIDESK=1 UNAOS_QUARRY=1 ./arroyo check` | green (rc=0) |
| `./arroyo kernel8` | builds — `UnaOS-pi4-baremetal.img` 64M |
| `UNAOS_PIDESK=1 UNAOS_QUARRY=1 ./arroyo kernel8-test 210`, 7 runs | 116, 117, 117, 117, 116, 115, **117**/117. `:: INWEDGE: … refused+2 … read+1 tries=1 :: PASS ::` and `:: QUARRY: … +wheel … :: PASS ::` on **all seven** — the arc's own subject never wavered |
| control, `7847ceea` (LOCKFIX tip), same host + knobs, 6 runs | 117, 117, 117, **116**, 117, 117. The control loses a required witness too, and loses it from the same lane |
| strings-proof | `reason=panel-busy` and `open panel-contended` each present once in the shipped `target/pi_baremetal/kernel8.img` — the new witnesses are reachable, not merely compiled |

**Honest reading of the forbidden hits.** Every one, armed and control, is in the host-load /
console-vs-compositor-residue lane PARITY.md §6.9c and quarry.md §14.6 already assign elsewhere:
`[wc-g] COHER`/`RACE-BLIT`, `[wc-d] verify … -> FAIL`, `[wc-h]`/`[wc-k] AT-RISK`, `[dragperf]
coalesced=0`, `[wc-c] side-by-side drawn=1`. The dropped required witnesses were `[wc-c] drawn=2`
(armed once, **control once**), `[wc-fv] focus-vis … raise=…/false` (armed once, not seen in six
control runs — a single occurrence of the same raise-composite-did-not-land shape, not reproduced in
six further armed runs), and, on the one 115/117 run, `VUGSPREAD … cores-used=1` plus `[wc-k]
TEAR-FREE` — pure host-CPU starvation, on a host running these QEMUs back to back. This host is
loaded, which is why the arc is judged on **paired** runs rather than an absolute count, and the
paired reading is that armed and control are indistinguishable outside the arc's own two witnesses,
which are green on every run.

### WEDGEPROBE — the interrupt/timer instrument behind a WEDGE conviction (aarch64, `exec-wedgeprobe`)

**The evidence that motivated it.** Three metal WEDGE convictions share one anatomy. dsktp boot 9:
core 3, task `99:input`. dsktp boot 10, episode 1: core 2, task `111:el0-fb` at ~4.5 s; episode 2:
core 2 again, task `117:el0-wcb`, after the core had RECOVERED (~11M scheduler passes elapsed
between episodes) and re-wedged — the failure is transient-then-recurring, not a one-way latch. In
all three, `[starve1]` printed `passes=N->N irq=M->M` for the wedged core: its scheduler passes
frozen AND its per-core acked-IRQ count frozen across the probe interval, while every sampled lock
counter was zero (`rx_ready locked=0`, `sem_stalls=0`, `futex_stalls=0`). The lock class is
refuted — POSFIX was aboard boot 10 and did not prevent it. The surviving hypothesis class:
**interrupt delivery dies on the wedged core** — no timer tick, so no preemption, so the pinned EL0
resident starves — while the current EL0 task keeps running at EL0 (the machine stays alive).

**What the probe is.** `wedgeprobe` (sched.rs), called from the `[starve1]` fire path only, whenever
any core is convicted WEDGE (frozen passes). Discipline per §INWEDGE/§POSFIX: read-only,
failure-path-only, no locks taken, no waits entered — relaxed atomics, `mrs`, and single volatile
MMIO reads. It prints:

* One `[wedgeprobe] … wedged_cpu=N …` line per convicted core: the cross-core-safe atomics that
  core's own IRQ path maintains — `gic::IRQ_TOTAL` / `IRQ_LAST_INTID` / `IRQ_UNHANDLED[_LAST]` and
  `percpu::cpu(N).ticks` / `.ipis`. There is **no last-irq timestamp** anywhere in the kernel
  (`[spin1]`'s `last=` is `IRQ_LAST_INTID`, an INTID — 30 is the timer PPI), so a frozen
  `irq_total` beside a live sibling IS the staleness evidence, and the line says so.
* One `[wedgeprobe] … probe_cpu=P …` line from the convicting core: `GICD_CTLR` (the one GLOBAL
  register — distributor enable), then the probe core's **banked** view — timer-PPI
  enable/pending/active bits (GICD_ISENABLER0/ISPENDR0/ISACTIVER0 low words are banked per CPU
  interface on GICv2), `GICC_CTLR/PMR/RPR/HPPIR` (the whole GICC frame is banked), and
  `CNTP_CTL_EL0` + `CVAL−now` (CNTP_* are per-core system registers). On the QEMU-virt v3 build the
  same fields come from this core's redistributor SGI frame + the ICC_* system registers
  (`gic::wedgeprobe_snapshot` dispatches like every other GIC entry point).

**The honest limitation, stated in the witness line itself.** The wedged core's banked GIC state and
its CNTP registers are **unreadable from any other core** — that is what "banked" means — and the
kernel has no run-on-core-N IPI sampling path (the only SGI is the resched kick; building one was
explicitly out of the arc's scope). So the probe-core line is a **contrast baseline** from a core
that is demonstrably still taking interrupts, not the wedged core's state. What it CAN settle: a
clear `GICD_CTLR` would convict a global distributor loss; a stuck `timer_active`/low `rpr` on a
future capture where the probe core is itself about to wedge would convict a lost EOI; and the
per-wedged-core counters separate "no interrupts acked at all" (frozen `irq_total`) from "interrupts
acked but ticks lost" (`irq_total` moving while `ticks` freezes — a dispatch, not delivery, fault).

**Gates (this arc).** `./arroyo check` green both arches; `./arroyo test-arm` clean;
`./arroyo kernel8-test` **117/117 required, 0 forbidden**; strings-proof: both `[wedgeprobe]`
witness texts present in the shipped `target/pi_baremetal/kernel8.img` (positive control:
`starve1` present in the same pass). The probe has not yet fired on metal — that is the next
conviction's capture, at an operator-attended arc boundary.

### Orphan-reaper wake on enqueue (aarch64, SCHED-4b)

**SCHED-4 sleep_ticks regression** (U11-reap FAIL, timer never ticks in QEMU) bisected and fixed by SCHED-4b (`d7631117`): semaphore wake on orphan enqueue — ~0% idle duty metal-confirmed (c2=0% P31b), U11-reap PASS restored.

**Spawn-site ordering (U11-REAP ORDERING FIX).** A second U11-reap FAIL, deterministic under
`UNAOS_PIDESK=1 UNAOS_PIRAST=1 ./arroyo kernel8-test 300` (107/108, byte-identical across runs), was
*not* in the reaper or in the fs code the fixture exercises — it was **where the reaper was spawned**.
The `spawn_auto("orphan-reaper", …)` call lived in the panel-service block, which runs **after**
`pi_rast_demo_maybe()`, while the whole EL0 fixture cascade (U11-reap included) is already running on
the APs. A teardown-orphaned chain queued by that fixture therefore waited on a service that did not
exist yet, and U11-reap's bounded 5 s CHECKPOINT-3 poll passed only when the raster demo happened to
finish inside it. The evidence is the log order: the reaper's `freed teardown-orphaned chain
@cluster N` line lands **before** the verdict at 4.5 s of demo (PASS, by half a second) and **316
lines after** it at 9.5 s (FAIL — then the chain is freed anyway, so nothing leaked). The
`UNAOS_VUGPAR=1` control "fixed" the failure for the same reason in reverse: it shortened the demo
back under the deadline.

The fix moves the spawn to the `start_aps` block, immediately after the APs are released and before
the fixture cascade — the first point in the boot at which a scheduled core exists, and the point at
which the reaper's own doc comment already claimed it was spawned. Placement policy is unchanged
(`spawn_auto`, load-balanced, SCHED-3b); at that point only the AP set is in `ONLINE_MASK`, so the
reaper can never be placed on the not-yet-scheduling BSP, and it blocks on `REAPER_SEM` immediately,
so an early spawn costs an idle task and no duty cycle. It is also no longer gated on
`framebuffer_addr != 0` — a panel-less boot has teardown orphans too. The general rule the FAIL
states: **the availability of a kernel service must not be a function of demo or panel composition.**
The fixture's FORBID and its 5 s bound were left untouched; they were measuring honestly.

### The idle-desktop core wedge (aarch64, SPIN-1…8 / WEDGE-4…6)

A reproducible single-core lockup on the Pi 4: open one window on an otherwise
idle desktop, use it, and roughly seven seconds after the scene settles core 3
goes to 100% and never dispatches again. It survived six instrumented
hypotheses, and the record of how each died is more useful than the list itself
— every one of them was killed by an instrument built specifically to convict
it, which is the only reason the search space ever closed.

**The signature.** `c3` at 100%, `svc=0`, the composite rate collapsing, and
serial dying at end-stage — so the witness must be caught early. The stalled
task reads `69:rx-backstop`, `state=1`, `bs_phase=1` (inside `sleep_ticks`),
with `bs_loops` frozen. Reproduction is deterministic to within one loop
iteration (`bs_loops` lands on 35 or 36).

**The ledger.** Each row is an instrument, not an argument.

| Hypothesis | Instrument | Verdict |
| --- | --- | --- |
| Run-queue lock deadlock | WEDGE-4 W4-B — bounded-spin stall witness inside the acquisition, printed through the lock-free UART seam | Refuted. Silent, and *reachably* silent (see below). |
| Semaphore raw lock | WEDGE-5 — the same witness on `Semaphore::lock_raw` | Refuted. `locked=0 stalls=0`. |
| A72 LL/SC false sharing | SPIN-3 — cache-line padding on the per-cpu accounting atomics | Refuted. Padding kept as hygiene: the A72 has no LSE, so every RMW is an LL/SC retry loop and neighbouring counters on one line really can starve each other. |
| Bad placement | SPREAD-11 — yield-path slot re-placement | Real but insufficient. Helped once (`ymoves=1`); the storm re-formed on two cores. |
| Bad placement, second look | SPREAD-12 — empty-core recruitment lane in `rewake_place` (`recruit`/`rstale`) | Diagnosis corrected: the declined moves were never *equal*-load. `REWAKE_MARGIN` — hysteresis sized for `n` vs `n−1` — was charged against `n` vs **zero**, so no lane could reach a 0% core. Fixed; metal verdict pending. `rstale` is also a new instrument on *this* hunt: an empty core failing the freshness gate is a wedged core seen from the placement path. A third field (`rdecl`) was cut before landing: its zero was structural, so its silence proved nothing — see the wire-signature section. |
| Bad placement, third look | SPREAD-13 — co-placement suspended while a core is spare, plus a spread lane (`spare`/`split`/`repack`) | PA3 confirmed SPREAD-12's lane fires (`recruit=81`) and that the load stayed `c1=99%` beside three 0% cores. The remaining hold is SPREAD-10's own asymmetry — siblings weighed *committed*, load weighed *runnable* — which makes a saturated co-resident triple read `home_act == 1` and so uncontended to every lane. Fixed by making co-placement conditional; metal verdict pending. `spare` is also a wedge-adjacent instrument: it is committed-empty **and** dispatch-fresh, so a wedged core cannot masquerade as spare capacity and suspend the policy fleet-wide. Two instrument caveats land with it, both documented at the counters: the suspension makes SPREAD-12's `recruit` a structural zero (read it only against `spare=0`), and the split/repack pair is bounded by the placement clock rather than structurally, so the two climbing together at ask rate is the documented 2-cycle rather than a falsification. |
| Corrupt parked frame | SPIN-6 — validate the saved SP against the task's own stack bounds at switch-in | Refuted. SP valid; no refusal ever printed. |
| Stale `current` pointer (core fine, witness lying) | SPIN-5 — a per-core dispatch heartbeat | Refuted, and inverted: the heartbeat is **frozen**, so the core's scheduler genuinely never runs again. |
| Interrupt storm (unhandled level SPI, EOI'd still asserted) | SPIN-7 — per-core IRQ accounting, incremented at the `GICC_IAR` read | Refuted. `total=5448 last=30 unhandled=0`, frozen across the whole span. |

**What the last row actually proves, and it is the load-bearing one.** A
*racing* total means a storm; a *frozen* total means the core is taking no
interrupts at all — not even its own timer. Those are opposite verdicts from
the same counter, which is why `unhandled` alone could never have settled it.
`last=30` is the timer PPI (`TIMER_INTID`), and the counter increments at
acknowledge, so the core's last act was taking a tick. Combined with a valid SP
and a frozen dispatch heartbeat, the core is executing with `PSTATE.I` masked
and is unreachable by IRQ and by IPI — measured at 38 s and counting.

**Reachability is a property of an instrument, not of the code it watches.**
WEDGE-4 ships two probes that print the same tag. W4-A (`preempt-in-section`)
reports from `timer_preempt`, so it needs a timer interrupt and **cannot fire on
a core that has masked interrupts**: its silence there is structural, and
carries no evidence. W4-B (`RQ STALL`) reports from inside the bounded-spin
acquisition through the lock-free UART seam, so it fires exactly in the state it
is meant to report on: its silence *is* evidence. Same subsystem, same log tag,
opposite evidential weight. Any probe added to a masked span must be checked
against this question before its silence is banked as a refutation.

**Where that leaves the search.** `run()`'s IRQ-masked span, which is short and
enumerable: `mask_irq` → `drain_due_sleepers` → `input_wait_backstop` →
`dispatch_next` up to its unmask or its switch-in. Every *unmasked* phase —
`try_steal`, the idle WFI, the pass accounting — is excluded a priori, because a
core parked there would still take its tick. Inside the masked span, the sleeper
list is per-core and only ever touched IRQ-masked by its own core, and the heap
lock is always taken inside `without_interrupts`, so neither can host a
cross-core stall.

**SPIN-8 — the core states its own position.** A per-core phase word (its own
cache line, per SPIN-3) is stored at each step of that span, and a per-core pass
counter ticks the loop top; `[spin1]` prints both, read from a healthy sibling.
A phase that does not move *while the pass counter also does not move*, across
prints seconds apart, is the wedge stated positively and names the statement —
with no FIQ, no GIC reconfiguration, and one relaxed store per step. Reading an
unmasked phase there would itself be a finding, since the frozen IRQ total says
it is impossible.

**WEDGE-6 — the last silent spin in that span.** `input_wait_backstop` runs on
every core on every scheduler pass with IRQ masked (VUGPAUSE-2) and calls
`futex_wake`, which scans every bucket and takes each one's raw lock — while
`futex_wait`'s `PARK_WAITQ` hand-off holds a bucket's lock **across a context
switch**, released by the scheduler in `park_blocked` rather than by the waiter.
A waiter whose core never reaches `park_blocked` leaves that bucket locked, and
every other core's scheduler loop then spins on it forever, IRQ-masked,
dispatching nothing and printing nothing. WEDGE-4 gave the run-queue lock a
voice and WEDGE-5 gave the semaphore one; `FutexBucket::lock_raw` was the last
unbounded spin in the masked span without one. It now uses W4-B's exact shape:
bounded spins, one lock-free line naming core/bucket/key, a counter on the
`[spin1]` line, then keep spinning — behaviour unchanged, the wedge merely
legible. `Condvar::lock_raw` remains unwitnessed by design: it is not reachable
from the masked span, so it cannot produce this signature.

**On the FIQ PC sampler.** The obvious next instrument — interrupt the core with
an FIQ, since `PSTATE.F` is a separate mask bit from `PSTATE.I`, and sample
`ELR`/`SPSR` — is a multi-commit arc rather than a one-liner, for two reasons
found by source audit. `PSTATE.F` is masked kernel-wide: boot enters Rust with
`SPSR_EL2 = 0x3c5` (D/A/I/F all set) and `enable_irq` clears only I and A, so
the *scheduler* context — which is where the wedged core is — has FIQ masked
too. And `__vec_fiq` is a halting fault dead-end, not a resumable handler, so an
FIQ that did land would kill the core it was sent to diagnose.

Both are confirmed on silicon by a line the boot prints for another reason
entirely: `:: AARCH64 boot diag: EL=1 CNTFRQ=54000000 Hz MMU=on DAIF(DAIF)=0b1111 ::`.
All four DAIF bits masked, and **`EL=1`** — the kernel has already dropped out of
EL2 by then, so a sampler must read `ELR_EL1`/`SPSR_EL1`. Any plan written
against `ELR_EL2` is a wasted build. There is also an
open GIC-side question: `GICD_IGROUPRn` and `GICC_CTLR.FIQEn` are Secure-only in
the GICv2 Non-secure view, so a Group-0/FIQ-routed SGI may not be reachable from
where this kernel runs at all, and the BCM2711 ARM-local per-core mailbox FIQ
path (which this tree does not map today) would be the alternative. If a PC
sample is ever taken, it must be printed beside whatever names the masking
window — a PC alone dates a symptom without naming a cause.

### Three EL0 deaths that look identical on the wire (aarch64, EL0-LIVE)

PA41's third Pi 4 metal boot froze with the desktop intact and every input dead.
The 2660-line capture (`boot3-inputdeath-tail.txt`) is unambiguous that EL0 had
stopped — `[prio]` totals `el0=2490629` **constant** across the whole tail,
`[wcn] passes=0`, `composites=0/s`, `[cursor3] offers=568 taken=0` (the cursor is
composited, so zero passes means a frozen arrow whatever the mouse does) — and it
cannot say **why**, because three completely different failures render the same:

| regime | what is true | the fix it demands |
| --- | --- | --- |
| **STARVED** | EL0 tasks are READY and never win a dispatch | priority / affinity |
| **STRANDED** | EL0 tasks exist, all PARKED, no wake arrives | a lost wakeup |
| **EXTINCT** | there are no EL0 tasks left | whatever killed them |

The capture is `EXTINCT`, and the evidence that says so was circumstantial and
only legible to a reader holding the source:

- `[spread4] live c0=0/0 c1=0/0 c2=0/0 c3=0/0` — zero **committed** EL0 residents
  on every core, beside `rewake=112 stay=2272 short=40357 refresh=389`, cumulative
  wake counters large enough to prove EL0 had certainly lived earlier in the boot.
- `[spread10] slots 1c=0 2c=0 3c+=0` — no live address-space slot either.
- Residency is released at exactly five sites (`exit`, `retire_killed`, SPIN-6's
  corrupt-switch refusal, `park_blocked`'s dead arm, and the balanced
  leave/enter of a placement move), and every one of the first four **destroys
  the task**. Residents `== 0` after a large wake history therefore means the
  fleet was reaped, not descheduled.
- Corroborating, and independent: `make_ready` is the sole EL0 wake funnel and
  unconditionally bumps one of `short` / `stay` / `rewake` for every EL0 wake.
  All three are byte-frozen for the whole tail — **zero EL0 wakes**. And
  `[prio] agedin=` falls to 0 once the storm's own tasks retire: the run queues
  hold no below-band ready task at all.

Contrast boot 2 of the same session, which froze differently:
`[spread4] live c0=0/1 c2=1/1 c3=1/1` with `[prio]` `el0` totals climbing
17 899 008 → 20 832 924. EL0 alive, running hard, screen dead. Same symptom to
the operator; opposite diagnosis.

`[el0live]` states the verdict outright instead of leaving it to be reconstructed:

```
[el0live] verdict=EXTINCT el0 runnable/parked/committed=0/0/0 last_disp=27756ms last_wake=27801ms stall>=2000ms | reaped exit=3 kill_oncpu=1 kill_offcpu=2 corrupt=0 nopark=0 | totals el0_disp=2490629
```

What it adds over the counters that already existed:

- **A clock.** `EL0_LAST_DISPATCH_CYC` / `EL0_LAST_WAKE_CYC` are per-core,
  cache-line padded (SPIN-3), and stamped from CNTPCT reads the dispatch and wake
  paths already take — no new `mrs` on either hot path. "EL0 has not run for N ms"
  becomes one field on one line, readable from a **truncated** tail, which is the
  material every metal freeze actually produces. `[prio] el0=` needs two lines and
  a subtraction.
- **The runnable/parked split**, so "not running" is qualified — the difference
  between `STARVED` and `STRANDED`. Read lock-free from the SPREAD-3/SPREAD-4
  counters; no run-queue lock on a witness path.
- **A reap ledger** — `EXTINCT` is useless without a cause. The four reap sites
  previously left no durable trace: `exit()` and `retire_killed` print nothing,
  and SPIN-6 / the dead park arm print one line each *at the instant they fire*,
  which a tail capture has by construction already scrolled past.
- **A fifth verdict, `LEAKED`.** `el0_active` saturates `committed - parked` at
  zero, so `[spread4]` renders `parked=3, committed=0` and `parked=0, committed=0`
  identically as `0/0` — an accounting leak and a dead fleet, pixel for pixel.
  `[el0live]` prints the raw parked count and ranks `LEAKED` above `EXTINCT`.

**The 2 s threshold is `syscall::TAKEOVER_STALE_SECS`, deliberately.** After two
seconds without a `SYS_INPUT_POLL`, `run_user_image` stops believing the focused
app is live-in-takeover, re-arms its deadline, and — if the quiet continues —
issues an **ASID-scoped `sched::kill`** against the whole address space. An EL0
outage past 2 s is therefore not a latency complaint; it is the precondition for
the shell to begin destroying the fleet. So `verdict=STARVED` on the wire is the
early warning for the `verdict=EXTINCT kill_offcpu=` that can follow it, and the
pair is what a future capture needs to convict PA41's death **event** itself —
which this capture, being entirely post-mortem, cannot.

**`EXTINCT` is not by itself a fault.** The shell runs foreground programs one at
a time, so every gap between two EL0 programs is a genuinely empty machine. What
convicts a freeze is `EXTINCT` that **persists** while the desktop is supposed to
be up: `[wcn] wins=` nonzero, the compositor still passing, and `last_disp`
climbing without bound. PA41 boot 3 held that state for 545 s.

A development build chained the line into `prio_witness`, whose third caller is
`render_service`'s `[sched6]` block, to get a real reading out of the raspi4b
battery. It worked — the battery walked `NONE → LIVE (2/1/3) → EXTINCT → LIVE →
EXTINCT`, ending `exit=68 kill_oncpu=4 kill_offcpu=19` with both kill arms
exercised, which is the functional proof that the clocks, the runnable/parked
split and the ledger all read correctly. **It was then removed**, because it put a
UART write on the render task's path and the next two battery runs came back
`[wc-h]`/`[wc-k] torn=1 → AT-RISK` after a 108/108 immediately before. Host load
(59) is the better explanation and `maxpresent_us=10583` was inside the 16.667 ms
frame budget — but a witness must not be able to perturb the thing it watches, and
the chain bought nothing on metal, where `load_witness_tick` already covers it at
the same cadence. The QEMU wiring proof stays with `load_accounting_witness`'s
`verdict=NONE … last_disp=--` baseline, exactly as `[spread4]` documents for
itself.

Emitted from the three sites the rest of the train uses. On the metal timer path
it is taken **before** `load_witness_tick`'s change-suppression, because a machine
whose EL0 fleet has just died is precisely a machine whose load has gone flat and
stopped changing; gating the liveness census on load movement would mute it exactly
when it is the only line worth having. It carries its own suppression instead:
silent while healthy and unchanged, printed every window while the verdict is not
`LIVE`. The raspi4b battery spawns no EL0, so the gate reads
`verdict=NONE … last_disp=--` — the honest baseline, and the proof that the clocks,
the residency reads and the ledger are wired.

---

### 2.x Kernel stack sizing — SPIN-6's three convictions, and what the refusal does NOT mean

Kernel task stacks are **individual heap allocations**, not a pool:
`alloc::vec![0u8; stack_bytes].into_boxed_slice()` (`sched.rs:3088`, and likewise
3233, 3320, 8494). Growing one allocation cannot shrink another. This matters
because SPIN-6's refusal text asks `(neighboring stack overflow?)` and that
suspect has now been **wrong three times running**.

**What the refusal actually proves.** `ctx_sp` is a field of the heap-allocated
`Task`, and SPIN-6 tests exactly that field (`sched.rs:4657-4660`). A neighbour
overflowing *into* this stack corrupts the parked **frame** — bytes inside
`[base, top)` — and leaves `ctx_sp` in range, so it cannot produce the line at
all. `ctx_sp < base` is reachable **only by the task's own SP**. Every SPIN-6
refusal to date has been self-overflow.

| task | observed | overrun | cure |
|---|---|---|---|
| `u7-launch` | `ctx_sp=0x20c9e70` vs `[0x20ca000,0x20ce000)` | 400 B | 32 KiB |
| `render` (boot 10, both flights) | `ctx_sp=0x2089f80` vs `[0x208a000,0x208e000)` | 128 B / 96 B | `RENDER_STACK_SIZE` |
| `usb-pump` (boot 11) | `ctx_sp=0x207df00` vs `[0x207e000,0x2082000)` | 256 B | `PUMP_PATH_STACK_SIZE` |

**The pump path.** Boot 11's fault was Peter's quarry close/reopen on glass.
`quarry::open()` — panel read, two VFS `read_dir` calls, surface allocation, a
full paint and `wm::create_at` — runs **synchronously at click-router depth** on
the input-drain task: `usb_pump` → `pump_usb_into_gui` → `wc_click_route`
(`syscall.rs:13731`, where `strip`/`dock::press_route` inline) → `quarry::service`
→ `open`. The dock latches the open rather than performing it, but the drain is
called on the same source line, one frame below — **the latch defers nothing**.
The boot-time open ran identical work on the BSP's 512 KiB boot stack, ~24 lines
before `usb-pump` exists, which is why only the reopen faulted.

The routing is nonetheless **correct and deliberate** (`dock.rs:929-932`): a
volume read belongs on the input-drain band, not on the compositor pass owner.
What was never done is sizing the task the design handed the work to. `input` is
sized with the same constant, because `input_service`'s poll-nap branch calls the
same `pump_usb_into_gui()` (`main.rs:4301`) and is the *only* caller under QEMU.

**⚠ `[u7stk]`'s `headroom` SATURATES — it cannot go negative.** The high-water
scan starts at `base` (`sched.rs:108-131`), so `hw <= len` and
`headroom = len - hw >= 0` always. A chain that ran 256 B past its floor and one
that stopped exactly at it print the identical `hw=16384 headroom=0`. Any stack
size chosen from a reading at or near `headroom=0` is chosen from a **saturated
instrument**, and every depth it reports is a *lower bound*. Sizing evidence must
say so. (The doc comment above the probe claimed the opposite until this arc.)

**The residual: SPIN-6 validates the pointer, not the frame.** This is why
boot 10 merely lost a task where boot 11 wedged a core. The overrunning task's
preemption frame lands in the slab *below* it, smashing that task's parked
callee-saved registers — including its saved `x30`. SPIN-6 refuses the
overflower (its `ctx_sp` is out of bounds) but **passes the victim**, whose
`ctx_sp` is an untouched heap field; `switch_context` then restores the smashed
`x30` and returns into it. Boot 11 landed at `ELR=0x1228` — below the kernel's
first byte (`pi-baremetal.ld` links at `0x80000`) — with `EC=0x00`/`FAR=0x0`,
i.e. the fetch *succeeded* and decoded as UNDEFINED, then `hlt_loop()`. The core
died holding the `input` router, which is the on-glass wedge. The fix shape is a
**redzone below each stack whose first touch faults**; it is not this arc —
§2.y below is that follow-up, and it revises one premise stated here.

`FORBID REFUSING corrupt switch-in: task=` and `FORBID AARCH64 EXCEPTION` both
already exist in `pi4-regression.spec` — replaying a metal capture against the
spec convicts this class without a new rule.

### 2.y REDZONE — the absorber below each stack and the sentinel above it (aarch64, `exec-redzone`)

§2.x closed on a residual: SPIN-6 validates the *pointer*, not the *frame*. This
section is that follow-up. It ships two guard spans per kernel stack, and it
**revises one premise of §2.x** — see "where the victim actually gets hit" below.

**1. Guard page or poison sentinel? The allocator decides, and it says sentinel.**
Kernel stacks come from `linked_list_allocator` 0.10.6 (`#[global_allocator]` at
`allocator.rs:83-84`) through `alloc::vec![0u8; N]`, i.e. `Layout { align: 1 }`.
The crate preserves `layout.align()` verbatim (`hole.rs:368-375`), and
`split_current` hands back the raw hole start whenever
`align_up(addr, align) == addr` — which is *always* true for align 1
(`hole.rs:88-91`). The only alignment ever enforced is `align_of::<Hole>()` =
**8 bytes**. The page-shaped bounds in the SPIN-6 captures are luck, not contract:
`usb-pump`'s `0x207e000` is only 8 KiB-aligned.

And the map has no page to take away. Pi 4 baremetal maps kernel RAM with **2 MiB
block descriptors** (`boot.rs:284-302`, `ram_block`); the heap at `0x0200_0000`
+64 MiB sits in L2 indices 16..39, all plain blocks. Exactly one 2 MiB block in
the system is broken down to 4 KiB leaves — the `USER_REGION` block (`boot.rs:268`),
which lives in `.bss`, not the heap. A true guard page therefore needs four pieces
that do not exist: a page-aligned stack allocator; a 2 MiB→L3 split of a *live,
SMP-shared* mapping with break-before-make; a broadcast TLBI; and mirroring of that
kernel-mapping edit into all 8 `SLOT_L1/L2/L3` copies, per the standing STOP
TRIPWIRE at `boot.rs:556-561`. There is no kernel-half 4 KiB permission or unmap
API at all — TCR sets `EPD1=1` (`boot.rs:157`) and every runtime page-table mutator
in `boot.rs` is user-window only.

**So this is a poison sentinel, and for the span it watches it is detection after
the fact, not prevention of the write.** Said plainly: nothing here traps on the
offending store. What it does do is make the store land somewhere harmless — which
is the part that actually mattered, and is argued next.

**Layout.** Every stack allocation grows by `STACK_REDZONE + STACK_HIGHGUARD`; the
*usable* span is byte-for-byte the size the caller asked for, so every sizing
decision STACKPOOL and SHELLUP measured is preserved unchanged:

```
[alloc,                    alloc+1024)          LOW REDZONE   — absorbs this task's OWN overrun
[alloc+1024,               top)                 usable stack  — exactly `stack_bytes`, the span SPIN-6 always policed
[top,                      top+512)             HIGH GUARD    — catches a NEIGHBOUR's overrun arriving from the slab above
```

Both spans are painted `GUARD_FILL = 0x5A` in `build_initial_frame`, the single
choke point all four allocation sites already call. The paint is **unconditional**
— not `witness`-gated — because a protection that only exists on witness builds
does not protect the media build, which is the build that dies at the bench.
`STACK_POISON` (`0xAB`, witness-only) is a different byte for a different job, and
`stk_probe` is retargeted at the *usable* span (`base + STACK_REDZONE`,
`len - REDZONE - HIGHGUARD`) so its `hw`/`headroom` readings stay directly
comparable with §2.x's table and its `:82-83` doc stays true verbatim.

**SPIN-6 is strictly strengthened, never weakened.** Its window becomes
`[alloc+REDZONE, top)` — the *same span in bytes* as before, just shifted up, with
the absorber underneath it instead of the neighbour's stack. Had the floor been
left at `alloc`, the redzone would have silently become 1 KiB of extra headroom and
SPIN-6 would have fired 1 KiB *later* than it does today. Keeping the floor at the
caller's declared bound is what makes the redzone containment rather than a raise.

**2. Where to check — and where the victim actually gets hit.** §2.x proposed
checking the victim's sentinel at switch-in. That is right, but only for a guard
*above* the stack, and the reason is a direction argument: task A overruns
**downward** past its own base into the slab below, so the first bytes it writes
there are the **highest** addresses of that slab — i.e. the victim's **top**, which
is exactly where a parked frame lives. A low redzone under the victim is never
touched by the overflower. Indeed the low redzone is **single-writer**: only its
owner's SP can reach it, so checking it at switch-in and at switch-out sample the
same fact, and switch-out samples it first. Hence one check at each boundary, each
watching a *different* span:

- **Switch-out** — the outgoing task's **LOW** redzone, at `dispatch_next`'s return
  from `switch_context`. That one site covers every switch-out path in the file
  (`yield_now`, `sleep_ticks`, `timer_preempt`, `exit` all switch to
  `scheduler_sp` and land there), and it names the overflower at the hop it
  offended.
- **Switch-in** — the incoming task's **HIGH** guard, inside SPIN-6's existing
  block. This is the direct fix for the blind spot: it is the only check that can
  see damage done *to* a parked task by somebody else.

**3. What happens on detection.** Graduated, because the two ends of a guard span
mean different things. `guard_state` returns bit 0 if the span's high end is no
longer fill (something crossed *into* it) and bit 1 if its low end is gone too (the
span was *traversed*). Both spans are entered from their high address, so one
encoding serves both.

| condition | meaning | action |
|---|---|---|
| LOW bit 0 | the task dipped below its floor; the absorber caught it | `[redzone] LOW-REDZONE entered` — **task continues**. Its parked frame is intact (SPIN-6 proves that separately) and no neighbour was reached. This is a *sizing* alarm, not a corruption proof; dropping a healthy task here would trade availability for nothing. |
| LOW bit 1 | the absorber was traversed — an escape is likely | `[redzone] LOW-REDZONE TRAVERSED`, naming the task to grow and warning that the slab below may already hold a smashed frame. |
| HIGH bit 0 | a neighbour crossed into this stack's slab; the 512 B guard absorbed it | `[redzone] HIGH-GUARD entered` — **task resumed**, frame provably untouched. |
| HIGH bit 1 | a neighbour traversed the guard and reached the parked frame | **refuse + drop**, through SPIN-6's existing arm. |

That last row is the answer to "what do you do with a smashed victim": **dropping
it is right and returning into it is wrong**, so it joins SPIN-6's refusal and
reuses that path verbatim — including the EL0 residency/`slot_res` bookkeeping and
the `EL0_REAPED.corrupt` counter, which a second bespoke drop path would have had
to duplicate and could have got wrong. The existing refusal is never bypassed or
loosened; the condition only gains a disjunct. Its message gains a `higuard={}`
field and changes `outside its stack` to `vs its stack`, because with the new arm
the old phrasing would assert something false on a high-guard refusal; the
`REFUSING corrupt switch-in: task=` substring that `pi4-regression.spec:1580`
forbids is preserved verbatim. The parenthetical `(neighboring stack overflow?)`
is gone on purpose — `higuard` now *answers* that question instead of asking it.
Both `[redzone]` report paths are capped at 16 lines each by their own counters, so
a chronically undersized task cannot become the next serial storm (STORM-R3). The
counter alone was **not** enough, and the first armed run proved it: a breached
guard is *persistent state*, not an event — nothing re-paints it — so the same task
re-reported on every subsequent switch and burned the whole cap on 16 identical
lines. Each side therefore also keeps a `GUARD_*_LAST` task-id, and a report is
emitted only when the offending id differs from the last one reported. Runs of the
same task collapse to one line; a *different* task still reports immediately.

**It found a fourth mis-sized task on its first armed run — and one SPIN-6 cannot
see.** `kernel8-test` prints
`[redzone] cpu=2 LOW-REDZONE entered task=114:shell-run`, with **zero** `[spin6]`
lines in the same boot. That combination is the whole point of the mechanism:
`shell-run` dips below its floor *transiently* and returns, so its parked `ctx_sp`
is always in range and SPIN-6 — which samples SP only at park time — is
structurally blind to it. Before this arc that dip landed in the neighbouring heap
slab silently; now it lands in the task's own absorber and says so. `shell-run` is
an EL0 task spawned through `run_user_image` (`arch/aarch64/syscall.rs:10438`,
`:10832`), so its EL1 kernel stack is the blanket `TASK_STACK_SIZE` that
`spawn_user_inner` hard-codes — there is no `spawn_user` variant that takes a size,
which is why the STACKPOOL/SHELLUP cure (a right-sized constant at the spawn site)
has no EL0 equivalent to apply. **Sizing it is out of this arc's lane** (`syscall.rs`,
or a new sized EL0 spawn entry point) and is left as the named follow-up, per
§2.x's diagnostic order: probe first, right-size second.

**4. Cost, and why it is not `witness`-gated.** Four `read_volatile` byte loads per
full switch cycle: two at switch-out (both ends of the low redzone), two at
switch-in (both ends of the high guard), plus two compares each and a
never-taken branch. **Byte** loads, not `u64` — the allocator guarantees only
8-byte alignment and only for the allocation *base*, so a word load at
`top + HIGHGUARD - 8` would carry an alignment precondition nothing here can
enforce. The dominant cost is not the loads but the **cache**: the guard bytes are
cold by construction, so this touches ~3–4 cold lines per switch. That is precisely
why the check is two bytes at each end rather than a scan of the span — scanning
1 KiB would drag 16 cold lines through L1 on every context switch, and the paint
already guarantees the interior is fill. Against a switch that already pays a
`TTBR0_EL1` write plus `ISB`, several atomics and a 176-byte register
save/restore, this is noise. It runs **always**, on the media build: the boot-11
class of fault appears on unattended metal boots, which is exactly the build a
witness gate would exclude, and the memory price is 1.5 KiB per task (~9% on a
16 KiB stack, ~60 KiB fleet-wide against a 48 MiB heap).

**Residual, stated honestly.** An overrun larger than `STACK_REDZONE + HIGHGUARD`
combined still escapes and still smashes a frame; the guards make that loud rather
than silent, but they do not stop it. All four recorded overruns — `u7-launch`
400 B, `render` 128 B / 96 B, `usb-pump` 256 B — fit inside the 1 KiB absorber with
2.5x margin, so for the entire observed fault population the victim is not merely
detected but **never created**. A write landing strictly inside a guard span without
touching either end byte is invisible to the fast path; such a write did not leave
the allocation, and SPIN-6's exact `ctx_sp` test still covers the parked case.
Overrun *magnitude* for a parked task comes from `base - ctx_sp` at the refusal
site, never from `[u7stk]`'s `headroom`, which saturates at 0 (§2.x).

**What QEMU can and cannot show — a prediction this arc got wrong, and the
correction.** The expectation going in was that neither `[redzone]` line could fire
under QEMU raspi4b "by construction", on the §2.x reasoning that no Group-1 timer
IRQ is delivered on these paths so no preemption frame ever lands. **That is true
of the boot-11 *death* and false of the *low redzone*.** The two are different
events. Banking a preemption frame below the floor needs the timer; simply
*running* below the floor needs only depth, and depth is architecture-independent —
so `kernel8-test` exercises the low redzone for real, and did, on `shell-run`. The
honest split is therefore:

- **Low redzone — LIVE under QEMU.** It is a depth instrument and the gate is a
  genuine test of it, not a smoke check.
- **High guard — inert under QEMU.** It can only fire once an overrun actually
  escapes an allocation, which on the recorded evidence takes a preemption frame
  the emulator never delivers. The QEMU legs prove it is non-regressive; they
  cannot prove it fires.

**What the next metal boot must print.** On a healthy boot: zero `[spin6]` lines,
zero `higuard=` refusals, and at most the known `shell-run` low-redzone line until
that task is sized. If the boot-11 chain recurs, the expected evidence is
`[redzone] cpu=N LOW-REDZONE entered task=<id>:usb-pump` (or `input`) *instead of*
the old `[spin6] … ctx_sp=0x207df00 outside its stack`, with the machine still
running and the panel still alive — the overrun absorbed and named rather than
banked into a neighbour. A `LOW-REDZONE TRAVERSED`, or any refusal carrying
`higuard=2`, means the 1 KiB absorber was not enough: that is the reading that
sends the sizes up again, and `higuard=2` is additionally the first direct
identification of a *victim* this kernel has ever been able to make.

### 2.z The `[u7stk]` gauge only reads where it is CALLED — the tegra reachability inventory and `stk_probe_bounds` (aarch64, orin 13, LEDGER S13)

The poison-scan gauge of §2.x is two functions and a macro, and a board only gets a reading at a
site that calls one of them from the task whose depth is wanted. This subsection records what the
Orin flights of 2026-09-01..05 established about reachability (`docs/dev/evidence/orin13/FLIGHT.md`
§A.1–A.5), and the commands that prove each point, so the finding is not re-derived.

**Fact 1 — the probe is `witness`-gated and task-scoped.** `sched::stk_probe(at)` (`sched.rs:90`)
exists only under `#[cfg(feature = "witness")]`; it reads `SCHED[cpu].current` and RETURNS with no
output when that is null — which is the boot core for the whole tegra terminus (`main.rs:2717`
onward), so a call placed on the terminus line prints nothing, whatever the build. Every caller must
carry its own `witness` gate, because the symbol does not exist on a knob-off image.

**Fact 2 — until orin 13 the probe had exactly one call site, and a tegra image could not reach it.**
At `ac27b8d2` the non-comment inventory was one line:

```
$ grep -rn 'stk_probe' unaos/crates/kernel/src --include='*.rs' | grep -v ':[[:space:]]*//'
unaos/crates/kernel/src/arch/aarch64/syscall.rs:16225:        crate::arch::sched::stk_probe($at)
```

— the body of the `u7stk!` macro, expanded sixty times inside one function, `syscall::u7_launcher`,
which is spawned only from the `kernel_main` body below `tegra_early_stop(boot_info) -> !`
(`main.rs:189-190`). Dead code on a jetson image regardless of `witness`, and the linker agrees: the
`[u7stk]` format piece is absent from the flown ELF, with a positive control to show the grep can hit.

```
$ grep -a -o 'u7stk'       ~/unaos-bench/flash/orin/render1-20260901T0347Z-c61b47e/kernel.elf | wc -l   # 0
$ grep -a -o 'RENDER-LIVE' ~/unaos-bench/flash/orin/render1-20260901T0347Z-c61b47e/kernel.elf | wc -l   # 1
```

The Pi, where `u7_launcher` runs, prints 52 `[u7stk]` lines on the same tree (PARITY.md §"THE
INSTRUMENT"). The prior belief "no `witness` ⇒ no `[u7stk]`" was therefore true and insufficient:
render1 was built WITH `witness` and carried no probe at all.

**Fact 3 — the witness that DID fire is the ungated one.** The dispatch-time low-redzone report
(`sched.rs:5482`, `[redzone] cpu=N LOW-REDZONE TRAVERSED task=<id>:<name> …`, capped at 16 by
`GUARD_LO_REPORTS`) is compiled into every image (§2.y). On render1 it printed 8 lines each for
`orin-render` and `jd2-console` and hit its cap — the 16 KiB `orin-render` stack was traversed before
any gauge existed to size it. Where a `[u7stk]` line is missing, `[redzone]` is the reading to look
for; its magnitude comes from `base - ctx_sp` at the refusal, never from a saturated `headroom=`.

**The per-task answer.** A task's depth needs its OWN probe call, from that task, after its deepest
call chain. Two shapes now exist in the tree (inventory at `6cc8de8c`, same command as Fact 2):

| site | shape | what it reads |
|---|---|---|
| `syscall.rs:16250` `u7stk!` | `stk_probe` inside `u7_launcher` | the Pi's launcher, 60 checkpoints |
| `main.rs:8340` (STACKSEED, `01739a93`) | `#[cfg(feature = "witness")] stk_probe("orin-render:pass1"/"pass2")` after the first two presents | `orin-render`, now spawned with `spawn_stack(…, 32 KiB)` (`main.rs:8199-8204`); render3b: `len=32768 used=1280 hw=13296 headroom=19472` |
| `main.rs:8459`, `:8471` (CASCADE, `a20839c6`) | `stk_probe_bounds(at, name, base, len)` — `stk_probe`'s scan and `[u7stk]` line over bounds the CALLER supplies (`sched.rs:10626`) | a stack no `Task` describes: the boot core's firmware stack across `desktop_firmware::activate`, over a poison window the caller paints below its own SP; prints `task=0:boot-core`. render3b: `pre-cascade hw=240`, `post-cascade hw=15472 headroom=17296` of a 32 KiB window |

`git show a20839c6 --stat` — `arch/aarch64/sched.rs | 46 +` is the whole of `stk_probe_bounds`
(hunk `@@ -10613,3 +10613,49 @@`, appended at the file tail so no existing line moves). The same
ELF grep on render3b's image now counts 3 `u7stk` tokens and 4 `boot-core` (control `RENDER-LIVE`
= 1). Both readings saturate exactly as §2.x says: `hw=len headroom=0` is a lower bound, and the
honest follow-up is a larger stack or a wider window on the next flight.

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
- **HID-REGRESS-B12 — a ticking AP must WFI, not busy-spin, when idle.** `arch::hlt`
  chooses WFI-park vs. poll-spin from `timer::is_live()`, which tracks the **boot
  core's** timer. On the Jetson the JM6 EL2->EL1 drop disables that timer
  (`set_not_live`), so post-drop `is_live()` is false and `hlt` poll-spins — correct
  for the timerless boot core, which owns the cooperative xHCI HID poll. But a JC3
  secondary has its OWN live tick, so post-drop it would fall into the same poll-spin
  branch and **busy-spin its `run()` steal loop** between ticks instead of parking.
  Five secondaries hammering the shared run-queue spinlocks/`ONLINE` atomics saturate
  the interconnect and starve the boot core's HID poll — armed keyboards/pointers
  deliver ZERO events (boot-12). Fix: `hlt` also parks on
  `timer::this_core_has_local_tick()` (this core's `AP_LOCAL_TICK` bit), so a
  self-ticking AP WFI-parks bounded to one ~4 ms tick — still self-scheduling, but
  idle-quiet, so input coexists. The boot core is never in `AP_LOCAL_TICK` (its
  poll-spin is unchanged); on QEMU `virt` the APs' tick is delivery-confirmed so
  `is_live()` is already true (the new term is redundant, no behaviour change); Pi
  (GICv2, no `AP_LOCAL_TICK`) reads false and is byte-identical.
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

## 4b. SIMMER — the per-core load animator

`simmer` (`arch/aarch64/sched.rs`) is a per-core load ANIMATOR: it makes each
online Orin core independently "breathe" like a moderately busy machine so the
`vug` per-core meter shows the bars rising and falling on independent rhythms.
Where `burst` is a one-shot balancer probe (migratable tasks that get *stolen*
across cores), `simmer` is a steady-state animator: one **PINNED** `PRIO_LOW`
task per core, each duty-cycling on its own cadence. Pinning is the point — each
core's bar is driven by its own animator, not by placement/stealing (`burst`
already proves stealing).

- **Toggle + default-quiet.** The `simmer` shell verb (inside `jd2_console_pump`,
  the boot core) toggles it; `simmer off` stops it. Nothing runs unless the verb
  is typed. `simmer_start`/`simmer_stop`/`simmer_active` back the verb; start and
  stop emit a single serial witness each (`:: SIMMER: staged N ... ::` /
  `:: SIMMER: stopped ::`) — the visual is the product, so there is no per-cycle
  spam.
- **Per-core rhythm.** Each animator seeds a small `xorshift32` from its core id
  (no wall-clock entropy) and redraws, every cycle, a period (~120–320 ms) and a
  duty (~15–70 %). The busy phase burns real work and `yield_now`s so higher-
  priority work preempts and every dispatch pass records the core BUSY; the idle
  phase `sleep_ticks` so the run queue drains and the meter reads the core IDLE —
  the down-stroke. Busy duration is bounded by the core's own `percpu.ticks`
  (its JC3 PPI clock) with a generous `cntpct` wall backstop.
- **Every online core EXCEPT the driver (boot) core is animated.** The boot core
  runs the cooperative loop, not `run()` — it neither drains its sleeper list nor
  (post-JM6, timer disabled) ticks, so a `sleep_ticks` there would park forever,
  breaking both the animation and a clean stop. This is also exactly the set `vug`
  displays as a scheduler busy-*fraction*: during `vug` the boot core renders (its
  dispatch counters freeze) and its bar reflects render load, while every other
  online core reads its honest busy fraction. On a fully-online Orin that is the
  boot core's render load plus five animated secondaries.
- **Clean stop.** Animators poll a shared `SIMMER_RUN` atomic and exit; a
  `SIMMER_LIVE` countdown lets `simmer_stop` wait (bounded, cooperatively
  yielding) for genuine quiescence before witnessing. A stop-ceiling emits an
  explicit warning rather than wedging.
- **Self-test (`simmer_test` / `UNAOS_SIMMER=1`).** A gated boot-core task stages
  the animators, samples the meter twice ~1 s apart and asserts multiple animated
  cores show BUSY deltas, then stops and asserts quiescence. Run:
  `UNAOS_GICV3=1 UNAOS_SIMMER=1 ./arroyo test-arm` — on QEMU `virt` (4 cores)
  three secondaries animate and the case reports PASS + quiescence PASS.

## 5. Status and limitations

- ~~**x86_64 only.** aarch64 runs a single polled core; it has no GIC-driven
  preemption or scheduler yet.~~ **Stale — retired.** aarch64 runs the full
  preemptive SMP scheduler on all four Pi 4 cores, GIC-driven off the per-core
  generic timer PPI (`TIMER_INTID = 30`), with work stealing, priority aging and
  EL0 tasks. Most of the sections above this one are aarch64 work.
- ~~**No priority inheritance** on `Mutex` (assessed as large/thorny under CPU
  pinning; deliberately deferred).~~ **Retired by R1 / rtpi (`UNAOS_RTPI=1`,
  x86_64).** The sleeping `Mutex` now runs a minimal, soft-RT **priority
  inheritance** protocol so a low-priority holder cannot be preempted by
  mid-priority tasks while a strictly-higher task blocks on the lock (bounded
  priority inversion). The redesign's core rule (see the "R1 / rtpi" block in
  `arch/x86_64/sched.rs`): donation state lives on the LONG-LIVED per-lock
  control block (`PiCtl`, embedded in the `'static`-by-contract `Mutex`), never
  on the transient `Task` — a `Task` pointer appears only as an IDENTITY TOKEN
  (compared, never dereferenced cross-CPU), so the donor-loads/holder-exits/
  donor-derefs use-after-free is structurally impossible. Three moves:
  (1) *acquire-time donation* — a blocker raises each lock's `boost` down the
  holder chain, propagated **transitively** via `PiCtl::owner_waits` (the
  holder's own published uplink), accelerating holders by identity
  (bump-if-running / relocate-if-ready); (2) *handoff inheritance* — the task
  that wins a contended acquire inherits the lock's standing `boost`, so a
  FIFO handoff to a lower waiter over a higher one does not reopen the
  inversion; (3) *revert on the last release* — the last waiter out resets the
  lock's `boost` (`nwait`-gated so a live donation is never destroyed),
  leak-free by construction. The boost is read by every placement/preemption
  site through `sched_prio` = max(base, max over held locks' `boost`), the
  per-task `held` set capped at `PI_HELD_MAX` (overflow degrades to a missed
  boost AND a severed transitive uplink for the dropped lock — bounded,
  documented at the constant). All per-task bookkeeping runs under
  `without_interrupts` so a task's self-identification (`pi_current`) cannot
  be preempted-and-stolen mid-read (the GR27 review's B1).
  Only the owner-tracked sleeping `Mutex` participates —
  the counting `Semaphore`, futex and the IRQ-masked `spin::Mutex`es are out of
  scope (no single owner / non-blocking / non-preemptible). **DEFAULT OFF**: the
  PI fields do not exist and `Mutex::lock` takes its original single-`wait()`
  path, so an unarmed build is byte-identical. Witness: the `[rtpi]` rollup
  (`inherits` / `max_jump` / `chain_max` / live `active` leak gauge) plus
  rate-limited `[rtpi] inherit …` traces, reading zero honestly when no
  inversion occurs (`src/rtpi.rs`).
- **APIC timer is uncalibrated** (~1 ms/tick on QEMU); a CPUID 0x15 / TSC-
  deadline calibration is future work.
- **`RwLock` reader starvation is unbounded** (condvar-blocked tasks do not age),
  and each condvar has a documented capacity precondition. These are recorded as
  preconditions rather than fixed.
- ~~**x86_64 tasks never migrate, and placement is decided once at spawn.**~~ **Stale
  — retired by SMPBAL-X86.** x86 now has `CPU_AUTO` placement over the dispatching
  cores and idle-core work stealing, for kernel *and* ring-3 tasks. What it still does
  **not** have, on purpose: no re-placement at wake (`rewake_place`), no service
  priority band (`PRIO_SERVICE`/`spawn_prio` remain aarch64-only, so an operator-started
  program is still a round-robin peer of the compositor wherever it lands), and none of
  the aarch64 SPREAD-4…15 layer. Correction happens on the idle side only.
- **No TLB-shootdown IPI on x86.** Cross-core staleness of *user* mappings is handled at
  DISPATCH by the `AS_GEN` deferred-reload scheme (SMPBAL-X86), not by invalidation — and
  dispatch is the scheme's boundary: a sibling ring-3 thread of the same process already
  RUNNING on another core (shared `user_cr3`, placed by `sibling_online_cpu`) keeps stale
  leaves for the rest of its quantum when a peer unmaps a user page (local `invlpg` only).
  Pre-existing, not widened by this arc (threads are `steal_ok == false`), and OPEN.
  Kernel-half mappings mutated after a slot's PML4 was built are a separate, pre-existing
  question the scheme also does not address.

---

## See also
- `unaos/crates/kernel/src/arch/x86_64/{sched,smp,percpu,acpi}.rs` — the implementation.
- `unaos/crates/kernel/src/main.rs` — `x86_usb_pump` / `x86_input_service` /
  `x86_render_service` and the SCHED-X86 handoff block in `kernel_main`.
- `unaos/crates/kernel/src/arch/x86_64/memory.rs` — `ensure_pat_wc`, and why every
  core that can blit must program PA4=WC.
- [`network_stack.md`](../06_NETWORK_STACK/network_stack.md) and the USB/video docs — the other subsystems the BSP services while the scheduler runs the APs.
