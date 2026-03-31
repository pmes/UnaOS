// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

pub mod sidebar;
pub mod comms;

use std::sync::{Arc, RwLock};

use objc2::{
    msg_send,
    ClassType,
    Allocated,
    rc::Retained,
};
use objc2_foundation::{NSRect, NSPoint, NSSize, NSArray};
use objc2_app_kit::{
    NSWindow,
    NSView,
    NSSplitViewController,
    NSSplitViewItem,
    NSViewController,
};

use async_channel;
use tokio::sync::broadcast::Receiver as BroadcastReceiver;

use bandy::state::AppState;
use bandy::SMessage;
use bandy::state::WorkspaceState;

pub fn build_workspace(
    _window: &NSWindow,
    tx_event: async_channel::Sender<crate::Event>,
    _app_state: Arc<RwLock<AppState>>,
    rx_synapse: BroadcastReceiver<SMessage>,
    workspace_tetra: &WorkspaceState,
) -> (Retained<NSView>, Retained<NSSplitViewController>) {

    let alloc: Allocated<NSSplitViewController> = unsafe { msg_send![NSSplitViewController::class(), alloc] };
    let split_controller: Retained<NSSplitViewController> = unsafe { msg_send![alloc, init] };

    let (sidebar_view, sidebar_vc) = sidebar::build_sidebar(rx_synapse);
    let (comms_view, comms_vc) = comms::build_comms_pane();

    unsafe {
        // Let split view controller manage the layout
        let alloc_item: Allocated<NSSplitViewItem> = msg_send![NSSplitViewItem::class(), alloc];
        let sidebar_item: Retained<NSSplitViewItem> = msg_send![alloc_item, init];
        sidebar_item.setViewController(&sidebar_vc);

        let alloc_item2: Allocated<NSSplitViewItem> = msg_send![NSSplitViewItem::class(), alloc];
        let comms_item: Retained<NSSplitViewItem> = msg_send![alloc_item2, init];
        comms_item.setViewController(&comms_vc);

        split_controller.addSplitViewItem(&sidebar_item);
        split_controller.addSplitViewItem(&comms_item);
    }

    let split_view = split_controller.splitView();
    // Enable autolayout constraints on the root view
    unsafe {
        split_view.setTranslatesAutoresizingMaskIntoConstraints(false);
    }

    (split_view.into_super(), split_controller)
}
