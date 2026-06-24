// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use async_channel::Sender;
use gtk4::prelude::*;
use gtk4::{
    Align, Box, Button, CheckButton, Entry, EventControllerKey, Expander, FileDialog,
    Label, Orientation,
    PolicyType, Popover, PropagationPhase, ScrolledWindow, Stack,
    StackSwitcher, StackTransitionType, ToggleButton, Overlay,
    gdk::{Key, ModifierType},
    glib,
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
    pub chat_manager: Rc<RefCell<super::chat_manager::ChatBoxManager>>,
    pub active_directive: Rc<RefCell<String>>,
    pub preflight_overlay: Overlay,
    pub preflight_stack_container: Box,
    pub preflight_stack: Stack,
    pub preflight_sys_buf: sourceview5::Buffer,
    pub preflight_dir_buf: sourceview5::Buffer,
    pub preflight_eng_buf: sourceview5::Buffer,
    pub preflight_prm_buf: sourceview5::Buffer,
}

struct PreflightStackData {
    preflight_stack_container: Box,
    preflight_stack: Stack,
    preflight_sys_buf: sourceview5::Buffer,
    preflight_dir_buf: sourceview5::Buffer,
    preflight_eng_buf: sourceview5::Buffer,
    preflight_prm_buf: sourceview5::Buffer,
}

