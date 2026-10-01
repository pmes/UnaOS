# DOCKPIN (R75) — the dock's pinned apps are a table, and the table is a file

## Design
**Finding.** `video/dock.rs` pinned exactly Console and Shell (`PinnedApp`), plus Quarry and pulse as hand-written pins;
DOCKRUN's "Keep in Dock" (`KEPT`, dock.rs DOCKRUN block) was a boot-scoped flag that drove no tile. The editor
(`textedit::OWNER`), Activity and Settings had no tile.
**Mechanism.** `DP_PINS` (dock.rs tail) = 6 rows {name, initial, kind KernelWindow|Ring3Program, verb, sentinel id}:
console, shell, quarry, activity, settings, editor. Pin set = bitmask `DP_MASK`, default all six.
- Pins: console/shell/quarry `pin_*_wanted` gain `&& dp_is_pinned(i)`; new `pin_extras` (called by a new `pin_pulse` wrapper
  over `pin_pulse_only`, so every chain caller sees it) and `pins_applied` folds `dp_extra_wanted` (PINCOUNT rule kept).
- Order: `fixed_rank` gives a PINNED extra `RANK_EXTRA + i` (band above `RANK_UNSEEN`, below the console/shell tail) open or
  closed. DOCKID's allocation-order stamp/`order_key` arrival path is untouched; an unpinned app's window ranks by arrival.
- Running/raise: a live window is an ordinary row (existing pip + raise arm). Kernel apps matched by owner; ring-3 apps by
  `wm::app_name_of(owner)` == name, cached in `DP_PROG_OWNER` by `dp_refresh` (called after each `dock_scan` in compose,
  press_at, router_model) so `dock_tiles` (under the table lock) takes no lock.
- Launch (`dp_launch`, from a new `press_at` arm; posts, never performs): editor -> `textedit::request_open(<home>/untitled.txt)`
  (missing path = new buffer); quarry -> existing arm; activity/settings -> verb latched, `take_verb_launch()` drained in
  `x86_render_service` (main.rs, one line-neutral fold) into `shell::dispatch_command(verb, shell_console, shell_pal)` — the
  same line an operator types; the press also posts the shell launch so the verb has a window.
- Menu: right-click now also opens on a closed PIN tile; `toggle_keep`/`is_kept` map owner -> table app and toggle the bit
  (non-table owners keep the old `KEPT` behaviour, DOCKRUN fixture unchanged).
- M2: toggle latches a save; `dockpin_service()` (from `desktop_app_service`, x86 wc) writes `<home>/.dock` (unlink+create+write
  through `shell::vfs_mount_table`, KERNEL_PRINCIPAL, same pattern as textedit save / `.settings`). `relaunch_furniture`
  (login) latches a load; missing file/no home = default; unknown names ignored; a non-empty file with no known name = default.
- M3 (drag reorder): SKIPPED — no drag seam on the dock (the `[wm-act]` drag path moves windows; `press_at` consumes presses only)
  and tile order is rank-derived by DOCKID, so reorder needs a persisted rank column, not a cheap hook.
**Witness.** `:: DOCKPIN: tiles=N pinned=P running=R loaded=L saved=B -> PASS ::` at login and on each change
(`saved=-1` = failed write = FAIL; PASS needs every pinned+available app to have a tile or the strip full). `tests dockpin`
fixture line adds `fixture parse= toggle= file=` (parse round trip, unpin editor drops its tile, file write/re-read/unpin).
**Spec.** x86-wc.spec: REQUIRE `:: DOCKPIN: tiles=.. loaded=\d+ saved=-?\d+ .*-> PASS ::`, FORBID `-> FAIL ::`.
DOCKRUN/DOCKID rows unchanged.

## Written
M1+M2 written (not compiled/run, R76). Boot 17 should show after login:
`:: DOCKPIN: tiles=<n> pinned=6 running=<r> loaded=0 saved=0 -> PASS ::` (default set), then after a Remove from Dock:
`:: DOCKPIN: ... pinned=5 ... saved=<bytes> -> PASS ::`, and after relogin `loaded=5`.
Known limits: editor tile launches only with `quarry` (its drain is `quarry::live::service`); `.dock` service/save is x86-wc only;
Activity/Settings windows are not on this branch, so they are matched as ring-3 programs by name (verb = name).
