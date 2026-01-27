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
                    .default_width(1100)
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
                    .css_classes(vec!["navigation-sidebar"]) // Adwaita style class
                    .build();

                let nav_list = gtk4::ListBox::builder()
                    .selection_mode(gtk4::SelectionMode::Single)
                    .vexpand(true)
                    .build();

                left_box.append(&nav_list);
                main_box.append(&left_box);

                // Separator
                main_box.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

                // 2. CENTER PANE (The Stage)
                let center_stack = gtk4::Stack::new();
                center_stack.set_hexpand(true);

                // Page 1: COMMS CONSOLE (Split View)
                let comms_box = gtk4::Box::builder()
                    .orientation(gtk4::Orientation::Vertical)
                    .spacing(12)
                    .margin_top(12)
                    .margin_bottom(12)
                    .margin_start(12)
                    .margin_end(12)
                    .build();

                // Top: Output Log
                let output_scroll = gtk4::ScrolledWindow::builder()
                    .vexpand(true)
                    .build();

                let output_view = gtk4::TextView::builder()
                    .editable(false)
                    .cursor_visible(false)
                    .monospace(true)
                    .wrap_mode(gtk4::WrapMode::WordChar)
                    .build();

                output_scroll.set_child(Some(&output_view));
                comms_box.append(&output_scroll);

                // Bottom: Input Area (Card Style)
                let input_card = gtk4::Box::builder()
                    .orientation(gtk4::Orientation::Horizontal)
                    .spacing(8)
                    .css_classes(vec!["card"])
                    .build();

                let input_view = gtk4::TextView::builder()
                    .wrap_mode(gtk4::WrapMode::WordChar)
                    .accepts_tab(false)
                    .hexpand(true)
                    .build();

                // Scrollable Input Wrapper
                let input_scroll = gtk4::ScrolledWindow::builder()
                    .child(&input_view)
                    .propagate_natural_height(true)
                    .max_content_height(150)
                    .min_content_height(40)
                    .build();

                input_card.append(&input_scroll);

                // Send Button
                let send_btn = gtk4::Button::from_icon_name("mail-send-symbolic");
                send_btn.add_css_class("flat");
                input_card.append(&send_btn);

                comms_box.append(&input_card);
                center_stack.add_named(&comms_box, Some("Comms"));

                // Page 2: WOLFPACK GRID (Placeholder)
                let wolfpack_label = gtk4::Label::builder()
                    .label("WOLFPACK STATUS: OFFLINE")
                    .css_classes(vec!["title-1"])
                    .build();
                center_stack.add_named(&wolfpack_label, Some("Wolfpack"));

                main_box.append(&center_stack);

                // Separator
                main_box.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));

                // 3. RIGHT PANE (Action Deck)
                let right_box = gtk4::Box::builder()
                    .orientation(gtk4::Orientation::Vertical)
                    .width_request(200)
                    .spacing(6)
                    .margin_top(12)
                    .margin_end(12)
                    .build();

                let action_list = gtk4::ListBox::builder()
                    .selection_mode(gtk4::SelectionMode::None)
                    .css_classes(vec!["boxed-list"])
                    .build();

                right_box.append(&action_list);
                main_box.append(&right_box);

                // Set Content
                toolbar_view.set_content(Some(&main_box));
                window.set_content(Some(&toolbar_view));


                // --- INPUT LOGIC: INTELLIGENT ENTER KEY ---
                let key_controller = EventControllerKey::new();
                let h_input = handler_rc.clone();
                let iv_clone = input_view.clone();

                key_controller.connect_key_pressed(move |_controller, keyval, _keycode, modifiers| {
                    if keyval == gtk4::gdk::Key::Return || keyval == gtk4::gdk::Key::KP_Enter || keyval == gtk4::gdk::Key::ISO_Enter {
                        let buffer = iv_clone.buffer();
                        let (start, end) = buffer.bounds();
                        let text = buffer.text(&start, &end, false);

                        let is_multiline = text.contains('\n') || text.len() > 80;
                        let force_send = modifiers.contains(gtk4::gdk::ModifierType::CONTROL_MASK);

                        // If Control+Enter OR (Not Multiline AND Not Shift+Enter (implicit in simple check))
                        // Simplified Logic:
                        // If Ctrl+Enter -> Send
                        // If Enter (no Shift) -> Send (unless explicitly desired otherwise, but prompt says "Intelligent")
                        // Prompt says: "It only sends a signal when the message is complete."
                        // Let's implement standard chat app behavior: Enter sends, Shift+Enter adds newline.

                        let shift_pressed = modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK);

                        if !shift_pressed || force_send {
                             // SEND ACTION
                             if !text.trim().is_empty() {
                                 h_input.borrow_mut().handle_event(Event::Input(text.to_string()));
                                 buffer.set_text("");
                             }
                             return glib::Propagation::Stop;
                        }
                        // Else: Allow default newline insertion (Shift+Enter)
                    }
                    glib::Propagation::Proceed
                });
                input_view.add_controller(key_controller);

                // Send Button Logic
                let h_btn = handler_rc.clone();
                let iv_btn = input_view.clone();
                send_btn.connect_clicked(move |_| {
                    let buffer = iv_btn.buffer();
                    let (start, end) = buffer.bounds();
                    let text = buffer.text(&start, &end, false);
                    if !text.trim().is_empty() {
                        h_btn.borrow_mut().handle_event(Event::Input(text.to_string()));
                        buffer.set_text("");
                    }
                });

                // --- SIGNAL HANDLING (Nav & Actions) ---
                let h_nav = handler_rc.clone();
                nav_list.connect_row_activated(move |_list, row| {
                     let idx = row.index();
                     if idx >= 0 {
                         h_nav.borrow_mut().handle_event(Event::Nav(idx as usize));
                     }
                });

                // --- RENDER LOOP ---
                let h_tick = handler_rc.clone();
                let text_buffer = output_view.buffer();

                // Track previous state to optimize updates
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
                             let row = gtk4::ListBoxRow::new();
                             let label = gtk4::Label::new(Some(item_text));
                             label.set_margin_start(10);
                             label.set_margin_end(10);
                             label.set_margin_top(10);
                             label.set_margin_bottom(10);
                             label.set_xalign(0.0);
                             row.set_child(Some(&label));
                             nav_list.append(&row);
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
                            if center_stack.visible_child_name().as_deref() != Some("Comms") {
                                center_stack.set_visible_child_name("Comms");
                            }
                            // Update Text (Console Log)
                            let start = text_buffer.start_iter();
                            let end = text_buffer.end_iter();
                            let current_text = text_buffer.text(&start, &end, false);
                            if current_text != state.console_output {
                                text_buffer.set_text(&state.console_output);
                                // Auto-scroll
                                let end_iter = text_buffer.end_iter();
                                let mark = text_buffer.create_mark(None, &end_iter, false);
                                output_view.scroll_to_mark(&mark, 0.0, true, 0.0, 1.0);
                            }
                        },
                        ViewMode::Wolfpack => {
                            if center_stack.visible_child_name().as_deref() != Some("Wolfpack") {
                                center_stack.set_visible_child_name("Wolfpack");
                            }
                        }
                    }

                    // 3. Sync Right (Actions)
                    let mut actions_cache = current_actions.borrow_mut();
                    if *actions_cache != state.actions {
                        // Rebuild Actions
                        while let Some(child) = action_list.first_child() {
                            action_list.remove(&child);
                        }
                        for (i, action_text) in state.actions.iter().enumerate() {
                            let row = gtk4::ListBoxRow::new();
                            let btn = gtk4::Button::with_label(action_text);
                            btn.set_height_request(50);

                            // Wire Click
                            let h_action = Rc::clone(&h_tick); // Clone the Rc
                            btn.connect_clicked(move |_| {
                                h_action.borrow_mut().handle_event(Event::Action(i));
                            });

                            row.set_child(Some(&btn));
                            row.set_activatable(false);
                            row.set_selectable(false);
                            action_list.append(&row);
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
