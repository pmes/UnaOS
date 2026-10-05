# SVGFILTERS — Filter Effects for svg_core, measured against Chromium (LEDGER SR65)

Branch `exec-media-svgfilters`, cut at `2d4e1b12` (`exec-rmbp-merge14`, which carries svg_core with
FONTBIDI). The work is all in `unaos/libs/media/svg_core`. **No new crate**, and no third-party crate:
svg_core still depends only on UnaOS's `font_core`, and its only dev-dependency is UnaOS's `pixel_core`.
`handlers/aether` is untouched.

## Finding

SVGCORE (SR52) scored 99.28 % against Chromium without filters. Three of its twelve worst files needed
filters, and it skipped the 398 files of the resvg `filters/` category. Aether's pages use CSS
`filter: drop-shadow()/blur()`, but neither svg_core nor Aether had a filter graph.

svg_core now implements Filter Effects Module Level 1 in two modules:

* `src/filter.rs` covers:
  * the `filter` property, both `url()` lists and the CSS filter functions
  * the `<filter>` element: its region, `filterUnits`/`primitiveUnits`, and primitive subregions
  * the graph: `in`/`in2`/`result`, SourceGraphic/SourceAlpha
  * `color-interpolation-filters`
  * the working-space decomposition
  * the CSS library API
* `src/fe.rs` holds the pixel operations of every primitive.

Hooks into existing code:

* `render.rs`: group effects now run filter → clip-path → mask → opacity. This applies to every element,
  `use`/`symbol` targets and the root `<svg>`.
* `text.rs`: a filtered `<tspan>` (SVG 2) renders its glyph runs through its filter.
* `style.rs`: adds `color-interpolation-filters`, `flood-color`, `flood-opacity` and `lighting-color`.

**CSS `filter` for Aether: a library API, not a handler change.** The call is:

```
svg_core::filter::apply_css_filter(&layer, value, &CssFilterContext { scale, font_size, viewport, current_color })
```

It runs the same primitives on any premultiplied layer, in sRGB, with an unbounded region. Aether calls it
later, from its own paint path, on a box layer rendered with margin. No `raster_core` was factored out: the
filter code needs only `Pixmap`, and Aether already reaches svg_core through pixel_core's svg face.

The filter is not a handler and has no bus surface.

## Spec coverage

All references are to Filter Effects Module Level 1.

