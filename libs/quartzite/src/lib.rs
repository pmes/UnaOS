//! Quartzite: The Diplomat's Bridge.
//!
//! This library acts as the universal translator between UnaOS's pure logic
//! and the messy, host-specific realities of the outside world. We do not
//! emulate the host; we abstract it. We are polite guests, but we maintain
//! our architectural purity.

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
// We strictly enforce target OS and feature flags to ensure zero cross-contamination.

// --- macOS (AppKit) ---
#[cfg(target_os = "macos")]
pub type NativeWindow = objc2_app_kit::NSWindow;
#[cfg(target_os = "macos")]
pub type NativeView = objc2::rc::Retained<objc2_app_kit::NSView>;
#[cfg(target_os = "macos")]
pub use platforms::macos::Backend;

// --- Windows 11+ (WinUI/Win32) ---
#[cfg(target_os = "windows")]
pub type NativeWindow = (); // Stub pending implementation
#[cfg(target_os = "windows")]
pub type NativeView = (); // Stub pending implementation
#[cfg(target_os = "windows")]
pub use platforms::windows::Backend;

// --- Linux (*nix) / GTK4 ---
#[cfg(all(target_os = "linux", feature = "gtk"))]
pub type NativeWindow = gtk4::ApplicationWindow;
#[cfg(all(target_os = "linux", feature = "gtk"))]
pub type NativeView = gtk4::Widget;
#[cfg(all(target_os = "linux", feature = "gtk"))]
pub use platforms::gtk::Backend;

// --- Linux (*nix) / Qt ---
#[cfg(all(target_os = "linux", feature = "qt"))]
pub type NativeWindow = (); // Stub pending implementation
#[cfg(all(target_os = "linux", feature = "qt"))]
pub type NativeView = (); // Stub pending implementation
#[cfg(all(target_os = "linux", feature = "qt"))]
pub use platforms::qt::Backend;

// --- Fallback (Headless / Testing) ---
#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", feature = "gtk"),
    all(target_os = "linux", feature = "qt")
)))]
pub type NativeWindow = ();
#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", feature = "gtk"),
    all(target_os = "linux", feature = "qt")
)))]
pub type NativeView = ();
#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", feature = "gtk"),
    all(target_os = "linux", feature = "qt")
)))]
pub struct Backend;

// -----------------------------------------------------------------------------
// ASSET DEPLOYMENT & INITIALIZATION
// -----------------------------------------------------------------------------

#[cfg(all(target_os = "linux", feature = "gtk"))]
const EMBEDDED_RESOURCE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/quartzite.gresource"));

#[cfg(all(target_os = "linux", feature = "gtk"))]
pub fn deploy_assets(path: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, EMBEDDED_RESOURCE)
}

#[cfg(all(target_os = "linux", feature = "gtk"))]
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

#[cfg(all(target_os = "linux", feature = "gtk"))]
pub fn init() {
    let res_bytes = glib::Bytes::from_static(EMBEDDED_RESOURCE);
    let resource = gtk4::gio::Resource::from_data(&res_bytes).expect("Failed to load GResource");

    gtk4::gio::resources_register(&resource);

    if let Some(display) = gtk4::gdk::Display::default() {
        let theme = gtk4::IconTheme::for_display(&display);
        theme.add_resource_path("/org/unaos/lumen/icons");
    }
}

// --- No-Op Fallbacks for Non-GTK Targets ---

#[cfg(not(all(target_os = "linux", feature = "gtk")))]
pub fn init() {
    // Handled natively by the host OS bundle (e.g., macOS App Bundle)
}

#[cfg(not(all(target_os = "linux", feature = "gtk")))]
pub fn deploy_assets(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(not(all(target_os = "linux", feature = "gtk")))]
pub fn init_with_path(_path: &std::path::Path) {
    // No-op
}
