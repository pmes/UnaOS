# GLASSFIX — SO2 on the glass, SO4 and SO11 re-scored

rmbp-0915, executor GLASSFIX, base `298208ed` (`hw-rmbp`), 2026-09-15.
One x86 QEMU run: `UNAOS_WC=1 UNAOS_QEMU_FULL=1 UNAOS_QMP_SHOT=… ./arroyo test 90`, rc=0,
completion marker at `serial.log` line 1984, wall 114.3 s.

This file is the record the QUEUE §1 closures cite. It carries ONE new thing — the capture the
SO2 ledger row has owed since render8 — and re-scores the two rows beside it, both of which were
already fixed or already dropped in the tree before this executor started.

## What was actually owed

The GLASSFIX brief described three live defects. Measured at its own base commit, none of them
was live:

| row | brief said | tree said at `298208ed` |
|---|---|---|
| SO2 | drop-down misplaced, wrong typeface | fixed by `b768331a` (MENUBAR2) — placement PIXEL-CONFIRMED orin 17; **typeface half unproven on the glass** |
| SO4 | crystal drop-down inset 12 px | fixed by `06ffdaf8` (render9 re-cut) — flown on render14 |
| SO11 | chrome painted at a different width from its surface | `dropped` — orin 17 proved occlusion, not chrome |

So the only debt was SO2's second half, in the ledger's own words: *"the TYPEFACE half is still
unproven on the glass — no capture has a drop-down open"*. That is what this run pays.

## Why no capture had a drop-down open

Not bad luck. `winmenu::selftest` dismisses the menu, closes its window and restores the bar
before it returns, so by the time `qmp_shoot.py` fires at `UNAOS_QMP_WAIT` (arroyo default
`secs - 3` = 87 s) there is nothing on the glass to photograph. Every capture since render8 was
taken over a cleaned-up desktop. `desktop-no-menu.png` here is that shape, from this same tree.

`winmenu::shotmenu_selftest` (this executor, behind `witness`) is the camera's fixture: it opens
the app menu through `strip::press_route` — the same routed seam `selftest`'s leg 2 uses — and
then deliberately does not clean up. No dismiss, no `wm::close`, no focus restore, no
`set_enabled(saved)`.

### Where it had to be called from, which had to be measured

The first cut folded the call at the witness ladder's tail, after `dock::apppin_selftest()`, on
the reasoning that APPPIN is the last fixture that moves focus. **The capture came back empty**,
and the wire said why:

```
:: SHOTMENU: win=1 … aligned=true :: PASS ::            serial.log:1904
[winmenu] dismiss reason=app-owner-change kind=app owner=1   serial.log:1918
```

Fourteen lines. `winx_launcher`, which hosts the whole witness ladder, is only the FIRST of ten
launchers on that task; the eight ring-3 demos chained after it (`winx2`, `winx3`, `winx7`,
`winx8`, `pulsew`, `sock2`..`sock4`, `zeolite`) each spawn windows that take focus, and an
app-owner change dismisses an open dropdown on the next composite. Waiting at the ladder's tail
was the other candidate and is wrong for the same reason: those nine launchers run on that same
task, so a wait there starves the work it waits for.

The call therefore sits on the closing brace of the whole demo chain, after `zeolite_launcher`.
The boot's last focus-moving event is the final `[dock] tile remove … reason=close` of the ring-3
sweep, which lands before `zeolite_launcher` prints; from there to the camera the desktop is
quiescent.

## The wire

```
[winmenu] drop x=40 title_x=40 y=34 bar_h=34 font=chrome20-bold -> ALIGNED
[winmenu] shotmenu name=Glass cell=9x20 bar_glyph=(40,7) menu_glyph=(125,39) drop=143x65+40+34 ::
:: SHOTMENU: win=1 named=true routed_open=true held=true aligned=true dx=0 dy=0 font=chrome20-bold panel=1280x800 :: PASS ::
```

The fixture publishes its own measurement anchors so the PNG scorer reads the kernel's
coordinates instead of hunting for glyphs.

