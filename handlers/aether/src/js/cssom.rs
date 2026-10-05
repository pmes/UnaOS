//! CSSOM (§6) and CSSOM View over Aether's style and layout.
//!
//! - `element.style`: a CSSStyleDeclaration whose declaration block is the element's `style`
//!   attribute, parsed by css_core and re-serialized after every mutation (CSSOM §6.7: "update style
//!   attribute"). Values are normalized the way Blink serializes specified values (hex/functional colors
//!   to `rgb()`/`rgba()`, keywords lowercase, unitless zero lengths to `0px`, numbers shortest), and the
//!   common shorthands expand to longhands and recompose on serialization.
//! - `getComputedStyle`: the 20 properties AETHERSTYLE's `computed_report` computes from Aether's
//!   cascade + layout (the values its renderer actually uses); any other property reads its inline or
//!   initial value and is ledgered.
//! - geometry (`getBoundingClientRect`, `getClientRects`, `offset*`, `client*`, `scroll*`): from the
//!   laid-out tree — taffy boxes for block-level boxes, AETHERINLINE's line fragments for inline boxes
//!   (one rect per fragment), atomic inlines at their fragment position. Layout is flushed (rebuilt
//!   against the current DOM and stylesheets) when the DOM changed since the last query, exactly when a
//!   browser would force a style/layout update.

use super::dom::{self, attr_value, iface, register_iface, set_attr, this_element, with_doc};
use super::idl::*;
use super::page;
use css_core::{ComponentValue as CV, Token};
use html_core::NodeId;
use js_core::vm::*;
use std::collections::HashMap;

// =================================================================================================
// Properties
// =================================================================================================

/// The CSS properties CSSStyleDeclaration exposes as attributes (camelCase and dashed).
pub const PROPERTIES: &[&str] = &[
    "align-content", "align-items", "align-self", "animation", "animation-delay", "animation-direction",
    "animation-duration", "animation-fill-mode", "animation-iteration-count", "animation-name",
    "animation-play-state", "animation-timing-function", "appearance", "aspect-ratio", "backdrop-filter",
    "backface-visibility", "background", "background-attachment", "background-blend-mode", "background-clip",
    "background-color", "background-image", "background-origin", "background-position", "background-position-x",
    "background-position-y", "background-repeat", "background-size", "block-size", "border", "border-block",
    "border-bottom", "border-bottom-color", "border-bottom-left-radius", "border-bottom-right-radius",
    "border-bottom-style", "border-bottom-width", "border-collapse", "border-color", "border-image",
    "border-inline", "border-left", "border-left-color", "border-left-style", "border-left-width",
    "border-radius", "border-right", "border-right-color", "border-right-style", "border-right-width",
    "border-spacing", "border-style", "border-top", "border-top-color", "border-top-left-radius",
    "border-top-right-radius", "border-top-style", "border-top-width", "border-width", "bottom", "box-shadow",
    "box-sizing", "break-after", "break-before", "break-inside", "caption-side", "caret-color", "clear", "clip",
    "clip-path", "color", "column-count", "column-gap", "column-rule", "column-width", "columns", "contain",
    "content", "counter-increment", "counter-reset", "cursor", "direction", "display", "empty-cells", "fill",
    "filter", "flex", "flex-basis", "flex-direction", "flex-flow", "flex-grow", "flex-shrink", "flex-wrap",
    "float", "font", "font-family", "font-feature-settings", "font-kerning", "font-size", "font-stretch",
    "font-style", "font-variant", "font-weight", "gap", "grid", "grid-area", "grid-auto-columns",
    "grid-auto-flow", "grid-auto-rows", "grid-column", "grid-column-end", "grid-column-start", "grid-row",
    "grid-row-end", "grid-row-start", "grid-template", "grid-template-areas", "grid-template-columns",
    "grid-template-rows", "height", "hyphens", "image-rendering", "inline-size", "inset", "isolation",
    "justify-content", "justify-items", "justify-self", "left", "letter-spacing", "line-height", "list-style",
    "list-style-image", "list-style-position", "list-style-type", "margin", "margin-block", "margin-bottom",
    "margin-inline", "margin-left", "margin-right", "margin-top", "mask", "mask-image", "max-block-size",
    "max-height", "max-inline-size", "max-width", "min-block-size", "min-height", "min-inline-size", "min-width",
    "mix-blend-mode", "object-fit", "object-position", "opacity", "order", "orphans", "outline", "outline-color",
    "outline-offset", "outline-style", "outline-width", "overflow", "overflow-wrap", "overflow-x", "overflow-y",
    "padding", "padding-block", "padding-bottom", "padding-inline", "padding-left", "padding-right",
    "padding-top", "page-break-after", "page-break-before", "page-break-inside", "perspective",
    "perspective-origin", "place-content", "place-items", "place-self", "pointer-events", "position", "quotes",
    "resize", "right", "rotate", "row-gap", "scale", "scroll-behavior", "stroke", "stroke-width", "tab-size",
    "table-layout", "text-align", "text-align-last", "text-decoration", "text-decoration-color",
    "text-decoration-line", "text-decoration-style", "text-decoration-thickness", "text-indent",
    "text-overflow", "text-rendering", "text-shadow", "text-transform", "text-underline-offset", "top",
    "touch-action", "transform", "transform-origin", "transform-style", "transition", "transition-delay",
    "transition-duration", "transition-property", "transition-timing-function", "translate", "unicode-bidi",
    "user-select", "vertical-align", "visibility", "white-space", "widows", "width", "will-change",
    "word-break", "word-spacing", "word-wrap", "writing-mode", "z-index", "zoom",
];

