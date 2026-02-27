#![cfg(target_os = "macos")]

use objc2::{define_class, msg_send, MainThreadOnly};
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSWindow, NSWindowStyleMask};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSRect, NSPoint, NSSize};
use std::cell::RefCell;

use crate::{NativeWindow, NativeView};

// -----------------------------------------------------------------------------
// THE IGNITION CHAMBER (THREAD LOCAL STORAGE)
// -----------------------------------------------------------------------------
thread_local! {
    static BOOTSTRAP_CLOSURE: RefCell<Option<Box<dyn FnOnce(&NativeWindow) -> NativeView>>> = RefCell::new(None);
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
        #[allow(non_snake_case)]
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn applicationDidFinishLaunching(&self, _notification: &NSObject) {
            println!("[UnaOS::Quartzite] macOS Application Runloop Ignited (objc2 0.6).");

            // 1. The engine is awake.
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
            let window: Retained<NSWindow> = unsafe {
                msg_send![
                    mtm.alloc::<NSWindow>(),
                    initWithContentRect: content_rect,
                    styleMask: style,
                    backing: 2usize, // NSBackingStoreBuffered
                    defer: false
                ]
            };

            window.setTitle(&objc2_foundation::NSString::from_str("Vein (Trinity)"));
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
        }

        #[allow(non_snake_case)]
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(&self, _sender: &NSApplication) -> bool {
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
}

impl Backend {
    pub fn new<F>(_app_id: &str, bootstrap_fn: F) -> Self
    where
        F: FnOnce(&NativeWindow) -> NativeView + 'static
    {
        BOOTSTRAP_CLOSURE.with(|b| {
            *b.borrow_mut() = Some(Box::new(bootstrap_fn));
        });

        let mtm = MainThreadMarker::new().expect("Backend::new must be on main thread");
        let app = NSApplication::sharedApplication(mtm);

        unsafe {
            // S41 Fix: setActivationPolicy returns BOOL. We must capture it to satisfy runtime check.
            let success: bool = msg_send![&app, setActivationPolicy: NSApplicationActivationPolicy::Regular];
            if !success {
                println!("[UnaOS::Quartzite] WARNING: Failed to set activation policy.");
            }
        }

        let delegate = UnaAppDelegate::new(mtm);

        unsafe {
            let _: () = msg_send![&app, setDelegate: &*delegate];
        }

        Backend { _app: app }
    }

    pub fn run(&self) {
        unsafe {
            let _: () = msg_send![&self._app, run];
        }
    }
}
