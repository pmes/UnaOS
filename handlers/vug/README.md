# Vug — 3D / CAD handler

Vug is the UnaOS handler responsible for 3D geometry: CAD-style viewing and
editing, and (longer term) CAM / slicing. In the handler manifest it is the
"3D" domain service — the counterpart to tools such as Fusion 360 or Cura.

## Status

**Design-stage / early prototype.** The crate today contains standalone
rendering experiments, not a wired handler. There is no `ignite(...)` entry
point, no `Synapse` subscription, and no `SMessage` handling yet; `main.rs` is a
placeholder. The pieces below are building blocks toward the handler described
in [`docs/CODEX.md`](../../docs/CODEX.md), not the handler itself.

## What exists today

- **`renderer.rs` — `Renderer`** (declared via `lib.rs`). A GTK4 `GLArea`
  OpenGL renderer that draws an animated, indexed colored cube with a
  perspective camera. Public surface:
  - `Renderer::new()` / `Default`
  - `load_gl_functions()` — loads GL symbols via `epoxy::get_proc_addr`
  - `init_gl()` — compiles the shaders and uploads the cube's VAO/VBO/EBO
  - `draw(&GLArea, &GLContext)` — renders one frame and requests continuous
    redraw
  - `update_spectrum(Vec<f32>)` — feeds an audio spectrum that scales the
    geometry (an early audio-reactive hook)

  Vector and matrix math (`Mat4`, `Vec3`, `Vec4`, look-at and perspective
  projections) come from the shared `euclase` library.

- **`viewport.rs` — `VugViewport`** (`forge` / `render`). A separate WGPU
  prototype that draws an RGB origin triad as a wireframe through the `euclase`
  `Cortex` (device / queue / surface) and the `vug_wireframe.wgsl` shader. Note
  this module is **not** yet referenced from `lib.rs` and pulls in `wgpu` /
  `bytemuck`, which are not in `Cargo.toml`; it is an exploratory file rather
  than a compiled part of the crate.

Two rendering backends are therefore present in parallel (GTK4 + raw OpenGL, and
WGPU); converging on one is part of the work remaining.

## How it will plug into the bus

UnaOS handlers are domain-service crates that expose an async entry point
(by convention `ignite(...)`), subscribe to the **Synapse** message bus, and
react to **`SMessage`** variants. The bus is defined in `libs/bandy`: `Synapse`
wraps a Tokio broadcast channel (`fire` to publish, `subscribe` to receive) and
`SMessage` is the single enum of every message type in the system. Handlers do
not call each other directly — they communicate only through `SMessage` on the
Synapse.

Vug does not yet implement this surface. The intended shape is an
`ignite(synapse)` task that subscribes to the bus, renders the active 3D model,
and publishes results (view state, generated geometry, and eventually CAM
toolpaths) back as `SMessage`s. The specific variants are not yet defined.

## Dependencies

- `gtk4`, `glib`, `epoxy`, `gl` — GTK4 windowing and the OpenGL path
- `euclase` (`libs/euclase`) — shared GPU rendering and vector/matrix math

## Build

```bash
cargo build -p vug
cargo test  -p vug
```

## See also

- [`docs/CODEX.md`](../../docs/CODEX.md) — handler manifest (Vug = "The Sculptor").
- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md) —
  vessels / handlers / Bandy / Synapse / SMessage.
