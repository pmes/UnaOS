//! CHARTER: Kernel — driver
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! XHCIISO (rmbp-ledger B534) — isochronous IN on xHCI, the 4 KiB EP0 buffer, and the UVC hand-off.
//!
//! Declared from `xhci/mod.rs` as `#[cfg(feature = "uvc")]` on an existing line (both arches — ARMSONDE, SR101),
//! so a knob-off build never lexes this file; every reach into it from `mod.rs` is a same-line cfg-gated
//! append (the usbnet shape). Nothing here runs unless the `uvc` knob is armed AND a camera enumerates.
//!
//! # THE SHAPE
//!
//! * **Candidates** — the root FSM's "no driver for device class" arm queues a slot whose device class is
//!   0xEF (USB 2.0 IAD ECN: Miscellaneous/IAD — every UVC camera with an IAD); the hub walk queues a slot whose
//!   64-byte configuration window carries a video IAD (`uvc::cfg_has_video_iad`). Queuing costs no traffic.
//! * **`service_uvc`** — rides the FTDI service ladder slot (every pass, all four x86 ladders) and, with no root
//!   port mid-enumeration, takes ONE candidate: the 64-byte configuration window into a 4 KiB EP0 buffer, then
//!   `uvc::probe_iso` with `XhciUvc` as its `IsoHost`.
//! * **`XhciUvc`** — EP0 through `sync_control` (any length up to the buffer: one Data TRB, 4 KiB-aligned so it
//!   cannot cross a 64 KiB boundary), a halted EP0 recovered through `ep0_resync`; the isochronous IN endpoint's
//!   Configure Endpoint (Add / Drop) and its ring.
//! * **`claim`** — called from the transfer-event dispatch beside the usbnet/ftdirx claims, for every event on
//!   the armed (slot, DCI). It retires TDs IN RING ORDER through the TRB the event names, hands each completed
//!   TD's bytes to `uvc::stream_payload`, and re-produces one TD per retired one at the tail, frame-ID-scheduled
//!   against MFINDEX, then rings the doorbell. The ring is therefore kept ahead by the claim path itself, which
//!   every event-draining loop in this driver reaches (the 1 ms `xhci-poll` task, and the sync EP0/command/BOT
//!   pumps) — no main-loop hook and no added line in `poll_events`.
//! * **Isochronous OUT** (UAC1, rmbp-ledger B706) — the same engine, the other direction: an `IsoEp` whose address
//!   has bit 7 clear is configured as EP Type 1 at DCI ep*2 into its own pipe (`OPIPE`), each produced TD is
//!   filled from `drivers::usb::uac::pull` before its TRB is written, and `claim_out` retires and refills it. Audio
//!   candidates (an Audio Control interface in the configuration window, `note_audio_root` / `note_downstream`)
//!   wait in their own queue; `service_uvc` takes a camera first, else one of them, and hands it to `uac::probe`.
//! * **Audio isochronous IN** (UACMIC, rmbp-ledger B723) — a microphone's capture endpoint is the IN direction of the
//!   same engine with ITS OWN pipe and armed word (`APIPE`), so a camera, a speaker and a microphone stream side by
//!   side: an `XhciUvc` built for an audio slot (`audio: true`) configures an IN `IsoEp` as EP Type 5 at DCI ep*2+1
//!   with the OUT pipe's short lead, each completed TD's bytes go to `drivers::usb::uac::push_capture` (the class
//!   driver's 64 KiB capture ring) from `claim_ain`, and the pipe exists only while a capture source is open:
//!   `service_uvc` asks `uac::ep0_due` first on every pass and hands `uac::ep0_service` the slot's host.
//! * **Audio feedback IN** (UACFEEDBACK, rmbp-ledger B789) — an asynchronous sink's feedback (synch) endpoint is the
//!   engine's FOURTH pipe (`FPIPE`, its own armed word), so it never lands in the microphone's: `iso_configure` routes
//!   an audio slot's IN endpoint there when the class driver names it (`ac::feedback_ep`); `claim_ain` (shared with the
//!   capture pipe) hands its completed values to `uac::push_feedback`, and every OUT TD's length is the class
//!   driver's (`ac::out_next_len`: the nominal until a value arrives, then metered to it).
//!
//! # SOURCES (DRIVERS-METHOD §5 — each constant on its line carries what it has)
//!
//! eXtensible Host Controller Interface 1.2: §4.11.2.5 (Isoch TRBs: Frame ID, SIA, the IST/895 ms window),
//! §6.4.1.3 (the Isoch TRB layout — the B534 row cites §6.4.1.2.3, which in this reading of 1.2 is the Status
//! Stage TRB; the seat should re-check the number, the layout is cited by field below), §6.2.3 (Endpoint Context),
//! §6.2.2 (Slot Context), §5.3.4 (HCSPARAMS2.IST), §5.5.1 (MFINDEX), §6.4.5 (completion codes), §4.6.6
//! (Configure Endpoint), §4.10.3 (isochronous error handling: Missed Service, Ring Overrun). USB 2.0 Table 9-13
//! and USB 3.2 §9.6.7 for the endpoint fields. Linux `drivers/usb/host/xhci-ring.c` (xhci_get_burst_count /
//! xhci_get_last_burst_packet_count) is the WORKING-DRIVER source for TBC/TLBPC — recalled, not re-read on this
//! arc, so those two fields are marked [ONE-SOURCE + recalled] and the first completion's code refutes them.
//! No capture exists yet (§2): the first flight's `[uvc-iso]` lines are it.

use super::context::{DeviceContext, InputContext, CTX_WORDS};
use super::dma_coherency;
use super::trb::Trb;
use super::XhciController;
use crate::drivers::uvc::{self, IsoEp, IsoHost};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

// UACMIC (rmbp-ledger B723): THE ONE DOOR from this engine to the USB Audio Class driver. With the `uac` feature it is
// `drivers::usb::uac`; without it (a `uvc` image: `uvc` no longer carries UAC1) every reach is an inert stub, so no
// audio candidate is ever queued, no OUT or audio IN pipe is ever configured, and the class driver is not lexed.
#[cfg(feature = "uac")]
use crate::drivers::usb::uac as ac;
#[cfg(not(feature = "uac"))]
mod ac {
    use crate::drivers::uvc::IsoHost;
    pub fn cfg_has_audio(_: &[u8]) -> bool {
        false
    }
    pub fn cfg_has_hid(_: &[u8]) -> bool {
        false
    }
    pub fn pull(d: &mut [u8]) {
        d.fill(0)
    }
    pub fn push_capture(_: &[u8]) {}
    pub fn push_feedback(_: &[u8]) {}
    pub fn capture_clock(_: usize, _: u32) {}
    pub fn feedback_ep(_: u8) -> bool {
        false
    }
    pub fn out_next_len(n: u32, _: u32, _: u32) -> u32 {
        n
    }
    pub fn disconnect(_: u8) {}
    pub fn out_td_len(_: u32) -> u32 {
        0
    }
    pub fn ep0_due() -> Option<u8> {
        None
    }
    pub fn probe(_: u8, _: u16, _: u16, _: &[u8], _: u8, _: &mut dyn IsoHost) {}
    pub fn capture_lost(_: u8, _: &str) {}
    pub fn ep0_service(_: &mut dyn IsoHost) {}
}

// ── Constants (xHCI 1.2 unless named) ───────────────────────────────────────────────────────────────────

