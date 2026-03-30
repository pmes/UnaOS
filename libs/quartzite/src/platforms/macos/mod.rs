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

use crate::{NativeView, NativeWindow};
use block2::RcBlock;
use objc2::{define_class, msg_send, msg_send_id, mutability, rc::Retained, ClassType, DefinedClass};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSWindow, NSWindowStyleMask};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;

// The function signature we expect from the core application
type BootstrapFn = Box<dyn FnOnce(&NativeWindow) -> NativeView + 'static>;

pub struct Backend {
    app: Retained<NSApplication>,
    // We keep a reference to the delegate so it isn't deallocated
    _delegate: Retained<AppDelegate>,
}

impl Backend {
    pub fn new<F>(app_id: &str, bootstrap: F) -> Self
    where
        F: FnOnce(&NativeWindow) -> NativeView + 'static,
    {
        // Require main thread access for AppKit
        let mtm = MainThreadMarker::new().expect("Backend::new must be called on the main thread");

        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

        // Define our custom application delegate
        let mut delegate = AppDelegate::new(mtm);
        delegate.set_bootstrap(Box::new(bootstrap));

        app.setDelegate(Some(objc2::ProtocolObject::from_ref(&*delegate)));

        Self { app, _delegate: delegate }
    }

    pub fn run(&self) {
        let mtm = MainThreadMarker::new().expect("Backend::run must be called on the main thread");
        self.app.run(mtm);
    }
}

// -----------------------------------------------------------------------------
// NATIVE DELEGATE DEFINITION
// -----------------------------------------------------------------------------

pub struct AppDelegateIvars {
    // Stores the closure until applicationDidFinishLaunching: is called
    bootstrap: RefCell<Option<BootstrapFn>>,
}

define_class!(
    pub struct AppDelegate;

    unsafe impl ClassType for AppDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "UnaAppDelegate";
    }

    impl DefinedClass for AppDelegate {
        type Ivars = AppDelegateIvars;
    }

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[method(applicationDidFinishLaunching:)]
        fn application_did_finish_launching(&self, _notification: &objc2_foundation::NSNotification) {
            let mtm = MainThreadMarker::new().expect("applicationDidFinishLaunching: must run on main thread");

            // Construct the root NSWindow
            let content_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 800.0));
            let style_mask = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::Miniaturizable;

            let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc::<NSWindow>(),
                content_rect,
                style_mask,
                objc2_app_kit::NSBackingStoreType::Buffered,
                false,
            );

            // We need a title to match our identity
            window.setTitle(&NSString::from_str("UnaOS - Lumen Workspace"));

            // Safely consume the bootstrap function
            let bootstrap_fn = self.ivars().bootstrap.borrow_mut().take()
                .expect("Bootstrap function was already consumed or not set");

            // Execute the bootstrap function, passing the window reference
            let root_view = bootstrap_fn(&window);

            // Set the view returned by the cross-platform bootstrap as the window's content
            window.setContentView(Some(&root_view));

            // Bring the window to the front
            window.makeKeyAndOrderFront(None);

            // Also ensure the app is brought to front
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        }

        #[method(applicationShouldTerminateAfterLastWindowClosed:)]
        fn application_should_terminate_after_last_window_closed(&self, _sender: &NSApplication) -> bool {
            true
        }
    }
);

impl AppDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(AppDelegateIvars {
            bootstrap: RefCell::new(None),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    pub fn set_bootstrap(&self, f: BootstrapFn) {
        *self.ivars().bootstrap.borrow_mut() = Some(f);
    }
}
