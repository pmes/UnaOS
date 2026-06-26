# junct

**Canonical role (per [CODEX](../../docs/CODEX.md)): "The Receiver" — the
communications aggregation handler**, unifying messaging, email, IRC, and RSS
into a single stream. **This role is not yet implemented.**

`junct` is a UnaOS **handler** — a domain-service crate that owns one capability
area and communicates over the Bandy message bus.

> **Status note:** the only code in this crate today is an unrelated placeholder —
> a microphone-capture + live-FFT path (described below) that publishes an audio
> spectrum onto the bus. It does not reflect junct's intended communications role
> and is expected to be replaced.

## Current code (placeholder: mic capture + FFT)

On construction, `JunctHandler`:

1. Acquires the default input device and stream configuration from the host
   audio backend via [`cpal`].
2. Opens a live input stream. In the audio callback it reads the first channel
   of each frame and accumulates samples into a fixed-size buffer of
   `resonance::BLOCK_SIZE` (currently 64).
3. When a block fills, it runs an in-place FFT (`resonance::dsp::FftContext`),
   computes the magnitude of the first `BLOCK_SIZE / 2` bins (the usable half of
   the real-input spectrum), and resets the buffer.
4. Publishes the magnitudes as `SMessage::Spectrum { magnitude }`.

The stream runs for the lifetime of the handler; the `cpal::Stream` is held in
the returned struct and stops when it is dropped.

## Bus integration

`junct` is a **producer** on the Synapse, the Bandy broadcast bus.

- **Entry point:** `JunctHandler::new(bandy_tx: broadcast::Sender<SMessage>) ->
  anyhow::Result<Self>`. The caller passes a sender cloned from the shared
  Synapse; the handler keeps it and emits from the audio callback thread.
- **Emits:** `SMessage::Spectrum { magnitude: Vec<f32> }` — one message per
  completed block, where `magnitude` holds `BLOCK_SIZE / 2` frequency-bin
  magnitudes.
- **Subscribes to:** nothing. `junct` is currently output-only; it does not
  react to inbound messages.
- **Failure mode:** returns `Err` if no default input device is available.
  Sends are best-effort (a full or closed channel is ignored), so the handler
  never blocks or panics on bus backpressure.

Note that this handler exposes `JunctHandler::new` rather than the `ignite(...)`
entry point used by most handlers, because it must hand back a live stream
object to keep capture running.

## Key types

| Item | Description |
| --- | --- |
| `JunctHandler` | Owns the live `cpal::Stream`. Construct with `new`; capture runs until dropped. |
| `SMessage::Spectrum { magnitude }` | The only message variant emitted (defined in `libs/bandy`). |

## Dependencies

- [`cpal`] — cross-platform audio host/device access.
- `resonance` — provides `BLOCK_SIZE` and the `dsp` FFT primitives
  (`Complex`, `FftContext`).
- `bandy` — `SMessage` and the broadcast bus.
- `tokio` — `broadcast::Sender` for publishing.

## Status

**Partial — implemented for spectrum capture only.**

The microphone-to-spectrum path described above is implemented and functional.
It is declared as a dependency of the `lumen` vessel but is not yet instantiated
there. The broader communications role implied by the crate's place in the
handler set (messaging, email, RSS) is **not implemented**; only audio input and
FFT spectrum publishing exist today.

Edition: Rust 2024. License: LGPL-3.0-or-later.

[`cpal`]: https://crates.io/crates/cpal
