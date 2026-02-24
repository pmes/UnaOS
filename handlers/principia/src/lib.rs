use anyhow::{Context, Result};
use bandy::{PrincipiaCommand, SMessage};
use std::path::{Path, PathBuf};
use std::fs;

pub struct Principia {
    current_root: Option<PathBuf>,
    config_path: PathBuf,
}

impl Principia {
    pub fn new() -> Self {
        let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config")).join("unaos");
        fs::create_dir_all(&config_dir).expect("Failed to create Principia config lobe");

        let config_path = config_dir.join("principia.toml");
        let current_root = fs::read_to_string(&config_path)
            .ok()
            .map(|s| PathBuf::from(s.trim()));

        Self { current_root, config_path }
    }

    /// The Synaptic Receiver
    pub fn process_impulse(&mut self, msg: &SMessage) -> Option<SMessage> {
        if let SMessage::Principia(PrincipiaCommand::SetSystemRoot(path)) = msg {
            if self.validate_root(path) {
                self.current_root = Some(path.clone());
                let _ = fs::write(&self.config_path, path.to_string_lossy().as_ref());

                // Fire the echo back across the bus
                return Some(SMessage::Principia(PrincipiaCommand::SystemRootChanged(path.clone())));
            }
        }
        None
    }

    #[inline]
    fn validate_root(&self, path: &Path) -> bool {
        // A valid UnaOS root must have a crates or libs directory
        path.exists() && path.is_dir() && (path.join("crates").exists() || path.join("libs").exists())
    }
}
