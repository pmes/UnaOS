//! Vertical margin collapsing (CSS 2.2 §8.3.1) over Aether's box tree.
//!
//! Aether lays a block container out as a column flex box, and flex items'
//! margins never collapse, so adjacent block margins would ADD (two 16px
//! margins gave a 32px gap). This pass computes the collapsed margins the
//! block formatting context would produce and writes them into the column
//! boxes' styles for one layout run; [`restore`] puts the specified margins
//! back afterwards, so the cascade, getComputedStyle and the next pass all
//! see author values, never collapsed ones.
//!
//! The rules implemented (§8.3.1):
//! - adjoining margins of in-flow block-level siblings collapse;
//! - a block's top margin collapses with its first in-flow child's top
//!   margin when the block has no top border, no top padding, and is not a
//!   formatting-context root; likewise bottom/last child when the block's
//!   height is `auto` and it has no bottom border or padding;
//! - an empty block (no in-flow content, no border/padding, zero/auto
//!   height) collapses its own top and bottom margins together, and the
//!   result collapses through with its neighbours;
//! - a collapsed margin is the largest positive plus the most negative;
//! - the root element's margins do not collapse; nor do those of
//!   absolutely positioned boxes, flex items, inline-level boxes, or boxes
//!   with `overflow` other than visible (each is its own formatting root).
use super::LayoutTree;
use taffy::prelude::*;
use taffy::style::{FlexDirection, LengthPercentageAuto, Position};

/// A set of adjoining margins: the largest positive and most negative.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub(crate) struct Adjoin {
    pos: f32,
    neg: f32,
}

impl Adjoin {
    fn of(m: f32) -> Self {
        Adjoin { pos: m.max(0.0), neg: m.min(0.0) }
    }
    fn with(self, o: Adjoin) -> Self {
        Adjoin { pos: self.pos.max(o.pos), neg: self.neg.min(o.neg) }
    }
    /// The collapsed margin width.
    pub(crate) fn value(self) -> f32 {
        self.pos + self.neg
    }
}

/// Specified vertical margins of the boxes this pass rewrote.
pub(crate) type Saved = Vec<(NodeId, LengthPercentageAuto, LengthPercentageAuto)>;

/// A definite px margin, or None (percent/auto: not collapsible here —
/// vertical `auto` is 0 in block flow; percentages resolve against a width
/// this pre-layout pass does not know, so those boxes keep their margins).
fn px(m: LengthPercentageAuto) -> Option<f32> {
    use taffy::style::CompactLength;
    let raw: CompactLength = m.into_raw();
    if raw.tag() == CompactLength::LENGTH_TAG {
        Some(raw.value())
    } else if raw.is_auto() {
        Some(0.0)
    } else {
        None
    }
}

fn zero_lp(v: taffy::style::LengthPercentage) -> bool {
    let raw = v.into_raw();
    raw.tag() == taffy::style::CompactLength::LENGTH_TAG && raw.value() == 0.0
}

fn tag_of(tree: &LayoutTree, id: NodeId) -> Option<String> {
    tree.node_map.get(&id).and_then(|n| n.as_element().map(|e| e.name.local.as_ref().to_string()))
}

/// Boxes that are block containers in Aether's approximation: a column
/// box that is not a real flex container, not a table part, not clipped.
fn is_block_container(tree: &LayoutTree, id: NodeId, st: &Style) -> bool {
    if st.display == Display::None || st.flex_direction != FlexDirection::Column {
        return false;
    }
    let paint = tree.paint_map.get(&id);
    if paint.and_then(|p| p.flex_container) == Some(true) {
        return false;
    }
    match tree.node_map.get(&id) {
        Some(n) => {
            let Some(el) = n.as_element() else { return n.as_document().is_some() };
            let tag = el.name.local.as_ref();
            !super::is_table_part(tag) && !super::is_inline_node(n)
        }
        // Anonymous boxes are rows; a column without a DOM node is not ours.
        None => false,
    }
}

