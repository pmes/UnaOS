pub mod pci;
pub mod xhci;
pub mod block;
pub mod e1000;
// M6g: the BCM2711 EMMC2/SDHCI microSD driver backing the block layer on the bare-metal Pi 4.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub mod emmc2;