| Area | Section | Where |
|---|---|---|
| `filter` property: `none`, `url()`, filter-function lists (an invalid entry voids the whole value) | §12, §13 | `filter::parse_filter` |
| `<filter>` region: x/y/width/height, defaulting to −10 %/−10 %/120 %/120 %; `filterUnits` (objectBoundingBox fractions or percentages, userSpaceOnUse lengths); an empty region or a missing bbox renders nothing | §8.1–8.3 | `filter_region` |
| Primitive subregions. The default is the union of the input subregions, or the filter region when an input is a standard input, when there are no inputs, or for `feTile`. x/y/width/height can be given in either `primitiveUnits` | §8.4 | `Run::subregion` |
| Graph: `in`/`in2`/`result`, SourceGraphic, SourceAlpha; an unknown name means the previous result | §8.4 | `Run::input` |
| `color-interpolation-filters`: linearRGB (initial) or sRGB. Each input is converted into the primitive's space with the exact IEC 61966-2-1 curves, on unpremultiplied values; flood, drop-shadow and lighting colours are adapted into the operating space | §17.7 | `fe::convert`, `fe::color_in_space` |
| `feGaussianBlur`: the three-box approximation, d = ⌊s·3√(2π)/4 + 0.5⌋. Odd d gives three centred boxes; even d gives two boxes of d offset by half a pixel either way plus one box of d+1. Sums are integers, rounded once per axis, horizontal then vertical (Skia's GaussPass) | §15.17 | `fe::blur` |
| `feOffset`, including fractional offsets (bilinear, as Skia draws a fractional translation) | §15.20 | `fe::offset` |
| `feFlood` | §15.15 | `fe::flood` |
| `feMerge` | §15.19 | `fe::over_into` |
| `feComposite`: over, in, out, atop, xor, lighter and arithmetic (clamped, premultiplied invariant kept) | §15.13 | `fe::composite` |
| `feBlend`: all 16 modes, separable and non-separable (Compositing 1 §5.3, §5.4) | §15.6 | `fe::blend` |
| `feColorMatrix`: matrix, saturate, hueRotate, luminanceToAlpha; an invalid type means matrix | §15.7 | `fe::color_matrix` |
| `feComponentTransfer`: identity, table, discrete, linear, gamma, as 8-bit tables on unpremultiplied values | §15.11 | `fe::TransferFn` |
| `feMorphology`: erode and dilate | §15.18 | `fe::morphology` |
| `feImage`: an element reference (translated to the subregion, or the viewport mapped onto it), a `data:` raster, or an SVG image | §15.16 | `Run::fe_image` |
| `feTile` | §15.22 | `fe::tile` |
| `feDropShadow` | §15.14 | `filter::drop_shadow` |
| `feTurbulence`: the spec's reference C code transcribed (lattice, gradients, `random`, stitching, both types) | §15.23 | `fe::Turbulence` |
| `feDisplacementMap` | §15.12 | `fe::displacement` |
| `feConvolveMatrix`: order, kernel, divisor, bias, target, edgeMode (duplicate, wrap, none), preserveAlpha | §15.10 | `fe::convolve` |
| `feDiffuseLighting`/`feSpecularLighting` with `feDistantLight`, `fePointLight`, `feSpotLight` (cone and its anti-aliased edge). Normals use the spec's Sobel kernels with the edge and corner factors | §15.8, §15.21, §15.9 | `fe::lighting` |
| CSS functions `blur() brightness() contrast() drop-shadow() grayscale() hue-rotate() invert() opacity() saturate() sepia()`, with their equivalent primitives | §13.1–13.2 | `filter::parse_fn`, `run_function` |

**Working space.** When the user→device transform is a scale plus translation, the filter runs in device
pixels. Otherwise the transform is decomposed into a scale (column norms), which applies while filtering,
and a remainder, which is applied by drawing the result bilinearly. This is Skia's decomposition. The
raster is the canvas plus one canvas of margin on every side, so content outside the canvas still feeds
offsets and blurs.

## Oracle

The setup is SVGCORE's harness, widened:

* `oracle/make-vectors.py` adds the 25 directories of resvg `filters/` (suite @ `1fe3cfe8`) as one oracle
  category each.
* `tests/vectors.txt` lists 1,516 files plus 21 fonts. Nothing is committed; `fetch-vectors.sh` fetches and
  sha-checks them.
* The reference is headless Chromium 1194, rendering each file as an `<img>` at the reference size on white
  (`oracle/chromium-svg.cjs`).
* `tests/oracle.rs` scores each file: mean |error| per channel, and the share of pixels within 8 levels.

### Numbers, `filters/` (398 files): 61.97 % before → **99.60 % within 8 levels** (mean |err| 0.356; 379 files ≥ 99 %)

| category | files | mean \|err\| | within 8 | files ≥ 99 % |
|---|---|---|---|---|
| filter (region, subregions, units, graph, transforms, clip/mask interplay) | 74 | 0.061 | 99.78 % | 73 |
| filter-functions (CSS) | 43 | 0.206 | 99.62 % | 41 |
| enable-background (ignored, like Chromium) | 21 | 0.016 | 99.99 % | 21 |
| feBlend | 10 | 0.007 | 100.00 % | 10 |
| feColorMatrix | 16 | 0.501 | 99.80 % | 14 |
| feComponentTransfer | 22 | 0.039 | 100.00 % | 22 |
| feComposite | 19 | 0.019 | 100.00 % | 19 |
| feConvolveMatrix | 25 | 0.424 | 99.07 % | 24 |
| feDiffuseLighting | 22 | 0.095 | 99.58 % | 21 |
| feDisplacementMap | 1 | 0.007 | 100.00 % | 1 |
| feDistantLight | 4 | 0.007 | 100.00 % | 4 |
| feDropShadow | 8 | 0.080 | 99.66 % | 8 |
| feFlood | 8 | 0.283 | 99.96 % | 8 |
| feGaussianBlur | 13 | 0.428 | 99.98 % | 13 |
| feImage | 27 | 2.647 | 98.21 % | 18 |
| feMerge | 3 | 0.045 | 99.63 % | 2 |
| feMorphology | 14 | 0.114 | 100.00 % | 14 |
| feOffset | 9 | 0.199 | 99.93 % | 9 |
| fePointLight | 4 | 0.210 | 99.91 % | 4 |
| feSpecularLighting | 8 | 0.199 | 99.78 % | 8 |
| feSpotLight | 12 | 0.239 | 99.79 % | 12 |
| feTile | 7 | 1.383 | 98.69 % | 6 |
| feTurbulence | 19 | 0.353 | 98.69 % | 18 |
| flood-color | 7 | 0.051 | 100.00 % | 7 |
| flood-opacity | 2 | 0.314 | 100.00 % | 2 |

Every primitive is at or above 98.2 % within 8 levels. Across the whole corpus (1,516 files, SVGCORE's
1,118 plus `filters/`), the score is **99.42 %** and 1,391 files are ≥ 99 %.

**SVGCORE's three filter files moved:**

| file | before | after |
|---|---|---|
| `text/text/filter-bbox` | 76.5 % | 99.6 % |
| `text/letter-spacing/filter-bbox` | 81.3 % | 99.3 % |
| `text/tspan/with-filter` | 85.9 % | 99.7 % |

**Remaining files below 97 %**, each with its cause:

* `feTurbulence/stitchTiles=stitch` (75 %): undefined behaviour in the suite; Chromium's tile origin differs.
* `feConvolveMatrix/kernelMatrix-with-zero-sum-and-no-divisor` (77 %): a float32 near-zero divisor amplifies
  Skia's rounding.
* `feImage/recursive-links-1/2` (84 %): Chromium breaks the cycle one level deeper.
* `filter/on-the-root-svg` (84 %, undefined behaviour): Chromium clamps the root layer's edges.
* `feDiffuseLighting/complex-transform`, `feTile/complex-transform` (91 %, the latter undefined behaviour):
  skew decomposition.
* `feImage` sampling of a 16 px image upscaled 15× (97 %).

### What the oracle taught us (Chromium's behaviour, measured, then matched)

1. **References.**
   * A missing `url()` is skipped; on its own it renders the element unfiltered.
   * An empty `<filter>` renders nothing.
   * `href` on `<filter>` is not followed, neither for attributes nor for primitives.
   * An unknown `in` (FillPaint, StrokePaint, BackgroundImage, BackgroundAlpha, typos) is the previous result.
   * An element met again inside its own filter (`feImage`) renders unfiltered there.
2. **Regions.**
   * SourceGraphic is not clipped to the filter region; a blur at the region's edge sees outside content.
   * A zero or negative subregion width or height removes the crop entirely: the flood covers the canvas.
3. **Numbers.**
   * `divisor="0"` means the default.
   * Number lists keep the numbers parsed before an error (`tableValues="1px"` → [1]).
   * The specular exponent clamps to 1–128.
   * `limitingConeAngle="0"` means no cone.
   * `diffuseConstant < 0` lights black.
   * Light colour scales are not clamped before the channels are.
4. **Lighting in scaled space.** `surfaceScale` and the lights' z map by the mean axis scale (Skia's
   `mapVectors` average). Without this, slopes are off by the zoom factor.
