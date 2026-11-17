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
    "font-style", "font-synthesis", "font-synthesis-small-caps", "font-synthesis-style", "font-synthesis-weight", "font-variant", "font-variant-caps", "font-weight", "gap", "grid", "grid-area", "grid-auto-columns",
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
    "text-overflow", "text-rendering", "text-shadow", "text-transform", "text-underline-offset", "text-wrap", "text-wrap-mode", "top",
    "touch-action", "transform", "transform-origin", "transform-style", "transition", "transition-delay",
    "transition-duration", "transition-property", "transition-timing-function", "translate", "unicode-bidi",
    "user-select", "vertical-align", "visibility", "white-space", "white-space-collapse", "widows", "width", "will-change",
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
const BORDER_IMAGE: [&str; 5] =
    ["border-image-source", "border-image-slice", "border-image-width", "border-image-outset", "border-image-repeat"];

/// The longhands of a shorthand this binding expands.
fn longhands(sh: &str) -> Option<Vec<String>> {
    SHORTHANDS.iter().find(|(s, _)| *s == sh).map(|(_, ls)| ls.iter().map(|l| l.to_string()).collect())
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
            plain_expand(prop, plain_n(prop).ok_or(())?, tv, |c| normalize_length_token(lp, c))
        }
        "border-style" => plain_expand(prop, plain_n(prop).ok_or(())?, tv, |c| ident_of(c).map(|i| i.to_ascii_lowercase()).filter(|i| BORDER_STYLES.contains(&i.as_str()))),
        "border-color" => plain_expand(prop, plain_n(prop).ok_or(())?, tv, |c| normalize_color(std::slice::from_ref(c))),
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
            if prop == "border" {
                for b in BORDER_IMAGE {
                    out.push((b.to_string(), "initial".to_string()));
                }
            }
            Ok(out)
        }
        "outline" => {
            let (w, s, c) = parse_border_side(v, "outline-width").ok_or(())?;
            Ok(vec![("outline-color".into(), c), ("outline-style".into(), s), ("outline-width".into(), w)])
        }
        "gap" => plain_expand(prop, plain_n(prop).ok_or(())?, tv, |c| normalize_length_token("row-gap", c)),
        "overflow" => plain_expand(prop, plain_n(prop).ok_or(())?, tv, |c| ident_of(c).map(|i| i.to_ascii_lowercase()).filter(|x| matches!(x.as_str(), "visible" | "hidden" | "clip" | "scroll" | "auto" | "overlay"))),
        "place-content" | "place-items" | "place-self" | "align-content" | "justify-content" | "align-items" | "justify-items" | "align-self" | "justify-self" => alignment_expand(prop, tv),
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
            if tv.iter().any(|c| matches!(c, CV::Token(Token::Delim('/')))) { return Ok(vec![(prop.to_string(), serialize_generic(prop, v))]); } // the `/` form stays opaque (owed AETHER-CSSOM-BORDER-RADIUS-SLASH)
            plain_expand(prop, plain_n(prop).ok_or(())?, tv, |c| normalize_length_token("border-top-left-radius", c))
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
        "font-synthesis" | "font-synthesis-weight" | "font-synthesis-style" | "font-synthesis-small-caps" => {
            let (w, s, c) = layout_core::css::parse_font_synthesis(prop, &css_core::serialize::to_css(tv)).ok_or(())?;
            let kw = |b: bool| (if b { "auto" } else { "none" }).to_string();
            Ok([("font-synthesis-weight", w), ("font-synthesis-style", s), ("font-synthesis-small-caps", c)].into_iter().filter_map(|(p, b)| Some((p.to_string(), kw(b?)))).collect())
        }
        "border-image" => expand_border_image(v).map(Vec::from),
        "word-wrap" => normalize_value("overflow-wrap", v).map(|x| vec![("overflow-wrap".into(), x)]).ok_or(()),
        _ => normalize_value(prop, v).map(|x| vec![(prop.to_string(), x)]).ok_or(()),
    }
}

/// The shorthands that may stand for longhand `p` in serialization, in preference order.
fn shorthands_of(p: &str) -> Vec<&'static str> {
    SHORTHANDS.iter().filter(|(_, ls)| ls.contains(&p)).map(|(sh, _)| *sh).collect()
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
    // `border` serializes only while its border-image part is still the reset value.
    let checked = if sh == "border" {
        if !vals[12..].iter().all(|v| *v == "initial") {
            return None;
        }
        &vals[..12]
    } else {
        &vals[..]
    };
    if checked.iter().any(|v| CSS_WIDE.contains(v)) {
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
    if let Some(&(_, n)) = PLAIN_N.iter().find(|(s, _)| *s == sh) { return Some((plain_serialize(n, &vals), important)); }
    let s = match sh {
        "border-top" | "border-right" | "border-bottom" | "border-left" => side(vals[0], vals[1], vals[2]),
        "border" => {
            // widths 0..4, styles 4..8, colors 8..12
            let same = |a: usize| vals[a] == vals[a + 1] && vals[a] == vals[a + 2] && vals[a] == vals[a + 3];
            if !(same(0) && same(4) && same(8)) {
                return None;
            }
            side(vals[0], vals[4], vals[8])
        }
        "flex" => format!("{} {} {}", vals[0], vals[1], vals[2]),
        "font-synthesis" => font_synthesis_words(&vals)?,
        "outline" => side(vals[2], vals[1], vals[0]),
        "background" => {
            let color = vals[8];
            let rest_initial = ls.iter().zip(vals.iter()).take(8).all(|(l, v)| bg_initial(l) == *v);
            if !rest_initial {
                return None;
            }
            if color == "transparent" { "none".to_string() } else { color.to_string() }
        }
        "border-image" => border_image_serialize(&vals)?,
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
    let styles: Vec<(usize, String)> = with_doc(|d| {
        d.descendants(NodeId(doc))
            .filter(|i| d.element(*i).is_some_and(|e| e.is_html("style")))
            .map(|i| (i.0, d.text_content(i)))
            .collect()
    });
    // AETHERDEBT SR93: a `<style>` CSP refuses (style-src-elem, nonce / hash) is not applied here
    // either (the load-time collection in lib.rs applies the same gate).
    let mut sheets: Vec<String> =
        styles.into_iter().filter(|(el, text)| crate::security::style_element_allowed(*el, text)).map(|(_, t)| t).collect();
    sheets.extend(page(|p| p.external_css.clone()));
    sheets
}

struct Flushed {
    version: u64,
    tree: crate::layout::LayoutTree,
    report: HashMap<NodeId, [String; crate::css::REPORT_COLS]>,
    boxes: HashMap<NodeId, Vec<(f32, f32, f32, f32)>>,
    /// AETHERFIT2 (SR191): every element in a `position: fixed` box's subtree, with the axes `LayoutTree::fixed_at`
    /// pins to the viewport — its rects do not move with the scroll on those axes (applied at read time,
    /// [`client_rect`]: this cache is keyed by DOM version, not by scroll).
    pinned: HashMap<NodeId, (bool, bool)>,
    /// AETHERCSSOMBOUNDINGSTICKY (SR1144): every element in a `position: sticky` box's subtree, with its (outermost) sticky
    /// root and the root's parent document origin — the root's hold for the current scroll is applied at read time
    /// ([`client_rect`] through `render::placed_origin`), like `pinned`.
    held: HashMap<NodeId, (taffy::NodeId, (f32, f32))>,
}

thread_local! {
    static FLUSHED: std::cell::RefCell<Option<Flushed>> = const { std::cell::RefCell::new(None) };
}

pub fn reset() {
    FLUSHED.with(|f| *f.borrow_mut() = None);
    CASCADED.with(|c| *c.borrow_mut() = None);
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
            let (boxes, pinned, held) = element_boxes(&tree);
            Flushed { version, tree, report, boxes, pinned, held }
        }
    };
    let r = f(&flushed);
    FLUSHED.with(|c| *c.borrow_mut() = Some(flushed));
    r
}

/// Every element's border-box rects in document coordinates (one rect for a block-level or atomic box, one per line
/// fragment for an inline box), and AETHERFIT2 (SR191) the elements a fixed box pins to the viewport, per axis: a
/// fixed box takes its origin from `LayoutTree::fixed_at` (the one viewport-origin map the painter reads) on each
/// pinned axis, and its subtree inherits the pin.
#[allow(clippy::type_complexity)]
fn element_boxes(tree: &crate::layout::LayoutTree) -> (HashMap<NodeId, Vec<(f32, f32, f32, f32)>>, HashMap<NodeId, (bool, bool)>, HashMap<NodeId, (taffy::NodeId, (f32, f32))>) {
    use crate::layout::inline::Frag;
    let mut out: HashMap<NodeId, Vec<(f32, f32, f32, f32)>> = HashMap::new();
    let mut pinned: HashMap<NodeId, (bool, bool)> = HashMap::new();
    let (mut held, mut hold_of) = (HashMap::new(), HashMap::new()); // AETHERCSSOMBOUNDINGSTICKY (SR1144)
    let mut work: Vec<(taffy::NodeId, f32, f32, (bool, bool))> = vec![(tree.root_node, 0.0, 0.0, (false, false))];
    let elem_of = |t: taffy::NodeId| tree.node_map.get(&t).filter(|n| n.is_element()).map(|n| n.id());
    // The root's own location.
    if let Ok(l) = tree.taffy.layout(tree.root_node) {
        work[0] = (tree.root_node, l.location.x, l.location.y, (false, false));
    }
    while let Some((t, mut x, mut y, mut pin)) = work.pop() {
        let Ok(l) = tree.taffy.layout(t) else { continue };
        let hold = sticky_hold(tree, t, x, y, l, pin, tree.fixed_at.contains_key(&t), &mut hold_of); // AETHERCSSOMBOUNDINGSTICKY (SR1144): before the fixed origin replaces (x, y)
        if let Some(&(fx, fy)) = tree.fixed_at.get(&t) {
            if let Some(fx) = fx {
                x = fx;
                pin.0 = true;
            }
            if let Some(fy) = fy {
                y = fy;
                pin.1 = true;
            }
        }
        if let Some(id) = elem_of(t) {
            out.entry(id).or_default().push((x, y, l.size.width, l.size.height));
            if let Some(hd) = hold { held.insert(id, hd); }
            if pin != (false, false) {
                pinned.insert(id, pin);
            }
        }
        if let Some(il) = tree.inline.get(&t) {
            let cx = x + l.border.left + l.padding.left;
            let cy = y + l.border.top + l.padding.top;
            for fr in &il.frags {
                match fr {
                    Frag::Box { node, x: fx, y: fy, w, h, .. } => {
                        if let Some(id) = elem_of(*node) {
                            out.entry(id).or_default().push((cx + fx, cy + fy, *w, *h));
                            if let Some(hd) = hold { held.insert(id, hd); }
                            if pin != (false, false) {
                                pinned.insert(id, pin);
                            }
                        }
                    }
                    Frag::Atomic { node, x: fx, y: fy } => work.push((*node, cx + fx, cy + fy, pin)),
                    Frag::Text { .. } => {}
                }
            }
            // Out-of-flow children are still placed by taffy against the root.
            for c in tree.taffy.children(t).unwrap_or_default() {
                if matches!(tree.paint_map.get(&c).and_then(|p| p.position_kind), Some(2 | 3)) {
                    if let Ok(cl) = tree.taffy.layout(c) {
                        work.push((c, x + cl.location.x, y + cl.location.y, pin));
                    }
                }
            }
        } else {
            for c in tree.taffy.children(t).unwrap_or_default() {
                if let Ok(cl) = tree.taffy.layout(c) {
                    work.push((c, x + cl.location.x, y + cl.location.y, pin));
                }
            }
        }
    }
    (out, pinned, held)
}

/// AETHERFIT2 (SR191): a document-space rect of element `id` in viewport coordinates — the scroll is subtracted
/// only on the axes a fixed box does not pin (CSS 2.2 §9.6.1: a fixed box does not move when the document scrolls).
/// The one conversion: [`client_rects`] and [`hit_test`] both read it.
fn client_rect(f: &Flushed, id: NodeId, r: (f32, f32, f32, f32), scroll: (f64, f64)) -> (f64, f64, f64, f64) {
    let (px, py) = f.pinned.get(&id).copied().unwrap_or((false, false));
    let (x, y, w, h) = r;
    let (x, y) = sticky_held_at(f, id, x, y, scroll);
    (x as f64 - if px { 0.0 } else { scroll.0 }, y as f64 - if py { 0.0 } else { scroll.1 }, w as f64, h as f64)
}

