#![cfg(target_os = "macos")]

use objc2::{declare_class, msg_send, msg_send_id, ClassType};
use objc2::mutability::MainThreadOnly;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSWindow, NSWindowStyleMask, NSWindowBackingStoreType, NSView};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSRect, NSPoint, NSSize};
use objc2::rc::Retained;
use std::cell::RefCell;
use std::rc::Rc;

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
        #[inherits(NSObject)]
        type Super = NSObject;
        type Mutability = MainThreadOnly;
        const NAME = "UnaAppDelegate";
    }

    unsafe impl UnaAppDelegate {}

    unsafe impl NSApplicationDelegate for UnaAppDelegate {
        #[method(applicationDidFinishLaunching:)]
        unsafe fn applicationDidFinishLaunching(&self, _notification: &NSObject) {
            println!("[UnaOS::Quartzite] macOS Application Runloop Ignited.");

            // 1. The engine is awake. Create the NativeWindow (NSWindow).
            let mtm = MainThreadMarker::new().expect("Must be on main thread");

            // Coordinates: (0, 0) is bottom-left on macOS. We center it later.
            let content_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1200.0, 800.0));
            let style = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable;

            let window = unsafe {
                let alloc: Retained<NSWindow> = msg_send![NSWindow::class(), alloc];

                NSWindow::initWithContentRect_styleMask_backing_defer(
                    alloc,
                    content_rect,
                    style,
                    NSWindowBackingStoreType::Buffered, // Corrected Enum
                    false
                )
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
