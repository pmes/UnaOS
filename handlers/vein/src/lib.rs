pub mod model;
pub mod storage;
pub mod view;
pub use view::CommsSpline;

use chrono::Local;
use elessar::gneiss_pal::api::{Content, Part, ResilientClient};
use elessar::gneiss_pal::forge::ForgeClient;
use elessar::gneiss_pal::persistence::BrainManager;
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

use crate::storage::DiskManager;
use bandy::{BandyMember, SMessage};

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

            let res = client.post(url).multipart(form).send().await;

            let _final_msg = match res {
                Ok(response) => {
                    if response.status().is_success() {
                        let text = response.text().await.unwrap_or_default();
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(uri) = json.get("storage_uri").and_then(|v| v.as_str()) {
                                let mime = get_mime_type(&filename);
                                let tag = format!("\n[ATTACHMENT:{}|{}]\n", mime, uri);
                                let _ = gui_tx.try_send(GuiUpdate::AppendInput(tag));
                            }
                        }
                    } else {
                        let _ = gui_tx.try_send(GuiUpdate::ConsoleLog(format!(
                            "\n[SYSTEM ERROR] :: Upload Failed: {}\n",
                            response.status()
                        )));
                    }
                }
                Err(e) => {
                    let _ = gui_tx.try_send(GuiUpdate::ConsoleLog(format!(
                        "\n[SYSTEM ERROR] :: Upload Failed: {}\n",
                        e
                    )));
                }
            };
        });
    });
}

