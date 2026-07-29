pub mod pci;
pub mod xhci;
// EHCI-1 scout: read-only EHCI reconnaissance probe (UNAOS_EHCISCOUT=1). x86_64-only; the whole
// module is unlinked when the knob is off, keeping media byte-identical.
#[cfg(all(target_arch = "x86_64", feature = "ehciscout"))]
pub mod ehci_scout;
// EHCI-3: the minimal EHCI HID driver (UNAOS_EHCIHID=1) — the 2012 rMBP internal keyboard/
// trackpad live on non-switchable EHCI-only ports. Knob off => unlinked, media byte-identical.
#[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
pub mod ehci;
// BATMON-1: Apple SMC polled key/value driver (UNAOS_SMC=1) — the 2012 rMBP battery monitor.
// x86_64-only; knob off => the module is unlinked and media byte-identical.
#[cfg(all(target_arch = "x86_64", feature = "smc"))]
pub mod smc;
pub mod block;
pub mod e1000;
// SDHC-1 (milestone 1): read-only SD Host Controller discovery on x86 — the 2012 rMBP's built-in
// card reader. Version/capability/present-state reads only; issues no MMIO write at all, so it is
// unconditional on this arch (there is no state it can perturb).
#[cfg(target_arch = "x86_64")]
pub mod sdhc;
// M6g: the BCM2711 EMMC2/SDHCI microSD driver backing the block layer on the bare-metal Pi 4.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub mod emmc2;

#[cfg(all(target_arch = "x86_64", any(feature = "nvidia-kepler", feature = "intel-ivb")))]
pub mod gpu;

// BENCH-RIDE: read-only knob-gated evidence probes riding the rMBP sitting boots (therm/pcilink/
// vrom). x86_64-only; all knobs off => unlinked, media byte-identical.
#[cfg(all(target_arch = "x86_64", any(feature = "thermprobe", feature = "pcilink", feature = "vromprobe")))]
pub mod bench_ride;
