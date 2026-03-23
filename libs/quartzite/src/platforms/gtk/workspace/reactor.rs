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
    pub console_store: gtk4::gio::ListStore,
    pub is_fetching: Rc<RefCell<bool>>,
    pub is_prepending: Rc<RefCell<bool>>,
    pub preflight_overlay: Overlay,
    pub preflight_stack_container: gtk4::Box,
    pub preflight_stack: Stack,
    pub preflight_sys_buf: sourceview5::Buffer,
    pub preflight_dir_buf: sourceview5::Buffer,
    pub preflight_eng_buf: sourceview5::Buffer,
    pub preflight_prm_buf: sourceview5::Buffer,
    pub matrix_store: gtk4::gio::ListStore,
}

pub fn spawn_listener(pointers: ReactorPointers, rx_gui: Receiver<GuiUpdate>) {
    gtk4::glib::MainContext::default().spawn_local(async move {
        while let Ok(update) = rx_gui.recv().await {
            match update {
                GuiUpdate::ConsoleLogBatch(logs) => {
                    let mut batch: Vec<gtk4::glib::Object> = Vec::new();
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
                        println!("Appending system log item: {}", content);
                        batch.push(obj.upcast());
                    }
                    if !batch.is_empty() {
                        let len = pointers.console_store.n_items();
                        pointers.console_store.splice(len, 0, &batch);
                        println!(">>> [J13 TRACE] REACTOR: Splice executed for {} items (ConsoleLogBatch)", batch.len());
                    }
                }
                GuiUpdate::HistorySeed(messages) => {
                    if messages.is_empty() {
                        *pointers.is_fetching.borrow_mut() = false;
                        *pointers.is_prepending.borrow_mut() = false;
                        continue;
                    }

                    let mut batch: Vec<gtk4::glib::Object> = Vec::new();
                    for (i, msg) in messages.into_iter().enumerate() {
                        let id = format!("{}-hist-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0), i);
                        let obj = HistoryObject::new(&id, &msg.sender, "History", &msg.timestamp, &msg.content, msg.is_chat);
                        println!("Seeding history item: {}", msg.content);
                        batch.push(obj.upcast());
                    }
                    if !batch.is_empty() {
                        // Splice at index 0 to properly prepend history
                        pointers.console_store.splice(0, 0, &batch);
                        println!(">>> [J16 TRACE] REACTOR: Splice executed for {} items (HistorySeed)", batch.len());
                    }

                    let fetch_lock = pointers.is_fetching.clone();
                    gtk4::glib::timeout_add_local(
                        std::time::Duration::from_millis(100),
                        move || {
                            *fetch_lock.borrow_mut() = false;
                            gtk4::glib::ControlFlow::Break
                        },
                    );
                }
                GuiUpdate::HistoryAppend(messages) => {
                    if messages.is_empty() {
                        *pointers.is_fetching.borrow_mut() = false;
                        *pointers.is_prepending.borrow_mut() = false;
                        continue;
                    }

                    let mut batch: Vec<gtk4::glib::Object> = Vec::new();
                    for (i, msg) in messages.into_iter().enumerate() {
                        let id = format!("{}-hist-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0), i);
                        let obj = HistoryObject::new(&id, &msg.sender, "History", &msg.timestamp, &msg.content, msg.is_chat);
                        println!("Appending history item: {}", msg.content);
                        batch.push(obj.upcast());
                    }
                    if !batch.is_empty() {
                        // Splice at index 0 to properly prepend older history
                        pointers.console_store.splice(0, 0, &batch);
                        println!(">>> [J16 TRACE] REACTOR: Splice executed for {} items (HistoryAppend)", batch.len());
                    }

                    let fetch_lock = pointers.is_fetching.clone();
                    gtk4::glib::timeout_add_local(
                        std::time::Duration::from_millis(100),
                        move || {
                            *fetch_lock.borrow_mut() = false;
                            gtk4::glib::ControlFlow::Break
                        },
                    );
                }
                GuiUpdate::ClearConsole => {
                    pointers.console_store.remove_all();
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
                    pointers.console_store.append(&err_obj);
                }
                GuiUpdate::ContextTelemetry(skeletons) => {
                    pointers.context_view.update(skeletons);
                }
                GuiUpdate::RefreshMatrix(topology) => {
                    pointers.matrix_store.remove_all();
                    for (id, label, depth) in topology {
                        let obj = crate::widgets::model::MatrixNodeObject::new(&id, &label, depth as u32);
                        pointers.matrix_store.append(&obj);
                    }
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
                _ => {}
            }
        }
    });
}
