# bandy

Bandy is the userspace message bus for UnaOS: the `SMessage` enum, the `Synapse`
broadcast channel, and the shared domain-state types that handlers and vessels
exchange.

## Responsibilities

Bandy defines *how* userspace components communicate and *what* they may say. It
provides three things:

1. **A single message vocabulary.** `SMessage` enumerates every message type in
   the system — there is one shared enum rather than per-pair protocols.
2. **A transport.** `Synapse` is a thin multi-producer / multi-consumer
   broadcast channel. Any handler or the GUI can publish and subscribe.
3. **Shared domain state.** The cross-cutting types passed between logic and UI
   (history, workspace layout, application state).

Handlers do not call each other directly. They publish and subscribe to
`SMessage` values on the Synapse, which decouples the logic layer from the UI
and from other handlers.

## Key public types and entry points

### Messages — `bandy::signals`
- **`SMessage`** — the central enum. Variants cover the system heartbeat
  (`Ping`, `Kill`, `Log`), AI flow (`UserPrompt`, `AiToken`,
  `ContextTelemetry`), storage requests/replies (`StorageQuery` /
  `StorageQueryResult`, `StorageSave` / `StorageSaveResult`,
  `StorageLoadPaged` / `StorageLoadPagedResult`), terminal I/O
  (`TerminalOutput`, `TerminalError`), file events, and UI events
  (`Input`, `NavSelect`, `ToggleSidebar`, `UiReady`, …). It derives `Clone`,
  `Serialize`, and `Deserialize`.
- **`PrincipiaCommand`** and **`MatrixEvent`** — sub-enums carried by the
  `SMessage::Principia` and `SMessage::Matrix` variants, namespacing the
  configuration and spatial-topology messages respectively.
- **`BandyMember`** — a trait for a bus participant: `publish(topic, msg) ->
  anyhow::Result<()>`.

### Transport — `bandy::synapse`
- **`Synapse`** — wraps a Tokio `broadcast` channel (buffer depth 1024).
  - `Synapse::new()` / `Default` — construct a bus.
  - `fire(&self, msg)` — publish synchronously; `fire_async(&self, msg).await`
    is the async counterpart. Both ignore the no-active-receiver case.
  - `subscribe(&self) -> broadcast::Receiver<SMessage>` — obtain a receiver.
  - `Synapse` is `Clone`; clones share the same underlying channel.

### Shared state — `bandy::state`
- **`AppState`** — the central mutable application state: chat/thought
  `history`, console logs, token usage, status flags, the active input buffer,
  shard statuses, live context, and the workspace-root anchor.
- **`HistoryItem`** — one entry in the timeline (`origin`, `content`,
  `timestamp`, `is_chat`).
- **`WorkspaceState`** — serializable pane layout (`left_pane`, `right_pane`,
  `split_ratio`), built from `ViewEntity` (`TopologyState` / `StreamState`).
  Supporting types include `TopologyState`, `ExpandableList`, `TopologyNode`,
  `DashboardState`, `DispatchRecord`, and `PreFlightPayload`.

### Domain ontology — `bandy::ontology`
- **`Origin`**, **`Shard`**, **`ShardRole`**, **`ShardStatus`**,
  **`WeightedSkeleton`**, **`SpatialNode`** / **`SpatialEdge`** — the value
  types referenced by messages and state.

### Telemetry — `bandy::telemetry`
- **`UnaLogger`** and **`ignite(log_dir)`** — a `log`-crate backend that routes
  records to per-subsystem log files (one file per crate target) and echoes to
  stdout.

## How it fits into UnaOS

Bandy is the central library that the userspace handlers (`handlers/`) and
vessels (`vessels/`) build on: a vessel wires a Tokio runtime, a `Synapse`, and a
selection of handlers together, and the Quartzite GUI (`libs/quartzite`)
subscribes to the same bus and routes user input back as `SMessage`s via
`Spline::bootstrap`. See
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
(sections 2–3) for the surrounding component model.

## Status

Implemented. `SMessage`, `Synapse`, the `bandy::state` domain types, and the
telemetry logger are in active use by the Lumen vessel and its handlers.
`BandyMember` is defined but not yet broadly implemented; `WeightedSkeleton`
content is intentionally non-serializable (in-process `Arc` only), with
inter-process telemetry deferred to `unafs` shared memory.
