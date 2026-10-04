# AETHERSEE — an executor loop that can see Aether's output

Ledger row SR23. Branch `exec-aether-see`, cut from `8225dd86`. Peter (2026-10-04): "there's a megaton of
work behind a web browser and would be helpful if the executors are able to 'see' and test their results."

## The finding

Aether already had the eye (`aether render --html page --out page.png --ledger page.txt`, one frame to PNG
plus the missing-API ledger), but nothing to compare the frame against, so "does this page look right?" was a
judgement made by whoever happened to look. The loop below supplies the reference (Chromium, pre-installed with
Playwright), a number (SSIM + pixel mismatch per page), a picture of the difference (`*.diff.png`, red where
the frames disagree), and a gate. The first look at the numbers showed that the largest misses are not exotic
APIs from the ledger: they are core layout and paint behaviour that the ledger cannot see, because the property
is "handled" (`display:flex` parses) or there is no property at all (the canvas background, which font the
measurer used).

## The loop in two commands

```
cargo build --release -p aether --bin aether && cargo build --release -p aether-audit
tools/aether-audit/run.sh          # renders, scores, writes out/SCORE.md, gates against baseline.json
```

`run.sh` does both builds itself, so the second line alone is enough. Then read `tools/aether-audit/out/SCORE.md`
and open the worst page's three PNGs (`out/<page>.aether.png`, `.ref.png`, `.diff.png`) with the Read tool.
`run.sh --only 05` re-renders one page; `run.sh --live` adds the optional live URLs (never gated);
`target/release/aether-audit gate --write` rewrites the baseline after an intended improvement.

## The corpus (`tools/aether-audit/corpus/`, 16 pages, 18 frames)

Every page is self-contained (inline CSS, images as `data:` URIs) and its first line is a comment saying what it
tests. A page whose header mentions 375 is rendered at 800x600 and at 375x812.

| page | tests |
|---|---|
| 01-blog-article | headings, paragraphs, ol/ul, blockquote, inline code, pre block, serif body |
| 02-flex-two-column | fixed sidebar + fluid main via `flex: 0 0 200px` / `flex: 1`, gap, header/footer |
| 03-card-gallery | flex-wrap card gallery (grid is not claimed by Aether), gap, radius, shadow |
| 04-sticky-nav | `position: sticky` nav of inline-block links under a gradient hero |
| 05-form | text/email/password inputs, select, checkbox, radio, textarea, buttons |
| 06-table-zebra | border-collapse, cell borders, th, `:nth-child(even)` zebra rows, caption |
| 07-images-svg | PNG scaled by width/height attributes, SVG data URI, inline `<svg>`, figcaption |
| 08-inline-wrap | nested b/i/u/s/mark/sup/sub/small/a; overflow-wrap and word-break on long words |
| 09-media-queries (+@375) | `@media (max-width: 600px)` switches direction, colour, display:none |
| 10-calc-clamp (+@375) | calc/clamp/min/max with %, vw, rem for widths, padding, font-size |
| 11-non-latin | Japanese, Chinese, Cyrillic, Greek, Hebrew fallback in a sans-serif page |
| 12-js-mutation | createElement/appendChild, textContent, classList, style, innerHTML on load |
| 13-ua-defaults | no author CSS at all: h1-h6, hr, dl, nested lists, pre, link colour |
| 14-boxes | border styles/radius, per-side borders, margin collapsing, inline-block chips |
| 15-landing | centred max-width hero, uppercase + letter-spacing, inline-block CTA buttons |
| 16-positioned | relative/absolute badge, z-index stacking, position: fixed bar |

Live (optional, `corpus/live.txt`): example.com, en.wikipedia.org/wiki/Web_browser, text.npr.org. All three
were refused by this sandbox's egress policy (`connect_rejected`) on 2026-10-04, so they are scored as
"no reference frame" and never enter the baseline.

## The scorer (`tools/aether-audit`, Rust bin `aether-audit`, plus `ref.mjs`)

- Reference: `ref.mjs` drives `/opt/pw-browsers` Chromium at device scale 1, no LCD text, and screenshots the
  viewport. Corpus references are cached by mtime (a reference only re-shoots when its page changes).
