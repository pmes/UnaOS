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

//! `pulse` — the UnaOS system monitor vessel, spiritual heir of BeOS Pulse.
//!
//! Like every vessel, this binary is wiring and lifecycle only: a Tokio
//! runtime, the Bandy Synapse, and a native Quartzite window. The per-core
//! samples come from a `PulseSource` behind a named seam so a future
//! UnaOS-kernel telemetry feed can replace the host sampler without touching
//! the UI.

use bandy::{telemetry, Synapse};
use gneiss_pal::paths::UnaPaths;

fn main() {
    // 0. Ignite the Substrate Reactor (Tokio)
    let rt = tokio::runtime::Runtime::new().expect("CRITICAL: Failed to ignite Tokio reactor");
    let _guard = rt.enter();

    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    // Signal Interceptor: SIGINT/SIGTERM -> broadcast shutdown so tasks drain cleanly.
    let signal_tx = shutdown_tx.clone();
    rt.spawn(async move {
        let mut sigint =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).unwrap();
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        tokio::select! {
            _ = sigint.recv() => {
                log::info!("[PULSE] :: SIGINT Caught. Initiating Graceful Shutdown...");
                let _ = signal_tx.send(());
            }
            _ = sigterm.recv() => {
                log::info!("[PULSE] :: SIGTERM Caught. Initiating Graceful Shutdown...");
                let _ = signal_tx.send(());
            }
        }
    });

    // 1. Establish Base Camp + Telemetry
    UnaPaths::awaken().expect("CRITICAL: Failed to awaken spatial paths");
    telemetry::ignite(UnaPaths::root().join("logs"));
    log::info!("Pulse Boot Sequence Initiated.");

    // 2. Ignite the Spine
    let synapse = Synapse::new();

    // 3. The Window (macOS AppKit via quartzite; further backends follow quartzite maturity)
    #[cfg(target_os = "macos")]
    {
        // Size the default window to the machine: one meter cell per core, capped sanely.
        // The meter itself derives its layout from the live bounds — this is only a
        // pleasant opening size, not a layout contract.
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let width = (72.0 * cores as f64 + 32.0).clamp(320.0, 1280.0);

        let rx_synapse = synapse.subscribe();
        quartzite::Backend::new_vessel("org.unaos.pulse", "pulse", (width, 96.0), move |window| {
            quartzite::meter::bootstrap_meter(window, rx_synapse)
        })
        .run();
    }

    #[cfg(not(target_os = "macos"))]
    {
        drop(synapse);
        log::error!("pulse: no native backend for this platform yet (macOS first).");
    }

    // Broadcast shutdown in case the GUI exited naturally instead of via SIGINT/SIGTERM.
    let _ = shutdown_tx.send(());
}
