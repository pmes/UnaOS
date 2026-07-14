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
use net::arp::{ArpCache, ArpStateMachine};
use net::ethernet::{EtherType, EthernetFrame};
use net::ipv4::Ipv4Header;

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
/// DHCP transaction ID (fixed — a single client doing a one-shot DISCOVER). ASCII "UNAO".
const DHCP_XID: u32 = 0x554E_414F;
/// TCP port the built-in echo listener accepts connections on (RFC 862 echo service).
const TCP_ECHO_PORT: u16 = 7;

/// ICMP identifier stamped on every echo request we originate (the boot self-test and the
/// `ping` shell command). ASCII "UN". Lets us recognise our own replies.
const PING_IDENT: u16 = 0x554E;
/// Payload carried in the echo requests we originate.
const PING_PAYLOAD: &[u8] = b"unaos-ping";

/// Boot connectivity self-test cadence: act every Nth `service_net` call. The main loop
/// runs at roughly the APIC-timer rate, so this only needs to be coarse — it spreads probes
/// out over time so a host responder (slirp gateway, or the socket injector once it
/// connects) has a chance to answer between them.
const SELFTEST_STRIDE: u32 = 16;
/// Give up resolving the gateway after this many ARP attempts (only reached on a truly dead
/// link — a real responder answers the first request). Bounds dead-network broadcast noise.
const SELFTEST_ARP_TRIES: u16 = 150;
/// After the gateway is resolved, send at most this many echo requests before concluding the
/// link has no ICMP responder (e.g. some slirp configs) and disarming the self-test.
const SELFTEST_PING_TRIES: u16 = 24;

/// Iterations of `poll()` to pump while a blocking `resolve`/`ping` waits for a reply.
/// Iteration-bounded rather than wall-clock-bounded to keep this driver arch-neutral (no
/// clock dependency); a reply on a local link lands long before this is exhausted, and the
/// bound only caps how long an unreachable target stalls the shell.
const PUMP_ITERS: u32 = 2_000_000;
/// ARP requests / per-sequence echo requests a blocking `resolve`/`ping` will attempt.
const RESOLVE_TRIES: u32 = 3;

/// Iterations of `poll()` to pump while a blocking `connect` drives the handshake/echo/close.
/// Larger than PUMP_ITERS because a connection is several round-trips; bounds how long an
/// unanswered SYN (no server, no RST) stalls the caller.
const CONNECT_PUMP_ITERS: u32 = 8_000_000;
/// First ephemeral local port for outbound connections (incremented per connect, wrapped back
/// into the ephemeral range).
const TCP_LOCAL_PORT_BASE: u16 = 49152;
/// Gateway TCP port the boot self-test connects to (the socket injector serves an echo here).
const TCP_SELFTEST_PORT: u16 = 7777;
/// One-shot payload the connect self-test / `connect` (no message) sends after the handshake.
const TCP_PROBE_PAYLOAD: &[u8] = b"unaos-tcp";
/// Gateway TCP port the boot self-test does a STREAMING fetch from (the socket injector serves a
/// multi-segment response then closes here) — exercises the outbound client's linger receive.
const TCP_STREAM_PORT: u16 = 7778;
/// Request the streaming self-test sends; the injector ignores its content and replies with a
/// fixed multi-segment response, so the exact bytes don't matter.
const STREAM_PROBE_PAYLOAD: &[u8] = b"GET /unaos\r\n";

