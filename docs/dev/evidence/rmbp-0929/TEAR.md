# TEAR — rmbp A5 "shell window tears under storm" (boot 17)

## Finding
`:: VUGART: frames=7124 coherent=6977 torn_rows=16416 mixed_frames=147 strand=147 … streak_max=2 severity=2`
(bands of 108 rows; VUGART2 bound `streak<=1` exceeded).

## What the tree already had (brief premise corrected)
- The beam HOLD exists and ran in boot 17: `video/beam.rs:hold` (spin + KVBLANK wait arm), called per band
  at `video/wm.rs` band loop (`beam::hold(by + band, by + band + rows, …)`, `par_blit` runs INSIDE the
  bracket), `wm.rs` direct/fill paths and `strip.rs`. Boot-17 `[wc-h]` lines show it:
  `beam=1185..1457/1852 beamwait_us=6588 torn=no`. Knob `UNAOS_BEAM=1` / feature `beam` is already
  three-place wired (arroyo:1409, builder main.rs:537, kernel Cargo `beam`).
- VUGART's "torn row" is NOT a scan-out tear. `user-vug/src/main.rs` (VUGART block ~1150-1450) stamps the
  generation of the last writer to finish each of the vug's three bands (worker A 0..108, B 108..216,
  parent 216..288) and counts rows of the presented surface not of the frame's generation: a USER-SPACE
  worker/barrier coherence miss. `strand=147 score=0` says all 147 are the "strand" class (a worker
  finishing a band after the parent abandoned the frame). A present-side beam hold cannot move it.
  Expect boot 18 `torn_rows` to be the same with `beam=held` and `beam=off`; that equality IS the proof.

## Milestones written
- M1 census (beam.rs tail of `settle`): `[beam] holds= held_us= gaveup= bands= guard=` and
  `:: BEAMHOLD: holds= held_us= gaveup= bands= -> PASS|FAIL ::` every 1024 closed brackets, x86 + `beam` only
  (PASS = gaveup==0). `guard=` prints FETCH_LINES (64); the write lead is per-rect (`span/3+16`). No-op on
  None (no bracket, `settle` returns early).
- M2: no new call needed — the per-band hold (serial band loop and the WCPAR bracket) already exists.
- M3: VUGART line carries `beam=held|off` (from `option_env!("UNAOS_BEAM")` at user-vug build time),
  `BUF_CAP` 112 -> 144 (x86), `x86-wc.spec` REQUIRE updated.

## Written
Boot 17+: `:: BEAMHOLD: holds=… held_us=… gaveup=0 bands=… -> PASS ::` and
`:: VUGART: … thr=streak<=1 beam=held fps=… -> PASS|FAIL ::`.
Not done: the real fix for `strand` (worker release/abandon race in user-vug) — a separate arc.
