# BRUSHCORE (LEDGER SR231) — brush_core, the no_std brush engine

Cut from hw-rmbp @ e925b9a35 on `exec-rmbp-local-brushcore`. Facet's second core (the cloud's line: "Brush engine
as a second crate brush_core (dab spacing, hardness, pressure curve, eraser as a blend mode)"). It paints a stroke
onto one RGBA8 buffer. Adjustments, the CLI, a face, raster_core commands and the kernel are not in this cut.

## CHARTER

- Handler: **Facet** (`docs/CODEX.md` §2, Images — "The Canvas. Raster/Vector engine").
- Seam: **shared-core** — a `no_std` + `alloc` lib under `unaos/libs/media/` beside raster_core (BP-handlers-4,
  BP-shape-2). First line of `src/lib.rs`: `// CHARTER: Facet (Images) — shared-core (brush_core, LEDGER SR231)`.
- No preference, no dotfile, nothing in the kernel. Nothing is std-gated, so a later face's
  `default-features = false` link is honest now.

## FILES

- `unaos/libs/media/brush_core/Cargo.toml` — raster_core's shape (edition 2024, `LGPL-3.0-or-later`,
  `default = ["std"]`, `std = []`); one dependency, `raster_core = { path = "../raster_core", default-features = false }`.
  No pixel_core line: raster_core exposes the one blend rule publicly (`raster_core::composite::blend_pixel`,
  `unaos/libs/media/raster_core/src/composite.rs:58`), with both short-cuts inside it (`over`, `:47`).
- `src/lib.rs` — charter, module map, re-exports, the fixed-point helpers (`floor`, `round`).
- `src/brush.rs` — `Brush`, `PressureCurve`, `radius_at`, `flow_at`.
- `src/stroke.rs` — `StrokePoint`, `Stroke`, `Dab`, `dabs`.
- `src/stamp.rs` — `Stamp`, `stamp`, `isqrt`.
- `src/apply.rs` — `apply`, `paint`.
- `tests/kats.rs` — the host KATs.
- Root `Cargo.toml` — one member line beside raster_core.
- This file, `docs/dev/LEDGER.md` (SR231), `docs/dev/exec-branches.txt`.

## DESIGN

### Types
- `Brush { diameter: f32, hardness: u8, spacing: f32, opacity: u8, flow: u8, pressure: PressureCurve, mode: BlendMode }`.
  `Brush::new(diameter)`: hardness 255, spacing 0.25, opacity 255, flow 255, `Linear`, `Normal`.
- `PressureCurve { Linear, Quadratic, Lut([u8; 256]) }`, `map(&self, p: u8) -> u8`:
  Linear `p`; Quadratic `round(p * p / 255)` = `(p*p + 127) / 255`; Lut `table[p]`.
- `StrokePoint { x: f32, y: f32, pressure: u8 }`, `Stroke { points: Vec<StrokePoint> }`.
- `Dab { x: f32, y: f32, radius: f32, flow: u8, hardness: u8 }` — the brief's dab plus `hardness`, copied from the
  brush by `dabs`, so `stamp(&Dab)` keeps the brief's one-argument signature.
- `Stamp { x0: i32, y0: i32, w: u32, h: u32, coverage: Vec<u8>, flow: u8 }` — the brief's stamp plus `flow`:
  `apply` takes no dab, so the dab's flow rides in its stamp (`stamp` copies it) to reach the per-pixel alpha.
  `Stamp::at(x, y)` reads coverage in canvas coordinates (0 outside).

### The two pressure formulas (`brush.rs`)
- `radius_at(p) = (diameter / 2) * map(p) / 255` (px, f32).
- `flow_at(p) = round(flow * map(p) / 255)` = `(flow * map(p) + 127) / 255` (integer).
- So pressure 0 gives radius 0 and flow 0 under every curve whose `map(0) = 0`; pressure 255 under Linear gives
  `diameter / 2` and `flow` exactly.

### Dab spacing (`stroke.rs` `dabs`)
- Step = `spacing * diameter` px of arc length, the NOMINAL diameter (pressure does not change the step), floored at
  `MIN_STEP = 0.25` px so a zero or negative spacing / diameter cannot loop forever.
- The first dab is at the first point. The walk carries `to_next` (the arc length still owed before the next dab)
  across segments: on a segment of length `L`, dabs fall at `t = to_next, to_next + step, ...` while
  `t <= L + EPS` (`EPS = 1e-3` px, so a dab due exactly on a vertex is not lost to rounding), at the fraction
  `clamp(t / L, 0, 1)`; then `to_next = t - L`. Zero-length segments (`L < EPS`) emit nothing.
