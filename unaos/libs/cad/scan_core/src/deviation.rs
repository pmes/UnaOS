// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Vug (CODEX §2, 3D), seam shared-core — the made part measured against its model: a scan registered to
//! the design's tessellation and its signed deviation per vertex, with the statistics and the faces ranked.
//!
//! VUGSCANDEVIATION (LEDGER SR510); VUG.md §3 principle 9 (measure the result). BP-vug-5: the deviation is a printed
//! residual of a measurement against the design, never a change to either.
//!
//! 1. The design is a mesh with the face index of each triangle ([`Design`]); the caller (tools/scan, vug-view) replays
//!    the document and hands [`design_of`] its solids, which tessellates them at [`DESIGN_CHORD_MM`] and names every
//!    face (VUGFACEDEVIATION, SR525: the ONE builder, lifted from tools/scan) — this core takes brep_core for that, and
//!    still no document (no doc_core).
//! 2. [`align`]: point-to-plane ICP (Chen & Medioni 1992; Low 2004's linearisation) of the scan to the design over
//!    mesh_core's `Bvh::closest`: each scan vertex (a deterministic stride subsample of at most
//!    [`AlignParams::max_points`]) carried by the estimate is paired with its nearest design point when it is within
//!    the level's gate AND its TSDF normal (toward free space) agrees with the design triangle's outward normal
//!    within [`AlignParams::normal_cos`] — the floor the part stands on, coplanar with its bottom face but facing the
//!    other way, pairs with nothing. The rows `[q × n, n]`, the 6x6 solve and the left step are pose.rs's own
//!    (`solve6`, `left_step`, SR478). Three levels, the gates k·σ·[8, 4, 2]; a level ends when the step is inside
//!    [`AlignParams::stop`]. Refused by name: too few pairs or a final rms above k·σ ([`DeviationError::NoAlignment`]),
//!    pairs that do not pin six directions ([`DeviationError::Unconstrained`]).
//! 3. [`deviations`]: every vertex at the aligned pose, its nearest design point within the last gate whose normal
//!    agrees: the signed distance, positive where the scan stands proud of the design (material outside), negative
//!    where it falls short; `None` for a vertex that is not the part (the fixture, the floor, clutter).
//! 4. [`Stats`] (rms, max, the 95th percentile, the fraction inside the band) and one [`FaceReport`] per design face
//!    (its signed mean is a plane's offset or a cylinder's radius error), by rms, worst first: a systematic offset over a
//!    whole face outranks one edge-rounding or noise vertex (the signed max is still each face's).
//! 5. [`shade`]: the band classifier, and [`colour`] the ONE ramp both faces colour from (palette_core's status rows;
//!    VUGFACEDEVIATION, SR525, lifted from tools/scan).
//!
//! The start is the caller's (`init`): a world-posed scan starts at the identity, a scan with no starting pose at
//! [`measure_coarse`]'s principal axes (VUGSCANDEVIATIONCOARSE SR615), a near-single view refused there at
//! [`coarse_view`]'s yaw sweep with its translation re-solved (VUGSCANDEVIATIONCOARSEVIEW SR626).

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use mesh_core::bvh::Bvh;

use crate::ScanMesh;
use crate::geom::{Tol, Transform, Vec3};
use crate::mesh::Mesh;
use crate::pose::{left_step, solve6};

/// The design surface: a mesh (CCW outward triangles, mm) and the face index of each triangle (`face_of[t] < faces`).
#[derive(Clone, Debug, PartialEq)]
pub struct Design {
    pub mesh: Mesh,
    pub face_of: Vec<u32>,
    pub faces: u32,
}

impl Design {
    /// Refuses a face index list that is not one per triangle, or an index past `faces`.
    pub fn new(mesh: Mesh, face_of: Vec<u32>, faces: u32) -> Result<Design, DeviationError> {
        if face_of.len() != mesh.triangles.len() {
            return Err(DeviationError::FaceCount { triangles: mesh.triangles.len(), face_of: face_of.len() });
        }
        if let Some((tri, &face)) = face_of.iter().enumerate().find(|(_, f)| **f >= faces) {
            return Err(DeviationError::FaceIndex { tri: tri as u32, face, faces });
        }
        Ok(Design { mesh, face_of, faces })
    }
}

/// The registration's rules.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlignParams {
    /// The scan's noise at its working distance (mm): `fixture::Noise::D435I`, 1.5 mm at 0.6 m.
    pub sigma_mm: f64,
    /// The acceptance multiple (doc_core's `K_SIGMA` is the same 3).
    pub k_sigma: f64,
    /// The least cosine between a scan normal and the paired design normal.
    pub normal_cos: f64,
    /// Iterations a level at most.
    pub iters: u32,
    /// The ICP's stride subsample bound (the deviations read every vertex).
    pub max_points: usize,
    /// The fewest final pairs an alignment stands on.
    pub min_pairs: u32,
    /// A level ends when the step's rotation (rad) and translation (mm) are both inside this.
    pub stop: Tol,
}

impl AlignParams {
    pub const DEFAULT: AlignParams = AlignParams {
        sigma_mm: 1.5,
        k_sigma: 3.0,
        normal_cos: 0.5,
        iters: 20,
        max_points: 8192,
        min_pairs: 64,
        stop: Tol { lin: 1e-3, ang: 1e-6 },
    };

    /// The pairing gate of level `l` (0 coarse .. 2 fine): k·σ·[8, 4, 2] mm.
    pub fn gate(&self, l: usize) -> f64 {
        self.k_sigma * self.sigma_mm * [8.0, 4.0, 2.0][l.min(2)]
    }
}

impl Default for AlignParams {
    fn default() -> Self {
        AlignParams::DEFAULT
    }
}

/// An accepted registration: the scan → design pose and what its final pairing measured at the last gate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aligned {
    pub pose: Transform,
    pub rms_mm: f64,
    pub pairs: u32,
    pub iters: u32,
}

/// Why a design or a registration was refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeviationError {
    /// The scan has no vertex.
    EmptyScan,
    /// The design has no triangle.
    EmptyDesign,
    /// The face index list is not one per triangle.
    FaceCount { triangles: usize, face_of: usize },
    /// A triangle names a face past the design's count.
    FaceIndex { tri: u32, face: u32, faces: u32 },
    /// Too few pairs, or the final residual above k·σ: the scan does not lie on the design from this start.
    NoAlignment { rms_mm: f64, pairs: u32 },
    /// The pairs of this level do not constrain all six directions (one plane, one cylinder alone).
    Unconstrained { level: u32 },
}

/// The deviation over every matched vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub matched: u32,
    pub unmatched: u32,
    pub rms_mm: f64,
    /// The largest |d|.
    pub max_mm: f64,
    /// The 95th percentile of |d| (nearest rank).
    pub p95_mm: f64,
    /// The fraction of the matched vertices with |d| <= the band.
    pub inside: f64,
    pub band_mm: f64,
}

/// One design face's deviation: its matched vertices, the signed mean, the rms and the signed worst.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceReport {
    pub face: u32,
    pub n: u32,
    pub mean_mm: f64,
    pub rms_mm: f64,
    pub max_mm: f64,
}

