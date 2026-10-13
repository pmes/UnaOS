# TRACKPAD — the rMBP Wellspring pad: mode, stream, scale

Status: **fixed-unflown at the newest rung** (POINTERSCALE2, rmbp-ledger B554). x86_64 only (the pad hangs off EHCI).
Code: `unaos/crates/kernel/src/drivers/ehci/mod.rs` (mode switch, decode, re-arm), `drivers/ehci/finemotion.rs`
(the sub-pixel curve and the one scale), `drivers/ehci/tpgest.rs` (Principia's `system.trackpad.speed` gain, after
the curve), `pointerfeel.rs` (`tests pointerfeel`). Ledger rows (`docs/dev/OS/rmbp-ledger.md`): B459 TPMODE (line
492), B496 FINEMOTION (529), B535 POINTERFEEL (568), B540 POINTERSCALE (573), B541 PUMPHOLE (574), B554
POINTERSCALE2 (589). Peter's standing order is R103: *"i hope you can get the damn mouse fixed once and for all"*
(`docs/dev/RULINGS.md` R103).

Wire captures cited below: `docs/dev/evidence/rmbp-0915/flight32/f32-boot1.log` (image 24) and
`flight33/f33-boot2.log` (image 25), cited `file:line`.

---

## 1. The mode switch (TPMODE, B459)

The pad powers up in HID (boot-mouse) mode, id `0x02` reports with coarse integer deltas. Multi-touch needs the
vendor mode: a class GET_REPORT / SET_REPORT pair on EP0 at `wValue=0x0300`, then a readback GET.

- **The defect.** On all six boots of flights 24/25 the switch tried `wIndex=1` (the interface) first; the device
  took the request and never answered the data stage, 2 s for the GET and 2 s for the SET, and the retry at
  `wIndex=0` latched silently. Trackpad-armed sat at 6.0–6.3 s against kbd-armed at 1.9–2.3 s (B459 status cell).
- **The fix.** Index 0 first (FreeBSD `wsp.c` TYPE2 and the flights 11/24/25 capture), `wIndex=intf` kept as the
  fallback. Three requests, not the row's two: GET, SET, readback GET.
- **Wire, every flown boot since:**
  `:: TPMODE: requests=3 timeouts=0 first=index0 latched=yes armed_ms=1658 bound=2500 … -> ok ::`
  (f32-boot1.log:26; `armed_ms=1657` at f33-boot2.log:28). The three `[tp] mode req=1..3 … widx=0 wValue=0x0300
  status=ok got=8b` lines precede it (f32-boot1.log:23–25).

## 2. The register latches; the sensor does not follow (POINTERFEEL, B535)

`latched=yes` is the REGISTER. It is not the stream. B535 read flights 28/30/31: `[ptr] fine … vendor=0` and f31
`ids=02:229,44:0` — every report on the legacy id, none on the vendor lane, with readback latched every boot. The
vendor stream had come on flights 13–20, when the index-0 write landed about 4 s after the report descriptor
(behind two NAKed attempts); since TPMODE the write lands ~1 ms after it and the stream has not followed — in
B535's words, "the register latches, the sensor engine does not follow".

**The re-assert.** The first `[tp] mode-mismatch readback=vendor stream=legacy` asks for a re-assert after the
endpoint walk (`tp_mode_reassert`, mod.rs:19733): GET, SET NORMAL `0x08`, settle 5 ms, SET VENDOR `0x01`, settle,
GET — the off-to-on edge, at most 2 per boot 2 s apart. It works on the wire:

```
f32-boot1.log:272  :: EHCI-HID: [1] [tp] mode-mismatch readback=vendor stream=legacy -> route=legacy (the wire beats the register) == witness ::
f32-boot1.log:281  [tp] mode reassert n=1 why=stream-legacy addr=8 if=1 was=0x01 normal=ok vendor=ok readback=0x01 readback_ok=true latched=yes ms=12
f32-boot1.log:305  [ptr] fine ep=1 n=101 moved=100 carried=1 lost=0 floor=none rel=1 vendor=101 -> OK
```

