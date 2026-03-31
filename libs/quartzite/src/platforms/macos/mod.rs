// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

pub mod spline;
pub mod toolbar;
pub mod workspace;

use crate::{NativeView, NativeWindow};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicyRegular, NSApplicationDelegate, NSResponder, NSWindow, NSWindowStyleMask};
use objc2_foundation::{MainThreadOnly, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;

// The UI Window Bootstrap Closure signature matching core expectations
type BootstrapFn = Box<dyn FnOnce(&NativeWindow) -> NativeView + 'static>;

// Global state to store the closure so the AppDelegate can claim it during `applicationDidFinishLaunching:`
// We use a thread_local because UI initialization strictly happens on the main thread.
thread_local! {
    static BOOTSTRAP_CLOSURE: RefCell<Option<BootstrapFn>> = RefCell::new(None);
}

pub struct Backend;

impl Backend {
    pub fn new<F>(app_id: &str, bootstrap: F) -> Self
    where
        F: FnOnce(&NativeWindow) -> NativeView + 'static,
    {
        // Setup standard application environment
        unsafe {
            let app = NSApplication::sharedApplication();
            app.setActivationPolicy(NSApplicationActivationPolicyRegular);
        }

        BOOTSTRAP_CLOSURE.with(|b| {
            *b.borrow_mut() = Some(Box::new(bootstrap));
        });

        Backend
    }

    pub fn run(&self) {
        unsafe {
            let app = NSApplication::sharedApplication();
            let delegate = AppDelegate::new();
            app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

            // Relinquish control to the OS Run Loop
            app.run();
        }
    }
}

// -----------------------------------------------------------------------------
// APP DELEGATE
// -----------------------------------------------------------------------------

pub struct AppDelegateIvars {
    pub window: RefCell<Option<Retained<NSWindow>>>,
    pub content_view: RefCell<Option<Retained<objc2_app_kit::NSView>>>,
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumenAppDelegate"]
    #[ivars = AppDelegateIvars]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn applicationDidFinishLaunching(&self, _notification: &objc2_foundation::NSNotification) {
            let mtm = MainThreadOnly::new().unwrap();

            unsafe {
                // Construct the Main Window
                let content_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1200.0, 800.0));
                let style_mask = NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::FullSizeContentView;

                let window: Retained<NSWindow> = msg_send![
                    NSWindow::class(),
                    alloc
                ];
                let window: Retained<NSWindow> = msg_send![
                    window,
                    initWithContentRect: content_rect,
                    styleMask: style_mask,
                    backing: objc2_app_kit::NSBackingStoreBuffered,
                    defer: false
                ];

                window.setTitle(&NSString::from_str("Lumen Workspace"));
                window.setTitlebarAppearsTransparent(true);
                window.setMovableByWindowBackground(true);

                // Wire up the Toolbar
                toolbar::attach_toolbar(&window, mtm);

                // Bootstrap the inner UI content from Core
                let content_view = BOOTSTRAP_CLOSURE.with(|b| {
                    if let Some(bootstrap) = b.borrow_mut().take() {
                        bootstrap(&window)
                    } else {
                        panic!("CRITICAL: Bootstrap closure missing during AppDelegate initialization.");
                    }
                });

                window.setContentView(Some(&content_view));
                window.makeKeyAndOrderFront(None::<&objc2::runtime::AnyObject>);

                // Store retained pointers in ivars to prevent deallocation
                *self.ivars().window.borrow_mut() = Some(window);
                *self.ivars().content_view.borrow_mut() = Some(content_view);
            }
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn applicationShouldTerminateAfterLastWindowClosed(&self, _sender: &NSApplication) -> bool {
            true
        }
    }
);

impl AppDelegate {
    pub fn new() -> Retained<Self> {
        let mtm = MainThreadOnly::new().unwrap();
        let this = Self::alloc().set_ivars(AppDelegateIvars {
            window: RefCell::new(None),
            content_view: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }
}
