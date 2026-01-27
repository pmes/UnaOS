use gneiss_pal::{EventLoop, AppHandler, Event, KeyCode, DashboardState, ViewMode};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use dotenvy::dotenv;

mod api;
use api::GeminiClient;

struct State {
    status_text: String,
    #[allow(dead_code)]
    network_initialized: bool,
    chat_history: Vec<String>,
}

struct VeinApp {
    state: Arc<Mutex<State>>,
    input_buffer: String,
    tx: mpsc::UnboundedSender<String>,
    frame_count: u64,
    // Local UI State
    mode: ViewMode,
    active_nav_index: usize,
}

impl AppHandler for VeinApp {
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Timer => {
                self.frame_count = self.frame_count.wrapping_add(1);
            }
            Event::Nav(index) => {
                self.active_nav_index = index;
                // Switch mode based on selection for now
                // Left Pane logic:
                // Comms: [General, Encrypted]
                // Wolfpack: [J-Series, S-Series]
                //
                // Wait, the lists change based on mode. But how do we switch mode?
                // The prompt says: "If Comms: Left = ... If Wolfpack: Left = ..."
                // It implies the mode switching might be external or via Actions?
                // Or maybe Nav items switch sub-contexts.

                // Let's assume Nav selection just updates active index.
                // Mode switching via Actions.
            }
            Event::Action(index) => {
                match self.mode {
                    ViewMode::Comms => {
                        match index {
                            0 => { // Clear
                                self.state.lock().unwrap().chat_history.clear();
                                self.input_buffer.clear();
                            }
                            1 => { // Save / Switch Mode Test
                                // Let's use this to toggle mode for demonstration
                                self.mode = ViewMode::Wolfpack;
                                self.active_nav_index = 0;
                            }
                            _ => {}
                        }
                    }
                    ViewMode::Wolfpack => {
                        match index {
                            0 => { // Deploy / Switch Back
                                self.mode = ViewMode::Comms;
                                self.active_nav_index = 0;
                            }
                            1 => { // Sleep
                                // Do nothing
                            }
                            _ => {}
                        }
                    }
                }
            }
            Event::Char(c) => {
                if self.mode == ViewMode::Comms {
                    self.input_buffer.push(c);
                }
            }
            Event::KeyDown(KeyCode::Backspace) => {
                if self.mode == ViewMode::Comms {
                    self.input_buffer.pop();
                }
            }
            Event::KeyDown(KeyCode::Enter) => {
                if self.mode == ViewMode::Comms && !self.input_buffer.is_empty() {
                    let msg = self.input_buffer.clone();
                    // Update Local State
                    {
                        let mut s = self.state.lock().unwrap();
                        s.chat_history.push(format!("> {}", msg));
                        // Keep history manageable
                        if s.chat_history.len() > 15 {
                            s.chat_history.remove(0);
                        }
                    }
                    // Send to background
                    if let Err(e) = self.tx.send(msg) {
                        eprintln!("Failed to send message: {}", e);
                    }
                    // Clear buffer
                    self.input_buffer.clear();
                }
            }
            _ => {}
        }
    }

    fn view(&self) -> DashboardState {
        // Grab state quickly and drop lock
        let (status_text, history) = {
            let s = self.state.lock().unwrap();
            (s.status_text.clone(), s.chat_history.clone())
        };

        // Construct Comms Output
        let mut console_output = String::new();
        if self.mode == ViewMode::Comms {
            console_output.push_str("UnaOS Virtual Office: ONLINE\n");
            console_output.push_str(&format!("{}\n", status_text));
            console_output.push_str("----------------------------------------\n\n");

            for msg in &history {
                console_output.push_str(msg);
                console_output.push_str("\n\n");
            }

            let cursor = if (self.frame_count % 60) < 30 { "_" } else { " " };
            console_output.push_str(&format!("> {}{}", self.input_buffer, cursor));
        } else {
            console_output = "Wolfpack Grid Active (Placeholder)".to_string();
        }

        let (nav_items, actions) = match self.mode {
            ViewMode::Comms => (
                vec!["General".to_string(), "Encrypted".to_string()],
                vec!["Clear".to_string(), "Wolfpack".to_string()]
            ),
            ViewMode::Wolfpack => (
                vec!["J-Series".to_string(), "S-Series".to_string()],
                vec!["Comms".to_string(), "Sleep".to_string()]
            ),
        };

        DashboardState {
            mode: self.mode.clone(),
            nav_items,
            active_nav_index: self.active_nav_index,
            console_output,
            actions,
        }
    }
}

