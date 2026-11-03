//! The primitives, each a call into build.rs's one topology road: the box is an extruded rectangle, and the quadrics
//! are revolutions of their meridian loop about the world z axis (the sketch placed in the x-z plane). Each takes
//! `Tol` like every other entry point (BP-vug-2).

use crate::Refusal;
use crate::build::{Seg2, check_loops, extrude_loops, revolve_loops};
use crate::curve::Frame;
use geom_core::{Arc2, Tol, Vec2, Vec3};
use alloc::vec;
use alloc::vec::Vec;
use core::f64::consts::{FRAC_PI_2, PI, TAU};

fn pos(x: f64, what: &'static str) -> Result<f64, Refusal> {
    if x > 0.0 && x.is_finite() { Ok(x) } else { Err(Refusal::BadParam(what)) }
}

fn poly(pts: &[(f64, f64)]) -> Vec<Seg2> {
    (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            Seg2::Line(Vec2::new(a.0, a.1), Vec2::new(b.0, b.1))
        })
        .collect()
}

/// The sketch plane x-z (sketch x = world x, sketch y = world z), revolved about the sketch y axis = world z.
fn xz() -> Result<Frame, Refusal> {
    Frame::new(Vec3::ZERO, -Vec3::Y, Vec3::X)
}

fn revolve_z(loops: Vec<Vec<Seg2>>, tol: Tol) -> Result<crate::Brep, Refusal> {
    check_loops(&loops, tol)?;
    revolve_loops(&loops, &xz()?, Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), TAU, tol)
}

/// The box [0, dx] x [0, dy] x [0, dz].
pub fn box_(dx: f64, dy: f64, dz: f64, tol: Tol) -> Result<crate::Brep, Refusal> {
    let (dx, dy, dz) = (pos(dx, "box dx")?, pos(dy, "box dy")?, pos(dz, "box dz")?);
    let loops = vec![poly(&[(0.0, 0.0), (dx, 0.0), (dx, dy), (0.0, dy)])];
    check_loops(&loops, tol)?;
    extrude_loops(&loops, &Frame::WORLD, Vec3::Z, dz, tol)
}

/// The cylinder of radius r about the z axis, from z = 0 to z = h.
pub fn cylinder(r: f64, h: f64, tol: Tol) -> Result<crate::Brep, Refusal> {
    let (r, h) = (pos(r, "cylinder r")?, pos(h, "cylinder h")?);
    revolve_z(vec![poly(&[(0.0, 0.0), (r, 0.0), (r, h), (0.0, h)])], tol)
}

/// The cone (frustum) about the z axis: radius r1 at z = 0, r2 at z = h (r2 = 0 is the pointed cone).
pub fn cone(r1: f64, r2: f64, h: f64, tol: Tol) -> Result<crate::Brep, Refusal> {
    let (r1, h) = (pos(r1, "cone r1")?, pos(h, "cone h")?);
    if !(r2 >= 0.0) || !r2.is_finite() {
        return Err(Refusal::BadParam("cone r2"));
    }
    let pts: Vec<(f64, f64)> =
        if r2 > tol.lin { vec![(0.0, 0.0), (r1, 0.0), (r2, h), (0.0, h)] } else { vec![(0.0, 0.0), (r1, 0.0), (0.0, h)] };
    revolve_z(vec![poly(&pts)], tol)
}

/// The sphere of radius r centred at the origin.
pub fn sphere(r: f64, tol: Tol) -> Result<crate::Brep, Refusal> {
    let r = pos(r, "sphere r")?;
    let arc = Seg2::Arc(Arc2 { centre: Vec2::new(0.0, 0.0), radius: r, start: -FRAC_PI_2, sweep: PI });
    revolve_z(vec![vec![arc, Seg2::Line(Vec2::new(0.0, r), Vec2::new(0.0, -r))]], tol)
}

/// The torus about the z axis: tube centre at distance `major`, tube radius `minor` (major > minor).
pub fn torus(major: f64, minor: f64, tol: Tol) -> Result<crate::Brep, Refusal> {
    let (major, minor) = (pos(major, "torus major")?, pos(minor, "torus minor")?);
    if !(major > minor) {
        return Err(Refusal::BadParam("torus major <= minor"));
    }
    let tube = Seg2::Arc(Arc2 { centre: Vec2::new(major, 0.0), radius: minor, start: 0.0, sweep: TAU });
    revolve_z(vec![vec![tube]], tol)
}
