//! CSS Values and Units 4: lengths and the other dimensions, the math functions (`calc()` `min()` `max()`
//! `clamp()` `abs()` `sign()` `round()` `mod()` `rem()`) as expression trees, and CSS Custom Properties
//! (`var()` substitution, cycle detection).

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::parser::{trim_ws, BlockKind, CV};
use crate::tokenizer::Token;

/// Resolve a length to CSS px. Font-relative units use `em` / `rem` (`ex` and `ch` as 0.5em, `cap` 0.7em,
/// `ic` 1em, `lh` / `rlh` as 1.2em — font metrics belong to the shaper); viewport units the given
/// viewport (all of the small / large / dynamic variants alike).
pub fn absolute_length(v: f64, unit: &str, em: f64, rem: f64, vw: f64, vh: f64) -> Option<f64> {
    let u = unit.to_ascii_lowercase();
    Some(
        v * match u.as_str() {
            "px" => 1.0,
            "cm" => 96.0 / 2.54,
            "mm" => 96.0 / 25.4,
            "q" => 96.0 / 101.6,
            "in" => 96.0,
            "pt" => 96.0 / 72.0,
            "pc" => 16.0,
            "em" => em,
            "rem" => rem,
            "ex" | "ch" => em * 0.5,
            "rex" | "rch" => rem * 0.5,
            "cap" => em * 0.7,
            "ic" => em,
            "ric" => rem,
            "lh" => em * 1.2,
            "rlh" => rem * 1.2,
            "vw" | "svw" | "lvw" | "dvw" | "vi" | "svi" | "lvi" | "dvi" => vw / 100.0,
            "vh" | "svh" | "lvh" | "dvh" | "vb" | "svb" | "lvb" | "dvb" => vh / 100.0,
            "vmin" | "svmin" | "lvmin" | "dvmin" => vw.min(vh) / 100.0,
            "vmax" | "svmax" | "lvmax" | "dvmax" => vw.max(vh) / 100.0,
            _ => return None,
        },
    )
}

/// The context lengths and percentages resolve in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LengthContext {
    pub font_size: f64,
    pub root_font_size: f64,
    pub viewport_width: f64,
    pub viewport_height: f64,
    /// What `100%` is, when known (e.g. the parent font size for `font-size`); `None` keeps percentages.
    pub percent_basis: Option<f64>,
}

impl LengthContext {
    pub fn length(&self, v: f64, unit: &str) -> Option<f64> {
        absolute_length(v, unit, self.font_size, self.root_font_size, self.viewport_width, self.viewport_height)
    }
}

/// The dimension category of a unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dim {
    Length,
    Angle,
    Time,
    Frequency,
    Resolution,
}

fn unit_dim(u: &str) -> Option<(Dim, f64)> {
    let u = u.to_ascii_lowercase();
    Some(match u.as_str() {
        "deg" => (Dim::Angle, 1.0),
        "grad" => (Dim::Angle, 0.9),
        "rad" => (Dim::Angle, 180.0 / core::f64::consts::PI),
        "turn" => (Dim::Angle, 360.0),
        "s" => (Dim::Time, 1.0),
        "ms" => (Dim::Time, 0.001),
        "hz" => (Dim::Frequency, 1.0),
        "khz" => (Dim::Frequency, 1000.0),
        "dppx" | "x" => (Dim::Resolution, 1.0),
        "dpi" => (Dim::Resolution, 1.0 / 96.0),
        "dpcm" => (Dim::Resolution, 2.54 / 96.0),
        _ => {
            absolute_length(1.0, &u, 1.0, 1.0, 1.0, 1.0)?;
            (Dim::Length, 0.0)
        }
    })
}

