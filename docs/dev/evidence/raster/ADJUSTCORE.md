# ADJUSTCORE (LEDGER SR232) — adjust_core, the no_std image adjustments as pure functions

Cut from hw-rmbp @ e925b9a35 on `exec-rmbp-local-adjustcore`. Facet's third core (after raster_core SR230 and the
brush cut): the adjustments only. No brush, no CLI, no face, no raster_core command, nothing in the kernel.

## CHARTER

- Handler: **Facet** (`docs/CODEX.md` §2, Images — "The Canvas. Raster/Vector engine").
- Seam: **shared-core** — a `no_std` + `alloc` lib under `unaos/libs/media/` both rings can link (the timeline_core /
  raster_core shape, BP-handlers-4, BP-shape-2). First line of `src/lib.rs`:
  `// CHARTER: Facet (Images) — shared-core (adjust_core, LEDGER SR232)`.
- No dependency at all (no raster_core, no pixel_core): the pixel format is restated, not imported — straight
  (non-premultiplied) RGBA8, row-major, `w * h * 4` bytes, raster_core's `Layer.pixels` layout (RASTERCORE.md).
- No preference, no dotfile, nothing in the kernel. Nothing in `src/` is std-gated.

## FILES

- `unaos/libs/media/adjust_core/Cargo.toml` — `default = ["std"]`, `std = []`, `LGPL-3.0-or-later`, edition 2024,
  no `[dependencies]`.
- `src/lib.rs` — charter, `Error` (+ `word`), the one length check `check`, the module map and re-exports.
- `src/levels.rs` — `Levels`, `levels`, `levels_lut`, the fixed-point `pow_q16` (`log2_q16`, `exp2_q16`).
- `src/curves.rs` — `Curves`, `Curves::from_points`, `Curves::identity`, `curves`.
- `src/color.rs` — `HueSat`, `hue_sat`, `invert`, `threshold`, `luma`.
- `src/blur.rs` — `box_blur`, `gaussian_blur`, `gaussian_kernel`, `GAUSS_SHIFT`.
- `src/sharpen.rs` — `Unsharp`, `unsharp`.
- `tests/kats.rs` — the host KATs.
- Root `Cargo.toml` — one member line beside raster_core.
- `docs/dev/evidence/raster/ADJUSTCORE.md` (this file), `docs/dev/LEDGER.md` (SR232), `docs/dev/exec-branches.txt`.

## DESIGN

### The one shape
Every function is `fn(px: &[u8], w: u32, h: u32, params) -> Result<Vec<u8>, Error>`. Chosen over `&mut [u8]` because
(a) the blurs need a second buffer anyway (a separable pass cannot run in place), so one shape for all eight keeps
the API uniform; (b) a pure function leaves the caller's layer untouched, which is what raster_core's
`apply(&self) -> Result<Document, Error>` wants (the old pixels stay in the history). The length check is ONE fn,
`check(px, w, h)`: `w * h * 4` computed with `checked_mul` in `usize`; an overflow or a mismatch is `BadLength`.
`Error { BadLength, BadParam }`, no `Display`, `word()` -> `"bad-length"` / `"bad-param"`.

Alpha: every function touches R, G, B only and copies A, EXCEPT `box_blur` and `gaussian_blur`, which blur all four
channels (a blur moves coverage too; a hard alpha edge stays hard otherwise). The blurs are straight-alpha blurs:
the colour of a transparent pixel bleeds into its neighbours. A premultiplied blur is owed (ADJUSTCORE-PREMUL).
`unsharp` blurs through `gaussian_blur` but takes only its RGB; the output alpha is the input's.

No float: all arithmetic is integer with named scales — `Q16` (1.0 = 65536), `gamma_q8` / `sigma_q8` (1.0 = 256),
the hue circle `HUE_CIRCLE = 1536` (six sectors x 256).

