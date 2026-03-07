// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use crate::{NativeView, NativeWindow};
use gneiss_pal::{Event, GuiUpdate};

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
        // The event loop is handled by CXX-Qt and our global channel hooks in bridge.rs.
        #[cfg(not(any(all(target_os = "linux", feature = "gtk"), target_os = "macos")))]
        return Self {};
    }

    pub fn bootstrap(
        &self,
        _window: &NativeWindow,
        _tx_event: async_channel::Sender<Event>,
        _rx_gui: async_channel::Receiver<GuiUpdate>,
        _rx_telemetry: async_channel::Receiver<bandy::SMessage>,
    ) -> NativeView {
        #[cfg(any(all(target_os = "linux", feature = "gtk"), target_os = "macos"))]
        return self.inner.bootstrap(_window, _tx_event, _rx_gui, _rx_telemetry);

        #[cfg(all(target_os = "linux", feature = "qt"))]
        {
            use crate::platforms::qt::ffi;

            // To fulfill the nervous system, we need to inject the event_tx to the backend.
            let _ = crate::platforms::qt::bridge::GLOBAL_TX.set(_tx_event);

            // Spawn the tokio backend to listen to GUI updates from Vein/Cortex
            // We loop and try to get the GLOBAL_QT_THREAD that QML sets on init
            tokio::spawn(async move {
                while let Ok(_update) = _rx_gui.recv().await {
                    if let Some(qt_thread) = crate::platforms::qt::bridge::GLOBAL_QT_THREAD.get() {
                        let thread = qt_thread.clone();
                        thread.queue(move |_qobj| {
                            // Property mutation goes here
                        }).unwrap();
                    }
                }
            });

            return crate::NativeView {
                ptr: ffi::create_main_window(),
            };
        }

        #[cfg(not(any(all(target_os = "linux", feature = "gtk"), target_os = "macos", all(target_os = "linux", feature = "qt"))))]
        return (); // Fallback
    }
}