/// The border-box rects of element `el` in viewport coordinates.
pub fn client_rects(el: usize) -> Vec<(f64, f64, f64, f64)> {
    let scroll = page(|p| p.scroll);
    with_layout(|f| {
        f.boxes
            .get(&NodeId(el))
            .map(|v| v.iter().map(|r| client_rect(f, NodeId(el), *r, scroll)).collect())
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
        "overflow-x" | "overflow-y" => "visible",
        "box-sizing" => "content-box",
        "vertical-align" => "baseline",
        "background-repeat" => "repeat",
        "flex-wrap" => "nowrap",
        "flex-grow" | "order" => "0",
        "flex-shrink" => "1",
        "justify-content" | "align-content" | "align-items" | "align-self" | "justify-items" | "justify-self" => "normal",
        "direction" => "ltr", "unicode-bidi" => "normal",
        "pointer-events" => "auto",
        "letter-spacing" | "word-spacing" | "row-gap" | "column-gap" => "normal",
        "font-synthesis-weight" | "font-synthesis-style" | "font-synthesis-small-caps" => "auto", "font-variant" | "font-variant-caps" => "normal",
        "min-width" | "min-height" | "border-top-left-radius" | "border-top-right-radius" | "border-bottom-right-radius" | "border-bottom-left-radius" => "0px",
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" | "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => "0px",
        "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width" => "0px",
        "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" => "none",
        _ => "",
    }
}

/// A computed value: AETHERSTYLE's report for its 20 properties, else the inline value, else the
/// initial value (ledgered — no number is invented for a property Aether does not compute).
pub fn computed_value(el: usize, prop: &str) -> String {
    // An element that is not in the document has no computed style: every property reads "" (CSSOM
    // §9 "getComputedStyle": the declarations are empty when the element is not connected).
    if !dom::is_connected(el) {
        return String::new();
    }
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
    if prop == "caption-side" { return caption_side_computed(el); } if prop == "font-synthesis" { return font_synthesis_computed(el); } if prop == "font-variant-caps" { return font_variant_caps_computed(el); }
    if matches!(prop, "width" | "height") {
        if let Some(r) = with_layout(|f| f.boxes.get(&NodeId(el)).and_then(|v| v.first().copied())) {
            crate::ledger::record_css("getComputedStyle-size-is-border-box");
            let v = if prop == "width" { r.2 } else { r.3 };
            return format!("{}px", trim_num(v as f64));
        }
    }
    // AETHERWPT4 (SR142): the cascaded value (every author sheet and the style attribute), inherited from the parent
    // for an inherited property the element does not declare — CSSOM §9's resolved value for the properties the
    // layout report does not compute.
    if let Some(v) = cascaded_value(el, prop) {
        return v;
    }
    let inline = inline_block(el).get(prop);
    if !inline.is_empty() {
        crate::ledger::record_css(&format!("getComputedStyle-inline-not-cascaded:{prop}"));
        return inline;
    }
    crate::ledger::record_css(&format!("getComputedStyle-initial-not-cascaded:{prop}"));
    shorthand_computed(el, prop).unwrap_or_else(|| initial_value(prop).to_string())
}

thread_local! {
    /// AETHERWPT4 (SR142): each element's cascaded declarations as a declaration block, for the DOM version they
    /// were computed at.
    static CASCADED: std::cell::RefCell<Option<(u64, HashMap<NodeId, Block>)>> = const { std::cell::RefCell::new(None) };
}

/// The cascaded value of `prop` on `el` (shorthands expanded to longhands, the cascade's last declaration of a longhand
/// winning), serialized as `element.style` serializes it; None when no author declaration sets it.
fn cascaded_declared(el: usize, prop: &str) -> Option<String> {
    let version = page(|p| p.version);
    let stale = CASCADED.with(|c| c.borrow().as_ref().map(|x| x.0) != Some(version));
    if stale {
        let (w, h) = page(|p| p.viewport);
        let sheets = current_sheets();
        let root = dom::main_doc();
        let decls = with_doc(|d| crate::css::author_declarations(d, NodeId(root), &sheets, w, h));
        let blocks: HashMap<NodeId, Block> = decls
            .into_iter()
            .map(|(id, list)| {
                let mut b = Block::default();
                for d in list {
                    let name = if d.name.starts_with("--") { d.name.clone() } else { d.name.to_ascii_lowercase() };
                    b.set(&name, &d.value, d.important);
                }
                (id, b)
            })
            .collect();
        CASCADED.with(|c| *c.borrow_mut() = Some((version, blocks)));
    }
    CASCADED.with(|c| {
        let c = c.borrow();
        let v = c.as_ref()?.1.get(&NodeId(el))?.get(prop);
        (!v.is_empty()).then_some(v)
    })
}

/// CSS Cascade 5 §7.3 (the CSS-wide keywords) and §7.2 (inheritance) over [`cascaded_declared`]: `inherit` and an
/// undeclared inherited property take the parent's computed value, `initial` the initial value, `unset` / `revert`
/// either by whether the property inherits. A value that still holds `var()` is left to the fallback path.
fn cascaded_value(el: usize, prop: &str) -> Option<String> {
    let declared = cascaded_declared(el, prop);
    let parent = || with_doc(|d| d.parent_element(NodeId(el)).map(|p| p.0)).filter(|p| with_doc(|d| d.element(NodeId(*p)).is_some()));
    let inherits = css_core::container::inherited_property(prop);
    let from_parent = || parent().map(|p| computed_value(p, prop)).filter(|v| !v.is_empty());
    let initial = || Some(initial_of(prop)).filter(|v| !v.is_empty());
    match declared.as_deref() {
        Some("inherit") => from_parent(),
        Some("initial") => initial(),
        Some("unset" | "revert" | "revert-layer") => {
            if inherits {
                from_parent()
            } else {
                initial()
            }
        }
        Some(v) if v.contains("var(") => None,
        Some(v) => Some(v.to_string()),
        None if inherits => from_parent(),
        None => None,
    }
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
    let first = with_layout(|f| offset_held_first(f, el)); // AETHERCSSOMOFFSETTOPSTICKY (SR1149): the origin at a stuck sticky root's held place
    let Some((x, y, w, h)) = first else { return Ok(num(0.0)) };
    let (bt, bl, br, bb) = with_layout(|f| {
        // borders from the taffy layout of this element's box
        let t = f.tree.node_map.iter().find(|(_, n)| n.id() == NodeId(el)).map(|(t, _)| *t);
        t.and_then(|t| f.tree.taffy.layout(t).ok()).map(|l| (l.border.top, l.border.left, l.border.right, l.border.bottom)).unwrap_or((0.0, 0.0, 0.0, 0.0))
    });
    // AETHERFIT2 (SR191): CSSOM View §7 — the root element's clientWidth / clientHeight are the viewport's (minus
    // any scrollbar: Aether paints none), its scrollWidth / scrollHeight the viewport's scrolling area (the
    // document's extent, never smaller than the viewport).
    let is_root = with_doc(|d| d.element_children(NodeId(dom::main_doc())).next() == Some(NodeId(el)));
    if is_root && matches!(which.as_str(), "clientWidth" | "clientHeight" | "scrollWidth" | "scrollHeight") {
        let (vw, vh) = page(|p| p.viewport);
        let (vw, vh) = (vw as f64, vh as f64);
        let v = match which.as_str() {
            "clientWidth" => vw,
            "clientHeight" => vh,
            "scrollWidth" => document_extent().0.max(vw),
            _ => document_extent().1.max(vh),
        };
        return Ok(num(v.round()));
    }
    let v = match which.as_str() {
        "offsetWidth" => w,
        "offsetHeight" => h,
        "clientWidth" | "scrollWidth" => w - bl - br,
        "clientHeight" | "scrollHeight" => h - bt - bb,
        "clientTop" => bt,
        "clientLeft" => bl,
        // CSSOM View §7: relative to the offsetParent's padding edge, or to the initial containing
        // block when the offsetParent is the body (or there is none).
        "offsetTop" | "offsetLeft" => {
            let top = which == "offsetTop";
            let parent = offset_parent(el);
            let is_body = parent.is_some_and(|p| with_doc(|d| dom::is_html_tag(d, p, "body")));
            match parent.filter(|_| !is_body) {
                Some(p) => {
                    let pr = with_layout(|f| offset_held_first(f, p)); // AETHERCSSOMOFFSETTOPSTICKY (SR1149): the offsetParent through the same hold
                    let pb = border_of(p);
                    match pr {
                        Some(r) => {
                            if top {
                                y - r.1 - pb.0
                            } else {
                                x - r.0 - pb.1
                            }
                        }
                        None => {
                            if top {
                                y
                            } else {
                                x
                            }
                        }
                    }
                }
                None => {
                    if top {
                        y
                    } else {
                        x
                    }
                }
            }
        }
        _ => 0.0,
    };
    // CSSOM View rounds these integer-typed metrics.
    Ok(num((v as f64).round()))
}

/// (top, left) border widths of `el`'s box.
fn border_of(el: usize) -> (f32, f32) {
    with_layout(|f| {
        let t = f.tree.node_map.iter().find(|(_, n)| n.id() == NodeId(el)).map(|(t, _)| *t);
        t.and_then(|t| f.tree.taffy.layout(t).ok()).map(|l| (l.border.top, l.border.left)).unwrap_or((0.0, 0.0))
    })
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

/// AETHERSTYLE3B (SR115): `scrollTop` / `scrollLeft` read the element's scroll offset of the last frame.
fn scroll_pos_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let (x, y) = crate::layout::element_scroll_offset(NodeId(el));
    Ok(num(if callee_str(vm, ctx) == "scrollTop" { y } else { x } as f64))
}

/// A write queues a scroll request; the new offset shows after the next frame.
fn scroll_pos_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = vm.to_number(&arg(vm, ctx, 0))? as f32;
    let (x, y) = if callee_str(vm, ctx) == "scrollTop" { (None, Some(v)) } else { (Some(v), None) };
    crate::layout::request_element_scroll(NodeId(el), x, y, crate::layout::ScrollBehavior::Auto);
    Ok(Value::Undefined)
}

/// `scroll()` / `scrollTo()` take `(x, y)` or `{left, top, behavior}`; `scrollBy()` adds to the offset.
fn scroll_to(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let by = callee_str(vm, ctx) == "scrollBy";
    let a0 = arg(vm, ctx, 0);
    let mut behavior = crate::layout::ScrollBehavior::Auto;
    let (mut x, mut y) = (None, None);
    if let Value::Object(o) = a0 {
        for (k, slot) in [("left", &mut x), ("top", &mut y)] {
            let v = vm.get(o, &PropertyKey::from_str(k))?;
            if !matches!(v, Value::Undefined) {
                *slot = Some(vm.to_number(&v)? as f32);
            }
        }
        let b = vm.get(o, &PropertyKey::from_str("behavior"))?;
        if !matches!(b, Value::Undefined) {
            let bs = vm.to_string(&b)?;
            behavior = if bs.eq_str("smooth") {
                crate::layout::ScrollBehavior::Smooth
            } else if bs.eq_str("instant") {
                crate::layout::ScrollBehavior::Instant
            } else {
                crate::layout::ScrollBehavior::Auto
            };
        }
    } else {
        x = Some(vm.to_number(&a0)? as f32);
        y = Some(vm.to_number(&arg(vm, ctx, 1))? as f32);
    }
    if by {
        let (cx, cy) = crate::layout::element_scroll_offset(NodeId(el));
        x = x.map(|d| cx + d);
        y = y.map(|d| cy + d);
    }
    crate::layout::request_element_scroll(NodeId(el), x, y, behavior);
    Ok(Value::Undefined)
}

// =================================================================================================
// AETHERWPT4 (SR142): CSSOM View §6.1 hit testing and §7 scrollIntoView
// =================================================================================================

/// The document's extent (the union of every box) — the scrollable overflow of the viewport. AETHERFIT2 (SR191): a
/// box a fixed box pins is not in it (css-overflow-3 §2.2: a fixed box's containing block is the viewport, so it
/// is not part of the document's scrollable overflow).
fn document_extent() -> (f64, f64) {
    with_layout(|f| {
        let mut w = 0f32;
        let mut h = 0f32;
        for (id, rs) in &f.boxes {
            if f.pinned.contains_key(id) {
                continue;
            }
            for (x, y, bw, bh) in rs {
                w = w.max(x + bw);
                h = h.max(y + bh);
            }
        }
        (w as f64, h as f64)
    })
}

