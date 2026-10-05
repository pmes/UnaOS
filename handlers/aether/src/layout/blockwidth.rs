//! Block-level widths (CSS 2.2 §10.3.3) over Aether's column boxes.
//!
//! In normal flow a block box with `width: auto` fills its containing
//! block: `margin-left + border + padding + width + margin-right` equals
//! the container's content width. Aether's builder gives every block the
//! placeholder `width: 100%`, which taffy reads as "the whole container,
//! THEN add margins/padding/border" — every padded or margined block
//! overflowed its parent on the right. This pass turns the placeholder
//! back into `auto` + `align-self: stretch` (the flex expression of
//! §10.3.3) for the layout run, and restores it after.
//!
//! `margin: auto` horizontally (centering) needs a used width to centre:
//! with a `max-width` the placeholder stays (100% clamped by max-width,
//! the auto margins split the rest — §10.3.3's over-constrained case);
//! without one, CSS resolves both auto margins to 0 and the box fills.
use super::LayoutTree;
use taffy::prelude::*;
use taffy::style::{AlignSelf, LengthPercentageAuto, Position};

pub(crate) type Saved = Vec<(NodeId, Style)>;

fn walk(tree: &mut LayoutTree, id: NodeId, saved: &mut Saved) {
    let Ok(st) = tree.taffy.style(id).cloned() else { return };
    let kids = tree.taffy.children(id).unwrap_or_default();
    if super::collapse::is_block_container_pub(tree, id, &st) {
        for &k in &kids {
            let Ok(kst) = tree.taffy.style(k).cloned() else { continue };
            if !super::collapse::in_flow_block_pub(tree, k, &kst)
                || kst.position == Position::Absolute
                || kst.size.width != Dimension::percent(1.0)
            {
                continue;
            }
            let auto_l = kst.margin.left == LengthPercentageAuto::auto();
            let auto_r = kst.margin.right == LengthPercentageAuto::auto();
            if (auto_l || auto_r) && !kst.max_size.width.is_auto() {
                continue; // centred under a max-width: keep the clamped 100%
            }
            let mut n = kst.clone();
            n.size.width = Dimension::auto();
            if auto_l {
                n.margin.left = LengthPercentageAuto::length(0.0);
            }
            if auto_r {
                n.margin.right = LengthPercentageAuto::length(0.0);
            }
            if n.align_self.is_none() {
                n.align_self = Some(AlignSelf::STRETCH);
            }
            saved.push((k, kst));
            let _ = tree.taffy.set_style(k, n);
        }
    }
    for k in kids {
        walk(tree, k, saved);
    }
}

pub(crate) fn apply(tree: &mut LayoutTree) -> Saved {
    let mut saved = Saved::new();
    let root = tree.root_node;
    walk(tree, root, &mut saved);
    saved
}

/// Restores width/margins/align-self only: the collapse pass, which runs
/// after this one, restores its own vertical margins first.
pub(crate) fn restore(tree: &mut LayoutTree, saved: Saved) {
    for (id, old) in saved.into_iter().rev() {
        if let Ok(st) = tree.taffy.style(id) {
            let mut st = st.clone();
            st.size.width = old.size.width;
            st.margin.left = old.margin.left;
            st.margin.right = old.margin.right;
            st.align_self = old.align_self;
            let _ = tree.taffy.set_style(id, st);
        }
    }
}