- Pressure at a dab is the endpoints' pressures interpolated at that fraction and rounded; its radius and flow are
  `radius_at` / `flow_at` of it.
- An empty stroke yields no dab; a one-point stroke yields one. At most `MAX_DABS = 2^20` dabs per stroke. The
  dab position is `to_next + k * step` with an integer `k` (an f32 accumulator stops moving on a long segment).

### The coverage curve (`stamp.rs` `stamp`)
- Geometry in 8-bit fixed point (1/256 px). The bounding box is `floor(x - r) .. ceil(x + r) + 1` per axis (one
  pixel of rim for the anti-aliasing); pixel centres sample at `(px + 0.5, py + 0.5)`.
- Distance: `d² = dx² + dy²` in 1/65536 px² (i64); a pixel with `d² >= (r + 0.5)²` is 0 without a root. Otherwise
  `d = isqrt(d²)` in 1/256 px — **an integer Newton iteration on u64** (`isqrt`), no float root, no `libm`.
- Inner radius `ri = r * hardness / 255`. The radial profile `g(t)`: 1 for `t <= ri`; `(r - t) / (r - ri)` for
  `ri < t < r` (LINEAR in distance); 0 for `t >= r` — 255 inside `ri`, falling to 0 at `r`.
- Rim anti-aliasing by sub-pixel distance: a pixel whose centre is at distance `d` takes `g` box-filtered over one
  pixel of distance, `coverage = 255 * (G(d + 1/2) - G(d - 1/2))`, where `G` is the integral of `g` (`G(s) = s` for
  `s <= ri`; `ri + u - u² / (2 (r - ri))` with `u = s - ri` on the ramp; `ri + (r - ri) / 2` past `r`), all in
  1/256-px integers (`integral`, `stamp.rs:66`). For a hard dab (`ri = r`) that is exactly
  `clamp(1/2 + r - d, 0, 1)` (a pixel whose centre is on the circle is half covered), 255 wherever `d <= r - 1/2`.
  The filter keeps the profile's area, and `g` non-increasing makes the coverage non-increasing from the centre.
  (The draft of this file had `min(profile, rim)`; that cuts the hard rim's outer half and loses ~half a pixel ring
  of area, so the box filter replaced it before the code was written.)
- A dab of radius `<= 0`, above `MAX_RADIUS = 4096` px, or centred beyond `±1e9` / non-finite is an empty stamp
  (`w = h = 0`): nothing above a ~67 MB mask is allocated and every box coordinate stays in i32.

### The blend rule (`apply.rs`), COPIED from raster_core
- Per stamp pixel inside the canvas: `a = round(rgba.a * coverage * flow / 255²)` = `(rgba.a * c * flow + 32512) / 65025`.
- The pixel goes through `raster_core::composite::blend_pixel(mode, rgba, a, dst)` — the same call raster_core's
  `layer_onto` makes (`composite.rs:90`; `blend_pixel` `:58`, `over` `:47`, the one `blend_nonpremult` call `:54`): Normal = src-over through pixel_core's `blend_nonpremult`, with the two
  exact identities (an opaque source IS the result; over a transparent destination the source IS the result —
  `blend_nonpremult` returns `src - 1` for an opaque source); the separable modes choose the colour first
  (W3C Compositing §5.8) then the same src-over; `Erase` is destination-out, `dst_a = round(dst_a * (255 - a) / 255)`,
  colour kept. brush_core writes no compositor of its own.
