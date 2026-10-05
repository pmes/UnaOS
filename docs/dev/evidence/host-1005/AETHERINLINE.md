# AETHERINLINE — Aether's inline formatting context, stacking contexts, font-relative units, tables and paint effects

Ledger row SR50. Branch `exec-host-aetherinline`, cut at `23fd7ed5`; `exec-host-aethervideo2` (@107efeb2) merged clean,
then `exec-host-aethercss` (@2d43c5c0) hand-joined (`5daa790a`). Lane: `handlers/aether` layout + paint (AETHERDOM,
SR49, holds the DOM/parse lane). Oracle: Chromium through EYES (`tools/eyes/run.sh aether`: SSIM on 2x-downsampled
greyscale plus pixel mismatch, baseline gate), and for video the `aethervideo-check` oracle. No new third-party crate.

## The finding

AETHERCSS left Aether at SSIM 0.971 with its inline layout still a flex approximation: every text run and inline box
was one taffy rectangle in a wrapping row, so text after a wrapped run could not continue on that run's last line and
an inline background painted one box instead of one strip per line. Stacking was per parent, `em` was 16px, tables had
no min-content or rowspan, paint had no radial gradients, inset shadows, alpha backgrounds or rounded clipping. Looking at
the frames also showed two text-level losses the row did not name: CJK text was invisible and zero-width (no font
fallback), and an unstyled page was sans where Chromium's default standard font is serif. Each was fixed from its
spec section and accepted only on an EYES improvement.

## Score (Chromium oracle)

Mean SSIM **0.971 → 0.988** (target ≥ 0.985), pixel mismatch **1.6% → 0.9%**; the 18 join cases **0.971 → 0.987**.
Text-heavy targets (≥ 0.96): **01 0.935 → 0.977, 11 0.934 → 0.973, 13 0.953 → 0.992, 14 0.936 → 0.977**.
[AETHERINLINE-SCORE-before.md](AETHERINLINE-SCORE-before.md) (the join), [AETHERINLINE-SCORE-after.md](AETHERINLINE-SCORE-after.md).

| case | join | after |
|---|---:|---:|
| 01-blog-article | 0.935 | 0.977 |
| 02-flex-two-column | 0.988 | 0.992 |
| 03-card-gallery | 0.993 | 0.998 |
| 04-sticky-nav | 0.972 | 0.990 |
| 05-form | 0.936 | 0.954 |
| 06-table-zebra | 0.986 | 0.994 |
| 07-images-svg | 0.937 | 0.936 |
| 08-inline-wrap | 0.979 | 0.990 |
| 09-media-queries | 0.992 | 0.995 |
| 09-media-queries@375 | 0.989 | 0.992 |
| 10-calc-clamp | 0.998 | 0.999 |
| 10-calc-clamp@375 | 0.998 | 0.999 |
| 11-non-latin | 0.934 | 0.973 |
| 12-js-mutation | 0.994 | 0.997 |
| 13-ua-defaults | 0.953 | 0.992 |
| 14-boxes | 0.936 | 0.977 |
| 15-landing | 0.954 | 0.994 |
| 16-positioned | 0.997 | 0.999 |
| 17-inline-continue (new) | 0.703 | 0.990 |
| 18-stacking (new) | 0.976 | 0.999 |
| 19-em-units (new) | 0.833 | 0.995 |
| 20-table-spans (new) | 0.849 | 0.990 |
| 21-paint-effects (new) | 0.836 | 1.000 |

New-case "join" columns are the binary before the milestone that implements them (17 at the join, 18/19/20 at M2,
21 at M4). 07-images-svg is 0.001 under the join (glyph AA inside the SVG labels; within the gate). Frames:
`aetherinline-<case>-after.png` beside `aetherinline-<case>-ref.png` for 17, 20, 21.

## Milestones

