use crate::AppHandler;

// Shared imports
#[allow(unused_imports)]
use crate::{Event, KeyCode};
#[allow(unused_imports)]
use std::cell::RefCell;
#[allow(unused_imports)]
use std::rc::Rc;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, DisplayHandle, WindowHandle,
};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

#[cfg(target_os = "macos")]
use raw_window_handle::{AppKitDisplayHandle, AppKitWindowHandle};

#[cfg(target_os = "windows")]
use raw_window_handle::Win32WindowHandle;

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::*;
    use gtk4::{prelude::*, EventControllerKey};
    use libadwaita as adw;
    use adw::prelude::*;
    use glib;

    pub struct EventLoop<H: AppHandler + 'static> {
        app: adw::Application,
        _marker: std::marker::PhantomData<H>,
    }

    impl<H: AppHandler + 'static> EventLoop<H> {
        pub fn new() -> Self {
            let app = adw::Application::builder()
                .application_id("org.unaos.vein")
                .build();
            Self {
                app,
                _marker: std::marker::PhantomData,
            }
        }

        pub fn run(self, handler: H) -> Result<(), String> {
            // Wrap handler for shared access in closures
            let handler_rc = Rc::new(RefCell::new(handler));

            self.app.connect_activate(move |app| {
                let window = adw::ApplicationWindow::builder()
                    .application(app)
                    .title("Vein")
                    .default_width(800)
                    .default_height(600)
                    .build();

                let toolbar_view = adw::ToolbarView::new();
                let header_bar = adw::HeaderBar::new();
                toolbar_view.add_top_bar(&header_bar);

                let text_view = gtk4::TextView::builder()
                    .editable(false)
                    .monospace(true)
                    .wrap_mode(gtk4::WrapMode::WordChar)
                    .build();

                // Set Initial Text
                text_view.buffer().set_text(&handler_rc.borrow().view());

                toolbar_view.set_content(Some(&text_view));
                window.set_content(Some(&toolbar_view));

                // Input Handling
                let key_controller = EventControllerKey::new();
                let h_input = handler_rc.clone();
                key_controller.connect_key_pressed(move |_controller, keyval, _keycode, _state| {
                    let mut h = h_input.borrow_mut();
                    let mut handled = false;

                    match keyval {
                        gtk4::gdk::Key::Return | gtk4::gdk::Key::KP_Enter | gtk4::gdk::Key::ISO_Enter => {
                            h.handle_event(Event::KeyDown(KeyCode::Enter));
                            handled = true;
                        }
                        gtk4::gdk::Key::BackSpace => {
                            h.handle_event(Event::KeyDown(KeyCode::Backspace));
                            handled = true;
                        }
                        _ => {
                            if let Some(c) = keyval.to_unicode() {
                                if !c.is_control() {
                                    h.handle_event(Event::Char(c));
                                    handled = true;
                                }
                            }
                        }
                    }

                    if handled {
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                });
                window.add_controller(key_controller);

                // Tick Loop (Visual Updates)
                let h_tick = handler_rc.clone();
                let buffer = text_view.buffer();
                glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
                    let mut h = h_tick.borrow_mut();

                    // Timer Event (e.g. for cursor blink)
                    h.handle_event(Event::Timer);

                    // Update View if changed
                    let new_text = h.view();
                    let start = buffer.start_iter();
                    let end = buffer.end_iter();
                    let current_text = buffer.text(&start, &end, false);

                    if current_text != new_text {
                        buffer.set_text(&new_text);
                    }

                    glib::ControlFlow::Continue
                });

                window.present();
            });

            self.app.run_with_args::<&str>(&[]);
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux_impl::EventLoop;

