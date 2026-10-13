# SOFTY — the desktop's second look (rmbp-ledger B629)

Peter, 2026-10-14: *"as a test of the flexibility of our interface and practice for you to make an aesthetically
pleasing interface, we need to come up with our windows clone i want to call softy"* — and on what it is for:
*"knowing most people prefer mac or win and coming up with clones skirting the line of legality not to steal but for
people's comfort. they deserve a ONE OS -- UnaOS"*.

Softy is **comfort for someone arriving from Windows**: the layout, the conventions and the places things are, on
UnaOS's one window system. The UnaOS look (menu bar on top, the dock, the UnaOS chrome) stays the default; Softy is
selected live by a preference and leaves no trace when switched off.

## What we follow, and what we do not copy

| We follow (function, layout, idiom) | We do not copy (marks, trade dress) |
|---|---|
| A taskbar flush at the BOTTOM, app tiles CENTRED, the tray and clock at the right | Microsoft's Windows logo — our Start button is our own mark, a faceted crystal |
| A Start button left of the tiles that opens a Start menu above the bar | Segoe UI or any vendor font — the glass's own faces (`video::text`) |
| Start menu: a search field, a "Pinned" grid, the user and a power button at the foot | Microsoft's icons — the registrar's own icons (`appres`), our monogram tiles where none exists |
| Window caption buttons at the top RIGHT in the order minimise, maximise, close; close turns red under the pointer | The Segoe MDL2 glyph shapes — our own strokes (a bar, a rounded square, a cross) |
| The caption at the left beside a small app icon | Wallpapers, sounds, the product's names ("Windows", "Start" stays a generic word) |
| A running-app pip under its tile, wider and accented for the focused app; clicking the focused app's tile minimises it | The Mica/Acrylic material as a whole: our surfaces are flat tones with hairlines |
| The clock opens a calendar above it | The overall trade dress: Softy's proportions, mark, glyphs and palette are ours |

The colours below are named once (`video/softy/palette.rs`); the accent is the brief's value and is the seat's to
align with `DESIGN-LANGUAGE.md` (DESIGNLANG, this wave) at the fold — Softy's finish is not modelled on any other
vendor's design language.

## The preference

`system.look = una | softy` in Principia's store — `prefs_core::appearance::LOOK_KEY` / `LOOKS`, schema row
`system.look` (Enum, default `una`, writers Settings and any session program). **The seat blesses the schema:
`docs/dev/PREFS-SCHEMA.md` is generated and must be regenerated at the fold (`prefs_core` `schema_gate`).**

`video::look` reads it at the login's load (a new session generation), re-reads it whenever a PrefChanged frame
(verb 19) has arrived (`prefs_client::CHANGED_N` moved), and applies a Settings choice (`look::choose`) — all on
the settings service pass, never in the input router. A switch is one act with no reboot:

```
[look] switch to=softy from=una ms=<n> windows_rechromed=<n> via=<login|bus|settings|tests>
```

## Dimensions and colours (logical px at 96 ppi; the glass scales them with `ui::px`)

