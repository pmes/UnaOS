# TPSPEED — prep

## The finding

**TPSPEED (B225).** "mousing is great" (flight 14, `div=8` freshly landed) vs. "the mouse takes a
lot of finger work to get across the screen" (flight 15 §2, same `div=8`) — a reading: 8 is at the
slow edge. Peter (B225): "a velocity term (small deltas by 8, large by less — a two-slope integer
curve, `tpscale_selftest` extended, `curve=` on the witness) or the divisor 6 as the fallback."

**TPDRAG (B219), read in this same doc per B225's note.** "i cannot click and hold with one finger
and drag with the other but doing it one handed works good." All 56 `[tp] mt` witnesses in flight
14's 577 s read `fingers=1` with two fingers down. JOB: capture the raw frame of a two-finger
contact and read the count field; the mover is the finger that MOVES, not index 0.

## Mechanism

`unaos/crates/kernel/src/drivers/ehci/mod.rs`:
- `TP_MT_DIV: i32 = 8` (17897), `fn tp_scale` (17899) — flat toward-zero division, applied in
  `mt_step` (17481-17493) to both `dx`/`dy` after the `TP_MT_MAX_STEP=128` clamp. Witness printed
  at 14189-14196 (`div=`, becomes `curve=`). `tpscale_selftest` (19179-19188) checks flight 13's
  five sample deltas; GO-RED today is `TP_MT_DIV = 1` (comment 19177-19178). Spec pins today:
  `x86-default.spec:375-376` (self-test) and `:475-476` (TPFRAME corpus, `d=` is post-scale).
- Wire layout (cleanroom-cited 17015-17030): `WSP2_HDR_LEN=30` (17037), one record `WSP2_FSIZE=28`
  (17040); `nfinger`@14 (17043), `ibt`@15 (17046); per-record `abs_x`@+2 (17049), `abs_y`@+4
  (17052), `touch_major`@+16 (17055); `WSP2_MAX_FINGERS=16` (17060) clamp. Second record starts at
  `WSP2_HDR_LEN+WSP2_FSIZE`(=58) — already used as `f1` by the two-finger selftest fixture
  (18048-18091: `fingers=2`, finger0 (1500,-2000), finger1 (-400,3100), `touch1` nonzero).
- `Wsp2Frame` (17800-17813) holds only `fingers, button, x0, y0, touch0`. `decode_wellspring_type2`
  (17833-17860) computes `fingers` correctly (clamped 17848-17849) but only ever reads `f0 =
  WSP2_HDR_LEN` (17855-17858) — finger[1..] is decoded by NOTHING live, though the selftest fixture
  proves the frame carries it. `mt_step` tracks one baseline fed only by `f.x0/f.y0`. B219's
  `fingers=1`-always reading means EITHER the pad truly reports 1 for two contacts on this metal,
  OR the held finger sits at finger[0] and finger[1] (the mover) is simply never read — the decoder
  cannot tell these apart today because it never looks past finger[0].
- Capture machinery to reuse: `tp_hex`/`TP_HEX_MAX`/`TP_FIRST_BYTES=16` (17359-17392, truncates
  short of a second record — need up to `WSP2_HDR_LEN+2*WSP2_FSIZE`=86 bytes); `tp_census_rollup`
  (17518-17537, bounded/only-when-moved pattern); `MT_RAW_DUMP_MAX=4` (16961, one-shot pattern).

## Plan

