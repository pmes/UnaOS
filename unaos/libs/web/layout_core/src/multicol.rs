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
//!
//! AETHERSTYLE3B (SR115): a `column-span: all` child of the container splits the flow (§6.1): the content
//! before it balances into one column set, the spanner sits below that set at the container's full width,
//! and the content after it balances into the next set ([`ColumnSet`], [`crate::LayoutTree::col_sets`]). A
//! block child with no painted box of its own (no background, border, padding, shadow, clip, filter or
//! transform; not a scroll, flex, grid or multicol container) is not a unit itself: its in-flow children (and
//! the lines of its plain IFC children) are, recursively — so a tall wrapper's content splits across columns
//! (§8 fragmentation, `break-inside: auto`).
//!
//! AETHERSTYLE3C (SR122): a decorated block (background, border, padding or shadow; otherwise plain — see
//! `fragmentable`) or decorated IFC root fragments too: its children (or lines) are units it owns, its own
//! block-start edge travelling with the first and its block-end edge with the last. When they land in more than
//! one column the box is painted per fragment ([`BoxFragment`], [`crate::LayoutTree::col_frags`], css-break-3
//! §5.4): `box-decoration-break: slice` (initial) slices the one box, `clone` gives every fragment the box's
//! full edges — the balancer then reserves the cloned block-start edges above a column's first unit and the
//! block-end edges below its last (`fill`'s pads). When they all land in one column the box moves whole as
//! before. A `column-span: all` box splits the flow at any depth of transparent wrappers (§6.1; the wrapper is
//! not narrowed to the column width, its other children are — [`narrow_targets`]); inside a decorated box it
//! stays a unit.

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
/// than `n` columns (the last column then takes the rest when `force`). AETHERSTYLE3C (SR122): `pads[i]` is what
/// unit `i` adds when a column starts at it (`.0`, the cloned block-start edges of the decorated box it continues)
/// and when a column ends after it (`.1`, the cloned block-end edges); empty = none.
fn fill(units: &[(f32, f32, Unit)], pads: &[(f32, f32)], h: f32, n: u32, force: bool) -> Option<(Vec<u32>, Vec<f32>)> {
    let mut cols = Vec::with_capacity(units.len());
    let mut tops: Vec<f32> = Vec::new();
    let mut k = 0u32;
    let mut has = false;
    for (i, &(top, bottom, _)) in units.iter().enumerate() {
        let (pre, post) = pads.get(i).copied().unwrap_or((0.0, 0.0));
        if tops.is_empty() {
            tops.push(top);
        }
        if has && bottom + post - tops[k as usize] > h + 0.01 {
            if k + 1 >= n {
                if !force {
                    return None;
                }
            } else {
                k += 1;
                tops.push(top - pre);
            }
        }
        has = true;
        cols.push(k);
    }
    Some((cols, tops))
}

