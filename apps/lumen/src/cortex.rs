use bandy::{SMessage, Synapse};
use gneiss_pal::paths::UnaPaths;
use std::path::PathBuf;

pub struct Cortex {
    pub vault_path: PathBuf,
}

impl Cortex {
    pub fn awaken(synapse: &mut Synapse) -> Self {
        let vault_path = UnaPaths::root().join("lumen");

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
