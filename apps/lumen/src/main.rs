use directories::BaseDirs;
use dotenvy::dotenv;
use bandy::SMessage;
use elessar::prelude::*;
use junct::JunctHandler;
use log::info;
use quartzite;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use tokio::sync::broadcast;
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
    // THE ANCHOR (Contextual Grounding)
    info!(":: LUMEN :: UnaOS Root Directory: {}", home.display());

    let asset_path = home.join("quartzite.gresource");
    let history_path = home.join("history.json");

    // 2. FORCE DEPLOY (S74)
    info!(":: LUMEN :: Deploying assets to {}", asset_path.display());
    if let Err(e) = quartzite::deploy_assets(&asset_path) {
        log::error!("Failed to deploy assets: {}", e);
    }

    // 3. Initialize Quartzite with specific path
    quartzite::init_with_path(&asset_path);

    // 4. Initialize Nervous System (Bandy)
    let (bandy_tx, _bandy_rx) = broadcast::channel::<SMessage>(100);

    let (gui_tx, gui_rx) = async_channel::unbounded();

    // Logic (With History Path)
    let app = VeinHandler::new(gui_tx, history_path, bandy_tx.clone());

    // 5. Initialize Voice (Junct)
    let _voice = match JunctHandler::new(bandy_tx.clone()) {
        Ok(v) => {
            info!(":: JUNCT :: Listening...");
            Some(v)
        }
        Err(e) => {
            log::warn!(":: JUNCT :: Failed to initialize microphone: {}", e);
            None
        }
    };

    // Test Pulse
    let _ = bandy_tx.send(SMessage::Log {
        level: "INFO".into(),
        source: "LUMEN".into(),
        content: "Nervous System Online".into(),
    });

    // View
    let spline = Rc::new(CommsSpline::new());
    let bootstrap = move |window: &ApplicationWindow, tx, rx| spline.bootstrap(window, tx, rx);

    // Engine
    Backend::new("org.unaos.lumen", app, gui_rx, bootstrap);
}
