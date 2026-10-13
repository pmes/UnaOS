# DESKTOPCRAFT — the Una desktop, painted with the kit

rmbp-ledger B631. Code: `unaos/crates/kernel/src/video/desktop.rs` (`wm::desktop`, `CHARTER: Kernel — wm`), declared
at `video/wm.rs`'s tail with `#[path]`; call sites in `wm.rs`, `menubar.rs`, `dock.rs`, `notify.rs`, `login.rs`;
the wallpaper and its VEIL in `wallpaper.rs`. Gate: the desktop family (x86 `wc`, aarch64 `desktop_firmware`), the
same as `draw`; knob-off nothing compiles. Fixture: `tests desktopcraft`.

## Why

Peter, 2026-10-14: "it's giving more a early x-windows vibe opposed to refined mac" · "take gnome for instance. very
pleasant to look at in many ways, albeit an absolute horror under the hood. we want people to see our screenshots and
be strongly compelled to try unaos." GLASSKIT (B628) gave the glass rounded, anti-aliased, shadowed primitives. This
arc applies them to the surfaces a screenshot shows: the window chrome, the menu bar, the dock, the wallpaper, the
toast cards and the login card.

## The rule this arc keeps

**Paint only.** Every box, disc hit box, strip rect, card rect, `ctl_rect` and resize zone is drawn where its owner
puts it. No router or hit test changed. Softy keeps its own painters: `look::is_softy()` is asked first at every
site, and the Una paint runs only in its `else`.

## Per surface

| surface | before | after | where |
|---|---|---|---|
| window frame band | `chrome_face` machined by `ceramic`, a 1 px keyline and a bevel | the wallpaper under the window's soft shadow (the kit's analytic falloff, zero at the band's outer edge) | `desktop::paint_box`, from `wm::paint_window`'s Softy fold |
| title band | Aqua-style gradient head, square except a hard 12 px top cut to flat `DESKTOP_BG` | rounded top corners (anti-aliased), a gentle vertical gradient on Light and flat on Dark, a one-device-px hairline outline and a hairline between the title band and the content | `desktop::chrome_row` |
| window controls | knurled, ceramic-shaded raster discs with glyphs always drawn | anti-aliased discs with a faint rim; × − + glyphs (anti-aliased strokes) only while the pointer is over the group; grey on an unfocused window | `desktop::chrome_row`, hover latched by `desktop::hover_track` |
| title | left-aligned after the discs, the chrome face | centred on the window, the UI face bold, the active / inactive title inks | `desktop::chrome_row` |
| menu bar field | ceramic face, a bevel row, a bottom keyline | the face at 92 % over the wallpaper's VEIL, one hairline at its foot | `desktop::bar_row`, from `menubar::compose_row` |
| battery item | 1 px raster outline and fill | a rounded anti-aliased outline, a rounded fill level (the error ink at 10 % and below, not charging), a rounded nub, an anti-aliased bolt | `desktop::battery_row`, from `menubar::draw_battery_glyph` |
| dock plate | ceramic face, keyline and bevel, corners cut to flat `DESKTOP_BG` | the face at 72 % over the VEIL, rounded at 18 px, the sharp wallpaper outside its corners, a hairline round it | `desktop::dock_plate_row`, from `dock::compose_row` |
| dock tiles | square-ish ceramic tiles, the indicator a 6 px disc in the accent | rounded squares at the kit's icon radius (22 %), a 4 % ink tint under the pointer, the pressed face while pressed, a 4 px running dot (`content_text` at 60 %) under live windows only; no magnification | `desktop::dock_tile_row`, `dock::craft_row` |
| Trash | GLASSKIT's rounded plate under Peter's stick figure | unchanged: `dock2_overlay` runs last, exactly as before | `dock::dock2_overlay` |
| wallpaper | the flat `DESKTOP_BG` unless `WALL.PNG` exists | a generated default (below); a `WALL.PNG` found later replaces it; `wallpaper off` restores the flat colour | `wallpaper::generate`, `desktop::wall_gen` |
| toast card | a flat face in a 1 px box | a rounded (12 px) card over the VEIL with a soft shadow in a 4 px band, a hairline edge, a rounded accent tile where the app has no icon, the kit's push button | `notify::craft_card`, `craft_icon`, `craft_button` |
| login card | flat boxes in 1 px frames, a 2 px inner frame, 2 px rules | the kit's text field (rounded, control edge, the accent focus ring outside), push buttons (rounded, soft shadow), user cards (accent pill when picked, a raised tile otherwise), one-device-px rules; the window chrome is the card's edge | `login::craft_box`, `craft_rule` |