/// TRB Type 5 = Isoch (§6.4.6 TRB type table · §6.4.1.3).
const TRB_ISOCH: u32 = 5;
/// TRB Type 6 = Link (§6.4.4.1), Toggle Cycle = bit 1.
const TRB_LINK: u32 = 6;
/// TRB Type 12 = Configure Endpoint Command (§6.4.3.5 · `configure_hub_interrupt_ep` uses the same).
const TRB_CONFIGURE_EP: u32 = 12;
/// Endpoint Context EP Type 5 = Isoch IN (§6.2.3, Table 6-9 EP Type encoding: 1 Isoch Out … 5 Isoch In).
const EP_TYPE_ISOCH_IN: u32 = 5;
/// Endpoint Context EP Type 1 = Isoch OUT (same table). UAC1 (rmbp-ledger B706).
const EP_TYPE_ISOCH_OUT: u32 = 1;
/// UAC1: the OUT pipe's lead — 32 TDs (32 ms at one TD per full-speed frame). A playback stream wants a short
/// lead (the latency from `audio` write to the wire), not the IN path's 1 MiB budget.
const OUT_TDS: usize = 32;
/// Isoch TRB control bits (§6.4.1.3): ISP bit 2, IOC bit 5, TBC bits 8:7, BEI bit 9, TLBPC bits 19:16,
/// Frame ID bits 30:20, SIA bit 31.
const ISO_ISP: u32 = 1 << 2;
const ISO_IOC: u32 = 1 << 5;
const ISO_BEI: u32 = 1 << 9;
const ISO_SIA: u32 = 1 << 31;
/// MFINDEX (§5.5.1): Runtime Base + 0x00, bits 13:0, 125 µs microframes, wrapping at 2^14.
const MFINDEX_MASK: u32 = 0x3FFF;
/// §4.11.2.5: a Frame ID must name a frame less than 895 ms ahead of MFINDEX. In microframes.
const FRAME_WINDOW_UF: u32 = 895 * 8;
/// Our own margin on top of IST when (re)starting the schedule: 2 ms (16 microframes) [ONE-SOURCE: chosen —
/// §4.11.2.5 requires only > IST; the margin covers the time from the MFINDEX read to the doorbell].
const START_MARGIN_UF: u32 = 16;
/// Ring slack: slots kept free between the producer and the oldest outstanding TD, so a duplicate Success
/// event for a just-retired TD (Panther Point, Linux's XHCI_SPURIOUS_SUCCESS, device 0x1e31 — the reason
/// `mouse_prev_phys` exists in mod.rs) names a FREE slot and is refused, never read as "retire everything".
const RING_SLACK: usize = 8;
/// TD buffer budget per stream: 1 MiB. High-speed 3072-byte ESITs (4 KiB stride) → 247 TDs ≈ 31 ms at one per
/// microframe; a SuperSpeed 48 KiB ESIT (64 KiB stride) → 16 TDs ≈ 2 ms.
const TD_BUDGET: usize = 1024 * 1024;
/// The EP0 buffer `uvc::probe_iso` reads into.
const EP0_CAP: usize = uvc::XHCI_EP0_CAP;
/// First-N completions witnessed by line (cc decoded), then counters only.
const FIRST_WITNESSED: u32 = 4;
/// Frame-ID scheduling yields to SIA once this many TDs retired as missed with not one delivered (§4 decision
/// tree: frameid rung refuted → sia rung, same boot).
const SIA_FALLBACK_MISSED: u32 = 64;

// ── Candidates ──────────────────────────────────────────────────────────────────────────────────────────

static PENDING: crate::sync::Mutex<[u8; 4]> = crate::sync::Mutex::new([0; 4]);

fn push_candidate(slot: u8, path: &str) {
    let mut q = PENDING.lock();
    if q.contains(&slot) {
        return;
    }
    match q.iter_mut().find(|s| **s == 0) {
        Some(s) => {
            *s = slot;
            crate::bootlog_println!("[uvc-iso] candidate slot={} path={} queued", slot, path);
        }
        None => serial_println!("[uvc-iso] candidate slot={} path={} DROPPED reason=queue-full(4)", slot, path),
    }
}

fn pop_candidate() -> u8 {
    let mut q = PENDING.lock();
    for s in q.iter_mut() {
        if *s != 0 {
            return core::mem::replace(s, 0);
        }
    }
    0
}

// UAC1 (rmbp-ledger B706): audio candidates — a second queue, taken by `service_uvc` when no camera waits.
static PENDING_AUDIO: crate::sync::Mutex<[u8; 4]> = crate::sync::Mutex::new([0; 4]);

fn push_audio(slot: u8, path: &str) {
    let mut q = PENDING_AUDIO.lock();
    if q.contains(&slot) {
        return;
    }
    match q.iter_mut().find(|s| **s == 0) {
        Some(s) => {
            *s = slot;
            crate::bootlog_println!("[usb-iso] audio candidate slot={} path={} queued", slot, path);
        }
        None => serial_println!("[usb-iso] audio candidate slot={} path={} DROPPED reason=queue-full(4)", slot, path),
    }
}

fn pop_audio() -> u8 {
    let mut q = PENDING_AUDIO.lock();
    for s in q.iter_mut() {
        if *s != 0 {
            return core::mem::replace(s, 0);
        }
    }
    0
}

/// UAC1: the root config walk (drivers/xhci/mod.rs, the fold after the bulk-pair arms). `buf` is the slot's
/// descriptor buffer holding the configuration descriptor just read. Queues an audio candidate when the first
/// 64 bytes carry an Audio Control interface; returns true when the ROOT PORT must be released here — the
/// window has no HID interface (a HID interface in the same 64 bytes is armed by `record_hid_interfaces`, whose
/// completion advances the port itself).
pub fn note_audio_root(slot: u8, buf: u64) -> bool {
    // SAFETY: `buf` is the slot's 256-byte descriptor buffer and the walk that called us just read it.
    let win = unsafe { core::slice::from_raw_parts(buf as *const u8, 64) };
    if slot == 0 || !ac::cfg_has_audio(win) {
        return false;
    }
    push_audio(slot, "root");
    !ac::cfg_has_hid(win)
}

/// Root FSM, "no driver for device class": a Miscellaneous/IAD device (class 0xEF) is a camera candidate.
pub fn note_root(slot: u8, class: u8) {
    if class == 0xEF && slot != 0 {
        push_candidate(slot, "root");
    }
}

/// Hub walk: `buf` holds the first 64 bytes of the configuration descriptor `enumerate_downstream` just read.
pub fn note_downstream(slot: u8, buf: u64) {
    // SAFETY: `buf` is the slot's 256-byte descriptor buffer and the caller's 64-byte read just completed.
    let win = unsafe { core::slice::from_raw_parts(buf as *const u8, 64) };
    if slot != 0 && uvc::cfg_has_video_iad(win) {
        push_candidate(slot, "hub");
    }
    if slot != 0 && ac::cfg_has_audio(win) {
        push_audio(slot, "hub"); // UAC1 (B706): a hubbed audio device — the hub FSM releases nothing, so no port to free
    }
}

// ── The armed pipe ──────────────────────────────────────────────────────────────────────────────────────

struct Pipe {
    slot: u8,
    dci: u8,
    /// UAC1 (B706): an isochronous OUT pipe — each produced TD is filled from `uac::pull` first.
    out: bool,
    /// UACMIC (B723): an audio isochronous IN pipe — each completed TD goes to `uac::push_capture`.
    ain: bool,
    /// UACFEEDBACK (B789): an audio feedback IN pipe — each completed TD goes to `uac::push_feedback`.
    fb: bool,
    /// Ring: `nslots` TRBs, data slots 0..nslots-1, the Link at `nslots`.
    ring: *mut Trb,
    ring_phys: u64,
    nslots: usize,
    enq: usize,
    deq: usize,
    cycle: bool,
    outstanding: usize,
    target: usize,
    /// One TD buffer per data slot, `stride` apart (a power of two ≥ the TD length, ≤ 64 KiB, and the region is
    /// `stride`-aligned, so no TD buffer crosses a 64 KiB boundary — §6.4.1.1's data-buffer rule).
    bufs: *mut u8,
    stride: usize,
    td_len: u32,
    /// UACFEEDBACK (B789): the largest TD the endpoint context reserves (the ESIT) — a metered OUT TD's bound.
    esit: u32,
    mps: u32,
    ss_burst: u32,
    ss: bool,
    interval_uf: u32,
    ist_uf: u32,
    next_uf: u32,
    sia: bool,
    db_addr: u64,
    mfindex_addr: usize,
    delivered: u32,
    missed_since_start: u32,
}

unsafe impl Send for Pipe {}

