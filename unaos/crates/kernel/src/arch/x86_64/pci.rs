// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use x86_64::instructions::port::Port;


pub unsafe fn read_config_32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    let address_port: u16 = 0xCF8;
    let data_port: u16 = 0xCFC;

    let address = 0x8000_0000
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);

    let mut addr_port = Port::<u32>::new(address_port);
    addr_port.write(address);

    let mut data = Port::<u32>::new(data_port);
    data.read()
}

pub unsafe fn read_config_16(bus: u8, slot: u8, func: u8, offset: u8) -> u16 {
    let address_port: u16 = 0xCF8;
    let data_port: u16 = 0xCFC;

    let address = 0x8000_0000
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);

    let mut addr_port = Port::<u32>::new(address_port);
    addr_port.write(address);

    let port_offset = (offset & 2) as u16;
    let mut data = Port::<u16>::new(data_port + port_offset);
    data.read()
}

pub unsafe fn write_config_16(bus: u8, slot: u8, func: u8, offset: u8, value: u16) {
    let address_port: u16 = 0xCF8;
    let data_port: u16 = 0xCFC;

    let address = 0x8000_0000
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);

    let mut addr_port = Port::<u32>::new(address_port);
    addr_port.write(address);

    let port_offset = (offset & 2) as u16;
    let mut data = Port::<u16>::new(data_port + port_offset);
    data.write(value);
}

pub unsafe fn write_config_32(bus: u8, slot: u8, func: u8, offset: u8, value: u32) {
    let address = 0x8000_0000
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);

    let mut addr_port = Port::<u32>::new(0xCF8);
    addr_port.write(address);
    let mut data = Port::<u32>::new(0xCFC);
    data.write(value);
}

