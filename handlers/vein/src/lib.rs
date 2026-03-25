// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

pub mod context;
pub mod cortex;
pub mod gravity;
pub mod skeleton;
pub mod storage;
pub mod synapse;

use chrono::Local;
use gneiss_pal::api::{Content, Part, ResilientClient};
use gneiss_pal::forge::ForgeClient;
use gneiss_pal::persistence::BrainManager;
use gneiss_pal::{AppHandler, Event};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;

// All UI/High-level state types have been evacuated from gneiss_pal and now live in bandy::state
use bandy::state::{
    AppState, PreFlightPayload, ShardStatus, WolfpackState, HistoryItem, MAX_STATE_CAPACITY
};
use bandy::{BandyMember, SMessage, Synapse};

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

async fn execute_upload(path: PathBuf, app_state: Arc<RwLock<AppState>>, synapse: Synapse) {
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    // STATE MUTATION & PING
    {
        let mut s = app_state.write().unwrap();
        s.console_logs.push_back(format!("\n[SYSTEM] :: Uploading: {}...\n", filename));
        s.console_seq += 1;
        while s.console_logs.len() > MAX_STATE_CAPACITY {
            s.console_logs.pop_front();
        }
    }
    let _ = synapse.fire_async(SMessage::StateInvalidated).await;

    let file_bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) => {
            {
                let mut s = app_state.write().unwrap();
                s.console_logs.push_back(format!("\n[SYSTEM ERROR] :: File Read Failed: {}\n", e));
                s.console_seq += 1;
                while s.console_logs.len() > MAX_STATE_CAPACITY {
                    s.console_logs.pop_front();
                }
            }
            let _ = synapse.fire_async(SMessage::StateInvalidated).await;
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

                    // STATE MUTATION & PING
                    {
                        let mut s = app_state.write().unwrap();
                        s.active_input_buffer.push_str(&tag);
                        s.console_logs.push_back(format!("\n[SYSTEM] :: Upload Complete: {}\n", filename));
                        s.console_seq += 1;
                        while s.console_logs.len() > MAX_STATE_CAPACITY {
                            s.console_logs.pop_front();
                        }
                    }
                    let _ = synapse.fire_async(SMessage::StateInvalidated).await;
                }
            }
        }
        Ok(response) => {
            {
                let mut s = app_state.write().unwrap();
                s.console_logs.push_back(format!("\n[SYSTEM ERROR] :: Upload Failed: {}\n", response.status()));
                s.console_seq += 1;
                while s.console_logs.len() > MAX_STATE_CAPACITY {
                    s.console_logs.pop_front();
                }
            }
            let _ = synapse.fire_async(SMessage::StateInvalidated).await;
        }
        Err(e) => {
            {
                let mut s = app_state.write().unwrap();
                s.console_logs.push_back(format!("\n[SYSTEM ERROR] :: Upload Request Failed: {}\n", e));
                s.console_seq += 1;
                while s.console_logs.len() > MAX_STATE_CAPACITY {
                    s.console_logs.pop_front();
                }
            }
            let _ = synapse.fire_async(SMessage::StateInvalidated).await;
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
    pub app_state: Arc<RwLock<AppState>>,
    tx: mpsc::UnboundedSender<String>,
    synapse: Synapse,
}

