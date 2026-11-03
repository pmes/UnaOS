//! CHARTER: Vug (CODEX §2, 3D) — shared-core: brep_core, the boundary-representation kernel of the CAD program
//! (docs/dev/USERLAND/VUG.md §4.3 and §5; the sheet docs/dev/blueprint/vug.md).
//!
//! VUGBREP1 (LEDGER SR468) builds:
//! - the topology arena (vertex, edge, co-edge, loop, face, shell, solid) where every face, edge and vertex carries a
//!   persistent [`Id`], minted once in a fixed order (the same input gives the same Ids), plus a [`Name`] saying which
//!   generator made it;
//! - the analytic surfaces (plane, cylinder, cone, sphere, torus), the analytic curves (line, circle, ellipse) and a
//!   rational NURBS curve (de Boor evaluation only), with the closed-form surface/surface intersections among planes,
//!   cylinders and spheres; every other pair is refused by name;
//! - [`validate`] (references, vertex-on-curve, edge-on-surface, loop closure, co-edge pairing, Euler-Poincaré per
//!   shell, planar loop direction, outward shell);
//! - [`primitives`], plus [`extrude`] / [`revolve`] of a sketch_core `Profile` (holes become inner loops);
//! - [`mass`] in closed form per face by the divergence theorem over the exact surfaces;
//! - [`tessellate`] into ONE mesh_core `Mesh` with a stated chord error and a per-triangle face-Id map.
//!
//! Refuse, don't guess: an input the arithmetic cannot decide, or a construction outside the analytic set, returns a
//! named [`Refusal`]. Coordinates are f64 (BP-vug-1). Every comparison takes the caller's `Tol` (BP-vug-2). The crate
//! is pure: no I/O, no clock, no global state (BP-vug-4). Booleans are `boolean` (SR481) and STEP is `step_core` (SR482); fillets and chamfers are `fillet` (SR502); NURBS surfaces and 3MF (VUG-3MF, mesh_core) are owed.
//!
//! VUGBREP2 (LEDGER SR481) builds [`union`] / [`difference`] / [`intersection`] over plane and cylinder faces
//! (src/bool.rs) and point classification by exact ray parity ([`point_in_solid`], src/classify.rs).
//!
//! VUGBREPSWAP (LEDGER SR492): the siblings are the real crates by path, re-exported here (the crates themselves for
//! sketch_core's `Edge` / `Loop`, which share names with this crate's topology); src/dep.rs and its shims are gone.
//!
//! VUGBREP3 (LEDGER SR502) builds [`fillet`] / [`chamfer`] of straight edges between two planar faces, convex or
//! concave (src/fillet.rs: the cylinder blend, the chamfer plane; `RadiusTooLarge` and `CornerNotBlended` by name).
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod build;
// VUGBREP2 (SR481): the file is src/bool.rs, the module `boolean`: a root `mod bool` would share the type namespace
// with the primitive `bool` that this file's derive expansions name.
#[path = "bool.rs"]
pub mod boolean;
pub mod classify;
pub mod curve;
mod earcut;
// VUGBREP3 (SR502): fillets and chamfers.
pub mod fillet;
pub mod mass;
pub mod primitives;
mod quad;
pub mod surface;
pub mod tess;
pub mod topo;
pub mod validate;

pub use boolean::{Op, difference, faces_named, intersection, union};
pub use build::{extrude, revolve};
pub use classify::{Class, Where, point_in_solid};
pub use curve::{Curve, Frame, NurbsCurve};
pub use fillet::{chamfer, fillet};
pub use mass::{Mass, area, centroid, inertia, properties, volume};
pub use surface::{Intersection, Surface, SurfaceKind, intersect};
pub use tess::{Tessellation, tessellate};
pub use topo::{Brep, CoEdge, Edge, Entity, Face, Id, Loop, Name, OpKind, Role, Shell, Solid, UvBox, Vertex};
pub use validate::{Report, validate};

