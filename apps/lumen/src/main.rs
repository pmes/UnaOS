// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

mod core;
mod cortex;

#[allow(unused_imports)]
use bandy::{SMessage, Synapse, telemetry};
use gneiss_pal::paths::UnaPaths;
use quartzite::{self, Backend, NativeWindow, NativeView};
use std::rc::Rc;
use vein::VeinHandler;
#[allow(unused_imports)]
use gneiss_pal::GuiUpdate;
use gneiss_pal::AppHandler;

fn main() {
    // 0. Ignite the Substrate Reactor (Tokio)
    let rt = tokio::runtime::Runtime::new().expect("CRITICAL: Failed to ignite Tokio reactor");
    let _guard = rt.enter();

    // 1. Establish Base Camp
    UnaPaths::awaken().expect("CRITICAL: Failed to awaken spatial paths");

    // Split the brain
    let vein_storage = UnaPaths::primary_vault();
    let cortex_vault = UnaPaths::subconscious_vault();

    // 2. Ignite Telemetry
    telemetry::ignite(UnaPaths::root().join("logs"));
    log::info!("Lumen Boot Sequence Initiated.");

    let (telemetry_tx, _telemetry_rx) = async_channel::unbounded::<SMessage>();

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
    let spline = Rc::new(quartzite::Spline::new());

    // THE FUSION
    let bootstrap = move |window: &NativeWindow| -> NativeView {
        // 1. Get the Vein UI (The Command Center)
        let vein_widget = spline.bootstrap(window, event_tx.clone(), gui_rx.clone(), _telemetry_rx.clone());

        // 2. Create the HUD (ContextView) - DEPRECATED (Phase 4)
        // The "TeleHUD" sidebar tab is now the sole authorized telemetry view.
        // We simply return the vein_widget directly.
        vein_widget
    };

    Backend::new("org.unaos.lumen", bootstrap).run();
}
