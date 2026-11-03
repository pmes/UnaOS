//! The validator: every later operation is checked by it (research 3.3 #2, "validator first"). It runs in a fixed
//! order, and the first failure is the refusal:
//! 1. references in range (`BadRef`);
//! 2. each edge's curve starts and ends on its vertices within Tol.lin (`VertexOffCurve`);
//! 3. each co-edge's curve lies on its face's surface at its ends and middle (`EdgeOffSurface`);
//! 4. each loop closes: a co-edge starts at the vertex where the previous one ended (`LoopNotClosed`);
//! 5. in each shell, each edge has exactly two co-edges (`CoEdgeCount`) of opposite sense (`CoEdgeSense`);
//! 6. per shell, Euler-Poincaré V - E + F - R = 2(1 - H) with an integer genus H >= 0 (`Euler`; R = inner loops);
//! 7. a planar face's outer loop is counter-clockwise about its outward normal, and its inner loops clockwise
//!    (`LoopDirection`);
//! 8. each solid's first shell encloses positive volume (`InsideOut`).

use crate::Refusal;
use geom_core::Tol;
use crate::mass::{loop_area, shell_volume};
use crate::surface::Surface;
use crate::topo::Brep;
use alloc::vec;
use alloc::vec::Vec;

/// What a valid Brep is made of. `genus` is per shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub vertices: usize,
    pub edges: usize,
    pub faces: usize,
    pub loops: usize,
    pub rings: usize,
    pub shells: usize,
    pub genus: Vec<i64>,
}

fn refs(b: &Brep) -> Result<(), Refusal> {
    let (nv, ne, nl, nf, ns) = (b.vertices.len(), b.edges.len(), b.loops.len(), b.faces.len(), b.shells.len());
    if b.edges.iter().any(|e| e.v.iter().any(|&v| v as usize >= nv)) {
        return Err(Refusal::BadRef { what: "edge vertex" });
    }
    if b.coedges.iter().any(|c| c.edge as usize >= ne || c.loop_ as usize >= nl) {
        return Err(Refusal::BadRef { what: "co-edge" });
    }
    for (li, l) in b.loops.iter().enumerate() {
        if l.coedges.is_empty() || l.face as usize >= nf {
            return Err(Refusal::BadRef { what: "loop" });
        }
        for &c in &l.coedges {
            if c as usize >= b.coedges.len() || b.coedges[c as usize].loop_ as usize != li {
                return Err(Refusal::BadRef { what: "loop co-edge" });
            }
        }
    }
    for (fi, f) in b.faces.iter().enumerate() {
        for lp in core::iter::once(f.outer).chain(f.inner.iter().copied()) {
            if lp as usize >= nl || b.loops[lp as usize].face as usize != fi {
                return Err(Refusal::BadRef { what: "face loop" });
            }
        }
    }
    if b.shells.iter().any(|s| s.faces.iter().any(|&f| f as usize >= nf)) {
        return Err(Refusal::BadRef { what: "shell face" });
    }
    if b.solids.iter().any(|s| s.shells.iter().any(|&h| h as usize >= ns)) {
        return Err(Refusal::BadRef { what: "solid shell" });
    }
    Ok(())
}

/// Validate a Brep against `tol`; the first failure, in the module's order, is the refusal.
pub fn validate(b: &Brep, tol: Tol) -> Result<Report, Refusal> {
    refs(b)?;

    for e in &b.edges {
        if !(e.t1 > e.t0) {
            return Err(Refusal::BadParam("edge parameter range"));
        }
        for (end, t) in [(0usize, e.t0), (1, e.t1)] {
            let v = &b.vertices[e.v[end] as usize];
            if e.curve.point(t).dist(v.p) > tol.lin {
                return Err(Refusal::VertexOffCurve { edge: e.id, vertex: v.id });
            }
        }
    }

    for f in &b.faces {
        for lp in core::iter::once(f.outer).chain(f.inner.iter().copied()) {
            for &ce in &b.loops[lp as usize].coedges {
                let e = &b.edges[b.coedges[ce as usize].edge as usize];
                for t in [e.t0, (e.t0 + e.t1) * 0.5, e.t1] {
                    if f.surface.distance(e.curve.point(t)) > tol.lin {
                        return Err(Refusal::EdgeOffSurface { edge: e.id, face: f.id });
                    }
                }
            }
        }
    }

    for l in &b.loops {
        let n = l.coedges.len();
        for i in 0..n {
            let (_, end) = b.coedge_ends(l.coedges[i]);
            let next = l.coedges[(i + 1) % n];
            let (start, _) = b.coedge_ends(next);
            if end != start {
                let edge = b.edges[b.coedges[next as usize].edge as usize].id;
                return Err(Refusal::LoopNotClosed { face: b.faces[l.face as usize].id, edge });
            }
        }
    }

    // Co-edge pairing, over the faces the shells list (a face dropped from its shell leaves its edges single).
    let mut uses = vec![0u32; b.edges.len()];
    let mut fwd = vec![0u32; b.edges.len()];
    let mut genus = Vec::new();
    for (si, s) in b.shells.iter().enumerate() {
        let mut vs = vec![false; b.vertices.len()];
        let mut es = vec![false; b.edges.len()];
        let mut rings = 0i64;
        for &f in &s.faces {
            rings += b.faces[f as usize].inner.len() as i64;
            for lp in b.face_loops(f) {
                for &ce in &b.loops[lp as usize].coedges {
                    let c = b.coedges[ce as usize];
                    uses[c.edge as usize] += 1;
                    if !c.reversed {
                        fwd[c.edge as usize] += 1;
                    }
                    es[c.edge as usize] = true;
                    for v in b.edges[c.edge as usize].v {
                        vs[v as usize] = true;
                    }
                }
            }
        }
        for (ei, used) in es.iter().enumerate() {
            if !used {
                continue;
            }
            if uses[ei] != 2 {
                return Err(Refusal::CoEdgeCount { edge: b.edges[ei].id, uses: uses[ei] });
            }
            if fwd[ei] != 1 {
                return Err(Refusal::CoEdgeSense { edge: b.edges[ei].id });
            }
        }
        let v = vs.iter().filter(|x| **x).count() as i64;
        let e = es.iter().filter(|x| **x).count() as i64;
        let chi = v - e + s.faces.len() as i64 - rings;
        if chi % 2 != 0 || 1 - chi / 2 < 0 {
            return Err(Refusal::Euler { shell: si as u32, chi });
        }
        genus.push(1 - chi / 2);
    }
    if let Some(ei) = (0..b.edges.len()).find(|&ei| uses[ei] == 0) {
        return Err(Refusal::CoEdgeCount { edge: b.edges[ei].id, uses: 0 });
    }

    for (fi, f) in b.faces.iter().enumerate() {
        if let Surface::Plane { .. } = f.surface {
            if !(loop_area(b, fi as u32, f.outer)? > 0.0) {
                return Err(Refusal::LoopDirection { face: f.id });
            }
            for &lp in &f.inner {
                if !(loop_area(b, fi as u32, lp)? < 0.0) {
                    return Err(Refusal::LoopDirection { face: f.id });
                }
            }
        }
    }

    for s in &b.solids {
        if let Some(&sh) = s.shells.first() {
            if !(shell_volume(b, sh)? > 0.0) {
                return Err(Refusal::InsideOut { shell: sh });
            }
        }
    }

    Ok(Report {
        vertices: b.vertices.len(),
        edges: b.edges.len(),
        faces: b.faces.len(),
        loops: b.loops.len(),
        rings: b.faces.iter().map(|f| f.inner.len()).sum(),
        shells: b.shells.len(),
        genus,
    })
}
