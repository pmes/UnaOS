
use crate::layout::{LayoutTree, PaintStyle};
use cssparser::{Parser, ParserInput, Token};
use taffy::prelude::*;
use taffy::style::{Dimension, Display, FlexDirection, LengthPercentage, LengthPercentageAuto};
use taffy::geometry::Rect;

pub fn apply_css(layout_tree: &mut LayoutTree, css: &str) {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);

    while !parser.is_exhausted() {
        if let Ok(token) = parser.next() {
            if let Token::Ident(name_rc) = token {
                let tag_name = name_rc.as_ref().to_string();
                // Expect a curly block next
                if let Ok(Token::CurlyBracketBlock) = parser.next() {
                    let mut block_parser = parser.parse_nested_block(|p| {
                        let mut style = Style::default();
                        let mut paint = PaintStyle::default();

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
                                                    "flex" => style.display = Display::Flex,
                                                    "none" => style.display = Display::None,
                                                    other => {
                                                        crate::ledger::record_css(&format!("display:{}", other));
                                                    }
                                                }
                                            }
                                        }
                                        "flex-direction" => {
                                            if let Some(Token::Ident(ident)) = first_val.as_ref() {
                                                match ident.as_ref() {
                                                    "row" => style.flex_direction = FlexDirection::Row,
                                                    "column" => style.flex_direction = FlexDirection::Column,
                                                    _ => {}
                                                }
                                            }
                                        }
                                        "width" => {
                                            if let Some(d) = parse_dimension(first_val.as_ref()) {
                                                style.size.width = d;
                                            }
                                        }
                                        "height" => {
                                            if let Some(d) = parse_dimension(first_val.as_ref()) {
                                                style.size.height = d;
                                            }
                                        }
                                        "padding" => {
                                            if let Some(lp) = parse_length_percentage(first_val.as_ref()) {
                                                style.padding = Rect { left: lp.clone(), right: lp.clone(), top: lp.clone(), bottom: lp };
                                            }
                                        }
                                        "margin" => {
                                            if let Some(lp) = parse_length_percentage_auto(first_val.as_ref()) {
                                                style.margin = Rect { left: lp.clone(), right: lp.clone(), top: lp.clone(), bottom: lp };
                                            }
                                        }
                                        "background-color" | "background" => {
                                            match color_from_token(first_val.as_ref()) {
                                                Some(c) => paint.background = Some(c),
                                                None => crate::ledger::record_css("background-value:unsupported"),
                                            }
                                        }
                                        "color" => match color_from_token(first_val.as_ref()) {
                                            Some(c) => paint.color = Some(c),
                                            None => crate::ledger::record_css("color-value:unsupported"),
                                        },
                                        "font-size" => match first_val.as_ref() {
                                            Some(Token::Dimension { value, unit, .. }) if unit.as_ref() == "px" => {
                                                paint.font_size = Some(*value);
                                            }
                                            _ => crate::ledger::record_css("font-size-value:unsupported"),
                                        },
                                        other => {
                                            crate::ledger::record_css(&format!("property:{}", other));
                                        }
                                    }
                                }
                            }
                        }
                        
                        Ok::<(Style, PaintStyle), cssparser::ParseError<'_, ()>>((style, paint))
                    });

                    if let Ok((style, paint)) = block_parser {
                        // Apply to matching nodes
                        for (node_id, dom_node) in &layout_tree.node_map {
                            if let Some(el) = dom_node.as_element() {
                                if el.name.local.as_ref() == tag_name {
                                    if let Ok(node_style_ref) = layout_tree.taffy.style(*node_id) {
                                        let mut node_style = node_style_ref.clone();
                                        // Update properties that are not default
                                        if style.display != Display::Flex { node_style.display = style.display; }
                                        node_style.flex_direction = style.flex_direction;
                                        if style.size.width != Dimension::auto() { node_style.size.width = style.size.width; }
                                        if style.size.height != Dimension::auto() { node_style.size.height = style.size.height; }
                                        node_style.padding = style.padding;
                                        node_style.margin = style.margin;
                                        
                                        let _ = layout_tree.taffy.set_style(*node_id, node_style);
                                    }
                                    let entry = layout_tree.paint_map.entry(*node_id).or_default();
                                    if paint.background.is_some() { entry.background = paint.background; }
                                    if paint.color.is_some() { entry.color = paint.color; }
                                    if paint.font_size.is_some() { entry.font_size = paint.font_size; }
                                }
                            }
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
