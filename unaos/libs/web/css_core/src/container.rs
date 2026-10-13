//! AETHERSTYLE3 (SR103): container queries (CSS Containment Level 3 §4, `@container`). The prelude is an
//! optional `<container-name>` and a `<container-condition>`, the Media Queries grammar ([`crate::media`]'s
//! condition parser) over the size features `width`, `height`, `inline-size`, `block-size`, `aspect-ratio`
//! and `orientation`; `style()` / `scroll-state()` queries are `<general-enclosed>` and evaluate unknown.
//!
//! A rule is evaluated against its element's QUERY CONTAINER — the nearest ancestor whose `container-type` is
//! `size` or `inline-size` and whose `container-name` matches (§3.1) — whose content-box size the caller found
//! by laying the page out once ([`ContainerSize`]). An axis the container does not contain (`inline-size`
//! contains only the inline axis) is unknown, and unknown is false, as in media queries (§4.1). The cascade
//! carries the conditions on each entry (`cascade::RuleEntry::container`) and asks the caller per element
//! (`cascade::cascade_contained`).
//!
//! AETHERSTYLE3B (SR115): `style()` queries (§6.2 container style queries) are parsed into [`StyleQuery`] —
//! `not` / `and` / `or` over `(<declaration>)` style features, a bare `--name` testing that the custom property
//! has a value — and evaluated against the query container's computed custom properties, which the caller
//! hands in ([`ContainerRule::evaluate_with`]). Every element is a style container, so a style-only rule
//! needs no size container ([`ContainerRule::needs_size`]) and no layout. A standard property inside
//! `style()` and `scroll-state()` queries stay unknown (false).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::media::{parse_media_condition, Environment, MediaCondition, MediaFeature, Tri};
use crate::parser::{BlockKind, CV};
use crate::tokenizer::Token;

/// One `@container` prelude.
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerRule {
    /// The container name the rule queries (`None`: any query container).
    pub name: Option<String>,
    /// The condition; `None` when it does not parse (the rule never applies).
    pub condition: Option<ContainerCondition>,
}

/// A `<container-condition>`: size features (the Media Queries grammar), `style()` queries, and the boolean
/// combinators over them.
#[derive(Clone, Debug, PartialEq)]
pub enum ContainerCondition {
    Not(Box<ContainerCondition>),
    And(Vec<ContainerCondition>),
    Or(Vec<ContainerCondition>),
    /// `( <size-feature> )` or a parenthesized media-style condition over size features.
    Size(MediaCondition),
    /// `style( <style-query> )`.
    Style(StyleQuery),
    /// `<general-enclosed>` (`scroll-state()`, anything else): unknown.
    Unknown,
}

/// A `<style-query>`.
#[derive(Clone, Debug, PartialEq)]
pub enum StyleQuery {
    Not(Box<StyleQuery>),
    And(Vec<StyleQuery>),
    Or(Vec<StyleQuery>),
    /// A style feature: the property and the value it must compute to (None: `style(--x)`, any value).
    Feature(String, Option<String>),
    Unknown,
}

/// What a query container offers: its content-box size per contained axis (px), whether its writing mode
/// is vertical (inline-size is then the height), and its font size (em in the query).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContainerSize {
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub vertical: bool,
    pub font_size: f64,
}

/// `[ <container-name> ]? <container-condition>`.
pub fn parse_container_prelude(prelude: &[CV]) -> ContainerRule {
    let mut i = 0;
    while i < prelude.len() && prelude[i].is_whitespace() {
        i += 1;
    }
    let mut name = None;
    if let Some(CV::Token(Token::Ident(n))) = prelude.get(i) {
        let l = n.to_ascii_lowercase();
        if !matches!(l.as_str(), "not" | "and" | "or" | "none" | "initial" | "inherit" | "unset" | "default") {
            name = Some(n.clone());
            i += 1;
        }
    }
    let w: Vec<&CV> = prelude[i..].iter().filter(|c| !c.is_whitespace()).collect();
    ContainerRule { name, condition: condition(&w) }
}

fn is_ident(c: &CV, word: &str) -> bool {
    matches!(c.ident(), Some(n) if n.eq_ignore_ascii_case(word))
}

/// `not <query-in-parens> | <query-in-parens> [ and <query-in-parens> ]* | … [ or … ]*`.
fn condition(w: &[&CV]) -> Option<ContainerCondition> {
    if w.is_empty() {
        return None;
    }
    if is_ident(w[0], "not") {
        return (w.len() == 2).then(|| ContainerCondition::Not(Box::new(in_parens(w[1]))));
    }
    let first = in_parens(w[0]);
    if w.len() == 1 {
        return Some(first);
    }
    let op = if is_ident(w[1], "and") { "and" } else if is_ident(w[1], "or") { "or" } else { return None };
    let mut items = alloc::vec![first];
    let mut i = 1;
    while i < w.len() {
        if !is_ident(w[i], op) {
            return None;
        }
        items.push(in_parens(w.get(i + 1)?));
        i += 2;
    }
    Some(if op == "and" { ContainerCondition::And(items) } else { ContainerCondition::Or(items) })
}

