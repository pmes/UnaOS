#![allow(deprecated)]

use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box, Orientation, Label, Button, Stack, ScrolledWindow,
    PolicyType, Align, ListBox, Separator, StackTransitionType, TextView, EventControllerKey,
    TextBuffer, Adjustment, FileChooserNative, ResponseType, FileChooserAction
};
use gtk4::gdk::{Key, ModifierType};
use std::rc::Rc;
use std::cell::RefCell;
use std::time::Duration;
use log::{info};
use std::time::Instant;
use std::io::Write;
use std::path::PathBuf; // Import PathBuf

pub mod persistence;

#[derive(Clone, Debug, PartialEq)]
pub enum ViewMode {
    Comms,
    Wolfpack,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SidebarPosition {
    Left,
    Right,
}

impl Default for SidebarPosition {
    fn default() -> Self {
        SidebarPosition::Right
    }
}

#[derive(Debug)]
pub enum Event {
    Input(String),
    TemplateAction(usize),
    NavSelect(usize),
    DockAction(usize),
    TextBufferUpdate(TextBuffer, Adjustment),
    UploadRequest, // Kept for compatibility, though effectively unused now
    FileSelected(PathBuf), // NEW: Carries the selected path back to Vein
}

#[derive(Clone, Debug)]
pub struct DashboardState {
    pub mode: ViewMode,
    pub nav_items: Vec<String>,
    pub active_nav_index: usize,
    pub console_output: String,
    pub actions: Vec<String>,
    pub sidebar_position: SidebarPosition,
    pub dock_actions: Vec<String>,
}

impl Default for DashboardState {
    fn default() -> Self {
        DashboardState {
            mode: ViewMode::Comms,
            nav_items: Vec::new(),
            active_nav_index: 0,
            console_output: String::new(),
            actions: Vec::new(),
            sidebar_position: SidebarPosition::default(),
            dock_actions: Vec::new(),
        }
    }
}

pub trait AppHandler: 'static {
    fn handle_event(&mut self, event: Event);
    fn view(&self) -> DashboardState;
}

#[allow(dead_code)]
pub struct Backend<A: AppHandler> {
    app_handler: Rc<RefCell<A>>,
    app_id: String,
}

impl<A: AppHandler> Backend<A> {
    pub fn new(app_id: &str, app_handler: A) -> Self {
        let app = Application::builder()
            .application_id(app_id)
            .build();

        let app_handler_rc = Rc::new(RefCell::new(app_handler));
        let app_handler_rc_clone = app_handler_rc.clone();

        app.connect_activate(move |app| {
            build_ui(app, app_handler_rc_clone.clone());
        });
        app.run();

        Self {
            app_handler: app_handler_rc,
            app_id: app_id.to_string(),
        }
    }
}

