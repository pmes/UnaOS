//! A11YFACE (SR124): css-ui-4 §4 outlines. `outline` is its own paint — outside the border box grown by
//! `outline-offset`, taking no space — and never the border: `outline: none` leaves a button's border alone,
//! `outline: 0` is no outline, `auto` is kept for the host's focus ring, the shorthand resets what it omits
//! (width medium, colour currentColor), and a negative offset never turns the band inside out.
mod common;
use common::*;
use layout_core::paint::{self, Outline, OUTLINE_AUTO, OUTLINE_DASHED, OUTLINE_NONE, OUTLINE_SOLID};

#[test]
fn outline_paints_outside_the_border_box_and_is_never_a_border() {
    let doc = page(vec![
        el("div", &[("id", "o"), ("style", "width: 100px; height: 40px; outline: 2px dashed rgb(255, 0, 0); outline-offset: 3px")], vec![]),
        el("button", &[("id", "b"), ("style", "border: 1px solid rgb(0, 0, 255); outline: none")], vec![text("Go")]),
        el("div", &[("id", "a"), ("style", "outline: auto")], vec![]),
        el("div", &[("id", "z"), ("style", "outline: 0")], vec![]),
        el("div", &[("id", "l"), ("style", "outline-style: solid; outline-color: currentcolor")], vec![]),
        el("div", &[("id", "t"), ("style", "outline: thick double transparent")], vec![]),
    ]);
    let tree = lay(&doc, 400.0, 300.0);
    let p = |id: &str| tree.paint_map.get(&id_of(&tree, &doc.by_id(id))).cloned().unwrap_or_default();

    let o = p("o");
    assert!(o.border.is_none(), "an outline is not a border: {:?}", o.border);
    assert_eq!(paint::outline_of(&o), Some(Outline { style: OUTLINE_DASHED, width: 2.0, offset: 3.0, color: Some((255, 0, 0, 255)) }));
    let bb = box_of(&tree, &doc.by_id("o"));
    assert_eq!((bb.2, bb.3), (100.0, 40.0), "an outline takes no space");
    let (inner, outer) = paint::outline_band(bb, 2.0, 3.0);
    assert_eq!(inner, (bb.0 - 3.0, bb.1 - 3.0, 106.0, 46.0), "the inner edge is the border box grown by the offset");
    assert_eq!(outer, (bb.0 - 5.0, bb.1 - 5.0, 110.0, 50.0), "the outer edge is the inner grown by the width");

    let b = p("b");
    assert_eq!(b.outline_style, Some(OUTLINE_NONE));
    assert_eq!(paint::outline_of(&b), None);
    assert!(b.border.is_some_and(|s| s.iter().all(|side| side.is_some())), "outline: none keeps the border: {:?}", b.border);

    assert_eq!(paint::outline_of(&p("a")).map(|o| o.style), Some(OUTLINE_AUTO), "auto is the host's focus ring");
    assert_eq!(paint::outline_of(&p("z")), None, "outline: 0 is no outline");
    assert_eq!(
        paint::outline_of(&p("l")),
        Some(Outline { style: OUTLINE_SOLID, width: 3.0, offset: 0.0, color: None }),
        "a style alone: medium width, currentColor"
    );
    assert_eq!(paint::outline_of(&p("t")).map(|o| (o.width, o.color)), Some((5.0, Some((0, 0, 0, 0)))), "thick, transparent");

    // A negative offset shrinks the band's inner edge, never past the box's centre.
    assert_eq!(paint::outline_band((0.0, 0.0, 10.0, 4.0), 1.0, -5.0).0, (2.0, 2.0, 6.0, 0.0));
}

/// The host's `outline: none` rule asks css_core whether a selector names `:focus-visible` (in any position).
#[test]
fn selectors_report_the_focus_visible_state() {
    use css_core::selectors::{parse_selector_str, ElementState, Namespaces};
    let ns = Namespaces::default();
    let names = |s: &str| parse_selector_str(s, &ns).unwrap().mentions_state(ElementState::FocusVisible);
    assert!(names("button:focus-visible"));
    assert!(names(".a, :is(.b, .c:focus-visible) > span"));
    assert!(names("li:has(> a:focus-visible)"));
    assert!(!names("button:focus"));
    assert!(!names("a:hover, .x"));
}
