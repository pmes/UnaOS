use gtk4::prelude::*;
use gtk4::{
    Box, Orientation, Label, Button, Stack, ScrolledWindow,
    PolicyType, Align, ListBox, Separator, StackTransitionType, TextView,
    TextBuffer, HeaderBar, StackSwitcher, ToggleButton, Image,
    Paned, ApplicationWindow, Widget, Window
};
use sourceview5::View as SourceView;

// Import Adwaita if feature is enabled
#[cfg(feature = "gnome")]
use libadwaita::prelude::*;
#[cfg(feature = "gnome")]
use libadwaita as adw;

use gneiss_pal::types::*;

pub struct CommsSpline {}

impl CommsSpline {
    pub fn new() -> Self {
        Self {}
    }

    pub fn bootstrap<W: IsA<Window> + IsA<Widget> + Cast>(&self, _window: &W, tx_event: async_channel::Sender<Event>) -> Widget {
        // --- HEADER BAR ---
        let header_bar = HeaderBar::new();
        let sidebar_toggle = ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .active(true)
            .tooltip_text("Toggle Sidebar")
            .build();
        header_bar.pack_start(&sidebar_toggle);

        // --- BODY CONTAINER ---
        let body_box = Box::new(Orientation::Horizontal, 0);

        // --- SIDEBAR ---
        let sidebar_box = Box::new(Orientation::Vertical, 0);
        sidebar_box.set_width_request(200);
        sidebar_box.add_css_class("sidebar");

        let sidebar_stack = Stack::new();
        sidebar_stack.set_vexpand(true);
        sidebar_stack.set_transition_type(StackTransitionType::SlideLeftRight);

        // Page 1: Rooms
        let rooms_list = ListBox::new();
        for (idx, item) in ["General", "Encrypted", "Jules (Private)"].iter().enumerate() {
            let row = Box::new(Orientation::Horizontal, 10);
            row.set_margin_start(10); row.set_margin_end(10);
            row.append(&Label::new(Some(item)));
            rooms_list.append(&row);
        }

        let tx_clone_nav = tx_event.clone();
        rooms_list.connect_row_activated(move |_list_box, row| {
            let idx = row.index() as usize;
            let _ = tx_clone_nav.send_blocking(Event::NavSelect(idx));
        });
        sidebar_stack.add_titled(&rooms_list, Some("rooms"), "Rooms");

        // Page 2: Status
        let status_box = Box::new(Orientation::Vertical, 10);
        status_box.set_margin_top(10);
        let shard_list = ListBox::new();
        shard_list.add_css_class("shard-list");
        let row_box = Box::new(Orientation::Horizontal, 10);
        let icon = Image::from_icon_name("computer-symbolic");
        icon.set_widget_name("una-prime");
        row_box.append(&icon);
        row_box.append(&Label::new(Some("Una-Prime")));
        shard_list.append(&row_box);
        status_box.append(&shard_list);

        sidebar_stack.add_titled(&status_box, Some("status"), "Status");

        sidebar_box.append(&sidebar_stack);

        let stack_switcher = StackSwitcher::builder().stack(&sidebar_stack).build();
        let tab_box = Box::new(Orientation::Horizontal, 0);
        tab_box.set_halign(Align::Center);
        tab_box.append(&stack_switcher);
        sidebar_box.append(&tab_box);

        body_box.append(&sidebar_box);
        body_box.append(&Separator::new(Orientation::Vertical));

        // --- CONTENT (Paned) ---
        let paned = Paned::new(Orientation::Vertical);
        paned.set_vexpand(true);
        paned.set_hexpand(true);
        paned.set_position(550);

        // Console (Top Pane)
        let scrolled_window = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .build();

        let text_buffer = TextBuffer::new(None);
        let console_text_view = TextView::builder()
            .wrap_mode(gtk4::WrapMode::WordChar)
            .editable(false)
            .monospace(true)
            .buffer(&text_buffer)
            .margin_start(12).margin_end(12).margin_top(12).margin_bottom(12)
            .build();

        let text_buffer_clone = text_buffer.clone();
        let scrolled_window_adj_clone = scrolled_window.vadjustment();
        let _ = tx_event.send_blocking(Event::TextBufferUpdate(text_buffer_clone, scrolled_window_adj_clone));

        scrolled_window.set_child(Some(&console_text_view));
        paned.set_start_child(Some(&scrolled_window));

        // Input Area (Bottom Pane)
        let input_container = Box::new(Orientation::Horizontal, 8);
        input_container.set_valign(Align::Fill);

        // Input Field
        let input_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .propagate_natural_height(true)
            .max_content_height(500)
            .vexpand(true)
            .valign(Align::Fill)
            .margin_top(10)
            .margin_bottom(10)
            .has_frame(false)
            .build();
        input_scroll.set_hexpand(true);
        input_scroll.add_css_class("chat-input-area");

        let text_view = SourceView::builder()
            .wrap_mode(gtk4::WrapMode::WordChar)
            .show_line_numbers(false)
            .auto_indent(true)
            .accepts_tab(false)
            .top_margin(2)
            .bottom_margin(2)
            .left_margin(8)
            .right_margin(8)
            .build();

        text_view.add_css_class("transparent-text");
        input_scroll.set_child(Some(&text_view));

        // Send Button
        let send_btn = Button::builder()
            .icon_name("paper-plane-symbolic")
            .valign(Align::End)
            .margin_bottom(10)
            .margin_end(10)
            .css_classes(vec!["suggested-action"])
            .build();

        let tx_clone_send = tx_event.clone();
        let buffer = text_view.buffer();
        send_btn.connect_clicked(move |_| {
            let (start, end) = buffer.bounds();
            let text = buffer.text(&start, &end, false).to_string();
            if !text.trim().is_empty() {
                let _ = tx_clone_send.send_blocking(Event::Input(text));
                buffer.set_text("");
            }
        });

        input_container.append(&input_scroll);
        input_container.append(&send_btn);

        paned.set_end_child(Some(&input_container));
        body_box.append(&paned);

        // Toggle Sidebar
        let sidebar_box_clone = sidebar_box.clone();
        sidebar_toggle.connect_toggled(move |btn| {
            sidebar_box_clone.set_visible(btn.is_active());
        });

        // --- POLYMORPHIC RETURN ---
        #[cfg(feature = "gnome")]
        {
            let view = adw::ToolbarView::new();
            view.add_top_bar(&header_bar);
            view.set_content(Some(&body_box));
            view.upcast::<Widget>()
        }

        #[cfg(not(feature = "gnome"))]
        {
            // GTK Mode: Set titlebar on window
            if let Some(app_win) = _window.dynamic_cast_ref::<gtk4::ApplicationWindow>() {
                app_win.set_titlebar(Some(&header_bar));
            }
            body_box.into()
        }
    }
}
