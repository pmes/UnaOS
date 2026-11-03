// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The writer: a `Doc` to text, the same bytes for the same `Doc` (a fixed key order per verb, an Option key omitted
//! when None), with the ONE f64 form and the ONE string escape.

use alloc::format;
use alloc::string::String;
use core::fmt::Write as _;

use make_core::{Aabb3, Op, StockShape, Vec2, Vec3};

use crate::{
    AXES, Cons, Doc, FACE_DIRS, FORMAT, KINEMATICS, MACHINE_KINDS, POWER_KINDS, PRef, Pose, SIDES, Stmt, TOOL_KINDS, UNITS, VERSION, YESNO,
    word_of,
};

/// The shortest text that parses back to the same bits: core::fmt's shortest round-trip digits (no ryu in the tree),
/// plain for 1e-5 <= |x| < 1e16 and exponent form otherwise; `0` / `-0`; `nan` / `inf` / `-inf` (written so the
/// writer is total; [`crate::read`] refuses them by name).
pub fn f64_text(x: f64) -> String {
    if x.is_nan() {
        return String::from("nan");
    }
    if x.is_infinite() {
        return String::from(if x > 0.0 { "inf" } else { "-inf" });
    }
    if x == 0.0 {
        return String::from(if x.is_sign_negative() { "-0" } else { "0" });
    }
    let a = if x < 0.0 { -x } else { x };
    if (1e-5..1e16).contains(&a) { format!("{}", x) } else { format!("{:e}", x) }
}

/// The ONE string escape, quotes included: `\` `"` LF CR TAB as `\\` `\"` `\n` `\r` `\t`, any other C0 control or
/// DEL as `\u{hex}`, everything else as its UTF-8.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The document as text: the header, then one line per statement, each ended by LF.
pub fn write(doc: &Doc) -> String {
    let mut w = W { out: String::new() };
    w.out.push_str(FORMAT);
    w.u("version", VERSION as u64);
    w.f("lin", doc.tol.lin);
    w.f("ang", doc.tol.ang);
    w.out.push('\n');
    for s in &doc.stmts {
        stmt(&mut w, s);
        w.out.push('\n');
    }
    w.out
}

struct W {
    out: String,
}

impl W {
    fn head(&mut self, verb: &str, id: &str) {
        self.out.push_str(verb);
        self.out.push(' ');
        self.out.push_str(id);
    }
    fn key(&mut self, k: &str) {
        self.out.push(' ');
        self.out.push_str(k);
        self.out.push('=');
    }
    fn f(&mut self, k: &str, x: f64) {
        self.key(k);
        self.out.push_str(&f64_text(x));
    }
    fn u(&mut self, k: &str, n: u64) {
        self.key(k);
        let _ = write!(self.out, "{}", n);
    }
    fn s(&mut self, k: &str, v: &str) {
        self.key(k);
        self.out.push_str(&escape(v));
    }
    fn w(&mut self, k: &str, word: &str) {
        self.key(k);
        self.out.push_str(word);
    }
    fn pref(&mut self, k: &str, p: &PRef) {
        self.key(k);
        match p {
            PRef::Point(id) => self.out.push_str(id),
            PRef::Centre(id) => {
                self.out.push_str(id);
                self.out.push_str(".centre")
            }
            PRef::Start(id) => {
                self.out.push_str(id);
                self.out.push_str(".start")
            }
            PRef::End(id) => {
                self.out.push_str(id);
                self.out.push_str(".end")
            }
        }
    }
    /// `[a b ...]` with no key.
    fn nums(&mut self, xs: &[f64]) {
        self.out.push('[');
        for (i, x) in xs.iter().enumerate() {
            if i > 0 {
                self.out.push(' ');
            }
            self.out.push_str(&f64_text(*x));
        }
        self.out.push(']');
    }
    fn arr(&mut self, k: &str, xs: &[f64]) {
        self.key(k);
        self.nums(xs);
    }
    fn v3(&mut self, k: &str, v: Vec3) {
        self.arr(k, &[v.x, v.y, v.z]);
    }
    fn bare_box(&mut self, b: &Aabb3) {
        self.out.push('[');
        self.nums(&[b.min.x, b.min.y, b.min.z]);
        self.out.push(' ');
        self.nums(&[b.max.x, b.max.y, b.max.z]);
        self.out.push(']');
    }
    fn bare_pts(&mut self, pts: &[Vec2]) {
        self.out.push('[');
        for (i, p) in pts.iter().enumerate() {
            if i > 0 {
                self.out.push(' ');
            }
            self.nums(&[p.x, p.y]);
        }
        self.out.push(']');
    }
    fn bare_polys(&mut self, polys: &[alloc::vec::Vec<Vec2>]) {
        self.out.push('[');
        for (i, p) in polys.iter().enumerate() {
            if i > 0 {
                self.out.push(' ');
            }
            self.bare_pts(p);
        }
        self.out.push(']');
    }
    fn pts(&mut self, k: &str, pts: &[Vec2]) {
        self.key(k);
        self.bare_pts(pts);
    }
    fn polys(&mut self, k: &str, polys: &[alloc::vec::Vec<Vec2>]) {
        self.key(k);
        self.bare_polys(polys);
    }
}