/// The paint layer an element is hit in: positioned boxes (and their descendants) above in-flow ones, ordered by the
/// nearest positioned ancestor-or-self's `z-index` (CSS 2 Appendix E's stacking order, without nested contexts).
/// AETHERFIT2 (SR191): an element in a top-layer element's subtree is above every document layer, a later top-layer
/// entry above an earlier one (HTML "top layer" / CSS Position 4) — `top` is the rendered top layer in order.
fn paint_layer(el: usize, top: &[usize]) -> (i32, i64) {
    let mut cur = Some(el);
    while let Some(c) = cur {
        if let Some(i) = top.iter().position(|&t| t == c) {
            return (2, i as i64);
        }
        cur = with_doc(|d| d.parent_element(NodeId(c)).map(|p| p.0));
    }
    let mut cur = Some(el);
    while let Some(c) = cur {
        let pos = computed_value(c, "position");
        if !pos.is_empty() && pos != "static" {
            let z = computed_value(c, "z-index").parse::<i64>().unwrap_or(0);
            return (1, z);
        }
        cur = with_doc(|d| d.parent_element(NodeId(c)).map(|p| p.0));
    }
    (0, 0)
}

/// CSSOM View §6.1 `elementsFromPoint`: every element whose border box contains the viewport point, topmost first
/// (positioned layers above the flow, then later in tree order — descendants and later siblings — above earlier), the
/// root element last; `pointer-events: none` and `visibility: hidden` boxes are not hit-test targets. Empty when the
/// point is outside the viewport.
fn hit_test(x: f64, y: f64) -> Vec<usize> {
    let (vw, vh) = page(|p| p.viewport);
    if x < 0.0 || y < 0.0 || x > vw as f64 || y > vh as f64 || !x.is_finite() || !y.is_finite() {
        return Vec::new();
    }
    let scroll = page(|p| p.scroll);
    let doc = dom::main_doc();
    let order: Vec<usize> = with_doc(|d| d.descendants(NodeId(doc)).filter(|i| d.element(*i).is_some()).map(|i| i.0).collect());
    // AETHERFIT2 (SR191): compared in CLIENT space through the one conversion (a fixed box's rect does not move with
    // the scroll), and the top layer as rendered — the same `top_layer()` list through the same `top_layer_box` path
    // the painter and the shell's hit test use — computed once per call.
    let (raw, top): (Vec<(usize, usize)>, Vec<usize>) = with_layout(|f| {
        let raw = order
            .iter()
            .enumerate()
            .filter(|(_, id)| {
                f.boxes.get(&NodeId(**id)).is_some_and(|rs| {
                    rs.iter().any(|r| {
                        let (bx, by, bw, bh) = client_rect(f, NodeId(**id), *r, scroll);
                        x >= bx && x < bx + bw && y >= by && y < by + bh
                    })
                })
            })
            .map(|(i, id)| (i, *id))
            .collect();
        let top = super::toplayer::top_layer()
            .into_iter()
            .map(|(n, _)| n)
            .filter(|&n| crate::render::top_layer_box(&f.tree, NodeId(n)).is_some())
            .collect();
        (raw, top)
    });
    let mut hits: Vec<((i32, i64), usize, usize)> = raw
        .into_iter()
        .filter(|(_, id)| computed_value(*id, "pointer-events") != "none" && computed_value(*id, "visibility") != "hidden")
        .map(|(i, id)| (paint_layer(id, &top), i, id))
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut out: Vec<usize> = hits.into_iter().map(|h| h.2).collect();
    let root = with_doc(|d| d.element_children(NodeId(doc)).next().map(|r| r.0));
    if let Some(r) = root {
        out.retain(|e| *e != r);
        out.push(r);
    }
    out
}

fn point_args(vm: &mut Vm, ctx: &CallCtx, what: &str) -> JsResult<(f64, f64)> {
    if ctx.argc < 2 {
        return vm.throw_type(&format!("Failed to execute '{what}' on 'Document': 2 arguments required, but only {} present.", ctx.argc));
    }
    let x = vm.to_number(&arg(vm, ctx, 0))?;
    let y = vm.to_number(&arg(vm, ctx, 1))?;
    if !x.is_finite() || !y.is_finite() {
        return vm.throw_type(&format!("Failed to execute '{what}' on 'Document': The provided double value is non-finite."));
    }
    Ok((x, y))
}

/// The document a hit test runs against: the page's own (other documents have no viewport, CSSOM View §6.1 "If
/// the document has no viewport, return null / the empty sequence").
fn this_main_doc(vm: &mut Vm, ctx: &CallCtx) -> bool {
    dom::node_of(vm, &ctx.this) == Some(dom::main_doc())
}

fn element_from_point(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (x, y) = point_args(vm, ctx, "elementFromPoint")?;
    if !this_main_doc(vm, ctx) {
        return Ok(Value::Null);
    }
    let first = hit_test(x, y).first().copied();
    Ok(dom::wrap_opt(vm, first))
}

fn elements_from_point(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (x, y) = point_args(vm, ctx, "elementsFromPoint")?;
    let hits = if this_main_doc(vm, ctx) { hit_test(x, y) } else { Vec::new() };
    let vals: Vec<Value> = hits.into_iter().map(|e| dom::wrap(vm, e)).collect();
    Ok(Value::Object(vm.new_array(vals)))
}

/// One axis of CSSOM View "determine the scroll-into-view position": the new scroll offset of a scrolling box whose
/// visible range is `[view0, view0 + view_len)` (in the same coordinates as the element's `[el0, el0 + el_len)`),
/// for `start` / `center` / `end` / `nearest`.
fn align_axis(how: &str, el0: f64, el_len: f64, view0: f64, view_len: f64) -> f64 {
    let el1 = el0 + el_len;
    let view1 = view0 + view_len;
    match how {
        "start" => el0,
        "end" => el1 - view_len,
        "center" => el0 + el_len / 2.0 - view_len / 2.0,
        _ => {
            // "nearest": both edges outside — nothing; else align the edge that brings the most of it in view.
            if (el0 < view0 && el1 > view1) || (el0 >= view0 && el1 <= view1) {
                view0
            } else if (el0 < view0 && el_len <= view_len) || (el1 > view1 && el_len > view_len) {
                el0
            } else {
                el1 - view_len
            }
        }
    }
}

fn enum_member(vm: &mut Vm, o: Obj, key: &str, allowed: &[&str], default: &'static str) -> JsResult<String> {
    let v = vm.get(o, &PropertyKey::from_str(key))?;
    if v.is_undefined() {
        return Ok(default.to_string());
    }
    let s = vm.to_string(&v)?.to_rust();
    if !allowed.contains(&s.as_str()) {
        return vm.throw_type(&format!("Failed to execute 'scrollIntoView' on 'Element': The provided value '{s}' is not a valid enum value of type {}.", if key == "behavior" { "ScrollBehavior" } else { "ScrollLogicalPosition" }));
    }
    Ok(s)
}

