# SVGCORE — UnaOS renders SVG from the specifications (LEDGER SR52)

Branch `exec-media-svgcore`, cut at `d8788fe1`. `exec-rmbp-merge13` (boot-23 integration) and `exec-web-fontcore`
(FONTCORE @969dd87c) were merged first. The only conflict was `Cargo.lock`, which was regenerated.

Crate: `unaos/libs/media/svg_core` (root-workspace member). It is `#![no_std]` + `alloc` and
`#![forbid(unsafe_code)]`, about 7,100 lines. Its **one dependency is UnaOS's own `font_core`**, and it uses
**no third-party crate at all**, dev-dependencies included. Its only dev-dependency is UnaOS's `pixel_core`.

## Finding

Before this arc, Aether and Facet drew SVG with `resvg` (DEPS SR31: chicken wire), and the kernel viewer had
no vector path. svg_core is the standalone core that replaces `resvg`:

* It parses SVG 1.1/2 documents.
* It renders them onto a premultiplied RGBA8 canvas.
* It fills paths with **font_core's exact-area scanline rasterizer**. This is one rasterizer with two users:
  svg_core drives `font_core::raster::Rasterizer::line` and resolves the signed area itself, adding the
  even-odd fold. No `raster_core` was factored out, because none was needed. font_core is unchanged, and its
  tests stay green.

pixel_core gains an `svg` format behind a feature, so the one `pixel_core::decode` call returns SVG as an
`Image` like every other format.

This arc does not touch `handlers/aether`. Two follow-ons belong to the seat at the fold:

* Retiring Aether's `resvg` feature means turning on `pixel_core/svg` (or `svg-std`) along the
  `gneiss_pal::dsp::image` path.
* The EYES `07-images-svg` re-score has to wait for that swap. Both SVGs on that page were spot-checked
  against Chromium:
  * the data-URL `<img>`: 99.10 % within 8 levels (mean 0.14)
  * the inline `<svg>` polygon: 100 % within 8 levels

svg_core is a library, not a handler. It has no bus surface; its consumers own the verbs.

## Spec coverage

