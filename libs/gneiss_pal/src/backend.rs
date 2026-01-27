use crate::AppHandler;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use crate::{Event, KeyCode};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::cell::RefCell;
#[cfg(any(target_os = "macos", target_os = "windows"))]
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
    use gtk4::prelude::*;
    use libadwaita as adw;
    use adw::prelude::*;

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

        pub fn run(self, _handler: H) -> Result<(), String> {
            self.app.connect_activate(|app| {
                let window = adw::ApplicationWindow::builder()
                    .application(app)
                    .title("Vein")
                    .default_width(800)
                    .default_height(600)
                    .build();

                let header_bar = gtk4::HeaderBar::new();
                window.set_titlebar(Some(&header_bar));

                let text_view = gtk4::TextView::builder()
                    .editable(false)
                    .build();
                text_view.buffer().set_text("System Check: Native GTK Widgets Active.");

                window.set_content(Some(&text_view));
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
        NSApplicationActivationPolicy, NSColor, NSView, NSImage, NSBitmapImageRep,
        NSGraphicsContext, NSDeviceRGBColorSpace, NSBitmapFormat
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
    static mut GLOBAL_BUFFER: Option<Vec<u32>> = None;
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
                 unsafe {
                     if let Some(buffer) = &mut GLOBAL_BUFFER {
                         let bounds = self.bounds();
                         let width = bounds.size.width as i32;
                         let height = bounds.size.height as i32;

                         if width > 0 && height > 0 && buffer.len() >= (width * height) as usize {

                             let mut planes_ptr: *mut u8 = buffer.as_mut_ptr() as *mut u8;
                             let planes: *mut *mut u8 = &mut planes_ptr;

                             // Alloc NSBitmapImageRep
                             let rep = NSBitmapImageRep::alloc(MainThreadMarker::new().unwrap_unchecked());

                             // initWithBitmapDataPlanes
                             let rep = rep.initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
                                 Some(planes),
                                 width as isize,
                                 height as isize,
                                 8,
                                 4,
                                 true,
                                 false,
                                 NSDeviceRGBColorSpace, // "NSDeviceRGBColorSpace"
                                 NSBitmapFormat::NSBitmapFormatAlphaFirst, // ARGB usually means Alpha First if little endian?
                                 // Gneiss uses 0xAARRGGBB.
                                 // Win32 GDI expects 0x00RRGGBB.
                                 // Let's assume standard RGB.
                                 (width * 4) as isize,
                                 32
                             );

                             if let Some(rep) = rep {
                                 // Create Image
                                 let image = NSImage::alloc(MainThreadMarker::new().unwrap_unchecked());
                                 let size = NSSize::new(bounds.size.width, bounds.size.height);
                                 let image = image.initWithSize(size);
                                 image.addRepresentation(&rep);

                                 // Draw
                                 image.drawInRect(bounds);
                             }
                         }
                     }
                 }
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

                unsafe {
                    let _: Retained<NSTimer> = NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                        0.016,
                        self,
                        sel!(onTimer:),
                        None,
                        true
                    );
                }

                app.activateIgnoringOtherApps(true);
            }
        }

        impl AppDelegate {
            #[unsafe(method(onTimer:))]
            fn on_timer(&self, _timer: &NSTimer) {
                unsafe {
                    if let (Some(handler), Some(buffer), Some(window)) = (&GLOBAL_HANDLER, &mut GLOBAL_BUFFER, &GLOBAL_WINDOW) {
                        let mut h = handler.borrow_mut();
                        h.handle_event(Event::Timer);

                        let view = window.contentView().unwrap();
                        let frame = view.frame();
                        let w = frame.size.width as u32;
                        let h = frame.size.height as u32;

                        if w > 0 && h > 0 {
                             if buffer.len() != (w * h) as usize {
                                 buffer.resize((w * h) as usize, 0);
                             }
                             h.draw(buffer, w, h);

                             // Trigger drawRect
                             view.setNeedsDisplay(true);
                        }
                    }
                }
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
                GLOBAL_BUFFER = Some(vec![0u32; 800 * 600]);
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
    static mut GLOBAL_BUFFER: Option<Vec<u32>> = None;
    static mut GLOBAL_WINDOW_STRUCT: Option<Window> = None;

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
                GLOBAL_BUFFER = Some(vec![0u32; 800 * 600]);

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

                GLOBAL_WINDOW_STRUCT = Some(Window::new(hwnd));

                SetTimer(hwnd, 1, 16, None);

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
                    let mut ps = PAINTSTRUCT::default();
                    let hdc = BeginPaint(window, &mut ps);
                    let mut rect = RECT::default();
                    GetClientRect(window, &mut rect);
                    let width = (rect.right - rect.left) as u32;
                    let height = (rect.bottom - rect.top) as u32;

                    if width > 0 && height > 0 {
                        if let (Some(handler_rc), Some(buffer)) = (&GLOBAL_HANDLER, &mut GLOBAL_BUFFER) {
                             if buffer.len() != (width * height) as usize {
                                 buffer.resize((width * height) as usize, 0);
                             }
                             handler_rc.borrow_mut().draw(buffer, width, height);

                             let bmi = BITMAPINFO {
                                bmiHeader: BITMAPINFOHEADER {
                                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                                    biWidth: width as i32,
                                    biHeight: -(height as i32),
                                    biPlanes: 1,
                                    biBitCount: 32,
                                    biCompression: BI_RGB,
                                    ..Default::default()
                                },
                                ..Default::default()
                            };

                            StretchDIBits(
                                hdc, 0, 0, width as i32, height as i32, 0, 0, width as i32, height as i32,
                                Some(buffer.as_ptr() as *const c_void), &bmi, DIB_RGB_COLORS, SRCCOPY
                            );
                        }
                    }
                    EndPaint(window, &ps);
                    LRESULT(0)
                }
                WM_TIMER => {
                     if let Some(handler_rc) = &GLOBAL_HANDLER {
                         handler_rc.borrow_mut().handle_event(Event::Timer);
                         InvalidateRect(window, None, FALSE);
                     }
                     LRESULT(0)
                }
                WM_KEYDOWN => {
                     if let Some(handler_rc) = &GLOBAL_HANDLER {
                         let key = match wparam.0 as u8 {
                             0x0D => Some(KeyCode::Enter),
                             0x08 => Some(KeyCode::Backspace),
                             _ => None
                         };
                         if let Some(k) = key {
                             handler_rc.borrow_mut().handle_event(Event::KeyDown(k));
                             InvalidateRect(window, None, FALSE);
                         }
                     }
                     LRESULT(0)
                }
                WM_CHAR => {
                    if let Some(handler_rc) = &GLOBAL_HANDLER {
                        if let Some(c) = std::char::from_u32(wparam.0 as u32) {
                            if !c.is_control() {
                                 handler_rc.borrow_mut().handle_event(Event::Char(c));
                                 InvalidateRect(window, None, FALSE);
                            }
                        }
                    }
                    LRESULT(0)
                }
                _ => DefWindowProcW(window, message, wparam, lparam),
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use win_impl::EventLoop;
