# UnaOS Userspace Architecture

This document describes the UnaOS **userspace** — the handlers, views, vessels, and kits
built on `libs/`. It is the counterpart to the kernel documentation under
[`docs/dev/OS/`](../OS), which covers the bare-metal side in `unaos/`. The July 2026
model reconciliation that produced the current shape is recorded in
[`RECONCILIATION-2026-07.md`](RECONCILIATION-2026-07.md).

UnaOS is developed as two layers that build independently:

- **Kernel** (`unaos/`) — a bare-metal Rust operating system (boot, memory, SMP
  scheduler, drivers, network stack). Built for the `*-unaos` targets.
- **Userspace** — host-native programs that today run as ordinary macOS/Linux processes
  and converge onto the kernel as it matures. Described here.

A userspace binary on a host ships **gneiss, the Ring 3 kernel**, riding the host
natively: UnaOS gets inside the machine and improves it, rather than layering an entire
foreign userspace on top with a native-looking skin.

---

## 1. Component model

### One thing at three tempos

The central userspace object is a **composition**: a set of handlers bound to views. It
exists in three states:

| State | Name | What it is |
| --- | --- | --- |
| **Live** | an **elessar workspace** | The composition running under the workspace runtime, reshaping as context shifts. |
| **Saved** | a **kit** | A snapshot of an elessar workspace — a helpful starting point a user selects. |
| **Frozen-portable** | a **vessel** | A kit compiled into a standalone binary that runs without elessar and without UnaOS. |

On UnaOS, selecting a kit opens it live in elessar. On any other platform, the compiled
vessel is the onramp: try a workspace natively, no full install. Today's vessels are
hand-written (the kit→vessel compiler does not exist yet); they prototype what that
compiler must produce.

### Handlers (`handlers/`) — headless capabilities

A handler owns one capability area and is deliberately **headless**: it has no path to
the user. Vein can think but has no mouth; junct can receive from every network but has
nowhere to put a message. That headlessness is why capabilities compose.

The manifest (now **21**, LOCKED with one dated amendment) is canon in
[`docs/CODEX.md`](../../CODEX.md). Handlers live **flat** — each locked name is a
top-level subdir of `handlers/`, because the name *is* the charter; classification lives
inside each handler (e.g. `comscan/src/transport/{serial,gpio,bt,sdr}/` for the wires and
`comscan/src/caps/squawk/` for the telemetry capability). Notable pairs and roles:

- **junct / vein — symmetric platform abstractions.** junct abstracts human conversation
  networks (Matrix, Email, IRC, RSS → one Stream); vein abstracts AI providers
  (local/cloud → one conversation). Same shape, different party on the far end; both
  surface through the same shared chat view. Neither owns a chat UI.
- **comscan — purely the wire.** Transport in (all telemetry hands off to its `squawk`
  capability), transport out (helm-approved commands to actuators).
- **helm — control authority** (the 2026-07 amendment; see §5).
- **principia — the policy engine**, including the safety levels helm enforces (§5).

### Views (`libs/views/`) — the paths to the user

**Quartzite is the only path to the user**; a **view** is a specific path (the chat view,
the grid view, the viewport). Views are reusable base code by design — one chat view
serves junct and vein both — and live as crates under `libs/views/` (they depend on
quartzite but are not part of it). A vessel or elessar binds handlers to views; neither
handlers nor views know about each other directly (they meet on the Bandy bus).

### Vessels (`vessels/`) and kits (`kits/`)

**Vessels** are the frozen-portable compositions: today, hand-written executables that
wire a Tokio runtime, the bus, a selection of handlers, and a GUI window.

| Vessel | Composition |
| --- | --- |
| `lumen` | AI-centric companion (vein + junct + matrix + the chat view). The reference vessel. |
| `pulse` | System monitor: per-core CPU segment bars (BeOS Pulse heir). |
| `phonolite` | Tone vessel: the resonance engine given a face. Domain logic lifts to `stria` when it becomes a real handler. |
| `facet` | Image/raster viewer (lux decode → quartzite window, euclase GPU path). |
| `una` | Code-focused IDE. **Parked** (excluded from the workspace build). |

**Kits** are the saved compositions — elessar-workspace snapshots, users' starting
points, compiled per platform as vessels for the try-without-install onramp.

UnaOS deliberately avoids fixed-feature "apps": a composition binds handlers dynamically
rather than bundling one monolithic feature set. There is no app grid — the user surface
is *anarchy, not total chaos*: no imposed hierarchy, order emerging from composition,
with principia as the law everyone reads and helm as the one authority over consequences.

> **Migration note (2026-07).** The `apps/` split (correction 1) is executed: source
> vessels now live under `vessels/`, the command-line tools under `tools/`, and `kits/`
> holds the elessar-workspace snapshots (see RECONCILIATION-2026-07 §directory map).

### Libraries (`libs/` and `unaos/libs/`)

Host-native userspace libraries live at `libs/`:

| Crate | Role |
| --- | --- |
| `gneiss_pal` | The Ring 3 kernel: filesystem, networking, geometry, DSP, windowing, paths, persistence. Prevents handlers re-implementing host services. |
| `bandy` | The message bus (`SMessage`, `Synapse`) and shared domain state. |
| `quartzite` | The GUI layer: renders a workspace natively, routes input back as messages. |
| `views/*` | Reusable view crates (the chat view first). |
| `elessar` | The workspace runtime (today: context detection — the seed; see §4). |
| `euclase` | GPU rendering (WGPU). |
| `resonance` | Audio engine and DSP. |
| `fs/unafs` | UnaFS host client (wraps the shared format core with host I/O). |
| `lux` | Image decoding (incl. camera RAW). |

**Ring 0-embeddable cores** live under `unaos/libs/` in device-class subdirs — `no_std`,
`forbid(unsafe_code)`, pulled by the kernel with `default-features = false`: `fs/` (the
UnaFS format core both rings share), `input/` (ibus RC-receiver decode), `pwm/`
(actuators), and `sys/helm/` (the safety interlock — system authority, not a device
class). The path convention itself carries the ring distinction.

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
2. **Native distribution.** A kit compiles to a native vessel per host platform —
   the try-without-install onramp.

The convergence target is `platforms/unaos`: a backend rendering directly to the
kernel's framebuffer (`Screen` / `FrameBuffer`) and consuming USB HID input. The
kernel (bottom-up) and the GUI API (top-down) meet there.
`Spline::bootstrap(&WorkspaceState)` is the stable seam between *what to show* (a
workspace value) and *how to show it natively*; a future `platforms/unaos`
backend implements the same seam.

---

## 4. The workspace runtime: Elessar

**Elessar is the workspace runtime**: the thing that takes a composition and makes it
live — binding handlers to views and reshaping as context shifts (CODEX §4, "The
Binder"). It is only definable in terms of kits and vessels: a kit is a snapshot *of* an
elessar workspace; a vessel is a kit that traded liveness for portability.

Today `libs/elessar` implements the seed — context detection (`find_workspace_root()`,
`detect_spline(path)` → `UnaOS`/`Rust`/`Web`/`Python`/`Void`): detection is step one of
binding. The snapshot format and the kit→vessel compiler are future arcs; packaging is
expected to be owned by the `aule` handler.

---

## 5. Safety: law → authority → interlock

Anything an AI will drive — actuators first, other consequence domains as they arrive —
passes through three layers:

1. **principia states the law.** Safety levels are policy: user-chosen, per
   action-domain, from "never" through "ask" to "autonomous within bounds."
2. **helm holds control authority** (handler; the 2026-07 CODEX amendment). Every
   AI-initiated physical action passes through helm, which reads principia's levels and
   decides pass/ask/refuse. The helm is the wheel and the captain's voice: direct human
   control and commanded intent at one station, one authority deciding which is in
   effect.
3. **The kernel helm core does not negotiate.** `unaos/libs/sys/helm/` is the hard
   interlock beneath: DISARM/MANUAL/AUTO with a FAILSAFE latch and the
   transmitter-as-human-estop property. Per-machine domains (`src/rover/` first),
   because failsafes do not generalize — a rover's safe state is "stop"; a mill's is
   "retract and stop the spindle."

comscan carries the approved commands to the hardware and the telemetry back (squawk);
it gatekeeps nothing — the gate is helm's.

---

## 6. Data flow example

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

## 7. Implementation status

| Area | Status |
| --- | --- |
| Bandy message bus (`SMessage` / `Synapse`) | Implemented |
| Quartzite GUI API + macOS backend | Implemented |
| Quartzite GTK backend (Linux) | Implemented |
| Quartzite Qt backend | Partial |
| Quartzite Windows / `unaos` backends | Not implemented |
| Lumen vessel | Implemented |
| Pulse vessel (system monitor) | Implemented (macOS backend) |
| Phonolite vessel (tone generator) | Implemented (macOS backend) |
| Facet vessel (image viewer) | Implemented (macOS backend; FACET-2 zoom/pan/readout; euclase GPU path) |
| Una vessel (IDE) | Parked |
| Elessar snapshot / kit / vessel-compile pipeline | Not implemented (detection seed only) |
| `vessels/` + `kits/` + `tools/` directory migration | Executed (correction 1; `vessels/`, `tools/`, `kits/` on disk) |
| `libs/views/` chat-view extraction | Adopted, pending arc |
| helm core `unaos/libs/sys/helm/` move | Executed — core moved from `libs/drive` to `unaos/libs/sys/helm` (crate `helm`, module `rover`) |
| helm handler scaffold | Adopted, pending arc |

---

## See also
- [`docs/CODEX.md`](../../CODEX.md) — the system canon and full handler manifest.
- [`RECONCILIATION-2026-07.md`](RECONCILIATION-2026-07.md) — the decision record behind this model.
- [`docs/dev/OS/`](../OS) — kernel subsystem documentation.
- Per-crate `README.md` files under `libs/`, `handlers/`, `vessels/`, and `tools/`.
