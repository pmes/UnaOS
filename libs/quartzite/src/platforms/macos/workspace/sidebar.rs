// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSResponder, NSOutlineView, NSOutlineViewDelegate, NSOutlineViewDataSource,
    NSControlTextEditingDelegate, NSTableColumn, NSView
};
use objc2_foundation::{NSObjectProtocol, NSInteger, NSString};

// -----------------------------------------------------------------------------
// SIDEBAR DELEGATE (LUMEN LEFT PANE)
// -----------------------------------------------------------------------------
struct SidebarDelegateIvars {}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaSidebarDelegate"]
    #[ivars = SidebarDelegateIvars]
    pub struct SidebarDelegate;

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
            Retained::cast_unchecked::<AnyObject>(NSString::from_str("StubNode"))
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
        #[unsafe(method(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(SidebarDelegateIvars {});
            unsafe { msg_send![super(this), init] }
        }

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
