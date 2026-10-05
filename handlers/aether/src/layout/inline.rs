//! The inline formatting context (CSS 2.2 §9.4.2, css-inline-3 §2-4).
//!
//! A block container whose in-flow children are all inline-level lays its
//! content out in LINE BOXES. This module builds them from the box tree the
//! builder made (taffy holds the boxes; their flex geometry is ignored for
//! inline content):
//!
//! - text runs break across item boundaries: a run continues on the line the
//!   previous item ended on (`fonts::lines::break_lines_from`), so text after a
//!   wrapped run carries on from that run's last line;
//! - non-atomic inline boxes (`span`, `a`, `b`, ...) are split into one
//!   FRAGMENT per line they touch; with `box-decoration-break: slice` (the
//!   initial value) the inline-start margin/border/padding belong to the first
//!   fragment and the inline-end ones to the last (css-break-3 §5.4);
//! - atomic inlines (inline-blocks, replaced elements, form controls) are
//!   placed whole, with a soft wrap opportunity before and after each one
//!   (css-text-3 §5.1);
//! - each line box is as tall as the boxes on it require (§10.8): every inline
//!   box (and the root's strut) contributes `line-height` split by half-leading
//!   around its baseline, shifted by its `vertical-align` (§10.8.1); atomic
//!   inlines contribute their margin box around their baseline;
//! - lines with no content (only collapsible spaces, nothing with inline
//!   padding/border/margin) are zero-height and not counted (§9.4.2);
//! - `text-align` places each line's content within the line box (§16.2);
//! - floats among the inline content are placed at the line edge and shorten
//!   the line boxes beside them (§9.5).
//!
//! The result is a list of fragments in paint order, in the root's
//! content-box coordinates; `render` paints them in place of the root's
//! children, and `remeasure` feeds the root's height back to taffy.

use super::{LayoutTree, TextRun};
use std::collections::HashMap;
use taffy::NodeId;

/// One painted piece of inline content, in the IFC root's content-box space.
#[derive(Clone, Debug)]
pub enum Frag {
    /// One line's fragment of a non-atomic inline box: its border box.
    /// `first`/`last` say whether the inline-start/-end edges are on it.
    Box { node: NodeId, x: f32, y: f32, w: f32, h: f32, first: bool, last: bool },
    /// Text drawn from (x, baseline).
    Text { node: NodeId, text: String, x: f32, baseline: f32, width: f32 },
    /// An atomic inline or a float: where its border box's top-left goes.
    Atomic { node: NodeId, x: f32, y: f32 },
}

#[derive(Clone, Debug, Default)]
pub struct InlineLayout {
    pub frags: Vec<Frag>,
    /// Content height: the line boxes (plus floats when the root is a BFC).
    pub height: f32,
    /// Baselines (content-box y) of the first and last line boxes.
    pub first_baseline: Option<f32>,
    pub last_baseline: Option<f32>,
    /// Line boxes: (top, height, baseline) — for tests and debugging.
    pub lines: Vec<(f32, f32, f32)>,
}

enum Item {
    Open(NodeId),
    Close(NodeId),
    Text(NodeId),
    Atomic(NodeId),
    Float(NodeId, u8),
    Break,
}

fn tag(tree: &LayoutTree, id: NodeId) -> Option<String> {
    tree.node_map.get(&id).and_then(|n| n.as_element().map(|e| e.name.local.as_ref().to_ascii_lowercase()))
}

/// Atomic inline-level: placed whole (inline-block, replaced, control, cell).
pub(super) fn is_atomic(tree: &LayoutTree, id: NodeId) -> bool {
    let Some(n) = tree.node_map.get(&id) else { return true }; // anonymous box
    if n.as_text().is_some() {
        return false;
    }
    if tree.paint_map.get(&id).and_then(|p| p.display_kind) == Some(2) {
        return true;
    }
    if tree.paint_map.get(&id).and_then(|p| p.flex_container) == Some(true) {
        return true;
    }
    matches!(
        tag(tree, id).as_deref(),
        Some(
            "img" | "input" | "button" | "select" | "textarea" | "video" | "audio" | "svg" | "canvas"
                | "iframe" | "object" | "embed" | "td" | "th" | "meter" | "progress"
        )
    )
}

