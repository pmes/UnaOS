// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.
//
// Intel 8254x / e1000e (QEMU `-device e1000e` = 82574L) network driver. Mirrors the
// xHCI/`drivers/block.rs` patterns: a global `NET_DEVICE` registry populated during PCI
// init, drained from the main loop via `service_net()`. DMA structures use the same "heap
// pointer == physical address" identity-map invariant the xHCI rings rely on (the heap is
// initialised directly at `region.phys_start`, and the kernel runs on UEFI's 1:1 page tables).
//
// Scope: PCI BAR map, software reset, MAC read (RAL/RAH, EEPROM fallback), RX + TX
// legacy-descriptor rings (DMA), promiscuous receive, link up, and a polled drain into
// `net::ingress` (which answers ARP / ICMP echo / UDP echo). RX is also interrupt-driven:
// the e1000e's RX interrupt is delivered as an MSI to the local APIC (IDT vector 0x41) to
// wake the CPU from `hlt`; the lock-free handler only ACKs + EOIs, the main loop does the work.
// (We use MSI rather than MSI-X because the e1000e keeps its MSI-X table in BAR3.)

use core::alloc::Layout;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::drivers::pci::PciScanner;
use net::arp::ArpStateMachine;
use net::ethernet::EthernetFrame;

// --- Register offsets (bytes from BAR0) ---
const REG_CTRL: u32 = 0x0000; // Device Control
const REG_STATUS: u32 = 0x0008; // Device Status
const REG_EERD: u32 = 0x0014; // EEPROM Read
const REG_ICR: u32 = 0x00C0; // Interrupt Cause Read (reading clears the causes)
const REG_IMS: u32 = 0x00D0; // Interrupt Mask Set
const REG_IMC: u32 = 0x00D8; // Interrupt Mask Clear
const REG_RCTL: u32 = 0x0100; // Receive Control
const REG_RDBAL: u32 = 0x2800; // RX Descriptor Base Low
const REG_RDBAH: u32 = 0x2804; // RX Descriptor Base High
const REG_RDLEN: u32 = 0x2808; // RX Descriptor Length (bytes)
const REG_RDH: u32 = 0x2810; // RX Descriptor Head
const REG_RDT: u32 = 0x2818; // RX Descriptor Tail
const REG_MTA: u32 = 0x5200; // Multicast Table Array (128 x u32)
const REG_RAL0: u32 = 0x5400; // Receive Address Low [0]
const REG_RAH0: u32 = 0x5404; // Receive Address High [0] (RA array has an 8-byte stride: RAL +0, RAH +4)
const REG_TCTL: u32 = 0x0400; // Transmit Control
const REG_TIPG: u32 = 0x0410; // Transmit Inter-Packet Gap
const REG_TDBAL: u32 = 0x3800; // TX Descriptor Base Low
const REG_TDBAH: u32 = 0x3804; // TX Descriptor Base High
const REG_TDLEN: u32 = 0x3808; // TX Descriptor Length (bytes)
const REG_TDH: u32 = 0x3810; // TX Descriptor Head
const REG_TDT: u32 = 0x3818; // TX Descriptor Tail

// --- CTRL bits ---
const CTRL_SLU: u32 = 1 << 6; // Set Link Up
const CTRL_ASDE: u32 = 1 << 5; // Auto-Speed Detection Enable
const CTRL_LRST: u32 = 1 << 3; // Link Reset
const CTRL_ILOS: u32 = 1 << 7; // Invert Loss-of-Signal
const CTRL_RST: u32 = 1 << 26; // Device Reset (self-clearing)
const CTRL_PHY_RST: u32 = 1 << 31; // PHY Reset

// --- STATUS bits ---
const STATUS_LU: u32 = 1 << 1; // Link Up

// --- RCTL bits ---
const RCTL_EN: u32 = 1 << 1; // Receiver Enable
const RCTL_UPE: u32 = 1 << 3; // Unicast Promiscuous Enable
const RCTL_MPE: u32 = 1 << 4; // Multicast Promiscuous Enable
const RCTL_BAM: u32 = 1 << 15; // Broadcast Accept Mode
const RCTL_SECRC: u32 = 1 << 26; // Strip Ethernet CRC
// Buffer size 2048 == RCTL.BSIZE bits[17:16]=00 with BSEX(25)=0 (the reset default).

