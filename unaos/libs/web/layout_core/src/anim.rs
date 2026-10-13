//! AETHERSTYLE3 (SR103): CSS transitions (css-transitions-1) and keyframe animations (css-animations-1) as
//! ONE shared timing model both rings drive per frame.
//!
//! * The declarations: `transition` / `transition-property|duration|timing-function|delay|behavior` and
//!   `animation` / `animation-name|duration|timing-function|delay|iteration-count|direction|fill-mode|
//!   play-state` are kept as lists ([`TransitionLists`], [`AnimationLists`]) and resolved to one spec per
//!   property / name (§2: the property or name list governs, the other lists repeat).
//! * The timing functions (css-easing-1/-2): `linear`, `ease*`, `cubic-bezier()`, `steps()` / `step-*`,
//!   `linear()` with its points.
//! * The computed-value interpolation (css-values-4 §3, css-color-4 §13, css-transforms-1 §9-§13): numbers,
//!   lengths, percentages, angles, lists of them, colours (premultiplied sRGB), transform lists — function by
//!   function when they match, else through the decomposed 2D matrices — and discrete values (they flip at
//!   50%, `visibility` stays visible while either end is).
//! * The [`Timeline`]: running transitions (a style change of a transitionable property starts one from the
//!   before-change value, a retarget starts from the current value, an exact reversal is shortened, §3) and
//!   running animations (keyframes sampled per property with implicit 0% / 100% from the base value, iteration,
//!   direction, fill, pause), sampled at the frame time the shell passes, with the events (`transitionstart`,
//!   `transitionend`, `animationstart`, `animationiteration`, `animationend`) the host dispatches.
//!
//! An animated value is applied by re-applying the property onto the element's cascaded style
//! ([`apply_value`]: the CSS text through `css::apply_declaration`, a transform list directly) — animations
//! below transitions (the cascade's Animation origin under its Transition origin, css-cascade-5 §6.1).

use crate::css::{self, ParseCx, SpecifiedStyle};
use crate::paint::{mat_mul, sin_cos, sqrt, TransformOp, IDENTITY};
use crate::prelude::*;
use crate::Host;

// ---- timing functions --------------------------------------------------------------------------------------

/// `jump-*` position of `steps()` (css-easing-1 §2.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StepPos {
    JumpStart,
    JumpEnd,
    JumpNone,
    JumpBoth,
}

/// An easing function.
#[derive(Clone, Debug, PartialEq)]
pub enum Easing {
    Linear,
    CubicBezier(f32, f32, f32, f32),
    Steps(u32, StepPos),
    /// `linear()` control points: (input, output), inputs ascending.
    Points(Vec<(f32, f32)>),
}

impl Default for Easing {
    /// `ease`, the initial value of both timing-function properties.
    fn default() -> Self {
        Easing::CubicBezier(0.25, 0.1, 0.25, 1.0)
    }
}

