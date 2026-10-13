//! AETHERSTYLE3C (SR122): what SR115's scroll snapping refused. In a 100px `scroll-snap-type: y mandatory`
//! box of four `scroll-snap-align: start` pages, `scroll-margin-top: 20px` on the second page moves its snap
//! position from 100 to 80 (the snap area is the border box outset by the margin); a container snapped to the
//! third page keeps that page across a relayout that grows the first page to 220px — `snapped_targets` names
//! the page, and `resnap` puts the offset on its new position (320), where re-snapping by offset would have
//! landed on the second page (200); with no recorded target it snaps from the offset to the nearest position.
mod common;
use common::*;
use layout_core::scroll;

fn doc(first: u32) -> N {
    let pg = |id: &'static str, st: String| el("div", &[("id", id), ("style", st.as_str())], vec![]);
    page(vec![el(
        "div",
        &[("id", "sc"), ("style", "width: 100px; height: 100px; overflow-y: scroll; scroll-snap-type: y mandatory")],
        vec![
            pg("p1", format!("height: {first}px; scroll-snap-align: start")),
            pg("p2", "height: 100px; scroll-snap-align: start; scroll-margin-top: 20px".into()),
            pg("p3", "height: 100px; scroll-snap-align: start".into()),
            pg("p4", "height: 100px; scroll-snap-align: start".into()),
        ],
    )])
}

#[test]
fn scroll_margin_and_resnap_by_element() {
    let d1 = doc(100);
    let mut t1 = lay(&d1, 400.0, 1000.0);
    let sc = id_of(&t1, &d1.by_id("sc"));
    let ys: Vec<f32> = scroll::snap_positions(&t1, sc).1.into_iter().map(|(y, _)| y).collect();
    assert_eq!(ys, vec![0.0, 80.0, 200.0, 300.0], "scroll-margin-top: 20px snaps the second page at 80");
    assert_eq!(scroll::scroll_by(&mut t1, &[sc], 0.0, 30.0), (0.0, 0.0));
    assert_eq!(scroll::offset(&t1, sc).1, 80.0);

    // snapped to the third page
    scroll::set_offset(&mut t1, sc, 0.0, 200.0);
    let p3 = id_of(&t1, &d1.by_id("p3"));
    assert_eq!(scroll::snapped_targets(&t1, sc), (None, Some(p3)));
    scroll::set_offset(&mut t1, sc, 0.0, 150.0);
    assert_eq!(scroll::snapped_targets(&t1, sc), (None, None), "between positions: no target");

    // the relayout: the first page grows to 220px; the host restores the old offset, then re-snaps
    let d2 = doc(220);
    let mut t2 = lay(&d2, 400.0, 1000.0);
    let sc2 = id_of(&t2, &d2.by_id("sc"));
    let ys: Vec<f32> = scroll::snap_positions(&t2, sc2).1.into_iter().map(|(y, _)| y).collect();
    assert_eq!(ys, vec![0.0, 200.0, 320.0, 420.0]);
    scroll::set_offset(&mut t2, sc2, 0.0, 200.0);
    let p3b = id_of(&t2, &d2.by_id("p3"));
    assert_eq!(scroll::resnap(&mut t2, sc2, (None, Some(p3b))), (0.0, 320.0), "the same element, not the same offset");
    assert_eq!(scroll::offset(&t2, sc2).1, 320.0);
    // no target: the nearest position from the offset
    scroll::set_offset(&mut t2, sc2, 0.0, 190.0);
    assert_eq!(scroll::resnap(&mut t2, sc2, (None, None)), (0.0, 200.0));
}
