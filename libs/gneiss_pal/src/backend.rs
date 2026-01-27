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

    pub struct Backend;

    impl Backend {
        pub fn new<H: AppHandler + 'static>(app_id: &str, handler: H) -> Self {
            let handler = Rc::new(RefCell::new(handler));

            let app = adw::Application::builder()
                .application_id(app_id)
                .build();

            app.connect_activate(move |app| {
                let h = handler.clone();

                // 1. THE WINDOW (Adwaita)
                let window = adw::ApplicationWindow::builder()
                    .application(app)
                    .title("UnaOS :: Vein") // Updated Title
                    .default_width(1200)
                    .default_height(800)
                    .build();

                // 2. THE CHASSIS (Tri-Pane)
                let main_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);

                // LEFT PANE (Navigation)
                let left_pane = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                left_pane.set_width_request(250);
                left_pane.add_css_class("navigation-sidebar"); // Native Adwaita styling

                // MARGINS (Calibration)
                left_pane.set_margin_top(12);
                left_pane.set_margin_bottom(12);
                left_pane.set_margin_start(12);
                left_pane.set_margin_end(6); // Added spacing

                let nav_list = gtk4::ListBox::new();
                nav_list.set_vexpand(true);
                nav_list.set_selection_mode(gtk4::SelectionMode::Single);

                let nav_scroll = gtk4::ScrolledWindow::builder()
                    .child(&nav_list)
                    .vexpand(true)
                    .hscrollbar_policy(gtk4::PolicyType::Never)
                    .build();

                left_pane.append(&nav_scroll);

                // CENTER PANE (The Stage)
                let center_stack = gtk4::Stack::new();
                center_stack.set_hexpand(true);

                // Page 1: COMMS (Console + Input)
                let comms_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

                // Output Log (Top)
                let output_view = gtk4::TextView::builder()
                    .editable(false)
                    .monospace(true)
                    .cursor_visible(false)
                    // .selectable(true) // Not available in gtk4::TextView builder, set later if needed or default
                    .wrap_mode(gtk4::WrapMode::WordChar)
                    .left_margin(12)
                    .right_margin(12)
                    .top_margin(12)
                    .bottom_margin(12)
                    .build();

                let output_scroll = gtk4::ScrolledWindow::builder()
                    .child(&output_view)
                    .vexpand(true)
                    .build();
                comms_box.append(&output_scroll);

                // Input Area (Bottom) - The "Morphing Box"
                let input_area = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
                input_area.add_css_class("toolbar");
                input_area.set_margin_start(10);
                input_area.set_margin_end(10);
                input_area.set_margin_top(10);
                input_area.set_margin_bottom(10);

                let input_view = gtk4::TextView::builder()
                    .wrap_mode(gtk4::WrapMode::WordChar)
                    .accepts_tab(false)
                    .build();

                // INPUT VOID CALIBRATION
                input_view.add_css_class("card");
                input_view.set_margin_start(10);
                input_view.set_margin_end(10);
                input_view.set_margin_top(10);
                input_view.set_margin_bottom(10);

                let input_scroll = gtk4::ScrolledWindow::builder()
                    .child(&input_view)
                    .min_content_height(35)   // Start small (1 line)
                    .max_content_height(150)  // Grow to ~5 lines
                    .propagate_natural_height(true) // Enable Morphing Physics
                    .vscrollbar_policy(gtk4::PolicyType::Automatic)
                    .hscrollbar_policy(gtk4::PolicyType::Never)
                    .hexpand(true)
                    .build();

                // Send Button
                let send_btn = gtk4::Button::from_icon_name("mail-send-symbolic");
                send_btn.add_css_class("suggested-action");
                send_btn.set_valign(gtk4::Align::End); // Align to bottom of input box
                send_btn.set_margin_bottom(4); // Visual tweak

                input_area.append(&input_scroll);
                input_area.append(&send_btn);
                comms_box.append(&input_area);

                center_stack.add_named(&comms_box, Some("comms"));

                // Wolfpack Page
                let wolf_label = gtk4::Label::new(Some("WOLFPACK STATUS: ACTIVE"));
                wolf_label.add_css_class("title-1");
                center_stack.add_named(&wolf_label, Some("wolfpack"));

                // RIGHT PANE (Actions)
                let right_pane = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
                right_pane.set_width_request(200);

                // MARGINS (Calibration)
                right_pane.set_margin_top(12);
                right_pane.set_margin_bottom(12);
                right_pane.set_margin_start(6); // Added spacing
                right_pane.set_margin_end(12);

                let right_scroll = gtk4::ScrolledWindow::builder()
                    .child(&right_pane)
                    .vexpand(true)
                    .build();

                // ASSEMBLE
                main_box.append(&left_pane);
                main_box.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));
                main_box.append(&center_stack);
                main_box.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));
                main_box.append(&right_scroll);

                // 3. HEADER BAR
                let header = adw::HeaderBar::new();
                let toolbar_view = adw::ToolbarView::new();
                toolbar_view.add_top_bar(&header);
                toolbar_view.set_content(Some(&main_box));
                window.set_content(Some(&toolbar_view));

                // 4. INTELLIGENT INPUT LOGIC
                let key_controller = EventControllerKey::new();
                let h_input = h.clone();
                let iv_clone = input_view.clone();

                key_controller.connect_key_pressed(move |_controller, keyval, _keycode, modifiers| {
                    if keyval == gtk4::gdk::Key::Return || keyval == gtk4::gdk::Key::KP_Enter || keyval == gtk4::gdk::Key::ISO_Enter {
                        // Shift+Enter -> Newline
                        if modifiers.contains(gtk4::gdk::ModifierType::SHIFT_MASK) {
                            return glib::Propagation::Proceed;
                        }

                        let buffer = iv_clone.buffer();
                        // Multi-line? -> Newline
                        if buffer.line_count() > 1 {
                            return glib::Propagation::Proceed;
                        }

                        // Single-line -> SEND
                        let (start, end) = buffer.bounds();
                        let text = buffer.text(&start, &end, false).to_string();
                        if !text.trim().is_empty() {
                             h_input.borrow_mut().handle_event(Event::Input(text));
                             buffer.set_text(""); // Clear
                        }
                        return glib::Propagation::Stop;
                    }
                    glib::Propagation::Proceed
                });
                input_view.add_controller(key_controller);

                // Send Button Click
                let h_btn = h.clone();
                let iv_btn = input_view.clone();
                send_btn.connect_clicked(move |_| {
                    let buffer = iv_btn.buffer();
                    let (start, end) = buffer.bounds();
                    let text = buffer.text(&start, &end, false).to_string();
                    if !text.trim().is_empty() {
                        h_btn.borrow_mut().handle_event(Event::Input(text));
                        buffer.set_text("");
                    }
                });

                // Nav Selection Logic
                let h_nav = h.clone();
                nav_list.connect_row_activated(move |_list, row| {
                    let idx = row.index();
                     if idx >= 0 {
                         h_nav.borrow_mut().handle_event(Event::NavSelect(idx as usize));
                     }
                });

                // 5. RENDER LOOP (The Heartbeat)
                let h_tick = h.clone();
                let buffer = output_view.buffer();

                // Optimization: Track state to avoid rebuilding lists every frame
                let current_actions = Rc::new(RefCell::new(Vec::<String>::new()));
                let current_navs = Rc::new(RefCell::new(Vec::<String>::new()));

                glib::timeout_add_local(std::time::Duration::from_millis(32), move || {
                    let mut h_lock = h_tick.borrow_mut();

                    // Timer Event
                    h_lock.handle_event(Event::Timer);

                    let state = h_lock.view();

                    // Sync Output
                    // Diff check to avoid spamming text buffer
                    let start = buffer.start_iter();
                    let end = buffer.end_iter();
                    let current_text = buffer.text(&start, &end, false);
                    if current_text != state.console_output {
                        buffer.set_text(&state.console_output);
                        // Auto-scroll to bottom
                        // We need to wait for layout to update for accurate scroll?
                        // Usually insert mark at end works.
                        let mark = buffer.create_mark(None, &buffer.end_iter(), false);
                        output_view.scroll_to_mark(&mark, 0.0, false, 0.0, 1.0);
                    }

                    // Sync Mode (Tabs)
                    let page_name = match state.mode {
                        ViewMode::Comms => "comms",
                        ViewMode::Wolfpack => "wolfpack",
                    };
                    if center_stack.visible_child_name().as_deref() != Some(page_name) {
                        center_stack.set_visible_child_name(page_name);
                    }

                    // Sync Left Pane (Nav)
                    let mut nav_cache = current_navs.borrow_mut();
                    if *nav_cache != state.nav_items {
                         while let Some(child) = nav_list.first_child() {
                             nav_list.remove(&child);
                         }
                         for item_text in &state.nav_items {
                             let row = gtk4::ListBoxRow::new();
                             let label = gtk4::Label::new(Some(item_text));

                             // CORRECTED SYNTAX: Explicitly set margins
                             label.set_margin_top(12);
                             label.set_margin_bottom(12);
                             label.set_margin_start(12);
                             label.set_margin_end(12);

                             label.set_xalign(0.0);
                             row.set_child(Some(&label));
                             nav_list.append(&row);
                         }
                         *nav_cache = state.nav_items.clone();
                    }

                    // Sync Nav Selection
                    if let Some(row) = nav_list.row_at_index(state.active_nav_index as i32) {
                        if !row.is_selected() {
                            nav_list.select_row(Some(&row));
                        }
                    }

                    // Sync Right Pane (Actions)
                    let mut actions_cache = current_actions.borrow_mut();
                    if *actions_cache != state.actions {
                        while let Some(child) = right_pane.first_child() {
                            right_pane.remove(&child);
                        }
                        for (i, label_text) in state.actions.iter().enumerate() {
                            let btn = gtk4::Button::with_label(label_text);
                            btn.set_height_request(50);
                            let h_action = h_tick.clone(); // Clone Rc for signal
                            btn.connect_clicked(move |_| {
                                h_action.borrow_mut().handle_event(Event::TemplateAction(i));
                            });
                            right_pane.append(&btn);
                        }
                        *actions_cache = state.actions.clone();
                    }

                    glib::ControlFlow::Continue
                });

                window.present();
            });

            app.run_with_args::<&str>(&[]);
            Self
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux_impl::Backend;

#[cfg(target_os = "macos")]
mod mac_impl {
    use super::*;
    pub struct Backend;
    impl Backend {
        pub fn new<H: AppHandler + 'static>(_app_id: &str, _handler: H) -> Self {
            panic!("MacOS backend is currently mothballed for 'The Great Evolution'. Use Linux.");
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac_impl::Backend;

#[cfg(target_os = "windows")]
mod win_impl {
    use super::*;
    pub struct Backend;
    impl Backend {
        pub fn new<H: AppHandler + 'static>(_app_id: &str, _handler: H) -> Self {
            panic!("Windows backend is currently mothballed for 'The Great Evolution'. Use Linux.");
        }
    }
}

#[cfg(target_os = "windows")]
pub use win_impl::Backend;
