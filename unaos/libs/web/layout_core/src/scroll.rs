//! AETHERSTYLE3 (SR103): scroll containers (css-overflow-3 §2-§3). A box whose `overflow` is `hidden`, `scroll`
//! or `auto` on an axis clips its content to its padding box and has its own scroll offset
//! ([`LayoutTree::scroll`], keyed by box; the host carries it across relayouts keyed by element); `scroll` and
//! `auto` are user-scrollable and get scrollbars — `scroll` always, `auto` only when the content overflows
//! (§3.1). The scrollable overflow area (§2.2) is the union of the descendants' margin boxes and line boxes that
//! the box does not itself clip away, plus the end padding; the offset is clamped to [0, overflow - client].
//!
//! Scrollbars are overlay bars (no layout space, the way css-overflow-4 `scrollbar-gutter: auto` lets a UA do
//! it): [`SCROLLBAR`] px inside the padding box's right / bottom edge, a track and a thumb whose length is the
//! visible fraction. A wheel delta goes to the innermost user-scrollable box under the pointer and what it
//! cannot take chains outward (css-overscroll-1 §2 scroll chaining), finally to the viewport; [`scroll_by`]
//! returns what is left. Painters and hit tests shift a scroll container's content by [`content_shift`].
//!
//! AETHERSTYLE3B (SR115): `scrollbar-gutter: stable` reserves the bar's space in layout (`css::SpecifiedStyle`'s
//! taffy fold), so the bar paints in its own gutter; a `scroll-snap-type` container lands every wheel step and
//! every programmatic scroll on a snap position of its `scroll-snap-align` descendants ([`snap_offset`]); a
//! programmatic scroll ([`scroll_target`]) animates as a [`SmoothScroll`] under `scroll-behavior: smooth`, driven
//! per frame by the host.
//!
//! AETHERSTYLE3C (SR122): a snap area is its box's border box outset by `scroll-margin` (css-scroll-snap-1
//! §6.3), and a snap container keeps its snapped TARGET across a relayout, not its offset (§5.5 re-snapping):
//! [`snapped_targets`] names the box each snapped axis sits on, the host records it by element with the offset,
//! and after the relayout [`resnap`] puts the offset on that element's new snap position (the nearest position
//! when it has none, or no longer snaps).

use crate::inline::Frag;
use crate::prelude::*;
use crate::{Host, LayoutTree};
use taffy::NodeId;

/// The overlay scrollbar's thickness in px.
pub const SCROLLBAR: f32 = 10.0;
/// The shortest thumb, so a long page keeps a grabbable thumb.
pub const MIN_THUMB: f32 = 18.0;

/// `overflow` codes ([`crate::PaintStyle::overflow`]).
pub const VISIBLE: u8 = 0;
pub const HIDDEN: u8 = 1;
pub const CLIP: u8 = 2;
pub const SCROLL: u8 = 3;
pub const AUTO: u8 = 4;

/// The computed overflow per axis [x, y] (css-overflow-3 §3.1: `visible` with a scrollable other axis computes
/// to `auto`, `clip` to `hidden`).
pub fn overflow_of<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> [u8; 2] {
    let Some(mut o) = tree.paint_map.get(&id).and_then(|p| p.overflow) else { return [VISIBLE; 2] };
    let scrolly = |v: u8| matches!(v, HIDDEN | SCROLL | AUTO);
    for (a, b) in [(0usize, 1usize), (1, 0)] {
        if scrolly(o[b]) {
            if o[a] == VISIBLE {
                o[a] = AUTO;
            } else if o[a] == CLIP {
                o[a] = HIDDEN;
            }
        }
    }
    o
}

/// A scroll container: either axis `hidden`, `scroll` or `auto`.
pub fn is_scroll_container<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> bool {
    overflow_of(tree, id).iter().any(|v| matches!(*v, HIDDEN | SCROLL | AUTO))
}

/// Which axes the user can scroll (`scroll` / `auto`).
pub fn user_scrollable<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> (bool, bool) {
    let o = overflow_of(tree, id);
    (matches!(o[0], SCROLL | AUTO), matches!(o[1], SCROLL | AUTO))
}

/// The padding box relative to the border box: (x, y, w, h).
pub fn client_rect<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> (f32, f32, f32, f32) {
    let Ok(l) = tree.taffy.layout(id) else { return (0.0, 0.0, 0.0, 0.0) };
    (
        l.border.left,
        l.border.top,
        (l.size.width - l.border.left - l.border.right).max(0.0),
        (l.size.height - l.border.top - l.border.bottom).max(0.0),
    )
}

