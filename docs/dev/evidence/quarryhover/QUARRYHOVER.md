# QUARRYHOVER (rmbp-ledger B647) — design and the arc's compile

**Finding.** SETTINGSTABS (B623) left owed "the hover line in quarry/live.rs and the wheel route"; SETTINGSCRAFT (B630)
built the pattern for Settings. Measured at e57d1b0e before any edit: Quarry (`video/quarry/live.rs`) had NO hover —
`hover` appeared in the file only as DRAGDROP's drop-target outline (`dragdrop.rs:19`, a drag session, not the pointer);
no row, sidebar row or toolbar button changed under the pointer. The wheel (`wheel_route` :2795 → `wheel_scroll` :2718)
reached the tree, the list and the Miller columns (`millercols::wheel`), but not the sidebar (`sidebar::paint` cut rows
past the half-height cap with a `break`, no offset existed) and moved the Icons grid by `WHEEL_ROWS` ITEMS (less than one
grid row a detent, `list_scroll` off its row boundary).

**Charter.** Matrix — kernel-by-ruling R50 (Quarry's standing charter; every file touched already carries it). Seam: wm
(a kernel-owned compositor window). No new file, no new store, no preference.

**Mechanism (file:line at the commit).**
- `Model::hover: Hover` (`live.rs` `enum Hover { None, Tree(i), List(i), Side(i), Tool(k) }`), resolved by `hover_at`
  through the SAME rects the press reads: `toolbar::layout` (0 back, 1 forward, 2 List, 3 Icons, 4 Columns, 5 search),
  `sidebar::row_at` (`sidebar::rect`, the press's row arithmetic, headers excluded), `Geom::tree_pane` rows,
  `Geom::list_pane` rows in the List view (the header and the scrollbar gutter excluded).
- `hover_route(x, y)`: `WIN` → `wm::info` → `wm::hit_test == id` (a window above Quarry, or the pointer outside it,
  clears the light) → panel→surface as `press_route` converts it → `hover_at` → repaint ONLY when `m.hover` changed;
  `HOVER_PAINTS` counts those repaints.
- `hover_poll()`: `is_open` (one load) → `panel_info_nonblocking` (LOCKFIX's one door) → `pal::cursor::pos` →
  `LAST_PTR` swap (an unmoved pointer costs one swap) → `hover_route`. Folded onto the `settings::hover_poll` line in
  `live::service` (LINE-NEUTRAL, code before the comment); `open()` resets `LAST_PTR` on the WINID fold line (same rule).
- Painted with the kit: `live::hover_wash` = GLASSKIT's `sidebar_row` hover face (`draw::fill_rrect`, `theme::hairline`,
  radius `DIMS.row_radius`) on the tree row (`repaint_locked`), the list row (`columns::paint_list`) and the sidebar row
  (`sidebar::paint`); `toolbar::button` takes a `draw::ControlState` and `face_of` applies the kit's own nudge
  (`DIMS.hover_tint` / `press_tint` toward `theme::content_text`, the private `ControlState::face` formula). No colour
  literal was added (the appearance gate's rule): every ink is a theme role, every size the kit's table.
- The wheel: `wheel_scroll` → `wheel_scroll_view(m, sx, sy, detents, view)`; the sidebar FIRST (`sidebar::wheel`: a
  `SCROLL` offset, one row a detent, clamped at every read by `scroll_of`; `paint` and `press` offset by it), then the
  tree / list / columns as before, and the Icons grid by whole grid rows (`iconview::grid`'s `(cols, vis)`,
  `list_scroll = row * cols`, the selection pulled into the viewport; `settle` is not called there because its row clamp
  is the List view's — `iconview::follow`'s own rule).
- `tests quarryhover` (`live::hover_selftest`, registered in tests.rs's settings block by a LINE-NEUTRAL append, the
  same gate as the `quarry` module): opens the live window when it is closed; puts the pointer with
  `pal::cursor::set_abs` (the absolute HID report, rounded UP so it lands on or 1 px past each target's centre) over the
  first tree row, the first sidebar row under "Favorites" (Home) and the List button, `hover_poll` after each; expects
  `Tree(0)`, `Side(1)`, `Tool(2)`, exactly one repaint per crossing and none for a second poll with the pointer still;
  then one detent through `wheel_scroll_view` over the tree, the list, the sidebar and the list pane as the Icons view
  (no folder mode is latched) and counts the panes that took it (4). Scroll/selection, the sidebar offset, the pointer
  (≤ 1 px) and the focus owner are put back; a window the fixture opened is closed.

**Wire (x86 QEMU, then the rmbp bench).**
`[quarryhover] seen tree=Tree(0) sidebar=Side(1) toolbar=Tool(2) paints=<p0>,<p1>,<p2>,<p3> idle_repaint=0 wheel=tree:<s>/<m>[=],list:…,sidebar:…,icons:…`
`:: QUARRYHOVER: rows=ok sidebar=ok toolbar=ok repaints=3 wheel_panes=4 -> PASS ::`
(`=` after a wheel leg = the detent landed on a bound, moved=false; the pane still took it.) A 640x480 panel prints the
door fixture's SKIP shape (`:: QUARRYHOVER: quarry declined to open … :: SKIP ::`).

**Knob-off.** `video/mod.rs:691` gates `pub mod quarry` on `all(x86_64, wc)` or `all(aarch64, desktop_firmware)`, so
every line in `quarry/*.rs` is absent from a knob-off image; the two folds in `live.rs` (`service`, `open`) and the
tests.rs append are LINE-NEUTRAL regardless. The measurement is the seat's at the fold (R107).

**Compile (R107: one compile, one re-compile on red).** Worktree /home/user/exec/quarryhover, branch
exec-rmbp-quarryhover, parent e57d1b0e.

```
cd unaos/crates/kernel && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave33/target cargo +nightly check -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target ../../x86_64-unknown-none.json --features "$(cat /root/unaos-bench/scratch/cloud/wave33/kfeats24.txt),stria,gen7r9,wifi9,usbnet"
```
- compile 1: rc=101 — two `E0063 missing field hover` at the selftest's two synthetic `Model { .. }` initializers
  (`live.rs:3312`, `:4343`); fixed by `hover: Hover::None` in both.
- compile 2: rc=0 (`Finished dev profile … in 2m 22s`). Warnings in this arc's files: one `private_interfaces` lint on
  `hover_wash` (`Geom` is private, the fn `pub(super)`) — the same class as the file's pre-existing `paint_overlay`
  warning; left as compiled. Logs: `~/unaos-bench/scratch/cloud/wave33/quarryhover/compile{1,2}.log`.

Not run (R107, the seat's at the fold): the aarch64 leg (`unaos/target/user_blob.bin` is absent in this worktree and
no other target directory is permitted this wave), QEMU, `tests quarryhover`, appearance-check.py, knoboff.
