
use crate::layout::{LayoutTree, PaintStyle};
use cssparser::{Parser, ParserInput, Token};
use taffy::prelude::*;
use taffy::style::{Dimension, Display, FlexDirection, LengthPercentage, LengthPercentageAuto};
use taffy::geometry::Rect;

/// A simple selector this cascade can match. Anything more complex
/// (descendant combinators, pseudo-classes, attribute selectors) is
/// recorded in the ledger and the rule is skipped, not misapplied.
#[derive(Debug, Clone)]
enum SimpleSelector {
    Tag(String),
    Class(String),
    Id(String),
}

impl SimpleSelector {
    fn matches(&self, el: &kuchiki::ElementData) -> bool {
        match self {
            SimpleSelector::Tag(t) => el.name.local.as_ref() == t,
            SimpleSelector::Class(c) => el
                .attributes
                .borrow()
                .get("class")
                .map(|v| v.split_whitespace().any(|cls| cls == c))
                .unwrap_or(false),
            SimpleSelector::Id(i) => el
                .attributes
                .borrow()
                .get("id")
                .map(|v| v == i.as_str())
                .unwrap_or(false),
        }
    }
}

/// Declarations a rule actually specified. Only `Some` fields are applied,
/// so a rule never stomps another rule's (or the UA default's) values.
#[derive(Default)]
struct SpecifiedStyle {
    display: Option<Display>,
    flex_direction: Option<FlexDirection>,
    width: Option<Dimension>,
    height: Option<Dimension>,
    padding: Option<LengthPercentage>,
    margin: Option<LengthPercentageAuto>,
    paint: PaintStyle,
}

