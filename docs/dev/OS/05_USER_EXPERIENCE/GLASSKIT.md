# GLASSKIT — the glass's drawing kit

rmbp-ledger B628. Source: `unaos/crates/kernel/src/video/draw.rs` (`CHARTER: Kernel — wm`), roles in
`unaos/crates/kernel/src/video/theme.rs` (the GLASSKIT block at its tail). Gate: the desktop family
(x86 `wc`, aarch64 `desktop_firmware`), the same as `dock`.

## Why

Before this kit the glass drew with `fill_rect`, `text`, `glyph` and the `ceramic` shading: rectangles and
raster glyphs, with nothing rounded, nothing anti-aliased, no shadow and no hairline. Peter, 2026-10-14:
"settings also kind of looks roughly patched in rather than a smooth apple like look/crafted interface …
it's giving more a early x-windows vibe opposed to refined mac." Those primitives cannot draw a refined
look. This kit can, and every window uses it.

## The rule

**New window code draws with the kit.** A box with a 1-px border made of bare `fill_rect` calls is not
written again. Use `fill_rrect` with `stroke_rrect` (or `group_box`) for a surface, `hairline_h` or
`hairline_v` for a separator, and the control set for a control. A window that needs a size the table does
not hold passes its own geometry. It never re-spells a value from `DIMS`.

## Contract

| property | the kit's answer |
|---|---|
| surface | `Surf { px: &mut [u32], w, h, clip }`: the glass's `u32` `0x__RRGGBB` rows at stride `w`. The destination's top byte is preserved |
| clipping | every op is clipped to the surface and to `Surf::clip` (`set_clip`, `reset_clip`); a shape may hang off either |
| colour | `Ink { rgb, a }`, an `0x00RRGGBB` colour with a straight alpha. A bare `u32` (a theme role) converts as opaque; `Ink::argb` reads an `0xAARRGGBB` word's alpha (the alpha-bearing roles); `Ink::fade(k)` is a disabled control's fade |
| heap | none on any paint path. The shadow's falloff table (≤ 769 B) and the dock seam's `RowKeep` (512 B) are on the stack. Only the `tests glasskit` fixture allocates (its offscreen surfaces) |
| units | device px. `line_aa` takes 1/`SUB` px (`SUB = 16`). The control defaults are logical px, scaled by `ui::px` at paint time |
| anti-aliasing | coverage, not supersampling. A corner pixel's coverage is its centre's signed distance to the arc, in 1/256 px, clamped to one pixel. Only corner pixels are tested; straight spans are filled with no coverage test |
| colour literals | none. `appearance-check.py` holds at `literals_outside_theme=0`. Every kit role is derived from a live token (mixed, lifted or given an alpha), so Light, Dark and the accent move them with everything else |

## Primitives

| fn | what it draws |
|---|---|
| `blend(dst, src, a)` | `dst` moved `a/255` of the way to `src`, per channel (two channels per multiply), exact at 0 and 255 |
| `over(dst_px, src_px)` | Porter-Duff OVER of a straight `0xAARRGGBB` source |
| `fill_rrect(s, r, radius, ink)` | anti-aliased rounded rectangle. `radius` is an `i32` or a `Radii { tl, tr, br, bl }`, held to `min(w,h)/2` |
| `fill_rrect_alpha(s, r, radius, ink, alpha)` | the same at a straight alpha |
| `fill(s, r, ink)` | a square fill through the kit (clipped, alpha-aware) |
| `stroke_rrect(s, r, radius, width, ink)` | anti-aliased stroke drawn inside `r`'s edge, as outer coverage minus inner coverage. Square corners take a four-band fast path |
| `hairline_h(s, x, y, len, ink)`, `hairline_v` | the one-device-pixel line (the fast path of a 1 px stroke) |
| `shadow_rrect(s, r, radius, blur, color_alpha)` | soft drop shadow (see below) |
| `gradient_v(s, r, top, bottom)`, `gradient_h(s, r, left, right)`, `gradient_rrect_v(s, r, radius, top, bottom)` | gradients ordered-dithered with a 4x4 Bayer matrix, so an 8-bit panel shows no banding. The end rows and columns are exact |
| `fill_circle(s, cx, cy, radius, ink)`, `fill_disc(s, r, ink)`, `stroke_circle(s, cx, cy, radius, width, ink)` | discs and rings for glyph-free icons |
| `line_aa(s, x0, y0, x1, y1, width, ink)` | anti-aliased line with round caps. Coverage is the pixel centre's distance to the segment |
| `row_keep` / `row_round` / `icon_radius` | the strip painter's seam: round, after the fact, the corners of a tile another painter filled square, leaving what it drew inside untouched |

