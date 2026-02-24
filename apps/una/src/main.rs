use anyhow::Result;
use bandy::{MatrixEvent, SMessage};
use crossbeam_channel::unbounded;
use glib::clone;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box, HeaderBar, Orientation, Paned, Stack, StackSwitcher,
    StackTransitionType, Widget,
};
use std::cell::RefCell;
use std::env;
use std::rc::Rc;

use matrix::create_view as create_matrix_view;
use tabula::{EditorMode, TabulaView};

const APP_ID: &str = "org.unaos.UnaIDE";

fn main() -> Result<()> {
    println!(":: UNA :: WAKING UP THE FORGE...");
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run();
    Ok(())
}

fn build_ui(app: &Application) {
    // 1. THE OS BUS (Bandy)
    let (nerve_tx, nerve_rx) = unbounded::<SMessage>();
    let (glib_tx, glib_rx) = glib::MainContext::channel(glib::PRIORITY_DEFAULT);

    std::thread::spawn(move || {
        while let Ok(msg) = nerve_rx.recv() {
            let _ = glib_tx.send(msg);
        }
    });

    let cwd = env::current_dir().unwrap_or_default();

    // 2. THE SIDEBAR (Matrix, Vaire, Aule)
    let stack = Stack::new();
    stack.set_vexpand(true);
    stack.set_transition_type(StackTransitionType::SlideLeftRight);

    let matrix_widget = create_matrix_view(nerve_tx.clone(), &cwd);
    stack.add_titled(&matrix_widget, Some("matrix"), "Matrix");

    // TODO: Add Vaire and Aule back to the stack once they accept nerve_tx
    // stack.add_titled(&vaire::create_view(), Some("vaire"), "Vairë");
    // stack.add_titled(&aule::create_view(nerve_tx.clone()), Some("aule"), "Aulë");

    let switcher = StackSwitcher::builder().stack(&stack).build();

    let sidebar = Box::new(Orientation::Vertical, 0);
    sidebar.set_width_request(260);
    sidebar.append(&switcher);
    sidebar.append(&stack);

    // 3. THE WORKSPACE (Tabula & Midden)
    let tabula = Rc::new(RefCell::new(TabulaView::new(EditorMode::Code("rust".to_string()))));
    let tabula_widget = tabula.borrow().widget();

    // TODO: Add Midden back to the bottom pane
    // let (midden_widget, midden_buf) = midden::create_view();

    let workspace = Paned::builder()
        .orientation(Orientation::Vertical)
        .start_child(&tabula_widget)
        // .end_child(&midden_widget)
        .position(600) // Height of Tabula before Midden starts
        .build();

    // 4. THE MASTER LAYOUT
    let main_paned = Paned::builder()
        .orientation(Orientation::Horizontal)
        .start_child(&sidebar)
        .end_child(&workspace)
        .position(260)
        .resize_start_child(false)
        .shrink_start_child(false)
        .build();

    let header = HeaderBar::new();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("UnaOS - The Forge")
        .default_width(1400)
        .default_height(900)
        .titlebar(&header)
        .child(&main_paned)
        .build();

    // 5. WIRE THE REFLEX ARC
    let tabula_clone = tabula.clone();
    glib_rx.attach(None, move |msg| {
        match msg {
            SMessage::Matrix(MatrixEvent::NodeSelected(path)) => {
                println!("[UNA] 🧠 Impulse Caught: Routing {:?} to Tabula", path);
                tabula_clone.borrow().load_file(&path);
            }
            // SMessage::TerminalOutput(text) => { ... route to Midden ... }
            _ => {}
        }
        glib::ControlFlow::Continue
    });

    window.present();
}
