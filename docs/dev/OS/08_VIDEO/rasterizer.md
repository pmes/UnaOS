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

Where each rule lives in code:

| Rule | Citation |
| --- | --- |
| `no_std` for every cross-compiled build; `std` only under `cargo test` | `rast/src/lib.rs:4` (`#![cfg_attr(not(test), no_std)]`) |
| `libm` is the only dependency, `default-features = false` | `rast/Cargo.toml:14` |
| `sqrtf` for vector length | `rast/src/math.rs:69` |
| `sinf` / `cosf` / `floorf` wrappers | `rast/src/math.rs:259,265,271` |
| No-FMA `Mat4::mul` (explicit four-product sum) | `rast/src/math.rs:145` |
| Zero allocation in the hot path (caller owns both planes) | `rast/src/raster.rs:54-61` |

**One copy, verified.** The crate is byte-identical across every track branch —
not by convention but as a checkable fact, since the whole crate is one git tree
object:

```
cd /path/to/UnaOS && git fetch origin
for r in HEAD origin/main origin/UnaOS-gemini origin/hw-pi4 origin/hw-rmbp; do
  git rev-parse "$r:unaos/crates/rast"
done | sort -u | wc -l      # must print 1
```

Verified 2026-08-18 from `hw-jetson`: all five refs report tree
`bdb6d3f9e2f3549d5532ba1f657df33f3aa7f1bf`. Section 3 explains why this
must stay true.

## 2. Crate API

The whole public surface is re-exported from `lib.rs:47-51`: the modules `math`
and `raster`, plus `Mat4`, `Vec3`, `Vec4`, `Rgba`, `ScreenVert`, `Target`,
`Z_FAR`. Everything below is public unless marked otherwise.

### 2.1 `math.rs` — fixed-shape `f32` linear algebra

- `Vec3` (`math.rs:25`) — `new`, `sub`, `add`, `scale`, `dot`, `cross`,
  `length`, `normalize` (`math.rs:33-74`).
- `Vec4` (`math.rs:86`) — `new`, `point` (homogeneous, `w = 1`), `lerp`
  (`math.rs:95-108`).
- `Mat4` (`math.rs:121`) — **column-major**, `pub m: [f32; 16]` indexed
  `m[col*4 + row]` (`math.rs:123`); `zero`, `identity`, `mul`, `transform`,
  `translation`, `rotation_x/y/z`, `perspective` (right-handed, `[-1,1]³` clip
  cube, OpenGL convention), `look_at` (`math.rs:128-231`).
- `PI` (`math.rs:255`) and the deterministic `libm` wrappers `sin` / `cos` /
  `floor` (`math.rs:259,265,271`).

### 2.2 `raster.rs` — the target and the 2D fill

**`Rgba(pub [u8; 4])`** (`raster.rs:26`) — the one canonical color format,
**`[R, G, B, A]`**. Constructors `Rgba::new` and `Rgba::rgb` (opaque alpha) at
`raster.rs:30,34`. Derives `PartialEq`/`Eq`, so tests compare colors directly.

**`ScreenVert`** (`raster.rs:46`) — `pub x: f32, y: f32, z: f32`. A vertex
*already projected to screen space*: sub-pixel pixel-space `x`/`y`, and a depth
`z` that need only be monotonic with distance (the pipeline feeds NDC z in
`[-1, 1]`). The raster stage is pure 2D — all clipping and projection happened
upstream (`raster.rs:42-44`).

**`Z_FAR: f32 = f32::INFINITY`** (`raster.rs:40`) — the depth-clear value.

**`Target<'a>`** (`raster.rs:54-61`) borrows both planes from the caller and
allocates nothing. Its four fields are `color: &'a mut [u8]`,
`depth: &'a mut [f32]`, `width`, `height`, `stride`.

- `Target::new(color, depth, width, height, stride) -> Option<Self>`
  (`raster.rs:67-81`). **Stride is in pixels, not bytes**: the color plane
  strides by `4*stride` bytes per row and the depth plane by `stride` entries
  (`raster.rs:59-60`). The constructor validates `stride >= width`,
  `color.len() >= 4*stride*height`, and `depth.len() >= stride*height`,
  returning `None` otherwise — so a mis-sized buffer can never produce an
  out-of-bounds write (`raster.rs:74-79`).
