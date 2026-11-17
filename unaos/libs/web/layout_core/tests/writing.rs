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

// ---- AETHERCSSOMUNICODEBIDI (LEDGER SR1008): css-writing-modes-4 §2.2 the unicode-bidi arm (named tail block) ----

/// AETHERCSSOMUNICODEBIDI (SR1008): `unicode-bidi` declared in order on one rule, its `PaintStyle::unicode_bidi` code.
fn unicode_bidi_after(decls: &[&str]) -> Option<u8> {
    let cx = layout_core::css::ParseCx::new(600.0, 400.0);
    let mut s = layout_core::css::SpecifiedStyle::<TH>::default();
    for v in decls {
        layout_core::css::apply_declaration("unicode-bidi", v, &mut s, &cx);
    }
    s.paint.unicode_bidi
}

/// AETHERCSSOMUNICODEBIDI (SR1008): the six keywords, ASCII-case-insensitive, and nothing written without a declaration.
#[test]
fn unicode_bidi_keywords() {
    assert_eq!(unicode_bidi_after(&[]), None, "undeclared: the tag road decides");
    for (v, want) in [
        ("normal", 0), ("embed", 1), ("isolate", 2), ("bidi-override", 3), ("isolate-override", 4), ("plaintext", 5),
        ("BIDI-OVERRIDE", 3), ("Isolate-Override", 4), (" plaintext ", 5),
    ] {
        assert_eq!(unicode_bidi_after(&[v]), Some(want), "unicode-bidi: {v:?}");
        assert_eq!(layout_core::css::unicode_bidi_code(v), Some(want), "unicode_bidi_code({v:?})");
    }
}

/// AETHERCSSOMUNICODEBIDI (SR1008): not inherited, so `initial` / `unset` are normal and beat an earlier bidi-override;
/// `inherit` (owed to the non-inherited inherit road), `revert` and garbage keep the earlier value.
#[test]
fn unicode_bidi_initial_unset_and_invalid() {
    assert_eq!(unicode_bidi_after(&["bidi-override", "initial"]), Some(0));
    assert_eq!(unicode_bidi_after(&["bidi-override", "UNSET"]), Some(0));
    assert_eq!(unicode_bidi_after(&["isolate", "inherit"]), Some(2));
    assert_eq!(unicode_bidi_after(&["isolate", "revert"]), Some(2));
    assert_eq!(unicode_bidi_after(&["embed", "sideways"]), Some(1));
    assert_eq!(unicode_bidi_after(&["garbage"]), None);
}

/// AETHERCSSOMUNICODEBIDI (SR1008): `merge_paint` carries the field: a later rule's value wins, an undeclared one keeps it.
#[test]
fn unicode_bidi_merges() {
    let cx = layout_core::css::ParseCx::new(600.0, 400.0);
    let mut a = layout_core::css::SpecifiedStyle::<TH>::default();
    layout_core::css::apply_declaration("unicode-bidi", "isolate-override", &mut a, &cx);
    let mut b = layout_core::css::SpecifiedStyle::<TH>::default();
    layout_core::css::apply_declaration("direction", "rtl", &mut b, &cx);
    let mut dst = layout_core::css::SpecifiedStyle::<TH>::default();
    layout_core::css::merge_paint(&mut dst.paint, &a.paint);
    layout_core::css::merge_paint(&mut dst.paint, &b.paint);
    assert_eq!(dst.paint.unicode_bidi, Some(4), "an undeclared later rule keeps it");
    let mut c = layout_core::css::SpecifiedStyle::<TH>::default();
    layout_core::css::apply_declaration("unicode-bidi", "normal", &mut c, &cx);
    layout_core::css::merge_paint(&mut dst.paint, &c.paint);
    assert_eq!(dst.paint.unicode_bidi, Some(0), "a later normal wins");
}
