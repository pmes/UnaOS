# MBENCH (kernel8-test) flaky under build load — measured 2026-09-06 by executor WINID2 (orin 16)

Trees: baseline = WINID c3d8f6b7 (unpatched by WINID2); patched = +dfe3584d. Host load: nine executors building concurrently. Command: `UNAOS_PIDESK=1 ./arroyo kernel8-test` (QEMU raspi4b, 640x480).

| run | tree | verdict |
|---|---|---|
| k8t-baseline-1 | baseline |   ❌ MBENCH FAIL — 118/119 required witnesses, 4 forbidden hit(s), 10086 lines scanned |
| k8t-baseline-2 | baseline |   ❌ MBENCH FAIL — 119/119 required witnesses, 2 forbidden hit(s), 10126 lines scanned |
| k8t-baseline-3 | baseline |   ✅ MBENCH PASS — 119/119 required witnesses, 0 forbidden hit(s), 9343 lines scanned |
| k8t-baseline-4 | baseline |   ❌ MBENCH FAIL — 119/119 required witnesses, 2 forbidden hit(s), 10113 lines scanned |
| k8t-patched-1 | patched |   ✅ MBENCH PASS — 119/119 required witnesses, 0 forbidden hit(s), 10162 lines scanned |
| k8t-patched-2 | patched |   ❌ MBENCH FAIL — 119/119 required witnesses, 4 forbidden hit(s), 10955 lines scanned |
| k8t-patched-3 | patched |   ✅ MBENCH PASS — 119/119 required witnesses, 0 forbidden hit(s), 10566 lines scanned |
| k8t-final-1 | patched |   ❌ MBENCH FAIL — 119/119 required witnesses, 2 forbidden hit(s), 9643 lines scanned |
| k8t-final-2 | patched |   ❌ MBENCH FAIL — 119/119 required witnesses, 2 forbidden hit(s), 9514 lines scanned |
| k8t-final-3 | patched |   ✅ MBENCH PASS — 119/119 required witnesses, 0 forbidden hit(s), 10073 lines scanned |

Failure signatures (verbatim, first hit per failing run; the same lines appear on BOTH trees):

```
== k8t-baseline-1
  ❌ FORBID hit @ line 322: [wc-d] verify win=1 surf=560x352 band=0..112 scale=1x at (40,43) panel=640x480 checked=62720 bad_cache=145 bad_ram=83 ram_indep=yes moved=3924 sprite_px=0 nonzero=7820 occ
  ❌ FORBID hit @ line 322: [wc-d] verify win=1 surf=560x352 band=0..112 scale=1x at (40,43) panel=640x480 checked=62720 bad_cache=145 bad_ram=83 ram_indep=yes moved=3924 sprite_px=0 nonzero=7820 occ
== k8t-baseline-2
  ❌ FORBID hit @ line 316: [wc-d] verify win=1 surf=560x352 band=0..112 scale=1x at (40,43) panel=640x480 checked=62720 bad_cache=91 bad_ram=91 ram_indep=yes moved=3916 sprite_px=0 nonzero=7999 occl
  ❌ FORBID hit @ line 316: [wc-d] verify win=1 surf=560x352 band=0..112 scale=1x at (40,43) panel=640x480 checked=62720 bad_cache=91 bad_ram=91 ram_indep=yes moved=3916 sprite_px=0 nonzero=7999 occl
== k8t-baseline-4
  ❌ FORBID hit @ line 325: [wc-g] win=1 seq=1 own=yes scale=1x app=0x77e8f71b6c545a8f blit=0x353068678e8d98f5 civac=0x353068678e8d98f5 after=0x353068678e8d98f5 fbbad=0/197120 occluded=0 occ=0/0 us=3
== k8t-final-1
  ❌ FORBID hit @ line 296: [wc-d] verify win=1 surf=560x352 band=0..112 scale=1x at (40,43) panel=640x480 checked=62720 bad_cache=867 bad_ram=867 ram_indep=yes moved=851 sprite_px=0 nonzero=10918 oc
  ❌ FORBID hit @ line 296: [wc-d] verify win=1 surf=560x352 band=0..112 scale=1x at (40,43) panel=640x480 checked=62720 bad_cache=867 bad_ram=867 ram_indep=yes moved=851 sprite_px=0 nonzero=10918 oc
== k8t-final-2
  ❌ FORBID hit @ line 304: [wc-d] verify win=1 surf=560x352 band=0..112 scale=1x at (40,43) panel=640x480 checked=62720 bad_cache=91 bad_ram=91 ram_indep=yes moved=852 sprite_px=0 nonzero=10688 occl
  ❌ FORBID hit @ line 304: [wc-d] verify win=1 surf=560x352 band=0..112 scale=1x at (40,43) panel=640x480 checked=62720 bad_cache=91 bad_ram=91 ram_indep=yes moved=852 sprite_px=0 nonzero=10688 occl
== k8t-patched-2
  ❌ FORBID hit @ line 301: [wc-g] win=1 seq=1 own=yes scale=1x app=0x6d6b33bd64ef6e0b blit=0x25b788cf9956ee69 civac=0x25b788cf9956ee69 after=0x25b788cf9956ee69 fbbad=15904/197120 occluded=0 occ=1/1 
  ❌ FORBID hit @ line 310: [wc-d] verify win=1 surf=560x352 band=0..48 scale=1x at (40,43) panel=640x480 checked=26880 bad_cache=6816 bad_ram=6816 ram_indep=yes moved=0 sprite_px=0 nonzero=10899 occ
```

