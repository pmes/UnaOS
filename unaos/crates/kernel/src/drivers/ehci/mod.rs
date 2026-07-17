// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! EHCI-3 — a minimal, polling-first EHCI HID driver (UNAOS_EHCIHID=1, feature `ehcihid`).
//!
//! Purpose: the 2012 rMBP's internal keyboard + trackpad are real USB devices asleep behind the
//! Panther Point EHCI companions on NON-switchable ports (EHCI-2 metal census: censusB = one
//! connected device per function, J-state, EHCI-owned). PORTSW routes only the *switchable*
//! shared ports to xHCI, so the two stacks own disjoint ports by hardware — this driver runs
//! permanently alongside the xHCI driver and never touches an xHCI register.
//!
//! Shape (design doc EHCI3-DESIGN): builds on the shared EHCI-2 wake (`ehci_scout::wake_run`/
//! `wake_route` — one wake path, not two), adds the transaction-initiating layer: port reset, an
//! async schedule with ONE reusable control QH for synchronous enumeration, a periodic frame
//! list with one interrupt QH per HID endpoint, and a main-loop `service_ehci_hid()` poll that
//! feeds boot reports through the same decode idiom (and scancode table) as the xHCI HID path.
//! No interrupts: no USBINTR write, no IDT vector, no MSI.
//!
//! Topology fork (the M1 evidence gate): the first GET_DESCRIPTOR on the root-port device
//! decides on a serial line whether it is the integrated Rate-Matching Hub (bDeviceClass 0x09 →
//! Topology A: hub walk + split transactions to the FS/LS children through the RMH's TT) or a
//! direct HID device (Topology B: no splits). The QH builder parameterizes hub-addr/port/
//! S-mask/C-mask — zero for B, populated for A — so both branches share every other line of
//! machinery. QEMU exercises B only (its usb-kbd trains high-speed on the root port; QEMU has
//! no TT/HS-hub model — the FS usb-hub cannot even coexist with firmware on the EHCI bus), so
//! Topology A is metal-first by construction.
//!
//! WRITE SURFACE (tripwire-grade; the EHCI-2 wake surface plus, all declared): PORTSC.PR (RW1C
//! bits masked), USBCMD.ASE/PSE, PERIODICLISTBASE / ASYNCLISTADDR / CTRLDSSEGMENT=0, USBSTS RW1C
//! acks, the driver's own frame-list/QH/qTD/buffer DMA memory, EP0 device + hub-class requests,
//! and (Peter-approved surface extension, 2026-07-16) the EHCI functions' own PCI COMMAND
//! Memory-Space + Bus-Master enables — read-checked, set only if clear, traced. Never any xHCI
//! register, never a switchable-mask port, never an unlisted register. Every MMIO access is
//! translate()-guarded; every wait is bounded; a stuck handshake is a traced STOP-NOTE.

pub mod qh;

use super::ehci_scout::{
    self, mmio_read32, mmio_write32, settle_ms, wait_bounded, EhciFnHandle, OP_PORTSC0, OP_USBCMD,
    OP_USBSTS,
};
use crate::arch::pci::{read_config_16, read_config_32, write_config_32};
use alloc::vec::Vec;
use qh::*;
use spin::Mutex;

/// EHCI class code (base 0x0C Serial Bus, subclass 0x03 USB, prog-IF 0x20 EHCI).
const EHCI_CLASS: u8 = 0x0C;
const EHCI_SUBCLASS: u8 = 0x03;
const EHCI_PROGIF: u8 = 0x20;

/// Operational registers this driver adds beyond the scout's set.
const OP_CTRLDSSEGMENT: u64 = 0x10;
const OP_PERIODICLISTBASE: u64 = 0x14;
const OP_ASYNCLISTADDR: u64 = 0x18;

/// USBCMD schedule enables.
const CMD_PSE: u32 = 1 << 4;
const CMD_ASE: u32 = 1 << 5;

/// PORTSC bits (see ehci_scout for the census decode of the same register).
const PORT_CCS: u32 = 1 << 0;
const PORT_PED: u32 = 1 << 2;
const PORT_PR: u32 = 1 << 8;
const PORT_OWNER: u32 = 1 << 13;
/// PORTSC RW1C change bits (CSC/PEC/OCC) — masked off every read-modify-write so a routine
/// PR/PP write never silently acknowledges a latched change (the EHCI-2 discipline).
const PORT_RW1C: u32 = (1 << 1) | (1 << 3) | (1 << 5);

/// USBSTS RW1C status bits acked during polling (USBINT/ERR/port-change/rollover/host-error/
/// async-advance). Ack-only — USBINTR is never written (polling model).
const STS_RW1C: u32 = 0x3F;

/// One endpoint-addressing tuple for the shared control QH: everything dword 1/2 of a QH needs.
/// `hub_addr`/`hub_port` are the TT fields — zero on Topology B / high-speed targets, the RMH's
/// address + downstream port on Topology A (that parameterization IS the branch reachability).
#[derive(Clone, Copy)]
struct Target {
    addr: u8,
    mps0: u16,
    eps: u32, // one of QH_EPS_FULL/LOW/HIGH
    hub_addr: u8,
    hub_port: u8,
}

