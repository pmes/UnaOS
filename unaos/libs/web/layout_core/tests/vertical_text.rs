//! AETHERSTYLE3B (SR115): what SR103's vertical text refused. In a `vertical-rl` paragraph the two CJK
//! ideographs of "漢字abc" get upright cells (Latin stays sideways under `mixed`), `text-orientation: upright`
//! sets every glyph upright and `sideways` none; an upright cell's pixels are translated into the column
//! (`to_physical_upright`), not turned; `to_logical` inverts the line rotation; and a pointer over the
//! physical column hits the right character through `hit::hit` — selection in vertical text.
mod common;
use common::*;
use layout_core::writing::{self, UprightCell};
use layout_core::DomNode;

#[test]
fn upright_cells_and_vertical_hit_testing() {
    let st = |extra: &str| format!("writing-mode: vertical-rl; height: 100px; font-size: 20px; line-height: 20px; {extra}");
    let (s1, s2, s3) = (st(""), st("text-orientation: upright"), st("text-orientation: sideways"));
    let doc = page(vec![
        el("div", &[("id", "cjk"), ("style", s1.as_str())], vec![text("漢字abc")]),
        el("div", &[("id", "up"), ("style", s2.as_str())], vec![text("ab")]),
        el("div", &[("id", "side"), ("style", s3.as_str())], vec![text("漢字")]),
    ]);
    let tree = lay(&doc, 800.0, 600.0);
    assert!(writing::upright('漢', 0) && writing::upright('。', 0) && writing::upright('あ', 0));
    assert!(!writing::upright('a', 0) && writing::upright('a', 1) && !writing::upright('漢', 2) && !writing::upright(' ', 1));

    let cjk = doc.by_id("cjk");
    let il = &tree.inline[&id_of(&tree, &cjk)];
    assert_eq!(il.vertical, writing::VERTICAL_RL);
    assert_eq!(il.upright, vec![UprightCell { x: 0.0, w: 10.0, top: 0.0, h: 20.0 }, UprightCell { x: 10.0, w: 10.0, top: 0.0, h: 20.0 }]);
    assert_eq!(tree.inline[&id_of(&tree, &doc.by_id("up"))].upright.len(), 2, "upright: Latin too");
    assert!(tree.inline[&id_of(&tree, &doc.by_id("side"))].upright.is_empty(), "sideways: nothing upright");

    // an upright cell is translated into the 20px column, centred: no rotation
    assert_eq!(writing::to_physical_upright(writing::VERTICAL_RL, 2, 5, 20, &il.upright), Some((7, 0)));
    assert_eq!(writing::to_physical_upright(writing::VERTICAL_RL, 12, 5, 20, &il.upright), Some((7, 10)));
    assert_eq!(writing::to_physical_upright(writing::VERTICAL_RL, 25, 5, 20, &il.upright), None, "Latin turns with the line");
    assert_eq!(writing::to_physical_upright(writing::SIDEWAYS_RL, 2, 5, 20, &il.upright), None);

    // to_logical inverts to_physical
    let line = |_: i32| Some((0, 20));
    let (px, py) = writing::to_physical(writing::VERTICAL_RL, 30, 3, 20, 100, &line);
    assert_eq!((px, py), (16, 30));
    let (lx, ly) = writing::to_logical(writing::VERTICAL_RL, px as f32 + 0.5, py as f32 + 0.5, 20.0, 100.0, &[(0.0, 20.0)]);
    assert_eq!((lx, ly), (30.5, 3.5));
    let (px, py) = writing::to_physical(writing::VERTICAL_LR, 30, 3, 20, 100, &line);
    let (lx, ly) = writing::to_logical(writing::VERTICAL_LR, px as f32 + 0.5, py as f32 + 0.5, 20.0, 100.0, &[(0.0, 20.0)]);
    assert_eq!((lx, ly), (30.5, 3.5));

    // a pointer 33px down the column hits between 'a' and 'b' (10px glyphs): UTF-16 offset 3 of the text
    let (x0, y0, _, _) = box_of(&tree, &cjk);
    let boxes = layout_core::hit::text_boxes(&tree);
    let h = layout_core::hit::hit(
        &tree,
        &boxes,
        |_| (x0 + 10.0, y0 + 33.0),
        |_, t: &str, b: usize| t[..b].chars().count() as f32 * 10.0,
        |n: &N| n.id,
    )
    .expect("a hit");
    let text_node = DomNode::children(&cjk)[0].id;
    assert_eq!(h.point.node, text_node);
    assert_eq!(h.point.offset, 3);
    assert!(h.inside);
}