/// CSSOM View `Element.scrollIntoView(arg)`: `true` / absent → block start, inline nearest; `false` → block end; a
/// ScrollIntoViewOptions dictionary otherwise. "Scroll an element into view": every scroll-container ancestor (an
/// `overflow` other than `visible` / `clip`) is asked to bring the element into its scrollport (applied with the next
/// frame, as `scrollTo` is), then the viewport (its offset moves at once). An element without a box does nothing.
fn scroll_into_view(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let a = arg(vm, ctx, 0);
    let (mut block, mut inline, mut behavior) = ("start".to_string(), "nearest".to_string(), "auto".to_string());
    match &a {
        Value::Object(o) => {
            let o = *o;
            let pos = ["start", "center", "end", "nearest"];
            behavior = enum_member(vm, o, "behavior", &["auto", "instant", "smooth"], "auto")?;
            block = enum_member(vm, o, "block", &pos, "start")?;
            inline = enum_member(vm, o, "inline", &pos, "nearest")?;
        }
        Value::Undefined | Value::Null => {}
        other => {
            if !vm.to_boolean(other) {
                block = "end".into();
            }
        }
    }
    if !dom::is_connected(el) {
        return Ok(Value::Undefined);
    }
    let rects = client_rects(el);
    if rects.is_empty() {
        return Ok(Value::Undefined);
    }
    let (x, y, w, h) = bounding(el);
    let sb = match behavior.as_str() {
        "smooth" => crate::layout::ScrollBehavior::Smooth,
        "instant" => crate::layout::ScrollBehavior::Instant,
        _ => crate::layout::ScrollBehavior::Auto,
    };
    // Scroll-container ancestors, innermost first.
    let mut cur = with_doc(|d| d.parent_element(NodeId(el)).map(|p| p.0));
    while let Some(c) = cur {
        let is_root = with_doc(|d| d.parent(NodeId(c)).is_some_and(|p| p.0 == dom::main_doc()));
        let is_body = with_doc(|d| dom::is_html_tag(d, c, "body"));
        if !is_root && !is_body {
            let ox = computed_value(c, "overflow-x");
            let oy = computed_value(c, "overflow-y");
            let scrolls = |v: &str| matches!(v, "auto" | "scroll" | "hidden");
            if scrolls(&ox) || scrolls(&oy) {
                let (cx, cy, _, _) = bounding(c);
                let (bt, bl) = border_of(c);
                let first = with_layout(|f| f.boxes.get(&NodeId(c)).and_then(|v| v.first().copied()));
                if let Some((_, _, cw, ch)) = first {
                    let (sx, sy) = crate::layout::element_scroll_offset(NodeId(c));
                    // The element's position in the container's scrolled content.
                    let ex = x - cx - bl as f64 + sx as f64;
                    let ey = y - cy - bt as f64 + sy as f64;
                    let tx = align_axis(&inline, ex, w, sx as f64, (cw - 2.0 * bl) as f64);
                    let ty = align_axis(&block, ey, h, sy as f64, (ch - 2.0 * bt) as f64);
                    let nx = scrolls(&ox).then_some(tx.max(0.0) as f32);
                    let ny = scrolls(&oy).then_some(ty.max(0.0) as f32);
                    crate::layout::request_element_scroll(NodeId(c), nx, ny, sb);
                }
            }
        }
        cur = with_doc(|d| d.parent_element(NodeId(c)).map(|p| p.0));
    }
    // The viewport.
    let (sx, sy) = page(|p| p.scroll);
    let (vw, vh) = page(|p| p.viewport);
    let (dw, dh) = document_extent();
    let tx = align_axis(&inline, x + sx, w, sx, vw as f64).clamp(0.0, (dw - vw as f64).max(0.0));
    let ty = align_axis(&block, y + sy, h, sy, vh as f64).clamp(0.0, (dh - vh as f64).max(0.0));
    page(|p| p.scroll = (tx, ty));
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
    for m in ["scrollTop", "scrollLeft"] {
        attr_with(vm, element, m, scroll_pos_get, Some(scroll_pos_set), s(m));
    }
    for m in ["scroll", "scrollTo", "scrollBy"] {
        op_with(vm, element, m, 0, scroll_to, s(m));
    }
    op(vm, element, "scrollIntoView", 0, scroll_into_view);
    let dp = iface("Document").unwrap().proto;
    op(vm, dp, "elementFromPoint", 2, element_from_point);
    op(vm, dp, "elementsFromPoint", 2, elements_from_point);
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

// ---- AETHERCAPTIONCSSOM (LEDGER SR686): getComputedStyle reports caption-side's used value (named tail block) ----

/// AETHERCAPTIONCSSOM (SR686): `caption-side` as Chromium computes it (the specified keyword, inherited, initial top;
/// a caption's align= is its presentation hint): the used value layout_core folded into `PaintStyle::caption_side`,
/// read through its ONE walk (`layout_core::caption_side_used`) on the flushed tree. An element without a box
/// (display: none) reads the cascaded value, else top.
fn caption_side_computed(el: usize) -> String {
    let used = with_layout(|f| {
        let (tid, _) = f.tree.node_map.iter().find(|(_, n)| n.is_element() && n.id() == NodeId(el))?;
        Some(layout_core::caption_side_used(&f.tree, *tid))
    });
    match used {
        Some(true) => "bottom".to_string(),
        Some(false) => "top".to_string(),
        None => cascaded_value(el, "caption-side").unwrap_or_else(|| "top".to_string()),
    }
}

// ---- AETHERFONTSYNTHESISCSSOM (LEDGER SR764): getComputedStyle's font-synthesis recomposed from its longhands (named tail block) ----

/// AETHERFONTSYNTHESISCSSOM (SR764): the computed `font-synthesis` is the shorthand serialization of its two longhands'
/// computed values (each cascaded, inherited or initial on its own), in Chromium's canonical order (`weight style`,
/// `weight`, `style`, `none`): an element that declares only `font-synthesis-weight: none` reads `style`, not the
/// shorthand its parent computes.
fn font_synthesis_computed(el: usize) -> String {
    shorthand_computed(el, "font-synthesis").unwrap_or_default()
}

// ---- AETHERFONTSYNTHESISSMALLCAPS (LEDGER SR772): the font-synthesis shorthand serialized over its three longhands (named tail block) ----

/// AETHERFONTSYNTHESISSMALLCAPS (SR772): `font-synthesis` from its `weight` / `style` / `small-caps` longhands' values
/// (in `longhands` order): each one that is `auto` names its keyword, in Chromium's canonical order (weight, style,
/// small-caps); all three `none` is `none`; any other value (a mix with a CSS-wide keyword) has no shorthand.
fn font_synthesis_words(vals: &[&str]) -> Option<String> {
    let mut on = Vec::new();
    for (k, v) in ["weight", "style", "small-caps"].iter().zip(vals.iter()) {
        match *v {
            "auto" => on.push(*k),
            "none" => {}
            _ => return None,
        }
    }
    Some(if on.is_empty() { "none".to_string() } else { on.join(" ") })
}

// ---- AETHERCSSOMSHORTHANDTABLE (LEDGER SR786): every shorthand's longhands in one table (named tail block) ----

/// AETHERCSSOMSHORTHANDTABLE (SR786): every shorthand this binding expands and its longhands in canonical order —
/// the ONE table [`longhands`] looks up and [`shorthands_of`] scans in reverse (a longhand's shorthands in row order,
/// which is `Block::serialize`'s preference order: `border` before `border-width` before `border-top`). The per-shorthand
/// grammar stays in [`expand`] and [`serialize_shorthand`]; the table holds only the pairs, so the two directions
/// cannot drift (owed AETHER-CSSOM-SHORTHAND-TABLE-SERIALIZE / -INITIAL).
const SHORTHANDS: &[(&str, &[&str])] = &[
    (
        "border",
        &[
            "border-top-width", "border-right-width", "border-bottom-width", "border-left-width", "border-top-style",
            "border-right-style", "border-bottom-style", "border-left-style", "border-top-color", "border-right-color",
            "border-bottom-color", "border-left-color", "border-image-source", "border-image-slice",
            "border-image-width", "border-image-outset", "border-image-repeat",
        ],
    ),
    (
        "border-image",
        &[
            "border-image-source", "border-image-slice", "border-image-width", "border-image-outset",
            "border-image-repeat",
        ],
    ),
    ("border-width", &["border-top-width", "border-right-width", "border-bottom-width", "border-left-width"]),
    ("border-style", &["border-top-style", "border-right-style", "border-bottom-style", "border-left-style"]),
    ("border-color", &["border-top-color", "border-right-color", "border-bottom-color", "border-left-color"]),
    ("border-top", &["border-top-width", "border-top-style", "border-top-color"]),
    ("border-right", &["border-right-width", "border-right-style", "border-right-color"]),
    ("border-bottom", &["border-bottom-width", "border-bottom-style", "border-bottom-color"]),
    ("border-left", &["border-left-width", "border-left-style", "border-left-color"]),
    ("margin", &["margin-top", "margin-right", "margin-bottom", "margin-left"]),
    ("padding", &["padding-top", "padding-right", "padding-bottom", "padding-left"]),
    ("inset", &["top", "right", "bottom", "left"]),
    ("gap", &["row-gap", "column-gap"]),
    ("overflow", &["overflow-x", "overflow-y"]),
    ("font-synthesis", &["font-synthesis-weight", "font-synthesis-style", "font-synthesis-small-caps"]),
    ("flex", &["flex-grow", "flex-shrink", "flex-basis"]),
    ("outline", &["outline-color", "outline-style", "outline-width"]),
    (
        "border-radius",
        &[
            "border-top-left-radius", "border-top-right-radius", "border-bottom-right-radius",
            "border-bottom-left-radius",
        ],
    ),
    (
        "background",
        &[
            "background-image", "background-position-x", "background-position-y", "background-size",
            "background-repeat", "background-attachment", "background-origin", "background-clip", "background-color",
        ],
    ),
    ("place-content", &["align-content", "justify-content"]),
    ("place-items", &["align-items", "justify-items"]),
    ("place-self", &["align-self", "justify-self"]),
];

#[cfg(test)]
mod shorthand_table_tests {
    use super::*;

    /// The old `longhands` (`sh=...`) and `shorthands_of` (`p>...`, non-empty only) results, captured from the two
    /// hand-written matches at cd877b54d over PROPERTIES and every longhand (242 names) before the table replaced them.
    const OLD_CAPTURE: &str = "border=border-top-width border-right-width border-bottom-width border-left-width border-top-style border-right-style border-bottom-style border-left-style border-top-color border-right-color border-bottom-color border-left-color border-image-source border-image-slice border-image-width border-image-outset border-image-repeat
border-image=border-image-source border-image-slice border-image-width border-image-outset border-image-repeat
border-width=border-top-width border-right-width border-bottom-width border-left-width
border-style=border-top-style border-right-style border-bottom-style border-left-style
border-color=border-top-color border-right-color border-bottom-color border-left-color
border-top=border-top-width border-top-style border-top-color
border-right=border-right-width border-right-style border-right-color
border-bottom=border-bottom-width border-bottom-style border-bottom-color
border-left=border-left-width border-left-style border-left-color
margin=margin-top margin-right margin-bottom margin-left
padding=padding-top padding-right padding-bottom padding-left
inset=top right bottom left
gap=row-gap column-gap
overflow=overflow-x overflow-y
font-synthesis=font-synthesis-weight font-synthesis-style font-synthesis-small-caps
flex=flex-grow flex-shrink flex-basis
outline=outline-color outline-style outline-width
border-radius=border-top-left-radius border-top-right-radius border-bottom-right-radius border-bottom-left-radius
background=background-image background-position-x background-position-y background-size background-repeat background-attachment background-origin background-clip background-color
background-attachment>background
background-blend-mode>background
background-clip>background
background-color>background
background-image>background
background-origin>background
background-position>background
background-position-x>background
background-position-y>background
background-repeat>background
background-size>background
border-bottom-color>border border-color border-bottom
border-bottom-left-radius>border-radius
border-bottom-right-radius>border-radius
border-bottom-style>border border-style border-bottom
border-bottom-width>border border-width border-bottom
border-color>border border-color
border-image-outset>border border-image
border-image-repeat>border border-image
border-image-slice>border border-image
border-image-source>border border-image
border-image-width>border border-image
border-left-color>border border-color border-left
border-left-style>border border-style border-left
border-left-width>border border-width border-left
border-radius>border-radius
border-right-color>border border-color border-right
border-right-style>border border-style border-right
border-right-width>border border-width border-right
border-style>border border-style
border-top-color>border border-color border-top
border-top-left-radius>border-radius
border-top-right-radius>border-radius
border-top-style>border border-style border-top
border-top-width>border border-width border-top
border-width>border border-width
bottom>inset
column-gap>gap
flex-basis>flex
flex-grow>flex
flex-shrink>flex
font-synthesis-small-caps>font-synthesis
font-synthesis-style>font-synthesis
font-synthesis-weight>font-synthesis
left>inset
margin-block>margin
margin-bottom>margin
margin-inline>margin
margin-left>margin
margin-right>margin
margin-top>margin
outline-color>outline
outline-style>outline
outline-width>outline
overflow-x>overflow
overflow-y>overflow
padding-block>padding
padding-bottom>padding
padding-inline>padding
padding-left>padding
padding-right>padding
padding-top>padding
right>inset
row-gap>gap
top>inset";

    /// The ten names the old `shorthands_of` prefix rules mapped to a shorthand whose longhands do not include them;
    /// the table (the `longhands` side) maps each to none.
    const FIXED: [&str; 10] = [
        "background-blend-mode", "background-position", "border-color", "border-radius", "border-style", "border-width",
        "margin-block", "margin-inline", "padding-block", "padding-inline",
    ];

    const PROBES: [&str; 8] = ["1px", "solid", "red", "1px solid red", "hidden", "1", "weight style small-caps", "center" /* SR1102: an alignment word for the place-* rows under alignment_expand */];

    fn universe() -> Vec<String> {
        let mut names: Vec<String> = PROPERTIES.iter().map(|s| s.to_string()).collect();
        names.extend(SHORTHANDS.iter().flat_map(|(_, ls)| ls.iter().map(|l| l.to_string())));
        names.sort();
        names.dedup();
        names
    }

    fn expand_str(prop: &str, value: &str) -> Result<Vec<(String, String)>, ()> {
        expand(prop, &css_core::parser::parse_component_values(value))
    }

    /// The identity: the table reproduces the old matches byte for byte, but for the ten named prefix over-matches.
    #[test]
    fn shorthand_table_reproduces_the_old_matches() {
        let mut keys = 0;
        let mut old_of: HashMap<&str, Vec<&str>> = HashMap::new();
        for line in OLD_CAPTURE.lines() {
            if let Some((sh, ls)) = line.split_once('=') {
                keys += 1;
                let want: Vec<String> = ls.split(' ').map(|s| s.to_string()).collect();
                assert_eq!(longhands(sh), Some(want), "longhands({sh})");
            } else if let Some((p, shs)) = line.split_once('>') {
                old_of.insert(p, shs.split(' ').collect());
            }
        }
        assert_eq!(keys + PLACE_SHORTHANDS.len(), SHORTHANDS.len(), "every old shorthand is a table row, plus SR1087's three place-* rows (19 + 3 = 22)");
        let names = universe();
        assert_eq!(names.len(), 246) /* SR800 fold: + font-variant-caps; SR1109: + text-wrap, text-wrap-mode, white-space-collapse */;
        for p in &names {
            let want = if let Some(sh) = PLACE_SHORTHANDS.iter().find(|sh| longhands(sh).unwrap().contains(p)) { vec![*sh] /* SR1087: a place-* longhand maps to its place row */ } else if FIXED.contains(&p.as_str()) { Vec::new() } else { old_of.get(p.as_str()).cloned().unwrap_or_default() };
            assert_eq!(shorthands_of(p), want, "shorthands_of({p})");
        }
        for p in FIXED {
            assert!(!old_of.get(p).unwrap().is_empty(), "{p} was an old over-match");
        }
    }

    /// The table is closed: no duplicate shorthand or longhand in a row, every longhand maps back to its shorthand,
    /// no shorthand is itself a longhand, and every name is a PROPERTIES entry (but border-image's five longhands).
    #[test]
    fn shorthand_table_is_closed() {
        for (i, (sh, ls)) in SHORTHANDS.iter().enumerate() {
            assert!(SHORTHANDS[..i].iter().all(|(s, _)| s != sh), "{sh} is one row");
            assert!(PROPERTIES.contains(sh), "{sh} in PROPERTIES");
            assert!(shorthands_of(sh).is_empty(), "{sh} is no longhand");
            for (j, l) in ls.iter().enumerate() {
                assert!(!ls[..j].contains(l), "{sh}: {l} once");
                assert!(shorthands_of(l).contains(sh), "{l} maps back to {sh}");
                assert!(PROPERTIES.contains(l) || l.starts_with("border-image-"), "{l} in PROPERTIES");
            }
        }
    }

    /// expand knows every table shorthand and no other: (a) each row's shorthand expands to exactly its longhands
    /// under some probe value; (b) a PROPERTIES name whose expansion names any other property is a row
    /// or `word-wrap` (expand's overflow-wrap alias).
    #[test]
    fn shorthand_table_matches_expand() {
        for (sh, ls) in SHORTHANDS {
            let want: Vec<&str> = ls.to_vec();
            let hit = PROBES.iter().any(|v| {
                expand_str(sh, v).is_ok_and(|pairs| pairs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>() == want)
            });
            assert!(hit, "{sh} expands to its table longhands");
        }
        for p in PROPERTIES {
            for v in PROBES {
                if let Ok(pairs) = expand_str(p, v) {
                    if pairs.iter().any(|(n, _)| n != p) {
                        assert!(*p == "word-wrap" || longhands(p).is_some(), "{p}: {v} expands but is no table row");
                    }
                }
            }
        }
    }

    #[test]
    fn shorthand_font_synthesis_longhands_map_back() {
        for l in ["font-synthesis-weight", "font-synthesis-style", "font-synthesis-small-caps"] {
            assert_eq!(shorthands_of(l), vec!["font-synthesis"], "{l}");
        }
    }

    #[test]
    fn shorthand_border_top_width_order() {
        assert_eq!(shorthands_of("border-top-width"), vec!["border", "border-width", "border-top"]);
        assert_eq!(shorthands_of("border-image-source"), vec!["border", "border-image"]);
    }

    /// The fixed drift's behaviour: a stored non-longhand is no longer dropped behind a prefix shorthand that serializes.
    #[test]
    fn shorthand_serialize_keeps_a_non_longhand() {
        assert_eq!(
            Block::parse("background: red; background-position: 10px").serialize(),
            "background: red; background-position: 10px;"
        );
        assert_eq!(Block::parse("margin: 1px; margin-block: 2px").serialize(), "margin: 1px; margin-block: 2px;");
    }
}

// ---- AETHERFONTVARIANTSMALLCAPS (LEDGER SR800): getComputedStyle's font-variant-caps from the flushed layout (named tail block) ----

/// AETHERFONTVARIANTSMALLCAPS (SR800): the computed `font-variant-caps`, read as SR686 reads caption-side: the used value
/// layout_core folded into `PaintStyle::small_caps` (the longhand, `font-variant`'s caps half or the `font` shorthand's
/// `small-caps` prefix, which this CSSOM does not expand), inherited through its ONE walk (`layout_core::inline::
/// small_caps_used`) on the flushed tree. An element without a box reads the cascaded value, else `normal`.
fn font_variant_caps_computed(el: usize) -> String {
    let used = with_layout(|f| {
        let (tid, _) = f.tree.node_map.iter().find(|(_, n)| n.is_element() && n.id() == NodeId(el))?;
        Some(layout_core::inline::small_caps_used(&f.tree, *tid))
    });
    match used {
        Some(c) => c.word().to_string(),
        None => cascaded_value(el, "font-variant-caps").unwrap_or_else(|| "normal".to_string()),
    }
}

// ---- AETHERCSSOMBORDERIMAGEEXPAND (LEDGER SR1030): border-image shorthand expand + serialize (named tail block) ----

/// AETHERCSSOMBORDERIMAGEEXPAND (SR1030): css-backgrounds-3 §6.6 `border-image: <source> || <slice> [ / <width> |
/// / <width>? / <outset> ]? || <repeat>` into its five longhands in [`BORDER_IMAGE`] order; an omitted part takes its
/// initial value (`none`, `100%`, `1`, `0`, `stretch`). `Err` drops the declaration. The CSS-wide keywords and `var()`
/// never reach here ([`expand`]'s guards). layout_core reads no border-image (owed AETHER-BORDER-IMAGE-PAINT).
fn expand_border_image(v: &[CV]) -> Result<[(String, String); 5], ()> {
    const REPEATS: [&str; 4] = ["stretch", "repeat", "round", "space"];
    let parts = split_values(v);
    if parts.is_empty() {
        return Err(());
    }
    let num = |c: &CV| match c {
        CV::Token(Token::Number(n)) if n.value >= 0.0 => {
            let mut s = String::new();
            css_core::serialize::number(n, &mut s);
            Some(s)
        }
        _ => None,
    };
    let num_or_pct = |c: &CV| match c {
        CV::Token(Token::Percentage(n)) if n.value >= 0.0 => {
            let mut s = String::new();
            css_core::serialize::number(n, &mut s);
            s.push('%');
            Some(s)
        }
        _ => num(c),
    };
    let negative = |c: &CV| matches!(c, CV::Token(Token::Dimension(n, _)) | CV::Token(Token::Percentage(n)) if n.value < 0.0);
    let width_tok = |c: &CV| -> Option<String> {
        if let Some(s) = num(c) {
            return Some(s);
        }
        if ident_of(c).is_some_and(|i| i.eq_ignore_ascii_case("auto")) {
            return Some("auto".into());
        }
        if negative(c) {
            return None;
        }
        normalize_length_token("border-image-width", c)
    };
    let outset_tok = |c: &CV| -> Option<String> {
        if let Some(s) = num(c) {
            return Some(s);
        }
        if negative(c) || matches!(c, CV::Token(Token::Percentage(_))) {
            return None;
        }
        normalize_length_token("border-image-outset", c)
    };
    let is_slash = |c: &CV| matches!(c, CV::Token(Token::Delim('/')));
    let is_image = |c: &CV| match c {
        CV::Token(Token::Url(_)) => true,
        CV::Function(name, _) => {
            let l = name.to_ascii_lowercase();
            matches!(l.as_str(), "url" | "image" | "image-set" | "cross-fade" | "element") || l.ends_with("gradient")
        }
        _ => false,
    };
    let quad = |xs: &[String]| four(xs).map(|q| four_serialize([q[0].as_str(), q[1].as_str(), q[2].as_str(), q[3].as_str()]));
    let (mut source, mut slice, mut width, mut outset, mut repeat): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = (None, None, None, None, None);
    let mut i = 0;
    while i < parts.len() {
        let c = &parts[i];
        let id = ident_of(c).map(|s| s.to_ascii_lowercase());
        if source.is_none() && (is_image(c) || id.as_deref() == Some("none")) {
            source = Some(if is_image(c) { serialize_generic("border-image-source", std::slice::from_ref(c)) } else { "none".into() });
            i += 1;
        } else if repeat.is_none() && id.as_deref().is_some_and(|k| REPEATS.contains(&k)) {
            let mut ks: Vec<String> = Vec::new();
            while i < parts.len() && ks.len() < 2 {
                match ident_of(&parts[i]).map(|s| s.to_ascii_lowercase()) {
                    Some(k) if REPEATS.contains(&k.as_str()) => {
                        ks.push(k);
                        i += 1;
                    }
                    _ => break,
                }
            }
            if ks.len() == 2 && ks[0] == ks[1] {
                ks.pop();
            }
            repeat = Some(ks.join(" "));
        } else if slice.is_none() && (num_or_pct(c).is_some() || id.as_deref() == Some("fill")) {
            // `[<number> | <percentage>]{1,4} && fill?`: `fill` once, before or after the numbers.
            let mut fill = false;
            let mut vals: Vec<String> = Vec::new();
            while i < parts.len() {
                let t = &parts[i];
                if !fill && ident_of(t).is_some_and(|k| k.eq_ignore_ascii_case("fill")) {
                    fill = true;
                    i += 1;
                    if !vals.is_empty() {
                        break;
                    }
                } else if vals.len() < 4 && num_or_pct(t).is_some() {
                    vals.push(num_or_pct(t).unwrap());
                    i += 1;
                } else {
                    break;
                }
            }
            if vals.is_empty() {
                return Err(());
            }
            slice = Some(format!("{}{}", quad(&vals[..]).ok_or(())?, if fill { " fill" } else { "" }));
            // `[ / <width> | / <width>? / <outset> ]?`
            if i < parts.len() && is_slash(&parts[i]) {
                i += 1;
                let mut ws: Vec<String> = Vec::new();
                while i < parts.len() && ws.len() < 4 {
                    match width_tok(&parts[i]) {
                        Some(s) => {
                            ws.push(s);
                            i += 1;
                        }
                        None => break,
                    }
                }
                if i < parts.len() && is_slash(&parts[i]) {
                    i += 1;
                    let mut os: Vec<String> = Vec::new();
                    while i < parts.len() && os.len() < 4 {
                        match outset_tok(&parts[i]) {
                            Some(s) => {
                                os.push(s);
                                i += 1;
                            }
                            None => break,
                        }
                    }
                    if os.is_empty() {
                        return Err(());
                    }
                    outset = Some(quad(&os[..]).ok_or(())?);
                } else if ws.is_empty() {
                    return Err(());
                }
                if !ws.is_empty() {
                    width = Some(quad(&ws[..]).ok_or(())?);
                }
            }
        } else {
            return Err(());
        }
    }
    let vals = [
        source.unwrap_or_else(|| "none".into()),
        slice.unwrap_or_else(|| "100%".into()),
        width.unwrap_or_else(|| "1".into()),
        outset.unwrap_or_else(|| "0".into()),
        repeat.unwrap_or_else(|| "stretch".into()),
    ];
    let mut it = vals.into_iter();
    Ok(BORDER_IMAGE.map(|p| (p.to_string(), it.next().unwrap())))
}

/// SR1030: Blink's `border-image` serialization from the five stored longhands, initial parts omitted: the slice is
/// written while any of slice / width / outset is non-initial, an omitted width keeps its `/` slot before an outset
/// (`30 / / 5px`); every part initial reads `none`.
fn border_image_serialize(vals: &[&str]) -> Option<String> {
    let [src, slice, width, outset, repeat] = vals else { return None };
    let (w0, o0) = (*width == "1", *outset == "0");
    let mut out: Vec<String> = Vec::new();
    if *src != "none" {
        out.push(src.to_string());
    }
    if *slice != "100%" || !w0 || !o0 {
        out.push(slice.to_string());
    }
    if !w0 || !o0 {
        out.push(if w0 { "/".to_string() } else { format!("/ {width}") });
    }
    if !o0 {
        out.push(format!("/ {outset}"));
    }
    if *repeat != "stretch" {
        out.push(repeat.to_string());
    }
    Some(if out.is_empty() { "none".to_string() } else { out.join(" ") })
}

#[cfg(test)]
mod border_image_tests {
    use super::*;

    /// The five longhands `css` leaves in a parsed block, in [`BORDER_IMAGE`] order.
    fn five(css: &str) -> Vec<String> {
        let b = Block::parse(css);
        BORDER_IMAGE.iter().map(|p| b.get(p)).collect()
    }

    #[test]
    fn border_image_chromium_case() {
        let css = "border-image: url(a.png) 30 / 10px round";
        assert_eq!(five(css), ["url(\"a.png\")", "30", "10px", "0", "round"]);
        assert_eq!(Block::parse(css).get("border-image"), "url(\"a.png\") 30 / 10px round");
    }

    #[test]
    fn border_image_none_takes_the_initials() {
        assert_eq!(five("border-image: none"), ["none", "100%", "1", "0", "stretch"]);
        assert_eq!(Block::parse("border-image: none").get("border-image"), "none");
    }

    #[test]
    fn border_image_source_alone() {
        assert_eq!(five("border-image: url(a.png)"), ["url(\"a.png\")", "100%", "1", "0", "stretch"]);
        assert_eq!(Block::parse("border-image: url(a.png)").get("border-image"), "url(\"a.png\")");
    }

    #[test]
    fn border_image_every_part() {
        let css = "border-image: 30 fill / 10px 20px / 5px space";
        assert_eq!(five(css), ["none", "30 fill", "10px 20px", "5px", "space"]);
        assert_eq!(Block::parse(css).get("border-image"), "30 fill / 10px 20px / 5px space");
    }

    #[test]
    fn border_image_omitted_width_keeps_its_slot() {
        let css = "border-image: 30 / / 5px";
        assert_eq!(five(css), ["none", "30", "1", "5px", "stretch"]);
        assert_eq!(Block::parse(css).get("border-image"), "30 / / 5px");
    }

    #[test]
    fn border_image_css_wide_reaches_the_five() {
        assert_eq!(five("border-image: inherit"), ["inherit"; 5]);
        assert_eq!(Block::parse("border-image: inherit").get("border-image"), "inherit");
    }

    #[test]
    fn border_image_invalid_is_dropped() {
        for bad in ["10px 10px 10px 10px 10px 10px", "30 /", "round round round", "fill", "30 / 10px /"] {
            assert!(Block::parse(&format!("border-image: {bad}")).decls.is_empty(), "{bad} is dropped");
        }
    }

    #[test]
    fn border_image_longhands_round_trip() {
        let b = Block::parse(
            "border-image-source: url(a.png); border-image-slice: 30; border-image-width: 10px; border-image-outset: 0; border-image-repeat: round",
        );
        assert_eq!(b.serialize(), "border-image: url(\"a.png\") 30 / 10px round;");
    }

    #[test]
    fn border_image_keeps_the_border_reset_guard() {
        let b = Block::parse("border: 1px solid red; border-image: url(a.png) 30");
        assert_eq!(b.get("border"), "");
        assert_eq!(b.serialize(), "border-width: 1px; border-style: solid; border-color: red; border-image: url(\"a.png\") 30;");
    }
}

// ---- AETHERCSSOMSHORTHANDTABLESERIALIZE (LEDGER SR1078): the plain n-value serialization rules as table data (named tail block) ----

/// AETHERCSSOMSHORTHANDTABLESERIALIZE (SR1078): the shorthands whose serialization is the plain n-value collapse —
/// n = 4 (top right bottom left: 4 -> 1-4 values through [`four_serialize`]) or n = 2 (2 -> 1-2 values). Each name is a
/// [`SHORTHANDS`] row with exactly n longhands; every other row has a grammar arm in [`serialize_shorthand`]. The
/// `shorthand_table_serialize_*` tests pin both, so a new 2- or 4-longhand row cannot silently read "".
const PLAIN_N: &[(&str, u8)] = &[
    ("margin", 4),
    ("padding", 4),
    ("inset", 4),
    ("border-width", 4),
    ("border-style", 4),
    ("border-color", 4),
    ("border-radius", 4),
    ("gap", 2),
    ("overflow", 2),
    ("place-content", 2),
    ("place-items", 2),
    ("place-self", 2),
];

/// The plain n-value serialization of a [`PLAIN_N`] shorthand's longhand values (in [`SHORTHANDS`] order).
fn plain_serialize(n: u8, vals: &[&str]) -> String {
    match n {
        4 => four_serialize([vals[0], vals[1], vals[2], vals[3]]),
        2 if vals[0] == vals[1] => vals[0].to_string(),
        2 => format!("{} {}", vals[0], vals[1]),
        _ => vals.join(" "),
    }
}

#[cfg(test)]
mod shorthand_table_serialize_tests {
    use super::*;

    /// The rows whose serialization is a hand-written grammar arm in [`serialize_shorthand`].
    const GRAMMAR: [&str; 10] = [
        "border", "border-image", "border-top", "border-right", "border-bottom", "border-left", "font-synthesis", "flex",
        "outline", "background",
    ];

    fn get(css: &str, sh: &str) -> String {
        Block::parse(css).get(sh)
    }

    #[test]
    fn shorthand_table_serialize_plain_rows_are_table_rows() {
        for (i, (sh, n)) in PLAIN_N.iter().enumerate() {
            assert!(PLAIN_N[..i].iter().all(|(s, _)| s != sh), "{sh} is one PLAIN_N row");
            assert!(*n == 2 || *n == 4, "{sh}: n is 2 or 4");
            assert_eq!(longhands(sh).map(|ls| ls.len()), Some(*n as usize), "{sh} has {n} longhands");
        }
    }

    #[test]
    fn shorthand_table_serialize_every_row_has_a_road() {
        for (sh, ls) in SHORTHANDS {
            let plain = PLAIN_N.iter().any(|(s, _)| s == sh);
            let grammar = GRAMMAR.contains(sh);
            assert!(plain != grammar, "{sh} is in exactly one of PLAIN_N / GRAMMAR");
            if (ls.len() == 2 || ls.len() == 4) && !grammar {
                assert!(plain, "{sh}: a 2/4-longhand row with no grammar arm is PLAIN_N");
            }
        }
    }

    #[test]
    fn shorthand_table_serialize_margin() {
        assert_eq!(get("margin: 1px 2px 3px 4px", "margin"), "1px 2px 3px 4px");
        assert_eq!(get("margin: 1px 2px 1px 2px", "margin"), "1px 2px");
        assert_eq!(get("margin: 1px 2px 3px", "margin"), "1px 2px 3px");
        assert_eq!(get("margin: 3px 3px 3px 3px", "margin"), "3px");
    }

    #[test]
    fn shorthand_table_serialize_gap() {
        assert_eq!(get("gap: 5px 5px", "gap"), "5px");
        assert_eq!(get("gap: 5px 10px", "gap"), "5px 10px");
    }

    #[test]
    fn shorthand_table_serialize_overflow() {
        assert_eq!(get("overflow: hidden hidden", "overflow"), "hidden");
        assert_eq!(get("overflow: hidden auto", "overflow"), "hidden auto");
    }

    #[test]
    fn shorthand_table_serialize_border_radius() {
        assert_eq!(get("border-radius: 4px", "border-radius"), "4px");
        let css = "border-top-left-radius: 1px; border-top-right-radius: 2px; border-bottom-right-radius: 1px; border-bottom-left-radius: 2px";
        assert_eq!(get(css, "border-radius"), "1px 2px");
    }

    #[test]
    fn shorthand_table_serialize_grammar_unmoved() {
        assert_eq!(get("border: 1px solid red", "border"), "1px solid red");
        assert_eq!(get("outline: red solid 1px", "outline"), "1px solid red");
        assert_eq!(get("flex: 1 2 10px", "flex"), "1 2 10px");
        assert_eq!(get("background: red", "background"), "red");
    }

    #[test]
    fn shorthand_table_serialize_missing_longhand() {
        assert_eq!(get("margin-top: 1px; margin-right: 1px; margin-bottom: 1px", "margin"), "");
        assert_eq!(get("margin-top: 1px; margin-right: 1px; margin-bottom: 1px; margin-left: 1px !important", "margin"), "");
        assert_eq!(get("gap: 5px; column-gap: 6px !important", "gap"), "");
    }
}

// ---- AETHERCSSOMSHORTHANDTABLEEXPAND (LEDGER SR1083): the plain n-value expansion through PLAIN_N (named tail block) ----

/// AETHERCSSOMSHORTHANDTABLEEXPAND (SR1083): a [`PLAIN_N`] shorthand's n (SR1078's serialize lookup as a fn).
fn plain_n(sh: &str) -> Option<u8> {
    PLAIN_N.iter().find(|(s, _)| *s == sh).map(|&(_, n)| n)
}

/// The plain n-value expansion of a [`PLAIN_N`] shorthand: every token through the shorthand's own validator `one`
/// (any refusal refuses the declaration), 1-4 values through [`four`] (n = 4) or the 1-2 collapse (n = 2), zipped
/// with its [`SHORTHANDS`] longhands.
fn plain_expand(prop: &str, n: u8, toks: &[CV], one: impl Fn(&CV) -> Option<String>) -> Result<Vec<(String, String)>, ()> {
    let vals: Vec<String> = split_values(toks).iter().map(|c| one(c)).collect::<Option<Vec<String>>>().ok_or(())?;
    let vals: Vec<String> = match (n, vals.len()) {
        (4, _) => four(&vals).ok_or(())?.to_vec(),
        (2, 1) => vec![vals[0].clone(), vals[0].clone()],
        (2, 2) => vals,
        _ => return Err(()),
    };
    Ok(longhands(prop).ok_or(())?.into_iter().zip(vals).collect())
}

#[cfg(test)]
mod shorthand_table_expand_tests {
    use super::*;

    /// Four distinct valid tokens of a [`PLAIN_N`] shorthand (the first n are used).
    fn sample(sh: &str) -> [&'static str; 4] {
        match sh {
            "border-style" => ["solid", "dotted", "dashed", "double"],
            "border-color" => ["red", "lime", "blue", "yellow"],
            "gap" => ["5px", "10px", "", ""],
            "overflow" => ["hidden", "auto", "", ""],
            "place-content" | "place-items" | "place-self" => ["center", "start", "", ""],
            _ => ["1px", "2px", "3px", "4px"],
        }
    }

    fn values(sh: &str, k: usize) -> String {
        sample(sh)[..k].join(" ")
    }

    fn expand_str(prop: &str, value: &str) -> Result<Vec<(String, String)>, ()> {
        expand(prop, &css_core::parser::parse_component_values(value))
    }

    fn pairs(prop: &str, value: &str) -> Vec<(String, String)> {
        expand_str(prop, value).unwrap_or_else(|_| panic!("{prop}: {value} expands"))
    }

    fn get(p: &[(String, String)], name: &str) -> String {
        p.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()).unwrap_or_default()
    }

    #[test]
    fn shorthand_table_expand_every_row_one_and_n() {
        for &(sh, n) in PLAIN_N {
            let n = n as usize;
            let want = longhands(sh).unwrap();
            let one = pairs(sh, sample(sh)[0]);
            assert_eq!(one.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(), want, "{sh}: one value names");
            assert!(one.iter().all(|(_, v)| *v == one[0].1), "{sh}: one value is every longhand");
            let all = pairs(sh, &values(sh, n));
            assert_eq!(all.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(), want, "{sh}: n values names");
            for (i, (_, v)) in all.iter().enumerate() {
                assert!(all[..i].iter().all(|(_, w)| w != v), "{sh}: n values are distinct");
            }
            assert_eq!(all.len(), n, "{sh}: n longhands");
        }
    }

    #[test]
    fn shorthand_table_expand_margin_two() {
        let p = pairs("margin", "1px 2px");
        assert_eq!((get(&p, "margin-top"), get(&p, "margin-bottom")), ("1px".to_string(), "1px".to_string()));
        assert_eq!((get(&p, "margin-left"), get(&p, "margin-right")), ("2px".to_string(), "2px".to_string()));
    }

    #[test]
    fn shorthand_table_expand_border_style_two() {
        let p = pairs("border-style", "solid none");
        let got: Vec<&str> = p.iter().map(|(_, v)| v.as_str()).collect();
        assert_eq!(got, ["solid", "none", "solid", "none"]);
    }

    #[test]
    fn shorthand_table_expand_border_color_three() {
        let p = pairs("border-color", "red lime blue");
        assert_eq!(p.len(), 4);
        assert_eq!(get(&p, "border-left-color"), get(&p, "border-right-color"));
        assert_ne!(get(&p, "border-top-color"), get(&p, "border-bottom-color"));
        assert_ne!(get(&p, "border-top-color"), get(&p, "border-right-color"));
    }

    #[test]
    fn shorthand_table_expand_gap_overflow_two() {
        let p = pairs("gap", "5px 10px");
        assert_eq!((get(&p, "row-gap"), get(&p, "column-gap")), ("5px".to_string(), "10px".to_string()));
        let p = pairs("overflow", "hidden auto");
        assert_eq!((get(&p, "overflow-x"), get(&p, "overflow-y")), ("hidden".to_string(), "auto".to_string()));
    }

    #[test]
    fn shorthand_table_expand_border_radius_four() {
        let p = pairs("border-radius", "1px 2px 3px 4px");
        let got: Vec<(&str, &str)> = p.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
        assert_eq!(
            got,
            [
                ("border-top-left-radius", "1px"),
                ("border-top-right-radius", "2px"),
                ("border-bottom-right-radius", "3px"),
                ("border-bottom-left-radius", "4px"),
            ]
        );
    }

    #[test]
    fn shorthand_table_expand_bad_token_refuses() {
        for &(sh, n) in PLAIN_N {
            let n = n as usize;
            for bad in 0..n {
                let mut toks: Vec<&str> = sample(sh)[..n].to_vec();
                toks[bad] = "7";
                let value = toks.join(" ");
                let mut b = Block::parse("color: red");
                let before = b.serialize();
                assert!(!b.set(sh, &css_core::parser::parse_component_values(&value), false), "{sh}: {value} refused");
                assert_eq!(b.serialize(), before, "{sh}: {value} leaves the block unchanged");
            }
        }
    }

    #[test]
    fn shorthand_table_expand_too_many_refused() {
        for &(sh, n) in PLAIN_N {
            let s = sample(sh);
            let value = if n == 4 { format!("{} {}", s.join(" "), s[0]) } else { format!("{} {} {}", s[0], s[1], s[0]) };
            assert!(expand_str(sh, &value).is_err(), "{sh}: {value} refused");
        }
    }

    #[test]
    fn shorthand_table_expand_round_trip() {
        for &(sh, n) in PLAIN_N {
            let value = values(sh, n as usize);
            let first = pairs(sh, &value);
            let mut b = Block::default();
            assert!(b.set(sh, &css_core::parser::parse_component_values(&value), false), "{sh}: {value} sets");
            let s = b.get(sh);
            assert_eq!(pairs(sh, &s), first, "{sh}: {value} -> {s} -> expand is identity");
        }
    }

    #[test]
    fn shorthand_table_expand_radius_slash_opaque() {
        let p = pairs("border-radius", "1px / 2px");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "border-radius");
    }
}

