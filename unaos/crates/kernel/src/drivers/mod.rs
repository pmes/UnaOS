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
pub mod block;
pub mod e1000;
// M6g: the BCM2711 EMMC2/SDHCI microSD driver backing the block layer on the bare-metal Pi 4.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub mod emmc2;
