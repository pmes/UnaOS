# FACETEDIT (rmbp-ledger B900) — Facet's kernel face: the editor over the three shared cores

Cut from hw-rmbp @ 04420a873 on `exec-rmbp-local-facetedit` (seat rmbp-0915). The cloud's line: "Then the kernel face
on unaos/crates/kernel/src/video/facet.rs, only after the host face has its KATs." — the host face has them
(RASTERFACE, LEDGER SR234, 9 tests). This cut is the on-UnaOS face: the Facet window gains an EDIT mode over
raster_core (SR230) / brush_core (SR231) / adjust_core (SR232), behind a new kernel feature `facetedit`
(`UNAOS_FACETEDIT=1`).

## CHARTER

- Handler: **Facet (Images)** (`docs/CODEX.md` §2, "The Canvas. Raster/Vector engine").
- Seam: `shared-core`. The kernel links the three `no_std` cores by path with `default-features = false`, the NLE6
  shape (`unaos/crates/kernel/Cargo.toml` `timeline_core`: optional path deps pulled by one feature). The face holds
  NO editing logic: the window, the input -> op mapping and the witness only. Every edit is a core call.
- First line of every new file under `video/`: `// CHARTER: Facet — shared-core face (Images; FACETEDIT, rmbp-ledger B900)`.
  The brief's spelling `CHARTER: Facet (Images) — shared-core face …` was written first and `scripts/charter-check.sh`
  REFUSED it (its `line_ok` reads the handler as the text before the dash, `Facet (Images)` is no CODEX handler:
  "declares no owner" x3); the gate is BP-shape-8's holder, so the handler word stands alone and `Images` moves
  inside the parenthesis (measured: the gate's `✅ charter` line after the change).
  `video/facet.rs` has no `CHARTER:` line at 04420a873 (its siblings `facet_anim.rs` / `facet_raw.rs` carry
  `//! CHARTER: Facet — shared-core`); this cut adds none to it (its edits are line-neutral wiring only) and changes
  no existing line's meaning.
- Preferences: none written. The view state (zoom, pan, tool, brush fields, colour) lives in memory for this cut;
  when it persists it goes to Principia's store as `system.facet.*` through the prefs bus (BP-handlers-5), never a
  dotfile (owed FACETEDIT-PREFS).

## MEASURE (R131, read whole before the first edit, at 04420a873)

- `unaos/crates/kernel/src/video/facet.rs` (2155 lines): the viewer. One `wm` row owned by `OWNER =
  KERNEL_OWNER_BASE + 4` over a cached-RAM ARGB surface (`SURF`), minted by `mint` -> `wm::create_at`; the decoded
  BASE image (`View.px`, `0xFFRRGGBB`, at most `BASE_W x BASE_H` = 2048x1536) is what zoom / pan re-sample. Input:
  `key_route(ev)` and `press_route(x, y)` are chained from `quarry::live::key_route` / `press_route` (router depth:
  they only QUEUE `Cmd`s), `service()` is chained from `quarry::live::service` and drains the latches, the command
  queue, the drag (`drag_poll` on `pal::cursor::button_down`) and `anim::tick`. Keys: `+ = - _ 0 1 [ ] Left Right
  Delete i p space`; `e` is free. The viewer has no menu of its own. Witness lines `[facet] open|decoded|present|
  refuse|closed`, `:: IMGVIEW:`; `tests imgview` registered in `tests.rs` (DONE3 block, cfg witness + facet + the
  desktop family). The module compiles under `facet` + the desktop family (x86 `wc` | aarch64 `desktop_firmware`),
  `video/mod.rs:933-937`. `facet` is on the image line (`scripts/image-line.env`), so facet.rs is in the knob-off
  image: every edit there is a line-neutral append (a panic Location's line must not move).
- The host face (`vessels/facet-view/src/editor/`: `mod.rs` 906, `canvas.rs` 113, `layers.rs` 136, `tools.rs` 148,
  `witness.rs` 42): `Editor` = one `Document`, one `History`, `active`, `View`, `Tool`, the brush / adjust fields;
  `commit` = `History::commit` (= `Document::apply` + push on success), `paste` = a core call over a copy of a layer
  + `PastePixels` over the whole layer; the gesture -> op table (`canvas_gesture`, `verb`, `adjust`, `panel_click`);
  `View` = zoom exponent `-3..=5` and an origin, `zoom_at` keeps the canvas point under the pointer; the token
  `:: FACET: gesture=<g> [fields] [why=<w>] layers=<n> active=<i> undo=<u> redo=<r> -> ok|refused ::`.
- `docs/dev/evidence/raster/RASTERFACE.md` and `RASTERCLI.md`: the op shapes (tools/raster `ops.rs`), the owed
  items (RASTERCLI-SELCLIP: brush / adjust ignore the selection; RASTERFACE-NAMES/-MENU/-PANKEYS/-PREFS).
- The cores at 04420a873: `raster_core` (`#![no_std]`, deps pixel_core only — already a kernel dep), `brush_core`
  (deps raster_core; f32 arithmetic, its own `floor` / `round` / `isqrt`, no std float method), `adjust_core` (no
  deps); each `default = ["std"]` with `std = []` (the seam only). `History` keeps WHOLE `Document` snapshots
  (`command.rs:142-178`), unbounded.
- NLE6's kernel face (`video/vplay.rs` / `video/player.rs`): `timeline_core` an optional path dep with
  `default-features = false`, pulled by `videoplayer = [..., "dep:timeline_core"]`; its fixture registers from
  `player.rs:988` (`crate::tests::register("nle", ...)`).
- `wm`: `create_at_native` (a native, never-magnified kernel window: Settings, Activity, Quarry use it; the surface is
  `metrics::size(logical)` device px), `present`, `close`, `retitle` (desktop family), `info`, `hit_test`,
  `close_box_hit`, `focus_changed`, `focus_asid`. The glass kit `video/draw.rs`: `Surf`, `Rect`, `ControlState`,
  `push_button`, `segmented_w`, `slider`, `checkbox`, `fill` — Settings' row widgets; no new widget is written.
  Colours: `theme::*` tokens only (BP-video-1: no colour literal in `video/**`, `scripts/appearance-check.py`).
- Cmd-Z / Shift-Cmd-Z reach a kernel window as `pal::Event::Action(Action::Undo | Action::Redo)` (the keymap rows
  `theme.rs:408`, UNDOKEYS B743) through the same `key_route` chain Facet already sits on.
- THE GATE (rule 6): `./arroyo foldleg` runs `scripts/foldleg.sh`, whose outer stage CLEARS every caller `UNAOS_*`
  knob that is not a `:?` bench-local value and loads only `scripts/image-line.env`; the inner stage compiles through
  the builder's own env -> feature map (`builder/src/main.rs`, e.g. `:448` `UNAOS_FACET` -> `facet`). Neither path
  knows `UNAOS_FACETEDIT` (the builder is not in this arc's file list), so the gate compiles the KNOB-OFF kernel with
  this arc's Cargo / mod.rs / facet.rs / tests.rs edits, and compiles the facetedit modules ZERO times. Grant (one
  line, not made): `builder/src/main.rs` beside `:448`: `if std::env::var("UNAOS_FACETEDIT").is_ok() {
  feats.push("facetedit"); }`; and a knob-on fold leg needs either that knob on a `--line` file or foldleg keeping a
  named caller knob.

## FILES

- `unaos/crates/kernel/Cargo.toml` — `facetedit = ["facet", "dep:raster_core", "dep:brush_core", "dep:adjust_core"]`
  with its comment at the `[features]` tail (beside `videoplayer`); three optional path deps
  (`default-features = false`) beside `timeline_core`.
- `unaos/arroyo` — `[ -n "${UNAOS_FACETEDIT:-}" ] && _feats="${_feats}facetedit,"` with its comment, beside the
  `UNAOS_VIDEO` line (x86 `_feats`; no `K8_FEATS` arm: x86-first).
- `unaos/crates/kernel/src/video/facet.rs` — four line-neutral folds (below) and one cfg'd tail fn
  (`edit_handoff`).
- `unaos/crates/kernel/src/video/mod.rs` — three TAIL `pub mod` lines folded onto the file's last code line, before
  its first `//`.
- NEW `unaos/crates/kernel/src/video/facetedit.rs` — the window: open from the viewer, mint, the queue, the input
  routing, refresh, save / open through the VFS, close, `tests facetedit`.
- NEW `unaos/crates/kernel/src/video/facetedit_tools.rs` — the model the window drives: `View` (integer), `Tool`,
  the fields, `Editor` and its gesture -> op table, the token.
- NEW `unaos/crates/kernel/src/video/facetedit_layers.rs` — the paint (canvas through the view, the layers strip,
  the tool rail, the adjust / file row, the fields row) and the ONE hit table the paint records.
- `unaos/crates/kernel/src/tests.rs` — `tests facetedit` registered on the `imgview` line (line-neutral, cfg'd).
- `unaos/scripts/banner-cert.sh` — one row and its comment: `facetedit|[facetedit] closed win=|wc+desktop_firmware|unmeasured-here`.
  The brief named cond `-`; the literal lives in a module gated on the desktop family, which `facetedit` does not
  imply, so the cond names it (the `svg` row's `wc+desktop_firmware` shape, rule 7(a)) — a `-` would read MISSING on a
  facetedit build without `wc` instead of UNVERIFIABLE.
- `docs/dev/evidence/raster/FACETEDIT.md` (this file), `docs/dev/OS/rmbp-ledger.md` (B900),
  `docs/dev/exec-branches.txt`.

## DESIGN

### Opening (the viewer's own key)
The viewer has no menu, so the door is a key: `e` while the viewer holds the keyboard. `facet::key_route` (router
depth) posts a latch (`facetedit::request_from_viewer`) and returns; `facet::service` -> `facetedit::service` takes
it on the render pass: `facet::edit_handoff()` hands over the shown path and MOVES the viewer's base image out of
its view (`0xFFRRGGBB`, `bw x bh`, at most 2048x1536; no copy), the viewer window closes (`facet::close`), and the
editor builds ONE
raster_core `Document` of that size whose layer 0 ("Background") is the image (RGBA, alpha 255) — `Document::new` +
`AddLayer` (tools/raster's `new` shape with the pixels filled) — and mints ONE native window owned by Facet's own
`OWNER` (the viewer's row is gone, so the owner names one window at a time). Token `gesture=open path=<p>
width=<w> height=<h>`. The close box closes the editor; nothing is saved implicitly.

### The window regions (device px = `metrics::size(logical)`)
```
+---------------------------------------------------------------+----------------+
| RAIL: segmented [Rect|Ellipse|Lasso|Wand|Brush|Eraser|Fill|Move]  [-] [+] [Undo] [Redo] |
+---------------------------------------------------------------+----------------+
|                                                               | LAYERS strip   |
|   CANVAS: the composite through the view, checkerboard under  | rows top-down  |
|   transparency, the selection tinted toward the accent,       | (top layer     |
|   MEDIA_BG outside the document                               | first): eye,   |
|                                                               | name, opacity, |
|                                                               | blend; active  |
|                                                               | row raised     |
|                                                               | [+][-][Up][Dn] |
|                                                               | [Blend][Eye]   |
+---------------------------------------------------------------+----------------+
| ADJUST: [Levels][Curves][Hue/Sat][Invert][Threshold][Box][Gauss][Unsharp]  [Save] [Open] |
| FIELDS: Size [slider] Hard [slider] Flow [slider]  R [s] G [s] B [s] A [s]  [swatch]      |
+--------------------------------------------------------------------------------+
```
Logical sizes: window 960x640 (clamped to the panel's work area), rail 40, two bottom rows of 40, strip 200, row 28.
The paint records every control's `Rect` in ONE hit table that the press reads (one geometry, the Settings rule);
the canvas region is the remainder. Controls are the glass kit's (`draw::segmented_w`, `push_button`, `slider`,
`checkbox`), labels `text::draw_text` in `Face::Ui`, colours `theme::*` tokens only: checker squares
`surface_raised` / `surface_sunken`, outside the document `MEDIA_BG`, selection `accent` at 1/4, the active row
`surface_raised`. The furniture rules of video.md are kept: a native kernel row (`create_at_native`, BP-video-4's
one dpi scale), presented through `wm::present`, no scan-out write, no overlay, no z or focus outside
`wm::focus_changed` on the user's own press (BP-video-23).

### The view (integer, the host's transform)
`screen = origin + canvas * 2^zoom`, `zoom` in `-3..=5` (1/8 .. 32, powers of two), origin kept in Q8 (1/256
screen px) so the soft-float kernel does no float math for the view. `cell(s) = floor(((s*256 + 128) - o) * 2^-zoom
/ 256)`. Zoom about the pointer: `p = (c*256 - o) * 2^-zoom; zoom' ; o' = c*256 - p * 2^zoom'` — the canvas point
under the pointer stays put; at a bound the gesture is refused (`why=zoom-bound`). The first view is the largest
power of two (at most 1) that shows the document, centred. Pan = origin += drag delta. The token prints the origin
as whole screen px (`o >> 8`); the host prints its f64 — same field, integer here.

### Input -> op (each gesture ONE op through ONE `History`, then ONE token — the host face's table, mirrored)
| gesture (kernel) | op (host face's arm) | token `gesture=` |
|---|---|---|
| rail segment press | tool := it (no op) | `tool tool=<w>` |
| Rect / Ellipse drag a->b on the canvas | `SetSelection(Some(Rect / Ellipse))` over the cells a..b clamped; zero area -> `SetSelection(None)` | `select-rect` / `select-ellipse` |
| Lasso press, moves, release | `SetSelection(Some(Polygon))`, fewer than 3 points -> `None` | `select-lasso` |
| Wand click | `Selection::magic_wand(doc, active, x, y, tolerance)` + `SetSelection` | `wand` |
| Brush / Eraser press, moves, release | `brush_core::paint` on a copy of the active layer (LAYER coordinates; `Brush::new(diameter)` + hardness / flow, mode Normal / Erase) + `PastePixels` over the layer | `brush` / `eraser` |
| Fill click | `FillSelection { active, colour }` | `fill` |
| Move-tool drag, or a drag after `space` | origin += delta (no op) | `pan` |
| wheel over the canvas / `-` `+` buttons / `+ = -` keys | `View::zoom_at` about the pointer / the region centre (no op) | `zoom` |
| strip row press | active := that layer (no op) | `layer op=select` |
| strip eye press / Eye button | `SetVisible` | `layer op=visible` |
| `+` (Layer add) | `AddLayer { len, Layer::new("Layer <n>") }`, active := it; at 8 layers refused `why=layer-cap` | `layer op=add` |
| `-` (Layer remove) | `RemoveLayer { active }`, active clamped | `layer op=remove` |
| Up / Down | `MoveLayer { active, active +- 1 }`, active follows | `layer op=up` / `op=down` |
| Blend | `SetBlend` (cycles `BlendMode::ALL`) | `layer op=blend` |
| adjust button | one adjust_core call on the active layer with the fields + `PastePixels` | `adjust kind=<k>` |
| Undo / Redo buttons, Cmd-Z / Shift-Cmd-Z (`Action::Undo` / `Redo`), `Z` | `History::undo` / `redo` | `undo` / `redo` |
| Save | `raster_core::encode` -> the VFS at `<folder>/<stem>.unarastr` beside the viewer's file | `save` |
| Open | that path through the VFS -> `raster_core::decode`; history cleared, view refitted | `open` |
| a field slider press | the field changes (no op) | `field key=<k>` |

`why` is a core's `Error::word()` or the face's own: `no-undo`, `no-redo`, `zoom-bound`, `layer-cap`, `vfs-<why>`,
`too-large`. The adjust fields are the host's defaults (levels identity, curves identity, threshold 128, box 1,
gaussian 256, unsharp 100/256/0, hue/sat 0); only the brush fields and the colour have sliders this cut (owed
FACETEDIT-ADJFIELDS). A canvas press starts a drag; the drag is sampled on each service pass while
`pal::cursor::button_down()` holds, and released when it does not (the viewer's `drag_poll` shape); the ONE op lands
on release. No in-flight preview this cut (owed FACETEDIT-PREVIEW).

### The witness tokens (the host face's shape, so a flight's wire and a host run read the same)
`:: FACET: gesture=<g> [fields] layers=<n> active=<i> undo=<u> redo=<r> -> ok ::` and
`:: FACET: gesture=<g> [fields] why=<word> layers=<n> active=<i> undo=<u> redo=<r> -> refused ::`, one
`serial_println!` per gesture. Plus `[facetedit] open path= doc=<w>x<h> win=<id> surf=<w>x<h> layer_bytes=<n>` and
`[facetedit] closed win=<id> tokens=<n>` (the certifying literal).

### Memory
A layer is `w * h * 4` heap bytes. At the rMBP panel's 2880x1800 that is 20,736,000 B (19.8 MiB) per layer; 8 layers
158 MiB, against the x86 heap's 256 MiB (`allocator.rs:40`) shared with the desktop — and `History` snapshots the
WHOLE document per step. So this cut: the document is the viewer's base (at most 2048x1536 = 12,582,912 B = 12 MiB per
layer), at most 8 layers (`layer-cap`), and the history is trimmed from its oldest end to at most 16 steps and 64 MiB
of snapshot pixels (`History.past` is a public `Vec`; the trim is the face's bound, not an edit). The composite is
cached (`w*h*4`) and rebuilt only after a document change.

### Arch
The three cores are arch-neutral (`no_std` + `alloc`, no arch code). The modules compile under `facetedit` + the
desktop family (x86 `wc` | aarch64 `desktop_firmware`), the same gate as `facet`, so the face is arch-neutral by
cfg; only arroyo's x86 `_feats` arms the knob (no `K8_FEATS` arm): x86-first, nothing on aarch64 is touched.

### Knob-off byte-identity, file by file (not measured here: `./arroyo knoboff facetedit` is the seat's at the fold)
- `Cargo.toml`: a new feature and three OPTIONAL deps; with `facetedit` off cargo compiles none of them and the
  kernel crate's cfg set is unchanged. `unaos/Cargo.lock` gains three package entries when cargo next resolves (not
  in this arc's file list; the fold's cargo run regenerates them) — lock entries are not compile inputs of a crate
  that does not enable them.
- `arroyo`: one comment block and one `[ -n ... ]` line; with the knob unset `_feats` is unchanged.
- `video/mod.rs`: three `#[cfg(all(feature = "facetedit", ...))] pub mod` items appended on the LAST code line before
  its `//`: no line moves, the items are cfg-erased.
- `video/facet.rs`: four folds, each on a line whose existing code is untouched to the left of the fold and which
  holds no panic site at or after the fold point — (1) `key_route`'s fn line: `#[cfg] if facetedit::key_route(ev) {
  return true; }` after the `{`; (2) the comment line above the `p` / space check in `key_route`: `#[cfg] if
  matches!(c, b'e' | b'E') { ... return true; }` before its `//`; (3) `press_route`'s fn line: `#[cfg] if
  facetedit::press_route(x, y) { return true; }` after the `{`; (4) `service`'s `anim::tick();` line: `#[cfg]
  facetedit::service();` before its `//`. The tail fn `edit_handoff` is NEW lines after the file's last line, under
  `#[cfg(feature = "facetedit")]`. Knob-off every fold is cfg-erased and no existing line number changes, so no
  `Location` (file, line, col) baked into the image moves.
- `tests.rs`: `#[cfg(feature = "facetedit")] register("facetedit", ...);` appended inside the `imgview` line's block
  after the existing call: same line, cfg-erased.
- `banner-cert.sh`: a registry row and comment (script, not the image).

## BLUEPRINT ROWS

Read whole before the first edit: `docs/dev/blueprint/handlers.md`, `docs/dev/blueprint/video.md`,
`docs/dev/blueprint/shape.md`.
- Kept: BP-shape-2 (one capability, ONE no_std core each, linked by both rings: the kernel now links the three),
  BP-shape-4 (no board name in the new files: subsystem names), BP-shape-5 (gated on its feature knob, not on
  `target_arch`), BP-shape-6 (the desktop-family cfg), BP-shape-8 (CHARTER on every new video/ file), BP-shape-10
  (path deps only, no new external crate), BP-shape-11; BP-handlers-1/-2 (Facet from CODEX §2), -3 (owner + seam on
  the first line), -4 (host and on-UnaOS face share the cores; no copy), -5 (no preference written); BP-video-1
  (tokens only; no colour literal), -2, -3, -4 (native row at the one dpi scale), -7 (`wm` asks `wincap`), -10 (no
  panel write), -15, -16 (no Settings row), -17 (no overlay row), -23 (focus only on the user's press).
- `changed=none`. handlers.md is not touched (its cap). PROPOSED (report only), extending RASTERFACE's BP-handlers-15
  proposal: "Facet's editor is three shared cores, raster_core (SR230) / brush_core (SR231) / adjust_core (SR232);
  `tools/raster` (SR233) is their headless face, `vessels/facet-view --edit` (SR234) their host face and
  `video/facetedit.rs` (B900, `facetedit`) their kernel face (the viewer's `e`); every mutation is one
  `Document::apply` on one `History`, every face gesture one `:: FACET: gesture=` token; no face holds editing
  logic." and the manifest row's Facet cell: "... ; the editor `unaos/crates/kernel/src/video/facetedit.rs`
  (shared-core, B900)".

## LEDGER ROW

`| B900 | **FACETEDIT — ...** | rmbp | rmbp | fixed-unflown ... |` in `docs/dev/OS/rmbp-ledger.md`, the B885 shape.

## THE FIXTURE — `tests facetedit`

Opens the editor on a generated 64x48 opaque image (pixel `(x, y)` = `(4x, 5y, (x^y)*4, 255)`) through the same
`open_doc` the viewer's `e` reaches (a window when the panel is up), then drives the eight host-face KAT gestures
through the SAME face functions the queue calls (region / panel coordinates in, the hit table read, one op each):
1. `zoom` — a wheel notch at the region centre: `view.zoom` = z0 + 1 AND the canvas cell under the centre unchanged.
2. `pan` — the Move segment pressed through the hit table, a drag of (+7, +5): the origin moved by exactly that (Q8).
3. `brush` — Brush, colour (255, 0, 0, 255) through the fields, a click over canvas (32, 24): the active layer's pixel
   there reads the colour.
4. `layers` — the strip's `+` through the hit table: `layers=2`; `Action::Undo` through `key_route`: `layers=1`.
5. `fill` — a layer added, a Rect drag over cells (8, 8)..(24, 20), the Fill tool clicked: the composite reads the
   colour inside and the original image outside.
6. `invert` — Invert pressed twice through the hit table: the composite differs after one and equals the pre-invert
   composite after two.
7. `roundtrip` — Save, then Open (`<home>/FEDTEST.unarastr` via the VFS): layer count and the composite's FNV-1a 64
   equal.
8. `refused` — `active := 5` unchecked, a Fill click: the token is `why=no-layer ... -> refused` and the Document's
   composite FNV and `undo=` are unchanged.
Verdict: `:: FACETEDIT: zoom=ok pan=ok brush=ok layers=ok fill=ok invert=ok roundtrip=ok refused=ok tokens=<n> -> PASS ::`
(each `=` its leg's measured readback: `ok` or `bad`; `tokens=` the `:: FACET:` lines this editor printed, the
`open` one included; FAIL when any leg is `bad`), preceded by `[facetedit] fixture win=<id|none> doc=64x48 zoom=<s>
path=<p>`. With no panel the legs still run on the model (the window is the only part skipped: `win=none`); the
scratch file is unlinked and the editor closed. The fixture holds the editor's lock for its run and never arms the
pointer-sampled drag: a canvas drag's motion / release are the model's own `motion` / `release`, the calls the
service pass's `drag_poll` makes; every press goes through `apply` and the hit table the paint recorded. Registered
on the `imgview` line (cfg `witness` + `facet` + the desktop family, + `facetedit`).

## MEASURED

- THE ONE COMPILE (rule 6, from `unaos/`): `UNAOS_WIFI_FW_PATH=… UNAOS_KEPLER_FW_PATH=… UNAOS_AETHER=1 UNAOS_MIDDEN=1
  UNAOS_UNA=1 UNAOS_SONDE=1 UNAOS_FACETEDIT=1 ./arroyo foldleg` (the brief's `bash -c 'set -a; . ./scripts/image-line.env
  …'` wrapper was refused by this seat's sandbox; foldleg's outer stage loads the same file itself and keeps only the
  `:?` values, so the line built is the same) — exit 0, last line `FOLDLEG: runtime_s=139`, verdict line
  `FOLDLEG: x86 kernel+loader built under image-line.env @3e6d45b10 blob=08c35316 features=59 -> PASS rc=0`, build_s=137.
  Log: `~/unaos-bench/scratch/rmbp-0915/facetedit/foldleg1.log` (+ `unaos/target/foldleg-builder.log`).
- WHAT IT PROVES, AND WHAT NOT: `caller_knobs_cleared=7` and the builder's feature list carries `facet` and NO
  `facetedit` — as MEASURE predicted, the gate built the KNOB-OFF image with every edit of this arc (Cargo.toml's
  optional deps resolve, the facet.rs / mod.rs / tests.rs folds are well-formed when cfg-erased). The three facetedit
  modules and facet.rs's tail fn were compiled ZERO times: code-only, uncompiled under the knob (the B883 / B885
  shape). The knob-on compile needs the builder grant (MEASURE) and a fold leg that keeps the knob.
- Read-checks run (no build): `python3 scripts/appearance-check.py .` -> `literals_outside_theme=0 files=0`;
  `bash scripts/charter-check.sh .` -> `✅ charter (… 433 kernel files …)` (after the header fix and the `.{}` EXT
  spelling: its dotfile regex had read `{}.unarastr"` as a home dotfile); `python3 ../tools/neutral-check.py` ->
  `new=0 -> PASS`; `grep -rnF '[facetedit] closed win=' crates libs | wc -l` -> 1.
- Anchors: `video/facetedit.rs` `:93` `service`, `:156` `open_doc`, `:184` `mint`, `:230` `apply` (press -> hit
  table -> op), `:296` `drag_poll`, `:319` `key_route`, `:361` `press_route`, `:440` `save`, `:447` `open_saved`,
  `:487` `selftest`; `video/facetedit_tools.rs` `:77` `View::zoom_at`, `:288` `commit` (the one `History::commit`),
  `:297` `trim`, `:307` `paste`, `:412` `verb`, `:469` `adjust`, `:583` `canvas_gesture`; `video/facetedit_layers.rs`
  `:88` `hit_at`, `:133` `paint`, `:202` `paint_canvas`, `:236` `paint_strip`; folds `video/facet.rs:1099`, `:1698`,
  `:1719`, `:1744`, tail `:2157`; `video/mod.rs:1267`; `tests.rs:204`; `Cargo.toml:3901-3909`, `:4017-4021`;
  `arroyo:1311-1312`.
- Files: `facetedit.rs` 616, `facetedit_tools.rs` 645, `facetedit_layers.rs` 273 lines (`wc -l`; tools is ~45 over the
  brief's ~600 — its gesture table is the host's arm for arm).
- Not committed: `unaos/Cargo.lock` (+21: the three optional packages, written by the gate's cargo run; not in the file
  list, the fold regenerates it).
- Owed: FACETEDIT-BUILDER (the grant above), FACETEDIT-KNOBON (the first compile and `tests facetedit` under the knob,
  then the flight), FACETEDIT-PREFS (`system.facet.*` in Principia), FACETEDIT-PREVIEW (the in-flight stroke /
  marquee drawn while the button is down), FACETEDIT-ADJFIELDS (adjust parameter fields; defaults only today),
  FACETEDIT-EXPORT (PNG export as the host's Export), FACETEDIT-OWNER (files are written as the kernel principal, as
  Quarry's are), FACETEDIT-DOCK (the dock's Facet tile reads `facet::win()`, NONE while editing), plus RASTERCLI-SELCLIP
  (brush / adjust ignore the selection) inherited from the cores.
