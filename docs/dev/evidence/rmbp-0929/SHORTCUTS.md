# SHORTCUTS (R75) — one table of every chord, and a help overlay

## Design
**Finding.** The desktop answers ~20 chords (WINCYCLE, SCREENLOCK, BRIGHTKEYS, volume, PRTSCR/SHOTREGION, title
double-click, DOCKRUN, QUARRYOPS, SHELLUX Ctrl keys, editor Ctrl-S, Activity q/k) and nothing lists them.
**Mechanism.**
- M1 `video/shortcuts.rs`: `pub static SHORTCUTS: &[Shortcut { chord, scope, action, arc }]` plus `C_*` chord-token
  constants. `video/theme.rs` CRISPY/PC rows read them (`token: super::shortcuts::C_CMD_L` ...), so the keymap token and
  the table chord are one string.
- M2 overlay: `Action::ShowShortcuts` (Cmd+/, usage 0x38, `theme.rs` CRISPY row 28) -> `wc_focus_key` arm
  (`arch/x86_64/syscall.rs`) -> `shortcuts::open` -> `wm::overlay_open` (= `splash_open` at an offset; chromeless compat
  row, `set_modal_top`). Close: `shortcuts::overlay_key` is the FIRST door in `wc_route_event` (any Key/Action closes and
  is eaten, KeyUp swallowed); `overlay_dismiss` is the first arm of `wc_click_route_at`'s DOCKRUN block (any press).
  Verb: `shortcuts` prints the table and opens the overlay.
- M3 `winmenu.rs`: default app menu gains a separator and `Help > Keyboard Shortcuts` (`APP_ITEM_SHORTCUTS` 0xA2,
  appended AFTER Quit so index 2 stays Quit); `item_chord` asks `shortcuts::chord_for(label)` and the row paints the
  chord right-aligned (width counted in `item_glyphs`).
**Witness.** `:: SHORTCUTS: entries= scopes= shown= -> PASS ::` from `shortcuts::selftest`, registered as `tests shortcuts`
(with `wc`: opens, eats a key, checks closed; `[shortcuts] fixture opened= up= key_eaten= closed=`).
**Pin.** `x86-wc.spec` REQUIRE `:: SHORTCUTS:`.

## Duplication (literals left in place)
F1/F2 (`f1-brightness-down` tokens), `prtsc` (PC table), the HID usages themselves, shellux's byte matches
(`0x03/0x0C/0x15/0x17`), editor Ctrl-S, Activity `q`/`k`, title double-click (`DBL_MS`), dock/quarry right-click masks,
F10-F12 volume. The table lists them as display text; they are not read from it.

## Written
M1+M2+M3 written, not compiled (R76). Boot 17 should show `:: SHORTCUTS: entries=22 scopes=8 shown=22 -> PASS ::` after
`tests shortcuts` (or `tests`), and `[shortcuts] overlay OPEN/CLOSE` lines.
