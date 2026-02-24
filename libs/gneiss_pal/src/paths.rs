use std::env;
use std::fs;
use std::path::PathBuf;

/// The Spatial Truth of UnaOS.
pub struct UnaPaths;

impl UnaPaths {
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

    /// The AI Cortex (Vein) - LLM Models and Subconscious
    pub fn cortex() -> PathBuf {
        Self::root().join("cortex")
    }

    /// The Subconscious Vault (Raw Telemetry)
    pub fn subconscious_vault() -> PathBuf {
        Self::cortex().join("subconscious.ufs")
    }

    /// The Memory Vault (UnaFS) - The Encrypted Block Storage
    pub fn vault() -> PathBuf {
        Self::root().join("vault")
    }

    /// The Primary Conscious Memory (Replaces lumen_storage)
    pub fn primary_vault() -> PathBuf {
        Self::vault().join("primary.ufs")
    }

    /// System Policy (Principia) - OS Configuration
    pub fn config() -> PathBuf {
        Self::root().join("principia")
    }

    /// Bootstraps the physical directory structure. Fails hard if the host rejects us.
    pub fn awaken() -> Result<(), String> {
        let required_nodes = [
            Self::cortex(),
            Self::vault(),
            Self::config(),
            Self::root().join("logs"),
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