### The shadow

The shadow is an analytic falloff, not a convolution. Its opacity at a pixel is
`peak * (1 - smoothstep(-blur, +blur, sd))`, where `sd` is the pixel's signed distance to the rounded
rectangle in quarter pixels. That matches the edge of a Gaussian-blurred box to within a few percent, and it
needs no buffer and no pass per axis. The cost per pixel:

- straight edges: one subtraction and a table read;
- the middle span of a top or bottom row: one opacity for the whole row;
- corners: one integer square root per row, then an incremental step per pixel (no division).

**Contract: the caster covers its own footprint.** The shadow does not touch pixels inside `r` that lie deeper
than `blur`, because the window that casts the shadow is painted over them afterwards. An offset shadow is
`r.offset(0, dy)` with `dy <= blur`. `blur` is held to `SHADOW_BLUR_MAX` (96 device px).

The budget is a 400x300 window at 2x (800x600 device px, blur 32, radius 20) in under 1 ms on the rMBP's class of
CPU. `tests glasskit` measures it on the running machine (`shadow_win_us=`), and the seat scores it on the bench.

## Controls

Each control is one function. It takes a position (and a width where the control has one), its value, and a
`ControlState`, and it returns its hit rect. `ControlState` is `NORMAL`, `HOVER`, `PRESSED`, `DISABLED` or
`FOCUSED`, and focus combines with the others (`HOVER.focus(true)`):

- hover and press tint the face toward the ink (`hover_tint`, `press_tint`);
- a disabled control fades to `disabled_alpha`. It is never re-coloured;
- focus draws the accent ring `focus_ring` px wide OUTSIDE the control.

| control | signature | drawn as |
|---|---|---|
| toggle | `toggle(s, x, y, on, st)` | pill track: the accent when on, `control_track` with a hairline when off. The knob carries a soft shadow and a hairline edge |
| checkbox | `checkbox(s, x, y, checked, st)` | rounded square. Checked: an accent fill with the mark drawn as two `line_aa` strokes in `ink_on_accent`. Unchecked: `knob_face` with a `control_edge` hairline |
| radio | `radio(s, x, y, selected, st)` | disc. Selected: an accent disc around an `ink_on_accent` dot |
| slider | `slider(s, x, y, w, value, max, st)` | a rail with the accent up to the value and a shadowed knob there |
| push button | `push_button(s, x, y, w, label, default, st)` | `w = 0` fits the label. The face is `knob_face`, or the accent for the default action. It has a soft shadow and a hairline edge, and the label is centred |
| segmented | `segmented(s, x, y, labels, sel, st)` | a `control_track` well. The selected segment is a raised `knob_face` pill; unselected segments are split by hairlines |
| text field | `text_field(s, x, y, w, text, st)` | `field_bg` with a `control_edge` hairline and clipped text. Focused: the ring and a caret |
| sidebar row | `sidebar_row(s, x, y, w, label, selected, st)` | selected: an accent pill with a bold `ink_on_accent` label. Hover: a hairline-ink wash |
| group box | `group_box(s, r, title)` → content rect | the inset grouped box: the title above it in `ink_secondary`, the fill `group_fill` (the window face one step lighter), a 1 px hairline at about 12 % alpha, radius `group_radius` |

