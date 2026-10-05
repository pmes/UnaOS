# QUARRY2 — Quarry's owed columns and types (B336)

Branch `exec-rmbp-quarry2`, cut from 8c750d43 (the merge11 tip). Answers the FILETYPE (B307) and
TRASHTIME/TRASHATTR (B308) hand-backs, and leaves the PIXELCORE (SR25) fold to one adapter.

## Finding

* Quarry's list view is NAME · SIZE · MODIFIED. FILETYPE gave every file a type (`una:type` on UnaFS,
  the sniff and the one extension table otherwise) and TRASHTIME gave every trashed object its origin
  (`una:trash-origin` on UnaFS, `.Trash/.index` on FAT), and neither is visible in the Finder.
* The list is sorted one way (folders first, then name) and the header is dead to the pointer.
* `.md` and `.json` are `text/plain` in `fs/filetype.rs`'s table (B307 owed): a Markdown file and a
  JSON file open as raw text in the editor, and the sniff cannot tell them from prose.
* Facet decodes PNG only, one frame; `pixel_core` (a sibling arc) brings GIF with frames.

## The seam

* Columns, sort, the header gesture: **Matrix — kernel-by-ruling R50** (Quarry is the Finder),
  `video/quarry/columns.rs` (new, child of `live.rs` like `ops.rs`/`openers.rs`). The type column
  reads `fs::filetype` + `fs::assoc` (the attribute, the sniff, the table: no store of its own); the
  origin column reads `fs::trash::entries()` (attributes or the FAT index, chosen by `trash::store`).
  Column widths and the sort are a PREFERENCE: Principia's store through the kernel prefs path the
  Settings window uses today (`prefs::get/set`, namespace `quarry`, keys `col.size` `col.modified`
  `col.type` `col.origin` `sort.key` `sort.desc`). The write is latched and done on Quarry's service
  pass, never in the click router. SETTINGSBUS moves `prefs::set` onto the bus; this code does not
  care which side of the bus answers.
* Types: **Kernel — fs-core** (`fs/filetype.rs`, `fs/assoc.rs`, existing): `text/markdown`,
  `application/json` and `image/gif` join the sniff, the extension table and the builtin association
  table (seeded into `/system/types/` on UnaFS by the existing idempotent seed).
* The two text renderers: **Tabula — owed** (`video/richtext.rs`, new, child of `fileview.rs`): pure
  functions bytes → (text, spans); the viewer paints spans. They are `no_std`, alloc-only and
  free of kernel imports so the day a Tabula core crate exists they move into it unchanged.
* Animated frames: **Facet — owed** (`video/facet_anim.rs`, new, child of `facet.rs`): a local
  `FrameDecoder` trait over `Decoded { w, h, frames: Option<Vec<(u16 delay_ms, Vec<u32> rgba)>> }`.
  Today's adapter is the PNG path, which answers `frames: None` (behaviour unchanged). The PIXELCORE
  fold is ONE `impl FrameDecoder for pixel_core::...` adapter.

## Milestones

* **M1 columns.** TYPE (the type's `una:name`, e.g. `Markdown`, `PNG image`) on every listing;
  ORIGIN when the list shows the session's Trash. Listing type = attribute → extension table → sniff
  (only for a name the table does not know, at most 32 per listing, so a navigation never reads every
  file of a big directory) → `Binary data`. Widths from prefs; `[` / `]` narrow / widen the sorted
  column (TYPE when sorted by name), persisted.
* **M2 sort.** Header click on NAME / SIZE / MODIFIED / TYPE sorts by it (a second click reverses);
  a chevron (`^` ascending, `v` descending) marks the column. Folders stay first; the sort is stable
  (`sort_by`, merge sort) so equal keys keep name order. `s` cycles the key from the keyboard.
* **M3 types.** Sniff: `# ` or `---` front matter → `text/markdown`; a leading `{`/`[` with a valid
  first token → `application/json`; `GIF87a`/`GIF89a` → `image/gif`. Extensions `.md .markdown .json
  .gif`. Openers `markdown` and `json`: the text viewer with headings bold, lists indented, front
  matter and code dimmed (a small renderer, not CommonMark), and JSON pretty-printed at 2 spaces with
  keys / strings / numbers / literals tinted (an invalid document shows raw, and says so).
* **M4 frames.** `facet_anim`: the decoder trait, the frame stepper (pure: elapsed ms → frame) and
  the viewer's tick on Facet's service pass, swapping the base image when the decoded image carries
  frames.
* **M5 `tests quarry2`.** Witness:
  `:: QUARRY2: columns=type,origin sort=name|size|mtime|type types=+markdown,+json gif_frames=<n|skip> -> PASS ::`
  (`gif_frames=skip` while the only adapter is PNG; the stepper is still driven over a synthetic
  three-frame decode and its verdict printed as `stepper=`).

## Owed

* A GIF decoder (PIXELCORE): until the fold a `.gif` is typed `image/gif`, opens in Facet and is
  refused there by name (`reason=not-png`).
* Column widths change by key, not by dragging the header edge (Quarry has no drag route).
* Origin column is not sortable (the brief's sort set is name, size, modified, type).
