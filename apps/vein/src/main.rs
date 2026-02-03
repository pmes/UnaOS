mod api;

// J7 FIX: Import GTK Prelude so we can use methods on Buffer/Adjustment
use gtk4::prelude::*;

use gneiss_pal::{
    AppHandler, Backend, DashboardState, Event, BrainManager,
    Shard, ShardRole, ShardStatus, SavedMessage, ViewMode, SidebarPosition
};
use crate::api::{GeminiClient, Content, Part};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use chrono::Local;

// --- State Wrapper for Thread Safety ---
struct VeinApp {
    state: Arc<Mutex<DashboardState>>,
    tx_logic: mpsc::Sender<String>, // Channel to send input to the async brain
    brain: BrainManager,
}

// --- The Brain Loop (Async Logic) ---
async fn run_brain(
    mut rx: mpsc::Receiver<String>,
    state: Arc<Mutex<DashboardState>>,
    brain_io: BrainManager
) {
    let client = GeminiClient::new();

    while let Some(user_input) = rx.recv().await {
        let is_s9 = user_input.trim().to_lowercase().starts_with("/s9");

        // 1. Update State (Thinking)
        {
            let mut s = state.lock().unwrap();
            s.console_output.push_str(&format!("\n[USER] {}\n", user_input));

            // Set Shard Status
            if let Some(shard) = s.shard_tree.iter_mut().find(|sh| sh.id == if is_s9 { "s9-mule" } else { "una-prime" }) {
                shard.status = ShardStatus::Thinking;
            }
        } // Lock drops

        // 2. Prepare AI Context
        let system_instruction = if is_s9 {
             "SYSTEM: You are S9-Mule. Write Rust code. No prose."
        } else {
             "SYSTEM: You are Una. Be terse. Be brilliant."
        };

        // Load history from disk for context
        let history = brain_io.load();
        let mut context = Vec::new();
        context.push(Content { role: "model".to_string(), parts: vec![Part::text(system_instruction.to_string())] });
        for msg in history {
            let role = if msg.role == "user" { "user" } else { "model" };
            context.push(Content { role: role.to_string(), parts: vec![Part::text(msg.content)] });
        }
        // Add current input
        context.push(Content { role: "user".to_string(), parts: vec![Part::text(user_input.clone())] });

        // 3. Call API
        let response = match client.generate_content(&context).await {
            Ok(text) => text,
            Err(e) => format!("Error: {}", e),
        };

        // 4. Update State (Response)
        let timestamp = Local::now().format("%H:%M:%S");
        let tag = if is_s9 { "[S9]" } else { "[UNA]" };
        let final_text = format!("\n{} [{}] :: {}\n", tag, timestamp, response);

        {
            let mut s = state.lock().unwrap();
            s.console_output.push_str(&final_text);

            // Restore Status
            if let Some(shard) = s.shard_tree.iter_mut().find(|sh| sh.id == if is_s9 { "s9-mule" } else { "una-prime" }) {
                shard.status = ShardStatus::Online;
            }
        }

        // 5. Save Memory
        let mut new_history = brain_io.load();
        new_history.push(SavedMessage { role: "user".to_string(), content: user_input });
        new_history.push(SavedMessage { role: "model".to_string(), content: response });
        brain_io.save(&new_history);
    }
}

impl AppHandler for VeinApp {
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Input(text) => {
                // Send to async brain
                let _ = self.tx_logic.blocking_send(text);
            }
            Event::NavSelect(idx) => {
                let mut s = self.state.lock().unwrap();
                s.active_nav_index = idx;
            }
            Event::ToggleSidebar => {
                let mut s = self.state.lock().unwrap();
                s.sidebar_collapsed = !s.sidebar_collapsed;
            }
            Event::TextBufferUpdate(buffer, adj) => {
                // Keep the UI buffer synced with state
                let s = self.state.lock().unwrap();
                if buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).as_str() != s.console_output {
                    buffer.set_text(&s.console_output);
                    // Auto-scroll
                    adj.set_value(adj.upper());
                }
            }
            _ => {}
        }
    }

    fn view(&self) -> DashboardState {
        self.state.lock().unwrap().clone()
    }
}

fn main() {
    // 1. Setup Brain & State
    let brain = BrainManager::new();
    let history = brain.load();

    // Initial Console State from History
    let mut initial_output = String::new();
    for msg in history {
        let tag = if msg.role == "user" { "[USER]" } else { "[UNA]" };
        initial_output.push_str(&format!("\n{} :: {}\n", tag, msg.content));
    }

    // Define Shards
    let mut root = Shard::new("una-prime", "Una-Prime", ShardRole::Root);
    root.status = ShardStatus::Online;
    let mut s9 = Shard::new("s9-mule", "S9-Mule", ShardRole::Builder);
    s9.status = ShardStatus::Offline;

    let initial_state = DashboardState {
        nav_items: vec!["Comms".into(), "Wolfpack".into(), "Forge".into()],
        active_nav_index: 0,
        console_output: initial_output,
        shard_tree: vec![root, s9], // Flat list for now, or build tree
        sidebar_position: SidebarPosition::Right,
        ..Default::default()
    };

    let state = Arc::new(Mutex::new(initial_state));

    // 2. Setup Channels & Async Runtime
    let (tx, rx) = mpsc::channel::<String>(100);

    let rt = Runtime::new().unwrap();
    let state_clone = state.clone();
    let brain_clone = brain.clone();

    // Spawn Background Brain
    thread::spawn(move || {
        rt.block_on(run_brain(rx, state_clone, brain_clone));
    });

    // 3. Launch UI
    let app = VeinApp {
        state,
        tx_logic: tx,
        brain: BrainManager::new(),
    };

    Backend::new("org.una.vein", app);
}
