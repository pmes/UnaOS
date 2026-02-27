#[cfg(target_os = "macos")]
pub mod macos_view;
#[cfg(target_os = "linux")]
pub mod telemetry;

#[cfg(target_os = "linux")]
pub mod model;

#[cfg(target_os = "linux")]
pub mod view;
