# Stria

**Crate:** `handlers/stria` · **Layer:** Handler (domain service) · **Status:** Bus-driven audio handler (live)

Stria is the audio handler for UnaOS userspace — "The Studio." It owns the
`libs/resonance` engine's whole lifecycle (build the graph, open the output
device, keep the stream alive) and integrates it with the rest of the system
over the Bandy nervous system: it **publishes** the engine's output level on the
bus and exposes a **programmatic control surface** for frequency, gain, and
running-state.

Where `vessels/phonolite` gives the engine a *face* (a window and sliders), stria
gives it a *nerve ending*: a headless, bus-facing service that a generator (a
shell verb, a UI, an AI) or a test can drive.

## What the code does today

A library crate (`lib.rs`), no window, no binary. The public surface:

- **`StriaHandler::ignite(synapse) -> Result<StriaHandler>`** — builds the
  resonance test graph, opens the default output device via
  `resonance::AudioEngine`, and spawns two Tokio tasks. Must be called from
  within a Tokio runtime. The engine starts running; the returned handle keeps
  the cpal stream (and the tasks) alive until it is dropped.
- **Control** — `set_frequency(hz)`, `set_gain(g)`, `set_running(bool)`. Each is
  non-blocking: it sends a `StriaControl` intent to the single owning control
  task, which applies intents in arrival order.
- **`meter()`** — a cloneable, `Send` read-only probe (`resonance::ResonanceMeter`)
  of the engine's level and liveness.
- **`StriaBus`** — a real `bandy::BandyMember`: `publish` fires the message on
  the `Synapse`, and `process_frame(samples, rate)` wraps a block as
  `SMessage::AudioChunk`. This is the seam AV-A1 left deliberately dead on
  `resonance::AudioEngine` (its `publish` only printed); here it delivers.

### Bus traffic

- **Output:** the level cadence (~30 Hz) reads the engine's per-block peak and
  publishes it as a single-bin `SMessage::Spectrum` beat — the existing
  RESONANCE-section message, so `libs/bandy` needs **zero** changes (as ROADMAP
  §3a promised). `AudioChunk` full-buffer publishing is the ready seam for a
  future real-time-safe sample tap (`StriaBus::process_frame`).
- **Input:** control is a direct programmatic API today (there is no dedicated
  media `SMessage` variant, and adding one is out of this arc's lane). This
  mirrors `handlers/junct`, which likewise produces bus traffic without
  consuming a control message.

Nothing here touches the Synapse from the real-time audio callback: the callback
only writes an atomic, and the cadence task turns that into bus traffic.

## Design notes (carried from the AV-A1 review)

- **Stop/start ordering contract.** `ResonanceHandle::stop()` is queue-routed;
  `start()` is atomic-direct. A rapid `stop(); start()` can net out *stopped*
  when the queued `Stop` drains after the direct start — unreachable at GUI
  timescales, real at bus rates. And the drain is *device-buffer paced*, not
  block paced: queued commands drain only inside the cpal callback, which fires
  at the device buffer period (commonly ~5–85 ms on CoreAudio), so no fixed time
  budget is safe. stria **respects** the contract timing-free: all control flows
  through one owning task, and a resume that follows a still-pending stop polls
  the engine's liveness flag until the `Stop` has *observably* drained
  (`is_active()` reads false) before the direct `start`, bounded by
  `STOP_DRAIN_TIMEOUT` (200 ms — then start anyway and log; only a full command
  ring or a dead device gets there). The invariant is unit-tested against a mock
  engine whose drain is arbitrarily delayed (the `EngineControl` seam).
- **No re-entrant borrows.** The `ResonanceHandle` has exactly one owner (the
  control task), reached only by message — nothing borrows it across a call, so
  the AV-A1 panel's `borrow_mut()`-across-callback hazard cannot arise here.
- **Liveness never desyncs.** The level cadence gates the published level on the
  engine's shared liveness flag (`governed_level`): a dead device (callback
  stops, peak atomic goes stale) still reports a truthful zero, and a persistent
  desired-but-inactive condition is surfaced once (`DeathWatch`).

Pure logic (`governed_level`, `level_beat`, `DeathWatch`), the ordering
invariant (via a mock `EngineControl` with delayed drain), and the live bus face
(`StriaBus`) are unit-tested host-side with no audio device required
(`cargo test -p stria`).

## Status

**Live.** Owns the resonance engine end to end and speaks `SMessage` on the bus;
programmatic control with the ordering contract respected. Not yet wired into a
vessel — no vessel currently mounts stria (phonolite drives the engine
directly). Whether phonolite later rides stria or keeps its direct handle is an
open design question, deferred to a future arc.

## See also

- [`docs/ROADMAP.md`](../../docs/ROADMAP.md) §3a — the creative lane.
- [`libs/resonance`](../../libs/resonance) — the engine stria drives.
- [`handlers/junct`](../junct) — the sibling cpal-input handler (bus producer).
- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
  — userspace component model and the Bandy bus.
- [`docs/CODEX.md`](../../docs/CODEX.md) — system canon and handler manifest.
