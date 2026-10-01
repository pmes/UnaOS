# STRAND — rmbp A5 "shell window tears under storm" (boot 17), the user-space half

## Finding
`:: VUGART: frames=7124 coherent=6977 torn_rows=16416 mixed_frames=147 strand=147 score=0 streak_max=2`
Every mixed frame is `strand`, rows in units of 108 (one band). TEAR.md (branch exec-rmbp-tear) shows it is not a
scan-out tear: `[wc-h] ... torn=no`, beam hold ran.

## Mechanism (`unaos/crates/user-vug/src/main.rs`)
- Parent publishes GEN then `PHASE=gen`; workers A/B paint bands 0/1, parent paints band 2, then the barrier waits
  `DONE >= live` and presents (`art_score` then `SYS_WIN_PRESENT`).
- `art_strand` (fired when the parent leaves its 64-yield spin) charged the frame as mixed whenever a band was
  stale with no writer in it, i.e. a worker released but not yet started (parked/dispatch latency). The parent then
  KEPT WAITING, presented a finished surface, and `art_score` skipped it via the latch. So `strand=147` counts
  frames where a worker was LATE, scored as torn although the present carried no stale band; the protocol had no
  bound, no repaint, and a count-based barrier (`DONE`) that a stale arrival could satisfy.

## Milestones
- M1 `art_log`: `[vug] strand gen= band= worker_gen= late_us= rep=` (first 8, then 1 per 1024); `rep=0` at spin
  exhaustion, `rep=1` at the repaint decision.
- M2 per-worker `WGEN` ack words (barrier exits only on `WGEN[b]==gen`), per-band `CLAIM` swap (one winner between the
  worker and the parent), `art_late`: after `LATE_TICKS` (3 ms) the parent REPAINTS every unclaimed, unacknowledged
  band for gen, then waits on any band a worker already claimed. All x86-only (aarch64 `.text` cap; image unchanged).
- M3 `strand=` stays a count and no longer charges `mixed_frames`; `art_score` is the only judge. Verdict PASS iff
  `mixed_frames==0` (coherent==frames); line gains `repaints= waits_us=` before `->`; `thr=coherent==frames`;
  `BUF_CAP` 176 (x86). `x86-wc.spec` REQUIRE updated.

## Written
Boot 18: `:: VUGART: frames=N coherent=N torn_rows=0 mixed_frames=0 strand=<k> score=0 streak_max=0 severity=0
thr=coherent==frames fps= ms= repaints=<r> waits_us=<w> -> PASS ::` plus up to 8 `[vug] strand` lines.
`waits_us` has 1 ms resolution (tick clock). If TEAR merges, its `beam=(held|off)` token goes between `thr=` and `fps=`
and its BUF_CAP (144) conflicts with 176; keep 176 and add `repaints= waits_us=` after `ms=` in the spec row.