/// The measured part: the registration, the signed deviation of each scan vertex (`None`: not the part), the
/// statistics, and every design face by rms, worst first (faces no vertex matched last, `n` 0).
#[derive(Clone, Debug, PartialEq)]
pub struct Deviation {
    pub aligned: Aligned,
    pub per_vertex: Vec<Option<f64>>,
    /// The design face each scan vertex paired with (an index into `faces`); None where per_vertex is None. VUGFACEDEVIATIONPICK (SR543) + VUGSCANGDT (SR542).
    pub face_of_vertex: Vec<Option<u32>>,
    pub stats: Stats,
    pub faces: Vec<FaceReport>,
}

/// Where a deviation falls against the band: inside it, beyond it short (Under) or proud (Over) by the fraction
/// `(|d| − band) / (2·band)` clamped to 1, or off the design.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shade {
    Inside,
    Under(f64),
    Over(f64),
    Off,
}

/// The band classifier a face colours from (VUG-FACE-DEVIATION reads the same).
pub fn shade(d: Option<f64>, band_mm: f64) -> Shade {
    let Some(d) = d else { return Shade::Off };
    if !d.is_finite() {
        return Shade::Off;
    }
    if d.abs() <= band_mm {
        return Shade::Inside;
    }
    let k = if band_mm > 0.0 { ((d.abs() - band_mm) / (2.0 * band_mm)).min(1.0) } else { 1.0 };
    if d < 0.0 { Shade::Under(k) } else { Shade::Over(k) }
}

/// The nearest design point of `q` within `gate` whose triangle's outward normal agrees with the scan normal `nq`:
/// the point and that normal.
fn pair(design: &Design, bvh: &Bvh, normals: &[Vec3], q: Vec3, nq: Vec3, gate: f64, cos: f64, tol: Tol) -> Option<(Vec3, Vec3)> {
    let near = bvh.closest(&design.mesh, q, gate, tol)?;
    let n = *normals.get(near.tri as usize)?;
    if n.mag_sq() == 0.0 || nq.dot(n) < cos {
        return None;
    }
    Some((near.point, n))
}

/// Register `scan` to `design` from `init` (scan → design), over `bvh` (built from `design.mesh`).
pub fn align(scan: &ScanMesh, design: &Design, bvh: &Bvh, init: Transform, p: &AlignParams, tol: Tol) -> Result<Aligned, DeviationError> {
    let nv = scan.mesh.positions.len();
    if nv == 0 {
        return Err(DeviationError::EmptyScan);
    }
    if design.mesh.triangles.is_empty() {
        return Err(DeviationError::EmptyDesign);
    }
    let normals = design.mesh.normals_face(tol);
    let stride = nv.div_ceil(p.max_points.max(1)).max(1);
    let sample: Vec<usize> = (0..nv).step_by(stride).collect();
    let scan_n = |i: usize| scan.normals.get(i).copied().unwrap_or(Vec3::ZERO);
    let mut pose = init;
    let mut iters = 0u32;
    for level in 0..3usize {
        let gate = p.gate(level);
        for _ in 0..p.iters {
            let mut ata = [[0.0f64; 6]; 6];
            let mut atb = [0.0f64; 6];
            let (mut pairs, mut sse) = (0u32, 0.0f64);
            for &i in &sample {
                let q = pose.apply_point(scan.mesh.positions[i]);
                let nq = pose.apply_vector(scan_n(i));
                let Some((c, n)) = pair(design, bvh, &normals, q, nq, gate, p.normal_cos, tol) else { continue };
                let r = n.dot(q - c);
                let jw = q.cross(n);
                let j = [jw.x, jw.y, jw.z, n.x, n.y, n.z];
                for a in 0..6 {
                    for b in 0..6 {
                        ata[a][b] += j[a] * j[b];
                    }
                    atb[a] -= j[a] * r;
                }
                pairs += 1;
                sse += r * r;
            }
            iters += 1;
            if pairs < p.min_pairs {
                let rms_mm = if pairs > 0 { libm::sqrt(sse / pairs as f64) } else { f64::INFINITY };
                return Err(DeviationError::NoAlignment { rms_mm, pairs });
            }
            let Some(x) = solve6(ata, atb) else {
                return Err(DeviationError::Unconstrained { level: level as u32 });
            };
            let (dw, dt) = (Vec3::new(x[0], x[1], x[2]), Vec3::new(x[3], x[4], x[5]));
            pose = left_step(&pose, dw, dt);
            if dw.mag() <= p.stop.ang.max(tol.ang) && dt.mag() <= p.stop.lin.max(tol.lin) {
                break;
            }
        }
    }
    // The final pairing at the last gate, at the pose the steps ended on (point-to-point distance: what the
    // deviation will read).
    let gate = p.gate(2);
    let (mut pairs, mut sse) = (0u32, 0.0f64);
    for &i in &sample {
        let q = pose.apply_point(scan.mesh.positions[i]);
        let nq = pose.apply_vector(scan_n(i));
        if let Some((c, _)) = pair(design, bvh, &normals, q, nq, gate, p.normal_cos, tol) {
            pairs += 1;
            sse += (q - c).mag_sq();
        }
    }
    let rms_mm = if pairs > 0 { libm::sqrt(sse / pairs as f64) } else { f64::INFINITY };
    if pairs < p.min_pairs || !(rms_mm <= p.k_sigma * p.sigma_mm) {
        return Err(DeviationError::NoAlignment { rms_mm, pairs });
    }
    Ok(Aligned { pose, rms_mm, pairs, iters })
}

/// The signed deviation of every scan vertex at `aligned.pose`, the statistics over the band, and the faces ranked.
pub fn deviations(scan: &ScanMesh, design: &Design, bvh: &Bvh, aligned: Aligned, p: &AlignParams, band_mm: f64, tol: Tol) -> Deviation {
    let normals = design.mesh.normals_face(tol);
    let gate = p.gate(2);
    let nf = design.faces as usize;
    // Per face: n, sum, sum of squares, the signed worst.
    let mut acc: Vec<(u32, f64, f64, f64)> = vec![(0, 0.0, 0.0, 0.0); nf];
    let mut per_vertex = Vec::with_capacity(scan.mesh.positions.len());
    let mut face_of_vertex = Vec::with_capacity(scan.mesh.positions.len());
    let mut abs: Vec<f64> = Vec::new();
    let mut sse = 0.0f64;
    for (i, &v) in scan.mesh.positions.iter().enumerate() {
        let q = aligned.pose.apply_point(v);
        let nq = aligned.pose.apply_vector(scan.normals.get(i).copied().unwrap_or(Vec3::ZERO));
        let r: Option<(f64, u32)> = bvh.closest(&design.mesh, q, gate, tol).and_then(|near| {
            let n = *normals.get(near.tri as usize)?;
            if n.mag_sq() == 0.0 || nq.dot(n) < p.normal_cos {
                return None;
            }
            let diff = q - near.point;
            let d = if diff.dot(n) < 0.0 { -near.dist } else { near.dist };
            let f = *design.face_of.get(near.tri as usize)? as usize;
            if let Some(a) = acc.get_mut(f) {
                a.0 += 1;
                a.1 += d;
                a.2 += d * d;
                if d.abs() > a.3.abs() {
                    a.3 = d;
                }
            }
            Some((d, f as u32))
        });
        if let Some((d, _)) = r {
            abs.push(d.abs());
            sse += d * d;
        }
        per_vertex.push(r.map(|(d, _)| d));
        face_of_vertex.push(r.map(|(_, f)| f));
    }
    let matched = abs.len();
    abs.sort_by(|a, b| a.total_cmp(b));
    let (rms_mm, max_mm, p95_mm, inside) = if matched == 0 {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        let rank = libm::ceil(0.95 * matched as f64) as usize;
        (
            libm::sqrt(sse / matched as f64),
            abs[matched - 1],
            abs[rank.clamp(1, matched) - 1],
            abs.iter().filter(|x| **x <= band_mm).count() as f64 / matched as f64,
        )
    };
    let stats = Stats {
        matched: matched as u32,
        unmatched: (per_vertex.len() - matched) as u32,
        rms_mm,
        max_mm,
        p95_mm,
        inside,
        band_mm,
    };
    let mut faces: Vec<FaceReport> = acc
        .iter()
        .enumerate()
        .map(|(f, &(n, s, s2, worst))| {
            let k = n.max(1) as f64;
            FaceReport {
                face: f as u32,
                n,
                mean_mm: if n > 0 { s / k } else { 0.0 },
                rms_mm: if n > 0 { libm::sqrt(s2 / k) } else { 0.0 },
                max_mm: worst,
            }
        })
        .collect();
    faces.sort_by(|a, b| (b.n > 0).cmp(&(a.n > 0)).then(b.rms_mm.total_cmp(&a.rms_mm)).then(a.face.cmp(&b.face)));
    Deviation { aligned, per_vertex, face_of_vertex, stats, faces }
}

