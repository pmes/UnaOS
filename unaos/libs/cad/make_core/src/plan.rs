// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The kinematic pass: the planner's duration of every move from the machine's feed and acceleration limits.
//!
//! Trapezoid, no jerk model; every motion move starts and ends at rest (no junction look-ahead), so the total is an
//! UPPER BOUND on the executor's time until VUG-MAKE-LOOKAHEAD plans junction velocities. These durations are what
//! VUG-MAKE-QUEUE will timestamp for a clock-synchronised executor.
//!
//! The program's position before its first motion move is the machine's state, not the program's: that move (and
//! the first move after a `Home`) is timed 0 and counted `untimed`, as are tool changes and heat waits.

use alloc::vec::Vec;
use core::f64::consts::{FRAC_PI_2, PI, TAU};
use libm::{atan2, fabs, hypot, sqrt};

use crate::geom::{Tol, Vec2, Vec3};
use crate::machine::{AxisId, Machine};
use crate::program::{Move, Program};

/// Time to cover `l` from rest to rest at cruise `v` (units/s) under acceleration `a` (units/s²).
pub fn trapezoid(l: f64, v: f64, a: f64) -> f64 {
    if l <= 0.0 || v <= 0.0 || !v.is_finite() {
        return 0.0;
    }
    if !(a > 0.0) || !a.is_finite() {
        return l / v;
    }
    if l >= v * v / a { l / v + v / a } else { 2.0 * sqrt(l / a) }
}

/// The directional limits for unit direction `u`: (speed cap in units/min, acceleration cap in units/s²); each axis
/// that moves bounds the move by its own limit over its share of the direction.
pub fn caps(m: &Machine, u: Vec3, rapid: bool) -> (f64, f64) {
    let (mut v, mut a) = (f64::INFINITY, f64::INFINITY);
    for (id, c) in [(AxisId::X, u.x), (AxisId::Y, u.y), (AxisId::Z, u.z)] {
        let c = fabs(c);
        if c <= 1e-12 {
            continue;
        }
        if let Some(ax) = m.axis(id) {
            v = v.min((if rapid { ax.rapid } else { ax.max_feed }) / c);
            a = a.min(ax.max_accel / c);
        }
    }
    (v, a)
}

/// The XY limits an arc runs under: (min X/Y feed, min X/Y acceleration).
pub fn arc_caps(m: &Machine) -> (f64, f64) {
    let (mut v, mut a) = (f64::INFINITY, f64::INFINITY);
    for id in [AxisId::X, AxisId::Y] {
        if let Some(ax) = m.axis(id) {
            v = v.min(ax.max_feed);
            a = a.min(ax.max_accel);
        }
    }
    (v, a)
}

fn norm_angle(x: f64) -> f64 {
    let mut x = libm::fmod(x, TAU);
    if x < 0.0 {
        x += TAU;
    }
    x
}

/// The sweep (radians, in `(0, 2π]`) from `s` to `e` about `c`; equal ends are a full circle (G-code's meaning).
pub fn arc_sweep(s: Vec2, e: Vec2, c: Vec2, cw: bool, tol: Tol) -> f64 {
    let a0 = atan2(s.y - c.y, s.x - c.x);
    let a1 = atan2(e.y - c.y, e.x - c.x);
    let sw = if cw { norm_angle(a0 - a1) } else { norm_angle(a1 - a0) };
    if sw <= tol.ang || (s - e).length() <= tol.lin { TAU } else { sw }
}

/// The XY extremes of an arc: its ends and every quadrant point the sweep passes.
pub fn arc_bbox(s: Vec2, e: Vec2, c: Vec2, cw: bool, tol: Tol) -> (Vec2, Vec2) {
    let r = (s - c).length();
    let sw = arc_sweep(s, e, c, cw, tol);
    let a0 = atan2(s.y - c.y, s.x - c.x);
    let (mut lo, mut hi) = (Vec2::new(s.x.min(e.x), s.y.min(e.y)), Vec2::new(s.x.max(e.x), s.y.max(e.y)));
    for k in [0.0, FRAC_PI_2, PI, 3.0 * FRAC_PI_2] {
        let d = if cw { norm_angle(a0 - k) } else { norm_angle(k - a0) };
        if d <= sw {
            let p = Vec2::new(c.x + r * libm::cos(k), c.y + r * libm::sin(k));
            lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
        }
    }
    (lo, hi)
}

fn straight(m: &Machine, from: Vec3, to: Vec3, feed: Option<f64>, tol: Tol) -> f64 {
    let d = to - from;
    let l = d.length();
    if l <= tol.lin {
        return 0.0;
    }
    let (vcap, a) = caps(m, d * (1.0 / l), feed.is_none());
    let v = match feed {
        Some(f) => f.min(vcap),
        None => vcap,
    };
    trapezoid(l, v / 60.0, a)
}

/// Per-move durations (seconds) and the count of moves the planner cannot time.
pub fn durations(p: &Program, m: &Machine, tol: Tol) -> (Vec<f64>, u32) {
    let mut out = Vec::with_capacity(p.moves.len());
    let mut untimed = 0u32;
    let mut pos: Option<Vec3> = None;
    let mut e_now = 0.0f64;
    for mv in &p.moves {
        let t = match mv {
            Move::Rapid { to } => {
                let t = match pos {
                    Some(s) => straight(m, s, *to, None, tol),
                    None => {
                        untimed += 1;
                        0.0
                    }
                };
                pos = Some(*to);
                t
            }
            Move::Linear { to, feed } => {
                let t = match pos {
                    Some(s) => straight(m, s, *to, Some(*feed), tol),
                    None => {
                        untimed += 1;
                        0.0
                    }
                };
                pos = Some(*to);
                t
            }
            Move::Arc { to, centre, cw, feed } => {
                let t = match pos {
                    Some(s) => {
                        let r = (s.xy() - *centre).length();
                        let sw = arc_sweep(s.xy(), to.xy(), *centre, *cw, tol);
                        let l = hypot(r * sw, to.z - s.z);
                        let (vcap, a) = arc_caps(m);
                        let v = (feed.min(vcap) / 60.0).min(sqrt(a * r));
                        trapezoid(l, v, a)
                    }
                    None => {
                        untimed += 1;
                        0.0
                    }
                };
                pos = Some(*to);
                t
            }
            Move::Extrude { to, e, feed } => {
                let t = match pos {
                    Some(s) if (*to - s).length() > tol.lin => straight(m, s, *to, Some(*feed), tol),
                    // A pure E move (retract / prime): the E axis is not modelled, so constant speed.
                    Some(_) if *feed > 0.0 => fabs(*e - e_now) / (*feed / 60.0),
                    Some(_) => 0.0,
                    None => {
                        untimed += 1;
                        0.0
                    }
                };
                pos = Some(*to);
                e_now = *e;
                t
            }
            Move::SetE { e } => {
                e_now = *e;
                0.0
            }
            Move::Dwell { s } => *s,
            Move::Home => {
                pos = None;
                untimed += 1;
                0.0
            }
            Move::ToolChange { .. } | Move::Heat { wait: true, .. } => {
                untimed += 1;
                0.0
            }
            _ => 0.0,
        };
        out.push(t);
    }
    (out, untimed)
}
