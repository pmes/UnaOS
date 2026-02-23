use std::path::PathBuf;
use unafs::FileSystem;
use vein::Cortex;
use bandy::{Node, SMessage};

/// Ignites the Lumen consciousness loop.
pub fn ignite(vault_path: PathBuf) {
    // 1. Mount the Substrate
    let mut memory = FileSystem::mount(&vault_path)
        .expect("CRITICAL: Memory mount failed. Lumen is amnesiac.");

    // 2. Awaken the Cortex
    let mut cortex = Cortex::awaken()
        .expect("CRITICAL: Vein cortex failed to ignite. Brain dead.");

    // 3. Tap into the Nervous System
    let mut nerve = Node::bind("lumen")
        .expect("CRITICAL: Bandy IPC binding failed. Lumen is deaf and mute.");

    println!(">> [LUMEN] Synapses firing. Listening to the Plexus...");

    // The Synaptic Loop
    loop {
        match nerve.poll() {
            Some(SMessage::Prompt { id, text }) => {
                println!(">> [LUMEN] Stimulus received: {}", text);

                // Fetch memories, inject into prompt, fire the LLM
                let context = memory.read_context_for(&text);
                let response = cortex.stimulate(&text, context);

                // Fire back across the nervous system
                nerve.send(id, SMessage::Reply { text: response });
            }
            Some(SMessage::Store { key, payload }) => {
                memory.write(&key, &payload).expect("Failed to engrave memory.");
                println!(">> [LUMEN] Memory engraved: {}", key);
            }
            Some(SMessage::Halt) => {
                println!(">> [LUMEN] Shutting down cortex. Goodnight.");
                memory.sync();
                break;
            }
            _ => std::thread::yield_now(), // Keep the engine idling hot
        }
    }
}
