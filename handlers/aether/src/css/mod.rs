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
    pub padding: Option<Rect<LengthPercentage>>,
    pub margin: Option<Rect<LengthPercentageAuto>>,
    pub position: Option<taffy::style::Position>,
    pub inset_top: Option<LengthPercentageAuto>,
    pub inset_left: Option<LengthPercentageAuto>,
    pub inset_right: Option<LengthPercentageAuto>,
    pub inset_bottom: Option<LengthPercentageAuto>,
    pub justify: Option<taffy::style::JustifyContent>,
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
            node_style.padding = p;
        }
        if let Some(m) = self.margin {
            node_style.margin = m;
        }
        if let Some(pos) = self.position {
            node_style.position = pos;
        }
        if let Some(v) = self.inset_top { node_style.inset.top = v; }
        if let Some(v) = self.inset_left { node_style.inset.left = v; }
        if let Some(v) = self.inset_right { node_style.inset.right = v; }
        if let Some(v) = self.inset_bottom { node_style.inset.bottom = v; }
        if let Some(j) = self.justify {
            node_style.justify_content = Some(j);
        }
        if let Some((w, _)) = self.paint.border {
            let b = LengthPercentage::length(w);
            node_style.border = Rect { left: b, right: b, top: b, bottom: b };
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
            // Column-flex approximations of block-ish display types.
            "flex" | "block" | "inline-block" | "inline-flex" | "inline" | "list-item"
            | "flow-root" | "table" | "table-cell" | "table-caption" | "table-row-group"
            | "table-header-group" | "table-footer-group" => {
                style.display = Some(Display::Flex)
            }
            "table-row" => {
                style.display = Some(Display::Flex);
                style.flex_direction = Some(FlexDirection::Row);
            }
            "inherit" | "initial" | "unset" | "revert" => {}
            other => crate::ledger::record_css(&format!("display:{}", other)),
        },
        "flex-direction" => match value {
            "row" => style.flex_direction = Some(FlexDirection::Row),
            "column" => style.flex_direction = Some(FlexDirection::Column),
            _ => {}
        },
        "width" => style.width = parse_dimension_str(value),
        "height" => style.height = parse_dimension_str(value),
        "padding" => style.padding = parse_sides(value, |v| parse_length_percentage_str(v)),
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            if let Some(v) = parse_length_percentage_str(value) {
                let mut p = style.padding.unwrap_or(Rect {
                    left: LengthPercentage::length(0.0),
                    right: LengthPercentage::length(0.0),
                    top: LengthPercentage::length(0.0),
                    bottom: LengthPercentage::length(0.0),
                });
                match prop {
                    "padding-top" => p.top = v,
                    "padding-right" => p.right = v,
                    "padding-bottom" => p.bottom = v,
                    _ => p.left = v,
                }
                style.padding = Some(p);
            }
        }
        "margin" => style.margin = parse_sides(value, |v| parse_length_percentage_auto_str(v)),
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
            if let Some(v) = parse_length_percentage_auto_str(value) {
                let mut m = style.margin.unwrap_or(Rect {
                    left: LengthPercentageAuto::length(0.0),
                    right: LengthPercentageAuto::length(0.0),
                    top: LengthPercentageAuto::length(0.0),
                    bottom: LengthPercentageAuto::length(0.0),
                });
                match prop {
                    "margin-top" => m.top = v,
                    "margin-right" => m.right = v,
                    "margin-bottom" => m.bottom = v,
                    _ => m.left = v,
                }
                style.margin = Some(m);
            }
        }
        "position" => match value {
            "absolute" | "fixed" => style.position = Some(taffy::style::Position::Absolute),
            "static" | "relative" | "sticky" => style.position = Some(taffy::style::Position::Relative),
            other => crate::ledger::record_css(&format!("position:{}", other)),
        },
        "top" => style.inset_top = parse_length_percentage_auto_str(value),
        "left" => style.inset_left = parse_length_percentage_auto_str(value),
        "right" => style.inset_right = parse_length_percentage_auto_str(value),
        "bottom" => style.inset_bottom = parse_length_percentage_auto_str(value),
        "text-align" => match value {
            "center" => style.justify = Some(taffy::style::JustifyContent::CENTER),
            "right" | "end" => style.justify = Some(taffy::style::JustifyContent::END),
            "left" | "start" | "justify" => style.justify = Some(taffy::style::JustifyContent::START),
            other => crate::ledger::record_css(&format!("text-align:{}", other)),
        },
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
        "line-height" => {
            let v = value.trim();
            let factor = if let Some(px) = v.strip_suffix("px").and_then(|n| n.trim().parse::<f32>().ok()) {
                Some(px / 16.0)
            } else if v == "normal" {
                Some(1.2)
            } else {
                v.parse::<f32>().ok()
            };
            match factor {
                Some(f) => style.paint.line_height = Some(f),
                None => crate::ledger::record_css(&format!("line-height-value:{}", clip(value))),
            }
        }
        "border" | "outline" => {
            let v = value.trim();
            if v == "none" || v == "0" {
                style.paint.border = None;
                return;
            }
            let mut width = 1.0f32;
            let mut color = (128, 128, 128);
            let mut got_any = false;
            for part in v.split_whitespace() {
                if let Some(px) = part.strip_suffix("px").and_then(|n| n.parse::<f32>().ok()) {
                    width = px;
                    got_any = true;
                } else if matches!(part, "solid" | "dotted" | "dashed" | "double" | "groove" | "ridge" | "inset" | "outset") {
                    got_any = true;
                } else if let Some(c) = parse_color_str(part) {
                    color = c;
                    got_any = true;
                }
            }
            if got_any {
                style.paint.border = Some((width, color));
            } else {
                crate::ledger::record_css(&format!("border-value:{}", clip(value)));
            }
        }
        "border-color" => {
            if let Some(c) = parse_color_str(value) {
                let w = style.paint.border.map(|(w, _)| w).unwrap_or(1.0);
                style.paint.border = Some((w, c));
            }
        }
        "border-width" => {
            if let Some(w) = parse_px(value) {
                let c = style.paint.border.map(|(_, c)| c).unwrap_or((128, 128, 128));
                style.paint.border = Some((w, c));
            }
        }
        "border-style" => {} // stroke style is uniform; nothing to record
        other => crate::ledger::record_css(&format!("property:{}", other)),
    }
}