fn camel(dashed: &str) -> String {
    if dashed == "float" {
        return "cssFloat".into();
    }
    let mut out = String::new();
    let mut up = false;
    for c in dashed.chars() {
        if c == '-' {
            up = true;
        } else if up {
            out.push(c.to_ascii_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn is_color_prop(p: &str) -> bool {
    matches!(
        p,
        "color" | "background-color" | "border-top-color" | "border-right-color" | "border-bottom-color"
            | "border-left-color" | "outline-color" | "text-decoration-color" | "caret-color" | "column-rule-color"
            | "fill" | "stroke" | "accent-color"
    )
}

fn is_length_prop(p: &str) -> bool {
    matches!(
        p,
        "width" | "height" | "min-width" | "min-height" | "max-width" | "max-height" | "margin-top" | "margin-right"
            | "margin-bottom" | "margin-left" | "padding-top" | "padding-right" | "padding-bottom" | "padding-left"
            | "top" | "right" | "bottom" | "left" | "border-top-width" | "border-right-width" | "border-bottom-width"
            | "border-left-width" | "font-size" | "letter-spacing" | "word-spacing" | "text-indent" | "row-gap"
            | "column-gap" | "outline-width" | "outline-offset" | "flex-basis" | "border-top-left-radius"
            | "border-top-right-radius" | "border-bottom-right-radius" | "border-bottom-left-radius"
            | "inline-size" | "block-size"
    )
}

fn length_keywords(p: &str) -> &'static [&'static str] {
    match p {
        "width" | "height" | "inline-size" | "block-size" | "min-width" | "min-height" | "flex-basis" => {
            &["auto", "min-content", "max-content", "fit-content", "content"]
        }
        "max-width" | "max-height" => &["none", "min-content", "max-content", "fit-content"],
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" | "top" | "right" | "bottom" | "left" => &["auto"],
        "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width" | "outline-width" => {
            &["thin", "medium", "thick"]
        }
        "font-size" => &["xx-small", "x-small", "small", "medium", "large", "x-large", "xx-large", "xxx-large", "larger", "smaller", "math"],
        "letter-spacing" | "word-spacing" | "row-gap" | "column-gap" => &["normal"],
        _ => &[],
    }
}

/// Properties whose identifiers are author-defined (case kept).
fn keeps_ident_case(p: &str) -> bool {
    matches!(
        p,
        "font-family" | "font" | "animation" | "animation-name" | "transition-property" | "transition" | "grid-area"
            | "grid-template-areas" | "counter-reset" | "counter-increment" | "will-change" | "content" | "quotes"
            | "list-style-type" | "font-feature-settings"
    ) || p.starts_with("--")
}

const CSS_WIDE: &[&str] = &["initial", "inherit", "unset", "revert", "revert-layer"];

fn trim(v: &[CV]) -> &[CV] {
    css_core::parser::trim_ws(v)
}

fn ident_of(cv: &CV) -> Option<&str> {
    match cv {
        CV::Token(Token::Ident(s)) => Some(s),
        _ => None,
    }
}

fn has_var(v: &[CV]) -> bool {
    css_core::values::contains_var(v)
}

/// Serializes component values with Blink's spacing: one space between tokens, `a, b` around commas,
/// identifiers lowercased unless the property keeps them.
fn serialize_generic(prop: &str, v: &[CV]) -> String {
    let lower = !keeps_ident_case(prop);
    let mut out = String::new();
    fn walk(v: &[CV], out: &mut String, lower: bool, prop: &str) {
        for cv in v {
            match cv {
                CV::Token(Token::Whitespace) => {
                    if !out.ends_with(' ') && !out.ends_with('(') && !out.is_empty() {
                        out.push(' ');
                    }
                }
                CV::Token(Token::Comma) => {
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    out.push_str(", ");
                }
                CV::Token(Token::Ident(s)) if lower => out.push_str(&s.to_ascii_lowercase()),
                CV::Token(Token::Dimension(n, u)) => {
                    css_core::serialize::number(n, out);
                    out.push_str(&u.to_ascii_lowercase());
                }
                CV::Token(Token::Hash { .. }) if is_color_prop(prop) => {
                    if let Some(c) = css_core::color::parse_color(std::slice::from_ref(cv)) {
                        out.push_str(&color_string(&c));
                    }
                }
                CV::Function(name, args) => {
                    let lname = name.to_ascii_lowercase();
                    if matches!(lname.as_str(), "rgb" | "rgba" | "hsl" | "hsla" | "hwb" | "lab" | "lch" | "oklab" | "oklch" | "color") {
                        if let Some(c) = css_core::color::parse_color(std::slice::from_ref(cv)) {
                            out.push_str(&color_string(&c));
                            continue;
                        }
                    }
                    out.push_str(&if lower { lname } else { name.clone() });
                    out.push('(');
                    let mut inner = String::new();
                    walk(css_core::parser::trim_ws(args), &mut inner, lower, prop);
                    out.push_str(inner.trim_end());
                    out.push(')');
                }
                other => out.push_str(&css_core::serialize::to_css(std::slice::from_ref(other))),
            }
        }
    }
    walk(trim(v), &mut out, lower, prop);
    out.trim().to_string()
}

fn color_string(c: &css_core::color::Color) -> String {
    match c {
        css_core::color::Color::CurrentColor => "currentcolor".into(),
        css_core::color::Color::Rgba(r) => r.serialize(),
    }
}

/// A `<color>` value as Blink stores it: named colors and keywords stay keywords, the rest is `rgb()`.
fn normalize_color(v: &[CV]) -> Option<String> {
    let v = trim(v);
    if let [CV::Token(Token::Ident(s))] = v {
        let l = s.to_ascii_lowercase();
        if l == "currentcolor" || l == "transparent" || css_core::color::named(&l).is_some() {
            return Some(l);
        }
        return None;
    }
    css_core::color::parse_color(v).map(|c| color_string(&c))
}

/// One length-percentage component (a single token or math function).
fn normalize_length_token(prop: &str, cv: &CV) -> Option<String> {
    match cv {
        CV::Token(Token::Number(n)) if n.value == 0.0 => Some("0px".into()),
        CV::Token(Token::Dimension(n, u)) => {
            let u = u.to_ascii_lowercase();
            let ok = matches!(
                u.as_str(),
                "px" | "em" | "rem" | "ex" | "ch" | "cm" | "mm" | "in" | "pt" | "pc" | "q" | "vw" | "vh" | "vmin" | "vmax"
                    | "vi" | "vb" | "svw" | "svh" | "lvw" | "lvh" | "dvw" | "dvh" | "cap" | "ic" | "lh" | "rlh" | "fr"
            );
            if !ok {
                return None;
            }
            let mut s = String::new();
            css_core::serialize::number(n, &mut s);
            s.push_str(&u);
            Some(s)
        }
        CV::Token(Token::Percentage(n)) => {
            let mut s = String::new();
            css_core::serialize::number(n, &mut s);
            s.push('%');
            Some(s)
        }
        CV::Token(Token::Ident(i)) => {
            let l = i.to_ascii_lowercase();
            length_keywords(prop).contains(&l.as_str()).then_some(l)
        }
        CV::Function(name, _) if css_core::values::is_math_function(&name.to_ascii_lowercase()) => {
            Some(serialize_generic(prop, std::slice::from_ref(cv)))
        }
        _ => None,
    }
}

/// Normalizes one longhand value; None when the value is invalid for the property.
pub fn normalize_value(prop: &str, v: &[CV]) -> Option<String> {
    let v = trim(v);
    if v.is_empty() {
        return None;
    }
    if prop.starts_with("--") {
        return Some(css_core::serialize::to_css(v).trim().to_string());
    }
    if has_var(v) {
        return Some(css_core::serialize::to_css(v).trim().to_string());
    }
    if let [CV::Token(Token::Ident(s))] = v {
        let l = s.to_ascii_lowercase();
        if CSS_WIDE.contains(&l.as_str()) {
            return Some(l);
        }
    }
    if is_color_prop(prop) {
        return normalize_color(v);
    }
    if is_length_prop(prop) {
        let parts: Vec<&CV> = v.iter().filter(|c| !c.is_whitespace()).collect();
        if parts.len() != 1 {
            return None;
        }
        return normalize_length_token(prop, parts[0]);
    }
    match prop {
        "opacity" | "flex-grow" | "flex-shrink" | "z-index" | "order" | "orphans" | "widows" => {
            if let [one] = v {
                return match one {
                    CV::Token(Token::Number(n)) => {
                        let mut s = String::new();
                        css_core::serialize::number(n, &mut s);
                        Some(s)
                    }
                    CV::Token(Token::Percentage(n)) if prop == "opacity" => {
                        let mut s = String::new();
                        css_core::serialize::number_f64(n.value / 100.0, &mut s);
                        Some(s)
                    }
                    CV::Token(Token::Ident(i)) if prop == "z-index" && i.eq_ignore_ascii_case("auto") => Some("auto".into()),
                    CV::Function(n, _) if css_core::values::is_math_function(&n.to_ascii_lowercase()) => {
                        Some(serialize_generic(prop, v))
                    }
                    _ => None,
                };
            }
            return None;
        }
        "line-height" => {
            if let [one] = v {
                return match one {
                    CV::Token(Token::Number(n)) => {
                        let mut s = String::new();
                        css_core::serialize::number(n, &mut s);
                        Some(s)
                    }
                    CV::Token(Token::Ident(i)) if i.eq_ignore_ascii_case("normal") => Some("normal".into()),
                    other => normalize_length_token("width", other).filter(|x| x != "auto"),
                };
            }
            return None;
        }
        _ => {}
    }
    Some(serialize_generic(prop, v))
}

// =================================================================================================
// Shorthands
// =================================================================================================

const SIDES: [&str; 4] = ["top", "right", "bottom", "left"];

/// The longhands of a shorthand this binding expands.
fn longhands(sh: &str) -> Option<Vec<String>> {
    let v = |xs: &[&str]| Some(xs.iter().map(|s| s.to_string()).collect::<Vec<String>>());
    match sh {
        "margin" | "padding" => Some(SIDES.iter().map(|s| format!("{sh}-{s}")).collect()),
        "inset" => v(&SIDES),
        "border-width" | "border-style" | "border-color" => {
            let k = &sh[7..];
            Some(SIDES.iter().map(|s| format!("border-{s}-{k}")).collect())
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            Some(["width", "style", "color"].iter().map(|k| format!("{sh}-{k}")).collect())
        }
        "border" => Some(
            ["width", "style", "color"]
                .iter()
                .flat_map(|k| SIDES.iter().map(move |s| format!("border-{s}-{k}")))
                .collect(),
        ),
        "gap" => v(&["row-gap", "column-gap"]),
        "overflow" => v(&["overflow-x", "overflow-y"]),
        "flex" => v(&["flex-grow", "flex-shrink", "flex-basis"]),
        "outline" => v(&["outline-color", "outline-style", "outline-width"]),
        "border-radius" => v(&["border-top-left-radius", "border-top-right-radius", "border-bottom-right-radius", "border-bottom-left-radius"]),
        "background" => v(&[
            "background-image",
            "background-position-x",
            "background-position-y",
            "background-size",
            "background-repeat",
            "background-attachment",
            "background-origin",
            "background-clip",
            "background-color",
        ]),
        _ => None,
    }
}

fn bg_initial(p: &str) -> &'static str {
    match p {
        "background-image" => "none",
        "background-position-x" | "background-position-y" => "0%",
        "background-size" => "auto",
        "background-repeat" => "repeat",
        "background-attachment" => "scroll",
        "background-origin" => "padding-box",
        "background-clip" => "border-box",
        _ => "transparent",
    }
}

fn split_values(v: &[CV]) -> Vec<CV> {
    trim(v).iter().filter(|c| !c.is_whitespace()).cloned().collect()
}

fn four(vals: &[String]) -> Option<[String; 4]> {
    match vals.len() {
        1 => Some([vals[0].clone(), vals[0].clone(), vals[0].clone(), vals[0].clone()]),
        2 => Some([vals[0].clone(), vals[1].clone(), vals[0].clone(), vals[1].clone()]),
        3 => Some([vals[0].clone(), vals[1].clone(), vals[2].clone(), vals[1].clone()]),
        4 => Some([vals[0].clone(), vals[1].clone(), vals[2].clone(), vals[3].clone()]),
        _ => None,
    }
}

const BORDER_STYLES: &[&str] = &["none", "hidden", "dotted", "dashed", "solid", "double", "groove", "ridge", "inset", "outset", "auto"];

/// `<line-width> || <line-style> || <color>` (border sides, outline): (width, style, color).
fn parse_border_side(v: &[CV], width_prop: &str) -> Option<(String, String, String)> {
    let (mut w, mut st, mut c) = (None, None, None);
    for cv in split_values(v) {
        if let Some(i) = ident_of(&cv) {
            let l = i.to_ascii_lowercase();
            if BORDER_STYLES.contains(&l.as_str()) && st.is_none() {
                st = Some(l);
                continue;
            }
        }
        if w.is_none() {
            if let Some(x) = normalize_length_token(width_prop, &cv) {
                w = Some(x);
                continue;
            }
        }
        if c.is_none() {
            if let Some(x) = normalize_color(std::slice::from_ref(&cv)) {
                c = Some(x);
                continue;
            }
        }
        return None;
    }
    if w.is_none() && st.is_none() && c.is_none() {
        return None;
    }
    Some((w.unwrap_or_else(|| "medium".into()), st.unwrap_or_else(|| "none".into()), c.unwrap_or_else(|| "currentcolor".into())))
}

/// Expands `prop: value` into longhand declarations; `Ok(None)` stores it opaquely (unknown
/// shorthand or longhand); `Err` rejects an invalid value.
fn expand(prop: &str, v: &[CV]) -> Result<Vec<(String, String)>, ()> {
    let tv = trim(v);
    if let [CV::Token(Token::Ident(s))] = tv {
        let l = s.to_ascii_lowercase();
        if CSS_WIDE.contains(&l.as_str()) {
            return Ok(match longhands(prop) {
                Some(ls) => ls.into_iter().map(|p| (p, l.clone())).collect(),
                None => vec![(prop.to_string(), l)],
            });
        }
    }
    if has_var(tv) {
        return Ok(vec![(prop.to_string(), css_core::serialize::to_css(tv).trim().to_string())]);
    }
    match prop {
        "margin" | "padding" | "inset" | "border-width" => {
            let lp = match prop {
                "margin" => "margin-top",
                "padding" => "padding-top",
                "inset" => "top",
                _ => "border-top-width",
            };
            let vals: Option<Vec<String>> = split_values(v).iter().map(|c| normalize_length_token(lp, c)).collect();
            let vals = four(&vals.ok_or(())?).ok_or(())?;
            Ok(longhands(prop).unwrap().into_iter().zip(vals).collect())
        }
        "border-style" => {
            let vals: Option<Vec<String>> = split_values(v)
                .iter()
                .map(|c| ident_of(c).map(|i| i.to_ascii_lowercase()).filter(|i| BORDER_STYLES.contains(&i.as_str())))
                .collect();
            let vals = four(&vals.ok_or(())?).ok_or(())?;
            Ok(longhands(prop).unwrap().into_iter().zip(vals).collect())
        }
        "border-color" => {
            let vals: Option<Vec<String>> = split_values(v).iter().map(|c| normalize_color(std::slice::from_ref(c))).collect();
            let vals = four(&vals.ok_or(())?).ok_or(())?;
            Ok(longhands(prop).unwrap().into_iter().zip(vals).collect())
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" | "border" => {
            let (w, s, c) = parse_border_side(v, "border-top-width").ok_or(())?;
            let sides: Vec<&str> = if prop == "border" { SIDES.to_vec() } else { vec![&prop[7..]] };
            let mut out = Vec::new();
            for k in ["width", "style", "color"] {
                for sd in &sides {
                    let val = match k {
                        "width" => w.clone(),
                        "style" => s.clone(),
                        _ => c.clone(),
                    };
                    out.push((format!("border-{sd}-{k}"), val));
                }
            }
            Ok(out)
        }
        "outline" => {
            let (w, s, c) = parse_border_side(v, "outline-width").ok_or(())?;
            Ok(vec![("outline-color".into(), c), ("outline-style".into(), s), ("outline-width".into(), w)])
        }
        "gap" => {
            let vals: Option<Vec<String>> = split_values(v).iter().map(|c| normalize_length_token("row-gap", c)).collect();
            let vals = vals.ok_or(())?;
            match vals.len() {
                1 => Ok(vec![("row-gap".into(), vals[0].clone()), ("column-gap".into(), vals[0].clone())]),
                2 => Ok(vec![("row-gap".into(), vals[0].clone()), ("column-gap".into(), vals[1].clone())]),
                _ => Err(()),
            }
        }
        "overflow" => {
            let vals: Option<Vec<String>> = split_values(v).iter().map(|c| ident_of(c).map(|i| i.to_ascii_lowercase())).collect();
            let vals = vals.ok_or(())?;
            if !vals.iter().all(|x| matches!(x.as_str(), "visible" | "hidden" | "clip" | "scroll" | "auto" | "overlay")) {
                return Err(());
            }
            match vals.len() {
                1 => Ok(vec![("overflow-x".into(), vals[0].clone()), ("overflow-y".into(), vals[0].clone())]),
                2 => Ok(vec![("overflow-x".into(), vals[0].clone()), ("overflow-y".into(), vals[1].clone())]),
                _ => Err(()),
            }
        }
        "flex" => {
            let parts = split_values(v);
            let num_of = |c: &CV| match c {
                CV::Token(Token::Number(n)) => {
                    let mut s = String::new();
                    css_core::serialize::number(n, &mut s);
                    Some(s)
                }
                _ => None,
            };
            let (g, sh, b) = match parts.as_slice() {
                [one] => match ident_of(one).map(|i| i.to_ascii_lowercase()).as_deref() {
                    Some("none") => ("0".to_string(), "0".to_string(), "auto".to_string()),
                    Some("auto") => ("1".into(), "1".into(), "auto".into()),
                    _ => match num_of(one) {
                        Some(n) => (n, "1".into(), "0%".into()),
                        None => ("1".into(), "1".into(), normalize_length_token("flex-basis", one).ok_or(())?),
                    },
                },
                [a, b2] => match (num_of(a), num_of(b2)) {
                    (Some(x), Some(y)) => (x, y, "0%".into()),
                    (Some(x), None) => (x, "1".into(), normalize_length_token("flex-basis", b2).ok_or(())?),
                    _ => return Err(()),
                },
                [a, b2, c] => (num_of(a).ok_or(())?, num_of(b2).ok_or(())?, normalize_length_token("flex-basis", c).ok_or(())?),
                _ => return Err(()),
            };
            Ok(vec![("flex-grow".into(), g), ("flex-shrink".into(), sh), ("flex-basis".into(), b)])
        }
        "border-radius" => {
            let parts = split_values(v);
            if parts.len() != 1 {
                return Ok(vec![(prop.to_string(), serialize_generic(prop, v))]);
            }
            let x = normalize_length_token("border-top-left-radius", &parts[0]).ok_or(())?;
            Ok(longhands(prop).unwrap().into_iter().map(|p| (p, x.clone())).collect())
        }
        "background" => match normalize_color(tv) {
            Some(c) => Ok(longhands("background")
                .unwrap()
                .into_iter()
                .map(|p| {
                    let val = if p == "background-color" { c.clone() } else { bg_initial(&p).to_string() };
                    (p, val)
                })
                .collect()),
            None => Ok(vec![(prop.to_string(), serialize_generic(prop, v))]),
        },
        "word-wrap" => normalize_value("overflow-wrap", v).map(|x| vec![("overflow-wrap".into(), x)]).ok_or(()),
        _ => normalize_value(prop, v).map(|x| vec![(prop.to_string(), x)]).ok_or(()),
    }
}

/// The shorthands that may stand for longhand `p` in serialization, in preference order.
fn shorthands_of(p: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    if p.starts_with("border-") && (p.ends_with("-width") || p.ends_with("-style") || p.ends_with("-color")) && !p.contains("radius") {
        out.push("border");
        if p.ends_with("-width") {
            out.push("border-width");
        } else if p.ends_with("-style") {
            out.push("border-style");
        } else {
            out.push("border-color");
        }
        for s in ["border-top", "border-right", "border-bottom", "border-left"] {
            if p.starts_with(&format!("{s}-")) {
                out.push(s);
            }
        }
    }
    if p.starts_with("margin-") {
        out.push("margin");
    }
    if p.starts_with("padding-") {
        out.push("padding");
    }
    if matches!(p, "top" | "right" | "bottom" | "left") {
        out.push("inset");
    }
    if matches!(p, "row-gap" | "column-gap") {
        out.push("gap");
    }
    if matches!(p, "overflow-x" | "overflow-y") {
        out.push("overflow");
    }
    if matches!(p, "flex-grow" | "flex-shrink" | "flex-basis") {
        out.push("flex");
    }
    if p.starts_with("outline-") && p != "outline-offset" {
        out.push("outline");
    }
    if p.ends_with("-radius") {
        out.push("border-radius");
    }
    if p.starts_with("background-") {
        out.push("background");
    }
    out
}

fn four_serialize(v: [&str; 4]) -> String {
    let [t, r, b, l] = v;
    if t == r && r == b && b == l {
        t.to_string()
    } else if t == b && r == l {
        format!("{t} {r}")
    } else if r == l {
        format!("{t} {r} {b}")
    } else {
        format!("{t} {r} {b} {l}")
    }
}

/// Serializes shorthand `sh` from the block when every longhand is present with one priority.
fn serialize_shorthand(block: &Block, sh: &str) -> Option<(String, bool)> {
    let ls = longhands(sh)?;
    let mut vals = Vec::new();
    let mut imp = None;
    for l in &ls {
        let d = block.decls.iter().find(|d| d.name == *l)?;
        if imp.is_some_and(|i| i != d.important) {
            return None;
        }
        imp = Some(d.important);
        vals.push(d.value.as_str());
    }
    let important = imp.unwrap_or(false);
    // CSS-wide keyword: all longhands equal.
    if CSS_WIDE.contains(&vals[0]) {
        return vals.iter().all(|v| *v == vals[0]).then(|| (vals[0].to_string(), important));
    }
    if vals.iter().any(|v| CSS_WIDE.contains(v)) {
        return None;
    }
    let side = |w: &str, s: &str, c: &str| {
        let mut parts = Vec::new();
        if w != "medium" {
            parts.push(w);
        }
        if s != "none" {
            parts.push(s);
        }
        if c != "currentcolor" {
            parts.push(c);
        }
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join(" ")
        }
    };
    let s = match sh {
        "margin" | "padding" | "inset" | "border-width" | "border-style" | "border-color" => {
            four_serialize([vals[0], vals[1], vals[2], vals[3]])
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" => side(vals[0], vals[1], vals[2]),
        "border" => {
            // widths 0..4, styles 4..8, colors 8..12
            let same = |a: usize| vals[a] == vals[a + 1] && vals[a] == vals[a + 2] && vals[a] == vals[a + 3];
            if !(same(0) && same(4) && same(8)) {
                return None;
            }
            side(vals[0], vals[4], vals[8])
        }
        "gap" | "overflow" => {
            if vals[0] == vals[1] {
                vals[0].to_string()
            } else {
                format!("{} {}", vals[0], vals[1])
            }
        }
        "flex" => format!("{} {} {}", vals[0], vals[1], vals[2]),
        "outline" => side(vals[2], vals[1], vals[0]),
        "border-radius" => {
            if vals.iter().all(|v| *v == vals[0]) {
                vals[0].to_string()
            } else {
                four_serialize([vals[0], vals[1], vals[2], vals[3]])
            }
        }
        "background" => {
            let color = vals[8];
            let rest_initial = ls.iter().zip(vals.iter()).take(8).all(|(l, v)| bg_initial(l) == *v);
            if !rest_initial {
                return None;
            }
            if color == "transparent" { "none".to_string() } else { color.to_string() }
        }
        _ => return None,
    };
    Some((s, important))
}

// =================================================================================================
// Declaration blocks
// =================================================================================================

#[derive(Clone, Debug)]
pub struct Decl {
    pub name: String,
    pub value: String,
    pub important: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Block {
    pub decls: Vec<Decl>,
}

impl Block {
    /// Parses a declaration list (a `style` attribute); invalid declarations are dropped, later ones win.
    pub fn parse(text: &str) -> Block {
        let mut b = Block::default();
        for d in css_core::stylesheet::parse_style_attribute(text) {
            let name = if d.name.starts_with("--") { d.name.clone() } else { d.name.to_ascii_lowercase() };
            b.set(&name, &d.value, d.important);
        }
        b
    }

    /// CSSOM "set a CSS declaration" (expanding shorthands). Returns false for an invalid value.
    pub fn set(&mut self, name: &str, value: &[CV], important: bool) -> bool {
        let Ok(pairs) = expand(name, value) else { return false };
        for (n, v) in pairs {
            match self.decls.iter_mut().find(|d| d.name == n) {
                Some(d) => {
                    d.value = v;
                    d.important = important;
                }
                None => self.decls.push(Decl { name: n, value: v, important }),
            }
        }
        true
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let targets: Vec<String> = longhands(name).unwrap_or_else(|| vec![name.to_string()]);
        let before = self.decls.len();
        self.decls.retain(|d| !targets.contains(&d.name) && d.name != name);
        self.decls.len() != before
    }

    /// getPropertyValue: a longhand's value, or a shorthand's serialization.
    pub fn get(&self, name: &str) -> String {
        if let Some(d) = self.decls.iter().find(|d| d.name == name) {
            return d.value.clone();
        }
        if longhands(name).is_some() {
            return serialize_shorthand(self, name).map(|x| x.0).unwrap_or_default();
        }
        String::new()
    }

    pub fn priority(&self, name: &str) -> bool {
        if let Some(d) = self.decls.iter().find(|d| d.name == name) {
            return d.important;
        }
        if longhands(name).is_some() {
            return serialize_shorthand(self, name).is_some_and(|x| x.1);
        }
        false
    }

    /// CSSOM "serialize a CSS declaration block" (shorthands recomposed at their first longhand).
    pub fn serialize(&self) -> String {
        let mut out: Vec<String> = Vec::new();
        let mut done: Vec<String> = Vec::new();
        for d in &self.decls {
            if done.contains(&d.name) {
                continue;
            }
            let mut emitted = false;
            for sh in shorthands_of(&d.name) {
                if let Some((v, imp)) = serialize_shorthand(self, sh) {
                    out.push(format!("{sh}: {v}{};", if imp { " !important" } else { "" }));
                    done.extend(longhands(sh).unwrap());
                    emitted = true;
                    break;
                }
            }
            if !emitted {
                out.push(format!("{}: {}{};", d.name, d.value, if d.important { " !important" } else { "" }));
                done.push(d.name.clone());
            }
        }
        out.join(" ")
    }
}

/// The inline declaration block of `el`.
pub fn inline_block(el: usize) -> Block {
    Block::parse(&attr_value(el, "style").unwrap_or_default())
}

fn store_block(vm: &mut Vm, el: usize, b: &Block) {
    set_attr(vm, el, "style", &b.serialize());
}

/// Maps an attribute name used on CSSStyleDeclaration (camel, webkit-cased, dashed) to a property.
fn prop_from_attr(name: &str) -> String {
    if name == "cssFloat" {
        return "float".into();
    }
    if name.contains('-') || name.starts_with("--") {
        return name.to_ascii_lowercase();
    }
    let mut out = String::new();
    let (name, prefix) = match name.strip_prefix("webkit") {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_uppercase()) => (rest, "-webkit-"),
        _ => (name, ""),
    };
    out.push_str(prefix);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

// =================================================================================================
// CSSStyleDeclaration natives
// =================================================================================================

enum Decl_ {
    Inline(usize),
    Computed(usize),
}

fn this_decl(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Decl_> {
    if let Some(o) = this_tagged(vm, &ctx.this, T_STYLE) {
        return Ok(Decl_::Inline(slot_num(vm, o, 0) as usize));
    }
    if let Some(o) = this_tagged(vm, &ctx.this, T_COMPSTYLE) {
        return Ok(Decl_::Computed(slot_num(vm, o, 0) as usize));
    }
    vm.throw_type("Illegal invocation")
}

fn read_prop(d: &Decl_, prop: &str) -> String {
    match d {
        Decl_::Inline(el) => inline_block(*el).get(prop),
        Decl_::Computed(el) => computed_value(*el, prop),
    }
}

fn write_prop(vm: &mut Vm, d: &Decl_, prop: &str, value: &str, important: bool) -> JsResult<()> {
    let Decl_::Inline(el) = d else {
        return throw_dom(vm, "NoModificationAllowedError", &format!("Failed to set the '{prop}' property on 'CSSStyleDeclaration': These styles are computed, and therefore the '{prop}' property is read-only."));
    };
    let mut b = inline_block(*el);
    if value.trim().is_empty() {
        b.remove(prop);
    } else {
        let cvs = css_core::parser::parse_component_values(value);
        if !b.set(prop, &cvs, important) {
            return Ok(()); // invalid values are ignored
        }
    }
    store_block(vm, *el, &b);
    Ok(())
}

fn prop_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_decl(vm, ctx)?;
    let prop = prop_from_attr(&callee_str(vm, ctx));
    Ok(s(&read_prop(&d, &prop)))
}

fn prop_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_decl(vm, ctx)?;
    let prop = prop_from_attr(&callee_str(vm, ctx));
    let v = string_null_empty(vm, &arg(vm, ctx, 0))?;
    write_prop(vm, &d, &prop, &v, false)?;
    Ok(Value::Undefined)
}

fn get_property_value(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_decl(vm, ctx)?;
    need(vm, ctx, 1, "CSSStyleDeclaration", "getPropertyValue")?;
    let p = string(vm, &arg(vm, ctx, 0))?;
    let p = if p.starts_with("--") { p } else { p.to_ascii_lowercase() };
    Ok(s(&read_prop(&d, &p)))
}

fn get_property_priority(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_decl(vm, ctx)?;
    need(vm, ctx, 1, "CSSStyleDeclaration", "getPropertyPriority")?;
    let p = string(vm, &arg(vm, ctx, 0))?.to_ascii_lowercase();
    Ok(s(match d {
        Decl_::Inline(el) if inline_block(el).priority(&p) => "important",
        _ => "",
    }))
}

fn set_property(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_decl(vm, ctx)?;
    need(vm, ctx, 2, "CSSStyleDeclaration", "setProperty")?;
    let p = string(vm, &arg(vm, ctx, 0))?;
    let p = if p.starts_with("--") { p } else { p.to_ascii_lowercase() };
    let v = string_null_empty(vm, &arg(vm, ctx, 1))?;
    let prio = arg(vm, ctx, 2);
    let prio = if prio.is_undefined() { String::new() } else { string(vm, &prio)? };
    if !prio.is_empty() && !prio.eq_ignore_ascii_case("important") {
        return Ok(Value::Undefined);
    }
    write_prop(vm, &d, &p, &v, !prio.is_empty())?;
    Ok(Value::Undefined)
}

fn remove_property(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_decl(vm, ctx)?;
    need(vm, ctx, 1, "CSSStyleDeclaration", "removeProperty")?;
    let p = string(vm, &arg(vm, ctx, 0))?;
    let p = if p.starts_with("--") { p } else { p.to_ascii_lowercase() };
    let Decl_::Inline(el) = d else {
        return throw_dom(vm, "NoModificationAllowedError", "These styles are computed, and therefore read-only.");
    };
    let mut b = inline_block(el);
    let old = b.get(&p);
    if b.remove(&p) {
        store_block(vm, el, &b);
    }
    Ok(s(&old))
}

fn css_text_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_decl(vm, ctx)?;
    Ok(s(&match d {
        Decl_::Inline(el) => inline_block(el).serialize(),
        Decl_::Computed(_) => String::new(),
    }))
}

fn css_text_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_decl(vm, ctx)?;
    let v = string_null_empty(vm, &arg(vm, ctx, 0))?;
    let Decl_::Inline(el) = d else {
        return throw_dom(vm, "NoModificationAllowedError", "These styles are computed, and therefore read-only.");
    };
    let b = Block::parse(&v);
    store_block(vm, el, &b);
    Ok(Value::Undefined)
}

