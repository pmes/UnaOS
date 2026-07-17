# RAST-1 landing — software rasterizer: the platform-neutral 3D target layer

**Branch:** `us-rast1` (off main `03105f0`). **Lane:** new `unaos/crates/rast`
crate + one knob-gated x86/virt demo wire-in (`UNAOS_RAST=1`). No Pi/BCM2711 or
tegra files touched.

## What landed

- **M1 — core crate (`unaos/crates/rast/`, `no_std`, zero-alloc hot path).**
  `f32` 3D math (`Vec3`/`Vec4`/column-major `Mat4`, perspective, look-at,
  rotations), flat-shaded z-buffered triangle fill into a caller-provided RGBA8
  slice, back-face cull, near-plane (`w`) clip, top-left fill rule. **Float choice
  documented: `f32` throughout, all ops the IEEE-754 primitives + `libm`
  (`sqrtf`/`floorf`/`sinf`/`cosf`)** — pure-Rust, correctly-rounded, byte-identical
  on both arches (the same kernel/host bit-identity `libs/fs/unafs` relies on). No
  fma (matrix inner loop written as an explicit sum of products), no fast-math.
- **M2 — host golden tests (`tests/golden.rs`, `cargo test -p rast`).** 7 tests,
  FNV-1a full-buffer digests. Coverage: single triangle + cull, z-fighting order
  (nearer wins both orders, orders converge byte-identically), near-plane clip (no
  NaN), degenerate/zero-area triangles, and a cube scene pinned to a cross-arch
  golden digest (`GOLDEN_CUBE_07 = 0x194446bca3dea139`).
- **M3 — demo wire-in (`crates/kernel/src/rast_demo.rs`, `rast` feature).**
  Spinning flat-shaded cube rendered into a heap-owned RGBA8 back buffer, presented
  centered on the panel through the public `Screen::put_pixel`/`flush` API
  (call-never-edit — no shared surface code changed). 320×240 render, bounded 90
  frames, honest fps line, then hands the panel to the shell. x86/virt only.
- **M4 — doc.** New `docs/dev/OS/08_VIDEO/rasterizer.md` (crate API, determinism
  contract, V3D-oracle role, golden-image scheme, the demo knob).

## Gate results

- `./arroyo check` **knob-off** — ✅ x86_64 OK, ✅ aarch64 OK.
- `./arroyo check` **knob-on** (`UNAOS_RAST=1`) — ✅ x86_64 OK, ✅ aarch64 OK.
- `cargo test -p rast` — **7 passed, 0 failed** (golden suite).
- Full x86 QEMU regression (`./arroyo test`) **knob-off** — green, **21 PASS, 0
  FAIL / 0 panic**.
- **Knob-on virt demo witnessed** (`UNAOS_RAST=1 ./arroyo test`), serial:
  ```
  :: RAST: software rasterizer demo — 320x240 spinning cube centered on 1280x800 panel, 90 frames ::
  :: RAST: 90 frames in 4115 ms — 21.871 fps (software rasterizer, panel present) ::
  ```

### Knob-off byte-identity (lane-cleanliness proof)

The full-ELF hash differs between base and branch **only in symbol/debug metadata**
(the two builds live at different worktree absolute paths, which leak into
`.symtab`/`.strtab`). All **loadable/semantic sections are byte-identical** between
base `03105f0` and branch `us-rast1`, knob-off:

| section | base `03105f0` | branch `us-rast1` |
| --- | --- | --- |
| `.text` | `9cd658d94d93d5fab0e76f64a0870582c45b0d354de38902c8c8ac1796ee1549` | **identical** |
| `.rodata` | — | **MATCH** |
| `.data` | — | **MATCH** |
| `.data.rel.ro` | — | **MATCH** |

(`.text` digest `9cd658d9…`; `nm` shows zero `rast` symbols in the knob-off
binary — the module + `rast`/`libm` deps are unlinked.) The knob-off kernel ELF
hash on this host was `0424d333…` (path-metadata-bearing; not a cross-machine
constant — the section-level identity above is the durable proof).

## Maestro seam addendum — honored

The concurrent PI-V3D-1 arc shares the three build-wiring files. RAST-1 added
**only its own line** to each, in the existing pattern, no restructuring:
- `unaos/crates/kernel/Cargo.toml` — `rast = ["dep:rast"]` feature + optional x86
  `rast` dep.
- `unaos/arroyo` — one `UNAOS_RAST → rast` `_feats` line.
- `unaos/builder/src/main.rs` — one `UNAOS_RAST` push line.

## Flagged / residual

- Demo is x86/virt only by brief. Pi 4 / Orin wire-ins are deliberately deferred
  to after this arc and PI-V3D-1 merge.
- Software present is per-pixel `put_pixel` (fixed 320×240 to stay witnessable);
  a faster bulk-blit present is a future optimization, not in this arc's lane.
- No security surface (new leaf crate + gated demo, call-never-edit on the panel
  API) — seat-read tier per the brief; no lens required.

## DONE gate — PASS

All brief expected outputs met: both-arch check (knob on+off), x86 regression
green knob-off, golden suite green, knob-on demo witnessed, doc landed, this report.