/// A math-function expression tree (css-values-4 §10.1).
#[derive(Clone, Debug, PartialEq)]
pub enum Calc {
    Number(f64),
    Percentage(f64),
    Dimension(f64, String),
    Sum(Vec<Calc>),
    Product(Vec<Calc>),
    Negate(Box<Calc>),
    Invert(Box<Calc>),
    Min(Vec<Calc>),
    Max(Vec<Calc>),
    /// `clamp(min, val, max)`; `None` for `none`.
    Clamp(Option<Box<Calc>>, Box<Calc>, Option<Box<Calc>>),
    Abs(Box<Calc>),
    Sign(Box<Calc>),
    /// `round(strategy, a, b)`; strategy `nearest` `up` `down` `to-zero`.
    Round(RoundStrategy, Box<Calc>, Box<Calc>),
    Mod(Box<Calc>, Box<Calc>),
    Rem(Box<Calc>, Box<Calc>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundStrategy {
    Nearest,
    Up,
    Down,
    ToZero,
}

/// Is this a math function name?
pub fn is_math_function(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "calc" | "min" | "max" | "clamp" | "abs" | "sign" | "round" | "mod" | "rem" | "-webkit-calc")
}

/// Parse a math function (the `Function` component value).
pub fn parse_math(cv: &CV) -> Option<Calc> {
    match cv {
        CV::Function(name, args) => math_fn(&name.to_ascii_lowercase(), args),
        _ => None,
    }
}

fn math_fn(name: &str, args: &[CV]) -> Option<Calc> {
    let parts: Vec<&[CV]> = args.split(|c| matches!(c, CV::Token(Token::Comma))).collect();
    let one = |p: &[CV]| full_sum(p);
    match name {
        "calc" | "-webkit-calc" => {
            if parts.len() != 1 {
                return None;
            }
            one(parts[0])
        }
        "min" | "max" => {
            let v: Option<Vec<Calc>> = parts.iter().map(|p| one(p)).collect();
            let v = v?;
            Some(if name == "min" { Calc::Min(v) } else { Calc::Max(v) })
        }
        "clamp" => {
            if parts.len() != 3 {
                return None;
            }
            let opt = |p: &[CV]| -> Option<Option<Box<Calc>>> {
                if matches!(trim_ws(p), [CV::Token(Token::Ident(s))] if s.eq_ignore_ascii_case("none")) {
                    Some(None)
                } else {
                    Some(Some(Box::new(one(p)?)))
                }
            };
            Some(Calc::Clamp(opt(parts[0])?, Box::new(one(parts[1])?), opt(parts[2])?))
        }
        "abs" | "sign" => {
            if parts.len() != 1 {
                return None;
            }
            let x = Box::new(one(parts[0])?);
            Some(if name == "abs" { Calc::Abs(x) } else { Calc::Sign(x) })
        }
        "round" => {
            let mut strategy = RoundStrategy::Nearest;
            let mut p = parts.as_slice();
            if let Some(first) = p.first()
                && let [CV::Token(Token::Ident(s))] = trim_ws(first) {
                    let s = s.to_ascii_lowercase();
                    let st = match s.as_str() {
                        "nearest" => Some(RoundStrategy::Nearest),
                        "up" => Some(RoundStrategy::Up),
                        "down" => Some(RoundStrategy::Down),
                        "to-zero" => Some(RoundStrategy::ToZero),
                        _ => None,
                    };
                    if let Some(st) = st {
                        strategy = st;
                        p = &p[1..];
                    }
                }
            match p {
                [a] => Some(Calc::Round(strategy, Box::new(one(a)?), Box::new(Calc::Number(1.0)))),
                [a, b] => Some(Calc::Round(strategy, Box::new(one(a)?), Box::new(one(b)?))),
                _ => None,
            }
        }
        "mod" | "rem" => {
            if parts.len() != 2 {
                return None;
            }
            let (a, b) = (Box::new(one(parts[0])?), Box::new(one(parts[1])?));
            Some(if name == "mod" { Calc::Mod(a, b) } else { Calc::Rem(a, b) })
        }
        _ => None,
    }
}

fn full_sum(p: &[CV]) -> Option<Calc> {
    let v = trim_ws(p);
    let mut i = 0;
    let r = sum(v, &mut i)?;
    if i == v.len() { Some(r) } else { None }
}

fn skip_ws(v: &[CV], i: &mut usize) -> bool {
    let s = *i;
    while *i < v.len() && v[*i].is_whitespace() {
        *i += 1;
    }
    *i > s
}

/// `<calc-sum>`: `+` / `-` must be surrounded by whitespace.
fn sum(v: &[CV], i: &mut usize) -> Option<Calc> {
    let mut terms = alloc::vec![product(v, i)?];
    loop {
        let save = *i;
        if !skip_ws(v, i) {
            *i = save;
            break;
        }
        let neg = match v.get(*i) {
            Some(c) if c.is_delim('+') => false,
            Some(c) if c.is_delim('-') => true,
            _ => {
                *i = save;
                break;
            }
        };
        *i += 1;
        if !skip_ws(v, i) {
            return None;
        }
        let t = product(v, i)?;
        terms.push(if neg { Calc::Negate(Box::new(t)) } else { t });
    }
    Some(if terms.len() == 1 { terms.pop().unwrap() } else { Calc::Sum(terms) })
}

fn product(v: &[CV], i: &mut usize) -> Option<Calc> {
    let mut f = alloc::vec![value(v, i)?];
    loop {
        let save = *i;
        skip_ws(v, i);
        match v.get(*i) {
            Some(c) if c.is_delim('*') => {
                *i += 1;
                skip_ws(v, i);
                f.push(value(v, i)?);
            }
            Some(c) if c.is_delim('/') => {
                *i += 1;
                skip_ws(v, i);
                f.push(Calc::Invert(Box::new(value(v, i)?)));
            }
            _ => {
                *i = save;
                break;
            }
        }
    }
    Some(if f.len() == 1 { f.pop().unwrap() } else { Calc::Product(f) })
}

fn value(v: &[CV], i: &mut usize) -> Option<Calc> {
    let c = v.get(*i)?;
    *i += 1;
    Some(match c {
        CV::Token(Token::Number(n)) => Calc::Number(n.value),
        CV::Token(Token::Percentage(n)) => Calc::Percentage(n.value),
        CV::Token(Token::Dimension(n, u)) => {
            unit_dim(u)?;
            Calc::Dimension(n.value, u.to_ascii_lowercase())
        }
        CV::Token(Token::Ident(s)) => match s.to_ascii_lowercase().as_str() {
            "e" => Calc::Number(core::f64::consts::E),
            "pi" => Calc::Number(core::f64::consts::PI),
            "infinity" => Calc::Number(f64::INFINITY),
            "-infinity" => Calc::Number(f64::NEG_INFINITY),
            "nan" => Calc::Number(f64::NAN),
            _ => return None,
        },
        CV::Block(BlockKind::Paren, inner) => full_sum(inner)?,
        CV::Function(name, args) => math_fn(&name.to_ascii_lowercase(), args)?,
        _ => return None,
    })
}

/// The type and value of a resolved expression: a number, a `<length-percentage>` (px + %), or another
/// dimension in its canonical unit (deg, s, Hz, dppx).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resolved {
    Number(f64),
    /// px + percentage (percentage kept when the basis is unknown)
    LengthPercentage { px: f64, pct: f64, has_pct: bool, has_len: bool },
    Other(Dim, f64),
}