/// First ephemeral local port for outbound UDP datagrams (incremented per send).
const UDP_LOCAL_PORT_BASE: u16 = 49152;
/// Largest outbound UDP payload `udp_send` will transmit.
const UDP_TX_CAP: usize = 256;
/// Gateway UDP port the boot self-test sends to (the socket injector serves an echo here).
const UDP_SELFTEST_PORT: u16 = 9998;
/// Payload the UDP self-test / `udpsend` (no message) emits.
const UDP_PROBE_PAYLOAD: &[u8] = b"unaos-udp";

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
    dhcp: net::dhcp::DhcpClient,
    tcp: net::tcp::TcpListener,
    /// The single in-flight outbound TCP connection (active open), if any. Driven by `poll()`
    /// and the blocking `connect`; `None` when idle.
    tcp_client: Option<net::tcp::TcpClient>,
    /// Next ephemeral local port + initial sequence ramp for outbound connections.
    tcp_local_port: u16,
    tcp_client_isn: u32,
    /// While an outbound UDP exchange is in flight, the `(peer IP, our ephemeral port)` we
    /// expect the reply from/on; `poll()` routes a matching datagram into `udp_rx` instead of
    /// the inbound UDP-echo responder (which would otherwise bounce our reply back — a loop).
    /// Source-scoped (peer IP), like the ICMP `pong` slot.
    udp_client: Option<([u8; 4], u16)>,
    /// Payload length of the captured reply (set by the capture path; consumed by `udp_send`).
    udp_rx: Option<usize>,
    udp_local_port: u16,
    /// IP→MAC mappings learned from inbound ARP (and IP frames) — the resolver for outbound.
    arp_cache: ArpCache,
    /// `(source IP, identifier, sequence)` of the most recent ICMP echo reply addressed to
    /// us. Set by [`observe`](Self::observe); consumed by the blocking `ping` and the boot
    /// self-test, both of which match on the source so a reply from one host can never be
    /// counted against a probe to a different host (or the gateway).
    pong: Option<([u8; 4], u16, u16)>,
    /// Boot connectivity self-test state, driven from `service_net` (gateway ARP + ping).
    selftest_armed: bool,
    selftest_counter: u32,
    selftest_arp_tries: u16,
    selftest_ping_tries: u16,
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
    /// Uses our *current* IP as the sender (which DHCP may have changed from the static one).
    fn send_arp_request(&mut self, target_ip: [u8; 4]) {
        let our_ip = self.arp_state.our_ip();
        let mut arp = [0u8; 28];
        let mut frame = [0u8; 64];
        if net::arp::build_request(&mut arp, self.mac, our_ip, target_ip).is_none() {
            return;
        }
        if let Some(len) = net::ethernet::write_frame(
            &mut frame,
            [0xFF; 6], // broadcast
            self.mac,
            EtherType::Arp.as_u16(),
            &arp,
        ) {
            self.transmit(&frame[..len]);
            serial_println!(
                "[e1000] TX ARP request who-has {}.{}.{}.{} (len {})",
                target_ip[0], target_ip[1], target_ip[2], target_ip[3], len
            );
        }
    }

    /// Build + transmit an ICMP echo request to `dst_ip` (its already-resolved `dst_mac`).
    /// Lays the frame out bottom-up — Ethernet[0..14] | IPv4[14..34] | ICMP[34..] — like the
    /// responder path in `net::ingress`. Returns false if serialization failed.
    fn send_echo_request(
        &mut self,
        dst_ip: [u8; 4],
        dst_mac: [u8; 6],
        ident: u16,
        seq: u16,
        data: &[u8],
    ) -> bool {
        let our_ip = self.arp_state.our_ip();
        let our_mac = self.mac;
        let mut frame = [0u8; 128];
        let icmp_len = match net::icmp::write_echo_request(&mut frame[34..], ident, seq, data) {
            Some(n) => n,
            None => return false,
        };
        if net::ipv4::write_header(&mut frame[14..34], our_ip, dst_ip, net::ipv4::PROTO_ICMP, icmp_len)
            .is_none()
        {
            return false;
        }
        if net::ethernet::write_header(&mut frame[0..14], dst_mac, our_mac, EtherType::Ipv4.as_u16())
            .is_none()
        {
            return false;
        }
        let total = 14 + 20 + icmp_len;
        self.transmit(&frame[..total]);
        true
    }

    /// Resolve `ip` to a MAC, consulting the ARP cache first; otherwise broadcast ARP
    /// requests and pump RX until a reply lands (bounded). Blocking — for the shell.
    fn resolve(&mut self, ip: [u8; 4]) -> Option<[u8; 6]> {
        if let Some(mac) = self.arp_cache.lookup(ip) {
            return Some(mac);
        }
        for _ in 0..RESOLVE_TRIES {
            self.send_arp_request(ip);
            for _ in 0..PUMP_ITERS {
                self.poll();
                if let Some(mac) = self.arp_cache.lookup(ip) {
                    return Some(mac);
                }
                core::hint::spin_loop();
            }
        }
        None
    }

    /// Send `count` ICMP echo requests to `ip`, pumping RX for each reply (bounded). Resolves
    /// the destination MAC via ARP first. Blocking — for the `ping` shell command.
    fn ping(&mut self, ip: [u8; 4], count: u16) -> PingOutcome {
        let mac = match self.resolve(ip) {
            Some(m) => m,
            None => return PingOutcome { resolved: false, mac: None, sent: 0, received: 0 },
        };
        let mut sent = 0u16;
        let mut received = 0u16;
        for seq in 1..=count {
            self.pong = None;
            if !self.send_echo_request(ip, mac, PING_IDENT, seq, PING_PAYLOAD) {
                break;
            }
            sent += 1;
            for _ in 0..PUMP_ITERS {
                self.poll();
                // Count only a reply from the host we pinged (source-scoped).
                if self.pong == Some((ip, PING_IDENT, seq)) {
                    received += 1;
                    break;
                }
                core::hint::spin_loop();
            }
        }
        PingOutcome { resolved: true, mac: Some(mac), sent, received }
    }

    /// Route a received frame to the in-flight outbound connection, if it matches. Reads our
    /// IP/MAC first so the `&mut self.tcp_client` borrow doesn't conflict.
    fn client_handle(&mut self, frame: &[u8], out: &mut [u8]) -> Option<usize> {
        let our_ip = self.arp_state.our_ip();
        let our_mac = self.mac;
        self.tcp_client.as_mut()?.handle(frame, our_ip, our_mac, out)
    }

    /// Blocking active-open TCP connect for the shell / boot self-test: resolve `ip`, send a
    /// SYN, then pump RX until the connection settles (handshake + optional `payload` exchange +
    /// close) or the bounded budget is exhausted. With `linger`, the client keeps receiving until
    /// the peer closes (a streaming fetch) instead of closing after the first response. Returns
    /// the outcome summary and the bytes of peer response captured (bounded by `CLIENT_RX_CAP`).
    fn connect_inner(&mut self, ip: [u8; 4], port: u16, payload: &[u8], linger: bool) -> (ConnectOutcome, alloc::vec::Vec<u8>) {
        let fail = |resolved| (ConnectOutcome { resolved, established: false, rx_len: 0, closed: false }, alloc::vec::Vec::new());
        let mac = match self.resolve(ip) {
            Some(m) => m,
            None => return fail(false),
        };

        // Pick an ephemeral local port and a fresh ISN per connection.
        let local_port = self.tcp_local_port;
        self.tcp_local_port = self.tcp_local_port.checked_add(1).unwrap_or(TCP_LOCAL_PORT_BASE);
        if self.tcp_local_port < TCP_LOCAL_PORT_BASE {
            self.tcp_local_port = TCP_LOCAL_PORT_BASE;
        }
        self.tcp_client_isn = self.tcp_client_isn.wrapping_add(0x0001_0000);
        let isn = self.tcp_client_isn;

        let our_ip = self.arp_state.our_ip();
        let our_mac = self.mac;
        let mut client = net::tcp::TcpClient::new(local_port, ip, mac, port, isn, payload);
        if linger {
            client = client.streaming();
        }
        let mut syn = [0u8; 64];
        let n = match client.open(&mut syn, our_ip, our_mac) {
            Some(n) => n,
            None => return fail(true),
        };
        self.tcp_client = Some(client);
        self.transmit(&syn[..n]);

        // Pump RX; poll() routes matching segments to the client (which emits the next segment).
        for _ in 0..CONNECT_PUMP_ITERS {
            self.poll();
            if self.tcp_client.as_ref().map(|c| c.is_done()).unwrap_or(true) {
                break;
            }
            core::hint::spin_loop();
        }

        let result = match self.tcp_client.as_ref() {
            Some(c) => (
                ConnectOutcome {
                    resolved: true,
                    established: c.established(),
                    rx_len: c.rx_data().len(),
                    closed: c.state() == net::tcp::ClientState::Done,
                },
                c.rx_data().to_vec(),
            ),
            None => fail(true),
        };
        self.tcp_client = None; // free the slot for the next connection
        result
    }

    /// One-shot connect (resolve, handshake, optional `payload` echo, active close).
    fn connect(&mut self, ip: [u8; 4], port: u16, payload: &[u8]) -> ConnectOutcome {
        self.connect_inner(ip, port, payload, false).0
    }

    /// Streaming fetch: send `payload` as a request, then read the full response until the peer
    /// closes (or the receive buffer fills). Returns the outcome and the captured response bytes.
    fn fetch(&mut self, ip: [u8; 4], port: u16, payload: &[u8]) -> (ConnectOutcome, alloc::vec::Vec<u8>) {
        self.connect_inner(ip, port, payload, true)
    }

    /// Capture an inbound UDP datagram that is the reply to an in-flight outbound exchange
    /// (destined to our IP on the active `udp_client_port`). Returns true when consumed, so
    /// `poll()` does NOT also hand it to the inbound UDP-echo responder (which would bounce
    /// our reply back to the peer and, against an echo server, loop).
    fn udp_client_capture(&mut self, frame: &[u8]) -> bool {
        let (peer, port) = match self.udp_client {
            Some(x) => x,
            None => return false,
        };
        let eth = match EthernetFrame::new(frame) {
            Some(e) => e,
            None => return false,
        };
        if eth.ethertype() != EtherType::Ipv4 {
            return false;
        }
        let ip = match Ipv4Header::new(eth.payload()) {
            Some(h) => h,
            None => return false,
        };
        if !ip.verify_checksum()
            || ip.destination_ip() != self.arp_state.our_ip()
            || ip.source_ip() != peer
            || ip.protocol() != net::udp::PROTO_UDP
        {
            return false;
        }
        let dg = match net::udp::UdpDatagram::new(ip.payload()) {
            Some(d) => d,
            None => return false,
        };
        if dg.dest_port() != port {
            return false;
        }
        self.udp_rx = Some(dg.payload().len());
        true
    }

    /// Blocking outbound UDP send for the shell / boot self-test: resolve `ip`, send one
    /// datagram from an ephemeral port, then pump RX briefly for a reply (UDP is best-effort,
    /// so no reply is not an error). Returns an outcome summary.
    fn udp_send(&mut self, ip: [u8; 4], port: u16, payload: &[u8]) -> UdpOutcome {
        let mac = match self.resolve(ip) {
            Some(m) => m,
            None => return UdpOutcome { resolved: false, sent: false, replied: false, rx_len: 0 },
        };
        let local = self.udp_local_port;
        self.udp_local_port = self.udp_local_port.checked_add(1).unwrap_or(UDP_LOCAL_PORT_BASE);
        if self.udp_local_port < UDP_LOCAL_PORT_BASE {
            self.udp_local_port = UDP_LOCAL_PORT_BASE;
        }

        let our_ip = self.arp_state.our_ip();
        let our_mac = self.mac;
        let pl = &payload[..payload.len().min(UDP_TX_CAP)];
        let mut frame = [0u8; 14 + 20 + 8 + UDP_TX_CAP];
        let udp_len = match net::udp::write_datagram(&mut frame[34..], local, port, our_ip, ip, pl) {
            Some(n) => n,
            None => return UdpOutcome { resolved: true, sent: false, replied: false, rx_len: 0 },
        };
        if net::ipv4::write_header(&mut frame[14..34], our_ip, ip, net::udp::PROTO_UDP, udp_len).is_none() {
            return UdpOutcome { resolved: true, sent: false, replied: false, rx_len: 0 };
        }
        if net::ethernet::write_header(&mut frame[0..14], mac, our_mac, EtherType::Ipv4.as_u16()).is_none() {
            return UdpOutcome { resolved: true, sent: false, replied: false, rx_len: 0 };
        }
        let total = 14 + 20 + udp_len;

        self.udp_client = Some((ip, local));
        self.udp_rx = None;
        self.transmit(&frame[..total]);

        for _ in 0..PUMP_ITERS {
            self.poll();
            if self.udp_rx.is_some() {
                break;
            }
            core::hint::spin_loop();
        }

        let (replied, rx_len) = match self.udp_rx {
            Some(n) => (true, n),
            None => (false, 0),
        };
        self.udp_client = None;
        UdpOutcome { resolved: true, sent: true, replied, rx_len }
    }

    /// One step of the boot connectivity self-test, called from `service_net` after `poll`.
    /// First ARP-resolves the gateway, then sends ICMP echo requests to it, until either a
    /// reply arrives (success) or the per-phase attempt budget is exhausted. Non-blocking:
    /// a little state advances each call, paced by `SELFTEST_STRIDE`.
    fn selftest_tick(&mut self) {
        if !self.selftest_armed {
            return;
        }
        self.selftest_counter = self.selftest_counter.wrapping_add(1);
        if self.selftest_counter % SELFTEST_STRIDE != 0 {
            return;
        }

        // Success: `observe` saw an echo reply from the gateway stamped with our identifier.
        if let Some((src, id, seq)) = self.pong {
            if src == GATEWAY_IP && id == PING_IDENT {
                serial_println!(
                    "[selftest] gateway {} reachable — ICMP echo reply seq={} (outbound path OK)",
                    fmt_ip(&GATEWAY_IP), seq
                );
                // Also exercise the outbound TCP path: connect to the gateway's echo port,
                // send a probe, read the echo, and close. Graceful if nothing listens there.
                let o = self.connect(GATEWAY_IP, TCP_SELFTEST_PORT, TCP_PROBE_PAYLOAD);
                if o.established {
                    serial_println!(
                        "[selftest] TCP connect to {}:{} OK — established, {} bytes echoed, closed={}",
                        fmt_ip(&GATEWAY_IP), TCP_SELFTEST_PORT, o.rx_len, o.closed
                    );
                    // A server is present, so also exercise the streaming/linger receive path:
                    // fetch a multi-segment response and read it all until the peer closes.
                    let (sf, _body) = self.fetch(GATEWAY_IP, TCP_STREAM_PORT, STREAM_PROBE_PAYLOAD);
                    if sf.established {
                        serial_println!(
                            "[selftest] TCP stream from {}:{} OK — {} bytes received, closed={}",
                            fmt_ip(&GATEWAY_IP), TCP_STREAM_PORT, sf.rx_len, sf.closed
                        );
                    } else {
                        serial_println!(
                            "[selftest] TCP stream to {}:{} — no streaming server",
                            fmt_ip(&GATEWAY_IP), TCP_STREAM_PORT
                        );
                    }
                } else {
                    serial_println!(
                        "[selftest] TCP connect to {}:{} — no server (ok if the backend has none)",
                        fmt_ip(&GATEWAY_IP), TCP_SELFTEST_PORT
                    );
                }
                // And the outbound UDP path: send a datagram to the gateway echo port, await echo.
                let u = self.udp_send(GATEWAY_IP, UDP_SELFTEST_PORT, UDP_PROBE_PAYLOAD);
                if u.replied {
                    serial_println!(
                        "[selftest] UDP echo from {}:{} OK — {} bytes",
                        fmt_ip(&GATEWAY_IP), UDP_SELFTEST_PORT, u.rx_len
                    );
                } else {
                    serial_println!(
                        "[selftest] UDP send to {}:{} — no echo (ok if the backend has none)",
                        fmt_ip(&GATEWAY_IP), UDP_SELFTEST_PORT
                    );
                }
                self.selftest_armed = false;
                return;
            }
        }

        match self.arp_cache.lookup(GATEWAY_IP) {
            None => {
                // Phase 1: resolve the gateway's MAC.
                self.send_arp_request(GATEWAY_IP);
                self.selftest_arp_tries += 1;
                if self.selftest_arp_tries >= SELFTEST_ARP_TRIES {
                    serial_println!(
                        "[selftest] gateway {} did not answer ARP — link self-test aborted",
                        fmt_ip(&GATEWAY_IP)
                    );
                    self.selftest_armed = false;
                }
            }
            Some(mac) => {
                // Phase 2: ping the resolved gateway.
                self.selftest_ping_tries += 1;
                self.send_echo_request(GATEWAY_IP, mac, PING_IDENT, self.selftest_ping_tries, PING_PAYLOAD);
                if self.selftest_ping_tries >= SELFTEST_PING_TRIES {
                    serial_println!(
                        "[selftest] gateway {} is-at {} but no ICMP reply (ok if the backend has no responder)",
                        fmt_ip(&GATEWAY_IP), fmt_mac(&mac)
                    );
                    self.selftest_armed = false;
                }
            }
        }
    }

    /// Passively learn from a received frame before the responder dispatch: cache the
    /// sender's IP→MAC, and capture ICMP echo replies addressed to us (so an outbound
    /// `ping` / the boot self-test can match them). Produces no reply.
    fn observe(&mut self, frame: &[u8]) {
        let eth = match EthernetFrame::new(frame) {
            Some(e) => e,
            None => return,
        };
        match eth.ethertype() {
            EtherType::Arp => {
                if let Some((ip, mac)) = net::arp::learn(eth.payload()) {
                    self.arp_cache.insert(ip, mac);
                }
            }
            EtherType::Ipv4 => {
                let ip = match Ipv4Header::new(eth.payload()) {
                    Some(h) => h,
                    None => return,
                };
                // Require a valid header addressed to us — mirrors the responder path in
                // `net::ingress`, so we never learn from / act on a corrupt IPv4 header.
                if !ip.verify_checksum() || ip.destination_ip() != self.arp_state.our_ip() {
                    return;
                }
                if ip.protocol() == net::ipv4::PROTO_ICMP {
                    if let Some((id, seq)) = net::icmp::parse_echo_reply(ip.payload()) {
                        let src = ip.source_ip();
                        self.pong = Some((src, id, seq));
                        // Learn the responder's MAC from the IP frame too.
                        self.arp_cache.insert(src, eth.source_mac());
                        serial_println!(
                            "[icmp] echo reply id={:#06x} seq={} from {}.{}.{}.{}",
                            id, seq, src[0], src[1], src[2], src[3]
                        );
                    }
                }
            }
            _ => {}
        }
    }

    /// Service the TCP listener's retransmission timers: if a connection's outstanding segment
    /// has passed its RTO, retransmit it. Called each main-loop pass from `service_net`.
    fn tcp_tick(&mut self) {
        let now = crate::arch::ticks();
        let our_ip = self.arp_state.our_ip();
        let our_mac = self.mac;
        let mut out = [0u8; RX_BUF_SIZE];
        if let Some(n) = self.tcp.tick(now, our_ip, our_mac, &mut out) {
            self.transmit(&out[..n]);
            serial_println!("[tcp] retransmit ({} bytes)", n);
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

            // Learn IP→MAC mappings and capture ICMP echo replies addressed to us before the
            // responder dispatch — the outbound `ping` / boot self-test consume these.
            self.observe(frame);

            // DHCP client traffic (UDP 67->68) drives the lease state machine; everything
            // else goes to the responder stack (ARP / ICMP echo / UDP echo).
            // Sized to RX_BUF_SIZE so a reply (e.g. a TCP/UDP echo) of any frame we can
            // receive always fits — never a silent serialization failure.
            let mut tx_scratch = [0u8; RX_BUF_SIZE];
            let reply = if let Some(dr) = net::dhcp::parse_reply(frame) {
                let out = self.dhcp.on_reply(&dr, &mut tx_scratch);
                if out.is_some() {
                    serial_println!(
                        "[dhcp] OFFER {}.{}.{}.{} from {}.{}.{}.{} -> REQUEST",
                        dr.your_ip[0], dr.your_ip[1], dr.your_ip[2], dr.your_ip[3],
                        dr.server_ip[0], dr.server_ip[1], dr.server_ip[2], dr.server_ip[3]
                    );
                }
                // Apply a fresh lease to our IP / ARP identity.
                if self.dhcp.state == net::dhcp::DhcpState::Bound {
                    if let Some(ip) = self.dhcp.leased_ip {
                        if self.arp_state.our_ip() != ip {
                            self.arp_state = ArpStateMachine::new(ip, self.mac);
                            serial_println!("[dhcp] bound: IP {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
                        }
                    }
                }
                out
            } else if let Some(n) = self.client_handle(frame, &mut tx_scratch) {
                // Outbound connection (active open) handled it.
                Some(n)
            } else if let Some(n) =
                self.tcp.handle(frame, crate::arch::ticks(), self.arp_state.our_ip(), self.mac, &mut tx_scratch)
            {
                // TCP echo listener (stateful) handled it.
                Some(n)
            } else if self.udp_client_capture(frame) {
                // Reply to an in-flight outbound UDP send — captured, never re-echoed.
                None
            } else {
                net::ingress(frame, &self.arp_state, &mut tx_scratch)
            };

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
            tcp_conns: self.tcp.active_conns(),
        }
    }
}

