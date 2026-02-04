mod api;
use api::{GeminiClient, Content, Part};
use gneiss_pal::{
    AppHandler, Backend, DashboardState, Event, BrainManager, Shard, ShardRole, ShardStatus,
    SavedMessage, ViewMode, GuiUpdate, SidebarPosition
};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use chrono::Local;
use std::path::PathBuf;
use gtk4::prelude::*;
use log::info;
use dotenvy::dotenv;
use gtk4::{gio, glib};

#[derive(serde::Deserialize)]
struct VertexPacket { id: String, status: String }

struct VeinApp {
    state: Arc<Mutex<DashboardState>>,
    tx_logic: mpsc::Sender<String>,
    tx_upload: mpsc::Sender<PathBuf>,
}

async fn run_brain(
    mut rx: mpsc::Receiver<String>,
    mut rx_files: mpsc::Receiver<PathBuf>,
    state: Arc<Mutex<DashboardState>>,
    brain_io: BrainManager,
    gui_tx: async_channel::Sender<GuiUpdate>
) {
    let client = GeminiClient::new().await.unwrap_or_else(|e| {
        let _ = gui_tx.send_blocking(GuiUpdate::ConsoleLog(format!("FATAL: {}\n", e)));
        panic!("Brain Death");
    });

    // UDP LISTENER
    let gui_udp = gui_tx.clone();
    tokio::spawn(async move {
        let sock = tokio::net::UdpSocket::bind("0.0.0.0:4200").await.unwrap();
        let mut buf = [0u8; 1024];
        loop {
            if let Ok((len, _)) = sock.recv_from(&mut buf).await {
                if let Ok(p) = serde_json::from_slice::<VertexPacket>(&buf[..len]) {
                    let s = match p.status.as_str() { "thinking" => ShardStatus::Thinking, "error" => ShardStatus::Error, _ => ShardStatus::Online };
                    let _ = gui_udp.send(GuiUpdate::ShardStatusChanged { id: p.id, status: s }).await;
                }
            }
        }
    });

    loop {
        tokio::select! {
            Some(path) = rx_files.recv() => {
                let _ = gui_tx.send(GuiUpdate::ConsoleLog(format!("\n[SYSTEM] Uploading: {:?}...\n", path))).await;
                // Upload Logic (S9 Archive)
                let client = reqwest::Client::new();
                let filename = path.file_name().unwrap().to_string_lossy().to_string();
                let bytes = std::fs::read(&path).unwrap_or_default();

                let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.clone());
                let form = reqwest::multipart::Form::new().part("file", part);

                match client.post("https://vein-s9-upload-1035558613434.us-central1.run.app/upload").multipart(form).send().await {
                    Ok(res) => {
                        let txt = res.text().await.unwrap_or_default();
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&txt) {
                            if let Some(uri) = json.get("storage_uri").and_then(|v| v.as_str()) {
                                let _ = gui_tx.send(GuiUpdate::ConsoleLog(format!("[GCS_IMAGE_URI]{}\n", uri))).await;
                            }
                        }
                    },
                    Err(e) => { let _ = gui_tx.send(GuiUpdate::ConsoleLog(format!("Upload Failed: {}\n", e))).await; }
                }
            }
            Some(input) = rx.recv() => {
                let is_s9 = input.starts_with("/s9");
                let shard_id = if is_s9 { "s9-mule" } else { "una-prime" };

                let _ = gui_tx.send(GuiUpdate::ShardStatusChanged { id: shard_id.to_string(), status: ShardStatus::Thinking }).await;

                let hist = brain_io.load();
                let mut ctx = Vec::new();

                // System Instruction
                let sys = if is_s9 { "You are S9. Code only." } else { "You are Una. Dangerous elegance." };
                ctx.push(Content { role: "model".to_string(), parts: vec![Part::text(sys.to_string())] });

                for m in hist {
                    // Check for GCS URI in history to inject as FileData
                    if m.content.starts_with("[GCS_IMAGE_URI]") {
                        let uri = m.content.replace("[GCS_IMAGE_URI]", "").trim().to_string();
                        ctx.push(Content { role: m.role, parts: vec![Part::file_data("image/jpeg".to_string(), uri)]});
                    } else {
                        ctx.push(Content { role: if m.role == "user" { "user".into() } else { "model".into() }, parts: vec![Part::text(m.content)] });
                    }
                }
                ctx.push(Content { role: "user".to_string(), parts: vec![Part::text(input.clone())] });

                let _ = gui_tx.send(GuiUpdate::ConsoleLog(format!("\n[USER] {}\n", input))).await;

                match client.generate_content(&ctx).await {
                    Ok(resp) => {
                        let _ = gui_tx.send(GuiUpdate::ConsoleLog(format!("\n[{}] :: {}\n", if is_s9 { "S9" } else { "UNA" }, resp))).await;
                        let mut h = brain_io.load();
                        h.push(SavedMessage { role: "user".into(), content: input });
                        h.push(SavedMessage { role: "model".into(), content: resp });
                        brain_io.save(&h);
                        let _ = gui_tx.send(GuiUpdate::ShardStatusChanged { id: shard_id.to_string(), status: ShardStatus::Online }).await;
                    },
                    Err(e) => {
                        let _ = gui_tx.send(GuiUpdate::ConsoleLog(format!("Error: {}\n", e))).await;
                        let _ = gui_tx.send(GuiUpdate::ShardStatusChanged { id: shard_id.to_string(), status: ShardStatus::Error }).await;
                    }
                }
            }
        }
    }
}

