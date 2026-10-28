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
        TetraNode::TextField { id: _, placeholder, action, secret } => {
            stretch(super::text_field::bootstrap_text_field(&placeholder, secret, action, synapse), 0)
        }
        TetraNode::Surface { id } => {
            // Note: SurfaceBlit must target a Surface by id once >1 surface exists
            stretch(super::image_view::bootstrap_image_surface(&id, synapse), 1)
        }
        TetraNode::Console(console) => {
            // FACELIVE (SR435): the GTK twin's Console arm (the installer's status panes were blank placeholders here)
            super::console_view::bootstrap_console(&console, synapse)
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

// FACELIVE (LEDGER SR435): the face-push door's AppKit side. `new_tetra_vessel` evaluates through `eval_live`; a vessel's
// `tetra::push_face` wakes `apply_face` on the main queue, which writes `tetra::take_face`'s patches into the views in
// place (a Console's text, a field's placeholder with its typed text and focus kept, a stack's children, a leaf).

thread_local! {
    /// The live tree's root view and the bus its new nodes speak on (main thread).
    static FACE_ROOT: std::cell::RefCell<Option<(Retained<NSView>, Synapse)>> = const { std::cell::RefCell::new(None) };
}

/// Evaluate a tetra vessel's tree through the door (a push that beat the window wins) and keep its root for patches.
pub fn eval_live(node: TetraNode, synapse: Synapse) -> Retained<NSView> {
    let node = crate::tetra::open_face(node, std::sync::Arc::new(|| dispatch2::DispatchQueue::main().exec_async(apply_face)));
    let view = crate::tetra::SurfaceMap::of(&node).scope(|| eval_tetra(node, synapse.clone()));
    FACE_ROOT.with(|r| *r.borrow_mut() = Some((view.clone(), synapse)));
    view
}

/// Whether `view` is a stack (it answers `arrangedSubviews`).
fn face_is_stack(view: &NSView) -> bool {
    let r: objc2::runtime::Bool = unsafe { msg_send![view, respondsToSelector: objc2::sel!(arrangedSubviews)] };
    r.as_bool()
}

/// The `i`-th arranged subview of a stack view (None when `view` is no stack or holds fewer).
fn face_child(view: &NSView, i: usize) -> Option<Retained<NSView>> {
    if !face_is_stack(view) {
        return None;
    }
    unsafe {
        let subs: Retained<objc2_foundation::NSArray<NSView>> = msg_send![view, arrangedSubviews];
        if i >= subs.count() {
            return None;
        }
        Some(subs.objectAtIndex(i))
    }
}

/// The view at `path` under `root`.
fn face_view(root: &Retained<NSView>, path: &[usize]) -> Option<Retained<NSView>> {
    let mut cur = root.clone();
    for &i in path {
        cur = face_child(&cur, i)?;
    }
    Some(cur)
}

/// Remove one arranged subview from its stack and from the view tree.
unsafe fn face_unarrange(stack: &NSView, view: &NSView) {
    unsafe {
        let _: () = msg_send![stack, removeArrangedSubview: view];
        let _: () = msg_send![view, removeFromSuperview];
    }
}

/// Apply the door's pending tree on the main thread.
fn apply_face() {
    let Some((full, patches)) = crate::tetra::take_face() else { return };
    let Some((root, synapse)) = FACE_ROOT.with(|r| r.borrow().clone()) else { return };
    crate::tetra::SurfaceMap::of(&full).scope(|| {
        for patch in patches {
            match patch {
                crate::tetra::FacePatch::Lines { path, console } => {
                    // The console view is a stack of [header label, level filters, scroll view over the text view].
                    let Some(v) = face_view(&root, &path) else { continue };
                    let (Some(header), Some(scroll)) = (face_child(&v, 0), face_child(&v, 2)) else { continue };
                    unsafe {
                        let s = objc2_foundation::NSString::from_str(&crate::widgets::console_rows::render_header(console.dropped, console.paused));
                        let _: () = msg_send![&header, setStringValue: &*s];
                        let doc: Option<Retained<NSView>> = msg_send![&scroll, documentView];
                        if let Some(doc) = doc {
                            let text = Retained::cast_unchecked::<objc2_app_kit::NSTextView>(doc);
                            super::console_view::set_text(&text, &crate::widgets::console_rows::render_body(&console.lines), true);
                        }
                    }
                }
                crate::tetra::FacePatch::Field { path, node } => {
                    let TetraNode::TextField { placeholder, .. } = node else { continue };
                    let Some(field) = face_view(&root, &path) else { continue };
                    let s = objc2_foundation::NSString::from_str(&placeholder);
                    let _: () = unsafe { msg_send![&field, setPlaceholderString: &*s] };
                }
                crate::tetra::FacePatch::Restack { path, node } => {
                    let Some(stack) = face_view(&root, &path) else { continue };
                    let (vertical, children) = match node {
                        TetraNode::VStack(c) => (true, c),
                        TetraNode::HStack(c) => (false, c),
                        _ => continue,
                    };
                    if !face_is_stack(&stack) {
                        continue;
                    }
                    while let Some(c) = face_child(&stack, 0) {
                        unsafe { face_unarrange(&stack, &c) };
                    }
                    unsafe {
                        let s = Retained::cast_unchecked::<NSStackView>(stack.clone());
                        let _: () = msg_send![&s, setOrientation: if vertical { 1isize } else { 0isize }];
                        fill_stack(&s, if vertical { 7 } else { 0 });
                        for child in children {
                            let v = eval_tetra(child, synapse.clone());
                            let _: () = msg_send![&s, addArrangedSubview: &*v];
                        }
                    }
                }
                crate::tetra::FacePatch::Replace { path, node } => {
                    let Some((&last, parent_path)) = path.split_last() else {
                        log::warn!("[quartzite] face root-leaf unpatched");
                        continue;
                    };
                    let (Some(parent), Some(old)) = (face_view(&root, parent_path), face_view(&root, &path)) else { continue };
                    let new = eval_tetra(node, synapse.clone());
                    unsafe {
                        let _: () = msg_send![&parent, insertArrangedSubview: &*new, atIndex: last as isize];
                        face_unarrange(&parent, &old);
                    }
                }
            }
        }
    });
}
