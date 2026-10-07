# FINEMOTION (rmbp-ledger B496) — fine trackpad motion does not move the pointer

**Finding (from the tree + the wire).** Flight 27's `[tp] mode-mismatch readback=vendor stream=legacy -> route=legacy`
is a ONE-SHOT fired by the first report after the mode switch (a stale id-0x02 report the pad had queued); it names
that one report's route, not the pointer's. Every flight that still printed the bootlog lane (13, 14, 18, 19, 20)
shows the same mismatch line FOLLOWED by vendor TYPE2 frames (`[tp] mt fingers=… curve=8/3@24`, 23..315 lines);
from flight 21 QUIETBOOT moved those lines to bootlog, so flight 27's wire is silent on the stream, not legacy.
The pointer therefore rides the VENDOR lane: `TpCensus::mt_step` -> `tp_scale(raw)` -> `tpgest::shape` (gain) ->
`push_pointer_report` -> `x86_ptr_install` -> `cursor::move_rel`. `tp_scale` divides each frame's raw delta by 8
**toward zero with no remainder** ("a sub-divisor jitter frame moves nothing"). A slow, accurate finger moves the
Wellspring sensor ~1..7 units per ~8 ms frame: every such frame becomes 0 px, forever — the dead zone Peter hit.
`tpgest`'s gain carries a remainder, but only of the already-zeroed pixels. Downstream is lossless: the pal ring
folds motion by summing (PTRDEAD), the render fold sums, `move_rel` adds 1:1, and the cursor's `floor_ms=8`
`deferred=` is a PAINT deferral (the position is installed at HID rate by PTRINSTALL2), never a dropped move.
The legacy id-0x02 lane installs its i8 deltas 1:1 — no floor there.

**The seam.** Driver (`drivers/ehci/finemotion.rs`, CHARTER Kernel — driver): the curve is unchanged in SHAPE
(8 below the knee of 24, 3 above) but evaluated in Q8 sub-pixels with the remainder carried per axis on the
endpoint's `TpCensus` (reset at lift and at every re-baseline, as `mt_prev` is). `tp_scale` stays as the integer
reference its TPSCALE self-test pins. No knob, no preference (the gain stays Principia's `system.trackpad.speed`).

**Milestones.**
- M1 — this design; the Q8 curve with a carried residual in `mt_step`; the TPFRAME fixture's expectation follows.
- M2 — the wire: `[ptr] delta route=<vendor|legacy> in=<dx,dy> scaled=<x.xx,y.yy> moved=<0|1> why=<px|carried|two-finger|zero>`
  for the first 20 deltas of a boot, then one `[ptr] fine` summary per second while deltas flow, carrying the
  route evidence (`rel=<n> vendor=<n>` reports by stream) so the route decision is read off every second.
- M3 — `tests finemotion`: 200 one-count and 20 ten-count deltas through `mt_step` (the vendor lane) and an id-0x02
  one-count report through `trackpad_dispatch` (the legacy lane).

**Witness (the next metal flight).** A slow, accurate stroke prints `[ptr] delta route=vendor in=3,1 scaled=0.37,0.12
moved=0 why=carried` then `moved=1 why=px` within a few frames; the summary reads
`[ptr] fine … lost=0 floor=none rel=<n> vendor=<n> -> OK`. `tests finemotion` prints
`:: FINEMOTION: one_count_moved=200/200 ten_count_moved=20/20 floor=none stall_s=0 -> PASS ::`.

**Owed.** The mode switch's stale first report (the misleading `route=legacy` one-shot) is named, not silenced; the
gain/curve values remain glass guesses (TPSCALE/TPSPEED); a resting finger's sensor jitter now accumulates in
sub-pixels instead of being floored — whether that reads as drift is a glass question for flight 28. The
`[lag] stall stage=hid` cadence and INPUTSTALL are HIDSTALL's, not this arc's.
