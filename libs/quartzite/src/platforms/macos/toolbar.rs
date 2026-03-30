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
    declare_class, msg_send, msg_send_id, mutability, rc::Retained, ClassType, DeclaredClass, ProtocolType
};
use objc2_app_kit::{
    NSButton, NSControl, NSFont, NSImage, NSImageNameNetwork, NSProgressIndicator,
    NSProgressIndicatorStyle, NSStackView, NSTextField, NSToolbar, NSToolbarDelegate,
    NSToolbarItem, NSToolbarItemIdentifier, NSToolbarToggleSidebarItemIdentifier,
    NSUserInterfaceLayoutOrientation, NSWindow,
};
use objc2_foundation::{NSArray, MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};

/// The identifier for the custom network telemetry toolbar item
pub const TELEMETRY_ITEM_ID: &str = "UnaOSTelemetryItem";
pub const DIAGNOSTICS_ITEM_ID: &str = "UnaOSDiagnosticsItem";

// -----------------------------------------------------------------------------
// NATIVE TOOLBAR DELEGATE
// -----------------------------------------------------------------------------

declare_class!(
    pub struct ToolbarDelegate;

    unsafe impl ClassType for ToolbarDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "UnaToolbarDelegate";
    }

    impl DeclaredClass for ToolbarDelegate {}

    unsafe impl NSToolbarDelegate for ToolbarDelegate {
        #[method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:)]
        fn toolbar_item_for_identifier(
            &self,
            _toolbar: &NSToolbar,
            identifier: &NSString,
            _will_be_inserted: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            let mtm = MainThreadMarker::new().expect("toolbar_item_for_identifier on main thread");
            let id_str = identifier.to_string();

            // 1. Sidebar Toggle Item (Native AppKit Zero-Math Animation)
            if id_str == NSToolbarToggleSidebarItemIdentifier.to_string() {
                let item = NSToolbarItem::initWithItemIdentifier(mtm.alloc(), identifier);
                // The item naturally configures itself to toggle the first NSSplitViewController sidebar
                return Some(item);
            }

            // 2. Telemetry Text Label (Monospaced Token Counter)
            if id_str == TELEMETRY_ITEM_ID {
                let item = NSToolbarItem::initWithItemIdentifier(mtm.alloc(), identifier);
                item.setLabel(&NSString::from_str("Telemetry"));
                item.setPaletteLabel(&NSString::from_str("Token Telemetry"));

                // Build a non-editable, non-bordered label
                let text_field = NSTextField::labelWithString(&NSString::from_str("0 tx / 0 rx"));
                text_field.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(12.0, objc2_app_kit::NSFontWeightRegular)));
                text_field.setTextColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));

                item.setView(Some(&text_field));
                return Some(item);
            }

            // 3. Network Diagnostics (Spinner + Button)
            if id_str == DIAGNOSTICS_ITEM_ID {
                let item = NSToolbarItem::initWithItemIdentifier(mtm.alloc(), identifier);
                item.setLabel(&NSString::from_str("Network"));
                item.setPaletteLabel(&NSString::from_str("Network Diagnostics"));

                // StackView container
                let stack = NSStackView::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::new(0., 0.), NSSize::new(60., 24.)));
                stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
                stack.setSpacing(8.0);

                // Spinner
                let spinner = NSProgressIndicator::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::new(0., 0.), NSSize::new(16., 16.)));
                spinner.setStyle(NSProgressIndicatorStyle::Spinning);
                spinner.setControlSize(objc2_app_kit::NSControlSize::Small);
                spinner.setDisplayedWhenStopped(false);
                // spinner.startAnimation(None); // Handled by spline.rs

                // Button
                let image = NSImage::imageNamed(NSImageNameNetwork).expect("Network image missing");
                let button = NSButton::imageWithImage_target_action(
                    &image,
                    None,
                    None // No action right now, but would trigger network inspector
                );
                button.setBezelStyle(objc2_app_kit::NSBezelStyle::TexturedRounded);

                stack.addView_inGravity(&spinner, objc2_app_kit::NSStackViewGravity::Leading);
                stack.addView_inGravity(&button, objc2_app_kit::NSStackViewGravity::Trailing);

                item.setView(Some(&stack));
                return Some(item);
            }

            None
        }

        #[method_id(toolbarDefaultItemIdentifiers:)]
        fn toolbar_default_item_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            let mtm = MainThreadMarker::new().expect("toolbarDefaultItemIdentifiers on main thread");
            NSArray::from_vec(vec![
                NSToolbarToggleSidebarItemIdentifier.copy(),
                NSString::from_str("NSToolbarFlexibleSpaceItem"),
                NSString::from_str(TELEMETRY_ITEM_ID),
                NSString::from_str(DIAGNOSTICS_ITEM_ID),
            ])
        }

        #[method_id(toolbarAllowedItemIdentifiers:)]
        fn toolbar_allowed_item_identifiers(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            let mtm = MainThreadMarker::new().expect("toolbarAllowedItemIdentifiers on main thread");
            NSArray::from_vec(vec![
                NSToolbarToggleSidebarItemIdentifier.copy(),
                NSString::from_str("NSToolbarSpaceItem"),
                NSString::from_str("NSToolbarFlexibleSpaceItem"),
                NSString::from_str(TELEMETRY_ITEM_ID),
                NSString::from_str(DIAGNOSTICS_ITEM_ID),
            ])
        }
    }
);

impl ToolbarDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc();
        let this: Retained<Self> = unsafe { msg_send_id![super(this), init] };
        this
    }
}

/// Helper function to attach the native Window Chrome (Toolbar) to the root Window.
pub fn attach_toolbar(window: &NSWindow, mtm: MainThreadMarker) -> Retained<ToolbarDelegate> {
    let toolbar = NSToolbar::initWithIdentifier(mtm.alloc(), &NSString::from_str("UnaOSWorkspaceToolbar"));

    // Store the delegate to prevent it from dropping. The caller must keep this reference alive.
    let delegate = ToolbarDelegate::new(mtm);

    // Wire the delegate
    toolbar.setDelegate(Some(objc2::ProtocolObject::from_ref(&*delegate)));

    // UI behavior configuration
    toolbar.setDisplayMode(objc2_app_kit::NSToolbarDisplayMode::IconOnly);
    toolbar.setShowsBaselineSeparator(true);

    // Attach to the window
    window.setToolbar(Some(&toolbar));

    delegate
}