pub fn apply_css(layout_tree: &mut LayoutTree, css: &str) {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);

    loop {
        // Parse the selector list up to the next `{`.
        let mut selectors: Vec<SimpleSelector> = Vec::new();
        let mut current: Option<SimpleSelector> = None;
        let mut complex = false;
        let mut saw_block = false;

        loop {
            let Ok(token) = parser.next_including_whitespace() else { break };
            match token {
                Token::CurlyBracketBlock => {
                    saw_block = true;
                    break;
                }
                Token::WhiteSpace(_) => {
                    // A second component after whitespace = descendant combinator.
                    if current.is_some() {
                        // Peek continues in the outer loop; mark complex only
                        // if another component follows before `{`.
                    }
                }
                Token::Comma => {
                    if let Some(sel) = current.take() {
                        selectors.push(sel);
                    }
                }
                Token::Ident(name) => {
                    if current.is_some() {
                        complex = true; // "div p" — descendant, unsupported
                    }
                    current = Some(SimpleSelector::Tag(name.as_ref().to_string()));
                }
                Token::Delim('.') => {
                    if let Ok(Token::Ident(name)) = parser.next_including_whitespace() {
                        if current.is_some() {
                            complex = true; // compound like div.warn
                        }
                        current = Some(SimpleSelector::Class(name.as_ref().to_string()));
                    }
                }
                Token::Hash(v) | Token::IDHash(v) => {
                    if current.is_some() {
                        complex = true;
                    }
                    current = Some(SimpleSelector::Id(v.as_ref().to_string()));
                }
                Token::AtKeyword(kw) => {
                    crate::ledger::record_css(&format!("at-rule:@{}", kw));
                    complex = true;
                }
                _ => {
                    complex = true; // pseudo-classes, attributes, combinators...
                }
            }
        }
        if let Some(sel) = current.take() {
            selectors.push(sel);
        }
        if !saw_block {
            break; // end of stylesheet
        }
        if complex || selectors.is_empty() {
            crate::ledger::record_css("selector:unsupported-complex");
            let _ = parser.parse_nested_block(|p| {
                while p.next().is_ok() {}
                Ok::<(), cssparser::ParseError<'_, ()>>(())
            });
            continue;
        }

        {
            {
                {
                    let mut block_parser = parser.parse_nested_block(|p| {
                        let mut style = SpecifiedStyle::default();

                        while let Ok(token) = p.next() {
                            if let Token::Ident(prop_rc) = token {
                                let prop_name = prop_rc.clone();
                                if let Ok(Token::Colon) = p.next() {
                                    let mut first_val = None;
                                    if let Ok(val_token) = p.next() {
                                        first_val = Some(val_token.clone());
                                    }
                                    while let Ok(t) = p.next() {
                                        if let Token::Semicolon = t { break; }
                                    }
                                    
                                    match prop_name.as_ref() {
                                        "display" => {
                                            if let Some(Token::Ident(ident)) = first_val.as_ref() {
                                                match ident.as_ref() {
                                                    "flex" => style.display = Some(Display::Flex),
                                                    "block" => style.display = Some(Display::Flex),
                                                    "none" => style.display = Some(Display::None),
                                                    other => {
                                                        crate::ledger::record_css(&format!("display:{}", other));
                                                    }
                                                }
                                            }
                                        }
                                        "flex-direction" => {
                                            if let Some(Token::Ident(ident)) = first_val.as_ref() {
                                                match ident.as_ref() {
                                                    "row" => style.flex_direction = Some(FlexDirection::Row),
                                                    "column" => style.flex_direction = Some(FlexDirection::Column),
                                                    _ => {}
                                                }
                                            }
                                        }
                                        "width" => {
                                            style.width = parse_dimension(first_val.as_ref());
                                        }
                                        "height" => {
                                            style.height = parse_dimension(first_val.as_ref());
                                        }
                                        "padding" => {
                                            style.padding = parse_length_percentage(first_val.as_ref());
                                        }
                                        "margin" => {
                                            style.margin = parse_length_percentage_auto(first_val.as_ref());
                                        }
                                        "background-color" | "background" => {
                                            if !token_is_neutral(first_val.as_ref()) {
                                                match color_from_token(first_val.as_ref()) {
                                                    Some(c) => style.paint.background = Some(c),
                                                    None => crate::ledger::record_css("background-value:unsupported"),
                                                }
                                            }
                                        }
                                        "color" => {
                                            if !token_is_neutral(first_val.as_ref()) {
                                                match color_from_token(first_val.as_ref()) {
                                                    Some(c) => style.paint.color = Some(c),
                                                    None => crate::ledger::record_css("color-value:unsupported"),
                                                }
                                            }
                                        }
                                        "font-size" => match font_size_from_token(first_val.as_ref()) {
                                            Some(px) => style.paint.font_size = Some(px),
                                            None => crate::ledger::record_css("font-size-value:unsupported"),
                                        },
                                        "font-weight" => match font_weight_from_token(first_val.as_ref()) {
                                            Some(b) => style.paint.bold = Some(b),
                                            None => crate::ledger::record_css("font-weight-value:unsupported"),
                                        },
                                        other => {
                                            crate::ledger::record_css(&format!("property:{}", other));
                                        }
                                    }
                                }
                            }
                        }
                        
                        Ok::<SpecifiedStyle, cssparser::ParseError<'_, ()>>(style)
                    });

                    if let Ok(style) = block_parser {
                        // Apply only the specified declarations to matching nodes.
                        for (node_id, dom_node) in &layout_tree.node_map {
                            let Some(el) = dom_node.as_element() else { continue };
                            if !selectors.iter().any(|s| s.matches(el)) {
                                continue;
                            }
                            if let Ok(node_style_ref) = layout_tree.taffy.style(*node_id) {
                                let mut node_style = node_style_ref.clone();
                                if let Some(d) = style.display { node_style.display = d; }
                                if let Some(fd) = style.flex_direction { node_style.flex_direction = fd; }
                                if let Some(w) = style.width { node_style.size.width = w; }
                                if let Some(h) = style.height { node_style.size.height = h; }
                                if let Some(p) = &style.padding {
                                    node_style.padding = Rect { left: p.clone(), right: p.clone(), top: p.clone(), bottom: p.clone() };
                                }
                                if let Some(m) = &style.margin {
                                    node_style.margin = Rect { left: m.clone(), right: m.clone(), top: m.clone(), bottom: m.clone() };
                                }
                                let _ = layout_tree.taffy.set_style(*node_id, node_style);
                            }
                            let entry = layout_tree.paint_map.entry(*node_id).or_default();
                            if style.paint.background.is_some() { entry.background = style.paint.background; }
                            if style.paint.color.is_some() { entry.color = style.paint.color; }
                            if style.paint.font_size.is_some() { entry.font_size = style.paint.font_size; }
                            if style.paint.bold.is_some() { entry.bold = style.paint.bold; }
                        }
                    }
                }
            }
        }
    }

    // set_style only marks nodes dirty; the boxes are stale until recomputed.
    let _ = layout_tree.taffy.compute_layout(
        layout_tree.root_node,
        taffy::geometry::Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        },
    );
}

