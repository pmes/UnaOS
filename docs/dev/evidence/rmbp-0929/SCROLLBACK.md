# SCROLLBACK (R75) — the shell window's output view scrolls

## Design
- Finding: `console.rs` header: "the panel has no scrollback"; the view's store (`Console::history`) was capped at 256 and `draw` showed only the newest page (`layout_for`). `clear` wiped it.
- Mechanism: `Console::history` (Vec<String>, cap `HISTORY_MAX` = 2000, oldest dropped) plus `view_off` (entries up from the tail), `new_lines`, `clear_abs`. `layout_span` is the one layout (`end = len - view_off`; floor = clear mark in the live view); `draw`, `cell_at` read it. Selection (`LineSel`) already records ABSOLUTE rows (`hist_base + i`), so M2 holds by construction: a click on a scrolled-back row resolves to its buffer row.
- Deviation, stated: the store keeps whole lines (a `String` per line), not a fixed-width row ring — rows re-wrap at `cols_for` on every paint (TERMWRAP) and a fixed-width ring would freeze the wrap width at print time.
- Routes: actions `ScrollPageUp/Down` (Shift+PgUp/PgDn), `ScrollTop/Bottom` (Cmd+Home/End on CRISPY, Ctrl+Home/End on both tables) as theme rows + `Console::act` -> `scroll_action`; the wheel: PLAIN wheel on the shell window (an `Event::Wheel` reaching the x86 wc pump means no ring-3 window took it; Shift is not visible at the event, so no separate Shift+wheel) -> `Console::wheel`, 3 entries/detent. `handle_key` head calls `snap_for_key` (printable/CR/BS snap to bottom).
- New output while scrolled: the view is anchored (`note_new` grows `view_off`), `[N new lines]` drawn in the freed prompt row; 4 px bar at the right edge from `draw_scroll_furniture`.
- `clear` / Ctrl-L: `Console::clear` moves `clear_abs` (lines kept); `clear --all`: `Console::clear_all`.
- Witness: `:: SCROLLBACK: rows=2000 cols= view_off= marker_ok= sel_ok= clear_ok= -> PASS ::`; spec pin in x86-wc.spec; TERMWRAP/TERMSEL/TERMSEL2 untouched (their fixtures never rely on `clear` emptying history).

## Written
All three milestones in one tree state. Boot 17 (`tests scrollback` from the desktop shell) should show `:: SCROLLBACK: rows=2000 cols=<n> view_off=100 marker_ok=ok sel_ok=ok clear_ok=ok -> PASS ::`.