pub use geom_core;
pub use geom_core::{Arc2, Degenerate, Tol, Vec2, Vec3};
pub use mesh_core;
pub use mesh_core::Mesh;
pub use sketch_core;
pub use sketch_core::{EntityId, Profile};

/// Every way brep_core says no. Each variant names what was refused, and where an entity is at fault, its [`Id`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Refusal {
    /// A geometric input geom_core calls degenerate (zero vector, zero radius, non-finite, ...).
    Degenerate(geom_core::Degenerate),
    /// An exact predicate could not decide (geom_core's `Predicate::Uncertain`), so the operation does not guess.
    Undecidable,
    /// A construction outside what this crate builds (named).
    Unsupported(&'static str),
    /// No closed-form intersection is implemented for this surface pair (VUG-BREP-SSI owed).
    NotClosedForm { a: SurfaceKind, b: SurfaceKind },
    /// An arena index points outside its table (named by the table).
    BadRef { what: &'static str },
    /// An edge's curve does not start or end on its vertex within `Tol.lin`.
    VertexOffCurve { edge: Id, vertex: Id },
    /// A co-edge's curve leaves its face's surface by more than `Tol.lin`.
    EdgeOffSurface { edge: Id, face: Id },
    /// A loop does not close: the co-edge on `edge` does not start where the previous one ended.
    LoopNotClosed { face: Id, edge: Id },
    /// In a closed shell an edge must be used by exactly two co-edges.
    CoEdgeCount { edge: Id, uses: u32 },
    /// The two co-edges of an edge run in the same sense (a face is turned against its neighbour).
    CoEdgeSense { edge: Id },
    /// V - E + F - R = 2(1 - H) has no integer genus H >= 0 for this shell (chi = V - E + F - R).
    Euler { shell: u32, chi: i64 },
    /// A planar face's outer loop is not counter-clockwise (or an inner loop not clockwise) about its outward normal.
    LoopDirection { face: Id },
    /// The shell encloses negative volume: its faces point inward.
    InsideOut { shell: u32 },
    /// A profile's outer loop is not counter-clockwise, or a hole is not clockwise.
    ProfileOrientation,
    /// A revolve profile has material on both sides of the axis.
    ProfileCrossesAxis,
    /// An arc extruded along a direction off the sketch normal would be an elliptic cylinder, which is not in the set.
    ObliqueArc,
    /// A parameter outside its domain (named).
    BadParam(&'static str),
    /// The planar face could not be triangulated (no ear, or a boundary node that does not meet its edge sample).
    Triangulation { face: Id },
    /// The chord bound was not met after the step was halved six times.
    ChordUnreachable { face: Id },
    /// VUGBREP2 (SR481): a boolean met a contact the closed-form road does not settle (coincident faces, a vertex on a
    /// face, an edge meeting an edge, tangency), named by `what`; `a` is the first operand's entity, `b` the second's
    /// (Ids are per operand).
    Contact { what: &'static str, a: Id, b: Id },
    /// VUGBREP2 (SR481): the kept faces of a boolean do not sew: this result edge is not used exactly once each way.
    NotClosed { edge: Id },
    /// VUGBREP3 (SR502): a fillet or chamfer this large would reach past a neighbour edge's far end, over another
    /// feature of a face it trims, or into another listed edge's trim; `max` is the bound it reaches (the radius; for a
    /// chamfer d1 at the given d2/d1), and a result needs the parameter below it by more than Tol.lin.
    RadiusTooLarge { edge: Id, max: f64 },
    /// VUGBREP3 (SR502): two or more listed edges meet at this vertex; the corner blend (and a chamfer's mitre) is
    /// VUG-BREP-3-CORNER, owed.
    CornerNotBlended { vertex: Id },
}

impl From<geom_core::Degenerate> for Refusal {
    fn from(d: geom_core::Degenerate) -> Self {
        Refusal::Degenerate(d)
    }
}