pub(crate) fn is_block_container_pub(tree: &LayoutTree, id: NodeId, st: &Style) -> bool {
    is_block_container(tree, id, st)
}

pub(crate) fn in_flow_block_pub(tree: &LayoutTree, id: NodeId, st: &Style) -> bool {
    in_flow_block(tree, id, st)
}

/// True when this box is an in-flow block-level participant of its
/// parent's block formatting context.
fn in_flow_block(tree: &LayoutTree, id: NodeId, st: &Style) -> bool {
    if st.display == Display::None || st.position == Position::Absolute {
        return false;
    }
    match tree.node_map.get(&id) {
        Some(n) => {
            if n.as_text().is_some() {
                return false;
            }
            !super::is_inline_node(n)
        }
        None => true, // anonymous block box around an inline run
    }
}

/// Collapses the margins of `id`'s subtree. `bfc_child` is true when `id`
/// is an in-flow block of a block container (so its own margins may join
/// its children's). Returns `id`'s effective (top, bottom) margins: its
/// own, collapsed with whatever escaped from its first/last children, or
/// None for a side that is not collapsible.
fn walk(
    tree: &mut LayoutTree,
    id: NodeId,
    bfc_child: bool,
    saved: &mut Saved,
) -> (Option<Adjoin>, Option<Adjoin>, bool) {
    let Ok(st) = tree.taffy.style(id).cloned() else { return (None, None, false) };
    let container = is_block_container(tree, id, &st);
    let kids = tree.taffy.children(id).unwrap_or_default();
    let tag = tag_of(tree, id);
    let clipped = tree.paint_map.get(&id).and_then(|p| p.clip).unwrap_or(false);
    // Formatting-context roots keep their margins apart from their children.
    let joins_children = bfc_child
        && container
        && !clipped
        && st.position != Position::Absolute
        && tag.as_deref().is_some_and(|t| t != "html");

    // Children first: their effective margins, then the sibling walk.
    let mut eff: Vec<(NodeId, Option<Adjoin>, Option<Adjoin>, bool, bool)> = Vec::new();
    for &k in &kids {
        let kst = tree.taffy.style(k).cloned().ok();
        let block = container && kst.as_ref().is_some_and(|s| in_flow_block(tree, k, s));
        let (t, b, empty) = walk(tree, k, block, saved);
        eff.push((k, t, b, empty, block));
    }

    let own_top = px(st.margin.top).map(Adjoin::of);
    let own_bot = px(st.margin.bottom).map(Adjoin::of);
    let mut top_out = own_top;
    let mut bot_out = own_bot;
    let mut is_empty_box = false;

    if container {
        let top_open = joins_children && zero_lp(st.padding.top) && zero_lp(st.border.top);
        let height_auto = st.size.height.is_auto()
            && (st.min_size.height.is_auto() || px(st.min_size.height) == Some(0.0));
        let bot_open = joins_children && height_auto && zero_lp(st.padding.bottom) && zero_lp(st.border.bottom);

        let mut set: Vec<(NodeId, f32, f32)> = Vec::new(); // (child, new top, new bottom)
        let mut pending: Option<Adjoin> = None; // margins awaiting the next block
        let mut prev: Option<usize> = None; // index in `set` of the previous block
        let mut seen_content = false;
        let mut broke = false; // a non-collapsible margin interrupted the chain
        for &(k, t, b, empty, block) in &eff {
            if !block {
                // An inline-level or out-of-flow box between blocks: an
                // in-flow inline box is content and separates the margins.
                let in_flow = tree
                    .taffy
                    .style(k)
                    .is_ok_and(|s| s.display != Display::None && s.position != Position::Absolute);
                if in_flow {
                    if let (Some(p), Some(i)) = (pending.take(), prev) {
                        set[i].2 = p.value();
                    }
                    prev = None;
                    seen_content = true;
                }
                continue;
            }
            let (Some(t), Some(b)) = (t, b) else {
                // Percentage margins: leave this child as specified and
                // close the chain around it.
                if let (Some(p), Some(i)) = (pending.take(), prev) {
                    set[i].2 = p.value();
                }
                prev = None;
                seen_content = true;
                broke = true;
                continue;
            };
            if empty {
                // Collapses through: its margins join the running set.
                pending = Some(pending.map_or(t.with(b), |p| p.with(t).with(b)));
                set.push((k, 0.0, 0.0));
                continue;
            }
            let joined = pending.map_or(t, |p| p.with(t));
            if !seen_content && !broke && top_open {
                // First in-flow block: its top margin escapes to the parent.
                top_out = Some(own_top.unwrap_or_default().with(joined));
                set.push((k, 0.0, 0.0));
            } else {
                if let Some(i) = prev {
                    set[i].2 = 0.0;
                }
                set.push((k, joined.value(), 0.0));
            }
            seen_content = true;
            pending = Some(b);
            prev = Some(set.len() - 1);
        }
        if !seen_content && !broke && top_open && bot_open && own_top.is_some() && own_bot.is_some() {
            // An empty box: own top and bottom and all children's collapse.
            is_empty_box = true;
            let all = pending.map_or(Adjoin::default(), |p| p);
            top_out = Some(own_top.unwrap().with(own_bot.unwrap()).with(all));
            bot_out = Some(Adjoin::default());
        } else if let Some(p) = pending {
            if bot_open && !broke {
                bot_out = Some(own_bot.unwrap_or_default().with(p));
                if let Some(i) = prev {
                    set[i].2 = 0.0;
                }
            } else if let Some(i) = prev {
                set[i].2 = p.value();
            }
        }
        for (k, t, b) in set {
            write_margins(tree, k, t, b, saved);
        }
    } else if kids.is_empty() {
        // A leaf element with no content: empty if nothing gives it height.
        let leaf_el = tree.node_map.get(&id).and_then(|n| n.as_element().map(|e| e.name.local.as_ref().to_string()));
        is_empty_box = bfc_child
            && leaf_el.is_some_and(|t| !matches!(t.as_str(), "img" | "input" | "select" | "textarea" | "button" | "hr" | "svg" | "canvas" | "video" | "iframe" | "br"))
            && st.size.height.is_auto()
            && (st.min_size.height.is_auto() || px(st.min_size.height) == Some(0.0))
            && zero_lp(st.padding.top)
            && zero_lp(st.padding.bottom)
            && zero_lp(st.border.top)
            && zero_lp(st.border.bottom);
    }
    if !bfc_child {
        // Not part of a block flow: its margins stand as specified.
        return (own_top, own_bot, false);
    }
    (top_out, bot_out, is_empty_box)
}

