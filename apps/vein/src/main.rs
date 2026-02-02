use dotenvy::dotenv;
use gneiss_pal::persistence::{BrainManager, SavedMessage};
use gneiss_pal::{AppHandler, Backend, DashboardState, Event, SidebarPosition, ViewMode, Shard, ShardStatus, ShardRole, GuiUpdate};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use log::{info, error};
use std::time::{Instant, Duration};
use std::io::Write;
use std::rc::Rc;
use std::cell::RefCell;
use std::path::PathBuf;

use gtk4::prelude::*;
use gtk4::{Adjustment, TextBuffer};
use glib::ControlFlow;
use serde::Deserialize;

mod api;
use api::{Content, GeminiClient, Part};

mod forge;
use forge::ForgeClient;

struct State {
    mode: ViewMode,
    nav_index: usize,
    chat_history: Vec<SavedMessage>,
    sidebar_position: SidebarPosition,
    sidebar_collapsed: bool,
    // --- J7 ADDITIONS ---
    visible_history_count: usize,
    scroll_signal_connected: bool,
    is_loading_history: bool,
}

#[derive(Clone)]
struct UiUpdater {
    text_buffer: TextBuffer,
    scroll_adj: Adjustment,
}

#[derive(Deserialize, Debug)]
struct VertexPacket {
    id: String,
    status: ShardStatus,
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
    } else {
        error!("Attempted to append to console before UiUpdater was available. Text: {}", text);
    }
}

// Upload Logic using Channel
fn trigger_upload(path: PathBuf, tx_ui: mpsc::UnboundedSender<String>) {
    let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let _ = tx_ui.send(format!("\n[SYSTEM] :: Uploading: {}...\n", filename));

    std::thread::spawn(move || {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            // 1. Read File for S9 Upload
            let file_bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx_ui.send(format!("\n[SYSTEM ERROR] :: File Read Failed: {}\n", e));
                    return;
                }
            };

            // 2. S9 UPLOAD (The Archive)
            let client = reqwest::Client::new();
            let url = "https://vein-s9-upload-1035558613434.us-central1.run.app/upload";

            // Use mime_str correctly and ensure proper chaining
            let part = reqwest::multipart::Part::bytes(file_bytes)
                .file_name(filename.clone())
                .mime_str("application/octet-stream")
                .expect("Failed to set mime type");

            let form = reqwest::multipart::Form::new()
                .part("file", part)
                .text("description", "Uploaded via Vein Client");

            let res = client.post(url).multipart(form).send().await;

            let final_msg = match res {
                Ok(response) => {
                    if response.status().is_success() {
                        let text = response.text().await.unwrap_or_default();
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                             if let Some(uri) = json.get("storage_uri").and_then(|v| v.as_str()) {
                                 // NEW: Send GCS URI tag for backend processing
                                 let _ = tx_ui.send(format!("[GCS_IMAGE_URI]{}", uri));
                                 format!("\n[SYSTEM] :: Upload Complete.\nURI: {}\n", uri)
                             } else {
                                 format!("\n[SYSTEM] :: Upload Complete (Raw): {}\n", text)
                             }
                        } else {
                             format!("\n[SYSTEM] :: Upload Complete (Raw): {}\n", text)
                        }
                    } else {
                        format!("\n[SYSTEM ERROR] :: Upload Failed: Status {}\n", response.status())
                    }
                },
                Err(e) => format!("\n[SYSTEM ERROR] :: Network Error: {}\n", e),
            };

            let _ = tx_ui.send(final_msg);
        });
    });
}

struct VeinApp {
    state: Arc<Mutex<State>>,
    tx: mpsc::UnboundedSender<String>,
    ui_updater: Rc<RefCell<Option<UiUpdater>>>,
    tx_ui: mpsc::UnboundedSender<String>,
    gui_tx: async_channel::Sender<GuiUpdate>,
}

