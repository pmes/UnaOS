// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The macOS Workspace
//!
//! Assembles the Left Pane (Sidebar) and the Right Pane (Comms) into an `NSSplitView`.

use objc2::rc::Retained;
use objc2_app_kit::{
    NSSplitView, NSSplitViewController, NSSplitViewItem, NSView,
};
use objc2_foundation::{MainThreadMarker, NSRect, NSPoint, NSSize};

pub mod sidebar;
pub mod comms;

// -----------------------------------------------------------------------------
// WORKSPACE ASSEMBLY
// -----------------------------------------------------------------------------

/// Builds and returns the main Workspace `NSView` hierarchy, combining
/// the Sidebar and Comms panels via `NSSplitView`.
pub fn build_workspace(mtm: MainThreadMarker) -> Retained<NSView> {
    // 1. Create the Split View Controller (manages standard modern macOS pane behavior)
    let split_vc = unsafe { NSSplitViewController::new(mtm) };

    // Create the raw Split View
    let split_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1024.0, 768.0));
    let split_view = unsafe { NSSplitView::initWithFrame(mtm.alloc(), split_rect) };
    split_view.setVertical(true); // Left/Right columns

    // Assign the view to the controller
    split_vc.setSplitView(&split_view);

    // 2. Build Sidebar (Left Pane)
    let sidebar_vc = sidebar::build_sidebar(mtm);
    let sidebar_item = unsafe { NSSplitViewItem::splitViewItemWithViewController(&sidebar_vc) };
    sidebar_item.setCanCollapse(true);
    // Optionally set minimum thickness etc.

    // 3. Build Comms (Right Pane)
    let comms_vc = comms::build_comms(mtm);
    let comms_item = unsafe { NSSplitViewItem::splitViewItemWithViewController(&comms_vc) };

    // 4. Combine
    unsafe {
        split_vc.addSplitViewItem(&sidebar_item);
        split_vc.addSplitViewItem(&comms_item);
    }

    // 5. Return the rendered root Split View
    split_vc.view()
}
