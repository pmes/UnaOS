use gtk4::prelude::*;
use gtk4::{
    Box, Orientation, Label, Button, Stack, ScrolledWindow,
    PolicyType, Align, ListBox, Separator, StackTransitionType, TextView,
    TextBuffer, HeaderBar, StackSwitcher, ToggleButton, Image,
    Paned, ApplicationWindow, Widget, Window, CssProvider, StyleContext,
    EventControllerKey, Spinner, MenuButton, Popover, FileDialog,
    gdk::{Key, ModifierType}, PropagationPhase
};
use async_channel::Receiver;
use sourceview5::View as SourceView;

// Import Adwaita if feature is enabled
#[cfg(feature = "gnome")]
use libadwaita::prelude::*;
#[cfg(feature = "gnome")]
use libadwaita as adw;

use gneiss_pal::types::*;
use gneiss_pal::shard::ShardStatus;

pub struct CommsSpline {}

impl CommsSpline {
    pub fn new() -> Self {
        Self {}
    }

    pub fn bootstrap<W: IsA<Window> + IsA<Widget> + Cast>(&self, window: &W, tx_event: async_channel::Sender<Event>, rx: Receiver<GuiUpdate>) -> Widget {
        // --- WINDOW TITLE ---
        window.set_title(Some("Vein"));

        // --- STYLE PROVIDER ---
        let provider = CssProvider::new();
        // Updated CSS for Visibility and "Breathing" Look
        provider.load_from_string("
            .sidebar { background-color: #1e1e1e; color: #ffffff; }
            .console { background-color: #101010; color: #dddddd; font-family: 'Monospace'; caret-color: #dddddd; padding: 12px; }

            /* Input Area Container (The Pill) */
            .chat-input-area {
                background-color: #2d2d2d;
                border-radius: 12px;
                padding: 2px;
            }

            /* The Text View inside */
            textview.transparent-text {
                background-color: transparent;
                color: #ffffff;
                caret-color: #ffffff;
                font-family: 'Sans';
                font-size: 15px;
                padding: 6px; /* Added internal padding */
            }

            textview.transparent-text text {
                background-color: transparent;
                color: #ffffff;
            }

            /* Send and Attach Buttons */
            .suggested-action {
                background-color: #0078d4;
                color: #ffffff;
                border-radius: 4px;
                padding: 0px;
                min-width: 34px;
                min-height: 34px;
                margin-left: 8px;
            }

            .suggested-action image {
                -gtk-icon-style: symbolic;
                color: #ffffff;
            }

            .attach-action {
                background-color: #333333;
                color: #cccccc;
                border-radius: 4px;
                padding: 0px;
                min-width: 42px;
                min-height: 42px;
                margin-right: 8px;
            }

            .attach-action image {
                -gtk-icon-style: symbolic;
                color: inherit; /* Inherit from button text color */
            }

            .attach-action:hover {
                color: #ffffff;
                background-color: #444444;
            }
            .attach-action:active {
                background-color: #222222;
            }

            .shard-list { background-color: transparent; }

            window { background-color: #1e1e1e; }

            /* Sidebar Stack Switcher */
            stackswitcher button {
                background: transparent;
                color: #888888;
                border: none;
                box-shadow: none;
                padding: 8px 16px;
                font-weight: bold;
            }
            stackswitcher button:checked {
                color: #ffffff;
                border-bottom: 2px solid #0078d4;
                background: rgba(255, 255, 255, 0.05);
            }
            stackswitcher button:hover {
                background: rgba(255, 255, 255, 0.1);
            }
        ");

        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("No display"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // --- ACTIONS MENU ---
        let menu_box = Box::new(Orientation::Vertical, 5);
        menu_box.set_margin_start(10);
        menu_box.set_margin_end(10);
        menu_box.set_margin_top(10);
        menu_box.set_margin_bottom(10);

        let btn_clear = Button::with_label("Clear Console");
        let tx_clear = tx_event.clone();
        btn_clear.connect_clicked(move |_| {
            let _ = tx_clear.send_blocking(Event::Input("/clear".into()));
        });

        let btn_wolf = Button::with_label("Wolfpack Mode");
        let tx_wolf = tx_event.clone();
        btn_wolf.connect_clicked(move |_| {
            let _ = tx_wolf.send_blocking(Event::Input("/wolf".into()));
        });

        let btn_comms = Button::with_label("Comms Mode");
        let tx_comms = tx_event.clone();
        btn_comms.connect_clicked(move |_| {
            let _ = tx_comms.send_blocking(Event::Input("/comms".into()));
        });

        menu_box.append(&btn_clear);
        menu_box.append(&btn_wolf);
        menu_box.append(&btn_comms);

        let popover = Popover::builder().child(&menu_box).build();
        let menu_button = MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .popover(&popover)
            .build();

        // --- HEADER BAR (Polymorphic) ---
        let sidebar_toggle = ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .active(true)
            .tooltip_text("Toggle Sidebar")
            .build();

        #[cfg(feature = "gnome")]
        let header_bar = {
            let hb = adw::HeaderBar::new();
            hb.pack_start(&sidebar_toggle);
            hb.pack_end(&menu_button);
            hb
        };

        #[cfg(not(feature = "gnome"))]
        let header_bar = {
            let hb = HeaderBar::new();
            hb.pack_start(&sidebar_toggle);
            hb.pack_end(&menu_button);
            hb.set_show_title_buttons(true);
            hb
        };

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

        // Page 2: Status (With Dynamic Updates)
        let status_box = Box::new(Orientation::Vertical, 10);
        status_box.set_margin_top(10);
        let shard_list = ListBox::new();
        shard_list.add_css_class("shard-list");

        // Helper to create row (Manual for now to keep refs)
        // 1. Una-Prime
        let row_una = Box::new(Orientation::Horizontal, 10);
        row_una.set_margin_start(10);
        let icon_una = Image::from_icon_name("computer-symbolic");
        icon_una.set_widget_name("una-prime");
        let label_una = Label::new(Some("Una-Prime"));
        let spinner_una = Spinner::new();
        row_una.append(&icon_una);
        row_una.append(&label_una);
        row_una.append(&spinner_una);
        shard_list.append(&row_una);

        // 2. S9-Mule
        let row_s9 = Box::new(Orientation::Horizontal, 10);
        row_s9.set_margin_start(10);
        let icon_s9 = Image::from_icon_name("network-server-symbolic");
        icon_s9.set_widget_name("s9-mule");
        let label_s9 = Label::new(Some("S9-Mule"));
        let spinner_s9 = Spinner::new();
        row_s9.append(&icon_s9);
        row_s9.append(&label_s9);
        row_s9.append(&spinner_s9);
        shard_list.append(&row_s9);

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
        console_text_view.add_css_class("console");

        let text_buffer_clone = text_buffer.clone();
        let scrolled_window_adj_clone = scrolled_window.vadjustment();
        let _ = tx_event.send_blocking(Event::TextBufferUpdate(text_buffer_clone, scrolled_window_adj_clone));

        scrolled_window.set_child(Some(&console_text_view));
        paned.set_start_child(Some(&scrolled_window));

        // Input Area (Bottom Pane)
        let input_container = Box::new(Orientation::Horizontal, 8);
        input_container.set_valign(Align::Fill);
        input_container.set_margin_start(16);
        input_container.set_margin_end(16);
        input_container.set_margin_bottom(16);
        input_container.set_margin_top(16);

        // Attach Button (Left)
        let attach_icon = Image::from_icon_name("share-symbolic");
        attach_icon.set_pixel_size(24);

        let attach_btn = Button::builder()
            .valign(Align::End)
            .css_classes(vec!["attach-action"])
            .child(&attach_icon)
            .build();

        // Implement Attach logic (Using FileDialog)
        let tx_clone_file = tx_event.clone();
        let window_clone = window.clone();
        attach_btn.connect_clicked(move |_| {
            let tx = tx_clone_file.clone();
            let parent_window = window_clone.clone();

            glib::MainContext::default().spawn_local(async move {
                let dialog = FileDialog::new();
                // dialog.set_title("Select File to Upload"); // Not available in all GTK versions, safe to omit

                if let Ok(file) = dialog.open_future(Some(&parent_window)).await {
                    if let Some(path) = file.path() {
                        let path_str = path.to_string_lossy().to_string();
                        // For now, just send the path as input or handle it differently.
                        // Ideally we'd have a specific Event::FileSelected
                        let _ = tx.send(Event::Input(format!("/upload {}", path_str))).await;
                    }
                }
            });
        });

        // Input Field (Center)
        let input_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .propagate_natural_height(true)
            .max_content_height(500)
            .vexpand(true)
            .valign(Align::Fill)
            .has_frame(false)
            .build();
        input_scroll.set_hexpand(true);
        input_scroll.add_css_class("chat-input-area");

        let text_view = SourceView::builder()
            .wrap_mode(gtk4::WrapMode::WordChar)
            .show_line_numbers(false)
            .auto_indent(true)
            .accepts_tab(false)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(10)
            .right_margin(10)
            .build();

        text_view.add_css_class("transparent-text");
        input_scroll.set_child(Some(&text_view));

        // Send Button (Right)
        let send_icon = Image::from_icon_name("paper-plane-symbolic");
        send_icon.set_pixel_size(24);

        let send_btn = Button::builder()
            .valign(Align::End)
            .css_classes(vec!["suggested-action"])
            .child(&send_icon)
            .build();

        let tx_clone_send = tx_event.clone();
        let buffer = text_view.buffer();

        // --- ENTER KEY HANDLER (CAPTURE PHASE) ---
        let key_controller = EventControllerKey::new();
        key_controller.set_propagation_phase(PropagationPhase::Capture);
        let tx_clone_key = tx_event.clone();
        let buffer_key = buffer.clone();
        key_controller.connect_key_pressed(move |_ctrl, key, _keycode, state| {
            if key == Key::Return && !state.contains(ModifierType::SHIFT_MASK) {
                let (start, end) = buffer_key.bounds();
                let text = buffer_key.text(&start, &end, false).to_string();
                if !text.trim().is_empty() {
                    let _ = tx_clone_key.send_blocking(Event::Input(text));
                    buffer_key.set_text("");
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        text_view.add_controller(key_controller);

        // Click Handler
        send_btn.connect_clicked(move |_| {
            let (start, end) = buffer.bounds();
            let text = buffer.text(&start, &end, false).to_string();
            if !text.trim().is_empty() {
                let _ = tx_clone_send.send_blocking(Event::Input(text));
                buffer.set_text("");
            }
        });

        input_container.append(&attach_btn);
        input_container.append(&input_scroll);
        input_container.append(&send_btn);

        paned.set_end_child(Some(&input_container));
        body_box.append(&paned);

        // Toggle Sidebar
        let sidebar_box_clone = sidebar_box.clone();
        sidebar_toggle.connect_toggled(move |btn| {
            sidebar_box_clone.set_visible(btn.is_active());
        });

        // --- STATUS UPDATE LOOP ---
        let label_una_clone = label_una.clone();
        let spinner_una_clone = spinner_una.clone();
        let label_s9_clone = label_s9.clone();
        let spinner_s9_clone = spinner_s9.clone();

        glib::MainContext::default().spawn_local(async move {
            while let Ok(update) = rx.recv().await {
                match update {
                    GuiUpdate::ShardStatusChanged { id, status } => {
                        let (spinner, label, name) = if id == "una-prime" {
                            (&spinner_una_clone, &label_una_clone, "Una-Prime")
                        } else if id == "s9-mule" {
                            (&spinner_s9_clone, &label_s9_clone, "S9-Mule")
                        } else {
                            continue;
                        };

                        match status {
                            ShardStatus::Thinking => {
                                spinner.start();
                                label.set_text(&format!("{} (Thinking)", name));
                            }
                            ShardStatus::Online => {
                                spinner.stop();
                                label.set_text(name);
                            }
                            ShardStatus::Error => {
                                spinner.stop();
                                label.set_text(&format!("{} (Error)", name));
                            }
                            _ => {
                                spinner.stop();
                                label.set_text(&format!("{} ({:?})", name, status));
                            }
                        }
                    }
                    GuiUpdate::SidebarStatus(state) => {
                         // Optional: Global Pulse
                    }
                    _ => {}
                }
            }
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
            if let Some(app_win) = window.dynamic_cast_ref::<gtk4::ApplicationWindow>() {
                app_win.set_titlebar(Some(&header_bar));
            }
            body_box.into()
        }
    }
}
