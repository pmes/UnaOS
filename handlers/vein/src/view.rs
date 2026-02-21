// handlers/vein/src/view.rs
use async_channel::Receiver;
use gtk4::prelude::*;
use gtk4::{
    Adjustment, Align, Box, Button, CheckButton, ColumnView, ColumnViewColumn, CssProvider, DropDown, Entry,
    EventControllerKey, FileDialog, Image, Label, ListBox, ListItem,
    Orientation, Paned, PolicyType, Popover, PropagationPhase, Scale, ScrolledWindow,
    SignalListItemFactory, SingleSelection, Spinner, Stack, StackSwitcher, StackTransitionType,
    StringList, StringObject, Switch, ToggleButton, Widget, Window,
    ListView, FilterListModel, Expander, NoSelection, GestureClick,
    gdk::{Key, ModifierType},
    gio,
};

use sourceview5::View as SourceView;
use sourceview5::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use libspelling;
use crate::model::DispatchObject;

// Wrapper to allow storing !Send GObjects in set_data (Safe on main thread)
struct SendWrapper<T>(pub T);
unsafe impl<T> Send for SendWrapper<T> {}
unsafe impl<T> Sync for SendWrapper<T> {}

fn enable_spelling(view: &SourceView) {
    if let Some(buffer) = view.buffer().downcast::<sourceview5::Buffer>().ok() {
        let provider = libspelling::Provider::default();
        let checker = libspelling::Checker::new(Some(&provider), Some("en_US"));
        let adapter = libspelling::TextBufferAdapter::new(&buffer, &checker);
        adapter.set_enabled(true);
        unsafe {
            buffer.set_data("spell-adapter", SendWrapper(adapter));
        }
    }
}

// Import Elessar (Engine)
use elessar::gneiss_pal::shard::ShardStatus;
use elessar::gneiss_pal::{GuiUpdate, WolfpackState};
use elessar::prelude::*;

#[cfg(not(feature = "gnome"))]
use gtk4::HeaderBar;

#[cfg(feature = "gnome")]
use libadwaita as adw;

pub struct CommsSpline {}

impl CommsSpline {
    pub fn new() -> Self {
        Self {}
    }

