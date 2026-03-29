// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use async_channel::Receiver;
use bandy::state::{ShardStatus, WolfpackState};
use gtk4::prelude::*;
use gtk4::{Label, Spinner, Image, Overlay, Stack};
use std::cell::RefCell;
use std::rc::Rc;

use crate::platforms::gtk::types::GuiUpdate;
use crate::widgets::model::HistoryObject;

pub struct ReactorPointers {
    pub spinner_una: Spinner,
    pub label_una: Label,
    pub spinner_s9: Spinner,
    pub label_s9: Label,
    pub token_label: Label,
    pub pulse_icon: Image,
    pub context_view: crate::widgets::telemetry::ContextView,
    pub active_directive: Rc<RefCell<String>>,
    pub chat_manager: Rc<RefCell<super::chat_manager::ChatBoxManager>>,
    pub preflight_overlay: Overlay,
    pub preflight_stack_container: gtk4::Box,
    pub preflight_stack: Stack,
    pub preflight_sys_buf: sourceview5::Buffer,
    pub preflight_dir_buf: sourceview5::Buffer,
    pub preflight_eng_buf: sourceview5::Buffer,
    pub preflight_prm_buf: sourceview5::Buffer,
    pub matrix_store: gtk4::gio::ListStore,
    pub matrix_selection: gtk4::MultiSelection,
    pub matrix_scroll: gtk4::ScrolledWindow,
    pub net_buffer: sourceview5::Buffer,
    pub net_view: sourceview5::View,
    pub network_btn: gtk4::Button,
}

