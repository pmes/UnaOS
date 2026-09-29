# WCDPOOL — the wc-d "no memory for 8x8 snapshot" SKIP

## Finding (boot 16, f16.log)
`[7507ms] [wc-d] verify win=30 -> SKIP (no memory for 8x8 source snapshot)` (and win=31, twice each) is NOT a real OOM.
It is the two WCD fixtures' literal: `wcd_oom_latch_selftest` (IDS=[30,31], first extent `8*(n+1) x 8`) and `wcd_skip_latch_check`
(same ids) in `video/wm.rs` tail, each printing the shipping line once per id to prove the latch (2 fixtures x 2 ids = 4 lines).
Real windows never appear. The heap on that boot was 256 MiB (`HEAP: chose 0x20200000..0x30200000 (256 MiB)`, `allocator.rs:40`);
no other oom/`no memory`/`mirror_oom>0` line exists in f16.log (`VUGPERF ... mirror_oom=0`).

## Milestones
- M1 (written): real SKIP (`wm.rs` verify_reference, ~8006) now ends ` heap_used= heap_free= largest=` via `WcdHeapNote` (Display, census
  computed only on the latched path; `allocator::heap_census` already existed, EL0MEM). `:: HEAP: size= used= free= largest= -> PASS ::` on
  the first `[comp2]` rollup (= desktop's first passes) and every 6th (30 s), from `wcd_heap_tick()` folded on the `wcd_skip_rollup()` call.
- M2 (NOT written, on purpose): the "SKIP" was fixture text; a 64x256 B pool would only serve 8x8 while real snapshots are surface-sized
  (up to multi-MB), so it would be dead code and cannot remove a real skip. Revisit only if a `heap_used=` SKIP line shows on metal.
- M3 (no change): x86 heap is already 256 MiB (raised from 48 MiB, GR27); the census on boot 17 says whether it is ever near full.

## Written
Boot 17 should show `:: HEAP: size=268435456 used=<n> free=<n> largest=<n> -> PASS ::` ~every 30 s; spin pin in `x86-wc.spec`.