/// Parses a CSS color from a single cssparser token (named or #hex).
/// `rgb()`/`hsl()` functions are not yet supported — callers ledger them.
pub fn color_from_token(token: Option<&Token>) -> Option<(u8, u8, u8)> {
    match token {
        Some(Token::Ident(name)) => named_color(name.as_ref()),
        Some(Token::Hash(v)) | Some(Token::IDHash(v)) => hex_color(v.as_ref()),
        _ => None,
    }
}

/// Parses a CSS color from a string: named, #rgb/#rrggbb, or rgb(r, g, b).
pub fn parse_color_str(value: &str) -> Option<(u8, u8, u8)> {
    let v = value.trim();
    if let Some(hex) = v.strip_prefix('#') {
        return hex_color(hex);
    }
    if let Some(inner) = v.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        let mut parts = inner.split(',').map(|p| p.trim().parse::<u8>());
        if let (Some(Ok(r)), Some(Ok(g)), Some(Ok(b))) = (parts.next(), parts.next(), parts.next()) {
            return Some((r, g, b));
        }
        return None;
    }
    named_color(v)
}

/// Keywords that specify "no concrete value here" — not coverage gaps.
pub fn is_neutral_keyword(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "transparent" | "inherit" | "initial" | "unset" | "currentcolor" | "none"
    )
}

fn token_is_neutral(token: Option<&Token>) -> bool {
    matches!(token, Some(Token::Ident(v)) if is_neutral_keyword(v.as_ref()))
}

const BASE_FONT_PX: f32 = 16.0;

/// Parses a font-size string: px, em/rem (relative to the 16px UA base —
/// approximation, not parent-relative), %, or absolute keywords.
pub fn parse_font_size(value: &str) -> Option<f32> {
    let v = value.trim().to_ascii_lowercase();
    if let Some(px) = v.strip_suffix("px").and_then(|n| n.trim().parse::<f32>().ok()) {
        return Some(px);
    }
    if let Some(em) = v
        .strip_suffix("rem")
        .or_else(|| v.strip_suffix("em"))
        .and_then(|n| n.trim().parse::<f32>().ok())
    {
        return Some(em * BASE_FONT_PX);
    }
    if let Some(pct) = v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
        return Some(pct / 100.0 * BASE_FONT_PX);
    }
    match v.as_str() {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        _ => v.parse::<f32>().ok(),
    }
}

fn font_size_from_token(token: Option<&Token>) -> Option<f32> {
    match token {
        Some(Token::Dimension { value, unit, .. }) => match unit.as_ref() {
            "px" => Some(*value),
            "em" | "rem" => Some(value * BASE_FONT_PX),
            _ => None,
        },
        Some(Token::Percentage { unit_value, .. }) => Some(unit_value * BASE_FONT_PX),
        Some(Token::Ident(kw)) => parse_font_size(kw.as_ref()),
        _ => None,
    }
}

/// Parses a font-weight into "bold or not".
pub fn parse_font_weight(value: &str) -> Option<bool> {
    let v = value.trim().to_ascii_lowercase();
    match v.as_str() {
        "bold" | "bolder" => Some(true),
        "normal" | "lighter" => Some(false),
        _ => v.parse::<f32>().ok().map(|n| n >= 600.0),
    }
}

fn font_weight_from_token(token: Option<&Token>) -> Option<bool> {
    match token {
        Some(Token::Ident(kw)) => parse_font_weight(kw.as_ref()),
        Some(Token::Number { value, .. }) => Some(*value >= 600.0),
        _ => None,
    }
}