- Off-canvas pixels are skipped (the stamp's box is clipped to `0..w, 0..h` in i64); a buffer whose length is not
  `w * h * 4` is left untouched.
- `paint(pixels, w, h, brush, stroke, rgba)`: `rgba.a` is first scaled by `brush.opacity`
  (`round(a * opacity / 255)`), then `dabs` → `stamp` → `apply` in order, mode `brush.mode`. Opacity here is a
  per-dab cap, not a per-stroke ceiling: overlapping dabs build past it (a stroke buffer is owed, BRUSHCORE-STROKEBUF).

### Integer vs float
- f32 for geometry only: point positions, arc length, the step, radius, and the fractions; the segment length uses
  the same `isqrt` on a 1/256-px fixed-point d² (no float root anywhere). Coverage, alpha and every pixel byte are
  integer. `floor` / `round` are written by hand (`f32::floor` is std-only).

## BLUEPRINT ROWS

Read whole before the first edit: `docs/dev/blueprint/handlers.md` (119 lines) and `docs/dev/blueprint/shape.md`.
- Kept: BP-handlers-1 (manifest unchanged), BP-handlers-2 (purpose from CODEX §2 Facet), BP-handlers-3 (no kernel
  code; the lib's first line names owner and seam), BP-handlers-4 (one no_std core for both faces — the face is a
  later cut), BP-handlers-5 (no preference), BP-shape-2 (the core lives in `unaos/libs/media/`), BP-shape-4 (no board
  names), BP-shape-10 (raster_core only, in-tree; no external dependency), BP-shape-11 (licence follows the sibling
  cores).
- `changed=none`. handlers.md is not touched (119 lines, its cap); proposed for when a face links it: the
  BP-handlers-4 list gains `Facet raster_core / brush_core (RASTERCORE SR230, BRUSHCORE SR231)`.

## LEDGER ROW

`| SR231 | **BRUSHCORE — Facet had a layered document (raster_core, SR230) and nothing to paint it with …** |` — the
full row is in `docs/dev/LEDGER.md` (status `fixed-unflown`).

## KATs (`unaos/libs/media/brush_core/tests/kats.rs`)

1. `straight_stroke_41_dabs` — (0,0)→(100,0), diameter 10, spacing 0.25: 41 dabs, first at (0,0), last at (100,0).
2. `one_point_one_dab` — one point: one dab at it; an empty stroke: none.
3. `pressure_curves` — Linear: pressure 0 gives radius 0 and flow 0, 255 gives `diameter / 2` and `flow`;
   Quadratic(128) < Linear(128); a Lut maps every p to its table entry.
4. `hard_and_soft_dabs` — hardness 255, radius 4: total coverage / 255 within 2 of `pi * 16`, every pixel with
   centre distance `<= 3.5` is 255; hardness 0: coverage non-increasing along the row from the centre to the rim.
5. `opaque_normal_dab_alpha_is_coverage` — one opaque Normal dab at (8,8) on a transparent 16x16: every pixel's
   alpha equals the stamp's coverage there, the centre's colour is the brush colour.
6. `erase_dab_keeps_colour` — an Erase dab over an opaque buffer: centre alpha 0, colour bytes unchanged.
7. `flow_accumulates` — flow 128 once vs twice on the same spot: twice > once, twice < 255.
8. `off_canvas_dab_clips` — a dab at (-3,-3), radius 5: no panic; painted pixels = the stamp's nonzero pixels that
   fall on the canvas, each alpha equal to its coverage.

## MEASURED

- Command (repo root, the seat's one compile, R107/R113):
  `CARGO_TARGET_DIR=~/unaos-bench/scratch/rmbp-0915/brushcore/target cargo test -p brush_core` — exit 0, first
  compile, no warnings: unit 0, `tests/kats.rs` **8 passed** (the 8 KATs above), doc-tests 0.
- Before the compile, the stamp arithmetic was replayed in Python (the same integer `G`, `math.isqrt`): hard radius 4
  at (8,8) has total coverage / 255 = 50.73 against `pi * 16 = 50.27` (60 nonzero pixels); the soft radius 4 row from
  the centre is `[210, 154, 93, 29, 0]`. KAT 4 asserts the first within 2 and the second monotone.
- KAT 7: one flow-128 dab gives centre alpha 128 (`(255 * 255 * 128 + 32512) / 65025`); the second goes through
  `blend_nonpremult` (alpha `128 + (128 * (256 - 128) >> 8) = 192`), asserted `> 128` and `< 255`.
- Anchors: `PressureCurve::map` `src/brush.rs:23`, `radius_at` `:64`, `flow_at` `:69`; `dabs` `src/stroke.rs:57`,
  `seg_len` `:46`; `isqrt` `src/stamp.rs:49`, `integral` `:66`, `stamp` `:78`; `apply` `src/apply.rs:20` (the one
  blend call `:44`), `paint` `:51`.
- Not committed: the root `Cargo.lock` gains the `brush_core` package entry when the workspace member is added;
  Cargo.lock is not in this arc's file list, so the commit leaves it to the seat's fold (a `cargo` run regenerates the
  same entry).
- Files: `src/apply.rs` 56, `src/brush.rs` 72, `src/lib.rs` 54, `src/stamp.rs` 114, `src/stroke.rs` 90,
  `tests/kats.rs` 177 lines (`wc -l`).
- Grants needed in raster_core / pixel_core: none (`composite::blend_pixel` and `BlendMode` are already public).

## OWED

- BRUSHCORE-STROKEBUF: brush opacity is a per-dab cap; a per-stroke ceiling (overlapping dabs never exceeding
  `opacity`) needs a stroke coverage buffer composited once at stroke end.
- BRUSHCORE-LAYER: painting onto a raster_core `Layer` as a `Command` (undoable) — a raster_core command, out of this
  cut by the brief.
- BRUSHCORE-SHAPES: elliptical / textured tips, angle, jitter, smoothing of the input polyline.