/// Truncates a value for a stable, bounded ledger key.
fn clip(v: &str) -> &str {
    &v[..v.len().min(24)]
}

/// One applicable rule: a single compiled selector with its declarations.
struct Rule {
    selector: kuchiki::Selector,
    style: std::rc::Rc<SpecifiedStyle>,
}

pub fn apply_css(layout_tree: &mut LayoutTree, css: &str) {
    apply_stylesheets(layout_tree, std::slice::from_ref(&css.to_string()));
}

/// Applies a set of stylesheets as ONE cascade — rules from every sheet
/// sort together by (specificity, source order), so a later sheet's less
/// specific rule no longer beats an earlier sheet's more specific one.
pub fn apply_stylesheets(layout_tree: &mut LayoutTree, sheets: &[String]) {
    let mut rules = Vec::new();
    for css in sheets {
        collect_rules(css, 0, &mut rules);
    }
    rules.sort_by(|a, b| a.selector.specificity().cmp(&b.selector.specificity()));

    for rule in &rules {
        let style = &rule.style;
        for (node_id, dom_node) in &layout_tree.node_map {
            let Some(el_ref) = dom_node.clone().into_element_ref() else { continue };
            if !rule.selector.matches(&el_ref) {
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
            if style.paint.border.is_some() { entry.border = style.paint.border; }
            if style.paint.line_height.is_some() { entry.line_height = style.paint.line_height; }
        }
    }

    // set_style only marks nodes dirty; re-lay out with text measurement.
    crate::layout::remeasure(layout_tree);
}

fn collect_rules(css: &str, depth: u8, rules: &mut Vec<Rule>) {
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
                        collect_rules(&body, depth + 1, rules);
                    }
                }
                "supports" => {
                    let condition = prelude.trim_start_matches("@supports").trim();
                    if supports_matches(condition) {
                        collect_rules(&body, depth + 1, rules);
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

        let style = std::rc::Rc::new(parse_declaration_block(&body));
        for selector in selectors.0 {
            rules.push(Rule { selector, style: style.clone() });
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

/// Properties this engine genuinely implements (the honest support set
/// for @supports; extend when apply_declaration grows an arm).
fn property_supported(prop: &str) -> bool {
    matches!(
        prop,
        "display" | "flex-direction" | "width" | "height" | "padding" | "margin"
            | "padding-top" | "padding-right" | "padding-bottom" | "padding-left"
            | "margin-top" | "margin-right" | "margin-bottom" | "margin-left"
            | "position" | "top" | "left" | "right" | "bottom" | "text-align"
            | "background-color" | "background" | "color" | "font-size" | "font-weight"
    )
}

/// Evaluates an @supports condition against our real capability set.
/// `not` inverts; and/or compose; unknown syntax evaluates false.
fn supports_matches(condition: &str) -> bool {
    let c = condition.trim();
    if let Some(rest) = c.strip_prefix("not ") {
        return !supports_matches(rest);
    }
    if c.contains(" or ") {
        return c.split(" or ").any(supports_matches);
    }
    if c.contains(" and ") {
        return c.split(" and ").all(supports_matches);
    }
    let inner = c.trim().trim_start_matches('(').trim_end_matches(')').trim();
    match inner.split_once(':') {
        Some((prop, _value)) => property_supported(prop.trim()),
        None => {
            crate::ledger::record_css(&format!("supports-condition:{}", clip(inner)));
            false
        }
    }
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

/// Parses a 1-4 value box shorthand ("10px", "0 auto", "1px 2px 3px 4px")
/// into a sides rect using CSS's top/right/bottom/left expansion.
fn parse_sides<T: Copy>(value: &str, parse_one: impl Fn(&str) -> Option<T>) -> Option<Rect<T>> {
    let parts: Vec<T> = value.split_whitespace().map(|p| parse_one(p)).collect::<Option<_>>()?;
    let (t, r, b, l) = match parts.as_slice() {
        [a] => (*a, *a, *a, *a),
        [v, h] => (*v, *h, *v, *h),
        [t, h, b] => (*t, *h, *b, *h),
        [t, r, b, l] => (*t, *r, *b, *l),
        _ => return None,
    };
    Some(Rect { top: t, right: r, bottom: b, left: l })
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
