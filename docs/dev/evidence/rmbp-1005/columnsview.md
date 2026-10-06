# COLUMNSVIEW (rmbp-ledger B436) — Quarry's third view: Miller columns

**Finding.** QUARRY3 (B413) built List and Icons; the Mac's Columns view (each pane a directory, the selection
opens the next pane to the right, the last pane a preview) was unbuilt — MACPARITY row 27's "column view with a
preview column". `columns.rs` is the LIST view's attribute columns (ATTRCOLUMNS B402); this arc's module is
`video/quarry/millercols.rs`, a child of quarry like `iconview.rs`, attached at live.rs's tail.

**The seam (R79: no second store).** The columns are a VIEW of Quarry's one model: the focused pane IS the list
(`Model::cwd`, `list`, `list_sel`), so Quick Look (Space), the path bar, Back/Forward, Enter/double-click
(`activate_row`) and FOLDERVIEW's per-folder mode keep working unchanged. The panes to its left are its ancestors
(each selecting the child on the path, read through `collect`, the shell's `ls` seam); the pane to its right is
the selection: a folder's listing, or for a file Quick Look's card (`quicklook::render_into` — the viewer the
type opens with, no second renderer), rendered on Quarry's service pass (the decode is I/O, never in the router).
The view is window state in `toolbar::View`; the folder's remembered mode is FOLDERVIEW's `una:view.mode`, which
gains `columns` through `toolbar::mode_name` / `toolbar::view_of`.

**Milestones.** M1 the module: panes from the root of the path, narrow lists, horizontal scroll by whole panes
when they overflow (the newest pane stays on the glass, a scroll strip shows the offset), Left/Right between
panes, Up/Down within, a press selects/opens; the third switcher button `Columns` (`v` cycles). M2 the preview
pane (a folder's listing; a file's Quick Look card, latched, rendered on the service pass). M3 `tests
columnsview`, MACPARITY row 27 folded.

**Witness.** glass: `[quarry] view=columns panes=<n> path=<p>` (on each change of the panes); switcher:
`[quarry3] view=columns`; fixture: `:: COLUMNSVIEW: panes=<n> depth=<n> preview=<ok> keys=<ok> -> PASS :: …`.

**Owed.** FOLDERVIEW's `valid_mode` accepts `columns` and its `set_mode`/`apply_mode` lines use
`toolbar::mode_name`/`view_of` (a fold edit: FOLDERVIEW is on its own branch); the context menu (QUARRYOPS)
hit-tests list-view rows, so a right-press in Columns targets the list geometry; the wheel scrolls the list's
offset, not a pane; Gallery view (MACPARITY row 27's fourth) is not this arc.
