# The Software Rasterizer (`rast`)

`rast` (`unaos/crates/rast/`) is the platform-neutral 3D target layer: a small,
deterministic, `no_std`, zero-alloc-in-the-hot-path software renderer. It has two
jobs.

1. **3D on every platform today.** It rasterizes flat-shaded, z-buffered
   triangles into a caller-provided RGBA8 framebuffer slice, so any UnaOS target
   with a framebuffer (x86/virt, Pi 4, Orin) can draw 3D through the existing
   [panel/framebuffer path](framebuffer.md) with no GPU.
2. **The V3D pixel-exact oracle.** Because it is platform-neutral and byte-
   deterministic — the same input scene produces *byte-identical* pixels on
   x86_64 and aarch64 — it is the reference output that future GPU (V3D) rendering
   is diffed against. The scout-confirmed V3D milestone "GPU clears/fills a buffer,
   CPU verifies bytes" generalizes to **"GPU output == rasterizer reference."**

---

## 1. The determinism contract

Determinism is a hard requirement, not a nicety: the oracle role depends on it.

- **Everything is `f32`.** Every operation is one of the IEEE-754 primitives
  (`+ - * /`, comparisons) or a `libm` call (`sqrtf` / `floorf` / `sinf` /
  `cosf`).
- **`libm`, not the platform math.** `libm` is pure-Rust and correctly-rounded,
  so its results are byte-identical on both arches — the same kernel/host
  bit-identity guarantee [`libs/fs/unafs`](../../../../unaos/libs/fs/unafs/README.md)
  already relies on. `core`'s own float intrinsics (`f32::sin`, …) are `std`-only
  under `no_std` anyway, and platform libm implementations are *not*
  correctly-rounded — they diverge. Hence `libm`.
- **No fused multiply-add.** An fma rounds once and so differs from a separate
  `*` then `+`, and whether the compiler contracts `a*b+c` into an fma is
  arch/opt-level dependent. The `Mat4::mul` inner loop is written as an explicit
  sum of four products so no contraction is possible.
- **No fast-math.** Rust enables none by default; the crate adds none.

These rules live in the module docs of `math.rs` and are exercised by the golden
tests below (a scene rendered twice must be byte-identical, and the golden digest
is the cross-arch reference).

## 2. Crate API

`math.rs` — fixed-shape `f32` linear algebra:

- `Vec3` / `Vec4` (dot, cross, normalize, homogeneous point, `lerp`).
- `Mat4` — **column-major** (`m[col*4 + row]`); `mul`, `transform`,
  `translation`, `rotation_x/y/z`, `perspective` (right-handed, `[-1,1]³` clip
  cube, OpenGL convention), `look_at`.
- `sin` / `cos` / `floor` — the deterministic `libm` wrappers.

`raster.rs` — the 2D fill stage:

- `Rgba([u8;4])` — the one canonical color format, **`[R,G,B,A]`** row-major.
- `Target::new(color, depth, width, height, stride)` — wraps caller-owned
  buffers (`color: &mut [u8]` RGBA8, `depth: &mut [f32]`), returning `None` on a
  mis-sized buffer so an out-of-bounds write is impossible. No allocation.
- `Target::clear`, `triangle(v, color, cull)`, plus `get` / `depth_at` /
  `color_bytes` oracle helpers.
- The z-buffer is a parallel `f32` plane; smaller z is nearer, cleared to
  `Z_FAR`. A fragment writes only when its interpolated depth is **strictly less**
  than the stored value, so z-fighting resolves deterministically by draw order.

`lib.rs` — the pipeline:

- `render_mesh(target, model, view_proj, verts, indices, base_color, light_dir,
  ambient, cull) -> u32` — the whole thing: model→world→clip transform, flat-
  shade from the world-space face normal, near-plane (`w`) clip, perspective
  divide, viewport map, back-face cull, z-buffered fill. Returns the triangle
  count rasterized.
- `shade_lambert(normal, light_dir, base, ambient)` — flat Lambert shading with
  an ambient floor.

### Conventions

- **Winding.** Front faces are counter-clockwise *as drawn on the y-down screen*,
  which is a negative signed area under the edge function (the y-down flip inverts
  orientation). Back faces are culled when `cull = true`.
- **Top-left fill rule.** Boundary pixels on shared edges rasterize into exactly
  one triangle — no double-draw, no seam — so adjacent triangles compose
  deterministically.
- **Near-plane clip.** Sutherland–Hodgman against the homogeneous `w >= ε` plane,
  so the perspective divide never sees a zero/negative `w` (no NaN, no panic).
  A clipped triangle fans into up to two, in a fixed-capacity buffer (no alloc).

## 3. The golden-image test scheme

The crate is platform-neutral, so `cargo test -p rast` runs the oracle on the
host (the crate is `#![no_std]` for kernel builds, `std` only under `cargo test`).
Each golden is captured as an **FNV-1a 64-bit digest of the whole RGBA8 color
plane** — a full-buffer checksum, so any single-pixel drift trips the test.

Coverage (`unaos/crates/rast/tests/golden.rs`):

- single triangle (interior covered, corners not; back-face culled),
- z-fighting / depth order (nearer wins in *both* draw orders; the two orders
  converge to an identical color plane),
- near-plane clip (a triangle straddling the eye rasterizes its visible part with
  no NaN depth),
- degenerate (zero-area) triangles draw nothing,
- a rendered cube scene: rendered twice ⇒ byte-identical (determinism), and its
  digest is pinned as `GOLDEN_CUBE_07` (the cross-arch reference; regenerate only
  deliberately, never to "make it pass").

That pinned digest is exactly the artifact future V3D output is diffed against.

## 4. The x86/virt demo (`UNAOS_RAST=1`)

A knob-gated demo (`rast` Cargo feature → `unaos/crates/kernel/src/rast_demo.rs`)
renders a spinning, flat-shaded, z-buffered cube through the panel `Screen`. It is
**call-never-edit** with respect to the shared video path: it renders into its own
heap-owned RGBA8 back buffer (the double buffer), then presents each frame through
the public `Screen::put_pixel` / `Screen::flush` API — it touches no shared surface
code. The demo renders at a fixed 320×240 and blits centered on the panel (a full
1280×800 per-pixel present is far too slow to witness), runs a bounded 90 frames,
then hands the panel back to the shell, emitting one honest fps line:

```
:: RAST: software rasterizer demo — 320x240 spinning cube centered on 1280x800 panel, 90 frames ::
:: RAST: 90 frames in 4115 ms — 21.871 fps (software rasterizer, panel present) ::
```

With the feature off the whole module + the `rast` dependency are unlinked and the
kernel image is byte-identical to baseline (RAST-1 verified `.text` / `.rodata` /
`.data` / `.data.rel.ro` all byte-identical vs the pre-arc commit). The demo is
x86/virt only for now; the Pi 4 / Orin wire-ins come after this arc and the
concurrent V3D arc merge.

## See also
- `unaos/crates/rast/` — the crate.
- [framebuffer](framebuffer.md) — the panel surface the demo presents through.
- [engine](engine.md) — the existing 2.5-D `vug` facet renderer (distinct: a
  painter's-order solid-facet engine, not a z-buffered 3D rasterizer).
