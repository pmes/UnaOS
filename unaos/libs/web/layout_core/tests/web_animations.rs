//! AETHERSTYLE3B (SR115): the TransitionEvent / AnimationEvent fields and the Web Animations model on the
//! shared timeline. A transition with a -200ms delay starts with elapsedTime 0.2 and ends with 1.0; a CSS
//! animation's start / iteration / end carry 0, 1 and 2 seconds and removing a running one fires
//! `animationcancel`; a script animation (`Timeline::animate`) samples, pauses, resumes, doubles its rate,
//! finishes (reported once, filling forwards), reverses from its end, finishes backwards and cancels to idle —
//! its play state and current time following each step; `animation_ids` lists transitions, CSS animations and
//! script animations in composite order.
mod common;
use common::*;
use layout_core::anim::{AnimEvent, AnimValue, AnimationSpec, Easing, KeyframeSet, Num, PlayState, ScriptTiming, Timeline, TransitionSpec};
use layout_core::css::ParseCx;

fn opacity(tl: &mut Timeline<usize>, t: f64, cx: &ParseCx) -> Option<AnimValue> {
    tl.sample::<TH>(t, &|_, _| None, cx).into_iter().flat_map(|(_, v)| v).find(|(p, _)| p == "opacity").map(|(_, v)| v)
}

