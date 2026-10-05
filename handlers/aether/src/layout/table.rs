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
//!
//! AETHERINLINE added: min-content column widths (a column never narrows
//! below its widest unbreakable content; between the min and max sums the
//! extra space goes to each column in proportion to max - min, §17.5.2.2
//! step 2 as Blink's auto table layout does), `rowspan` (the slot grid
//! skips occupied columns; a spanning cell is taken out of its row's flow,
//! placed over its column and sized to the rows it spans after the rows
//! settle, §17.2.1 / §17.5.3), and `table-layout: fixed` (§17.5.2.1: the
//! first row's specified cell widths fix their columns, the rest share the
//! remainder equally; cell content does not widen anything).
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

fn span_attr(tree: &LayoutTree, cell: NodeId, name: &str) -> usize {
    tree.node_map
        .get(&cell)
        .and_then(|n| n.as_element().and_then(|e| e.attributes.borrow().get(name).and_then(|v| v.trim().parse::<usize>().ok())))
        .unwrap_or(1)
        .clamp(1, 1000)
}

fn colspan(tree: &LayoutTree, cell: NodeId) -> usize {
    span_attr(tree, cell, "colspan")
}

fn rowspan(tree: &LayoutTree, cell: NodeId) -> usize {
    span_attr(tree, cell, "rowspan")
}

