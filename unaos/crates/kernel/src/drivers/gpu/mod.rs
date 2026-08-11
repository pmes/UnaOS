#[cfg(feature = "nvidia-kepler")]
pub mod kepler;
#[cfg(feature = "nvidia-kepler")]
pub mod kepler_display;
/// CE-LADDER — read-only copy-engine reconnaissance. `nvidia-kepler-ce` implies
/// `nvidia-kepler` in Cargo.toml, so this gate alone is sufficient. DEFAULT OFF => the
/// module and its single call site vanish and every artifact is byte-identical.
#[cfg(feature = "nvidia-kepler-ce")]
pub mod kepler_ce;

#[cfg(feature = "intel-ivb")]
pub mod igpu;
// GEN7-3D R1: the Ivy Bridge render-engine reconnaissance rung. Read-only, x86_64-only,
// DEFAULT OFF. `gen7` implies `intel-ivb` in Cargo.toml because the rung consumes the
// BAR0 window `igpu::init` maps and publishes.
#[cfg(feature = "gen7")]
pub mod gen7;
pub mod detect;
