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

/// USBCMD bits.
const CMD_RS: u32 = 1 << 0;
const CMD_HCRESET: u32 = 1 << 1;
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
/// USBSTS Async Schedule Status (read-only; tracks USBCMD.ASE with a lag).
const STS_PSS: u32 = 1 << 14;
const STS_HCHALTED: u32 = 1 << 12;
const STS_HSE: u32 = 1 << 4; // Host System Error (DMA master/target abort) — halts the HC

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
    qtd_phys: u64,
    buf: *mut u8,
    buf_phys: u64,
    mps: u16,
    toggle: bool,
    is_kbd: bool,
    is_rel_mouse: bool,
    reports: u32,
    dead: bool,
}

/// One woken, schedule-bearing EHCI function. All DMA structures live in the static
/// `qh::DMA_POOLS` (kernel image, low physical); every `*_phys` field is the page-table-
/// resolved physical address the controller is actually programmed with (probe-5 discipline —
/// never the heap virt==phys shortcut).
pub struct Controller {
    idx: usize,
    op: u64,
    bus: u8,
    dev: u8,
    func: u8,
    /// Dummy async head (H=1, permanently inactive) + the work QH transfers run on.
    async_head: *mut Qh,
    head_phys: u64,
    async_qh: *mut Qh,
    qh_phys: u64,
    /// The three reusable control-transfer qTDs (SETUP/DATA/STATUS) + their buffers. One
    /// synchronous transfer at a time — enumeration is strictly one-device-at-a-time (the same
    /// invariant the xHCI enum FSM enforces), so reuse is safe by construction.
    qtd_setup: *mut Qtd,
    qtd_setup_phys: u64,
    qtd_data: *mut Qtd,
    qtd_data_phys: u64,
    qtd_status: *mut Qtd,
    qtd_status_phys: u64,
    setup_buf: *mut u8,
    setup_buf_phys: u64,
    data_buf: *mut u8,
    data_buf_phys: u64,
    frame_list: *mut u32,
    frame_list_phys: u64,
    int_next: usize,
    periodic_on: bool,
    /// Probe-14 self-adaptation: false = standard qTD-chain transfers (QEMU's model requires
    /// fetched qTDs); true = overlay-direct (this metal HSEs on the qTD-fetch burst write —
    /// flipped automatically on the first chain HSE, permanent for the controller's lifetime).
    overlay_mode: bool,
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
        // Static pools are zeroed at link time — set every frame-list entry to Terminate.
        for i in 0..1024 {
            core::ptr::write_volatile(self.frame_list.add(i), PTR_TERMINATE);
        }
        let _ = mmio_write32(self.op + OP_PERIODICLISTBASE, self.frame_list_phys as u32);

        // Linux-shaped async ring: inactive dummy HEAD (H=1) -> work QH -> head. The head
        // carries the reclamation bit and NEVER a transfer (probe-7 metal finding: an active
        // self-linked H-QH master-aborts Panther Point's async engine; QEMU tolerated it).
        let head = self.async_head;
        (*head).horiz = (self.qh_phys as u32) | PTR_TYPE_QH;
        (*head).ep_chars = QH_HEAD | QH_DTC | QH_EPS_HIGH | (64 << QH_MPS_SHIFT);
        (*head).ep_caps = QH_MULT1;
        (*head).overlay[0] = PTR_TERMINATE;
        (*head).overlay[1] = PTR_TERMINATE;
        (*head).overlay[2] = QTD_HALTED; // permanently idle (Linux marks its dummy head halted)
        let qh = self.async_qh;
        (*qh).horiz = (self.head_phys as u32) | PTR_TYPE_QH;
        (*qh).ep_chars = QH_DTC | QH_EPS_HIGH | (64 << QH_MPS_SHIFT); // rewritten per target
        (*qh).ep_caps = QH_MULT1;
        (*qh).overlay[0] = PTR_TERMINATE;
        (*qh).overlay[1] = PTR_TERMINATE;
        (*qh).overlay[2] = 0; // inactive token — controller skips until a transfer is primed
        let _ = mmio_write32(self.op + OP_ASYNCLISTADDR, self.head_phys as u32);

