# Research: (Q1) scan-to-CAD with a RealSense D435i, (Q2) CAD-kernel landscape for a from-scratch Rust core

Date: 2026-11-01. Web research only; no code. Every claim that matters carries a URL. Where a source is a vendor page, forum post or blog, it is labelled as such.

---

## Q1. Scan-to-CAD with an Intel RealSense D435i

### 1.1 What the sensor actually is

| Property | Value | Source |
|---|---|---|
| Depth technology | Active stereo (IR projector + two IR imagers), ~50 mm baseline, 90° HFOV | Intel tuning guide (D435 example: HFOV 90°, Xres 848, baseline 50 mm) — https://dev.realsenseai.com/docs/tuning-depth-cameras-for-best-performance |
| Depth resolution / fps | up to 1280x720; up to 90 fps at lower res | https://www.realsenseai.com/products/depth-camera-d435i/ |
| Optimal depth resolution | **848x480** for the D435 family; lower resolutions degrade precision; Min-Z scales linearly with X-resolution | tuning guide (above) |
| Min-Z at 848x480 | ~16.8–19.5 cm (the guide itself gives both numbers); product page says "Min-Z 20 cm", ideal range 0.3–3 m | tuning guide; product page |
| Stated accuracy | "<2 % at 2 m" (Z-accuracy, HD resolution, 80 % ROI) | https://support.realsenseai.com/hc/en-us/articles/360059129453-Depth-accuracy-for-Intel-RealSense-D400-Series-Cameras ; https://intel.com/content/www/us/en/support/articles/000030916.html |
| Error law | "The depth error scales as the square of the distance away" (RMS error ≈ dist² · subpixel / (focal_px · baseline)); well-calibrated subpixel RMS < 0.1 px, recalibrate above ~0.2 px | tuning guide; subpixel linearity white paper https://dev.realsenseai.com/docs/white-paper-subpixel-linearity-improvement-for-intel-realsense-depth-cameras/ |
| IMU | BMI055 6-DoF gyro + accel; **no onboard pose** — you must run VO/VIO yourself (unlike the discontinued T265) | https://discuss.ardupilot.org/t/d435i-for-poshold-in-gps-denied-environments/141147 (forum); https://www.gpsworld.com/two-little-eyes-that-can-see-and-navigate/ |
| SDK | librealsense2, Apache-2.0; examples include `pointcloud`, `measure`, `motion` (IMU rotation) — **no KinectFusion-style reconstruction example** in `examples/` | https://github.com/IntelRealSense/librealsense/tree/master/examples |

Independent measurements of the D435i (both are blogs/reports, not peer-reviewed):

- Gee, "RealSense D435i Accuracy (Indoors)", ChArUco board, 640x480: 3D error 1.06 mm @ 621 mm, 1.59 mm @ 745 mm, 4.0 mm @ 848 mm, 10.5 mm @ 995 mm, 12.4 mm @ 1151 mm; plane-fit residual 0.9 → 3.1 mm over the same range. https://wildboar-dev.github.io/RealSense-Accuracy/
- 4DFS foot-scanner report: plane-fit RMSE < 0.02 mm (spatial noise only, 50-frame average, 200–700 mm, 848x480; **does not measure absolute offset**) and finds 848x480 the best resolution, higher laser power and smaller ROI help. https://jurc192.github.io/4DFS_updates/accuracy_report
- arXiv 2501.18315 (surface-defect study): a RealSense at 50 cm settled at RMSE just above 2 mm against a reference. https://arxiv.org/pdf/2501.18315
- arXiv 2408.01615 (3.5 mm soft robots): D435i point clouds "significant noise and incomplete details"; authors switched to the D405. https://arxiv.org/pdf/2408.01615
- RealSense forum (MartyG): D435 is poor at small objects; D405 recommended for close range. https://support.intelrealsense.com/hc/en-us/community/posts/360033356573-Regarding-D435-Object-Scanning-Limitations

Reading of the numbers: single-frame, single-view depth at 0.4–0.7 m is ~1–2 mm RMS noise with an unknown systematic bias of a few mm; at 1 m it is ~1 cm; at 2 m it is ~4 cm (2 %). Averaging and multi-view fusion reduce noise but not calibration bias (the subpixel-linearity white paper documents "pixel-locking" bias that averaging cannot remove).

### 1.2 The accepted pipeline (depth → B-rep)