### levels
A 256-entry LUT built once per call (`levels_lut`), applied to R, G, B. Params `Levels { in_lo, in_hi, gamma_q8,
out_lo, out_hi }`; `in_lo >= in_hi` or `gamma_q8 == 0` is `BadParam`. Per input `v`:
- `t = clamp(round((v - in_lo) * 65536 / (in_hi - in_lo)), 0, 65536)` (Q16, clamps outside the input range);
- `p = t ^ (gamma_q8 / 256)` — the exponent IS gamma: `gamma < 1.0` lifts the midtones (the brief's KAT: q8 = 128
  maps 128 above 128). `gamma_q8 == 256` skips the power (exact identity); `t == 0` gives 0;
- `out = out_lo + round((out_hi - out_lo) * p / 65536)` (signed, so `out_lo > out_hi` inverts; allowed).
`pow_q16(t, g_q8) = exp2_q16(g * log2_q16(t) / 256)`: `log2_q16` normalises `t` to a Q30 mantissa in [1, 2) and
takes 16 fractional bits by repeated squaring; `exp2_q16` (argument <= 0) multiplies the Q30 constants
`2^(2^-k)`, k = 1..16 (the table `EXP2_FRAC`, computed as `round(2^(2^-k) * 2^30)`), for each set fractional bit,
then shifts by the integer part.

### curves
`Curves { master: [u8; 256], r: Option<[u8; 256]>, g, b }` (None = identity). Per channel `c`:
`out = master[chan[c]]` — the channel's curve first, then the master. `Curves::from_points(&[(u8, u8)]) -> [u8;256]`:
the points are sorted by x (a repeated x keeps the LAST given), `(0, 0)` is added if no point has x = 0 and
`(255, 255)` if none has x = 255; between neighbours `(x0, y0) - (x1, y1)` the table is linear,
`y = y0 + round_half_away((y1 - y0) * (x - x0) / (x1 - x0))`. Piecewise-linear interpolation is monotone between
the control points: the table never overshoots its neighbours' y (no spline ringing); a rising curve stays rising.

### hue_sat
`HueSat { hue_deg: i16 /* -180..=180 */, sat_pct: i16 /* -100..=100 */, light_pct: i16 /* -100..=100 */ }`, out of
range is `BadParam`. Three steps, each skipped when its parameter is 0 (so all-zero is the exact identity):
1. Hue, in HSL. Hue rotation keeps L and S, so it keeps `max`, `min` and `d = max - min` of the pixel: only the
   hue is computed (`0..1536`, `256` per sector: max = R -> `round(256 (G - B) / d)` mod 1536, max = G ->
   `512 + round(256 (B - R) / d)`, max = B -> `1024 + round(256 (R - G) / d)`), shifted by
   `round(hue_deg * 1536 / 360)` mod 1536, and the pixel rebuilt from sector `s = h / 256` and `f = h % 256`:
   `up = min + round(d f / 256)`, `down = max - round(d f / 256)`; sectors 0..5 = (max,up,min) (down,max,min)
   (min,max,up) (min,down,max) (up,min,max) (max,min,down). A grey (`d == 0`) is unchanged.
2. Saturation, about the pixel's luma (luma-preserving, not HSL's L): `Y = luma(R, G, B)` (below),
   `k = 100 + sat_pct` (0..200, scale 100), `c' = clamp(Y + round((c - Y) * k / 100))`. `sat_pct = -100` gives
   `R = G = B = Y`; `+100` doubles the distance from the grey axis (clamped).
3. Lightness, toward white or black: `light_pct > 0`: `c' = c + round((255 - c) * light_pct / 100)`;
   `light_pct < 0`: `c' = round(c * (100 + light_pct) / 100)`.

### invert, threshold
`invert`: `c' = 255 - c` on R, G, B. `threshold(level: u8)`: `luma = (77 R + 150 G + 29 B) >> 8`
(`77 + 150 + 29 = 256`, so a grey `v` has luma `v`); `luma >= level` -> R = G = B = 255, else 0.