| Area | Spec | Module |
|---|---|---|
| XML 1.0 + Namespaces: elements, attributes with value normalization (§3.3.3), the 5 predefined + numeric references, DTD internal-subset `<!ENTITY>` whose replacement text may contain markup (re-parsed, §4.4), CDATA, comments, PIs; bounded entity expansion (billion laughs refused) | W3C XML 1.0 5th ed., Namespaces in XML 1.0 | `xml.rs` |
| Minimal CSS: declaration blocks (`!important`), rule sets, Selectors-3 subset (type, `*`, `#id`, `.class`, all attribute operators, `:first-child`, descendant/child), specificity + order, at-rules skipped. **The seam for CSSCORE (SR47)**: `css::parse_declarations` and `css::Stylesheet` are the only CSS entry points | CSS 2.1 syntax, Selectors 3, Cascade 4 | `css.rs` |
| The cascade (presentation attributes < sheet < `style`, `!important` above both), inheritance, `inherit`, `font`/`marker` shorthands, CSS `transform` property, `transform-origin` | SVG 2 §6 | `style.rs` |
| Colours: 148 keywords, `#rgb[a]`, `#rrggbb[aa]`, `rgb[a]()` (legacy and space syntax), `hsl[a]()`, `currentColor`, `transparent` | CSS Color 3/4 | `color.rs` |
| Lengths: px, in, cm, mm, Q, pt, pc, em, ex, ch, ic, lh, rem, rlh, vw/vh/vi/vb/vmin/vmax, %. Font-relative units use the font's real metrics | CSS Values 4 | `style.rs`, `render.rs` |
| Path data: every command, implicit lineto, compact flags/numbers, render-up-to-error. Arcs go to **center parameterization (F.6.5)** with radius correction (F.6.6), then to cubics ≤ 90° | SVG 1.1 §8.3, App. F.6; SVG 2 §9.5.4 | `geom.rs` |
| Transforms (`matrix translate scale rotate(a cx cy) skewX skewY`), viewBox, preserveAspectRatio (align, meet/slice) | SVG 1.1 §7.6–7.8 | `geom.rs` |
| Basic shapes `rect` (rx/ry incl. auto + clamping), `circle`, `ellipse`, `line`, `polyline`, `polygon` | SVG 2 §10 | `render.rs` |
| Fill (nonzero / even-odd), stroke by outline offsetting: joins miter (+ limit), miter-clip, round, bevel; caps butt, round, square; zero-length subpaths get caps; dasharray/offset; opacity, fill-/stroke-opacity, `paint-order`, `visibility`, `display`, `shape-rendering` | SVG 2 §13 | `raster.rs`, `stroke.rs` |
| Paint servers: linear, radial as the **two-point conical** gradient with `fr`, units, `gradientTransform`, spread pad/reflect/repeat, `href` inheritance, stops (offset clamping, stop-color/-opacity); patterns (units, content units, viewBox, `patternTransform`, `href`) rendered to a device-resolution tile | SVG 2 §14 | `paint.rs`, `render.rs` |
| Group effects by offscreen layer: `opacity`, `clip-path` (clipPathUnits, child `clip-rule`, `use` children, nested clip-path on clipPath and on children), `mask` (units, content units, region, luminance or `mask-type: alpha`, `color-interpolation: linearRGB`), CSS basic-shape clip paths `circle() ellipse() inset() polygon()` with reference box | CSS Masking 1, CSS Shapes 1 §3 | `render.rs` |
| `use` (shadow-tree style parent, cycles refused), `symbol` and nested `svg` viewports with `overflow` clipping, `switch` + conditional processing (`systemLanguage`, `requiredExtensions`), `defs` | SVG 2 §5, §8.2 | `render.rs` |
| Markers: start, mid, end; orient angle (deg/grad/rad/turn), `auto`, `auto-start-reverse`; markerUnits, refX/Y, viewBox, overflow; `context-fill`/`context-stroke`, including paint servers resolved against the context element | SVG 2 §11.6, §13.3 | `marker.rs` |
| `<text>`/`<tspan>`: `xml:space` white space; per-character `x y dx dy rotate` lists (innermost first, rotate repeats); text chunks + `text-anchor`; letter/word spacing; `baseline-shift` (accumulating) and `dominant-baseline`; `text-decoration` (underline, overline, line-through); font selection by family list, generic families, CSS Fonts 4 §5.2 weight/style matching, per-character fallback; shaping (GSUB ligatures, GPOS/`kern` kerning) and glyph outlines through **font_core** | SVG 1.1 §10, SVG 2 §11, CSS Fonts 4 | `text.rs`, `fonts.rs` |
| `<image>`: `data:` URLs (RFC 2397, base64 RFC 4648 / percent-encoding), raster via a caller hook (pixel_core supplies it), nested SVG images rendered by svg_core, auto width/height from the aspect ratio, preserveAspectRatio, `image-rendering` | SVG 2 §12.3 | `image.rs`, `render.rs` |

## The faces (M4)

* **pixel_core `svg` feature** (`unaos/libs/media/pixel_core/src/svg.rs`):
  * `sniff` returns `Format::Svg` for markup that begins (after BOM, whitespace, comments or a DOCTYPE) with
    `<svg`, or with `<?xml` followed by `<svg`.
  * `decode` renders at the intrinsic size: width/height, else derived from the viewBox, else 100×100.
  * `svg::decode_at(bytes, Some((w, h)), fonts)` renders at a requested size, mapping the viewBox with the
    root's preserveAspectRatio.
  * `svg::intrinsic_size` returns the intrinsic size.
  * Raster `<image>` content decodes through pixel_core itself.
  * The canvas allocation is probed with `try_reserve` first, so a kernel heap answers `OutOfMemory`.
  * `svg-std` adds the host's `/usr/share/fonts`, loaded once, to the plain `decode` path. Without it, text
    is skipped unless fonts are passed in.
