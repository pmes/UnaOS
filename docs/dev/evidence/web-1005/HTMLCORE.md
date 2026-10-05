# HTMLCORE (LEDGER SR46): UnaOS's own HTML parser

Branch `exec-web-htmlcore`, cut at `5f042713`. Crate `unaos/libs/web/html_core`: `#![no_std]` + `alloc`,
`#![forbid(unsafe_code)]`, **zero dependencies and zero dev-dependencies**. It is a member of the root
workspace, and it also type-checks for `aarch64-unknown-none-softfloat`. `handlers/aether` was not touched.
The `kuchiki` → `html_core` swap is the follow-on arc, gated by EYES.

## Finding

Aether parses HTML with `html5ever` and keeps its DOM in `kuchiki`. `kuchiki` has been unmaintained since 2020,
and it pulls a second, 2020-era copy of html5ever, cssparser and selectors into the tree (EYES SR23, DEPS SR31).
Both are chicken wire in the sense of R83: they do work that UnaOS says is its own. HTMLCORE replaces that work
with code written from the WHATWG HTML Living Standard, and every stage is checked against an oracle.

## Spec coverage (WHATWG HTML, source of 2026-10-05)

| section | what | file |
|---|---|---|
| §13.2.3.5 | input preprocessing (CR/CRLF → LF) | `src/tokenizer.rs` `preprocess` |
| §13.2.5.1–80 | **every tokenizer state**: data/RCDATA/RAWTEXT/script data (escaped, double-escaped)/PLAINTEXT, tags, attributes (duplicates dropped), comments (incl. the `<!--` nesting states), DOCTYPE (public/system, force-quirks), CDATA (only in foreign content), character references (named: longest match over the generated table, with the attribute legacy rule; numeric: the C1 remap table, surrogate/out-of-range/null → U+FFFD) | `src/tokenizer.rs` |
| 2026 PI states | `<?target data>` processing instructions (open, target, after-target, data, questionable) behind `TokenizerOpts::processing_instructions` (**off** by default, because shipping Chromium and the tree-construction corpus predate them) | `src/tokenizer.rs` |
| §13.5 | named character references: **generated data** (`src/entities.rs`, 2231 names) from the spec's entities.json by `tools/gen_entities.py` | `src/entities.rs` |
| §13.2.4 | insertion modes, the stack of open elements (default/list-item/button/table scope; `select` is in the default scope list under the relaxed-select parser), the list of active formatting elements (Noah's Ark ×3, reconstruct, clear to marker), head/form pointers, frameset-ok, reset-the-insertion-mode | `src/tree_builder.rs` |
| §13.2.6.1 | appropriate place for inserting (foster parenting, template contents, fragment root target), create element for token, insert character/comment/PI, SVG/MathML attribute adjustment, foreign attribute namespacing | `src/tree_builder.rs` |
| §13.2.6.4.1–23 | **all 21 insertion modes** (initial with the full quirks/limited-quirks DOCTYPE tables, … after after frameset). The relaxed `<select>` parser is implemented (no "in select" modes; `select`/`option`/`optgroup`/`hr`/`input` rules in "in body") | `src/tree_builder.rs` |
| §13.2.6.4.7 | the adoption agency algorithm (outer loop 8, inner-loop AFE removal past 3, bookmark, furthest block) | `adoption_agency` |
| §13.2.6.5 | foreign content: breakout list, `font` with color/face/size, SVG tag-name fixes, MathML text and HTML integration points (`annotation-xml` encoding), self-closing foreign elements | `foreign` |
| §4.10.x | the parser-driven `option` → `selectedcontent` cloning (option popped → update descendant selectedcontent; selectedness setting algorithm; list of options) | `tree_builder.rs` (select side effects) |
| §13.4 | fragment parsing (context element, tokenizer state by context, root insertion target, template context, form pointer) | `parse_fragment` |
| §13.3 | serialization: void set, attribute names by namespace, escaping (`&`, NBSP, `<`, `>` always; `"` in attributes), raw-text parents, noscript per the scripting flag, template contents, PI, doctype. It runs **iteratively**, so tree depth is unbounded | `src/serialize.rs` |
| DOM | arena (`NodeId`), Document/DocumentFragment/Doctype/Element(ns, local, attrs)/Text/Comment/PI; parent/child/sibling links; append/insert_before/detach/reparent/clone_subtree; hooks for a later CSSCORE: `parent_element`, `prev/next_sibling_element`, `element_children`, `element_id`, `has_class`, `query_first`, `query_all`, `descendants`, `text_content` | `src/dom.rs` |

The scripting flag is a parameter. UnaOS parses with scripting **off**, so `<noscript>` content is parsed as
markup. The html5lib `#script-off` vectors and the Chromium oracle run the same way.

## Oracles and known-answer tests (`cargo test --release -p html_core`, all green)

The vectors are fetched at test time. Their URLs and sha256 sums are in `unaos/libs/web/html_core/vectors.txt`,
and the files are cached in `target/vectors-cache/html_core/`. Each test skips, saying why, when it cannot fetch.
Two html5lib-tests commits are pinned. **9329e64** is the last commit that has `tree-construction/`; after it
the tree tests moved to WPT. **c777c40** (2026-09-29) adds the processing-instruction tokenizer tests.

**M1: tokenizer (`tests/m1_tokenizer.rs`).** Output tokens are compared exactly (characters coalesced,
attributes compared as a map). Parse-error codes are not compared.

| file | PI off (9329e64) | PI on (c777c40) |
|---|---|---|
| contentModelFlags | 24/24 | 24/24 |
| domjs | 59/59 | 59/59 |
| entities | 80/80 | 80/80 |
| escapeFlag | 9/9 | 9/9 |
| namedEntities | 4210/4210 | 4210/4210 |
| numericEntities | 336/336 | 336/336 |
| pendingSpecChanges | 1/1 | 1/1 |
| test1 | 69/69 | 69/69 |
| test2 | 45/45 | 46/46 |
| test3 | 1786/1786 | 1802/1802 |
| test4 | 85/85 | 85/85 |
| unicodeChars | 323/323 | 323/323 |
| unicodeCharsProblematic | 1/1 (+4 unrepresentable) | 1/1 (+4) |
| xmlViolation | 0/0 (+4 unrepresentable) | 0/0 (+4) |
| **total** | **7028/7028** | **7045/7045** |

"Unrepresentable" covers two cases. Four vectors feed a lone UTF-16 surrogate, which a Rust `&str` cannot hold.
The four xmlViolation vectors expect the §13.2.7 XML-infoset coercions, which a browser parser does not apply.

**M2: tree construction + DOM (`tests/m2_tree.rs`, `tests/m2_dom.rs`).** Scripting is off, and the html5lib
dump format is compared exactly. The result is **1784/1784** over 57 files, with 8 `#script-on` vectors skipped.
1792 is the total vector count. **No failures.** Every file passes in full. Some examples: adoption01 18/18,
template 112/112, tests1 112/112, tests16 191/191 (+6 script-on), tests19 103/103, tests_innerHTML_1 81/81,
foreign-fragment 66/66, webkit02 48/48 (+1). The 4 webkit02 `<selectedcontent>` vectors need the option-pop
cloning side effect, and they pass with it. DOM navigation, template contents and fragment-in-context each have
their own unit tests.

**M3: serializer + Chromium oracle + fuzz (`tests/m3_oracle.rs`, `tests/m3_fuzz.rs`).**

- **§13.3 known answers:** escaping in text and attribute mode, void and legacy-void elements, raw-text parents,
  noscript with scripting off, template contents, namespaced attributes with SVG case fixing, the leading LF in
  pre, the doctype, and a 200 000-deep tree serialized without recursion.
- **EYES corpus oracle.** EYES has 16 pages. Its 18 cases are those 16 documents, two of them rendered at a
  second viewport, which does not change the parse. Each page is copied verbatim from `exec-aether-see@b530ccd0`
  into `tests/data/eyes/`. Chromium (Playwright 1.56.1, chromium-1194, `javaScriptEnabled: false`) loads it from
  `file://`. `tests/oracle/outer_html.cjs` writes `document.documentElement.outerHTML` to `*.chromium.html`.
  The result is **16/16 byte-equal after whitespace normalization, and 16/16 raw byte-equal**:

  | page | bytes | normalized | raw |
  |---|---|---|---|
  | 01-blog-article | 1350 | equal | equal |
  | 02-flex-two-column | 1205 | equal | equal |
  | 03-card-gallery | 1508 | equal | equal |
  | 04-sticky-nav | 1229 | equal | equal |
  | 05-form | 1313 | equal | equal |
  | 06-table-zebra | 1141 | equal | equal |
  | 07-images-svg | 3830 | equal | equal |
  | 08-inline-wrap | 928 | equal | equal |
  | 09-media-queries | 951 | equal | equal |
  | 10-calc-clamp | 848 | equal | equal |
  | 11-non-latin | 634 | equal | equal |
  | 12-js-mutation | 860 | equal | equal |
  | 13-ua-defaults | 422 | equal | equal |
  | 14-boxes | 985 | equal | equal |
  | 15-landing | 1022 | equal | equal |
  | 16-positioned | 1156 | equal | equal |

- **A second, larger Chromium oracle (live, skips without node or Playwright).** Every html5lib document input
  (1592 non-fragment, non-script-on vectors) goes through Chromium's `DOMParser`, which builds a
  scripting-disabled document. Its `outerHTML` is compared with ours. **1589/1592 are byte-equal.** In the other
  3, Chromium departs from the spec, and the html5lib vector agrees with us:
  - `noscript01.dat#12`: Chromium closes `<noscript>` at an ignored `<head>`.
  - `tests25.dat#7`: Chromium closes the obsolete `<command>` immediately.
  - `webkit02.dat#15`: Chromium's adoption agency also reopens `<em>` inside `<aside>`.

  The test allows exactly these three and fails on any other disagreement.
- **Fuzz.** The test takes 1000 mutants of the 1792 html5lib inputs plus the 16 EYES pages. Mutations are
  deletes, duplicates, swaps, random code points and a dictionary of hostile snippets (tables, SVG/MathML,
  templates, select, frameset, CDATA, char refs, NUL). It uses xorshift64* with a fixed seed. Each mutant goes
  through the document parser (PI on for a quarter of them), a check that every arena link is consistent, the
  serializer, a re-parse of the output, and the fragment parser in a rotating context (body, td, select,
  template, textarea, svg, mi, html). The result is **0 panics**.

## Third-party crates

There are none: no runtime dependencies and no dev-dependencies. The test harness has its own small JSON reader,
and it shells out to `curl` and `sha256sum` to fetch vectors. The oracle uses the container's preinstalled
Playwright and Chromium, which are an external tool, not a crate. R83 is satisfied trivially.

## Honest ceiling (not done)

- **Parse errors are recognised but not reported.** They are only counted, with no codes and no line/column, so
  the `#errors` sections of html5lib are not checked.
- **Encoding.** The input is an already-decoded `&str`. §13.2.3 byte-stream sniffing (BOM, `<meta charset>`
  prescan, change-the-encoding) is not implemented. That belongs to the caller, or to a future
  `encoding_core`.
- **Scripting on.** The flag works for `<noscript>` and fragment tokenizer states, but there is no script
  execution, `document.write` or parser pause. The 8 `#script-on` vectors are skipped, as SR46 scopes it.
- **The DOM is parser-shaped, not a full DOM:**
  - no form-owner association;
  - no custom elements;
  - no declarative shadow roots (`<template shadowrootmode>` stays a template, since "allow declarative shadow
    roots" is false);
  - not the 2026 `<template for>` content patching;
  - none of the `selectedcontent` moving/removing microtask steps;
  - no mutation events.
- **Processing instructions** (2026 spec) are implemented and proven in the tokenizer. Tree construction inserts
  PI nodes and the serializer writes `<?t d?>`. They stay off by default until the tree-construction corpus
  (now in WPT) and Chromium follow.
- **Depth cost.** Scope checks are O(stack depth) per tag, which is what the spec says. A pathological
  200 000-deep *parse* is therefore quadratic. A 5 000-deep parse is instant, and serialization is iterative
  and linear. Chromium caps parser nesting depth, and this crate does not.
- **Tokenizer interface.** The tokenizer emits one `Character` token per code point. That is correct and fast
  enough for the corpus. Runs of characters are an optimisation still owed.

## Owed

1. **The Aether swap**, the follow-on arc: replace `kuchiki`/`html5ever` in `handlers/aether` with `html_core`
   (an adapter from `dom::Document` to Aether's layout/style tree walk). EYES is the gate, and its scores must
   not regress. CSSCORE (selectors) can build on the `query_*` and sibling/parent hooks.
2. Parse-error reporting (codes plus positions), then the `#errors` comparison.
3. An encoding sniffer feeding `parse_document` from bytes.
4. Moving the tree-construction KATs to the WPT copy (`html/syntax/parsing`) once it diverges from the
   9329e64 pin.

## How a future executor continues

- Run `cargo test --release -p html_core` from the repo root. It needs network access to fetch the vectors and
  skips without it.
- Regenerate the Chromium goldens with
  `NODE_PATH=$(npm root -g) PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers node unaos/libs/web/html_core/tests/oracle/outer_html.cjs unaos/libs/web/html_core/tests/data/eyes`.
- Regenerate the entity table with `python3 unaos/libs/web/html_core/tools/gen_entities.py entities.json > unaos/libs/web/html_core/src/entities.rs`.
- Entry points are `html_core::parse_document(&str, ParseOpts)`, `html_core::parse_fragment(...)`,
  `serialize::{outer_html, inner_html}` and `dom::Document`.