// --- TCTL bits ---
const TCTL_EN: u32 = 1 << 1; // Transmit Enable
const TCTL_PSP: u32 = 1 << 3; // Pad Short Packets
const TCTL_CT_SHIFT: u32 = 4; // Collision Threshold
const TCTL_COLD_SHIFT: u32 = 12; // Collision Distance
// Standard IEEE 802.3 inter-packet-gap timing for the 82540 (IPGT=10, IPGR1, IPGR2).
const TIPG_DEFAULT: u32 = 0x0060_200A;

// --- TX descriptor cmd/status bits ---
const TX_CMD_EOP: u8 = 1 << 0; // End Of Packet
const TX_CMD_IFCS: u8 = 1 << 1; // Insert FCS (CRC)
const TX_CMD_RS: u8 = 1 << 3; // Report Status (sets DD when done)
const TX_STATUS_DD: u8 = 1 << 0; // Descriptor Done

// --- Interrupt cause/mask bits ---
const IMS_RXT0: u32 = 1 << 7; // Receiver Timer Interrupt (fires when a packet is received)

// --- EEPROM (EERD) bits, 82540EM layout ---
const EERD_START: u32 = 1 << 0;
const EERD_DONE: u32 = 1 << 4;

// --- RX descriptor status bits ---
const RX_STATUS_DD: u8 = 1 << 0; // Descriptor Done (hardware wrote it)

/// Number of RX descriptors. `NUM_RX * 16` must be a multiple of 128 (RDLEN
/// alignment): 32 * 16 = 512 satisfies this.
const NUM_RX: usize = 32;
/// Per-descriptor packet buffer size. 2048 matches RCTL.BSIZE=2048.
const RX_BUF_SIZE: usize = 2048;

/// Number of TX descriptors (`NUM_TX * 16` must be a 128-byte multiple: 8 * 16 = 128).
const NUM_TX: usize = 8;
/// Per-descriptor transmit buffer size (one full Ethernet frame fits).
const TX_BUF_SIZE: usize = 2048;

/// Static IP we answer ARP for (slirp's default DHCP lease).
const OUR_IP: [u8; 4] = [10, 0, 2, 15];
/// slirp's virtual gateway. We ARP-probe it at bring-up to exercise TX and provoke a
/// real inbound frame (slirp answers ARP for its gateway), proving the RX path too.
const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];

/// Legacy receive descriptor (16 bytes). Written by hardware via DMA, so every
/// access goes through `read_volatile`/`write_volatile` on whole-struct copies —
/// the struct is `packed`, so taking a reference to a field would be unaligned UB.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct RxDesc {
    addr: u64,     // physical address of the packet buffer
    length: u16,   // bytes written by hardware
    checksum: u16,
    status: u8,    // bit0 = DD, bit1 = EOP
    errors: u8,
    special: u16,
}

/// Legacy transmit descriptor (16 bytes). Like `RxDesc`, accessed only via
/// whole-struct volatile reads/writes (it is `packed`).
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct TxDesc {
    addr: u64,   // physical address of the frame buffer
    length: u16, // frame length in bytes
    cso: u8,     // checksum offset
    cmd: u8,     // EOP | IFCS | RS ...
    status: u8,  // bit0 = DD once transmitted
    css: u8,     // checksum start
    special: u16,
}

pub struct E1000 {
    mmio_base: usize,
    mac: [u8; 6],
    rx_ring: *mut RxDesc,
    rx_buffers: *mut u8,
    rx_cur: usize,
    rx_count: u64,
    tx_ring: *mut TxDesc,
    tx_buffers: *mut u8,
    tx_cur: usize,
    tx_count: u64,
    arp_state: ArpStateMachine,
}

// The driver owns raw DMA pointers; it is only ever touched behind `NET_DEVICE`'s
// Mutex, so sharing across the (single-CPU) interrupt/main contexts is sound.
unsafe impl Send for E1000 {}

impl E1000 {
    #[inline]
    fn reg_read(&self, off: u32) -> u32 {
        unsafe { read_volatile((self.mmio_base + off as usize) as *const u32) }
    }

    #[inline]
    fn reg_write(&self, off: u32, val: u32) {
        unsafe { write_volatile((self.mmio_base + off as usize) as *mut u32, val) }
    }