fn decl_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_decl(vm, ctx)?;
    Ok(num(match d {
        Decl_::Inline(el) => inline_block(el).decls.len(),
        Decl_::Computed(_) => crate::css::REPORT_PROPS.len(),
    } as f64))
}

fn decl_item(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_decl(vm, ctx)?;
    need(vm, ctx, 1, "CSSStyleDeclaration", "item")?;
    let i = unsigned_long(vm, &arg(vm, ctx, 0))? as usize;
    Ok(s(&match d {
        Decl_::Inline(el) => inline_block(el).decls.get(i).map(|d| d.name.clone()).unwrap_or_default(),
        Decl_::Computed(_) => crate::css::REPORT_PROPS.get(i).map(|x| x.to_string()).unwrap_or_default(),
    }))
}

fn parent_rule(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_decl(vm, ctx)?;
    Ok(Value::Null)
}

/// `element.style` ([SameObject], [PutForwards=cssText]).
fn style_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(dom::cached(vm, el, dom::SO_STYLE, |vm| {
        let p = iface("CSSStyleDeclaration").unwrap().proto;
        host_obj(vm, p, T_STYLE, vec![num(el as f64)])
    }))
}

fn style_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string_null_empty(vm, &arg(vm, ctx, 0))?;
    let b = Block::parse(&v);
    store_block(vm, el, &b);
    Ok(Value::Undefined)
}

