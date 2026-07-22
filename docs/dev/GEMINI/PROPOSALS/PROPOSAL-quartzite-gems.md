# PROPOSAL: Aether via Quartzite Gems & Tetra Expansion
**STATUS: PROPOSED**

## 1. Gem Reuse
Instead of hand-rolling widgets per platform in a monolithic `browser.rs`, we will decouple and reuse existing native "gems" to form the browser:
- **Content Surface:** We will reuse `macos/image_view.rs` (and introduce its GTK twin if missing, utilizing the Cairo drawing area logic we just built). It is already a CPU-raster pixel blitter designed to receive `SMessage::SurfaceBlit` and emit input events.
- **Chrome/Titlebar:** We will reuse `macos/window_chrome.rs` (NSToolbar) and `gtk/mega_bar.rs` (HeaderBar) to host the window controls.
- **Controls (Target-Action):** We will reuse the `tone_panel.rs` control idiom (wiring native target-actions to Rust closures that fire `SMessage`) to create decoupled Button and TextField gems.

## 2. New Gems and API Patterns
We will invent two missing generic control gems following the established `define_class!` (macOS) and subclassing (GTK) patterns:

### `TextField` Gem
A single-line text input control.
- **API:** `bootstrap_text_field(placeholder: &str, on_commit: impl Fn(String) -> SMessage, synapse: Synapse) -> NativeView`
- **Behavior:** Renders `NSTextField` / `gtk4::Entry`. On enter/commit, it fires the returned `SMessage` (e.g. `SMessage::OpenDocument { url }`) via the synapse.

### `Button` Gem
A simple clickable button.
- **API:** `bootstrap_button(label: &str, action: SMessage, synapse: Synapse) -> NativeView`
- **Behavior:** Renders `NSButton` / `gtk4::Button`. On click, it fires the specified `action` over the synapse (e.g. `SMessage::BrowserNavBack`).

## 3. Tetra Node Additions & Spline Mapping
We will expand the `tetra::TetraNode` vocabulary to natively express cross-platform layout and UI controls:

```rust
pub enum TetraNode {
    // ... existing variants (Matrix, Stream, Empty) ...

    // Layout
    VStack(Vec<TetraNode>),
    HStack(Vec<TetraNode>),

    // Controls
    Button { id: String, label: String, action: SMessage },
    TextField { id: String, placeholder: String }, 
    Surface { id: String }, // Maps to image_view
}
```

### Spline / Translator Mapping
`Backend::new_vessel` (and eventually the Workspace `Spline`) will be modified to accept a `TetraNode` tree instead of a generic UI closure. The backend's translator will recursively map the nodes:
- `VStack` / `HStack` → `NSStackView` (macOS) / `gtk4::Box` (GTK).
- `Button` → invokes `bootstrap_button`.
- `TextField` → invokes `bootstrap_text_field`, wrapping the text payload into `SMessage::OpenDocument { url: text }` (or driven by ID).
- `Surface` → invokes `image_view::bootstrap_image_view`.

## 4. Aether's Composition (ONE Tetra Tree)
`browser.rs` will be deleted entirely from all platforms. `aether-shell`'s `main.rs` will declaratively construct its UI using the expanded `TetraNode` vocabulary:

```rust
let aether_ui = TetraNode::VStack(vec![
    TetraNode::HStack(vec![
        TetraNode::Button { id: "back".into(), label: "<".into(), action: SMessage::BrowserNavBack },
        TetraNode::Button { id: "fwd".into(), label: ">".into(), action: SMessage::BrowserNavForward },
        TetraNode::Button { id: "reload".into(), label: "C".into(), action: SMessage::BrowserNavReload },
        TetraNode::TextField { id: "url".into(), placeholder: "Enter URL...".into() },
    ]),
    TetraNode::Surface { id: "viewport".into() }
]);

let gui = quartzite::Backend::new_vessel(
    "org.unaos.aether",
    "Aether",
    (800.0, 600.0),
    aether_ui // Replaces the platform-specific UI closure
);
```
