// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AETHERMETAL (SR66) M2, LAYOUTCORE (SR76): painting the `layout_core::LayoutTree` — the box tree and line boxes
// the host engine's layout produces — into a back buffer the size of the window, then onto the compositor
// surface. The host's `render/` reads the same tree; this is the subset this window draws, in CSS2 Appendix E
// order per box: background (border box), borders (solid), the list marker, then either the box's line boxes
// (an inline formatting context: inline boxes' backgrounds, text runs at their baselines, underlines, atomic
// inlines placed where the line put them) or its children. Inherited paint (colour, size, weight, italic,
// family, underline) walks down the box tree with the UA defaults the host painter uses (`layout_core`'s
// `default_*`, the link blue). The canvas takes the root's background, or the body's (CSS2 §14.2). The page is
// translated up by the scroll offset and clipped to the window; boxes wholly below it are skipped.
//
// AETHERMETAL M3 (SR96): the interaction's paint. A text fragment carrying a [`Mark`] (the selection, a find
// match, the current find match — keyed by its IFC root and fragment index, built by `ui`) gets its marked bytes'
// line-box band filled first (the caret x of each end measured exactly as the hit test measures it), the glyphs
// on top, and a selection's glyphs again in white (Chromium's highlight, the host's `selection::HIGHLIGHT`
// over white). Over the page: the focused link's outline and the status line (the hovered href, the find bar, a
// flash). No pointer: the compositor's sprite is the one pointer (AETHERPTR, rmbp-ledger B543).
//
// UNAKEYS (rmbp-ledger B552): no colour is spelled here — the page's initial colours are `theme::PAGE_*` and every
// colour over the page (the marks, the status line) is a field of the live `theme::cur()` (Principia's appearance).

use alloc::vec::Vec;

use crate::style::{ImgHost, Tree};
use crate::txt::Fonts;
use layout_core::taffy::{self, NodeId};
use layout_core::{DomNode, FontSel, NodeMap};

pub struct Canvas {
    pub px: Vec<u32>,
    pub w: i32,
    pub h: i32,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Canvas {
        Canvas { px: alloc::vec![0; (w * h) as usize], w, h }
    }

    pub fn fill(&mut self, x0: i32, y0: i32, w: i32, h: i32, c: u32) {
        let (xs, ys) = (x0.max(0), y0.max(0));
        let (xe, ye) = ((x0 + w).min(self.w), (y0 + h).min(self.h));
        for y in ys..ye {
            let row = (y * self.w) as usize;
            for x in xs..xe {
                self.px[row + x as usize] = c;
            }
        }
    }

    fn blend(&mut self, x: i32, y: i32, ink: u32, a: u8) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            let i = (y * self.w + x) as usize;
            self.px[i] = font_core::ui::blend(self.px[i], ink, a);
        }
    }

    /// How many pixels differ from `bg` — the witness's `painted=`.
    pub fn count_not(&self, bg: u32) -> usize {
        self.px.iter().filter(|&&p| p != bg).count()
    }
}

fn fl(v: f32) -> i32 {
    font_core::fmath::floor(v) as i32
}

fn rgb(c: (u8, u8, u8)) -> u32 {
    ((c.0 as u32) << 16) | ((c.1 as u32) << 8) | c.2 as u32
}

/// The inherited paint of a box (text nodes carry their parent's).
#[derive(Clone, Copy)]
pub struct Ink {
    pub color: u32,
    pub size: f32,
    pub sel: FontSel,
    underline: bool,
    /// list-style-type code (css-lists-3; 1 disc, 2 circle, 3 square, 4 decimal …), inherited.
    list: u8,
}

