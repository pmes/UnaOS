# GLASSFIX2 — SO5 measured and BLOCKED; SO12 / S15 already fixed in tree, re-measured on this one

rmbp-0915, executor GLASSFIX2, base `c1e71690` (`hw-rmbp`), 2026-09-15.

Two runs carry this file: one x86 QEMU run,
`UNAOS_WC=1 UNAOS_QEMU_FULL=1 UNAOS_QMP_SHOT=… ./arroyo test 90`, **rc=0**, completion marker at
`serial.log` line 2045; and one aarch64 run, `UNAOS_PIDESK=1 ./arroyo kernel8-test`, for the one
measurement the x86 desktop structurally cannot take.

**Neither row was open in this tree, and the two are blocked in opposite directions.** SO12 and S15
were fixed by a commit that is an ancestor of this base and are re-measured here at HEAD. SO5 is
REAL and still live — on aarch64 — and its one-line fix is in `crates/kernel/src/pal.rs`, a file
this brief did not name. **That change was NOT made. It is stated exactly, below, for the grant.**

| row | brief said | tree says at `c1e71690` |
|---|---|---|
| SO5 | pointer sprite changes size over the desktop backdrop | **LIVE on aarch64, NOT LIVE on x86** — `pal::cursor::SPRITE_OWNS_PAINT` is `cfg!(target_arch = "x86_64")`, so the second sprite never paints here. Fix is one line in `pal.rs`: ⚠ **STOP — out of this brief's file list** |
| SO12 | boot cascade opens the console over the pulse window's title bar | ✓ FIXED IN TREE by `9b1a0605` (CASCADEFIT), `git merge-base --is-ancestor 9b1a0605 HEAD` rc=0. Re-measured at HEAD: `overlap_rows=0 -> FIT` |
| S15 | pulse window overlaps the console on both aarch64 boards | ✓ FIXED IN TREE by the SAME commit and the same expression — same measurement, same line |

---

## SO5 — two sprites, not one that grows

### The mechanism, from the code

There are **two arrow sprites** in this kernel, drawn at two different block scales:

| painter | buffer | extent | where |
|---|---|---|---|
| `video::cursor` | FRONT (scan-out) | `(ui::BASE_CELL + 1) · s` = **9·s** | `cursor::draw_locked`'s `side`, `s` from `cursor::block_scale(fb)` = `Metrics::for_height(PANEL height).scale` |
| `pal::cursor` | whatever back buffer the caller owns | `9 · sprite_scale` with `sprite_scale = metrics().scale + 1` = **9·(s+1)** | `pal.rs` `cursor::extent` / `cursor::paint` |

Which one the operator sees is decided by the desktop present's occluder subtraction
(`screen::present_background` → `next_visible_span`), not by either painter. Over a **window** the
back buffer's spans are covered and only the 9·s arrow lands. Over the **backdrop** nothing covers
them and the 9·(s+1) arrow does. Cross a window edge and the size toggles. **Nothing grows — a
bigger sprite stops being covered**, which is why reading the draw code for a resize finds nothing.

The compositor's own scale is derived from the **panel**, never from the surface
(`cursor.rs:1646`), so WC-D's per-window integer upscale reaches a window's content and stops at its
frame. There is no per-surface cursor scale anywhere in `wm.rs` or `wcg.rs` to fix; the divergence is
the `+ 1`, and it is in `pal.rs`.

### Why x86 reads `same=1` and what that is worth

`pal::cursor::SPRITE_OWNS_PAINT` is `cfg!(target_arch = "x86_64")` (`pal.rs:301`). On x86 it is
**true**, so `draw` / `draw_over` / `repaint_on_move` hand off to `video::cursor` and
`pal::cursor::paint` never runs: **one sprite, one size, over every surface.** The rule holds here
by construction. On aarch64 the constant is **false**, both painters are live on one panel, and at
the Orin's 1920x1200 (`Metrics::for_height(1200).scale == 1`) the pair is 9x9 and 18x18 — exactly
2x, which is the ratio Peter saw. That reading is already on the record from the other side:
`arch/aarch64/display_tegra.rs`'s `[sprite]` witness printed
`size=18x18 scale=2 over=desktop … compositor=9x9 backbuffer=18x18 same=0 n=8/8` on render8.