// ---- AETHERCSSOMPLACEEXPAND (LEDGER SR1087): place-* as PLAIN_N n = 2 rows (named tail block) ----

/// AETHERCSSOMPLACEEXPAND (SR1087): css-align-3 §6.4 / §7.4 / §8.3's two-longhand shorthands (align-* then justify-*),
/// the [`SHORTHANDS`] rows past SR786's capture; each is a [`PLAIN_N`] n = 2 row expanded through [`alignment_expand`] (SR1102).
#[cfg(test)]
const PLACE_SHORTHANDS: [&str; 3] = ["place-content", "place-items", "place-self"];

#[cfg(test)]
mod cssom_place_tests {
    use super::*;

    fn cv(value: &str) -> Vec<CV> {
        css_core::parser::parse_component_values(value)
    }

    #[test]
    fn cssom_place_items_one_value_sets_both() {
        let b = Block::parse("place-items: center");
        assert_eq!(b.get("align-items"), "center");
        assert_eq!(b.get("justify-items"), "center");
        assert_eq!(b.get("place-items"), "center");
    }

    #[test]
    fn cssom_place_content_two_values() {
        let b = Block::parse("place-content: start end");
        assert_eq!(b.get("align-content"), "start");
        assert_eq!(b.get("justify-content"), "end");
        assert_eq!(b.get("place-content"), "start end");
    }

