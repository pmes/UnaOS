# AETHERSTYLE (LEDGER SR54): Aether styles through css_core

Branch `exec-host-aetherstyle`, cut at `b58f647b`. It merges `exec-host-aetherdom@26a02278` (Aether on
html_core's arena, `dom/select.rs` as the seam; it carries aethervideo2, htmlcore, EYES and DEPS) and then
`exec-web-csscore@06c2e152` (`unaos/libs/web/css_core`). The only conflict was the root `Cargo.toml` member
list, resolved by keeping both. Lane: style. The `dom/` and `css/` modules changed, plus css_core's cascade.
Layout and paint changed by one word: `layout::is_inline` became `pub(crate)` so that the computed-value
report can use it. AETHERINLINE (SR50) is live on those files.

## Finding

Aether parsed CSS with `cssparser` 0.38 and matched with servo `selectors` 0.41 (SR31 lists both as chicken
wire). Around them sat about 900 lines of salvage machinery:

- a text-level rule splitter;
- a rewrite-before-compile "lowering" of `:is/:where/:not(list)/:required`, because selectors 0.41 rejects
  them;
- named ledger buckets for whatever could not be rewritten;
- a flat, document-scoped `var()` map with no per-element cascade;
- a hand-rolled `@media` evaluator (min/max-width plus a few keywords) and an `@supports` string splitter.

CSSCORE (SR47) agreed with Chromium on every computed cell of every EYES page. AETHERSTYLE removes the two
crates and the salvage, and runs Aether's whole cascade through css_core.

## What changed

| milestone | file | change |
|---|---|---|
| M1 | `handlers/aether/src/dom/select.rs` | **`El<'a> { doc: &Document, id }` implements `css_core::matching::Element`**. It covers name, namespace, attributes, parent and element siblings, first element child, `:empty` and `:root` (parent is the Document node, so a detached element is not root). The fast `attr`/`has_attr`/`id_is`/`has_class` read html_core's attribute list directly. `state()` stays css_core's `html_state`: the attribute-derived states, with no hover, focus or target in a static render. `Selectors` is now a css_core `SelectorList`, and `matches_in(doc, id, scope)` gives `:scope`. |
| M1 | `src/dom/mod.rs` | `select()` (querySelector/All, `net`, `media`, `lib`) compiles with css_core and walks the subtree under ONE arena borrow. JS `querySelector` therefore gets all of Selectors 4: `:is/:where/:has/:not(list)`, `nth-*(of S)`, attribute flags and form states. |
| M2 | `src/css/mod.rs` | **The cascade is css_core end to end**, in this order: `parse_stylesheet` (Syntax 3, error recovery, Nesting) → `RuleSet::add_sheet`, which resolves `@media` (MQ4 against Aether's viewport through `media_environment`), `@supports` (css_core's grammar, with declarations judged by Aether's honest `property_supported`), `@layer` and `@import` (no loader, so ignored) → `build_index` → per element, in pre-order, `cascade_filtered`. That last step does the rule-hash candidates and ancestor filter, Selectors 4 matching, and the full Cascade 5 sort including the style attribute and `!important` layering. Next, `compute_custom_properties` computes the element's custom properties against its parent's (inheritance, cycles, guaranteed-invalid). Then every other winning declaration, in cascade order, is `var()`-substituted (`substitute_vars`; a failure is invalid at computed-value time and is dropped and ledgered), serialized (`serialize::to_css`) and handed to **Aether's own computed-value step, `apply_declaration`**. That step is unchanged and folds the value into the `SpecifiedStyle` that layout and paint consume. Elements whose winning declaration list is identical and uses no `var()` share one folded style. The old `Rule`, `MatchPlan`, `RuleKey`, `required_tokens`, the lowering, `collect_rules`, `media_matches`, `supports_matches` and `collect_custom_props` are deleted (−1,650 lines). |
| M3 | `handlers/aether/Cargo.toml`, `Cargo.lock` | − `cssparser`, − `selectors`, − `precomputed-hash`; + `css_core` (path); + dev-dependency `crypto_core` (path, in-tree: the vector sha256). **`cargo tree -p aether -e normal` shows neither cssparser nor selectors.** Nothing else in the workspace used them, so the lock lost 9 packages: cssparser 0.38.0, cssparser-macros 0.7.1, selectors 0.41.0, servo_arc 0.5.0, precomputed-hash 0.1.1, derive_more + derive_more-impl 2.1.1, dtoa 1.0.11, dtoa-short 0.3.5. |
| M4 | `unaos/libs/web/css_core/src/cascade.rs` | **Rule hashing.** `RuleSet::build_index` buckets every selector by its subject compound: id, else class, else type (lowercased), else attribute name, else universal. `cascade` walks only the buckets the element can satisfy, in entry order. **Ancestor Bloom filter**: each entry carries a 256-bit mask of the ids, classes and types that its compounds left of a descendant/child combinator require of some ancestor. `AncestorFilter::with_element` builds the element's filter top-down, and `cascade_filtered` skips entries the filter rules out. A rule's per-selector dedup became a neighbour check, because a rule's selectors are consecutive entries. |
| M4 | `src/css/mod.rs` | `last_style_timing()` reports (cascade, re-layout). `AETHER_NO_RULE_INDEX` gives the full scan, for measurement. `AETHER_STYLE_PROFILE` prints the phases. |
| M5 | `src/css/mod.rs` | `computed_report(tree)` gives **Aether's computed values** for the 20 oracle properties, serialized like `getComputedStyle`. They are read from what layout and paint consume: the taffy style; the paint map with the renderer's own inheritance (`layout::default_*` UA defaults, link colour, underline propagation); and the folded `SpecifiedStyle` for the facts the box tree no longer distinguishes (flex vs block, absolute). |

**Where `Element` lives, and why.** It lives in `handlers/aether/src/dom/select.rs`, NOT in html_core:

1. The two cores are independent `no_std` libraries. html_core has consumers that never style anything,
   and css_core has its own test DOM.
2. The impl needs nothing beyond html_core's public traversal hooks and attribute list.
3. The states the matcher asks about beyond attributes (`:hover`, `:focus`, `:target`) belong to Aether's
   event loop, not to either core.

A newtype in the one crate that links both keeps each core unaware of the other. The newtype borrows the
arena, so a cascade or a query is one shared borrow with no allocation per hop.

## Gates

**EYES** (`tools/eyes/run.sh aether`, Chromium oracle). I ran the merged base before any edit, then the swap.
**The gate is GREEN, `scores.json` is byte-identical, and all 18 subject PNGs are byte-identical before and
after.** Mean SSIM is 0.797 and mean mismatch 8.9%.

| case | SSIM before | SSIM after | mismatch % before | after |
|---|---:|---:|---:|---:|
| 01-blog-article | 0.6532 | 0.6532 | 12.04 | 12.04 |
| 02-flex-two-column | 0.7540 | 0.7540 | 8.59 | 8.59 |
| 03-card-gallery | 0.9661 | 0.9661 | 0.87 | 0.87 |
| 04-sticky-nav | 0.7219 | 0.7219 | 6.91 | 6.91 |
| 05-form | 0.7030 | 0.7030 | 4.63 | 4.63 |
| 06-table-zebra | 0.7913 | 0.7913 | 6.19 | 6.19 |
| 07-images-svg | 0.8292 | 0.8292 | 12.02 | 12.02 |
| 08-inline-wrap | 0.7446 | 0.7446 | 8.33 | 8.33 |
| 09-media-queries | 0.9772 | 0.9772 | 1.29 | 1.29 |
| 09-media-queries@375 | 0.9782 | 0.9782 | 1.22 | 1.22 |
| 10-calc-clamp | 0.7182 | 0.7182 | 26.00 | 26.00 |
| 10-calc-clamp@375 | 0.7416 | 0.7416 | 24.60 | 24.60 |
| 11-non-latin | 0.8111 | 0.8111 | 4.10 | 4.10 |
| 12-js-mutation | 0.7726 | 0.7726 | 8.38 | 8.38 |
| 13-ua-defaults | 0.8887 | 0.8887 | 2.58 | 2.58 |
| 14-boxes | 0.5447 | 0.5447 | 14.89 | 14.89 |
| 15-landing | 0.8869 | 0.8869 | 3.56 | 3.56 |
| 16-positioned | 0.8623 | 0.8623 | 13.34 | 13.34 |

The row expected "better where selectors 0.41's pseudo-class gaps lost rules". No EYES page has such a
rule: every selector on the 16 pages compiled under 0.41 or was rescued by the lowering, and every `@media`
query was one the old evaluator understood. Every case is unchanged, which meets the "unchanged or better"
gate. The pages are limited by Aether's layout and paint model, not by the cascade; the computed-value
oracle below shows exactly where. The swap's gains are on paths EYES does not exercise: `:has()`,
`:focus-visible` negations, `nth-*(of S)`, form states, `@layer`, MQ4 range syntax, element-scoped custom
properties, `@supports selector()`, and a correct `:is()` specificity (the lowering charged each branch its
own).

**`cargo test --release -p aether`: 107 passed, 0 failed.** That is 102 lib tests (the old lowering, rule-key,
`split_selector_list` and `media_matches` tests were rewritten as behaviour tests through the real cascade:
escaped utility classes, Selectors 4 matching and §17 specificity, list validity per spec, per-element
custom properties, MQ4), the `dom` selector table extended to css_core's grammar, `dom_oracle` 1,
`style_oracle` 1 and `style_time` 3. **`cargo test --release -p css_core`: green.** The oracle is still
6980/6980 and the stress set 5960/5960, now WITH the rule hash built. The new `m4_rule_index_is_exact`
covers all 27 fixtures, 647 elements, and compares index alone and index plus ancestor filter against the
full scan: identical declarations, order and sort keys. css_core still builds for
`aarch64-unknown-none-softfloat`.

**css_core's 20-property oracle through AETHER's computed values** (`cargo test --release -p aether --test
style_oracle -- --nocapture`). For each of the 18 cases, Chromium's own DOM from css_core's fixture is rebuilt
in Aether (serialized to HTML with a placeholder character where Chromium had text, and parsed by
`aether::dom::parse_html`; element count and order are asserted equal). It then runs through
`layout::build_tree` → `css::apply_stylesheets` → `css::computed_report`, and is compared with Chromium's
`getComputedStyle` (px within 0.01, everything else exact).

**6627/6980 cells agree (94.9%), and 108/349 elements agree on all 20.** css_core's own harness scores
6980/6980 on the same input, so every disagreeing cell is Aether's computed-value model, not the cascade.

| case | cells | | case | cells |
|---|---|---|---|---|
| 01-blog-article | 427/460 | | 09-media-queries@375 | 254/260 |
| 02-flex-two-column | 348/360 | | 10-calc-clamp | 234/240 |
| 03-card-gallery | 755/760 | | 10-calc-clamp@375 | 233/240 |
| 04-sticky-nav | 345/360 | | 11-non-latin | 248/260 |
| 05-form | 480/560 | | 12-js-mutation | 327/340 |
| 06-table-zebra | 742/800 | | 13-ua-defaults | 413/460 |
| 07-images-svg | 407/420 | | 14-boxes | 290/300 |
| 08-inline-wrap | 388/400 | | 15-landing | 232/240 |
| 09-media-queries | 254/260 | | 16-positioned | 250/260 |

Per property over 349 elements:

| at 100% | high (≥ 98%) | lower |
|---|---|---|
| font-weight, font-style, visibility, flex-direction (349) | color 348, text-decoration-line 347, padding-top 347, position 345, line-height 345, background-color 343, list-style-type 343, white-space 342, border-top-width 342 | border-top-style 340, text-align 337, border-left-color 333, font-size 323, margin-top 303, **display 265**, **font-family 228** |

`STYLE_ORACLE_VERBOSE=1` prints every cell. The misses, which are AETHERINLINE's and the paint lane's work
list:

- **display**: no `list-item`, `table-*` or `inline-block`. Author `display: block` on an inline tag stays
  inline, because the box tree decides inline-ness by tag at build time.
- **font-family**: Aether has three classes (sans/serif/mono), where Chromium returns the author's list,
  and Aether's default face is sans where Chromium's is Times.
- **margin-top**: Aether's UA margins are approximations (`p` 8px vs 1em, `h2` 12px vs 0.83em).
- **Form controls**: 13.333px Arial, inset borders, Field background.
- **Others**: `pre` has no `white-space: pre`; there is no `relative` position (static and relative are
  one); there are no list markers.

The test gates on this floor (6627).

**Style time** (`cargo test --release -p aether --test style_time -- --nocapture`). The figure is the median
of 5 runs of `css::apply_stylesheets` from parse to the folded box styles. Re-layout is reported separately.
The machine was shared, at load average about 20, so expect ±30%.

| page | servo selectors (merged base) | css_core, full scan | css_core + rule hash + ancestor filter |
|---|---:|---:|---:|
| EYES 01-blog-article (23 elements, 443 B CSS) | 0.05 ms | — | 0.12 ms |
| Wikipedia article (9,887 elements, its inline `<style>`s) | 16.2 ms | 11.7 ms | 8.3 ms |
| Wikipedia article + Bootstrap 5.3.3 (2,047 selector entries, 285 KB) | 71.5 ms | **799.8 ms** | **60.8 ms** |

- The hash took css_core from 0.8 s to 61 ms on the Wikipedia-sized page, which is now faster than the old
  path.
- The old path had its own key hash plus a "class must exist on the page" pre-filter, and it silently
  dropped every rule its selector engine could not compile.
- On the big page, about 18 ms of the 61 ms is css_core parsing 285 KB, and css_core's `cascade_filtered`
  is about 15 ms across all 9,887 elements.
- Index shape for Bootstrap: 1,882 entries in 1,247 class buckets, 104 in 51 type buckets, 15 in 5
  attribute buckets, and only 46 universal.
- `rule_index_exact_on_wikipedia` re-proves exactness on the real page: index plus filter equals the full
  scan on all 9,887 elements.
- EYES 01's 0.05 → 0.12 ms is noise-level on a 23-element page.

Vectors (`handlers/aether/tests/vectors.txt`) are fetched at test time, sha256-pinned, and skipped offline
(`AETHER_OFFLINE`):

- Mozilla Readability's saved English Wikipedia article `wikipedia-2` (tag 0.6.0), ~9.9k elements.
- Bootstrap 5.3.3's dist CSS, standing in for the Vector skin CSS. The article linked the skin through
  `load.php`, and `en.wikipedia.org` (like `web.archive.org` and `cdn.jsdelivr.net`) is refused by this
  sandbox's egress policy.

The article's own inline sheets are XHTML-serialized inside `<![CDATA[`. Per CSS Syntax that makes each
sheet one invalid rule, as it would in any browser, so that row measures the per-element walk with almost
no rules.

## Third-party crates

None is left in Aether's style path. `css_core` (in-tree, zero dependencies) does parsing, matching,
`@media`, `@supports`, `@layer`, the cascade, `var()` and `calc()`. The dev-dependency `crypto_core` is
in-tree. Aether's remaining third-party crates are not this lane's: `taffy` 0.14 (layout, AETHERINLINE),
`boa`, `font-kit`, `resvg`, `image`, `reqwest` and so on.

## Honest ceiling

- **EYES shows no visible gain.** The pages never hit the selectors the old engine lost. The gain is
  correctness on real-world CSS EYES lacks, and the visible ceiling is the computed-value model above.
- **Spec-correct rule dropping.** A selector list with an invalid member, such as an unknown vendor
  pseudo-class like `:-moz-ui-invalid`, now drops the whole rule, as Chromium does. The old lowering kept
  the siblings and so over-applied. `:-webkit-any()`, which the old lowering rewrote, is not a css_core
  pseudo-class and is owed (Chromium still accepts it).
- **No invalidation.** A JS mutation re-runs the whole cascade on the next layout, as before.
- **No `:hover`/`:focus`/`:target`.** The event loop does not feed states into `El::state`.
- **Not consumed.** `@font-face`, `@keyframes` and `@import` are parsed into data but not used (they are
  ledgered as `at-rule:@…`), and `revert`/`revert-layer` reach `apply_declaration` as keywords it ignores.
- **Two `var()` paths.** The layout builder's build-time inline-style pass still uses the string `var()`
  resolver, now fed the document element's computed custom properties. The cascade re-applies every inline
  style with per-element `var()` anyway.
- **Report fidelity.** `computed_report` reports Aether's model honestly. Where Aether has no notion, it
  reports what Aether effectively does: solid borders, `disc`, the three family classes.
- **Ancestor filter.** It is per-element (a 32-byte copy per element, kept in a map), not a push/pop stack.

## Owed

1. **AETHERINLINE / paint** (from the oracle above): `display` (list-item, table-*, inline-block, and
   author `display` deciding inline-ness), font-family lists, UA margins in em, form-control defaults,
   `white-space: pre`, `position: relative`, list markers. Port css_core's `tests/common/style.rs` UA sheet
   into Aether as an author-origin-0 sheet when the computed-value model can consume it.
2. `getComputedStyle` in JS still reads only the inline style. It should read `css::computed_report`.
3. Interaction states into `El::state` (`:hover`/`:focus`/`:focus-visible`) from the event loop, with
   restyle on change. Style invalidation instead of a full re-cascade.
4. `:-webkit-any()` in css_core. `@font-face` → FONTCORE; `@keyframes` → animation.

## How a future executor continues

Run:

- `cargo test --release -p aether`, for the units, `dom_oracle`, `style_oracle` (prints per-page and
  per-property agreement; `STYLE_ORACLE_VERBOSE=1` for every cell) and `style_time` (fetches two vectors;
  `AETHER_NO_RULE_INDEX=1` for the full scan; `AETHER_STYLE_PROFILE=1` for phases).
- `cargo test --release -p css_core`, where the oracle runs with the index and `m4_rule_index_is_exact`
  guards the hash.
- `tools/eyes/run.sh aether`, the gate.

The cascade is `css::apply_stylesheets` → `cascade_document`. The computed-value step is `apply_declaration`.
The matcher seam is `dom::El`. When Aether's computed-value model grows, raise `style_oracle`'s floor.
