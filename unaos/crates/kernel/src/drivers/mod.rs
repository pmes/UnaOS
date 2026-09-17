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

// AHCI (rmbp-ledger B89, first rung): the SATA host controller — READ-ONLY (UNAOS_AHCI=1). Enumerate
// implemented ports, IDENTIFY the disks, read sectors, publish under `BlockHandle::Ahci`. No WRITE
// opcode is compiled into this file and `install/` is never told about the handle (B91: an installer
// that could target the internal SSD is one slip from erasing Catalina). x86_64-gated because the
// enumeration seam is `arch::pci` config space; the FILE is arch-neutral in name and shape — AHCI
// exists on aarch64 boards too, and only `hba_take`'s BAR read is x86. Knob off => the module is
// never lexed (the `#[cfg]`-erased `pub mod` is the one case LAWS §5 names as byte-safe) and media
// are byte-identical. DECLARED LAST so the knob-off line numbering of every module above is
// untouched.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
pub mod ahci;

// HDA (rmbp-ledger B127): the Intel High Definition Audio controller — the kernel's first audio
// line (UNAOS_HDA=1 for the census/reset/CORB-RIRB/widget walk, UNAOS_HDATONE=1 for the output
// stream). x86_64-gated because the enumeration seam is `arch::pci` config space; the FILE is
// arch-neutral in name and shape — HDA exists on other machines and the Pi's HDMI/PWM audio is its
// twin, not its replacement (LAWS §3, ONE OS). Knob off => the module is never lexed (the
// `#[cfg]`-erased `pub mod` is the one case LAWS §5 names as byte-safe) and media are
// byte-identical. DECLARED LAST, after `ahci` and for the same reason it was: the knob-off line
// numbering of every module above is untouched.
#[cfg(all(target_arch = "x86_64", feature = "hda"))]
pub mod hda;
