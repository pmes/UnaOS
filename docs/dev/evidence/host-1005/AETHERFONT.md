# AETHERFONT — Aether's text on UnaOS's own font_core

Ledger row SR61. Branch `exec-host-aetherfont`, cut at `069f7fd5`. Step one was a join (below). Lane: the
`handlers/aether` text lane, meaning font loading, measuring, line breaking and glyph painting, under
AETHERINLINE's line boxes. The core is `unaos/libs/text/font_core` (FONTCORE SR48 + FONTBIDI SR56), which
gained one module, `name`. No new crate.

## The join (first commit, `67038b30`)

I merged `exec-rmbp-merge13` (`ec47345a`): Aether on html_core's arena DOM (AETHERDOM), css_core
(AETHERSTYLE), AETHERVIDEO, EYES, DEPS and every core. Then I merged `exec-host-aetherinline` @003edc3d
(`4fe0a798`). Both merges were textually clean, because git's virtual merge base resolved every hunk. The
join commit is the semantic fix-up on top of them:

| decision | why |
|---|---|
| `dom::NodeRef::{preceding_siblings, following_siblings}` (snapshots, nearest first like kuchiki) | AETHERINLINE's inter-element-space test and list ordinals call them. |
| three `kuchiki::NodeRef` paths in `render` → `crate::dom::NodeRef` | Those were AETHERINLINE additions that never saw AETHERDOM. |
| **css_core serializer**: a dimension unit starting with `e` was always escaped (`10em` → `10\65 m`) | Every `em`/`ex` value css_core handed to `apply_declaration` failed to parse, so AETHERINLINE's font-relative re-resolution never ran. The escape now applies only when the unit would re-tokenize as an exponent (`e`+digit, `e`±digit), which is what the comment already said and what Chromium does. |
| **cascade share key**: an element with a `style` attribute never shares a folded style | The key is the winning declarations' addresses. A style attribute's declarations live in a Vec that is freed every iteration, so the next element reused those addresses, and two inline-styled siblings folded into one style. This was a latent AETHERSTYLE bug, exposed by AETHERINLINE's KATs. |
| Chromium outerHTML goldens for AETHERINLINE's pages 17–21 (HTMLCORE's `outer_html.cjs`) | `dom_oracle` and html_core's `m3_oracle` cover every EYES page. |

The join matched AETHERINLINE's numbers exactly: **EYES mean 0.988** over 23 cases, every case equal to its
baseline. The style oracle scored **6697/6980**, above the 6627 floor, because AETHERCSS's display, margins and
font-size work now reaches it. The DOM oracle was 21/21 byte-equal, and `cargo test -p aether` was green.

## The finding

Aether measured and drew text with `font-kit` (FreeType, fontconfig and core-text bindings). It had three
"family classes" in place of `font-family`, and a bool in place of `font-weight`. Glyphs were placed one
character at a time, so there was no kerning, no ligatures and no bidi: Hebrew was drawn in logical order.
Line breaks came only at spaces. Decorations used guessed geometry. font_core already shaped every script
glyph-for-glyph like HarfBuzz, and the kernel draws with it.

## What it does now (spec sections)

