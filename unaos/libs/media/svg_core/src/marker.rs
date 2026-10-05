//! Markers (SVG 2 §11.6): `marker-start` on the first vertex, `marker-end` on the last, `marker-mid` on every
//! other vertex of `<path>`, `<line>`, `<polyline>`, `<polygon>`; `orient` = angle | `auto` (bisector of the
//! incoming and outgoing tangents) | `auto-start-reverse`; `markerUnits` strokeWidth/userSpaceOnUse;
//! `refX`/`refY` in viewBox space; `viewBox`/`preserveAspectRatio`; `overflow` clipping of the marker viewport.
//! Marker content is styled from the marker's own ancestors; `context-fill`/`context-stroke` refer to the
//! referencing element's paints.

use crate::fmath::{atan2, hypot};
use crate::geom::{P, Path, Rect, Seg, Transform, parse_par, parse_view_box, view_box_transform};
use crate::raster::{FillRule, Mask, Pixmap, fill_coverage};
use crate::render::{Ctx, Renderer};
use crate::style::{Axis, Style, parse_length};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::f64::consts::PI;

struct Vertex {
    p: P,
    tin: Option<P>,
    tout: Option<P>,
}

fn nz(v: P) -> bool {
    v.0.abs() > 1e-12 || v.1.abs() > 1e-12
}

fn first_nz(c: &[P]) -> Option<P> {
    c.iter().copied().find(|v| nz(*v))
}

fn vertices(path: &Path) -> Vec<Vertex> {
    let mut out: Vec<Vertex> = Vec::new();
    let mut cur = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    let mut start_idx = 0;
    for s in &path.segs {
        match *s {
            Seg::Move(p) => {
                out.push(Vertex { p, tin: None, tout: None });
                cur = p;
                start = p;
                start_idx = out.len() - 1;
            }
            Seg::Line(p) | Seg::Quad(_, p) | Seg::Cubic(_, _, p) => {
                let (t0, t1) = match *s {
                    Seg::Line(_) => {
                        let d = (p.0 - cur.0, p.1 - cur.1);
                        (Some(d), Some(d))
                    }
                    Seg::Quad(a, _) => (first_nz(&[(a.0 - cur.0, a.1 - cur.1), (p.0 - cur.0, p.1 - cur.1)]), first_nz(&[(p.0 - a.0, p.1 - a.1), (p.0 - cur.0, p.1 - cur.1)])),
                    Seg::Cubic(a, b, _) => (
                        first_nz(&[(a.0 - cur.0, a.1 - cur.1), (b.0 - cur.0, b.1 - cur.1), (p.0 - cur.0, p.1 - cur.1)]),
                        first_nz(&[(p.0 - b.0, p.1 - b.1), (p.0 - a.0, p.1 - a.1), (p.0 - cur.0, p.1 - cur.1)]),
                    ),
                    _ => (None, None),
                };
                if let Some(last) = out.last_mut() {
                    if last.tout.is_none() {
                        last.tout = t0;
                    }
                }
                out.push(Vertex { p, tin: t1, tout: None });
                cur = p;
            }
            Seg::Close => {
                let d = (start.0 - cur.0, start.1 - cur.1);
                if nz(d) {
                    if let Some(last) = out.last_mut() {
                        last.tout = Some(d);
                    }
                    out.push(Vertex { p: start, tin: Some(d), tout: None });
                }
                // The closing vertex joins with the subpath's first outgoing direction.
                let first_out = out.get(start_idx).and_then(|v| v.tout);
                let close_in = out.last().and_then(|v| v.tin);
                if let Some(last) = out.last_mut() {
                    last.tout = first_out;
                }
                if let Some(f) = out.get_mut(start_idx) {
                    if f.tin.is_none() {
                        f.tin = close_in;
                    }
                }
                cur = start;
            }
        }
    }
    out
}

fn bisector(v: &Vertex) -> f64 {
    let a = v.tin.map(|t| atan2(t.1, t.0));
    let b = v.tout.map(|t| atan2(t.1, t.0));
    match (a, b) {
        (Some(a), Some(b)) => {
            let mut d = b - a;
            while d > PI {
                d -= 2.0 * PI;
            }
            while d < -PI {
                d += 2.0 * PI;
            }
            a + d / 2.0
        }
        (Some(a), None) => a,
        (None, Some(b)) => b,
        _ => 0.0,
    }
}