/// The box's current offset (0, 0 when it is not scrolled or not a scroll container).
pub fn offset<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> (f32, f32) {
    tree.scroll.get(&id).copied().unwrap_or((0.0, 0.0))
}

/// How far a scroll container's content is moved for painting and hit testing: minus its offset.
pub fn content_shift<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> (f32, f32) {
    match tree.scroll.get(&id) {
        Some(&(x, y)) if is_scroll_container(tree, id) => (-x, -y),
        _ => (0.0, 0.0),
    }
}

/// The scrollable overflow extent (w, h) measured from the padding box's origin (css-overflow-3 §2.2).
pub fn scroll_size<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> (f32, f32) {
    let Ok(l) = tree.taffy.layout(id) else { return (0.0, 0.0) };
    let (bl, bt) = (l.border.left, l.border.top);
    let (cx, cy, cw, ch) = client_rect(tree, id);
    let _ = (cx, cy);
    // extents in the border-box space of `id`
    let mut maxx = 0.0f32;
    let mut maxy = 0.0f32;
    fn extend<H: Host>(tree: &LayoutTree<H>, id: NodeId, ox: f32, oy: f32, maxx: &mut f32, maxy: &mut f32) {
        let Ok(l) = tree.taffy.layout(id) else { return };
        if let Some(il) = tree.inline.get(&id) {
            let (cx, cy) = (ox + l.border.left + l.padding.left, oy + l.border.top + l.padding.top);
            for f in &il.frags {
                match f {
                    Frag::Text { x, width, .. } => *maxx = maxx.max(cx + x + width),
                    Frag::Box { x, y, w, h, .. } => {
                        *maxx = maxx.max(cx + x + w);
                        *maxy = maxy.max(cy + y + h);
                    }
                    Frag::Atomic { node, x, y } => {
                        if let Ok(al) = tree.taffy.layout(*node) {
                            let (ax, ay) = (cx + x, cy + y);
                            *maxx = maxx.max(ax + al.size.width + al.margin.right);
                            *maxy = maxy.max(ay + al.size.height + al.margin.bottom);
                            if !clips(tree, *node) {
                                extend(tree, *node, ax, ay, maxx, maxy);
                            }
                        }
                    }
                }
            }
            for &(top, h, _) in &il.lines {
                *maxy = maxy.max(cy + top + h);
            }
            return;
        }
        for k in tree.taffy.children(id).unwrap_or_default() {
            if tree.taffy.style(k).is_ok_and(|s| s.display == taffy::style::Display::None) {
                continue;
            }
            // a fixed box belongs to the viewport, not to this scroller
            if tree.paint_map.get(&k).and_then(|p| p.position_kind) == Some(3) {
                continue;
            }
            let Ok(kl) = tree.taffy.layout(k) else { continue };
            let (kx, ky) = (ox + kl.location.x, oy + kl.location.y);
            *maxx = maxx.max(kx + kl.size.width + kl.margin.right.max(0.0));
            *maxy = maxy.max(ky + kl.size.height + kl.margin.bottom.max(0.0));
            if !clips(tree, k) {
                extend(tree, k, kx, ky, maxx, maxy);
            }
        }
    }
    fn clips<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> bool {
        tree.paint_map.get(&id).and_then(|p| p.clip).unwrap_or(false)
    }
    extend(tree, id, 0.0, 0.0, &mut maxx, &mut maxy);
    // the end padding belongs to the scrollable overflow (css-overflow-3 §2.2)
    let w = (maxx - bl + l.padding.right).max(cw);
    let h = (maxy - bt + l.padding.bottom).max(ch);
    (w, h)
}

/// The largest offset per axis.
pub fn max_scroll<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> (f32, f32) {
    let (_, _, cw, ch) = client_rect(tree, id);
    let (sw, sh) = scroll_size(tree, id);
    ((sw - cw).max(0.0), (sh - ch).max(0.0))
}

/// Sets the offset, clamped to [0, max]; returns the offset applied. A box that is not a scroll container
/// keeps none.
pub fn set_offset<H: Host>(tree: &mut LayoutTree<H>, id: NodeId, x: f32, y: f32) -> (f32, f32) {
    if !is_scroll_container(tree, id) {
        tree.scroll.remove(&id);
        return (0.0, 0.0);
    }
    let (mx, my) = max_scroll(tree, id);
    let v = (x.clamp(0.0, mx), y.clamp(0.0, my));
    if v == (0.0, 0.0) {
        tree.scroll.remove(&id);
    } else {
        tree.scroll.insert(id, v);
    }
    v
}

/// The user-scrollable boxes from `id` outward (innermost first): the scroll chain of a wheel over `id`.
pub fn scroll_chain<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut cur = Some(id);
    while let Some(c) = cur {
        let (ux, uy) = user_scrollable(tree, c);
        if ux || uy {
            out.push(c);
        }
        cur = tree.taffy.parent(c);
    }
    out
}

