// SPDX-License-Identifier: GPL-3.0-or-later

//! The Workspace Left Pane (Sidebar)
//!
//! Hosts the `NSOutlineView` inside an `NSScrollView`.
//! Manages the workspace file tree / history tree.

use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSResponder, NSScrollView, NSView, NSOutlineView, NSOutlineViewDataSource, NSOutlineViewDelegate,
    NSTableColumn, NSTableCellView, NSTextField, NSLayoutConstraint
};
use objc2_foundation::{NSArray, NSString, MainThreadOnly, NSObject};

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaSidebarDelegate"]
    pub struct SidebarDelegate;

    unsafe impl NSObjectProtocol for SidebarDelegate {}

    unsafe impl NSOutlineViewDelegate for SidebarDelegate {
        #[unsafe(method_id(outlineView:viewForTableColumn:item:))]
        fn outline_view_view_for_table_column_item(
            &self,
            _outline_view: &NSOutlineView,
            _table_column: Option<&NSTableColumn>,
            item: &NSObject,
        ) -> Option<Retained<NSView>> {
            // Note: Since we return NSStrings in the DataSource for dummy data,
            // we can safely cast the item here to NSString.
            let text = unsafe { Retained::cast::<NSString>(Retained::retain(item)) };

            unsafe {
                let cell: Retained<NSTableCellView> = msg_send![NSTableCellView::class(), alloc];
                let cell: Retained<NSTableCellView> = msg_send![cell, initWithFrame: foundation::NSRect::ZERO];

                let str = NSString::from_str(&text.to_string());
                let text_field: Retained<NSTextField> = msg_send![NSTextField::class(), labelWithString: &*str];

                let _: () = msg_send![&text_field, setTranslatesAutoresizingMaskIntoConstraints: false];
                let _: () = msg_send![&cell, setTextField: &*text_field];
                let _: () = msg_send![&cell, addSubview: &*text_field];

                // Anchor TextField to cell
                let constraints = [
                    NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                        &*text_field, 1, 0, Some(&*cell), 1, 1.0, 0.0, // Leading
                    ),
                    NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                        &*text_field, 2, 0, Some(&*cell), 2, 1.0, 0.0, // Trailing
                    ),
                    NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                        &*text_field, 10, 0, Some(&*cell), 10, 1.0, 0.0, // Center Y
                    ),
                ];
                let array = NSArray::from_slice(&constraints);
                let _: () = msg_send![NSLayoutConstraint::class(), activateConstraints: &*array];

                Some(Retained::cast::<NSView>(cell))
            }
        }
    }
);

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaSidebarDataSource"]
    pub struct SidebarDataSource;

    unsafe impl NSObjectProtocol for SidebarDataSource {}

    unsafe impl NSOutlineViewDataSource for SidebarDataSource {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn outline_view_number_of_children_of_item(&self, _outline_view: &NSOutlineView, item: Option<&NSObject>) -> isize {
            if item.is_none() {
                // Return some dummy top-level rows
                5
            } else {
                0
            }
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn outline_view_is_item_expandable(&self, _outline_view: &NSOutlineView, item: &NSObject) -> bool {
            false
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        fn outline_view_child_of_item(&self, _outline_view: &NSOutlineView, index: isize, item: Option<&NSObject>) -> Retained<NSObject> {
            let item_str = NSString::from_str(&format!("Item {}", index));
            // In a real application, we would return a custom model object here.
            unsafe { Retained::cast::<NSObject>(item_str) }
        }
    }
);

pub struct SidebarRefs {
    pub scroll_view: Retained<NSScrollView>,
    pub outline_view: Retained<NSOutlineView>,
    pub delegate: Retained<SidebarDelegate>,
    pub data_source: Retained<SidebarDataSource>,
}

pub fn create_sidebar() -> SidebarRefs {
    let _mtm = MainThreadOnly::new();

    // Create the Data Source and Delegate
    let delegate: Retained<SidebarDelegate> = unsafe { msg_send![SidebarDelegate::class(), alloc] };
    let delegate: Retained<SidebarDelegate> = unsafe { msg_send![delegate, init] };

    let data_source: Retained<SidebarDataSource> = unsafe { msg_send![SidebarDataSource::class(), alloc] };
    let data_source: Retained<SidebarDataSource> = unsafe { msg_send![data_source, init] };

    // Create the Outline View
    let outline_view: Retained<NSOutlineView> = unsafe { msg_send![NSOutlineView::class(), alloc] };
    let outline_view: Retained<NSOutlineView> = unsafe { msg_send![outline_view, init] };

    // Need at least one column for the outline view
    let col_id = NSString::from_str("MainColumn");
    let column: Retained<NSTableColumn> = unsafe { msg_send![NSTableColumn::class(), alloc] };
    let column: Retained<NSTableColumn> = unsafe { msg_send![column, initWithIdentifier: &*col_id] };
    unsafe {
        let _: () = msg_send![&outline_view, addTableColumn: &*column];
        let _: () = msg_send![&outline_view, setOutlineTableColumn: &*column];
        let _: () = msg_send![&outline_view, setHeaderView: None::<&objc2::runtime::AnyObject>]; // Hide header
    }

    let delegate_obj: &ProtocolObject<dyn NSOutlineViewDelegate> = ProtocolObject::from_ref(&*delegate);
    let ds_obj: &ProtocolObject<dyn NSOutlineViewDataSource> = ProtocolObject::from_ref(&*data_source);

    unsafe {
        let _: () = msg_send![&outline_view, setDelegate: delegate_obj];
        let _: () = msg_send![&outline_view, setDataSource: ds_obj];
        let _: () = msg_send![&outline_view, reloadData];

        // Listen to the Spline's generic refresh
        use objc2_foundation::NSNotificationCenter;
        let center = NSNotificationCenter::defaultCenter();
        let notif_name = NSString::from_str("UnaStateInvalidated");
        // Using `addObserver` would require retaining an observer object. For now we just implement the UI.
        // We will attach an observer using the delegate itself.
        let sel_reload = sel!(reloadData);
        let _: () = msg_send![&center, addObserver: &*outline_view, selector: sel_reload, name: &*notif_name, object: None::<&objc2::runtime::AnyObject>];
    }

    // Wrap in ScrollView
    let scroll_view: Retained<NSScrollView> = unsafe { msg_send![NSScrollView::class(), alloc] };
    let scroll_view: Retained<NSScrollView> = unsafe { msg_send![scroll_view, init] };

    unsafe {
        let _: () = msg_send![&scroll_view, setDocumentView: &*outline_view];
        let _: () = msg_send![&scroll_view, setHasVerticalScroller: true];
        let _: () = msg_send![&scroll_view, setAutohidesScrollers: true];
        let _: () = msg_send![&scroll_view, setTranslatesAutoresizingMaskIntoConstraints: false];
    }

    SidebarRefs {
        scroll_view,
        outline_view,
        delegate,
        data_source,
    }
}
