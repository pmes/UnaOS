use std::env;
use std::fs;
use std::path::PathBuf;

/// The Spatial Truth of UnaOS.
pub struct UnaPaths;

impl UnaPaths {
    /// The root of the organism.
    /// Bare-metal: `/`. Host-mode: `$UNA_ROOT` or XDG Data Local (`~/.local/share/unaos`).
    pub fn root() -> PathBuf {
        if let Ok(root) = env::var("UNA_ROOT") {
            return PathBuf::from(root);
        }

        let mut vault = PathBuf::from(
            env::var("HOME").expect("CRITICAL: HOME environment variable missing. Engine stalled."),
        );
        vault.push(".local/share/unaos");
        vault
    }

    // --- The Organs ---

    /// The AI Cortex (Vein) - LLM Models and Vector DBs
    pub fn cortex() -> PathBuf {
        Self::root().join("cortex")
    }

    /// The Memory Vault (UnaFS) - The Encrypted Block Storage
    pub fn vault() -> PathBuf {
        Self::root().join("vault")
    }

    /// System Policy (Principia) - OS Configuration
    pub fn config() -> PathBuf {
        Self::root().join("principia")
    }

    /// The Lumen Storage File - The specific UnaFS block file
    pub fn lumen_storage() -> PathBuf {
        Self::root().join("lumen").join("lumen_storage.ufs")
    }

    /// Bootstraps the physical directory structure. Fails hard if the host rejects us.
    pub fn awaken() -> Result<(), String> {
        let required_nodes = [
            Self::cortex(),
            Self::vault(),
            Self::config(),
            Self::root().join("lumen"),
            Self::root().join("logs"), // Ensure Telemetry has a physical home
        ];

        for node in required_nodes {
            if !node.exists() {
                fs::create_dir_all(&node).map_err(|e| {
                    format!(
                        "CRITICAL: Failed to carve spatial anchor at {}: {}",
                        node.display(),
                        e
                    )
                })?;
            }
        }

        Ok(())
    }
}
