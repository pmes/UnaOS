// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//! CHARTER: Kernel — driver (the radio is hardware on the EHCI controller; the link key is Holocron's: sealed as the record `bt/<addr12>` through Holocron's verbs, BTKEYSEAL B446)
//!
//! BTHID (rmbp-ledger B339) — a Bluetooth HID host over BR/EDR, written from the Bluetooth Core
//! specification (Vol 4 Part E: HCI commands and events; Vol 3 Part A: L2CAP; Vol 3 Part B: SDP;
//! Vol 3 Part C §5: Simple Secure Pairing) and the HID-over-Bluetooth profile (HIDP: PSM 0x11
//! control, PSM 0x13 interrupt, the 0xA1 DATA|Input transaction header, SET_PROTOCOL, the SDP
//! HIDDescriptorList attribute 0x0206).
//!
//! THE SHAPE. A child module of `drivers/ehci` because the radio is a device on the EHCI controller
//! and everything below the HCI packet — the EP0 command path (`bt_hci_send`), the event endpoint and
//! its reassembly (`bt_read_full_event`), the ACL bulk pipes (`bt_acl_txn`) — already exists there and
//! is reused, not copied. Above HCI it reuses the parent's HID path too: the pointer field map is
//! produced by the SAME `parse_report_descriptor` the EHCI-HID USB path uses, decoded by the same
//! `decode_report_pointer` / `decode_boot_keyboard`, and delivered into the same `pal` ring.
//!
//! NEVER HOLDING INPUT (rmbp-ledger A8, B137). After one short synchronous bring-up (bounded HCI
//! commands, run from the boot campaign's post-GUI drain) the stack is a state machine stepped once
//! per `service_ehci_hid` pass: one HCI command in flight, events read with a ZERO first-packet budget
//! (an idle endpoint costs one token read), ACL polled at most every 2 ms with a 250 us budget. The
//! state is taken with `try_lock`, so a shell verb holding it costs the pass nothing but a skip. Every
//! host-side wait carries a deadline; nothing at boot but the bounded 5 s reconnect of a bonded device,
//! and that only after the desktop.
//!
//! THE LINK KEY. A Holocron record `bt/<addr12>` sealed under the session user's ring ([`super::btkeyseal`],
//! BTKEYSEAL B446); the bond's non-secret facts are typed attributes on one object per bonded device under
//! `<home>/.config/unaos/bt/<addr12>` (`bt.keytype` Int, `bt.name` Str, `bt.class` Int, `bt.hiddesc` Blob
//! when it fits). A legacy plain `bt.linkkey` attribute is read once, sealed, and removed. All file I/O happens in [`store_service`], called from the
//! storage-ready passes in `main.rs` — never under the `EHCI_HID` lock (the holocron rule). Key bytes
//! never appear on the wire.

use super::*;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

// ── HCI opcodes (Vol 4 Part E §7) ───────────────────────────────────────────────────────────────
const H_INQUIRY: u16 = 0x0401;
const H_INQUIRY_CANCEL: u16 = 0x0402;
const H_CREATE_CONN: u16 = 0x0405;
const H_DISCONNECT: u16 = 0x0406;
const H_CREATE_CONN_CANCEL: u16 = 0x0408;
const H_ACCEPT_CONN: u16 = 0x0409;
const H_REJECT_CONN: u16 = 0x040A;
const H_LINK_KEY_REPLY: u16 = 0x040B;
const H_LINK_KEY_NEG: u16 = 0x040C;
const H_PIN_REPLY: u16 = 0x040D;
const H_PIN_NEG: u16 = 0x040E;
const H_AUTH_REQ: u16 = 0x0411;
const H_SET_ENC: u16 = 0x0413;
const H_RNR: u16 = 0x0419;
const H_RNR_CANCEL: u16 = 0x041A;
const H_IO_CAP_REPLY: u16 = 0x042B;
const H_CONFIRM_REPLY: u16 = 0x042C;
const H_CONFIRM_NEG: u16 = 0x042D;
const H_PASSKEY_NEG: u16 = 0x042F;
const H_OOB_NEG: u16 = 0x0433;
const H_IO_CAP_NEG: u16 = 0x0434;
const H_SET_EVENT_MASK: u16 = 0x0C01;
const H_RESET: u16 = 0x0C03;
const H_WRITE_LOCAL_NAME: u16 = 0x0C13;
const H_WRITE_PAGE_TIMEOUT: u16 = 0x0C18;
const H_WRITE_SCAN_ENABLE: u16 = 0x0C1A;
const H_WRITE_COD: u16 = 0x0C24;
const H_WRITE_INQUIRY_MODE: u16 = 0x0C45;
const H_WRITE_SSP_MODE: u16 = 0x0C56;
const H_READ_BUFFER_SIZE: u16 = 0x1005;
const H_READ_BD_ADDR: u16 = 0x1009;

// ── HCI events (Vol 4 Part E §7.7) ──────────────────────────────────────────────────────────────
const E_INQUIRY_COMPLETE: u8 = 0x01;
const E_INQUIRY_RESULT: u8 = 0x02;
const E_CONN_COMPLETE: u8 = 0x03;
const E_CONN_REQUEST: u8 = 0x04;
const E_DISCONN_COMPLETE: u8 = 0x05;
const E_AUTH_COMPLETE: u8 = 0x06;
const E_RNR_COMPLETE: u8 = 0x07;
const E_ENC_CHANGE: u8 = 0x08;
const E_CMD_COMPLETE: u8 = 0x0E;
const E_CMD_STATUS: u8 = 0x0F;
const E_NOCP: u8 = 0x13;
const E_PIN_REQUEST: u8 = 0x16;
const E_LINK_KEY_REQUEST: u8 = 0x17;
const E_LINK_KEY_NOTIFY: u8 = 0x18;
const E_INQUIRY_RESULT_RSSI: u8 = 0x22;
const E_EXT_INQUIRY_RESULT: u8 = 0x2F;
const E_IO_CAP_REQUEST: u8 = 0x31;
const E_IO_CAP_RESPONSE: u8 = 0x32;
const E_CONFIRM_REQUEST: u8 = 0x33;
const E_PASSKEY_REQUEST: u8 = 0x34;
const E_OOB_REQUEST: u8 = 0x35;
const E_SSP_COMPLETE: u8 = 0x36;
const E_PASSKEY_NOTIFY: u8 = 0x3B;

/// Reset default (bits 0..44) + Extended Inquiry Result (46) + the SSP family IO Capability
/// Request/Response, User Confirmation/Passkey Request, Remote OOB, Simple Pairing Complete (48..53)
/// + Link Supervision Timeout Changed (55) + User Passkey Notification (58) + Remote Host Supported
/// Features (60). LE Meta (61) is NOT set: this stack is BR/EDR.
const EVENT_MASK: u64 = 0x14BF_5FFF_FFFF_FFFF;
/// The General Inquiry Access Code, LAP 0x9E8B33, wire order.
const GIAC: [u8; 3] = [0x33, 0x8B, 0x9E];
/// Inquiry_Length 0x08 x 1.28 s = 10.24 s; the host gives the controller 11 s before it cancels.
const INQUIRY_LEN: u8 = 0x08;
const INQUIRY_HOST_MS: u64 = 11_000;
/// One Remote_Name_Request is a page; the controller bounds it by Page_Timeout (5 s, written at
/// bring-up). The host's own deadline is 6 s, then Remote_Name_Request_Cancel.
const RNR_HOST_MS: u64 = 6_000;
/// Page_Timeout 0x1F40 slots x 0.625 ms = 5000 ms — the reconnect bound the brief names.
const PAGE_TIMEOUT_SLOTS: u16 = 0x1F40;
const PAGE_HOST_MS: u64 = 7_000;
const SETUP_MS: u64 = 30_000;
const CONFIRM_MS: u64 = 60_000;
const CHAN_MS: u64 = 10_000;
const RECONNECT_MS: u64 = 5_000;
/// A `bt pair` arms an inbound-pairing window for the named address as well, so a device that pages
/// US to pair (some keyboards do) is accepted.
const PAIR_WINDOW_MS: u64 = 120_000;
/// IO capability DisplayYesNo; general bonding, MITM requested (the controller falls back to just
/// works when the peer has no IO, and the host accepts that for a pointing device).
const IO_CAP: u8 = 0x01;
const AUTH_REQ: u8 = 0x05;
/// Our L2CAP receive MTU.
const L2_MTU: u16 = 672;
const PSM_SDP: u16 = 0x0001;
const PSM_HID_CTRL: u16 = 0x0011;
const PSM_HID_INTR: u16 = 0x0013;

const MAX_DEVS: usize = 16;
const MAX_BONDS: usize = 8;
const MAX_LINKS: usize = 3;
const MAX_CHANS: usize = 4;
const CMDQ: usize = 16;
const ACL_RX_MAX: usize = 1100;
const L2_RX_MAX: usize = 2048;
const SDP_MAX: usize = 2048;

fn ms() -> u64 {
    crate::arch::ms()
}

fn le16(p: &[u8], o: usize) -> u16 {
    if o + 2 > p.len() {
        return 0;
    }
    (p[o] as u16) | ((p[o + 1] as u16) << 8)
}

fn le32(p: &[u8], o: usize) -> u32 {
    if o + 4 > p.len() {
        return 0;
    }
    (p[o] as u32) | ((p[o + 1] as u32) << 8) | ((p[o + 2] as u32) << 16) | ((p[o + 3] as u32) << 24)
}

fn addr_at(p: &[u8], o: usize) -> [u8; 6] {
    let mut a = [0u8; 6];
    if o + 6 <= p.len() {
        a.copy_from_slice(&p[o..o + 6]);
    }
    a
}

/// MSB-first, as a person reads it; `a` is wire order (LSB first).
pub fn fmt_addr(a: &[u8; 6]) -> String {
    format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", a[5], a[4], a[3], a[2], a[1], a[0])
}

/// The object name under `.config/unaos/bt/`: twelve hex digits, MSB first.
fn addr12(a: &[u8; 6]) -> String {
    format!("{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}", a[5], a[4], a[3], a[2], a[1], a[0])
}

fn hexv(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// `aa:bb:cc:dd:ee:ff` (or twelve bare hex digits) -> wire order.
fn parse_addr(s: &str) -> Option<[u8; 6]> {
    let digits: Vec<u8> = s.bytes().filter(|&b| b != b':' && b != b'-').collect();
    if digits.len() != 12 {
        return None;
    }
    let mut a = [0u8; 6];
    for i in 0..6 {
        let v = (hexv(digits[2 * i])? << 4) | hexv(digits[2 * i + 1])?;
        a[5 - i] = v;
    }
    Some(a)
}

/// Class of Device, short: the major class, and for a Peripheral the keyboard/pointing bits.
fn cod_kind(cod: u32) -> &'static str {
    let major = (cod >> 8) & 0x1F;
    let minor = (cod >> 2) & 0x3F;
    match major {
        0x01 => "computer",
        0x02 => "phone",
        0x03 => "network",
        0x04 => "audio/video",
        0x05 => match (minor >> 4) & 0x3 {
            1 => "peripheral/keyboard",
            2 => "peripheral/pointing",
            3 => "peripheral/kbd+pointing",
            _ => "peripheral",
        },
        0x06 => "imaging",
        0x07 => "wearable",
        0x08 => "toy",
        0x09 => "health",
        _ => "other",
    }
}

fn is_keyboard_cod(cod: u32) -> bool {
    (cod >> 8) & 0x1F == 0x05 && ((cod >> 2) >> 4) & 0x1 == 1
}

fn op_name(op: u16) -> &'static str {
    match op {
        H_INQUIRY => "Inquiry",
        H_INQUIRY_CANCEL => "Inquiry_Cancel",
        H_CREATE_CONN => "Create_Connection",
        H_DISCONNECT => "Disconnect",
        H_CREATE_CONN_CANCEL => "Create_Connection_Cancel",
        H_ACCEPT_CONN => "Accept_Connection_Request",
        H_REJECT_CONN => "Reject_Connection_Request",
        H_LINK_KEY_REPLY => "Link_Key_Request_Reply",
        H_LINK_KEY_NEG => "Link_Key_Request_Negative_Reply",
        H_PIN_REPLY => "PIN_Code_Request_Reply",
        H_PIN_NEG => "PIN_Code_Request_Negative_Reply",
        H_AUTH_REQ => "Authentication_Requested",
        H_SET_ENC => "Set_Connection_Encryption",
        H_RNR => "Remote_Name_Request",
        H_RNR_CANCEL => "Remote_Name_Request_Cancel",
        H_IO_CAP_REPLY => "IO_Capability_Request_Reply",
        H_CONFIRM_REPLY => "User_Confirmation_Request_Reply",
        H_CONFIRM_NEG => "User_Confirmation_Request_Negative_Reply",
        H_PASSKEY_NEG => "User_Passkey_Request_Negative_Reply",
        H_OOB_NEG => "Remote_OOB_Data_Request_Negative_Reply",
        H_IO_CAP_NEG => "IO_Capability_Request_Negative_Reply",
        H_SET_EVENT_MASK => "Set_Event_Mask",
        H_RESET => "Reset",
        H_WRITE_LOCAL_NAME => "Write_Local_Name",
        H_WRITE_PAGE_TIMEOUT => "Write_Page_Timeout",
        H_WRITE_SCAN_ENABLE => "Write_Scan_Enable",
        H_WRITE_COD => "Write_Class_of_Device",
        H_WRITE_INQUIRY_MODE => "Write_Inquiry_Mode",
        H_WRITE_SSP_MODE => "Write_Simple_Pairing_Mode",
        H_READ_BUFFER_SIZE => "Read_Buffer_Size",
        H_READ_BD_ADDR => "Read_BD_ADDR",
        _ => "?",
    }
}

fn printable(b: &[u8]) -> String {
    let mut s = String::new();
    for &c in b {
        if c == 0 {
            break;
        }
        s.push(if (0x20..0x7F).contains(&c) { c as char } else { '.' });
    }
    s
}

// ── State ───────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Dev {
    used: bool,
    addr: [u8; 6],
    cod: u32,
    psrm: u8,
    clk: u16,
    rssi: Option<i8>,
    name: [u8; 32],
    name_len: u8,
    /// 0 none yet, 1 from EIR, 2 from Remote_Name_Request, 3 the name request failed.
    name_src: u8,
}

impl Dev {
    const EMPTY: Dev = Dev { used: false, addr: [0; 6], cod: 0, psrm: 1, clk: 0, rssi: None, name: [0; 32], name_len: 0, name_src: 0 };
    fn name(&self) -> String {
        printable(&self.name[..self.name_len as usize])
    }
}

