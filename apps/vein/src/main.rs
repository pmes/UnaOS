use gneiss_pal::{EventLoop, AppHandler, Event, DashboardState, ViewMode};
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
    tx: mpsc::UnboundedSender<String>,
    // Local UI State
    mode: ViewMode,
    active_nav_index: usize,
}

impl AppHandler for VeinApp {
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Input(text) => {
                // Command Handling (Client-side)
                if text.trim() == "/wolf" {
                    self.mode = ViewMode::Wolfpack;
                    self.active_nav_index = 0; // Reset nav
                    {
                         let mut s = self.state.lock().unwrap();
                         s.chat_history.push("> [SYSTEM]: Switching to Wolfpack Grid...".to_string());
                    }
                    return;
                } else if text.trim() == "/comms" {
                    self.mode = ViewMode::Comms;
                    self.active_nav_index = 0;
                    {
                         let mut s = self.state.lock().unwrap();
                         s.chat_history.push("> [SYSTEM]: Secure Comms Established.".to_string());
                    }
                    return;
                }

                // Normal Message Handling
                {
                    let mut s = self.state.lock().unwrap();
                    s.chat_history.push(format!("> {}", text));
                    // Keep history manageable
                    if s.chat_history.len() > 50 {
                        s.chat_history.remove(0);
                    }
                }

                // Send to background Async Core
                if let Err(e) = self.tx.send(text) {
                    eprintln!("Failed to send message: {}", e);
                }
            }
            Event::Nav(index) => {
                self.active_nav_index = index;
                // Simple logic: If in Comms mode, index 0 = General, 1 = Encrypted.
                // Switching channels could be implemented here by filtering history.
                // For now, we just track the selection.
            }
            Event::Action(index) => {
                match self.mode {
                    ViewMode::Comms => {
                        match index {
                            0 => { // Clear
                                self.state.lock().unwrap().chat_history.clear();
                            }
                            1 => { // Save Log (Placeholder)
                                let mut s = self.state.lock().unwrap();
                                s.chat_history.push("[SYSTEM]: Log saved to secure storage.".to_string());
                            }
                            _ => {}
                        }
                    }
                    ViewMode::Wolfpack => {
                        match index {
                            0 => { // Deploy J1
                                let mut s = self.state.lock().unwrap();
                                s.status_text = "Deploying J-Series Unit...".to_string();
                            }
                            1 => { // Sleep All
                                let mut s = self.state.lock().unwrap();
                                s.status_text = "Wolfpack Units Entering Sleep Mode.".to_string();
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn view(&self) -> DashboardState {
        // Grab state snapshot
        let (status_text, history) = {
            let s = self.state.lock().unwrap();
            (s.status_text.clone(), s.chat_history.clone())
        };

        // Construct Console Output
        let mut console_output = String::new();
        if self.mode == ViewMode::Comms {
            console_output.push_str("UnaOS Virtual Office: ONLINE\n");
            console_output.push_str(&format!("{}\n", status_text));
            console_output.push_str("----------------------------------------\n\n");

            for msg in &history {
                console_output.push_str(msg);
                console_output.push_str("\n\n");
            }
        } else {
            console_output.push_str("Wolfpack Grid Active\n");
            console_output.push_str(&format!("Status: {}\n", status_text));
        }

        let (nav_items, actions) = match self.mode {
            ViewMode::Comms => (
                vec!["General".to_string(), "Encrypted".to_string()],
                vec!["Clear".to_string(), "Save Log".to_string()]
            ),
            ViewMode::Wolfpack => (
                vec!["J-Series".to_string(), "S-Series".to_string()],
                vec!["Deploy J1".to_string(), "Sleep All".to_string()]
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
                             if s.chat_history.len() > 50 {
                                 s.chat_history.remove(0);
                             }
                         }
                         Err(e) => {
                             let mut s = state_for_bg.lock().unwrap();
                             s.chat_history.push(format!("Error: {}", e));
                             if s.chat_history.len() > 50 {
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
        tx,
        mode: ViewMode::Comms,
        active_nav_index: 0,
    };

    if let Err(e) = event_loop.run(handler) {
        eprintln!(":: VEIN CRASH :: {}", e);
    }

    println!(":: VEIN :: Shutdown.");
}
