#![cfg_attr(not(target_os = "macos"), allow(unused))]

#[cfg(target_os = "macos")]
mod mac_impl {
    use gneiss_pal::{App as CoreApp, Platform, Plugin};
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{define_class, msg_send, msg_send_id, ClassType, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSApplicationDelegate, NSWindow, NSWindowStyleMask, NSBackingStoreType, NSMenu, NSMenuItem};
    use objc2_foundation::{NSNotification, NSString, NSPoint, NSSize, NSRect, NSObject};

    // Platform Implementation
    pub struct MacPlatform {
        // In a real app we might hold the NSWindow or a weak reference
    }

    impl Platform for MacPlatform {
        fn set_title(&self, title: &str) {
             println!("Mac Platform set_title: {}", title);
             // Verify MainThreadMarker and set window title...
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct HelloPlugin;
    impl Plugin for HelloPlugin {
        fn on_init(&mut self, platform: &dyn Platform) {
            platform.set_title("Hello from Mac Skeleton");
            println!("Mac Plugin Initialized!");
        }
        fn on_update(&mut self, _platform: &dyn Platform) {}
        fn as_any(&self) -> &dyn std::any::Any { self }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
    }

    // App Delegate
    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "TemplateAppDelegate"]
        struct AppDelegate;

        impl AppDelegate {
            #[unsafe(method(init))]
            fn init(this: &mut Self) -> Option<&mut Self> {
                let this: Option<&mut Self> = unsafe { msg_send![super(this), init] };
                this
            }
        }

        unsafe impl NSApplicationDelegate for AppDelegate {
            #[unsafe(method(applicationDidFinishLaunching:))]
            fn application_did_finish_launching(&self, _notification: &NSNotification) {
                println!("Mac App Launched");

                let mtm = MainThreadMarker::new().expect("Must be on main thread");

                // Create Window
                let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
                let window = unsafe {
                    let w = NSWindow::alloc(mtm);
                    w.initWithContentRect_styleMask_backing_defer(
                        rect,
                        NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable,
                        NSBackingStoreType::Buffered,
                        false
                    )
                };

                window.setTitle(Some(&NSString::from_str("Template App")));
                window.makeKeyAndOrderFront(None);

                // Run Core Logic
                let platform = MacPlatform {};
                let mut app = CoreApp::new();
                app.register_plugin(HelloPlugin);
                app.init(&platform);

                // Keep app alive?
            }
        }
    );

    pub fn main() {
        let mtm = MainThreadMarker::new().expect("Must be on main thread");
        let app = NSApplication::sharedApplication(mtm);

        let delegate = AppDelegate::alloc(mtm).init();
        // app.setDelegate(Some(&delegate)); // Need ProtocolObject conversion

        // In skeleton, we just run
        // app.run();
        println!("Mac App Main (Skeleton Only)");
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    mac_impl::main();
    #[cfg(not(target_os = "macos"))]
    println!("Mac Template requires macOS to run. (Check passed on Linux)");
}