### The dimensions — named once

`draw::DIMS` is the only place these values are written. They are logical (96-ppi) px unless noted, and they
are proposed defaults on the 8 px grid. The DESIGN-LANGUAGE survey
(`docs/dev/OS/05_USER_EXPERIENCE/DESIGN-LANGUAGE.md`) aligns them at the fold by editing this one table.

| field | value | field | value |
|---|---|---|---|
| `grid` | 8 | `field_h` | 24 |
| `toggle_w` x `toggle_h` | 32 x 18 | `field_radius` | 6 |
| `toggle_inset` | 2 | `field_pad_x` | 8 |
| `check` / `check_radius` / `check_stroke` | 16 / 4 / 2 | `caret_w` | 1 |
| `radio` / `radio_dot` | 16 / 6 | `focus_ring` | 3 |
| `slider_track` / `slider_knob` | 4 / 18 | `row_h` / `row_radius` / `row_pad_x` | 28 / 6 / 10 |
| `button_h` / `button_radius` | 24 / 6 | `group_radius` / `group_pad` / `group_title_gap` | 10 / 12 / 6 |
| `button_pad_x` / `button_min_w` | 14 / 64 | `knob_blur` / `knob_drop` | 3 / 1 |
| `seg_h` / `seg_radius` | 24 / 6 | `button_blur` / `button_drop` | 2 / 1 |
| `seg_pad_x` / `seg_inset` | 12 / 2 | `hairline_dev` | 1 DEVICE px at every scale |
| `icon_radius_pct` | 22 % of the tile's side | `disabled_alpha` / `hover_tint` / `press_tint` | 110 / 10 / 28 (of 255) |

### Roles (theme.rs, derived and never literal)

| role | derivation |
|---|---|
| `hairline()` | `content_text` at `HAIRLINE_ALPHA` 31 (about 12 %) |
| `control_edge()` | `content_text` at `CONTROL_EDGE_ALPHA` 56 |
| `focus_ring()` | `accent` at `FOCUS_RING_ALPHA` 120 |
| `shadow_soft()` / `shadow_window()` | black at `SHADOW_SOFT_ALPHA` (46 Light, 110 Dark) / `SHADOW_WINDOW_ALPHA` (72 Light, 150 Dark) |
| `group_fill()` | `chrome_face` lifted `GROUP_LIFT` 8 per channel |
| `control_track()` | `chrome_face` one sixth of the way to `content_text` |
| `knob_face()`, `ink_on_accent()` | `field_bg` on Light, `content_text` on Dark |
| `ink_secondary()` | `content_text` two fifths of the way to `chrome_face` |

## The proof site

The dock's Trash tile, in `video/dock.rs` `dock2_overlay`, is the call site of `trash_glyph_row`. The change is
one LINE-NEUTRAL line, with the code before the existing comment:

1. `row_keep` saves the pixels at the tile's two row ends;
2. `trash_glyph_row` paints exactly as before, keeping Peter's stick-figure glyph unchanged;
3. `row_round` blends the four corners back by the coverage of a rounded rectangle of `icon_radius(tile)`.

The tile's plate is now rounded and anti-aliased, and the glyph is not touched. The dock's own background plate
is DOCKSHELL2's this wave and is not part of this change.

## `tests glasskit`

`tests glasskit` is a verb (R80) and never runs at boot. It is registered in `tests.rs`'s settings block
(witness + desktop). It draws into an offscreen 256x256 surface and checks:

- **corner**: the corner pixels of a rounded rect are the background, and the body is the ink;
- **aa**: the arc has intermediate values;
- **stroke**: a middle row holds exactly `2 x width` full-ink pixels, at widths 1 and 3, and the top band is full;
- **hairline**: exact length, with nothing beside the line;
- **shadow**: darker at the edge and monotone lighter away from it, along a row, a column and the corner diagonal, with the background past the blur;
- **gradient**: the end rows and columns are exact;
- **dither**: a middle row holds two values;
- **circle**: the centre is inked, the box corner is the background, the rim is intermediate, and a ring lands on its radius;
- **line**: inked on its axis, the background off it, intermediate at its edge;
- **blend**: alpha 0 gives dst, 255 gives src, 128 the midpoint, OVER reads the top byte, and dst's top byte is kept;
- **controls**: every control paints in every state, and the dock seam restores a corner and keeps the interior.