5. **Turbulence.**
   * The seed is truncated.
   * `baseFrequency` ignores `primitiveUnits`.
   * The lattice is sampled at the pixel centre in user space **plus half a user unit**, at every scale
     (fit 0.04 mean error at 1.5× and at 3×).
6. **CSS functions.**
   * They run in sRGB with an unbounded region.
   * `blur(4)` (unitless) is invalid, but `drop-shadow(10 20)` is valid.
   * `hue-rotate(45)` is invalid.
   * The colour comes only first or last.
   * Commas, percentages, a single length or four lengths are invalid.

## KATs: `cargo test --release -p svg_core` passes 46 tests (12 new for SR65)

* **`fe` unit tests (6).**
  * Box sizes for s = 1, 2, 4, 5. A blurred line keeps its symmetry and the centre weight 19/125 for
    d = 5; the even-d kernel is centred.
  * The sRGB↔linear round trip and premultiplied conversion values.
  * Composite operators and arithmetic; multiply and screen.
  * Transfer tables (truncating like Chromium); saturate(1) and hueRotate(0) are the identity.
  * Turbulence reference values: `random(setup_seed(0)) = 16807`, `setup_seed(−20) = 21`, unit gradients,
    the lattice is a permutation, noise is 0 on lattice points.
  * Morphology and tile.
