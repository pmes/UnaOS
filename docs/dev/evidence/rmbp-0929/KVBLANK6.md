# KVBLANK6 — the Kepler vblank interrupt arms at boot, whatever the compositor is doing (boot 17 finding)

## Design
Finding (boot 17): every `:: kepler: vblank head=0 …` census reads `mode=poll vbwait_irq=0 isr_calls=0 acks=0 rearms=0 rearm_written=00000000`; no `vblank-intr vector close` line; `:: KVBLANK4: … vbwait_mode=poll -> PASS ::` at 7.7 s; `vblank-intr census sample=1/8 … en=00000000 writes=0`.

Hypothesis (rung 3 deferred behind `tests` / `desktop_allowed()`): REFUTED by the code. Rung 3 is not a fixture and no `desktop_allowed()`/`tests` gate touches it. It is `LADDER_IRQ_ARM/RUN`, the 4th/5th step of a ladder stepped by `edge()` (`kepler_vblank.rs` `ladder_tick`), and `edge()` runs only when `note()` is fed by `scanout_beam()`, whose only caller is the compositor's `video::beam::hold` (`video/beam.rs`, `scanout_beam()` at hold / wait_next_edge). Rungs 1-3 need 8 census samples (30 vblanks apart), a 16-vblank window, a 60-vblank vector window. In the installer stages (R77: furniture held, rare presents) `hold` is called rarely, so the ladder starves. Evidence in the in-tree flight-17 log (`rmbp-0915/flight17/f17-boot1.log`): census samples 3..8 at 23.7/33.8/34.4/34.9/36.4/42.3 s with `count_delta=921/609/…` (hold-call-paced, not vblank-paced), rung 3 only at 43-44 s. Boot 17 never got past the census. (The brief's boot-17 log with KVBLANK5 books is not in the tree; this is read from the code and the flight-17 log.)

Mechanism: `kepler_vblank.rs` — `edge()` stamps `LAST_COMP_EDGE_MS` and spawns the pump once; `pump_task` (kernel task, 8 ms) calls `crate::arch::scanout_beam()` (-> `note` -> `edge` -> `ladder_tick`) when no compositor edge is newer than 40 ms; `LADDER_BUSY` serialises the ladder between the two feeders.

Milestones
- M1: `:: kepler: vblank-intr rung3 scheduled=boot reason=ladder-fed-by-pump-not-by-compositor-presents stage_resolved=… ::` printed when the pump spawns (first edge after arm). `scheduled=tests|held` are not reachable: nothing gates rung 3 on either.
- M2: pump drives the ladder at boot independent of the desktop stage; the close keeps the vector armed when delivery >= 90% (KVBLANK5 M3 `kept_live=1`, now also requires a wire); `:: kepler: vblank-intr vector census … verdict=live …` at the deadman cadence (1 s, from `report`) and once at shutdown (`power::shutdown` -> `shutdown_census`, kind `shutdown`); `tests kvblank` (registered beside `dock` in `arch/x86_64/syscall.rs`) re-runs the fixture only (`selftest_rerun`).
- M3: `vbwait_mode()` is one function used by `report` and the KVBLANK4 fixture line.

Pins: `x86-witness.spec:1659` still matches on QEMU (fixture's `vbwait_mode` is `poll` there; no Kepler, pump never spawns — `VB_BAR0==0`). No spec change, no new knob.

## Written
Boot 18 should show: `rung3 scheduled=boot` early (during the installer), `vector close … mode=irq deliver=msi` within ~60 vblanks of the census finishing, `books … kept_live=1`, then each second `vector census … mode=irq isr_calls≈acks≈rearms_isr` climbing, `stuck=0`, `vbwait_mode=irq`, and a `vector shutdown` line on shutdown. If `stuck` grows the W1C ack guess (KVBLANK5 M2) is wrong.
