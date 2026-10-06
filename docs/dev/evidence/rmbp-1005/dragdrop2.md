# DRAGDROP2 (rmbp-ledger B470) — MACPARITY row 18, the drag session's second half

Branch `exec-rmbp-dragdrop2`, cut from 575194d1. Knob: none new — rides `wc` + `quarry` (as DRAGDROP did).

## Finding
DRAGDROP (B440) built `video/dnd.rs` (the session on PREFSUI's capture) with Quarry and the dock's Trash as its
only participants: `n` is always 1 (Quarry has no multi-selection at all: `list_sel` is one index), a dock APP
tile, the bare desktop and any ring-3 window are "no target", nothing springs, and no event code or bus verb
carries a drop to a program. The flights 24/25 wires carry no `[dnd]` line (the arc was unflown), so this is
built from the B440 seam, not from a fault. SMALLFIX4 (B466, `exec-rmbp-smallfix4`) mints no event code or bus
verb: INPUT_EV_DROP takes 12 and BUS_VERB_DROP_GET takes 24 (the next free in SMALLFIX3's lists).

## The seam
* Multi-selection is Quarry's (the Finder by R50): `quarry/dragdrop.rs` keeps the EXTRA marks of the shown
  folder beside `list_sel` — Cmd-press toggles a row, Shift-press extends from the anchor, a plain press on an
  unmarked row clears them. A drag from a marked row carries the whole selection; the ghost reads `<n> items`.
* The dock is a participant through `dock::dnd_app_at`: a tile's app key (`appres::key_of_title`), its
  REGISTRANT facts (`appres::app(key).doctypes` — FILETYPES' declaration, `type/*` and `*/*` honoured) against
  each payload path's type (`filetype::type_of_in`); a yes lights the tile, the drop opens each file there
  through THE one dispatch (`quarry::openers::open`).
* The desktop: a release where no window, strip (bar/dock) or other participant is under the pointer is a drop
  on `<home>/Desktop` (created on first use) — Quarry's own move/copy bodies (DIRNS-confined), Option copies.
* Spring-loading: the session stamps when the hover lands on a Quarry folder (row, tree row, Locations row); the
  device-service pass (`desktop_app_service`, ~1 kHz) calls `dnd::service`, which after 800 ms shows that folder
  in place (`[dnd] spring dir=<d>`); Esc backs out to the folder the drag began in and cancels.
* The ring-3 protocol (CHARTER Kernel — fulfiller): a drop on a ring-3 program's window stores the paths for
  that owner under a token and pushes `INPUT_EV_DROP` (payload `[15:8]` token, `[7:0]` count) to its input
  ring; the program asks `BUS_VERB_DROP_GET` (body `[token]`) and the reply body is the paths, `\n`-joined
  (`una_abi::drop_paths` is the one parse both rings link). Lumen is the first participant: a dropped file
  becomes `open <path>` in its input line (`:: LUMEN: drop n=<n> first=<p> ::`).

## Milestones
* M1 — una-abi: INPUT_EV_DROP (12), BUS_VERB_DROP_GET (24), the body codec, both lists; bus verb_valid; the
  x86 dispatch arm; dnd's pending-drop store + `bus_drop_get` + the ring-3 window target.
* M2 — multi-selection (marks, paint, the n-item payload and ghost count), the dock APP tile target, the
  desktop target, spring-loading (+ the service hook).
* M3 — Lumen's drop handler; `tests dragdrop2`; MACPARITY row 18 folded.

## Witness (the wire a metal boot prints)
* `tests dragdrop2` → `:: DRAGDROP2: multi=ok dock=ok desktop=ok spring=ok ring3=ok -> PASS ::`
* by hand: `[dnd] start kind=file n=<n> …`, `[dnd] spring dir=<d> after_ms=<n>`, `[dnd] spring back dir=<d>`,
  `[dnd] drop to=<win|trash|sidebar|dock:<app>|desktop|app:<owner>> action=<move|copy|open|deliver|…> ok=<0|1>`,
  `[dnd] ring3 owner=<o> token=<t> n=<n> pushed=<0|1>`, `[dnd] ring3 get owner=<o> token=<t> n=<n>`,
  and from Lumen `:: LUMEN: drop n=<n> first=<path> ::`.

## Owed
* A per-app declaration of the drop types a ring-3 window takes (a resources key); today any live ring-3 window
  is a target and the program may ignore the event. aarch64's DROP_GET arm and capture feed (PREFSUI's line).
* A second Quarry window; desktop ICONS (no icon surface exists on this tree: the desktop target is the folder);
  a translucent ghost; spring-loaded dock/sidebar Favorites; text drags into fields (rides INPUT_EV_DROP).
