pub mod layout {
    use std::collections::HashMap;
    use taffy::prelude::*;
    use kuchiki::NodeRef;
    pub struct LayoutTree {
        pub taffy: Taffy,
        pub root_node: Node,
        pub node_map: HashMap<Node, NodeRef>,
    }
}
pub mod css {
    use crate::css_test_mod::layout::LayoutTree;
    use taffy::prelude::*;
    use taffy::style::{Display, FlexDirection};
    use cssparser::{Parser, ParserInput, DeclarationListParser, RuleListParser, QualifiedRuleParser, AtRuleParser, ToCss};

    pub struct Rule {
        pub selector: String,
        pub declarations: Vec<Declaration>,
    }

    pub struct Declaration {
        pub name: String,
        pub value: String,
    }

    pub struct MyRuleParser;

    impl<'i> QualifiedRuleParser<'i> for MyRuleParser {
        type Prelude = String;
        type QualifiedRule = Rule;
        type Error = ();

        fn parse_prelude<'t>(
            &mut self,
            input: &mut Parser<'i, 't>,
        ) -> Result<Self::Prelude, cssparser::ParseError<'i, Self::Error>> {
            let mut selector = String::new();
            while let Ok(token) = input.next() {
                selector.push_str(&token.to_css_string());
            }
            Ok(selector)
        }

        fn parse_block<'t>(
            &mut self,
            prelude: Self::Prelude,
            _start: &cssparser::ParserState,
            input: &mut Parser<'i, 't>,
        ) -> Result<Self::QualifiedRule, cssparser::ParseError<'i, Self::Error>> {
            let mut decl_parser = MyDeclParser;
            let iter = DeclarationListParser::new(input, &mut decl_parser);
            let mut declarations = Vec::new();
            for decl in iter {
                if let Ok(d) = decl {
                    declarations.push(d);
                }
            }
            Ok(Rule { selector: prelude, declarations })
        }
    }

    pub struct MyDeclParser;

    impl<'i> cssparser::DeclarationParser<'i> for MyDeclParser {
        type Declaration = Declaration;
        type Error = ();

        fn parse_value<'t>(
            &mut self,
            name: cssparser::CowRcStr<'i>,
            input: &mut Parser<'i, 't>,
        ) -> Result<Self::Declaration, cssparser::ParseError<'i, Self::Error>> {
            let mut value = String::new();
            while let Ok(token) = input.next() {
                value.push_str(&token.to_css_string());
            }
            Ok(Declaration {
                name: name.to_string(),
                value,
            })
        }
    }

    impl<'i> AtRuleParser<'i> for MyRuleParser {
        type Prelude = ();
        type AtRule = Rule;
        type Error = ();
    }

    pub fn parse_stylesheet(css: &str) -> Vec<Rule> {
        let mut input = ParserInput::new(css);
        let mut parser = Parser::new(&mut input);
        let mut rule_parser = MyRuleParser;
        let iter = RuleListParser::new_for_stylesheet(&mut parser, &mut rule_parser);
        let mut rules = Vec::new();
        for rule in iter {
            if let Ok(r) = rule {
                rules.push(r);
            }
        }
        rules
    }

    pub fn apply_css(css: &str, layout_tree: &mut LayoutTree) {
        let rules = parse_stylesheet(css);
        
        let mut reverse_map = std::collections::HashMap::new();
        for (taffy_node, dom_node) in &layout_tree.node_map {
            reverse_map.insert(dom_node.clone(), *taffy_node);
        }
        
        if let Some(root_dom_node) = layout_tree.node_map.get(&layout_tree.root_node) {
            for rule in rules {
                if let Ok(matched_elements) = root_dom_node.select(&rule.selector) {
                    for matched in matched_elements {
                        let dom_node = matched.as_node();
                        if let Some(taffy_node) = reverse_map.get(dom_node) {
                            if let Ok(mut style) = layout_tree.taffy.style(*taffy_node).cloned() {
                                for decl in &rule.declarations {
                                    apply_declaration(&mut style, &decl.name, &decl.value);
                                }
                                let _ = layout_tree.taffy.set_style(*taffy_node, style);
                            }
                        }
                    }
                }
            }
        }
    }

    fn apply_declaration(style: &mut Style, name: &str, value: &str) {
        let value = value.trim();
        match name {
            "display" => {
                if value == "flex" {
                    style.display = Display::Flex;
                } else if value == "none" {
                    style.display = Display::None;
                }
            }
            "flex-direction" => {
                if value == "row" {
                    style.flex_direction = FlexDirection::Row;
                } else if value == "column" {
                    style.flex_direction = FlexDirection::Column;
                }
            }
            "width" => {
                if value.ends_with("px") {
                    if let Ok(v) = value[..value.len()-2].parse::<f32>() {
                        style.size.width = Dimension::Length(v);
                    }
                } else if value.ends_with("%") {
                    if let Ok(v) = value[..value.len()-1].parse::<f32>() {
                        style.size.width = Dimension::Percent(v / 100.0);
                    }
                }
            }
            "height" => {
                if value.ends_with("px") {
                    if let Ok(v) = value[..value.len()-2].parse::<f32>() {
                        style.size.height = Dimension::Length(v);
                    }
                } else if value.ends_with("%") {
                    if let Ok(v) = value[..value.len()-1].parse::<f32>() {
                        style.size.height = Dimension::Percent(v / 100.0);
                    }
                }
            }
            _ => {}
        }
    }
}
