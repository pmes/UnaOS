use gneiss_pal::{EventLoop, AppHandler, Event, KeyCode};
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
}

impl AppHandler for VeinApp {
    fn handle_event(&mut self, event: Event) {
        // Blink timer and input handling
        if let Event::Timer = event {
            self.frame_count = self.frame_count.wrapping_add(1);
        }

        match event {
            Event::Char(c) => {
                self.input_buffer.push(c);
            }
            Event::KeyDown(KeyCode::Backspace) => {
                self.input_buffer.pop();
            }
            Event::KeyDown(KeyCode::Enter) => {
                if !self.input_buffer.is_empty() {
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

    fn view(&self) -> String {
        // Grab state quickly and drop lock
        let (status_text, history) = {
            let s = self.state.lock().unwrap();
            (s.status_text.clone(), s.chat_history.clone())
        };

        let mut view = String::new();
        view.push_str("UnaOS Virtual Office: ONLINE\n");
        view.push_str(&format!("{}\n", status_text));
        view.push_str("----------------------------------------\n\n");

        for msg in &history {
            view.push_str(msg);
            view.push_str("\n\n");
        }

        let cursor = if (self.frame_count % 60) < 30 { "_" } else { " " };
        view.push_str(&format!("> {}{}", self.input_buffer, cursor));

        view
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
    };

    if let Err(e) = event_loop.run(handler) {
        eprintln!(":: VEIN CRASH :: {}", e);
    }

    println!(":: VEIN :: Shutdown.");
}
