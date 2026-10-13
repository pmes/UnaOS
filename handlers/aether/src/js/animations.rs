//! AETHERSTYLE3B (LEDGER SR115): the Web Animations API (Web Animations 1, css-animations-2 / css-transitions-2
//! §6) and the TransitionEvent / AnimationEvent fields, over the page's one timeline (`crate::css::anim`, the
//! `layout_core::anim::Timeline` CSS transitions and animations already run on).
//!
//! CHARTER: Aether — binding.
//!
//! * `element.animate(keyframes, options)`, `new KeyframeEffect(target, keyframes, options)`,
//!   `new Animation(effect)`: the keyframes (array or property-indexed form, computed offsets per §5.3.3) and the
//!   `EffectTiming` go to `Timeline::animate` through the native `__anim(op, element, json)`; the Animation's
//!   `play` / `pause` / `finish` / `cancel` / `reverse` / `updatePlaybackRate`, `currentTime`, `startTime`,
//!   `playbackRate`, `playState` and the effect's `getKeyframes` / `setKeyframes` / `getTiming` /
//!   `getComputedTiming` / `updateTiming` are the timeline's model (`Timeline::info` and its verbs).
//! * `element.getAnimations()` / `document.getAnimations()`: CSS transitions (CSSTransition), CSS animations
//!   (CSSAnimation) and script animations in composite order.
//! * [`poll`] runs once per event-loop turn: the CSS events the frames queued are dispatched as trusted
//!   TransitionEvent / AnimationEvent with `propertyName` / `animationName`, `elapsedTime` and `pseudoElement`;
//!   the script animations that finished resolve `finished` and fire `finish` (an AnimationPlaybackEvent).
//!
//! Not modelled: `composite` other than `replace`, `iterationStart`, `commitStyles()` (a no-op),
//! `ScrollTimeline` / `ViewTimeline`, the pending (play-pending) state — `ready` is resolved at once.

use super::events;
use super::idl::{arg, s};
use super::TargetKey;
use html_core::NodeId;
use js_core::vm::*;
use layout_core::anim::{AnimKind, Easing, KeyframeSet, ScriptTiming, StepPos};
use serde_json::{json, Value as J};

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = arg(vm, ctx, i);
    Ok(vm.to_string(&v)?.to_rust())
}

fn fmt_num(v: f32) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// An easing as its CSS text (the named curves by name).
fn easing_text(e: &Easing) -> String {
    match e {
        Easing::Linear => "linear".into(),
        Easing::CubicBezier(a, b, c, d) => {
            let named = [("ease", [0.25, 0.1, 0.25, 1.0]), ("ease-in", [0.42, 0.0, 1.0, 1.0]), ("ease-out", [0.0, 0.0, 0.58, 1.0]), ("ease-in-out", [0.42, 0.0, 0.58, 1.0])];
            let v = [*a, *b, *c, *d];
            match named.iter().find(|(_, p)| p.iter().zip(v.iter()).all(|(x, y)| (x - y).abs() < 1e-6)) {
                Some((n, _)) => (*n).into(),
                None => format!("cubic-bezier({}, {}, {}, {})", fmt_num(*a), fmt_num(*b), fmt_num(*c), fmt_num(*d)),
            }
        }
        Easing::Steps(n, pos) => {
            let p = match pos {
                StepPos::JumpStart => "jump-start",
                StepPos::JumpEnd => "jump-end",
                StepPos::JumpNone => "jump-none",
                StepPos::JumpBoth => "jump-both",
            };
            format!("steps({n}, {p})")
        }
        Easing::Points(pts) => {
            let parts: Vec<String> = pts.iter().map(|(i, o)| format!("{} {}%", fmt_num(*o), fmt_num(i * 100.0))).collect();
            format!("linear({})", parts.join(", "))
        }
    }
}

const DIRECTIONS: [&str; 4] = ["normal", "reverse", "alternate", "alternate-reverse"];
const FILLS: [&str; 4] = ["none", "forwards", "backwards", "both"];