/// Inherited paint for every box, root down (also the hit test's font for a text box's caret).
pub fn inks(tree: &Tree) -> NodeMap<Ink> {
    fn walk(tree: &Tree, id: NodeId, inh: Ink, out: &mut NodeMap<Ink>) {
        let mut me = inh;
        if let Some(n) = tree.node_map.get(&id) {
            if let Some(tag) = n.tag() {
                let tag = tag.as_str();
                let p = tree.paint_map.get(&id);
                me.color = p
                    .and_then(|p| p.color)
                    .map(rgb)
                    .unwrap_or(if tag == "a" && n.has_attr("href") { crate::theme::PAGE_LINK } else { inh.color });
                if let Some(fs) = p.and_then(|p| p.used_font_size) {
                    me.size = fs;
                }
                me.sel.weight = p
                    .and_then(|p| p.weight)
                    .map(|w| w.resolve(inh.sel.weight))
                    .unwrap_or_else(|| layout_core::default_weight(tag, inh.sel.weight));
                me.sel.italic = p.and_then(|p| p.italic).unwrap_or_else(|| layout_core::default_italic(tag, inh.sel.italic));
                me.sel.family = p.and_then(|p| p.family).unwrap_or_else(|| layout_core::default_family::<ImgHost>(tag, inh.sel.family));
                me.underline = p.and_then(|p| p.underline).unwrap_or(inh.underline || matches!(tag, "u" | "ins") || (tag == "a" && n.has_attr("href")));
                match tag {
                    "ul" => me.list = 1,
                    "ol" => me.list = 4,
                    _ => {}
                }
                if let Some(ls) = p.and_then(|p| p.list_style) {
                    me.list = ls;
                }
            }
        }
        out.insert(id, me);
        for k in tree.taffy.children(id).unwrap_or_default() {
            walk(tree, k, me, out);
        }
    }
    let mut out = NodeMap::new();
    let root = Ink { color: crate::theme::PAGE_INK, size: 16.0, sel: FontSel::default(), underline: false, list: 1 };
    walk(tree, tree.root_node, root, &mut out);
    out
}

/// The canvas colour: the root element's background, else the body's, else white (CSS2 §14.2).
pub fn canvas_bg(tree: &Tree) -> u32 {
    let bg_of = |want: &str| {
        tree.node_map.iter().find_map(|(id, n)| {
            if n.tag().as_deref() != Some(want) {
                return None;
            }
            let p = tree.paint_map.get(id)?;
            p.background.filter(|_| p.bg_alpha.unwrap_or(1.0) >= 0.5).map(rgb)
        })
    };
    bg_of("html").or_else(|| bg_of("body")).unwrap_or(crate::theme::PAGE_BG)
}

struct Cx<'a> {
    tree: &'a Tree,
    inks: &'a NodeMap<Ink>,
    fonts: &'a Fonts,
    marks: &'a NodeMap<Vec<Mark>>,
}

/// What a mark paints: the selection, a find match, the current find match.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    Select,
    Find,
    FindCur,
}

/// Bytes `[s, e)` of fragment `frag` of an IFC are marked.
#[derive(Clone, Copy)]
pub struct Mark {
    pub frag: usize,
    pub s: usize,
    pub e: usize,
    pub kind: MarkKind,
}

/// Paint the page with its top at `-scroll`; `bg` is the canvas colour, `inks` the tree's inherited paint,
/// `marks` the selection and find highlights by IFC root.
pub fn paint(cv: &mut Canvas, tree: &Tree, scroll: f32, bg: u32, fonts: &Fonts, inks: &NodeMap<Ink>, marks: &NodeMap<Vec<Mark>>) {
    cv.fill(0, 0, cv.w, cv.h, bg);
    let cx = Cx { tree, inks, fonts, marks };
    let root = tree.root_node;
    let (x, y) = tree.taffy.layout(root).map(|l| (l.location.x, l.location.y)).unwrap_or((0.0, 0.0));
    paint_box(cv, &cx, root, x, y - scroll);
}

fn text(cv: &mut Canvas, cx: &Cx, ink: &Ink, x: f32, baseline: f32, s: &str, w: f32) {
    let base = font_core::fmath::round(baseline);
    cx.fonts.0.borrow_mut().draw(x, base, s, ink.sel, ink.size, ink.color, &mut |gx, gy, a| cv.blend(gx, gy, ink.color, a));
    if ink.underline {
        let uy = base as i32 + 1 + (ink.size / 14.0) as i32;
        let th = if ink.size >= 20.0 { 2 } else { 1 };
        cv.fill(fl(x), uy, fl(x + w) - fl(x), th, ink.color);
    }
}

/// The list marker text for item `k` (1-based) in list-style `code`.
fn marker(code: u8, k: usize) -> Option<alloc::string::String> {
    use alloc::string::ToString;
    Some(match code {
        0 => return None,
        2 => "\u{25E6}".to_string(),
        3 => "\u{25AA}".to_string(),
        4..=10 => {
            let mut s = k.to_string();
            s.push('.');
            s
        }
        _ => "\u{2022}".to_string(),
    })
}