fn split_commas(v: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in v.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(v[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(v[start..].trim());
    out
}

/// Whitespace-separated tokens, a function call with its arguments as one token.
fn tokens(v: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start: Option<usize> = None;
    for (i, c) in v.char_indices() {
        match c {
            '(' => {
                depth += 1;
                start.get_or_insert(i);
            }
            ')' => depth -= 1,
            c if c.is_whitespace() && depth == 0 => {
                if let Some(s) = start.take() {
                    out.push(&v[s..i]);
                }
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        out.push(&v[s..]);
    }
    out
}

fn num(v: &str) -> Option<f32> {
    v.trim().parse::<f32>().ok().filter(|x| x.is_finite())
}

impl Easing {
    /// Parses one `<easing-function>`.
    pub fn parse(v: &str) -> Option<Easing> {
        let v = v.trim();
        Some(match v.to_ascii_lowercase().as_str() {
            "linear" => Easing::Linear,
            "ease" => Easing::default(),
            "ease-in" => Easing::CubicBezier(0.42, 0.0, 1.0, 1.0),
            "ease-out" => Easing::CubicBezier(0.0, 0.0, 0.58, 1.0),
            "ease-in-out" => Easing::CubicBezier(0.42, 0.0, 0.58, 1.0),
            "step-start" => Easing::Steps(1, StepPos::JumpStart),
            "step-end" => Easing::Steps(1, StepPos::JumpEnd),
            l => {
                let open = l.find('(')?;
                let name = &l[..open];
                let args = l[open + 1..].strip_suffix(')')?;
                let parts = split_commas(args);
                match name {
                    "cubic-bezier" => {
                        let [a, b, c, d] = parts.as_slice() else { return None };
                        let (x1, y1, x2, y2) = (num(a)?, num(b)?, num(c)?, num(d)?);
                        if !(0.0..=1.0).contains(&x1) || !(0.0..=1.0).contains(&x2) {
                            return None;
                        }
                        Easing::CubicBezier(x1, y1, x2, y2)
                    }
                    "steps" => {
                        let n = parts.first()?.trim().parse::<u32>().ok()?;
                        let pos = match parts.get(1).map(|s| s.trim()) {
                            None | Some("end") | Some("jump-end") => StepPos::JumpEnd,
                            Some("start") | Some("jump-start") => StepPos::JumpStart,
                            Some("jump-none") => StepPos::JumpNone,
                            Some("jump-both") => StepPos::JumpBoth,
                            _ => return None,
                        };
                        if n == 0 || (pos == StepPos::JumpNone && n < 2) {
                            return None;
                        }
                        Easing::Steps(n, pos)
                    }
                    "linear" => {
                        // css-easing-2 §3.1: `<number> <percentage>{0,2}` per point; missing inputs spread
                        let mut pts: Vec<(Option<f32>, f32)> = Vec::new();
                        for p in parts {
                            let t: Vec<&str> = p.split_whitespace().collect();
                            let out = num(t.first()?)?;
                            let pct = |s: &str| s.strip_suffix('%').and_then(num).map(|x| x / 100.0);
                            match t.len() {
                                1 => pts.push((None, out)),
                                2 => pts.push((Some(pct(t[1])?), out)),
                                3 => {
                                    pts.push((Some(pct(t[1])?), out));
                                    pts.push((Some(pct(t[2])?), out));
                                }
                                _ => return None,
                            }
                        }
                        if pts.len() < 2 {
                            return None;
                        }
                        let n = pts.len();
                        if pts[0].0.is_none() {
                            pts[0].0 = Some(0.0);
                        }
                        if pts[n - 1].0.is_none() {
                            pts[n - 1].0 = Some(1.0);
                        }
                        // clamp each input to at least the largest before it
                        let mut hi = f32::MIN;
                        for p in pts.iter_mut() {
                            if let Some(x) = p.0.as_mut() {
                                if *x < hi {
                                    *x = hi;
                                }
                                hi = *x;
                            }
                        }
                        let mut i = 0;
                        while i < n {
                            if pts[i].0.is_none() {
                                let a = i - 1;
                                let mut b = i;
                                while pts[b].0.is_none() {
                                    b += 1;
                                }
                                let (xa, xb) = (pts[a].0.unwrap(), pts[b].0.unwrap());
                                for (k, j) in (a + 1..b).enumerate() {
                                    pts[j].0 = Some(xa + (xb - xa) * (k + 1) as f32 / (b - a) as f32);
                                }
                                i = b;
                            }
                            i += 1;
                        }
                        Easing::Points(pts.into_iter().map(|(x, y)| (x.unwrap_or(0.0), y)).collect())
                    }
                    _ => return None,
                }
            }
        })
    }

    /// The output progress for input progress `t` (`before`: the before flag of css-easing-1 §2.3).
    pub fn apply(&self, t: f32, before: bool) -> f32 {
        match self {
            Easing::Linear => t,
            Easing::CubicBezier(x1, y1, x2, y2) => bezier(*x1, *y1, *x2, *y2, t),
            Easing::Steps(n, pos) => {
                let n = *n as f32;
                let mut step = (t * n).floor();
                if matches!(pos, StepPos::JumpStart | StepPos::JumpBoth) {
                    step += 1.0;
                }
                if before && (t * n) - (t * n).floor() == 0.0 {
                    step -= 1.0;
                }
                if t >= 0.0 && step < 0.0 {
                    step = 0.0;
                }
                let jumps = match pos {
                    StepPos::JumpBoth => n + 1.0,
                    StepPos::JumpNone => n - 1.0,
                    _ => n,
                };
                if t <= 1.0 && step > jumps {
                    step = jumps;
                }
                step / jumps
            }
            Easing::Points(p) => {
                if p.is_empty() {
                    return t;
                }
                // the last segment whose start input is <= t (extrapolating at the ends)
                let mut i = 0;
                while i + 2 < p.len() && p[i + 1].0 <= t {
                    i += 1;
                }
                let (a, b) = (p[i], p[(i + 1).min(p.len() - 1)]);
                if (b.0 - a.0).abs() < 1e-9 {
                    return if t < a.0 { a.1 } else { b.1 };
                }
                a.1 + (b.1 - a.1) * (t - a.0) / (b.0 - a.0)
            }
        }
    }
}

/// y of the cubic Bézier (0,0) (x1,y1) (x2,y2) (1,1) at the x = `t` (Newton, then bisection).
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    if t <= 0.0 {
        // css-easing-1 §2.2: extrapolate along the start tangent
        return if x1 > 0.0 { y1 / x1 * t } else if y1 == 0.0 && x2 > 0.0 { y2 / x2 * t } else { 0.0 };
    }
    if t >= 1.0 {
        return if x2 < 1.0 { 1.0 + (y2 - 1.0) / (x2 - 1.0) * (t - 1.0) } else { 1.0 };
    }
    let (x1, y1, x2, y2, t) = (x1 as f64, y1 as f64, x2 as f64, y2 as f64, t as f64);
    let cx = 3.0 * x1;
    let bx = 3.0 * (x2 - x1) - cx;
    let ax = 1.0 - cx - bx;
    let cy = 3.0 * y1;
    let by = 3.0 * (y2 - y1) - cy;
    let ay = 1.0 - cy - by;
    let x_at = |s: f64| ((ax * s + bx) * s + cx) * s;
    let dx_at = |s: f64| (3.0 * ax * s + 2.0 * bx) * s + cx;
    let mut s = t;
    let mut ok = false;
    for _ in 0..8 {
        let e = x_at(s) - t;
        if e.abs() < 1e-7 {
            ok = true;
            break;
        }
        let d = dx_at(s);
        if d.abs() < 1e-6 {
            break;
        }
        s -= e / d;
    }
    if !ok {
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        s = t;
        for _ in 0..40 {
            let x = x_at(s);
            if (x - t).abs() < 1e-7 {
                break;
            }
            if x < t {
                lo = s;
            } else {
                hi = s;
            }
            s = (lo + hi) / 2.0;
        }
    }
    (((ay * s + by) * s + cy) * s) as f32
}

/// A `<time>` in milliseconds.
pub fn parse_time(v: &str) -> Option<f32> {
    let v = v.trim().to_ascii_lowercase();
    if let Some(ms) = v.strip_suffix("ms") {
        return num(ms);
    }
    if let Some(s) = v.strip_suffix('s') {
        return num(s).map(|s| s * 1000.0);
    }
    None
}

// ---- the declaration lists ----------------------------------------------------------------------------------

/// The `transition-*` longhands as specified (None = not specified here; the initial values apply).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransitionLists {
    pub property: Option<Vec<String>>,
    pub duration: Option<Vec<f32>>,
    pub timing: Option<Vec<Easing>>,
    pub delay: Option<Vec<f32>>,
    pub discrete: Option<Vec<bool>>,
}

/// One resolved transition (css-transitions-1 §2).
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionSpec {
    /// A property name, or `all`.
    pub property: String,
    pub duration: f32,
    pub delay: f32,
    pub easing: Easing,
    /// `transition-behavior: allow-discrete`.
    pub discrete: bool,
}

fn cycle<T: Clone>(v: &Option<Vec<T>>, i: usize, d: T) -> T {
    match v {
        Some(l) if !l.is_empty() => l[i % l.len()].clone(),
        _ => d,
    }
}

impl TransitionLists {
    /// Overlays `src`'s specified lists.
    pub fn merge(&mut self, src: &TransitionLists) {
        if src.property.is_some() { self.property = src.property.clone(); }
        if src.duration.is_some() { self.duration = src.duration.clone(); }
        if src.timing.is_some() { self.timing = src.timing.clone(); }
        if src.delay.is_some() { self.delay = src.delay.clone(); }
        if src.discrete.is_some() { self.discrete = src.discrete.clone(); }
    }

    /// The transitions in force: one per listed property (initial `all`), the other lists repeating.
    pub fn resolve(&self) -> Vec<TransitionSpec> {
        let props = self.property.clone().unwrap_or_else(|| vec![String::from("all")]);
        let mut out = Vec::new();
        for (i, p) in props.iter().enumerate() {
            if p == "none" {
                return Vec::new();
            }
            let spec = TransitionSpec {
                property: p.clone(),
                duration: cycle(&self.duration, i, 0.0f32).max(0.0),
                delay: cycle(&self.delay, i, 0.0),
                easing: cycle(&self.timing, i, Easing::default()),
                discrete: cycle(&self.discrete, i, false),
            };
            if spec.duration + spec.delay.max(0.0) > 0.0 || spec.duration > 0.0 {
                out.push(spec);
            }
        }
        out
    }

    /// The spec that applies to `prop` (the last match wins, §2).
    pub fn spec_for<'a>(specs: &'a [TransitionSpec], prop: &str) -> Option<&'a TransitionSpec> {
        specs.iter().rev().find(|s| s.property == "all" || s.property == prop || covers(&s.property, prop))
    }
}

/// A shorthand name covers its longhands (`margin` → `margin-top`, `border-color` → `border-top-color`).
fn covers(short: &str, long: &str) -> bool {
    if let Some(rest) = long.strip_prefix(short) {
        return rest.starts_with('-');
    }
    match short {
        "border-color" => long.starts_with("border-") && long.ends_with("-color"),
        "border-width" => long.starts_with("border-") && long.ends_with("-width"),
        "inset" => matches!(long, "top" | "right" | "bottom" | "left"),
        _ => false,
    }
}

