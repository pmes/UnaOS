//! AETHERSTYLE3 (SR103): the host's side of `layout_core::anim` — the one [`Timeline`] of the page, keyed by
//! element, driven per frame by `AetherEngine::tick`.
//!
//! * [`sync`] runs after every cascade (`css::apply_stylesheets`): each element's new computed style is
//!   compared with the one before it (the before-change style, css-transitions-1 §3) — a changed animatable
//!   value under a matching `transition` starts or retargets a transition; the `animation-name` list starts,
//!   keeps or cancels animations with the document's `@keyframes`; then the current animated values are applied.
//! * [`frame`] samples the timeline at the frame time and re-applies the animated values onto the boxes (the
//!   values are folded over the cascaded style with `layout_core::apply_specs`; a layout property re-lays out
//!   with `remeasure`, a paint-only one only repaints). When a value stops being animated the engine relayouts
//!   from the cascade, which is how the element returns to its un-animated value.
//! * [`take_events`] hands the queued `transition*` / `animation*` events to the engine to dispatch.
//!
//! AETHERSTYLE3B (SR115): the events carry their `elapsedTime` (`layout_core::anim::TimedEvent`) and are
//! dispatched by the script lane as TransitionEvent / AnimationEvent with their fields
//! (`js::animations::poll` takes them with [`take_timed_events`]); [`take_events`] now hands the engine none,
//! so the plain-`Event` loop in lib.rs no longer double-fires them. The Web Animations binding drives the same
//! timeline ([`with_timeline`], [`poke`]).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use html_core::NodeId;
use layout_core::anim::{initial_value, is_paint_only, AnimEvent, KeyframeSet, TimedEvent, Timeline, TransitionLists};

use super::{parse_cx, SpecifiedStyle};
use crate::layout::{AetherHost, LayoutTree};

thread_local! {
    static TIMELINE: RefCell<Timeline<NodeId>> = RefCell::new(Timeline::new());
    /// The computed style of each element as of the last cascade (the next change's before-change style).
    static PREV: RefCell<HashMap<NodeId, Rc<SpecifiedStyle>>> = RefCell::new(HashMap::new());
    static KEYFRAMES: RefCell<HashMap<String, KeyframeSet>> = RefCell::new(HashMap::new());
    /// What the last frame animated: (element, property).
    static APPLIED: RefCell<HashSet<(NodeId, String)>> = RefCell::new(HashSet::new());
    static EPOCH: std::time::Instant = std::time::Instant::now();
    /// AETHERSTYLE3B (SR115): the timed events not yet dispatched (bounded: a page without script drops the
    /// oldest), and a script change to the timeline that the next frame must sample.
    static PENDING: RefCell<Vec<TimedEvent<NodeId>>> = const { RefCell::new(Vec::new()) };
    static POKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// How many undispatched events are kept.
const MAX_PENDING: usize = 1024;

/// Moves the timeline's new events to the pending queue.
fn drain_events() {
    let new = TIMELINE.with(|t| t.borrow_mut().take_timed_events());
    if new.is_empty() {
        return;
    }
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        p.extend(new);
        let over = p.len().saturating_sub(MAX_PENDING);
        if over > 0 {
            p.drain(..over);
        }
    });
}

/// AETHERSTYLE3B (SR115): the Web Animations binding's access to the page's timeline.
pub(crate) fn with_timeline<R>(f: impl FnOnce(&mut Timeline<NodeId>) -> R) -> R {
    TIMELINE.with(|t| f(&mut t.borrow_mut()))
}

/// A script changed the timeline (paused, seeked, cancelled …): the next frame samples it even when nothing runs.
pub(crate) fn poke() {
    POKED.with(|p| p.set(true));
}

/// The cascaded (base) value of `prop` on `el`: what an animation's implicit keyframe holds.
pub(crate) fn base_value(el: NodeId, prop: &str) -> Option<String> {
    PREV.with(|p| value_of(p.borrow().get(&el).map(|s| &**s), prop))
}

/// AETHERSTYLE3C (SR122): `commitStyles()` — the values animation `id` applies now on `el`, as (property, CSS
/// text), composited onto `el`'s cascaded values (`layout_core::anim::Timeline::effect_values`).
pub(crate) fn commit_values(id: u64, el: NodeId) -> Vec<(String, String)> {
    let cx = parse_cx();
    let now = now_ms();
    let vals = PREV.with(|p| {
        let p = p.borrow();
        let base = |prop: &str| value_of(p.get(&el).map(|s| &**s), prop);
        TIMELINE.with(|t| t.borrow().effect_values::<AetherHost>(id, now, &base, &cx))
    });
    vals.unwrap_or_default().into_iter().map(|(p, v)| (p, layout_core::anim::to_css(&v))).collect()
}