        // ASE is NOT set here. Real Intel EHCI parks async traversal on an empty schedule
        // (EHCI 4.8.3 empty-schedule detection: one H-bit QH with an inactive overlay) and
        // never re-walks when a transfer is later primed — the first rMBP probe showed exactly
        // that (SETUP qTD stayed Active, zero error bits, wake clean). QEMU re-walks every
        // frame and masked it. So the async schedule is enabled per transfer, with bounded
        // USBSTS.ASS handshakes — the Linux ehci-hcd idiom.
        // Evidence line: virt AND page-table-resolved phys of the QH the controller will
        // fetch (probe-5: static-pool DMA, low physical), + CTRLDSSEGMENT read-back.
        serial_println!(
            ":: EHCI-HID: [{}] schedules armed (static pool): framelist phys={:#x} async head={:#x} work QH={:#x} CTRLDSSEGMENT={:#x} (dummy-head ring, ASE per-transfer, PSE deferred) ::",
            self.idx,
            self.frame_list_phys,
            self.head_phys,
            self.qh_phys,
            mmio_read32(self.op + OP_CTRLDSSEGMENT).unwrap_or(u32::MAX)
        );
    }

    /// Probe-2 metal finding (2026-07-16): Apple EFI drives the internal keyboard over these
    /// EHCI functions pre-boot and leaves USBCMD.PSE=1 behind — the shared wake's RS=1 then set
    /// the controller fetching firmware's STALE frame list (its memory long reclaimed) →
    /// garbage pointers → USBSTS Host System Error → HCHalted, RS dropped, every transfer
    /// timed out (USBSTS=0x0000f01e/f01f, both functions). This is exactly the pre-approved
    /// HCRESET trigger (Peter item (c): reset only on M1-observed inconsistent state, tracing
    /// the inconsistency). Detect stale schedule enables / a latched HSE, trace them verbatim,
    /// then stop + HCRESET so the controller is programmed from its true default state.
    /// Returns true when a reset was performed (caller must re-start RS and re-route CF —
    /// HCRESET clears both). A clean controller (QEMU; a warm re-init) is left untouched.
    unsafe fn quiesce_if_firmware_stale(&mut self) -> bool {
        let cmd = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
        let sts = mmio_read32(self.op + OP_USBSTS).unwrap_or(0);
        let stale = cmd & (CMD_PSE | CMD_ASE) != 0 || sts & STS_HSE != 0;
        if !stale {
            return false;
        }
        serial_println!(
            ":: EHCI-HID: [{}] firmware-stale controller state: USBCMD={:#010x} (PSE={} ASE={}) USBSTS={:#010x} (HSE={}) — pre-approved HCRESET path (traced inconsistency) ::",
            self.idx, cmd,
            (cmd >> 4) & 1, (cmd >> 5) & 1, sts, (sts >> 4) & 1
        );
        // Stop first (RS=0 + schedule enables off), bounded wait for halt (no-op if HSE
        // already halted it), then reset and wait for HCRESET to self-clear.
        let _ = mmio_write32(self.op + OP_USBCMD, cmd & !(CMD_RS | CMD_PSE | CMD_ASE));
        let halted = wait_bounded(|| {
            mmio_read32(self.op + OP_USBSTS).unwrap_or(0) & STS_HCHALTED != 0
        });
        let _ = mmio_write32(self.op + OP_USBCMD, CMD_HCRESET);
        let reset_done = wait_bounded(|| {
            mmio_read32(self.op + OP_USBCMD).unwrap_or(CMD_HCRESET) & CMD_HCRESET == 0
        });
        serial_println!(
            ":: EHCI-HID: [{}] HCRESET: halted={} reset-cleared={} USBCMD={:#010x} USBSTS={:#010x} (defaults; RS + CONFIGFLAG re-applied next) ::",
            self.idx, halted, reset_done,
            mmio_read32(self.op + OP_USBCMD).unwrap_or(0),
            mmio_read32(self.op + OP_USBSTS).unwrap_or(0)
        );
        true
    }

    /// Enable/disable the periodic schedule with a bounded wait for USBSTS.PSS (bit 14) to
    /// agree with USBCMD.PSE (EHCI 4.8 discipline). Returns whether the status bit followed.
    unsafe fn set_periodic_schedule(&mut self, on: bool) -> bool {
        let cmd = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
        let want = if on { cmd | CMD_PSE } else { cmd & !CMD_PSE };
        let _ = mmio_write32(self.op + OP_USBCMD, want);
        wait_bounded(|| {
            let sts = mmio_read32(self.op + OP_USBSTS).unwrap_or(0);
            ((sts & STS_PSS) != 0) == on
        })
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
        // PROBE-8 / metal finding: EP0 control transfers run on the PERIODIC engine. The async
        // engine on this Panther Point master-aborts its very first schedule fetch in every
        // configuration tried (heap + static DMA, active-H-QH + Linux-shaped dummy-head ring,
        // post-HCRESET, VT-d off, BME on — probes 1-7), while the periodic engine DMAs the same
        // pool cleanly. EHCI QHs are engine-agnostic (4.10) — a control QH executes identically
        // from the frame list; only the service cadence differs (S-mask-paced instead of
        // continuous). HS targets get S-mask 0xFF (every µframe → ~1 ms per control transfer);
        // FS/LS-behind-TT keep the split masks (SSPLIT µframe 0, CSPLITs 2-4). The async ring
        // stays programmed-but-disabled (ASE is never set).
        // Periodic-QH discipline (probe-14c: RL must be 0 on periodic QHs — the RL=4 async
        // idiom made the QH invisible to the periodic scheduler; S-mask 0x01 is the exact
        // shape every passing smoke used).
        let qh = self.async_qh;
        let mut chars = (t.addr as u32)
            | t.eps
            | QH_DTC
            | ((t.mps0 as u32) << QH_MPS_SHIFT);
        if t.eps != QH_EPS_HIGH {
            chars |= QH_CTL_EP;
        }
        (*qh).ep_chars = chars;
        let masks = if t.eps == QH_EPS_HIGH {
            0x01 << QH_SMASK_SHIFT
        } else {
            (0x01 << QH_SMASK_SHIFT) | (0x1C << QH_CMASK_SHIFT)
        };
        (*qh).ep_caps = QH_MULT1
            | masks
            | ((t.hub_addr as u32) << QH_HUBADDR_SHIFT)
            | ((t.hub_port as u32) << QH_PORT_SHIFT);

        // PROBE-14 / metal finding: OVERLAY-DIRECT transactions. Across 13 metal probes the
        // one operation never seen to succeed — and present in every failure — is the qTD
        // fetch → overlay load, the controller's only multi-dword BURST WRITE (burst reads,
        // dword token write-backs, payload reads, and live-port transactions all passed the
        // smoke battery). So no qTD is ever handed to the controller: software pre-loads the
        // QH overlay with each stage's token/buffer (exactly the shape every passing smoke
        // used) and runs SETUP / DATA / STATUS as three sequential overlay transactions.
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

        // Mode selection: QEMU's hcd-ehci only executes fetched qTDs (it ignores software-
        // primed overlays), while this metal HSEs on the qTD fetch. Chain mode runs first;
        // an HSE'd chain transfer flips the controller to overlay-direct permanently and
        // retries (the failed SETUP never reached the wire — clean retry).
        if !self.overlay_mode {
            // Chain mode; an HSE propagates to the caller, which must FULLY re-init the
            // controller (HCRESET — an HSE'd controller is wedged; RS alone does not
            // recover it, probe-14 finding) and set overlay_mode before retrying.
            return self.chain_txn(t, bm_req, b_req, w_length, dir_in);
        }

        // Three overlay-direct stages. DT: SETUP=0, DATA starts 1 (controller maintains the
        // toggle in the overlay token across packets within a stage), STATUS=1 opposite PID.
        self.overlay_txn(bm_req, b_req, "SETUP", QTD_PID_SETUP, 8, self.setup_buf_phys, t.addr)?;
        let mut got = 0u32;
        if w_length > 0 {
            let data_pid = if dir_in { QTD_PID_IN } else { QTD_PID_OUT };
            got = self.overlay_txn(
                bm_req, b_req, "DATA", data_pid | QTD_DT, w_length as u32, self.data_buf_phys, t.addr,
            )?;
        }
        let status_pid = if w_length == 0 || !dir_in { QTD_PID_IN } else { QTD_PID_OUT };
        self.overlay_txn(bm_req, b_req, "STATUS", status_pid | QTD_DT, 0, 0, t.addr)?;
        Ok(got)
    }

    /// Chain-mode EP0 transfer (the standard qTD-chain shape; QEMU's model requires it). On a
    /// Host System Error the caller switches to overlay-direct. Returns bytes transferred.
    unsafe fn chain_txn(
        &mut self,
        t: &Target,
        bm_req: u8,
        b_req: u8,
        w_length: u16,
        dir_in: bool,
    ) -> Result<u32, &'static str> {
        let qh = self.async_qh;
        let (setup, data, status) = (self.qtd_setup, self.qtd_data, self.qtd_status);
        let status_pid = if w_length == 0 || !dir_in { QTD_PID_IN } else { QTD_PID_OUT };
        write_qtd(status, PTR_TERMINATE, status_pid | QTD_DT | QTD_IOC, 0, 0);
        let first_after_setup = if w_length > 0 {
            let data_pid = if dir_in { QTD_PID_IN } else { QTD_PID_OUT };
            write_qtd(data, self.qtd_status_phys as u32, data_pid | QTD_DT, w_length as u32, self.data_buf_phys);
            self.qtd_data_phys as u32
        } else {
            self.qtd_status_phys as u32
        };
        write_qtd(setup, first_after_setup, QTD_PID_SETUP, 8, self.setup_buf_phys);

        (*qh).overlay[1] = PTR_TERMINATE;
        core::ptr::write_volatile(&mut (*qh).overlay[2], 0);
        core::ptr::write_volatile(&mut (*qh).overlay[0], self.qtd_setup_phys as u32);
        let old_head = core::ptr::read_volatile(self.frame_list);
        (*qh).horiz = old_head;
        for i in 0..1024 {
            core::ptr::write_volatile(self.frame_list.add(i), (self.qh_phys as u32) | PTR_TYPE_QH);
        }
        let _ = self.set_periodic_schedule(true);
        let done = wait_bounded(|| {
            let st = core::ptr::read_volatile(&(*status).token);
            if st & QTD_ACTIVE == 0 {
                return true;
            }
            let hse = mmio_read32(self.op + OP_USBSTS).unwrap_or(0) & STS_HSE != 0;
            hse || (core::ptr::read_volatile(&(*setup).token) & QTD_HALTED != 0)
                || (w_length > 0 && core::ptr::read_volatile(&(*data).token) & QTD_HALTED != 0)
        });
        for i in 0..1024 {
            core::ptr::write_volatile(self.frame_list.add(i), old_head);
        }
        if !self.periodic_on {
            let _ = self.set_periodic_schedule(false);
        }
        (*qh).horiz = PTR_TERMINATE;
        core::ptr::write_volatile(&mut (*qh).overlay[0], PTR_TERMINATE);

        let sts = mmio_read32(self.op + OP_USBSTS).unwrap_or(0);
        if sts & STS_HSE != 0 {
            // Leave the HSE LATCHED: the caller's quiesce path keys its full HCRESET off it
            // (probe-14b: acking here left the controller wedged and the re-reset PR stuck).
            return Err("hse");
        }
        if !done {
            serial_println!(
                ":: EHCI-HID: [{}] STOP-NOTE EP0 chain timeout addr={} req={:#04x}/{:#04x} setup-token={:#010x} — not forced ::",
                self.idx, t.addr, bm_req, b_req,
                core::ptr::read_volatile(&(*setup).token)
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
                    ":: EHCI-HID: [{}] EP0 chain {} error addr={} req={:#04x}/{:#04x} token={:#010x} (halted/xact — likely STALL) ::",
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

    /// One overlay-direct transaction stage on the (already retargeted) work QH: software
    /// writes the token/buffer straight into the overlay — the controller never fetches a qTD
    /// (the burst-write class 13 metal probes indicted). Splices the QH into the frame list,
    /// runs the periodic engine, bounded-waits on the token, unsplices. Returns bytes done.
    unsafe fn overlay_txn(
        &mut self,
        bm_req: u8,
        b_req: u8,
        stage: &str,
        pid_dt: u32,
        len: u32,
        buf_phys: u64,
        addr: u8,
    ) -> Result<u32, &'static str> {
        let qh = self.async_qh;
        (*qh).current_qtd = 0;
        (*qh).overlay[0] = PTR_TERMINATE;
        (*qh).overlay[1] = PTR_TERMINATE;
        (*qh).overlay[3] = buf_phys as u32;
        (*qh).overlay[4] = 0;
        core::ptr::write_volatile(
            &mut (*qh).overlay[2],
            QTD_ACTIVE | QTD_CERR3 | (len << QTD_TOTAL_SHIFT) | pid_dt | QTD_IOC,
        );
        let old_head = core::ptr::read_volatile(self.frame_list);
        (*qh).horiz = old_head;
        for i in 0..1024 {
            core::ptr::write_volatile(self.frame_list.add(i), (self.qh_phys as u32) | PTR_TYPE_QH);
        }
        let pss_on = self.set_periodic_schedule(true);
        let done = wait_bounded(|| {
            core::ptr::read_volatile(&(*qh).overlay[2]) & QTD_ACTIVE == 0
        });
        for i in 0..1024 {
            core::ptr::write_volatile(self.frame_list.add(i), old_head);
        }
        let pss_off = if self.periodic_on {
            true // interrupt endpoints own the periodic schedule — leave it running
        } else {
            self.set_periodic_schedule(false)
        };
        (*qh).horiz = PTR_TERMINATE;
        let tok = core::ptr::read_volatile(&(*qh).overlay[2]);

        if !done {
            serial_println!(
                ":: EHCI-HID: [{}] STOP-NOTE EP0 {} timeout addr={} req={:#04x}/{:#04x} token={:#010x} USBCMD={:#010x} USBSTS={:#010x} PSS on={} off={} — not forced ::",
                self.idx, stage, addr, bm_req, b_req, tok,
                mmio_read32(self.op + OP_USBCMD).unwrap_or(0),
                mmio_read32(self.op + OP_USBSTS).unwrap_or(0),
                pss_on, pss_off
            );
            return Err("timeout");
        }
        if tok & QTD_ERR_MASK != 0 {
            serial_println!(
                ":: EHCI-HID: [{}] EP0 {} error addr={} req={:#04x}/{:#04x} token={:#010x} (halted/xact — likely STALL) ::",
                self.idx, stage, addr, bm_req, b_req, tok
            );
            return Err("stall");
        }
        Ok(len.saturating_sub((tok >> QTD_TOTAL_SHIFT) & 0x7FFF))
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

    /// Probe-13: the pass-3 smoke shape (pre-loaded 0-length IN to a bogus address — pure
    /// pipeline + write-back, no real listener) re-run while a port is ENABLED. Every
    /// pre-enable smoke passed and every live-port transfer HSE'd; this isolates "DMA while a
    /// port is active" as the failing ingredient. Returns whether HSE fired (and restores
    /// running state if it did).
    unsafe fn live_port_smoke(&mut self, tag: &str) -> bool {
        let qh = self.async_qh;
        (*qh).horiz = PTR_TERMINATE;
        (*qh).ep_chars = 42 | (1 << 8) | QH_DTC | QH_EPS_HIGH | (8 << QH_MPS_SHIFT);
        (*qh).ep_caps = QH_MULT1 | 0x01;
        (*qh).overlay[0] = PTR_TERMINATE;
        (*qh).overlay[1] = PTR_TERMINATE;
        core::ptr::write_volatile(&mut (*qh).overlay[2], QTD_ACTIVE | (1 << 10) | QTD_PID_IN);
        for i in 0..1024 {
            core::ptr::write_volatile(self.frame_list.add(i), (self.qh_phys as u32) | PTR_TYPE_QH);
        }
        let _ = self.set_periodic_schedule(true);
        settle_ms(5);
        let sts = mmio_read32(self.op + OP_USBSTS).unwrap_or(0);
        let _ = self.set_periodic_schedule(false);
        for i in 0..1024 {
            core::ptr::write_volatile(self.frame_list.add(i), PTR_TERMINATE);
        }
        let tok = core::ptr::read_volatile(&(*qh).overlay[2]);
        (*qh).overlay[0] = PTR_TERMINATE;
        serial_println!(
            ":: EHCI-HID: [{}] live-port smoke ({}): USBSTS={:#010x} HSE={} HCHalted={} post-token={:#010x} == witness ::",
            self.idx, tag, sts, (sts >> 4) & 1, (sts >> 12) & 1, tok
        );
        let hse = sts & STS_HSE != 0;
        if hse {
            let _ = mmio_write32(self.op + OP_USBSTS, sts & STS_RW1C);
            let cmd = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
            let _ = mmio_write32(self.op + OP_USBCMD, (cmd & !CMD_PSE) | CMD_RS);
            let _ = wait_bounded(|| {
                mmio_read32(self.op + OP_USBSTS).unwrap_or(STS_HCHALTED) & STS_HCHALTED == 0
            });
        }
        hse
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
        if self.int_next >= MAX_INT_EPS {
            serial_println!(
                ":: EHCI-HID: [{}] static int-EP pool exhausted ({}) — endpoint skipped ::",
                self.idx, MAX_INT_EPS
            );
            return;
        }
        let slot = &mut (*self.pool()).int_slots[self.int_next];
        let (qh, qtd, buf) = (
            &mut slot.qh as *mut Qh,
            &mut slot.qtd as *mut Qtd,
            slot.buf.0.as_mut_ptr(),
        );
        let (Some(qh_phys), Some(qtd_phys), Some(buf_phys)) = (
            phys_of(qh, 32),
            phys_of(qtd, 32),
            phys_of(buf, 64),
        ) else {
            serial_println!(
                ":: EHCI-HID: [{}] STOP-NOTE int-EP slot failed the phys/alignment contract — endpoint skipped ::",
                self.idx
            );
            return;
        };
        self.int_next += 1;

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

        // First transfer of a freshly-configured interrupt endpoint is DATA0, armed in
        // whichever mode enumeration settled on (overlay-direct on this metal, qTD-chain on
        // QEMU — see Controller::overlay_mode).
        if self.overlay_mode {
            let _ = (qtd, qtd_phys); // slot storage retained; the controller never sees it
            (*qh).current_qtd = 0;
            (*qh).overlay[0] = PTR_TERMINATE;
            (*qh).overlay[1] = PTR_TERMINATE;
            (*qh).overlay[3] = buf_phys as u32;
            (*qh).overlay[4] = 0;
            core::ptr::write_volatile(
                &mut (*qh).overlay[2],
                QTD_ACTIVE | QTD_CERR3 | ((mps as u32) << QTD_TOTAL_SHIFT) | QTD_PID_IN | QTD_IOC,
            );
        } else {
            write_qtd(qtd, PTR_TERMINATE, QTD_PID_IN | QTD_IOC, mps as u32, buf_phys);
            (*qh).overlay[1] = PTR_TERMINATE;
            (*qh).overlay[2] = 0;
            (*qh).overlay[0] = qtd_phys as u32;
        }

        // Link: new QH points at the current chain head, then every frame-list entry points at
        // the new QH (entries were Terminate or the old head — both cases are one word).
        let fl = self.frame_list;
        let old_head = core::ptr::read_volatile(fl);
        (*qh).horiz = old_head;
        for i in 0..1024 {
            core::ptr::write_volatile(fl.add(i), (qh_phys as u32) | PTR_TYPE_QH);
        }

        if !self.periodic_on {
            let cmd = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
            let _ = mmio_write32(self.op + OP_USBCMD, cmd | CMD_PSE);
            self.periodic_on = true;
        }

        self.int_eps.push(IntEp {
            qh,
            qtd,
            qtd_phys,
            buf,
            buf_phys,
            mps,
            toggle: false,
            is_kbd,
            is_rel_mouse: is_rel,
            reports: 0,
            dead: false,
        });
    }

    /// The controller's static DMA pool (index-bound checked at construction).
    fn pool(&self) -> *mut qh::DmaPool {
        unsafe { &mut DMA_POOLS[self.idx] as *mut qh::DmaPool }
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
        let om = self.overlay_mode;
        for e in self.int_eps.iter_mut() {
            if e.dead {
                continue;
            }
            let tok = if om {
                core::ptr::read_volatile(&(*e.qh).overlay[2])
            } else {
                core::ptr::read_volatile(&(*e.qtd).token)
            };
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
            // Re-arm in the controller's transfer mode: flip the software toggle (QH_DTC —
            // the toggle lives here), then either rewrite the overlay in place
            // (overlay-direct; no qTD fetch — this metal) or refresh + point at the qTD.
            e.toggle = !e.toggle;
            let dt = if e.toggle { QTD_DT } else { 0 };
            if om {
                (*e.qh).overlay[0] = PTR_TERMINATE;
                (*e.qh).overlay[1] = PTR_TERMINATE;
                (*e.qh).overlay[3] = e.buf_phys as u32;
                (*e.qh).overlay[4] = 0;
                core::ptr::write_volatile(
                    &mut (*e.qh).overlay[2],
                    QTD_ACTIVE | QTD_CERR3 | ((e.mps as u32) << QTD_TOTAL_SHIFT) | QTD_PID_IN | QTD_IOC | dt,
                );
            } else {
                write_qtd(e.qtd, PTR_TERMINATE, QTD_PID_IN | QTD_IOC | dt, e.mps as u32, e.buf_phys);
                (*e.qh).overlay[1] = PTR_TERMINATE;
                core::ptr::write_volatile(&mut (*e.qh).overlay[2], 0);
                core::ptr::write_volatile(&mut (*e.qh).overlay[0], e.qtd_phys as u32);
            }
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

/// Probe-3/4 evidence: dump VT-d (DMAR) state, READ-ONLY. Probe 3 showed both EHCI functions'
/// first DMA fetch master-aborting (USBSTS HSE + halt) on a freshly HCRESET controller while
/// xHCI DMAs the same heap fine — the signature of per-function DMA blocking, i.e. an IOMMU
/// left translating by Apple EFI (which drove the pre-boot keyboard over these very
/// functions). This dump answers it with registers instead of a theory: DMAR present? each
/// DRHD's translation-enable (GSTS.TES), fault status (FSTS), and any latched fault-recording
/// entries — which name the faulting source BDF, reason, and address. Zero writes.
unsafe fn dmar_report() {
    let Some(dmar) = crate::arch::x86_64::acpi::find_acpi_table(b"DMAR") else {
        serial_println!(":: EHCI-HID: DMAR: no ACPI DMAR table — VT-d not described; IOMMU theory falsified ::");
        return;
    };
    let len = mmio_read32(dmar + 4).unwrap_or(0) as u64; // SDT header length
    let haw = mmio_read32(dmar + 36).unwrap_or(0);
    serial_println!(
        ":: EHCI-HID: DMAR present @ {:#x} len={} host-addr-width={} flags={:#04x} ::",
        dmar, len, (haw & 0xFF) + 1, (haw >> 8) & 0xFF
    );
    // Remapping structures start at offset 48: u16 type, u16 length; type 0 = DRHD with the
    // remapping-unit register base at +8.
    let mut off = 48u64;
    let mut unit = 0;
    while off + 4 <= len {
        let head = mmio_read32(dmar + off).unwrap_or(0);
        let (typ, slen) = (head & 0xFFFF, (head >> 16) & 0xFFFF);
        if slen == 0 {
            break;
        }
        if typ == 0 {
            let base = (mmio_read32(dmar + off + 8).unwrap_or(0) as u64)
                | ((mmio_read32(dmar + off + 12).unwrap_or(0) as u64) << 32);
            let cap_lo = mmio_read32(base + 0x08).unwrap_or(0);
            let cap_hi = mmio_read32(base + 0x0C).unwrap_or(0);
            let gsts = mmio_read32(base + 0x1C).unwrap_or(0);
            let fsts = mmio_read32(base + 0x34).unwrap_or(0);
            let cap = (cap_lo as u64) | ((cap_hi as u64) << 32);
            let fro = ((cap >> 24) & 0x3FF) * 16; // fault-recording offset, 128-bit units
            let nfr = ((cap >> 40) & 0xFF) + 1;
            serial_println!(
                ":: EHCI-HID: DMAR DRHD[{}] base={:#x} GSTS={:#010x} (TES={}) FSTS={:#010x} CAP={:#018x} NFR={} ::",
                unit, base, gsts, (gsts >> 31) & 1, fsts, cap, nfr
            );
            // Probe-9: Protected Memory Regions — the one DMA gate that operates with
            // translation OFF (VT-d 10.4.16-20). PMEN.EPM=1 with PLMR/PHMR covering DRAM
            // blocks device DMA exactly like the observed master aborts; every OS clears it
            // at IOMMU handoff. Read-only dump: enable/status + both regions' bounds.
            let pmen = mmio_read32(base + 0x64).unwrap_or(0);
            serial_println!(
                ":: EHCI-HID: DMAR DRHD[{}] PMEN={:#010x} (EPM={} PRS={}) PLMR {:#010x}..{:#010x} PHMR {:#010x}_{:08x}..{:#010x}_{:08x} == witness ::",
                unit, pmen, (pmen >> 31) & 1, pmen & 1,
                mmio_read32(base + 0x68).unwrap_or(0),
                mmio_read32(base + 0x6C).unwrap_or(0),
                mmio_read32(base + 0x74).unwrap_or(0),
                mmio_read32(base + 0x70).unwrap_or(0),
                mmio_read32(base + 0x7C).unwrap_or(0),
                mmio_read32(base + 0x78).unwrap_or(0)
            );
            // Device scopes: which BDFs this unit owns (why xHCI may be exempt). Raw bytes of
            // the DRHD structure past the 16-byte header: scope entries are (type, len, rsvd,
            // enum-id, start-bus, path pairs...).
            let mut soff = off + 16;
            while soff < off + slen as u64 {
                let d0 = mmio_read32(dmar + soff).unwrap_or(0);
                let d1 = mmio_read32(dmar + soff + 4).unwrap_or(0);
                let slen2 = (d0 >> 8) & 0xFF;
                serial_println!(
                    ":: EHCI-HID: DMAR DRHD[{}] scope: type={} len={} start-bus={} path[0]={:#04x},{:#04x} ::",
                    unit, d0 & 0xFF, slen2, (d0 >> 24) & 0xFF, d1 & 0xFF, (d1 >> 8) & 0xFF
                );
                if slen2 == 0 {
                    break;
                }
                soff += slen2 as u64;
            }
            for i in 0..nfr.min(4) {
                let fr = base + fro + i * 16;
                let lo = (mmio_read32(fr).unwrap_or(0) as u64)
                    | ((mmio_read32(fr + 4).unwrap_or(0) as u64) << 32);
                let hi = (mmio_read32(fr + 8).unwrap_or(0) as u64)
                    | ((mmio_read32(fr + 12).unwrap_or(0) as u64) << 32);
                if hi >> 63 != 0 {
                    let sid = hi & 0xFFFF;
                    serial_println!(
                        ":: EHCI-HID: DMAR DRHD[{}] FAULT[{}]: source {:02x}:{:02x}.{} reason={:#04x} addr={:#x} == witness ::",
                        unit, i, (sid >> 8) & 0xFF, (sid >> 3) & 0x1F, sid & 0x7,
                        (hi >> 32) & 0xFF, lo & !0xFFF
                    );
                }
            }
            unit += 1;
        }
        off += slen as u64;
    }
    if unit == 0 {
        serial_println!(":: EHCI-HID: DMAR table has no DRHD units ::");
    }
}

/// Probe-5 evidence, read-only: decode the function's PCI STATUS error bits (Received Master
/// Abort / Received Target Abort / Signaled Target Abort / parity) — these name what the
/// fabric did to the controller's DMA request — then dump all 64 config dwords for offline
/// analysis against the Intel 7-series datasheet.
unsafe fn pci_evidence(bus: u8, dev: u8, func: u8, idx: usize) {
    let sc = read_config_32(bus, dev, func, 0x04);
    let status = (sc >> 16) as u16;
    serial_println!(
        ":: EHCI-HID: [{}] PCI STATUS={:#06x} RMA={} RTA={} STA={} SSE={} DPE={} MDPE={} == witness ::",
        idx, status,
        (status >> 13) & 1, // Received Master Abort
        (status >> 12) & 1, // Received Target Abort
        (status >> 11) & 1, // Signaled Target Abort
        (status >> 14) & 1, // Signaled System Error
        (status >> 15) & 1, // Detected Parity Error
        (status >> 8) & 1   // Master Data Parity Error
    );
    for row in 0..8u8 {
        let base = row * 32;
        serial_println!(
            ":: EHCI-HID: [{}] CFG {:#04x}: {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} ::",
            idx, base,
            read_config_32(bus, dev, func, base),
            read_config_32(bus, dev, func, base + 4),
            read_config_32(bus, dev, func, base + 8),
            read_config_32(bus, dev, func, base + 12),
            read_config_32(bus, dev, func, base + 16),
            read_config_32(bus, dev, func, base + 20),
            read_config_32(bus, dev, func, base + 24),
            read_config_32(bus, dev, func, base + 28)
        );
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
    // Probe-13 (b), Peter-approved 2026-07-17: the RCBA CG (clock gating) base + a one-shot
    // flag — the clear fires only if the live-port smoke actually HSEs.
    let rcba = unsafe { (read_config_32(0, 31, 0, 0xF0) as u64) & !0x3FFF };
    let mut cg_cleared = false;

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
                    if h.addr64 != 0 {
                        serial_println!(
                            ":: EHCI-HID: [{}] note: controller advertises 64-bit addressing; CTRLDSSEGMENT pinned to 0 (all DMA < 4 GiB) ::",
                            idx
                        );
                    }
                    if idx >= MAX_CONTROLLERS {
                        serial_println!(
                            ":: EHCI-HID: [{}] more EHCI functions than static DMA pools ({}) — skipped ::",
                            idx, MAX_CONTROLLERS
                        );
                        continue;
                    }
                    let pool = &mut DMA_POOLS[idx];
                    let (
                        Some(fl_phys),
                        Some(head_phys),
                        Some(qh_phys),
                        Some(su_phys),
                        Some(da_phys),
                        Some(st_phys),
                        Some(sb_phys),
                        Some(db_phys),
                    ) = (
                        phys_of(pool.frame_list.as_ptr(), 4096),
                        phys_of(&pool.qh_head, 32),
                        phys_of(&pool.qh_ctrl, 32),
                        phys_of(&pool.qtd_setup, 32),
                        phys_of(&pool.qtd_data, 32),
                        phys_of(&pool.qtd_status, 32),
                        phys_of(&pool.setup_buf, 64),
                        phys_of(&pool.data_buf, 64),
                    )
                    else {
                        serial_println!(
                            ":: EHCI-HID: [{}] STOP-NOTE static DMA pool failed the phys/alignment contract — controller skipped ::",
                            idx
                        );
                        continue;
                    };
                    let mut c = Controller {
                        idx,
                        op: h.op,
                        bus,
                        dev,
                        func,
                        async_head: &mut pool.qh_head as *mut Qh,
                        head_phys,
                        async_qh: &mut pool.qh_ctrl as *mut Qh,
                        qh_phys,
                        qtd_setup: &mut pool.qtd_setup as *mut Qtd,
                        qtd_setup_phys: su_phys,
                        qtd_data: &mut pool.qtd_data as *mut Qtd,
                        qtd_data_phys: da_phys,
                        qtd_status: &mut pool.qtd_status as *mut Qtd,
                        qtd_status_phys: st_phys,
                        setup_buf: pool.setup_buf.0.as_mut_ptr(),
                        setup_buf_phys: sb_phys,
                        data_buf: pool.data_buf.0.as_mut_ptr(),
                        data_buf_phys: db_phys,
                        frame_list: pool.frame_list.as_mut_ptr(),
                        frame_list_phys: fl_phys,
                        int_next: 0,
                        periodic_on: false,
                        overlay_mode: false,
                        next_addr: 1,
                        int_eps: Vec::new(),
                    };
                    // Firmware-stale detection BEFORE any schedule programming: probe 2 showed
                    // Apple EFI leaves PSE=1 behind (its pre-boot keyboard), which HSE-halts
                    // the controller once RS runs over the reclaimed frame list. After the
                    // pre-approved HCRESET the controller is at defaults (halted, CF=0) — the
                    // bases are then programmed in the halted state (textbook), RS re-started,
                    // and CF re-routed via the shared wake_route.
                    let did_reset = c.quiesce_if_firmware_stale();
                    c.init_schedules();
                    if did_reset {
                        let cmd = mmio_read32(h.op + OP_USBCMD).unwrap_or(0);
                        let _ = mmio_write32(h.op + OP_USBCMD, cmd | CMD_RS);
                        let running = wait_bounded(|| {
                            mmio_read32(h.op + OP_USBSTS).unwrap_or(STS_HCHALTED)
                                & STS_HCHALTED
                                == 0
                        });
                        serial_println!(
                            ":: EHCI-HID: [{}] post-HCRESET restart: RS=1 running={} ::",
                            idx, running
                        );
                    }
                    ehci_scout::wake_route(&h, idx);
                    // Clear any latched RW1C status (incl. a pre-existing HSE) before the
                    // first transfer, so a fresh error is unambiguously ours.
                    if let Some(sts) = mmio_read32(h.op + OP_USBSTS) {
                        if sts & STS_RW1C != 0 {
                            let _ = mmio_write32(h.op + OP_USBSTS, sts & STS_RW1C);
                        }
                    }
                    // Probe-6/10 discriminators, two 5 ms periodic smoke passes:
                    //  (1) all-Terminate frame list — one 4-byte frame-list read per frame,
                    //      the simplest upstream read (probe-6: PASSED on metal).
                    //  (2) frame list -> one INACTIVE QH (halted token, nothing to execute) —
                    //      forces the 32/48-byte burst QH fetch but requires ZERO write-back.
                    //      Clean => burst reads fine, the wall is DMA WRITES; HSE => burst
                    //      reads themselves are gated (4-byte reads pass regardless).
                    // Pass 3 (probe-11): the WRITE discriminator. The QH's overlay is
                    // PRE-LOADED with an active zero-length interrupt-IN to a nonexistent
                    // address (no qTD fetch, no overlay load) — the controller executes the
                    // wire transaction (nobody answers, XactErr) and must then WRITE the
                    // token back. HSE here = upstream DMA writes are gated, QED (probes 1-10
                    // proved every read class passes). A written-back token instead would
                    // falsify the write theory on the spot.
                    // Passes 4/5 (probe-12): payload-read isolation. 4 = zero-length OUT to
                    // the bogus address (controls for OUT direction, still no buffer);
                    // 5 = 8-byte OUT (forces the controller to READ the payload buffer to
                    // transmit — the ONE access class every failing SETUP starts with and
                    // pass 3 never exercised). HSE on 5 alone names the wall: payload-read
                    // DMA gated while structure reads/writes + wire transactions all pass.
                    for (pass, arm_qh) in [(1u32, false), (2, true), (3, true), (4, true), (5, true)] {
                        if arm_qh {
                            let qh = c.async_qh; // idle work QH, reused as the inert target
                            let self_buf = c.setup_buf_phys as u32;
                            (*qh).horiz = PTR_TERMINATE;
                            (*qh).ep_chars = (42) // bogus device address, EP 1
                                | (1 << 8)
                                | QH_DTC
                                | QH_EPS_HIGH
                                | (8 << QH_MPS_SHIFT);
                            (*qh).ep_caps = QH_MULT1 | 0x01; // S-mask µframe 0
                            (*qh).overlay[0] = PTR_TERMINATE;
                            (*qh).overlay[1] = PTR_TERMINATE;
                            (*qh).overlay[2] = match pass {
                                2 => QTD_HALTED, // inactive — fetch, no work, no write
                                // Pre-loaded active tokens, CERR=1 (fail fast), bogus addr:
                                3 => QTD_ACTIVE | (1 << 10) | QTD_PID_IN, // 0-len IN
                                4 => QTD_ACTIVE | (1 << 10) | QTD_PID_OUT, // 0-len OUT, no buffer
                                _ => {
                                    // 8-byte OUT — forces the payload buffer READ.
                                    (*qh).overlay[3] = self_buf; // buffer page 0 = setup_buf
                                    QTD_ACTIVE | (1 << 10) | QTD_PID_OUT | (8 << QTD_TOTAL_SHIFT)
                                }
                            };
                            for i in 0..1024 {
                                core::ptr::write_volatile(
                                    c.frame_list.add(i),
                                    (c.qh_phys as u32) | PTR_TYPE_QH,
                                );
                            }
                        }
                        let cmd = mmio_read32(h.op + OP_USBCMD).unwrap_or(0);
                        let _ = mmio_write32(h.op + OP_USBCMD, cmd | CMD_PSE);
                        ehci_scout::settle_ms(5);
                        let sts = mmio_read32(h.op + OP_USBSTS).unwrap_or(0);
                        let _ = mmio_write32(h.op + OP_USBCMD, cmd & !CMD_PSE);
                        let _ = wait_bounded(|| {
                            mmio_read32(h.op + OP_USBSTS).unwrap_or(0) & (1 << 14) == 0
                        });
                        serial_println!(
                            ":: EHCI-HID: [{}] periodic DMA smoke pass {} ({}): USBSTS={:#010x} HSE={} HCHalted={} post-token={:#010x} == witness ::",
                            idx, pass,
                            match pass {
                                1 => "empty frame list",
                                2 => "inactive-QH burst fetch, zero writeback",
                                3 => "preloaded 0-len IN bogus -> forced token WRITE-back",
                                4 => "preloaded 0-len OUT bogus (no buffer)",
                                _ => "preloaded 8-byte OUT bogus -> forced PAYLOAD READ",
                            },
                            sts, (sts >> 4) & 1, (sts >> 12) & 1,
                            core::ptr::read_volatile(&(*c.async_qh).overlay[2])
                        );
                        if arm_qh {
                            for i in 0..1024 {
                                core::ptr::write_volatile(c.frame_list.add(i), PTR_TERMINATE);
                            }
                        }
                        if sts & STS_HSE != 0 {
                            // Ack + restart so later steps still report honestly.
                            let _ = mmio_write32(h.op + OP_USBSTS, sts & STS_RW1C);
                            let cmd2 = mmio_read32(h.op + OP_USBCMD).unwrap_or(0);
                            let _ = mmio_write32(h.op + OP_USBCMD, (cmd2 & !CMD_PSE) | CMD_RS);
                            let _ = wait_bounded(|| {
                                mmio_read32(h.op + OP_USBSTS).unwrap_or(STS_HCHALTED) & STS_HCHALTED == 0
                            });
                        }
                    }
                    for port in 0..h.n_ports {
                        let portsc = mmio_read32(h.op + OP_PORTSC0 + 4 * port as u64).unwrap_or(0);
                        if portsc & PORT_CCS == 0 || portsc & PORT_OWNER != 0 {
                            continue;
                        }
                        if c.reset_root_port(port) {
                            // Transport probe (probe-14): one bare chain-mode GET_DESCRIPTOR(8)
                            // to addr 0. An HSE means this silicon aborts the qTD-fetch burst
                            // write — flip to OVERLAY-DIRECT and FULLY re-init (an HSE'd
                            // controller is wedged; HCRESET, bases, RS, CF, port — all redone).
                            if !c.overlay_mode {
                                let probe_t = Target {
                                    addr: 0, mps0: 64, eps: QH_EPS_HIGH, hub_addr: 0, hub_port: 0,
                                };
                                if let Err("hse") = c.control(&probe_t, 0x80, 6, 0x0100, 0, 8, true) {
                                    c.overlay_mode = true;
                                    serial_println!(
                                        ":: EHCI-HID: [{}] qTD-fetch HSE — OVERLAY-DIRECT mode + full HCRESET re-init (probe-14 silicon finding) ::",
                                        idx
                                    );
                                    let _ = c.quiesce_if_firmware_stale(); // HSE latched -> resets
                                    c.init_schedules();
                                    let cmd = mmio_read32(h.op + OP_USBCMD).unwrap_or(0);
                                    let _ = mmio_write32(h.op + OP_USBCMD, cmd | CMD_RS);
                                    let _ = wait_bounded(|| {
                                        mmio_read32(h.op + OP_USBSTS).unwrap_or(STS_HCHALTED)
                                            & STS_HCHALTED == 0
                                    });
                                    ehci_scout::wake_route(&h, idx);
                                    if let Some(sts) = mmio_read32(h.op + OP_USBSTS) {
                                        if sts & STS_RW1C != 0 {
                                            let _ = mmio_write32(h.op + OP_USBSTS, sts & STS_RW1C);
                                        }
                                    }
                                    if !c.reset_root_port(port) {
                                        continue;
                                    }
                                }
                            }
                            let _ = (&cg_cleared, rcba); // probe-13 levers retired (smokes all passed)
                            c.enumerate_at_zero(QH_EPS_HIGH, 0, 0, 0);
                        }
                    }
                    ctrls.push(c);
                }
            }
        }
    }

    // Probe-4 evidence dump: VT-d state AFTER the enumeration attempts, so any DMA fault our
    // transfers raised is latched in the fault-recording registers (read-only).
    unsafe { dmar_report() };

    // Probe-5 evidence: PCI STATUS decode (received/signaled abort bits name the failure class
    // at the fabric level) + full config-space dump of each EHCI function for offline
    // comparison against the 7-series datasheet. Read-only.
    for c in ctrls.iter() {
        unsafe { pci_evidence(c.bus, c.dev, c.func, c.idx) };
    }
    unsafe {
        // Working-DMA reference: the xHCI function's config space (progIF 0x30), tagged [90+].
        for dev in 0u8..=31 {
            let class_reg = read_config_32(0, dev, 0, 0x08);
            if class_reg >> 8 == 0x0C0330 {
                pci_evidence(0, dev, 0, 90 + dev as usize);
            }
        }
        // PCH RCBA window, READ-ONLY: Function Disable / Backed-Up Control / clock gating —
        // the registers Apple EFI is most likely to have left gating the EHCI DMA engines.
        // RCBA base from the LPC bridge (0:31.0) config 0xF0 (bit 0 = enable).
        let rcba_reg = read_config_32(0, 31, 0, 0xF0);
        let rcba = (rcba_reg as u64) & !0x3FFF;
        serial_println!(
            ":: EHCI-HID: RCBA reg={:#010x} base={:#x} en={} ::",
            rcba_reg, rcba, rcba_reg & 1
        );
        if rcba_reg & 1 == 1 && rcba != 0 {
            // Probe-11: the chipset-config VIRTUAL CHANNEL block (V0/V1/VCp at the RCBA head)
            // — a misconfigured isoch/private channel would abort device WRITES specifically.
            for off in [0x0000u64, 0x0014, 0x0018, 0x001C, 0x0020, 0x0024, 0x0028, 0x0030] {
                if let Some(v) = mmio_read32(rcba + off) {
                    serial_println!(":: EHCI-HID: RCBA+{:#06x} = {:#010x} ::", off, v);
                }
            }
            for off in [0x3400u64, 0x3404, 0x3410, 0x3414, 0x3418, 0x341C, 0x3420, 0x3428, 0x342C, 0x3430, 0x3434] {
                if let Some(v) = mmio_read32(rcba + off) {
                    serial_println!(":: EHCI-HID: RCBA+{:#06x} = {:#010x} ::", off, v);
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
