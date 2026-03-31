// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use crate::{NativeView, NativeWindow};
use gneiss_pal::Event;
use std::sync::{Arc, RwLock, Mutex};
use tokio::sync::broadcast::Receiver as BroadcastReceiver;
use bandy::state::AppState;
use bandy::SMessage;
use objc2_app_kit::{NSView, NSWindow, NSColor};
use objc2_foundation::{NSRect, NSSize};
use objc2::{msg_send, ClassType};
use objc2::rc::Retained;

// -----------------------------------------------------------------------------
// MAC OS SPLINE
// -----------------------------------------------------------------------------
pub struct MacOSSpline {
    // Wrap any inner mutable state in an Arc<Mutex> so that the async loops can
    // clone the Arc and move it into the thread without lifetime or borrow checker conflicts.
    inner: Arc<Mutex<MacOSSplineInner>>,
}

struct MacOSSplineInner {
    // Future stub: Store references to UI components we need to mutate.
    // e.g., text views, scroll views, sidebars.
}

impl MacOSSpline {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MacOSSplineInner {})),
        }
    }

    pub fn bootstrap(
        &self,
        window: &NativeWindow,
        tx_event: async_channel::Sender<Event>,
        app_state: Arc<RwLock<AppState>>,
        mut rx_synapse: BroadcastReceiver<SMessage>,
        workspace_tetra: &bandy::state::WorkspaceState,
    ) -> NativeView {
        // 1. Build the UI
        let mtm = objc2_foundation::MainThreadMarker::new().unwrap();

        let root_frame = NSRect::new(
            objc2_foundation::NSPoint::new(0.0, 0.0),
            NSSize::new(1024.0, 768.0),
        );
        let root_view = unsafe {
            let view: objc2::rc::Allocated<NSView> = msg_send![NSView::class(), alloc];
            let view: Retained<NSView> = msg_send![view, initWithFrame: root_frame];
            view
        };
        // Explicitly set autoresizing masks to false per UnaOS guidelines for the root layout
        unsafe {
            let _: () = msg_send![&root_view, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
        }

        // 2. Spawn the Main Thread Router
        // Using `dispatch2` for macOS GCD to cross the Tokio async/sync boundary natively
        // without introducing memory leaks like `block2` does.
        let spline_inner_arc = self.inner.clone();

        tokio::spawn(async move {
            loop {
                // Keep the compiler happy about the unused spline_inner_arc
                let _inner = spline_inner_arc.clone();
                let _tx = tx_event.clone();

                match rx_synapse.recv().await {
                    Ok(msg) => {
                        // Dispatch to Main Thread to update AppKit UI
                        dispatch2::Queue::main().exec_async(move || {
                            // Route the SMessage to native AppKit updates
                            // e.g., insertRowsAtIndexes:, NSTextView::setString:
                            match msg {
                                SMessage::Matrix(_m) => {
                                    // Handle matrix changes
                                },
                                SMessage::NetworkLog(_log) => {
                                    // Update network logs
                                },
                                _ => {
                                    // Handle other SMessages
                                }
                            }
                        });
                    }
                    Err(_) => break, // Channel closed or lagged
                }
            }
        });

        root_view
    }
}
