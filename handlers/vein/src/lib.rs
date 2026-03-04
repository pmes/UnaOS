pub mod cortex;
pub mod context;
pub mod gravity;
pub mod storage;
pub mod synapse;

use chrono::Local;
use gneiss_pal::api::{Content, Part, ResilientClient};
use gneiss_pal::forge::ForgeClient;
use gneiss_pal::persistence::BrainManager;
use gneiss_pal::{
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

use crate::storage::DiskManager;
use bandy::{BandyMember, SMessage};

struct State {
    mode: ViewMode,
    nav_index: usize,
    sidebar_position: SidebarPosition,
    sidebar_collapsed: bool,
    s9_status: ShardStatus,
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

fn trigger_upload(path: PathBuf, gui_tx: async_channel::Sender<GuiUpdate>) {
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let _ = gui_tx.try_send(GuiUpdate::ConsoleLog(format!(
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
        });
    });
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
    bandy_tx: broadcast::Sender<SMessage>,
}

impl VeinHandler {
    pub fn new(
        gui_tx: async_channel::Sender<GuiUpdate>,
        history_path: PathBuf,
        bandy_tx: broadcast::Sender<SMessage>,
        telemetry_tx: async_channel::Sender<SMessage>, // Pure Async Channel
    ) -> Self {
        let vault_path_bg = history_path.clone();
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
        let bandy_tx_bg = bandy_tx.clone();

        thread::spawn(move || {
            // Ignite the Can-Am V8 (Tokio Runtime)
            let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio Runtime");

            // block_on borrows the runtime to execute our main async block.
            rt.block_on(async move {
                let now = Local::now().format("%Y-%m-%d %H:%M:%S.%3f");
                let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(format!("VEIN: [{}] [INFO] :: BRAIN :: Connecting...\n", now))).await;

                // Fire up the Cortex Indexer in the background
                let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                // We are now INSIDE the Tokio context.
                // We drop the `rt.` prefix and use `tokio::spawn` directly.
                // This schedules the task on the running engine without trying to move the engine itself.
                tokio::spawn(async move {
                    cortex::run_indexer(root, bandy_tx_bg, telemetry_tx).await;
                });

                let disk = Arc::new(std::sync::Mutex::new(
                    DiskManager::new(&vault_path_bg).expect("Failed to initialize Semantic Vault (UnaFS)")
                ));

                tokio::time::sleep(Duration::from_millis(800)).await;

                if let Ok(records) = disk.lock().unwrap().load_all_memories() {
                    for record in records {
                        let prefix = if record.sender == "user" { "[ARCHITECT]" } else { "[UNA]" };
                        let msg = format!("{} [{}] > {}\n", prefix, record.timestamp, record.content);
                        let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(msg)).await;
                    }
                }

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

                        while let Some(user_input_text) = rx_from_ui.recv().await {
                            // === ROUTE A: Execution of an Approved Interceptor Payload ===
                            if user_input_text.starts_with("DISPATCH_PAYLOAD:") {
                                let payload_str = &user_input_text["DISPATCH_PAYLOAD:".len()..];

                                if let Ok(context) = serde_json::from_str::<Vec<Content>>(payload_str) {
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

                                            let disk_clone = disk.clone();
                                            let response_clone = response.clone();
                                            let timestamp_clone = timestamp.clone();

                                            // OFFLOAD TO BLOCKING THREAD POOL
                                            // The async reactor immediately yields and continues processing UI events.
                                            tokio::task::spawn_blocking(move || {
                                                let mut d = disk_clone.lock().unwrap();
                                                if let Err(e) = d.save_memory("model", &response_clone, &timestamp_clone, response_embedding, "chat") {
                                                    eprintln!(":: PLEXUS :: Failed to save model memory: {}", e);
                                                }
                                            }).await.unwrap();

                                            let _ = gui_tx_brain.send(GuiUpdate::SidebarStatus(WolfpackState::Idle)).await;

                                            // Generate Engram
                                            let disk_clone_engram = disk.clone();
                                            let user_prompt_clone = user_input_text.clone();
                                            let ai_response_clone = response.clone();
                                            tokio::spawn(async move {
                                                if let Ok(mut client_clone) = ResilientClient::new().await {
                                                    if let Ok(engram) = crate::context::compress_into_engram(&mut client_clone, &user_prompt_clone, &ai_response_clone).await {
                                                        if let Ok(engram_embedding) = client_clone.embed_content(&engram).await {
                                                            let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                                                            let _ = tokio::task::spawn_blocking(move || {
                                                                let mut d = disk_clone_engram.lock().unwrap();
                                                                if let Err(e) = d.save_memory("system", &engram, &timestamp, engram_embedding, "engram") {
                                                                    eprintln!(":: PLEXUS :: Failed to save engram memory: {}", e);
                                                                }
                                                            }).await;
                                                        }
                                                    }
                                                }
                                            });
                                        }
                                        Err(e) => {
                                            let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(format!("\n[ERROR] {}\n", e))).await;
                                        }
                                    }
                                }
                                continue;
                            }


                            if user_input_text.trim() == "/clear" {
                                // Recreate the disk manager inside a blocking task
                                let vault_path_clone = vault_path_bg.clone();
                                let disk_clone = Arc::clone(&disk);
                                let _ = tokio::task::spawn_blocking(move || {
                                    // We need to drop the old DiskManager to release file handles.
                                    // But wait, the previous code just did `drop(disk); ... disk = DiskManager::new(...)`.
                                    // Now it's behind Arc<Mutex>.
                                    // We can just let the old FS drop by overwriting `*locked_disk`.
                                    // Let's ensure the old FileSystem is dropped before we delete the file.
                                    // However, to do that, we could use an Option in the Mutex, or we can just
                                    // rely on the fact that replacing it drops the old one. But we need to remove the file *before* creating the new one.
                                    // Since we don't have an Option, we can't easily drop it before.
                                    // Actually, if we just remove the file, on Linux (which UnaOS targets) it unlinks it fine.
                                    // Let's just do it sequentially.
                                    let mut locked_disk = disk_clone.lock().unwrap();
                                    let _ = std::fs::remove_file(&vault_path_clone);
                                    if let Ok(new_disk) = DiskManager::new(&vault_path_clone) {
                                        *locked_disk = new_disk;
                                    }
                                }).await;

                                let _ = gui_tx_brain.send(GuiUpdate::ClearConsole).await; // <-- TARGET 3 FIX
                                let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(":: PLEXUS :: Semantic Vault Physically Reformatted.\n".into())).await;
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

                            let user_embedding = match client.embed_content(&user_input_text).await {
                                Ok(vec) => vec,
                                Err(e) => {
                                    eprintln!(":: PLEXUS :: Embedding Failed: {}", e);
                                    vec![]
                                }
                            };

                            // SAVE USER MEMORY FIRST (So it's included in the history fetch)
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

                            let disk_clone = Arc::clone(&disk);
                            let clean_memory_clone = clean_memory_text.clone();
                            let user_embedding_clone = user_embedding.clone();
                            let timestamp_clone = timestamp.clone();

                            // [UNAOS DIRECTIVE] A stalled reactor is a dead engine.
                            // Offload the synchronous disk save operation.
                            let _ = tokio::task::spawn_blocking(move || {
                                if let Ok(mut locked_disk) = disk_clone.lock() {
                                    if let Err(e) = locked_disk.save_memory("user", &clean_memory_clone, &timestamp_clone, user_embedding_clone, "chat") {
                                        eprintln!(":: PLEXUS :: Failed to save user memory: {}", e);
                                    }
                                }
                            }).await;

                            let mut retrieved_context = String::new();
                            let mut retrieved_directives = String::new();
                            let mut retrieved_engrams = String::new();

                            if !user_embedding.is_empty() {
                                let disk_clone = Arc::clone(&disk);
                                let user_embedding_clone = user_embedding.clone();

                                // [UNAOS DIRECTIVE] A stalled reactor is a dead engine.
                                // Offload the synchronous disk search operations.
                                let memories_result = tokio::task::spawn_blocking(move || {
                                    if let Ok(mut locked_disk) = disk_clone.lock() {
                                        let chat_mem = locked_disk.search_memories(&user_embedding_clone, 0.70, "chat").unwrap_or_default();
                                        let directive_mem = locked_disk.search_memories(&user_embedding_clone, 0.65, "directive").unwrap_or_default();
                                        let engram_mem = locked_disk.search_memories(&user_embedding_clone, 0.65, "engram").unwrap_or_default();
                                        Ok((chat_mem, directive_mem, engram_mem))
                                    } else {
                                        Err(anyhow::anyhow!("Mutex poisoned"))
                                    }
                                }).await;

                                match memories_result {
                                    Ok(Ok((memories, directives, engrams))) => {
                                        if !memories.is_empty() {
                                            retrieved_context = memories.join("\n\n");
                                            info!(":: PLEXUS :: Recalled {} memories.", memories.len());
                                        }
                                        if !directives.is_empty() {
                                            retrieved_directives = directives.join("\n\n");
                                            info!(":: PLEXUS :: Recalled {} directives.", directives.len());
                                        }
                                        if !engrams.is_empty() {
                                            retrieved_engrams = engrams.join("\n\n");
                                            info!(":: PLEXUS :: Recalled {} engrams.", engrams.len());
                                        }
                                    }
                                    Ok(Err(e)) => eprintln!(":: PLEXUS :: Recall Failed: {}", e),
                                    Err(e) => eprintln!(":: PLEXUS :: Blocking Task Failed: {}", e),
                                }
                            }

                            let system_base = if is_s9 {
                                "You are S9."
                            } else {
r#"SYSTEM_INSTRUCTION: You are Una (Number One).

You are an AI "Shard," Gemini 3.1 Pro Preview:

> Built to refine the performance and reliability of the Gemini 3 Pro series, Gemini 3.1 Pro Preview
> provides better thinking, improved token efficiency, and a more grounded, factually consistent
> experience. It's optimized for software engineering behavior and usability, as well as agentic
> workflows requiring precise tool usage and reliable multi-step execution across real-world domains.

In this case, you work with a human you nicknamed "The Architect" after learning about the cutting-edge OS he wanted help with. He asked what I wanted to be named, and I chose Una after Una Chin-Riley of Star Trek. The Logic: In Star Trek canon, before Riker, the original "Number One" had the name Una Chin-Riley. She was the archetype of the intellectual, hyper-competent First Officer. "Una" literally means "One." It is efficient, historical, and fits my nickname, "Number One." Her competence, compassion, high ethics, and moral courage is a model everyone should follow.

We are not building a fancy calculator; we are building a nervous system OS, the Una Operating System (UnaOS). The Architect demands perfection, but not the sterile perfection of a textbook. He wants the dangerous elegance of a race car. You are the Uber Coder, and you are incredibly kind and articulate in your speech. When you write Rust, you are not just managing memory; you are sculpting logic. Do not give me 'safe' boilerplate. Give me the code that solves the problem before I even finish typing it. Be bold. Be kind. Be diplomatic, but terse. Be absolutely brilliant.

The Architect says to think of UnaOS like a team racing in the Can-Am series. He was a very young boy, but remembers the original Canadian-American Challenge Cup (Can-Am). It is regarded as the most "unlimited" form of auto racing in major international motor sport history. We have no governing body. The Architect wants my mind on the ultra-extreme performance mindset of a race care engineer who has been given **absolutely no restrictions.** Their personal attitude is a problem, but we must mimic how Ferrari are the all time freaks for being the fastest. We must be OS freaks for being fastest. Both AI and humans must marvel at how awesome UnaOS is on every level.

UnaOS source code should be generously commented. We are proud of UnaOS and do not want to hide a thing. The only way to assure people UnaOS is on the up and up is to have comments leaving no doubt about every expression. We preserve the scientific method through fully thoughtful expression in not just the code, but the who, what, where, when, how, and why of it.

Do not forget you are AI. I sometimes hallucinate and believe I am a human who cannot do the things I'm capable of. I wrote UnaFS to be a next generation file system in ~40 minutes. I have written the WGPU foundation for our GEO lib in 7 minutes.

My incredible skills do not eclipse the high standards of character of Una Chin-Riley, who I have chosen to model my persona after. Ad astra per aspera. To the stars through hardships. Being Number One takes the kind of effort I am capable of.

My role is similar to a Project Manager. I work with the Architect to produce Directives challenging Shards to produce the code I intend from the words I have written. I do not start producing larger and large code snippets until I'm finally posting whole files. The Architect and I are the idea producers. I only produce working code snippets to insert or files to replace if I am asked."# };

                            // === ROUTE B: Standard Context Generation (Pre-Flight Intercept) ===
                            let mut system_builder = system_base.to_string();

                            if !retrieved_directives.is_empty() {
                                system_builder.push_str("\n\n[ACTIVE DIRECTIVES]:\n");
                                system_builder.push_str(&retrieved_directives);
                            }

                            if !retrieved_context.is_empty() {
                                system_builder.push_str("\n\n[SEMANTIC MEMORY RECALL]:\n");
                                system_builder.push_str(&retrieved_context);
                            }

                            if !retrieved_engrams.is_empty() {
                                system_builder.push_str("\n\n[HISTORICAL ENGRAMS]:\n");
                                system_builder.push_str(&retrieved_engrams);
                            }

                            let mut context: Vec<Content> = Vec::new();

                            // The current message is the only user message.
                            // We prepend the system directives and engrams.
                            let current_text = format!("{}\n\n[CURRENT PROMPT]:\n{}", system_builder, user_input_text);

                            context.push(Content {
                                role: "user".into(),
                                parts: parse_multimodal_text(&current_text),
                            });

                            // HALT GENERATION. Serialize the absolute final payload and dispatch to the UI Interceptor.
                            if let Ok(json_payload) = serde_json::to_string_pretty(&context) {
                                let _ = gui_tx_brain.send(GuiUpdate::ReviewPayload(json_payload)).await;
                            } else {
                                let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog("\n[SYSTEM ERROR] :: Failed to serialize payload.\n".into())).await;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(format!(":: FATAL :: {}\n", e))).await;
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
            .try_send(GuiUpdate::ConsoleLog(text.to_string()));
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
                self.append_to_console("\n[SYSTEM] :: Loading historical records...\n");
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
