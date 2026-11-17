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
//! A definite width wider than the container is over-constrained: [`auto_overflow`] (SR1065) zeroes its auto margins.
//!
//! ABSSHRINK (LEDGER SR420): an absolutely positioned / fixed box with no author width gets the same
//! placeholder-to-`auto` turn ([`abs_shrink`]), so taffy's own abspos auto size answers CSS 2.2 §10.3.7.
use crate::prelude::*;
use crate::tree::LayoutTree;
use crate::Host;
use taffy::prelude::*;
use taffy::style::{AlignSelf, LengthPercentageAuto, Position};

pub(crate) type Saved = Vec<(NodeId, Style)>;

fn walk<H: Host>(tree: &mut LayoutTree<H>, id: NodeId, saved: &mut Saved) {
    let Ok(st) = tree.taffy.style(id).cloned() else { return };
    let kids = tree.taffy.children(id).unwrap_or_default();
    let block = crate::collapse::is_block_container_pub(tree, id, &st);
    for &k in &kids { abs_shrink(tree, k, block, saved); } // ABSSHRINK (SR420)
    if block {
        for &k in &kids {
            let Ok(kst) = tree.taffy.style(k).cloned() else { continue };
            if !crate::collapse::in_flow_block_pub(tree, k, &kst)
                || kst.position == Position::Absolute
                || kst.size.width != Dimension::percent(1.0)
            {
                continue;
            }
            let auto_l = kst.margin.left == LengthPercentageAuto::auto();
            let auto_r = kst.margin.right == LengthPercentageAuto::auto();
            if (auto_l || auto_r) && (!kst.max_size.width.is_auto() || shrink_table(tree, k)) {
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

pub(crate) fn apply<H: Host>(tree: &mut LayoutTree<H>) -> Saved {
    let mut saved = Saved::new();
    let root = tree.root_node;
    walk(tree, root, &mut saved);
    saved
}

/// Restores width/margins/align-self only: the collapse pass, which runs
/// after this one, restores its own vertical margins first.
pub(crate) fn restore<H: Host>(tree: &mut LayoutTree<H>, saved: Saved) {
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

/// ABSSHRINK (LEDGER SR420): CSS 2.2 §10.3.7 / css-position-3 §5.1. An absolutely positioned or fixed box whose
/// width is the builder's `100%` placeholder (no author `width`, `PaintStyle::has_width`) is handed to taffy as
/// `auto` for the run: taffy's abspos auto size (`layout_oof_box`) then stretches it only between two non-auto
/// insets and otherwise measures it shrink-to-fit against the stretch-fit size — the placeholder made it the
/// whole containing block (a `right: 8px` box at `x = -8`). The static-position fallback (`apply_specs` keeps a
/// box with no inset in flow, taffy `Relative`) under a block container takes `auto` + `align-self: start`: a
/// column item that does not stretch is taffy's own fit-content cross size. The original style joins the same
/// [`Saved`], so [`restore`] and the kept-tree undo put the placeholder back.
fn abs_shrink<H: Host>(tree: &mut LayoutTree<H>, k: NodeId, block: bool, saved: &mut Saved) {
    let Some(p) = tree.paint_map.get(&k) else { return };
    if !matches!(p.position_kind, Some(2 | 3)) || p.has_width == Some(true) {
        return;
    }
    let Ok(kst) = tree.taffy.style(k).cloned() else { return };
    if kst.size.width != Dimension::percent(1.0) {
        return;
    }
    let absolute = kst.position == Position::Absolute;
    if !absolute && !block {
        return;
    }
    let mut n = kst.clone();
    n.size.width = Dimension::auto();
    if !absolute && n.align_self.is_none() {
        n.align_self = Some(AlignSelf::START);
    }
    saved.push((k, kst));
    let _ = tree.taffy.set_style(k, n);
}

// ---- AETHERMARGINAUTOOVERFLOW (LEDGER SR1065): an over-constrained auto margin is 0, the box overflows to the end side (named tail block) ----

/// AETHERMARGINAUTOOVERFLOW (SR1065): CSS 2.2 §10.3.3: "if 'width' is not 'auto' and 'border-left-width' +
/// 'padding-left' + 'width' + 'padding-right' + 'border-right-width' (plus any of 'margin-left' or 'margin-right'
/// that are not 'auto') is larger than the width of the containing block, then any 'auto' values for 'margin-left'
/// or 'margin-right' are, for the following rules, treated as zero". Aether's blocks are taffy flex columns, and
/// taffy 0.15's `resolve_cross_axis_auto_margins` hands an auto cross margin the NEGATIVE free space: a 600px
/// `margin: 0 auto` box in a 400px column sat at x = -100 (`margin-left: auto` alone at -200). The containing
/// block's width is a layout output, so this runs after the first taffy pass(es) (`passes`, then one more
/// `run_pass` when it returns true): each in-flow block kid of a block container with an auto horizontal margin
/// whose outer width (its non-auto margins resolved by taffy) exceeds the container's content width gets every auto
/// horizontal margin as 0. ltr: the box sits at its margin-left; rtl: `place_narrow_blocks`' align-self END, which
/// taffy ignores while a margin is auto, now places it at its margin-right from the right edge (it overflows to the
/// left). The original style joins the same [`Saved`], so [`restore`] and the kept-tree undo put the auto margins
/// back. A table keeps its own road (AETHER-MARGIN-AUTO-OVERFLOW-TABLE owed).
pub(crate) fn auto_overflow<H: Host>(tree: &mut LayoutTree<H>, saved: &mut Saved) -> bool {
    use crate::DomNode;
    let mut changed = false;
    let mut stack = vec![tree.root_node];
    while let Some(id) = stack.pop() {
        let kids = tree.taffy.children(id).unwrap_or_default();
        stack.extend(kids.iter().copied());
        let Ok(st) = tree.taffy.style(id).cloned() else { continue };
        if !crate::collapse::is_block_container_pub(tree, id, &st) {
            continue;
        }
        let Ok(pl) = tree.taffy.layout(id).copied() else { continue };
        let cb = pl.size.width - pl.padding.left - pl.padding.right - pl.border.left - pl.border.right;
        for k in kids {
            let Ok(kst) = tree.taffy.style(k).cloned() else { continue };
            let auto_l = kst.margin.left == LengthPercentageAuto::auto();
            let auto_r = kst.margin.right == LengthPercentageAuto::auto();
            if !(auto_l || auto_r)
                || kst.position == Position::Absolute
                || !crate::collapse::in_flow_block_pub(tree, k, &kst)
                || tree.node_map.get(&k).and_then(|n| n.tag()).is_some_and(|t| t == "table")
            {
                continue;
            }
            let Ok(kl) = tree.taffy.layout(k).copied() else { continue };
            let fixed = (if auto_l { 0.0 } else { kl.margin.left }) + (if auto_r { 0.0 } else { kl.margin.right });
            if cb - kl.size.width - fixed >= -0.01 {
                continue;
            }
            let mut n = kst.clone();
            if auto_l {
                n.margin.left = LengthPercentageAuto::length(0.0);
            }
            if auto_r {
                n.margin.right = LengthPercentageAuto::length(0.0);
            }
            saved.push((k, kst));
            let _ = tree.taffy.set_style(k, n);
            changed = true;
        }
    }
    changed
}

// ---- AETHERTABLEALIGN (LEDGER SR1071): a shrink-to-fit table keeps its auto margins (named tail block) ----

/// AETHERTABLEALIGN (SR1071): a table has a used width of its own (`table::apply` fixes it in the second pass, after
/// this pass ran), so its auto horizontal margins are kept like the max-width case: taffy's auto margins then centre
/// it (`<table align=center>`, an author `margin: 0 auto`), either direction, ahead of the table's align-self.
fn shrink_table<H: Host>(tree: &LayoutTree<H>, k: NodeId) -> bool {
    use crate::DomNode;
    tree.node_map.get(&k).and_then(|n| n.tag()).is_some_and(|t| t == "table")
}
