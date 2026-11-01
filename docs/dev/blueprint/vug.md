# BP-vug — Vug, the 3D handler: CAD and Design → Make (read at c2d86af47)

## 1. What it is
- CODEX §2: Vug (3D) is "CAD viewing, editing, and CAM/Slicing"; CODEX §5 the Hardware Protocol: Vug computes the toolpath, Helm holds the wheel, Comscan pumps the stream; "no file exporting, no slicer app".
- At the read: `handlers/vug` is a rendering prototype (`handlers/vug/src/renderer.rs` a GTK4 GL cube; `handlers/vug/src/viewport.rs` a wgpu triad, not linked), 475 lines, no ignite, no bus; `temple_core` (SR147) is the one Vug core in the tree (the TEMPLE renderer). No geometry kernel, no sketch, no mesh I/O, no CAM exists.
- The probes are Sonde's: `probe_core` holds the depth scale, the pinhole intrinsics and `point_cloud` (`unaos/libs/media/probe_core/src/depth.rs:122`); the D435i path (kernel UVC + xHCI isochronous, B534, SR92, SR101, SR105) is fixed-unflown: every flight to date reads `:: SONDE: dev=none … -> SKIP(no-device) ::`.
- The program (docs/dev/USERLAND/VUG.md, Peter 2026-11-01: "making sure UnaOS becomes people's go-to for CAD … bake the concept of direct manufacturing in"): the CAD cores under `unaos/libs/cad/` — `geom_core`, `sketch_core`, `mesh_core`, `brep_core`, `scan_core`, `make_core` — `no_std` + `alloc`, `forbid(unsafe_code)`, the midden_core shape (BP-handlers-4); the handler, the faces and the kernel link them.

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-vug-1 | Geometry is `f64` in the CAD cores (a 1 m part held to 1e-6 mm is 1e-9 relative, past f32's 6e-8; every B-rep kernel in use is double); euclase stays `f32` (VECTORS_DIRECTIVE governs the GPU math) and the one conversion is at the render seam, `mesh_core`'s `f32` buffers. | `libs/euclase/src/vec3.rs` `docs/dev/VECTORS_DIRECTIVE.md:11-30` | review; `cargo test -p geom_core` |
| BP-vug-2 | One tolerance policy: every core takes `geom_core::Tol` (linear in millimetres, angular in radians) as an argument; no core carries an epsilon of its own, and a comparison that needs one names its `Tol`. | `docs/dev/USERLAND/VUG.md` | `cargo test -p geom_core` |
| BP-vug-3 | The design holds the Make: a manufacturing operation is attached to the model (make_core's `Op` on a feature or a body), never to an exported file; a machine stream leaves Vug only as make_core's `Program` through Helm's gate to Comscan (CODEX §5). | `docs/CODEX.md:73-78` `unaos/libs/sys/helm/src/lib.rs` | review |
| BP-vug-4 | Every core is pure: a value in, a validated value or a named refusal out; no I/O, no clock, no allocation outside `alloc`; host and kernel link the same crate with `default-features = false` (BP-handlers-4, the timeline_core manifest shape). | `unaos/libs/media/timeline_core/Cargo.toml:7-10` | `cargo metadata` members; review |
| BP-vug-5 | A scan is a measurement, not a model: scan_core's mesh carries per-vertex confidence and the probe's intrinsics; a B-rep recovered from it is a separate step whose fit residuals are printed, never a silent promotion to design geometry. | `unaos/libs/media/probe_core/src/depth.rs:60-110` | review |
| BP-vug-6 | The formats the cores read and write without a middleman: STL, OBJ, PLY (mesh_core), 3MF and STEP AP242 (brep_core, owed), G-code dialects (make_core); the native document is a text form a diff can read. | `docs/dev/USERLAND/VUG.md` | `cargo test -p mesh_core -p make_core` |

## 3. Seams
- Sonde -> Vug: `SMessage::ProbeFrame` (`handlers/sonde/src/lib.rs:145`) carries a depth frame with its scale and intrinsics; scan_core consumes probe_core frames, never the bus.
- Vug -> Helm -> Comscan: make_core emits `Program` (the machine-neutral IR) and ONE dialect writer per controller family; Helm's review reads the `Program` and the `Machine`, never the text.
- Vug -> euclase / quartzite: mesh_core tessellation at `f32` for the viewport; the faces hold no geometry of their own.
- Vug -> Aulë: none at the read (CODEX §4 binds Aulë in the background of a CAD project; what it builds there is unruled).

## 4. Wire
No kernel wire at the read. Host: `cargo test -p geom_core`, `-p sketch_core`, `-p mesh_core`, `-p make_core` (each crate's own suite is its witness until a face exists).

## 5. Ruled, unbuilt / owed
- VUG-GEOM (LEDGER SR464, in flight): `geom_core` — f64 vectors, matrices, transforms, `Tol`, AABB, the exact orientation predicates, the closed-form intersections.
- VUG-SKETCH (LEDGER SR465, in flight): `sketch_core` — the 2D constraint solver (entities, constraints, Gauss-Newton / Levenberg-Marquardt, DOF and redundancy named, drag solve, profile extraction).
- VUG-MESH (LEDGER SR466, in flight): `mesh_core` — the indexed triangle mesh, half-edge adjacency, normals, BVH pick, manifold check, weld, STL / OBJ / PLY.
- VUG-MAKE (LEDGER SR467, in flight): `make_core` — `Machine`, `Tool`, `Stock`, `Op`, the `Program` IR, grbl / Marlin / LinuxCNC writers and the reader, `review` for Helm.
- VUG-BREP-1 (LEDGER SR468, in flight): `brep_core` — topology (vertex, edge, loop, face, shell, solid) over the analytic surfaces (plane, cylinder, cone, sphere, torus) and lines / circles / NURBS curves, the validator, primitives, extrude and revolve of a sketch `Profile`, mass properties, tessellation into mesh_core with a stated chord error; VUG-BREP-2 booleans, VUG-BREP-3 fillets and chamfers, VUG-STEP (AP242), VUG-3MF (owed).
- VUG-SCAN-1 (LEDGER SR469, in flight): `scan_core` — TSDF fusion of probe_core clouds with given poses, marching cubes into mesh_core with per-vertex confidence, plane / cylinder RANSAC fitting with residuals; VUG-SCAN-POSE (frame-to-model ICP) and VUG-PROBE (the D435i on metal; SR105 fixed-unflown) owed.
- VUG-FACE-1 (LEDGER SR470, in flight): the host viewport vessel vug-view (a new vessel directory beside facet-view) on quartzite (macOS first) + euclase: a mesh_core mesh drawn, orbit about the cursor / pan / zoom with remappable presets (SolidWorks, Fusion, Blender, Onshape), pick with the BVH, STL / OBJ / PLY open; VUG-FACE-2 the sketch face, VUG-FACE-ELF the ring-3 face (owed).
- VUG-DOC: the text document and its reader / writer; VUG-HELM: Helm levels `make_core::review`; VUG-COMSCAN: the stream to a serial controller with grbl ok / ack (owed).

## 6. Read further
- docs/dev/USERLAND/VUG.md (the program, the research, the principles); docs/CODEX.md §5; handlers/vug/README.md; `unaos/libs/media/probe_core/src/lib.rs`.