    #[test]
    fn cssom_place_self_two_values() {
        let b = Block::parse("place-self: center start");
        assert_eq!((b.get("align-self"), b.get("justify-self")), ("center".to_string(), "start".to_string()));
        assert_eq!(b.get("place-self"), "center start");
    }

    #[test]
    fn cssom_place_camel_case_setter_expands() {
        assert_eq!(prop_from_attr("placeSelf"), "place-self");
        assert_eq!(prop_from_attr("placeItems"), "place-items");
        assert_eq!(prop_from_attr("placeContent"), "place-content");
        let mut b = Block::default();
        assert!(b.set(&prop_from_attr("placeSelf"), &cv("auto"), false));
        assert_eq!((b.get("align-self"), b.get("justify-self")), ("auto".to_string(), "auto".to_string()));
        assert_eq!(b.get("place-self"), "auto");
    }

    #[test]
    fn cssom_place_remove_removes_both() {
        let mut b = Block::parse("place-content: center; color: red");
        assert!(b.remove("place-content"));
        assert_eq!(b.get("align-content"), "");
        assert_eq!(b.get("justify-content"), "");
        assert_eq!(b.serialize(), "color: red;");
    }

    #[test]
    fn cssom_place_three_values_refused() {
        let mut b = Block::parse("color: red");
        let before = b.serialize();
        assert!(!b.set("place-items", &cv("center start end"), false));
        assert_eq!(b.serialize(), before);
    }