One legacy report, then the vendor lane carries everything (`rel=1 vendor=101`). The same pair repeats on image 25
(f33-boot2.log:276, :285; `rel=2` there, f33-boot2.log:313). Boot-time TPMODE is unchanged (`requests=3`).

## 3. The report stream and its gaps (POINTERFEEL, PUMPHOLE B541, POINTERSCALE2)

**Why a new fixture.** On flight 31 every pointer fixture passed (HIDSTALL, FINEMOTION 200/200, INPUTSTALL) while
Peter said the mouse was "completely fucking terrible in nearly every way" (B535): the fixtures inject deltas or
time the kernel's own path; none counted the pad's real stream under a finger. `tests pointerfeel` does: a 5 s
window that opens on the first finger-down report, every report both EHCI arms install, every gap over 16 ms named.
PASS needs `reports>0`, `route=mt`, and no gap the kernel caused.

**Flight 32 — the first count** (f32-boot1.log:2095):
`:: POINTERFEEL: reports=246 rate_hz=49.2 gaps=11 gap_max_ms=2016 worst_holder=unheld/step=bootlog route=mt -> FAIL
… fr=inline …`. The first gaps were 17–19 ms (f32-boot1.log:2096–2098).

**STARVED vs SILENT (PUMPHOLE).** `step=bootlog` was a sample of where the usb-pump sat, not a hold: the census named
no hold in that step all session, and the hid-pump is its own PRIO_NORMAL task preempted at `QUANTUM_TICKS=4`
(sched.rs:174) (B541). The fixture now times the hid pass itself and classes each gap over 16 ms STARVED (the pass
gap also over 16 ms, or the pump core held) or SILENT (the pass polled on cadence; the pad sent nothing). The verdict
counts STARVED only. PUMPHOLE also moved the flight recorder's body off the pump (`fr=inline` → `fr=worker`): the
`flight_recorder::service` gate returned at `block::info().is_none()` before the worker hand on a card-only boot.

**Flight 33** (f33-boot2.log:1687):
`:: POINTERFEEL: reports=244 rate_hz=48.8 gaps=3 gap_max_ms=1001 worst_holder=silent/pass_max_ms=2 route=mt …
carried=0/241 carried_pct=0 -> PASS :: … fr=worker:c2 … starved=0 starved_max_ms=0 silent=3 silent_max_ms=1001
pass_max_ms=8 pass_bound_ms=16`. The kernel starved nothing; the pad itself went silent for a second, three times,
and the fixture was green at about 49 reports a second.

**The rate rule (POINTERSCALE2).** A green fixture at under half the pad's frames is wrong. The vendor endpoint is
`mps=64 interval=2` in the corpus (B554; full-speed would be 500 Hz). The fixture now prints `expected_hz=
binterval= speed= rate_pct= rate=<ok|LOW>` and FAILs below 80 % of the bInterval rate. Unflown.

