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
            env::var("HOME").expect("CRITICAL: HOME environment variable missing. Engine stalled.")
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

    /// Bootstraps the physical directory structure.
    pub fn awaken() -> std::io::Result<()> {
        fs::create_dir_all(Self::cortex())?;
        fs::create_dir_all(Self::vault())?;
        fs::create_dir_all(Self::config())?;
        fs::create_dir_all(Self::root().join("lumen"))?;
        Ok(())
    }
}
