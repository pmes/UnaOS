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
                let _: () = msg_send![&stack, setOrientation: 1isize]; fill_stack(&stack, 7); // Vertical
                
                for child in children {
                    let child_view = eval_tetra(child, synapse.clone());
                    // Arranged: Fill, the lowest hugging child takes the height
                    let _: () = msg_send![&stack, addArrangedSubview: &*child_view];
                }
                Retained::cast_unchecked::<NSView>(stack)
            }
        }
        TetraNode::HStack(children) => {
            unsafe {
                let stack: Allocated<NSStackView> = msg_send![NSStackView::class(), alloc];
                let stack: Retained<NSStackView> = msg_send![stack, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))];
                let _: () = msg_send![&stack, setOrientation: 0isize]; fill_stack(&stack, 0); // Horizontal
                
                for child in children {
                    let child_view = eval_tetra(child, synapse.clone());
                    // Arranged: Fill, the lowest hugging child takes the width
                    let _: () = msg_send![&stack, addArrangedSubview: &*child_view];
                }
                Retained::cast_unchecked::<NSView>(stack)
            }
        }
        TetraNode::Button { id: _, label, action } => {
            super::button::bootstrap_button(&label, action, synapse)
        }
        TetraNode::TextField { id: _, placeholder, action } => {
            stretch(super::text_field::bootstrap_text_field(&placeholder, action, synapse), 0)
        }
        TetraNode::Surface { id } => {
            // Note: SurfaceBlit must target a Surface by id once >1 surface exists
            stretch(super::image_view::bootstrap_image_surface(&id, synapse), 1)
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

/// AETHERMAC (SR196): a tetra stack lays its children out with `Fill` (NSStackViewDistributionFill = 0: the child
/// with the lowest hugging priority takes the slack along the axis) instead of gravity areas; `cross` != 0 sets the
/// cross-axis alignment (NSLayoutAttributeWidth = 7: a vertical stack's children fill its width).
unsafe fn fill_stack(stack: &NSStackView, cross: isize) {
    unsafe {
        let _: () = msg_send![stack, setDistribution: 0isize];
        if cross != 0 {
            let _: () = msg_send![stack, setAlignment: cross];
        }
    }
}

/// AETHERMAC (SR196): `view` yields along `axis` (0 horizontal, 1 vertical): hugging and compression resistance
/// 1.0, so the address field takes the toolbar's slack and the page surface takes the window's height (and lets
/// the window shrink below the surface's 800x600 frame).
fn stretch(view: Retained<NSView>, axis: isize) -> Retained<NSView> {
    unsafe {
        let _: () = msg_send![&view, setContentHuggingPriority: 1.0f32, forOrientation: axis];
        let _: () = msg_send![&view, setContentCompressionResistancePriority: 1.0f32, forOrientation: axis];
    }
    view
}
