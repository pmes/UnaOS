use gtk4::prelude::*;
use sourceview5::prelude::*;
use sourceview5::View as SourceView;
use gtk4::{ScrolledWindow, TextBuffer, Widget};

pub fn create_view() -> (Widget, TextBuffer) {
    let scroll = ScrolledWindow::new();
    scroll.set_vexpand(true);
    let view = SourceView::builder()
        .monospace(true)
        .show_line_numbers(true)
        .auto_indent(true)
        .build();

    let buffer = view.buffer().upcast::<TextBuffer>();
    scroll.set_child(Some(&view));

    (scroll.upcast::<Widget>(), buffer)
}