#[test]
fn event_fields_and_script_animations() {
    let cx = ParseCx::new(800.0, 600.0);
    let fade = KeyframeSet { frames: vec![(0.0, None, vec![("opacity".into(), "0".into())]), (1.0, None, vec![("opacity".into(), "1".into())])] };

    // transition events: elapsedTime excludes the delay (a negative delay starts part-way)
    let mut tl: Timeline<usize> = Timeline::new();
    let spec = TransitionSpec { property: "all".into(), duration: 1000.0, delay: -200.0, easing: Easing::Linear, discrete: false };
    assert!(tl.style_change::<TH>(&1, "width", "100px", "200px", Some(&spec), 0.0, &cx));
    let _ = tl.sample::<TH>(0.0, &|_, _| None, &cx);
    let ev = tl.take_timed_events();
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].event.clone(), ev[0].elapsed), (AnimEvent::TransitionStart(1, "width".into()), 0.2));
    assert_eq!((ev[0].event.interface(), ev[0].event.name()), ("TransitionEvent", "width"));
    let _ = tl.sample::<TH>(900.0, &|_, _| None, &cx);
    let ev = tl.take_timed_events();
    assert_eq!((ev[0].event.kind(), ev[0].elapsed), ("transitionend", 1.0));

    // CSS animation events: 0 s, 1 s (second iteration), 2 s (the end)
    let anim = AnimationSpec { name: "fade".into(), duration: 1000.0, delay: 0.0, easing: Easing::Linear, iterations: 2.0, direction: 0, fill: 0, paused: false };
    tl.set_animations(&2, &[anim.clone()], &|_| Some(fade.clone()), 0.0);
    let _ = opacity(&mut tl, 10.0, &cx);
    let _ = opacity(&mut tl, 1250.0, &cx);
    let _ = opacity(&mut tl, 2500.0, &cx);
    let got: Vec<(&'static str, f64)> = tl.take_timed_events().iter().map(|e| (e.event.kind(), e.elapsed)).collect();
    assert_eq!(got, vec![("animationstart", 0.0), ("animationiteration", 1.0), ("animationend", 2.0)]);
    tl.set_animations(&3, &[anim], &|_| Some(fade.clone()), 0.0);
    let _ = opacity(&mut tl, 500.0, &cx);
    let _ = tl.take_timed_events();
    tl.set_animations(&3, &[], &|_| None, 600.0);
    let ev = tl.take_timed_events();
    assert_eq!((ev[0].event.clone(), ev[0].elapsed), (AnimEvent::AnimationCancel(3, "fade".into()), 0.6));

    // a script animation: element.animate([{opacity: 0}, {opacity: 1}], {duration: 1000, fill: 'forwards'})
    let mut tl: Timeline<usize> = Timeline::new();
    let timing = ScriptTiming { duration: 1000.0, fill: 1, ..ScriptTiming::default() };
    let id = tl.animate(&7, fade.clone(), timing, "a1", 0.0);
    assert_eq!(opacity(&mut tl, 500.0, &cx), Some(AnimValue::Nums(vec![Num::N(0.5)])));
    let i = tl.info(id, 500.0).unwrap();
    assert_eq!((i.play_state, i.current_time, i.name.as_str()), (PlayState::Running, Some(500.0), "a1"));
    assert!(tl.pause(id, 500.0));
    assert_eq!(opacity(&mut tl, 900.0, &cx), Some(AnimValue::Nums(vec![Num::N(0.5)])), "paused: held");
    assert_eq!(tl.info(id, 900.0).unwrap().play_state, PlayState::Paused);
    assert!(tl.play(id, 1000.0));
    assert_eq!(opacity(&mut tl, 1250.0, &cx), Some(AnimValue::Nums(vec![Num::N(0.75)])));
    assert!(tl.set_rate(id, 2.0, 1250.0));
    assert_eq!(opacity(&mut tl, 1375.0, &cx), Some(AnimValue::Nums(vec![Num::N(1.0)])), "rate 2: at the end, filling forwards");
    assert_eq!(tl.take_finished(), vec![id]);
    assert!(tl.take_finished().is_empty(), "the finish is reported once");
    assert_eq!(tl.info(id, 1375.0).unwrap().play_state, PlayState::Finished);
    assert!(tl.take_timed_events().is_empty(), "script animations fire no CSS events");
    assert!(tl.reverse(id, 1400.0));
    assert_eq!(tl.info(id, 1400.0).unwrap().rate, -2.0);
    assert_eq!(opacity(&mut tl, 1650.0, &cx), Some(AnimValue::Nums(vec![Num::N(0.5)])), "reversed from its end");
    assert!(tl.finish(id, 1700.0));
    assert_eq!(tl.info(id, 1700.0).unwrap().current_time, Some(0.0));
    assert!(tl.cancel(id, 1700.0));
    assert_eq!(tl.info(id, 1700.0).unwrap().play_state, PlayState::Idle);
    assert_eq!(opacity(&mut tl, 1800.0, &cx), None, "idle: nothing applies");
    assert!(tl.animation_ids(Some(&7)).is_empty());
    let inf = tl.animate(&7, fade.clone(), ScriptTiming { iterations: f64::INFINITY, duration: 10.0, ..ScriptTiming::default() }, "", 0.0);
    assert!(!tl.finish(inf, 0.0), "an infinite end cannot finish");

    // composite order: transitions, CSS animations, script animations
    let mut tl: Timeline<usize> = Timeline::new();
    let s1 = tl.animate(&9, fade.clone(), ScriptTiming { duration: 1000.0, ..ScriptTiming::default() }, "", 0.0);
    let spec = TransitionSpec { property: "all".into(), duration: 1000.0, delay: 0.0, easing: Easing::Linear, discrete: false };
    tl.style_change::<TH>(&9, "width", "1px", "2px", Some(&spec), 0.0, &cx);
    let css = AnimationSpec { name: "fade".into(), duration: 1000.0, delay: 0.0, easing: Easing::Linear, iterations: 1.0, direction: 0, fill: 0, paused: false };
    tl.set_animations(&9, &[css], &|_| Some(fade.clone()), 0.0);
    let ids = tl.animation_ids(Some(&9));
    assert_eq!(ids.len(), 3);
    assert_eq!(ids[2], s1);
    assert_eq!(tl.info(ids[0], 0.0).unwrap().kind, layout_core::anim::AnimKind::Transition);
    assert_eq!(tl.info(ids[1], 0.0).unwrap().kind, layout_core::anim::AnimKind::Css);
}
