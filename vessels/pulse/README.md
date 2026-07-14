# Pulse

Pulse is the UnaOS **vessel** for watching the machine breathe: a per-core CPU
monitor in the spirit of BeOS Pulse.

## Overview

A vessel is an executable a user runs: it wires together a Tokio runtime, the
message bus, and a native GUI window (see
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)).
Pulse is the vessel scoped to system vitals — one window, one row of numbered
segment bars, one per core: `CPU 1 ▮▮▮▮▯▯ 2 ▮▮▯▯▯▯ …`. Idle cores stay visible
as dim empty ladders (alive but empty). The palette is the kernel's own: the
Moonstone field (`#2D2B55`) and the lilac/purple meter ramp from the vug
render meters.

It composes existing userspace libraries rather than implementing its own
stack:

- **`libs/bandy`** — the in-process bus. The sampler publishes
  `SMessage::CorePulse { loads }` beats on the `Synapse`; the meter view
  subscribes. Nothing calls anything directly.
- **`libs/quartzite`** — the GUI layer. Pulse opens its window through
  `Backend::new_vessel` (the single-view sibling of the workspace `Backend::new`)
  and renders through the `meter` module's `SegmentMeterView`, whose layout is
  derived from the live window bounds on every draw — no fixed pixel geometry.
- **`libs/gneiss_pal`** — paths and telemetry plumbing, as in every vessel.

## The seam: `PulseSource`

`src/source.rs` defines the named seam between the UI and wherever per-core
load truly comes from:

- `PulseSource` — the trait: `sample() -> Vec<f32>`, one `0.0..=1.0` entry per
  core, on a calm 250 ms cadence.
- `HostPulseSource` — v1: the HOST's per-core CPU stats via `sysinfo` (this
  layer is host-native today).

**The banked vision is a kernel telemetry feed**: a `PulseSource` fed by the
UnaOS kernel itself — serial bridge or network telemetry from the metal
machines — firing the same `CorePulse` beat, with zero UI change. Same honest
seam as the kernel's vug render meter: name the seam so a real feed can
replace the source someday.

Handler-extraction candidate: if a system-stats domain handler materializes
(`principia` — System — is the natural owner once it becomes a real crate),
the sampler lifts out of the vessel additively.

## Running

```sh
cargo run -p pulse
```

Dev affordance: `UNAOS_METER_SNAPSHOT=/path.png` (with optional
`UNAOS_METER_SNAPSHOT_DELAY_MS`, default 1500) makes the meter write a
pixel-true PNG of itself shortly after launch — no screen-recording
entitlement needed. Used for landing-report screenshots and headless visual
checks.

## How it fits into UnaOS

Pulse is one of the vessels under `vessels/`, alongside `lumen` (the reference
GUI vessel) and `facet`. Domain logic does not live in the vessel: sampling
sits behind the `PulseSource` seam, rendering lives in quartzite's `meter`
module, and the two only meet on the bus. See
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
for the component model and [`docs/CODEX.md`](../../docs/CODEX.md) for the
system canon.

## Status

Implemented on macOS (AppKit via quartzite); further backends follow quartzite
maturity. The kernel-side pulse view (in-kernel shell meter) is a separate arc.
