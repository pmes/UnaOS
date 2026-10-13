//! AETHERSTYLE3B (SR115): the scroll features SR103 refused. `scrollbar-gutter: stable` reserves the 10px bar
//! in layout (and `both-edges` on the start side too); a `scroll-snap-type: y mandatory` box lands each wheel
//! step on the next `scroll-snap-align: start` child and passes the delta outward at its last snap position,
//! `scroll-snap-stop: always` stops a long step at its area, `proximity` snaps only near a position,
//! `scroll-padding` insets the snapport; a programmatic scroll is clamped and snapped (`scroll_target`) and
//! under `scroll-behavior: smooth` eases from the old offset to the target (`SmoothScroll`).
mod common;
use common::*;
use layout_core::scroll;

fn pages(id: &'static str, style: &str, stop_second: bool) -> Spec {
    let page_div = |i: usize| {
        let st = if stop_second && i == 1 { "height: 100px; scroll-snap-align: start; scroll-snap-stop: always" } else { "height: 100px; scroll-snap-align: start" };
        el("div", &[("style", st)], vec![])
    };
    el("div", &[("id", id), ("style", style)], (0..4).map(page_div).collect())
}

#[test]
fn gutter_snap_and_smooth_scrolling() {
    let doc = page(vec![
        el("div", &[("id", "g"), ("style", "width: 100px; height: 50px; overflow-y: auto; scrollbar-gutter: stable")], vec![el("div", &[("id", "gc"), ("style", "height: 20px")], vec![])]),
        el("div", &[("id", "g2"), ("style", "width: 100px; height: 50px; overflow: hidden; scrollbar-gutter: stable both-edges")], vec![el("div", &[("id", "gc2"), ("style", "height: 20px")], vec![])]),
        pages("m", "width: 100px; height: 100px; overflow-y: scroll; scroll-snap-type: y mandatory", false),
        pages("p", "width: 100px; height: 100px; overflow-y: scroll; scroll-snap-type: y proximity", false),
        pages("st", "width: 100px; height: 100px; overflow-y: scroll; scroll-snap-type: y mandatory", true),
        pages("pad", "width: 100px; height: 100px; overflow-y: scroll; scroll-snap-type: y mandatory; scroll-padding-top: 10px; scroll-behavior: smooth", false),
    ]);
    let mut tree = lay(&doc, 400.0, 1000.0);
    // scrollbar-gutter
    let (gx, _, _, _) = box_of(&tree, &doc.by_id("g"));
    let (cx, _, cw, _) = box_of(&tree, &doc.by_id("gc"));
    assert_eq!((cx - gx, cw), (0.0, 90.0), "stable: the bar's 10px are reserved at the end edge");
    let (g2x, _, _, _) = box_of(&tree, &doc.by_id("g2"));
    let (c2x, _, c2w, _) = box_of(&tree, &doc.by_id("gc2"));
    assert_eq!((c2x - g2x, c2w), (10.0, 80.0), "both-edges: and at the start edge");

    // mandatory: a wheel step goes to the next snap position, the delta is consumed
    let m = id_of(&tree, &doc.by_id("m"));
    let ys: Vec<f32> = scroll::snap_positions(&tree, m).1.into_iter().map(|(y, _)| y).collect();
    assert_eq!(ys, vec![0.0, 100.0, 200.0, 300.0]);
    assert_eq!(scroll::scroll_by(&mut tree, &[m], 0.0, 30.0), (0.0, 0.0));
    assert_eq!(scroll::offset(&tree, m), (0.0, 100.0));
    scroll::scroll_by(&mut tree, &[m], 0.0, 30.0);
    assert_eq!(scroll::offset(&tree, m).1, 200.0);
    scroll::scroll_by(&mut tree, &[m], 0.0, -10.0);
    assert_eq!(scroll::offset(&tree, m).1, 100.0);
    scroll::set_offset(&mut tree, m, 0.0, 300.0);
    assert_eq!(scroll::scroll_by(&mut tree, &[m], 0.0, 30.0), (0.0, 30.0), "no snap position ahead: the delta chains out");
    assert_eq!(scroll::offset(&tree, m).1, 300.0);

    // proximity: only near a snap position
    let p = id_of(&tree, &doc.by_id("p"));
    scroll::scroll_by(&mut tree, &[p], 0.0, 10.0);
    assert_eq!(scroll::offset(&tree, p).1, 10.0, "90px from the next position: no snap");
    scroll::scroll_by(&mut tree, &[p], 0.0, 80.0);
    assert_eq!(scroll::offset(&tree, p).1, 100.0, "10px from it: snapped");

    // scroll-snap-stop: always
    let st = id_of(&tree, &doc.by_id("st"));
    scroll::scroll_by(&mut tree, &[st], 0.0, 250.0);
    assert_eq!(scroll::offset(&tree, st).1, 100.0, "a long step stops at the always-stop area");

    // scroll-padding, programmatic scrolls, smooth behavior
    let pad = id_of(&tree, &doc.by_id("pad"));
    let ys: Vec<f32> = scroll::snap_positions(&tree, pad).1.into_iter().map(|(y, _)| y).collect();
    assert_eq!(ys, vec![0.0, 90.0, 190.0, 290.0]);
    assert_eq!(scroll::scroll_target(&tree, pad, 0.0, 150.0), (0.0, 190.0));
    assert_eq!(scroll::scroll_target(&tree, pad, 0.0, 9999.0), (0.0, 290.0), "clamped to the range, then snapped");
    assert!(scroll::smooth_behavior(&tree, pad));
    assert!(!scroll::smooth_behavior(&tree, m));
    let f = scroll::SmoothScroll::new((0.0, 0.0), (0.0, 300.0), 1000.0);
    assert_eq!(f.duration, 240.0);
    assert_eq!(f.at(1000.0), ((0.0, 0.0), false));
    let ((_, mid), done) = f.at(1120.0);
    assert!(!done && (mid - 150.0).abs() < 0.01, "ease-in-out midpoint: {mid}");
    assert_eq!(f.at(1240.0), ((0.0, 300.0), true));
}
