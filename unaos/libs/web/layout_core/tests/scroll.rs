//! AETHERSTYLE3 (SR103): scroll containers. The fixture's `overflow: auto` box keeps its 50px height with
//! 200px of content (its content overflows instead of shrinking), shows a vertical overlay scrollbar only, and a
//! wheel delta over it scrolls it first and chains the rest to the `overflow-y: scroll` box around it; the text
//! inside moves with the offset.
mod common;
use common::*;
use layout_core::scroll;

#[test]
fn nested_scrollers_chain_and_shift_content() {
    let doc = page(vec![el(
        "div",
        &[("id", "outer"), ("style", "width: 200px; height: 100px; overflow-y: scroll")],
        vec![
            el(
                "div",
                &[("id", "inner"), ("style", "width: 100px; height: 50px; overflow: auto")],
                vec![el("div", &[("id", "tall"), ("style", "height: 200px")], vec![text("hello")])],
            ),
            el("div", &[("id", "after"), ("style", "height: 300px")], vec![]),
        ],
    )]);
    let mut tree = lay(&doc, 400.0, 300.0);
    let (outer, inner) = (doc.by_id("outer"), doc.by_id("inner"));
    assert_eq!(box_of(&tree, &inner), (0.0, 0.0, 100.0, 50.0));
    assert_eq!(box_of(&tree, &doc.by_id("tall")).3, 200.0, "content overflows, it does not shrink");
    let (oi, ii) = (id_of(&tree, &outer), id_of(&tree, &inner));
    assert_eq!(scroll::overflow_of(&tree, ii), [scroll::AUTO, scroll::AUTO]);
    assert_eq!(scroll::max_scroll(&tree, ii), (0.0, 150.0));
    assert_eq!(scroll::max_scroll(&tree, oi), (0.0, 250.0));
    let bars = scroll::scrollbars(&tree, ii);
    assert!(bars.v.is_some() && bars.h.is_none(), "auto: a bar only on the overflowing axis");
    let v = bars.v.unwrap();
    assert_eq!(v.track, (90.0, 0.0, 10.0, 50.0));
    assert!((v.thumb.3 - 12.5f32.max(scroll::MIN_THUMB)).abs() < 0.01);

    let chain = scroll::scrollers_at(&tree, 20.0, 20.0, (0.0, 0.0));
    assert_eq!(chain, vec![ii, oi]);
    let left = scroll::scroll_by(&mut tree, &chain, 0.0, 200.0);
    assert_eq!(left, (0.0, 0.0));
    assert_eq!(scroll::offset(&tree, ii), (0.0, 150.0));
    assert_eq!(scroll::offset(&tree, oi), (0.0, 50.0));
    assert_eq!(scroll::scroll_by(&mut tree, &chain, 0.0, 1000.0), (0.0, 800.0), "the rest goes to the viewport");

    // the text box inside moves by both offsets
    let tb = layout_core::hit::text_boxes(&tree);
    assert_eq!(tb.len(), 1);
    assert!((tb[0].top - (-150.0 - 250.0)).abs() < 0.01, "top {}", tb[0].top);
    assert_eq!(scroll::box_origin(&tree, id_of(&tree, &doc.by_id("tall")), (0.0, 0.0)), Some((0.0, -400.0)));

    // a thumb drag maps the thumb's travel onto the range
    assert_eq!(scroll::drag_offset(&tree, ii, 1, 0.0, 1000.0), 150.0);
    assert!(scroll::hit_scrollbar(&tree, ii, 95.0, 49.0).is_some_and(|(axis, on, _)| axis == 1 && on));
}