/// The `animation-*` longhands as specified.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationLists {
    pub name: Option<Vec<String>>,
    pub duration: Option<Vec<f32>>,
    pub timing: Option<Vec<Easing>>,
    pub delay: Option<Vec<f32>>,
    /// Iteration count; `f32::INFINITY` = `infinite`.
    pub iterations: Option<Vec<f32>>,
    /// 0 normal, 1 reverse, 2 alternate, 3 alternate-reverse.
    pub direction: Option<Vec<u8>>,
    /// 0 none, 1 forwards, 2 backwards, 3 both.
    pub fill: Option<Vec<u8>>,
    /// running (false) / paused (true).
    pub paused: Option<Vec<bool>>,
}

/// One resolved animation (css-animations-1 §4).
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationSpec {
    pub name: String,
    pub duration: f32,
    pub delay: f32,
    pub easing: Easing,
    pub iterations: f32,
    pub direction: u8,
    pub fill: u8,
    pub paused: bool,
}

impl AnimationLists {
    pub fn merge(&mut self, src: &AnimationLists) {
        if src.name.is_some() { self.name = src.name.clone(); }
        if src.duration.is_some() { self.duration = src.duration.clone(); }
        if src.timing.is_some() { self.timing = src.timing.clone(); }
        if src.delay.is_some() { self.delay = src.delay.clone(); }
        if src.iterations.is_some() { self.iterations = src.iterations.clone(); }
        if src.direction.is_some() { self.direction = src.direction.clone(); }
        if src.fill.is_some() { self.fill = src.fill.clone(); }
        if src.paused.is_some() { self.paused = src.paused.clone(); }
    }

    /// One spec per listed name other than `none`.
    pub fn resolve(&self) -> Vec<AnimationSpec> {
        let Some(names) = &self.name else { return Vec::new() };
        names
            .iter()
            .enumerate()
            .filter(|(_, n)| n.as_str() != "none")
            .map(|(i, n)| AnimationSpec {
                name: n.clone(),
                duration: cycle(&self.duration, i, 0.0f32).max(0.0),
                delay: cycle(&self.delay, i, 0.0),
                easing: cycle(&self.timing, i, Easing::default()),
                iterations: cycle(&self.iterations, i, 1.0f32).max(0.0),
                direction: cycle(&self.direction, i, 0),
                fill: cycle(&self.fill, i, 0),
                paused: cycle(&self.paused, i, false),
            })
            .collect()
    }
}

fn direction_code(t: &str) -> Option<u8> {
    Some(match t {
        "normal" => 0,
        "reverse" => 1,
        "alternate" => 2,
        "alternate-reverse" => 3,
        _ => return None,
    })
}

fn fill_code(t: &str) -> Option<u8> {
    Some(match t {
        "none" => 0,
        "forwards" => 1,
        "backwards" => 2,
        "both" => 3,
        _ => return None,
    })
}

/// Applies one `transition*` / `animation*` declaration; `false` when it is not one of them or does not parse.
pub fn apply_declaration<H: Host>(prop: &str, value: &str, style: &mut SpecifiedStyle<H>) -> bool {
    let items = split_commas(value);
    let list = |f: &dyn Fn(&str) -> Option<f32>| -> Option<Vec<f32>> { items.iter().map(|i| f(i)).collect() };
    match prop {
        "transition-property" => {
            let names: Vec<String> = items.iter().map(|s| s.to_ascii_lowercase()).collect();
            if names.iter().any(|n| n.is_empty() || n.contains(' ')) {
                return false;
            }
            style.transition.property = Some(names);
        }
        "transition-duration" => match list(&parse_time) {
            Some(v) => style.transition.duration = Some(v),
            None => return false,
        },
        "transition-delay" => match list(&parse_time) {
            Some(v) => style.transition.delay = Some(v),
            None => return false,
        },
        "transition-timing-function" => match items.iter().map(|i| Easing::parse(i)).collect::<Option<Vec<_>>>() {
            Some(v) => style.transition.timing = Some(v),
            None => return false,
        },
        "transition-behavior" => {
            let v: Option<Vec<bool>> = items
                .iter()
                .map(|i| match *i {
                    "allow-discrete" => Some(true),
                    "normal" => Some(false),
                    _ => None,
                })
                .collect();
            match v {
                Some(v) => style.transition.discrete = Some(v),
                None => return false,
            }
        }
        "transition" => {
            let mut l = TransitionLists { property: Some(Vec::new()), duration: Some(Vec::new()), timing: Some(Vec::new()), delay: Some(Vec::new()), discrete: Some(Vec::new()) };
            for item in &items {
                let (mut prop_name, mut dur, mut del, mut ease, mut disc) = (None, None, None, None, false);
                for t in tokens(item) {
                    let lt = t.to_ascii_lowercase();
                    if let Some(ms) = parse_time(&lt) {
                        if dur.is_none() {
                            dur = Some(ms);
                        } else if del.is_none() {
                            del = Some(ms);
                        } else {
                            return false;
                        }
                    } else if let Some(e) = Easing::parse(&lt) {
                        if ease.is_some() {
                            return false;
                        }
                        ease = Some(e);
                    } else if lt == "allow-discrete" {
                        disc = true;
                    } else if lt == "normal" {
                    } else if prop_name.is_none() {
                        prop_name = Some(lt);
                    } else {
                        return false;
                    }
                }
                l.property.as_mut().unwrap().push(prop_name.unwrap_or_else(|| String::from("all")));
                l.duration.as_mut().unwrap().push(dur.unwrap_or(0.0));
                l.delay.as_mut().unwrap().push(del.unwrap_or(0.0));
                l.timing.as_mut().unwrap().push(ease.unwrap_or_default());
                l.discrete.as_mut().unwrap().push(disc);
            }
            style.transition = l;
        }
        "animation-name" => style.animation.name = Some(items.iter().map(|s| s.trim_matches(|c| c == '"' || c == '\'').to_string()).collect()),
        "animation-duration" => match items.iter().map(|i| if *i == "auto" { Some(0.0) } else { parse_time(i) }).collect::<Option<Vec<_>>>() {
            Some(v) => style.animation.duration = Some(v),
            None => return false,
        },
        "animation-delay" => match list(&parse_time) {
            Some(v) => style.animation.delay = Some(v),
            None => return false,
        },
        "animation-timing-function" => match items.iter().map(|i| Easing::parse(i)).collect::<Option<Vec<_>>>() {
            Some(v) => style.animation.timing = Some(v),
            None => return false,
        },
        "animation-iteration-count" => match list(&|i: &str| if i == "infinite" { Some(f32::INFINITY) } else { num(i).filter(|n| *n >= 0.0) }) {
            Some(v) => style.animation.iterations = Some(v),
            None => return false,
        },
        "animation-direction" => match items.iter().map(|i| direction_code(i)).collect::<Option<Vec<_>>>() {
            Some(v) => style.animation.direction = Some(v),
            None => return false,
        },
        "animation-fill-mode" => match items.iter().map(|i| fill_code(i)).collect::<Option<Vec<_>>>() {
            Some(v) => style.animation.fill = Some(v),
            None => return false,
        },
        "animation-play-state" => {
            let v: Option<Vec<bool>> = items
                .iter()
                .map(|i| match *i {
                    "running" => Some(false),
                    "paused" => Some(true),
                    _ => None,
                })
                .collect();
            match v {
                Some(v) => style.animation.paused = Some(v),
                None => return false,
            }
        }
        "animation" => {
            let mut l = AnimationLists {
                name: Some(Vec::new()),
                duration: Some(Vec::new()),
                timing: Some(Vec::new()),
                delay: Some(Vec::new()),
                iterations: Some(Vec::new()),
                direction: Some(Vec::new()),
                fill: Some(Vec::new()),
                paused: Some(Vec::new()),
            };
            for item in &items {
                let (mut name, mut dur, mut del, mut ease, mut it, mut dir, mut fill, mut ps) = (None, None, None, None, None, None, None, None);
                for t in tokens(item) {
                    let lt = t.to_ascii_lowercase();
                    if let Some(ms) = parse_time(&lt) {
                        if dur.is_none() {
                            dur = Some(ms);
                        } else if del.is_none() {
                            del = Some(ms);
                        } else {
                            return false;
                        }
                    } else if ease.is_none() && Easing::parse(&lt).is_some() {
                        ease = Easing::parse(&lt);
                    } else if it.is_none() && (lt == "infinite" || num(&lt).is_some_and(|n| n >= 0.0)) {
                        it = Some(if lt == "infinite" { f32::INFINITY } else { num(&lt).unwrap() });
                    } else if dir.is_none() && direction_code(&lt).is_some() && lt != "normal" {
                        dir = direction_code(&lt);
                    } else if fill.is_none() && fill_code(&lt).is_some() && lt != "none" {
                        fill = fill_code(&lt);
                    } else if ps.is_none() && matches!(lt.as_str(), "running" | "paused") {
                        ps = Some(lt == "paused");
                    } else if name.is_none() {
                        name = Some(t.trim_matches(|c| c == '"' || c == '\'').to_string());
                    } else {
                        return false;
                    }
                }
                l.name.as_mut().unwrap().push(name.unwrap_or_else(|| String::from("none")));
                l.duration.as_mut().unwrap().push(dur.unwrap_or(0.0));
                l.delay.as_mut().unwrap().push(del.unwrap_or(0.0));
                l.timing.as_mut().unwrap().push(ease.unwrap_or_default());
                l.iterations.as_mut().unwrap().push(it.unwrap_or(1.0));
                l.direction.as_mut().unwrap().push(dir.unwrap_or(0));
                l.fill.as_mut().unwrap().push(fill.unwrap_or(0));
                l.paused.as_mut().unwrap().push(ps.unwrap_or(false));
            }
            style.animation = l;
        }
        _ => return false,
    }
    true
}

