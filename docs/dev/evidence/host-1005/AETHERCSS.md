# AETHERCSS — Aether's CSS layout and paint, from the specifications, proven by EYES

Ledger row SR41. Branch `exec-host-aethercss`, cut at `ddc2112b`, `exec-aether-see` (EYES, `b530ccd0`) merged
first (`5e0b18d5`, clean). Oracle: Chromium via EYES (`tools/eyes/run.sh aether`, 18 cases, SSIM on 2x-downsampled
greyscale + pixel mismatch, baseline gate). No new third-party crate; the kuchiki DOM is untouched (its own arc).

## The finding

EYES named the misses (margin collapsing, list markers, `pre`, border styles/radius, letter-spacing, gradients,
`clamp()`, z-index, word-break, border-collapse, box-shadow, form controls). Looking at the frames showed that the
biggest losses were not those named properties: every box drifted down because line heights were fractional
(18.4px instead of Chromium's 18) and text rode the top of its line box; every padded block overflowed on the right
because taffy's default `box-sizing` is border-box and blocks were `width: 100%` placeholders; `h2` was 24px in a
14px body; a `label { display: block }` stayed inline because Aether builds its box tree from tag defaults before
the cascade; tables had no table layout at all. Each was fixed from its spec section, in the order of SSIM gained.

## Score (Chromium oracle, 800x600 and 375x812)

Mean SSIM **0.797 → 0.971** (target ≥ 0.90), mismatch **8.9% → 1.6%**; worst case **0.545 → 0.934** (target: none
under 0.80). Every ledgered CSS miss in the corpus is gone (0 missing APIs on all 18 cases).
[AETHERCSS-SCORE-before.md](AETHERCSS-SCORE-before.md), [AETHERCSS-SCORE-after.md](AETHERCSS-SCORE-after.md).

| case | before | after |
|---|---:|---:|
| 01-blog-article | 0.653 | 0.935 |
| 02-flex-two-column | 0.754 | 0.988 |
| 03-card-gallery | 0.966 | 0.993 |
| 04-sticky-nav | 0.722 | 0.972 |
| 05-form | 0.703 | 0.936 |
| 06-table-zebra | 0.791 | 0.986 |
| 07-images-svg | 0.829 | 0.937 |
| 08-inline-wrap | 0.745 | 0.979 |
| 09-media-queries | 0.977 | 0.992 |
| 09-media-queries@375 | 0.978 | 0.989 |
| 10-calc-clamp | 0.718 | 0.998 |
| 10-calc-clamp@375 | 0.742 | 0.998 |
| 11-non-latin | 0.811 | 0.934 |
| 12-js-mutation | 0.773 | 0.994 |
| 13-ua-defaults | 0.889 | 0.953 |
| 14-boxes | 0.545 | 0.936 |
| 15-landing | 0.887 | 0.954 |
| 16-positioned | 0.862 | 0.997 |

Frames: `aethercss-<case>-after.png` beside `aethercss-<case>-ref.png` (Chromium) for 14-boxes, 05-form,
06-table-zebra.

## Milestones (one commit each; the SCORE before/after is in every message)

| M | commit | spec | what | moved |
|---|---|---|---|---|
| 1 | `93b7583d` | CSS2 §8.3.1; WHATWG rendering §15.3 (html.css); Cascade §6 | margin collapsing (`layout/collapse.rs`: siblings, parent/first+last child, empty blocks, ±; root/flex items/inline/clipped/abs excluded; applied per layout run, specified margins restored); UA margins in em; per-side `margin-*`/`padding-*` longhands | 0.797→0.856 (14 .545→.824, 10 .718→.903, 12 .773→.940) |
| 2 | `af0b320e` | CSS2 §10.3.3; css-sizing-3 | auto-width blocks fill minus margins/border/padding (`layout/blockwidth.rs`); `box-sizing: content-box` initial value (taffy defaults to border-box); html.css border-box controls | →0.861 |
| 3 | `965cc7a1` | CSS2 §10.8; css-backgrounds-3 §4-5 | `line-height: normal` = rounded ascent+descent+gap, half-leading baseline, real space advance (`fonts/`); per-corner `border-radius` (AA distance field, §5.5 reduction), dashed/dotted/double with Chromium's mark distribution (`render/boxpaint.rs`) | →0.881 (04 .824→.961, 14 →.930) |
| 4 | `b8909f12` | css-text-3 §3-5, §8.2, §4.1.1 | ONE line breaker for measurer and painter (`fonts/lines.rs`): white-space (all six), word-break, overflow-wrap, letter-spacing; inter-element spaces; mark/del/sup/sub UA; line-through | →0.887 (15 →.940, 08 →.782) |
| 5 | `d029258b` | HTML rendering §15.5 + Chromium control theme; CSS2 §9.2.1.1 | form controls at Chromium metrics (intrinsic content sizes via the measure function, 13.333px control font, checkbox/radio/select/textarea/button painting); heading sizes em of the parent with rescaled UA margins; author outer `display` re-derives the parent's formatting context | →0.922 (05 →.936, 07 →.937, 02 →.988) |
| 6 | `d6c91833` | css-values-4 §10 | `clamp()/min()/max()` font sizes; %-mixed math widths resolved after a first layout pass; math inside box shorthands | →0.940 (10@375 .767→.998) |
| 7 | `4254404b` | CSS2 §17.5.2.2, §17.6 | automatic table layout between two passes (`layout/table.rs`): column grid, colspan, shrink-to-fit or author width, proportional distribution; collapse/separate border models | →0.951 (06 .793→.986) |
| 8 | `a0d30424` | CSS2 §10.8.1 | vertical-align super/sub grow the line box | →0.962 (08 →.979) |
| 9 | `24054495` | CSS2 §9.9, Appendix E; §10.1 | z-index painting order per parent; `position: fixed` against the viewport | →0.969 (16 .865→.997) |
| 10 | `9c4bca6c` | css-lists-3 §3; css-counter-styles-3 | outside markers: disc/circle/square at Blink geometry, decimal/alpha/roman/leading-zero, value/start/reversed | 01 .932→.935 |
| 11 | `ac9f1c97` | css-images-3 §3.1; css-backgrounds-3 §7.1 | linear/repeating-linear gradients; outer box-shadow (Gaussian of sigma = blur/2) | →0.971 (03 →.993) |

## Known-answer tests (all in `cargo test -p aether`, 109 passed; was 89)

`test_margin_collapsing_kat`, `test_ua_margins_and_longhands`, `test_block_width_kat`, `test_line_metrics_kat`,
`boxpaint::{used_radii_kat, dash_and_dot_fit_kat, rounded_ring_paint_kat}`,
`lines::{white_space_kat, word_breaking_kat, letter_spacing_kat}`, `test_form_control_metrics_kat`,
`test_display_block_on_inline_tag`, `test_heading_em_sizes`, `test_css_math_kat`, `test_table_layout_kat`,
`test_sup_grows_line_box`, `test_z_index_and_fixed_kat`, `test_marker_text_kat`,
`effects::{gradient_parse_and_stops_kat, box_shadow_parse_kat}` — 20 new.

## The honest ceiling (what is NOT done)

- Inline layout is still Aether's flex approximation: a text run that wraps occupies a rectangle, so inline
  content does not continue on the wrapped run's last line; inline backgrounds paint the run's box, not per-line
  fragments (`pre > code` shows one block where Chromium paints one strip per line).
- Stacking contexts are per parent: positioned descendants are not hoisted to the enclosing stacking context;
  `opacity`/`transform` do not create one.
- `em` in lengths other than font-size/UA margins still resolves against 16px; `line-height: <px>` is a
  multiplier of 16px.
- Tables: no min-content column widths (columns scale proportionally when the table is too narrow), no rowspan,
  no `table-layout: fixed`, no vertical-align in cells.
- Gradients: radial/conic are not painted; gradient stop colours honour alpha, plain colours elsewhere do not
  (rgba backgrounds paint opaque). box-shadow: no inset; overflow clipping is rectangular (rounded overflow:hidden
  lets children paint over the corners).
- Markers: inside position, `list-style-image`, and counter() in `content` are not implemented.
- Form controls: no focus ring, no native disabled look, `<select multiple>`/size, file/color/range inputs paint
  as buttons/fields.
- Fonts are font-kit faces; Chromium's glyph rasterisation and hinting differ (the residual SSIM on text-heavy
  pages, 01/11/13/14 at 0.93-0.95, is mostly glyph-level).

