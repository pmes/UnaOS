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

/// Represents the completed Workspace view hierarchy along with retained pointers
/// to the core delegates and controllers to prevent them from dropping while the view exists.
pub struct WorkspaceRefs {
    pub view: Retained<NSView>,
    pub root_vc: Retained<NSSplitViewController>,
    pub left_refs: sidebar::SidebarRefs,
    pub right_refs: comms::CommsRefs,
}

/// Assembles the complete Lumen Workspace natively using NSSplitViewController.
/// The `bootstrap_fn` closure provided by the user calls this to build the view hierarchy.
pub fn build_workspace(mtm: MainThreadMarker) -> WorkspaceRefs {
    // We use an NSSplitViewController as the root view controller to manage the left and right panes.
    let split_vc = NSSplitViewController::new(mtm);

    // 1. Build the Left Pane (Sidebar)
    let left_refs = sidebar::build_left_pane(mtm);
    let left_vc = NSViewController::new(mtm);
    left_vc.setView(Some(&left_refs.view));

    let left_item = NSSplitViewItem::sidebarWithViewController(&left_vc);
    left_item.setCanCollapse(true);
    // Explicit sizing for the sidebar
    left_item.setMinimumThickness(200.0);
    left_item.setMaximumThickness(400.0);
    left_item.setPreferredThicknessFraction(0.25);

    // 2. Build the Right Pane (Comms / Reactor)
    let right_refs = comms::build_right_pane(mtm);
    let right_vc = NSViewController::new(mtm);
    right_vc.setView(Some(&right_refs.view));

    let right_item = NSSplitViewItem::splitViewItemWithViewController(&right_vc);

    // Add items to the split view controller
    split_vc.addSplitViewItem(&left_item);
    split_vc.addSplitViewItem(&right_item);

    // Extract the root view
    let root_view = split_vc.view();
    root_view.setTranslatesAutoresizingMaskIntoConstraints(false);

    WorkspaceRefs {
        view: root_view,
        root_vc: split_vc,
        left_refs,
        right_refs,
    }
}
