# PLAN — Aether becomes a real web browser (JS-enabled)

**Authority:** Peter, 2026-07-20 — UnaOS gets a web browser; Aether's read-only/no-JS
constraint is lifted by owner decision. This is a **charter change**, so it is recorded as
one (M0), not smuggled through a README edit.

**Repo:** UnaOS Ring-3 userspace. **Branch:** `gemini/aether-browser` off `main`; commit
only there; adversarial review before merge (criteria at the bottom — write to them).
**Bus reality (verified in `handlers/stria/src/lib.rs`):** the IPC substrate is bandy's
**Synapse** — `bandy::{BandyMember, SMessage, Synapse}` over tokio broadcast. Use it; do NOT
invent a new bus. Playback is **Stria's** domain (A/V handler, exists at `handlers/stria`) —
Aether renders and resolves; it hands media to Stria over `SMessage`. That division is the
whole reason this stays sane: Aether is the browser, Stria is the player.

## Why staged, and why NOT "load youtube.com's player" as the gate
A from-scratch engine that runs youtube.com's live SPA needs DOM+CSSOM, the event loop,
`fetch`/XHR, Media Source Extensions, WebCodecs, workers, WebCrypto, canvas — years, and the
gate is unfalsifiable until the very end. Instead every milestone ships a **visible, checkable
result**, and YouTube playback arrives at M4 via the media pipeline (resolve → Stria),
BEFORE the full-SPA milestone (M6). You get playing video early; the general browser grows
under it.

## Milestones (each: a lane, a deliverable, a checkable oracle)

### M0 — Charter amendment (design, not code)
Amend `docs/CODEX.md`: Aether's row changes from "read-only renderer, no JIT JS" to
"web browser; JS-enabled; delegates A/V playback to Stria." One paragraph in the manifest +
a dated amendment note. **This is the ONLY sanctioned way to change the constraint** — the
README follows the charter, never leads it. Oracle: the CODEX diff is the amendment; Peter
signs off on the wording before any code lands.

### M1 — Fetch + parse + static render (no JS yet)
- `src/net/` — `reqwest` (async, tokio) HTTPS GET; redirects, content-type, charset.
- `src/dom/` — `html5ever` → a mutable DOM tree (the living tree M3 will mutate).
- `src/layout/` — `taffy` block/flex layout → a display list (boxes + text runs + colors).
- `src/render/` — rasterize the display list to a framebuffer surface; publish it to the
  window compositor over `SMessage` (match how stria/vaire publish; check the SMessage enum
  for a surface/blit variant, add one if none exists — flag it for review).
- Oracle: `aether open <url>` renders a static page (pick 3 fixtures: a plain article, a
  CSS-flexbox layout, an image page) to a PNG dump AND to the live window; golden-file the
  display list for the fixtures (checked-in HTML, no network in the unit tests).

