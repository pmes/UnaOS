// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The macOS Comms (Right Pane)
//!
//! Provides the primary chat/reactor interface utilizing `NSScrollView`
//! anchored to an input `NSTextView` via rigid Auto Layout constraints.

use objc2::rc::Retained;
use objc2_app_kit::{
    NSLayoutConstraint, NSScrollView, NSTextView, NSView, NSViewController,
    NSColor, NSText, NSTextContainer
};
use objc2_foundation::{MainThreadMarker, NSRect, NSPoint, NSSize, NSArray, NSString};

// -----------------------------------------------------------------------------
// COMMS ASSEMBLY
// -----------------------------------------------------------------------------

/// Builds and returns the Comms (Reactor) ViewController hierarchy
pub fn build_comms(mtm: MainThreadMarker) -> Retained<NSViewController> {
    // 1. Create the Root View Container
    let root_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
    let root_view = unsafe { NSView::initWithFrame(mtm.alloc(), root_rect) };

    // We strictly use Auto Layout. Disable the legacy autoresizing mask.
    root_view.setTranslatesAutoresizingMaskIntoConstraints(false);
    unsafe { root_view.setBackgroundColor(Some(&NSColor::textBackgroundColor())) };

    // 2. Build the Message History Scroll View
    let scroll_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 500.0));
    let history_scroll = unsafe { NSScrollView::initWithFrame(mtm.alloc(), scroll_rect) };
    history_scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
    history_scroll.setHasVerticalScroller(true);
    history_scroll.setAutohidesScrollers(true);

    // Create an NSTextView to dump historical text into.
    let history_text = unsafe { NSTextView::initWithFrame(mtm.alloc(), scroll_rect) };
    history_text.setEditable(false);
    history_text.setSelectable(true);
    unsafe { history_text.setString(&NSString::from_str("Reactor offline...")) };

    history_scroll.setDocumentView(Some(&history_text));

    // 3. Build the Input Buffer (Bottom TextView)
    let input_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 100.0));
    let input_scroll = unsafe { NSScrollView::initWithFrame(mtm.alloc(), input_rect) };
    input_scroll.setTranslatesAutoresizingMaskIntoConstraints(false);

    let input_text = unsafe { NSTextView::initWithFrame(mtm.alloc(), input_rect) };
    input_text.setEditable(true);
    input_text.setSelectable(true);

    input_scroll.setDocumentView(Some(&input_text));

    // 4. Assemble Hierarchy
    unsafe {
        root_view.addSubview(&history_scroll);
        root_view.addSubview(&input_scroll);
    }

    // 5. Wire Auto Layout Constraints
    let constraints = vec![
        // History Scroll Constraints
        unsafe { history_scroll.topAnchor().constraintEqualToAnchor(&root_view.topAnchor()) },
        unsafe { history_scroll.leadingAnchor().constraintEqualToAnchor(&root_view.leadingAnchor()) },
        unsafe { history_scroll.trailingAnchor().constraintEqualToAnchor(&root_view.trailingAnchor()) },

        // Input Scroll Constraints
        unsafe { input_scroll.topAnchor().constraintEqualToAnchor_constant(&history_scroll.bottomAnchor(), 10.0) },
        unsafe { input_scroll.leadingAnchor().constraintEqualToAnchor(&root_view.leadingAnchor()) },
        unsafe { input_scroll.trailingAnchor().constraintEqualToAnchor(&root_view.trailingAnchor()) },
        unsafe { input_scroll.bottomAnchor().constraintEqualToAnchor_constant(&root_view.bottomAnchor(), -10.0) },

        // Rigid Height for Input
        unsafe { input_scroll.heightAnchor().constraintEqualToConstant(80.0) },
    ];

    unsafe { NSLayoutConstraint::activateConstraints(&NSArray::from_vec(constraints)) };

    // 6. Wrap in ViewController
    let vc = unsafe { NSViewController::new(mtm) };
    vc.setView(&root_view);

    vc
}