- `clear(c)` (`raster.rs:93`) — color to `c`, depth to `Z_FAR`.
- `width()` / `height()` (`raster.rs:84,88`).
- `triangle(v, color, cull) -> bool` (`raster.rs:142`) — see below.
- Oracle helpers: `get(x, y)` (`raster.rs:112`), `depth_at(x, y)`
  (`raster.rs:124`), `color_bytes()` (`raster.rs:130`, the raw RGBA8 plane the
  kernel blits from and the goldens digest).

**Cull mode is a `bool`, not an enum.** `triangle`'s third argument is
`cull: bool` (`raster.rs:142`); there is no front-face/none/both enumeration and
no configurable winding order. The behaviour is fixed at `raster.rs:145-160`:

1. Zero signed area ⇒ return `false` (degenerate, nothing drawn) —
   `raster.rs:146-148`.
2. `cull == true` and `area2 >= 0.0` ⇒ return `false` (back-facing) —
   `raster.rs:149-151`.
3. `cull == false` ⇒ the winding is normalized by swapping `v[1]`/`v[2]` when
   needed, so both sides draw — `raster.rs:156-160`.

The `bool` return means "was this triangle accepted", not "did it change
pixels": a wholly off-screen but validly-wound triangle returns `true`
(`raster.rs:168-170`).

**Depth handling** (`raster.rs:199-208`). Depth is interpolated barycentrically
from the three `ScreenVert.z` values, and a fragment writes **only when
`z < self.depth[di]` — strictly less**. Smaller z is nearer. Equal depth means
the first writer wins, so z-fighting resolves deterministically by draw order,
and disjoint depths make the result order-independent (pinned by the test at
`tests/golden.rs:82`).

### 2.3 `lib.rs` — the pipeline entry points

- `render_mesh(target, model, view_proj, verts, indices, base_color, light_dir,
  ambient, cull) -> u32` (`lib.rs:104-114`) — the whole pipeline for an indexed
  mesh. `indices` is a flat list of triples consumed by `chunks_exact(3)`
  (`lib.rs:119`); out-of-range indices are skipped, not panicked on
  (`lib.rs:122-124`). Returns the count of triangles actually rasterized,
  post-cull and post-clip (`lib.rs:117,154-156`).
- `shade_lambert(normal, light_dir, base, ambient) -> Rgba` (`lib.rs:60`) — flat
  Lambert `max(0, n·l)` over a base color with an ambient floor; normalizes both
  inputs itself.
- `W_EPS: f32 = 1.0e-5` (`lib.rs:55`) — private. Clip-space `w` below this is
  clipped away, so the perspective divide never sees a zero or negative `w`.

### 2.4 What the API deliberately does not have

This subsection exists because the absences are load-bearing, and because they
are what `orin-3d.md` §3.1 cites when it explains why band-parallel rendering
was not built.

- **No viewport origin.** `Target` carries `width`, `height` and `stride` and
  nothing else (`raster.rs:54-61`); `Target::new` takes no origin
  (`raster.rs:67-73`). Meanwhile `render_mesh` maps NDC straight onto
  `target.width()`/`height()` (`lib.rs:115-116`, `to_screen` at `lib.rs:85-91`).
  A band-sized `Target` therefore renders **the whole scene squashed into the
  band**, not the band's slice of the scene.
- **No scissor rectangle.** `triangle` clamps its bounding box to the target's
  own full extent — `0..=width-1` by `0..=height-1` (`raster.rs:164-167`) — and
  offers no way to restrict writes further.
- **No clear rectangle.** `clear` always covers the full `width × height`
  (`raster.rs:94-102`).

Consequently a caller cannot say "render this scene, but only the rows I own".
Section 5 covers the two proposed API extensions that would lift this, and why
neither has been made.

### 2.5 Pipeline stages, and which are private

`render_mesh` is a closed pipeline. Only its two ends are public:

| Stage | Where | Visibility |
| --- | --- | --- |
| model → world (and the shading normal) | `lib.rs:126-131` | inside `render_mesh` (public entry) |
| world → clip (`view_proj.transform`) | `lib.rs:134-136` | inside `render_mesh` |
| `clip_near` — Sutherland–Hodgman against `w >= W_EPS` | `lib.rs:174` | **private** |
| `divide_and_map` — perspective divide + viewport map | `lib.rs:164` | **private** |
| `to_screen` — NDC → pixel space | `lib.rs:85` | **private** |
| `Target::triangle` — cull, fill rule, depth test, write | `raster.rs:142` | **public** |

The intermediate vertex type `ClipVert` (`lib.rs:78`) is private too.

**This privacy is the second half of the parallelism blocker.** Because
`clip_near`, `divide_and_map` and `to_screen` are all private, a caller cannot
run transform-and-clip itself and feed band-offset `ScreenVert`s into the public
`Target::triangle` without re-implementing the projection pipeline — which is a
fork of the oracle in all but file location, and would put `GOLDEN_CUBE_07` at
risk for exactly the reason section 3 gives.

### 2.6 Conventions

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

### 3.1 `GOLDEN_CUBE_07` — what it pins and how it is checked

**What it pins.** One fixed pose of the canonical unit cube: 8 corners and 12
outward-wound triangles (`tests/golden.rs:201-222`) rendered at **96×96** into a
`0x101018` background, with `model = rotation_y(0.7) · rotation_x(0.35)`,
`perspective(PI/3, 1.0, 0.5, 100.0)`, an eye at `(0, 0, 5)` looking at the
origin, base color `0x40B0FF`, light `(0.4, 0.8, 0.6)`, ambient `0.25`, and
back-face culling on (`render_cube_scene`, `tests/golden.rs:224-253`). The
`_07` suffix is the `0.7` rotation angle.

**Where the digest lives.** `const GOLDEN_CUBE_07: u64 = 0x1944_46bc_a3de_a139;`
— `unaos/crates/rast/tests/golden.rs:279`. There is no separate golden file and
no image blob checked in; the reference *is* that one literal.

**How it is checked.** The digest is FNV-1a 64 over the **entire RGBA8 color
plane** (`fnv1a`, `tests/golden.rs:18-25`), so any single-pixel drift trips the
test. Two tests guard it:

- `cube_scene_is_deterministic` (`tests/golden.rs:256`) — the same pose rendered
  twice must produce byte-identical output. This is the arch-neutrality
  guarantee in miniature.
- `cube_scene_matches_golden` (`tests/golden.rs:266`) — the digest must equal
  `GOLDEN_CUBE_07`, and reports the offending value on failure.

Run it with a plain host `cargo test` — the crate is a member of the kernel
workspace (`unaos/Cargo.toml:10`), and `std` links only under `cfg(test)`:

```
cd unaos && cargo test -p rast
```

Verified 2026-08-18 from the `hw-jetson` branch at `12b0993c`: **7 passed, 0
failed**, including `cube_scene_matches_golden`. Note that this is a *host*
run — an **x86_64** build of the crate, not an aarch64 one. The claim that the
same digest falls out on aarch64 rests on the determinism contract in section 1
and on the comment at `tests/golden.rs:277-278`; running the goldens under an
aarch64 host or emulator is the check that would turn it from argued into
observed, and that has not been recorded here.

### 3.2 The standing law: one copy, never regenerated to pass

Two rules in [`orin-3d.md`](../../../../unaos/docs/dev/OS/09_PLATFORM/orin-3d.md)
§4 "Do not do" bind this crate directly. They are restated here because this is
the doc a contributor reads first.

> **§4.3 — Do not fork the oracle layer.** `unaos/crates/rast/` is byte-identical
> across `hw-jetson`, `hw-pi4` and `UnaOS-gemini` today, and that is the whole
> point. If a track needs something from `rast`, it is a shared-lane change,
> which means **stop and report** — do not edit it from a platform track.
>
> **§4.4 — Do not regenerate a golden to make a test pass.** Regenerate only
> deliberately, with the reason recorded.

**Why, stated plainly.** The digest is not a regression test for the
rasterizer's own sake. Its value is that it is *the same number everywhere*. Two
consequences follow, and both evaporate the moment a second copy of the crate
exists:

