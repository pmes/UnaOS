// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

pub mod context;
pub mod cortex;
pub mod gravity;
pub mod storage;
pub mod synapse;

use chrono::Local;
use gneiss_pal::api::{Content, Part, ResilientClient};
use gneiss_pal::forge::ForgeClient;
use gneiss_pal::persistence::BrainManager;
use gneiss_pal::{
    AppHandler, DashboardState, Event, GuiUpdate, PreFlightPayload, Shard, ShardRole, ShardStatus,
    SidebarPosition, ViewMode, WolfpackState,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

use bandy::{BandyMember, SMessage, Synapse};

struct State {
    mode: ViewMode,
    nav_index: usize,
    sidebar_position: SidebarPosition,
    sidebar_collapsed: bool,
    s9_status: ShardStatus,
    live_context: Vec<bandy::WeightedSkeleton>,
    skeleton_cache: std::collections::HashMap<PathBuf, Arc<String>>, // <-- NEW
    focused_file: Option<PathBuf>,                                   // <-- NEW
    pending_prompts: std::collections::HashMap<u64, String>,         // <-- NEW
}

fn get_mime_type(filename: &str) -> String {
    let lower = filename.to_lowercase();
    if lower.ends_with(".pdf") {
        "application/pdf".to_string()
    } else if lower.ends_with(".png") {
        "image/png".to_string()
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg".to_string()
    } else if lower.ends_with(".mp4") {
        "video/mp4".to_string()
    } else {
        "application/octet-stream".to_string()
    }
}

async fn execute_upload(path: PathBuf, gui_tx: async_channel::Sender<GuiUpdate>) {
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let _ = gui_tx
        .send(GuiUpdate::ConsoleLog(format!(
            "\n[SYSTEM] :: Uploading: {}...\n",
            filename
        )))
        .await;

    let file_bytes = match tokio::fs::read(&path).await {
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

    match client.post(url).multipart(form).send().await {
        Ok(response) if response.status().is_success() => {
            let text = response.text().await.unwrap_or_default();
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(uri) = json.get("storage_uri").and_then(|v| v.as_str()) {
                    let mime = get_mime_type(&filename);
                    let tag = format!("\n[ATTACHMENT:{}|{}]\n", mime, uri);
                    let _ = gui_tx.send(GuiUpdate::AppendInput(tag)).await;
                    let _ = gui_tx
                        .send(GuiUpdate::ConsoleLog(format!(
                            "\n[SYSTEM] :: Upload Complete: {}\n",
                            filename
                        )))
                        .await;
                }
            }
        }
        Ok(response) => {
            let _ = gui_tx
                .send(GuiUpdate::ConsoleLog(format!(
                    "\n[SYSTEM ERROR] :: Upload Failed: {}\n",
                    response.status()
                )))
                .await;
        }
        Err(e) => {
            let _ = gui_tx
                .send(GuiUpdate::ConsoleLog(format!(
                    "\n[SYSTEM ERROR] :: Upload Request Failed: {}\n",
                    e
                )))
                .await;
        }
    }
}

fn parse_multimodal_text(text: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut current_text = text.to_string();

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

                if clean_mime.starts_with("image/")
                    || clean_mime.starts_with("video/")
                    || clean_mime == "application/pdf"
                {
                    parts.push(Part::file_data(clean_mime, clean_uri));
                } else {
                    parts.push(Part::text(format!(
                        "[ATTACHMENT:{}|{}]",
                        clean_mime, clean_uri
                    )));
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
    synapse: Synapse,
    telemetry_tx: async_channel::Sender<SMessage>, // <-- NEW
}

impl VeinHandler {
    pub fn new(
        gui_tx: async_channel::Sender<GuiUpdate>,
        history_path: PathBuf,
        synapse: Synapse,
        telemetry_tx: async_channel::Sender<SMessage>, // Pure Async Channel
        shutdown_tx: tokio::sync::broadcast::Sender<()>,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let brain = BrainManager::new(history_path);

        let state = Arc::new(Mutex::new(State {
            mode: ViewMode::Comms,
            nav_index: 0,
            sidebar_position: SidebarPosition::default(),
            sidebar_collapsed: false,
            s9_status: ShardStatus::Offline,
            live_context: Vec::new(),
            skeleton_cache: std::collections::HashMap::new(), // <-- NEW
            focused_file: None,                               // <-- NEW
            pending_prompts: std::collections::HashMap::new(), // <-- NEW
        }));

        let (tx_to_bg, mut rx_from_ui) = mpsc::unbounded_channel::<String>();

        let gui_tx_brain = gui_tx.clone();
        let state_bg = state.clone();
        let brain_bg = brain.clone();
        let synapse_bg = synapse.clone();
        let telemetry_tx_bg = telemetry_tx.clone();
        let synapse_loop = synapse.clone(); // <-- Clone for the async block to prevent moving the field
        let tx_to_bg_loop = tx_to_bg.clone();

        // Capture the JoinHandle by spawning on the current Runtime (instead of std::thread)
        let brain_loop_handle = tokio::runtime::Handle::current().spawn(async move {
                let now = Local::now().format("%Y-%m-%d %H:%M:%S.%3f");
                let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(format!("VEIN: [{}] [INFO] :: BRAIN :: Connecting...\n", now))).await;

                // Fire up the Cortex Indexer in the background
                let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                // We are now INSIDE the Tokio context.
                // We drop the `rt.` prefix and use `tokio::spawn` directly.
                // This schedules the task on the running engine without trying to move the engine itself.
                let state_indexer = state_bg.clone();
                let telemetry_tx_indexer = telemetry_tx_bg.clone();
                let mut shutdown_rx_indexer = shutdown_tx.subscribe();
                tokio::spawn(async move {
                    tokio::select! {
                        _ = shutdown_rx_indexer.recv() => {
                            log::info!(":: CORTEX :: Shutting down indexer cleanly.");
                            return;
                        }
                                cache = cortex::run_indexer(root, synapse_bg) => {
                            let live_ctx = {
                                let mut s = state_indexer.lock().unwrap();
                                s.skeleton_cache = cache;

                                // Initial Gravity Calculation
                                let gravity = crate::gravity::GravityWell::new();
                                let live_ctx = gravity.calculate_scores(&s.skeleton_cache);
                                s.live_context = live_ctx.clone();
                                live_ctx
                            };

                            if !live_ctx.is_empty() {
                                let _ = telemetry_tx_indexer.send(SMessage::ContextTelemetry { skeletons: live_ctx }).await;
                            }
                        }
                    }
                });

                tokio::time::sleep(Duration::from_millis(800)).await;

                // Request initial history load via Synapse
                let _ = synapse_loop.fire_async(SMessage::StorageLoadAll { receipt_id: 0 }).await;

                let forge_client = match ForgeClient::new() {
                    Ok(client) => {
                        let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(":: FORGE :: CONNECTED\n".into())).await;
                        Some(client)
                    }
                    Err(_) => None,
                };

                let client_res = ResilientClient::new().await;
                match client_res {
                    Ok(mut client) => {
                        let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(":: BRAIN :: ONLINE (PLEXUS ENABLED)\n\n".into())).await;

                        let directive = brain_bg.get_active_directive();
                        let _ = gui_tx_brain.send(GuiUpdate::ActiveDirective(directive)).await;

                        let mut shutdown_rx_brain = shutdown_tx.subscribe();
                        let bandy_rx_brain = synapse_loop.rx();
                        let mut receipt_counter: u64 = 1;

                        loop {
                            tokio::select! {
                                Ok(msg) = bandy_rx_brain.recv() => {
                                    match msg {
                                        SMessage::TriggerUpload(path) => {
                                            let gui_tx_upload = gui_tx_brain.clone();
                                            tokio::spawn(async move {
                                                execute_upload(path, gui_tx_upload).await;
                                            });
                                        }
                                        SMessage::StorageLoadAllResult { records, receipt_id: _ } => {
                                            let items: Vec<gneiss_pal::HistoryItem> = records.into_iter().map(|r| gneiss_pal::HistoryItem {
                                                sender: r.sender,
                                                content: r.content,
                                                timestamp: r.timestamp,
                                                is_chat: r.is_chat,
                                            }).collect();
                                            let _ = gui_tx_brain.send(GuiUpdate::HistoryBatch(items)).await;
                                        }
                                        SMessage::StorageQueryResult { receipt_id, memories, directives, engrams, chrono } => {
                                            let mut s = state_bg.lock().unwrap();
                                            if let Some(prompt) = s.pending_prompts.remove(&receipt_id) {
                                                let payload = serde_json::to_string(&serde_json::json!({
                                                    "receipt_id": receipt_id,
                                                    "memories": memories,
                                                    "directives": directives,
                                                    "engrams": engrams,
                                                    "chrono": chrono,
                                                    "prompt": prompt
                                                })).unwrap();
                                                let _ = tx_to_bg_loop.send(format!("STORAGE_RESULT:{}", payload));
                                            }
                                        }
                                        SMessage::StorageSaveResult { receipt_id: _, success, error } => {
                                            if !success {
                                                if let Some(err) = error {
                                                    eprintln!(":: PLEXUS :: Failed to save memory: {}", err);
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                _ = shutdown_rx_brain.recv() => {
                                    log::info!(":: VEIN :: Brain Loop terminating...");
                                    break;
                                }
                                user_input_opt = rx_from_ui.recv() => {
                                    let user_input_text = match user_input_opt {
                                        Some(text) => text,
                                        None => break,
                                    };
                            // === ROUTE: Directive Injection ===
                            if user_input_text == "LOAD_HISTORY" {
                                receipt_counter += 1;
                                let _ = synapse_loop.fire_async(SMessage::StorageLoadAll { receipt_id: receipt_counter }).await;
                                continue;
                            }

                            if user_input_text.starts_with("SAVE_DIRECTIVE:") {
                                let dir_text = user_input_text["SAVE_DIRECTIVE:".len()..].to_string();
                                let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                                let synapse_clone = synapse_loop.clone();

                                match client.embed_content(&dir_text).await {
                                    Ok(embedding) => {
                                        receipt_counter += 1;
                                        let _ = synapse_clone.fire_async(SMessage::StorageSave {
                                            receipt_id: receipt_counter,
                                            sender: "system".to_string(),
                                            content: dir_text,
                                            timestamp,
                                            embedding,
                                            memory_type: "directive".to_string(),
                                        }).await;
                                    }
                                    Err(e) => {
                                        eprintln!(":: PLEXUS :: Failed to embed directive: {}", e);
                                    }
                                }
                                continue;
                            }

                            // === ROUTE A: Execution of an Approved Interceptor Payload ===
                            if user_input_text.starts_with("DISPATCH_PAYLOAD:") {
                                let payload_str = &user_input_text["DISPATCH_PAYLOAD:".len()..];

                                if let Ok(payload) = serde_json::from_str::<PreFlightPayload>(payload_str) {
                                    let mut system_builder = payload.system.clone();
                                    if !payload.directives.is_empty() {
                                        system_builder.push_str("\n\n[ACTIVE DIRECTIVES]:\n");
                                        system_builder.push_str(&payload.directives);
                                    }
                                    if !payload.engrams.is_empty() {
                                        system_builder.push_str("\n\n[HISTORICAL & SHORT-TERM ENGRAMS]:\n");
                                        system_builder.push_str(&payload.engrams);
                                    }

                                    let mut context: Vec<Content> = Vec::new();
                                    let current_text = format!("{}\n\n[CURRENT PROMPT]:\n{}", system_builder, payload.prompt);
                                    context.push(Content {
                                        role: "user".into(),
                                        parts: parse_multimodal_text(&current_text),
                                    });

                                    match client.generate_content(&context).await {
                                        Ok((response, metadata)) => {
                                            let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                                            let display = format!("\n[UNA] [{}] :: {}\n", timestamp, response);

                                            // ONLY echo to console upon actual API generation
                                            let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(display.clone())).await;

                                            if let Some(meta) = metadata {
                                                let p_tok = meta.prompt_token_count.unwrap_or(0);
                                                let c_tok = meta.candidates_token_count.unwrap_or(0);
                                                let t_tok = meta.total_token_count.unwrap_or(0);
                                                let _ = gui_tx_brain.send(GuiUpdate::TokenUsage(p_tok, c_tok, t_tok)).await;
                                            }

                                            let safe_embed: String = response.chars().take(6000).collect();
                                            let response_embedding = match client.embed_content(&safe_embed).await {
                                                Ok(vec) => vec,
                                                Err(_) => vec![],
                                            };

                                            let response_clone = response.clone();
                                            let timestamp_clone = timestamp.clone();
                                            let synapse_clone = synapse_loop.clone();

                                            receipt_counter += 1;
                                            let _ = synapse_clone.fire_async(SMessage::StorageSave {
                                                receipt_id: receipt_counter,
                                                sender: "model".to_string(),
                                                content: response_clone,
                                                timestamp: timestamp_clone,
                                                embedding: response_embedding,
                                                memory_type: "chat".to_string(),
                                            }).await;

                                            let _ = gui_tx_brain.send(GuiUpdate::SidebarStatus(WolfpackState::Idle)).await;

                                            // Generate Engram
                                            let mut raw_user_prompt = payload.prompt.clone();
                                            if raw_user_prompt.is_empty() {
                                                raw_user_prompt = "[System: User provided multimodal input without text.]".to_string();
                                            }

                                            let ai_response_clone = response.clone();
                                            tokio::spawn(async move {
                                                if let Ok(mut client_clone) = ResilientClient::new().await {
                                                    if let Ok(engram) = crate::context::compress_into_engram(&mut client_clone, &raw_user_prompt, &ai_response_clone).await {
                                                        if let Ok(engram_embedding) = client_clone.embed_content(&engram).await {
                                                            let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                                                            let _ = synapse_clone.fire_async(SMessage::StorageSave {
                                                                receipt_id: 0,
                                                                sender: "system".to_string(),
                                                                content: engram,
                                                                timestamp,
                                                                embedding: engram_embedding,
                                                                memory_type: "engram".to_string(),
                                                            }).await;
                                                        }
                                                    }
                                                }
                                            });
                                        }
                                        Err(e) => {
                                            let err_msg = format!("Synapse failure: {}", e);
                                            let _ = gui_tx_brain.send(GuiUpdate::SynapseError(err_msg)).await;
                                        }
                                    }
                                } else {
                                    let _ = gui_tx_brain.send(GuiUpdate::SynapseError("Failed to deserialize PreFlightPayload".to_string())).await;
                                }
                                continue;
                            }


                            if user_input_text.trim() == "/clear" {
                                let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(":: VEIN :: /clear command has been deprecated architecturally.\n".into())).await;
                                continue;
                            }

                            let is_s9 = user_input_text.starts_with("/s9");

                            if user_input_text.starts_with("READ_REPO:") {
                                let parts: Vec<&str> = user_input_text.split(':').collect();
                                if parts.len() >= 5 {
                                    let owner = parts[1];
                                    let repo = parts[2];
                                    let branch_raw = parts[3];
                                    let path = parts[4];
                                    let branch = if branch_raw.is_empty() { None } else { Some(branch_raw) };
                                    let response_msg = if let Some(fc) = &forge_client {
                                        match fc.get_file_content(owner, repo, path, branch).await {
                                            Ok(content) => format!("\n[FORGE READ] :: {}/{}/{} ::\n{}\n", owner, repo, path, content),
                                            Err(e) => format!("\n[FORGE ERROR] :: {}\n", e),
                                        }
                                    } else {
                                        "\n[FORGE] :: Offline.\n".to_string()
                                    };
                                    let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(response_msg)).await;
                                }
                                continue;
                            }

                            {
                                let mut s = state_bg.lock().unwrap();
                                if is_s9 { s.s9_status = ShardStatus::Thinking; }
                            }

                            if is_s9 {
                                let _ = gui_tx_brain.send(GuiUpdate::ShardStatusChanged {
                                    id: "s9-mule".into(),
                                    status: ShardStatus::Thinking,
                                }).await;
                            }

                            if user_input_text.starts_with("STORAGE_RESULT:") {
                                let payload_str = &user_input_text["STORAGE_RESULT:".len()..];
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload_str) {
                                    // Extract components
                                    let mut retrieved_context = String::new();
                                    let mut retrieved_directives = String::new();
                                    let mut retrieved_engrams = String::new();
                                    let mut chronological_engrams = String::new();

                                    if let Some(memories) = json.get("memories").and_then(|v| v.as_array()) {
                                        let mems: Vec<String> = memories.iter().map(|m| m.as_str().unwrap_or("").to_string()).collect();
                                        if !mems.is_empty() {
                                            retrieved_context = mems.join("\n\n");
                                        }
                                    }

                                    if let Some(directives) = json.get("directives").and_then(|v| v.as_array()) {
                                        let dirs: Vec<String> = directives.iter().map(|d| d.as_str().unwrap_or("").to_string()).collect();
                                        if !dirs.is_empty() {
                                            retrieved_directives = dirs.join("\n\n");
                                        }
                                    }

                                    if let Some(engrams) = json.get("engrams").and_then(|v| v.as_array()) {
                                        let engs: Vec<String> = engrams.iter().map(|e| e.as_str().unwrap_or("").to_string()).collect();
                                        if !engs.is_empty() {
                                            retrieved_engrams = engs.join("\n\n");
                                        }
                                    }

                                    if let Some(chrono) = json.get("chrono").and_then(|v| v.as_array()) {
                                        let mut chr: Vec<String> = chrono.iter().map(|c| c.as_str().unwrap_or("").to_string()).collect();
                                        if !chr.is_empty() {
                                            chr.reverse();
                                            chronological_engrams = chr.join("\n\n");
                                        }
                                    }

                                    // Now assemble the PreFlightPayload
                                    let mut system_builder = if is_s9 {
                                        "You are S9.".to_string()
                                    } else {
                                        "SYSTEM_INSTRUCTION: You are an AI Shard operating within the UnaOS cognitive matrix.".to_string()
                                    };

                                    if !retrieved_directives.is_empty() {
                                        system_builder.push_str("\n\n[ACTIVE DIRECTIVES]:\n");
                                        system_builder.push_str(&retrieved_directives);
                                    }

                                    // Inject high-gravity live workspace context
                                    {
                                        let s = state_bg.lock().unwrap();
                                        if !s.live_context.is_empty() {
                                            system_builder.push_str("\n\n[LIVE WORKSPACE CONTEXT (GRAVITY WELL)]:\n");
                                            for skel in &s.live_context {
                                                system_builder.push_str(&format!(
                                                    "--- FILE: {} (Gravity: {:.2}) ---\n{}\n\n",
                                                    skel.path.display(),
                                                    skel.score,
                                                    skel.content
                                                ));
                                            }
                                        }
                                    }

                                    if !retrieved_context.is_empty() {
                                        system_builder.push_str("\n\n[SEMANTIC MEMORY RECALL]:\n");
                                        system_builder.push_str(&retrieved_context);
                                    }

                                    let mut engrams_combined = String::new();
                                    if !retrieved_engrams.is_empty() {
                                        engrams_combined.push_str(&retrieved_engrams);
                                    }
                                    if !chronological_engrams.is_empty() {
                                        if !engrams_combined.is_empty() {
                                            engrams_combined.push_str("\n\n");
                                        }
                                        engrams_combined.push_str(&chronological_engrams);
                                    }

                                    let prompt = json.get("prompt").and_then(|v| v.as_str()).unwrap_or("").to_string();

                                    let pre_flight_payload = PreFlightPayload {
                                        system: system_builder,
                                        directives: retrieved_directives,
                                        engrams: engrams_combined,
                                        prompt,
                                    };

                                    // HALT GENERATION. Dispatch the strongly typed payload to the UI Interceptor.
                                    let _ = gui_tx_brain.send(GuiUpdate::ReviewPayload(pre_flight_payload)).await;
                                }
                                continue;
                            }

                            // Normal User Input Flow
                            let user_embedding = match client.embed_content(&user_input_text).await {
                                Ok(vec) => vec,
                                Err(e) => {
                                    eprintln!(":: PLEXUS :: Embedding Failed: {}", e);
                                    vec![]
                                }
                            };

                            // SAVE USER MEMORY LAST (Resolving Temporal Paradox)
                            // We strip heavy attachments first.
                            let parsed_parts_for_save = parse_multimodal_text(&user_input_text);
                            let mut clean_memory_text = String::new();
                            for part in &parsed_parts_for_save {
                                if let Part::Text { text } = part {
                                    clean_memory_text.push_str(text);
                                } else {
                                    clean_memory_text.push_str(" [System: User attached a file/image] ");
                                }
                            }
                            let timestamp = Local::now().format("%H:%M:%S").to_string();

                            let synapse_clone = synapse_loop.clone();
                            let clean_memory_clone = clean_memory_text.clone();
                            let user_embedding_clone = user_embedding.clone();
                            let timestamp_clone = timestamp.clone();

                            // Fire-and-forget user memory save via Synapse
                            receipt_counter += 1;
                            let _ = synapse_clone.fire_async(SMessage::StorageSave {
                                receipt_id: receipt_counter,
                                sender: "user".to_string(),
                                content: clean_memory_clone,
                                timestamp: timestamp_clone,
                                embedding: user_embedding_clone.clone(),
                                memory_type: "chat".to_string(),
                            }).await;

                            receipt_counter += 1;
                            let query_receipt_id = receipt_counter;

                            {
                                let mut s = state_bg.lock().unwrap();
                                s.pending_prompts.insert(query_receipt_id, user_input_text.clone());
                            }

                            // Send query
                            let _ = synapse_loop.fire_async(SMessage::StorageQuery {
                                receipt_id: query_receipt_id,
                                embedding: user_embedding_clone,
                            }).await;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(format!(":: FATAL :: {}\n", e))).await;
                    }
                }
        });

        (
            Self {
                state,
                tx: tx_to_bg,
                gui_tx,
                synapse,
                telemetry_tx, // <-- NEW
            },
            brain_loop_handle,
        )
    }

    fn append_to_console(&self, text: &str) {
        let _ = self
            .gui_tx
            .try_send(GuiUpdate::ConsoleLog(text.to_string()));
    }
}

impl BandyMember for VeinHandler {
    fn publish(&self, _topic: &str, msg: SMessage) -> anyhow::Result<()> {
        self.synapse.fire(msg);
        Ok(())
    }
}

impl AppHandler for VeinHandler {
    fn handle_event(&mut self, event: Event) {
        let mut s = self.state.lock().unwrap();

        match event {
            Event::Input { target: _, text } => {
                // --- NEW: DYNAMIC GRAVITY RECALCULATION ---
                {
                    let mut gravity = crate::gravity::GravityWell::new();
                    if let Some(f) = &s.focused_file {
                        gravity.set_focus(f.clone());
                    }
                    gravity.extract_keywords(&text);
                    let live_ctx = gravity.calculate_scores(&s.skeleton_cache);
                    s.live_context = live_ctx.clone();

                    // Instantly update the TeleHUD
                    let _ = self.telemetry_tx.send_blocking(SMessage::ContextTelemetry {
                        skeletons: live_ctx,
                    });
                }
                // ------------------------------------------

                let timestamp = Local::now().format("%H:%M:%S").to_string();
                let _current_text = format!("\n[ARCHITECT] [{}] > {}\n", timestamp, text);

                // Input echo removed as per Architect mandate

                if text.trim() == "/wolf" {
                    s.mode = ViewMode::Wolfpack;
                    self.append_to_console("\n[SYSTEM] :: Switching to Wolfpack Grid...\n");
                } else if text.trim() == "/comms" {
                    s.mode = ViewMode::Comms;
                    self.append_to_console("\n[SYSTEM] :: Secure Comms Established.\n");
                } else if text.trim() == "/clear" {
                    self.append_to_console("\n:: VEIN :: SYSTEM CLEARED\n\n");
                    let _ = self.tx.send("/clear".to_string());
                } else if let Some(path_str) = text.trim().strip_prefix("/upload ") {
                    let path = PathBuf::from(path_str.trim());
                    let _ = self.publish("upload", SMessage::TriggerUpload(path));
                } else if let Some(dir_text) = text.trim().strip_prefix("/directive ") {
                    self.append_to_console("\n[SYSTEM] :: Burning Active Directive to Vault...\n");
                    let _ = self.tx.send(format!("SAVE_DIRECTIVE:{}", dir_text));
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
                let _ = self.publish("upload", SMessage::TriggerUpload(path.clone()));

                // --- NEW: DYNAMIC GRAVITY FOCUS ---
                s.focused_file = Some(path.clone());
                let mut gravity = crate::gravity::GravityWell::new();
                gravity.set_focus(path);
                let live_ctx = gravity.calculate_scores(&s.skeleton_cache);
                s.live_context = live_ctx.clone();

                let _ = self.telemetry_tx.send_blocking(SMessage::ContextTelemetry {
                    skeletons: live_ctx,
                });
                // ----------------------------------
            }
            Event::ToggleSidebar => {
                s.sidebar_collapsed = !s.sidebar_collapsed;
            }
            Event::ComplexInput {
                target: _,
                subject,
                body,
                point_break,
                action: _,
            } => {
                let prefix = if point_break { "Point Break: " } else { "" };
                let full_message = format!("\nSubject: {}{}\n\n{}", prefix, subject, body);

                let timestamp = Local::now().format("%H:%M:%S").to_string();
                let current_text = format!("\n[ARCHITECT] [{}] > {}\n", timestamp, full_message);
                self.append_to_console(&current_text);

                if let Err(e) = self.tx.send(full_message) {
                    self.append_to_console(&format!("\n[SYSTEM ERROR] :: Failed to send: {}\n", e));
                }
            }
            Event::DispatchPayload(json_payload) => {
                // The UI has approved the payload. We send it back to the Brain via the mpsc channel
                // prefixed with our magic header so the loop picks it up.
                let _ = self.tx.send(format!("DISPATCH_PAYLOAD:{}", json_payload));
            }
            Event::LoadHistory => {
                // Not returning anything directly from memory yet, we need access to the disk manager.
                // However, since DiskManager is behind a Mutex inside the background thread,
                // we should probably send a message to the background thread to handle it, or just
                // load them if we have access. Wait, `handle_event` doesn't have direct access to `disk`.
                // Actually, `vein`'s history logic should query `UnaFS`.
                // To do this right, we can send a custom command via `self.tx`, e.g. "LOAD_HISTORY".
                let _ = self.tx.send("LOAD_HISTORY".to_string());
            }
            _ => {}
        }
    }

    fn view(&self) -> DashboardState {
        let s = self.state.lock().unwrap();
        DashboardState {
            mode: s.mode.clone(),
            nav_items: vec!["Prime".into(), "Encrypted".into(), "Jules (Private)".into()],
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
