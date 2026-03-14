import re

# `libs/quartzite/src/spline.rs` needs to match the user's explicit provided block.
with open('libs/quartzite/src/spline.rs', 'r') as f:
    code = f.read()

# I'm going to literally replace the entire file with the one the user provided.
user_file_spline = """// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use crate::{NativeView, NativeWindow};
use gneiss_pal::Event;
use std::sync::{Arc, RwLock};

// Import the single source of truth from the nervous system
use bandy::state::AppState;
use bandy::SMessage;

#[cfg(all(target_os = "linux", feature = "gtk"))]
use crate::platforms::gtk::spline::CommsSpline;

#[cfg(target_os = "macos")]
use crate::platforms::macos::spline::MacOSSpline;

pub struct Spline {
    #[cfg(all(target_os = "linux", feature = "gtk"))]
    inner: CommsSpline,

    #[cfg(target_os = "macos")]
    inner: MacOSSpline,
}

impl Spline {
    pub fn new() -> Self {
        #[cfg(all(target_os = "linux", feature = "gtk"))]
        return Self {
            inner: CommsSpline::new(),
        };

        #[cfg(target_os = "macos")]
        return Self {
            inner: MacOSSpline::new(),
        };

        // For the Qt platform, Spline is entirely stateless.
        // The event loop is handled by CXX-Qt and our global channel hooks in window.rs.
        #[cfg(not(any(all(target_os = "linux", feature = "gtk"), target_os = "macos"))))]
        return Self {};
    }

    pub fn bootstrap(
        &self,
        _window: &NativeWindow,
        _tx_event: async_channel::Sender<Event>,
        _app_state: Arc<RwLock<AppState>>,
        _rx_synapse: async_channel::Receiver<SMessage>,
    ) -> NativeView {
        #[cfg(any(all(target_os = "linux", feature = "gtk"), target_os = "macos"))]
        return self
            .inner
            .bootstrap(_window, _tx_event, _app_state, _rx_synapse);

        #[cfg(all(target_os = "linux", feature = "qt"))]
        {
            use crate::platforms::qt::ffi;

            // To fulfill the nervous system, we inject the event_tx to the backend.
            let _ = crate::platforms::qt::window::GLOBAL_TX.set(_tx_event);

            // Spawn the tokio backend to listen to StateInvalidated pings from Vein/Cortex
            crate::platforms::qt::window::spawn_state_listener(_app_state, _rx_synapse);

            return crate::NativeView {
                ptr: ffi::create_main_window(),
            };
        }

        #[cfg(not(any(
            all(target_os = "linux", feature = "gtk"),
            target_os = "macos",
            all(target_os = "linux", feature = "qt")
        )))]
        return (); // Fallback
    }
}
"""

with open('libs/quartzite/src/spline.rs', 'w') as f:
    f.write(user_file_spline)

# Fix the extra bracket the user provided by mistake in their snippet: `macos"))))]`
with open('libs/quartzite/src/spline.rs', 'r') as f:
    code = f.read()
code = code.replace('macos"))))]', 'macos")))]')
with open('libs/quartzite/src/spline.rs', 'w') as f:
    f.write(code)


with open('libs/quartzite/src/platforms/gtk/spline.rs', 'r') as f:
    code = f.read()

code = code.replace(
    'use gneiss_pal::{GuiUpdate, WolfpackState};',
    'use crate::platforms::gtk::types::GuiUpdate;\nuse bandy::state::{WolfpackState, PreFlightPayload, AppState, HistoryItem, ShardStatus};'
)
code = code.replace('gneiss_pal::PreFlightPayload', 'PreFlightPayload')
code = code.replace('gneiss_pal::HistoryItem', 'HistoryItem')
code = code.replace('gneiss_pal::ShardStatus', 'ShardStatus')

