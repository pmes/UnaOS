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
    /// True when `node_id` is a directory on disk. Topology rebroadcasts carry
    /// only *visible* rows, so a collapsed directory arrives childless —
    /// expandability must come from the filesystem, not from loaded children,
    /// or chevrons vanish after every rebuild.
    pub is_dir: RefCell<bool>,
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
                is_dir: RefCell::new(false),
            });
            unsafe { msg_send![super(this), init] }
        }
    }
);

/// Recursively re-apply expansion state after a `reloadData` (which collapses
/// everything). Only descends into expanded nodes — collapsed subtrees keep
/// their native default.
pub fn restore_expansion(outline_view: &NSOutlineView, node: &Retained<UnaMatrixNode>) {
    if *node.ivars().is_expanded.borrow() {
        unsafe {
            let _: () = msg_send![outline_view, expandItem: &**node];
        }
        for child in node.ivars().children.borrow().iter() {
            restore_expansion(outline_view, child);
        }
    }
}

impl UnaMatrixNode {
    pub fn build_from(rust_node: &TopologyNode) -> Retained<Self> {
        let node: Allocated<UnaMatrixNode> = unsafe { msg_send![UnaMatrixNode::class(), alloc] };
        let node: Retained<UnaMatrixNode> = unsafe { msg_send![node, init] };

        *node.ivars().node_id.borrow_mut() = rust_node.id.clone();
        *node.ivars().label.borrow_mut() = rust_node.label.clone();
        *node.ivars().is_expanded.borrow_mut() = rust_node.is_expanded;
        *node.ivars().is_dir.borrow_mut() = std::fs::metadata(&rust_node.id)
            .map(|m| m.is_dir())
            .unwrap_or(false);

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
    /// UI → brain sender. Selection changes fire `ToggleMatrixNode(id)` so the
    /// brain sees file activations (the GTK/Qt sidebars already do this; the
    /// macOS outline handled expand/collapse natively and never reported
    /// clicks until now).
    pub tx_event: RefCell<Option<async_channel::Sender<bandy::SMessage>>>,
    /// True while `restore_expansion` re-applies expansion after a reload;
    /// programmatic expandItem/collapseItem fire the same DidExpand/DidCollapse
    /// notifications as user chevron clicks, and echoing those back to the
    /// brain would toggle its state right back (feedback loop).
    pub suppress_expand_events: RefCell<bool>,
    /// Collapse-burst root. AppKit consults shouldCollapseItem for the user's
    /// item FIRST, then for each expanded descendant (trace-verified
    /// 2026-07-18); only the first may reach the brain — reporting descendants
    /// erases the nested expansion state that restores the shape on reopen.
    /// Set on the burst's first shouldCollapse, cleared by the root's own
    /// DidCollapse (which fires last, deepest-first order).
    pub collapse_burst_root: RefCell<Option<String>>,
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
                tx_event: RefCell::new(None),
                suppress_expand_events: RefCell::new(false),
                collapse_burst_root: RefCell::new(None),
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
            // Directories are always expandable even when a rebuild delivered
            // them childless (collapsed dirs carry no children in the
            // visible-rows broadcast); their children arrive on expand.
            if !node.ivars().children.borrow().is_empty() || *node.ivars().is_dir.borrow() {
                objc2::runtime::Bool::YES
            } else {
                objc2::runtime::Bool::NO
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
        /// Fires on every selection change (single click). Reports the selected
        /// node's id to the brain as `ToggleMatrixNode` — the brain decides what
        /// a click means (file → editor load, directory → topology bookkeeping).
        #[unsafe(method(outlineViewSelectionDidChange:))]
        fn outline_view_selection_did_change(&self, _notification: &objc2_foundation::NSNotification) {
            // reloadData can move selection; don't echo synthetic selection
            // changes back to the brain (same guard as DidExpand/DidCollapse).
            if *self.ivars().suppress_expand_events.borrow() {
                return;
            }
            let Some(outline_view) = self.ivars().outline_view.borrow().clone() else { return };
            let row: NSInteger = unsafe { msg_send![&*outline_view, selectedRow] };
            if row < 0 {
                return;
            }
            let item: *mut AnyObject = unsafe { msg_send![&*outline_view, itemAtRow: row] };
            if item.is_null() {
                return;
            }
            let node = unsafe {
                Retained::cast_unchecked::<UnaMatrixNode>(Retained::retain(item).unwrap())
            };
            // Selection is a *focus* gesture and only files report it (the
            // brain routes it to the editor). Directories must NOT fire here:
            // collapsing a parent moves selection onto it, and reporting that
            // as a toggle re-expanded the brain's node and reset the tree.
            // Directory expansion is the chevron handlers' job alone.
            if *node.ivars().is_dir.borrow() {
                return;
            }
            let id = node.ivars().node_id.borrow().clone();
            if let Some(tx) = self.ivars().tx_event.borrow().as_ref() {
                let _ = tx.try_send(bandy::SMessage::ToggleMatrixNode(id));
            }
        }

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

                    let cell_obj: *mut AnyObject = msg_send![&text_field, cell];
                    if !cell_obj.is_null() {
                        let _: () = msg_send![cell_obj, setWraps: objc2::runtime::Bool::NO];
                        let _: () = msg_send![cell_obj, setLineBreakMode: 4isize]; // NSLineBreakByTruncatingTail
                    }
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

        /// Consulted only for the item the *user* targets (not for the
        /// DidCollapse cascade AppKit fires for expanded descendants, and not
        /// for programmatic expandItem during restore) — so this, not
        /// DidExpand/DidCollapse, is where the brain gets told. Trace evidence
        /// 2026-07-18: the cascade fires deepest-first with rows still live,
        /// so no Did*-side filter can distinguish the user's click.
        #[unsafe(method(outlineView:shouldExpandItem:))]
        fn outline_view_should_expand_item(
            &self,
            _outline_view: &NSOutlineView,
            item: &AnyObject,
        ) -> objc2::runtime::Bool {
            let node = unsafe { Retained::cast_unchecked::<UnaMatrixNode>(Retained::retain(item as *const AnyObject as *mut AnyObject).unwrap()) };
            if !*self.ivars().suppress_expand_events.borrow() {
                let id = node.ivars().node_id.borrow().clone();
                if let Some(tx) = self.ivars().tx_event.borrow().as_ref() {
                    let _ = tx.try_send(bandy::SMessage::ToggleMatrixNode(id));
                }
            }
            objc2::runtime::Bool::YES
        }

        #[unsafe(method(outlineView:shouldCollapseItem:))]
        fn outline_view_should_collapse_item(
            &self,
            _outline_view: &NSOutlineView,
            item: &AnyObject,
        ) -> objc2::runtime::Bool {
            let node = unsafe { Retained::cast_unchecked::<UnaMatrixNode>(Retained::retain(item as *const AnyObject as *mut AnyObject).unwrap()) };
            let id = node.ivars().node_id.borrow().clone();
            if !*self.ivars().suppress_expand_events.borrow() {
                let is_cascade = self.ivars().collapse_burst_root.borrow().as_ref()
                    .is_some_and(|root| id.starts_with(&format!("{root}/")));
                if !is_cascade {
                    *self.ivars().collapse_burst_root.borrow_mut() = Some(id.clone());
                    if let Some(tx) = self.ivars().tx_event.borrow().as_ref() {
                        let _ = tx.try_send(bandy::SMessage::ToggleMatrixNode(id));
                    }
                }
            }
            objc2::runtime::Bool::YES
        }

        #[unsafe(method(outlineViewItemDidExpand:))]
        fn outline_view_item_did_expand(&self, notification: &objc2_foundation::NSNotification) {
            unsafe {
                if let Some(user_info) = notification.userInfo() {
                    let key = NSString::from_str("NSObject"); // NSOutlineView's item key in userInfo is usually @"NSObject"
                    let item: *mut AnyObject = msg_send![&user_info, objectForKey: &*key];

                    if !item.is_null() {
                        let node = Retained::cast_unchecked::<UnaMatrixNode>(Retained::retain(item).unwrap());
                        // Bookkeeping only — the brain is told in
                        // shouldExpandItem (user-targeted items only).
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
                        // Bookkeeping is deliberately NOT done here: the
                        // DidCollapse cascade covers expanded descendants whose
                        // is_expanded must survive (it restores the shape when
                        // the parent reopens). The burst root's own DidCollapse
                        // fires last — it closes the burst window.
                        let id = node.ivars().node_id.borrow().clone();
                        let mut burst = self.ivars().collapse_burst_root.borrow_mut();
                        if burst.as_deref() == Some(id.as_str()) {
                            *burst = None;
                        }
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
pub fn create_sidebar(
    _mtm: MainThreadMarker,
    workspace_state: &bandy::state::WorkspaceState,
    tx_event: async_channel::Sender<bandy::SMessage>,
) -> (Retained<NSView>, Retained<SidebarDelegate>) {
    // 1. Instantiate the delegate
    let delegate: Allocated<SidebarDelegate> = unsafe { msg_send![SidebarDelegate::class(), alloc] };
    let delegate: Retained<SidebarDelegate> = unsafe { msg_send![delegate, init] };
    *delegate.ivars().tx_event.borrow_mut() = Some(tx_event);

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
    scroll_view.setHasHorizontalScroller(true);
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
