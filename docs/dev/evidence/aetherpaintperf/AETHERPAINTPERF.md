# AETHERPAINTPERF (LEDGER SR130): what a frame pays for

Arc base `4f3a436e`, branch `exec-host-aetherpaintperf`. This file is the design summary and the
expected measurement. The commit bodies hold the full design with file:line evidence. Nothing here was
run by the executor: under R107 it wrote the code, compiled it, and left every check to the fold.

## What changed

| Path | Before | After |
|---|---|---|
| Restyle (`css::apply_stylesheets_scoped`, from `AetherEngine::relayout` and the first cascade of a load) | Every relayout ran css_core's cascade on every element (`cascade_document`) | Only the elements the recorded DOM changes can re-match are re-cascaded. The rest reuse their last cascade. |
| Dirty roots (css_core `cascade::invalidation`, `RuleSet::invalidation`) | None | Each class, id and attribute a selector reads gets a reach (SelfOnly, Subtree or Siblings). Attribute-derived states are folded in. Host-driven states (`:hover`, `:focus*`) are diffed by the engine. `:empty` and sibling structure carry flags. |
| Mutation record (js/dom.rs `restyle_root` at each of its ten `touch()` sites) | Only the `mutated` flag and the DOM version | One `css::Change` per DOM version. A version with no recorded change makes the restyle whole. |
| Paint (`render::render_frame`) | Walked and painted every box, clipping each pixel to the damage | An index walk records each box's subtree ink and the text boxes, without writing pixels. The paint walk then skips every box whose ink meets no damage rect. |
| Text (`draw_shaped`, render/glyphcache.rs) | Each run was shaped, placed and rasterized on every frame | Rasterized runs are kept by (text, face, size, spacing, direction, colour, exact pen, font generation), in an LRU capped at 8 MiB. |
| Animation frames (`css::animation_frame_scoped`, `AetherEngine::animation_damage`) | Every frame damaged the whole viewport | When only colours, opacity or background-position animate, the frame damages just the animated boxes' subtree ink. |
| Counters (`aether::perf`) | None | `cascaded`, `laid_out`, `painted`, `skipped` and `glyph_hits`. The line `:: paintperf: cascaded=… laid_out=… painted=… glyph_hits=… ::` prints on every frame under `AETHER_PERF=1`. `perf::set_scoped(false)` or `AETHER_NO_SCOPE=1` turns every scope off and serves as the whole-page oracle. |

## The fixture (handlers/aether/tests/perf_paint.rs)

`cargo test -p aether --test perf_paint -- --nocapture`. Each fixture prints `PAINTPERF fixture=… before: … | after: …`.
Viewport 800x600.

