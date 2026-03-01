//! The Embassies.
//!
//! Each module represents a sovereign territory we operate within.
//! We adapt to their customs (APIs) without compromising our core logic.

#[cfg(feature = "gtk")]
pub mod gtk;

#[cfg(feature = "gnome")]
pub mod gnome;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(feature = "qt")]
pub mod qt;