So the x86 `same=1` is **not** a claim that SO5 is fixed. It is the measurement that the rule the fix
must satisfy is *already satisfied on this arch by a different mechanism*, and it is the scoring
instrument the aarch64 fix will be graded by when it is granted. The go-red below shows the
instrument reads `same=0` the moment the second sprite is live.

### ⚠ STOP — THE FIX, EXACTLY, AND WHY IT WAS NOT MADE

The brief's FILES list is `video/wm.rs`, `wcg.rs`, `pulsewin.rs`, `strip.rs`, the evidence directory,
the ledger, the queue and the UX doc. **`crates/kernel/src/pal.rs` is not on it, and it is not even
under `video/`** — it is shared kernel core. MANDATORY HEAD rule 3: *"A fix that needs another file:
STOP and report the exact change; do not make it."* So, for the grant:

```rust
// crates/kernel/src/pal.rs, mod cursor, line 382-384 — the ONE line.
    fn sprite_scale(pal: &impl GneissPal) -> usize {
-       pal.metrics().scale + 1
+       pal.metrics().scale
    }
```

**Converge DOWN, not up**, and that direction is a correction of the SO5 row's older text. The row's
`WAS:` half says *"converge UPWARD (cursor.rs → sprite_scale)"*; its CURRENT status paragraph
(orin 17 PIXELS, 2026-09-06) overrules it with a metal measurement: *"A 9-px-tall arrow is the
`compositor=9x9` regime, i.e. the OVER-A-WINDOW side of the divergence pinned on metal, and it is the
side the `pal.rs` convergence must not change."* The 9·s arrow is the one that has been photographed
on the glass; raising `video::cursor` to 9·(s+1) would move the sprite everyone has already seen.

Two collateral facts the grant should price in, neither of which this seat may settle:

1. `sprite_scale`'s doc comment is a **deliberate design statement** — *"one step above the text
   scale so the cursor reads at a glance (16 px at the 480p QEMU panel, 24 px on the 2880×1800
   Retina)"*. Removing the `+ 1` retires that intent. It is a taste call, and Peter is the taste
   gate.
2. The structural alternative is making `SPRITE_OWNS_PAINT` true on aarch64 and retiring the
   back-buffer sprite outright — the CURSOR-13 ownership arc, which drags the Pi/Orin render pump's
   save-under bracket with it. **That is the better end state** (§9 of `ui_guidelines.md`: a second
   sprite is the defect even when its scale agrees), and it is a bigger change than one line. Both
   are filed in `DESKFIX-video.patch` per `display_tegra.rs:5300`.

Nothing was changed for SO5. The row stays **open**.

---

## SO12 / S15 — fixed in tree, and the rule is not the one the brief assumed

### Already fixed, proven by content

```
$ git merge-base --is-ancestor 9b1a0605 HEAD && echo ANCESTOR
ANCESTOR
9b1a0605 desk/cascade: FIT — the console's boot rect ends above the pulse's title band
         (61-row overlap on render8); witness [deskcascade] fit ... overlap_rows=
```

CASCADEFIT's mechanism, verified by reading it at HEAD: `pulsewin::boot_keepout_top`
(`pulsewin.rs:998`) publishes the pulse's prospective box top less one `wm::BORDER * 2` gutter;
`fbcon::console_work_bottom` (`fbcon.rs:3137`) caps the console's work-area bottom at it; the
console's HEIGHT is capped after the staging-budget loop and **its width and its `x` never move**.
That is a fix IN PLACE, not a relocation (LAWS §6).

### Re-measured at HEAD, on the board the rows are written about

The x86 desktop cannot take this measurement and says so honestly: `pulsewin::ever_armed()` is false
on a `desktop_uefi` desktop (its only non-dock caller is `desktop_firmware::activate`, which x86
never reaches), so `boot_keepout_top` is `None`, the cap has no subject, and `fbcon::CONSOLE_WIN`
is `WIN_NONE` — the x86 gate run prints no `[deskcascade] fit` line at all. So the aarch64 leg took
it, on this tree:

```
$ UNAOS_PIDESK=1 ./arroyo kernel8-test        # QEMU raspi4b, 640x480
[pulsewin] open win=2 panel=640x480 surf=426x120 box=436x164 at (10,230) view=Pi LED lamps
[deskcascade] fit console=570x220+35+0 pulse=436x164+10+230 overlap_rows=0 -> FIT
```

**0 overlapping rows**, against CASCADEFIT's own recorded A/B at this panel — `570x396 at (35,4)` →
`570x220 at (35,0)`, overlap **164 → 0**, the pulse box unmoved. The console box this run reports is
`570x220+35+0` to the pixel: the fix is in and is doing what it said.

S15 is the same expression on the same pair. Its own row had already re-measured the *prompt* half
away on render6 (overlap 0) and left only the console **frame** overlap, 993x24 px; CASCADEFIT's cap
is what closes that, and `overlap_rows=0` is the whole claim.

`UNAOS_PIDESK=1 ./arroyo kernel8-test` exits 1 on `[wc-fv] focus-vis … -> FAIL` (125/126 required
witnesses, 3 forbidden hits). That is the pidesk leg's **known baseline red**, named in SO12's own
row (*"pidesk legs known-red at baseline"*), and it cannot be this arc's: every line GLASSFIX2 adds
is `#[cfg(all(target_arch = "x86_64", …))]`, call site included, so the `kernel8.img` this run booted
contains none of it.

### The Mac rule this desktop actually implements — stronger than a cascade

The brief states the rule as *"a cascade offsets each new window by a fixed step from the previous
one so every title bar stays visible"*. **`wm::place` is not a cascade. It is a FLOW TILER**: rows go
left to right at `theme::GAP` spacing and wrap (`wm.rs:24052`ff), so two tiled boxes cannot share a
pixel — strictly stronger than a fixed-step offset. The only arm that could collide is the
last-resort clamp, and TILEFIT already walks a clamped row UP the work area until its box is
distinct, reporting `[wm] tile-fit … alias=none -> DISTINCT` (**47 lines, 47 DISTINCT** on the gate
run).

The windows that can collide are the ones `place` SKIPS — `if !r.used || r.compat || r.pinned
{ continue; }` — i.e. pinned rows, i.e. the pulse monitor. That is exactly why SO12/S15 needed a
keep-out rather than a cascade step, and it is why this fixture measures **both** populations.

---

## The fixture

`wm::glassfix2_selftest`, behind `witness` (+ `x86_64` + `wc`, matching its host), defined at
`video/wm.rs`'s tail and **called by a same-line fold on `dmgovlp_selftest`'s last statement** —
`focus_reset(); glassfix2_selftest();`, ahead of that line's first `//`. `wm.rs` compiles into the
knob-off `kernel8.img` whose byte-identity is the Pi track's standing proof and panic `Location`s
embed line numbers, so the file is N→N above its own tail (PARITY.md §5.3, LEDGER P7 — a statement
appended AFTER the `//` would be a comment, would compile nothing, and would leave the check green).
The witness ladder's own call sites are in `arch/x86_64/syscall.rs`, which DIRNS holds and which has
not reported, so the call is hosted inside `video/` as the brief required.

### Two corrections the fixture only got by measuring itself

**1. `overlaps=0` over the empty set is not a measurement.** The first cut read the table as it found
it. `dmgovlp_selftest` has just closed its own six rows, so the wire came back:

```
:: GLASSFIX2: … cascade overlaps=0 … n=0 … -> PASS ::          serial-run1.log
```

A rule satisfied over zero windows. The fixture now **mints four rows through the real `create`** and
lets the tiler place them (a geometry the fixture chose would be testing its own arithmetic), then
closes them: `minted=4/4 n=4`.

