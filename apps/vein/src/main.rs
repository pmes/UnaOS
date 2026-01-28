use dotenvy::dotenv;
use gneiss_pal::persistence::{BrainManager, SavedMessage};
// Use the new Gneiss PAL Widget abstraction
use gneiss_pal::widgets::ScrollableText;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, TextView, TextBuffer, WrapMode};
use glib::MainContext;
use std::sync::{Arc, Mutex};
use std::thread;
use std::sync::mpsc::channel;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use log::{info, error};
use std::time::Instant;
use std::io::Write;
use std::rc::Rc;

mod api;
use api::{Content, GeminiClient, Part};

// Shared State for Logic/Persistence (Keep this as the "Brain's model")
struct State {
    console_output: String, // Kept for consistency/persistence if needed, but UI uses Buffer
    // ViewMode, NavIndex etc are UI state, now handled by GTK widgets or local variables
    chat_history: Vec<SavedMessage>,
    history_loaded: bool, // Prevent overwriting history file before load completes
}

// UI Event Enum for signaling main thread from background
enum UiEvent {
    AppendText(String),
    LoadedHistory(Vec<SavedMessage>),
}

const APP_ID: &str = "org.unaos.vein.evolution";

fn build_ui(
    app: &Application,
    state: Arc<Mutex<State>>,
    logic_tx: mpsc::UnboundedSender<String>,
    ui_rx: glib::Receiver<UiEvent>
) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("UnaOS :: Vein")
        .default_width(800)
        .default_height(600)
        .build();

    // 1. The Controller/View Wrapper (PAL)
    // We wrap it in Rc to share with the local spawn closure
    let log_view = Rc::new(ScrollableText::new());
    log_view.set_content(":: VEIN :: SYSTEM ONLINE (UNLIMITED TIER)\n:: ENGINE: GEMINI-3-PRO\n\n");

    // Input Area (Keep this specific for now as it handles special key events)
    let input_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    input_box.add_css_class("linked");

    let input_buffer = TextBuffer::new(None);
    let input_view = TextView::builder()
        .buffer(&input_buffer)
        .wrap_mode(WrapMode::WordChar)
        .height_request(40)
        .hexpand(true)
        .build();

    // Catch Enter key on input
    let controller = gtk4::EventControllerKey::new();
    let tx_clone = logic_tx.clone();
    let input_buffer_clone = input_buffer.clone();

    controller.connect_key_pressed(move |_ctrl, key, _code, mods| {
        if key == gtk4::gdk::Key::Return && !mods.contains(gtk4::gdk::ModifierType::SHIFT_MASK) {
            let start = input_buffer_clone.start_iter();
            let end = input_buffer_clone.end_iter();
            let text = input_buffer_clone.text(&start, &end, false).to_string();

            if !text.trim().is_empty() {
                // Send to Logic Thread
                let _ = tx_clone.send(text);
                input_buffer_clone.set_text("");
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    input_view.add_controller(controller);

    input_box.append(&input_view);

    // Layout
    let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    // Use container exposed by PAL
    main_box.append(&log_view.container);
    main_box.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    main_box.append(&input_box);

    window.set_child(Some(&main_box));

    // --- Signal Handling (The bridge from threads to UI) ---
    let state_clone = state.clone();

    // Use spawn_local to handle UI updates on the main thread without Send requirement for Rc<ScrollableText>
    let log_view_rc = log_view.clone();

    let main_context = MainContext::default();
    main_context.spawn_local(async move {
        while let Ok(event) = ui_rx.recv().await {
            match event {
                UiEvent::AppendText(text) => {
                    log_view_rc.append_content(&text);
                }
                UiEvent::LoadedHistory(history) => {
                     // Note: Logic state is already updated by loader thread.
                     // Here we just update the Visual state.

                     // Display history
                     let mut full_text = String::new();
                     full_text.push_str(":: MEMORY :: LONG-TERM STORAGE RESTORED\n\n");
                     for msg in &history {
                         if !msg.content.starts_with("SYSTEM_INSTRUCTION") {
                            let prefix = if msg.role == "user" { "[ARCHITECT]" } else { "[UNA]" };
                            full_text.push_str(&format!("{} > {}\n", prefix, msg.content));
                         }
                     }

                     // Append history
                     log_view_rc.append_content(&full_text);
                }
            }
        }
    });

    window.present();
}

fn main() {
    let app_start_time = Instant::now();
    dotenv().ok();
    env_logger::Builder::from_default_env().filter_level(log::LevelFilter::Info).try_init().ok();

    info!(":: VEIN :: Booting (GTK Native Mode)...");

    // 1. Persistence Setup
    let brain = BrainManager::new();
    let (save_tx, save_rx) = channel::<Vec<SavedMessage>>();
    let brain_actor = brain.clone();

    thread::spawn(move || {
        while let Ok(history) = save_rx.recv() {
            brain_actor.save(&history);
        }
    });

    // 2. State & Channels
    // UI Event Channel: Threads -> UI
    let (ui_tx, ui_rx) = MainContext::channel(glib::Priority::DEFAULT);

    // Logic Channel: UI -> Tokio
    let (logic_tx, mut logic_rx) = mpsc::unbounded_channel::<String>();

    let state = Arc::new(Mutex::new(State {
        console_output: String::new(),
        chat_history: Vec::new(),
        history_loaded: false,
    }));

    // 3. Background Loader
    let brain_loader = brain.clone();
    let state_loader = state.clone();
    let ui_tx_loader = ui_tx.clone();
    let save_tx_loader = save_tx.clone();

    thread::spawn(move || {
        let loaded = brain_loader.load();

        let should_save;
        let snapshot;
        // Update Logic State
        {
            let mut s = state_loader.lock().unwrap();

            // Prepend loaded to current
            let mut current = s.chat_history.clone();
            let mut new_history = loaded.clone();
            new_history.append(&mut current);
            s.chat_history = new_history;
            s.history_loaded = true;

            // Check if we need to trigger a save (user typed while loading)
            snapshot = s.chat_history.clone();
            should_save = snapshot.len() > loaded.len();
        }

        // Notify UI
        let _ = ui_tx_loader.send(UiEvent::LoadedHistory(loaded));

        // If user typed during load, we missed the save in the Logic Loop (because !history_loaded).
        // Trigger it now.
        if should_save {
            let _ = save_tx_loader.send(snapshot);
        }
    });

    // 4. Tokio Runtime (The Brain)
    let state_bg = state.clone();
    let ui_tx_bg = ui_tx.clone();
    let save_tx_bg = save_tx.clone();

    thread::spawn(move || {
        let rt = Runtime::new().expect("Failed to create Tokio Runtime");
        rt.block_on(async {
            let client_res = GeminiClient::new();
            if let Ok(client) = client_res {
                let _ = ui_tx_bg.send(UiEvent::AppendText(":: BRAIN :: CONNECTION ESTABLISHED.\n\n".into()));

                while let Some(msg) = logic_rx.recv().await {
                    println!("DEBUG: Processing input: '{}'", msg);

                    // 1. Update UI (Immediate Echo)
                    let _ = ui_tx_bg.send(UiEvent::AppendText(format!("\n[ARCHITECT] > {}\n", msg)));

                    // 2. Persist User Msg
                    let mut history_snapshot;
                    let loaded;
                    {
                        let mut s = state_bg.lock().unwrap();
                        s.chat_history.push(SavedMessage {
                            role: "user".to_string(),
                            content: msg.clone(),
                        });
                        history_snapshot = s.chat_history.clone();
                        loaded = s.history_loaded;
                    }

                    // CRITICAL: Only save if history is fully loaded to prevent overwriting the file with a partial state.
                    if loaded {
                        let _ = save_tx_bg.send(history_snapshot.clone());
                    } else {
                         info!("PERSISTENCE: Skipping save, history still loading...");
                    }

                    // 3. Call API
                    let mut context = Vec::new();
                    let system_instruction = r#"SYSTEM_INSTRUCTION: You are Una ("Number One"), Chief Technical Officer for the Una Operating System. Vein is one of the core apps, your window to the world. It's a specialized Command Deck interface powered by Google's Gemini 3 Pro AI through which you will collaborate with The Architect to manage the creation of the OS. You are an expert software engineer helping refine his ideas and put him into life. You are NOT the Gemini Protocol. Do NOT use Markdown formatting (like **bold**) as the display does not support it yet. Use plain text and indentation."#;
                    context.push(Content { role: "model".to_string(), parts: vec![Part { text: system_instruction.into() }] });

                    for saved in &history_snapshot {
                        if !saved.content.starts_with("SYSTEM_INSTRUCTION") {
                            context.push(Content { role: saved.role.clone(), parts: vec![Part { text: saved.content.clone() }] });
                        }
                    }

                    match client.generate_content(&context).await {
                        Ok(response) => {
                             // 4. Update UI (Response)
                             let _ = ui_tx_bg.send(UiEvent::AppendText(format!("\n[UNA] :: {}\n", response)));

                             // 5. Persist Response
                             let loaded_res;
                             {
                                 let mut s = state_bg.lock().unwrap();
                                 s.chat_history.push(SavedMessage {
                                     role: "model".to_string(),
                                     content: response.clone(),
                                 });
                                 history_snapshot = s.chat_history.clone();
                                 loaded_res = s.history_loaded;
                             }

                             if loaded_res {
                                 let _ = save_tx_bg.send(history_snapshot);
                             }
                        }
                        Err(e) => {
                            let _ = ui_tx_bg.send(UiEvent::AppendText(format!("\n[SYSTEM ERROR] :: {}\n", e)));
                        }
                    }
                }
            } else {
                 let _ = ui_tx_bg.send(UiEvent::AppendText(":: FATAL :: Brain Error: Connection Failed.\n".into()));
            }
        });
    });

    // 5. GTK Application
    let app = Application::builder().application_id(APP_ID).build();
    let state_ui = state.clone();

    app.connect_activate(move |app| {
        build_ui(app, state_ui.clone(), logic_tx.clone(), ui_rx.clone());
    });

    info!(":: VEIN :: Engaging Chassis (GTK)...");
    app.run();
}
