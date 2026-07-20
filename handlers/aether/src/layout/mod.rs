use kuchiki::NodeRef;
use taffy::prelude::*;
use std::collections::HashMap;

pub struct LayoutTree {
    pub taffy: taffy::TaffyTree,
    pub root_node: taffy::NodeId,
    pub node_map: HashMap<taffy::NodeId, NodeRef>, // Maps layout boxes back to DOM nodes
}

pub fn compute_layout(dom: &NodeRef) -> LayoutTree {
    let mut taffy = taffy::TaffyTree::new();
    let mut node_map = HashMap::new();

    // Very basic recursive layout generation for M1
    fn build_taffy_tree(
        dom_node: &NodeRef,
        taffy: &mut taffy::TaffyTree,
        node_map: &mut HashMap<taffy::NodeId, NodeRef>
    ) -> Option<taffy::NodeId> {
        let mut child_ids = Vec::new();
        
        for child in dom_node.children() {
            if let Some(id) = build_taffy_tree(&child, taffy, node_map) {
                child_ids.push(id);
            }
        }
        
        let style = Style {
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
        
        if let Ok(node_id) = taffy.new_with_children(style, &child_ids) {
             node_map.insert(node_id, dom_node.clone());
             Some(node_id)
        } else {
             None
        }
    }

    let root_node = build_taffy_tree(dom, &mut taffy, &mut node_map).unwrap_or_else(|| {
        taffy.new_leaf(Style::default()).unwrap()
    });

    let size = Size {
        width: AvailableSpace::Definite(800.0),
        height: AvailableSpace::Definite(600.0),
    };
    
    taffy.compute_layout(root_node, size).unwrap();

    LayoutTree {
        taffy,
        root_node,
        node_map,
    }
}