1. **Cross-arch equality is checkable.** x86_64 and aarch64 running the same
   source and arriving at the same 64-bit digest is what proves the determinism
   contract in section 1 actually holds, rather than merely being intended. Two
   forks with two goldens prove nothing about each other.
2. **"GPU output == rasterizer reference" becomes a real test.** The V3D
   milestone generalizes to diffing GPU-rendered pixels against this crate's
   output. That comparison is only meaningful against a single, pinned
   reference. An Orin-flavoured or Pi-flavoured `rast` would leave no
   well-defined thing for the GPU to be equal *to*.

Regenerating the digest to clear a red test destroys the same property from the
other direction: it converts the reference into a record of whatever the code
last did, which is not a reference at all. If output genuinely must change, the
change is deliberate, shared-lane, and the reason is recorded with the new
constant.


## 4. The demo (`UNAOS_RAST=1`, and `UNAOS_PIRAST=1` on the Pi)

A knob-gated demo (`rast` Cargo feature → `unaos/crates/kernel/src/rast_demo.rs`)
renders a spinning, flat-shaded, z-buffered cube through the panel `Screen`. It is
**call-never-edit** with respect to the shared video path: it renders into its own
heap-owned RGBA8 back buffer (the double buffer), then presents each frame through
the public `Screen::put_pixel` / `Screen::flush` API — it touches no shared surface
code. The demo renders at a fixed 320×240 and blits centered on the panel (a full-
panel per-pixel present is far too slow to witness), runs a bounded 90 frames, then
hands the panel back to the shell, emitting one honest fps line:

```
:: RAST: software rasterizer demo — 320x240 spinning cube centered on 1280x800 panel, 90 frames ::
:: RAST: 90 frames in 4115 ms — 21.871 fps (software rasterizer, panel present) ::
```

**Frame pacing (RAST-PACE).** Each frame is held to a target wall-clock interval
(`FRAME_MS = 33`, ≈ 30 fps) so the spin is *visible and platform-consistent* rather than
flashing past. Without pacing the Orin panel finished all 90 frames in ~91 ms (989 fps) —
a ~0.1 s blue flash. The pace is a **pure delay**: the slot deadline for frame *n* is
`t_start + (n+1)·FRAME_MS`, and the loop busy-waits on `crate::arch::ms()` only while the
current wall clock is *behind* that deadline, so a platform whose present is already
slower than the target (x86 panel present at ~22 fps above) never waits and runs at its
own speed — pacing only ever DELAYS, never skips. The busy-wait is bounded by a finite
`PACE_POLL_CAP` poll backstop (never an unbounded spin; a stuck/degenerate clock can't
hang the demo, and QEMU still boots straight through). The emitted fps line reports
MEASURED elapsed time, so it stays honest: ≈ 30 fps where pacing binds, present-bound fps
where the platform is the slower one.

`rast_demo::run()` is arch-neutral (it drives only `Screen` + `crate::arch::ms()`),
so the same code serves three panels; the wire-in differs per platform:

- **x86/virt** (RAST-1) and **aarch64/virt** (RAST-TEGRA): the shared GUI setup in
  `kernel_main` runs the demo just before handing the panel to the shell. The GICv2
  `virt` boot reaches that path with a `ramfb` framebuffer, so a headless QEMU run
  witnesses the demo — `UNAOS_RAST=1 ./arroyo test-arm` prints the two `RAST:` lines
  on aarch64 (a smaller ramfb panel, e.g. 800×600, and a much higher fps).
