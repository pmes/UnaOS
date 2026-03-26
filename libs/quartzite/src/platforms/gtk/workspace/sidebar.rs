// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use async_channel::Sender;
use gtk4::prelude::*;
use gtk4::{
    Adjustment, Align, Box, Button, ColumnView, ColumnViewColumn, DropDown,
    Image, Label, ListBox, ListItem, ListView, Orientation, PolicyType, Scale, ScrolledWindow,
    SignalListItemFactory, SingleSelection, Spinner, Stack, StackSwitcher, StackTransitionType,
    StringList, StringObject, Switch, ToggleButton, Window, gio,
};
use sourceview5::View as SourceView;
use sourceview5::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::Event;
use bandy::state::{WorkspaceState, ViewEntity};

use crate::NativeWindow;

pub struct SidebarWidgets {
    pub left_stack: Stack,
    pub status_group: Box,
    pub left_switcher: StackSwitcher,
    pub composer_btn: Button,
    pub network_btn: Button,
}

pub struct SidebarPointers {
    pub active_target: Rc<RefCell<String>>,
    pub spinner_una: Spinner,
    pub label_una: Label,
    pub spinner_s9: Spinner,
    pub label_s9: Label,
    pub token_label: Label,
    pub context_view: crate::widgets::telemetry::ContextView,
    pub matrix_store: gio::ListStore,
    pub matrix_selection: gtk4::MultiSelection,
}

// Helper to avoid circular dependencies in spline
fn enable_spelling(view: &SourceView) {
    if let Some(buffer) = view.buffer().downcast::<sourceview5::Buffer>().ok() {
        let provider = libspelling::Provider::default();
        let checker = libspelling::Checker::new(Some(&provider), Some("en_US"));
        let adapter = libspelling::TextBufferAdapter::new(&buffer, &checker);

        adapter.set_language("en_US");
        adapter.set_enabled(true);
        view.insert_action_group("spelling", Some(&adapter));
        let menu = adapter.menu_model();
        view.set_extra_menu(Some(&menu));

        struct SendWrapper<T>(pub T);
        unsafe impl<T> Send for SendWrapper<T> {}
        unsafe impl<T> Sync for SendWrapper<T> {}

        unsafe {
            buffer.set_data("spell-adapter", SendWrapper(adapter));
        }
    }
}

