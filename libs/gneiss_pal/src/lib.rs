// libs/gneiss_pal/src/lib.rs (Updated)
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

#[cfg(feature = "gnome")]
pub use platforms::gnome::Backend;

#[cfg(all(feature = "gtk", not(feature = "gnome")))]
pub use platforms::gtk::Backend;

// --- ELESSAR MUTATION ---
// Re-export sourceview5 for consumers
pub mod prelude {
    pub use sourceview5::prelude::*;
    pub use sourceview5::View as SourceView;
    pub use sourceview5::{Buffer, StyleSchemeManager};
    pub use crate::types::*;
    pub use crate::shard::*;
    pub use crate::Backend; // Export Backend so vein can see new()
}
