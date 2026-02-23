use std::path::PathBuf;
use unafs::FileSystem;
use crate::cortex::Cortex;
use bandy::{SMessage, Synapse};

/// Ignites the Lumen consciousness loop.
pub fn ignite(vault_path: PathBuf, mut synapse: Synapse) {
    // 1. Mount the Substrate
    let mut memory = FileSystem::mount(&vault_path)
        .expect("CRITICAL: Memory mount failed. Lumen is amnesiac.");

    // 2. Awaken the Cortex
    let mut cortex = Cortex::awaken(&mut synapse);

    // 3. Tap into the Nervous System
    let mut rx = synapse.rx();

    println!(">> [LUMEN] Synapses firing. Listening to the Plexus...");

    // The Synaptic Loop
    let rt = tokio::runtime::Runtime::new().expect("Failed to ignite Tokio runtime");
    rt.block_on(async move {
        loop {
            match rx.recv().await {
                Ok(SMessage::UserPrompt(text)) => {
                    println!(">> [LUMEN] Stimulus received: {}", text);
                    // Fire back across the nervous system
                    synapse.fire(SMessage::AiToken("Acknowledged.".to_string()));
                }
                Ok(SMessage::Kill(target)) if target == "lumen" => {
                    println!(">> [LUMEN] Shutting down cortex. Goodnight.");
                    break;
                }
                Ok(_) => {} // Ignore other stimuli
                Err(_) => {
                    println!(">> [LUMEN] Nervous system severed.");
                    break;
                }
            }
        }
    });
}
