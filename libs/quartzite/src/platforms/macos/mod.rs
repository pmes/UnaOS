// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSWindow, NSView,
    NSResponder
};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol};
use std::cell::RefCell;

pub mod spline;
pub mod window_chrome;
pub mod workspace;

// The UI bootstrapping closure
type BootstrapFn = Box<dyn FnOnce(&NSWindow) -> Retained<NSView> + 'static>;

// -----------------------------------------------------------------------------
// APP DELEGATE
// -----------------------------------------------------------------------------
struct AppDelegateIvars {
    bootstrap: RefCell<Option<BootstrapFn>>,
    window: RefCell<Option<Retained<NSWindow>>>,
    // Holding the delegate to prevent dropping
    window_delegate: RefCell<Option<Retained<window_chrome::WindowDelegate>>>,
    toolbar_delegate: RefCell<Option<Retained<window_chrome::ToolbarDelegate>>>,
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaAppDelegate"]
    #[ivars = AppDelegateIvars]
    struct AppDelegate;

    unsafe impl AppDelegate {
        #[unsafe(method(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(AppDelegateIvars {
                bootstrap: RefCell::new(None),
                window: RefCell::new(None),
                window_delegate: RefCell::new(None),
                toolbar_delegate: RefCell::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn application_did_finish_launching(&self, _notification: &AnyObject) {
            let mtm = MainThreadMarker::from(self);

            // 1. Create the native window through our chrome abstraction
            let (window, window_delegate, toolbar_delegate) = window_chrome::create_window(mtm);

            // 2. Invoke the bootstrap closure to get the root view
            if let Some(bootstrap_fn) = self.ivars().bootstrap.borrow_mut().take() {
                let root_view = bootstrap_fn(&window);
                window.setContentView(Some(&root_view));
            }

            // 3. Keep references alive
            *self.ivars().window.borrow_mut() = Some(window.clone());
            *self.ivars().window_delegate.borrow_mut() = Some(window_delegate);
            *self.ivars().toolbar_delegate.borrow_mut() = Some(toolbar_delegate);

            // 4. Show the window
            window.makeKeyAndOrderFront(None::<&AnyObject>);
            let _: () = msg_send![&window, center];
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn application_should_terminate_after_last_window_closed(
            &self,
            _sender: &NSApplication,
        ) -> objc2::runtime::Bool {
            objc2::runtime::Bool::YES
        }
    }
);

unsafe impl NSObjectProtocol for AppDelegate {}

// -----------------------------------------------------------------------------
// BACKEND
// -----------------------------------------------------------------------------
pub struct Backend {
    delegate: Retained<AppDelegate>,
}

impl Backend {
    pub fn new<F>(_app_id: &str, bootstrap: F) -> Self
    where
        F: FnOnce(&NSWindow) -> Retained<NSView> + 'static,
    {
        // Allocate and initialize the custom delegate
        let delegate: Allocated<AppDelegate> = unsafe { msg_send![AppDelegate::class(), alloc] };
        let delegate: Retained<AppDelegate> = unsafe { msg_send![delegate, init] };

        // Attach the bootstrap closure
        *delegate.ivars().bootstrap.borrow_mut() = Some(Box::new(bootstrap));

        Self { delegate }
    }

    pub fn run(&self) {
        let mtm = MainThreadMarker::new().expect("Must run on main thread");
        let app = NSApplication::sharedApplication(mtm);

        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.setDelegate(Some(ProtocolObject::from_ref(&*self.delegate)));

        // Hand over control to AppKit
        unsafe { msg_send![&app, run] };
    }
}
