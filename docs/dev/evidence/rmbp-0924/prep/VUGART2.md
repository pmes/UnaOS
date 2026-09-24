# VUGART2 — prep

## The finding

Flight 14 §4 (`docs/dev/evidence/rmbp-0915/flight14/FLIGHT14.md:85-88`): glass "did much much
better, all six opened and started working almost instantly"; wire every `:: VUGART:` line FAIL —
`frames=3265 coherent=3230 torn_rows=4104 mixed_frames=35 -> FAIL`, plus 20+ smaller FAILs.
Flight 15 §2 (`docs/dev/evidence/rmbp-0915/flight15/FLIGHT15.md:28`): "the fixture red 993 lines
and green 75 across four boots (the PASS run is the `frames=N coherent=N torn_rows=0` shape at
342-350 s)"; glass: "running very better". B221 (`docs/dev/OS/rmbp-ledger.md:249`) frames it as a
threshold question, not a mechanism question: does the instrument count tears the eye cannot see
at this rate, or is the flight-11 zero-tolerance threshold (B162, `mixed_frames=0`) simply wrong
for a real bench population.

## Mechanism

`unaos/crates/user-vug/src/main.rs`. Three disjoint raster bands, A `0..108` B `108..216` parent
`216..288` (`SH=288 BAND_MID=108 BAND_PAR=216`, `:1119,1133-1134`), one published projection
(`:1150-1187`). `art_bad_rows(g, abandoned)` (`:1292-1324`) has **two different tests behind one
name**: present-time (`abandoned=false`, from `art_score`) a band is bad if `stale || busy`;
barrier-park-time (`abandoned=true`, from `art_strand`) a band is bad only if `stale && !busy` —
busy-but-still-drawing is normal mid-raster and false-positived frame 1 in the original go-red
(`:1296-1306`). `art_strand(g, passes)` (`:1338-1358`) fires **exactly once**, only at
`passes == BARRIER_SPIN_YIELDS+1` (`BARRIER_SPIN_YIELDS=64`, `:1476`) — the pass where the barrier
stops spinning and is about to park — and emits immediately, since the parent may never return.
`art_score(g)` (`:1362-1384`) runs once per present and emits only on a power-of-two or
`% VUGART_PERIOD(64)` (`:1245`). Call sites: the B162 release fix `PHASE.store(gen, Release)`
(`gen = attempts+1`) at `:3477-3524`; `art_strand(gen, passes)` inside the barrier's post-spin
`else` branch at `:3628-3634`; `art_score(gen)` before present at `:3683-3686`; final `art_emit()`
at exit, `:3816-3819`. Existing pin: `unaos/scripts/specs/x86-wc.spec:594-627`. B162:
`docs/dev/OS/rmbp-ledger.md:204` (QEMU population `frames=2`, go-red `torn_rows=216
mixed_frames=1` = exactly bands A+B, `216 = BAND_ROWS[0]+BAND_ROWS[1]`).

**FLIGHT11.md has zero `VUGART` lines** (`awk '/VUGART/' docs/dev/evidence/rmbp-0922/flight11/FLIGHT11.md`
— no output): the instrument postdates that flight; it's cited there only for Peter's words and the
`[uvug9] stall … rc=9` precondition.

**New this round — the raw logs, not summarized anywhere yet.** `flight14/f14-boot1.log` (512
`VUGART` lines) and `flight15/f15-boots.log` (1068 lines): diffing consecutive same-process rollups
where exactly one new frame and one new mixed frame appear (an isolated, un-repeated bad frame)
gives, verbatim: flight14 `frames=733,769,917,1208,1857,3201,3265 torn_delta=108` and
`frames=3137 torn_delta=216`; flight15 `frames=2,76,594,1089,1345 torn_delta=108` (repeats across
4 boots). None of these frame numbers sit on `art_score`'s periodic schedule (power-of-two or
`%64`), so every one is an **`art_strand` immediate emit — a genuine barrier miss**, not a
present-time race. And in every case `df=1, dm=1` back to the previous rollup: **no two are
adjacent** — each stall resolves within the frame it's flagged in, the next frame is clean. That is
why the glass sees nothing: post-B162 `PHASE.store(gen)` always advances, so a stalled band is a
few dozen `SYS_YIELD` passes wide (microseconds, resolved by the barrier's own wake,
`:3560-3597`) — not the flight-11 freeze (multi-second `stall_witness` gaps, because the OLD
release word never advanced and the barrier parked forever). `torn_rows` is a magnitude count, not
a duration count: one frame at 216/288 rows (75%, flight14 frame 3137) is loud on that axis alone
but on screen for one frame interval, not the run.

## Plan

- **M1 — split the counter, not the threshold.** `unaos/crates/user-vug/src/main.rs`: tag
  mixed-frame counters by origin (`art_strand`'s genuine barrier miss vs `art_score`'s present-time
  race) — today `A_MIXED`/`A_TORN` conflate both sites (`:1352-1355`, `:1372-1376`). Witness shape
  unchanged until M2; the FAIL population becomes attributable. Go-red: drop `BARRIER_SPIN_YIELDS`
  to 0 so `art_strand` fires every frame, confirm `strand=` (not `score=`) absorbs the flood.