## What stays owed

- A real inline formatting context (line boxes with inline fragments) — the largest remaining structural gap,
  and the step after which inline backgrounds/borders and mid-line wraps match.
- Stacking-context hoisting; opacity compositing; rgba everywhere.
- The kuchiki DOM replacement (its own arc, SR23's chicken-wire table); taffy remains the flex/size engine under
  Aether's layout (named chicken wire in EYES.md; this arc added no crate and leaned on taffy only for flex
  sizing — collapsing, block widths, tables, line breaking, painting are Aether's own).
- More EYES cases for what this arc built but the corpus does not stress: rowspan tables, nested stacking,
  `pre-wrap`, roman lists, radial gradients (each a one-file case).

## How a future executor continues

`tools/eyes/run.sh aether` (rebuilds; ~5 min on a quiet host), read `tools/eyes/out/aether/SCORE.md`, open the
worst case's `.subject.png`/`.ref.png`. Pixel probes without PIL: a 30-line PNG reader in Python (zlib +
filters) prints an ASCII crop — enough to measure a marker's offset or a dash pattern. The layout passes run in
`layout::remeasure` in this order: display contexts, text-align, inline whitespace, font/size resolve (+ UA
margin rescale, sup/sub), block widths, margin collapsing, layout pass 1, (tables + %-math, pass 2), restore.
