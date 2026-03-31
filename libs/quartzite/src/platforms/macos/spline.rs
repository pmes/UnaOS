// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use crate::{NativeView, NativeWindow, Event};
use bandy::{state::AppState, SMessage, MatrixEvent, WorkspaceEvent};
use dispatch2::DispatchQueue;
use objc2_app_kit::{NSView, NSWindow};
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast::Receiver as BroadcastReceiver;
use tokio::task;

// The UI routing spine
pub struct MacOSSpline;

impl MacOSSpline {
    pub fn new() -> Self {
        Self
    }

    pub fn bootstrap(
        &self,
        _window: &NativeWindow,
        tx_event: async_channel::Sender<Event>,
        app_state: Arc<RwLock<AppState>>,
        mut rx_synapse: BroadcastReceiver<SMessage>,
        workspace_tetra: &bandy::state::WorkspaceState,
    ) -> NativeView {

        // Let's create our workspace UI here since this needs to be passed back to the app delegate
        let workspace_view = crate::platforms::macos::workspace::build(
            tx_event.clone(),
            app_state.clone(),
            workspace_tetra
        );

        // Spawn a background Tokio thread to listen for core SMessage events and dispatch to main thread
        task::spawn(async move {
            while let Ok(msg) = rx_synapse.recv().await {
                // To mutate AppKit, we must escape the Tokio thread into Grand Central Dispatch main
                DispatchQueue::main().exec_async(move || {
                    match msg {
                        SMessage::StateInvalidated => {
                            // Core state has mutated. Update entire UI context here.
                            crate::platforms::macos::workspace::handle_state_invalidated(&workspace_view);
                        }
                        SMessage::Matrix(MatrixEvent::TopologyMutated(_nodes)) => {
                            // Update sidebar NSOutlineView here
                            crate::platforms::macos::workspace::handle_topology_mutated(&workspace_view);
                        }
                        SMessage::Workspace(WorkspaceEvent::StreamRenderComplete) => {
                            // Scroll terminal view to bottom
                            crate::platforms::macos::workspace::handle_stream_render(&workspace_view);
                        }
                        _ => {
                            // Ignore other signals
                        }
                    }
                });
            }
        });

        // Notify Core that UI is rendered and ready to receive matrix nodes
        let tx = tx_event.clone();
        task::spawn(async move {
            let _ = tx.send(Event::UiReady).await;
        });

        workspace_view
    }
}
