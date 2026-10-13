//! AETHERSTYLE3 (SR103): multi-column layout (css-multicol-1). A block container with a non-`auto`
//! `column-count` or `column-width` is a multicol container: its content is laid out in ONE column box of the
//! used column width (§3.4's pseudo-algorithm over the content width and `column-gap`, `normal` = 1em), then
//! fragmented into the column boxes, which sit side by side `width + gap` apart.
//!
//! Fragmentation units are the container's in-flow block children, and the LINES of a child that is a plain
//! inline formatting context (no background, border or padding of its own — its box is not painted per
//! column) and of the container itself when it is an IFC root. Balancing (§7.1, `column-fill: balance`, the
//! initial value) finds the shortest column height at which a greedy fill uses at most `count` columns; with
//! `column-fill: auto` and a definite height the columns fill sequentially at that height. A block child moves
//! by [`crate::LayoutTree::col_shift`]; a line's fragments are rewritten in place in its IFC. The container's
//! height becomes the tallest column. `column-rule` paints between columns that hold content ([`Columns`]).

use crate::inline::{Frag, InlineLayout};
use crate::prelude::*;
use crate::{Host, LayoutTree};
use taffy::NodeId;

/// The used column geometry of one multicol container (content-box space).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Columns {
    /// Used column count (§3.4) and how many of them hold content.
    pub count: u32,
    pub filled: u32,
    pub width: f32,
    pub gap: f32,
    /// The column boxes' height (the container's content height).
    pub height: f32,
    /// `column-rule`: (width px, colour, border-style code); None = no rule.
    pub rule: Option<(f32, (u8, u8, u8), u8)>,
}

/// Whether box `id` is a multicol container: a block container whose `column-count` or `column-width` is not
/// `auto` (an author flex or grid container is not one — css-multicol-1 §2).
pub fn is_multicol<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> bool {
    let Some(p) = tree.paint_map.get(&id) else { return false };
    let count = p.col_count.flatten();
    let width = p.col_width.flatten();
    if count.is_none() && width.is_none() {
        return false;
    }
    if p.flex_container == Some(true) || p.display_kind == Some(0) {
        return false;
    }
    !tree.taffy.style(id).is_ok_and(|s| s.display == taffy::style::Display::Grid || s.display == taffy::style::Display::None)
}

/// §3.4: the used (count, width, gap) for a content box `avail` px wide; `font_size` resolves `normal` gaps.
pub fn used_columns<H: Host>(tree: &LayoutTree<H>, id: NodeId, avail: f32, font_size: f32) -> Option<(u32, f32, f32)> {
    let p = tree.paint_map.get(&id)?;
    let gap = p.col_gap.unwrap_or(font_size).max(0.0);
    let count = p.col_count.flatten();
    let width = p.col_width.flatten().map(|w| w.max(0.0));
    let u = avail.max(0.0);
    let (n, w) = match (count, width) {
        (None, None) => return None,
        (Some(n), None) => {
            let n = n.max(1);
            (n, ((u - (n - 1) as f32 * gap) / n as f32).max(0.0))
        }
        (None, Some(cw)) => {
            let n = (((u + gap) / (cw + gap)).floor() as u32).max(1);
            (n, ((u + gap) / n as f32 - gap).max(0.0))
        }
        (Some(c), Some(cw)) => {
            let n = c.max(1).min((((u + gap) / (cw + gap)).floor() as u32).max(1));
            (n, ((u + gap) / n as f32 - gap).max(0.0))
        }
    };
    Some((n, w, gap))
}

/// One fragmentation unit: its top and bottom in the container's content space, and what moves with it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Unit {
    /// A whole block child.
    Box(NodeId),
    /// Line `line` of the IFC rooted at `ifc`.
    Line { ifc: NodeId, line: usize },
}

/// Greedy fill at column height `h`: the column of each unit and each column's top. `None` when it needs more
/// than `n` columns (the last column then takes the rest when `force`).
fn fill(units: &[(f32, f32, Unit)], h: f32, n: u32, force: bool) -> Option<(Vec<u32>, Vec<f32>)> {
    let mut cols = Vec::with_capacity(units.len());
    let mut tops: Vec<f32> = Vec::new();
    let mut k = 0u32;
    let mut has = false;
    for &(top, bottom, _) in units {
        if tops.is_empty() {
            tops.push(top);
        }
        if has && bottom - tops[k as usize] > h + 0.01 {
            if k + 1 >= n {
                if !force {
                    return None;
                }
            } else {
                k += 1;
                tops.push(top);
            }
        }
        has = true;
        cols.push(k);
    }
    Some((cols, tops))
}