| Fixture | Elements | Before (scopes off) | After: bound asserted | After: expected |
|---|---|---|---|---|
| `list`: 2,000 rows with an nth-child zebra; row 5's `textContent` changes; one tick, one frame | 2005 | cascaded ≥ 2000, painted ≥ 2000, glyph_hits 0 | cascaded ≤ 4, painted ≤ 100 (5 %), skipped ≥ 1900, glyph_hits ≥ 20, one scoped restyle | cascaded 1 (the row); painted about 40 (the document node, html, body, #list and about 36 on-screen rows); laid_out = the whole tree (not scoped) |
| `anim`: 300 flex boxes, one with a `background-color` animation; ten frames | 305 | painted per frame ≥ 300 | per frame: cascaded 0, laid_out 0, painted ≤ 8 | painted 5 (the document node, html, body, #grid, #a) |
| `keys`: an autofocus field and 200 paragraphs; twenty typed characters | 206 | painted per frame ≥ 200 | per frame: cascaded 0, laid_out 0, painted ≤ 8; value `abcdefghijklmnopqrst` | painted about 6 (the document node, html, body, form, the field and the first paragraph, whose glyph ink margin reaches up into the field) |

Byte identity is one assertion per fixture. The frame the scoped path painted must equal the frame the
whole-page path paints from the same state: scopes off, a whole relayout (`list` and `keys`; not `anim`,
because a relayout re-samples the timeline at a later time), and viewport damage.

## What stays whole-page, and why

1. **The box build and the taffy passes.** Every relayout builds a fresh tree (`layout_core::build_tree`,
   tree.rs:250) and lays all of it out (`remeasure`, tree.rs:1367), so `laid_out` equals the tree. Keeping the
   taffy tree and re-syncing only dirty subtrees was not built. The reason is that `remeasure` rewrites the
   styles of the tree it is handed, and several of those rewrites are not idempotent:
   - `fix_display_contexts` (tree.rs:1001), `propagate_text_align` (835), `collapse_inline_whitespace` (896)
     and `rescale_ua_margins` (1141);
   - the pass-1 % width resolution (around 1617);
   - the multicol narrowing and its restore (around 1652 and 1796);
   - the saved heights and widths, and `writing` / `collapse` / `blockwidth::restore`.

   Running them again over a kept tree changes their inputs. That a kept tree equals a fresh build can only
   be shown with a test loop, and R107 allows none. For that reason layout_core/tree.rs and
   layout_core/tests are untouched. The multicol, scroll and animation layout passes run as before. Only paint-only
   animation frames skip layout, and they already did.
2. **Per-relayout sheet work.** The stylesheets are parsed, the rule set and its index are built, and
   `ledger_rules` and `webfont::apply_rules` run (css/mod.rs `apply_stylesheets_with`). This costs per
   sheet, not per element.
3. **Whole-document map work after the cascade.** This covers `generated::generate` (counters and quotes
   follow document order), the `apply_specs` fold, `anim::sync`, `LAST_CURSORS` and `LAST_CASCADE`. Each is
   one pass over the styles, with no matching.
4. **The frame after a relayout damages the viewport** (`relayout`). Invalidating paint across two layouts
   needs a fingerprint of every paint input of every box: form values, images, canvas bitmaps, media frames,
   selection, caret and counters. That fingerprint is not built. The paint index still skips every
   off-screen box of that frame.
5. **The index walk.** It visits every box once per frame. It writes no pixels and does no shaping.
6. **Paths the restyle cascades whole:**
   - a DOM change made outside js/dom.rs: innerHTML and friends (js/html.rs), form state (js/controls.rs,
     js/forms.rs, js/events.rs), js/loader.rs, js/frames.rs, a11y/keyboard.rs;
   - `:has()`, `:lang()`, `:dir()`, `::slotted()`, `::part()` or `:defined` in the rules;
   - `@container` rules, whose matches follow layout sizes;
   - shadow trees;
   - changed sheets, viewport or media environment;
   - more than 65,536 changes between restyles;
   - `perf::set_scoped(false)`.
7. **Animation frames that damage the viewport.** This happens when the animation is a transform, filter,
   clip-path, visibility, z-index, outline colour or layout property. It also happens when the animated
   element is the root or body (the canvas background), when a box under it has an outline (painted after
   the walk, outside the box), or when its ink is unbounded.
8. **Boxes the index never skips (unbounded ink), together with their ancestors:**
   - transforms, filters, clip-paths and backdrop filters;
   - box shadows and text shadows;
   - vertical writing modes, column fragments, multicol rules and scrollbars;
   - form controls, `<video>` and `<audio>`;
   - author `::marker` text;
   - text outside an inline formatting context.
9. **The glyph-run cache** keys on the exact pen position, so a run that moved (scrolled horizontally, or
   reflowed) misses.

## Owed outside this arc's files (report only)

- Scoped restyle for innerHTML and the form-state paths needs a recorded change after each of their
  `touch()` calls. The change is one line after each: `crate::css::note_change(page(|p| p.version),
  crate::css::Change::Children { parent, added })` for a children change, or `Change::Attribute { … }` for
  an attribute change. The sites are js/html.rs:993, 1053, 1079, 1133, 1150, 1216 and 1291;
  js/controls.rs:582 and 2315; js/forms.rs:1054; js/events.rs:878; js/loader.rs:812 and 847;
  js/frames.rs:353; a11y/keyboard.rs:215 and 218. Until then, each of these leaves a gap that makes the
  restyle whole.

## Compile and seat notes

`CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave30/target cargo check -p css_core -p layout_core -p aether --tests`
returned rc=0 at each code commit.

The shared target directory collides across worktrees. A path dependency's metadata hash is relative to
the workspace, so every worktree's `css_core` writes the same `libcss_core-<hash>.rmeta`. One compile went
red with `could not find invalidation in cascade` against another executor's `css_core`. After `touch`
and a rerun it returned rc=0. A fold check that sees that error is reading a neighbour's artifact.
