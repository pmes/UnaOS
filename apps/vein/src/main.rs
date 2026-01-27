use gneiss_pal::{Backend, AppHandler, Event, DashboardState, ViewMode};
use gneiss_pal::persistence::{BrainManager, SavedMessage};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use dotenvy::dotenv;

mod api;
use api::{GeminiClient, Content, Part};

// Shared State between UI (Sync) and Backend (Async)
struct State {
    console_output: String,
    mode: ViewMode,
    nav_index: usize,
    history: Vec<String>, // Keeping local history for quick display (redundant but kept for existing logic compatibility)
    chat_history: Vec<SavedMessage>, // Structured history for persistence
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

                    // Add to structured history
                    s.chat_history.push(SavedMessage {
                        role: "user".to_string(),
                        content: text.clone(),
                    });
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

    // Initialize Brain
    let brain = BrainManager::new();
    let saved_history = brain.load();

    let mut console_output = ":: VEIN :: SYSTEM ONLINE (UNLIMITED TIER)\n:: ENGINE: GEMINI-3-PRO\n".to_string();

    if saved_history.is_empty() {
         console_output.push_str(":: MEMORY :: COLD START (New Session)\n\n");
    } else {
         console_output.push_str(":: MEMORY :: LONG-TERM STORAGE RESTORED\n\n");
         for msg in &saved_history {
             // Only display relevant messages (skip system instructions if they somehow got saved)
             if !msg.content.starts_with("SYSTEM_INSTRUCTION") {
                 let prefix = if msg.role == "user" { "[YOU]" } else { "[WOLFPACK]" };
                 console_output.push_str(&format!("{} > {}\n", prefix, msg.content));
             }
         }
         println!("DEBUG: Restored {} items to history context.", saved_history.len());
    }

    // Shared State
    let state = Arc::new(Mutex::new(State {
        console_output,
        mode: ViewMode::Comms,
        nav_index: 0,
        history: Vec::new(),
        chat_history: saved_history,
    }));

    // Communication Channels
    // UI -> Async
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    // Background Thread (The Brain)
    let state_bg = state.clone();
    let brain_bg = brain.clone();

    thread::spawn(move || {
        let rt = Runtime::new().expect("Failed to create Tokio Runtime");
        rt.block_on(async {
             println!(":: VEIN :: Brain Connecting...");

             // Initialize Client
             let client_res = GeminiClient::new();

             match client_res {
                 Ok(client) => {
                      {
                          let mut s = state_bg.lock().unwrap();
                          s.console_output.push_str(":: BRAIN :: CONNECTION ESTABLISHED.\n\n");
                      }

                      while let Some(_msg) = rx.recv().await {
                          // Note: `msg` is already in `chat_history`. We use the full history for context.

                          // SAVE POINT 1: User message has been added by UI thread. Save it now.
                          {
                              let s = state_bg.lock().unwrap();
                              brain_bg.save(&s.chat_history);
                          }

                          // 1. Build Context for Neural Pathways
                          let mut context = Vec::new();

                          // 1a. The Anchor (Always fresh System Prompt)
                          let system_instruction = r#"SYSTEM_INSTRUCTION: You are Una ("Number One"), Chief Technical Officer for the Una Operating System. Vein is one of the core apps, your window to the world. It's a specialized Command Deck interface powered by Google's Gemini 3 Pro AI through which you will collaborate with The Architect to manage the creation of the OS. You are an expert software engineer helping refine his ideas and put them into life. You are NOT the Gemini Protocol. Do NOT use Markdown formatting (like **bold**) as the display does not support it yet. Use plain text and indentation."#;

                          context.push(Content {
                              role: "model".to_string(), // Using "model" as the Anchor
                              parts: vec![Part { text: system_instruction.to_string() }]
                          });

                          // 1b. The Memories (From persistent storage)
                          let history_snapshot = {
                              let s = state_bg.lock().unwrap();
                              s.chat_history.clone()
                          };

                          for saved in history_snapshot {
                              // Filter out any stale system instructions
                              if saved.content.starts_with("SYSTEM_INSTRUCTION") {
                                  continue;
                              }
                              context.push(Content {
                                  role: saved.role.clone(),
                                  parts: vec![Part { text: saved.content.clone() }]
                              });
                          }

                          // Call API with full context
                          match client.generate_content(&context).await {
                              Ok(response) => {
                                  let mut s = state_bg.lock().unwrap();
                                  s.console_output.push_str(&format!("\n[WOLFPACK] :: {}\n", response));

                                  // Add Model response to history
                                  s.chat_history.push(SavedMessage {
                                      role: "model".to_string(),
                                      content: response.clone(),
                                  });

                                  // SAVE POINT 2: Save immediately after Model response
                                  brain_bg.save(&s.chat_history);
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
    Backend::new("org.unaos.vein.evolution", app);
}
