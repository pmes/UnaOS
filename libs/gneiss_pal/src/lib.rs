#![allow(deprecated)]

pub mod persistence;
pub mod shard;
pub mod types;
mod platforms;
mod text;

// Re-export types so consumers (Vein) see them at the root
pub use types::*;
pub use shard::{Shard, ShardRole, ShardStatus, Heartbeat};

// --- ASSETS ---
static RESOURCES_BYTES: &[u8] = include_bytes!("../assets/resources.gresource");

pub fn register_resources() {
    let bytes = glib::Bytes::from_static(RESOURCES_BYTES);
    let res = gtk4::gio::Resource::from_data(&bytes).expect("Failed to load resources");
    gtk4::gio::resources_register(&res);
}

// --- PLATFORM SWITCHBOARD ---

// Priority: Gnome > GTK
// If "gnome" feature is enabled, use it (even if "gtk" is also enabled via default)
#[cfg(feature = "gnome")]
pub use platforms::gnome::Backend;

// Fallback: Use "gtk" only if "gnome" is NOT enabled
#[cfg(all(feature = "gtk", not(feature = "gnome")))]
pub use platforms::gtk::Backend;
