//! AETHERMETAL M3 (LEDGER SR96): hit testing over the layout tree — the host's `selection.rs` hit-test shape as
//! ONE function both rings call.
//!
//! CHARTER: Aether (CODEX §2: Aether owns the Web) — seam `shared-core` (this crate, `no_std` + `alloc`): the
//! host browser (handlers/aether/src/selection.rs, AETHERSELECT SR69) and the ring-3 program APPS/AETHER.ELF
//! (crates/user-aether) turn a pointer into a DOM boundary point here.
//!
//! What moved from handlers/aether/src/selection.rs (std → alloc; the logic verbatim):
//!
//! * [`Point`] — a DOM boundary point (an arena index + a UTF-16 offset);
//! * [`utf16_to_byte`] / [`byte_to_utf16`], [`align`] (a Text node's line fragments ↔ its data offsets),
//!   [`words`] / [`word_at`] (UAX #29 over grapheme clusters), [`line_of`], [`collapses_at`];
//! * [`hit_text`] — the nearest grapheme boundary by caret x; the caret is a CALLER closure (the host shapes
//!   with its `Advancer`, the image measures with `font_core::ui`), so no font type crosses the seam;
//! * [`pick`] and [`hit`] — the paint-order nearest text box and the boundary point / word / block under the
//!   pointer. The boxes are [`TextBox`]es: the host maps the ones its last frame painted
//!   (`render::text_boxes`, each with its own scroll), the image builds them from the tree with
//!   [`text_boxes`] (document coordinates, the same walk its painter makes).
//!
//! The DOM node's arena index is a caller closure too (`id_of`): `DomNode` names no identity number.

use crate::inline::{Frag, InlineLayout};
use crate::prelude::*;
use crate::tree::LayoutTree;
use crate::{DomNode, Host};
use taffy::NodeId;

/// A DOM boundary point: `node` is an arena id, `offset` a UTF-16 offset (character data) or child index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Point {
    pub node: usize,
    pub offset: u32,
}

// =================================================================================================
// UTF-16 / byte offsets
// =================================================================================================

pub fn utf16_to_byte(s: &str, u: u32) -> usize {
    let mut n = 0u32;
    for (b, c) in s.char_indices() {
        if n >= u {
            return b;
        }
        n += c.len_utf16() as u32;
    }
    s.len()
}

pub fn byte_to_utf16(s: &str, b: usize) -> u32 {
    s[..b.min(s.len())].encode_utf16().count() as u32
}

// =================================================================================================
// Fragment text ↔ DOM offsets
// =================================================================================================

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

/// Aligns the texts a Text node's line fragments draw (whitespace collapsed, line-end spaces trimmed,
/// tabs expanded — `lines`) with the node's `data`: for each fragment, the UTF-16 offset in `data` of each
/// of its char boundaries (`chars + 1` entries). A space the layout carried in from a neighbouring node
/// (`lead`) has zero width here; a character text-transform changed maps one to one.
pub fn align(data: &str, frags: &[&str], collapse: bool) -> Vec<Vec<u32>> {
    let src: Vec<(u32, char)> = {
        let mut v = Vec::with_capacity(data.len());
        let mut u = 0u32;
        for c in data.chars() {
            v.push((u, c));
            u += c.len_utf16() as u32;
        }
        v
    };
    let total = data.encode_utf16().count() as u32;
    let off = |i: usize| if i < src.len() { src[i].0 } else { total };
    let mut i = 0usize;
    let mut out = Vec::with_capacity(frags.len());
    for f in frags {
        let mut v = Vec::with_capacity(f.len() + 1);
        let mut in_tab = false;
        for c in f.chars() {
            if collapse {
                if c == ' ' {
                    v.push(off(i));
                    while i < src.len() && is_ws(src[i].1) {
                        i += 1;
                    }
                    continue;
                }
                while i < src.len() && is_ws(src[i].1) {
                    i += 1;
                }
            } else {
                while i < src.len() && matches!(src[i].1, '\n' | '\r') && c != src[i].1 {
                    i += 1;
                }
                if c == ' ' && in_tab && (i >= src.len() || src[i].1 != ' ') {
                    v.push(off(i));
                    continue;
                }
                in_tab = false;
                if c == ' ' && i < src.len() && src[i].1 == '\t' {
                    v.push(off(i));
                    i += 1;
                    in_tab = true;
                    continue;
                }
            }
            v.push(off(i));
            if i < src.len() {
                i += 1;
            }
        }
        v.push(off(i));
        out.push(v);
    }
    out
}

