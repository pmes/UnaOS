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
/// KFBIND (shut-out register §2, rung KF27) — derive the GK107 PBDMA register base from the
/// PTOP device-info table and read the channel's fetch pointers THERE instead of at the
/// legacy `0x40000 + i*0x2000` guess, with `IB_GET` (USERD +0x88) read for the first time in
/// the campaign as the falsifier. READ-ONLY: zero device writes, and the bind/enable write the
/// rung is named for is printed as SKIPPED with its uncited reason. `nvidia-kepler-kfbind`
/// implies `nvidia-kepler` AND `nvidia-kepler-fifo` in Cargo.toml — the second because both
/// call sites are inside `kepler::init`'s fifo leg — so this gate alone is sufficient, exactly
/// as `kepler_ce`'s is. DEFAULT OFF => the module and both call sites vanish and every
/// artifact is byte-identical.
#[cfg(feature = "nvidia-kepler-kfbind")]
pub mod kepler_fifo;

#[cfg(feature = "intel-ivb")]
pub mod igpu;
// GEN7-3D R1: the Ivy Bridge render-engine reconnaissance rung. Read-only, x86_64-only,
// DEFAULT OFF. `gen7` implies `intel-ivb` in Cargo.toml because the rung consumes the
// BAR0 window `igpu::init` maps and publishes.
#[cfg(feature = "gen7")]
pub mod gen7;
pub mod detect;