// Cells spanning rows, as (cell, first row index, rows spanned), per table:
// placed in pass two, sized by `fix_rowspans` once the rows settle.
thread_local! {
    static ROWSPANS: std::cell::RefCell<Vec<(NodeId, Vec<NodeId>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Sizes every row-spanning cell to the rows it spans (their border boxes
/// plus the spacing between them). Returns whether any height changed.
pub(crate) fn fix_rowspans(tree: &mut LayoutTree) -> bool {
    let spans = ROWSPANS.with(|r| r.borrow().clone());
    let mut changed = false;
    for (cell, rows) in spans {
        let mut top = f32::MAX;
        let mut bottom = f32::MIN;
        for r in &rows {
            if let Ok(l) = tree.taffy.layout(*r) {
                top = top.min(l.location.y);
                bottom = bottom.max(l.location.y + l.size.height);
            }
        }
        if top > bottom {
            continue;
        }
        let h = bottom - top;
        if let Ok(st) = tree.taffy.style(cell) {
            if st.size.height != Dimension::length(h) {
                let mut st = st.clone();
                st.size.height = Dimension::length(h);
                let _ = tree.taffy.set_style(cell, st);
                changed = true;
            }
        }
    }
    changed
}

/// Builds the UA defaults the separated model needs at build time: none —
/// spacing is applied here, after the cascade (it depends on the model).
pub(crate) fn apply(tree: &mut LayoutTree, min_content: &dyn Fn(&LayoutTree, NodeId) -> f32) {
    ROWSPANS.with(|r| r.borrow_mut().clear());
    for table in tables(tree) {
        layout_table(tree, table, min_content);
    }
}

fn layout_table(tree: &mut LayoutTree, table: NodeId, min_content: &dyn Fn(&LayoutTree, NodeId) -> f32) {
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
    // Slots still covered by a rowspan from an earlier row: col -> rows left.
    let mut covered: Vec<usize> = Vec::new();
    let mut spanning: Vec<(NodeId, usize, usize, usize)> = Vec::new(); // (cell, row, col, rows)
    for (ri, &r) in rows.iter().enumerate() {
        let mut col = 0usize;
        let mut line = Vec::new();
        let skip = |col: &mut usize, covered: &Vec<usize>| {
            while covered.get(*col).is_some_and(|&n| n > 0) {
                *col += 1;
            }
        };
        for c in tree.taffy.children(r).unwrap_or_default() {
            if !matches!(tag(tree, c).as_deref(), Some("td" | "th")) {
                continue;
            }
            skip(&mut col, &covered);
            let span = colspan(tree, c);
            let rs = rowspan(tree, c).min(rows.len() - ri);
            line.push((c, col, span));
            if rs > 1 {
                spanning.push((c, ri, col, rs));
                if covered.len() < col + span {
                    covered.resize(col + span, 0);
                }
                for k in &mut covered[col..col + span] {
                    *k = rs; // decremented below, after this row
                }
            }
            col += span;
        }
        ncols = ncols.max(col).max(covered.len());
        for k in covered.iter_mut() {
            *k = k.saturating_sub(1);
        }
        grid.push(line);
    }
    if ncols == 0 {
        return;
    }
    let cell_w = |tree: &LayoutTree, c: NodeId| tree.taffy.layout(c).map(|l| l.size.width).unwrap_or(0.0);
    let mut maxw = vec![0.0f32; ncols];
    let mut minw = vec![0.0f32; ncols];
    let fixed_layout = paint.table_fixed == Some(true);
    for line in &grid {
        for &(c, col, span) in line {
            if span == 1 {
                maxw[col] = maxw[col].max(cell_w(tree, c));
                minw[col] = minw[col].max(min_content(tree, c).min(cell_w(tree, c)));
            }
        }
    }
    for line in &grid {
        for &(c, col, span) in line {
            if span > 1 {
                let have: f32 = minw[col..col + span].iter().sum::<f32>() + spacing * (span - 1) as f32;
                let need = min_content(tree, c);
                if need > have {
                    let add = (need - have) / span as f32;
                    for w in &mut minw[col..col + span] {
                        *w += add;
                    }
                }
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
    let fixed = paint.has_width == Some(true) || fixed_layout;
    let total_max: f32 = maxw.iter().sum();
    let total_min: f32 = minw.iter().sum();
    let mut target = if fixed || sum > avail { avail } else { sum };
    if !fixed_layout {
        // A table is never narrower than its columns' min-content.
        target = target.max(total_min + gaps);
    }
    let room = (target - gaps).max(0.0);
    let widths: Vec<f32> = if fixed_layout {
        // §17.5.2.1: the first row's specified widths fix their columns;
        // the remaining columns share what is left equally.
        let mut w: Vec<Option<f32>> = vec![None; ncols];
        if let Some(first) = grid.first() {
            for &(c, col, span) in first {
                let spec = tree.taffy.style(c).ok().and_then(|st| {
                    let raw = st.size.width.into_raw();
                    match raw.tag() {
                        taffy::style::CompactLength::LENGTH_TAG => Some(raw.value()),
                        taffy::style::CompactLength::PERCENT_TAG => Some(raw.value() * room),
                        _ => None,
                    }
                });
                if let Some(v) = spec.filter(|_| tree.paint_map.get(&c).and_then(|p| p.has_width) == Some(true)) {
                    let pad = tree.taffy.layout(c).map(|l| l.padding.left + l.padding.right + l.border.left + l.border.right).unwrap_or(0.0);
                    let content_box = tree.taffy.style(c).map(|st| st.box_sizing == BoxSizing::ContentBox).unwrap_or(true);
                    let bw = if content_box { v + pad } else { v };
                    for k in &mut w[col..col + span] {
                        *k = Some(bw / span as f32);
                    }
                }
            }
        }
        let used: f32 = w.iter().flatten().sum();
        let free = w.iter().filter(|x| x.is_none()).count();
        let each = if free > 0 { ((room - used) / free as f32).max(0.0) } else { 0.0 };
        w.into_iter().map(|x| x.unwrap_or(each)).collect()
    } else if room >= total_max && total_max > 0.0 {
        // Wider than max-content (an author width): grow with max-content.
        maxw.iter().map(|w| w / total_max * room).collect()
    } else if room > total_min && total_max > total_min {
        // Between the sums: min plus a share of (max - min).
        let f = (room - total_min) / (total_max - total_min);
        minw.iter().zip(&maxw).map(|(a, b)| a + (b - a).max(0.0) * f).collect()
    } else if total_min > 0.0 {
        minw.clone()
    } else {
        vec![room / ncols as f32; ncols]
    };
    // Column x offsets inside the row (for row-spanning cells).
    let col_x: Vec<f32> = (0..ncols)
        .map(|c| widths[..c].iter().sum::<f32>() + spacing * c as f32)
        .collect();

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
        // The in-flow cells' running column, to leave room for slots a
        // rowspan from above (or a spanning cell of this row) occupies.
        let mut next_col = 0usize;
        let mut flow_i = 0usize;
        for &(c, col, span) in line.iter() {
            let mut w: f32 = widths[col..col + span].iter().sum::<f32>() + spacing * (span - 1) as f32;
            let spans_rows = spanning.iter().any(|s| s.0 == c);
            if collapse && col > 0 {
                w += shared;
            }
            if let Ok(st) = tree.taffy.style(c) {
                let mut st = st.clone();
                st.size.width = Dimension::length(w);
                st.box_sizing = BoxSizing::BorderBox;
                st.flex_shrink = 0.0;
                st.flex_grow = 0.0;
                if spans_rows {
                    st.position = taffy::style::Position::Absolute;
                    st.inset.left = LengthPercentageAuto::length(col_x[col] + if collapse && col > 0 { -shared } else { 0.0 });
                    st.inset.top = LengthPercentageAuto::length(0.0);
                    st.margin.left = LengthPercentageAuto::length(0.0);
                } else {
                    // Skipped columns (covered by row-spanning cells) become a
                    // leading margin.
                    let skipped: f32 = widths[next_col..col].iter().sum::<f32>() + spacing * (col - next_col) as f32;
                    let base = if collapse && col > 0 { -shared } else { 0.0 };
                    st.margin.left = LengthPercentageAuto::length(base + skipped);
                    flow_i += 1;
                    next_col = col + span;
                }
                st.align_self = Some(AlignSelf::STRETCH);
                let _ = tree.taffy.set_style(c, st);
            }
        }
        let _ = (ri, flow_i);
    }
    ROWSPANS.with(|reg| {
        let mut reg = reg.borrow_mut();
        for &(c, ri, _, rs) in &spanning {
            reg.push((c, rows[ri..ri + rs].to_vec()));
        }
    });
}
