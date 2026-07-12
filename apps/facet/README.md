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

## Responsibilities

Landed (FACET-1, MVP):

- Open an image file named on the command line and decode it via `lux`
  (`decode` dispatches on magic bytes: PNG, JPEG, or Sony ARW).
- Pack the linear `RgbBuffer` down to 8-bit sRGB RGBA (the sRGB OETF lives in
  the vessel; `lux` hands out *linear* f32 RGB).
- Display it in a Quartzite window, drawn aspect-fit and centered on a dark
  field, rescaling with the window (CPU blit via `NSBitmapImageRep`).

Intended (later arcs):

- Pan and zoom, and the euclase textured-quad (GPU) presentation path.
- Inspection: per-pixel RGBA readout and isolation of individual channels.
- Participate in the userspace bus: a file opened elsewhere (for example in the
  Matrix file model) can be routed to Facet for viewing.

## Usage

```
facet <image-file>     # PNG, JPEG, or Sony ARW
```

## Public API

Facet exposes no Rust API yet. As a vessel it is expected to follow the
established pattern: a `main` that constructs a `Synapse`, bootstraps a
`WorkspaceState` through `Spline::bootstrap`, and runs the Quartzite
`Backend::run` event loop — mirroring the `lumen` vessel
([`apps/lumen`](../lumen)).

## How it fits into UnaOS

Facet is one of the vessels under `apps/`, alongside `pulse` and `phonolite`
(the two other single-view vessels it copies), `lumen` (the reference GUI
vessel) and the `apps/cli/*` tools. It does not own image infrastructure; that
lives in the shared libraries (`lux`, `euclase`) so capabilities are not
duplicated across vessels. See
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
for the full component model and [`docs/CODEX.md`](../../docs/CODEX.md) for the
system canon.

## Status

**FACET-1 landed (macOS backend).** The MVP viewer works: `facet <image>`
decodes via `lux`, packs to sRGB, and shows the picture aspect-fit in a
Quartzite window. Presentation is a CPU blit (`NSBitmapImageRep`), tagged sRGB
for color-managed display; the euclase textured-quad (GPU) path, pan/zoom, and
pixel readout are the later arcs (see [`docs/ROADMAP.md`](../../docs/ROADMAP.md)
§3a). Non-macOS backends follow quartzite's platform maturity.