## Dimensions and colours

Every value is in ONE table, `desktop::CRAFT` (logical 96-ppi px, scaled by `ui::px`; strengths of 255). No colour
literal: every colour is a live theme token, mixed (`draw::blend`) or given an alpha, so Light, Dark and the accent move
everything (`appearance-check.py` stays at `literals_outside_theme=0`).

| field | value | meaning |
|---|---|---|
| `win_radius` | 10 | the window's top corners (held to half the body width and the title height) |
| `focus_blur` / `focus_alpha` | 24 / 31 (12 %) | the focused window's shadow |
| `idle_blur` / `idle_alpha` | 12 / 20 (8 %) | an unfocused window's shadow |
| `drop` | 1 | the shadow's fall below the window |
| `title_lift` | 120 | Light title gradient: top = `chrome_face` 120/255 toward `knob_face`, bottom = `chrome_face`; Dark: flat `chrome_face` |
| `idle_disc` | 56 | unfocused disc = `chrome_face` 56/255 toward `content_text` |
| `glyph_dark` | 140 | a disc's glyph = the disc's colour 140/255 toward black |
| `disc_rim` | 40 | a disc's rim: black at 40/255 |
| `bar_tint` | 235 (92 %) | the menu bar: `chrome_face` over the VEIL |
| `dock_tint` / `dock_radius` | 184 (72 %) / 18 | the dock plate |
| `dock_hover` | 10 (4 %) | `content_text` over a hovered tile |
| `dot` / `dot_alpha` | 4 / 153 (60 %) | the running dot (radius 2) in `content_text` |
| `wall_dark` | 90 | wallpaper tone A = `DESKTOP_BG` 90/255 toward black |
| `wall_tint` | 96 | wallpaper tone B = `DESKTOP_BG` 96/255 toward the accent |
| `glow_alpha` | 150 | the glow disc's peak over the gradient |
| `glow_radius_pm` / `glow_x_pm` / `glow_y_pm` | 620 / 300 / 760 | the glow's radius (permille of the panel height) and centre (permille of the panel) |

