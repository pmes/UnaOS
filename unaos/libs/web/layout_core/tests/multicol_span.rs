//! AETHERSTYLE3B (SR115): what SR103's multicol refused. A `column-span: all` child splits a `columns: 2`
//! container (410px, gap 10px) into two column sets: a / b balance into the first set (b moves 210px right,
//! 50px up), the 30px spanner moves up under that set at the full 410px width, c / d balance into the second
//! set below it, and the container is 130px tall with its two sets recorded for the rules. A tall wrapper with
//! no box of its own fragments: its four 50px children split two per column.
mod common;
use common::*;
use layout_core::multicol::ColumnSet;

#[test]
fn spanner_splits_sets_and_wrapper_fragments() {
    let blk = |id: &'static str| el("div", &[("id", id), ("style", "height: 50px")], vec![]);
    let doc = page(vec![
        el(
            "div",
            &[("id", "mc"), ("style", "width: 410px; columns: 2; column-gap: 10px; column-rule: 2px solid rgb(0, 0, 255)")],
            vec![blk("a"), blk("b"), el("div", &[("id", "span"), ("style", "height: 30px; column-span: all")], vec![]), blk("c"), blk("d")],
        ),
        el("div", &[("id", "next"), ("style", "height: 10px")], vec![]),
        el("div", &[("id", "wrapmc"), ("style", "width: 410px; columns: 2; column-gap: 10px")], vec![el("div", &[("id", "wrap")], vec![blk("w1"), blk("w2"), blk("w3"), blk("w4")])]),
    ]);
    let tree = lay(&doc, 800.0, 600.0);
    let sh = |id: &str| layout_core::col_shift(&tree, id_of(&tree, &doc.by_id(id)));
    let mc = id_of(&tree, &doc.by_id("mc"));
    assert_eq!(sh("a"), (0.0, 0.0));
    assert_eq!(sh("b"), (210.0, -50.0));
    assert_eq!(sh("span"), (0.0, -50.0), "the spanner sits under the first set");
    assert_eq!(box_of(&tree, &doc.by_id("span")).2, 410.0, "at the container's full width");
    assert_eq!(box_of(&tree, &doc.by_id("a")).2, 200.0);
    assert_eq!(sh("c"), (0.0, -50.0));
    assert_eq!(sh("d"), (210.0, -100.0));
    assert_eq!(box_of(&tree, &doc.by_id("mc")).3, 130.0, "set 50 + spanner 30 + set 50");
    assert_eq!(box_of(&tree, &doc.by_id("next")).1, 130.0);
    assert_eq!(
        tree.col_sets[&mc],
        vec![ColumnSet { top: 0.0, height: 50.0, filled: 2 }, ColumnSet { top: 80.0, height: 50.0, filled: 2 }]
    );

    // the wrapper's children are the units
    assert_eq!(sh("w1"), (0.0, 0.0));
    assert_eq!(sh("w2"), (0.0, 0.0));
    assert_eq!(sh("w3"), (210.0, -100.0));
    assert_eq!(sh("w4"), (210.0, -100.0));
    assert_eq!(box_of(&tree, &doc.by_id("wrapmc")).3, 100.0);
    assert!(!tree.col_sets.contains_key(&id_of(&tree, &doc.by_id("wrapmc"))), "one set: no split recorded");
}
