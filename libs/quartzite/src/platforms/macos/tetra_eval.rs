use objc2::rc::{Allocated, Retained};
use objc2::{msg_send, ClassType};
use objc2_app_kit::{NSStackView, NSView};
use objc2_foundation::{MainThreadMarker, NSRect, NSPoint, NSSize};

use crate::tetra::TetraNode;
use bandy::Synapse;

pub fn eval_tetra(node: TetraNode, synapse: Synapse) -> Retained<NSView> {
    let _mtm = MainThreadMarker::new().expect("eval_tetra must run on main thread");

    match node {
        TetraNode::VStack(children) => {
            unsafe {
                let stack: Allocated<NSStackView> = msg_send![NSStackView::class(), alloc];
                let stack: Retained<NSStackView> = msg_send![stack, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))];
                let _: () = msg_send![&stack, setOrientation: 1isize]; // Vertical
                
                for child in children {
                    let child_view = eval_tetra(child, synapse.clone());
                    // Top gravity
                    let _: () = msg_send![&stack, addView: &*child_view, inGravity: 1isize];
                }
                Retained::cast_unchecked::<NSView>(stack)
            }
        }
        TetraNode::HStack(children) => {
            unsafe {
                let stack: Allocated<NSStackView> = msg_send![NSStackView::class(), alloc];
                let stack: Retained<NSStackView> = msg_send![stack, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))];
                let _: () = msg_send![&stack, setOrientation: 0isize]; // Horizontal
                
                for child in children {
                    let child_view = eval_tetra(child, synapse.clone());
                    // Leading gravity
                    let _: () = msg_send![&stack, addView: &*child_view, inGravity: 1isize];
                }
                Retained::cast_unchecked::<NSView>(stack)
            }
        }
        TetraNode::Button { id: _, label, action } => {
            super::button::bootstrap_button(&label, action, synapse)
        }
        TetraNode::TextField { id: _, placeholder, action } => {
            super::text_field::bootstrap_text_field(&placeholder, action, synapse)
        }
        TetraNode::Surface { id } => {
            // Note: SurfaceBlit must target a Surface by id once >1 surface exists
            super::image_view::bootstrap_image_surface(&id, synapse)
        }
        _ => {
            // Placeholder for unsupported nodes
            unsafe {
                let view: Allocated<NSView> = msg_send![NSView::class(), alloc];
                let view: Retained<NSView> = msg_send![view, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(10.0, 10.0))];
                view
            }
        }
    }
}
