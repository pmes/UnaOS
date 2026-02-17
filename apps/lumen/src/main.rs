use directories::BaseDirs;
use dotenvy::dotenv;
use elessar::prelude::*;
use elessar::quartzite;
use log::info;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use vein::{CommsSpline, VeinHandler};

// Directive S68: The Lumen Homestead
fn get_lumen_home() -> PathBuf {
    let base_dirs = BaseDirs::new().expect("Alien Soil not supported");
    let mut path = base_dirs.data_local_dir().to_path_buf();
    path.push("unaos");
    path.push("lumen");

    if !path.exists() {
        fs::create_dir_all(&path).expect("Failed to establish base camp");
    }

    path
}

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

    // 1. Establish Base Camp
    let home = get_lumen_home();
    let asset_path = home.join("quartzite.gresource");
    let history_path = home.join("history.json");

    // 2. Deploy Assets (Stop Gap)
    if !asset_path.exists() {
        info!(":: LUMEN :: Deploying assets to {}", asset_path.display());
        if let Err(e) = quartzite::deploy_assets(&asset_path) {
            log::error!("Failed to deploy assets: {}", e);
        }
    }

    // 3. Initialize Quartzite with specific path
    quartzite::init_with_path(&asset_path);

    let (gui_tx, gui_rx) = async_channel::unbounded();

    // Logic (With History Path)
    let app = VeinHandler::new(gui_tx, history_path);

    // View
    let spline = Rc::new(CommsSpline::new());
    let bootstrap = move |window: &ApplicationWindow, tx, rx| spline.bootstrap(window, tx, rx);

    // Engine
    Backend::new("org.unaos.lumen", app, gui_rx, bootstrap);
}
