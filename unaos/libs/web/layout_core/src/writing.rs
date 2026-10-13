//! AETHERSTYLE3 (SR103): writing modes (css-writing-modes-4). `writing-mode` is inherited; a box in a vertical
//! mode has a vertical inline axis — its lines run top to bottom and stack right to left (`vertical-rl`,
//! `sideways-rl`) or left to right (`vertical-lr`, `sideways-lr`).
//!
//! * An IFC root in a vertical mode lays its lines out along its INLINE size — its own definite height, else
//!   its vertical parent's definite content height (a block box fills its containing block's inline size),
//!   else the viewport height (§7.3: an orthogonal flow's available inline size falls back to the initial
//!   containing block) — in the inline formatter's usual horizontal space; `InlineLayout::vertical` marks it,
//!   the inline loop sizes the box physically (width = the lines' block extent, height = the inline size or the
//!   longest line), and each ring's painter maps the fragments onto the box with the mode's rotation
//!   ([`to_physical`]).
//! * A block container in a vertical mode stacks its block children along the horizontal block axis: it is laid
//!   out as a row (`vertical-lr`) or row-reverse (`vertical-rl`) flex box whose children size their width from
//!   content ([`apply`] / [`restore`], the blockwidth shape: styles saved and put back after the passes).
//! * A vertical box in a horizontal parent (an orthogonal flow) is as wide as its content, not its parent.
//!
//! Logical properties map through the element's own `writing-mode` and `direction` (`css::apply_logical`).
//!
//! AETHERSTYLE3B (SR115): in `vertical-rl` / `vertical-lr`, a glyph whose Vertical_Orientation (UAX #50) is
//! upright — CJK ideographs, kana, Hangul, Bopomofo, Yi, the CJK symbols and full-width forms — under
//! `text-orientation: mixed`, and every glyph under `upright`, is not turned with its line: the inline formatter
//! records its staging cell ([`UprightCell`], `InlineLayout::upright`) and the painters map that cell's pixels
//! by translation into its place in the column ([`to_physical_upright`]) instead of the line's rotation. The
//! inline advance stays the horizontal one (a full-width glyph's advance is its 1em vertical advance; a
//! proportional glyph forced upright keeps its own width as its cell). Pointer hit testing maps a physical
//! point back into the staging space ([`to_logical`]), which is what makes selection work in vertical text.
//!
//! AETHERSTYLE3C (SR122): `text-combine-upright: all` (§9.1) lays a run out as ONE upright cell 1em long in the
//! inline direction; its glyphs keep their horizontal axis and are compressed horizontally to fit
//! ([`CombinedCell`]: the cell and its scale). Under `text-orientation: upright` a glyph that is not natively
//! upright (Latin, digits) takes a 1em vertical advance, centred in its cell (the font's vertical metrics; a
//! full-width glyph already advances 1em). A caret in vertical text is a horizontal bar across its line's
//! column ([`caret_rect`]; `hit::caret_rect` maps a text box's caret offset into the box's physical space).

use crate::prelude::*;
use crate::tree::TextRun;
use crate::{Host, LayoutTree, NodeMap};
use taffy::prelude::*;

pub const HORIZONTAL_TB: u8 = 0;
pub const VERTICAL_RL: u8 = 1;
pub const VERTICAL_LR: u8 = 2;
pub const SIDEWAYS_RL: u8 = 3;
pub const SIDEWAYS_LR: u8 = 4;

/// `writing-mode` keyword → code (the SVG 1.1 / css3-writing-modes legacy spellings included).
pub fn parse_writing_mode(v: &str) -> Option<u8> {
    Some(match v {
        "horizontal-tb" | "lr" | "lr-tb" | "rl" | "rl-tb" => HORIZONTAL_TB,
        "vertical-rl" | "tb" | "tb-rl" => VERTICAL_RL,
        "vertical-lr" | "tb-lr" => VERTICAL_LR,
        "sideways-rl" => SIDEWAYS_RL,
        "sideways-lr" => SIDEWAYS_LR,
        _ => return None,
    })
}

