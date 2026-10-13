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

use alloc::string::String;

use crate::media::{parse_media_condition, Environment, MediaCondition, MediaFeature, Tri};
use crate::parser::CV;
use crate::tokenizer::Token;

/// One `@container` prelude.
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerRule {
    /// The container name the rule queries (`None`: any query container).
    pub name: Option<String>,
    /// The condition; `None` when it does not parse (the rule never applies).
    pub condition: Option<MediaCondition>,
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
    ContainerRule { name, condition: parse_media_condition(&prelude[i..]) }
}

impl ContainerRule {
    /// The condition against one query container (`Tri::True` applies the rule).
    pub fn evaluate(&self, size: &ContainerSize) -> Tri {
        match &self.condition {
            Some(c) => eval(c, size),
            None => Tri::False,
        }
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