fn stmt(w: &mut W, s: &Stmt) {
    match s {
        Stmt::Sketch { id, o, z, x } => {
            w.head("sketch", id);
            w.arr("o", o);
            w.arr("z", z);
            w.arr("x", x);
        }
        Stmt::Point { id, sketch, at } => {
            w.head("point", id);
            w.w("sketch", sketch);
            w.f("x", at[0]);
            w.f("y", at[1]);
        }
        Stmt::Line { id, sketch, a, b } => {
            w.head("line", id);
            w.w("sketch", sketch);
            w.w("a", a);
            w.w("b", b);
        }
        Stmt::Circle { id, sketch, centre, r } => {
            w.head("circle", id);
            w.w("sketch", sketch);
            w.w("centre", centre);
            w.f("r", *r);
        }
        Stmt::Arc { id, sketch, centre, r, start, end } => {
            w.head("arc", id);
            w.w("sketch", sketch);
            w.w("centre", centre);
            w.f("r", *r);
            w.f("start", *start);
            w.f("end", *end);
        }
        Stmt::Constraint { id, sketch, c } => {
            w.head(c.verb(), id);
            w.w("sketch", sketch);
            cons(w, c);
        }
        Stmt::Extrude { id, sketch, outer, dir, length } => {
            w.head("extrude", id);
            w.w("sketch", sketch);
            w.w("outer", outer);
            w.arr("dir", dir);
            w.f("length", *length);
        }
        Stmt::Revolve { id, sketch, outer, axis_o, axis_d, angle } => {
            w.head("revolve", id);
            w.w("sketch", sketch);
            w.w("outer", outer);
            w.arr("axis_o", axis_o);
            w.arr("axis_d", axis_d);
            w.f("angle", *angle);
        }
        Stmt::Machine(m) => {
            w.head("machine", &m.id);
            w.s("name", &m.name);
            w.w("kind", word_of(&MACHINE_KINDS, m.kind));
            w.w("kinematics", word_of(&KINEMATICS, m.kinematics));
            w.w("units", word_of(&UNITS, m.units));
            w.key("envelope");
            w.bare_box(&m.envelope);
            w.f("safe_z", m.safe_z);
            w.w("power", word_of(&POWER_KINDS, m.power.kind));
            w.f("power_max", m.power.max);
            w.f("ask_above", m.power.ask_above);
            if let Some(t) = m.loaded {
                w.u("loaded", t as u64);
            }
        }
        Stmt::Axis(a) => {
            w.out.push_str("axis");
            w.w("axis", word_of(&AXES, a.id));
            w.f("min", a.min);
            w.f("max", a.max);
            w.f("max_feed", a.max_feed);
            w.f("max_accel", a.max_accel);
            w.f("rapid", a.rapid);
        }
        Stmt::Tool(t) => {
            w.out.push_str("tool");
            w.u("number", t.number as u64);
            w.w("kind", word_of(&TOOL_KINDS, t.kind));
            w.f("dc", t.dc);
            w.f("lf", t.lf);
            w.f("apmx", t.apmx);
            w.u("nof", t.nof as u64);
            w.f("sig", t.sig);
        }
        Stmt::Stock(st) => {
            w.out.push_str("stock");
            match st.shape {
                StockShape::Box { size } => {
                    w.w("shape", "box");
                    w.v3("size", size);
                }
                StockShape::Cylinder { radius, height } => {
                    w.w("shape", "cylinder");
                    w.f("radius", radius);
                    w.f("height", height);
                }
            }
            w.v3("origin", st.origin);
        }
        Stmt::Fixture(fx) => {
            w.out.push_str("fixture");
            w.s("name", &fx.name);
            w.key("keepout");
            w.out.push('[');
            for (i, b) in fx.keepout.iter().enumerate() {
                if i > 0 {
                    w.out.push(' ');
                }
                w.bare_box(b);
            }
            w.out.push(']');
        }
        Stmt::Op { id, on, op } => write_op(w, id, on, op),
        Stmt::Scan { id, path, pose, confidence } => {
            w.head("scan", id);
            w.s("path", path);
            match pose {
                Pose::Identity => w.w("pose", "identity"),
                Pose::Turntable => w.w("pose", "turntable"),
                Pose::File(p) => {
                    w.w("pose", "file");
                    w.s("poses", p);
                }
                Pose::Icp => w.w("pose", "icp"),
            }
            w.f("confidence", *confidence);
        }
    }
}