### box_blur
`box_blur(radius: u16)`: separable, horizontal pass then vertical pass, each a running sum over the window
`2r + 1` with edge clamp (`px[clamp(i, 0, n - 1)]`): the sum starts as the clamped window at index 0 and slides
by adding `clamp(i + r + 1)` and removing `clamp(i - r)`. Each pass rounds, `out = (sum + n / 2) / n` with
`n = 2r + 1`, into a u8 intermediate. `u32` sums (`(2 * 65535 + 1) * 255 < 2^32`). All four channels, alpha too.
`radius == 0` is a copy.

### gaussian_blur
`gaussian_blur(sigma_q8: u16)`: `radius = ceil(3 sigma) = (3 * sigma_q8 + 255) / 256`; `sigma_q8 == 0` is a copy.
The kernel (`gaussian_kernel(sigma_q8) -> Vec<u32>`, length `2 radius + 1`) is SAMPLED: `w(i) = exp(-i^2 / (2
sigma^2))` as `exp2_q16(-(i^2 * 2^32 / (2 sigma_q8^2)) * LOG2_E_Q16 / 2^16)` (`LOG2_E_Q16 = 94548 =
round(log2(e) * 65536)`), then normalised to the power-of-two sum `1 << GAUSS_SHIFT` (`GAUSS_SHIFT = 16`):
`w'(i) = round(w(i) * 65536 / sum w)`, and the residual (65536 minus the rounded sum) goes on the centre tap, so
the row sums to exactly 65536 (a constant image stays the constant: `c * 65536 >> 16 = c`). Separable, horizontal
then vertical, edge clamp, `out = (acc + 32768) >> 16` per pass into a u8 intermediate. All four channels.

### unsharp
`Unsharp { amount_pct: u16, sigma_q8: u16, threshold: u8 }`: `b = gaussian_blur(px, sigma_q8)`; per R, G, B:
`diff = o - b`; if `|diff| >= threshold`: `o' = clamp(o + round(diff * amount_pct / 100))`, else `o' = o`. Alpha is
the original's. `amount_pct == 0` is the identity (`diff * 0 = 0`).

## BLUEPRINT ROWS

Read whole before the first edit: `docs/dev/blueprint/handlers.md` (119 lines) and `docs/dev/blueprint/shape.md`.
- Kept: BP-handlers-1 (manifest unchanged), BP-handlers-2 (purpose from CODEX §2 Facet), BP-handlers-3 (no kernel
  code; the lib's first line names owner and seam), BP-handlers-4 (one no_std core both faces will link — no face
  yet), BP-handlers-5 (no preference), BP-shape-2 (the core lives in `unaos/libs/media/`), BP-shape-4 (no board
  names), BP-shape-10 (no dependency at all), BP-shape-11 (licence follows the sibling cores, LGPL-3.0-or-later).
- `changed=none`. handlers.md is not touched (119 lines, at its cap). Proposed row for when a face links it: the
  BP-handlers-4 list gains `Facet raster_core / adjust_core (RASTERCORE SR230, ADJUSTCORE SR232)`.

## LEDGER ROW

`| SR232 | **ADJUSTCORE — Facet had no adjustments …` — the full row is in `docs/dev/LEDGER.md` (status
`fixed-unflown`).

## KATs (`unaos/libs/media/adjust_core/tests/kats.rs`)

1. `levels_kat` — identity `(0, 255, 256, 0, 255)` leaves a 4x4 gradient unchanged; `in_lo = 64, in_hi = 192`
   maps 64 -> 0, 192 -> 255, 10 -> 0, 250 -> 255; `gamma_q8 = 128` maps 128 above 128 (value quoted in MEASURED).
2. `curves_kat` — `from_points(&[])` and `from_points(&[(0,0),(255,255)])` are the identity table; `(128, 200)`
   maps 128 -> 200 and 64 -> 100.
3. `color_kat` — hue +120 on pure red is pure green within 2 per channel; sat -100 on a coloured pixel gives
   R = G = B within 2 of the input's luma; invert twice is the identity.
