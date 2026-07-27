use kuchiki::NodeRef;
use taffy::prelude::*;
use std::collections::HashMap;

/// Specified paint properties for one box. `None` = not specified here;
/// color and font-size inherit down the tree at render time.
#[derive(Debug, Clone, Copy, Default)]
pub struct PaintStyle {
    pub background: Option<(u8, u8, u8)>,
    pub color: Option<(u8, u8, u8)>,
    pub font_size: Option<f32>,
}

pub struct LayoutTree {
    pub taffy: taffy::TaffyTree,
    pub root_node: taffy::NodeId,
    pub node_map: HashMap<taffy::NodeId, NodeRef>, // Maps layout boxes back to DOM nodes
    pub paint_map: HashMap<taffy::NodeId, PaintStyle>,
    pub dirty: bool,
}

impl LayoutTree {
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn recompute(&mut self, dom: &NodeRef) {
        if !self.dirty {
            return;
        }
        let new_tree = compute_layout(dom);
        self.taffy = new_tree.taffy;
        self.root_node = new_tree.root_node;
        self.node_map = new_tree.node_map;
        self.paint_map = new_tree.paint_map;
        self.dirty = false;
    }
}

/// Elements whose subtrees produce no boxes.
fn is_non_rendered(name: &str) -> bool {
    matches!(name, "head" | "script" | "style" | "title" | "meta" | "link" | "template")
}

pub fn compute_layout(dom: &NodeRef) -> LayoutTree {
    let mut taffy = taffy::TaffyTree::new();
    let mut node_map = HashMap::new();
    let mut paint_map = HashMap::new();

    fn build_taffy_tree(
        dom_node: &NodeRef,
        taffy: &mut taffy::TaffyTree,
        node_map: &mut HashMap<taffy::NodeId, NodeRef>,
        paint_map: &mut HashMap<taffy::NodeId, PaintStyle>,
    ) -> Option<taffy::NodeId> {
        if let Some(el) = dom_node.as_element() {
            if is_non_rendered(el.name.local.as_ref()) {
                return None;
            }
        } else if dom_node.as_text().is_some() {
            // Whitespace-only text produces no box.
            if dom_node.text_contents().trim().is_empty() {
                return None;
            }
        }

        let mut child_ids = Vec::new();
        for child in dom_node.children() {
            if let Some(id) = build_taffy_tree(&child, taffy, node_map, paint_map) {
                child_ids.push(id);
            }
        }

        let mut style = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            size: Size {
                width: Dimension::percent(1.0),
                height: Dimension::auto(),
            },
            min_size: Size {
                width: Dimension::auto(),
                height: Dimension::length(20.0),
            },
            margin: Rect {
                left: LengthPercentage::length(2.0).into(),
                right: LengthPercentage::length(2.0).into(),
                top: LengthPercentage::length(2.0).into(),
                bottom: LengthPercentage::length(2.0).into(),
            },
            ..Default::default()
        };

        // Inline style="..." — paint properties plus width/height.
        let mut paint = PaintStyle::default();
        if let Some(el) = dom_node.as_element() {
            if let Some(inline) = el.attributes.borrow().get("style") {
                apply_inline_style(inline, &mut style, &mut paint);
            }
        }

        if let Ok(node_id) = taffy.new_with_children(style, &child_ids) {
            node_map.insert(node_id, dom_node.clone());
            paint_map.insert(node_id, paint);
            Some(node_id)
        } else {
            None
        }
    }

    let root_node = build_taffy_tree(dom, &mut taffy, &mut node_map, &mut paint_map)
        .unwrap_or_else(|| taffy.new_leaf(Style::default()).unwrap());

    let size = Size {
        width: AvailableSpace::Definite(800.0),
        height: AvailableSpace::Definite(600.0),
    };

    taffy.compute_layout(root_node, size).unwrap();

    LayoutTree {
        taffy,
        root_node,
        node_map,
        paint_map,
        dirty: false,
    }
}

/// Applies a `style="..."` attribute: paint properties into `paint`,
/// px width/height into the taffy style. Unknown declarations go to the ledger.
fn apply_inline_style(inline: &str, style: &mut Style, paint: &mut PaintStyle) {
    for decl in inline.split(';') {
        let Some((prop, value)) = decl.split_once(':') else { continue };
        let prop = prop.trim().to_ascii_lowercase();
        let value = value.trim();
        match prop.as_str() {
            "background-color" | "background" => {
                match crate::css::parse_color_str(value) {
                    Some(c) => paint.background = Some(c),
                    None => crate::ledger::record_css(&format!("background-value:{}", value)),
                }
            }
            "color" => match crate::css::parse_color_str(value) {
                Some(c) => paint.color = Some(c),
                None => crate::ledger::record_css(&format!("color-value:{}", value)),
            },
            "font-size" => match crate::css::parse_px(value) {
                Some(px) => paint.font_size = Some(px),
                None => crate::ledger::record_css(&format!("font-size-value:{}", value)),
            },
            "width" => {
                if let Some(px) = crate::css::parse_px(value) {
                    style.size.width = Dimension::length(px);
                }
            }
            "height" => {
                if let Some(px) = crate::css::parse_px(value) {
                    style.size.height = Dimension::length(px);
                    style.min_size.height = Dimension::length(px);
                }
            }
            other => crate::ledger::record_css(&format!("inline-property:{}", other)),
        }
    }
}