/// `is_atomic` for the painter.
pub fn is_atomic_pub(tree: &LayoutTree, id: NodeId) -> bool {
    is_atomic(tree, id)
}

fn display_none(tree: &LayoutTree, id: NodeId) -> bool {
    tree.taffy.style(id).map(|s| s.display == taffy::style::Display::None).unwrap_or(false)
}

fn out_of_flow(tree: &LayoutTree, id: NodeId) -> bool {
    matches!(tree.paint_map.get(&id).and_then(|p| p.position_kind), Some(2 | 3))
}

/// True when `id` lays its children out as an inline formatting context:
/// it has children, all in-flow ones are inline-level, and it is a block
/// container (not a table row/section, flex container or control).
pub(super) fn is_ifc_root(tree: &LayoutTree, id: NodeId) -> bool {
    let kids = tree.taffy.children(id).unwrap_or_default();
    if kids.is_empty() {
        return false;
    }
    if let Some(t) = tag(tree, id) {
        if matches!(
            t.as_str(),
            "table" | "thead" | "tbody" | "tfoot" | "tr" | "colgroup" | "button" | "select" | "textarea"
                | "input" | "video" | "audio" | "img" | "svg" | "html"
        ) {
            return false;
        }
    }
    if tree.paint_map.get(&id).and_then(|p| p.flex_container) == Some(true) {
        return false;
    }
    let mut any = false;
    for k in kids {
        if display_none(tree, k) || out_of_flow(tree, k) {
            continue;
        }
        let float = tree.paint_map.get(&k).and_then(|p| p.float).is_some_and(|f| f != 0);
        if !float && !super::is_inline_level(tree, k) {
            return false;
        }
        if tree.node_map.get(&k).is_none() {
            return false; // an anonymous block among the children
        }
        any = true;
    }
    any
}

fn collect(tree: &LayoutTree, id: NodeId, out: &mut Vec<Item>) {
    for k in tree.taffy.children(id).unwrap_or_default() {
        if display_none(tree, k) || out_of_flow(tree, k) {
            continue;
        }
        let Some(n) = tree.node_map.get(&k) else {
            out.push(Item::Atomic(k));
            continue;
        };
        if n.as_text().is_some() {
            out.push(Item::Text(k));
            continue;
        }
        if let Some(f) = tree.paint_map.get(&k).and_then(|p| p.float).filter(|&f| f != 0) {
            out.push(Item::Float(k, f));
            continue;
        }
        if tag(tree, k).as_deref() == Some("br") {
            out.push(Item::Break);
            continue;
        }
        if is_atomic(tree, k) {
            out.push(Item::Atomic(k));
            continue;
        }
        out.push(Item::Open(k));
        collect(tree, k, out);
        out.push(Item::Close(k));
    }
}

/// Font metrics of a run: (ascent, descent, line-height, x-height, half-leading
/// baseline offset from the top of the inline box), Chromium-rounded.
#[derive(Clone, Copy, Debug)]
struct Metrics {
    a: f32,
    d: f32,
    lh: f32,
    xh: f32,
    off: f32,
}

fn metrics(run: &TextRun) -> Metrics {
    match crate::fonts::face(run.family, run.bold, run.italic) {
        Some(f) => {
            let (a, d, _) = crate::fonts::line_metrics(&f, run.font_size);
            let lh = crate::fonts::line_height(&f, run.font_size, run.line_height);
            let m = f.metrics();
            let xh = m.x_height * run.font_size / m.units_per_em as f32;
            Metrics { a, d, lh, xh, off: crate::fonts::baseline_offset(&f, run.font_size, run.line_height) }
        }
        None => {
            let lh = if run.line_height > 0.0 {
                run.font_size * run.line_height
            } else if run.line_height < 0.0 {
                -run.line_height
            } else {
                run.font_size * 1.15
            };
            Metrics { a: run.font_size * 0.8, d: run.font_size * 0.2, lh, xh: run.font_size * 0.5, off: run.font_size * 0.8 }
        }
    }
}