/// Balances `units` into `n` columns (or fills at `fixed` height): per unit its (dx, dy), the used height and
/// how many columns hold content. `pads` as in [`fill`].
pub(crate) fn balance(units: &[(f32, f32, Unit)], pads: &[(f32, f32)], n: u32, w: f32, gap: f32, fixed: Option<f32>) -> (Vec<(f32, f32)>, f32, u32) {
    if units.is_empty() {
        return (Vec::new(), 0.0, 0);
    }
    let first = units[0].0;
    let total = units.iter().map(|u| u.1).fold(first, f32::max) - first;
    let tallest = units.iter().map(|u| u.1 - u.0).fold(0.0f32, f32::max);
    let pad_max = pads.iter().map(|p| p.0 + p.1).fold(0.0f32, f32::max);
    let h = match fixed {
        Some(h) => h.max(tallest),
        None => {
            let (mut lo, mut hi) = (tallest, total.max(tallest) + pad_max);
            if fill(units, pads, lo, n, false).is_some() {
                hi = lo;
            }
            for _ in 0..24 {
                if hi - lo <= 0.25 {
                    break;
                }
                let mid = (lo + hi) / 2.0;
                if fill(units, pads, mid, n, false).is_some() {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            hi
        }
    };
    let (cols, tops) = fill(units, pads, h, n, true).unwrap_or_default();
    let mut shifts = Vec::with_capacity(units.len());
    let mut used = 0.0f32;
    for (i, &(_, bottom, _)) in units.iter().enumerate() {
        let k = cols[i];
        let top = tops[k as usize];
        shifts.push((k as f32 * (w + gap), first - top));
        // the cloned block-end edges close a column that ends at this unit
        let ends = cols.get(i + 1).is_none_or(|n| *n != k);
        let post = if ends { pads.get(i).map(|p| p.1).unwrap_or(0.0) } else { 0.0 };
        used = used.max(bottom + post - top);
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
    let (shifts, h, filled) = balance(&units, &[], n, w, gap, fixed);
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

/// AETHERSTYLE3B (SR115): one column set of a container split by spanners: its top and height in the
/// container's content space and how many columns hold content.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColumnSet {
    pub top: f32,
    pub height: f32,
    pub filled: u32,
}

/// A `column-span: all` box.
pub fn is_spanner<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> bool {
    tree.paint_map.get(&id).and_then(|p| p.col_span) == Some(true)
}

/// A block box whose in-flow content fragments in its place: it paints nothing of its own (see the module
/// notes) and is a plain block container with in-flow children.
fn transparent<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> bool {
    plain_block(tree, id) && !decorated(tree, id) && !flow_children(tree, id).is_empty()
}

/// A block container whose content may fragment: not an IFC root, multicol, scroll, flex or positioned box, and
/// no clip, filter or transform (those paint the box as one layer).
fn plain_block<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> bool {
    if tree.inline.contains_key(&id) || is_multicol(tree, id) || crate::scroll::is_scroll_container(tree, id) {
        return false;
    }
    let Ok(st) = tree.taffy.style(id) else { return false };
    if st.display != taffy::style::Display::Flex {
        return false;
    }
    if tree.node_map.get(&id).and_then(|n| crate::DomNode::tag(n)).is_some_and(|t| crate::tree::is_table_part(&t)) {
        return false;
    }
    match tree.paint_map.get(&id) {
        Some(p) => {
            p.flex_container != Some(true)
                && p.clip != Some(true)
                && !p.filter.as_ref().is_some_and(|f| f.is_some())
                && !p.transform.as_ref().is_some_and(|t| !t.is_empty())
                && !matches!(p.position_kind, Some(1..))
        }
        None => true,
    }
}

/// The box paints something of its own: a background, a border, padding or a shadow.
fn decorated<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> bool {
    let Ok(l) = tree.taffy.layout(id) else { return false };
    let painted = tree.paint_map.get(&id).is_some_and(|p| {
        p.background.is_some()
            || p.bg_image.is_some()
            || p.bg_gradient.as_ref().is_some_and(|g| g.is_some())
            || p.shadows.as_ref().is_some_and(|s| !s.is_empty())
    });
    painted
        || l.border.top + l.border.bottom + l.border.left + l.border.right != 0.0
        || l.padding.top + l.padding.bottom + l.padding.left + l.padding.right != 0.0
}

/// AETHERSTYLE3C (SR122): a decorated box whose content fragments (css-break-3 §5.4 `box-decoration-break`):
/// a plain block container with in-flow children, or an IFC root (its lines), that paints a box of its own.
fn fragmentable<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> bool {
    if !decorated(tree, id) {
        return false;
    }
    if let Some(il) = tree.inline.get(&id) {
        let Some(p) = tree.paint_map.get(&id) else { return !il.lines.is_empty() };
        return !il.lines.is_empty()
            && il.columns.is_none()
            && il.vertical == 0
            && !crate::scroll::is_scroll_container(tree, id)
            && p.clip != Some(true)
            && p.flex_container != Some(true)
            && !p.filter.as_ref().is_some_and(|f| f.is_some())
            && !p.transform.as_ref().is_some_and(|t| !t.is_empty())
            && !matches!(p.position_kind, Some(1..))
            && !matches!(p.display_kind, Some(0 | 2));
    }
    plain_block(tree, id) && !flow_children(tree, id).is_empty()
}

/// AETHERSTYLE3C (SR122): one column fragment of a decorated box split across columns (css-break-3 §5.4). In
/// the box's own border-box space (unshifted): the piece's `top` and `height` (x and width are the box's);
/// `shift` moves the piece to its column like [`crate::LayoutTree::col_shift`]; `edges` say whether the piece
/// draws the box's block-start / block-end border and padding. Under `slice` (the initial value) the pieces are
/// slices of the one box laid end to end — its background positioned against the whole box, the block-start
/// edges on the first piece only, the block-end edges on the last; under `clone` (`cloned`) every piece is a box
/// of its own with all four edges, its background positioned against the piece.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxFragment {
    pub shift: (f32, f32),
    pub top: f32,
    pub height: f32,
    pub edges: (bool, bool),
    pub cloned: bool,
}

/// A decorated box whose units fragment: its border box's top and height in the container's content space,
/// its block-start / block-end edges (border + padding) and its `box-decoration-break`.
struct Deco {
    id: NodeId,
    top: f32,
    height: f32,
    start: f32,
    end: f32,
    cloned: bool,
}

/// One fragmentation unit while the flow is collected: (top, bottom, what moves), the clone pads ([`fill`])
/// and the decorated box it belongs to.
struct U {
    top: f32,
    bottom: f32,
    unit: Unit,
    pad: (f32, f32),
    owner: Option<usize>,
}

/// The flow of a multicol container as column sets split by spanners.
enum Item {
    Set(Vec<U>),
    Spanner(NodeId),
}

/// What the flow collection fills in.
struct Flow {
    items: Vec<Item>,
    decos: Vec<Deco>,
    /// The bottom of the last thing collected (the container's single-column content end).
    end: f32,
}

impl Flow {
    fn set(&mut self) -> &mut Vec<U> {
        if !matches!(self.items.last(), Some(Item::Set(_))) {
            self.items.push(Item::Set(Vec::new()));
        }
        match self.items.last_mut() {
            Some(Item::Set(v)) => v,
            _ => unreachable!(),
        }
    }
    fn push(&mut self, top: f32, bottom: f32, unit: Unit) {
        self.end = self.end.max(bottom);
        self.set().push(U { top, bottom, unit, pad: (0.0, 0.0), owner: None });
    }
}

/// The in-flow children of `parent` (whose border-box top is `base_y` in the container's content space) into
/// the flow: AETHERSTYLE3C (SR122) a `column-span: all` box at any depth of transparent wrappers splits the flow
/// (css-multicol-1 §6.1: the spanner is in the container's formatting context), unless it sits inside a
/// decorated box (`in_deco`), where it stays a unit.
fn collect_flow<H: Host>(tree: &LayoutTree<H>, parent: NodeId, base_y: f32, depth: u32, in_deco: bool, flow: &mut Flow) {
    for k in flow_children(tree, parent) {
        if !in_deco && is_spanner(tree, k) {
            if let Ok(kl) = tree.taffy.layout(k) {
                flow.end = flow.end.max(base_y + kl.location.y + kl.size.height + kl.margin.bottom.max(0.0));
            }
            flow.items.push(Item::Spanner(k));
            flow.items.push(Item::Set(Vec::new()));
            continue;
        }
        collect_child(tree, k, base_y, depth, in_deco, flow);
    }
}

/// The fragmentation units of one in-flow child `k` whose parent's border box's top is `base_y` in the
/// container's content space: a whole box, the lines of a splittable IFC, (recursively) a transparent block's
/// own units, or (AETHERSTYLE3C) a decorated box's content as units it owns ([`Deco`]).
fn collect_child<H: Host>(tree: &LayoutTree<H>, k: NodeId, base_y: f32, depth: u32, in_deco: bool, flow: &mut Flow) {
    let Ok(kl) = tree.taffy.layout(k).cloned() else { return };
    let top = base_y + kl.location.y - kl.margin.top.max(0.0);
    let bottom = base_y + kl.location.y + kl.size.height + kl.margin.bottom.max(0.0);
    if splittable(tree, k) {
        let il = &tree.inline[&k];
        let dy = base_y + kl.location.y + kl.border.top + kl.padding.top;
        if il.lines.is_empty() {
            flow.push(top, bottom, Unit::Box(k));
        }
        for (i, &(t, h, _)) in il.lines.iter().enumerate() {
            flow.push(dy + t, dy + t + h, Unit::Line { ifc: k, line: i });
        }
    } else if depth < 8 && transparent(tree, k) {
        collect_flow(tree, k, base_y + kl.location.y, depth + 1, in_deco, flow);
    } else if depth < 8 && !in_deco && fragmentable(tree, k) {
        let first = flow.set().len();
        if let Some(il) = tree.inline.get(&k) {
            let dy = base_y + kl.location.y + kl.border.top + kl.padding.top;
            for (i, &(t, h, _)) in il.lines.iter().enumerate() {
                flow.push(dy + t, dy + t + h, Unit::Line { ifc: k, line: i });
            }
        } else {
            collect_flow(tree, k, base_y + kl.location.y, depth + 1, true, flow);
        }
        let cloned = tree.paint_map.get(&k).and_then(|p| p.deco_clone) == Some(true);
        let start = kl.border.top + kl.padding.top;
        let end = kl.border.bottom + kl.padding.bottom;
        let di = flow.decos.len();
        // the decorated box's content stays in one set (no spanner inside it), so its units are a run
        let units = flow.set();
        if units.len() <= first {
            flow.push(top, bottom, Unit::Box(k));
            return;
        }
        let last = units.len() - 1;
        // the box's own block-start edge (and margin) travels with its first unit, its block-end with its last
        units[first].top = top;
        units[last].bottom = bottom;
        for (i, u) in units.iter_mut().enumerate().skip(first) {
            u.owner = Some(di);
            if cloned {
                u.pad = (if i > first { start } else { 0.0 }, if i < last { end } else { 0.0 });
            }
        }
        flow.end = flow.end.max(bottom);
        flow.decos.push(Deco { id: k, top: base_y + kl.location.y, height: kl.size.height, start, end, cloned });
    } else {
        flow.push(top, bottom, Unit::Box(k));
    }
}

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

/// A transparent wrapper with a spanner somewhere inside it (through more transparent wrappers).
fn holds_spanner<H: Host>(tree: &LayoutTree<H>, id: NodeId, depth: u32) -> bool {
    depth < 8
        && transparent(tree, id)
        && flow_children(tree, id).into_iter().any(|k| is_spanner(tree, k) || holds_spanner(tree, k, depth + 1))
}

/// AETHERSTYLE3C (SR122): the boxes of multicol container `id` the single-column pass narrows to the column
/// width: its in-flow children, except spanners (they keep the full width) and wrappers that hold one (their
/// own children are narrowed instead, recursively, so the spanner inside stays full-width).
pub(crate) fn narrow_targets<H: Host>(tree: &LayoutTree<H>, id: NodeId) -> Vec<NodeId> {
    fn walk<H: Host>(tree: &LayoutTree<H>, id: NodeId, depth: u32, out: &mut Vec<NodeId>) {
        for k in flow_children(tree, id) {
            if is_spanner(tree, k) {
                continue;
            }
            if holds_spanner(tree, k, depth) {
                walk(tree, k, depth + 1, out);
            } else {
                out.push(k);
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, id, 0, &mut out);
    out
}

/// css-multicol-1 on a block multicol container whose children are already laid out at the column width:
/// fragments them (whole boxes, the lines of a splittable IFC child, the content of a decorated box) into the
/// columns, records the boxes' [`LayoutTree::col_shift`], the decorated boxes' [`LayoutTree::col_frags`] and
/// rewrites the split lines. Returns the column geometry.
pub(crate) fn balance_blocks<H: Host>(tree: &mut LayoutTree<H>, id: NodeId, n: u32, w: f32, gap: f32) -> Columns {
    let Ok(cl) = tree.taffy.layout(id).cloned() else { return Columns::default() };
    let content_top = cl.border.top + cl.padding.top;
    // AETHERSTYLE3B (SR115): the flow splits at each `column-span: all` child into column sets;
    // AETHERSTYLE3C (SR122): at any depth of transparent wrappers.
    let mut flow = Flow { items: vec![Item::Set(Vec::new())], decos: Vec::new(), end: 0.0 };
    collect_flow(tree, id, -content_top, 0, false, &mut flow);
    let Flow { items, decos, end: content_end } = flow;
    let split = items.iter().any(|i| matches!(i, Item::Spanner(_)));
    let fixed = if split { None } else { fixed_height(tree, id) };
    let mut units: Vec<U> = Vec::new();
    let mut shifts: Vec<(f32, f32)> = Vec::new();
    let mut sets: Vec<ColumnSet> = Vec::new();
    let (mut h, mut filled) = (0.0f32, 0u32);
    let mut acc = 0.0f32; // how far everything after the sets so far has moved up
    for item in items {
        match item {
            Item::Set(seg) => {
                if seg.is_empty() {
                    continue;
                }
                let plain: Vec<(f32, f32, Unit)> = seg.iter().map(|u| (u.top, u.bottom, u.unit)).collect();
                let pads: Vec<(f32, f32)> = seg.iter().map(|u| u.pad).collect();
                let (s, used, f) = balance(&plain, &pads, n, w, gap, fixed);
                let first = seg[0].top;
                let end = seg.iter().map(|u| u.bottom).fold(first, f32::max);
                sets.push(ColumnSet { top: first + acc, height: used, filled: f });
                h = h.max(used);
                filled = filled.max(f);
                shifts.extend(s.into_iter().map(|(dx, dy)| (dx, dy + acc)));
                units.extend(seg);
                acc += first + used - end;
            }
            Item::Spanner(k) => {
                if acc != 0.0 {
                    tree.col_shift.insert(k, (0.0, acc));
                }
            }
        }
    }
    if split {
        h = (content_end + acc).max(0.0);
        tree.col_sets.insert(id, sets);
    }
    // AETHERSTYLE3C (SR122): a decorated box whose units all landed in one place moves whole (its shift is its
    // own, its content's are dropped); one split across columns is painted per fragment.
    for (di, d) in decos.iter().enumerate() {
        let idx: Vec<usize> = (0..units.len()).filter(|&i| units[i].owner == Some(di)).collect();
        let Some(&i0) = idx.first() else { continue };
        if idx.iter().all(|&i| shifts[i] == shifts[i0]) {
            if shifts[i0] != (0.0, 0.0) {
                tree.col_shift.insert(d.id, shifts[i0]);
            }
            for &i in &idx {
                shifts[i] = (0.0, 0.0);
            }
            continue;
        }
        let mut groups: Vec<(usize, usize)> = Vec::new(); // (first unit, last unit) per fragment
        for &i in &idx {
            match groups.last_mut() {
                Some(g) if shifts[g.0] == shifts[i] => g.1 = i,
                _ => groups.push((i, i)),
            }
        }
        let count = groups.len();
        let mut frags = Vec::with_capacity(count);
        for (g, &(a, b)) in groups.iter().enumerate() {
            let (first, last) = (g == 0, g + 1 == count);
            let (top, bottom) = if d.cloned {
                (
                    if first { 0.0 } else { units[a].top - d.top - d.start },
                    if last { d.height } else { units[b].bottom - d.top + d.end },
                )
            } else {
                (if first { 0.0 } else { units[a].top - d.top }, if last { d.height } else { units[groups[g + 1].0].top - d.top })
            };
            frags.push(BoxFragment {
                shift: shifts[a],
                top,
                height: (bottom - top).max(0.0),
                edges: if d.cloned { (true, true) } else { (first, last) },
                cloned: d.cloned,
            });
        }
        tree.col_frags.insert(d.id, frags);
    }
    // per IFC child: the shift of each of its lines
    let mut per_ifc: Vec<(NodeId, Vec<(f32, f32)>)> = Vec::new();
    for (u, s) in units.iter().zip(shifts.iter()) {
        match u.unit {
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
        if per_line.iter().all(|s| *s == (0.0, 0.0)) {
            continue;
        }
        if let Some(il) = tree.inline.get_mut(&ifc) {
            shift_lines(il, &per_line, &hf);
        }
    }
    Columns { count: n, filled, width: w, gap, height: h, rule: rule_of(tree, id) }
}