- Aether: `target/release/aether render --html <page> --width W --height H`, 90 s timeout (child killed by its
  own handle).
- Scores per frame: **SSIM** (mean over 8x8 windows, stride 4, of the 2x-downsampled greyscale frames; the
  downsample makes it a layout-and-structure measure that is tolerant of sub-pixel anti-aliasing) and
  **mismatch %** (pixels whose largest channel differs by more than 40). The ledger dump gives the distinct
  missing-API count and the top three by call count.
- Output: `out/<id>.aether.png`, `out/<id>.ref.png`, `out/<id>.diff.png` (reference faded to grey, red where the
  two frames differ, intensity by difference), `out/scores.json`, `out/SCORE.md` worst first.
- Determinism: two consecutive runs, and a run with `--refresh-ref`, produced byte-identical `scores.json`.

Before any fix: mean SSIM **0.741**, mean mismatch **14.2%** ([SCORE-before.md](SCORE-before.md)).

## M2 — looking at the three worst

**15-landing (SSIM 0.510, mismatch 45.3%).** ![before](fix2-canvas-before.png) vs ![ref](fix2-canvas-ref.png)
- Missing default/propagation: the dark `body` background paints only the body's own box; everything below the
  content is white, where every browser paints the root/body background over the whole canvas. This alone is
  most of the 45% mismatch.
- Font metrics: the h1 wraps "read" onto a second line and the line overlaps the paragraph — the box was
  measured with the regular face, painted with the bold face, so painted text is wider than its box.
- Missing property: `letter-spacing` (ledgered), `border-radius` (parsed, not painted); the CTA anchors'
  inline-block shrink-to-fit width wraps "Get started" and clips the second line.
- Layout: the paragraph ignores the hero's `max-width` and spans the viewport.

**14-boxes (SSIM 0.525, mismatch 15.6%).**
- Missing default/propagation: same canvas background miss (`#fafafa` body, white canvas below it).
- Layout: no margin collapsing — adjacent 16px margins give a 32px gap, so every box drifts further down.
- Missing paint: dashed/dotted styles paint solid, `border-radius` not painted; boxes are 16px too wide on the
  right (body padding-right lost).

**01-blog-article (SSIM 0.613, mismatch 13.4%).** ![before](fix3-font-before.png) vs ![ref](fix3-font-ref.png)
- Font metrics: every heading overlaps the next block. Headings are bold; the measurer always used the family's
  *regular* face, the painter the *bold* face (and, for serif, a sans-bold face), so "Three stages" is measured
  narrow, wrapped by the painter at a width it was never given, and the second line lands on the list.
- Family: headings in a `font-family: serif` page paint in sans-serif bold (the painter only ever had a sans
  bold face); italics never paint italic.
- Missing default style: list markers (1. 2. 3., bullets) are not painted; `pre` collapses its newlines.

**Also read:** 03-card-gallery (SSIM 0.741, mismatch 32.1%) — ![before](fix1-flex-before.png) vs
![ref](fix1-flex-ref.png). The six cards stack in one column. `display:flex` maps to a taffy flex box but
keeps Aether's internal *column* direction (its block approximation), and `flex`, `flex-wrap`, `gap`,
`flex-grow/shrink/basis` are not implemented at all (`property:flex`, `property:gap`, `property:flex-wrap` in
the ledger). Same miss on 02 (sidebar not beside main) and 09 (three columns stacked at 800px).

### The three largest fixable misses (picked)

1. **Flexbox is not flexbox** — `display:flex` must be a row container and the flex properties must reach taffy
   (02, 03, 09; the most common modern layout primitive).
2. **Canvas background** — the root/body background must cover the whole viewport (14, 15, and any page with a
   coloured body).
3. **Measure and paint disagree on the font** — one face resolver (family + weight + style) shared by the text
   measurer and the painter (01, 15 and every page with bold/serif/italic text).

Margin collapsing, list markers, `pre` whitespace, border styles/radius and letter-spacing are real but each is
smaller or larger-scoped; they are recorded under "What stays owed".
