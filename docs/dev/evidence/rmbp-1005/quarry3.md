# QUARRY3 (rmbp-ledger B413) — Quarry's sidebar, toolbar, icon view and Quick Look

Branch `exec-rmbp-quarry3`, cut from f1eea8d9. MACPARITY row 27 (cloud review §15); LAUNCHER row 36 reuses the search.

## Finding
Quarry (`video/quarry/live.rs`) is a tree + a columned list (QUARRY2) with right-click operations (QUARRYOPS), the
Trash and OPENERS' dispatch. No sidebar, no toolbar (back/forward, view switcher, search), no path bar segments,
no icon view, no preview. The viewers exist: Facet (`facet::decode_file`), the text viewer (`fileview` +
`richtext` for Markdown/JSON), the player (`hda_play`, its codec sniff `audio_core::sniff`). APPRES draws a
path's icon (`appres::blit_path_icon`). DIALOG's sibling shape for a floating, input-less panel is the chromeless
compat row `wm::overlay_open` (toast, shortcuts). UnaFS has no NAME B+tree (its two B+trees are the attribute
catalog); the name tree is the directory entries.

## Seam (R79)
- Quarry is the Finder by ruling (R50): the four new files are children of `live` (`#[path]`, like `columns`/`ops`),
  reaching the model without widening it. No second store: the sidebar reads `/volumes` through the one listing and
  `fs::removable`'s registry; history/view/search are window state, not preferences.
- Search: a shared-core method `UnaFS::find_names` in `libs/fs/unafs` (both rings link the crate) — the kernel
  asks it through `fs::unafs::with_unafs`; a FAT root falls back to a bounded VFS walk. LAUNCHER reuses it.
- Quick Look: NO second renderer — images through `facet::decode_file`, text through `fileview::quicklook_body`
  (the viewer's own read/sanitise/render/painter over a caller-sized surface), audio through the player's sniff
  (`audio_core::sniff`, a card: format/size; Return plays), anything else an icon + facts card.
- Eject: `fs::removable::eject` drops the mount (the same path a detach takes) and parks the disk until replug.

## Milestones
- M1 the shared core: `UnaFS::find_names` + host test.
- M2 the sidebar (`quarry/sidebar.rs`): Favorites (Home, Desktop, Documents, Downloads, Applications=/apps, Trash) +
  Locations (`/volumes` entries; removables carry an eject glyph) above the tree.
- M3 the toolbar (`quarry/toolbar.rs`): Back/Forward over a history, view switcher (list/icons), the search field
  (as-you-type), the path bar under it (segments; a click goes up).
- M4 the icon view (`quarry/iconview.rs`): APPRES/type icons at 64 logical px in a grid, name below; press/keys.
- M5 Quick Look (`quarry/quicklook.rs`): Space opens the floating panel, Space/Esc closes, arrows move the
  selection and the panel follows. `[quarry] quicklook path= type= via=<viewer> ms=`.
- M6 `tests quarry3`: `:: QUARRY3: sidebar=ok toolbar=ok views=list,icons search=ok quicklook=<n>/24 -> PASS ::`.

## Witness (what the next flight reads)
- `[quarry3] sidebar favorites=<n>/6 locations=<names> removable=<names|->` at open.
- `[quarry] quicklook path=<p> type=<mime> via=<facet|fileview|markdown|json|play|card> ms=<n>` per preview.
- `tests quarry3` → the line above; PASS = every leg ok and every test-f sample whose opener this build carries previews.

## Owed
- A real NAME index (a B+tree over names) in UnaFS — today `find_names` walks the name tree, bounded.
- Columns view (the third Finder view); Quick Look playing audio/video inline (DECSTALL first; WEBM/MP4 have no player).
- aarch64: Quick Look's panel uses a titled native row (no chromeless overlay row there yet).