## The pixels — `desktop-menu-open.png`, 1280x800

Scored by `score_shot.py` (pure stdlib, in this directory; output in `score_shot.out`):

```
[px] drop LEFT edge  x=40 (row y=36; x=39 is desktop)  title glyph cell x=40  dx=0
[px] drop TOP  edge  y=34 (col x=111; y=33 is bar)     bar bottom=34          dy=0
[px] "Glass" BAR  (40,7) 45x20 bg=(74, 115, 170) ink=(255, 255, 255)
[px] "Glass" MENU (125,39) 45x20 bg=(236, 236, 238) ink=(37, 38, 41)
[px] alpha delta max=0.009 mean=0.0012 >0.25: 0 of 900
[px] ink px (alpha>0.5) bar=174 menu=174 delta=0
:: GLASSFIX-PX: dx=0 dy=0 alpha_over_0.25=0 ink_delta=0 :: PASS ::
```

**1. Placement, dx = 0.** The drop-down's leftmost painted column is x=40 and x=39 is desktop
background; the bar title's glyph cell origin is x=40. Flush, no inset — the Mac rule.

**2. Placement, dy = 0.** The drop-down's first painted row is y=34, the bar's bottom; y=33 is
still bar. Directly under the bar, no gap.

**3. Typeface — the half that was owed.** The app name `Glass` is painted TWICE by two different
files from two different models: once as the bar's caption (`menubar::compose_row` at
`text_x(0)`) and once inside the drop-down's `About Glass` row (`winmenu::compose_item_row`,
`FLAG_APPNAME`). Same face and weight => the same rasteriser output.

A fixed RGB threshold cannot test that, and saying so is part of the record: the bar draws white
ink on lit-title blue, the menu near-black on chrome face, so identical glyphs land at different
RGB distances. A first pass with a fixed threshold reported "6 px of 900 different", all six on
the `s` bowls, and all six thresholding rather than type. Projecting each pixel onto its block's
own background->ink axis recovers the coverage the rasteriser emitted:

- max alpha delta **0.009** (8-bit rounding of two different blends),
- **0** pixels of 900 differing by more than 0.25,
- ink pixels at alpha>0.5: **174 in the bar, 174 in the menu, delta 0**.

The quantised coverage maps in `score_shot.out` are identical row for row except where a value
sits on a rounding boundary. The drop-down is set in the bar's own type, at the bar's own weight.

### A scorer bug worth recording

The left-edge probe is pinned to the menu's top band (`my + 2`). A scan of the menu's mid-row
reports x=12 and a spurious `dx=-28`: that row crosses the fixture window's own chrome, which
starts at x=12 and carries the red close disc at x=29..38. That was this scorer's first reading,
and it is the same class of mistake as SO11's — furniture in front of the thing being measured,
read as the thing being measured.

## SO11, re-scored in code

No fix and no witness are owed, and the asked-for `FIT|MISFIT` tripwire was declined rather than
forgotten. `wm::outer_box` (`video/wm.rs:14866`) is already the single expression:

```
bw = cw + 2 * BORDER          where cw = r.w * r.scale
```

and every chrome consumer derives from it — the painter (`strip_w = bw - 2*BORDER`, `wm.rs:20452`)
and the drag-handle hit test (`wm.rs:15950`) both. So `chrome_w == surface_w` holds identically;
a runtime assertion could only ever print `FIT`, and its go-red leg would be unreachable without
vandalising `outer_box` itself. The orin 17 pixel reading that dropped the row stands: the ~300 px
chrome was a re-minted console raised above the pulse window, measured 1280 px over a 1280 px
surface on SCREEN0/1/2.

## Files

| file | what it is |
|---|---|
| `desktop-no-menu.png` | the capture shape every run produced before this fixture — bar restored, menu cleaned up, nothing to score |
| `desktop-menu-open.png` | the capture SO2 owed: the drop-down open under its title, in the bar's type |
| `score_shot.py` | the scorer; reads its anchors from the fixture's own witness lines |
| `score_shot.out` | its output on `desktop-menu-open.png`, rc=0 |