fn build_ui(app: &Application, app_handler_rc: Rc<RefCell<impl AppHandler>>) {
    let ui_build_start_time = Instant::now();
    info!("UI_BUILD: Starting build_ui function.");
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    // --- MAIN WINDOW ---
    let window = ApplicationWindow::builder()
        .application(app)
        .default_width(1000)
        .default_height(700)
        .title("Vein (Powered by unaOS Gneiss)")
        .build();

    let split_view = libadwaita::OverlaySplitView::new();
    window.set_child(Some(&split_view));

    // --- SIDEBAR (Left/Right Panel) ---
    let sidebar_box = Box::new(Orientation::Vertical, 0);
    sidebar_box.set_width_request(260);

    let sidebar_header = libadwaita::HeaderBar::new();
    sidebar_header.set_show_end_title_buttons(false);
    sidebar_header.set_show_start_title_buttons(false);
    sidebar_box.append(&sidebar_header);

    let sidebar_stack = Stack::new();
    sidebar_stack.set_vexpand(true);
    sidebar_stack.set_transition_type(StackTransitionType::SlideLeftRight);
    sidebar_box.append(&sidebar_stack);

    let rooms_list = ListBox::new();
    rooms_list.set_selection_mode(gtk4::SelectionMode::None);
    let active_state = app_handler_rc.borrow().view();
    for (idx, item) in active_state.nav_items.iter().enumerate() {
        rooms_list.append(&make_sidebar_row(item, idx == active_state.active_nav_index));
    }
    let app_handler_rc_clone_for_nav = app_handler_rc.clone();
    rooms_list.connect_row_activated(move |_list_box, row| {
        let idx = row.index() as usize;
        app_handler_rc_clone_for_nav.borrow_mut().handle_event(Event::NavSelect(idx));
    });
    sidebar_stack.add_named(&rooms_list, Some("rooms"));

    let status_box = Box::new(Orientation::Vertical, 10);
    set_margins(&status_box, 10);
    status_box.append(&Label::builder().label(":: SYSTEM STATUS ::").css_classes(vec!["heading"]).build());
    status_box.append(&make_status_row("S9 (Upload)", "🟢 Online"));
    status_box.append(&make_status_row("Una (Link)", "🟢 Connected"));
    sidebar_stack.add_named(&status_box, Some("status"));

    // --- BOTTOM DOCK ---
    let bottom_dock = Box::new(Orientation::Horizontal, 5);
    set_margins(&bottom_dock, 10);
    bottom_dock.set_halign(Align::Center);
    sidebar_box.append(&Separator::new(Orientation::Horizontal));
    sidebar_box.append(&bottom_dock);

    let sidebar_stack_clone = sidebar_stack.clone();
    let app_handler_rc_clone_for_dock = app_handler_rc.clone();

    for (idx, action_text) in active_state.dock_actions.iter().enumerate() {
        let button = Button::builder().label(action_text).build();
        let handler_clone = app_handler_rc_clone_for_dock.clone();
        let sidebar_stack_clone_inner = sidebar_stack_clone.clone();
        let action_text_clone = action_text.clone();

        button.connect_clicked(move |_| {
            handler_clone.borrow_mut().handle_event(Event::DockAction(idx));
            if action_text_clone == "Rooms" {
                sidebar_stack_clone_inner.set_visible_child_name("rooms");
            } else if action_text_clone == "Status" {
                sidebar_stack_clone_inner.set_visible_child_name("status");
            }
        });
        bottom_dock.append(&button);
    }

    // --- MAIN CONTENT AREA ---
    let main_content_box = Box::new(Orientation::Vertical, 0);

    let scrolled_window = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .vexpand(true)
        .build();

    let text_buffer = TextBuffer::new(None);
    text_buffer.set_text(&app_handler_rc.borrow().view().console_output);

    let console_text_view = TextView::builder()
        .wrap_mode(gtk4::WrapMode::WordChar)
        .editable(false)
        .buffer(&text_buffer)
        .vexpand(true)
        .build();

    let text_buffer_clone = text_buffer.clone();
    let scrolled_window_adj_clone = scrolled_window.vadjustment();

    app_handler_rc.borrow_mut().handle_event(Event::TextBufferUpdate(text_buffer_clone, scrolled_window_adj_clone));

    scrolled_window.set_child(Some(&console_text_view));
    main_content_box.append(&scrolled_window);

    // --- INPUT AREA ---
    let input_container = Box::new(Orientation::Horizontal, 10);
    set_margins(&input_container, 10);
    input_container.add_css_class("linked");

    // NEW: Upload Button logic using pure GTK4 FileChooserNative
    let upload_btn = Button::builder().icon_name("paperclip-symbolic").valign(Align::End).css_classes(vec!["suggested-action"]).build();
    let app_handler_rc_for_upload = app_handler_rc.clone();
    let window_weak = window.downgrade(); // Use weak ref to avoid cycles

    upload_btn.connect_clicked(move |_| {
        let dialog = FileChooserNative::builder()
            .title("Select File to Upload")
            .action(FileChooserAction::Open)
            .modal(true)
            .accept_label("Upload")
            .cancel_label("Cancel")
            .build();

        if let Some(window) = window_weak.upgrade() {
            dialog.set_transient_for(Some(&window));
        }

        let handler_clone = app_handler_rc_for_upload.clone();

        dialog.connect_response(move |d, response| {
            if response == ResponseType::Accept {
                if let Some(file) = d.file() {
                    if let Some(path) = file.path() {
                        handler_clone.borrow_mut().handle_event(Event::FileSelected(path));
                    }
                }
            }
            d.destroy();
        });

        dialog.show();
    });
    input_container.append(&upload_btn);

    let input_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .has_frame(true)
        .hexpand(true)
        .height_request(45)
        .max_content_height(150)
        .build();

    let text_view = TextView::builder()
        .wrap_mode(gtk4::WrapMode::WordChar)
        .accepts_tab(false)
        .top_margin(8).bottom_margin(8).left_margin(8).right_margin(8)
        .build();

    input_scroll.set_child(Some(&text_view));
    let send_btn = Button::builder().icon_name("mail-send-symbolic").valign(Align::End).css_classes(vec!["suggested-action"]).build();

    let app_handler_rc_for_send = app_handler_rc.clone();
    let text_view_for_send = text_view.clone();
    let scrolled_window_adj = scrolled_window.vadjustment();

    let send_message_rc: Rc<dyn Fn() + 'static> = Rc::new(move || {
        let buffer = text_view_for_send.buffer();
        let (start, end) = buffer.bounds();
        let text = buffer.text(&start, &end, false).to_string();
        let clean_text = text.trim();

        if clean_text.is_empty() { return; }

        app_handler_rc_for_send.borrow_mut().handle_event(Event::Input(clean_text.to_string()));

        buffer.set_text("");

        let adj_clone = scrolled_window_adj.clone();
        glib::timeout_add_local(Duration::from_millis(50), move || {
            adj_clone.set_value(adj_clone.upper());
            glib::ControlFlow::Break
        });
    });

    let controller = EventControllerKey::new();
    let send_action_clone_for_controller = send_message_rc.clone();

    controller.connect_key_pressed(move |ctrl, key, _, modifiers| {
        if key == Key::Return || key == Key::KP_Enter {
            let tv = ctrl.widget()
                .expect("Controller must be attached to a TextView")
                .downcast::<TextView>()
                .expect("Widget must be a TextView");
            let buffer = tv.buffer();

            if buffer.line_count() == 1 || modifiers.contains(ModifierType::SHIFT_MASK) {
                if !modifiers.contains(ModifierType::SHIFT_MASK) {
                    send_action_clone_for_controller();
                    return glib::Propagation::Stop;
                }
            }
            return glib::Propagation::Proceed;
        }
        glib::Propagation::Proceed
    });

    text_view.add_controller(controller);
    let send_action_clone_for_button = send_message_rc.clone();
    send_btn.connect_clicked(move |_| send_action_clone_for_button());

    input_container.append(&input_scroll);
    input_container.append(&send_btn);
    main_content_box.append(&input_container);

    split_view.set_sidebar(Some(&sidebar_box));
    split_view.set_content(Some(&main_content_box));

    window.present();
    info!("UI_BUILD: Window presented. Total build_ui duration: {:?}", ui_build_start_time.elapsed());
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

fn set_margins(w: &Box, s: i32) { w.set_margin_top(s); w.set_margin_bottom(s); w.set_margin_start(s); w.set_margin_end(s); }
fn make_sidebar_row(n: &str, a: bool) -> Box {
    let r = Box::new(Orientation::Horizontal, 10); set_margins(&r, 10);
    r.append(&Label::new(Some(n))); if a { r.append(&Label::new(Some("●"))); } r
}
fn make_status_row(s: &str, st: &str) -> Box {
    let r = Box::new(Orientation::Horizontal, 10); set_margins(&r, 5);
    r.append(&Label::builder().label(s).hexpand(true).xalign(0.0).build()); r.append(&Label::new(Some(st))); r
}
