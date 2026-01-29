use dotenvy::dotenv;
use gneiss_pal::persistence::{BrainManager, SavedMessage};
use gneiss_pal::{AppHandler, Backend, DashboardState, Event, SidebarPosition, ViewMode};
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

// NEW: Base64 for Image Encoding
use base64::{Engine as _, engine::general_purpose};

mod api;
use api::{Content, GeminiClient, Part};

mod forge;
use forge::ForgeClient;

struct State {
    mode: ViewMode,
    nav_index: usize,
    chat_history: Vec<SavedMessage>,
    sidebar_position: SidebarPosition,
}

#[derive(Clone)]
struct UiUpdater {
    text_buffer: TextBuffer,
    scroll_adj: Adjustment,
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
            // 1. Read File for S9 Upload AND Vision
            let file_bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx_ui.send(format!("\n[SYSTEM ERROR] :: File Read Failed: {}\n", e));
                    return;
                }
            };

            // 2. ENCODE FOR VISION (The Retina)
            // Encode to Base64
            let b64 = general_purpose::STANDARD.encode(&file_bytes);
            // Assume PNG/JPEG based on extension, or default to png
            let mime = if filename.to_lowercase().ends_with(".jpg") || filename.to_lowercase().ends_with(".jpeg") {
                "image/jpeg"
            } else {
                "image/png"
            };
            let image_data_uri = format!("data:{};base64,{}", mime, b64);

            // Send the image payload to the UI thread (hidden from user view, saved to history)
            let _ = tx_ui.send(format!("[IMAGE_PAYLOAD]{}", image_data_uri));


            // 3. S9 UPLOAD (The Archive)
            let client = reqwest::Client::new();
            let url = "https://vein-s9-upload-1035558613434.us-central1.run.app/upload";

            let part = reqwest::multipart::Part::bytes(file_bytes)
                .file_name(filename.clone())
                .mime_str("application/octet-stream").unwrap();

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
}

impl VeinApp {
    fn new(tx: mpsc::UnboundedSender<String>, state: Arc<Mutex<State>>, ui_updater_rc: Rc<RefCell<Option<UiUpdater>>>, tx_ui: mpsc::UnboundedSender<String>) -> Self {
        Self { state, tx, ui_updater: ui_updater_rc, tx_ui }
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
                } else {
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
                    text_buffer: buffer,
                    scroll_adj: adj,
                });
            }
            Event::FileSelected(path) => {
                trigger_upload(path, self.tx_ui.clone());
            }
            Event::UploadRequest => {}
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
        }
    }
}

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

    if saved_history.is_empty() {
        initial_console_output.push_str(":: MEMORY :: COLD START (New Session)\n\n");
    } else {
        initial_console_output.push_str(":: MEMORY :: LONG-TERM STORAGE RESTORED\n\n");
        for msg in &saved_history {
            if !msg.content.starts_with("SYSTEM_INSTRUCTION") {
                let prefix = if msg.role == "user" { "[ARCHITECT]" } else { "[UNA]" };
                // Hide huge image payloads from initial console load to avoid lag
                if msg.content.starts_with("data:image/") {
                    initial_console_output.push_str(&format!("{} > [IMAGE DATA]\n", prefix));
                } else {
                    initial_console_output.push_str(&format!("{} > {}\n", prefix, msg.content));
                }
            }
        }
    }

    let state = Arc::new(Mutex::new(State {
        mode: ViewMode::Comms,
        nav_index: 0,
        chat_history: saved_history,
        sidebar_position: SidebarPosition::default(),
    }));

    let (tx_to_bg, mut rx_from_ui) = mpsc::unbounded_channel::<String>();
    let (tx_to_ui, mut rx_from_bg) = mpsc::unbounded_channel::<String>();

    let ui_updater_rc = Rc::new(RefCell::new(None::<UiUpdater>));
    let ui_updater_rc_clone_for_app = ui_updater_rc.clone();

    let state_bg = state.clone();
    let brain_bg = brain.clone();
    let tx_to_ui_bg_clone = tx_to_ui.clone();

    thread::spawn(move || {
        let rt = Runtime::new().expect("Failed to create Tokio Runtime");
        rt.block_on(async move {
            info!(":: VEIN :: Brain Connecting...");

            // Initialize Forge (GitHub) Client
            let _forge_client = match ForgeClient::new() {
                Ok(client) => {
                    let _ = tx_to_ui_bg_clone.send(":: FORGE :: CONNECTED (GitHub Integration Active)\n".to_string());
                    Some(client)
                },
                Err(_) => {
                    let _ = tx_to_ui_bg_clone.send(":: FORGE :: OFFLINE (No Token Detected)\n".to_string());
                    None
                }
            };

            let client_res = GeminiClient::new();

            match client_res {
                Ok(client) => {
                    if let Err(e) = tx_to_ui_bg_clone.send(":: BRAIN :: CONNECTION ESTABLISHED.\n\n".to_string()) {
                        error!("Failed to send initial connection message to UI: {}", e);
                    }

                    while let Some(_user_input_text) = rx_from_ui.recv().await {
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

                        let history_snapshot = {
                            let s = state_bg.lock().unwrap();
                            s.chat_history.clone()
                        };

                        // MODIFIED: Parse history to find images
                        for saved in history_snapshot {
                            if saved.content.starts_with("SYSTEM_INSTRUCTION") {
                                continue;
                            }

                            if saved.content.starts_with("data:image/") {
                                 // It's an image! Parse mime and data.
                                 // Format: data:image/png;base64,....
                                 let parts: Vec<&str> = saved.content.split(",").collect();
                                 if parts.len() == 2 {
                                     let meta = parts[0]; // data:image/png;base64
                                     let data = parts[1].to_string();
                                     let mime = meta.replace("data:", "").replace(";base64", "");

                                     context.push(Content {
                                         role: saved.role.clone(),
                                         parts: vec![Part::image(mime, data)]
                                     });
                                 }
                            } else {
                                context.push(Content {
                                    role: saved.role.clone(),
                                    parts: vec![Part::text(saved.content.clone())]
                                });
                            }
                        }

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
                            }
                            Err(e) => {
                                let display_error = format!("\n[SYSTEM ERROR] :: {}\n", e);
                                if let Err(send_e) = tx_to_ui_bg_clone.send(display_error) {
                                    error!("Failed to send API error to UI: {}", send_e);
                                }
                                error!("Gemini API interaction failed: {}", e);
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
    let tx_to_ui_for_app = tx_to_ui.clone();
    let app = VeinApp::new(tx_to_bg, state.clone(), ui_updater_rc_clone_for_app, tx_to_ui_for_app);

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
            // MODIFIED: Handle hidden image payloads
            if message_to_ui.starts_with("[IMAGE_PAYLOAD]") {
                let image_data = message_to_ui.replace("[IMAGE_PAYLOAD]", "");
                let mut s = state_clone_for_bg.lock().unwrap();
                s.chat_history.push(SavedMessage {
                    role: "user".to_string(),
                    content: image_data, // Store raw data for API
                });
                // Do NOT display in console
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

    Backend::new("org.unaos.vein.evolution", app);
    info!("SHUTDOWN: UI Backend runtime complete. Total application runtime: {:?}", app_start_time.elapsed());
}
