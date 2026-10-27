//! AETHERFIT (SR189): `width` / `height: fit-content` (css-sizing-3 §2.1, shrink-to-fit: min(max-content,
//! max(min-content, the available size))) and the viewport as a `position: fixed` box's containing block
//! (CSS 2.2 §10.1.3): an in-flow fit-content box shrinks to its text, a long text clamps to the available width,
//! and the spec's popover / modal dialog rule (`inset: 0; margin: auto; width / height: fit-content`) centres a
//! fixed box in the VIEWPORT even inside an offset, narrow, short parent — recorded in `fixed_at`, with the
//! specified insets restored after the run (a second layout gives the same answer).
mod common;
use common::*;
use layout_core::taffy;

#[test]
fn fit_content_shrinks_in_flow_and_clamps() {
    let doc = page(vec![
        el("div", &[("id", "auto"), ("style", "height: 20px")], vec![text("short")]),
        el("div", &[("id", "fit"), ("style", "width: fit-content")], vec![text("short")]),
        el("div", &[("id", "long"), ("style", "width: fit-content")], vec![text(&"word ".repeat(200))]),
    ]);
    let tree = lay(&doc, 400.0, 300.0);
    let (_, _, auto_w, _) = box_of(&tree, &doc.by_id("auto"));
    let (_, _, fit_w, _) = box_of(&tree, &doc.by_id("fit"));
    let (_, _, long_w, long_h) = box_of(&tree, &doc.by_id("long"));
    assert_eq!(auto_w, 400.0, "an auto width fills the container");
    assert!(fit_w > 0.0 && fit_w < 100.0, "fit-content shrinks to the text's max-content: {fit_w}");
    assert!(long_w <= 400.0 && long_w > 300.0, "a long text clamps to the available width: {long_w}");
    assert!(long_h > 20.0, "and wraps: {long_h}");
}

#[test]
fn fixed_fit_content_centres_in_the_viewport() {
    let fixed = "position: fixed; inset: 0; margin: auto; width: fit-content; height: fit-content";
    let doc = page(vec![el(
        "div",
        &[("id", "parent"), ("style", "margin: 30px 0 0 50px; width: 120px; height: 10px; padding: 5px; border: 2px solid")],
        vec![el("div", &[("id", "box"), ("style", fixed)], vec![text("centred")])],
    )]);
    let (vw, vh) = (400.0, 300.0);
    let mut tree = lay(&doc, vw, vh);
    let (x, y, w, h) = box_of(&tree, &doc.by_id("box"));
    assert!(w > 0.0 && w < 200.0, "fit-content width {w}");
    assert!(h > 0.0 && h < 100.0, "fit-content height {h}");
    assert!((x - (vw - w) / 2.0).abs() < 0.01, "centred horizontally in the viewport: x {x} w {w}");
    assert!((y - (vh - h) / 2.0).abs() < 0.01, "centred vertically in the viewport: y {y} h {h}");
    let id = id_of(&tree, &doc.by_id("box"));
    assert_eq!(tree.fixed_at.get(&id).copied(), Some((Some(x), Some(y))), "fixed_at holds the viewport origin");
    // The rewrite is for the run only: the specified insets are back, and laying out again answers the same.
    let st = tree.taffy.style(id).unwrap();
    assert_eq!(st.inset.left, taffy::style::LengthPercentageAuto::length(0.0));
    assert_eq!(st.inset.top, taffy::style::LengthPercentageAuto::length(0.0));
    // The same tree laid out a second time: every box's style is identical (the rewrite-then-restore leaves no
    // trace) and the fixed box lands on the same viewport origin.
    let mut ids: Vec<taffy::NodeId> = tree.node_map.iter().map(|(t, _)| *t).collect();
    ids.sort_by_key(|t| u64::from(*t));
    let before: Vec<taffy::Style> = ids.iter().map(|t| tree.taffy.style(*t).unwrap().clone()).collect();
    let first = tree.fixed_at.get(&id).copied();
    layout_core::remeasure(&mut tree, &F);
    let after: Vec<taffy::Style> = ids.iter().map(|t| tree.taffy.style(*t).unwrap().clone()).collect();
    assert!(before == after, "a second layout leaves every style as the first left it");
    assert_eq!(tree.fixed_at.get(&id).copied(), first, "a second layout gives the same fixed_at");
    assert_eq!(box_of(&tree, &doc.by_id("box")), (x, y, w, h));
    let again = lay(&doc, vw, vh);
    let id2 = id_of(&again, &doc.by_id("box"));
    assert_eq!(again.fixed_at.get(&id2).copied(), tree.fixed_at.get(&id).copied());
    assert_eq!(box_of(&again, &doc.by_id("box")), (x, y, w, h));
}

#[test]
fn fixed_box_without_insets_keeps_its_place() {
    let doc = page(vec![el("div", &[("style", "height: 40px")], vec![]), el("div", &[("id", "box"), ("style", "position: fixed; width: 10px; height: 10px")], vec![])]);
    let tree = lay(&doc, 400.0, 300.0);
    let id = id_of(&tree, &doc.by_id("box"));
    assert_eq!(tree.fixed_at.get(&id), None, "no definite inset: no viewport origin recorded");
}