    /// Read a 16-bit EEPROM word (used only as a MAC fallback).
    fn eeprom_read(&self, addr: u8) -> u16 {
        self.reg_write(REG_EERD, EERD_START | ((addr as u32) << 8));
        // Bounded poll — QEMU completes EEPROM reads immediately.
        for _ in 0..100_000 {
            let v = self.reg_read(REG_EERD);
            if v & EERD_DONE != 0 {
                return (v >> 16) as u16;
            }
            core::hint::spin_loop();
        }
        0
    }

    /// Read the station MAC. QEMU reloads RAR[0] from the configured address on
    /// reset, so RAL0/RAH0 are normally valid; fall back to EEPROM words 0..3 if not.
    fn read_mac(&self) -> [u8; 6] {
        let ral = self.reg_read(REG_RAL0);
        let rah = self.reg_read(REG_RAH0);
        let av = (rah & (1 << 31)) != 0;
        if av || ral != 0 || (rah & 0xFFFF) != 0 {
            return [
                ral as u8,
                (ral >> 8) as u8,
                (ral >> 16) as u8,
                (ral >> 24) as u8,
                rah as u8,
                (rah >> 8) as u8,
            ];
        }
        // EEPROM fallback: words 0,1,2 hold the 6 MAC bytes (little-endian per word).
        let mut mac = [0u8; 6];
        for word in 0..3 {
            let w = self.eeprom_read(word as u8);
            mac[word * 2] = w as u8;
            mac[word * 2 + 1] = (w >> 8) as u8;
        }
        mac
    }

    /// Allocate the RX descriptor ring + contiguous packet buffers and point each
    /// descriptor at its buffer. The `alloc_zeroed` pointer doubles as the physical
    /// address (identity map), matching the xHCI ring allocation.
    fn alloc_rx(&mut self) {
        let ring_layout = Layout::from_size_align(NUM_RX * core::mem::size_of::<RxDesc>(), 4096).unwrap();
        let buf_layout = Layout::from_size_align(NUM_RX * RX_BUF_SIZE, 4096).unwrap();
        self.rx_ring = unsafe { alloc::alloc::alloc_zeroed(ring_layout) as *mut RxDesc };
        self.rx_buffers = unsafe { alloc::alloc::alloc_zeroed(buf_layout) };

        for i in 0..NUM_RX {
            let buf_phys = (self.rx_buffers as u64) + (i * RX_BUF_SIZE) as u64;
            let desc = RxDesc {
                addr: buf_phys,
                length: 0,
                checksum: 0,
                status: 0,
                errors: 0,
                special: 0,
            };
            unsafe { write_volatile(self.rx_ring.add(i), desc) };
        }
    }

    /// Allocate the TX descriptor ring + contiguous frame buffers. Descriptors start
    /// with DD set so the first `transmit()` doesn't block waiting on a stale slot.
    fn alloc_tx(&mut self) {
        let ring_layout = Layout::from_size_align(NUM_TX * core::mem::size_of::<TxDesc>(), 4096).unwrap();
        let buf_layout = Layout::from_size_align(NUM_TX * TX_BUF_SIZE, 4096).unwrap();
        self.tx_ring = unsafe { alloc::alloc::alloc_zeroed(ring_layout) as *mut TxDesc };
        self.tx_buffers = unsafe { alloc::alloc::alloc_zeroed(buf_layout) };

        for i in 0..NUM_TX {
            let desc = TxDesc {
                addr: 0,
                length: 0,
                cso: 0,
                cmd: 0,
                status: TX_STATUS_DD, // mark free
                css: 0,
                special: 0,
            };
            unsafe { write_volatile(self.tx_ring.add(i), desc) };
        }
    }

