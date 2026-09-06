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
