// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use crate::{NativeView, NativeWindow};
use gneiss_pal::Event;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast::Receiver as BroadcastReceiver;

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
        #[cfg(not(any(all(target_os = "linux", feature = "gtk"), target_os = "macos")))]
        return Self {};
    }

    pub fn bootstrap(
        &self,
        _window: &NativeWindow,
        _tx_event: async_channel::Sender<Event>,
        _app_state: Arc<RwLock<AppState>>,
        _rx_synapse: BroadcastReceiver<SMessage>,
        _workspace_tetra: &bandy::state::WorkspaceState,
    ) -> NativeView {
        #[cfg(any(all(target_os = "linux", feature = "gtk"), target_os = "macos"))]
        return self
            .inner
            .bootstrap(_window, _tx_event, _app_state, _rx_synapse, _workspace_tetra);

        #[cfg(all(target_os = "linux", feature = "qt"))]
        {
            use crate::platforms::qt::ffi;

            // To fulfill the nervous system, we inject the event_tx to the backend.
            let _ = crate::platforms::qt::window::GLOBAL_TX.set(_tx_event);

            // Spawn the tokio backend to listen to StateInvalidated pings from Vein/Cortex
            crate::platforms::qt::window::spawn_state_listener(_app_state, _rx_synapse);

            let default_tetra = bandy::state::StreamState::default();
            let stream_tetra = match &_workspace_tetra.right_pane {
                bandy::state::ViewEntity::Stream(tetra) => tetra,
                _ => &default_tetra,
            };
            return crate::NativeView {
                ptr: ffi::create_main_window(
                    _workspace_tetra.split_ratio,
                    stream_tetra.input_anchor.clone() as i32,
                    stream_tetra.scroll_behavior.clone() as i32,
                    stream_tetra.alignment.clone() as i32
                ),
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