/// The grapheme boundary of `text` nearest to `x` (byte offset), and the byte offset of the grapheme under
/// `x`. `caret(b)` is the x of the caret before byte `b` (0 at 0, the run's advance at `text.len()`).
pub fn hit_text(text: &str, x: f32, caret: impl Fn(usize) -> f32) -> (usize, usize) {
    let bounds: Vec<usize> =
        font_core::grapheme::clusters(text).into_iter().map(|(a, _)| a).chain(core::iter::once(text.len())).collect();
    let xs: Vec<f32> = bounds.iter().map(|&b| caret(b)).collect();
    let mut near = (f32::INFINITY, 0usize);
    for (k, &b) in bounds.iter().enumerate() {
        let d = (xs[k] - x).abs();
        if d < near.0 {
            near = (d, b);
        }
    }
    let mut under = bounds.first().copied().unwrap_or(0);
    for k in 0..bounds.len().saturating_sub(1) {
        if x >= xs[k] {
            under = bounds[k];
        }
    }
    (near.1, under)
}

// =================================================================================================
// Words (UAX #29, simplified to the rules a click needs)
// =================================================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wb {
    Letter,
    Numeric,
    MidLetter,
    MidNum,
    MidNumLet,
    ExtendNumLet,
    Space,
    Ideo,
    Other,
}

fn wb(c: char) -> Wb {
    let u = c as u32;
    if matches!(u, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF) {
        return Wb::Ideo;
    }
    match c {
        ' ' | '\t' | '\u{a0}' | '\u{3000}' | '\u{2000}'..='\u{200a}' => Wb::Space,
        '\n' | '\r' | '\x0c' => Wb::Other,
        ':' | '\u{b7}' | '\u{387}' | '\u{5f4}' | '\u{2027}' | '\u{fe13}' | '\u{fe55}' | '\u{ff1a}' => Wb::MidLetter,
        ',' | ';' | '\u{37e}' | '\u{589}' | '\u{60c}' | '\u{60d}' | '\u{66c}' | '\u{7f8}' | '\u{2044}' | '\u{fe10}'
        | '\u{fe14}' | '\u{fe50}' | '\u{fe54}' | '\u{ff0c}' | '\u{ff1b}' => Wb::MidNum,
        '.' | '\'' | '\u{2018}' | '\u{2019}' | '\u{2024}' | '\u{fe52}' | '\u{ff07}' | '\u{ff0e}' => Wb::MidNumLet,
        '_' | '\u{203f}' | '\u{2040}' | '\u{2054}' | '\u{fe33}' | '\u{fe34}' | '\u{fe4d}'..='\u{fe4f}' | '\u{ff3f}' => {
            Wb::ExtendNumLet
        }
        _ if c.is_numeric() => Wb::Numeric,
        _ if c.is_alphabetic() => Wb::Letter,
        _ => Wb::Other,
    }
}

/// UAX #29 word segments of `text` as byte ranges (over grapheme clusters, so marks stay with their base).
pub fn words(text: &str) -> Vec<(usize, usize)> {
    let gs: Vec<(usize, usize, Wb)> = font_core::grapheme::clusters(text)
        .into_iter()
        .map(|(a, b)| (a, b, text[a..b].chars().next().map(wb).unwrap_or(Wb::Other)))
        .collect();
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    let ah = |w: Wb| w == Wb::Letter;
    while i < gs.len() {
        let start = gs[i].0;
        let c = gs[i].2;
        let mut j = i + 1;
        match c {
            Wb::Space => {
                while j < gs.len() && gs[j].2 == Wb::Space {
                    j += 1;
                }
            }
            Wb::Letter | Wb::Numeric | Wb::ExtendNumLet => loop {
                if j >= gs.len() {
                    break;
                }
                let prev = gs[j - 1].2;
                let cur = gs[j].2;
                let next = gs.get(j + 1).map(|g| g.2);
                let joins = match (prev, cur) {
                    // WB5, WB8, WB9, WB10, WB13a, WB13b
                    (Wb::Letter | Wb::Numeric | Wb::ExtendNumLet, Wb::Letter | Wb::Numeric | Wb::ExtendNumLet) => true,
                    // WB6/WB7: letter (MidLetter | MidNumLet) letter
                    (p, Wb::MidLetter | Wb::MidNumLet) if ah(p) && next.is_some_and(ah) => {
                        j += 1;
                        true
                    }
                    // WB11/WB12: digit (MidNum | MidNumLet) digit
                    (Wb::Numeric, Wb::MidNum | Wb::MidNumLet) if next == Some(Wb::Numeric) => {
                        j += 1;
                        true
                    }
                    _ => false,
                };
                if !joins {
                    break;
                }
                j += 1;
            },
            _ => {}
        }
        let end = if j <= gs.len() { gs[j - 1].1 } else { text.len() };
        out.push((start, end));
        i = j;
    }
    out
}