- **aarch64/tegra — the Jetson Orin Nano panel** (RAST-TEGRA): wired at the tail of
  `tegra_early_stop` (`tegra_rast_demo_maybe`), post-drop at EL1, right before
  `run_capstone_boot_core`. It draws the cube through the **JD1-inherited scanout**
  — no mode-set, no scanout reprogramming; it builds a `Screen` over `video::WRITER`
  (seeded by JD1, mapped into both translation tables so it is reachable at EL1) and
  presents through the same public API. `crate::arch::ms()` reads `CNTVCT` on the
  timerless post-drop core (the VUGFIX tegra fallback), so the fps line still ticks.
  This is the **first 3D pixels drawn on Orin silicon**. QEMU never builds `tegra`,
  so the on-panel cube is verified on the attended Orin bench; the aarch64/virt path
  above is the honest QEMU proof of the same arch-neutral render.

  **Byte-identity wire-in note.** The tegra call is made on the *same source line* as
  the `run_capstone_boot_core` terminus, and the runner itself lives at the file tail
  (with an `#[inline(always)]` empty knob-off twin). This adds zero source lines ahead
  of any panic `Location` literal — the panic-line byte-identity constraint
  (PI-V3D-1 bisect-proven): a stray gated block mid-`kernel_main` shifts embedded line
  numbers and perturbs `.rodata` even knob-off.

- **aarch64/pi — the Raspberry Pi 4 / BCM2711 panel** (PI-RAST): wired at
  `main.rs::pi_rast_demo_maybe`, called on the GUI-handoff `fbcon::detach()` line in
  `kernel_main`'s aarch64/baremetal block. Like tegra this is an **inherited scanout**
  — the panel is whatever the VideoCore firmware handed back through the mailbox
  (`mailbox::init_framebuffer` → `video::WRITER`); there is no mode-set, no scanout
  reprogramming, and nothing in the V3D tree is touched. **Geometry is read live**
  (`screen.width()/height()`), never hardcoded: the bench Pi is 1920×1200 and QEMU
  raspi4b is 640×480, and the fixed 320×240 render is centered on whichever it finds.

  **Why that call site.** The Pi's boot core detaches fbcon, spawns the `input` +
  `render` service tasks onto two APs, then joins the scheduler (`run_bsp`) — unlike
  the Orin, whose terminus *is* the scheduler entry. The demo runs on the detach line,
  which is *after* the panel/framebuffer, heap and timer are up (so it can never race
  bring-up) and *before* `render_service` exists (so it can never fight the compositor
  for the panel). Its full-panel `Screen` shadow (~9 MiB at 1920×1200) is scoped to the
  call and dropped before those spawns, so it never coexists with the render task's
  identical shadow on the 48 MiB metal heap. The 90 paced frames are bounded, so the
  boot always reaches the shell; `render_service` repaints the console over the cube.
  `pal::cursor::SPRITE_OWNS_PAINT` stays `false` on aarch64 — untouched.

  The Pi wire-in prints its own panel-naming header and a second honest fps line
  measured across a strictly wider span (the `Screen` build *plus* the render loop),
  bracketing the shared `:: RAST:` pair:

  ```
  :: PI-RAST: BCM2711 mailbox panel 1920x1200 (live firmware geometry, inherited scanout) — software rasterizer cube, the Pi's first 3D pixels ::
  :: PI-RAST: 90 frames in <n> ms — <f> fps (software rasterizer, BCM2711 mailbox-fb present) ::
  ```

  **Knob class.** `pirast` is a *thin* feature: it only implies `rast`, so the dep gate
  and the `rast_demo` module gate are untouched and this arc shifts no line in `lib.rs`.
  The arm lives in `arroyo`'s **curated** `K8_FEATS` block and deliberately **not** in
  `builder/src/main.rs` — builder produces the x86/virt ESP media and never
  `kernel8.img`, the same class as the V3D / PIUSB / GENET knobs. Same byte-identity
  wire-in discipline as tegra: called on an existing source line, runner + empty
  `#[inline(always)]` knob-off twin at the file tail, zero source lines added ahead of
  any panic `Location`.

With the feature off the whole module + the `rast` dependency are unlinked and the
kernel image is byte-identical to baseline on **both arches** — RAST-1 verified the
x86 sections; RAST-TEGRA re-verified x86 (`.text 9cd6…`→unchanged) and the aarch64
`tegra` kernel (`.text a2ce1599…`, `.rodata 5d1f7604…`, `.data 4f1fe11e…`,
`.data.rel.ro e17e3b13…` all byte-identical vs the pre-arc base), 0 `rast` symbols
knob-off. PI-RAST re-verified the third panel the same way: `kernel8.img` built with
`UNAOS_PIRAST` unset is **byte-identical** (whole-image sha256) to a build of the
pre-arc base at the same worktree.

