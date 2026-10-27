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

/// AETHERFIT2 (SR191): CSS 2.2 §10.1.3 — a fixed box's % `width` / `height` / `max-*` resolve against the VIEWPORT
/// (its containing block), not its narrow, short DOM parent; the rewrite rides the fixed pass's one
/// rewrite-then-restore step, so a second layout leaves every style as the first did.
#[test]
fn fixed_percent_size_resolves_against_the_viewport() {
    let fixed = "position: fixed; left: 0; top: 0; width: 50%; height: 2000px; max-height: 100%";
    let doc = page(vec![el(
        "div",
        &[("id", "parent"), ("style", "margin: 30px 0 0 50px; width: 120px; height: 10px; padding: 5px; border: 2px solid")],
        vec![el("div", &[("id", "box"), ("style", fixed)], vec![])],
    )]);
    let (vw, vh) = (400.0, 300.0);
    let mut tree = lay(&doc, vw, vh);
    let (x, y, w, h) = box_of(&tree, &doc.by_id("box"));
    assert_eq!((x, y), (0.0, 0.0), "pinned to the viewport's origin");
    assert_eq!(w, vw / 2.0, "width: 50% of the viewport, not of the 120px parent");
    assert_eq!(h, vh, "max-height: 100% of the viewport clamps the 2000px height");
    let id = id_of(&tree, &doc.by_id("box"));
    let st = tree.taffy.style(id).unwrap();
    assert_eq!(st.size.width, taffy::style::Dimension::percent(0.5), "the specified % is back after the run");
    assert_eq!(st.max_size.height, taffy::style::LengthPercentageAuto::percent(1.0));
    let mut ids: Vec<taffy::NodeId> = tree.node_map.iter().map(|(t, _)| *t).collect();
    ids.sort_by_key(|t| u64::from(*t));
    let before: Vec<taffy::Style> = ids.iter().map(|t| tree.taffy.style(*t).unwrap().clone()).collect();
    layout_core::remeasure(&mut tree, &F);
    let after: Vec<taffy::Style> = ids.iter().map(|t| tree.taffy.style(*t).unwrap().clone()).collect();
    assert!(before == after, "a second layout leaves every style as the first left it");
    assert_eq!(box_of(&tree, &doc.by_id("box")), (x, y, w, h));
}

/// AETHERFIT2 (SR191): css-sizing-3 §2.1 / §3.2 — `min-content`, `max-content` and `fit-content(<length-percentage>)`
/// parse onto taffy's own keywords; in the block axis they behave as `auto` (§3.1).
#[test]
fn sizing_keywords_parse_onto_taffy() {
    let doc = page(vec![
        el("div", &[("id", "widest"), ("style", "width: fit-content")], vec![text("bbbbbbbb")]),
        el("div", &[("id", "min"), ("style", "width: min-content")], vec![text("aa bbbbbbbb c")]),
        el("div", &[("style", "width: 100px")], vec![el("div", &[("id", "max"), ("style", "width: max-content")], vec![text("word word word word word word")])]),
        el("div", &[("id", "oneline"), ("style", "width: fit-content")], vec![text("word word word word word word")]),
        el("div", &[("id", "fill")], vec![text("word word")]),
        el("div", &[("id", "two"), ("style", "width: fit-content")], vec![text("word word")]),
        el("div", &[("id", "fit200"), ("style", "width: fit-content(200px)")], vec![text("word word")]),
        el("div", &[("id", "fit50"), ("style", "width: fit-content(50%)")], vec![text("word word")]),
        el("div", &[("id", "bad"), ("style", "width: fit-content(red)")], vec![text("word word")]),
        el("div", &[("id", "autoh"), ("style", "width: 100px")], vec![text("aa bb cc dd ee ff gg hh")]),
        el("div", &[("id", "minh"), ("style", "width: 100px; height: min-content")], vec![text("aa bb cc dd ee ff gg hh")]),
        el("div", &[("id", "fith"), ("style", "width: 100px; height: fit-content(5px)")], vec![text("aa bb cc dd ee ff gg hh")]),
    ]);
    let tree = lay(&doc, 400.0, 300.0);
    let w = |id: &str| box_of(&tree, &doc.by_id(id)).2;
    let h = |id: &str| box_of(&tree, &doc.by_id(id)).3;
    assert!(w("widest") > 0.0);
    assert_eq!(w("min"), w("widest"), "min-content: the widest word");
    // max-content: one line, overflowing its 100px container (layout_core's text measure caps a max-content line
    // at the viewport width — this text is shorter than the viewport, so the cap does not bite).
    assert!(w("max") > 100.0, "max-content overflows its container: {}", w("max"));
    assert_eq!(w("max"), w("oneline"), "max-content: the text's one-line width");
    // fit-content(<length-percentage>) = min(max-content, max(min-content, the argument)): under a 200px / 50% limit
    // a two-word text takes its max-content, where the line-filling auto width takes the container. (The clamp of
    // a text WIDER than the argument is owed: layout_core's IFC-root measure lays a content-sized box at its
    // max-content, capped at the viewport, whatever the definite available width — the bare `fit-content` keyword
    // shares it.)
    assert!(w("fill") > 300.0, "auto fills the line: {}", w("fill"));
    assert!(w("two") < 100.0, "fit-content: max-content {}", w("two"));
    assert_eq!(w("fit200"), w("two"), "fit-content(200px) parses onto taffy's keyword");
    assert_eq!(w("fit50"), w("two"), "fit-content(50%) parses onto taffy's keyword");
    assert_eq!(w("bad"), w("fill"), "an invalid fit-content() argument drops the declaration");
    assert!(h("autoh") > 0.0);
    assert_eq!(h("minh"), h("autoh"), "a block-axis min-content behaves as auto");
    assert_eq!(h("fith"), h("autoh"), "a block-axis fit-content() behaves as auto");
}
