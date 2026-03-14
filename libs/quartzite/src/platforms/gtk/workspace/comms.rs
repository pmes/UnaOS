// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use async_channel::Sender;
use gtk4::prelude::*;
use gtk4::{
    Align, Box, Button, CheckButton, Entry, EventControllerKey, Expander, FileDialog,
    GestureClick, Label, ListItem, ListView, NoSelection, Orientation,
    PolicyType, Popover, PropagationPhase, ScrolledWindow, SignalListItemFactory, Stack,
    StackSwitcher, StackTransitionType, ToggleButton, Overlay,
    gdk::{Key, ModifierType},
    gio, glib,
};
use sourceview5::View as SourceView;
use sourceview5::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::Event;
use crate::NativeWindow;
use crate::widgets::model::HistoryObject;

pub struct CommsWidgets {
    pub workspace_stack: Stack,
    pub right_switcher: StackSwitcher,
}

pub struct CommsPointers {
    pub console_store: gio::ListStore,
    pub active_directive: Rc<RefCell<String>>,
    pub is_prepending: Rc<RefCell<bool>>,
    pub is_fetching: Rc<RefCell<bool>>,
    pub history_sync_cursor: Rc<RefCell<usize>>,
    pub preflight_overlay: Overlay,
    pub preflight_stack_container: Box,
    pub preflight_stack: Stack,
    pub preflight_sys_buf: sourceview5::Buffer,
    pub preflight_dir_buf: sourceview5::Buffer,
    pub preflight_eng_buf: sourceview5::Buffer,
    pub preflight_prm_buf: sourceview5::Buffer,
}

// Helper to avoid circular dependencies
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

