// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, ClassType, DeclaredClass, MainThreadOnly};
use objc2_app_kit::{
    NSResponder, NSWindow, NSWindowDelegate, NSToolbar, NSToolbarDelegate, NSView,
    NSWindowStyleMask, NSBackingStoreType, NSToolbarItemIdentifier, NSToolbarItem,
    NSTitlebarAccessoryViewController
};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSRect, NSSize, NSString, NSArray};

use crate::platforms::macos::workspace;

// -----------------------------------------------------------------------------
// WINDOW DELEGATE
// -----------------------------------------------------------------------------
struct WindowDelegateIvars {}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaWindowDelegate"]
    #[ivars = WindowDelegateIvars]
    pub struct WindowDelegate;

    unsafe impl NSWindowDelegate for WindowDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(WindowDelegateIvars {});
            unsafe { msg_send![super(this), init] }
        }
    }
);

unsafe impl NSObjectProtocol for WindowDelegate {}

// -----------------------------------------------------------------------------
// TOOLBAR DELEGATE
// -----------------------------------------------------------------------------
struct ToolbarDelegateIvars {}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaToolbarDelegate"]
    #[ivars = ToolbarDelegateIvars]
    pub struct ToolbarDelegate;

    unsafe impl NSToolbarDelegate for ToolbarDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(ToolbarDelegateIvars {});
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn toolbar_allowed_item_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            NSArray::new()
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn toolbar_default_item_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            NSArray::new()
        }

        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn toolbar_item_for_identifier(
            &self,
            _toolbar: &NSToolbar,
            _item_identifier: &NSToolbarItemIdentifier,
            _flag: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            None
        }
    }
);

unsafe impl NSObjectProtocol for ToolbarDelegate {}

// -----------------------------------------------------------------------------
// CHROME ASSEMBLY
// -----------------------------------------------------------------------------
pub fn create_window(mtm: MainThreadMarker) -> (Retained<NSWindow>, Retained<WindowDelegate>, Retained<ToolbarDelegate>) {
    // 1. Allocate and initialize the Window Delegate
    let window_delegate: Allocated<WindowDelegate> = unsafe { msg_send![WindowDelegate::class(), alloc] };
    let window_delegate: Retained<WindowDelegate> = unsafe { msg_send![window_delegate, init] };

    // 2. Window Construction
    let frame = NSRect::new(objc2_foundation::NSPoint::new(0.0, 0.0), NSSize::new(1024.0, 768.0));
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::FullSizeContentView;

    let window = NSWindow::initWithContentRect_styleMask_backing_defer(
        NSWindow::alloc(mtm),
        frame,
        style,
        NSBackingStoreType::Buffered,
        false,
    );

    window.setTitle(&NSString::from_str("UnaOS: Lumen"));
    window.setTitlebarAppearsTransparent(true);
    window.setDelegate(Some(ProtocolObject::from_ref(&*window_delegate)));

    // 3. Toolbar Construction
    let toolbar_delegate: Allocated<ToolbarDelegate> = unsafe { msg_send![ToolbarDelegate::class(), alloc] };
    let toolbar_delegate: Retained<ToolbarDelegate> = unsafe { msg_send![toolbar_delegate, init] };
    let toolbar_id = NSString::from_str("UnaMainToolbar");
    let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &toolbar_id);

    // Store delegate onto toolbar (or global state) to prevent premature drop.
    // For now we will just set it. In a real implementation we would anchor it.
    toolbar.setDelegate(Some(ProtocolObject::from_ref(&*toolbar_delegate)));
    window.setToolbar(Some(&toolbar));

    // 4. Return the assembled pieces
    (window, window_delegate, toolbar_delegate)
}
