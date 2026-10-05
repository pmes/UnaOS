# AETHERJS (LEDGER SR63): Aether runs JavaScript on js_core

Branch `exec-host-aetherjs`, cut from the boot-24 integration `exec-rmbp-merge14@fec61014` (js_core JSCORE
@6f0d1002, Aether on html_core + css_core + font_core, http_core, EYES with 24 cases). Lane: `handlers/aether`
(the script lane), plus two small hooks in `html_core` and `js_core` named below. No new crate.

## 1. The boa-era DOM binding surface (what Aether exposed through boa_engine 0.22 at the base)

The base bound the page through `handlers/aether/src/js/mod.rs`, `api/{window,element,cssom,events,fetch,platform,
video,websockets}.rs`, `js_scratch.rs` and a JS prelude. It was not WebIDL: every element was a fresh plain
object whose methods were own properties, and many interfaces were JS stubs.

| area | boa-era surface |
|---|---|
| document wrapper (own-property methods) | `getElementById`, `querySelector(All)`, `createElement`, `createTextNode`, `createComment`, `createElementNS` (namespace ignored), `getElementsByTagName/ClassName`, `readyState`, `cookie`, `currentScript` |
| element wrapper (own-property methods) | `appendChild`, `insertBefore`, `replaceChild`, `removeChild`, `cloneNode`, `contains`, `matches`, `closest`, `remove`, `set/get/has/removeAttribute`, `addEventListener`, `removeEventListener`, `dispatchEvent`, `click`, `focus`, `blur` |
| element accessors | `innerHTML`, `textContent`, `className`, `id`, `value`, `parentNode`, `parentElement`, `firstChild`, `childNodes`, `children`, `firstElementChild`, `nextElementSibling`, `tagName`, `dataset` |
| classList | `add`, `remove`, `toggle`, `contains` (no live DOMTokenList) |
| style | 27 camel-cased properties read/written over the `style` attribute string |
| interfaces | a JS-stub constructor chain (`Node`, `Element`, `HTMLElement`, … for `instanceof`), no prototypes carrying the members |
| prelude globals | `requestAnimationFrame`, `performance`, `matchMedia`, `Image`, `localStorage`, `navigator`, `MessageChannel`, `Event`, `CustomEvent`, observer stubs, `btoa/atob`, `structuredClone`, `getComputedStyle` (inline style only) |
| window | `window`, `location`, `history`, `setTimeout/Interval`, `fetch`, `XMLHttpRequest`, `URL`, `TextEncoder/Decoder`, `AbortController`, `DOMParser`, `XMLSerializer`, `crypto.getRandomValues` |
| media | a `video.rs` HTMLMediaElement shim (play/pause/currentTime) |
| scripts | all `<script>`s ran after the parse in document order; no `defer`/`async` distinction, no `document.write`, no inline `on*` attributes, modules best-effort |

## 2. Finding

boa was the last third-party engine in the browser (DEPS SR31: OWED, chicken wire) and it pulled 54 packages into
the lock. js_core (SR55) is spec-complete at 99.69 % of test262. The binding above was shaped by boa's API rather
than by WebIDL; scripts ran after parse rather than per HTML §4.12.1. This arc replaces both.

## 3. What changed