static PIPE: crate::sync::Mutex<Option<Pipe>> = crate::sync::Mutex::new(None);
static PIPE_SLOT: AtomicU8 = AtomicU8::new(0);
static PIPE_DCI: AtomicU8 = AtomicU8::new(0);
// UAC1 (B706): the isochronous OUT pipe — its own armed word, so a camera and a speaker stream side by side.
static OPIPE: crate::sync::Mutex<Option<Pipe>> = crate::sync::Mutex::new(None);
static OPIPE_SLOT: AtomicU8 = AtomicU8::new(0);
static OPIPE_DCI: AtomicU8 = AtomicU8::new(0);
static ST_O_TDS: AtomicU32 = AtomicU32::new(0);
static ST_O_MISSED: AtomicU32 = AtomicU32::new(0);
static ST_O_UNDERRUN: AtomicU32 = AtomicU32::new(0);
static ST_O_EVENTS: AtomicU64 = AtomicU64::new(0); static ST_O_DUP: AtomicU32 = AtomicU32::new(0);

// UACMIC (B723): the audio isochronous IN pipe — its own armed word beside the camera's and the speaker's.
static APIPE: crate::sync::Mutex<Option<Pipe>> = crate::sync::Mutex::new(None);
static APIPE_SLOT: AtomicU8 = AtomicU8::new(0);
static APIPE_DCI: AtomicU8 = AtomicU8::new(0);
// UACFEEDBACK (B789): the audio feedback IN pipe — its own armed word beside the capture pipe's.
static FPIPE: crate::sync::Mutex<Option<Pipe>> = crate::sync::Mutex::new(None);
static FPIPE_SLOT: AtomicU8 = AtomicU8::new(0);
static FPIPE_DCI: AtomicU8 = AtomicU8::new(0);
static ST_A_TDS: AtomicU32 = AtomicU32::new(0);
static ST_A_MISSED: AtomicU32 = AtomicU32::new(0);
static ST_A_OVERRUN: AtomicU32 = AtomicU32::new(0);
static ST_A_EVENTS: AtomicU64 = AtomicU64::new(0);
/// UACFEEDBACK (B789): events on the feedback pipe (its first few are witnessed by line, as every pipe's).
static ST_F_EVENTS: AtomicU64 = AtomicU64::new(0);

/// UACMIC: the audio IN pipe's counters (TDs retired, retired-not-received, controller ring overruns, events).
pub fn ain_stats() -> (u32, u32, u32, u64) {
    (ST_A_TDS.load(Ordering::Relaxed), ST_A_MISSED.load(Ordering::Relaxed), ST_A_OVERRUN.load(Ordering::Relaxed), ST_A_EVENTS.load(Ordering::Relaxed))
}

/// UAC1: the OUT pipe's counters (TDs retired, retired-not-sent, controller ring underruns, events).
pub fn out_stats() -> (u32, u32, u32, u64) {
    (ST_O_TDS.load(Ordering::Relaxed), ST_O_MISSED.load(Ordering::Relaxed), ST_O_UNDERRUN.load(Ordering::Relaxed), ST_O_EVENTS.load(Ordering::Relaxed))
}

static ST_TDS: AtomicU32 = AtomicU32::new(0);
static ST_MISSED: AtomicU32 = AtomicU32::new(0);
static ST_OVERRUN: AtomicU32 = AtomicU32::new(0);
static ST_XACT: AtomicU32 = AtomicU32::new(0);
static ST_DUP: AtomicU32 = AtomicU32::new(0);
static ST_RESYNC: AtomicU32 = AtomicU32::new(0);
static ST_EVENTS: AtomicU64 = AtomicU64::new(0);
static ST_SIA: AtomicBool = AtomicBool::new(false);

/// The counters `tests uvc` prints beside the frame count.
#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub tds: u32,
    pub missed: u32,
    pub overrun: u32,
    pub xact: u32,
    pub dup: u32,
    pub resync: u32,
    pub events: u64,
    pub sia: bool,
}

pub fn stats() -> Stats {
    Stats {
        tds: ST_TDS.load(Ordering::Relaxed),
        missed: ST_MISSED.load(Ordering::Relaxed),
        overrun: ST_OVERRUN.load(Ordering::Relaxed),
        xact: ST_XACT.load(Ordering::Relaxed),
        dup: ST_DUP.load(Ordering::Relaxed),
        resync: ST_RESYNC.load(Ordering::Relaxed),
        events: ST_EVENTS.load(Ordering::Relaxed),
        sia: ST_SIA.load(Ordering::Relaxed),
    }
}

/// §6.4.5 completion codes this path meets, by name.
fn cc_name(cc: u8) -> &'static str {
    match cc {
        1 => "Success",
        3 => "Babble",
        4 => "USB-Transaction-Error",
        5 => "TRB-Error",
        6 => "Stall",
        7 => "Resource-Error",
        8 => "Bandwidth-Error",
        11 => "Slot-Not-Enabled",
        12 => "Endpoint-Not-Enabled",
        13 => "Short-Packet",
        14 => "Ring-Underrun",
        15 => "Ring-Overrun",
        17 => "Parameter-Error",
        18 => "Bandwidth-Overrun",
        19 => "Context-State-Error",
        23 => "Missed-Service",
        26 => "Stopped",
        27 => "Stopped-Length-Invalid",
        _ => "other",
    }
}

fn ep_state_name(s: u8) -> &'static str {
    match s {
        0 => "Disabled",
        1 => "Running",
        2 => "Halted",
        3 => "Stopped",
        4 => "Error",
        _ => "?",
    }
}

impl Pipe {
    fn mfindex(&self) -> u32 {
        // SAFETY: `mfindex_addr` is this controller's Runtime Base + 0 (§5.5.1), mapped for the boot.
        unsafe { core::ptr::read_volatile(self.mfindex_addr as *const u32) & MFINDEX_MASK }
    }

    fn index_of(&self, phys: u64) -> Option<usize> {
        if phys < self.ring_phys || phys >= self.ring_phys + (self.nslots as u64) * 16 || (phys - self.ring_phys) % 16 != 0 {
            return None;
        }
        Some(((phys - self.ring_phys) / 16) as usize)
    }

    /// The Frame ID for the next TD, or SIA. Re-anchored on MFINDEX whenever the schedule has fallen behind the
    /// controller (or drifted out of the 895 ms window): §4.11.2.5 — a Frame ID must lie after MFINDEX + IST.
    fn schedule(&mut self) -> u32 {
        if self.sia {
            return ISO_SIA;
        }
        let now = self.mfindex();
        let ahead = self.next_uf.wrapping_sub(now) & MFINDEX_MASK;
        if ahead <= self.ist_uf || ahead >= FRAME_WINDOW_UF {
            let iv = self.interval_uf.max(1);
            let start = now + self.ist_uf + START_MARGIN_UF;
            self.next_uf = ((start + iv - 1) / iv * iv) & MFINDEX_MASK;
            ST_RESYNC.fetch_add(1, Ordering::Relaxed);
        }
        let fid = (self.next_uf >> 3) & 0x7FF;
        self.next_uf = (self.next_uf + self.interval_uf.max(1)) & MFINDEX_MASK;
        fid << 20
    }

    /// TBC / TLBPC for one TD of `td_len` bytes (§6.4.1.3 · Linux xhci_get_burst_count /
    /// xhci_get_last_burst_packet_count [recalled]): high-speed TBC 0, TLBPC = packets - 1; SuperSpeed
    /// TBC = ceil(packets / (bMaxBurst+1)) - 1, TLBPC = the last burst's packets - 1.
    fn burst_fields(&self, len: u32) -> u32 {
        let packets = ((len + self.mps - 1) / self.mps.max(1)).max(1);
        let (tbc, tlbpc) = if self.ss {
            let b = self.ss_burst + 1;
            let r = packets % b;
            ((packets + b - 1) / b - 1, if r == 0 { b - 1 } else { r - 1 })
        } else {
            (0, packets - 1)
        };
        ((tbc & 0x3) << 7) | ((tlbpc & 0xF) << 16)
    }

