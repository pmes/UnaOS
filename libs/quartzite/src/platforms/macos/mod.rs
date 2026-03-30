// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! macOS AppKit Embassy
//!
//! The `mod.rs` acts as the `Backend` bridge and `NSApplicationDelegate` definition.
//! We enforce strict Can-Am rules here: precise memory lifetimes, safe `define_class!`
//! structures, and pure native OS UI boundaries.

use std::cell::RefCell;
use std::rc::Rc;
use objc2::rc::Retained;
use objc2::{define_class, msg_send_id};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSBackingStoreType, NSWindow, NSWindowStyleMask, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};

pub mod toolbar;
pub mod spline;
pub mod workspace;

// -----------------------------------------------------------------------------
// UI REFERENCE STORAGE
// -----------------------------------------------------------------------------
// We store strong references to critical pieces of our UI hierarchy to prevent
// them from being deallocated prematurely by Rust's drop checker.
pub struct WorkspaceRefs {
    // We will expand this as needed for Sidebar and Comms components.
    pub root_view: Retained<NSView>,
}

// -----------------------------------------------------------------------------
// APP DELEGATE STATE (IVARS)
// -----------------------------------------------------------------------------
pub struct AppDelegateIvars {
    pub window: RefCell<Option<Retained<NSWindow>>>,
    pub workspace_refs: RefCell<Option<Rc<WorkspaceRefs>>>,
    pub bootstrap_fn: RefCell<Option<Box<dyn FnOnce(&NSWindow) -> Retained<NSView>>>>,
}

// -----------------------------------------------------------------------------
// THE NSAPPLICATION DELEGATE
// -----------------------------------------------------------------------------
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "UnaAppDelegate"]
    #[ivars = AppDelegateIvars]
    pub struct AppDelegate;

    // Conforming to the NSApplicationDelegate protocol
    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &objc2_foundation::NSNotification) {
            let mtm = MainThreadMarker::new().expect("Must be on the main thread");

            // 1. Create the Main Window
            let window_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1024.0, 768.0));
            let style_mask = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::FullSizeContentView;

            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    mtm.alloc(),
                    window_rect,
                    style_mask,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };

            // 2. Configure Window Appearance
            window.setTitle(&NSString::from_str("UnaOS"));
            window.setTitlebarAppearsTransparent(true);
            unsafe { window.setTitleVisibility(objc2_app_kit::NSWindowTitleVisibility::Hidden) };

            // Set minimum size
            window.setMinSize(NSSize::new(800.0, 600.0));

            // Center and make it key
            window.center();
            window.makeKeyAndOrderFront(None);

            // 3. Attach the Toolbar (Window Chrome)
            toolbar::setup_toolbar(&window, mtm);

            // 4. Fire the Bootstrap Function to generate the root UI
            let root_view = if let Some(boot_fn) = self.ivars().bootstrap_fn.borrow_mut().take() {
                boot_fn(&window)
            } else {
                panic!("Bootstrap function was consumed or missing.");
            };

            // 5. Assign the Root View to the Window
            window.setContentView(Some(&root_view));

            // 6. Retain references
            *self.ivars().workspace_refs.borrow_mut() = Some(Rc::new(WorkspaceRefs { root_view }));
            *self.ivars().window.borrow_mut() = Some(window);
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate(&self, _sender: &NSApplication) -> bool {
            true // Standard macOS app behavior for single-window tools
        }
    }
);

impl AppDelegate {
    pub fn new(bootstrap_fn: impl FnOnce(&NSWindow) -> Retained<NSView> + 'static, mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(AppDelegateIvars {
            window: RefCell::new(None),
            workspace_refs: RefCell::new(None),
            bootstrap_fn: RefCell::new(Some(Box::new(bootstrap_fn))),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// -----------------------------------------------------------------------------
// THE BACKEND
// -----------------------------------------------------------------------------
pub struct Backend {
    delegate: Retained<AppDelegate>,
}

impl Backend {
    /// Initializes the AppKit Backend, establishing the application delegate
    /// and retaining the bootstrap function.
    pub fn new<F>(_app_id: &str, bootstrap_fn: F) -> Self
    where
        F: FnOnce(&NSWindow) -> Retained<NSView> + 'static,
    {
        // For macOS, we need a MainThreadMarker.
        // It's safe here because `Backend::new` is always called from `main`.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };

        let delegate = AppDelegate::new(bootstrap_fn, mtm);

        Backend { delegate }
    }

    /// Triggers the NSApplication run loop.
    pub fn run(&self) {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let app = NSApplication::sharedApplication(mtm);
        app.setDelegate(Some(objc2::ProtocolObject::from_ref(&*self.delegate)));
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.run();
    }
}