/// [`align`] from `init`, then [`deviations`] with the band, over one BVH of the design.
pub fn measure(scan: &ScanMesh, design: &Design, init: Transform, p: &AlignParams, band_mm: f64, tol: Tol) -> Result<Deviation, DeviationError> {
    let bvh = Bvh::build(&design.mesh);
    let aligned = align(scan, design, &bvh, init, p, tol)?;
    Ok(deviations(scan, design, &bvh, aligned, p, band_mm, tol))
}

// ---- VUGFACEDEVIATION (LEDGER SR525): the ONE design builder and the ONE ramp, lifted from tools/scan ----

/// The design's tessellation chord (mm): its sagitta bias (at most half of it) is far under the scan's noise.
pub const DESIGN_CHORD_MM: f64 = 0.05;

/// Why [`design_of`] refused the solids.
#[derive(Clone, Debug, PartialEq)]
pub enum DesignError {
    /// brep_core could not tessellate a solid.
    Tessellate { solid: String, error: brep_core::Refusal },
    /// A tessellated triangle names a face the solid does not hold.
    FaceId { solid: String, id: u64 },
    /// The assembled design was refused ([`Design::new`]).
    Design(DeviationError),
}

/// Every solid tessellated at [`DESIGN_CHORD_MM`] into ONE design with each triangle's face, and the faces' names
/// (`<solid>/<op>.<role>.<a>.<b>:<kind>`, brep_core's persistent name, lower case) in face-index order.
pub fn design_of<'a>(solids: impl IntoIterator<Item = (&'a str, &'a brep_core::Brep)>, tol: Tol) -> Result<(Design, Vec<String>), DesignError> {
    let mut mesh = Mesh::default();
    let mut face_of: Vec<u32> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for (id, brep) in solids {
        let t = brep_core::tessellate(brep, DESIGN_CHORD_MM, tol).map_err(|error| DesignError::Tessellate { solid: id.into(), error })?;
        let mut index: BTreeMap<brep_core::Id, u32> = BTreeMap::new();
        for f in &brep.faces {
            index.insert(f.id, names.len() as u32);
            names.push(face_name(id, f));
        }
        let base = mesh.positions.len() as u32;
        mesh.positions.extend_from_slice(&t.mesh.positions);
        for (tri, fid) in t.mesh.triangles.iter().zip(&t.face_of) {
            let Some(&f) = index.get(fid) else {
                return Err(DesignError::FaceId { solid: id.into(), id: fid.0 });
            };
            mesh.triangles.push([tri[0] + base, tri[1] + base, tri[2] + base]);
            face_of.push(f);
        }
    }
    let faces = names.len() as u32;
    let design = Design::new(mesh, face_of, faces).map_err(DesignError::Design)?;
    Ok((design, names))
}

/// `<solid>/<op>.<role>.<a>.<b>:<kind>`, brep_core's persistent name of a face, lower case.
fn face_name(solid: &str, f: &brep_core::Face) -> String {
    let n = f.name;
    format!(
        "{solid}/{}.{}.{}.{}:{}",
        format!("{:?}", n.op).to_ascii_lowercase(),
        format!("{:?}", n.role).to_ascii_lowercase(),
        n.a,
        n.b,
        format!("{:?}", f.surface.kind()).to_ascii_lowercase()
    )
}

/// The band class in palette_core's status colours (`0x00RRGGBB`): inside POSITIVE, short toward INFO, proud toward
/// CRITICAL (by the class's fraction), off the design TEXT_SECONDARY (the dark row, vug-view's albedo).
pub fn colour(s: Shade) -> u32 {
    let ch = |c: u32, i: u32| ((c >> (16 - 8 * i)) & 0xff) as f64;
    let mix = |a: u32, b: u32, k: f64| {
        let k = k.clamp(0.0, 1.0);
        (0..3u32).fold(0u32, |acc, i| {
            let m = libm::round(ch(a, i) + (ch(b, i) - ch(a, i)) * k) as u32 & 0xff;
            acc | (m << (16 - 8 * i))
        })
    };
    match s {
        Shade::Inside => palette_core::STATUS_POSITIVE,
        Shade::Under(k) => mix(palette_core::STATUS_POSITIVE, palette_core::STATUS_INFO, k),
        Shade::Over(k) => mix(palette_core::STATUS_POSITIVE, palette_core::STATUS_CRITICAL, k),
        Shade::Off => palette_core::TEXT_SECONDARY.1,
    }
}

// ---- VUGSCANTSDFCONFIDENCE (LEDGER SR586): the confidence floor, a READ rule on the mesh's per-vertex confidence ----
// The scan's per-vertex confidence is the fused observation weight (`ScanMesh::confidence`, BP-vug-5). `floored`
// unpairs every vertex under a floor, so every reader of the Deviation inherits it with no signature change: gdt's
// form sets (`gdt`, `frame_of`, `in_frame`) skip it and `shade` reads it Off. MEASURED by tests/gdt.rs
// `tsdf_outliers_probe` (`conf floor_w`): at the post form set's 10th percentile (weight 4.00) the post's
// cylindricity read 3.542 -> 3.378 mm at D435i noise but 3.214 -> 3.259 noise-free, the top unchanged, so no caller
// applies it by default (0.0, OFF). Not a volume knob: `tsdf::Knobs` is the integrate's. The second accumulator (the
// votes' spread) was measured and refused (tsdf.rs `Voxel`).

/// `dev` with every paired vertex whose `scan.confidence` is under `conf_floor` unpaired (`per_vertex` and
/// `face_of_vertex` None); `stats` and `faces` stay as measured. A floor of 0.0 (or below) changes nothing.
pub fn floored(dev: &Deviation, scan: &ScanMesh, conf_floor: f32) -> Deviation {
    let mut out = dev.clone();
    for (i, (pv, fv)) in out.per_vertex.iter_mut().zip(out.face_of_vertex.iter_mut()).enumerate() {
        if pv.is_some() && scan.confidence.get(i).is_some_and(|w| *w < conf_floor) {
            (*pv, *fv) = (None, None);
        }
    }
    out
}