Wire:

```
:: GLASSKIT: corner=ok aa=ok stroke=ok hairline=ok shadow=ok gradient=ok dither=ok circle=ok line=ok blend=ok controls=ok -> PASS ::
:: GLASSKIT: cost clock=<tsc|ms> fill_rrect_ns=<n> stroke_rrect_ns=<n> gradient_ns=<n> circle_ns=<n> line_ns=<n> controls_ns=<n> shadow_win_us=<n> shadow_budget_us=1000 win=400x300@2x blur=32 ::
```

The cost line is a measurement and not a verdict. Under QEMU TCG the numbers say nothing about metal. The
1 ms budget is scored on the rMBP bench.

## The Settings page (SETTINGSCRAFT, rmbp-ledger B630)

`video/settings.rs` is the kit's first whole window. It follows DESIGN-LANGUAGE §3.4 (Settings) on the language's
4 pt grid. It changes no dpi, font or core palette value: every colour is a theme role or derived from one, and the
language's five open decisions are left to Peter.

**Geometry** (logical px; the surface is `metrics::size` of them):

| part | value |
|---|---|
| window | 920 x 560 (`WIN_W` x `WIN_H`); the floor rule for small panels stays (`open` refuses a window larger than the panel) |
| column | 220 wide; ground = the window face 12/255 toward the ink (`SIDEBAR_TINT`: darker on Light, lighter on Dark, as `surface.sidebar` sits against `surface.window`); a hairline at its right edge |
| user card | at the top: a 32 avatar disc (accent, the initial in `ink_on_accent`), the name bold, the role secondary; a press opens Users |
| search | the kit's `text_field` under the card, 24 tall, a `Search` placeholder in `ink_secondary`; it filters the tab names (ASCII case-insensitive substring), Enter opens the first match, Esc clears |
| tab rows | `sidebar_row` pills 28 tall at a 32 pitch, inset 8; a 16 icon tile at 8 from the pill's edge, the label 8 after it; selected = the accent pill with the bold light label |
| pane | title = the tab's name in the Chrome face, bold, in a 60 band; content padding 20; rows 48 tall |
| grouped list | `group_box` per band of rows, inset 4 from its band; a hairline between its rows from 12 in; the label at 12, the control right-aligned to 12 from the box's edge (`CTRL_R`) |

