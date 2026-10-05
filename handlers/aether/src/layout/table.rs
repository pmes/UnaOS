//! Table layout (CSS 2.2 §17.5.2.2 automatic table layout, §17.6 border
//! models) over Aether's flex-backed box tree.
//!
//! The builder lays a `tr` out as a row of content-sized cells, so cells
//! of one column never lined up and a `width: 100%` table did not widen
//! its columns. This pass runs between two layout passes: the first gives
//! every cell its max-content width (the cells are content-sized items);
//! from those it builds the column grid (colspan spreads its excess over
//! the spanned columns), takes the table width (the containing width when
//! the author set one or the columns overflow it, else the sum —
//! shrink-to-fit), distributes the difference over the columns in
//! proportion to their max-content widths, and fixes every cell's
//! border-box width to its columns. Rows stretch their cells to one
//! height. `border-collapse: collapse` overlaps adjacent cell borders
//! (each later cell/row is pulled back by the border it shares);
//! the separated model spaces cells by `border-spacing` (UA 2px), also
//! around the table's edge.
use super::LayoutTree;
use taffy::prelude::*;
use taffy::style::{AlignItems, AlignSelf, BoxSizing, FlexWrap, LengthPercentage, LengthPercentageAuto};

fn tag(tree: &LayoutTree, id: NodeId) -> Option<String> {
    tree.node_map.get(&id).and_then(|n| n.as_element().map(|e| e.name.local.as_ref().to_string()))
}

/// The tables in the tree.
pub(crate) fn tables(tree: &LayoutTree) -> Vec<NodeId> {
    tree.node_map
        .iter()
        .filter(|(_, n)| n.as_element().is_some_and(|e| e.name.local.as_ref() == "table"))
        .map(|(id, _)| *id)
        .collect()
}

/// Rows of a table in order: direct `tr`s and those of its row groups.
fn rows(tree: &LayoutTree, table: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    for k in tree.taffy.children(table).unwrap_or_default() {
        match tag(tree, k).as_deref() {
            Some("tr") => out.push(k),
            Some("thead" | "tbody" | "tfoot") => {
                out.extend(tree.taffy.children(k).unwrap_or_default().into_iter().filter(|r| tag(tree, *r).as_deref() == Some("tr")))
            }
            _ => {}
        }
    }
    out
}

fn colspan(tree: &LayoutTree, cell: NodeId) -> usize {
    tree.node_map
        .get(&cell)
        .and_then(|n| n.as_element().and_then(|e| e.attributes.borrow().get("colspan").and_then(|v| v.trim().parse::<usize>().ok())))
        .unwrap_or(1)
        .clamp(1, 1000)
}

/// Builds the UA defaults the separated model needs at build time: none —
/// spacing is applied here, after the cascade (it depends on the model).
pub(crate) fn apply(tree: &mut LayoutTree) {
    for table in tables(tree) {
        layout_table(tree, table);
    }
}