// ---- animatable values --------------------------------------------------------------------------------------

/// The properties whose specified value is kept for transitions and keyframes (css-transitions-1 §2.1's
/// animatable set this engine lays out or paints).
pub fn is_animatable(prop: &str) -> bool {
    matches!(
        prop,
        "opacity" | "color" | "background-color" | "border-color" | "border-top-color" | "border-right-color"
            | "border-bottom-color" | "border-left-color" | "outline-color" | "width" | "height" | "min-width"
            | "min-height" | "max-width" | "max-height" | "top" | "right" | "bottom" | "left" | "inset" | "margin"
            | "margin-top" | "margin-right" | "margin-bottom" | "margin-left" | "padding" | "padding-top"
            | "padding-right" | "padding-bottom" | "padding-left" | "font-size" | "letter-spacing" | "word-spacing"
            | "line-height" | "transform" | "translate" | "rotate" | "scale" | "border-width" | "border-top-width"
            | "border-right-width" | "border-bottom-width" | "border-left-width" | "border-radius" | "flex-grow"
            | "flex-shrink" | "flex-basis" | "gap" | "row-gap" | "column-gap" | "z-index" | "visibility"
            | "filter" | "backdrop-filter" | "clip-path" | "background-position" | "transform-origin"
            | "column-width" | "column-count" | "text-indent" | "outline-width" | "outline-offset"
    )
}

/// Paint-only properties: animating them needs no relayout.
pub fn is_paint_only(prop: &str) -> bool {
    matches!(
        prop,
        "opacity" | "color" | "background-color" | "border-color" | "border-top-color" | "border-right-color"
            | "border-bottom-color" | "border-left-color" | "outline-color" | "transform" | "translate" | "rotate"
            | "scale" | "visibility" | "filter" | "backdrop-filter" | "clip-path" | "transform-origin" | "z-index"
            | "background-position"
    )
}

/// The value a property has when no declaration gives one, for the end of a transition or an implicit
/// keyframe (css-*: the initial values this engine can interpolate from).
pub fn initial_value(prop: &str) -> Option<&'static str> {
    Some(match prop {
        "opacity" => "1",
        "transform" | "translate" | "rotate" | "scale" | "filter" | "backdrop-filter" | "clip-path" => "none",
        "background-color" => "transparent",
        "visibility" => "visible",
        "letter-spacing" | "word-spacing" => "0px",
        "margin" | "margin-top" | "margin-right" | "margin-bottom" | "margin-left" | "padding" | "padding-top"
        | "padding-right" | "padding-bottom" | "padding-left" | "border-radius" | "text-indent" => "0px",
        "flex-grow" => "0",
        "flex-shrink" => "1",
        _ => return None,
    })
}

/// One component of a numeric list value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Num {
    /// A plain number.
    N(f32),
    /// A length in px.
    Px(f32),
    /// A percentage (0..100).
    Pct(f32),
    /// An angle in degrees.
    Deg(f32),
}

/// A computed value that interpolates.
#[derive(Clone, Debug, PartialEq)]
pub enum AnimValue {
    /// One or more numbers / lengths / percentages / angles (a shorthand's components).
    Nums(Vec<Num>),
    /// Straight RGBA, components 0..255, alpha 0..1.
    Color([f32; 4]),
    /// A transform list (`none` = empty).
    Transform(Vec<TransformOp>),
    /// Anything else: flips discretely.
    Discrete(String),
}

fn parse_num(t: &str, cx: &ParseCx) -> Option<Num> {
    let l = t.to_ascii_lowercase();
    if let Some(p) = l.strip_suffix('%') {
        return num(p).map(Num::Pct);
    }
    for (u, k) in [("deg", 1.0f32), ("grad", 0.9), ("rad", 57.29578), ("turn", 360.0)] {
        if let Some(v) = l.strip_suffix(u) {
            if let Some(v) = num(v) {
                return Some(Num::Deg(v * k));
            }
        }
    }
    if let Some(v) = num(&l) {
        return Some(Num::N(v));
    }
    css::parse_px(&l, cx).map(Num::Px)
}