**The tab icons** are drawn as code (no bitmap), each a rounded tile (`icon_radius`) with a glyph in the ink drawn on a
fill: General a gear (slate), Users a head and shoulders and Display a screen on a stand (the first accent), About an
"i" (slate), Login Items a list (slate), Appearance a half-lit disc (the ink's own tile), Trackpad a pad with its click
strip (slate), File Types a page with a turned corner (slate), Notifications a bell (rose). The tiles come from the
accents table. There is no literal.

**Controls, one by one** (all from this kit; none is a bare `fill_rect` box):

| pane | control | before | now |
|---|---|---|---|
| column | tab rows | flat rects, a 3 px focus bar | `sidebar_row` pills with icon tiles; the focus ring after a key |
| General | Volume | flat rail, a square 8 px knob | `slider` (rail grown by half a knob each end, so the knob's centre is the press map's point) |
| General | Mute · Auto-hide dock | 24 px square box | `toggle`, right-aligned |
| General | Trackpad · Apply · Off · Password | flat rect buttons | `push_button` |
| General | Wallpaper | flat rect, a `_` caret | `text_field` (caret, ring) |
| General | Dock position | three flat rects | `segmented_w` |
| Display | Brightness · Blank screen | flat rail, square knob | `slider` |
| Display | Resolution | flat rect and a `v` | a popup: `push_button` with a two-stroke chevron. Open: a raised overlay (`shadow_rrect`, `fill_rrect`, `stroke_rrect`) with the current entry an accent pill |
| Display | Font · Font size | flat segments, `  -` / `  +` | `segmented_w`, `push_button` |
| Users | list | text rows, an accent bar | avatar discs, a row wash for the selection, `push_button`s (Yes is the default) |
| Users | Add form | flat fields, an `Admin: yes` button | `text_field`s, Create the default `push_button`, Admin a `checkbox` |
| About | facts | label / value rows | one centred card: an accent mark tile, the name, the version, the facts around the centre |
| Login Items | Up · Down · Remove · < · > · Add | flat rect buttons | `push_button` (Add the default) |
| Appearance | Light / Dark / Auto | flat segments | `segmented_w` |
| Appearance | swatches | squares, a square ring | discs with a hairline edge, a ring on the chosen one and on hover |
| Trackpad | four switches | 24 px boxes and `on` / `off` | `toggle`, right-aligned |
| File Types | rows | `[icon] description - mime` text | a page icon tile, the description, the mime in the secondary ink; the row lights under the pointer |
| Notifications | Do Not Disturb · Allow · Sound · steps · Style | boxes and flat rects | `toggle`, `checkbox`, `push_button` |

**States.** Hover: polled on the desktop service pass (`settings::hover_poll`, one line in `quarry/live.rs` beside
`settings::service`), which reads the cursor as the dock's hover does. Each paint records the rect of every control it
drew. A repaint happens only when the pointer crosses into another control or tab row, not on every motion sample.
Pressed: a press shows its control's pressed face for `PRESS_MS` (100 ms, `motion.fast`). Focus: the ring shows only after a key
(DESIGN-LANGUAGE §2.7). A text field shows its caret whenever it holds the keys. The wheel reaches `settings::key_route`
first in `quarry::live::key_route`'s chain and goes by the pointer, not the focus (QSCROLL's rule). Over the column it
scrolls the rows; over File Types it pages (`[settings] wheel area=<column|pane> d=<n> moved=<0|1>`). The selected
pill's ease is not done: the window repaints on demand, with no frame clock to step a 120 ms curve on.

**What the kit lacked**, and what this page did about it:

- a fixed-width segmented control: `draw::segmented_w` was ADDED (`segmented` now calls it), so the painted segments are the press router's `x / (w / n)` segments;
- a popup button and a menu overlay: composed here from `push_button`, `line_aa`, `shadow_rrect`, `fill_rrect` and `stroke_rrect`;
- an icon slot in `sidebar_row`: the pill is drawn with an empty label, then the tile and the label go on top;
- a placeholder in `text_field`: drawn after it in `ink_secondary`;
- a grouped list: `group_box` plus `hairline_h` separators (`settings::groups`);
- a title-size face: the Chrome face in bold stands in until DESIGN-LANGUAGE §2.2's type ramp lands;
- a motion primitive (an eased value stepped by a frame clock): none, so there is no pill ease.

**`tests settings`** keeps its line and gains `craft=`. The fixture paints the General pane offscreen in the Light
set and then in the Dark set (it restores the live set, accent and highlight afterwards) and reads pixels back:

- **icons**: every tab's tile has a corner that is not the tile's colour, and the tile and a glyph both show inside it;
- **hairline**: the first box's top edge is the hairline blended over the box's fill, with the window face above it
  and the fill below it, and the separator runs from the row inset with the fill left of it;
- **round**: the box's corner and the selected pill's corner are the ground underneath them. No bare 1-px-bordered
  rectangle is left;
- **sets**: the column differs from the pane, and the pane's face differs between the two sets.

Wire: `[settings] fixture … col_ok=1 craft=ok win=920x560`. On a failure, `craft=` names the first failed check, with
`-dark` added when it failed in the Dark set.
