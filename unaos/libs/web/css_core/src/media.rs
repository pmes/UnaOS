//! Media Queries Level 4: the grammar (§3) parsed over component values and evaluated (§2.4, three-valued
//! logic) against an [`Environment`] (the viewport and the user's preferences).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::parser::{trim_ws, BlockKind, CV};
use crate::tokenizer::Token;
use crate::values::absolute_length;

/// What a media query is evaluated against. Lengths in CSS px.
#[derive(Clone, Debug, PartialEq)]
pub struct Environment {
    pub width: f64,
    pub height: f64,
    /// device pixels per CSS px
    pub dppx: f64,
    /// `"screen"` or `"print"`
    pub media_type: &'static str,
    pub dark: bool,
    pub reduced_motion: bool,
    /// primary pointer: `"none"`, `"coarse"`, `"fine"`
    pub pointer: &'static str,
    pub hover: bool,
    pub color_bits: u32,
    pub monochrome_bits: u32,
    /// `"none"`, `"initial-only"`, `"enabled"`
    pub scripting: &'static str,
    /// the initial font size MQ font-relative units resolve against
    pub font_size: f64,
}

impl Default for Environment {
    /// Chromium headless on a desktop: 800×600 screen, 1dppx, light, fine pointer with hover.
    fn default() -> Self {
        Environment {
            width: 800.0,
            height: 600.0,
            dppx: 1.0,
            media_type: "screen",
            dark: false,
            reduced_motion: false,
            pointer: "fine",
            hover: true,
            color_bits: 8,
            monochrome_bits: 0,
            scripting: "enabled",
            font_size: 16.0,
        }
    }
}