// --- SOCK-1: additive raw L2 accessors for the smoltcp `Device` adapter (smolnet.rs) ---
// x86-only + feature-gated: knob-off / aarch64 builds don't compile any of this, so the driver is
// byte-identical. These expose the ring at the Ethernet-frame boundary WITHOUT the hand-rolled
// `observe`/`net::ingress` processing — smoltcp owns the stack when it's driving. Poll-driven only
// (called from the blocking `smolnet::ping`/`arp`/witness pumps), never from the MSI handler.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
impl E1000 {
    /// Pop one completed RX descriptor's raw Ethernet frame into `out` and recycle the descriptor
    /// back to hardware (clear DD, advance RDT) — the same head/tail protocol `poll()` uses, minus
    /// the responder dispatch. Returns the copied length, or `None` if the ring is empty.
    fn rx_frame_raw(&mut self, out: &mut [u8]) -> Option<usize> {
        let desc = unsafe { read_volatile(self.rx_ring.add(self.rx_cur)) };
        if desc.status & RX_STATUS_DD == 0 {
            return None;
        }
        let len = (desc.length as usize).min(RX_BUF_SIZE).min(out.len());
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.rx_buffers.add(self.rx_cur * RX_BUF_SIZE),
                out.as_mut_ptr(),
                len,
            );
        }
        self.rx_count += 1;
        let mut d = desc;
        d.status = 0;
        unsafe { write_volatile(self.rx_ring.add(self.rx_cur), d) };
        let old = self.rx_cur;
        self.rx_cur = (self.rx_cur + 1) % NUM_RX;
        self.reg_write(REG_RDT, old as u32);
        Some(len)
    }

    /// Transmit one raw Ethernet frame (smoltcp already built the full L2 frame). Thin wrapper over
    /// the existing `transmit` so smolnet shares the TX ring + `tx_count`.
    fn tx_frame_raw(&mut self, frame: &[u8]) {
        self.transmit(frame);
    }

    /// Our current IP (DHCP may have moved it off the static default).
    fn our_ip_raw(&self) -> [u8; 4] {
        self.arp_state.our_ip()
    }
}

