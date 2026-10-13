//! AETHERSTYLE3 (SR103): writing modes. A `vertical-rl` paragraph 100px tall breaks its 8px-a-character words
//! along its 100px inline (vertical) size — two 20px lines, so the box is 40px wide and 100px tall; a
//! `vertical-rl` block container stacks its two paragraphs right to left, each filling its 60px inline size;
//! logical properties map through the element's own writing mode; `to_physical` turns staging pixels
//! clockwise (rl) and flips lines back left to right (lr).
mod common;
use common::*;
use layout_core::writing;

#[test]
fn vertical_lines_boxes_and_logical_mapping() {
    let doc = page(vec![
        el("div", &[("id", "v"), ("style", "writing-mode: vertical-rl; height: 100px; font-size: 16px; line-height: 20px")], vec![text("abc abc abc abc")]),
        el(
            "div",
            &[("id", "vb"), ("style", "writing-mode: vertical-rl; height: 60px; font-size: 16px; line-height: 20px")],
            vec![
                el("p", &[("id", "p1"), ("style", "margin: 0")], vec![text("ab")]),
                el("p", &[("id", "p2"), ("style", "margin: 0")], vec![text("cd")]),
            ],
        ),
        el("div", &[("id", "m"), ("style", "writing-mode: vertical-lr; margin-block-start: 7px; inline-size: 50px; block-size: 30px")], vec![]),
    ]);
    let tree = lay(&doc, 800.0, 600.0);
    let v = doc.by_id("v");
    let (_, _, w, h) = box_of(&tree, &v);
    assert_eq!((w, h), (40.0, 100.0), "block extent across, inline size down");
    let il = &tree.inline[&id_of(&tree, &v)];
    assert_eq!(il.vertical, writing::VERTICAL_RL);
    assert_eq!(il.lines.len(), 2);
    assert_eq!(il.inline_extent, 100.0);

    let (vx, _, vw, vh) = box_of(&tree, &doc.by_id("vb"));
    assert_eq!((vw, vh), (40.0, 60.0), "an orthogonal flow is as wide as its content");
    let (x1, _, w1, h1) = box_of(&tree, &doc.by_id("p1"));
    let (x2, _, w2, h2) = box_of(&tree, &doc.by_id("p2"));
    assert_eq!((w1, h1, w2, h2), (20.0, 60.0, 20.0, 60.0), "each fills the 60px inline size");
    assert_eq!((x1 - vx, x2 - vx), (20.0, 0.0), "vertical-rl: the first block at the right");

    // vertical-lr: margin-block-start is the left margin, inline-size the height, block-size the width
    let m = id_of(&tree, &doc.by_id("m"));
    let ml = tree.taffy.layout(m).unwrap();
    assert_eq!((ml.margin.left, ml.size.width, ml.size.height), (7.0, 30.0, 50.0));

    // staging -> physical: rl turns clockwise; lr flips each line back left to right
    let none = |_: i32| None;
    assert_eq!(writing::to_physical(writing::VERTICAL_RL, 5, 0, 40, 100, &none), (39, 5));
    let lines = |y: i32| if y < 20 { Some((0, 20)) } else { Some((20, 20)) };
    assert_eq!(writing::to_physical(writing::VERTICAL_LR, 5, 0, 40, 100, &lines), (19, 5));
    assert_eq!(writing::to_physical(writing::VERTICAL_LR, 5, 19, 40, 100, &lines), (0, 5));
    assert_eq!(writing::to_physical(writing::SIDEWAYS_LR, 0, 3, 40, 100, &none), (3, 99));
    assert_eq!(writing::logical_sides(writing::VERTICAL_RL, false), ("top", "bottom", "right", "left"));
}
