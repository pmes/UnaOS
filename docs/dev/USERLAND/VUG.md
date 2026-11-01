# VUG — the CAD program: Design → Make, with the machine on the other end

Peter, 2026-11-01 (verbatim): "i would like you to go off the deep end making sure UnaOS becomes people's go-to for
CAD. one of our long-term goals is to have whatever machine tool hooked up and controlled directly by UnaOS. this
means we need research on what engineers want in the ultimate CAD app. there's got to be something out there on next
generation designs eliminating the complications of CAD/CAM. don't worry about actually controlling a machine right
now but bake the concept of direct manufacturing in." Earlier taste, standing: "clean flowy non-presumptuous apps".

Canon: CODEX §2 (Vug: 3D — "CAD viewing, editing, and CAM/Slicing"), CODEX §5 (the Hardware Protocol: Vug computes,
Helm holds the wheel, Comscan pumps; "no file exporting, no slicer app"), CODEX §4 (a CAD project binds Vug + Comscan
+ Aulë). The sheet is `docs/dev/blueprint/vug.md`. This document holds the program, the measurements and the research;
the sheet holds the invariants; the ledger holds the rows.

## 1. Where the tree is (read at c2d86af47)

- **Vug**: a 475-line rendering prototype (`handlers/vug`): a GTK4 GL cube and an unlinked wgpu triad. No handler
  ignite, no bus verbs, no geometry. `temple_core` (SR147) is a software ray tracer for the TEMPLE scene, not a CAD core.
- **The D435i**: the kernel streams a UVC device over the xHCI isochronous pipe (XHCIISO B534), the D400 census and
  vendor extension unit are read (REALSENSE SR92), the cfg is both arches (ARMSONDE SR101), and the first depth frame
  reaches Sonde's handler path with `probe_core`'s depth scale, pinhole intrinsics, deprojection and `point_cloud`
  (SONDE2 SR105). All of it is **fixed-unflown**: no D435i frame has been captured on metal; every flight reads
  `:: SONDE: dev=none … -> SKIP(no-device) ::`. The next metal boot with the camera on a USB 3 port is the witness.
- **Make**: nothing. Comscan is a 330-line log view; Helm has the rover's wheel (`unaos/libs/sys/helm`), no machine gate.

## 2. The D435i and CAD — what it is for (the honest answer)

The D435i is a stereo depth camera: 848x480 depth at up to 90 fps, a usable range of about 0.3 m to 3 m, and an
accuracy that degrades with the square of the distance (roughly 2 % at 2 m, a few millimetres at 0.5 m). That makes it:

- GOOD for: scanning a room, a bench, a fixture, a stock block or a found object's **envelope** to work against;
  hand and gesture input for the viewport; a live reference view beside the model; checking that a printed or
  machined part is the shape that was designed to a few millimetres; the rover's and the machine's obstacle sense.
- NOT for: sub-millimetre part metrology or reverse-engineering a precision part from a scan alone. A scan into Vug
  is a **measurement** with a stated confidence (BP-vug-5), the model is drawn against it, and a B-rep recovered from
  it prints its fit residuals.

So yes: the camera is a CAD input, as an envelope and a reference, and it is Sonde's job to deliver it as frames and
clouds (`probe_core`) that Vug's `scan_core` fuses. The research on what the camera can and cannot do is §4.3.

## 3. The principles (what the research says engineers want; see §4 for the evidence)

1. **No file in the middle.** Design → Make is one document: the part knows how it is made. An operation is attached
   to the model, the stream to the machine is derived, and the only hand-off is Helm's gate (BP-vug-3).
2. **A sketch should feel like drawing.** Constraints inferred as you draw, shown, and removable; the solver keeps the
   rest of the sketch still while you drag one thing (the drag solve); over-constraint named, never a red mystery.
3. **The feature tree must not break.** Topological naming is the reason every history-based CAD app rots; the model
   references geometry by stable semantic ids, and a reference that cannot be re-resolved says so by name.