/// SOCK-1: pull one raw L2 frame for the smoltcp Device. Short-locks `NET_DEVICE` (the token-driven
/// smolnet poll must not hold the lock across a transmit, so each ring op locks independently).
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
pub fn raw_rx(out: &mut [u8]) -> Option<usize> {
    NET_DEVICE.lock().as_mut().and_then(|n| n.rx_frame_raw(out))
}

/// SOCK-1: transmit one raw L2 frame from the smoltcp Device. Short-locks `NET_DEVICE`.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
pub fn raw_tx(frame: &[u8]) {
    if let Some(n) = NET_DEVICE.lock().as_mut() {
        n.tx_frame_raw(frame);
    }
}

/// SOCK-1: `(MAC, current IP, link-up)` for the smolnet interface config / `netinfo`. `None` if no NIC.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
pub fn hw_addr() -> Option<([u8; 6], [u8; 4], bool)> {
    NET_DEVICE.lock().as_ref().map(|n| (n.mac, n.our_ip_raw(), n.link_up()))
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
    /// Active TCP echo-listener connections right now.
    pub tcp_conns: usize,
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
        dhcp: net::dhcp::DhcpClient::new([0; 6], DHCP_XID),
        tcp: net::tcp::TcpListener::new(TCP_ECHO_PORT),
        tcp_client: None,
        tcp_local_port: TCP_LOCAL_PORT_BASE,
        tcp_client_isn: 0x00A0_0000,
        udp_client: None,
        udp_rx: None,
        udp_local_port: UDP_LOCAL_PORT_BASE,
        arp_cache: ArpCache::new(),
        pong: None,
        selftest_armed: false,
        selftest_counter: 0,
        selftest_arp_tries: 0,
        selftest_ping_tries: 0,
    };
    nic.reset_and_init();
    // Re-arm the ARP state machine + DHCP client now that the MAC is known.
    nic.arp_state = ArpStateMachine::new(OUR_IP, nic.mac);
    nic.dhcp = net::dhcp::DhcpClient::new(nic.mac, DHCP_XID);

    serial_println!(
        "[e1000] up: MAC={} link={} ip={}.{}.{}.{} (RX {} / TX {} desc, promiscuous)",
        fmt_mac(&nic.mac),
        if nic.link_up() { "UP" } else { "DOWN" },
        OUR_IP[0], OUR_IP[1], OUR_IP[2], OUR_IP[3],
        NUM_RX, NUM_TX
    );

    // Arm the boot connectivity self-test: from the main loop it ARP-resolves the gateway
    // and pings it (ICMP echo request), exercising the full *outbound* build path end-to-end
    // and provoking inbound RX. It runs a bounded number of attempts, then disarms — see
    // `selftest_tick`. (Supersedes the old one-shot gateway ARP probe.)
    nic.selftest_armed = true;

    // Kick off DHCP (best effort): broadcast a DISCOVER now; the OFFER/ACK are handled in
    // poll(), which applies the lease. If no server answers (socket / vmnet-host modes), we
    // keep the static OUR_IP so the ARP/ICMP/UDP responders still work.
    {
        let mut frame = [0u8; 400];
        if let Some(n) = nic.dhcp.discover(&mut frame) {
            nic.transmit(&frame[..n]);
            serial_println!("[dhcp] DISCOVER sent (xid {:#010x})", DHCP_XID);
        }
    }

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