* **`filter` unit tests (2).**
  * `filter` value parsing, including every rule above.
  * `apply_css_filter`:
    * grayscale of R/G/B → 54/182/18
    * invert + opacity
    * invalid values are ignored
    * drop-shadow at scale 2
* **`render_kat`, 4 new hand-derived renders.**
  * The flood region at its exact pixel edges.
  * Offset plus dilate.
  * **linearRGB vs sRGB**: slope 0.5 on red gives 127 in sRGB and 187 in linearRGB.
  * Blur mass and symmetry with the 11-tap support for s = 2.
  * Missing vs empty filter; `hue-rotate(180deg)` of red → (0, 109, 109).
* **`digests`.** 1,516 pinned renders with their frozen Chromium scores, re-pinned
  (`tests/digests.txt`, 107 KB).

Gate: `cargo test --release -p svg_core -p pixel_core` is green, in 90 s with the vectors present.

## Ceiling: what is NOT done

* `enable-background`, BackgroundImage and BackgroundAlpha are treated as Chromium treats them (ignored /
  unknown result). FillPaint and StrokePaint are not paints.
* `kernelUnitLength` is ignored on lighting and convolution, as in Chromium.
* `edgeMode` on `feGaussianBlur` is not implemented (Chromium doesn't support it either).
* Filter resolution is the device resolution; `filterRes` is gone from the spec.
* The cases listed above as below 97 %: stitch tile origin, near-zero convolution divisor, deep `feImage`
  recursion, root-layer edge clamping, skewed lighting and tiles.
* There is no GPU or SIMD path: primitives run per pixel on the CPU. A 300 px filtered file renders in
  10–100 ms. The degenerate `stdDeviation="1000"` takes 1.3 s, because the box sums run over lines padded by
  the kernel. The whole `filters/` category takes about 10 s including PNG decoding.
* The working raster is the canvas plus a margin equal to the graph's reach (the sum of blur 3σ, offsets,
  radii and scales), capped at one canvas; `feTile` takes the full canvas margin. Content beyond that cannot
  reach a visible pixel.

## Owed

1. **Aether CSS `filter`.** In `handlers/aether` (untouched here, because AETHERJS is in flight), render the
   box into a layer with a margin of 3σ plus the shadow offset, call `svg_core::filter::apply_css_filter`,
   and composite. EYES pages with `filter:` re-score after that.
2. The Aether / kernel SVG face swaps owed by SVGCORE are unchanged.
3. Optional: cropping each primitive to the bounds it can actually affect, rather than the whole working
   raster. This is speed only; the results don't change.

## How to continue

```sh
unaos/libs/media/svg_core/fetch-vectors.sh                        # → target/svg-vectors (1537 files)
cargo test --release -p svg_core -p pixel_core                     # gate (digests check runs when vectors exist)
unaos/libs/media/svg_core/oracle/run-oracle.sh target/svg-vectors /tmp/svg-oracle   # Chromium scores
SVGCORE_FILTER=tests/filters/ …                                    # restrict tests/oracle.rs to the filters
SVGCORE_DIGESTS_WRITE=1 SVGCORE_VECTORS=target/svg-vectors cargo test --release -p svg_core --test digests
```

After an intended rendering change, rerun the oracle, check the per-category table and repin.
`SVGCORE_DIGESTS_WRITE` keeps the old scores, so copy the new oracle scores into `tests/digests.txt` for
changed rows.
