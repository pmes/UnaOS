#![allow(deprecated)]

use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box, Orientation, Label, Button, Stack, ScrolledWindow,
    PolicyType, Align, ListBox, Separator, StackTransitionType, TextView, EventControllerKey,
    TextBuffer, Adjustment, FileChooserNative, ResponseType, FileChooserAction,
    HeaderBar, StackSwitcher, ToggleButton, CssProvider, StyleContext, Image
};
use gtk4::gdk::{Key, ModifierType};
use std::rc::Rc;
use std::cell::RefCell;
use std::time::{Duration, Instant};
use log::info;
use std::io::{Read, Write};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

// --- CORE DATA STRUCTURES ---

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SavedMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ShardStatus {
    Online,
    Offline,
    Thinking,
    Error,
    Syncing,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ShardRole {
    Root,
    Builder,
    Viewer,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Shard {
    pub id: String,
    pub name: String,
    pub role: ShardRole,
    pub status: ShardStatus,
    pub children: Vec<Shard>,
    pub expanded: bool,
}

impl Shard {
    pub fn new(id: &str, name: &str, role: ShardRole) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            role,
            status: ShardStatus::Offline,
            children: Vec::new(),
            expanded: true,
        }
    }
}

// --- BRAIN (PERSISTENCE LAYER) ---

#[derive(Clone)]
pub struct BrainManager {
    memory_path: String,
}

impl BrainManager {
    pub fn new() -> Self {
        BrainManager {
            memory_path: "memories".to_string(),
        }
    }

    pub fn save(&self, history: &Vec<SavedMessage>) {
        let json = serde_json::to_string_pretty(history).unwrap();
        let path = Path::new(&self.memory_path).join("chat_history.json");

        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .unwrap();
        file.write_all(json.as_bytes()).unwrap();
    }

    pub fn load(&self) -> Vec<SavedMessage> {
        let path = Path::new(&self.memory_path).join("chat_history.json");
        if path.exists() {
            let mut file = File::open(path).unwrap();
            let mut contents = String::new();
            file.read_to_string(&mut contents).unwrap();

            if contents.trim().is_empty() {
                return Vec::new();
            }

            match serde_json::from_str(&contents) {
                Ok(data) => data,
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        }
    }
}

// --- UI TYPES & TRAITS ---

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
    UploadRequest,
    FileSelected(PathBuf),
    ToggleSidebar,
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

// --- GTK BACKEND ---

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

    // --- MAIN WINDOW ---
    let window = ApplicationWindow::builder()
        .application(app)
        .default_width(1100)
        .default_height(750)
        .title("Vein")
        .build();

    // --- HEADER BAR ---
    let header_bar = HeaderBar::new();
    let sidebar_toggle = ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .active(true)
        .tooltip_text("Toggle Sidebar")
        .build();
    header_bar.pack_start(&sidebar_toggle);
    window.set_titlebar(Some(&header_bar));

    // --- BODY CONTAINER ---
    let body_box = Box::new(Orientation::Horizontal, 0);

    // --- SIDEBAR ---
    let sidebar_box = Box::new(Orientation::Vertical, 0);
    sidebar_box.set_width_request(200);
    sidebar_box.add_css_class("sidebar");

    let sidebar_stack = Stack::new();
    sidebar_stack.set_vexpand(true);
    sidebar_stack.set_transition_type(StackTransitionType::SlideLeftRight);

    // Page 1: Rooms
    let rooms_list = ListBox::new();
    let active_state = app_handler_rc.borrow().view();
    for (idx, item) in active_state.nav_items.iter().enumerate() {
        rooms_list.append(&make_sidebar_row(item, idx == active_state.active_nav_index));
    }
    let app_handler_rc_clone_for_nav = app_handler_rc.clone();
    rooms_list.connect_row_activated(move |_list_box, row| {
        app_handler_rc_clone_for_nav.borrow_mut().handle_event(Event::NavSelect(row.index() as usize));
    });
    sidebar_stack.add_titled(&rooms_list, Some("rooms"), "Rooms");

    // Page 2: Status
    let status_box = Box::new(Orientation::Vertical, 10);
    set_margins(&status_box, 10);
    status_box.append(&Label::builder().label(":: SYSTEM STATUS ::").css_classes(vec!["heading"]).build());
    status_box.append(&make_status_row("S9 (Upload)", "🟢 Online"));
    status_box.append(&make_status_row("Una (Link)", "🟢 Connected"));

    let relink_btn = Button::with_label("Re-Link Brain");
    relink_btn.add_css_class("destructive-action");
    status_box.append(&relink_btn);
    sidebar_stack.add_titled(&status_box, Some("status"), "Status");

    sidebar_box.append(&sidebar_stack);

    let tab_box = Box::new(Orientation::Horizontal, 0);
    tab_box.set_halign(Align::Center);
    tab_box.set_margin_top(5);
    tab_box.set_margin_bottom(5);
    let stack_switcher = StackSwitcher::builder().stack(&sidebar_stack).build();
    tab_box.append(&stack_switcher);
    sidebar_box.append(&tab_box);

    body_box.append(&sidebar_box);
    body_box.append(&Separator::new(Orientation::Vertical));

    // --- CONTENT ---
    let content_box = Box::new(Orientation::Vertical, 0);
    content_box.set_hexpand(true);

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
        .monospace(true)
        .buffer(&text_buffer)
        .margin_start(12).margin_end(12).margin_top(12).margin_bottom(12)
        .build();