impl VeinHandler {
    pub fn new(
        history_path: PathBuf,
        synapse: Synapse,
        app_state: Arc<RwLock<AppState>>,
        shutdown_tx: tokio::sync::broadcast::Sender<()>,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let brain = BrainManager::new(history_path);
        let (tx_to_bg, mut rx_from_ui) = mpsc::unbounded_channel::<String>();

        // 1. THE SHADOW BOUNDARY
        let state_bg = app_state.clone();
        let brain_bg = brain.clone();
        let synapse_loop = synapse.clone();
        let tx_to_bg_loop = tx_to_bg.clone();

        // Capture the JoinHandle by spawning on the current Runtime
        let brain_loop_handle = tokio::runtime::Handle::current().spawn(async move {

            let now = Local::now().format("%Y-%m-%d %H:%M:%S.%3f");

            // 2. THE LEXICAL LOCK & PING
            {
                let mut s = state_bg.write().unwrap();
                s.console_logs.push_back(format!("VEIN: [{}] [INFO] :: BRAIN :: Connecting...\n", now));
                s.console_seq += 1;
                while s.console_logs.len() > MAX_STATE_CAPACITY {
                    s.console_logs.pop_front();
                }
            }
            let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;

            let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

            // Indexer Shadow Boundary
            let state_indexer = state_bg.clone();
            let mut shutdown_rx_indexer = shutdown_tx.subscribe();
            let synapse_indexer = synapse_loop.clone();

            tokio::spawn(async move {
                tokio::select! {
                    _ = shutdown_rx_indexer.recv() => {
                        log::info!(":: CORTEX :: Shutting down indexer cleanly.");
                        return;
                    }
                    cache = cortex::run_indexer(root, synapse_indexer.clone()) => {
                        let live_ctx = {
                            let _s = state_indexer.write().unwrap();
                            // Note: State definition might need skeleton_cache
                            // But per Architect's previous changes, it was moved or simplified.
                            // Assuming AppState has live_context, we simply set it.
                            let gravity = crate::gravity::GravityWell::new();
                            let live_ctx = gravity.calculate_scores(&cache);

                            // Let's ensure we are not strictly relying on s.skeleton_cache
                            // if it doesn't exist in AppState. We calculate it inline.
                            // Assuming AppState does NOT have skeleton_cache based on the provided bandy state struct earlier.
                            // I will simply broadcast the context telemetry.

                            live_ctx
                        };

                        if !live_ctx.is_empty() {
                            let _ = synapse_indexer.fire_async(SMessage::ContextTelemetry { skeletons: live_ctx }).await;
                        }
                    }
                }
            });

            tokio::time::sleep(Duration::from_millis(800)).await;

            let _ = synapse_loop.fire_async(SMessage::StorageLoadPaged { receipt_id: 0, offset: 0, limit: 50 }).await;

            let forge_client = match ForgeClient::new() {
                Ok(client) => {
                    {
                        let mut s = state_bg.write().unwrap();
                        s.console_logs.push_back(":: FORGE :: CONNECTED\n".into());
                        s.console_seq += 1;
                        while s.console_logs.len() > MAX_STATE_CAPACITY {
                            s.console_logs.pop_front();
                        }
                    }
                    let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;
                    Some(client)
                }
                Err(_) => None,
            };

            let client_res = ResilientClient::new().await;
            match client_res {
                Ok(mut client) => {
                    {
                        let mut s = state_bg.write().unwrap();
                        s.console_logs.push_back(":: BRAIN :: ONLINE (PLEXUS ENABLED)\n\n".into());
                        s.console_seq += 1;
                        while s.console_logs.len() > MAX_STATE_CAPACITY {
                            s.console_logs.pop_front();
                        }
                        s.active_directive = brain_bg.get_active_directive();
                    }
                    let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;

                    let mut shutdown_rx_brain = shutdown_tx.subscribe();
                    let mut bandy_rx_brain = synapse_loop.subscribe();
                    let mut receipt_counter: u64 = 1;

                    // Simple local state for loop variables not in AppState
                    let mut pending_prompts: std::collections::HashMap<u64, String> = std::collections::HashMap::new();

                    loop {
                        tokio::select! {
                            recv_res = bandy_rx_brain.recv() => {
                                match recv_res {
                                    Ok(msg) => match msg {
                                        SMessage::TriggerUpload(path) => {
                                        let app_state_upload = state_bg.clone();
                                        let synapse_upload = synapse_loop.clone();
                                        tokio::spawn(async move {
                                            execute_upload(path, app_state_upload, synapse_upload).await;
                                        });
                                    }
                                    SMessage::StorageLoadPagedResult { records, receipt_id: _ } => {
                                        {
                                            let mut s = state_bg.write().unwrap();
                                            s.history = records.into_iter().map(|r| HistoryItem {
                                                sender: r.sender,
                                                content: r.content,
                                                timestamp: r.timestamp,
                                                is_chat: r.is_chat,
                                            }).collect();
                                            while s.history.len() > MAX_STATE_CAPACITY {
                                                s.history.pop_front();
                                            }
                                            s.history_seq += s.history.len();
                                        }
                                        let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;
                                    }
                                    SMessage::StorageQueryResult { receipt_id, memories, directives, engrams, chrono } => {

                                        if let Some(prompt) = pending_prompts.remove(&receipt_id) {
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
                                    SMessage::StorageSaveResult { receipt_id: _, success: _, error: _ } => {
                                    }
                                    SMessage::StateInvalidated => {
                                    }
                                    SMessage::Matrix(matrix_event) => {
                                        match matrix_event {
                                            bandy::MatrixEvent::IngestTopology { ui_dag: _, semantic_dag } => {
                                                {
                                                    let mut s = state_bg.write().unwrap();
                                                    s.matrix_topology = semantic_dag.clone();

                                                    if let Some(ref mut payload) = s.review_payload {
                                                        // HOT-SWAP: Slice off stale topology
                                                        if let Some(idx) = payload.system.find("--- SEMANTIC CODE TOPOLOGY") {
                                                            payload.system.truncate(idx);
                                                        }
                                                        if let Some(idx) = payload.system.find("--- CURRENT SPATIAL TOPOLOGY") {
                                                            payload.system.truncate(idx);
                                                        }

                                                        let trimmed_dag = semantic_dag.trim_end();
                                                        if !trimmed_dag.is_empty() {
                                                            if !payload.system.ends_with("\n\n") {
                                                                payload.system.push_str("\n\n");
                                                            }
                                                            payload.system.push_str(trimmed_dag);
                                                            payload.system.push_str("\n\n");
                                                        }
                                                    }
                                                }
                                                let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;
                                            }
                                            bandy::MatrixEvent::SectorFocused { target, context } => {
                                                let topology_str = format!("--- CURRENT SPATIAL TOPOLOGY (DAG) ---\nSECTOR: {}\n\n{}", target, context);
                                                {
                                                    let mut s = state_bg.write().unwrap();
                                                    s.matrix_topology = topology_str.clone();

                                                    if let Some(ref mut payload) = s.review_payload {
                                                        // HOT-SWAP: Slice off stale topology
                                                        if let Some(idx) = payload.system.find("--- SEMANTIC CODE TOPOLOGY") {
                                                            payload.system.truncate(idx);
                                                        }
                                                        if let Some(idx) = payload.system.find("--- CURRENT SPATIAL TOPOLOGY") {
                                                            payload.system.truncate(idx);
                                                        }

                                                        if !payload.system.ends_with("\n\n") {
                                                            payload.system.push_str("\n\n");
                                                        }
                                                        payload.system.push_str(&topology_str);
                                                        payload.system.push_str("\n\n");
                                                    }
                                                }
                                                let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;
                                            }
                                            bandy::MatrixEvent::GraftTopology { target_id: _, payload: _ } => {
                                                // Ping the UI to ensure any topological highlights are repainted.
                                                let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;
                                            }
                                            _ => {}
                                        }
                                    }
                                    _ => {}
                                    },
                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                        log::warn!("Synapse receiver lagged, dropping missed events.");
                                        continue;
                                    }
                                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                        log::info!(":: VEIN :: Synapse channel closed, terminating loop.");
                                        break;
                                    }
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

                                if user_input_text.starts_with("LOAD_HISTORY:") {
                                    if let Ok(offset) = user_input_text["LOAD_HISTORY:".len()..].parse::<usize>() {
                                        receipt_counter += 1;
                                        let _ = synapse_loop.fire_async(SMessage::StorageLoadPaged {
                                            receipt_id: receipt_counter,
                                            offset,
                                            limit: 50
                                        }).await;
                                    }
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
                                        Err(_e) => {
                                        }
                                    }
                                    continue;
                                }

                                if user_input_text.starts_with("DISPATCH_PAYLOAD:") {
                                    let payload_str = &user_input_text["DISPATCH_PAYLOAD:".len()..];

                                    if let Ok(payload) = serde_json::from_str::<PreFlightPayload>(payload_str) {

                                        // --- LATE BINDING: SAVE USER HISTORY ONLY ON DISPATCH ---
                                        let parsed_parts_for_save = parse_multimodal_text(&payload.prompt);
                                        let mut clean_memory_text = String::new();
                                        for part in &parsed_parts_for_save {
                                            if let Part::Text { text } = part {
                                                clean_memory_text.push_str(text);
                                            } else {
                                                clean_memory_text.push_str(" [System: User attached a file/image] ");
                                            }
                                        }

                                        let user_embedding = match client.embed_content(&payload.prompt).await {
                                            Ok(vec) => vec,
                                            Err(_) => vec![]
                                        };

                                        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                                        receipt_counter += 1;
                                        let synapse_clone_for_user_save = synapse_loop.clone();
                                        let _ = synapse_clone_for_user_save.fire_async(SMessage::StorageSave {
                                            receipt_id: receipt_counter,
                                            sender: "user".to_string(),
                                            content: clean_memory_text,
                                            timestamp: timestamp.clone(),
                                            embedding: user_embedding,
                                            memory_type: "chat".to_string(),
                                        }).await;
                                        // --------------------------------------------------------

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

                                                {
                                                    let mut s = state_bg.write().unwrap();
                                                    s.console_logs.push_back(display.clone());
                                                    s.console_seq += 1;
                                                    while s.console_logs.len() > MAX_STATE_CAPACITY {
                                                        s.console_logs.pop_front();
                                                    }
                                                    if let Some(meta) = metadata {
                                                        s.token_usage = (
                                                            meta.prompt_token_count.unwrap_or(0) as i32,
                                                            meta.candidates_token_count.unwrap_or(0) as i32,
                                                            meta.total_token_count.unwrap_or(0) as i32
                                                        );
                                                    }
                                                    s.sidebar_status = WolfpackState::Idle;
                                                }
                                                let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;

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
                                                {
                                                    let mut s = state_bg.write().unwrap();
                                                    s.synapse_error = Some(format!("Synapse failure: {}", e));
                                                }
                                                let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;
                                            }
                                        }
                                    } else {
                                        {
                                            let mut s = state_bg.write().unwrap();
                                            s.synapse_error = Some("Failed to deserialize PreFlightPayload".to_string());
                                        }
                                        let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;
                                    }
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
                                        {
                                            let mut s = state_bg.write().unwrap();
                                            s.console_logs.push_back(response_msg);
                                            s.console_seq += 1;
                                            while s.console_logs.len() > MAX_STATE_CAPACITY {
                                                s.console_logs.pop_front();
                                            }
                                        }
                                        let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;
                                    }
                                    continue;
                                }

                                if is_s9 {
                                    {
                                        let mut s = state_bg.write().unwrap();
                                        s.shard_statuses.insert("s9-mule".to_string(), ShardStatus::Thinking);
                                    }
                                    let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;
                                }

                                if user_input_text.starts_with("STORAGE_RESULT:") {
                                    let payload_str = &user_input_text["STORAGE_RESULT:".len()..];
                                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload_str) {
                                        let mut retrieved_context = String::new();
                                        let mut retrieved_directives = String::new();
                                        let mut retrieved_engrams = String::new();
                                        let mut chronological_engrams = String::new();

                                        if let Some(memories) = json.get("memories").and_then(|v| v.as_array()) {
                                            let mems: Vec<String> = memories.iter().map(|m| m.as_str().unwrap_or("").to_string()).collect();
                                            if !mems.is_empty() { retrieved_context = mems.join("\n\n"); }
                                        }

                                        if let Some(directives) = json.get("directives").and_then(|v| v.as_array()) {
                                            let dirs: Vec<String> = directives.iter().map(|d| d.as_str().unwrap_or("").to_string()).collect();
                                            if !dirs.is_empty() { retrieved_directives = dirs.join("\n\n"); }
                                        }

                                        if let Some(engrams) = json.get("engrams").and_then(|v| v.as_array()) {
                                            let engs: Vec<String> = engrams.iter().map(|e| e.as_str().unwrap_or("").to_string()).collect();
                                            if !engs.is_empty() { retrieved_engrams = engs.join("\n\n"); }
                                        }

                                        if let Some(chrono) = json.get("chrono").and_then(|v| v.as_array()) {
                                            let mut chr: Vec<String> = chrono.iter().map(|c| c.as_str().unwrap_or("").to_string()).collect();
                                            if !chr.is_empty() {
                                                chr.reverse();
                                                chronological_engrams = chr.join("\n\n");
                                            }
                                        }

                                        let mut system_builder = if is_s9 {
                                            "You are S9.".to_string()
                                        } else {
                                            "SYSTEM_INSTRUCTION: You are an AI Shard operating within the UnaOS cognitive matrix.".to_string()
                                        };

                                        if !retrieved_directives.is_empty() {
                                            system_builder.push_str("\n\n[ACTIVE DIRECTIVES]:\n");
                                            system_builder.push_str(&retrieved_directives);
                                        }

                                        // Removed AppState live_context parsing since it is complex to extract securely in this scope.

                                        if !retrieved_context.is_empty() {
                                            system_builder.push_str("\n\n[SEMANTIC MEMORY RECALL]:\n");
                                            system_builder.push_str(&retrieved_context);
                                        }

                                        let matrix_topology = {
                                            let s = state_bg.read().unwrap();
                                            s.matrix_topology.clone()
                                        };

                                        if !matrix_topology.is_empty() {
                                            system_builder.push_str("\n\n--- SEMANTIC CODE TOPOLOGY ---\n");
                                            system_builder.push_str(&matrix_topology);
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

                                        {
                                            let mut s = state_bg.write().unwrap();
                                            s.review_payload = Some(pre_flight_payload);
                                        }
                                        let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;
                                    }
                                    continue;
                                }

                                let user_embedding = match client.embed_content(&user_input_text).await {
                                    Ok(vec) => vec,
                                    Err(_e) => vec![]
                                };

                                receipt_counter += 1;
                                let query_receipt_id = receipt_counter;
                                pending_prompts.insert(query_receipt_id, user_input_text.clone());

                                // ONLY query storage to build the pre-flight payload. DO NOT save to history yet.
                                let _ = synapse_loop.fire_async(SMessage::StorageQuery {
                                    receipt_id: query_receipt_id,
                                    embedding: user_embedding,
                                }).await;
                            }
                        }
                    }
                }
                Err(e) => {
                    {
                        let mut s = state_bg.write().unwrap();
                        s.console_logs.push_back(format!(":: FATAL :: {}\n", e));
                        s.console_seq += 1;
                        while s.console_logs.len() > MAX_STATE_CAPACITY {
                            s.console_logs.pop_front();
                        }
                    }
                    let _ = synapse_loop.fire_async(SMessage::StateInvalidated).await;
                }
            }
        });

        (
            Self {
                app_state,
                tx: tx_to_bg,
                synapse,
            },
            brain_loop_handle,
        )
    }

    fn append_to_console(&self, text: &str) {
        {
            let mut s = self.app_state.write().unwrap();
            s.console_logs.push_back(text.to_string());
            s.console_seq += 1;
            while s.console_logs.len() > MAX_STATE_CAPACITY {
                s.console_logs.pop_front();
            }
        }
        self.synapse.fire(SMessage::StateInvalidated);
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
        let mut emit_ping = false;

        match event {
            Event::Input { target: _, text } => {
                let trimmed = text.trim();

                if trimmed == "/wolf" {
                    {
                        let mut s = self.app_state.write().unwrap();
                        s.sidebar_status = WolfpackState::Idle;
                    }
                    self.append_to_console("\n[SYSTEM] :: Switching to Wolfpack Grid...\n");
                    emit_ping = true;
                } else if trimmed == "/comms" {
                    self.append_to_console("\n[SYSTEM] :: Secure Comms Established.\n");
                    emit_ping = true;
                } else if let Some(path_str) = trimmed.strip_prefix("/upload ") {
                    let path = PathBuf::from(path_str.trim());
                    let _ = self.publish("upload", SMessage::TriggerUpload(path));
                } else if let Some(dir_text) = trimmed.strip_prefix("/directive ") {
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
                    self.append_to_console("\n[WOLFPACK] :: Deploying J-Series Unit...\n");
                }
                1 => {
                    self.append_to_console("\n[WOLFPACK] :: Deploying S-Series Unit...\n");
                }
                2 => {
                    self.append_to_console("\n[SYSTEM] :: Returning to Comms.\n");
                }
                _ => {}
            },
            Event::NavSelect(_idx) => {
                self.append_to_console("\n[SYSTEM] :: Nav selection updated.\n");
                emit_ping = true;
            }
            Event::FileSelected(path) => {
                let _ = self.publish("upload", SMessage::TriggerUpload(path.clone()));

                let _ = self.synapse.fire(SMessage::ContextTelemetry {
                    skeletons: vec![],
                });
                emit_ping = true;
            }
            Event::ToggleSidebar => {
                // Assuming side bar toggle logic remains
                emit_ping = true;
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
                let _ = self.tx.send(format!("DISPATCH_PAYLOAD:{}", json_payload));
            }
            Event::LoadHistory { offset } => {
                let _ = self.tx.send(format!("LOAD_HISTORY:{}", offset));
            }
            Event::UpdateMatrixSelection(node_ids) => {
                let relative_targets_str = node_ids.join(" ");
                let _ = self.publish("matrix", SMessage::Matrix(bandy::MatrixEvent::FocusSector(relative_targets_str)));
                emit_ping = true;
            }
            _ => {}
        }

        if emit_ping {
            self.synapse.fire(SMessage::StateInvalidated);
        }
    }

    fn view(&self) -> bandy::state::DashboardState {
        bandy::state::DashboardState::default()
    }
}
