#[cfg(feature = "nvidia-kepler")]
pub mod kepler;
/// PCIH — PCIe link-health witness for the BAR1 wedge theory: boot-time endpoint/root-port
/// link census, the `noaspm` LNKCTL[1:0] kill switch, and the root-port-only wedge-time
/// sampler `wm::wcser_overdue_probe` calls at its tripwire. Rides every kepler boot;
/// x86_64-only in effect (aarch64 gets inline shims so an armed aarch64 type-check stays green).
#[cfg(feature = "nvidia-kepler")]
pub mod pcihealth;
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