// ---- VUGSCANDEVIATIONCOARSE (LEDGER SR615): the global coarse registration ----
// A scan with no starting pose (a `--track` scan in frame 0's camera) opens against the design from its principal axes:
// the exact area moments of the scan mesh and of the design mesh ([`moments`], one fn for both: no sampler), their
// eigenvectors through geom_core's `jacobi_sym`, the four right-handed sign ambiguities of the design's frame each scored
// by the trimmed residual of `pair` (the deviation measure's own). The least start score above the fine's last gate
// (`AlignParams::gate(2)`) refuses (a fragment: its axes are not the part's). Each start is refined by `align` at
// σ·[`COARSE_WIDEN`] (the scene's ground biases the scan's frame toward the fine basin's edge) and the refined poses are
// SELECTED by the trimmed residual at gate(2) over [`COARSE_SELECT_KEEP`]: MEASURED (M3, the 12-frame orbit carried by
// a known pose) the start score picked a 180°-wrong sign (trimmed 5.357, the box faces symmetric under the turn) and the
// fine alignment ACCEPTED it (rms 0.555: `align`'s acceptance reads paired vertices only, the post unpaired), so a
// box-symmetric part is told apart only after refining all four. Then `align` at the caller's params. Cost:
// 4 × [`COARSE_POINTS`] `Bvh::closest` to score the starts, 4 × 3·iters·COARSE_POINTS for the refines (the budget of one
// align at 8192), 4 × COARSE_POINTS to select, then the default `align` (<= 3·iters·max_points).

/// How a registration started: the caller's pose (`Fine`), or [`coarse`]'s principal axes, kept (`CoarseFine`, the
/// start's trimmed rms) or refused at the floor (`CoarseRefused`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Road {
    Fine,
    CoarseFine { rms_mm: f64 },
    CoarseRefused { rms_mm: f64 },
}

/// The stride sample the four candidate starts are scored on.
pub const COARSE_POINTS: usize = 2048;
/// The trimmed fraction: the RMS of the best tenth of the sampled residuals (an unpaired vertex counts at the gate).
pub const COARSE_KEEP: f64 = 0.1;
/// The first refine's σ multiple: gates k·σ·2·[8, 4, 2], accepted at k·σ·2.
pub const COARSE_WIDEN: f64 = 2.0;
/// The trimmed fraction the refined poses are selected on (at gate(2)): the part is ~36% of the fixture scan's area,
/// its post ~8%, so a pose that leaves the post unpaired counts it inside the kept 30%.
pub const COARSE_SELECT_KEEP: f64 = 0.3;

/// The area-weighted centroid and covariance (row-major 3 x 3) of `mesh`, exact per triangle:
/// ∫x dA = A·s/3, ∫xxᵀ dA = A/12·(aaᵀ + bbᵀ + ccᵀ + ssᵀ), s = a + b + c. None on zero area.
fn moments(mesh: &Mesh) -> Option<(Vec3, [f64; 9])> {
    let (mut area, mut c, mut m) = (0.0f64, Vec3::ZERO, [0.0f64; 9]);
    for t in 0..mesh.triangles.len() {
        let Some([a, b, d]) = mesh.corners(t) else { continue };
        let w = (b - a).cross(d - a).mag() * 0.5;
        if !(w > 0.0) {
            continue;
        }
        let s = a + b + d;
        area += w;
        c = c + s * (w / 3.0);
        for v in [a, b, d, s] {
            let x = [v.x, v.y, v.z];
            for r in 0..3 {
                for k in 0..3 {
                    m[r * 3 + k] += w / 12.0 * x[r] * x[k];
                }
            }
        }
    }
    if !(area > 0.0) {
        return None;
    }
    let c = c * (1.0 / area);
    let x = [c.x, c.y, c.z];
    for r in 0..3 {
        for k in 0..3 {
            m[r * 3 + k] = m[r * 3 + k] / area - x[r] * x[k];
        }
    }
    Some((c, m))
}

/// The principal axes of `mesh`: the centroid, the largest eigenvector and the middle one (unit), and the three
/// eigenvalues ascending (VUGSCANDEVIATIONCOARSESYM SR636: [`pivot`] reads them).
fn axes(mesh: &Mesh) -> Option<(Vec3, Vec3, Vec3, [f64; 3])> {
    let (c, m) = moments(mesh)?;
    let (w, v) = geom_core::linalg::jacobi_sym(&m, 3);
    if w.len() != 3 || v.len() != 9 {
        return None;
    }
    let col = |j: usize| Vec3::new(v[j], v[3 + j], v[6 + j]);
    let x = col(2).normalize().ok()?;
    let y = col(1).normalize().ok()?;
    Some((c, x, y, [w[0], w[1], w[2]]))
}

/// The trimmed residual of `scan` at `pose`: per sampled vertex its `pair` distance within `gate` (or `gate` when it
/// pairs with nothing), sorted, the RMS of the best ceil(`keep`·n).
fn trimmed(scan: &ScanMesh, design: &Design, bvh: &Bvh, normals: &[Vec3], sample: &[usize], pose: Transform, gate: f64, keep: f64, p: &AlignParams, tol: Tol) -> f64 {
    let mut r: Vec<f64> = sample
        .iter()
        .map(|&i| {
            let q = pose.apply_point(scan.mesh.positions[i]);
            let nq = pose.apply_vector(scan.normals.get(i).copied().unwrap_or(Vec3::ZERO));
            pair(design, bvh, normals, q, nq, gate, p.normal_cos, tol).map_or(gate, |(c, _)| (q - c).mag())
        })
        .collect();
    if r.is_empty() {
        return f64::INFINITY;
    }
    r.sort_by(|a, b| a.total_cmp(b));
    let k = (libm::ceil(r.len() as f64 * keep) as usize).clamp(1, r.len());
    libm::sqrt(r[..k].iter().map(|x| x * x).sum::<f64>() / k as f64)
}

/// The stride sample of at most [`COARSE_POINTS`] vertices the coarse road scores on.
fn coarse_sample(nv: usize) -> Vec<usize> {
    (0..nv).step_by(nv.div_ceil(COARSE_POINTS).max(1)).collect()
}

/// The four coarse starts (scan → design) from the principal axes: the scan's frame onto each of the design's four
/// right-handed sign variants (x, y signs ++, --, -+, +-), each with its trimmed rms at the fine basin's gate
/// (`p.gate(0)`, [`COARSE_KEEP`]). None: an empty scan or design, or zero area.
pub fn coarse(scan: &ScanMesh, design: &Design, bvh: &Bvh, p: &AlignParams, tol: Tol) -> Option<Vec<(Transform, f64)>> {
    let nv = scan.mesh.positions.len();
    if nv == 0 || design.mesh.triangles.is_empty() {
        return None;
    }
    let (cs, xs, ys, _) = axes(&scan.mesh)?;
    let (cd, xd, yd, _) = axes(&design.mesh)?;
    let inv = crate::geom::from_axes(xs, ys, xs.cross(ys), cs).invert().ok()?;
    let normals = design.mesh.normals_face(tol);
    let sample = coarse_sample(nv);
    let mut out = Vec::with_capacity(4);
    for (sx, sy) in [(1.0, 1.0), (-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0)] {
        let (x, y) = (xd * sx, yd * sy);
        let start = crate::geom::from_axes(x, y, x.cross(y), cd).compose(inv);
        out.push((start, trimmed(scan, design, bvh, &normals, &sample, start, p.gate(0), COARSE_KEEP, p, tol)));
    }
    Some(out)
}

