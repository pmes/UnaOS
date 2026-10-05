# FONTCORE — UnaOS reads fonts from the specification (LEDGER SR48)

Branch `exec-web-fontcore`, cut at `cc7b39a6`. Crate: `unaos/libs/text/font_core` (root-workspace member).
`#![no_std]` + `alloc`, `#![forbid(unsafe_code)]`, **zero dependencies** (none at all, dev-dependencies included).

## Finding

Before this arc nothing in UnaOS parsed a TrueType/OpenType file: Aether shapes and rasterizes with
`font-kit` + `ab_glyph` (chicken wire, DEPS SR31) and the kernel draws bitmap fonts. FONTCORE is the
standalone core that replaces both. It does not touch `handlers/aether` or the kernel. Those swaps are
follow-ons. It is a library, not a handler: it has no bus surface, and its consumers (Aether, the
kernel desktop) own the domain verbs.

## What it implements (spec sections)

| Area | Spec | Module |
|---|---|---|
| sfnt table directory, TrueType Collections (`ttcf`) | OpenType "Organization of an OpenType Font" | `font.rs` |
| `head`, `hhea`, `maxp`, `hmtx`, `OS/2` (v0–v5 fields used by layout), `post` 1.0/2.0 names (258 Mac names) | OpenType tables of the same names | `font.rs`, `post_names.rs` |
| `cmap` formats 0, 4, 6, 12, 13; subtable preference 3,10/0,4 fmt 12 > 3,1 fmt 4 > Symbol > Mac | OpenType "cmap" | `cmap.rs` |
| `glyf`/`loca` (short/long), simple glyphs (flag repeats, short/same coords), composites (word/byte args, xy offsets **and** point matching, scale / xy-scale / 2×2, SCALED/UNSCALED_COMPONENT_OFFSET), depth ≤ 8 | OpenType "glyf", "loca" | `glyf.rs` |
| CFF 1: INDEX, DICT (ints and reals), Top DICT, Private DICT, charset 0/1/2 + 391 standard strings, CID FDArray/FDSelect 0/3; Type 2 charstrings: every path, hint, hintmask, subr, flex and arithmetic/storage operator | Adobe TN #5176, TN #5177 | `cff.rs`, `cff_strings.rs` |
| Outlines as quadratic/cubic paths (TrueType implied on-curve midpoints) | — | `path.rs` |
| OpenType Layout common tables (ScriptList with DFLT/dflt/latn fallback, FeatureList, LookupList, Coverage 1/2, ClassDef 1/2), GSUB 1 (single), 4 (ligature), 7 (extension); GPOS 2 (pair, formats 1 and 2), 9 (extension); GDEF glyph classes + lookup-flag skipping; legacy `kern` format 0 (MS and Apple headers, 16-bit length overflow handled) | OpenType "OpenType Layout Common Table Formats", GSUB, GPOS, GDEF, kern | `layout.rs` |
| Scanline rasterizer: signed-area accumulation (exact per line piece), nonzero `min(1,|area|)`, 256 levels, 26.6 point snap, adaptive flattening (≤ 1/32 px), subpixel origins, no hinting. `RenderMode::SkiaAaa` adds Skia analytic-AA's quarter-pixel edge-y snap | — | `raster.rs` |
| Glyph cache keyed (font id, glyph, size in 1/64 px, subpixel x, subpixel y), 4 steps per axis, capped | — | `cache.rs` |
| Shaper: script itemization (latn/grek/cyrl), cmap, GSUB type 1/4 lookups of `ccmp locl rlig liga clig calt` in lookup order, GPOS `kern` pairs, with the `kern` table used **only** when GPOS has no `kern` feature for the script (HarfBuzz's rule), measuring, greedy line layout, `draw_text` (source-over compositing) | — | `shape.rs` |
| UAX #14, Unicode 17.0: LB1–LB25, LB28–LB31, i.e. every rule that can fire on Latin/IPA/Greek/Cyrillic/general punctuation/currency; class table generated from LineBreak-17.0.0.txt | UAX #14 | `linebreak/` |

## Oracles and method

* **Tables: fontTools 4.66.1**. `pip install fonttools` worked in this container. `oracle/gen_kat.py`
  reads each container font with fontTools and writes `tests/data/kat_data.rs`. Each font is
  identified by sha256, and a font that is missing or different is skipped by name. The test fails
  if no font at all was checked.
* **Pixels, widths and line breaks: headless Chromium 1194** (`/opt/pw-browsers`).
  `oracle/chromium_oracle.js` renders every job, 188 in total: 7 fonts × 7 strings × 12/16/24/48 px,
  keeping only (string, font) pairs the font fully covers so Chromium never falls back to another font.
  Each job uses a local `@font-face` on the **same file**, `font-kerning: normal`,
  `text-rendering: geometricPrecision`, `-webkit-font-smoothing: antialiased`,
  `--font-render-hinting=none` and grayscale AA (the run checks for colour fringes: 0). The script
  decodes the screenshot PNG in node (zlib) and records `canvas.measureText` (fontKerning normal).
  `tests/oracle.rs` (opt-in: `FONTCORE_ORACLE_DIR`, or run everything with `oracle/run.sh`) draws the
  same string at the same origin. It registers the image by the best integer shift in ±2 px (every
  job registered at 0,0). Then it scores each glyph by the mean |coverage error| over that glyph's
  bitmap box, where Chromium coverage is 255 − gray. `oracle/chromium_lines.js` lays 3 paragraphs
  (Latin, Cyrillic, Greek) into 120/200/333 px boxes at 12/16/24 px in 3 fonts. It reads back where
  each line starts with `Range.getClientRects`.
* **Frozen slices** so the gate (`cargo test --release -p font_core`) checks real Chromium output on a
  host without Chromium: `tests/data/chrome_kats.rs` holds 4 coverage crops and all 188
  measureText widths. `tests/data/chrome_lines.tsv` holds all 81 line layouts. `oracle/capture_kats.py`
  regenerates them.
* **UAX #14: Unicode's own conformance file.** `tests/data/lbtest_subset.txt` holds the 3699
  LineBreakTest-17.0.0 lines whose code points all fall in the supported ranges, cut by
  `oracle/filter_lbtest.py`. URLs and sha256 are in `oracle/vectors.txt`. www.unicode.org is
  refused by the egress proxy, so the files came from the unicodetools repository.

### What the oracle taught us (Chromium's rasterization on Linux)

1. **Subpixel x positioning in quarters, rounded to nearest.** Rounding with floor instead raises
   the 16 px error from 4.5 to 13 levels. Exact (64-step) placement raises it to 7.8. This matches
   FONTCORE's cache quantization.
2. **Skia's A8 pre-blend.** A grid search over the Skia mask-gamma family `1 − (1 − (a + c·a(1−a)))^(1/g)`
   lands exactly on **c = 0.2, g = 1.2**, the documented Chromium Linux `SK_GAMMA_CONTRAST` /
   `SK_GAMMA_EXPONENT`.
3. **Glyph masks come from Skia's analytic-AA scan converter, not FreeType's rasterizer.** Vertical
   edge coverage comes in exact quarters of a pixel, while horizontal coverage is continuous. For
   example, an 'h' top at 16 px reads 51 = ¼ × 204. That is `SkScan_AAAPath`'s `snapY`.
   `RenderMode::SkiaAaa` reproduces it as an option. FONTCORE's default stays exact area, as SR48
   specifies.

## Numbers (honest)

Raster, 188 jobs, 1791 glyphs per size. Per-glyph mean |error| in levels out of 255, and the share of
glyphs whose mean error is ≤ 8 levels:

| size | `exact` (default, raw — **the SR48 gate**) | `exact+pre` (Skia pre-blend applied) | `aaa+pre` (`SkiaAaa` + pre-blend) |
|---|---|---|---|
| 12 px | 5.06 · 90.6 % | 4.49 · 94.7 % | 3.29 · 95.6 % |
| 16 px | 4.54 · **95.7 %** | 4.10 · 97.0 % | 3.22 · 97.2 % |
| 24 px | 3.88 · **98.2 %** | 3.56 · 98.7 % | 2.81 · 98.8 % |
| 48 px | 2.28 · **99.8 %** | 2.15 · 99.9 % | 1.79 · 100.0 % |

Target ≥ 90 % within 8 levels at 16 px and above: **met by the default exact mode** (95.7 / 98.2 / 99.8 %).
The CFF font (Loma) is the closest match: 0.85 levels mean over inked pixels in the frozen 24 px
crop in `SkiaAaa` mode.

* **Shaping widths**: 188/188 strings within 0.5 px of `canvas.measureText`. The worst error is
  **0.0000 px**: kerning (GPOS) and ligatures (`ffi`, `fl`, …) agree exactly.
* **Line layout**: 81/81 paragraphs break exactly where Chromium breaks them (777/777 line starts).
* **UAX #14**: 3699/3699 conformance lines in the supported ranges.

Evidence images in `fontcore/` show Chromium, FONTCORE (exact) and |diff| × 4: `sans_kern_16`
(× 3 zoom), `serif_greek_24`, `loma_cff_fox_48`.

## KAT list (`cargo test --release -p font_core`: 23 tests, all green)

* `kat::tables_match_fonttools`: 6 fonts (DejaVu Sans, Sans Bold, Serif, Sans Mono, Liberation Sans,
  Loma CFF) checked for numGlyphs, upem, hhea, OS/2 typo/win/xHeight/capHeight/weight, post header,
  a–z advances, **1200 cmap lookups** (200 code points × 6, mapped and unmapped), **423 outlines**
  (contour count, exact control box, exact signed area), **60 composites** (component glyphs and
  offsets), post/CFF glyph names, **160 kern pairs** plus pair counts, all 368 CFF charstring widths
  against fontTools' Type 2 interpreter, and every glyph of every font loading.
* `kat::gsub_ligatures_and_gpos_pairs_match_fonttools`: liga/kern lookup selection,
  **22 ligatures**, **202 GPOS pair values** (formats 1 and 2).
* `kat::cmap_subtable_formats_agree`: formats 4, 12 and 6 agree, and format 12 reaches past the BMP.
* `fuzz::{truncated, mutated, garbage}`: 700 truncations (every table boundary plus random cuts),
  600 table-targeted mutations and random garbage per run. All of it goes through parse, cmap,
  outlines, names, rasterization, shaping, layout and drawing with **no panic**. A
  `FONTCORE_FUZZ_ROUNDS=5000` debug campaign (overflow checks on, 20 000 mutations) was also clean.
* `raster::coverage_integrates_to_the_outline_area`: per-pixel signed areas sum to fontTools' glyph
  area for all 423 outlines (worst 0.19 %, the cost of flattening). Nonzero ink never exceeds the
  area. One glyph, Liberation's composite Å, fills less because its contours overlap, as nonzero
  fill must.
* `raster::frozen_chromium_crops`, `raster::subpixel_positions_and_cache`, `raster::every_glyph_rasterizes`,
  and 7 unit tests in `raster`/`fmath`: exact areas on squares and triangles, winding cancel and
  saturate, π r², the quarter snap.
* `shape::{widths_match_canvas_measure_text, line_layout_matches_chromium, kerning_and_ligatures}`,
  `linebreak::{line_break_test_subset, mandatory_and_basic_breaks}`.
* `oracle::chromium_oracle` is the full 188-job comparison. It is opt-in and prints SKIP without
  `FONTCORE_ORACLE_DIR`.

## Ceiling: what is NOT done

* **Hinting**: none, by design. Chromium on Linux is compared with `--font-render-hinting=none`.
* **GSUB** types 2, 3, 5, 6 and 8 (multiple, alternate, contextual, chained contextual, reverse) are
  parsed past but **not applied**. DejaVu's `ccmp` is contextual, so mark-composition sequences
  (base + combining marks) do not get its substitutions. Plain Latin, Greek and Cyrillic text is
  unaffected, as the 0.0000 px width agreement shows.
* **GPOS** types 1 and 3–8 (single, cursive, mark-to-base, mark-to-ligature, mark-to-mark, context)
  are not applied, so combining marks are not positioned. Device tables are ignored (no hinting).
* **CFF**: the `seac` accented-`endchar` form (4-arg endchar) returns an error (needs the Standard
  Encoding table), `random` is unsupported, Expert charsets give no names, and FontMatrix is honoured
  only as a scale. **CFF2** and variable fonts (`fvar`/`gvar`/`HVAR`) are not handled.
* **cmap** formats 2, 8, 10 and 14 (variation selectors) are not handled. Format 13 is read.
* **Scripts**: line breaking covers Latin/IPA/Greek/Cyrillic/punctuation ranges only. The rules for
  CJK/Hangul/Brahmic/emoji/RI (LB20 CB, LB23a, LB26/27, LB28a, LB30a/b, East Asian LB19a/LB30) are
  not implemented. No bidi, no vertical text.
* **Color/bitmap glyphs** (COLR/CPAL, CBDT, sbix, SVG) are not handled.
* **Rasterizer vs Chromium residual**: at 12 px, 90.6 % of glyphs are within 8 levels in exact mode
  (95.6 % in `SkiaAaa` with the pre-blend). The remaining difference comes from Skia's own curve
  subdivision and per-quarter-row alpha rounding.

## Owed (follow-ons)

1. **Aether swap**: replace `font-kit`/`ab_glyph` in `handlers/aether` with `font_core` (`Font`,
   `GlyphCache::with_mode(.., RenderMode::SkiaAaa)` when web-identical output matters, `shape`,
   `layout_lines`). Then retire DEPS SR31's two font rows.
2. **Kernel desktop fonts**: link `font_core` from `unaos/crates/kernel/Cargo.toml` by path (it is
   already `no_std` + `alloc`, and `cargo check -p font_core --target aarch64-unknown-none-softfloat`
   is clean. The x86_64 bare-metal target is not installed on this host, so it was not checked).
3. GSUB 6 / GPOS 4–6 (marks), then CFF `seac`, then CFF2/variations.
4. Extend UAX #14 and the shaper to further scripts with their rules.

## Third-party crates

**None.** FONTCORE has no dependencies or dev-dependencies of any kind. fontTools (Python) and
Chromium are **test oracles only**, run by scripts under `oracle/`. They are never linked or
vendored. The 258 Macintosh glyph names and the 391 CFF standard strings are specification data,
typed out from the spec tables.

## How to continue

* `cargo test --release -p font_core` is the gate. `unaos/libs/text/font_core/oracle/run.sh [dir]`
  regenerates the Chromium rasters and prints the full table above.
* After changing the rasterizer or shaper, rerun `run.sh`. If the frozen data needs refreshing, use
  `python3 oracle/capture_kats.py <dir> > tests/data/chrome_kats.rs` and
  `node oracle/chromium_lines.js > tests/data/chrome_lines.tsv`.
* New fonts for the KATs: add them to `FONTS` in `oracle/gen_kat.py`, then run
  `python3 oracle/gen_kat.py > tests/data/kat_data.rs`.
* New line-break ranges: extend `RANGES` in `oracle/gen_lb.py`, regenerate `src/linebreak/table.rs`,
  then re-cut `tests/data/lbtest_subset.txt` with `oracle/filter_lbtest.py`.
