// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

pub mod comms;
pub mod sidebar;

use crate::{NativeView, Event};
use bandy::state::{AppState, WorkspaceState};
use objc2::rc::Retained;
use objc2::{msg_send, ClassType, MainThreadOnly};
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSLayoutConstraint, NSLayoutAttribute, NSLayoutRelation, NSSplitView, NSSplitViewDividerStyle,
    NSView, NSWindow, NSColor
};
use objc2_foundation::{NSArray, NSRect, NSPoint, NSSize};
use std::sync::{Arc, RwLock};

pub fn build(
    _tx_event: async_channel::Sender<Event>,
    _app_state: Arc<RwLock<AppState>>,
    _workspace_tetra: &WorkspaceState,
) -> NativeView {
    let mtm = MainThreadOnly::new().unwrap();

    unsafe {
        // Create root view (The Container)
        let root_view: Retained<NSView> = msg_send![NSView::class(), alloc];
        let root_view: Retained<NSView> = msg_send![root_view, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1200.0, 800.0))];
        root_view.setTranslatesAutoresizingMaskIntoConstraints(false);

        // Optional: set a background color or Visual Effect View for native feel
        let split_view: Retained<NSSplitView> = msg_send![NSSplitView::class(), alloc];
        let split_view: Retained<NSSplitView> = msg_send![split_view, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1200.0, 800.0))];
        split_view.setVertical(true);
        split_view.setDividerStyle(NSSplitViewDividerStyle::Thin);
        split_view.setTranslatesAutoresizingMaskIntoConstraints(false);

        // Build Left Pane (Sidebar)
        let sidebar_view = sidebar::build(mtm);

        // Build Right Pane (Comms / Reactor)
        let comms_view = comms::build(mtm);

        split_view.addArrangedSubview(&sidebar_view);
        split_view.addArrangedSubview(&comms_view);

        root_view.addSubview(&split_view);

        // Apply explicit constraints (No Autoresizing)
        let c1 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &split_view,
            NSLayoutAttribute::Leading,
            NSLayoutRelation::Equal,
            Some(&root_view),
            NSLayoutAttribute::Leading,
            1.0,
            0.0,
        );
        let c2 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &split_view,
            NSLayoutAttribute::Trailing,
            NSLayoutRelation::Equal,
            Some(&root_view),
            NSLayoutAttribute::Trailing,
            1.0,
            0.0,
        );
        let c3 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &split_view,
            NSLayoutAttribute::Top,
            NSLayoutRelation::Equal,
            Some(&root_view),
            NSLayoutAttribute::Top,
            1.0,
            0.0,
        );
        let c4 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &split_view,
            NSLayoutAttribute::Bottom,
            NSLayoutRelation::Equal,
            Some(&root_view),
            NSLayoutAttribute::Bottom,
            1.0,
            0.0,
        );
        let c5 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &sidebar_view,
            NSLayoutAttribute::Width,
            NSLayoutRelation::GreaterThanOrEqual,
            None::<&AnyObject>,
            NSLayoutAttribute::NotAnAttribute,
            1.0,
            200.0,
        );

        let constraints = NSArray::from_slice(&[&*c1, &*c2, &*c3, &*c4, &*c5]);
        NSLayoutConstraint::activateConstraints(&constraints);

        root_view
    }
}

// -----------------------------------------------------------------------------
// EVENT HANDLERS
// -----------------------------------------------------------------------------

pub fn handle_state_invalidated(_view: &NativeView) {
    // Traverse the UI tree and update views.
}

pub fn handle_topology_mutated(_view: &NativeView) {
    // Notify the NSOutlineView data source that the matrix tree changed
}

pub fn handle_stream_render(_view: &NativeView) {
    // Trigger scroll-to-bottom on the comms log NSScrollView
}
