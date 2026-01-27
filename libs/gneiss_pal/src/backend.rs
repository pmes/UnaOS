use crate::AppHandler;

// Shared imports
#[allow(unused_imports)]
use crate::{Event, KeyCode, DashboardState, ViewMode};
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
                    .default_width(1000)
                    .default_height(700)
                    .build();

                let toolbar_view = adw::ToolbarView::new();
                let header_bar = adw::HeaderBar::new();
                toolbar_view.add_top_bar(&header_bar);

                // --- LAYOUT STRUCTURE ---
                // Root Horizontal Box
                let main_box = gtk4::Box::builder()
                    .orientation(gtk4::Orientation::Horizontal)
                    .spacing(0)
                    .build();

                // 1. LEFT PANE (Navigation)
                let left_box = gtk4::Box::builder()
                    .orientation(gtk4::Orientation::Vertical)
                    .width_request(200)
                    .css_classes(vec!["sidebar"]) // Adwaita style class
                    .build();

                let nav_list = gtk4::ListBox::builder()
                    .selection_mode(gtk4::SelectionMode::Single)
                    .build();

                // Add Scroll for List
                let left_scroll = gtk4::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk4::PolicyType::Never)
                    .child(&nav_list)
                    .vexpand(true)
                    .build();

                left_box.append(&left_scroll);
                main_box.append(&left_box);

                // Separator
                main_box.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

                // 2. CENTER PANE (Stack)
                let center_stack = gtk4::Stack::new();
                center_stack.set_hexpand(true);

                // Page 1: Comms (TextView)
                let text_view = gtk4::TextView::builder()
                    .editable(false)
                    .monospace(true)
                    .wrap_mode(gtk4::WrapMode::WordChar)
                    .bottom_margin(20)
                    .top_margin(20)
                    .left_margin(20)
                    .right_margin(20)
                    .build();

                let text_scroll = gtk4::ScrolledWindow::builder()
                    .child(&text_view)
                    .build();

                center_stack.add_named(&text_scroll, Some("comms"));

                // Page 2: Wolfpack (Label for now)
                let wolfpack_label = gtk4::Label::builder()
                    .label("Wolfpack Grid System - OFFLINE")
                    .css_classes(vec!["title-1"])
                    .build();

                center_stack.add_named(&wolfpack_label, Some("wolfpack"));

                main_box.append(&center_stack);

                // Separator
                main_box.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

                // 3. RIGHT PANE (Actions)
                let right_box = gtk4::Box::builder()
                    .orientation(gtk4::Orientation::Vertical)
                    .width_request(200)
                    .spacing(10)
                    .margin_top(10)
                    .margin_bottom(10)
                    .margin_start(10)
                    .margin_end(10)
                    .build();

                main_box.append(&right_box);

                // Set Content
                toolbar_view.set_content(Some(&main_box));
                window.set_content(Some(&toolbar_view));


                // --- INPUT HANDLING ---
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

                // --- SIGNAL HANDLING (Nav List) ---
                let h_nav = handler_rc.clone();
                nav_list.connect_row_activated(move |_list, row| {
                     let idx = row.index();
                     if idx >= 0 {
                         h_nav.borrow_mut().handle_event(Event::Nav(idx as usize));
                     }
                });

                // --- RENDER LOOP ---
                let h_tick = handler_rc.clone();
                let text_buffer = text_view.buffer();

                // Track previous state to optimize updates
                // Note: We can't easily store full state in closure without refcells,
                // so we'll just check specific things or rebuild cheap things.
                // For strings, we check. For lists, we might rebuild if different length/content.

                // Helper to manage action buttons closure state
                let current_actions = Rc::new(RefCell::new(Vec::<String>::new()));
                let current_navs = Rc::new(RefCell::new(Vec::<String>::new()));

                glib::timeout_add_local(std::time::Duration::from_millis(32), move || {
                    let mut h = h_tick.borrow_mut();

                    // Timer Event
                    h.handle_event(Event::Timer);

                    // Get View State
                    let state = h.view();

                    // 1. Sync Left (Nav)
                    let mut navs_cache = current_navs.borrow_mut();
                    if *navs_cache != state.nav_items {
                        // Rebuild Nav
                        while let Some(child) = nav_list.first_child() {
                            nav_list.remove(&child);
                        }
                        for item_text in &state.nav_items {
                             let label = gtk4::Label::new(Some(item_text));
                             label.set_margin_start(10);
                             label.set_margin_end(10);
                             label.set_margin_top(10);
                             label.set_margin_bottom(10);
                             label.set_xalign(0.0);
                             nav_list.append(&label);
                        }
                        *navs_cache = state.nav_items.clone();
                    }

                    // Sync Selection
                    if let Some(row) = nav_list.row_at_index(state.active_nav_index as i32) {
                        if !row.is_selected() {
                            nav_list.select_row(Some(&row));
                        }
                    }

                    // 2. Sync Center (Stack & Text)
                    match state.mode {
                        ViewMode::Comms => {
                            if center_stack.visible_child_name().as_deref() != Some("comms") {
                                center_stack.set_visible_child_name("comms");
                            }
                            // Update Text
                            let start = text_buffer.start_iter();
                            let end = text_buffer.end_iter();
                            let current_text = text_buffer.text(&start, &end, false);
                            if current_text != state.console_output {
                                text_buffer.set_text(&state.console_output);
                                // Scroll to bottom?
                                // Usually handled by setting iter to end and placing cursor
                            }
                        },
                        ViewMode::Wolfpack => {
                            if center_stack.visible_child_name().as_deref() != Some("wolfpack") {
                                center_stack.set_visible_child_name("wolfpack");
                            }
                        }
                    }

                    // 3. Sync Right (Actions)
                    let mut actions_cache = current_actions.borrow_mut();
                    if *actions_cache != state.actions {
                        // Rebuild Actions
                        while let Some(child) = right_box.first_child() {
                            right_box.remove(&child);
                        }
                        for (i, action_text) in state.actions.iter().enumerate() {
                            let btn = gtk4::Button::with_label(action_text);
                            btn.set_height_request(50);

                            // Wire Click
                            let h_btn = h_tick.clone(); // Clone the RC, not the ref
                             // Wait, we are borrowing h_tick (h) right now. We cannot clone it easily inside the loop to pass to signal?
                             // Actually, we are inside the closure of timeout_add_local.
                             // `h` is a RefMut. `h_tick` is the Rc<RefCell>.
                             // We need to pass a clone of the Rc to the button signal.
                             // BUT we are currently borrowing it mutably via `h`.
                             // If we attach the signal now, the signal handler won't run until main loop, so the borrow will be dropped.
                             // So it is safe to clone the Rc.

                             // Problem: We can't access `h_tick` inside the closure easily if we move it?
                             // `h_tick` is already moved into the timeout closure.
                             // We need to clone it *outside* the loop? No, the loop runs repeatedly.
                             // We can clone `h_tick` (the Rc) inside the loop? Yes, Rc::clone(&h_tick).

                             // However, `h_tick` is captured by the closure.
                             // Is it captured by value or ref? `move ||` -> by value.

                             // So we can clone it.

                             // WAIT. We are currently borrowing `h_tick` as `h`.
                             // `h` is `RefMut`.
                             // We can clone `h_tick` (the Rc) safely even if borrowed, as long as we don't borrow it again in this scope.
                             // The button callback runs LATER.

                            let h_action = Rc::clone(&h_tick); // Clone the Rc

                            btn.connect_clicked(move |_| {
                                h_action.borrow_mut().handle_event(Event::Action(i));
                            });

                            right_box.append(&btn);
                        }
                        *actions_cache = state.actions.clone();
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
                 // Stub
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