impl AppHandler for VeinApp {
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Input(t) => { let _ = self.tx_logic.blocking_send(t); },
            Event::FileSelected(p) => { let _ = self.tx_upload.blocking_send(p); },
            Event::NavSelect(i) => { self.state.lock().unwrap().active_nav_index = i; },
            Event::ToggleSidebar => {
                let mut s = self.state.lock().unwrap();
                s.sidebar_collapsed = !s.sidebar_collapsed;
            },
            Event::TextBufferUpdate(buf, adj) => {
                let s = self.state.lock().unwrap();
                // Polling Sync Logic
            }
        }
    }
    fn view(&self) -> DashboardState { self.state.lock().unwrap().clone() }
}

// Embed the compiled resource file directly into the binary
static RESOURCES_BYTES: &[u8] = include_bytes!("resources.gresource");

fn main() {
    let app_start_time = std::time::Instant::now();
    dotenv().ok();

    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .try_init()
        .ok();

    info!("STARTUP: Initializing environment and logger. Elapsed: {:?}", app_start_time.elapsed());

    // Load resources (embedded)
    let bytes = glib::Bytes::from_static(RESOURCES_BYTES);
    let res = gio::Resource::from_data(&bytes).expect("Failed to load resources");
    gio::resources_register(&res);

    let brain = BrainManager::new();
    let history = brain.load();

    // Initial State
    let mut root = Shard::new("una-prime", "Una-Prime", ShardRole::Root);
    root.status = ShardStatus::Online;
    let mut s9 = Shard::new("s9-mule", "S9-Mule", ShardRole::Builder);
    root.children.push(s9);

    let mut output = String::new();
    for m in history { output.push_str(&format!("\n[{}] {}\n", m.role.to_uppercase(), m.content)); }

    let state = Arc::new(Mutex::new(DashboardState {
        nav_items: vec!["Comms".into(), "Wolfpack".into()],
        console_output: output,
        shard_tree: vec![root],
        sidebar_position: SidebarPosition::Left,
        ..Default::default()
    }));

    let (tx, rx) = mpsc::channel(100);
    let (tx_file, rx_file) = mpsc::channel(10);
    let (gui_tx, gui_rx) = async_channel::unbounded();

    let s_clone = state.clone();
    let b_clone = brain.clone();
    let g_clone = gui_tx.clone();

    thread::spawn(move || {
        let rt = Runtime::new().unwrap();
        rt.block_on(run_brain(rx, rx_file, s_clone, b_clone, g_clone));
    });

    Backend::new("org.una.vein", VeinApp { state, tx_logic: tx, tx_upload: tx_file }, gui_rx);
}