    let text_buffer_clone = text_buffer.clone();
    let scrolled_window_adj_clone = scrolled_window.vadjustment();
    let app_handler_rc_clone_for_update = app_handler_rc.clone();

    // UI REFRESH LOOP
    glib::timeout_add_local(Duration::from_millis(100), move || {
        let buf = text_buffer_clone.clone();
        let adj = scrolled_window_adj_clone.clone();
        app_handler_rc_clone_for_update.borrow_mut().handle_event(Event::TextBufferUpdate(buf, adj));
        glib::ControlFlow::Continue
    });

    scrolled_window.set_child(Some(&console_text_view));
    content_box.append(&scrolled_window);

    // Input Area
    let input_container = Box::new(Orientation::Horizontal, 8);
    set_margins(&input_container, 10);
    input_container.set_valign(Align::End);

    // Upload
    let upload_icon = Image::from_resource("/org/una/vein/icons/share-symbolic");
    let upload_btn = Button::builder().child(&upload_icon).valign(Align::End).build();
    let app_handler_rc_for_upload = app_handler_rc.clone();
    let window_weak = window.downgrade();
    upload_btn.connect_clicked(move |_| {
         let dialog = FileChooserNative::builder().title("Select File").action(FileChooserAction::Open).modal(true).accept_label("Upload").cancel_label("Cancel").build();
        if let Some(w) = window_weak.upgrade() { dialog.set_transient_for(Some(&w)); }
        let h = app_handler_rc_for_upload.clone();
        dialog.connect_response(move |d, r| {
            if r == ResponseType::Accept {
                if let Some(f) = d.file() { if let Some(p) = f.path() { h.borrow_mut().handle_event(Event::FileSelected(p)); } }
            }
            d.destroy();
        });
        dialog.show();
    });
    input_container.append(&upload_btn);

    // Pill Input
    let input_scroll = ScrolledWindow::builder().hscrollbar_policy(PolicyType::Never).vscrollbar_policy(PolicyType::Automatic).propagate_natural_height(true).max_content_height(150).vexpand(false).valign(Align::Fill).has_frame(false).build();
    input_scroll.set_hexpand(true);
    input_scroll.add_css_class("chat-input-area");

    let text_view = TextView::builder().wrap_mode(gtk4::WrapMode::WordChar).accepts_tab(false).top_margin(6).bottom_margin(6).left_margin(8).right_margin(8).build();
    text_view.add_css_class("transparent-text");
    input_scroll.set_child(Some(&text_view));

    // Send
    let send_icon = Image::from_resource("/org/una/vein/icons/paper-plane-symbolic");
    let send_btn = Button::builder().child(&send_icon).valign(Align::End).css_classes(vec!["suggested-action"]).build();

    // Visual Listener
    let buffer = text_view.buffer();
    let btn_style = send_btn.clone();
    buffer.connect_changed(move |buf| {
        if buf.line_count() > 1 { btn_style.remove_css_class("suggested-action"); } else { btn_style.add_css_class("suggested-action"); }
    });

    let app_handler_rc_for_send = app_handler_rc.clone();
    let text_view_for_send = text_view.clone();
    let scroll_adj = scrolled_window.vadjustment();

    let send_action = Rc::new(move || {
        let buf = text_view_for_send.buffer();
        let (start, end) = buf.bounds();
        let text = buf.text(&start, &end, false).to_string();
        if text.trim().is_empty() { return; }
        app_handler_rc_for_send.borrow_mut().handle_event(Event::Input(text.trim().to_string()));
        buf.set_text("");
        let adj = scroll_adj.clone();
        glib::timeout_add_local(Duration::from_millis(50), move || { adj.set_value(adj.upper()); glib::ControlFlow::Break });
    });

    let controller = EventControllerKey::new();
    let send_key = send_action.clone();
    controller.connect_key_pressed(move |ctrl, key, _, mods| {
        if key == Key::Return || key == Key::KP_Enter {
             if !mods.contains(ModifierType::SHIFT_MASK) {
                 send_key();
                 return glib::Propagation::Stop;
             }
        }
        glib::Propagation::Proceed
    });
    text_view.add_controller(controller);
    send_btn.connect_clicked(move |_| send_action());

    input_container.append(&input_scroll);
    input_container.append(&send_btn);
    content_box.append(&input_container);
    body_box.append(&content_box);
    window.set_child(Some(&body_box));

    // CSS
    let provider = CssProvider::new();
    provider.load_from_data("
        window { border-radius: 8px; }
        .sidebar { background: #1e1e1e; }
        .chat-input-area { background: #2d2d2d; border: 1px solid #555555; border-radius: 20px; transition: border-color 0.2s; }
        .chat-input-area:focus-within { border-color: #3584e4; background: #333333; }
        .transparent-text { background: transparent; caret-color: white; color: white; }
        textview { font-family: 'Monospace'; font-size: 11pt; padding: 0px; }
    ");
    StyleContext::add_provider_for_display(&gtk4::gdk::Display::default().expect("No display"), &provider, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);

    // Toggle Logic
    let sb_clone = sidebar_box.clone();
    let ah_clone = app_handler_rc.clone();
    sidebar_toggle.connect_toggled(move |btn| {
        sb_clone.set_visible(btn.is_active());
        ah_clone.borrow_mut().handle_event(Event::ToggleSidebar);
    });

    if app_handler_rc.borrow().view().sidebar_collapsed {
        sidebar_toggle.set_active(false);
        sidebar_box.set_visible(false);
    }

    window.present();
    info!("UI_BUILD: Window presented. Duration: {:?}", ui_build_start_time.elapsed());
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
