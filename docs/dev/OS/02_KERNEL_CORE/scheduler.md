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

The measurement (P68, from the serial wire) was that `bg-el0` launches placed by
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
:: SCHED: task 'bg-el0' -> core 2 (policy: load-balanced EL0 residents=1, no-migrate) ::
```

`residents` is **inclusive** of the task just placed — the committed count on
that core after this placement. The next attended boot therefore proves the
accounting directly: successive `bg-el0` launches should walk the cores rather
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

### Orphan-reaper wake on enqueue (aarch64, SCHED-4b)

**SCHED-4 sleep_ticks regression** (U11-reap FAIL, timer never ticks in QEMU) bisected and fixed by SCHED-4b (`d7631117`): semaphore wake on orphan enqueue — ~0% idle duty metal-confirmed (c2=0% P31b), U11-reap PASS restored.

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
| Bad placement | SPREAD-11 — yield-path slot re-placement | Real but insufficient. Helped once (`ymoves=1`); the storm re-forms on two cores because `rewake_place` declines equal-load moves. |
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
FIQ that did land would kill the core it was sent to diagnose. There is also an
open GIC-side question: `GICD_IGROUPRn` and `GICC_CTLR.FIQEn` are Secure-only in
the GICv2 Non-secure view, so a Group-0/FIQ-routed SGI may not be reachable from
where this kernel runs at all, and the BCM2711 ARM-local per-core mailbox FIQ
path (which this tree does not map today) would be the alternative. If a PC
sample is ever taken, it must be printed beside whatever names the masking
window — a PC alone dates a symptom without naming a cause.

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

- ~~**x86_64 only.** aarch64 runs a single polled core; it has no GIC-driven
  preemption or scheduler yet.~~ **Stale — retired.** aarch64 runs the full
  preemptive SMP scheduler on all four Pi 4 cores, GIC-driven off the per-core
  generic timer PPI (`TIMER_INTID = 30`), with work stealing, priority aging and
  EL0 tasks. Most of the sections above this one are aarch64 work.
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
