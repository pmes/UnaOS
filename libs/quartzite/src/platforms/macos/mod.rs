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

pub mod spline;
pub mod toolbar;
pub mod workspace;

use core::cell::RefCell;
use core::ptr;

use objc2::{ProtocolObject,
    define_class, msg_send,
    ClassType,
    DefinedClass,
    MainThreadOnly,
    rc::{Allocated, Retained},
};
use objc2_foundation::{
    MainThreadMarker,
    NSString,
    NSObject,
    NSObjectProtocol,
    NSNotification,
    NSRect, NSSize, NSPoint,
};
use objc2_app_kit::{
    NSApplicationActivationPolicy,
    NSWindowStyleMask,
    NSBackingStoreType,
    NSApplication,
    NSApplicationDelegate,
    NSApplicationActivationPolicy::Regular,
    NSWindow,
    NSWindowStyleMask::Titled,
    NSWindowStyleMask::Closable,
    NSWindowStyleMask::Resizable,
    NSWindowStyleMask::Miniaturizable,
    NSWindowStyleMask::FullSizeContentView,
    NSBackingStoreType::Buffered,
    NSView,
    NSResponder,
};

pub struct AppDelegateIvars {
    pub window: RefCell<Option<Retained<NSWindow>>>,
    // The bootstrap closure given to us from lumen
    pub bootstrap_fn: RefCell<Option<Box<dyn FnOnce(&NSWindow) -> Retained<NSView> + 'static>>>,
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "LumenAppDelegate"]
    #[ivars = AppDelegateIvars]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn application_did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = MainThreadMarker::from(self);

            // Create window
            let window: Allocated<NSWindow> = unsafe { msg_send![NSWindow::class(), alloc] };
            let style_mask = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::FullSizeContentView;

            let rect = NSRect {
                origin: NSPoint { x: 0.0, y: 0.0 },
                size: NSSize { width: 1200.0, height: 800.0 },
            };

            let window: Retained<NSWindow> = unsafe {
                msg_send![
                    window,
                    initWithContentRect: rect,
                    styleMask: style_mask,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            };

            // Call the bootstrap closure, which constructs the rest of the UI (toolbar, workspace)
            // and returns the root view.
            let mut bootstrap_opt = self.ivars().bootstrap_fn.borrow_mut();
            if let Some(bootstrap) = bootstrap_opt.take() {
                let root_view = bootstrap(&window);
                unsafe {
                    msg_send![&window, setContentView: root_view.as_ref()];
                }
            }

            // Show window
            unsafe {
                msg_send![&window, center];
                msg_send![&window, makeKeyAndOrderFront: None::<&NSObject>];
            }

            // Save window in ivars
            *self.ivars().window.borrow_mut() = Some(window);
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn application_should_terminate_after_last_window_closed(&self, _sender: &NSApplication) -> objc2::runtime::Bool {
            objc2::runtime::Bool::YES // Terminate app when window is closed
        }
    }
);

impl AppDelegate {
    pub fn new(
        bootstrap: impl FnOnce(&NSWindow) -> Retained<NSView> + 'static,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let allocated: Allocated<Self> = unsafe { msg_send![Self::class(), alloc] };
        let this = allocated.set_ivars(AppDelegateIvars {
            window: RefCell::new(None),
            bootstrap_fn: RefCell::new(Some(Box::new(bootstrap))),
        });
        unsafe { msg_send![super(this), init] }
    }
}

pub fn run_macos_app(bootstrap: impl FnOnce(&NSWindow) -> Retained<NSView> + 'static) {
    let mtm = MainThreadMarker::new().expect("run_macos_app must be called on the main thread");
    let app = NSApplication::sharedApplication(mtm);

    // Set activation policy
    unsafe {
        msg_send![&app, setActivationPolicy: NSApplicationActivationPolicy::Regular];
    }

    let delegate = AppDelegate::new(bootstrap, mtm);
    unsafe {
        app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        app.run();
    }
}
