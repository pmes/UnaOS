# VUG-HONESTY — landing report

**Branch:** `us-vughonesty` (off main `03105f0`). **Executor:** one Opus session.
**Lane:** Orin/vug display; primary `vug.rs` + additive introspection accessor in
`arch/{aarch64,x86_64}/sched.rs`. **Status:** DONE gate passed, committed (never merged/pushed).

## Reconciliation verdict (Maestro brief-read flag)

The arc is **VALID and NARROW**. The brief's premise ("parked cores read pinned, no idle
accounting") predates the merged idle/busy-heartbeats (`99d35ce`/`a86b123`, doc `4f661ad`), which
this base already carries. Those fixed the **counters**: `sched::note_core_idle` bumps `CPU_IDLE`
at park-entry + every WFI wake, so a parked online secondary reads `idle > 0` (not `(0,0)`), and the
busy-heartbeat gives `busy > 0`. That made the BSP's **one-shot boot witness** honest.

**A genuine residual remained, one layer below** — in the live `vug` *display*, untouched by the
counter work. I built strictly **on** `note_core_idle` (did not duplicate or parallel it).

## Root cause (stated before fixing)

`vug`'s CPU-pulse meter does not read cumulative counters — `CpuPulse::refresh` samples their
**per-window deltas** (~5×/sec) and shows `db/(db+di)`. Its fallback branch read: *"`db+di == 0`
this window ⇒ the demo core executing outside the scheduler ⇒ credit it the render loop's own
busy% (`own_load`)."* That is correct for exactly **one** core — the core running the render loop,
whose own counters freeze while it draws. It was applied to **every** frozen-counter core.

A parked EL2 `virt`/Orin secondary gets **no periodic wake** (no per-core timer; `note_core_idle`
bumps only at park-entry and on the rare BSP→AP SGI), so between two 200 ms windows its counters do
not move → `db+di == 0` → the old code credited it `own_load` too. While the crystal spins at a high
render busy%, **all parked cores mirrored the busy demo core and read PINNED** — fabricated load on
cores doing nothing. A never-online core `(0,0)` read identically. This is precisely the R18 XCARVE
metal witness finding, surviving *below* the counter layer the heartbeats fixed (Maestro residual
(a) never-woken + (b) no idle-vs-parked visual — both real).

## What landed

Display-layer only; no scheduler logic, no counter, no `note_core_idle` seam changed.

- `vug.rs`:
  - `classify_load(db, di, is_demo, own_load)` — pure decision: `db+di>0` → honest busy fraction;
    frozen **and** demo core → `own_load`; frozen **and not** demo → `PARKED` (a `u32::MAX`
    load-array sentinel, disjoint from 0..=100). Never fabricates.
  - `refresh` now identifies the demo core live via `sched::meter_current_cpu()` and routes each
    core through `classify_load`; only the demo core takes the `own_load` fallback.
  - `draw_pulse_bar` renders `PARKED` as a **dashed, cooler track** (`METER_PARKED`, every other
    segment) — distinct from an idle core's solid-dim track, so "idle 0%" ≠ "never woken"
    (JD16/JD17 unset-≠-invent doctrine). `run_pulse` prints `park` in place of a percent.
  - `parked_display_witness()` — deterministic, framebuffer-free; asserts the separating cases and
    emits one PASS/FAIL serial line.
- `arch/aarch64/sched.rs`, `arch/x86_64/sched.rs`: additive `meter_current_cpu()` — a read-only
  self-index (`TPIDR` on aarch64, `gs:[0]` on x86), same introspection-only contract as the existing
  `meter_cpu_count`/`meter_cpu_ticks`. **No counter mutation, no scheduling-path effect.**
  - x86 disclosure: the brief cautioned against touching x86 arch files. The shared `refresh` calls
    `meter_current_cpu()`, so x86 must provide the mirror to compile; it is a read-only accessor of
    the same shape, not scheduler logic, and does not touch the RAST-1 panel wire-in. The latent
    fabrication bug is in the shared code, so the fix improves honesty on both arches; x86's normal
    APs run the scheduler (`db+di>0`), so no behavioral change on the tested x86 path.
  - Witness wired into the in-lane `run_capstone_boot_core` (aarch64 sched.rs).
- `docs/dev/OS/01_BOOT_HAL/arch_arm64.md`: new `### VUG-HONESTY` subsection folded after the
  busy-heartbeat section (the doc that documents vug/the heartbeats).

## Gate results

- `./arroyo check` both arches: ✅ (x86_64 OK, aarch64 OK).
- `UNAOS_TEGRA=1 ./arroyo check` both arches: ✅.
- `UNAOS_GICV3=1 ./arroyo test-arm 40`: ✅ — `3/3 secondaries online`; `AP 1/2/3 pulse (busy=8,
  idle=2) ran+idle`; idle-heartbeat **PASS**; busy-heartbeat **PASS**;
  **`:: VUG-HONESTY: parked-core display witness PASS ... a frozen non-demo core reads PARKED
  (never the demo core's load) ::`**; `CAPSTONE COMPLETE — all 6 sync primitives`.
- `./arroyo test-arm 22`: ✅ MISSION SUCCESS.
- `./arroyo kernel8-test`: ✅ CAPSTONE COMPLETE, 0 FAIL (23 PASS; shared `sched.rs` unregressed on
  the Pi — its APs run the full `run()` loop, an untouched path).
- `./arroyo test` (x86 headless): ✅ MISSION SUCCESS, 0 FAIL, no behavioral change.

## Review tier

Seat-read (thin display logic). The sched.rs additions are **read-only accessors**, not accounting
mutation, so the brief's "1 lens only if sched/percpu accounting was touched" does not trigger. A
careful self-review of the diff was done (sentinel disjointness vs 0..=100 confirmed; parked bar
covers both the corner meter and the full-screen `pulse`; demo-core log now fires only for the real
demo core).

## Flagged

- **Metal witness** rides the next natural Orin sitting: the default image post-merge already
  carries the fix (no new knob, no new media) — parked APs' bars should render dashed `park`, not
  pinned, on a live multi-core Orin panel. Note for the sitting brief; nothing new to stage.
- A `/Volumes/UNAOS` card was mounted during the session; `kernel8-test` does **not** write to it
  (only the manual `esp-arm` staging path does, lines 565-569 of `arroyo`), so the run was safe.
- The idle/busy-heartbeat serial witnesses and the counter-level honesty are unchanged and still
  green — this arc completes their **display** third leg, it does not alter them.
