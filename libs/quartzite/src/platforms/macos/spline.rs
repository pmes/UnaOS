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
        let (workspace_view, sidebar_delegate, comms_delegate) = crate::platforms::macos::workspace::build(
            tx_event.clone(),
            app_state.clone(),
            workspace_tetra
        );

        // Store references in AppState or locally bound to the window for future routing.
        // For strict AppKit, we pass these strongly retained objects up to the AppDelegate through thread locals
        // to prevent deallocation.
        crate::platforms::macos::DELEGATES.with(|d| {
            let mut del_store = d.borrow_mut();
            *del_store = Some((sidebar_delegate.clone(), comms_delegate.clone()));
        });

        let view_ptr = workspace_view.clone();

        // Spawn a background OS thread to listen for core SMessage events and dispatch to main thread.
        // We cannot use Tokio here as the AppKit `run` loop has no async context.
        std::thread::spawn(move || {
            while let Ok(msg) = rx_synapse.blocking_recv() {
                let v = view_ptr.clone();
                // To mutate AppKit, we must escape the Tokio thread into Grand Central Dispatch main
                DispatchQueue::main().exec_async(move || {
                    match msg {
                        SMessage::StateInvalidated => {
                            crate::platforms::macos::workspace::handle_state_invalidated(&v);
                        }
                        SMessage::Matrix(MatrixEvent::TopologyMutated(_nodes)) => {
                            crate::platforms::macos::workspace::handle_topology_mutated(&v);
                        }
                        SMessage::Workspace(WorkspaceEvent::StreamRenderComplete) => {
                            crate::platforms::macos::workspace::handle_stream_render(&v);
                        }
                        _ => {}
                    }
                });
            }
        });

        // Notify Core that UI is rendered and ready to receive matrix nodes
        let tx = tx_event.clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(Event::UiReady);
        });

        workspace_view
    }
}
