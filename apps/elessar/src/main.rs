// apps/elessar/src/main.rs (The Host)
use dotenvy::dotenv;
use gneiss_pal::{AppHandler, DashboardState, Event, GuiUpdate};
use quartzite::Backend as QuartziteBackend;
use std::sync::{Arc, Mutex};
use std::rc::Rc;
use std::cell::RefCell;
use log::info;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use gtk4::prelude::*;
use gtk4::{Adjustment, TextBuffer, ApplicationWindow};
use glib::ControlFlow;
use std::time::Duration;

mod splines;
use splines::ide::{IdeSpline, load_tabula_text};

// Elessar doesn't need the full Vein State for this Exam
struct ElessarState {
    // Placeholder
}

struct ElessarApp {
    gui_tx: async_channel::Sender<GuiUpdate>,
}

impl ElessarApp {
    fn new(gui_tx: async_channel::Sender<GuiUpdate>) -> Self {
        Self { gui_tx }
    }

    fn append_to_console(&self, text: &str) {
        let _ = self.gui_tx.send_blocking(GuiUpdate::ConsoleLog(text.to_string()));
    }
}

impl AppHandler for ElessarApp {
    fn handle_event(&mut self, event: Event) {
        match event {
             Event::AuleIgnite => {
                self.append_to_console("[AULË] :: Ignition Sequence Start...\n");
            },
            Event::MatrixFileClick(path) => {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        load_tabula_text(&content);
                        self.append_to_console(&format!("[MATRIX] :: Loaded {}\n", path.display()));
                    },
                    Err(e) => {
                        self.append_to_console(&format!("[MATRIX ERROR] :: {}\n", e));
                    }
                }
            },
            _ => {}
        }
    }

    fn view(&self) -> DashboardState {
        DashboardState::default() // Stub
    }
}

fn main() {
    dotenv().ok();
    env_logger::Builder::from_default_env().filter_level(log::LevelFilter::Info).try_init().ok();

    info!(":: ELESSAR :: Booting...");

    let (gui_tx, gui_rx) = async_channel::unbounded();

    let app = ElessarApp::new(gui_tx.clone());

    let ide_spline = Rc::new(IdeSpline::new());

    // Bootstrap Closure
    let bootstrap = move |window: &ApplicationWindow, tx: async_channel::Sender<Event>, rx: async_channel::Receiver<GuiUpdate>| {
        ide_spline.bootstrap(window, tx, rx)
    };

    QuartziteBackend::new("org.unaos.elessar", app, gui_rx, bootstrap);
}