pub fn build(
    window: &NativeWindow,
    tx_event: Sender<Event>,
    active_target: Rc<RefCell<String>>,
    composer_btn: Button,
) -> (CommsWidgets, CommsPointers) {
    let workspace_stack = Stack::new();
    workspace_stack.set_vexpand(true);
    workspace_stack.set_transition_type(StackTransitionType::SlideLeftRight);

    let comms_page = Box::new(Orientation::Vertical, 0);
    comms_page.set_hexpand(true);
    comms_page.set_vexpand(true);

    let chat_overlay = Overlay::new();
    chat_overlay.set_hexpand(true);
    chat_overlay.set_vexpand(true);

    let scrolled_window = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build();

    chat_overlay.set_child(Some(&scrolled_window));

    let adj = scrolled_window.vadjustment();
    let was_at_bottom = Rc::new(RefCell::new(true));
    let was_at_top = Rc::new(RefCell::new(true));
    let last_upper = Rc::new(RefCell::new(0.0));
    let is_prepending = Rc::new(RefCell::new(false));
    let is_fetching = Rc::new(RefCell::new(false));

    let tx_clone_hist = tx_event.clone();
    let was_at_bottom_val = was_at_bottom.clone();
    let was_at_top_val = was_at_top.clone();
    let is_prepending_val = is_prepending.clone();
    let is_fetching_val = is_fetching.clone();

    adj.connect_value_notify(move |a| {
        let upper = a.upper();
        let page_size = a.page_size();
        if upper <= page_size || upper == 0.0 {
            return;
        }

        let val = a.value();
        let lower = a.lower();

        *was_at_bottom_val.borrow_mut() = (val - (upper - page_size)).abs() < 10.0;

        let is_at_top = val <= lower + 10.0;
        let previously_at_top = *was_at_top_val.borrow();
        *was_at_top_val.borrow_mut() = is_at_top;

        if is_at_top && !previously_at_top && upper > page_size {
            if !*is_fetching_val.borrow() {
                *is_fetching_val.borrow_mut() = true;
                *is_prepending_val.borrow_mut() = true;
                let tx_for_async = tx_clone_hist.clone();
                glib::MainContext::default().spawn_local(async move {
                    let _ = tx_for_async.send(Event::LoadHistory).await;
                });
            }
        }
    });

    let was_at_bottom_upper = was_at_bottom.clone();
    let is_prepending_upper = is_prepending.clone();
    let last_upper_ref = last_upper.clone();

    adj.connect_upper_notify(move |a| {
        let upper = a.upper();
        let page_size = a.page_size();
        if upper <= page_size || upper == 0.0 {
            return;
        }

        let old_upper = *last_upper_ref.borrow();
        let delta = upper - old_upper;
        *last_upper_ref.borrow_mut() = upper;

        if *was_at_bottom_upper.borrow() {
            a.set_value(upper - page_size);
        } else if *is_prepending_upper.borrow() && delta > 0.0 {
            a.set_value(a.value() + delta);
            *is_prepending_upper.borrow_mut() = false;
        }
    });

    let console_store = gio::ListStore::new::<HistoryObject>();
    // REMOVED FilterListModel per Architect instructions
    let console_selection = NoSelection::new(Some(console_store.clone()));

    // Create a Rust struct to hold the precise pointers without DOM traversal
    #[derive(Clone)]
    struct BubbleWidgets {
        left_spacer: Box,
        bubble: Box,
        right_spacer: Box,
        left_expand_btn: Button,
        meta_label: Label,
        right_expand_btn: Button,
        chat_content_view: SourceView,
        expander: Expander,
    }

    let console_factory = SignalListItemFactory::new();
    console_factory.connect_setup(move |_factory, item| {
        let item = item.downcast_ref::<ListItem>().unwrap();
        let root = Box::new(Orientation::Horizontal, 0);
        root.set_hexpand(true);
        root.add_css_class("console-row");

        let left_spacer = Box::new(Orientation::Horizontal, 0);
        left_spacer.set_hexpand(true);
        root.append(&left_spacer);

        let bubble = Box::new(Orientation::Vertical, 4);
        bubble.add_css_class("bubble-box");
        bubble.set_hexpand(true);

        let header_box = Box::new(Orientation::Horizontal, 8);

        let left_expand_btn = Button::builder()
            .icon_name("pan-down-symbolic")
            .css_classes(vec!["flat"])
            .build();
        left_expand_btn.set_visible(false);

        let meta_label = Label::new(None);
        meta_label.set_xalign(0.0);
        meta_label.add_css_class("dim-label");
        meta_label.set_hexpand(true);

        let right_expand_btn = Button::builder()
            .icon_name("pan-down-symbolic")
            .css_classes(vec!["flat"])
            .build();
        right_expand_btn.set_visible(false);

        header_box.append(&left_expand_btn);
        header_box.append(&meta_label);
        header_box.append(&right_expand_btn);

        bubble.append(&header_box);

        // --- Standard Mode (Message View) ---
        let chat_content_buffer = sourceview5::Buffer::new(None);
        let chat_content_view = SourceView::with_buffer(&chat_content_buffer);
        chat_content_view.set_editable(false);
        chat_content_view.set_wrap_mode(gtk4::WrapMode::WordChar);
        chat_content_view.set_show_line_numbers(false);
        chat_content_view.set_monospace(true);
        chat_content_view.set_hexpand(true);
        chat_content_view.set_focusable(true);
        chat_content_view.set_cursor_visible(false);
        chat_content_view.add_css_class("view");

        bubble.append(&chat_content_view);

        // --- Standard Mode (Expander) ---
        let expander = Expander::new(None);
        let expander_label = Label::new(None);
        expander.set_child(Some(&expander_label));
        let payload_content_buffer = sourceview5::Buffer::new(None);
        let payload_content_view = SourceView::with_buffer(&payload_content_buffer);
        payload_content_view.set_editable(false);
        payload_content_view.set_wrap_mode(gtk4::WrapMode::WordChar);
        payload_content_view.set_show_line_numbers(true);
        payload_content_view.set_monospace(true);
        payload_content_view.set_cursor_visible(false);
        payload_content_view.add_css_class("view");
        let payload_scroll = ScrolledWindow::builder()
            .child(&payload_content_view)
            .max_content_height(300)
            .build();
        expander.set_child(Some(&payload_scroll));
        bubble.append(&expander);

        root.append(&bubble);

        let right_spacer = Box::new(Orientation::Horizontal, 0);
        right_spacer.set_hexpand(true);
        root.append(&right_spacer);

        // Pack pointers securely into a struct and attach to the list item
        let widgets = BubbleWidgets {
            left_spacer,
            bubble: bubble.clone(),
            right_spacer,
            left_expand_btn: left_expand_btn.clone(),
            meta_label,
            right_expand_btn: right_expand_btn.clone(),
            chat_content_view: chat_content_view.clone(),
            expander,
        };
        let boxed_widgets = glib::BoxedAnyObject::new(widgets);
        unsafe {
            item.set_data("widgets", boxed_widgets);
        }

        let gesture = GestureClick::new();
        gesture.set_propagation_phase(PropagationPhase::Target);
        let item_clone = item.clone();
        let chat_content_view_clone = chat_content_view.clone();
        let left_btn_clone = left_expand_btn.clone();
        let right_btn_clone = right_expand_btn.clone();
        gesture.connect_pressed(move |_, n_press, _, _| {
            if n_press == 1 {
                if let Some(obj) = item_clone
                    .item()
                    .and_downcast::<HistoryObject>()
                {
                    let expanded = !obj.is_expanded();
                    obj.set_is_expanded(expanded);
                    let content = obj.content();
                    let line_count = content.trim_end().lines().count();
                    if line_count > 11 && !expanded {
                        let truncated: String = content
                            .trim_end()
                            .lines()
                            .take(11)
                            .collect::<Vec<&str>>()
                            .join("\n");
                        chat_content_view_clone.buffer().set_text(&truncated);
                        left_btn_clone.set_icon_name("pan-down-symbolic");
                        right_btn_clone.set_icon_name("pan-down-symbolic");
                    } else {
                        chat_content_view_clone
                            .buffer()
                            .set_text(content.trim_end());
                        if line_count > 11 {
                            left_btn_clone.set_icon_name("pan-up-symbolic");
                            right_btn_clone.set_icon_name("pan-up-symbolic");
                        }
                    }
                }
            }
        });
        bubble.add_controller(gesture);
        item.set_child(Some(&root));
    });

    console_factory.connect_bind(move |_factory, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else { return; };
        let Some(obj) = item.item().and_then(|c| c.downcast::<HistoryObject>().ok()) else { return; };

        // Retrieve preserved absolute pointers securely
        let boxed_widgets = unsafe { item.data::<glib::BoxedAnyObject>("widgets") };
        let Some(boxed_ptr) = boxed_widgets else { return; };
        let widgets = unsafe { boxed_ptr.as_ref() }.borrow::<BubbleWidgets>();

        widgets.bubble.remove_css_class("architect-bubble");
        widgets.bubble.remove_css_class("una-bubble");
        widgets.left_spacer.set_visible(false);
        widgets.right_spacer.set_visible(false);

        widgets.chat_content_view.set_visible(false);
        widgets.expander.set_visible(false);
        widgets.left_expand_btn.set_visible(false);
        widgets.right_expand_btn.set_visible(false);

        // STANDARD MODE
        let is_chat = obj.is_chat();
        let sender = obj.sender();
        let timestamp = obj.timestamp();
        let content = obj.content();
        let subject = obj.subject();

        if is_chat {
            widgets.chat_content_view.set_visible(true);
            widgets.meta_label.set_text(&format!("{} • {}", sender, timestamp));
            widgets.meta_label.remove_css_class("role-architect");
            widgets.meta_label.remove_css_class("role-una");
            widgets.meta_label.remove_css_class("role-system");

            let is_expanded = obj.is_expanded();
            let line_count = content.trim_end().lines().count();

            if sender == "Architect" {
                widgets.meta_label.add_css_class("role-architect");
                widgets.bubble.add_css_class("architect-bubble");
                widgets.left_spacer.set_visible(true);
                widgets.right_spacer.set_visible(false);
                widgets.meta_label.set_halign(gtk4::Align::End);
                widgets.meta_label.set_xalign(1.0);
                if line_count > 11 {
                    widgets.left_expand_btn.set_visible(true);
                    widgets.right_expand_btn.set_visible(false);
                    widgets.left_expand_btn.set_icon_name(if is_expanded { "pan-up-symbolic" } else { "pan-down-symbolic" });
                } else {
                    widgets.left_expand_btn.set_visible(false);
                    widgets.right_expand_btn.set_visible(false);
                }
            } else {
                if sender == "Una-Prime" {
                    widgets.meta_label.add_css_class("role-una");
                } else {
                    widgets.meta_label.add_css_class("role-system");
                }
                widgets.bubble.add_css_class("una-bubble");
                widgets.left_spacer.set_visible(false);
                widgets.right_spacer.set_visible(true);
                widgets.meta_label.set_halign(gtk4::Align::Start);
                widgets.meta_label.set_xalign(0.0);
                if line_count > 11 {
                    widgets.left_expand_btn.set_visible(false);
                    widgets.right_expand_btn.set_visible(true);
                    widgets.right_expand_btn.set_icon_name(if is_expanded { "pan-up-symbolic" } else { "pan-down-symbolic" });
                } else {
                    widgets.left_expand_btn.set_visible(false);
                    widgets.right_expand_btn.set_visible(false);
                }
            }
            if line_count > 11 && !is_expanded {
                let truncated: String = content.trim_end().lines().take(11).collect::<Vec<&str>>().join("\n");
                widgets.chat_content_view.buffer().set_text(&truncated);
            } else {
                widgets.chat_content_view.buffer().set_text(content.trim_end());
            }
        } else {
            widgets.expander.set_visible(true);
            widgets.bubble.add_css_class("una-bubble");
            widgets.left_spacer.set_visible(false);
            widgets.right_spacer.set_visible(true);
            widgets.expander.set_label(Some(&format!("{} | {} | {}", sender, subject, timestamp)));
            // Direct access: No .child() traversal needed, we captured payload_content_view inside the BubbleWidgets if we want it, but wait, we didn't add payload_content_view to BubbleWidgets. I'll just use the traversal here since it's structurally fixed.
            if let Some(scroll) = widgets.expander.child().and_then(|c: gtk4::Widget| c.downcast::<ScrolledWindow>().ok()) {
                if let Some(content_view) = scroll.child().and_then(|c: gtk4::Widget| c.downcast::<SourceView>().ok()) {
                    content_view.buffer().downcast::<sourceview5::Buffer>().unwrap().set_text(&content);
                }
            }
            widgets.expander.set_expanded(false);
        }
    });

    let console_list_view = ListView::new(Some(console_selection), Some(console_factory));
    console_list_view.add_css_class("console");
    console_list_view.set_vexpand(true);
    console_list_view.set_hexpand(true);
    scrolled_window.set_child(Some(&console_list_view));

    // --- PRE-FLIGHT STACK (Layer 2) ---
    let preflight_stack_container = Box::new(Orientation::Vertical, 0);
    preflight_stack_container.set_halign(gtk4::Align::Fill);
    preflight_stack_container.set_valign(gtk4::Align::Fill);
    preflight_stack_container.set_vexpand(true);
    preflight_stack_container.set_hexpand(true);
    preflight_stack_container.add_css_class("background"); // Ensure opacity over chat

    let preflight_stack = Stack::new();
    preflight_stack.set_transition_type(StackTransitionType::SlideLeftRight);
    preflight_stack.set_vexpand(true);
    preflight_stack.set_hexpand(true);

    let create_preflight_tab = |_title: &str| -> (Box, sourceview5::Buffer) {
        let vbox = Box::new(Orientation::Vertical, 4);
        vbox.set_vexpand(true);
        vbox.set_hexpand(true);

        let buffer = sourceview5::Buffer::new(None);
        let view = SourceView::with_buffer(&buffer);
        view.set_wrap_mode(gtk4::WrapMode::WordChar);
        view.set_editable(true);
        view.set_monospace(true);
        view.set_vexpand(true);
        view.add_css_class("view");

        let scroll = ScrolledWindow::builder()
            .child(&view)
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .build();

        vbox.append(&scroll);
        (vbox, buffer)
    };

    let (box_sys, preflight_sys_buf) = create_preflight_tab("SYSTEM");
    let (box_dir, preflight_dir_buf) = create_preflight_tab("DIRECTIVES");
    let (box_eng, preflight_eng_buf) = create_preflight_tab("ENGRAMS");
    let (box_prm, preflight_prm_buf) = create_preflight_tab("PROMPT");

    preflight_stack.add_titled(&box_sys, Some("sys"), "System");
    preflight_stack.add_titled(&box_dir, Some("dir"), "Directives");
    preflight_stack.add_titled(&box_eng, Some("eng"), "Engrams");
    preflight_stack.add_titled(&box_prm, Some("prm"), "Prompt");

    let preflight_switcher = StackSwitcher::new();
    preflight_switcher.set_stack(Some(&preflight_stack));
    preflight_switcher.set_halign(Align::Center);
    preflight_switcher.set_margin_top(8);
    preflight_switcher.set_margin_bottom(8);

    preflight_stack_container.append(&preflight_switcher);
    preflight_stack_container.append(&preflight_stack);

    let dispatch_actions_box = Box::new(Orientation::Horizontal, 8);
    dispatch_actions_box.set_halign(Align::End);
    dispatch_actions_box.set_margin_top(8);
    dispatch_actions_box.set_margin_bottom(8);
    dispatch_actions_box.set_margin_end(16);

    let cancel_btn = Button::builder()
        .icon_name("window-close-symbolic")
        .tooltip_text("Discard Payload")
        .css_classes(vec!["flat", "destructive-action"])
        .build();
    let dispatch_btn = Button::builder()
        .icon_name("document-save-symbolic")
        .tooltip_text("Save and Send Payload")
        .css_classes(vec!["suggested-action"])
        .build();

    dispatch_actions_box.append(&cancel_btn);
    dispatch_actions_box.append(&dispatch_btn);
    preflight_stack_container.append(&dispatch_actions_box);

    chat_overlay.add_overlay(&preflight_stack_container);
    // Initially hide the PreFlight Stack
    preflight_stack_container.set_visible(false);

    let tx_dispatch_preflight = tx_event.clone();
    let sys_buf_clone = preflight_sys_buf.clone();
    let dir_buf_clone = preflight_dir_buf.clone();
    let eng_buf_clone = preflight_eng_buf.clone();
    let prm_buf_clone = preflight_prm_buf.clone();

    dispatch_btn.connect_clicked(move |_| {
        let (s, e) = sys_buf_clone.bounds();
        let system_text = sys_buf_clone.text(&s, &e, false).to_string();

        let (s, e) = dir_buf_clone.bounds();
        let directives_text = dir_buf_clone.text(&s, &e, false).to_string();

        let (s, e) = eng_buf_clone.bounds();
        let engrams_text = eng_buf_clone.text(&s, &e, false).to_string();

        let (s, e) = prm_buf_clone.bounds();
        let prompt_text = prm_buf_clone.text(&s, &e, false).to_string();

        let payload = bandy::state::PreFlightPayload {
            system: system_text,
            directives: directives_text,
            engrams: engrams_text,
            prompt: prompt_text,
        };
        let json = serde_json::to_string(&payload).unwrap();

        let tx_async = tx_dispatch_preflight.clone();
        glib::MainContext::default().spawn_local(async move {
            let _ = tx_async.send(Event::DispatchPayload(json)).await;
        });
    });

    let preflight_overlay_container = preflight_stack_container.clone();
    cancel_btn.connect_clicked(move |_| {
        // Here we just hide the overlay; the reactor will handle dropping it from AppState
        preflight_overlay_container.set_visible(false);
    });

    comms_page.append(&chat_overlay);

    let input_container = Box::new(Orientation::Horizontal, 8);
    input_container.set_valign(Align::Fill);
    input_container.set_margin_start(16);
    input_container.set_margin_end(16);
    input_container.set_margin_bottom(16);
    input_container.set_margin_top(16);

    let attach_btn = Button::builder()
        .valign(Align::End)
        .icon_name("share-symbolic")
        .css_classes(vec!["attach-action"])
        .tooltip_text("Attach File")
        .build();
    let tx_clone_file = tx_event.clone();
    let window_clone = window.clone();
    let target_file = active_target.clone();
    attach_btn.connect_clicked(move |_| {
        let tx = tx_clone_file.clone();
        let parent_window = window_clone.clone();
        let target = target_file.clone();
        glib::MainContext::default().spawn_local(async move {
            let dialog = FileDialog::new();
            let result = dialog.open_future(Some(&parent_window)).await;
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    let path_str = path.to_string_lossy().to_string();
                    let _ = tx
                        .send(Event::Input {
                            target: target.borrow().clone(),
                            text: format!("/upload {}", path_str),
                        })
                        .await;
                }
            }
        });
    });

    let active_directive = Rc::new(RefCell::new("Directive 055".to_string()));

    let tx_composer = tx_event.clone();
    let popover_composer = Popover::builder().build();
    let pop_box = Box::new(Orientation::Vertical, 8);
    pop_box.set_margin_top(10);
    pop_box.set_margin_bottom(10);
    pop_box.set_margin_start(10);
    pop_box.set_margin_end(10);

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
    let target_comp = active_target.clone();

    btn_comp_send.connect_clicked(move |_| {
        if let Some(pop) = pop_weak.upgrade() {
            let subject = sub_ent.text().to_string();
            let (start, end) = bod_buf.bounds();
            let body = bod_buf.text(&start, &end, false).to_string();
            let pb = pb_chk.is_active();
            let action = if b_ex.is_active() {
                "exec"
            } else if b_ar.is_active() {
                "arch"
            } else if b_db.is_active() {
                "debug"
            } else {
                "una"
            };
            let tx_async = tx_composer.clone();
            let target_val = target_comp.borrow().clone();
            let action_val = action.to_string();
            glib::MainContext::default().spawn_local(async move {
                let _ = tx_async
                    .send(Event::ComplexInput {
                        target: target_val,
                        subject,
                        body,
                        point_break: pb,
                        action: action_val,
                    })
                    .await;
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

    let input_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .vexpand(true)
        .valign(Align::Fill)
        .has_frame(false)
        .build();
    input_scroll.set_hexpand(true);
    let text_view = SourceView::builder()
        .wrap_mode(gtk4::WrapMode::WordChar)
        .show_line_numbers(false)
        .auto_indent(true)
        .accepts_tab(false)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(10)
        .right_margin(10)
        .vexpand(true)
        .build();
    enable_spelling(&text_view);
    text_view.add_css_class("view");
    input_scroll.set_child(Some(&text_view));

    let draft_path = gneiss_pal::paths::UnaPaths::root().join(".lumen_draft.txt");
    if let Ok(draft) = std::fs::read_to_string(&draft_path) {
        text_view.buffer().set_text(&draft);
    }
    let pending_save: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let draft_path_clone = draft_path.clone();
    let buffer_for_save = text_view.buffer();
    buffer_for_save.connect_changed(move |buf: &gtk4::TextBuffer| {
        if let Some(source) = pending_save.borrow_mut().take() {
            source.remove();
        }
        let (start, end) = buf.bounds();
        let text = buf.text(&start, &end, false).to_string();
        let path = draft_path_clone.clone();
        let pending_timeout = pending_save.clone();
        *pending_save.borrow_mut() = Some(glib::timeout_add_local(
            std::time::Duration::from_millis(500),
            move || {
                let _ = std::fs::write(&path, &text);
                *pending_timeout.borrow_mut() = None;
                glib::ControlFlow::Break
            },
        ));
    });

    let send_btn = Button::builder()
        .valign(Align::End)
        .icon_name("paper-plane-symbolic")
        .css_classes(vec!["suggested-action"])
        .tooltip_text("Send Message (Ctrl+Enter)")
        .build();
    let tx_clone_send = tx_event.clone();
    let buffer = text_view.buffer();
    let btn_send_clone = send_btn.clone();
    buffer.connect_changed(move |buf: &gtk4::TextBuffer| {
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
    let target_key = active_target.clone();
    let draft_wipe_path1 = draft_path.clone();
    key_controller.connect_key_pressed(move |_ctrl, key, _keycode, state| {
        if key != Key::Return {
            return glib::Propagation::Proceed;
        }
        if state.contains(ModifierType::SHIFT_MASK) {
            return glib::Propagation::Proceed;
        }
        let is_ctrl = state.contains(ModifierType::CONTROL_MASK);
        if is_ctrl || buffer_key.line_count() <= 1 {
            let (start, end) = buffer_key.bounds();
            let text = buffer_key.text(&start, &end, false).to_string();
            if !text.trim().is_empty() {
                let _ = std::fs::remove_file(&draft_wipe_path1);
                let tx_async = tx_clone_key.clone();
                let target_val = target_key.borrow().clone();
                glib::MainContext::default().spawn_local(async move {
                    let _ = tx_async
                        .send(Event::Input {
                            target: target_val,
                            text,
                        })
                        .await;
                });
                buffer_key.set_text("");
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    text_view.add_controller(key_controller);

    let target_send = active_target.clone();
    let buffer_send = buffer.clone();
    let draft_wipe_path2 = draft_path.clone();
    send_btn.connect_clicked(move |_| {
        let (start, end) = buffer_send.bounds();
        let text = buffer_send.text(&start, &end, false).to_string();
        if !text.trim().is_empty() {
            let _ = std::fs::remove_file(&draft_wipe_path2);
            let tx_async = tx_clone_send.clone();
            let target_val = target_send.borrow().clone();
            glib::MainContext::default().spawn_local(async move {
                let _ = tx_async
                    .send(Event::Input {
                        target: target_val,
                        text,
                    })
                    .await;
            });
            buffer_send.set_text("");
        }
    });

    input_container.append(&attach_btn);
    input_container.append(&input_scroll);
    input_container.append(&send_btn);

    comms_page.append(&input_container);

    workspace_stack.add_titled(&comms_page, Some("comms"), "Comms");

    let right_switcher = StackSwitcher::new();
    right_switcher.set_stack(Some(&workspace_stack));

    if let Some(settings) = gtk4::Settings::default() {
        let buf_chat = text_view
            .buffer()
            .downcast::<sourceview5::Buffer>()
            .unwrap();
        let buf_comp = body_buffer.clone();

        let update_theme = move |is_dark: bool| {
            let manager = sourceview5::StyleSchemeManager::default();
            let scheme_name = if is_dark { "Adwaita-dark" } else { "Adwaita" };
            if let Some(scheme) = manager.scheme(scheme_name) {
                buf_chat.set_style_scheme(Some(&scheme));
                buf_comp.set_style_scheme(Some(&scheme));
            }
        };

        update_theme(settings.is_gtk_application_prefer_dark_theme());

        settings.connect_gtk_application_prefer_dark_theme_notify(move |s| {
            update_theme(s.is_gtk_application_prefer_dark_theme());
        });
    }

    let tx_clone_load_hist = tx_event.clone();
    glib::MainContext::default().spawn_local(async move {
        let _ = tx_clone_load_hist.send(Event::LoadHistory).await;
    });

    let widgets = CommsWidgets {
        workspace_stack,
        right_switcher,
    };

    let pointers = CommsPointers {
        console_store,
        active_directive,
        is_prepending,
        is_fetching,
        history_sync_cursor: Rc::new(RefCell::new(0)),
        preflight_overlay: chat_overlay,
        preflight_stack_container,
        preflight_stack,
        preflight_sys_buf,
        preflight_dir_buf,
        preflight_eng_buf,
        preflight_prm_buf,
    };

    (widgets, pointers)
}
