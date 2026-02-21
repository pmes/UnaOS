use gtk4::prelude::*;
use gtk4::{ScrolledWindow, TextBuffer, Widget};
use sourceview5::View as SourceView;
use sourceview5::prelude::*;
use libspelling;

// Wrapper to allow storing !Send GObjects in set_data (Safe on main thread)
struct SendWrapper<T>(pub T);
unsafe impl<T> Send for SendWrapper<T> {}
unsafe impl<T> Sync for SendWrapper<T> {}

#[derive(Debug, Clone)]
pub enum EditorMode {
    Code(String), // Language ID (e.g., "rust", "python")
    Prose,
    Log,
}

pub fn create_view(mode: EditorMode) -> (Widget, TextBuffer) {
    let scroll = ScrolledWindow::new();
    scroll.set_vexpand(true);

    // Base configuration
    let view = SourceView::builder()
        .auto_indent(true)
        .build();

    // Mode-specific configuration
    match &mode {
        EditorMode::Code(lang_id) => {
            view.set_monospace(true);
            view.set_show_line_numbers(true);
            view.set_wrap_mode(gtk4::WrapMode::None);

            // Syntax Highlighting
            let lm = sourceview5::LanguageManager::default();
            if let Some(lang) = lm.language(lang_id) {
                if let Some(buffer) = view.buffer().downcast::<sourceview5::Buffer>().ok() {
                    buffer.set_language(Some(&lang));
                }
            }
        },
        EditorMode::Prose => {
            view.set_monospace(false); // Sans-serif for prose
            view.set_show_line_numbers(false);
            view.set_wrap_mode(gtk4::WrapMode::WordChar);
            view.set_left_margin(12);
            view.set_right_margin(12);

            // Spellcheck (The Boz Protocol)
            if let Some(buffer) = view.buffer().downcast::<sourceview5::Buffer>().ok() {
                let provider = libspelling::Provider::default();
                let adapter = libspelling::TextBufferAdapter::new(&buffer, &provider);
                adapter.set_enabled(true);
                buffer.set_data("spell-adapter", SendWrapper(adapter));
            }
        },
        EditorMode::Log => {
            view.set_monospace(true);
            view.set_show_line_numbers(false);
            view.set_editable(false);
            view.set_wrap_mode(gtk4::WrapMode::WordChar);
        }
    }

    let buffer = view.buffer().upcast::<TextBuffer>();
    scroll.set_child(Some(&view));

    (scroll.upcast::<Widget>(), buffer)
}
