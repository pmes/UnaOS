// handlers/vein/src/view.rs
use async_channel::Receiver;
use gtk4::prelude::*;
use gtk4::{
    Adjustment, Align, Box, Button, CheckButton, ColumnView, ColumnViewColumn, CssProvider, DropDown, Entry,
    EventControllerKey, FileDialog, GLArea, Image, Label, ListBox, ListItem, MenuButton,
    Orientation, Paned, PolicyType, Popover, PropagationPhase, Scale, ScrolledWindow, Separator,
    SignalListItemFactory, SingleSelection, Spinner, Stack, StackSwitcher, StackTransitionType,
    StringList, StringObject, Switch, TextBuffer, TextView, ToggleButton, Widget, Window,
    gdk::{Key, ModifierType},
    gio,
};
#[cfg(not(feature = "gnome"))]
use gtk4::HeaderBar;

use sourceview5::View as SourceView;
use std::cell::RefCell;
use std::rc::Rc;
use vug::renderer::Renderer;
use vug::{gl, epoxy};

// Import Elessar (Engine)
use elessar::gneiss_pal::shard::ShardStatus;
use elessar::gneiss_pal::{GuiUpdate, WolfpackState};
use elessar::prelude::*;

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

        // THE PULSE (Visual Feedback)
        let provider = CssProvider::new();
        provider.load_from_string("
            box.sidebar { background-color: #28282c; color: #ffffff; }
            .background { background-color: #28282c; color: #ffffff; }
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
            .composer-action { background-color: #333333; color: #cccccc; border-radius: 4px; padding: 0px; min-width: 42px; min-height: 42px; margin-right: 8px; }
            .composer-action:hover { color: #ffffff; background-color: #444444; }
            .pulse-active { color: #0078d4; -gtk-icon-style: symbolic; transition: opacity 1s ease-in-out; }
            window { background-color: #1e1e1e; }
            .nexus-header { font-weight: bold; margin-top: 12px; margin-bottom: 4px; opacity: 0.7; font-size: 0.9em; }
        ");

        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("No display"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // --- Root Layout (Directive 058-B v3) ---
        let main_h_paned = Paned::new(Orientation::Horizontal);
        main_h_paned.set_position(215);
        main_h_paned.set_hexpand(true);
        main_h_paned.set_vexpand(true);
        main_h_paned.set_wide_handle(true); // Directive 061

        // --- Left Pane (The Silhouette) ---
        let left_vbox = Box::new(Orientation::Vertical, 0);
        left_vbox.add_css_class("sidebar");
        left_vbox.add_css_class("background"); // Directive 059 & 060
        left_vbox.add_css_class("navigation-sidebar"); // Directive 061
        left_vbox.set_width_request(215); // Directive 060

        // Blank HeaderBar
        #[cfg(feature = "gnome")]
        let blank_header_bar = {
            let hb = adw::HeaderBar::new();
            hb.set_show_start_title_buttons(false);
            hb.set_show_end_title_buttons(false);
            hb
        };

        #[cfg(not(feature = "gnome"))]
        let blank_header_bar = {
            let hb = HeaderBar::new();
            hb.set_show_title_buttons(false);
            hb
        };

        // Ensure Left Header is Empty
        blank_header_bar.set_title_widget(Some(&Label::new(None)));
        blank_header_bar.add_css_class("titlebar"); // Directive 063: GTK Titlebar Fix

        left_vbox.append(&blank_header_bar);

        // Sidebar Content
        let sidebar_box = Box::new(Orientation::Vertical, 0);
        sidebar_box.set_vexpand(true); // Ensure sidebar fills remaining space

        let sidebar_stack = Stack::new();
        sidebar_stack.set_vexpand(true);
        sidebar_stack.set_transition_type(StackTransitionType::SlideLeftRight);

        // 1. Nodes Tab (Column View)
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

        // Add "New Node" button to Nodes tab bottom?
        // Logic: Keep simple. "New Node" button was in footer.
        // I'll put it at the bottom of the Nodes tab.
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

        // New Node Click Logic (Abbreviated for brevity, same as before)
        let tx_node_create = tx_event.clone();
        let _parent_win = window.upcast_ref::<Window>().clone(); // Omega: _parent_win

        new_node_btn.connect_clicked(move |_| {
            let dialog = Window::builder()
                .title("New Node Configuration")
                .modal(true)
                .transient_for(&_parent_win) // Omega: _parent_win
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
        node_actions_box.append(&new_node_btn);

        // THE COMPOSER (Relocated)
        let composer_icon = Image::from_icon_name("chat-message-new-symbolic");
        let composer_btn = Button::builder()
            .css_classes(vec!["flat"])
            .tooltip_text("Open Composer (Formal Command)")
            .child(&composer_icon)
            .build();

        node_actions_box.append(&composer_btn);
        nodes_box.append(&node_actions_box);

        sidebar_stack.add_titled(&nodes_box, Some("nodes"), "Nodes");

        // 2. THE NEXUS Tab (Hierarchy)
        let nexus_box = Box::new(Orientation::Vertical, 0);
        nexus_box.set_margin_start(10);
        nexus_box.set_margin_end(10);
        nexus_box.set_margin_top(10);

        let nexus_list = ListBox::new();
        nexus_list.add_css_class("shard-list");
        nexus_list.set_selection_mode(gtk4::SelectionMode::None);

        // Header: Primes
        nexus_list.append(&Label::builder().label("PRIMES").xalign(0.0).css_classes(vec!["nexus-header"]).build());

        // Una Prime
        let row_una = Box::new(Orientation::Horizontal, 10);
        let icon_una = Image::from_icon_name("computer-symbolic");
        let label_una = Label::new(Some("Una-Prime"));
        let spinner_una = Spinner::new();
        row_una.append(&icon_una);
        row_una.append(&label_una);
        row_una.append(&spinner_una);
        nexus_list.append(&row_una);

        // Claude Prime (Placeholder)
        let row_claude = Box::new(Orientation::Horizontal, 10);
        let icon_claude = Image::from_icon_name("avatar-default-symbolic");
        let label_claude = Label::new(Some("Claude-Prime"));
        let spinner_claude = Spinner::new(); // Static for now
        row_claude.append(&icon_claude);
        row_claude.append(&label_claude);
        row_claude.append(&spinner_claude);
        nexus_list.append(&row_claude);

        // Header: Sub-processes
        nexus_list.append(&Label::builder().label("SUB-PROCESSES").xalign(0.0).css_classes(vec!["nexus-header"]).build());

        // S9-Mule
        let row_s9 = Box::new(Orientation::Horizontal, 10);
        row_s9.set_margin_start(15); // Indent
        let icon_s9 = Image::from_icon_name("network-server-symbolic");
        let label_s9 = Label::new(Some("S9-Mule"));
        let spinner_s9 = Spinner::new();
        row_s9.append(&icon_s9);
        row_s9.append(&label_s9);
        row_s9.append(&spinner_s9);
        nexus_list.append(&row_s9);

        nexus_box.append(&nexus_list);
        sidebar_stack.add_titled(&nexus_box, Some("nexus"), "Nexus"); // Directive 063: Native casing

        sidebar_box.append(&sidebar_stack);

        let stack_switcher = StackSwitcher::builder().stack(&sidebar_stack).halign(Align::Center).build();
        sidebar_box.append(&stack_switcher);
        sidebar_box.append(&Separator::new(Orientation::Horizontal)); // Footer separator

        left_vbox.append(&sidebar_box);
        main_h_paned.set_start_child(Some(&left_vbox));

        // --- Right Pane (The Command Center) ---
        let right_vbox = Box::new(Orientation::Vertical, 0);
        right_vbox.set_hexpand(true); // Directive 060: Kinematic Enforcement

        // Directive 063: GTK4 Window Padding (Non-Adwaita only)
        #[cfg(not(feature = "gnome"))]
        {
            right_vbox.set_margin_start(8);
            right_vbox.set_margin_end(8);
            right_vbox.set_margin_bottom(8);
        }

        // Command HeaderBar
        #[cfg(feature = "gnome")]
        let command_header_bar = {
            let hb = adw::HeaderBar::new();
            hb.set_show_start_title_buttons(true);
            hb.set_show_end_title_buttons(true);
            hb
        };

        #[cfg(not(feature = "gnome"))]
        let command_header_bar = {
            let hb = HeaderBar::new();
            hb.set_show_title_buttons(true);
            hb
        };

        // Explicit Title for Right Header (Directive 059)
        command_header_bar.set_title_widget(Some(&Label::new(Some("Lumen"))));
        command_header_bar.add_css_class("titlebar"); // Directive 063: GTK Titlebar Fix

        // Grouping: Toggle + Telemetry
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

        command_header_bar.pack_start(&status_group);
        right_vbox.append(&command_header_bar);

        // --- Main Content (Console/Input) ---
        // The Root Vertical Split: Content (Top) / Input (Bottom)
        let paned = Paned::new(Orientation::Vertical);
        paned.set_vexpand(true);
        paned.set_hexpand(true);
        paned.set_position(550);

        // Console
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

        // --- The Spatial Cortex (Restored) ---
        // GL Area setup
        let gl_area = GLArea::new();
        gl_area.set_has_depth_buffer(true);
        // Request Core Profile 3.3
        gl_area.set_required_version(3, 3);
        gl_area.set_size_request(300, 200); // Give it some height
        gl_area.set_hexpand(true);
        gl_area.set_vexpand(true);

        let renderer = Rc::new(RefCell::new(Renderer::new()));

        // 1. THE RENDERER CLONE (Omega Fix)
        let renderer_draw = renderer.clone();
        let renderer_realize = renderer.clone();

        gl_area.connect_realize(move |area| {
            eprintln!(":: VUG :: connect_realize callback FIRED");
            area.make_current();

            // Safety check: If GTK failed to allocate the GPU context, abort gracefully
            if let Some(err) = area.error() {
                eprintln!(":: VUG :: Fatal GL Error on Realize: {:?}", err);
                return;
            }

            eprintln!(":: VUG :: Calling vug::Renderer::load_gl_functions");
            vug::renderer::Renderer::load_gl_functions();
            eprintln!(":: VUG :: Calling init_gl");
            renderer_realize.borrow_mut().init_gl();
        });

        gl_area.connect_render(move |area, ctx| renderer_draw.borrow_mut().draw(area, ctx));

        // --- DIRECTIVE 059 - OVERRIDE: THE GL_AREA QUARANTINE ---
        // Do not attach the GL Area to the UI hierarchy. It remains orphaned.
        // We attach the console directly to the start child of the main pane.
        paned.set_start_child(Some(&scrolled_window));

        // --- Input Area ---
        let input_container = Box::new(Orientation::Horizontal, 8);
        input_container.set_valign(Align::Fill);
        input_container.set_margin_start(16);
        input_container.set_margin_end(16);
        input_container.set_margin_bottom(16);
        input_container.set_margin_top(16);

        // Attach Button
        let attach_icon = Image::from_icon_name("share-symbolic");
        // Directive 063: Remove pixel size
        let attach_btn = Button::builder()
            .valign(Align::End)
            .css_classes(vec!["attach-action"])
            .tooltip_text("Attach File")
            .child(&attach_icon)
            .build();
        // (Attach Logic same as before, omitted for brevity but preserved in intent)
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

        // THE COMPOSER (Wolfpack Interface) - Button moved to Sidebar
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

        // Action Buttons (EXEC, ARCH, DEBUG, UNA)
        let action_box = Box::new(Orientation::Horizontal, 0);
        action_box.add_css_class("linked");
        let btn_exec = ToggleButton::with_label("EXEC");
        let btn_arch = ToggleButton::with_label("ARCH");
        let btn_debug = ToggleButton::with_label("DEBUG");
        let btn_una = ToggleButton::with_label("UNA");
        // Group them
        btn_arch.set_group(Some(&btn_exec));
        btn_debug.set_group(Some(&btn_exec));
        btn_una.set_group(Some(&btn_exec));
        btn_exec.set_active(true); // Default

        action_box.append(&btn_exec);
        action_box.append(&btn_arch);
        action_box.append(&btn_debug);
        action_box.append(&btn_una);
        pop_box.append(&action_box);

        // Subject
        let subject_entry = Entry::new();
        subject_entry.set_placeholder_text(Some("Subject"));
        pop_box.append(&subject_entry);

        // Body
        let body_buffer = TextBuffer::new(None);
        let body_view = TextView::with_buffer(&body_buffer);
        body_view.set_wrap_mode(gtk4::WrapMode::WordChar);
        body_view.set_height_request(150);
        let body_scroll = ScrolledWindow::builder()
            .child(&body_view)
            .has_frame(true)
            .vexpand(true)
            .build();
        pop_box.append(&body_scroll);

        // Point Break
        let pb_check = CheckButton::with_label("Point Break");
        pop_box.append(&pb_check);

        // Send Button
        let btn_comp_send = Button::with_label("Transmit Order");
        btn_comp_send.add_css_class("suggested-action");
        let pop_weak = popover_composer.downgrade();

        let sub_ent = subject_entry.clone();
        let bod_buf = body_buffer.clone();
        let pb_chk = pb_check.clone();
        let b_ex = btn_exec.clone();
        let b_ar = btn_arch.clone();
        let b_db = btn_debug.clone();
        let _b_un = btn_una.clone(); // Omega: _b_un

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
            // Auto-fill subject with Active Directive
            sub_ent_pop.set_text(&ad_ref.borrow());
            popover_composer.set_parent(btn);
            popover_composer.popup();
        });

        // Chat Input (Baseline)
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
        // Directive 063: Remove pixel size
        let send_btn = Button::builder()
            .valign(Align::End)
            .css_classes(vec!["suggested-action"])
            .tooltip_text("Send Message (Ctrl+Enter)")
            .child(&send_icon)
            .build();

        // (Key Handling same as before)
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
        paned.set_end_child(Some(&input_container));

        right_vbox.append(&paned);
        main_h_paned.set_end_child(Some(&right_vbox));

        // Toggle logic for the new layout:
        // Hiding sidebar_box works, but left_vbox still occupies 215px.
        // We should collapse the main_h_paned position or hide the left pane.
        // However, standard toggle behavior often just hides content or collapses.
        // Given "Full Height Split", if we toggle off, we likely want the sidebar GONE.
        // So let's hide the left_vbox entirely.
        let left_vbox_clone = left_vbox.clone();
        sidebar_toggle.connect_toggled(move |btn| {
            left_vbox_clone.set_visible(btn.is_active());
        });

        // Clones for async loop
        let label_una_clone = label_una.clone();
        let spinner_una_clone = spinner_una.clone();
        let label_s9_clone = label_s9.clone();
        let spinner_s9_clone = spinner_s9.clone();
        let gl_area_clone = gl_area.clone();
        let renderer_clone = renderer.clone();
        let token_label_clone = token_label.clone();
        let pulse_icon_clone = pulse_icon.clone();
        let active_directive_async = active_directive_clone.clone();

        glib::MainContext::default().spawn_local(async move {
            while let Ok(update) = rx.recv().await {
                match update {
                    GuiUpdate::Spectrum(data) => {
                        renderer_clone.borrow_mut().update_spectrum(data);
                        gl_area_clone.queue_render();
                    }
                    GuiUpdate::ConsoleLog(text) => {
                        let mut end_iter = text_buffer_clone.end_iter();
                        text_buffer_clone.insert(&mut end_iter, &text);
                        let adj = scroll_adj_clone.clone();
                        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
                            if adj.upper() > adj.page_size() {
                                let val = adj.upper() - adj.page_size();
                                adj.set_value(val);
                            }
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
                                 pulse_icon_clone.set_opacity(1.0);
                             },
                             _ => {
                                 pulse_icon_clone.remove_css_class("pulse-active");
                                 pulse_icon_clone.set_opacity(0.3);
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
            if let Some(app_win) = window.dynamic_cast_ref::<gtk4::ApplicationWindow>() {
                // Directive 061: Destroy the Global Titlebar
                app_win.set_titlebar(Some(&gtk4::Box::new(gtk4::Orientation::Horizontal, 0)));
            }
            main_h_paned.upcast::<Widget>()
        }

        #[cfg(not(feature = "gnome"))]
        {
            if let Some(app_win) = window.dynamic_cast_ref::<gtk4::ApplicationWindow>() {
                // Remove any existing titlebar from window if possible, or set None.
                // app_win.set_titlebar(None::<&Widget>);
                // But set_titlebar expects Some.
                // We'll just set child. The window manager decorations depend on the platform.
                // If we want CSD, the HeaderBars inside the panes handle it.
                app_win.set_titlebar(None::<&Widget>);
            }
            main_h_paned.into()
        }
    }
}