## 5. Consumers, and the extensions parallelism would need

### 5.1 Who calls the crate today

`rast` has exactly one in-tree consumer: the knob-gated kernel demo. Nothing in
the shared video stack depends on it, which is what keeps the crate free to stay
platform-neutral.

- **`unaos/crates/kernel/src/rast_demo.rs`** — gated by the `rast` Cargo feature
  (`kernel/Cargo.toml:1079`, dep at `1140`), armed by `UNAOS_RAST=1`
  (`unaos/arroyo:181`). The module gate is `#[cfg(feature = "rast")]`
  (`kernel/src/lib.rs:120-121`) and is deliberately **not** arch-gated, so
  x86/virt, aarch64/virt and aarch64/tegra all link the same code.
- Call sites: the shared GUI path at `kernel/src/main.rs:1452`, and the Orin
  terminus line at `main.rs:2459` → `tegra_rast_demo_maybe` (`main.rs:5102`).
- `rast` is a workspace member (`unaos/Cargo.toml:10`) and appears in the
  curated feature sets for `x86-all`, `arm-pi` and `arm-tegra`
  (`unaos/arroyo:1591,1595,1600`).

### 5.2 RAST-MC — frame pipelining, which needed zero crate changes

The multi-core rung on Orin (`rast_demo.rs::run_mc`, `rast_demo.rs:415`, gated
`all(feature = "tegra", target_arch = "aarch64")`) is worth recording here for
one reason: **it required no change to this crate at all.**

It probes each secondary with a pinned `sched::spawn` (`rast_demo.rs:469`),
enlists the cores that actually dispatch, gives each its own full-size RGBA8 +
`f32` depth pair off the heap, and assigns frames round-robin — the core at slot
`k` renders every frame `f ≡ k (mod nslots)`. Each worker calls
`mc_render_frame` (`rast_demo.rs:301`), which builds an ordinary full-frame
`Target::new(color, depth, w, h, w)` (`rast_demo.rs:314`) and makes the ordinary
whole-frame `render_mesh` call (`rast_demo.rs:316`) — byte-for-byte the same
call the single-core path makes at `rast_demo.rs:136`. The boot core presents
finished frames in strict frame order through `Screen::put_pixel`
(`mc_present`, `rast_demo.rs:333`).

This is **frame-level** parallelism, and it fits the existing API precisely
because every worker still owns a whole frame. Pixels and their sequence are
identical to single-core by construction. The cost of staying inside the
contract is Amdahl: only the render half is parallel, the present half stays
serial on the boot core, so the ceiling is
`total / max(present_total, render_total / nslots)` — of order 2× regardless of
core count. Full accounting, including the per-core heap footprint and the
witness lines, is in
[`orin-3d.md`](../../../../unaos/docs/dev/OS/09_PLATFORM/orin-3d.md) §3.1.

> **Status.** RAST-MC's witness lines are **PENDING on Orin silicon** — it had
> not run on the Jetson as of 2026-08-18, and QEMU cannot stand in for the
> `tegra` path. That pending status is a property of the wire-in, not of this
> crate. It is **no longer pending as a mechanism**, though: see §5.2a.

### 5.2a RASTPORT — the same rung on x86 (`UNAOS_RASTMC=1`)

RAST-MC is no longer aarch64-only. Its 21 `#[cfg]` sites moved from
`all(feature = "tegra", target_arch = "aarch64")` to
`any(that, all(feature = "rastmc", target_arch = "x86_64"))`, and the rung ran
end to end under QEMU on x86 with **verdict PASS, 2.090x vs a same-boot 1-core
baseline**, 5 render cores each rendering exactly 18 of 90 frames. That number
sits on the Amdahl ceiling the section above predicts a priori.

Three bridges were needed, not the two the triage expected — `percpu::NUM_CPUS`
→ a neutral `sched::sched_cpu_slots()` (`gdt::MAX_CPUS` on x86), a new x86
`sched::online_cpu_count()` twin, and a shim for `sched::spawn`, whose x86
signature takes a fifth `priority` argument. Crate changes: **still zero.**