    fn write_trb(&mut self, i: usize, t: Trb) {
        // SAFETY: `i` < nslots + 1 (the ring's allocation); the control dword (cycle) is written LAST.
        unsafe {
            let p = self.ring.add(i) as *mut u32;
            core::ptr::write_volatile(p, t.parameter as u32);
            core::ptr::write_volatile(p.add(1), (t.parameter >> 32) as u32);
            core::ptr::write_volatile(p.add(2), t.status);
            core::sync::atomic::fence(Ordering::SeqCst);
            core::ptr::write_volatile(p.add(3), t.control);
            core::sync::atomic::fence(Ordering::SeqCst);
            dma_coherency::clean(p as usize, 16);
        }
    }

    /// Produce one TD at the enqueue slot (§4.11.2.5 Isoch TD: one TRB, its own buffer, IOC so every TD posts
    /// the event whose residual is the payload's length; BEI on 7 of 8 so the events do not each interrupt).
    fn produce(&mut self) {
        let i = self.enq;
        let buf = self.bufs as u64 + (i * self.stride) as u64;
        // UACFEEDBACK (B789): an OUT TD's length is the class driver's (metered to an asynchronous sink's feedback,
        // else the nominal `td_len`); an IN TD is `td_len` as before.
        let len = if self.out { ac::out_next_len(self.td_len, self.interval_uf, self.esit) } else { self.td_len };
        if self.out {
            // UAC1 (B706): the TD's payload, from the class driver's ring (silence where it is short).
            // SAFETY: TD buffer `i` is `stride` >= esit >= len bytes inside the pipe's allocation, and slot `i` is free
            // (the controller retired its previous TD before `outstanding` dropped below the target).
            ac::pull(unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) });
        }
        dma_coherency::clean(buf as usize, len as usize);
        let bei = if i % 8 != 7 { ISO_BEI } else { 0 };
        let isp = if self.out { 0 } else { ISO_ISP }; // §6.4.1.3: Interrupt-on-Short-Packet is an IN notion
        let ctl = (TRB_ISOCH << 10) | isp | ISO_IOC | bei | self.burst_fields(len) | self.schedule() | (self.cycle as u32);
        self.write_trb(i, Trb { parameter: buf, status: len & 0x1FFFF, control: ctl });
        self.enq += 1;
        if self.enq == self.nslots {
            // Arm the Link for the lap now ending (its cycle = this lap's), THEN adopt the new colour (§4.9.2.2).
            let link = Trb { parameter: self.ring_phys, status: 0, control: (TRB_LINK << 10) | (1 << 1) | (self.cycle as u32) };
            self.write_trb(self.nslots, link);
            self.cycle = !self.cycle;
            self.enq = 0;
        }
        self.outstanding += 1;
    }

    fn top_up(&mut self) -> usize {
        let mut n = 0;
        while self.outstanding < self.target {
            self.produce();
            n += 1;
        }
        n
    }

    fn doorbell(&self) {
        // SAFETY: `db_addr` is DB[slot] of this controller (Cap Base + DBOFF + 4*slot); target = the DCI.
        unsafe { super::ring_doorbell_asm(self.db_addr, self.dci as u32) };
    }
}

/// THE TRANSFER-EVENT CLAIM — every event on the armed (slot, DCI), whatever its code. `true` = consumed.
pub fn claim(slot: u8, dci: u8, param: u64, cc: u8, residual: u32) -> bool {
    if slot != 0 && OPIPE_SLOT.load(Ordering::Acquire) == slot && OPIPE_DCI.load(Ordering::Relaxed) == dci {
        return claim_out(slot, dci, param, cc, residual); // UAC1 (B706): the isochronous OUT pipe
    }
    if slot != 0 && FPIPE_SLOT.load(Ordering::Acquire) == slot && FPIPE_DCI.load(Ordering::Relaxed) == dci {
        return claim_ain(&FPIPE, slot, dci, param, cc, residual); // UACFEEDBACK (B789): the audio feedback IN pipe
    }
    if slot != 0 && APIPE_SLOT.load(Ordering::Acquire) == slot && APIPE_DCI.load(Ordering::Relaxed) == dci {
        return claim_ain(&APIPE, slot, dci, param, cc, residual); // UACMIC (B723): the audio isochronous IN pipe
    }
    if slot == 0 || PIPE_SLOT.load(Ordering::Acquire) != slot || PIPE_DCI.load(Ordering::Relaxed) != dci {
        return false;
    }
    let n_ev = ST_EVENTS.fetch_add(1, Ordering::Relaxed);
    let mut g = PIPE.lock();
    let Some(p) = g.as_mut() else { return true };
    match cc {
        4 => { ST_XACT.fetch_add(1, Ordering::Relaxed); }
        14 | 15 => { ST_OVERRUN.fetch_add(1, Ordering::Relaxed); }
        _ => {}
    }
    if (n_ev as u32) < FIRST_WITNESSED {
        serial_println!(
            "[uvc-iso] event n={} slot={} dci={} cc={} ({}) trb={:#x} residual={} mfindex={} outstanding={}",
            n_ev, slot, dci, cc, cc_name(cc), param, residual, p.mfindex(), p.outstanding
        );
    }
    match p.index_of(param) {
        Some(i) if i < p.nslots => {
            let dist = (i + p.nslots - p.deq) % p.nslots;
            if dist >= p.outstanding {
                ST_DUP.fetch_add(1, Ordering::Relaxed); // names a free slot: a duplicate or a stale event
                return true;
            }
            for k in 0..=dist {
                let j = (p.deq + k) % p.nslots;
                ST_TDS.fetch_add(1, Ordering::Relaxed);
                if k == dist && (cc == 1 || cc == 13) {
                    let len = p.td_len.saturating_sub(residual) as usize;
                    let b = p.bufs as usize + j * p.stride;
                    dma_coherency::inval(b, len);
                    // SAFETY: TD buffer `j` is `stride` >= td_len >= len bytes inside the pipe's allocation, and
                    // the controller has retired the TD that wrote it.
                    uvc::stream_payload(unsafe { core::slice::from_raw_parts(b as *const u8, len) });
                    p.delivered = p.delivered.wrapping_add(1);
                } else {
                    ST_MISSED.fetch_add(1, Ordering::Relaxed);
                    p.missed_since_start = p.missed_since_start.wrapping_add(1);
                }
            }
            p.deq = (i + 1) % p.nslots;
            p.outstanding -= dist + 1;
        }
        _ => {} // Ring Overrun/Underrun carry no TRB pointer (§4.10.3.1): nothing to retire; refill below
    }
    if !p.sia && p.delivered == 0 && p.missed_since_start >= SIA_FALLBACK_MISSED {
        p.sia = true;
        ST_SIA.store(true, Ordering::Relaxed);
        serial_println!("[uvc-iso] sched frameid->sia reason=missed={} delivered=0 (the frame-ID rung is refuted on this controller; SIA from the next TD)", p.missed_since_start);
    }
    if p.top_up() != 0 || cc == 14 || cc == 15 {
        p.doorbell();
    }
    true
}

