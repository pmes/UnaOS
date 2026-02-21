pub mod view;
pub mod model;
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

struct State {
    mode: ViewMode,
    nav_index: usize,
    chat_history: Vec<SavedMessage>,
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
                                format!("\n[SYSTEM] :: Upload Complete.\nURI: {}\n", uri)
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
        let brain = BrainManager::new(history_path);
        let saved_history = brain.load();

        let state = Arc::new(Mutex::new(State {
            mode: ViewMode::Comms,
            nav_index: 0,
            chat_history: saved_history.clone(),
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
                            .send(GuiUpdate::ConsoleLog(":: BRAIN :: ONLINE\n\n".into()))
                            .await;

                        // Broadcast Active Directive
                        let directive = brain_bg.get_active_directive();
                        let _ = gui_tx_brain.send(GuiUpdate::ActiveDirective(directive)).await;

                        while let Some(user_input_text) = rx_from_ui.recv().await {
                            let is_s9 = user_input_text.starts_with("/s9");

                            if user_input_text.starts_with("READ_REPO:") {
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
                                brain_bg.save(&s.chat_history);
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

                            let system_instruction =
                                if is_s9 { "You are S9." } else { "You are Una." };
                            let mut context = Vec::new();
                            context.push(Content {
                                role: "model".into(),
                                parts: vec![Part::text(system_instruction.into())],
                            });

                            let history = {
                                let guard = state_bg.lock().unwrap();
                                guard.chat_history.clone()
                            };
                            for msg in history.iter().rev().take(20).rev() {
                                if msg.content.starts_with("SYSTEM") {
                                    continue;
                                }
                                context.push(Content {
                                    role: msg.role.clone(),
                                    parts: vec![Part::text(msg.content.clone())],
                                });
                            }

                            context.push(Content {
                                role: "user".into(),
                                parts: vec![Part::text(user_input_text.clone())],
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
                                        s.chat_history.push(SavedMessage {
                                            role: "model".into(),
                                            content: response.clone(),
                                            timestamp: Some(timestamp),
                                        });
                                        brain_bg.save(&s.chat_history);
                                        if is_s9 {
                                            s.s9_status = ShardStatus::Online;
                                        }
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

        // Restore History
        for msg in saved_history {
            if !msg.content.starts_with("SYSTEM") {
                let prefix = if msg.role == "user" {
                    "[ARCHITECT]"
                } else {
                    "[UNA]"
                };
                let ts = msg.timestamp.clone().unwrap_or_else(|| "--:--:--".to_string());
                let _ = gui_tx.send_blocking(GuiUpdate::ConsoleLog(format!(
                    "{} [{}] > {}\n",
                    prefix, ts, msg.content
                )));
            }
        }

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
            Event::Input(text) => {
                let timestamp = Local::now().format("%H:%M:%S").to_string();
                let current_text = format!("\n[ARCHITECT] [{}] > {}\n", timestamp, text);
                s.chat_history.push(SavedMessage {
                    role: "user".to_string(),
                    content: text.clone(),
                    timestamp: Some(timestamp),
                });
                self.append_to_console(&current_text);

                if text.trim() == "/wolf" {
                    s.mode = ViewMode::Wolfpack;
                    self.append_to_console("\n[SYSTEM] :: Switching to Wolfpack Grid...\n");
                } else if text.trim() == "/comms" {
                    s.mode = ViewMode::Comms;
                    self.append_to_console("\n[SYSTEM] :: Secure Comms Established.\n");
                } else if text.trim() == "/clear" {
                    self.append_to_console("\n:: VEIN :: SYSTEM CLEARED\n\n");
                } else if text.trim().starts_with("/upload") {
                    let parts: Vec<&str> = text.split_whitespace().collect();
                    if parts.len() >= 2 {
                        let path = PathBuf::from(parts[1]);
                        trigger_upload(path, self.gui_tx.clone());
                    }
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
            Event::ComplexInput { subject, body, point_break, action: _ } => {
                let prefix = if point_break { "Point Break: " } else { "" };
                let full_message = format!("\nSubject: {}{}\n\n{}", prefix, subject, body);

                let timestamp = Local::now().format("%H:%M:%S").to_string();
                let current_text = format!("\n[ARCHITECT] [{}] > {}\n", timestamp, full_message);
                s.chat_history.push(SavedMessage {
                    role: "user".to_string(),
                    content: full_message.clone(),
                    timestamp: Some(timestamp),
                });
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
