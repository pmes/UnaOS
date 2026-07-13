# UnaOS Userspace Architecture

This document describes the UnaOS **userspace** — the application environment in
`apps/`, `handlers/`, and `libs/`. It is the counterpart to the kernel
documentation under [`docs/dev/OS/`](../OS), which covers the bare-metal side in
`unaos/`.

UnaOS is developed as two layers that build independently:

- **Kernel** (`unaos/`) — a bare-metal Rust operating system (boot, memory, SMP
  scheduler, drivers, network stack). Built for the `*-unaos` targets.
- **Userspace** — a set of host-native programs that today run as ordinary
  macOS/Linux processes and are intended to converge onto the kernel as it
  matures. Described here.

---

## 1. Component model

Userspace is organized into three kinds of crate.

### Libraries (`libs/`)
Shared infrastructure used by handlers and vessels.

| Crate | Role |
| --- | --- |
| `gneiss_pal` | Platform abstraction layer: filesystem, networking, geometry, DSP, windowing, paths, persistence. The common foundation that prevents handlers from re-implementing host services. |
| `bandy` | The message bus (`SMessage`, `Synapse`) and shared domain state (`AppState`, `WorkspaceState`, `HistoryItem`). |
| `quartzite` | The GUI layer. Renders a workspace natively on the host and routes input back as messages. |
| `elessar` | Workspace/context detection. |
| `euclase` | GPU rendering (WGPU): shaders, render graph, and the presentation layer (`Cortex` + textured `quad` over a CAMetalLayer) — first live consumer is `facet`. |
| `resonance` | Audio engine and DSP. |
| `unafs` | Virtual filesystem client. |
| `lux` | Image decoding (incl. camera RAW). |

### Handlers (`handlers/`)
Domain services — each owns one capability area. A handler is a self-contained
crate exposing an async entry point (by convention `ignite(...)`) plus its
logic. Some are implemented; some are design-stage (README only). Examples:

| Handler | Capability |
| --- | --- |
| `vein` | AI / LLM integration and provider abstraction. |
| `matrix` | Spatial file model (workspace topology tree). |
| `midden` | Shell / command interpreter. |
| `principia` | System configuration and policy. |
| `tabula` | Text and code editing. |
| `vaire` | Version control (Git). |
| `amber_bytes` | Disk forensics: partition/block-level recovery ("The Block"). |
| `stria` | Audio/video editing. |
| `junct` | Communications (messaging, email, RSS). |

The full set ("the 20") is enumerated in [`docs/CODEX.md`](../../CODEX.md).

### Vessels (`apps/`)
The executables a user runs. A vessel wires together a Tokio runtime, the
message bus, a selection of handlers, and a GUI window.

