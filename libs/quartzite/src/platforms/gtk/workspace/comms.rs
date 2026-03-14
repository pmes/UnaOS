// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use async_channel::Sender;
use gtk4::prelude::*;
use gtk4::{
    Align, Box, Button, CheckButton, Entry, EventControllerKey, Expander, FileDialog,
    FilterListModel, GestureClick, Image, Label, ListItem, ListView, NoSelection, Orientation,
    Paned, PolicyType, Popover, PropagationPhase, ScrolledWindow, SignalListItemFactory, Stack,
    StackSwitcher, StackTransitionType, ToggleButton,
    gdk::{Key, ModifierType},
    gio, glib,
};
use sourceview5::View as SourceView;
use sourceview5::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::Event;
use crate::NativeWindow;
use crate::widgets::model::DispatchObject;

pub struct CommsWidgets {
    pub workspace_stack: Stack,
    pub right_switcher: StackSwitcher,
}

pub struct CommsPointers {
    pub console_store: gio::ListStore,
    pub active_directive: Rc<RefCell<String>>,
    pub is_prepending: Rc<RefCell<bool>>,
    pub is_fetching: Rc<RefCell<bool>>,
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

    let main_paned = Paned::new(Orientation::Vertical);
    main_paned.set_vexpand(true);
    main_paned.set_hexpand(true);
    main_paned.set_position(9999);
    main_paned.set_shrink_end_child(false);
    main_paned.set_wide_handle(false);

    let scrolled_window = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .vexpand(true)
        .build();

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
        let val = a.value();
        let page_size = a.page_size();
        let upper = a.upper();
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
        let old_upper = *last_upper_ref.borrow();
        let delta = upper - old_upper;
        *last_upper_ref.borrow_mut() = upper;