fn in_parens(c: &CV) -> ContainerCondition {
    match c {
        CV::Function(name, args) if name.eq_ignore_ascii_case("style") => ContainerCondition::Style(style_query(args)),
        CV::Block(BlockKind::Paren, inner) => {
            let w: Vec<&CV> = inner.iter().filter(|c| !c.is_whitespace()).collect();
            let nested = w.first().is_some_and(|f| matches!(f, CV::Block(BlockKind::Paren, _) | CV::Function(..)) || is_ident(f, "not"));
            if nested {
                condition(&w).unwrap_or(ContainerCondition::Unknown)
            } else {
                parse_media_condition(core::slice::from_ref(c)).map(ContainerCondition::Size).unwrap_or(ContainerCondition::Unknown)
            }
        }
        _ => ContainerCondition::Unknown,
    }
}

/// `<style-query>`: `not <style-in-parens>`, `<style-in-parens>` joined by one of `and` / `or`, or one
/// `<style-feature>` (a declaration).
fn style_query(args: &[CV]) -> StyleQuery {
    let w: Vec<&CV> = args.iter().filter(|c| !c.is_whitespace()).collect();
    if w.is_empty() {
        return StyleQuery::Unknown;
    }
    if is_ident(w[0], "not") {
        return if w.len() == 2 { StyleQuery::Not(Box::new(style_in_parens(w[1]))) } else { StyleQuery::Unknown };
    }
    if !matches!(w[0], CV::Block(BlockKind::Paren, _)) {
        return style_feature(args);
    }
    let first = style_in_parens(w[0]);
    if w.len() == 1 {
        return first;
    }
    let op = if is_ident(w[1], "and") { "and" } else if is_ident(w[1], "or") { "or" } else { return StyleQuery::Unknown };
    let mut items = alloc::vec![first];
    let mut i = 1;
    while i < w.len() {
        match (is_ident(w[i], op), w.get(i + 1)) {
            (true, Some(n)) => items.push(style_in_parens(n)),
            _ => return StyleQuery::Unknown,
        }
        i += 2;
    }
    if op == "and" { StyleQuery::And(items) } else { StyleQuery::Or(items) }
}

fn style_in_parens(c: &CV) -> StyleQuery {
    match c {
        CV::Block(BlockKind::Paren, inner) => {
            let w: Vec<&CV> = inner.iter().filter(|c| !c.is_whitespace()).collect();
            if w.first().is_some_and(|f| matches!(f, CV::Block(BlockKind::Paren, _)) || is_ident(f, "not")) {
                style_query(inner)
            } else {
                style_feature(inner)
            }
        }
        _ => StyleQuery::Unknown,
    }
}

/// `<style-feature>`: `<name> : <value>` or a bare `<name>`.
fn style_feature(v: &[CV]) -> StyleQuery {
    let v = crate::parser::trim_ws(v);
    let Some(name) = v.first().and_then(|c| c.ident()) else { return StyleQuery::Unknown };
    let rest = crate::parser::trim_ws(&v[1..]);
    if rest.is_empty() {
        return StyleQuery::Feature(String::from(name), None);
    }
    if !matches!(rest[0], CV::Token(Token::Colon)) {
        return StyleQuery::Unknown;
    }
    let value = crate::serialize::to_css(crate::parser::trim_ws(&rest[1..]));
    StyleQuery::Feature(String::from(name), Some(value))
}

/// Whitespace runs collapsed, ends trimmed: the comparison form of a computed value text.
fn squash(v: &str) -> String {
    let mut out = String::new();
    for part in v.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(part);
    }
    out
}

fn eval_style(q: &StyleQuery, style: &dyn Fn(&str) -> Option<String>) -> Tri {
    match q {
        StyleQuery::Not(q) => not(eval_style(q, style)),
        StyleQuery::And(v) => all(v.iter().map(|q| eval_style(q, style))),
        StyleQuery::Or(v) => any(v.iter().map(|q| eval_style(q, style))),
        StyleQuery::Feature(name, value) => {
            if !name.starts_with("--") {
                return Tri::Unknown;
            }
            let cur = style(name).map(|c| squash(&c)).filter(|c| !c.is_empty());
            let hit = match value {
                None => cur.is_some(),
                Some(v) => cur.as_deref() == Some(squash(v).as_str()),
            };
            if hit { Tri::True } else { Tri::False }
        }
        StyleQuery::Unknown => Tri::Unknown,
    }
}

fn all(it: impl Iterator<Item = Tri>) -> Tri {
    let mut r = Tri::True;
    for t in it {
        match t {
            Tri::False => return Tri::False,
            Tri::Unknown => r = Tri::Unknown,
            Tri::True => {}
        }
    }
    r
}

fn any(it: impl Iterator<Item = Tri>) -> Tri {
    let mut r = Tri::False;
    for t in it {
        match t {
            Tri::True => return Tri::True,
            Tri::Unknown => r = Tri::Unknown,
            Tri::False => {}
        }
    }
    r
}

