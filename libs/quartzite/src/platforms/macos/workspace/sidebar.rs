// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, ClassType};
use objc2_app_kit::{
    NSResponder, NSOutlineView, NSOutlineViewDelegate, NSOutlineViewDataSource,
    NSControlTextEditingDelegate, NSTableColumn, NSView, NSScrollView
};
use objc2_foundation::{
    NSObjectProtocol, NSInteger, NSString, NSRect, NSPoint, NSSize,
    MainThreadMarker
};

// -----------------------------------------------------------------------------
// SIDEBAR DELEGATE (LUMEN LEFT PANE)
// -----------------------------------------------------------------------------
pub struct SidebarDelegateIvars {}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaSidebarDelegate"]
    #[ivars = SidebarDelegateIvars]
    pub struct SidebarDelegate;

    impl SidebarDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(SidebarDelegateIvars {});
            unsafe { msg_send![super(this), init] }
        }
    }

    // --- Outline View Data Source ---
    unsafe impl NSOutlineViewDataSource for SidebarDelegate {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn outline_view_number_of_children_of_item(
            &self,
            _outline_view: &NSOutlineView,
            _item: Option<&AnyObject>,
        ) -> NSInteger {
            0
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn outline_view_is_item_expandable(
            &self,
            _outline_view: &NSOutlineView,
            _item: &AnyObject,
        ) -> objc2::runtime::Bool {
            objc2::runtime::Bool::NO
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        fn outline_view_child_of_item(
            &self,
            _outline_view: &NSOutlineView,
            _index: NSInteger,
            _item: Option<&AnyObject>,
        ) -> Retained<AnyObject> {
            // For stub purposes, return a generic NSString
            unsafe {
                Retained::cast_unchecked::<AnyObject>(NSString::from_str("StubNode"))
            }
        }

        #[unsafe(method_id(outlineView:objectValueForTableColumn:byItem:))]
        fn outline_view_object_value_for_table_column_by_item(
            &self,
            _outline_view: &NSOutlineView,
            _table_column: Option<&NSTableColumn>,
            _item: Option<&AnyObject>,
        ) -> Option<Retained<AnyObject>> {
            None
        }
    }

    // --- Outline View Delegate ---
    unsafe impl NSOutlineViewDelegate for SidebarDelegate {
        #[unsafe(method_id(outlineView:viewForTableColumn:item:))]
        fn outline_view_view_for_table_column_item(
            &self,
            _outline_view: &NSOutlineView,
            _table_column: Option<&NSTableColumn>,
            _item: &AnyObject,
        ) -> Option<Retained<NSView>> {
            None
        }
    }
);

unsafe impl NSObjectProtocol for SidebarDelegate {}
unsafe impl NSControlTextEditingDelegate for SidebarDelegate {}

// -----------------------------------------------------------------------------
// ASSEMBLY
// -----------------------------------------------------------------------------
pub fn create_sidebar(_mtm: MainThreadMarker) -> (Retained<NSView>, Retained<SidebarDelegate>) {
    // 1. Instantiate the delegate
    let delegate: Allocated<SidebarDelegate> = unsafe { msg_send![SidebarDelegate::class(), alloc] };
    let delegate: Retained<SidebarDelegate> = unsafe { msg_send![delegate, init] };

    // 2. Create the outline view (the actual sidebar content)
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(250.0, 768.0));
    let outline_view: Allocated<NSOutlineView> = unsafe { msg_send![NSOutlineView::class(), alloc] };
    let outline_view: Retained<NSOutlineView> = unsafe { msg_send![outline_view, initWithFrame: frame] };

    // Set the delegates wrapped as protocol objects
    unsafe {
        outline_view.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        outline_view.setDataSource(Some(ProtocolObject::from_ref(&*delegate)));

        // Optional: Hide the header for a cleaner sidebar look
        outline_view.setHeaderView(None);

        // Create the dummy column
        let column: Allocated<NSTableColumn> = msg_send![NSTableColumn::class(), alloc];
        let column_id = NSString::from_str("MainColumn");
        let column: Retained<NSTableColumn> = msg_send![column, initWithIdentifier: &*column_id];
        outline_view.addTableColumn(&column);
        outline_view.setOutlineTableColumn(Some(&column));
    }

    // 3. Create the scroll view wrapper
    let scroll_view: Allocated<NSScrollView> = unsafe { msg_send![NSScrollView::class(), alloc] };
    let scroll_view: Retained<NSScrollView> = unsafe { msg_send![scroll_view, initWithFrame: frame] };

    // Turn off automatic layout constraints
    unsafe {
        let _: () = msg_send![&scroll_view, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
    }

    scroll_view.setHasVerticalScroller(true);
    scroll_view.setHasHorizontalScroller(false);
    scroll_view.setAutohidesScrollers(true);

    // Attach the outline view to the scroll view
    scroll_view.setDocumentView(Some(&outline_view));

    // Return the scroll view as the root view of this component, and the delegate to hold state
    (unsafe { Retained::cast_unchecked::<NSView>(scroll_view) }, delegate)
}
