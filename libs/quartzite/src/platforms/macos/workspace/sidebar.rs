// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use objc2::rc::Retained;
use objc2::{define_class, msg_send, ClassType, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSControlTextEditingDelegate, NSOutlineView, NSOutlineViewDataSource, NSOutlineViewDelegate,
    NSScrollView, NSTableColumn, NSTextField, NSView, NSColor
};
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSObjectProtocol, NSString, NSPoint, NSSize, NSRect,
    NSLayoutConstraint, NSLayoutAttribute, NSLayoutRelation, NSArray
};
use std::cell::RefCell;

// -----------------------------------------------------------------------------
// DELEGATE & DATA SOURCE
// -----------------------------------------------------------------------------

pub struct SidebarDelegateIvars {
    pub outline_view: RefCell<Option<Retained<NSOutlineView>>>,
}

define_class!(
    #[unsafe(super(objc2_app_kit::NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumenSidebarDelegate"]
    #[ivars = SidebarDelegateIvars]
    pub struct SidebarDelegate;

    unsafe impl NSObjectProtocol for SidebarDelegate {}

    // Required by AppKit if implementing specific delegate methods, even if unused directly
    unsafe impl NSControlTextEditingDelegate for SidebarDelegate {}

    unsafe impl NSOutlineViewDataSource for SidebarDelegate {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn outlineView_numberOfChildrenOfItem(
            &self,
            _outline_view: &NSOutlineView,
            _item: Option<&AnyObject>,
        ) -> isize {
            // Returns 0 until the Core engine fires SMessage::Matrix(TopologyMutated)
            0
        }

        #[unsafe(method(outlineView:child:ofItem:))]
        fn outlineView_child_ofItem(
            &self,
            _outline_view: &NSOutlineView,
            _index: isize,
            _item: Option<&AnyObject>,
        ) -> Retained<AnyObject> {
            // Return a dummy object if requested before population
            unsafe { msg_send![objc2_foundation::NSObject::class(), new] }
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn outlineView_isItemExpandable(
            &self,
            _outline_view: &NSOutlineView,
            _item: &AnyObject,
        ) -> bool {
            false
        }
    }

    unsafe impl NSOutlineViewDelegate for SidebarDelegate {
        #[unsafe(method(outlineView:viewForTableColumn:item:))]
        fn outlineView_viewForTableColumn_item(
            &self,
            outline_view: &NSOutlineView,
            _table_column: Option<&NSTableColumn>,
            _item: &AnyObject,
        ) -> Option<Retained<NSView>> {
            unsafe {
                // Return a simple NSTextField as the view
                let id = NSString::from_str("DataCell");
                let mut view: Option<Retained<NSTextField>> = outline_view.makeViewWithIdentifier_owner(&id, Some(self));

                if view.is_none() {
                    let new_view: Retained<NSTextField> = msg_send![NSTextField::class(), alloc];
                    let new_view: Retained<NSTextField> = msg_send![new_view, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 20.0))];
                    new_view.setIdentifier(&id);
                    new_view.setBezeled(false);
                    new_view.setDrawsBackground(false);
                    new_view.setEditable(false);
                    view = Some(new_view);
                }

                if let Some(view) = &view {
                    view.setStringValue(&NSString::from_str("Loading..."));
                    // Casting NSTextField up to NSView
                    let view_as_nsview: Retained<NSView> = objc2::rc::Retained::cast(view.clone());
                    return Some(view_as_nsview);
                }

                None
            }
        }
    }
);

impl SidebarDelegate {
    pub fn new() -> Retained<Self> {
        let _mtm = MainThreadOnly::new().unwrap();
        let this = Self::alloc().set_ivars(SidebarDelegateIvars {
            outline_view: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }
}

// -----------------------------------------------------------------------------
// UI BUILDER
// -----------------------------------------------------------------------------

pub fn build(_mtm: MainThreadOnly) -> Retained<NSView> {
    unsafe {
        // Container
        let container: Retained<NSView> = msg_send![NSView::class(), alloc];
        let container: Retained<NSView> = msg_send![container, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(250.0, 800.0))];
        container.setTranslatesAutoresizingMaskIntoConstraints(false);

        // Scroll View
        let scroll_view: Retained<NSScrollView> = msg_send![NSScrollView::class(), alloc];
        let scroll_view: Retained<NSScrollView> = msg_send![scroll_view, initWithFrame: container.bounds()];
        scroll_view.setTranslatesAutoresizingMaskIntoConstraints(false);
        scroll_view.setHasVerticalScroller(true);
        scroll_view.setAutohidesScrollers(true);

        // Outline View
        let outline_view: Retained<NSOutlineView> = msg_send![NSOutlineView::class(), alloc];
        let outline_view: Retained<NSOutlineView> = msg_send![outline_view, initWithFrame: container.bounds()];

        let column: Retained<NSTableColumn> = msg_send![NSTableColumn::class(), alloc];
        let column: Retained<NSTableColumn> = msg_send![column, initWithIdentifier: &*NSString::from_str("MainColumn")];
        outline_view.addTableColumn(&column);
        outline_view.setOutlineTableColumn(Some(&column));
        outline_view.setHeaderView(None::<&objc2_app_kit::NSTableHeaderView>);
        outline_view.setTranslatesAutoresizingMaskIntoConstraints(false);

        // Disable column resizing to fill width
        outline_view.setColumnAutoresizingStyle(objc2_app_kit::NSTableViewColumnAutoresizingStyle::UniformColumnAutoresizingStyle);

        // Wire Delegate & DataSource
        let delegate = SidebarDelegate::new();
        let delegate_obj = objc2::runtime::ProtocolObject::from_ref(&*delegate);

        outline_view.setDataSource(Some(delegate_obj));
        outline_view.setDelegate(Some(delegate_obj));

        // Retain the delegate strongly by passing it into the outline_view's ivars,
        // preventing deallocation at end of scope
        *delegate.ivars().outline_view.borrow_mut() = Some(outline_view.clone());

        scroll_view.setDocumentView(Some(&outline_view));
        container.addSubview(&scroll_view);

        // Explicit Constraints
        let c1 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &scroll_view,
            NSLayoutAttribute::Leading,
            NSLayoutRelation::Equal,
            Some(&container),
            NSLayoutAttribute::Leading,
            1.0,
            0.0,
        );
        let c2 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &scroll_view,
            NSLayoutAttribute::Trailing,
            NSLayoutRelation::Equal,
            Some(&container),
            NSLayoutAttribute::Trailing,
            1.0,
            0.0,
        );
        let c3 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &scroll_view,
            NSLayoutAttribute::Top,
            NSLayoutRelation::Equal,
            Some(&container),
            NSLayoutAttribute::Top,
            1.0,
            0.0,
        );
        let c4 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &scroll_view,
            NSLayoutAttribute::Bottom,
            NSLayoutRelation::Equal,
            Some(&container),
            NSLayoutAttribute::Bottom,
            1.0,
            0.0,
        );

        let constraints = NSArray::from_slice(&[&*c1, &*c2, &*c3, &*c4]);
        NSLayoutConstraint::activateConstraints(&constraints);

        container
    }
}
