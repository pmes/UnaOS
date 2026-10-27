# RASTERCLI (LEDGER SR233) — tools/raster, the host CLI over raster_core / brush_core / adjust_core

Cut from hw-rmbp @ 85fb9145b on `exec-rmbp-local-rastercli`. Facet's host witness (the cloud's line: "tools/raster
CLI in tools/nle's shape ... so every face op has a host witness"). A thin CLI only: no face, no core change,
nothing in the kernel.

## CHARTER

- Handler: **Facet** (`docs/CODEX.md` §2, Images — "The Canvas. Raster/Vector engine").
- Seam: a host **tool** in `tools/` (the `tools/nle` shape: argument plumbing over the shared cores, no logic of its
  own beyond parsing and printing). First line of `src/main.rs`:
  `// CHARTER: Facet (Images) — host tool over the shared cores (tools/raster, LEDGER SR233)`.
- No preference, no dotfile, nothing in the kernel.

## MEASURE (R131, before the build)

- `tools/nle` (`Cargo.toml`, `src/main.rs`, 175 lines): hand-written JSON (`println!("{{\"frames\":{},...}}")`), no
  serde, no third-party crate; `usage()` exits 2, `die()` exits 1 with `nle: <msg>` on stderr; one JSON line on
  stdout; `fnv` = FNV-1a 64 printed `"{:016x}"`. It has NO `--do` and NO tests: the `--do` grammar and the
  binary-driving test are copied from the tree's other tools (`vessels/stria-view --do`; `tools/play-check/tests`,
  `tools/una-card/tests`: `Command::new(env!("CARGO_BIN_EXE_<bin>"))`).
- `raster_core` (`src/lib.rs`): `Document`, `Layer`, `BlendMode`, `Mask`, `Selection`, `Command`, `History`,
  `encode`, `decode`, `Error` + `word()`. `Document::apply(&self, &Command) -> Result<Document, Error>`;
  `History::commit / undo / redo`; `Selection::magic_wand(doc, layer, x, y, tol) -> Result<Selection, Error>`;
  `Document::composite() -> Vec<u8>`. raster_core does NOT re-export pixel_core; its container writes layers with
  `pixel_core::png::encode::encode_rgba` (`container.rs`), so this tool takes pixel_core as a direct path dependency
  for `export` (the same writer, no second PNG encoder) — the brief's Cargo line allows exactly this case.
- `brush_core`: `Brush` (diameter, hardness, spacing, opacity, flow, pressure, mode), `Stroke { points:
  Vec<StrokePoint { x, y, pressure }> }`, `paint(pixels, w, h, &Brush, &Stroke, rgba)` in place.
- `adjust_core`: one shape `fn(&[u8], w, h, params) -> Result<Vec<u8>, Error>` for `levels(Levels)`,
  `curves(&Curves)` (`Curves::from_points`), `hue_sat(HueSat)`, `invert`, `threshold(u8)`, `box_blur(u16)`,
  `gaussian_blur(u16 sigma_q8)`, `unsharp(Unsharp)`; `Error::word()` = `bad-length` / `bad-param`.
- Owed items the arc files name and this cut answers on the HOST only: BRUSHCORE-LAYER and ADJUSTCORE-CMD (paint /
  adjust a layer undoably) are met here as core call + `Command::PastePixels`, without a new raster_core command.

## FILES

- `tools/raster/Cargo.toml` — bin `raster`, edition 2024, `LGPL-3.0-or-later` (as tools/nle); path deps raster_core,
  brush_core, adjust_core (`default-features = false`), pixel_core (the PNG writer for `export`). No external crate.
- `tools/raster/src/main.rs` — charter, args, the one JSON line, exit codes.
- `tools/raster/src/script.rs` — the `--do` parser: text -> `Vec<Step>` (line number, verb name, `Op`).
- `tools/raster/src/ops.rs` — the verb table: `Op` -> one raster_core `Command` or one core call.
- `tools/raster/tests/cli.rs` — the KATs, driving the built binary.
- Root `Cargo.toml` — one member line beside `tools/nle`.
- `docs/dev/evidence/raster/RASTERCLI.md` (this file), `docs/dev/LEDGER.md` (SR233), `docs/dev/exec-branches.txt`.

## DESIGN

### Invocation
`raster --do <script-file>` (`-` reads stdin). Anything else is a usage error (exit 2).

### The script
One verb per line; `#` starts a comment (to end of line); blank lines are skipped; tokens split on whitespace. The
WHOLE script is parsed before anything runs: a parse error (unknown verb, wrong arity, a number that does not
parse) runs nothing and exits 2. Then every step runs in order against ONE in-memory `Document` and ONE `History`;
the first failing step stops the run (exit 1).