fn cons(w: &mut W, c: &Cons) {
    match c {
        Cons::Coincident(a, b) => {
            w.pref("a", a);
            w.pref("b", b);
        }
        Cons::PointOnLine(p, l) | Cons::Midpoint(p, l) => {
            w.pref("p", p);
            w.w("line", l);
        }
        Cons::PointOnCircle(p, c) => {
            w.pref("p", p);
            w.w("circle", c);
        }
        Cons::Horizontal(l) | Cons::Vertical(l) => w.w("line", l),
        Cons::Parallel(a, b)
        | Cons::Perpendicular(a, b)
        | Cons::TangentCircles(a, b)
        | Cons::EqualLength(a, b)
        | Cons::EqualRadius(a, b) => {
            w.w("a", a);
            w.w("b", b);
        }
        Cons::TangentLineCircle(l, c) => {
            w.w("line", l);
            w.w("circle", c);
        }
        Cons::Distance(a, b, d) => {
            w.pref("a", a);
            w.pref("b", b);
            w.f("d", *d);
        }
        Cons::DistancePointLine(p, l, d) => {
            w.pref("p", p);
            w.w("line", l);
            w.f("d", *d);
        }
        Cons::Angle(a, b, t) => {
            w.w("a", a);
            w.w("b", b);
            w.f("angle", *t);
        }
        Cons::Radius(c, r) => {
            w.w("circle", c);
            w.f("r", *r);
        }
        Cons::Diameter(c, d) => {
            w.w("circle", c);
            w.f("d", *d);
        }
        Cons::Fix(p, at) => {
            w.pref("p", p);
            w.arr("at", at);
        }
        Cons::Symmetric(a, b, l) => {
            w.pref("a", a);
            w.pref("b", b);
            w.w("line", l);
        }
    }
}