/// `EffectTiming` from the prelude's normalized JSON (iterations -1 = Infinity). Err = the TypeError text.
fn timing_of(t: &J) -> Result<ScriptTiming, String> {
    let n = |k: &str, d: f64| t.get(k).and_then(J::as_f64).unwrap_or(d);
    let it = n("iterations", 1.0);
    let easing_s = t.get("easing").and_then(J::as_str).unwrap_or("linear");
    let easing = Easing::parse(easing_s).ok_or_else(|| format!("'{easing_s}' is not a valid value for easing"))?;
    let direction = t.get("direction").and_then(J::as_str).unwrap_or("normal");
    let fill = t.get("fill").and_then(J::as_str).unwrap_or("auto");
    Ok(ScriptTiming {
        duration: n("duration", 0.0).max(0.0),
        delay: n("delay", 0.0),
        end_delay: n("endDelay", 0.0),
        iterations: if it < 0.0 { f64::INFINITY } else { it },
        direction: DIRECTIONS.iter().position(|d| *d == direction).ok_or_else(|| format!("'{direction}' is not a valid value for direction"))? as u8,
        fill: if fill == "auto" { 0 } else { FILLS.iter().position(|f| *f == fill).ok_or_else(|| format!("'{fill}' is not a valid value for fill"))? as u8 },
        easing,
    })
}

fn timing_json(t: &ScriptTiming) -> J {
    json!({
        "duration": t.duration,
        "delay": t.delay,
        "endDelay": t.end_delay,
        "iterations": if t.iterations.is_infinite() { -1.0 } else { t.iterations },
        "direction": DIRECTIONS[(t.direction & 3) as usize],
        "fill": FILLS[(t.fill & 3) as usize],
        "easing": easing_text(&t.easing),
    })
}

/// The prelude's keyframes `[{offset|null, easing|null, decls: [[prop, value]]}]` as a `KeyframeSet`, with the
/// missing offsets computed (Web Animations 1 §5.3.3: first 0, last 1, the rest evenly between known ones).
fn keyframes_of(v: &J) -> Result<KeyframeSet, String> {
    let list = v.as_array().cloned().unwrap_or_default();
    let mut offs: Vec<Option<f64>> = list.iter().map(|f| f.get("offset").and_then(J::as_f64)).collect();
    let n = offs.len();
    if n == 1 && offs[0].is_none() {
        offs[0] = Some(1.0);
    } else if n > 1 {
        if offs[0].is_none() {
            offs[0] = Some(0.0);
        }
        if offs[n - 1].is_none() {
            offs[n - 1] = Some(1.0);
        }
    }
    let mut i = 0;
    while i < n {
        if offs[i].is_some() {
            i += 1;
            continue;
        }
        let a = i - 1;
        let mut b = i;
        while offs[b].is_none() {
            b += 1;
        }
        let (oa, ob) = (offs[a].unwrap_or(0.0), offs[b].unwrap_or(1.0));
        for (k, slot) in offs.iter_mut().enumerate().take(b).skip(a + 1) {
            *slot = Some(oa + (ob - oa) * (k - a) as f64 / (b - a) as f64);
        }
        i = b;
    }
    let mut frames = Vec::with_capacity(n);
    for (f, o) in list.iter().zip(offs) {
        let easing = match f.get("easing").and_then(J::as_str) {
            Some(e) => Some(Easing::parse(e).ok_or_else(|| format!("'{e}' is not a valid value for easing"))?),
            None => None,
        };
        let decls: Vec<(String, String)> = f
            .get("decls")
            .and_then(J::as_array)
            .map(|d| d.iter().filter_map(|p| Some((p.get(0)?.as_str()?.to_string(), p.get(1)?.as_str()?.to_string()))).collect())
            .unwrap_or_default();
        frames.push((o.unwrap_or(0.0) as f32, easing, decls));
    }
    frames.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(KeyframeSet { frames })
}

