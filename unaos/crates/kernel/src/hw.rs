//! CHARTER: Kernel — driver (the hardware census: one table read from the drivers' own registries; HWREPORT rmbp-ledger B725)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HWREPORT (rmbp-ledger B725, BP-bar-25: the user's hardware is the user's — what the OS talks to is shown, by name,
//! with its state; a device no driver claimed says so). [`census`] is ONE table: every device the kernel enumerated,
//! one [`Row`] each — its bus, its ids, where it sits, the driver that claimed it (by module name) or `none`, and its
//! state. It is BUILT FROM THE REGISTRIES the drivers already keep and issues no transfer and no bus walk of its own:
//!
//! * PCI — `arch::pci::functions()`, the function table the boot's PCI init recorded ONCE (config reads, MF-gated).
//!   The claim is read from each driver's own record: the xHCI the controller slot holds (`xhci::claim`), the EHCI
//!   controllers `ehci::census` names, the NIC `e1000::info`, the radio `wifi::bus::found`, the GPUs by vendor under
//!   their knob, HDA `hda::vol::ready`, AHCI `ahci::disk_count`, the SD host `sdhc::card_num_blocks`; a bridge is
//!   the PCI layer's own (`pci`).
//! * USB — the xHCI slot table (`xhci::census_slots`: the hub / HID / BOT claims the controller holds) and each class
//!   driver's own slot or id (`usbnet::slot`, `isoch::in_slot` for UVC, `ptp::vid_pid`, `uac::device`, FTDI's
//!   `0403:6001` + `ftdi::is_live`); the EHCI HID endpoints (`ehci::census`).
//! * Block — the SATA registry (`block::ahci_disk`, the model from the SMARTLINE cache) and the SD card.
//! * SMC — `smc::present`; ACPI — the MADT's processor count.
//!
//! States: `up` (a driver claimed it and serves it) · `held` (a driver claimed it and holds it short of service: a
//! bring-up pending, no medium, the glass still the firmware's) · `absent` (a registry entry whose device is gone) ·
//! `refused:<why>` (`no-driver`: nothing in the tree drives it yet; `halted`: the driver gave the endpoint up;
//! `no-descriptor`: the device never answered its descriptor).
//!
//! Wire: `[hw] census devices=<n> driven=<n> none=<n>` per completed census; `tests hwreport` prints one `[hw] row`
//! per device and `:: HWREPORT: devices=<n> driven=<n> none=<n> rows_ok=1 -> PASS ::`.

use alloc::string::String;
use alloc::vec::Vec;

/// One device, as the census lists it.
#[derive(Clone)]
pub struct Row {
    /// `pci` · `usb` · `sata` · `sd` · `smc` · `acpi`.
    pub bus: &'static str,
    /// `vid:pid` for PCI and USB, else the class token (`hid:kbd`, `port0`, `card`, …).
    pub id: String,
    /// Where it sits: the PCI `bus:dev.fn`, the USB slot and port, the HBA index.
    pub at: String,
    /// What it is, in words.
    pub name: String,
    /// The module that claimed it, or `none`.
    pub driver: &'static str,
    /// `up` · `held` · `absent` · `refused:<why>`.
    pub state: &'static str,
}

/// How many devices each registry holds — the denominator `tests hwreport` checks the rows against.
#[derive(Clone, Copy, Default)]
pub struct Counts {
    pub pci: usize,
    pub usb: usize,
    pub block: usize,
    pub other: usize,
}

/// One census: the rows, the registries' own counts, and whether a registry was held (the USB loan or the EHCI
/// table busy: its rows are missing from this census, never guessed).
#[derive(Clone)]
pub struct Census {
    pub rows: Vec<Row>,
    pub reg: Counts,
    pub busy: bool,
}

impl Census {
    /// Devices a driver claimed.
    pub fn driven(&self) -> usize {
        self.rows.iter().filter(|r| r.driver != "none").count()
    }
}

/// The last completed census (`busy` = false), for a reader that may not wait (the Settings paint).
static LAST: crate::sync::Mutex<Option<Census>> = crate::sync::Mutex::new(None);

/// The last completed census, copied out under `try_lock` (held = none this frame). Never builds one.
pub fn last() -> Option<Census> {
    LAST.try_lock().and_then(|g| g.clone())
}

const NONE: &str = "none";
const NO_DRIVER: &str = "refused:no-driver";

/// Build the census. `wait`: a held registry (the xHCI loan, the EHCI table) is retried, bounded, unmasked only
/// (the shell verb and the fixture); without it a held registry marks the census `busy` (the Settings service pass,
/// which asks again on its next pass). A completed census prints `[hw] census …` and becomes [`last`].
pub fn census(wait: bool) -> Census {
    let mut c = Census { rows: Vec::new(), reg: Counts::default(), busy: false };
    pci_rows(&mut c, wait);
    usb_rows(&mut c, wait);
    block_rows(&mut c);
    other_rows(&mut c);
    if !c.busy {
        let d = c.driven();
        serial_println!("[hw] census devices={} driven={} none={}", c.rows.len(), d, c.rows.len() - d);
        if let Some(mut g) = LAST.try_lock() {
            *g = Some(c.clone());
        }
    }
    c
}

/// One row as a line: the shell verb's and the fixture's shape.
pub fn line(r: &Row) -> String {
    let drv = if r.driver == NONE { "none" } else { r.driver };
    alloc::format!("{:<4} {:<10} {:<14} {:<8} {:<18} {}", r.bus, r.id, r.at, drv, r.state, r.name)
}