/// The first line's baseline of `id` from its border-box top, through first children.
fn first_baseline(tree: &Tree, id: NodeId) -> Option<f32> {
    let l = tree.taffy.layout(id).ok()?;
    if let Some(il) = tree.inline.get(&id) {
        return il.first_baseline.map(|b| b + l.border.top + l.padding.top);
    }
    let k = *tree.taffy.children(id).ok()?.first()?;
    let kl = tree.taffy.layout(k).ok()?;
    first_baseline(tree, k).map(|b| b + kl.location.y)
}

/// Paint box `id` with its border box's top-left at (`x`, `y`) on the canvas.
fn paint_box(cv: &mut Canvas, cx: &Cx, id: NodeId, x: f32, y: f32) {
    let tree = cx.tree;
    let Ok(l) = tree.taffy.layout(id) else { return };
    if tree.taffy.style(id).is_ok_and(|s| s.display == taffy::style::Display::None) {
        return;
    }
    let vh = cv.h as f32;
    if y > vh {
        return;
    }
    let p = tree.paint_map.get(&id);
    let hidden = p.and_then(|p| p.hidden).unwrap_or(false);
    let ink = cx.inks.get(&id).copied().unwrap_or(Ink { color: crate::theme::PAGE_INK, size: 16.0, sel: FontSel::default(), underline: false, list: 1 });
    let node = tree.node_map.get(&id);
    let (w, h) = (l.size.width, l.size.height);
    if !hidden && y + h >= 0.0 {
        if let Some(c) = p.and_then(|p| p.background).filter(|_| p.and_then(|p| p.bg_alpha).unwrap_or(1.0) >= 0.5) {
            cv.fill(fl(x), fl(y), fl(x + w) - fl(x), fl(y + h) - fl(y), rgb(c));
        }
        let sides = p.and_then(|p| p.border).unwrap_or([None; 4]);
        let col = |i: usize| sides[i].map(|(_, c)| rgb(c)).unwrap_or(ink.color);
        let (xi, yi, wi, hi) = (fl(x), fl(y), fl(x + w) - fl(x), fl(y + h) - fl(y));
        let (bt, br, bb, bl) = (l.border.top, l.border.right, l.border.bottom, l.border.left);
        if bt > 0.0 && sides[0].is_some() {
            cv.fill(xi, yi, wi, bt as i32, col(0));
        }
        if bb > 0.0 && sides[2].is_some() {
            cv.fill(xi, yi + hi - bb as i32, wi, bb as i32, col(2));
        }
        if bl > 0.0 && sides[3].is_some() {
            cv.fill(xi, yi, bl as i32, hi, col(3));
        }
        if br > 0.0 && sides[1].is_some() {
            cv.fill(xi + wi - br as i32, yi, br as i32, hi, col(1));
        }
        // The list marker, outside the content box at the first line's baseline.
        let is_item = p.and_then(|p| p.list_item) == Some(true) || node.and_then(|n| n.tag()).as_deref() == Some("li");
        if is_item {
            let k = tree
                .taffy
                .parent(id)
                .and_then(|pa| tree.taffy.children(pa).ok())
                .map(|sib| {
                    sib.iter()
                        .take_while(|&&s| s != id)
                        .filter(|&&s| tree.node_map.get(&s).and_then(|n| n.tag()).as_deref() == Some("li"))
                        .count()
                        + 1
                })
                .unwrap_or(1);
            if let (Some(m), Some(fb)) = (marker(ink.list, k), first_baseline(tree, id)) {
                let base = y + fb;
                if base > -40.0 && base < vh + 40.0 {
                    let mw = cx.fonts.0.borrow_mut().measure(&m, ink.sel, ink.size);
                    let mx = x + l.border.left + l.padding.left - ink.size * 0.5 - mw;
                    text(cv, cx, &ink, mx, base, &m, mw);
                }
            }
        }
    }
    // Content origin.
    let (ox, oy) = (x + l.border.left + l.padding.left, y + l.border.top + l.padding.top);
    if let Some(il) = tree.inline.get(&id) {
        let marks = cx.marks.get(&id);
        for (fi, f) in il.frags.iter().enumerate() {
            match f {
                layout_core::inline::Frag::Box { node, x: fx, y: fy, w: fw, h: fh, .. } => {
                    let bp = tree.paint_map.get(node);
                    if let Some(c) = bp.and_then(|p| p.background).filter(|_| bp.and_then(|p| p.bg_alpha).unwrap_or(1.0) >= 0.5) {
                        let (bx, by) = (ox + fx, oy + fy);
                        cv.fill(fl(bx), fl(by), fl(bx + fw) - fl(bx), fl(by + fh) - fl(by), rgb(c));
                    }
                }
                layout_core::inline::Frag::Text { node, text: s, x: fx, baseline, width } => {
                    let base = oy + baseline;
                    if hidden || base < -40.0 || base > vh + 40.0 {
                        continue;
                    }
                    let tink = cx.inks.get(node).copied().unwrap_or(ink);
                    let mut white: Option<(f32, &str)> = None;
                    for m in marks.into_iter().flatten().filter(|m| m.frag == fi && m.e > m.s && m.e <= s.len()) {
                        let caret = |b: usize| if b == 0 { 0.0 } else { cx.fonts.0.borrow_mut().measure(&s[..b], tink.sel, tink.size) };
                        let (x0, x1) = (ox + fx + caret(m.s), ox + fx + caret(m.e));
                        let (top, lh) = layout_core::hit::line_of(il, *baseline).map(|(_, t, h)| (t, h)).unwrap_or((baseline - tink.size, tink.size * 1.25));
                        let th = crate::theme::cur();
                        let c = match m.kind {
                            MarkKind::Select => th.sel_bg,
                            MarkKind::Find => th.find_bg,
                            MarkKind::FindCur => th.find_cur_bg,
                        };
                        cv.fill(fl(x0), fl(oy + top), (fl(x1) - fl(x0)).max(1), fl(oy + top + lh) - fl(oy + top), c);
                        if m.kind == MarkKind::Select {
                            white = Some((x0, &s[m.s..m.e]));
                        }
                    }
                    text(cv, cx, &tink, ox + fx, base, s, *width);
                    if let Some((x0, part)) = white {
                        let wink = Ink { color: crate::theme::cur().sel_ink, underline: false, ..tink };
                        text(cv, cx, &wink, x0, base, part, 0.0);
                    }
                }
                layout_core::inline::Frag::Atomic { node, x: fx, y: fy } => {
                    paint_box(cv, cx, *node, ox + fx, oy + fy);
                }
            }
        }
        return;
    }
    // A text box outside any inline formatting context (a control's label): one line from its top.
    if let Some(n) = node.filter(|n| n.is_text()) {
        if !hidden {
            let (a, _, _) = cx.fonts.0.borrow().line_metrics(ink.sel, ink.size);
            let s: alloc::string::String = n.text().split_whitespace().collect::<Vec<_>>().join(" ");
            let tw = cx.fonts.0.borrow_mut().measure(&s, ink.sel, ink.size);
            text(cv, cx, &ink, x, y + a, &s, tw);
        }
        return;
    }
    for k in tree.taffy.children(id).unwrap_or_default() {
        let Ok(kl) = tree.taffy.layout(k) else { continue };
        paint_box(cv, cx, k, x + kl.location.x, y + kl.location.y);
    }
}