fn keyframes_json(k: &KeyframeSet) -> J {
    J::Array(
        k.frames
            .iter()
            .map(|(o, e, d)| {
                json!({
                    "offset": o,
                    "easing": e.as_ref().map(easing_text).unwrap_or_else(|| "linear".into()),
                    "decls": d.iter().map(|(p, v)| json!([p, v])).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

/// `__anim(op, element|null, json)` → JSON text (`target`: the element itself).
fn anim_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    use crate::css::anim::{now_ms, poke, with_timeline};
    let op = str_arg(vm, ctx, 0)?;
    let el = super::dom::node_of(vm, &arg(vm, ctx, 1));
    let a: J = serde_json::from_str(&str_arg(vm, ctx, 2)?).unwrap_or(J::Null);
    let id = a.get("id").and_then(J::as_u64).unwrap_or(0);
    let now = now_ms();
    let out = match op.as_str() {
        "now" => json!(now),
        "animate" => {
            let Some(el) = el else { return vm.throw_type("Failed to execute 'animate': the effect has no target element.") };
            let frames = match keyframes_of(a.get("keyframes").unwrap_or(&J::Null)) {
                Ok(f) => f,
                Err(m) => return vm.throw_type(&m),
            };
            let timing = match timing_of(a.get("timing").unwrap_or(&J::Null)) {
                Ok(t) => t,
                Err(m) => return vm.throw_type(&m),
            };
            let name = a.get("name").and_then(J::as_str).unwrap_or("");
            let idle = a.get("idle").and_then(J::as_bool).unwrap_or(false);
            let h = with_timeline(|t| {
                let h = t.animate(&NodeId(el), frames, timing, name, now);
                if idle {
                    t.cancel(h, now);
                }
                h
            });
            poke();
            json!({ "id": h })
        }
        "info" => match with_timeline(|t| t.info(id, now)) {
            None => J::Null,
            Some(i) => json!({
                "kind": match i.kind { AnimKind::Script => "script", AnimKind::Css => "css", AnimKind::Transition => "transition" },
                "name": i.name,
                "state": i.play_state.as_str(),
                "currentTime": i.current_time,
                "startTime": i.start_time,
                "rate": i.rate,
                "timing": timing_json(&i.timing),
                "progress": i.progress,
                "iteration": i.current_iteration,
                "target": i.key.0,
            }),
        },
        "list" => json!(with_timeline(|t| t.animation_ids(el.map(NodeId).as_ref()))),
        "target" => {
            let k = with_timeline(|t| t.info(id, now)).map(|i| i.key.0);
            return Ok(match k {
                Some(n) => super::dom::wrap(vm, n),
                None => Value::Null,
            });
        }
        "play" | "pause" | "finish" | "cancel" | "reverse" => {
            let ok = with_timeline(|t| match op.as_str() {
                "play" => t.play(id, now),
                "pause" => t.pause(id, now),
                "finish" => t.finish(id, now),
                "cancel" => t.cancel(id, now),
                _ => t.reverse(id, now),
            });
            poke();
            json!(ok)
        }
        "rate" => {
            let r = a.get("rate").and_then(J::as_f64).unwrap_or(1.0);
            let ok = with_timeline(|t| t.set_rate(id, r, now));
            poke();
            json!(ok)
        }
        "seek" => {
            let v = a.get("t").and_then(J::as_f64).unwrap_or(0.0);
            let ok = with_timeline(|t| t.set_current_time(id, v, now));
            poke();
            json!(ok)
        }
        "start" => {
            let v = a.get("t").and_then(J::as_f64).unwrap_or(now);
            let ok = with_timeline(|t| t.set_start_time(id, v));
            poke();
            json!(ok)
        }
        "frames" => with_timeline(|t| t.keyframes(id)).map(|k| keyframes_json(&k)).unwrap_or(J::Null),
        "setFrames" => {
            let frames = match keyframes_of(a.get("keyframes").unwrap_or(&J::Null)) {
                Ok(f) => f,
                Err(m) => return vm.throw_type(&m),
            };
            let ok = with_timeline(|t| t.set_keyframes(id, frames));
            poke();
            json!(ok)
        }
        "timing" => {
            let timing = match timing_of(a.get("timing").unwrap_or(&J::Null)) {
                Ok(t) => t,
                Err(m) => return vm.throw_type(&m),
            };
            let ok = with_timeline(|t| t.set_timing(id, &timing));
            poke();
            json!(ok)
        }
        "base" => {
            let p = a.get("prop").and_then(J::as_str).unwrap_or("");
            json!(el.and_then(|e| crate::css::anim::base_value(NodeId(e), p)))
        }
        _ => return vm.throw_type("__anim: unknown operation"),
    };
    Ok(s(&out.to_string()))
}

pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__anim", 3, anim_native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__anim"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("animations-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] animations prelude failed: {m}");
    }
}

/// One event-loop turn: the CSS transition / animation events as trusted TransitionEvent / AnimationEvent with
/// their fields (css-transitions-1 §6.1, css-animations-1 §4.1), then the finished script animations' `finish`.
/// Returns how many were delivered.
pub fn poll(vm: &mut Vm) -> usize {
    if super::engine_poisoned() {
        return 0;
    }
    let mut n = 0;
    for e in crate::css::anim::take_timed_events() {
        let el = e.event.key().0;
        if super::dom::kind(el) != super::dom::NK::Element {
            continue;
        }
        let init = vm.new_plain_object();
        let name_key = if e.event.interface() == "TransitionEvent" { "propertyName" } else { "animationName" };
        let _ = vm.create_data_property(init, PropertyKey::from_str(name_key), s(e.event.name()));
        let _ = vm.create_data_property(init, PropertyKey::from_str("elapsedTime"), super::idl::num(e.elapsed));
        let _ = vm.create_data_property(init, PropertyKey::from_str("pseudoElement"), s(""));
        let ev = events::new_event(vm, e.event.interface(), e.event.kind(), true, false, Some(init));
        let r = super::idl::root(vm, Value::Object(ev));
        let _ = events::dispatch(vm, ev, TargetKey::Node(el));
        super::idl::unroot(vm, r);
        super::checkpoint(vm);
        n += 1;
        if super::engine_poisoned() {
            return n;
        }
    }
    let done = crate::css::anim::with_timeline(|t| t.take_finished());
    if !done.is_empty() {
        let g = vm.realm().global;
        let f = vm.get(g, &PropertyKey::from_str("__anim_settle")).unwrap_or(Value::Undefined);
        if vm.is_callable(&f) {
            let ids = json!(done).to_string();
            super::invoke(vm, "animation-finish", &f, &Value::Undefined, &[s(&ids)]);
            super::checkpoint(vm);
            n += done.len();
        }
    }
    n
}

const PRELUDE: &str = r#"
(function () {
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var TOKEN = {};
    var call = function (op, el, args) {
        var r = __anim(op, el === undefined ? null : el, JSON.stringify(args || {}));
        return typeof r === 'string' ? JSON.parse(r) : r;
    };
    var OBJS = new Map();
    var ST = new WeakMap();
    var FX = new WeakMap();
    var SKIP = { offset: 1, easing: 1, composite: 1, computedOffset: 1 };
    var kebab = function (p) {
        if (p === 'cssFloat') { return 'float'; }
        if (p === 'cssOffset') { return 'offset'; }
        if (p.indexOf('--') === 0) { return p; }
        return p.replace(/[A-Z]/g, function (c) { return '-' + c.toLowerCase(); });
    };
    var camel = function (p) {
        if (p === 'float') { return 'cssFloat'; }
        if (p.indexOf('--') === 0) { return p; }
        return p.replace(/-([a-z])/g, function (_, c) { return c.toUpperCase(); });
    };
    var offsetOf = function (v) {
        if (v === null || v === undefined) { return null; }
        var n = Number(v);
        if (!(n >= 0 && n <= 1)) { throw new TypeError("Offsets must be null or in the range [0,1]."); }
        return n;
    };
    // Web Animations 1 §5.3.4 "process a keyframes argument" → [{offset, easing, decls}]
    var normFrames = function (kf) {
        var out = [];
        if (kf === null || kf === undefined) { return out; }
        if (typeof kf !== 'object') { throw new TypeError("Failed to convert value to 'object'."); }
        if (typeof kf[Symbol.iterator] === 'function') {
            for (var f of kf) {
                if (f === null || typeof f !== 'object') { throw new TypeError("Keyframes must be objects."); }
                var o = { offset: offsetOf(f.offset), easing: f.easing === undefined ? null : String(f.easing), decls: [] };
                for (var k in f) { if (!SKIP[k]) { o.decls.push([kebab(k), String(f[k])]); } }
                out.push(o);
            }
            var last = -1;
            out.forEach(function (o) {
                if (o.offset !== null) {
                    if (o.offset < last) { throw new TypeError("Offsets must be monotonically non-decreasing."); }
                    last = o.offset;
                }
            });
            return out;
        }
        var eas = kf.easing;
        Object.keys(kf).forEach(function (k) {
            if (SKIP[k]) { return; }
            var vals = Array.isArray(kf[k]) ? kf[k] : [kf[k]];
            var m = vals.length;
            vals.forEach(function (v, i) {
                var e = Array.isArray(eas) ? (eas.length ? eas[i % eas.length] : null) : (eas === undefined ? null : eas);
                out.push({ offset: m === 1 ? 1 : i / (m - 1), easing: e === null ? null : String(e), decls: [[kebab(k), String(v)]] });
            });
        });
        out.sort(function (a, b) { return a.offset - b.offset; });
        return out;
    };
    var normTiming = function (o) {
        var t = { duration: 0, delay: 0, endDelay: 0, iterations: 1, direction: 'normal', fill: 'auto', easing: 'linear', iterationStart: 0 };
        if (typeof o === 'number') { t.duration = o; }
        else if (o !== null && o !== undefined && typeof o === 'object') {
            ['duration', 'delay', 'endDelay', 'iterations', 'direction', 'fill', 'easing', 'iterationStart'].forEach(function (k) { if (o[k] !== undefined) { t[k] = o[k]; } });
        }
        if (t.duration === 'auto') { t.duration = 0; }
        t.duration = Number(t.duration); t.delay = Number(t.delay); t.endDelay = Number(t.endDelay); t.iterations = Number(t.iterations);
        if (!(t.duration >= 0)) { throw new TypeError("duration must be non-negative or auto"); }
        if (!(t.iterations >= 0)) { throw new TypeError("iterationCount must be non-negative"); }
        if (!isFinite(t.delay) || !isFinite(t.endDelay)) { throw new TypeError("delay must be finite"); }
        t.direction = String(t.direction); t.fill = String(t.fill); t.easing = String(t.easing);
        return t;
    };
    var wire = function (t) {
        return { duration: t.duration, delay: t.delay, endDelay: t.endDelay, iterations: t.iterations === Infinity ? -1 : t.iterations, direction: t.direction, fill: t.fill, easing: t.easing };
    };
    var unwire = function (t) {
        return { delay: t.delay, endDelay: t.endDelay, fill: t.fill, iterationStart: 0, iterations: t.iterations < 0 ? Infinity : t.iterations, duration: t.duration, direction: t.direction, easing: t.easing };
    };
    var framesOut = function (list) {
        return list.map(function (f) {
            var o = { offset: f.offset, computedOffset: f.offset, easing: f.easing || 'linear', composite: 'auto' };
            f.decls.forEach(function (d) { o[camel(d[0])] = d[1]; });
            return o;
        });
    };
    class AnimationEffect {
        constructor(tok) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } }
        getTiming() {
            var fx = FX.get(this), id = fx.anim ? ST.get(fx.anim).id : 0;
            if (id) { var i = call('info', null, { id: id }); if (i) { return unwire(i.timing); } }
            return unwire(wire(fx.timing));
        }
        getComputedTiming() {
            var fx = FX.get(this), t = this.getTiming(), id = fx.anim ? ST.get(fx.anim).id : 0;
            var i = id ? call('info', null, { id: id }) : null;
            var active = t.duration * t.iterations;
            return Object.assign(t, {
                endTime: Math.max(t.delay + active + t.endDelay, 0), activeDuration: active,
                localTime: i ? i.currentTime : null, progress: i ? i.progress : null, currentIteration: i ? i.iteration : null
            });
        }
        updateTiming(o) {
            var fx = FX.get(this), cur = this.getTiming();
            var t = normTiming(Object.assign({}, cur, o || {}));
            fx.timing = t;
            var id = fx.anim ? ST.get(fx.anim).id : 0;
            if (id) { call('timing', null, { id: id, timing: wire(t) }); }
        }
    }
    class KeyframeEffect extends AnimationEffect {
        constructor(target, keyframes, options) {
            super(TOKEN);
            if (target instanceof KeyframeEffect && keyframes === undefined) {
                var src = FX.get(target);
                FX.set(this, { target: src.target, frames: src.frames.slice(), timing: Object.assign({}, src.timing), anim: null });
                return;
            }
            if (target !== null && !(target instanceof Element)) { throw new TypeError("Failed to construct 'KeyframeEffect': parameter 1 is not of type 'Element'."); }
            FX.set(this, { target: target, frames: normFrames(keyframes), timing: normTiming(options), anim: null });
        }
        get target() { return FX.get(this).target; }
        set target(v) { FX.get(this).target = v; }
        get pseudoElement() { return null; }
        get composite() { return 'replace'; }
        getKeyframes() {
            var fx = FX.get(this), id = fx.anim ? ST.get(fx.anim).id : 0;
            if (id) { var k = call('frames', null, { id: id }); if (k) { return framesOut(k); } }
            return framesOut(fx.frames);
        }
        setKeyframes(kf) {
            var fx = FX.get(this);
            fx.frames = normFrames(kf);
            var id = fx.anim ? ST.get(fx.anim).id : 0;
            if (id) { call('setFrames', null, { id: id, keyframes: fx.frames }); }
        }
    }
    var timeline = null;
    class AnimationTimeline {
        constructor(tok) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } }
        get currentTime() { return call('now', null, {}); }
    }
    class DocumentTimeline extends AnimationTimeline {
        constructor() { super(TOKEN); }
    }
    class AnimationPlaybackEvent extends Event {
        constructor(type, init) {
            super(type, init);
            var i = init || {};
            this.__ct = i.currentTime === undefined ? null : i.currentTime;
            this.__tt = i.timelineTime === undefined ? null : i.timelineTime;
        }
        get currentTime() { return this.__ct; }
        get timelineTime() { return this.__tt; }
    }
    var fresh = function (st, anim) {
        st.finished = new Promise(function (res, rej) { st.resolve = res; st.reject = rej; });
        st.finished.catch(function () {});
        st.done = false;
    };
    var state = function (a) { return ST.get(a); };
    var ensure = function (a, idle) {
        var st = state(a);
        if (st.id) { return st.id; }
        var fx = st.effect ? FX.get(st.effect) : null;
        if (!fx || !fx.target) { return 0; }
        var r = call('animate', fx.target, { keyframes: fx.frames, timing: wire(fx.timing), name: st.name, idle: !!idle });
        st.id = r.id;
        OBJS.set(st.id, a);
        return st.id;
    };
    var info = function (a) { var id = state(a).id; return id ? call('info', null, { id: id }) : null; };
    var dispatchPlayback = function (a, type) {
        var i = info(a);
        a.dispatchEvent(new AnimationPlaybackEvent(type, { currentTime: i ? i.currentTime : null, timelineTime: timeline ? timeline.currentTime : null }));
    };
    class Animation extends EventTarget {
        constructor(effect, tl) {
            super();
            var st = { id: 0, effect: effect || null, name: '', handlers: {} };
            fresh(st, this);
            ST.set(this, st);
            if (effect) { FX.get(effect).anim = this; }
            var self = this;
            ['finish', 'cancel', 'remove'].forEach(function (t) {
                self.addEventListener(t, function (e) { var h = st.handlers[t]; if (typeof h === 'function') { h.call(self, e); } });
            });
        }
        get id() { return state(this).name; }
        set id(v) { state(this).name = String(v); }
        get effect() { return state(this).effect; }
        set effect(v) { state(this).effect = v; if (v) { FX.get(v).anim = this; } }
        get timeline() { return timeline; }
        get playState() { var i = info(this); return i ? i.state : 'idle'; }
        get replaceState() { return 'active'; }
        get pending() { return false; }
        get currentTime() { var i = info(this); return i ? i.currentTime : null; }
        set currentTime(v) {
            if (v === null) { return; }
            var id = ensure(this, true);
            if (id) { call('seek', null, { id: id, t: Number(v) }); }
        }
        get startTime() { var i = info(this); return i ? i.startTime : null; }
        set startTime(v) {
            if (v === null) { return; }
            var id = ensure(this, true);
            if (id) { call('start', null, { id: id, t: Number(v) }); }
        }
        get playbackRate() { var i = info(this); return i ? i.rate : 1; }
        set playbackRate(v) { var id = ensure(this, true); if (id) { call('rate', null, { id: id, rate: Number(v) }); } }
        updatePlaybackRate(v) { this.playbackRate = v; }
        get ready() { return Promise.resolve(this); }
        get finished() { return state(this).finished; }
        get onfinish() { return state(this).handlers.finish || null; }
        set onfinish(f) { state(this).handlers.finish = f; }
        get oncancel() { return state(this).handlers.cancel || null; }
        set oncancel(f) { state(this).handlers.cancel = f; }
        get onremove() { return state(this).handlers.remove || null; }
        set onremove(f) { state(this).handlers.remove = f; }
        play() {
            var st = state(this);
            if (st.done) { fresh(st, this); }
            var id = st.id;
            if (!id) { ensure(this, false); return; }
            if (call('play', null, { id: id }) !== true) {
                throw new DOMException("Failed to execute 'play' on 'Animation': Cannot play reversed Animation with infinite target effect end.", 'InvalidStateError');
            }
        }
        pause() {
            var id = ensure(this, true);
            if (id && call('pause', null, { id: id }) !== true) {
                throw new DOMException("Failed to execute 'pause' on 'Animation': Cannot pause, Animation has infinite target effect end.", 'InvalidStateError');
            }
        }
        finish() {
            var id = ensure(this, true);
            if (!id || call('finish', null, { id: id }) !== true) {
                throw new DOMException("Failed to execute 'finish' on 'Animation': Cannot finish Animation with a playbackRate of 0 or an infinite target effect end.", 'InvalidStateError');
            }
        }
        cancel() {
            var st = state(this);
            if (!st.id) { return; }
            var was = this.playState;
            call('cancel', null, { id: st.id });
            if (was !== 'idle') {
                if (!st.done) {
                    st.reject(new DOMException('The user aborted a request.', 'AbortError'));
                    st.done = true;
                }
                dispatchPlayback(this, 'cancel');
            }
        }
        reverse() {
            var id = ensure(this, true);
            if (id) { var st = state(this); if (st.done) { fresh(st, this); } call('reverse', null, { id: id }); }
        }
        persist() {}
        commitStyles() {}
    }
    class CSSAnimation extends Animation {
        get animationName() { return state(this).cssName; }
    }
    class CSSTransition extends Animation {
        get transitionProperty() { return state(this).cssName; }
    }
    var wrapCss = function (id) {
        var a = OBJS.get(id);
        if (a) { return a; }
        var i = call('info', null, { id: id });
        if (!i) { return null; }
        var target = __anim('target', null, JSON.stringify({ id: id }));
        var fx = new KeyframeEffect(target, null, unwire(i.timing));
        var C = i.kind === 'transition' ? CSSTransition : (i.kind === 'css' ? CSSAnimation : Animation);
        a = new C(fx, timeline);
        var st = state(a);
        st.id = id;
        st.cssName = i.name;
        OBJS.set(id, a);
        return a;
    };
    var list = function (el) {
        return call('list', el, {}).map(wrapCss).filter(function (a) { return a !== null; });
    };
    timeline = new DocumentTimeline();
    hide('__anim_settle', function (json) {
        JSON.parse(json).forEach(function (id) {
            var a = OBJS.get(id);
            if (!a) { return; }
            var st = state(a);
            if (!st.done) { st.done = true; st.resolve(a); }
            dispatchPlayback(a, 'finish');
        });
    });
    [AnimationEffect, KeyframeEffect, AnimationTimeline, DocumentTimeline, AnimationPlaybackEvent, Animation, CSSAnimation, CSSTransition].forEach(function (C) {
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
        hide(C.name, C);
    });
    var EP = typeof Element === 'function' && Element.prototype;
    if (EP) {
        Object.defineProperty(EP, 'animate', { value: function animate(keyframes, options) {
            var fx = new KeyframeEffect(this, keyframes, options);
            var a = new Animation(fx, timeline);
            if (options && typeof options === 'object' && options.id !== undefined) { a.id = options.id; }
            a.play();
            return a;
        }, writable: true, configurable: true, enumerable: true });
        Object.defineProperty(EP, 'getAnimations', { value: function getAnimations() { return list(this); }, writable: true, configurable: true, enumerable: true });
    }
    var DP = typeof Document === 'function' && Document.prototype;
    if (DP) {
        Object.defineProperty(DP, 'getAnimations', { value: function getAnimations() { return list(null); }, writable: true, configurable: true, enumerable: true });
        Object.defineProperty(DP, 'timeline', { get: function () { return timeline; }, configurable: true, enumerable: true });
    }
})();
"#;
