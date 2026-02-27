#![cfg(target_os = "macos")]

use objc2::{declare_class, msg_send, msg_send_id, ClassType, DeclaredClass};
use objc2::mutability::MainThreadOnly;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSWindow, NSWindowStyleMask};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSRect, NSPoint, NSSize};
use objc2::rc::Retained;
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
declare_class!(
    struct UnaAppDelegate;

    unsafe impl ClassType for UnaAppDelegate {
        type Super = NSObject;
        type Mutability = MainThreadOnly;
        const NAME: &'static str = "UnaAppDelegate";
    }

    impl DeclaredClass for UnaAppDelegate {
        type Ivars = ();
    }

    // Explicitly implement NSObjectProtocol as required by objc2 0.5+
    unsafe impl NSObjectProtocol for UnaAppDelegate {}

    unsafe impl NSApplicationDelegate for UnaAppDelegate {
        #[method(applicationDidFinishLaunching:)]
        unsafe fn applicationDidFinishLaunching(&self, _notification: &NSObject) {
            println!("[UnaOS::Quartzite] macOS Application Runloop Ignited.");

            // 1. The engine is awake. Create the NativeWindow (NSWindow).
            // We re-obtain the MainThreadMarker to satisfy local scope requirements,
            // though self is technically proof of main thread execution in this callback.
            let mtm = MainThreadMarker::new().expect("Must be on main thread");

            // Coordinates: (0, 0) is bottom-left on macOS. We center it later.
            let content_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1200.0, 800.0));
            let style = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable;

            // -------------------------------------------------------------------
            // UNAOS DIRECT RUNTIME INVOCATION (THE CAN-AM WAY)
            // The objc2_app_kit crate strips generated methods if transitive feature
            // flags for their arguments are missing in Cargo.toml.
            // Instead of fighting the wrapper, we bypass it. We use msg_send_id!
            // to send the initialization message directly to the Objective-C runtime.
            // This is raw, zero-overhead execution. No restrictor plates.
            // -------------------------------------------------------------------
            let window: Retained<NSWindow> = unsafe {
                msg_send_id![
                    mtm.alloc::<NSWindow>(),
                    initWithContentRect: content_rect,
                    styleMask: style,
                    backing: 2usize, // NSBackingStoreBuffered = 2 (NSUInteger maps to usize)
                    defer: false
                ]
            };

            window.setTitle(&objc2_foundation::NSString::from_str("Vein (Trinity)"));
            window.center();

            // 2. Extract the bootstrap closure from Thread Local Storage.
            let bootstrap = BOOTSTRAP_CLOSURE.with(|b| b.borrow_mut().take())
                .expect("CRITICAL: Bootstrap closure missing during macOS ignition sequence.");

            // 3. Execute the bootstrap closure, passing the NativeWindow reference.
            let root_view = bootstrap(&window);

            // 4. Mount the NativeView to the NativeWindow.
            window.setContentView(Some(&root_view));

            // Bring the window to the front and make it the key window.
            window.makeKeyAndOrderFront(None);

            // Activate app ignoring other apps (Can-Am style)
            let app = NSApplication::sharedApplication(mtm);
            unsafe {
                let _: () = msg_send![&app, activateIgnoringOtherApps: true];
            }
        }

        #[method(applicationShouldTerminateAfterLastWindowClosed:)]
        unsafe fn should_terminate_after_last_window_closed(&self, _sender: &NSApplication) -> bool {
            true
        }
    }
);

impl UnaAppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc();
        unsafe { msg_send_id![this, init] }
    }
}

// -----------------------------------------------------------------------------
// THE BACKEND IMPLEMENTATION
// -----------------------------------------------------------------------------
pub struct Backend {
    _app: Retained<NSApplication>,
}

impl Backend {
    /// Initializes the macOS AppKit backend.
    pub fn new<F>(_app_id: &str, bootstrap_fn: F) -> Self
    where
        F: FnOnce(&NativeWindow) -> NativeView + 'static
    {
        // 1. Stash the closure in our thread-local ignition chamber.
        BOOTSTRAP_CLOSURE.with(|b| {
            *b.borrow_mut() = Some(Box::new(bootstrap_fn));
        });

        // 2. Obtain the shared NSApplication instance.
        let mtm = MainThreadMarker::new().expect("Backend::new must be on main thread");
        let app = NSApplication::sharedApplication(mtm);

        unsafe {
            let _: () = msg_send![&app, setActivationPolicy: NSApplicationActivationPolicy::Regular];
        }

        // 3. Allocate and set our custom delegate.
        // We use our helper new() to keep it clean
        let delegate = UnaAppDelegate::new(mtm);

        unsafe {
            let _: () = msg_send![&app, setDelegate: &*delegate];
        }

        Backend { _app: app }
    }

    /// Engages the main runloop. This function will not return until the app terminates.
    pub fn run(&self) {
        unsafe {
            // Drop the hammer. Enter the Apple runloop.
            let _: () = msg_send![&self._app, run];
        }
    }
}
