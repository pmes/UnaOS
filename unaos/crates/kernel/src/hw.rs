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
#[cfg(target_arch = "x86_64")]
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
            (0x0C, 0x03, 0x30) if !xhci_seen => {
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
            String::from(pci_class_name(f.class, f.sub, f.progif)),
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
// `sdmmc_tegra::tegra_sd_card_blocks`, the Pi's `emmc2::card_num_blocks`). OWED (outside this arc's files): the
// Tegra PCIe functions (pcie_probe.rs keeps no table) and the RTL8168 (`NET4_DEVICE` is private to its `mod metal`).

/// Is a controller installed in the shared xHCI slot (`Busy` = installed and on loan)?
#[cfg(not(target_arch = "x86_64"))]
fn arm_xhci_installed() -> bool {
    !matches!(crate::drivers::xhci::claim(), Err(crate::drivers::xhci::XhciClaimError::NotReady))
}

/// The aarch64 host controller, NIC and panel rows. PCI rows count in `reg.pci`; platform and panel rows in `reg.other`.
#[cfg(not(target_arch = "x86_64"))]
fn arm_rows(c: &mut Census, wait: bool) {
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
