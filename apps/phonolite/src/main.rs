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

//! `phonolite` — the UnaOS tone vessel: resonance given a face.
//!
//! Like every vessel, this binary is wiring and lifecycle only: a Tokio
//! runtime, the Bandy Synapse, and a native Quartzite window. The sound is
//! `libs/resonance` (a sine → gain graph on the default output device); the
//! controls are quartzite's tone panel — the first input-control surface —
//! driving the engine through its `ResonanceHandle`. The level bar rides the
//! bus: a cadence task reads the engine's per-block peak from an atomic and
//! fires `SMessage::Spectrum` beats; the panel repaints on the main thread.
//! Nothing touches the Synapse from the real-time audio callback.

mod tuning;

use bandy::{Synapse, telemetry};
use gneiss_pal::paths::UnaPaths;
use resonance::{AudioEngine, create_test_graph};

/// The level-meter cadence: ~30 Hz — fluid for an LED ladder, far below any
/// rate that would matter to the audio thread (which only writes an atomic).
const LEVEL_BEAT: std::time::Duration = std::time::Duration::from_millis(33);

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
                log::info!("[PHONOLITE] :: SIGINT Caught. Initiating Graceful Shutdown...");
                let _ = signal_tx.send(());
            }
            _ = sigterm.recv() => {
                log::info!("[PHONOLITE] :: SIGTERM Caught. Initiating Graceful Shutdown...");
                let _ = signal_tx.send(());
            }
        }
    });

    // 1. Establish Base Camp + Telemetry
    UnaPaths::awaken().expect("CRITICAL: Failed to awaken spatial paths");
    telemetry::ignite(UnaPaths::root().join("logs"));
    log::info!("Phonolite Boot Sequence Initiated.");

    // 2. Ignite the Spine
    let synapse = Synapse::new();

    // 2.5 The Voice: engine + handle. The engine owns the cpal stream and
    // must stay alive for the whole run (it lives here, on the main thread,
    // across `.run()`); the handle is the control side the panel drives.
    let graph = create_test_graph();
    let (engine, handle) = match AudioEngine::new(graph) {
        Ok(pair) => pair,
        Err(e) => {
            log::error!("phonolite: no audio engine: {e}");
            return;
        }
    };
    log::info!(
        "[PHONOLITE] :: Audio engine live at {} Hz device rate.",
        engine.sample_rate
    );

    // 2.6 The Heartbeat: engine level -> Synapse, at a calm cadence. The
    // audio callback only ever writes an atomic; this task turns it into
    // `SMessage::Spectrum` beats for the panel's level bar.
    let probe = handle.meter();
    let level_synapse = synapse.clone();
    let mut level_shutdown = shutdown_tx.subscribe();
    let level_task = rt.spawn(async move {
        let mut cadence = tokio::time::interval(LEVEL_BEAT);
        cadence.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = level_shutdown.recv() => {
                    log::info!("[PHONOLITE] :: Level beat terminating cleanly.");
                    break;
                }
                _ = cadence.tick() => {
                    level_synapse.fire(tuning::level_beat(probe.level()));
                }
            }
        }
    });

    // 3. The Window (macOS AppKit via quartzite; further backends follow quartzite maturity)
    #[cfg(target_os = "macos")]
    {
        use quartzite::platforms::macos::tone_panel::{
            self, PANEL_SIZE, ToneCallbacks, ToneControlConfig,
        };

        let rx_synapse = synapse.subscribe();
        quartzite::Backend::new_vessel("org.unaos.phonolite", "phonolite", PANEL_SIZE, {
            move |window| {
                // The handle lives on the main thread from here on, shared by
                // the three control callbacks.
                let handle = std::rc::Rc::new(std::cell::RefCell::new(handle));
                let h_toggle = handle.clone();
                let h_freq = handle.clone();
                let h_gain = handle;

                tone_panel::bootstrap_tone_panel(
                    window,
                    ToneControlConfig {
                        freq_min_hz: tuning::FREQ_MIN_HZ,
                        freq_max_hz: tuning::FREQ_MAX_HZ,
                        initial_freq_hz: tuning::INITIAL_FREQ_HZ,
                        gain_max: tuning::GAIN_MAX,
                        initial_gain: tuning::INITIAL_GAIN,
                        initially_running: true,
                    },
                    ToneCallbacks {
                        on_running_changed: Box::new(move |running| {
                            let mut handle = h_toggle.borrow_mut();
                            if running {
                                handle.start();
                            } else if !handle.stop() {
                                log::warn!("[PHONOLITE] :: command queue full (stop)");
                            }
                        }),
                        on_frequency_hz: Box::new(move |hz| {
                            if !h_freq.borrow_mut().set_frequency(hz) {
                                log::warn!("[PHONOLITE] :: command queue full (frequency)");
                            }
                        }),
                        on_gain: Box::new(move |gain| {
                            if !h_gain
                                .borrow_mut()
                                .set_param(tuning::GAIN_NODE, tuning::GAIN_PARAM, gain)
                            {
                                log::warn!("[PHONOLITE] :: command queue full (gain)");
                            }
                        }),
                    },
                    rx_synapse,
                )
            }
        })
        .run();
    }

    #[cfg(not(target_os = "macos"))]
    {
        drop(synapse);
        drop(handle);
        log::error!("phonolite: no native backend for this platform yet (macOS first).");
    }

    // Broadcast shutdown in case the GUI exited naturally instead of via SIGINT/SIGTERM.
    let _ = shutdown_tx.send(());
    rt.block_on(async {
        let _ = level_task.await;
    });

    // The engine (and its cpal stream) lives to the very end.
    drop(engine);
}
