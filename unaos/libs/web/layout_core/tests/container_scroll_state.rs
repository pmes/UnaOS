//! AETHERSTYLE3C (SR122): what SR115's container queries refused. `@container scroll-state(...)` rules
//! evaluate against the nearest `container-type: scroll-state` ancestor's state as a first layout leaves it: a
//! snap container scrolled to 100 of 200 is `scrollable: top` (and `block-end`), the page it is snapped to is
//! `snapped: y`, a `position: sticky; top: 0` box the viewport has scrolled past is `stuck: top` (so `not
//! scroll-state(stuck)` does not apply); a bare feature means "not none", the axis and logical keywords map
//! through the writing mode. A standard property inside `style()` compares the query container's resolved
//! declaration (keywords case-insensitively); only such rules ask the caller for declarations.
mod common;
use common::*;
use css_core::container::{ContainerRule, ScrollState, AXIS_Y, BOTTOM, LEFT, RIGHT, TOP};
use css_core::media::Tri;
use layout_core::scroll;
use layout_core::DomNode;

const SHEET: &str = "
@container scroll-state(scrollable: top) { .pk { width: 30px } }
@container scroll-state(snapped: y) { .sn { width: 40px } }
@container scroll-state(stuck: top) { .sk { width: 50px } }
@container not scroll-state(stuck) { .nk { width: 60px } }
@container style(color: RED) { .c { width: 1px } }
@container style(--x: 1) { .c { width: 2px } }
";

