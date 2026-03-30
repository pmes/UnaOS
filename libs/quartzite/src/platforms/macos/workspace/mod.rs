// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

pub mod sidebar;
pub mod comms;

use objc2::rc::Retained;
use objc2_app_kit::{NSSplitViewController, NSSplitViewItem, NSView, NSViewController};
use objc2_foundation::MainThreadMarker;

/// Assembles the complete Lumen Workspace natively using NSSplitViewController.
/// The `bootstrap_fn` closure provided by the user calls this to build the view hierarchy.
pub fn build_workspace(mtm: MainThreadMarker) -> Retained<NSView> {
    // We use an NSSplitViewController as the root view controller to manage the left and right panes.
    let split_vc = NSSplitViewController::new(mtm);

    // 1. Build the Left Pane (Sidebar)
    let left_pane = sidebar::build_left_pane(mtm);
    let left_vc = NSViewController::new(mtm);
    left_vc.setView(Some(&left_pane));

    let left_item = NSSplitViewItem::sidebarWithViewController(&left_vc);
    left_item.setCanCollapse(true);
    // Explicit sizing for the sidebar
    left_item.setMinimumThickness(200.0);
    left_item.setMaximumThickness(400.0);
    left_item.setPreferredThicknessFraction(0.25);

    // 2. Build the Right Pane (Comms / Reactor)
    let right_pane = comms::build_right_pane(mtm);
    let right_vc = NSViewController::new(mtm);
    right_vc.setView(Some(&right_pane));

    let right_item = NSSplitViewItem::splitViewItemWithViewController(&right_vc);

    // Add items to the split view controller
    split_vc.addSplitViewItem(&left_item);
    split_vc.addSplitViewItem(&right_item);

    // The caller (mod.rs AppDelegate) will simply assign the split_vc's view to the NSWindow contentView.
    // However, since split_vc will be dropped at the end of this function if we just return the view,
    // we need to ensure the view retains the controller or the architecture handles the lifecycle.
    // In AppKit, NSWindow.contentViewController = split_vc is preferred over just contentView.
    // For now, to bridge cleanly with `NativeView`, we return the view and leak the controller,
    // OR the backend needs to manage it. Let's return the view directly, but we MUST
    // store the split_vc somewhere so it lives as long as the window.

    // For this blueprint implementation, we return the split view itself.
    let root_view = split_vc.view();
    root_view.setTranslatesAutoresizingMaskIntoConstraints(false);

    // To prevent the controller from deallocating immediately and breaking our split logic:
    // (In a full implementation, `Backend` should probably store the RootViewController)
    // We use a safe `forget` here because this is the root application view that lives forever.
    std::mem::forget(split_vc);

    root_view
}