/// UAC1 (B706) — THE OUT CLAIM: every event on the armed OUT (slot, DCI). Retires TDs in ring order through the
/// TRB the event names (the duplicate/stale guard of `claim`), counts the ones not sent, and tops the ring up —
/// each new TD pulls the next payload from the class driver's ring. No lock but the pipe's own is taken.
fn claim_out(slot: u8, dci: u8, param: u64, cc: u8, residual: u32) -> bool {
    let n_ev = ST_O_EVENTS.fetch_add(1, Ordering::Relaxed);
    let mut g = OPIPE.lock();
    let Some(p) = g.as_mut() else { return true };
    if cc == 14 {
        ST_O_UNDERRUN.fetch_add(1, Ordering::Relaxed); // §4.10.3.1: the controller found no TD in time (CRAWLC: cc 14 Ring Underrun only — 15, Ring Overrun, is an IN pipe's)
    }
    if (n_ev as u32) < FIRST_WITNESSED {
        serial_println!(
            "[usb-iso] event n={} slot={} dci={} dir=out cc={} ({}) trb={:#x} residual={} mfindex={} outstanding={}",
            n_ev, slot, dci, cc, cc_name(cc), param, residual, p.mfindex(), p.outstanding
        );
    }
    if let Some(i) = p.index_of(param) {
        if i < p.nslots {
            let dist = (i + p.nslots - p.deq) % p.nslots;
            if dist >= p.outstanding {
                ST_O_DUP.fetch_add(1, Ordering::Relaxed); // CRAWLC: the OUT pipe's own count, not the IN pipe's `dup=`
                return true;
            }
            for k in 0..=dist {
                ST_O_TDS.fetch_add(1, Ordering::Relaxed);
                if k == dist && cc == 1 {
                    p.delivered = p.delivered.wrapping_add(1);
                } else {
                    ST_O_MISSED.fetch_add(1, Ordering::Relaxed);
                    p.missed_since_start = p.missed_since_start.wrapping_add(1);
                }
            }
            p.deq = (i + 1) % p.nslots;
            p.outstanding -= dist + 1;
        }
    }
    if !p.sia && p.delivered == 0 && p.missed_since_start >= SIA_FALLBACK_MISSED {
        p.sia = true;
        serial_println!("[usb-iso] sched frameid->sia dir=out reason=missed={} delivered=0 (SIA from the next TD)", p.missed_since_start);
    }
    if p.top_up() != 0 || cc == 14 || cc == 15 {
        p.doorbell();
    }
    true
}

/// UACMIC (B723) — THE AUDIO IN CLAIM: every event on the armed audio IN (slot, DCI). Retires TDs in ring order
/// through the TRB the event names (the same duplicate/stale guard), hands the completed TD's bytes (`td_len -
/// residual`, a short packet is the normal case: 44.1 kHz alternates 44 and 45 frames) to the class driver's capture
/// ring, and tops the ring up. No lock but the pipe's own is taken; `push_capture` is lock-free.
/// UACFEEDBACK (B789): the same claim serves the feedback pipe (`pipe` = `&FPIPE`, `p.fb`): its values go to
/// `uac::push_feedback` (lock-free) and the capture counters (`ain_stats`, the microphone's) are not touched.
fn claim_ain(pipe: &crate::sync::Mutex<Option<Pipe>>, slot: u8, dci: u8, param: u64, cc: u8, residual: u32) -> bool {
    let mut g = pipe.lock();
    let Some(p) = g.as_mut() else { return true };
    let cap = !p.fb;
    let n_ev = if cap { ST_A_EVENTS.fetch_add(1, Ordering::Relaxed) } else { ST_F_EVENTS.fetch_add(1, Ordering::Relaxed) };
    if cap && (cc == 14 || cc == 15) {
        ST_A_OVERRUN.fetch_add(1, Ordering::Relaxed);
    }
    if (n_ev as u32) < FIRST_WITNESSED {
        serial_println!(
            "[usb-iso] event n={} slot={} dci={} dir={} cc={} ({}) trb={:#x} residual={} mfindex={} outstanding={}",
            n_ev, slot, dci, if cap { "in-audio" } else { "in-feedback" }, cc, cc_name(cc), param, residual, p.mfindex(), p.outstanding
        );
    }
    if let Some(i) = p.index_of(param) {
        if i < p.nslots {
            let dist = (i + p.nslots - p.deq) % p.nslots;
            if dist >= p.outstanding {
                ST_DUP.fetch_add(1, Ordering::Relaxed);
                return true;
            }
            for k in 0..=dist {
                let j = (p.deq + k) % p.nslots;
                if cap {
                    ST_A_TDS.fetch_add(1, Ordering::Relaxed);
                }
                if k == dist && (cc == 1 || cc == 13) {
                    let len = p.td_len.saturating_sub(residual) as usize;
                    let b = p.bufs as usize + j * p.stride;
                    dma_coherency::inval(b, len);
                    // SAFETY: TD buffer `j` is `stride` >= td_len >= len bytes inside the pipe's allocation, and the
                    // controller has retired the TD that wrote it.
                    let bytes = unsafe { core::slice::from_raw_parts(b as *const u8, len) };
                    if cap { ac::push_capture(bytes); ac::capture_clock(len, p.interval_uf) } else { ac::push_feedback(bytes) } // UACIMPLICIT (B809): the delivered length is an implicit-feedback sink's clock
                    p.delivered = p.delivered.wrapping_add(1);
                } else {
                    if cap {
                        ST_A_MISSED.fetch_add(1, Ordering::Relaxed);
                    }
                    p.missed_since_start = p.missed_since_start.wrapping_add(1);
                }
            }
            p.deq = (i + 1) % p.nslots;
            p.outstanding -= dist + 1;
        }
    }
    if !p.sia && p.delivered == 0 && p.missed_since_start >= SIA_FALLBACK_MISSED {
        p.sia = true;
        serial_println!("[usb-iso] sched frameid->sia dir={} reason=missed={} delivered=0 (SIA from the next TD)", if cap { "in-audio" } else { "in-feedback" }, p.missed_since_start);
    }
    if p.top_up() != 0 || cc == 14 || cc == 15 {
        p.doorbell();
    }
    true
}

/// The device left: forget the candidate and the pipe. The ring and TD buffers are deliberately NOT freed —
/// the controller may still hold the TR Dequeue Pointer until the slot is disabled, so they are leaked, one
/// stream's worth (≤ 1 MiB + 4 KiB) per camera detach. The assembler is dropped (it is ours alone).
pub fn disconnect(slot: u8) {
    if slot == 0 {
        return;
    }
    {
        let mut q = PENDING.lock();
        for s in q.iter_mut() {
            if *s == slot {
                *s = 0;
            }
        }
    }
    if PIPE_SLOT.load(Ordering::Acquire) == slot {
        PIPE_SLOT.store(0, Ordering::Release);
        if let Some(p) = PIPE.lock().take() {
            core::mem::forget(p);
        }
        uvc::stream_disarm();
        serial_println!("[uvc-iso] disconnect slot={} — stream disarmed", slot);
    }
    {
        let mut q = PENDING_AUDIO.lock(); // UAC1 (B706): the audio queue and the OUT pipe forget the slot too
        for s in q.iter_mut() {
            if *s == slot {
                *s = 0;
            }
        }
    }
    if OPIPE_SLOT.load(Ordering::Acquire) == slot {
        OPIPE_SLOT.store(0, Ordering::Release);
        if let Some(p) = OPIPE.lock().take() {
            core::mem::forget(p); // as above: the controller may still hold the dequeue pointer
        }
        serial_println!("[usb-iso] disconnect slot={} dir=out — stream disarmed", slot);
    }
    if APIPE_SLOT.load(Ordering::Acquire) == slot {
        APIPE_SLOT.store(0, Ordering::Release); // UACMIC (B723): the audio IN pipe forgets the slot too
        if let Some(p) = APIPE.lock().take() {
            core::mem::forget(p); // as above
        }
        serial_println!("[usb-iso] disconnect slot={} dir=in-audio — stream disarmed", slot);
    }
    if FPIPE_SLOT.load(Ordering::Acquire) == slot {
        FPIPE_SLOT.store(0, Ordering::Release); // UACFEEDBACK (B789): the feedback pipe forgets the slot too
        if let Some(p) = FPIPE.lock().take() {
            core::mem::forget(p); // as above
        }
        serial_println!("[usb-iso] disconnect slot={} dir=in-feedback — stream disarmed", slot);
    }
    ac::disconnect(slot);
}

/// The 4 KiB EP0 buffer, allocated once per boot (4 KiB-aligned, never freed).
fn ep0_buf() -> Option<u64> {
    static BUF: AtomicU64 = AtomicU64::new(0);
    let b = BUF.load(Ordering::Acquire);
    if b != 0 {
        return Some(b);
    }
    let l = core::alloc::Layout::from_size_align(EP0_CAP, 4096).ok()?;
    // SAFETY: a non-zero-size layout.
    let p = unsafe { alloc::alloc::alloc_zeroed(l) } as u64;
    if p == 0 {
        return None;
    }
    BUF.store(p, Ordering::Release);
    Some(p)
}