/// One armed HID interrupt-IN endpoint: its periodic QH, single re-armed qTD, report buffer,
/// and the software-tracked data toggle (QH runs DTC=1 so the toggle is explicit here, never
/// hidden controller state).
struct IntEp {
    qh: *mut Qh,
    qtd: *mut Qtd,
    buf: *mut u8,
    mps: u16,
    toggle: bool,
    is_kbd: bool,
    is_rel_mouse: bool,
    reports: u32,
    dead: bool,
}

/// One woken, schedule-bearing EHCI function.
pub struct Controller {
    idx: usize,
    op: u64,
    async_qh: *mut Qh,
    /// The three reusable control-transfer qTDs (SETUP/DATA/STATUS) + their buffers. One
    /// synchronous transfer at a time — enumeration is strictly one-device-at-a-time (the same
    /// invariant the xHCI enum FSM enforces), so reuse is safe by construction.
    qtd_setup: *mut Qtd,
    qtd_data: *mut Qtd,
    qtd_status: *mut Qtd,
    setup_buf: *mut u8,
    data_buf: *mut u8,
    frame_list: u64,
    periodic_on: bool,
    /// N2: driver-owned address allocator — EHCI has no controller slot model. Monotonic;
    /// a failed enumeration BURNS its address (never reused for a possibly-half-addressed
    /// device — mirror of dispose_downstream_slot's honesty). The 7-bit space bounds this at
    /// 127 devices per controller per boot; with 2 root ports + a ≤8-port RMH tier and no
    /// hot-plug rescan in this arc, exhaustion is unreachable in practice and traced if hit.
    next_addr: u8,
    int_eps: Vec<IntEp>,
}

// Raw pointers to identity-mapped DMA memory; access is serialized by the EHCI_HID mutex.
unsafe impl Send for Controller {}

pub static EHCI_HID: Mutex<Option<Vec<Controller>>> = Mutex::new(None);

/// PCI COMMAND register: Memory Space (bit 1) + Bus Master (bit 2). Read-checked, set only if
/// clear (Peter-approved write-surface extension): without BME the controller can never fetch a
/// single QH, so this is a hard precondition traced honestly rather than assumed.
unsafe fn ensure_bus_master(bus: u8, dev: u8, func: u8, idx: usize) {
    let cmd = read_config_32(bus, dev, func, 0x04);
    if cmd & 0x6 == 0x6 {
        serial_println!(
            ":: EHCI-HID: [{}] PCI COMMAND={:#06x} — memory-space + bus-master already enabled ::",
            idx,
            cmd & 0xFFFF
        );
        return;
    }
    // Preserve the status half (hi16 is RW1C — writing back read 1s would ack them; the dword
    // write only carries the low half's new value, status bits are written as read... so mask
    // the RW1C status half to 0 instead: config writes to STATUS are write-1-to-clear).
    write_config_32(bus, dev, func, 0x04, (cmd & 0x0000_FFFF) | 0x6);
    let after = read_config_32(bus, dev, func, 0x04);
    serial_println!(
        ":: EHCI-HID: [{}] PCI COMMAND {:#06x}->{:#06x} (memory-space + bus-master enabled; declared surface extension) ::",
        idx,
        cmd & 0xFFFF,
        after & 0xFFFF
    );
}

impl Controller {
    /// Program the still-disabled schedules: CTRLDSSEGMENT=0 (32-bit addressing — Panther Point
    /// HCCPARAMS advertises nothing else), the all-Terminate frame list, and the single
    /// self-linked head-of-reclamation control QH. Enables ASE; PSE waits until the first
    /// interrupt endpoint exists (an empty periodic walk buys nothing).
    unsafe fn init_schedules(&mut self) {
        let _ = mmio_write32(self.op + OP_CTRLDSSEGMENT, 0);
        let _ = mmio_write32(self.op + OP_PERIODICLISTBASE, self.frame_list as u32);

        let qh = self.async_qh;
        (*qh).horiz = (qh as u32) | PTR_TYPE_QH; // single-QH circular async list
        (*qh).ep_chars = QH_HEAD | QH_DTC | QH_EPS_HIGH | (64 << QH_MPS_SHIFT); // rewritten per target
        (*qh).ep_caps = QH_MULT1;
        (*qh).overlay[0] = PTR_TERMINATE;
        (*qh).overlay[1] = PTR_TERMINATE;
        (*qh).overlay[2] = 0; // inactive token — controller skips until a transfer is primed
        let _ = mmio_write32(self.op + OP_ASYNCLISTADDR, qh as u32);

        let cmd = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
        let _ = mmio_write32(self.op + OP_USBCMD, cmd | CMD_ASE);
        serial_println!(
            ":: EHCI-HID: [{}] schedules armed: framelist={:#x} asyncQH={:#x} ASE=1 (PSE deferred to first HID endpoint) ::",
            self.idx,
            self.frame_list,
            qh as u64
        );
    }