impl Resolved {
    /// Px when no unresolved percentage remains.
    pub fn px(&self) -> Option<f64> {
        match self {
            Resolved::LengthPercentage { px, pct, .. } if *pct == 0.0 => Some(*px),
            Resolved::Number(n) if *n == 0.0 => Some(0.0),
            _ => None,
        }
    }
    fn scalar(&self) -> Option<f64> {
        match self {
            Resolved::Number(n) => Some(*n),
            Resolved::Other(_, v) => Some(*v),
            Resolved::LengthPercentage { px, pct, has_pct, has_len } => {
                if *pct == 0.0 || !*has_len && *px == 0.0 {
                    if *has_pct && !*has_len { Some(*pct) } else { Some(*px) }
                } else {
                    None
                }
            }
        }
    }
    fn same_kind(&self, o: &Resolved) -> bool {
        match (self, o) {
            (Resolved::Number(_), Resolved::Number(_)) => true,
            (Resolved::LengthPercentage { .. }, Resolved::LengthPercentage { .. }) => true,
            (Resolved::Other(a, _), Resolved::Other(b, _)) => a == b,
            _ => false,
        }
    }
    fn with(&self, v: f64) -> Resolved {
        match self {
            Resolved::Number(_) => Resolved::Number(v),
            Resolved::Other(d, _) => Resolved::Other(*d, v),
            Resolved::LengthPercentage { has_pct, has_len, .. } => {
                if *has_pct && !*has_len {
                    Resolved::LengthPercentage { px: 0.0, pct: v, has_pct: true, has_len: false }
                } else {
                    Resolved::LengthPercentage { px: v, pct: 0.0, has_pct: false, has_len: true }
                }
            }
        }
    }
    fn scale(&self, k: f64) -> Resolved {
        match *self {
            Resolved::Number(n) => Resolved::Number(n * k),
            Resolved::Other(d, v) => Resolved::Other(d, v * k),
            Resolved::LengthPercentage { px, pct, has_pct, has_len } => Resolved::LengthPercentage { px: px * k, pct: pct * k, has_pct, has_len },
        }
    }
}

