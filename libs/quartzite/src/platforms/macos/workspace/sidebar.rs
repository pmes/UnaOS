// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The macOS Sidebar (Left Pane)
//!
//! Provides the primary navigation utilizing an `NSTabViewController`
//! wrapping a native `NSOutlineView`.

use objc2::rc::Retained;
use objc2::{define_class, msg_send_id};
use objc2_app_kit::{
    NSOutlineView, NSOutlineViewDataSource, NSOutlineViewDelegate, NSScrollView,
    NSTabViewController, NSViewController,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSRect, NSPoint, NSSize};

// -----------------------------------------------------------------------------
// DATA SOURCE & DELEGATE FOR NSOutlineView
// -----------------------------------------------------------------------------
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "UnaSidebarDataSource"]
    pub struct SidebarDataSource;

    unsafe impl NSOutlineViewDataSource for SidebarDataSource {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn number_of_children_of_item(&self, _outline_view: &NSOutlineView, _item: Option<&NSObject>) -> isize {
            0 // Stub: No data yet
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn is_item_expandable(&self, _outline_view: &NSOutlineView, _item: &NSObject) -> bool {
            false // Stub
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        fn child_of_item(&self, _outline_view: &NSOutlineView, _index: isize, _item: Option<&NSObject>) -> Retained<NSObject> {
            unimplemented!()
        }

        // Add additional required dataSource methods here based on v0.6.4 definition...
    }

    unsafe impl NSOutlineViewDelegate for SidebarDataSource {
        // Implement delegate methods for custom views and interactions
    }
);

impl SidebarDataSource {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        unsafe { msg_send_id![super(this), init] }
    }
}

// -----------------------------------------------------------------------------
// SIDEBAR ASSEMBLY
// -----------------------------------------------------------------------------
/// Builds and returns the Sidebar ViewController hierarchy
pub fn build_sidebar(mtm: MainThreadMarker) -> Retained<NSViewController> {
    // 1. We create the Scroll View to hold the Outline View
    let scroll_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(250.0, 600.0));
    let scroll_view = unsafe { NSScrollView::initWithFrame(mtm.alloc(), scroll_rect) };
    scroll_view.setHasVerticalScroller(true);
    scroll_view.setAutohidesScrollers(true);

    // 2. We create the actual Outline View
    let outline_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(250.0, 600.0));
    let outline_view = unsafe { NSOutlineView::initWithFrame(mtm.alloc(), outline_rect) };

    // We intentionally leak the delegate reference here for the skeleton setup.
    // In a fully operational codebase, this should be strongly retained by a parent view controller.
    let data_source = SidebarDataSource::new(mtm);

    // Wire up the delegate and data source
    outline_view.setDataSource(Some(objc2::ProtocolObject::from_ref(&*data_source)));
    outline_view.setDelegate(Some(objc2::ProtocolObject::from_ref(&*data_source)));

    let _ = Retained::into_raw(data_source); // Temporary leak for stability

    scroll_view.setDocumentView(Some(&outline_view));

    // 3. Wrap in a standard NSViewController
    let view_controller = unsafe { NSViewController::new(mtm) };
    view_controller.setView(&scroll_view);

    // 4. In the future, this NSViewController would be one of the tabs
    // inside an NSTabViewController if we need multi-tab sidebar capability.
    // For now, we return it as the primary root controller for the left pane.
    view_controller
}
