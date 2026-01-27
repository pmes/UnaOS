// DIRECTIVE: STABILIZED CORE
#![allow(deprecated)]

use libadwaita::prelude::*;
use libadwaita::{Application, ApplicationWindow, HeaderBar, WindowTitle, OverlaySplitView};
use gtk4::{
    Box, Orientation, Label, Button, Stack, ScrolledWindow,
    PolicyType, Align, ListBox, Separator, StackTransitionType
};
use std::rc::Rc;
use std::cell::RefCell;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use glib::clone;
use serde::{Deserialize, Serialize};

const APP_ID: &str = "org.unaos.vein";
const BUFFER_LIMIT: usize = 50;

// --- MEMORY STRUCTURES ---
#[derive(Serialize, Deserialize, Clone, Debug)]
struct StoredMessage {
    sender: String,
    content: String,
}

fn main() {
    let app = Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &Application) {
    // --- LAYOUT ---
    let content_split = OverlaySplitView::new();

    // 1. SIDEBAR
    let sidebar_box = Box::new(Orientation::Vertical, 0);
    sidebar_box.set_width_request(250);

    let sidebar_stack = Stack::new();
    sidebar_stack.set_vexpand(true);
    sidebar_stack.set_transition_type(StackTransitionType::SlideLeftRight);

    // Sidebar: Rooms
    let rooms_list = ListBox::new();
    rooms_list.append(&make_sidebar_row("General", true));
    rooms_list.append(&make_sidebar_row("Encrypted", false));
    rooms_list.append(&make_sidebar_row("Jules (Private)", false));
    sidebar_stack.add_named(&rooms_list, Some("rooms"));

    // Sidebar: Status
    let status_box = Box::new(Orientation::Vertical, 10);
    status_box.set_margin_top(20);
    status_box.set_margin_start(10);
    status_box.set_margin_end(10);

    status_box.append(&Label::builder().label(":: DR. S8 DIAGNOSTICS ::").css_classes(vec!["heading"]).build());
    status_box.append(&make_status_row("S9 (Upload)", "🟢 Online"));
    status_box.append(&make_status_row("Vein (Cloud)", "🟡 Building..."));
    status_box.append(&make_status_row("Jules", "🔵 Thinking"));
    sidebar_stack.add_named(&status_box, Some("status"));

    // Sidebar: Dock
    let bottom_dock = Box::new(Orientation::Horizontal, 5);
    bottom_dock.set_margin_top(10);
    bottom_dock.set_margin_bottom(10);
    bottom_dock.set_halign(Align::Center);

    let btn_rooms = Button::builder().icon_name("mail-message-new-symbolic").build();
    btn_rooms.connect_clicked(clone!(@weak sidebar_stack => move |_| {
        sidebar_stack.set_visible_child_name("rooms");
    }));

    let btn_status = Button::builder().icon_name("system-run-symbolic").build();
    btn_status.connect_clicked(clone!(@weak sidebar_stack => move |_| {
        sidebar_stack.set_visible_child_name("status");
    }));

    bottom_dock.append(&btn_rooms);
    bottom_dock.append(&btn_status);
    sidebar_box.append(&sidebar_stack);
    sidebar_box.append(&Separator::new(Orientation::Horizontal));
    sidebar_box.append(&bottom_dock);
    content_split.set_sidebar(Some(&sidebar_box));

    // 2. MAIN STAGE
    let main_box = Box::new(Orientation::Vertical, 0);
    let header = HeaderBar::new();
    let title = WindowTitle::new("Vein", "UnaOS Control Node");
    header.set_title_widget(Some(&title));
    main_box.append(&header);

    // FIX: CPU FAN ISSUE
    // We set Vertical Policy to ALWAYS. This prevents the "Scrollbar Toggle Loop"
    // where the UI constantly resizes itself, pinning a CPU core.
    let scrolled_window = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Always)
        .vexpand(true)
        .build();

    let chat_box = Box::new(Orientation::Vertical, 10);
    chat_box.set_margin_top(20);
    chat_box.set_margin_bottom(20);
    chat_box.set_margin_start(20);
    chat_box.set_margin_end(20);
    chat_box.set_valign(Align::End);

    // --- LOGIC: MEMORY RECALL (LOAD) ---
    let history = load_history();
    if history.is_empty() {
        println!(":: MEMORY :: No history found. Starting fresh.");
        chat_box.append(&make_message("Vein", "Dr. S8 Online. Memory Empty."));
    } else {
        println!(":: MEMORY :: Loaded {} messages.", history.len());
        for msg in &history {
            chat_box.append(&make_message(&msg.sender, &msg.content));
        }
    }

    scrolled_window.set_child(Some(&chat_box));
    main_box.append(&scrolled_window);

    let runtime_history = Rc::new(RefCell::new(history));

    // --- LOGIC: TRIMMER ---
    let enforce_limit = Rc::new(clone!(@weak chat_box => move || {
        let mut current_count = 0;
        let mut child = chat_box.first_child();
        while let Some(c) = child {
            current_count += 1;
            child = c.next_sibling();
        }
        if current_count > BUFFER_LIMIT as i32 {
            if let Some(oldest) = chat_box.first_child() {
                chat_box.remove(&oldest);
            }
        }
    }));

    let vadj = scrolled_window.vadjustment();

    // --- INPUT AREA ---
    let input_box = Box::new(Orientation::Horizontal, 10);
    input_box.set_margin_top(10);
    input_box.set_margin_bottom(10);
    input_box.set_margin_start(10);
    input_box.set_margin_end(10);
    input_box.add_css_class("linked");

    let input_entry = gtk4::Entry::builder().placeholder_text("Enter Directive...").hexpand(true).build();
    let send_btn = Button::builder().icon_name("mail-send-symbolic").css_classes(vec!["suggested-action"]).build();

    // SEND BUTTON LOGIC
    send_btn.connect_clicked(clone!(@weak chat_box, @weak input_entry, @weak vadj, @strong enforce_limit, @strong runtime_history => move |_| {
        let text = input_entry.text();
        if text.is_empty() { return; }

        chat_box.append(&make_message("Architect", &text));
        let response = format!("Acknowledged: {}", text);
        chat_box.append(&make_message("Vein", &response));

        {
            let mut hist = runtime_history.borrow_mut();
            hist.push(StoredMessage { sender: "Architect".into(), content: text.to_string() });
            hist.push(StoredMessage { sender: "Vein".into(), content: response });

            if hist.len() > BUFFER_LIMIT {
                let overflow = hist.len() - BUFFER_LIMIT;
                hist.drain(0..overflow);
            }
            save_history(&hist);
        }

        input_entry.set_text("");
        enforce_limit();

        let adj_clone = vadj.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            adj_clone.set_value(adj_clone.upper());
            glib::ControlFlow::Break
        });
    }));

    input_box.append(&input_entry);
    input_box.append(&send_btn);
    main_box.append(&input_box);
    content_split.set_content(Some(&main_box));

    let window = ApplicationWindow::builder()
        .application(app)
        .default_width(1000)
        .default_height(700)
        .content(&content_split)
        .title("Vein Dwelling")
        .build();

    window.present();

    let adj_initial = vadj.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        adj_initial.set_value(adj_initial.upper());
        glib::ControlFlow::Break
    });
}

