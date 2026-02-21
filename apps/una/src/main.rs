use async_channel::{Receiver, Sender};
use dotenvy::dotenv;
use elessar::prelude::*;
use gtk4::{
    Align, Box, HeaderBar, Orientation, Paned, ScrolledWindow, Stack, StackSwitcher,
    StackTransitionType, TextBuffer,
};
use log::info;
use std::rc::Rc;

// Handlers
use aule;
use matrix;
use midden;
use tabula;
use vaire;

struct UnaApp {
    gui_tx: Sender<GuiUpdate>,
}

impl UnaApp {
    fn new(gui_tx: Sender<GuiUpdate>) -> Self {
        Self { gui_tx }
    }
    fn log(&self, msg: &str) {
        let _ = self
            .gui_tx
            .send_blocking(GuiUpdate::ConsoleLog(msg.to_string()));
    }
}

impl AppHandler for UnaApp {
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::AuleIgnite => self.log("[AULË] :: Ignition Sequence Start...\n"),
            Event::MatrixFileClick(path) => match std::fs::read_to_string(&path) {
                Ok(content) => {
                    self.log(&format!("[MATRIX] :: Loaded {}\n", path.display()));
                    let _ = self.gui_tx.send_blocking(GuiUpdate::EditorLoad(content));
                }
                Err(e) => self.log(&format!("[MATRIX ERROR] :: {}\n", e)),
            },
            _ => {}
        }
    }
    fn view(&self) -> DashboardState {
        DashboardState::default()
    }
}

struct IdeSpline {}

impl IdeSpline {
    fn new() -> Self {
        Self {}
    }

    fn bootstrap<W: IsA<Window> + IsA<Widget> + Cast>(
        &self,
        window: &W,
        tx: Sender<Event>,
        rx: Receiver<GuiUpdate>,
    ) -> Widget {
        window.set_title(Some("Una (The IDE)"));

        let header = HeaderBar::new();
        let main_box = Box::new(Orientation::Horizontal, 0);

        // Sidebar
        let sidebar = Box::new(Orientation::Vertical, 0);
        sidebar.set_width_request(250);
        sidebar.add_css_class("sidebar");

        let stack = Stack::new();
        stack.set_vexpand(true);
        stack.set_transition_type(StackTransitionType::SlideLeftRight);

        stack.add_titled(&matrix::create_view(tx.clone()), Some("matrix"), "Matrix");
        stack.add_titled(&vaire::create_view(), Some("vaire"), "Vairë");
        stack.add_titled(&aule::create_view(tx.clone()), Some("aule"), "Aulë");

        sidebar.append(&stack);

        let switcher = StackSwitcher::builder().stack(&stack).build();
        let sw_box = Box::new(Orientation::Horizontal, 0);
        sw_box.set_halign(Align::Center);
        sw_box.append(&switcher);
        sidebar.append(&sw_box);
        main_box.append(&sidebar);

        // Workspace
        let paned = Paned::new(Orientation::Vertical);
        paned.set_hexpand(true);
        paned.set_vexpand(true);
        paned.set_position(400);

        let (tabula_widget, tabula_buf) = tabula::create_view(tabula::EditorMode::Prose);
        paned.set_start_child(Some(&tabula_widget));

        let (midden_widget, midden_buf) = midden::create_view();
        paned.set_end_child(Some(&midden_widget));

        main_box.append(&paned);

        // RX Loop
        let midden_buf_clone = midden_buf.clone();
        let tabula_buf_clone = tabula_buf.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(update) = rx.recv().await {
                match update {
                    GuiUpdate::ConsoleLog(text) => {
                        let mut end = midden_buf_clone.end_iter();
                        midden_buf_clone.insert(&mut end, &text);
                    }
                    GuiUpdate::EditorLoad(content) => {
                        tabula_buf_clone.set_text(&content);
                    }
                    _ => {}
                }
            }
        });

        // Return
        #[cfg(feature = "gnome")]
        {
            let view = libadwaita::ToolbarView::new();
            view.add_top_bar(&header);
            view.set_content(Some(&main_box));
            view.upcast::<Widget>()
        }
        #[cfg(not(feature = "gnome"))]
        {
            if let Some(app_win) = window.dynamic_cast_ref::<gtk4::ApplicationWindow>() {
                app_win.set_titlebar(Some(&header));
            }
            main_box.into()
        }
    }
}

fn main() {
    dotenv().ok();
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();
    info!(":: UNA :: Booting...");

    let (gui_tx, gui_rx) = async_channel::unbounded();
    let app = UnaApp::new(gui_tx);
    let spline = Rc::new(IdeSpline::new());

    Backend::new("org.unaos.una", app, gui_rx, move |w, tx, rx| {
        spline.bootstrap(w, tx, rx)
    });
}