/// Scrolls along `chain` (innermost first): each box takes what its range and user-scrollable axes allow and
/// passes the rest outward (scroll chaining). Returns the delta no box took (the viewport's).
///
/// AETHERSTYLE3B (SR115): a scroll snap container ([`snap_axes`]) lands on a snap position instead
/// ([`snap_offset`] in the direction of the delta, css-scroll-snap-1 §7); when it moves, it takes that axis's
/// whole delta (the wheel step chose the snap position), and when no snap position lies that way the delta
/// passes outward.
pub fn scroll_by<H: Host>(tree: &mut LayoutTree<H>, chain: &[NodeId], dx: f32, dy: f32) -> (f32, f32) {
    let (mut rx, mut ry) = (dx, dy);
    for &id in chain {
        if rx == 0.0 && ry == 0.0 {
            break;
        }
        let (ux, uy) = user_scrollable(tree, id);
        let (ox, oy) = offset(tree, id);
        let mut want = (if ux { ox + rx } else { ox }, if uy { oy + ry } else { oy });
        let snap = snap_axes(tree, id);
        if snap != (false, false) {
            want = snap_offset(tree, id, (ox, oy), want, (rx, ry));
        }
        let (nx, ny) = set_offset(tree, id, want.0, want.1);
        if ux {
            if snap.0 && (nx - ox).abs() > 0.01 {
                rx = 0.0;
            } else {
                rx -= nx - ox;
            }
        }
        if uy {
            if snap.1 && (ny - oy).abs() > 0.01 {
                ry = 0.0;
            } else {
                ry -= ny - oy;
            }
        }
    }
    (rx, ry)
}

// ---- AETHERSTYLE3B (SR115): scroll snapping (css-scroll-snap-1) --------------------------------------------------

/// The axes (x, y) box `id` snaps on: `scroll-snap-type` on a scroll container.
pub fn snap_axes<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> (bool, bool) {
    if !is_scroll_container(tree, id) {
        return (false, false);
    }
    match tree.paint_map.get(&id).and_then(|p| p.snap_type) {
        Some((a, _)) => (a & 1 != 0, a & 2 != 0),
        None => (false, false),
    }
}

/// AETHERSTYLE3C (SR122): every snap area of `id` — the box, its snap position per axis (None where its
/// `scroll-snap-align` is `none`), clamped to the scroll range, and its `scroll-snap-stop: always` — in tree
/// order: each descendant whose nearest scroll container is `id`, its border box outset by `scroll-margin`
/// (§6.3), aligned start / end / center against the padding box less `scroll-padding` (§6.1).
pub fn snap_areas<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> Vec<(NodeId, Option<f32>, Option<f32>, bool)> {
    let (_, _, cw, ch) = client_rect(tree, id);
    let (mx, my) = max_scroll(tree, id);
    let sp = tree.paint_map.get(&id).and_then(|p| p.scroll_padding).unwrap_or([0.0; 4]);
    let Ok(l) = tree.taffy.layout(id) else { return Vec::new() };
    // areas in the padding-box space of `id` (unscrolled)
    fn walk<H: Host>(tree: &LayoutTree<H>, id: NodeId, ox: f32, oy: f32, out: &mut Vec<(NodeId, f32, f32, f32, f32)>) {
        for k in tree.taffy.children(id).unwrap_or_default() {
            if tree.taffy.style(k).is_ok_and(|s| s.display == taffy::style::Display::None) {
                continue;
            }
            let Ok(kl) = tree.taffy.layout(k) else { continue };
            let cs = crate::col_shift(tree, k);
            let (kx, ky) = (ox + kl.location.x + cs.0, oy + kl.location.y + cs.1);
            out.push((k, kx, ky, kl.size.width, kl.size.height));
            if !is_scroll_container(tree, k) {
                walk(tree, k, kx, ky, out);
            }
        }
    }
    let mut boxes = Vec::new();
    walk(tree, id, -l.border.left, -l.border.top, &mut boxes);
    let mut out = Vec::new();
    for (k, x, y, w, h) in boxes {
        let Some(p) = tree.paint_map.get(&k) else { continue };
        let Some(align) = p.snap_align else { continue };
        let stop = p.snap_stop == Some(true);
        let m = p.scroll_margin.unwrap_or([0.0; 4]);
        let (x, y, w, h) = (x - m[3], y - m[0], w + m[1] + m[3], h + m[0] + m[2]);
        let pos = |a: u8, start: f32, size: f32, port: f32, ps: f32, pe: f32| -> Option<f32> {
            let port_size = (port - ps - pe).max(0.0);
            Some(match a {
                1 => start - ps,
                2 => start + size - ps - port_size,
                3 => start + size / 2.0 - ps - port_size / 2.0,
                _ => return None,
            })
        };
        let px = pos(align[0], x, w, cw, sp[3], sp[1]).map(|v| v.clamp(0.0, mx));
        let py = pos(align[1], y, h, ch, sp[0], sp[2]).map(|v| v.clamp(0.0, my));
        if px.is_some() || py.is_some() {
            out.push((k, px, py, stop));
        }
    }
    out
}

