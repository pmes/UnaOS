# Pivot Aether Shell to Quartzite (STATUS: PROPOSED)

This plan describes the architectural pivot for the Aether shell, removing bespoke pixel-painted chrome in favor of `quartzite`'s native GUI capabilities (`new_vessel`), ensuring the browser uses real platform widgets (GTK/AppKit) while the engine renders the content view.

## Open Questions

1. **Wiring Mode:** The `PLAN-GEMINI-aether-shell-pivot.md` document mentions wiring chrome via `bandy SMessage`. `SMessage` currently has `OpenDocument` and `SurfaceBlit`. Should I also add `BrowserNavBack` and `BrowserNavForward` to `SMessage` to handle the history buttons, or should `aether-shell` pass a shared `Rc<RefCell<AetherEngine>>` directly into the quartzite bootstrapper like `pulse` does with its state? (Using `SMessage` is the purest interpretation of "quartzite's contract", but passing an engine reference is much faster for high-frequency events like scroll/mouse move).
2. **GTK `new_vessel` API:** GTK currently lacks `Backend::new_vessel`. I will implement it mirroring the macOS version, where it accepts a `build_view` closure. Is it acceptable to use standard GTK `ApplicationWindow` for this, similar to `Backend::new`?

## Proposed Changes

### `libs/bandy`
Extend `SMessage` to cover the full browser chrome contract if we are routing *everything* through the bus.
#### [MODIFY] `libs/bandy/src/signals.rs`
- Add `BrowserBack`, `BrowserForward`, `BrowserReload` variants.
- Add input variants for the content view (e.g., `BrowserScroll(dx, dy)`, `BrowserClick(x, y)`).

### `libs/quartzite`
Introduce the browser widget bootstrappers and add the missing `new_vessel` backend for GTK.

#### [MODIFY] `libs/quartzite/src/lib.rs`
- Export a new `pub mod browser` (similar to `meter`).

#### [NEW] `libs/quartzite/src/browser/mod.rs`
- Define the cross-platform `bootstrap_browser` signature.

#### [MODIFY] `libs/quartzite/src/platforms/gtk/mod.rs`
- Implement `Backend::new_vessel` for GTK.

#### [NEW] `libs/quartzite/src/platforms/gtk/browser.rs`
- Implement `bootstrap_browser`. Constructs a `gtk4::Box` containing `gtk4::Button`s (Back/Forward), `gtk4::Entry` (URL bar), and a `gtk4::DrawingArea` for the content view.
- Wire GTK signals to emit `SMessage` on the synapse and subscribe to `SurfaceBlit` to update the `DrawingArea` via `cairo::ImageSurface`.

#### [NEW] `libs/quartzite/src/platforms/macos/browser.rs`
- Implement `bootstrap_browser`. Constructs an `NSStackView` containing an `NSTextField` (URL), `NSButton`s, and a custom `NSView`/`NSImageView` for the content view.
- Wire AppKit delegates to emit `SMessage` and subscribe to `SurfaceBlit` to update an `NSImage`.

### `vessels/aether-shell`
Gut the bespoke code and wire the vessel through `quartzite`.

#### [DELETE] `vessels/aether-shell/src/shell_macos.rs`
#### [DELETE] `vessels/aether-shell/src/shell_gtk.rs`
#### [MODIFY] `vessels/aether-shell/src/main.rs`
- Remove the `gtk` and `softbuffer` dependencies.
- Initialize `quartzite::Backend::new_vessel("org.unaos.aether", "Aether Browser", (800.0, 600.0), |window| bootstrap_browser(window, tx, rx))`.
- Run an async loop that listens to `SMessage::OpenDocument`, `BrowserBack`, `BrowserScroll`, etc. It will drive the `AetherEngine` (fetch HTML, layout, render) and emit `SMessage::SurfaceBlit` with the resulting pixels back to the bus.

## Verification Plan

### Automated Tests
- `cargo test -p aether` must pass.
- `cargo check -p aether-shell` for GTK.

### Manual Verification
- Compile and run `aether-shell` on GTK (and macOS if possible).
- Verify the window has native buttons and text field.
- Verify typing a URL in the text field and pressing Enter fetches the page and renders it in the content view below.
- Verify the back/forward buttons function correctly.
- Ensure the viewport doesn't blank out on repaint (confirming our previous fix survives the refactor).
