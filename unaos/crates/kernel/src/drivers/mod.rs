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
// BCMA-RECON (GR20): read-only reconnaissance of the Broadcom WiFi radio (UNAOS_BCMARECON=1) —
// PCI class 0x02 SUBCLASS 0x80, the subclass every targeted walk in this kernel structurally cannot
// match, which is why the radio in the 2012 rMBP has never been looked at. Config reads + BAR0
// reads only. x86_64-only; knob off => the module is unlinked and media byte-identical.
#[cfg(all(target_arch = "x86_64", feature = "bcmarecon"))]
pub mod bcma;
pub mod block;
pub mod e1000;
// SDHC (milestone 2): the SD Host Controller driver on x86 — the 2012 rMBP's built-in PCIe card
// reader, which reaches a card WITHOUT xHCI and without USB Bulk-Only Transport. Milestone 1's
// read-only discovery witness still runs first and unchanged; milestone 2 then claims the function,
// resets the controller, programs bus power and the SD clock, and identifies the card. PIO only —
// no DMA, so the function's Bus Master bit is left as the firmware set it.
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
