# Phonolite

Phonolite is the UnaOS **vessel** for hearing the machine speak: a live tone
generator — resonance given a face. (The mineral: phonolite is the volcanic
"sounding stone" that rings when struck.)

## Overview

A vessel is an executable a user runs: it wires together a Tokio runtime, the
message bus, and a native GUI window (see
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)).
Phonolite is the vessel scoped to the audio engine: one window holding a
Start/Stop button, a log-scale frequency slider (55–1760 Hz, five octaves), a
gain slider (capped at 0.5 — a bench instrument, not a siren), and a live
`LVL ▮▯▯▯▯▯` level ladder. The palette is the kernel's own: the Moonstone
field (`#2D2B55`) and the lilac/purple meter ramp from the vug render meters.

It composes existing userspace libraries rather than implementing its own
stack:

- **`libs/resonance`** — the sound. The engine runs a sine → gain graph inside
  a real-time cpal callback, re-tuned to the device's true sample rate at
  start; the vessel drives it through the nameable `ResonanceHandle`
  (frequency, gain, stop/start) over a lock-free command ring.
- **`libs/bandy`** — the in-process bus. A ~30 Hz cadence task reads the
  engine's per-block peak from a `ResonanceMeter` probe (an atomic the audio
  callback writes) and fires single-bin `SMessage::Spectrum` beats; the level
  bar subscribes. Nothing fires the Synapse from the real-time callback.
- **`libs/quartzite`** — the GUI layer. Phonolite opens its window through
  `Backend::new_vessel` and builds its face from the `tone_panel` module —
  quartzite's **first input-control surface**: AppKit target-action landing in
  a `define_class!` view whose ivars hold plain Rust callbacks, so the vessel
  hands over closures and receives values without ever touching AppKit.
- **`libs/gneiss_pal`** — paths and telemetry plumbing, as in every vessel.

## What's real

- The pitch is honest: the graph runs at the device's reported rate (48 kHz on
  the reference Mac), so 440 Hz is truly 440 Hz.
- The controls are live: slider moves land in the audio thread over the
  lock-free ring within one 64-sample block; Stop emits true silence (and
  freezes the oscillator phase); Start resumes.
- The level bar is bus-routed end to end: callback atomic → cadence task →
  `Spectrum` beat → main-thread repaint.
- `src/tuning.rs` owns the vessel's decisions (range, gain cap, the test
  graph's node ids, the level-beat seam) as pure, unit-tested logic.

## Running

```sh
cargo run -p phonolite
```

Dev affordance: `UNAOS_PANEL_SNAPSHOT=/path.png` (with optional
`UNAOS_PANEL_SNAPSHOT_DELAY_MS`, default 1500) makes the panel write a
pixel-true PNG of itself shortly after launch — no screen-recording
entitlement needed. Used for landing-report screenshots and headless visual
checks.

## Banked vision

- **Handler extraction (AV-A2)**: `stria` — Audio/video editing, "The Studio"
  — is the natural home for real audio domain logic. When stria is rewritten
  as a bus-driven handler around the finished engine, phonolite stays what it
  is: the thin face; the graph-building and signal routing lift out of vessel
  scope entirely.
- Richer graphs (wave selection, envelopes, a second oscillator) belong to
  the engine/handler side, not this panel; the panel grows controls only as
  the engine grows honest parameters to bind them to.

## How it fits into UnaOS

Phonolite is one of the vessels under `vessels/`, alongside `lumen` (the
reference GUI vessel) and `pulse` (the system monitor whose scaffold and meter
idioms this vessel reuses). Domain logic does not live in the vessel: sound
lives in resonance, controls live in quartzite's `tone_panel`, and the level
readout meets the UI on the bus. See
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
for the component model and [`docs/CODEX.md`](../../docs/CODEX.md) for the
system canon.

## Status

Implemented on macOS (AppKit via quartzite); further backends follow quartzite
maturity. Kernel-side audio (x86 HDA, Pi HDMI/PWM) is a separate roadmap row —
this vessel is host-native by design.
