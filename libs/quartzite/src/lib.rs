pub mod platforms;
pub mod text;

// Re-export specific logic types that UI might need directly
pub use gneiss_pal::shard::*;
pub use gneiss_pal::types::*;

// -----------------------------------------------------------------------------
// THE DIPLOMAT'S BRIDGE: NATIVE ABSTRACTIONS
// -----------------------------------------------------------------------------
// These type aliases allow our core applications to write unified bootstrap
// closures while quartzite handles the platform-specific memory and types.

#[cfg(not(target_os = "macos"))]
pub type NativeWindow = gtk4::ApplicationWindow;
#[cfg(not(target_os = "macos"))]
pub type NativeView = gtk4::Widget;

#[cfg(target_os = "macos")]
pub type NativeWindow = objc2_app_kit::NSWindow;
#[cfg(target_os = "macos")]
// Retained ensures we safely cross the Objective-C ARC memory boundary.
pub type NativeView = objc2::rc::Retained<objc2_app_kit::NSView>;

// -----------------------------------------------------------------------------
// PLATFORM ROUTING
// -----------------------------------------------------------------------------
#[cfg(not(target_os = "macos"))]
pub use platforms::gtk::Backend;

#[cfg(target_os = "macos")]
pub use platforms::macos::Backend;

#[cfg(not(target_os = "macos"))]
const EMBEDDED_RESOURCE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/quartzite.gresource"));

#[cfg(not(target_os = "macos"))]
pub fn deploy_assets(path: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, EMBEDDED_RESOURCE)
}

#[cfg(not(target_os = "macos"))]
pub fn init_with_path(path: &std::path::Path) {
    println!(":: QUARTZITE :: Loading assets from: {}", path.display());

    let resource = gtk4::gio::Resource::load(path).expect("Failed to load GResource from path");
    gtk4::gio::resources_register(&resource);

    if let Some(display) = gtk4::gdk::Display::default() {
        let theme = gtk4::IconTheme::for_display(&display);
        theme.add_resource_path("/org/unaos/lumen/icons");
        println!(":: QUARTZITE :: Search Path Added: /org/unaos/lumen/icons");
    }
}

// Initialize function to setup resources and theme (Embedded fallback)
#[cfg(not(target_os = "macos"))]
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

// Dummy init functions for macOS to prevent breaking API contracts
#[cfg(target_os = "macos")]
pub fn init() {
    // macOS resources are handled by the app bundle or embedded differently.
}

#[cfg(target_os = "macos")]
pub fn deploy_assets(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn init_with_path(_path: &std::path::Path) {
    // No-op
}