/// Parses "NNpx" (or a bare number) into pixels.
pub fn parse_px(value: &str) -> Option<f32> {
    let v = value.trim();
    v.strip_suffix("px").unwrap_or(v).trim().parse::<f32>().ok()
}

fn named_color(name: &str) -> Option<(u8, u8, u8)> {
    match name.to_ascii_lowercase().as_str() {
        "black" => Some((0, 0, 0)),
        "white" => Some((255, 255, 255)),
        "red" => Some((255, 0, 0)),
        "green" => Some((0, 128, 0)),
        "blue" => Some((0, 0, 255)),
        "yellow" => Some((255, 255, 0)),
        "orange" => Some((255, 165, 0)),
        "purple" => Some((128, 0, 128)),
        "gray" | "grey" => Some((128, 128, 128)),
        "silver" => Some((192, 192, 192)),
        "navy" => Some((0, 0, 128)),
        "teal" => Some((0, 128, 128)),
        "maroon" => Some((128, 0, 0)),
        "olive" => Some((128, 128, 0)),
        "aqua" | "cyan" => Some((0, 255, 255)),
        "fuchsia" | "magenta" => Some((255, 0, 255)),
        "lime" => Some((0, 255, 0)),
        other => {
            crate::ledger::record_css(&format!("named-color:{}", other));
            None
        }
    }
}

fn hex_color(hex: &str) -> Option<(u8, u8, u8)> {
    match hex.len() {
        3 => {
            let mut it = hex.chars().map(|c| c.to_digit(16).map(|d| (d * 17) as u8));
            if let (Some(Some(r)), Some(Some(g)), Some(Some(b))) = (it.next(), it.next(), it.next()) {
                return Some((r, g, b));
            }
            None
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((r, g, b))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_size_and_weight_parsing() {
        assert_eq!(parse_font_size("14px"), Some(14.0));
        assert_eq!(parse_font_size("1.5em"), Some(24.0));
        assert_eq!(parse_font_size("2rem"), Some(32.0));
        assert_eq!(parse_font_size("110%"), Some(17.6));
        assert_eq!(parse_font_size("medium"), Some(16.0));
        assert_eq!(parse_font_size("banana"), None);
        assert_eq!(parse_font_weight("bold"), Some(true));
        assert_eq!(parse_font_weight("400"), Some(false));
        assert_eq!(parse_font_weight("700"), Some(true));
        assert!(is_neutral_keyword("transparent"));
        assert!(is_neutral_keyword("Inherit"));
        assert!(!is_neutral_keyword("red"));
    }
}

fn parse_dimension<'i>(token: Option<&Token<'i>>) -> Option<Dimension> {
    match token {
        Some(Token::Ident(ident)) if ident.as_ref() == "auto" => Some(Dimension::auto()),
        Some(Token::Dimension { value, unit, .. }) if unit.as_ref() == "px" => Some(Dimension::length(*value)),
        Some(Token::Percentage { unit_value, .. }) => Some(Dimension::percent(*unit_value)),
        Some(Token::Number { value, .. }) => Some(Dimension::length(*value)),
        _ => None,
    }
}

fn parse_length_percentage<'i>(token: Option<&Token<'i>>) -> Option<LengthPercentage> {
    match token {
        Some(Token::Dimension { value, unit, .. }) if unit.as_ref() == "px" => Some(LengthPercentage::length(*value)),
        Some(Token::Percentage { unit_value, .. }) => Some(LengthPercentage::percent(*unit_value)),
        Some(Token::Number { value, .. }) => Some(LengthPercentage::length(*value)),
        _ => None,
    }
}

fn parse_length_percentage_auto<'i>(token: Option<&Token<'i>>) -> Option<LengthPercentageAuto> {
    match token {
        Some(Token::Ident(ident)) if ident.as_ref() == "auto" => Some(LengthPercentageAuto::auto()),
        Some(Token::Dimension { value, unit, .. }) if unit.as_ref() == "px" => Some(LengthPercentageAuto::length(*value)),
        Some(Token::Percentage { unit_value, .. }) => Some(LengthPercentageAuto::percent(*unit_value)),
        Some(Token::Number { value, .. }) => Some(LengthPercentageAuto::length(*value)),
        _ => None,
    }
}