/// The computed value of `prop` from its CSS text (lengths at the cascade's 16px basis).
pub fn parse_value<H: Host>(prop: &str, text: &str, cx: &ParseCx) -> AnimValue {
    let t = text.trim();
    let color_prop = prop.ends_with("color") || prop == "color";
    if color_prop {
        if t.eq_ignore_ascii_case("transparent") {
            return AnimValue::Color([0.0, 0.0, 0.0, 0.0]);
        }
        if let Some(((r, g, b), a)) = H::parse_color_alpha(t) {
            return AnimValue::Color([r as f32, g as f32, b as f32, a]);
        }
        if let Some((r, g, b)) = css::parse_color_str::<H>(t) {
            return AnimValue::Color([r as f32, g as f32, b as f32, 1.0]);
        }
        return AnimValue::Discrete(t.to_string());
    }
    if prop == "transform" {
        return match css::parse_transform_list(t, cx) {
            Some(ops) => AnimValue::Transform(ops),
            None => AnimValue::Discrete(t.to_string()),
        };
    }
    if matches!(prop, "visibility" | "filter" | "backdrop-filter" | "clip-path") {
        return AnimValue::Discrete(t.to_string());
    }
    let parts: Option<Vec<Num>> = t.split_whitespace().map(|p| parse_num(p, cx)).collect();
    match parts {
        Some(v) if !v.is_empty() => AnimValue::Nums(v),
        _ => AnimValue::Discrete(t.to_string()),
    }
}

fn lerp(a: f32, b: f32, p: f32) -> f32 {
    a + (b - a) * p
}

/// atan2 without libm: argument reduction (twice halving the angle) and the Taylor series of atan.
pub fn atan2(y: f32, x: f32) -> f32 {
    use core::f32::consts::{FRAC_PI_2, PI};
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    fn atan(z: f32) -> f32 {
        // |z| <= 1 after the caller's reduction
        let (z, k) = (z as f64, 1.0f64);
        let mut z = z;
        let mut mul = k;
        for _ in 0..2 {
            z = z / (1.0 + sqrt((1.0 + z * z) as f32) as f64);
            mul *= 2.0;
        }
        let z2 = z * z;
        let mut term = z;
        let mut sum = 0.0;
        for n in 0..12 {
            sum += term / (2 * n + 1) as f64;
            term *= -z2;
        }
        (sum * mul) as f32
    }
    let base = if x.abs() >= y.abs() { atan(y / x) } else { FRAC_PI_2 - atan(x / y) };
    let a = if x.abs() >= y.abs() {
        if x < 0.0 {
            if y >= 0.0 { base + PI } else { base - PI }
        } else {
            base
        }
    } else if y > 0.0 {
        base
    } else {
        base - PI
    };
    if a > PI { a - 2.0 * PI } else if a < -PI { a + 2.0 * PI } else { a }
}

/// A 2D matrix decomposed (css-transforms-1 §12.2's unmatrix, in this crate's column convention): translate,
/// scale, rotation angle (radians) and the residual 2x2 shear.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Decomposed {
    t: (f32, f32),
    s: (f32, f32),
    angle: f32,
    k: [f32; 4],
}

fn decompose(m: [f32; 6]) -> Decomposed {
    let (mut c0, mut c1) = ((m[0], m[1]), (m[2], m[3]));
    let mut s = (sqrt(c0.0 * c0.0 + c0.1 * c0.1), sqrt(c1.0 * c1.0 + c1.1 * c1.1));
    let det = m[0] * m[3] - m[1] * m[2];
    if det < 0.0 {
        if m[0] < m[3] {
            s.0 = -s.0;
        } else {
            s.1 = -s.1;
        }
    }
    if s.0 != 0.0 {
        c0 = (c0.0 / s.0, c0.1 / s.0);
    }
    if s.1 != 0.0 {
        c1 = (c1.0 / s.1, c1.1 / s.1);
    }
    let angle = atan2(c0.1, c0.0);
    let (sn, cs) = sin_cos(-angle);
    // K = R(-angle) · [c0 c1]
    let k = [cs * c0.0 - sn * c0.1, cs * c1.0 - sn * c1.1, sn * c0.0 + cs * c0.1, sn * c1.0 + cs * c1.1];
    Decomposed { t: (m[4], m[5]), s, angle, k }
}

fn recompose(d: Decomposed) -> [f32; 6] {
    let (sn, cs) = sin_cos(d.angle);
    // [c0 c1] = R(angle) · K, then the columns scale
    let c0 = (cs * d.k[0] - sn * d.k[2], sn * d.k[0] + cs * d.k[2]);
    let c1 = (cs * d.k[1] - sn * d.k[3], sn * d.k[1] + cs * d.k[3]);
    [c0.0 * d.s.0, c0.1 * d.s.0, c1.0 * d.s.1, c1.1 * d.s.1, d.t.0, d.t.1]
}

/// css-transforms-1 §13.1: interpolates two matrices through their decompositions.
pub fn interpolate_matrix(a: [f32; 6], b: [f32; 6], p: f32) -> [f32; 6] {
    use core::f32::consts::PI;
    let (mut da, db) = (decompose(a), decompose(b));
    if (da.s.0 < 0.0 && db.s.1 < 0.0) || (da.s.1 < 0.0 && db.s.0 < 0.0) {
        da.s = (-da.s.0, -da.s.1);
        da.angle += if da.angle < 0.0 { PI } else { -PI };
    }
    let mut aa = if da.angle == 0.0 { 2.0 * PI } else { da.angle };
    let mut ab = if db.angle == 0.0 { 2.0 * PI } else { db.angle };
    if (aa - ab).abs() > PI {
        if aa > ab {
            aa -= 2.0 * PI;
        } else {
            ab -= 2.0 * PI;
        }
    }
    let d = Decomposed {
        t: (lerp(da.t.0, db.t.0, p), lerp(da.t.1, db.t.1, p)),
        s: (lerp(da.s.0, db.s.0, p), lerp(da.s.1, db.s.1, p)),
        angle: lerp(aa, ab, p),
        k: [lerp(da.k[0], db.k[0], p), lerp(da.k[1], db.k[1], p), lerp(da.k[2], db.k[2], p), lerp(da.k[3], db.k[3], p)],
    };
    recompose(d)
}

fn has_pct(ops: &[TransformOp]) -> bool {
    ops.iter().any(|o| matches!(o, TransformOp::Translate(x, y) if x[1] != 0.0 || y[1] != 0.0))
}

fn interpolate_transform(a: &[TransformOp], b: &[TransformOp], p: f32) -> Option<Vec<TransformOp>> {
    // `none` against a list: the list's identity functions (css-transforms-1 §9)
    let ident = |o: &TransformOp| match o {
        TransformOp::Translate(..) => TransformOp::Translate([0.0, 0.0], [0.0, 0.0]),
        TransformOp::Matrix(_) => TransformOp::Matrix(IDENTITY),
    };
    let (a, b): (Vec<TransformOp>, Vec<TransformOp>) = match (a.is_empty(), b.is_empty()) {
        (true, false) => (b.iter().map(ident).collect(), b.to_vec()),
        (false, true) => (a.to_vec(), a.iter().map(ident).collect()),
        _ => (a.to_vec(), b.to_vec()),
    };
    let same = a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(x, y)| core::mem::discriminant(x) == core::mem::discriminant(y));
    if same {
        return Some(
            a.iter()
                .zip(b.iter())
                .map(|(x, y)| match (x, y) {
                    (TransformOp::Translate(ax, ay), TransformOp::Translate(bx, by)) => TransformOp::Translate(
                        [lerp(ax[0], bx[0], p), lerp(ax[1], bx[1], p)],
                        [lerp(ay[0], by[0], p), lerp(ay[1], by[1], p)],
                    ),
                    (TransformOp::Matrix(m), TransformOp::Matrix(n)) => TransformOp::Matrix(interpolate_matrix(*m, *n, p)),
                    _ => *x,
                })
                .collect(),
        );
    }
    // mismatched lists: the whole matrices (percentages cannot be resolved without the box)
    if has_pct(&a) || has_pct(&b) {
        return None;
    }
    let full = |l: &[TransformOp]| l.iter().fold(IDENTITY, |m, o| mat_mul(m, o.matrix(0.0, 0.0)));
    Some(vec![TransformOp::Matrix(interpolate_matrix(full(&a), full(&b), p))])
}