# Clear old ShardStatus imports
code = code.replace('use gneiss_pal::shard::ShardStatus;\n', '')
code = re.sub(r'(?:bandy::state::)+ShardStatus::', 'bandy::state::ShardStatus::', code)

code = code.replace(
    'pub fn bootstrap(\n        &self,\n        window: &ApplicationWindow,\n        tx_event: async_channel::Sender<Event>,\n        rx: Receiver<GuiUpdate>,\n        rx_telemetry: Receiver<bandy::SMessage>,\n    ) -> gtk4::Widget {',
    'pub fn bootstrap(\n        &self,\n        window: &ApplicationWindow,\n        tx_event: async_channel::Sender<Event>,\n        app_state: std::sync::Arc<std::sync::RwLock<AppState>>,\n        rx_synapse: async_channel::Receiver<bandy::SMessage>,\n    ) -> gtk4::Widget {'
)

code = code.replace(
    'return build_gtk_ui(window, tx_event, rx, rx_telemetry);',
    'return build_gtk_ui(window, tx_event, app_state, rx_synapse);'
)

code = code.replace(
    'fn build_gtk_ui(\n    window: &ApplicationWindow,\n    tx_event: async_channel::Sender<Event>,\n    rx: Receiver<GuiUpdate>,\n    rx_telemetry: Receiver<bandy::SMessage>,\n) -> gtk4::Widget {',
    'fn build_gtk_ui(\n    window: &ApplicationWindow,\n    tx_event: async_channel::Sender<Event>,\n    app_state: std::sync::Arc<std::sync::RwLock<AppState>>,\n    rx_synapse: async_channel::Receiver<bandy::SMessage>,\n) -> gtk4::Widget {'
)


translator_loop = """
    let (tx_gui, mut rx) = async_channel::unbounded::<GuiUpdate>();
    let rx_glib = rx.clone();

    let rx_synapse_clone = rx_synapse.clone();
    let app_state_clone = app_state.clone();

    tokio::spawn(async move {
        while let Ok(msg) = rx_synapse_clone.recv().await {
            if matches!(msg, bandy::SMessage::StateInvalidated) {
                let (history, logs, payload, tokens, sidebar, active_dir, synapse_err, shards) = {
                    let st = app_state_clone.read().unwrap();
                    (
                        st.history.clone(),
                        st.console_logs.clone(),
                        st.review_payload.clone(),
                        st.token_usage.clone(),
                        st.sidebar_status.clone(),
                        st.active_directive.clone(),
                        st.synapse_error.clone(),
                        st.shard_statuses.clone()
                    )
                };

                let _ = tx_gui.send(GuiUpdate::HistoryBatch(history)).await;
                if let Some(log) = logs.last() {
                    let _ = tx_gui.send(GuiUpdate::ConsoleLog(log.clone())).await;
                }
                if let Some(p) = payload {
                    let _ = tx_gui.send(GuiUpdate::ReviewPayload(p)).await;
                }
                let _ = tx_gui.send(GuiUpdate::TokenUsage(tokens.0, tokens.1, tokens.2)).await;
                let _ = tx_gui.send(GuiUpdate::SidebarStatus(sidebar)).await;
                if !active_dir.is_empty() {
                    let _ = tx_gui.send(GuiUpdate::ActiveDirective(active_dir)).await;
                }
                if let Some(err) = synapse_err {
                    let _ = tx_gui.send(GuiUpdate::SynapseError(err)).await;
                }

                for (id, status) in shards {
                    let _ = tx_gui.send(GuiUpdate::ShardStatusChanged { id, status }).await;
                }
            }
        }
    });

    let mut rx_glib = rx_glib;
"""

code = code.replace('let mut rx_glib = rx;', translator_loop)

# Fix remaining reference to `rx_telemetry` further down in the GUI code
code = code.replace('rx_telemetry', 'rx_synapse')

with open('libs/quartzite/src/platforms/gtk/spline.rs', 'w') as f:
    f.write(code)