| M | commit | spec | what | moved |
|---|---|---|---|---|
| join | `5daa790a` | — | AETHERVIDEO + AETHERCSS hand-joined (media map + sizes map through resolve, both box-builder skips); comments/doctype generate no box (the video oracle failed without it) | 0.971 = AETHERCSS; video oracle passes |
| 1 | `e028325d` | CSS2 §9.4.2, §10.6.1, §10.8, §9.5, §16.2; css-text-3 §4.1.3, §5.1; css-break-3 §5.4 | `layout/inline.rs`: line boxes; runs continue across items (`fonts::lines::break_lines_from`); inline boxes split per line with slice edges; atomic inlines with baselines; strut + half-leading + vertical-align (all keywords, length, %); empty lines zero-height; text-align; floats shorten lines; heights fed back to taffy | 0.971→0.976; 01 →.971, 15 →.990, 05 →.951; 17: .703→.980 |
| 2 | `f50ab729` | CSS2 Appendix E, §9.9.1; css-position-3 §8; css-flexbox-1 §4.3; css-color-4 | stacking contexts: layers collected through normal flow and z-auto positioned boxes, never into nested contexts; seven-step order; opacity groups composited once (exact source-over by backdrop snapshot) | 18: .974→.996 |
| 3 | `08af0628` | css-values-4 §6.1; css-fonts-4 §2.5; CSS2 §10.8.1 | em/ex/ch of the element's own font, font-size em/% of the parent, rem of the root (re-applied in cascade order at remeasure); line-height number vs length/percentage; monospace 13/16 only for keyword-derived sizes | 19: .833→.993 |
| 4 | `ebdf65d1` | CSS2 §17.5.2.1, §17.5.2.2, §17.2.1, §17.5.3-4 | min-content columns, rowspan (slot grid + spanning cell sized to its rows), table-layout: fixed, cells stretch with vertical-align middle | 06 .986→.988; 20: .849→.987 |
| 5 | `ec964d47` | css-images-3 §3.2; css-backgrounds-3 §7.1.3, §5.3; css-color-4 | radial/repeating-radial gradients, inset shadows, rgba backgrounds and borders source-over, rounded overflow clip | 03 →.995; 21: .836→1.000 |
| 5b | `3c59a270` | — (Chromium's per-char fallback; Skia positioning) | per-character font fallback in fontconfig's sans-serif order; quarter-pixel glyph phases | 0.979→0.984; 11 .935→.973, 04 →.990, 08 →.990 |
| 5c | `f5eeb01c` | CSS Fonts initial value (UA) | initial font serif (Chromium's default standard font) | 13 .952→.992 |
| 5d | `a05c322e` | css-backgrounds-3 §4.3 (Blink) | dotted sides as Blink distributes them (measured) | 14 .938→.977 |

## Known-answer tests (`cargo test -p aether`: 128 passed; 121 at the join)

`lines::continuation_kat`, `test_inline_formatting_context_kat`, `test_stacking_context_kat`,
`test_font_relative_units_kat`, `test_table_spans_kat`, `test_paint_effects_kat`, `effects::radial_gradient_kat`
(new); `effects::box_shadow_parse_kat` (now inset), `boxpaint::dash_and_dot_fit_kat` (the measured dotted pattern).
`cargo test -p aethervideo-check`: the frame-0/7/9 oracle passes.

## The honest ceiling

- Floats: only floats INSIDE an inline formatting context shorten its lines; a float that is a block-level sibling
  does not wrap the next paragraph's lines (no block-formatting-context float list), and line edges are taken at
  the line's top only.
- Inline layout: no `text-indent`, `text-align: justify` (treated as start), bidi/RTL reordering, `::first-line`;
  relatively positioned non-atomic inlines are painted in flow without their offset; a relatively positioned
  atomic inline in a line loses its offset. Hit-testing and `getBoundingClientRect` still read the taffy (flex)
  boxes of inline content, not the line fragments.
- Absolutely positioned boxes are placed against their PARENT box (taffy), not the nearest positioned ancestor.
- Stacking: `transform`, `filter`, `isolation`, `mix-blend-mode` do not form contexts; clips of non-containing-block
  ancestors still clip hoisted positioned descendants.
- Tables: rowspan cells do not grow the last spanned row when their content is taller; no `vertical-align: baseline`
  row alignment (treated as top), no column groups/`<col>` widths, collapsed borders do not resolve conflicts.
- Paint: conic gradients; multiple background layers; per-side border alpha (one alpha for all sides); opacity
  groups snapshot the whole surface (correct, not cheap).
- Fonts: font-kit faces and its rasterizer; the fallback chain is this machine's fontconfig order, hard-coded, and a
  fallback face's metrics do not enlarge `line-height: normal` lines as Blink's do. The residual on 05/07 is form
  control and SVG label glyphs.

## Third-party crates the arc leans on

None added. Still under Aether's layout: `taffy` 0.14 (flex sizing and block geometry — chicken wire for block/flex
layout, named in EYES.md; inline content, line boxes, tables, stacking, margin collapsing and painting are Aether's
own), `font-kit` (face loading and glyph rasterization — chicken wire for glyph rendering), `kuchiki` (DOM, AETHERDOM's
arc). `taffy` no longer positions inline content: an IFC root's children keep taffy nodes only for intrinsic sizing.

## What stays owed

- A block formatting context with a float list (floats wrapping following blocks' lines), containing-block-correct
  absolute positioning, line fragments for hit-testing/`getBoundingClientRect`.
- taffy out of the block path entirely (it remains for flex); a glyph rasterizer of our own (font-kit's is the gap on
  05/07).

## How a future executor continues

`tools/eyes/run.sh aether`; read `tools/eyes/out/aether/SCORE.md`, open the worst case's subject/ref PNGs. Remeasure
order now: display contexts, text-align, inline whitespace, font resolve (+ font-relative re-application, second
resolve), UA margin rescale, block widths, margin collapsing, layout pass 1, (tables + %-math, pass 2), rowspan
heights, IFC line boxes with height feedback until stable, restore. `UNAOS_LAYOUTDUMP=12` prints each IFC's lines and
fragments under its root.