/// `getComputedStyle(element, pseudo)`.
pub fn get_computed_style(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    need(vm, ctx, 1, "Window", "getComputedStyle")?;
    let v = arg(vm, ctx, 0);
    let el = match dom::node_of(vm, &v) {
        Some(n) if dom::kind(n) == dom::NK::Element => n,
        _ => return vm.throw_type("Failed to execute 'getComputedStyle' on 'Window': parameter 1 is not of type 'Element'."),
    };
    let p = iface("CSSStyleDeclaration").unwrap().proto;
    Ok(Value::Object(host_obj(vm, p, T_COMPSTYLE, vec![num(el as f64)])))
}

// =================================================================================================
// Layout flush and computed values
// =================================================================================================

/// The page's stylesheets as the loader applies them: the document's `<style>` elements (now, in
/// tree order) then the external sheets.
fn current_sheets() -> Vec<String> {
    let doc = dom::main_doc();
    let mut sheets: Vec<String> = with_doc(|d| {
        d.descendants(NodeId(doc))
            .filter(|i| d.element(*i).is_some_and(|e| e.is_html("style")))
            .map(|i| d.text_content(i))
            .collect()
    });
    sheets.extend(page(|p| p.external_css.clone()));
    sheets
}

struct Flushed {
    version: u64,
    tree: crate::layout::LayoutTree,
    report: HashMap<NodeId, [String; 20]>,
    boxes: HashMap<NodeId, Vec<(f32, f32, f32, f32)>>,
}