fn write_margins(tree: &mut LayoutTree, id: NodeId, top: f32, bottom: f32, saved: &mut Saved) {
    let Ok(st) = tree.taffy.style(id) else { return };
    let (t0, b0) = (st.margin.top, st.margin.bottom);
    if px(t0) == Some(top) && px(b0) == Some(bottom) {
        return;
    }
    let mut st = st.clone();
    st.margin.top = LengthPercentageAuto::length(top);
    st.margin.bottom = LengthPercentageAuto::length(bottom);
    saved.push((id, t0, b0));
    let _ = tree.taffy.set_style(id, st);
}

/// Rewrites the box tree's vertical margins to their collapsed values.
/// Returns what [`restore`] needs to put the specified margins back.
pub(crate) fn apply(tree: &mut LayoutTree) -> Saved {
    let mut saved = Saved::new();
    let root = tree.root_node;
    walk(tree, root, false, &mut saved);
    saved
}

/// Puts back the specified margins [`apply`] replaced. The computed layout
/// stays; only the styles return to author values.
pub(crate) fn restore(tree: &mut LayoutTree, saved: Saved) {
    for (id, t, b) in saved.into_iter().rev() {
        if let Ok(st) = tree.taffy.style(id) {
            let mut st = st.clone();
            st.margin.top = t;
            st.margin.bottom = b;
            let _ = tree.taffy.set_style(id, st);
        }
    }
}