#[inline]
pub fn is_vertical(mode: u8) -> bool {
    mode != HORIZONTAL_TB
}

/// The block axis runs right to left.
#[inline]
pub fn block_rtl(mode: u8) -> bool {
    matches!(mode, VERTICAL_RL | SIDEWAYS_RL)
}

/// The physical side names of the flow-relative sides (css-writing-modes-4 §6.4): (inline-start, inline-end,
/// block-start, block-end) for `mode` and `rtl` (the `direction`).
pub fn logical_sides(mode: u8, rtl: bool) -> (&'static str, &'static str, &'static str, &'static str) {
    match mode {
        HORIZONTAL_TB => {
            if rtl {
                ("right", "left", "top", "bottom")
            } else {
                ("left", "right", "top", "bottom")
            }
        }
        SIDEWAYS_LR => {
            // the inline axis runs bottom to top
            if rtl {
                ("top", "bottom", "left", "right")
            } else {
                ("bottom", "top", "left", "right")
            }
        }
        m => {
            let (bs, be) = if block_rtl(m) { ("right", "left") } else { ("left", "right") };
            if rtl {
                ("bottom", "top", bs, be)
            } else {
                ("top", "bottom", bs, be)
            }
        }
    }
}

/// Where a pixel (x, y) of a vertical IFC's horizontal staging layout lands in its physical content box:
/// `block` is the lines' block extent (the box's content width), `inline` the inline extent (its content
/// height), `line_of(y)` the (top, height) of the line holding staging row y (`vertical-lr` flips each line
/// back into place). Integer pixels: a bijection of the staging rectangle onto the content box.
pub fn to_physical(mode: u8, x: i32, y: i32, block: i32, inline: i32, line_of: &dyn Fn(i32) -> Option<(i32, i32)>) -> (i32, i32) {
    match mode {
        SIDEWAYS_LR => (y, inline - 1 - x),
        VERTICAL_LR => match line_of(y) {
            // lines left to right, each line's glyphs still turned clockwise
            Some((t, h)) => (2 * t + h - 1 - y, x),
            None => (y, x),
        },
        _ => (block - 1 - y, x),
    }
}

/// A physical point (x, y) in a vertical IFC's content box → the staging-space point it shows (the inverse of
/// [`to_physical`], continuous): `block` / `inline` as there, `lines` the IFC's line boxes (top, height).
pub fn to_logical(mode: u8, x: f32, y: f32, block: f32, inline: f32, lines: &[(f32, f32)]) -> (f32, f32) {
    match mode {
        SIDEWAYS_LR => (inline - y, x),
        VERTICAL_LR => match lines.iter().find(|(t, h)| x >= *t && x < t + h) {
            Some(&(t, h)) => (y, 2.0 * t + h - x),
            None => (y, x),
        },
        _ => (y, block - x),
    }
}

/// UAX #50 Vertical_Orientation U / Tu (upright in vertical text) for the ranges a page sets upright: CJK and
/// its punctuation, kana, Hangul, Bopomofo, Yi, the compatibility and full-width forms, the supplementary
/// ideographic planes.
pub fn is_upright_char(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF | 0x2E80..=0x2FFF | 0x3000..=0x303F | 0x3040..=0x30FF | 0x3100..=0x31FF | 0x3200..=0x33FF
        | 0x3400..=0x4DBF | 0x4DC0..=0x4DFF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xA960..=0xA97F | 0xAC00..=0xD7FF
        | 0xF900..=0xFAFF | 0xFE10..=0xFE1F | 0xFE30..=0xFE6F | 0xFF01..=0xFF60 | 0xFF61..=0xFFDC | 0xFFE0..=0xFFE6
        | 0x1F200..=0x1F2FF | 0x20000..=0x3FFFD)
}

