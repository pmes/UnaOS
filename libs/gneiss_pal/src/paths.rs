use std::path::PathBuf;

/// The Spatial Truth of UnaOS.
pub struct UnaPaths;

impl UnaPaths {
    /// The root of the organism.
    /// Bare-metal: `/`. Host-mode: `$UNA_ROOT` or XDG Data Local (`~/.local/share/unaos`).
    pub fn root() -> PathBuf {
        std::env::var("UNA_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                directories::BaseDirs::new()
                    .expect("CRITICAL: Alien Soil not supported. Host OS rejected spatial query.")
                    .data_local_dir()
                    .join("unaos")
            })
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
        Self::vault().join("lumen_storage.ufs")
    }

    /// Bootstraps the physical directory structure.
    pub fn awaken() -> std::io::Result<()> {
        std::fs::create_dir_all(Self::cortex())?;
        std::fs::create_dir_all(Self::vault())?;
        std::fs::create_dir_all(Self::config())?;
        Ok(())
    }
}
