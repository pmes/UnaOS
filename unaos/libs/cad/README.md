# unaos/libs/cad — Vug's CAD cores

The geometry kernel UnaOS owns, as `no_std` + `alloc` cores in the midden_core shape (BP-handlers-4): the host
faces, the ring-3 `VUG.ELF` and the kernel link the same crates. Program and research: `docs/dev/USERLAND/VUG.md`;
invariants: `docs/dev/blueprint/vug.md` (f64 geometry, one `Tol`, pure cores, the design holds its Make).

| crate | owns | ledger |
|---|---|---|
| `geom_core` | f64 vectors, matrices, transforms, `Tol`, AABB, exact orientation predicates that refuse by name (`Predicate::Uncertain`) rather than return a wrong sign, closed-form intersections | SR464 |
| `sketch_core` | 2D entities, constraints, the solver, DOF/redundancy, the drag solve, profiles (VUGSKETCH: one weighted min-norm LM for solve and drag; redundancy named by the ordered rank, conflicts refused by name) | SR465 |
| `mesh_core` | the triangle mesh (`Mesh`, `validate`), half-edge adjacency (`HalfEdge`), angle-weighted normals, BVH pick (`Bvh`), manifold check, weld, `to_f32_buffers` (the one render conversion), `stl` / `obj` / `ply` read and write | SR466 |
| `make_core` | Machine, Tool, Stock, fixtures, Op, the `Program` IR, G-code dialects, `review` for Helm — built: the planner (kinematic pass, refusals by name, grbl / Marlin / LinuxCNC), geom shim until SR464 folds | SR467 |
| `brep_core` | owed (VUG-BREP-1, LEDGER SR468, in flight) | — |
| `scan_core` | TSDF fusion of probe_core depth frames, marching cubes with per-vertex confidence, plane / cylinder RANSAC with residuals, the synthetic depth fixture | SR469 |