* **The kernel does not enable it. Measured on the x86 build** (`unaos/crates/kernel`, x86_64-unaos.json,
  build-std, release):
  * default features: `.text` 1,750,539 → 1,750,859 bytes (+320 B; the viewer is not compiled)
  * `--features wc,facet`, the kernel that has the facet viewer: `.text` 2,688,275 → 3,073,145 bytes
    (**+385 KB, +14 %**) and `.data` +12.8 KB
  * Both builds link cleanly.
  * Turning it on is a one-line change, `pixel_core = { …, features = ["svg"] }`, in
    `unaos/crates/kernel/Cargo.toml`. That is the seat's call.
* svg_core cannot depend on pixel_core, because that would be a cycle. `<image>` decoding is therefore a hook
  (`Options::image_decoder`).

## Oracle (M5)

**Corpus.** These categories of the resvg test suite (linebender/resvg @ `1fe3cfe8`,
`crates/resvg/tests/tests`) were used: shapes, path, painting (no filters), paint-servers, text (14 property
families), masking, structure.

* That is 1,118 SVG files plus the suite's 21 fonts.
* Each file is rendered at the size of resvg's own reference PNG: 300×300 for most files, some 300×150 or
  500×500.
* Nothing is committed. `tests/vectors.txt` lists each file with its category, path, sha256 and size.
  `fetch-vectors.sh` fetches them (URL-encoded, sha-checked) into `target/svg-vectors`.

**Reference: headless Chromium 1194** (`/opt/pw-browsers`).

* `oracle/chromium-svg.cjs` renders each file as an `<img>` of exactly W×H CSS px, as a data URL, on a white
  page, `--force-color-profile=srgb --disable-gpu --font-render-hinting=none`, and takes a screenshot.
* Chromium sees **only the suite's fonts**, through a private fontconfig (`oracle/fonts.conf.in`).
  Generic families map to Noto Serif/Sans/Mono, and `tests/common` hands svg_core the same mapping, so both
  renderers draw text from the same files.
* `tests/oracle.rs` (opt-in) renders the same file at the same size, composites it over white, and scores
  every pixel:
  * mean |error| per channel, in levels out of 255
  * the share of pixels whose largest channel error is ≤ 8 levels
* `oracle/run-oracle.sh <vectors> <out>` reruns the whole thing. It takes 60 s end to end.

### Numbers (honest)

| category | files | mean \|err\| | pixels within 8 levels | files ≥ 99 % within 8 |
|---|---|---|---|---|
| shapes | 76 | 0.076 | 99.74 % | 74 |
| paths | 57 | 0.073 | 99.66 % | 56 |
| painting (strokes, dashes, markers, paint-order, opacity…) | 275 | 0.279 | 99.63 % | 258 |
| gradients | 122 | 0.230 | 99.76 % | 120 |
| patterns | 32 | 0.199 | 99.31 % | 29 |
| text | 224 | 1.412 | 98.72 % | 159 |
| clip / mask | 93 | 0.156 | 99.59 % | 91 |
| use / symbol / defs | 66 | 0.041 | 99.97 % | 66 |
| structure (svg, g, style, switch, image, transform…) | 173 | 1.181 | 98.36 % | 151 |
| **all** | **1118** | **0.590** | **99.28 %** | **1004** |

12 files fall below 90 %. Each has a named cause:

* Chromium shows its **broken-image icon** for an external URL or a data URL without a media type. svg_core
  draws nothing, by design, because an SVG image has no network or file access. This covers
  `url-to-png`, `url-to-svg`, `embedded-svgz` and `embedded-svg-without-mime`.
* **Filters** are out of scope: `filter-bbox` ×2, `tspan/with-filter`.
* Chromium **inherits no attributes across gradient types**, and its behaviour diverges in the two
  linear↔radial cross-reference files.
* `mask/recursive-on-child`, `context/with-pattern-objectBoundingBox-in-use`, `image/objectBoundingBox-clip-on-sliced-image`.

