with open('libs/quartzite/src/platforms/gtk/spline.rs', 'r') as f:
    code = f.read()

# I will exactly match the text for `fn build_gtk_ui` signature and replace it.
code = code.replace(
    'fn build_gtk_ui(\n    window: &ApplicationWindow,\n    tx_event: async_channel::Sender<Event>,\n    rx: Receiver<GuiUpdate>,\n    rx_telemetry: Receiver<bandy::SMessage>,\n) -> gtk4::Widget {',
    'fn build_gtk_ui(\n    window: &ApplicationWindow,\n    tx_event: async_channel::Sender<Event>,\n    app_state: std::sync::Arc<std::sync::RwLock<AppState>>,\n    rx_synapse: Receiver<bandy::SMessage>,\n) -> gtk4::Widget {'
)

code = code.replace(
    'fn build_gtk_ui(\n    window: &ApplicationWindow,\n    tx_event: async_channel::Sender<Event>,\n    rx: Receiver<GuiUpdate>,\n    rx_synapse: Receiver<bandy::SMessage>,\n) -> gtk4::Widget {',
    'fn build_gtk_ui(\n    window: &ApplicationWindow,\n    tx_event: async_channel::Sender<Event>,\n    app_state: std::sync::Arc<std::sync::RwLock<AppState>>,\n    rx_synapse: Receiver<bandy::SMessage>,\n) -> gtk4::Widget {'
)

with open('libs/quartzite/src/platforms/gtk/spline.rs', 'w') as f:
    f.write(code)