/// Balances `units` into `n` columns (or fills at `fixed` height): per unit its (dx, dy), the used height and
/// how many columns hold content.
pub(crate) fn balance(units: &[(f32, f32, Unit)], n: u32, w: f32, gap: f32, fixed: Option<f32>) -> (Vec<(f32, f32)>, f32, u32) {
    if units.is_empty() {
        return (Vec::new(), 0.0, 0);
    }
    let first = units[0].0;
    let total = units.iter().map(|u| u.1).fold(first, f32::max) - first;
    let tallest = units.iter().map(|u| u.1 - u.0).fold(0.0f32, f32::max);
    let h = match fixed {
        Some(h) => h.max(tallest),
        None => {
            let (mut lo, mut hi) = (tallest, total.max(tallest));
            if fill(units, lo, n, false).is_some() {
                hi = lo;
            }
            for _ in 0..24 {
                if hi - lo <= 0.25 {
                    break;
                }
                let mid = (lo + hi) / 2.0;
                if fill(units, mid, n, false).is_some() {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            hi
        }
    };
    let (cols, tops) = fill(units, h, n, true).unwrap_or_default();
    let mut shifts = Vec::with_capacity(units.len());
    let mut used = 0.0f32;
    for (i, &(_, bottom, _)) in units.iter().enumerate() {
        let k = cols[i];
        let top = tops[k as usize];
        shifts.push((k as f32 * (w + gap), first - top));
        used = used.max(bottom - top);
    }
    let filled = cols.last().map(|k| k + 1).unwrap_or(0);
    (shifts, if fixed.is_some() { h } else { used }, filled)
}

/// The line of `il` a fragment belongs to (by its baseline or vertical middle).
fn frag_line(il: &InlineLayout, f: &Frag, heights: &dyn Fn(NodeId) -> f32) -> Option<usize> {
    let y = match f {
        Frag::Text { baseline, .. } => *baseline,
        Frag::Box { y, h, .. } => y + h / 2.0,
        Frag::Atomic { node, y, .. } => y + heights(*node) / 2.0,
    };
    let mut best = None;
    for (i, &(top, h, _)) in il.lines.iter().enumerate() {
        if y >= top - 0.01 && y <= top + h + 0.01 {
            return Some(i);
        }
        if top <= y {
            best = Some(i);
        }
    }
    best.or(if il.lines.is_empty() { None } else { Some(0) })
}

/// Moves each line's fragments of `il` by the per-line shift.
pub(crate) fn shift_lines(il: &mut InlineLayout, per_line: &[(f32, f32)], heights: &dyn Fn(NodeId) -> f32) {
    let lines = il.clone();
    for f in il.frags.iter_mut() {
        let Some(li) = frag_line(&lines, f, heights) else { continue };
        let Some(&(dx, dy)) = per_line.get(li) else { continue };
        match f {
            Frag::Text { x, baseline, .. } => {
                *x += dx;
                *baseline += dy;
            }
            Frag::Box { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
            Frag::Atomic { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
        }
    }
    for (i, l) in il.lines.iter_mut().enumerate() {
        if let Some(&(_, dy)) = per_line.get(i) {
            l.0 += dy;
            l.2 += dy;
        }
    }
    if let (Some(&(_, dy)), Some(b)) = (per_line.last(), il.last_baseline.as_mut()) {
        *b += dy;
    }
}

/// css-multicol-1 on an IFC root that is itself the multicol container: its lines (laid out at the column
/// width) fragment into the columns. Returns the column geometry; `il.height` becomes the column height.
pub(crate) fn balance_ifc<H: Host>(tree: &LayoutTree<H>, root: NodeId, il: &mut InlineLayout, n: u32, w: f32, gap: f32) -> Columns {
    let units: Vec<(f32, f32, Unit)> =
        il.lines.iter().enumerate().map(|(i, &(t, h, _))| (t, t + h, Unit::Line { ifc: root, line: i })).collect();
    let fixed = fixed_height(tree, root);
    let (shifts, h, filled) = balance(&units, n, w, gap, fixed);
    let heights = |id: NodeId| tree.taffy.layout(id).map(|l| l.size.height).unwrap_or(0.0);
    shift_lines(il, &shifts, &heights);
    il.height = h;
    Columns { count: n, filled, width: w, gap, height: h, rule: rule_of(tree, root) }
}

/// The content height `column-fill: auto` fills at (a definite `height`), else None (balance).
pub(crate) fn fixed_height<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> Option<f32> {
    let p = tree.paint_map.get(&id)?;
    if p.col_fill != Some(1) {
        return None;
    }
    let st = tree.taffy.style(id).ok()?;
    let raw = st.size.height.into_raw();
    if raw.tag() != taffy::style::CompactLength::LENGTH_TAG {
        return None;
    }
    let l = tree.taffy.layout(id).ok()?;
    let edges = l.padding.top + l.padding.bottom + l.border.top + l.border.bottom;
    Some(if st.box_sizing == taffy::style::BoxSizing::BorderBox { raw.value() - edges } else { raw.value() }.max(0.0))
}

/// `column-rule` as painted: (width, colour, style); None for `none` / `hidden` (the initial style) or a zero
/// width. The width defaults to `medium` (3px), the colour to `currentcolor`.
pub(crate) fn rule_of<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> Option<(f32, (u8, u8, u8), u8)> {
    let p = tree.paint_map.get(&id)?;
    let style = p.col_rule_style?;
    if style == RULE_NONE {
        return None;
    }
    let w = p.col_rule_width.unwrap_or(3.0);
    (w > 0.0).then(|| (w, p.col_rule_color.or(p.color).unwrap_or((0, 0, 0)), style))
}

/// `column-rule-style: none | hidden`.
pub const RULE_NONE: u8 = 255;

/// A child whose lines may fragment one by one: an IFC root with no paint of its own around the lines.
fn splittable<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> bool {
    if !tree.inline.contains_key(&id) {
        return false;
    }
    let Some(p) = tree.paint_map.get(&id) else { return true };
    let Ok(l) = tree.taffy.layout(id) else { return false };
    p.background.is_none()
        && p.bg_image.is_none()
        && p.bg_gradient.as_ref().is_none_or(|g| g.is_none())
        && p.shadows.as_ref().is_none_or(|s| s.is_empty())
        && l.border.top + l.border.bottom + l.border.left + l.border.right == 0.0
        && l.padding.top + l.padding.bottom == 0.0
        && p.clip != Some(true)
}

/// The in-flow block children of a multicol container (out-of-flow and `display: none` boxes stay put).
pub(crate) fn flow_children<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> Vec<NodeId> {
    tree.taffy
        .children(id)
        .unwrap_or_default()
        .into_iter()
        .filter(|k| {
            !tree.taffy.style(*k).is_ok_and(|s| s.display == taffy::style::Display::None)
                && !matches!(tree.paint_map.get(k).and_then(|p| p.position_kind), Some(2 | 3))
                && tree.paint_map.get(k).and_then(|p| p.float).unwrap_or(0) == 0
        })
        .collect()
}

/// css-multicol-1 on a block multicol container whose children are already laid out at the column width:
/// fragments them (whole boxes, or the lines of a splittable IFC child) into the columns, records the boxes'
/// [`LayoutTree::col_shift`] and rewrites the split lines. Returns the column geometry.
pub(crate) fn balance_blocks<H: Host>(tree: &mut LayoutTree<H>, id: NodeId, n: u32, w: f32, gap: f32) -> Columns {
    let Ok(cl) = tree.taffy.layout(id).cloned() else { return Columns::default() };
    let content_top = cl.border.top + cl.padding.top;
    let mut units: Vec<(f32, f32, Unit)> = Vec::new();
    for k in flow_children(tree, id) {
        let Ok(kl) = tree.taffy.layout(k).cloned() else { continue };
        let top = kl.location.y - kl.margin.top.max(0.0) - content_top;
        let bottom = kl.location.y + kl.size.height + kl.margin.bottom.max(0.0) - content_top;
        if splittable(tree, k) {
            let il = &tree.inline[&k];
            let dy = kl.location.y - content_top + kl.border.top + kl.padding.top;
            if il.lines.is_empty() {
                units.push((top, bottom, Unit::Box(k)));
            }
            for (i, &(t, h, _)) in il.lines.iter().enumerate() {
                units.push((dy + t, dy + t + h, Unit::Line { ifc: k, line: i }));
            }
        } else {
            units.push((top, bottom, Unit::Box(k)));
        }
    }
    let fixed = fixed_height(tree, id);
    let (shifts, h, filled) = balance(&units, n, w, gap, fixed);
    // per IFC child: the shift of each of its lines
    let mut per_ifc: Vec<(NodeId, Vec<(f32, f32)>)> = Vec::new();
    for (u, s) in units.iter().zip(shifts.iter()) {
        match u.2 {
            Unit::Box(k) => {
                if *s != (0.0, 0.0) {
                    tree.col_shift.insert(k, *s);
                }
            }
            Unit::Line { ifc, line, .. } => {
                let e = match per_ifc.iter_mut().find(|(i, _)| *i == ifc) {
                    Some(e) => e,
                    None => {
                        per_ifc.push((ifc, Vec::new()));
                        per_ifc.last_mut().unwrap()
                    }
                };
                if e.1.len() <= line {
                    e.1.resize(line + 1, (0.0, 0.0));
                }
                e.1[line] = *s;
            }
        }
    }
    let heights: Vec<(NodeId, f32)> = tree.taffy.children(id).unwrap_or_default().iter().flat_map(|k| {
        let mut v = Vec::new();
        fn walk<H: Host>(tree: &LayoutTree<H>, id: NodeId, v: &mut Vec<(NodeId, f32)>) {
            if let Ok(l) = tree.taffy.layout(id) {
                v.push((id, l.size.height));
            }
            for k in tree.taffy.children(id).unwrap_or_default() {
                walk(tree, k, v);
            }
        }
        walk(tree, *k, &mut v);
        v
    }).collect();
    let hf = |nid: NodeId| heights.iter().find(|(i, _)| *i == nid).map(|(_, h)| *h).unwrap_or(0.0);
    for (ifc, per_line) in per_ifc {
        if let Some(il) = tree.inline.get_mut(&ifc) {
            shift_lines(il, &per_line, &hf);
        }
    }
    Columns { count: n, filled, width: w, gap, height: h, rule: rule_of(tree, id) }
}
