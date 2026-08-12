use gtk4::prelude::*;
use gtk4::{Box, Orientation};
use crate::tetra::TetraNode;
use crate::NativeView;
use bandy::Synapse;

pub fn eval_tetra(node: TetraNode, synapse: Synapse) -> NativeView {
    match node {
        TetraNode::VStack(children) => {
            let container = Box::new(Orientation::Vertical, 0);
            for child in children {
                container.append(&eval_tetra(child, synapse.clone()));
            }
            container.into()
        }
        TetraNode::HStack(children) => {
            let container = Box::new(Orientation::Horizontal, 0);
            for child in children {
                container.append(&eval_tetra(child, synapse.clone()));
            }
            container.into()
        }
        TetraNode::Button { id: _, label, action } => {
            super::button::bootstrap_button(&label, action, synapse)
        }
        TetraNode::TextField { id: _, placeholder, action } => {
            super::text_field::bootstrap_text_field(&placeholder, action, synapse)
        }
        TetraNode::Surface { id } => {
            super::image_view::bootstrap_image_surface(&id, synapse)
        }
        TetraNode::Console(console) => {
            super::console_view::bootstrap_console(&console, synapse)
        }
        _ => {
            let container = Box::new(Orientation::Horizontal, 0);
            container.into()
        }
    }
}
