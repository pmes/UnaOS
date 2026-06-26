# Stria

**Crate:** `handlers/stria` · **Layer:** Handler (domain service) · **Status:** Design-stage skeleton

Stria is the audio/video handler for UnaOS userspace — the planned media engine
responsible for time-based media (playback, non-destructive editing, and DSP),
complementing the image/raster handling done elsewhere in the system.

## Responsibility

Per the system manifest ([`docs/CODEX.md`](../../docs/CODEX.md)), Stria — "The
Studio" — is intended to own:

- Media **playback** with non-destructive A-B looping and bookmarking.
- A **DSP graph** for audio/video processing pipelines.
- The logic backing media preview and a studio editing surface in the host
  vessel.

None of this media functionality is implemented yet. The current contents are a
scaffold (see Status).

## What the code does today

The crate currently builds as a standalone binary that brings up a window and an
event loop; it does not yet decode, play, or edit any media.

- **`main.rs`** — entry point. Prints startup banners, queries the host CPU
  count via `num_cpus`, opens a 1280×720 window through
  `gneiss_pal::WaylandApp`, and runs the event loop. The loop exits on
  `WindowEvent::CloseRequested` and on the `Escape` key.
- **`engine/mod.rs`** — `MediaGraph`, a stub for the future processing pipeline.
  It stores a worker-thread count (`MediaGraph::new(cores)` / `.cores()`) and is
  not yet wired into `main`. The intended pipeline stages (decode → filter →
  render) and thread-to-core pinning are described only in comments.
- **`ui.rs`** — placeholder; no UI implemented.

Dependencies declared in `Cargo.toml`: `gneiss_pal` (windowing/PAL), `tokio`,
`crossbeam`, `glam`, `num_cpus`. There is currently **no** GStreamer/FFmpeg,
GTK4, or `cpal` dependency, despite earlier documentation; those are aspirational.

## Synapse / SMessage integration

A UnaOS handler is a domain-service crate that exposes an async entry point (by
convention `ignite(...)`) and reacts to `SMessage` traffic on the `Synapse`
broadcast bus defined in [`libs/bandy`](../../libs/bandy). See
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md).

Stria does **not** yet follow this pattern. It has no `ignite(...)` entry point,
does not depend on `bandy`, and neither subscribes to nor emits any `SMessage`.
There are also no dedicated media `SMessage` variants defined in the bus today.
Converting Stria into a bus-driven handler — subscribing to playback/transport
requests and emitting media-state updates — is future work.

## Status

**Design-stage skeleton.** A window/event-loop binary plus an unwired
`MediaGraph` stub. No media decode/playback, no DSP, no Synapse/SMessage
integration. The capability scope above reflects the documented design intent,
not shipped behavior.

## See also

- [`docs/CODEX.md`](../../docs/CODEX.md) — system canon and handler manifest.
- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
  — userspace component model (libraries / handlers / vessels) and the Bandy bus.
