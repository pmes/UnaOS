use crate::layout::{LayoutTree, PaintStyle};
use cssparser::{Parser, ParserInput, Token};
use taffy::prelude::*;
use taffy::style::{Dimension, Display, FlexDirection, LengthPercentage, LengthPercentageAuto};
use taffy::geometry::Rect;

/// The viewport media queries are evaluated against (engine default surface).
const VIEWPORT_W: f32 = 800.0;

/// Declarations a rule actually specified. Only `Some` fields are applied,
/// so a rule never stomps another rule's (or the UA default's) values.
#[derive(Default)]
pub(crate) struct SpecifiedStyle {
    pub display: Option<Display>,
    pub flex_direction: Option<FlexDirection>,
    pub width: Option<Dimension>,
    pub height: Option<Dimension>,
    pub padding: Option<LengthPercentage>,
    pub margin: Option<LengthPercentageAuto>,
    pub paint: PaintStyle,
}

impl SpecifiedStyle {
    /// Folds the specified layout declarations into a taffy style.
    pub(crate) fn fold_into(&self, node_style: &mut Style) {
        if let Some(d) = self.display { node_style.display = d; }
        if let Some(fd) = self.flex_direction { node_style.flex_direction = fd; }
        if let Some(w) = self.width { node_style.size.width = w; }
        if let Some(h) = self.height {
            node_style.size.height = h;
            node_style.min_size.height = h;
        }
        if let Some(p) = self.padding {
            node_style.padding = Rect { left: p, right: p, top: p, bottom: p };
        }
        if let Some(m) = self.margin {
            node_style.margin = Rect { left: m, right: m, top: m, bottom: m };
        }
    }
}

/// One declaration, shared by the stylesheet cascade and inline styles.
/// Values arrive as raw strings so function values (rgb()...) parse uniformly.
pub(crate) fn apply_declaration(prop: &str, value: &str, style: &mut SpecifiedStyle) {
    let value = value.trim();
    match prop {
        "display" => match value {
            "none" => style.display = Some(Display::None),
            "flex" | "block" | "inline-block" | "list-item" => style.display = Some(Display::Flex),
            other => crate::ledger::record_css(&format!("display:{}", other)),
        },
        "flex-direction" => match value {
            "row" => style.flex_direction = Some(FlexDirection::Row),
            "column" => style.flex_direction = Some(FlexDirection::Column),
            _ => {}
        },
        "width" => style.width = parse_dimension_str(value),
        "height" => style.height = parse_dimension_str(value),
        "padding" => style.padding = parse_length_percentage_str(value),
        "margin" => style.margin = parse_length_percentage_auto_str(value),
        "background-color" | "background" => {
            if !is_neutral_keyword(value) {
                match parse_color_str(value) {
                    Some(c) => style.paint.background = Some(c),
                    None => crate::ledger::record_css(&format!("background-value:{}", clip(value))),
                }
            }
        }
        "color" => {
            if !is_neutral_keyword(value) {
                match parse_color_str(value) {
                    Some(c) => style.paint.color = Some(c),
                    None => crate::ledger::record_css(&format!("color-value:{}", clip(value))),
                }
            }
        }
        "font-size" => match parse_font_size(value) {
            Some(px) => style.paint.font_size = Some(px),
            None => crate::ledger::record_css(&format!("font-size-value:{}", clip(value))),
        },
        "font-weight" => match parse_font_weight(value) {
            Some(b) => style.paint.bold = Some(b),
            None => crate::ledger::record_css(&format!("font-weight-value:{}", clip(value))),
        },
        other => crate::ledger::record_css(&format!("property:{}", other)),
    }
}

/// Truncates a value for a stable, bounded ledger key.
fn clip(v: &str) -> &str {
    &v[..v.len().min(24)]
}

pub fn apply_css(layout_tree: &mut LayoutTree, css: &str) {
    apply_stylesheet(layout_tree, css, 0);

    // set_style only marks nodes dirty; re-lay out with text measurement.
    crate::layout::remeasure(layout_tree);
}