thread_local! {
    static FLUSHED: std::cell::RefCell<Option<Flushed>> = const { std::cell::RefCell::new(None) };
}

pub fn reset() {
    FLUSHED.with(|f| *f.borrow_mut() = None);
}

/// Runs `f` over an up-to-date layout of the main document (rebuilt when the DOM changed).
fn with_layout<R>(f: impl FnOnce(&Flushed) -> R) -> R {
    let version = page(|p| p.version);
    let cached = FLUSHED.with(|c| c.borrow_mut().take());
    let flushed = match cached {
        Some(c) if c.version == version => c,
        _ => {
            let (w, h) = page(|p| p.viewport);
            let doc = dom::node_ref(dom::main_doc());
            let sheets = current_sheets();
            let mut tree = crate::layout::build_tree(&doc, w, h);
            crate::css::apply_stylesheets(&mut tree, &sheets);
            let report = crate::css::computed_report(&tree);
            let boxes = element_boxes(&tree);
            Flushed { version, tree, report, boxes }
        }
    };
    let r = f(&flushed);
    FLUSHED.with(|c| *c.borrow_mut() = Some(flushed));
    r
}

/// Every element's border-box rects in document coordinates: one rect for a block-level or atomic box,
/// one per line fragment for an inline box.
fn element_boxes(tree: &crate::layout::LayoutTree) -> HashMap<NodeId, Vec<(f32, f32, f32, f32)>> {
    use crate::layout::inline::Frag;
    let mut out: HashMap<NodeId, Vec<(f32, f32, f32, f32)>> = HashMap::new();
    let mut work: Vec<(taffy::NodeId, f32, f32)> = vec![(tree.root_node, 0.0, 0.0)];
    let elem_of = |t: taffy::NodeId| tree.node_map.get(&t).filter(|n| n.is_element()).map(|n| n.id());
    // The root's own location.
    if let Ok(l) = tree.taffy.layout(tree.root_node) {
        work[0] = (tree.root_node, l.location.x, l.location.y);
    }
    while let Some((t, x, y)) = work.pop() {
        let Ok(l) = tree.taffy.layout(t) else { continue };
        if let Some(id) = elem_of(t) {
            out.entry(id).or_default().push((x, y, l.size.width, l.size.height));
        }
        if let Some(il) = tree.inline.get(&t) {
            let cx = x + l.border.left + l.padding.left;
            let cy = y + l.border.top + l.padding.top;
            for fr in &il.frags {
                match fr {
                    Frag::Box { node, x: fx, y: fy, w, h, .. } => {
                        if let Some(id) = elem_of(*node) {
                            out.entry(id).or_default().push((cx + fx, cy + fy, *w, *h));
                        }
                    }
                    Frag::Atomic { node, x: fx, y: fy } => work.push((*node, cx + fx, cy + fy)),
                    Frag::Text { .. } => {}
                }
            }
            // Out-of-flow children are still placed by taffy against the root.
            for c in tree.taffy.children(t).unwrap_or_default() {
                if matches!(tree.paint_map.get(&c).and_then(|p| p.position_kind), Some(2 | 3)) {
                    if let Ok(cl) = tree.taffy.layout(c) {
                        work.push((c, x + cl.location.x, y + cl.location.y));
                    }
                }
            }
        } else {
            for c in tree.taffy.children(t).unwrap_or_default() {
                if let Ok(cl) = tree.taffy.layout(c) {
                    work.push((c, x + cl.location.x, y + cl.location.y));
                }
            }
        }
    }
    out
}

