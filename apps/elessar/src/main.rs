// apps/elessar/src/main.rs (The Host)
use dotenvy::dotenv;
use gneiss_pal::{AppHandler, Backend, DashboardState, Event, GuiUpdate, ViewMode, SidebarPosition};
use std::sync::{Arc, Mutex};
use std::rc::Rc;
use std::cell::RefCell;
use log::info;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use gtk4::prelude::*;
use gtk4::{Adjustment, TextBuffer};
use glib::ControlFlow;
use std::time::Duration;

mod splines;
use splines::ide::{IdeSpline, load_tabula_text};

// Elessar doesn't need the full Vein State for this Exam, just enough to satisfy AppHandler
struct ElessarState {
    // Placeholder
}

#[derive(Clone)]
struct UiUpdater {
    text_buffer: TextBuffer,
    scroll_adj: Adjustment,
}

fn do_append_and_scroll(ui_updater_rc: &Rc<RefCell<Option<UiUpdater>>>, text: &str) {
    if let Some(ref ui_updater) = *ui_updater_rc.borrow() {
        let mut end_iter = ui_updater.text_buffer.end_iter();
        ui_updater.text_buffer.insert(&mut end_iter, text);

        let adj_clone = ui_updater.scroll_adj.clone();
        glib::timeout_add_local(Duration::from_millis(50), move || {
            adj_clone.set_value(adj_clone.upper());
            ControlFlow::Break
        });
    }
}

struct ElessarApp {
    ui_updater: Rc<RefCell<Option<UiUpdater>>>,
}

impl ElessarApp {
    fn new(ui_updater: Rc<RefCell<Option<UiUpdater>>>) -> Self {
        Self { ui_updater }
    }

    fn append_to_console(&self, text: &str) {
        do_append_and_scroll(&self.ui_updater, text);
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
            Event::TextBufferUpdate(buffer, adj) => {
                // This is the Midden (Terminal) buffer
                *self.ui_updater.borrow_mut() = Some(UiUpdater {
                    text_buffer: buffer,
                    scroll_adj: adj,
                });
                self.append_to_console("[ELESSAR] :: Midden Connected.\n");
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

    let (gui_tx, gui_rx) = async_channel::unbounded(); // Not really used but required by Backend

    let ui_updater = Rc::new(RefCell::new(None::<UiUpdater>));
    let app = ElessarApp::new(ui_updater.clone());

    let ide_spline = Arc::new(IdeSpline::new());
    let ide_spline_clone = ide_spline.clone();

    // S40: Bootstrap into IdeSpline
    Backend::new("org.unaos.elessar", app, gui_rx, move |window, tx, rx| {
        ide_spline_clone.bootstrap(window, tx, rx)
    });
}
