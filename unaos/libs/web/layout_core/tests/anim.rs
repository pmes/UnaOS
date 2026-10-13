//! AETHERSTYLE3 (SR103): transitions and keyframe animations. A width transition 100px → 200px (1s,
//! linear) sampled at 500ms is folded onto the fixture box, which lays out 150px wide; reversing it halfway
//! shortens the reverse to 500ms; a two-iteration alternate opacity animation reads 0.25 at 250ms and 0.75 at
//! 1250ms with its start / iteration / end events; the easing functions, the colour mix and the decomposed
//! rotation are checked at their midpoints.
mod common;
use common::*;
use layout_core::anim::{
    apply_value, interpolate, parse_value, AnimEvent, AnimValue, AnimationSpec, Easing, KeyframeSet, Num, Timeline, TransitionSpec,
};
use layout_core::css::{ParseCx, SpecifiedStyle};
use std::rc::Rc;

#[test]
fn transitions_animations_and_interpolation() {
    let cx = ParseCx::new(800.0, 600.0);
    let doc = page(vec![el("div", &[("id", "a"), ("style", "width: 100px; height: 10px")], vec![])]);
    let a = doc.by_id("a");

    // a 1s linear width transition, sampled halfway and folded onto the box
    let spec = TransitionSpec { property: "all".into(), duration: 1000.0, delay: 0.0, easing: Easing::Linear, discrete: false };
    let mut tl: Timeline<usize> = Timeline::new();
    assert!(tl.style_change::<TH>(&a.id, "width", "100px", "200px", Some(&spec), 0.0, &cx));
    let vals = tl.sample::<TH>(500.0, &|_, _| None, &cx);
    assert_eq!(vals.len(), 1);
    let (prop, v) = &vals[0].1[0];
    assert_eq!((prop.as_str(), v), ("width", &AnimValue::Nums(vec![Num::Px(150.0)])));
    let mut s = SpecifiedStyle::<TH>::default();
    apply_value(prop, v, &mut s, &cx);
    let tree = lay_styled(&doc, &Sheet(vec![(a.id, Rc::new(s))]), 800.0, 600.0);
    assert_eq!(box_of(&tree, &a).2, 150.0, "the animated width lays out");
    assert!(tl.take_events().contains(&AnimEvent::TransitionStart(a.id, "width".into())));

    // reversing halfway: the reverse runs from 150px and takes half the duration (css-transitions-1 §3)
    assert!(tl.style_change::<TH>(&a.id, "width", "200px", "100px", Some(&spec), 500.0, &cx));
    let mid = tl.sample::<TH>(750.0, &|_, _| None, &cx);
    assert_eq!(mid[0].1[0].1, AnimValue::Nums(vec![Num::Px(125.0)]));
    let _ = tl.sample::<TH>(1001.0, &|_, _| None, &cx);
    assert!(tl.take_events().contains(&AnimEvent::TransitionEnd(a.id, "width".into())));
    assert!(!tl.active());

    // keyframes: two alternate iterations of opacity 0 -> 1
    let sheet = css_core::stylesheet::parse_stylesheet("@keyframes fade { from { opacity: 0 } to { opacity: 1 } }");
    let kf = match &sheet.rules[0] {
        css_core::stylesheet::CssRule::Keyframes(k) => KeyframeSet::from_rule(k),
        _ => panic!("keyframes"),
    };
    let anim = AnimationSpec { name: "fade".into(), duration: 1000.0, delay: 0.0, easing: Easing::Linear, iterations: 2.0, direction: 2, fill: 0, paused: false };
    tl.set_animations(&a.id, &[anim], &|n| (n == "fade").then(|| kf.clone()), 0.0);
    let op = |tl: &mut Timeline<usize>, t: f64| -> Option<AnimValue> {
        tl.sample::<TH>(t, &|_, _| None, &cx).into_iter().flat_map(|(_, v)| v).find(|(p, _)| p == "opacity").map(|(_, v)| v)
    };
    assert_eq!(op(&mut tl, 250.0), Some(AnimValue::Nums(vec![Num::N(0.25)])));
    assert_eq!(op(&mut tl, 1250.0), Some(AnimValue::Nums(vec![Num::N(0.75)])), "the second iteration runs backwards");
    assert_eq!(op(&mut tl, 2500.0), None, "fill: none — nothing after the end");
    let ev = tl.take_events();
    assert!(ev.contains(&AnimEvent::AnimationStart(a.id, "fade".into())));
    assert!(ev.contains(&AnimEvent::AnimationIteration(a.id, "fade".into())));
    assert!(ev.contains(&AnimEvent::AnimationEnd(a.id, "fade".into())));

    // easing functions
    let ease = Easing::parse("ease").unwrap().apply(0.5, false);
    assert!((ease - 0.8024).abs() < 0.002, "ease(0.5) = {ease}");
    assert_eq!(Easing::parse("steps(4, end)").unwrap().apply(0.3, false), 0.25);
    assert_eq!(Easing::parse("steps(4, jump-start)").unwrap().apply(0.3, false), 0.5);
    assert!((Easing::parse("linear(0, 0.25 75%, 1)").unwrap().apply(0.75, false) - 0.25).abs() < 1e-6);
    assert!(Easing::parse("cubic-bezier(2, 0, 0, 1)").is_none());

    // colours mix in premultiplied sRGB; a rotation interpolates through its decomposition
    let red = parse_value::<TH>("color", "red", &cx);
    let blue = parse_value::<TH>("color", "rgb(0, 0, 255)", &cx);
    match interpolate("color", &red, &blue, 0.5) {
        AnimValue::Color(c) => assert!((c[0] - 127.5).abs() < 0.01 && (c[2] - 127.5).abs() < 0.01 && c[3] == 1.0),
        v => panic!("{v:?}"),
    }
    let r0 = parse_value::<TH>("transform", "rotate(0deg) scale(2)", &cx);
    let r90 = parse_value::<TH>("transform", "rotate(90deg)", &cx);
    match interpolate("transform", &r0, &r90, 0.5) {
        AnimValue::Transform(ops) => {
            let m = ops.iter().fold(layout_core::paint::IDENTITY, |m, o| layout_core::paint::mat_mul(m, o.matrix(0.0, 0.0)));
            // halfway: rotated 45deg, scaled 1.5
            assert!((m[0] - 1.5 * 0.70711).abs() < 0.01 && (m[1] - 1.5 * 0.70711).abs() < 0.01, "{m:?}");
        }
        v => panic!("{v:?}"),
    }
}
