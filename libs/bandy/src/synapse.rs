use crate::SMessage;
use tokio::sync::broadcast;

/// The connective tissue of the nervous system.
/// Uses a broadcast channel so multiple lobes (UI, Subconscious, AI)
/// can react to the same stimulus simultaneously.
#[derive(Clone)]
pub struct Synapse {
    tx: broadcast::Sender<SMessage>,
}

impl Synapse {
    pub fn new() -> Self {
        // 1024 action potentials in flight. If we hit this, the system is seizing.
        let (tx, _) = broadcast::channel(1024);
        Self { tx }
    }

    /// Fires a stimulus across the nervous system.
    pub fn fire(&self, msg: SMessage) {
        // We ignore SendError. If a tree falls in the forest...
        let _ = self.tx.send(msg);
    }

    /// Direct access to the transmitter.
    pub fn tx(&self) -> broadcast::Sender<SMessage> {
        self.tx.clone()
    }

    /// Sprout a new nerve ending to listen to the system.
    pub fn rx(&self) -> broadcast::Receiver<SMessage> {
        self.tx.subscribe()
    }
}

impl Default for Synapse {
    fn default() -> Self {
        Self::new()
    }
}