/// The scroll offsets per axis at which a snap area of `id` aligns with its snapport (§6.1): [`snap_areas`]'
/// positions, sorted and deduplicated; each with its `scroll-snap-stop: always`.
pub fn snap_positions<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> (Vec<(f32, bool)>, Vec<(f32, bool)>) {
    let mut xs: Vec<(f32, bool)> = Vec::new();
    let mut ys: Vec<(f32, bool)> = Vec::new();
    for (_, px, py, stop) in snap_areas(tree, id) {
        if let Some(v) = px {
            xs.push((v, stop));
        }
        if let Some(v) = py {
            ys.push((v, stop));
        }
    }
    let tidy = |v: &mut Vec<(f32, bool)>| {
        v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
        v.dedup_by(|a, b| {
            if (a.0 - b.0).abs() < 0.5 {
                b.1 |= a.1;
                true
            } else {
                false
            }
        });
    };
    tidy(&mut xs);
    tidy(&mut ys);
    (xs, ys)
}

/// AETHERSTYLE3C (SR122): the boxes (x, y) whose snap position the current offset of snap container `id` sits
/// on, per snapping axis (the first such area in tree order; None when the axis does not snap or sits between
/// positions).
pub fn snapped_targets<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> (Option<NodeId>, Option<NodeId>) {
    let (sx, sy) = snap_axes(tree, id);
    if !sx && !sy {
        return (None, None);
    }
    let (ox, oy) = offset(tree, id);
    let areas = snap_areas(tree, id);
    let on = |cur: f32, get: &dyn Fn(&(NodeId, Option<f32>, Option<f32>, bool)) -> Option<f32>| {
        areas.iter().find(|a| get(a).is_some_and(|p| (p - cur).abs() < 0.5)).map(|a| a.0)
    };
    (
        if sx { on(ox, &|a| a.1) } else { None },
        if sy { on(oy, &|a| a.2) } else { None },
    )
}

/// AETHERSTYLE3C (SR122): re-snaps snap container `id` after a relayout (§5.5): an axis whose recorded target
/// (`targets`, the boxes the host found for the elements [`snapped_targets`] named before) still has a snap
/// position there goes to that position; any other snapping axis snaps from its current offset to the nearest
/// position (mandatory always, proximity only when near). Returns the offset applied.
pub fn resnap<H: Host>(tree: &mut LayoutTree<H>, id: NodeId, targets: (Option<NodeId>, Option<NodeId>)) -> (f32, f32) {
    let (sx, sy) = snap_axes(tree, id);
    let cur = offset(tree, id);
    if !sx && !sy {
        return cur;
    }
    let areas = snap_areas(tree, id);
    let at = |t: Option<NodeId>, axis: u8| -> Option<f32> {
        let t = t?;
        areas.iter().find(|a| a.0 == t).and_then(|a| if axis == 0 { a.1 } else { a.2 })
    };
    let nearest = snap_offset(tree, id, cur, cur, (0.0, 0.0));
    let x = if sx { at(targets.0, 0).unwrap_or(nearest.0) } else { cur.0 };
    let y = if sy { at(targets.1, 1).unwrap_or(nearest.1) } else { cur.1 };
    set_offset(tree, id, x, y)
}

/// One axis's snapped offset (§7): `dir` > 0 / < 0 picks among the positions past `cur` that way — the nearest
/// to `want`, but never past a `scroll-snap-stop: always` position — and `dir` == 0 the nearest overall.
/// `mandatory` always snaps (no position that way: stay at `cur`); `proximity` only within `near` px of `want`.
fn snap_axis(positions: &[(f32, bool)], cur: f32, want: f32, dir: f32, mandatory: bool, near: f32) -> f32 {
    if positions.is_empty() {
        return want;
    }
    fn nearest<'a>(cands: impl Iterator<Item = &'a (f32, bool)>, want: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        for &(p, _) in cands {
            if best.is_none_or(|b| (p - want).abs() < (b - want).abs()) {
                best = Some(p);
            }
        }
        best
    }
    let found = if dir > 0.0 {
        let ahead: Vec<&(f32, bool)> = positions.iter().filter(|(p, _)| *p > cur + 0.5).collect();
        let stop = ahead.iter().position(|(_, s)| *s).map(|i| i + 1).unwrap_or(ahead.len());
        nearest(ahead[..stop].iter().copied(), want)
    } else if dir < 0.0 {
        let behind: Vec<&(f32, bool)> = positions.iter().filter(|(p, _)| *p < cur - 0.5).rev().collect();
        let stop = behind.iter().position(|(_, s)| *s).map(|i| i + 1).unwrap_or(behind.len());
        nearest(behind[..stop].iter().copied(), want)
    } else {
        nearest(positions.iter(), want)
    };
    match found {
        Some(p) if mandatory || (p - want).abs() <= near => p,
        None if mandatory && dir != 0.0 => cur,
        _ => want,
    }
}

