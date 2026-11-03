//! The topology arena. A solid is shells, a shell is faces, a face is a surface bounded by one outer loop and any
//! number of inner loops. A loop is a cycle of co-edges, and a co-edge is one use of an edge in one sense. In a closed
//! shell every edge has exactly two co-edges of opposite sense (a seam edge has both in the same face). Vertices sit
//! at edge ends.
//!
//! The stable-reference problem (VUG.md §3.3, research 4.1 #1): every vertex, edge and face carries an `Id`, minted
//! once from the Brep's counter in a fixed construction order, so the same input always yields the same Ids. Its
//! `Name` records the generator (operation, role, loop index, segment index) a later operation re-resolves against.
//! What this arc cannot carry it says: `Name` holds the profile's loop and segment indices, not sketch_core's
//! EntityId (VUG-BREP-NAMES owed).

use crate::curve::Curve;
use crate::dep::Vec3;
use crate::surface::Surface;
use alloc::vec::Vec;

/// A persistent identity, minted once per vertex, edge and face.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id(pub u64);

/// The operation that made an entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Extrude,
    Revolve,
}

/// The entity's role in its operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// On the start cap (extrude: the profile plane; revolve: angle 0).
    CapStart,
    /// On the end cap (extrude: the offset plane; revolve: the end angle).
    CapEnd,
    /// A face swept from a profile segment.
    Side,
    /// The seam edge of a periodic face (a revolve of 2π, or a full-circle profile segment).
    Seam,
    /// An edge swept by a profile vertex (extrude: the straight edge; revolve: the circle).
    Sweep,
}

/// What generated an entity: `a` is the profile loop index (0 = outer), `b` the segment (or vertex) index in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name {
    pub op: OpKind,
    pub role: Role,
    pub a: u32,
    pub b: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vertex {
    pub id: Id,
    pub name: Name,
    pub p: Vec3,
}

/// An edge: `curve` from `v[0]` at `t0` to `v[1]` at `t1` (t1 > t0; a closed edge has v[0] == v[1]).
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub id: Id,
    pub name: Name,
    pub curve: Curve,
    pub t0: f64,
    pub t1: f64,
    pub v: [u32; 2],
}

/// One use of an edge in a loop; `reversed` traverses it from v[1] to v[0].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoEdge {
    pub edge: u32,
    pub reversed: bool,
    pub loop_: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Loop {
    pub coedges: Vec<u32>,
    pub face: u32,
}

/// A curved face's rectangular domain in its surface's (u, v).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UvBox {
    pub u0: f64,
    pub u1: f64,
    pub v0: f64,
    pub v1: f64,
}

/// A face. Its outward normal is the surface normal, negated when `reversed`. Planar faces are bounded by their loops.
/// Curved faces in this arc are uv-rectangles (`uv`), the only shape extrude / revolve produce.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub id: Id,
    pub name: Name,
    pub surface: Surface,
    pub reversed: bool,
    pub outer: u32,
    pub inner: Vec<u32>,
    pub uv: Option<UvBox>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Shell {
    pub faces: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Solid {
    pub shells: Vec<u32>,
}

/// An Id resolved to its arena slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entity {
    Vertex(u32),
    Edge(u32),
    Face(u32),
}

/// The B-rep arena. The fields are public so a test (or a later operation) can read and mangle them; `validate` is
/// the judge of any Brep, however built.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Brep {
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
    pub coedges: Vec<CoEdge>,
    pub loops: Vec<Loop>,
    pub faces: Vec<Face>,
    pub shells: Vec<Shell>,
    pub solids: Vec<Solid>,
    next: u64,
}

impl Brep {
    pub fn new() -> Self {
        Self::default()
    }

    fn mint(&mut self) -> Id {
        self.next += 1;
        Id(self.next)
    }

    pub fn add_vertex(&mut self, name: Name, p: Vec3) -> u32 {
        let id = self.mint();
        self.vertices.push(Vertex { id, name, p });
        (self.vertices.len() - 1) as u32
    }

    pub fn add_edge(&mut self, name: Name, curve: Curve, t0: f64, t1: f64, v0: u32, v1: u32) -> u32 {
        let id = self.mint();
        self.edges.push(Edge { id, name, curve, t0, t1, v: [v0, v1] });
        (self.edges.len() - 1) as u32
    }

    /// A face from its loops (the first is the outer loop), each a list of (edge, reversed) co-edges in order.
    pub fn add_face(
        &mut self,
        name: Name,
        surface: Surface,
        reversed: bool,
        uv: Option<UvBox>,
        loops: &[Vec<(u32, bool)>],
    ) -> u32 {
        let id = self.mint();
        let face = self.faces.len() as u32;
        let mut ls = Vec::new();
        for l in loops {
            let li = self.loops.len() as u32;
            let mut ces = Vec::new();
            for &(edge, rev) in l {
                self.coedges.push(CoEdge { edge, reversed: rev, loop_: li });
                ces.push((self.coedges.len() - 1) as u32);
            }
            self.loops.push(Loop { coedges: ces, face });
            ls.push(li);
        }
        let outer = ls.first().copied().unwrap_or(u32::MAX);
        let inner = ls.iter().skip(1).copied().collect();
        self.faces.push(Face { id, name, surface, reversed, outer, inner, uv });
        face
    }

    pub fn add_shell(&mut self, faces: Vec<u32>) -> u32 {
        self.shells.push(Shell { faces });
        (self.shells.len() - 1) as u32
    }

    pub fn add_solid(&mut self, shells: Vec<u32>) -> u32 {
        self.solids.push(Solid { shells });
        (self.solids.len() - 1) as u32
    }

    /// The vertex, edge or face carrying `id`.
    pub fn find(&self, id: Id) -> Option<Entity> {
        if let Some(i) = self.vertices.iter().position(|v| v.id == id) {
            return Some(Entity::Vertex(i as u32));
        }
        if let Some(i) = self.edges.iter().position(|e| e.id == id) {
            return Some(Entity::Edge(i as u32));
        }
        self.faces.iter().position(|f| f.id == id).map(|i| Entity::Face(i as u32))
    }

    /// The generator of the entity carrying `id`.
    pub fn name(&self, id: Id) -> Option<Name> {
        Some(match self.find(id)? {
            Entity::Vertex(i) => self.vertices[i as usize].name,
            Entity::Edge(i) => self.edges[i as usize].name,
            Entity::Face(i) => self.faces[i as usize].name,
        })
    }

    /// Every loop of a face, outer first.
    pub fn face_loops(&self, face: u32) -> impl Iterator<Item = u32> + '_ {
        let f = &self.faces[face as usize];
        core::iter::once(f.outer).chain(f.inner.iter().copied())
    }

    /// The (start, end) vertex of a co-edge in its traversal sense.
    pub fn coedge_ends(&self, ce: u32) -> (u32, u32) {
        let c = self.coedges[ce as usize];
        let e = &self.edges[c.edge as usize];
        if c.reversed { (e.v[1], e.v[0]) } else { (e.v[0], e.v[1]) }
    }

    /// The co-edge's parameter range in traversal order (t_from, t_to).
    pub fn coedge_range(&self, ce: u32) -> (f64, f64) {
        let c = self.coedges[ce as usize];
        let e = &self.edges[c.edge as usize];
        if c.reversed { (e.t1, e.t0) } else { (e.t0, e.t1) }
    }
}