/// The word segment holding the grapheme at byte `b` (the last one when `b` is the end).
pub fn word_at(text: &str, b: usize) -> (usize, usize) {
    let ws = words(text);
    for &(s, e) in &ws {
        if b >= s && b < e {
            return (s, e);
        }
    }
    ws.last().copied().unwrap_or((b, b))
}

// =================================================================================================
// Lines and white-space
// =================================================================================================

/// The line box (index, top, height) of the IFC `il` a fragment with baseline `baseline` sits on.
pub fn line_of(il: &InlineLayout, baseline: f32) -> Option<(usize, f32, f32)> {
    let mut best: Option<(usize, f32, f32, f32)> = None;
    for (i, &(top, h, _)) in il.lines.iter().enumerate() {
        let d = if baseline < top {
            top - baseline
        } else if baseline > top + h {
            baseline - top - h
        } else {
            0.0
        };
        if best.is_none_or(|b| d < b.3) {
            best = Some((i, top, h, d));
        }
    }
    best.map(|(i, t, h, _)| (i, t, h))
}

/// Whether text in `tid`'s white-space mode collapses (normal / nowrap / pre-line).
pub fn collapses_at<H: Host>(tree: &LayoutTree<H>, tid: NodeId) -> bool {
    let mut ws = None;
    let mut n = tree.taffy.parent(tid);
    let mut chain = Vec::new();
    while let Some(p) = n {
        chain.push(p);
        n = tree.taffy.parent(p);
    }
    let mut cur = 0u8;
    for p in chain.into_iter().rev() {
        if let Some(dn) = tree.node_map.get(&p) {
            if let Some(tag) = dn.tag().filter(|_| dn.is_element()) {
                cur = tree.paint_map.get(&p).and_then(|s| s.white_space).unwrap_or_else(|| crate::default_white_space(&tag, cur));
                ws = Some(cur);
            }
        }
    }
    matches!(ws.unwrap_or(0), 0 | 1 | 4)
}

// =================================================================================================
// Text boxes and the hit
// =================================================================================================

/// One painted text fragment: the IFC root and the fragment's index in its `frags`, the text node's box, the
/// pen origin x and advance width, and the line box it sits on — in whatever space the caller painted it
/// (the image: document coordinates; the host: its viewport at the box's own recorded scroll).
#[derive(Clone, Copy, Debug)]
pub struct TextBox {
    pub ifc: NodeId,
    pub frag: usize,
    pub node: NodeId,
    pub x: f32,
    pub w: f32,
    pub top: f32,
    pub bottom: f32,
}

/// Every text fragment of `tree` in paint order (topmost last), in document coordinates — the walk the
/// image's painter makes (border-box origins down the taffy tree, an IFC's content box for its fragments,
/// atomic inlines where their line placed them; `display: none` and `hidden` text skipped).
pub fn text_boxes<H: Host>(tree: &LayoutTree<H>) -> Vec<TextBox> {
    let mut out = Vec::new();
    let root = tree.root_node;
    let (x, y) = tree.taffy.layout(root).map(|l| (l.location.x, l.location.y)).unwrap_or((0.0, 0.0));
    walk(tree, root, x, y, &mut out);
    out
}

fn walk<H: Host>(tree: &LayoutTree<H>, id: NodeId, x: f32, y: f32, out: &mut Vec<TextBox>) {
    let Ok(l) = tree.taffy.layout(id) else { return };
    if tree.taffy.style(id).is_ok_and(|s| s.display == taffy::style::Display::None) {
        return;
    }
    let hidden = tree.paint_map.get(&id).and_then(|p| p.hidden).unwrap_or(false);
    // AETHERSTYLE3 (SR103): a scroll container's content moves by its offset.
    let (shx, shy) = crate::scroll::content_shift(tree, id);
    let (ox, oy) = (x + shx + l.border.left + l.padding.left, y + shy + l.border.top + l.padding.top);
    if let Some(il) = tree.inline.get(&id) {
        for (i, f) in il.frags.iter().enumerate() {
            match f {
                Frag::Text { node, x: fx, baseline, width, .. } => {
                    if hidden {
                        continue;
                    }
                    let (top, bottom) = match line_of(il, *baseline) {
                        Some((_, t, h)) => (t, t + h),
                        None => (baseline - 12.0, baseline + 4.0),
                    };
                    out.push(TextBox { ifc: id, frag: i, node: *node, x: ox + fx, w: *width, top: oy + top, bottom: oy + bottom });
                }
                Frag::Atomic { node, x: fx, y: fy } => walk(tree, *node, ox + fx, oy + fy, out),
                Frag::Box { .. } => {}
            }
        }
        return;
    }
    for k in tree.taffy.children(id).unwrap_or_default() {
        let Ok(kl) = tree.taffy.layout(k) else { continue };
        let (cx, cy) = crate::col_shift(tree, k);
        walk(tree, k, x + shx + cx + kl.location.x, y + shy + cy + kl.location.y, out);
    }
}