| file | change |
|---|---|
| `src/js/mod.rs` | `Engine { vm }`, the page state (`PAGE` thread-local: arena, wrappers, listeners, timers), `js_core::Host` for Aether, task entry with a per-task instruction budget (400 M) and stack-overflow/runaway recovery, the microtask checkpoint after every task and after every script (HTML §8.1.7.3), unhandled-rejection reporting. |
| `src/js/idl.rs` | WebIDL plumbing: interface objects (`[[Call]]` throws "Illegal constructor" unless constructible), interface prototype objects chained per the IDL inheritance, regular attributes as accessor pairs on the prototype `{enumerable, configurable}`, operations `{writable, enumerable, configurable}`, constants on interface and prototype `{enumerable}` only, `[LegacyUnforgeable]`, `@@toStringTag`, platform-object branding (a `Kind::Internal` object with a brand symbol + tag), `[SameObject]` caches, `[LegacyPlatformObject]` collections via Proxy. |
| `src/js/dom.rs` | DOM §4 over the html_core arena: `Node` (all §4.4 members incl. `compareDocumentPosition`, `isEqualNode`, `normalize`, `lookupNamespaceURI`), the §4.2.3 mutation algorithms with every pre-insert validity check and DOMException name, `ParentNode`/`ChildNode`/`NonDocumentTypeChildNode` mixins, `Element` (attributes incl. NS forms, `toggleAttribute`, `insertAdjacent*`, `getElementsBy*`, `matches/closest/webkitMatchesSelector`), `Attr`, `NamedNodeMap`, `CharacterData`/`Text`/`Comment`/`ProcessingInstruction`/`CDATASection` (UTF-16 offsets), `DocumentType`, `DocumentFragment`, `Document` (constructor, `createElement*`, `importNode/adoptNode`, `createDocumentFragment`, …), `DOMImplementation`, live `HTMLCollection`/`NodeList` and static `NodeList`, `DOMTokenList`, `innerHTML/outerHTML` via html_core §13.4 fragment parse, `XMLSerializer`. |
| `src/js/events.rs` | DOM §2: `EventTarget` (options, `once`, `passive`, `signal`), `Event`/`CustomEvent`/`UIEvent`/`MouseEvent`/`KeyboardEvent`/`FocusEvent`/`InputEvent`/`ErrorEvent`/`ProgressEvent`, the §2.9 dispatch algorithm (capture/target/bubble, `composedPath`, `stopImmediatePropagation`, legacy `returnValue`/`cancelBubble`), event handler IDL attributes and content attributes (`onclick="…"` compiled lazily with the HTML §8.1.8.1 name and deactivation), `report the exception` → `error` event → console, activation behaviour (`click()` on checkboxes/radios/links/submit). |
| `src/js/html.rs` | HTML interfaces: `HTMLElement` and ~60 subclasses with the reflected-attribute table (DOMString, boolean, long with clamping/IndexSizeError, enumerated, URL), `dataset` (DOMStringMap via Proxy), forms (`value`/`checked`/`selectedIndex`/`options`, dirty value flag, `form.elements`), tables (`rows`, `insertRow/Cell`, `tBodies`, `createTHead/TFoot/Caption`), anchors (`href` decomposition), `HTMLMediaElement` (`play()` promise, `pause`, `paused`, `currentTime`, `muted`, `volume`, `readyState`, `networkState`, `canPlayType`) backed by Aether's media module — AUDIOTRACK's owed JS surface. |
| `src/js/cssom.rs` | CSSOM: `CSSStyleDeclaration` over a css_core-parsed declaration block (`cssText`, `getPropertyValue/Priority`, `setProperty`, `removeProperty`, `item/length`, camel-case and dashed attributes), Blink-compatible shorthand serialization (margin/padding/border/border-*/border-image/background/font/flex/…); `getComputedStyle` from AETHERSTYLE's `computed_report` (layout flushed on read; detached elements → empty); `getBoundingClientRect`/`getClientRects` from AETHERINLINE's line fragments and box tree; `offset*`/`client*`/`scroll*`; `DOMRect(ReadOnly)`. |
| `src/js/window.rs` | `Window` global with `WindowProperties` (named access via Proxy), timers on the page's event loop with the HTML nesting clamp (level > 5 → 4 ms), `requestAnimationFrame` (run in the rendering step), `queueMicrotask`, `console`, `btoa/atob`, `matchMedia`, `Location`, `History`, `Navigator`, `Screen`, `performance`, the `Document` page members (`write`/`writeln`/`open`/`close`, `readyState`, `currentScript`, `cookie`, `title`, `body`, `head`, …). |
| `src/js/loader.rs` | HTML §4.12.1 script processing and §13.2.7 "the end": the parser runs incrementally (`html_core::TreeBuilder::step_token` to the next `</script>`), *prepare the script element* (classic/module/JSON-ignored; `src`, `defer`, `async`, `nomodule`, `type`/`language`), parser-blocking scripts (the event loop spins before each one, as Chromium's pending tasks do), defer list run after parse, async/ordered-as-soon-as-possible lists, module graphs (static + dynamic `import`, `import.meta.url`) through http_core, `document.write` inserting at the tokenizer's insertion point (and the implicit `document.open` after load), dynamically inserted scripts, `DOMContentLoaded` → `load` with virtual time frozen while the document loads. |
| `src/js/host.rs` | `AetherHost`: console sink, module specifier resolution, script fetch (`file:`, `data:`, `http(s):` via http_core). |
| `src/event_loop/mod.rs` | rewritten on js_core's timer list: virtual clock (signed offset, freeze/thaw), `fire_due_timers`, `boot_drain`, `settle(engine, budget_ms)` (what `--virtual-time-budget` does). |
| `src/api/{fetch,platform}.rs` | ported to js_core natives (fetch/XHR over http_core, URL, TextEncoder, AbortController, DOMParser, crypto, storage, MessageChannel, observer stubs, structuredClone, requestIdleCallback); `api/{window,video,cssom,websockets,element,scratch_fetch}.rs` and `js_scratch.rs` deleted (their surface is in `src/js/`). |
| `src/lib.rs`, `net`, `media`, `render`, `layout`, `css`, `dom` | page load goes through `js::loader::load` with the prefetched scripts; `tick` drives the loop; clicks go through `Engine::click` and honour `preventDefault`; form controls render their dirty value and selectedness; `computed_report`'s `position` reads the paint position (style oracle 6831/6980, position 349/349). |
| `unaos/libs/web/html_core` | `TreeBuilder::step_token(tz, limit)`/`finish()` + `script_ready` (the parser pause point), `Tokenizer::insert_at_cursor` (the `document.write` insertion point), `next_token_until`. `run()` is unchanged in behaviour. |
| `unaos/libs/web/js_core` | `Vm::track_rejections` + `rejected_unhandled` (rooted) for HTML's unhandled-rejection reporting. |
| `Cargo.toml`, `Cargo.lock` | − `boa_engine 0.22`, − `boa_gc`; + `js_core` (path). 54 packages leave the lock. `cargo tree -p aether -e normal` shows no `boa*`. |

## 4. Gates

**EYES** (`tools/eyes/run.sh aether`, Chromium 1194). The base was run on a detached base worktree before any edit;
the swap was run twice (the last after M4). **gate GREEN; `scores.json` and all 24 subject PNGs are byte-identical
before and after.** 12-js-mutation is unchanged (it is not worse; its residual is text rasterization, not script).

| case | SSIM before | SSIM after | mismatch % before | after | |
|---|---:|---:|---:|---:|---|
| 01-blog-article | 0.9939 | 0.9939 | 1.11 | 1.11 | same |
| 02-flex-two-column | 0.9989 | 0.9989 | 0.28 | 0.28 | same |
| 03-card-gallery | 0.9988 | 0.9988 | 0.16 | 0.16 | same |
| 04-sticky-nav | 0.9931 | 0.9931 | 1.17 | 1.17 | same |
| 05-form | 0.9546 | 0.9546 | 1.01 | 1.01 | same |
| 06-table-zebra | 0.9941 | 0.9941 | 0.69 | 0.69 | same |
| 07-images-svg | 0.9395 | 0.9395 | 2.95 | 2.95 | same |
| 08-inline-wrap | 0.9946 | 0.9946 | 1.20 | 1.20 | same |
| 09-media-queries | 0.9978 | 0.9978 | 0.44 | 0.44 | same |
| 09-media-queries@375 | 0.9977 | 0.9977 | 0.40 | 0.40 | same |
| 10-calc-clamp | 0.9987 | 0.9987 | 0.13 | 0.13 | same |
| 10-calc-clamp@375 | 0.9986 | 0.9986 | 0.38 | 0.38 | same |
| 11-non-latin | 0.9904 | 0.9904 | 0.85 | 0.85 | same |
| 12-js-mutation | 0.9965 | 0.9965 | 0.27 | 0.27 | same |
| 13-ua-defaults | 0.9969 | 0.9969 | 0.44 | 0.44 | same |
| 14-boxes | 0.9773 | 0.9773 | 0.79 | 0.79 | same |
| 15-landing | 0.9957 | 0.9957 | 0.79 | 0.79 | same |
| 16-positioned | 0.9988 | 0.9988 | 0.17 | 0.17 | same |
| 17-inline-continue | 0.9925 | 0.9925 | 1.70 | 1.70 | same |
| 18-stacking | 0.9990 | 0.9990 | 0.18 | 0.18 | same |
| 19-em-units | 0.9988 | 0.9988 | 0.30 | 0.30 | same |
| 20-table-spans | 0.9906 | 0.9906 | 0.74 | 0.74 | same |
| 21-paint-effects | 0.9996 | 0.9996 | 0.01 | 0.01 | same |
| 22-fonts | 0.9919 | 0.9919 | 1.22 | 1.22 | same |

**Script oracle** (`tests/script_oracle.rs`, 20 pages served over HTTP to both browsers; Aether's serialized
document after `settle(10 s)` vs `headless_shell --dump-dom --virtual-time-budget=10000`): **20/20 byte-equal.**

| page | exercises |
|---|---|
| 01-dom-build | createElement/TextNode/Comment, createDocumentFragment, append/prepend |
| 02-traversal | parent/child/sibling accessors, childNodes/children, compareDocumentPosition, namedItem |
| 03-query | querySelector(All), getElementsByTagName/ClassName/Name, matches/closest |
| 04-attributes | get/set/removeAttribute, toggleAttribute, setAttributeNS, attributes (NamedNodeMap), getAttributeNode |
| 05-classlist | DOMTokenList add/remove/toggle/replace/item |
| 06-innerhtml | innerHTML/outerHTML incl. template contents, insertAdjacentHTML, importNode |
| 07-chardata | data/length/substringData/insert/delete/replaceData/splitText, normalize, wholeText |
| 08-style | CSSStyleDeclaration cssText/setProperty/priority/shorthand serialization |
| 09-computed | getComputedStyle values from the cascade |
| 10-geometry | getBoundingClientRect/getClientRects/offset*/client* from layout |
| 11-events | dispatch phases, once/passive, stopPropagation, CustomEvent, composedPath |
| 12-handlers | inline `on*` attributes, IDL handlers, onerror / error reporting |
| 13-loop | setTimeout/setInterval, microtasks vs tasks, requestAnimationFrame, queueMicrotask, promise ordering |
| 14-script-order | parser-blocking vs defer vs async vs module ordering |
| 15-document-write | document.write during parse incl. written `<script>`s |
| 16-current-script | document.currentScript for inline, external, async, module (null), timers |
| 17-dynamic-scripts | inserted scripts, `async` handling, load/error events |
| 18-forms | value/checked/selectedIndex, form.elements, matches on form state |
| 19-window | location parts, history, btoa/atob, matchMedia |
| 20-documents-media | createHTMLDocument, DOMParser, XMLSerializer, importNode, HTMLMediaElement play/paused/muted/volume/canPlayType |

**WPT `dom/nodes`** (`tests/wpt_dom.rs`, release build): web-platform-tests pinned at `da1f6d20caf4`, sparse
checkout fetched at test time into `target/wpt`; each `.html` / `.window.js` test runs in Aether with
testharness.js and a `testharnessreport.js` that records results; per-file results go to
`target/wpt-dom-nodes.tsv`.

| set | files | subtests passed / total | |
|---|---:|---:|---:|
| covered (Aether's surface) | 202 | **6089 / 6313** | **96.5 %** |
| all of `dom/nodes` | 256 | 7775 / 9057 | 85.8 % |

Excluded from "covered" (54 files, outside the surface by the ceiling below): frames / browsing contexts 27,
MutationObserver 16 (a stub that never delivers), shadow DOM 8, Range 1, NodeIterator/TreeWalker 1, custom
elements 1. 154 covered files pass every subtest; 48 do not. The largest residuals: arbitrary namespaces and
prefixes (`DOMImplementation-createDocument` 385/434, `Document-createAttribute` 2/36, `Attr-prefix` 0/6,
`Element-removeAttribute(NS)`, `Element-setAttributeNodeNS`, `getElementById-namespaced-id`, the
querySelector namespace cases), `case.html` 260/285 (XML-document case rules), `Document-createEvent` 257/279
(legacy event interfaces not exposed), `Node-textContent` 73/81, `Node-baseURI` 4/9, `remove-unscopable` 0/6
(`@@unscopables`), `name-validation` 1/5, `CharacterData-surrogates` 2/8; `Comment`/`Text-constructor` time out
on their last subtest (15/16), and the six `NodeList-static-length-getter-tampered*` and two
`Document-characterSet-normalization` files (very long loops / encoding labels) do not complete in the harness
budget. The test asserts covered ≥ 80 % as its floor.

**`cargo test -p aether`**: green (lib 152, dom_oracle, script_oracle 20/20, style_oracle, text_oracle, style_time);
the boa-era engine tests are re-pointed (`eval_number`/`eval_string`), with new KATs for stack overflow, runaway
loops, prefetched-script `currentScript`, select selectedness and the timer clamp. `cargo test -p html_core` and
`-p js_core` green (JS_OFFLINE=1).

## 5. Third-party crates

None added. Aether's remaining direct third-party dependencies are unchanged from the base: taffy 0.14.0, resvg
0.48.1, image 0.25.10 (chicken wire, per DEPS SR31), and the utilities anyhow 1.0.104, base64 0.23.1, clap 4.6.7,
directories 6.0.0, futures-util 0.3.34, serde 1.0.229, serde_json 1.0.151, thiserror 2.0.21, tokio 1.53.2, url
2.5.8. js_core has zero dependencies. boa_engine 0.22 and boa_gc 0.22 (chicken wire) are gone.

## 6. Ceiling

Arbitrary element namespaces and prefixes (html_core's `Namespace` holds HTML/SVG/MathML/XLink/XML/XMLNS/none),
XML documents, MutationObserver, Range/StaticRange, NodeIterator/TreeWalker, shadow DOM, custom elements,
frames/`contentDocument`, `@@unscopables`; `innerText` is a layout-free approximation; inline handlers get the
global scope rather than the element/form/document scope chain; `getComputedStyle` answers the properties
`computed_report` carries; `document.open` after load replaces the document rather than navigating;
`location.assign` is staged to the shell; `localStorage` is in-memory per run.

## 7. Owed / how to continue

MutationObserver (microtask-queued records from the mutation algorithms in `dom.rs`) is the largest WPT block
excluded; Range and NodeIterator/TreeWalker next. Arbitrary namespaces need an html_core `Namespace::Other(atom)`.
Rerun: `cargo test --release -p aether --test wpt_dom -- --nocapture` (`AETHER_WPT_ONLY=a,b` filters by file name, `AETHER_WPT_VERBOSE=1` prints failing subtests),
`cargo test -p aether --test script_oracle` (`AETHER_ORACLE_RECORD=1` re-records with Chromium), `tools/eyes/run.sh
aether`.
