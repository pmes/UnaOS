# MENUSCRAFT (rmbp-ledger B642): the menus on the kit

DESIGN-LANGUAGE §3.7 (menus), §3.2 (the bar's status items), §2.4 (`elev.2`), §2.8 (icons). It builds on
DESKTOPCRAFT (B631), which drew the chrome, bar, dock, wallpaper, toast and login with GLASSKIT (`video/draw.rs`) and
left the menus, the toast's hover ×, theme-change regeneration and the status glyphs owed.
Paint only. Nothing routes differently. The one geometry change is that a dropdown's published rect now includes the
shadow band, and every hit test reads the body inside it (below).

## 1. What each menu paints now

All five dropdown surfaces paint on one **sheet**, `crystal::sheet` (`video/crystal.rs`, at the tail):

| Surface | Where | Painter |
|---|---|---|
| the SHARD menu (the crystal) | `crystal::compose` / `compose_row` | `sheet::row(…, square_top=true)`, then the rows |
| the battery panel | crystal's panel mode | same sheet (no hover pill: these rows are information) |
| every status item's menu (STATUSTRAY: input, network, volume, clock) | crystal's panel mode | same sheet. Slider and month grid unchanged |
| a window's menus (app, File, Edit, Help, Window, tenant titles) | `winmenu::compose` / `compose_row` | `crystal::sheet::row(…, true)`, then the rows |
| the lit title in the bar | `winmenu::draw_bar_row` | a rounded `accent.fill` pill inset 3 from the bar's edges (it was a square box the bar's full height) |

The sheet, row by row (`sheet::row`):

1. **The scene under the band and the rounded corners.** See `sheet::Under` and §4.
2. **`elev.2`, two layers** (`draw::shadow_rrect` twice): `0 3 band−3` (light 18 %, dark 45 %) and `0 0 2` (light 12 %,
   dark 30 %). The wide layer's blur is held to the band, the way DESKTOPCRAFT holds a window's blur to its frame band.
3. **The body**: `surface.overlay`, `radius.overlay` 10 on the bottom corners. The top corners are square because the
   sheet hangs from the bar.
4. **A `line.subtle` hairline** around the body. In Dark there is also `rim.dark` (white at 8 %, inset 1).

Then the content, in body coordinates (the body's origin is the rect's origin):

* **Container padding 5** above the first row and below the last (`crystal::PADY`, exported as `DROP_PADY`).
  The width is at least `MIN_W` 180.