/// One Op statement, verb per variant (the compose site for VUGMAKEPOCKET / VUGMAKESLICE: a new field is a new key).
fn write_op(w: &mut W, id: &str, on: &str, op: &Op) {
    match op {
        Op::Drill { points, top, depth, peck, tool, feeds } => {
            w.head("drill", id);
            w.w("on", on);
            w.pts("points", points);
            w.f("top", *top);
            w.f("depth", *depth);
            w.f("peck", *peck);
            w.u("tool", *tool as u64);
            w.f("feed", feeds.feed);
            w.f("plunge", feeds.plunge);
            w.f("rpm", feeds.rpm);
        }
        Op::Contour { profile, top, depth, side, tool, feeds } => {
            w.head("contour", id);
            w.w("on", on);
            w.pts("profile", profile);
            w.f("top", *top);
            w.f("depth", *depth);
            w.w("side", word_of(&SIDES, *side));
            w.u("tool", *tool as u64);
            w.f("feed", feeds.feed);
            w.f("plunge", feeds.plunge);
            w.f("rpm", feeds.rpm);
        }
        Op::Pocket2_5D { outline, islands, top, depth, stepover, chord, climb, tool, feeds } => {
            w.head("pocket", id);
            w.w("on", on);
            w.pts("outline", outline);
            w.polys("islands", islands);
            w.f("top", *top);
            w.f("depth", *depth);
            w.f("stepover", *stepover);
            w.f("chord", *chord);
            w.w("climb", word_of(&YESNO, *climb));
            w.u("tool", *tool as u64);
            w.f("feed", feeds.feed);
            w.f("plunge", feeds.plunge);
            w.f("rpm", feeds.rpm);
        }
        Op::Face { lo, hi, top, depth, stepover, along, tool, feeds } => {
            w.head("face", id);
            w.w("on", on);
            w.arr("lo", &[lo.x, lo.y]);
            w.arr("hi", &[hi.x, hi.y]);
            w.f("top", *top);
            w.f("depth", *depth);
            w.f("stepover", *stepover);
            w.w("along", word_of(&FACE_DIRS, *along));
            w.u("tool", *tool as u64);
            w.f("feed", feeds.feed);
            w.f("plunge", feeds.plunge);
            w.f("rpm", feeds.rpm);
        }
        Op::Slice { triangles, params } => {
            // VUGMAKESLICE (SR483), composed at the fold: the soup as [[x y z] [x y z] [x y z]] per triangle.
            w.head("slice", id);
            w.w("on", on);
            w.key("triangles");
            w.out.push('[');
            for (i, t) in triangles.iter().enumerate() {
                if i > 0 {
                    w.out.push(' ');
                }
                w.out.push('[');
                for (j, p) in t.iter().enumerate() {
                    if j > 0 {
                        w.out.push(' ');
                    }
                    w.nums(&[p.x, p.y, p.z]);
                }
                w.out.push(']');
            }
            w.out.push(']');
            w.u("tool", params.tool as u64);
            w.f("layer_h", params.layer_h);
            w.f("first_layer", params.first_layer);
            w.f("width", params.width);
            w.u("perimeters", params.perimeters as u64);
            w.f("density", params.density);
            w.u("top_layers", params.top_layers as u64);
            w.u("bottom_layers", params.bottom_layers as u64);
            w.f("filament", params.filament);
            w.f("print_feed", params.print_feed);
            w.f("first_layer_feed", params.first_layer_feed);
            w.f("retract", params.retract);
            w.f("retract_feed", params.retract_feed);
            w.f("retract_min_travel", params.retract_min_travel);
            w.f("z_hop", params.z_hop);
            w.f("hotend_c", params.hotend_c);
            w.f("bed_c", params.bed_c);
        }
        Op::PrintLayers { slices, tool } => {
            w.head("print_layers", id);
            w.w("on", on);
            w.key("slices");
            w.out.push('[');
            for (i, (z, polys)) in slices.iter().enumerate() {
                if i > 0 {
                    w.out.push(' ');
                }
                w.out.push('[');
                w.out.push_str(&f64_text(*z));
                w.out.push(' ');
                w.bare_polys(polys);
                w.out.push(']');
            }
            w.out.push(']');
            w.u("tool", *tool as u64);
        }
        Op::LaserCut { polylines, passes } => {
            w.head("laser_cut", id);
            w.w("on", on);
            w.polys("polylines", polylines);
            w.u("passes", *passes as u64);
        }
    }
}

/// VUGDOCDIFF (SR501): one statement as the writer's text, no header and no LF — the canonical `key=value` form
/// (a fixed key order per verb) that `diff` reads field by field, so the field diff needs no per-variant code.
pub(crate) fn stmt_text(s: &Stmt) -> String {
    let mut w = W { out: String::new() };
    stmt(&mut w, s);
    w.out
}
