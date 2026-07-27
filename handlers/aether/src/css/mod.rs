
use crate::layout::LayoutTree;
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
                                        other => {
                                            crate::ledger::record_css(&format!("property:{}", other));
                                        }
                                    }
                                }
                            }
                        }
                        
                        Ok::<Style, cssparser::ParseError<'_, ()>>(style)
                    });
                    
                    if let Ok(style) = block_parser {
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
                                }
                            }
                        }
                    }
                }
            }
        }
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