/// The ring and TD buffers built by `iso_configure` (the TR Dequeue Pointer goes into the endpoint context),
/// published by `iso_start`.
struct Built {
    pipe: Pipe,
}

/// One slot's `IsoHost`.
pub struct XhciUvc<'a> {
    c: &'a mut XhciController,
    slot: u8,
    buf: u64,
    built: Option<Built>,
    /// UACMIC (B723): the slot is an audio device — an IN endpoint is a capture pipe (`APIPE`), not the camera's.
    audio: bool,
}

impl XhciUvc<'_> {
    fn speed(&self) -> u32 {
        let oc = self.c.slots[self.slot as usize].output_context;
        dma_coherency::inval(oc as usize, core::mem::size_of::<DeviceContext>());
        // SAFETY: the slot's output device context (Slot Context DW0, §6.2.2: Speed bits 23:20).
        (unsafe { core::ptr::read_volatile(oc as *const u32) } >> 20) & 0xF
    }

    fn out_ep(&self, dci: u32) -> (u32, u32, u32) {
        let oc = self.c.slots[self.slot as usize].output_context as *const u32;
        dma_coherency::inval(oc as usize, core::mem::size_of::<DeviceContext>());
        // SAFETY: Endpoint Context `dci` of the output device context (§6.2.1: context `dci` at dci*CTX_WORDS).
        unsafe {
            let e = oc.add(dci as usize * CTX_WORDS);
            (core::ptr::read_volatile(e), core::ptr::read_volatile(e.add(1)), core::ptr::read_volatile(e.add(4)))
        }
    }

    /// Configure Endpoint with Add (`add`) or Drop flag for `dci` (§4.6.6; Input Control Context DW0 = Drop
    /// flags, DW1 = Add flags, A0 = the Slot Context). The Slot Context is copied from the output context with
    /// Context Entries raised to `dci` when adding (never lowered — a HID button endpoint above it stays).
    fn configure(&mut self, dci: u32, add: bool, ep: [u32; 5]) -> Result<(), (u8, &'static str)> {
        let s = self.slot as usize;
        let input = self.c.slots[s].input_context;
        let output = self.c.slots[s].output_context;
        if input.is_null() || output.is_null() {
            return Err((0, "no-context"));
        }
        // SAFETY: the slot's own input/output contexts (heap, identity-mapped, 64-byte aligned); no command is in
        // flight on this slot (we are its only user in the service context).
        unsafe {
            let base = input as *mut u32;
            core::ptr::write_bytes(base as *mut u8, 0, core::mem::size_of::<InputContext>());
            dma_coherency::inval(output as usize, core::mem::size_of::<DeviceContext>());
            if add {
                base.add(1).write_volatile(1 | (1 << dci));
            } else {
                base.add(0).write_volatile(1 << dci);
                base.add(1).write_volatile(1);
            }
            let slot_ctx = base.add(CTX_WORDS);
            for i in 0..8 {
                slot_ctx.add(i).write_volatile(core::ptr::read_volatile((output as *const u32).add(i)));
            }
            let dw0 = slot_ctx.read_volatile();
            let entries = (dw0 >> 27) & 0x1F;
            if add && dci > entries {
                slot_ctx.write_volatile((dw0 & !(0x1F << 27)) | (dci << 27));
            }
            if add {
                let e = base.add((1 + dci as usize) * CTX_WORDS);
                for (k, v) in ep.iter().enumerate() {
                    e.add(k).write_volatile(*v);
                }
            }
            dma_coherency::clean(base as usize, core::mem::size_of::<InputContext>());
        }
        let trb = Trb { parameter: input as u64, status: 0, control: (TRB_CONFIGURE_EP << 10) | ((self.slot as u32) << 24) };
        match self.c.run_command_sync(trb) {
            Ok((1, _)) => Ok(()),
            Ok((cc, _)) => Err((cc, cc_name(cc))),
            Err(()) => Err((0, "timeout")),
        }
    }
}

