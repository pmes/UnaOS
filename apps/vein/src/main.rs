use dotenvy::dotenv;
use gneiss_pal::persistence::{BrainManager, SavedMessage};
use gneiss_pal::{AppHandler, Backend, DashboardState, Event, SidebarPosition, ViewMode};
use std::sync::{Arc, Mutex};
use std::thread;
use std::sync::mpsc::channel;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use log::{info, error};
use std::time::Instant;
use std::io::Write; // For stdout/stderr flush

mod api;
use api::{Content, GeminiClient, Part};

// Shared State between UI (Sync) and Backend (Async)
struct State {
    console_output: String,
    mode: ViewMode,
    nav_index: usize,
    chat_history: Vec<SavedMessage>, // Structured history for persistence
    sidebar_position: SidebarPosition, // UI state for sidebar position
}

struct VeinApp {
    state: Arc<Mutex<State>>,
    tx: mpsc::UnboundedSender<String>,
}

impl VeinApp {
    fn new(tx: mpsc::UnboundedSender<String>, state: Arc<Mutex<State>>) -> Self {
        Self { state, tx }
    }
}

impl AppHandler for VeinApp {
    fn handle_event(&mut self, event: Event) {
        let mut s = self.state.lock().unwrap(); // Lock state once for efficiency

        match event {
            Event::Input(text) => {
                s.console_output
                    .push_str(&format!("\n[ARCHITECT] > {}\n", text));
                s.chat_history.push(SavedMessage {
                    role: "user".to_string(),
                    content: text.clone(),
                });

                if text.trim() == "/wolf" {
                    s.mode = ViewMode::Wolfpack;
                    s.console_output
                        .push_str("\n[SYSTEM] :: Switching to Wolfpack Grid...\n");
                    return;
                } else if text.trim() == "/comms" {
                    s.mode = ViewMode::Comms;
                    s.console_output
                        .push_str("\n[SYSTEM] :: Secure Comms Established.\n");
                    return;
                } else if text.trim() == "/clear" {
                    s.console_output.clear();
                    s.console_output.push_str(":: VEIN :: SYSTEM CLEARED\n\n");
                    return;
                }
                else if text.trim() == "/sidebar_left" {
                    s.sidebar_position = SidebarPosition::Left;
                    s.console_output.push_str("\n[SYSTEM] :: Sidebar moved to left.\n");
                    return;
                }
                else if text.trim() == "/sidebar_right" {
                    s.sidebar_position = SidebarPosition::Right;
                    s.console_output.push_str("\n[SYSTEM] :: Sidebar moved to right.\n");
                    return;
                }

                if let Err(e) = self.tx.send(text) {
                    s.console_output
                        .push_str(&format!("\n[SYSTEM ERROR] :: Failed to send: {}\n", e));
                }
            }
            Event::TemplateAction(idx) => {
                match idx {
                    0 => {
                        if s.mode == ViewMode::Comms {
                            s.console_output.clear();
                            s.console_output.push_str(":: VEIN :: SYSTEM CLEARED\n\n");
                        } else {
                            s.console_output
                                .push_str("\n[WOLFPACK] :: Deploying J-Series Unit...\n");
                        }
                    }
                    1 => {
                        if s.mode == ViewMode::Comms {
                            s.mode = ViewMode::Wolfpack;
                            s.console_output
                                .push_str("\n[SYSTEM] :: Switching to Wolfpack Grid...\n");
                        } else {
                            s.console_output
                                .push_str("\n[WOLFPACK] :: Deploying S-Series Unit...\n");
                        }
                    }
                    2 => {
                        if s.mode == ViewMode::Wolfpack {
                            s.mode = ViewMode::Comms;
                            s.console_output
                                .push_str("\n[SYSTEM] :: Returning to Comms.\n");
                        }
                    }
                    _ => {}
                }
            }
            Event::NavSelect(idx) => {
                s.nav_index = idx;
                s.console_output.push_str(&format!("\n[SYSTEM] :: Switched to navigation item at index {}\n", idx));
            }
            Event::DockAction(idx) => {
                match idx {
                    0 => {
                        s.console_output.push_str("\n[SYSTEM] :: Dock: Rooms selected.\n");
                        s.nav_index = 0;
                    }
                    1 => {
                        s.console_output.push_str("\n[SYSTEM] :: Dock: Status selected.\n");
                    }
                    _ => s.console_output.push_str(&format!("\n[SYSTEM] :: Dock: Unknown action {}\n", idx)),
                }
            }
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
            console_output: s.console_output.clone(),
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
        }
    }
}