/// The value at progress `p` between `a` and `b` (`discrete`: transition-behavior allow-discrete). None when
/// the pair does not interpolate and is not allowed to flip.
pub fn interpolate(prop: &str, a: &AnimValue, b: &AnimValue, p: f32) -> AnimValue {
    let flip = || if p < 0.5 { a.clone() } else { b.clone() };
    match (a, b) {
        (AnimValue::Nums(x), AnimValue::Nums(y)) if x.len() == y.len() => {
            let mut out = Vec::with_capacity(x.len());
            for (u, v) in x.iter().zip(y.iter()) {
                out.push(match (u, v) {
                    (Num::N(u), Num::N(v)) => {
                        let r = lerp(*u, *v, p);
                        // integer-valued properties round (css-values-4 §3: <integer>)
                        Num::N(if matches!(prop, "z-index" | "column-count") { r.round() } else { r })
                    }
                    (Num::Px(u), Num::Px(v)) => Num::Px(lerp(*u, *v, p)),
                    (Num::Pct(u), Num::Pct(v)) => Num::Pct(lerp(*u, *v, p)),
                    (Num::Deg(u), Num::Deg(v)) => Num::Deg(lerp(*u, *v, p)),
                    // 0 against a length: a unitless zero is a length here
                    (Num::N(z), Num::Px(v)) if *z == 0.0 => Num::Px(lerp(0.0, *v, p)),
                    (Num::Px(u), Num::N(z)) if *z == 0.0 => Num::Px(lerp(*u, 0.0, p)),
                    _ => return flip(),
                });
            }
            AnimValue::Nums(out)
        }
        (AnimValue::Color(x), AnimValue::Color(y)) => {
            // premultiplied sRGB (css-color-4 §13.3)
            let a2 = lerp(x[3], y[3], p);
            let c = |i: usize| if a2 > 0.0 { lerp(x[i] * x[3], y[i] * y[3], p) / a2 } else { 0.0 };
            AnimValue::Color([c(0), c(1), c(2), a2])
        }
        (AnimValue::Transform(x), AnimValue::Transform(y)) => match interpolate_transform(x, y, p) {
            Some(t) => AnimValue::Transform(t),
            None => flip(),
        },
        (AnimValue::Discrete(x), AnimValue::Discrete(y)) if prop == "visibility" => {
            // visible throughout when either end is visible (css-transitions-1 §2.1)
            let vis = |s: &str| s == "visible";
            if p > 0.0 && p < 1.0 && (vis(x) || vis(y)) {
                AnimValue::Discrete(String::from("visible"))
            } else {
                flip()
            }
        }
        _ => flip(),
    }
}

/// Whether the pair interpolates smoothly (else a transition is started only with allow-discrete).
pub fn interpolable(prop: &str, a: &AnimValue, b: &AnimValue) -> bool {
    match (a, b) {
        (AnimValue::Nums(x), AnimValue::Nums(y)) => {
            x.len() == y.len()
                && x.iter().zip(y.iter()).all(|(u, v)| {
                    core::mem::discriminant(u) == core::mem::discriminant(v)
                        || matches!((u, v), (Num::N(z), Num::Px(_)) | (Num::Px(_), Num::N(z)) if *z == 0.0)
                })
        }
        (AnimValue::Color(_), AnimValue::Color(_)) => true,
        (AnimValue::Transform(x), AnimValue::Transform(y)) => interpolate_transform(x, y, 0.5).is_some(),
        (AnimValue::Discrete(_), AnimValue::Discrete(_)) => prop == "visibility",
        _ => false,
    }
}

fn fmt_f(v: f32) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{}", r) }
}

/// The CSS text of a value (transforms excepted: they are applied directly).
pub fn to_css(v: &AnimValue) -> String {
    match v {
        AnimValue::Nums(l) => {
            let parts: Vec<String> = l
                .iter()
                .map(|n| match n {
                    Num::N(x) => fmt_f(*x),
                    Num::Px(x) => format!("{}px", fmt_f(*x)),
                    Num::Pct(x) => format!("{}%", fmt_f(*x)),
                    Num::Deg(x) => format!("{}deg", fmt_f(*x)),
                })
                .collect();
            parts.join(" ")
        }
        AnimValue::Color(c) => {
            let ch = |x: f32| x.round().clamp(0.0, 255.0) as u8;
            format!("rgba({}, {}, {}, {})", ch(c[0]), ch(c[1]), ch(c[2]), fmt_f(c[3].clamp(0.0, 1.0)))
        }
        AnimValue::Transform(_) => String::from("none"),
        AnimValue::Discrete(s) => s.clone(),
    }
}

/// Applies an animated value onto an element's style: a transform list directly (`PaintStyle::transform`),
/// anything else as its CSS text through `css::apply_declaration`.
pub fn apply_value<H: Host>(prop: &str, v: &AnimValue, style: &mut SpecifiedStyle<H>, cx: &ParseCx) {
    match v {
        AnimValue::Transform(ops) if prop == "transform" => style.paint.transform = Some(ops.clone()),
        _ => css::apply_declaration(prop, &to_css(v), style, cx),
    }
}

// ---- keyframes ------------------------------------------------------------------------------------------------

/// One `@keyframes` rule as data: its keyframes in offset order, each with its own timing function (the
/// `animation-timing-function` declared inside it) and its declarations (`!important` ones dropped, §3).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyframeSet {
    pub frames: Vec<(f32, Option<Easing>, Vec<(String, String)>)>,
}

impl KeyframeSet {
    /// From css_core's parse (one entry per offset; a selector list is split).
    pub fn from_rule(k: &css_core::stylesheet::Keyframes) -> KeyframeSet {
        let mut frames: Vec<(f32, Option<Easing>, Vec<(String, String)>)> = Vec::new();
        for f in &k.frames {
            let mut decls = Vec::new();
            let mut ease = None;
            for d in &f.declarations {
                if d.important {
                    continue;
                }
                let name = d.name.to_ascii_lowercase();
                let v = css_core::serialize::to_css(css_core::parser::trim_ws(&d.value));
                if name == "animation-timing-function" {
                    ease = Easing::parse(&v);
                } else {
                    decls.push((name, v));
                }
            }
            for &o in &f.offsets {
                frames.push((o as f32, ease.clone(), decls.clone()));
            }
        }
        // stable: equal offsets keep source order (later declarations win when merged)
        frames.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
        KeyframeSet { frames }
    }

    /// The properties the keyframes animate.
    pub fn properties(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (_, _, d) in &self.frames {
            for (p, _) in d {
                if !out.contains(p) {
                    out.push(p.clone());
                }
            }
        }
        out
    }
}

// ---- the timeline ---------------------------------------------------------------------------------------------