    /// AETHERCSSOMALIGNWORDSONE (SR1102) flipped this pin: place-* and the align-* longhands validate through layout_core's
    /// alignment grammar ([`alignment_expand`]), so an unknown identifier is REFUSED (it was stored to both longhands).
    #[test]
    fn cssom_place_ident_only_validator() {
        let b = Block::parse("place-items: bogus");
        assert_eq!((b.get("align-items"), b.get("justify-items")), (String::new(), String::new()), "SR1102: refused (was stored)");
        let mut b = Block::default();
        assert!(!b.set("place-items", &cv("10px"), false));
        assert!(!b.set("place-self", &cv("center 7"), false));
        assert_eq!(b.serialize(), "");
    }

    #[test]
    fn cssom_place_css_wide_and_case() {
        let b = Block::parse("place-items: inherit");
        assert_eq!((b.get("align-items"), b.get("justify-items")), ("inherit".to_string(), "inherit".to_string()));
        assert_eq!(b.get("place-items"), "inherit");
        assert_eq!(Block::parse("place-self: CENTER Start").get("place-self"), "center start");
    }

    #[test]
    fn cssom_place_css_text_round_trip() {
        assert_eq!(Block::parse("align-items: center; justify-items: center").serialize(), "place-items: center;");
        assert_eq!(Block::parse("place-self: auto start").serialize(), "place-self: auto start;");
        assert_eq!(Block::parse("align-items: center").serialize(), "align-items: center;");
        assert_eq!(Block::parse("place-content: center; justify-content: end").serialize(), "place-content: center end;");
    }

    #[test]
    fn cssom_place_table_pins() {
        assert_eq!(SHORTHANDS.len(), 22);
        assert_eq!(PLAIN_N.len(), 12);
        for (sh, a, j) in [
            ("place-content", "align-content", "justify-content"),
            ("place-items", "align-items", "justify-items"),
            ("place-self", "align-self", "justify-self"),
        ] {
            assert!(PLACE_SHORTHANDS.contains(&sh));
            assert_eq!(plain_n(sh), Some(2), "{sh}");
            assert_eq!(longhands(sh), Some(vec![a.to_string(), j.to_string()]), "{sh}");
            assert_eq!(shorthands_of(a), vec![sh]);
            assert_eq!(shorthands_of(j), vec![sh]);
        }
    }
}

// ---- AETHERCSSOMBOUNDINGSTICKY (LEDGER SR1144): a stuck sticky box's CSSOM rects read at its held place (named tail block) ----

/// AETHERCSSOMBOUNDINGSTICKY (SR1144): the sticky root that moves box `t` (border-box origin (`x`, `y`) in document space,
/// before any fixed origin), and the root's parent document origin (`placed_origin`'s `abs`): a fixed box drops the hold
/// (the painter places it against the viewport); otherwise the parent's hold is inherited (the painter places
/// descendants from the parent's placed origin, so the OUTERMOST sticky root owns its subtree: an inner root's own
/// extra hold is owed AETHER-CSSOM-BOUNDING-STICKY-NESTED); otherwise a sticky box outside any pinned subtree starts one.
/// Records `t`'s hold in `hold_of` for its children (the walk pops a parent before its children).
#[allow(clippy::too_many_arguments)]
fn sticky_hold(
    tree: &crate::layout::LayoutTree,
    t: taffy::NodeId,
    x: f32,
    y: f32,
    l: &taffy::Layout,
    pin: (bool, bool),
    fixed: bool,
    hold_of: &mut HashMap<taffy::NodeId, (taffy::NodeId, (f32, f32))>,
) -> Option<(taffy::NodeId, (f32, f32))> {
    if fixed {
        return None;
    }
    let inherited = tree.taffy.parent(t).and_then(|p| hold_of.get(&p).copied());
    let sticky = pin == (false, false) && tree.paint_map.get(&t).and_then(|p| p.position_kind) == Some(4);
    let hold = inherited.or(if sticky { Some((t, (x - l.location.x, y - l.location.y))) } else { None });
    if let Some(h) = hold {
        hold_of.insert(t, h);
    }
    hold
}

/// AETHERCSSOMBOUNDINGSTICKY (SR1144): element `id`'s flow origin (`x`, `y`) shifted by its sticky root's hold for the
/// current `scroll` — the root's place from the painter's ONE origin fn (`render::placed_origin`, with the painter's
/// truncated scroll and viewport size, as the shell's `hit_origin` calls it) minus its flow place. Unchanged for an
/// element outside every sticky subtree.
fn sticky_held_at(f: &Flushed, id: NodeId, x: f32, y: f32, scroll: (f64, f64)) -> (f32, f32) {
    let Some(&(root, (ax, ay))) = f.held.get(&id) else { return (x, y) };
    let Ok(l) = f.tree.taffy.layout(root) else { return (x, y) };
    let (vw, vh) = page(|p| p.viewport);
    let (hx, hy) = crate::render::placed_origin(&f.tree, root, ax, ay, l, vw as u32, vh as u32, scroll.0 as i32, scroll.1 as i32);
    (x + hx - (ax + l.location.x), y + hy - (ay + l.location.y))
}

// ---- AETHERCSSOMALIGNWORDSONE (LEDGER SR1102): place-* and the six align longhands through layout_core's alignment grammar (named tail block) ----

/// AETHERCSSOMALIGNWORDSONE (SR1102): a place-* shorthand or an align- / justify- content / items / self longhand.
/// Idents only (lowercased), grouped into alignment values through `layout_core::css::alignment_words` (`safe center` /
/// `first baseline` is ONE value), 1..=n groups for its n longhands (a shorthand's one value fills both halves), each
/// (longhand, value) through `layout_core::css::alignment_valid`; any refusal refuses the declaration.
fn alignment_expand(prop: &str, toks: &[CV]) -> Result<Vec<(String, String)>, ()> {
    let words: Vec<String> = split_values(toks).iter().map(|c| ident_of(c).map(|i| i.to_ascii_lowercase())).collect::<Option<Vec<String>>>().ok_or(())?;
    let mut groups: Vec<String> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let pair = words.get(i + 1).map(|b| format!("{} {}", words[i], b)).filter(|p| layout_core::css::alignment_words(p).is_some());
        i += if pair.is_some() { 2 } else { 1 };
        groups.push(pair.unwrap_or_else(|| words[i - 1].clone()));
    }
    let ls = longhands(prop).unwrap_or_else(|| vec![prop.to_string()]);
    if groups.is_empty() || groups.len() > ls.len() {
        return Err(());
    }
    let out: Vec<(String, String)> = ls.into_iter().enumerate().map(|(k, l)| (l, groups[k.min(groups.len() - 1)].clone())).collect();
    if out.iter().all(|(l, v)| layout_core::css::alignment_valid(l, v)) { Ok(out) } else { Err(()) }
}

#[cfg(test)]
mod cssom_align_words_one_tests {
    use super::*;

    fn cv(value: &str) -> Vec<CV> {
        css_core::parser::parse_component_values(value)
    }

    fn pair(b: &Block, a: &str, j: &str) -> (String, String) {
        (b.get(a), b.get(j))
    }

    #[test]
    fn cssom_align_words_one_place_items_safe_center() {
        let b = Block::parse("place-items: safe center");
        assert_eq!(pair(&b, "align-items", "justify-items"), ("safe center".to_string(), "safe center".to_string()), "one group, both halves (was safe / center)");
        assert_eq!(b.get("place-items"), "safe center");
    }

    #[test]
    fn cssom_align_words_one_place_content_two_groups() {
        let b = Block::parse("place-content: first baseline space-between");
        assert_eq!(pair(&b, "align-content", "justify-content"), ("first baseline".to_string(), "space-between".to_string()));
        assert_eq!(b.get("place-content"), "first baseline space-between");
        let b = Block::parse("place-content: unsafe end safe start");
        assert_eq!(pair(&b, "align-content", "justify-content"), ("unsafe end".to_string(), "safe start".to_string()));
    }

    #[test]
    fn cssom_align_words_one_place_self_unchanged() {
        let b = Block::parse("place-self: center start");
        assert_eq!(pair(&b, "align-self", "justify-self"), ("center".to_string(), "start".to_string()));
        assert_eq!(b.get("place-self"), "center start");
    }

    #[test]
    fn cssom_align_words_one_bogus_refused() {
        let mut b = Block::parse("color: red");
        for (p, v) in [("place-items", "bogus"), ("align-items", "bogus"), ("align-items", "space-between"), ("align-items", "auto"), ("justify-content", "first baseline"), ("align-self", "left"), ("align-content", "safe"), ("place-items", "safe safe center")] {
            assert!(!b.set(p, &cv(v), false), "{p}: {v} refused (armed: it was stored)");
        }
        assert_eq!(b.serialize(), "color: red;");
    }

    #[test]
    fn cssom_align_words_one_longhands_stored() {
        let mut b = Block::default();
        for (p, v) in [("align-items", "safe center"), ("justify-content", "space-evenly"), ("justify-self", "LEFT"), ("align-self", "last baseline"), ("justify-items", "self-end")] {
            assert!(b.set(p, &cv(v), false), "{p}: {v} stored");
            assert_eq!(b.get(p), v.to_ascii_lowercase(), "{p}: {v} is one value");
        }
    }

    #[test]
    fn cssom_align_words_one_three_groups_refused() {
        let mut b = Block::default();
        for v in ["safe center start end", "center start end", "first baseline center end"] {
            assert!(!b.set("place-items", &cv(v), false), "{v}: three groups refused");
        }
        assert!(!b.set("align-items", &cv("center start"), false), "a longhand takes one group");
        assert_eq!(b.serialize(), "");
    }

    #[test]
    fn cssom_align_words_one_round_trip_collapses() {
        assert_eq!(Block::parse("align-items: safe center; justify-items: safe center").serialize(), "place-items: safe center;");
        assert_eq!(Block::parse("align-content: first baseline; justify-content: space-between").serialize(), "place-content: first baseline space-between;");
        assert_eq!(Block::parse("align-items: safe center; justify-items: center").get("place-items"), "safe center center");
        assert_eq!(Block::parse("place-items: safe center center").get("justify-items"), "center");
    }

    /// The pin: every word each layout_core arm maps (the justify-content arm, apply_align_content, self_alignment), and
    /// its safe / first-baseline forms, is valid for that property; every value alignment_valid takes, alignment_words takes.
    #[test]
    fn cssom_align_words_one_superset_of_layout_arms() {
        use layout_core::css::{alignment_valid, alignment_words};
        let jc: &[&str] = &["center", "flex-end", "end", "flex-start", "start", "normal", "space-between", "space-around", "space-evenly", "safe center", "unsafe end"];
        let ac: &[&str] = &["normal", "flex-start", "start", "flex-end", "end", "center", "stretch", "space-between", "space-around", "space-evenly", "safe center", "first baseline"];
        let items: &[&str] = &["center", "flex-end", "end", "flex-start", "start", "baseline", "stretch", "normal", "first baseline", "last baseline", "safe center"];
        for (props, words) in [(&["justify-content"][..], jc), (&["align-content"][..], ac), (&["align-items", "justify-items", "align-self", "justify-self"][..], items)] {
            for p in props {
                for w in words {
                    assert!(alignment_valid(p, w), "{p}: {w} (the layout arm maps it)");
                }
            }
        }
        for p in ["align-self", "justify-self"] {
            assert!(alignment_valid(p, "auto"), "{p}: auto");
        }
        let probe = ["bogus", "safe", "left", "right", "self-start", "safe left", "auto", "safe stretch", "safe space-between", "baseline first", "center start", "a b c"];
        for p in ["align-content", "justify-content", "align-items", "justify-items", "align-self", "justify-self"] {
            for w in jc.iter().chain(ac).chain(items).chain(probe.iter()) {
                assert!(!alignment_valid(p, w) || alignment_words(w).is_some(), "{p}: {w} valid only through alignment_words");
            }
            assert!(!alignment_valid(p, "bogus"), "{p}: bogus refused");
        }
    }
}

