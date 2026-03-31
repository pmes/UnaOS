// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use core::cell::RefCell;
use std::sync::{Arc, Mutex};

use objc2::runtime::ProtocolObject;
use objc2::{
    define_class, msg_send,
    ClassType,
    DefinedClass,
    MainThreadOnly,
    rc::{Allocated, Retained},
};
use objc2_foundation::{
    NSString,
    NSObject,
    NSObjectProtocol,
    NSArray,
    NSRect, NSPoint, NSSize,
};
use objc2_app_kit::{
    NSView,
    NSViewController,
    NSScrollView,
    NSOutlineView,
    NSOutlineViewDataSource,
    NSOutlineViewDelegate,
    NSControlTextEditingDelegate,
    NSTableColumn,
    NSTextField,
    NSLayoutConstraint,
    NSResponder,
};

use tokio::sync::broadcast::Receiver as BroadcastReceiver;
use bandy::SMessage;
use bandy::state::WorkspaceState;
use dispatch2::DispatchQueue;

// The pure data structure
struct TreeItem {
    pub name: String,
    pub children: Vec<TreeItem>,
}

pub struct SidebarDataSourceIvars {
    // The data backing the NSOutlineView. We map it to Rust structs.
    // In a full app, this would be bandy states or nodes.
    // We will just store dummy data here, but the data will be populated from rx_synapse
    data: RefCell<Vec<TreeItem>>,
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "LumenSidebarDataSource"]
    #[ivars = SidebarDataSourceIvars]
    pub struct SidebarDataSource;

    unsafe impl NSObjectProtocol for SidebarDataSource {}

    unsafe impl NSOutlineViewDataSource for SidebarDataSource {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn outline_view_number_of_children_of_item(&self, _outline_view: &NSOutlineView, item: Option<&NSObject>) -> isize {
            if let Some(_item) = item {
                // If it's a child node, we'll pretend there are no sub-children for now
                // Usually we'd look up the item via pointer address
                0
            } else {
                self.ivars().data.borrow().len() as isize
            }
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        fn outline_view_child_of_item(&self, _outline_view: &NSOutlineView, index: isize, item: Option<&NSObject>) -> Retained<NSObject> {
            if item.is_none() {
                // Root item, return a dummy NSObject representing the item
                // In AppKit, outline view data source nodes can be anything. We'll just return NSString.
                let data = self.ivars().data.borrow();
                let tree_item = &data[index as usize];
                NSString::from_str(&tree_item.name).into_super()
            } else {
                NSString::from_str("Child").into_super()
            }
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn outline_view_is_item_expandable(&self, _outline_view: &NSOutlineView, _item: &NSObject) -> objc2::runtime::Bool {
            objc2::runtime::Bool::NO
        }
    }
);

impl SidebarDataSource {
    pub fn new() -> Retained<Self> {
        let alloc: Allocated<Self> = unsafe { msg_send![Self::class(), alloc] };
        let this = alloc.set_ivars(SidebarDataSourceIvars {
            data: RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub fn set_data(&self, data: Vec<TreeItem>) {
        *self.ivars().data.borrow_mut() = data;
    }
}

pub struct SidebarDelegateIvars {
    // A delegate handles UI generation for items
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "LumenSidebarDelegate"]
    #[ivars = SidebarDelegateIvars]
    pub struct SidebarDelegate;

    unsafe impl NSObjectProtocol for SidebarDelegate {}
    unsafe impl NSControlTextEditingDelegate for SidebarDelegate {}

    unsafe impl NSOutlineViewDelegate for SidebarDelegate {
        #[unsafe(method_id(outlineView:viewForTableColumn:item:))]
        fn outline_view_view_for_table_column_item(&self, _outline_view: &NSOutlineView, _table_column: Option<&NSTableColumn>, item: &NSObject) -> Option<Retained<NSView>> {
            // Reconstruct the text
            let text = unsafe { Retained::cast_unchecked::<NSString>(msg_send![item, retain]) };

            // Generate NSTextField
            let tf_alloc: Allocated<NSTextField> = unsafe { msg_send![NSTextField::class(), alloc] };
            let tf: Retained<NSTextField> = unsafe {
                msg_send![
                    tf_alloc,
                    initWithFrame: NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { width: 100.0, height: 20.0 } }
                ]
            };

            unsafe {
                tf.setStringValue(&text);
                tf.setEditable(false);
                tf.setBordered(false);
                tf.setDrawsBackground(false);
            }

            Some(unsafe { Retained::cast_unchecked::<NSView>(tf) })
        }
    }
);

impl SidebarDelegate {
    pub fn new() -> Retained<Self> {
        let alloc: Allocated<Self> = unsafe { msg_send![Self::class(), alloc] };
        let this = alloc.set_ivars(SidebarDelegateIvars {});
        unsafe { msg_send![super(this), init] }
    }
}

pub struct SidebarVCIvars {
    pub outline_delegate: RefCell<Option<Retained<SidebarDelegate>>>,
    pub outline_data_source: RefCell<Option<Retained<SidebarDataSource>>>,
    // Store tokio abort handle to clean up background task when view controller drops
    pub abort_handle: RefCell<Option<tokio::task::AbortHandle>>,
}

define_class!(
    #[unsafe(super(NSViewController))]
    #[name = "LumenSidebarViewController"]
    #[ivars = SidebarVCIvars]
    pub struct SidebarViewController;
);

impl SidebarViewController {
    pub fn new() -> Retained<Self> {
        let alloc: Allocated<Self> = unsafe { msg_send![Self::class(), alloc] };
        let this = alloc.set_ivars(SidebarVCIvars {
            outline_delegate: RefCell::new(None),
            outline_data_source: RefCell::new(None),
            abort_handle: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }
}

impl Drop for SidebarVCIvars {
    fn drop(&mut self) {
        if let Some(handle) = self.abort_handle.borrow_mut().take() {
            handle.abort();
        }
    }
}

pub fn build_sidebar(mut rx_synapse: BroadcastReceiver<SMessage>) -> (Retained<NSView>, Retained<SidebarViewController>) {
    let scroll_alloc: Allocated<NSScrollView> = unsafe { msg_send![NSScrollView::class(), alloc] };
    let scroll_view: Retained<NSScrollView> = unsafe {
        msg_send![scroll_alloc, initWithFrame: NSRect::ZERO]
    };

    unsafe {
        scroll_view.setHasVerticalScroller(true);
        scroll_view.setHasHorizontalScroller(false);
        scroll_view.setAutohidesScrollers(true);
        scroll_view.setTranslatesAutoresizingMaskIntoConstraints(false);
    }

    let alloc: Allocated<NSOutlineView> = unsafe { msg_send![NSOutlineView::class(), alloc] };
    let outline_view: Retained<NSOutlineView> = unsafe {
        msg_send![alloc, initWithFrame: NSRect::ZERO]
    };

    let column_alloc: Allocated<NSTableColumn> = unsafe { msg_send![NSTableColumn::class(), alloc] };
    let column: Retained<NSTableColumn> = unsafe {
        let identifier = NSString::from_str("SidebarColumn");
        msg_send![column_alloc, initWithIdentifier: &*identifier]
    };

    unsafe {
        outline_view.addTableColumn(&column);
        outline_view.setOutlineTableColumn(Some(&column));
        outline_view.setHeaderView(None);
        scroll_view.setDocumentView(Some(&outline_view));
    }

    let data_source = SidebarDataSource::new();
    let delegate = SidebarDelegate::new();

    unsafe {
        outline_view.setDataSource(Some(ProtocolObject::from_ref(&*data_source)));
        outline_view.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    }

    let vc = SidebarViewController::new();
    *vc.ivars().outline_delegate.borrow_mut() = Some(delegate);
    *vc.ivars().outline_data_source.borrow_mut() = Some(data_source.clone());
    unsafe {
        vc.setView(&scroll_view);
    }

    // Keep an independent strong reference for the background task to use
    // instead of capturing a raw pointer which may be dropped.
    // In objective-c, Retained clones increment the retain count.
    let ds_retained = data_source.clone();
    let outline_retained = outline_view.clone();

    // Spin up async listener
    let task = tokio::spawn(async move {
        while let Ok(msg) = rx_synapse.recv().await {
            match msg {
                SMessage::Matrix(bandy::MatrixEvent::TopologyMutated(topology)) => {
                    // Update data source on main thread
                    // The block captures the strong Retained pointers
                    let ds_clone = ds_retained.clone();
                    let outline_clone = outline_retained.clone();
                    dispatch2::DispatchQueue::main().exec_async(move || {
                        let ds = ds_clone.as_ref();
                        let outline = outline_clone.as_ref();

                        let items = topology.into_iter().map(|(id, name, _)| {
                            TreeItem { name: format!("{} ({})", name, id), children: vec![] }
                        }).collect();

                        ds.set_data(items);
                        unsafe {
                            outline.reloadData();
                        }
                    });
                }
                _ => {}
            }
        }
    });

    *vc.ivars().abort_handle.borrow_mut() = Some(task.abort_handle());

    // We return scroll_view but we keep vc alive
    (scroll_view.into_super(), vc)
}
