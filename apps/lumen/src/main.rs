use dotenvy::dotenv;
use elessar::prelude::*;
use std::rc::Rc;
use vein::{VeinHandler, CommsSpline};
use log::info;

fn main() {
    dotenv().ok();
    env_logger::Builder::from_default_env().filter_level(log::LevelFilter::Info).init();
    info!(":: LUMEN :: Booting...");

    let (gui_tx, gui_rx) = async_channel::unbounded();

    // Logic
    let app = VeinHandler::new(gui_tx);

    // View
    let spline = Rc::new(CommsSpline::new());
    let bootstrap = move |window: &ApplicationWindow, tx, rx| {
        spline.bootstrap(window, tx, rx)
    };

    // Engine
    Backend::new("org.unaos.lumen", app, gui_rx, bootstrap);
}
