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
    }
}

pub fn init(_dtb_addr: u64, _dtb_size: usize) {
    // VPERF (bench builds only): read-only display diagnostics — the effective framebuffer memory
    // type (MTRR + live PTE + PAT) and which class-0x03 device's BAR owns the fb address. Rides
    // the PCI-init point so the lines land once, early, in every knob-ON boot log.
    #[cfg(feature = "videobench")]
    {
        crate::video::vperf::report_fbmem();
        crate::video::vperf::pci_display_probe();
    }

    // EHCI-1 scout (opt-in, UNAOS_EHCISCOUT=1): STRICTLY READ-ONLY EHCI reconnaissance — dump the
    // EHCI companion controllers' PCI/MMIO/PORTSC state so an EHCI driver arc can be planned against
    // real register evidence (the 2012 rMBP internal kbd/trackpad live on EHCI-only ports). Runs
    // independently of the xHCI scan below; issues NO writes to any register or port. Knob OFF =>
    // this call does not exist and the module is unlinked (media byte-identical).
    #[cfg(feature = "ehciscout")]
    crate::drivers::ehci_scout::scout();

    // EHCI-2 configure-and-relook (opt-in, UNAOS_EHCICONFIG=1): after the read-only census, run a
    // MINIMAL EHCI wake sequence (PMCSR->D0, USBLEGSUP OS-ownership handshake, RS=1, CONFIGFLAG=1 +
    // port-power) with two PORTSC censuses (before/after CONFIGFLAG=1) so the attended rMBP sitting
    // can distinguish asleep-until-configured USB internals from not-USB. Writes are confined to the
    // EHCI functions' own registers; it never touches xHCI routing, never enumerates, never transfers.
    // Knob OFF => this call does not exist and the config path is unlinked (module byte-identical to
    // the EHCI-1 read-only scout).
    #[cfg(feature = "ehciconfig")]
    crate::drivers::ehci_scout::configure_and_relook();

    if let Some((xhci_phys_addr, bus, dev, func)) = crate::drivers::pci::PciScanner::scan() {
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
            xhci.start();

            // Store globally
            *crate::drivers::xhci::XHCI_CONTROLLER.lock() = Some(xhci);
        }
    }

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