struct InputAreaData {
    input_container: Box,
    chat_input_buffer: sourceview5::Buffer,
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


fn setup_preflight_stack(tx_event: &Sender<Event>) -> PreflightStackData {
    let preflight_stack_container = Box::new(Orientation::Vertical, 0);
    preflight_stack_container.set_halign(gtk4::Align::Fill);
    preflight_stack_container.set_valign(gtk4::Align::Fill);
    preflight_stack_container.set_vexpand(true);
    preflight_stack_container.set_hexpand(true);
    preflight_stack_container.add_css_class("background"); // Ensure opacity over chat
    preflight_stack_container.add_css_class("card");
    preflight_stack_container.set_margin_top(12);
    preflight_stack_container.set_margin_bottom(12);
    preflight_stack_container.set_margin_start(12);
    preflight_stack_container.set_margin_end(12);

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
        view.set_left_margin(12);
        view.set_right_margin(12);
        view.set_top_margin(12);
        view.set_bottom_margin(12);
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
        .css_classes(vec!["raised", "destructive-action"])
        .build();
    let dispatch_btn = Button::builder()
        .icon_name("document-save-symbolic")
        .tooltip_text("Save and Send Payload")
        .css_classes(vec!["suggested-action"])
        .build();

    dispatch_actions_box.append(&cancel_btn);
    dispatch_actions_box.append(&dispatch_btn);
    preflight_stack_container.append(&dispatch_actions_box);

    let tx_dispatch_preflight = tx_event.clone();
    let sys_buf_clone = preflight_sys_buf.clone();
    let dir_buf_clone = preflight_dir_buf.clone();
    let eng_buf_clone = preflight_eng_buf.clone();
    let prm_buf_clone = preflight_prm_buf.clone();
    let preflight_overlay_container_dispatch = preflight_stack_container.clone();

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

        preflight_overlay_container_dispatch.set_visible(false);

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

    PreflightStackData {
        preflight_stack_container,
        preflight_stack,
        preflight_sys_buf,
        preflight_dir_buf,
        preflight_eng_buf,
        preflight_prm_buf,
    }
}

fn setup_input_area(tx_event: &Sender<Event>, window: &NativeWindow, active_target: &Rc<RefCell<String>>, matrix_selection: gtk4::MultiSelection) -> InputAreaData {
    let input_container = Box::new(Orientation::Horizontal, 8);
    input_container.set_valign(Align::End);
    input_container.set_margin_start(16);
    input_container.set_margin_end(16);
    input_container.set_margin_bottom(16);
    input_container.set_margin_top(16);
    input_container.set_hexpand(true);
    input_container.set_size_request(-1, 120); // Prevents the slider from crushing the input

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

    let input_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .has_frame(true)
        .propagate_natural_height(true)
        .max_content_height(150)
        .build();
    input_scroll.set_hexpand(true);
    input_scroll.add_css_class("card");
    input_scroll.set_margin_top(8);
    input_scroll.set_margin_bottom(8);
    input_scroll.set_margin_start(8);
    input_scroll.set_margin_end(8);
    let text_view = SourceView::builder()
        .wrap_mode(gtk4::WrapMode::WordChar)
        .show_line_numbers(false)
        .auto_indent(true)
        .accepts_tab(false)
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
                let path_for_task = path.clone();
                let text_for_task = text.clone();
                // Offload disk I/O to the Tokio blocking thread pool to protect the UI render cycle
                tokio::task::spawn_blocking(move || {
                    let _ = std::fs::write(&path_for_task, &text_for_task);
                });
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
        let has_class = btn_send_clone.has_css_class("suggested-action");
        if buf.line_count() > 1 && has_class {
            btn_send_clone.remove_css_class("suggested-action");
        } else if buf.line_count() <= 1 && !has_class {
            btn_send_clone.add_css_class("suggested-action");
        }
    });

    let key_controller = EventControllerKey::new();
    key_controller.set_propagation_phase(PropagationPhase::Capture);
    let buffer_key = buffer.clone();
    let send_btn_clone = send_btn.clone();
    key_controller.connect_key_pressed(move |_ctrl, key, _keycode, state| {
        if key != Key::Return {
            return glib::Propagation::Proceed;
        }
        if state.contains(ModifierType::SHIFT_MASK) {
            return glib::Propagation::Proceed;
        }
        let is_ctrl = state.contains(ModifierType::CONTROL_MASK);
        if is_ctrl || buffer_key.line_count() <= 1 {
            send_btn_clone.emit_clicked();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    text_view.add_controller(key_controller);

    let target_send = active_target.clone();
    let buffer_send = buffer.clone();
    let draft_wipe_path2 = draft_path.clone();
    let matrix_selection_btn_clone = matrix_selection.clone();
    send_btn.connect_clicked(move |_| {
        let (start, end) = buffer_send.bounds();
        let text = buffer_send.text(&start, &end, false).to_string();
        if !text.trim().is_empty() {
            let bitset = matrix_selection_btn_clone.selection();
            let mut selected_ids = Vec::new();

            let size = bitset.size() as u32;
            for i in 0..size {
                let pos = bitset.nth(i);

                if let Some(obj) = matrix_selection_btn_clone.item(pos) {
                    if let Ok(node_obj) = obj.downcast::<crate::widgets::model::MatrixNodeObject>() {
                        selected_ids.push(node_obj.id());
                    }
                }
            }

            let cart_payload = selected_ids.clone();
            let wipe_path = draft_wipe_path2.clone();
            // Offload disk I/O to prevent UI stutter when clearing the draft
            tokio::task::spawn_blocking(move || {
                let _ = std::fs::remove_file(&wipe_path);
            });

            let tx_async = tx_clone_send.clone();
            let target_val = target_send.borrow().clone();
            glib::MainContext::default().spawn_local(async move {
                // 1. Dispatch the topological constraints FIRST
                if !cart_payload.is_empty() {
                    let _ = tx_async.send(crate::Event::UpdateMatrixSelection(cart_payload)).await;
                }

                // 2. Dispatch the standard input
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

    let chat_input_buffer = text_view.buffer().downcast::<sourceview5::Buffer>().unwrap();

    InputAreaData {
        input_container,
        chat_input_buffer,
    }
}


pub fn build(
    window: &NativeWindow,
    tx_event: Sender<Event>,
    active_target: Rc<RefCell<String>>,
    composer_btn: Button,
    tetra: &crate::tetra::StreamTetra,
    matrix_selection: gtk4::MultiSelection,
) -> (CommsWidgets, CommsPointers) {
    let workspace_stack = Stack::new();
    workspace_stack.set_vexpand(true);
    workspace_stack.set_transition_type(StackTransitionType::SlideLeftRight);

    let comms_page = gtk4::Paned::new(Orientation::Vertical);
    comms_page.set_hexpand(true);
    comms_page.set_vexpand(true);

    let chat_overlay = Overlay::new();
    chat_overlay.set_hexpand(true);
    chat_overlay.set_vexpand(true);

    let chat_manager = super::chat_manager::ChatBoxManager::new(tx_event.clone(), tetra);
    let scrolled_window = chat_manager.borrow().scrolled_window.clone();

    chat_overlay.set_child(Some(&scrolled_window));


    // --- PRE-FLIGHT STACK (Layer 2) ---
    let preflight_data = setup_preflight_stack(&tx_event);
    let preflight_stack_container = preflight_data.preflight_stack_container;
    let preflight_stack = preflight_data.preflight_stack;
    let preflight_sys_buf = preflight_data.preflight_sys_buf;
    let preflight_dir_buf = preflight_data.preflight_dir_buf;
    let preflight_eng_buf = preflight_data.preflight_eng_buf;
    let preflight_prm_buf = preflight_data.preflight_prm_buf;

    chat_overlay.add_overlay(&preflight_stack_container);
    // Initially hide the PreFlight Stack
    preflight_stack_container.set_visible(false);

    let input_area_data = setup_input_area(&tx_event, window, &active_target, matrix_selection);
    let input_container = input_area_data.input_container;
    let chat_input_buffer = input_area_data.chat_input_buffer;

    comms_page.set_start_child(Some(&chat_overlay));
    comms_page.set_end_child(Some(&input_container));
    comms_page.set_shrink_end_child(false);
    comms_page.set_resize_end_child(false);

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
    body_view.set_vexpand(true);
    body_view.set_size_request(-1, 150);

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

    // The paned now controls the input orientation dynamically via start/end, removing the need for box prepending/appending
    workspace_stack.add_titled(&comms_page, Some("comms"), "Comms");

    let right_switcher = StackSwitcher::new();
    right_switcher.set_stack(Some(&workspace_stack));

    if let Some(settings) = gtk4::Settings::default() {
        let buf_chat = chat_input_buffer.clone();
        let buf_comp = body_buffer.clone();
        let buf_sys = preflight_sys_buf.clone();
        let buf_dir = preflight_dir_buf.clone();
        let buf_eng = preflight_eng_buf.clone();
        let buf_prm = preflight_prm_buf.clone();

        let update_theme = move |is_dark: bool| {
            let manager = sourceview5::StyleSchemeManager::default();
            let scheme_name = if is_dark { "Adwaita-dark" } else { "Adwaita" };
            if let Some(scheme) = manager.scheme(scheme_name) {
                buf_chat.set_style_scheme(Some(&scheme));
                buf_comp.set_style_scheme(Some(&scheme));
                buf_sys.set_style_scheme(Some(&scheme));
                buf_dir.set_style_scheme(Some(&scheme));
                buf_eng.set_style_scheme(Some(&scheme));
                buf_prm.set_style_scheme(Some(&scheme));
            }
        };

        update_theme(settings.is_gtk_application_prefer_dark_theme());

        settings.connect_gtk_application_prefer_dark_theme_notify(move |s| {
            update_theme(s.is_gtk_application_prefer_dark_theme());
        });
    }

    let widgets = CommsWidgets {
        workspace_stack,
        right_switcher,
    };

    let pointers = CommsPointers {
        chat_manager,
        active_directive,
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
