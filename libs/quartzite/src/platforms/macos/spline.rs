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
    NSSplitViewController, NSSplitViewItem, NSViewController, NSSplitView, NSView,
};
use objc2_foundation::MainThreadMarker;
use objc2::{msg_send, ClassType, DefinedClass};
use dispatch2::MainThreadBound;
use objc2::rc::{Retained, Allocated};

use super::workspace::sidebar;
use super::workspace::comms;
use super::workspace::editor;
use super::workspace::console;

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
        Option<Retained<comms::CommsDelegate>>,
        Option<Retained<editor::EditorDelegate>>,
        Option<Retained<console::ConsoleDelegate>>,
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
        let (sidebar_view, sidebar_delegate) =
            sidebar::create_sidebar(mtm, _workspace_tetra, tx_event.clone());
        let sidebar_vc: Allocated<NSViewController> = unsafe { msg_send![NSViewController::class(), alloc] };
        let sidebar_vc: Retained<NSViewController> = unsafe { msg_send![sidebar_vc, init] };
        sidebar_vc.setView(&sidebar_view);

        // Define as a sidebar
        let sidebar_item: Retained<NSSplitViewItem> = unsafe { msg_send![NSSplitViewItem::class(), sidebarWithViewController: &*sidebar_vc] };

        // Enforce the 250px minimum width for the left pane
        unsafe {
            let _: () = msg_send![&sidebar_item, setMinimumThickness: 250.0f64];
        }

        // --- Right Pane: Editor (Code layout) or Comms (Comms layout) ---
        // The workspace's right pane selects which delegate we build; exactly one
        // of `comms_delegate`/`editor_delegate` ends up `Some`. The editor gets
        // the UI → brain sender so it can emit EditorEdited/EditorSaveRequest.
        let (right_pane_view, comms_delegate, editor_delegate) = match &_workspace_tetra.right_pane {
            bandy::state::ViewEntity::Editor(editor_state) => {
                let (editor_view, editor_delegate) = editor::create_editor(mtm, editor_state, tx_event.clone());
                (editor_view, None, Some(editor_delegate))
            }
            _ => {
                let (comms_view, comms_delegate) = comms::create_comms(mtm, &_app_state);
                (comms_view, Some(comms_delegate), None)
            }
        };

        // --- Optional Bottom Pane: Console ---
        // When the workspace requests a bottom pane, build the console and stack
        // it *under* the right pane in a vertical split (horizontal divider). The
        // console's input field gets its own clone of the UI → brain sender.
        let (right_view, console_delegate): (Retained<NSView>, Option<Retained<console::ConsoleDelegate>>) =
            if _workspace_tetra.bottom_pane.is_some() {
                let (console_view, console_delegate) = console::create_console(mtm, tx_event.clone());

                let stack: Allocated<NSSplitView> = unsafe { msg_send![NSSplitView::class(), alloc] };
                let stack: Retained<NSSplitView> = unsafe { msg_send![stack, init] };
                stack.setVertical(false); // horizontal divider → panes stack vertically
                unsafe {
                    let _: () = msg_send![&stack, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
                }
                stack.addSubview(&right_pane_view);
                stack.addSubview(&console_view);
                unsafe {
                    // Editor/comms takes the growth; console holds its size.
                    let _: () = msg_send![&stack, setHoldingPriority: 250.0f32, forSubviewAtIndex: 0isize];
                    let _: () = msg_send![&stack, setHoldingPriority: 750.0f32, forSubviewAtIndex: 1isize];
                }
                let stack_view = unsafe { Retained::cast_unchecked::<NSView>(stack) };
                (stack_view, Some(console_delegate))
            } else {
                (right_pane_view, None)
            };

        let right_vc: Allocated<NSViewController> = unsafe { msg_send![NSViewController::class(), alloc] };
        let right_vc: Retained<NSViewController> = unsafe { msg_send![right_vc, init] };
        right_vc.setView(&right_view);

        // Define as main content item
        let right_item: Retained<NSSplitViewItem> = unsafe { msg_send![NSSplitViewItem::class(), splitViewItemWithViewController: &*right_vc] };

        // Assemble the split view controller
        svc.addSplitViewItem(&sidebar_item);
        svc.addSplitViewItem(&right_item);

        // Prevent AppKit components from deallocation by attaching them to the root Window/run loop.
        // Anchor split_view_controller
        _window.setContentViewController(Some(&svc));

        // Extract the assembled root view
        let root_view = svc.view();

        // 2. Spawn the main-thread router.
        //
        // The AppKit delegates are `!Send`, so the background tokio task cannot capture them
        // directly. `MainThreadBound` is the idiomatic bridge: it is `Send + Sync` but only
        // yields the delegate back on the main thread — which is exactly where the `dispatch2`
        // main-queue closures below run their UI work. This replaces the previous
        // `Retained::into_raw` → `usize` → `Retained::retain`/`cast_unchecked` round-trip
        // (which leaked a retain and reconstituted the object from a raw pointer each message).
        // The comms/editor delegates are mutually exclusive (one is `None`); bind
        // whichever exists so the router can address it from the tokio task.
        let comms_bound = comms_delegate
            .as_ref()
            .map(|d| Arc::new(MainThreadBound::new(d.clone(), mtm)));
        let editor_bound = editor_delegate
            .as_ref()
            .map(|d| Arc::new(MainThreadBound::new(d.clone(), mtm)));
        let console_bound = console_delegate
            .as_ref()
            .map(|d| Arc::new(MainThreadBound::new(d.clone(), mtm)));
        let sidebar_bound = Arc::new(MainThreadBound::new(sidebar_delegate.clone(), mtm));

        tokio::spawn(async move {
            // Keep `tx_event` owned by the task even though the current routes don't send on it.
            let _tx_event = tx_event;
            loop {
                match rx_synapse.recv().await {
                    Ok(msg) => {
                        match msg {
                            SMessage::StorageLoadPagedResult { records, .. } => {
                                let Some(comms_bound) = comms_bound.clone() else { continue };
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    let mtm = MainThreadMarker::new().unwrap();
                                    let comms_delegate = comms_bound.get(mtm);

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
                                let Some(comms_bound) = comms_bound.clone() else { continue };
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    let mtm = MainThreadMarker::new().unwrap();
                                    let comms_delegate = comms_bound.get(mtm);

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
                            SMessage::EditorLoad { content, .. } => {
                                let Some(editor_bound) = editor_bound.clone() else { continue };
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    let mtm = MainThreadMarker::new().unwrap();
                                    let editor_delegate = editor_bound.get(mtm);
                                    editor_delegate.set_content(&content);
                                });
                            },
                            SMessage::ConsoleAppend(line) => {
                                let Some(console_bound) = console_bound.clone() else { continue };
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    let mtm = MainThreadMarker::new().unwrap();
                                    let console_delegate = console_bound.get(mtm);
                                    console_delegate.append_line(&line);
                                });
                            },
                            SMessage::NetworkLog(_) => {
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    // SMessage::NetworkLog routed to main thread.
                                    log::info!("[MacOSSpline] SMessage::NetworkLog routed to main thread.");
                                });
                            },
                            SMessage::Matrix(matrix_event) => {
                                let sidebar_bound = sidebar_bound.clone();
                                dispatch2::DispatchQueue::main().exec_async(move || {
                                    let mtm = MainThreadMarker::new().unwrap();
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

        (root_view, sidebar_delegate, comms_delegate, editor_delegate, console_delegate)
    }
}
