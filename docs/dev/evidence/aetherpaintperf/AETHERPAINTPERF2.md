# AETHERPAINTPERF2 (LEDGER SR132): layout and the post-relayout frame, scoped

Arc base `96bdc4de`, branch `exec-host-aetherpaintperf2`. This file is the design summary and the expected
measurement. The commit bodies hold the full design with file:line evidence. Nothing here was run by the
executor: under R107 it wrote the code, compiled it, and left every check to the fold.

## What changed

| Path | Before (SR130) | After |
|---|---|---|
| DOM changes made outside js/dom.rs | Sixteen `touch()` sites recorded no change, so the next restyle cascaded the whole document | Each records one `css::Change` (form state, `document.open`, re-parse, nested documents, the listbox keys). The restyle stays scoped |
| Relayout (`AetherEngine::relayout`) | A fresh box tree, laid out from an empty taffy cache. `laid_out` was the whole tree on every relayout | The fresh tree is `layout_core::retarget`ed onto the previous tree's boxes. The cascade and the style rewrites run on that twin, and the kept taffy tree takes the twin's values (`layout_core::remeasure_kept`). Taffy re-lays only the boxes whose style, children or measure inputs moved, plus their ancestors |
| `laid_out` | `total_node_count` of every remeasured tree | `LayoutTree::relaid`: before each taffy pass, the dirty boxes reachable through dirty boxes, summed over passes |
| The frame after a relayout | Viewport damage | The union of the old and new subtree ink of every on-screen box whose place in the walk, origin, layout box or paint inputs changed (`render::relayout_damage`) |

## How the kept relayout stays the fresh layout

`remeasure`'s style rewrites are not idempotent. For example, margins collapsed twice are collapsed again. Some
rewrites are set after a taffy pass and restored at the end. The kept relayout therefore never reruns them over
their own output. The rewrites always run on the fresh build's specified styles, in a twin. The twin is a copy of
the kept tree's taffy arena, so it has the same slot keys. The kept tree then takes the twin's values box by box,
and only where a value differs. Every value every pass sees is the fresh tree's. A clean box answers from a taffy
cache that holds the layout of the same style, subtree and measure inputs.

The twin carries the kept keys, not fresh ones, for two reasons:

- The cascade keys `LAST_CASCADE` and `computed_report` by taffy id (css/mod.rs:723, 1162).
- The kept arena makes the same number of inserts from the same slotmap state, and that replay is checked.

One rewrite is left applied on a kept tree: the block-width placeholders. No reader after layout reads them.
`apply_specs` and the in-place `remeasure` put them back before folding or laying out again.

`layout_core/tests/relayout_kept.rs` covers each rewrite: a fresh layout, a kept relayout of it, and a kept
relayout of that must give the same boxes and line boxes.

## The fixtures (handlers/aether/tests/perf_paint.rs)

`cargo test -p aether --test perf_paint -- --nocapture`. Viewport 800x600. Each fixture also asserts byte identity
against the scopes-off whole-page frame (`perf::set_scoped(false)`, a fresh relayout, viewport damage).

| Fixture | Boxes | Bounds asserted (after) | Expected (after) |
|---|---|---|---|
| `list`: 2,000 rows, row 5's `textContent` changes | 4,004 | laid_out ≤ 16 (before ≥ 4,000), cascaded ≤ 4, painted ≤ 30, skipped ≥ 1,900, glyph_hits ≥ 1, one scoped restyle | cascaded 1. laid_out 6: the document node, html, body, #list, the row and its new text. painted about 13: those four ancestors and the rows whose ink meets row 5's old and new ink, rows 1 to 9. glyph_hits about 8 |
| `spa` (new): 2,000 rows in 40 pages of 50, page 0's rows replaced by `innerHTML` | 4,044 | cascaded ≤ 60, laid_out ≤ 120 (before ≥ 4,000), painted ≤ 100, one scoped restyle | cascaded 51 (the page and its 50 rows: the zebra reads sibling structure). laid_out 105: the four ancestors, the page, 50 rows and 50 texts. painted about 40: every visible row was replaced, so the damage is about the viewport. glyph_hits about 0: the new rows' text is new |
| `anim`: 300 boxes, one `background-color` animation | 305 | per frame: cascaded 0, laid_out 0, painted ≤ 8 | unchanged from SR130: painted 5 |
| `keys`: a field and 200 paragraphs, 20 keystrokes | 206 | per frame: cascaded 0, laid_out 0 (typing relayouts nothing), painted ≤ 8 | unchanged from SR130: painted about 6 |

