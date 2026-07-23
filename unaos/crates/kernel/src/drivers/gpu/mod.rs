#[cfg(feature = "nvidia-kepler")]
pub mod kepler;
#[cfg(feature = "nvidia-kepler")]
pub mod kepler_display;

#[cfg(feature = "intel-ivb")]
pub mod igpu;
pub mod detect;