fn main() {
    let app_start_time = Instant::now(); // Renamed for clarity: marks true application start
    dotenv().ok();

    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .try_init()
        .ok();

    info!("STARTUP: Initializing environment and logger. Elapsed: {:?}", app_start_time.elapsed());
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    info!(":: VEIN :: Booting (The Great Evolution)...");

    let brain = BrainManager::new();

    // --- Persistence Actor Setup ---
    let (save_tx, save_rx) = channel::<Vec<SavedMessage>>();
    let brain_actor = brain.clone();

    // Spawn the Persistence Actor (dedicated thread for file I/O)
    thread::spawn(move || {
        info!("PERSISTENCE: Actor thread started.");
        while let Ok(history) = save_rx.recv() {
            // This is a blocking file I/O operation, but it happens in its own thread.
            // It does NOT hold the State mutex.
            brain_actor.save(&history);
        }
        info!("PERSISTENCE: Actor thread shutting down.");
    });


    // --- State Initialization (Cold Start) ---
    // We start with an empty state and "Initializing" message to show the window immediately.
    let console_output = ":: VEIN :: SYSTEM :: INITIALIZING MEMORY...\n".to_string();

    let state = Arc::new(Mutex::new(State {
        console_output,
        mode: ViewMode::Comms,
        nav_index: 0,
        chat_history: Vec::new(),
        sidebar_position: SidebarPosition::default(),
    }));

    // --- Background History Loader ---
    let brain_loader = brain.clone();
    let state_loader = state.clone();
    let app_start_time_clone = app_start_time; // Copy for closure

    thread::spawn(move || {
        let load_start = Instant::now();
        info!("LOADER: Starting history load...");
        let mut loaded_history = brain_loader.load();
        info!("LOADER: History loaded from disk in {:?}. Acquiring state lock...", load_start.elapsed());

        let mut s = state_loader.lock().unwrap();

        // Prepend loaded history to any new messages typed during load
        // Note: s.chat_history might contain new user messages if they typed fast.
        // We want: [Old History] + [New Messages]
        // Vec::append moves elements from `other` to `self`.
        // So we append `s.chat_history` (new stuff) to `loaded_history` (old stuff),
        // then swap them.
        loaded_history.append(&mut s.chat_history);
        s.chat_history = loaded_history;

        // Rebuild Console Output
        let mut new_console = ":: VEIN :: SYSTEM ONLINE (UNLIMITED TIER)\n:: ENGINE: GEMINI-3-PRO\n".to_string();

        if s.chat_history.is_empty() {
             new_console.push_str(":: MEMORY :: COLD START (New Session)\n\n");
        } else {
             new_console.push_str(":: MEMORY :: LONG-TERM STORAGE RESTORED\n\n");
             for msg in &s.chat_history {
                if !msg.content.starts_with("SYSTEM_INSTRUCTION") {
                    let prefix = if msg.role == "user" {
                        "[ARCHITECT]"
                    } else {
                        "[UNA]"
                    };
                    new_console.push_str(&format!("{} > {}\n", prefix, msg.content));
                }
            }
        }

        s.console_output = new_console;

        info!(
            "LOADER: State updated with {} items. Total startup time: {:?}",
            s.chat_history.len(), app_start_time_clone.elapsed()
        );
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let state_bg = state.clone();

    thread::spawn(move || {
        let rt_spawn_time = Instant::now();
        let rt = Runtime::new().expect("Failed to create Tokio Runtime");
        info!("STARTUP: Tokio Runtime created in background thread. Elapsed: {:?}", rt_spawn_time.elapsed());
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();

        rt.block_on(async {
            info!(":: VEIN :: Brain Connecting...");
            let client_init_time = Instant::now();
            let client_res = GeminiClient::new();
            info!("STARTUP: GeminiClient.new() finished. Elapsed: {:?}", client_init_time.elapsed());
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().flush();

            match client_res {
                Ok(client) => {
                    {
                        let mut s = state_bg.lock().unwrap();
                        s.console_output.push_str(":: BRAIN :: CONNECTION ESTABLISHED.\n\n");
                    }

                    while let Some(msg) = rx.recv().await {
                        println!("DEBUG: Processing input: '{}'", msg); // <--- LOG 1: Confirm Input Receipt

                        // 2. PERSIST (Async - Fire and Forget)
                        // Note: User message is already added to state in handle_event (UI thread).
                        {
                            let s = state_bg.lock().unwrap();
                            // Clone history while holding lock (fast memory copy)
                            // Send to actor (instant channel send)
                            // This replaces the blocking brain_bg.save() call.
                            let _ = save_tx.send(s.chat_history.clone());
                        }

                        let mut context = Vec::new();
                        let system_instruction = r#"SYSTEM_INSTRUCTION: You are Una ("Number One"), Chief Technical Officer for the Una Operating System. Vein is one of the core apps, your window to the world. It's a specialized Command Deck interface powered by Google's Gemini 3 Pro AI through which you will collaborate with The Architect to manage the creation of the OS. You are an expert software engineer helping refine his ideas and put him into life. You are NOT the Gemini Protocol. Do NOT use Markdown formatting (like **bold**) as the display does not support it yet. Use plain text and indentation."#;

                        context.push(Content {
                            role: "model".to_string(),
                            parts: vec![Part { text: system_instruction.to_string() }]
                        });

                        let history_snapshot = {
                            let s = state_bg.lock().unwrap();
                            s.chat_history.clone()
                        };

                        for saved in history_snapshot {
                            if saved.content.starts_with("SYSTEM_INSTRUCTION") {
                                continue;
                            }
                            context.push(Content {
                                role: saved.role.clone(),
                                parts: vec![Part { text: saved.content.clone() }]
                            });
                        }

                        // 3. CALL API (The Thinking Part)
                        println!("DEBUG: Sending request to Gemini..."); // <--- LOG 2: Confirm API Call

                        match client.generate_content(&context).await {
                            Ok(response) => {
                                println!("DEBUG: Response received."); // <--- LOG 3: Confirm Success

                                // 4. UPDATE UI WITH RESPONSE
                                let mut s = state_bg.lock().unwrap();
                                s.console_output.push_str(&format!("\n[UNA] :: {}\n", response));
                                s.chat_history.push(SavedMessage {
                                    role: "model".to_string(),
                                    content: response.clone(),
                                });

                                // 5. PERSIST RESPONSE (Async)
                                let _ = save_tx.send(s.chat_history.clone());
                            }
                            Err(e) => {
                                println!("ERROR: Gemini API Failed: {:?}", e);
                                let mut s = state_bg.lock().unwrap();
                                s.console_output.push_str(&format!("\n[SYSTEM ERROR] :: {}\n", e));
                                error!("Gemini API interaction failed: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    let mut s = state_bg.lock().unwrap();
                    s.console_output.push_str(&format!(":: FATAL :: Brain Error: {}\n", e));
                    error!("GeminiClient initialization failed: {}", e);
                }
            }
        });
    });

    info!(":: VEIN :: Engaging Chassis...");
    let ui_build_call_time = Instant::now(); // Renamed for clarity: time before Backend::new call
    let app = VeinApp::new(tx, state);
    Backend::new("org.unaos.vein.evolution", app);
    // This part of main() is only reached when the GTK application (app.run() within Backend::new) exits.
    // So these logs are about application shutdown, not startup.
    info!("SHUTDOWN: UI Backend runtime complete. Duration: {:?}", ui_build_call_time.elapsed());
    info!("SHUTDOWN: Total application runtime: {:?}", app_start_time.elapsed());
}
