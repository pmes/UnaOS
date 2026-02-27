mod core;
mod cortex;
mod ui;

use crate::ui::telemetry::ContextView;
use bandy::{SMessage, Synapse, telemetry};
use gneiss_pal::paths::UnaPaths;
use quartzite::{self, Backend, NativeWindow, NativeView};
use std::rc::Rc;
#[cfg(target_os = "linux")]
use crate::ui::view::CommsSpline;
use vein::VeinHandler;
use gneiss_pal::GuiUpdate;
use gneiss_pal::AppHandler; // Fix E0599

// Platform-specific imports
#[cfg(target_os = "linux")]
use gtk4::prelude::*;
#[cfg(target_os = "linux")]
use gtk4::{Orientation, Paned};
#[cfg(target_os = "linux")]
use glib::MainContext;

fn main() {
    // 0. Ignite the Substrate Reactor (Tokio)
    let rt = tokio::runtime::Runtime::new().expect("CRITICAL: Failed to ignite Tokio reactor");
    let _guard = rt.enter();

    // 1. Establish Base Camp
    UnaPaths::awaken().expect("CRITICAL: Failed to awaken spatial paths");

    // Deploy assets on Linux only (macOS uses bundle or embedded differently)
    #[cfg(target_os = "linux")]
    let asset_path = UnaPaths::root().join("quartzite.gresource");
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = quartzite::deploy_assets(&asset_path) {
            log::error!("Failed to deploy assets: {}", e);
        }
        quartzite::init_with_path(&asset_path);
    }

    // Split the brain
    let vein_storage = UnaPaths::primary_vault();
    let cortex_vault = UnaPaths::subconscious_vault();

    // 2. Ignite Telemetry
    telemetry::ignite(UnaPaths::root().join("logs"));
    log::info!("Lumen Boot Sequence Initiated.");

    let (telemetry_tx, telemetry_rx) = async_channel::unbounded::<SMessage>();

    // 3. Ignite the Spine
    let synapse = Synapse::new();

    // 4. Initialize Crypto
    let _ = rustls::crypto::ring::default_provider().install_default();

    // 5. Awaken the Autonomous Core
    let core_synapse = synapse.clone();
    rt.spawn(async move {
        core::ignite(cortex_vault, core_synapse).await;
    });

    // 6. Ignite the AI Handler (The Conscious Vein)
    let (gui_tx, gui_rx) = async_channel::unbounded();
    // Channels for UI Events (Spline -> Vein)
    let (event_tx, event_rx) = async_channel::unbounded::<quartzite::Event>();

    // We move VeinHandler into a separate task.
    // Since VeinHandler is "Pure Logic", it should run on Tokio.
    // The `handle_event` method processes events from the UI.

    let vein_handler = VeinHandler::new(gui_tx, vein_storage, synapse.tx(), telemetry_tx);

    // Spawn the Brain Loop
    rt.spawn(async move {
        let mut vein = vein_handler;
        while let Ok(event) = event_rx.recv().await {
            vein.handle_event(event);
        }
    });

    // 7. View & Engine Ignition
    // On Linux, we use CommsSpline. On macOS, we use a placeholder or partial logic.
    #[cfg(target_os = "linux")]
    let spline = Rc::new(CommsSpline::new());

    // THE FUSION
    let bootstrap = move |window: &NativeWindow| -> NativeView {
        // 1. Get the Vein UI (The Command Center)

        #[cfg(target_os = "linux")]
        let vein_widget = spline.bootstrap(window, event_tx.clone(), gui_rx.clone());

        #[cfg(target_os = "macos")]
        let vein_widget = {
            // macOS UI implementation placeholder
            unsafe {
                 // In a real app, we would alloc init an NSView here.
                 // For now, we return a Retained<NSView> using standard alloc/init.
                 use objc2::{msg_send, ClassType};
                 use objc2_app_kit::NSView;
                 use objc2::rc::Retained;

                 let view: Retained<NSView> = msg_send![NSView::class(), new];
                 view
            }
        };

        // 2. Create the HUD (ContextView) - GTK Only for now
        #[cfg(target_os = "linux")]
        {
            let hud = ContextView::new();
            let hud_widget = hud.container.clone();

            let telemetry_rx_clone = telemetry_rx.clone();
            MainContext::default().spawn_local(async move {
                while let Ok(msg) = telemetry_rx_clone.recv().await {
                    if let SMessage::ContextTelemetry { skeletons } = msg {
                        hud.update(skeletons);
                    }
                }
            });

            let root = Paned::new(Orientation::Horizontal);
            root.set_start_child(Some(&vein_widget));
            root.set_end_child(Some(&hud_widget));
            root.set_position(900);
            root.set_wide_handle(true);
            root.set_shrink_end_child(false);

            root.upcast::<gtk4::Widget>()
        }

        #[cfg(target_os = "macos")]
        vein_widget
    };

    Backend::new("org.unaos.lumen", bootstrap).run();
}
