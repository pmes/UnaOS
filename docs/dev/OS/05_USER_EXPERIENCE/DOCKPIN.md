# DOCKPIN — the dock's pin table

The dock's pinned apps are one table, `DP_PINS` in `unaos/crates/kernel/src/video/dock.rs` (the DOCKPIN block,
R75; first design record `docs/dev/evidence/rmbp-0929/DOCKPIN.md`). One row per app: name, initial, kind,
launch line, sentinel tile id. The pin SET is a bitmask over the rows (`DP_MASK`), saved in Principia's store as
`system.dock.pins` (comma-joined names, in the dock's order). An unset key, or a value naming no known app, is the
default set `DP_ALL`.

| idx | name | kind | launch line | tile id | default set |
|---|---|---|---|---|---|
| 0 | console | kernel window | (Console launch) | `CONSOLE_PIN_ID` (MAX-3) | always |
| 1 | shell | kernel window | (Shell launch) | `SHELL_PIN_ID` (MAX) | always |
| 2 | quarry | kernel window | `quarry::request_open` | `QUARRY_PIN_ID` (MAX-1) | always (available only on `quarry`) |
| 3 | activity | ring-3 program | `activity` | MAX-4 | always |
| 4 | settings | ring-3 program | `settings` | MAX-5 | always |
| 5 | editor | kernel window | `edit` (an untitled buffer) | MAX-6 | always |
| 6 | lumen | ring-3 program | `/apps/LUMEN.ELF` | MAX-7 | `lumen` builds |
| 7 | aether | ring-3 program | `/apps/AETHER.ELF` | MAX-9 | `aether` builds |
| 8 | midden | ring-3 program | `/apps/MIDDEN.ELF` | MAX-10 | `midden` builds |
| 9 | una | ring-3 program | `/apps/UNA.ELF` | MAX-11 | `una` builds |

MAX is `wm::WinId::MAX`; MAX-2 is the pulse pin and MAX-8 the Trash tile (`TRASH_PIN_ID`), so the table skips
them. Every synthetic id sits above `wm::WIN_ID_SENTINEL_FLOOR` (MAX-255) and can never name a live window.

**Ring-3 rows (6..9).** The launch line is the PROGRAM PATH (EXECNAME, B322): the pin posts it and the shell-owning
render body runs it through `shell::dispatch_command`, exactly as typed at the prompt. The shell's spawn arms the
window's app name from the path's stem (`wm::program_name`), and `dp_refresh` matches a live window to its row by
`wm::app_name_of(owner)` == the row's name, case-insensitive. A pinned row keeps one tile, open or closed.

**Feature rule.** A row whose program is optional joins `DP_ALL` only on its own feature (`cfg!(feature = …)` in
`DP_ALL`); every build carries the row, so any build pins it when `system.dock.pins` names it or the operator
chooses Keep in Dock. An operator who has already saved a pin set keeps it: a new row reaches that operator only
when they pin it.

**Sized by the table.** `DP_PROG_OWNER`, `EXTRA_POS`, the parse/render of `system.dock.pins`, the reorder band and
`installed_names` (login items) all iterate `DP_PINS.len()`; `DP_MASK` is a `u32` (32 rows at most). The `initial`
column is carried for the row's record and read by no painter: a tile draws its program's APPRES icon by name,
else its caption.

**Wire.** At login, `:: DOCKPIN: left=<n> … pinned=<p> … -> held ::` counts the pins, and a press names its tile
as `[dock] press at (x,y) tile=<t>/<N> app=<name> -> launch` (N counts every tile, the Trash included). On a build
carrying `lumen aether midden una` the default strip is ten pins plus the Trash: `left=10`, `pinned=10`, `N=11`.