/// An event the host dispatches at the element (css-transitions-1 §6, css-animations-1 §4.2).
#[derive(Clone, Debug, PartialEq)]
pub enum AnimEvent<K> {
    TransitionStart(K, String),
    TransitionEnd(K, String),
    TransitionCancel(K, String),
    AnimationStart(K, String),
    AnimationIteration(K, String),
    AnimationEnd(K, String),
}

impl<K> AnimEvent<K> {
    /// The DOM event type.
    pub fn kind(&self) -> &'static str {
        match self {
            AnimEvent::TransitionStart(..) => "transitionstart",
            AnimEvent::TransitionEnd(..) => "transitionend",
            AnimEvent::TransitionCancel(..) => "transitioncancel",
            AnimEvent::AnimationStart(..) => "animationstart",
            AnimEvent::AnimationIteration(..) => "animationiteration",
            AnimEvent::AnimationEnd(..) => "animationend",
        }
    }
    pub fn key(&self) -> &K {
        match self {
            AnimEvent::TransitionStart(k, _)
            | AnimEvent::TransitionEnd(k, _)
            | AnimEvent::TransitionCancel(k, _)
            | AnimEvent::AnimationStart(k, _)
            | AnimEvent::AnimationIteration(k, _)
            | AnimEvent::AnimationEnd(k, _) => k,
        }
    }
}

#[derive(Clone, Debug)]
struct Transition<K> {
    key: K,
    prop: String,
    from: AnimValue,
    to: AnimValue,
    start: f64,
    duration: f64,
    delay: f64,
    easing: Easing,
    /// css-transitions-1 §3's reversing-adjusted start value and shortening factor.
    reversing_from: AnimValue,
    shortening: f64,
    started: bool,
}

#[derive(Clone, Debug)]
struct Animation<K> {
    key: K,
    spec: AnimationSpec,
    frames: KeyframeSet,
    start: f64,
    /// While paused: the local time it froze at.
    paused_at: Option<f64>,
    started: bool,
    iteration: i64,
    ended: bool,
}

/// Running transitions and animations, sampled per frame.
#[derive(Clone, Debug)]
pub struct Timeline<K> {
    transitions: Vec<Transition<K>>,
    animations: Vec<Animation<K>>,
    events: Vec<AnimEvent<K>>,
}

impl<K> Default for Timeline<K> {
    fn default() -> Self {
        Timeline { transitions: Vec::new(), animations: Vec::new(), events: Vec::new() }
    }
}

impl<K: Clone + PartialEq> Timeline<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Anything is running or waiting to start (the shell keeps sending frames).
    pub fn active(&self) -> bool {
        !self.transitions.is_empty() || self.animations.iter().any(|a| !a.ended && a.paused_at.is_none())
    }

    /// The events since the last call.
    pub fn take_events(&mut self) -> Vec<AnimEvent<K>> {
        core::mem::take(&mut self.events)
    }

    /// The current value of a running transition of `prop` on `key`.
    fn current(&self, key: &K, prop: &str, now: f64) -> Option<AnimValue> {
        let t = self.transitions.iter().find(|t| &t.key == key && t.prop == prop)?;
        Some(transition_value(prop, t, now))
    }

    /// A style change of `prop` on `key` from `before` to `after` (computed-value texts) under `spec`
    /// (css-transitions-1 §3): starts, retargets or cancels the property's transition. Returns whether a
    /// transition now runs for it.
    #[allow(clippy::too_many_arguments)]
    pub fn style_change<H: Host>(&mut self, key: &K, prop: &str, before: &str, after: &str, spec: Option<&TransitionSpec>, now: f64, cx: &ParseCx) -> bool {
        let to = parse_value::<H>(prop, after, cx);
        let running = self.transitions.iter().position(|t| &t.key == key && t.prop == prop);
        if let Some(i) = running {
            if self.transitions[i].to == to {
                return true; // same end value: it keeps running
            }
        }
        let Some(spec) = spec.filter(|s| s.duration > 0.0 || s.delay > 0.0) else {
            if let Some(i) = running {
                let t = self.transitions.remove(i);
                self.events.push(AnimEvent::TransitionCancel(t.key, t.prop));
            }
            return false;
        };
        let from = match running {
            Some(_) => self.current(key, prop, now).unwrap_or_else(|| parse_value::<H>(prop, before, cx)),
            None => parse_value::<H>(prop, before, cx),
        };
        if from == to || (!interpolable(prop, &from, &to) && !spec.discrete) {
            if let Some(i) = running {
                let t = self.transitions.remove(i);
                self.events.push(AnimEvent::TransitionCancel(t.key, t.prop));
            }
            return false;
        }
        // §3 step 4: reversing a running transition shortens it
        let (mut duration, mut delay, mut shortening, mut reversing_from) = (spec.duration as f64, spec.delay as f64, 1.0f64, from.clone());
        if let Some(i) = running {
            let old = &self.transitions[i];
            if old.reversing_from == to {
                let p = progress_of(old, now);
                shortening = (p * old.shortening + (1.0 - old.shortening)).clamp(0.0, 1.0);
                if delay < 0.0 {
                    delay *= shortening;
                }
                duration *= shortening;
                reversing_from = old.to.clone();
            }
            self.transitions.remove(i);
        }
        self.transitions.push(Transition {
            key: key.clone(),
            prop: String::from(prop),
            from,
            to,
            start: now,
            duration,
            delay,
            easing: spec.easing.clone(),
            reversing_from,
            shortening,
            started: false,
        });
        true
    }

    /// The animations `key`'s style names now (css-animations-1 §4): a newly listed name starts at `now` with
    /// its `@keyframes` (`frames(name)`; an unknown name runs nothing), a name that stays keeps running with
    /// its timing updated (play-state toggles freeze / resume local time), a name that left is cancelled.
    pub fn set_animations(&mut self, key: &K, specs: &[AnimationSpec], frames: &dyn Fn(&str) -> Option<KeyframeSet>, now: f64) {
        let mut keep = vec![false; self.animations.len()];
        for spec in specs {
            let found = self.animations.iter().position(|a| &a.key == key && a.spec.name == spec.name);
            match found {
                Some(i) => {
                    keep[i] = true;
                    let a = &mut self.animations[i];
                    match (a.paused_at, spec.paused) {
                        (None, true) => a.paused_at = Some(now - a.start),
                        (Some(local), false) => {
                            a.start = now - local;
                            a.paused_at = None;
                        }
                        _ => {}
                    }
                    if let Some(f) = frames(&spec.name) {
                        a.frames = f;
                    }
                    a.spec = spec.clone();
                }
                None => {
                    let Some(f) = frames(&spec.name) else { continue };
                    self.animations.push(Animation {
                        key: key.clone(),
                        spec: spec.clone(),
                        frames: f,
                        start: now,
                        paused_at: if spec.paused { Some(0.0) } else { None },
                        started: false,
                        iteration: 0,
                        ended: false,
                    });
                    keep.push(true);
                }
            }
        }
        let mut i = 0;
        self.animations.retain(|a| {
            let k = keep.get(i).copied().unwrap_or(true) || &a.key != key;
            i += 1;
            k
        });
    }

    /// Drops everything for `key` (its element left the document or lost its box).
    pub fn forget(&mut self, key: &K) {
        self.transitions.retain(|t| &t.key != key);
        self.animations.retain(|a| &a.key != key);
    }

    /// The keys with anything running.
    pub fn keys(&self) -> Vec<K> {
        let mut out: Vec<K> = Vec::new();
        for k in self.transitions.iter().map(|t| &t.key).chain(self.animations.iter().map(|a| &a.key)) {
            if !out.contains(k) {
                out.push(k.clone());
            }
        }
        out
    }

    /// The animated values at `now`, per key: animations first (in list order), then transitions (they win,
    /// css-cascade-5 §6.1). `base(key, prop)` is the element's cascaded value (implicit 0% / 100% keyframes).
    /// Finished transitions are dropped and finished animations stop applying unless they fill forwards; the
    /// events are queued.
    pub fn sample<H: Host>(&mut self, now: f64, base: &dyn Fn(&K, &str) -> Option<String>, cx: &ParseCx) -> Vec<(K, Vec<(String, AnimValue)>)> {
        let mut out: Vec<(K, Vec<(String, AnimValue)>)> = Vec::new();
        let push = |out: &mut Vec<(K, Vec<(String, AnimValue)>)>, k: &K, prop: &str, v: AnimValue| {
            let e = match out.iter_mut().position(|(kk, _)| kk == k) {
                Some(i) => &mut out[i].1,
                None => {
                    out.push((k.clone(), Vec::new()));
                    &mut out.last_mut().unwrap().1
                }
            };
            e.retain(|(p, _)| p != prop);
            e.push((String::from(prop), v));
        };
        for a in self.animations.iter_mut() {
            let local = match a.paused_at {
                Some(l) => l,
                None => now - a.start,
            };
            let s = &a.spec;
            let dur = s.duration as f64;
            let active = dur * s.iterations as f64;
            let t = local - s.delay as f64;
            let (phase, it_progress, iteration) = if t < 0.0 {
                (0u8, 0.0f64, 0i64)
            } else if t >= active && active.is_finite() {
                let it = if dur > 0.0 { s.iterations as f64 } else { s.iterations as f64 };
                let whole = floor64(it);
                let frac = it - whole;
                if frac == 0.0 && whole > 0.0 { (2, 1.0, whole as i64 - 1) } else { (2, frac, whole as i64) }
            } else if dur <= 0.0 {
                (1, 1.0, 0)
            } else {
                let it = floor64(t / dur);
                (1, t / dur - it, it as i64)
            };
            // events
            if phase >= 1 && !a.started {
                a.started = true;
                self.events.push(AnimEvent::AnimationStart(a.key.clone(), s.name.clone()));
            }
            if phase == 1 && iteration > a.iteration {
                a.iteration = iteration;
                self.events.push(AnimEvent::AnimationIteration(a.key.clone(), s.name.clone()));
            }
            if phase == 2 && !a.ended {
                a.ended = true;
                self.events.push(AnimEvent::AnimationEnd(a.key.clone(), s.name.clone()));
            }
            let fills = match phase {
                0 => s.fill == 2 || s.fill == 3,
                2 => s.fill == 1 || s.fill == 3,
                _ => true,
            };
            if !fills {
                continue;
            }
            let reversed = match s.direction {
                1 => true,
                2 => iteration % 2 == 1,
                3 => iteration % 2 == 0,
                _ => false,
            };
            let p = if reversed { 1.0 - it_progress } else { it_progress } as f32;
            for prop in a.frames.properties() {
                if let Some(v) = keyframe_value::<H>(&a.frames, &prop, p, &s.easing, &|pp| base(&a.key, pp), cx) {
                    push(&mut out, &a.key, &prop, v);
                }
            }
        }
        // finished animations that do not fill forwards are dropped
        self.animations.retain(|a| !(a.ended && !(a.spec.fill == 1 || a.spec.fill == 3)));
        let mut done = Vec::new();
        for (i, t) in self.transitions.iter_mut().enumerate() {
            let local = now - t.start;
            if local >= t.delay && !t.started {
                t.started = true;
                self.events.push(AnimEvent::TransitionStart(t.key.clone(), t.prop.clone()));
            }
            if local >= t.delay + t.duration {
                done.push(i);
                self.events.push(AnimEvent::TransitionEnd(t.key.clone(), t.prop.clone()));
                continue;
            }
            let v = transition_value(&t.prop, t, now);
            push(&mut out, &t.key, &t.prop, v);
        }
        for i in done.into_iter().rev() {
            self.transitions.remove(i);
        }
        out
    }
}

