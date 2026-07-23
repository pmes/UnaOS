#[cfg(feature = "nvidia-kepler")]
pub mod kepler;

#[cfg(feature = "intel-ivb")]
pub mod igpu;
pub mod detect;