Other roles used: `hairline()` (title outline, title rule, dock plate edge, card edge, login rules), `control_edge()`
(fields and buttons), `focus_ring()`, `shadow_soft()` (toast card, buttons), `group_fill()` (toast card, login user
tile), `knob_face()` (buttons, the glow's lift, the bolt), `ctrl_close/min/zoom()` (discs), the title inks. The toast
card's band is `CARD_MARGIN` 4 and its radius `CARD_RADIUS` 12 (`notify.rs`); login boxes take `draw::DIMS`'
`field_radius`, `button_radius`, `button_blur`, `button_drop` and `focus_ring`.

### The shadow, and what the compositor allows today

`wm` paints each window straight into the panel's buffer inside its own outer box and repaints only damaged boxes. A
pixel outside a window's box is not repainted when that window presents, so a shadow blended there would darken again
on every present. The shadow is therefore drawn inside the box, in the frame band `wm::BORDER` wide (5 logical px, so
10 device px at 2x and 12 at 2.5x) that the box already owns. The band shows the wallpaper (never a read-back of the
panel) under `draw::shadow_rrect`'s falloff, and its blur is held to the band less the drop, so it reaches zero at the
band's outer edge and draws no hard edge. The CRAFT values (24 and 12 px) are the intent. The band decides the visible
width until the compositor damages and re-composites a margin around every box. That is a compositor change (damage
grown by the shadow margin, and the layers under the margin re-composited in z order before the window), owed and not
taken here. Where windows overlap, the upper window's band shows the wallpaper over the lower window: a thin
separation, the same trade the stock chrome's corner cut to `DESKTOP_BG` made.

### The VEIL

`wallpaper.rs` builds it once per picture (the generated default or a `WALL.PNG`): 8 x 8 cell means, then one [1 2 1]
pass each way over the cells (about a 24 px blur), stored beside the picture. Painters sample it bilinearly between
cell centres (`Back::soft`), at the cost of a fill. It is published to painters as a read-only snapshot (`wallpaper::back`):
they paint masked and possibly on several staging cores, so they never take the wallpaper's lock. A replaced picture
is retired and freed only by the next install.

### The wallpaper

`desktop::wall_gen`, generated once when the default probe finds no picture file (on the probe's worker core on x86):
a two-tone diagonal from tone A (top left) to tone B (bottom right) and a soft disc of the accent (lifted 48/255
toward `knob_face`) at (30 %, 76 %) of the panel, falloff `(1 - d²/r²)³`. Integer only, ordered-dithered (4x4 Bayer),
one table of `w` words. Witness: `:: WALLPAPER: src=procedural WxH=<w>x<h> scaled=<w>x<h> letterbox=0 ms=<n> -> PASS ::`.
On a diskless QEMU boot this line replaces the `src=none` line the probe used to print. The default is not
regenerated on a theme or accent change (owed).

## `tests desktopcraft`

A verb (R80), registered in `tests.rs`'s settings block beside `glasskit`. It paints the default desktop into an
offscreen surface the size of the rMBP panel (2880x1800, or 1440x900 where memory refuses): the generated wallpaper and
its VEIL, the menu bar's field, three windows (Lumen, Settings, Aether from bottom to top, Aether focused and hovered)
with flat content, and the dock with nine tiles. It goes through the same `paint_box` / `bar_row` / `dock_plate_row` /
`dock_tile_row` code the glass runs. Checks:

- **corners**: each window's top-left body corner pixel is the wallpaper under the shadow only (never darker than the
  peak allows, never the title face);
- **shadow**: below each window at mid-width, the darkness (wallpaper minus pixel, three channels) is positive next to
  the body and falls monotonically to about zero at the band's edge;
- **hairline**: the dock plate's top edge differs from the plate two rows in and from the wallpaper;
- **black**: no run of pure `#000000` longer than 2 px anywhere outside the title glyph bands.

Wire:

```
:: DESKTOPCRAFT: corners=ok shadow=ok hairline=ok black=ok windows=3 surface=2880x1800 -> PASS ::
:: DESKTOPCRAFT: cost clock=<tsc|ms> frame_us=<n> budget_us=16667 backdrop_us=<n> chrome_us=<n> bar_us=<n> dock_us=<n> wall_ms=<n> veil_ms=<n> black_runs=<n> -> UNDER|OVER ::
```

`frame_us` is one full repaint of everything above: the backdrop copy, the bar, three windows' chrome and flat content,
and the dock. That is more than a steady-state pass, which repaints only damaged boxes. `wall_ms` and `veil_ms` are paid once
per picture, off the paint path. **The frame cost:** measured on the running machine by this line. It has not been
measured on metal in this arc. The seat records the QEMU reading at the fold and the rMBP reading on the bench, here.
Under QEMU TCG the number says nothing about metal.

## The screenshots

The seat captures them with the EYES facet runner (`screenshot`, PRTSCR to `~/Desktop/SCREEN0.PNG`) on QEMU, with
Aether, Lumen and Settings open after login: before (the parent image) and after (this arc), to
`docs/dev/evidence/desktopcraft/before.png` and `docs/dev/evidence/desktopcraft/after.png`. Owed to the fold.

## Not in this arc (each needs a file or a mechanism the brief did not name)

- **Menus** (menu-bar dropdowns, the crystal menu) as rounded shadowed panels with an accent highlight row: they are
  painted in `video/winmenu.rs` and `video/crystal.rs`, which the brief does not name.
- **The toast's close × on hover**: the card is painted once when shown. A hover glyph needs a repaint-on-hover path in
  `notify::pass`, which is not card paint.
- **The login shake** on a wrong password: the screen repaints only on events, so it has no cadence to animate on.
- **Desktop icons**: the tree has no desktop-icon painter (no `desktop.rs` existed; this arc's `desktop.rs` is the
  Una look's painter module). Plates for icons wait for the icons.
- **Wi-Fi, sound and bell glyphs** (`menubar::tray_paint`, `bell_row`) are kept as they were. **Even spacing** of the
  status items is geometry (the slots are the press router's), so it is kept.
- **A shadow wider than the frame band** (above).
- **Values awaiting Peter** (DESIGN-LANGUAGE's five large decisions): the scale, the OKLCH tokens, the fonts, and the
  exact shadow and veil specs. Nothing here changes dpi, fonts or the core palette. The disc fills are still
  `palette_core`'s `CTRL_*` (DESIGN-LANGUAGE §2.3 replaces them). `CRAFT` is the one table to align.

## Legal checklist (DESIGN-LANGUAGE §7.3)

1. Pixels: everything is drawn from code: the wallpaper is generated, and the glyphs are strokes. No image or icon comes from another product.
2. Fonts: unchanged (the tree's OFL faces).
3. Names: no visible string added.
4. Values: radii and strengths are ours. The inherited `CTRL_*` disc hexes are the open instance, which waits for the palette change.
5. Whole: same idea, different hand. The VEIL is not a live blur, the dock has no magnification, and the wallpaper is ours.
6. Function first: no position, chord or order changed.