#[test]
fn scroll_state_and_standard_style_queries() {
    let sheet = css_core::stylesheet::parse_stylesheet(SHEET);
    let env = css_core::media::Environment::viewport(800.0, 600.0);
    let supports = |_: &css_core::parser::Declaration| true;
    let import = |_: &str| -> Option<&css_core::stylesheet::Stylesheet> { None };
    let cond = css_core::cascade::Conditions { env: &env, supports: &supports, import: &import };
    let mut rules = css_core::cascade::RuleSet::new();
    rules.add_sheet(&sheet, css_core::cascade::Origin::Author, &cond);
    let rule = |i: usize| -> &ContainerRule { rules.entries[i].container[0] };

    // the evaluator alone
    let st = ScrollState { stuck: TOP, snapped: AXIS_Y, scrollable: TOP | BOTTOM, vertical: false, rtl: false };
    let none = |_: &str| -> Option<String> { None };
    assert!(rule(0).needs_scroll_state() && !rule(0).needs_size());
    assert_eq!(rule(0).evaluate_full(None, &none, Some(&st)), Tri::True);
    assert_eq!(rule(0).evaluate_full(None, &none, None), Tri::Unknown, "no scroll-state container: unknown");
    assert_eq!(rule(2).evaluate_full(None, &none, Some(&ScrollState::default())), Tri::False);
    assert_eq!(rule(3).evaluate_full(None, &none, Some(&st)), Tri::False, "stuck somewhere: not (stuck) is false");
    let vert = ScrollState { scrollable: LEFT, vertical: true, ..ScrollState::default() };
    let parse = |src: &str| {
        let s = css_core::stylesheet::parse_stylesheet(&format!("@container {src} {{ .q {{ width: 1px }} }}"));
        let env = css_core::media::Environment::viewport(800.0, 600.0);
        let supports = |_: &css_core::parser::Declaration| true;
        let import = |_: &str| -> Option<&css_core::stylesheet::Stylesheet> { None };
        let cond = css_core::cascade::Conditions { env: &env, supports: &supports, import: &import };
        let mut r = css_core::cascade::RuleSet::new();
        r.add_sheet(&s, css_core::cascade::Origin::Author, &cond);
        r.entries[0].container[0].clone()
    };
    assert_eq!(parse("scroll-state(scrollable: block-end)").evaluate_full(None, &none, Some(&vert)), Tri::True, "vertical-rl: block-end is left");
    assert_eq!(parse("scroll-state(scrollable: x)").evaluate_full(None, &none, Some(&vert)), Tri::True);
    assert_eq!(parse("scroll-state(scrollable: y)").evaluate_full(None, &none, Some(&vert)), Tri::False);
    assert_eq!(parse("scroll-state((snapped: both) or (stuck: right))").evaluate_full(None, &none, Some(&ScrollState { stuck: RIGHT, ..st })), Tri::True);

    // standard properties in style()
    let red = |n: &str| (n == "color").then(|| String::from("red"));
    assert!(rule(4).queries_standard_style() && !rule(5).queries_standard_style());
    assert_eq!(rule(4).evaluate_full(None, &red, None), Tri::True, "keywords compare case-insensitively");
    assert_eq!(parse("style(color: blue)").evaluate_full(None, &red, None), Tri::False);
    assert_eq!(parse("style(color)").evaluate_full(None, &red, None), Tri::True);

    // end to end: a first layout, the states, the second cascade
    let pg = |id: &'static str, extra: &str, kids: Vec<Spec>| {
        el("div", &[("id", id), ("class", id), ("style", format!("height: 100px; scroll-snap-align: start; {extra}").as_str())], kids)
    };
    let doc = page(vec![
        el(
            "div",
            &[("id", "sc"), ("style", "width: 200px; height: 100px; overflow-y: scroll; scroll-snap-type: y mandatory; container-type: scroll-state")],
            vec![
                pg("p1", "", vec![el("div", &[("id", "pk"), ("class", "pk")], vec![])]),
                pg("p2", "container-type: scroll-state", vec![el("div", &[("id", "sn"), ("class", "sn")], vec![])]),
                pg("p3", "", vec![]),
            ],
        ),
        el(
            "div",
            &[("style", "height: 1000px")],
            vec![
                el("div", &[("style", "height: 100px")], vec![]),
                el(
                    "div",
                    &[("id", "st"), ("style", "position: sticky; top: 0px; height: 20px; container-type: scroll-state")],
                    vec![el("div", &[("id", "sk"), ("class", "sk")], vec![]), el("div", &[("id", "nk"), ("class", "nk")], vec![])],
                ),
            ],
        ),
    ]);
    let first = cascade_sheet(&doc, &rules, None, 800.0, 600.0);
    let mut t1 = lay_styled(&doc, &first, 800.0, 600.0);
    let sc = id_of(&t1, &doc.by_id("sc"));
    scroll::set_offset(&mut t1, sc, 0.0, 100.0);
    let vs = (0.0, 250.0);
    let s_sc = scroll::scroll_state(&t1, sc, vs, (800.0, 600.0));
    assert_eq!(s_sc.scrollable, TOP | BOTTOM);
    assert_eq!(scroll::scroll_state(&t1, id_of(&t1, &doc.by_id("p2")), vs, (800.0, 600.0)).snapped, AXIS_Y);
    assert_eq!(scroll::sticky_stuck(&t1, id_of(&t1, &doc.by_id("st")), vs, (800.0, 600.0)), TOP, "200px down, scrolled 250: held at the top");
    assert_eq!(scroll::sticky_stuck(&t1, id_of(&t1, &doc.by_id("st")), (0.0, 0.0), (800.0, 600.0)), 0);

    let query = |e: &N, c: &ContainerRule| -> bool {
        let mut cur = DomNode::parent(e);
        while let Some(a) = cur {
            if let Some((tid, _)) = t1.node_map.iter().find(|(_, n)| n.id == a.id) {
                if t1.paint_map.get(tid).and_then(|p| p.container_scroll) == Some(true) {
                    let st = scroll::scroll_state(&t1, *tid, vs, (800.0, 600.0));
                    return c.evaluate_full(None, &|_| None, Some(&st)) == Tri::True;
                }
            }
            cur = DomNode::parent(&a);
        }
        false
    };
    let second = cascade_sheet(&doc, &rules, Some(&query), 800.0, 600.0);
    let t2 = lay_styled(&doc, &second, 800.0, 600.0);
    assert_eq!(box_of(&t2, &doc.by_id("pk")).2, 30.0, "scrollable: top");
    assert_eq!(box_of(&t2, &doc.by_id("sn")).2, 40.0, "snapped: y");
    assert_eq!(box_of(&t2, &doc.by_id("sk")).2, 50.0, "stuck: top");
    assert_eq!(box_of(&t2, &doc.by_id("nk")).2, 800.0, "not scroll-state(stuck) does not apply");
}
