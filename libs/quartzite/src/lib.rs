pub mod backend;
pub mod text;

pub use backend::Backend;

// Re-export specific logic types that UI might need directly
pub use gneiss_pal::shard::*;
pub use gneiss_pal::types::*;

use gtk4::prelude::*; // Required for Display/IconTheme traits

const EMBEDDED_RESOURCE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/quartzite.gresource"));

pub fn deploy_assets(path: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, EMBEDDED_RESOURCE)
}

pub fn init_with_path(path: &std::path::Path) {
    let resource = gtk4::gio::Resource::load(path).expect("Failed to load GResource from path");
    gtk4::gio::resources_register(&resource);

    if let Some(display) = gtk4::gdk::Display::default() {
        let theme = gtk4::IconTheme::for_display(&display);
        theme.add_resource_path("/org/unaos/lumen/icons");
    }
}

// Initialize function to setup resources and theme (Embedded fallback)
pub fn init() {
    // 1. Load the compiled binary from the OUT_DIR
    let res_bytes = glib::Bytes::from_static(EMBEDDED_RESOURCE);

    let resource = gtk4::gio::Resource::from_data(&res_bytes).expect("Failed to load GResource");

    gtk4::gio::resources_register(&resource);

    // 2. Register the Search Path
    if let Some(display) = gtk4::gdk::Display::default() {
        let theme = gtk4::IconTheme::for_display(&display);
        theme.add_resource_path("/org/unaos/lumen/icons");
    }
}