// ---- AETHERCSSOMOFFSETTOPSTICKY (LEDGER SR1149): offsetTop / offsetLeft read a stuck sticky box at its held place (named tail block) ----

/// AETHERCSSOMOFFSETTOPSTICKY (SR1149): element `el`'s first border-box rect in document space with its origin through
/// SR1144's ONE read-time hold ([`sticky_held_at`] at the page's scroll: document space, before any scroll subtraction),
/// so `box_metric`'s offsetTop / offsetLeft read a stuck sticky box (and its subtree) where it is painted, as
/// getBoundingClientRect does. Width and height are untouched (a hold moves, it does not resize). Read for both the
/// element and its offsetParent: inside the parent's own stuck subtree both shift equally and the difference holds.
fn offset_held_first(f: &Flushed, el: usize) -> Option<(f32, f32, f32, f32)> {
    let (x, y, w, h) = f.boxes.get(&NodeId(el)).and_then(|v| v.first().copied())?;
    let (x, y) = sticky_held_at(f, NodeId(el), x, y, page(|p| p.scroll));
    Some((x, y, w, h))
}

// ---- AETHERCSSOMSHORTHANDTABLEINITIAL (LEDGER SR1086): a shorthand's initial derived from its longhands' initials (named tail block) ----

/// AETHERCSSOMSHORTHANDTABLEINITIAL (SR1086): a [`SHORTHANDS`] row's initial value, DERIVED: a [`Block`] of the row's
/// longhands each at its own [`initial_value`], serialized by [`serialize_shorthand`] — so a shorthand's initial and
/// its longhands' rows cannot disagree. None for a name that is not a row, for a row with a longhand that has no
/// initial row (its value is the report's or has no static form), and for a row whose grammar does not serialize at
/// initial (owed AETHER-CSSOM-INITIAL-GRAMMAR-ROWS). Built on SR1092's [`shorthand_from`].
fn shorthand_initial(sh: &str) -> Option<String> {
    shorthand_from(sh, |l| initial_value(l).to_string())
}

/// The initial value of `prop` as getComputedStyle serializes it: a shorthand's derived through
/// [`shorthand_initial`], else the longhand's [`initial_value`] row ("" for a property with neither).
fn initial_of(prop: &str) -> String {
    shorthand_initial(prop).unwrap_or_else(|| initial_value(prop).to_string())
}

#[cfg(test)]
mod shorthand_table_initial_tests {
    use super::*;

    /// The rows whose longhands have no static initial row (colours resolve currentcolor per element; border-image-*
    /// and most background-* have no `initial_value` row), so they read "" (owed AETHER-CSSOM-INITIAL-GRAMMAR-ROWS).
    const NOT_DERIVED: [&str; 9] =
        ["border", "border-image", "border-color", "border-top", "border-right", "border-bottom", "border-left", "outline", "background"];

    #[test]
    fn shorthand_table_initial_rows_derive() {
        for (sh, _) in SHORTHANDS {
            let got = shorthand_initial(sh);
            if NOT_DERIVED.contains(sh) {
                assert_eq!(got, None, "{sh}: a named non-deriving row");
            } else {
                assert!(got.as_deref().is_some_and(|v| !v.is_empty()), "{sh}: derives a non-empty initial, got {got:?}");
            }
        }
        for sh in NOT_DERIVED {
            assert!(SHORTHANDS.iter().any(|(s, _)| *s == sh), "{sh} is a SHORTHANDS row");
        }
    }

    /// SR1092: no shorthand row is kept — `overflow` reads `visible` through `computed_value`'s fallback composing its two
    /// longhands (`shorthand_computed`), so the exception SR1086 kept is gone.
    #[test]
    fn shorthand_table_initial_no_shorthand_in_initial_value() {
        for (sh, _) in SHORTHANDS {
            assert_eq!(initial_value(sh), "", "{sh}: no hand-written shorthand row in initial_value");
        }
    }

    #[test]
    fn shorthand_table_initial_byte_identity() {
        // The two literals initial_value carried before SR1086.
        assert_eq!(initial_of("font-synthesis"), "weight style small-caps");
        assert_eq!(initial_of("overflow"), "visible");
    }

    #[test]
    fn shorthand_table_initial_pins() {
        for (prop, want) in [
            ("gap", "normal"),
            ("margin", "0px"),
            ("padding", "0px"),
            ("overflow", "visible"),
            ("border-radius", "0px"),
            ("inset", "auto"),
            ("flex", "0 1 auto"),
            ("border-width", "0px"),
            ("border-style", "none"),
            ("opacity", "1"),
            ("overflow-x", "visible"),
            ("bogus", ""),
        ] {
            assert_eq!(initial_of(prop), want, "{prop}: initial");
        }
    }

    /// getComputedStyle through `cascaded_value`'s `initial` closure: a declared CSS-wide `initial` on a shorthand reads
    /// the derived initial. SR1092: an undeclared shorthand composes from its longhands' computed values at
    /// `computed_value`'s fallback (`shorthand_computed`), so #a's undeclared gap reads `normal` (was "").
    #[test]
    fn shorthand_table_initial_computed_gap() {
        let html = r#"<!doctype html><html><body><div id="a"></div><div id="g" style="gap: initial"></div><div id="r" style="border-radius: initial"></div></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        for (id, prop, want) in [("g", "gap", "normal"), ("r", "border-radius", "0px"), ("a", "gap", "normal")] {
            let got = engine
                .js_engine
                .as_mut()
                .unwrap()
                .eval_string(&format!("getComputedStyle(document.getElementById('{id}')).getPropertyValue('{prop}')"))
                .unwrap();
            assert_eq!(got, want, "#{id}: computed {prop}");
        }
    }
}

// ---- AETHERCSSOMWHITESPACELONGHANDSACCESSOR (LEDGER SR1109): css-text-4's white-space longhands as CSSStyleDeclaration accessors (named tail block) ----

/// AETHERCSSOMWHITESPACELONGHANDSACCESSOR (SR1109): `white-space-collapse`, `text-wrap-mode` and `text-wrap` are PROPERTIES
/// names, so `install`'s table loop gives them camelCase and dashed accessors through `prop_from_attr` (no accessor code).
#[cfg(test)]
mod white_space_longhands_accessor_tests {
    use super::*;

    const NAMES: [(&str, &str); 3] =
        [("white-space-collapse", "whiteSpaceCollapse"), ("text-wrap-mode", "textWrapMode"), ("text-wrap", "textWrap")];

    /// PROPERTIES stays strictly sorted (and so free of duplicates): every insert lands in place.
    #[test]
    fn cssom_white_space_longhands_accessor_properties_sorted() {
        assert!(PROPERTIES.windows(2).all(|w| w[0] < w[1]), "PROPERTIES is strictly sorted");
    }

    /// Each name is a PROPERTIES entry whose camelCase accessor maps back to it.
    #[test]
    fn cssom_white_space_longhands_accessor_names_round_trip() {
        for (dashed, cc) in NAMES {
            assert!(PROPERTIES.contains(&dashed), "{dashed} in PROPERTIES");
            assert_eq!(camel(dashed), cc, "camel({dashed})");
            assert_eq!(prop_from_attr(cc), dashed, "prop_from_attr({cc})");
        }
    }

    /// None is a SHORTHANDS row or a longhand of one (the white-space / text-wrap rows are owed).
    #[test]
    fn cssom_white_space_longhands_accessor_no_shorthand() {
        for (dashed, _) in NAMES {
            assert!(shorthands_of(dashed).is_empty(), "shorthands_of({dashed})");
            assert!(longhands(dashed).is_none(), "longhands({dashed})");
        }
    }

    /// The declaration block stores and serializes the longhand as written.
    #[test]
    fn cssom_white_space_longhands_accessor_block_serializes() {
        let mut b = Block::parse("");
        assert!(b.set("white-space-collapse", &css_core::parser::parse_component_values("preserve"), false));
        assert_eq!(b.get("white-space-collapse"), "preserve");
        assert_eq!(b.serialize(), "white-space-collapse: preserve;");
    }
}

// ---- AETHERCSSOMSHORTHANDCOMPUTED (LEDGER SR1092): a shorthand's computed value from its longhands' computed values (named tail block) ----

/// AETHERCSSOMSHORTHANDCOMPUTED (SR1092): the ONE shorthand builder — a [`Block`] of the [`SHORTHANDS`] row's longhands
/// each at `value_of(longhand)`, serialized by [`serialize_shorthand`]. None for a name that is not a row, for a row with
/// a longhand whose value is "" (SR1086's None-on-empty rule), and for a row whose grammar does not serialize those
/// values. [`shorthand_initial`] passes [`initial_value`], [`shorthand_computed`] passes [`computed_value`].
fn shorthand_from(sh: &str, value_of: impl Fn(&str) -> String) -> Option<String> {
    let mut b = Block::default();
    for name in longhands(sh)? {
        let value = value_of(name.as_str());
        if value.is_empty() {
            return None;
        }
        b.decls.push(Decl { name, value, important: false });
    }
    serialize_shorthand(&b, sh).map(|x| x.0)
}

/// AETHERCSSOMSHORTHANDCOMPUTED (SR1092): a shorthand's computed value is the serialization of its longhands' computed
/// values (each through [`computed_value`]: report column, cascaded, inherited or initial on its own) — the SR764
/// `font_synthesis_computed` road generalised. `computed_value` takes it at its fallback, so an undeclared or partly
/// declared shorthand (`margin-top: 5px` alone) composes (`5px 0px 0px`) and a fully declared one keeps its cascaded
/// read. None where a longhand has no computed value or the grammar does not serialize (border with no colour row;
/// owed AETHER-CSSOM-BORDER-COMPUTED).
fn shorthand_computed(el: usize, sh: &str) -> Option<String> {
    shorthand_from(sh, |l| computed_value(el, l))
}

#[cfg(test)]
mod shorthand_computed_tests {
    use super::*;

    /// The derived initials SR1086 pinned, through the shared builder (byte-identical after the refactor).
    #[test]
    fn shorthand_computed_initial_unchanged() {
        for (prop, want) in [
            ("font-synthesis", "weight style small-caps"),
            ("overflow", "visible"),
            ("gap", "normal"),
            ("margin", "0px"),
            ("place-items", "normal"),
            ("border", ""),
        ] {
            assert_eq!(initial_of(prop), want, "{prop}: initial");
        }
        assert_eq!(initial_value("overflow"), "", "SR1092: the overflow exception is gone from initial_value");
    }

    #[test]
    fn shorthand_computed_reads() {
        let html = r#"<!doctype html><html><body><div id="a"></div><div id="m" style="margin-top:5px"></div><div id="o" style="overflow-x:hidden"></div><div id="f" style="margin: 1em"></div><div id="p" style="align-items:center; justify-items:center"></div><div id="s" style="font-synthesis-weight:none"></div></body></html>"#;
        let mut engine = crate::AetherEngine::new();
        engine.handle_event(crate::api::events::Event::Resize(400, 300));
        engine.load_html("https://example.com/", html, true);
        engine.render_frame();
        let mut read = |target: &str, prop: &str| {
            engine
                .js_engine
                .as_mut()
                .unwrap()
                .eval_string(&format!("getComputedStyle({target}).getPropertyValue('{prop}')"))
                .unwrap()
        };
        let by_id = |id: &str| format!("document.getElementById('{id}')");
        for (id, prop, want) in [
            ("m", "margin", "5px 0px 0px"),
            ("a", "margin", "0px"),
            ("a", "padding", "0px"),
            ("a", "gap", "normal"),
            ("a", "overflow", "visible"),
            ("a", "border-radius", "0px"),
            ("a", "inset", "auto"),
            ("a", "flex", "0 1 auto"),
            ("a", "border-width", "0px"),
            ("a", "border-style", "none"),
            // Owed AETHER-CSSOM-BORDER-COMPUTED / AETHER-CSSOM-INITIAL-GRAMMAR-ROWS: a colour or border-image longhand
            // has no computed row, so these rows still read "".
            ("a", "border", ""),
            ("a", "border-color", ""),
            ("a", "outline", ""),
            ("a", "background", ""),
            ("o", "overflow", "hidden visible"),
            // A fully declared shorthand keeps the cascaded road (the composition sits at the fallback).
            ("f", "margin", "1em"),
            ("p", "place-items", "center"),
            ("s", "font-synthesis", "style small-caps"),
        ] {
            assert_eq!(read(&by_id(id), prop), want, "#{id}: computed {prop}");
        }
        // AETHERCSSOMBOXSIDECOLUMNS (SR1096): every margin side has a report column, so the UA body margin composes as
        // Chromium reads it (it read `8px 0px 0px` while only margin-top had a column).
        assert_eq!(read("document.body", "margin"), "8px", "body: computed margin");
    }
}
