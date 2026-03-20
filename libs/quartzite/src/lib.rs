// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Quartzite: The Diplomat's Bridge.
//!
//! This library acts as the universal translator between UnaOS's pure logic
//! and the messy, host-specific realities of the outside world. We do not
//! emulate the host; we abstract it. We are polite guests, but we maintain
//! our architectural purity.

pub mod platforms;
pub mod spline;
pub mod tetra;
pub mod text;
pub mod widgets;

// Re-export specific logic types that UI might need directly
pub use gneiss_pal::shard::*;
pub use gneiss_pal::types::*;
pub use spline::Spline;

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
pub struct NativeWindow {
    pub ptr: *mut std::ffi::c_void,
}
#[cfg(all(target_os = "linux", feature = "qt"))]
pub struct NativeView {
    pub ptr: cxx::UniquePtr<platforms::qt::ffi::LumenMainWindow>,
}
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

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", feature = "gtk"),
    all(target_os = "linux", feature = "qt")
)))]
impl Backend {
    pub fn new<F>(_app_id: &str, _bootstrap: F) -> Self {
        Backend
    }
    pub fn run(&self) {}
}

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