/// Main-loop hook: poll the NIC for received frames, advance the boot connectivity self-test,
/// then service TCP retransmission timers. No-op if no NIC is present (e.g. on aarch64 / when
/// no e1000 was found).
pub fn service_net() {
    if let Some(nic) = NET_DEVICE.lock().as_mut() {
        nic.poll();
        nic.selftest_tick();
        nic.tcp_tick();
    }
    // SOCK-1 (knob-on): the smoltcp boot connectivity witness. Runs AFTER the NET_DEVICE guard
    // above is dropped — its blocking ICMP pump short-locks NET_DEVICE per ring op, so holding the
    // lock here would deadlock (spin::Mutex is not reentrant). One-shot; no-op knob-off / no NIC.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    crate::smolnet::witness_tick();
    // SOCK-2 (knob-on): the smoltcp persistent-socket UDP round-trip witness. Same one-shot,
    // post-guard discipline as SOCK-1's — its pump short-locks NET_DEVICE per ring op.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    crate::smolnet::witness_tick2();
    // SOCK-3 (knob-on): the smoltcp persistent-socket TCP round-trip witness. Same one-shot,
    // post-guard discipline — its connect/recv pumps short-lock NET_DEVICE per ring op.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    crate::smolnet::witness_tick3();
    // SOCK-6 (knob-on): the smoltcp TCP SERVER witness — STATEFUL (arms a listener, then polls accept
    // across passes). Awaits an inbound connect from scripts/net-inject.py under UNAOS_NET=socket;
    // hermetic slirp never connects in, so it prints an honest PENDING note and keeps listening cheaply.
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    crate::smolnet::witness_tick6();
}