/// AETHERSTYLE3B (SR115): the events since the last call, with their `elapsedTime`, for the script lane.
pub(crate) fn take_timed_events() -> Vec<TimedEvent<NodeId>> {
    drain_events();
    PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// The frame clock in milliseconds.
pub(crate) fn now_ms() -> f64 {
    EPOCH.with(|e| e.elapsed().as_secs_f64() * 1000.0)
}

/// A new document: nothing runs, no before-change styles.
pub(crate) fn reset() {
    TIMELINE.with(|t| *t.borrow_mut() = Timeline::new());
    PENDING.with(|p| p.borrow_mut().clear());
    POKED.with(|p| p.set(false));
    PREV.with(|p| p.borrow_mut().clear());
    KEYFRAMES.with(|k| k.borrow_mut().clear());
    APPLIED.with(|a| a.borrow_mut().clear());
}

/// The animatable value `prop` has in `style` (its own declaration, else the initial value this engine knows).
fn value_of(style: Option<&SpecifiedStyle>, prop: &str) -> Option<String> {
    style
        .and_then(|s| s.animatable.iter().rev().find(|(p, _)| p == prop).map(|(_, v)| v.clone()))
        .or_else(|| initial_value(prop).map(String::from))
}

/// After a cascade: transitions for the changed values, the animation lists, the current values applied.
pub(crate) fn sync(tree: &mut LayoutTree, styles: &HashMap<NodeId, Rc<SpecifiedStyle>>, keyframes: Vec<(String, KeyframeSet)>) {
    KEYFRAMES.with(|k| {
        let mut k = k.borrow_mut();
        k.clear();
        k.extend(keyframes);
    });
    let now = now_ms();
    let cx = parse_cx();
    let prev = PREV.with(|p| std::mem::take(&mut *p.borrow_mut()));
    let any_rules = !prev.is_empty() || styles.values().any(|s| s.transition != TransitionLists::default() || s.animation.name.is_some());
    if any_rules {
        TIMELINE.with(|t| {
            let mut t = t.borrow_mut();
            for (id, st) in styles {
                if let Some(old) = prev.get(id) {
                    let specs = st.transition.resolve();
                    if !specs.is_empty() || !t.keys().is_empty() {
                        let mut props: Vec<&str> = old.animatable.iter().map(|(p, _)| p.as_str()).collect();
                        for (p, _) in &st.animatable {
                            if !props.contains(&p.as_str()) {
                                props.push(p);
                            }
                        }
                        for prop in props {
                            let (Some(before), Some(after)) = (value_of(Some(&**old), prop), value_of(Some(&**st), prop)) else { continue };
                            if before == after {
                                continue;
                            }
                            let spec = TransitionLists::spec_for(&specs, prop);
                            t.style_change::<AetherHost>(id, prop, &before, &after, spec, now, &cx);
                        }
                    }
                }
                let anims = st.animation.resolve();
                KEYFRAMES.with(|k| {
                    let k = k.borrow();
                    t.set_animations(id, &anims, &|n| k.get(n).cloned(), now);
                });
            }
            // AETHERSTYLE3B (SR115): an element without a computed style keeps its script animations (it is
            // still in the document); its CSS animations end. Only an element with no box is forgotten.
            let boxed: HashSet<NodeId> = tree.node_map.values().map(|n| n.id()).collect();
            for key in t.keys() {
                if !styles.contains_key(&key) {
                    if boxed.contains(&key) {
                        t.set_animations(&key, &[], &|_| None, now);
                    } else {
                        t.forget(&key);
                    }
                }
            }
        });
    }
    PREV.with(|p| *p.borrow_mut() = styles.clone());
    let values = sample(now);
    drain_events();
    APPLIED.with(|a| *a.borrow_mut() = values.iter().flat_map(|(k, v)| v.iter().map(move |(p, _)| (*k, p.clone()))).collect());
    if !values.is_empty() {
        apply(tree, &values);
    }
}

fn sample(now: f64) -> Vec<(NodeId, Vec<(String, layout_core::anim::AnimValue)>)> {
    let cx = parse_cx();
    PREV.with(|p| {
        let p = p.borrow();
        let base = |k: &NodeId, prop: &str| value_of(p.get(k).map(|s| &**s), prop);
        TIMELINE.with(|t| t.borrow_mut().sample::<AetherHost>(now, &base, &cx))
    })
}

/// Folds the animated values onto each element's boxes; re-lays out when a layout property moved.
fn apply(tree: &mut LayoutTree, values: &[(NodeId, Vec<(String, layout_core::anim::AnimValue)>)]) {
    let cx = parse_cx();
    let mut specs: Vec<(taffy::NodeId, Rc<SpecifiedStyle>)> = Vec::new();
    let mut relayout = false;
    for (key, props) in values {
        let mut s = SpecifiedStyle::default();
        for (prop, v) in props {
            layout_core::anim::apply_value(prop, v, &mut s, &cx);
            relayout |= !is_paint_only(prop);
        }
        let s = Rc::new(s);
        for (tid, n) in &tree.node_map {
            if n.id() == *key && n.is_element() {
                specs.push((*tid, s.clone()));
            }
        }
    }
    if specs.is_empty() {
        return;
    }
    layout_core::apply_specs(tree, &specs);
    if relayout {
        crate::layout::remeasure(tree);
    }
}

/// What a frame needs from the engine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Frame {
    /// Nothing is animating.
    Idle,
    /// Values moved: repaint.
    Repaint,
    /// A value stopped being animated: relayout from the cascade.
    Relayout,
}

/// One animation frame at the frame clock.
pub(crate) fn frame(tree: &mut LayoutTree) -> Frame {
    let active = TIMELINE.with(|t| t.borrow().active());
    let had = APPLIED.with(|a| !a.borrow().is_empty());
    let poked = POKED.with(|p| p.replace(false));
    if !active && !had && !poked {
        return Frame::Idle;
    }
    let values = sample(now_ms());
    drain_events();
    let now: HashSet<(NodeId, String)> = values.iter().flat_map(|(k, v)| v.iter().map(move |(p, _)| (*k, p.clone()))).collect();
    let stopped = APPLIED.with(|a| a.borrow().iter().any(|e| !now.contains(e)));
    APPLIED.with(|a| *a.borrow_mut() = now);
    if stopped {
        return Frame::Relayout;
    }
    if values.is_empty() {
        return Frame::Idle;
    }
    apply(tree, &values);
    Frame::Repaint
}

/// The events for the engine to dispatch as plain Events — none since AETHERSTYLE3B (SR115): the script lane
/// dispatches them as TransitionEvent / AnimationEvent ([`take_timed_events`]).
pub(crate) fn take_events() -> Vec<AnimEvent<NodeId>> {
    Vec::new()
}