**2. A detector that can only print zero is not a detector.** Before the rows come down, one of them
is walked ONTO another's title band through the real `move_to` verb — the same seam a drag uses — and
the scan is re-run. `control=1` is the fixture reporting that it SAW the violation it is looking for.
`control=0` red-lines the verdict even when the live scan is clean, because a clean scan from a blind
detector is precisely the failure this arm exists to catch.

### The verdict line

```
:: GLASSFIX2: sprite same=1 backdrop=9x9 window=9x9 scale=1 owns_paint=1 compositor=9x9
   backbuffer=18x18 | cascade overlaps=0 worst=win0-over-win0:0rows minted=4/4 n=4 control=1
   panel=1280x800 | console=win0 pulse=win0 pulse_over_console=0 -> PASS ::
```

`console=win0 pulse=win0` is `WIN_NONE` twice — the honest reading of a desktop that opened neither,
and the field that tells a later reader which desktop the line came from.

---

## The capture, and what it can and cannot score

`desktop-glassfix2.png` (1280x800) is the QMP screendump of the gate run, taken at `UNAOS_QMP_WAIT`
(= `secs - 3` = 87 s). `score_shot.py` extends GLASSFIX's scorer with three legs and keeps its SO2
leg, so one script scores the whole desktop-glass arc. `python3 score_shot.py desktop-glassfix2.png
<serial.log>` → **rc=0**, output in `score_shot.out`:

```
:: GLASSFIX-PX:  dx=0 dy=0 alpha_over_0.25=0 ink_delta=0 :: PASS ::            (leg A, inherited)
[B] close disc at (29,56) painted 11x24 of 24x24 area=190 cover_x=40 -> COVERED-BY-MENU
[B] discs=1 covered_by_window=0
[C] re-derived from the PNG: h=800 -> scale=1 compositor=9x9 backbuffer=18x18 owns_paint=1
[C] independent arithmetic AGREES with the witness
[C] fixture preconditions: minted=4/4 rows_scanned=4 armed_control=1
[D] [wm] tile-fit lines=47 aliased=0
:: GLASSFIX2-PX: discs=1 covered_by_window=0 sprite_backdrop=9 sprite_window=9 same=1
   cascade_overlaps=0 pulse_over_console=0 windows=4 minted=4 control=1 deskcascade=0/0FIT
   tilefit=47/47DISTINCT wire=PASS :: PASS ::
```