Text below 99 % is mostly **Arabic** (no joining forms, no bidi; see the ceiling) and glyph-edge
rasterization.

**Gate pins** (`tests/digests.txt`, 77 KB): each file's CRC-32 of svg_core's RGBA output next to its frozen
Chromium score. `cargo test -p svg_core` therefore checks that today's renders are exactly the ones Chromium
scored, on a host without Chromium. When the vectors are absent it prints SKIP.

### What the oracle taught us (Chromium's SVG-as-image semantics, each measured, then matched)

1. **Invalid references.**
   * A missing or recursive `clip-path`/`mask` is ignored: the element renders unclipped/unmasked.
   * A `clipPath` with `display:none` is ignored.
   * A paint server inside a `display:none` subtree, or under a failing `systemLanguage`, is invalid.
     Conditional attributes on the resource itself are ignored.
   * `mask` on a `<mask>` is not applied. `display:none` on the root is ignored.
   * A `<switch>` picks its first passing child even when that child is `display:none`.
   * `rgb()` with mixed number/percentage arguments is invalid.
   * An invalid `paint-order` value is dropped.
2. **Root effects.** `clip-path`/`mask` on the outermost `<svg>` resolve in its CSS box (canvas pixels), not
   in the viewBox. CSS `background-color` on the root paints the canvas.
3. **Sizing.** An SVG with no size, or with zero/negative width/height, renders unscaled with the canvas as
   its viewport. An absolutely sized SVG without a viewBox scales onto the canvas.
4. **Transforms.** `transform` on a nested `<svg>` applies (SVG 2), and so do `transform-origin` and the CSS
   `transform` property. `transform` on `<symbol>` is ignored.
5. **Gradients.**
   * Negative `fr` is treated as 0.
   * `r ≤ 0` paints the last stop.
   * Concentric equal circles paint the inside with `t = 0`.
   * Attributes are inherited through `href` only from gradients of the same kind.
6. **Text.**
   * Shaping runs **cross `dx`/`dy`** (kerning kept) but **break at absolute `x`/`y`**.
   * `sub`/`super` shift by half of (ascent − descent).
   * Decorations are `font-size/10` thick. The underline is centred on the font's underline position, the
     overline sits on the ascent line, and line-through is centred at ⅔ of the x-height.
   * Glyph masks follow Skia analytic AA: svg_core rasterizes glyph fills with font_core's
     `RenderMode::SkiaAaa` (quarter-pixel edge-y snap). That mode took text from 136 to 152 files ≥ 99 %.
     Font_core's mask-gamma pre-blend and SkiaAaa for all paths both **lowered** the score, so both stay off.
7. **Images.**
   * Raster images clamp to their edge pixels.
   * Bilinear sampling beats both Mitchell and Catmull-Rom cubics.
   * For an SVG image, the `<image>` element's `preserveAspectRatio` is honoured only when it is `none`.
   * A data URL holding SVG needs its media type; raster formats are sniffed.
8. **Languages.** Chromium's SVG-as-image matches no `systemLanguage`, so every such test fails conditions.
   The oracle harness passes an empty language list. A real caller passes its own.

## KATs (`cargo test --release -p svg_core`: 34 tests, all green; the 1,118-file oracle is opt-in)

* **21 unit tests:**
  * XML: namespaces, entities containing markup, CDATA, attribute normalization, refusals including
    billion laughs
  * CSS: declarations, selector matching, specificity, at-rules
  * cascade order and computed values
  * colours: the keyword table is sorted and complete
  * float math (no_std sin/cos/atan2/sqrt/ln/exp)
  * transforms, the path grammar (implicit commands, error recovery, compact arc flags)
  * arcs: semicircle bbox, radius correction, ellipse area π·a·b
  * viewBox/preserveAspectRatio
  * fill rules and canvas clipping
  * premultiplied blending
  * stroke areas: butt/square/round caps, miter/bevel rings in both windings, dashes with offset, the
    zero-length round-cap disc
  * spread methods, linear and radial (centred and focal) gradients
  * data URLs