    /// One synchronous EP0 control transfer through the shared QH (the EHCI analogue of xHCI's
    /// `sync_control`: main-loop context, never inside an interrupt). Returns the transferred
    /// data-stage byte count. Bounded — a wedged Active bit is a traced Err, never a hang.
    unsafe fn control(
        &mut self,
        t: &Target,
        bm_req: u8,
        b_req: u8,
        w_value: u16,
        w_index: u16,
        w_length: u16,
        dir_in: bool,
    ) -> Result<u32, &'static str> {
        // Retarget the shared QH. C-bit (control-endpoint) only for FS/LS targets — that is
        // what makes the controller drive the SSPLIT/CSPLIT control dance through the TT named
        // by hub_addr/hub_port on Topology A; both fields stay 0 on Topology B.
        let qh = self.async_qh;
        let mut chars = (t.addr as u32)
            | t.eps
            | QH_DTC
            | QH_HEAD
            | ((t.mps0 as u32) << QH_MPS_SHIFT);
        if t.eps != QH_EPS_HIGH {
            chars |= QH_CTL_EP;
        }
        (*qh).ep_chars = chars;
        (*qh).ep_caps = QH_MULT1
            | ((t.hub_addr as u32) << QH_HUBADDR_SHIFT)
            | ((t.hub_port as u32) << QH_PORT_SHIFT);

        // Setup packet.
        let sb = self.setup_buf;
        sb.write(bm_req);
        sb.add(1).write(b_req);
        sb.add(2).write(w_value as u8);
        sb.add(3).write((w_value >> 8) as u8);
        sb.add(4).write(w_index as u8);
        sb.add(5).write((w_index >> 8) as u8);
        sb.add(6).write(w_length as u8);
        sb.add(7).write((w_length >> 8) as u8);

        // qTD chain: SETUP (DT0) -> [DATA (DT1, controller advances the toggle across packets
        // within the qTD)] -> STATUS (opposite direction, DT1, IOC). Data <= 64 B by contract.
        let (setup, data, status) = (self.qtd_setup, self.qtd_data, self.qtd_status);
        let status_pid = if w_length == 0 || !dir_in { QTD_PID_IN } else { QTD_PID_OUT };
        write_qtd(status, PTR_TERMINATE, status_pid | QTD_DT | QTD_IOC, 0, 0);
        let first_after_setup = if w_length > 0 {
            let data_pid = if dir_in { QTD_PID_IN } else { QTD_PID_OUT };
            write_qtd(data, status as u32, data_pid | QTD_DT, w_length as u32, self.data_buf as u64);
            data as u32
        } else {
            status as u32
        };
        write_qtd(setup, first_after_setup, QTD_PID_SETUP, 8, self.setup_buf as u64);

        // Prime the QH overlay and let the async traversal pick it up (EHCI has no doorbell:
        // the controller polls the list while ASE=1).
        (*qh).overlay[1] = PTR_TERMINATE;
        core::ptr::write_volatile(&mut (*qh).overlay[2], 0);
        core::ptr::write_volatile(&mut (*qh).overlay[0], setup as u32);

        // Bounded completion wait on the terminating qTD, failing fast on any halted qTD.
        let done = wait_bounded(|| {
            let st = core::ptr::read_volatile(&(*status).token);
            if st & QTD_ACTIVE == 0 {
                return true;
            }
            (core::ptr::read_volatile(&(*setup).token) & QTD_HALTED != 0)
                || (w_length > 0 && core::ptr::read_volatile(&(*data).token) & QTD_HALTED != 0)
        });
        // Quiesce the QH again whatever happened.
        core::ptr::write_volatile(&mut (*qh).overlay[0], PTR_TERMINATE);

