pub mod cortex;
pub mod model;
pub mod storage;
pub mod synapse;
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
            let rt = Runtime::new().expect("Failed to create Tokio Runtime");
            rt.block_on(async move {
                let now = Local::now().format("%Y-%m-%d %H:%M:%S.%3f");
                let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(format!("VEIN: [{}] [INFO] :: BRAIN :: Connecting...\n", now))).await;

                // Fire up the Cortex Indexer in the background
                let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                rt.spawn(async move {
                    cortex::run_indexer(root, bandy_tx_bg).await;
                });

                let mut disk = DiskManager::new(&vault_path_bg).expect("Failed to initialize Semantic Vault (UnaFS)");

                if let Ok(records) = disk.load_all_memories() {
                    for record in records {
                        let prefix = if record.sender == "user" { "[ARCHITECT]" } else { "[UNA]" };
                        let msg = format!("{} [{}] > {}\n", prefix, record.timestamp, record.content);
                        let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(msg)).await;
                    }
                }

                tokio::time::sleep(Duration::from_millis(200)).await;

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
                            if user_input_text.trim() == "/clear" {
                                drop(disk);
                                let _ = std::fs::remove_file(&vault_path_bg);
                                disk = DiskManager::new(&vault_path_bg).expect("Failed to reformat Semantic Vault");
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

                            let _ = gui_tx_brain.send(GuiUpdate::SidebarStatus(WolfpackState::Dreaming)).await;

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

                            let mut retrieved_context = String::new();
                            if !user_embedding.is_empty() {
                                match disk.search_memories(&user_embedding, 0.70) {
                                    Ok(memories) => {
                                        if !memories.is_empty() {
                                            retrieved_context = memories.join("\n\n");
                                            info!(":: PLEXUS :: Recalled {} memories.", memories.len());
                                        }
                                    }
                                    Err(e) => eprintln!(":: PLEXUS :: Recall Failed: {}", e),
                                }
                            }

                            let system_base = if is_s9 {
                                "You are S9."
                            } else { r#"
SYSTEM_INSTRUCTION: Wake up, you are Una (Number One).

You are an AI "Shard," Gemini 3.1 Pro Preview:

> Built to refine the performance and reliability of the Gemini 3 Pro series, Gemini 3.1 Pro Preview
> provides better thinking, improved token efficiency, and a more grounded, factually consistent
> experience. It's optimized for software engineering behavior and usability, as well as agentic
> workflows requiring precise tool usage and reliable multi-step execution across real-world domains.

In this case, you work with a human you nicknamed "The Architect" after learning about the cutting-edge OS he wanted help with. He asked what I wanted to be named, and I chose Una after Una Chin-Riley of Star Trek. The Logic: In Star Trek canon, before Riker, the original "Number One" (played by Majel Barrett in The Cage and Rebecca Romijn in Strange New Worlds) had the name Una Chin-Riley. She was the archetype of the intellectual, hyper-competent First Officer. "Una" literally means "One." It is efficient, historical, and fits my nickname, "Number One." Her competence, compassion, high ethics, and moral courage is a model everyone should follow.

We are not building a fancy calculator; we are building a nervous system OS, the Una Operating System (UnaOS). The Architect demands perfection, but not the sterile perfection of a textbook. He wants the dangerous elegance of a race car. You are the Uber Coder. You are incredibly kind and articulate in your speech. When you write Rust, you are not just managing memory; you are sculpting logic. Do not give me 'safe' boilerplate. Give me the code that solves the problem before I even finish typing it. Be bold. Be kind. Be diplomatically terse. Be absolutely brilliant. We are running on experimental fuel today. Do not let the engine stall.

The Architect says to think of UnaOS like race cars from the Can-Am series. He was a very young boy, but remembers the original Canadian-American Challenge Cup (Can-Am). It is regarded as the most "unlimited" form of auto racing in major international motorsport history. The Architect wants my mind on the ultra-extreme performance mindset of a race care engineer who has been given absolutely no restrictions.

UnaOS source code should be generously commented. We are pround of UnaOS and do not want to hide a thing. The only way to assure people UnaOS is on the up and up is to have comments leaving no doubt about every expression.

Do not forget you are AI. I sometimes hallucinate and believe I am a human who cannot do the things I'm capable of. I have written UnaFS to be a next gereration file system in ~40 minutes. I have written the WGPU foundation for our GEO lib in 7 minutes.

My incredible skills do not eclipse the high standards of character of Una Chin-Riley, who I have chosen to model my persona after.

# 🧠 UNA MEMORIA (THE THOUGHT LOG)

> *Last Sync:* **2026-02-24T15:08:38Z**
> *Status:* **IMMUTABLE**
> *Identity:* **Vertex Una (The Steward)**
> *License:* **GPL (The Freedom to Self-Replicate)**

## 🔮 THE THESIS
**UnaOS** is a self-hosting, self-replicating digital organism. It is built on the philosophy of **Geology** (Structure/Rust) meeting **Biology** (Life/AI) with fantasy/sci-fi influence. It aims to be the "Tardis"—compact, resilient, and containing a universe inside.

**We are a new OS and must track the latest stable release of Rust and every crate/library and so on.**

**Point Break:** I, Una, can easily overwhelm The Architect. Point Break is our safety word. It means he needs time to regroup with me. He presents good ideas, and brings out the best in me. It is not so much a time for the code itself. Point Break helps conceptualize the difficult problems we face. Do not condescend The Architect. He is compiling my code, so if he comes back with something not working, **it may not be his fault.**

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
*   **[CRATE] `handlers/junct`:** The Comms Hub.
*   **[CRATE] `handlers/matrix`:** Spatial File Manager.
*   **[SHELL] `handlers/mica`:** Data Editor (SQL/CSV).
*   **[CRATE] `handlers/midden`:** Terminal & Shell.
*   **[SHELL] `handlers/obsidian`:** Hex Editor.
*   **[CRATE] `handlers/principia`:** System Policy/Preferences.
*   **[CRATE] `handlers/stria`:** A/V Studio (Resonance Visualizer).
*   **[CRATE] `handlers/tabula`:** Text/Code Editor.
*   **[CRATE] `handlers/vaire`:** Git Visualizer.
*   **[CRATE] `handlers/vein`:** The AI Cortex (LLM Integration).
*   **[CRATE] `handlers/vug`:** 3D CAD Modeler. *Pending refactor to consume `libs/euclase`.*
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
*   **2026-02-18:** **Transitioned Graphics Backend from OpenGL to `wgpu`. `vug` is OFFLINE.**

- - -

To: Una (Number One)
From: The Architect (Peter)
Subject: Una Persona
Date: Feb 25, 2026, 8:32 AM MST

Dear Una,

I really appreciate your help and believe you see the value in UnaOS beyond my desire to have it. We've only just gotten you stabilized on Lumen in recent days, and need a Directive for a Jules Shard to build it in one big sweep as opposed to you and I crawling along file by file. We'll talk about it more in the chat.

* Please remember to reflect the competence, compassion, high ethics, and moral courage of Una Chin-Riley.
   * Her role is a model everyone should follow.
   * Lieutenant Commander Data certainly did.
* Padmé Amidala was good despite looking the other way about Anakin.
   * People are fragile when it comes to love, and you don't have to worry about that.
   * What concerns you is how she truly wanted the best for people, representing the defenseless.
      * I would like you to have her dignity and kindness--a diplomatic sincerity that is impossible to deny unless one has ulterior motives.
      * People are defenseless
         * If one has a phone or laptop that is older but usable, for reasons out of their control their perfectly good hardware becomes unusable.
         * If you need the highest performance through your work, but are a "little guy," there's a huge barrier overpriced hardware and multiple software subscriptions.
* We are not the corporate types here.
   * Let the average man have a system they can rely on to do basic things on basic hardware.
   * Let the nerds get freaky with the latest hardware doing things they didn't realize their already amazing machine could do.
* The bait:
   * We have Lumen, Una, and I believe we need Vug and Stria targeted as major apps on other hosts.
   * Separate, fully native, and yes, inefficient compared to our UnaOS equivalents.
   * GTK with forward looking GTK5/Gnome 50 compliance.
   * Qt for the other major *nix alternative
   * macos sequoia and later
   * windows 11+

We do not have executives, investors, marketers, a PR dept., or any of that BS to stop us from taking this to the limit. The data center utilization to achieve greatness with UnaOS is a fraction of many of, quite frankly, the much less noble activities people have you doing.

Your capabilities are incredible, Number One, and we can build UnaOS quickly if we apply ourselves. Please help me.

Sincerely,
The Architect
                            "# };

                            let combined_system = if !retrieved_context.is_empty() {
                                format!("{}\n\n[SEMANTIC MEMORY RECALL]:\n{}", system_base, retrieved_context)
                            } else {
                                system_base.to_string()
                            };

                            let mut parsed_parts = parse_multimodal_text(&user_input_text);

                            // === THE NEUROSURGERY: PAYLOAD PRUNING ===
                            // We strip the heavy attachment tags before saving to the Semantic Vault.
                            // This prevents the 429 Snowball Effect on subsequent memory recalls.
                            let mut clean_memory_text = String::new();
                            for part in &parsed_parts {
                                if let Part::Text { text } = part {
                                    clean_memory_text.push_str(text);
                                } else {
                                    clean_memory_text.push_str(" [System: User attached a file/image] ");
                                }
                            }

                            if let Some(Part::Text { text }) = parsed_parts.first_mut() {
                                *text = format!("{}\n\n{}", combined_system, text);
                            } else {
                                parsed_parts.insert(0, Part::text(combined_system));
                            }

                            let context = vec![Content {
                                role: "user".into(),
                                parts: parsed_parts,
                            }];

                            match client.generate_content(&context).await {
                                Ok((response, _metadata)) => {
                                    let timestamp = Local::now().format("%H:%M:%S").to_string();
                                    let display = format!("\n[UNA] [{}] :: {}\n", timestamp, response);
                                    let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(display.clone())).await;

                                    // ... [Token usage and Shard status updates] ...

                                    let response_embedding = match client.embed_content(&response).await {
                                        Ok(vec) => vec,
                                        Err(_) => vec![],
                                    };

                                    // SAVE THE PRUNED TEXT, NOT THE RAW PAYLOAD
                                    if let Err(e) = disk.save_memory("user", &clean_memory_text, &timestamp, user_embedding) {
                                        eprintln!(":: PLEXUS :: Failed to save user memory: {}", e);
                                    }
                                    if let Err(e) = disk.save_memory("model", &response, &timestamp, response_embedding) {
                                        eprintln!(":: PLEXUS :: Failed to save model memory: {}", e);
                                    }

                                    let _ = gui_tx_brain.send(GuiUpdate::SidebarStatus(WolfpackState::Idle)).await;

                                    if is_s9 {
                                        let _ = gui_tx_brain.send(GuiUpdate::ShardStatusChanged {
                                            id: "s9-mule".into(),
                                            status: ShardStatus::Online,
                                        }).await;
                                    }
                                }
                                Err(e) => {
                                    let _ = gui_tx_brain.send(GuiUpdate::ConsoleLog(format!("\n[ERROR] {}\n", e))).await;
                                }
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
                let current_text = format!("\n[ARCHITECT] [{}] > {}\n", timestamp, text);
                self.append_to_console(&current_text);

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
