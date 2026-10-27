# RASTERCORE (LEDGER SR230) — raster_core, the no_std raster document core

Cut from hw-rmbp @ 3376c4c69 on `exec-rmbp-local-rastercore`. The first cut of Facet's editor ("the Photoshop
killer"): the document model only. Brush engine (brush_core), adjustments (adjust_core), the CLI, the host face
and the kernel face are LATER cuts and are not in this one.

## CHARTER

- Handler: **Facet** (`docs/CODEX.md:31`, Images — "The Canvas. Raster/Vector engine").
- Seam: **shared-core** — a `no_std` + `alloc` lib under `unaos/libs/media/` that both rings can link (the
  midden_core / timeline_core shape, BP-handlers-4, BP-shape-2). First line of `src/lib.rs`:
  `// CHARTER: Facet (Images) — shared-core (raster_core, LEDGER SR230)`.
- Why no kernel code yet: there is no face to drive it. The kernel viewer (`unaos/crates/kernel/src/video/facet.rs`)
  only shows images; an editing face (host or kernel) is a later cut and links this crate by path with
  `default-features = false` the day it exists. Nothing is std-gated, so that link is honest now.
- No preference, no dotfile, nothing in the kernel.

## FILES

- `unaos/libs/media/raster_core/Cargo.toml` — timeline_core's shape: `default = ["std"]`, `std = []`, licence
  `LGPL-3.0-or-later`; one dependency, `pixel_core` (path `../pixel_core`; it has no `std` feature —
  `default = []` — so no `default-features` line is needed).
- `unaos/libs/media/raster_core/src/lib.rs` — charter, module map, `Error` (+ `word`).
- `src/document.rs` — `Document`, `Layer`, `BlendMode`, `Mask`, `validate`.
- `src/selection.rs` — `Selection`, `rasterize`, `magic_wand`.
- `src/composite.rs` — `Document::composite`, the blend-mode formulas.
- `src/command.rs` — `Command`, `Document::apply`, `History`.
- `src/container.rs` — `encode` / `decode` (the `UNARASTR` chunked container).
- `unaos/libs/media/raster_core/tests/kats.rs` — the host KATs.
- `Cargo.toml` (root) — one workspace member line beside timeline_core.
- `docs/dev/evidence/raster/RASTERCORE.md` (this file), `docs/dev/LEDGER.md` (SR230), `docs/dev/exec-branches.txt`.

## DESIGN

### Types
- `Document { width: u32, height: u32, layers: Vec<Layer>, selection: Option<Selection> }` — layers bottom first.
- `Layer { name: String, pixels: Vec<u8>, width: u32, height: u32, offset: (i32, i32), opacity: u8,
  blend: BlendMode, visible: bool, mask: Option<Mask> }` — pixels are straight (non-premultiplied) RGBA8,
  row-major, `width * height * 4` bytes, pixel_core's own layout. `offset` places the layer's (0,0) on the canvas.
- `BlendMode { Normal, Multiply, Screen, Overlay, Darken, Lighten, Difference, Erase }` (wire byte 0..=7).
- `Mask { width, height, coverage: Vec<u8> }` — 0 = hidden, 255 = shown; a layer mask has the layer's size.
- `Selection { Rect{x,y,w,h}, Ellipse{cx,cy,rx,ry}, Polygon(Vec<(i32,i32)>), Coverage(Mask) }` in canvas
  coordinates. `rasterize(&self, w, h) -> Mask` samples pixel CENTRES (`x + 0.5`, `y + 0.5`): a rect covers its
  cells; an ellipse covers centres with `((x+.5-cx)/rx)^2 + ((y+.5-cy)/ry)^2 <= 1` (integer arithmetic in i64);
  a polygon is filled even-odd by scanline at each row's centre (half-open edge rule `y0 <= yc` != `y1 <= yc`);
  a coverage is copied (clipped/zero-extended to `w x h`).
