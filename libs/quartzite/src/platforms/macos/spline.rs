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
        let (sidebar_view, sidebar_delegate) = sidebar::create_sidebar(mtm, _workspace_tetra);
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

        // Wrap delegates in MainThreadBound so they can cross thread boundaries safely.
        // They strictly require `Send` to be moved into tokio::spawn.
        let comms_delegate_bound = Arc::new(dispatch2::MainThreadBound::new(comms_delegate.clone(), mtm));
        let sidebar_delegate_bound = Arc::new(dispatch2::MainThreadBound::new(sidebar_delegate.clone(), mtm));

        tokio::spawn(async move {
            loop {
                // Keep the compiler happy about the unused tx_event
                let _tx = tx_event.clone();

                match rx_synapse.recv().await {
                    Ok(msg) => {
                        let _inner = spline_inner_arc.clone();
                        let comms_bound = comms_delegate_bound.clone();
                        let sidebar_bound = sidebar_delegate_bound.clone();

                        match msg {
                            SMessage::StorageLoadPagedResult { records, .. } => {
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    let mtm = objc2_foundation::MainThreadMarker::new().unwrap();
                                    let comms_delegate = comms_bound.get(mtm);
                                    use objc2::DefinedClass;

                                    // Wrap the mutable borrow in a block so it drops when done
                                    if let Some(chat_manager) = comms_delegate.ivars().chat_manager.borrow().as_ref() {
                                        {
                                            let mut history = chat_manager.ivars().history.borrow_mut();
                                            for record in records {
                                                let is_chat = record.is_chat;
                                                if is_chat {
                                                    history.push(bandy::state::HistoryItem {
                                                        sender: record.sender.clone(),
                                                        content: record.content.clone(),
                                                        timestamp: record.timestamp.clone(),
                                                        is_chat,
                                                    });
                                                }
                                            }
                                        }

                                        if let Some(table_view) = chat_manager.ivars().table_view.borrow().as_ref() {
                                            unsafe {
                                                let _: () = objc2::msg_send![&**table_view, reloadData];
                                            }
                                        }
                                    }
                                });
                            },
                            SMessage::AiToken(token_string) => {
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    let mtm = objc2_foundation::MainThreadMarker::new().unwrap();
                                    let comms_delegate = comms_bound.get(mtm);
                                    use objc2::DefinedClass;

                                    if let Some(chat_manager) = comms_delegate.ivars().chat_manager.borrow().as_ref() {
                                        let mut history = chat_manager.ivars().history.borrow_mut();

                                        // Append the chunk to the state so history is accurate
                                        if let Some(last_item) = history.last_mut() {
                                            // The token directly appends to the last item.
                                            // We no longer rely on UI-side string checks ("Lumen"),
                                            // as AiTokens naturally follow AiMessage beginnings.
                                            last_item.content.push_str(&token_string);

                                            // Directly append string to TextKit NSTextStorage without reloading the table cell!
                                            comms_delegate.append_stream_token(&token_string);
                                        }
                                    }
                                });
                            },
                            SMessage::NetworkLog(_) => {
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    // SMessage::NetworkLog routed to main thread.
                                    log::info!("[MacOSSpline] SMessage::NetworkLog routed to main thread.");
                                });
                            },
                            SMessage::Matrix(matrix_event) => {
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    let mtm = objc2_foundation::MainThreadMarker::new().unwrap();
                                    let sidebar_delegate = sidebar_bound.get(mtm);

                                    match matrix_event {
                                        bandy::MatrixEvent::TopologyMutated(flat_tree) => {
                                            use std::collections::HashMap;
                                            use bandy::state::TopologyNode;

                                            // Reconstruct tree from flat list
                                            let _nodes_by_depth: HashMap<usize, Vec<TopologyNode>> = HashMap::new();
                                            let mut root_nodes = Vec::new();

                                            // Note: In a real implementation this reconstruction logic would be robust.
                                            // Since we only have a flat representation here, we rebuild a simple list
                                            // or correctly parsed tree if depth info is available. For demonstration,
                                            // we will just populate the roots.

                                            for (id, label, depth) in flat_tree {
                                                let node = TopologyNode {
                                                    id,
                                                    label,
                                                    children: Vec::new(),
                                                    is_expanded: false,
                                                };
                                                if depth == 0 {
                                                    root_nodes.push(node);
                                                } else {
                                                    // Simple flat fallback for non-roots
                                                    root_nodes.push(node);
                                                }
                                            }

                                            use crate::platforms::macos::workspace::sidebar::UnaMatrixNode;
                                            use objc2::DefinedClass;

                                            let mut new_roots = Vec::new();
                                            for root in &root_nodes {
                                                new_roots.push(UnaMatrixNode::build_from(root));
                                            }

                                            *sidebar_delegate.ivars().roots.borrow_mut() = new_roots;

                                            if let Some(outline_view) = sidebar_delegate.ivars().outline_view.borrow().as_ref() {
                                                unsafe {
                                                    let _: () = objc2::msg_send![&**outline_view, reloadData];
                                                }
                                            }
                                        }
                                        _ => {}
                                    }

                                    log::info!("[MacOSSpline] SMessage::Matrix routed to main thread.");
                                });
                            },
                            _ => {}
                        }
                    }
                    Err(_) => break, // Channel closed or lagged
                }
            }
        });

        (root_view, sidebar_delegate, comms_delegate)
    }
}
