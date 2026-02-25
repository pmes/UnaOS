use std::{cell::RefCell, rc::Rc};
use glib::clone;

// 1. Restore on boot
if let Ok(draft) = std::fs::read_to_string("/tmp/unaos_lumen_draft.txt") {
    text_buffer.set_text(&draft);
}

// 2. Debounced Auto-save
let pending_save = Rc::new(RefCell::new(None));
text_buffer.connect_changed(clone!(@weak text_buffer, @strong pending_save => move |_| {
    if let Some(source) = pending_save.borrow_mut().take() {
        source.remove(); // Cancel previous pending save
    }

    let text = text_buffer.text(&text_buffer.start_iter(), &text_buffer.end_iter(), false).to_string();

    *pending_save.borrow_mut() = Some(glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        // Fire and forget. If it fails, we don't care, we try again next keystroke.
        std::fs::write("/tmp/unaos_lumen_draft.txt", &text).ok();
        glib::ControlFlow::Break
    }));
}));
