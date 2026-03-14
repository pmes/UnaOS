// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use async_channel::Receiver;
use bandy::state::{ShardStatus, WolfpackState};
use gtk4::prelude::*;
use gtk4::{Label, Spinner};
use std::cell::RefCell;
use std::rc::Rc;

use crate::platforms::gtk::types::GuiUpdate;
use crate::widgets::model::DispatchObject;

pub struct ReactorPointers {
    pub spinner_una: Spinner,
    pub label_una: Label,
    pub spinner_s9: Spinner,
    pub label_s9: Label,
    pub token_label: Label,
    pub pulse_icon: Spinner,
    pub context_view: crate::widgets::telemetry::ContextView,
    pub active_directive: Rc<RefCell<String>>,
    pub console_store: gtk4::gio::ListStore,
    pub is_fetching: Rc<RefCell<bool>>,
    pub is_prepending: Rc<RefCell<bool>>,
}

pub fn spawn_listener(pointers: ReactorPointers, rx_gui: Receiver<GuiUpdate>) {
    gtk4::glib::MainContext::default().spawn_local(async move {
        while let Ok(update) = rx_gui.recv().await {
            match update {
                GuiUpdate::ConsoleLog(text) => {
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

                        let n = pointers.console_store.n_items();
                        let mut removals = Vec::new();

                        let mut target_staging_idx = None;
                        for i in (0..n).rev() {
                            if let Some(obj) = pointers
                                .console_store
                                .item(i)
                                .and_downcast::<DispatchObject>()
                            {
                                if obj.message_type() == 1 && obj.is_locked() {
                                    target_staging_idx = Some(i);
                                    break;
                                }
                            }
                        }

                        for i in 0..n {
                            if let Some(obj) = pointers
                                .console_store
                                .item(i)
                                .and_downcast::<DispatchObject>()
                            {
                                let t = obj.message_type();
                                if t == 2 {
                                    removals.push(i);
                                } else if t == 1 {
                                    if Some(i) == target_staging_idx {
                                        let timestamp =
                                            chrono::Local::now().format("%H:%M:%S").to_string();
                                        let id = obj.id();
                                        let prm = obj.prompt_text();
                                        let user_obj = DispatchObject::new(
                                            &id,
                                            "Architect",
                                            "Log",
                                            &timestamp,
                                            &prm,
                                            true,
                                        );
                                        pointers.console_store.splice(i, 1, &[user_obj]);
                                    }
                                }
                            }
                        }
                        for idx in removals.iter().rev() {
                            pointers.console_store.remove(*idx);
                        }
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
                    let id = format!("{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
                    let obj =
                        DispatchObject::new(&id, &sender, &subject, &timestamp, &content, is_chat);
                    pointers.console_store.append(&obj);
                }
                GuiUpdate::HistoryBatch(messages) => {
                    if messages.is_empty() {
                        *pointers.is_fetching.borrow_mut() = false;
                        *pointers.is_prepending.borrow_mut() = false;
                        continue;
                    }

                    *pointers.is_prepending.borrow_mut() = true;
                    let mut new_objects = Vec::new();
                    for (i, msg) in messages.into_iter().enumerate() {
                        let id = format!(
                            "{}-hist-{}",
                            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                            i
                        );
                        let obj = DispatchObject::new(
                            &id,
                            &msg.sender,
                            "History",
                            &msg.timestamp,
                            &msg.content,
                            msg.is_chat,
                        );
                        new_objects.push(obj);
                    }
                    pointers.console_store.splice(0, 0, &new_objects);

                    let fetch_lock = pointers.is_fetching.clone();
                    gtk4::glib::timeout_add_local(
                        std::time::Duration::from_millis(500),
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
                    let (spinner, label, name) = if id == "una-prime" {
                        (&pointers.spinner_una, &pointers.label_una, "Una-Prime")
                    } else if id == "s9-mule" {
                        (&pointers.spinner_s9, &pointers.label_s9, "S9-Mule")
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
                GuiUpdate::SidebarStatus(state) => match state {
                    WolfpackState::Dreaming => {
                        pointers.pulse_icon.start();
                    }
                    _ => {
                        pointers.pulse_icon.stop();
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
                    let id = format!("{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
                    let staging_obj = DispatchObject::new_staging(
                        &id,
                        &payload.system,
                        &payload.directives,
                        &payload.engrams,
                        &payload.prompt,
                    );
                    pointers.console_store.append(&staging_obj);
                }
                GuiUpdate::SynapseError(err_msg) => {
                    let n = pointers.console_store.n_items();
                    let mut pulse_idx = None;
                    for i in 0..n {
                        if let Some(obj) = pointers
                            .console_store
                            .item(i)
                            .and_downcast::<DispatchObject>()
                        {
                            if obj.message_type() == 2 {
                                pulse_idx = Some(i);
                                break;
                            }
                        }
                    }
                    if let Some(idx) = pulse_idx {
                        pointers.console_store.remove(idx);
                    }

                    let n = pointers.console_store.n_items();
                    for i in 0..n {
                        if let Some(obj) = pointers
                            .console_store
                            .item(i)
                            .and_downcast::<DispatchObject>()
                        {
                            if obj.message_type() == 1 {
                                obj.set_is_locked(false);
                                pointers.console_store.items_changed(i, 1, 1);
                            }
                        }
                    }

                    let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                    let id = format!("{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
                    let err_obj =
                        DispatchObject::new(&id, "System Error", "Log", &timestamp, &err_msg, true);
                    pointers.console_store.append(&err_obj);
                }
                GuiUpdate::ContextTelemetry(skeletons) => {
                    pointers.context_view.update(skeletons);
                }
                _ => {}
            }
        }
    });
}
