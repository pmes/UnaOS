# WINDOWLIST (R75) — the menubar's Window menu, ⌘M, ⌘`, and the dock tile's window rows

## Design
Finding: APPMENU (R73) gave every window an app menu (`video/winmenu.rs` `APP_MENU_DEFAULT`, About/Quit/Help) but no Window menu;
there was no minimise chord and ⌘Tab (WINCYCLE) cycles all windows, never one app's.

Mechanism:
- Bar: `winmenu::bar_boxes` lays a `Window` box LAST (after the app name and any tenant titles) when `s.app` or Show Desktop holds
  windows down (`winlist::desktop_hidden`). `BAR_BOXES_MAX` is now `MENU_TITLES_MAX + 2`. `menu_of` resolves it to `winlist::rows()` with
  `Sink::Win`; `OPEN_WIN` is the third open kind beside `OPEN_APP`; pseudo-owner `WIN_OWNER`.
- Rows (`video/winlist.rs`, rebuilt from the live table at every open): Minimize, Zoom, Snap Left, Snap Right (x86), Bring All to Front,
  Show Desktop (checked while active), keyline, one row per live app window (`wm::wl_rows`), focused one checked, parked ones suffixed ` (min)`.
  Chords show on the right through `item_chord` (`shortcuts::chord_for`: new table rows Minimize, Snap Left/Right, Next window of app).
- Acts: `wm::minimise`, `wm::zoom`, `winsnap::key`, `wm::raise_one`, `wm::cycle_commit`. Show Desktop minimises each live app window and keeps a
  bit mask (`SD_MASK`); the second pick raises exactly those back and re-focuses the window that had focus. `wm` tail helpers `wl_*` (WINDOWLIST block).
- Keyboard: `Action::Minimize` (code 39, ⌘M; Alt+M on PC), `Action::CycleApp` (code 40, ⌘`; Alt+` on PC) in `keymap.rs`/`clipboard::action_code`/`theme.rs`
  rows; routed from `wc_focus_key` through `winlist::key` on the WINSNAP seam.
- Dock: `dock.rs` running-tile menu gains one row per window when the owner has two or more (`menu_win_rows`/`menu_win_row`), picking raises + focuses.

Milestones: M1 menu, M2 keyboard, M3 dock rows (all committed together).
Witness: `:: WINDOWLIST: rows= live= focused= minimised= show_desktop_ok= -> PASS ::` from `tests windowlist`.
Pins: `x86-wc.spec` REQUIRE/FORBID `:: WINDOWLIST:`. Knob: none.

## Written
Boot 17 `tests windowlist` should print `:: WINDOWLIST: rows=<7+live> live=<n>=2 focused=<id of 2nd window> minimised=<live> show_desktop_ok=true -> PASS ::`,
plus `[winmenu] open title=Window ... kind=window`, `[winmenu] pick owner=window id=...`, `[winlist] pick window win=`, `[winlist] show-desktop hide|restore`.
Not witnessed: Zoom/Snap/Bring-All picks, ⌘M/⌘` chords, dock rows (written, not fixtured).