4. `threshold_kat` — a 50% grey (128) at 128 gives white, at 129 black.
5. `box_blur_kat` — radius 1 on a 5x5 black with one 255 centre gives 28 at the centre and its 8 neighbours, 0
   elsewhere; radius 0 is the identity.
6. `gaussian_kat` — a constant image stays the constant; on an impulse the centre is the maximum; every kernel row
   sums to `1 << GAUSS_SHIFT` (65536).
7. `unsharp_kat` — amount 0 is the identity; amount 100 on a 50 | 200 step edge makes the bright side brighter and
   the dark side darker next to the edge.
8. `refusals_and_alpha_kat` — every function returns `BadLength` on a short buffer; levels / curves / hue_sat /
   invert / threshold / unsharp keep alpha byte for byte; the blurs blur it (a constant alpha stays constant,
   an alpha impulse spreads); out-of-range levels / hue_sat params are `BadParam`.

## MEASURED

- Command (repo root, the seat's one compile, R107/R113):
  `CARGO_TARGET_DIR=~/unaos-bench/scratch/rmbp-0915/adjustcore/target cargo test -p adjust_core -- --nocapture` —
  exit 0, first compile, no warnings: unit 0, `tests/kats.rs` **8 passed** (the 8 KATs above), doc-tests 0.
- Values printed by that run:
  - levels `gamma_q8 = 128`: 128 -> **181** (`sqrt(128/255) * 255 = 180.67`; the KAT pins 181).
  - hue +120 on `[255, 0, 0]` -> `[0, 255, 0]` exactly; sat -100 on `[200, 80, 30]` (luma 110) -> `[110, 110, 110]`.
  - gaussian kernel rows, every one summing to **65536 = 2^16**: sigma_q8 64 -> `[22, 65492, 22]`; 128 ->
    `[17, 6976, 51550, 6976, 17]`; 256 (sigma 1.0) -> `[291, 3539, 15862, 26152, 15862, 3539, 291]` (centre
    `26152 / 65536 = 0.3990`, against `1 / sqrt(2 pi) = 0.3989`); 384 -> 11 taps; 1000 -> 25 taps.
  - gaussian sigma 1.0 on a 255 impulse (9x9): centre 41, right neighbour 25, diagonal 15.
  - unsharp 100 %, sigma 1.0 on a 50 | 200 step: the dark side next to the edge 50 -> 5, the bright side 200 -> 245;
    the far ends stay 50 and 200.
- Anchors: `check` `src/lib.rs:57`, `map_rgb` `:66`; `log2_q16` `src/levels.rs:33`, `exp2_q16` `:50`, `pow_q16`
  `:65`, `levels_lut` `:77`, `levels` `:93`; `Curves::from_points` `src/curves.rs:36`, `curves` `:67`; `luma`
  `src/color.rs:21`, `rotate` `:26`, `hue_sat` `:58`, `invert` `:88`, `threshold` `:98`; `pass` `src/blur.rs:18`,
  `box_line` `:38`, `box_blur` `:52`, `gaussian_kernel` `:65`, `kernel_line` `:91`, `gaussian_blur` `:106`;
  `unsharp` `src/sharpen.rs:18`.
- Not committed: the root `Cargo.lock` gains the `adjust_core` package entry when the workspace member is added;
  Cargo.lock is not in this arc's file list, so the commit leaves it to the seat's fold (a `cargo` run regenerates it).
- Owed: ADJUSTCORE-PREMUL (a premultiplied blur so transparent colour does not bleed); ADJUSTCORE-CMD (raster_core
  commands that apply an adjustment to a layer / through a selection); ADJUSTCORE-FAST (box-blur-cascade gaussian
  for large sigma: the sampled kernel is O(radius) per sample).
- Files: `src/blur.rs` 116, `src/color.rs` 106, `src/curves.rs` 77, `src/levels.rs` 97, `src/lib.rs` 85,
  `src/sharpen.rs` 37, `tests/kats.rs` 197 lines (`wc -l`).
