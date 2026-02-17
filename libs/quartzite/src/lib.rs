pub mod backend;
pub mod text;

pub use backend::Backend;

// Re-export specific logic types that UI might need directly
pub use gneiss_pal::shard::*;
pub use gneiss_pal::types::*;

use gtk4::prelude::*; // Required for Display/IconTheme traits

// Initialize function to setup resources and theme
pub fn init() {
    // 1. Load the compiled binary from the OUT_DIR
    // This looks for the file created by build.rs
    let res_bytes = glib::Bytes::from_static(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/quartzite.gresource"
    )));

    let resource = gtk4::gio::Resource::from_data(&res_bytes).expect("Failed to load GResource");

    gtk4::gio::resources_register(&resource);

    // 2. Register the Search Path
    // This tells GTK: "If asked for an icon, look in this virtual folder too."
    if let Some(display) = gtk4::gdk::Display::default() {
        let theme = gtk4::IconTheme::for_display(&display);
        theme.add_resource_path("/org/unaos/lumen/icons");
    }
}
