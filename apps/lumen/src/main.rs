use dotenvy::dotenv;
use elessar::prelude::*;
use log::info;
use std::rc::Rc;
use vein::{CommsSpline, VeinHandler};

fn main() {
    dotenv().ok();
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    // Initialize Rustls Crypto Provider (Ring)
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    info!(":: LUMEN :: Booting...");

    let (gui_tx, gui_rx) = async_channel::unbounded();

    // Logic
    let app = VeinHandler::new(gui_tx);

    // View
    let spline = Rc::new(CommsSpline::new());
    let bootstrap = move |window: &ApplicationWindow, tx, rx| spline.bootstrap(window, tx, rx);

    // Engine
    Backend::new("org.unaos.lumen", app, gui_rx, bootstrap);
}
