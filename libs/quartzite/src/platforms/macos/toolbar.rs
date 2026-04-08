// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use core::cell::RefCell;
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
};
use objc2_app_kit::{
    NSWindow,
    NSToolbar,
    NSToolbarDelegate,
    NSToolbarItem,
    NSToolbarFlexibleSpaceItemIdentifier,
    NSResponder,
};

pub struct ToolbarDelegateIvars {
    pub tx_event: RefCell<Option<async_channel::Sender<crate::Event>>>,
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "LumenToolbarDelegate"]
    #[ivars = ToolbarDelegateIvars]
    pub struct ToolbarDelegate;

    unsafe impl NSObjectProtocol for ToolbarDelegate {}

    unsafe impl NSToolbarDelegate for ToolbarDelegate {
        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_item_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            unsafe {
                NSArray::from_slice(&[NSToolbarFlexibleSpaceItemIdentifier])
            }
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_item_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            unsafe {
                NSArray::from_slice(&[NSToolbarFlexibleSpaceItemIdentifier])
            }
        }

        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn toolbar_item(&self, _toolbar: &NSToolbar, item_identifier: &NSString, _flag: bool) -> Option<Retained<NSToolbarItem>> {
            let alloc: Allocated<NSToolbarItem> = unsafe { msg_send![NSToolbarItem::class(), alloc] };
            let item: Retained<NSToolbarItem> = unsafe { msg_send![alloc, initWithItemIdentifier: item_identifier] };
            Some(item)
        }
    }
);

impl ToolbarDelegate {
    pub fn new(tx_event: async_channel::Sender<crate::Event>) -> Retained<Self> {
        let alloc: Allocated<Self> = unsafe { msg_send![Self::class(), alloc] };
        let this = alloc.set_ivars(ToolbarDelegateIvars {
            tx_event: RefCell::new(Some(tx_event)),
        });
        unsafe { msg_send![super(this), init] }
    }
}

pub fn build_toolbar(window: &NSWindow, tx_event: async_channel::Sender<crate::Event>) -> Retained<ToolbarDelegate> {
    let identifier = NSString::from_str("LumenMainToolbar");
    let alloc: Allocated<NSToolbar> = unsafe { msg_send![NSToolbar::class(), alloc] };
    let toolbar: Retained<NSToolbar> = unsafe { msg_send![alloc, initWithIdentifier: &*identifier] };

    let delegate = ToolbarDelegate::new(tx_event);
    unsafe {
        toolbar.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        window.setToolbar(Some(&toolbar));
    }

    delegate
}