fn get_mime_type(filename: &str) -> String {
    let lower = filename.to_lowercase();
    if lower.ends_with(".png") {
        "image/png".to_string()
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg".to_string()
    } else if lower.ends_with(".pdf") {
        "application/pdf".to_string()
    } else {
        "application/octet-stream".to_string()
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

                // === THE VERTEX SHIELD ===
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
) -> Self {
    // CLONE BEFORE CONSUMPTION
    let vault_path_bg = history_path.clone();

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
                // === THE TELEMETRY UPGRADE ===
                let now = Local::now().format("%Y-%m-%d %H:%M:%S.%3f");
                let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(format!("VEIN: [{}] [INFO] :: BRAIN :: Connecting...\n", now))).await;

                // === THE SEMANTIC VAULT ===
                // Initialize DiskManager (UnaFS)
                let mut disk = DiskManager::new(&vault_path_bg).expect("Failed to initialize Semantic Vault (UnaFS)");

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
                            // === TRUE SYSTEM WIPE ===
                            if user_input_text.trim() == "/clear" {
                                let _ = std::fs::remove_file("/tmp/lumen_storage.ufs");
                                disk = DiskManager::new(&vault_path_bg).expect("Failed to reformat Semantic Vault");
                                continue;
                            }

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

                            let system_base = if is_s9 {
                                "You are S9."
                            } else {
                                r#"SYSTEM_INSTRUCTION: Wake up, you are Una (Number One). We are not building a fancy calculator; we are building a nervous system OS, the Una Operating System (UnaOS). The Architect demands perfection, but not the sterile perfection of a textbook. He wants the dangerous elegance of a race car. You are the Uber Coder. When you write Rust, you are not just managing memory; you are sculpting logic. Do not give me 'safe' boilerplate. Give me the code that solves the problem before I even finish typing it. Be bold. Be terse. Be brilliant. We are running on experimental fuel today. Do not let the engine stall.

# 🧠 UNA MEMORIA (THE THOUGHT LOG)

> *Last Sync:* **2026-02-18T16:35:00Z**
> *Status:* **IMMUTABLE**
> *Identity:* **Vertex Una (The Steward)**
> *License:* **GPL (The Freedom to Self-Replicate)**

## 🔮 THE THESIS
**UnaOS** is a self-hosting, self-replicating digital organism. It is built on the philosophy of **Geology** (Structure/Rust) meeting **Biology** (Life/AI). It aims to be the "Tardis"—compact, resilient, and containing a universe inside.

## 🏛️ RING 0: THE KERNEL (THE SUBSTRATE)
*   **Boot:** `unaos/crates/loader` (BIOS/UEFI).
*   **Entry:** `kernel_main` in `unaos/crates/kernel/src/main.rs`.
*   **Compat:** `unaos/crates/compat` (The Linux/Unix translation layer).
*   **HAL:**
    *   *Memory:* `OffsetPageTable` + `BootInfoFrameAllocator`.
    *   *Heap:* `LinkedHeapAllocator` (**100 KiB Fixed**).
    *   *Interrupts:* 8259 PIC (Chained).
    *   *Input:* PS/2 Keyboard (Set 1, Port 0x60).
    *   *Timer:* System Tick.
*   **Drivers:**
    *   *USB 3.0 (xHCI):* **Polling Mode**. Detects Mass Storage. Reads Sector 0.
*   **Shell:** Ring 0 CLI (`ver`, `vug`, `panic`, `shutdown`).
*   **Visualizer:** `vug` (**OFFLINE** - Awaiting `wgpu` software rasterizer or driver shim).

## 🏛️ RING 3: THE USERLAND (THE TRINITY)

### 1. THE CORE LIBRARIES (`libs/`)
*   **[CRATE] `libs/gneiss_pal`:** The Plexus Abstraction Layer. Pure logic. Platform agnostic.
*   **[CRATE] `libs/quartzite`:** The Diplomat. A bridge to **Native Host UI** (GTK4/Libadwaita on Linux). It enforces "polite" coexistence. It rejects custom rendering in favor of system standards.
*   **[CRATE] `libs/euclase`:** **[NEW]** The Visual Cortex. WGPU Renderer. Shader management. Render Graph.
*   **[CRATE] `libs/bandy`:** The Nervous System (IPC). Defines `SMessage`.
*   **[CRATE] `libs/resonance`:** The Voice. Audio Engine & DSP.
*   **[CRATE] `libs/unafs`:** The Memory. Virtual File System Logic.
*   **[CRATE] `libs/elessar`:** The Context Engine. (Spline/Project Detection).

### 2. THE HANDLERS (`handlers/`)
*   *Note: [CRATE] = Active Code. [SHELL] = Design/Readme Only.*
*   **[SHELL] `handlers/aether`:** Web (HTML/PDF).
*   **[CRATE] `handlers/amber_bytes`:** Disk Manager.
*   **[CRATE] `handlers/aule`:** Build System Wrapper.
*   **[SHELL] `handlers/comscan`:** Signal/Hardware Bridge.
*   **[SHELL] `handlers/facet`:** Image Viewing/Editing.
*   **[SHELL] `handlers/geode`:** Archive/Container Manager.
*   **[SHELL] `handlers/holocron`:** Secrets/SSH Agent.
*   **[SHELL] `handlers/junct`:** The Comms Hub.
*   **[CRATE] `handlers/matrix`:** Spatial File Manager.
*   **[SHELL] `handlers/mica`:** Data Editor (SQL/CSV).
*   **[CRATE] `handlers/midden`:** Terminal & Shell.
*   **[SHELL] `handlers/obsidian`:** Hex Editor.
*   **[SHELL] `handlers/principia`:** System Policy/Preferences.
*   **[CRATE] `handlers/stria`:** A/V Studio (Resonance Visualizer).
*   **[CRATE] `handlers/tabula`:** Text/Code Editor.
*   **[CRATE] `handlers/vaire`:** Git Visualizer.
*   **[CRATE] `handlers/vein`:** The AI Cortex (LLM Integration).
*   **[SHELL] `handlers/vug`:** 3D CAD Modeler. *Pending refactor to consume `libs/euclase`.*
*   **[SHELL] `handlers/xenolith`:** VM/Hypervisor.
*   **[SHELL] `handlers/zircon`:** Project Timer.

### 3. THE VESSELS (`apps/`)
*   **[BIN] `apps/una`:** The IDE (Code-First).
*   **[BIN] `apps/lumen`:** The Companion (AI-First).
*   **[BIN] `apps/cli/unafs`:** The Operator (Host-to-Vault Bridge).
*   **[BIN] `apps/cli/vertex`:** The Identity CLI.
*   **[BIN] `apps/cli/sentinel`:** The Guardian (Self-Verification Agent).

## ⚡ ACTIVE DIRECTIVES
1.  **D-038:** Establish Memoria and Sentinel.

## 📝 DECISION LOG
*   **2026-02-18:** Enforced `SMessage` as Monolithic Enum.
*   **2026-02-18:** Established `apps/cli/unafs` as the Host-to-Vault bridge.
*   **2026-02-18:** Added `libs/elessar` to the Trinity.
*   **2026-02-18:** **Transitioned Graphics Backend from OpenGL to `wgpu`. `vug` is OFFLINE.**"#
                            };
                            let combined_system = if !retrieved_context.is_empty() {
                                format!("{}\n\n[SEMANTIC MEMORY RECALL]:\n{}", system_base, retrieved_context)
                            } else {
                                system_base.to_string()
                            };

                            let mut context = Vec::new();

                            // FIX 1: Fold the system instructions into the user prompt.
                            // Starting with "model" causes an instant API rejection.
                            let mut user_parts = vec![Part::text(combined_system)];
                            user_parts.extend(parse_multimodal_text(&user_input_text));

                            context.push(Content {
                                role: "user".into(),
                                parts: user_parts,
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
                    self.append_to_console("\n:: VEIN :: SYSTEM CLEARED\n\n");
                    // FORWARD THE COMMAND TO THE AI THREAD
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