pub fn build(window: &NativeWindow, tx_event: Sender<Event>, _workspace_tetra: &crate::tetra::WorkspaceTetra, workspace_state: &WorkspaceState) -> (SidebarWidgets, SidebarPointers) {
    // UI Controls
    let sidebar_toggle = ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .active(true)
        .tooltip_text("Toggle Sidebar")
        .build();

    let network_btn = Button::builder()
        .icon_name("network-idle-symbolic")
        .css_classes(vec!["flat", "icon-button"])
        .tooltip_text("Network Inspector")
        .valign(gtk4::Align::Center)
        .build();

    let token_label = Label::new(Some("Tokens: IN: 0 | OUT: 0 | TOTAL: 0"));
    token_label.set_margin_start(10);
    token_label.set_margin_end(10);
    token_label.set_wrap(true);
    token_label.set_justify(gtk4::Justification::Center);

    let status_group = Box::new(Orientation::Horizontal, 8);
    status_group.set_valign(gtk4::Align::Center);
    status_group.append(&sidebar_toggle);
    status_group.append(&network_btn);
    status_group.append(&token_label);

    let left_stack = Stack::new();
    left_stack.set_vexpand(true);
    left_stack.set_transition_type(StackTransitionType::SlideLeftRight);

    let left_switcher = StackSwitcher::new();
    left_switcher.set_stack(Some(&left_stack));
    left_switcher.set_halign(Align::Center);

    let left_stack_clone = left_stack.clone();
    sidebar_toggle.connect_toggled(move |btn| {
        left_stack_clone.set_visible(btn.is_active());
    });

    // 1. Nodes Tab
    let store = gio::ListStore::new::<StringObject>();
    for item in ["Prime", "Encrypted", "Jules (Private)"].iter() {
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
    column_view.set_vexpand(true);
    let tx_clone_nav = tx_event.clone();
    column_view
        .model()
        .unwrap()
        .connect_selection_changed(move |model, _, _| {
            let selection = model.downcast_ref::<SingleSelection>().unwrap();
            if let Some(_) = selection.selected_item() {
                let idx = selection.selected() as usize;
                let _ = tx_clone_nav.send_blocking(Event::NavSelect(idx));
            }
        });
    let nodes_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Automatic)
        .child(&column_view)
        .min_content_height(200)
        .min_content_width(200)
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

        vbox.append(&Label::new(Some("Temperature (0.0 - 1.0)")));
        let adj = Adjustment::new(0.7, 0.0, 1.0, 0.1, 0.1, 0.0);
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
        prompt_view.set_vexpand(true);
        let scroll = ScrolledWindow::builder()
            .child(&prompt_view)
            .vexpand(true)
            .min_content_height(150)
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
                let model_obj = dropdown
                    .selected_item()
                    .and_then(|obj| obj.downcast::<StringObject>().ok());
                let model = model_obj
                    .map(|s| s.string().to_string())
                    .unwrap_or_default();
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

    let composer_icon = Image::from_icon_name("chat-message-new-symbolic");
    let composer_btn = Button::builder()
        .css_classes(vec!["flat"])
        .tooltip_text("Open Composer (Formal Command)")
        .child(&composer_icon)
        .build();

    node_actions_box.append(&composer_btn);
    nodes_box.append(&node_actions_box);
    let page = left_stack.add_named(&nodes_box, Some("nodes"));
    page.set_icon_name("system-users-symbolic");

    // 2. THE NEXUS Tab
    let nexus_box = Box::new(Orientation::Vertical, 0);
    nexus_box.set_margin_start(10);
    nexus_box.set_margin_end(10);
    nexus_box.set_margin_top(10);

    let nexus_list = ListBox::new();
    nexus_list.add_css_class("shard-list");
    nexus_list.set_selection_mode(gtk4::SelectionMode::Single);

    let active_target = Rc::new(RefCell::new("Una-Prime".to_string()));
    let active_target_clone = active_target.clone();

    nexus_list.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            if let Some(child) = row.child() {
                if let Some(box_widget) = child.downcast_ref::<Box>() {
                    if let Some(label_widget) = box_widget
                        .last_child()
                        .and_then(|w| w.prev_sibling())
                        .and_then(|w| w.downcast::<Label>().ok())
                    {
                        let text = label_widget.text().to_string();
                        let name = text.split(" (").next().unwrap_or(&text).to_string();
                        *active_target_clone.borrow_mut() = name;
                    }
                }
            }
        }
    });

    nexus_list.append(
        &Label::builder()
            .label("PRIMES")
            .xalign(0.0)
            .css_classes(vec!["nexus-header"])
            .build(),
    );

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

    nexus_list.append(
        &Label::builder()
            .label("SUB-PROCESSES")
            .xalign(0.0)
            .css_classes(vec!["nexus-header"])
            .build(),
    );

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
    let page = left_stack.add_named(&nexus_box, Some("nexus"));
    page.set_icon_name("network-workgroup-symbolic");

    if let Some(row) = nexus_list.row_at_index(1) {
        nexus_list.select_row(Some(&row));
    }

    // 3. THE TeleHUD Tab
    let telehud_box = Box::new(Orientation::Vertical, 12);
    telehud_box.set_margin_start(12);
    telehud_box.set_margin_end(12);

    // --- MATRIX TETRA ---
    let matrix_store = gio::ListStore::new::<crate::widgets::model::MatrixNodeObject>();
    if let ViewEntity::Topology(topology_state) = &workspace_state.left_pane {
        let flat_nodes = topology_state.tree.flatten();
        for (node, depth) in flat_nodes {
            let obj = crate::widgets::model::MatrixNodeObject::new(&node.id, &node.label, depth as u32);
            matrix_store.append(&obj);
        }
    }

    let matrix_selection = gtk4::MultiSelection::new(Some(matrix_store.clone()));
    let matrix_factory = SignalListItemFactory::new();

    matrix_factory.connect_setup(move |_factory, item| {
        let item = item.downcast_ref::<ListItem>().unwrap();

        // Enable visual selection highlight (Fixes GTK Ghost Outline)
        item.set_selectable(true);

        let row = Box::new(Orientation::Horizontal, 10);
        row.set_margin_start(10);
        row.set_margin_end(10);
        row.set_margin_top(5);
        row.set_margin_bottom(5);
        let label = Label::new(None);
        label.set_xalign(0.0);
        row.append(&label);
        item.set_child(Some(&row));
    });

    matrix_factory.connect_bind(move |_factory, item| {
        let item = item.downcast_ref::<ListItem>().unwrap();
        let child = item.child().unwrap().downcast::<Box>().unwrap();
        let label = child.first_child().unwrap().downcast::<Label>().unwrap();
        let obj = item.item().unwrap().downcast::<crate::widgets::model::MatrixNodeObject>().unwrap();

        let depth = obj.depth() as i32;
        label.set_margin_start(10 + (depth * 20));
        label.add_css_class("monospace");
        label.set_label(&obj.label());
    });

    // A standard ListView natively has no headers, permanently destroying the void.
    let matrix_view = ListView::new(Some(matrix_selection.clone()), Some(matrix_factory));
    matrix_view.set_enable_rubberband(true);
    matrix_view.set_single_click_activate(false);

    let tx_matrix_sel = tx_event.clone();
    matrix_selection.connect_selection_changed(move |selection, _, _| {
        let mut selected_ids = Vec::new();
        for i in 0..selection.n_items() {
            if selection.is_selected(i) {
                if let Some(item) = selection.item(i) {
                    if let Ok(obj) = item.downcast::<crate::widgets::model::MatrixNodeObject>() {
                        selected_ids.push(obj.id());
                    }
                }
            }
        }
        let _ = tx_matrix_sel.send_blocking(Event::UpdateMatrixSelection(selected_ids));
    });

    let nav_history_back = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let nav_history_forward = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let current_matrix_path = std::rc::Rc::new(std::cell::RefCell::new(String::new()));

    let nav_box = Box::new(Orientation::Horizontal, 5);
    nav_box.set_margin_start(5);
    nav_box.set_margin_end(5);

    let btn_back = Button::from_icon_name("go-previous-symbolic");
    let btn_up = Button::from_icon_name("go-up-symbolic");
    let btn_forward = Button::from_icon_name("go-next-symbolic");

    btn_back.add_css_class("flat");
    btn_up.add_css_class("flat");
    btn_forward.add_css_class("flat");

    // Progressive Disclosure: Spawn buttons in a deactivated state.
    // They should only illuminate when spatial navigation actually permits their use.
    btn_back.set_sensitive(false);
    btn_up.set_sensitive(false);
    btn_forward.set_sensitive(false);

    // Shared update function to recalibrate button sensitivities based on local history state
    let update_nav_btns = {
        let btn_back = btn_back.clone();
        let btn_up = btn_up.clone();
        let btn_forward = btn_forward.clone();
        let b_stack = nav_history_back.clone();
        let f_stack = nav_history_forward.clone();
        let c_path = current_matrix_path.clone();

        move || {
            btn_back.set_sensitive(!b_stack.borrow().is_empty());
            btn_forward.set_sensitive(!f_stack.borrow().is_empty());
            let current = c_path.borrow().clone();
            btn_up.set_sensitive(!current.is_empty() && current.contains('/'));
        }
    };

    let tx_matrix_nav = tx_event.clone();
    let nav_back_clone = nav_history_back.clone();
    let nav_forward_clone = nav_history_forward.clone();
    let current_path_clone = current_matrix_path.clone();
    let update_btns_activate = update_nav_btns.clone();

    matrix_view.connect_activate(move |view, pos| {
        let model = view.model().unwrap().downcast::<gtk4::MultiSelection>().unwrap();
        if let Some(item) = model.item(pos) {
            let obj = item.downcast::<crate::widgets::model::MatrixNodeObject>().unwrap();
            let new_id = obj.id();

            // Only push to history if the path actually changed
            let current = current_path_clone.borrow().clone();
            if current != new_id && !current.is_empty() {
                nav_back_clone.borrow_mut().push(current);
                nav_forward_clone.borrow_mut().clear();
            }
            *current_path_clone.borrow_mut() = new_id.clone();

            // Recalibrate navigation controls
            update_btns_activate();

            // Checkpoint Delta: The Interactive Trigger
            // Trigger semantic extraction for the selected matrix node.
            let _ = tx_matrix_nav.send_blocking(Event::ToggleMatrixNode(new_id));
        }
    });

    let back_b = nav_history_back.clone();
    let back_f = nav_history_forward.clone();
    let back_c = current_matrix_path.clone();
    let update_btns_back = update_nav_btns.clone();

    btn_back.connect_clicked(move |_| {
        let prev_opt = back_b.borrow_mut().pop();
        if let Some(prev) = prev_opt {
            let current = back_c.borrow().clone();
            if !current.is_empty() {
                back_f.borrow_mut().push(current);
            }
            *back_c.borrow_mut() = prev.clone();
            update_btns_back();
            // let _ = tx_back.send_blocking(Event::FocusMatrixSector(prev));
        }
    });

    let fwd_b = nav_history_back.clone();
    let fwd_f = nav_history_forward.clone();
    let fwd_c = current_matrix_path.clone();
    let update_btns_fwd = update_nav_btns.clone();

    btn_forward.connect_clicked(move |_| {
        let next_opt = fwd_f.borrow_mut().pop();
        if let Some(next) = next_opt {
            let current = fwd_c.borrow().clone();
            if !current.is_empty() {
                fwd_b.borrow_mut().push(current);
            }
            *fwd_c.borrow_mut() = next.clone();
            update_btns_fwd();
            // let _ = tx_fwd.send_blocking(Event::FocusMatrixSector(next));
        }
    });

    let up_b = nav_history_back.clone();
    let up_f = nav_history_forward.clone();
    let up_c = current_matrix_path.clone();
    let update_btns_up = update_nav_btns.clone();

    btn_up.connect_clicked(move |_| {
        let current = up_c.borrow().clone();
        if !current.is_empty() && current.contains('/') {
            // Split by '/' and remove the last segment to get the parent directory
            let mut parts: Vec<&str> = current.split('/').collect();
            parts.pop();
            let parent = parts.join("/");

            up_b.borrow_mut().push(current);
            up_f.borrow_mut().clear();
            *up_c.borrow_mut() = parent.clone();
            update_btns_up();
            // let _ = tx_up.send_blocking(Event::FocusMatrixSector(parent));
        }
    });

    nav_box.append(&btn_back);
    nav_box.append(&btn_up);
    nav_box.append(&btn_forward);

    telehud_box.append(&nav_box);

    let matrix_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Automatic)
        .child(&matrix_view)
        .min_content_height(150)
        .vexpand(true)
        .hexpand(true)
        .build();

    telehud_box.append(&matrix_scroll);
    // --- END MATRIX TETRA ---

    let context_view = crate::widgets::telemetry::ContextView::new();
    telehud_box.append(&context_view.container);

    let page = left_stack.add_named(&telehud_box, Some("telehud"));
    page.set_icon_name("error-correct-symbolic");

    let _ = tx_event.try_send(crate::Event::UiReady);

    let widgets = SidebarWidgets {
        left_stack,
        status_group,
        left_switcher,
        composer_btn,
        network_btn,
    };

    let pointers = SidebarPointers {
        active_target,
        spinner_una,
        label_una,
        spinner_s9,
        label_s9,
        token_label,
        context_view,
        matrix_store,
        matrix_selection,
    };

    (widgets, pointers)
}
