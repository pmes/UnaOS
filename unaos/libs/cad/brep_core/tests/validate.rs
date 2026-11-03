//! VUGBREP1: mangled Breps are refused by name.

use brep_core::dep::Tol;
use brep_core::primitives::{box_, sphere};
use brep_core::{Refusal, validate};

const T: Tol = Tol::DEFAULT;

#[test]
fn a_flipped_coedge_breaks_its_loop() {
    let mut b = box_(1.0, 2.0, 3.0, T).unwrap();
    b.coedges[0].reversed = !b.coedges[0].reversed;
    assert!(matches!(validate(&b, T), Err(Refusal::LoopNotClosed { .. })), "{:?}", validate(&b, T));
}

#[test]
fn a_reversed_face_loop_breaks_the_pairing() {
    let mut b = box_(1.0, 2.0, 3.0, T).unwrap();
    let lp = b.faces[0].outer as usize;
    b.loops[lp].coedges.reverse();
    for ce in b.loops[lp].coedges.clone() {
        b.coedges[ce as usize].reversed = !b.coedges[ce as usize].reversed;
    }
    assert!(matches!(validate(&b, T), Err(Refusal::CoEdgeSense { .. })), "{:?}", validate(&b, T));
}

#[test]
fn flipped_faces_are_refused() {
    let mut b = box_(1.0, 2.0, 3.0, T).unwrap();
    for f in &mut b.faces {
        f.reversed = !f.reversed;
    }
    assert!(matches!(validate(&b, T), Err(Refusal::LoopDirection { .. })), "{:?}", validate(&b, T));
    let mut s = sphere(2.0, T).unwrap();
    for f in &mut s.faces {
        f.reversed = !f.reversed;
    }
    assert_eq!(validate(&s, T), Err(Refusal::InsideOut { shell: 0 }));
}

#[test]
fn a_dropped_face_leaves_single_coedges() {
    let mut b = box_(1.0, 2.0, 3.0, T).unwrap();
    b.shells[0].faces.remove(0);
    assert!(matches!(validate(&b, T), Err(Refusal::CoEdgeCount { uses: 1, .. })), "{:?}", validate(&b, T));
}

#[test]
fn a_moved_vertex_is_off_its_curve() {
    let mut b = box_(1.0, 2.0, 3.0, T).unwrap();
    b.vertices[0].p.x += 0.5;
    let id = b.vertices[0].id;
    assert!(matches!(validate(&b, T), Err(Refusal::VertexOffCurve { vertex, .. }) if vertex == id));
}
