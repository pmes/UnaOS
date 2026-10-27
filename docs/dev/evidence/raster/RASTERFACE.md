# RASTERFACE (LEDGER SR234) — Facet's host editor face in vessels/facet-view

Cut from hw-rmbp @ 2e6832e8d on `exec-rmbp-local-rasterface`. The cloud's line: "Then the host face in stria-view's
shape: Quartzite eval_tetra, native AppKit on the Mac and GTK on Linux, no Qt anywhere. Canvas with zoom and pan,
layers panel, tool rail, one witness token per gesture." The cores (raster_core SR230, brush_core SR231,
adjust_core SR232) and their headless face (tools/raster SR233) are folded; this cut is the windowed face. The
kernel face (`unaos/crates/kernel/src/video/facet.rs`) is the NEXT cut and is not touched.

## CHARTER

- Handler: **Facet** (`docs/CODEX.md` §2, Images — "The Canvas. Raster/Vector engine").
- Seam: the host FACE of a live handler over the shared cores, in stria-view's shape (a vessel that composes the
  handler's cores behind Quartzite's windowing). First line of every new file:
  `// CHARTER: Facet (Images) — host face over the shared cores (RASTERFACE, LEDGER SR234)`.
- WHERE (rule 11, no second face): `vessels/facet-view` already owns Facet's window (`src/main.rs`: quartzite's
  AppKit image surface over `facet::serve`); `handlers/facet` owns no window (its `main.rs` is the bus server /
  CLI; `source.rs` / `meta.rs` decode and read metadata). So the editor EXTENDS facet-view: new modules under
  `src/editor/`, one `pub mod editor;` line in `src/lib.rs`, one early branch in `src/main.rs` (`--edit` /
  `--synth` hand the process to the editor; every other argument line is the viewer, unchanged).
- No preference file, no dotfile: the view state (zoom, pan, tool, brush fields, colour) lives in memory for this
  cut. When it persists it goes to Principia's store (BP-handlers-5), never a dotfile (owed RASTERFACE-PREFS).

## MEASURE (R131, before the build; read at 2e6832e8d)

- `vessels/stria-view` (Cargo.toml, `src/main.rs` 286 lines): features `gtk = ["quartzite/gtk"]`, `macos =
  ["quartzite/macos"]`, quartzite a plain path dep with `default-features = false`; ONE `editor_tree()` (a
  `TetraNode::VStack` of a `Surface` over an `HStack` of `Button`s whose action is `SMessage::Input { target:
  SURFACE, text: <verb> }`) handed to `quartzite::Backend::new_tetra_vessel`, which runs Quartzite's `eval_tetra`
  (GTK `platforms/gtk/tetra_eval.rs`, AppKit `platforms/macos/tetra_eval.rs`); the model (`stria::nle_face::Face`)
  is platform-free and served on the Synapse by `serve` (`face.on(&msg)` -> `bus.fire(face.frame())`); the
  headless witness (`--synth --do ...`) drives the SAME `Face::on` with the messages a face sends (`press:x:y` ->
  `BrowserButton{down:true}`, `move:x:y` -> `BrowserMouseMove`, `release`, `key:NAME`, `wheel:N` ->
  `BrowserScroll(0, -3N)`) and prints one JSON line; windowed only under `cfg(any(target_os = "macos",
  all(target_os = "linux", feature = "gtk")))`; GTK's surface reads BGRA (NLE2-ORDER), AppKit / headless RGBA.