Reading: the hits are the desktop's own cache/RAM verify (`[wc-d] verify … bad_cache=N bad_ram=N`) and the glyph race witness (`[wc-g] … RACE-PRESENT`), on the unpatched baseline as often as on the patched tree; WINID2 touches wcg.rs only and cannot reach either. Hypothesis: host load changes QEMU timing enough to trip these two witnesses. Status: flaky-under-load, NOT ruled out; a quiet-box re-run is owed before the landing (LAWS: failed under conditions, code kept). The seat's wave-2 run (gates-99c153ca, one executor building) passed 119/119.

## Discriminators (pi 7's questions, run on the failing captures 2026-09-06)

| run | REQUIRE count | truncated? | `[wc-d] verify` bad_cache / bad_ram | `[wc-g] RACE-PRESENT` |
|---|---|---|---|---|
| k8t-baseline-1 | 118/119 | one REQUIRE missing (the only shortfall) | 145 / 83 (×4), then 0/0 | 0 |
| k8t-baseline-2 | 119/119 | no | 91 / 91 (×4), then 0/0 | 0 |
| k8t-baseline-4 | 119/119 | no | 0 / 0 | 2 |
| k8t-final-1 | 119/119 | no | 867 / 867 (×4), then 0/0 | 0 |
| k8t-final-2 | 119/119 | no | 91 / 91 (×4), then 0/0 | 0 |
| k8t-patched-2 | 119/119 | no | 6816 / 6816 (×4), then 0/0 | 2 |
| k8t-baseline-3 (PASS) | 119/119 | no | 0 / 0 | 0 |

Reading: (1) NOT pi's truncation family — five of six failing runs carry every REQUIRE line and reach the tail; the hits are content-bearing events from detectors that fired. (2) NOT the `wm.rs:4131` non-atomic read-back shape either — that instance is `bad_cache=0 bad_ram=144` (asymmetric); here `bad_cache == bad_ram` in four of five `[wc-d]` failures (91/91, 867/867, 6816/6816) and near-equal in the fifth (145/83), i.e. BOTH read-back passes disagree with the expected content by the same count, which reads as the expected region itself having changed under the verifier (a legitimate concurrent repaint landing before both reads, or a real intermittent race) — the window is load-widened either way. Three hypotheses stay open: host-load timing (QEMU), a non-atomic verify window hit by a legitimate write, a real race. The quiet-box run separates the first from the other two; the equal-counter signature separates this from `wm.rs:4131`. pi 7's spec asserts the same lines (`pi4-regression.spec:576-577`, `:952-954`), so this is a shared row (SO7), not an Orin one.

## got/want discriminator (pi 7, second pass) — verifier-side, and the tree already names the shape

| run | `[wc-d]` first bad | got | want | moved= |
|---|---|---|---|---|
| k8t-baseline-1 | (40,43) band 0..112 | 0x000000 | 0x050505 | — |
| k8t-baseline-2 | (40,43) band 0..112 | 0x000000 | 0x111111 | — |
| k8t-final-1 | (50,125) band 0..112 | 0x1b1b1b | 0x000000 | 851 |
| k8t-final-2 | (40,43) band 0..112 | 0x000000 | 0x171717 | — |
| k8t-patched-2 | (40,43) band 0..48 | 0x2d2b55 | 0x000000 | — |

Every `got` is a legitimate frame colour, never garbage: `0x000000` = `BG_DEFAULT`, `0x1b1b1b` = an anti-aliased `FG_DEFAULT` edge over `BG_DEFAULT`, `0x2d2b55` = the desktop backdrop; the `want` greys (0x05/0x11/0x17/0x1b) are the console's anti-aliased text edges. `desktop_firmware.rs:147` already documents this exact pair (`got=0x1b1b1b want=0x000000`) as "a printing core repaints rows edge to edge … reading a panel this writer had just been over", and `wm.rs:6080` defines `moved=` as a reference that moved UNDER the verifier, counted instead of charged to the blit — `moved=851` on k8t-final-1 is the verifier reporting exactly that. Reading: the equal-count failures are the CONSOLE PRINTING CORE repainting the verified band after `want` was captured — the reference is stale, the memory is self-consistent, no kernel memory bug; host load widens the window (more console lines land inside the verify). `want` is not mis-computed (the pairs are real adjacent frames). CONSEQUENCES: the quiet-box run only removes the pressure and convicts nothing; the fix is verifier-side — quiesce the console writer or re-anchor `want` after the repaint (a re-read before verdict) — in the FIXTURE, not the kernel. The 145/83 run (k8t-baseline-1) is asymmetric and is NOT this shape; it stays a separate, unworked observation (4131's family or a third thing).
