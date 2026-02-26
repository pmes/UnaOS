mod core;
mod cortex;
mod ui;

use crate::ui::telemetry::ContextView;
use bandy::{SMessage, Synapse, telemetry};
use gneiss_pal::paths::UnaPaths;
use gtk4::prelude::*;
use gtk4::{ApplicationWindow, Paned, Orientation};
use quartzite::{self, Backend};
use std::rc::Rc;
use vein::{CommsSpline, VeinHandler};
use glib::MainContext;

fn main() {
    // 0. Ignite the Substrate Reactor (Tokio)
    let rt = tokio::runtime::Runtime::new().expect("CRITICAL: Failed to ignite Tokio reactor");
    let _guard = rt.enter();

    // 1. Establish Base Camp
    UnaPaths::awaken().expect("CRITICAL: Failed to awaken spatial paths");
    let asset_path = UnaPaths::root().join("quartzite.gresource");

    // Split the brain: Conscious (Vein) vs Subconscious (Core)
    let vein_storage = UnaPaths::primary_vault();
    let cortex_vault = UnaPaths::subconscious_vault();

    // 2. Ignite Telemetry
    telemetry::ignite(UnaPaths::root().join("logs"));
    log::info!("Lumen Boot Sequence Initiated.");

    // NEW: Create Telemetry Channel (Pure Async)
    // This is the direct line from the Cortex to the HUD.
    // Una: We use async_channel here, not glib::MainContext::channel.
    let (telemetry_tx, telemetry_rx) = async_channel::unbounded::<SMessage>();

    // 3. Ignite the Spine
    let synapse = Synapse::new();

    // 4. Initialize Crypto Substrate
    let _ = rustls::crypto::ring::default_provider().install_default();

    // 5. Awaken the Autonomous Core (The Subconscious)
    let core_synapse = synapse.clone();
    rt.spawn(async move {
        core::ignite(cortex_vault, core_synapse).await;
    });

    // 6. FORCE DEPLOY ASSETS (S74)
    if let Err(e) = quartzite::deploy_assets(&asset_path) {
        synapse.fire(SMessage::Log {
            level: String::from("ERROR"),
            source: String::from("LUMEN_UI"),
            content: format!("Failed to deploy assets: {}", e),
        });
    }
    quartzite::init_with_path(&asset_path);

    // 7. Ignite the AI Handler (The Conscious Vein)
    let (gui_tx, gui_rx) = async_channel::unbounded();
    // We pass the telemetry_tx to Vein so it can broadcast gravity updates.
    let app = VeinHandler::new(gui_tx, vein_storage, synapse.tx(), telemetry_tx);

    synapse.fire(SMessage::Log {
        level: String::from("INFO"),
        source: String::from("LUMEN"),
        content: String::from("Nervous System Online. Handing control to Quartzite."),
    });

    // 8. View & Engine Ignition
    let spline = Rc::new(CommsSpline::new());

    // THE FUSION: We wrap the Vein UI with our new HUD.
    let bootstrap = move |window: &ApplicationWindow, tx: async_channel::Sender<quartzite::Event>, rx: async_channel::Receiver<vein::model::GuiUpdate>| {
        // 1. Get the Vein UI (The Command Center)
        // We cast the generic type to Widget immediately.
        let vein_widget = spline.bootstrap(window, tx, rx);

        // 2. Create the HUD (ContextView)
        let hud = ContextView::new();
        let hud_widget = hud.container.clone();

        // 3. Attach the Telemetry Stream (Async -> Main Loop Bridge)
        // We spawn a local task on the GTK main loop to poll the async channel.
        let telemetry_rx_clone = telemetry_rx.clone();

        MainContext::default().spawn_local(async move {
            while let Ok(msg) = telemetry_rx_clone.recv().await {
                if let SMessage::ContextTelemetry { skeletons } = msg {
                    hud.update(skeletons);
                }
            }
        });

        // 4. Fuse them (Paned)
        // We put the HUD on the right (End) and the Cortex on the left (Start).
        let root = Paned::new(Orientation::Horizontal);
        root.set_start_child(Some(&vein_widget));
        root.set_end_child(Some(&hud_widget));
        root.set_position(900); // Prioritize Vein width
        root.set_wide_handle(true);
        root.set_shrink_end_child(false); // HUD should not shrink to zero

        root.upcast::<gtk4::Widget>()
    };

    // The GTK loop blocks here, keeping the Tokio runtime alive in the background.
    Backend::new("org.unaos.lumen", app, gui_rx, bootstrap);
}
