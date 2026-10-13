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
pub fn scroll_by<H: Host>(tree: &mut LayoutTree<H>, chain: &[NodeId], dx: f32, dy: f32) -> (f32, f32) {
    let (mut rx, mut ry) = (dx, dy);
    for &id in chain {
        if rx == 0.0 && ry == 0.0 {
            break;
        }
        let (ux, uy) = user_scrollable(tree, id);
        let (ox, oy) = offset(tree, id);
        let want = (if ux { ox + rx } else { ox }, if uy { oy + ry } else { oy });
        let (nx, ny) = set_offset(tree, id, want.0, want.1);
        if ux {
            rx -= nx - ox;
        }
        if uy {
            ry -= ny - oy;
        }
    }
    (rx, ry)
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
        if tree.paint_map.get(&k).and_then(|q| q.position_kind) == Some(3) {
            o = (vs.0 + kl.location.x, vs.1 + kl.location.y);
            continue;
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
