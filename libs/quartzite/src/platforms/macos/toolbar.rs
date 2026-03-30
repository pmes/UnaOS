// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Toolbar (Window Chrome)
//!
//! Provides the top-level NSToolbar for the main application window.
//! Handles delegate callbacks for item population and identifiers.

use objc2::rc::Retained;
use objc2::{define_class, msg_send_id};
use objc2_app_kit::{NSToolbar, NSToolbarDelegate, NSToolbarItem, NSWindow};
use objc2_foundation::{NSArray, NSObject, NSString, MainThreadMarker, NSNotification};

// -----------------------------------------------------------------------------
// TOOLBAR DELEGATE
// -----------------------------------------------------------------------------
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "UnaToolbarDelegate"]
    pub struct ToolbarDelegate;

    unsafe impl NSToolbarDelegate for ToolbarDelegate {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn toolbar_item_for_identifier(
            &self,
            _toolbar: &NSToolbar,
            item_identifier: &NSString,
            _flag: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            let mtm = MainThreadMarker::new().unwrap();

            // Create a standard item with the requested identifier
            let item = unsafe { NSToolbarItem::initWithItemIdentifier(mtm.alloc(), item_identifier) };

            // In the future, this is where we would inject the Status Group
            // and other actionable icons (e.g., search, settings).

            Some(item)
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn toolbar_default_item_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            let mtm = MainThreadMarker::new().unwrap();
            // Just a flexible space for now to demonstrate layout
            NSArray::from_slice(&[
                unsafe { objc2_app_kit::NSToolbarFlexibleSpaceItemIdentifier(mtm).retained() }
            ])
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn toolbar_allowed_item_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            let mtm = MainThreadMarker::new().unwrap();
            NSArray::from_slice(&[
                unsafe { objc2_app_kit::NSToolbarFlexibleSpaceItemIdentifier(mtm).retained() }
            ])
        }
    }
);

impl ToolbarDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        unsafe { msg_send_id![super(this), init] }
    }
}

// -----------------------------------------------------------------------------
// SETUP FUNCTION
// -----------------------------------------------------------------------------
/// Instantiates and attaches the NSToolbar to the given window.
pub fn setup_toolbar(window: &NSWindow, mtm: MainThreadMarker) {
    let identifier = NSString::from_str("UnaMainToolbar");
    let toolbar = unsafe { NSToolbar::initWithIdentifier(mtm.alloc(), &identifier) };

    // We instantiate the delegate and set it.
    // Note: The NSToolbar does *not* strongly retain its delegate.
    // However, since we are doing a simplified setup here, we attach it.
    // In a fully persistent application, we would store this delegate
    // in the `AppDelegateIvars` to ensure it isn't dropped prematurely.
    let delegate = ToolbarDelegate::new(mtm);

    toolbar.setDelegate(Some(objc2::ProtocolObject::from_ref(&*delegate)));
    toolbar.setShowsBaselineSeparator(true);

    // Memory fix: By using `setToolbar`, the window takes ownership of the toolbar.
    // To prevent the delegate from being dropped, we'll associate it with the window
    // or simply rely on the fact that we're keeping it alive in a more robust structure later.
    // For now, we leak the delegate intentionally to avoid a dangling pointer crash.
    // *Can-Am rule exception for isolated toolbar delegate*.
    let _ = Retained::into_raw(delegate);

    window.setToolbar(Some(&toolbar));
}