/// [`coarse`]'s four starts, refused when the least start rms is above the floor `p.gate(2)` (`Road::CoarseRefused`,
/// `NoAlignment` with that rms and no pair); else each start refined by [`align`] at σ·[`COARSE_WIDEN`] over
/// [`COARSE_POINTS`], the accepted refines selected by [`trimmed`] at `p.gate(2)` over [`COARSE_SELECT_KEEP`] (the
/// least, the first on a tie), [`align`] at `p` from it and [`deviations`] with the band (`Road::CoarseFine` carrying
/// the least start rms; no refine accepted, or the last align's refusal, is the result). One BVH of the design.
pub fn measure_coarse(scan: &ScanMesh, design: &Design, p: &AlignParams, band_mm: f64, tol: Tol) -> (Road, Result<Deviation, DeviationError>) {
    if scan.mesh.positions.is_empty() {
        return (Road::CoarseRefused { rms_mm: f64::INFINITY }, Err(DeviationError::EmptyScan));
    }
    if design.mesh.triangles.is_empty() {
        return (Road::CoarseRefused { rms_mm: f64::INFINITY }, Err(DeviationError::EmptyDesign));
    }
    let bvh = Bvh::build(&design.mesh);
    let starts = sym_or(scan, design, &bvh, p, tol, coarse(scan, design, &bvh, p, tol).unwrap_or_default()); // VUGSCANDEVIATIONCOARSESYM (SR636): a design degenerate in one moment pair takes the sweep about its pivot
    let rms_mm = starts.iter().map(|s| s.1).fold(f64::INFINITY, f64::min);
    let (starts, rms_mm) = view_or(scan, design, &bvh, p, tol, starts, rms_mm); // VUGSCANDEVIATIONCOARSEVIEW (SR626): SR615's floor refusing, a near-single view takes the swept starts
    if !(rms_mm <= p.gate(2)) {
        return (Road::CoarseRefused { rms_mm }, Err(DeviationError::NoAlignment { rms_mm, pairs: 0 }));
    }
    let wide = AlignParams { sigma_mm: p.sigma_mm * COARSE_WIDEN, max_points: p.max_points.min(COARSE_POINTS), ..*p };
    let normals = design.mesh.normals_face(tol);
    let sample = coarse_sample(scan.mesh.positions.len());
    let mut best: Option<(Transform, f64)> = None;
    let mut refused: Option<(f64, DeviationError)> = None;
    for &(start, start_rms) in &starts {
        match align(scan, design, &bvh, start, &wide, tol) {
            Ok(w) => {
                let score = trimmed(scan, design, &bvh, &normals, &sample, w.pose, p.gate(2), COARSE_SELECT_KEEP, p, tol);
                if best.map_or(true, |(_, b)| score < b) {
                    best = Some((w.pose, score));
                }
            }
            Err(e) => {
                if refused.map_or(true, |(r, _)| start_rms < r) {
                    refused = Some((start_rms, e));
                }
            }
        }
    }
    let result = match (best, refused) {
        (Some((pose, _)), _) => align(scan, design, &bvh, pose, p, tol).map(|a| deviations(scan, design, &bvh, a, p, band_mm, tol)),
        (None, Some((_, e))) => Err(e),
        (None, None) => Err(DeviationError::NoAlignment { rms_mm, pairs: 0 }),
    };
    (Road::CoarseFine { rms_mm }, result)
}

// ---- VUGSCANDEVIATIONCOARSEVIEW (LEDGER SR626): the near-single-view start ----
// A short tracked scan sees ONE side of the part from frame 0's camera (its world's origin), with the ground it stands
// on: MEASURED (an analytic model of SR621's 12-frame fixture) the scan is 79% ground, its largest and middle axes are
// the ground's and sit 70.5° off the design's in yaw, its smallest axis (the ground normal) 3.7° off, and its centroid
// 56 mm off the design's (a part-only centroid 57 mm: a one-sided view's centroid sits on its seen faces), so none of
// [`coarse`]'s four signs reaches the fine basin (SR621: 18.7 / 19.1 against gate(2) 9). The view's mean normal is the
// ground's up (the axis already right), and one fine step cannot cross 70°. So [`coarse_view`] widens the candidate set:
// the design frame swept about its smallest axis, both signs, in [`COARSE_VIEW_STEPS`] steps onto the scan's frame, each
// start's translation re-solved by [`COARSE_VIEW_SHIFTS`] point-to-point means of `pair` at gate(0)·[`COARSE_WIDEN`]
// ([`shift`]: the rotation is left to `align`) before it is scored by [`trimmed`] (the start score, gate(0),
// [`COARSE_KEEP`]); the best [`COARSE_VIEW_KEEP`] by the keep-[`COARSE_SELECT_KEEP`] score go into `measure_coarse`'s
// own refine / select loop (the box-symmetric 180° turn ranks first on the start's score and is told apart there, as
// SR615 measured). It runs only when SR615's floor refuses AND the scan is a view: at least [`COARSE_VIEW_FRONT`] of
// the sampled vertices face the scan's carried camera ([`front`], `ScanMesh::camera`, VUGSCANCOARSEVIEWORIGIN SR643: a
// mesh with none is refused this road; frame 0's camera, [0,0,0] in a tracked world; modelled 0.9975 for the 12-frame
// tracked scan, 0.855 / 0.865 for SR615's carried orbit and its half fragment, which keep SR615's road). Cost:
// 2·STEPS × (SHIFTS + 2) × [`COARSE_POINTS`] `Bvh::closest` (≈ 344k) and COARSE_POINTS for the front, then SR615's
// budget unchanged.

/// The least fraction of the sampled scan vertices whose normal faces the scan's carried camera for the view road.
pub const COARSE_VIEW_FRONT: f64 = 0.95;
/// The yaw steps of the sweep about the design's smallest axis (30°: the nearest within 15° of the truth).
pub const COARSE_VIEW_STEPS: usize = 12;
/// The point-to-point translation means each swept start takes before it is scored.
pub const COARSE_VIEW_SHIFTS: usize = 5;
/// The swept starts handed to `measure_coarse`'s refine / select loop (SR615's four-refine budget).
pub const COARSE_VIEW_KEEP: usize = 4;

/// The fraction of `sample`'s vertices whose normal faces `camera` (n·(c − q) > 0; VUGSCANCOARSEVIEWORIGIN SR643: the
/// scan's carried camera, never an assumed origin); a vertex with no normal counts against it. 0.0 on an empty sample.
fn front(scan: &ScanMesh, camera: Vec3, sample: &[usize]) -> f64 {
    if sample.is_empty() {
        return 0.0;
    }
    let facing = sample
        .iter()
        .filter(|&&i| {
            let q = scan.mesh.positions[i];
            let n = scan.normals.get(i).copied().unwrap_or(Vec3::ZERO);
            n.dot(camera - q) > 0.0
        })
        .count();
    facing as f64 / sample.len() as f64
}