/// The offset a scroll of snap container `id` from `cur` toward `want` lands on, per snapping axis (`dir` is
/// the scroll's direction per axis; 0 = a programmatic or post-layout snap to the nearest position).
pub fn snap_offset<H: Host>(tree: &LayoutTree<H>, id: NodeId, cur: (f32, f32), want: (f32, f32), dir: (f32, f32)) -> (f32, f32) {
    let (sx, sy) = snap_axes(tree, id);
    if !sx && !sy {
        return want;
    }
    let mandatory = tree.paint_map.get(&id).and_then(|p| p.snap_type).is_some_and(|(_, m)| m == 1);
    let (_, _, cw, ch) = client_rect(tree, id);
    let (xs, ys) = snap_positions(tree, id);
    (
        if sx { snap_axis(&xs, cur.0, want.0, dir.0, mandatory, cw * 0.3) } else { want.0 },
        if sy { snap_axis(&ys, cur.1, want.1, dir.1, mandatory, ch * 0.3) } else { want.1 },
    )
}

// ---- AETHERSTYLE3C (SR122): scroll-state container queries (css-conditional-5 §2.3) ---------------------------

/// Which inset edges a `position: sticky` box is stuck to, as the painter places it (css-position-3 §3.4:
/// against the viewport — Aether's one sticky scrollport — scrolled to `vs` and `viewport` big, never leaving
/// its containing block, the parent's content box): `css_core::container` edge bits, 0 when it sits at its
/// normal-flow place or is not sticky.
pub fn sticky_stuck<H: Host>(tree: &LayoutTree<H>, id: NodeId, vs: (f32, f32), viewport: (f32, f32)) -> u8 {
    use css_core::container::{BOTTOM, LEFT, RIGHT, TOP};
    let Some(p) = tree.paint_map.get(&id) else { return 0 };
    if p.position_kind != Some(4) {
        return 0;
    }
    let Some(ins) = p.inset_px else { return 0 };
    let (Some((nx, ny)), Ok(l)) = (box_origin(tree, id, vs), tree.taffy.layout(id)) else { return 0 };
    let (w, h) = (l.size.width, l.size.height);
    let (cb_l, cb_t, cb_r, cb_b) = match tree.taffy.parent(id).and_then(|q| Some((box_origin(tree, q, vs)?, tree.taffy.layout(q).ok()?))) {
        Some(((px, py), pl)) => (
            px + pl.border.left + pl.padding.left,
            py + pl.border.top + pl.padding.top,
            px + pl.size.width - pl.border.right - pl.padding.right,
            py + pl.size.height - pl.border.bottom - pl.padding.bottom,
        ),
        None => (f32::MIN, f32::MIN, f32::MAX, f32::MAX),
    };
    let mut out = 0u8;
    if let Some(t) = ins[0] {
        let target = vs.1 + t;
        if ny < target && target.min(cb_b - h).max(ny) > ny {
            out |= TOP;
        }
    } else if let Some(b) = ins[2] {
        let target = vs.1 + viewport.1 - b - h;
        if ny > target && target.max(cb_t).min(ny) < ny {
            out |= BOTTOM;
        }
    }
    if let Some(lf) = ins[3] {
        let target = vs.0 + lf;
        if nx < target && target.min(cb_r - w).max(nx) > nx {
            out |= LEFT;
        }
    } else if let Some(r) = ins[1] {
        let target = vs.0 + viewport.0 - r - w;
        if nx > target && target.max(cb_l).min(nx) < nx {
            out |= RIGHT;
        }
    }
    out
}

