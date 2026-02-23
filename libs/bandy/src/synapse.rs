use crate::SMessage;
use async_channel::{unbounded, Receiver, Sender};

/// The connective tissue of the nervous system.
#[derive(Clone)]
pub struct Synapse {
    tx: Sender<SMessage>,
    rx: Receiver<SMessage>,
}

impl Synapse {
    pub fn new() -> Self {
        let (tx, rx) = unbounded();
        Self { tx, rx }
    }

    /// Fires a stimulus across the nervous system.
    pub fn fire(&self, msg: SMessage) {
        if let Err(e) = self.tx.send_blocking(msg) {
            eprintln!(">> [SYNAPSE FAULT] Failed to fire message: {}", e);
        }
    }

    pub fn tx(&self) -> Sender<SMessage> {
        self.tx.clone()
    }

    pub fn rx(&self) -> Receiver<SMessage> {
        self.rx.clone()
    }
}

impl Default for Synapse {
    fn default() -> Self {
        Self::new()
    }
}
