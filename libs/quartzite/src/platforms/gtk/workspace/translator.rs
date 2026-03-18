// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use async_channel::Receiver;
use std::sync::{Arc, RwLock};
use bandy::state::AppState;
use bandy::SMessage;
use crate::platforms::gtk::types::GuiUpdate;

pub fn spawn_translator(
    rx_synapse: Receiver<SMessage>,
    app_state: Arc<RwLock<AppState>>,
) -> Receiver<GuiUpdate> {
    let (tx_gui, rx_gui) = async_channel::unbounded::<GuiUpdate>();

    tokio::spawn(async move {
        let mut history_cursor = 0;
        let mut console_cursor = 0;

        println!(">>> [J13 TRACE] TRANSLATOR: Thread spawned. Waiting for Synapse messages...");

        while let Ok(msg) = rx_synapse.recv().await {
            println!(">>> [J13 TRACE] TRANSLATOR: Received a Synapse message.");
            match msg {
                SMessage::StateInvalidated => {
                    let (new_history_len, new_console_len) = {
                        println!(">>> [J13 TRACE] TRANSLATOR: Processing StateInvalidated. Attempting to acquire read lock...");
                        let st = app_state.read().unwrap();
                        println!(">>> [J13 TRACE] TRANSLATOR: Read lock acquired. history_len: {}, console_len: {}", st.history.len(), st.console_logs.len());
                        (st.history.len(), st.console_logs.len())
                    };

                    // Handle full state rollbacks/clears gracefully
                    if new_history_len < history_cursor || new_console_len < console_cursor {
                        history_cursor = 0;
                        console_cursor = 0;
                        let _ = tx_gui.send(GuiUpdate::ClearConsole).await;
                    }

                    let (history_delta, logs_delta, payload, tokens, sidebar, active_dir, synapse_err, shards) = {
                        let st = app_state.read().unwrap();

                        let h_delta = if st.history.len() > history_cursor {
                            // If cursor is 0 (initial boot or clear), grab everything.
                            // Otherwise, only grab the delta.
                            st.history[history_cursor..].to_vec()
                        } else {
                            Vec::new()
                        };

                        let l_delta = if st.console_logs.len() > console_cursor {
                            st.console_logs[console_cursor..].to_vec()
                        } else {
                            Vec::new()
                        };

                        (
                            h_delta,
                            l_delta,
                            st.review_payload.clone(),
                            st.token_usage.clone(),
                            st.sidebar_status.clone(),
                            st.active_directive.clone(),
                            st.synapse_error.clone(),
                            st.shard_statuses.clone()
                        )
                    };

                    if !history_delta.is_empty() {
                        println!(">>> [J13 TRACE] TRANSLATOR: Sending HistoryBatch with {} items", history_delta.len());
                        let _ = tx_gui.send(GuiUpdate::HistoryBatch(history_delta)).await;
                        history_cursor = new_history_len;
                    }
                    if !logs_delta.is_empty() {
                        let _ = tx_gui.send(GuiUpdate::ConsoleLogBatch(logs_delta)).await;
                        console_cursor = new_console_len;
                    }

                    if let Some(p) = payload {
                        let _ = tx_gui.send(GuiUpdate::ReviewPayload(p)).await;
                    }
                    let _ = tx_gui.send(GuiUpdate::TokenUsage(tokens.0, tokens.1, tokens.2)).await;
                    let _ = tx_gui.send(GuiUpdate::SidebarStatus(sidebar)).await;
                    if !active_dir.is_empty() {
                        let _ = tx_gui.send(GuiUpdate::ActiveDirective(active_dir)).await;
                    }
                    if let Some(err) = synapse_err {
                        let _ = tx_gui.send(GuiUpdate::SynapseError(err)).await;
                    }
                    for (id, status) in shards {
                        let _ = tx_gui.send(GuiUpdate::ShardStatusChanged { id, status }).await;
                    }
                }
                SMessage::ContextTelemetry { skeletons } => {
                    let _ = tx_gui.send(GuiUpdate::ContextTelemetry(skeletons)).await;
                }
                _ => {}
            }
        }
    });

    rx_gui
}