Two facts a reader of this section needs:

- **Arming `rast` on x86 compiles the SCHED-X86 render/service handoff out of
  the boot** (`main.rs`'s `not(feature = "rast")` gate). The BSP stays inline,
  which is why the demo's call site is reachable at all and why the presenter
  has no render-lane peer. There is correspondingly no scheduled render lane on
  such a build, so nothing here says anything about coexisting with one.
- **Under `wc` with a real Kepler takeover the pixels do not reach the glass.**
  `desktop_uefi::activate()` opens a centred console window;
  `Screen::present_background` subtracts occluder boxes *before* copying to the
  framebuffer, so a centred demo blit is discarded and `flush()` reports success
  anyway. Timings and serial witnesses stay honest. **QEMU has no Kepler, so
  QEMU shows the cube and the bench rMBP will not** — making this demo visible
  on x86 under the compositor means rendering into a window instead of poking
  panel coordinates, which is a design change to a `call-never-edit` path and
  was deliberately not attempted.

### 5.3 The Pi consumer

The Pi 4 wire-in (**PI-RAST**, `UNAOS_PIRAST=1`) drives the same arch-neutral
`rast_demo::run` through the BCM2711 mailbox panel, reading panel geometry live
rather than hardcoding it. Its section of this document lives on the `hw-pi4`
branch and has not yet reached this branch — see the note in section 6. It, too,
required no change to the crate.

### 5.4 Proposed API extensions — PROPOSALS, not decisions

Band or tile decomposition is the decomposition that would parallelize *both*
halves of a frame and scale with core count. Section 2.4 and 2.5 explain why it
is not expressible through the current API. Two minimal extensions would lift
the block. **Both are proposals recorded for discussion. Neither has been built,
neither has been agreed, and neither may be implemented from a platform track:**
`rast` is shared-lane and golden-pinned (§3.2), so any change here requires
agreement across the track seats and a re-verified `GOLDEN_CUBE_07`.

- **(a) A viewport origin on `Target`.** Something of the shape
  `Target::new_offset(color, depth, w, h, stride, origin_x, origin_y)`, where the
  *frame* dimensions used by `to_screen` stay `(w, h)` while the *writable* rows
  are the band. Smaller change; solves only the band case.
- **(b) A public split of the pipeline.** Something of the shape
  `project_mesh(model, view_proj, verts, indices, w, h, cull, &mut FnMut([ScreenVert; 3], Rgba))`,
  emitting already-projected, already-shaded, already-clipped triangles that a
  caller may offset and hand to the existing public `Target::triangle`.

(b) is the more useful of the two: besides bands and tiles, it is also what a
future GPU-vs-reference diff wants, since it exposes the exact triangle stream
the reference rasterized. Both options are stated in
[`orin-3d.md`](../../../../unaos/docs/dev/OS/09_PLATFORM/orin-3d.md) §3.1 in the
same terms.

## 6. Known divergence between branches

As of 2026-08-18 the crate `unaos/crates/rast/` is one identical tree object on
every track branch (verified in section 1), but **this document is not**:
`hw-pi4` carries a PI-RAST subsection in section 4 and an extra byte-identity
sentence that `hw-jetson`, `origin/main`, `UnaOS-gemini` and `hw-rmbp` do not
have. Compare with:

```
git diff origin/main:docs/dev/OS/08_VIDEO/rasterizer.md \
         origin/hw-pi4:docs/dev/OS/08_VIDEO/rasterizer.md
```

The two sets of additions are in different sections and are meant to **union**,
not to supersede each other. Whichever arc lands second reconciles by keeping
both.

## See also
- `unaos/crates/rast/` — the crate.
- [orin-3d](../../../../unaos/docs/dev/OS/09_PLATFORM/orin-3d.md) — §3.1 the
  RAST-MC rung and the parallelism analysis; §4.3/§4.4 the no-fork and
  no-regenerate laws.
- [framebuffer](framebuffer.md) — the panel surface the demo presents through.
- [engine](engine.md) — the existing 2.5-D `vug` facet renderer (distinct: a
  painter's-order solid-facet engine, not a z-buffered 3D rasterizer).
