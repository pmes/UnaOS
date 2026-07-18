# AARCH64-PRIO landing — fixed-priority multilevel run queues + anti-starvation aging

Track: schedprio · Branch: `us-schedprio` · Base: `main` `256a5f7`.
Lane: `arch/aarch64/sched.rs` + `docs/dev/OS/01_BOOT_HAL/arch_arm64.md`. x86 files read-only.
One arc, 3 code milestones (M1 queues / M2 aging / M3 witness) + doc (M4).

## The gap this closes

The aarch64 scheduler was flat round-robin (a single `VecDeque<Box<Task>>` per CPU) — the audited
gap vs x86, which has multilevel priority queues + aging. This ports the x86 DESIGN
(`arch/x86_64/sched.rs`), adapted to the aarch64 module's own structures, preserving every aarch64
contract (per-CPU run-queue spinlock ownership, cooperative + Pi preemptive paths, the CPU_BUSY/IDLE
telemetry, the `virt` busy/idle-heartbeat, the deferred secondary tick).

## Scope delivered

- **M1 — multilevel queues.** `RunQueue` = `[VecDeque<Box<Task>>; NUM_PRIORITIES]` (4 levels:
  `PRIO_LOW`/`PRIO_NORMAL`/`PRIO_HIGH`/`PRIO_RT`), `const fn new()` so `RUN_QUEUES` stays a plain
  const static (no lazy_static — this module's existing style). `pop_highest` pops the front of the
  highest non-empty level. `Task` grows `priority` (immutable base) + `wait_ticks` (lock-protected).
- **Spawn API.** `spawn`/`spawn_user`/`spawn_joinable` UNCHANGED — they default to `PRIO_NORMAL`, a
  single level, so every existing call site (many in `main.rs`) stays behaviourally identical to the
  pre-priority flat round-robin. New `spawn_prio(name, entry, arg, cpu, priority)` picks a level.
  `spawn_inner` grew a `priority` param (internal). No mechanical call-site churn was needed.
- **M2 — aging (x86 pattern).** `RunQueue::push` (ENQUEUE) re-bases to the base level + zeroes
  `wait_ticks`; `RunQueue::age` (RELOCATE) sweeps HIGH→LOW under the run-queue lock, promoting any
  task past `AGE_TICKS` one level up via a raw `VecDeque` move (base untouched; carries surplus
  credit). Sweep runs in `dispatch_next` gated to ~every `AGING_INTERVAL`, in the SAME lock
  acquisition as `pop_highest` (age-then-pick). A promoted-then-dispatched task re-bases on its next
  enqueue.
  - **Clock adaptation (the one deliberate divergence from x86):** x86 ages in its always-live LVT
    `percpu.ticks`. The aarch64 cooperative dispatch paths (BSP demo, `virt` secondaries, `virt`
    CAPSTONE, QEMU raspi4b) have NO live periodic tick (QEMU delivers no Group-1 timer IRQ →
    `percpu.ticks` frozen at 0). So aging advances one unit per `dispatch_next` pass on the owning
    CPU (`SchedCpu::age_passes`/`age_last_sweep`) — which ticks on EVERY path (cooperative +
    preemptive, QEMU + metal). A pass IS the starvation measure. Every other aging invariant matches
    x86 exactly.
- **M3 — witness.** `priority_aging_witness(cpu)`: `PRIO_HIGH` loaders keep the top level
  continuously non-empty while one `PRIO_LOW` candidate (runs to completion in one dispatch, never
  yields) must be aged up. Asserts the low task completed WHILE high load was still active (only
  possible via aging) → `:: AARCH64 SCHED: priority+aging PASS ::`. Bounded + never hangs: finite
  work, no low-task yield, so a broken aging path FAILs loudly instead of wedging the core. Hooked
  into `run_capstone_boot_core` (before the CAPSTONE), so it runs on the `virt` GICv3 boot core
  (test-arm 40) and, as a bonus, the Orin metal boot core (JM6 path).
- **M4 — doc.** `arch_arm64.md` scheduler section + this landing report.

## Gates (verbatim)

- `./arroyo check` — `✅ x86_64 OK` / `✅ aarch64 OK` (no new warnings from this diff; the lone
  `unused_braces` at sched.rs:966 is pre-existing in `meter_cpu_count`).
- `UNAOS_TEGRA=1 ./arroyo check` — `✅ x86_64 OK` / `✅ aarch64 OK` (the Orin `run_capstone_boot_core`
  path, where the witness lives, compiles under the tegra feature).
- `UNAOS_GICV3=1 ./arroyo test-arm 40`:
  - `:: AARCH64 SCHED: priority+aging witness — 2 PRIO_HIGH loaders vs 1 PRIO_LOW candidate on cpu 0 ::`
  - `:: AARCH64 SCHED: priority+aging PASS ::`  ← the new witness
  - `:: AARCH64 SMP: 3/3 secondaries online via PSCI CPU_ON on the GICv3 path ::`
  - `:: AARCH64 SMP: AP {1,2,3} pulse (busy=8, idle=2) ran+idle ::` (busy=8 UNCHANGED)
  - `:: AARCH64 SMP: per-core idle heartbeat PASS ...` + `... per-core busy heartbeat PASS ...`
  - `:: CAPSTONE COMPLETE — all 6 sync primitives verified in one boot ::` (6/6 unchanged)
- `./arroyo test-arm 22`: `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<`
- `./arroyo kernel8-test`: **39 PASS / 0 FAIL**, `CAPSTONE COMPLETE` (shared sched.rs unregressed on
  the Pi; its APs run the full `run()` loop via `start_aps`. All Pi tasks default to `PRIO_NORMAL`,
  so the multilevel queue behaves as flat round-robin. 0 FAIL is the invariant; the PASS count is
  boot-to-boot demo variance, previously 43).
- `./arroyo test 22` (x86): `MISSION SUCCESS`, 4/4 SMP online, all U*/SOCK tests PASS — x86 sched
  untouched, unregressed.

## Lens (ONE, at landing) — verdict PASS

Adversarial self-review of the diff:
- **busy-heartbeat non-regression (the named contract to protect).** `dispatch_next` still bumps
  `CPU_BUSY`/`CPU_IDLE` only on real dispatch/idle. The `sec-probe` tasks are all `PRIO_NORMAL`, so
  they occupy one level and round-robin FIFO exactly as before; the aging sweep never promotes them
  (they re-base + zero `wait_ticks` on every yield, so they never reach `AGE_TICKS` inside their
  8-dispatch drain). Verified: `busy=8` on all 3 APs, idle-heartbeat + busy-heartbeat both PASS.
- **Lock-ordering.** The aging relocate's `push_back` into `level+1` may reallocate under the
  run-queue lock, taking the heap lock (run-queue → heap). This is the SAME ordering as `spawn`'s and
  `make_ready`'s post-lock `push`, and no site ever takes a run-queue lock while holding the heap lock
  (allocation in `spawn` releases the heap lock before the run-queue lock). Heap is always innermost,
  never inverted — benign, exactly as x86 documents for its run queues.
- **Aging-counter soundness.** `age_passes`/`age_last_sweep` are owning-CPU-only (dispatch_next runs
  sequentially on one core), so `Relaxed` is correct. `elapsed` is `min`-saturated before the `u32`
  cast, so a large inter-sweep gap loses no credit (`age` carries surplus). The counters are never
  reset across witness→CAPSTONE, which is fine (elapsed stays ~AGING_INTERVAL).
- **Witness cannot hang / cannot cheat.** Every task does finite work; the low task never yields, so
  `run_until_empty` always drains even if aging is broken (the low task then runs after the load
  drained → `under_load == false` → FAIL, loud). PASS requires the low task to have run while
  `PW_HIGH_ACTIVE > 0`, which strict priority alone forbids — only aging admits it.
- **Isolation.** The witness stages + drains its own tasks and leaves the queue empty, so it does not
  perturb the CAPSTONE that follows (CAPSTONE COMPLETE still lands, 6/6).

No MUST/SHOULD-FIX outstanding.

## Spawn-API shape chosen

`spawn`/`spawn_user`/`spawn_user_slot`/`spawn_joinable` keep their current signatures and default to
`PRIO_NORMAL` (the single level = today's behaviour); a new `pub fn spawn_prio(name, entry, arg, cpu,
priority)` is the only new public entry point. This satisfies "default = today's single level; keep
every existing caller compiling with its current behavior" with ZERO call-site churn — chosen over
adding a `priority` param to `spawn` (which would have forced a mechanical edit at every one of the
~40 `spawn`/`spawn_user*` sites in `main.rs` for no behavioural gain).

## Flagged / ledger (not this gate)

- The Pi preemptive path (`start_aps`, metal) and the Orin metal boot get the priority machinery for
  free (all current tasks are `PRIO_NORMAL`, so no observable change), but no metal sitting has
  exercised a genuine *priority mix* or the *preemptive* aging clock on silicon — that is a future
  metal verification at an arc boundary, not this QEMU gate's job.
- `PRIO_RT` (level 3) is defined but unused by any current workload — reserved for a future real-time
  class; the top level is intentionally never a promotion target (aging only relocates levels 0..2).
