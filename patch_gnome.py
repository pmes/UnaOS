import re

with open('libs/quartzite/src/platforms/gnome/mod.rs', 'r') as f:
    code = f.read()

# Replace GuiUpdate import
code = code.replace(
    'use gneiss_pal::{AppHandler, Event, GuiUpdate};',
    'use gneiss_pal::{AppHandler, Event};\nuse bandy::SMessage;\nuse bandy::state::AppState;\nuse std::sync::{Arc, RwLock};'
)

# Fix Backend::new signature
code = code.replace(
    'pub fn new<F>(app_id: &str, app_handler: A, rx: Receiver<GuiUpdate>, bootstrap_fn: F) -> Self',
    'pub fn new<F>(app_id: &str, app_handler: A, app_state: Arc<RwLock<AppState>>, rx_synapse: Receiver<SMessage>, bootstrap_fn: F) -> Self'
)

# Fix F signature inside Backend::new
code = code.replace(
    'Receiver<GuiUpdate>,\n            ) -> gtk4::Widget',
    'Arc<RwLock<AppState>>,\n                Receiver<SMessage>,\n            ) -> gtk4::Widget'
)

# Fix build_ui parameters inside Backend::new
code = code.replace(
    'build_ui(\n                app,\n                rx_clone.clone(),\n                bootstrap_rc.clone(),\n                tx_event.clone(),\n            );',
    'let app_state_clone = app_state.clone();\n            let rx_synapse_clone = rx_synapse.clone();\n            build_ui(\n                app,\n                app_state_clone,\n                rx_synapse_clone,\n                bootstrap_rc.clone(),\n                tx_event.clone(),\n            );'
)

# Remove old rx_clone
code = code.replace('let rx_clone = rx.clone(); // Clone channel receiver (async-channel is multi-consumer)', '')

# Fix build_ui function signature
code = code.replace(
    'fn build_ui<F>(\n    app: &Application,\n    rx: Receiver<GuiUpdate>,\n    bootstrap: Rc<F>,\n    tx_event: async_channel::Sender<Event>,\n) where',
    'fn build_ui<F>(\n    app: &Application,\n    app_state: Arc<RwLock<AppState>>,\n    rx_synapse: Receiver<SMessage>,\n    bootstrap: Rc<F>,\n    tx_event: async_channel::Sender<Event>,\n) where'
)

# Fix F signature in build_ui constraints
code = code.replace(
    'F: Fn(&ApplicationWindow, async_channel::Sender<Event>, Receiver<GuiUpdate>) -> gtk4::Widget',
    'F: Fn(&ApplicationWindow, async_channel::Sender<Event>, Arc<RwLock<AppState>>, Receiver<SMessage>) -> gtk4::Widget'
)

# Fix bootstrap call in build_ui
code = code.replace(
    'let content = bootstrap(gtk_window, tx_event, rx);',
    'let content = bootstrap(gtk_window, tx_event, app_state, rx_synapse);'
)

with open('libs/quartzite/src/platforms/gnome/mod.rs', 'w') as f:
    f.write(code)