1. **Capture**: librealsense2, 848x480, laser on, fixed exposure, low gain, decimation/temporal/hole-filling filters; record to `.bag`. Open3D can drive the camera directly and record a bag (since v0.12; requires synchronized color+depth). https://www.open3d.org/docs/latest/tutorial/sensor/realsense.html
2. **Pose**: frame-to-model ICP (KinectFusion), RGB-D odometry + pose graph (Choi–Zhou–Koltun, CVPR 2015, the basis of Open3D's reconstruction system), or a VIO/SLAM stack (RTAB-Map, OpenVINS, VINS-Fusion) when the IMU is used. The D435i IMU helps gravity alignment and fast-motion tracking; it does not by itself give drift-free pose. https://openaccess.thecvf.com/content_cvpr_2015/html/Choi_Robust_Reconstruction_of_2015_CVPR_paper.html ; https://answers.ros.org/question/376539/rtabmap-visual-odometry-getting-lost-too-often-using-d435i
3. **Volumetric fusion (TSDF)**: Open3D's tensor `VoxelBlockGrid.integrate` (block activation + voxel integration), then `extract_triangle_mesh` (marching cubes) or `extract_point_cloud`. https://www.open3d.org/docs/latest/tutorial/t_reconstruction_system/integration.html ; legacy 4-stage pipeline (make fragments → register fragments → refine registration → integrate scene) https://www.open3d.org/docs/latest/tutorial/reconstruction_system/index.html
   - Lineage: KinectFusion (GPU TSDF, small volume, drift, no loop closure) → ElasticFusion (surfels, deformation-graph loop closure; fails under fast motion) → BundleFusion (global pose optimisation over all frames; two high-end GPUs, stores all frames). https://arxiv.org/pdf/1604.01093 ; https://graphics.stanford.edu/projects/bundlefusion ; https://arxiv.org/pdf/1909.04250
4. **Mesh cleanup**: MeshLab (GPL) / CloudCompare (GPL) for cropping, outlier removal, Poisson/ball-pivot remeshing, decimation, hole filling.
5. **Primitive detection / segmentation**:
   - Schnabel, Wahl, Klein 2007 "Efficient RANSAC for Point-Cloud Shape Detection" — planes, spheres, cylinders, cones, tori; millions of points in under a minute. https://cg.cs.uni-bonn.de/backend/v1/files/publications/schnabel-2007-efficient.pdf
   - CGAL `Shape_detection` (Efficient RANSAC + deterministic region growing), same five primitives, custom shapes via `Shape_base`; key knobs `epsilon`, `cluster_epsilon`, `min_points`, `normal_threshold`. https://doc.cgal.org/latest/Shape_detection/
   - CloudCompare `qRansacSD` wraps Schnabel's library 1.1 unchanged; users report many false detections and hard-to-set parameters; "no tool in CC that can do the cleanup automagically". https://cloudcompare.org/doc/wiki/index.php/PluginRansacSD
   - PCL `SampleConsensusModel*` (`SACMODEL_NORMAL_PLANE`, `SACMODEL_CYLINDER`, sphere, cone…), with normal-distance weight and radius limits. https://pcl.readthedocs.io/projects/tutorials/en/latest/cylinder_segmentation.html
6. **Reverse engineering to B-rep**: fit constrained primitives (plane/cylinder/sphere/cone/torus), extract edges as surface–surface intersections, recover sketches (2D profiles on fitted planes), rebuild as extrude/revolve features, then deviation analysis against the scan.
   - Geomagic Design X (3D Systems): history-based CAD on scan data, "constrained prismatic feature fitting", auto-surfacing, Accuracy Analyzer, LiveTransfer of feature trees to SolidWorks/NX/Creo. (reseller/press pages) https://industrial-ia.com/products/3d-systems/geomagic-design-x/ ; https://www.metal-am.com/?p=76729
   - QuickSurface (KVS): extracts plane/cylinder/cone/sphere with relations (parallel, perpendicular, coincident, offset), splits mesh, auto-surfaces; ~US$4–6k. https://global.revopoint3d.com/QuickSurface ; https://develop3d.com/reviews/review-kvs-quicksurface-1-0-3d-3d-scanning-cad-reverse-engineering/
   - Consumer UX: Polycam (LiDAR/photogrammetry, "about ±½ inch on standard interior captures", mesh/point-cloud output, human-in-the-loop scan-to-CAD service), Scaniverse (free, on-device), RealityScan (Epic; photogrammetry, now with LiDAR). https://learn.poly.cam/hc/en-us/articles/50434745985556-How-Accurate-Are-Polycam-Scans ; https://aecmag.com/reality-capture-modelling/polycam-for-aec/ ; https://www.skyebrowse.com/news/posts/polycam-vs-scaniverse

### 1.3 Achievable accuracy for parts of 10 cm – 1 m

Combining the error law (∝ dist²), the measured 1–2 mm at 0.6–0.7 m, and the 0.3 m ideal minimum:

| Part size | Standoff needed (to frame it, 87° HFOV) | Per-frame depth noise | Fused-model realistic accuracy | Notes |
|---|---|---|---|---|
| 10 cm | 0.3 m (at Min-Z) | ~0.5–1 mm | **±1–2 mm** best case; features < 3–5 mm lost | Forum/arXiv evidence says D435 is the wrong tool; D405 (7–50 cm, ±2 % @ 50 cm, 7 cm Min-Z) is the RealSense answer. https://www.realsenseai.com/products/d405/ |
| 30 cm | 0.4–0.5 m | ~1–1.5 mm | **±1–3 mm** | Good for envelope, hole positions to ~2 mm, not hole diameters |
| 1 m | 1–1.5 m | ~10–20 mm per frame | **±5–10 mm** after multi-view fusion, worse at the far side | Comparable to Polycam's ±12 mm; fine for fixtures/stock |
| Room (3–5 m) | 1–3 m | 10–40 mm | **±2–5 cm**, drift-limited unless loop-closed | Use RTAB-Map/BundleFusion-class SLAM |

Fusion typically halves per-frame noise but cannot recover systematic calibration bias; primitive fitting over thousands of points (a plane or cylinder axis) can reach sub-mm *orientation/position* precision even when individual points are noisy — that is why "scan to primitives" works where "scan to freeform surface" does not.

### 1.4 Good for / not good for

Good for (with citations above):
- Room / fixture / stock-material scanning: envelopes, clearances, where the table and the vise are (cm-class).
- Referencing a physical object's envelope: bounding box, major planes, cylinder axes, hole patterns to ~2 mm.
- Hand / gesture input: Intel's SDK has no native skeleton tracking; Cubemos is dead; Nuitrack (commercial) or MediaPipe on the RGB stream + depth lookup are the current routes. https://support.realsenseai.com/hc/en-us/community/posts/39737691107731/comments/39737965542163 ; https://docs.derivative.ca/RealSense
- Live point clouds at 90 fps for interaction, collision volumes, "snap the model to the real part".

Not good for:
- Sub-millimetre part metrology or anything where a tolerance is tighter than ~1 mm: 2 % @ 2 m, ~1 mm RMS at 0.6 m, unmeasured bias, pixel-locking. Structured-light/laser scanners (Revopoint, Einscan, Artec) are 0.02–0.1 mm class.
- Small parts (< 10 cm) or fine features (< 3 mm): Min-Z 0.2–0.3 m means too few pixels; use D405.
- Shiny/black/transparent surfaces (stereo drop-outs); thin edges (edge bleeding).
- Drift-free room models without loop closure.

---

## Q2. CAD-kernel landscape for a from-scratch, Rust, no_std-friendly design app

### 2.1 B-rep kernels

| Kernel | Lang / licence | Booleans | Fillets | NURBS | STEP | Robustness notes | Source |
|---|---|---|---|---|---|---|---|
| **Parasolid** (Siemens) | C, commercial, per-seat royalty | Yes, with region control and auto-matching of nearly coincident regions (v26, v33) | Yes, incl. constant-width, face-face through capping face (v38) | Yes | Via Siemens/3rd-party | Tolerant modelling since ROMULUS lineage; "Convergent Modelling" mixes facets + classic B-rep | https://blogs.sw.siemens.com/plm-components/parasolid-v33-0-release-highlights/ ; https://blogs.sw.siemens.com/plm-components/whats-new-in-parasolid-v-38-0/ ; https://www.spatial.com/glossary/parasolid |
| **ACIS** (Spatial/Dassault) | C++, commercial | Yes | Yes | Yes | Yes | Tolerant modelling added in v5–8 "as an afterthought"; Spatial now also pushes CGM | https://www.3ds.com/newsroom/press-releases/spatial-announces-release-3d-acis-modeler-version-80 ; https://www.designnews.com/design-software/a-new-modeler-from-spatial- |
| Users of Parasolid | Onshape, SolidWorks, Solid Edge, NX, **Plasticity** | | | | | | https://en.wikipedia.org/wiki/Plasticity_(software) ; https://en.wikipedia.org/wiki/.x_t |
| **Open CASCADE (OCCT)** | C++, **LGPL-2.1 + OCCT-exception-1.0** (since 6.7.0); commercial licence available | Yes (BOP) | Yes | Yes | Yes (STEP/IGES, best open STEP) | Tracker full of fillet/boolean failures: fillet `IsDone` true but wrong at one radius, "no suitable edges", boolean on STEP input fails, tiny geometry after intersection breaks fillets; 8.0.1 notes still list boolean-section hangs and shape-healing crashes; mesher fails on 1.56 % of ABC / 8.82 % of Fusion models (2026 paper) | https://spdx.org/licenses/OCCT-exception-1.0.html ; https://dev.opencascade.org/content/open-cascade-technology-changes-its-license-gnu-lgpl ; https://tracker.dev.opencascade.org/view.php?id=26243 ; https://tracker.dev.opencascade.org/view.php?id=28596 ; https://tracker.freecad.org/view.php?id=1958 ; https://newreleases.io/project/github/Open-Cascade-SAS/OCCT/release/V8.0.1 ; https://arxiv.org/abs/2604.02141 |
| **opencascade-rs** (bschwind) | Rust cxx bindings, LGPL-2.1, "major work in progress", `occt-sys` builds OCCT 7.8.1 | via OCCT | via OCCT | via OCCT | via OCCT | Inherits OCCT; last push May 2026; 56 open issues | https://github.com/bschwind/opencascade-rs ; https://docs.rs/crate/opencascade-sys/0.3.0/source/Cargo.toml |
| **truck** (RICOS) | Rust, **Apache-2.0**; crates base/geotrait/geometry/topology/polymesh/meshalgo/modeling/shapeops/platform/rendimpl/js (+ stepio, drafting, assembly in tree) | `truck-shapeops` (solid booleans) | not documented | B-spline/NURBS core | `truck-stepio` (undocumented) | ~2.9k commits, 1.7k stars, tutorials at v0.6.x; fork `monstertruck` adds fillet API | https://github.com/ricosjp/truck ; https://docs.rs/monstertruck |
| **Fornjot** (Hanno Braun) | Rust, 0BSD, **shut down 19 Jun 2026**, archived | no | no | no (approximated geometry experiments) | no (3MF export) | README: "goals were never reached"; "kept discovering … why b-rep kernels are considered hard"; lessons: prototype early, be a tool not a building block | https://github.com/hannobraun/fornjot ; https://archive.hannobraun.com/fornjot/blog/shutting-down-fornjot/ |
| **nacre** (elgar328) | Rust, MIT/Apache-2.0, pure Rust, WASM | planar solids incl. coplanar contact; quadrics in progress (cylinder vs. axis-aligned planes) | no | general NURBS = "research track" | shape-only STEP export | **Exact**: vertices defined by meeting surfaces, coordinates are a cache with proven bound, no global tolerance, refuses with a named reason instead of returning a wrong solid | https://github.com/elgar328/nacre |
| **next.BREP `BREP_kernel`** (mmiscool) | Rust, **Autodrop3d licence (not OSI)**, v0.11.0 (2026-10-28) | regularized union/intersect/subtract with exact-volume verification | `fillet_edge(s)`, chamfer | exact rational NURBS eval | AP242 export + import | Announced Oct 2026, no robustness statements; depends on wasm-bindgen, spade (not no_std) | https://docs.rs/crate/BREP_kernel/latest ; https://users.rust-lang.org/t/next-brep-a-rust-b-rep-geometry-kernel-and-parametric-cad-app-for-browser-and-desktop/142801 |
| **brepkit** (andymai) | Rust, licence inconsistent (GitHub: MIT/Apache-2.0; docs.rs: AGPL-3.0 or commercial) | yes | fillet, chamfer, shell, offset, sweep, loft | yes | STEP, IGES(exp), STL, 3MF, OBJ, PLY, glTF | no `unsafe`, no `unwrap`/`panic` policy | https://github.com/andymai/brepkit ; https://docs.rs/crate/brepkit-render/latest |
| **catallax** (BohnBawerick) | Rust, MIT, alpha, 0 stars | plane/cylinder/sphere pairs + some cone/torus; exact arithmetic with certified enclosures | some exact fillets/chamfers | no | AP242 export with deviation report | **Hybrid**: one "recipe" evaluated by an exact B-rep kernel and an SDF kernel that cross-check; deterministic budgets; refuse-don't-guess; much code AI-written | https://github.com/BohnBawerick/catallax |
| solidcore (SMLib port), cad-kernel (bmander, zero-dep, bit-identical native/WASM, 29/33 brep-bench), zenith, curvo (NURBS), geomcore | Rust, assorted | partial | | | | all pre-alpha | https://github.com/bherbruck/solidcore/wiki ; https://github.com/bmander/cad-kernel ; https://crates.io/keywords/nurbs |
| **Zoo / KittyCAD engine** | closed, hosted (video-streamed over WebSocket); app + KCL open source | GPU-accelerated CSG | B-spline fillets with constraints | "exact NURBS and analytic surfaces", minimal primitive set | yes | Patented GPU surface–surface intersection by dense sampling + LOD refinement; explicitly **rejects implicit** modelling (cubic scaling, GPU memory, cost of extracting clean B-rep) | https://zoo.dev/research/zoo-cad-engine-overview ; https://zoo.dev/blog/zoo-design-studio-v1 ; https://github.com/KittyCAD/modeling-app |

### 2.2 Implicit / SDF / field-based

| System | Licence | What it does | Caveats | Source |
|---|---|---|---|---|
| **libfive** (Keeter) | core + stdlib + Python: **MPL-2.0**; Studio/Guile: GPL-2+ | f-rep tree, interval arithmetic, Manifold Dual Contouring mesher (QEF sharp features, manifold by construction, may self-intersect); newer simplicial-partition mesher | needs interval arithmetic on the field, not just a distance function | https://github.com/libfive/libfive ; https://mattkeeter.com/blog/2026-07-03-meshing ; https://groups.google.com/g/curv/c/rCSaFlgrU |
| **fidget** (Keeter, Rust) | MPL-2.0 | JIT (x86_64/aarch64) + interpreter (WASM) evaluation of implicit tapes; MDC meshing; "expect to fork it" | meshing less battle-tested than libfive; JIT needs std | https://www.lib.rs/crates/fidget ; https://www.mattkeeter.com/research/mpr/keeter_mpr20.pdf |
| **nTop** | commercial | implicit kernel; booleans/offsets/rounds "never fail" (min/max of fields); field-driven design (lattices, topology optimisation) | heavy compute at high resolution; non-feature-tree UX; ex-CTO now builds open implicit tooling (GCL) | https://www.ntop.com/resources/blog/implicit-modeling-for-mechanical-design/ ; https://breakingthebottleneck.substack.com/p/friday-5-blake-courter-co-founder |
| **ImplicitCAD** (Haskell) | AGPL-3.0 | SDF CSG incl. rounded/bevelled ops, shells; STL/OBJ/SVG/DXF | "very much a work in progress", feature list dated 2020 | https://github.com/Haskell-Things/ImplicitCAD |
| **csgrs** (Rust) | MIT | BSP-tree mesh CSG (CSG.js port) + 2D via `geo`; deps nalgebra, geo, robust; no manifold guarantee; no_std is a roadmap item | 0.x, many TODOs | https://docs.rs/crate/csgrs/latest |
| **manifold** (Lalish) | Apache-2.0 | guaranteed-manifold mesh booleans; used by OpenSCAD, Blender, Godot, trimesh, BRL-CAD; input must be manifold; prefer 3MF/glTF over STL | serial triangulation; WASM serial-only | https://github.com/elalish/manifold |
| **manifold-rust** | Apache-2.0 | pure-Rust port validated boolean-by-boolean against the C++; adds "robust" engine for non-manifold input; C FFI, WASM | 0.13.x | https://docs.rs/crate/manifold-rust/0.13.1 |
| **CGAL Nef_3** | GPL (CGAL) | exact booleans on Nef polyhedra with exact-predicates/exact-constructions kernel; handles all degeneracies | designed for correctness over speed; a BSP system reports 16–28x faster | https://doc.cgal.org/latest/Nef_3/group__PkgNef3Ref.html ; https://www.academia.edu/89052468/Fast_Exact_Linear_Booleans |
| **Plasticity** | commercial app | chose **Parasolid** (same kernel as NX) rather than an implicit or open kernel | | https://en.wikipedia.org/wiki/Plasticity_(software) |

### 2.3 Constraint solvers

| Solver | Licence | Approach | Source |
|---|---|---|---|
| **planegcs** (FreeCAD Sketcher GCS) | LGPL-2.1+ | numerical (DogLeg/LM/BFGS), extracted to Python/WASM packages; defaults 1e-10 tolerance, 100 iterations | https://github.com/Salusoft89/planegcs ; https://pypi.org/project/planegcs/ |
| **SolveSpace / libslvs** | GPL-3.0+ (Rust `slvs` crate GPL-3.0, last release 2023) | constraints as symbolic equations, solved by modified Newton with forward substitution where possible; Jacobians as expression trees | https://solvespace.com/tech.pl ; https://lib.rs/crates/slvs |
| **D-Cubed 2D DCM** (Siemens) | commercial | simultaneous solve, order-independent, allows cyclic constraints; licensed by T-FLEX and others | https://www.siemens.com/en-us/products/plm-components/d-cubed/2d-dcm/ ; https://blogs.sw.siemens.com/plm-components/?p=9 |
| **Onshape** | proprietary, in-house ("our solver"), runs in the cloud | solves constraints + dimensions together, "move as little as possible", no sketch history, heavy use of initial-guess positions; users call it brittle | https://www.onshape.com/en/blog/under-the-hood-onshape-sketches ; https://forum.onshape.com/discussion/comment/124515 |

### 2.4 Tessellation and rendering of B-rep

- OCCT `BRepMesh` is parameter-sensitive (linear/angular deflection), misses features, fails on 1.56 % of ABC and 8.82 % of Fusion-360 models; "Topology-First B-Rep Meshing" (Zhou, Zint, Panozzo, Schneider et al., arXiv 2604.02141) makes topology an invariant and degrades only geometry as tolerance grows — the right spec for a new kernel's mesher. https://arxiv.org/abs/2604.02141
- truck tessellates through `truck-meshalgo` and renders with wgpu (`truck-platform`/`truck-rendimpl`); Zoo renders server-side and streams video; next.BREP renders with wgpu (WebGPU/WebGL2); catallax ships "display meshes with proven chord error".
- Implicit: libfive/fidget mesh with Manifold Dual Contouring or render directly on GPU (Keeter MPR, SIGGRAPH 2020).

### 2.5 Licensing summary

| Licence | Projects |
|---|---|
| Commercial | Parasolid, ACIS, D-Cubed, nTop, Zoo engine, Geomagic, QuickSurface, BREP_kernel (Autodrop3d licence) |
| LGPL-2.1 (+exception) | OCCT, opencascade-rs, planegcs |
| GPL / AGPL | SolveSpace+libslvs, CGAL (Nef, Shape_detection), MeshLab, CloudCompare, ImplicitCAD (AGPL), brepkit per docs.rs (AGPL/commercial) |
| MPL-2.0 | libfive core, fidget |
| Apache-2.0 | truck, librealsense, manifold, manifold-rust, Open3D (MIT actually), PCL (BSD) |
| MIT / MIT-or-Apache | nacre, catallax, csgrs, kurbo, nalgebra, robust, geometry-predicates |
| 0BSD | Fornjot (archived) |

### 2.6 no_std-relevant Rust crates

- `robust` 1.2.0 (georust): Shewchuk adaptive predicates, `no_std` feature. https://docs.rs/crate/robust/latest/features
- `geometry-predicates` 0.3: safe port, no_std, does not handle exponent overflow. https://www.lib.rs/crates/geometry-predicates
- `kurbo` (linebender): 2D Bézier/curve toolkit, no_std via libm, CI on x86_64-unknown-none. https://deepwiki.com/linebender/kurbo/4.4-embedded-and-no_std-support
- `nalgebra`: no_std with `default-features = false` (static sizes only). https://git.m-labs.hk/M-Labs/nalgebra/commit/ca093fad298b5a040e54c347e137fc0303581faa
- `manifold-rust`, `csgrs`, `truck`, `BREP_kernel`, `fidget` (JIT): std today.

---

## 3. Recommendation

### 3.1 Representation to start with

| Option | Verdict | Why |
|---|---|---|
| Pure B-rep with NURBS from day one | **No** | Six-year Fornjot post-mortem; OCCT's tracker; Zoo needed a patented GPU SSI; nacre and catallax both label general NURBS intersection a "research track". Robustness is the whole problem and it is not solvable incrementally by a small team. |
| Pure implicit / SDF | **No (as the master)** | Booleans/offsets never fail, but Zoo's critique stands: cubic scaling, no edges/faces to dimension, expensive and lossy extraction to a B-rep that STEP/drawings/CAM need. Good for lattices and for *checking*. |
| **Hybrid: exact-arithmetic B-rep over a small analytic surface set, with a parallel SDF/field evaluator and a feature "recipe" as the master** | **Yes** | Matches nacre's "exact geometry is the truth, approximations are a cache", catallax's two-kernel cross-check, and Zoo's "minimal set of geometric primitives". Starts with planes/cylinders/spheres/cones/tori where closed-form intersections exist, refuses what it cannot prove, adds NURBS only as a bounded extension. Honest (refuse, do not guess), testable (SDF sampling and exact-volume checks against the B-rep; byte-identical results across threads), owned (no OCCT). |

### 3.2 Rust crates worth reading or depending on

| Crate | Use | Licence | Read or depend? |
|---|---|---|---|
| `robust` / `geometry-predicates` | orient2d/3d, incircle/insphere | MIT/Apache | **depend** (no_std) |
| `nalgebra` (default-features off) or hand-rolled `[f64;3]` | linear algebra | Apache/MIT | depend (no_std) |
| `kurbo` | 2D curves, arc-length, offsets for sketches | Apache/MIT | depend (no_std) |
| `nacre` | exact B-rep design, refusal semantics, vertex-as-surface-meet | MIT/Apache | **read**, possibly depend later |
| `catallax` | hybrid B-rep + SDF cross-check, deterministic budgets | MIT | read |
| `truck` | NURBS eval, topology data structures, shapeops approach, wgpu tessellation | Apache-2.0 | read; borrow `truck-geometry` B-spline code |
| `manifold-rust` | robust mesh booleans for the display/export path and for stock/scan meshes | Apache-2.0 | depend (std side, host tools) |
| `fidget` | implicit tape evaluation, interval arithmetic, MDC meshing | MPL-2.0 | read; depend on host for lattices/checks |
| `cad-kernel` (bmander) | zero-dep, bit-identical native/WASM discipline; brep-bench test corpus | — | read the bench |
| `BREP_kernel`, `brepkit` | feature coverage checklists (fillet/offset/shell/STEP AP242) | non-OSI / AGPL | read only, do not depend |
| `opencascade-rs` | only for an offline STEP import/export bridge in a host tool | LGPL | avoid in the core |

### 3.3 First 10 geometry capabilities, in order

1. **Numeric foundation**: f64 + exact fallback (rational or adaptive-precision), `robust` predicates, interval arithmetic type, named `Refusal` error type; byte-determinism tests across thread counts.
2. **Topology data model**: half-edge B-rep (vertex/edge/loop/face/shell/solid) with Euler-operator construction and a validator (closed, oriented, manifold) — validator first, so every later op is checked.
3. **Analytic surfaces and curves**: plane, cylinder, sphere, cone, torus; line, circle, ellipse; exact point/normal/curvature evaluation; closed-form surface–surface intersections for plane–plane, plane–quadric, cylinder–cylinder (the nacre M6 set).
4. **2D sketch + constraint solver**: lines/arcs/circles, coincident/parallel/perpendicular/tangent/equal/dimension constraints; Newton/LM on an expression graph (SolveSpace model), with degree-of-freedom reporting and "move as little as possible" (Onshape behaviour).
5. **Extrude and revolve of a closed sketch** into a valid solid; mass properties (volume, centroid, inertia) with exact-volume cross-checks.
6. **Planar-solid booleans** (union/subtract/intersect) with coplanar-contact handling and regularisation; verified by volume identities and by SDF sampling of the same recipe (catallax pattern).
7. **Tessellation with topological guarantee** and proven chord error (topology-first meshing spec); display mesh + STL/3MF/glTF export; this is also how scans and stock meshes meet the model via `manifold-rust`.
8. **Quadric booleans**: cylinder/sphere/cone against planes and each other (holes, bosses, counterbores), the 80 % case for machined parts.
9. **Fillets and chamfers on planar and cylindrical edges** (rolling-ball with analytic blends only; refuse the rest), then offset/shell/thicken in the same analytic domain.
10. **STEP AP242 export (then import)** of analytic B-rep with a deviation report, and a "scan reference" capability: fit plane/cylinder/sphere primitives (Schnabel-style RANSAC) to a D435i point cloud and expose them as datum geometry the sketch solver can constrain to.

Defer: general NURBS–NURBS intersection, sweeps/lofts, draft, sheet metal, assemblies — every open kernel surveyed lists these as unfinished or research.

---

## Source index (all URLs cited above, grouped)

RealSense: https://www.realsenseai.com/products/depth-camera-d435i/ · https://www.realsenseai.com/products/d405/ · https://dev.realsenseai.com/docs/tuning-depth-cameras-for-best-performance · https://dev.realsenseai.com/docs/white-paper-subpixel-linearity-improvement-for-intel-realsense-depth-cameras/ · https://support.realsenseai.com/hc/en-us/articles/360059129453-Depth-accuracy-for-Intel-RealSense-D400-Series-Cameras · https://wildboar-dev.github.io/RealSense-Accuracy/ · https://jurc192.github.io/4DFS_updates/accuracy_report · https://arxiv.org/pdf/2408.01615 · https://arxiv.org/pdf/2501.18315 · https://github.com/IntelRealSense/librealsense/tree/master/examples
Reconstruction: https://www.open3d.org/docs/latest/tutorial/t_reconstruction_system/integration.html · https://www.open3d.org/docs/latest/tutorial/reconstruction_system/index.html · https://www.open3d.org/docs/latest/tutorial/sensor/realsense.html · https://openaccess.thecvf.com/content_cvpr_2015/html/Choi_Robust_Reconstruction_of_2015_CVPR_paper.html · https://arxiv.org/pdf/1604.01093 · https://arxiv.org/pdf/1909.04250
Primitives / RE: https://cg.cs.uni-bonn.de/backend/v1/files/publications/schnabel-2007-efficient.pdf · https://doc.cgal.org/latest/Shape_detection/ · https://cloudcompare.org/doc/wiki/index.php/PluginRansacSD · https://pcl.readthedocs.io/projects/tutorials/en/latest/cylinder_segmentation.html · https://global.revopoint3d.com/QuickSurface · https://industrial-ia.com/products/3d-systems/geomagic-design-x/ · https://learn.poly.cam/hc/en-us/articles/50434745985556-How-Accurate-Are-Polycam-Scans
Kernels: https://github.com/ricosjp/truck · https://github.com/hannobraun/fornjot · https://archive.hannobraun.com/fornjot/blog/shutting-down-fornjot/ · https://github.com/elgar328/nacre · https://github.com/BohnBawerick/catallax · https://docs.rs/crate/BREP_kernel/latest · https://github.com/andymai/brepkit · https://github.com/bschwind/opencascade-rs · https://spdx.org/licenses/OCCT-exception-1.0.html · https://tracker.dev.opencascade.org/view.php?id=28596 · https://arxiv.org/abs/2604.02141 · https://zoo.dev/research/zoo-cad-engine-overview · https://en.wikipedia.org/wiki/Plasticity_(software) · https://blogs.sw.siemens.com/plm-components/whats-new-in-parasolid-v-38-0/
Implicit / mesh: https://github.com/libfive/libfive · https://www.lib.rs/crates/fidget · https://mattkeeter.com/blog/2026-07-03-meshing · https://github.com/elalish/manifold · https://docs.rs/crate/manifold-rust/0.13.1 · https://docs.rs/crate/csgrs/latest · https://github.com/Haskell-Things/ImplicitCAD · https://doc.cgal.org/latest/Nef_3/group__PkgNef3Ref.html · https://www.ntop.com/resources/blog/implicit-modeling-for-mechanical-design/
Solvers: https://github.com/Salusoft89/planegcs · https://solvespace.com/tech.pl · https://lib.rs/crates/slvs · https://www.siemens.com/en-us/products/plm-components/d-cubed/2d-dcm/ · https://www.onshape.com/en/blog/under-the-hood-onshape-sketches
no_std crates: https://docs.rs/crate/robust/latest/features · https://www.lib.rs/crates/geometry-predicates · https://deepwiki.com/linebender/kurbo/4.4-embedded-and-no_std-support
