use anyhow::Result;
use gtk4::prelude::*;
use gtk4::{
    Box, HeaderBar, Orientation, Paned, Stack, StackSwitcher,
    StackTransitionType,
};
use std::cell::RefCell;
use std::env;
use std::rc::Rc;
use std::path::PathBuf;

use gneiss_pal::{Event, GuiUpdate};
use quartzite::{Backend, NativeView, NativeWindow};

use matrix::create_view as create_matrix_view;
use tabula::{EditorMode, TabulaView};

const APP_ID: &str = "org.unaos.UnaIDE";

fn main() -> Result<()> {
    println!(":: UNA :: WAKING UP THE FORGE...");

    // 0. Ignite the Substrate Reactor (Tokio)
    let rt = tokio::runtime::Runtime::new().expect("CRITICAL: Failed to ignite Tokio reactor");
    let _guard = rt.enter();

    // 1. Establish async_channel pairs
    let (tx_brain, rx_brain) = async_channel::unbounded::<Event>();
    let (tx_gui, rx_gui) = async_channel::unbounded::<GuiUpdate>();

    // 2. Spawn central background task (Tokio)
    rt.spawn(async move {
        while let Ok(event) = rx_brain.recv().await {
            match event {
                Event::FileSelected(path) => {
                    println!("[UNA CORE] 🧠 Routing Impulse: {:?}", path);
                    // Bouncing it as EditorLoad to trigger tabula
                    let _ = tx_gui.send(GuiUpdate::EditorLoad(path.to_string_lossy().to_string())).await;
                }
                _ => {}
            }
        }
    });

    let cwd = env::current_dir().unwrap_or_default();

    // THE FUSION
    let bootstrap = move |window: &NativeWindow| -> NativeView {
        // Prevent GTK from drawing its default top titlebar
        let dummy_titlebar = gtk4::Box::new(Orientation::Horizontal, 0);
        window.set_titlebar(Some(&dummy_titlebar));

        // 3. THE SIDEBAR (Left Pane)
        let left_stack = Stack::new();
        left_stack.set_vexpand(true);
        left_stack.set_transition_type(StackTransitionType::SlideLeftRight);

        let matrix_widget = create_matrix_view(tx_brain.clone(), &cwd);
        left_stack.add_titled(&matrix_widget, Some("matrix"), "Matrix");

        let left_switcher = StackSwitcher::builder().stack(&left_stack).build();
        let left_toolbar = Box::new(Orientation::Horizontal, 0);
        left_toolbar.add_css_class("toolbar");
        left_toolbar.append(&left_switcher);

        let left_header = HeaderBar::builder().show_title_buttons(false).build();

        let left_vbox = Box::new(Orientation::Vertical, 0);
        left_vbox.set_width_request(260);
        left_vbox.append(&left_header);
        left_vbox.append(&left_toolbar);
        left_vbox.append(&left_stack);


        // 4. THE WORKSPACE (Right Pane)
        let right_stack = Stack::new();
        right_stack.set_vexpand(true);
        right_stack.set_transition_type(StackTransitionType::SlideLeftRight);

        let tabula = Rc::new(RefCell::new(TabulaView::new(EditorMode::Code(
            "rust".to_string(),
        ))));
        let tabula_widget = tabula.borrow().widget();
        right_stack.add_titled(&tabula_widget, Some("tabula"), "Editor");

        let right_switcher = StackSwitcher::builder().stack(&right_stack).build();
        let right_toolbar = Box::new(Orientation::Horizontal, 0);
        right_toolbar.add_css_class("toolbar");
        right_toolbar.append(&right_switcher);

        let right_header = HeaderBar::builder().show_title_buttons(true).build();

        let right_vbox = Box::new(Orientation::Vertical, 0);
        right_vbox.set_hexpand(true);
        right_vbox.append(&right_header);
        right_vbox.append(&right_toolbar);
        right_vbox.append(&right_stack);

        // 5. THE MASTER LAYOUT
        let main_paned = Paned::builder()
            .orientation(Orientation::Horizontal)
            .start_child(&left_vbox)
            .end_child(&right_vbox)
            .position(260)
            .resize_start_child(false)
            .shrink_start_child(false)
            .wide_handle(true)
            .build();

        // 6. WIRE THE REFLEX ARC (UI Receiver Loop)
        let tabula_clone = tabula.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(update) = rx_gui.recv().await {
                match update {
                    GuiUpdate::EditorLoad(path_str) => {
                        let path = PathBuf::from(path_str);
                        println!("[UNA UI] ⚡ Loading into Tabula: {:?}", path);
                        tabula_clone.borrow().load_file(&path);
                    }
                    _ => {}
                }
            }
        });

        main_paned.upcast::<gtk4::Widget>()
    };

    // 7. Ignite Quartzite
    Backend::new(APP_ID, bootstrap).run();

    Ok(())
}