* **The hovered item: an `accent.fill` pill**, inset 5, `radius.item` 5, the item band's full height. The label (and
  the keycaps) use the white `on_accent` ink. A disabled row and a separator never light.
  The hover is latched per pass from `pal::cursor::pos` (the dock's DESKTOPCRAFT `craft_hover` shape) and folded
  into the dropdown's damage signature, so only a hover change repaints.
* **Separators**: a 1 px `line.subtle` line inset 10, centred in an 11 px band (5 + 1 + 5; `SEP_H` was 7).
* **Keycap hints** (window menus; the SHARD tree carries no chords): a chord without spaces (`cmd-x`, `cmd-alt-left`)
  is drawn as one `radius.small` 4 outline per key (`line.control`, or on the pill the white label at 50 %). Each
  key's token is capitalised (`Cmd` `X`), in `text.secondary`. A phrase (`F1 / F2`) is plain `text.secondary` text.
  `sheet::chord_cells` is both the width rule `winmenu::item_glyphs` reserves and the painter's own rule.
* **Labels**: `text.primary`, or `text.disabled` for a disabled row. Text inks pass through `sheet::unfill` and every
  finished row through `sheet::seal_row`, so no dropdown pixel is ever pure white. That is MENU-UNDER's cursor-FILL
  guard, which a white pill label would otherwise have broken.

**Item height** stays `CELL_H + 8` (28 at the 20-px chrome cell). §3.7's 24 assumes `type.body`, and the chrome face
belongs to FONTSCRAFT (B640). `uimetrics_sanity` still pins the 4 px of air.

**"Keycap hints in the caption size"**: no caption face exists yet (type roles belong to FONTSCRAFT). The keycaps
draw in the dropdown's own face (`DROP_FACE` = Chrome) and move to the caption role when FONTSCRAFT adds it. That
is a one-argument change at `sheet::keycaps`'s two call sites (winmenu `compose_row`).

## 2. The geometry the routers share

`sheet::BAND()` = 8 logical px, on the body's right and at its foot only. There is no band on the left, because the
SHARD menu is flush with the panel's left edge. There is none on top, because the sheet meets the bar.

| | before | now |
|---|---|---|
| `crystal::menu_rect`, `panel_rect`, `winmenu::layout_open_rect` | body | body + `BAND` right and below. They decline if the band does not fit, as before for the body |
| `open_rect` (MENU-OCC occluder, `screen::present_background` subtraction, the vacate rect) | body | the same outer rect, so the windows under the menu withhold the band too |
| `crystal::item_at`, `menu_contains`, the panel-mode press, `winmenu::press_at` | the rect | `sheet::body(rect)`. A press on the shadow is a press outside the menu (dismiss) |
| row offsets (`row_top`, `item_top`) | `BORDER` | `BORDER + PADY` |

The fixtures that place presses (`crystal::selftest` legs 3–4, `logout_row_fire`, `winmenu` legs, `wl_row_center`,
SHOTMENU) take the rect's x-centre and `row_top`/`item_top`. Both are still inside the body.

## 3. The status-item glyph/slot table (`video/menubar.rs`)

There is **one table, `STATUS`**, right to left in the Mac's order. `FLOW` is derived from it (its ids), `flow_w`
reads the glyph to get the width, and `flow_slot` places every item `ITEM_GAP` (4) left of its right neighbour. So
the press cells (`tray_box_abs` → `statusmenu::item_at`, `bell_box_abs` → `notify::press_at`, `batt_box_abs`) and the
painter (`status_glyphs_row`) are read off the same rows and cannot drift.

| # (right→left) | item | glyph | width (logical) | press cell | painter |
|---|---|---|---|---|---|
| 0 | clock | `Clock` | `CLOCK_GLYPHS × CELL_W` | — (statusmenu: clock) | `compose_row` text |
| 1 | notify (bell) | `Bell` | `ICON_HIT` 28 | `bell_box_abs` (R116's cell, still routable) | kit: domed body + lip + clapper; **filled** while the Center is open; `accent` dot while unread |
| 2 | battery | `Battery` | `BATT_ITEM_W` | `batt_box_abs` | DESKTOPCRAFT's `battery_row` + percent |
| 3 | network | `Net` | `ICON_HIT` 28 | `tray_box_abs` | kit: dot + three AA arcs fanned ±45° (Wi-Fi); three nodes on a bus (USB); faint (35 %) while down |
| 4 | volume | `Speaker` | `ICON_HIT` 28 | `tray_box_abs` | kit: box + AA cone + up to three AA waves by level; an × when muted |
| 5 | input | `Label` | `2 × CELL_W` | `tray_box_abs` | `tray_paint` text (`US`/`PC`) |

Icons are 16 on the 16-pt grid (`ICON`), centred in their 28-wide, full-height cell, with 1.5-pt strokes (§2.8). The
arcs use `menubar::ring_wedge` (radial and wedge-edge anti-aliasing, integer `isqrt`). Everything else is
`draw::{fill_rrect, stroke_rrect, fill_circle, line_aa}`. The NOTIFY bitmap bell (`BELL_BITS`), the tray's
hand-plotted speaker and the `WiFi`/`ETH` text label are gone. The bell's badge was a `9+` count drawn beside it in a
cell the slot reserved. It is now a dot, so the cell is one hit box like the others. The count is in the Center
(R116, not this arc).

## 4. The scene under the band (and what is owed)

A strip writes the front buffer and never reads it back (VRAM), so the band cannot blend over the real pixels.
`sheet::Under::scan` builds an estimate from the compositor's table, using the `wm::dock_scan` rows that `compose`
already scans for CLOBBER-REPAIR plus `wm::info`. It takes up to 16 visible windows, sorted by z. Each window's
content is `content_fill`, its title band is `chrome_face`, and its frame band is the wallpaper. Everything else is
the wallpaper (`wm::desktop::live_back`, sharp). While the menu is open, MENU-OCC makes the windows withhold the whole
rect, so the estimate is what the glass shows there.
**Owed (compositor, not this lane):** to put exact pixels under an overlay's shadow, the compositor has to give the
overlay the composed scene beneath it. This is the same change as DESKTOPCRAFT's owed window-shadow margin.

## 5. The toast's hover × (`video/notify.rs`, `pass` only)

`pass` latches the hovered card's sequence number. On a hover **edge** it repaints the card it left (without the ×)
and the card it entered (with the ×), and presents each once (`wm::present`). The × is a `surface.raised` disc (18)
on the card's top-left corner, with a `line.subtle` edge, a soft `elev.2` lift and a `text.secondary` cross.
Routing is unchanged: any press on a card except its action button already dismisses it (`REQ_DISMISS`).

## 6. The wallpaper and VEIL follow the theme (`video/wallpaper.rs`)

`generate` records the `theme::epoch` its picture was drawn at (`GEN_EPOCH`). `take_stale` is the `Screen::flush`
hook, and it now calls `regen_on_theme` first. If the installed picture is the generated default and the epoch has
moved (an accent change, or Light/Dark), it regenerates the picture and its VEIL in one `install`. On x86 this runs
on the `wall-regen` worker task (the probe's path); elsewhere it runs inline. It is rate-limited to `RETRY_MS`.
A picture *file* is never regenerated: it does not depend on the theme, and its tint is applied when it is sampled.
Wire: `[wallpaper] regen epoch=<a>-><b> src=procedural`, then the usual `:: WALLPAPER: src=procedural … -> PASS ::`.
**Owed (strip.rs, not this lane):** `strip::seal` folds `theme::epoch`, so the bar and the dock repaint the moment the
theme changes, but they repaint over the old VEIL. The regenerated VEIL reaches them on their next repaint for any
other reason, or through `wm::desktop_repaint`'s full-panel erase if that clobbers them. The exact one-line change is
to fold a wallpaper install generation (a counter `wallpaper::install` bumps) into `strip::seal` beside the epoch.

## 7. Palette names depended on (DESIGN-LANGUAGE §2.3 → today's API)

`crystal::sheet` is the seam, one function per token, which DESIGNAPPLY (B639) re-points:

| §2.3 token | today's call |
|---|---|
| `surface.overlay` | `theme::group_fill()` (the toast card's face, so the two overlays agree) |
| `line.subtle` | `theme::hairline()` |
| `line.control` | `theme::control_edge()` |
| `text.primary` | `theme::title_text_active()` |
| `text.secondary` | `theme::ink_secondary()` |
| `text.disabled` | `theme::title_text_inactive()` |
| `accent.fill` | `theme::accent()` |
| the label on `accent.fill` | `theme::ink_on_accent()` |
| `surface.raised` (the toast ×) | `theme::knob_face()` |
| `elev.2` (the toast ×) | `theme::shadow_soft()`. The sheet's own `elev.2` alphas are §2.4's numbers (`sheet::elev2`) |
| `rim.dark` | white at 8 % (`sheet::RIM_DARK`), §2.3's value |
| scene estimate | `theme::content_fill()`, `theme::chrome_face()`, `theme::DESKTOP_BG` |
| status glyph inks | `theme::title_text_inactive()`, `title_text_active()`, `accent()` |

None of `palette_core`, `wm.rs`, `desktop.rs` CRAFT, `softy/*` or `font_core`/theme type roles is touched.

## 8. The fixture

`tests menuscraft` is registered in `menubar::ensure_tests` beside `tests menubar`, inside the same `DONE` guard
(R80: a verb, never at boot). It needs no `tests.rs` line, because `tests.rs:152` already calls
`menubar::ensure_tests` under the furniture gate. Expected wire on x86 QEMU and the bench (numbers are scale-dependent):

```
:: MENUSCRAFT: sheet=<w>x<h> band=<b> corners=ok face=ok hairline=ok shadow=ok(<near><<scene>,<far>) pill=ok white=0 glyphs=3/3 inside=ok gap=ok(<g>) -> PASS ::
```

At scale 2.0: `band=16`, `gap=ok(8)`, `sheet=376x256`. `glyphs=` counts the icon items seated at the live bar
width; it reads `n/n` with `n < 3` only on a bar too narrow to seat all three.
Go-red, by mutation, for the fold: remove `sheet::seal_row` from `sheet::row`'s fixture use, or make `unfill` an
identity on a Light theme whose `group_fill` is `#FFFFFF`, and the result is `white=<n> -> FAIL`. Drop
`ITEM_GAP()` from `flow_slot` and the result is `gap=BAD`. Paint a glyph one hit box to the left and the result is
`inside=BAD`.

## 9. Not done here (files not in the lane), the exact changes

* **Context menus.** The dock's tile menu (`video/dock.rs` `menu_row`/`menu_box` :3030) and Quarry's file-ops menu
  (`video/quarry/ops.rs` `menu_w`/`menu_h` :300) have their own painters. The change is for each row composer to call
  `crystal::sheet::row(out, outer, j, &Under::scan(..), false)` first (all four corners round, because a context menu
  floats), to grow its rect by `sheet::BAND()` right and below, to hit-test `sheet::body`, and to light the hovered row
  with `sheet::pill`.
* **The strip seal and the wallpaper generation** (§6).
* **The compositor's scene-under-overlay** (§4).

## 10. Build

ONE compile of the leg touched: the x86 kernel `cargo check` under the bench's line (R107/R113/R78; no QEMU, no
matrix). The command and its rc are in the commit body and the ledger row.