/// Outcome of a blocking [`ping`] (rendered by the `ping` shell command).
#[derive(Clone, Copy)]
pub struct PingOutcome {
    /// Whether the destination MAC was resolved (an unreachable host fails here).
    pub resolved: bool,
    /// The resolved peer MAC, if any.
    pub mac: Option<[u8; 6]>,
    /// Echo requests sent and replies received.
    pub sent: u16,
    pub received: u16,
}

/// Blocking ICMP ping from the shell: resolve `ip` then send `count` echo requests,
/// returning a summary. `None` if no NIC is present.
pub fn ping(ip: [u8; 4], count: u16) -> Option<PingOutcome> {
    NET_DEVICE.lock().as_mut().map(|nic| nic.ping(ip, count))
}

/// Blocking ARP resolve from the shell. Returns the peer MAC, or `None` if unresolved /
/// no NIC is present.
pub fn arp_resolve(ip: [u8; 4]) -> Option<[u8; 6]> {
    NET_DEVICE.lock().as_mut().and_then(|nic| nic.resolve(ip))
}

/// Outcome of a blocking [`connect`] (rendered by the `connect` shell command).
#[derive(Clone, Copy)]
pub struct ConnectOutcome {
    /// Whether the destination MAC resolved (an unreachable host fails here).
    pub resolved: bool,
    /// Whether the TCP handshake completed.
    pub established: bool,
    /// Bytes of peer response received.
    pub rx_len: usize,
    /// Whether the connection closed cleanly (FIN exchange completed).
    pub closed: bool,
}

