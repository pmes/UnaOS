#![allow(deprecated)]
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box, Orientation, Label, Button, Stack, ScrolledWindow,
    PolicyType, Align, ListBox, Separator, StackTransitionType, TextView, EventControllerKey,
    TextBuffer, Adjustment, FileDialog, ResponseType, FileChooserAction,
    HeaderBar, StackSwitcher, ToggleButton, CssProvider, StyleContext, Image, MenuButton, Popover,
    Paned, Window
};
use gtk4::gdk::{Key, ModifierType};
use std::rc::Rc;
use std::cell::RefCell;
use std::time::Duration;
use log::info;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};

// --- DATA TYPES ---

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SavedMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ShardStatus {
    Online, Offline, Thinking, Error, Paused, OnCall, Active
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ShardRole {
    Root, Builder, Storage, Kernel, Viewer
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Shard {
    pub id: String,
    pub name: String,
    pub role: ShardRole,
    pub status: ShardStatus,
    pub children: Vec<Shard>,
}

impl Shard {
    pub fn new(id: &str, name: &str, role: ShardRole) -> Self {
        Self {
            id: id.to_string(), name: name.to_string(), role,
            status: ShardStatus::Offline, children: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum GuiUpdate {
    ShardStatusChanged { id: String, status: ShardStatus },
    ConsoleLog(String),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum ViewMode { #[default] Comms, Wolfpack }

// --- PERSISTENCE ---

#[derive(Clone)]
pub struct BrainManager { path: String }
impl BrainManager {
    pub fn new() -> Self { Self { path: "memories".to_string() } }
    pub fn save(&self, h: &Vec<SavedMessage>) {
        let _ = fs::create_dir_all(&self.path);
        let f = OpenOptions::new().write(true).create(true).truncate(true).open(Path::new(&self.path).join("history.json"));
        if let Ok(mut file) = f { let _ = file.write_all(serde_json::to_string_pretty(h).unwrap().as_bytes()); }
    }
    pub fn load(&self) -> Vec<SavedMessage> {
        if let Ok(mut f) = File::open(Path::new(&self.path).join("history.json")) {
            let mut s = String::new();
            if f.read_to_string(&mut s).is_ok() { return serde_json::from_str(&s).unwrap_or_default(); }
        }
        Vec::new()
    }
}

// --- EVENTS ---

#[derive(Clone, Debug)]
pub enum Event {
    Input(String),
    NavSelect(usize),
    TextBufferUpdate(TextBuffer, Adjustment),
    FileSelected(PathBuf),
    ToggleSidebar,
}

#[derive(Clone, Debug, Default)]
pub struct DashboardState {
    pub mode: ViewMode,
    pub nav_items: Vec<String>,
    pub active_nav_index: usize,
    pub console_output: String,
    pub shard_tree: Vec<Shard>,
    pub sidebar_collapsed: bool,
    pub sidebar_position: SidebarPosition,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum SidebarPosition { #[default] Right, Left }

pub trait AppHandler: 'static {
    fn handle_event(&mut self, event: Event);
    fn view(&self) -> DashboardState;
}

// --- BACKEND ---

pub struct Backend<A: AppHandler> {
    _marker: std::marker::PhantomData<A>,
}

impl<A: AppHandler> Backend<A> {
    pub fn new(app_id: &str, app_handler: A, rx: async_channel::Receiver<GuiUpdate>) {
        let app = Application::builder().application_id(app_id).build();
        let h_rc = Rc::new(RefCell::new(app_handler));
        app.connect_activate(move |app| build_ui(app, h_rc.clone(), rx.clone()));
        app.run();
    }
}

fn build_ui(app: &Application, handler: Rc<RefCell<impl AppHandler>>, rx: async_channel::Receiver<GuiUpdate>) {
    let window = ApplicationWindow::builder().application(app).default_width(1200).default_height(800).title("Vein").build();

    // Header
    let header = HeaderBar::new();
    let toggle_btn = ToggleButton::builder().icon_name("sidebar-show-symbolic").active(true).build();
    header.pack_start(&toggle_btn);
    window.set_titlebar(Some(&header));

    let main_box = Box::new(Orientation::Horizontal, 0);

    // Sidebar
    let sidebar = Box::new(Orientation::Vertical, 0);
    sidebar.set_width_request(220);
    sidebar.add_css_class("sidebar");

    let stack = Stack::new();
    let rooms = ListBox::new();
    let state_view = handler.borrow().view();
    for (i, item) in state_view.nav_items.iter().enumerate() {
        rooms.append(&make_row(item, i == state_view.active_nav_index));
    }
    stack.add_titled(&rooms, Some("rooms"), "Rooms");

    // Shard Status Page
    let status_box = Box::new(Orientation::Vertical, 5);
    let shard_list = ListBox::new();
    shard_list.add_css_class("shard-list");
    shard_list.set_selection_mode(gtk4::SelectionMode::None);
    build_shard_rows(&shard_list, &state_view.shard_tree, 0);
    status_box.append(&shard_list);
    stack.add_titled(&status_box, Some("status"), "Status");

    sidebar.append(&stack);
    sidebar.append(&StackSwitcher::builder().stack(&stack).halign(Align::Center).margin_bottom(10).build());

    // Content Area
    let content_stack = Stack::new();
    content_stack.set_hexpand(true);

    // Comms View (Console + Input)
    let comms_box = Box::new(Orientation::Vertical, 0);

    // Console
    let scroller = ScrolledWindow::builder().vexpand(true).hscrollbar_policy(PolicyType::Never).build();
    let console = TextView::builder().editable(false).wrap_mode(gtk4::WrapMode::WordChar).build();
    console.add_css_class("console");
    scroller.set_child(Some(&console));
    comms_box.append(&scroller);

    // Input Area
    let input_box = Box::new(Orientation::Horizontal, 10);
    input_box.set_margin_start(10); input_box.set_margin_end(10); input_box.set_margin_bottom(10);

    let upload_btn = Button::builder().icon_name("folder-open-symbolic").valign(Align::End).build();
    let h_up = handler.clone();
    let w_up = window.downgrade();

    // Modern File Dialog
    upload_btn.connect_clicked(move |_| {
        let h = h_up.clone();
        let w_clone = w_up.clone();
        glib::MainContext::default().spawn_local(async move {
            if let Some(w) = w_clone.upgrade() {
                let dialog = FileDialog::builder().title("Upload").modal(true).build();
                let parent_window: Option<&Window> = Some(w.upcast_ref());

                // GTK 0.10: open_future uses async/await
                if let Ok(file) = dialog.open_future(parent_window).await {
                    if let Some(p) = file.path() {
                        h.borrow_mut().handle_event(Event::FileSelected(p));
                    }
                }
            }
        });
    });
    input_box.append(&upload_btn);

    let input_scroll = ScrolledWindow::builder().max_content_height(150).vexpand(false).propagate_natural_height(true).hscrollbar_policy(PolicyType::Never).build();
    input_scroll.set_hexpand(true);
    input_scroll.add_css_class("chat-input");

    let input_view = TextView::builder().wrap_mode(gtk4::WrapMode::WordChar).top_margin(8).bottom_margin(8).left_margin(8).right_margin(8).build();
    input_view.add_css_class("transparent-text");
    input_scroll.set_child(Some(&input_view));
    input_box.append(&input_scroll);

    let send_btn = Button::builder().icon_name("mail-send-symbolic").valign(Align::End).build();
    let h_send = handler.clone();
    let v_send = input_view.clone();
    let adj_scroll = scroller.vadjustment();

    let send_fn = Rc::new(move || {
        let b = v_send.buffer();
        let (s, e) = b.bounds();
        let txt = b.text(&s, &e, false).to_string();
        if !txt.trim().is_empty() {
            h_send.borrow_mut().handle_event(Event::Input(txt));
            b.set_text("");
            let a = adj_scroll.clone();
            glib::timeout_add_local(Duration::from_millis(50), move || { a.set_value(a.upper()); glib::ControlFlow::Break });
        }
    });

    let send_key = send_fn.clone();
    let ctrl = EventControllerKey::new();
    ctrl.connect_key_pressed(move |_, k, _, m| {
        if (k == Key::Return || k == Key::KP_Enter) && !m.contains(ModifierType::SHIFT_MASK) {
            send_key(); return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    input_view.add_controller(ctrl);
    send_btn.connect_clicked(move |_| send_fn());
    input_box.append(&send_btn);

    comms_box.append(&input_box);
    content_stack.add_named(&comms_box, Some("comms"));

    // Wolfpack View (Placeholder)
    let wolfpack_box = Box::new(Orientation::Vertical, 0);
    wolfpack_box.append(&Label::new(Some(":: WOLFPACK GRID ACTIVE ::")));
    content_stack.add_named(&wolfpack_box, Some("wolfpack"));

    // Layout Logic (Sidebar Position)
    let sep = Separator::new(Orientation::Vertical);

    if state_view.sidebar_position == SidebarPosition::Left {
        main_box.append(&sidebar);
        main_box.append(&sep);
        main_box.append(&content_stack);
    } else {
        main_box.append(&content_stack);
        main_box.append(&sep);
        main_box.append(&sidebar);
    }

    window.set_child(Some(&main_box));

    // ViewMode Switcher
    let stack_clone = content_stack.clone();
    let h_mode = handler.clone();
    glib::timeout_add_local(Duration::from_millis(200), move || {
        let mode = h_mode.borrow().view().mode;
        let target = match mode { ViewMode::Comms => "comms", ViewMode::Wolfpack => "wolfpack" };
        if stack_clone.visible_child_name().map(|s| s != target).unwrap_or(true) {
            stack_clone.set_visible_child_name(target);
        }
        glib::ControlFlow::Continue
    });

    // Connectors
    let s_box = sidebar.clone();
    toggle_btn.connect_toggled(move |b| s_box.set_visible(b.is_active()));

    let buf = console.buffer();
    let adj = scroller.vadjustment();
    let h_upd = handler.clone();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        h_upd.borrow_mut().handle_event(Event::TextBufferUpdate(buf.clone(), adj.clone()));
        glib::ControlFlow::Continue
    });

    // Styles
    let css = CssProvider::new();
    css.load_from_data("
        .sidebar { background: #1e1e1e; }
        .chat-input { background: #2d2d2d; border-radius: 20px; border: 1px solid #555; }
        .chat-input:focus-within { border-color: #3584e4; }
        .transparent-text { background: transparent; color: white; caret-color: white; }
        .console { font-family: 'Monospace'; font-size: 11pt; }
        .status-online { color: #2ec27e; }
        .status-error { color: #e01b24; }
        .status-thinking { color: #D7C3F1; }
    ");
    StyleContext::add_provider_for_display(&gtk4::gdk::Display::default().unwrap(), &css, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);

    window.present();

    // Event Loop
    let list_weak = shard_list.downgrade();
    let buf_weak = console.buffer().downgrade();

    glib::MainContext::default().spawn_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                GuiUpdate::ConsoleLog(t) => if let Some(b) = buf_weak.upgrade() {
                    b.insert(&mut b.end_iter(), &t);
                },
                GuiUpdate::ShardStatusChanged { id, status } => {
                    if let Some(list) = list_weak.upgrade() { update_shard_icon(&list, &id, status); }
                }
            }
        }
    });
}

fn make_row(label: &str, active: bool) -> Box {
    let b = Box::new(Orientation::Horizontal, 10); b.set_margin_top(5); b.set_margin_start(10);
    b.append(&Label::new(Some(label))); if active { b.append(&Label::new(Some("●"))); } b
}

fn build_shard_rows(list: &ListBox, shards: &[Shard], depth: usize) {
    for s in shards {
        let b = Box::new(Orientation::Horizontal, 10);
        b.set_margin_start(10 + (depth as i32 * 20)); b.set_margin_top(5); b.set_margin_bottom(5);
        let icon = Image::from_icon_name(match s.role { ShardRole::Root => "computer-symbolic", _ => "network-server-symbolic" });
        icon.set_widget_name(&s.id);
        b.append(&icon);
        b.append(&Label::new(Some(&s.name)));
        list.append(&b);
        build_shard_rows(list, &s.children, depth + 1);
    }
}

fn update_shard_icon(list: &ListBox, id: &str, status: ShardStatus) {
    let mut i = 0;
    while let Some(row) = list.row_at_index(i) {
        if let Some(b) = row.child().and_then(|c| c.downcast::<Box>().ok()) {
            if let Some(img) = b.first_child().and_then(|c| c.downcast::<Image>().ok()) {
                if img.widget_name() == id {
                    img.remove_css_class("status-online"); img.remove_css_class("status-error"); img.remove_css_class("status-thinking");
                    match status {
                        ShardStatus::Online => img.add_css_class("status-online"),
                        ShardStatus::Thinking => img.add_css_class("status-thinking"),
                        ShardStatus::Error => img.add_css_class("status-error"),
                        _ => {}
                    }
                }
            }
        }
        i += 1;
    }
}
