# Lumen

The AI-centric companion vessel for UnaOS, and the reference GUI vessel: it wires together a Tokio runtime, the Bandy message bus, a set of domain handlers (Vein, Matrix, Amber Bytes), and a native Quartzite window into a single runnable application.

## What it is

In UnaOS terminology a **vessel** is an executable a user runs (see [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)). Rather than bundling one monolithic feature set, a vessel composes shared libraries and capability **handlers** at startup. Lumen is the canonical example: a binary crate (`fn main`) whose job is wiring and lifecycle, not domain logic.

## Responsibilities

Lumen owns the process boot sequence and the top-level event plumbing:

- **Runtime and lifecycle.** Starts a Tokio runtime, installs the default `rustls` crypto provider, and runs a signal interceptor that converts `SIGINT`/`SIGTERM` into a broadcast shutdown so every spawned task can drain cleanly.
- **Bus construction.** Creates the `Synapse` — the in-process broadcast bus carrying `SMessage` values — and hands clones to each handler and to the GUI.
- **Handler ignition.** Spawns the async entry point (`ignite(...)`) of each composed handler as a Tokio task: Amber Bytes (storage), Matrix (workspace topology), and Vein (AI/LLM) via `VeinHandler`. It also runs an internal **cortex** loop that records bus traffic to a UnaFS-backed vault.
- **GUI wiring.** Builds the initial `WorkspaceState`, constructs a Quartzite `Spline`, and launches the native window through `quartzite::Backend`. UI input flows back to logic as `SMessage`s.
- **The brain loop.** A central `tokio::select!` loop that mediates between UI events and the bus — handling workspace-structure mutations (topology toggle and graft) locally and forwarding everything else to `VeinHandler`.

## Boot sequence (`src/main.rs`)

1. Start the Tokio runtime and spawn the `SIGINT`/`SIGTERM` → `shutdown_tx` interceptor.
2. Resolve on-disk paths via `gneiss_pal::paths::UnaPaths` (a primary vault and a subconscious vault) and start telemetry logging.
3. Create the `Synapse` and install the `rustls` ring provider.
4. Spawn the handler tasks: `core::ignite` (the cortex recorder), `amber_bytes::ignite`, and `matrix::ignite`. The absolute workspace root is resolved once via `elessar::find_workspace_root()` and shared as an `Arc`.
5. Construct the shared `AppState`, the UI event channel (`async_channel`), and the initial `WorkspaceState` (a Matrix topology tree in the left pane, a stream view in the right).
6. Build `VeinHandler::new(...)` and spawn the **brain loop**.
7. Create the `Spline`, define the platform `bootstrap` closure, and call `Backend::new(...).run()` — this blocks on the native event loop until the window closes.
8. On exit, broadcast shutdown and await the handler tasks before returning.

## Prompt → AiToken data flow

This is the path Lumen exists to serve:

1. The Quartzite GUI publishes `SMessage::UserPrompt("…")` on the UI event channel.
2. The brain loop forwards it to `VeinHandler::handle_event`, which queries the configured LLM provider.
3. Vein streams the reply back as a sequence of `SMessage::AiToken("…")` on the `Synapse`.
4. The GUI, subscribed to the `Synapse`, appends each token to the chat view as it arrives.

In parallel, the cortex loop (`src/core.rs`) subscribes to the same bus and persists stimuli — prompts, file events, and log lines — into a UnaFS substrate, independent of the UI.

## Key types and entry points

- `fn main()` — the process entry point and wiring described above.
- `core::ignite(vault_path, synapse, shutdown_rx)` — the autonomous cortex loop that records bus traffic.
- `cortex::Cortex` — `awaken(...)` mounts (or formats) the UnaFS vault; `imprint(key, data)` burns a record into it.
- Composed from sibling crates: `bandy::{Synapse, SMessage}` (bus), `vein::VeinHandler` (AI), `matrix::MatrixScanner` (topology), `quartzite::{Backend, Spline, NativeWindow}` (GUI), and `bandy::state::{AppState, WorkspaceState}` (shared state).

## How it fits into UnaOS

Lumen is the reference **vessel** in the userspace layer documented in [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md): it composes `libs/` infrastructure and `handlers/` domain services over the Bandy bus and renders through the Quartzite GUI API. See [`docs/CODEX.md`](../../docs/CODEX.md) for the full system canon and handler manifest.

## Build features

Platform GUI backends are selected via Cargo features, each forwarding to the corresponding `quartzite` backend: `gtk`, `gnome`, `qt`, and `macos`. The macOS path uses a distinct `bootstrap` closure signature (gated by `#[cfg]`).

## Status

Implemented. Lumen boots, composes the Vein / Matrix / Amber Bytes handlers over the Synapse, and renders a native Quartzite window with the prompt → AiToken loop wired end to end. GUI backend maturity follows Quartzite (macOS and GTK implemented; Qt partial).
