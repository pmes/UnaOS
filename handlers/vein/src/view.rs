// handlers/vein/src/view.rs
use async_channel::Receiver;
use gtk4::prelude::*;
use gtk4::{
    Adjustment, Align, Box, Button, ColumnView, ColumnViewColumn, CssProvider, DropDown,
    EventControllerKey, FileDialog, Image, Label, ListBox, ListItem, MenuButton, Orientation,
    Paned, PolicyType, Popover, PropagationPhase, Scale, ScrolledWindow, Separator,
    SignalListItemFactory, SingleSelection, Spinner, Stack, StackSwitcher, StackTransitionType,
    StringList, StringObject, Switch, TextBuffer, TextView, ToggleButton, Widget, Window,
    gdk::{Key, ModifierType},
    gio,
};
#[cfg(not(feature = "gnome"))]
use gtk4::HeaderBar;

use sourceview5::View as SourceView;

// Import Elessar (Engine)
use elessar::gneiss_pal::shard::ShardStatus;
use elessar::prelude::*; // Provides Event, GuiUpdate, AppHandler // Specific import if not in prelude (it's not)

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

        let provider = CssProvider::new();
        provider.load_from_string("
            .sidebar { background-color: #1e1e1e; color: #ffffff; }
            .console { background-color: #101010; color: #dddddd; font-family: 'Monospace'; caret-color: #dddddd; padding: 12px; }
            .chat-input-area { background-color: #2d2d2d; border-radius: 12px; padding: 2px; }
            textview.transparent-text { background-color: transparent; color: #ffffff; caret-color: #ffffff; font-family: 'Sans'; font-size: 15px; padding: 6px; }
            textview.transparent-text text { background-color: transparent; color: #ffffff; }
            .suggested-action { background-color: #0078d4; color: #ffffff; border-radius: 4px; padding: 0px; min-width: 34px; min-height: 34px; margin-left: 8px; }
            .suggested-action image { -gtk-icon-style: symbolic; color: #ffffff; }
            .attach-action { background-color: #333333; color: #cccccc; border-radius: 4px; padding: 0px; min-width: 42px; min-height: 42px; margin-right: 8px; }
            .attach-action image { -gtk-icon-style: symbolic; color: inherit; }
            .attach-action:hover { color: #ffffff; background-color: #444444; }
            .attach-action:active { background-color: #222222; }
            .shard-list { background-color: transparent; }
            window { background-color: #1e1e1e; }
            stackswitcher button { background: transparent; color: #888888; border: none; box-shadow: none; padding: 8px 16px; font-weight: bold; }
            stackswitcher button:checked { color: #ffffff; border-bottom: 2px solid #0078d4; background: rgba(255, 255, 255, 0.05); }
            stackswitcher button:hover { background: rgba(255, 255, 255, 0.1); }
        ");

        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("No display"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let menu_box = Box::new(Orientation::Vertical, 5);
        menu_box.set_margin_start(10);
        menu_box.set_margin_end(10);
        menu_box.set_margin_top(10);
        menu_box.set_margin_bottom(10);

        let btn_clear = Button::new();
        let icon_clear = Image::from_icon_name("user-trash-symbolic");
        icon_clear.set_pixel_size(16);
        btn_clear.set_child(Some(&icon_clear));
        btn_clear.set_tooltip_text(Some("Clear Console"));
        let tx_clear = tx_event.clone();
        btn_clear.connect_clicked(move |_| {
            let _ = tx_clear.send_blocking(Event::Input("/clear".into()));
        });

        let btn_wolf = Button::new();
        let icon_wolf = Image::from_icon_name("view-grid-symbolic");
        icon_wolf.set_pixel_size(16);
        btn_wolf.set_child(Some(&icon_wolf));
        btn_wolf.set_tooltip_text(Some("Wolfpack Mode"));
        let tx_wolf = tx_event.clone();
        btn_wolf.connect_clicked(move |_| {
            let _ = tx_wolf.send_blocking(Event::Input("/wolf".into()));
        });

        let btn_comms = Button::new();
        let icon_comms = Image::from_icon_name("call-start-symbolic");
        icon_comms.set_pixel_size(16);
        btn_comms.set_child(Some(&icon_comms));
        btn_comms.set_tooltip_text(Some("Comms Mode"));
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

        let body_box = Box::new(Orientation::Horizontal, 0);

        let sidebar_box = Box::new(Orientation::Vertical, 0);
        sidebar_box.set_width_request(200);
        sidebar_box.add_css_class("sidebar");

        let sidebar_stack = Stack::new();
        sidebar_stack.set_vexpand(true);
        sidebar_stack.set_transition_type(StackTransitionType::SlideLeftRight);

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
        let column = ColumnViewColumn::new(None, Some(factory));
        column_view.append_column(&column);

        let tx_clone_nav = tx_event.clone();
        column_view
            .model()
            .unwrap()
            .connect_selection_changed(move |model, _pos, _n_items| {
                let selection = model.downcast_ref::<SingleSelection>().unwrap();
                if let Some(_selected_item) = selection.selected_item() {
                    let idx = selection.selected() as usize;
                    let _ = tx_clone_nav.send_blocking(Event::NavSelect(idx));
                }
            });

        let rooms_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .child(&column_view)
            .vexpand(true)
            .build();

        // Context Menu for Nodes
        let menu_model = gio::Menu::new();
        menu_model.append(Some("Connect"), Some("node.connect"));
        menu_model.append(Some("Edit (Principia)"), Some("node.edit"));
        menu_model.append(Some("Delete"), Some("node.delete"));

        let popover_menu = Popover::builder()
            .child(&Box::new(Orientation::Vertical, 0)) // Placeholder, PopoverMenu handles model
            .build();
        // GTK4 PopoverMenu from model is tricky without detailed setup,
        // using simple Popover with buttons for robustness in this snippet context
        // but following the directive "ColumnView needs a GtkPopoverMenu"
        // simpler approach: Right click controller on ColumnView

        let click_controller = gtk4::GestureClick::new();
        click_controller.set_button(3); // Right click

        // Manual menu construction for robustness
        let menu_box = Box::new(Orientation::Vertical, 0);
        let btn_conn = Button::with_label("Connect");
        btn_conn.add_css_class("flat");
        let btn_edit = Button::with_label("Edit (Principia)");
        btn_edit.add_css_class("flat");
        let btn_del = Button::with_label("Delete");
        btn_del.add_css_class("flat");

        menu_box.append(&btn_conn);
        menu_box.append(&btn_edit);
        menu_box.append(&btn_del);

        let ctx_popover = Popover::builder()
            .child(&menu_box)
            .has_arrow(false)
            .build();

        click_controller.connect_pressed(move |_gesture, _n, x, y| {
            ctx_popover.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            ctx_popover.popup();
        });
        column_view.add_controller(click_controller);

        // sidebar_stack.add_titled(&rooms_scroll, Some("nodes"), "Nodes"); // Moved down to layout

        let status_box = Box::new(Orientation::Vertical, 10);
        status_box.set_margin_top(10);
        let shard_list = ListBox::new();
        shard_list.add_css_class("shard-list");

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

        // Sidebar Footer Structure (Directive S69)
        let footer = Box::new(Orientation::Vertical, 8);
        footer.set_margin_start(10);
        footer.set_margin_end(10);
        footer.set_margin_bottom(10);
        footer.set_margin_top(10);

        // A. The Action Row (+ and Filters)
        let actions_box = Box::new(Orientation::Horizontal, 5);
        actions_box.set_halign(Align::Center);

        let new_node_btn = Button::new();
        let icon_new_node = Image::from_icon_name("list-add-symbolic");
        icon_new_node.set_pixel_size(16);
        new_node_btn.set_child(Some(&icon_new_node));
        new_node_btn.set_tooltip_text(Some("New Node"));
        new_node_btn.add_css_class("flat");

        let tx_node_create = tx_event.clone();
        let parent_win = window.upcast_ref::<Window>().clone();

        new_node_btn.connect_clicked(move |_| {
            let dialog = Window::builder()
                .title("New Node Configuration")
                .modal(true)
                .transient_for(&parent_win)
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
            let prompt_buffer = TextBuffer::new(None);
            let prompt_view = TextView::with_buffer(&prompt_buffer);
            prompt_view.set_wrap_mode(gtk4::WrapMode::WordChar);
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

        let filter_group = Box::new(Orientation::Horizontal, 0);
        filter_group.add_css_class("linked");

        let btn_exec = ToggleButton::with_label("EXEC");
        let btn_arch = ToggleButton::with_label("ARCH");
        let btn_debug = ToggleButton::with_label("DEBUG");
        let btn_una = ToggleButton::with_label("UNA");

        for (btn, action) in [
            (&btn_exec, "exec"),
            (&btn_arch, "arch"),
            (&btn_debug, "debug"),
            (&btn_una, "una")
        ] {
             btn.add_css_class("small-button");
             let tx = tx_event.clone();
             let action_str = action.to_string();
             btn.connect_toggled(move |b| {
                 let _ = tx.send_blocking(Event::NodeAction {
                     action: action_str.clone(),
                     active: b.is_active()
                 });
             });
        }

        filter_group.append(&btn_exec);
        filter_group.append(&btn_arch);
        filter_group.append(&btn_debug);
        filter_group.append(&btn_una);

        actions_box.append(&new_node_btn);
        actions_box.append(&filter_group);

        footer.append(&actions_box);

        // B. The Mode Switcher
        stack_switcher.set_halign(Align::Center);
        footer.append(&stack_switcher);

        // Sidebar Assembly (Three-Tier)
        // 1. Content (Stack) - expands
        // 2. Separator
        // 3. Footer (Fixed)

        sidebar_stack.add_titled(&rooms_scroll, Some("nodes"), "Nodes"); // Nodes is just the list now

        // Note: sidebar_stack was appended to sidebar_box earlier.

        sidebar_box.append(&Separator::new(Orientation::Horizontal));
        sidebar_box.append(&footer);

        body_box.append(&sidebar_box);
        body_box.append(&Separator::new(Orientation::Vertical));

        let paned = Paned::new(Orientation::Vertical);
        paned.set_vexpand(true);
        paned.set_hexpand(true);
        paned.set_position(550);

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
            .margin_start(12)
            .margin_end(12)
            .margin_top(12)
            .margin_bottom(12)
            .build();
        console_text_view.add_css_class("console");

        let text_buffer_clone = text_buffer.clone();
        let scrolled_window_adj = scrolled_window.vadjustment();
        let scroll_adj_clone = scrolled_window_adj.clone();

        scrolled_window.set_child(Some(&console_text_view));
        paned.set_start_child(Some(&scrolled_window));

        let input_container = Box::new(Orientation::Horizontal, 8);
        input_container.set_valign(Align::Fill);
        input_container.set_margin_start(16);
        input_container.set_margin_end(16);
        input_container.set_margin_bottom(16);
        input_container.set_margin_top(16);

        let attach_icon = Image::from_icon_name("share-symbolic");
        attach_icon.set_pixel_size(24);
        let attach_btn = Button::builder()
            .valign(Align::End)
            .css_classes(vec!["attach-action"])
            .tooltip_text("Attach File")
            .child(&attach_icon)
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

        let send_icon = Image::from_icon_name("paper-plane-symbolic");
        send_icon.set_pixel_size(24);
        let send_btn = Button::builder()
            .valign(Align::End)
            .css_classes(vec!["suggested-action"])
            .tooltip_text("Send Message (Ctrl+Enter)")
            .child(&send_icon)
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
            if key != Key::Return {
                return glib::Propagation::Proceed;
            }

            if state.contains(ModifierType::SHIFT_MASK) {
                return glib::Propagation::Proceed;
            }

            let is_ctrl = state.contains(ModifierType::CONTROL_MASK);
            let line_count = buffer_key.line_count();

            if is_ctrl || line_count <= 1 {
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
        paned.set_end_child(Some(&input_container));
        body_box.append(&paned);

        let sidebar_box_clone = sidebar_box.clone();
        sidebar_toggle.connect_toggled(move |btn| {
            sidebar_box_clone.set_visible(btn.is_active());
        });

        let label_una_clone = label_una.clone();
        let spinner_una_clone = spinner_una.clone();
        let label_s9_clone = label_s9.clone();
        let spinner_s9_clone = spinner_s9.clone();

        glib::MainContext::default().spawn_local(async move {
            while let Ok(update) = rx.recv().await {
                match update {
                    GuiUpdate::ConsoleLog(text) => {
                        let mut end_iter = text_buffer_clone.end_iter();
                        text_buffer_clone.insert(&mut end_iter, &text);
                        let adj = scroll_adj_clone.clone();
                        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
                            let val = (adj.upper() - adj.page_size()).max(adj.lower());
                            adj.set_value(val);
                            glib::ControlFlow::Break
                        });
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
                    _ => {}
                }
            }
        });

        #[cfg(feature = "gnome")]
        {
            let view = adw::ToolbarView::new();
            view.add_top_bar(&header_bar);
            view.set_content(Some(&body_box));
            view.upcast::<Widget>()
        }

        #[cfg(not(feature = "gnome"))]
        {
            if let Some(app_win) = window.dynamic_cast_ref::<gtk4::ApplicationWindow>() {
                app_win.set_titlebar(Some(&header_bar));
            }
            body_box.into()
        }
    }
}