impl Environment {
    pub fn viewport(width: f64, height: f64) -> Self {
        Environment { width, height, ..Default::default() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tri {
    True,
    False,
    Unknown,
}

impl Tri {
    fn not(self) -> Tri {
        match self {
            Tri::True => Tri::False,
            Tri::False => Tri::True,
            Tri::Unknown => Tri::Unknown,
        }
    }
    fn from(b: bool) -> Tri {
        if b { Tri::True } else { Tri::False }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MfValue {
    Number(f64),
    Dimension(f64, String),
    Ratio(f64, f64),
    Ident(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeOp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

impl RangeOp {
    fn flip(self) -> RangeOp {
        match self {
            RangeOp::Lt => RangeOp::Gt,
            RangeOp::Le => RangeOp::Ge,
            RangeOp::Gt => RangeOp::Lt,
            RangeOp::Ge => RangeOp::Le,
            RangeOp::Eq => RangeOp::Eq,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MediaFeature {
    Boolean(String),
    /// `(name: value)`, `name` may carry `min-` / `max-`.
    Plain(String, MfValue),
    /// `feature op value` constraints (each normalised to "feature OP value").
    Range(String, Vec<(RangeOp, MfValue)>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum MediaCondition {
    Not(Box<MediaCondition>),
    And(Vec<MediaCondition>),
    Or(Vec<MediaCondition>),
    Feature(MediaFeature),
    /// `<general-enclosed>`: always unknown.
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MediaQuery {
    /// A query that failed to parse: "not all".
    Invalid,
    Typed { not: bool, media_type: String, condition: Option<MediaCondition> },
    Condition(MediaCondition),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct MediaQueryList(pub Vec<MediaQuery>);

fn lower(s: &str) -> String {
    s.chars().map(|c| c.to_ascii_lowercase()).collect()
}

fn nonws(v: &[CV]) -> Vec<&CV> {
    v.iter().filter(|c| !c.is_whitespace()).collect()
}

/// `<media-query-list>`: an empty list is "all"; each malformed query becomes "not all".
pub fn parse_media_query_list(v: &[CV]) -> MediaQueryList {
    if trim_ws(v).is_empty() {
        return MediaQueryList(Vec::new());
    }
    MediaQueryList(v.split(|c| matches!(c, CV::Token(Token::Comma))).map(parse_media_query).collect())
}

fn parse_media_query(v: &[CV]) -> MediaQuery {
    let w = nonws(v);
    if w.is_empty() {
        return MediaQuery::Invalid;
    }
    if let Some(first) = w[0].ident() {
        let l = lower(first);
        if !(l == "not" && matches!(w.get(1), Some(CV::Block(BlockKind::Paren, _)))) {
            let mut i = 0;
            let mut not = false;
            if l == "not" || l == "only" {
                not = l == "not";
                i = 1;
            }
            let Some(t) = w.get(i).and_then(|c| c.ident()) else { return MediaQuery::Invalid };
            let t = lower(t);
            if matches!(t.as_str(), "not" | "and" | "or" | "only" | "layer") {
                return MediaQuery::Invalid;
            }
            i += 1;
            if i == w.len() {
                return MediaQuery::Typed { not, media_type: t, condition: None };
            }
            if !matches!(w[i].ident(), Some(a) if a.eq_ignore_ascii_case("and")) {
                return MediaQuery::Invalid;
            }
            return match condition(&w[i + 1..], false) {
                Some(c) => MediaQuery::Typed { not, media_type: t, condition: Some(c) },
                None => MediaQuery::Invalid,
            };
        }
    }
    match condition(&w, true) {
        Some(c) => MediaQuery::Condition(c),
        None => MediaQuery::Invalid,
    }
}

/// `<media-condition>` (or `-without-or`) over non-whitespace values.
fn condition(w: &[&CV], allow_or: bool) -> Option<MediaCondition> {
    if w.is_empty() {
        return None;
    }
    if matches!(w[0].ident(), Some(n) if n.eq_ignore_ascii_case("not")) {
        if w.len() != 2 {
            return None;
        }
        return Some(MediaCondition::Not(Box::new(in_parens(w[1])?)));
    }
    let first = in_parens(w[0])?;
    if w.len() == 1 {
        return Some(first);
    }
    let op = lower(w[1].ident()?);
    if op != "and" && !(op == "or" && allow_or) {
        return None;
    }
    let mut items = alloc::vec![first];
    let mut i = 1;
    while i < w.len() {
        if !matches!(w[i].ident(), Some(o) if o.eq_ignore_ascii_case(&op)) {
            return None;
        }
        items.push(in_parens(*w.get(i + 1)?)?);
        i += 2;
    }
    Some(if op == "and" { MediaCondition::And(items) } else { MediaCondition::Or(items) })
}

/// `<media-in-parens>`: `( <media-condition> )`, a `<media-feature>`, or `<general-enclosed>`.
fn in_parens(c: &CV) -> Option<MediaCondition> {
    match c {
        CV::Block(BlockKind::Paren, inner) => {
            let w = nonws(inner);
            if w.is_empty() {
                return Some(MediaCondition::Unknown);
            }
            if matches!(w[0], CV::Block(BlockKind::Paren, _)) || matches!(w[0].ident(), Some(n) if n.eq_ignore_ascii_case("not")) {
                return Some(condition(&w, true).unwrap_or(MediaCondition::Unknown));
            }
            Some(feature(inner).map(MediaCondition::Feature).unwrap_or(MediaCondition::Unknown))
        }
        CV::Function(..) => Some(MediaCondition::Unknown),
        _ => None,
    }
}

enum Item {
    V(MfValue),
    Op(RangeOp),
}

fn feature(inner: &[CV]) -> Option<MediaFeature> {
    let w = nonws(inner);
    if let [CV::Token(Token::Ident(n))] = w.as_slice() {
        return Some(MediaFeature::Boolean(lower(n)));
    }
    if let (Some(CV::Token(Token::Ident(n))), Some(CV::Token(Token::Colon))) = (w.first(), w.get(1)) {
        return Some(MediaFeature::Plain(lower(n), mf_value(&w[2..])?));
    }
    // range syntax: tokenise into values and operators (`<=` / `>=` must be adjacent)
    let v = trim_ws(inner);
    let mut items = Vec::new();
    let mut i = 0;
    let mut cur: Vec<&CV> = Vec::new();
    while i < v.len() {
        let op = match &v[i] {
            CV::Token(Token::Delim('<')) => Some(RangeOp::Lt),
            CV::Token(Token::Delim('>')) => Some(RangeOp::Gt),
            CV::Token(Token::Delim('=')) => Some(RangeOp::Eq),
            _ => None,
        };
        if let Some(mut op) = op {
            if !cur.is_empty() {
                items.push(Item::V(mf_value(&cur)?));
                cur.clear();
            }
            if op != RangeOp::Eq && v.get(i + 1).map(|c| c.is_delim('=')).unwrap_or(false) {
                op = if op == RangeOp::Lt { RangeOp::Le } else { RangeOp::Ge };
                i += 1;
            }
            items.push(Item::Op(op));
        } else if !v[i].is_whitespace() {
            cur.push(&v[i]);
        }
        i += 1;
    }
    if !cur.is_empty() {
        items.push(Item::V(mf_value(&cur)?));
    }
    let name_of = |it: &Item| match it {
        Item::V(MfValue::Ident(n)) => Some(n.clone()),
        _ => None,
    };
    match items.as_slice() {
        [a, Item::Op(op), b] => {
            if let Some(n) = name_of(a) {
                let Item::V(val) = b else { return None };
                Some(MediaFeature::Range(n, alloc::vec![(*op, val.clone())]))
            } else if let Some(n) = name_of(b) {
                let Item::V(val) = a else { return None };
                Some(MediaFeature::Range(n, alloc::vec![(op.flip(), val.clone())]))
            } else {
                None
            }
        }
        [Item::V(lo), Item::Op(o1), name, Item::Op(o2), Item::V(hi)] => {
            let n = name_of(name)?;
            let lt = |o: &RangeOp| matches!(o, RangeOp::Lt | RangeOp::Le);
            let gt = |o: &RangeOp| matches!(o, RangeOp::Gt | RangeOp::Ge);
            if !((lt(o1) && lt(o2)) || (gt(o1) && gt(o2))) {
                return None;
            }
            Some(MediaFeature::Range(n, alloc::vec![(o1.flip(), lo.clone()), (*o2, hi.clone())]))
        }
        _ => None,
    }
}

fn mf_value(w: &[&CV]) -> Option<MfValue> {
    match w {
        [CV::Token(Token::Number(n))] => Some(MfValue::Number(n.value)),
        [CV::Token(Token::Dimension(n, u))] => Some(MfValue::Dimension(n.value, lower(u))),
        [CV::Token(Token::Ident(s))] => Some(MfValue::Ident(lower(s))),
        [CV::Token(Token::Number(a)), d, CV::Token(Token::Number(b))] if d.is_delim('/') => Some(MfValue::Ratio(a.value, b.value)),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------
// evaluation
// ------------------------------------------------------------------------------------------------

impl MediaQueryList {
    /// Does the list match? (Empty: yes.)
    pub fn matches(&self, env: &Environment) -> bool {
        self.0.is_empty() || self.0.iter().any(|q| q.evaluate(env) == Tri::True)
    }
}

impl MediaQuery {
    pub fn evaluate(&self, env: &Environment) -> Tri {
        match self {
            MediaQuery::Invalid => Tri::False,
            MediaQuery::Condition(c) => c.evaluate(env),
            MediaQuery::Typed { not, media_type, condition } => {
                let t = Tri::from(media_type == "all" || media_type == env.media_type);
                let r = match (t, condition) {
                    (Tri::False, _) => Tri::False,
                    (_, None) => t,
                    (_, Some(c)) => c.evaluate(env),
                };
                if *not { r.not() } else { r }
            }
        }
    }
}

impl MediaCondition {
    pub fn evaluate(&self, env: &Environment) -> Tri {
        match self {
            MediaCondition::Not(c) => c.evaluate(env).not(),
            MediaCondition::And(v) => {
                let mut r = Tri::True;
                for c in v {
                    match c.evaluate(env) {
                        Tri::False => return Tri::False,
                        Tri::Unknown => r = Tri::Unknown,
                        Tri::True => {}
                    }
                }
                r
            }
            MediaCondition::Or(v) => {
                let mut r = Tri::False;
                for c in v {
                    match c.evaluate(env) {
                        Tri::True => return Tri::True,
                        Tri::Unknown => r = Tri::Unknown,
                        Tri::False => {}
                    }
                }
                r
            }
            MediaCondition::Feature(f) => f.evaluate(env),
            MediaCondition::Unknown => Tri::Unknown,
        }
    }
}

/// The value of a range feature in its canonical unit, or `None` for a discrete / unknown one.
fn range_value(name: &str, env: &Environment) -> Option<f64> {
    Some(match name {
        "width" | "device-width" => env.width,
        "height" | "device-height" => env.height,
        "aspect-ratio" | "device-aspect-ratio" => env.width / env.height,
        "resolution" => env.dppx,
        "color" => env.color_bits as f64,
        "color-index" => 0.0,
        "monochrome" => env.monochrome_bits as f64,
        _ => return None,
    })
}

/// A feature value in the canonical unit of `name` (px, ratio, dppx, integer).
fn canonical(name: &str, v: &MfValue, env: &Environment) -> Option<f64> {
    match name {
        "width" | "height" | "device-width" | "device-height" => match v {
            MfValue::Number(n) if *n == 0.0 => Some(0.0),
            MfValue::Dimension(n, u) => absolute_length(*n, u, env.font_size, env.font_size, env.width, env.height),
            _ => None,
        },
        "aspect-ratio" | "device-aspect-ratio" => match v {
            MfValue::Ratio(a, b) => Some(if *b == 0.0 { f64::INFINITY } else { a / b }),
            MfValue::Number(n) => Some(*n),
            _ => None,
        },
        "resolution" => match v {
            MfValue::Dimension(n, u) => match u.as_str() {
                "dppx" | "x" => Some(*n),
                "dpi" => Some(n / 96.0),
                "dpcm" => Some(n * 2.54 / 96.0),
                _ => None,
            },
            _ => None,
        },
        "color" | "color-index" | "monochrome" => match v {
            MfValue::Number(n) if *n == (*n as i64) as f64 && *n >= 0.0 => Some(*n),
            _ => None,
        },
        _ => None,
    }
}

fn compare(a: f64, op: RangeOp, b: f64) -> bool {
    match op {
        RangeOp::Lt => a < b,
        RangeOp::Le => a <= b,
        RangeOp::Gt => a > b,
        RangeOp::Ge => a >= b,
        RangeOp::Eq => (a - b).abs() < 1e-9,
    }
}

/// A discrete feature's current keyword and whether it is "true" in the boolean context.
fn discrete(name: &str, env: &Environment) -> Option<(&'static str, bool)> {
    Some(match name {
        "orientation" => (if env.height >= env.width { "portrait" } else { "landscape" }, true),
        "hover" | "any-hover" => (if env.hover { "hover" } else { "none" }, env.hover),
        "pointer" | "any-pointer" => (env.pointer, env.pointer != "none"),
        "prefers-color-scheme" => (if env.dark { "dark" } else { "light" }, true),
        "prefers-reduced-motion" => (if env.reduced_motion { "reduce" } else { "no-preference" }, env.reduced_motion),
        "prefers-contrast" => ("no-preference", false),
        "prefers-reduced-transparency" | "prefers-reduced-data" => ("no-preference", false),
        "forced-colors" => ("none", false),
        "inverted-colors" => ("none", false),
        "scripting" => (env.scripting, env.scripting != "none"),
        "update" => (if env.media_type == "print" { "none" } else { "fast" }, env.media_type != "print"),
        "overflow-block" => (if env.media_type == "print" { "paged" } else { "scroll" }, true),
        "overflow-inline" => ("scroll", true),
        "display-mode" => ("browser", true),
        "color-gamut" => ("srgb", true),
        "dynamic-range" | "video-dynamic-range" => ("standard", true),
        "grid" => ("0", false),
        _ => return None,
    })
}

impl MediaFeature {
    pub fn evaluate(&self, env: &Environment) -> Tri {
        match self {
            MediaFeature::Boolean(n) => {
                if let Some(v) = range_value(n, env) {
                    Tri::from(v != 0.0)
                } else if let Some((_, b)) = discrete(n, env) {
                    Tri::from(b)
                } else {
                    Tri::Unknown
                }
            }
            MediaFeature::Plain(n, v) => {
                let (op, base) = if let Some(b) = n.strip_prefix("min-") {
                    (RangeOp::Ge, b)
                } else if let Some(b) = n.strip_prefix("max-") {
                    (RangeOp::Le, b)
                } else {
                    (RangeOp::Eq, n.as_str())
                };
                if let Some(cur) = range_value(base, env) {
                    match canonical(base, v, env) {
                        Some(x) => Tri::from(compare(cur, op, x)),
                        None => Tri::Unknown,
                    }
                } else if op == RangeOp::Eq {
                    match (discrete(base, env), v) {
                        (Some((k, _)), MfValue::Ident(i)) => Tri::from(k == i.as_str()),
                        (Some(("0", _)), MfValue::Number(x)) => Tri::from(*x == 0.0),
                        _ => Tri::Unknown,
                    }
                } else {
                    Tri::Unknown
                }
            }
            MediaFeature::Range(n, cons) => {
                let Some(cur) = range_value(n, env) else { return Tri::Unknown };
                let mut r = Tri::True;
                for (op, v) in cons {
                    match canonical(n, v, env) {
                        Some(x) => {
                            if !compare(cur, *op, x) {
                                r = Tri::False;
                            }
                        }
                        None => return Tri::Unknown,
                    }
                }
                r
            }
        }
    }
}