/// The text mode a run breaks with (`nowrap` folded into white-space).
fn mode_of(run: &TextRun) -> crate::fonts::lines::TextMode {
    let mut m = run.mode;
    if run.nowrap && m.white_space == 0 {
        m.white_space = 1;
    }
    m
}

enum Piece {
    Open { node: NodeId, x: f32 },
    Close { node: NodeId, x: f32 },
    Text { node: NodeId, parent: NodeId, text: String, x: f32, w: f32 },
    Atomic { node: NodeId, parent: NodeId, x: f32 },
}

struct Float {
    side: u8,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Inputs the IFC needs about boxes that are not text.
pub(super) struct Ctx<'a> {
    pub text: &'a HashMap<NodeId, TextRun>,
    pub elems: &'a HashMap<NodeId, TextRun>,
    /// Last-line baselines of atomic inline-blocks laid out already (from
    /// their border-box top).
    pub baselines: &'a HashMap<NodeId, f32>,
}

fn run_of<'a>(ctx: &'a Ctx, id: NodeId, fallback: &'a TextRun) -> &'a TextRun {
    ctx.elems.get(&id).or_else(|| ctx.text.get(&id)).unwrap_or(fallback)
}

/// Margin-box geometry of an atomic box from its taffy layout:
/// (margin-box width, margin-box height, baseline from the margin-box top).
fn atomic_box(tree: &LayoutTree, ctx: &Ctx, id: NodeId, fallback: &TextRun) -> (f32, f32, f32) {
    let Ok(l) = tree.taffy.layout(id) else { return (0.0, 0.0, 0.0) };
    let w = l.size.width + l.margin.left + l.margin.right;
    let h = l.size.height + l.margin.top + l.margin.bottom;
    let t = tag(tree, id);
    let clip = tree.paint_map.get(&id).and_then(|p| p.clip).unwrap_or(false);
    let base = if let Some(&b) = ctx.baselines.get(&id).filter(|_| !clip) {
        l.margin.top + b
    } else if matches!(t.as_deref(), Some("input" | "select" | "button" | "textarea")) {
        let kind = tree.node_map.get(&id).and_then(super::control_kind);
        if matches!(kind, Some(super::Control::Checkbox | super::Control::Radio)) {
            // Blink: a checkbox/radio's baseline is its bottom margin edge.
            h
        } else {
            let run = run_of(ctx, id, fallback);
            let m = metrics(run);
            let content_h = l.size.height - l.padding.top - l.padding.bottom - l.border.top - l.border.bottom;
            let lh = m.a + m.d + crate::fonts::face(run.family, run.bold, run.italic)
                .map(|f| crate::fonts::line_metrics(&f, run.font_size).2)
                .unwrap_or(0.0);
            l.margin.top + l.border.top + l.padding.top + ((content_h - lh) / 2.0).floor().max(0.0) + m.a
        }
    } else {
        h // replaced elements and boxes without line boxes: bottom margin edge
    };
    (w, h, base)
}