- **M2 — streak + severity fields.** Same file, `art_score`/`art_strand`/`art_emit` (`:1338-1411`):
  `A_STREAK_MAX` (consecutive mixed frames, reset on any coherent one) and `A_MAX_TORN` (worst
  single-frame `bad_rows`). New line:
  `:: VUGART: frames=<n> coherent=<n> torn_rows=<n> mixed_frames=<n> streak_max=<n> severity=<n> -> PASS|FAIL ::`.
  Go-red: B162's own `PHASE.store(1, …)` mutation must now read `streak_max=2` on the 2-frame QEMU
  population, vs `streak_max<=1` on a healthy run.
- **M3 — streak-bound REQUIRE.** `unaos/scripts/specs/x86-wc.spec`, replacing `:626-627`: gate on
  `streak_max`, not `mixed_frames`, so an isolated self-healing barrier miss (measured: all 16
  isolated events across both flights had `df=1`, none adjacent) stays green while a repeat (the
  actual freeze shape) still reds. `mixed_frames`/`torn_rows`/`severity` stay on the line as
  diagnostics, ungated. Go-red: same M2 mutation — `streak_max=2` trips the new bound (`<=1`).

## Draft code (unbuilt)

```rust
// main.rs, after A_STRAND_GEN (~:1224), M1/M2 — new statics:
#[cfg(target_arch = "x86_64")] static A_STRAND_MIXED: AtomicU32 = AtomicU32::new(0); // art_strand's mixed
#[cfg(target_arch = "x86_64")] static A_SCORE_MIXED: AtomicU32 = AtomicU32::new(0);  // art_score's mixed
#[cfg(target_arch = "x86_64")] static A_STREAK: AtomicU32 = AtomicU32::new(0);       // current run
#[cfg(target_arch = "x86_64")] static A_STREAK_MAX: AtomicU32 = AtomicU32::new(0);
#[cfg(target_arch = "x86_64")] static A_MAX_TORN: AtomicU32 = AtomicU32::new(0);     // worst single-frame bad_rows
```

```rust
// art_strand, after `let bad = art_bad_rows(g, true);` (~:1349): bump A_STRAND_MIXED, A_STREAK
// (latch into A_STREAK_MAX if higher), A_MAX_TORN (latch if `bad` is higher).
// art_score, inside the `if bad_rows != 0 || cl != seen` arm (~:1372-1376): same three bumps using
// A_SCORE_MIXED/bad_rows; in the `else` arm, `A_STREAK.store(0, Relaxed)` (a coherent frame ends the streak).
// art_emit (~:1391-1405): append ` streak_max=<A_STREAK_MAX>` and ` severity=<A_MAX_TORN>` to the line,
// and change the PASS/FAIL token from `mixed == 0` to `A_STREAK_MAX.load(..) <= 1` (open Q2 below).
```

```
# unaos/scripts/specs/x86-wc.spec — replace :626-627, M3:
REQUIRE :: VUGART: frames=[1-9]\d* coherent=\d+ torn_rows=\d+ mixed_frames=\d+ streak_max=[01] severity=\d+ -> PASS ::
FORBID :: VUGART: .* streak_max=[2-9]\d* .* -> FAIL ::
```

## Spec pins

`unaos/scripts/specs/x86-wc.spec`, replacing `:626-627`
(`REQUIRE … mixed_frames=0 -> PASS ::` / `FORBID :: VUGART: .* -> FAIL ::`) with the two lines shown
in the draft above. No regex look-around in either (plain anchors + `\d+`/`[1-9]\d*`/`[01]`/
`[2-9]\d*` classes, matching the existing file's style).

## Open questions

1. **What counts as "the same tear" for `streak_max`.** Counting consecutive *scored* frames
   (strand OR score) conflates a genuinely repeating stall with two independent one-offs landing
   adjacently by chance — unlikely at the observed rate (~1 isolated event / 100-400 frames) but not
   impossible over a run of "hundreds of thousands of frames" (`:1237`). Is `streak_max<=1` the
   right bound, or does it need a second signal (same band index twice) first?
2. **Does `art_emit`'s own PASS/FAIL token move to `streak_max`, or stay `mixed==0` with only the
   spec's FORBID re-keyed?** Changing what the fixture itself prints as PASS/FAIL changes what
   "green" means to every future raw-log reader, not just this gate.
3. **Is `severity=216` (75% of the surface) acceptable at all on its own, independent of streak?**
   It never repeated in either flight, but nothing here bounds `severity` alone — should an
   isolated-but-huge tear FAIL even at `streak_max=1`?

## Next-session start

1. `sed -n '1140,1420p' unaos/crates/user-vug/src/main.rs` to re-orient (re-check line numbers
   against whatever the other executors land first).
2. Land M1, confirm the `PHASE.store(1, …)` go-red still gives `mixed_frames=1`, now attributed
   `strand=1 score=0`.
3. Land M2, re-run the go-red (expect `streak_max=2`), then M3 against the fixture's real field
   order — not the draft order above.
