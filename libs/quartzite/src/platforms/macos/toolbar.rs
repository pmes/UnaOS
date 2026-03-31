// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use objc2::rc::Retained;
use objc2::{define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{
    NSButton, NSControl, NSFont, NSProgressIndicator, NSProgressIndicatorStyle, NSStackView,
    NSStackViewDistribution, NSTextField, NSToolbar, NSToolbarDelegate, NSToolbarDisplayMode,
    NSToolbarItem, NSToolbarItemIdentifier, NSToolbarSizeMode, NSView, NSWindow,
};
use objc2_foundation::{
    NSArray, MainThreadOnly, NSObjectProtocol, NSString, NSPoint, NSSize, NSRect
};
use std::cell::RefCell;

// -----------------------------------------------------------------------------
// CONSTANTS
// -----------------------------------------------------------------------------
// AppKit requires unique NSString identifiers for each toolbar item.
const TOOLBAR_IDENTIFIER: &str = "org.unaos.lumen.toolbar";
const SIDEBAR_TOGGLE_ID: &str = "NSToolbarToggleSidebarItemIdentifier"; // Native AppKit constant
const NETWORK_DIAGNOSTICS_ID: &str = "org.unaos.lumen.network_diagnostics";
const TOKEN_TELEMETRY_ID: &str = "org.unaos.lumen.token_telemetry";

// -----------------------------------------------------------------------------
// TOOLBAR DELEGATE
// -----------------------------------------------------------------------------

pub struct ToolbarDelegateIvars {
    // We retain the toolbar so it isn't deallocated prematurely.
    pub toolbar: RefCell<Option<Retained<NSToolbar>>>,
}

define_class!(
    #[unsafe(super(objc2_app_kit::NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumenToolbarDelegate"]
    #[ivars = ToolbarDelegateIvars]
    pub struct ToolbarDelegate;

    unsafe impl NSObjectProtocol for ToolbarDelegate {}

    unsafe impl NSToolbarDelegate for ToolbarDelegate {
        #[unsafe(method(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn toolbar_itemForItemIdentifier_willBeInsertedIntoToolbar(
            &self,
            _toolbar: &NSToolbar,
            item_identifier: &NSToolbarItemIdentifier,
            _flag: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            let mtm = MainThreadOnly::new().unwrap();

            if item_identifier.isEqual(Some(&*NSString::from_str(SIDEBAR_TOGGLE_ID))) {
                // Return a native Sidebar Toggle item
                unsafe {
                    let item: Retained<NSToolbarItem> = msg_send![NSToolbarItem::class(), alloc];
                    let item: Retained<NSToolbarItem> = msg_send![item, initWithItemIdentifier: item_identifier];
                    return Some(item);
                }
            } else if item_identifier.isEqual(Some(&*NSString::from_str(NETWORK_DIAGNOSTICS_ID))) {
                return Some(build_network_diagnostics_item(mtm));
            } else if item_identifier.isEqual(Some(&*NSString::from_str(TOKEN_TELEMETRY_ID))) {
                return Some(build_token_telemetry_item(mtm));
            }

            None
        }

        #[unsafe(method(toolbarAllowedItemIdentifiers:))]
        fn toolbarAllowedItemIdentifiers(
            &self,
            _toolbar: &NSToolbar,
        ) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            NSArray::from_slice(&[
                &*NSString::from_str(SIDEBAR_TOGGLE_ID),
                &*NSString::from_str(NETWORK_DIAGNOSTICS_ID),
                &*NSString::from_str(TOKEN_TELEMETRY_ID),
                &*NSString::from_str("NSToolbarFlexibleSpaceItemIdentifier"),
            ])
        }

        #[unsafe(method(toolbarDefaultItemIdentifiers:))]
        fn toolbarDefaultItemIdentifiers(
            &self,
            _toolbar: &NSToolbar,
        ) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            NSArray::from_slice(&[
                &*NSString::from_str(SIDEBAR_TOGGLE_ID),
                &*NSString::from_str("NSToolbarFlexibleSpaceItemIdentifier"),
                &*NSString::from_str(NETWORK_DIAGNOSTICS_ID),
                &*NSString::from_str(TOKEN_TELEMETRY_ID),
            ])
        }
    }
);

