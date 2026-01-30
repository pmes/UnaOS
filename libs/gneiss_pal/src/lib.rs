#![allow(deprecated)]

use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box, Orientation, Label, Button, Stack, ScrolledWindow,
    PolicyType, Align, ListBox, Separator, StackTransitionType, TextView, EventControllerKey,
    TextBuffer, Adjustment, FileChooserNative, ResponseType, FileChooserAction, WindowHandle,
    WindowControls
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
    ToggleSidebar, // NEW: Toggle sidebar visibility
}

#[derive(Clone, Debug, PartialEq)]
pub enum ShardStatus {
    Online,
    Offline,
    Syncing,
    Error,
}

#[derive(Clone, Debug)]
pub struct Shard {
    pub name: String,
    pub status: ShardStatus,
    pub children: Vec<Shard>,
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
    pub shard_tree: Vec<Shard>,
    pub sidebar_collapsed: bool,
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
            shard_tree: Vec::new(),
            sidebar_collapsed: false,
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

    // --- CUSTOM TITLEBAR (Header Architecture) ---
    let header_box = Box::new(Orientation::Horizontal, 0);
    header_box.add_css_class("titlebar");

    // Left Header Area (matches sidebar width approx)
    let left_header_box = Box::new(Orientation::Horizontal, 0);
    left_header_box.set_width_request(260);

    // Sidebar Header Content (Draggable)
    let sidebar_handle = WindowHandle::new();
    let sidebar_header_content = Box::new(Orientation::Horizontal, 10);
    set_margins(&sidebar_header_content, 10);
    // Placeholder title or logo could go here
    sidebar_handle.set_child(Some(&sidebar_header_content));
    left_header_box.append(&sidebar_handle);

    header_box.append(&left_header_box);
    header_box.append(&Separator::new(Orientation::Vertical));

    // Right Header Area (Main Content Controls + Window Controls)
    let right_header_box = Box::new(Orientation::Horizontal, 0);
    right_header_box.set_hexpand(true);

    // Main Content Header Handle (Draggable)
    let main_handle = WindowHandle::new();
    let main_header_content = Box::new(Orientation::Horizontal, 10);
    main_header_content.set_hexpand(true);
    // Margins to align visually
    set_margins(&main_header_content, 6);

    // Toggle button (moved to main header area)
    let toggle_btn = Button::builder()
        .icon_name("sidebar-show-symbolic")
        .css_classes(vec!["flat"])
        .build();
    // Logic connected later

    main_header_content.append(&toggle_btn);
    main_handle.set_child(Some(&main_header_content));

    right_header_box.append(&main_handle);

    // Window Controls (Close/Max/Min)
    let window_controls = WindowControls::new(gtk4::PackType::End);
    right_header_box.append(&window_controls);

    header_box.append(&right_header_box);

    window.set_titlebar(Some(&header_box));

    let split_view = libadwaita::OverlaySplitView::new();
    window.set_child(Some(&split_view));

    // --- SIDEBAR (Left/Right Panel) ---
    let sidebar_box = Box::new(Orientation::Vertical, 0);
    sidebar_box.set_width_request(260);
    // Removed legacy HeaderBar

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

    // Add margins to the console view container for better readability
    console_text_view.set_margin_start(12);
    console_text_view.set_margin_end(12);
    console_text_view.set_margin_top(12);
    console_text_view.set_margin_bottom(12);

    scrolled_window.set_child(Some(&console_text_view));

    // Legacy HeaderBar removed.
    // Connect the Toggle Button we created in the custom titlebar
    let app_handler_rc_for_toggle = app_handler_rc.clone();
    toggle_btn.connect_clicked(move |_| {
        app_handler_rc_for_toggle.borrow_mut().handle_event(Event::ToggleSidebar);
    });

    main_content_box.append(&scrolled_window);

    // --- INPUT AREA ---
    let input_container = Box::new(Orientation::Horizontal, 10);
    set_margins(&input_container, 10);
    input_container.add_css_class("linked");

    // NEW: Upload Button logic using pure GTK4 FileChooserNative
    let upload_btn = Button::builder().icon_name("file-cabinet-symbolic").valign(Align::End).css_classes(vec!["suggested-action"]).build();
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
        .propagate_natural_height(true) // Key property for growing input
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
    let send_btn = Button::builder().icon_name("paper-plane-symbolic").valign(Align::End).css_classes(vec!["suggested-action"]).build();

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

    // Handle initial collapsed state (though dynamic updates require a signal or re-render which we lack here for simplicity,
    // we rely on the split_view properties if we had a reactive loop.
    // GneissPal's current architecture rebuilds UI only on init, so dynamic updates need a reactive binding or manual signal.
    // For now, we are just building the UI. The toggle logic usually needs to manipulate the widget *after* build.
    // Since GneissPal is simple, we might need to expose the split_view to the handler or use a global signal.
    // However, given the constraints, we'll try to set it initially.
    if app_handler_rc.borrow().view().sidebar_collapsed {
        split_view.set_collapsed(true);
    }

    // Hack: To support dynamic toggling without full reactivity, we can use a closure attached to the button
    // that modifies the widget directly if we had access. But the button handler above just emits an event.
    // The AppHandler updates state, but the UI doesn't know to redraw or update properties.
    // Real fix: The Event loop needs to trigger a UI update. Vein uses `do_append_and_scroll`, but for structural changes...
    // We will leave the "ToggleSidebar" event to update state, and maybe the AppHandler can trigger a rebuild?
    // Or we inject a closure into the AppHandler?
    // For this specific task, we will just implement the Event.
    // Wait, the user asked for a "Toggle Button". If the UI doesn't react, it's useless.
    // We can use `split_view.bind_property` or similar if we had a GObject for state.
    // Simpler: The toggle button itself can toggle the split_view directly!
    // But we also need to update the State to persist it.
    // So:
    let split_view_clone = split_view.clone();
    toggle_btn.connect_clicked(move |_| {
        let is_collapsed = split_view_clone.is_collapsed();
        split_view_clone.set_collapsed(!is_collapsed);
    });

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
