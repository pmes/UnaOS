# KERNEL-CLOCK landing — calibrated timebase → `sleep_ms` + bounded `join_timeout`

**Branch:** `hw-rmbp` (off main `80833c2`). **Lane:** `arch/x86_64/sched.rs`,
`arch/x86_64/apic.rs`, `arch/x86_64/mod.rs`, this landing report.
The queued kernel-clock arc (scheduler brief §4 candidate).

## Why

`sleep_ticks`/`quantum`/`AGE_TICKS` counted raw local-APIC ticks. **M1 (APIC-timer
calibration → 1 kHz wall-clock tick) already landed in-tree** via the net-track
cross-pollination (`apic::calibrate` measures TSC + APIC rate against the ACPI PM timer,
arms the heartbeat at `TICK_HZ = 1000`, and `arch::ticks()`/`ms()` read it) — so this arc
did **not** re-implement it (per the brief: "cross-pollinate, don't duplicate"). It builds
the two consumer layers the brief names on top of that timebase.

## What landed

- **M2 — `sched::sleep_ms(ms)`.** `sleep_ticks` expressed in real time via a new
  `arch::ms_to_ticks(ms)` (`ms * apic::TICK_HZ / 1000`), which derives the ms↔tick mapping
  from the one rate the timer is armed at rather than hard-coding "1 tick == 1 ms" a second
  time. `apic::TICK_HZ` was made `pub` for this. Graceful degradation before calibration
  (~0.8 ms/tick under QEMU → proportionally short), same as `ms()`/`ticks()`.
- **M2 witness — `SLEEPMS:` demo.** Sleeps `SLEEP_MS_TARGET = 100` ms and measures the
  ACTUAL elapsed against an **independent** reference — the invariant TSC (`now_cycles`),
  which advances regardless of interrupt delivery, so a broken calibration or lost wake
  reads a wildly wrong duration. Generous 2× tolerance band `[50, 200]` ms absorbs QEMU/TCG
  timer-delivery looseness while catching gross misses. Bounded by the sleep → cannot hang,
  no watchdog needed. Reports `SKIPPED` if the TSC never calibrated (no wall reference).
- **M3 — `Semaphore::try_wait()` + `JoinHandle::join_timeout(ticks) -> JoinResult`.**
  `try_wait` is `wait`'s fast path with the park removed (safe from any context, never
  switches). `join_timeout` polls the completion semaphore with `try_wait` between
  `sleep_ticks` naps (`JOIN_POLL_TICKS = 2`) until it posts or the deadline elapses, so a
  hung/never-returning task can **never** trap the joiner — it returns `TimedOut`. It
  reuses ONLY the existing sleeper machinery: no new park kind, no dual-deadline, no
  lock-handoff — so none of the §2 invariants are touched. Asserts it runs on a scheduled
  task (off-task `sleep_ticks` is a no-op → would busy-spin).
- **M3 witness — `JOINTMO:` demo.** One coordinator exercises both outcomes: a hung
  stand-in (sleeps 400 ms) joined with a 40 ms timeout must return `TimedOut`; a quick task
  (15 ms) joined with a 400 ms timeout must return `Completed`. PASS = exactly both.
  Bounded by both timeouts → cannot hang.
- **M4 — docs.** Module-header "KERNEL-CLOCK layer" note in `sched.rs` (the layering + the
  timed-out-handle soundness argument) + this landing report.

Both witnesses are sleep-driven and self-checking, so they run on **any** topology (even a
single AP, unlike the RwLock showcase). Pinned to the last AP to keep the ms measurement
off the busy pair on AP[0].

## Soundness of a timed-out join

`join_timeout` consumes `self` (the handle's `Arc<Semaphore>` clone) on every path. On
`TimedOut` the handle drops while the joined task may still hold its own `done_sem` clone,
so the completion semaphore stays alive until the task finishes and drops it — a later
`post()` into an empty waiter list just bumps the count on a soon-to-be-freed semaphore. No
dangle, no leak. (`join_timeout` never parks on the waiter list, so `WAIT_CAPACITY` is not
touched.)

## Lane / invariant discipline — honored

Confined to the arc's lane (`arch/x86_64` sched/apic/mod + the report). The new public
fns are compiled but only exercised from `start_demo` (the `sched_demo` feature), so the
default non-demo boot path is unchanged. aarch64 untouched (no scheduler there). No §2
invariant weakened.

## Gate results (verbatim)

- `./arroyo check` — `✅ x86_64 OK`, `✅ aarch64 OK`.
- `./arroyo test 22` (x2APIC, MISSION) — `MISSION SUCCESS`, no panic / `=> FAIL`. exit 0.
- `UNAOS_CPU=qemu64 ./arroyo test 22` (xAPIC, MISSION) — `xAPIC software-enabled` (ids 0–3),
  `MISSION SUCCESS`. exit 0.
- `UNAOS_SCHED_DEMO=1 ./arroyo test 30` (x2APIC demo) — witnesses:
  - `APIC: calibrated over 358201 PM ticks (100 ms) — TSC 2.399 GHz, APIC timer 62.498 MHz (÷16); 1 kHz tick => initcnt 62498.`
  - `SLEEPMS: [cpu3] slept 142 ms (target 100, tol [50,200], TSC ref) => PASS`
  - `JOINTMO: [cpu3] hung(t/o 40ms)=>TimedOut, quick(t/o 400ms)=>Completed => PASS`
  - `RWLOCK: [cpu3] done 5/5, torn=false, max_concurrent_readers=4 => PASS` (unregressed)
  - `MISSION SUCCESS`.
- `UNAOS_CPU=qemu64 UNAOS_SCHED_DEMO=1 ./arroyo test 45` (xAPIC demo, bonus) —
  `SLEEPMS ... slept 127 ms ... => PASS`, `JOINTMO ... => PASS`, `RWLOCK ... => PASS`.
- `./arroyo test-arm 22` — `MISSION SUCCESS`, unregressed. exit 0.

## Measured

Calibration approach: **APIC-vs-TSC cross-timing against the ACPI PM timer** (the in-tree
net-track approach; CPUID 0x15/0x16 predate Ivy Bridge so nothing is discoverable — it is
measured). Under QEMU: TSC ≈ 2.399 GHz, post-÷16 APIC timer ≈ 62.5 MHz → 1 kHz heartbeat
at initcnt ≈ 62498. Measured `sleep_ms(100)` landed at 127–142 ms wall (TSC-referenced)
under load — within the 2× tolerance; the overshoot is scheduler/quantum latency plus
TCG jitter, not a rate error (the global ms-clock diagnostic reads 719 Hz under TCG timer
coalescing, as documented). On metal the calibrated timer should read much closer to 100.

## Flagged

- Nothing outside lane. `join_timeout`'s poll granularity means worst-case overshoot past a
  just-missed completion is one `JOIN_POLL_TICKS` (2 ticks ≈ 2 ms) — documented, acceptable
  for a coarse kernel join.
