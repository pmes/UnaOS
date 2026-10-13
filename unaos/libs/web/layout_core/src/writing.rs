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