fn main() {
    // Load environment variables
    dotenv().ok();

    println!(":: VEIN :: Booting...");

    // Shared State
    let state = Arc::new(Mutex::new(State {
        status_text: "Initializing Network Stack...".to_string(),
        network_initialized: false,
        chat_history: Vec::new(),
    }));

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state_for_bg = state.clone();

    // Spawn Background Async Runtime
    thread::spawn(move || {
        let rt = Runtime::new().expect("Failed to create Tokio Runtime");

        rt.block_on(async {
            println!(":: VEIN :: Async Core Starting...");

            // Initialize Gemini Client
            let client_option = match GeminiClient::new() {
                Ok(client) => Some(client),
                Err(e) => {
                    let mut s = state_for_bg.lock().unwrap();
                    s.status_text = format!("Config Error: {}", e);
                    s.network_initialized = true;
                    eprintln!(":: VEIN :: Config Error: {}", e);
                    None
                }
            };

            // Initial System Check (only if client exists)
            if let Some(client) = &client_option {
                 // Simulate startup delay
                 tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;

                 println!(":: VEIN :: Connecting to Synapse...");
                 match client.generate_content("Hello, I am Vein. System check.").await {
                     Ok(response) => {
                         let mut s = state_for_bg.lock().unwrap();
                         s.chat_history.push(format!("System: {}", response));
                         s.status_text = "System Online".to_string();
                         s.network_initialized = true;
                         println!(":: VEIN :: Synapse Connection Established.");
                     }
                     Err(e) => {
                         let mut s = state_for_bg.lock().unwrap();
                         s.status_text = format!("Connection Failed: {}", e);
                         s.network_initialized = true;
                         eprintln!(":: VEIN :: Synapse Connection Failed: {}", e);
                     }
                 }
            }

            // Message Loop
            if let Some(client) = &client_option {
                while let Some(msg) = rx.recv().await {
                    match client.generate_content(&msg).await {
                         Ok(response) => {
                             let mut s = state_for_bg.lock().unwrap();
                             s.chat_history.push(response);
                             if s.chat_history.len() > 15 {
                                 s.chat_history.remove(0);
                             }
                         }
                         Err(e) => {
                             let mut s = state_for_bg.lock().unwrap();
                             s.chat_history.push(format!("Error: {}", e));
                             if s.chat_history.len() > 15 {
                                 s.chat_history.remove(0);
                             }
                         }
                    }
                }
            } else {
                 // If no client, we can't do anything but maybe log errors
                 while let Some(_) = rx.recv().await {
                      state_for_bg.lock().unwrap().chat_history.push("System Error: AI Config Missing".to_string());
                 }
            }
        });
    });

    // Start UI
    println!(":: VEIN :: Initializing Graphical Interface...");

    // NEW: Use EventLoop instead of WaylandApp
    let event_loop = EventLoop::new();

    let handler = VeinApp {
        state,
        input_buffer: String::new(),
        tx,
        frame_count: 0,
        mode: ViewMode::Comms,
        active_nav_index: 0,
    };

    if let Err(e) = event_loop.run(handler) {
        eprintln!(":: VEIN CRASH :: {}", e);
    }

    println!(":: VEIN :: Shutdown.");
}
