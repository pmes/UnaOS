use serde::{Deserialize, Serialize};

/// SMessage (The Shard Message).
/// The atomic unit of truth in UnaOS.
/// This Enum defines the limits of what can be said between processes.
///
/// EVOLUTION PROTOCOL:
/// 1. Add variant here.
/// 2. Update handlers/vein/src/nerve.rs (The Brain).
/// 3. Update apps/lumen/src/main.rs (The Dispatch).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SMessage {
    // --- SYSTEM HEARTBEAT ---
    /// A ping to check if a Shard is alive.
    Ping,
    /// A command to terminate a Shard.
    Kill(String),
    /// A structured log entry. (Does not require `log` crate).
    Log {
        level: String,
        source: String,
        content: String
    },

    // --- RESONANCE (The Voice) ---
    /// Raw audio data passed from the DSP to the AI or Network.
    AudioChunk {
        source_id: String,
        samples: Vec<f32>,
        sample_rate: u32
    },
    /// Frequency domain data for visualization.
    Spectrum {
        magnitude: Vec<f32>
    },

    // --- VEIN / LUMEN (The Mind) ---
    /// A prompt typed or spoken by the user.
    UserPrompt(String),
    /// A token stream from the LLM.
    AiToken(String),
    /// A request for the AI to analyze a specific context.
    AnalyzeContext {
        id: String,
        content: String,
    },

    // --- UNAFS / MATRIX (The Memory) ---
    /// Notification that a file has changed.
    FileEvent {
        path: String,
        event: String // e.g., "Created", "Modified"
    },

    // --- MIDDEN (The Terminal) ---
    /// No operation.
    NoOp,
    /// Standard output from the terminal.
    TerminalOutput(String),
    /// Standard error from the terminal.
    TerminalError(String),
    /// A generic file system event message (unstructured).
    FileSystemEvent(String),
}

/// The trait that defines a "Nerve Ending" in the system.
/// Any struct implementing this can send/receive SMessages.
pub trait BandyMember {
    /// Publish a message to a specific topic.
    /// TODO: Implement transport layer (rumqttc / zeromq / shared_memory).
    fn publish(&self, topic: &str, msg: SMessage) -> anyhow::Result<()>;

    // Note: Subscription will be added when we define the async runtime model.
}
