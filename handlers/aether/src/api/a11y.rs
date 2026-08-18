use kuchiki::{NodeRef, NodeData};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct A11yNode {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub state: HashMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub children: Vec<A11yNode>,
}

pub fn build_a11y_tree(node: &NodeRef) -> Option<A11yNode> {
    match node.data() {
        NodeData::Element(element_data) => {
            let attrs = element_data.attributes.borrow();
            
            // Explicit ARIA role
            let mut role = attrs.get("role").unwrap_or("").to_string();
            
            let tag_name = element_data.name.local.to_string();
            
            if role.is_empty() {
                role = match tag_name.as_str() {
                    "a" => "link".to_string(),
                    "button" => "button".to_string(),
                    "nav" => "navigation".to_string(),
                    "header" => "banner".to_string(),
                    "footer" => "contentinfo".to_string(),
                    "main" => "main".to_string(),
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "heading".to_string(),
                    "ul" | "ol" => "list".to_string(),
                    "li" => "listitem".to_string(),
                    "img" => "img".to_string(),
                    "input" => {
                        let ty = attrs.get("type").unwrap_or("text");
                        match ty {
                            "checkbox" => "checkbox".to_string(),
                            "radio" => "radio".to_string(),
                            "submit" | "button" | "reset" => "button".to_string(),
                            _ => "textbox".to_string(),
                        }
                    },
                    "textarea" => "textbox".to_string(),
                    "form" => "form".to_string(),
                    "dialog" => "dialog".to_string(),
                    "article" => "article".to_string(),
                    "section" => "region".to_string(),
                    "aside" => "complementary".to_string(),
                    "figure" => "figure".to_string(),
                    "select" => "combobox".to_string(),
                    "option" => "option".to_string(),
                    "table" => "table".to_string(),
                    "tr" => "row".to_string(),
                    "td" => "cell".to_string(),
                    "th" => "columnheader".to_string(),
                    "html" | "body" => "document".to_string(),
                    "iframe" => "iframe".to_string(),
                    "hr" => "separator".to_string(),
                    "progress" => "progressbar".to_string(),
                    "math" => "math".to_string(),
                    _ => "generic".to_string(),
                };
            }

            let mut name = attrs.get("aria-label").map(|s| s.to_string());
            if name.is_none() {
                if let Some(alt) = attrs.get("alt") {
                    name = Some(alt.to_string());
                } else if let Some(title) = attrs.get("title") {
                    name = Some(title.to_string());
                }
            }
            
            let mut state = HashMap::new();
            for (qual_name, attr) in attrs.map.iter() {
                let k = qual_name.local.to_string();
                if k.starts_with("aria-") {
                    state.insert(k.clone(), attr.value.clone());
                }
            }

            let value = attrs.get("value").map(|s| s.to_string());

            let mut children = Vec::new();
            for child in node.children() {
                if let Some(child_node) = build_a11y_tree(&child) {
                    children.push(child_node);
                }
            }

            Some(A11yNode {
                role,
                name,
                value,
                state,
                children,
            })
        },
        NodeData::Text(text_ref) => {
            let text = text_ref.borrow().trim().to_string();
            if text.is_empty() {
                None
            } else {
                Some(A11yNode {
                    role: "text".to_string(),
                    name: Some(text),
                    value: None,
                    state: HashMap::new(),
                    children: Vec::new(),
                })
            }
        },
        NodeData::Document(_) | NodeData::DocumentFragment => {
            let mut children = Vec::new();
            for child in node.children() {
                if let Some(child_node) = build_a11y_tree(&child) {
                    children.push(child_node);
                }
            }
            Some(A11yNode {
                role: "root".to_string(),
                name: None,
                value: None,
                state: HashMap::new(),
                children,
            })
        },
        _ => None,
    }
}