// --- PERSISTENCE HELPERS ---

fn get_history_path() -> PathBuf {
    let mut path = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    path.push(".local/share/unaos/vein");

    if let Err(e) = fs::create_dir_all(&path) {
        eprintln!(":: ERROR :: Could not create config dir: {}", e);
    }

    path.push("history.json");
    // DEBUG LOG
    println!(":: SYSTEM :: Storage Path: {:?}", path);
    path
}

fn load_history() -> Vec<StoredMessage> {
    let path = get_history_path();
    if !path.exists() {
        return Vec::new();
    }

    match File::open(path) {
        Ok(mut file) => {
            let mut content = String::new();
            if file.read_to_string(&mut content).is_ok() {
                match serde_json::from_str::<Vec<StoredMessage>>(&content) {
                    Ok(data) => return data,
                    Err(e) => println!(":: ERROR :: JSON Corrupt: {}", e),
                }
            }
        }
        Err(e) => println!(":: ERROR :: File Read Failed: {}", e),
    }
    Vec::new()
}

fn save_history(history: &Vec<StoredMessage>) {
    let path = get_history_path();
    if let Ok(json) = serde_json::to_string_pretty(history) {
        if let Ok(mut file) = File::create(path) {
            let _ = file.write_all(json.as_bytes());
            println!(":: MEMORY :: Saved.");
        }
    }
}

// --- UI HELPERS ---

fn make_sidebar_row(name: &str, active: bool) -> Box {
    let row = Box::new(Orientation::Horizontal, 10);
    row.set_margin_top(10);
    row.set_margin_bottom(10);
    row.set_margin_start(10);
    row.set_margin_end(10);

    let label = Label::new(Some(name));
    row.append(&label);
    if active {
        let dot = Label::new(Some("●"));
        row.append(&dot);
    }
    row
}

fn make_status_row(shard: &str, status: &str) -> Box {
    let row = Box::new(Orientation::Horizontal, 10);
    row.set_margin_top(5);
    row.set_margin_bottom(5);
    row.set_margin_start(5);
    row.set_margin_end(5);

    let l1 = Label::builder().label(shard).hexpand(true).xalign(0.0).build();
    let l2 = Label::new(Some(status));
    row.append(&l1);
    row.append(&l2);
    row
}

fn make_message(sender: &str, content: &str) -> Box {
    let msg_box = Box::new(Orientation::Vertical, 5);
    msg_box.set_margin_bottom(15);

    let sender_lbl = Label::builder().label(sender).css_classes(vec!["dim-label", "caption"]).xalign(0.0).build();
    let bubble = Label::builder().label(content).wrap(true).xalign(0.0).build();

    msg_box.append(&sender_lbl);
    msg_box.append(&bubble);
    msg_box
}
