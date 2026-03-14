with open('libs/quartzite/src/platforms/gtk/spline.rs', 'r') as f:
    code = f.read()

code = code.replace(
    'pub fn bootstrap(\n        &self,\n        window: &crate::NativeWindow,\n        tx_event: async_channel::Sender<Event>,\n        rx: Receiver<GuiUpdate>,\n        rx_synapse: Receiver<bandy::SMessage>,\n    ) -> crate::NativeView {',
    'pub fn bootstrap(\n        &self,\n        window: &crate::NativeWindow,\n        tx_event: async_channel::Sender<Event>,\n        app_state: std::sync::Arc<std::sync::RwLock<AppState>>,\n        rx_synapse: Receiver<bandy::SMessage>,\n    ) -> crate::NativeView {'
)

with open('libs/quartzite/src/platforms/gtk/spline.rs', 'w') as f:
    f.write(code)