/// PORTSW-1: Intel PCH xHCI port switchover (EHCI->xHCI). On Intel chipsets (e.g. Panther Point in
/// the 2012 MacBook Pro Retina) the USB2 ports are physically shared with an EHCI companion
/// controller and default to EHCI — so the internal keyboard/trackpad are invisible to an xHCI-only
/// driver — and the SuperSpeed lanes need enabling. Routing every *switchable* port to xHCI makes
/// those devices RE-ENUMERATE on the existing xHCI+HID stack.
///
/// The routing-MASK registers advertise which ports CAN switch on THIS silicon:
///   XUSB2PR    @0xD0  USB2 routing SELECT  (0 = EHCI, 1 = xHCI, per port bit)
///   USB2PRM    @0xD4  USB2 routing MASK    (which USB2 port bits are switchable)
///   USB3_PSSEN @0xD8  SuperSpeed enable    SELECT
///   USB3PRM    @0xDC  SuperSpeed routing MASK
///
/// MASK-READ-BEFORE-WRITE discipline (PORTSW-1 hard rule 1): we NEVER write a bit the mask doesn't
/// advertise. The write is `current | (mask & mask)` == `current | mask` — it only SETS advertised
/// bits and never clears an already-set bit, so an unmasked/undefined bit can't wedge the controller.
/// (Linux `usb_enable_intel_xhci_ports()` copies the mask straight into the select register; the
/// `current |` here is strictly more conservative — same routed bits, no clobber of foreign bits.)
/// If a mask reads 0 the silicon advertises NO switchable ports of that class, so we skip its write
/// and report — do NOT force it (PORTSW-1 STOP tripwire).
///
/// Ordering matches Linux: enable SuperSpeed (USB3) before routing USB2. Runs in `init` BEFORE
/// `xhci::init` resets+starts the controller, i.e. before enumeration — ONE topology per boot, never
/// re-flipped live (PORTSW-1 hard rule 2). Gated to Intel xHCI device ids with an EHCI companion;
/// a clean no-op on everything else (QEMU's qemu-xhci is vendor 0x1b36 — the whole flip is inert
/// there, which is expected: QEMU doesn't model Panther-Point routing). 0x1E31 is the Panther Point
/// part on the MacBookPro10,1. Runs BY DEFAULT on x86 (metal-gated policy: on the 2012 rMBP the
/// knob-off no-routing boot dropped ALL external USB — serial, storage, input — so routing is
/// DEFAULT-ON here). Suppressed ONLY under the `noportsw` opt-out feature (UNAOS_NOPORTSW=1) for the
/// never-run no-routing topology experiment; opted out = this function does not exist and no
/// config-space write is issued => byte-identical no-routing media.
///
/// Metal-confirmed default (2026-07-16): the config-space read-backs in the witness are what let a
/// real-HW boot tell whether the mux actually toggled or firmware (Apple EFI) locked the shared-port
/// bits.
#[cfg(not(feature = "noportsw"))]
fn enable_intel_xhci_ports(bus: u8, dev: u8, func: u8) {
    const XUSB2PR: u8 = 0xD0;
    const USB2PRM: u8 = 0xD4;
    const USB3_PSSEN: u8 = 0xD8;
    const USB3PRM: u8 = 0xDC;
    // Intel xHCI controllers whose USB2 ports are shared with an EHCI companion.
    const SWITCHABLE: &[u16] = &[
        0x1E31, // Panther Point (7-series — 2012 MacBook Pro Retina)
        0x8C31, // Lynx Point
        0x9C31, // Lynx Point-LP
        0x9CB1, // Wildcat Point-LP
        0x22B5, // Cherry View / Braswell
    ];

    unsafe {
        let vendor = read_config_16(bus, dev, func, 0x00);
        let device = read_config_16(bus, dev, func, 0x02);
        // Always log the controller's identity so a serial-less metal boot can SEE which xHCI this is
        // — the live test of whether the rMBP's Panther Point id (0x1e31) is the one we gate on.
        serial_println!(":: xHCI PCI id {:04x}:{:04x} @ {}:{}.{} (PORTSW default-on) ::", vendor, device, bus, dev, func);

        // A config-space write is issued ONLY on a known Intel shared-port controller (an EHCI
        // companion routes the USB2 ports). On anything else — notably QEMU's qemu-xhci (0x1b36) —
        // the flip is INERT: we read the register block (harmless; PCI config reads never have side
        // effects) purely so the witness can print its mask read-back, but we write NOTHING, so the
        // topology is unchanged. This is the expected QEMU no-op (PORTSW-1 hard rule 4).
        let intel_switchable = vendor == 0x8086 && SWITCHABLE.contains(&device);
        if !intel_switchable {
            serial_println!(
                ":: xHCI {:04x}:{:04x} not a shared-port Intel part — port switchover INERT (no config writes; e.g. QEMU, or firmware already routed / no EHCI companion) ::",
                vendor, device
            );
        }

        // Read masks + current SELECT for both registers (harmless on any controller).
        let ss_mask = read_config_32(bus, dev, func, USB3PRM);
        let ss_before = read_config_32(bus, dev, func, USB3_PSSEN);
        let usb2_mask = read_config_32(bus, dev, func, USB2PRM);
        let usb2_before = read_config_32(bus, dev, func, XUSB2PR);

        // Apply the flip ONLY on the matched Intel silicon, mask-disciplined, SuperSpeed before USB2.
        let (ss_after, usb2_after) = if intel_switchable {
            // --- SuperSpeed enable (USB3) ---
            let ss_after = if ss_mask != 0 {
                let want = ss_before | (ss_mask & ss_mask); // set only advertised bits, clear none
                write_config_32(bus, dev, func, USB3_PSSEN, want);
                read_config_32(bus, dev, func, USB3_PSSEN) // read-back = the metal proof
            } else {
                serial_println!(":: PORTSW-1: USB3PRM mask=0 — no switchable SuperSpeed ports advertised; skipping USB3_PSSEN write ::");
                ss_before
            };
            // --- USB2 routing (EHCI->xHCI) ---
            let usb2_after = if usb2_mask != 0 {
                let want = usb2_before | (usb2_mask & usb2_mask); // set only advertised bits, clear none
                write_config_32(bus, dev, func, XUSB2PR, want);
                read_config_32(bus, dev, func, XUSB2PR) // read-back = the metal proof
            } else {
                // STOP-tripwire condition: no USB2 port advertised switchable — the internal
                // keyboard/trackpad cannot be routed on this silicon as read. Report, don't force.
                serial_println!(":: PORTSW-1: USB2PRM mask=0 — NO switchable USB2 ports advertised; internal kbd/trackpad NOT routable on this silicon; skipping XUSB2PR write ::");
                usb2_before
            };
            (ss_after, usb2_after)
        } else {
            // Inert path: no write issued, so after == before by construction.
            (ss_before, usb2_before)
        };

        // The assertable metal record: masks + before/after for both registers. On the matched
        // Intel part a read-back that equals `before | mask` confirms the mux toggled; a smaller
        // value means firmware locked some shared-port bits (Apple EFI may pre-own or refuse to
        // release them). On QEMU before == after (inert). Uncounted witness (`== witness ::`).
        serial_println!(
            ":: PORTSW-1: XUSB2PR mask={:#x} routed {:#x}->{:#x} + USB3_PSSEN mask={:#x} {:#x}->{:#x} (default-on) == witness ::",
            usb2_mask, usb2_before, usb2_after, ss_mask, ss_before, ss_after
        );
        // GUI-WITNESS: record the mux flip as a boot milestone. "flip" = a real XUSB2PR write on
        // matched Intel silicon (the rMBP metal path); "inert" = the no-op read-only case (QEMU, or
        // no EHCI companion). Distinguishing the two on-panel tells a silent-serial bench whether the
        // internal kbd/trackpad were routed to xHCI at all.
        crate::bootlog::record(if intel_switchable { "portsw:flip" } else { "portsw:inert" });
    }
}

