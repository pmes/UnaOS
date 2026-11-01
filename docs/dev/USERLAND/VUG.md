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
8. **The machine is a first-class model.** Kinematics, envelope, tool library, stock, fixtures and feeds live in
   `make_core`; a program is reviewed against them before Helm sees it; a program that would crash the machine is a
   refusal by name, not a warning.
9. **Measure the result.** The probes (Sonde) close the loop: scan the stock before, probe the part after; the
   deviation is a number on the model, not a photo.
10. **One core, both rings.** The cores run on the Mac (the host faces), on UnaOS (the ELF face) and in the kernel
    where a service needs them, the same bytes (BP-vug-4).

## 4. The research (2026-11-01)

Three reports, in scratch at the time of writing and summarised here once they land: (4.1) what engineers want from
a next-generation CAD app and where today's tools fail; (4.2) eliminating the CAD → CAM → post → controller chain:
STEP-NC, MTConnect/OPC UA, feature-based CAM, closed-loop machines; (4.3) scan-to-CAD with the D435i and the Rust
geometry-kernel landscape. **This section is filled in by the seat when the reports land (the same session).**

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
