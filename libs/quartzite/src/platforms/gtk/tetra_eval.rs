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
        TetraNode::TextField { id: _, placeholder, action, secret } => {
            super::text_field::bootstrap_text_field(&placeholder, secret, action, synapse)
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

// FACELIVE (LEDGER SR435): the face-push door's GTK side. `new_tetra_vessel` evaluates through `eval_live`; a vessel's
// `tetra::push_face` wakes `apply_face` on the main context, which writes `tetra::take_face`'s patches into the widgets
// in place (a Console's text, a field's placeholder with its typed text and focus kept, a stack's children, a leaf).

thread_local! {
    /// The live tree's root widget and the bus its new nodes speak on (main thread).
    static FACE_ROOT: std::cell::RefCell<Option<(NativeView, Synapse)>> = const { std::cell::RefCell::new(None) };
}

/// Evaluate a tetra vessel's tree through the door (a push that beat the window wins) and keep its root for patches.
pub fn eval_live(node: TetraNode, synapse: Synapse) -> NativeView {
    let node = crate::tetra::open_face(node, std::sync::Arc::new(|| gtk4::glib::MainContext::default().invoke(apply_face)));
    let view = crate::tetra::SurfaceMap::of(&node).scope(|| eval_tetra(node, synapse.clone()));
    FACE_ROOT.with(|r| *r.borrow_mut() = Some((view.clone(), synapse)));
    view
}

/// The widget at `path` under `root` (each step the n-th child of a stack's Box).
fn face_widget(root: &gtk4::Widget, path: &[usize]) -> Option<gtk4::Widget> {
    let mut cur = root.clone();
    for &i in path {
        let mut child = cur.first_child();
        for _ in 0..i {
            child = child.and_then(|c| c.next_sibling());
        }
        cur = child?;
    }
    Some(cur)
}

/// Put `new` in `old`'s place in its parent Box.
fn face_swap(old: &gtk4::Widget, new: &gtk4::Widget) {
    let Some(parent) = old.parent().and_then(|p| p.downcast::<Box>().ok()) else { return };
    parent.insert_child_after(new, old.prev_sibling().as_ref());
    parent.remove(old);
}

/// Apply the door's pending tree on the main thread.
fn apply_face() {
    let Some((full, patches)) = crate::tetra::take_face() else { return };
    let Some((root, synapse)) = FACE_ROOT.with(|r| r.borrow().clone()) else { return };
    crate::tetra::SurfaceMap::of(&full).scope(|| {
        for patch in patches {
            match patch {
                crate::tetra::FacePatch::Lines { path, console } => {
                    let Some(w) = face_widget(&root, &path) else { continue };
                    if let Some(header) = w.first_child().and_then(|c| c.downcast::<gtk4::Label>().ok()) {
                        header.set_label(&crate::widgets::console_rows::render_header(console.dropped, console.paused));
                    }
                    let text = w.last_child().and_then(|c| c.downcast::<gtk4::ScrolledWindow>().ok()).and_then(|s| s.child()).and_then(|c| c.downcast::<gtk4::TextView>().ok());
                    if let Some(text) = text {
                        text.buffer().set_text(&crate::widgets::console_rows::render_body(&console.lines));
                    }
                }
                crate::tetra::FacePatch::Field { path, node } => {
                    let Some(old) = face_widget(&root, &path) else { continue };
                    let new = eval_tetra(node, synapse.clone());
                    let focus = old.has_focus();
                    if let (Some(o), Some(n)) = (old.downcast_ref::<super::glyph::GlyphEntry>(), new.downcast_ref::<super::glyph::GlyphEntry>()) {
                        n.set_text(&o.text());
                    }
                    face_swap(&old, &new);
                    if focus {
                        new.grab_focus();
                    }
                }
                crate::tetra::FacePatch::Restack { path, node } => {
                    let Some(b) = face_widget(&root, &path).and_then(|w| w.downcast::<Box>().ok()) else { continue };
                    let (orientation, children) = match node {
                        TetraNode::VStack(c) => (Orientation::Vertical, c),
                        TetraNode::HStack(c) => (Orientation::Horizontal, c),
                        _ => continue,
                    };
                    while let Some(c) = b.first_child() {
                        b.remove(&c);
                    }
                    b.set_orientation(orientation);
                    for child in children {
                        b.append(&eval_tetra(child, synapse.clone()));
                    }
                }
                crate::tetra::FacePatch::Replace { path, node } => {
                    if path.is_empty() {
                        log::warn!("[quartzite] face root-leaf unpatched");
                        continue;
                    }
                    let Some(old) = face_widget(&root, &path) else { continue };
                    face_swap(&old, &eval_tetra(node, synapse.clone()));
                }
            }
        }
    });
}