// ── GPACE — the inside of the BPACE `pci-usb` delta ─────────────────────────────────────────────
// The s60 metal ledger read `pci-usb d=4620ms` — the largest single block in that boot, and an
// undivided delta nothing had ever split. Two things about it are easy to get wrong, and this
// instrument exists to make both of them impossible to get wrong again:
//
//   1. **`pci-usb` is not a USB block.** BPACE stamps it after `pci::init` RETURNS, and `d=` is
//      always the delta from the previous stamp — which, because `xhci.start()` kicks port-1
//      enumeration before returning, is `enum:p1`. So the window covers the `start_next_port`
//      tail, the BENCH-RIDE probes, `gpu::detect`, `igpu::init`, `kepler::init`, `sdhc::probe`
//      and the NIC block. The xHCI bring-up itself is upstream of it, already subdivided by the
//      BOOTPACE M4 tags.
//   2. **Most of that 4620 ms is not in a default build.** The s60 stick was armed with
//      `UNAOS_KEPLER` / `UNAOS_KEPLER_TAKEOVER` / `UNAOS_IVB` / `UNAOS_WC`; the 2026-07-30 metal
//      baseline, with none of them, reads `pci-usb … 113 ms`. A split that did not say WHICH BUILD
//      it measured would let a reader charge a default boot for 4.5 s it never pays, so the report
//      names the compiled knob set on its own line (`build=`), and a knobless build prints
//      `build=default(no-gpu-knobs)`.
//
// Design follows the EPACE precedent (`drivers/ehci/mod.rs`): cycle accumulators per phase CLASS,
// converted to ms only at PRINT time, printed as two lines at the one unconditional exit of
// `pci::init`. Deliberately NOT more `bootpace::record` stamps — the ring is at n=31 of CAP=64 with
// drop-NEWEST, and seven more stamps would spend headroom the late boot tags need.
//
// Instrument honesty (the can-this-lie-while-looking-right check):
//   * **The self-check is structural, not aspirational.** `span` is measured from
//     `bootpace::last_stamp()` — the very stamp BPACE will compute `pci-usb d=` against, captured
//     at the instant the xHCI block ends — to the report. Anchoring on the LAST stamp rather than
//     on the literal tag `enum:p1` is what makes `span == pci-usb d=` a property of the
//     construction instead of a coincidence of this machine's topology; the tag that actually
//     anchored is printed as `anchor=` so a reader can see when the topology changes.
//   * **A short-counting class cannot hide.** The named classes are disjoint, sequential spans
//     inside `span`, and `resid = span - Σ(classes)` over the PRINTED millisecond values, so the
//     row a reader adds up closes on `span` exactly rather than to within a per-class flooring
//     error. A class that under-measures therefore INFLATES `resid` — the arithmetic still closes,
//     so the lie surfaces as unattributed time rather than as a clean-looking total.
//   * **The one reading that would convict the tiling is not clamped into looking healthy.**
//     `Σ > span` in the CYCLE domain means the classes overlap, the anchor is wrong, or
//     `now_cycles()` went backwards across cores. A `saturating_sub` would render all three as
//     `resid=0ms` — indistinguishable from health. The cycle comparison is kept as an explicit
//     tripwire that prints `:: GPACE: OVERLAP … ::` ahead of the report instead.
//   * **`bench`'s count is resolvable.** It is the only class with more than one call site, so the
//     three BENCH-RIDE probes are named individually in `build=` (`therm+`/`pcilink+`/`vrom+`);
//     `bench=..ms(n=2)` would otherwise not say WHICH two ran.
//   * **Zero and never-ran are structurally distinct.** Every class prints `<v><unit>(n=<count>)`.
//     `0ms(n=0)` is "this code was not compiled in / never reached"; `0ms(n=1)` is "it ran and cost
//     nothing". This project has twice been bitten by conflating the two.
//   * **It can execute in every state it reports on.** The NIC block used to `return` early on a
//     non-Intel part — which this machine's Broadcom 0x14e4 takes on EVERY boot — so a report
//     placed after it would never have run on the machine it exists to measure. That block is now
//     `init_network()`, and its early exit returns from the helper; `pci::init` has exactly one
//     exit, and the report sits on it.
//   * **Same clock as BPACE.** Conversion goes through `bootpace::origin_hz()`, the rate the ledger
//     divides its own `d=` by, so the two instruments cannot disagree merely by arithmetic. `hz=0`
//     prints raw counter ticks with a `cy` suffix, never a fabricated millisecond.
//
// Three readings that differ (the baseline law):
//   * bench media, GPU knobs armed — `span` ≈ 4600 ms with `kepler=` dominant;
//   * default `./arroyo esp-x86` — the line STILL prints, with `igpu=0ms(n=0) kepler=0ms(n=0)`,
//     `detect=0ms(n=0)`, `build=default(no-gpu-knobs)` and `span` ≈ 100 ms. That is what proves the
//     numbers report the GPU path rather than the reporter's own liveness;
//   * `UNAOS_SKIP_XHCI=1` — the line is ABSENT entirely, because `pci::init` is never called.
const G_XTAIL: usize = 0; // enum:p1 → end of the xHCI block: the `start_next_port` tail
const G_BENCH: usize = 1; // the knob-gated BENCH-RIDE probes (therm / pcilink / vrom)
const G_DETECT: usize = 2; // gpu::detect::detect_gpus() — the class-0x03 census
const G_IGPU: usize = 3; // gpu::igpu::init, per Ivy Bridge IGD found
const G_KEPLER: usize = 4; // gpu::kepler::init, per GK107 found
const G_SDHC: usize = 5; // drivers::sdhc::probe() — the read-only SDHC census
const G_NIC: usize = 6; // init_network(): the class-0x02 lookup + e1000 bring-up or the skip
const N_GPACE: usize = 7;
const GPACE_TAGS: [&str; N_GPACE] =
    ["xtail", "bench", "detect", "igpu", "kepler", "sdhc", "nic"];

#[derive(Clone, Copy)]
struct Gpace {
    cy: [u64; N_GPACE],
    n: [u32; N_GPACE],
}

impl Gpace {
    const fn new() -> Self {
        Gpace { cy: [0; N_GPACE], n: [0; N_GPACE] }
    }
    /// Close a span opened at `t0` (a `now_cycles()` reading) into class `class`, and count it.
    /// `n` is incremented on the CLOSE, so a class only ever reports a count for work that actually
    /// ran to completion here.
    fn add(&mut self, class: usize, t0: u64) {
        self.cy[class] =
            self.cy[class].wrapping_add(crate::arch::now_cycles().wrapping_sub(t0));
        self.n[class] = self.n[class].saturating_add(1);
    }
    /// Σ of every named class — the subtrahend of `resid`.
    fn sum(&self) -> u64 {
        let mut s = 0u64;
        let mut k = 0;
        while k < N_GPACE {
            s = s.wrapping_add(self.cy[k]);
            k += 1;
        }
        s
    }
}

