# TRACKPADPANE (rmbp-ledger B412) — MACPARITY row 16: the Trackpad pane, and why two fingers never reached the pointer

## Design (written before the code)

**Finding (the wire, rung 0 read from flights already captured).** The two-finger-hold-and-drag defect (flight 14 §4,
TPDRAG B219) is not the mover logic: TWO-FINGER FRAMES NEVER REACH THE DECODER. A Wellspring TYPE2 raw frame is
`30 + 28*n` bytes (one finger 58, two 86, three 114). The vendor endpoint's mps is 64 and, knob-off (`mtraw` is not
on the metal line), the qTD is armed for `rx_total = mps` with a 64-byte `IntBuf`: EHCI 3.5.3 retires the qTD after
64 bytes, so a two-finger frame lands as a 64-byte head plus a 22-byte tail in the NEXT transfer — neither passes
`decode_wellspring_type2`'s `(len-30) % 28 == 0` gate, both are dropped silently (no motion, no button edge), and the
drag dies the moment the second finger lands. The census on the wire says exactly that:
- flight 14 (Peter tried it): `[tp] ids=02:5,44:0,other:3686 sizes=2/64` — max 64, never 58-and-above-but-legal 86;
- flights 17/18/19: `sizes=2/64`; flights 15/20 (no two-finger attempt): `sizes=2/58`;
- flight 20: 135 `[tp] mt` witnesses, all `fingers=1`; `:: TPRAW2:` (fires on the first DECODED frame with nfinger>=2)
  has never printed on any flight — it sits behind the decode that rejects the truncated frame.
TPDRAG's own fixture (`:: TPDRAG: decoded=true mover_seq=0110 … -> PASS`, flights 17-20) drives `mt_step` with
hand-built frames and so never met the endpoint.

**Capture.** The working state is the pad's own stream: FreeBSD wsp.c (BSD-2-Clause) receives into `WSP_BUFFER_MAX
1024`; our `mtraw` knob arm (IVY) already arms the vendor qTD for the whole buffer. No new capture needed for rung 1:
the next boot's census line IS the capture (`sizes=` max and `TPRAW2`'s raw bytes).

**Rung ledger (the TPDRAG ladder, carried forward).**
| rung | hypothesis | writes | discriminator | status |
|---|---|---|---|---|
| 0 | premise: two-finger frames arrive whole and the mover logic is the fault | none | if FALSE: sizes max = 64 = mps, TPRAW2 silent, `[tp] mt fingers=2` absent | refuted — flight 14 `sizes=2/64`, flight 20 135x `fingers=1`, TPRAW2 never on any wire |
| 1 | the vendor qTD is armed for one packet; a frame > mps is split and both halves rejected | vendor-mt `rx_total = INT_BUF_LEN` (256), `IntBuf` 64 -> 256 B / align 256, ISR slot 64 -> 256 B | CONFIRMS: census `sizes=…/86` (or 114), `:: TPRAW2: fingers=2`, `[tp] gesture kind=drag2|scroll`; REFUTES: sizes max stays 64 with two fingers down | open (built, unflown) |
| 1-alt | the pad sends multi-finger data as several mps packets with a non-short final packet (no short-packet terminator) | — | the qTD would run to 256 and `sizes` would read 256 / ISR `oversize`; | open |
| 2 | with whole frames, TPDRAG's mover logic drives the drag (button = `ibt`, held finger idle, moving finger moves) | none (B219's code) | `[tp] gesture kind=drag2 fingers=2` then motion while the click is held | open |

**The seams (R79).** The five keys are Principia's (`prefs_core::schema`, `system.trackpad.*`); the rules — the speed's
gain table, the legacy `pointer.speed` mapping, the wheel sign — are `prefs_core::trackpad` (shared-core, both rings).
The kernel's Settings pane reads/writes the keys over the bus (the PREFSUI widgets, the capture seam for the slider)
and hands the values to the driver's gesture stage `drivers/ehci/tpgest.rs` (atomics, applied live). Nothing keeps a
second store.

**The speed (read the curve first).** `tp_scale` is TPSPEED's two-slope curve 8/3@24, approved on flight 19. The
speed is ONE gain on its output, `px = (curve * g + residue) / 8` per axis with the residue carried while the finger
stays down: speed 5 is `g = 8` and reproduces today's pixels exactly; the knee and the slopes' ratio never move.
The old R75 3-step `pointer` row (which changed the divisors, i.e. the shape) is retired into the slider: its divisors
are constants again, a stored `pointer.speed` maps once to speed 4/5/7 when `trackpad.speed` is unset.

**Gestures (the driver's tail stage, after `mt_step`).** natural scroll: two fingers down, no click -> the mover's
curve dy becomes `Event::Wheel` detents (sign flipped when natural is off); secondary: a click whose PRESS edge sees
two fingers is `Button(0x02)` (latched to its release, so a second finger landing on a held click stays primary —
that is TPDRAG); tap: down->up within 180 ms with <= 3 px travel and no click -> one click (two-finger tap =
secondary); three-finger drag: three fingers down = primary held, motion from the mover. Each a one-shot-bounded
`[tp] gesture kind=<…>` line.

**Milestones.** M1 the receive fix (rung 1) + the gesture stage + its witness; M2 prefs_core schema rows +
`prefs_core::trackpad` + PREFS-SCHEMA.md; M3 the Trackpad tab (slider via the capture seam, four toggles), load at
login; M4 `tests trackpad`.

**Witness (what the next flight reads, in order).**
1. `:: EHCI-HID: [1] [tp] ids=… sizes=2/<max>` — max 86 or 114 with two/three fingers down (rung 1 confirmed); 64 = refuted.
2. `:: TPRAW2: fingers=2 bytes=…` — the first whole two-finger frame (the capture).
3. `[tp] gesture kind=drag2|scroll|secondary|tap|three-drag fingers=<n> len=<n> …` (first 4 of each kind).
4. `[settings] trackpad.<key>=<v> applied=1` per change; `[settings] slider drag key=trackpad.speed samples=<n> …`.
5. `tests trackpad` -> `:: TRACKPADPANE: speed=<n> tap=<0/1> natural=<0/1> secondary=two-finger three_drag=<0/1> applied=live -> PASS ::`.

**Owed.** Tap thresholds (180 ms, 3 px) and the scroll detent (6 px of curve per detent) are ONE-SOURCE glass
guesses; secondary-click corner variants; momentum/inertial scroll; aarch64 has no Wellspring path (the pane's keys
store, `applied=none`).
