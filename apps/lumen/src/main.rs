mod cortex;

use bandy::{SMessage, Synapse};
use cortex::Cortex;
use gneiss_pal::paths::UnaPaths;
use gtk4::ApplicationWindow;
use quartzite::{self, Backend};
use std::rc::Rc;
use vein::{CommsSpline, VeinHandler};

fn main() {
    // 1. Ignite the Spine
    let mut synapse = Synapse::new();

    // 2. Establish Base Camp
    UnaPaths::awaken().expect("CRITICAL: Failed to awaken spatial paths");
    let asset_path = UnaPaths::root().join("quartzite.gresource");
    let storage_path = UnaPaths::lumen_storage();

    // 3. Initialize Crypto Substrate
    let _ = rustls::crypto::ring::default_provider().install_default();

    // 4. Awaken the Cortex
    let mut brain = Cortex::awaken(&mut synapse);
    brain.imprint(
        "sys.boot.timestamp",
        &chrono::Utc::now().timestamp().to_be_bytes(),
    );

    // 5. FORCE DEPLOY ASSETS (S74)
    if let Err(e) = quartzite::deploy_assets(&asset_path) {
        synapse.fire(SMessage::Log {
            level: String::from("ERROR"),
            source: String::from("LUMEN_UI"),
            content: format!("Failed to deploy assets: {}", e),
        });
    }
    quartzite::init_with_path(&asset_path);

    // 6. Ignite the AI Handler (Vein)
    let (gui_tx, gui_rx) = async_channel::unbounded();
    let app = VeinHandler::new(gui_tx, storage_path, synapse.tx());

    synapse.fire(SMessage::Log {
        level: String::from("INFO"),
        source: String::from("LUMEN"),
        content: String::from("Nervous System Online. Handing control to Quartzite."),
    });

    // 7. View & Engine Ignition
    let spline = Rc::new(CommsSpline::new());
    let bootstrap = move |window: &ApplicationWindow, tx, rx| spline.bootstrap(window, tx, rx);

    Backend::new("org.unaos.lumen", app, gui_rx, bootstrap);
}
