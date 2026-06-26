// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use crate::{NativeView, NativeWindow};
use std::sync::{Arc, RwLock, Mutex};
use tokio::sync::broadcast::Receiver as BroadcastReceiver;
use bandy::state::AppState;
use bandy::SMessage;
use objc2_app_kit::{
    NSSplitViewController, NSSplitViewItem, NSViewController
};
use objc2_foundation::MainThreadMarker;
use objc2::{msg_send, ClassType, DefinedClass};
use objc2::rc::{Retained, Allocated};

use super::workspace::sidebar;
use super::workspace::comms;

// -----------------------------------------------------------------------------
// MAC OS SPLINE
// -----------------------------------------------------------------------------
pub struct MacOSSpline {
    // Wrap any inner mutable state in an Arc<Mutex> so that the async loops can
    // clone the Arc and move it into the thread without lifetime or borrow checker conflicts.
    _inner: Arc<Mutex<MacOSSplineInner>>,
}

struct MacOSSplineInner {
    // Placeholder for thread-safe (Send/Sync) state. AppKit UI components MUST NOT
    // be stored here because they cross the tokio async thread boundary.
}

impl MacOSSpline {
    pub fn new() -> Self {
        Self {
            _inner: Arc::new(Mutex::new(MacOSSplineInner {})),
        }
    }

    pub fn bootstrap(
        &self,
        _window: &NativeWindow,
        tx_event: async_channel::Sender<SMessage>,
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
        // 2. Spawn the Main Thread Router
        // To safely pass AppKit pointers into the background tokio closure, we must marshal them
        // into usize pointers, because AppKit items like `Retained<T>` are `!Send`.
        let comms_delegate_ptr = Retained::into_raw(comms_delegate.clone()) as usize;
        let sidebar_delegate_ptr = Retained::into_raw(sidebar_delegate.clone()) as usize;

        tokio::spawn(async move {
            loop {
                // Keep the compiler happy about the unused tx_event
                let _tx = tx_event.clone();

                match rx_synapse.recv().await {
                    Ok(msg) => {
                        let comms_ptr = comms_delegate_ptr;
                        let sidebar_ptr = sidebar_delegate_ptr;

                        match msg {
                            SMessage::StorageLoadPagedResult { records, .. } => {
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    let comms_delegate = unsafe {
                                        Retained::retain(comms_ptr as *mut objc2::runtime::AnyObject).unwrap()
                                    };
                                    let comms_delegate = unsafe {
                                        Retained::cast_unchecked::<comms::CommsDelegate>(comms_delegate)
                                    };

                                    // Wrap the mutable borrow in a block so it drops when done
                                    if let Some(chat_manager) = comms_delegate.ivars().chat_manager.borrow().as_ref() {
                                        {
                                            let mut history = chat_manager.ivars().history.borrow_mut();
                                            for record in records {
                                                let is_chat = record.is_chat;
                                                if is_chat {
                                                    history.push(bandy::state::HistoryItem {
                                                        origin: record.origin.clone(),
                                                        display_name: record.display_name.clone(),
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
                                    let comms_delegate = unsafe {
                                        Retained::retain(comms_ptr as *mut objc2::runtime::AnyObject).unwrap()
                                    };
                                    let comms_delegate = unsafe {
                                        Retained::cast_unchecked::<comms::CommsDelegate>(comms_delegate)
                                    };

                                    if let Some(chat_manager) = comms_delegate.ivars().chat_manager.borrow().as_ref() {
                                        let mut history = chat_manager.ivars().history.borrow_mut();

                                        // Append the chunk to the state so history is accurate
                                        if let Some(last_item) = history.last_mut() {
                                            // The token directly appends to the last item.
                                            // We no longer rely on UI-side string checks ("Lumen"),
                                            // as AiTokens naturally follow AiMessage beginnings.
                                            last_item.content.push_str(&token_string);

                                            // Directly append string to TextKit NSTextStorage without reloading the table cell!
                                            chat_manager.append_stream_token(&token_string);
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
                                    let sidebar_delegate = unsafe {
                                        Retained::retain(sidebar_ptr as *mut objc2::runtime::AnyObject).unwrap()
                                    };
                                    let sidebar_delegate = unsafe {
                                        Retained::cast_unchecked::<sidebar::SidebarDelegate>(sidebar_delegate)
                                    };

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