- `Selection::magic_wand(doc, layer, x, y, tolerance) -> Result<Selection, Error>` — 4-connected flood fill (an
  explicit stack, no recursion) over the layer's pixels seen on the canvas, from canvas pixel `(x, y)`; a pixel joins
  when its Chebyshev distance over RGBA (`max |c - seed|` per channel) is `<= tolerance`; the result is a
  canvas-sized `Coverage` (255 in, 0 out). Tolerance 255 takes every connected layer pixel on the canvas.
  (The brief wrote `-> Selection`; a `Result` names a bad layer index or an off-canvas seed instead of guessing.)

### The one compositor
`Document::composite(&self) -> Vec<u8>` (RGBA8, `w*h*4`, starts transparent): bottom layer first, hidden layers
skipped. Per canvas pixel under a layer: the layer's alpha is scaled by its opacity and its mask,
`a' = round(round(a * opacity / 255) * m / 255)`. Then:
- **Normal**: `blend_nonpremult([r,g,b,a'], dst)` — pixel_core's (`unaos/libs/media/pixel_core/src/lib.rs:253`).
- **Separable modes** (Multiply, Screen, Overlay, Darken, Lighten, Difference): the W3C Compositing 1 §5.8 colour
  `Cs' = ((255 - ab) * Cs + ab * B(Cb, Cs)) / 255` (rounded), then the SAME `blend_nonpremult([Cs', a'], dst)` for the
  alpha composite. B per channel (0..255 integers, rounded): Multiply `b*s/255`; Screen `b + s - b*s/255`; Overlay
  `b < 128 ? 2*b*s/255 : 255 - 2*(255-b)*(255-s)/255`; Darken `min`; Lighten `max`; Difference `|b - s|`.
- **Erase** (the eraser as a blend mode): destination-out, `dst_a = round(dst_a * (255 - a') / 255)`, colour kept.
  Src-over cannot remove alpha, so this is not a second src-over; it is the only arm that does not call
  `blend_nonpremult`, and it composites no colour.
- Two exact identities of src-over are taken before the call: `a' == 255` gives the source pixel, `dst_a == 0` gives
  `[Cs', a']`. Measured reason: `blend_nonpremult` with an opaque source returns `src - 1` per channel
  (`scale = floor(2^24 / 255) = 65793`, `255 * 65793 = 2^24 - 1`; KAT `blend_nonpremult_opaque_off_by_one`), and
  libwebp's own caller (`BlendPixelRowNonPremult`) skips opaque sources for the same reason. Every partial-alpha
  pixel goes through `blend_nonpremult`; no other src-over formula exists in the crate.

### The command shape (timeline_core's)
- `Command { AddLayer{index, layer}, RemoveLayer{index}, MoveLayer{from,to}, SetOpacity{layer,opacity},
  SetBlend{layer,blend}, SetVisible{layer,visible}, SetOffset{layer,offset}, Rename{layer,name},
  FillSelection{layer, rgba}, ClearSelection{layer}, SetMask{layer, mask: Option<Mask>},
  SetSelection(Option<Selection>), PastePixels{layer, x, y, w, h, rgba} }`.