/// `pose` with its translation re-solved: [`COARSE_VIEW_SHIFTS`] times, the mean of `c − q` over the sampled vertices
/// `pair` pairs within `gate`, applied on the left; it stops early when nothing pairs.
fn shift(scan: &ScanMesh, design: &Design, bvh: &Bvh, normals: &[Vec3], sample: &[usize], pose: Transform, gate: f64, p: &AlignParams, tol: Tol) -> Transform {
    let mut pose = pose;
    for _ in 0..COARSE_VIEW_SHIFTS {
        let (mut sum, mut n) = (Vec3::ZERO, 0u32);
        for &i in sample {
            let q = pose.apply_point(scan.mesh.positions[i]);
            let nq = pose.apply_vector(scan.normals.get(i).copied().unwrap_or(Vec3::ZERO));
            if let Some((c, _)) = pair(design, bvh, normals, q, nq, gate, p.normal_cos, tol) {
                sum = sum + (c - q);
                n += 1;
            }
        }
        if n == 0 {
            break;
        }
        pose = Transform::from_translation(sum * (1.0 / n as f64)).compose(pose);
    }
    pose
}

/// The swept starts (scan → design): None when the scan or design is empty, has zero area, or (`Sweep::View`) carries no camera (`ScanMesh::camera` None, VUGSCANCOARSEVIEWORIGIN SR643) or is not a
/// view ([`front`] under [`COARSE_VIEW_FRONT`]); else the design frame (x, y, z = x × y from the principal axes) with
/// the sweep's pivot (z for `Sweep::View`, frame index `i` for `Sweep::About(i)`, VUGSCANDEVIATIONCOARSESYM SR636) of
/// both signs, the next axis swept about it in [`COARSE_VIEW_STEPS`] steps, onto the scan's frame as [`coarse`] builds
/// it (for `Sweep::About`, cycled so the scan's own unique axis, [`partner`], takes the pivot's slot: VUGSCANCOARSESYMPIVOT
/// SR642), each [`shift`]ed at `p.gate(0)`·[`COARSE_WIDEN`]; the best [`COARSE_VIEW_KEEP`] by [`trimmed`] at `p.gate(0)`
/// over [`COARSE_SELECT_KEEP`] (the first on a tie), each carrying its start score ([`trimmed`] at `p.gate(0)`,
/// [`COARSE_KEEP`]).
pub fn coarse_view(scan: &ScanMesh, design: &Design, bvh: &Bvh, p: &AlignParams, tol: Tol, sweep: Sweep) -> Option<Vec<(Transform, f64)>> {
    Some(swept(scan, design, bvh, p, tol, sweep)?.into_iter().take(COARSE_VIEW_KEEP).map(|(_, t, s)| (t, s)).collect()) // VUGSCANCOARSESYMTHREEKEEP (SR659): the sweep lifted into `swept`, the keep here
}

/// Every start of [`coarse_view`]'s sweep as (rank, start, score), sorted by the rank ([`trimmed`] at `p.gate(0)` over
/// [`COARSE_SELECT_KEEP`], the first on a tie), the score [`trimmed`] at `p.gate(0)`, [`COARSE_KEEP`]; None as
/// [`coarse_view`] (VUGSCANCOARSESYMTHREEKEEP SR659: [`three_or`] keeps by the rank, the diagnostic seam the test prints).
pub fn swept(scan: &ScanMesh, design: &Design, bvh: &Bvh, p: &AlignParams, tol: Tol, sweep: Sweep) -> Option<Vec<(f64, Transform, f64)>> {
    let nv = scan.mesh.positions.len();
    if nv == 0 || design.mesh.triangles.is_empty() {
        return None;
    }
    let sample = coarse_sample(nv);
    let (axis, view) = match sweep {
        Sweep::View => (2, true),
        Sweep::About(i) => (i.min(2), false),
    };
    if view && !scan.camera.is_some_and(|camera| front(scan, camera, &sample) >= COARSE_VIEW_FRONT) { // VUGSCANCOARSEVIEWORIGIN (SR643): no carried camera, no view road (the About road needs none)
        return None;
    }
    let (cs, xs, ys, ws) = axes(&scan.mesh)?;
    let (cd, xd, yd, _) = axes(&design.mesh)?;
    let ends = partner(ws, sweep); // VUGSCANCOARSESYMPIVOT (SR642): the scan's partner of the pivot by its own eigenvalues; VUGSCANCOARSESYMBOTHENDS (SR647): both ends when the scan is degenerate at both
    let frame = [xd, yd, xd.cross(yd)];
    let normals = design.mesh.normals_face(tol);
    let mut scored: Vec<(f64, Transform, f64)> = Vec::with_capacity(ends.len() * 2 * COARSE_VIEW_STEPS);
    for &j in ends {
        let inv = scan_frame(xs, ys, cs, axis, j).invert().ok()?;
        for sz in [1.0, -1.0] {
            let z = frame[axis] * sz;
            let a = frame[(axis + 1) % 3];
            for k in 0..COARSE_VIEW_STEPS {
                let t = 2.0 * core::f64::consts::PI * k as f64 / COARSE_VIEW_STEPS as f64;
                let (c, s) = (libm::cos(t), libm::sin(t));
                // Rodrigues: the pivot's next axis turned by t about the unit pivot z (SR626's xd about zd for axis 2).
                let x = a * c + z.cross(a) * s + z * (z.dot(a) * (1.0 - c));
                let mut e = [Vec3::ZERO; 3];
                e[axis] = z;
                e[(axis + 1) % 3] = x;
                e[(axis + 2) % 3] = z.cross(x);
                let start = crate::geom::from_axes(e[0], e[1], e[2], cd).compose(inv);
                let start = shift(scan, design, bvh, &normals, &sample, start, p.gate(0) * COARSE_WIDEN, p, tol);
                let rank = trimmed(scan, design, bvh, &normals, &sample, start, p.gate(0), COARSE_SELECT_KEEP, p, tol);
                let score = trimmed(scan, design, bvh, &normals, &sample, start, p.gate(0), COARSE_KEEP, p, tol);
                scored.push((rank, start, score));
            }
        }
    }
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    Some(scored)
}

/// `measure_coarse`'s starts: SR615's four when their least score is inside `p.gate(2)`; else [`coarse_view`]'s when the
/// scan is a view (their least start score the road's rms, refused above the same floor); else SR615's unchanged.
fn view_or(scan: &ScanMesh, design: &Design, bvh: &Bvh, p: &AlignParams, tol: Tol, starts: Vec<(Transform, f64)>, rms_mm: f64) -> (Vec<(Transform, f64)>, f64) {
    if rms_mm <= p.gate(2) {
        return (starts, rms_mm);
    }
    match coarse_view(scan, design, bvh, p, tol, Sweep::View) {
        Some(view) if !view.is_empty() => {
            let r = view.iter().map(|s| s.1).fold(f64::INFINITY, f64::min);
            (view, r)
        }
        _ => (starts, rms_mm),
    }
}

// ---- VUGSCANROADWORDMARGIN (LEDGER SR637): the road's one word ----
// vug-view's corner (`align_line`, `align=` before it) and tools/scan's report (`road_report`, `road: ` before it)
// spelled the road each in their own match; the word is the road's, so it is spelled once here and the prefixes stay
// the callers'.

impl Road {
    /// The road's word: none for `Fine` (the caller's pose), `coarse+fine rms=<mm>` (the coarse start's trimmed rms)
    /// or `coarse refused rms=<mm>`, at three decimals.
    pub fn word(self) -> Option<String> {
        match self {
            Road::Fine => None,
            Road::CoarseFine { rms_mm } => Some(alloc::format!("coarse+fine rms={rms_mm:.3}")),
            Road::CoarseRefused { rms_mm } => Some(alloc::format!("coarse refused rms={rms_mm:.3}")),
        }
    }
}

