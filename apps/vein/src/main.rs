use dotenvy::dotenv;
use gneiss_pal::persistence::{BrainManager, SavedMessage};
// We rely on gneiss_pal for Persistence only now.
// UI is handled natively by GTK4.
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, ScrolledWindow, TextView, TextBuffer, PolicyType, WrapMode};
use glib::MainContext;
use std::sync::{Arc, Mutex};
use std::thread;
use std::sync::mpsc::channel;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use log::{info, error};
use std::time::Instant;
use std::io::Write;

mod api;
use api::{Content, GeminiClient, Part};

// Shared State for Logic/Persistence (Keep this as the "Brain's model")
struct State {
    console_output: String, // Kept for consistency/persistence if needed, but UI uses Buffer
    // ViewMode, NavIndex etc are UI state, now handled by GTK widgets or local variables
    chat_history: Vec<SavedMessage>,
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

    // 1. The Controller: Handles the scrolling logic
    let scrolled_window = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build();

    // 2. The Model: TextBuffer
    let buffer = TextBuffer::new(None);
    buffer.set_text(":: VEIN :: SYSTEM ONLINE (UNLIMITED TIER)\n:: ENGINE: GEMINI-3-PRO\n\n");

    // 3. The View: Displays only what fits on screen
    let text_view = TextView::builder()
        .buffer(&buffer)
        .editable(false)
        .monospace(true)
        .wrap_mode(WrapMode::WordChar)
        .left_margin(10)
        .right_margin(10)
        .top_margin(10)
        .bottom_margin(10)
        .build();

    scrolled_window.set_child(Some(&text_view));

    // Input Area
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
    main_box.append(&scrolled_window);
    main_box.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    main_box.append(&input_box);

    window.set_child(Some(&main_box));

    // --- Signal Handling (The bridge from threads to UI) ---
    // We attach the receiver to the main context (UI thread)
    let buffer_clone = buffer.clone();
    let state_clone = state.clone();
    let text_view_clone = text_view.clone();

    ui_rx.attach(None, move |event| {
        match event {
            UiEvent::AppendText(text) => {
                let mut end = buffer_clone.end_iter();
                buffer_clone.insert(&mut end, &text);

                // Auto-scroll
                let mark = buffer_clone.create_mark(None, &buffer_clone.end_iter(), false);
                text_view_clone.scroll_to_mark(&mark, 0.0, false, 0.0, 1.0);
            }
            UiEvent::LoadedHistory(history) => {
                 let mut s = state_clone.lock().unwrap();
                 // Merge history logic (simpler here: just prepend visual, logic already handled in loader thread if we were using the old way.
                 // But wait, we moved logic ownership to MainContext? No, logic is in threads.)

                 // Display history
                 let mut full_text = String::new();
                 full_text.push_str(":: MEMORY :: LONG-TERM STORAGE RESTORED\n\n");
                 for msg in &history {
                     if !msg.content.starts_with("SYSTEM_INSTRUCTION") {
                        let prefix = if msg.role == "user" { "[ARCHITECT]" } else { "[UNA]" };
                        full_text.push_str(&format!("{} > {}\n", prefix, msg.content));
                     }
                 }

                 // Prepend? Standard TextBuffer doesn't handle prepend easily while preserving scroll,
                 // but typically we load history once at start.
                 // For now, let's just Insert at Start (after the header).
                 // Actually, "LoadedHistory" event comes from Loader.

                 // We can just dump it.
                 let mut start = buffer_clone.start_iter();
                 // Skip the initial "SYSTEM ONLINE" header if we want, or just append.
                 // Let's just append to the end for simplicity, assuming this happens fast.
                 // OR, strictly follows:
                 // The buffer was "Initializing...".
                 // Now we replace or append.

                 // Let's clear and re-render full history + any new logs
                 // buffer_clone.set_text(&full_text); // Simple reset
                 // But wait, if user typed while loading?
                 // The loader thread logic in previous step handled merging.
                 // Here, we just display what was loaded.
                 // For "Buttery Smooth", we can just insert.

                 // Let's iterate and insert.
                 let mut end = buffer_clone.end_iter();
                 buffer_clone.insert(&mut end, &full_text);
            }
        }
        glib::ControlFlow::Continue
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
    }));

    // 3. Background Loader
    let brain_loader = brain.clone();
    let state_loader = state.clone();
    let ui_tx_loader = ui_tx.clone();

    thread::spawn(move || {
        let loaded = brain_loader.load();

        // Update Logic State
        {
            let mut s = state_loader.lock().unwrap();
            // We blindly overwrite/append for now.
            // In a real scenario, we'd merge carefuly.
            // Since this runs at startup, s.chat_history is likely empty or has just a few inputs.
            // Let's Prepend loaded to current.
            let mut current = s.chat_history.clone();
            let mut new_history = loaded.clone();
            new_history.append(&mut current);
            s.chat_history = new_history;
        }

        // Notify UI
        let _ = ui_tx_loader.send(UiEvent::LoadedHistory(loaded));
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
                    {
                        let mut s = state_bg.lock().unwrap();
                        s.chat_history.push(SavedMessage {
                            role: "user".to_string(),
                            content: msg.clone(),
                        });
                        history_snapshot = s.chat_history.clone();
                    }
                    let _ = save_tx_bg.send(history_snapshot.clone());

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
                             {
                                 let mut s = state_bg.lock().unwrap();
                                 s.chat_history.push(SavedMessage {
                                     role: "model".to_string(),
                                     content: response.clone(),
                                 });
                                 history_snapshot = s.chat_history.clone();
                             }
                             let _ = save_tx_bg.send(history_snapshot);
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
