# APPMENU2 (B393) — the WM's default menu set, the system chords, the crystal's Lock/Force Quit, unbounded publishers

**Finding (read from the code, MACPARITY rows 2, 4, 13, 14).** `winmenu::APP_MENU_DEFAULT` is About/Quit/
Keyboard Shortcuts and the only other WM box is `Window` (`winlist`); there is no File/Edit/Help. The
CRISPY keymap has no Cmd-Q/W/H/comma/Opt-Esc row, so those chords reach ring 3 as unknown actions and the
WM honours none of them. The crystal has `Lock` (not "Lock Screen") and no Force Quit route though
Activity exists. The menu Quit on a RING-3 window ran `wm::close(win)` only — the window went, the process
stayed (QUITLEAK's kernel arms do not cover user owners). Publishers are capped at four twice:
`winmenu::WINMENU_MAX` (OWNERS/TREES arrays) and `appmenu::SLOTS` (OWN/TREES/WINS + four `pickN` fns).

**The seam.** Kernel — wm (CODEX: the window manager owns the menu bar and the system chords). One new
file `video/sysmenu.rs` (`//! CHARTER: Kernel — wm`) holds the default File/Edit/Help trees' ACTIONS,
the chord router `sysmenu::key(Action)`, and the `tests appmenu` fixture; `winmenu.rs` keeps the one
renderer/registry and gains box KINDS (app/file/edit/tenant/window/help). An app ADDS titles through the
existing declaration (`winmenu::publish` / the bus `MENU_PUBLISH`); a tenant title that names File, Edit
or Help takes that slot, the set is never replaced. Quit on a user window is the close box's own
metal-proven path (`wc_close_click`: close the owner's rows, then `bg_kill`) through a tail-appended pub
wrapper in `arch/x86_64/syscall.rs`, so a stuck app is quittable (row 13).

**Milestones.** M1 the default set: app menu (About <app>, Settings..., Hide <app>, Hide Others, Show All,
Quit <app>), File (New/Open/Save greyed, Close Window live), Edit (Undo/Redo greyed; Cut/Copy/Paste/Select
All routed as `pal` actions to the focused consumer — the terminal's `clipboard::terminal_action` or the
ring-3 app's input ring), Window (unchanged), Help (<app> Help — greyed until the app note names a doc —
and Keyboard Shortcuts). M2 the chords: keymap actions QuitApp/CloseWindow/HideApp/OpenSettings/ForceQuit,
CRISPY rows cmd-q, cmd-w, cmd-h, cmd-comma, cmd-alt-esc, ctrl-cmd-q (LockScreen; cmd-l kept), dispatched
in `wc_focus_key` beside `winlist::key`; SHORTCUTS rows so the menus draw the chords. M3 the crystal:
"Lock Screen" and "Force Quit..." (→ Activity). M4 publishers unbounded: winmenu's registry a Vec behind
the same try_lock plus a growable `SlotBits` for the lock-free `has_tree`; appmenu's slots on
`rowstore::SegVec`, the four `pickN` fns replaced by one sink that reads the picking window. M5 the
witness.

**Witness** (`tests appmenu`, R80 — never at boot):
`:: APPMENU2: default_set=5 chords=[Q,W,H,M,comma,optesc] crystal=[lock,forcequit] publishers=unbounded -> PASS ::`
— default_set counts the WM boxes laid out for a minted fixture window (app, File, Edit, Window, Help),
chords are the CRISPY resolver's answers, crystal reads the ROWS labels, publishers publishes six fixture
windows (more than the old cap of four) and reads all six back. On the glass: every window's bar shows
`<App> File Edit … Window Help`; the wire prints `[sysmenu] chord action=quit-app win=… owner=… -> …`.

**Owed.** A graceful close REQUEST to ring 3 (no `INPUT_EV_CLOSE` exists; Quit is the close box's
close-then-kill with a zero bound); the app note naming a Help doc (una-abi + midden_core note field);
Undo/Redo (no action exists); the PC keymap rows for the new chords; the aarch64 router (it has no
`winlist::key` call either); Force Quit's "stuck" filter in Activity (it opens on the full list).