fn apply_stylesheet(layout_tree: &mut LayoutTree, css: &str, depth: u8) {
    if depth > 4 {
        return; // pathological nesting guard
    }
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);

    loop {
        // Slice the raw prelude up to the next `{`.
        let start = parser.position();
        let mut saw_block = false;
        let mut at_rule: Option<String> = None;

        loop {
            let Ok(token) = parser.next() else { break };
            match token {
                Token::CurlyBracketBlock => {
                    saw_block = true;
                    break;
                }
                Token::AtKeyword(kw) => at_rule = Some(kw.as_ref().to_string()),
                _ => {}
            }
        }
        if !saw_block {
            break; // end of stylesheet
        }

        let prelude = parser.slice_from(start);
        let prelude = prelude.trim_end().trim_end_matches('{').trim().to_string();

        // Capture the raw block body so it can be recursed or skipped.
        let body = parser
            .parse_nested_block(|p| {
                let s = p.position();
                while p.next().is_ok() {}
                Ok::<String, cssparser::ParseError<'_, ()>>(p.slice_from(s).to_string())
            })
            .unwrap_or_default();

        if let Some(kw) = at_rule {
            match kw.as_str() {
                "media" => {
                    let condition = prelude.trim_start_matches("@media").trim();
                    if media_matches(condition) {
                        apply_stylesheet(layout_tree, &body, depth + 1);
                    }
                }
                other => crate::ledger::record_css(&format!("at-rule:@{}", other)),
            }
            continue;
        }

        // Compile with kuchiki's real selector engine (servo selectors).
        let selectors = match kuchiki::Selectors::compile(&prelude) {
            Ok(s) => s,
            Err(_) => {
                crate::ledger::record_css(&format!("selector-compile-failed:{}", clip(&prelude)));
                continue;
            }
        };

        let style = parse_declaration_block(&body);

        // Apply only the specified declarations to matching nodes.
        for (node_id, dom_node) in &layout_tree.node_map {
            let Some(el_ref) = dom_node.clone().into_element_ref() else { continue };
            if !selectors.matches(&el_ref) {
                continue;
            }
            if let Ok(node_style_ref) = layout_tree.taffy.style(*node_id) {
                let mut node_style = node_style_ref.clone();
                style.fold_into(&mut node_style);
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

/// Parses a `prop: value; prop: value` declaration block (no braces).
pub(crate) fn parse_declaration_block(body: &str) -> SpecifiedStyle {
    let mut style = SpecifiedStyle::default();
    for decl in body.split(';') {
        let Some((prop, value)) = decl.split_once(':') else { continue };
        let prop = prop.trim().to_ascii_lowercase();
        if prop.is_empty() || prop.starts_with("--") {
            continue; // custom properties: no cascade var() support yet
        }
        // Strip !important — priority handling is source-order for now.
        let value = value.trim().trim_end_matches("!important").trim();
        apply_declaration(&prop, value, &mut style);
    }
    style
}

/// Evaluates an @media condition against the fixed viewport. Comma = OR,
/// "and" = AND. Unknown features evaluate false (and are ledgered), so a
/// query is never wrongly applied.
fn media_matches(condition: &str) -> bool {
    if condition.is_empty() {
        return true;
    }
    condition.split(',').any(|clause| {
        clause.split(" and ").all(|part| {
            let p = part.trim().trim_start_matches('(').trim_end_matches(')').trim();
            match p {
                "screen" | "all" => true,
                "print" => false,
                _ => {
                    if let Some((feature, value)) = p.split_once(':') {
                        let value = value.trim();
                        match feature.trim() {
                            "min-width" => parse_px(value).map(|v| VIEWPORT_W >= v).unwrap_or(false),
                            "max-width" => parse_px(value).map(|v| VIEWPORT_W <= v).unwrap_or(false),
                            f => {
                                crate::ledger::record_css(&format!("media-feature:{}", f));
                                false
                            }
                        }
                    } else {
                        crate::ledger::record_css(&format!("media-condition:{}", clip(p)));
                        false
                    }
                }
            }
        })
    })
}

/// Parses a CSS color from a string: named, #rgb/#rrggbb, rgb()/rgba().
/// rgba() alpha is ignored (no compositing yet) unless fully transparent.
pub fn parse_color_str(value: &str) -> Option<(u8, u8, u8)> {
    let v = value.trim();
    if let Some(hex) = v.strip_prefix('#') {
        return hex_color(hex);
    }
    let lower = v.to_ascii_lowercase();
    if let Some(inner) = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split([',', ' ', '/']).filter(|s| !s.trim().is_empty()).collect();
        if parts.len() >= 3 {
            let ch = |s: &str| -> Option<u8> {
                let s = s.trim();
                if let Some(p) = s.strip_suffix('%') {
                    p.trim().parse::<f32>().ok().map(|f| (f / 100.0 * 255.0) as u8)
                } else {
                    s.parse::<f32>().ok().map(|f| f.clamp(0.0, 255.0) as u8)
                }
            };
            if let (Some(r), Some(g), Some(b)) = (ch(parts[0]), ch(parts[1]), ch(parts[2])) {
                // Fully transparent = paint nothing.
                if let Some(a) = parts.get(3).and_then(|s| s.trim().parse::<f32>().ok()) {
                    if a == 0.0 {
                        return None;
                    }
                }
                return Some((r, g, b));
            }
        }
        return None;
    }
    named_color(&lower)
}

/// Keywords that specify "no concrete value here" — not coverage gaps.
pub fn is_neutral_keyword(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "transparent" | "inherit" | "initial" | "unset" | "currentcolor" | "none"
    )
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

/// Parses a font-weight into "bold or not".
pub fn parse_font_weight(value: &str) -> Option<bool> {
    let v = value.trim().to_ascii_lowercase();
    match v.as_str() {
        "bold" | "bolder" => Some(true),
        "normal" | "lighter" => Some(false),
        _ => v.parse::<f32>().ok().map(|n| n >= 600.0),
    }
}

/// Parses "NNpx" (or a bare number) into pixels.
pub fn parse_px(value: &str) -> Option<f32> {
    let v = value.trim();
    v.strip_suffix("px").unwrap_or(v).trim().parse::<f32>().ok()
}

fn parse_dimension_str(value: &str) -> Option<Dimension> {
    let v = value.trim();
    if v == "auto" {
        return Some(Dimension::auto());
    }
    if let Some(pct) = v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
        return Some(Dimension::percent(pct / 100.0));
    }
    parse_px(v).map(Dimension::length)
}

fn parse_length_percentage_str(value: &str) -> Option<LengthPercentage> {
    let v = value.trim();
    if let Some(pct) = v.strip_suffix('%').and_then(|n| n.trim().parse::<f32>().ok()) {
        return Some(LengthPercentage::percent(pct / 100.0));
    }
    parse_px(v).map(LengthPercentage::length)
}

fn parse_length_percentage_auto_str(value: &str) -> Option<LengthPercentageAuto> {
    let v = value.trim();
    if v == "auto" {
        return Some(LengthPercentageAuto::auto());
    }
    parse_length_percentage_str(v).map(Into::into)
}

fn named_color(name: &str) -> Option<(u8, u8, u8)> {
    match name {
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
            crate::ledger::record_css(&format!("named-color:{}", clip(other)));
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
        6 | 8 => {
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

    #[test]
    fn test_color_functions() {
        assert_eq!(parse_color_str("rgb(10, 20, 30)"), Some((10, 20, 30)));
        assert_eq!(parse_color_str("rgba(10, 20, 30, 0.5)"), Some((10, 20, 30)));
        assert_eq!(parse_color_str("rgba(10, 20, 30, 0)"), None);
        assert_eq!(parse_color_str("#abc"), Some((170, 187, 204)));
        assert_eq!(parse_color_str("#336699"), Some((51, 102, 153)));
    }

    #[test]
    fn test_media_matches() {
        assert!(media_matches("screen"));
        assert!(!media_matches("print"));
        assert!(media_matches("screen and (min-width: 600px)"));
        assert!(!media_matches("screen and (min-width: 1200px)"));
        assert!(media_matches("(max-width: 900px)"));
        assert!(media_matches("print, screen"));
        assert!(!media_matches("(prefers-reduced-motion: reduce)"));
    }
}
