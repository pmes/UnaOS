use bandy::{SMessage, Synapse};
use directories::BaseDirs;
use std::path::{Path, PathBuf};

pub struct Cortex {
    pub vault_path: PathBuf,
}

impl Cortex {
    pub fn awaken(synapse: &mut Synapse) -> Self {
        let base_dirs = BaseDirs::new().expect("CRITICAL: Alien Soil not supported");
        let vault_path = base_dirs.data_local_dir().join("unaos").join("lumen");

        if !vault_path.exists() {
            std::fs::create_dir_all(&vault_path).expect("CRITICAL: Failed to establish base camp");
        }

        synapse.fire(SMessage::Log {
            level: String::from("INFO"),
            source: String::from("LUMEN_CORTEX"),
            content: format!("Cortex Online. Vault anchored at: {}", vault_path.display()),
        });

        Self { vault_path }
    }

    pub fn imprint(&mut self, key: &str, _data: &[u8]) {
        // TODO: Wire to unafs::Vault when the crate is ready.
        // For now, we acknowledge the imprint in the nervous system.
        println!(":: CORTEX :: Imprinted [{}]", key);
    }
}