pub fn render_markers(r: &mut Renderer, path: &Path, node: usize, ctx: &Ctx, st: &Style, canvas: &mut Pixmap) {
    let name = r.doc.nodes[node].name();
    if !matches!(name, "path" | "line" | "polyline" | "polygon") {
        return;
    }
    if st.marker_start.is_none() && st.marker_mid.is_none() && st.marker_end.is_none() {
        return;
    }
    let vs = vertices(path);
    if vs.is_empty() {
        return;
    }
    let sw = r.stroke_style(st, ctx).map(|s| s.width).unwrap_or(0.0);
    let n = vs.len();
    for (i, v) in vs.iter().enumerate() {
        let (id, is_start) = if i == 0 {
            (st.marker_start.as_ref(), true)
        } else if i == n - 1 {
            (st.marker_end.as_ref(), false)
        } else {
            (st.marker_mid.as_ref(), false)
        };
        let Some(id) = id else { continue };
        let Some(m) = r.doc.by_id(id).filter(|&m| r.doc.nodes[m].is_svg("marker")) else { continue };
        draw_marker(r, m, v, is_start, sw, ctx, st, canvas, path);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_marker(r: &mut Renderer, m: usize, v: &Vertex, is_start: bool, sw: f64, ctx: &Ctx, st: &Style, canvas: &mut Pixmap, path: &Path) {
    let mn = r.doc.nodes[m].clone();
    let mst = r.doc_style(m);
    let lenv = |a: &str, def: f64, axis: Axis| mn.attr(a).and_then(parse_length).map(|l| r.len(l, axis, ctx, &mst)).unwrap_or(def);
    let mw = lenv("markerWidth", 3.0, Axis::X);
    let mh = lenv("markerHeight", 3.0, Axis::Y);
    if !(mw > 0.0 && mh > 0.0) {
        return;
    }
    let ref_x = lenv("refX", 0.0, Axis::X);
    let ref_y = lenv("refY", 0.0, Axis::Y);
    let user_units = mn.attr("markerUnits").map(|u| u.trim() == "userSpaceOnUse").unwrap_or(false);
    let orient = mn.attr("orient").map(|o| o.trim()).unwrap_or("0");
    let angle = match orient {
        "auto" => bisector(v),
        "auto-start-reverse" => {
            let a = bisector(v);
            if is_start { a + PI } else { a }
        }
        o => {
            // <angle>: deg (default), grad, rad, turn.
            let (num, k) = if let Some(n) = o.strip_suffix("deg") {
                (n, PI / 180.0)
            } else if let Some(n) = o.strip_suffix("grad") {
                (n, PI / 200.0)
            } else if let Some(n) = o.strip_suffix("rad") {
                (n, 1.0)
            } else if let Some(n) = o.strip_suffix("turn") {
                (n, 2.0 * PI)
            } else {
                (o, PI / 180.0)
            };
            num.trim().parse::<f64>().map(|d| d * k).unwrap_or(0.0)
        }
    };
    let vb = mn.attr("viewBox").and_then(parse_view_box);
    let par = mn.attr("preserveAspectRatio").map(parse_par).unwrap_or_default();
    let vbt = match vb {
        Some(b) if b.w > 0.0 && b.h > 0.0 => view_box_transform(&b, &par, 0.0, 0.0, mw, mh),
        Some(_) => return,
        None => Transform::IDENTITY,
    };
    let rp = vbt.apply(ref_x, ref_y);
    let scale = if user_units { 1.0 } else { sw };
    if scale <= 0.0 {
        return;
    }
    let (s, c) = crate::fmath::sin_cos(angle);
    let base = ctx
        .ts
        .mul(&Transform::translate(v.p.0, v.p.1))
        .mul(&Transform::new(c, s, -s, c, 0.0, 0.0))
        .mul(&Transform::scale(scale, scale))
        .mul(&Transform::translate(-rp.0, -rp.1));
    let content = base.mul(&vbt);
    if r.depth_guard(m) {
        return;
    }
    let cctx = Ctx {
        ts: content,
        vw: vb.map(|b| b.w).unwrap_or(mw),
        vh: vb.map(|b| b.h).unwrap_or(mh),
        style: mst.clone(),
        ctx_fill: Some(Box::new(st.fill.clone())),
        ctx_stroke: Some(Box::new(st.stroke.clone())),
        ctx_elem: Some(Box::new(crate::render::CtxElem { bbox: path.bbox(), ts: ctx.ts, vw: ctx.vw, vh: ctx.vh, style: st.clone() })),
    };
    let mut layer = Pixmap::new(r.w, r.h);
    let kids: Vec<usize> = r.doc.elements(m).collect();
    for k in kids {
        r.render(k, &cctx, &mut layer);
    }
    r.depth_release();
    // overflow: hidden (the default for markers) clips to the marker viewport.
    let mstyle_ov = Style::compute(&r.doc_style(r.doc.nodes[m].parent.unwrap_or(0)), &r.props[m]);
    if !mstyle_ov.overflow_visible {
        let clip = match vb {
            Some(b) if par.slice || par.align.is_none() => {
                let _ = b;
                Rect::new(0.0, 0.0, mw, mh)
            }
            _ => Rect::new(0.0, 0.0, mw, mh),
        };
        let mut p = Path::new();
        p.move_to(clip.x, clip.y);
        p.line_to(clip.right(), clip.y);
        p.line_to(clip.right(), clip.bottom());
        p.line_to(clip.x, clip.bottom());
        p.close();
        let mut mk = Mask::new(r.w, r.h, 0);
        if let Some(cv) = fill_coverage(&p.transform(&base), FillRule::NonZero, r.w, r.h, true) {
            mk.union_coverage(&cv);
        }
        canvas.draw_pixmap(&layer, 1.0, Some(&mk));
    } else {
        canvas.draw_pixmap(&layer, 1.0, None);
    }
    let _ = hypot(0.0, 0.0);
}