/// Whether `c` stands upright in a vertical line under `text-orientation` `orientation` (0 mixed, 1 upright,
/// 2 sideways; css-writing-modes-4 §5.1). Spaces never get a cell.
pub fn upright(c: char, orientation: u8) -> bool {
    if c.is_whitespace() {
        return false;
    }
    match orientation {
        1 => true,
        2 => false,
        _ => is_upright_char(c),
    }
}

/// One upright glyph's cell in a vertical IFC's staging space: its inline start and advance, and its line box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UprightCell {
    pub x: f32,
    pub w: f32,
    pub top: f32,
    pub h: f32,
}

/// AETHERSTYLE3C (SR122): one `text-combine-upright: all` run of a vertical IFC: its 1em cell (in the IFC's
/// `upright` cells too) and the horizontal scale (<= 1) the painter draws its glyphs at so they fit the cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CombinedCell {
    pub cell: UprightCell,
    pub scale: f32,
}

/// AETHERSTYLE3C (SR122): the caret before staging x `x` on the line (`top`, `h`) of a vertical IFC (`block` /
/// `inline` as in [`to_physical`]) as a physical rectangle (x, y, w, h) in its content box: a 1px bar across
/// the line's column at the inline position. A horizontal root's caret is the usual vertical bar.
pub fn caret_rect(mode: u8, x: f32, top: f32, h: f32, block: f32, inline: f32) -> (f32, f32, f32, f32) {
    match mode {
        HORIZONTAL_TB => (x, top, 1.0, h),
        VERTICAL_LR => (top, x, h, 1.0),
        SIDEWAYS_LR => (top, inline - x, h, 1.0),
        _ => (block - top - h, x, h, 1.0),
    }
}

/// Where staging pixel (x, y) lands when it lies in an upright cell (None otherwise, or in a sideways mode,
/// where text-orientation does not apply): the cell is translated, not turned — its glyph's horizontal axis
/// becomes the column's, centred in the column (`h` wide) and in the cell's `w` of inline length.
pub fn to_physical_upright(mode: u8, x: i32, y: i32, block: i32, cells: &[UprightCell]) -> Option<(i32, i32)> {
    if !matches!(mode, VERTICAL_RL | VERTICAL_LR) {
        return None;
    }
    for c in cells {
        let (cx0, cx1) = (c.x.floor() as i32, (c.x + c.w).floor() as i32);
        let (ct, cb) = (c.top.floor() as i32, (c.top + c.h).floor() as i32);
        if x < cx0 || x >= cx1 || y < ct || y >= cb {
            continue;
        }
        let (cw, ch) = (cx1 - cx0, cb - ct);
        let col_left = if mode == VERTICAL_RL { block - cb } else { ct };
        let pad = (ch - cw) / 2;
        return Some((col_left + pad + (x - cx0), cx0 + (y - ct) - pad));
    }
    None
}

/// The available inline size of a vertical IFC root and whether the box fills it (a block-level box in a
/// vertical parent with a definite inline size) — see the module notes.
pub(crate) fn inline_avail<H: Host>(tree: &LayoutTree<H>, elems: &NodeMap<TextRun>, root: NodeId) -> (f32, bool) {
    let definite = |id: NodeId| -> Option<f32> {
        let st = tree.taffy.style(id).ok()?;
        let raw = st.size.height.into_raw();
        if raw.tag() != taffy::style::CompactLength::LENGTH_TAG {
            return None;
        }
        let l = tree.taffy.layout(id).ok()?;
        let edges = l.padding.top + l.padding.bottom + l.border.top + l.border.bottom;
        Some(if st.box_sizing == taffy::style::BoxSizing::BorderBox { raw.value() - edges } else { raw.value() }.max(0.0))
    };
    if let Some(h) = definite(root) {
        return (h, true);
    }
    let edges = tree
        .taffy
        .layout(root)
        .map(|l| l.padding.top + l.padding.bottom + l.border.top + l.border.bottom + l.margin.top.max(0.0) + l.margin.bottom.max(0.0))
        .unwrap_or(0.0);
    if let Some(p) = tree.taffy.parent(root) {
        let pv = elems.get(&p).is_some_and(|r| is_vertical(r.writing_mode));
        if pv {
            if let Some(h) = definite(p) {
                return ((h - edges).max(0.0), true);
            }
        }
    }
    ((tree.viewport.1 - edges).max(0.0), false)
}