// ---- AETHERMETAL M3 (SR96): over the page ---------------------------------------------------------------

/// The status line's height (px) at the window's foot.
pub const STATUS_H: i32 = 14;

/// A 1 px outline (the focused link's text boxes).
pub fn outline(cv: &mut Canvas, x: i32, y: i32, w: i32, h: i32, c: u32) {
    cv.fill(x, y, w, 1, c);
    cv.fill(x, y + h - 1, w, 1, c);
    cv.fill(x, y, 1, h, c);
    cv.fill(x + w - 1, y, 1, h, c);
}

/// The status line: `s` in the small face on the theme's status band at the window's foot (the hovered href, the
/// find bar, a flash); `accent` turns it to the alert band (the find bar with no match).
pub fn status(cv: &mut Canvas, fonts: &Fonts, s: &str, accent: bool) {
    let th = crate::theme::cur();
    let y = cv.h - STATUS_H;
    cv.fill(0, y, cv.w, STATUS_H, if accent { th.status_alert } else { th.status_bg });
    cv.fill(0, y, cv.w, 1, th.status_rule);
    let sel = FontSel::default();
    let ink = th.status_ink;
    let mut t = fonts.0.borrow_mut();
    let (a, _, _) = t.line_metrics(sel, 11.0);
    t.draw(4.0, (y + 2) as f32 + a, s, sel, 11.0, ink, &mut |gx, gy, cov| cv.blend(gx, gy, ink, cov));
}