/// The text box nearest the pointer: vertical distance first, then horizontal; later boxes paint on top, so a
/// tie goes to the later one. `pointer(i)` is the pointer in box `i`'s space. `(index, inside)` — `inside`
/// when the pointer is within the box's line-box rectangle.
pub fn pick(boxes: &[TextBox], pointer: &dyn Fn(usize) -> (f32, f32)) -> Option<(usize, bool)> {
    let mut pick: Option<(usize, f32, f32)> = None; // (box, dy, dx)
    for (i, b) in boxes.iter().enumerate() {
        let (px, py) = pointer(i);
        let dy = if py < b.top {
            b.top - py
        } else if py >= b.bottom {
            py - b.bottom + 0.001
        } else {
            0.0
        };
        let dx = if px < b.x {
            b.x - px
        } else if px > b.x + b.w {
            px - b.x - b.w
        } else {
            0.0
        };
        if pick.is_none_or(|(_, pdy, pdx)| dy < pdy || (dy == pdy && dx <= pdx)) {
            pick = Some((i, dy, dx));
        }
    }
    pick.map(|(i, dy, dx)| (i, dy == 0.0 && dx == 0.0))
}

/// Where a pointer lands in the text: the boundary point nearest to it, the word around the grapheme under it
/// (two points in the same Text node), the block that holds the text, and — for a caller that keeps its own
/// text model — the box picked and the fragment byte offsets.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub point: Point,
    pub word: (Point, Point),
    /// The IFC root's element, else the text's nearest block-level element ancestor.
    pub block: Option<usize>,
    /// The picked box's index in the slice given.
    pub index: usize,
    /// The pointer is over the box itself (not merely nearest to it).
    pub inside: bool,
    /// Byte offsets into the fragment's text: the nearest grapheme boundary, the grapheme under the pointer.
    pub near: usize,
    pub under: usize,
}

/// THE hit test both rings call. `pointer(i)` is the pointer in box `i`'s space; `caret(i, text, b)` the x of
/// the caret before byte `b` of box `i`'s fragment `text`, from its pen origin; `id_of` a DOM node's arena
/// index.
pub fn hit<H: Host>(
    tree: &LayoutTree<H>,
    boxes: &[TextBox],
    pointer: impl Fn(usize) -> (f32, f32),
    caret: impl Fn(usize, &str, usize) -> f32,
    id_of: impl Fn(&H::Node) -> usize,
) -> Option<Hit> {
    if boxes.is_empty() {
        return None;
    }
    let (i, inside) = pick(boxes, &pointer)?;
    let b = &boxes[i];
    let il = tree.inline.get(&b.ifc)?;
    let Frag::Text { text, .. } = il.frags.get(b.frag)? else { return None };
    let dn = tree.node_map.get(&b.node)?;
    let (px, _) = pointer(i);
    let (near, under) = hit_text(text, px - b.x, |k| caret(i, text, k));
    // Map the fragment's bytes to the node's offsets: align every fragment of this node in the IFC.
    let idxs: Vec<usize> = il
        .frags
        .iter()
        .enumerate()
        .filter(|(_, f)| matches!(f, Frag::Text { node, .. } if *node == b.node))
        .map(|(k, _)| k)
        .collect();
    let texts: Vec<&str> = idxs
        .iter()
        .map(|&k| match &il.frags[k] {
            Frag::Text { text, .. } => text.as_str(),
            _ => "",
        })
        .collect();
    let data = dn.text();
    let maps = align(&data, &texts, collapses_at(tree, b.node));
    let k = idxs.iter().position(|&f| f == b.frag)?;
    let char_ix = |byte: usize| text[..byte.min(text.len())].chars().count();
    let to16 = |byte: usize| maps[k][char_ix(byte).min(maps[k].len() - 1)];
    let id = id_of(dn);
    let point = Point { node: id, offset: to16(near) };
    let u = utf16_to_byte(&data, to16(under));
    let (ws, we) = word_at(&data, u);
    let word = (Point { node: id, offset: byte_to_utf16(&data, ws) }, Point { node: id, offset: byte_to_utf16(&data, we) });
    let mut block = tree.node_map.get(&b.ifc).map(&id_of);
    if block.is_none() {
        let mut n = dn.parent();
        while let Some(p) = n {
            if p.is_element() && !crate::is_inline(&p.tag().unwrap_or_default()) {
                block = Some(id_of(&p));
                break;
            }
            n = p.parent();
        }
    }
    Some(Hit { point, word, block, index: i, inside, near, under })
}