/// Blocking outbound TCP connect from the shell: resolve `ip`, open to `port`, send `payload`
/// (empty = connect then immediately close), read the response, and close. `None` if no NIC.
pub fn connect(ip: [u8; 4], port: u16, payload: &[u8]) -> Option<ConnectOutcome> {
    NET_DEVICE.lock().as_mut().map(|nic| nic.connect(ip, port, payload))
}

/// Blocking streaming fetch from the shell (e.g. an HTTP GET): connect, send `payload` as the
/// request, then read the whole response until the peer closes. Returns the outcome plus the
/// captured response bytes (bounded by `CLIENT_RX_CAP`). `None` if no NIC is present.
pub fn fetch(ip: [u8; 4], port: u16, payload: &[u8]) -> Option<(ConnectOutcome, alloc::vec::Vec<u8>)> {
    NET_DEVICE.lock().as_mut().map(|nic| nic.fetch(ip, port, payload))
}

/// Outcome of a blocking [`udp_send`] (rendered by the `udpsend` shell command).
#[derive(Clone, Copy)]
pub struct UdpOutcome {
    /// Whether the destination MAC resolved (an unreachable host fails here).
    pub resolved: bool,
    /// Whether the datagram was transmitted.
    pub sent: bool,
    /// Whether a reply datagram arrived (UDP is best-effort — no reply is not an error).
    pub replied: bool,
    /// Bytes of reply payload received.
    pub rx_len: usize,
}

/// Blocking outbound UDP send from the shell: resolve `ip`, send one datagram to `port`, and
/// briefly await a reply. `None` if no NIC is present.
pub fn udp_send(ip: [u8; 4], port: u16, payload: &[u8]) -> Option<UdpOutcome> {
    NET_DEVICE.lock().as_mut().map(|nic| nic.udp_send(ip, port, payload))
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

/// Format an IPv4 address as `a.b.c.d`.
pub fn fmt_ip(ip: &[u8; 4]) -> alloc::string::String {
    alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}