### M2 — Interaction shell
- `src/main.rs` — `ignite(synapse)` entry (stria's idiom): subscribe to
  `SMessage::OpenDocument { url }`, drive fetch→parse→layout→render; publish load state.
- Scroll, link navigation (hit-test the display list), back/forward, an address input.
- Oracle: mock-dispatch `OpenDocument`, assert a display list is produced and a clicked link
  emits the next `OpenDocument`.

### M3 — JavaScript engine (`boa_engine`)
- `src/js/` — `boa` context; bind a **real DOM API** onto the M1 tree (getElementById,
  querySelector, createElement, textContent, addEventListener, setAttribute, the mutations
  that trigger M1 re-layout), plus `fetch()` wired to `src/net`, timers, and `console`.
- Re-layout/re-render on mutation (dirty-flag the affected subtree; don't rebuild the world).
- Oracle: fixtures with `<script>` that mutate the DOM (append nodes, change text/style,
  `fetch` a checked-in JSON and render it) → assert the post-script display list matches a
  golden; event fixture (a click handler that mutates) round-trips.

### M4 — Media playback (page stream → Stria) — **the requested win, landed early**
**Revised 2026-07-27 (Peter):** no site-specific resolver. The earlier innertube
approach reverse-engineered YouTube's private API — a hack that (a) breaks the moment
YouTube changes its client contract (it since added PO-token attestation, so keyless
resolve is dead) and (b) walks straight into the plan's own legal ground rules. It is
**removed**. We own the browser AND the OS: a media element's source is already ours.
- `AetherEngine::media_sources()` — after a page loads, collect the resolved
  `<video>`/`<audio>`/`<source>` `src` (absolute URL + mime); hand each to **Stria**
  over `SMessage::PlayMedia { url, title, mime }` (variant already in bandy). Same
  passthrough as audio — Aether renders, Stria decodes/presents. Aether never decodes.
- This is media-source-agnostic: it plays whatever the page exposes, no per-site code.
  For sites that hide the stream behind a scripted player (YouTube's SPA), the media
  surfaces once M6's SPA/MSE path runs the player JS — at which point the SAME
  passthrough carries it to Stria. No special case, no private-API scraping, ever.
- Oracle: offline unit test asserts media_sources() resolves relative/absolute srcs and
  mimes; the attended check is a page with a direct `<video src>` PLAYING through Stria.

### M5 — CSS + web-platform breadth
- Real CSS cascade/specificity (`selectors` + a cascade over the taffy layout), fonts,
  images inline in layout, forms (input/submit → `OpenDocument`/POST). Grow the DOM/JS API
  surface driven by what real pages hit (measure against a fixture corpus; log unimplemented
  APIs rather than silently no-op'ing them).
- Oracle: a corpus of ~20 real static-ish pages renders recognizably; the unimplemented-API
  log is the honest coverage ledger (no silent gaps).

### M6 — Full-SPA target (youtube.com et al.) — explicitly the LAST, longest milestone
- MSE/WebCodecs-class media path, workers, storage, the deep event loop — grown demand-first
  from M5's coverage ledger. This is where "navigate youtube.com and stream from the site"
  becomes reachable, and it rides on top of everything proven below it.
- Oracle: the coverage ledger drives it; each capability added closes a named gap with a
  fixture. No "it works" without a fixture that failed before and passes after.

## Ground rules
- Rust, async/tokio, matching stria/vaire idiom (`ignite`, `BandyMember`, `SMessage`).
- Lane: `handlers/aether/**` + the CODEX amendment (M0) + any `SMessage` variant additions in
  `libs/bandy` (flag each addition explicitly for review — that's the one cross-lane touch,
  and it's additive). Nothing else outside the lane without STOP-and-report.
- Every dependency justified in one README line. Prefer in-repo `libs/` primitives where they
  exist (check before adding).
- Honest failure over silent no-op everywhere: unimplemented DOM/CSS/JS APIs LOG their name;
  resolver errors are typed; no fabricated success.
- Legal: public non-DRM streams; no cipher descrambling (that's the `ciphered-only` error
  path), no age-gate bypass, no download-to-disk verb.

## What the adversarial review will check (write to this)
1. M0 landed FIRST as a real CODEX amendment Peter approved — the README never leads the
   charter.
2. Lane discipline: only `handlers/aether/**`, the CODEX amendment, and flagged additive
   `SMessage`/bandy variants.
3. Playback delegates to Stria over `SMessage` — Aether does not embed a video decoder
   (charter division intact: Aether renders, Stria plays).
4. Every milestone's oracle is REACHABLE and was actually run; no milestone gated on M6.
5. Error/coverage honesty: typed resolver errors each reachable in a test; unimplemented-API
   ledger real, not empty-because-silent.
6. Fixtures genuine; unit tests hit no network; `--live` gated.
7. Dependency hygiene (each justified; no yt-dlp; no YouTube-specific crates).
8. Media plays at M4 through the page-stream→Stria passthrough (media_sources →
   `PlayMedia`) — NO site-specific resolver, no private-API scraping. A page with a
   direct `<video src>` plays through Stria before the M6 SPA work.

## Deliverables
Branch `gemini/aether-browser`, clean `aether:`-prefixed commits, README rewritten AFTER M0,
all tests green (`cargo test` in `handlers/aether`), and `handlers/aether/REPORT.md`: what
each milestone built, what was deliberately deferred, every deviation from this plan with its
reason and the fixture that proves it.
