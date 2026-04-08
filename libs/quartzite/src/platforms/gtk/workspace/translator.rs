// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use async_channel::Receiver as AsyncReceiver;
use tokio::sync::broadcast::Receiver as BroadcastReceiver;
use std::sync::{Arc, RwLock};
use bandy::state::AppState;
use bandy::SMessage;
use crate::platforms::gtk::types::GuiUpdate;

pub fn spawn_translator(
    mut rx_synapse: BroadcastReceiver<SMessage>,
    app_state: Arc<RwLock<AppState>>,
) -> AsyncReceiver<GuiUpdate> {
    let (tx_gui, rx_gui) = async_channel::unbounded::<GuiUpdate>();

    tokio::spawn(async move {
        let mut last_history_seq = 0;
        let mut last_console_seq = 0;

        loop {
            match rx_synapse.recv().await {
                Ok(msg) => {
                    match msg {
                SMessage::StateInvalidated => {
                    let (new_history_seq, new_console_seq) = {
                        let st = app_state.read().unwrap();
                        (st.history_seq, st.console_seq)
                    };

                    // Handle full state rollbacks/clears gracefully
                    if new_history_seq < last_history_seq || new_console_seq < last_console_seq {
                        last_history_seq = 0;
                        last_console_seq = 0;
                        let _ = tx_gui.send(GuiUpdate::ClearConsole).await;
                    }

                    let (history_delta, logs_delta, payload, tokens, sidebar, active_dir, synapse_err, shards) = {
                        let st = app_state.read().unwrap();

                        let h_delta_count = st.history_seq.saturating_sub(last_history_seq);
                        let h_delta = if h_delta_count > 0 {
                            if h_delta_count >= st.history.len() {
                                st.history.iter().cloned().collect::<Vec<_>>()
                            } else {
                                st.history.iter().skip(st.history.len() - h_delta_count).cloned().collect::<Vec<_>>()
                            }
                        } else {
                            Vec::new()
                        };

                        let l_delta_count = st.console_seq.saturating_sub(last_console_seq);
                        let l_delta = if l_delta_count > 0 {
                            if l_delta_count >= st.console_logs.len() {
                                st.console_logs.iter().cloned().collect::<Vec<_>>()
                            } else {
                                st.console_logs.iter().skip(st.console_logs.len() - l_delta_count).cloned().collect::<Vec<_>>()
                            }
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
                        if last_history_seq == 0 {
                            let _ = tx_gui.send(GuiUpdate::HistorySeed(history_delta)).await;
                        } else {
                            let _ = tx_gui.send(GuiUpdate::HistoryAppend(history_delta)).await;
                        }
                        last_history_seq = new_history_seq;
                    }
                    if !logs_delta.is_empty() {
                        let _ = tx_gui.send(GuiUpdate::ConsoleLogBatch(logs_delta)).await;
                        last_console_seq = new_console_seq;
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
                        SMessage::Matrix(bandy::MatrixEvent::TopologyMutated(topology)) => {
                            let _ = tx_gui.send(GuiUpdate::RefreshMatrix(topology)).await;
                        }
                        SMessage::Matrix(bandy::MatrixEvent::IngestTopology { ui_dag, semantic_dag: _ }) => {
                            // Checkpoint Beta: UI State Handshake
                            // We only need the dictionary (file paths) for the visual list.
                            if ui_dag.contains('$') {
                                let parts: Vec<&str> = ui_dag.splitn(2, '$').collect();
                                if let Some(dict_str) = parts.first() {
                                    let mut paths: Vec<String> = dict_str.split(',').map(|s| s.to_string()).collect();
                                    paths.sort_unstable();
                                    let _ = tx_gui.send(GuiUpdate::IngestMatrixTopology(paths)).await;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    });

    rx_gui
}