### Verbs (layer indices count from the bottom, 0; colours `rrggbbaa` hex)
| verb | what runs | kind |
|---|---|---|
| `new <w> <h>` | `Document::new(w, h)` then `apply(AddLayer { 0, Layer::new("Background", w, h) })`; history cleared | core call + Command (base state, not undoable) |
| `open <path>` | `raster_core::decode(bytes)`; history cleared | core call (base state) |
| `save <path>` | `raster_core::encode(doc)` written to the path | core call (no mutation) |
| `layer add <name>` | `AddLayer { index: len, Layer::new(name, W, H) }` (on top, canvas-sized, transparent) | Command |
| `layer remove <i>` | `RemoveLayer` | Command |
| `layer move <from> <to>` | `MoveLayer` | Command |
| `layer opacity <i> <0-255>` | `SetOpacity` | Command |
| `layer blend <i> <normal\|multiply\|screen\|overlay\|darken\|lighten\|difference\|erase>` | `SetBlend` | Command |
| `layer visible <i> <0\|1>` | `SetVisible` | Command |
| `layer offset <i> <x> <y>` | `SetOffset` | Command |
| `layer rename <i> <name>` | `Rename` | Command |
| `select rect <x> <y> <w> <h>` | `SetSelection(Some(Rect))` | Command |
| `select ellipse <cx> <cy> <rx> <ry>` | `SetSelection(Some(Ellipse))` | Command |
| `select poly x1,y1 x2,y2 ...` | `SetSelection(Some(Polygon))` | Command |
| `select wand <layer> <x> <y> <tol>` | `Selection::magic_wand` then `SetSelection` | core call + Command |
| `select none` | `SetSelection(None)` | Command |
| `fill <layer> <rrggbbaa>` | `FillSelection` | Command |
| `clear <layer>` | `ClearSelection` | Command |
| `brush <layer> <rrggbbaa> <diameter> <hardness> <spacing> <flow> <normal\|erase> <x,y[,p]> ...` | `brush_core::paint` on a copy of the layer's pixels (points in LAYER coordinates, pressure default 255; `Brush::new(diameter)` with the given hardness / spacing / flow / mode, opacity 255, Linear pressure), then `PastePixels { layer, 0, 0, w, h, pixels }` | core call + Command |
| `adjust <layer> levels <in_lo> <in_hi> <gamma_q8> <out_lo> <out_hi>` | `adjust_core::levels` then `PastePixels` | core call + Command |
| `adjust <layer> curves <x,y> ...` | `Curves { master: Curves::from_points(..), r/g/b: None }`, `adjust_core::curves`, `PastePixels` | core call + Command |
| `adjust <layer> huesat <hue> <sat> <light>` | `adjust_core::hue_sat` then `PastePixels` | core call + Command |
| `adjust <layer> invert` | `adjust_core::invert` then `PastePixels` | core call + Command |
| `adjust <layer> threshold <level>` | `adjust_core::threshold` then `PastePixels` | core call + Command |
| `adjust <layer> boxblur <r>` | `adjust_core::box_blur` then `PastePixels` | core call + Command |
| `adjust <layer> gaussian <sigma_q8>` | `adjust_core::gaussian_blur` then `PastePixels` | core call + Command |
| `adjust <layer> unsharp <amount> <sigma_q8> <threshold>` | `adjust_core::unsharp` then `PastePixels` | core call + Command |
| `undo` / `redo` | `History::undo` / `History::redo` | History |
| `export <path.png>` | `Document::composite()` -> `pixel_core::png::encode::encode_rgba` | core call (no mutation) |
| `stat` | nothing (the final JSON carries the state) | — |

Every mutating verb goes through `History::commit` (= `Document::apply` + push), so `undo` / `redo` reach all of
them. `brush` and `adjust` act on the whole layer; the selection does not clip them (RASTERCLI-SELCLIP, owed).

### The JSON status line (stdout, exactly one line, nle's hand-written style)
`{"ok":<bool>,"verbs":<n run ok>,"last":"<verb>","line":<script line>,"width":W,"height":H,"layers":L,`
`"undo":<history past>,"redo":<history future>,"selection":"<none|rect|ellipse|poly|coverage>","fnv":"<016x>","error":"<word>"}`
- `verbs`: steps that completed; `last`: the name of the last step run (the failing one on error; `layer add`,
  `select rect`, `adjust invert` name their sub-verb); `line`: its 1-based script line (0 when nothing ran).
