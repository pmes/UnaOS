mod core;
mod cortex;

use bandy::{SMessage, Synapse, telemetry};
use gneiss_pal::paths::UnaPaths;
use gtk4::ApplicationWindow;
use quartzite::{self, Backend};
use std::rc::Rc;
use vein::{CommsSpline, VeinHandler};

fn main() {
    // 0. Ignite the Substrate Reactor (Tokio)
    let rt = tokio::runtime::Runtime::new().expect("CRITICAL: Failed to ignite Tokio reactor");
    let _guard = rt.enter();

    // 1. Establish Base Camp
    UnaPaths::awaken().expect("CRITICAL: Failed to awaken spatial paths");
    let asset_path = UnaPaths::root().join("quartzite.gresource");

    // Split the brain: Conscious (Vein) vs Subconscious (Core)
    let vein_storage = UnaPaths::lumen_storage();
    let cortex_vault = UnaPaths::root().join("cortex.vault");

    // 2. Ignite Telemetry
    telemetry::ignite(UnaPaths::root().join("logs"));
    log::info!("Lumen Boot Sequence Initiated.");

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
    let app = VeinHandler::new(gui_tx, vein_storage, synapse.tx());

    synapse.fire(SMessage::Log {
        level: String::from("INFO"),
        source: String::from("LUMEN"),
        content: String::from("Nervous System Online. Handing control to Quartzite."),
    });

    // 8. View & Engine Ignition
    let spline = Rc::new(CommsSpline::new());
    let bootstrap = move |window: &ApplicationWindow, tx, rx| spline.bootstrap(window, tx, rx);

    // The GTK loop blocks here, keeping the Tokio runtime alive in the background.
    Backend::new("org.unaos.lumen", app, gui_rx, bootstrap);
}