#[cfg(target_os = "macos")]
mod mac_impl {
    use super::*;
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, msg_send_id, ClassType, MainThreadMarker, sel};
    use objc2_app_kit::{
        NSApplication, NSApplicationDelegate, NSWindow, NSWindowStyleMask, NSBackingStoreType,
        NSApplicationActivationPolicy, NSColor, NSView,
        NSGraphicsContext, NSDeviceRGBColorSpace
    };
    use objc2_foundation::{NSNotification, NSString, NSPoint, NSSize, NSRect, NSObject, NSObjectProtocol, NSTimer, NSDate};

    // Separate Window Struct
    pub struct Window {
        window: Retained<NSWindow>,
    }

    impl Window {
        pub fn new(mtm: MainThreadMarker) -> Self {
            let rect = NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(800.0, 600.0));
            let window = unsafe {
                let w = NSWindow::alloc(mtm);
                w.initWithContentRect_styleMask_backing_defer(
                    rect,
                    NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable | NSWindowStyleMask::Miniaturizable,
                    NSBackingStoreType::Buffered,
                    false
                )
            };
            unsafe {
                 window.setTitle(Some(&NSString::from_str("UnaOS :: Vein (Mac Native)")));
                 window.makeKeyAndOrderFront(None);
                 window.setBackgroundColor(Some(&NSColor::blackColor()));

                 // Create and set custom view
                 let view_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
                 let view = GneissView::alloc(mtm).initWithFrame(view_rect);
                 window.setContentView(Some(&view));
            }
            Self { window }
        }
    }

    impl HasDisplayHandle for Window {
         fn display_handle(&self) -> Result<DisplayHandle<'_>, raw_window_handle::HandleError> {
             let handle = AppKitDisplayHandle::new();
             unsafe { DisplayHandle::borrow_raw(RawDisplayHandle::AppKit(handle)) }
        }
    }

    impl HasWindowHandle for Window {
        fn window_handle(&self) -> Result<WindowHandle<'_>, raw_window_handle::HandleError> {
             Err(raw_window_handle::HandleError::Unavailable)
        }
    }

    pub struct EventLoop<H: AppHandler + 'static> {
        handler: Option<H>,
    }

    static mut GLOBAL_HANDLER: Option<Rc<RefCell<dyn AppHandler>>> = None;
    static mut GLOBAL_WINDOW: Option<Retained<NSWindow>> = None;

    define_class!(
        #[unsafe(super(NSView))]
        #[name = "GneissView"]
        struct GneissView;

        impl GneissView {
             #[unsafe(method(initWithFrame:))]
             fn init_with_frame(this: &mut Self, frame_rect: NSRect) -> Option<&mut Self> {
                 unsafe { msg_send![super(this), initWithFrame: frame_rect] }
             }

             #[unsafe(method(drawRect:))]
             fn draw_rect(&self, dirty_rect: NSRect) {
                 // Stub: No drawing
             }
        }
    );

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "GneissAppDelegate"]
        struct AppDelegate;

        unsafe impl NSApplicationDelegate for AppDelegate {
            #[unsafe(method(applicationDidFinishLaunching:))]
            fn application_did_finish_launching(&self, _notification: &NSNotification) {
                let mtm = MainThreadMarker::new().expect("Must be on main thread");
                let app = NSApplication::sharedApplication(mtm);
                app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

                let win_struct = Window::new(mtm);

                unsafe {
                    GLOBAL_WINDOW = Some(win_struct.window.clone());
                }

                app.activateIgnoringOtherApps(true);
            }
        }
    );

    impl<H: AppHandler + 'static> EventLoop<H> {
        pub fn new() -> Self {
            Self { handler: None }
        }

        pub fn run(self, handler: H) -> Result<(), String> {
            let mtm = MainThreadMarker::new().expect("Must be on main thread");
            let app = NSApplication::sharedApplication(mtm);

            unsafe {
                GLOBAL_HANDLER = Some(Rc::new(RefCell::new(handler)));
            }

            let delegate = AppDelegate::alloc(mtm).init();
            app.setDelegate(delegate.as_ref().map(|d| d as &ProtocolObject<dyn NSApplicationDelegate>));
            unsafe { app.run() };
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac_impl::EventLoop;

#[cfg(target_os = "windows")]
mod win_impl {
    use super::*;
    use windows::{
        core::*,
        Win32::Foundation::*,
        Win32::System::LibraryLoader::GetModuleHandleW,
        Win32::UI::WindowsAndMessaging::*,
        Win32::Graphics::Gdi::*,
    };
    use std::ffi::c_void;
    use std::num::NonZeroIsize;

    pub struct Window {
        hwnd: HWND,
    }

    impl Window {
        pub fn new(hwnd: HWND) -> Self {
            Self { hwnd }
        }
    }

    impl HasDisplayHandle for Window {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, raw_window_handle::HandleError> {
             let handle = raw_window_handle::WindowsDisplayHandle::new();
             unsafe { DisplayHandle::borrow_raw(RawDisplayHandle::Windows(handle)) }
        }
    }

    impl HasWindowHandle for Window {
        fn window_handle(&self) -> Result<WindowHandle<'_>, raw_window_handle::HandleError> {
             let nzhwnd = NonZeroIsize::new(self.hwnd.0).ok_or(raw_window_handle::HandleError::Unavailable)?;
             let mut handle = Win32WindowHandle::new(nzhwnd);
             let hinstance = unsafe { GetModuleHandleW(None).unwrap_or(HMODULE(0)) };
             let nzhinstance = NonZeroIsize::new(hinstance.0).map(|v| handle.hinstance = Some(v));
             unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) }
        }
    }

    static mut GLOBAL_HANDLER: Option<Rc<RefCell<dyn AppHandler>>> = None;

    pub struct EventLoop<H: AppHandler + 'static> {
        handler: Option<H>,
    }

    impl<H: AppHandler + 'static> EventLoop<H> {
        pub fn new() -> Self {
            Self { handler: None }
        }

        pub fn run(self, handler: H) -> Result<(), String> {
            unsafe {
                let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
                let class_name = w!("GneissAppClass");

                let wc = WNDCLASSW {
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap(),
                    hInstance: instance.into(),
                    lpszClassName: class_name,
                    lpfnWndProc: Some(wnd_proc),
                    hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                    ..Default::default()
                };

                RegisterClassW(&wc);
                GLOBAL_HANDLER = Some(Rc::new(RefCell::new(handler)));

                let hwnd = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    class_name,
                    w!("UnaOS :: Vein (Win Native)"),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    800,
                    600,
                    None,
                    None,
                    instance,
                    None,
                );

                let mut message = MSG::default();
                while GetMessageW(&mut message, None, 0, 0).into() {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            Ok(())
        }
    }

    extern "system" fn wnd_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe {
            match message {
                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                WM_PAINT => {
                    // Stub: No drawing
                    let mut ps = PAINTSTRUCT::default();
                    BeginPaint(window, &mut ps);
                    EndPaint(window, &ps);
                    LRESULT(0)
                }
                _ => DefWindowProcW(window, message, wparam, lparam),
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use win_impl::EventLoop;
