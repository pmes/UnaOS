// SPDX-License-Identifier: GPL-3.0-or-later

//! The Toolbar (Window Chrome)
//!
//! Constructs the native `NSToolbar` and delegates the items required by UnaOS:
//! 1. Sidebar Toggle
//! 2. Flexible Space
//! 3. Network Diagnostics
//! 4. Token Telemetry

use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSButton, NSProgressIndicator, NSResponder, NSStackView, NSTextField, NSToolbar,
    NSToolbarDelegate, NSToolbarItem, NSToolbarItemIdentifier, NSToolbarFlexibleSpaceItemIdentifier,
    NSToolbarToggleSidebarItemIdentifier
};
use objc2_foundation::{NSArray, NSString, MainThreadOnly};

// Define our standard identifier constants
fn network_diag_id() -> Retained<NSString> {
    NSString::from_str("UnaNetworkDiagnostics")
}

fn token_telemetry_id() -> Retained<NSString> {
    NSString::from_str("UnaTokenTelemetry")
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaToolbarDelegate"]
    pub struct ToolbarDelegate;

    unsafe impl NSObjectProtocol for ToolbarDelegate {}

    unsafe impl NSToolbarDelegate for ToolbarDelegate {
        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            let net_id = network_diag_id();
            let tel_id = token_telemetry_id();
            let identifiers: [&NSToolbarItemIdentifier; 4] = [
                unsafe { NSToolbarToggleSidebarItemIdentifier },
                &*net_id,
                &*tel_id,
                unsafe { NSToolbarFlexibleSpaceItemIdentifier },
            ];
            NSArray::from_slice(&identifiers)
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            let net_id = network_diag_id();
            let tel_id = token_telemetry_id();
            let identifiers: [&NSToolbarItemIdentifier; 4] = [
                unsafe { NSToolbarToggleSidebarItemIdentifier },
                unsafe { NSToolbarFlexibleSpaceItemIdentifier },
                &*net_id,
                &*tel_id,
            ];
            NSArray::from_slice(&identifiers)
        }

        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item_for_identifier(
            &self,
            _toolbar: &NSToolbar,
            item_identifier: &NSToolbarItemIdentifier,
            _flag: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            unsafe {
                if item_identifier.isEqual(NSToolbarToggleSidebarItemIdentifier) {
                    let item: Retained<NSToolbarItem> = msg_send![NSToolbarItem::class(), alloc];
                    let item: Retained<NSToolbarItem> = msg_send![item, initWithItemIdentifier: item_identifier];
                    return Some(item);
                } else if item_identifier.isEqual(NSToolbarFlexibleSpaceItemIdentifier) {
                    let item: Retained<NSToolbarItem> = msg_send![NSToolbarItem::class(), alloc];
                    let item: Retained<NSToolbarItem> = msg_send![item, initWithItemIdentifier: item_identifier];
                    return Some(item);
                } else if item_identifier.isEqual(&*network_diag_id()) {
                    let item: Retained<NSToolbarItem> = msg_send![NSToolbarItem::class(), alloc];
                    let item: Retained<NSToolbarItem> = msg_send![item, initWithItemIdentifier: item_identifier];

                    let stack: Retained<NSStackView> = msg_send![NSStackView::class(), alloc];
                    let stack: Retained<NSStackView> = msg_send![stack, init];

                    let _: () = msg_send![&stack, setOrientation: 0_isize]; // NSUserInterfaceLayoutOrientationHorizontal
                    let _: () = msg_send![&stack, setSpacing: 8.0_f64];

                    // Progress Indicator (Spinner)
                    let spinner: Retained<NSProgressIndicator> = msg_send![NSProgressIndicator::class(), alloc];
                    let spinner: Retained<NSProgressIndicator> = msg_send![spinner, init];

                    let _: () = msg_send![&spinner, setStyle: 1_isize]; // NSProgressIndicatorStyleSpinning
                    let _: () = msg_send![&spinner, setControlSize: 1_isize]; // NSControlSizeSmall
                    let _: () = msg_send![&spinner, startAnimation: None::<&objc2::runtime::AnyObject>];

                    let _: () = msg_send![&spinner, setTranslatesAutoresizingMaskIntoConstraints: false];

                    // Network Button
                    let title = NSString::from_str("0 ms");
                    let btn: Retained<NSButton> = msg_send![NSButton::class(), buttonWithTitle: &*title, target: None::<&objc2::runtime::AnyObject>, action: core::ptr::null_mut::<objc2::sel::Sel>()];

                    let _: () = msg_send![&btn, setBezelStyle: 1_isize]; // NSBezelStyleRounded
                    let _: () = msg_send![&btn, setTranslatesAutoresizingMaskIntoConstraints: false];

                    let _: () = msg_send![&stack, addView: &*spinner, inGravity: 1_isize]; // NSStackViewGravityLeading
                    let _: () = msg_send![&stack, addView: &*btn, inGravity: 1_isize];

                    let _: () = msg_send![&item, setView: &*stack];
                    let label = NSString::from_str("Network");
                    let _: () = msg_send![&item, setLabel: &*label];
                    return Some(item);
                } else if item_identifier.isEqual(&*token_telemetry_id()) {
                    let item: Retained<NSToolbarItem> = msg_send![NSToolbarItem::class(), alloc];
                    let item: Retained<NSToolbarItem> = msg_send![item, initWithItemIdentifier: item_identifier];

                    let str = NSString::from_str("0 tk/s");
                    let label: Retained<NSTextField> = msg_send![NSTextField::class(), labelWithString: &*str];

                    let _: () = msg_send![&label, setTranslatesAutoresizingMaskIntoConstraints: false];

                    let _: () = msg_send![&item, setView: &*label];
                    let item_label = NSString::from_str("Telemetry");
                    let _: () = msg_send![&item, setLabel: &*item_label];
                    return Some(item);
                }

                None
            }
        }
    }
);

pub fn create_toolbar() -> (Retained<NSToolbar>, Retained<ToolbarDelegate>) {
    let _mtm = MainThreadOnly::new();
    let delegate: Retained<ToolbarDelegate> = unsafe { msg_send![ToolbarDelegate::class(), alloc] };
    let delegate: Retained<ToolbarDelegate> = unsafe { msg_send![delegate, init] };

    let ident = NSString::from_str("UnaMainToolbar");
    let toolbar: Retained<NSToolbar> = unsafe { msg_send![NSToolbar::class(), alloc] };
    let toolbar: Retained<NSToolbar> = unsafe { msg_send![toolbar, initWithIdentifier: &*ident] };

    let protocol_obj: &ProtocolObject<dyn NSToolbarDelegate> = ProtocolObject::from_ref(&*delegate);
    unsafe {
        let _: () = msg_send![&toolbar, setDelegate: protocol_obj];
        let _: () = msg_send![&toolbar, setDisplayMode: 2_isize]; // NSToolbarDisplayModeIconOnly
        let _: () = msg_send![&toolbar, setShowsBaselineSeparator: true];
    }

    (toolbar, delegate)
}
