// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{
    NSResponder, NSOutlineView, NSOutlineViewDelegate, NSOutlineViewDataSource,
    NSControlTextEditingDelegate, NSTableColumn, NSView, NSScrollView,
    NSTableCellView, NSTextField
};
use objc2_foundation::{
    NSObjectProtocol, NSInteger, NSString, NSRect, NSPoint, NSSize,
    MainThreadMarker, NSObject
};
use std::cell::RefCell;
use bandy::state::TopologyNode;

// -----------------------------------------------------------------------------
// MATRIX NODE FFI BRIDGE
// -----------------------------------------------------------------------------
pub struct UnaMatrixNodeIvars {
    pub node_id: RefCell<String>,
    pub label: RefCell<String>,
    pub children: RefCell<Vec<Retained<UnaMatrixNode>>>,
    pub is_expanded: RefCell<bool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "UnaMatrixNode"]
    #[ivars = UnaMatrixNodeIvars]
    pub struct UnaMatrixNode;

    impl UnaMatrixNode {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(UnaMatrixNodeIvars {
                node_id: RefCell::new(String::new()),
                label: RefCell::new(String::new()),
                children: RefCell::new(Vec::new()),
                is_expanded: RefCell::new(false),
            });
            unsafe { msg_send![super(this), init] }
        }
    }
);

impl UnaMatrixNode {
    pub fn build_from(rust_node: &TopologyNode) -> Retained<Self> {
        let node: Allocated<UnaMatrixNode> = unsafe { msg_send![UnaMatrixNode::class(), alloc] };
        let node: Retained<UnaMatrixNode> = unsafe { msg_send![node, init] };

        *node.ivars().node_id.borrow_mut() = rust_node.id.clone();
        *node.ivars().label.borrow_mut() = rust_node.label.clone();
        *node.ivars().is_expanded.borrow_mut() = rust_node.is_expanded;

        let mut children = Vec::new();
        for child in &rust_node.children {
            children.push(Self::build_from(child));
        }
        *node.ivars().children.borrow_mut() = children;

        node
    }
}

// -----------------------------------------------------------------------------
// SIDEBAR DELEGATE (LUMEN LEFT PANE)
// -----------------------------------------------------------------------------
pub struct SidebarDelegateIvars {
    pub roots: RefCell<Vec<Retained<UnaMatrixNode>>>,
    pub outline_view: RefCell<Option<Retained<NSOutlineView>>>,
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaSidebarDelegate"]
    #[ivars = SidebarDelegateIvars]
    pub struct SidebarDelegate;

