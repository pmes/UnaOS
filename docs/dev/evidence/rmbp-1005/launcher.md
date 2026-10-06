# LAUNCHER (rmbp-ledger B417, MACPARITY row 36) — Cmd-Space: one field, programs, files, settings, math

Cut from d27ec897 (exec-rmbp-merge17). No knob: rides `wc` (x86 compositor); file opens ride `quarry` (the opener dispatch).

## Finding (read from the code)
- Cmd-Space is unbound: no `CRISPY_ROWS` row on usage 0x2C; no launcher module; `shortcuts.rs` has the chromeless
  overlay pattern (`wm::overlay_open` + `set_modal_top`), the key door is `syscall.rs` `wc_route_event`'s door chain,
  the click door is `wc_click_route_at`'s first arm.
- Programs: `dock::installed_names()` (console, shell, quarry, activity, settings, editor, lumen) launch through
  `dock::launch_named` (the pin's own post); every `/apps/*.ELF` launches through `dock::post_line_launch(path)` (the
  shell line EXECNAME resolves; `take_verb_launch` marks it a glass launch, `toast::note_glass`). Names/icons: APPRES.
- Files: UnaFS's NAME INDEX is the per-directory name B+tree (`ls`); there is no volume-wide name-attribute index
  (the catalog trees index attributes, and no `name` attribute is kept). QUARRY3 (B413) has no commits on its branch.
- Settings: `prefs_core::schema::SCHEMA` carries every key + doc; `settings::open` picks the tab from `CUR.tab` and the
  first control of that tab — there is no "open on this row".
- The kernel text path draws bytes, not UTF-8: the label is `Display > Brightness` (glass and wire), not `›`.

## The seam (R79)
- `fs/search.rs` (CHARTER Kernel — fs-core): `by_name(prefix, limit)` and `snapshot(budget)` + `filter(..)` — a
  bounded breadth-first walk of the name trees through the VFS (`read_dir`), home first, `/volumes` and the type
  database skipped. QUARRY3 joins it at the fold (one function, two callers).
- `video/launcher.rs` (CHARTER Kernel — wm): the overlay row, the key/click doors, the ranking, the math evaluator,
  the recency LRU in `<home>/settings/launcher` (SETTINGSFILES' shape: one TOML file, namespace `launcher`).
- Settings: `settings::request_open_at(key)` (tail) — the owner opens itself on the row the key names.
- Keymap: `Action::Launcher` (`cmd-space`, CRISPY tail row), consumed by the launcher's door.

## Milestones
- M1 `fs/search.rs` + the pure core in `launcher.rs` (match quality, math, settings labels, programs list).
- M2 the overlay + Cmd-Space + the doors (type, Backspace, Up/Down, Return, Esc, click) + the service pass.
- M3 picks (program / file / setting / math to the clipboard), the LRU file, recency ranking, `settings::request_open_at`.
- M4 `tests launcher` (R80: typed) and the witness.

## Witness (the next flight)
Typed: `tests launcher` -> `:: LAUNCHER: programs=<n> files=<n> settings=<n> math=ok open=ok ms=<n> -> PASS ::`.
On the glass: Cmd-Space -> `[launcher] open win=<id>`; each keystroke -> `[launcher] query=<q> hits=p<n>/f<n>/s<n>[/m1] ms=<n>`;
Return -> `[launcher] pick kind=<program|file|setting|math> name=<n> -> <how>`; Esc -> `[launcher] close`; the LRU save
-> `[launcher] saved <home>/settings/launcher recent=<n>`.

## Owed
- A volume-wide name index (a `una:name` ordered-tree entry per inode would make `by_name` one range scan); the walk is
  bounded (`SNAP_BUDGET`) and taken once per open, so a large home is cut, and the wire says `truncated=1`.
- Programs' ring-3 launches carry `origin=glass` through the shell line seam (`note_glass`); table apps through the pin.
- Settings keys outside `system.*` are listed but open the General tab.
