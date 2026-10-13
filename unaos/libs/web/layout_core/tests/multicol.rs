//! AETHERSTYLE3 (SR103): multi-column layout. A `columns: 2` container 410px wide (gap 10px) gets two 200px
//! columns; its four 50px blocks balance two per column — the third block moves 210px right and 100px up — and
//! the container is 100px tall. A `column-count: 2` paragraph balances its own lines.
mod common;
use common::*;

#[test]
fn columns_balance_blocks_and_lines() {
    let blk = |id: &'static str| el("div", &[("id", id), ("style", "height: 50px")], vec![]);
    let doc = page(vec![
        el(
            "div",
            &[("id", "mc"), ("style", "width: 410px; columns: 2; column-gap: 10px; column-rule: 2px solid rgb(0, 0, 255)")],
            vec![blk("a"), blk("b"), blk("c"), blk("d")],
        ),
        el("div", &[("id", "next"), ("style", "height: 10px")], vec![]),
        // 8px characters: 20 words of 3 chars + spaces, a 100px column holds 3 words a line
        el("p", &[("id", "p"), ("style", "margin: 0; width: 210px; column-count: 2; column-gap: 10px; font-size: 16px; line-height: 20px")], vec![text(&"abc ".repeat(24))]),
    ]);
    let tree = lay(&doc, 800.0, 600.0);
    let mc = doc.by_id("mc");
    let (_, my, mw, mh) = box_of(&tree, &mc);
    assert_eq!((my, mw, mh), (0.0, 410.0, 100.0), "the tallest column is the container's height");
    assert_eq!(box_of(&tree, &doc.by_id("next")).1, 100.0, "what follows sits below the columns");
    let cols = tree.multicol[&id_of(&tree, &mc)];
    assert_eq!((cols.count, cols.filled, cols.width, cols.gap), (2, 2, 200.0, 10.0));
    assert_eq!(cols.rule, Some((2.0, (0, 0, 255), 0)));
    assert_eq!(box_of(&tree, &doc.by_id("a")).2, 200.0, "children lay out at the column width");
    let sh = |id: &str| layout_core::col_shift(&tree, id_of(&tree, &doc.by_id(id)));
    assert_eq!(sh("a"), (0.0, 0.0));
    assert_eq!(sh("b"), (0.0, 0.0));
    assert_eq!(sh("c"), (210.0, -100.0));
    assert_eq!(sh("d"), (210.0, -100.0));

    // the paragraph: 24 words, 3 a line -> 8 lines of 20px, 4 per column
    let p = id_of(&tree, &doc.by_id("p"));
    let il = &tree.inline[&p];
    let pc = il.columns.expect("an IFC multicol root");
    assert_eq!((pc.count, pc.width), (2, 100.0));
    assert_eq!(il.height, 80.0);
    assert_eq!(box_of(&tree, &doc.by_id("p")).3, 80.0);
    let xs: Vec<f32> = il
        .frags
        .iter()
        .filter_map(|f| match f {
            layout_core::inline::Frag::Text { x, .. } => Some(*x),
            _ => None,
        })
        .collect();
    assert!(xs.iter().any(|x| *x >= 110.0), "some lines moved into the second column: {xs:?}");
    assert!(xs.iter().all(|x| *x < 110.0 + 100.0));
}
