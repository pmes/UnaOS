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
use objc2_app_kit::{
    NSSplitViewController, NSSplitViewItem, NSViewController
};
use objc2_foundation::{NSRect, NSSize, MainThreadMarker};
use objc2::{msg_send, ClassType};
use objc2::rc::{Retained, Allocated};

use super::workspace::sidebar;
use super::workspace::comms;

// -----------------------------------------------------------------------------
// MAC OS SPLINE
// -----------------------------------------------------------------------------
pub struct MacOSSpline {
    // Wrap any inner mutable state in an Arc<Mutex> so that the async loops can
    // clone the Arc and move it into the thread without lifetime or borrow checker conflicts.
    inner: Arc<Mutex<MacOSSplineInner>>,
}

struct MacOSSplineInner {
    // Store references to UI components we need to mutate.
    split_view_controller: Option<Retained<NSSplitViewController>>,
    sidebar_delegate: Option<Retained<sidebar::SidebarDelegate>>,
    comms_delegate: Option<Retained<comms::CommsDelegate>>,
}

impl MacOSSpline {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MacOSSplineInner {
                split_view_controller: None,
                sidebar_delegate: None,
                comms_delegate: None,
            })),
        }
    }

    pub fn bootstrap(
        &self,
        _window: &NativeWindow,
        tx_event: async_channel::Sender<Event>,
        _app_state: Arc<RwLock<AppState>>,
        mut rx_synapse: BroadcastReceiver<SMessage>,
        _workspace_tetra: &bandy::state::WorkspaceState,
    ) -> NativeView {
        // 1. Build the UI
        let mtm = MainThreadMarker::new().unwrap();

        // Root NSSplitViewController is the master frame separating Left Pane (Sidebar) and Right Pane (Comms)
        let svc: Allocated<NSSplitViewController> = unsafe { msg_send![NSSplitViewController::class(), alloc] };
        let svc: Retained<NSSplitViewController> = unsafe { msg_send![svc, init] };

        // Ensure split view uses safe constraints
        let split_view = svc.splitView();
        unsafe {
            let _: () = msg_send![&split_view, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
        }

        // --- Lumen Left Pane (Sidebar) ---
        let (sidebar_view, sidebar_delegate) = sidebar::create_sidebar(mtm);
        let sidebar_vc: Allocated<NSViewController> = unsafe { msg_send![NSViewController::class(), alloc] };
        let sidebar_vc: Retained<NSViewController> = unsafe { msg_send![sidebar_vc, init] };
        sidebar_vc.setView(&sidebar_view);

        // Define as a sidebar
        let sidebar_item: Retained<NSSplitViewItem> = unsafe { msg_send![NSSplitViewItem::class(), sidebarWithViewController: &*sidebar_vc] };

        // Enforce the 250px minimum width for the left pane
        unsafe {
            let _: () = msg_send![&sidebar_item, setMinimumThickness: 250.0f64];
        }

        // --- Reactor Right Pane (Comms) ---
        let (comms_view, comms_delegate) = comms::create_comms(mtm);
        let comms_vc: Allocated<NSViewController> = unsafe { msg_send![NSViewController::class(), alloc] };
        let comms_vc: Retained<NSViewController> = unsafe { msg_send![comms_vc, init] };
        comms_vc.setView(&comms_view);

        // Define as main content item
        let comms_item: Retained<NSSplitViewItem> = unsafe { msg_send![NSSplitViewItem::class(), splitViewItemWithViewController: &*comms_vc] };

        // Assemble the split view controller
        svc.addSplitViewItem(&sidebar_item);
        svc.addSplitViewItem(&comms_item);

        // Extract the assembled root view
        let root_view = svc.view();

        // Hold references to prevent deallocation
        if let Ok(mut lock) = self.inner.lock() {
            lock.split_view_controller = Some(svc.clone());
            lock.sidebar_delegate = Some(sidebar_delegate);
            lock.comms_delegate = Some(comms_delegate);
        }

        // 2. Spawn the Main Thread Router
        // Using `dispatch2` for macOS GCD to cross the Tokio async/sync boundary natively
        let spline_inner_arc = self.inner.clone();

        tokio::spawn(async move {
            loop {
                // Keep the compiler happy about the unused tx_event
                let _tx = tx_event.clone();

                match rx_synapse.recv().await {
                    Ok(msg) => {
                        let _inner = spline_inner_arc.clone();
                        // Dispatch to Main Thread to update AppKit UI
                        dispatch2::DispatchQueue::main().exec_async(move || {
                            // SMessage Routing - Logging the pipeline connection on the main thread
                            match msg {
                                SMessage::Matrix(_) => {
                                    log::info!("[MacOSSpline] SMessage::Matrix routed to main thread - Bubble Matrix layout delegated.");
                                },
                                SMessage::NetworkLog(_) => {
                                    log::info!("[MacOSSpline] SMessage::NetworkLog routed to main thread.");
                                },
                                _ => {
                                    log::info!("[MacOSSpline] Generic SMessage routed to main thread.");
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
