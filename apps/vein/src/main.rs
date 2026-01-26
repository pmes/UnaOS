use gneiss_pal::{EventLoop, AppHandler, text, Event, KeyCode};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use dotenvy::dotenv;
use ab_glyph::FontRef;

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
    font: FontRef<'static>,
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

    fn draw(&mut self, buffer: &mut [u32], width: u32, height: u32) {
        // Layout Constants
        const PADDING_BOTTOM: i32 = 16;
        const PADDING_TEXT: i32 = 12;
        const PADDING: i32 = 12;
        const TOP_MARGIN: i32 = 50; // Don't draw over title

        // Fill Background with Una Blue
        let bg_color = 0x00aaff;
        for pixel in buffer.iter_mut() {
            *pixel = bg_color;
        }

        // Grab state quickly and drop lock
        let (status_text, history) = {
            let s = self.state.lock().unwrap();
            (s.status_text.clone(), s.chat_history.clone())
        };

        // Title
        text::draw_text(
            buffer,
            width,
            height,
            "UnaOS Virtual Office: ONLINE",
            50,
            TOP_MARGIN,
            0xFFFFFFFF, // White
            &self.font,
        );

        // Network Status
        text::draw_text(
            buffer,
            width,
            height,
            &status_text,
            50,
            90, // A bit lower
            0xFFFFFFFF, // White
            &self.font,
        );

        // Dynamic Layout Calculation
        // 1. Measure Input Height
        let input_text = format!("{} _", self.input_buffer);
        let input_text_height = text::measure_text_height(width, &input_text, &self.font);

        let input_area_height = input_text_height + PADDING * 2;
        let input_bg_y = (height as i32) - input_area_height - PADDING_BOTTOM;
        let input_text_y = input_bg_y + PADDING;

        // 2. Separator is just above the input pill
        let sep_y = input_bg_y - PADDING;
        let mut cursor_y = sep_y - PADDING_TEXT;

        // Draw Pill (Dark Gray)
        // Rect: x=50, y=input_bg_y, w=width-100, h=input_area_height
        let pill_x = 50;
        let pill_w = width as i32 - 100;
        let pill_color = 0xFF333333;

        if pill_w > 0 && input_area_height > 0 {
             for row in 0..input_area_height {
                 for col in 0..pill_w {
                     let py = input_bg_y + row;
                     let px = pill_x + col;
                     if px >= 0 && px < width as i32 && py >= 0 && py < height as i32 {
                         let idx = (py * width as i32 + px) as usize;
                         if idx < buffer.len() {
                             buffer[idx] = pill_color;
                         }
                     }
                 }
             }
        }

        // Draw Input Text (White)
        // Show cursor if blink logic matches
        let cursor_char = if (self.frame_count % 60) < 30 { "_" } else { " " };
        text::draw_text(
            buffer,
            width,
            height,
            &format!("{} {}", self.input_buffer, cursor_char),
            50,
            input_text_y,
            0xFFFFFFFF, // White
            &self.font,
        );

        // Draw Separator Line
        if sep_y > 0 && sep_y < height as i32 {
             for x in 0..width {
                  // Make it 2px thick
                  for dy in 0..2 {
                      let py = sep_y + dy;
                      if py < height as i32 {
                          let idx = (py * width as i32 + x as i32) as usize;
                          if idx < buffer.len() {
                              buffer[idx] = 0xFFCCCCCC;
                          }
                      }
                  }
             }
        }

        // Draw History (Bottom-Up)
        for msg in history.iter().rev() {
            let msg_height = text::measure_text_height(width, msg, &self.font);

            cursor_y -= msg_height;

            text::draw_text(buffer, width, height, msg, 50, cursor_y, 0xFFFFFFFF, &self.font);

            // Check boundary AFTER drawing to allow clipping
            if cursor_y < 120 {
                break;
            }

            // Add gap
            cursor_y -= 8;
        }
    }
}

fn main() {
    // Load environment variables
    dotenv().ok();

    println!(":: VEIN :: Booting...");

    // Load Font Once
    let font = text::get_font();

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
        font,
    };

    if let Err(e) = event_loop.run(handler) {
        eprintln!(":: VEIN CRASH :: {}", e);
    }

    println!(":: VEIN :: Shutdown.");
}