impl ToolbarDelegate {
    pub fn new() -> Retained<Self> {
        let mtm = MainThreadOnly::new().unwrap();
        let this = Self::alloc().set_ivars(ToolbarDelegateIvars {
            toolbar: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }
}

// -----------------------------------------------------------------------------
// COMPONENT BUILDERS
// -----------------------------------------------------------------------------

fn build_network_diagnostics_item(mtm: MainThreadOnly) -> Retained<NSToolbarItem> {
    unsafe {
        let identifier = NSString::from_str(NETWORK_DIAGNOSTICS_ID);
        let item: Retained<NSToolbarItem> = msg_send![NSToolbarItem::class(), alloc];
        let item: Retained<NSToolbarItem> = msg_send![item, initWithItemIdentifier: &*identifier];
        item.setLabel(&NSString::from_str("Network"));

        // Create an NSStackView to hold a spinning progress indicator and a diagnostic button
        let stack: Retained<NSStackView> = msg_send![NSStackView::class(), alloc];
        let stack: Retained<NSStackView> = msg_send![stack, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 24.0))];
        stack.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Horizontal);
        stack.setSpacing(8.0);
        stack.setDistribution(NSStackViewDistribution::GravityAreas);

        let progress: Retained<NSProgressIndicator> = msg_send![NSProgressIndicator::class(), alloc];
        let progress: Retained<NSProgressIndicator> = msg_send![progress, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(16.0, 16.0))];
        progress.setStyle(NSProgressIndicatorStyle::Spinning);
        progress.setControlSize(objc2_app_kit::NSControlSize::Small);
        progress.setDisplayedWhenStopped(true);
        // We do not start the animation yet. SMessage routing will trigger this when the network is active.

        let button: Retained<NSButton> = msg_send![NSButton::class(), alloc];
        let button: Retained<NSButton> = msg_send![button, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(60.0, 24.0))];
        button.setTitle(&NSString::from_str("0ms"));
        button.setBezelStyle(objc2_app_kit::NSBezelStyle::RoundRect);

        stack.addView_inGravity(&progress, objc2_app_kit::NSStackViewGravity::Center);
        stack.addView_inGravity(&button, objc2_app_kit::NSStackViewGravity::Center);

        item.setView(Some(&stack));
        item
    }
}

fn build_token_telemetry_item(mtm: MainThreadOnly) -> Retained<NSToolbarItem> {
    unsafe {
        let identifier = NSString::from_str(TOKEN_TELEMETRY_ID);
        let item: Retained<NSToolbarItem> = msg_send![NSToolbarItem::class(), alloc];
        let item: Retained<NSToolbarItem> = msg_send![item, initWithItemIdentifier: &*identifier];
        item.setLabel(&NSString::from_str("Telemetry"));

        // Create a monospaced NSTextField for tokens
        let label: Retained<NSTextField> = msg_send![NSTextField::class(), alloc];
        let label: Retained<NSTextField> = msg_send![label, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(150.0, 24.0))];

        label.setStringValue(&NSString::from_str("Tokens: 0 / 0 TPS"));
        label.setBezeled(false);
        label.setDrawsBackground(false);
        label.setEditable(false);
        label.setSelectable(false);

        if let Some(font) = NSFont::monospacedSystemFontOfSize_weight(11.0, objc2_app_kit::NSFontWeightRegular) {
            label.setFont(Some(&font));
        }

        label.setAlignment(objc2_app_kit::NSTextAlignment::Right);

        item.setView(Some(&label));
        item
    }
}

// -----------------------------------------------------------------------------
// EXPORTED ATTACHMENT FUNCTION
// -----------------------------------------------------------------------------

pub fn attach_toolbar(window: &NSWindow, mtm: MainThreadOnly) {
    unsafe {
        let identifier = NSString::from_str(TOOLBAR_IDENTIFIER);
        let toolbar: Retained<NSToolbar> = msg_send![NSToolbar::class(), alloc];
        let toolbar: Retained<NSToolbar> = msg_send![toolbar, initWithIdentifier: &*identifier];

        toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
        toolbar.setShowsBaselineSeparator(true);

        let delegate = ToolbarDelegate::new();
        // Since the delegate only implements `NSToolbarDelegate`, we cast to `ProtocolObject`
        toolbar.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate)));

        // Retain the delegate strongly by stashing it back inside the delegate's own ivars
        // Note: This creates a circular reference if not explicitly cleaned up,
        // but for a root window toolbar that lasts the lifetime of the application, it's acceptable.
        *delegate.ivars().toolbar.borrow_mut() = Some(toolbar.clone());

        window.setToolbar(Some(&toolbar));
    }
}
