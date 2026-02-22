pub mod view;
pub mod model;
pub mod storage;
pub use view::CommsSpline;

use chrono::Local;
use elessar::gneiss_pal::api::{Content, Part, ResilientClient};
use elessar::gneiss_pal::forge::ForgeClient;
use elessar::gneiss_pal::persistence::{BrainManager, SavedMessage};
use elessar::gneiss_pal::{
    AppHandler, DashboardState, Event, GuiUpdate, Shard, ShardRole, ShardStatus, SidebarPosition,
    ViewMode, WolfpackState,
};
use log::info;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::{broadcast, mpsc};

use bandy::{BandyMember, SMessage};
use crate::storage::DiskManager;

struct State {
    mode: ViewMode,
    nav_index: usize,
    sidebar_position: SidebarPosition,
    sidebar_collapsed: bool,
    s9_status: ShardStatus,
}

// Upload Logic
fn trigger_upload(path: PathBuf, gui_tx: async_channel::Sender<GuiUpdate>) {
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let _ = gui_tx.send_blocking(GuiUpdate::ConsoleLog(format!(
        "\n[SYSTEM] :: Uploading: {}...\n",
        filename
    )));

    std::thread::spawn(move || {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let file_bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    let _ = gui_tx
                        .send(GuiUpdate::ConsoleLog(format!(
                            "\n[SYSTEM ERROR] :: File Read Failed: {}\n",
                            e
                        )))
                        .await;
                    return;
                }
            };

            let client = reqwest::Client::new();
            let url = "https://vein-s9-upload-1035558613434.us-central1.run.app/upload";

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
                                let mime = get_mime_type(&filename);
                                let tag = format!("\n[ATTACHMENT:{}|{}]\n", mime, uri);
                                let _ = gui_tx.send(GuiUpdate::AppendInput(tag)).await;
                                format!("\n[SYSTEM] :: {} Encased.\n", filename)
                            } else {
                                format!("\n[SYSTEM] :: Upload Complete (Raw): {}\n", text)
                            }
                        } else {
                            format!("\n[SYSTEM] :: Upload Complete (Raw): {}\n", text)
                        }
                    } else {
                        format!(
                            "\n[SYSTEM ERROR] :: Upload Failed: Status {}\n",
                            response.status()
                        )
                    }
                }
                Err(e) => format!("\n[SYSTEM ERROR] :: Network Error: {}\n", e),
            };

            let _ = gui_tx.send(GuiUpdate::ConsoleLog(final_msg)).await;
        });
    });
}

fn get_mime_type(filename: &str) -> &str {
    let lower = filename.to_lowercase();
    if lower.ends_with(".png") { "image/png" }
    else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") { "image/jpeg" }
    else if lower.ends_with(".pdf") { "application/pdf" }
    else if lower.ends_with(".mp4") { "video/mp4" }
    else if lower.ends_with(".mp3") || lower.ends_with(".wav") { "audio/mpeg" }
    else { "text/plain" }
}

fn parse_multimodal_text(content: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut current_text = content.to_string();

    while let Some(start) = current_text.find("[ATTACHMENT:") {
        if let Some(end) = current_text[start..].find("]") {
            let absolute_end = start + end;
            let tag = &current_text[start + 12..absolute_end];

            if let Some((mime, uri)) = tag.split_once('|') {
                if start > 0 {
                    parts.push(Part::text(current_text[..start].to_string()));
                }

                let clean_mime = mime.trim().to_string();
                let clean_uri = uri.trim().to_string();

                // === THE VERTEX SHIELD ===
                // Prevent the parser from eating its own source code.
                // Only build binary payloads for actual vision types.
                if clean_mime.starts_with("image/") || clean_mime.starts_with("video/") || clean_mime == "application/pdf" {
                    parts.push(Part::file_data(clean_mime, clean_uri));
                } else {
                    // It's just source code being pasted. Restore it safely as raw text.
                    parts.push(Part::text(format!("[ATTACHMENT:{}|{}]", clean_mime, clean_uri)));
                }
            }
            current_text = current_text[absolute_end + 1..].to_string();
        } else {
            break;
        }
    }
    if !current_text.trim().is_empty() {
        parts.push(Part::text(current_text));
    }
    if parts.is_empty() {
        parts.push(Part::text(" ".to_string()));
    }
    parts
}

pub struct VeinHandler {
    state: Arc<Mutex<State>>,
    tx: mpsc::UnboundedSender<String>,
    gui_tx: async_channel::Sender<GuiUpdate>,
    bandy_tx: broadcast::Sender<SMessage>,
}

