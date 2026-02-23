use bandy::{SMessage, Synapse};
use std::collections::BTreeMap;
use std::path::PathBuf;
use unafs::{AttributeValue, FileDevice, FileSystem, UnaFS};

/// The Subconscious Vault.
/// Silently records the raw stimuli of the nervous system.
pub struct Cortex {
    pub vault_path: PathBuf,
    fs: FileSystem,
}

impl Cortex {
    pub fn awaken(vault_path: PathBuf, synapse: &mut Synapse) -> Self {
        // Ensure the substrate exists
        let device = FileDevice::open(&vault_path).unwrap_or_else(|_| {
            std::fs::File::create(&vault_path)
                .unwrap()
                .set_len(64 * 1024 * 1024)
                .unwrap();
            FileDevice::open(&vault_path).expect("CRITICAL: Failed to allocate Cortex substrate")
        });

        // Mount or Reformat
        let fs = match UnaFS::mount(device) {
            Ok(fs) => fs,
            Err(_) => {
                let dev = FileDevice::open(&vault_path).unwrap();
                UnaFS::format(dev, 64).expect("CRITICAL: Cortex lobotomy failed.")
            }
        };

        synapse.fire(SMessage::Log {
            level: "INFO".into(),
            source: "CORTEX".into(),
            content: format!("Deep subconscious online at {}", vault_path.display()),
        });

        Self { vault_path, fs }
    }

    /// Burns a raw memory into the UnaFS Substrate.
    pub fn imprint(&mut self, key: &str, data: &[u8]) {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "type".to_string(),
            AttributeValue::String("imprint".to_string()),
        );
        attrs.insert("key".to_string(), AttributeValue::String(key.to_string()));
        attrs.insert(
            "timestamp".to_string(),
            AttributeValue::Int(chrono::Utc::now().timestamp()),
        );

        if let Ok(inode) = self.fs.create_inode(attrs) {
            let _ = self.fs.write_data(inode, 0, data);
        }
    }
}
