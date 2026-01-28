#[cfg(target_os = "linux")]
use gtk4::prelude::*;
#[cfg(target_os = "linux")]
use gtk4::{TextView, TextBuffer, ScrolledWindow, PolicyType, WrapMode};

// Platform-agnostic struct wrapper (inner fields depend on OS)
pub struct ScrollableText {
    #[cfg(target_os = "linux")]
    pub container: ScrolledWindow,
    #[cfg(target_os = "linux")]
    view: TextView,
    #[cfg(target_os = "linux")]
    buffer: TextBuffer,
}

impl ScrollableText {
    #[cfg(target_os = "linux")]
    pub fn new() -> Self {
        let buffer = TextBuffer::new(None);

        let view = TextView::builder()
            .buffer(&buffer)
            .editable(false)
            .monospace(true)
            .wrap_mode(WrapMode::WordChar) // Default to word wrap as per Vein requirements
            .left_margin(10)
            .right_margin(10)
            .top_margin(10)
            .bottom_margin(10)
            .build();

        let container = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .child(&view)
            .vexpand(true)
            .hexpand(true)
            .build();

        Self {
            container,
            view,
            buffer,
        }
    }

    #[cfg(target_os = "linux")]
    pub fn set_content(&self, text: &str) {
        self.buffer.set_text(text);
    }

    #[cfg(target_os = "linux")]
    pub fn append_content(&self, text: &str) {
        let mut end = self.buffer.end_iter();
        self.buffer.insert(&mut end, text);
        self.scroll_to_bottom();
    }

    #[cfg(target_os = "linux")]
    pub fn scroll_to_bottom(&self) {
        // Create a mark at the end and scroll to it
        let mark = self.buffer.create_mark(None, &self.buffer.end_iter(), false);
        self.view.scroll_to_mark(&mark, 0.0, false, 0.0, 1.0);
    }
}