4. **Direct and parametric are one tool.** Push a face and the sketch behind it updates; edit the sketch and the push
   stays. (Hybrid history; the research names the products that got there.)
5. **Own the geometry.** A kernel we read and test (geom, sketch, mesh, brep: `unaos/libs/cad/`, `no_std`, f64,
   one tolerance policy), not a wrapped OpenCascade. Robustness is a test suite, not a hope.
6. **The formats people actually exchange** open and save without a translator: STL, OBJ, PLY, 3MF, STEP AP242,
   G-code; the native document is text a diff can read (BP-vug-6).
7. **Clean, flowy, non-presumptuous.** Three-button mouse + keyboard + pen; no modal dialogs for the common path;
   every pick snaps and says what it snapped to; the viewport never drops below the refresh under an edit.
8. **The machine is a first-class model and the app is the planner, never the executor.** Kinematics, envelope,
   tool library, stock, fixtures and feeds live in `make_core` (MDES and ISO 13399 shapes); the app plans every move
   from physics and ships a timestamped queue to a dumb clock-synchronised executor (Klipper's split); E-stop, STO
   and limits are hardwired, never the app's; a program is reviewed against the model before Helm sees it; one that
   would crash the machine is a refusal by name, not a warning.
9. **Measure the result.** The probes (Sonde) close the loop: scan the stock before, probe the part after; the
   deviation is a number on the model, not a photo.
10. **One core, both rings.** The cores run on the Mac (the host faces), on UnaOS (the ELF face) and in the kernel
    where a service needs them, the same bytes (BP-vug-4).

## 4. The research (2026-11-01; the three reports are in `docs/dev/evidence/vug/`)

### 4.1 What engineers want (`research-cad-wants.md`, ~60 sources, 17 verbatim complaints)
The pain list has been stable for a decade: fragile feature trees (the topological naming problem is universal, not
FreeCAD's alone: "fillets broke, and I had to reselect which edges"), STEP stripping intent ("all design intent,
associativity, assembly relationships"), subscription and cloud hostage anxiety ("files can be held hostage if you do
not pay them every year"), large-assembly rebuilds ("death by a thousand cuts"), modal undiscoverable UI, shallow undo
that dies on save, no real version control ("merging in the style of git is a completely foreign concept"), and a
scarcity of kernels under all of it (Parasolid's licence sets the price floor of Plasticity and Shapr3D; Fornjot, the
Rust kernel, shut down 2026-06-19: "goals were never reached"). Next-generation bets that work: direct-first with
optional history (Solid Edge ST, Shapr3D), Blender-like hotkey grammar (Plasticity), fields (nTop: booleans and
offsets that do not fail), code as the artefact with a GUI that emits it (Zoo KCL, build123d), git-native text models.
The ranked twelve the report ends with, which this program adopts as its bar: (1) stable references by design,
(2) hybrid direct + recoverable history, (3) owned, open, text-diffable files and an offline option, (4) lossless STEP
AP242 / 3MF, (5) never block the UI, (6) instant conventional remappable navigation that orbits about the cursor,
(7) sketch inference that is visible and never silently over-defines, drag-first and type-to-override, (8) deep
model-only undo and git-grade branch / merge of a text artefact, (9) assembly scalability by architecture, (10) one
command grammar, (11) GUI and code bidirectional, (12) fail-safe kernel operations (fields for the operations B-rep
hates, B-rep for exchange). Latency: navigation and selection at the refresh, sketch feedback under 100 ms, feature
regeneration with a progressive ghost. Gap: no 2024-2026 survey with original data was found; Reddit is unindexed.

### 4.2 Eliminating the CAD → CAM → post → controller chain (`research-direct-manufacturing.md`, ~70 sources)
Nobody has eliminated it in production; the closest are single-vendor vertical stacks (OMAX, Glowforge, Carbide,
Bambu) and BYU's patented direct machining (the controller reads geometry into its motion buffer). STEP-NC (ISO 14649
/ 10303-238) is technically alive (edition 3, 2022; kinematics, tolerances, additive, a machining twin) and
commercially stalled: post-processors are the vendors' moat and the same neutral file cuts differently per controller.
MTConnect is read-only telemetry; OPC UA 40501/40502 monitors; the real write channels are proprietary (FOCAS, Siemens,
Okuma). Klipper is the cleanest answer to where real time lives: the host computes every step time from physics and
kinematics and ships a timestamped queue to an MCU that only fires on the tick (25 µs precision, 100 ms ahead,
multi-MCU clock sync). LinuxCNC's HAL (typed pins, a 1 ms servo thread) with Mesa FPGA step generation is the
industrial pattern. Automatic CAM products remove the programmer and keep CAM + post. Feature recognition is a research
frontier (B-rep GNNs at 99 %); PARC's feature-free configuration-space planning argues taxonomies limit usability;
SDF / implicit toolpaths make offset, slice and collision trivial. Metrology feeds back to offsets commercially, not to
the model; the NIST pattern is STEP for why, MTConnect for what, QIF for how, keyed by persistent ids. Bambu's 2025
lock-out of third-party slicers is the warning against building on a vendor's loop. The report's ten principles, which
`make_core` carries: plan on the host and execute on a clock-synchronised dumb executor; keep safety (E-stop, STO,
limits) in hardwired relays and a PLC, never in the app; ship geometry and intent and verify geometrically before
anything moves; make the machine a declarative model the app owns (MDES for machines, ISO 13399 for tools); plan from
the solid with feature recognition as a hint; use a distance field as the in-process model for stock, offsets and
collision; constrain the design by the process while designing; close the loop into the model with typed metrology on
persistent ids; treat the controller as a replaceable executor; abstract the machine with typed pins so mills, lathes,
printers and robots share planners. Protocols in order: Klipper MCU + Moonraker, grbl 1.1 streaming with the real-time
bytes, LinuxCNC HAL with Mesa, an MTConnect client, OPC UA 40501/40502, STEP AP242 / AP238 out and QIF in, MDES and
ISO 13399 in, FOCAS last. Not first: Bambu MQTT, Siemens MyCC, Prusa Connect cloud.

### 4.3 The D435i and the kernel landscape (`research-scan-and-kernels.md`, ~100 sources)
The sensor: active stereo, 50 mm baseline, best at 848x480, Min-Z about 0.2 m, ideal 0.3 to 3 m, error growing with
the square of distance; measured 1 mm at 0.62 m, 4 mm at 0.85 m, 10 to 12 mm at 1 to 1.15 m, bias unmeasured; the
IMU gives no pose (visual odometry is the app's). Fused: a 30 cm part to ±1-3 mm, a 1 m fixture to ±5-10 mm, a room
to ±2-5 cm; a 10 cm part wants the D405. Good for envelopes, datum planes and cylinder axes, live clouds and gesture
input; not for sub-millimetre metrology, features under 3 mm, shiny or black surfaces. The pipeline: fragments with a
pose graph (Choi, Zhou, Koltun) or KinectFusion-class frame-to-model ICP, TSDF voxel blocks, marching cubes, Schnabel's
Efficient RANSAC for planes, spheres, cylinders, cones and tori, then constrained primitive fitting and sketch recovery
with residuals (Geomagic Design X and QuickSurface are the professional bar, Polycam ±12 mm the consumer one).
Kernels: Parasolid and ACIS are the bar (tolerant modelling, region-controlled booleans); OpenCascade's mesher fails
on 1.6 % of ABC and 8.8 % of Fusion models and its tracker documents fillet and boolean failures; in Rust, truck
(NURBS, booleans, no fillets), nacre (exact arithmetic, refuses rather than guesses), Fornjot (dead; its post-mortem:
B-rep is hard, prototype early, be a tool); Zoo's engine is closed and rejects implicit modelling (cubic scaling, lossy
extraction); libfive and fidget are the implicit references, manifold the robust mesh-boolean one. The
recommendation this program takes: a hybrid, an exact-arithmetic B-rep over the analytic surfaces (plane, cylinder,
sphere, cone, torus) as the master, a parallel distance-field evaluator for cross-checks, lattices and the in-process
model, the feature recipe as the source of truth, refuse-don't-guess; not pure NURBS B-rep, not pure implicit. The
first ten capabilities in order: numerics + predicates + intervals + a Refusal type with determinism tests; half-edge
topology with a validator; analytic surfaces with closed-form intersections; the 2D sketch solver with DOF reporting;
extrude / revolve + mass properties; planar booleans checked by volume identities and SDF sampling; tessellation with
a stated chord error + STL / 3MF / glTF; quadric booleans; analytic fillets and chamfers, then offset and shell;
STEP AP242 and RANSAC fitting of D435i clouds as datum geometry. Defer: general NURBS intersections, sweeps and lofts,
draft, sheet metal, assemblies.

## 5. The architecture

```
 Sonde (probe_core: frames, clouds)        Helm (the wheel)            Comscan (serial, USB)
        │                                      ▲                            ▲
        ▼                                      │ review(Program, Machine)   │ dialect text
  scan_core ──mesh──▶ mesh_core ◀──tess── brep_core ──bodies──▶ make_core ──Program──┘
                         ▲                     ▲                     ▲
                         │ f32 buffers         │ profiles            │ features carry Ops
                      the faces            sketch_core            the document (text)
                   (quartzite+euclase,                 ▲
                    the UnaOS ELF face)             geom_core (f64, Tol, predicates)
```

- `geom_core` — f64 vectors, matrices, transforms, intervals, AABB, `Tol`, exact orientation predicates, the
  closed-form intersections. Everything else depends on it.
- `sketch_core` — 2D entities and constraints, the solver, DOF and redundancy analysis, the drag solve, profiles.
- `mesh_core` — the triangle mesh with half-edge adjacency, normals, BVH pick, manifold check, weld, STL/OBJ/PLY.
- `brep_core` — vertices, edges, loops, faces, shells, solids over planes, cylinders, cones, spheres, tori and NURBS;
  primitives, extrude/revolve of a profile, tessellation into mesh_core, then booleans, fillets, STEP AP242, 3MF.
- `scan_core` — TSDF fusion of probe_core clouds with poses, marching cubes into mesh_core, primitive fitting.
- `make_core` — Machine, Tool, Stock, fixtures, Op, the `Program` IR, dialect writers and the reader, `review`.
- The document — a text form: sketches, features, their Ops, in dependency order; the only persistent artefact.
- The faces — the host viewport (quartzite + euclase, macOS first as every face here), the UnaOS ring-3 ELF face
  (`APPS/VUG.ELF`), one painter, the three-button convention, pen and touch where the host gives them.

## 6. The ladder (ledger ids as they are cut)

| arc | id | builds |
|---|---|---|
| VUG-GEOM | SR464 | `geom_core` |
| VUG-SKETCH | SR465 | `sketch_core` |
| VUG-MESH | SR466 | `mesh_core` |
| VUG-MAKE | SR467 | `make_core` (model, IR, dialects, review) |
| VUG-BREP-1 | — | topology + analytic surfaces + primitives + extrude/revolve + tessellation |
| VUG-SCAN-1 | — | TSDF fusion + marching cubes on probe_core frames (synthetic fixture until a metal frame) |
| VUG-FACE-1 | — | the viewport (mesh view, orbit/pan/zoom, pick, STL/OBJ open) on quartzite |
| VUG-DOC-1 | — | the text document and its reader/writer |
| VUG-BREP-2 | — | booleans; VUG-BREP-3 fillets/chamfers; VUG-STEP; VUG-3MF |
| VUG-HELM | — | Helm reads `review`; VUG-COMSCAN the serial pump with ok/ack; VUG-PROBE the D435i on metal |

Each arc is measured (R131) before it builds; each new crate is a root-workspace member with its own suite as the
witness; nothing here touches the kernel until a core is linked by a kernel face.
