# Aether — the UnaOS web browser engine

**Crate:** `handlers/aether` · **Vessel:** `vessels/aether-shell` · **Layer:** Handler (domain service) + shell vessel · **Status:** live, under active development

Aether is UnaOS's web browser: a from-scratch HTML/CSS/JS engine plus
`vessels/aether-shell`, the macOS window vessel that gives it a face over
Quartzite. It fetches, parses, styles, lays out, paints, and *runs* pages —
there is no WebKit, Blink, or Gecko under it and no embedded webview.

Aether was originally chartered as a read-only, no-JS document reader. That
constraint was **lifted by owner decision on 2026-07-20**, recorded as the M0
charter amendment in [`docs/CODEX.md`](../../docs/CODEX.md) ("Amendment II:
Aether becomes a real web browser. JS-enabled; delegates A/V playback to
Stria."). The charter leads; this README follows it.

## What it is today

- **Network** — `reqwest` over tokio: HTTPS GET with redirects, content-type
  and charset handling, plus subresource discovery (stylesheets, scripts,
  images, favicon) fetched concurrently. Cookies live in a process-wide,
  in-memory jar shared by every client, so a `Set-Cookie` from one navigation
  is sent on the next and on subresource fetches to the same host. The jar is
  **session-scoped** — nothing is written to disk, so quitting Aether logs you
  out.
- **DOM** — `kuchiki` (html5ever underneath) parses to a mutable node tree that
  scripts mutate in place.
- **CSS** — a real cascade, not a property bag. Selectors compile through
  kuchiki's servo `selectors` engine (compound, descendant, pseudo-class and
  attribute selectors); rules sort by `(importance, specificity, source
  order)`, with inline styles and `!important` in their correct tiers; only
  *specified* declarations are applied, so a rule never stomps another rule's
  or the UA default's values. Lengths resolve through a real math evaluator —
  `calc()`, `min()`, `max()`, `clamp()`, nested parentheses, `+ - * /`,
  px/rem/em and the viewport units. `@media` and `@supports` are evaluated
  **honestly**: `@supports` answers against the engine's actual capability set,
  so a page that feature-detects gets the truth rather than a blanket yes.
- **Layout** — `taffy` block/flex layout over a box tree built from the DOM,
  with quirks-mode detection, UA default font sizes and families per tag, and a
  text remeasure pass.
- **Paint** — `font-kit` glyph rasterization (via `pathfinder_geometry`
  transforms) onto a BGRA software surface: text runs, backgrounds and
  background images, per-side borders, underline, clipping,
  `visibility:hidden`, text-transform, and the text-indent image-replacement
  idiom.
- **JavaScript** — `boa_engine` with a real DOM API surface (query and
  traversal, mutation, events, `document.cookie`, `localStorage`, `location`,
  `history`, `fetch`, `URL`/`URLSearchParams`, `TextEncoder`/`TextDecoder`,
  `AbortController`, `DOMParser`, `crypto`) and a **bounded** event loop: a
  timer queue kept separate from the job queue, at most one timer generation
  per tick, and a microtask drain that bails with a ledger line rather than
  pegging the thread.
- **Media passthrough** — Aether renders, **Stria plays**. Media the page's own
  markup already names (`<video>`, `<audio>`, `<source>`) is handed to Stria
  over the bandy Synapse as `SMessage::PlayMedia { url, title, mime }`. Aether
  embeds no decoder.

### Standing rule: never a site-specific resolver

Aether must **never** scrape or reverse a particular site's private API to
obtain a media stream. The only sanctioned path is the one above: hand the
page's own stream URL to Stria. A site-specific resolver is both a maintenance
trap and a ban risk, and is out of bounds no matter how convenient the target
site makes it.

## Architecture map (`src/`)