        if !done {
            serial_println!(
                ":: EHCI-HID: [{}] STOP-NOTE EP0 timeout addr={} req={:#04x}/{:#04x} (setup token {:#010x}) — not forced ::",
                self.idx, t.addr, bm_req, b_req, core::ptr::read_volatile(&(*setup).token)
            );
            return Err("timeout");
        }
        for (name, q) in [("SETUP", setup), ("DATA", data), ("STATUS", status)] {
            if name == "DATA" && w_length == 0 {
                continue;
            }
            let tok = core::ptr::read_volatile(&(*q).token);
            if tok & QTD_ERR_MASK != 0 {
                serial_println!(
                    ":: EHCI-HID: [{}] EP0 {} error addr={} req={:#04x}/{:#04x} token={:#010x} (halted/xact — likely STALL) ::",
                    self.idx, name, t.addr, bm_req, b_req, tok
                );
                return Err("stall");
            }
        }
        let residual = if w_length > 0 {
            (core::ptr::read_volatile(&(*data).token) >> QTD_TOTAL_SHIFT) & 0x7FFF
        } else {
            0
        };
        Ok((w_length as u32).saturating_sub(residual))
    }

    /// Debounce + reset + enable one root port. Returns true when the port enabled on EHCI
    /// (PED=1 ⇒ a high-speed-capable device trained). PED=0 after a clean reset is the
    /// no-companion release case — paced retries (the xHCI metal lesson), then an honest STOP.
    unsafe fn reset_root_port(&mut self, port: u32) -> bool {
        let addr = self.op + OP_PORTSC0 + 4 * port as u64;
        settle_ms(100); // USB 2.0 TATTDB connect debounce (xHCI metal lesson, transport-free)
        for (attempt, pace) in [(1u32, 0u64), (2, 200), (3, 400), (4, 600)] {
            if pace != 0 {
                settle_ms(pace);
            }
            let before = mmio_read32(addr).unwrap_or(0);
            if before & PORT_CCS == 0 {
                serial_println!(
                    ":: EHCI-HID: [{}] port {} connect dropped during reset sequence (PORTSC={:#010x}) ::",
                    self.idx, port, before
                );
                return false;
            }
            // Assert reset: PR=1 with PED cleared, RW1C change bits masked. Hold >= 50 ms.
            let _ = mmio_write32(addr, (before & !PORT_RW1C & !PORT_PED) | PORT_PR);
            settle_ms(50);
            let held = mmio_read32(addr).unwrap_or(0);
            let _ = mmio_write32(addr, held & !PORT_RW1C & !PORT_PR);
            let cleared = wait_bounded(|| mmio_read32(addr).unwrap_or(PORT_PR) & PORT_PR == 0);
            settle_ms(10); // post-reset recovery before trusting PED
            let after = mmio_read32(addr).unwrap_or(0);
            serial_println!(
                ":: EHCI-HID: [{}] port {} reset attempt {}: PORTSC {:#010x} -> {:#010x} (PR-cleared={} PED={} owner={}) ::",
                self.idx, port, attempt, before, after, cleared,
                (after >> 2) & 1,
                if after & PORT_OWNER != 0 { "companion" } else { "EHCI" }
            );
            if cleared && after & PORT_PED != 0 {
                return true;
            }
            if after & PORT_OWNER != 0 {
                break; // released toward a companion that does not exist — no retry will help
            }
        }
        serial_println!(
            ":: EHCI-HID: [{}] STOP-NOTE port {} did not enable on EHCI after paced retries — FS/LS-on-root-port release case (no companion on this silicon); reported, not forced ::",
            self.idx, port
        );
        false
    }

    /// N2 address allocation. The failure paths after this call BURN the address by
    /// construction: `next_addr` is monotonic and nothing ever hands an address back.
    fn alloc_addr(&mut self) -> Option<u8> {
        if self.next_addr > 127 {
            serial_println!(
                ":: EHCI-HID: [{}] STOP-NOTE 7-bit address space exhausted (127 enumerations this boot) ::",
                self.idx
            );
            return None;
        }
        let a = self.next_addr;
        self.next_addr += 1;
        Some(a)
    }

    /// Enumerate the device currently answering address 0 (one at a time, strictly). `depth`
    /// bounds the hub recursion at the RMH tier (root=0; children of the RMH=1; deeper hubs are
    /// out of this arc's scope and traced as skipped).
    unsafe fn enumerate_at_zero(&mut self, eps: u32, hub_addr: u8, hub_port: u8, depth: u8) {
        let mut t = Target {
            addr: 0,
            mps0: if eps == QH_EPS_HIGH { 64 } else { 8 },
            eps,
            hub_addr,
            hub_port,
        };

        // 8-byte device-descriptor header first: learns the real bMaxPacketSize0 before any
        // longer read (the XENUM-3 short-read trap — a FS MPS0 is 8/16/32, never the 64 guess).
        if self.control(&t, 0x80, 6, 0x0100, 0, 8, true).is_err() {
            serial_println!(
                ":: EHCI-HID: [{}] enumeration aborted: device at addr 0 (via hub {} port {}) never answered GET_DESCRIPTOR(8) ::",
                self.idx, hub_addr, hub_port
            );
            return;
        }
        let mps0 = *self.data_buf.add(7) as u16;
        if [8u16, 16, 32, 64].contains(&mps0) {
            t.mps0 = mps0;
        }

        let Some(addr) = self.alloc_addr() else { return };
        if self.control(&t, 0x00, 5, addr as u16, 0, 0, false).is_err() {
            serial_println!(
                ":: EHCI-HID: [{}] address {} BURNED (SET_ADDRESS failed; possibly half-addressed device — address never reused, state cleared) ::",
                self.idx, addr
            );
            return;
        }
        settle_ms(2); // SET_ADDRESS recovery interval (USB 9.2.6.3)
        t.addr = addr;

        // Full device descriptor: the M1 branch-decider evidence.
        let Ok(n) = self.control(&t, 0x80, 6, 0x0100, 0, 18, true) else {
            serial_println!(
                ":: EHCI-HID: [{}] address {} BURNED (full device descriptor unreadable post-address) ::",
                self.idx, addr
            );
            return;
        };
        if n < 18 {
            serial_println!(
                ":: EHCI-HID: [{}] address {} BURNED (short device descriptor: {} bytes) ::",
                self.idx, addr, n
            );
            return;
        }
        let d = core::slice::from_raw_parts(self.data_buf, 18);
        let class = d[4];
        let vid = (d[8] as u16) | ((d[9] as u16) << 8);
        let pid = (d[10] as u16) | ((d[11] as u16) << 8);
        let speed = match t.eps {
            QH_EPS_HIGH => "HS",
            QH_EPS_LOW => "LS",
            _ => "FS",
        };
        // The M1 witness. At depth 0 this line IS the topology fork decision (design §2.4).
        if depth == 0 {
            serial_println!(
                ":: EHCI-HID: [{}] M1 root device addr={} {:04x}:{:04x} class={:#04x} speed={} -> TOPOLOGY {} == witness ::",
                self.idx, addr, vid, pid, class, speed,
                if class == 0x09 { "A (hub tier / RMH)" } else { "B (direct device)" }
            );
        } else {
            serial_println!(
                ":: EHCI-HID: [{}] M1 hub-downstream device addr={} {:04x}:{:04x} class={:#04x} speed={} (hub {} port {}) == witness ::",
                self.idx, addr, vid, pid, class, speed, hub_addr, hub_port
            );
        }

        if class == 0x09 {
            if depth >= 1 {
                serial_println!(
                    ":: EHCI-HID: [{}] hub behind hub (addr {}) — beyond the RMH tier, out of this arc's scope; skipped ::",
                    self.idx, addr
                );
                return;
            }
            self.bring_up_hub(&t);
        } else {
            self.configure_hid(&t);
        }
    }

    /// Topology A: enumerate the hub (the RMH on metal), walk its downstream ports, and
    /// enumerate each connected child through the hub's TT.
    unsafe fn bring_up_hub(&mut self, hub: &Target) {
        if self.control(hub, 0x00, 9, 1, 0, 0, false).is_err() {
            serial_println!(":: EHCI-HID: [{}] hub addr {} SET_CONFIGURATION failed ::", self.idx, hub.addr);
            return;
        }
        // USB2 hub descriptor (class 0xA0, type 0x29): bNbrPorts at [2], bPwrOn2PwrGood at [5].
        let Ok(n) = self.control(hub, 0xA0, 6, 0x2900, 0, 9, true) else {
            serial_println!(":: EHCI-HID: [{}] hub addr {} hub-descriptor read failed ::", self.idx, hub.addr);
            return;
        };
        if n < 7 {
            serial_println!(":: EHCI-HID: [{}] hub addr {} short hub descriptor ({} bytes) ::", self.idx, hub.addr, n);
            return;
        }
        let nbr_ports = (*self.data_buf.add(2)).min(15);
        let pwr2good_ms = (*self.data_buf.add(5) as u64) * 2;
        serial_println!(
            ":: EHCI-HID: [{}] hub addr {}: {} downstream ports (pwr-on 2 good {} ms) — walking ::",
            self.idx, hub.addr, nbr_ports, pwr2good_ms
        );

        // Power every port first (SET_PORT_FEATURE PORT_POWER=8; harmless where always-on).
        for port in 1..=nbr_ports as u16 {
            let _ = self.control(hub, 0x23, 3, 8, port, 0, false);
        }
        settle_ms(pwr2good_ms + 100);

        for port in 1..=nbr_ports as u16 {
            // GET_PORT_STATUS: wPortStatus (lo16) + wPortChange (hi16).
            let Ok(4) = self.control(hub, 0xA3, 0, 0, port, 4, true) else { continue };
            let st = (*self.data_buf as u32)
                | ((*self.data_buf.add(1) as u32) << 8)
                | ((*self.data_buf.add(2) as u32) << 16)
                | ((*self.data_buf.add(3) as u32) << 24);
            if st & 1 == 0 {
                continue; // no connection
            }
            // Reset the downstream port (SET_PORT_FEATURE PORT_RESET=4), bounded completion.
            if self.control(hub, 0x23, 3, 4, port, 0, false).is_err() {
                continue;
            }
            settle_ms(50);
            // Bounded reset-completion poll (explicit loop: each probe is itself a control
            // transfer, so the generic wait_bounded closure can't drive it). ~600 ms worst case.
            let mut status = 0u32;
            let mut ok = false;
            for _ in 0..60 {
                if let Ok(4) = self.control(hub, 0xA3, 0, 0, port, 4, true) {
                    status = (*self.data_buf as u32)
                        | ((*self.data_buf.add(1) as u32) << 8)
                        | ((*self.data_buf.add(2) as u32) << 16)
                        | ((*self.data_buf.add(3) as u32) << 24);
                    if status & (1 << 4) == 0 {
                        ok = true; // PORT_RESET no longer asserted
                        break;
                    }
                }
                settle_ms(10);
            }
            // Ack the change bits we may have latched (C_PORT_CONNECTION=16, C_PORT_RESET=20).
            let _ = self.control(hub, 0x23, 1, 16, port, 0, false);
            let _ = self.control(hub, 0x23, 1, 20, port, 0, false);
            if !ok || status & (1 << 1) == 0 {
                serial_println!(
                    ":: EHCI-HID: [{}] hub {} port {} did not enable after reset (status {:#010x}) — skipped ::",
                    self.idx, hub.addr, port, status
                );
                continue;
            }
            // wPortStatus bit9 = low speed, bit10 = high speed, neither = full speed.
            let child_eps = if status & (1 << 9) != 0 {
                QH_EPS_LOW
            } else if status & (1 << 10) != 0 {
                QH_EPS_HIGH
            } else {
                QH_EPS_FULL
            };
            // A FS/LS child is reached via THIS hub's TT: hub_addr = the hub, port = this
            // port. A HS child needs no TT (fields stay zero).
            let (ha, hp) = if child_eps == QH_EPS_HIGH { (0, 0) } else { (hub.addr, port as u8) };
            settle_ms(10);
            self.enumerate_at_zero(child_eps, ha, hp, 1);
        }
    }

    /// Topology B leaf (or an RMH child): read the config, boot-protocol every boot-capable HID
    /// interface, and arm one periodic interrupt QH per HID endpoint.
    unsafe fn configure_hid(&mut self, t: &Target) {
        let Ok(n) = self.control(t, 0x80, 6, 0x0200, 0, 9, true) else {
            serial_println!(":: EHCI-HID: [{}] addr {} config-descriptor header read failed ::", self.idx, t.addr);
            return;
        };
        if n < 9 {
            return;
        }
        let total =
            (((*self.data_buf.add(2) as u16) | ((*self.data_buf.add(3) as u16) << 8)).min(64)) as u16;
        let config_value = *self.data_buf.add(5);
        if self.control(t, 0x80, 6, 0x0200, 0, total, true).is_err() {
            return;
        }

        // Walk interface/endpoint descriptors — the same walk as xHCI's parse_hid_config, but
        // collecting EVERY HID interrupt-IN endpoint: the Apple internal keyboard+trackpad are
        // one composite device with multiple HID interfaces.
        let cfg = core::slice::from_raw_parts(self.data_buf, total as usize);
        let mut found: [Option<(u8, u8, u16, u8, u8)>; 4] = [None; 4]; // (proto, ep, mps, interval, intf)
        let mut nfound = 0;
        let (mut off, mut in_hid, mut proto, mut intf) = (0usize, false, 0u8, 0u8);
        while off + 2 <= cfg.len() {
            let len = cfg[off] as usize;
            if len == 0 {
                break;
            }
            match cfg[off + 1] {
                0x04 if off + 8 <= cfg.len() => {
                    intf = cfg[off + 2];
                    in_hid = cfg[off + 5] == 0x03;
                    proto = cfg[off + 7];
                }
                0x05 if in_hid && off + 7 <= cfg.len() => {
                    let ep = cfg[off + 2];
                    if ep & 0x80 != 0 && cfg[off + 3] & 0x3 == 3 && nfound < 4 {
                        let mps = ((cfg[off + 4] as u16) | ((cfg[off + 5] as u16) << 8)) & 0x7FF;
                        found[nfound] = Some((proto, ep & 0xF, mps, cfg[off + 6], intf));
                        nfound += 1;
                    }
                }
                _ => {}
            }
            off += len;
        }
        if nfound == 0 {
            serial_println!(
                ":: EHCI-HID: [{}] addr {} has no HID interrupt-IN endpoint — nothing to arm ::",
                self.idx, t.addr
            );
            return;
        }
        if self.control(t, 0x00, 9, config_value as u16, 0, 0, false).is_err() {
            serial_println!(":: EHCI-HID: [{}] addr {} SET_CONFIGURATION failed ::", self.idx, t.addr);
            return;
        }

        for slot in found.iter().flatten() {
            let (proto, ep, mps, interval, intf) = *slot;
            // Only boot interfaces accept SET_PROTOCOL (proto 1 = keyboard, 2 = mouse). A
            // report-protocol/vendor interface (proto 0 — the likely Apple trackpad case, R3)
            // is skipped with an honest line; the keyboard still gates M3.
            if proto != 1 && proto != 2 {
                serial_println!(
                    ":: EHCI-HID: [{}] addr {} intf {} is non-boot HID (proto {}) — needs a report-descriptor parser, out of this arc; skipped ::",
                    self.idx, t.addr, intf, proto
                );
                continue;
            }
            // SET_PROTOCOL(boot): bmRequestType 0x21, bRequest 0x0B, wValue 0 (=Boot), wIndex =
            // interface — the exact request the xHCI path sends (set_hid_boot_protocol).
            if self.control(t, 0x21, 0x0B, 0, intf as u16, 0, false).is_err() {
                serial_println!(
                    ":: EHCI-HID: [{}] addr {} intf {} SET_PROTOCOL(boot) refused — skipped (R3) ::",
                    self.idx, t.addr, intf
                );
                continue;
            }
            self.arm_interrupt_ep(t, ep, mps.min(64), proto == 1, proto == 2);
            serial_println!(
                ":: EHCI-HID: [{}] M2 armed {} addr={} ep=IN{} mps={} interval={} (boot protocol) == witness ::",
                self.idx,
                if proto == 1 { "keyboard" } else { "boot-mouse" },
                t.addr, ep, mps, interval
            );
        }
    }

    /// Build + link one periodic interrupt QH and arm its first qTD.
    ///
    /// N1 — split-compatibility of the every-frame frame list (EHCI 4.12.2): S-mask and C-mask
    /// are *microframe* masks evaluated within EACH frame the QH is reached in, not frame
    /// selectors. Linking the QH from every frame-list entry therefore stays split-correct: on
    /// a FS/LS endpoint the controller issues the start-split in µframe 0 (S-mask 0x01) and
    /// polls complete-splits in µframes 2-4 (C-mask 0x1C) of every frame — a 1 ms service rate
    /// that over-serves an 8 ms boot-HID interval but is legal and loses nothing. On a HS
    /// endpoint (Topology B) C-mask must be zero and S-mask alone paces one transaction per
    /// frame. The simplification is deliberate and verified against 4.12.2, not inherited
    /// silently; bInterval striding is a later refinement (design R8) if TT load ever demands.
    unsafe fn arm_interrupt_ep(&mut self, t: &Target, ep: u8, mps: u16, is_kbd: bool, is_rel: bool) {
        let qh = alloc_qh();
        let qtd = alloc_qtd();
        let buf = alloc_buf64();

        (*qh).ep_chars = (t.addr as u32)
            | ((ep as u32) << 8)
            | t.eps
            | QH_DTC
            | ((mps as u32) << QH_MPS_SHIFT);
        let split = if t.eps == QH_EPS_HIGH {
            0
        } else {
            (0x1C << QH_CMASK_SHIFT)
                | ((t.hub_addr as u32) << QH_HUBADDR_SHIFT)
                | ((t.hub_port as u32) << QH_PORT_SHIFT)
        };
        (*qh).ep_caps = QH_MULT1 | (0x01 << QH_SMASK_SHIFT) | split;

        // First transfer of a freshly-configured interrupt endpoint is DATA0.
        write_qtd(qtd, PTR_TERMINATE, QTD_PID_IN | QTD_IOC, mps as u32, buf as u64);
        (*qh).overlay[1] = PTR_TERMINATE;
        (*qh).overlay[2] = 0;
        (*qh).overlay[0] = qtd as u32;

        // Link: new QH points at the current chain head, then every frame-list entry points at
        // the new QH (entries were Terminate or the old head — both cases are one word).
        let fl = self.frame_list as *mut u32;
        let old_head = core::ptr::read_volatile(fl);
        (*qh).horiz = old_head;
        for i in 0..1024 {
            core::ptr::write_volatile(fl.add(i), (qh as u32) | PTR_TYPE_QH);
        }

        if !self.periodic_on {
            let cmd = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
            let _ = mmio_write32(self.op + OP_USBCMD, cmd | CMD_PSE);
            self.periodic_on = true;
        }

        self.int_eps.push(IntEp {
            qh,
            qtd,
            buf,
            mps,
            toggle: false,
            is_kbd,
            is_rel_mouse: is_rel,
            reports: 0,
            dead: false,
        });
    }

    /// Main-loop poll: ack USBSTS, then for each armed endpoint consume a completed report,
    /// decode it through the boot-report layouts (same table + logic as the xHCI HID path), and
    /// re-arm. The EHCI analogue of the xHCI `queue_keyboard_read` re-arm idiom.
    unsafe fn service(&mut self) {
        if let Some(sts) = mmio_read32(self.op + OP_USBSTS) {
            if sts & STS_RW1C != 0 {
                let _ = mmio_write32(self.op + OP_USBSTS, sts & STS_RW1C);
            }
        }
        let idx = self.idx;
        for e in self.int_eps.iter_mut() {
            if e.dead {
                continue;
            }
            let tok = core::ptr::read_volatile(&(*e.qtd).token);
            if tok & QTD_ACTIVE != 0 {
                continue;
            }
            if tok & QTD_ERR_MASK != 0 {
                serial_println!(
                    ":: EHCI-HID: [{}] STOP-NOTE interrupt endpoint halted (token {:#010x}) — endpoint retired, not forced ::",
                    idx, tok
                );
                e.dead = true;
                continue;
            }
            let len = (e.mps as u32).saturating_sub((tok >> QTD_TOTAL_SHIFT) & 0x7FFF) as usize;
            if len > 0 {
                let report = core::slice::from_raw_parts(e.buf, len.min(8));
                e.reports = e.reports.wrapping_add(1);
                if e.is_kbd {
                    decode_boot_keyboard(report);
                    if e.reports == 1 || e.reports % 32 == 0 {
                        serial_println!(
                            ":: EHCI-HID: [{}] kbd {} reports, last {:02x} {:02x} .. == witness ::",
                            idx, e.reports, report[0], report.get(2).copied().unwrap_or(0)
                        );
                    }
                } else if e.is_rel_mouse && len >= 3 {
                    let (dx, dy) = (report[1] as i8 as i32, report[2] as i8 as i32);
                    if dx != 0 || dy != 0 {
                        crate::pal::push_event(crate::pal::Event::Mouse { x: dx, y: dy });
                    }
                    if e.reports == 1 || e.reports % 32 == 0 {
                        serial_println!(
                            ":: EHCI-HID: [{}] mouse {} reports, last dx={} dy={} buttons={:#04x} == witness ::",
                            idx, e.reports, dx, dy, report[0]
                        );
                    }
                }
            }
            // Re-arm: flip the software toggle (QH_DTC — the toggle lives here, not in the QH),
            // refresh the qTD, re-prime the overlay.
            e.toggle = !e.toggle;
            let dt = if e.toggle { QTD_DT } else { 0 };
            write_qtd(e.qtd, PTR_TERMINATE, QTD_PID_IN | QTD_IOC | dt, e.mps as u32, e.buf as u64);
            (*e.qh).overlay[1] = PTR_TERMINATE;
            core::ptr::write_volatile(&mut (*e.qh).overlay[2], 0);
            core::ptr::write_volatile(&mut (*e.qh).overlay[0], e.qtd as u32);
        }
    }
}