/// Lays out one IFC root at its current taffy width.
pub(super) fn layout_root(tree: &LayoutTree, ctx: &Ctx, root: NodeId) -> InlineLayout {
    let default_run = TextRun { font_size: 16.0, ..Default::default() };
    let root_run = run_of(ctx, root, &default_run).clone();
    // The UNROUNDED width the box was sized with: a shrink-to-fit box is
    // exactly its content's max-content width, and the rounded one can be a
    // fraction narrower (which would wrap its last word).
    let rl = tree.taffy.unrounded_layout(root);
    let avail_w = (rl.size.width - rl.padding.left - rl.padding.right - rl.border.left - rl.border.right + 0.01).max(0.0);
    let mut items = Vec::new();
    collect(tree, root, &mut items);

    // Baseline shift (down = +) of each inline box relative to the root's
    // baseline, and its own metrics.
    let mut shift: HashMap<NodeId, f32> = HashMap::new();
    shift.insert(root, 0.0);
    let root_m = metrics(&root_run);
    let mut parent_of: HashMap<NodeId, NodeId> = HashMap::new();

    let mut out = InlineLayout::default();
    let mut floats: Vec<Float> = Vec::new();
    let mut pending_floats: Vec<(NodeId, u8)> = Vec::new();
    let mut y = 0.0f32;
    let mut stack: Vec<NodeId> = Vec::new(); // open inline boxes
    let mut pieces: Vec<Piece> = Vec::new();
    let mut line_open: Vec<NodeId> = Vec::new(); // boxes open when the line began
    let mut cur_x = 0.0f32;
    let mut has_content = false;
    let mut forced = false;
    let align = root_run.text_align;

    let edges = |floats: &[Float], y: f32| -> (f32, f32) {
        let mut l = 0.0f32;
        let mut r = avail_w;
        for f in floats {
            if y >= f.y && y < f.y + f.h {
                if f.side == 1 {
                    l = l.max(f.x + f.w);
                } else {
                    r = r.min(f.x);
                }
            }
        }
        (l, r.max(l))
    };

    let place_float = |floats: &mut Vec<Float>, out: &mut InlineLayout, id: NodeId, side: u8, y: f32| {
        let (w, h, _) = atomic_box(tree, ctx, id, &default_run);
        let mut fy = y;
        // Move down past floats until it fits (§9.5.1 rule 8 approximation).
        loop {
            let (l, r) = edges(floats, fy);
            if r - l >= w || floats.iter().all(|f| fy >= f.y + f.h) {
                let x = if side == 1 { l } else { r - w };
                floats.push(Float { side, x, y: fy, w, h });
                let m = tree.taffy.layout(id).map(|l| (l.margin.left, l.margin.top)).unwrap_or((0.0, 0.0));
                out.frags.push(Frag::Atomic { node: id, x: x + m.0, y: fy + m.1 });
                break;
            }
            fy = floats.iter().filter(|f| f.y + f.h > fy).map(|f| f.y + f.h).fold(f32::MAX, f32::min);
        }
    };

    // Ends the current line: vertical alignment, fragments, advance y.
    #[allow(clippy::too_many_arguments)]
    fn finish_line(
        tree: &LayoutTree,
        ctx: &Ctx,
        root: NodeId,
        root_m: Metrics,
        default_run: &TextRun,
        shift: &HashMap<NodeId, f32>,
        pieces: &mut Vec<Piece>,
        line_open: &[NodeId],
        stack: &[NodeId],
        cur_x: f32,
        has_content: bool,
        forced: bool,
        left: f32,
        right: f32,
        align: u8,
        y: &mut f32,
        out: &mut InlineLayout,
    ) {
        let pcs = std::mem::take(pieces);
        if !has_content && !forced {
            return;
        }
        // Boxes on this line: continued ones, then those opened on it.
        let mut boxes: Vec<NodeId> = line_open.to_vec();
        for p in &pcs {
            if let Piece::Open { node, .. } = p {
                boxes.push(*node);
            }
        }
        // §10.8: the strut and every inline box contribute line-height
        // around their (shifted) baseline.
        let mut top = -root_m.off;
        let mut bottom = root_m.lh - root_m.off;
        let mut tb_atomics: Vec<(f32, f32, u8)> = Vec::new();
        for &b in &boxes {
            let m = metrics(run_of(ctx, b, default_run));
            let s = shift.get(&b).copied().unwrap_or(0.0);
            top = top.min(s - m.off);
            bottom = bottom.max(s + m.lh - m.off);
        }
        for p in &pcs {
            match p {
                Piece::Text { node, parent, .. } => {
                    let m = metrics(run_of(ctx, *node, default_run));
                    let s = shift.get(parent).copied().unwrap_or(0.0);
                    top = top.min(s - m.off);
                    bottom = bottom.max(s + m.lh - m.off);
                }
                Piece::Atomic { node, parent, .. } => {
                    let (_, h, base) = atomic_box(tree, ctx, *node, default_run);
                    let va = tree.paint_map.get(node).and_then(|p| p.vertical_align).unwrap_or((0, 0.0));
                    if matches!(va.0, 6 | 7) {
                        tb_atomics.push((h, base, va.0));
                        continue;
                    }
                    let s = shift.get(parent).copied().unwrap_or(0.0) + atomic_shift(ctx, *parent, default_run, va, h, base);
                    top = top.min(s - base);
                    bottom = bottom.max(s + h - base);
                }
                _ => {}
            }
        }
        for &(h, _, _) in &tb_atomics {
            if bottom - top < h {
                bottom = top + h;
            }
        }
        let line_h = bottom - top;
        let baseline = *y - top;
        // Line content width without hanging trailing spaces (css-text-3
        // §4.1.3: a collapsible space at the end of a line is removed).
        let mut pcs = pcs;
        let mut content_w = cur_x;
        for p in pcs.iter_mut().rev() {
            match p {
                Piece::Text { node, text, w, .. } => {
                    let trimmed_len = text.trim_end_matches(' ').len();
                    if trimmed_len != text.len() && !matches!(run_of(ctx, *node, default_run).mode.white_space, 2 | 3 | 5) {
                        let adv_w = trailing_space_width(ctx, *node, default_run, text.len() - trimmed_len);
                        *w = (*w - adv_w).max(0.0);
                        text.truncate(trimmed_len);
                        content_w = (content_w - adv_w).max(0.0);
                    }
                    break;
                }
                Piece::Close { .. } => continue,
                _ => break,
            }
        }
        let free = (right - left - content_w).max(0.0);
        let dx = left + match align {
            1 => free / 2.0,
            2 => free,
            _ => 0.0,
        };
        // Box fragment extents: from the box's open on this line (or the line
        // start) to its close (or the line's content end).
        let box_frag = |node: NodeId, x0: f32, first: bool, pcs: &[Piece], from: usize| -> Frag {
            let mut x1 = content_w;
            let mut last = false;
            for p in &pcs[from..] {
                if let Piece::Close { node: n, x } = p {
                    if *n == node {
                        x1 = *x;
                        last = true;
                        break;
                    }
                }
            }
            let m = metrics(run_of(ctx, node, default_run));
            let s = shift.get(&node).copied().unwrap_or(0.0);
            let (pt, pb, bt, bb) = tree
                .taffy
                .layout(node)
                .map(|l| (l.padding.top, l.padding.bottom, l.border.top, l.border.bottom))
                .unwrap_or_default();
            let top_y = baseline + s - m.a - pt - bt;
            // Fragments start on whole pixels, as text does (see Text below).
            let (bx0, bx1) = ((dx + x0).round(), (dx + x1).round());
            Frag::Box { node, x: bx0, y: top_y, w: (bx1 - bx0).max(0.0), h: m.a + m.d + pt + pb + bt + bb, first, last }
        };
        for &b in line_open {
            out.frags.push(box_frag(b, 0.0, false, &pcs, 0));
        }
        let _ = stack;
        for (i, p) in pcs.iter().enumerate() {
            match p {
                Piece::Open { node, x } => out.frags.push(box_frag(*node, *x, true, &pcs, i)),
                Piece::Close { .. } => {}
                Piece::Text { node, parent, text, x, w } => {
                    let s = shift.get(parent).copied().unwrap_or(0.0);
                    // Each fragment starts on a whole pixel (glyphs inside it
                    // keep their fractional advances), which is where the
                    // flex approximation put each run and matches Chromium's
                    // text-run origins on the corpus best.
                    out.frags.push(Frag::Text { node: *node, text: text.clone(), x: (dx + x).round(), baseline: baseline + s, width: *w });
                }
                Piece::Atomic { node, parent, x } => {
                    let (_, h, base) = atomic_box(tree, ctx, *node, default_run);
                    let va = tree.paint_map.get(node).and_then(|p| p.vertical_align).unwrap_or((0, 0.0));
                    let mtop = match va.0 {
                        6 => *y,
                        7 => *y + line_h - h,
                        _ => {
                            let s = shift.get(parent).copied().unwrap_or(0.0)
                                + atomic_shift(ctx, *parent, default_run, va, h, base);
                            baseline + s - base
                        }
                    };
                    let (ml, mt) = tree.taffy.layout(*node).map(|l| (l.margin.left, l.margin.top)).unwrap_or((0.0, 0.0));
                    out.frags.push(Frag::Atomic { node: *node, x: (dx + x + ml).round(), y: mtop + mt });
                }
            }
        }
        let _ = root;
        if out.first_baseline.is_none() {
            out.first_baseline = Some(baseline);
        }
        out.last_baseline = Some(baseline);
        out.lines.push((*y, line_h, baseline));
        *y += line_h;
    }

    let mut idx = 0;
    while idx < items.len() {
        let (left, right) = edges(&floats, y);
        let line_w = right - left;
        match &items[idx] {
            Item::Open(id) => {
                let parent = *stack.last().unwrap_or(&root);
                parent_of.insert(*id, parent);
                let pm = metrics(run_of(ctx, parent, &default_run));
                let prun = run_of(ctx, parent, &default_run).clone();
                let m = metrics(run_of(ctx, *id, &default_run));
                let va = tree.paint_map.get(id).and_then(|p| p.vertical_align).or_else(|| match tag(tree, *id).as_deref() {
                    Some("sup") => Some((2, 0.0)),
                    Some("sub") => Some((1, 0.0)),
                    _ => None,
                });
                let ps = shift.get(&parent).copied().unwrap_or(0.0);
                let s = match va.map(|v| v.0).unwrap_or(0) {
                    // Blink: sub lowers by parent-size/5 + 1, super raises by parent-size/3 + 1.
                    1 => prun.font_size / 5.0 + 1.0,
                    2 => -(prun.font_size / 3.0 + 1.0),
                    3 => -pm.xh / 2.0 - ((-m.off) + (m.lh - m.off)) / 2.0,
                    4 => -pm.a + m.off,
                    5 => pm.d - (m.lh - m.off),
                    8 => -va.unwrap().1,
                    9 => -va.unwrap().1 * m.lh,
                    _ => 0.0,
                };
                shift.insert(*id, ps + s);
                let (ml, bl, pl) = tree.taffy.layout(*id).map(|l| (l.margin.left, l.border.left, l.padding.left)).unwrap_or_default();
                cur_x += ml;
                pieces.push(Piece::Open { node: *id, x: cur_x });
                cur_x += bl + pl;
                if bl + pl + ml > 0.0 {
                    has_content = true;
                }
                stack.push(*id);
            }
            Item::Close(id) => {
                let (mr, br, pr) = tree.taffy.layout(*id).map(|l| (l.margin.right, l.border.right, l.padding.right)).unwrap_or_default();
                cur_x += br + pr;
                pieces.push(Piece::Close { node: *id, x: cur_x });
                cur_x += mr;
                if br + pr + mr > 0.0 {
                    has_content = true;
                }
                stack.pop();
            }
            Item::Text(id) => {
                let parent = *stack.last().unwrap_or(&root);
                let Some(run) = ctx.text.get(id) else {
                    idx += 1;
                    continue;
                };
                let mode = mode_of(run);
                let face = crate::fonts::face(run.family, run.bold, run.italic);
                let Some(font) = face.as_deref() else {
                    idx += 1;
                    continue;
                };
                let key = crate::fonts::face_key(run.family, run.bold, run.italic);
                let adv = crate::fonts::lines::Advancer::new(font, key, run.font_size, mode.letter_spacing);
                let lines = crate::fonts::lines::break_lines_from(&adv, &run.text, &mode, line_w, cur_x, has_content);
                let preserved = matches!(mode.white_space, 2 | 3 | 5);
                for (i, l) in lines.into_iter().enumerate() {
                    if i > 0 {
                        let (left, right) = edges(&floats, y);
                        finish_line(
                            tree, ctx, root, root_m, &default_run, &shift, &mut pieces, &line_open, &stack, cur_x,
                            has_content, forced || (preserved || mode.white_space == 4), left, right, align, &mut y, &mut out,
                        );
                        for (f, side) in pending_floats.drain(..) {
                            place_float(&mut floats, &mut out, f, side, y);
                        }
                        line_open = stack.clone();
                        cur_x = 0.0;
                        has_content = false;
                        forced = false;
                    }
                    let mut text = l.text;
                    let mut w = l.width;
                    if !has_content && !preserved && cur_x <= 0.0 {
                        // A collapsible space at the start of a line is removed.
                        let t = text.trim_start_matches(' ');
                        if t.len() != text.len() {
                            w -= adv.str(&text[..text.len() - t.len()]);
                            text = t.to_string();
                        }
                    }
                    if !text.is_empty() {
                        if preserved || !text.trim().is_empty() {
                            has_content = true;
                        }
                        pieces.push(Piece::Text { node: *id, parent, text, x: cur_x, w });
                        cur_x += w;
                    }
                }
            }
            Item::Atomic(id) => {
                let parent = *stack.last().unwrap_or(&root);
                let (w, _, _) = atomic_box(tree, ctx, *id, &default_run);
                let prun = run_of(ctx, parent, &default_run);
                let wraps = !matches!(mode_of(prun).white_space, 1 | 2);
                if has_content && wraps && cur_x + w > line_w + 0.01 {
                    finish_line(
                        tree, ctx, root, root_m, &default_run, &shift, &mut pieces, &line_open, &stack, cur_x,
                        has_content, forced, left, right, align, &mut y, &mut out,
                    );
                    for (f, side) in pending_floats.drain(..) {
                        place_float(&mut floats, &mut out, f, side, y);
                    }
                    line_open = stack.clone();
                    cur_x = 0.0;
                    has_content = false;
                    forced = false;
                    continue; // re-place on the new line (edges may differ)
                }
                pieces.push(Piece::Atomic { node: *id, parent, x: cur_x });
                cur_x += w;
                has_content = true;
            }
            Item::Float(id, side) => {
                let (w, _, _) = atomic_box(tree, ctx, *id, &default_run);
                if cur_x + w <= line_w + 0.01 || !has_content {
                    // Fits beside the current line: placed at its top; the
                    // line's content moves over (pieces are line-relative).
                    place_float(&mut floats, &mut out, *id, *side, y);
                } else {
                    pending_floats.push((*id, *side));
                }
            }
            Item::Break => {
                let (left, right) = edges(&floats, y);
                finish_line(
                    tree, ctx, root, root_m, &default_run, &shift, &mut pieces, &line_open, &stack, cur_x,
                    true, true, left, right, align, &mut y, &mut out,
                );
                for (f, side) in pending_floats.drain(..) {
                    place_float(&mut floats, &mut out, f, side, y);
                }
                line_open = stack.clone();
                cur_x = 0.0;
                has_content = false;
                forced = false;
            }
        }
        idx += 1;
    }
    let (left, right) = edges(&floats, y);
    finish_line(
        tree, ctx, root, root_m, &default_run, &shift, &mut pieces, &line_open, &stack, cur_x, has_content, forced,
        left, right, align, &mut y, &mut out,
    );
    for (f, side) in pending_floats.drain(..) {
        place_float(&mut floats, &mut out, f, side, y);
    }
    out.height = y;
    if establishes_bfc(tree, root) {
        let fb = floats.iter().map(|f| f.y + f.h).fold(0.0f32, f32::max);
        out.height = out.height.max(fb);
    }
    out
}

