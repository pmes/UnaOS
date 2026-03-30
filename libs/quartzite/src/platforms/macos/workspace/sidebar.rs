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

use objc2::{
    define_class, msg_send, msg_send_id, mutability, rc::Retained, ClassType, DefinedClass, ProtocolType
};
use objc2_app_kit::{
    NSOutlineView, NSOutlineViewDataSource, NSOutlineViewDelegate, NSScrollView,
    NSTableColumn, NSTabViewController, NSTabViewItem, NSView, NSViewController,
    NSLayoutConstraint, NSLayoutAnchor
};
use objc2_foundation::{NSArray, MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};
use std::ffi::c_void;

/// The Left Pane (Sidebar) consists of an NSTabViewController containing
/// an NSOutlineView for the Nodes, Nexus, and TeleHUD views.
pub fn build_left_pane(mtm: MainThreadMarker) -> Retained<NSView> {
    // 1. Root Container
    let root_view = NSView::initWithFrame(
        mtm.alloc(),
        NSRect::new(NSPoint::new(0., 0.), NSSize::new(250., 800.)),
    );
    root_view.setTranslatesAutoresizingMaskIntoConstraints(false);

    // 2. The Tab View Controller (replaces GTK Stack / StackSwitcher)
    let tab_vc = NSTabViewController::new(mtm);
    tab_vc.setTabViewStyle(objc2_app_kit::NSTabViewControllerTabStyle::SegmentedControlOnTop);

    // Create the three tabs required by UnaOS (Nodes, Nexus, TeleHUD)
    let nodes_vc = build_outline_tab(mtm, "Nodes");
    let nexus_vc = build_outline_tab(mtm, "Nexus");
    let telehud_vc = build_outline_tab(mtm, "TeleHUD");

    tab_vc.addTabViewItem(&NSTabViewItem::tabViewItemWithViewController(&nodes_vc));
    tab_vc.addTabViewItem(&NSTabViewItem::tabViewItemWithViewController(&nexus_vc));
    tab_vc.addTabViewItem(&NSTabViewItem::tabViewItemWithViewController(&telehud_vc));

    // 3. Anchor the Tab View to the Root View
    let tab_view = tab_vc.view();
    tab_view.setTranslatesAutoresizingMaskIntoConstraints(false);
    root_view.addSubview(&tab_view);

    let constraints = NSArray::from_vec(vec![
        tab_view.topAnchor().constraintEqualToAnchor(root_view.topAnchor()),
        tab_view.bottomAnchor().constraintEqualToAnchor(root_view.bottomAnchor()),
        tab_view.leadingAnchor().constraintEqualToAnchor(root_view.leadingAnchor()),
        tab_view.trailingAnchor().constraintEqualToAnchor(root_view.trailingAnchor()),
    ]);
    NSLayoutConstraint::activateConstraints(&constraints);

    // Prevent the tab_vc from dropping
    std::mem::forget(tab_vc);

    root_view
}

// -----------------------------------------------------------------------------
// NATIVE OUTLINE VIEW FACTORY & DELEGATES
// -----------------------------------------------------------------------------

fn build_outline_tab(mtm: MainThreadMarker, title: &str) -> Retained<NSViewController> {
    let vc = NSViewController::new(mtm);
    vc.setTitle(&NSString::from_str(title));

    let scroll_view = NSScrollView::initWithFrame(
        mtm.alloc(),
        NSRect::new(NSPoint::new(0., 0.), NSSize::new(250., 800.)),
    );
    scroll_view.setTranslatesAutoresizingMaskIntoConstraints(false);
    scroll_view.setHasVerticalScroller(true);
    scroll_view.setAutohidesScrollers(true);

    let outline_view = NSOutlineView::initWithFrame(
        mtm.alloc(),
        NSRect::new(NSPoint::new(0., 0.), NSSize::new(250., 800.)),
    );
    outline_view.setTranslatesAutoresizingMaskIntoConstraints(false);

    // Outline View Configuration
    let column = NSTableColumn::initWithIdentifier(mtm.alloc(), &NSString::from_str("MainColumn"));
    outline_view.addTableColumn(&column);
    outline_view.setOutlineTableColumn(Some(&column));
    outline_view.setHeaderView(None); // Hide headers
    outline_view.setRowHeight(24.0);

    // Instantiate and wire the Data Source and Delegate
    let data_source = OutlineDataSource::new(mtm);
    outline_view.setDataSource(Some(objc2::ProtocolObject::from_ref(&*data_source)));

    let delegate = OutlineDelegate::new(mtm);
    outline_view.setDelegate(Some(objc2::ProtocolObject::from_ref(&*delegate)));

    scroll_view.setDocumentView(Some(&outline_view));
    vc.setView(Some(&scroll_view));

    // Prevent delegates from dropping
    std::mem::forget(data_source);
    std::mem::forget(delegate);

    vc
}

define_class!(
    pub struct OutlineDataSource;

    unsafe impl ClassType for OutlineDataSource {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "UnaOutlineDataSource";
    }

    impl DefinedClass for OutlineDataSource {}

    unsafe impl NSOutlineViewDataSource for OutlineDataSource {
        #[method(outlineView:numberOfChildrenOfItem:)]
        fn number_of_children_of_item(&self, _outline_view: &NSOutlineView, item: Option<&NSObject>) -> isize {
            if item.is_none() {
                // Root level items (e.g., loaded from `spline.rs` state)
                0
            } else {
                // Child items
                0
            }
        }

        #[method(outlineView:isItemExpandable:)]
        fn is_item_expandable(&self, _outline_view: &NSOutlineView, _item: &NSObject) -> bool {
            false
        }

        #[method_id(outlineView:child:ofItem:)]
        fn child_of_item(&self, _outline_view: &NSOutlineView, _index: isize, _item: Option<&NSObject>) -> Retained<NSObject> {
            let mtm = MainThreadMarker::new().expect("child_of_item on main thread");
            NSObject::new(mtm)
        }

        #[method_id(outlineView:objectValueForTableColumn:byItem:)]
        fn object_value_for_table_column(
            &self,
            _outline_view: &NSOutlineView,
            _table_column: Option<&NSTableColumn>,
            _item: Option<&NSObject>,
        ) -> Option<Retained<NSObject>> {
            None
        }
    }
);

impl OutlineDataSource {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc();
        unsafe { msg_send_id![super(this), init] }
    }
}

define_class!(
    pub struct OutlineDelegate;

    unsafe impl ClassType for OutlineDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "UnaOutlineDelegate";
    }

    impl DefinedClass for OutlineDelegate {}

    unsafe impl NSOutlineViewDelegate for OutlineDelegate {
        #[method_id(outlineView:viewForTableColumn:item:)]
        fn view_for_table_column(
            &self,
            _outline_view: &NSOutlineView,
            _table_column: Option<&NSTableColumn>,
            _item: &NSObject,
        ) -> Option<Retained<NSView>> {
            // In a real implementation, we would return an NSTableCellView containing
            // an icon and a text field, recycled using `makeViewWithIdentifier:owner:`.
            None
        }
    }
);

impl OutlineDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc();
        unsafe { msg_send_id![super(this), init] }
    }
}