| Vessel | Role |
| --- | --- |
| `lumen` | AI-centric companion. The reference GUI vessel. |
| `pulse` | System monitor: numbered per-core CPU segment bars (BeOS Pulse heir). Host sampler behind the `PulseSource` seam; a UnaOS-kernel telemetry feed is the banked replacement source. |
| `phonolite` | Tone vessel: the resonance engine given a face — start/stop, log-scale frequency + gain sliders (quartzite's first input-control surface, `tone_panel`), bus-routed level ladder. Domain logic lifts to `stria` when it becomes a real handler. |
| `una` | Code-focused IDE. **Currently parked** (excluded from the workspace build). |
| `facet` | Image / raster viewer: `facet <image>` decodes via `lux` (PNG/JPEG/ARW), packs linear→sRGB, and shows the picture in a quartzite window — GPU textured quad via euclase by default (CAMetalLayer, sRGB), CPU `image_view` blit as automatic fallback or via `FACET_CPU=1` — with zoom-about-cursor, drag-pan, reset-to-fit (`0`/`f`), and a live per-pixel readout (sRGB + source linear). |
| `apps/cli/*` | Command-line tools: `unafs`, `vertex`, `sentinel`, `unafs_bench`. |

UnaOS deliberately avoids fixed-feature "apps": a vessel composes handlers
dynamically rather than bundling one monolithic feature set.

---

## 2. The message bus: Bandy

`libs/bandy` defines how userspace components communicate.

- **`SMessage`** — a single enum enumerating every message type in the system
  (system heartbeat, AI prompts/tokens, storage queries, terminal I/O, UI
  events, …). Adding a variant is a deliberate, reviewed change. Representative
  variants: `UserPrompt(String)`, `AiToken(String)`, `StateInvalidated`,
  `Matrix(MatrixEvent)`, `Principia(PrincipiaCommand)`,
  `StorageQuery{…}` / `StorageQueryResult{…}`, `TerminalOutput(String)`.
- **`Synapse`** — a thin wrapper over a Tokio broadcast channel (buffer 1024).
  `fire(msg)` publishes; `subscribe()` returns a receiver. It is multi-producer /
  multi-consumer: any handler or the GUI can publish and subscribe.
- **Shared state** — `bandy::state` holds the cross-cutting domain types passed
  between logic and UI: `AppState`, `HistoryItem`, and `WorkspaceState`
  (`ViewEntity`, `TopologyState`, `StreamState`). These derive
  `Serialize`/`Deserialize`.

Handlers do not call each other directly; they publish and subscribe to
`SMessage` on the Synapse. This decouples the logic layer from the UI and from
other handlers.

---

## 3. The GUI layer: Quartzite

`libs/quartzite` is the GUI API. It renders a `WorkspaceState` natively on the
host and routes user input back to the logic layer as `SMessage`s.

### Public API
- `Backend::new(app_id, …, bootstrap).run()` — creates the host
  application/window and runs the platform event loop.
- `Backend::new_vessel(app_id, title, content_size, build_view).run()` — the
  lightweight sibling for single-view vessels (e.g. `pulse`): one plain titled
  window whose entire content is the view returned by `build_view` (macOS
  backend; other backends as they mature).
- `Spline::bootstrap(window, tx_event, app_state, rx_synapse, &workspace_state)
  -> BootstrapPayload` — builds the native view tree for a given workspace and
  wires it to the message bus.
- `NativeWindow` / `NativeView` / `BootstrapPayload` — platform type aliases
  selected by `#[cfg]`.

### Platform backends
Quartzite selects exactly one backend at compile time (`src/platforms/`):

| Backend | Path | Status |
| --- | --- | --- |
| macOS (AppKit, `objc2`) | `platforms/macos` | Implemented |
| Linux (GTK4 + libadwaita) | `platforms/gtk` | Implemented |
| Qt (CXX-Qt) | `platforms/qt` | Partial |
| Windows | `platforms/windows` | Stub |
| `unaos` (native kernel) | `platforms/unaos` | Reserved — not implemented |

### Strategy: why multiple backends
The kernel now boots, so UnaOS will soon need its own GUI, and Quartzite is that
API. The host backends serve two purposes:

1. **Proving ground for the API.** Building against mature toolkits forces the
   abstraction to be correct before it is implemented on bare metal.
2. **Native distribution.** A workspace can, in principle, be compiled and
   shipped as a native app per host platform.

The convergence target is `platforms/unaos`: a backend rendering directly to the
kernel's framebuffer (`Screen` / `FrameBuffer`) and consuming USB HID input. The
kernel (bottom-up) and the GUI API (top-down) meet there.
`Spline::bootstrap(&WorkspaceState)` is the stable seam between *what to show* (a
workspace value) and *how to show it natively*; a future `platforms/unaos`
backend implements the same seam.

---

## 4. Workspace / context: Elessar

`libs/elessar` determines the working context. Today it implements project-type
detection: `find_workspace_root()` locates the project root, and
`detect_spline(path)` classifies it (`UnaOS` / `Rust` / `Web` / `Python` /
`Void`). The broader intent — capturing a workspace as a serializable snapshot
for compilation and packaging into a native app — is **not yet implemented**;
packaging is expected to be owned by the `aule` handler.

---

## 5. Data flow example

A user prompt in Lumen:

1. The GUI publishes `SMessage::UserPrompt("…")` on the event channel.
2. Lumen's brain loop / `VeinHandler` receives it and queries the configured LLM
   provider.
3. Vein streams the response as a sequence of `SMessage::AiToken("…")` on the
   Synapse.
4. The GUI, subscribed to the Synapse, updates the chat view incrementally as
   each token arrives.

Other handlers observe the same bus — e.g. Matrix maintains the workspace
topology and republishes `MatrixEvent::TopologyMutated` when the tree changes.

---

## 6. Implementation status

| Area | Status |
| --- | --- |
| Bandy message bus (`SMessage` / `Synapse`) | Implemented |
| Quartzite GUI API + macOS backend | Implemented |
| Quartzite GTK backend (Linux) | Implemented |
| Quartzite Qt backend | Partial |
| Quartzite Windows / `unaos` backends | Not implemented |
| Lumen vessel | Implemented |
| Pulse vessel (system monitor) | Implemented (macOS backend) |
| Phonolite vessel (tone generator) | Implemented (macOS backend; resonance engine honest — device-rate graph, live command path, level readback) |
| Facet vessel (image viewer) | Implemented (macOS backend; FACET-1 MVP — lux decode → sRGB pack → CPU-blit aspect-fit view; FACET-2 — zoom-about-cursor / drag-pan / reset-to-fit / live per-pixel readout. Euclase GPU path later) |
| Una vessel (IDE) | Parked |
| Elessar snapshot / packaging pipeline | Not implemented (detection only) |

---

## See also
- [`docs/CODEX.md`](../../CODEX.md) — the system canon and full handler manifest.
- [`docs/dev/OS/`](../OS) — kernel subsystem documentation.
- Per-crate `README.md` files under `libs/`, `handlers/`, and `apps/`.