* **`render_kat`, 8 hand-derived pixel tests:**
  * half-pixel edges through a viewBox
  * even-odd vs nonzero
  * group opacity applied once
  * dash/gap pixels
  * a gradient value at a pixel centre with reflect
  * clipPath, mask luminance and `use`
  * stylesheet vs `style`/`!important`
  * intrinsic sizes and refusals
  * text ink with DejaVu (skipped without it)
* **`pixel_face`, 3 tests:** sniff + intrinsic decode; requested size; a PNG inside an SVG decoded by
  pixel_core.
* **`digests`, 1 test:** 1,118 pinned renders (vectors fetched; SKIP offline).
* **`oracle`, 1 test:** opt-in, the full Chromium comparison.

Gate: `cargo test --release -p svg_core -p pixel_core -p font_core`, all green (font_core untouched).
`cargo test --release -p pixel_core` alone is also green (svg off).

## Ceiling: what is NOT done

* **Filters** (`filter`, all `fe*` primitives): the element renders unfiltered. The suite's 398 filter files
  were not scored. This is the largest remaining SVG area.
* **Complex scripts in text.** There are no Arabic joining forms (GSUB `init/medi/fina/isol`), no contextual
  GSUB (types 5/6), no mark positioning (GPOS 4–6), and no bidi (UAX #9). These are font_core's ceiling,
  inherited. Also missing: `textPath`, `textLength`/`lengthAdjust`, vertical writing modes, `tref`, and
  font-variant / font-stretch / font-variation matching.
* `mix-blend-mode`, `isolation`, `enable-background`, `clip` (the CSS property), `shape-rendering` beyond the
  crispEdges threshold, and `image-rendering` other than smooth/pixelated.
* External resources (`href` to files or URLs) are never fetched, by design for an SVG used as an image.
  Only `data:` URLs load. Gzip (`.svgz`) is not inflated, although pixel_core's inflater could do it.
* CSS beyond the minimal subset: no sibling combinators, no pseudo-classes other than `:first-child`, no
  `@media`, no `var()`. That work is CSSCORE SR47's.
* Animation (SMIL/CSS). The first frame is the document as written.

## Owed (follow-ons)

1. **Aether face swap**, with `handlers/aether` untouched here: turn on `pixel_core/svg-std` along
   `gneiss_pal::dsp::image`, retire the `resvg` feature (DEPS SR31), re-score EYES `07-images-svg`.
2. **Kernel viewer**: enable `pixel_core/svg` in `unaos/crates/kernel/Cargo.toml` if +385 KB `.text` on the
   facet kernel is acceptable, and hand it fonts (`svg::decode_at(…, fonts)`) for text.
3. **Filters**, as a module of svg_core, scored against the suite's `filters/` category with the same oracle.
4. **Arabic + bidi** in font_core's shaper. svg_core's text layout already iterates clusters, so that work
   would land there without svg_core changes beyond RTL chunk order.
5. **CSSCORE (SR47)** replaces `css.rs` behind the same two calls.

## How to continue

```sh
unaos/libs/media/svg_core/fetch-vectors.sh                       # → target/svg-vectors (1139 files, 12 MB)
cargo test --release -p svg_core -p pixel_core -p font_core      # gate (digests check runs when vectors exist)
unaos/libs/media/svg_core/oracle/run-oracle.sh target/svg-vectors /tmp/svg-oracle   # Chromium scores, 60 s
cargo run --release -p svg_core --example svgcompare -- in.svg chrome.png out.png W H  # Chromium | ours | diff×4
SVGCORE_DIGESTS_WRITE=1 SVGCORE_VECTORS=target/svg-vectors cargo test --release -p svg_core --test digests
```

After an intended rendering change, rerun the oracle, check the per-category numbers, and repin. Use
`oracle/make-vectors.py` to widen the corpus, for example with the `filters/` category.