fn eval_cond(c: &ContainerCondition, size: Option<&ContainerSize>, style: &dyn Fn(&str) -> Option<String>) -> Tri {
    match c {
        ContainerCondition::Not(c) => not(eval_cond(c, size, style)),
        ContainerCondition::And(v) => all(v.iter().map(|c| eval_cond(c, size, style))),
        ContainerCondition::Or(v) => any(v.iter().map(|c| eval_cond(c, size, style))),
        ContainerCondition::Size(m) => size.map(|s| eval(m, s)).unwrap_or(Tri::Unknown),
        ContainerCondition::Style(q) => eval_style(q, style),
        ContainerCondition::Unknown => Tri::Unknown,
    }
}

fn has_size(c: &ContainerCondition) -> bool {
    match c {
        ContainerCondition::Not(c) => has_size(c),
        ContainerCondition::And(v) | ContainerCondition::Or(v) => v.iter().any(has_size),
        ContainerCondition::Size(_) => true,
        _ => false,
    }
}

impl ContainerRule {
    /// The condition against one query container (`Tri::True` applies the rule).
    pub fn evaluate(&self, size: &ContainerSize) -> Tri {
        self.evaluate_with(Some(size), &|_| None)
    }

    /// The condition against one query container: its size (None: not a size container) and its computed
    /// custom properties (`style(name)` → the value text, None when the property has no value).
    pub fn evaluate_with(&self, size: Option<&ContainerSize>, style: &dyn Fn(&str) -> Option<String>) -> Tri {
        match &self.condition {
            Some(c) => eval_cond(c, size, style),
            None => Tri::False,
        }
    }

    /// The condition queries size features: its query container must be a size container (§3.1).
    pub fn needs_size(&self) -> bool {
        self.condition.as_ref().is_some_and(has_size)
    }

    /// The rule names this container (no name: any).
    pub fn names(&self, names: &[String]) -> bool {
        match &self.name {
            None => true,
            Some(n) => names.iter().any(|m| m == n),
        }
    }
}

fn not(t: Tri) -> Tri {
    match t {
        Tri::True => Tri::False,
        Tri::False => Tri::True,
        Tri::Unknown => Tri::Unknown,
    }
}

fn eval(c: &MediaCondition, size: &ContainerSize) -> Tri {
    match c {
        MediaCondition::Not(c) => not(eval(c, size)),
        MediaCondition::And(v) => {
            let mut r = Tri::True;
            for c in v {
                match eval(c, size) {
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
                match eval(c, size) {
                    Tri::True => return Tri::True,
                    Tri::Unknown => r = Tri::Unknown,
                    Tri::False => {}
                }
            }
            r
        }
        MediaCondition::Feature(f) => feature(f, size),
        MediaCondition::Unknown => Tri::Unknown,
    }
}

/// The physical feature a container size feature reads, and which axes it needs: (name, needs width, needs
/// height).
fn physical(base: &str, vertical: bool) -> Option<(&'static str, bool, bool)> {
    Some(match base {
        "width" => ("width", true, false),
        "height" => ("height", false, true),
        "inline-size" => if vertical { ("height", false, true) } else { ("width", true, false) },
        "block-size" => if vertical { ("width", true, false) } else { ("height", false, true) },
        "aspect-ratio" => ("aspect-ratio", true, true),
        "orientation" => ("orientation", true, true),
        _ => return None,
    })
}

fn feature(f: &MediaFeature, size: &ContainerSize) -> Tri {
    let name = match f {
        MediaFeature::Boolean(n) | MediaFeature::Plain(n, _) | MediaFeature::Range(n, _) => n.as_str(),
    };
    let (prefix, base) = if let Some(b) = name.strip_prefix("min-") {
        ("min-", b)
    } else if let Some(b) = name.strip_prefix("max-") {
        ("max-", b)
    } else {
        ("", name)
    };
    // `orientation` and the range form have no min-/max- variants
    if !prefix.is_empty() && (base == "orientation" || matches!(f, MediaFeature::Range(..))) {
        return Tri::Unknown;
    }
    let Some((phys, need_w, need_h)) = physical(base, size.vertical) else { return Tri::Unknown };
    if (need_w && size.width.is_none()) || (need_h && size.height.is_none()) {
        return Tri::Unknown;
    }
    let env = Environment {
        width: size.width.unwrap_or(0.0),
        height: size.height.unwrap_or(0.0),
        font_size: size.font_size,
        ..Environment::default()
    };
    let mut full = String::from(prefix);
    full.push_str(phys);
    let mapped = match f {
        MediaFeature::Boolean(_) => MediaFeature::Boolean(full),
        MediaFeature::Plain(_, v) => MediaFeature::Plain(full, v.clone()),
        MediaFeature::Range(_, cons) => MediaFeature::Range(full, cons.clone()),
    };
    mapped.evaluate(&env)
}