// ---- VUGSCANDEVIATIONCOARSESYM (LEDGER SR636): the start of a design with near-equal principal moments ----
// Near-equal moments leave the design's axes in that pair free: a small feature, not the part, sets them, so SR615's four
// signs are a coin toss. MEASURED (an analytic model of the design's exact moments and of the orbit scan's visible
// surfaces): on a box 150 x Y x 100 with a post r20 h60 on its top (8% of the design's area) the pair's largest axis
// turns from the box's own long axis by 27.2° at Y = 156 (ratio 0.880), 20.4° at 0.863, 15.6° at 0.840, 11.0° at 0.803,
// so above ~0.835 such a feature turns the axes past 15°, SR626's half step: [`COARSE_SYM_RATIO`] 0.83. The test reads
// the DESIGN's moments only: SR626's tracked views are near-degenerate in the SCAN (0.951 / 0.873, a frustum of ground)
// while the bench design is not (0.447 / 0.256), so every SR615 / SR626 fixture keeps its road. On the 150 x 156 x 100
// box SR615's four starts sit 177 / 165 / 119 / 63° off the truth and its road accepts the 180° turn at rms 2.73;
// [`sym_or`] runs BEFORE the floor and gives `measure_coarse` SR626's own sweep ([`coarse_view`], `Sweep::About`) about
// the pivot, the well-determined axis (it contains SR615's four signs at steps 0 and 6), the best [`COARSE_VIEW_KEEP`]
// into SR615's refine / select loop: modelled the true start ranks first of 24 and aligns within 0.11°. Cost: SR626's,
// 2·STEPS × (SHIFTS + 2) × [`COARSE_POINTS`] `Bvh::closest` (≈ 344k), and one moments pass more over the design.
// Three near-equal moments stay on SR615's four (VUG-SCAN-DEVIATION-COARSE-SYM-THREE): the two-angle sweep (the cube's
// 13 symmetry lines × 2 × 12 = 312 starts, ≈ 4.47M closest) ranked the true start 2nd..49th, inside the KEEP 4 once in
// eight (a near-cube's 24 near-symmetric basins tie on the shift-only score).

/// The least ratio of two adjacent ascending eigenvalues of the design's moments (lo / hi) that makes them a degenerate
/// pair.
pub const COARSE_SYM_RATIO: f64 = 0.83;

/// What [`coarse_view`] sweeps about: `View` SR626's near-single view (the design's smallest axis, gated by [`front`]);
/// `About(i)` the pivot of a degenerate pair (`i` the design-frame index: 0 the largest axis, 2 the smallest), no view
/// test.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sweep {
    View,
    About(usize),
}

/// The pivot of `mesh`'s principal frame when exactly one adjacent pair of its ascending eigenvalues is degenerate
/// (lo ≥ [`COARSE_SYM_RATIO`]·hi): 2 (the smallest axis) when the middle and largest pair, 0 (the largest) when the
/// smallest and middle pair; None when neither pair or both (three near-equal: SR615's four, owed).
fn pivot(mesh: &Mesh) -> Option<usize> {
    let (_, _, _, w) = axes(mesh)?;
    let pair = degenerate; // VUGSCANCOARSESYMBOTHENDS (SR647): the one pair test
    match (pair(w[0], w[1]), pair(w[1], w[2])) {
        (false, true) => Some(2),
        (true, false) => Some(0),
        _ => None,
    }
}

/// `measure_coarse`'s starts: when the design is degenerate in one pair ([`pivot`]), [`coarse_view`]'s sweep about the
/// pivot (`Sweep::About`) in place of SR615's four; else `starts` unchanged.
fn sym_or(scan: &ScanMesh, design: &Design, bvh: &Bvh, p: &AlignParams, tol: Tol, starts: Vec<(Transform, f64)>) -> Vec<(Transform, f64)> {
    match pivot(&design.mesh).and_then(|axis| coarse_view(scan, design, bvh, p, tol, Sweep::About(axis))) {
        Some(sym) if !sym.is_empty() => sym,
        _ => three_or(scan, design, bvh, p, tol, starts), // VUGSCANCOARSESYMTHREEKEEP (SR659): a design three-equal keeps every near-tie start of the three sweeps
    }
}

// ---- VUGSCANCOARSESYMPIVOT (LEDGER SR642): the scan's partner of the design's pivot ----
// SR636's sweep turns the design about its pivot (the unique axis of a degenerate moment pair) but put that pivot on the
// scan frame's axis of the SAME index: right when both are the ground normal (SR636's near-square box: the design's
// smallest, the scan's smallest), wrong for a tall part on a ground scan. MEASURED (an analytic model of the orbit scan's
// visible surfaces, SR636's): the bench's post r40 h150 with four pins r10 h40 at radius 75 (C4: the design's two lateral
// moments equal, 1162.0 / 1162.0 / 2646.8, so its pivot is the LARGEST axis, index 0; a bare cylinder cannot be aligned
// at all: q × n has no axial part on it, so `align`'s spin is unconstrained) on a 360 x 310 ground (66.2% of the scan):
// the scan's eigenvalues 1949.6 / 5972.2 / 7924.0 (ratios 0.326 / 0.754) make its unique axis the SMALLEST, the ground
// normal, and the same-index pairing laid the post on the ground's long axis (every kept start 90° off, the road
// refused). [`partner`] reads the scan's own spectrum: the end whose ratio to its neighbour is the less degenerate (w0·w2
// against w1², division-free), the tie to the design's pivot; [`scan_frame`] cycles the scan frame so that axis takes
// the pivot's slot (a cyclic order keeps it right-handed; the same index is SR636's frame float for float, so SR636's
// fixture, whose scan pairs 2 with 2, keeps its numbers). Modelled the kept starts 0.0° off in axis and the road aligned
// within 0.16° modulo the pins' C4. A scan near-degenerate itself has no trustworthy end (the bare post on a 240 x 220
// ground: 0.957 / 0.896 picks a horizontal axis): both ends are tried by VUGSCANCOARSESYMBOTHENDS (SR647, below).

/// The scan-frame indices (0 the largest axis, 2 the smallest) that take the sweep's pivot, from the scan's ascending
/// eigenvalues `w`: `Sweep::View` `[2]` (SR626's road); `Sweep::About(i)` the scan's unique end, 2 when w0/w1 < w1/w2
/// (w0·w2 < w1²), 0 when greater, else (equal, or unordered) the design's pivot `i`; when both adjacent pairs are
/// [`degenerate`] (VUGSCANCOARSESYMBOTHENDS SR647: no trustworthy axis) all three, that end first, then the other end,
/// then the middle, `[2, 0, 1]` / `[0, 2, 1]`; else the list of one.
fn partner(w: [f64; 3], sweep: Sweep) -> &'static [usize] {
    match sweep {
        Sweep::View => &[2],
        Sweep::About(i) => {
            let (ends, mid) = (w[0] * w[2], w[1] * w[1]);
            let one = if ends < mid {
                2
            } else if ends > mid {
                0
            } else {
                i.min(2)
            };
            match (one, degenerate(w[0], w[1]) && degenerate(w[1], w[2])) {
                (2, true) => &[2, 0, 1],
                (0, true) => &[0, 2, 1],
                (0, _) => &[0],
                (1, _) => &[1],
                _ => &[2],
            }
        }
    }
}

