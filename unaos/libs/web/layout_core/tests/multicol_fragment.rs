//! AETHERSTYLE3C (SR122): what SR115's multicol refused. A decorated box (5px border, 5px block padding, a
//! background) holding four 40px blocks in a `columns: 2` container (410px, gap 10px) fragments: under
//! `box-decoration-break: slice` the column height is 90px, the last two blocks move 210px right and 90px up,
//! and the box is two fragments — 0..90 with its block-start edge, 90..180 with its block-end edge, the second
//! moved with its column; under `clone` the balancer reserves the cloned 10px edges (column height 100px, the
//! moved blocks land 10px below the column top) and both fragments carry all edges. A box whose content stays in
//! one column moves whole (no fragments). A `column-span: all` box inside an unstyled wrapper splits the flow as
//! a direct child does, at the container's full width.
mod common;
use common::*;
use layout_core::multicol::{BoxFragment, ColumnSet};

#[test]
fn decorated_boxes_fragment_and_nested_spanners_split() {
    let blk = |id: &'static str, h: &str| el("div", &[("id", id), ("style", format!("height: {h}px").as_str())], vec![]);
    let deco = |id: &'static str, mode: &str, kids: Vec<Spec>| {
        el("div", &[("id", id), ("style", format!("border: 5px solid rgb(0, 0, 0); padding: 5px 0; background: rgb(255, 0, 0); box-decoration-break: {mode}").as_str())], kids)
    };
    let mc = |id: &'static str, kids: Vec<Spec>| el("div", &[("id", id), ("style", "width: 410px; columns: 2; column-gap: 10px")], kids);
    let doc = page(vec![
        mc("m1", vec![deco("s", "slice", vec![blk("s1", "40"), blk("s2", "40"), blk("s3", "40"), blk("s4", "40")])]),
        mc("m2", vec![deco("c", "clone", vec![blk("c1", "40"), blk("c2", "40"), blk("c3", "40"), blk("c4", "40")])]),
        mc("m3", vec![blk("w0", "200"), deco("whole", "slice", vec![blk("x1", "20")])]),
        mc(
            "m4",
            vec![el("div", &[("id", "wrap")], vec![blk("a", "50"), blk("b", "50"), el("div", &[("id", "span"), ("style", "height: 30px; column-span: all")], vec![]), blk("e", "50"), blk("d", "50")])],
        ),
    ]);
    let tree = lay(&doc, 800.0, 600.0);
    let sh = |id: &str| layout_core::col_shift(&tree, id_of(&tree, &doc.by_id(id)));
    let frags = |id: &str| tree.col_frags.get(&id_of(&tree, &doc.by_id(id))).cloned();

    // slice
    assert_eq!(box_of(&tree, &doc.by_id("m1")).3, 90.0, "balanced at 90px");
    assert_eq!((sh("s1"), sh("s2")), ((0.0, 0.0), (0.0, 0.0)));
    assert_eq!((sh("s3"), sh("s4")), ((210.0, -90.0), (210.0, -90.0)));
    assert_eq!(sh("s"), (0.0, 0.0), "a fragmented box does not move as a whole");
    assert_eq!(
        frags("s"),
        Some(vec![
            BoxFragment { shift: (0.0, 0.0), top: 0.0, height: 90.0, edges: (true, false), cloned: false },
            BoxFragment { shift: (210.0, -90.0), top: 90.0, height: 90.0, edges: (false, true), cloned: false },
        ])
    );

    // clone: the cloned edges take column space
    assert_eq!(box_of(&tree, &doc.by_id("m2")).3, 100.0, "balanced at 100px with the cloned edges");
    assert_eq!((sh("c3"), sh("c4")), ((210.0, -80.0), (210.0, -80.0)), "10px below the column top");
    assert_eq!(
        frags("c"),
        Some(vec![
            BoxFragment { shift: (0.0, 0.0), top: 0.0, height: 100.0, edges: (true, true), cloned: true },
            BoxFragment { shift: (210.0, -80.0), top: 80.0, height: 100.0, edges: (true, true), cloned: true },
        ])
    );

    // content that stays in one column: the box moves whole, as before
    assert_eq!(frags("whole"), None);
    assert_eq!(sh("whole"), (210.0, -200.0));
    assert_eq!(sh("x1"), (0.0, 0.0), "its content moves with it");

    // a spanner inside a wrapper
    assert_eq!(sh("a"), (0.0, 0.0));
    assert_eq!(sh("b"), (210.0, -50.0));
    assert_eq!(sh("span"), (0.0, -50.0));
    assert_eq!(box_of(&tree, &doc.by_id("span")).2, 410.0, "the wrapped spanner keeps the full width");
    assert_eq!(box_of(&tree, &doc.by_id("a")).2, 200.0, "the wrapper's other children take the column width");
    assert_eq!(sh("e"), (0.0, -50.0));
    assert_eq!(sh("d"), (210.0, -100.0));
    assert_eq!(box_of(&tree, &doc.by_id("m4")).3, 130.0);
    assert_eq!(
        tree.col_sets[&id_of(&tree, &doc.by_id("m4"))],
        vec![ColumnSet { top: 0.0, height: 50.0, filled: 2 }, ColumnSet { top: 80.0, height: 50.0, filled: 2 }]
    );
}
