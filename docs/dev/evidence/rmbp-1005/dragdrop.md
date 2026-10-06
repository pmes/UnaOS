# DRAGDROP (rmbp-ledger B440) — MACPARITY row 18, the system drag session

Branch `exec-rmbp-dragdrop`, cut from 4ead840a. Knob: none new — the session rides `wc` (x86) and `quarry`.

## Finding
No drag-and-drop exists: a press on a Quarry row selects (a double press opens) and nothing follows the pointer.
The seam it needs already exists: PREFSUI's pointer CAPTURE (`video/capture.rs`, B389) hands a holder every
motion sample and the release (x86's drain feeds it; the dock's pin drag and the sliders are holders). The ops a
drop needs exist too: `quarry::ops::{op_rename, op_copy}` (DIRNS-confined, the menu's own bodies) and
`fs::trash::trash` (DOCK2's Trash). Quarry is ONE window on this tree (`live::WIN`): "Quarry to Quarry" is a drop
on a folder row / icon of that window, or on a sidebar row; a second Quarry window is not built (QUARRY3 did not
build one) — owed with it, the session already names `to=<win>`.

## The seam
* `video/dnd.rs` — CHARTER: Kernel — wm. THE SESSION, one at a time: `arm(payload, x, y)` captures the pointer
  (capture seam); travel past the threshold STARTS it (`[dnd] start …`), opens the GHOST (a chromeless overlay
  row, the item's name on the selection token at 50 percent over the content token — rows are opaque, so true
  alpha over the glass is owed) and follows the pointer; each motion asks the participants `drop_ok(kind)` at
  the point (the dock first — it composites on top — then Quarry); a yes is the HOVER the participant
  highlights; release delivers `drop(kind, payload)` to the hovered target or cancels; Esc cancels. Anything
  else under the pointer (another window, FOLDERVIEW's frame, the desktop) is no target: ignored, ok=0 never
  printed — `[dnd] cancel why=no-target`.
* Participants (no ring-3 protocol this arc): `video/quarry/dragdrop.rs` (CHARTER: Matrix — kernel-by-ruling
  R50): the source (a pressed list row / icon), folder targets (list row or icon of a directory: move within a
  volume, copy across volumes, Option forces copy), the sidebar (Favorites section accepts a FOLDER = adds a
  favorite; a Locations row = copy onto that volume). `video/dock.rs` tail: the Trash tile accepts files
  (`fs::trash::trash`, DOCK2's path) and lights while hovered.
* Favorites a user adds are Principia's (R79): `system.quarry.favorites`, a schema row (comma-joined paths),
  read by the sidebar's refresh, written through the PrefSet bus.

## Milestones
* M1 — `video/dnd.rs`: the session model (pure: threshold, hover, action decision, cancel) + capture wiring + ghost.
* M2 — Quarry participant: source arm on a row press, folder/sidebar targets, the drops (move/copy/favorite),
  the hover highlight; the schema row for favorites.
* M3 — the dock's Trash tile as a target (hit, highlight, drop through `fs::trash::trash`).
* M4 — `tests dragdrop` (headless over the model: the session state machine driven with explicit points and a
  fixture resolver; the real ops on a fixture folder under the user's home) + MACPARITY row 18 folded.

## Witness (the wire a metal boot prints)
* `tests dragdrop` → `:: DRAGDROP: session=ok move=ok copy=ok trash=ok cancel=ok -> PASS ::`
* by hand: `[dnd] start kind=file n=1 from=<win>` then `[dnd] drop to=<win|trash|sidebar> action=<move|copy|favorite|trash> ok=<0|1>`
  (or `[dnd] cancel why=<esc|no-target|refused>`).

## Owed
* The ring-3 bus protocol for drops into apps (a `DragEnter/DragOver/Drop` bus verb set, payload by type —
  `text/uri-list`-shaped paths, text) — named here, not built; text selection into a field rides it.
* A second Quarry window; spring-loaded folders; multi-selection payloads (n is always 1 today); drag onto a
  dock APP tile to open; the desktop as a drop target (FOLDERVIEW); true translucency of the ghost.
* aarch64: nothing feeds the capture there (PREFSUI's owed line), so a Pi press never starts a drag.
