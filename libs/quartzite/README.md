# quartzite

The UnaOS GUI layer: a multi-platform API that renders a workspace natively on
the host and routes user input back to the logic layer as messages.

## What it does

Quartzite is the seam between *what to show* (a `WorkspaceState` value, owned by
the logic layer) and *how to show it natively* (host toolkit widgets). It does
not draw its own pixels or carry application logic; instead it selects one
host backend at compile time and builds that toolkit's real view tree from the
workspace, then wires the views to the message bus so that user actions become
`SMessage`s and incoming `SMessage`s update the views.

Responsibilities:

- Construct the host application object and main window, and run the platform
  event loop.
- Translate a `bandy::state::WorkspaceState` into a native view tree (its layout
  intent is captured by the `tetra` module — `WorkspaceTetra` / `TetraNode` /
  `StreamTetra`).
- Connect that view tree to Bandy — UnaOS's in-process message bus — so the
  GUI publishes user input on an `async_channel::Sender<SMessage>` and consumes
  state changes from a `Synapse` broadcast receiver (`StateInvalidated` and
  related events drive re-renders).
- Embed and register host assets (fonts, GTK `GResource` icon bundles, CSS).

## Key public API

Entry points (`src/lib.rs`, `src/spline.rs`):

- `Backend::new(app_id, …, bootstrap).run()` — creates the host application and
  window, then runs the native event loop. The exact `new` signature is
  platform-specific: the macOS backend threads the bus handles
  (`tx_event`, `app_state`, `rx_synapse`, `workspace_state`) through to the
  `bootstrap` closure; the GTK backend takes the simplified
  `FnOnce(&NativeWindow) -> NativeView` form.
- `Spline::new()` / `Spline::bootstrap(window, tx_event, app_state, rx_synapse,
  &workspace_state) -> BootstrapPayload` — builds the native view tree for a
  given workspace and binds it to the bus. This is the stable seam every backend
  implements.
- `BootstrapPayload`, `NativeWindow`, `NativeView` — platform type aliases
  resolved by `#[cfg]` (e.g. `NSView` / `NSWindow` on macOS, `gtk4::Widget` /
  `ApplicationWindow` on Linux). On macOS the payload also carries the
  `SidebarDelegate` and `CommsDelegate` so the caller can keep them alive.
- `init()` / `init_with_path(path)` / `deploy_assets(path)` — asset registration;
  active on GTK, no-ops elsewhere (host bundles handle their own resources).

Supporting modules: `tetra` (serializable layout description), `text`
(`ab_glyph` text measurement over an embedded font), `widgets` (GTK-only view
models such as `DispatchRecord`, telemetry, scrollable text).

## Platform backends

Exactly one backend compiles per target (`src/platforms/`):

| Backend | Path | Toolkit | Status |
| --- | --- | --- | --- |
| macOS | `platforms/macos` | AppKit via `objc2` | Implemented |
| Linux | `platforms/gtk` | GTK4 (+ libadwaita under the `gnome` feature) | Implemented |
| Qt | `platforms/qt` | CXX-Qt / QML | Partial |
| Windows | `platforms/windows` | WinUI 3 / Win32 | Stub |
| `unaos` | reserved | kernel framebuffer + USB HID | Not yet present |

The selection is driven by `target_os` and Cargo features (`gtk`, `gnome`, `qt`,
`macos`); a headless fallback `Backend` compiles when no host backend matches.

## How it fits into UnaOS

Quartzite is one of the userspace libraries under `libs/`. Vessels (the
executables in `vessels/`, e.g. `lumen`) compose a Tokio runtime, the Bandy bus,
and a set of handlers, then call `Backend::new(...).run()` to present the
window. The same workspace can in principle be compiled per host, which keeps
the API honest against mature toolkits before it is implemented on bare metal.
The convergence target is a `platforms/unaos` backend that renders directly to
the kernel `Screen` / `FrameBuffer` and consumes USB HID input, so the
bottom-up kernel and the top-down GUI API meet at `Spline::bootstrap`. See
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
(§3) and [`docs/CODEX.md`](../../docs/CODEX.md).

> Note: an earlier JSON-DSL proc-macro experiment for describing UI has been
> retired. The Rust API above (`Backend` + `Spline`) is the supported surface.

## Status

Implemented (macOS AppKit, Linux GTK4) · Partial (Qt) · Stub (Windows) ·
Design-stage (`platforms/unaos` native kernel backend, the convergence target).