impl Calc {
    /// Evaluate in `cx`. `None` for a type error (e.g. a length times a length) or a comparison that
    /// needs an unknown percentage basis.
    pub fn resolve(&self, cx: &LengthContext) -> Option<Resolved> {
        Some(match self {
            Calc::Number(n) => Resolved::Number(*n),
            Calc::Percentage(p) => match cx.percent_basis {
                Some(b) => Resolved::LengthPercentage { px: b * p / 100.0, pct: 0.0, has_pct: false, has_len: true },
                None => Resolved::LengthPercentage { px: 0.0, pct: *p, has_pct: true, has_len: false },
            },
            Calc::Dimension(v, u) => match unit_dim(u)? {
                (Dim::Length, _) => Resolved::LengthPercentage { px: cx.length(*v, u)?, pct: 0.0, has_pct: false, has_len: true },
                (d, k) => Resolved::Other(d, v * k),
            },
            Calc::Sum(ts) => {
                let mut acc = ts[0].resolve(cx)?;
                for t in &ts[1..] {
                    let r = t.resolve(cx)?;
                    acc = match (acc, r) {
                        (Resolved::Number(a), Resolved::Number(b)) => Resolved::Number(a + b),
                        (Resolved::Other(d, a), Resolved::Other(e, b)) if d == e => Resolved::Other(d, a + b),
                        (
                            Resolved::LengthPercentage { px: a, pct: p, has_pct: hp, has_len: hl },
                            Resolved::LengthPercentage { px: b, pct: q, has_pct: hq, has_len: hm },
                        ) => Resolved::LengthPercentage { px: a + b, pct: p + q, has_pct: hp || hq, has_len: hl || hm },
                        _ => return None,
                    };
                }
                acc
            }
            Calc::Negate(x) => x.resolve(cx)?.scale(-1.0),
            Calc::Product(fs) => {
                let mut acc = Resolved::Number(1.0);
                for f in fs {
                    let r = f.resolve(cx)?;
                    acc = match (acc, r) {
                        (Resolved::Number(a), b) => b.scale(a),
                        (a, Resolved::Number(b)) => a.scale(b),
                        _ => return None,
                    };
                }
                acc
            }
            Calc::Invert(x) => match x.resolve(cx)? {
                Resolved::Number(n) => Resolved::Number(1.0 / n),
                _ => return None,
            },
            Calc::Min(v) | Calc::Max(v) => {
                let rs: Option<Vec<Resolved>> = v.iter().map(|c| c.resolve(cx)).collect();
                let rs = rs?;
                let first = rs[0];
                let mut best = first.scalar()?;
                for r in &rs[1..] {
                    if !r.same_kind(&first) {
                        return None;
                    }
                    let s = r.scalar()?;
                    best = if matches!(self, Calc::Min(_)) { best.min(s) } else { best.max(s) };
                }
                // a mix of plain % and lengths cannot be compared without the basis
                if let Resolved::LengthPercentage { .. } = first {
                    let kinds: Vec<(bool, bool)> = rs
                        .iter()
                        .map(|r| match r {
                            Resolved::LengthPercentage { has_pct, has_len, .. } => (*has_pct, *has_len),
                            _ => (false, false),
                        })
                        .collect();
                    if kinds.iter().any(|k| k.0) && kinds.iter().any(|k| k.1) {
                        return None;
                    }
                }
                first.with(best)
            }
            Calc::Clamp(lo, val, hi) => {
                let v = val.resolve(cx)?;
                let mut x = v.scalar()?;
                if let Some(h) = hi {
                    let h = h.resolve(cx)?;
                    if !h.same_kind(&v) {
                        return None;
                    }
                    x = x.min(h.scalar()?);
                }
                if let Some(l) = lo {
                    let l = l.resolve(cx)?;
                    if !l.same_kind(&v) {
                        return None;
                    }
                    x = x.max(l.scalar()?);
                }
                v.with(x)
            }
            Calc::Abs(x) => {
                let r = x.resolve(cx)?;
                r.with(r.scalar()?.abs())
            }
            Calc::Sign(x) => {
                let s = x.resolve(cx)?.scalar()?;
                Resolved::Number(if s > 0.0 { 1.0 } else if s < 0.0 { -1.0 } else { s })
            }
            Calc::Round(st, a, b) => {
                let (ra, rb) = (a.resolve(cx)?, b.resolve(cx)?);
                let (x, step) = (ra.scalar()?, rb.scalar()?);
                if step == 0.0 {
                    return Some(ra.with(f64::NAN));
                }
                let q = x / step;
                let r = match st {
                    RoundStrategy::Nearest => libm_round(q),
                    RoundStrategy::Up => libm_ceil(q),
                    RoundStrategy::Down => libm_floor(q),
                    RoundStrategy::ToZero => libm_trunc(q),
                };
                ra.with(r * step)
            }
            Calc::Mod(a, b) | Calc::Rem(a, b) => {
                let (ra, rb) = (a.resolve(cx)?, b.resolve(cx)?);
                let (x, y) = (ra.scalar()?, rb.scalar()?);
                let r = x % y;
                let r = if matches!(self, Calc::Mod(..)) && r != 0.0 && (r < 0.0) != (y < 0.0) { r + y } else { r };
                ra.with(r)
            }
        })
    }
}