        if upper > page_size {
            if *was_at_bottom_upper.borrow() {
                a.set_value(upper - page_size);
            } else if *is_prepending_upper.borrow() && delta > 0.0 {
                a.set_value(a.value() + delta);
                *is_prepending_upper.borrow_mut() = false;
            }
        }
    });

    let console_store = gio::ListStore::new::<DispatchObject>();
    let console_filter = FilterListModel::new(Some(console_store.clone()), None::<gtk4::Filter>);
    let console_selection = NoSelection::new(Some(console_filter));

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
        chat_content_view.set_width_request(800);
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
            .height_request(200)
            .build();
        expander.set_child(Some(&payload_scroll));
        bubble.append(&expander);

        // --- Staging Mode (Payload Editor) ---
        let staging_box = Box::new(Orientation::Vertical, 8);
        staging_box.set_visible(false);
        staging_box.set_hexpand(true);
        staging_box.set_vexpand(true);

        let create_staging_section = |title: &str| -> (Box, SourceView) {
            let section_box = Box::new(Orientation::Vertical, 4);
            section_box.set_vexpand(true);
            section_box.set_hexpand(true);

            let label = Label::builder()
                .label(title)
                .xalign(0.0)
                .css_classes(vec!["dim-label"])
                .build();
            let view = SourceView::builder()
                .wrap_mode(gtk4::WrapMode::WordChar)
                .editable(true)
                .monospace(true)
                .build();
            view.add_css_class("view");
            view.set_vexpand(true);

            let scroll = ScrolledWindow::builder()
                .child(&view)
                .hscrollbar_policy(PolicyType::Never)
                .vscrollbar_policy(PolicyType::Automatic)
                .min_content_height(80)
                .vexpand(true)
                .build();

            section_box.append(&label);
            section_box.append(&scroll);

            (section_box, view)
        };

        let (box_sys, system_view) = create_staging_section("SYSTEM");
        let (box_dir, directives_view) = create_staging_section("DIRECTIVES");
        let (box_eng, engrams_view) = create_staging_section("ENGRAMS");
        let (box_prm, prompt_view) = create_staging_section("PROMPT");

        let paned_1 = Paned::new(Orientation::Vertical);
        let paned_2 = Paned::new(Orientation::Vertical);
        let paned_3 = Paned::new(Orientation::Vertical);

        paned_1.set_wide_handle(true);
        paned_2.set_wide_handle(true);
        paned_3.set_wide_handle(true);

        paned_1.set_vexpand(true);
        paned_1.set_hexpand(true);
        paned_2.set_vexpand(true);
        paned_2.set_hexpand(true);
        paned_3.set_vexpand(true);
        paned_3.set_hexpand(true);

        paned_1.set_shrink_start_child(false);
        paned_1.set_shrink_end_child(false);
        paned_2.set_shrink_start_child(false);
        paned_2.set_shrink_end_child(false);
        paned_3.set_shrink_start_child(false);
        paned_3.set_shrink_end_child(false);

        paned_3.set_start_child(Some(&box_eng));
        paned_3.set_end_child(Some(&box_prm));

        paned_2.set_start_child(Some(&box_dir));
        paned_2.set_end_child(Some(&paned_3));

        paned_1.set_start_child(Some(&box_sys));
        paned_1.set_end_child(Some(&paned_2));

        staging_box.append(&paned_1);

        let actions_box = Box::new(Orientation::Horizontal, 8);
        actions_box.set_halign(Align::End);
        let cancel_btn = Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Delete Post")
            .css_classes(vec!["flat", "destructive-action"])
            .build();
        let dispatch_btn = Button::builder()
            .icon_name("document-save-symbolic")
            .tooltip_text("Save and Send")
            .css_classes(vec!["suggested-action"])
            .build();
        actions_box.append(&cancel_btn);
        actions_box.append(&dispatch_btn);
        staging_box.append(&actions_box);

        bubble.append(&staging_box);

        // --- Pulse Mode (Animation) ---
        let pulse_box = Box::new(Orientation::Horizontal, 8);
        pulse_box.set_visible(false);
        pulse_box.set_halign(Align::Center);
        pulse_box.set_margin_top(12);
        pulse_box.set_margin_bottom(12);

        let pulse_spinner = gtk4::Spinner::new();
        pulse_spinner.set_spinning(true);
        pulse_spinner.add_css_class("pulse-spinner");

        let pulse_icon = Image::builder()
            .icon_name("brain-symbolic")
            .pixel_size(32)
            .build();
        pulse_icon.add_css_class("accent");

        pulse_box.append(&pulse_icon);
        pulse_box.append(&pulse_spinner);
        bubble.append(&pulse_box);

        root.append(&bubble);
        let right_spacer = Box::new(Orientation::Horizontal, 0);
        right_spacer.set_hexpand(true);
        root.append(&right_spacer);

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
                    .and_downcast::<DispatchObject>()
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

    let tx_dispatch = tx_event.clone();
    let console_store_bind = console_store.clone();
    console_factory.connect_bind(move |_factory, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else { return; };
        let Some(root) = item.child().and_then(|c| c.downcast::<Box>().ok()) else { return; };

        let Some(left_spacer) = root.first_child().and_then(|c| c.downcast::<Box>().ok()) else { return; };
        let Some(bubble) = left_spacer.next_sibling().and_then(|c| c.downcast::<Box>().ok()) else { return; };
        let Some(right_spacer) = bubble.next_sibling().and_then(|c| c.downcast::<Box>().ok()) else { return; };

        let Some(obj) = item.item().and_then(|c| c.downcast::<DispatchObject>().ok()) else { return; };

        let Some(header_box) = bubble.first_child().and_then(|c| c.downcast::<Box>().ok()) else { return; };
        let Some(left_expand_btn) = header_box.first_child().and_then(|c| c.downcast::<Button>().ok()) else { return; };
        let Some(meta_label) = left_expand_btn.next_sibling().and_then(|c| c.downcast::<Label>().ok()) else { return; };
        let Some(right_expand_btn) = header_box.last_child().and_then(|c| c.downcast::<Button>().ok()) else { return; };

        let Some(chat_view) = header_box.next_sibling().and_then(|c| c.downcast::<SourceView>().ok()) else { return; };
        let Some(expander) = chat_view.next_sibling().and_then(|c| c.downcast::<Expander>().ok()) else { return; };
        let Some(staging_box) = expander.next_sibling().and_then(|c| c.downcast::<Box>().ok()) else { return; };
        let Some(pulse_box) = staging_box.next_sibling().and_then(|c| c.downcast::<Box>().ok()) else { return; };

        bubble.remove_css_class("architect-bubble");
        bubble.remove_css_class("una-bubble");
        left_spacer.set_visible(false);
        right_spacer.set_visible(false);

        chat_view.set_visible(false);
        expander.set_visible(false);
        staging_box.set_visible(false);
        pulse_box.set_visible(false);
        left_expand_btn.set_visible(false);
        right_expand_btn.set_visible(false);

        let Some(paned_1) = staging_box.first_child().and_then(|c| c.downcast::<Paned>().ok()) else { return; };
        let Some(actions_box) = paned_1.next_sibling().and_then(|c| c.downcast::<Box>().ok()) else { return; };

        let Some(box_sys) = paned_1.start_child().and_then(|c| c.downcast::<Box>().ok()) else { return; };
        let Some(sys_scroll) = box_sys.last_child().and_then(|c| c.downcast::<ScrolledWindow>().ok()) else { return; };
        let Some(system_view) = sys_scroll.child().and_downcast::<SourceView>() else { return; };

        let Some(paned_2) = paned_1.end_child().and_then(|c| c.downcast::<Paned>().ok()) else { return; };
        let Some(box_dir) = paned_2.start_child().and_then(|c| c.downcast::<Box>().ok()) else { return; };
        let Some(dir_scroll) = box_dir.last_child().and_then(|c| c.downcast::<ScrolledWindow>().ok()) else { return; };
        let Some(directives_view) = dir_scroll.child().and_downcast::<SourceView>() else { return; };

        let Some(paned_3) = paned_2.end_child().and_then(|c| c.downcast::<Paned>().ok()) else { return; };
        let Some(box_eng) = paned_3.start_child().and_then(|c| c.downcast::<Box>().ok()) else { return; };
        let Some(eng_scroll) = box_eng.last_child().and_then(|c| c.downcast::<ScrolledWindow>().ok()) else { return; };
        let Some(engrams_view) = eng_scroll.child().and_downcast::<SourceView>() else { return; };

        let Some(box_prm) = paned_3.end_child().and_then(|c| c.downcast::<Box>().ok()) else { return; };
        let Some(prm_scroll) = box_prm.last_child().and_then(|c| c.downcast::<ScrolledWindow>().ok()) else { return; };
        let Some(prompt_view) = prm_scroll.child().and_downcast::<SourceView>() else { return; };

        let Some(cancel_btn) = actions_box.first_child().and_then(|c| c.downcast::<Button>().ok()) else { return; };
        let Some(dispatch_btn) = cancel_btn.next_sibling().and_then(|c| c.downcast::<Button>().ok()) else { return; };

        let message_type = obj.message_type();

        if message_type == 1 {
            // STAGING MODE
            staging_box.set_visible(true);
            bubble.add_css_class("architect-bubble");
            left_spacer.set_visible(true);
            right_spacer.set_visible(false);

            meta_label.set_text("PRE-FLIGHT STAGING");
            meta_label.add_css_class("role-architect");
            meta_label.set_xalign(1.0);

            system_view.buffer().set_text(&obj.system_text());
            directives_view.buffer().set_text(&obj.directives_text());
            engrams_view.buffer().set_text(&obj.engrams_text());
            prompt_view.buffer().set_text(&obj.prompt_text());

            let is_locked = obj.is_locked();
            system_view.set_editable(!is_locked);
            directives_view.set_editable(!is_locked);
            engrams_view.set_editable(!is_locked);
            prompt_view.set_editable(!is_locked);
            dispatch_btn.set_sensitive(!is_locked);
            cancel_btn.set_sensitive(!is_locked);

            let obj_clone = obj.clone();
            let bubble_clone = bubble.clone();
            let console_store_clone = console_store_bind.clone();

            if let Some(sig) = unsafe { cancel_btn.steal_data::<glib::SignalHandlerId>("clicked_sig") } {
                cancel_btn.disconnect(sig);
            }
            let cancel_sig = cancel_btn.connect_clicked(move |_| {
                let dialog = gtk4::AlertDialog::builder()
                    .message("Delete Payload?")
                    .detail("Are you sure you want to discard this pre-flight payload? This cannot be undone.")
                    .buttons(["Cancel", "Delete"])
                    .cancel_button(0)
                    .default_button(1)
                    .modal(true)
                    .build();

                let obj_cancel = obj_clone.clone();
                let store_cancel = console_store_clone.clone();

                let root_win = bubble_clone.root().and_downcast::<gtk4::Window>();

                dialog.choose(
                    root_win.as_ref(),
                    None::<&gio::Cancellable>,
                    move |result| {
                        if let Ok(choice) = result {
                            if choice == 1 {
                                let n = store_cancel.n_items();
                                for i in 0..n {
                                    if let Some(item) = store_cancel.item(i).and_downcast::<DispatchObject>() {
                                        if item.id() == obj_cancel.id() {
                                            store_cancel.remove(i);
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                );
            });

            let tx_clone2 = tx_dispatch.clone();
            let obj_clone2 = obj.clone();
            let sys_view_clone2 = system_view.clone();
            let dir_view_clone2 = directives_view.clone();
            let eng_view_clone2 = engrams_view.clone();
            let prm_view_clone2 = prompt_view.clone();

            let obj_sys = obj.clone();
            if let Some(sig) = unsafe { system_view.buffer().steal_data::<glib::SignalHandlerId>("changed_sig") } {
                system_view.buffer().disconnect(sig);
            }
            let sig_sys = system_view.buffer().connect_changed(move |buf| {
                let (s, e) = buf.bounds();
                obj_sys.set_system_text(buf.text(&s, &e, false).to_string());
            });
            unsafe { system_view.buffer().set_data("changed_sig", sig_sys); }

            let obj_dir = obj.clone();
            if let Some(sig) = unsafe { directives_view.buffer().steal_data::<glib::SignalHandlerId>("changed_sig") } {
                directives_view.buffer().disconnect(sig);
            }
            let sig_dir = directives_view.buffer().connect_changed(move |buf| {
                let (s, e) = buf.bounds();
                obj_dir.set_directives_text(buf.text(&s, &e, false).to_string());
            });
            unsafe { directives_view.buffer().set_data("changed_sig", sig_dir); }

            let obj_eng = obj.clone();
            if let Some(sig) = unsafe { engrams_view.buffer().steal_data::<glib::SignalHandlerId>("changed_sig") } {
                engrams_view.buffer().disconnect(sig);
            }
            let sig_eng = engrams_view.buffer().connect_changed(move |buf| {
                let (s, e) = buf.bounds();
                obj_eng.set_engrams_text(buf.text(&s, &e, false).to_string());
            });
            unsafe { engrams_view.buffer().set_data("changed_sig", sig_eng); }

            let obj_prm = obj.clone();
            if let Some(sig) = unsafe { prompt_view.buffer().steal_data::<glib::SignalHandlerId>("changed_sig") } {
                prompt_view.buffer().disconnect(sig);
            }
            let sig_prm = prompt_view.buffer().connect_changed(move |buf| {
                let (s, e) = buf.bounds();
                obj_prm.set_prompt_text(buf.text(&s, &e, false).to_string());
            });
            unsafe { prompt_view.buffer().set_data("changed_sig", sig_prm); }

            let cancel_btn_clone = cancel_btn.clone();
            let dispatch_btn_clone = dispatch_btn.clone();
            let console_store_pulse = console_store_bind.clone();

            if let Some(sig) = unsafe { dispatch_btn_clone.steal_data::<glib::SignalHandlerId>("clicked_sig") } {
                dispatch_btn_clone.disconnect(sig);
            }
            let btn_for_closure = dispatch_btn_clone.clone();
            let dispatch_sig = dispatch_btn_clone.connect_clicked(move |_| {
                obj_clone2.set_is_locked(true);
                sys_view_clone2.set_editable(false);
                dir_view_clone2.set_editable(false);
                eng_view_clone2.set_editable(false);
                prm_view_clone2.set_editable(false);
                cancel_btn_clone.set_sensitive(false);
                btn_for_closure.set_sensitive(false);

                let id = format!("{}-pulse", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
                let pulse_obj = DispatchObject::new_pulse(&id);
                console_store_pulse.append(&pulse_obj);

                let (s, e) = sys_view_clone2.buffer().bounds();
                let system_text = sys_view_clone2.buffer().text(&s, &e, false).to_string();

                let (s, e) = dir_view_clone2.buffer().bounds();
                let directives_text = dir_view_clone2.buffer().text(&s, &e, false).to_string();

                let (s, e) = eng_view_clone2.buffer().bounds();
                let engrams_text = eng_view_clone2.buffer().text(&s, &e, false).to_string();

                let (s, e) = prm_view_clone2.buffer().bounds();
                let prompt_text = prm_view_clone2.buffer().text(&s, &e, false).to_string();

                obj_clone2.set_system_text(system_text.clone());
                obj_clone2.set_directives_text(directives_text.clone());
                obj_clone2.set_engrams_text(engrams_text.clone());
                obj_clone2.set_prompt_text(prompt_text.clone());

                let payload = bandy::state::PreFlightPayload {
                    system: system_text,
                    directives: directives_text,
                    engrams: engrams_text,
                    prompt: prompt_text,
                };
                let json = serde_json::to_string(&payload).unwrap();

                let tx_async = tx_clone2.clone();
                glib::MainContext::default().spawn_local(async move {
                    let _ = tx_async.send(Event::DispatchPayload(json)).await;
                });
            });
            unsafe {
                cancel_btn.set_data("clicked_sig", cancel_sig);
                dispatch_btn.set_data("clicked_sig", dispatch_sig);
            }

        } else if message_type == 2 {
            // PULSE MODE
            pulse_box.set_visible(true);
            bubble.add_css_class("una-bubble");
            left_spacer.set_visible(false);
            right_spacer.set_visible(true);
            meta_label.set_text("AWAITING SYNAPSE...");
            meta_label.add_css_class("role-una");
            meta_label.set_xalign(0.0);
        } else {
            // STANDARD MODE
            let is_chat = obj.is_chat();
            let sender = obj.sender();
            let timestamp = obj.timestamp();
            let content = obj.content();
            let subject = obj.subject();

            if is_chat {
                chat_view.set_visible(true);
                meta_label.set_text(&format!("{} • {}", sender, timestamp));
                meta_label.remove_css_class("role-architect");
                meta_label.remove_css_class("role-una");
                meta_label.remove_css_class("role-system");

                let is_expanded = obj.is_expanded();
                let line_count = content.trim_end().lines().count();

                if sender == "Architect" {
                    meta_label.add_css_class("role-architect");
                    bubble.add_css_class("architect-bubble");
                    left_spacer.set_visible(true);
                    right_spacer.set_visible(false);
                    meta_label.set_halign(gtk4::Align::End);
                    meta_label.set_xalign(1.0);
                    if line_count > 11 {
                        left_expand_btn.set_visible(true);
                        right_expand_btn.set_visible(false);
                        left_expand_btn.set_icon_name(if is_expanded { "pan-up-symbolic" } else { "pan-down-symbolic" });
                    } else {
                        left_expand_btn.set_visible(false);
                        right_expand_btn.set_visible(false);
                    }
                } else {
                    if sender == "Una-Prime" {
                        meta_label.add_css_class("role-una");
                    } else {
                        meta_label.add_css_class("role-system");
                    }
                    bubble.add_css_class("una-bubble");
                    left_spacer.set_visible(false);
                    right_spacer.set_visible(true);
                    meta_label.set_halign(gtk4::Align::Start);
                    meta_label.set_xalign(0.0);
                    if line_count > 11 {
                        left_expand_btn.set_visible(false);
                        right_expand_btn.set_visible(true);
                        right_expand_btn.set_icon_name(if is_expanded { "pan-up-symbolic" } else { "pan-down-symbolic" });
                    } else {
                        left_expand_btn.set_visible(false);
                        right_expand_btn.set_visible(false);
                    }
                }
                if line_count > 11 && !is_expanded {
                    let truncated: String = content.trim_end().lines().take(11).collect::<Vec<&str>>().join("\n");
                    chat_view.buffer().set_text(&truncated);
                } else {
                    chat_view.buffer().set_text(content.trim_end());
                }
            } else {
                expander.set_visible(true);
                bubble.add_css_class("una-bubble");
                left_spacer.set_visible(false);
                right_spacer.set_visible(true);
                expander.set_label(Some(&format!("{} | {} | {}", sender, subject, timestamp)));
                if let Some(scroll) = expander.child().and_then(|c| c.downcast::<ScrolledWindow>().ok()) {
                    if let Some(content_view) = scroll.child().and_then(|c| c.downcast::<SourceView>().ok()) {
                        content_view.buffer().set_text(&content);
                    }
                }
                expander.set_expanded(false);
            }
        }
    });

    let console_list_view = ListView::new(Some(console_selection), Some(console_factory));
    console_list_view.add_css_class("console");
    console_list_view.set_valign(Align::End);
    scrolled_window.set_child(Some(&console_list_view));

    main_paned.set_start_child(Some(&scrolled_window));

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

    main_paned.set_end_child(Some(&input_container));
    comms_page.append(&main_paned);

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
    };

    (widgets, pointers)
}