/// The border-box rects of element `el` in viewport coordinates.
pub fn client_rects(el: usize) -> Vec<(f64, f64, f64, f64)> {
    let (sx, sy) = page(|p| p.scroll);
    with_layout(|f| {
        f.boxes
            .get(&NodeId(el))
            .map(|v| v.iter().map(|(x, y, w, h)| (*x as f64 - sx, *y as f64 - sy, *w as f64, *h as f64)).collect())
            .unwrap_or_default()
    })
}

fn bounding(el: usize) -> (f64, f64, f64, f64) {
    let rects = client_rects(el);
    let rects: Vec<_> = if rects.len() > 1 { rects.into_iter().filter(|r| r.2 > 0.0 || r.3 > 0.0).collect() } else { rects };
    if rects.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let x0 = rects.iter().map(|r| r.0).fold(f64::INFINITY, f64::min);
    let y0 = rects.iter().map(|r| r.1).fold(f64::INFINITY, f64::min);
    let x1 = rects.iter().map(|r| r.0 + r.2).fold(f64::NEG_INFINITY, f64::max);
    let y1 = rects.iter().map(|r| r.1 + r.3).fold(f64::NEG_INFINITY, f64::max);
    (x0, y0, x1 - x0, y1 - y0)
}

/// Initial values for the properties `computed_report` does not cover (CSS specs' initial values,
/// serialized as Chromium's getComputedStyle does).
fn initial_value(p: &str) -> &'static str {
    match p {
        "opacity" => "1",
        "z-index" | "width" | "height" | "top" | "left" | "right" | "bottom" | "flex-basis" | "cursor" => "auto",
        "float" | "clear" | "transform" | "box-shadow" | "text-shadow" | "max-width" | "max-height" | "filter"
        | "animation-name" | "text-transform" | "background-image" | "list-style-image" => "none",
        "overflow" | "overflow-x" | "overflow-y" => "visible",
        "box-sizing" => "content-box",
        "vertical-align" => "baseline",
        "background-repeat" => "repeat",
        "flex-wrap" => "nowrap",
        "flex-grow" | "order" => "0",
        "flex-shrink" => "1",
        "justify-content" | "align-content" | "align-items" | "align-self" | "justify-items" | "justify-self" => "normal",
        "direction" => "ltr",
        "pointer-events" => "auto",
        "letter-spacing" | "word-spacing" => "normal",
        "min-width" | "min-height" => "0px",
        "margin-right" | "margin-bottom" | "margin-left" | "padding-right" | "padding-bottom" | "padding-left" => "0px",
        "border-right-width" | "border-bottom-width" | "border-left-width" => "0px",
        "border-right-style" | "border-bottom-style" | "border-left-style" => "none",
        _ => "",
    }
}