/// Cycles → whole ms at print time via the BPACE ledger's own rate, or raw ticks when that rate is
/// still unknown. Never fabricates a millisecond out of a guessed frequency (the `[vugfps]` lesson).
///
/// The expression is `cy / (hz / 1000)` — deliberately `bootpace::Dur`'s formula and NOT EPACE's
/// `cy * 1000 / hz`. The two can differ by one millisecond, but only just barely: at this machine's
/// `hz=2693848854` the relative gap is ≈3.2e-7 (~0.0006 ms over a 1845 ms reading), so it changes a
/// printed digit only when it straddles a floor boundary, on the order of 1e-6 per reading. This is
/// not a bug being fixed. It is that the whole deliverable is `span` versus the ledger's own
/// rendered `pci-usb d=`, and identical-by-construction is worth having over almost-always-equal
/// when it costs one expression: it makes "the same clock" true of the printed digits and not
/// merely of the counter. The `hz >= 1000` guard is `Dur`'s too, so a sub-kHz rate falls to raw
/// ticks in both places rather than dividing by zero in one of them; it also retires the
/// `saturating_mul` ceiling the old form carried.
fn gpace_fmt(cy: u64) -> (u64, &'static str) {
    let hz = crate::bootpace::origin_hz();
    if hz >= 1000 { (cy / (hz / 1000), "ms") } else { (cy, "cy") }
}

// The compiled knob set, as `&'static str` fragments a `no_std` format can concatenate without an
// allocator. This is the field that stops a bench-media 4600 ms from being read as something a
// default boot pays.
#[cfg(feature = "nvidia-kepler")]
const GB_KEPLER: &str = "kepler+";
#[cfg(not(feature = "nvidia-kepler"))]
const GB_KEPLER: &str = "";
#[cfg(feature = "nvidia-kepler-takeover")]
const GB_TAKEOVER: &str = "takeover+";
#[cfg(not(feature = "nvidia-kepler-takeover"))]
const GB_TAKEOVER: &str = "";
#[cfg(feature = "nvidia-kepler-fifo")]
const GB_FIFO: &str = "fifo+";
#[cfg(not(feature = "nvidia-kepler-fifo"))]
const GB_FIFO: &str = "";
#[cfg(feature = "intel-ivb")]
const GB_IVB: &str = "ivb+";
#[cfg(not(feature = "intel-ivb"))]
const GB_IVB: &str = "";
#[cfg(feature = "wc")]
const GB_WC: &str = "wc+";
#[cfg(not(feature = "wc"))]
const GB_WC: &str = "";
#[cfg(feature = "smc")]
const GB_SMC: &str = "smc+";
#[cfg(not(feature = "smc"))]
const GB_SMC: &str = "";
// The three BENCH-RIDE probes are named INDIVIDUALLY rather than rolled into one `benchride+`.
// `bench` is the only class whose `n=` counts more than one call site, so a single fragment left
// `bench=..ms(n=2)` unresolvable — two of three ran, and no reading said which two. Every other
// class is disambiguated by its own fragment (`kepler=0ms(n=0)` against `kepler+`), and this was
// the one place the none-vs-zero rule broke. Worse, `thermprobe` pulls in `smc`, so it was already
// showing up twice in `build=` under two different names. With three fragments, `bench`'s `n=` is
// fully determined by `build=`.
#[cfg(feature = "thermprobe")]
const GB_THERM: &str = "therm+";
#[cfg(not(feature = "thermprobe"))]
const GB_THERM: &str = "";
#[cfg(feature = "pcilink")]
const GB_PCILINK: &str = "pcilink+";
#[cfg(not(feature = "pcilink"))]
const GB_PCILINK: &str = "";
#[cfg(feature = "vromprobe")]
const GB_VROM: &str = "vrom+";
#[cfg(not(feature = "vromprobe"))]
const GB_VROM: &str = "";
/// Prints in place of the fragments when NONE of them is compiled in, so "default build" is a
/// positive statement in the log rather than an empty field a reader has to interpret.
const GB_NONE: &str = if cfg!(any(
    feature = "nvidia-kepler",
    feature = "nvidia-kepler-takeover",
    feature = "nvidia-kepler-fifo",
    feature = "intel-ivb",
    feature = "wc",
    feature = "smc",
    feature = "thermprobe",
    feature = "pcilink",
    feature = "vromprobe",
)) {
    ""
} else {
    "default(no-gpu-knobs)"
};

