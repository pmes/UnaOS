//! AETHERSTYLE3C (SR122): what SR115's Web Animations model refused, on the shared timeline. A script
//! animation with `composite: add` lays opacity 0.5 over the base 0.25 (0.75); transform lists append under
//! `add` and accumulate function by function (scale 2 accumulated with scale 3 is scale 4); `iterationStart:
//! 0.5` starts mid-iteration (0.5 at 0 ms, iteration 1 at 0.1 at 600 ms, filling at 0.5 after the end); a new
//! or played / paused animation is pending until the next sample (start time unresolved, then reported by
//! `take_ready`); `effect_values` is commitStyles' answer; a CSS transition pauses at its current value and
//! resumes from it.
mod common;
use common::*;
use layout_core::anim::{AnimValue, Easing, KeyframeSet, Num, PlayState, ScriptTiming, Timeline, TransitionSpec};
use layout_core::css::ParseCx;
use layout_core::paint::TransformOp;

fn frames(prop: &str, a: &str, b: &str) -> KeyframeSet {
    KeyframeSet { frames: vec![(0.0, None, vec![(prop.into(), a.into())]), (1.0, None, vec![(prop.into(), b.into())])] }
}

fn value(out: &[(usize, Vec<(String, AnimValue)>)], key: usize, prop: &str) -> Option<AnimValue> {
    out.iter().find(|(k, _)| *k == key).and_then(|(_, l)| l.iter().find(|(p, _)| p == prop).map(|(_, v)| v.clone()))
}

fn near(v: Option<AnimValue>, want: f32) -> bool {
    matches!(v, Some(AnimValue::Nums(ref n)) if matches!(n.as_slice(), [Num::N(x)] | [Num::Px(x)] if (x - want).abs() < 1e-3))
}

#[test]
fn composite_iteration_start_pending_commit_and_transition_pause() {
    let cx = ParseCx::new(800.0, 600.0);
    let base = |_: &usize, p: &str| match p {
        "opacity" => Some(String::from("0.25")),
        "transform" => Some(String::from("scale(2)")),
        _ => None,
    };
    let mut tl: Timeline<usize> = Timeline::new();

    // composite: add, and the play-pending state of a new animation
    let add = tl.animate(&1, frames("opacity", "0.5", "0.5"), ScriptTiming { duration: 1000.0, composite: 1, ..ScriptTiming::default() }, "", 0.0);
    let i = tl.info(add, 0.0).unwrap();
    assert!(i.pending && i.start_time.is_none(), "pending until the next frame");
    let out = tl.sample::<TH>(100.0, &base, &cx);
    assert!(near(value(&out, 1, "opacity"), 0.75), "0.25 + 0.5");
    assert_eq!(tl.take_ready(), vec![add]);
    let i = tl.info(add, 100.0).unwrap();
    assert!(!i.pending && i.start_time == Some(0.0) && i.timing.composite == 1);

    // transforms: add appends, accumulate combines
    let ta = tl.animate(&4, frames("transform", "scale(3)", "scale(3)"), ScriptTiming { duration: 1000.0, composite: 1, ..ScriptTiming::default() }, "", 0.0);
    let tc = tl.animate(&5, frames("transform", "scale(3)", "scale(3)"), ScriptTiming { duration: 1000.0, composite: 2, ..ScriptTiming::default() }, "", 0.0);
    let out = tl.sample::<TH>(100.0, &base, &cx);
    match value(&out, 4, "transform") {
        Some(AnimValue::Transform(ops)) => assert_eq!(ops.len(), 2, "add: the base list then the effect's"),
        v => panic!("add: {v:?}"),
    }
    match value(&out, 5, "transform") {
        Some(AnimValue::Transform(ops)) => match ops.as_slice() {
            [TransformOp::Matrix(m)] => assert!((m[0] - 4.0).abs() < 1e-3 && (m[3] - 4.0).abs() < 1e-3 && m[1].abs() < 1e-3, "{m:?}"),
            o => panic!("accumulate: {o:?}"),
        },
        v => panic!("accumulate: {v:?}"),
    }
    let _ = (ta, tc, tl.take_ready());

    // iterationStart
    let fade = frames("opacity", "0", "1");
    let is = tl.animate(&2, fade, ScriptTiming { duration: 1000.0, iterations: 2.0, iteration_start: 0.5, fill: 1, ..ScriptTiming::default() }, "", 0.0);
    assert!(near(value(&tl.sample::<TH>(0.0, &base, &cx), 2, "opacity"), 0.5), "starts half-way through an iteration");
    assert!(near(value(&tl.sample::<TH>(600.0, &base, &cx), 2, "opacity"), 0.1));
    let i = tl.info(is, 600.0).unwrap();
    assert_eq!(i.current_iteration, Some(1.0));
    assert!((i.progress.unwrap() - 0.1).abs() < 1e-9);
    assert!(near(value(&tl.sample::<TH>(2500.0, &base, &cx), 2, "opacity"), 0.5), "after the end: 0.5 + 2 iterations, filling");

    // commitStyles' values: the composited value now
    let vals = tl.effect_values::<TH>(add, 100.0, &|p| base(&1, p), &cx).unwrap();
    assert_eq!(vals.len(), 1);
    assert_eq!((vals[0].0.as_str(), layout_core::anim::to_css(&vals[0].1)), ("opacity", String::from("0.75")));
    assert_eq!(layout_core::anim::transform_css(&[TransformOp::Translate([10.0, 0.5], [0.0, 0.0])]), "translate(calc(10px + 50%), 0px)");
    assert!(tl.effect_values::<TH>(9999, 0.0, &|_| None, &cx).is_none());

    // a CSS transition paused and played
    let spec = TransitionSpec { property: "all".into(), duration: 1000.0, delay: 0.0, easing: Easing::Linear, discrete: false };
    assert!(tl.style_change::<TH>(&3, "width", "100px", "200px", Some(&spec), 0.0, &cx));
    let t = tl.animation_ids(Some(&3))[0];
    let _ = tl.sample::<TH>(0.0, &base, &cx);
    assert!(tl.pause(t, 500.0));
    let i = tl.info(t, 900.0).unwrap();
    assert_eq!((i.play_state, i.current_time, i.pending, i.start_time), (PlayState::Paused, Some(500.0), true, None));
    assert!(near(value(&tl.sample::<TH>(900.0, &base, &cx), 3, "width"), 150.0), "held at its paused value");
    assert!(tl.take_ready().contains(&t));
    assert!(tl.play(t, 1000.0));
    assert_eq!(tl.info(t, 1000.0).unwrap().play_state, PlayState::Running);
    assert!(near(value(&tl.sample::<TH>(1250.0, &base, &cx), 3, "width"), 175.0), "resumed from 500 ms");
}
