# Facet

Facet is the UnaOS **vessel** for viewing and inspecting raster images.

## Overview

A vessel is an executable a user runs: it wires together a Tokio runtime, the
message bus, a selection of handlers, and a native GUI window (see
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)).
Facet is the vessel scoped to images — decoding a file, presenting it on screen,
and supporting close inspection (zoom, pan, per-pixel and per-channel readout).

It is intended to compose existing userspace libraries rather than implement its
own stack:

- **`libs/lux`** — image decoding, including camera RAW. Facet's source of pixel
  data.
- **`libs/euclase`** — GPU rendering (WGPU): shaders and render graph. The
  intended path for presenting and scaling large images on the GPU.
- **`libs/quartzite`** — the GUI layer. Renders the workspace natively on the
  host and routes input back as messages.
- **`libs/bandy`** — the in-process message bus (`SMessage`, `Synapse`).
  Facet publishes and subscribes here rather than calling other components
  directly.

## Responsibilities (intended)

- Open an image file and decode it via `lux`.
- Display it in a Quartzite window with pan and zoom.
- Support inspection: per-pixel RGBA readout and isolation of individual
  channels.
- Participate in the userspace bus: a file opened elsewhere (for example in the
  Matrix file model) can be routed to Facet for viewing.

## Public API

Facet exposes no Rust API yet. As a vessel it is expected to follow the
established pattern: a `main` that constructs a `Synapse`, bootstraps a
`WorkspaceState` through `Spline::bootstrap`, and runs the Quartzite
`Backend::run` event loop — mirroring the `lumen` vessel
([`apps/lumen`](../lumen)).

## How it fits into UnaOS

Facet is one of the vessels under `apps/`, alongside `lumen` (the reference GUI
vessel) and the `apps/cli/*` tools. It does not own image infrastructure; that
lives in the shared libraries (`lux`, `euclase`) so capabilities are not
duplicated across vessels. See
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
for the full component model and [`docs/CODEX.md`](../../docs/CODEX.md) for the
system canon.

## Status

**Design-stage.** This crate currently contains only this README — there is no
`Cargo.toml` or `src/`, and it is not part of the workspace build. The
description above states intent, derived from the architecture documents and the
available libraries, not shipped behavior. The supporting libraries it would
build on (`lux`, `euclase`, `quartzite`, `bandy`) exist independently.