/// The scroll state a `container-type: scroll-state` box offers its `@container scroll-state()` queries:
/// `stuck` from [`sticky_stuck`] (the caller's viewport scroll), `snapped` — the axes on which the box is the
/// snap target its nearest snap container sits on ([`snapped_targets`]), `scrollable` — the directions the box,
/// as a scroll container, can still scroll (offset above 0: toward top / left; below its maximum: toward bottom
/// / right).
pub fn scroll_state<H: Host>(tree: &LayoutTree<H>, id: NodeId, vs: (f32, f32), viewport: (f32, f32)) -> css_core::container::ScrollState {
    use css_core::container::{AXIS_X, AXIS_Y, BOTTOM, LEFT, RIGHT, TOP};
    let mut st = css_core::container::ScrollState::default();
    if let Some(p) = tree.paint_map.get(&id) {
        st.vertical = crate::writing::is_vertical(p.writing_mode.unwrap_or(0));
        st.rtl = p.rtl == Some(true);
    }
    st.stuck = sticky_stuck(tree, id, vs, viewport);
    let mut cur = tree.taffy.parent(id);
    while let Some(c) = cur {
        if is_scroll_container(tree, c) {
            if snap_axes(tree, c) != (false, false) {
                let (tx, ty) = snapped_targets(tree, c);
                if tx == Some(id) {
                    st.snapped |= AXIS_X;
                }
                if ty == Some(id) {
                    st.snapped |= AXIS_Y;
                }
            }
            break;
        }
        cur = tree.taffy.parent(c);
    }
    if is_scroll_container(tree, id) {
        let (ox, oy) = offset(tree, id);
        let (mx, my) = max_scroll(tree, id);
        if oy > 0.5 {
            st.scrollable |= TOP;
        }
        if oy < my - 0.5 {
            st.scrollable |= BOTTOM;
        }
        if ox > 0.5 {
            st.scrollable |= LEFT;
        }
        if ox < mx - 0.5 {
            st.scrollable |= RIGHT;
        }
    }
    st
}

// ---- AETHERSTYLE3B (SR115): smooth scrolling (CSSOM View §5.1 `scroll-behavior`) -------------------------------

/// Whether a programmatic scroll of `id` with behavior `auto` animates (`scroll-behavior: smooth`).
pub fn smooth_behavior<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> bool {
    tree.paint_map.get(&id).and_then(|p| p.scroll_smooth) == Some(true)
}

/// A smooth scroll in progress: from `from` to `to` over `duration` ms from `start` (the frame clock).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmoothScroll {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub start: f64,
    pub duration: f64,
}

impl SmoothScroll {
    /// A smooth scroll whose duration grows with the distance (150 ms .. 450 ms, the range UAs use).
    pub fn new(from: (f32, f32), to: (f32, f32), start: f64) -> Self {
        let d = ((to.0 - from.0).abs()).max((to.1 - from.1).abs()) as f64;
        SmoothScroll { from, to, start, duration: (150.0 + d * 0.3).min(450.0) }
    }

    /// The offset at `now` (an ease-in-out curve) and whether the scroll has arrived.
    pub fn at(&self, now: f64) -> ((f32, f32), bool) {
        let t = if self.duration <= 0.0 { 1.0 } else { ((now - self.start) / self.duration).clamp(0.0, 1.0) };
        if t >= 1.0 {
            return (self.to, true);
        }
        let e = (if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0) * (-2.0 * t + 2.0) * (-2.0 * t + 2.0) / 2.0 }) as f32;
        ((self.from.0 + (self.to.0 - self.from.0) * e, self.from.1 + (self.to.1 - self.from.1) * e), false)
    }
}

/// A programmatic scroll of `id` to (x, y) (CSSOM View `scrollTo` / `scrollTop =`): clamped, snapped to the
/// nearest snap position on a snap container (§7). Returns the target; the caller sets it at once or animates
/// to it with a [`SmoothScroll`].
pub fn scroll_target<H: Host>(tree: &LayoutTree<H>, id: NodeId, x: f32, y: f32) -> (f32, f32) {
    let (mx, my) = max_scroll(tree, id);
    let cur = offset(tree, id);
    let want = (x.clamp(0.0, mx), y.clamp(0.0, my));
    snap_offset(tree, id, cur, want, (0.0, 0.0))
}

/// One scrollbar in the box's border-box space: its track and thumb rectangles (x, y, w, h).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub track: (f32, f32, f32, f32),
    pub thumb: (f32, f32, f32, f32),
}

/// A box's scrollbars: vertical, horizontal.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bars {
    pub v: Option<Bar>,
    pub h: Option<Bar>,
}

