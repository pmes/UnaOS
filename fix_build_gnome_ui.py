import re

with open('libs/quartzite/src/platforms/gtk/spline.rs', 'r') as f:
    code = f.read()

code = code.replace(
    'fn build_gnome_ui(\n    window: &crate::NativeWindow,\n    tx_event: async_channel::Sender<Event>,\n    rx: Receiver<GuiUpdate>,\n    rx_synapse: Receiver<bandy::SMessage>,\n) -> crate::NativeView {',
    'fn build_gnome_ui(\n    window: &crate::NativeWindow,\n    tx_event: async_channel::Sender<Event>,\n    app_state: std::sync::Arc<std::sync::RwLock<AppState>>,\n    rx_synapse: Receiver<bandy::SMessage>,\n) -> crate::NativeView {'
)

# Replace the inner GLib loop
translator = """
    let (tx_gui, rx) = async_channel::unbounded::<GuiUpdate>();
    let rx_synapse_clone = rx_synapse.clone();
    let app_state_clone = app_state.clone();

    tokio::spawn(async move {
        while let Ok(msg) = rx_synapse_clone.recv().await {
            if matches!(msg, bandy::SMessage::StateInvalidated) {
                let (history, logs, payload, tokens, sidebar, active_dir, synapse_err, shards) = {
                    let st = app_state_clone.read().unwrap();
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
        }
    });

    let mut rx_glib = rx;
"""

# Replace the original `let mut rx_glib = rx;`
code = code.replace('let mut rx_glib = rx;\n', translator)

with open('libs/quartzite/src/platforms/gtk/spline.rs', 'w') as f:
    f.write(code)