**Depth refused by the silicon.** N transfers in flight for the pointer (the xHCI keyboard's shape) cannot be built
here: both rMBP functions run overlay-direct and the controller HSEs on a qTD fetch, so the endpoint holds ONE
transfer (B535 (4), B554 (3); mod.rs IntEp doc and the ISRARM block). What POINTERSCALE2 fixed instead: `isr_rearm`
flipped the data toggle once per TRANSFER while the device advances it once per PACKET, so every multi-packet TYPE2
frame (two/three fingers, 86/114 B over MPS 64) desynchronised the toggle and lost the next frame. The flip now
follows the retired token's packet parity (`isr_dt_flip`, mod.rs:19789).

## 4. Fine motion: carry, never floor (FINEMOTION, B496)

Flight 27, Peter: *"once i try to move with accuracy it basically will not move."* `tp_scale` divided each frame's
raw delta by 8 toward zero and threw the remainder away; a slow finger moves 1–7 units a frame, so every such frame
was 0 px. Now the curve runs in Q8 sub-pixels and the remainder is carried per axis while the finger stays down,
reset on lift and on a two-to-one-finger re-baseline. Witness: the first 20 deltas `[ptr] delta …`, then one
`[ptr] fine …` per second while deltas flow (`finemotion.rs` header; `WIT_MAX = 20`, `SUM_MS = 1000`).

## 5. The one-scale rule (POINTERSCALE, B540) and its drift bug (POINTERSCALE2, B554)

**Rule: one gain, from one fixed range, applied to the vector.** Three things broke it in turn.

1. **Anisotropy.** Flight 32: `[ptr] delta route=vendor in=-18,-51 scaled=-2.25,-12.00` (f32-boot1.log:282).
   FLIGHT32.md §3 read this as x÷8, y÷4; B540 corrected it from the code: TPSPEED's two-slope curve (÷8 up to a
   knee of 24, ÷3 above) ran PER AXIS, x below the knee and y above it, so every fast diagonal bent toward its
   longer leg. `step2` (finemotion.rs:162) now evaluates the curve once on the frame's magnitude and applies that
   gain to both legs. Image 25 shows it: `in=-25,-25 scaled=-7.50,-7.50` (f33-boot2.log:292).
2. **The ±37/38 ceiling.** mod.rs' `TP_MT_MAX_STEP` clamped each axis at 128 units a frame, curve(128) = 37.67 px:
   `[tp] mt route=vendor … dx=38 dy=-37` (f32-boot1.log:338). A faster finger moved no faster. B540 removed it and
   re-baselined a JUMP (over a quarter of the range, or the first frame after 250 ms of silence) instead.
3. **The guessed scale.** 8 units/px was a constant. B540 derived it from the pad's measured x extent: one slow
   pad-width stroke = `PAD_PX` = 1440 px, the panel's logical width (finemotion.rs:56), seeded with flight 32's
   `-3243..4069` = 5.07 u/px.

**The drift bug.** B540's range was WIDEN-ONLY and live, so every new extreme lowered the gain for the rest of the
session. Flight 33, Peter: *"mouse even less responsive than ever!"*:

```
f33-boot2.log:278   [ptr] fine … rebase=0 div=5.07u/px range=x-3243..4069,y-141..6165   (06:00:03)
f33-boot2.log:1691  [ptr] fine … rebase=5 div=5.55u/px range=x-3243..4751,y-141..6165   (06:00:54)
f33-boot2.log:2198  [ptr] fine … rebase=9 div=5.84u/px range=x-3243..5182,y-141..6165   (06:02:25)
```

About 15 % slower in under two and a half minutes. Meanwhile the quarter-range jump rule was DISCARDING fast strokes
(`rebase=8` by 06:01:36, f33-boot2.log:1889).

**The cure (B554, unflown).** `PAD_RANGE_X = (-3243, 4751)`, `PAD_RANGE_Y = (-141, 6165)` (finemotion.rs:63–64): the
OBSERVED extent across f32/f33, a named constant, never re-derived. One scale for the life of the boot: 7994 u =
1440 px, `div=5.55u/px` (finemotion.rs:65). The live extent is only printed (`range_seen=`) and read by nothing. A
fast step is CLAMPED in vector magnitude at `MAX_STEP = PAD_SPAN / 8` = 999 u (finemotion.rs:71, `clamp` at :117),
direction kept, counted `clamped=`; `rebase=` counts only a true re-touch (`is_jump`, :175). `tests finemotion`
proves both (`jump=clamped retouch=rebased`).

## 6. Open

- The glass verdict on B554: the fixed scale, the clamp, the toggle parity and the rate rule have flown nowhere.
- `PAD_PX` (the speed before Principia's gain) and the 250 ms re-touch bound are guesses for the glass (B540).
- The pad's report descriptor carries no logical range for the finger fields, so the range is observed, not
  declared (finemotion.rs doc above `PAD_RANGE_X`). A pad on another model would need its own row.
- The 1001 ms SILENT gaps (f33-boot2.log:1687) are the pad's; nothing on the wire says why it went quiet.
