// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use crate::{NativeView, NativeWindow};
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast::Receiver as BroadcastReceiver;

// Import the single source of truth from the nervous system
use bandy::state::AppState;
use bandy::SMessage;

#[derive(Debug, Clone)]
pub enum Event {
    UiReady,
    LoadHistory { offset: usize },
    DispatchPayload(String),
    Input { target: String, content: String, origin: String },
    ComplexInput { target: String, action: String, payload: String, origin: String },
    NavSelect(usize),
    CreateNode { name: String, kind: String },
    ToggleMatrixNode(String),
    FocusMatrixSector(String),
    UpdateMatrixSelection(Vec<String>),
}

#[cfg(target_os = "macos")]
use objc2::rc::Retained;

/// The macOS bootstrap payload. Element order: root view, the sidebar delegate,
/// then the two mutually-exclusive right-pane delegates — exactly one of
/// `comms`/`editor` is `Some`, selected by the workspace's `right_pane`
/// (`ViewEntity::Stream` → comms, `ViewEntity::Editor` → editor) — and finally
/// the optional bottom-pane `console` delegate, `Some` when the workspace's
/// `bottom_pane` is set. Callers that don't inspect the payload (e.g. lumen)
/// are unaffected by the widening.
#[cfg(target_os = "macos")]
pub type BootstrapPayload = (
    NativeView,
    Retained<crate::platforms::macos::workspace::sidebar::SidebarDelegate>,
    Option<Retained<crate::platforms::macos::workspace::comms::CommsDelegate>>,
    Option<Retained<crate::platforms::macos::workspace::editor::EditorDelegate>>,
    Option<Retained<crate::platforms::macos::workspace::console::ConsoleDelegate>>,
);

#[cfg(not(target_os = "macos"))]
pub type BootstrapPayload = NativeView;

#[cfg(all(target_os = "linux", feature = "gtk"))]
// use crate::platforms::gtk::spline::CommsSpline;

#[cfg(target_os = "macos")]
// use crate::platforms::macos::spline::MacOSSpline;

/// The platform-neutral entry point to Quartzite's GUI. [`Spline::bootstrap`] is the stable seam
/// between a workspace snapshot and native rendering; it dispatches to the compile-time-selected
/// backend under `platforms/`.
pub struct Spline {
    // #[cfg(all(target_os = "linux", feature = "gtk"))]
    // inner: CommsSpline,

    // #[cfg(target_os = "macos")]
    // inner: MacOSSpline,
}

impl Spline {
    pub fn new() -> Self {
        #[cfg(all(target_os = "linux", feature = "gtk"))]
        return Self {
            // inner: CommsSpline::new(),
        };

        #[cfg(target_os = "macos")]
        return Self {
            // inner: MacOSSpline::new(),
        };

        // For the Qt platform, Spline is entirely stateless.
        // The event loop is handled by CXX-Qt and our global channel hooks in window.rs.
        #[cfg(not(any(all(target_os = "linux", feature = "gtk"), target_os = "macos")))]
        return Self {};
    }

    /// The platform seam: render a workspace snapshot (`bandy::state::WorkspaceState`) into a
    /// native view tree, wire it to the message bus, and return the platform's
    /// [`BootstrapPayload`]. Each backend under `platforms/` implements this for its host toolkit
    /// (macOS AppKit, GTK, Qt). This is the single point a future native `platforms/unaos` backend
    /// would target — rendering a workspace directly onto the kernel framebuffer
    /// (`Screen` / `FrameBuffer`) with USB HID input — without changing any caller.
    pub fn bootstrap(
        &self,
        _window: &NativeWindow,
        _tx_event: async_channel::Sender<SMessage>,
        _app_state: Arc<RwLock<AppState>>,
        _rx_synapse: BroadcastReceiver<SMessage>,
        _workspace_tetra: &bandy::state::WorkspaceState,
    ) -> BootstrapPayload {
        #[cfg(any(all(target_os = "linux", feature = "gtk"), target_os = "macos"))]
        {
            #[cfg(target_os = "macos")]
            unimplemented!("MacOSSpline is disabled for aether-shell browser path");
            
            #[cfg(all(target_os = "linux", feature = "gtk"))]
            return gtk4::Box::new(gtk4::Orientation::Horizontal, 0).into(); // Fallback, GTK workspace is deprecated
        }

        #[cfg(all(target_os = "linux", feature = "qt"))]
        {
            use crate::platforms::qt::ffi;

            // To fulfill the nervous system, we inject the event_tx to the backend.
            let _ = crate::platforms::qt::window::GLOBAL_TX.set(_tx_event);

            // Spawn the tokio backend to listen to StateInvalidated pings from Vein/Cortex
            crate::platforms::qt::window::spawn_state_listener(_app_state, _rx_synapse);

            let tetra = crate::tetra::WorkspaceTetra::from_state(_workspace_tetra);
            let default_tetra = crate::tetra::StreamTetra::default();
            let stream_tetra = match &tetra.right_pane {
                crate::tetra::TetraNode::Stream(t) => t,
                _ => &default_tetra,
            };
            return crate::NativeView {
                ptr: ffi::create_main_window(
                    tetra.split_ratio,
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
