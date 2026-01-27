use libadwaita::prelude::*;
use libadwaita::{Application, ApplicationWindow, HeaderBar, WindowTitle, OverlaySplitView};
use gtk4::{
    Box, Orientation, Label, Button, Stack, ScrolledWindow,
    PolicyType, Align, ListBox, Separator, StackTransitionType
};
use std::rc::Rc;
use glib::clone;

// DIRECTIVE: UPDATE IDENTITY
// We establish the namespace 'org.unaos.vein' to claim our territory in the OS.
const APP_ID: &str = "org.unaos.vein";
const BUFFER_LIMIT: i32 = 50;

fn main() {
    let app = Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &Application) {
    // --- THE LAYOUT ARCHITECTURE ---
    let content_split = OverlaySplitView::new();

    // ---------------------------------------------------------
    // 1. THE SIDEBAR (Zed-Style Stack)
    // ---------------------------------------------------------
    let sidebar_box = Box::new(Orientation::Vertical, 0);
    sidebar_box.set_width_request(250);

    let sidebar_stack = Stack::new();
    sidebar_stack.set_vexpand(true);
    sidebar_stack.set_transition_type(StackTransitionType::SlideLeftRight);

    // PANEL A: Chat Rooms
    let rooms_list = ListBox::new();
    rooms_list.append(&make_sidebar_row("General", true));
    rooms_list.append(&make_sidebar_row("Encrypted", false));
    rooms_list.append(&make_sidebar_row("Jules (Private)", false));
    sidebar_stack.add_named(&rooms_list, "rooms");

    // PANEL B: Shard Status
    let status_box = Box::new(Orientation::Vertical, 10);
    status_box.set_margin_top(20);
    status_box.set_margin_start(10);
    status_box.set_margin_end(10);

    status_box.append(&Label::builder().label(":: DR. S8 DIAGNOSTICS ::").css_classes(vec!["heading"]).build());
    status_box.append(&make_status_row("S9 (Upload)", "🟢 Online"));
    status_box.append(&make_status_row("Vein (Cloud)", "🟡 Building..."));
    status_box.append(&make_status_row("Jules", "🔵 Thinking"));

    sidebar_stack.add_named(&status_box, "status");

    // THE DOCK
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

    // ---------------------------------------------------------
    // 2. THE MAIN STAGE
    // ---------------------------------------------------------
    let main_box = Box::new(Orientation::Vertical, 0);

    // The Header
    let header = HeaderBar::new();
    // UPDATED TITLE to reflect UnaOS Identity
    let title = WindowTitle::new("Vein", "UnaOS Control Node");
    header.set_title_widget(Some(&title));
    main_box.append(&header);

    let scrolled_window = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vexpand(true)
        .build();

    let chat_box = Box::new(Orientation::Vertical, 10);
    chat_box.set_margin_all(20);
    chat_box.set_valign(Align::End);

    // Initial Wake-Up Messages
    chat_box.append(&make_message("Vein", "Dr. S8 Online."));
    chat_box.append(&make_message("Vein", "Identity confirmed: org.unaos.vein"));

    scrolled_window.set_child(Some(&chat_box));
    main_box.append(&scrolled_window);

    // --- LOGIC: SMART PAGER & TRIMMER ---
    let enforce_limit = Rc::new(clone!(@weak chat_box => move || {
        let mut current_count = 0;
        let mut child = chat_box.first_child();
        while let Some(c) = child {
            current_count += 1;
            child = c.next_sibling();
        }

        if current_count > BUFFER_LIMIT {
            if let Some(oldest) = chat_box.first_child() {
                chat_box.remove(&oldest);
            }
        }
    }));

    let vadj = scrolled_window.vadjustment();

    // Scroll Listener
    vadj.connect_value_changed(clone!(@weak chat_box => move |adj| {
        let current_scroll = adj.value();
        if current_scroll < 1.0 {
            // Future history fetch point
        }
    }));

    // --- INPUT AREA ---
    let input_box = Box::new(Orientation::Horizontal, 10);
    input_box.set_margin_all(10);
    input_box.add_css_class("linked");

    let input_entry = gtk4::Entry::builder().placeholder_text("Enter Directive...").hexpand(true).build();
    let send_btn = Button::builder().icon_name("mail-send-symbolic").css_classes(vec!["suggested-action"]).build();

    send_btn.connect_clicked(clone!(@weak chat_box, @weak input_entry, @weak vadj, @strong enforce_limit => move |_| {
        let text = input_entry.text();
        if text.is_empty() { return; }

        chat_box.append(&make_message("Architect", &text));

        let response = format!("Acknowledged: {}", text);
        chat_box.append(&make_message("Vein", &response));

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
}

// --- HELPER FUNCTIONS ---

fn make_sidebar_row(name: &str, active: bool) -> Box {
    let row = Box::new(Orientation::Horizontal, 10);
    row.set_margin_all(10);
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
    row.set_margin_all(5);
    let l1 = Label::builder().label(shard).hexpand(true).xalign(0.0).build();
    let l2 = Label::new(Some(status));
    row.append(&l1);
    row.append(&l2);
    row
}

fn make_message(sender: &str, content: &str) -> Box {
    let msg_box = Box::new(Orientation::Vertical, 5);
    msg_box.set_margin_bottom(15);

    let sender_lbl = Label::builder()
        .label(sender)
        .css_classes(vec!["dim-label", "caption"])
        .xalign(0.0)
        .build();

    let bubble = Label::builder()
        .label(content)
        .wrap(true)
        .xalign(0.0)
        .build();

    msg_box.append(&sender_lbl);
    msg_box.append(&bubble);
    msg_box
}