| | spec | code |
|---|---|---|
| fontconfig read as data (no libfontconfig): `<dir>` with xdg/`~`/relative prefixes, `<include>` of files and `conf.d`, and `<alias>` prefer/accept/default applied in config order exactly as fontconfig's pattern substitution does | fonts-conf(5) | `fonts/fontconfig.rs` (own small XML reader) |
| Every `.ttf/.otf/.ttc/.otc` face under those directories, read with font_core: the **OpenType `name` table** (new in font_core: formats 0/1, UTF-16BE and Mac Roman, families from IDs 1+16, full and PostScript names), plus OS/2 weight, width, italic and oblique | OpenType `name`, `OS/2` | `font_core/src/name.rs`, `fonts/db.rs` |
| Family resolution as Chromium does it on Linux: the installed name; fontconfig's substitute only for `sans`/`serif`/`monospace`; otherwise only Skia's metric-compatible classes (Arial = Liberation Sans = Arimo, Times New Roman = Liberation Serif = Tinos, …). Generic families resolve to Chromium's defaults (standard/serif "Times New Roman", sans-serif "Arial", monospace "Monospace"). The font list is the CSS list, then the standard font, then platform fallback. | css-fonts-4 §4–5; Skia `FontEquivClass`; Blink `FontFallbackList` | `fonts/db.rs`, `fonts/mod.rs` |
| Face selection: stretch, then style, then weight | css-fonts-4 §5.2 | `db::match_style` |
| Synthesis as Blink decides it (bold when the wanted weight is more than 200 above the face's; oblique when italic is wanted and the face is upright) | Blink `FontCache` | `db::synthesis` |
| Per-character platform fallback in fontconfig's `sans-serif` sort, style-matched within the fallback family | `gfx::GetFontForCharacter` | `fonts::fallback_for` |
| `font-family` as an interned list (computed value serialized like `getComputedStyle`); numeric `font-weight` with `bolder`/`lighter`; `font-stretch`; html.css `b, strong { bolder }`; controls in `Arial` | css-fonts-4 §2.1–2.3 | `css/mod.rs`, `layout/mod.rs` |
| `@font-face`: `local()` against full/PostScript names; `url()` from `data:`, `file:`, or `http(s):` (fetched by `net::fetch_page` through http_core); `format()` hints skip what font_core cannot read without fetching it; WOFF 1.0 unpacked with **pixel_core's zlib**; descriptor ranges matched; unicode-range subsets stacked (last rule first) | css-fonts-4 §4.3–4.5; W3C WOFF 1.0 | `fonts/webfont.rs` |
| Shaping: `font_core::shape_fallback` for every width. This gives UAX #9 bidi with the paragraph direction from `direction`/`dir`, per-cluster fallback, GSUB/GPOS, kerning, and ligatures (turned off when `letter-spacing` is set) | UAX #9, OpenType Layout | `fonts/shape.rs` |
| `letter-spacing` after each grapheme cluster, `word-spacing` on word separators | css-text-3 §7.1, §8.2 | `shape::Advancer` |
| UAX #14 opportunities inside space-free words feed the line breaker (hyphens, ideographs); `keep-all` keeps letters together; a line's width is its text shaped as one run | UAX #14; css-text-3 §5 | `fonts/lines.rs` |
| `line-height: normal` takes the union of every face a fragment used (each face's ascent/descent with half its gap) | CSS 2.2 §10.8; Blink `AccumulateUsedFonts` | `layout/inline.rs::text_metrics` |
| `text-align` start/end resolved against `direction` | css-text-3 §6.1 | `layout/inline.rs` |
| Glyphs: font_core's rasterizer in `RenderMode::SkiaAaa`, quarter-pixel x phases, whole-pixel baselines. **Skia's A8 pre-blend**: `SkTMaskGamma_build_correcting_lut` with contrast 0.2 and gamma 1.2, the ink's luminance in 3 bits. Black ink reproduces FONTCORE's fitted table within 1 level. | Skia | `fonts/raster.rs` |
| Synthetic bold = FreeType's `FT_Outline_Embolden` at Skia's strength ppem/24, advances unchanged; synthetic oblique = Blink's −1/4 skew | FreeType, Skia, Blink | `raster::{embolden, skew}` |
| Decorations as Blink paints `auto`: thickness max(1, size/10) painted as whole rows; underline top max(1, ⌈th/2⌉) below the baseline; line-through top round(−ascent/3 − th/2) | css-text-decor-4 §2–3 (Blink) | `fonts::decoration_metrics` |
| `text-shadow`: glyph coverage offset, Gaussian σ = blur/2, painted under the text | css-text-decor-3 §4 | `render::draw_text_shadows` |

**Removed:** `font-kit` and `pathfinder_geometry` are gone from aether. **16 packages left `Cargo.lock`:**
font-kit, pathfinder_geometry, pathfinder_simd, freetype-sys, yeslogic-fontconfig-sys, core-text,
core-graphics, core-graphics-types, dwrote, wio, float-ord, foreign-types ×3, dirs, lazy_static.
`cargo tree -p aether -e normal` shows no font-kit, freetype, fontconfig-sys or pathfinder.

`ab_glyph` was never aether's. It is `libs/quartzite`'s, the vessel GUI toolkit used by aether-shell, lumen and
phonolite, so it stays in the lock until quartzite moves to font_core. That is owed below.

## Oracles and numbers

**EYES** (`tools/eyes/run.sh aether`, Chromium, 24 cases). Over the 23 join cases, mean SSIM went from
**0.9879 to 0.9911**. With the new case the mean over 24 is **0.9912**. Pixel mismatch went from 0.9% to 0.7%.
The text-heavy pages moved: **01 0.9773 → 0.9939, 11-non-latin 0.9727 → 0.9904**, 13 0.9917 → 0.9969, and 14
was unchanged at 0.9773 (its residual is inline-block spacing, which belongs to the IFC). The gate is GREEN and
the baseline was accepted.
[before](AETHERFONT-SCORE-before.md) · [after](AETHERFONT-SCORE-after.md) · frames:
`aetherfont-{11-non-latin,22-fonts}-{after,ref}.png`.

| case | join | after | Δ | mismatch % join | after |
|---|---:|---:|---:|---:|---:|
| 01-blog-article | 0.9773 | 0.9939 | +0.0166 | 1.98 | 1.11 |
| 02-flex-two-column | 0.9915 | 0.9989 | +0.0074 | 0.76 | 0.28 |
| 03-card-gallery | 0.9978 | 0.9988 | +0.0010 | 0.26 | 0.16 |
| 04-sticky-nav | 0.9900 | 0.9931 | +0.0031 | 1.38 | 1.17 |
| 05-form | 0.9544 | 0.9546 | +0.0002 | 1.00 | 1.01 |
| 06-table-zebra | 0.9937 | 0.9941 | +0.0004 | 0.70 | 0.69 |
| 07-images-svg | 0.9364 | 0.9395 | +0.0031 | 3.04 | 2.95 |
| 08-inline-wrap | 0.9904 | 0.9946 | +0.0042 | 1.26 | 1.20 |
| 09-media-queries | 0.9948 | 0.9978 | +0.0030 | 0.53 | 0.44 |
| 09-media-queries@375 | 0.9924 | 0.9977 | +0.0053 | 0.56 | 0.40 |
| 10-calc-clamp | 0.9989 | 0.9987 | −0.0002 | 0.13 | 0.13 |
| 10-calc-clamp@375 | 0.9986 | 0.9986 | 0.0000 | 0.38 | 0.38 |
| 11-non-latin | 0.9727 | 0.9904 | +0.0177 | 1.69 | 0.85 |
| 12-js-mutation | 0.9966 | 0.9965 | −0.0001 | 0.27 | 0.27 |
| 13-ua-defaults | 0.9917 | 0.9969 | +0.0052 | 0.61 | 0.44 |
| 14-boxes | 0.9773 | 0.9773 | 0.0000 | 0.82 | 0.79 |
| 15-landing | 0.9937 | 0.9957 | +0.0020 | 0.95 | 0.79 |
| 16-positioned | 0.9989 | 0.9988 | −0.0001 | 0.16 | 0.17 |
| 17-inline-continue | 0.9901 | 0.9925 | +0.0024 | 1.89 | 1.70 |
| 18-stacking | 0.9988 | 0.9990 | +0.0002 | 0.18 | 0.18 |
| 19-em-units | 0.9950 | 0.9988 | +0.0038 | 0.39 | 0.30 |
| 20-table-spans | 0.9903 | 0.9906 | +0.0003 | 0.73 | 0.74 |
| 21-paint-effects | 0.9996 | 0.9996 | 0.0000 | 0.01 | 0.01 |
| **22-fonts (new)** | — | **0.9919** | — | — | 1.22 |

Every case is unchanged or better at EYES' three decimals. At four decimals, 10, 12 and 16 are 0.0001–0.0002
lower. Those pages are almost all boxes, and the cause is glyph anti-aliasing: Chromium hints installed faces
(see the ceiling), and the font-kit and font_core unhinted masks miss that by different single levels.

`22-fonts` is new and referenced against Chromium. It covers:

- an OpenType/CFF `@font-face` data URI, a WOFF 1.0 bold, and a synthetic oblique;
- an installed `DejaVu Serif`, `Liberation Mono`, `Helvetica → Arial` and an unknown family;
- weights 100–800 and `bolder`;
- letter- and word-spacing, `text-shadow`, underline and line-through;
- `dir=rtl` Hebrew with digits and Latin;
- UAX #14 wraps after a hyphen and between ideographs.

**Text oracle** (`cargo test --release -p aether --test text_oracle -- --nocapture`). There are 50 strings
(`tests/text_oracle/strings.tsv`) at 12, 16, 24 and 48 px, and each string is set in one face or list. The
strings cover Latin kerning and ligatures, accents, Greek, Cyrillic, Hebrew, Arabic, combining marks, CJK and
Thai via fallback, the generics, aliases, an unknown family, and bold, italic and synthetic styles. One
generated page is rendered by `aether render` and by Chromium with the EYES flags.

- **Widths: 400/400 within 0.5 px, worst 0.0234 px.** This compares Aether's `Advancer` (what layout measures
  with) against Chromium's laid-out `Range` width. Chromium's widths are frozen in
  `tests/data/text_oracle_widths.tsv`, so the 200-row width gate runs without Chromium.
- **Rasters, web-font page: 5487/5504 glyphs within 8 levels (99.7 %)**, with 12 px 99.6 %, 16 px 99.6 %,
  24 px 99.7 % and 48 px 99.9 %. The gate is ≥ 90 %. On this page every row's own face file is loaded
  through `@font-face`, which Chromium rasterizes unhinted, so it is a like-for-like test of the raster
  path. Each row is registered by the best integer shift within ±4 px. Each glyph scores the mean |coverage
  difference| over its box, FONTCORE's method.
- **Rasters, system-font page: 25.8 / 23.5 / 49.8 / 65.3 %**, reported and not gated. With system fonts,
  Chromium applies fontconfig's `hintslight` (10-hinting-slight.conf) even under `--font-render-hinting=none`.
  The same face, as a web font and as an installed family, renders visibly differently in one Chromium
  screenshot: crisp horizontal stems for the installed family. FreeType then runs its light hinting, which
  for TrueType faces means the native v40 bytecode interpreter (backward-compatibility mode: y only). Aether
  does not hint. This gap is the system-font residual on every EYES page.

**Decorations.** I measured 6 faces × 12 sizes × (underline, line-through) in Chromium. All **72/72 row
geometries are identical** to Aether's after the calibration.

**Style oracle** (`--test style_oracle`): **6697 → 6817/6980**. font-family went from 229/349 to **349/349**,
because the computed family is now the author's list (or `"Times New Roman"`, or `Arial` on controls), as
Chromium returns it.

**Family resolution and fallback vs fontconfig** (`fonts::db::tests::resolution_and_fallback_vs_fontconfig`).
Arial resolves to Liberation Sans, Times New Roman to Liberation Serif, and Monospace to DejaVu Sans Mono. A
non-equivalent substitute such as "Helvetica Neue" is refused. The family order of the first `fc-match -s
sans-serif` faces is reproduced. The test skips without fontconfig's tools.

## Known-answer tests

`cargo test --release -p aether` is green: **151 lib tests** (133 at the join), plus `dom_oracle`,
`style_oracle`, `style_time` and `text_oracle` (2). `cargo test --release -p font_core` is green. New tests:

- **font_core `tests/name.rs`**: `name_records_match_fonttools` checks **48 faces and 1089 records identical to
  fontTools 4.66.1** (`oracle/gen_name_kat.py`, `tests/data/name_kat.tsv`, each face sha-pinned), plus
  `family_names_order`.
- **`fonts::fontconfig`**: `xml_and_alias_kat`.
- **`fonts::db`**: `match_style_kat` (the §5.2 table for weight, style, stretch and synthesis),
  `style_of_dejavu`, `resolution_and_fallback_vs_fontconfig`.
- **`fonts`**: `family_list_parse_and_serialize_kat`, `stacks_resolve_like_chromium`, `decoration_metrics_kat`.
- **`fonts::shape`**: `kerning_ligatures_and_spacing_kat`, `rtl_runs_come_out_in_visual_order`.
- **`fonts::raster`**: `preblend_matches_fontcore_dark_table`, `quarter_pixel_split`, `embolden_and_skew_kat`
  (FreeType's grow-right-and-up on CW and CCW contours), `synthetic_bold_inks_more`.
- **`fonts::webfont`**: `woff1_roundtrip`, `woff1_zlib_table`, `descriptors_kat`, `font_face_rules_kat`.
- **`fonts::lines`**: `uax14_soft_wraps_kat`.
- **Rewritten**: the css `parse_font_weight` KAT, `test_font_family_classes`, the face-identity test.

## Third-party crates

None added. Aether's text path has no third-party crate. Faces, the `name` table, shaping, bidi, line
breaking, rasterization, WOFF and its zlib are all UnaOS's own code (font_core and pixel_core).

`base64` 0.23 (already a dependency, a utility) decodes `data:` URIs. Chromium, fontTools and `fc-match` are
test oracles only. The `fontconfig-parser` 0.5.8 in `cargo tree -p aether` comes from resvg/usvg's own text
stack behind the `svg` feature (PIXELCORE's chicken wire), not from Aether's text.

## Honest ceiling

- **No hinting.** Chromium hints installed TrueType faces (FreeType v40, from fontconfig's `hintslight`) and
  CFF faces (Adobe's engine). Aether renders them unhinted. This is the 24–65 % system-font raster share and
  the remaining glyph residual on EYES. Web fonts match at 99.7 %.
- **Color glyphs are not drawn.** CBDT, sbix, COLR and SVG glyph tables are unread, so emoji draw nothing, and
  bitmap-only faces such as NotoColorEmoji and Type 1 `.pfb` files are skipped at discovery.
- **fontconfig is partial.** Only `<alias>` and directories are evaluated. `<match>` edits (lang-specific
  prefers, per-font hinting) and `<selectfont>` are not, and faces matching no family of the `sans-serif`
  pattern are ordered by a Latin-coverage test and discovery order, not by fontconfig's full sort.
- **Fallback is per character.** It follows the sans-serif order with the requested weight and slant. It does
  not take the locale or the requested family's generic into account.
- **`@font-face` is partial.**
  - WOFF2 (Brotli + glyf transform) is not decoded. Such a source is skipped and ledgered, and the next source
    is tried.
  - `unicode-range` is parsed, but a face is picked by its cmap, not by its declared range.
  - There is no `font-display`.
  - Variable fonts (`fvar`/`gvar`) render at their default instance.
  - Fonts fetched over http(s) wait for `net::fetch_page`, so there is no late swap.
- **Bidi runs inside one text run.** A line is reordered within each text run, but not across inline elements
  (UAX #9 over the whole line box), and `unicode-bidi` is not read.
- **No device pixel ratio.** Aether paints at DPR 1 (there is no device scale in its surface model), so glyphs
  are sized in CSS px. The rasterizer takes any px size, so DPR is a multiplier where the scale lands.
- **Shadows and decorations are partial.** `text-shadow` blur is an exact Gaussian; Skia uses a triple box
  blur, which differs by a level at large radii. `text-decoration-{color,style,thickness}`,
  `text-underline-offset` and `overline` are not implemented.
- The `font` shorthand parses weight and stretch keywords but does not reset `font-variant`.

## Owed

1. **FONTHINT** (font_core): a TrueType bytecode interpreter in v40 "minimal" mode (y only, backward
   compatibility) and Adobe-style CFF hinting, behind the `hintslight` that fontconfig gives each face. The
   text oracle's system-font page is its gate, and it is the largest remaining glyph residual.
2. WOFF2 (Brotli, RFC 7932, plus the WOFF2 glyf/loca/hmtx transforms) as a no_std core.
3. Color glyph tables (COLRv0/v1 + CPAL, CBDT) for emoji.
4. Line-box-wide bidi reordering across inline elements, and `unicode-bidi`.
5. Quartzite (aether-shell's chrome) onto font_core, so that `ab_glyph`, `owned_ttf_parser` and `ttf-parser`
   can leave the lock (resvg's `ttf-parser` stays until SVG text is svg_core's).
6. AETHERINLINE follow-ups seen here: whitespace between inline-blocks is dropped (14-boxes, and the first
   draft of 22-fonts), and floats lose their author width.

## How a future executor continues

Run these:

- `cargo test --release -p aether`, which includes `fonts::*` and `--test text_oracle`.
  `AETHER_TEXT_ORACLE_OUT=<dir>` keeps the pages and both screenshots, `AETHER_TEXT_ORACLE_ROWS=1` prints every
  row, and `AETHER_TEXT_ORACLE_FREEZE=1` re-freezes the widths.
- `cargo test --release -p font_core --test name`.
- `tools/eyes/run.sh aether`.

The seam is `handlers/aether/src/fonts/`:

- `FontSel` names what a run asks for.
- `fonts::face`/`stack`/`run_faces` resolve it.
- `shape::Advancer` measures, and is the only width source for layout and paint.
- `raster::draw_glyph` paints.

A hinter lands in `raster::rasterize`, which takes the face, glyph, size and phase: hint the outline there, and
everything above it is unchanged.