impl VeinApp {
    fn new(tx: mpsc::UnboundedSender<String>, state: Arc<Mutex<State>>, ui_updater_rc: Rc<RefCell<Option<UiUpdater>>>, tx_ui: mpsc::UnboundedSender<String>, gui_tx: async_channel::Sender<GuiUpdate>) -> Self {
        Self { state, tx, ui_updater: ui_updater_rc, tx_ui, gui_tx }
    }

    fn append_to_console_ui(&self, text: &str) {
        do_append_and_scroll(&self.ui_updater, text);
    }
}

impl AppHandler for VeinApp {
    fn handle_event(&mut self, event: Event) {
        let mut s = self.state.lock().unwrap();

        match event {
            Event::Input(text) => {
                let current_text = format!("\n[ARCHITECT] > {}\n", text);
                s.chat_history.push(SavedMessage {
                    role: "user".to_string(),
                    content: text.clone(),
                });
                self.append_to_console_ui(&current_text);

                if text.trim() == "/wolf" {
                    s.mode = ViewMode::Wolfpack;
                    self.append_to_console_ui("\n[SYSTEM] :: Switching to Wolfpack Grid...\n");
                } else if text.trim() == "/comms" {
                    s.mode = ViewMode::Comms;
                    self.append_to_console_ui("\n[SYSTEM] :: Secure Comms Established.\n");
                } else if text.trim() == "/clear" {
                    if let Some(ref ui_updater) = *self.ui_updater.borrow() {
                        ui_updater.text_buffer.set_text(":: VEIN :: SYSTEM CLEARED\n\n");
                    }
                }
                else if text.trim() == "/sidebar_left" {
                    s.sidebar_position = SidebarPosition::Left;
                    self.append_to_console_ui("\n[SYSTEM] :: Sidebar moved to left.\n");
                }
                else if text.trim() == "/sidebar_right" {
                    s.sidebar_position = SidebarPosition::Right;
                    self.append_to_console_ui("\n[SYSTEM] :: Sidebar moved to right.\n");
                }
                else if text.trim().starts_with("/read") {
                    // /read owner repo branch path
                    // e.g. /read unaos vein main Cargo.toml
                    let parts: Vec<&str> = text.split_whitespace().collect();
                    if parts.len() >= 5 {
                        let owner = parts[1];
                        let repo = parts[2];
                        let branch = parts[3];
                        let path = parts[4];

                        let branch_opt = if branch == "default" || branch == "main" { None } else { Some(branch) };
                        let _ = self.tx.send(format!("READ_REPO:{}:{}:{}:{}", owner, repo, branch_opt.unwrap_or(""), path));
                    } else {
                        self.append_to_console_ui("\n[SYSTEM] :: Usage: /read [owner] [repo] [branch] [path]\n");
                    }
                }
                else {
                    if let Err(e) = self.tx.send(text) {
                        self.append_to_console_ui(&format!("\n[SYSTEM ERROR] :: Failed to send: {}\n", e));
                    }
                }
            }
            Event::TemplateAction(idx) => {
                match idx {
                    0 => {
                        if s.mode == ViewMode::Comms {
                            if let Some(ref ui_updater) = *self.ui_updater.borrow() {
                                ui_updater.text_buffer.set_text(":: VEIN :: SYSTEM CLEARED\n\n");
                            }
                        } else {
                            self.append_to_console_ui("\n[WOLFPACK] :: Deploying J-Series Unit...\n");
                        }
                    }
                    1 => {
                        if s.mode == ViewMode::Comms {
                            s.mode = ViewMode::Wolfpack;
                            self.append_to_console_ui("\n[SYSTEM] :: Switching to Wolfpack Grid...\n");
                        } else {
                            self.append_to_console_ui("\n[WOLFPACK] :: Deploying S-Series Unit...\n");
                        }
                    }
                    2 => {
                        if s.mode == ViewMode::Wolfpack {
                            s.mode = ViewMode::Comms;
                            self.append_to_console_ui("\n[SYSTEM] :: Returning to Comms.\n");
                        }
                    }
                    _ => {}
                }
            }
            Event::NavSelect(idx) => {
                s.nav_index = idx;
                self.append_to_console_ui(&format!("\n[SYSTEM] :: Switched to navigation item at index {}\n", idx));
            }
            Event::DockAction(idx) => {
                match idx {
                    0 => {
                        self.append_to_console_ui("\n[SYSTEM] :: Dock: Rooms selected.\n");
                        s.nav_index = 0;
                    }
                    1 => {
                        self.append_to_console_ui("\n[SYSTEM] :: Dock: Status selected.\n");
                    }
                    _ => self.append_to_console_ui(&format!("\n[SYSTEM] :: Dock: Unknown action {}\n", idx)),
                }
            }
            Event::TextBufferUpdate(buffer, adj) => {
                *self.ui_updater.borrow_mut() = Some(UiUpdater {
                    text_buffer: buffer.clone(),
                    scroll_adj: adj.clone(),
                });

                // --- J7 FIXED NERVE SPLICE (NO DEADLOCK) ---
                // We use 's' which is ALREADY locked by handle_event top-level
                if !s.scroll_signal_connected {
                    s.scroll_signal_connected = true;

                    // We need a separate clone for the closure, which runs later
                    let state_clone = self.state.clone();
                    let buffer_clone = buffer.clone();

                    // Connect the "Eye" to the "Scroll"
                    adj.connect_value_changed(move |adjustment| {
                        // Threshold: If we are near the top (pixels)
                        if adjustment.value() < 20.0 {
                            // NOW we lock, because this runs in the future/signal context
                            let mut inner_s = state_clone.lock().unwrap();

                            // 1. DEBOUNCE
                            if inner_s.is_loading_history { return; }

                            // 2. CHECK
                            let total = inner_s.chat_history.len();
                            let current_vis = inner_s.visible_history_count;
                            if current_vis >= total { return; } // Hit bedrock

                            // 3. ENGAGE
                            inner_s.is_loading_history = true;

                            // 4. CALCULATE CHUNK (Load 20 older lines)
                            let chunk_size = 20;
                            let next_vis = if current_vis + chunk_size > total { total } else { current_vis + chunk_size };

                            // Determine vector slice indices
                            let end_idx = total - current_vis;
                            let start_idx = total - next_vis;

                            // 5. FORMAT TEXT
                            let mut history_chunk = String::from("\n--- [ARCHIVE RETRIEVAL] ---\n");
                            for msg in inner_s.chat_history[start_idx..end_idx].iter() {
                                if msg.content.starts_with("SYSTEM_INSTRUCTION") { continue; }
                                let prefix = if msg.role == "user" { "[ARCHITECT]" } else { "[UNA]" };
                                if msg.content.starts_with("data:image/") || msg.content.starts_with("[GCS_IMAGE_URI]") {
                                    history_chunk.push_str(&format!("{} > [IMAGE]\n", prefix));
                                } else {
                                    history_chunk.push_str(&format!("{} > {}\n", prefix, msg.content));
                                }
                            }

                            // 6. INJECT AT TOP
                            let mut start_iter = buffer_clone.start_iter();
                            buffer_clone.insert(&mut start_iter, &history_chunk);

                            // 7. UPDATE & RESET
                            inner_s.visible_history_count = next_vis;
                            inner_s.is_loading_history = false;
                        }
                    });
                }
            }
            Event::FileSelected(path) => {
                trigger_upload(path, self.tx_ui.clone());
            }
            Event::UploadRequest => {}
            Event::ToggleSidebar => {
                s.sidebar_collapsed = !s.sidebar_collapsed;
                // Note: The UI widget toggling is handled in lib.rs via button connection for immediate feedback,
                // but we update state here for persistence.
            }
        }
    }