fn trailing_space_width(ctx: &Ctx, node: NodeId, fallback: &TextRun, n: usize) -> f32 {
    let run = run_of(ctx, node, fallback);
    crate::fonts::face(run.family, run.bold, run.italic)
        .map(|f| crate::fonts::space_advance(&f, run.font_size) + run.mode.letter_spacing)
        .unwrap_or(run.font_size * 0.25)
        * n as f32
}

/// vertical-align shift (down = +) of an atomic box of margin height `h`
/// whose baseline is `base` below its margin top, inside `parent`.
fn atomic_shift(ctx: &Ctx, parent: NodeId, fallback: &TextRun, va: (u8, f32), h: f32, base: f32) -> f32 {
    let prun = run_of(ctx, parent, fallback);
    let pm = metrics(prun);
    match va.0 {
        1 => prun.font_size / 5.0 + 1.0,
        2 => -(prun.font_size / 3.0 + 1.0),
        // middle: the box's vertical midpoint at the parent baseline plus
        // half the parent's x-height.
        3 => -pm.xh / 2.0 - (h / 2.0 - base),
        4 => -pm.a + base,
        5 => pm.d - (h - base),
        8 => -va.1,
        9 => -va.1 * pm.lh,
        _ => 0.0,
    }
}

/// Block formatting context roots contain their floats (§10.6.7).
fn establishes_bfc(tree: &LayoutTree, id: NodeId) -> bool {
    let p = tree.paint_map.get(&id);
    p.and_then(|p| p.display_kind) == Some(2)
        || p.and_then(|p| p.clip).unwrap_or(false)
        || p.and_then(|p| p.float).is_some_and(|f| f != 0)
        || matches!(p.and_then(|p| p.position_kind), Some(2 | 3))
        || matches!(tag(tree, id).as_deref(), Some("td" | "th" | "body"))
}