impl IsoHost for XhciUvc<'_> {
    fn control(&mut self, bm: u8, br: u8, wv: u16, wi: u16, wl: u16, dir_in: bool) -> Result<u32, &'static str> {
        let wl = wl.min(EP0_CAP as u16);
        match self.c.ep0_control(self.slot, bm, br, wv, wi, wl, self.buf, dir_in) {
            Ok(1) => Ok(if wl == 0 { 0 } else { self.c.last_control_len }), #[cfg(all(target_arch = "x86_64", feature = "lidsleep", feature = "wc"))] Err(super::Ep0Err::Held) => { super::suspend::held_deferred(self.slot, super::suspend::DEFER_UVC); Err("held") } // HELDMAP (B838): refused on a held slot (EP0 Stopped, not Halted) — no resync; the refusal goes back to the class driver
            other => {
                // A STALL halts EP0 (§4.10.2.1) and a timeout leaves a TD on it: either way the next request
                // needs the ring resynchronised, exactly the USBLUN recovery.
                #[cfg(not(feature = "nousblun"))]
                let _ = self.c.ep0_resync(self.slot);
                Err(match other {
                    Ok(6) => "stall",
                    Ok(4) => "usb-transaction-error",
                    Ok(3) => "babble",
                    Ok(_) => "cc-error",
                    Err(_) => "timeout",
                })
            }
        }
    }

    fn buf(&mut self) -> *mut u8 {
        self.buf as *mut u8
    }

    fn cap(&self) -> usize {
        EP0_CAP
    }

    fn name(&self) -> &'static str {
        "xhci"
    }

    fn iso_configure(&mut self, ep: &IsoEp) -> Result<(), (u8, &'static str)> {
        let out = ep.addr & 0x80 == 0; // UAC1 (B706): an OUT endpoint is a playback pipe (DCI ep*2, EP Type 1)
        let fb = !out && self.audio && ac::feedback_ep(ep.addr); // UACFEEDBACK (B789): the async sink's feedback IN is its own pipe
        let ain = !out && self.audio && !fb; // UACMIC (B723): an audio slot's IN endpoint is a capture pipe (its own word)
        let tag = if out || ain || fb { "[usb-iso]" } else { "[uvc-iso]" };
        if (if out { OPIPE_SLOT.load(Ordering::Acquire) } else if fb { FPIPE_SLOT.load(Ordering::Acquire) } else if ain { APIPE_SLOT.load(Ordering::Acquire) } else { PIPE_SLOT.load(Ordering::Acquire) }) != 0 {
            return Err((0, "one-stream-already-armed"));
        }
        let speed = self.speed();
        let dci = (ep.addr & 0x0F) as u32 * 2 + if out { 0 } else { 1 };
        let ss = speed >= 4;
        if speed == 2 {
            return Err((0, "low-speed-has-no-isochronous"));
        }
        let mps = ep.mps as u32;
        // §6.2.3.4 Max Burst Size: SuperSpeed = bMaxBurst; high-speed = wMaxPacketSize bits 12:11; FS = 0.
        // §6.2.3 Mult (DW0 9:8): SuperSpeed isochronous only; 0 otherwise.
        let (burst, mult) = if ss { (ep.ss_burst as u32, ep.ss_mult as u32) } else if speed == 3 { (ep.hs_extra as u32, 0) } else { (0, 0) };
        // §6.2.3.8 Max ESIT Payload = the bytes one interval moves (the companion's wBytesPerInterval at SS).
        let esit = if ss { ep.esit_bytes() } else { mps * (burst + 1) };
        if esit == 0 || mps == 0 {
            return Err((0, "zero-packet-size"));
        }
        if esit > 65536 {
            return Err((0, "esit-over-64KiB"));
        }
        // §6.2.3.6 Interval: 2^Interval x 125 µs. HS/SS isochronous bInterval is already that exponent + 1
        // (USB 2.0 Table 9-13); FS isochronous counts frames, so +3 (the same encoding the hub path uses).
        let biv = ep.interval.clamp(1, 16) as u32;
        let ivl = if speed >= 3 { biv - 1 } else { biv - 1 + 3 };
        let interval_uf = 1u32 << ivl;
        // UAC1 (B706): an OUT TD carries the bytes one interval plays (the class driver's figure), never more
        // than the ESIT the endpoint context reserves; an IN TD is the whole ESIT, as before.
        let td_len = if out { ac::out_td_len(interval_uf) } else { esit };
        if td_len == 0 || td_len > esit {
            return Err((0, "payload-over-esit"));
        }
        // The ring and TD buffers, before the command: the TR Dequeue Pointer is part of the context.
        let stride = (esit as usize).next_power_of_two().max(64);
        let target = (TD_BUDGET / stride).clamp(8, 255 - RING_SLACK);
        let target = if out || ain || fb { target.min(OUT_TDS) } else { target }; // UACMIC: audio IN takes the OUT pipe's short lead (UACFEEDBACK: and feedback)
        let nslots = target + RING_SLACK;
        let rl = core::alloc::Layout::from_size_align((nslots + 1) * 16, 4096).map_err(|_| (0u8, "layout"))?;
        let bl = core::alloc::Layout::from_size_align(nslots * stride, stride).map_err(|_| (0u8, "layout"))?;
        // SAFETY: non-zero layouts; zeroed so no stale cycle bit is ever a produced TRB (§4.9.2).
        let ring = unsafe { alloc::alloc::alloc_zeroed(rl) } as *mut Trb;
        let bufs = unsafe { alloc::alloc::alloc_zeroed(bl) };
        if ring.is_null() || bufs.is_null() {
            return Err((0, "no-memory"));
        }
        dma_coherency::clean(ring as usize, (nslots + 1) * 16);
        let ring_phys = ring as u64;
        // The Link TRB at the end, NOT yet produced (cycle 0 against the ring's initial producer cycle 1).
        // SAFETY: slot `nslots` is inside the ring allocation.
        unsafe {
            let p = ring.add(nslots) as *mut u32;
            core::ptr::write_volatile(p, ring_phys as u32);
            core::ptr::write_volatile(p.add(1), (ring_phys >> 32) as u32);
            core::ptr::write_volatile(p.add(3), (TRB_LINK << 10) | (1 << 1));
        }
        // The Endpoint Context (§6.2.3, Table 6-9):
        //   DW0 Mult[9:8] Interval[23:16] MaxESITPayloadHi[31:24];  DW1 CErr[2:1]=0 (isochronous — §4.10.2.? the
        //   error count does not apply) EPType[5:3]=5 MaxBurst[15:8] MPS[31:16];  DW2/3 TR Dequeue | DCS=1;
        //   DW4 AverageTRBLength[15:0] (one TRB per TD → the ESIT) MaxESITPayloadLo[31:16].
        let ctx = [
            (mult << 8) | (ivl << 16) | ((esit >> 16) << 24),
            ((if out { EP_TYPE_ISOCH_OUT } else { EP_TYPE_ISOCH_IN }) << 3) | (burst << 8) | (mps << 16),
            (ring_phys as u32) | 1,
            (ring_phys >> 32) as u32,
            (td_len & 0xFFFF) | ((esit & 0xFFFF) << 16), // Average TRB Length = the TD (== the ESIT on IN)
        ];
        let before = self.c.ep_state_of(self.slot, dci as u8);
        let r = self.configure(dci, true, ctx);
        let after = self.c.ep_state_of(self.slot, dci as u8);
        let (d0, d1, d4) = self.out_ep(dci);
        serial_println!(
            "{} cfg-ep slot={} dci={} speed={} type={} mps={} burst={} mult={} esit={} interval_enc={} ({} uframes) ring={:#x} slots={} epstate={}({})->{}({}) result={}",
            tag, self.slot, dci, speed, if out { "isoch-out" } else if fb { "isoch-feedback" } else { "isoch-in" }, mps, burst + 1, mult + 1, esit, ivl, interval_uf, ring_phys, nslots,
            before, ep_state_name(before), after, ep_state_name(after),
            match r { Ok(()) => "cc=1 (Success)", Err((_, n)) => n }
        );
        serial_println!(
            "{} cfg-ep out dw0={:#010x}[state={} mult={} interval={} esit_hi={}] dw1={:#010x}[type={} burst={} mps={}] dw4={:#010x}[avg={} esit_lo={}]",
            tag, d0, d0 & 7, (d0 >> 8) & 3, (d0 >> 16) & 0xFF, d0 >> 24, d1, (d1 >> 3) & 7, (d1 >> 8) & 0xFF, d1 >> 16, d4, d4 & 0xFFFF, d4 >> 16
        );
        if let Err(e) = r {
            // The ring was never handed to a running endpoint: it is ours to drop.
            // SAFETY: the allocations above, with their own layouts, referenced by no context that took effect.
            unsafe {
                alloc::alloc::dealloc(ring as *mut u8, rl);
                alloc::alloc::dealloc(bufs, bl);
            }
            return Err(e);
        }
        // HCSPARAMS2.IST (§5.3.4): bits 3:0; bit 3 set = the count is in frames, else microframes.
        // SAFETY: capability registers of this controller.
        let hcs2 = unsafe { core::ptr::read_volatile((self.c.base_addr + 0x08) as *const u32) };
        let ist = hcs2 & 0xF;
        let ist_uf = if ist & 0x8 != 0 { (ist & 0x7) * 8 } else { ist & 0x7 };
        // SAFETY: as above — RTSOFF (Cap + 0x18, bits 31:5) and DBOFF (Cap + 0x14, bits 31:2).
        let (rts, dbo) = unsafe {
            (
                core::ptr::read_volatile((self.c.base_addr + 0x18) as *const u32) & !0x1F,
                core::ptr::read_volatile((self.c.base_addr + 0x14) as *const u32) & !0x3,
            )
        };
        self.built = Some(Built {
            pipe: Pipe {
                slot: self.slot,
                dci: dci as u8,
                out,
                ain,
                fb,
                ring,
                ring_phys,
                nslots,
                enq: 0,
                deq: 0,
                cycle: true,
                outstanding: 0,
                target,
                bufs,
                stride,
                td_len,
                esit,
                mps,
                ss_burst: burst,
                ss,
                interval_uf,
                ist_uf,
                next_uf: 0,
                sia: false,
                db_addr: (self.c.base_addr + dbo as usize + self.slot as usize * 4) as u64,
                mfindex_addr: self.c.base_addr + rts as usize,
                delivered: 0,
                missed_since_start: 0,
            },
        });
        Ok(())
    }

    fn iso_start(&mut self, _ep: &IsoEp) -> Result<(u32, u32, &'static str), &'static str> {
        let Some(Built { mut pipe }) = self.built.take() else { return Err("not-configured") };
        // Anchor the schedule on MFINDEX now (`schedule` re-anchors whenever next_uf is not ahead of it), and
        // produce the whole target before anything can complete: the controller sees no TD until the doorbell.
        let now = pipe.mfindex();
        pipe.next_uf = now; // `ahead == 0` → the first `schedule` re-anchors to now + IST + margin
        let resync0 = ST_RESYNC.load(Ordering::Relaxed);
        let n = pipe.top_up();
        let _ = ST_RESYNC.swap(resync0, Ordering::Relaxed); // the start anchor is not a resync
        let first_frame = (now + pipe.ist_uf + START_MARGIN_UF) >> 3 & 0x7FF;
        let lead = pipe.target as u32 * pipe.interval_uf;
        serial_println!(
            "{} start slot={} dci={} tds={} slack={} stride={} td_len={} interval_uf={} ist_uf={} mfindex={} first_frame~{} lead_uframes={} sched=frameid ring={:#x} bufs={:#x}",
            if pipe.out || pipe.ain || pipe.fb { "[usb-iso]" } else { "[uvc-iso]" }, pipe.slot, pipe.dci, n, RING_SLACK, pipe.stride, pipe.td_len, pipe.interval_uf, pipe.ist_uf, now, first_frame, lead,
            pipe.ring_phys, pipe.bufs as u64
        );
        let (slot, dci) = (pipe.slot, pipe.dci);
        if pipe.out {
            // UAC1 (B706): the OUT pipe publishes to its own word; the IN statics are untouched.
            *OPIPE.lock() = Some(pipe);
            OPIPE_DCI.store(dci, Ordering::Relaxed);
            OPIPE_SLOT.store(slot, Ordering::Release);
            self.c.ring_doorbell(slot, dci as u32);
            return Ok((n as u32, lead, "frameid"));
        }
        if pipe.ain || pipe.fb {
            // UACMIC (B723): the audio IN pipe publishes to its own word; the camera's statics are untouched.
            // UACFEEDBACK (B789): the feedback pipe to ITS word, beside the capture pipe's.
            let (word, dword, cell) = if pipe.fb { (&FPIPE_SLOT, &FPIPE_DCI, &FPIPE) } else { (&APIPE_SLOT, &APIPE_DCI, &APIPE) };
            *cell.lock() = Some(pipe);
            dword.store(dci, Ordering::Relaxed);
            word.store(slot, Ordering::Release);
            self.c.ring_doorbell(slot, dci as u32);
            return Ok((n as u32, lead, "frameid"));
        }
        *PIPE.lock() = Some(pipe);
        ST_SIA.store(false, Ordering::Relaxed);
        PIPE_DCI.store(dci, Ordering::Relaxed);
        PIPE_SLOT.store(slot, Ordering::Release);
        self.c.ring_doorbell(slot, dci as u32);
        Ok((n as u32, lead, "frameid"))
    }

    fn iso_drop(&mut self, ep: &IsoEp) {
        let out = ep.addr & 0x80 == 0; // UAC1 (B706): the pipe of the endpoint's direction
        let dci = (ep.addr & 0x0F) as u32 * 2 + if out { 0 } else { 1 };
        let fb = !out && self.audio && ac::feedback_ep(ep.addr); // UACFEEDBACK (B789): the feedback pipe
        let ain = !out && self.audio && !fb; // UACMIC (B723): an audio slot's IN endpoint is the capture pipe
        let (word, pipe) = if out { (&OPIPE_SLOT, &OPIPE) } else if fb { (&FPIPE_SLOT, &FPIPE) } else if ain { (&APIPE_SLOT, &APIPE) } else { (&PIPE_SLOT, &PIPE) };
        if word.load(Ordering::Acquire) == self.slot {
            word.store(0, Ordering::Release);
            if let Some(p) = pipe.lock().take() {
                core::mem::forget(p); // see `disconnect`: the controller may still point at it
            }
        }
        if let Some(b) = self.built.take() {
            core::mem::forget(b); // the context took effect: the controller holds the dequeue pointer
        }
        let r = self.configure(dci, false, [0; 5]);
        serial_println!(
            "{} drop-ep slot={} dci={} result={} epstate={}",
            if out || ain || fb { "[usb-iso]" } else { "[uvc-iso]" }, self.slot, dci, match r { Ok(()) => "cc=1 (Success)", Err((_, n)) => n }, self.c.ep_state_of(self.slot, dci as u8)
        );
    }
}

