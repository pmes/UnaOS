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
        while let Ok(msg) = rx_synapse.recv().await {
            match msg {
                SMessage::StateInvalidated => {
                    let (history, logs, payload, tokens, sidebar, active_dir, synapse_err, shards) = {
                        let st = app_state.read().unwrap();
                        (
                            st.history.clone(),
                            st.console_logs.clone(),
                            st.review_payload.clone(),
                            st.token_usage.clone(),
                            st.sidebar_status.clone(),
                            st.active_directive.clone(),
                            st.synapse_error.clone(),
                            st.shard_statuses.clone()
                        )
                    };
                    let _ = tx_gui.send(GuiUpdate::HistoryBatch(history)).await;
                    if let Some(log) = logs.last() {
                        let _ = tx_gui.send(GuiUpdate::ConsoleLog(log.clone())).await;
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