/// The scan's principal frame (x the largest, y the middle, x × y) at `cs`, cycled so its axis `j` sits at slot `axis`
/// (`j == axis` is the frame in its own order).
fn scan_frame(xs: Vec3, ys: Vec3, cs: Vec3, axis: usize, j: usize) -> Transform {
    let f = [xs, ys, xs.cross(ys)];
    let mut g = [Vec3::ZERO; 3];
    for k in 0..3 {
        g[(axis + k) % 3] = f[(j + k) % 3];
    }
    crate::geom::from_axes(g[0], g[1], g[2], cs)
}

// ---- VUGSCANCOARSESYMBOTHENDS (LEDGER SR647): a scan degenerate at both ends tries all three axes ----
// SR642's [`partner`] reads the scan's own unique end, but a scan near-isotropic ITSELF has none to trust: MEASURED (an
// analytic model of the orbit scan's visible surfaces, SR642's calibrated: its fixture 1951.8 / 5978.4 / 7933.5 against
// the run's 1949.6 / 5972.2 / 7924.0, the bare post on 240 x 220 0.957 / 0.896 exactly) SR642's post with its four pins
// on a 240 x 220 ground has ratios 0.946 / 0.870 (w0·w2 / w1² 1.087): the ground normal is the SMALLEST axis but the
// end rule picks the largest, the ground's long axis, and the sweep lays the post down. The rule: both adjacent ratios
// [`degenerate`] (the design's own pair test, [`COARSE_SYM_RATIO`] 0.83, no new constant). The fused scan itself
// ([`scan_spectrum`], the test's diagnostic) reads 2610.1 / 2655.3 / 3076.3 (0.983 / 0.863) with the ground normal 4.1°
// off the MIDDLE axis (85.9° / 89.7° off the ends: the model's smallest led its middle by 5%, and the scan's noise and
// rim swap them), so both ends alone kept four starts ~90° off; the candidate list is therefore all three axes.
// Over the pinned post's grounds 120 x 110 .. 480 x 440 the end rule is wrong at exactly 220 x 202 (0.882 / 0.977) and 240 x 220, and the rule
// fires at exactly those two and at no ground where the end rule is right (200 x 183: 0.890 / 0.790; 260 x 238: 0.767 /
// 0.861); a band on w0·w2 / w1² separates nothing (wrong at 0.903 / 1.087, right at 0.890 / 1.127). [`coarse_view`]
// runs its sweep once per candidate (ONE loop over the list; the list of one is SR626's / SR636's / SR642's sweep float
// for float) and the best [`COARSE_VIEW_KEEP`] of the union go into `measure_coarse`'s refine / select loop unchanged.
// Cost: three times SR626's sweep (3 x 24 starts, ≈ 1.03M `Bvh::closest`) only for a scan degenerate at both ends.

/// Two adjacent ascending eigenvalues (lo ≤ hi) are a degenerate pair: lo ≥ [`COARSE_SYM_RATIO`]·hi (`pivot` on the
/// design, `partner` on the scan).
fn degenerate(lo: f64, hi: f64) -> bool {
    hi > 0.0 && lo >= COARSE_SYM_RATIO * hi
}

/// The scan's principal spectrum (VUGSCANCOARSESYMBOTHENDS SR647, the diagnostic seam): the three eigenvalues of its area
/// moments ascending and their unit eigenvectors in the same order (the smallest x × y, the middle, the largest), as
/// [`partner`] reads them. None: zero area.
pub fn scan_spectrum(mesh: &Mesh) -> Option<([f64; 3], [Vec3; 3])> {
    let (_, x, y, w) = axes(mesh)?;
    Some((w, [x.cross(y), y, x]))
}

// ---- VUGSCANCOARSESYMTHREEKEEP (LEDGER SR659): a three-equal design keeps every near-tie start ----
// A design whose three moments are near-equal (both adjacent ratios [`degenerate`]: [`pivot`] None) stayed on SR615's
// four starts. MEASURED by SR657 (vug-view tests `a_near_cube_s_basins_scored`: a 200 x 210 x 220 box with one pin r15
// h40, ratios 0.925 / 0.884): the trimmed keep 0.3 ranks the true basin 1st of the cube group's 24 at the basin and
// after the wide refine (gate(2) 0.0543 against its three 180° twins' 0.0599 mm), but the sweep about each design axis
// kept 4 a sweep and those 12 held the twins and dimension-swapped basins, not the truth: each swept start sits 12-15°
// off its basin (the 30° step) and that offset, not the basin, sets its start rank, so the near-tie group and the
// swapped basins interleave before the refine (basin 13 at 1.439 ahead of the twins at 1.513-1.709, keep 0.1). The
// keep-0.3 ranks were not printed, so the near-tie window is the road's own fine floor, `p.gate(2)` (no new constant):
// [`three_or`] runs [`swept`] about each design axis (`Sweep::About(0..3)`, [`partner`] as SR642 / SR647 read it) and
// hands EVERY start whose rank is within `p.gate(2)` of the best into `measure_coarse`'s refine / select loop
// unchanged, which tells the near-tie group apart (SR657's T1); a start that pairs nothing ranks toward `p.gate(0)` and
// drops. Cost: the three sweeps (3 x 24 starts, ≈ 1.03M `Bvh::closest`), then N wide refines (≤ 3·iters·COARSE_POINTS
// ≈ 123k each, ≤ 72 ≈ 8.9M) only for a three-equal design. Every other design's road is unchanged ([`three`] false).
// MEASURED (SR659, the near-cube): the 72 ranks span 4.649 .. 13.438, all within best + 9, so every start is kept; the
// truth's best start (rank 6.400, 16.2° off, 9th) refines and is selected: aligned 0.583, 0.006° / 0.031 mm.

/// The design's three moments are near-equal: both adjacent ascending eigenvalue pairs [`degenerate`] (SR647's one test).
fn three(mesh: &Mesh) -> bool {
    axes(mesh).is_some_and(|(_, _, _, w)| degenerate(w[0], w[1]) && degenerate(w[1], w[2]))
}

/// `measure_coarse`'s starts for a [`three`]-equal design: every start of [`swept`] about each design axis whose rank is
/// within `p.gate(2)` of the best rank of the union, in rank order, each with its start score; else (not three-equal,
/// or no start) `starts` unchanged.
fn three_or(scan: &ScanMesh, design: &Design, bvh: &Bvh, p: &AlignParams, tol: Tol, starts: Vec<(Transform, f64)>) -> Vec<(Transform, f64)> {
    if !three(&design.mesh) {
        return starts;
    }
    let mut all: Vec<(f64, Transform, f64)> = Vec::new();
    for i in 0..3usize {
        if let Some(s) = swept(scan, design, bvh, p, tol, Sweep::About(i)) {
            all.extend(s);
        }
    }
    let best = all.iter().map(|s| s.0).fold(f64::INFINITY, f64::min);
    if !best.is_finite() {
        return starts;
    }
    all.sort_by(|a, b| a.0.total_cmp(&b.0));
    let kept: Vec<(Transform, f64)> = all.into_iter().filter(|s| s.0 <= best + p.gate(2)).map(|(_, t, s)| (t, s)).collect();
    if kept.is_empty() { starts } else { kept }
}