    impl SidebarDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(SidebarDelegateIvars {
                roots: RefCell::new(Vec::new()),
                outline_view: RefCell::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    // --- Outline View Data Source ---
    unsafe impl NSOutlineViewDataSource for SidebarDelegate {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn outline_view_number_of_children_of_item(
            &self,
            _outline_view: &NSOutlineView,
            item: Option<&AnyObject>,
        ) -> NSInteger {
            if let Some(item_ptr) = item {
                // It's a child node
                let node = unsafe { Retained::cast_unchecked::<UnaMatrixNode>(Retained::retain(item_ptr as *const AnyObject as *mut AnyObject).unwrap()) };
                node.ivars().children.borrow().len() as NSInteger
            } else {
                // It's the root level
                self.ivars().roots.borrow().len() as NSInteger
            }
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn outline_view_is_item_expandable(
            &self,
            _outline_view: &NSOutlineView,
            item: &AnyObject,
        ) -> objc2::runtime::Bool {
            let node = unsafe { Retained::cast_unchecked::<UnaMatrixNode>(Retained::retain(item as *const AnyObject as *mut AnyObject).unwrap()) };
            if node.ivars().children.borrow().is_empty() {
                objc2::runtime::Bool::NO
            } else {
                objc2::runtime::Bool::YES
            }
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        fn outline_view_child_of_item(
            &self,
            _outline_view: &NSOutlineView,
            index: NSInteger,
            item: Option<&AnyObject>,
        ) -> Retained<AnyObject> {
            if let Some(item_ptr) = item {
                let node = unsafe { Retained::cast_unchecked::<UnaMatrixNode>(Retained::retain(item_ptr as *const AnyObject as *mut AnyObject).unwrap()) };
                let child = &node.ivars().children.borrow()[index as usize];
                unsafe { Retained::cast_unchecked::<AnyObject>(child.clone()) }
            } else {
                let root = &self.ivars().roots.borrow()[index as usize];
                unsafe { Retained::cast_unchecked::<AnyObject>(root.clone()) }
            }
        }

        #[unsafe(method_id(outlineView:objectValueForTableColumn:byItem:))]
        fn outline_view_object_value_for_table_column_by_item(
            &self,
            _outline_view: &NSOutlineView,
            _table_column: Option<&NSTableColumn>,
            item: Option<&AnyObject>,
        ) -> Option<Retained<AnyObject>> {
            if let Some(item_ptr) = item {
                let node = unsafe { Retained::cast_unchecked::<UnaMatrixNode>(Retained::retain(item_ptr as *const AnyObject as *mut AnyObject).unwrap()) };
                let label = node.ivars().label.borrow().clone();
                Some(unsafe { Retained::cast_unchecked::<AnyObject>(NSString::from_str(&label)) })
            } else {
                None
            }
        }
    }

    // --- Outline View Delegate ---
    unsafe impl NSOutlineViewDelegate for SidebarDelegate {
        #[unsafe(method_id(outlineView:viewForTableColumn:item:))]
        fn outline_view_view_for_table_column_item(
            &self,
            outline_view: &NSOutlineView,
            _table_column: Option<&NSTableColumn>,
            item: &AnyObject,
        ) -> Option<Retained<NSView>> {
            let node = unsafe { Retained::cast_unchecked::<UnaMatrixNode>(Retained::retain(item as *const AnyObject as *mut AnyObject).unwrap()) };
            let label_str = node.ivars().label.borrow().clone();

            let identifier = NSString::from_str("SidebarCell");
            let mut cell: Option<Retained<NSTableCellView>> = unsafe {
                let recycled: *mut AnyObject = msg_send![outline_view, makeViewWithIdentifier: &*identifier, owner: self];
                if !recycled.is_null() {
                    Some(Retained::cast_unchecked::<NSTableCellView>(Retained::retain(recycled).unwrap()))
                } else {
                    None
                }
            };

            if cell.is_none() {
                let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(200.0, 24.0));
                let new_cell: Allocated<NSTableCellView> = unsafe { msg_send![NSTableCellView::class(), alloc] };
                let new_cell: Retained<NSTableCellView> = unsafe { msg_send![new_cell, initWithFrame: frame] };
                unsafe {
                    let _: () = msg_send![&new_cell, setIdentifier: &*identifier];
                }

                let text_field: Allocated<NSTextField> = unsafe { msg_send![NSTextField::class(), alloc] };
                let text_field: Retained<NSTextField> = unsafe { msg_send![text_field, initWithFrame: frame] };
                unsafe {
                    let _: () = msg_send![&text_field, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
                    let _: () = msg_send![&text_field, setBordered: objc2::runtime::Bool::NO];
                    let _: () = msg_send![&text_field, setDrawsBackground: objc2::runtime::Bool::NO];
                    let _: () = msg_send![&text_field, setEditable: objc2::runtime::Bool::NO];
                    let _: () = msg_send![&text_field, setSelectable: objc2::runtime::Bool::NO];
                }

                new_cell.addSubview(&text_field);
                unsafe { new_cell.setTextField(Some(&text_field)); }

                let constraints = unsafe {
                    objc2_foundation::NSArray::from_slice(&[
                        &*objc2_app_kit::NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                            &text_field, objc2_app_kit::NSLayoutAttribute::CenterY, objc2_app_kit::NSLayoutRelation::Equal,
                            Some(&new_cell), objc2_app_kit::NSLayoutAttribute::CenterY, 1.0, 0.0
                        ),
                        &*objc2_app_kit::NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                            &text_field, objc2_app_kit::NSLayoutAttribute::Leading, objc2_app_kit::NSLayoutRelation::Equal,
                            Some(&new_cell), objc2_app_kit::NSLayoutAttribute::Leading, 1.0, 4.0
                        ),
                        &*objc2_app_kit::NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                            &text_field, objc2_app_kit::NSLayoutAttribute::Trailing, objc2_app_kit::NSLayoutRelation::Equal,
                            Some(&new_cell), objc2_app_kit::NSLayoutAttribute::Trailing, 1.0, -4.0
                        ),
                    ])
                };
                objc2_app_kit::NSLayoutConstraint::activateConstraints(&constraints);

                cell = Some(new_cell);
            }

            let cell = cell.unwrap();
            let text_field = unsafe { cell.textField().unwrap() };

            let ns_text = NSString::from_str(&label_str);
            unsafe {
                let _: () = msg_send![&text_field, setStringValue: &*ns_text];
            }

            Some(unsafe { Retained::cast_unchecked::<NSView>(cell) })
        }

        #[unsafe(method(outlineViewItemDidExpand:))]
        fn outline_view_item_did_expand(&self, notification: &objc2_foundation::NSNotification) {
            unsafe {
                if let Some(user_info) = notification.userInfo() {
                    let key = NSString::from_str("NSObject"); // NSOutlineView's item key in userInfo is usually @"NSObject"
                    let item: *mut AnyObject = msg_send![&user_info, objectForKey: &*key];

                    if !item.is_null() {
                        let node = Retained::cast_unchecked::<UnaMatrixNode>(Retained::retain(item).unwrap());
                        *node.ivars().is_expanded.borrow_mut() = true;
                    }
                }
            }
        }

        #[unsafe(method(outlineViewItemDidCollapse:))]
        fn outline_view_item_did_collapse(&self, notification: &objc2_foundation::NSNotification) {
            unsafe {
                if let Some(user_info) = notification.userInfo() {
                    let key = NSString::from_str("NSObject"); // NSOutlineView's item key in userInfo
                    let item: *mut AnyObject = msg_send![&user_info, objectForKey: &*key];

                    if !item.is_null() {
                        let node = Retained::cast_unchecked::<UnaMatrixNode>(Retained::retain(item).unwrap());
                        *node.ivars().is_expanded.borrow_mut() = false;
                    }
                }
            }
        }
    }
);

unsafe impl NSObjectProtocol for SidebarDelegate {}
unsafe impl NSControlTextEditingDelegate for SidebarDelegate {}

// -----------------------------------------------------------------------------
// ASSEMBLY
// -----------------------------------------------------------------------------
pub fn create_sidebar(_mtm: MainThreadMarker, workspace_state: &bandy::state::WorkspaceState) -> (Retained<NSView>, Retained<SidebarDelegate>) {
    // 1. Instantiate the delegate
    let delegate: Allocated<SidebarDelegate> = unsafe { msg_send![SidebarDelegate::class(), alloc] };
    let delegate: Retained<SidebarDelegate> = unsafe { msg_send![delegate, init] };

    // 1.5 Synchronous Initial Data Population
    if let bandy::state::ViewEntity::Topology(matrix_state) = &workspace_state.left_pane {
        let mut new_roots = Vec::new();
        for root in &matrix_state.tree.roots {
            new_roots.push(UnaMatrixNode::build_from(root));
        }
        *delegate.ivars().roots.borrow_mut() = new_roots;
    }

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

    // Anchor outline_view into delegate
    *delegate.ivars().outline_view.borrow_mut() = Some(outline_view.clone());

    // 3. Create the scroll view wrapper
    let scroll_view: Allocated<NSScrollView> = unsafe { msg_send![NSScrollView::class(), alloc] };
    let scroll_view: Retained<NSScrollView> = unsafe { msg_send![scroll_view, initWithFrame: frame] };

    scroll_view.setHasVerticalScroller(true);
    scroll_view.setHasHorizontalScroller(false);
    scroll_view.setAutohidesScrollers(true);

    // Attach the outline view to the scroll view
    scroll_view.setDocumentView(Some(&outline_view));

    // Reload the outline view immediately so data renders on first frame
    unsafe {
        let _: () = msg_send![&outline_view, reloadData];
    }

    // Enforce Layout Integrity (Squeezing) - Sidebar minimum width
    unsafe {
        let width_anchor: Retained<objc2_app_kit::NSLayoutDimension> = msg_send![&scroll_view, widthAnchor];
        let constraint: Retained<objc2_app_kit::NSLayoutConstraint> = msg_send![&width_anchor, constraintGreaterThanOrEqualToConstant: 200.0f64];
        let _: () = msg_send![&constraint, setActive: objc2::runtime::Bool::YES];
    }

    // Enforce initial collapsed/expanded states natively based on Rust state
    let roots_ref = delegate.ivars().roots.borrow();
    for root in roots_ref.iter() {
        let is_expanded = *root.ivars().is_expanded.borrow();
        unsafe {
            if is_expanded {
                let _: () = msg_send![&outline_view, expandItem: &**root];
            } else {
                let _: () = msg_send![&outline_view, collapseItem: &**root];
            }
        }
    }
    // Drop the borrow before returning
    drop(roots_ref);

    // Respect the Safe Area (Traffic Light Overlap)
    let insets = objc2_foundation::NSEdgeInsets { top: 38.0, left: 0.0, bottom: 0.0, right: 0.0 };
    unsafe {
        let _: () = msg_send![&scroll_view, setContentInsets: insets];
    }

    // Return the scroll view as the root view of this component, and the delegate to hold state
    (unsafe { Retained::cast_unchecked::<NSView>(scroll_view) }, delegate)
}