/// The `hw` verb's body: the table, then the census line's numbers.
pub fn shell_verb(console: &mut crate::console::Console) {
    let c = census(true);
    console.println(&alloc::format!("{:<4} {:<10} {:<14} {:<8} {:<18} {}", "bus", "id", "at", "driver", "state", "device"));
    for r in &c.rows {
        console.println(&line(r));
    }
    let d = c.driven();
    console.println(&alloc::format!(
        "{} devices, {} with a driver, {} without{}",
        c.rows.len(), d, c.rows.len() - d, if c.busy { " (USB busy: its devices are not listed, retry)" } else { "" }
    ));
}

fn push(c: &mut Census, bus: &'static str, id: String, at: String, name: String, driver: &'static str, state: &'static str) {
    c.rows.push(Row { bus, id, at, name, driver, state });
}

/// A held registry's bounded retry: unmasked callers only, one `hlt` per try.
fn retry<T>(wait: bool, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let tries = if wait && !crate::arch::irqs_masked() { 400 } else { 1 };
    for i in 0..tries {
        if let Some(v) = f() {
            return Some(v);
        }
        if i + 1 < tries {
            crate::hlt();
        }
    }
    None
}

// ── PCI ─────────────────────────────────────────────────────────────────────────────────────────────────────────

/// The class in words (PCI Code and ID Assignment, the classes this kernel meets).
#[cfg(any(target_arch = "x86_64", feature = "pcie2"))] // ORINPCIEROWS (LEDGER SR170): the aarch64 PCIe rows name their class here too. LINE-NEUTRAL, code before the comment.
fn pci_class_name(class: u8, sub: u8, progif: u8) -> &'static str {
    match (class, sub) {
        (0x01, 0x06) => "SATA controller",
        (0x01, 0x08) => "NVMe controller",
        (0x01, _) => "Storage controller",
        (0x02, 0x00) => "Ethernet controller",
        (0x02, 0x80) => "Wireless network controller",
        (0x02, _) => "Network controller",
        (0x03, _) => "Display controller",
        (0x04, 0x03) => "Audio controller",
        (0x04, _) => "Multimedia controller",
        (0x05, _) => "Memory controller",
        (0x06, 0x00) => "Host bridge",
        (0x06, 0x01) => "ISA bridge",
        (0x06, 0x04) => "PCI bridge",
        (0x06, _) => "Bridge",
        (0x07, _) => "Communication controller",
        (0x08, 0x05) => "SD host controller",
        (0x08, _) => "System peripheral",
        (0x0C, 0x00) => "FireWire controller",
        (0x0C, 0x03) => match progif {
            0x30 => "USB 3 controller (xHCI)",
            0x20 => "USB 2 controller (EHCI)",
            0x10 => "USB controller (OHCI)",
            0x00 => "USB controller (UHCI)",
            _ => "USB controller",
        },
        (0x0C, 0x05) => "SMBus controller",
        (0x0C, 0x0A) => "Thunderbolt controller",
        (0x0C, _) => "Serial bus controller",
        (0x0D, _) => "Wireless controller",
        (0x11, _) => "Signal processing controller",
        _ => "PCI device",
    }
}

