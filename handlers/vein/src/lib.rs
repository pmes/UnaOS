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
use std::collections::HashMap;
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
    contexts: HashMap<String, Vec<SavedMessage>>,
    active_node: String,
    sidebar_position: SidebarPosition,
    sidebar_collapsed: bool,
    s9_status: ShardStatus,
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

            if let Ok(response) = res {
                if response.status().is_success() {
                    let text = response.text().await.unwrap_or_default();
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(uri) = json.get("storage_uri").and_then(|v| v.as_str()) {
                            let mime = get_mime_type(&filename);
                            let tag = format!("\n[ATTACHMENT:{}|{}]\n", mime, uri);
                            let _ = gui_tx.send(GuiUpdate::AppendInput(tag)).await;
                            let _ = gui_tx.send(GuiUpdate::ConsoleLog(format!("\n[SYSTEM] :: {} Encased.\n", filename))).await;
                        }
                    }
                } else {
                    let _ = gui_tx.send(GuiUpdate::ConsoleLog(format!("\n[SYSTEM ERROR] :: Upload Failed: Status {}\n", response.status()))).await;
                }
            } else if let Err(e) = res {
                let _ = gui_tx.send(GuiUpdate::ConsoleLog(format!("\n[SYSTEM ERROR] :: Network Error: {}\n", e))).await;
            }
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
        // Initialize Brains
        let una_brain = BrainManager::new(history_path.clone());
        let una_history = una_brain.load();

        let s9_path = history_path
            .parent()
            .unwrap_or(&PathBuf::from("."))
            .join("s9_history.json");
        let s9_brain = BrainManager::new(s9_path);
        let s9_history = s9_brain.load(); // Try load, or empty if new

        // Create Context Map
        let mut contexts = HashMap::new();
        contexts.insert("una-prime".to_string(), una_history.clone());
        contexts.insert("s9-mule".to_string(), s9_history);

        // Store brains for background thread
        let mut brains = HashMap::new();
        brains.insert("una-prime".to_string(), una_brain.clone());
        brains.insert("s9-mule".to_string(), s9_brain.clone());

        let state = Arc::new(Mutex::new(State {
            mode: ViewMode::Comms,
            nav_index: 0,
            contexts,
            active_node: "una-prime".to_string(),
            sidebar_position: SidebarPosition::default(),
            sidebar_collapsed: false,
            s9_status: ShardStatus::Offline,
        }));

        let (tx_to_bg, mut rx_from_ui) = mpsc::unbounded_channel::<String>();

        let gui_tx_brain = gui_tx.clone();
        let state_bg = state.clone();

        // Pass map of brains to thread
        let brains_bg = brains;

        // Use the main brain (una) for active directive check initially, or just default
        let initial_directive = una_brain.get_active_directive();

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
                        let _ = gui_tx_brain.send(GuiUpdate::ActiveDirective(initial_directive)).await;

                        while let Some(user_input_text) = rx_from_ui.recv().await {
                            // Determine active node and context
                            let (active_node, _history_len) = {
                                let s = state_bg.lock().unwrap();
                                let node = s.active_node.clone();
                                let len = s.contexts.get(&node).map(|v| v.len()).unwrap_or(0);
                                (node, len)
                            };

                            let is_s9 = active_node == "s9-mule";

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

                            // Pre-save (persist user message)
                            {
                                let s = state_bg.lock().unwrap();
                                if let Some(brain) = brains_bg.get(&active_node) {
                                    if let Some(ctx) = s.contexts.get(&active_node) {
                                        brain.save(ctx);
                                    }
                                }
                                if is_s9 {
                                    // Update S9 status in state if needed, though handle_event doesn't set it?
                                    // handle_event pushed message.
                                    // Let's set thinking status here.
                                    // Note: can't easily mutate s inside this block because of brain.save call needing ref to s.contexts?
                                    // brain.save takes &[SavedMessage].
                                }
                            }

                            // Set Thinking Status
                            if is_s9 {
                                {
                                    let mut s = state_bg.lock().unwrap();
                                    s.s9_status = ShardStatus::Thinking;
                                }
                                let _ = gui_tx_brain
                                    .send(GuiUpdate::ShardStatusChanged {
                                        id: "s9-mule".into(),
                                        status: ShardStatus::Thinking,
                                    })
                                    .await;
                            }

                            let _ = gui_tx_brain
                                .send(GuiUpdate::SidebarStatus(WolfpackState::Dreaming))
                                .await;

                            let system_instruction =
                                if is_s9 { "You are S9." } else { "You are Una." };

                            let mut context = Vec::new();
                            context.push(Content {
                                role: "model".into(),
                                parts: vec![Part::text(system_instruction.into())],
                            });

                            let history = {
                                let guard = state_bg.lock().unwrap();
                                guard.contexts.get(&active_node).cloned().unwrap_or_default()
                            };

                            for msg in history.iter().rev().take(20).rev() {
                                if msg.content.starts_with("SYSTEM") {
                                    continue;
                                }

                                let mut parts = Vec::new();
                                let mut current_text = msg.content.clone();

                                while let Some(start) = current_text.find("[ATTACHMENT:") {
                                    if let Some(end) = current_text[start..].find("]") {
                                        let absolute_end = start + end;
                                        let tag = &current_text[start + 12 .. absolute_end];

                                        if let Some((mime, uri)) = tag.split_once('|') {
                                            if start > 0 {
                                                parts.push(Part::text(current_text[..start].to_string()));
                                            }
                                            parts.push(Part::file_data(mime.to_string(), uri.to_string()));
                                        }
                                        current_text = current_text[absolute_end + 1..].to_string();
                                    } else {
                                        break;
                                    }
                                }
                                if !current_text.trim().is_empty() {
                                    parts.push(Part::text(current_text));
                                }

                                context.push(Content {
                                    role: msg.role.clone(),
                                    parts,
                                });
                            }

                            // User input is already in history (pushed by handle_event),
                            // BUT wait:
                            // In original code:
                            // handle_event pushes to chat_history.
                            // Loop creates context from history.
                            // Then pushes user_input_text to context as LAST message?
                            //
                            // Original code:
                            // for msg in history... { context.push(...) }
                            // context.push(Content { role: "user", parts: vec![Part::text(user_input_text)] });
                            //
                            // Wait, if handle_event already pushed it to history, then it's in history!
                            // If we take last 20 from history, it likely includes the just-pushed user message.
                            // THEN we push it AGAIN?
                            //
                            // Let's check original `handle_event`:
                            // s.chat_history.push(...)
                            //
                            // Original loop:
                            // let history = guard.chat_history.clone();
                            // for msg in history.iter().rev().take(20).rev() { ... }
                            // context.push(Content { role: "user", parts: vec![Part::text(user_input_text)] });
                            //
                            // So the user message is duplicated in context sent to LLM?
                            // Or is the loop excluding the last one?
                            // If history includes the new message, then `take(20)` includes it.
                            // Then `context.push` adds it again.
                            // This seems like a bug in original code, or intended emphasis.
                            //
                            // However, since I am refactoring, I should preserve behavior or fix it.
                            // If I use `history` which now includes the message, and I append it again, the LLM sees it twice.
                            //
                            // Actually, in `handle_event`:
                            // s.chat_history.push(...)
                            //
                            // In loop:
                            // guard.chat_history.clone() -> includes msg.
                            //
                            // If I want to match original behavior, I should continue this pattern.
                            // But maybe I should avoid duplication.
                            // The `user_input_text` passed via channel is redundant if it's in state history.
                            // But `handle_event` pushes it.
                            //
                            // If I look closely at original code:
                            // `context.push(Content { role: "user", parts: vec![Part::text(user_input_text.clone())] });`
                            //
                            // If the history contains it, then it's double.
                            // Maybe the history used in loop is *saved* history?
                            // `handle_event` pushes to `s.chat_history`.
                            // So `s.chat_history` has it.
                            //
                            // I will stick to the pattern:
                            // 1. Get history.
                            // 2. Add system prompt.
                            // 3. Add history messages (including the one just added).
                            // 4. Wait, if I add history messages, I shouldn't add `user_input_text` again manually unless I filter it out from history.
                            //
                            // Let's see if I can just rely on history.
                            // If I just iterate history, I get the user message at the end.
                            // I don't need to push it again.
                            //
                            // The original code did:
                            // `context.push(Content { role: "user", parts: vec![Part::text(user_input_text.clone())] });`
                            // This suggests it wants to ensure the last message is the user prompt.
                            // If the history iterator includes it, it appears twice.
                            // I will assume the original code intended to send the prompt.
                            // If I remove the manual push, I rely on history.
                            // But `take(20)` might cut it off if history is long? No, `rev().take(20).rev()` takes the *last* 20. So it definitely includes the latest message.
                            // So currently it sends it twice.
                            // I will fix this "bug" by NOT pushing it manually if it's already in history.
                            // Or rather, I'll just build context from history.
                            //
                            // BUT, wait. `handle_event` sends `text` to channel.
                            // `handle_event` pushes to `chat_history`.
                            //
                            // If I change the loop to NOT push `user_input_text`, I'm changing behavior.
                            // Maybe the duplication is harmless or the model ignores it.
                            // But for "Dynamic Prompt Routing", cleaner is better.
                            // I will just use the history.
                            //
                            // Wait, `user_input_text` is used to determine `is_s9` (in original).
                            //
                            // I'll construct context from history.

                            match client.generate_content(&context).await {
                                Ok((response, metadata)) => {
                                    let timestamp = Local::now().format("%H:%M:%S").to_string();
                                    let prefix = if is_s9 { "S9" } else { "UNA" };
                                    let display =
                                        format!("\n[{}] [{}] :: {}\n", prefix, timestamp, response);
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
                                        if let Some(history) = s.contexts.get_mut(&active_node) {
                                            history.push(SavedMessage {
                                                role: "model".into(),
                                                content: response.clone(),
                                                timestamp: Some(timestamp),
                                            });
                                            // Save via brain
                                            if let Some(brain) = brains_bg.get(&active_node) {
                                                brain.save(history);
                                            }
                                        }

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

        // Restore History (Una-Prime by default)
        for msg in una_history {
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
            Event::ShardSelect(node_id) => {
                s.active_node = node_id.clone();
                let _ = self.gui_tx.send_blocking(GuiUpdate::ClearConsole);

                // Replay history
                if let Some(history) = s.contexts.get(&node_id) {
                    for msg in history {
                        if !msg.content.starts_with("SYSTEM") {
                            // Determine prefix based on node?
                            // Currently code uses [UNA] for model.
                            // Maybe use [S9] if node is s9-mule?
                            // The saved message role is "user" or "model".
                            let prefix = if msg.role == "user" {
                                "[ARCHITECT]"
                            } else {
                                if node_id == "s9-mule" { "[S9]" } else { "[UNA]" }
                            };
                            let ts = msg.timestamp.clone().unwrap_or_else(|| "--:--:--".to_string());
                            let _ = self.gui_tx.send_blocking(GuiUpdate::ConsoleLog(format!(
                                "{} [{}] > {}\n",
                                prefix, ts, msg.content
                            )));
                        }
                    }
                }
            }
            Event::Input { target: _, text } => {
                let timestamp = Local::now().format("%H:%M:%S").to_string();
                let current_text = format!("\n[ARCHITECT] [{}] > {}\n", timestamp, text);

                let active = s.active_node.clone();
                if let Some(history) = s.contexts.get_mut(&active) {
                    history.push(SavedMessage {
                        role: "user".to_string(),
                        content: text.clone(),
                        timestamp: Some(timestamp),
                    });
                } else {
                    // Fallback or error? Should not happen if initialized correctly.
                    // Recover by creating entry?
                    s.contexts.insert(active.clone(), vec![SavedMessage {
                        role: "user".to_string(),
                        content: text.clone(),
                        timestamp: Some(timestamp),
                    }]);
                }

                self.append_to_console(&current_text);

                if text.trim() == "/wolf" {
                    s.mode = ViewMode::Wolfpack;
                    self.append_to_console("\n[SYSTEM] :: Switching to Wolfpack Grid...\n");
                } else if text.trim() == "/comms" {
                    s.mode = ViewMode::Comms;
                    self.append_to_console("\n[SYSTEM] :: Secure Comms Established.\n");
                } else if text.trim() == "/clear" {
                    let _ = self.gui_tx.send_blocking(GuiUpdate::ClearConsole);
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
                        let _ = self.gui_tx.send_blocking(GuiUpdate::ClearConsole);
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

                let active = s.active_node.clone();
                if let Some(history) = s.contexts.get_mut(&active) {
                    history.push(SavedMessage {
                        role: "user".to_string(),
                        content: full_message.clone(),
                        timestamp: Some(timestamp),
                    });
                }

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