- `width` / `height` / `layers` / `selection`: the current document (0 / 0 / 0 / `none` before `new` / `open`).
- `fnv`: FNV-1a 64 of `Document::composite()` (nle's fnv), `0000000000000000` with no document.
- `error`: `''` on ok; else raster_core's `Error::word()`, adjust_core's `Error::word()`, or the tool's own words
  for what the cores cannot say: `script` (parse), `usage`, `io` (a file read / write), `no-document` (a verb before
  `new` / `open`), `no-undo` / `no-redo` (empty history side), `png-encode` (pixel_core's writer refused; raster_core's
  own word for it).
- The JSON escapes `"` and `\` and control characters in strings (names reach `last` only as verb words).

### Exit codes
`0` ok; `1` a verb failed at run time (the line names it in `last` and `error`); `2` a usage or script error
(nothing ran; `ok:false`, `error:"script"` or `"usage"`, `last` = the offending verb word, `line` = its line).

## BLUEPRINT ROWS

Read whole before the first edit: `docs/dev/blueprint/handlers.md` (119 lines) and `docs/dev/blueprint/shape.md`.
- Kept: BP-handlers-1 (manifest unchanged), BP-handlers-2 (purpose from CODEX §2 Facet), BP-handlers-3 (no kernel
  code; the tool's first line names owner and seam), BP-handlers-4 (the tool links the shared cores, keeps no copy),
  BP-handlers-5 (no preference), BP-shape-4 (no board names), BP-shape-10 (no new dependency: path deps only),
  BP-shape-11 (licence as tools/nle).
- `changed=none`. handlers.md is not touched (119 lines, at its cap). Proposed row (report only): BP-handlers-15 —
  "Facet's editor is three shared cores, raster_core (SR230) / brush_core (SR231) / adjust_core (SR232); `tools/raster`
  (SR233) is their headless face (one `--do` script, one JSON line) and holds no logic; every mutation is one
  `Document::apply` on one `History`."

## LEDGER ROW

`| SR233 | **RASTERCLI — ...` in `docs/dev/LEDGER.md` (status `fixed-unflown`).

## KATs (`tools/raster/tests/cli.rs`, driving the binary)

1. `new_reports_size` — `new 8 8` -> exit 0, `ok:true`, width 8, height 8, layers 1.
2. `fill_rect_exports_red` — `new 8 8`, `layer add top`, `select rect 2 2 3 3`, `fill 1 ff0000ff`, `export` -> the PNG
   (pixel_core::decode_png) is red inside the rect, transparent outside.
3. `brush_centre_dab` — 32x32, `brush 0 00ff00ff 9 255 0.25 255 normal 16,16` -> centre pixel green opaque, corner
   transparent.
4. `invert_twice_identity` — a filled + brushed layer exported, then the same with `adjust 0 invert` twice: the two
   PNG files are byte-identical.
5. `undo_redo_layer_add` — `layer add x`, `undo` -> layers 1, `redo` -> layers 2.
6. `save_open_same_stat` — script A builds and `save`s; script B `open`s: width, height, layers, selection and fnv
   equal.
7. `errors_exit_codes` — an unknown verb -> exit 2, `ok:false`, `error:"script"`; `layer opacity 5 10` on one layer ->
   exit 1, `last:"layer opacity"`, `error:"no-layer"`.
8. `comments_and_blanks` — a script with a `#` comment line, a trailing comment and a blank line runs (`verbs` counts
   the real steps).

## MEASURED

- Command (repo root, the seat's one compile, R107/R113):
  `CARGO_TARGET_DIR=~/unaos-bench/scratch/rmbp-0915/rastercli/target cargo test -p raster` — exit 0, first compile,
  0 warnings (`grep -c ^warning` on the log = 0): bin unit tests 0, `tests/cli.rs` **8 passed** (the 8 KATs above).
  Log: `~/unaos-bench/scratch/rmbp-0915/rastercli/test1.log`.
- Anchors: `State::commit` `src/ops.rs:29` (the one `History::commit`), `State::paste` `:37` (core call +
  `PastePixels`), `State::run` `:44` (the verb table), `brush` `:120` (`brush_core::paint`), `adjust` `:135` (one
  adjust_core call per kind); `parse` `src/script.rs:315`, `name_of` `:276`; `status` `src/main.rs:40` (the one JSON
  line), `main` `:69` (exit codes).
- Grants needed in the cores: none. pixel_core is a direct path dependency for `export` (raster_core does not
  re-export it; the writer is the one raster_core's container calls, `pixel_core::png::encode::encode_rgba`).
- Not committed: the root `Cargo.lock` gains a 10-line `raster` package entry (deps adjust_core, brush_core,
  pixel_core, raster_core) when the member is added; Cargo.lock is not in this arc's file list, so it is left to the
  seat's fold (a `cargo` run regenerates the same entry), as SR230-SR232 did.
- Owed: RASTERCLI-SELCLIP (`brush` / `adjust` ignore the selection; a selection-clipped paste needs a raster_core
  command or a masked paste); RASTERCLI-MASK (no verb for `SetMask`); RASTERCLI-PRESSURE (the brush's opacity and
  pressure curve are fixed at `Brush::new`'s Linear / 255); RASTERCLI-CANVAS (brush points are layer coordinates, not
  canvas coordinates through the layer offset).
- Files: `src/main.rs` 108, `src/ops.rs` 150, `src/script.rs` 330, `tests/cli.rs` 159 lines (`wc -l`).
