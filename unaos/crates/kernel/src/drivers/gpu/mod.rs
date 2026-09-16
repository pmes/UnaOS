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
///
/// ⚠ THE GATE CARRIES AN `any(...)` BECAUSE THIS FILE NOW HOLDS TWO RUNGS. KFCTXBIND (§2 rung
/// KF28, `nvidia-kepler-kfctxbind`) lives inside it as `mod ctxbind` and DELIBERATELY DOES NOT
/// IMPLY `nvidia-kepler-kfbind`: `KEPLER-METAL-LOG.md`'s KF28 entry forbids flying the two
/// rungs on the same boot, so KF28's dependency on KF27 is a RUNTIME gate (it re-runs KF27's
/// base ladder read-only and scores the verdict) and never a Cargo one. Naming only KFBIND
/// made `UNAOS_KEPLER_KFCTXBIND=1` a build error — measured: `E0433: cannot find kepler_fifo
/// in gpu ... gated behind the nvidia-kepler-kfbind feature`, and `check`'s derived
/// `x86-mix-*` legs draw features from the Cargo universe by mask bit, so half of them compile
/// exactly that configuration.
///
/// ⛔ AND THE TWO PARENTS ARE SPELLED OUT, which is not redundancy — GATE-FC2 convicted the
/// short form. Both features imply `nvidia-kepler` + `nvidia-kepler-fifo` in Cargo, so this
/// `all(...)` admits exactly the configurations the bare `any(...)` did; the difference is that
/// `must_all()` takes NO required atom from an `any(...)`, so the declaration read as wider
/// than all four call sites (which sit inside `kepler::init`'s fifo leg) and fc2 reported
/// `decl=target_arch = "x86_64" ... all-under=nvidia-kepler + nvidia-kepler-fifo -> FINDING`.
/// The declaration now STATES what was already true of it. Default OFF either way: the new
/// feature is brand-new, so no existing build moves.
///
/// ⚠ THE `any(...)` NOW NAMES THREE RUNGS. KFUNWEDGE (§2 rung KF29,
/// `nvidia-kepler-kfunwedge`) lives in this file as `mod unwedge`, beside `mod ctxbind`, and
/// DELIBERATELY DOES NOT IMPLY EITHER OF THE OTHER TWO: it is a SACRIFICIAL rung that fires
/// the `0x409504` poison on purpose, and a boot carrying KF27 or KF28 with it would collect
/// their verdicts over a unit this rung had already wedged. Its dependency on KF27's base
/// ladder is therefore a RUNTIME gate — it re-runs that ladder read-only — exactly as KF28's
/// is, and never a Cargo one. Omitting the atom here would make `UNAOS_KEPLER_KFUNWEDGE=1`
/// alone a build error, the same `E0433` the KFCTXBIND note above measured. Both parents stay
/// spelled out for GATE-FC2's reason: all three features imply `nvidia-kepler` +
/// `nvidia-kepler-fifo`, so this `all(...)` admits exactly the configurations the bare
/// `any(...)` would. Default OFF: the feature is brand-new, so no existing build moves.
#[cfg(all(feature = "nvidia-kepler", feature = "nvidia-kepler-fifo", any(feature = "nvidia-kepler-kfbind", feature = "nvidia-kepler-kfctxbind", feature = "nvidia-kepler-kfunwedge")))]
pub mod kepler_fifo;

#[cfg(feature = "intel-ivb")]
pub mod igpu;
// GEN7-3D R1: the Ivy Bridge render-engine reconnaissance rung. Read-only, x86_64-only,
// DEFAULT OFF. `gen7` implies `intel-ivb` in Cargo.toml because the rung consumes the
// BAR0 window `igpu::init` maps and publishes.
#[cfg(feature = "gen7")]
pub mod gen7;
pub mod detect;
