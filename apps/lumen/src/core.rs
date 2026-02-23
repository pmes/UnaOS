use crate::cortex::Cortex;
use bandy::{SMessage, Synapse};
use std::path::PathBuf;
use tokio::sync::broadcast::error::RecvError;

/// The Autonomous Loop.
/// Runs silently in the background, absorbing the nervous system's telemetry.
pub async fn ignite(vault_path: PathBuf, mut synapse: Synapse) {
    let mut cortex = Cortex::awaken(vault_path, &mut synapse);
    let mut rx = synapse.rx();

    log::info!(">> [LUMEN CORE] Synapses firing. Autonomous loop engaged.");

    loop {
        match rx.recv().await {
            Ok(msg) => match msg {
                SMessage::UserPrompt(text) => {
                    cortex.imprint("stimulus.prompt", text.as_bytes());
                }
                SMessage::FileEvent { path, event } => {
                    let payload = format!("{}|{}", path, event);
                    cortex.imprint("stimulus.fs", payload.as_bytes());
                }
                SMessage::Kill(target) if target == "lumen" => {
                    log::warn!(">> [LUMEN CORE] Kill signal received. Severing.");
                    break;
                }
                SMessage::Log { source, content, .. } => {
                    let payload = format!("{}: {}", source, content);
                    cortex.imprint("stimulus.log", payload.as_bytes());
                }
                _ => {} // The subconscious absorbs the noise.
            },
            Err(RecvError::Lagged(skipped)) => {
                log::warn!(">> [LUMEN CORE] Synapse overloaded. Skipped {} stimuli.", skipped);
            }
            Err(RecvError::Closed) => {
                log::warn!(">> [LUMEN CORE] Nervous system severed. Shutting down.");
                break;
            }
        }
    }
}
