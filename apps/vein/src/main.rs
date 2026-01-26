use gneiss_pal::{WaylandApp, AppHandler, text, Event, KeyCode};
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
}

impl AppHandler for VeinApp {
    fn handle_event(&mut self, event: Event) {
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

    fn draw(&mut self, buffer: &mut [u32], width: u32, height: u32) {
        // Fill Background with Una Blue
        let bg_color = 0x00aaff;
        for pixel in buffer.iter_mut() {
            *pixel = bg_color;
        }

        let state = self.state.lock().unwrap();

        // Title
        text::draw_text(
            buffer,
            width,
            height,
            "UnaOS Virtual Office: ONLINE",
            50,
            50,
            0xFFFFFFFF, // White
        );

        // Network Status
        let mut y = text::draw_text(
            buffer,
            width,
            height,
            &state.status_text,
            50,
            90, // A bit lower
            0xFFFFFFFF, // White
        );

        y += 20; // Padding

        // Draw History
        for msg in &state.chat_history {
            y = text::draw_text(buffer, width, height, msg, 50, y, 0xFFFFFFFF);
            // Simple clip check
            if y > (height as i32 - 60) {
                 break;
            }
        }

        // Draw Input Line
        let input_y = (height as i32) - 40;

        // Separator Line
        let sep_y = input_y - 10;
        if sep_y > 0 && sep_y < height as i32 {
             for x in 0..width {
                  let idx = (sep_y * width as i32 + x as i32) as usize;
                  if idx < buffer.len() {
                      buffer[idx] = 0xFFCCCCCC;
                  }
             }
        }

        text::draw_text(
            buffer,
            width,
            height,
            &format!("{} _", self.input_buffer),
            50,
            input_y,
            0xFF00FF00, // Green
        );
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
    let app = WaylandApp::new().expect("Failed to initialize PAL");
    let handler = VeinApp {
        state,
        input_buffer: String::new(),
        tx,
    };

    if let Err(e) = app.run_window(handler) {
        eprintln!(":: VEIN CRASH :: {}", e);
    }

    println!(":: VEIN :: Shutdown.");
}