pub fn spawn_listener(pointers: ReactorPointers, rx_gui: Receiver<GuiUpdate>) {
    gtk4::glib::MainContext::default().spawn_local(async move {
        while let Ok(update) = rx_gui.recv().await {
            match update {
                GuiUpdate::ConsoleLogBatch(logs) => {
                    let mut batch: Vec<HistoryObject> = Vec::new();
                    for (i, text) in logs.into_iter().enumerate() {
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
                            let after_s = &text.trim()[2..];
                            if let Some(first_char) = after_s.chars().next() {
                                if first_char.is_numeric() {
                                    sender = "Shard".to_string();
                                    is_chat = false;
                                    subject = "Wolfpack Output".to_string();
                                }
                            }
                        }

                        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                        let id = format!("{}-sys-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0), i);
                        let obj = HistoryObject::new(&id, &sender, &subject, &timestamp, &content, is_chat);
                        batch.push(obj);
                    }
                    if !batch.is_empty() {
                        pointers.chat_manager.borrow_mut().append_batch(batch);
                    }
                }
                GuiUpdate::HistorySeed(messages) => {
                    let mut cm = pointers.chat_manager.borrow_mut();
                    if messages.is_empty() {
                        cm.set_fetching(false);
                        cm.set_prepending(false);
                        continue;
                    }

                    let mut batch: Vec<HistoryObject> = Vec::new();
                    for (i, msg) in messages.into_iter().enumerate() {
                        let id = format!("{}-hist-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0), i);
                        let obj = HistoryObject::new(&id, &msg.sender, "History", &msg.timestamp, &msg.content, msg.is_chat);
                        batch.push(obj);
                    }
                    if !batch.is_empty() {
                        cm.prepend_history(batch);
                    }

                    let cm_clone = pointers.chat_manager.clone();
                    gtk4::glib::timeout_add_local(
                        std::time::Duration::from_millis(100),
                        move || {
                            cm_clone.borrow_mut().set_fetching(false);
                            gtk4::glib::ControlFlow::Break
                        },
                    );
                }
                GuiUpdate::HistoryAppend(messages) => {
                    let mut cm = pointers.chat_manager.borrow_mut();
                    if messages.is_empty() {
                        cm.set_fetching(false);
                        cm.set_prepending(false);
                        continue;
                    }

                    let mut batch: Vec<HistoryObject> = Vec::new();
                    for (i, msg) in messages.into_iter().enumerate() {
                        let id = format!("{}-hist-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0), i);
                        let obj = HistoryObject::new(&id, &msg.sender, "History", &msg.timestamp, &msg.content, msg.is_chat);
                        batch.push(obj);
                    }
                    if !batch.is_empty() {
                        cm.prepend_history(batch);
                    }

                    let cm_clone = pointers.chat_manager.clone();
                    gtk4::glib::timeout_add_local(
                        std::time::Duration::from_millis(100),
                        move || {
                            cm_clone.borrow_mut().set_fetching(false);
                            gtk4::glib::ControlFlow::Break
                        },
                    );
                }
                GuiUpdate::ClearConsole => {
                    pointers.chat_manager.borrow_mut().clear();
                }
                GuiUpdate::ShardStatusChanged { id, status } => {
                    if id == "una-prime" {
                        match status {
                            ShardStatus::Thinking => {
                                pointers.spinner_una.set_spinning(true);
                                pointers.spinner_una.start();
                                pointers.label_una.set_text("Una-Prime (Thinking)");
                                pointers.pulse_icon.add_css_class("thinking-pulse");
                            }
                            ShardStatus::Online => {
                                pointers.spinner_una.set_spinning(false);
                                pointers.spinner_una.stop();
                                pointers.label_una.set_text("Una-Prime");
                                pointers.pulse_icon.remove_css_class("thinking-pulse");
                            }
                            ShardStatus::Error => {
                                pointers.spinner_una.set_spinning(false);
                                pointers.spinner_una.stop();
                                pointers.label_una.set_text("Una-Prime (Error)");
                                pointers.pulse_icon.remove_css_class("thinking-pulse");
                            }
                            _ => {
                                pointers.spinner_una.set_spinning(false);
                                pointers.spinner_una.stop();
                                pointers.label_una.set_text(&format!("Una-Prime ({:?})", status));
                                pointers.pulse_icon.remove_css_class("thinking-pulse");
                            }
                        }
                    } else if id == "s9-mule" {
                        match status {
                            ShardStatus::Thinking => {
                                pointers.spinner_s9.set_spinning(true);
                                pointers.spinner_s9.start();
                                pointers.label_s9.set_text("S9-Mule (Thinking)");
                            }
                            ShardStatus::Online => {
                                pointers.spinner_s9.set_spinning(false);
                                pointers.spinner_s9.stop();
                                pointers.label_s9.set_text("S9-Mule");
                            }
                            ShardStatus::Error => {
                                pointers.spinner_s9.set_spinning(false);
                                pointers.spinner_s9.stop();
                                pointers.label_s9.set_text("S9-Mule (Error)");
                            }
                            _ => {
                                pointers.spinner_s9.set_spinning(false);
                                pointers.spinner_s9.stop();
                                pointers.label_s9.set_text(&format!("S9-Mule ({:?})", status));
                            }
                        }
                    }
                }
                GuiUpdate::SidebarStatus(state) => match state {
                    WolfpackState::Dreaming => {
                        // The dreaming state can also activate the pulse
                        pointers.pulse_icon.add_css_class("thinking-pulse");
                    }
                    _ => {
                        pointers.pulse_icon.remove_css_class("thinking-pulse");
                    }
                },
                GuiUpdate::TokenUsage(p, c, t) => {
                    let text = format!("Tokens: IN: {} | OUT: {} | TOTAL: {}", p, c, t);
                    pointers.token_label.set_text(&text);
                }
                GuiUpdate::ActiveDirective(d) => {
                    *pointers.active_directive.borrow_mut() = d;
                }
                GuiUpdate::ReviewPayload(payload) => {
                    // Update TextBuffers with Payload
                    pointers.preflight_sys_buf.set_text(&payload.system);
                    pointers.preflight_dir_buf.set_text(&payload.directives);
                    pointers.preflight_eng_buf.set_text(&payload.engrams);
                    pointers.preflight_prm_buf.set_text(&payload.prompt);

                    // Show Pre-Flight Stack Overlay via direct pointer
                    pointers.preflight_stack_container.set_visible(true);
                }
                GuiUpdate::SynapseError(err_msg) => {
                    let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                    let id = format!("{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
                    let err_obj =
                        HistoryObject::new(&id, "System Error", "Log", &timestamp, &err_msg, true);
                    pointers.chat_manager.borrow_mut().append_batch(vec![err_obj]);
                }
                GuiUpdate::ContextTelemetry(skeletons) => {
                    pointers.context_view.update(skeletons);
                }
                GuiUpdate::RefreshMatrix(topology) => {
                    // 0. Cache the VAdjustment value
                    let adj = pointers.matrix_scroll.vadjustment();
                    let current_scroll = adj.value();

                    // 1. Cache the active selection IDs
                    let mut saved_ids = std::collections::HashSet::new();
                    let current_bitset = pointers.matrix_selection.selection();
                    let size = current_bitset.size() as u32;
                    for i in 0..size {
                        if let Some(item) = pointers.matrix_store.item(current_bitset.nth(i)) {
                            if let Ok(obj) = item.downcast::<crate::widgets::model::MatrixNodeObject>() {
                                saved_ids.insert(obj.id());
                            }
                        }
                    }

                    // 2. ATOMIC SPLICE: Build new items and swap them in one move
                    let mut new_items = Vec::new();
                    for (id, label, depth) in topology {
                        new_items.push(crate::widgets::model::MatrixNodeObject::new(&id, &label, depth as u32));
                    }
                    pointers.matrix_store.splice(0, pointers.matrix_store.n_items(), &new_items);

                    // 3. Rebuild bitset and restore highlights
                    let new_bitset = gtk4::Bitset::new_empty();
                    for i in 0..pointers.matrix_store.n_items() {
                        if let Some(item) = pointers.matrix_store.item(i) {
                            if let Ok(obj) = item.downcast::<crate::widgets::model::MatrixNodeObject>() {
                                if saved_ids.contains(&obj.id()) {
                                    new_bitset.add(i);
                                }
                            }
                        }
                    }
                    pointers.matrix_selection.set_selection(&new_bitset, &new_bitset);

                    // 4. Restore the VAdjustment value via idle add
                    gtk4::glib::idle_add_local(move || {
                        adj.set_value(current_scroll);
                        gtk4::glib::ControlFlow::Break
                    });
                }
                GuiUpdate::IngestMatrixTopology(paths) => {
                    // Checkpoint Gamma: The Left Pane Model
                    // We inject the extracted dictionary paths into the Matrix ListStore.
                    pointers.matrix_store.remove_all();
                    for path in paths {
                        // Create a MatrixNodeObject with calculated depth.
                        let depth = path.matches('/').count() as u32;

                        // Extract just the filename to display, but keep the full path as the ID.
                        let label = path.split('/').last().unwrap_or(&path).to_string();

                        // Prevent visual indent underflow panics using saturating_sub or guards.
                        let obj = crate::widgets::model::MatrixNodeObject::new(&path, &label, depth);
                        pointers.matrix_store.append(&obj);
                    }
                }
                GuiUpdate::NetworkLog(log) => {
                    let mut end_iter = pointers.net_buffer.end_iter();
                    pointers.net_buffer.insert(&mut end_iter, &format!("{}\n", log));

                    // Auto-scroll to bottom
                    let mark = pointers.net_buffer.create_mark(None, &pointers.net_buffer.end_iter(), false);
                    pointers.net_view.scroll_to_mark(&mark, 0.0, false, 0.0, 0.0);
                }
                GuiUpdate::NetworkState(state) => {
                    pointers.network_btn.set_icon_name(&state);
                }
                _ => {}
            }
        }
    });
}