/// A computed value: AETHERSTYLE's report for its 20 properties, else the inline value, else the
/// initial value (ledgered — no number is invented for a property Aether does not compute).
pub fn computed_value(el: usize, prop: &str) -> String {
    if let Some(i) = crate::css::REPORT_PROPS.iter().position(|p| *p == prop) {
        let v = with_layout(|f| f.report.get(&NodeId(el)).map(|r| r[i].clone()));
        // An element without a box (detached, display:none ancestor) reports what the report says
        // for missing elements: display none, the rest from its own declared/initial values.
        if let Some(v) = v {
            return v;
        }
        if prop == "display" {
            return "none".into();
        }
    }
    if matches!(prop, "width" | "height") {
        if let Some(r) = with_layout(|f| f.boxes.get(&NodeId(el)).and_then(|v| v.first().copied())) {
            crate::ledger::record_css("getComputedStyle-size-is-border-box");
            let v = if prop == "width" { r.2 } else { r.3 };
            return format!("{}px", trim_num(v as f64));
        }
    }
    let inline = inline_block(el).get(prop);
    if !inline.is_empty() {
        crate::ledger::record_css(&format!("getComputedStyle-inline-not-cascaded:{prop}"));
        return inline;
    }
    crate::ledger::record_css(&format!("getComputedStyle-initial-not-cascaded:{prop}"));
    initial_value(prop).to_string()
}

fn trim_num(v: f64) -> String {
    let r = (v * 10000.0).round() / 10000.0;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
}

// =================================================================================================
// CSSOM View: DOMRect, Element/HTMLElement geometry
// =================================================================================================

fn rect_obj(vm: &mut Vm, r: (f64, f64, f64, f64), readonly: bool) -> Value {
    let p = iface(if readonly { "DOMRectReadOnly" } else { "DOMRect" }).unwrap().proto;
    Value::Object(host_obj(vm, p, T_RECT, vec![num(r.0), num(r.1), num(r.2), num(r.3)]))
}

fn this_rect(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Obj> {
    match this_tagged(vm, &ctx.this, T_RECT) {
        Some(o) => Ok(o),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn rect_field(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_rect(vm, ctx)?;
    let f = callee_str(vm, ctx);
    let (x, y, w, h) = (slot_num(vm, o, 0), slot_num(vm, o, 1), slot_num(vm, o, 2), slot_num(vm, o, 3));
    let minmax = |a: f64, b: f64, max: bool| {
        if a.is_nan() || b.is_nan() {
            f64::NAN
        } else if max {
            a.max(b)
        } else {
            a.min(b)
        }
    };
    Ok(num(match f.as_str() {
        "x" => x,
        "y" => y,
        "width" => w,
        "height" => h,
        "top" => minmax(y, y + h, false),
        "right" => minmax(x, x + w, true),
        "bottom" => minmax(y, y + h, true),
        "left" => minmax(x, x + w, false),
        _ => f64::NAN,
    }))
}

fn rect_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_rect(vm, ctx)?;
    let f = callee_str(vm, ctx);
    let v = vm.to_number(&arg(vm, ctx, 0))?;
    let i = match f.as_str() {
        "x" => 0,
        "y" => 1,
        "width" => 2,
        _ => 3,
    };
    set_slot(vm, o, i, num(v));
    Ok(Value::Undefined)
}

fn rect_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'DOMRect': Please use the 'new' operator");
    }
    let mut v = [0.0; 4];
    for (i, slot) in v.iter_mut().enumerate() {
        let a = arg(vm, ctx, i);
        if !a.is_undefined() {
            *slot = vm.to_number(&a)?;
        }
    }
    let ro = callee_str(vm, ctx) == "ro";
    let default = iface(if ro { "DOMRectReadOnly" } else { "DOMRect" }).unwrap().proto;
    let p = proto_from_new_target(vm, &ctx.new_target, default)?;
    Ok(Value::Object(host_obj(vm, p, T_RECT, vec![num(v[0]), num(v[1]), num(v[2]), num(v[3])])))
}

fn rect_to_json(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_rect(vm, ctx)?;
    let (x, y, w, h) = (slot_num(vm, o, 0), slot_num(vm, o, 1), slot_num(vm, o, 2), slot_num(vm, o, 3));
    let out = vm.new_plain_object();
    for (k, v) in [("x", x), ("y", y), ("width", w), ("height", h), ("top", y.min(y + h)), ("right", x.max(x + w)), ("bottom", y.max(y + h)), ("left", x.min(x + w))] {
        vm.create_data_property(out, PropertyKey::from_str(k), num(v))?;
    }
    Ok(Value::Object(out))
}

fn get_bounding_client_rect(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let r = bounding(el);
    Ok(rect_obj(vm, r, false))
}

fn get_client_rects(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let rects = client_rects(el);
    let vals: Vec<Value> = rects.into_iter().map(|r| rect_obj(vm, r, false)).collect();
    let arr = vm.new_array(vals);
    let p = iface("DOMRectList").unwrap().proto;
    vm.heap.get_mut(arr).proto = Some(p);
    Ok(Value::Object(arr))
}

