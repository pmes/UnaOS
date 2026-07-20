#[cfg(feature = "nvidia-kepler")]
pub mod kepler;

#[cfg(feature = "intel-ivb")]
pub mod ivb;

pub mod detect;