// no_std float helpers (core has no floor/ceil/round on f64 without std)
fn libm_trunc(x: f64) -> f64 {
    if !x.is_finite() || x.abs() >= 4503599627370496.0 {
        x
    } else {
        (x as i64) as f64
    }
}
fn libm_floor(x: f64) -> f64 {
    let t = libm_trunc(x);
    if t > x { t - 1.0 } else { t }
}
fn libm_ceil(x: f64) -> f64 {
    let t = libm_trunc(x);
    if t < x { t + 1.0 } else { t }
}
/// Round half towards +∞ (css-values-4 `nearest`).
fn libm_round(x: f64) -> f64 {
    libm_floor(x + 0.5)
}

// ------------------------------------------------------------------------------------------------
// custom properties
// ------------------------------------------------------------------------------------------------

/// Does the value reference `var()` anywhere?
pub fn contains_var(v: &[CV]) -> bool {
    v.iter().any(|c| match c {
        CV::Function(n, args) => n.eq_ignore_ascii_case("var") || contains_var(args),
        CV::Block(_, inner) => contains_var(inner),
        _ => false,
    })
}

/// Substitute every `var(--name [, fallback])` in `v` (css-variables §3). `lookup` yields a custom
/// property's computed value. `Err` = invalid at computed-value time (no value and no fallback).
pub fn substitute_vars(v: &[CV], lookup: &dyn Fn(&str) -> Option<Vec<CV>>) -> Result<Vec<CV>, ()> {
    let mut out = Vec::with_capacity(v.len());
    for c in v {
        match c {
            CV::Function(n, args) if n.eq_ignore_ascii_case("var") => {
                let (name, fallback) = var_parts(args).ok_or(())?;
                match lookup(&name) {
                    Some(val) => out.extend(val),
                    None => match fallback {
                        Some(fb) => out.extend(substitute_vars(trim_ws(fb), lookup)?),
                        None => return Err(()),
                    },
                }
            }
            CV::Function(n, args) => out.push(CV::Function(n.clone(), substitute_vars(args, lookup)?)),
            CV::Block(k, inner) => out.push(CV::Block(*k, substitute_vars(inner, lookup)?)),
            c => out.push(c.clone()),
        }
    }
    Ok(out)
}