impl VeinHandler {
    pub fn new(
        gui_tx: async_channel::Sender<GuiUpdate>,
        history_path: PathBuf,
        bandy_tx: broadcast::Sender<SMessage>,
    ) -> Self {
        // BrainManager kept ONLY for directive reading
        let brain = BrainManager::new(history_path);

        let state = Arc::new(Mutex::new(State {
            mode: ViewMode::Comms,
            nav_index: 0,
            sidebar_position: SidebarPosition::default(),
            sidebar_collapsed: false,
            s9_status: ShardStatus::Offline,
        }));

        let (tx_to_bg, mut rx_from_ui) = mpsc::unbounded_channel::<String>();

        let gui_tx_brain = gui_tx.clone();
        let state_bg = state.clone();
        let brain_bg = brain.clone();

        thread::spawn(move || {
            let rt = Runtime::new().expect("Failed to create Tokio Runtime");
            rt.block_on(async move {
                info!(":: VEIN :: Brain Connecting...");

                // === THE SEMANTIC VAULT ===
                // Initialize DiskManager (UnaFS)
                let mut disk = DiskManager::new().expect("Failed to initialize Semantic Vault (UnaFS)");

                // Load History for UI (Visual Only)
                if let Ok(records) = disk.load_all_memories() {
                    for record in records {
                        let prefix = if record.sender == "user" {
                            "[ARCHITECT]"
                        } else {
                            "[UNA]"
                        };
                        let msg = format!("{} [{}] > {}\n", prefix, record.timestamp, record.content);
                        let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(msg)).await;
                    }
                }

                tokio::time::sleep(Duration::from_millis(200)).await;

                let forge_client = match ForgeClient::new() {
                    Ok(client) => {
                        let _ = gui_tx_brain
                            .send(GuiUpdate::ConsoleLog(":: FORGE :: CONNECTED\n".into()))
                            .await;
                        Some(client)
                    }
                    Err(_) => None,
                };

                let client_res = ResilientClient::new().await;
                match client_res {
                    Ok(mut client) => {
                        let _ = gui_tx_brain
                            .send(GuiUpdate::ConsoleLog(":: BRAIN :: ONLINE (PLEXUS ENABLED)\n\n".into()))
                            .await;

                        // Broadcast Active Directive
                        let directive = brain_bg.get_active_directive();
                        let _ = gui_tx_brain.send(GuiUpdate::ActiveDirective(directive)).await;

                        while let Some(user_input_text) = rx_from_ui.recv().await {
                            let is_s9 = user_input_text.starts_with("/s9");

                            if user_input_text.starts_with("READ_REPO:") {
                                // ... (Forge logic kept as is) ...
                                let parts: Vec<&str> = user_input_text.split(':').collect();
                                if parts.len() >= 5 {
                                    let owner = parts[1];
                                    let repo = parts[2];
                                    let branch_raw = parts[3];
                                    let path = parts[4];
                                    let branch = if branch_raw.is_empty() {
                                        None
                                    } else {
                                        Some(branch_raw)
                                    };
                                    let response_msg = if let Some(fc) = &forge_client {
                                        match fc.get_file_content(owner, repo, path, branch).await {
                                            Ok(content) => format!(
                                                "\n[FORGE READ] :: {}/{}/{} ::\n{}\n",
                                                owner, repo, path, content
                                            ),
                                            Err(e) => format!("\n[FORGE ERROR] :: {}\n", e),
                                        }
                                    } else {
                                        "\n[FORGE] :: Offline.\n".to_string()
                                    };
                                    let _ = gui_tx_brain
                                        .send(GuiUpdate::ConsoleLog(response_msg))
                                        .await;
                                }
                                continue;
                            }

                            {
                                let mut s = state_bg.lock().unwrap();
                                // brain_bg.save(&s.chat_history); // REMOVED
                                if is_s9 {
                                    s.s9_status = ShardStatus::Thinking;
                                }
                            }

                            let _ = gui_tx_brain
                                .send(GuiUpdate::SidebarStatus(WolfpackState::Dreaming))
                                .await;

                            if is_s9 {
                                let _ = gui_tx_brain
                                    .send(GuiUpdate::ShardStatusChanged {
                                        id: "s9-mule".into(),
                                        status: ShardStatus::Thinking,
                                    })
                                    .await;
                            }

                            // === THE PLEXUS LOOP ===

                            // 1. Embed User Input
                            let user_embedding = match client.embed_content(&user_input_text).await {
                                Ok(vec) => vec,
                                Err(e) => {
                                    eprintln!(":: PLEXUS :: Embedding Failed: {}", e);
                                    vec![] // Continue without RAG if embedding fails
                                }
                            };

                            // 2. RAG Retrieval
                            let mut retrieved_context = String::new();
                            if !user_embedding.is_empty() {
                                match disk.search_memories(&user_embedding, 0.70) {
                                    Ok(memories) => {
                                        if !memories.is_empty() {
                                            retrieved_context = memories.join("\n\n");
                                            info!(":: PLEXUS :: Recalled {} memories.", memories.len());
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!(":: PLEXUS :: Recall Failed: {}", e);
                                    }
                                }
                            }

                            let system_base = if is_s9 { "You are S9." } else { "You are Una." };
                            let combined_system = if !retrieved_context.is_empty() {
                                format!("{}\n\n[SEMANTIC MEMORY RECALL]:\n{}", system_base, retrieved_context)
                            } else {
                                system_base.to_string()
                            };

                            let mut context = Vec::new();
                            context.push(Content {
                                role: "model".into(),
                                parts: vec![Part::text(combined_system)],
                            });

                            // Only push current user input. NO HISTORY LOOP.
                            context.push(Content {
                                role: "user".into(),
                                parts: parse_multimodal_text(&user_input_text),
                            });

                            match client.generate_content(&context).await {
                                Ok((response, metadata)) => {
                                    let timestamp = Local::now().format("%H:%M:%S").to_string();
                                    let display =
                                        format!("\n[UNA] [{}] :: {}\n", timestamp, response);
                                    let _ = gui_tx_brain
                                        .send(GuiUpdate::ConsoleLog(display.clone()))
                                        .await;

                                    if let Some(meta) = metadata {
                                        if let Some(total) = meta.total_token_count {
                                            let _ = gui_tx_brain.send(GuiUpdate::TokenUsage(total as u64)).await;
                                        }
                                    }

                                    {
                                        let mut s = state_bg.lock().unwrap();
                                        // s.chat_history.push(...) // REMOVED
                                        if is_s9 {
                                            s.s9_status = ShardStatus::Online;
                                        }
                                    }

                                    // 3. Save Memories (User + Model)
                                    // Embed Response
                                    let response_embedding = match client.embed_content(&response).await {
                                        Ok(vec) => vec,
                                        Err(_) => vec![],
                                    };

                                    if let Err(e) = disk.save_memory("user", &user_input_text, &timestamp, user_embedding) {
                                        eprintln!(":: PLEXUS :: Failed to save user memory: {}", e);
                                    }
                                    if let Err(e) = disk.save_memory("model", &response, &timestamp, response_embedding) {
                                        eprintln!(":: PLEXUS :: Failed to save model memory: {}", e);
                                    }

                                    let _ = gui_tx_brain
                                        .send(GuiUpdate::SidebarStatus(WolfpackState::Idle))
                                        .await;

                                    if is_s9 {
                                        let _ = gui_tx_brain
                                            .send(GuiUpdate::ShardStatusChanged {
                                                id: "s9-mule".into(),
                                                status: ShardStatus::Online,
                                            })
                                            .await;
                                    }
                                }
                                Err(e) => {
                                    let _ = gui_tx_brain
                                        .send(GuiUpdate::ConsoleLog(format!("\n[ERROR] {}\n", e)))
                                        .await;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = gui_tx_brain
                            .send(GuiUpdate::ConsoleLog(format!(":: FATAL :: {}\n", e)))
                            .await;
                    }
                }
            });
        });

        // REMOVED: Old History Restore Loop (Now handled inside thread)

        // Spawn Bandy Listener
        let mut bandy_rx = bandy_tx.subscribe();
        let gui_tx_bandy = gui_tx.clone();

        thread::spawn(move || {
            let rt = Runtime::new().expect("Failed to create Bandy Runtime");
            rt.block_on(async move {
                while let Ok(msg) = bandy_rx.recv().await {
                    match msg {
                        SMessage::FileEvent { path, event } => {
                            let _ = gui_tx_bandy
                                .send(GuiUpdate::ConsoleLog(format!(
                                    "\n[BANDY] File {}: {}\n",
                                    event, path
                                )))
                                .await;
                        }
                        SMessage::Log {
                            level,
                            source,
                            content,
                        } => {
                            let _ = gui_tx_bandy
                                .send(GuiUpdate::ConsoleLog(format!(
                                    "\n[LOG:{}] {}: {}\n",
                                    level, source, content
                                )))
                                .await;
                        }
                        SMessage::Spectrum { magnitude } => {
                            let _ = gui_tx_bandy.send(GuiUpdate::Spectrum(magnitude)).await;
                        }
                        _ => {}
                    }
                }
            });
        });

        Self {
            state,
            tx: tx_to_bg,
            gui_tx,
            bandy_tx,
        }
    }

    fn append_to_console(&self, text: &str) {
        let _ = self
            .gui_tx
            .send_blocking(GuiUpdate::ConsoleLog(text.to_string()));
    }
}

impl BandyMember for VeinHandler {
    fn publish(&self, _topic: &str, msg: SMessage) -> anyhow::Result<()> {
        self.bandy_tx
            .send(msg)
            .map_err(|e| anyhow::anyhow!("Bandy Send Error: {}", e))?;
        Ok(())
    }
}

impl AppHandler for VeinHandler {
    fn handle_event(&mut self, event: Event) {
        let mut s = self.state.lock().unwrap();

        match event {
            Event::Input { target: _, text } => {
                let timestamp = Local::now().format("%H:%M:%S").to_string();
                let current_text = format!("\n[ARCHITECT] [{}] > {}\n", timestamp, text);
                // s.chat_history.push(...); // REMOVED
                self.append_to_console(&current_text);

                if text.trim() == "/wolf" {
                    s.mode = ViewMode::Wolfpack;
                    self.append_to_console("\n[SYSTEM] :: Switching to Wolfpack Grid...\n");
                } else if text.trim() == "/comms" {
                    s.mode = ViewMode::Comms;
                    self.append_to_console("\n[SYSTEM] :: Secure Comms Established.\n");
                } else if text.trim() == "/clear" {
                    // s.chat_history.clear(); // REMOVED
                    self.append_to_console("\n:: VEIN :: SYSTEM CLEARED\n\n");
                } else if let Some(path_str) = text.trim().strip_prefix("/upload ") {
                    let path = PathBuf::from(path_str.trim());
                    trigger_upload(path, self.gui_tx.clone());
                } else {
                    if let Err(e) = self.tx.send(text) {
                        self.append_to_console(&format!(
                            "\n[SYSTEM ERROR] :: Failed to send: {}\n",
                            e
                        ));
                    }
                }
            }
            Event::TemplateAction(idx) => match idx {
                0 => {
                    if s.mode == ViewMode::Comms {
                        self.append_to_console(":: VEIN :: SYSTEM CLEARED\n\n");
                    } else {
                        self.append_to_console("\n[WOLFPACK] :: Deploying J-Series Unit...\n");
                    }
                }
                1 => {
                    if s.mode == ViewMode::Comms {
                        s.mode = ViewMode::Wolfpack;
                        self.append_to_console("\n[SYSTEM] :: Switching to Wolfpack Grid...\n");
                    } else {
                        self.append_to_console("\n[WOLFPACK] :: Deploying S-Series Unit...\n");
                    }
                }
                2 => {
                    if s.mode == ViewMode::Wolfpack {
                        s.mode = ViewMode::Comms;
                        self.append_to_console("\n[SYSTEM] :: Returning to Comms.\n");
                    }
                }
                _ => {}
            },
            Event::NavSelect(idx) => {
                s.nav_index = idx;
                self.append_to_console(&format!(
                    "\n[SYSTEM] :: Switched to navigation item at index {}\n",
                    idx
                ));
            }
            Event::FileSelected(path) => {
                trigger_upload(path, self.gui_tx.clone());
            }
            Event::ToggleSidebar => {
                s.sidebar_collapsed = !s.sidebar_collapsed;
            }
            Event::ComplexInput { target: _, subject, body, point_break, action: _ } => {
                let prefix = if point_break { "Point Break: " } else { "" };
                let full_message = format!("\nSubject: {}{}\n\n{}", prefix, subject, body);

                let timestamp = Local::now().format("%H:%M:%S").to_string();
                let current_text = format!("\n[ARCHITECT] [{}] > {}\n", timestamp, full_message);
                // s.chat_history.push(...); // REMOVED
                self.append_to_console(&current_text);

                if let Err(e) = self.tx.send(full_message) {
                    self.append_to_console(&format!("\n[SYSTEM ERROR] :: Failed to send: {}\n", e));
                }
            }
            _ => {}
        }
    }

    fn view(&self) -> DashboardState {
        let s = self.state.lock().unwrap();
        DashboardState {
            mode: s.mode.clone(),
            nav_items: vec![
                "General".into(),
                "Encrypted".into(),
                "Jules (Private)".into(),
            ],
            active_nav_index: s.nav_index,
            console_output: String::new(),
            actions: match s.mode {
                ViewMode::Comms => vec!["Clear Buffer".into(), "Wolfpack View".into()],
                ViewMode::Wolfpack => vec![
                    "Deploy J1".into(),
                    "Deploy S5".into(),
                    "Back to Comms".into(),
                ],
            },
            sidebar_position: s.sidebar_position.clone(),
            dock_actions: vec!["Rooms".into(), "Status".into()],
            shard_tree: {
                let mut root = Shard::new("una-prime", "Una-Prime", ShardRole::Root);
                root.status = ShardStatus::Online;

                let mut child = Shard::new("s9-mule", "S9-Mule", ShardRole::Builder);
                child.status = s.s9_status.clone();

                root.children.push(child);
                vec![root]
            },
            sidebar_collapsed: s.sidebar_collapsed,
        }
    }
}
