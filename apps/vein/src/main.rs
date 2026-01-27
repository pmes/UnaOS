use gneiss_pal::{Backend, AppHandler, Event, DashboardState, ViewMode};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use dotenvy::dotenv;

mod api;
use api::GeminiClient;

// Shared State between UI (Sync) and Backend (Async)
struct State {
    console_output: String,
    mode: ViewMode,
    nav_index: usize,
    history: Vec<String>, // Keeping local history for quick display
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
        match event {
            Event::Input(text) => {
                // UI Thread Update (Instant Feedback)
                {
                    let mut s = self.state.lock().unwrap();
                    s.console_output.push_str(&format!("\n[YOU] > {}\n", text));
                    s.history.push(format!("[YOU] > {}", text));
                }

                // Command Handling (Client-side immediate)
                if text.trim() == "/wolf" {
                    let mut s = self.state.lock().unwrap();
                    s.mode = ViewMode::Wolfpack;
                    s.console_output.push_str("\n[SYSTEM] :: Switching to Wolfpack Grid...\n");
                    return;
                } else if text.trim() == "/comms" {
                    let mut s = self.state.lock().unwrap();
                    s.mode = ViewMode::Comms;
                    s.console_output.push_str("\n[SYSTEM] :: Secure Comms Established.\n");
                    return;
                } else if text.trim() == "/clear" {
                     let mut s = self.state.lock().unwrap();
                     s.console_output.clear();
                     s.console_output.push_str(":: VEIN :: SYSTEM CLEARED\n\n");
                     return;
                }

                // Send to Async Core
                if let Err(e) = self.tx.send(text) {
                     let mut s = self.state.lock().unwrap();
                     s.console_output.push_str(&format!("\n[SYSTEM ERROR] :: Failed to send: {}\n", e));
                }
            }
            Event::TemplateAction(idx) => {
                match idx {
                    0 => { // Clear / Deploy J1
                        let mut s = self.state.lock().unwrap();
                        if s.mode == ViewMode::Comms {
                            s.console_output.clear();
                            s.console_output.push_str(":: VEIN :: SYSTEM CLEARED\n\n");
                        } else {
                            s.console_output.push_str("\n[WOLFPACK] :: Deploying J-Series Unit...\n");
                        }
                    }
                    1 => { // Wolfpack View / Deploy S5
                        let mut s = self.state.lock().unwrap();
                        if s.mode == ViewMode::Comms {
                            s.mode = ViewMode::Wolfpack;
                            s.console_output.push_str("\n[SYSTEM] :: Switching to Wolfpack Grid...\n");
                        } else {
                            s.console_output.push_str("\n[WOLFPACK] :: Deploying S-Series Unit...\n");
                        }
                    }
                    2 => { // Back to Comms
                         let mut s = self.state.lock().unwrap();
                         if s.mode == ViewMode::Wolfpack {
                             s.mode = ViewMode::Comms;
                             s.console_output.push_str("\n[SYSTEM] :: Returning to Comms.\n");
                         }
                    }
                    _ => {}
                }
            }
            Event::NavSelect(idx) => {
                let mut s = self.state.lock().unwrap();
                s.nav_index = idx;
                // Channel switching logic could go here
            }
            _ => {}
        }
    }

    fn view(&self) -> DashboardState {
        let s = self.state.lock().unwrap();

        DashboardState {
            mode: s.mode.clone(),
            nav_items: vec![
                "General".into(),
                "Encrypted".into(),
                "Jules (Private)".into()
            ],
            active_nav_index: s.nav_index,
            console_output: s.console_output.clone(),
            actions: match s.mode {
                ViewMode::Comms => vec![
                    "Clear Buffer".into(),
                    "Wolfpack View".into()
                ],
                ViewMode::Wolfpack => vec![
                    "Deploy J1".into(),
                    "Deploy S5".into(),
                    "Back to Comms".into()
                ],
            },
        }
    }
}

fn main() {
    dotenv().ok();
    println!(":: VEIN :: Booting (The Great Evolution)...");

    // Shared State
    let state = Arc::new(Mutex::new(State {
        console_output: ":: VEIN :: SYSTEM ONLINE (UNLIMITED TIER)\n:: ENGINE: GEMINI-3-PRO\n\n".to_string(),
        mode: ViewMode::Comms,
        nav_index: 0,
        history: Vec::new(),
    }));

    // Communication Channels
    // UI -> Async
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    // Background Thread (The Brain)
    let state_bg = state.clone();
    thread::spawn(move || {
        let rt = Runtime::new().expect("Failed to create Tokio Runtime");
        rt.block_on(async {
             println!(":: VEIN :: Brain Connecting...");

             // Initialize Client
             // Note: In "The Great Evolution", we assume the client is configured via env
             // and supports the "gemini-3-pro" string if available, or we fall back.
             let client_res = GeminiClient::new(); // Existing wrapper

             match client_res {
                 Ok(client) => {
                      {
                          let mut s = state_bg.lock().unwrap();
                          s.console_output.push_str(":: BRAIN :: CONNECTION ESTABLISHED.\n\n");
                      }

                      while let Some(msg) = rx.recv().await {
                          // Call API
                          match client.generate_content(&msg).await {
                              Ok(response) => {
                                  let mut s = state_bg.lock().unwrap();
                                  s.console_output.push_str(&format!("\n[WOLFPACK] :: {}\n", response));
                              }
                              Err(e) => {
                                  let mut s = state_bg.lock().unwrap();
                                  s.console_output.push_str(&format!("\n[SYSTEM ERROR] :: {}\n", e));
                              }
                          }
                      }
                 }
                 Err(e) => {
                      let mut s = state_bg.lock().unwrap();
                      s.console_output.push_str(&format!(":: FATAL :: Brain Error: {}\n", e));
                 }
             }
        });
    });

    // Start UI (The Body)
    println!(":: VEIN :: Engaging Chassis...");
    let app = VeinApp::new(tx, state);

    // "org.unaos.vein"
    Backend::new("org.unaos.vein", app);
}