/// Every IFC root in the tree, children before parents (an inline-block's
/// own lines decide its baseline in the line that holds it).
pub(super) fn roots_post_order(tree: &LayoutTree) -> Vec<NodeId> {
    fn walk(tree: &LayoutTree, id: NodeId, out: &mut Vec<NodeId>) {
        for k in tree.taffy.children(id).unwrap_or_default() {
            walk(tree, k, out);
        }
        if is_ifc_root(tree, id) {
            out.push(id);
        }
    }
    let mut out = Vec::new();
    walk(tree, tree.root_node, &mut out);
    out
}

/// Lays out every IFC; returns (root, content height) and stores the
/// layouts on the tree.
pub(super) fn layout_all(tree: &mut LayoutTree, text: &HashMap<NodeId, TextRun>, elems: &HashMap<NodeId, TextRun>) -> Vec<(NodeId, f32)> {
    let roots = roots_post_order(tree);
    let mut baselines: HashMap<NodeId, f32> = HashMap::new();
    let mut layouts: HashMap<NodeId, InlineLayout> = HashMap::new();
    let mut heights = Vec::new();
    for &r in &roots {
        let ctx = Ctx { text, elems, baselines: &baselines };
        let il = layout_root(tree, &ctx, r);
        if let (Some(b), Ok(l)) = (il.last_baseline, tree.taffy.layout(r)) {
            let b = b + l.border.top + l.padding.top;
            baselines.insert(r, b);
            // An inline-block whose lines sit inside a block child: its
            // baseline is that last line's, measured from its own top.
            let mut a = r;
            while let Some(p) = tree.taffy.parent(a) {
                if tree.paint_map.get(&p).and_then(|q| q.display_kind) == Some(2) || is_atomic(tree, p) {
                    if !baselines.contains_key(&p) {
                        let mut off = b;
                        let mut c = r;
                        while c != p {
                            off += tree.taffy.layout(c).map(|l| l.location.y).unwrap_or(0.0);
                            c = tree.taffy.parent(c).unwrap_or(p);
                        }
                        baselines.insert(p, off);
                    }
                    break;
                }
                if is_ifc_root(tree, p) {
                    break;
                }
                a = p;
            }
        }
        heights.push((r, il.height));
        layouts.insert(r, il);
    }
    tree.inline = layouts;
    heights
}