**M1 — TPSPEED, two-slope curve** (chosen over flat 6: flight 14 says 8 is right for its small
precision deltas, flight 15 says the same 8 is slow for large fast strokes; a flat 6 would speed up
BOTH and risk flight 13's "hypersensitive" complaint on precision work again; fallback to flat 6
only if the curve can't be validated against a real capture before Tuesday).
- Files: `mod.rs` `tp_scale`(17899), `tpscale_selftest`(19179-19188), witness(14196-14197),
  `x86-default.spec:375-376,475-476`.
- Witness: `[tp] mt fingers={} x={} y={} dx={} dy={} curve=lo{}/hi{}@{} frame={}` (e.g.
  `curve=lo8/hi3@40`: <=40 raw units / 8, above / 3 — threshold and hi are open questions below).
- GO-RED: `TP_MT_DIV_HIGH = TP_MT_DIV_LOW` (flattens the curve) — new self-test assertion that the
  128-unit sample scales smaller under HIGH than under LOW alone must then fail.

**M2 — TPDRAG, read finger[1], move by the finger that moves.**
- M2a capture (read-only): one-shot dump (pattern: `MT_RAW_DUMP_MAX`) firing the first time
  `decode_wellspring_type2`'s pre-clamp `declared` byte (17848) is `>=2`; new
  `TP_2F_DUMP_BYTES=WSP2_HDR_LEN+2*WSP2_FSIZE`(86) hex buffer sized off it as `TP_HEX_MAX` is off
  `TP_FIRST_BYTES`(17366); called from the `TpRoute::Vendor(f)` site (14189) but needs the raw
  `report: &[u8]` before decode discards it. Witness: `[tp] 2f raw declared={} len={} bytes={}`.
  GO-RED: cap the dump at the old 16 bytes — assertion that the hex covers finger[1]'s `abs_x`
  (byte 60) must fail.
- M2b decode: add `x1,y1,touch1: i32` to `Wsp2Frame`(17800-17813); read them in
  `decode_wellspring_type2`(17833-17860) at `f1=WSP2_HDR_LEN+WSP2_FSIZE` via the SAME
  `WSP2_F_ABS_X/ABS_Y/WSP2_F_TOUCH_MAJOR` offsets applied to `f1` (as the selftest fixture already
  writes, 18054-18060) — no new offset constants.
- M2c mover choice: `mt_step`(17481-17493) tracks `mt_prev0/mt_prev1`; on `fingers==2` the mover is
  whichever finger's `|dx|+|dy|` is larger this frame (ties keep previous mover); pointer moves by
  the mover's scaled delta; the OTHER finger's nonzero `touch` (or `ibt`, OR'd) drives the button.
  `fingers<=1` unchanged. Witness gains `mover={0|1}`. GO-RED: hard-code `mover=0` — a fixture where
  finger 1 moves and finger 0 is held must then wrongly show `dx=0 dy=0`.

## Spec pins

`x86-default.spec` (edit lines 375-376 in place):
```
REQUIRE :: EHCI-HID: TPSCALE self-test: curve=lo[2-9][0-9]*/hi[1-9][0-9]*@[0-9]+ raw=128,88,-60,7,-7 -> px=.* -> PASS ::
FORBID :: EHCI-HID: TPSCALE self-test: .* -> FAIL ::
```
New (TPDRAG):
```
REQUIRE :: EHCI-HID: \[[0-9]+\] \[tp\] 2f raw declared=2 len=86 bytes=[0-9a-f ]+ == witness ::
FORBID :: EHCI-HID: \[[0-9]+\] \[tp\] 2f raw declared=2 len=(1[0-9]|[0-9]) bytes=
REQUIRE :: EHCI-HID: \[[0-9]+\] \[tp\] mt fingers=2 mover=[01] x=-?[0-9]+ y=-?[0-9]+ dx=-?[0-9]+ dy=-?[0-9]+ curve=.* frame=[0-9]+ == witness ::
```
No look-around anywhere; all plain anchored regex per LAWS.

## Draft code (unbuilt)

After `mod.rs:17899` (`fn tp_scale`):
```rust
const TP_MT_DIV_LOW: i32 = 8;
const TP_MT_DIV_HIGH: i32 = 3; // placeholder — see Open questions
const TP_MT_CURVE_KNEE: i32 = 40; // placeholder — see Open questions
fn tp_scale(raw: i32) -> i32 {
    let div = if raw.abs() <= TP_MT_CURVE_KNEE { TP_MT_DIV_LOW } else { TP_MT_DIV_HIGH };
    raw / div
}
```
After `mod.rs:17813` (`Wsp2Frame` struct fields):
```rust
    x1: i32,
    y1: i32,
    touch1: i32,
```

## Open questions

- Does this pad's firmware put `2` at `nfinger`@14 for a genuine two-finger touch, or does B219's
  always-`fingers=1` mean something else entirely? Only the M2a metal capture answers this.
- TPSPEED's knee (`@40`) and `hi` divisor are placeholders — need flight 15's actual frame deltas
  during a fast stroke, which no existing capture has (flight 13's corpus predates that complaint).
- Should the M2c mover choice latch until both fingers lift, instead of largest-delta-this-frame?
  Only a real capture shows which reading avoids flapping.

## Next-session start

1. `grep -n "TP_MT_DIV\|fn tp_scale\|tpscale_selftest" unaos/crates/kernel/src/drivers/ehci/mod.rs`
   to re-orient; implement M1 (curve + self-test + witness + spec edit) — no capture dependency.
2. Implement M2a (86-byte one-shot capture) and get ONE real two-finger frame off the metal rMBP
   before touching `Wsp2Frame`/`mt_step` — M2b/M2c's offsets are a guess until that capture lands.
3. `./arroyo check` after each milestone; land M1 and M2 as separate changes.