/// The class-0x02 network block, lifted out of `init` verbatim.
///
/// It is a separate function for one reason: it contains an early `return` on a non-Intel NIC, and
/// this machine's Broadcom 0x14e4 takes that branch on EVERY boot. While the block was inline, any
/// report placed after it was unreachable on the very machine it existed to measure — an instrument
/// that cannot run in the state it reports on. The `return` now leaves the HELPER; `pci::init` has
/// exactly one exit, and the GPACE report sits on it.
fn init_network() {
    // Network controller (PCI class 0x02 = Network, subclass 0x00 = Ethernet).
    // QEMU's e1000 (82540EM) lands here; bring it up for polled RX.
    if let Some((bus, slot, func)) = crate::drivers::pci::PciScanner::find_device(0x02, 0x00) {
        let vendor = unsafe { read_config_16(bus, slot, func, 0x00) };
        serial_println!(
            ":: x86_64 PCI: Found network controller (class 0x02) vendor {:#06x} at {}:{}.{} ::",
            vendor, bus, slot, func
        );
        // Only the Intel e1000/e1000e family is supported. On a real 2012 MacBook Pro the NIC is a
        // Broadcom Wi-Fi part (vendor 0x14e4) that also reports class 0x02 — poking it with e1000
        // register writes is wrong and its RX/TX bring-up (+ DHCP) just stalls. Gate to Intel.
        if vendor != 0x8086 {
            serial_println!(":: x86_64 PCI: non-Intel NIC ({:#06x}) — no e1000 driver, skipping ::", vendor);
            return;
        }
        crate::drivers::e1000::init(bus, slot, func);
        // Route the NIC's RX interrupt to the BSP local APIC via MSI (IDT vector 0x41),
        // the same local-APIC delivery the xHCI uses. The e1000e keeps its MSI-X table in
        // BAR3 (not mappable by enable_msix), so plain MSI is used.
        let msg_addr = 0xFEE0_0000u32 | ((crate::arch::apic::apic_id() as u32) << 12);
        crate::drivers::e1000::enable_interrupts(
            bus, slot, func, msg_addr,
            crate::arch::interrupts::NIC_MSI_VECTOR as u32,
        );
    } else {
        serial_println!(":: x86_64 PCI: No network controller (class 0x02) found ::");
    }
}

