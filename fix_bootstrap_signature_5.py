with open('libs/quartzite/src/platforms/gtk/spline.rs', 'r') as f:
    code = f.read()

# Replace inner method `fn build_gtk_ui` precisely
code = code.replace(
    'fn build_gtk_ui(\n    window: &ApplicationWindow,\n    tx_event: async_channel::Sender<Event>,\n    rx: Receiver<GuiUpdate>,\n    rx_synapse: Receiver<bandy::SMessage>,\n) -> gtk4::Widget {',
    'fn build_gtk_ui(\n    window: &ApplicationWindow,\n    tx_event: async_channel::Sender<Event>,\n    app_state: std::sync::Arc<std::sync::RwLock<AppState>>,\n    rx_synapse: Receiver<bandy::SMessage>,\n) -> gtk4::Widget {\n' +
    '    let (tx_gui, rx) = async_channel::unbounded::<GuiUpdate>();\n' +
    '    let rx_synapse_clone = rx_synapse.clone();\n' +
    '    let app_state_clone = app_state.clone();\n' +
    '    tokio::spawn(async move {\n' +
    '        while let Ok(msg) = rx_synapse_clone.recv().await {\n' +
    '            if matches!(msg, bandy::SMessage::StateInvalidated) {\n' +
    '                let (history, logs, payload, tokens, sidebar, active_dir, synapse_err, shards) = {\n' +
    '                    let st = app_state_clone.read().unwrap();\n' +
    '                    (\n' +
    '                        st.history.clone(),\n' +
    '                        st.console_logs.clone(),\n' +
    '                        st.review_payload.clone(),\n' +
    '                        st.token_usage.clone(),\n' +
    '                        st.sidebar_status.clone(),\n' +
    '                        st.active_directive.clone(),\n' +
    '                        st.synapse_error.clone(),\n' +
    '                        st.shard_statuses.clone()\n' +
    '                    )\n' +
    '                };\n' +
    '                let _ = tx_gui.send(GuiUpdate::HistoryBatch(history)).await;\n' +
    '                if let Some(log) = logs.last() {\n' +
    '                    let _ = tx_gui.send(GuiUpdate::ConsoleLog(log.clone())).await;\n' +
    '                }\n' +
    '                if let Some(p) = payload {\n' +
    '                    let _ = tx_gui.send(GuiUpdate::ReviewPayload(p)).await;\n' +
    '                }\n' +
    '                let _ = tx_gui.send(GuiUpdate::TokenUsage(tokens.0, tokens.1, tokens.2)).await;\n' +
    '                let _ = tx_gui.send(GuiUpdate::SidebarStatus(sidebar)).await;\n' +
    '                if !active_dir.is_empty() {\n' +
    '                    let _ = tx_gui.send(GuiUpdate::ActiveDirective(active_dir)).await;\n' +
    '                }\n' +
    '                if let Some(err) = synapse_err {\n' +
    '                    let _ = tx_gui.send(GuiUpdate::SynapseError(err)).await;\n' +
    '                }\n' +
    '                for (id, status) in shards {\n' +
    '                    let _ = tx_gui.send(GuiUpdate::ShardStatusChanged { id, status }).await;\n' +
    '                }\n' +
    '            }\n' +
    '        }\n' +
    '    });\n'
)

with open('libs/quartzite/src/platforms/gtk/spline.rs', 'w') as f:
    f.write(code)