/// Boot-keyboard report decode — the same layout, scancode table, and Event delivery as the
/// xHCI keyboard path (xhci mod.rs event dispatch), so a key is a key whichever controller
/// carried it. Table shared via pub(crate) rather than duplicated.
unsafe fn decode_boot_keyboard(report: &[u8]) {
    if report.len() < 3 {
        return;
    }
    let shift = report[0] & 0x22 != 0; // L-Shift (bit 1) or R-Shift (bit 5)
    for &keycode in report.iter().skip(2) {
        if keycode <= 1 {
            continue; // no key / ErrorRollOver
        }
        if (keycode as usize) < super::xhci::HID_SCANCODE_TO_ASCII.len() {
            let (unshifted, shifted) = super::xhci::HID_SCANCODE_TO_ASCII[keycode as usize];
            let ascii = if shift { shifted } else { unshifted };
            if ascii != 0 {
                serial_println!("EHCI-HID: KEY: '{}' (scancode {:#x})", ascii as char, keycode);
                crate::pal::push_event(crate::pal::Event::Key(ascii));
            }
        }
    }
}

/// EHCI-3 bring-up: walk PCI for EHCI functions, run the SHARED EHCI-2 wake on each (one wake
/// path — wake_run + wake_route from ehci_scout), arm schedules, reset + enumerate the
/// connected root ports. Runs at PCI-init time, after the scout modes and BEFORE the PORTSW
/// flip / xhci::init (the internal HID sit on non-switchable EHCI-only ports, so the two
/// stacks' port sets are disjoint by hardware — PORTSW-1 §7f).
pub fn init() {
    serial_println!(":: EHCI-HID: begin (EHCI-3 driver, polling model, knob-gated) ::");
    let mut ctrls: Vec<Controller> = Vec::new();

    for bus in 0u8..=255 {
        for dev in 0u8..=31 {
            let vendor = unsafe { read_config_16(bus, dev, 0, 0x00) };
            if vendor == 0xFFFF {
                continue;
            }
            let ht = ((unsafe { read_config_32(bus, dev, 0, 0x0C) } >> 16) & 0xFF) as u8;
            let max_func = if ht & 0x80 != 0 { 7 } else { 0 };
            for func in 0..=max_func {
                if func != 0 && unsafe { read_config_16(bus, dev, func, 0x00) } == 0xFFFF {
                    continue;
                }
                let class_reg = unsafe { read_config_32(bus, dev, func, 0x08) };
                if ((class_reg >> 24) & 0xFF) as u8 != EHCI_CLASS
                    || ((class_reg >> 16) & 0xFF) as u8 != EHCI_SUBCLASS
                    || ((class_reg >> 8) & 0xFF) as u8 != EHCI_PROGIF
                {
                    continue;
                }
                let idx = ctrls.len();
                unsafe {
                    ensure_bus_master(bus, dev, func, idx);
                    // The one shared wake path (idempotent when EHCI-2 mode already ran it).
                    let Some(h): Option<EhciFnHandle> = ehci_scout::wake_run(bus, dev, func, idx)
                    else {
                        continue;
                    };
                    ehci_scout::wake_route(&h, idx);
                    if h.addr64 != 0 {
                        serial_println!(
                            ":: EHCI-HID: [{}] note: controller advertises 64-bit addressing; CTRLDSSEGMENT pinned to 0 (all DMA < 4 GiB) ::",
                            idx
                        );
                    }
                    let mut c = Controller {
                        idx,
                        op: h.op,
                        async_qh: alloc_qh(),
                        qtd_setup: alloc_qtd(),
                        qtd_data: alloc_qtd(),
                        qtd_status: alloc_qtd(),
                        setup_buf: alloc_buf64(),
                        data_buf: alloc_buf64(),
                        frame_list: alloc_frame_list(),
                        periodic_on: false,
                        next_addr: 1,
                        int_eps: Vec::new(),
                    };
                    c.init_schedules();
                    for port in 0..h.n_ports {
                        let portsc = mmio_read32(h.op + OP_PORTSC0 + 4 * port as u64).unwrap_or(0);
                        if portsc & PORT_CCS == 0 || portsc & PORT_OWNER != 0 {
                            continue;
                        }
                        if c.reset_root_port(port) {
                            c.enumerate_at_zero(QH_EPS_HIGH, 0, 0, 0);
                        }
                    }
                    ctrls.push(c);
                }
            }
        }
    }

    let n = ctrls.len();
    let armed: usize = ctrls.iter().map(|c| c.int_eps.len()).sum();
    *EHCI_HID.lock() = Some(ctrls);
    serial_println!(
        ":: EHCI-HID: end ({} controllers, {} HID endpoints armed) ::",
        n, armed
    );
}

/// Main-loop service hook (the EHCI analogue of `service_hubs`): poll every armed HID endpoint,
/// decode + deliver completed reports, re-arm. Cheap when nothing completed.
pub fn service_ehci_hid() {
    let mut g = EHCI_HID.lock();
    let Some(ctrls) = g.as_mut() else { return };
    for c in ctrls.iter_mut() {
        unsafe { c.service() };
    }
}