- `libs/quartzite` (`src/lib.rs`, `src/platforms/mod.rs`): the AppKit embassy is chosen by `target_os` alone
  (AETHERMAC-SELECT2), GTK by `all(target_os = "linux", feature = "gtk")`, Qt by `feature = "qt"` (never enabled
  here); the headless fallback's `new_tetra_vessel` is a no-op. `TetraNode` has `VStack`, `HStack`, `Button {id,
  label, action}`, `TextField {id, placeholder, action: TextAction}` (`BrowserText` fires the entered line on
  activate), `Surface {id}`; there is NO menu, checkbox or slider node and a Surface's pointer events carry no id
  (NLE2-SURFACEID), so ONE surface carries every painted region.
- The surfaces' input (GTK `platforms/gtk/image_view.rs`): `BrowserButton {down, x, y}` (GestureDrag begin / end),
  `BrowserClick` (on release), `BrowserMouseMove`, `BrowserKey("<mods>Name")` + `BrowserText(c)` for a printable
  key; no scroll controller on GTK (NLE2-GTKWHEEL), no `BrowserResize` (NLE2-AUDIO); `BrowserButton` names no
  button (no middle-button pan) and keys send no release (no hold-space). AppKit swallows Cmd chords
  (NLE2-MACKEYS): the face carries Undo / Redo buttons beside the Ctrl / Meta chords.
- `vessels/facet-view`: `src/lib.rs` the viewer controller (`Viewer`, Facet bus client), `src/main.rs` the window
  (macOS only: `Backend::new_vessel` + `bootstrap_image_surface`) and the headless `--keys --out` witness,
  `tests/viewer.rs` (3 tests). quartzite was a macOS-only target dependency; the editor needs it on Linux (GTK)
  too, so it becomes a plain dependency (`default-features = false`) with stria-view's two features. The viewer's
  macOS-only code is unchanged (still `cfg(target_os = "macos")`).
- `tools/raster` (`src/ops.rs` 150, `src/script.rs` 330, `src/main.rs` 108): `State { doc, hist }` and `Op` are
  `mod`s of a BIN crate (no `[lib]`; `ops` and `script` are private modules of `main.rs`), so the face cannot link
  them. GRANT PROPOSED (one line, not made): `tools/raster/Cargo.toml` gains `[lib] path = "src/ops.rs"`-style
  exposure — concretely a `src/lib.rs` with `pub mod ops; pub mod script;` and `main.rs` using `raster::ops` — after
  which the face's op table becomes `raster::ops::State::run(&Op)`. Meanwhile the face calls the cores directly in
  ops.rs's exact shapes (the table below names each ops.rs arm it mirrors).
- The cores (APIs as RASTERCLI.md lists): `Document::apply`, `History::commit / undo / redo` (commit applies first
  and pushes only on success, so a refused op leaves the document and the stack unchanged, `command.rs:159`),
  `Selection::{Rect, Ellipse, Polygon, Coverage}` with `inside` (`selection.rs:35`: a rect / ellipse wholly inside,
  a polygon >= 3 points in `0..=w`), `Selection::magic_wand`, `Document::composite`, `raster_core::{encode,
  decode}`; `brush_core::{Brush::new, paint, Stroke, StrokePoint}`; the eight `adjust_core` fns;
  `pixel_core::png::encode::encode_rgba` (export, as ops.rs).

## FILES

- `vessels/facet-view/Cargo.toml` — quartzite a plain dep (`default-features = false`), features `gtk` / `macos`
  (stria-view's), path deps raster_core / brush_core / adjust_core (`default-features = false`) and pixel_core. No
  Qt, no new external crate.
- `vessels/facet-view/src/lib.rs` — one line: `pub mod editor;`.
- `vessels/facet-view/src/main.rs` — one early branch: `--edit` / `--synth` -> `facet_view::editor::main`.
- `vessels/facet-view/src/editor/mod.rs` — `Editor` (the model: one `Document`, one `History`, the active layer,
  the view, the tool, the fields), `Editor::on(&SMessage)` (the input), the gesture -> op table, `frame()`, the
  tetra tree, `serve`, the `--synth` driver and the CLI.
- `vessels/facet-view/src/editor/canvas.rs` — `View` (the view transform: zoom exponent, origin) and the canvas
  paint (checkerboard under transparency, the selection tint).
- `vessels/facet-view/src/editor/layers.rs` — the layers panel: row geometry, paint, hit test.
- `vessels/facet-view/src/editor/tools.rs` — `Tool` (8 values), the brush / adjust fields, the button verbs.
- `vessels/facet-view/src/editor/witness.rs` — the one token per gesture.
- `vessels/facet-view/tests/editor.rs` — the 8 KATs (headless, the `--synth` way).
- `docs/dev/evidence/raster/RASTERFACE.md` (this file), `docs/dev/LEDGER.md` (SR234), `docs/dev/exec-branches.txt`.

## DESIGN

### The window: one tetra tree (stria-view's shape)
```
VStack [
  HStack [ tool buttons x8 (Rect, Ellipse, Lasso, Wand, Brush, Eraser, Fill, Move), Zoom-, Zoom+, Undo, Redo ],
  Surface "facet://editor"            -- the canvas (left) + the layers panel (right strip), ONE surface
  HStack [ Layer+, Layer-, Up, Down, Blend, Eye, Levels, Curves, Hue/Sat, Invert, Threshold, Box, Gauss, Unsharp, Save, Export ],
  TextField (BrowserText)             -- the fields: `key=value`, Enter applies
]
```
Every button fires `SMessage::Input { target: "facet.editor", text: <verb> }`; the verbs are `tools::VERBS`. The
TextField fires `BrowserText("<key>=<value>")`: `color=rrggbbaa`, `diameter=<f32>`, `hardness=<u8>`, `flow=<u8>`,
`spacing=<f32>`, `tolerance=<u8>`, `opacity=<u8>` (active layer, a Command), `blend=<mode>` (active layer, a
Command), `rename=<name>` (a Command), the adjust fields `levels=a,b,g,c,d`, `curves=x,y x,y ...`, `huesat=h,s,l`,
`threshold=<u8>`, `boxblur=<u16>`, `gaussian=<u16>`, `unsharp=a,s,t`, and the files `open=<path>`,
`save=<path>`, `export=<path.png>`. A one-character `BrowserText` is a key (stria's KEYS rule: one-character keys
from `BrowserText`, named keys from `BrowserKey`), so a field line always has `=`.

### The surface: canvas + layers panel
- Surface size `(W, H)` (launch size; `BrowserResize` taken). The panel is the right `PANEL_W = 160` px; the canvas
  region is `x < W - PANEL_W`.
- The view transform (`canvas.rs`): scale `s = 2^z`, `z` in `-3..=5` (1/8 .. 32, powers of two), origin
  `(ox, oy)` in surface pixels: `screen = origin + canvas * s`, `canvas = floor((screen - origin) / s)`. The first
  view centres the document at the largest power of two that fits (at most 1).
- Zoom about the cursor (`View::zoom_at`): the canvas point under the cursor stays put:
  `p = (c - o) / s; o' = c - p * s'`. A wheel notch up (`BrowserScroll` dy < 0) is z+1, down z-1; buttons and
  `+` / `-` zoom about the canvas region's centre. At a bound the gesture is refused (`why=zoom-bound`).
- Pan: a drag with the Move tool, or any drag right after `key:space` (space ARMS a one-drag pan: the bus has no
  middle button and no key release), moves the origin by the drag delta.
- Paint: each canvas-region pixel maps through the transform; inside the document it is the composite over an
  8-px screen checkerboard (204 / 255 grey), selected cells tinted (1/4 toward blue); outside it is 64 grey. The
  panel: rows top-down from the TOP layer (so the stack reads bottom-to-top), `ROW_H = 28`: an eye box (filled =
  visible), a 20x20 thumbnail (nearest over the checkerboard), an opacity bar (length = opacity), eight blend ticks
  (the lit one = the mode, `BlendMode::ALL` order), the active row outlined; below the rows the colour swatch over
  the checkerboard. No glyphs this cut (RASTERFACE-NAMES).

### Gesture -> op (each ONE op through ONE `History`, then ONE witness token)
| gesture | op (the ops.rs arm it mirrors) | token `gesture=` |
|---|---|---|
| Rect drag a->b | `SetSelection(Some(Rect))` over the cells a..b clamped to the canvas; zero area -> `SetSelection(None)` (`Op::SelectRect` / `SelectNone`) | `select-rect` |
| Ellipse drag a->b | `SetSelection(Some(Ellipse{centre, radii}))` of the drag box clamped to the canvas (`Op::SelectEllipse`) | `select-ellipse` |
| Lasso press, moves, release | `SetSelection(Some(Polygon(points)))`, points clamped to `0..=w/h`; fewer than 3 -> `None` (`Op::SelectPoly`) | `select-lasso` |
| Wand click | `Selection::magic_wand(doc, active, x, y, tolerance)` + `SetSelection` (`Op::SelectWand`) | `wand` |
| Brush press, moves, release | `brush_core::paint` on a copy of the active layer (points in LAYER coordinates = canvas less the layer offset; `Brush::new(diameter)` + hardness / spacing / flow, mode Normal) + `PastePixels` over the whole layer (`Op::Brush`, ops.rs `paste`) | `brush` |
| Eraser (same) | the same with `mode = Erase` | `eraser` |
| Fill click | `FillSelection { layer: active, rgba: colour }` (`Op::Fill`) | `fill` |
| Move drag | the view origin += delta (no op) | `pan` |
| wheel / Zoom+- / `+` `-` | `View::zoom_at` (no op) | `zoom` |
| panel row click | active = that layer (no op) | `layer op=select` |
| panel eye click / Eye button | `SetVisible` (`Op::LayerVisible`) | `layer op=visible` |
| Layer+ | `AddLayer { index: len, Layer::new("Layer <n>", W, H) }`, active = it (`Op::LayerAdd`) | `layer op=add` |
| Layer- | `RemoveLayer { active }`, active clamped (`Op::LayerRemove`) | `layer op=remove` |
| Up / Down | `MoveLayer { active, active +- 1 }`, active follows (`Op::LayerMove`) | `layer op=up` / `op=down` |
| `opacity=` / Blend (cycles `BlendMode::ALL`) / `blend=` / `rename=` | `SetOpacity` / `SetBlend` / `Rename` (`Op::LayerOpacity` / `LayerBlend` / `LayerRename`) | `layer op=opacity` / `op=blend` / `op=rename` |
| adjust button | one adjust_core call on the active layer with the current fields + `PastePixels` (`Op::Adjust`) | `adjust kind=<k>` |
| Undo / Redo, Ctrl/Meta+z, Ctrl/Meta+Shift+Z, `Z` | `History::undo` / `redo` (`Op::Undo` / `Redo`) | `undo` / `redo` |
| `open=` | `raster_core::decode`; history cleared (`Op::Open`) | `open` |
| `save=` / Save (to the last path) | `raster_core::encode` (`Op::Save`) | `save` |
| `export=` / Export (`<path>.png`) | `pixel_core::png::encode::encode_rgba(composite)` (`Op::Export`) | `export` |
| a field (`color=`, `diameter=`, ...) | the field changes (no op) | `field key=<k>` |
| launch | `Document::new` + `AddLayer Background` (`Op::New`) or `decode` (`Op::Open`) | `new` / `open` |

A drag that starts in the panel is a panel click on release; a canvas drag outside the document still runs its
op (selections clamp; the brush clips in brush_core). The brush previews its stroke while the button is down
(painted on a copy for the frame only); the ONE op lands on release.

### The witness tokens (`witness.rs`, stdout)
`:: FACET: gesture=<g> [fields] layers=<n> active=<i> undo=<u> redo=<r> -> ok ::`, or
`:: FACET: gesture=<g> [fields] why=<word> layers=<n> active=<i> undo=<u> redo=<r> -> refused ::`.
Fields: `new`/`open` `width= height=`; `zoom` `zoom=<s>` (`2`, `1/2`) `origin=x,y`; `pan` `dx= dy= origin=x,y`;
selections `x= y= w= h=` or `points=<n>`; `brush`/`eraser` `points=<n> layer=<i>`; `fill` `layer= rgba=`;
`layer` `op=<op> index=<i>`; `adjust` `kind= layer=`; `field` `key=`; files `path=`. `why` is a core's
`Error::word()` or the face's own: `no-undo`, `no-redo`, `io`, `png-encode`, `zoom-bound`, `bad-field`, `no-path` (Save / Export before any path). In the
window the serve loop prints each token as it is made; `--synth` prints them in order and a final
`gesture=stat ... zoom= origin= fnv=<016x>` token (FNV-1a 64 of the composite, tools/raster's fnv).

### The `--synth` driver (stria-view's `--do`, whitespace-separated so the fields keep their commas)
`facet-view --synth new WxH | open <path> [--size WxH] [--do "<tokens>"]`; tokens: `press:x:y`, `move:x:y`,
`release`, `click:x:y` (press + release), `wheel:n`, `key:NAME`, a `tools::VERBS` word (a button), `set:<k>=<v>`
(the TextField's `BrowserText`), `active:<i>` (sets the active index unchecked, so the core is the one gate — KAT 8).
Every token becomes the very messages the window sends, through the same `Editor::on`.
`facet-view --edit <file.unarastr> [--size WxH]` opens the window (GTK with `--features gtk`, AppKit on macOS).

### The Mac half
The AppKit face is Quartzite's existing AppKit `eval_tetra` evaluating the same tree (no Quartzite edit). It is
READ-CHECKED only: there is no macOS on this bench, nothing here claims it compiles. `main.rs`'s macOS viewer code
is unchanged; the editor branch is target-neutral and reaches `Backend::new_tetra_vessel`.

## BLUEPRINT ROWS

Read whole before the first edit: `docs/dev/blueprint/handlers.md` (119 lines) and `docs/dev/blueprint/shape.md`;
`video.md` not read (the face touches no shared token / look rule).
- Kept: BP-handlers-1 (manifest unchanged), -2 (purpose from CODEX §2 Facet), -3 (no kernel code), -4 (the face
  links the shared cores and keeps no copy), -5 (no preference file), BP-shape-4 (no board names), -10 (no new
  external crate), -11 (licence: facet-view's GPL-3.0-or-later). The NLE2 rows' Quartzite facts are read, not
  changed (NLE2-SURFACEID, NLE2-ORDER, NLE2-GTKWHEEL, NLE2-MACKEYS bind this face too).
- `changed=none`. handlers.md is not touched (119-line cap). Proposed ONE row (report only), folding the
  executors' proposal with the face: BP-handlers-15 — "Facet's editor is three shared cores, raster_core (SR230) /
  brush_core (SR231) / adjust_core (SR232); `tools/raster` (SR233) is their headless face and `vessels/facet-view
  --edit` (SR234) their windowed face (one Quartzite tetra tree, AppKit / GTK, no Qt); every mutation is one
  `Document::apply` on one `History`, every face gesture one witness token; neither face holds editing logic."

## LEDGER ROW

`| SR234 | **RASTERFACE — ...` in `docs/dev/LEDGER.md` (status `fixed-unflown`).

## KATs (`vessels/facet-view/tests/editor.rs`, headless)

1. `--synth new 64x48` (the built binary) -> the first token `gesture=new width=64 height=48 layers=1 ... -> ok`.
2. A wheel notch up -> the view scale doubles, token `zoom=2`; a Move-tool drag -> the origin moves by the delta.
3. A brush click at the canvas centre -> the active layer's pixel there is the colour (read from the Document),
   token `gesture=brush`.
4. Layer+ -> `layers=2`; Undo -> `layers=1`.
5. A Rect drag then a Fill click -> the composite is the colour inside the rect, transparent outside.
6. Invert twice -> the composite equals the original.
7. `save=` then `open=` through a scratch file -> the composite and layer count equal.
8. `active:5` then Fill -> token `why=no-layer ... -> refused` and the Document unchanged.

## MEASURED

- Compile 1 (the brief's GTK leg), repo root:
  `CARGO_TARGET_DIR=~/unaos-bench/scratch/rmbp-0915/rasterface/target cargo test -p facet-view --features gtk` —
  rc=101, RED ON THE BENCH, NOT THE CODE: `sourceview5-sys v0.11.2` and `libspelling-sys v0.5.0` build scripts fail
  (pkg-config: `gtksourceview-5` and `libspelling-1` absent; `gtk4` present). quartzite's `gtk` feature pulls both
  unconditionally (`libs/quartzite/Cargo.toml` `gtk = [... "dep:sourceview5", "dep:libspelling" ...]`), so no
  vessel's GTK leg links on this bench until those two dev packages are installed; facet-view's code was not
  reached. Log: `~/unaos-bench/scratch/rmbp-0915/rasterface/test1.log`.
- Compile 2 (the one re-compile; the code needs no fix, so it ran the leg the bench CAN link — every line of the
  editor is target-neutral except `surface_order`'s `cfg!` and the window call, which reach quartzite's headless
  `Backend` here):
  `CARGO_TARGET_DIR=~/unaos-bench/scratch/rmbp-0915/rasterface/target cargo test -p facet-view` — rc=0, no
  facet-view warnings: `tests/editor.rs` **9 passed** (the 8 KATs + the tree / panel test), `tests/viewer.rs` 3
  passed (unchanged viewer), lib / bin unit 0. Log: `~/unaos-bench/scratch/rmbp-0915/rasterface/test2.log`.
- The GTK leg (`--features gtk`) and the AppKit half are READ-CHECKED only: GTK because of the bench libraries
  above, AppKit because there is no macOS here. Neither is claimed to compile. Both evaluate `editor::tree()`
  through Quartzite's own `eval_tetra` (no Quartzite edit); under GTK the frame is BGRA (`surface_order`).
- Anchors (`vessels/facet-view/src/editor/`): `mod.rs:155` `commit` (the one `History::commit`), `:162` `paste`
  (core call + `PastePixels`, ops.rs's `paste`), `:217` `verb` (buttons), `:288` `adjust` (the eight adjust_core
  calls), `:441` `on` (the input), `:592` `canvas_gesture` (the tool table), `:673` `frame`, `:715` `tree`,
  `:805` `drive` (`--synth`), `:840` `main` (CLI); `canvas.rs:60` `View::zoom_at`; `layers.rs:42` `hit`;
  `witness.rs:25` `token`.
- Files: `mod.rs` 906, `canvas.rs` 113, `layers.rs` 136, `tools.rs` 148, `witness.rs` 42, `tests/editor.rs` 211
  lines (`wc -l`).
- Grants needed: (1) tools/raster exposes `ops` / `script` as a lib (`src/lib.rs` with `pub mod ops; pub mod
  script;`) so the face's op table becomes `raster::ops::State::run` — not made, the face mirrors ops.rs's shapes
  meanwhile; (2) BENCH, not code: install the gtksourceview-5 and libspelling-1 dev packages (or quartzite splits
  `sourceview5` / `libspelling` off `gtk`) for any GTK leg to link here.
- Not committed: the root `Cargo.lock` gains facet-view's four new path deps when cargo runs; Cargo.lock is not in
  this arc's file list (as SR230-SR233), left to the fold.
- Owed: RASTERFACE-NAMES (layer names and field values drawn as text: text_host, not a dependency this cut),
  RASTERFACE-MENU (the blend / adjust menus are buttons and a field line: Quartzite has no menu node),
  RASTERFACE-PANKEYS (middle-button pan and hold-space: `BrowserButton` names no button and keys send no
  release; space arms one drag), RASTERFACE-PREFS (zoom / last tool to Principia's store), RASTERFACE-GTKRUN (the
  first GTK run, on a bench with the two libraries), RASTERFACE-MACRUN (the first AppKit build and run, Peter's
  Mac), RASTERFACE-SELCLIP (brush / adjust ignore the selection, RASTERCLI-SELCLIP's core gap), plus NLE2-GTKWHEEL
  (no wheel zoom on GTK) and NLE2-MACKEYS (Cmd+Z on AppKit: the Undo / Redo buttons and `Z` cover it).

## MEASURED (the bench, 2026-10-27 06:1xZ) — RASTERFACE-GTKRUN, the GTK leg
After `libgtksourceview-5-dev` + `libspelling-1-dev` (gtksourceview-5 5.18.0, libspelling-1 0.4.9) went onto the bench toolbox: `CARGO_TARGET_DIR=~/unaos-bench/scratch/rmbp-0915/rasterface/target-gtk cargo test -p facet-view --features gtk` → `Finished test profile in 1m 12s`, rc=0, `tests/editor.rs` 9 passed, `tests/viewer.rs` 3 passed, one warning (`unused variable: height`). The GTK leg LINKS and its headless tests pass; the first windowed run on a display is still owed (RASTERFACE-GTKWIN). The AppKit half stays read-checked only (no macOS on the bench; R135: the Mac builds are not the bench's).