#[cfg(target_arch = "x86_64")]
fn pci_rows(c: &mut Census, wait: bool) {
    let fns = crate::arch::pci::functions();
    c.reg.pci = fns.len();
    // The xHCI the kernel brought up is the FIRST 0x0C/0x03/0x30 function in walk order (`PciScanner::scan`
    // returns on it); the controller slot says whether it is installed.
    let xhci_up = !matches!(crate::drivers::xhci::claim(), Err(crate::drivers::xhci::XhciClaimError::NotReady));
    let mut xhci_seen = false;
    // The NIC `init_network` hands to the e1000 is the FIRST 0x02/0x00 function (`find_device(0x02, 0x00)`).
    let mut enet_seen = false;
    #[cfg(feature = "ehcihid")]
    let ehci: Vec<(u8, u8, u8)> = retry(wait, crate::drivers::ehci::census).map(|v| v.into_iter().map(|(b, _)| b).collect()).unwrap_or_default();
    for f in &fns {
        let bdf = (f.bus, f.dev, f.func);
        let (mut driver, mut state) = (NONE, NO_DRIVER);
        match (f.class, f.sub, f.progif) {
            (0x0C, 0x03, 0x30) if !xhci_seen && crate::drivers::xhci::picked().is_none_or(|p| p == bdf) => { // ZENBOOKXHCIPICK (zenbook-ledger Z7): the PICKED controller is the one labelled `xhci` (`xhci::pick`, `UNAOS_XHCI_PICK`); no pick recorded = the first, as before. LINE-NEUTRAL.
                xhci_seen = true;
                (driver, state) = ("xhci", if xhci_up { "up" } else { "held" });
            }
            #[cfg(feature = "ehcihid")]
            (0x0C, 0x03, 0x20) if ehci.contains(&bdf) => (driver, state) = ("ehci", "up"),
            (0x02, 0x00, _) if !enet_seen => {
                enet_seen = true;
                if crate::drivers::e1000::info().is_some() {
                    (driver, state) = ("e1000", "up");
                }
            }
            #[cfg(feature = "wifi")]
            (0x02, _, _) if crate::wifi::bus::found().is_some_and(|w| (w.bus, w.slot, w.func) == bdf) => {
                (driver, state) = ("wifi", if crate::wifi::status::ucode_ok() { "up" } else { "held" });
            }
            #[cfg(feature = "nvidia-kepler")]
            (0x03, _, _) if f.vendor == 0x10DE => {
                #[cfg(feature = "nvidia-kepler-takeover")]
                let on = crate::drivers::gpu::kepler_gpublit::armed();
                #[cfg(not(feature = "nvidia-kepler-takeover"))]
                let on = false;
                (driver, state) = ("kepler", if on { "up" } else { "held" });
            }
            #[cfg(feature = "intel-ivb")]
            (0x03, _, _) if f.vendor == 0x8086 => {
                (driver, state) = ("igpu", if crate::drivers::gpu::igpu::blt_ring_live() { "up" } else { "held" });
            }
            #[cfg(feature = "hda")]
            (0x04, 0x03, _) => (driver, state) = ("hda", if crate::drivers::hda::vol::ready() { "up" } else { "held" }),
            #[cfg(feature = "ahci")]
            (0x01, 0x06, 0x01) => (driver, state) = ("ahci", if crate::drivers::ahci::disk_count() > 0 { "up" } else { "held" }),
            (0x08, 0x05, _) => (driver, state) = ("sdhc", if crate::drivers::sdhc::card_num_blocks().is_some() { "up" } else { "held" }),
            (0x06, 0x00 | 0x01 | 0x04 | 0x09, _) => (driver, state) = ("pci", "up"),
            _ => {}
        }
        push(
            c, "pci",
            alloc::format!("{:04x}:{:04x}", f.vendor, f.device),
            alloc::format!("{}:{}.{}", f.bus, f.dev, f.func),
            String::from(pci_id_name(f.vendor, f.device).unwrap_or_else(|| pci_class_name(f.class, f.sub, f.progif))),
            driver, state,
        );
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn pci_rows(c: &mut Census, wait: bool) { arm_rows(c, wait) } // HWCENSUSARM (LEDGER SR165): the aarch64 hosts, NIC and panel (file tail). LINE-NEUTRAL, code before the comment.

// ── USB ─────────────────────────────────────────────────────────────────────────────────────────────────────────

fn usb_rows(c: &mut Census, wait: bool) {
    use crate::drivers::xhci::{census_slots, XhciClaimError};
    let slots = retry(wait, || match census_slots() {
        Ok(v) => Some(Some(v)),
        Err(XhciClaimError::NotReady) => Some(None),
        Err(XhciClaimError::Busy) => None,
    });
    match slots {
        None => c.busy = true,
        Some(None) => {}
        Some(Some(slots)) => {
            c.reg.usb += slots.len();
            for (slot, port, vid, pid, roles) in slots {
                let (driver, state, name) = usb_claim(slot, vid, pid, roles);
                push(c, "usb", alloc::format!("{:04x}:{:04x}", vid, pid), alloc::format!("slot {} port {}", slot, port), String::from(name), driver, state);
            }
        }
    }
    #[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
    match retry(wait, crate::drivers::ehci::census) {
        None => c.busy = true,
        Some(ctrls) => {
            for ((b, d, f), eps) in ctrls {
                c.reg.usb += eps.len();
                for (i, (kind, dead)) in eps.into_iter().enumerate() {
                    let (id, name) = match kind {
                        0 => ("hid:kbd", "Keyboard"),
                        1 => ("hid:mouse", "Mouse"),
                        2 => ("hid:pointer", "Trackpad"),
                        _ => ("hid:other", "Input device"),
                    };
                    push(c, "usb", String::from(id), alloc::format!("ehci {}:{}.{} ep{}", b, d, f, i), String::from(name), "hid", if dead { "refused:halted" } else { "up" });
                }
            }
        }
    }
}

/// Which driver holds xHCI slot `slot` (`roles`: bit 0 hub, 1 keyboard, 2 pointer, 3 BOT storage — the claims the
/// controller keeps), asked of each class driver's own record first.
fn usb_claim(slot: u8, vid: u16, pid: u16, roles: u8) -> (&'static str, &'static str, &'static str) {
    #[cfg(feature = "usbnet")]
    {
        use crate::drivers::xhci::usbnet;
        if usbnet::slot() == slot {
            return ("usbnet", "up", "Network adapter");
        }
        if usbnet::bringup_pending() == Some(slot) {
            return ("usbnet", "held", "Network adapter");
        }
    }
    #[cfg(feature = "uvc")]
    {
        use crate::drivers::xhci::{isoch, ptp};
        if isoch::in_slot() == slot {
            return ("uvc", "up", "Camera");
        }
        if ptp::is_open() && ptp::vid_pid() == (vid, pid) {
            return if ptp::is_mtp_session() { ("mtp", "up", "Phone (MTP)") } else { ("ptp", "up", "Camera (PTP)") };
        }
        #[cfg(feature = "uac")]
        if crate::drivers::usb::uac::device().is_some_and(|(v, p, _)| (v, p) == (vid, pid)) {
            return ("uac", "up", "Audio device");
        }
    }
    if (vid, pid) == (0x0403, 0x6001) && crate::drivers::xhci::ftdi::is_live() {
        return ("ftdi", "up", "Serial adapter");
    }
    if roles & 8 != 0 {
        let published = crate::drivers::block::USB_DISKS.lock().iter().flatten().any(|d| d.info.slot_id == slot);
        return ("storage", if published { "up" } else { "held" }, "USB disk");
    }
    match roles & 6 {
        6 => return ("hid", "up", "Keyboard and pointer"),
        2 => return ("hid", "up", "Keyboard"),
        4 => return ("hid", "up", "Pointer"),
        _ => {}
    }
    if roles & 1 != 0 {
        return ("hub", "up", "USB hub");
    }
    if vid == 0 && pid == 0 {
        return (NONE, "refused:no-descriptor", "USB device");
    }
    (NONE, NO_DRIVER, "USB device")
}

// ── Block, SMC, ACPI ────────────────────────────────────────────────────────────────────────────────────────────

#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))] fn size_words(bytes: u64) -> String { // HWCENSUSARM (SR165): unused on an aarch64 build with no SD driver. LINE-NEUTRAL fold
    if bytes >= 1_000_000_000 {
        alloc::format!("{} GB", bytes / 1_000_000_000)
    } else {
        alloc::format!("{} MB", bytes / 1_000_000)
    }
}

#[cfg(target_arch = "x86_64")]
fn block_rows(c: &mut Census) {
    #[cfg(feature = "ahci")]
    {
        let smart = crate::drivers::ahci::smart::disks();
        for ix in 0..crate::drivers::block::MAX_AHCI_DISKS {
            let Some(d) = crate::drivers::block::ahci_disk(ix) else { continue };
            c.reg.block += 1;
            let size = size_words(d.info.num_blocks.saturating_mul(d.info.block_size as u64));
            let name = match smart.iter().find(|s| s.ix == ix) {
                Some(s) => alloc::format!("{} ({})", s.ident.model(), size),
                None => alloc::format!("SATA disk ({})", size),
            };
            push(c, "sata", alloc::format!("port{}", d.port), alloc::format!("ahci ix{}", ix), name, "ahci", "up");
        }
    }
    if let Some(n) = crate::drivers::sdhc::card_num_blocks() {
        c.reg.block += 1;
        push(c, "sd", String::from("card"), String::from("sdhc"), alloc::format!("SD card ({})", size_words(n.saturating_mul(512))), "sdhc", "up");
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn block_rows(c: &mut Census) { arm_block_rows(c) } // HWCENSUSARM (LEDGER SR165): the aarch64 SD card (file tail). LINE-NEUTRAL, code before the comment.

#[cfg(target_arch = "x86_64")]
fn other_rows(c: &mut Census) {
    #[cfg(feature = "smc")]
    if crate::drivers::smc::present() {
        c.reg.other += 1;
        push(c, "smc", String::from("smc"), String::from("lpc"), String::from("System Management Controller"), "smc", "up");
    }
    let n = crate::arch::x86_64::acpi::cpu_count();
    if n > 0 {
        c.reg.other += 1;
        push(c, "acpi", String::from("madt"), String::from("lapic"), alloc::format!("Processors ({})", n), "smp", "up");
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn other_rows(_c: &mut Census) {}

// ── HWCENSUSARM (LEDGER SR165, BP-bar-25) — the aarch64 census — TAIL-APPENDED, so no x86 line above moves ──────────
// The Pi and the Orin, from the same kind of registries the x86 rows read (no transfer, no bus walk of hw.rs's own):
// the xHCI host the shared controller slot holds (`xhci::claim` not `NotReady` — the Orin's XUSB, the Pi's VL805, a
// virt ECAM xHCI; its devices come from `usb_rows`' slot walk, usbnet included), the Pi's GENET (`genet::netinfo`),
// the panel (`video::WRITER`'s geometry, one masked `try_lock`), and the SD card the block layer was given (the Orin's
// `sdmmc_tegra::tegra_sd_card_blocks`, the Pi's `emmc2::card_num_blocks`). The Tegra PCIe functions and the RTL8168
// are ORINPCIEROWS's (LEDGER SR170): `pcie_probe::functions` and `rtl8168_tegra::census`, read by `arm_pcie_rows` below.

/// Is a controller installed in the shared xHCI slot (`Busy` = installed and on loan)?
#[cfg(not(target_arch = "x86_64"))]
fn arm_xhci_installed() -> bool {
    !matches!(crate::drivers::xhci::claim(), Err(crate::drivers::xhci::XhciClaimError::NotReady))
}

/// The aarch64 host controller, NIC and panel rows. PCI rows count in `reg.pci`; platform and panel rows in `reg.other`.
#[cfg(not(target_arch = "x86_64"))]
fn arm_rows(c: &mut Census, wait: bool) { #[cfg(feature = "pcie2")] arm_pcie_rows(c, wait); // ORINPCIEROWS (LEDGER SR170): the recorded PCIe functions (file tail). LINE-NEUTRAL, code before the comment.
    if arm_xhci_installed() {
        #[cfg(feature = "tegra")]
        {
            c.reg.other += 1;
            push(c, "plat", String::from("xhci"), String::from("xusb"), String::from("USB 3 controller (xHCI)"), "xhci", "up");
        }
        #[cfg(all(not(feature = "tegra"), feature = "piusb"))]
        {
            c.reg.pci += 1;
            push(c, "pci", String::from("1106:3483"), String::from("1:0.0"), String::from("USB 3 controller (xHCI)"), "xhci", "up");
        }
        #[cfg(not(any(feature = "tegra", feature = "piusb")))]
        {
            c.reg.pci += 1;
            push(c, "pci", String::from("xhci"), String::from("ecam"), String::from("USB 3 controller (xHCI)"), "xhci", "up");
        }
    }
    #[cfg(feature = "genet")]
    if let Some(n) = crate::arch::aarch64::genet::netinfo() {
        c.reg.other += 1;
        push(c, "plat", String::from("genet"), String::from("soc"), String::from("Ethernet controller"), "genet", if n.link_up { "up" } else { "held" });
    }
    // The panel: one masked, non-blocking read of the panel handle's geometry (never `panel_info_nonblocking`: its
    // refusal count is the `[inwedge]` witness's). Held = retried when the caller may wait, else the census is busy.
    let info = retry(wait, || crate::arch::without_interrupts(|| crate::video::WRITER.try_lock().map(|fb| fb.info())));
    match info {
        None => c.busy = true,
        Some(i) if i.width > 0 && i.height > 0 => {
            c.reg.other += 1;
            push(c, "disp", String::from("fb"), String::from("scanout"), alloc::format!("Display panel ({}x{})", i.width, i.height), "fb", "up");
        }
        Some(_) => {}
    }
}

/// The aarch64 SD card the block layer was given (counted in `reg.block`, like the x86 SD host's card).
#[cfg(not(target_arch = "x86_64"))]
#[allow(unused_variables)]
fn arm_block_rows(c: &mut Census) {
    #[cfg(all(feature = "sdmmc", feature = "tegra"))]
    {
        let n = crate::arch::aarch64::sdmmc_tegra::tegra_sd_card_blocks();
        if n > 0 {
            c.reg.block += 1;
            push(c, "sd", String::from("card"), String::from("sdmmc"), alloc::format!("SD card ({})", size_words(n.saturating_mul(512))), "sdmmc", "up");
        }
    }
    #[cfg(feature = "baremetal")]
    if let Some(n) = crate::drivers::emmc2::card_num_blocks() {
        c.reg.block += 1;
        push(c, "sd", String::from("card"), String::from("emmc2"), alloc::format!("SD card ({})", size_words(n.saturating_mul(512))), "emmc2", "up");
    }
}

// ── ORINPCIEROWS (LEDGER SR170, BP-bar-25) — the Tegra PCIe rows — TAIL-APPENDED, so no line above moves ─────────────
// One `pci` row per function the PCIe recon recorded where it already read config space (`pcie_probe::functions`;
// hw.rs walks nothing): the root port is the PCI layer's own (`pci up`), the RTL8168 is claimed from its driver's
// registry (`rtl8168_tegra::census`: `up` with link, `held` without; a held registry = this census is busy, as the USB
// loan), any other function says `refused:no-driver`. Counted in `reg.pci`.

/// The aarch64 PCIe function rows (the root port, the NIC below it).
#[cfg(all(not(target_arch = "x86_64"), feature = "pcie2"))]
#[allow(unused_variables)]
fn arm_pcie_rows(c: &mut Census, wait: bool) {
    let fns = crate::arch::aarch64::pcie_probe::functions();
    c.reg.pci += fns.len();
    for f in &fns {
        let (mut driver, mut state) = (NONE, NO_DRIVER);
        match (f.class, f.vid, f.did) {
            (0x06, _, _) => (driver, state) = ("pci", "up"),
            #[cfg(feature = "net4")]
            (_, 0x10EC, 0x8168) => match retry(wait, crate::arch::aarch64::rtl8168_tegra::census) {
                Some(Some(link)) => (driver, state) = ("rtl8168", if link { "up" } else { "held" }),
                Some(None) => {}
                None => {
                    c.busy = true;
                    (driver, state) = ("rtl8168", "held");
                }
            },
            _ => {}
        }
        push(
            c, "pci",
            alloc::format!("{:04x}:{:04x}", f.vid, f.did),
            alloc::format!("{}:{}.{}", f.bus, f.dev, f.func),
            String::from(pci_class_name(f.class, f.sub, f.progif)),
            driver, state,
        );
    }
}

// ── ZENBOOKSURVEY (zenbook-ledger Z1) — THE PLATFORM — TAIL-APPENDED, so no line above moves ───────────────────────
// Which machine is this? The loader hands the SMBIOS entry point from the UEFI configuration table
// (`BootInfo::smbios_ptr`, tagged with `unaos_boot_info::SMBIOS_TAG`); main.rs lifts it into [`set_smbios`];
// `pci.rs init` resolves it ONCE, first thing ([`platform_say`]), before any probe that touches an Apple-only register
// window, and prints `[hw] platform=<token> dmi="<product>" vendor="<manufacturer>" src=<how>`. The SMC, the gmux and
// the HDA codec drive ask [`gate`]: today's code runs on the MacBookPro10,1, under QEMU, on any Apple machine, on a
// machine whose SMBIOS could not be read (an older loader, a bad anchor or checksum, an unmapped page — so a rMBP under
// an old loader keeps every probe) and before the resolution; a POSITIVELY identified non-Apple machine (the Zenbook
// UM5606W, or any other vendor) gets `[hw] skip <probe> platform=<token>` once and the probe's own "absent" answer.
// `tests platform` re-prints the line and runs the parser over five canned tables (`:: PLATFORM: … -> PASS ::`).
// Readers take no lock: the resolution lives in a `spin::Once` (the acpi.rs TOPOLOGY idiom), the skip lines in one
// atomic bitmask. `pci_id_name` is the PCI id table the census's name column asks first (the class name stays the
// fallback); the sibling ZENBOOK arcs add their own rows above its last arm.

/// The machine this kernel booted on, from the SMBIOS Type 1 (System Information) record.
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    /// Type 1 product `MacBookPro10,1` — the 2012 Retina MacBook Pro the x86 line was built on.
    MacBookPro10_1,
    /// A product containing `UM5606` — the ASUS Zenbook UM5606W (Strix Point).
    ZenbookUm5606w,
    /// Manufacturer `QEMU`, or a product starting `Standard PC (` (OVMF's q35 / i440fx tables).
    Qemu,
    /// Anything else: the Type 1 product (empty when no SMBIOS was read).
    Unknown(&'static str),
}

impl Platform {
    /// The wire token.
    pub fn token(self) -> &'static str {
        match self {
            Platform::MacBookPro10_1 => "macbookpro10_1",
            Platform::ZenbookUm5606w => "zenbook_um5606w",
            Platform::Qemu => "qemu",
            Platform::Unknown(_) => "unknown",
        }
    }
}

/// The Apple-only probes [`gate`] answers for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Probe {
    /// The Apple SMC at ports 0x300/0x304 (`drivers/smc.rs`).
    Smc,
    /// The Apple gmux at ports 0x7C2-0x7D4 and its IGD switch ladder (`drivers/gpu/igpu.rs`).
    Gmux,
    /// The HDA codec drive, whose GPIO policy was measured on the CS4206 only (`drivers/hda.rs`).
    Hda, Ec, // ZENBOOKEC (zenbook-ledger Z8): `Ec` = the ACPI embedded controller census (`drivers/ec.rs`), gated the INVERSE way (`ec_runs`, file tail). LINE-NEUTRAL fold, code before the comment.
}

impl Probe {
    /// The wire name.
    pub fn name(self) -> &'static str {
        match self {
            Probe::Smc => "smc",
            Probe::Gmux => "gmux",
            Probe::Hda => "hda", Probe::Ec => "ec", // ZENBOOKEC (Z8). LINE-NEUTRAL fold.
        }
    }
    fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

const K_MAC: u8 = 0;
const K_ZENBOOK: u8 = 1;
const K_QEMU: u8 = 2;
/// A Type 1 from a non-Apple vendor: Apple-only probes are skipped.
const K_OTHER: u8 = 3;
/// A Type 1 from Apple that is not the 10,1: today's probes run.
const K_APPLE: u8 = 4;
/// No usable SMBIOS: today's probes run.
const K_NODMI: u8 = 5;
const DMI_LEN: usize = 64;

/// One resolution: the kind, how it was read, and the two Type 1 strings (printable ASCII, `"` -> `?`).
struct Dmi {
    kind: u8,
    src: &'static str,
    vendor: [u8; DMI_LEN],
    vlen: usize,
    product: [u8; DMI_LEN],
    plen: usize,
}

impl Dmi {
    fn empty(src: &'static str) -> Dmi {
        Dmi { kind: K_NODMI, src, vendor: [0; DMI_LEN], vlen: 0, product: [0; DMI_LEN], plen: 0 }
    }
    fn vendor_str(&self) -> &str {
        core::str::from_utf8(&self.vendor[..self.vlen]).unwrap_or("")
    }
    fn product_str(&self) -> &str {
        core::str::from_utf8(&self.product[..self.plen]).unwrap_or("")
    }
}

/// Copy a Type 1 string into a fixed buffer: printable ASCII only, trailing spaces dropped.
fn dmi_copy(dst: &mut [u8; DMI_LEN], src: &[u8]) -> usize {
    let mut n = 0;
    for &b in src.iter().take(DMI_LEN) {
        dst[n] = if (0x20..0x7F).contains(&b) && b != b'"' { b } else { b'?' };
        n += 1;
    }
    while n > 0 && dst[n - 1] == b' ' {
        n -= 1;
    }
    n
}

/// The loader's tagged pointer, as main.rs lifted it (0 = none).
static SMBIOS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The one resolution, written once by [`platform_say`].
static PLAT: spin::Once<Dmi> = spin::Once::new();
/// One bit per [`Probe`] whose skip line has printed this boot.
static SKIPPED: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// main.rs:219 — store `BootInfo::smbios_ptr` (read later, never here: the heap and the live map are not up yet).
pub fn set_smbios(ptr: u64) {
    SMBIOS.store(ptr, core::sync::atomic::Ordering::Relaxed);
}

/// Classify one Type 1 (manufacturer, product).
fn classify(vendor: &[u8], product: &[u8]) -> u8 {
    let trim = |s: &[u8]| -> usize { s.iter().rposition(|&b| b != b' ' && b != 0).map_or(0, |i| i + 1) };
    let (v, p) = (&vendor[..trim(vendor)], &product[..trim(product)]);
    if v.is_empty() && p.is_empty() {
        K_NODMI
    } else if p == b"MacBookPro10,1" {
        K_MAC
    } else if p.windows(6).any(|w| w == b"UM5606") {
        K_ZENBOOK
    } else if v == b"QEMU" || p.starts_with(b"Standard PC (") {
        K_QEMU
    } else if v.starts_with(b"Apple") {
        K_APPLE
    } else {
        K_OTHER
    }
}

/// Does a machine of this kind run the Apple-only probes as today? Only a positively identified non-Apple one does not.
fn kind_runs(kind: u8) -> bool {
    !matches!(kind, K_ZENBOOK | K_OTHER)
}

fn sum8(b: &[u8]) -> u8 {
    b.iter().fold(0u8, |a, &x| a.wrapping_add(x))
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// The `idx`-th (1-based) string of a structure's string set (`strs` = the bytes after the formatted area, up to and
/// including the first NUL of the double-NUL terminator). Index 0 is "no string".
fn dmi_string(strs: &[u8], idx: u8) -> &[u8] {
    if idx == 0 {
        return &[];
    }
    strs.split(|&b| b == 0).nth(idx as usize - 1).unwrap_or(&[])
}

/// Read the Type 1 (manufacturer, product) through the entry point at `ep`. Pure: every byte comes from `read(addr,
/// len)`, which answers `None` for a range it cannot show. SMBIOS 3.0 `_SM3_` (24-byte entry, 64-bit table address,
/// DSP0134 §5.2.2) or 2.x `_SM_` + `_DMI_` (31-byte entry, §5.2.1); both checksums are checked; the walk is bounded
/// by the table length (capped at 64 KiB) and stops at type 127.
fn smbios_type1<'a>(ep: u64, read: &dyn Fn(u64, usize) -> Option<&'a [u8]>) -> Result<(&'static str, &'a [u8], &'a [u8]), &'static str> {
    let head = read(ep, 5).ok_or("unmapped")?;
    let (src, taddr, tlen) = if head == b"_SM3_" {
        let e = read(ep, 0x18).ok_or("unmapped")?;
        let n = (e[6] as usize).max(0x18);
        let e = read(ep, n).ok_or("unmapped")?;
        if sum8(e) != 0 {
            return Err("bad-checksum");
        }
        ("smbios3", u64::from_le_bytes([e[0x10], e[0x11], e[0x12], e[0x13], e[0x14], e[0x15], e[0x16], e[0x17]]), le32(e, 0x0C) as usize)
    } else if &head[..4] == b"_SM_" {
        let e = read(ep, 0x1F).ok_or("unmapped")?;
        let n = (e[5] as usize).clamp(0x1E, 0x1F);
        if sum8(&e[..n]) != 0 || &e[0x10..0x15] != b"_DMI_" || sum8(&e[0x10..0x1F]) != 0 {
            return Err("bad-checksum");
        }
        ("smbios2", le32(e, 0x18) as u64, le16(e, 0x16) as usize)
    } else {
        return Err("bad-anchor");
    };
    let t = read(taddr, tlen.min(0x1_0000)).ok_or("unmapped")?;
    let mut i = 0usize;
    while i + 4 <= t.len() {
        let (ty, l) = (t[i], t[i + 1] as usize);
        if l < 4 || i + l > t.len() {
            return Err("bad-table");
        }
        let s = i + l;
        let mut j = s;
        loop {
            if j + 1 >= t.len() {
                return Err("bad-table");
            }
            if t[j] == 0 && t[j + 1] == 0 {
                break;
            }
            j += 1;
        }
        if ty == 1 && l >= 6 {
            let strs = &t[s..j + 1];
            return Ok((src, dmi_string(strs, t[i + 4]), dmi_string(strs, t[i + 5])));
        }
        if ty == 127 {
            break;
        }
        i = j + 2;
    }
    Err("no-type1")
}

/// `len` bytes of firmware memory at physical `addr`, when every 4 KiB page of it is present in the live tables and
/// maps to itself (the kernel runs on the firmware's identity map, arch/x86_64/memory.rs; `memory::translate` is the
/// read-only walk). `None` instead of a fault for a pointer an older loader's bytes made up.
#[cfg(target_arch = "x86_64")]
fn read_phys(addr: u64, len: usize) -> Option<&'static [u8]> {
    if addr == 0 || len == 0 || addr >> 48 != 0 {
        return None;
    }
    let end = addr.checked_add(len as u64)?;
    let mut page = addr & !0xFFF;
    while page < end {
        if crate::arch::x86_64::memory::translate(page)? != page {
            return None;
        }
        page += 0x1000;
    }
    // SAFETY: every page of [addr, addr + len) is present and identity-mapped (checked above); the SMBIOS entry point
    // and table are firmware data nothing in the kernel writes.
    Some(unsafe { core::slice::from_raw_parts(addr as *const u8, len) })
}

#[cfg(not(target_arch = "x86_64"))]
fn read_phys(_addr: u64, _len: usize) -> Option<&'static [u8]> {
    None
}

/// The one resolution from the stored pointer.
fn resolve() -> Dmi {
    let raw = SMBIOS.load(core::sync::atomic::Ordering::Relaxed);
    if raw == 0 {
        return Dmi::empty("absent");
    }
    if raw >> 48 != unaos_boot_info::SMBIOS_TAG >> 48 {
        return Dmi::empty("bad-tag");
    }
    let mut d = Dmi::empty("absent");
    match smbios_type1(raw & 0x0000_FFFF_FFFF_FFFF, &read_phys) {
        Ok((src, v, p)) => {
            d.src = src;
            d.vlen = dmi_copy(&mut d.vendor, v);
            d.plen = dmi_copy(&mut d.product, p);
            d.kind = classify(&d.vendor[..d.vlen], &d.product[..d.plen]);
        }
        Err(why) => d.src = why,
    }
    d
}

/// The machine, as resolved at `pci.rs init` (`Unknown("")` before that).
pub fn platform() -> Platform {
    match PLAT.get() {
        None => Platform::Unknown(""),
        Some(d) => match d.kind {
            K_MAC => Platform::MacBookPro10_1,
            K_ZENBOOK => Platform::ZenbookUm5606w,
            K_QEMU => Platform::Qemu,
            _ => Platform::Unknown(d.product_str()),
        },
    }
}

/// Resolve (once) and print `[hw] platform=… dmi="…" vendor="…" src=…`. Called first thing in `pci.rs init` and by
/// `tests platform`.
pub fn platform_say() {
    let d = PLAT.call_once(resolve);
    serial_println!("[hw] platform={} dmi=\"{}\" vendor=\"{}\" src={}", platform().token(), d.product_str(), d.vendor_str(), d.src);
}

/// May the Apple-only probe `p` touch its registers? `true` = run today's code. `false` only on a positively identified
/// non-Apple machine, and then `[hw] skip <probe> platform=<token>` prints once per probe per boot. Lock-free.
pub fn gate(p: Probe) -> bool { if p == Probe::Ec { return ec_runs(); } // ZENBOOKEC (zenbook-ledger Z8): the EC probe runs ONLY on a positively identified non-Apple machine and prints no skip line elsewhere (body `ec_runs` at the file tail). LINE-NEUTRAL fold, code before the comment.
    let Some(d) = PLAT.get() else { return true };
    if kind_runs(d.kind) {
        return true;
    }
    if SKIPPED.fetch_or(p.bit(), core::sync::atomic::Ordering::Relaxed) & p.bit() == 0 {
        serial_println!("[hw] skip {} platform={}", p.name(), platform().token());
    }
    false
}

/// `tests platform`: the live line again, then the parser over five canned tables (an SMBIOS 3.0 entry point and a
/// two-structure table each): the rMBP, the Zenbook, QEMU, a Dell, and the QEMU table with its entry checksum broken.
/// PASS = all five classify as expected AND the live platform resolved.
pub fn platform_selftest() {
    platform_say();
    const BASE: u64 = 0x1000;
    let image = |vendor: &[u8], product: &[u8], corrupt: bool| -> Vec<u8> {
        let mut t: Vec<u8> = alloc::vec![1, 8, 0, 0, 1, 2, 0, 0];
        t.extend_from_slice(vendor);
        t.push(0);
        t.extend_from_slice(product);
        t.extend_from_slice(&[0, 0]);
        t.extend_from_slice(&[127, 4, 1, 0, 0, 0]);
        let mut img = alloc::vec![0u8; 0x40];
        img[..5].copy_from_slice(b"_SM3_");
        img[6] = 0x18;
        img[7] = 3;
        img[0x0C..0x10].copy_from_slice(&(t.len() as u32).to_le_bytes());
        img[0x10..0x18].copy_from_slice(&(BASE + 0x40).to_le_bytes());
        img[5] = 0u8.wrapping_sub(sum8(&img[..0x18]));
        if corrupt {
            img[5] ^= 0x5A;
        }
        img.extend_from_slice(&t);
        img
    };
    let cases: [(&[u8], &[u8], bool, u8); 5] = [
        (b"Apple Inc.", b"MacBookPro10,1", false, K_MAC),
        (b"ASUSTeK COMPUTER INC.", b"ASUS Zenbook S 16 UM5606WA_UM5606WA", false, K_ZENBOOK),
        (b"QEMU", b"Standard PC (Q35 + ICH9, 2009)", false, K_QEMU),
        (b"Dell Inc.", b"XPS 13 9340", false, K_OTHER),
        (b"QEMU", b"Standard PC (Q35 + ICH9, 2009)", true, K_NODMI),
    ];
    let mut ok = 0usize;
    for (v, p, bad, want) in cases {
        let img = image(v, p, bad);
        let read = |a: u64, n: usize| kat_read(&img, BASE, a, n);
        let kind = match smbios_type1(BASE, &read) {
            Ok((_, v, p)) => classify(v, p),
            Err(_) => K_NODMI,
        };
        if kind == want && kind_runs(kind) == !matches!(want, K_ZENBOOK | K_OTHER) {
            ok += 1;
        } else {
            serial_println!("[hw] platform kat miss want={} got={}", want, kind);
        }
    }
    let live = PLAT.get();
    let run = if live.map_or(true, |d| kind_runs(d.kind)) { "run" } else { "skip" };
    let pass = ok == cases.len() && live.is_some();
    serial_println!(
        ":: PLATFORM: platform={} src={} kat={}/{} gate=smc:{},gmux:{},hda:{} -> {} ::",
        platform().token(), live.map_or("unresolved", |d| d.src), ok, cases.len(), run, run, run, if pass { "PASS" } else { "FAIL" }
    );
}

/// `tests platform`'s reader over one canned image based at `base`.
fn kat_read(img: &[u8], base: u64, a: u64, n: usize) -> Option<&[u8]> {
    let o = a.checked_sub(base)? as usize;
    img.get(o..o.checked_add(n)?)
}

/// The census's name for a PCI function this kernel knows by id (pci.ids 2026.10.27), asked before the class name.
/// QEMU's and the rMBP's functions match no row, so their census text is unchanged. ZENBOOK sibling arcs: add ONE row
/// above the last arm and name the hunk.
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
fn pci_id_name(vendor: u16, device: u16) -> Option<&'static str> {
    match (vendor, device) {
        (0x1002, 0x150E) => Some("Radeon 890M graphics (Strix, gfx1150)"),
        (0x1002, 0x1640) => Some("Radeon HD Audio (HDMI/DP)"),
        (0x1022, 0x1507) => Some("Strix root complex"),
        (0x1022, 0x1508) => Some("Strix IOMMU"),
        (0x1022, 0x1509) => Some("Strix dummy host bridge"),
        (0x1022, 0x150A) => Some("Strix PCIe USB4 bridge"),
        (0x1022, 0x150B) => Some("Strix GPP bridge"),
        (0x1022, 0x150C) => Some("Strix internal GPP bridge"),
        (0x1022, 0x150D) => Some("Strix PCIe dummy function"),
        (0x1022, 0x151A | 0x151B | 0x151E | 0x151F) => Some("Strix USB 3.1 controller (xHCI)"),
        (0x1022, 0x151C | 0x151D) => Some("Strix USB4 host interface (NHI)"),
        (0x1022, 0x15E2) => Some("AMD audio coprocessor (ACP)"),
        (0x1022, 0x15E3) => Some("Ryzen HD Audio controller"),
        (0x1022, 0x16F8..=0x16FF) => Some("Strix data fabric"),
        (0x1022, 0x17E0) => Some("AMD platform security processor (CCP/ASP)"),
        (0x1022, 0x17F0) => Some("AMD neural processing unit (NPU)"),
        (0x1022, 0x790B) => Some("AMD FCH SMBus controller"),
        (0x1022, 0x790E) => Some("AMD FCH LPC bridge"),
        _ => None,
    }
}

/// ZENBOOKEC (zenbook-ledger Z8): may the ACPI EC census (`drivers/ec.rs`) touch the ECDT's ports? The inverse of the
/// Apple gate: `true` only on a positively identified non-Apple machine (the Zenbook, another vendor's Type 1); the rMBP,
/// QEMU, any Apple machine and a boot with no readable SMBIOS answer `false`, silently.
fn ec_runs() -> bool {
    PLAT.get().is_some_and(|d| !kind_runs(d.kind))
}