    /// Full software reset + RX bring-up. Returns once the receiver is enabled.
    fn reset_and_init(&mut self) {
        // Mask every interrupt source (Phase 1 is polled).
        self.reg_write(REG_IMC, 0xFFFF_FFFF);

        // Software reset; RST is self-clearing.
        let ctrl = self.reg_read(REG_CTRL);
        self.reg_write(REG_CTRL, ctrl | CTRL_RST);
        for _ in 0..1_000_000 {
            if self.reg_read(REG_CTRL) & CTRL_RST == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Re-mask interrupts (reset re-enables some) and drain any pending causes.
        self.reg_write(REG_IMC, 0xFFFF_FFFF);
        let _ = self.reg_read(REG_ICR);

        // Bring the link up; clear reset/loopback-ish control bits.
        let ctrl = self.reg_read(REG_CTRL);
        self.reg_write(
            REG_CTRL,
            (ctrl | CTRL_SLU | CTRL_ASDE) & !(CTRL_LRST | CTRL_PHY_RST | CTRL_ILOS),
        );

        // Clear the multicast table so stale entries can't reject frames.
        for i in 0..128u32 {
            self.reg_write(REG_MTA + i * 4, 0);
        }

        self.mac = self.read_mac();

        // Program the RX ring registers.
        self.alloc_rx();
        let ring_phys = self.rx_ring as u64;
        self.reg_write(REG_RDBAL, ring_phys as u32);
        self.reg_write(REG_RDBAH, (ring_phys >> 32) as u32);
        self.reg_write(REG_RDLEN, (NUM_RX * core::mem::size_of::<RxDesc>()) as u32);
        self.reg_write(REG_RDH, 0);
        self.reg_write(REG_RDT, (NUM_RX - 1) as u32);

        // Enable the receiver. Promiscuous (UPE|MPE) + broadcast during bring-up so
        // we observe everything on the wire; strip the Ethernet CRC.
        self.reg_write(
            REG_RCTL,
            RCTL_EN | RCTL_UPE | RCTL_MPE | RCTL_BAM | RCTL_SECRC,
        );

        // Program the TX ring and enable the transmitter.
        self.alloc_tx();
        let tring = self.tx_ring as u64;
        self.reg_write(REG_TDBAL, tring as u32);
        self.reg_write(REG_TDBAH, (tring >> 32) as u32);
        self.reg_write(REG_TDLEN, (NUM_TX * core::mem::size_of::<TxDesc>()) as u32);
        self.reg_write(REG_TDH, 0);
        self.reg_write(REG_TDT, 0);
        self.reg_write(REG_TIPG, TIPG_DEFAULT);
        // EN | PSP | CT=0x0F | COLD=0x40 (full-duplex collision distance).
        self.reg_write(
            REG_TCTL,
            TCTL_EN | TCTL_PSP | (0x0F << TCTL_CT_SHIFT) | (0x40 << TCTL_COLD_SHIFT),
        );
    }

    /// Transmit a single Ethernet frame. Copies it into the next TX buffer, posts a
    /// descriptor (EOP|IFCS|RS), bumps the tail, and waits for the descriptor-done bit.
    fn transmit(&mut self, frame: &[u8]) {
        let i = self.tx_cur;
        let len = frame.len().min(TX_BUF_SIZE);
        let buf = unsafe { self.tx_buffers.add(i * TX_BUF_SIZE) };
        unsafe { core::ptr::copy_nonoverlapping(frame.as_ptr(), buf, len) };

        let desc = TxDesc {
            addr: (self.tx_buffers as u64) + (i * TX_BUF_SIZE) as u64,
            length: len as u16,
            cso: 0,
            cmd: TX_CMD_EOP | TX_CMD_IFCS | TX_CMD_RS,
            status: 0,
            css: 0,
            special: 0,
        };
        unsafe { write_volatile(self.tx_ring.add(i), desc) };

        self.tx_cur = (self.tx_cur + 1) % NUM_TX;
        self.reg_write(REG_TDT, self.tx_cur as u32);

        // Wait for the controller to mark the descriptor done (bounded). On real
        // hardware a stalled link could leave DD clear; surface that rather than
        // silently counting a frame that never went out.
        let mut done = false;
        for _ in 0..1_000_000 {
            let d = unsafe { read_volatile(self.tx_ring.add(i)) };
            if d.status & TX_STATUS_DD != 0 {
                done = true;
                break;
            }
            core::hint::spin_loop();
        }
        if done {
            self.tx_count += 1;
        } else {
            serial_println!("[e1000] TX timeout: descriptor {} never completed", i);
        }
    }

    /// Broadcast an ARP request ("who-has `target_ip`"). Exercises the TX path and,
    /// for a target the peer answers (e.g. the slirp gateway), provokes an inbound reply.
    fn send_arp_request(&mut self, target_ip: [u8; 4]) {
        let mut arp = [0u8; 28];
        let mut frame = [0u8; 64];
        if net::arp::build_request(&mut arp, self.mac, OUR_IP, target_ip).is_none() {
            return;
        }
        if let Some(len) = net::ethernet::write_frame(
            &mut frame,
            [0xFF; 6], // broadcast
            self.mac,
            net::ethernet::EtherType::Arp.as_u16(),
            &arp,
        ) {
            self.transmit(&frame[..len]);
            serial_println!(
                "[e1000] TX ARP request who-has {}.{}.{}.{} (len {})",
                target_ip[0], target_ip[1], target_ip[2], target_ip[3], len
            );
        }
    }

    fn link_up(&self) -> bool {
        self.reg_read(REG_STATUS) & STATUS_LU != 0
    }

    /// Unmask the receiver-timer interrupt (RXT0) so received packets raise the MSI.
    fn enable_rx_interrupt(&self) {
        let _ = self.reg_read(REG_ICR); // clear any stale causes first
        self.reg_write(REG_IMS, IMS_RXT0);
    }

    /// Drain all completed RX descriptors, handing each frame to `net::ingress`.
    /// Recycles descriptors back to hardware by advancing RDT, following the
    /// standard 8254x head/tail protocol.
    pub fn poll(&mut self) {
        loop {
            let desc = unsafe { read_volatile(self.rx_ring.add(self.rx_cur)) };
            if desc.status & RX_STATUS_DD == 0 {
                break; // hardware hasn't filled this slot yet
            }
            // `length` is written by the device via DMA; clamp to the buffer size so a
            // misbehaving NIC can never make us construct an out-of-bounds slice.
            let len = (desc.length as usize).min(RX_BUF_SIZE);
            let frame = unsafe {
                core::slice::from_raw_parts(self.rx_buffers.add(self.rx_cur * RX_BUF_SIZE), len)
            };

            self.rx_count += 1;
            if let Some(eth) = EthernetFrame::new(frame) {
                serial_println!(
                    "[e1000] RX #{} len={} src={} dst={} type={:?} irqs={}",
                    self.rx_count,
                    len,
                    fmt_mac(&eth.source_mac()),
                    fmt_mac(&eth.destination_mac()),
                    eth.ethertype(),
                    IRQ_COUNT.load(Ordering::Relaxed)
                );
            } else {
                serial_println!("[e1000] RX #{} len={} (runt/unparseable)", self.rx_count, len);
            }

            // Route the frame up the stack; ingress writes any reply (e.g. an ARP
            // reply) into tx_scratch and returns its length.
            let mut tx_scratch = [0u8; 1518];
            let reply = net::ingress(frame, &self.arp_state, &mut tx_scratch);

            // Recycle: clear DD and hand the descriptor back via RDT.
            let mut d = desc;
            d.status = 0;
            unsafe { write_volatile(self.rx_ring.add(self.rx_cur), d) };
            let old = self.rx_cur;
            self.rx_cur = (self.rx_cur + 1) % NUM_RX;
            self.reg_write(REG_RDT, old as u32);

            // Transmit the reply (if any) once the RX descriptor is back in hardware's hands.
            if let Some(n) = reply {
                self.transmit(&tx_scratch[..n]);
                serial_println!("[e1000] TX reply len={}", n);
            }
        }
    }

    fn snapshot(&self) -> NetInfo {
        NetInfo {
            mac: self.mac,
            link_up: self.link_up(),
            rx_count: self.rx_count,
            tx_count: self.tx_count,
            irq_count: IRQ_COUNT.load(Ordering::Relaxed),
            mmio_base: self.mmio_base,
        }
    }
}

/// Read-only snapshot of the NIC state for the shell.
#[derive(Clone, Copy)]
pub struct NetInfo {
    pub mac: [u8; 6],
    pub link_up: bool,
    pub rx_count: u64,
    pub tx_count: u64,
    pub irq_count: u64,
    pub mmio_base: usize,
}

/// The one registered network device (populated by [`init`]).
pub static NET_DEVICE: Mutex<Option<E1000>> = Mutex::new(None);

/// MMIO base of the NIC, published for the lock-free MSI handler (which must not take the
/// NET_DEVICE lock). Set during [`init`]; 0 means no NIC / interrupts not wired.
static MMIO_BASE: AtomicUsize = AtomicUsize::new(0);

/// Count of NIC interrupts taken (RX MSI). Proves interrupt delivery; bumped from the handler.
pub static IRQ_COUNT: AtomicU64 = AtomicU64::new(0);

/// Lock-free MSI acknowledgement, called from the IDT vector 0x41 handler. Reads ICR (which
/// clears the e1000e's interrupt causes so it can raise again) and bumps the IRQ counter. Must
/// not touch NET_DEVICE — frame processing happens in the polled `service_net`.
pub fn interrupt_ack() {
    let base = MMIO_BASE.load(Ordering::Acquire);
    if base != 0 {
        unsafe {
            let _ = read_volatile((base + REG_ICR as usize) as *const u32);
        }
        IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

/// Probe + bring up an Intel 8254x NIC at the given PCI address. Called from the
/// (x86_64) PCI init after the BAR has been located.
pub fn init(bus: u8, slot: u8, func: u8) {
    PciScanner::enable_bus_master(bus, slot, func);
    let bar = PciScanner::get_bar_address(bus, slot, func);
    serial_println!("[e1000] BAR0 = {:#x}; enabling bus master + bringing up RX/TX", bar);

    let mut nic = E1000 {
        mmio_base: bar as usize,
        mac: [0; 6],
        rx_ring: core::ptr::null_mut(),
        rx_buffers: core::ptr::null_mut(),
        rx_cur: 0,
        rx_count: 0,
        tx_ring: core::ptr::null_mut(),
        tx_buffers: core::ptr::null_mut(),
        tx_cur: 0,
        tx_count: 0,
        arp_state: ArpStateMachine::new(OUR_IP, [0; 6]),
    };
    nic.reset_and_init();
    // Re-arm the ARP state machine now that the MAC is known.
    nic.arp_state = ArpStateMachine::new(OUR_IP, nic.mac);

    serial_println!(
        "[e1000] up: MAC={} link={} ip={}.{}.{}.{} (RX {} / TX {} desc, promiscuous)",
        fmt_mac(&nic.mac),
        if nic.link_up() { "UP" } else { "DOWN" },
        OUR_IP[0], OUR_IP[1], OUR_IP[2], OUR_IP[3],
        NUM_RX, NUM_TX
    );

    // Probe the gateway: exercises TX end-to-end and provokes an inbound ARP reply
    // (proving RX) since slirp answers ARP for its own gateway address.
    nic.send_arp_request(GATEWAY_IP);

    // Publish the MMIO base so the lock-free MSI handler can read/clear ICR. Interrupts
    // themselves are wired by `enable_interrupts`, called from the arch-specific PCI init.
    MMIO_BASE.store(nic.mmio_base, Ordering::Release);
    *NET_DEVICE.lock() = Some(nic);
}

/// Wire the NIC's RX interrupt: enable MSI (delivering `vector` to `msg_addr` — the interrupt
/// controller) then unmask the receiver interrupt. Arch-neutral: the caller supplies the
/// arch-specific message address (on x86, the local APIC) and IDT vector, so this module never
/// references the APIC/IDT directly. Returns true if MSI was enabled (else the NIC stays polled).
pub fn enable_interrupts(bus: u8, slot: u8, func: u8, msg_addr: u32, vector: u32) -> bool {
    let ok = PciScanner::enable_msi(bus, slot, func, msg_addr, vector);
    if ok {
        if let Some(nic) = NET_DEVICE.lock().as_ref() {
            nic.enable_rx_interrupt();
        }
    }
    serial_println!(
        "[e1000] RX interrupt (MSI vector {:#x}): {}",
        vector,
        if ok { "enabled" } else { "unavailable (polled only)" }
    );
    ok
}

/// Main-loop hook: poll the NIC for received frames. No-op if no NIC is present
/// (e.g. on aarch64 / when no e1000 was found).
pub fn service_net() {
    if let Some(nic) = NET_DEVICE.lock().as_mut() {
        nic.poll();
    }
}

/// Snapshot of the current NIC state, if any (used by the `netinfo` shell command).
pub fn info() -> Option<NetInfo> {
    NET_DEVICE.lock().as_ref().map(|n| n.snapshot())
}

/// Format a MAC address as `xx:xx:xx:xx:xx:xx`.
pub fn fmt_mac(mac: &[u8; 6]) -> alloc::string::String {
    alloc::format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}