/// Before the block passes: every vertical box's flow is set up (see the module notes). Returns the styles to
/// put back after the passes, in the order they were first changed.
pub(crate) fn apply<H: Host>(tree: &mut LayoutTree<H>, elems: &NodeMap<TextRun>) -> Vec<(NodeId, Style)> {
    let mut saved: Vec<(NodeId, Style)> = Vec::new();
    let save = |tree: &LayoutTree<H>, saved: &mut Vec<(NodeId, Style)>, id: NodeId| {
        if !saved.iter().any(|(s, _)| *s == id) {
            if let Ok(st) = tree.taffy.style(id) {
                saved.push((id, st.clone()));
            }
        }
    };
    let vertical: Vec<(NodeId, u8)> = elems
        .iter()
        .filter(|(id, r)| is_vertical(r.writing_mode) && tree.node_map.get(id).is_some_and(|n| crate::DomNode::is_element(n)))
        .map(|(id, r)| (*id, r.writing_mode))
        .collect();
    for (id, mode) in vertical {
        let has_width = tree.paint_map.get(&id).and_then(|p| p.has_width) == Some(true);
        let parent_vertical = tree.taffy.parent(id).and_then(|p| elems.get(&p)).is_some_and(|r| is_vertical(r.writing_mode));
        // an orthogonal flow: as wide as its content
        if !parent_vertical && !has_width {
            save(tree, &mut saved, id);
            if let Ok(st) = tree.taffy.style(id) {
                let mut st = st.clone();
                st.size.width = Dimension::auto();
                st.align_self = Some(AlignSelf::FLEX_START);
                let _ = tree.taffy.set_style(id, st);
            }
        }
        if crate::inline::is_ifc_root(tree, id) {
            continue;
        }
        let p = tree.paint_map.get(&id);
        if p.and_then(|p| p.flex_container) == Some(true)
            || tree.taffy.style(id).is_ok_and(|s| s.display != Display::Flex)
            || tree.node_map.get(&id).and_then(|n| crate::DomNode::tag(n)).is_some_and(|t| crate::tree::is_table_part(&t))
        {
            continue;
        }
        save(tree, &mut saved, id);
        if let Ok(st) = tree.taffy.style(id) {
            let mut st = st.clone();
            st.flex_direction = if block_rtl(mode) { FlexDirection::RowReverse } else { FlexDirection::Row };
            st.flex_wrap = taffy::style::FlexWrap::NoWrap;
            st.align_items = AlignItems::STRETCH;
            let _ = tree.taffy.set_style(id, st);
        }
        for k in tree.taffy.children(id).unwrap_or_default() {
            if tree.paint_map.get(&k).and_then(|p| p.has_width) == Some(true) {
                continue;
            }
            save(tree, &mut saved, k);
            if let Ok(st) = tree.taffy.style(k) {
                let mut st = st.clone();
                st.size.width = Dimension::auto();
                st.flex_shrink = 0.0;
                let _ = tree.taffy.set_style(k, st);
            }
        }
    }
    saved
}

/// After the passes: the saved styles go back (the computed layout stays).
pub(crate) fn restore<H: Host>(tree: &mut LayoutTree<H>, saved: Vec<(NodeId, Style)>) {
    for (id, st) in saved.into_iter().rev() {
        let _ = tree.taffy.set_style(id, st);
    }
}