impl XhciController {
    /// The main-loop pass (folded onto the FTDI ladder slot in `service_ftdi`): ONE camera candidate per pass,
    /// and never while a root port is mid-enumeration (the one-port-at-a-time invariant `service_hub_changes`
    /// keeps — every transfer here pumps the event ring the root FSM is driven from).
    pub(super) fn service_uvc(&mut self) { #[cfg(feature = "sonde")] crate::sonde::tap_service(); // SONDE3 (LEDGER SR108): the continuous depth tap — republishes /dev/probe/depth0 from every Nth completed frame, one decode per pass at most (2 ms bound), before the enum_active guard because it makes no USB transfer. LINE-NEUTRAL append, before this comment; a sonde-off image compiles this line as it was.
        if self.enum_active {
            return;
        }
        if let Some(slot) = ac::ep0_due() {
            self.service_capture(slot); // UACMIC (B723) / SINKALL (B767): the class driver's EP0 job (the playback volume, or a capture source opened or closed) — one per pass, first
            return;
        }
        let slot = pop_candidate();
        let (slot, audio) = if slot != 0 { (slot, false) } else if crate::boot::services_up() { (pop_audio(), true) } else { (0, true) }; // CRAWLC: an audio stream is a service — it waits in its queue for the services gate, as HDA does (BP-boot-3). UAC1 (B706): a camera first, else an audio device
        if slot == 0 {
            return;
        }
        let s = slot as usize;
        if s >= self.slots.len() || !self.slots[s].active || self.slots[s].ep0_ring.is_none() {
            serial_println!("[uvc-iso] candidate slot={} gone before service — skipped", slot);
            return;
        }
        let Some(buf) = ep0_buf() else {
            serial_println!("[uvc-iso] slot={} reason=no-ep0-buffer", slot);
            return;
        };
        // The same 64-byte window `configure_hid`/the hub walk read, now into the 4 KiB buffer.
        let ok = matches!(self.sync_control(slot, 0x80, 0x06, 0x0200, 0, 64, buf, true), Ok(1));
        let got = (self.last_control_len as usize).min(64);
        if !ok || got < 9 {
            serial_println!("[uvc-iso] slot={} cfg-window read failed ok={} got={}", slot, ok, got);
            return;
        }
        let mut win = [0u8; 64];
        // SAFETY: the EP0 buffer, `got` <= 64 bytes just written by the controller.
        win[..got].copy_from_slice(unsafe { core::slice::from_raw_parts(buf as *const u8, got) });
        let cfg_value = win[5]; // bConfigurationValue (USB 2.0 Table 9-10)
        crate::bootlog_println!("[uvc-iso] service slot={} vid={:04x} pid={:04x} cfg_value={}", slot, self.slots[s].vid, self.slots[s].pid, cfg_value);
        let (vid, pid) = (self.slots[s].vid, self.slots[s].pid);
        let mut host = XhciUvc { c: self, slot, buf, built: None, audio };
        if audio {
            ac::probe(slot, vid, pid, &win[..got], cfg_value, &mut host); // UAC1 (B706)
        } else {
            uvc::probe_iso(0, slot, &win[..got], cfg_value, &mut host);
        }
        if let Some(b) = host.built.take() {
            // `iso_configure` succeeded but no `iso_start`/`iso_drop` followed (an early return in the probe):
            // drop the endpoint so the reserved bandwidth is returned.
            let (dci, out) = (b.pipe.dci, b.pipe.out);
            core::mem::forget(b);
            let ep = IsoEp { addr: if out { dci / 2 } else { 0x80 | ((dci - 1) / 2) }, ..IsoEp::default() };
            host.iso_drop(&ep);
        }
    }

    /// UACMIC (B723): the EP0 job of the audio device on `slot` — the playback volume, or open or close its capture pipe (the class driver decides which,
    /// `uac::ep0_service`), in the service pass — the only context that may run EP0 and Configure Endpoint.
    fn service_capture(&mut self, slot: u8) {
        let s = slot as usize;
        if s >= self.slots.len() || !self.slots[s].active || self.slots[s].ep0_ring.is_none() {
            ac::capture_lost(slot, "slot-gone");
            return;
        }
        let Some(buf) = ep0_buf() else {
            ac::capture_lost(slot, "no-ep0-buffer");
            return;
        };
        let mut host = XhciUvc { c: self, slot, buf, built: None, audio: true };
        ac::ep0_service(&mut host);
        if let Some(b) = host.built.take() {
            // Configured but neither started nor dropped (an early return): give the bandwidth back.
            let dci = b.pipe.dci;
            core::mem::forget(b);
            host.iso_drop(&IsoEp { addr: 0x80 | ((dci - 1) / 2), ..IsoEp::default() });
        }
    }
}

/// HWREPORT (rmbp-ledger B725): the slot the isochronous IN pipe (the UVC stream) is open on, 0 = none — `hw::census`'s
/// read of which device the camera driver holds. Tail-appended.
pub fn in_slot() -> u8 {
    PIPE_SLOT.load(core::sync::atomic::Ordering::Relaxed)
}