**Leg B distinguishes a menu from a window, and it had to.** SO12's operator-facing complaint is that
a covered title bar makes the close disc and drag handle unreachable, so the pixel test is the
close-disc census: find each `theme::CTRL_CLOSE` (#FF5F57) component and walk its `CONTROL_BOX` = 24
px span for the first pixel that is neither disc nor title chrome. On this capture the one disc is
painted **11x24 of 24x24** and covered from `x=40`. That is not SO12 — it is
`winmenu::shotmenu_selftest`'s drop-down, which GLASSFIX cut *specifically* to hold a menu open
across this screendump, and the wire gives its rect (`drop=143x65+40+34`) to score against. **A Mac's
menu covers what is under it and the next click dismisses it; a window the desktop opened at boot
over another window's title bar is SO12 and nothing dismisses it.** The first cut of this leg
red-lined on the menu, which is how the distinction got written.

**The sprite cannot be photographed, and that is stated rather than worked around.**
`pal::cursor::HIDE_AFTER_MS` is 1500 ms and the camera fires ~85 s after the last fixture that moves
the pointer, so there is no arrow in any `UNAOS_QMP_WAIT` capture (the GLASSFIX shots carry none
either — measured: zero reddish or isolated-white components). Leg C therefore scores SO5 by
**re-deriving both extents from the PNG's own height** — `scale = clamp(h/900, 1, 4)`, compositor
`(BASE_CELL+1)·s`, PAL `9·(s+1)` — and refusing the wire's numbers if they disagree. It is an
independent arithmetic, not a re-read: a witness that printed its own constant would pass its own
test.

---

## Go-red

The fix under test for SO5 *on this arch* is `SPRITE_OWNS_PAINT` retiring the second sprite, so the
go-red removes exactly that condition — which reproduces the aarch64 regime, the state SO5 describes:

```rust
-   let over_backdrop = if crate::pal::cursor::SPRITE_OWNS_PAINT { comp } else { back };
+   let over_backdrop = back;   // GO-RED (temporary, not committed)
```

Same command, same tree otherwise:

```
:: GLASSFIX2: sprite same=0 backdrop=18x18 window=9x9 scale=1 owns_paint=1 compositor=9x9
   backbuffer=18x18 | cascade overlaps=0 … minted=4/4 n=4 control=1 … -> FAIL ::
arroyo test rc=1
score_shot.py rc=1 — "[C] independent arithmetic DISAGREES with the witness"
```

18 px over the backdrop against 9 px over a window, on a 1280x800 panel at scale 1: the 2x the Orin
photographed. Reverted; `md5sum` of the restored `wm.rs` matches the green tree byte for byte.

The cascade half's go-red is **in the boot**, not in a second run — the armed control above
(`control=1`) is that leg walking a window onto a title bar and confirming the scan catches it, every
boot, rather than once in an evidence file.

---

## Gate

| # | gate | result |
|---|---|---|
| 1 | `cd unaos && ./arroyo check` | **rc=0** (`check-final.log`) |
| 2 | `UNAOS_WC=1 UNAOS_QEMU_FULL=1 UNAOS_QMP_SHOT=… ./arroyo test 90` | **rc=0**, marker at `serial.log:2045`, `:: GLASSFIX2: … -> PASS ::` |
| 2b | `python3 score_shot.py desktop-glassfix2.png serial.log` | **rc=0**, `:: GLASSFIX2-PX: … :: PASS ::` |
| 3 | go-red → `-> FAIL`, `arroyo test` rc=1, scorer rc=1; restored | **as designed** |
| 4 | `bash unaos/scripts/ledger-check.sh` | **rc=0** |
| — | invariants: PULSEQUIT, APPPIN, MENUBAR, WINMENU, SHOTMENU, DOCKID, CRYSTAL-MENU | **all PASS** on the gate run |

### One thing the next seat should not have to rediscover: this box is not quiet

Two earlier runs of the identical gate command exited 1 on fixtures **this arc does not touch**, and
both were host load, not regressions — three or four other seats were building and booting QEMU
concurrently (`loadavg` 9–12):

* run 1 — `[dmgovlp] verdict … drag_evt=0 drag_px=0 relay=0 narrow=0/12 cur=9/12 adopt_stretch=0/4
  -> FAIL`, with `[comp2] pass_us=62330 max_us=167551` against the quiet box's `53034 / 142168`. A/B
  at the same hour with **base `wm.rs` swapped back in**: `drag_evt=5 drag_px=38590 relay=3
  narrow=3/12 cur=12/12 adopt_stretch=4/4 -> PASS`, rc=0 — which looked like a conviction and was
  not. The fixture parks the real sprite and `pal::cursor` auto-hides after 1500 ms; twelve passes at
  167 ms each walk off that cliff, and the whole drag/carry half goes inert together.
* run 2 — `:: S5DRAIN: FAIL — filled=19 …` and `:: SINKDRAIN: FAIL — staged=8 drained=0 …`, both
  green on runs 1 and 3 of the same binary; the S5 ring filled to 19 lines instead of 64 before the
  drain won the race.
* run 3, quieter box, same tree as run 2 — **rc=0, everything green.** That is the gate run.

This is QUEUE §5 / SO7-B26's quiet-box obligation showing up twice in one session. **An A/B against
a base tree is not a control when the two arms ran under different host load**; the control that
actually settled it was re-running the SAME tree.

## Files

* `desktop-glassfix2.png` — the gate run's QMP screendump (1280x800)
* `score_shot.py` — the extended scorer (legs A–D); `score_shot.out` — its run on the capture above
* fixture: `unaos/crates/kernel/src/video/wm.rs` (tail definition + the same-line fold in
  `dmgovlp_selftest`)
* rules: `docs/dev/OS/05_USER_EXPERIENCE/ui_guidelines.md` §8 (SO12 / S15) and §9 (SO5)
