# GPUTESTS — what the GPU wave left owed, closed as one arc (ledger B334)

Branch `exec-rmbp-gputests` (cut from 8c750d43, the merge11 tip). Seam: `CHARTER: Kernel — driver`
for every file touched (`drivers/gpu/{gen7,igpu,kepler,kepler_ce,kepler_vblank}.rs`, `tests.rs`);
`video/blitter.rs` untouched. No new file under the GATE-CHARTER scope, no new dotfile, no new knob,
no new shell verb (`tests gen7` / `tests kvblank8` / `tests kblit` are existing fixture names).
R80 (Peter, 2026-10-04): nothing runs at boot but the boot.

## Findings

1. **The Ivy Bridge ladder R1–R7 runs at boot** under `UNAOS_IVB3D` (`igpu::init` calls seven rungs;
   flight 20 printed `:: gen7: recon …` / `r7 verdict=r7-blit-verified` on both boots). GEN7R8 moved
   only R8. The ordering constraint is real: R1's GGTT census must read FIRMWARE's PTEs, before
   `bring_up_blt_ring` writes its ring PTE.
2. **KVBLANK rung 3 arms at boot**: the edge-driven ladder walks census → window → vector on its own,
   and rung 3's close applies the 90 % delivery check (`keep`) on the boot path.
3. `wc_gpublit = []` — the knob is meaningless without `wc` (KCOMP).
4. `kepler_ce` is gated on the feature only, so an aarch64 build carrying `nvidia-kepler-ce`
   compiles a whole GK107 copy-engine driver as dead code (KBLIT).
5. KBLIT's scratch window is the fixed `KB_BASE = 48 MiB`, chosen by hand against the allocator's
   32 MiB skip; the fifo leg and the takeover allocate from the real `VramAllocator`, so nothing
   proves they never meet.
6. `kvblank selftest … waited_us=16833 bound=waited_us<=16667 FAIL` (flight 19) is read as a 1 %
   pace error. It is ONE wait on a simulated timer counter; it measures the wait loop's overshoot,
   not a clock. Nothing on the wire measures the real frame period against the TSC to ppm.

## Milestones

- **M1 — the ladder behind `tests gen7`.** Boot (`igpu::init`, the same spot, still above
  `bring_up_blt_ring`) calls `gen7::bank(...)` only: it banks BAR0/BDF, R1's twelve GGTT sample PTEs
  (read-only, firmware's), and the panel geometry R8 used to read at its stash; it prints nothing.
  `tests gen7` (registered on any `gen7` build) runs R1..R7 in order from the bank — R1's GGTT census
  answers from the banked PTEs (`:: gen7: ggtt source=boot-bank …` says so), every other rung reads
  live, as its ownership guards require — then R8 under `gen7r8`. Once per boot (R5–R8 hold pages); a
  second run replays the witness. Refused when `igpu`'s own BLT ring came up (the rungs would arm the
  BCS under a live ring). Witness:
  `:: GEN7LADDER: rungs=R1-R7 ggtt=boot-bank wake=<w> r7=<verdict> us=<n> replay=<0|1> -> PASS|FAIL ::`
  (PASS iff R7 returned `r7-blit-verified`), then `:: GEN7R8: … ::` unchanged.
- **M2 — KVBLANK rung 3 from the fixture.** Rung 2's close parks the ladder (`LADDER_IRQ_PARKED`)
  instead of arming the vector. `tests kvblank8` un-parks it, waits (bounded) for rung 3's window to
  arm and close on the edge driver — its `vector armed` / `vector close` / `isr books` lines are
  unchanged — and prints the 90 % check as a fixture line before the KVBLANK8 instrument runs:
  `:: KVBLANK8: rung3 irq=<n> vbl_delta=<v> ratio_pct=<p> kept=<0|1> -> PASS|FAIL ::`. Under the
  `kvblank_trace` knob the boot path keeps arming rung 3 (a knob is R80-admissible; the trace exists
  to watch the takeover).
- **M3 — Cargo.** `wc_gpublit = ["wc"]`; `kepler_ce` declared under
  `all(target_arch = "x86_64", feature = "nvidia-kepler-ce")` with both consumers matched; arroyo's
  `arm_features` strips `nvidia-kepler-ce`; the k8-reach `UNAOS_KEPLER_CE` row TODO → NA.
- **M4 — KBLIT's window from the allocator.** `kepler::init` hands `arm_context` the live
  `VramAllocator`; it reserves the KBLIT span (`0x90000`) there, so the fifo leg's later allocations
  are past it, and banks the base. `tests kblit` uses the banked base; the BAR1/VRAM bound check stays.
- **M5 — `pace_err_ppm=`.** The kvblank8 census times 60 hardware frames edge-to-edge on the TSC
  (endpoints aligned to `HEAD_STAT.VERT[31:16]` edges) and appends `waited_us=<mean> pace_err_ppm=<signed>`
  and `isr_pace_err_ppm=` (from the ISR timestamps when ≥ 2 deliveries) to the census line.

## Witness (a metal boot that fires the tests)

Boot: no `:: gen7:` line, no `vblank-intr vector` line (unless `UNAOS_KVBLANK_TRACE`).
`tests gen7`: R1..R7's lines as flight 20 printed them at boot, plus `:: gen7: ggtt source=boot-bank …`,
`:: GEN7LADDER: … -> PASS ::`, then `:: GEN7R8: … -> PASS ::`.
`tests kvblank8`: `vector armed`, `vector close`, `isr books`, `:: KVBLANK8: rung3 … ::`, the KVBLANK8 lines,
`:: KVBLANK8: census … waited_us=<n> pace_err_ppm=<n> isr_pace_err_ppm=<n|-> ::`.

## Owed

- KVBLANK rungs 1 and 2 (census, enable window) still print at boot on the edge driver; the brief
  named rung 3 only.
- The `kvblank selftest arm=wait sim=timer` fixture still runs inside `beam::hold` on `witness` builds.
- The compositor's "first need" does not arm rung 3; it paces on the poll source until `tests kvblank8`.
- KBLIT's identity page table still indexes PT entries from the window's first page, not from the PDE's
  VA origin ([EXT-UNPINNED], as KBLIT wrote it); a metal `advanced=1` is what tests it.