#[derive(Clone)]
struct Bond {
    addr: [u8; 6],
    key: [u8; 16],
    ktype: u8,
    cod: u32,
    name: String,
    desc: Vec<u8>,
    dirty: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChanSt {
    /// We sent Connection Request; waiting for the response.
    WaitConnRsp,
    /// The peer asked, we answered PENDING until the link is encrypted (`ident` = its request).
    PendingAccept,
    Config,
    Open,
    WaitDiscRsp,
}

#[derive(Clone, Copy)]
struct Chan {
    psm: u16,
    lcid: u16,
    rcid: u16,
    st: ChanSt,
    ours_ok: bool,
    theirs_ok: bool,
    rmtu: u16,
    ident: u8,
    t: u64,
    cfg_retry: bool,
    /// This host sent the Connection Request (false: the peer opened it and we accepted).
    ours: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LinkSt {
    Paging,
    Accepting,
    Authing,
    Encrypting,
    Up,
    Closing,
}

impl LinkSt {
    fn name(self) -> &'static str {
        match self {
            LinkSt::Paging => "paging",
            LinkSt::Accepting => "accepting",
            LinkSt::Authing => "authenticating",
            LinkSt::Encrypting => "encrypting",
            LinkSt::Up => "up",
            LinkSt::Closing => "closing",
        }
    }
}

#[derive(Clone, Copy, Default)]
struct PtrMap {
    rid: u8,
    l: ReportLayout,
    wheel_off: u16,
    wheel_size: u8,
}

#[derive(Clone, Copy, Default)]
struct HidMap {
    ptr: Option<PtrMap>,
    kbd_rid: Option<u8>,
    kbd_boot_ok: bool,
    boot: bool,
}

struct Link {
    addr: [u8; 6],
    handle: u16,
    st: LinkSt,
    outbound: bool,
    pair_new: bool,
    reconnect: bool,
    t: u64,
    peer_io: Option<(u8, u8)>,
    confirm: Option<u32>,
    confirm_at: u64,
    passkey: Option<u32>,
    chans: Vec<Chan>,
    next_lcid: u16,
    ident: u8,
    rx: Vec<u8>,
    rx_want: usize,
    sdp_acc: Vec<u8>,
    sdp_tid: u16,
    sdp_rounds: u8,
    sdp_done: bool,
    hid_up: bool,
    map: HidMap,
    reports: u32,
    prev_btn: u8,
    kprev: [u8; 6],
    kmods: u8,
    kleds: u8,
}

impl Link {
    fn new(addr: [u8; 6], st: LinkSt, outbound: bool, pair_new: bool, now: u64) -> Link {
        Link {
            addr,
            handle: 0,
            st,
            outbound,
            pair_new,
            reconnect: false,
            t: now,
            peer_io: None,
            confirm: None,
            confirm_at: 0,
            passkey: None,
            chans: Vec::new(),
            next_lcid: 0x0040,
            ident: 0,
            rx: Vec::new(),
            rx_want: 0,
            sdp_acc: Vec::new(),
            sdp_tid: 0,
            sdp_rounds: 0,
            sdp_done: false,
            hid_up: false,
            map: HidMap::default(),
            reports: 0,
            prev_btn: 0,
            kprev: [0; 6],
            kmods: 0,
            kleds: 0,
        }
    }
    fn chan_psm(&self, psm: u16) -> Option<usize> {
        self.chans.iter().position(|c| c.psm == psm && c.st != ChanSt::WaitDiscRsp)
    }
    fn open_rcid(&self, psm: u16) -> Option<u16> {
        self.chans.iter().find(|c| c.psm == psm && c.st == ChanSt::Open).map(|c| c.rcid)
    }
}

#[derive(Clone, Copy)]
struct Cmd {
    op: u16,
    len: u8,
    p: [u8; 32],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scan {
    Idle,
    Inquiring,
    Naming,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rc {
    Wait,
    Run,
    Done,
}

struct Bt {
    up: bool,
    idx: usize,
    bd: [u8; 6],
    acl_len: u16,
    acl_num: u16,
    credits: u16,
    evt_toggle: bool,
    evt_armed: bool,
    inflight: Option<(u16, u64)>,
    cmdq: [Cmd; CMDQ],
    cq_head: usize,
    cq_len: usize,
    acl_rx: Vec<u8>,
    acl_next_poll: u64,
    acltx: Vec<(u16, Vec<u8>)>,
    devs: [Dev; MAX_DEVS],
    scan: Scan,
    scan_t0: u64,
    scan_ms: u64,
    naming: Option<([u8; 6], u64)>,
    links: Vec<Link>,
    paging: Option<[u8; 6]>,
    pair_window: Option<([u8; 6], u64)>,
    bonds: Vec<Bond>,
    forgotten: Vec<[u8; 6]>,
    bonds_loaded: bool,
    rc: Rc,
    rc_deadline: u64,
    rc_next: usize,
    rc_tried: u32,
    // counters for `tests bt`
    inquiry_found: u32,
    paired: u32,
    connected: u32,
    hid_reports: u32,
    hci_nonzero: u32,
    events: u32,
}

impl Bt {
    fn new() -> Bt {
        Bt {
            up: false,
            idx: 0,
            bd: [0; 6],
            acl_len: 0,
            acl_num: 0,
            credits: 0,
            evt_toggle: false,
            evt_armed: false,
            inflight: None,
            cmdq: [Cmd { op: 0, len: 0, p: [0; 32] }; CMDQ],
            cq_head: 0,
            cq_len: 0,
            acl_rx: Vec::new(),
            acl_next_poll: 0,
            acltx: Vec::new(),
            devs: [Dev::EMPTY; MAX_DEVS],
            scan: Scan::Idle,
            scan_t0: 0,
            scan_ms: 0,
            naming: None,
            links: Vec::new(),
            paging: None,
            pair_window: None,
            bonds: Vec::new(),
            forgotten: Vec::new(),
            bonds_loaded: false,
            rc: Rc::Wait,
            rc_deadline: 0,
            rc_next: 0,
            rc_tried: 0,
            inquiry_found: 0,
            paired: 0,
            connected: 0,
            hid_reports: 0,
            hci_nonzero: 0,
            events: 0,
        }
    }

    fn push_cmd(&mut self, op: u16, params: &[u8]) -> bool {
        if self.cq_len >= CMDQ || params.len() > 32 {
            serial_println!(":: bthid: HCI {} ({:#06x}) NOT QUEUED — queue full or params too long ({}) ::", op_name(op), op, params.len());
            return false;
        }
        let mut c = Cmd { op, len: params.len() as u8, p: [0; 32] };
        c.p[..params.len()].copy_from_slice(params);
        let slot = (self.cq_head + self.cq_len) % CMDQ;
        self.cmdq[slot] = c;
        self.cq_len += 1;
        true
    }

    fn pop_cmd(&mut self) -> Option<Cmd> {
        if self.cq_len == 0 {
            return None;
        }
        let c = self.cmdq[self.cq_head];
        self.cq_head = (self.cq_head + 1) % CMDQ;
        self.cq_len -= 1;
        Some(c)
    }

    fn dev_slot(&mut self, addr: &[u8; 6]) -> usize {
        if let Some(i) = self.devs.iter().position(|d| d.used && d.addr == *addr) {
            return i;
        }
        if let Some(i) = self.devs.iter().position(|d| !d.used) {
            self.devs[i] = Dev::EMPTY;
            self.devs[i].used = true;
            self.devs[i].addr = *addr;
            return i;
        }
        MAX_DEVS - 1 // full: the last row is recycled, never an overrun
    }

    fn dev(&self, addr: &[u8; 6]) -> Option<&Dev> {
        self.devs.iter().find(|d| d.used && d.addr == *addr)
    }

    fn bond(&self, addr: &[u8; 6]) -> Option<&Bond> {
        self.bonds.iter().find(|b| b.addr == *addr)
    }

    fn link_addr(&self, addr: &[u8; 6]) -> Option<usize> {
        self.links.iter().position(|l| l.addr == *addr)
    }

    fn link_handle(&self, h: u16) -> Option<usize> {
        self.links.iter().position(|l| l.handle == h && l.handle != 0)
    }

    fn pairing_allowed(&self, addr: &[u8; 6], now: u64) -> bool {
        if let Some(li) = self.link_addr(addr) {
            if self.links[li].pair_new {
                return true;
            }
        }
        matches!(self.pair_window, Some((a, until)) if a == *addr && now < until)
    }

    fn desc_for(&self, addr: &[u8; 6]) -> Option<Vec<u8>> {
        self.bond(addr).filter(|b| !b.desc.is_empty()).map(|b| b.desc.clone())
    }

    // ── L2CAP send side ─────────────────────────────────────────────────────────────────────────

    fn l2_send(&mut self, handle: u16, cid: u16, body: &[u8]) {
        if self.acltx.len() >= 8 {
            serial_println!(":: bthid: l2cap tx queue full — PDU on cid={:#06x} dropped ::", cid);
            return;
        }
        let mut v = Vec::with_capacity(body.len() + 4);
        v.extend_from_slice(&(body.len() as u16).to_le_bytes());
        v.extend_from_slice(&cid.to_le_bytes());
        v.extend_from_slice(body);
        self.acltx.push((handle, v));
    }

    /// One signalling command on CID 0x0001. `ident` None = a fresh request identifier.
    fn sig(&mut self, li: usize, code: u8, ident: Option<u8>, data: &[u8]) -> u8 {
        let id = match ident {
            Some(i) => i,
            None => {
                let l = &mut self.links[li];
                l.ident = if l.ident == 0xFF { 1 } else { l.ident + 1 };
                l.ident
            }
        };
        let mut b = Vec::with_capacity(4 + data.len());
        b.push(code);
        b.push(id);
        b.extend_from_slice(&(data.len() as u16).to_le_bytes());
        b.extend_from_slice(data);
        let h = self.links[li].handle;
        self.l2_send(h, 0x0001, &b);
        id
    }

    fn open_chan(&mut self, li: usize, psm: u16, now: u64) {
        if self.links[li].chan_psm(psm).is_some() {
            return;
        }
        if self.links[li].chans.len() >= MAX_CHANS {
            serial_println!(":: BTHID: l2cap {} connect psm={:#06x} NOT SENT — {} channels already held ::", fmt_addr(&self.links[li].addr), psm, MAX_CHANS);
            return;
        }
        let lcid = self.links[li].next_lcid;
        self.links[li].next_lcid = lcid + 1;
        let mut d = [0u8; 4];
        d[..2].copy_from_slice(&psm.to_le_bytes());
        d[2..].copy_from_slice(&lcid.to_le_bytes());
        let ident = self.sig(li, 0x02, None, &d);
        self.links[li].chans.push(Chan { psm, lcid, rcid: 0, st: ChanSt::WaitConnRsp, ours_ok: false, theirs_ok: false, rmtu: 672, ident, t: now, cfg_retry: false, ours: true });
        serial_println!(":: BTHID: l2cap {} connect psm={:#06x} lcid={:#06x} -> request sent ::", fmt_addr(&self.links[li].addr), psm, lcid);
    }

    fn send_cfg(&mut self, li: usize, ci: usize, with_mtu: bool) {
        let rcid = self.links[li].chans[ci].rcid;
        let mut d = Vec::with_capacity(8);
        d.extend_from_slice(&rcid.to_le_bytes());
        d.extend_from_slice(&[0, 0]);
        if with_mtu {
            d.extend_from_slice(&[0x01, 0x02]);
            d.extend_from_slice(&L2_MTU.to_le_bytes());
        }
        self.sig(li, 0x04, None, &d);
    }

    fn close_chan(&mut self, li: usize, ci: usize, now: u64) {
        let (lcid, rcid) = (self.links[li].chans[ci].lcid, self.links[li].chans[ci].rcid);
        let mut d = [0u8; 4];
        d[..2].copy_from_slice(&rcid.to_le_bytes());
        d[2..].copy_from_slice(&lcid.to_le_bytes());
        self.sig(li, 0x06, None, &d);
        self.links[li].chans[ci].st = ChanSt::WaitDiscRsp;
        self.links[li].chans[ci].t = now;
    }

    fn disconnect(&mut self, li: usize, why: &str) {
        let h = self.links[li].handle;
        serial_println!(":: BTHID: link {} handle={:#06x} DISCONNECT — {} ::", fmt_addr(&self.links[li].addr), h, why);
        if h != 0 && self.links[li].st != LinkSt::Closing {
            let mut p = [0u8; 3];
            p[..2].copy_from_slice(&h.to_le_bytes());
            p[2] = 0x13; // Remote User Terminated Connection
            self.push_cmd(H_DISCONNECT, &p);
        }
        self.links[li].st = LinkSt::Closing;
        self.links[li].t = ms();
    }

    // ── Public actions (verbs, reconnect) ───────────────────────────────────────────────────────

    fn start_scan(&mut self, now: u64) -> Result<(), &'static str> {
        if !self.up {
            return Err("the radio is not up");
        }
        if self.scan != Scan::Idle {
            return Err("a scan is already running");
        }
        if self.paging.is_some() || self.rc == Rc::Run {
            return Err("a page is in flight (connect or reconnect); try again in a few seconds");
        }
        let mut p = [0u8; 5];
        p[..3].copy_from_slice(&GIAC);
        p[3] = INQUIRY_LEN;
        p[4] = 0; // unlimited responses
        if !self.push_cmd(H_INQUIRY, &p) {
            return Err("the HCI command queue is full");
        }
        for d in self.devs.iter_mut() {
            *d = Dev::EMPTY;
        }
        self.scan = Scan::Inquiring;
        self.scan_t0 = now;
        self.inquiry_found = 0;
        serial_println!(":: BTHID: inquiry start lap=GIAC(0x9e8b33) length=10240ms host_deadline={}ms at={} ::", INQUIRY_HOST_MS, now);
        Ok(())
    }

    fn start_connect(&mut self, addr: [u8; 6], pair_new: bool, reconnect: bool, now: u64) -> Result<(), &'static str> {
        if !self.up {
            return Err("the radio is not up");
        }
        if let Some(li) = self.link_addr(&addr) {
            if self.links[li].st == LinkSt::Up && !pair_new {
                return Err("already connected");
            }
            if self.links[li].st != LinkSt::Up {
                return Err("a connection to that device is already being set up");
            }
            // Pairing anew over a live link: authenticate again with the stored key refused.
            self.links[li].pair_new = true;
            let h = self.links[li].handle;
            self.links[li].st = LinkSt::Authing;
            self.links[li].t = now;
            self.push_cmd(H_AUTH_REQ, &h.to_le_bytes());
            return Ok(());
        }
        if self.paging.is_some() {
            return Err("another page is in flight");
        }
        if self.scan != Scan::Idle {
            return Err("a scan is running; wait for it to finish");
        }
        if self.links.len() >= MAX_LINKS {
            return Err("too many links");
        }
        let (psrm, clk) = match self.dev(&addr) {
            Some(d) => (d.psrm.min(2), d.clk | 0x8000),
            None => (0x01, 0),
        };
        let mut p = [0u8; 13];
        p[..6].copy_from_slice(&addr);
        p[6..8].copy_from_slice(&0xCC18u16.to_le_bytes()); // DM1 DH1 DM3 DH3 DM5 DH5
        p[8] = psrm;
        p[9] = 0;
        p[10..12].copy_from_slice(&clk.to_le_bytes());
        p[12] = 0x01; // allow role switch
        if !self.push_cmd(H_CREATE_CONN, &p) {
            return Err("the HCI command queue is full");
        }
        let mut l = Link::new(addr, LinkSt::Paging, true, pair_new, now);
        l.reconnect = reconnect;
        self.links.push(l);
        self.paging = Some(addr);
        if pair_new {
            self.pair_window = Some((addr, now + PAIR_WINDOW_MS));
        }
        serial_println!(":: BTHID: page {} psrm={} clock_offset={:#06x} purpose={} page_timeout=5000ms ::", fmt_addr(&addr), psrm, clk, if reconnect { "reconnect" } else if pair_new { "pair" } else { "connect" });
        Ok(())
    }

    fn confirm(&mut self, addr: [u8; 6], yes: bool) -> Result<u32, &'static str> {
        let li = self.link_addr(&addr).ok_or("no link to that device")?;
        let v = self.links[li].confirm.take().ok_or("no confirmation is pending for that device")?;
        self.push_cmd(if yes { H_CONFIRM_REPLY } else { H_CONFIRM_NEG }, &addr);
        serial_println!(":: BTHID: pair {} numeric comparison value={:06} -> user said {} ::", fmt_addr(&addr), v, if yes { "YES" } else { "NO" });
        Ok(v)
    }

    // ── Event dispatch ──────────────────────────────────────────────────────────────────────────