fn layout_table(tree: &mut LayoutTree, table: NodeId) {
    let rows = rows(tree, table);
    if rows.is_empty() {
        return;
    }
    let paint = tree.paint_map.get(&table).cloned().unwrap_or_default();
    let collapse = paint.border_collapse.unwrap_or(false);
    let spacing = if collapse { 0.0 } else { paint.border_spacing.unwrap_or(2.0) };

    // The grid and each column's max-content width (pass-one cell widths).
    let mut grid: Vec<Vec<(NodeId, usize, usize)>> = Vec::new(); // (cell, first col, span)
    let mut ncols = 0usize;
    for &r in &rows {
        let mut col = 0usize;
        let mut line = Vec::new();
        for c in tree.taffy.children(r).unwrap_or_default() {
            if !matches!(tag(tree, c).as_deref(), Some("td" | "th")) {
                continue;
            }
            let span = colspan(tree, c);
            line.push((c, col, span));
            col += span;
        }
        ncols = ncols.max(col);
        grid.push(line);
    }
    if ncols == 0 {
        return;
    }
    let cell_w = |tree: &LayoutTree, c: NodeId| tree.taffy.layout(c).map(|l| l.size.width).unwrap_or(0.0);
    let mut maxw = vec![0.0f32; ncols];
    for line in &grid {
        for &(c, col, span) in line {
            if span == 1 {
                maxw[col] = maxw[col].max(cell_w(tree, c));
            }
        }
    }
    for line in &grid {
        for &(c, col, span) in line {
            if span > 1 {
                let have: f32 = maxw[col..col + span].iter().sum::<f32>() + spacing * (span - 1) as f32;
                let need = cell_w(tree, c);
                if need > have {
                    let add = (need - have) / span as f32;
                    for w in &mut maxw[col..col + span] {
                        *w += add;
                    }
                }
            }
        }
    }
    // Collapsed: neighbouring cells share one border line.
    let overlap = |tree: &LayoutTree, c: NodeId| -> f32 {
        if !collapse {
            return 0.0;
        }
        tree.taffy.style(c).map(|s| s.border.left.into_raw().value()).unwrap_or(0.0)
    };
    let shared = if collapse {
        grid.iter().flat_map(|l| l.iter().skip(1)).map(|&(c, _, _)| overlap(tree, c)).fold(0.0f32, f32::max)
    } else {
        0.0
    };

    // Table width: the containing width when the author fixed it or the
    // columns overflow it, else shrink to the columns.
    let Ok(tl) = tree.taffy.layout(table).cloned() else { return };
    let edges = tl.padding.left + tl.padding.right + tl.border.left + tl.border.right;
    let avail = (tl.size.width - edges).max(0.0);
    // Collapsed cells are widened by the shared border and pulled back by
    // the same amount, so only the separated model's spacing takes room.
    let gaps = spacing * (ncols + 1) as f32;
    let sum: f32 = maxw.iter().sum::<f32>() + gaps;
    let fixed = paint.has_width == Some(true);
    let target = if fixed || sum > avail { avail } else { sum };
    let total_max: f32 = maxw.iter().sum();
    let room = (target - gaps).max(0.0);
    let widths: Vec<f32> = if total_max > 0.0 {
        maxw.iter().map(|w| w / total_max * room).collect()
    } else {
        vec![room / ncols as f32; ncols]
    };

    // Table box.
    if let Ok(st) = tree.taffy.style(table) {
        let mut st = st.clone();
        if !fixed {
            st.size.width = Dimension::length(target + edges);
            st.box_sizing = BoxSizing::BorderBox;
            if st.align_self.is_none() {
                st.align_self = Some(AlignSelf::START);
            }
        }
        let _ = tree.taffy.set_style(table, st);
    }
    // Row groups and the table space their rows by the spacing.
    let mut groups = vec![table];
    groups.extend(tree.taffy.children(table).unwrap_or_default().into_iter().filter(|k| matches!(tag(tree, *k).as_deref(), Some("thead" | "tbody" | "tfoot"))));
    for g in groups {
        if let Ok(st) = tree.taffy.style(g) {
            let mut st = st.clone();
            st.gap.height = LengthPercentage::length(spacing);
            if g == table && !collapse {
                st.padding = taffy::geometry::Rect {
                    left: LengthPercentage::length(spacing),
                    right: LengthPercentage::length(spacing),
                    top: LengthPercentage::length(0.0),
                    bottom: LengthPercentage::length(spacing),
                };
            }
            let _ = tree.taffy.set_style(g, st);
        }
    }
    for (ri, (&r, line)) in rows.iter().zip(&grid).enumerate() {
        if let Ok(st) = tree.taffy.style(r) {
            let mut st = st.clone();
            st.flex_direction = taffy::style::FlexDirection::Row;
            st.flex_wrap = FlexWrap::NoWrap;
            st.align_items = Some(AlignItems::STRETCH);
            st.gap.width = LengthPercentage::length(spacing);
            st.margin.top = LengthPercentageAuto::length(if !collapse { if ri == 0 { spacing } else { 0.0 } } else if ri == 0 { 0.0 } else { -shared });
            let _ = tree.taffy.set_style(r, st);
        }
        for (ci, &(c, col, span)) in line.iter().enumerate() {
            let mut w: f32 = widths[col..col + span].iter().sum::<f32>() + spacing * (span - 1) as f32;
            if collapse && ci > 0 {
                w += shared;
            }
            if let Ok(st) = tree.taffy.style(c) {
                let mut st = st.clone();
                st.size.width = Dimension::length(w);
                st.box_sizing = BoxSizing::BorderBox;
                st.flex_shrink = 0.0;
                st.flex_grow = 0.0;
                st.margin.left = LengthPercentageAuto::length(if collapse && ci > 0 { -shared } else { 0.0 });
                st.align_self = Some(AlignSelf::STRETCH);
                let _ = tree.taffy.set_style(c, st);
            }
        }
    }
}
