# Aether: standing gap list

**Run date:** 2026-07-28 · **Tree HEAD:** `5a3a2c1c4d55bb1d975cf96a523031534d27ff40`
(plus uncommitted WIP — see [In flight](#in-flight))

This document replaces random discovery of missing browser basics with a
systematic, verified inventory. Section A walks the capability surface a
minimal-but-real desktop browser is expected to have and records what the tree
actually does, verified by reading the code (not commit messages). Section B
aggregates the engine's own API-coverage ledger across an 11-page corpus.
Section C is the prioritized shortlist.

Accessibility is **out of scope for this pass** and deliberately unlisted.
`handlers/aether/src/api/a11y.rs` exists but nothing in the shell consumes it;
a11y needs its own inventory.

Re-run recipe (section B):

```
cargo build --release -p aether --bin aether
target/release/aether render <url> --out <png> --ledger <ledger>
```

---

## A. UX capability table

Status key: **DONE** — works end-to-end. **PARTIAL** — a usable path exists but
a named piece is missing. **ABSENT** — no code path at all. **BROKEN** — a code
path exists and produces a wrong result.

### Window chrome

| Item | Status | Evidence | Gap |
| --- | --- | --- | --- |
| Address bar reflects current URL | DONE | `vessels/aether-shell/src/main.rs` (post-turn choke point → `BrowserUrlChanged`); `libs/quartzite/src/platforms/macos/text_field.rs` | — |
| Back / Forward / Reload buttons | DONE | `vessels/aether-shell/src/main.rs` (TetraNode::Button → `BrowserNavBack/Forward/Reload`); `handlers/aether/src/lib.rs` `get_back_url`/`get_forward_url` | History re-fetches over the network; no back-forward cache. |
| Window title follows `<title>` | DONE (in flight, uncommitted) | `handlers/aether/src/lib.rs` sets `self.title`; `vessels/aether-shell/src/main.rs` fires `BrowserTitleChanged` (deduped, same choke point as the url); `libs/quartzite/src/platforms/macos/window_title.rs` writes it | Landed mid-audit — see [In flight](#in-flight). |
| Favicon in chrome | DONE (in flight, uncommitted) | `handlers/aether/src/net/mod.rs` `favicon_url`/`fetch_favicon`; shell fires `BrowserFaviconChanged` after delivery; `window_title.rs` sets it as the window's document (proxy) icon | Landed mid-audit. Proxy icon only — no tab strip to carry it. |
| Load progress / spinner | ABSENT | no load-state message in `libs/bandy/src/signals.rs`; `vessels/aether-shell/src/main.rs` awaits `fetch_page` inline | A multi-second load is indistinguishable from a hang. |
| Tabs / multiple windows | ABSENT | `TetraNode::Surface` is singular; `libs/quartzite/src/platforms/macos/tetra_eval.rs` notes "SurfaceBlit must target a Surface by id once >1 surface exists" | Single document per process. |

### Pointer

| Item | Status | Evidence | Gap |
| --- | --- | --- | --- |
| Cursor shape changes (pointer over links, I-beam over text) | ABSENT | no `NSCursor` / `cursor` handling anywhere in `libs/quartzite/src`; CSS `cursor` is ledgered unsupported (119 hits, 8 of 11 corpus pages) | Cursor is always the arrow. Links are indistinguishable from text by feel. |
| Hover states (`:hover`, JS `mouseover`) | ABSENT | `handlers/aether/src/lib.rs` — `Event::MouseMove(_x, _y) => {}` is an empty arm; no `BrowserMouseMove` variant in `libs/bandy/src/signals.rs`; `engine_tests.rs::test_hover_focus_not_applied` asserts `:hover` must not match | Motion never reaches the engine. Nav menus that open on hover cannot be opened. |
| Click → link navigation | DONE | `handlers/aether/src/lib.rs` `MouseUp` ancestor walk for `<a href>` → `pending_nav` | — |
| Click → form submit (submit button / `<button>`) | ABSENT | `handlers/aether/src/lib.rs` `MouseUp` handles `a` (navigate) and `input`/`textarea`/`select` (focus only); no submit branch. Submission is reachable **only** via `Return` in a focused `<input>` (`Event::KeyDown`) | Clicking Search/Submit does nothing. |
| Text drag-select | ABSENT | `Event::MouseDown` is an empty arm in `handlers/aether/src/lib.rs`; no selection model in `handlers/aether/src/render/mod.rs` | — |
| Right-click context menu | ABSENT | no `rightMouseDown:` in `libs/quartzite/src/platforms/macos/image_view.rs` | — |
| Hit testing | PARTIAL | `handlers/aether/src/lib.rs` `hit_test` walks the taffy tree and keeps the last containing box | Tree order, not paint order — overlapping/positioned boxes can mis-hit. |

### Keyboard

| Item | Status | Evidence | Gap |
| --- | --- | --- | --- |
| Typing into a focused `<input>` | PARTIAL | `handlers/aether/src/lib.rs` `Event::Text` appends to the `value` attribute; `Event::KeyDown` handles `BackSpace`/`Return` | Append-only: no caret, no arrow-key cursor movement, no mid-string edit, no selection, no paste. |
| Typing into `<textarea>` | ABSENT | `Event::Text` gates on `&*el.name.local == "input"` | Textareas take focus but cannot be typed into. |
| Page scrolling by key (Space / PageDown / arrows / Home / End) | ABSENT | `libs/quartzite/src/platforms/macos/image_view.rs` `keyDown:` maps only Backspace/Return/printable → `BrowserText`; the engine's only scroll source is `Event::Scroll` | Keyboard cannot move the page. |
| Arrow keys | BROKEN | same `keyDown:` — arrows arrive as private-use chars (U+F700…F703) which pass the `!c.is_control()` test and are sent as `BrowserText` | Pressing an arrow inserts a garbage character into the focused field. |
| Tab focus traversal | ABSENT | no `tabindex` handling in `handlers/aether/src`; `focused_node` is set only by `MouseUp` | Focus is mouse-only. |
| Cmd+L (focus address bar), Cmd+R (reload), Cmd+[ / Cmd+] | ABSENT | menu built in `libs/quartzite/src/platforms/macos/mod.rs` binds only Cmd+Q/S/Z/X/C/V/A — no browser items | — |
| Cmd+C / Cmd+A over page content | ABSENT | Edit-menu items target the responder chain, but `FacetImageView` implements no `copy:`/`selectAll:`, and there is no selection to copy | Menu items are present and inert on the page surface. |
| Modifier-key leakage | BROKEN | `keyDown:` in `image_view.rs` inspects `charactersIgnoringModifiers` without checking `modifierFlags` | An unbound Cmd+<key> that reaches the view types its letter into the page. |

### Text

| Item | Status | Evidence | Gap |
| --- | --- | --- | --- |
| Text selection | ABSENT | no selection model in `handlers/aether/src/render/mod.rs`; `::selection` rules ledgered as `pseudo-element-rule` (`handlers/aether/src/css/mod.rs`) | — |
| Copy to clipboard | ABSENT | no `NSPasteboard` use in `libs/quartzite/src/platforms/macos/image_view.rs` | Nothing can be copied out of a page. |
| `window.getSelection()` | ABSENT | not installed in `handlers/aether/src/js/mod.rs` or `handlers/aether/src/api/window.rs` | Editors/copy widgets that call it throw. |
| Find in page | ABSENT | no search UI in `vessels/aether-shell/src/main.rs` | — |
| Font families sans/serif/mono | DONE | `handlers/aether/src/fonts/mod.rs`, `handlers/aether/src/render/mod.rs` family selection | Bold non-sans falls back to sans-bold. |
| `@font-face` web fonts | ABSENT | ledgered `at-rule:@font-face`, 110 hits across 3 pages | Icon fonts render as mojibake or nothing. |

### Forms

Rendered evidence: a probe page with one control of each type
(`handlers/aether/src/render/mod.rs` is the only painter of form controls; every
control below draws the same 1px `(118,118,118)` rectangle plus its
`value`/`placeholder` text).

| Item | Status | Evidence | Gap |
| --- | --- | --- | --- |
| `<input type=text>` paint | DONE | `render/mod.rs` control branch (value vertically centred) | — |
| `<input type=password>` paint | BROKEN | `render/mod.rs` draws `attrs.get("value")` verbatim, no masking | **Passwords render in plaintext on screen.** |
| `<input type=checkbox>` / `radio` | ABSENT | no glyph branch in `render/mod.rs`; `layout/mod.rs` gives every `input` a 160×24 UA minimum | Renders as a wide empty text box. Visible on Wikipedia's ToC toggles. Clicking never toggles `checked` (`lib.rs` `MouseUp` only sets focus). |
| `<select>` dropdown | ABSENT / BROKEN | `layout/mod.rs` skip-list (`head/script/style/…`) does not include `option`, so every `<option>` lays out as ordinary text | Select renders as a box with all options stacked as visible text; no popup, no selected-value display. |
| `<input type=file>` | ABSENT | no branch in `render/mod.rs`; no file picker in the shell | Empty box; uploads impossible. |
| `<input type=range>` / `date` / `color` | ABSENT | no branch in `render/mod.rs` | Each paints as a text box showing the raw value string. |
| Focus ring | ABSENT | no focus-dependent paint in `render/mod.rs`; `focused_node` never reaches the renderer | Nothing shows which field has focus. |
| Caret | ABSENT | same | — |
| Disabled state | ABSENT | `disabled` attribute unread in `render/mod.rs` and `forms/mod.rs` | Disabled controls look and behave live. |
| Push-button label centring | DONE (in flight) | `render/mod.rs` `center_box` / `centered_line_origin_y` — uncommitted WIP | — |
| Form submission (GET/POST, urlencoding) | PARTIAL | `handlers/aether/src/forms/mod.rs`; `lib.rs::build_form_submission` | Collects the `value` attribute of every named `input`/`textarea`/`select`: checkbox/radio `checked` is ignored (all are submitted), `<select>` submits its `value` attribute rather than the selected `<option>`, and `<textarea>` submits an attribute it does not have. |
| `enctype=multipart/form-data` | ABSENT | `forms/mod.rs` emits urlencoded only | — |

### Navigation

| Item | Status | Evidence | Gap |
| --- | --- | --- | --- |
| Typed URL / scheme guessing | DONE | `handlers/aether/src/net/mod.rs` `normalize_url` (https then http) | — |
| History back/forward | DONE | `lib.rs` history vector + `vessels/aether-shell/src/main.rs` | Re-fetches; no cache; `history.length` in JS is a constant `1` (`api/window.rs`). |
| Redirects | PARTIAL / BROKEN | `net/mod.rs` `fetch_document` returns only the body — `response.url()` is discarded; `fetch_page` uses `normalize_url(input)` as the base | After a redirect the base URL is the **requested** URL: relative links/images/sheets resolve against the wrong origin or path, and the address bar shows the pre-redirect URL. |
| HTTP error pages | BROKEN | `net/mod.rs` bails on `!status.is_success()` | A site's own 404/403/500 body is discarded and replaced by our error page. |
| Non-UTF-8 documents | ABSENT | `net/mod.rs` `String::from_utf8(...).context("Invalid UTF-8")` | A latin-1 page fails the whole load; no `Content-Type` charset or `<meta charset>` handling. |
| Error page | DONE | `lib.rs::load_error_page` | Plain, no retry affordance. |
| `#fragment` anchors | ABSENT | `lib.rs` `MouseUp` builds a full `OpenDocument` for any `href`; `load_impl` resets `scroll_y = 0.0`; `js/mod.rs::set_location` hardcodes `location.hash = ''` | An in-page anchor re-fetches the document and lands at the top, and page JS always reads an empty hash. |
| `history.pushState` / SPA routing | ABSENT | `api/window.rs` `history` exposes `back`/`forward`/`go` as no-ops; no `pushState` | — |
| `window.location` assignment → navigation | PARTIAL | `js/mod.rs` `set_location` gives a real `location`; no navigation is staged from a JS assignment | Script-driven navigation does not happen. |
| Cookies | DONE | `net/mod.rs` shared `Jar`, wired to `document.cookie` | Session-scoped, never persisted. |

### Content

| Item | Status | Evidence | Gap |
| --- | --- | --- | --- |
| Wheel / trackpad scrolling | DONE | `image_view.rs` `scrollWheel:` → `BrowserScroll`; `lib.rs` clamped scroll with a copy-within fast path | — |
| Live resize / reflow | DONE | `image_view.rs` 150 ms throttle + guaranteed `viewDidEndLiveResize` fire; `main.rs::coalesce` (last-resize-wins, scroll-sum); `lib.rs` `Event::Resize` relayouts | — |
| Page zoom (Cmd +/−) | ABSENT | no zoom state in `handlers/aether/src`; `image_view.rs` zoom paths are explicitly disabled in browser mode | — |
| Print | ABSENT | no `window.print` in `js/mod.rs`; no print path in the shell | — |
| Scrollbar | ABSENT | nothing painted in `render/mod.rs`; no chrome scrollbar | No indication of document length or position. |

### Media

| Item | Status | Evidence | Gap |
| --- | --- | --- | --- |
| `<video>`/`<audio>` → Stria passthrough | DONE | `lib.rs::media_source_for` / `media_sources`, `take_pending_media`; `vessels/aether-shell/src/main.rs` fires `SMessage::PlayMedia` | Per the charter: no site-specific resolver. |
| In-page media controls (`controls` attribute) | ABSENT | no media branch in `render/mod.rs` | A `<video>` paints as an empty box; play is a whole-element click. |
| Poster frames | ABSENT | same | — |

---

## B. Engine gap leaderboard

Corpus, all 11 pages, all completed under 90 s (longest: yahoo 21 s,
wikipedia 12 s, reddit 12 s): example.com, news.ycombinator.com,
en.wikipedia.org/wiki/Rust_(programming_language), www.wikipedia.org,
www.google.com, www.google.com/search?q=test, old.reddit.com, docs.rs,
craigslist.org, rust-lang.org, yahoo.com.

Distinct ledger keys per page: wikipedia_rust 494, yahoo 454, reddit 182,
rustlang 94, docsrs 87, wikipedia_portal 55, google 33, hn 11, craigslist 4,
google_search 3, example 0.

**Caveat on `google_search`:** Google served the scripting-fallback page
("If you're having trouble accessing Google Search…"), so its 3-key ledger
describes that stub, not a results page.

Top 25 by total call count. Keys marked (\*) are families collapsed across
their per-value/per-URL suffixes; everything else is a literal ledger key.

| # | Cat | Key | Count | Pages hit |
| --- | --- | --- | --- | --- |
| 1 | CSS | pseudo-element-rule (`::before`/`::after`/`::selection`/…) | 2410 | craigslist, docsrs, google, reddit, rustlang, wikipedia_portal, wikipedia_rust, yahoo |
| 2 | CSS | selector-compile-failed (\*) — 532 distinct selectors | 929 | docsrs, google, reddit, rustlang, wikipedia_portal, wikipedia_rust, yahoo |
| 3 | CSS | named-color (\*) — unparsed colour values | 359 | docsrs, hn, reddit, rustlang, wikipedia_portal, wikipedia_rust, yahoo |
| 4 | DOM | img-missing (\*) | 342 | wikipedia_rust, yahoo |
| 5 | CSS | property:box-shadow | 225 | docsrs, google, reddit, rustlang, wikipedia_portal, wikipedia_rust, yahoo |
| 6 | CSS | property:vertical-align | 221 | docsrs, google, reddit, rustlang, wikipedia_portal, wikipedia_rust, yahoo |
| 7 | CSS | property:transform | 122 | docsrs, reddit, rustlang, wikipedia_portal, wikipedia_rust, yahoo |
| 8 | CSS | property:z-index | 121 | craigslist, docsrs, google, reddit, rustlang, wikipedia_portal, wikipedia_rust, yahoo |
| 9 | CSS | property:cursor | 119 | docsrs, google, hn, reddit, rustlang, wikipedia_portal, wikipedia_rust, yahoo |
| 10 | CSS | media-condition (\*) (`only screen`, …) | 115 | docsrs, hn, reddit, rustlang, wikipedia_portal, yahoo |
| 11 | CSS | property:letter-spacing | 113 | docsrs, google, reddit, rustlang, yahoo |
| 12 | CSS | at-rule:@font-face | 110 | docsrs, rustlang, yahoo |
| 13 | CSS | property:transition | 87 | docsrs, reddit, rustlang, wikipedia_portal, wikipedia_rust, yahoo |
| 14 | CSS | property:transition-duration | 82 | wikipedia_rust, yahoo |
| 15 | CSS | font-size-value (\*) (`larger`, `smaller`, …) | 80 | docsrs, google_search, reddit, wikipedia_portal, wikipedia_rust, yahoo |
| 16 | CSS | property:fill | 75 | docsrs, google, reddit, wikipedia_portal, wikipedia_rust, yahoo |
| 17 | CSS | property:clear | 65 | reddit, rustlang, wikipedia_portal, wikipedia_rust |
| 18 | CSS | property:gap | 64 | reddit, rustlang, wikipedia_rust, yahoo |
| 19 | CSS | property:transition-timing-function | 58 | wikipedia_rust, yahoo |
| 20 | CSS | at-rule:@keyframes | 52 | craigslist, docsrs, google, reddit, wikipedia_rust, yahoo |
| 21 | CSS | property:transition-property | 51 | wikipedia_rust, yahoo |
| 22 | CSS | property:flex | 50 | google, reddit, rustlang, wikipedia_portal, wikipedia_rust, yahoo |
| 23 | CSS | property:grid-template-columns | 50 | wikipedia_rust, yahoo |
| 24 | CSS | property:animation | 44 | craigslist, docsrs, google, reddit, wikipedia_rust, yahoo |
| 25 | CSS | property:list-style-type | 39 | docsrs, reddit, rustlang, wikipedia_portal, wikipedia_rust, yahoo |

Just below the cut: `property:-webkit-transform` (37),
`property:outline-color` (36), `property:order` (35),
`property:text-overflow` (32), `property:border-spacing` (30),
`property:filter` (25), `property:aspect-ratio` (24).

**#2 deserves a closer look.** `selector-compile-failed` means the rule was
dropped whole — not one declaration ignored, the entire rule never tested.
532 distinct selectors across 7 of 11 pages. The recurring shapes in the
truncated keys are modern selector syntax the engine's compiler rejects:
`:focus-visible`, `:where(…)`, `:has(…)`, `:focus-within`, and CSS-escaped
utility-class names (`.focus-visible\:outline-…`, `.has-\[\:focus-visible\]…`).
Yahoo's design system alone (`.uds-*` variants) accounts for ~200 hits. This is
invisible in a screenshot — the page simply renders unstyled in those regions —
which is exactly the failure mode this document exists to surface.

**Legacy colour syntax:** reddit's `named-color:-moz-linear-gradient(top` and
`named-color:-webkit-gradient(linear,` (28 each) show old vendor gradients
being fed to the colour parser rather than a gradient parser.

### JS/DOM side of the ledger

The ledger is overwhelmingly CSS. The entire JS/DOM half of the corpus is:

| Cat | Key | Count | Pages |
| --- | --- | --- | --- |
| DOM | img-missing (\*) | 342 | wikipedia_rust, yahoo |
| DOM | img-fetch-failed (\*) | 18 | wikipedia_rust |
| DOM | document.cookie:get | 9 | google_search, wikipedia_portal, wikipedia_rust, yahoo |
| JS | script-error (\*) | 2 | wikipedia_rust, yahoo |
| DOM | document.cookie:set | 1 | google_search |
| DOM | document.currentScript:element | 1 | yahoo |
| DOM | img-fetch-cap-reached | 1 | yahoo |
| JS | observer-never-delivers:MutationObserver | 1 | yahoo |

Distinct script errors observed across the corpus:
`ReferenceError: $ is not defined`, `ReferenceError: r is not defined` (×4),
`TypeError: cannot convert 'null' or 'undefined' to object`,
`TypeError: cannot bind 'this' without a [[Call]] internal method`,
`TypeError: Failed to construct URL: Invalid URL: undefined`.

**Defect in the ledger dump itself, found during this pass.** Six of those
eight lines are missing their count column, because the 64-character error
digest can contain a newline from boa's stack trace. `ApiCoverageLedger::
dump_to_file` (`handlers/aether/src/ledger/mod.rs`) writes one `writeln!` per
record assuming the name is single-line, so such a record splits across two
physical lines and the second is unparseable:

```
JS       | script-error:ReferenceError: $ is not defined
    at onPageLoad (unknown at : | 1
```

Both producers are affected: `lib.rs`'s `script-error:` slicing and
`event_loop::record_digest`. Any automated aggregation over these ledgers
silently drops the affected records. Sanitizing the digest (strip control
characters, as the `UNAOS_JSDEBUG` head string already does) is an **S**.

**This near-silence is itself a finding.** The ledger instruments the CSS
cascade densely and the JS lane barely at all: a missing global is a
`ReferenceError` that shows up as one collapsed `script-error` line for the
whole bundle, and a missing *method* on a supported object is not ledgered at
all. Section A's JS rows (`getSelection`, `pushState`, `print`,
`innerWidth`/`innerHeight`, `scrollTo`) were all found by reading
`handlers/aether/src/js/mod.rs` and `api/window.rs`, not by the ledger — the
ledger would not have surfaced any of them.

---

## C. Prioritized shortlist

Ten highest-leverage items, combining both halves. Size is rough
implementation effort: **S** ≈ a sitting, **M** ≈ an arc, **L** ≈ multiple arcs.

1. **Mask `<input type=password>`** — plaintext passwords currently paint to the screen; draw bullets in the control branch of `render/mod.rs`. (**S**)
2. **Submit on click** — a click on `<button>` / `<input type=submit>` must run `build_form_submission`; today only `Return` in a focused input submits, so every search box on the web is dead to the mouse. (**S**)
3. **Keyboard page scrolling + arrow-key fix** — Space/PageUp/PageDown/Home/End/arrows in `image_view.rs::keyDown:` mapped to `BrowserScroll`, and stop private-use arrow codepoints being typed into fields. (**S**)
4. **Checkbox / radio paint + toggle** — glyph branch in `render/mod.rs`, `checked` toggle in `MouseUp`, and `checked`-aware form serialization in `build_form_submission`. Visible on the very first Wikipedia render. (**M**)
5. **Skip `<option>` in layout, paint the selected value** — one skip-list entry in `layout/mod.rs` stops every select from dumping its options as page text; then draw the selected option in the box. A real dropdown popup is a separate, later item. (**S** for the fix, **M** with the popup)
6. **Redirect-aware base URL** — carry `response.url()` out of `net::fetch_document` and use it as `Page::base_url`; today every redirected page resolves its relative URLs against the wrong base and the address bar lies. (**S**)
7. **`MouseMove` end-to-end: `BrowserMouseMove` → hover → cursor shape** — the single change that unlocks three of Peter's finds at once (`:hover` matching, `cursor:` honoured — #9 on the leaderboard, 8 of 11 pages — and hover-opened navigation). (**M**)
8. **Text selection + Cmd+C** — selection model in the engine (anchor/focus offsets over the layout text runs), inverse-video paint, `NSPasteboard` copy, and `window.getSelection()` on top of it. (**L**)
9. **Modern selector syntax — `:where()`, `:has()`, `:focus-visible`, `:focus-within`, escaped utility-class names** — 929 hits over 532 distinct selectors on 7 of 11 pages, each one a *whole rule* silently dropped. Highest ratio of pages-affected to effort on the leaderboard, and invisible without the ledger. (**M**)
10. **`::before` / `::after` generated content** — 2410 hits, the largest single item by an order of magnitude; icons, separators, and quote marks across every non-trivial page depend on it. (**L**)

Two more that did not make the ten but are near-free: **sanitize ledger error
digests** (see the defect note in section B — **S**, and it makes every future
run of this document trustworthy), and **a load-state message** so a slow page
is distinguishable from a hang (**S** for the message, **M** with chrome
affordance).

Runner-up cluster, all cosmetic-but-pervasive and cheap once one of them is
scaffolded: `box-shadow`, `vertical-align`, `letter-spacing`, `z-index`,
`text-overflow`, `list-style-type`, `@font-face`.

---

## In flight

Verified against `git status` at HEAD `5a3a2c1c` — these files carry
uncommitted work by other sessions, so the items below are **already being
handled** and are listed here only so they are not double-counted:

- `handlers/aether/src/event_loop/mod.rs`, `handlers/aether/src/lib.rs`,
  `handlers/aether/src/main.rs`, `handlers/aether/src/engine_tests.rs` — the
  timer-queue / bounded-job-executor arc (see the 2026-07-28 section of
  `PLAN-aether-browser.md`).
- `handlers/aether/src/render/mod.rs` — push-button label centring
  (`center_box`, `measure_text_width`, `centered_line_origin_y`).
- `handlers/aether/src/js/mod.rs` — small edits alongside the timer arc.
- **Window title + favicon, end to end** — this arc landed *during* this audit
  (the working tree gained `vessels/aether-shell/src/main.rs`,
  `libs/quartzite/src/platforms/macos/mod.rs` and a new
  `libs/quartzite/src/platforms/macos/window_title.rs` between the corpus run
  and this write-up). The two chrome rows in section A were re-verified against
  the tree afterwards and are recorded DONE. Load progress is a separate item
  and remains ABSENT.
- Unrelated to Aether, also uncommitted at the time of writing:
  `handlers/principia/` (new `Cargo.toml`, `src/prefs.rs`),
  `libs/bandy/tests/smessage_kats.rs`, `Cargo.toml`/`Cargo.lock`.