| Module | What it does today |
| --- | --- |
| `lib.rs` | `AetherEngine` — the whole page lifecycle: document, layout tree, JS engine, history, scroll, viewport, damage rects, surface, staged navigation and staged media. Shells drive it through `handle_event` / `tick` / `render_frame`. |
| `main.rs` | The `aether` binary: `open <url>` (handler mode on the Synapse) and `render` (the headless oracle, below). |
| `net/` | Fetching and resource discovery: URL normalization, the shared cookie jar and `document.cookie` read/write bridge, concurrent stylesheet/script/image fetches, image-kind sniffing, CSS `url()` reference collection, favicon resolution. |
| `css/` | Parsing (`cssparser`), the cascade, specificity ordering, `!important` tiers, the length/math evaluator, `@media` and `@supports` evaluation. The largest module in the crate. |
| `layout/` | DOM → box tree → `taffy` layout; `PaintStyle` (the specified paint properties per box), quirks-mode detection, UA default sizes/families, remeasure. |
| `render/` | Rasterization of the laid-out tree to the BGRA surface, with inherited paint state (color, font size, weight, line height, decoration, family, text-transform, button label centring), plus `dump_layout`. |
| `js/` | The boa bindings: per-thread DOM state and node-id mirror, event dispatch, `currentScript`, page URL, and the DOM/BOM API wiring. |
| `api/` | Web-platform API breadth split by area: `element`, `events`, `fetch` (with a per-page request budget), `window`, `platform` (URL/URLSearchParams, TextEncoder/TextDecoder, AbortController, DOMParser, crypto — real implementations over the same `url` and `kuchiki` the engine itself uses), `video`, and `a11y` (an accessibility-tree builder that nothing in the shell consumes yet). `cssom.rs` and `websockets.rs` are declared but stubbed to no-ops. |
| `event_loop/` | The timer queue and bounded job executor, plus the monotonic clock (`now_ms`, `advance_clock`, `freeze_clock` for tests) and `create_context`. Its module docs record why timers were moved off boa's job queue: a self-re-arming `setTimeout` used to make one `tick()` never return. |
| `images/` | Per-page decoded-image store keyed by absolute URL, `src` resolution against the page base, and SVG rasterization through `resvg`. Misses are ledgered (`img-missing`). |
| `fonts/` | Thin `font-kit` `SystemSource` wrapper — best-match family/properties lookup returning a loaded `Font`. |
| `forms/` | Form model (`Form`, `FormInput`, `HttpMethod`) and `OpenDocument`, the staged navigation record produced by a link click or form submit. |
| `storage/` | `localStorage` as a boa class, persisted to a JSON file under the platform data dir (`directories::ProjectDirs`). |
| `ledger/` | The API-coverage ledger (below): thread-local collection of missing DOM/CSS/JS features, with snapshot and file dump. |
| `headless.rs` | `render_headless_opts` — load (URL or local file), render one frame, write PNG + ledger. Shared by the CLI and the tests. |
| `workers/` | A minimal worker: a spawned thread with its own boa `Context`, driven by an mpsc channel pair. Not yet wired into the page's JS surface. |
| `dom/` | **Vestigial.** Six lines wrapping `kuchiki::parse_html`; the engine calls kuchiki directly. |
| `js_scratch.rs`, `api/scratch_fetch.rs` | **Vestigial.** Neither is declared in `lib.rs` or `api/mod.rs`, so neither compiles. Leftover boa API experiments. |
| `engine_tests.rs` | The crate's test suite (`#[cfg(test)]`). |

## The coverage ledger: unsupported features log, never silently no-op

The engine's central honesty rule is that a feature it does not implement
**records itself**. `ledger::record_dom`, `record_css`, and `record_js` collect
into a thread-local `ApiCoverageLedger` (the engine, cascade, and JS bindings
all run on one thread, so a thread-local is the honest single collection
point). A page that looks wrong therefore arrives with a list of *why*, instead
of a silent no-op that has to be rediscovered by bisecting the rendering.

### The headless oracle

```
cargo build --release -p aether --bin aether
target/release/aether render <url> --out page.png --ledger page.txt \
    [--scroll N] [--width W] [--height H] [--html <file>]
```

Loads a URL (or a local file with `--html`), renders exactly one frame, writes
the PNG and the ledger dump, and prints the surface size and the distinct
missing-API count. Defaults: 800×600 viewport, `aether-render.png`,
`aether-ledger.txt`, scroll 0. `--scroll` renders below the fold for audits.
This is the oracle every gap pass and regression check is written against.

### Debug knobs

All five are present in the tree as documented:

| Variable | Effect |
| --- | --- |
| `UNAOS_JSDEBUG=1` | Trace each script's index, head, and outcome to stderr. |
| `UNAOS_JSEVAL=<expr>` | Evaluate one expression against the booted page, post-boot, in the page's own realm. |
| `UNAOS_JSDUMP=<dir>` | Write every script's full source into that directory. |
| `UNAOS_JSPRELUDE=<file>` | Evaluate a script of our own *before* the page's, in the same realm. |
| `UNAOS_LAYOUTDUMP=<max-depth>` | Dump the computed box tree (tag, id/class, rect) to the given depth. |

## Dependencies

One line of justification per direct dependency in `handlers/aether/Cargo.toml`:

| Crate | Why |
| --- | --- |
| `boa_engine` | The JavaScript engine — pure Rust, embeddable, no JIT. |
| `boa_gc` | `Trace`/`Finalize` derives for the native classes exposed to JS (`api/element.rs`). |
| `html5ever` | **Unused direct dependency, flagged to the integrator for removal.** kuchiki carries its own (older) html5ever; the only mentions of the direct 0.39 dependency in this tree are comments explaining that version skew. |
| `kuchiki` | The DOM: HTML parsing, mutable node tree, and the servo selector matching engine the cascade compiles against. |
| `taffy` | Block and flexbox layout over the box tree. |
| `reqwest` | HTTP(S): async page and subresource fetches, a blocking client for sync paths, cookie-jar integration. |
| `tokio` | The async runtime the fetches and the handler's ignite loop run on. |
| `bandy` | The Synapse message bus — `SMessage::OpenDocument` in, `SMessage::PlayMedia` out, shell events across. |
| `gneiss_pal` | Platform abstraction layer. **Currently referenced nowhere in `src/`** — a dependency ahead of its use; deserves the same removal review as `html5ever`. |
| `anyhow` | Error propagation across the engine and the CLI. |
| `thiserror` | Declared for typed engine errors; **no current use in `src/`.** |
| `clap` | The binary's `open` / `render` subcommands and flags. |
| `image` | Raster decode (PNG/JPEG/…) and the headless PNG writer. |
| `serde`, `serde_json` | `localStorage` persistence and the a11y-tree node types. |
| `cssparser` | Tokenizing and parsing stylesheets and declaration blocks. |
| `selectors` | The servo selector types the cascade compiles and compares specificity with (shared with kuchiki). |
| `url` | The single URL implementation, used by the network stack, cookie scoping, and the JS `URL`/`URLSearchParams` APIs alike. |
| `font-kit` | System font enumeration, best-match selection, and glyph rasterization. |
| `pathfinder_geometry` | font-kit's own geometry types (`Transform2F`, vectors), required to call its rasterizer; no new transitive weight. |
| `base64` | Decoding `data:` URIs. |
| `resvg` | Rasterizing SVG images into the page image store. |
| `futures-util` | `join_all` and buffered streams for the concurrent subresource fetches. |
| `tokio-tungstenite` | Declared for the WebSocket API; **`api/websockets.rs` is still a stub and nothing in `src/` references the crate.** |
| `directories` | Platform data directory for the `localStorage` file. |

## Testing

```
cargo test -p aether
```

**72 passed, 0 failed** on the working tree at the time of writing. That tree
also carries other executors' uncommitted work across the engine and shell, so
the authoritative figure is the gate result at HEAD.

## Status and open gaps

- [`docs/dev/USERLAND/PLAN-aether-browser.md`](../../docs/dev/USERLAND/PLAN-aether-browser.md)
  — the milestone plan and development log, from the M0 charter amendment
  through the full-SPA milestone.
- [`docs/dev/USERLAND/AETHER-GAPS.md`](../../docs/dev/USERLAND/AETHER-GAPS.md)
  — the standing, verified gap list: capability-surface inventory, ledger
  aggregated across a page corpus, and the prioritized shortlist. **In flight
  at the time of writing** (present in the working tree, not yet committed).

## See also

- [`docs/CODEX.md`](../../docs/CODEX.md) — the handler manifest and the M0
  charter amendment.
- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
  — the handler/vessel/Bandy component model.
- [`handlers/stria/README.md`](../stria/README.md) — the audio handler that
  receives Aether's `PlayMedia` handoffs.

Edition: Rust 2024. License: LGPL-3.0-or-later.