- `Document::apply(&self, &Command) -> Result<Document, Error>`: pure; clones, edits the clone, calls
  `validate` at the end and returns it (timeline_core's `&self -> Result<Timeline, EditError>` + `validate`).
- `FillSelection` / `ClearSelection` act where the document's selection (none = the whole layer) covers the layer's
  pixel (the selection is canvas space; the layer's offset is subtracted): coverage 255 replaces the pixel
  (fill: `rgba`; clear: `[0,0,0,0]`); partial coverage `c` fills through `blend_nonpremult([rgb, a*c/255], px)` and
  clears by `a = a * (255 - c) / 255`.
- `PastePixels` writes `w x h` RGBA into the layer at layer coordinates `(x, y)`; outside the layer or a wrong length
  is `Error::Bounds` / `Error::Dimensions`.
- `Document::validate(&self) -> Result<(), Error>`: canvas `>= 1x1` and within pixel_core's `MAX_DIM`; every layer
  `>= 1x1`, `pixels.len() == w*h*4`; opacity is a `u8` (0..=255 by type); every mask has its layer's size and
  `coverage.len() == w*h`; the selection lies inside the canvas (rect / ellipse bounding box / every polygon vertex
  in `0..=W, 0..=H`, a coverage the canvas's size).
- `History { past: Vec<Document>, future: Vec<Document> }`: `push(prev)` (clears `future`), `undo(current) ->
  Option<Document>`, `redo(current) -> Option<Document>`, and `commit(&mut self, current, cmd)` = apply + push.
  It holds whole documents — the simplest correct shape. **Memory cost**: every step keeps a full copy of every
  layer's pixels (a 4000x3000 four-layer document is ~192 MB per step). A later cut turns the stack into inverse
  commands (or copy-on-write tiles) behind the same `push` / `undo` / `redo`.

### The container (`UNARASTR`, version 1, little-endian throughout)
```
header (16 bytes):  magic b"UNARASTR" (8) | version u32 = 1 | chunk_count u32
chunk:              tag [u8;4] | len u32 | payload[len]
META (exactly one, first): width u32 | height u32 | layer_count u32 |
                    per layer: name_len u32 | name UTF-8 | w u32 | h u32 | ox i32 | oy i32 |
                               opacity u8 | blend u8 | visible u8 | has_mask u8
LAYR (one per layer, in layer order): a PNG (colour type 6) written by pixel_core's
                    `png::encode::encode_rgba`, read by `decode_png`; its size must equal META's w x h
MASK (one per masked layer): layer_index u32 | w u32 | h u32 | coverage RAW (w*h bytes)
SELE (optional, at most one): kind u8 (0 rect, 1 ellipse, 2 polygon, 3 coverage) |
                    rect/ellipse: 4 x i32 | polygon: n u32, n x (i32, i32) | coverage: w u32 | h u32 | bytes
```
- Mask coverage is RAW in this cut (zlib through pixel_core's inflater is a later cut; there is no deflate writer
  exported for arbitrary bytes besides the PNG path).
- Unknown tags are skipped (forward compatible); a missing or second META, a LAYR count unequal to META's, a
  bad UTF-8 name, a bad blend byte or a short payload is `Error::Malformed`; a chunk running past the end or a
  header under 16 bytes is `Error::Truncated`; wrong magic `Error::Magic`; another version `Error::Version`.
  The decoded document goes through `validate`.
- `encode(&Document) -> Result<Vec<u8>, Error>` (it validates first; pixel_core's writer can refuse);
  `decode(&[u8]) -> Result<Document, Error>`.

`Error` (no `Display`): `Empty, TooLarge, NoLayer, Dimensions, Mask, Selection, Bounds, Truncated, Magic, Version,
Malformed, Png, PngEncode` with `fn word(&self) -> &'static str`.

## BLUEPRINT ROWS

Read whole before the first edit: `docs/dev/blueprint/handlers.md` (119 lines) and `docs/dev/blueprint/shape.md`.
- Kept: BP-handlers-1 (manifest unchanged), BP-handlers-2 (purpose from CODEX §2 Facet), BP-handlers-3 (no kernel
  code; the lib's first line names owner and seam), BP-handlers-4 (one no_std core for both faces — the face is a
  later cut), BP-handlers-5 (no preference), BP-shape-2 (the core lives in `unaos/libs/media/`), BP-shape-4 (no board
  names), BP-shape-10 (pixel_core, in-tree; no external dependency), BP-shape-11 (licence follows the sibling cores).
- `changed=none`. handlers.md is not touched (119 lines, at its cap); proposed row for when a face links it: the
  BP-handlers-4 list gains `Facet raster_core (RASTERCORE SR230)`.

## LEDGER ROW

`| SR230 | **RASTERCORE — Facet had no editing model: no layered document, no selection, no undo, no file format of
its own; the cloud's instruction (2026-10-27) asks for the core first, no_std, host KATs.** …` — the full row is in
`docs/dev/LEDGER.md` (status `fixed-unflown`).

## KATs (`unaos/libs/media/raster_core/tests/kats.rs`)

1. `composite_normal_4x4` — a 4x4 opaque blue base under a 2x2 red layer at offset (1,1): opacity 255 gives red
   exactly in the 4 covered pixels and blue elsewhere; opacity 128 gives `[127, 0, 126, 255]` (hand-computed through
   `blend_nonpremult`: `a' = 128`, `dfa = 127`, `scale = 65793`).
2. `separable_modes_one_pair` — base `[200, 100, 50, 255]`, top `[100, 200, 250, 255]` at opacity 255: Multiply
   `[78, 78, 49]`, Screen `[222, 222, 251]`, Difference `[100, 100, 200]`; and `blend_nonpremult_opaque_off_by_one`
   pins the pixel_core fact the identity short-cut exists for.
3. `erase_clears_alpha` — an Erase layer of alpha 255 over an opaque pixel leaves alpha 0.
4. `selection_counts` — rect 5x3 = 15; ellipse (8,8,5,3) = 48 against `pi*5*3 = 47.12` (within 1); triangle
   (1,1)-(21,4)-(6,17) = 152 against shoelace 152.5 (within 2) — counts measured beforehand by the same
   pixel-centre rule.
5. `magic_wand_regions` — 8x8 left half colour A plus an isolated A pixel at (6,6) inside B: tolerance 0 from (1,1)
   selects exactly the 32 left-half pixels (not the island); tolerance 255 selects all 64.
6. `every_command_undo_redo` — each of the 13 commands on a 3-layer document: apply, push, undo gives the original,
   redo gives the edited, and the edit differs from the original.
7. `container_round_trip` — a document with a mask, a polygon selection, every field off-default: `decode(encode(d))
   == d`.
8. `container_refusals` — a truncated container is `Error::Truncated`, a wrong magic `Error::Magic`.
9. `validate_rejects_bad_pixels` — a layer whose pixel length is short is `Error::Dimensions` (directly and via
   `apply`).

## MEASURED

- Command (repo root, the seat's one compile, R107/R113):
  `CARGO_TARGET_DIR=~/unaos-bench/scratch/rmbp-0915/rastercore/target cargo test -p raster_core` — exit 0, first
  compile, no warnings: unit 0, `tests/kats.rs` **10 passed** (the 9 KATs above + `blend_nonpremult_opaque_off_by_one`),
  doc-tests 0.
- Anchors: the one src-over call `src/composite.rs:54` (inside `over`, `:47`); the mode arms `blend_pixel` `:58`;
  `Document::composite` `:98`; `Document::apply` `src/command.rs:60` (validate at its end); `History` `:142`;
  `Document::validate` `src/document.rs:176`; `Selection::rasterize` `src/selection.rs:57`, `magic_wand` `:132`;
  `encode` `src/container.rs:53`, `decode` `:171`.
- The identity short-cut has a sibling in pixel_core's own APNG compositor:
  `unaos/libs/media/pixel_core/src/png/decode.rs:513` (`src[3] == 255 || canvas[d + 3] == 0` -> the source).
- Signature notes against the brief: `encode` returns `Result<Vec<u8>, Error>` (pixel_core's PNG writer can refuse,
  and `encode` validates first); `magic_wand` returns `Result<Selection, Error>` (a bad layer index or off-canvas seed
  is named, not guessed).
- Not committed: the root `Cargo.lock` gains the 7-line `raster_core` package entry when the workspace member is
  added; Cargo.lock is not in this arc's file list, so the commit leaves it to the seat's fold (a `cargo` run
  regenerates the same entry).
- Files: `src/command.rs` 178, `src/composite.rs` 106, `src/container.rs` 284, `src/document.rs` 197, `src/lib.rs` 86,
  `src/selection.rs` 162, `tests/kats.rs` 272 lines (`wc -l`).
