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
use objc2_foundation::MainThreadMarker;
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
    // Placeholder for thread-safe (Send/Sync) state. AppKit UI components MUST NOT
    // be stored here because they cross the tokio async thread boundary.
}

impl MacOSSpline {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MacOSSplineInner {})),
        }
    }

    pub fn bootstrap(
        &self,
        _window: &NativeWindow,
        tx_event: async_channel::Sender<Event>,
        _app_state: Arc<RwLock<AppState>>,
        mut rx_synapse: BroadcastReceiver<SMessage>,
        _workspace_tetra: &bandy::state::WorkspaceState,
    ) -> (
        NativeView,
        Retained<sidebar::SidebarDelegate>,
        Retained<comms::CommsDelegate>,
    ) {
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
        let (comms_view, comms_delegate) = comms::create_comms(mtm, &_app_state);
        let comms_vc: Allocated<NSViewController> = unsafe { msg_send![NSViewController::class(), alloc] };
        let comms_vc: Retained<NSViewController> = unsafe { msg_send![comms_vc, init] };
        comms_vc.setView(&comms_view);

        // Define as main content item
        let comms_item: Retained<NSSplitViewItem> = unsafe { msg_send![NSSplitViewItem::class(), splitViewItemWithViewController: &*comms_vc] };

        // Assemble the split view controller
        svc.addSplitViewItem(&sidebar_item);
        svc.addSplitViewItem(&comms_item);

        // Prevent AppKit components from deallocation by attaching them to the root Window/run loop.
        // Anchor split_view_controller
        _window.setContentViewController(Some(&svc));

        // Extract the assembled root view
        let root_view = svc.view();

        // 2. Spawn the Main Thread Router
        // Using `dispatch2` for macOS GCD to cross the Tokio async/sync boundary natively
        let spline_inner_arc = self.inner.clone();

        // Wrap comms_delegate in MainThreadBound so it can cross thread boundaries safely.
        // It strictly requires `Send` to be moved into tokio::spawn.
        let comms_delegate_bound = Arc::new(dispatch2::MainThreadBound::new(comms_delegate.clone(), mtm));

        // Get initial history seq to calculate delta
        let mut last_history_seq = {
            let st = _app_state.read().unwrap();
            st.history_seq
        };
        let app_state_clone = _app_state.clone();

        tokio::spawn(async move {
            loop {
                // Keep the compiler happy about the unused tx_event
                let _tx = tx_event.clone();

                match rx_synapse.recv().await {
                    Ok(msg) => {
                        let _inner = spline_inner_arc.clone();
                        let comms_bound = comms_delegate_bound.clone();

                        match msg {
                            SMessage::StateInvalidated => {
                                // Extract the history delta from AppState safely
                                let new_history_seq;
                                let mut history_delta = Vec::new();

                                {
                                    let st = app_state_clone.read().unwrap();
                                    new_history_seq = st.history_seq;

                                    // Handle full state rollbacks/clears gracefully
                                    if new_history_seq < last_history_seq {
                                        last_history_seq = 0;
                                    }

                                    let h_delta_count = st.history_seq.saturating_sub(last_history_seq);
                                    if h_delta_count > 0 {
                                        let delta_items = if h_delta_count >= st.history.len() {
                                            st.history.iter().cloned().collect::<Vec<_>>()
                                        } else {
                                            st.history.iter().skip(st.history.len() - h_delta_count).cloned().collect::<Vec<_>>()
                                        };

                                        // Filter only chat items to render inside Comms
                                        history_delta = delta_items.into_iter().filter(|item| item.is_chat).collect();
                                    }
                                } // Drop AppState read lock BEFORE hopping to the Main Thread

                                last_history_seq = new_history_seq;
                                if !history_delta.is_empty() {

                                    dispatch2::DispatchQueue::main().exec_async(move || {
                                        let mtm = MainThreadMarker::new().unwrap();
                                        let comms_delegate = comms_bound.get(mtm);
                                        use objc2::DefinedClass;

                                        if let (Some(doc_view), Some(stack_view)) = (
                                            comms_delegate.ivars().doc_view.borrow().as_ref(),
                                            comms_delegate.ivars().stack_view.borrow().as_ref()
                                        ) {
                                            for item in history_delta {
                                                let is_user = item.sender == "Architect";
                                                let _ = comms::append_bubble(doc_view, stack_view, &item.content, is_user);
                                            }
                                        }
                                    });
                                }
                            },
                            SMessage::NetworkLog(_) => {
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    log::info!("[MacOSSpline] SMessage::NetworkLog routed to main thread.");
                                });
                            },
                            SMessage::Matrix(_) => {
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    log::info!("[MacOSSpline] SMessage::Matrix routed to main thread.");
                                });
                            },
                            _ => {
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    log::info!("[MacOSSpline] Generic SMessage routed to main thread.");
                                });
                            }
                        }
                    }
                    Err(_) => break, // Channel closed or lagged
                }
            }
        });

        (root_view, sidebar_delegate, comms_delegate)
    }
}
