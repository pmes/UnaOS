import re

with open('libs/quartzite/src/platforms/gtk/spline.rs', 'r') as f:
    code = f.read()

code = code.replace(
    'pub fn bootstrap(\n        &self,\n        window: &ApplicationWindow,\n        tx_event: async_channel::Sender<Event>,\n        rx: Receiver<GuiUpdate>,\n        rx_synapse: Receiver<bandy::SMessage>,\n    ) -> gtk4::Widget {',
    'pub fn bootstrap(\n        &self,\n        window: &ApplicationWindow,\n        tx_event: async_channel::Sender<Event>,\n        app_state: std::sync::Arc<std::sync::RwLock<AppState>>,\n        rx_synapse: Receiver<bandy::SMessage>,\n    ) -> gtk4::Widget {'
)

code = code.replace(
    'return build_gnome_ui(window, tx_event, rx, rx_synapse);',
    'return build_gnome_ui(window, tx_event, app_state, rx_synapse);'
)
code = code.replace(
    'return build_gtk_ui(window, tx_event, rx, rx_synapse);',
    'return build_gtk_ui(window, tx_event, app_state, rx_synapse);'
)

with open('libs/quartzite/src/platforms/gtk/spline.rs', 'w') as f:
    f.write(code)
