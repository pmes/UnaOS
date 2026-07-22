# BRIEF — Aether via quartzite gems + grow tetra (proposal first)

**Ruling (Peter, 2026-07-22):** STOP reinventing. `platforms/{gtk,macos,qt}/browser.rs`
threw away gems that already exist and hand-rolled raw widgets — that is why it is blank
and unwired. Delete `browser.rs` on every platform. Reuse the gems; invent only what is
genuinely missing, following the established gem pattern; and grow `tetra` so gems
compose ONCE across platforms.

## The gem pattern (already established — follow it, do not invent a new one)
A gem = a `define_class!` native view + `bootstrap_<gem>(…, synapse) -> view` that wires
`SMessage` so **the vessel never touches AppKit/GTK**. Layout derives from live bounds
(no hardcoded pixel geometry). Two idioms already in the tree:
- **surface** — `macos/image_view.rs`, `macos/meter.rs`: self-drawn view, own pointer
  handling, fed pixels/data via a setter.
- **control** — `macos/tone_panel.rs`: AppKit target-action into a `define_class!` view
  whose ivars hold Rust callbacks.
Chrome already exists: `macos/window_chrome.rs` (NSToolbar), `gtk/mega_bar.rs`
(HeaderBar).

## Reuse first
- **Content view = `macos/image_view.rs`** — it is ALREADY a CPU-raster pixel blitter
  (RGBA) with zoom/pan; it is exactly the browser content surface. `facet` uses it.
  Use it (or its gtk twin); do NOT allocate a raw `NSImageView`. Add a gtk equivalent if
  one is missing, in the same shape.
- **URL field + nav buttons** = the `tone_panel` control idiom (target-action + Rust
  callbacks firing `SMessage::OpenDocument`/`BrowserNav*`). On gtk, the `mega_bar`/
  HeaderBar already hosts controls.
- **Titlebar/chrome** = `window_chrome` (mac) / `mega_bar` (gtk).

Invent a new gem ONLY where none exists (e.g. a single-line URL text-input control gem
if `tone_panel` doesn't cover it) — and build it as `define_class!` + `bootstrap_*`,
live-bounds layout, wired to Synapse. Reusable by any vessel, not Aether-specific.

## Grow tetra (the improvement)
Today gems are written twice (mac + gtk) with no shared vocabulary, so a vessel needs
platform-aware composition. `tetra::TetraNode` is the intended shared description but is
a stub (`Matrix/Stream/Empty`). Extend it into a real cross-platform node vocabulary —
enough to describe a chrome row of controls above a content surface — so the VESSEL
composes the browser once as tetra nodes and each platform's `spline`/translator maps
tetra → the native gems above. That is what makes them reusable gems instead of parallel
per-platform code. Aether becomes: build a tetra tree, hand it to `new_vessel`; zero
per-platform Aether code, zero `browser.rs`, zero vessel `build.rs`.

## Proposal first
`PROPOSALS/PROPOSAL-quartzite-gems.md` (STATUS: PROPOSED). Must show: (1) which existing
gems are reused and how, (2) any new gem's API in the established pattern, (3) the tetra
node additions and how spline/translator map them per platform, (4) Aether's composition
as ONE tetra tree. No `browser.rs`. Reviewed before code.

## Gate
Same vessel code → three native results: `cargo run -p aether-shell` (macOS),
`--features gtk` (Linux), `--features qt` (KDE); type `google.com` → renders + title
updates; zero per-platform Aether files. Oracles are Peter's per box — don't claim a
pass you didn't run there.