/// floor for the timeline's f64 clock (no libm; |x| stays far below 2^53 ms).
fn floor64(x: f64) -> f64 {
    let t = x as i64 as f64;
    if t > x { t - 1.0 } else { t }
}

/// A transition's input progress at `now` (0 before its delay ends).
fn progress_of<K>(t: &Transition<K>, now: f64) -> f64 {
    let local = now - t.start - t.delay;
    if t.duration <= 0.0 {
        return if local >= 0.0 { 1.0 } else { 0.0 };
    }
    (local / t.duration).clamp(0.0, 1.0)
}

fn transition_value<K>(prop: &str, t: &Transition<K>, now: f64) -> AnimValue {
    let local = now - t.start - t.delay;
    let p = progress_of(t, now) as f32;
    let eased = t.easing.apply(p, local < 0.0);
    interpolate(prop, &t.from, &t.to, eased)
}

/// The value of `prop` at iteration progress `p` of a keyframe set: the bracketing keyframes that set it (an
/// implicit 0% / 100% keyframe holds the base value), the segment eased by its start keyframe's timing
/// function or the animation's (css-animations-1 §3.3).
fn keyframe_value<H: Host>(set: &KeyframeSet, prop: &str, p: f32, easing: &Easing, base: &dyn Fn(&str) -> Option<String>, cx: &ParseCx) -> Option<AnimValue> {
    let mut frames: Vec<(f32, Option<Easing>, String)> = Vec::new();
    for (o, e, d) in &set.frames {
        if let Some((_, v)) = d.iter().rev().find(|(pp, _)| pp == prop) {
            match frames.iter_mut().find(|f| f.0 == *o) {
                Some(f) => f.2 = v.clone(),
                None => frames.push((*o, e.clone(), v.clone())),
            }
        }
    }
    if frames.is_empty() {
        return None;
    }
    let base_v = || base(prop).or_else(|| initial_value(prop).map(String::from));
    if frames[0].0 > 0.0 {
        frames.insert(0, (0.0, None, base_v()?));
    }
    if frames.last().map(|f| f.0) < Some(1.0) {
        frames.push((1.0, None, base_v()?));
    }
    let mut i = 0;
    while i + 2 < frames.len() && frames[i + 1].0 <= p {
        i += 1;
    }
    let (a, b) = (&frames[i], &frames[(i + 1).min(frames.len() - 1)]);
    let span = b.0 - a.0;
    let local = if span > 0.0 { ((p - a.0) / span).clamp(0.0, 1.0) } else { 1.0 };
    let eased = a.1.as_ref().unwrap_or(easing).apply(local, false);
    let (va, vb) = (parse_value::<H>(prop, &a.2, cx), parse_value::<H>(prop, &b.2, cx));
    Some(interpolate(prop, &va, &vb, eased))
}
