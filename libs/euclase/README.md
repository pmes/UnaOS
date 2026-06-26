# Euclase

GPU rendering library for UnaOS userspace, built on [WGPU]. Euclase provides the
graphics-device setup, math primitives, and shaders that 3D vessels and handlers
use to draw to a window.

## Responsibilities

Euclase is the rendering foundation referenced in the userspace architecture
(`libs/euclase` — "GPU rendering (WGPU): shaders, render graph"). It covers two
concerns:

1. **Device and surface bring-up.** Acquiring a WGPU instance, adapter, device,
   and queue, and configuring a presentable surface against a host window.
2. **GPU-ready math and shaders.** `#[repr(C)]`, `Pod`/`Zeroable` vector,
   matrix, and quaternion types that can be uploaded to GPU buffers without
   conversion, plus the WGSL shaders that consume them.

The crate targets the Vulkan and Metal backends only (`Backends::VULKAN |
Backends::METAL`). It does not own a window of its own — it binds to a window
provided by Quartzite, the UnaOS GUI layer.

## Public API

### Device context — `cortex`
- **`Cortex<'a>`** — the rendering context. Holds the WGPU `instance`,
  `surface`, `Arc<Device>`, `Arc<Queue>`, and the active
  `SurfaceConfiguration`.
- **`Cortex::ignite(window, width, height) -> Cortex`** — async constructor.
  Creates the surface against a caller-supplied window target, requests a
  high-performance adapter, and requests a device with the
  `POLYGON_MODE_LINE` feature enabled (required for wireframe rendering). Picks
  an sRGB surface format when available and configures the surface with
  `PresentMode::AutoNoVsync`.
- **`Cortex::resize(width, height)`** — reconfigures the surface on window
  resize.

### Math types (re-exported from the crate root)
- **`Vec3`**, **`Vec4`** — 3- and 4-component `f32` vectors with the usual
  operator overloads, plus `dot`, `cross` (Vec3), `magnitude`, `normalize`, and
  `lerp`.
- **`Mat4`** — column-major 4×4 matrix compatible with WGSL `mat4x4<f32>`.
  Includes `identity`, `from_translation`, `from_scale`, `from_rotation`,
  perspective projections (`perspective_rh_zo`, `perspective_rh_gl`),
  `look_at_rh`, point/vector transforms, and `to_cols_array` for buffer upload.
- **`Quat`** — quaternion for rotation, with `from_axis_angle`, `normalize`,
  Hamilton-product multiplication, and `slerp`.

All math types derive `bytemuck::Pod`/`Zeroable` so they can be copied straight
into GPU buffers, and gain `serde` support under the optional `serde` feature.

### Utilities — `utils`
- **`to_radians(degrees)`**, **`to_degrees(radians)`** — angle conversion
  helpers.

### Shaders
- **`src/shaders/vug_wireframe.wgsl`** — a vertex/fragment shader pair for
  wireframe rendering. It takes a camera view-projection matrix as a uniform and
  per-vertex position and color, applied to the UnaOS signature glow tint. The
  shader is named for **Vug**, the 3D modeling/CAD handler that is the primary
  consumer of this crate.

## How it fits into UnaOS

Euclase is a userspace library under `libs/` (see
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)).
It supplies the GPU rendering primitives that the **Vug** 3D handler uses to draw
models, and binds to windows surfaced by **Quartzite**, the host GUI layer. It
does not interact with the Bandy message bus directly; rendering is driven by the
handler or vessel that owns the `Cortex`.

## Status

**Partial.** Implemented and in use: device/surface bring-up (`Cortex`), the
full math suite (`Vec3`/`Vec4`/`Mat4`/`Quat`) with unit tests, and the wireframe
WGSL shader. The render-graph layer described in the architecture map is not yet
present in this crate — current consumers issue WGPU passes directly using the
`Cortex` device and queue.

[WGPU]: https://wgpu.rs