    fn on_event(&mut self, pkt: &[u8], now: u64) {
        if pkt.len() < 2 {
            return;
        }
        self.events += 1;
        let code = pkt[0];
        let plen = (pkt[1] as usize).min(pkt.len() - 2);
        let p = &pkt[2..2 + plen];
        match code {
            E_CMD_COMPLETE if p.len() >= 3 => {
                let op = le16(p, 1);
                let status = p.get(3).copied().unwrap_or(0xFF);
                self.on_cmd_done(op, status, true, now);
            }
            E_CMD_STATUS if p.len() >= 4 => {
                let op = le16(p, 2);
                self.on_cmd_done(op, p[0], false, now);
            }
            E_INQUIRY_RESULT | E_INQUIRY_RESULT_RSSI | E_EXT_INQUIRY_RESULT => {
                let mut recs: Vec<InqRec> = Vec::new();
                parse_inquiry(code, p, &mut |r| recs.push(r));
                for r in recs {
                    self.on_inquiry_rec(&r);
                }
            }
            E_INQUIRY_COMPLETE => {
                let st = p.first().copied().unwrap_or(0xFF);
                serial_println!(":: BTHID: inquiry complete status={:#04x} responses={} elapsed={}ms ::", st, self.inquiry_found, now.saturating_sub(self.scan_t0));
                if self.scan == Scan::Inquiring {
                    self.scan = Scan::Naming;
                    self.next_name(now);
                }
            }
            E_RNR_COMPLETE if p.len() >= 7 => {
                let addr = addr_at(p, 1);
                let st = p[0];
                let i = self.dev_slot(&addr);
                if st == 0 {
                    let raw = &p[7..];
                    let n = raw.iter().position(|&c| c == 0).unwrap_or(raw.len()).min(32);
                    self.devs[i].name[..n].copy_from_slice(&raw[..n]);
                    self.devs[i].name_len = n as u8;
                    self.devs[i].name_src = 2;
                } else {
                    self.devs[i].name_src = 3;
                }
                serial_println!(":: BTHID: name {} status={:#04x} name=\"{}\" ::", fmt_addr(&addr), st, self.devs[i].name());
                if matches!(self.naming, Some((a, _)) if a == addr) {
                    self.naming = None;
                    self.next_name(now);
                }
            }
            E_CONN_REQUEST if p.len() >= 10 => {
                let addr = addr_at(p, 0);
                let cod = (p[6] as u32) | ((p[7] as u32) << 8) | ((p[8] as u32) << 16);
                let lt = p[9];
                let known = self.bond(&addr).is_some();
                let ok = lt == 0x01 && (known || self.pairing_allowed(&addr, now)) && self.links.len() < MAX_LINKS && self.link_addr(&addr).is_none();
                if ok {
                    let mut q = [0u8; 7];
                    q[..6].copy_from_slice(&addr);
                    q[6] = 0x01; // remain peripheral: a role switch some HID devices refuse is not asked for
                    self.push_cmd(H_ACCEPT_CONN, &q);
                    self.links.push(Link::new(addr, LinkSt::Accepting, false, !known, now));
                } else {
                    let mut q = [0u8; 7];
                    q[..6].copy_from_slice(&addr);
                    q[6] = 0x0F; // Connection Rejected due to Unacceptable BD_ADDR
                    self.push_cmd(H_REJECT_CONN, &q);
                }
                serial_println!(":: BTHID: inbound connection request {} class={:#08x} ({}) link_type={:#04x} bonded={} -> {} ::", fmt_addr(&addr), cod, cod_kind(cod), lt, known, if ok { "ACCEPT" } else { "REJECT (not bonded, no `bt pair` window)" });
            }
            E_CONN_COMPLETE if p.len() >= 11 => {
                let st = p[0];
                let h = le16(p, 1) & 0x0FFF;
                let addr = addr_at(p, 3);
                let lt = p[9];
                if self.paging == Some(addr) {
                    self.paging = None;
                }
                if lt != 0x01 {
                    return; // SCO/eSCO is not this stack's
                }
                let li = match self.link_addr(&addr) {
                    Some(li) => li,
                    None => {
                        self.links.push(Link::new(addr, LinkSt::Accepting, false, false, now));
                        self.links.len() - 1
                    }
                };
                serial_println!(":: BTHID: connection complete {} status={:#04x} handle={:#06x} encryption={} outbound={} ::", fmt_addr(&addr), st, h, p[10], self.links[li].outbound);
                if st != 0 {
                    let rc = self.links[li].reconnect;
                    self.links.remove(li);
                    if rc {
                        serial_println!(":: BTHID: reconnect {} -> NOT REACHED status={:#04x} ::", fmt_addr(&addr), st);
                    }
                    return;
                }
                let l = &mut self.links[li];
                l.handle = h;
                l.st = LinkSt::Authing;
                l.t = now;
                self.push_cmd(H_AUTH_REQ, &h.to_le_bytes());
            }
            E_DISCONN_COMPLETE if p.len() >= 4 => {
                let h = le16(p, 1) & 0x0FFF;
                if let Some(li) = self.link_handle(h) {
                    let l = self.links.remove(li);
                    if l.hid_up {
                        self.connected = self.connected.saturating_sub(1);
                    }
                    serial_println!(":: BTHID: disconnected {} handle={:#06x} status={:#04x} reason={:#04x} hid_reports={} ::", fmt_addr(&l.addr), h, p[0], p[3], l.reports);
                }
            }
            E_AUTH_COMPLETE if p.len() >= 3 => {
                let h = le16(p, 1) & 0x0FFF;
                let st = p[0];
                if let Some(li) = self.link_handle(h) {
                    let addr = self.links[li].addr;
                    serial_println!(":: BTHID: authentication {} status={:#04x} ::", fmt_addr(&addr), st);
                    if st == 0 {
                        self.links[li].st = LinkSt::Encrypting;
                        self.links[li].t = now;
                        self.push_cmd(H_SET_ENC, &[h as u8, (h >> 8) as u8, 0x01]);
                    } else {
                        if st == 0x06 && self.bond(&addr).is_some() && !self.links[li].pair_new {
                            serial_println!(":: BTHID: authentication {} — the peer refused the STORED link key (status 0x06); it has forgotten us. `bt pair {}` to pair again ::", fmt_addr(&addr), fmt_addr(&addr));
                        }
                        self.disconnect(li, "authentication failed");
                    }
                }
            }
            E_ENC_CHANGE if p.len() >= 4 => {
                let h = le16(p, 1) & 0x0FFF;
                if let Some(li) = self.link_handle(h) {
                    let on = p[3] != 0;
                    serial_println!(":: BTHID: encryption {} status={:#04x} enabled={} ::", fmt_addr(&self.links[li].addr), p[0], on);
                    if p[0] == 0 && on {
                        if self.links[li].st != LinkSt::Up {
                            self.links[li].st = LinkSt::Up;
                            self.links[li].t = now;
                            self.on_link_up(li, now);
                        }
                    } else {
                        self.disconnect(li, "encryption refused or turned off");
                    }
                }
            }
            E_NOCP if !p.is_empty() => {
                let n = p[0] as usize;
                let mut sum = 0u32;
                for i in 0..n {
                    // Parameter-major arrays: all handles, then all counts.
                    sum += le16(p, 1 + 2 * n + 2 * i) as u32;
                }
                self.credits = (self.credits as u32 + sum).min(self.acl_num as u32) as u16;
            }
            E_PIN_REQUEST if p.len() >= 6 => {
                let addr = addr_at(p, 0);
                if self.pairing_allowed(&addr, now) {
                    let mut q = [0u8; 23];
                    q[..6].copy_from_slice(&addr);
                    q[6] = 4;
                    q[7..11].copy_from_slice(b"0000");
                    self.push_cmd(H_PIN_REPLY, &q);
                    serial_println!(":: BTHID: pair {} LEGACY PIN pairing (pre-2.1 device) -> answered PIN 0000 ::", fmt_addr(&addr));
                } else {
                    self.push_cmd(H_PIN_NEG, &addr);
                    serial_println!(":: BTHID: pair {} legacy PIN request REFUSED — no `bt pair` for this device ::", fmt_addr(&addr));
                }
            }
            E_LINK_KEY_REQUEST if p.len() >= 6 => {
                let addr = addr_at(p, 0);
                let fresh = self.link_addr(&addr).map(|li| self.links[li].pair_new).unwrap_or(false);
                match self.bond(&addr).map(|b| (b.key, b.ktype)) {
                    Some((key, ktype)) if !fresh => {
                        let mut q = [0u8; 22];
                        q[..6].copy_from_slice(&addr);
                        q[6..].copy_from_slice(&key);
                        self.push_cmd(H_LINK_KEY_REPLY, &q);
                        serial_println!(":: BTHID: link key request {} -> stored key supplied (type={:#04x}) ::", fmt_addr(&addr), ktype);
                    }
                    _ => {
                        self.push_cmd(H_LINK_KEY_NEG, &addr);
                        serial_println!(":: BTHID: link key request {} -> none ({}) — pairing follows ::", fmt_addr(&addr), if fresh { "pairing anew" } else { "not bonded" });
                    }
                }
            }
            E_IO_CAP_REQUEST if p.len() >= 6 => {
                let addr = addr_at(p, 0);
                if self.pairing_allowed(&addr, now) {
                    let mut q = [0u8; 9];
                    q[..6].copy_from_slice(&addr);
                    q[6] = IO_CAP;
                    q[7] = 0x00; // no OOB data
                    q[8] = AUTH_REQ;
                    self.push_cmd(H_IO_CAP_REPLY, &q);
                } else {
                    let mut q = [0u8; 7];
                    q[..6].copy_from_slice(&addr);
                    q[6] = 0x18; // Pairing Not Allowed
                    self.push_cmd(H_IO_CAP_NEG, &q);
                    serial_println!(":: BTHID: pair {} IO capability request REFUSED — no `bt pair` for this device ::", fmt_addr(&addr));
                }
            }
            E_IO_CAP_RESPONSE if p.len() >= 9 => {
                let addr = addr_at(p, 0);
                if let Some(li) = self.link_addr(&addr) {
                    self.links[li].peer_io = Some((p[6], p[8]));
                }
                serial_println!(":: BTHID: pair {} peer io_capability={:#04x} oob={:#04x} auth_req={:#04x} ::", fmt_addr(&addr), p[6], p[7], p[8]);
            }
            E_CONFIRM_REQUEST if p.len() >= 10 => {
                let addr = addr_at(p, 0);
                let v = le32(p, 6) % 1_000_000;
                let peer_io = self.link_addr(&addr).and_then(|li| self.links[li].peer_io).map(|x| x.0);
                // Just works when the peer has no input and no output (a mouse): there is nothing to
                // compare, and the controller asks the host only to consent.
                if peer_io == Some(0x03) {
                    self.push_cmd(H_CONFIRM_REPLY, &addr);
                    serial_println!(":: BTHID: pair {} method=just-works (peer NoInputNoOutput) -> confirmed ::", fmt_addr(&addr));
                } else if let Some(li) = self.link_addr(&addr) {
                    self.links[li].confirm = Some(v);
                    self.links[li].confirm_at = now;
                    serial_println!(":: BTHID: pair {} method=numeric-comparison value={:06} -> waiting {} s for `bt pair {} yes` ::", fmt_addr(&addr), v, CONFIRM_MS / 1000, fmt_addr(&addr));
                } else {
                    self.push_cmd(H_CONFIRM_NEG, &addr);
                }
            }
            E_PASSKEY_NOTIFY if p.len() >= 10 => {
                let addr = addr_at(p, 0);
                let v = le32(p, 6) % 1_000_000;
                if let Some(li) = self.link_addr(&addr) {
                    self.links[li].passkey = Some(v);
                }
                serial_println!(":: BTHID: pair {} method=passkey-entry passkey={:06} -> type it on the keyboard, then Enter ::", fmt_addr(&addr), v);
            }
            E_PASSKEY_REQUEST if p.len() >= 6 => {
                let addr = addr_at(p, 0);
                self.push_cmd(H_PASSKEY_NEG, &addr);
                serial_println!(":: BTHID: pair {} passkey ENTRY asked of this host — refused (no entry path; owed) ::", fmt_addr(&addr));
            }
            E_OOB_REQUEST if p.len() >= 6 => {
                let addr = addr_at(p, 0);
                self.push_cmd(H_OOB_NEG, &addr);
            }
            E_SSP_COMPLETE if p.len() >= 7 => {
                let addr = addr_at(p, 1);
                if let Some(li) = self.link_addr(&addr) {
                    self.links[li].confirm = None;
                    self.links[li].passkey = None;
                }
                serial_println!(":: BTHID: pair {} simple pairing complete status={:#04x} ::", fmt_addr(&addr), p[0]);
            }
            E_LINK_KEY_NOTIFY if p.len() >= 23 => {
                let addr = addr_at(p, 0);
                let mut key = [0u8; 16];
                key.copy_from_slice(&p[6..22]);
                let ktype = p[22];
                let (cod, name) = match self.dev(&addr) {
                    Some(d) => (d.cod, d.name()),
                    None => (0, String::new()),
                };
                match self.bonds.iter_mut().find(|b| b.addr == addr) {
                    Some(b) => {
                        b.key = key;
                        b.ktype = ktype;
                        if cod != 0 {
                            b.cod = cod;
                        }
                        if !name.is_empty() {
                            b.name = name;
                        }
                        b.dirty = true;
                    }
                    None => {
                        if self.bonds.len() >= MAX_BONDS {
                            self.bonds.remove(0);
                        }
                        self.bonds.push(Bond { addr, key, ktype, cod, name, desc: Vec::new(), dirty: true });
                    }
                }
                self.forgotten.retain(|a| *a != addr);
                if let Some(li) = self.link_addr(&addr) {
                    self.links[li].pair_new = false;
                }
                self.paired += 1;
                STORE_DIRTY.store(true, Ordering::Release);
                serial_println!(":: BTHID: link key {} type={:#04x} -> bonded; staged — the storage pass seals it as Holocron bt/{} ::", fmt_addr(&addr), ktype, addr12(&addr));
            }
            _ => {}
        }
    }

    fn on_cmd_done(&mut self, op: u16, status: u8, complete: bool, now: u64) {
        if matches!(self.inflight, Some((o, _)) if o == op) {
            self.inflight = None;
        }
        if op == 0 {
            return; // a NOP credit update
        }
        if status != 0 {
            self.hci_nonzero += 1;
        }
        serial_println!(":: bthid: HCI {} ({:#06x}) {} status={:#04x} ::", op_name(op), op, if complete { "CmdComplete" } else { "CmdStatus" }, status);
        if status == 0 {
            return;
        }
        match op {
            H_INQUIRY => {
                self.scan = Scan::Idle;
            }
            H_CREATE_CONN => {
                if let Some(a) = self.paging.take() {
                    if let Some(li) = self.link_addr(&a) {
                        self.links.remove(li);
                    }
                }
            }
            H_RNR => {
                if let Some((a, _)) = self.naming.take() {
                    let i = self.dev_slot(&a);
                    self.devs[i].name_src = 3;
                }
                self.next_name(now);
            }
            H_AUTH_REQ | H_SET_ENC => {
                // The handle is not in the CmdStatus; fail every link waiting at that step.
                let want = if op == H_AUTH_REQ { LinkSt::Authing } else { LinkSt::Encrypting };
                if let Some(li) = self.links.iter().position(|l| l.st == want) {
                    self.disconnect(li, "the controller refused the security step");
                }
            }
            _ => {}
        }
    }

    fn on_inquiry_rec(&mut self, r: &InqRec) {
        let i = self.dev_slot(&r.addr);
        let fresh = self.devs[i].cod == 0 && self.devs[i].rssi.is_none();
        let d = &mut self.devs[i];
        d.cod = r.cod;
        d.psrm = r.psrm;
        d.clk = r.clk;
        if r.rssi.is_some() {
            d.rssi = r.rssi;
        }
        if r.name_len > 0 && d.name_src != 2 {
            d.name = r.name;
            d.name_len = r.name_len;
            d.name_src = 1;
        }
        if fresh {
            self.inquiry_found += 1;
        }
        let d = self.devs[i];
        serial_println!(":: BTHID: inquiry dev {} class={:#08x} ({}) psrm={} rssi={} name=\"{}\" ::", fmt_addr(&d.addr), d.cod, cod_kind(d.cod), d.psrm, match d.rssi { Some(v) => format!("{}dBm", v), None => String::from("n/a") }, d.name());
    }

    fn next_name(&mut self, now: u64) {
        if self.naming.is_some() {
            return;
        }
        let Some(i) = self.devs.iter().position(|d| d.used && d.name_src == 0) else {
            let named = self.devs.iter().filter(|d| d.used && d.name_len > 0).count();
            self.scan_ms = now.saturating_sub(self.scan_t0);
            self.scan = Scan::Idle;
            serial_println!(":: BTHID: scan done devices={} named={} elapsed={}ms ::", self.inquiry_found, named, self.scan_ms);
            return;
        };
        let d = self.devs[i];
        let mut p = [0u8; 10];
        p[..6].copy_from_slice(&d.addr);
        p[6] = d.psrm.min(2);
        p[7] = 0;
        p[8..10].copy_from_slice(&(d.clk | 0x8000).to_le_bytes());
        self.devs[i].name_src = 3; // provisional: overwritten by the completion
        if self.push_cmd(H_RNR, &p) {
            self.naming = Some((d.addr, now));
        }
    }

