import re

with open('libs/quartzite/src/platforms/gtk/spline.rs', 'r') as f:
    code = f.read()

code = code.replace(
    'use gneiss_pal::{GuiUpdate, WolfpackState};',
    'use crate::platforms::gtk::types::GuiUpdate;\nuse bandy::state::{WolfpackState, PreFlightPayload, AppState, HistoryItem, ShardStatus};'
)
code = code.replace('gneiss_pal::PreFlightPayload', 'PreFlightPayload')
code = code.replace('gneiss_pal::HistoryItem', 'HistoryItem')
code = code.replace('gneiss_pal::ShardStatus', 'ShardStatus')

# Clear old ShardStatus imports
code = code.replace('use gneiss_pal::shard::ShardStatus;\n', '')

# Ensure bootstrap replaces the signature perfectly
code = code.replace(
    'pub fn bootstrap(\n        &self,\n        window: &ApplicationWindow,\n        tx_event: async_channel::Sender<Event>,\n        rx: Receiver<GuiUpdate>,\n        rx_telemetry: Receiver<bandy::SMessage>,\n    ) -> gtk4::Widget {',
    'pub fn bootstrap(\n        &self,\n        window: &ApplicationWindow,\n        tx_event: async_channel::Sender<Event>,\n        app_state: std::sync::Arc<std::sync::RwLock<AppState>>,\n        rx_synapse: Receiver<bandy::SMessage>,\n    ) -> gtk4::Widget {'
)

code = code.replace(
    'return build_gtk_ui(window, tx_event, rx, rx_telemetry);',
    'return build_gtk_ui(window, tx_event, app_state, rx_synapse);'
)

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

"""

code = code.replace(
    'fn build_gtk_ui(\n    window: &ApplicationWindow,\n    tx_event: async_channel::Sender<Event>,\n    rx: Receiver<GuiUpdate>,\n    rx_telemetry: Receiver<bandy::SMessage>,\n) -> gtk4::Widget {',
    'fn build_gtk_ui(\n    window: &ApplicationWindow,\n    tx_event: async_channel::Sender<Event>,\n    app_state: std::sync::Arc<std::sync::RwLock<AppState>>,\n    rx_synapse: Receiver<bandy::SMessage>,\n) -> gtk4::Widget {\n' + translator
)

# And now we have a shadowing issue where the original code had:
# `rx_telemetry: Receiver<bandy::SMessage>` inside `build_gtk_ui` and used it later down in the function.
code = code.replace('rx_telemetry', 'rx_synapse')

with open('libs/quartzite/src/platforms/gtk/spline.rs', 'w') as f:
    f.write(code)