pub fn init(_dtb_addr: u64, _dtb_size: usize) {
    // GPACE: the phase accumulators for everything BPACE lumps into `pci-usb d=`. Plain memory,
    // no lock, no allocation — see the module block above `G_XTAIL`.
    let mut pace = Gpace::new();

    // BPACE (M4): entry to the whole PCI/USB bring-up. Placed FIRST so that `d=` from `sched` is
    // step 4e (`apic::report_tick_rate` — a 50 ms PM-timer window) and nothing else, and so a boot
    // that dies anywhere below still shows that it got this far. Everything from here to
    // `pci-usb` used to be one undivided delta; see `docs/dev/OS/01_BOOT_HAL/bootpace.md` §6a.
    crate::bootpace::record("pci-enter");

    // VPERF (bench builds only): read-only display diagnostics — the effective framebuffer memory
    // type (MTRR + live PTE + PAT) and which class-0x03 device's BAR owns the fb address. Rides
    // the PCI-init point so the lines land once, early, in every knob-ON boot log.
    #[cfg(feature = "videobench")]
    {
        crate::video::vperf::report_fbmem();
        crate::video::vperf::pci_display_probe();
    }

    // EHCI-1 scout (opt-in probe, UNAOS_EHCISCOUT=1 -> `ehciscout_run`): STRICTLY READ-ONLY EHCI reconnaissance — dump the
    // EHCI companion controllers' PCI/MMIO/PORTSC state so an EHCI driver arc can be planned against
    // real register evidence (the 2012 rMBP internal kbd/trackpad live on EHCI-only ports). Runs
    // independently of the xHCI scan below; issues NO writes to any register or port. Knob OFF =>
    // this call does not exist and the module is unlinked (media byte-identical).
    #[cfg(feature = "ehciscout_run")]
    crate::drivers::ehci_scout::scout();

    // EHCI-2 configure-and-relook (opt-in probe, UNAOS_EHCICONFIG=1 -> `ehciconfig_run`): after the read-only census, run a
    // MINIMAL EHCI wake sequence (PMCSR->D0, USBLEGSUP OS-ownership handshake, RS=1, CONFIGFLAG=1 +
    // port-power) with two PORTSC censuses (before/after CONFIGFLAG=1) so the attended rMBP sitting
    // can distinguish asleep-until-configured USB internals from not-USB. Writes are confined to the
    // EHCI functions' own registers; it never touches xHCI routing, never enumerates, never transfers.
    // Knob OFF => this call does not exist and the config path is unlinked (module byte-identical to
    // the EHCI-1 read-only scout).
    #[cfg(feature = "ehciconfig_run")]
    crate::drivers::ehci_scout::configure_and_relook();

    // EHCI-3 driver (EHCI-4 M1: DEFAULT-ON, opt out with UNAOS_NOEHCIHID=1): the minimal EHCI HID
    // driver — shared EHCI-2 wake, port reset, synchronous EP0 enumeration (M1 topology-fork
    // witness), periodic interrupt-IN QHs feeding boot reports to the main-loop service_ehci_hid()
    // poll. Runs BEFORE the PORTSW flip + xhci::init below: the internal HID sit on NON-switchable
    // EHCI-only ports (PORTSW-1 §7f), so the port sets are disjoint by hardware and the two stacks
    // coexist permanently. Never touches an xHCI register. Opt-out => this call + the module unlink.
    //
    // BPACE (M4): stamped on BOTH sides. `ehci::init` is default-ON on the metal build, walks all
    // 256 PCI buses of config space, then runs a wake + port-reset + synchronous EP0 enumeration per
    // EHCI function — every one of those waits is `wait_bounded`, so the phase can legitimately cost
    // hundreds of ms and a boot that dies inside it must still show that it ENTERED it. A single
    // trailing stamp could not tell "EHCI was slow" from "EHCI never ran".
    crate::bootpace::record("ehci-hid");
    #[cfg(feature = "ehcihid")]
    crate::drivers::ehci::init();
    crate::bootpace::record("ehci-hid-done");

    // BATMON-1 (UNAOS_SMC=1): fire the Apple SMC key-inventory scout once, here at the early x86
    // bring-up point (like the EHCI scout above) so its `:: SMC-SCOUT: ... ::` lines land once in
    // every knob-ON boot log. Read-only w.r.t. SMC state (port I/O confined to 0x300/0x304, no
    // value-mutating WRITE_CMD). Knob OFF => this call does not exist and the module is unlinked.
    #[cfg(feature = "smc")]
    {
        crate::drivers::smc::scout();
        // M2: take one battery reading at boot so every knob-ON boot log carries a `:: SMC-BATT: ::`
        // witness proving the snapshot read path ran (honest `present=false` on QEMU / a machine
        // without a battery). The main loops + the vug meter cadence keep refreshing it live.
        crate::drivers::smc::battery::refresh_if_due();
    }

    // BPACE (M4): the xHCI bus scan. Split from the `if let` so the stamp lands whether or not a
    // controller was found — on a machine with no xHCI the tag is still present and `pci-usb`
    // follows it directly, which is how the ledger distinguishes "no controller" from "the scan
    // hung". Config-space reads only, no waits: a single trailing stamp is sufficient here.
    let xhci_found = crate::drivers::pci::PciScanner::scan();
    crate::bootpace::record("pci-scan");
    if let Some((xhci_phys_addr, bus, dev, func)) = xhci_found {
        serial_println!(":: x86_64 PCI Init: Found xHCI at {:#x} ::", xhci_phys_addr);

        // DIAGNOSTIC (read-only): dump interrupt line/pin + capability list to plan
        // interrupt-driven bring-up (INTx IRQ vs MSI-X).
        crate::drivers::pci::PciScanner::probe_irq_caps(bus, dev, func);

        // Enable PCI Memory Space + Bus Master (DMA) for the controller. Without Bus
        // Master the xHCI can never fetch command TRBs or write event TRBs.
        crate::drivers::pci::PciScanner::enable_bus_master(bus, dev, func);

        // PORTSW-1 (DEFAULT-ON, opt out with UNAOS_NOPORTSW=1): route USB2/USB3 ports from the EHCI
        // companion to xHCI BEFORE the controller starts, so the 2012 rMBP internal keyboard/trackpad
        // re-enumerate on the xHCI+HID stack. Mask-disciplined config-space writes; inert on non-Intel
        // (QEMU). Metal-gated policy: on the 2012 rMBP the no-routing boot dropped ALL external USB, so
        // routing runs by default. Opt out (UNAOS_NOPORTSW=1) => this call does not exist and no
        // config-space write is issued (the no-routing EHCI-internal/xHCI-external topology).
        #[cfg(not(feature = "noportsw"))]
        enable_intel_xhci_ports(bus, dev, func);

        // BPACE (M4): the PCI-side preamble to the controller — `probe_irq_caps`,
        // `enable_bus_master`, and the PORTSW-1 routing flip. All config-space I/O with no
        // handshake wait, so `d=` here should be ~0; on a `noportsw` build the tag still records
        // (the flip is what is absent, not the stamp) and `d=` is the two capability accesses
        // alone. Immediately precedes `xhci::init`, so it doubles as that call's entry marker.
        crate::bootpace::record("portsw");

        // Initialize xHCI
        crate::drivers::xhci::init(xhci_phys_addr); // Reset and command ring

        unsafe {
            let mut xhci = crate::drivers::xhci::XhciController::new(xhci_phys_addr as usize);

            // Allocate the global command + event rings, capture their physical
            // addresses, then DROP the guards before start()/poll so nothing can
            // deadlock by re-locking them later.
            let (event_ring_phys, command_ring_phys) = {
                let mut cmd_ring_guard = crate::drivers::xhci::COMMAND_RING.lock();
                let mut evt_ring_guard = crate::drivers::xhci::EVENT_RING.lock();

                *cmd_ring_guard = Some(crate::drivers::xhci::ring::TransferRing::new(256));
                *evt_ring_guard = Some(crate::drivers::xhci::event::EventRing::new());

                (evt_ring_guard.as_mut().unwrap().get_ptr(),
                 cmd_ring_guard.as_mut().unwrap().get_ptr())
            };

            let erst_table_phys = &raw mut crate::drivers::xhci::ERST_TABLE as u64;
            xhci.init_interrupter(event_ring_phys, erst_table_phys);

            // Route the controller's interrupts via MSI-X straight to the local APIC (no
            // 8259, no I/O APIC). init_interrupter just published the IR0/OP MMIO bases the
            // handler needs; IMAN.IE is set there and USBCMD.INTE in start(). The MSI message
            // targets the BSP local APIC (0xFEE00000 | dest_id<<12) at IDT vector 0x40.
            let msg_addr = 0xFEE0_0000u32 | ((crate::arch::apic::apic_id() as u32) << 12);
            crate::drivers::pci::PciScanner::enable_msix(
                bus, dev, func, xhci_phys_addr, msg_addr,
                crate::arch::interrupts::XHCI_MSI_VECTOR as u32,
            );

            xhci.init_pointers(command_ring_phys);

            // BPACE (M4): the ring/interrupter programming is done and RS has NOT been set yet.
            // `d=` from `xhci-cnr` covers the heap allocation of the command + event rings,
            // `init_interrupter` (which re-runs `wait_for_cnr_clear`, a `hw_wait_budget()`-bounded
            // wait), the MSI-X programming and `init_pointers`. Stamped before `start()` so a boot
            // that wedges inside `start()` still shows the controller was fully programmed.
            crate::bootpace::record("xhci-ptrs");

            xhci.start();

            // Store globally
            crate::drivers::xhci::install(xhci);
        }
    }

    // ── GPACE anchor ────────────────────────────────────────────────────────────────────────────
    // Everything measured from here on is what BPACE will charge to `pci-usb d=`. The anchor is the
    // ledger's LAST stamp at this instant — the one BPACE itself will subtract — read out of the
    // ring without adding to it. On this machine that is `enum:p1`, because `xhci.start()` kicks
    // port-1 enumeration before returning; with no xHCI present it is `pci-scan`, and the arithmetic
    // is unchanged. `anchor=` is printed so the reader never has to assume which it was.
    //
    // `xtail` therefore holds the `start_next_port` tail: whatever `xhci.start()` did after its last
    // stamp, plus publishing the controller into `XHCI_CONTROLLER`.
    let (anchor_cy, anchor_tag) = crate::bootpace::last_stamp()
        .unwrap_or((crate::arch::now_cycles(), "none"));
    pace.add(G_XTAIL, anchor_cy);

    // BENCH-RIDE probes run here — serial is live (post-xHCI) and the GPU dispatch hasn't run,
    // so their evidence survives a GPU-init wedge. All read-only, knob-gated, one-shot.
    // GPACE: each probe closes into `bench`, so `n=` counts how many of the three were compiled in
    // — `0ms(n=0)` on the media that carries none of them, which is the default.
    #[cfg(all(target_arch = "x86_64", feature = "thermprobe"))]
    {
        let t = crate::arch::now_cycles();
        crate::drivers::bench_ride::therm_snapshot();
        pace.add(G_BENCH, t);
    }
    #[cfg(all(target_arch = "x86_64", feature = "pcilink"))]
    {
        let t = crate::arch::now_cycles();
        crate::drivers::bench_ride::pcilink_snapshot();
        pace.add(G_BENCH, t);
    }
    #[cfg(all(target_arch = "x86_64", feature = "vromprobe"))]
    {
        let t = crate::arch::now_cycles();
        crate::drivers::bench_ride::vrom_sniff();
        pace.add(G_BENCH, t);
    }

    // GPU init runs AFTER the xHCI block: bench serial on the rMBP is the usbdebug FTDI behind
    // xHCI, so any GPU-init wedge before this point is invisible (zero serial from power-on —
    // sitting #7 boot 2 hard-hang signature). After xHCI is up, a wedge leaves breadcrumbs.
    // GPACE: the GPU dispatch is measured entirely from OUT HERE, at its call sites. `detect`,
    // `igpu` and `kepler` are wall-clock spans around calls this file already makes; not one line
    // is added to `gpu/detect.rs`, `gpu/igpu.rs` or `gpu/kepler.rs`, which belong to other lanes.
    // Each span therefore contains everything its callee did — MMIO, settles, AND the serial
    // printing of its own witness lines. When the whole block is compiled out (a default build) all
    // three classes read `0ms(n=0)`: never measured, not measured-as-zero.
    #[cfg(all(target_arch = "x86_64", any(feature = "nvidia-kepler", feature = "intel-ivb")))]
    {
        let t_detect = crate::arch::now_cycles();
        let gpus = crate::drivers::gpu::detect::detect_gpus();
        pace.add(G_DETECT, t_detect);
        let mut kepler_found = false;
        for gpu in &gpus {
            match gpu.gpu_type {
                #[cfg(feature = "nvidia-kepler")]
                crate::drivers::gpu::detect::GpuType::NvidiaKepler => {
                    kepler_found = true;
                    let t = crate::arch::now_cycles();
                    crate::drivers::gpu::kepler::init(gpu);
                    pace.add(G_KEPLER, t);
                }
                #[cfg(feature = "intel-ivb")]
                crate::drivers::gpu::detect::GpuType::IntelIvyBridge => {
                    let t = crate::arch::now_cycles();
                    crate::drivers::gpu::igpu::init(gpu);
                    pace.add(G_IGPU, t);
                }
                _ => {}
            }
        }
        #[cfg(feature = "nvidia-kepler")]
        if !kepler_found {
            serial_println!(":: kepler: no-device ::");
        }

        // DEFERRED CALL SITE FOR FLIGHT 1 GMUX SWITCH:
        // Here, Kepler has finished its takeover. If we switch GMUX to IGD now,
        // it overrides Kepler's routing. We then rerun the iGPU plane census
        // and arm the blitter ring.
        #[cfg(all(feature = "gmux_igd", feature = "intel-ivb"))]
        unsafe { crate::drivers::gpu::igpu::gmux_igd_switch() };
    }

    // SDHC-1 (milestone 1): storage-class PCI census + the read-only SD Host Controller
    // version/capability probe. Runs HERE — after the GPU dispatch (paging + the frame allocator
    // the MMIO mapping needs are long up) and BEFORE the network block, whose non-Intel early exit
    // would otherwise have swallowed the witness on the 2012 rMBP (Broadcom Wi-Fi). GPACE moved
    // that exit into `init_network()`, so the ordering is no longer load-bearing for THIS witness —
    // but it stays, because the sequence is what the metal baselines were taken against.
    // Read-only end to end: no reset, no clock/power programming, no command, no config write.
    {
        let t = crate::arch::now_cycles();
        crate::drivers::sdhc::probe();
        pace.add(G_SDHC, t);
    }

    // GPACE: the network block is `init_network()` precisely so its non-Intel early exit cannot
    // swallow the report below — see the helper's doc comment. `n=1` always; a Broadcom skip is a
    // measured, cheap `nic=`, not an absence.
    {
        let t = crate::arch::now_cycles();
        init_network();
        pace.add(G_NIC, t);
    }

    // ── GPACE report — the one unconditional exit of `pci::init` ────────────────────────────────
    // Two lines (three when the tripwire fires). The first is the split; the second is the
    // self-check plus the build identity.
    //
    // `resid = span - Σ(classes)` closes over the PRINTED MILLISECOND VALUES, not over the cycle
    // counts. That distinction is the whole point of the field. Each class is floored to ms
    // independently, so a cycle-domain residual then floored a ninth time leaves the printed row
    // short of the printed `span` by up to `N_GPACE + 1` ms — noise against the 4620 ms armed
    // block, but up to 8% of the ~100 ms default-build baseline that §9a calls load-bearing, and
    // sitting in exactly the field whose job is to prove nothing went unattributed. A reader adds
    // up the line; the line must add up. `Σfloor(x_k) ≤ floor(Σx_k) ≤ floor(span)` makes the
    // printed-domain subtraction non-negative by construction, so the flooring loss lands in
    // `resid` — which already means "unattributed" — and the row closes exactly.
    //
    // A class that under-counts therefore still shows up as unattributed time rather than as a
    // tidy-looking total. `span` must equal the independent BPACE `pci-usb d=` to the millisecond:
    // same anchor stamp, same `hz`, and since `gpace_fmt` now shares `Dur`'s divisor, the same
    // rendering. The only slack is these two lines' own print cost, which lands inside `pci-usb`
    // and outside `span` — measured at 1 ms (§9).
    {
        let now = crate::arch::now_cycles();
        let span = now.wrapping_sub(anchor_cy);

        // TRIPWIRE. The cycle comparison survives here — not as the printed residual, but as the
        // one reading that can convict the tiling. `Σ > span` is what overlapping spans, a wrong
        // anchor, or a non-monotonic `now_cycles()` across cores would produce; the previous
        // `saturating_sub` rendered every one of those as `resid=0ms`, which is ALSO the healthy
        // reading. Clamping a broken instrument into the shape of a working one is this seat's
        // recurring defect, and it had no business living inside the instrument built to avoid it.
        // One branch, never taken in a sound build, turns that mask into a conviction.
        let sum_cy = pace.sum();
        if sum_cy > span {
            serial_println!(
                ":: GPACE: OVERLAP sum>span by {}cy — classes are not disjoint ::",
                sum_cy - span
            );
        }

        let mut v = [0u64; N_GPACE];
        let mut unit = "ms";
        let mut k = 0;
        while k < N_GPACE {
            let (val, u) = gpace_fmt(pace.cy[k]);
            v[k] = val;
            unit = u;
            k += 1;
        }
        let (sv, su) = gpace_fmt(span);
        // Printed-domain closure: `resid` is what the printed span has left after the printed
        // classes. In a sound build — equivalently, whenever the tripwire above stayed silent —
        // the row sums to `span` exactly, for any `hz`, including the `cy` case (which floors
        // nothing, so `rv = span - Σ` outright). The OVERLAP case is the deliberate exception: the
        // row does NOT close there and `resid=0ms` still prints, which is precisely why that
        // condition gets a line of its own rather than being left to this subtraction. The
        // `saturating_sub` is therefore never silent — `named > sv` implies `Σcy > span`, which is
        // exactly the tripwire's condition.
        let mut named = 0u64;
        let mut j = 0;
        while j < N_GPACE {
            named = named.saturating_add(v[j]);
            j += 1;
        }
        let rv = sv.saturating_sub(named);
        serial_println!(
            ":: GPACE: {}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) resid={}{} == witness ::",
            GPACE_TAGS[G_XTAIL], v[G_XTAIL], unit, pace.n[G_XTAIL],
            GPACE_TAGS[G_BENCH], v[G_BENCH], unit, pace.n[G_BENCH],
            GPACE_TAGS[G_DETECT], v[G_DETECT], unit, pace.n[G_DETECT],
            GPACE_TAGS[G_IGPU], v[G_IGPU], unit, pace.n[G_IGPU],
            GPACE_TAGS[G_KEPLER], v[G_KEPLER], unit, pace.n[G_KEPLER],
            GPACE_TAGS[G_SDHC], v[G_SDHC], unit, pace.n[G_SDHC],
            GPACE_TAGS[G_NIC], v[G_NIC], unit, pace.n[G_NIC],
            rv, unit
        );
        // `since-entry=` correlates this line with the ledger's `t=` column. `entry` is the boot's
        // first stamp and cannot legitimately be missing here; if it ever is, the unit prints `?`
        // rather than a zero that would read as a real measurement.
        let (tv, tu) = match crate::bootpace::cycles_of("entry") {
            Some(e) => gpace_fmt(now.wrapping_sub(e)),
            None => (0, "?"),
        };
        serial_println!(
            ":: GPACE: span={}{} anchor={} since-entry={}{} hz={} build={}{}{}{}{}{}{}{}{}{} == the pci-usb d= split ::",
            sv, su, anchor_tag, tv, tu, crate::bootpace::origin_hz(),
            GB_NONE, GB_KEPLER, GB_TAKEOVER, GB_FIFO, GB_IVB, GB_WC, GB_SMC,
            GB_THERM, GB_PCILINK, GB_VROM
        );
        #[cfg(feature = "intel-ivb")]
        crate::drivers::gpu::igpu::print_blt_stats();
    }
}