    fn on_link_up(&mut self, li: usize, now: u64) {
        let addr = self.links[li].addr;
        serial_println!(":: BTHID: link {} handle={:#06x} UP (authenticated + encrypted) outbound={} ::", fmt_addr(&addr), self.links[li].handle, self.links[li].outbound);
        // Answer every channel request the peer opened before the link was secure.
        let pend: Vec<usize> = (0..self.links[li].chans.len()).filter(|&ci| self.links[li].chans[ci].st == ChanSt::PendingAccept).collect();
        for ci in pend {
            self.accept_chan(li, ci, now);
        }
        let have_desc = self.desc_for(&addr).is_some();
        if !have_desc {
            self.open_chan(li, PSM_SDP, now);
        } else if self.links[li].outbound {
            self.open_chan(li, PSM_HID_CTRL, now);
        }
    }

    fn accept_chan(&mut self, li: usize, ci: usize, now: u64) {
        let c = self.links[li].chans[ci];
        let mut d = [0u8; 8];
        d[..2].copy_from_slice(&c.lcid.to_le_bytes());
        d[2..4].copy_from_slice(&c.rcid.to_le_bytes());
        // result 0 success, status 0
        self.sig(li, 0x03, Some(c.ident), &d);
        self.links[li].chans[ci].st = ChanSt::Config;
        self.links[li].chans[ci].t = now;
        self.send_cfg(li, ci, true);
        serial_println!(":: BTHID: l2cap {} inbound psm={:#06x} lcid={:#06x} rcid={:#06x} -> accepted ::", fmt_addr(&self.links[li].addr), c.psm, c.lcid, c.rcid);
    }

    // ── ACL / L2CAP receive side ────────────────────────────────────────────────────────────────

