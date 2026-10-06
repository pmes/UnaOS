# PREFSUI (rmbp-ledger B389) — R91 login items, R93 slider drag, R93 resolution dropdown

## Design (written before the code)

**Finding (the wire, flights 24/25).** `[backlight] slider click pos=16%` 13:12:14, `pos=35%` 13:12:16 … — every
knob move is a separate PRESS: `settings::press_route` is the window's only pointer door (chained from
`quarry::live::press_route` on the press edge); no motion sample and no release ever reaches Settings, so a drag
is whatever presses the hand happens to make. No login-items key exists in `prefs_core::schema` (31 rows) and
nothing reads one at `login ok`; the dock tile menu is Quit / Keep in Dock only. The Display pane's "UI scale" row
is read-only text; no mode list exists anywhere in the kernel (the GOP is gone after the Kepler takeover — the
display path sets exactly one mode, the panel's native one; the only live lever is UIMETRICS's scale).

**The seams (R79).**
- POINTER CAPTURE — `video/capture.rs` (`CHARTER: Kernel — wm`): a press that lands on a draggable control
  CAPTURES the pointer (owner + key + start value); the x86 drain's tail (`wc_route_tail`, beside TERMSEL2's
  `pointer_held`) feeds every motion sample to the holder, the release edge (beside `termsel::pointer_release`)
  ends it. One capture at a time; any future drag (MACPARITY row 18) is one more holder, not a new path.
- LOGIN ITEMS — the list is Principia's preference `system.login.items` (comma-joined program names like
  `system.dock.pins`, EMPTY default, R88); the parse / render / toggle / move rules are `prefs_core::login`
  (shared-core, both rings); the kernel's `video/loginitems.rs` (`CHARTER: Principia — shared-core`) only reads
  and writes the key over the bus and launches through the dock's own launch seams (`dock::launch_named`, the
  same posts a tile press makes). Edited from the Settings **Login Items** tab and the dock tile menu's
  **Open at Login** row (one key, two editors).
- DISPLAY MODE — `prefs_core::display::modes(native w, h, edid s2)`: the panel's native mode at each UI scale
  1x..2.5x whose "looks like" width is at least 1024 (rMBP 2880x1800: 2880x1800, 1920x1200, 1440x900, 1152x720);
  the dropdown shows `Looks like 1440x900` for a scaled entry (it IS a scale — the panel stays 2880x1800);
  selecting applies the scale live through `video::dpi::set_live` and persists `system.display.mode`.

**Milestones.** M1 prefs_core (login + modes, two schema rows, `settings.tab` 0..4, PREFS-SCHEMA.md
regenerated, host tests); M2 capture seam + slider drag on Brightness / Blank screen / Volume; M3 login items
(store, Login Items tab, dock menu row, the launch after the desktop); M4 the Resolution dropdown; M5 the
witness in `tests settings`.

**Witness lines (the next flight reads).**
- `[settings] slider drag key=<brightness|idle_min|volume> samples=<n> ms=<n> from=<v> to=<v>` (one per drag).
- `[login] items n=<n> launched=<list or none>` (one per login, after the desktop).
- `[settings] login_items op=<add|remove|up|down|toggle> name=<n> items=<list> via=<settings|dock>`.
- `[settings] display mode=<WxH> looks_like=<WxH> scale=<s> applied=<0|1>`.
- `:: PREFSUI: slider_drag=ok login_items=<n> modes=<n> -> PASS ::` from `tests settings`.

**Owed.** aarch64 has no motion/release door into the capture (its arch router is byte-identity bound): the Pi's
sliders stay click-to-set. A live scale change re-lays the furniture and every window opened after it; windows
already open keep their surfaces until reopened. Reorder by drag in the Login Items list (Up/Down buttons today).