`cargo test -p layout_core --test relayout_kept` adds the following.

- A fixture per rewrite: `fix_display_contexts`, `propagate_text_align`, `collapse_inline_whitespace`, `rescale_ua_margins`, the % width pass, block widths with margin collapsing, the inline heights, vertical writing, replaced sizes, and a table with multicol, which lays out whole. For each, kept relayouts once and twice give the same geometry as a fresh layout.
- 200 fixed-height rows: relaid 0 on an unchanged kept relayout.
- One row's text changed: relaid ≤ 8, where a fresh layout gives 404.

## What stays whole-page, and why

1. **The box build and the style rewrites.** Every relayout still builds a fresh box tree. That is an O(n) DOM walk with no layout. It then clones the kept taffy arena, pairs boxes and runs `apply_specs`, generated content and the rewrites over the whole twin. `sync` compares every box's style and children. All of this is O(n) without taffy work. Building the twin incrementally would need a builder that proves a reused DOM subtree's build inputs unchanged: `nested_list` reads ancestors, inter-element space reads siblings, quirks reads the document. The value diff needs no such proof.
2. **The inline formatting contexts** (`inline::layout_all`). Every root's line boxes are broken again on every relayout. This is layout_core's own line breaker, not taffy, so `laid_out` does not count it.
3. **Boxes re-laid on every relayout although unchanged**, together with their ancestors:
   - auto-height inline formatting roots: their height is set after the first taffy pass and restored at the end;
   - boxes whose vertical margins collapse: `collapse::apply` and `restore` (devtools reads the restored margins);
   - vertical-writing boxes and table cells' minimum heights.

   The restores rewrite their styles after the last taffy pass, which empties their caches. On a text page of auto-height paragraphs, most paragraphs are re-laid.
4. **Generated-content boxes** (`::before`, `::after`, `::marker`). The cascade builds them into the twin on new slots every time (layout/generated.rs, not this arc's file), so they and their ancestors are re-laid. Keeping their keys means pairing them with the kept tree's boxes of the same host element and pseudo kind. That change is owed in layout/generated.rs.
5. **Relayouts laid out whole** (`laid_out` = the tree):
   - the first relayout after a load, or any relayout without a previous tree;
   - the scopes off;
   - a changed viewport;
   - sheets with `@container` (their second round rebuilds the tree);
   - a web-font generation that moved since the kept tree was measured;
   - a tree holding a table (cross-box lists keyed by box), a multicol container (narrowing restores in box-key order), or an author outline (painted in box-key order). These are the paths byte identity could not be argued for under the kept keys, because the kept keys order boxes differently from a fresh build;
   - a twin whose insert replay does not match;
   - an animation frame that moved a layout property: that frame reruns the in-place `remeasure` on the kept tree, with every measured leaf dirtied.
6. **The frame after a relayout damages the viewport** when:
   - there is a selection, find highlights or a focused element (carets and the focus ring paint from state the diff does not read);
   - an author outline exists, or the canvas background, the viewport or the scroll offset changed;
   - there is no snapshot of the replaced layout;
   - a changed box sits in a call whose own ink is unbounded (a transform, filter or clip-path group draws it elsewhere than its ink).

   Images, background images, canvases, SVG, frames, media and form controls always count as changed, so their ink is damaged.
7. **The index walk** visits every box per frame (SR130). It now also records the on-screen boxes. A post-relayout frame runs it twice: once for the diff, once for the paint.
8. **Restyle paths**: as SR130 lists, minus the sixteen sites this arc records.

## Compile and seat notes

`CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave31/target cargo check -p layout_core -p aether --tests`
returned rc=0 at each code commit.

The render commit's first run returned rc=101. The errors named this branch's new `LayoutTree` fields as missing,
because a neighbour worktree's `layout_core` artifact was in the shared target. After `touch` and a rerun, it
returned rc=0. This is the collision AETHERPAINTPERF.md records.
