# QUARRYLIVE (rmbp-ledger B494) — design

**Finding.** Flight 26 boot 1, Peter: "quarry should be live updated and double click item in tree should expand".
Quarry already had the change seam and did not listen to it: `fs::NS_GEN` (NSGEN/SR3) moves once per successful
create/write/truncate/unlink/rename/rmdir in `MountTable`, and `drivers::block::usb_publish_gen` once per volume
publish/retraction; Quarry's `volume_gen()` sums them but only `collect_cached` read it, i.e. only on the NEXT
navigation. Nothing re-listed an open window. The list view had no disclosure triangle (the tree pane did).

**Seam.** No new store and no new fs mechanism: the existing namespace generation (`fs::ns_gen`) is the change
notification; Quarry's service pass compares one atomic against the generation it last listed at (no polling
of the listing). Kernel window module, child of `video::quarry::live` (CHARTER: Matrix — kernel-by-ruling R50, as every Quarry child module).

**Milestones.**
- M1 LIVE — `video/quarry/livedir.rs::service` (rides `live::service`): `volume_gen() != seen` → invalidate the cache,
  rebuild the tree keeping every expanded path and the selected path, re-`show` the cwd keeping the selected row
  by name and the scroll; a vanished cwd falls back to its parent. Bursts (a copy's writes) coalesce: at most one
  relist per 250 ms; a pending change is not lost (seen is only advanced when the relist runs). A search in
  progress defers the relist. Witness `[quarry] live dir=<d> gen=<n> relisted=<n>`.
- M2 DISCLOSE — the List view's name column carries a triangle on every folder row (indent per level, the Mac's
  shape); a press on it expands the folder IN PLACE (child rows spliced under it, sorted by the same key, nested
  expansion kept, re-spliced after every sort and every live relist); a double-click on a folder row opens it in
  the same window (unchanged), on a file its opener (unchanged — an inline row's path is `cwd/<folder>/<leaf>`).
  Icons and Columns views unchanged (the splice runs in List only). Witness `[quarry] disclose row=<r> open=<0|1> rows=<n>`.
- M3 `tests quarrylive` — through the LIVE window: open Quarry at a scratch folder under the home, create a file,
  run the live pass, see it listed; delete it, see it gone; press-equivalent toggle on a sub-folder, see its child
  inline. `:: QUARRYLIVE: changed=1 relisted=1 expand=ok -> PASS ::`.

**Metal witness (next flight).** Create/delete a file in the folder Quarry shows (shell `touch`/`rm`, or a
Quarry op): `[quarry] live dir=/home/<u> gen=<n> relisted=<n>` and the row appears/leaves with no reopen.
A triangle press: `[quarry] disclose row=<name> open=1 rows=<n>`. `tests quarrylive` prints the line above.

**Owed.** Rename typed on an INLINE row renames into the cwd (ops' rename joins the cwd); a per-directory
generation (today any namespace change re-lists the shown folder — cheap, one cached read per expanded folder);
Peter's literal words ("double click item in TREE should expand") are also taken: a double-click on a tree-pane row
toggles its expansion (the single press still navigates, the triangle still toggles).
