use gtk4::prelude::*;
use gtk4::{ScrolledWindow, TextView, TextBuffer, Widget};

pub fn create_view() -> (Widget, TextBuffer) {
    let scroll = ScrolledWindow::new();
    scroll.set_vexpand(true);
    let view = TextView::builder()
        .monospace(true)
        .editable(false)
        .build();
    view.add_css_class("console");

    let buffer = view.buffer();
    scroll.set_child(Some(&view));

    (scroll.upcast::<Widget>(), buffer)
}