/// The scrollbars a box shows: `scroll` always, `auto` when it overflows on that axis (css-overflow-3 §3.1).
pub fn scrollbars<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> Bars {
    let o = overflow_of(tree, id);
    if !matches!(o[0], SCROLL | AUTO) && !matches!(o[1], SCROLL | AUTO) {
        return Bars::default();
    }
    let (cx, cy, cw, ch) = client_rect(tree, id);
    let (sw, sh) = scroll_size(tree, id);
    let (mx, my) = ((sw - cw).max(0.0), (sh - ch).max(0.0));
    let show_v = o[1] == SCROLL || (o[1] == AUTO && my > 0.5);
    let show_h = o[0] == SCROLL || (o[0] == AUTO && mx > 0.5);
    let (ox, oy) = offset(tree, id);
    let mut bars = Bars::default();
    if show_v && cw >= SCROLLBAR && ch > 0.0 {
        let len = if show_h { (ch - SCROLLBAR).max(0.0) } else { ch };
        let thumb = if sh > 0.0 { (len * ch / sh).clamp(MIN_THUMB.min(len), len) } else { len };
        let pos = if my > 0.0 { (len - thumb) * (oy / my) } else { 0.0 };
        let x = cx + cw - SCROLLBAR;
        bars.v = Some(Bar { track: (x, cy, SCROLLBAR, len), thumb: (x + 2.0, cy + pos, SCROLLBAR - 4.0, thumb) });
    }
    if show_h && ch >= SCROLLBAR && cw > 0.0 {
        let len = if show_v { (cw - SCROLLBAR).max(0.0) } else { cw };
        let thumb = if sw > 0.0 { (len * cw / sw).clamp(MIN_THUMB.min(len), len) } else { len };
        let pos = if mx > 0.0 { (len - thumb) * (ox / mx) } else { 0.0 };
        let y = cy + ch - SCROLLBAR;
        bars.h = Some(Bar { track: (cx, y, len, SCROLLBAR), thumb: (cx + pos, y + 2.0, thumb, SCROLLBAR - 4.0) });
    }
    bars
}

/// What a point (border-box local) over a box's scrollbar hits: (axis 0 = horizontal / 1 = vertical, on the
/// thumb, the point's position along the track from the track's start).
pub fn hit_scrollbar<H: Host>(tree: &LayoutTree<H>, id: NodeId, lx: f32, ly: f32) -> Option<(u8, bool, f32)> {
    let bars = scrollbars(tree, id);
    let inside = |r: (f32, f32, f32, f32)| lx >= r.0 && lx < r.0 + r.2 && ly >= r.1 && ly < r.1 + r.3;
    if let Some(b) = bars.v.filter(|b| inside(b.track)) {
        let on = ly >= b.thumb.1 && ly < b.thumb.1 + b.thumb.3;
        return Some((1, on, ly - b.track.1));
    }
    if let Some(b) = bars.h.filter(|b| inside(b.track)) {
        let on = lx >= b.thumb.0 && lx < b.thumb.0 + b.thumb.2;
        return Some((0, on, lx - b.track.0));
    }
    None
}

/// The offset a thumb dragged `delta` px along `axis` from `start` (the offset when the drag began) gives:
/// the thumb's travel maps linearly onto the scroll range.
pub fn drag_offset<H: Host>(tree: &LayoutTree<H>, id: NodeId, axis: u8, start: f32, delta: f32) -> f32 {
    let bars = scrollbars(tree, id);
    let (mx, my) = max_scroll(tree, id);
    let (bar, max) = if axis == 1 { (bars.v, my) } else { (bars.h, mx) };
    let Some(b) = bar else { return start };
    let (len, thumb) = if axis == 1 { (b.track.3, b.thumb.3) } else { (b.track.2, b.thumb.2) };
    let travel = (len - thumb).max(1.0);
    (start + delta * max / travel).clamp(0.0, max)
}