| Token | Value | Use |
|---|---|---|
| `SURFACE` | `#F3F3F3` | taskbar, the focused window's head and frame |
| `SURFACE_IDLE` | `#FAFAFA` | an unfocused window's head and frame |
| `FLYOUT` / `FLYOUT_FOOT` | `#F9F9F9` / `#EFEFEF` | Start and calendar body / Start footer |
| `TEXT` / `TEXT_2` / `TEXT_IDLE` | `#202020` / `#5C5C5C` / `#8C8C8C` | ink / secondary / unfocused caption |
| `ACCENT` / `ON_ACCENT` | `#0067C0` / `#FFFFFF` | the one accent; ink on it |
| `STROKE` at `STROKE_A` | black at 26/255 (~10 %) | every hairline: the bar's top edge, flyout outlines, dividers |
| `EDGE_IDLE` | `#C8C8C8` | an unfocused window's 1 px border (focused: `ACCENT`) |
| `HOVER` at `HOVER_A` | white at 190/255 | a hovered tile, tray group, clock |
| `PRESS` at `PRESS_A` | black at 14/255 | pressed |
| `BTN_HOVER_A` | black at 18/255 | hovered minimise / maximise |
| `CLOSE_HOT` / `CLOSE_INK` | `#C42B1C` / `#FFFFFF` | the close button under the pointer |
| `FIELD` / `FIELD_LINE` | `#FFFFFF` / `#8A8A8A` | the search field and its editable underline |
| `PIP` | `#8C8C8C` | a running app's pip (focused: `ACCENT`) |
| `APP_HUES` | eleven calm hues | monogram tiles when the registrar has no icon |
| `BAR_H` | 48 | the taskbar's height |
| `TILE_W` x `TILE_H`, `TILE_GAP`, `TILE_R` | 44 x 40, 4, 6 | one taskbar button |
| `TILE_ICON` | 24 | the icon in it |
| `PIP_W` / `PIP_W_ON` / `PIP_H` | 6 / 16 / 3 | the running pip |
| `TRAY_ICON`, `TRAY_GAP`, `TRAY_PAD`, `EDGE_PAD` | 16, 8, 10, 12 | the tray and the band's right margin |
| `START_W` x `START_H` | 600 x 620 (clamped to the panel) | the Start menu |
| `FLY_GAP`, `FLY_R`, `START_PAD` | 12, 8, 32 | its float above the bar, radius, inner pad |
| `SEARCH_H` | 36 (pill: radius 18) | the search field |
| `GRID_COLS`, `CELL_W` x `CELL_H`, `CELL_R`, `GRID_ICON` | 6, 88 x 84, 6, 32 | the pinned grid |
| `FOOT_H`, `AVATAR`, `POWER_BTN` | 64, 32, 40 | the footer |
| `POWER_W`, `POWER_ROW_H` | 180, 36 | the power menu (Sleep, Restart, Shut down) |
| `CAL_W`, `CAL_PAD`, `CAL_CELL` | 336, 20, 40 | the calendar |
| `BTN_W` | 46 | one caption button (the head's full height) |
| `GLYPH`, `GLYPH_T` | 10, 1 | the caption glyphs' box and stroke |
| `WIN_R` | 8 | the window head's corner radius |
| `WIN_ICON`, `WIN_ICON_GAP` | 16, 10 | the head's app icon and the gap before the caption |

The window GEOMETRY is unchanged (`wm::BORDER`, `wm::TITLE_H`, `outer_box`): the tiler, the drag, the resize
zones and every router keep their numbers; Softy re-skins the frame and moves the controls.

## Glyphs (ours)

* Start: a crystal rhombus, left facet in the accent, right facet the accent lightened, a white girdle line.
* Minimise: a 10 px horizontal bar. Maximise: a 10 px square, 2 px corner radius. Close: two 10 px diagonals.
* Tray: network — four rising bars (faint with a slash when the link is down); volume — a speaker body and cone
  with one or two wave strokes by level, a cross when muted; battery — a rounded outline with a nub, the fill by
  percent (red under 15 % unless charging), a bolt when charging.
* Search: a ring with a handle. Power: a ring open at the top with its stem through the gap.

## The pieces and the hooks

New files: `video/look.rs` (the switch), `video/softy/{mod,palette,paint,taskbar,startmenu,chrome}.rs` — every one
`CHARTER: Kernel — wm`. Declared at the tail of `video/mod.rs` under the dock's gate.

Each existing draw path gets ONE line-neutral hook (code first, the comment after it):

| File:line | Hook |
|---|---|
| `video/menubar.rs:799` (`strip_rect`) | the disabled test gains `or look::is_softy()` — no top strip under Softy |
| `video/menubar.rs:1037` (`compose`) | under Softy: vacate the bar's last rect once, draw nothing |
| `video/dock.rs:699` (`strip_rect`) | under Softy the dock tenancy answers `softy::taskbar::rect` |
| `video/dock.rs:827` (`compose`) | under Softy: `vacate_off()`, then `softy::taskbar::compose()` |
| `video/dock.rs:1087` (`press_at`) | under Softy: `softy::taskbar::press_at` |
| `video/dock.rs:3788` (`shown`) | `!look::is_softy() && …` — no dock layout, so every dock router declines |
| `video/wm.rs:14852` (`outside_top_corner`) | the corner radius is Softy's 8 px under Softy |
| `video/wm.rs:15603` (`control_hit`) | the caption buttons at the RIGHT (`softy::chrome::ctrl_at`) |
| `video/wm.rs:15969` (`title_bar_hit`) | the drag handle excludes the Softy buttons |
| `video/wm.rs:20703` (`paint_window`) | after the stock chrome, before the content: `softy::chrome::paint` |
| `video/settings.rs:358` (`service`) | `look::service()` beside `appearance::service()` (the brief's one settings line) |
| `tests.rs:191` | `register("softy", …)` |

The wm hooks are `#[cfg]`-gated statements on existing lines (wm.rs is in every image); the knob-off measurement is
the seat's at the fold (R107).

The dock's pins feed the taskbar through the dock's getters (`dock::DP_PINS`, `dock::dp_is_pinned`), and every
launch goes through `dock::launch_named` — the dock's own direct pin launch, so R115 holds on the taskbar and in
Start exactly as far as it holds on the dock.

## The drawing facade

`video/softy/paint.rs` is Softy's only door to pixels: `fill`, `rrect`, `rrect_top`, `stroke_rrect`,
`cut_top_corners`, `hairline_h`, `hairline_v`, `disc`, `line`, `text`, `text_w`, `app_icon`, `blend`, over a
`Surf` (a window of rows of a scene: a whole surface, or one scanline of a strip). Today the bodies are integer
loops with a 4x4 sub-sample on edges. **What it needs from GLASSKIT (`video::draw`) to be repointed:** a
coverage-based `fill_rrect` with per-corner radii and a straight alpha, `stroke_rrect` with a 1 px hairline fast
path, `fill_circle`, `line_aa` with a width, and — the one Softy cannot draw today — `shadow_rrect`, PLUS an
under-sample from the compositor so a flyout's shadow and rounded corners can show what is beneath them (today a
corner shows the desktop colour, the compositor's own rule for window heads). The kit must accept a row window
(`y0`, `h`) or a sub-slice per row, since the taskbar is row-composed through `strip::paint`.

## Behaviour

* **Taskbar** — Start, then the pinned table apps, then each running app no pin names (one tile per owner). A
  tile press raises the app's window (`wm::wl_focus`), minimises it when it is already the focused visible one
  (`wm::minimise`), else launches it (`dock::launch_named`). The tray group opens Settings; the clock toggles the
  calendar. Hover and the lit focused tile are drawn; the band repaints only when its signature moves.
* **Start menu** — a non-modal chromeless wm window (`wm::overlay_open`; the Pi uses `splash_open` + `move_to`).
  Search opens the Launcher (x86). A grid press launches. The power button folds out Sleep (`crystal::power_fire`),
  Restart and Shut down (`dialog::power_confirm`, the SHARD menu's confirm). Closes on a press outside, Esc (the
  key door), a pick, or a look switch.
* **Calendar** — the long date, the month with today in an accent disc; it repaints on the minute.
* **Chrome** — the light head (lighter when unfocused), a 1 px accent border on the focused window, 8 px top
  corners, the app icon and caption at the left, the three buttons at the right; hover tracked once per composite.

Wire: `[softy] taskbar built tiles=<n> tray=<n> h=<px>` (tiles and tray count Start and the clock),
`[softy] start open|close by=<button|key|outside|pick|switch|search>`, `[softy] calendar open|close by=…`,
`[softy] tile press app=<name> -> launch <how>|raise win=<id> ok=<0|1>|minimise win=<id> <outcome>`,
`[softy] start launch app=<name> -> <how>`, `[softy] start power row=<label>`.

## The fixture

`tests softy` (witness images; SKIPs with no panel, a too-small panel, or before the desktop is built): switch to
Softy and composite; assert the dock tenant answers the taskbar's rect (flush bottom, full width, `BAR_H` tall),
the menu bar is absent and the band painted; press the Start button (open, Start above the bar) and again (closed);
press the first pinned tile (one press counted — a launch or raise row on the wire); hit-test the focused window's
caption buttons at its right edge (minimise, maximise, close left to right, nothing at the left); switch back and
assert the taskbar is gone, the dock tenant is the dock's again and the menu bar's presence is what it was.

```
:: SOFTY: switch=ok bar=0,<y>,<pw>x<h> tiles=<n> start=open,close tile=<app> buttons=right(min,max,close) back=menubar,dock -> PASS ::
```

`buttons=nowindow` is a pass with no focused window to ask.

## Not yet Windows-like (owed)

* **The Win key.** A bare Command (Win) press is not an event today: it needs a `keymap::Action` row and a
  modifier-tap in the HID decoders, then ONE line in x86's `wc_route_event` door chain
  (`arch/x86_64/syscall.rs:7319`): `|| crate::video::softy::startmenu::key_door(raw)` (Esc closes a flyout; a
  new `Action::Start` arm calls `startmenu::toggle_by_key()`). Not in this arc's files.
* **The Settings control.** Settings > Appearance needs two lines (`settings.rs` is SETTINGSTABS'/SETTINGSCRAFT's):
  in `paint_appearance` a row `Look: UnaOS | Softy` (the Light/Dark segment's shape, `look::current()`), and in
  `press_appearance` an arm calling `super::look::choose(i)`. `look::choose` stores the key and switches.
* **Soft shadows** under the Start menu, the calendar and windows: the compositor has no under-sample (above).
* **Pressed states** on the caption buttons (hover only); the taskbar's pressed state is drawn but not latched.
* **Quick settings** (the tray opens Settings), **task view**, **snap layouts**, **jump lists**, **"All apps"**
  and a search field that types in place (it opens the Launcher), **desktop icons** (no desktop icon layer exists
  in the tree, so no hook was made), **rounded bottom window corners**.
* **Dark Softy**: the palette is light only; `appearance.mode` is not read by Softy yet.
