# CSSCORE (LEDGER SR47): CSS from the specifications — `unaos/libs/web/css_core`

Branch `exec-web-csscore`, cut at a098002a. Commits: M1 f88df3a8 (Syntax), M2 70972ce6 (Selectors), M3 de6b9777
(cascade helpers + Chromium oracle), M4 this doc. Standalone: nothing in `handlers/aether` was touched and nothing
depends on `html_core` (SR46, building in parallel); the matcher works over its own `Element` trait.

## Finding

Aether parses CSS with `cssparser` and matches selectors with `selectors` (SR31 lists both as chicken wire, owed
together). `css_core` replaces both from the specifications: `#![no_std]` + `alloc`, `#![forbid(unsafe_code)]`,
**zero dependencies** (its only dev-dependency is UnaOS's own `crypto_core`, used to verify fetched vector files).
It builds for `aarch64-unknown-none-softfloat` as well as the host. A root-workspace member; `cargo test --release
-p css_core` runs every proof below.

Every stage is judged by an oracle: css-parsing-tests for Syntax, WPT's expectations for Selectors, and Chromium
141's `getComputedStyle` for the cascade.

## Spec sections covered

| module | specification | what |
|---|---|---|
| `tokenizer.rs` | CSS Syntax 3 §3.3, §4 | preprocessing; every token type incl. numbers (exact decimal → f64), dimensions, percentages, `url()` / bad-url, strings / bad-string, escapes, hash id/unrestricted, CDO/CDC, the 2014 match tokens (`~=` `|=` `^=` `$=` `*=`, kept because css-parsing-tests distinguish them); spans for `<urange>` |
| `parser.rs` | Syntax 3 §5 | component values, simple blocks, functions; "parse a stylesheet / list of rules / rule / declaration / list of component values / component value"; a block's contents (declarations + at-rules + nested qualified rules, CSS Nesting's re-try rule); `!important`; all error recovery |
| `anb.rs`, `urange.rs` | Syntax 3 §6, §7.1 | An+B over tokens (every `n-dimension` / `ndashdigit` form); `<urange>` via the token grammar + source text |
| `selectors.rs` | Selectors 4 §3–§16, §17 | type / universal with `ns|`, `*|`, `|`; `#id`, `.class`; attribute selectors, all six operators, `i` / `s`; `:not()` (complex list), `:is()` / `:where()` (forgiving), `:has()` (relative, not nestable); `:nth-child(An+B of S)`, `:nth-last-child`, `:nth-of-type`, `:nth-last-of-type`, first/last/only-child/-of-type; `:root` `:empty` `:scope`; `:hover` `:active` `:focus` `:focus-visible` `:focus-within` `:link` `:visited` `:any-link` `:target` `:checked` `:indeterminate` `:default` `:enabled` `:disabled` `:required` `:optional` `:read-only` `:read-write` `:placeholder-shown` `:valid` `:invalid` `:in-range` `:out-of-range` `:defined` `:open` `:autofill` as state hooks; `:lang()` (RFC 4647 extended filtering), `:dir()`; pseudo-elements `::before` `::after` `::first-line` `::first-letter` `::marker` `::placeholder` `::selection` `::backdrop` `::file-selector-button` `::slotted()` `::part()` `::highlight()` and the one-colon legacy forms; combinators ` ` `>` `+` `~`; relative selectors; CSS Nesting `&` (desugared to `:is(parent)`) and implicit-`&` nested selectors; specificity |
| `matching.rs` | Selectors 4, HTML §4.16 | the `Element` trait; `html_state` (HTML's attribute-derived states, incl. fieldset/optgroup disabling); HTML's case-insensitive attribute list; right-to-left matching with backtracking, `:has()` anchoring, `query_all` |
| `media.rs` | Media Queries 4 §2–§4 | `<media-query-list>` incl. range syntax (`400px <= width < 700px`), `not` / `only` / `and` / `or`, `<general-enclosed>`; three-valued evaluation against an `Environment` (width/height/aspect-ratio/orientation/resolution/color/monochrome/hover/pointer/prefers-*/scripting/update/overflow-*/display-mode/color-gamut/dynamic-range) |
| `values.rs` | Values 4 §6–§10, css-variables 1 | absolute + font + viewport lengths; angle/time/frequency/resolution; `calc()` `min()` `max()` `clamp()` `abs()` `sign()` `round()` `mod()` `rem()` as trees, resolved to number / px+% / dimension with type checking; `var()` substitution with fallbacks; custom-property computation with cycle detection (guaranteed-invalid) |
| `color.rs` | Color 4 (sRGB) | 148 named colors, `transparent`, `currentcolor`, system colors (values probed from Chromium 141), 3/4/6/8-digit hex, `rgb()` `rgba()` `hsl()` `hsla()` `hwb()` legacy and modern (`/ alpha`, `none`, `calc()` channels); Chromium's serialization |
| `stylesheet.rs` | Syntax 3, Cascade 5, Nesting, Conditional 3, Fonts 4, Animations 1 | rules as data: style rules with nesting (nested style rules, nested conditional rules, "nested declarations" rules), `@media`, `@supports` (incl. `selector()`), `@layer` (statement + block, dotted names), `@import` (url, `layer()`, `supports()`, media; placement rules), `@namespace`, `@font-face` (`family()`, `sources()`), `@keyframes` (offsets, `!important` dropped); others kept as syntax |
| `cascade.rs` | Cascade 5 §6 | `RuleSet` per environment (conditionals resolved, imports through a resolver, layer order registered on first sight, sub-layers before the parent's own rules); `cascade()` sorts by origin+importance, element-attached (style attribute), layer (reversed for `!important`), specificity (the best matching selector of a rule), order |
| `serialize.rs` | CSSOM §2.1 | identifiers, strings, component values back to text |

## Oracles and KATs (all in `cargo test --release -p css_core`)

| test | oracle | result |
|---|---|---|
| `syntax.rs` | css-parsing-tests (servo/rust-cssparser's maintained copy @106021c7), pinned by URL + sha256 in `tests/vectors.txt`, fetched at test time, skipped offline | component_value_list 44/44, one_component_value 10/10, declaration_list 9/9, one_declaration 22/22, rule_list 16/16, one_rule 14/14, stylesheet 18/18, An+B 128/128, urange 12/12 — **273/273** |
| `cascade.rs` colors | css-parsing-tests color files, same pinning | color3 233/233, color3_keywords 801/801, color3_hsl 15552/15552, color4_hwb 7776/7776 — **24362/24362** |
| `selectors.rs` | WPT @564b9b1e, expectations from the WPT files; Chromium used only to parse the markup into a JSON tree (`tools/wpt-fixtures.mjs` → `tests/wpt/*.json`) | Selectors-API (`dom/nodes/ParentNode-querySelector-All`, HTML): invalid 34/34, valid **595/595** over the Document, In-document Element (after the data-clone copy is appended) and Detached Element contexts; css/selectors: has-basic 18/18, has-relative-argument 35/35, is-where-basic 15/15, is-where-not 18/18, not-complex 20/20, attribute-selectors/attribute-case/semantics 165/165 — **900/900 incl. invalid**; plus §17 specificity examples, child-indexed-pseudo-class, nth-child-of-has |
| `cascade.rs` | spec examples | MQ4 56 queries × 2 viewports, math functions 40, custom properties 15, stylesheet data 22, cascade order 12 |
| `oracle.rs` EYES | **Chromium 141** on EYES's 18 Aether cases (16 pages from `exec-aether-see:tools/eyes/suites/aether`, read with `git show`, two at 375×812) | see below — **6980/6980 cells, 349/349 elements, 18/18 pages at 100%** |
| `oracle.rs` stress | same oracle on CSSCORE's own pages (`tests/oracle-pages/`) + WPT's style-based selector tests | **5960/5960 cells, 298/298 elements** |

### The Chromium oracle method

`tools/oracle.mjs` loads each page in Chromium at the case's viewport, lets its scripts run, and records (a) the DOM
as a JSON element tree (Chromium's own, so `12-js-mutation`'s script output is included and only the CSS side is
judged), (b) every `<style>` element's text in document order, (c) `getComputedStyle()` of 20 properties for every
element: `display position color background-color font-size font-weight font-style font-family line-height
text-align text-decoration-line white-space visibility list-style-type border-top-width border-top-style
border-left-color padding-top margin-top flex-direction`. They were chosen to exercise the cascade, inheritance,
`@media`, `calc()`/`clamp()`, `var()`, colors, keywords and shorthands, and to avoid properties whose resolved value
is a layout result (width, height). `tests/oracle.rs` runs css_core over (a)+(b) plus the style attributes, with
the media `Environment` at the case's viewport, and compares (px within 0.01, everything else exact).

The computed-value step is **test code** (`tests/common/style.rs`), because the style system belongs to Aether:
the UA sheet (HTML §15 Rendering, written with the values Chromium's html.css uses), shorthand expansion for the
shorthands the pages use, inheritance, blockification, the Blink keyword-font-size rule (`monospace` → 13px for
keyword-derived sizes, kept through `em` / `%`), border-width snapping, and LayoutTheme's padding reset on native
checkboxes and radios. css_core supplies everything else.

| EYES case | cells | elements | | EYES case | cells | elements |
|---|---|---|---|---|---|---|
| 01-blog-article | 460/460 | 23/23 | | 09-media-queries@375 | 260/260 | 13/13 |
| 02-flex-two-column | 360/360 | 18/18 | | 10-calc-clamp | 240/240 | 12/12 |
| 03-card-gallery | 760/760 | 38/38 | | 10-calc-clamp@375 | 240/240 | 12/12 |
| 04-sticky-nav | 360/360 | 18/18 | | 11-non-latin | 260/260 | 13/13 |
| 05-form | 560/560 | 28/28 | | 12-js-mutation | 340/340 | 17/17 |
| 06-table-zebra | 800/800 | 40/40 | | 13-ua-defaults | 460/460 | 23/23 |
| 07-images-svg | 420/420 | 21/21 | | 14-boxes | 300/300 | 15/15 |
| 08-inline-wrap | 400/400 | 20/20 | | 15-landing | 240/240 | 12/12 |
| 09-media-queries | 260/260 | 13/13 | | 16-positioned | 260/260 | 13/13 |

Stress set: s1-cascade 680/680 and @375 680/680 (layers incl. `!important` reversal, inline vs `!important`,
`:where`/`:is` specificity, nesting incl. `.v &` and nested `@media`, `@supports` incl. `selector()`, MQ4 ranges,
`or`, unknown features); s2-values 1680/1680 (`var()` fallbacks / cycles / invalid-at-computed-time, calc trees,
font-size keywords / `larger` / `%` / `rem`, line-height inheritance, `bolder` / `lighter`, hex-alpha / modern
rgb / hwb / hsla, border shorthands and snapping, `font` shorthand, two-value `display`, blockification,
`inherit` / `initial` / `unset`); s3-selectors 1340/1340 (attribute operators and flags, HTML's case-insensitive
`type`, `:nth-child(even of .k)`, `:has()` forms, `:lang` / `:dir`, form states incl. fieldset disabling, `:link`,
`@namespace svg`); WPT is-nested 380/380, is-specificity 320/320, has-specificity 320/320, not-specificity 220/220,
is-where-pseudo-classes 340/340.

How the agreement got to 100%, for the record: the first EYES run agreed on 6954/6980 cells. The misses were
`body` missing from the UA sheet's block list, and a non-inherited property given an explicit `inherit` falling back
to its initial value. Both were fixed in the harness. The first stress run agreed on 5949/5960. The misses were
Chromium's disabled-control colours (added to the UA sheet from html.css), the checkbox padding reset (LayoutTheme),
and `hwb()` channel rounding at an exact .5, which was a css_core fix in `Rgba::serialize`.

## Third-party crates

None in `css_core`. The dev-dependency `crypto_core` is in-tree. The generators (`tools/*.mjs`) run Node with the
pre-installed Playwright and Chromium. They are oracle tooling, are not linked, and are not chicken wire.

## Ceiling (what is NOT done)

- **Syntax**: ident code points follow the pre-2023 definition (any non-ASCII), which css-parsing-tests still
  expect. There are no source positions on errors (devtools will need them). The `@charset` conventions of the
  reference harness live in the test, not the core.
- **Selectors**: no quirks mode (case-insensitive class/id), `:visited` never matches (by design), no shadow DOM
  (`:host`, `:host-context`, `::slotted`/`::part` parse but never match), no column combinator `||`,
  `:popover-open` / `:modal` / `:fullscreen` / `:state()` are not implemented, `:dir()` reads the `dir` attribute
  only (no `auto` / bidi), `:lang()` has no Content-Language fallback. **No rule hashing / bloom filter and no
  invalidation**: every rule is tested against every element (fine for the corpus, too slow for wikipedia-scale
  pages).
- **Media / conditionals**: no `calc()` inside media feature values, the `device-*` features alias the viewport,
  and `@container`, `@scope`, `@starting-style`, `@property` (registered custom properties), `@counter-style` and
  `@page` are kept as syntax only.
- **Values**: `ex` / `ch` / `cap` / `lh` are fixed ratios of `em` (font metrics belong to the shaper). Trig,
  `pow` / `sqrt` / `hypot` / `log` / `exp`, `progress()` and `calc-size()` are not implemented, nor typed `attr()`.
  Colors stop at sRGB: no `lab()` `lch()` `oklab()` `oklch()` `color()` `color-mix()`, relative colors or
  `light-dark()`.
- **Cascade**: there are no animation or transition origins. `revert` / `revert-layer` resolution belongs to the
  style system (the harness approximates it as `unset`). There is no `@scope` proximity and no shadow-tree
  encapsulation contexts. `@import` cycles are the resolver's responsibility.
- **Oracle scope**: 20 properties, and percentages of padding/margin are not resolved (they need layout). The
  computed-value layer is harness code, so it does not prove Aether's own style system.

## Owed

1. **The Aether swap** (`cssparser` + `selectors` → `css_core`), with EYES as its gate. It is the ledger's
   follow-on, and the move should port `tests/common/style.rs`'s UA sheet and computed-value rules into Aether's
   style system rather than duplicate them.
2. **html_core's DOM implements `matching::Element`** once SR46 lands. The trait needs element names and
   namespaces, attributes, parent and element siblings, `is_empty`, and the states. `html_state` already derives
   HTML's attribute-based states.
3. Rule hashing (by id / class / type of the rightmost compound) plus an ancestor bloom filter, so the cascade
   scales to real pages.
4. The items under Ceiling, in roughly this order of page impact: lab/oklch colors and `color-mix()`, `@container`,
   trig math, `@property`, shadow DOM selectors.

No new handler: CSS is Aether's domain (CODEX §2), and css_core is the library Aether will link.

## How a future executor continues

`cargo test --release -p css_core` (set `CSS_OFFLINE=1` to skip the fetched files, and `ORACLE_VERBOSE=1` to print
every disagreeing cell). To regenerate the oracle fixtures after a Chromium bump or new EYES pages, run
`cd unaos/libs/web/css_core && node tools/oracle.mjs [ref]`. For the WPT fixtures, run `node tools/wpt-fixtures.mjs`.
To widen the oracle, add a property to `PROPS` in both `tools/oracle.mjs` and `tests/common/style.rs`, regenerate,
and teach `Styler::compute` its computed value. New stress cases go in `tests/oracle-pages/` and the `stress` list
in `tools/oracle.mjs`. Both oracle tests assert 100%, so a regression goes red.