    pub fn bootstrap<W: IsA<Window> + IsA<Widget> + Cast>(
        &self,
        window: &W,
        tx_event: async_channel::Sender<Event>,
        rx: Receiver<GuiUpdate>,
    ) -> Widget {
        window.set_title(Some("Vein (Trinity Architecture)"));

        // THE PULSE (Stripped of Tab Hacks)
        let provider = CssProvider::new();
        provider.load_from_string("
            .console { font-family: 'Monospace'; background: transparent; }
            .console-row { margin-bottom: 16px; padding: 0px; }

            .bubble-box {
                border-radius: 12px;
                padding: 12px;
            }

            .architect-bubble {
                background-color: alpha(currentColor, 0.08);
            }

            .una-bubble {
                background-color: alpha(currentColor, 0.05);
            }

            .chat-input-area { border-radius: 12px; padding: 2px; border: 1px solid alpha(currentColor, 0.1); }
            .transparent-text { background-color: transparent; font-family: 'Sans'; font-size: 15px; padding: 6px; color: @theme_text_color; }
            .transparent-text text { background-color: transparent; color: inherit; }

            /* Native Icon Scaling */
            .suggested-action { background-color: #0078d4; color: #ffffff; border-radius: 4px; }
            .attach-action { border-radius: 4px; }

            /* Pulse Animation */
            @keyframes throb {
                0% { opacity: 0.3; }
                50% { opacity: 1.0; }
                100% { opacity: 0.3; }
            }
            .pulse-active { animation: throb 1.5s infinite ease-in-out; color: #0078d4; }
            .nexus-header { font-weight: bold; margin-top: 12px; margin-bottom: 4px; opacity: 0.7; font-size: 0.9em; }

            .role-architect { color: #0078d4; font-weight: bold; }
            .role-una { color: #d40078; font-weight: bold; }
            .role-system { color: #888888; font-style: italic; }
        ");

        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("No display"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // UI Controls (Created early so they can be packed into either architecture)
        let sidebar_toggle = ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .active(true)
            .tooltip_text("Toggle Sidebar")
            .build();

        let token_label = Label::new(Some("Tokens: 0"));
        token_label.add_css_class("dim-label");
        token_label.set_margin_end(10);

        let pulse_icon = Image::from_icon_name("activity-start-symbolic");
        pulse_icon.set_pixel_size(16);
        pulse_icon.set_opacity(0.3);

        let status_group = Box::new(Orientation::Horizontal, 8);
        status_group.append(&sidebar_toggle);
        status_group.append(&token_label);
        status_group.append(&pulse_icon);

        // --- Root Layout ---
        let main_h_paned = Paned::new(Orientation::Horizontal);
        main_h_paned.set_position(215);
        main_h_paned.set_hexpand(true);
        main_h_paned.set_vexpand(true);
        main_h_paned.set_wide_handle(false); // Thin vertical divider
        main_h_paned.set_shrink_start_child(false);

        // --- Left Pane (The Silhouette) ---
        let left_vbox = Box::new(Orientation::Vertical, 0);
        left_vbox.add_css_class("background");
        left_vbox.add_css_class("navigation-sidebar");
        left_vbox.set_width_request(215);

        // Sidebar Content
        let sidebar_box = Box::new(Orientation::Vertical, 0);
        sidebar_box.set_vexpand(true);

        let sidebar_stack = Stack::new();
        sidebar_stack.set_vexpand(true);
        sidebar_stack.set_transition_type(StackTransitionType::SlideLeftRight);

        // 1. Nodes Tab
        let store = gio::ListStore::new::<StringObject>();
        for item in ["General", "Encrypted", "Jules (Private)"].iter() {
            store.append(&StringObject::new(item));
        }
        let selection_model = SingleSelection::new(Some(store));
        let factory = SignalListItemFactory::new();
        factory.connect_setup(move |_factory, item| {
            let item = item.downcast_ref::<ListItem>().unwrap();
            let label = Label::new(None);
            label.set_margin_start(10);
            label.set_margin_end(10);
            label.set_margin_top(12);
            label.set_margin_bottom(12);
            label.set_xalign(0.0);
            item.set_child(Some(&label));
        });
        factory.connect_bind(move |_factory, item| {
            let item = item.downcast_ref::<ListItem>().unwrap();
            let child = item.child().unwrap().downcast::<Label>().unwrap();
            let obj = item.item().unwrap().downcast::<StringObject>().unwrap();
            child.set_label(&obj.string());
        });
        let column_view = ColumnView::new(Some(selection_model));
        column_view.append_column(&ColumnViewColumn::new(None, Some(factory)));
        let tx_clone_nav = tx_event.clone();
        column_view.model().unwrap().connect_selection_changed(move |model, _, _| {
            let selection = model.downcast_ref::<SingleSelection>().unwrap();
            if let Some(_) = selection.selected_item() {
                let idx = selection.selected() as usize;
                let _ = tx_clone_nav.send_blocking(Event::NavSelect(idx));
            }
        });
        let nodes_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .child(&column_view)
            .vexpand(true)
            .build();

        let nodes_box = Box::new(Orientation::Vertical, 0);
        nodes_box.append(&nodes_scroll);

        let node_actions_box = Box::new(Orientation::Horizontal, 5);
        node_actions_box.set_halign(Align::Center);
        node_actions_box.set_margin_bottom(10);
        node_actions_box.set_margin_top(10);

        let new_node_btn = Button::new();
        let icon_new_node = Image::from_icon_name("list-add-symbolic");
        new_node_btn.set_child(Some(&icon_new_node));
        new_node_btn.set_tooltip_text(Some("New Node"));
        new_node_btn.add_css_class("flat");

        let tx_node_create = tx_event.clone();
        let _parent_win = window.upcast_ref::<Window>().clone();

        new_node_btn.connect_clicked(move |_| {
            let dialog = Window::builder()
                .title("New Node Configuration")
                .modal(true)
                .transient_for(&_parent_win)
                .default_width(400)
                .default_height(500)
                .build();

            let vbox = Box::new(Orientation::Vertical, 12);
            vbox.set_margin_top(12);
            vbox.set_margin_bottom(12);
            vbox.set_margin_start(12);
            vbox.set_margin_end(12);

            vbox.append(&Label::new(Some("Model")));
            let models = StringList::new(&["Gemini 2.0 Flash", "Gemini 1.5 Pro", "Claude 3.5 Sonnet"]);
            let dropdown = DropDown::new(Some(models), None::<gtk4::Expression>);
            vbox.append(&dropdown);

            let hbox_hist = Box::new(Orientation::Horizontal, 12);
            hbox_hist.append(&Label::new(Some("Enable History")));
            let switch_hist = Switch::new();
            switch_hist.set_active(true);
            hbox_hist.append(&switch_hist);
            vbox.append(&hbox_hist);

            vbox.append(&Label::new(Some("Temperature (0.1 - 0.9)")));
            let adj = Adjustment::new(0.7, 0.1, 0.9, 0.1, 0.0, 0.0);
            let scale = Scale::new(Orientation::Horizontal, Some(&adj));
            scale.set_digits(1);
            scale.set_draw_value(true);
            vbox.append(&scale);

            vbox.append(&Label::new(Some("System Prompt")));
            let prompt_buffer = sourceview5::Buffer::new(None);
            let prompt_view = SourceView::with_buffer(&prompt_buffer);
            prompt_view.set_show_line_numbers(false);
            prompt_view.set_monospace(false);
            prompt_view.set_wrap_mode(gtk4::WrapMode::WordChar);
            enable_spelling(&prompt_view);
            let scroll = ScrolledWindow::builder()
                .child(&prompt_view)
                .vexpand(true)
                .height_request(150)
                .build();
            vbox.append(&scroll);

            let hbox_btns = Box::new(Orientation::Horizontal, 12);
            hbox_btns.set_halign(Align::End);

            let btn_cancel = Button::with_label("Cancel");
            let win_weak = dialog.downgrade();
            btn_cancel.connect_clicked(move |_| {
                if let Some(win) = win_weak.upgrade() {
                    win.close();
                }
            });

            let btn_create = Button::with_label("Create Node");
            btn_create.add_css_class("suggested-action");
            let win_weak2 = dialog.downgrade();
            let tx = tx_node_create.clone();

            btn_create.connect_clicked(move |_| {
                if let Some(win) = win_weak2.upgrade() {
                    let model_obj = dropdown.selected_item().and_then(|obj| obj.downcast::<StringObject>().ok());
                    let model = model_obj.map(|s| s.string().to_string()).unwrap_or_default();
                    let history = switch_hist.is_active();
                    let temp = adj.value();
                    let (start, end) = prompt_buffer.bounds();
                    let prompt = prompt_buffer.text(&start, &end, false).to_string();

                    let _ = tx.send_blocking(Event::CreateNode {
                        model,
                        history,
                        temperature: temp,
                        system_prompt: prompt,
                    });
                    win.close();
                }
            });

            hbox_btns.append(&btn_cancel);
            hbox_btns.append(&btn_create);
            vbox.append(&hbox_btns);

            dialog.set_child(Some(&vbox));
            dialog.present();
        });
        node_actions_box.append(&new_node_btn);

        // THE COMPOSER
        let composer_icon = Image::from_icon_name("chat-message-new-symbolic");
        let composer_btn = Button::builder()
            .css_classes(vec!["flat"])
            .tooltip_text("Open Composer (Formal Command)")
            .child(&composer_icon)
            .build();

        node_actions_box.append(&composer_btn);
        nodes_box.append(&node_actions_box);

        sidebar_stack.add_titled(&nodes_box, Some("nodes"), "Nodes");

        // 2. THE NEXUS Tab
        let nexus_box = Box::new(Orientation::Vertical, 0);
        nexus_box.set_margin_start(10);
        nexus_box.set_margin_end(10);
        nexus_box.set_margin_top(10);

        let nexus_list = ListBox::new();
        nexus_list.add_css_class("shard-list");
        nexus_list.set_selection_mode(gtk4::SelectionMode::None);

        nexus_list.append(&Label::builder().label("PRIMES").xalign(0.0).css_classes(vec!["nexus-header"]).build());

        let row_una = Box::new(Orientation::Horizontal, 10);
        let icon_una = Image::from_icon_name("computer-symbolic");
        let label_una = Label::new(Some("Una-Prime"));
        let spinner_una = Spinner::new();
        row_una.append(&icon_una);
        row_una.append(&label_una);
        row_una.append(&spinner_una);
        nexus_list.append(&row_una);

        let row_claude = Box::new(Orientation::Horizontal, 10);
        let icon_claude = Image::from_icon_name("avatar-default-symbolic");
        let label_claude = Label::new(Some("Claude-Prime"));
        let spinner_claude = Spinner::new();
        row_claude.append(&icon_claude);
        row_claude.append(&label_claude);
        row_claude.append(&spinner_claude);
        nexus_list.append(&row_claude);

        nexus_list.append(&Label::builder().label("SUB-PROCESSES").xalign(0.0).css_classes(vec!["nexus-header"]).build());

        let row_s9 = Box::new(Orientation::Horizontal, 10);
        row_s9.set_margin_start(15);
        let icon_s9 = Image::from_icon_name("network-server-symbolic");
        let label_s9 = Label::new(Some("S9-Mule"));
        let spinner_s9 = Spinner::new();
        row_s9.append(&icon_s9);
        row_s9.append(&label_s9);
        row_s9.append(&spinner_s9);
        nexus_list.append(&row_s9);

        nexus_box.append(&nexus_list);
        sidebar_stack.add_titled(&nexus_box, Some("nexus"), "Nexus");

        sidebar_box.append(&sidebar_stack);

        // Native Tabs (No CSS overrides)
        let stack_switcher = StackSwitcher::builder().stack(&sidebar_stack).halign(Align::Center).build();
        sidebar_box.append(&stack_switcher);

        left_vbox.append(&sidebar_box);
        main_h_paned.set_start_child(Some(&left_vbox));

        // --- Right Pane (The Command Center) ---
        let right_vbox = Box::new(Orientation::Vertical, 0);
        right_vbox.set_hexpand(true);

        // --- ARCHITECTURE SPLIT (ADWAITA vs PURE GTK) ---
        #[cfg(feature = "gnome")]
        {
            // Adwaita Split HeaderBars
            let blank_header_bar = adw::HeaderBar::new();
            blank_header_bar.set_show_start_title_buttons(false);
            blank_header_bar.set_show_end_title_buttons(false);
            blank_header_bar.set_title_widget(Some(&adw::WindowTitle::new("", "")));
            left_vbox.prepend(&blank_header_bar); // Add to top of left column

            let command_header_bar = adw::HeaderBar::new();
            command_header_bar.set_show_start_title_buttons(true);
            command_header_bar.set_show_end_title_buttons(true);
            command_header_bar.set_title_widget(Some(&adw::WindowTitle::new("Lumen", "")));
            command_header_bar.pack_start(&status_group);
            right_vbox.append(&command_header_bar); // Add to top of right column
        }

        #[cfg(not(feature = "gnome"))]
        {
            // Pure GTK Unified HeaderBar
            let unified_header_bar = HeaderBar::new();
            unified_header_bar.set_show_title_buttons(true);
            unified_header_bar.set_title_widget(Some(&Label::new(Some("Lumen"))));

            // The Phantom Spacer: Matches the sidebar width and syncs with the toggle
            let header_spacer = Box::new(Orientation::Horizontal, 0);

            // 1. Bind the spacer's width directly to the Paned slider's pixel position
            main_h_paned.bind_property("position", &header_spacer, "width-request")
                .sync_create()
                .build();

            // 2. Bind the visibility to the toggle button
            sidebar_toggle.bind_property("active", &header_spacer, "visible")
                .sync_create()
                .build();

            // Pack the spacer first to push the status group over
            unified_header_bar.pack_start(&header_spacer);
            unified_header_bar.pack_start(&status_group);

            if let Some(app_win) = window.dynamic_cast_ref::<gtk4::ApplicationWindow>() {
                app_win.set_titlebar(Some(&unified_header_bar));
            }
        }

        // --- Main Content (Console/Input Slider) ---
        let main_paned = Paned::new(Orientation::Vertical);
        main_paned.set_vexpand(true);
        main_paned.set_hexpand(true);
        main_paned.set_position(9999);
        main_paned.set_shrink_end_child(false);
        main_paned.set_wide_handle(true); // Restores the horizontal slider grip

        // Console (REFACTORED FOR LISTVIEW)
        let scrolled_window = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .build();

        let console_store = gio::ListStore::new::<DispatchObject>();
        let console_filter = FilterListModel::new(Some(console_store.clone()), None::<gtk4::Filter>);
        let console_selection = NoSelection::new(Some(console_filter));

        let console_factory = SignalListItemFactory::new();
        console_factory.connect_setup(move |_factory, item| {
            let item = item.downcast_ref::<ListItem>().unwrap();

            // Root Container (Expanded Horizontally)
            let root = Box::new(Orientation::Horizontal, 0);
            root.set_hexpand(true);
            root.add_css_class("console-row");

            // Left Spacer
            let left_spacer = Box::new(Orientation::Horizontal, 0);
            left_spacer.set_hexpand(true);
            root.append(&left_spacer);

            // Bubble Container (Restricted Width)
            let bubble = Box::new(Orientation::Vertical, 4);
            bubble.add_css_class("bubble-box");
            bubble.set_width_request(400); // Minimum comfortable width

            // 1. Meta Label
            let meta_label = Label::new(None);
            meta_label.set_xalign(0.0);
            meta_label.add_css_class("dim-label");
            bubble.append(&meta_label);

            // 2. Chat Content
            let chat_content_buffer = sourceview5::Buffer::new(None);
            let chat_content_view = SourceView::with_buffer(&chat_content_buffer);
            chat_content_view.set_editable(false);
            chat_content_view.set_wrap_mode(gtk4::WrapMode::WordChar);
            chat_content_view.set_show_line_numbers(false);
            chat_content_view.add_css_class("transparent-text");
            chat_content_view.set_monospace(false);
            chat_content_view.set_width_request(600); // Forces a minimum readable width
            chat_content_view.set_hexpand(true);
            chat_content_view.set_focusable(true); // Allow copy paste
            bubble.append(&chat_content_view);

            // 3. Payload Expander
            let expander = Expander::new(None);
            let expander_label = Label::new(None);
            expander.set_child(Some(&expander_label));

            let payload_content_buffer = sourceview5::Buffer::new(None);
            let payload_content_view = SourceView::with_buffer(&payload_content_buffer);
            payload_content_view.set_editable(false);
            payload_content_view.set_wrap_mode(gtk4::WrapMode::WordChar);
            payload_content_view.set_show_line_numbers(true);
            payload_content_view.set_monospace(true);

            let payload_scroll = ScrolledWindow::builder()
                .child(&payload_content_view)
                .height_request(200)
                .build();

            expander.set_child(Some(&payload_scroll));
            bubble.append(&expander);

            root.append(&bubble);

            // Right Spacer
            let right_spacer = Box::new(Orientation::Horizontal, 0);
            right_spacer.set_hexpand(true);
            root.append(&right_spacer);

            // Expansion Gesture (Setup Phase)
            let gesture = GestureClick::new();
            let item_clone = item.clone();
            let chat_content_view_clone = chat_content_view.clone();

            gesture.connect_pressed(move |_, n_press, _, _| {
                if n_press == 1 {
                    if let Some(obj) = item_clone.item().and_downcast::<crate::model::DispatchObject>() {
                        let expanded = !obj.is_expanded();
                        obj.set_is_expanded(expanded);

                        let content = obj.content();
                        let line_count = content.lines().count();

                        if line_count > 11 && !expanded {
                            let truncated: String = content.lines().take(11).collect::<Vec<&str>>().join("\n") + "\n\n... [Click to expand]";
                            chat_content_view_clone.buffer().set_text(&truncated);
                        } else {
                            chat_content_view_clone.buffer().set_text(&content);
                        }
                    }
                }
            });
            bubble.add_controller(gesture);

            item.set_child(Some(&root));
        });

        console_factory.connect_bind(move |_factory, item| {
            let item = item.downcast_ref::<ListItem>().unwrap();
            let root = item.child().unwrap().downcast::<Box>().unwrap();

            let mut iter = root.first_child();
            let left_spacer = iter.unwrap().downcast::<Box>().unwrap();
            iter = left_spacer.next_sibling();
            let bubble = iter.unwrap().downcast::<Box>().unwrap();
            iter = bubble.next_sibling();
            let right_spacer = iter.unwrap().downcast::<Box>().unwrap();

            let obj = item.item().unwrap().downcast::<DispatchObject>().unwrap();

            // Children: Meta Label (0), Chat View (1), Expander (2)
            let mut iter_bubble = bubble.first_child();
            let meta_label = iter_bubble.unwrap().downcast::<Label>().unwrap();
            iter_bubble = meta_label.next_sibling();
            let chat_view = iter_bubble.unwrap().downcast::<SourceView>().unwrap();
            iter_bubble = chat_view.next_sibling();
            let expander = iter_bubble.unwrap().downcast::<Expander>().unwrap();

            // Gesture logic handled in setup via item.item() dynamic retrieval

            let is_chat = obj.is_chat();
            let sender = obj.sender();
            let timestamp = obj.timestamp();
            let content = obj.content();
            let subject = obj.subject();

            // Reset Bubble Classes and Visibility
            bubble.remove_css_class("architect-bubble");
            bubble.remove_css_class("una-bubble");
            left_spacer.set_visible(false);
            right_spacer.set_visible(false);

            // Expansion Logic (GestureClick)
            // Remove old controllers to prevent stacking if recycled
            // Ideally we'd store the controller but for now we clear or just add new one if careful.
            // Since bind is called often, adding a controller every time is a leak/duplicate risk.
            // Correct way: Check if controller exists or use a custom widget subclass.
            // Hack for SignalListItemFactory: We can store the controller in the widget data or just not re-add if present.
            // Better: attach controller in setup, and in bind just update state?
            // BUT we need the specific object in the closure.
            // The factory `setup` creates the gesture.

            if is_chat {
                chat_view.set_visible(true);
                expander.set_visible(false);

                meta_label.set_text(&format!("{} • {}", sender, timestamp));

                // Style sender
                meta_label.remove_css_class("role-architect");
                meta_label.remove_css_class("role-una");
                meta_label.remove_css_class("role-system");

                if sender == "Architect" {
                    // Architect (Right Aligned): Left Spacer Visible, Right Spacer Hidden
                    meta_label.add_css_class("role-architect");
                    bubble.add_css_class("architect-bubble");
                    left_spacer.set_visible(true);
                    right_spacer.set_visible(false);
                    meta_label.set_xalign(1.0);
                } else {
                    // Una/System (Left Aligned): Left Spacer Hidden, Right Spacer Visible
                    if sender == "Una-Prime" {
                        meta_label.add_css_class("role-una");
                    } else {
                        meta_label.add_css_class("role-system");
                    }
                    bubble.add_css_class("una-bubble");
                    left_spacer.set_visible(false);
                    right_spacer.set_visible(true);
                    meta_label.set_xalign(0.0);
                }

                // Truncation Logic
                let is_expanded = obj.is_expanded();
                let line_count = content.lines().count();
                if line_count > 11 && !is_expanded {
                    let truncated: String = content.lines().take(11).collect::<Vec<&str>>().join("\n") + "\n\n... [Click to expand]";
                    chat_view.buffer().set_text(&truncated);
                } else {
                    chat_view.buffer().set_text(&content);
                }

            } else {
                chat_view.set_visible(false);
                expander.set_visible(true);

                // Payloads generally align left but take full space if possible,
                // but for bubbles we can stick to Una style (Left aligned) or fill.
                // Let's stick to Una style (Left Aligned) with spacer to avoid full stretch.
                bubble.add_css_class("una-bubble"); // Default to dark background
                left_spacer.set_visible(false);
                right_spacer.set_visible(true);

                expander.set_label(Some(&format!("{} | {} | {}", sender, subject, timestamp)));

                let scroll = expander.child().unwrap().downcast::<ScrolledWindow>().unwrap();
                let content_view = scroll.child().unwrap().downcast::<SourceView>().unwrap();
                content_view.buffer().set_text(&content);
                expander.set_expanded(false);
            }
        });

        let console_list_view = ListView::new(Some(console_selection), Some(console_factory));
        console_list_view.add_css_class("console");

        // Disable click selection highlighting visually if desired, but SingleSelection is needed for list view.
        // CSS .console:selected { background: transparent; } might be needed in provider.

        let scrolled_window_adj = scrolled_window.vadjustment();
        let scroll_adj_clone = scrolled_window_adj.clone();
        scrolled_window.set_child(Some(&console_list_view));

        // --- The Spatial Cortex (Euclase Target) ---
        let spatial_canvas = gtk4::Picture::new();
        spatial_canvas.set_hexpand(true);
        spatial_canvas.set_vexpand(true);
        // We will bind this to a toggle button later to reveal the 3rd pane.

        // Attach console to the top pane
        main_paned.set_start_child(Some(&scrolled_window));

        // --- Input Area ---
        let input_container = Box::new(Orientation::Horizontal, 8);
        input_container.set_valign(Align::Fill);
        input_container.set_margin_start(16);
        input_container.set_margin_end(16);
        input_container.set_margin_bottom(16);
        input_container.set_margin_top(16);

        // Native Button Scaling Fix
        let attach_btn = Button::builder()
            .valign(Align::End)
            .icon_name("share-symbolic")
            .css_classes(vec!["attach-action"])
            .tooltip_text("Attach File")
            .build();

        let tx_clone_file = tx_event.clone();
        let window_clone = window.clone();
        attach_btn.connect_clicked(move |_| {
            let tx = tx_clone_file.clone();
            let parent_window = window_clone.clone();
            glib::MainContext::default().spawn_local(async move {
                let dialog = FileDialog::new();
                let result = dialog.open_future(Some(&parent_window)).await;
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        let path_str = path.to_string_lossy().to_string();
                        let _ = tx.send(Event::Input(format!("/upload {}", path_str))).await;
                    }
                }
            });
        });

        // THE COMPOSER
        let active_directive = Rc::new(RefCell::new("Directive 055".to_string()));
        let active_directive_clone = active_directive.clone();

        let tx_composer = tx_event.clone();
        let popover_composer = Popover::builder().build();
        let pop_box = Box::new(Orientation::Vertical, 8);
        pop_box.set_margin_top(10);
        pop_box.set_margin_bottom(10);
        pop_box.set_margin_start(10);
        pop_box.set_margin_end(10);
        pop_box.set_width_request(400);

        let action_box = Box::new(Orientation::Horizontal, 0);
        action_box.add_css_class("linked");
        let btn_exec = ToggleButton::with_label("EXEC");
        let btn_arch = ToggleButton::with_label("ARCH");
        let btn_debug = ToggleButton::with_label("DEBUG");
        let btn_una = ToggleButton::with_label("UNA");

        btn_arch.set_group(Some(&btn_exec));
        btn_debug.set_group(Some(&btn_exec));
        btn_una.set_group(Some(&btn_exec));
        btn_exec.set_active(true);

        action_box.append(&btn_exec);
        action_box.append(&btn_arch);
        action_box.append(&btn_debug);
        action_box.append(&btn_una);
        pop_box.append(&action_box);

        let subject_entry = Entry::new();
        subject_entry.set_placeholder_text(Some("Subject"));
        pop_box.append(&subject_entry);

        let body_buffer = sourceview5::Buffer::new(None);
        let body_view = SourceView::with_buffer(&body_buffer);
        body_view.set_show_line_numbers(false);
        body_view.set_monospace(false);
        body_view.set_wrap_mode(gtk4::WrapMode::WordChar);
        enable_spelling(&body_view);
        body_view.set_height_request(150);
        let body_scroll = ScrolledWindow::builder()
            .child(&body_view)
            .has_frame(true)
            .vexpand(true)
            .build();
        pop_box.append(&body_scroll);

        let pb_check = CheckButton::with_label("Point Break");
        pop_box.append(&pb_check);

        let btn_comp_send = Button::with_label("Transmit Order");
        btn_comp_send.add_css_class("suggested-action");
        let pop_weak = popover_composer.downgrade();

        let sub_ent = subject_entry.clone();
        let bod_buf = body_buffer.clone();
        let pb_chk = pb_check.clone();
        let b_ex = btn_exec.clone();
        let b_ar = btn_arch.clone();
        let b_db = btn_debug.clone();
        let _b_un = btn_una.clone();

        btn_comp_send.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                let subject = sub_ent.text().to_string();
                let (start, end) = bod_buf.bounds();
                let body = bod_buf.text(&start, &end, false).to_string();
                let pb = pb_chk.is_active();
                let action = if b_ex.is_active() { "exec" }
                             else if b_ar.is_active() { "arch" }
                             else if b_db.is_active() { "debug" }
                             else { "una" };

                let _ = tx_composer.send_blocking(Event::ComplexInput {
                    subject,
                    body,
                    point_break: pb,
                    action: action.to_string(),
                });
                pop.popdown();
            }
        });
        pop_box.append(&btn_comp_send);
        popover_composer.set_child(Some(&pop_box));

        let ad_ref = active_directive.clone();
        let sub_ent_pop = subject_entry.clone();
        composer_btn.connect_clicked(move |btn| {
            sub_ent_pop.set_text(&ad_ref.borrow());
            popover_composer.set_parent(btn);
            popover_composer.popup();
        });

        // Chat Input
        let input_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .propagate_natural_height(true)
            .max_content_height(500)
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
        enable_spelling(&text_view);
        text_view.add_css_class("transparent-text");
        input_scroll.set_child(Some(&text_view));

        // Native Button Scaling Fix
        let send_btn = Button::builder()
            .valign(Align::End)
            .icon_name("paper-plane-symbolic")
            .css_classes(vec!["suggested-action"])
            .tooltip_text("Send Message (Ctrl+Enter)")
            .build();

        let tx_clone_send = tx_event.clone();
        let buffer = text_view.buffer();
        let btn_send_clone = send_btn.clone();
        buffer.connect_changed(move |buf| {
            if buf.line_count() > 1 {
                btn_send_clone.remove_css_class("suggested-action");
            } else {
                btn_send_clone.add_css_class("suggested-action");
            }
        });
        let key_controller = EventControllerKey::new();
        key_controller.set_propagation_phase(PropagationPhase::Capture);
        let tx_clone_key = tx_event.clone();
        let buffer_key = buffer.clone();
        key_controller.connect_key_pressed(move |_ctrl, key, _keycode, state| {
            if key != Key::Return { return glib::Propagation::Proceed; }
            if state.contains(ModifierType::SHIFT_MASK) { return glib::Propagation::Proceed; }
            let is_ctrl = state.contains(ModifierType::CONTROL_MASK);
            if is_ctrl || buffer_key.line_count() <= 1 {
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

        // Attach input to bottom pane
        main_paned.set_end_child(Some(&input_container));

        right_vbox.append(&main_paned);
        main_h_paned.set_end_child(Some(&right_vbox));

        let left_vbox_clone = left_vbox.clone();
        sidebar_toggle.connect_toggled(move |btn| {
            left_vbox_clone.set_visible(btn.is_active());
        });

        // Async loop
        let label_una_clone = label_una.clone();
        let spinner_una_clone = spinner_una.clone();
        let label_s9_clone = label_s9.clone();
        let spinner_s9_clone = spinner_s9.clone();
        let token_label_clone = token_label.clone();
        let pulse_icon_clone = pulse_icon.clone();
        let active_directive_async = active_directive_clone.clone();

        glib::MainContext::default().spawn_local(async move {
            while let Ok(update) = rx.recv().await {
                match update {
                    GuiUpdate::Spectrum(_data) => {
                        // Euclase engine hook will go here.
                    }
                    GuiUpdate::ConsoleLog(text) => {
                        // Parse incoming log
                        let mut sender = "System".to_string();
                        let mut is_chat = true;
                        let content = text.clone();
                        let mut subject = "Log".to_string();

                        if text.trim().starts_with("[ARCHITECT]") {
                            sender = "Architect".to_string();
                            is_chat = true;
                        } else if text.trim().starts_with("[UNA]") {
                            sender = "Una-Prime".to_string();
                            is_chat = true;
                        } else if text.trim().starts_with("[S") {
                            // Check if it's likely a shard (S followed by digit)
                            let after_s = &text.trim()[2..];
                            if let Some(first_char) = after_s.chars().next() {
                                if first_char.is_numeric() {
                                    sender = "Shard".to_string();
                                    is_chat = false; // Wolfpack
                                    subject = "Wolfpack Output".to_string();
                                }
                            }
                        }

                        // Generate local timestamp
                        let timestamp = glib::DateTime::now_local()
                            .map(|dt| dt.format("%H:%M:%S").unwrap().to_string())
                            .unwrap_or_else(|_| "00:00:00".to_string());

                        let id = format!("{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));

                        let obj = DispatchObject::new(
                            &id,
                            &sender,
                            &subject,
                            &timestamp,
                            &content,
                            is_chat
                        );

                        console_store.insert(0, &obj);
                    }
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
                                spinner.set_spinning(true);
                                spinner.start();
                                label.set_text(&format!("{} (Thinking)", name));
                            }
                            ShardStatus::Online => {
                                spinner.set_spinning(false);
                                spinner.stop();
                                label.set_text(name);
                            }
                            ShardStatus::Error => {
                                spinner.set_spinning(false);
                                spinner.stop();
                                label.set_text(&format!("{} (Error)", name));
                            }
                            _ => {
                                spinner.set_spinning(false);
                                spinner.stop();
                                label.set_text(&format!("{} ({:?})", name, status));
                            }
                        }
                    }
                    GuiUpdate::SidebarStatus(state) => {
                         match state {
                             WolfpackState::Dreaming => {
                                 pulse_icon_clone.add_css_class("pulse-active");
                             },
                             _ => {
                                 pulse_icon_clone.remove_css_class("pulse-active");
                             }
                         }
                    }
                    GuiUpdate::TokenUsage(tokens) => {
                        token_label_clone.set_text(&format!("Tokens: {}", tokens));
                    }
                    GuiUpdate::ActiveDirective(d) => {
                        *active_directive_async.borrow_mut() = d;
                    }
                    _ => {}
                }
            }
        });

        #[cfg(feature = "gnome")]
        {
            main_h_paned.upcast::<Widget>()
        }

        #[cfg(not(feature = "gnome"))]
        {
            main_h_paned.into()
        }
    }
}
