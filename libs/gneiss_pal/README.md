# Gneiss PAL

**Gneiss PAL** To put it in plain English, write an app for jOS, and you write an app for them all. Our Platform Abstraction Layer is a rock-solid foundation for building native, multi-platform applications in the safety and security of Rust. It implements a Plug-in Architecture that separates core application logic from native frontend implementations. Yep - macOS, Windows, GTK, and Qt apps in one shot.

## Architecture

The architecture relies on two key traits defined in `gneiss_pal`:

1.  **`Platform` (The Host)**:
    -   Implemented by each frontend (e.g., `GtkPlatform`, `WinPlatform`).
    -   Exposes capabilities to the core (e.g., `set_title`).
    -   Provides an `as_any()` method to allow plugins to downcast and access platform-specific features (like attaching widgets).

2.  **`Plugin` (The Logic)**:
    -   Implemented by feature modules.
    -   Hooks into lifecycle events (`on_init`, `on_update`).
    -   Connects the shared core logic to the specific platform implementation.

### Directory Structure

*   **`core/`**: Shared traits (`App`, `Platform`, `Plugin`) and helpers.
*   **`x/gtk/`**: GTK4 + Libadwaita frontend skeleton (`gneiss_gtk`).
*   **`x/gtk_pvp/`**: A functional verification app (`gneiss_demo_gtk`) running a video player inside the architecture.
*   **`x/qt/`**: Qt 6 + CXX-Qt skeleton (`gneiss_qt`).
*   **`mac/`**: macOS (Cocoa/AppKit) skeleton (`gneiss_mac`).
*   **`win/`**: Windows (Win32) skeleton (`gneiss_win`).

## Getting Started

### Prerequisites
*   Rust (stable)
*   **GTK**: `libgtk-4-dev`, `libadwaita-1-dev`
*   **Qt**: `qt6-base-dev`, `qt6-declarative-dev` (for Qt skeleton)
*   **MPV**: `libmpv-dev` (for the demo)

### Running the Demo (GTK)

To see the template in action:

```bash
cargo run --manifest-path Cargo.toml -p gneiss_demo_gtk -- --debug path/to/video.mp4
```

### Extending

To add a new feature:
1.  Create a struct implementing the `gneiss_pal::Plugin` trait.
2.  In `on_init`, use `platform.as_any().downcast_ref::<SpecificPlatform>()` to attach UI elements.
3.  Register the plugin in your frontend's `main.rs`.