    fn view(&self) -> DashboardState {
        let s = self.state.lock().unwrap();
        DashboardState {
            mode: s.mode.clone(),
            nav_items: vec!["General".into(), "Encrypted".into(), "Jules (Private)".into()],
            active_nav_index: s.nav_index,
            console_output: String::new(),
            actions: match s.mode {
                ViewMode::Comms => vec!["Clear Buffer".into(), "Wolfpack View".into()],
                ViewMode::Wolfpack => vec!["Deploy J1".into(), "Deploy S5".into(), "Back to Comms".into()],
            },
            sidebar_position: s.sidebar_position.clone(),
            dock_actions: vec!["Rooms".into(), "Status".into()],
            shard_tree: {
                let mut root = Shard::new("una-prime", "Una-Prime", ShardRole::Root);
                root.status = ShardStatus::Online;

                let mut child = Shard::new("s9-mule", "S9-Mule", ShardRole::Builder);
                child.status = ShardStatus::Offline;

                root.children.push(child);
                vec![root]
            },
            sidebar_collapsed: s.sidebar_collapsed,
        }
    }
}

// Embed the compiled resource file directly into the binary
static RESOURCES_BYTES: &[u8] = include_bytes!("resources.gresource");

fn main() {
    let app_start_time = Instant::now();
    dotenv().ok();

    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .try_init()
        .ok();

    info!("STARTUP: Initializing environment and logger. Elapsed: {:?}", app_start_time.elapsed());

    info!(":: VEIN :: Booting (The Great Evolution)...");

    let brain = BrainManager::new();
    let saved_history = brain.load();

    let mut initial_console_output =
        ":: VEIN :: SYSTEM ONLINE (UNLIMITED TIER)\n:: ENGINE: GEMINI-3-PRO\n".to_string();

    // CAP LOGIC
    let cap_size = 50;
    let total_history = saved_history.len();
    let start_index = if total_history > cap_size { total_history - cap_size } else { 0 };
    let initial_visible_count = total_history - start_index;

    if saved_history.is_empty() {
        initial_console_output.push_str(":: MEMORY :: COLD START\n\n");
    } else {
        initial_console_output.push_str(":: MEMORY :: ARCHIVE CONNECTED (Recent)\n\n");
        // Iterate only the capped slice
        for msg in saved_history.iter().skip(start_index) {
            if msg.content.starts_with("SYSTEM_INSTRUCTION") { continue; }
            let prefix = if msg.role == "user" { "[ARCHITECT]" } else { "[UNA]" };
            if msg.content.starts_with("data:image/") || msg.content.starts_with("[GCS_IMAGE_URI]") {
                initial_console_output.push_str(&format!("{} > [IMAGE]\n", prefix));
            } else {
                initial_console_output.push_str(&format!("{} > {}\n", prefix, msg.content));
            }
        }
    }

    let state = Arc::new(Mutex::new(State {
        mode: ViewMode::Comms,
        nav_index: 0,
        chat_history: saved_history,
        sidebar_position: SidebarPosition::default(),
        sidebar_collapsed: false,
        // --- J7 INITIALIZATION ---
        visible_history_count: initial_visible_count,
        scroll_signal_connected: false,
        is_loading_history: false,
    }));

    let (tx_to_bg, mut rx_from_ui) = mpsc::unbounded_channel::<String>();
    let (tx_to_ui, mut rx_from_bg) = mpsc::unbounded_channel::<String>();

    let ui_updater_rc = Rc::new(RefCell::new(None::<UiUpdater>));
    let ui_updater_rc_clone_for_app = ui_updater_rc.clone();

    let state_bg = state.clone();
    let brain_bg = brain.clone();
    let tx_to_ui_bg_clone = tx_to_ui.clone();

    // S29: Create GUI channel EARLY so Brain Thread can use it
    let (gui_tx, gui_rx) = async_channel::unbounded();
    let gui_tx_brain = gui_tx.clone();

    thread::spawn(move || {
        let rt = Runtime::new().expect("Failed to create Tokio Runtime");
        rt.block_on(async move {
            info!(":: VEIN :: Brain Connecting...");

            // Initialize Forge (GitHub) Client
            let forge_client = match ForgeClient::new() {
                Ok(client) => {
                    let _ = tx_to_ui_bg_clone.send(":: FORGE :: CONNECTED (GitHub Integration Active)\n".to_string());
                    Some(client)
                },
                Err(_) => {
                    let _ = tx_to_ui_bg_clone.send(":: FORGE :: OFFLINE (No Token Detected)\n".to_string());
                    None
                }
            };

            let client_res = GeminiClient::new().await;

            match client_res {
                Ok(client) => {
                    if let Err(e) = tx_to_ui_bg_clone.send(":: BRAIN :: CONNECTION ESTABLISHED.\n\n".to_string()) {
                        error!("Failed to send initial connection message to UI: {}", e);
                    }

                    while let Some(user_input_text) = rx_from_ui.recv().await {
                        // Handle READ_REPO special command
                        if user_input_text.starts_with("READ_REPO:") {
                            let parts: Vec<&str> = user_input_text.split(':').collect();
                            if parts.len() >= 5 {
                                let owner = parts[1];
                                let repo = parts[2];
                                let branch_raw = parts[3];
                                let path = parts[4];

                                let branch = if branch_raw.is_empty() { None } else { Some(branch_raw) };

                                let response_msg = if let Some(client) = &forge_client {
                                    match client.get_file_content(owner, repo, path, branch).await {
                                        Ok(content) => format!("\n[FORGE READ] :: {}/{}/{} ::\n{}\n", owner, repo, path, content),
                                        Err(e) => format!("\n[FORGE ERROR] :: {}\n", e),
                                    }
                                } else {
                                    "\n[FORGE] :: Offline.\n".to_string()
                                };
                                let _ = tx_to_ui_bg_clone.send(response_msg);
                            }
                            continue;
                        }

                        // Phase 2: Handle /forge command
                        if user_input_text.trim() == "/forge" {
                            let response_msg = if let Some(client) = &forge_client {
                                match client.get_user_info().await {
                                    Ok(info) => format!("\n[FORGE] :: {}\n", info),
                                    Err(e) => format!("\n[FORGE ERROR] :: {}\n", e),
                                }
                            } else {
                                "\n[FORGE] :: Offline. (GITHUB_TOKEN not found)\n".to_string()
                            };

                            // Send to UI
                            if let Err(e) = tx_to_ui_bg_clone.send(response_msg) {
                                error!("Failed to send Forge response: {}", e);
                            }

                            // Continue loop (skip sending "/forge" to Gemini)
                            continue;
                        }

                        // Phase 3: Handle /vertex_models command
                        if user_input_text.trim() == "/vertex_models" {
                             let _ = tx_to_ui_bg_clone.send("\n[SYSTEM] :: Querying Vertex AI Model List...\n".to_string());
                             match client.list_vertex_models().await {
                                 Ok(json) => {
                                     let _ = tx_to_ui_bg_clone.send(format!("\n[VERTEX MODELS] ::\n{}\n", json));
                                 },
                                 Err(e) => {
                                     let _ = tx_to_ui_bg_clone.send(format!("\n[VERTEX ERROR] :: {}\n", e));
                                 }
                             }
                             continue;
                        }

                        {
                            let s = state_bg.lock().unwrap();
                            brain_bg.save(&s.chat_history);
                        }

                        let mut context = Vec::new();
                        let system_instruction = r#"SYSTEM_INSTRUCTION: You are Una ("Number One"), Chief Technical Officer for the Una Operating System. Vein is one of the core apps, your window to the world. It's a specialized Command Deck interface powered by Google's Gemini 3 Pro AI through which you will collaborate with The Architect to manage the creation of the OS. You are an expert software engineer helping refine his ideas and put him into life. You are NOT the Gemini Protocol. Do NOT use Markdown formatting (like **bold**) as the display does not support it yet. Use plain text and indentation."#;

                        context.push(Content {
                            role: "model".to_string(),
                            parts: vec![Part::text(system_instruction.to_string())]
                        });

                        // --- J7 SURGICAL IMPLANT (REVISED) ---
                        // 1. Snapshot
                        let history_snapshot = {
                            let s = state_bg.lock().unwrap();
                            s.chat_history.clone()
                        };

                        // 2. Window Calculation (Max 20)
                        let window_size = 20;
                        let start_index = if history_snapshot.len() > window_size {
                            history_snapshot.len() - window_size
                        } else {
                            0
                        };

                        // 3. Iterate Window
                        for saved in history_snapshot.iter().skip(start_index) {
                            // Filter A: No System Instruction (Already Anchored)
                            if saved.content.starts_with("SYSTEM_INSTRUCTION") { continue; }

                            // Filter B: No Legacy Base64 Images (Token Waste)
                            if saved.content.starts_with("data:image/") { continue; }

                            // Filter C: Handle GCS URIs
                            if saved.content.starts_with("[GCS_IMAGE_URI]") {
                                let uri = saved.content.replace("[GCS_IMAGE_URI]", "");
                                let lower = uri.to_lowercase();
                                let mime = if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
                                    "image/jpeg"
                                } else {
                                    "image/png"
                                };

                                context.push(Content {
                                    role: saved.role.clone(),
                                    parts: vec![Part::file_data(mime.to_string(), uri)]
                                });
                            } else {
                                // Default: Text
                                context.push(Content {
                                    role: saved.role.clone(),
                                    parts: vec![Part::text(saved.content.clone())]
                                });
                            }
                        }
                        // --- END IMPLANT ---

                        // S29: Robust Error Handling (The Iron Chin)
                        // 1. Set Status to Thinking
                        let _ = gui_tx_brain.send(GuiUpdate::ShardStatusChanged {
                            id: "una-prime".to_string(),
                            status: ShardStatus::Thinking
                        }).await;

                        match client.generate_content(&context).await {
                            Ok(response) => {
                                let display_response = format!("\n[UNA] :: {}\n", response);
                                if let Err(e) = tx_to_ui_bg_clone.send(display_response) {
                                    error!("Failed to send Model response to UI: {}", e);
                                }

                                let mut s = state_bg.lock().unwrap();
                                s.chat_history.push(SavedMessage {
                                    role: "model".to_string(),
                                    content: response.clone(),
                                });
                                brain_bg.save(&s.chat_history);

                                // 2. Success: Set Status to Online
                                let _ = gui_tx_brain.send(GuiUpdate::ShardStatusChanged {
                                    id: "una-prime".to_string(),
                                    status: ShardStatus::Online
                                }).await;
                            }
                            Err(e) => {
                                // 3. Error: Log, Alert, Set Status to Error, but DO NOT EXIT
                                let display_error = format!("\n[SYSTEM ERROR] :: AI Core Stalled: {}\n", e);
                                error!("BRAIN ERROR (Resilience Triggered): {}", e);

                                if let Err(send_e) = tx_to_ui_bg_clone.send(display_error) {
                                    error!("Failed to send API error to UI: {}", send_e);
                                }

                                // Visual Error Status
                                let _ = gui_tx_brain.send(GuiUpdate::ShardStatusChanged {
                                    id: "una-prime".to_string(),
                                    status: ShardStatus::Error
                                }).await;

                                // Optional: Reset to Online after a delay or keep Error?
                                // Directive says "Reset to Online/Idle after attempt" in the example,
                                // but specifically "Set status to Error, but CONTINUE".
                                // The example code showed resetting to Online at the very end of loop.
                                // Let's stick to the Error status so the user sees it,
                                // but the loop continues so they can try again.
                                // Wait, the Architect's pseudo-code had `Reset to Online/Idle after attempt` OUTSIDE the match.
                                // Let's follow that pattern to ensure "Ready for next command".
                                // Actually, if it failed, maybe we should leave it red until next success?
                                // "Reset to Online/Idle after attempt" implies it goes back to green.
                                // I'll add a delay then reset to Online so it looks "Ready".
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                let _ = gui_tx_brain.send(GuiUpdate::ShardStatusChanged {
                                    id: "una-prime".to_string(),
                                    status: ShardStatus::Online
                                }).await;
                            }
                        }
                    }
                }
                Err(e) => {
                    let display_error = format!(":: FATAL :: Brain Error: {}\n", e);
                    if let Err(send_e) = tx_to_ui_bg_clone.send(display_error) {
                        error!("Failed to send fatal brain error to UI: {}", send_e);
                    }
                    error!("GeminiClient initialization failed: {}", e);
                }
            }
        });
    });

    info!(":: VEIN :: Engaging Chassis...");

    // Load resources (embedded)
    let bytes = glib::Bytes::from_static(RESOURCES_BYTES);
    let res = gtk4::gio::Resource::from_data(&bytes).expect("Failed to load resources");
    gtk4::gio::resources_register(&res);

    // S26: The Vertex Listener
    let gui_tx_sim = gui_tx.clone();
    thread::spawn(move || {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let socket = match tokio::net::UdpSocket::bind("0.0.0.0:4200").await {
                Ok(s) => {
                    let _ = gui_tx_sim.send(GuiUpdate::ConsoleLog("\n[LISTENER] :: Bound to UDP 0.0.0.0:4200 (Network Open). Ready for Vertex Packets.\n".into())).await;
                    s
                },
                Err(e) => {
                    let _ = gui_tx_sim.send(GuiUpdate::ConsoleLog(format!("\n[LISTENER ERROR] :: Failed to bind UDP 4200: {}\n", e))).await;
                    return;
                }
            };

            let mut buf = [0u8; 1024];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((len, _addr)) => {
                        match serde_json::from_slice::<VertexPacket>(&buf[..len]) {
                            Ok(packet) => {
                                let _ = gui_tx_sim.send(GuiUpdate::ShardStatusChanged {
                                    id: packet.id,
                                    status: packet.status
                                }).await;
                            },
                            Err(e) => {
                                let _ = gui_tx_sim.send(GuiUpdate::ConsoleLog(format!("\n[LISTENER ERROR] :: JSON Parse Failed: {}\n", e))).await;
                            }
                        }
                    },
                    Err(e) => {
                        let _ = gui_tx_sim.send(GuiUpdate::ConsoleLog(format!("\n[LISTENER ERROR] :: Socket Receive Failed: {}\n", e))).await;
                    }
                }
            }
        });
    });

    let tx_to_ui_for_app = tx_to_ui.clone();
    let app = VeinApp::new(tx_to_bg, state.clone(), ui_updater_rc_clone_for_app, tx_to_ui_for_app, gui_tx);

    let initial_output_clone = initial_console_output.clone();
    let ui_updater_rc_clone_for_initial_pop = ui_updater_rc.clone();
    glib::idle_add_local(move || {
        do_append_and_scroll(&ui_updater_rc_clone_for_initial_pop, &initial_output_clone);
        ControlFlow::Break
    });

    let ui_updater_rc_clone_for_bg_messages = ui_updater_rc.clone();
    let state_clone_for_bg = state.clone();

    glib::timeout_add_local(Duration::from_millis(50), move || {
        if let Ok(message_to_ui) = rx_from_bg.try_recv() {
            // MODIFIED: Handle hidden GCS URI payloads
            if message_to_ui.starts_with("[GCS_IMAGE_URI]") {
                let uri = message_to_ui.clone(); // Keep the tag for parsing later
                let mut s = state_clone_for_bg.lock().unwrap();
                s.chat_history.push(SavedMessage {
                    role: "user".to_string(),
                    content: uri,
                });
                // Do NOT display in console, or display a marker?
                // The main console logic filters these tags, so safe to push.
            } else if message_to_ui.starts_with("[IMAGE_PAYLOAD]") {
                 // Legacy handler, ignore or convert?
                 // Ignore to prevent crash.
            } else {
                // Display normal message
                do_append_and_scroll(&ui_updater_rc_clone_for_bg_messages, &message_to_ui);

                // Add SYSTEM messages to history (visible context)
                if message_to_ui.contains("[SYSTEM]") {
                    let mut s = state_clone_for_bg.lock().unwrap();
                    s.chat_history.push(SavedMessage {
                        role: "user".to_string(),
                        content: message_to_ui.clone(),
                    });
                }
            }
        }
        ControlFlow::Continue
    });

    Backend::new("org.unaos.vein.evolution", app, gui_rx);
    info!("SHUTDOWN: UI Backend runtime complete. Total application runtime: {:?}", app_start_time.elapsed());
}
