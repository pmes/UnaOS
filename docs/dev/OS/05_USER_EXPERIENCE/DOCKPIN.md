# DOCKPIN — the dock's pin table and how a pin launches

The dock's pinned apps are one table, `DP_PINS` in `unaos/crates/kernel/src/video/dock.rs` (dock.rs:3231; the
DOCKPIN block, R75; first design record `docs/dev/evidence/rmbp-0929/DOCKPIN.md`). The strip itself — tiles, raise,
quit, the console/shell launch bodies — is [`dock.md`](dock.md). Ledger rows (`docs/dev/OS/rmbp-ledger.md`): B546
DOCKPINS (line 579), B555 DOCKSHELL (line 587). Both are fixed-unflown at their newest rung: DOCKPINS flew on image
25 (flight 33); DOCKSHELL has flown nowhere.

## 1. The table

One row per app: name, initial, kind, launch line (`verb`), sentinel tile id (`DpSpec`, dock.rs:3229). The pin SET
is a bitmask over the rows (`DP_MASK`), saved in Principia's store as `system.dock.pins` (comma-joined names, in the
dock's order). An unset key, or a value naming no known app, is the default set `DP_ALL` (dock.rs:3243).

| idx | name | kind | launch line | tile id | default set |
|---|---|---|---|---|---|
| 0 | console | kernel window | (Console launch) | `CONSOLE_PIN_ID` (MAX-3) | always |
| 1 | shell | kernel window | (Shell launch) | `SHELL_PIN_ID` (MAX) | always |
| 2 | quarry | kernel window | `quarry::request_open` | `QUARRY_PIN_ID` (MAX-1) | always (available only on `quarry`) |
| 3 | activity | ring-3 program | `activity` (word verb) | MAX-4 | always |
| 4 | settings | ring-3 program | `settings` (word verb) | MAX-5 | always |
| 5 | editor | kernel window | `edit` (an untitled buffer) | MAX-6 | always |
| 6 | lumen | ring-3 program | `/apps/LUMEN.ELF` | MAX-7 | `lumen` builds |
| 7 | aether | ring-3 program | `/apps/AETHER.ELF` | MAX-9 | `aether` builds |
| 8 | midden | ring-3 program | `/apps/MIDDEN.ELF` | MAX-10 | `midden` builds |
| 9 | una | ring-3 program | `/apps/UNA.ELF` | MAX-11 | `una` builds |

MAX is `wm::WinId::MAX`; MAX-2 is the pulse pin and MAX-8 the Trash tile (`TRASH_PIN_ID`), so the table skips
them. Every synthetic id sits above `wm::WIN_ID_SENTINEL_FLOOR` (MAX-255) and can never name a live window.

**DOCKPINS (B546).** Flight 32, Peter: *"aether is too hard to launch ... i got it to run frm shell"*. The table
carried seven rows (console … lumen) and none for AETHER, MIDDEN or UNA, though the image carried all three and
APPRES already drew their icons; the only way in was typing the path. Rows 7–9 were added in the lumen shape.

**Feature rule.** A row whose program is optional joins `DP_ALL` only on its own feature (`cfg!(feature = …)` in
`DP_ALL`); every build carries the row, so any build pins it when `system.dock.pins` names it or the operator
chooses Keep in Dock. An operator who has already saved a pin set keeps it: a new row reaches that operator only
when they pin it.

**Sized by the table.** `DP_PROG_OWNER`, `EXTRA_POS`, the parse/render of `system.dock.pins`, the reorder band and
`installed_names` (login items) all iterate `DP_PINS.len()`; `DP_MASK` is a `u32` (32 rows at most). The `initial`
column is read by no painter: a tile draws its program's APPRES icon by name, else its caption.

**Row join.** The shell's spawn (and, since DOCKSHELL, the direct launch) arms the window's app name from the path's
stem; `dp_refresh` matches a live window to its row by `wm::app_name_of(owner)` == the row's name,
case-insensitive. A pinned row keeps one tile, open or closed.

## 2. The verb-posted launch, and why it is wrong (DOCKSHELL, B555)

A press runs inside the click router, which may only POST (`dp_launch`, dock.rs:3331: "POST (never perform)"). Until
B555 every ring-3 row took one arm: latch the verb in `DP_VERB_OWED` and post the SHELL's launch
(`post_launch(PinnedApp::Shell)`, dock.rs:3338–3340). The verb seam's only drain is the render body that owns the
shell `Console` (`take_verb_launch`, dock.rs:3347, then `shelltask::submit_glass`). So a path pin opened a shell
window and typed its path into it. Flight 33, Peter: *"aether launches from taskbar by launching shell and then
launching aether from there. why?"* / *"that is not how all docked apps behave"*. The wire proves it twice:

```
f33-boot2.log:4170  [06:08:36Z] [dock] press at (1470,1706) tile=5/11 app=aether -> launch verb-posted
f33-boot2.log:4178  [06:08:36Z] [dock] launch app=shell by=x86_render_service win=2 gen=4 tries=1 -> LAUNCHED
f33-boot2.log:4180  [06:08:36Z] [shelltask] dockpin verb=/apps/AETHER.ELF -> task queued=1
f33-boot2.log:4196  [06:08:37Z] [dock] launch app=aether first_window_ms=835
```

(the first press, 06:02:33, is f33-boot2.log:2240–2266: the same four lines, `first_window_ms=863`; a later Una press
RAISED the live shell and queued `/apps/UNA.ELF` into it, f33-boot2.log:4628–4630). Wrong because: a shell window the
operator never asked for lands on the glass; the program's launch rides a console it has no use for; the tile's
behaviour differs from every other docked app (FLIGHT33.md:24). The image-25 strip itself was right: `:: DOCKPIN:
left=10 minimized=0 pinned=10 …` (f33-boot2.log:122) and `tile=5/11` above, as B546 predicted.

## 3. The Quarry tile as the model

Quarry's pin never had this defect. Its press posts an open request and the owning service mints the window — no
shell, no verb:

```
f33-boot2.log:2479  [06:03:28Z] [dock] press at (156,1701) tile=0/11 quarry=pin -> open requested
f33-boot2.log:2487  [06:03:28Z] [dock] launch app=quarry first_window_ms=405
```

(`crate::video::quarry::request_open()`, dock.rs:1132). And Quarry's own program launch (`quarry::live::launch`:
read, check, spawn detached, name the window) is the model for a path pin (DOCKSHELL block, dock.rs:4694–4695).

**The DOCKSHELL fix (unflown).** A ring-3 row whose verb is a program path (`dp_is_direct`, dock.rs:4725 — x86 only)
posts a bit in `DP_DIRECT_OWED` (`dp_direct_post`, :4730) and nothing else. `dockpin_service` drains it
(`dp_direct_service`, :4759, chained at :3421) on a one-shot `dock-launch` task on a worker core that is not the hid
pump's (HIDSTALL2's Trash rule; inline `on=pump` only when there is no such core). `dp_direct_launch` (:4782) runs
the shell's path-launch checks with no console, in the shell's order: stat, the image cap, `fs::apptrust::launch_gate`
(SHELLTRUST B527 — a foreign program posts the ask and nothing runs), the read, the ELF64 x86-64 pre-check, the note
through `midden_core::launch_mode` (EXECNAME B322), APPRES's sight first (LUMENFAST B507),
`spawn_user_image_bg_argv` under `origin::with(Glass, …)`, then `wm::app_name_arm_launch` so the row join is
unchanged. A CONSOLE program (no detach note) alone falls back to the verb seam (`-> foreground (verb seam)`,
:4801–4806); none of the four pins is one. The word verbs (`activity`, `settings`) and Quarry's `run <path>` line
keep the shell seam. aarch64 keeps the verb seam for every ring-3 row (no direct spawn there, :4831).

## 4. Wire

- Login: `:: DOCKPIN: left=<n> … pinned=<p> … -> held ::` counts the pins; on a build carrying `lumen aether midden
  una` the default strip is ten pins plus the Trash (`left=10`, `pinned=10`, `N=11`).
- Press: `[dock] press at (x,y) tile=<t>/<N> app=<name> -> <word>` (N counts every tile, the Trash included); the
  word for a path pin is now `direct-posted`, for a word verb still `verb-posted`.
- Direct launch (expected, B555): `[dock] direct app=aether path=/apps/AETHER.ELF bytes=<n> note=1 pid=<p> slot=<s>
  sight_ms=<n> spawn_ms=<n> on=worker -> started` (or `-> refused=<why>`), then `[dock] launch app=aether by=dock
  direct=1 first_window_ms=<n>`, and NO `[dock] launch app=shell` / `[shelltask] dockpin verb=` line.
- Fixture: `tests dockpins` presses each built path pin through `dp_launch`, waits for its window, kills it:
  `:: DOCKPINS: pins=4 direct=4 landed=4 shell_windows=<a>-><a> shell_posted=0 verb_posted=0 -> PASS ::`.

## 5. Open

- DOCKSHELL's glass: the next flight's Aether press must show `direct-posted` and no shell window.
- aarch64 path pins still open a shell (the verb seam); a direct spawn there is unbuilt.
