# unaos/libs/cad — Vug's CAD cores

The geometry kernel UnaOS owns, as `no_std` + `alloc` cores in the midden_core shape (BP-handlers-4): the host
faces, the ring-3 `VUG.ELF` and the kernel link the same crates. Program and research: `docs/dev/USERLAND/VUG.md`;
invariants: `docs/dev/blueprint/vug.md` (f64 geometry, one `Tol`, pure cores, the design holds its Make).

| crate | owns | ledger |
|---|---|---|
| `geom_core` | f64 vectors, matrices, transforms, `Tol`, AABB, exact orientation predicates that refuse by name (`Predicate::Uncertain`) rather than return a wrong sign, closed-form intersections | SR464 |
| `sketch_core` | 2D entities, constraints, the solver, DOF/redundancy, the drag solve, profiles (VUGSKETCH: one weighted min-norm LM for solve and drag; redundancy named by the ordered rank, conflicts refused by name) | SR465 |
| `mesh_core` | the triangle mesh (`Mesh`, `validate`), half-edge adjacency (`HalfEdge`), angle-weighted normals, BVH pick (`Bvh`), manifold check, weld, `to_f32_buffers` (the one render conversion), `stl` / `obj` / `ply` read and write; `threemf` 3MF read / write over `unaos/libs/fs/zip_core` (components composed, units to mm, refusals by name) | SR466, SR486 |
| `make_core` | Machine, Tool, Stock, fixtures, Op, the `Program` IR, G-code dialects, `review` for Helm — built: the planner (kinematic pass, refusals by name, grbl / Marlin / LinuxCNC), geom_core by path (SR473); 2.5D pocketing and facing (`offset::rings`, SR479); FDM slicing of a triangle soup to a Marlin program (`slice`, SR483); material-removal simulation on a dense signed-distance stock (`sim`, SR496) | SR467, SR479, SR483, SR496 |
| `brep_core` | B-rep topology with persistent Ids, analytic surfaces, validate, primitives, extrude / revolve, closed-form mass, chord-bounded tessellation, booleans (plane / cylinder faces) and point classification; geom_core / sketch_core / mesh_core by path (no shims) | SR468, SR481, SR492 |
| `step_core` | STEP: the ISO 10303-21 clear-text tokenizer and instance table, `read` of AP214 advanced B-reps into brep_core (unit scaled, the uv rectangle of each curved face rebuilt, trimmed curved faces refused) and a deterministic `write`; every refusal named by line / column or by entity and instance | SR482 |
| `scan_core` | TSDF fusion of probe_core depth frames, marching cubes with per-vertex confidence, plane / cylinder RANSAC with residuals, the synthetic depth fixture | SR469 |
| `doc_core` | the design document: text, one statement per line (sketches, constraints, features, the Make, scan references), `read` / `write` (byte-deterministic) / `replay` through the cores with each failing statement named by its line | SR480 |

Every core takes `geom_core` by path for its geometry (one `Tol`, BP-vug-2), and `scan_core` also takes `mesh_core`; no core carries a private stand-in (VUGSHIMSWAP, LEDGER SR473).

The headless face over `scan_core` + `mesh_core` is `tools/scan` (VUGSCANTOOL, LEDGER SR471): a `.prb` recording (`probe_core::recording`), an rs-convert CSV or a realsense-viewer `.raw` in, a PLY and a residual report out.
