#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2::{define_class, msg_send, MainThreadOnly, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSWindow,
    NSWindowStyleMask, NSBackingStoreType,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;
use std::sync::Once;

use crate::{NativeView, NativeWindow};

pub mod spline;

// -----------------------------------------------------------------------------
// THE IGNITION CHAMBER (THREAD LOCAL STORAGE)
// -----------------------------------------------------------------------------
thread_local! {
    static BOOTSTRAP_CLOSURE: RefCell<Option<Box<dyn FnOnce(&NativeWindow) -> NativeView>>> = RefCell::new(None);
    // MACH: We must retain the window explicitly because NSApplication might not hold it strongly enough
    // against the AutoreleasePool if we used a convenience constructor that autoreleases.
    // Although `alloc/init` should be +1, the crash at `objc_release` suggests a double-free or
    // use-after-free. Keeping a strong reference here ensures it lives as long as the thread.
    static WINDOW_HOLDER: RefCell<Option<Retained<NSWindow>>> = RefCell::new(None);
}

// -----------------------------------------------------------------------------
// THE DELEGATE (OBJECTIVE-C FFI)
// -----------------------------------------------------------------------------
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "UnaAppDelegate"]
    struct UnaAppDelegate;

    unsafe impl NSObjectProtocol for UnaAppDelegate {}

    unsafe impl NSApplicationDelegate for UnaAppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn application_did_finish_launching(&self, _notification: &NSObject) {
            println!("[UnaOS::Quartzite] macOS Application Runloop Ignited (objc2 0.6).");

            let mtm = MainThreadMarker::new().expect("Must be on main thread");

            // Coordinates: (0, 0) is bottom-left on macOS.
            let content_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1200.0, 800.0));
            let style = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable;

            // -------------------------------------------------------------------
            // UNAOS THREAD SAFETY MANDATE (APPKIT)
            // -------------------------------------------------------------------
            // ALLOC/INIT returns a Retained object (+1).
            // We must keep it alive.
            let window: Retained<NSWindow> = unsafe {
                msg_send![
                    mtm.alloc::<NSWindow>(),
                    initWithContentRect: content_rect,
                    styleMask: style,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            };

            // MACH: Prevent AppKit from auto-releasing the window when closed.
            // Since we hold a Retained<NSWindow> in WINDOW_HOLDER, we want explicit ownership.
            // If we don't do this, AppKit releases it on close/exit, and then our thread-local destructor
            // releases it again -> Segfault 11.
            unsafe {
                window.setReleasedWhenClosed(false);
            }

            window.setTitle(&NSString::from_str("Vein (Trinity)"));
            window.center();

            // 2. Extract bootstrap closure
            let bootstrap = BOOTSTRAP_CLOSURE.with(|b| b.borrow_mut().take())
                .expect("CRITICAL: Bootstrap closure missing during macOS ignition sequence.");

            // 3. Execute bootstrap
            let root_view = bootstrap(&window);

            // 4. Mount View
            window.setContentView(Some(&root_view));
            window.makeKeyAndOrderFront(None);

            // Activate app
            let app = NSApplication::sharedApplication(mtm);
            unsafe {
                let _: () = msg_send![&app, activateIgnoringOtherApps: true];
            }

            // MACH: Store the window in Thread Local Storage to guarantee its survival.
            WINDOW_HOLDER.with(|w| {
                *w.borrow_mut() = Some(window);
            });
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn application_should_terminate_after_last_window_closed(&self, _sender: &NSApplication) -> bool {
            true
        }
    }
);

impl UnaAppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc();
        unsafe { msg_send![this, init] }
    }
}

// -----------------------------------------------------------------------------
// THE BACKEND IMPLEMENTATION
// -----------------------------------------------------------------------------
pub struct Backend {
    _app: Retained<NSApplication>,
    // S41 Fix: We must retain the delegate because NSApplication.delegate is weak.
    _delegate: Retained<UnaAppDelegate>,
}

impl Backend {
    pub fn new<F>(_app_id: &str, bootstrap_fn: F) -> Self
    where
        F: FnOnce(&NativeWindow) -> NativeView + 'static
    {
        // 1. Store the bootstrap closure for the delegate to pick up later.
        BOOTSTRAP_CLOSURE.with(|b| {
            *b.borrow_mut() = Some(Box::new(bootstrap_fn));
        });

        let mtm = MainThreadMarker::new().expect("Backend::new must be on main thread");
        let app = NSApplication::sharedApplication(mtm);

        // 2. Set Activation Policy (Regular App)
        // CRITICAL FIX: explicit boolean return capture prevents runtime signature mismatch panic.
        unsafe {
            let success: bool = msg_send![&app, setActivationPolicy: NSApplicationActivationPolicy::Regular];
            if !success {
                println!("[UnaOS::Quartzite] WARNING: Failed to set activation policy.");
            }
        }

        // 3. Create and Assign Delegate
        let delegate = UnaAppDelegate::new(mtm);
        unsafe {
            // setDelegate: does not retain, so we must hold `delegate` in `Backend`.
            let _: () = msg_send![&app, setDelegate: &*delegate];
        }

        Backend {
            _app: app,
            _delegate: delegate
        }
    }

    pub fn run(&self) {
        unsafe {
            let _: () = msg_send![&self._app, run];
        }
    }
}