fn box_metric(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let which = callee_str(vm, ctx);
    let first = with_layout(|f| f.boxes.get(&NodeId(el)).and_then(|v| v.first().copied()));
    let Some((x, y, w, h)) = first else { return Ok(num(0.0)) };
    let (bt, bl, br, bb) = with_layout(|f| {
        // borders from the taffy layout of this element's box
        let t = f.tree.node_map.iter().find(|(_, n)| n.id() == NodeId(el)).map(|(t, _)| *t);
        t.and_then(|t| f.tree.taffy.layout(t).ok()).map(|l| (l.border.top, l.border.left, l.border.right, l.border.bottom)).unwrap_or((0.0, 0.0, 0.0, 0.0))
    });
    let v = match which.as_str() {
        "offsetWidth" => w,
        "offsetHeight" => h,
        "clientWidth" | "scrollWidth" => w - bl - br,
        "clientHeight" | "scrollHeight" => h - bt - bb,
        "clientTop" => bt,
        "clientLeft" => bl,
        "offsetTop" => {
            let p = offset_parent(el).and_then(|p| with_layout(|f| f.boxes.get(&NodeId(p)).and_then(|v| v.first().copied())));
            y - p.map(|r| r.1).unwrap_or(0.0)
        }
        "offsetLeft" => {
            let p = offset_parent(el).and_then(|p| with_layout(|f| f.boxes.get(&NodeId(p)).and_then(|v| v.first().copied())));
            x - p.map(|r| r.0).unwrap_or(0.0)
        }
        _ => 0.0,
    };
    // CSSOM View rounds these integer-typed metrics.
    Ok(num((v as f64).round()))
}

/// CSSOM View "offsetParent": the nearest positioned ancestor, `td`/`th`/`table`, or the body.
fn offset_parent(el: usize) -> Option<usize> {
    let mut cur = with_doc(|d| d.parent_element(NodeId(el)).map(|p| p.0));
    while let Some(c) = cur {
        let tag = with_doc(|d| dom::local_name_in(d, c));
        if tag == "body" || matches!(tag.as_str(), "td" | "th" | "table") {
            return Some(c);
        }
        let pos = computed_value(c, "position");
        if pos != "static" && !pos.is_empty() {
            return Some(c);
        }
        cur = with_doc(|d| d.parent_element(NodeId(c)).map(|p| p.0));
    }
    None
}

fn offset_parent_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let p = offset_parent(el);
    Ok(dom::wrap_opt(vm, p))
}

fn scroll_pos_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_element(vm, ctx)?;
    Ok(num(0.0))
}

fn scroll_pos_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_element(vm, ctx)?;
    crate::ledger::record_dom("element-scroll-offsets-not-modelled");
    Ok(Value::Undefined)
}

fn noop_el(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_element(vm, ctx)?;
    Ok(Value::Undefined)
}

pub fn install(vm: &mut Vm) {
    let csd = interface(vm, "CSSStyleDeclaration", None, None);
    register_iface("CSSStyleDeclaration", csd);
    let p = csd.proto;
    attr(vm, p, "cssText", css_text_get, Some(css_text_set));
    attr(vm, p, "length", decl_length, None);
    op(vm, p, "item", 1, decl_item);
    op(vm, p, "getPropertyValue", 1, get_property_value);
    op(vm, p, "getPropertyPriority", 1, get_property_priority);
    op(vm, p, "setProperty", 2, set_property);
    op(vm, p, "removeProperty", 1, remove_property);
    attr(vm, p, "parentRule", parent_rule, None);
    for prop in PROPERTIES {
        let c = camel(prop);
        attr_with(vm, p, &c, prop_get, Some(prop_set), s(&c));
        if c != *prop {
            attr_with(vm, p, prop, prop_get, Some(prop_set), s(prop));
        }
        // webkit-prefixed aliases Blink still exposes for the common transform/transition family.
        if matches!(*prop, "transform" | "transition" | "animation" | "user-select" | "appearance" | "box-shadow" | "filter") {
            let w = format!("webkit{}{}", c[..1].to_ascii_uppercase(), &c[1..]);
            attr_with(vm, p, &w, prop_get, Some(prop_set), s(&w));
        }
    }

    let element = iface("Element").unwrap().proto;
    attr(vm, iface("HTMLElement").unwrap().proto, "style", style_get, Some(style_set));
    attr(vm, iface("SVGElement").unwrap().proto, "style", style_get, Some(style_set));
    op(vm, element, "getBoundingClientRect", 0, get_bounding_client_rect);
    op(vm, element, "getClientRects", 0, get_client_rects);
    for m in ["clientTop", "clientLeft", "clientWidth", "clientHeight", "scrollWidth", "scrollHeight"] {
        attr_with(vm, element, m, box_metric, None, s(m));
    }
    attr(vm, element, "scrollTop", scroll_pos_get, Some(scroll_pos_set));
    attr(vm, element, "scrollLeft", scroll_pos_get, Some(scroll_pos_set));
    for m in ["scrollIntoView", "scroll", "scrollTo", "scrollBy"] {
        op(vm, element, m, 0, noop_el);
    }
    let he = iface("HTMLElement").unwrap().proto;
    for m in ["offsetTop", "offsetLeft", "offsetWidth", "offsetHeight"] {
        attr_with(vm, he, m, box_metric, None, s(m));
    }
    attr(vm, he, "offsetParent", offset_parent_get, None);

    let fpr = vm.intr().function_proto;
    let ro = interface(vm, "DOMRectReadOnly", None, None);
    let roc = vm.make_native_with("DOMRectReadOnly", 0, rect_ctor, true, Some(fpr), vec![s("ro")]);
    rewire(vm, ro, roc, "DOMRectReadOnly");
    let ro = Iface { ctor: roc, proto: ro.proto };
    register_iface("DOMRectReadOnly", ro);
    for f in ["x", "y", "width", "height", "top", "right", "bottom", "left"] {
        attr_with(vm, ro.proto, f, rect_field, None, s(f));
    }
    op(vm, ro.proto, "toJSON", 0, rect_to_json);
    let r = interface(vm, "DOMRect", Some(ro), None);
    let rc = vm.make_native_with("DOMRect", 0, rect_ctor, true, Some(ro.ctor), vec![s("rw")]);
    rewire(vm, r, rc, "DOMRect");
    let r = Iface { ctor: rc, proto: r.proto };
    register_iface("DOMRect", r);
    for f in ["x", "y", "width", "height"] {
        attr_with(vm, r.proto, f, rect_field, Some(rect_set), s(f));
    }
    let rl = interface(vm, "DOMRectList", None, None);
    register_iface("DOMRectList", rl);
    // A DOMRectList here is an Array whose prototype is DOMRectList.prototype → %Array.prototype%
    // (length, indexing and iteration are the array's; `item` is DOMRectList's).
    let ap = vm.intr().array_proto;
    vm.heap.get_mut(rl.proto).proto = Some(ap);
    op(vm, rl.proto, "item", 1, rect_list_item);
}

fn rect_list_item(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = ctx.this.as_object() else { return vm.throw_type("Illegal invocation") };
    let i = unsigned_long(vm, &arg(vm, ctx, 0))?;
    let v = vm.get(o, &PropertyKey::Index(i))?;
    Ok(if v.is_undefined() { Value::Null } else { v })
}

fn rewire(vm: &mut Vm, i: Iface, c: Obj, name: &str) {
    vm.heap.get_mut(c).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(i.proto), 0));
    vm.heap.get_mut(i.proto).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(c), WC));
    let g = vm.realm().global;
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(c), WC));
    root(vm, Value::Object(c));
}

/// Unit-test hook: normalize `prop: value` through the inline-style path.
pub fn normalize_for_test(prop: &str, value: &str) -> Option<String> {
    let mut b = Block::default();
    if !b.set(prop, &css_core::parser::parse_component_values(value), false) {
        return None;
    }
    Some(b.serialize())
}
