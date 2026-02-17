pub mod backend;
pub mod text;

pub use backend::Backend;

// Re-export specific logic types that UI might need directly
pub use gneiss_pal::shard::*;
pub use gneiss_pal::types::*;

// --- ASSETS ---
static RESOURCES_BYTES: &[u8] = include_bytes!("../assets/resources.gresource");

pub fn register_resources() {
    let bytes = glib::Bytes::from_static(RESOURCES_BYTES);
    let res = gtk4::gio::Resource::from_data(&bytes).expect("Failed to load resources");
    gtk4::gio::resources_register(&res);
}

// Initialize function to setup resources and theme
pub fn init() {
    register_resources();
}
