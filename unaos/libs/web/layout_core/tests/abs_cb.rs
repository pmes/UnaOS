//! AETHERABSCB (LEDGER SR431): CSS 2.2 §10.1.4 — an absolutely positioned box's containing block is the padding box
//! of its nearest positioned ancestor, else the initial containing block; Aether's boxes are all taffy `Relative`,
//! so `fixed_viewport`'s rewrite places a box whose parent is static against that ancestor. Expected values are
//! §10.1's (a Chrome page gives the same). SR420's six shrink-to-fit cases stay in `abs_shrink.rs`, unchanged.
mod common;
use common::*;
use layout_core::taffy;

const GP: &str = "position: relative; width: 300px; height: 200px; padding: 20px";
const STATIC_P: &str = "margin-left: 50px; padding-top: 30px; width: 100px; height: 50px";

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

/// `kid` inside a parent styled `parent` inside the `position: relative` grandparent `#gp` (padding box 340 x 240 at
/// the page origin); the kid's box relative to `#gp`'s border-box origin.
fn in_grandparent(parent: &str, kid: &str) -> (f32, f32, f32, f32) {
    let doc = page(vec![el("div", &[("id", "gp"), ("style", GP)], vec![el("div", &[("id", "p"), ("style", parent)], vec![el("div", &[("id", "k"), ("style", kid)], vec![])])])]);
    let tree = lay(&doc, 400.0, 300.0);
    let (gx, gy, _, _) = box_of(&tree, &doc.by_id("gp"));
    let (x, y, w, h) = box_of(&tree, &doc.by_id("k"));
    (x - gx, y - gy, w, h)
}

#[test]
fn static_parent_takes_the_positioned_grandparent() {
    // top / left from the grandparent's padding edge, not the static parent's (which sits at 70, 20: was 77, 25).
    let (x, y, _, _) = in_grandparent(STATIC_P, "position: absolute; top: 5px; left: 7px; width: 10px; height: 10px");
    assert!(near(x, 7.0) && near(y, 5.0), "top 5 left 7 against #gp: x {x} y {y}");
    // right / bottom from the grandparent's far padding edges: 340 - 7 - 10, 240 - 5 - 10.
    let (x, y, _, _) = in_grandparent(STATIC_P, "position: absolute; right: 7px; bottom: 5px; width: 10px; height: 10px");
    assert!(near(x, 323.0) && near(y, 225.0), "right 7 bottom 5 against #gp: x {x} y {y}");
    // A % width is of the containing block's padding box (340), not the parent's (100).
    let (x, _, w, _) = in_grandparent(STATIC_P, "position: absolute; top: 0; left: 0; width: 50%; height: 10px");
    assert!(near(w, 170.0) && near(x, 0.0), "width 50% of #gp: x {x} w {w}");
}

#[test]
fn no_positioned_ancestor_takes_the_initial_containing_block() {
    let doc = page(vec![el(
        "div",
        &[("style", "padding: 30px 0 0 40px")],
        vec![
            el("div", &[("id", "a"), ("style", "position: absolute; top: 0; left: 0; width: 10px; height: 10px")], vec![]),
            el("div", &[("id", "b"), ("style", "position: absolute; right: 0; bottom: 0; width: 10px; height: 10px")], vec![]),
        ],
    )]);
    let tree = lay(&doc, 400.0, 300.0);
    let (x, y, _, _) = box_of(&tree, &doc.by_id("a"));
    assert!(near(x, 0.0) && near(y, 0.0), "top 0 left 0 at the canvas origin, not the padded div's (40, 30): x {x} y {y}");
    let (x, y, _, _) = box_of(&tree, &doc.by_id("b"));
    assert!(near(x, 390.0) && near(y, 290.0), "right 0 bottom 0 at the viewport's corner: x {x} y {y}");
}

#[test]
fn positioned_parent_keeps_taffys_place() {
    // Control: the parent is `position: relative`, so it IS the containing block (20 + 50 + 7, 20 + 5).
    let parent = "position: relative; margin-left: 50px; width: 100px; height: 50px";
    let (x, y, _, _) = in_grandparent(parent, "position: absolute; top: 5px; left: 7px; width: 10px; height: 10px");
    assert!(near(x, 77.0) && near(y, 25.0), "a relative parent is the containing block: x {x} y {y}");
}

#[test]
fn nested_absolute_settles() {
    // #o (absolute, static parent) against #gp; #i (absolute, static parent #q) against #o.
    let inner = el("div", &[("id", "i"), ("style", "position: absolute; top: 1px; left: 2px; width: 5px; height: 5px")], vec![]);
    let q = el("div", &[("id", "q"), ("style", "margin-left: 30px; padding-top: 4px")], vec![inner]);
    let o = el("div", &[("id", "o"), ("style", "position: absolute; top: 10px; left: 10px; width: 100px; height: 100px")], vec![q]);
    let doc = page(vec![el("div", &[("id", "gp"), ("style", GP)], vec![el("div", &[("style", STATIC_P)], vec![o])])]);
    let tree = lay(&doc, 400.0, 300.0);
    let (gx, gy, _, _) = box_of(&tree, &doc.by_id("gp"));
    let (ox, oy, _, _) = box_of(&tree, &doc.by_id("o"));
    let (ix, iy, _, _) = box_of(&tree, &doc.by_id("i"));
    assert!(near(ox - gx, 10.0) && near(oy - gy, 10.0), "#o against #gp: {} {}", ox - gx, oy - gy);
    assert!(near(ix - ox, 2.0) && near(iy - oy, 1.0), "#i against #o, not #q: {} {}", ix - ox, iy - oy);
}

#[test]
fn rewrite_is_restored_and_stable() {
    let doc = page(vec![el("div", &[("id", "gp"), ("style", GP)], vec![el("div", &[("style", STATIC_P)], vec![el("div", &[("id", "k"), ("style", "position: absolute; top: 5px; left: 7px; width: 50%; height: 10px")], vec![])])])]);
    let mut tree = lay(&doc, 400.0, 300.0);
    let id = id_of(&tree, &doc.by_id("k"));
    let st = tree.taffy.style(id).unwrap().clone();
    assert_eq!(st.inset.left, taffy::style::LengthPercentageAuto::length(7.0), "the specified left is back after the run");
    assert_eq!(st.size.width, taffy::style::Dimension::percent(0.5), "and the specified % width");
    let mut ids: Vec<taffy::NodeId> = tree.node_map.iter().map(|(t, _)| *t).collect();
    ids.sort_by_key(|t| u64::from(*t));
    let before: Vec<taffy::Style> = ids.iter().map(|t| tree.taffy.style(*t).unwrap().clone()).collect();
    let first = box_of(&tree, &doc.by_id("k"));
    layout_core::remeasure(&mut tree, &F);
    let after: Vec<taffy::Style> = ids.iter().map(|t| tree.taffy.style(*t).unwrap().clone()).collect();
    assert!(before == after, "a second layout leaves every style as the first left it");
    assert_eq!(box_of(&tree, &doc.by_id("k")), first, "and the same box");
}