/// The user-scrollable boxes whose padding box contains the document point (x, y), innermost first — the
/// wheel's scroll chain at the pointer. Positions follow the painter: border-box origins down the tree, IFC
/// atomics where their line placed them, every scroll container's content shifted by its offset, fixed boxes
/// against the viewport scroll `vs`.
pub fn scrollers_at<H: Host>(tree: &LayoutTree<H>, x: f32, y: f32, vs: (f32, f32)) -> Vec<NodeId> {
    fn walk<H: Host>(tree: &LayoutTree<H>, id: NodeId, o: (f32, f32), p: (f32, f32), vs: (f32, f32), out: &mut Vec<NodeId>) {
        let Ok(l) = tree.taffy.layout(id) else { return };
        if tree.taffy.style(id).is_ok_and(|s| s.display == taffy::style::Display::None) {
            return;
        }
        let inside = p.0 >= o.0 && p.0 < o.0 + l.size.width && p.1 >= o.1 && p.1 < o.1 + l.size.height;
        let clips = tree.paint_map.get(&id).and_then(|q| q.clip).unwrap_or(false);
        if clips && !inside {
            return;
        }
        let (ux, uy) = user_scrollable(tree, id);
        if (ux || uy) && inside {
            out.push(id);
        }
        let sh = content_shift(tree, id);
        let shifted = (o.0 + sh.0, o.1 + sh.1);
        if let Some(il) = tree.inline.get(&id) {
            let (cx, cy) = (shifted.0 + l.border.left + l.padding.left, shifted.1 + l.border.top + l.padding.top);
            for f in &il.frags {
                if let Frag::Atomic { node, x, y } = f {
                    walk(tree, *node, (cx + x, cy + y), p, vs, out);
                }
            }
            return;
        }
        for k in tree.taffy.children(id).unwrap_or_default() {
            let Ok(kl) = tree.taffy.layout(k) else { continue };
            let cs = crate::col_shift(tree, k);
            let ko = if tree.paint_map.get(&k).and_then(|q| q.position_kind) == Some(3) {
                (vs.0 + kl.location.x, vs.1 + kl.location.y)
            } else {
                (shifted.0 + kl.location.x + cs.0, shifted.1 + kl.location.y + cs.1)
            };
            walk(tree, k, ko, p, vs, out);
        }
    }
    let mut out = Vec::new();
    let root = tree.root_node;
    let o = tree.taffy.layout(root).map(|l| (l.location.x, l.location.y)).unwrap_or((0.0, 0.0));
    walk(tree, root, o, (x, y), vs, &mut out);
    out.reverse();
    out
}

/// The document-space border-box origin of `id`, as [`scrollers_at`] places it (None when it has no box).
pub fn box_origin<H: Host>(tree: &LayoutTree<H>, id: NodeId, vs: (f32, f32)) -> Option<(f32, f32)> {
    let mut chain = vec![id];
    let mut cur = id;
    while let Some(p) = tree.taffy.parent(cur) {
        chain.push(p);
        cur = p;
    }
    chain.reverse();
    let mut o = tree.taffy.layout(chain[0]).map(|l| (l.location.x, l.location.y)).ok()?;
    for w in chain.windows(2) {
        let (p, k) = (w[0], w[1]);
        let kl = tree.taffy.layout(k).ok()?;
        // AETHERFIT (SR189): a fixed box sits at its viewport origin (`LayoutTree::fixed_at`) per axis with an inset.
        if tree.paint_map.get(&k).and_then(|q| q.position_kind) == Some(3) {
            if let Some(&(fx, fy)) = tree.fixed_at.get(&k) {
                o = (fx.map_or(o.0 + kl.location.x, |x| vs.0 + x), fy.map_or(o.1 + kl.location.y, |y| vs.1 + y));
                continue;
            }
        }
        let sh = content_shift(tree, p);
        let cs = crate::col_shift(tree, k);
        // an IFC's atomic: where its line placed it
        if let Some(il) = tree.inline.get(&p) {
            let pl = tree.taffy.layout(p).ok()?;
            if let Some((fx, fy)) = il.frags.iter().find_map(|f| match f {
                Frag::Atomic { node, x, y } if *node == k => Some((*x, *y)),
                _ => None,
            }) {
                o = (o.0 + sh.0 + pl.border.left + pl.padding.left + fx, o.1 + sh.1 + pl.border.top + pl.padding.top + fy);
                continue;
            }
        }
        o = (o.0 + sh.0 + kl.location.x + cs.0, o.1 + sh.1 + kl.location.y + cs.1);
    }
    Some(o)
}

/// Before layout: the in-flow children of a scroll container that is a block container (not an author flex or
/// grid container) keep their content size instead of shrinking to its height — Aether lays blocks out as
/// column flex boxes, and block layout (CSS 2.2 §10.6.3) never shrinks a child to fit, it overflows; the
/// overflow is what the box scrolls.
pub(crate) fn prepare<H: Host>(tree: &mut LayoutTree<H>) {
    let scrollers: Vec<NodeId> = tree
        .paint_map
        .iter()
        .filter(|(_, p)| p.overflow.is_some_and(|o| o.iter().any(|v| matches!(*v, HIDDEN | SCROLL | AUTO))) && p.flex_container != Some(true))
        .map(|(id, _)| *id)
        .collect();
    for id in scrollers {
        if tree.taffy.style(id).is_ok_and(|s| s.display == taffy::style::Display::Grid) {
            continue;
        }
        for k in tree.taffy.children(id).unwrap_or_default() {
            if let Ok(st) = tree.taffy.style(k) {
                if st.flex_shrink != 0.0 {
                    let mut st = st.clone();
                    st.flex_shrink = 0.0;
                    let _ = tree.taffy.set_style(k, st);
                }
            }
        }
    }
}