/// `var()` arguments: the custom property name and the optional fallback (possibly empty).
pub fn var_parts(args: &[CV]) -> Option<(String, Option<&[CV]>)> {
    let mut i = 0;
    while i < args.len() && args[i].is_whitespace() {
        i += 1;
    }
    let name = match args.get(i) {
        Some(CV::Token(Token::Ident(n))) if n.starts_with("--") => n.clone(),
        _ => return None,
    };
    i += 1;
    while i < args.len() && args[i].is_whitespace() {
        i += 1;
    }
    match args.get(i) {
        None => Some((name, None)),
        Some(CV::Token(Token::Comma)) => Some((name, Some(&args[i + 1..]))),
        _ => None,
    }
}

/// The names `var()` references directly (fallbacks included).
fn references(v: &[CV], out: &mut Vec<String>) {
    for c in v {
        match c {
            CV::Function(n, args) => {
                if n.eq_ignore_ascii_case("var") {
                    if let Some((name, fb)) = var_parts(args) {
                        out.push(name);
                        if let Some(fb) = fb {
                            references(fb, out);
                        }
                    }
                } else {
                    references(args, out);
                }
            }
            CV::Block(_, inner) => references(inner, out),
            _ => {}
        }
    }
}

/// An element's computed custom properties: `specified` are its cascaded `--*` values (CSS-wide
/// keywords included), `inherited` its parent's computed map. `var()` references are substituted;
/// properties in a dependency cycle, or referencing an invalid property without fallback, become the
/// guaranteed-invalid value (absent from the map). Values are whitespace-trimmed.
pub fn compute_custom_properties(specified: &[(String, Vec<CV>)], inherited: &BTreeMap<String, Vec<CV>>) -> BTreeMap<String, Vec<CV>> {
    let mut raw: BTreeMap<String, Option<Vec<CV>>> = BTreeMap::new();
    for (name, value) in specified {
        let v = trim_ws(value);
        let kw = match v {
            [CV::Token(Token::Ident(k))] => Some(k.to_ascii_lowercase()),
            _ => None,
        };
        let r = match kw.as_deref() {
            Some("initial") => None,
            Some("inherit") | Some("unset") | Some("revert") | Some("revert-layer") => inherited.get(name).cloned(),
            _ => Some(v.to_vec()),
        };
        raw.insert(name.clone(), r);
    }
    let mut out = inherited.clone();
    for k in raw.keys() {
        out.remove(k);
    }
    // depth-first resolution with cycle detection
    #[derive(Clone, Copy, PartialEq)]
    enum St {
        Todo,
        Busy,
        Done,
    }
    let names: Vec<String> = raw.keys().cloned().collect();
    let mut st: BTreeMap<String, St> = names.iter().map(|n| (n.clone(), St::Todo)).collect();
    let mut cyclic: Vec<String> = Vec::new();
    fn visit(
        n: &str,
        raw: &BTreeMap<String, Option<Vec<CV>>>,
        st: &mut BTreeMap<String, St>,
        out: &mut BTreeMap<String, Vec<CV>>,
        cyclic: &mut Vec<String>,
        stack: &mut Vec<String>,
    ) {
        match st.get(n) {
            Some(St::Done) | None => return,
            Some(St::Busy) => {
                // every property on the stack from n onwards is in the cycle
                if let Some(p) = stack.iter().position(|s| s == n) {
                    for s in &stack[p..] {
                        cyclic.push(s.clone());
                    }
                }
                return;
            }
            Some(St::Todo) => {}
        }
        st.insert(String::from(n), St::Busy);
        stack.push(String::from(n));
        if let Some(Some(v)) = raw.get(n) {
            let mut refs = Vec::new();
            references(v, &mut refs);
            for r in &refs {
                visit(r, raw, st, out, cyclic, stack);
            }
            if !cyclic.iter().any(|c| c == n) {
                let res = substitute_vars(v, &|name| if cyclic.iter().any(|c| c == name) { None } else { out.get(name).cloned() });
                if let Ok(val) = res {
                    out.insert(String::from(n), trim_ws(&val).to_vec());
                }
            }
        }
        stack.pop();
        st.insert(String::from(n), St::Done);
    }
    for n in &names {
        let mut stack = Vec::new();
        visit(n, &raw, &mut st, &mut out, &mut cyclic, &mut stack);
    }
    for c in &cyclic {
        out.remove(c);
    }
    out
}
