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
//! The start is the caller's (`init`): a world-posed scan starts at the identity; a global coarse registration is
//! owed (VUG-SCAN-DEVIATION-COARSE).

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
    let mut abs: Vec<f64> = Vec::new();
    let mut sse = 0.0f64;
    for (i, &v) in scan.mesh.positions.iter().enumerate() {
        let q = aligned.pose.apply_point(v);
        let nq = aligned.pose.apply_vector(scan.normals.get(i).copied().unwrap_or(Vec3::ZERO));
        let d = bvh.closest(&design.mesh, q, gate, tol).and_then(|near| {
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
            Some(d)
        });
        if let Some(d) = d {
            abs.push(d.abs());
            sse += d * d;
        }
        per_vertex.push(d);
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
    Deviation { aligned, per_vertex, stats, faces }
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
