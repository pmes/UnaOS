# AETHERDOM (LEDGER SR49): Aether's document model is html_core

Branch `exec-host-aetherdom`, cut at `eb429296`, then merged `exec-host-aethervideo2@107efeb2` (the boot-23 Aether:
EYES's layout refactor + harness, AETHERVIDEO, DEPS's bumps) and `exec-web-htmlcore@e3a58572` (`html_core`). Lane:
DOM/parse only. AETHERCSS (SR41) is live on layout/paint, so those files changed by a path line or a comment.

## Finding

Aether parsed with `html5ever` and kept its DOM in `kuchiki` 0.8 (2020, unmaintained). kuchiki pulled a second,
2020-era generation of html5ever 0.25 / markup5ever 0.10 / cssparser 0.27 / selectors 0.22 / string_cache /
tendril / phf 0.8 into the lock. Aether's CSS matching ran on kuchiki's selectors 0.22, even though the crate
already declared selectors 0.41 and never used it. HTMLCORE (SR46) is spec-complete and byte-equal to Chromium on
every EYES page. This arc puts Aether on it.

## What changed

| file | change |
|---|---|
| `handlers/aether/src/dom/mod.rs` | **the `dom` module**. A document is one `html_core::Document` arena behind `Rc<RefCell<…>>`, and a `NodeRef` is that arena plus a `NodeId` (equality is identity, hashable). The API keeps the names and shapes callers already used: `as_element()` → `ElementData { name: { ns, local }, attributes, template_contents }`, `attributes.borrow()/borrow_mut()` (`get/contains/insert/remove`, `map.iter()` in source order), `as_text()/as_comment()` (live `borrow()/borrow_mut()`), `as_doctype()`, `as_document()`, `data()`, `parent/first_child/last_child/next_sibling/previous_sibling`, `children/descendants/inclusive_descendants/ancestors` (snapshots, so mutating while iterating is safe), `text_contents`, `select/select_first`, `append/prepend/insert_before/insert_after/detach`, `new_text/new_comment/new_element` (created in the node's own arena), `clone_node(deep)`, `set_inner_html`/`parse_fragment_here` (§13.4), `inner_html/outer_html`, and `Display` = the §13.3 serialization. Cross-document insertion deep-imports the node. Inserting an ancestor or the Document is refused, which is the DOM's HierarchyRequestError. |
| `handlers/aether/src/dom/select.rs` | servo `selectors` **0.41** driven over the arena through its `Element` impl. The `El` handle walks the matcher without allocating. Quirks mode comes from the document. The pseudo-class set is the same one kuchiki parsed (`:any-link :link :visited :active :focus :hover :enabled :disabled :checked :indeterminate`), and only the link pair matches. There are no pseudo-elements and no `:is/:where/:has`. That keeps the set of compiling rules, and with it the cascade's lowering and ledger, unchanged. |
| `js/mod.rs` | `innerHTML` set is a context fragment parse (`set_inner_html`). Before, it was a whole-document parse with the `<body>` children moved over, which mis-parsed `<tr>`/`<td>`/`<option>` contexts. `innerHTML` get uses `inner_html()`. `createElement*`/`createTextNode`/`createComment` create nodes in the live arena. Before, a QualName was cloned off `<html>` to get around a cross-version html5ever type clash. `cloneNode` is a real DOM clone. Before, it reserialized, reparsed and took the first body child, which lost `<td>`, `<tr>`, `<html>` and anything else the body context refuses. |
| `api/platform.rs` | `DOMParser` → `dom::parse_html`. |
| `lib.rs`, `layout/mod.rs`, `media/mod.rs`, `images/mod.rs`, `api/a11y.rs`, `css/mod.rs` | `kuchiki::X` → `crate::dom::X`. The only other change is a comment in css. |
| `Cargo.toml` | − `html5ever 0.40`, − `kuchiki 0.8`; + `html_core` (path); + `precomputed-hash 0.1.1` (latest, a utility: the trait selectors 0.41 requires of its atom types). |
| `tests/dom_oracle.rs` | the serialized-DOM proof (below). |

There is no `document.write` in Aether, so nothing needed re-pointing for it.

## Gates

**EYES** (`tools/eyes/run.sh aether`, Chromium 1194 oracle). The merged base was run before any edit, then the swap
was run. **gate GREEN, and `scores.json` is byte-identical before and after**: every one of the 18 cases is the
same to four decimals. Mean SSIM is 0.797 and mean mismatch is 8.9%, both before and after. Render times per case
are unchanged (0.1–0.2 s).

| case | SSIM before | SSIM after | mismatch % before | after | |
|---|---:|---:|---:|---:|---|
| 01-blog-article | 0.6532 | 0.6532 | 12.04 | 12.04 | same |
| 02-flex-two-column | 0.7540 | 0.7540 | 8.59 | 8.59 | same |
| 03-card-gallery | 0.9661 | 0.9661 | 0.87 | 0.87 | same |
| 04-sticky-nav | 0.7219 | 0.7219 | 6.91 | 6.91 | same |
| 05-form | 0.7030 | 0.7030 | 4.63 | 4.63 | same |
| 06-table-zebra | 0.7913 | 0.7913 | 6.19 | 6.19 | same |
| 07-images-svg | 0.8292 | 0.8292 | 12.02 | 12.02 | same |
| 08-inline-wrap | 0.7446 | 0.7446 | 8.33 | 8.33 | same |
| 09-media-queries | 0.9772 | 0.9772 | 1.29 | 1.29 | same |
| 09-media-queries@375 | 0.9782 | 0.9782 | 1.22 | 1.22 | same |
| 10-calc-clamp | 0.7182 | 0.7182 | 26.00 | 26.00 | same |
| 10-calc-clamp@375 | 0.7416 | 0.7416 | 24.60 | 24.60 | same |
| 11-non-latin | 0.8111 | 0.8111 | 4.10 | 4.10 | same |
| 12-js-mutation | 0.7726 | 0.7726 | 8.38 | 8.38 | same |
| 13-ua-defaults | 0.8887 | 0.8887 | 2.58 | 2.58 | same |
| 14-boxes | 0.5447 | 0.5447 | 14.89 | 14.89 | same |
| 15-landing | 0.8869 | 0.8869 | 3.56 | 3.56 | same |
| 16-positioned | 0.8623 | 0.8623 | 13.34 | 13.34 | same |
"Unchanged" is the expected result, and it has a reason. None of the 16 pages hits a construct where html5ever
0.25 and the current spec disagree. HTMLCORE's oracle shows the page DOMs are already Chromium's, and the
matcher's accepted-selector set was kept on purpose. The wins from the swap are correctness on paths EYES does not
yet exercise: innerHTML in table/select contexts, cloneNode of table parts, source-order attributes (kuchiki
sorted them in a BTreeMap), and the 2026 parser rules.

**Serialized DOM = html_core = Chromium** (`cargo test --release -p aether --test dom_oracle`). For each of the 16
pages, the document element's outerHTML through Aether's `NodeRef` is compared with
`html_core::parse_document` + `serialize::outer_html` run stand-alone, and with Chromium's
`document.documentElement.outerHTML` (HTMLCORE's goldens). **16/16 raw byte-equal to both.** None of the pages has
a `<noscript>`, which is the only construct where Aether's scripting-on parse would differ from the scripting-off
goldens, and the test asserts that.

**`cargo test --release -p aether`: 106 passed, 0 failed.** That is 101 lib tests on the merged base, still green,
plus 4 new `dom` unit tests (parse/navigate/mutate/serialize, context-fragment innerHTML with the scripting flag,
cross-document import, a selector-matching table covering combinators, nth-child, attribute operators, :not,
:empty, :root, the link pseudo-classes, SVG, case-insensitive tag names, refusal of pseudo-elements, :focus-visible
and :is, and specificity order) plus the oracle test.

**Dependencies.** `cargo tree -p aether -e normal` shows no kuchiki, html5ever or markup5ever. The only
cssparser/selectors left are 0.38/0.41. In `Cargo.lock`, **44 package entries left**: html5ever ×2,
markup5ever ×2, kuchiki, cssparser 0.27 + macros, selectors 0.22, servo_arc 0.1, string_cache ×2 + codegen ×2,
tendril ×2, web_atoms, phf 0.8/0.10 families, rand 0.7 family, futf, mac, utf-8, … No version moved anywhere
else. The M1–M3 commit message says 46; the counted number is 44.

## Third-party crates this arc leans on

- `selectors` 0.41.0 (latest). **Chicken wire** until CSSCORE (SR47): it does the matching.
- `cssparser` 0.38.0 (latest). Chicken wire under selectors and the cascade, also CSSCORE's.
- `precomputed-hash` 0.1.1 (latest). A utility trait.

Nothing of html5ever/kuchiki remains. Parsing, the tree and serialization are UnaOS's own `html_core`.

## Honest ceiling

- **One RefCell per document.** A guard (`attributes.borrow()`, `as_text().borrow()`) holds the whole arena's
  borrow, so a caller must drop it before mutating the same document. Every current caller does, and all tests
  pass. A violation would be a panic, which the JS `guarded` boundary turns into a poisoned engine, not a dead
  process. Navigation snapshots ids, so it never holds a borrow.
- **The arena never frees.** Detached nodes, and each fragment imported by innerHTML, stay in the arena until the
  page is dropped. Ids are never reused, which is what makes a stale JS handle safe. An innerHTML-churning page
  grows memory until navigation. Owed: a free-list or compaction keyed off the JS registry.
- **Cross-document insertion copies.** A JS handle to a `DOMParser` node appended into the live document still
  points at the old copy. Adoption with identity preserved is owed.
- **The selector set is held at kuchiki parity on purpose.** `:checked/:disabled/:enabled/:focus/:hover` parse and
  never match, and `:is/:where/:has` and pseudo-elements go through the cascade's own lowering. Making them real
  belongs to CSSCORE/AETHERCSS, not this lane.
- `createElementNS` still ignores its namespace (it is ledgered), though the arena can now hold SVG/MathML nodes.
- `ElementData.name.local` is an owned snapshot, one small allocation per `as_element()`. The matcher path
  (`El`) allocates nothing. EYES timings show no cost.

## Owed

1. CSSCORE (SR47): replace `dom/select.rs` with UnaOS's matcher over `html_core`'s hooks (`parent_element`,
   `prev/next_sibling_element`, `element_children`, `has_class`, `element_id`). Its `Element` surface is the
   contract.
2. Arena reclamation; identity-preserving adopt; `createElementNS` namespaces.
3. An encoding sniffer in front of `parse_html`, from bytes (HTMLCORE owed #3). Aether still hands it decoded text.

## How a future executor continues

`cargo test --release -p aether` (dom units + `--test dom_oracle`); `tools/eyes/run.sh aether` (gate). The
`dom` module is the only file that knows `html_core`. Callers see the `NodeRef` API, so a later swap of
the arena, or of the matcher, stays inside `src/dom/`.