    fn on_acl(&mut self, pkt: &[u8], now: u64) {
        if pkt.len() < 4 {
            return;
        }
        let hdr = le16(pkt, 0);
        let h = hdr & 0x0FFF;
        let pb = (hdr >> 12) & 0x3;
        let data = &pkt[4..];
        let Some(li) = self.link_handle(h) else { return };
        let l = &mut self.links[li];
        if pb == 0b01 {
            // Continuation of an L2CAP PDU.
            if l.rx_want == 0 || l.rx.len() + data.len() > L2_RX_MAX {
                l.rx.clear();
                l.rx_want = 0;
                return;
            }
            l.rx.extend_from_slice(data);
        } else {
            if data.len() < 4 {
                return;
            }
            l.rx.clear();
            l.rx_want = 4 + le16(data, 0) as usize;
            if l.rx_want > L2_RX_MAX {
                serial_println!(":: BTHID: l2cap {} PDU of {} bytes exceeds the {}-byte reassembly buffer — dropped ::", fmt_addr(&l.addr), l.rx_want, L2_RX_MAX);
                l.rx_want = 0;
                return;
            }
            l.rx.extend_from_slice(data);
        }
        if l.rx.len() < l.rx_want {
            return;
        }
        let pdu: Vec<u8> = core::mem::take(&mut l.rx);
        let want = l.rx_want;
        l.rx_want = 0;
        let cid = le16(&pdu, 2);
        let body = &pdu[4..want.min(pdu.len())];
        if cid == 0x0001 {
            self.on_sig(li, body, now);
        } else if let Some(ci) = self.links[li].chans.iter().position(|c| c.lcid == cid && c.st == ChanSt::Open) {
            let psm = self.links[li].chans[ci].psm;
            match psm {
                PSM_SDP => self.on_sdp(li, body, now),
                PSM_HID_INTR => self.on_hid_intr(li, body),
                PSM_HID_CTRL => {
                    if let Some(&b) = body.first() {
                        if b >> 4 == 0x0 && b & 0x0F != 0 {
                            serial_println!(":: BTHID: hid {} control HANDSHAKE result={:#04x} ::", fmt_addr(&self.links[li].addr), b & 0x0F);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn on_sig(&mut self, li: usize, mut b: &[u8], now: u64) {
        while b.len() >= 4 {
            let code = b[0];
            let ident = b[1];
            let len = (le16(b, 2) as usize).min(b.len() - 4);
            let d: Vec<u8> = b[4..4 + len].to_vec();
            b = &b[4 + len..];
            self.on_sig_cmd(li, code, ident, &d, now);
            if li >= self.links.len() {
                return;
            }
        }
    }

    fn on_sig_cmd(&mut self, li: usize, code: u8, ident: u8, d: &[u8], now: u64) {
        let addr = self.links[li].addr;
        match code {
            0x01 => {
                serial_println!(":: BTHID: l2cap {} COMMAND REJECT ident={} reason={:#06x} ::", fmt_addr(&addr), ident, le16(d, 0));
                if let Some(ci) = self.links[li].chans.iter().position(|c| c.ident == ident && c.st == ChanSt::WaitConnRsp) {
                    self.links[li].chans.remove(ci);
                }
            }
            0x02 if d.len() >= 4 => {
                let psm = le16(d, 0);
                let scid = le16(d, 2);
                let lcid = self.links[li].next_lcid;
                // Only the two HID channels are served: this host runs no SDP SERVER, so an inbound SDP
                // connection (a device asking about US) is refused rather than accepted and left mute.
                if !(psm == PSM_HID_CTRL || psm == PSM_HID_INTR) || self.links[li].chans.len() >= MAX_CHANS {
                    let mut r = [0u8; 8];
                    r[2..4].copy_from_slice(&scid.to_le_bytes());
                    r[4..6].copy_from_slice(&(if self.links[li].chans.len() >= MAX_CHANS { 0x0004u16 } else { 0x0002u16 }).to_le_bytes()); // no resources / PSM not supported
                    self.sig(li, 0x03, Some(ident), &r);
                    serial_println!(":: BTHID: l2cap {} inbound psm={:#06x} -> REFUSED (PSM not supported) ::", fmt_addr(&addr), psm);
                    return;
                }
                self.links[li].next_lcid = lcid + 1;
                self.links[li].chans.push(Chan { psm, lcid, rcid: scid, st: ChanSt::PendingAccept, ours_ok: false, theirs_ok: false, rmtu: 672, ident, t: now, cfg_retry: false, ours: false });
                let ci = self.links[li].chans.len() - 1;
                if self.links[li].st == LinkSt::Up {
                    self.accept_chan(li, ci, now);
                } else {
                    let mut r = [0u8; 8];
                    r[..2].copy_from_slice(&lcid.to_le_bytes());
                    r[2..4].copy_from_slice(&scid.to_le_bytes());
                    r[4..6].copy_from_slice(&0x0001u16.to_le_bytes()); // pending
                    r[6..8].copy_from_slice(&0x0001u16.to_le_bytes()); // authentication pending
                    self.sig(li, 0x03, Some(ident), &r);
                    serial_println!(":: BTHID: l2cap {} inbound psm={:#06x} -> PENDING until the link is encrypted ::", fmt_addr(&addr), psm);
                }
            }
            0x03 if d.len() >= 8 => {
                let dcid = le16(d, 0);
                let scid = le16(d, 2);
                let result = le16(d, 4);
                let Some(ci) = self.links[li].chans.iter().position(|c| c.lcid == scid && c.st == ChanSt::WaitConnRsp) else { return };
                match result {
                    0 => {
                        self.links[li].chans[ci].rcid = dcid;
                        self.links[li].chans[ci].st = ChanSt::Config;
                        self.links[li].chans[ci].t = now;
                        self.send_cfg(li, ci, true);
                    }
                    1 => {
                        self.links[li].chans[ci].t = now; // pending: the clock restarts
                    }
                    r => {
                        let psm = self.links[li].chans[ci].psm;
                        self.links[li].chans.remove(ci);
                        serial_println!(":: BTHID: l2cap {} connect psm={:#06x} REFUSED result={:#06x} status={:#06x} ::", fmt_addr(&addr), psm, r, le16(d, 6));
                        if psm == PSM_SDP {
                            self.links[li].sdp_done = true;
                            self.after_sdp(li, now);
                        }
                    }
                }
            }
            0x04 if d.len() >= 4 => {
                let dcid = le16(d, 0);
                let Some(ci) = self.links[li].chans.iter().position(|c| c.lcid == dcid) else {
                    let mut r = [0u8; 6];
                    r[..2].copy_from_slice(&0x0002u16.to_le_bytes()); // invalid CID
                    r[2..4].copy_from_slice(&dcid.to_le_bytes());
                    self.sig(li, 0x01, Some(ident), &r);
                    return;
                };
                let opts = parse_cfg_opts(&d[4..]);
                if let Some(m) = opts.mtu {
                    self.links[li].chans[ci].rmtu = m;
                }
                let rcid = self.links[li].chans[ci].rcid;
                let mut r = Vec::with_capacity(17);
                r.extend_from_slice(&rcid.to_le_bytes());
                r.extend_from_slice(&[0, 0]);
                if opts.non_basic_mode {
                    // Unacceptable parameters: this host speaks Basic mode only, and says so.
                    r.extend_from_slice(&0x0001u16.to_le_bytes());
                    r.extend_from_slice(&[0x04, 0x09, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                    self.sig(li, 0x05, Some(ident), &r);
                    return;
                }
                r.extend_from_slice(&0x0000u16.to_le_bytes());
                self.sig(li, 0x05, Some(ident), &r);
                self.links[li].chans[ci].theirs_ok = true;
                self.maybe_open(li, ci, now);
            }
            0x05 if d.len() >= 6 => {
                let scid = le16(d, 0);
                let result = le16(d, 4);
                let Some(ci) = self.links[li].chans.iter().position(|c| c.lcid == scid) else { return };
                if result == 0 {
                    self.links[li].chans[ci].ours_ok = true;
                    self.maybe_open(li, ci, now);
                } else if !self.links[li].chans[ci].cfg_retry {
                    self.links[li].chans[ci].cfg_retry = true;
                    self.send_cfg(li, ci, false);
                } else {
                    serial_println!(":: BTHID: l2cap {} configure lcid={:#06x} REFUSED twice result={:#06x} — channel closed ::", fmt_addr(&addr), scid, result);
                    self.close_chan(li, ci, now);
                }
            }
            0x06 if d.len() >= 4 => {
                let dcid = le16(d, 0);
                let scid = le16(d, 2);
                self.sig(li, 0x07, Some(ident), &d[..4]);
                if let Some(ci) = self.links[li].chans.iter().position(|c| c.lcid == dcid && c.rcid == scid) {
                    let c = self.links[li].chans.remove(ci);
                    serial_println!(":: BTHID: l2cap {} psm={:#06x} closed by the peer ::", fmt_addr(&addr), c.psm);
                    if c.psm != PSM_SDP && self.links[li].hid_up {
                        self.links[li].hid_up = false;
                        self.connected = self.connected.saturating_sub(1);
                    }
                }
            }
            0x07 if d.len() >= 4 => {
                let scid = le16(d, 2);
                if let Some(ci) = self.links[li].chans.iter().position(|c| c.lcid == scid) {
                    self.links[li].chans.remove(ci);
                }
            }
            0x08 => {
                self.sig(li, 0x09, Some(ident), d);
            }
            0x0A if d.len() >= 2 => {
                let mut r = [0u8; 4];
                r[..2].copy_from_slice(&d[..2]);
                r[2..].copy_from_slice(&0x0001u16.to_le_bytes()); // not supported
                self.sig(li, 0x0B, Some(ident), &r);
            }
            _ => {}
        }
    }

    fn maybe_open(&mut self, li: usize, ci: usize, now: u64) {
        let c = self.links[li].chans[ci];
        if !(c.ours_ok && c.theirs_ok) || c.st == ChanSt::Open {
            return;
        }
        self.links[li].chans[ci].st = ChanSt::Open;
        let addr = self.links[li].addr;
        serial_println!(":: BTHID: l2cap {} psm={:#06x} lcid={:#06x} rcid={:#06x} OPEN peer_mtu={} ::", fmt_addr(&addr), c.psm, c.lcid, c.rcid, c.rmtu);
        match c.psm {
            PSM_SDP => {
                if self.links[li].sdp_tid == 0 {
                    self.sdp_request(li, &[]);
                }
            }
            PSM_HID_CTRL => {
                // HIDP: whoever opened control opens interrupt. A device that opened control will
                // open interrupt itself; a control channel WE opened is followed by ours.
                if c.ours {
                    self.open_chan(li, PSM_HID_INTR, now);
                }
            }
            PSM_HID_INTR => {
                if self.links[li].open_rcid(PSM_HID_CTRL).is_some() {
                    self.hid_connected(li);
                }
            }
            _ => {}
        }
        if c.psm == PSM_HID_CTRL && self.links[li].open_rcid(PSM_HID_INTR).is_some() {
            self.hid_connected(li);
        }
    }

    // ── SDP: the report descriptor (HIDDescriptorList, attribute 0x0206) ────────────────────────

    fn sdp_request(&mut self, li: usize, cont: &[u8]) {
        let Some(ci) = self.links[li].chan_psm(PSM_SDP) else { return };
        let rcid = self.links[li].chans[ci].rcid;
        self.links[li].sdp_tid = self.links[li].sdp_tid.wrapping_add(1).max(1);
        let tid = self.links[li].sdp_tid;
        let mut params = Vec::with_capacity(32);
        params.extend_from_slice(&[0x35, 0x03, 0x19, 0x11, 0x24]); // DES { UUID16 0x1124 HID }
        params.extend_from_slice(&0x0280u16.to_be_bytes()); // MaximumAttributeByteCount
        params.extend_from_slice(&[0x35, 0x03, 0x09, 0x02, 0x06]); // DES { attr 0x0206 }
        params.push(cont.len() as u8);
        params.extend_from_slice(cont);
        let mut pdu = Vec::with_capacity(params.len() + 5);
        pdu.push(0x06); // SDP_ServiceSearchAttributeRequest
        pdu.extend_from_slice(&tid.to_be_bytes());
        pdu.extend_from_slice(&(params.len() as u16).to_be_bytes());
        pdu.extend_from_slice(&params);
        let h = self.links[li].handle;
        self.l2_send(h, rcid, &pdu);
    }

    fn on_sdp(&mut self, li: usize, b: &[u8], now: u64) {
        let addr = self.links[li].addr;
        if b.len() < 5 {
            return;
        }
        if b[0] != 0x07 {
            serial_println!(":: BTHID: sdp {} answered pdu={:#04x} (error {:#06x}) — no descriptor; boot protocol fallback ::", fmt_addr(&addr), b[0], if b.len() >= 7 { u16::from_be_bytes([b[5], b[6]]) } else { 0 });
            self.finish_sdp(li, now);
            return;
        }
        if b.len() < 7 {
            return;
        }
        let cnt = u16::from_be_bytes([b[5], b[6]]) as usize;
        if 7 + cnt >= b.len() + 1 {
            self.finish_sdp(li, now);
            return;
        }
        let lists = &b[7..(7 + cnt).min(b.len())];
        if self.links[li].sdp_acc.len() + lists.len() <= SDP_MAX {
            self.links[li].sdp_acc.extend_from_slice(lists);
        }
        let ci = 7 + cnt;
        let clen = b.get(ci).copied().unwrap_or(0) as usize;
        if clen > 0 && ci + 1 + clen <= b.len() && self.links[li].sdp_rounds < 8 {
            self.links[li].sdp_rounds += 1;
            let cont: Vec<u8> = b[ci + 1..ci + 1 + clen].to_vec();
            self.sdp_request(li, &cont);
            return;
        }
        let acc = core::mem::take(&mut self.links[li].sdp_acc);
        match sdp_hid_descriptor(&acc) {
            Some((o, n)) => {
                let desc = acc[o..o + n].to_vec();
                serial_println!(":: BTHID: sdp {} hid descriptor len={} rounds={} ::", fmt_addr(&addr), n, self.links[li].sdp_rounds + 1);
                if let Some(bd) = self.bonds.iter_mut().find(|x| x.addr == addr) {
                    bd.desc = desc.clone();
                    bd.dirty = true;
                    STORE_DIRTY.store(true, Ordering::Release);
                }
                let was_boot = self.links[li].map.boot;
                self.links[li].map = unsafe { build_map(&desc) };
                self.links[li].map.boot = was_boot; // still boot until after_sdp switches the device to report mode
                self.witness_map(li, n);
            }
            None => {
                serial_println!(":: BTHID: sdp {} carried no HIDDescriptorList (attribute 0x0206) in {} bytes — boot protocol fallback ::", fmt_addr(&addr), acc.len());
            }
        }
        self.finish_sdp(li, now);
    }

    fn finish_sdp(&mut self, li: usize, now: u64) {
        self.links[li].sdp_done = true;
        if let Some(ci) = self.links[li].chan_psm(PSM_SDP) {
            self.close_chan(li, ci, now);
        }
        self.after_sdp(li, now);
    }

    fn after_sdp(&mut self, li: usize, now: u64) {
        let l = &self.links[li];
        if l.hid_up {
            // HID came up first (an inbound device): switch out of boot mode if a map now exists.
            if l.map.boot && (l.map.ptr.is_some() || l.map.kbd_rid.is_some()) {
                if let Some(rc) = l.open_rcid(PSM_HID_CTRL) {
                    let h = l.handle;
                    self.links[li].map.boot = false;
                    self.l2_send(h, rc, &[0x71]); // SET_PROTOCOL report
                }
            }
            return;
        }
        if l.outbound || l.chan_psm(PSM_HID_CTRL).is_none() {
            self.open_chan(li, PSM_HID_CTRL, now);
        }
    }

    fn witness_map(&self, li: usize, n: usize) {
        let l = &self.links[li];
        let m = &l.map;
        match m.ptr {
            Some(p) => serial_println!(":: BTHID: hid {} layout (shared parse_report_descriptor) pointer rid={} {} x={}/{} y={}/{} buttons={}@{} wheel={}/{} keyboard_rid={} boot_compatible={} desc_len={} ::", fmt_addr(&l.addr), p.rid, if p.l.relative { "rel" } else { "abs" }, p.l.x_off, p.l.x_size, p.l.y_off, p.l.y_size, p.l.btn_count, p.l.btn_off, p.wheel_off, p.wheel_size, m.kbd_rid.map(|r| r as i32).unwrap_or(-1), m.kbd_boot_ok, n),
            None => serial_println!(":: BTHID: hid {} layout pointer=none keyboard_rid={} boot_compatible={} desc_len={} ::", fmt_addr(&l.addr), m.kbd_rid.map(|r| r as i32).unwrap_or(-1), m.kbd_boot_ok, n),
        }
    }

    fn hid_connected(&mut self, li: usize) {
        if self.links[li].hid_up {
            return;
        }
        self.links[li].hid_up = true;
        self.connected += 1;
        let addr = self.links[li].addr;
        if self.links[li].map.ptr.is_none() && self.links[li].map.kbd_rid.is_none() {
            if let Some(d) = self.desc_for(&addr) {
                self.links[li].map = unsafe { build_map(&d) };
                self.witness_map(li, d.len());
            }
        }
        let have_map = self.links[li].map.ptr.is_some() || self.links[li].map.kbd_rid.is_some();
        if !have_map {
            // HIDP boot protocol: ID 1 keyboard, ID 2 mouse, fixed layouts. Asked for until the
            // descriptor arrives (an inbound device may open HID before our SDP answers).
            self.links[li].map.boot = true;
            if let Some(rc) = self.links[li].open_rcid(PSM_HID_CTRL) {
                let h = self.links[li].handle;
                self.l2_send(h, rc, &[0x70]); // SET_PROTOCOL boot
            }
        }
        let rc = self.links[li].reconnect;
        serial_println!(":: BTHID: hid {} CONNECTED mode={} (control 0x11 + interrupt 0x13 open){} ::", fmt_addr(&addr), if have_map { "report" } else { "boot" }, if rc { " via reconnect" } else { "" });
    }

    fn on_hid_intr(&mut self, li: usize, b: &[u8]) {
        if b.len() < 2 || b[0] != 0xA1 {
            return; // only DATA|Input carries a report
        }
        let report = &b[1..];
        let l = &mut self.links[li];
        l.reports += 1;
        self.hid_reports += 1;
        let m = l.map;
        if m.boot {
            match report[0] {
                1 if report.len() >= 9 => kbd_report(l, &report[1..9], 1),
                2 if report.len() >= 4 => {
                    let btn = report[1];
                    let dx = report[2] as i8 as i32;
                    let dy = report[3] as i8 as i32;
                    let w = report.get(4).copied().unwrap_or(0) as i8;
                    ptr_deliver(l, btn, dx, dy, true, w);
                }
                _ => {}
            }
        } else {
            let rid = report[0];
            if let Some(p) = m.ptr {
                if p.rid == 0 || p.rid == rid {
                    let (x, y, buttons, _f) = decode_report_pointer(report, &p.l);
                    let body = if p.rid != 0 { &report[1..] } else { report };
                    let w = if p.wheel_size > 0 { sign_extend(extract_bits(body, p.wheel_off, p.wheel_size), p.wheel_size).clamp(-127, 127) as i8 } else { 0 };
                    ptr_deliver(l, buttons, x, y, p.l.relative, w);
                }
            }
            if let Some(k) = m.kbd_rid {
                let body = if k != 0 && rid == k { &report[1..] } else if k == 0 { report } else { &[][..] };
                if body.len() >= 8 && m.kbd_boot_ok {
                    kbd_report(l, &body[..8], k);
                }
            }
        }
        let l = &self.links[li];
        if l.reports == 1 || l.reports % 64 == 0 {
            serial_println!(":: BTHID: hid {} reports={} last_len={} first_byte={:#04x} ::", fmt_addr(&l.addr), l.reports, report.len(), report[0]);
        }
        // An LED change from the keyboard decode rides the interrupt channel as DATA|Output.
        if let Some(leds) = PENDING_LED.with(|v| v.take()) {
            let (h, rc) = (l.handle, l.open_rcid(PSM_HID_INTR));
            if let Some(rc) = rc {
                let rid = l.map.kbd_rid.unwrap_or(1);
                if rid != 0 {
                    self.l2_send(h, rc, &[0xA2, rid, leds]);
                } else {
                    self.l2_send(h, rc, &[0xA2, leds]);
                }
            }
        }
    }

    // ── Timers ──────────────────────────────────────────────────────────────────────────────────

    fn timers(&mut self, now: u64) {
        if self.scan == Scan::Inquiring && now.saturating_sub(self.scan_t0) > INQUIRY_HOST_MS + 1000 {
            serial_println!(":: BTHID: inquiry host deadline ({} ms) passed with no Inquiry Complete -> Inquiry_Cancel ::", INQUIRY_HOST_MS);
            self.push_cmd(H_INQUIRY_CANCEL, &[]);
            self.scan = Scan::Naming;
            self.next_name(now);
        }
        if let Some((a, t)) = self.naming {
            if now.saturating_sub(t) > RNR_HOST_MS {
                serial_println!(":: BTHID: name {} host deadline {} ms passed -> Remote_Name_Request_Cancel ::", fmt_addr(&a), RNR_HOST_MS);
                self.push_cmd(H_RNR_CANCEL, &a);
                self.naming = None;
                self.next_name(now);
            }
        }
        let mut i = 0;
        while i < self.links.len() {
            let age = now.saturating_sub(self.links[i].t);
            let st = self.links[i].st;
            let addr = self.links[i].addr;
            if st == LinkSt::Paging && age > PAGE_HOST_MS {
                serial_println!(":: BTHID: page {} host deadline {} ms passed -> Create_Connection_Cancel ::", fmt_addr(&addr), PAGE_HOST_MS);
                self.push_cmd(H_CREATE_CONN_CANCEL, &addr);
                self.links[i].st = LinkSt::Closing;
                self.links[i].t = now;
            } else if matches!(st, LinkSt::Accepting | LinkSt::Authing | LinkSt::Encrypting) && age > SETUP_MS {
                if self.links[i].handle != 0 {
                    self.disconnect(i, "security setup exceeded 30 s");
                } else {
                    self.links.remove(i);
                    continue;
                }
            } else if st == LinkSt::Closing && age > 5000 {
                if self.paging == Some(addr) {
                    self.paging = None;
                }
                self.links.remove(i);
                continue;
            }
            if let Some(v) = self.links[i].confirm {
                if now.saturating_sub(self.links[i].confirm_at) > CONFIRM_MS {
                    serial_println!(":: BTHID: pair {} confirmation of {:06} not given within {} s -> refused ::", fmt_addr(&addr), v, CONFIRM_MS / 1000);
                    self.links[i].confirm = None;
                    self.push_cmd(H_CONFIRM_NEG, &addr);
                }
            }
            // Channels that never finished opening.
            let stale = self.links[i].chans.iter().position(|c| matches!(c.st, ChanSt::WaitConnRsp | ChanSt::Config | ChanSt::WaitDiscRsp) && now.saturating_sub(c.t) > CHAN_MS);
            if let Some(ci) = stale {
                let c = self.links[i].chans.remove(ci);
                serial_println!(":: BTHID: l2cap {} psm={:#06x} did not finish within {} s — abandoned ::", fmt_addr(&addr), c.psm, CHAN_MS / 1000);
                if c.psm == PSM_SDP && !self.links[i].sdp_done {
                    self.links[i].sdp_done = true;
                    self.after_sdp(i, now);
                } else if c.psm != PSM_SDP && !self.links[i].hid_up {
                    self.disconnect(i, "a HID channel never opened");
                }
            }
            // An inbound device that came up but opened no HID channel within 3 s: open it ourselves.
            if self.links[i].st == LinkSt::Up && !self.links[i].outbound && self.links[i].sdp_done && !self.links[i].hid_up && self.links[i].chan_psm(PSM_HID_CTRL).is_none() && age > 3000 {
                self.open_chan(i, PSM_HID_CTRL, now);
            }
            i += 1;
        }
        // RECONNECT at desktop-ready: the bond store has loaded (it needs the session's home) and
        // the gui stamp exists. Bonds are paged one at a time inside ONE 5 s deadline.
        match self.rc {
            Rc::Wait => {
                if self.up && self.bonds_loaded && bt_bootpace_ms("gui").is_some() {
                    if self.bonds.is_empty() {
                        self.rc = Rc::Done;
                        serial_println!(":: BTHID: reconnect — no bonded device; nothing paged ::");
                    } else {
                        self.rc = Rc::Run;
                        self.rc_deadline = now + RECONNECT_MS;
                        self.rc_next = 0;
                        serial_println!(":: BTHID: reconnect start bonds={} deadline={}ms (gui at {} ms) at={} ::", self.bonds.len(), RECONNECT_MS, bt_bootpace_ms("gui").unwrap_or(0), now);
                    }
                }
            }
            Rc::Run => {
                if now >= self.rc_deadline {
                    if let Some(a) = self.paging {
                        if let Some(li) = self.link_addr(&a) {
                            if self.links[li].reconnect && self.links[li].st == LinkSt::Paging {
                                self.push_cmd(H_CREATE_CONN_CANCEL, &a);
                                self.links[li].st = LinkSt::Closing;
                                self.links[li].t = now;
                            }
                        }
                    }
                    self.rc = Rc::Done;
                    serial_println!(":: BTHID: reconnect deadline reached — paged={} links={} ::", self.rc_tried, self.links.len());
                } else if self.paging.is_none() && self.scan == Scan::Idle {
                    while self.rc_next < self.bonds.len() {
                        let a = self.bonds[self.rc_next].addr;
                        self.rc_next += 1;
                        if self.link_addr(&a).is_none() {
                            self.rc_tried += 1;
                            let _ = self.start_connect(a, false, true, now);
                            return;
                        }
                    }
                    self.rc = Rc::Done;
                    serial_println!(":: BTHID: reconnect done — paged={} links={} ::", self.rc_tried, self.links.len());
                }
            }
            Rc::Done => {}
        }
    }
}

/// The LED byte `decode_boot_keyboard` asked to light, handed from the decode (which borrows the
/// link) to the send (which borrows the state). One slot; a pass delivers it immediately.
struct LedSlot(crate::sync::Mutex<Option<u8>>);
impl LedSlot {
    fn with<R>(&self, f: impl FnOnce(&mut Option<u8>) -> R) -> R {
        f(&mut self.0.lock())
    }
}
static PENDING_LED: LedSlot = LedSlot(crate::sync::Mutex::new(None));

fn ptr_deliver(l: &mut Link, buttons: u8, x: i32, y: i32, relative: bool, wheel: i8) {
    let changed = buttons != l.prev_btn;
    l.prev_btn = buttons;
    let motion = if x == 0 && y == 0 {
        None
    } else if relative {
        Some(crate::pal::Event::Mouse { x, y })
    } else {
        Some(crate::pal::Event::MouseAbsolute { x, y })
    };
    crate::pal::push_pointer_report(motion, if changed { Some(crate::pal::Event::Button(buttons)) } else { None });
    if wheel != 0 {
        crate::pal::push_event(crate::pal::Event::Wheel(wheel));
    }
}

fn kbd_report(l: &mut Link, body8: &[u8], _rid: u8) {
    let mut r = [0u8; 8];
    r.copy_from_slice(body8);
    // Ctrl+Alt+B on THIS keyboard would ask the legacy chain to reset the radio this keyboard rides
    // on; the key is masked here so the BT keyboard cannot cut its own link.
    if r[0] & 0x11 != 0 && r[0] & 0x44 != 0 {
        for k in r[2..].iter_mut() {
            if *k == 0x05 {
                *k = 0;
            }
        }
    }
    let led = unsafe { decode_boot_keyboard(&r, &mut l.kprev, &mut l.kmods, &mut l.kleds) };
    if led {
        PENDING_LED.with(|v| *v = Some(l.kleds));
    }
}

// ── Pure parsers (driven by `tests bt` with no radio) ────────────────────────────────────────────

#[derive(Clone, Copy)]
struct InqRec {
    addr: [u8; 6],
    psrm: u8,
    cod: u32,
    clk: u16,
    rssi: Option<i8>,
    name: [u8; 32],
    name_len: u8,
}

/// Inquiry Result (0x02), with RSSI (0x22), Extended (0x2F). The arrays of 0x02 and 0x22 are laid
/// out PARAMETER-MAJOR (all BD_ADDRs, then all modes, …) as Vol 4 Part E §7.7.2/§7.7.33 size them
/// ("6 octets * Num_Responses"); with one response, the common case, both readings coincide. Every
/// record is bounds-checked against the event; a lying count truncates the walk.
fn parse_inquiry(code: u8, p: &[u8], out: &mut dyn FnMut(InqRec)) {
    if p.is_empty() {
        return;
    }
    let n = p[0] as usize;
    let mk = |addr, psrm, cod: &[u8], clk, rssi| InqRec { addr, psrm, cod: (cod[0] as u32) | ((cod[1] as u32) << 8) | ((cod[2] as u32) << 16), clk, rssi, name: [0; 32], name_len: 0 };
    match code {
        E_INQUIRY_RESULT => {
            if p.len() < 1 + 14 * n {
                return;
            }
            let (a, m, c, k) = (1, 1 + 6 * n, 1 + 9 * n, 1 + 12 * n);
            for i in 0..n {
                out(mk(addr_at(p, a + 6 * i), p[m + i], &p[c + 3 * i..c + 3 * i + 3], le16(p, k + 2 * i), None));
            }
        }
        E_INQUIRY_RESULT_RSSI => {
            if p.len() < 1 + 14 * n {
                return;
            }
            let (a, m, c, k, r) = (1, 1 + 6 * n, 1 + 8 * n, 1 + 11 * n, 1 + 13 * n);
            for i in 0..n {
                out(mk(addr_at(p, a + 6 * i), p[m + i], &p[c + 3 * i..c + 3 * i + 3], le16(p, k + 2 * i), Some(p[r + i] as i8)));
            }
        }
        E_EXT_INQUIRY_RESULT => {
            if p.len() < 15 {
                return;
            }
            let mut rec = mk(addr_at(p, 1), p[7], &p[9..12], le16(p, 12), Some(p[14] as i8));
            if let Some(nm) = eir_name(&p[15..]) {
                let k = nm.len().min(32);
                rec.name[..k].copy_from_slice(&nm[..k]);
                rec.name_len = k as u8;
            }
            out(rec);
        }
        _ => {}
    }
}

/// The device name from Extended Inquiry Response data: Complete Local Name (0x09) preferred over
/// Shortened (0x08). A structure running past the end ends the walk.
fn eir_name(eir: &[u8]) -> Option<&[u8]> {
    let mut i = 0;
    let mut short: Option<&[u8]> = None;
    while i < eir.len() {
        let len = eir[i] as usize;
        if len == 0 || i + 1 + len > eir.len() {
            break;
        }
        let t = eir[i + 1];
        let d = &eir[i + 2..i + 1 + len];
        if t == 0x09 && !d.is_empty() {
            return Some(d);
        }
        if t == 0x08 && !d.is_empty() {
            short = Some(d);
        }
        i += 1 + len;
    }
    short
}

/// Locate the report descriptor in an SDP attribute list: attribute id 0x0206 (uint16 element
/// `09 02 06`), then inside its value the first `08 22` (class descriptor type Report) followed by a
/// text-string element (`25 len8` / `26 len16`). Returns (offset, length) into `b`, bounds-checked.
fn sdp_hid_descriptor(b: &[u8]) -> Option<(usize, usize)> {
    let start = b.windows(3).position(|w| w == [0x09, 0x02, 0x06])? + 3;
    let mut i = start;
    while i + 3 < b.len() {
        if b[i] == 0x08 && b[i + 1] == 0x22 {
            let t = b[i + 2];
            let (off, len) = match t {
                0x25 => (i + 4, b[i + 3] as usize),
                0x26 if i + 4 < b.len() => (i + 5, u16::from_be_bytes([b[i + 3], b[i + 4]]) as usize),
                _ => {
                    i += 1;
                    continue;
                }
            };
            if off + len <= b.len() && len > 0 {
                return Some((off, len));
            }
            return None;
        }
        i += 1;
    }
    None
}

#[derive(Default)]
struct CfgOpts {
    mtu: Option<u16>,
    non_basic_mode: bool,
}

/// L2CAP configuration options (Vol 3 Part A §5): MTU (0x01) read; Retransmission and Flow Control
/// (0x04) with a mode other than Basic flagged; hint-bit options skipped as the hint bit means.
fn parse_cfg_opts(mut o: &[u8]) -> CfgOpts {
    let mut r = CfgOpts::default();
    while o.len() >= 2 {
        let t = o[0];
        let len = o[1] as usize;
        if 2 + len > o.len() {
            break;
        }
        let d = &o[2..2 + len];
        match t & 0x7F {
            0x01 if len >= 2 => r.mtu = Some(le16(d, 0)),
            0x04 if len >= 1 && t & 0x80 == 0 => r.non_basic_mode = d[0] != 0,
            _ => {}
        }
        o = &o[2 + len..];
    }
    r
}

/// Per-Report-ID classification of a report descriptor for the two things the shared pointer parser
/// does not map: the keyboard section (modifier bitmap at Usage Page 0x07 usages 0xE0.., then the
/// key array) and the wheel (Generic Desktop 0x38). It does not decode X/Y — that is
/// `parse_report_descriptor`'s, and stays so.
#[derive(Clone, Copy, Default)]
struct Sect {
    rid: u8,
    bits: u16,
    wheel: Option<(u16, u8)>,
    kbd_mods: Option<u16>,
    kbd_keys: Option<(u16, u8)>,
}

fn classify(desc: &[u8], out: &mut [Sect; 8]) -> usize {
    let mut n = 1usize;
    out[0] = Sect::default();
    let mut cur = 0usize;
    let (mut page, mut rsize, mut rcount) = (0u16, 0u32, 0u32);
    let mut usages: [u16; 16] = [0; 16];
    let mut nusg = 0usize;
    let (mut umin, mut umax) = (0u16, 0u16);
    let mut i = 0usize;
    while i < desc.len() {
        let b = desc[i];
        if b == 0xFE {
            break;
        }
        let size = match b & 3 {
            3 => 4,
            s => s as usize,
        };
        if i + 1 + size > desc.len() {
            break;
        }
        let mut data = 0u32;
        for k in 0..size {
            data |= (desc[i + 1 + k] as u32) << (8 * k);
        }
        match b & 0xFC {
            0x04 => page = data as u16,
            0x74 => rsize = data,
            0x94 => rcount = data,
            0x84 => {
                let rid = data as u8;
                cur = match out[..n].iter().position(|s| s.rid == rid) {
                    Some(c) => c,
                    None if n == 1 && out[0].rid == 0 && out[0].bits == 0 => {
                        out[0].rid = rid;
                        0
                    }
                    None if n < out.len() => {
                        out[n] = Sect { rid, ..Sect::default() };
                        n += 1;
                        n - 1
                    }
                    None => break,
                };
            }
            0x08 => {
                if nusg < usages.len() {
                    usages[nusg] = data as u16;
                    nusg += 1;
                }
            }
            0x18 => umin = data as u16,
            0x28 => umax = data as u16,
            0x80 => {
                let is_const = data & 1 != 0;
                let is_var = data & 2 != 0;
                let count = rcount.min(512);
                let off = out[cur].bits;
                if !is_const {
                    if is_var {
                        for j in 0..count {
                            let u = if (j as usize) < nusg { usages[j as usize] } else if nusg > 0 { usages[nusg - 1] } else if umax >= umin { umin.wrapping_add(j as u16) } else { 0 };
                            let f = off.saturating_add((j as u16).saturating_mul(rsize as u16));
                            if page == 0x01 && u == 0x38 && out[cur].wheel.is_none() {
                                out[cur].wheel = Some((f, rsize as u8));
                            }
                        }
                        if page == 0x07 && umin == 0xE0 && rsize == 1 && out[cur].kbd_mods.is_none() {
                            out[cur].kbd_mods = Some(off);
                        }
                    } else if page == 0x07 && rsize == 8 && out[cur].kbd_keys.is_none() {
                        out[cur].kbd_keys = Some((off, count as u8));
                    }
                }
                let adv = (rsize.min(u16::MAX as u32) as u16).saturating_mul(count.min(u16::MAX as u32) as u16);
                out[cur].bits = out[cur].bits.saturating_add(adv);
                nusg = 0;
                umin = 0;
                umax = 0;
            }
            0x90 | 0xB0 => {
                nusg = 0;
                umin = 0;
                umax = 0;
            }
            _ => {}
        }
        i += 1 + size;
    }
    n
}

/// Every Report ID item's (position, id).
fn report_id_items(desc: &[u8]) -> Vec<(usize, u8)> {
    let mut v = Vec::new();
    let mut i = 0usize;
    while i < desc.len() {
        let b = desc[i];
        if b == 0xFE {
            break;
        }
        let size = match b & 3 {
            3 => 4,
            s => s as usize,
        };
        if i + 1 + size > desc.len() {
            break;
        }
        if b & 0xFC == 0x84 && size >= 1 {
            v.push((i, desc[i + 1]));
        }
        i += 1 + size;
    }
    v
}

/// The HID map. The pointer comes from the SHARED `parse_report_descriptor` (the EHCI-HID USB path's
/// parser), one Report ID at a time: that parser keeps a single report id (the last it saw) and
/// restarts its bit offset at each Report ID item, so handing it the descriptor PREFIX that ends at
/// the next Report ID item yields exactly the field map of the section that contains X/Y — the first
/// prefix whose parse gains X/Y is the pointer's section. Keyboard and wheel from [`classify`].
unsafe fn build_map(desc: &[u8]) -> HidMap {
    let mut m = HidMap::default();
    let ids = report_id_items(desc);
    if ids.is_empty() {
        if let Some(l) = parse_report_descriptor(desc) {
            if l.has_xy {
                m.ptr = Some(PtrMap { rid: 0, l, wheel_off: 0, wheel_size: 0 });
            }
        }
    } else {
        for i in 0..ids.len() {
            let end = ids.get(i + 1).map(|x| x.0).unwrap_or(desc.len());
            let before = parse_report_descriptor(&desc[..ids[i].0]).map(|l| l.has_xy).unwrap_or(false);
            if before {
                break;
            }
            if let Some(l) = parse_report_descriptor(&desc[..end]) {
                if l.has_xy && l.report_id == ids[i].1 {
                    m.ptr = Some(PtrMap { rid: ids[i].1, l, wheel_off: 0, wheel_size: 0 });
                    break;
                }
            }
        }
    }
    let mut s = [Sect::default(); 8];
    let n = classify(desc, &mut s);
    if let Some(p) = m.ptr.as_mut() {
        if let Some(x) = s[..n].iter().find(|x| x.rid == p.rid) {
            if let Some((o, z)) = x.wheel {
                p.wheel_off = o;
                p.wheel_size = z;
            }
        }
    }
    if let Some(x) = s[..n].iter().find(|x| x.kbd_mods.is_some()) {
        m.kbd_rid = Some(x.rid);
        m.kbd_boot_ok = x.kbd_mods == Some(0) && matches!(x.kbd_keys, Some((16, c)) if c >= 6);
    }
    m
}

// ── The state, the boot bring-up, the per-pass pump ──────────────────────────────────────────────

static BT: crate::sync::Mutex<Option<Box<Bt>>> = crate::sync::Mutex::new(None);
static STORE_DIRTY: AtomicBool = AtomicBool::new(false);
static STORE_LOADED: AtomicBool = AtomicBool::new(false);
static TESTS_REGISTERED: AtomicBool = AtomicBool::new(false);

fn hci_sync(c: &mut Controller, radio: &BtRadio, e: &BtEvtEp, tog: &mut bool, armed: &mut bool, op: u16, params: &[u8], out: &mut [u8]) -> Option<u8> {
    let r = unsafe { c.bt_hci_command(&radio.target, radio.intf, e, tog, op, params, out, armed) };
    let st = match r {
        Some(n) if n >= 1 => Some(out[0]),
        _ => None,
    };
    match st {
        Some(s) => serial_println!(":: bthid: HCI {} ({:#06x}) CmdComplete status={:#04x} ::", op_name(op), op, s),
        None => serial_println!(":: bthid: HCI {} ({:#06x}) NO-RESPONSE (bounded wait expired) ::", op_name(op), op),
    }
    st
}

/// BTHID bring-up, called from the boot campaign's post-GUI drain (`bt_drain_boot_campaign`) in
/// place of the legacy chain under `btc`, and again by `bt reset`. Synchronous and bounded: each
/// command waits at most one hardware budget, and a radio that does not answer HCI_Reset stops here.
pub(super) unsafe fn boot_bringup(c: &mut Controller, radio: &BtRadio, e: &BtEvtEp) {
    let t0 = ms();
    serial_println!(":: BTHID: bring-up at {} ms on controller [{}] addr={} — under btc this REPLACES the legacy LE-scan/inquiry/page chain at boot (it stays on Ctrl+Alt+B); nothing is paged here ::", t0, c.idx, radio.target.addr);
    c.bt_quiesce_events(e);
    let _ = c.bt_resync_device_toggles(radio, true);
    let mut tog = false;
    let mut armed = false;
    let mut out = [0u8; 16];
    let fail = |why: &str| serial_println!(":: BTHID: bring-up STOPPED — {} ; the radio is left idle and `bt reset` retries ::", why);
    if hci_sync(c, radio, e, &mut tog, &mut armed, H_RESET, &[], &mut out) != Some(0) {
        c.bt_quiesce_events(e);
        return fail("HCI_Reset did not complete with status 0x00");
    }
    let mut bd = [0u8; 6];
    if hci_sync(c, radio, e, &mut tog, &mut armed, H_READ_BD_ADDR, &[], &mut out) == Some(0) {
        bd.copy_from_slice(&out[1..7]);
    }
    let (mut acl_len, mut acl_num) = (0u16, 0u16);
    if hci_sync(c, radio, e, &mut tog, &mut armed, H_READ_BUFFER_SIZE, &[], &mut out) == Some(0) {
        acl_len = le16(&out, 1);
        acl_num = le16(&out, 4);
    }
    if hci_sync(c, radio, e, &mut tog, &mut armed, H_SET_EVENT_MASK, &EVENT_MASK.to_le_bytes(), &mut out) != Some(0) {
        return fail("HCI_Set_Event_Mask refused");
    }
    let ssp = hci_sync(c, radio, e, &mut tog, &mut armed, H_WRITE_SSP_MODE, &[0x01], &mut out) == Some(0);
    if hci_sync(c, radio, e, &mut tog, &mut armed, H_WRITE_INQUIRY_MODE, &[0x02], &mut out) != Some(0) {
        let _ = hci_sync(c, radio, e, &mut tog, &mut armed, H_WRITE_INQUIRY_MODE, &[0x01], &mut out);
    }
    let _ = hci_sync(c, radio, e, &mut tog, &mut armed, H_WRITE_COD, &[0x0C, 0x01, 0x00], &mut out); // Computer / Laptop
    let mut name = [0u8; 248];
    name[..5].copy_from_slice(b"UnaOS");
    let _ = hci_sync(c, radio, e, &mut tog, &mut armed, H_WRITE_LOCAL_NAME, &name, &mut out);
    let _ = hci_sync(c, radio, e, &mut tog, &mut armed, H_WRITE_PAGE_TIMEOUT, &PAGE_TIMEOUT_SLOTS.to_le_bytes(), &mut out);
    // Page scan only: a bonded keyboard or mouse can page US to reconnect; inquiry scan stays off,
    // so this machine is connectable by its bonds and not discoverable by the room.
    let ps = hci_sync(c, radio, e, &mut tog, &mut armed, H_WRITE_SCAN_ENABLE, &[0x02], &mut out) == Some(0);
    let mut g = BT.lock();
    let bt = g.get_or_insert_with(|| Box::new(Bt::new()));
    bt.up = acl_num > 0 && acl_len > 0;
    bt.idx = c.idx;
    bt.bd = bd;
    bt.acl_len = acl_len;
    bt.acl_num = acl_num;
    bt.credits = acl_num;
    bt.evt_toggle = tog;
    bt.evt_armed = armed;
    bt.inflight = None;
    bt.cq_len = 0;
    bt.links.clear();
    bt.paging = None;
    bt.scan = Scan::Idle;
    bt.naming = None;
    bt.acl_rx.clear();
    bt.acltx.clear();
    if bt.up {
        serial_println!(":: BTHID: up bd_addr={} acl={}x{} ssp={} page_scan={} at={} took={}ms ::", fmt_addr(&bd), acl_len, acl_num, if ssp { "on" } else { "REFUSED" }, if ps { "on" } else { "REFUSED" }, ms(), ms().saturating_sub(t0));
    } else {
        serial_println!(":: BTHID: bring-up STOPPED — Read_Buffer_Size gave no ACL buffers (len={} num={}); no ACL traffic is possible ::", acl_len, acl_num);
    }
}

/// Once per `service_ehci_hid` pass, under the `EHCI_HID` lock. Cheap when idle: a `try_lock`, one
/// event-token read, and nothing else.
pub(super) fn pump(ctrls: &mut [Controller]) {
    if !TESTS_REGISTERED.swap(true, Ordering::AcqRel) {
        crate::tests::register("bt", fixture);
    }
    let Some(mut g) = BT.try_lock() else { return };
    let Some(bt) = g.as_mut() else { return };
    // The Ctrl+Alt+B legacy chain resets the radio under us; say so once and stand down.
    if bt.up && BT_RETRIGGER_PENDING.load(Ordering::Relaxed) != 0 {
        bt.up = false;
        serial_println!(":: BTHID: the legacy chain (Ctrl+Alt+B) is about to reset the radio — BTHID stands down; `bt reset` brings it back ::");
        return;
    }
    let want_reset = RESET_REQ.swap(false, Ordering::AcqRel);
    let Some(c) = ctrls.iter_mut().find(|c| c.idx == bt.idx && c.bt_radio.is_some()) else { return };
    let Some(radio) = c.bt_radio else { return };
    let Some(e) = (unsafe { c.bt_evt_ep_current(radio.evt_mps) }) else { return };
    if want_reset {
        drop(g);
        unsafe { boot_bringup(c, &radio, &e) };
        return;
    }
    if !bt.up {
        return;
    }
    unsafe { step(bt, c, &radio, &e) };
}

static RESET_REQ: AtomicBool = AtomicBool::new(false);

unsafe fn step(bt: &mut Bt, c: &mut Controller, radio: &BtRadio, e: &BtEvtEp) {
    let now = ms();
    bt.timers(now);
    // One HCI command in flight; a command the controller never answers is released after 2 s.
    let send_next = |bt: &mut Bt, c: &mut Controller| {
        if let Some((op, t)) = bt.inflight {
            if ms().saturating_sub(t) < 2000 {
                return;
            }
            serial_println!(":: bthid: HCI {} ({:#06x}) NO-RESPONSE within 2000 ms — released ::", op_name(op), op);
            bt.inflight = None;
        }
        if let Some(cmd) = bt.pop_cmd() {
            if c.bt_hci_send(&radio.target, radio.intf, cmd.op, &cmd.p[..cmd.len as usize]) {
                bt.inflight = Some((cmd.op, ms()));
            }
        }
    };
    send_next(bt, c);
    let mut asm = [0u8; BT_EVT_ASM_MAX];
    for _ in 0..8 {
        match c.bt_read_full_event(e, &mut bt.evt_toggle, &mut bt.evt_armed, 0, Controller::bt_l3_budget(20), Controller::bt_l3_budget(40), &mut asm) {
            BtEvt::Got { len, .. } => {
                if len >= 2 {
                    bt.on_event(&asm[..len], ms());
                    send_next(bt, c);
                }
            }
            BtEvt::Idle(_) => break,
            BtEvt::Stop => {
                bt.up = false;
                serial_println!(":: BTHID: the HCI event endpoint stopped (halt or a mid-event timeout) — BTHID is DOWN; `bt reset` re-runs the bring-up ::");
                return;
            }
        }
    }
    // ACL receive, rate-limited, only while a link has a handle.
    let (bulk_in, bulk_out, in_mps, out_mps) = c.bt_acl;
    if bulk_in == 0 || bulk_out == 0 || in_mps == 0 || out_mps == 0 {
        return;
    }
    let linked = bt.links.iter().any(|l| l.handle != 0);
    if linked && now >= bt.acl_next_poll {
        bt.acl_next_poll = now + 2;
        for _ in 0..6 {
            if bt.acl_rx.len() + in_mps as usize > ACL_RX_MAX {
                serial_println!(":: BTHID: ACL reassembly overrun at {} bytes — partial packet dropped ::", bt.acl_rx.len());
                bt.acl_rx.clear();
            }
            let budget = if bt.acl_rx.is_empty() { Controller::bt_l3_budget(1) / 4 } else { Controller::bt_l3_budget(5) };
            match c.bt_acl_txn(&radio.target, bulk_in, true, in_mps, in_mps as u32, c.bt_acl_tog.1, budget) {
                Ok((n, next)) => {
                    c.bt_acl_tog.1 = next;
                    for i in 0..n as usize {
                        bt.acl_rx.push(c.data_buf.add(i).read());
                    }
                    if bt.acl_rx.len() >= 4 {
                        let want = 4 + le16(&bt.acl_rx, 2) as usize;
                        if bt.acl_rx.len() >= want {
                            let pkt: Vec<u8> = core::mem::take(&mut bt.acl_rx);
                            bt.on_acl(&pkt[..want], ms());
                            continue;
                        }
                    }
                    if n == 0 && bt.acl_rx.is_empty() {
                        continue;
                    }
                }
                Err("nodata") => break,
                Err(why) => {
                    // A halted bulk-IN: clear it (USB 2.0 §9.4.5 resets both toggles to DATA0).
                    let ok = c.control(&radio.target, 0x02, 0x01, 0x0000, (bulk_in | 0x80) as u16, 0, false).is_ok();
                    c.bt_acl_tog.1 = false;
                    bt.acl_rx.clear();
                    serial_println!(":: BTHID: ACL IN{} {} -> ClearFeature(ENDPOINT_HALT) {} ::", bulk_in, why, if ok { "OK" } else { "FAILED" });
                    break;
                }
            }
        }
    }
    // ACL transmit, within the controller's buffer credits; one PDU per ACL packet.
    let mut sent = 0;
    while sent < 4 && bt.credits > 0 && !bt.acltx.is_empty() {
        let (h, pdu) = bt.acltx.remove(0);
        let total = pdu.len() + 4;
        if total > 256 || pdu.len() > bt.acl_len as usize {
            serial_println!(":: BTHID: ACL PDU of {} bytes exceeds one packet (limit {}) — not sent ::", pdu.len(), (bt.acl_len as usize).min(252));
            continue;
        }
        let hdr = (h & 0x0FFF) | (0b10 << 12);
        let head = [hdr as u8, (hdr >> 8) as u8, pdu.len() as u8, (pdu.len() >> 8) as u8];
        for (i, &b) in head.iter().chain(pdu.iter()).enumerate() {
            c.data_buf.add(i).write(b);
        }
        match c.bt_acl_txn(&radio.target, bulk_out, false, out_mps, total as u32, c.bt_acl_tog.0, Controller::bt_l3_budget(50)) {
            Ok((moved, next)) => {
                c.bt_acl_tog.0 = next;
                bt.credits -= 1;
                if moved as usize != total {
                    serial_println!(":: BTHID: ACL OUT short {}/{} bytes ::", moved, total);
                }
            }
            Err(why) => {
                serial_println!(":: BTHID: ACL OUT{} failed ({}) — PDU lost ::", bulk_out, why);
            }
        }
        sent += 1;
    }
}

// ── The bond store (attributes), outside the EHCI lock ──────────────────────────────────────────

fn store_dir() -> Option<String> {
    match crate::prefs::home() {
        Some(h) => Some(format!("{}/.config/unaos/bt", h)),
        None if cfg!(feature = "login") => None, // no session yet: there is no home to read
        None => Some(String::from("/.config/unaos/bt")),
    }
}

/// From the storage-ready passes in `main.rs`. Loads the bonds once (when a home exists and the
/// root pass is open), then writes dirty bonds. DEFERS while `EHCI_HID` is held, so the I/O never
/// runs inside a service pass.
pub fn store_service() {
    let need_load = !STORE_LOADED.load(Ordering::Acquire);
    if !need_load && !STORE_DIRTY.load(Ordering::Acquire) && !seal_due() {
        return;
    }
    if EHCI_HID.is_locked() {
        return;
    }
    let Some(dir) = store_dir() else { return };
    if need_load {
        if !crate::fs::bootdisk::root_pass_open("bthid-store") {
            return;
        }
        STORE_LOADED.store(true, Ordering::Release);
        let loaded = store_load(&dir);
        let mut g = BT.lock();
        let bt = g.get_or_insert_with(|| Box::new(Bt::new()));
        for b in loaded {
            if bt.bond(&b.addr).is_none() && bt.bonds.len() < MAX_BONDS {
                bt.bonds.push(b);
            }
        }
        bt.bonds_loaded = true;
        serial_println!(":: BTHID: store loaded bonds={} sealed_pending={} from {} ::", bt.bonds.len(), PENDING.lock().len(), dir);
        drop(g);
        seal_pass(&dir);
        return;
    }
    STORE_DIRTY.store(false, Ordering::Release);
    let (dirty, gone): (Vec<Bond>, Vec<[u8; 6]>) = {
        let mut g = BT.lock();
        let Some(bt) = g.as_mut() else { return };
        let d: Vec<Bond> = bt.bonds.iter().filter(|b| b.dirty).cloned().collect();
        for b in bt.bonds.iter_mut() {
            b.dirty = false;
        }
        (d, core::mem::take(&mut bt.forgotten))
    };
    for b in dirty {
        store_write(&dir, &b);
    }
    for a in gone {
        let mt = crate::shell::vfs_mount_table();
        let p = format!("{}/{}", dir, addr12(&a));
        let r = mt.unlink(&p, crate::fs::vfs::KERNEL_PRINCIPAL);
        UNSEALED.lock().retain(|e| e.0 != a);
        let d = super::btkeyseal::delete(&addr12(&a));
        serial_println!(":: BTHID: store forget {} -> {} sealed_key={} ::", p, if r.is_ok() { "removed" } else { "absent or refused" }, d.reason());
    }
    if seal_due() {
        seal_pass(&dir);
    }
}

fn store_load(dir: &str) -> Vec<Bond> {
    use crate::fs::vfs::AttrValue;
    let mt = crate::shell::vfs_mount_table();
    let mut v = Vec::new();
    let Ok(ents) = mt.read_dir(dir) else { return v };
    for ent in ents {
        let Some(addr) = parse_addr(&ent.name) else { continue };
        let p = format!("{}/{}", dir, ent.name);
        let Ok(attrs) = mt.list_attrs(&p, crate::fs::vfs::KERNEL_PRINCIPAL) else {
            serial_println!(":: BTHID: store {} has no readable attributes — skipped ::", p);
            continue;
        };
        let mut b = Bond { addr, key: [0; 16], ktype: 0, cod: 0, name: String::new(), desc: Vec::new(), dirty: false };
        let mut plain = false;
        for (k, val) in attrs {
            match (k.as_str(), val) {
                ("bt.linkkey", AttrValue::Blob(mut x)) if x.len() == 16 => {
                    b.key.copy_from_slice(&x);
                    holocron_wipe(&mut x);
                    plain = true;
                }
                ("bt.keytype", AttrValue::Int(t)) => b.ktype = t as u8,
                ("bt.class", AttrValue::Int(t)) => b.cod = t as u32,
                ("bt.name", AttrValue::Str(s)) => b.name = s,
                ("bt.hiddesc", AttrValue::Blob(x)) => b.desc = x,
                _ => {}
            }
        }
        if plain {
            // BTKEYSEAL MIGRATION: the plain key is read ONCE (the bond works this boot), queued to be
            // sealed, and its attribute removed only after Holocron answered OK.
            seal_queue(b.addr, true);
            v.push(b);
            continue;
        }
        match super::btkeyseal::get(&addr12(&addr)) {
            Ok(mut k) => {
                b.key = k;
                holocron_wipe(&mut k);
                serial_println!(":: BTKEYSEAL: unseal {} -> ok ::", fmt_addr(&addr));
                v.push(b);
            }
            Err(super::btkeyseal::Answer::NotFound) => {
                serial_println!(":: BTKEYSEAL: unseal {} -> not-found (no sealed key: `bt pair {}` again) ::", fmt_addr(&addr), fmt_addr(&addr));
            }
            Err(_) => PENDING.lock().push(b), // locked / no Holocron yet: the seal pass retries
        }
    }
    v
}

fn store_write(dir: &str, b: &Bond) {
    use crate::fs::vfs::{AttrValue, NodeKind, KERNEL_PRINCIPAL as K};
    let mt = crate::shell::vfs_mount_table();
    let base = &dir[..dir.len() - "/.config/unaos/bt".len()];
    for d in [format!("{}/.config", base), format!("{}/.config/unaos", base), String::from(dir)] {
        if mt.stat(&d).is_err() {
            let _ = mt.create(&d, NodeKind::Dir, K);
        }
    }
    let p = format!("{}/{}", dir, addr12(&b.addr));
    let line = format!("addr={} class={:#08x} kind={} name={} keytype={:#04x}\n", fmt_addr(&b.addr), b.cod, cod_kind(b.cod), b.name, b.ktype);
    let _ = mt.unlink(&p, K);
    let file = mt.create(&p, NodeKind::File, K).and_then(|_| mt.write(&p, 0, line.as_bytes(), K));
    if let Err(e) = file {
        serial_println!(":: BTHID: store {} create/write REFUSED ({:?}) — the key is held in RAM for this session ::", p, e);
        return;
    }
    // BTKEYSEAL (B446): the key is NEVER written here — it is queued for Holocron (sealed on this pass).
    seal_queue(b.addr, false);
    SEAL_RETRY_AT.store(0, Ordering::Release);
    let mut res = mt.set_attr(&p, "bt.keytype", AttrValue::Int(b.ktype as i64), K);
    if res.is_ok() {
        let _ = mt.set_attr(&p, "bt.class", AttrValue::Int(b.cod as i64), K);
        if !b.name.is_empty() {
            let _ = mt.set_attr(&p, "bt.name", AttrValue::Str(b.name.clone()), K);
        }
        if !b.desc.is_empty() && b.desc.len() <= crate::fs::vfs::ATTR_VALUE_MAX {
            res = mt.set_attr(&p, "bt.hiddesc", AttrValue::Blob(b.desc.clone()), K);
        }
    }
    match res {
        Ok(()) => serial_println!(":: BTHID: store wrote {} attrs=bt.keytype,bt.class,bt.name{} key=holocron:bt/{} -> OK ::", p, if b.desc.is_empty() { "" } else { ",bt.hiddesc" }, addr12(&b.addr)),
        Err(e) => serial_println!(":: BTHID: store {} attributes REFUSED ({:?}; a FAT volume carries no typed attributes) — the key still goes to Holocron ::", p, e),
    }
}

// ── The verb ────────────────────────────────────────────────────────────────────────────────────

fn resolve(bt: &Bt, s: &str) -> Option<[u8; 6]> {
    if let Some(n) = s.strip_prefix('#') {
        let k: usize = n.parse().ok()?;
        return bt.devs.iter().filter(|d| d.used).nth(k.checked_sub(1)?).map(|d| d.addr);
    }
    parse_addr(s)
}

/// `bt` | `bt list` | `bt scan` | `bt pair <addr|#n> [yes|no]` | `bt connect <addr|#n>` |
/// `bt disconnect <addr>` | `bt forget <addr>` | `bt reset`.
pub fn verb(args: &[&str], out: &mut dyn FnMut(&str)) {
    let sub = args.first().copied().unwrap_or("list");
    if sub == "reset" {
        RESET_REQ.store(true, Ordering::Release);
        return out("bt: bring-up queued for the next service pass (watch `bt` for `up`)");
    }
    let mut g = BT.lock();
    let Some(bt) = g.as_mut() else {
        return out("bt: no Bluetooth radio is up — the boot campaign claims it after the desktop (serial `bt-l0`/`BTHID:` lines say why)");
    };
    let now = ms();
    match sub {
        "list" | "status" => list(bt, now, out),
        "scan" => match bt.start_scan(now) {
            Ok(()) => out("bt: inquiry running (10.24 s, then names one by one); `bt` shows the table"),
            Err(e) => out(&format!("bt: scan refused — {}", e)),
        },
        "pair" | "connect" => {
            let Some(a) = args.get(1).and_then(|s| resolve(bt, s)) else {
                return out(&format!("usage: bt {} <aa:bb:cc:dd:ee:ff | #n>{}", sub, if sub == "pair" { " [yes|no]" } else { "" }));
            };
            if sub == "pair" {
                if let Some(ans) = args.get(2) {
                    let yes = matches!(*ans, "yes" | "y");
                    return match bt.confirm(a, yes) {
                        Ok(v) => out(&format!("bt: {} {:06} for {}", if yes { "confirmed" } else { "rejected" }, v, fmt_addr(&a))),
                        Err(e) => out(&format!("bt: {}", e)),
                    };
                }
            }
            match bt.start_connect(a, sub == "pair", false, now) {
                Ok(()) => out(&format!("bt: {} {} — paging (5 s bound); `bt` shows progress{}", if sub == "pair" { "pairing" } else { "connecting" }, fmt_addr(&a), if sub == "pair" { ", and any number to confirm" } else { "" })),
                Err(e) => out(&format!("bt: {} refused — {}", sub, e)),
            }
        }
        "disconnect" => {
            let Some(a) = args.get(1).and_then(|s| resolve(bt, s)) else { return out("usage: bt disconnect <addr>") };
            match bt.link_addr(&a) {
                Some(li) => {
                    bt.disconnect(li, "bt disconnect");
                    out("bt: disconnecting");
                }
                None => out("bt: not connected"),
            }
        }
        "forget" => {
            let Some(a) = args.get(1).and_then(|s| resolve(bt, s)) else { return out("usage: bt forget <addr>") };
            let had = bt.bonds.len();
            bt.bonds.retain(|b| b.addr != a);
            if bt.bonds.len() != had {
                bt.forgotten.push(a);
                STORE_DIRTY.store(true, Ordering::Release);
                out(&format!("bt: forgot {} (its object is removed on the next storage pass)", fmt_addr(&a)));
            } else {
                out("bt: not bonded");
            }
        }
        _ => out("usage: bt [list] | bt scan | bt pair <addr|#n> [yes|no] | bt connect <addr|#n> | bt disconnect <addr> | bt forget <addr> | bt reset"),
    }
}

fn list(bt: &Bt, now: u64, out: &mut dyn FnMut(&str)) {
    out(&format!("bt: radio {} {}  acl={}x{}  bonds={}  links={}", fmt_addr(&bt.bd), if bt.up { "up" } else { "DOWN (bt reset)" }, bt.acl_len, bt.acl_num, bt.bonds.len(), bt.links.len()));
    match bt.scan {
        Scan::Inquiring => out(&format!("bt: inquiring — {} ms of 10240", now.saturating_sub(bt.scan_t0))),
        Scan::Naming => out("bt: asking names, one device at a time (5 s bound each)"),
        Scan::Idle => {}
    }
    let rows: Vec<&Dev> = bt.devs.iter().filter(|d| d.used).collect();
    if !rows.is_empty() {
        out("   #  ADDRESS            CLASS     KIND                     RSSI  NAME");
        for (i, d) in rows.iter().enumerate() {
            let paired = if bt.bond(&d.addr).is_some() { " [paired]" } else { "" };
            out(&format!("  {:2}  {}  {:#08x}  {:<23}  {:>4}  {}{}", i + 1, fmt_addr(&d.addr), d.cod, cod_kind(d.cod), d.rssi.map(|r| format!("{}", r)).unwrap_or_else(|| String::from("-")), d.name(), paired));
        }
    } else if bt.scan == Scan::Idle {
        out("bt: no devices seen this session — `bt scan` (put the device in pairing mode first)");
    }
    for b in bt.bonds.iter() {
        out(&format!("bt: bonded {}  {}  {}  key type {:#04x}{}", fmt_addr(&b.addr), cod_kind(b.cod), b.name, b.ktype, if b.desc.is_empty() { "" } else { "  (descriptor stored)" }));
    }
    for l in bt.links.iter() {
        out(&format!("bt: link {}  handle={:#06x}  {}  hid={}  reports={}", fmt_addr(&l.addr), l.handle, l.st.name(), if l.hid_up { if l.map.boot { "boot" } else { "report" } } else { "no" }, l.reports));
        if let Some(v) = l.confirm {
            out(&format!("bt: >>> confirm {:06} is shown on {} — type `bt pair {} yes` (or `no`)", v, fmt_addr(&l.addr), fmt_addr(&l.addr)));
        }
        if let Some(v) = l.passkey {
            let kb = bt.dev(&l.addr).map(|d| is_keyboard_cod(d.cod)).unwrap_or(true);
            out(&format!("bt: >>> type {:06} then Enter on {}{}", v, if kb { "the keyboard " } else { "" }, fmt_addr(&l.addr)));
        }
    }
}

// ── `tests bt` ──────────────────────────────────────────────────────────────────────────────────

fn fixture() {
    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut leg = |name: &str, ok: bool, note: String| {
        if ok {
            pass += 1;
        } else {
            fail += 1;
        }
        serial_println!(":: BTHID: fixture {} {} -> {} ::", name, note, if ok { "PASS" } else { "FAIL" });
    };
    // 1. Inquiry Result with RSSI, one response.
    let ev = [1u8, 0x3c, 0x2d, 0xcc, 0x26, 0xc6, 0x88, 0x01, 0x00, 0x80, 0x25, 0x00, 0x34, 0x12, 0xC3];
    let mut got: Option<InqRec> = None;
    parse_inquiry(E_INQUIRY_RESULT_RSSI, &ev, &mut |r| got = Some(r));
    let ok = matches!(got, Some(r) if fmt_addr(&r.addr) == "88:c6:26:cc:2d:3c" && r.cod == 0x002580 && r.psrm == 1 && r.clk == 0x1234 && r.rssi == Some(-61));
    leg("inquiry-rssi", ok, format!("addr/class/psrm/clock/rssi of a synthetic 0x22 event"));
    // 2. EIR name: shortened then complete; complete wins.
    let eir = [0x02, 0x01, 0x06, 0x05, 0x08, b'M', b'a', b'g', b'i', 0x0C, 0x09, b'M', b'a', b'g', b'i', b'c', b' ', b'M', b'o', b'u', b's', b'e', 0x00];
    let ok = eir_name(&eir) == Some(&b"Magic Mouse"[..]);
    leg("eir-name", ok, format!("complete name preferred over shortened"));
    // 3. SDP attribute list carrying a HIDDescriptorList.
    let desc = combo_desc();
    let mut sdp = Vec::new();
    let inner_len = 2 + 2 + desc.len(); // 08 22 25 n desc
    let list_len = 2 + inner_len;
    sdp.extend_from_slice(&[0x35, (3 + 2 + list_len) as u8, 0x09, 0x02, 0x06, 0x35, list_len as u8, 0x35, inner_len as u8, 0x08, 0x22, 0x25, desc.len() as u8]);
    sdp.extend_from_slice(&desc);
    let ok = matches!(sdp_hid_descriptor(&sdp), Some((o, n)) if &sdp[o..o + n] == &desc[..]);
    leg("sdp-descriptor", ok, format!("attribute 0x0206 -> {} descriptor bytes", desc.len()));
    // 4. Classification through the SHARED parser, and a decode through the shared decoder.
    let m = unsafe { build_map(&desc) };
    let p = m.ptr.unwrap_or_default();
    let ok_map = m.ptr.is_some() && p.rid == 2 && p.l.relative && p.l.x_off == 8 && p.l.x_size == 8 && p.l.y_off == 16 && p.l.btn_off == 0 && p.l.btn_count == 3 && p.wheel_off == 24 && p.wheel_size == 8 && m.kbd_rid == Some(1) && m.kbd_boot_ok;
    let rep = [0x02u8, 0x01, 0x05, 0xFB, 0xFF];
    let (x, y, b, _) = decode_report_pointer(&rep, &p.l);
    let w = sign_extend(extract_bits(&rep[1..], p.wheel_off, p.wheel_size), p.wheel_size);
    let ok = ok_map && x == 5 && y == -5 && b == 1 && w == -1;
    leg("descriptor-map", ok, format!("ptr rid={} x={}/{} y={} btn={}@{} wheel={}/{} kbd={:?} boot={} decode=({},{},{},{})", p.rid, p.l.x_off, p.l.x_size, p.l.y_off, p.l.btn_count, p.l.btn_off, p.wheel_off, p.wheel_size, m.kbd_rid, m.kbd_boot_ok, x, y, b, w));
    // 5. L2CAP configuration options.
    let a = parse_cfg_opts(&[0x01, 0x02, 0x30, 0x00]);
    let bmode = parse_cfg_opts(&[0x04, 0x09, 0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0x81, 0x02, 0x00, 0x00]);
    let ok = a.mtu == Some(48) && !a.non_basic_mode && bmode.non_basic_mode && bmode.mtu.is_none();
    leg("l2cap-config", ok, format!("mtu=48 parsed; streaming mode flagged; hint-bit option skipped"));
    let (up, inq, paired, conn, reps) = match BT.try_lock() {
        Some(g) => match g.as_ref() {
            Some(bt) => (bt.up, bt.inquiry_found, bt.bonds.len() as u32, bt.connected, bt.hid_reports),
            None => (false, 0, 0, 0, 0),
        },
        None => (false, 0, 0, 0, 0),
    };
    let verdict = if fail > 0 { "FAIL" } else if up { "PASS" } else { "SKIP" };
    serial_println!(":: BTHID: inquiry={} paired={} connected={} hid_reports={} -> {} ::", inq, paired, conn, reps, verdict);
    if verdict == "SKIP" {
        serial_println!(":: BTHID: SKIP = fixtures {}/{} passed but no radio is up (no `BTHID: up` this boot) ::", pass, pass + fail);
    }
}

/// A keyboard (Report ID 1, boot layout) + mouse (Report ID 2: 3 buttons, X, Y, wheel) descriptor.
fn combo_desc() -> Vec<u8> {
    alloc::vec![
        0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x01, //
        0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, //
        0x95, 0x01, 0x75, 0x08, 0x81, 0x01, //
        0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65, 0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00, //
        0xC0, //
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02, 0x09, 0x01, 0xA1, 0x00, //
        0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, //
        0x95, 0x01, 0x75, 0x05, 0x81, 0x01, //
        0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x03, 0x81, 0x06, //
        0xC0, 0xC0,
    ]
}

// ── BTKEYSEAL (rmbp-ledger B446): the link key goes to Holocron, never to the store's attributes ───────
// Every Holocron call runs from `store_service` (the storage pass, no driver lock held) through
// `super::btkeyseal`. Bonds whose key is to be sealed wait in UNSEALED (the key stays in `bt.bonds`, RAM,
// for the session); objects whose sealed key could not be opened yet wait in PENDING. A pass that meets a
// refusal stops at the first one (the reason applies to the rest) and retries after SEAL_RETRY_MS; the
// `waiting` witness prints once per reason change, never per pass.

/// `(addr, plain)`: a key to seal; `plain` = a legacy `bt.linkkey` attribute to remove after the OK.
static UNSEALED: crate::sync::Mutex<Vec<([u8; 6], bool)>> = crate::sync::Mutex::new(Vec::new());
/// Bonds whose sealed key is not open yet (Holocron locked or not running).
static PENDING: crate::sync::Mutex<Vec<Bond>> = crate::sync::Mutex::new(Vec::new());
static SEAL_RETRY_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static SEAL_REASON: crate::sync::Mutex<&'static str> = crate::sync::Mutex::new("");
const SEAL_RETRY_MS: u64 = 3000;

fn holocron_wipe(b: &mut [u8]) {
    for x in b.iter_mut() {
        unsafe { core::ptr::write_volatile(x, 0) };
    }
}

fn seal_queue(addr: [u8; 6], plain: bool) {
    let mut q = UNSEALED.lock();
    match q.iter_mut().find(|e| e.0 == addr) {
        Some(e) => e.1 |= plain,
        None => q.push((addr, plain)),
    }
}

fn seal_due() -> bool {
    (!UNSEALED.lock().is_empty() || !PENDING.lock().is_empty()) && crate::arch::ms() >= SEAL_RETRY_AT.load(Ordering::Acquire)
}

/// `(sealed_queue, plain_left, pending)` for the witnesses.
fn seal_counts() -> (usize, usize, usize) {
    let q = UNSEALED.lock();
    (q.len(), q.iter().filter(|e| e.1).count(), PENDING.lock().len())
}

fn seal_pass(dir: &str) {
    use super::btkeyseal::{self as ks, Answer};
    SEAL_RETRY_AT.store(crate::arch::ms() + SEAL_RETRY_MS, Ordering::Release);
    let mut why: Option<&'static str> = None;
    // 1. Seal what is queued.
    let queue = core::mem::take(&mut *UNSEALED.lock());
    let mut left: Vec<([u8; 6], bool)> = Vec::new();
    for (addr, plain) in queue {
        if why.is_some() {
            left.push((addr, plain));
            continue;
        }
        let snap = BT.lock().as_ref().and_then(|bt| bt.bond(&addr).map(|b| (b.key, b.name.clone())));
        let Some((mut key, name)) = snap else { continue }; // forgotten meanwhile
        let a12 = addr12(&addr);
        let label = format!("{} {}", fmt_addr(&addr), name);
        match ks::put(&a12, &label, &key) {
            Answer::Ok(_) if plain => {
                let p = format!("{}/{}", dir, a12);
                let r = crate::shell::vfs_mount_table().remove_attr(&p, "bt.linkkey", crate::fs::vfs::KERNEL_PRINCIPAL);
                serial_println!(":: BTKEYSEAL: migrate {} plain=read sealed=ok plain_removed={} ::", fmt_addr(&addr), if r.is_ok() { "ok" } else { "refused" });
                if r.is_err() {
                    left.push((addr, true));
                }
            }
            Answer::Ok(_) => serial_println!(":: BTKEYSEAL: seal {} -> sealed bt/{} ::", fmt_addr(&addr), a12),
            other => {
                why = Some(other.reason());
                left.push((addr, plain));
            }
        }
        holocron_wipe(&mut key);
    }
    for (a, p) in left {
        seal_queue(a, p);
    }
    // 2. Open what is pending.
    if why.is_none() && !PENDING.lock().is_empty() {
        let pend = core::mem::take(&mut *PENDING.lock());
        let mut got: Vec<Bond> = Vec::new();
        let mut keep: Vec<Bond> = Vec::new();
        for mut b in pend {
            if why.is_some() {
                keep.push(b);
                continue;
            }
            match ks::get(&addr12(&b.addr)) {
                Ok(mut k) => {
                    b.key = k;
                    holocron_wipe(&mut k);
                    serial_println!(":: BTKEYSEAL: unseal {} -> ok ::", fmt_addr(&b.addr));
                    got.push(b);
                }
                Err(Answer::NotFound) => serial_println!(":: BTKEYSEAL: unseal {} -> not-found (no sealed key: `bt pair` again) ::", fmt_addr(&b.addr)),
                Err(a) => {
                    why = Some(a.reason());
                    keep.push(b);
                }
            }
        }
        PENDING.lock().extend(keep);
        if !got.is_empty() {
            let mut g = BT.lock();
            let bt = g.get_or_insert_with(|| Box::new(Bt::new()));
            for b in got {
                if bt.bond(&b.addr).is_none() && bt.bonds.len() < MAX_BONDS {
                    bt.bonds.push(b);
                }
            }
            if bt.rc == Rc::Done {
                bt.rc = Rc::Wait; // the keys arrived after the reconnect ran: page the newly opened bonds
            }
        }
    }
    let r = why.unwrap_or("none");
    let mut last = SEAL_REASON.lock();
    if *last != r {
        *last = r;
        let (q, pl, pe) = seal_counts();
        serial_println!(":: BTKEYSEAL: waiting reason={} pending={} unsealed={} plain_left={} ::", r, pe, q, pl);
    }
}

/// `tests btkeyseal`: how many bonds Holocron holds sealed, how many plain `bt.linkkey` attributes are
/// left in the store, and the codec leg. PASS = plain_left 0 and Holocron answered; FAIL = a plain key left
/// or the codec broke; SKIP = no Holocron to count with (no ring / locked / not running).
pub fn btkeyseal_selftest() {
    let codec = super::btkeyseal::codec_ok();
    let mut plain_left = 0usize;
    let mut objects = 0usize;
    if let Some(dir) = store_dir() {
        let mt = crate::shell::vfs_mount_table();
        if let Ok(ents) = mt.read_dir(&dir) {
            for ent in ents {
                if parse_addr(&ent.name).is_none() {
                    continue;
                }
                objects += 1;
                let p = format!("{}/{}", dir, ent.name);
                if let Ok(attrs) = mt.list_attrs(&p, crate::fs::vfs::KERNEL_PRINCIPAL) {
                    for (k, mut v) in attrs {
                        if k == "bt.linkkey" {
                            plain_left += 1;
                        }
                        if let crate::fs::vfs::AttrValue::Blob(x) = &mut v {
                            holocron_wipe(x);
                        }
                    }
                }
            }
        }
    }
    let (q, _, pe) = seal_counts();
    let (sealed, holo) = match super::btkeyseal::count() {
        Ok(n) => (alloc::format!("{}", n), "answered"),
        Err(a) => (String::from("n/a"), a.reason()),
    };
    let verdict = if plain_left > 0 || !codec { "FAIL" } else if holo == "answered" { "PASS" } else { "SKIP" };
    serial_println!(":: BTKEYSEAL: sealed={} plain_left={} objects={} pending={} unsealed={} holocron={} codec={} -> {} ::", sealed, plain_left, objects, pe, q, holo, if codec { "ok" } else { "FAIL" }, verdict);
}
