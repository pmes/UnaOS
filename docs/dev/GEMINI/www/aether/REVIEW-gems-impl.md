# REVIEW — gems impl (commit eaaac7cb): broken on macOS again + one design gap

Structure is right: `browser.rs` deleted both platforms, `Button`/`TextField`/`Surface`
tetra nodes, `tetra_eval` translators, `TextAction` (A1 ✓ — text→SMessage is explicit,
not id-based), `new_vessel` closure preserved so pulse is intact (A2 ✓). But the macOS
lib does not compile — 3 errors, pushed as if it passed. It does not.

## Fix these
1. **`objc2::Allocated` → `objc2::rc::Allocated`** in `platforms/macos/button.rs:2` and
   `text_field.rs:2`. Trivial — `image_view.rs`/`meter.rs` already import it that way.

2. **`bootstrap_image_view(synapse)` is wrong — signature AND semantics.**
   `image_view::bootstrap_image_view` is facet's STATIC viewer:
   `(window, rgba: &[u8], linear: &[f32], width, height)` — fed ONE decoded frame at
   bootstrap, no bus subscription. The browser `Surface` needs a **live** raster that
   subscribes to `SMessage::SurfaceBlit` and re-draws on each frame. That capability
   does not exist yet. Decide and state it:
   - **(a)** add a live-blit entry to image_view — e.g. `bootstrap_image_surface(window,
     synapse, id) -> NSView` that reuses image_view's raster/zoom/pan draw core but
     subscribes to `SurfaceBlit` (matched by id) and calls its existing set-pixels path
     on each message; OR
   - **(b)** a distinct `Surface` gem that shares the raster core.
   Either is fine — (a) is less code. Do NOT call the static viewer with a synapse; it
   cannot consume blits. The gtk side needs the same live-blit surface (its
   `image_view.rs` twin must subscribe too).

## Process
You cannot compile the macOS backend on Linux — so for any change touching
`platforms/macos/*`, say "NOT COMPILED HERE — Mac owed," do not imply a pass. This is
the third macOS-broken push claimed as done.

## Gate
`cargo check -p aether-shell` green on a Mac AND `--features gtk` on Linux, both pasted.
Then the oracle: `cargo run` each, type `google.com`, page renders in the live surface +
title updates.
