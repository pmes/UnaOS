//! CHARTER: Stria — driver
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
#![cfg(feature = "uac")] // UACMIC (B723) M1: the class is the `uac` feature's alone — a `uvc` image declares `drivers::usb` and compiles none of this file

//! UAC1 (rmbp-ledger B706) — USB Audio Class 1 PLAYBACK: a USB speaker, headset or DAC plays what the kernel writes.
//! UACMIC (rmbp-ledger B723) — and CAPTURE: a USB microphone or a headset's mic is read through `audio::source`.
//!
//! The class is the `uac` feature (UACMIC M1; `uac` implies `uvc`, whose isochronous engine it rides: XHCIISO,
//! `drivers/xhci/isoch.rs`, reached only through that file's `ac` door). Declared from `drivers/mod.rs` on the existing
//! `uvc` line (`pub mod usb { pub mod uac; }`) and erased by its own inner `#![cfg(feature = "uac")]` above, so a
//! `uvc`-without-`uac` image compiles none of it (the declaration's cfg word moving to `uac` is owed, cosmetic). Host-neutral like `drivers/uvc.rs`: every transfer goes through
//! `uvc::IsoHost` (EP0 `control`, `iso_configure` / `iso_start` / `iso_drop`); the xHCI host is `isoch::XhciUvc`.
//!
//! # THE SHAPE
//!
//! * **Candidates** — a configuration whose first 64 bytes carry an Audio Control interface (class 1, subclass 1)
//!   is queued by `isoch::note_audio_root` (the root config walk) or `isoch::note_downstream` (the hub walk), and
//!   taken one per pass by `isoch::service_uvc`, which calls [`probe`].
//! * **[`probe`]** — the `uvc::probe_iso` order: re-read the whole configuration into the 4 KiB EP0 buffer, [`parse`]
//!   it, pick the alternate [`pick`] names, SET_CONFIGURATION, SET_INTERFACE(alt 0), Configure Endpoint (host
//!   first: a bandwidth refusal leaves the device idle), SET_INTERFACE(alt), the sampling-frequency SET_CUR when
//!   the endpoint declares it, then `iso_start`. The stream runs from then on: silence until a writer.
//! * **The ring** — a 64 KiB single-producer single-consumer byte ring. The producer is the audio seam
//!   (`crate::audio`, under its one-writer guard); the consumer is the OUT pipe's TD fill (`isoch::Pipe::produce`,
//!   reached from the transfer-event claim), which takes NO lock here — head and tail are atomics.
//! * **Counters** — `frames` = 4-byte frames pulled from the ring; `underruns` = TD fills that came up short while a
//!   writer was active and had not finished (the drain after [`finish`] is not an underrun).
//! * **Capture** (UACMIC) — [`probe`] also records the first AudioStreaming alternate whose isochronous endpoint is IN
//!   and carries PCM, Type I, 16-bit in 2-byte subframes, mono or stereo, at 48 kHz (else 44.1 kHz) ([`pick_capture`]),
//!   and leaves that interface at alt 0: the microphone is NOT sampled at enumeration. A capture source opening
//!   (`audio::source`, [`capture_open`]) raises the wanted count; the service pass (`isoch::service_uvc`, the one
//!   context that may run EP0 and Configure Endpoint) sees [`capture_due`] and runs [`capture_service`]: Configure
//!   Endpoint (the audio IN pipe, `APIPE`), SET_INTERFACE(alt), the sampling-frequency SET_CUR when declared,
//!   `iso_start`. The last source closing runs it the other way: SET_INTERFACE(0), drop the endpoint. [`capture_armed`]
//!   — the IN pipe runs — is the ONE count the menu bar's mic dot paints from (`video::menubar::micdot`). Completed
//!   TDs land in a 64 KiB single-producer single-consumer ring ([`push_capture`] from the claim path, lock-free;
//!   [`read_capture`] from the one reader); a push that does not fit counts an overrun and drops the newest bytes.
//!
//! # SOURCES (DRIVERS-METHOD §5)
//!
//! USB Device Class Definition for Audio Devices 1.0: §4.3.2 (class-specific AC interface header, bcdADC), §4.5.2
//! (AS_GENERAL: bTerminalLink, wFormatTag), §4.6.1.2 (class-specific AS isochronous endpoint: bmAttributes bit 0 =
//! Sampling Frequency control), §5.2.3.2.3.1 (Sampling Frequency Control: SET_CUR, bmRequestType 0x22, wValue
//! 0x0100, wIndex = endpoint, 3-byte rate), A.5/A.6 (descriptor subtypes). Audio Data Formats 1.0: §2.2.5 (Type I
//! format descriptor: bNrChannels, bSubframeSize, bBitResolution, bSamFreqType, tSamFreq), A.1.1 (wFormatTag 1 = PCM).
//! USB 2.0 Table 9-12/9-13 (interface, endpoint; the audio endpoint is 9 bytes with bRefresh/bSynchAddress). QEMU's
//! `hw/usb/dev-audio.c` (recalled, not re-read on this arc) is the fixture's shape: full-speed, AC interface 0, AS
//! interface 1 alt 1, OUT 0x01 adaptive, 192-byte packets at 48 kHz stereo, no sampling-frequency control.

use crate::drivers::uvc::{IsoEp, IsoHost};
use alloc::string::String;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

// ── Constants ───────────────────────────────────────────────────────────────────────────────────────────

const DT_INTERFACE: u8 = 0x04; // USB 2.0 Table 9-5
const DT_ENDPOINT: u8 = 0x05;
const DT_CS_INTERFACE: u8 = 0x24; // UAC1 A.4
const DT_CS_ENDPOINT: u8 = 0x25;
const CC_AUDIO: u8 = 0x01; // UAC1 A.1
const CC_HID: u8 = 0x03;
const SC_AUDIOCONTROL: u8 = 0x01; // UAC1 A.2
const SC_AUDIOSTREAMING: u8 = 0x02;
const AC_HEADER: u8 = 0x01; // UAC1 A.5
const AS_GENERAL: u8 = 0x01; // UAC1 A.6
const AS_FORMAT_TYPE: u8 = 0x02;
const EP_GENERAL: u8 = 0x01; // UAC1 A.8
const FORMAT_PCM: u16 = 0x0001; // Frmts A.1.1
const FORMAT_TYPE_I: u8 = 0x01; // Frmts A.1.2
const RQ_SET_CUR: u8 = 0x01; // UAC1 A.9
const SAMPLING_FREQ_CONTROL: u16 = 0x01; // UAC1 A.10.2 (endpoint control selector)
/// The one format this arc plays: PCM 16-bit, two channels, 48 kHz.
pub const RATE: u32 = 48_000;
pub const CHANNELS: u8 = 2;
pub const BITS: u8 = 16;
const FRAME_BYTES: u32 = 4;
/// The streaming alternates recorded per configuration.
const MAX_ALTS: usize = 8;
/// The playback ring: 64 KiB = 341 ms at 192 000 bytes/s.
const RING: usize = 64 * 1024;
/// UACMIC: the capture rates, in preference order (Type I, 16-bit, mono or stereo).
pub const CAP_RATES: [u32; 2] = [48_000, 44_100];

// ── Descriptor walk ─────────────────────────────────────────────────────────────────────────────────────

fn le16(b: &[u8], o: usize) -> u16 {
    b[o] as u16 | (b[o + 1] as u16) << 8
}

fn le24(b: &[u8], o: usize) -> u32 {
    b[o] as u32 | (b[o + 1] as u32) << 8 | (b[o + 2] as u32) << 16
}

/// Any interface of `class` (and `sub`, unless `None`) in the descriptor window.
fn has_intf(cfg: &[u8], class: u8, sub: Option<u8>) -> bool {
    let mut off = 0usize;
    while off + 2 <= cfg.len() {
        let len = cfg[off] as usize;
        if len < 2 {
            break;
        }
        if cfg[off + 1] == DT_INTERFACE && off + 8 <= cfg.len() && cfg[off + 5] == class && sub.map(|s| cfg[off + 6] == s).unwrap_or(true) {
            return true;
        }
        off += len;
    }
    false
}

/// An Audio Control interface in the window: the device is a candidate.
pub fn cfg_has_audio(cfg: &[u8]) -> bool {
    has_intf(cfg, CC_AUDIO, Some(SC_AUDIOCONTROL))
}

/// A HID interface in the same window (a headset's buttons): the HID path owns the root port's advance.
pub fn cfg_has_hid(cfg: &[u8]) -> bool {
    has_intf(cfg, CC_HID, None)
}

/// One AudioStreaming alternate that carries an endpoint.
#[derive(Clone, Copy, Default)]
pub struct Alt {
    pub intf: u8,
    pub alt: u8,
    pub fmt_tag: u16,
    pub ftype: u8,
    pub nch: u8,
    pub subframe: u8,
    pub bits: u8,
    pub rates: [u32; 4],
    pub nrates: usize,
    pub continuous: bool,
    pub lo: u32,
    pub hi: u32,
    pub ep: IsoEp,
    pub have_ep: bool,
    /// The class-specific endpoint's bmAttributes bit 0: the sampling frequency is settable (§4.6.1.2).
    pub freq_ctl: bool,
}

impl Alt {
    fn takes(&self, hz: u32) -> bool {
        if self.continuous {
            self.lo <= hz && hz <= self.hi
        } else {
            self.rates[..self.nrates].contains(&hz)
        }
    }

    /// PCM, Type I, 2 x 16-bit in 2-byte subframes, 48 kHz, on an isochronous OUT endpoint.
    pub fn plays_ours(&self) -> bool {
        self.fmt_tag == FORMAT_PCM
            && self.ftype == FORMAT_TYPE_I
            && self.nch == CHANNELS
            && self.subframe == 2
            && self.bits == BITS
            && self.have_ep
            && self.ep.addr & 0x80 == 0
            && self.takes(RATE)
    }

    /// UACMIC: PCM, Type I, 1 or 2 x 16-bit in 2-byte subframes, at `hz`, on an isochronous IN endpoint.
    pub fn captures(&self, hz: u32) -> bool {
        self.fmt_tag == FORMAT_PCM
            && self.ftype == FORMAT_TYPE_I
            && (self.nch == 1 || self.nch == 2)
            && self.subframe == 2
            && self.bits == BITS
            && self.have_ep
            && self.ep.addr & 0x80 != 0
            && self.takes(hz)
    }
}

/// The configuration as this driver reads it.
#[derive(Clone, Copy, Default)]
pub struct Parsed {
    pub have_ac: bool,
    pub ac_intf: u8,
    pub bcd_adc: u16,
    pub as_seen: u8,
    pub alts: [Alt; MAX_ALTS],
    pub nalt: usize,
    pub ran_short: bool,
}

/// Walk a full configuration descriptor (UAC1 §4.3-§4.6, Frmts §2.2.5).
pub fn parse(cfg: &[u8]) -> Parsed {
    let mut p = Parsed::default();
    let (mut in_ac, mut cur): (bool, Option<usize>) = (false, None);
    let mut off = 0usize;
    while off + 2 <= cfg.len() {
        let len = cfg[off] as usize;
        if len < 2 {
            break;
        }
        if off + len > cfg.len() {
            p.ran_short = true;
            break;
        }
        let d = &cfg[off..off + len];
        match d[1] {
            // Table 9-12: bInterfaceNumber[2] bAlternateSetting[3] bNumEndpoints[4] class[5] subclass[6].
            DT_INTERFACE if len >= 9 => {
                in_ac = d[5] == CC_AUDIO && d[6] == SC_AUDIOCONTROL;
                let in_as = d[5] == CC_AUDIO && d[6] == SC_AUDIOSTREAMING;
                cur = None;
                if in_ac && !p.have_ac {
                    p.have_ac = true;
                    p.ac_intf = d[2];
                }
                if in_as {
                    p.as_seen = p.as_seen.saturating_add(1);
                    if d[4] > 0 && p.nalt < MAX_ALTS {
                        p.alts[p.nalt] = Alt { intf: d[2], alt: d[3], ..Alt::default() };
                        cur = Some(p.nalt);
                        p.nalt += 1;
                    }
                }
            }
            DT_CS_INTERFACE if len >= 3 => {
                // §4.3.2: HEADER bcdADC[3..5].
                if in_ac && d[2] == AC_HEADER && len >= 5 {
                    p.bcd_adc = le16(d, 3);
                }
                if let Some(i) = cur {
                    let a = &mut p.alts[i];
                    if d[2] == AS_GENERAL && len >= 7 {
                        a.fmt_tag = le16(d, 5); // §4.5.2: bTerminalLink[3] bDelay[4] wFormatTag[5..7]
                    } else if d[2] == AS_FORMAT_TYPE && len >= 8 {
                        // Frmts §2.2.5: bFormatType[3] bNrChannels[4] bSubframeSize[5] bBitResolution[6] bSamFreqType[7].
                        a.ftype = d[3];
                        a.nch = d[4];
                        a.subframe = d[5];
                        a.bits = d[6];
                        let nf = d[7] as usize;
                        if nf == 0 {
                            if len >= 14 {
                                a.continuous = true;
                                a.lo = le24(d, 8);
                                a.hi = le24(d, 11);
                            }
                        } else {
                            for k in 0..nf {
                                let o = 8 + 3 * k;
                                if o + 3 > len || a.nrates >= a.rates.len() {
                                    break;
                                }
                                a.rates[a.nrates] = le24(d, o);
                                a.nrates += 1;
                            }
                        }
                    }
                }
            }
            // Table 9-13: bEndpointAddress[2] bmAttributes[3] (bits 1..0 = 01 isochronous) wMaxPacketSize[4..6] bInterval[6].
            DT_ENDPOINT if len >= 7 => {
                if let Some(i) = cur {
                    let a = &mut p.alts[i];
                    if !a.have_ep && (d[3] & 0x03) == 0x01 {
                        let w = le16(d, 4);
                        a.ep = IsoEp { intf: a.intf, alt: a.alt, addr: d[2], mps: w & 0x07FF, hs_extra: ((w >> 11) & 0x03) as u8, interval: d[6], ..IsoEp::default() };
                        a.have_ep = true;
                    }
                }
            }
            // §4.6.1.2: EP_GENERAL bmAttributes[3] bit 0 = Sampling Frequency.
            DT_CS_ENDPOINT if len >= 4 => {
                if let Some(i) = cur {
                    if d[2] == EP_GENERAL {
                        p.alts[i].freq_ctl = d[3] & 0x01 != 0;
                    }
                }
            }
            _ => {}
        }
        off += len;
    }
    p
}

/// The first alternate that plays PCM 16-bit / 2 ch / 48 kHz on an OUT endpoint.
pub fn pick(p: &Parsed) -> Option<Alt> {
    p.alts[..p.nalt].iter().find(|a| a.plays_ours()).copied()
}

/// UACMIC: the capture alternate and its rate — 48 kHz on any alternate first, else 44.1 kHz.
pub fn pick_capture(p: &Parsed) -> Option<(Alt, u32)> {
    CAP_RATES.iter().find_map(|&hz| p.alts[..p.nalt].iter().find(|a| a.captures(hz)).map(|a| (*a, hz)))
}

/// Bytes one service interval of `interval_uf` microframes plays: 48 000 x 4 x interval / 8000 = 24 per microframe
/// (192 per full-speed frame) — always a whole number of frames.
pub fn td_bytes(interval_uf: u32) -> u32 {
    RATE * FRAME_BYTES * interval_uf / 8000
}

/// The OUT pipe's TD length (called by `isoch`'s `iso_configure` for an OUT endpoint); recorded as this stream's packet.
pub fn out_td_len(interval_uf: u32) -> u32 {
    let n = td_bytes(interval_uf);
    PKT.store(n, Ordering::Relaxed);
    n
}

// ── The stream ──────────────────────────────────────────────────────────────────────────────────────────

struct RingBuf(UnsafeCell<[u8; RING]>);
// SAFETY: single producer (the `crate::audio` writer, under WRITER) and single consumer (the OUT pipe's fill, under
// the pipe lock); each touches only the bytes its own index owns, published with Release / read with Acquire.
unsafe impl Sync for RingBuf {}
static BUF: RingBuf = RingBuf(UnsafeCell::new([0; RING]));
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);
static WRITER: AtomicBool = AtomicBool::new(false);
static PLAYING: AtomicBool = AtomicBool::new(false);
static ENDED: AtomicBool = AtomicBool::new(false);
static FRAMES: AtomicU64 = AtomicU64::new(0);
static UNDERRUNS: AtomicU32 = AtomicU32::new(0);

static ENUMERATED: AtomicBool = AtomicBool::new(false);
static STREAMING: AtomicBool = AtomicBool::new(false);
static SLOT: AtomicU8 = AtomicU8::new(0);
static VIDPID: AtomicU32 = AtomicU32::new(0);
static PKT: AtomicU32 = AtomicU32::new(0);
static WHY: crate::sync::Mutex<&'static str> = crate::sync::Mutex::new("none");

/// THE CONSUMER — one OUT TD's payload. Copies whole frames from the ring and zero-fills the rest.
pub fn pull(dst: &mut [u8]) {
    let h = HEAD.load(Ordering::Acquire);
    let t = TAIL.load(Ordering::Relaxed);
    let avail = h.wrapping_sub(t);
    let n = avail.min(dst.len()) & !3;
    let base = BUF.0.get() as *const u8;
    for (k, b) in dst[..n].iter_mut().enumerate() {
        // SAFETY: index < RING; these bytes were published by the producer's Release store of HEAD.
        *b = unsafe { *base.add((t + k) % RING) };
    }
    TAIL.store(t.wrapping_add(n), Ordering::Release);
    for b in dst[n..].iter_mut() {
        *b = 0;
    }
    if n > 0 {
        FRAMES.fetch_add((n / FRAME_BYTES as usize) as u64, Ordering::Relaxed);
    }
    if n < dst.len() && PLAYING.load(Ordering::Acquire) {
        if ENDED.load(Ordering::Acquire) {
            PLAYING.store(false, Ordering::Release); // the writer finished and the ring is drained
        } else {
            UNDERRUNS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Take the one writer's seat. Err when the stream is not running or another writer holds it.
pub fn begin() -> Result<(), &'static str> {
    if !STREAMING.load(Ordering::Acquire) {
        return Err("no-stream");
    }
    if WRITER.swap(true, Ordering::AcqRel) {
        return Err("writer-busy");
    }
    ENDED.store(false, Ordering::Release);
    Ok(())
}

/// THE PRODUCER — append whole frames; returns the bytes taken (0 = the ring is full).
pub fn write(src: &[u8]) -> usize {
    let t = TAIL.load(Ordering::Acquire);
    let h = HEAD.load(Ordering::Relaxed);
    let free = RING - h.wrapping_sub(t);
    let n = free.min(src.len()) & !3;
    let base = BUF.0.get() as *mut u8;
    for (k, b) in src[..n].iter().enumerate() {
        // SAFETY: index < RING; the consumer has released these bytes (they lie past the head, inside `free`).
        unsafe { *base.add((h + k) % RING) = *b };
    }
    HEAD.store(h.wrapping_add(n), Ordering::Release);
    if n > 0 {
        PLAYING.store(true, Ordering::Release);
    }
    n
}

/// The writer is done: the ring drains, and a short fill after this is the end, not an underrun.
pub fn finish() {
    ENDED.store(true, Ordering::Release);
    WRITER.store(false, Ordering::Release);
}

/// Bytes written and not yet pulled.
pub fn queued() -> usize {
    HEAD.load(Ordering::Acquire).wrapping_sub(TAIL.load(Ordering::Acquire))
}

pub fn streaming() -> bool {
    STREAMING.load(Ordering::Acquire)
}

/// (frames pulled, underruns) since boot.
pub fn counters() -> (u64, u32) {
    (FRAMES.load(Ordering::Relaxed), UNDERRUNS.load(Ordering::Relaxed))
}

/// (vid, pid, packet bytes) of the streaming device.
pub fn device() -> Option<(u16, u16, u32)> {
    if !STREAMING.load(Ordering::Acquire) {
        return None;
    }
    let v = VIDPID.load(Ordering::Relaxed);
    Some(((v >> 16) as u16, v as u16, PKT.load(Ordering::Relaxed)))
}

/// `[audio] usb vid=<x> pid=<x> fmt=pcm16/2/48000 isoch_pkt=<n> underruns=<n>`
pub fn wire() {
    if let Some((vid, pid, pkt)) = device() {
        serial_println!("[audio] usb vid={:04x} pid={:04x} fmt=pcm{}/{}/{} isoch_pkt={} underruns={}", vid, pid, BITS, CHANNELS, RATE, pkt, UNDERRUNS.load(Ordering::Relaxed));
    }
}

fn fail(slot: u8, stage: &str, why: &'static str) {
    *WHY.lock() = why;
    serial_println!("[uac] abort slot={} stage={} reason={}", slot, stage, why);
}

// ── Bring-up ────────────────────────────────────────────────────────────────────────────────────────────

/// One audio candidate: `win` is the configuration window the service read, `cfg_value` its bConfigurationValue.
pub fn probe(slot: u8, vid: u16, pid: u16, win: &[u8], cfg_value: u8, h: &mut dyn IsoHost) {
    if STREAMING.load(Ordering::Acquire) || CAP.lock().is_some() {
        serial_println!("[uac] skip slot={} {:04x}:{:04x} reason=one-stream-already-armed", slot, vid, pid);
        return;
    }
    // CRAWLC: ENUMERATED / SLOT / VIDPID are stored after `pick` (below): a hubbed webcam's microphone (IN only, no
    // OUT alternate in our format) is not ours, so it neither flips `tests uac1` to enumerated=1 nor takes the slot
    // `disconnect` keys on.
    let cap = h.cap();
    let wtotal = if win.len() >= 4 { le16(win, 2) } else { 0 };
    let want = (wtotal as usize).min(cap) as u16;
    let n = match h.control(0x80, 6, 0x0200, 0, want, true) {
        Ok(n) if n >= 9 => (n as usize).min(cap),
        Ok(_) => return fail(slot, "cfg-reread", "runt"),
        Err(e) => return fail(slot, "cfg-reread", e),
    };
    // SAFETY: the host's EP0 buffer, valid for `cap` bytes; the transfer above wrote `n` <= cap. Read before the
    // next transfer reuses it.
    let full = unsafe { core::slice::from_raw_parts(h.buf() as *const u8, n) };
    let p = parse(full);
    crate::census_println!(
        "[uac] dev={:04x}:{:04x} slot={} host={} cfg_value={} wTotalLength={} read={} ac_intf={} bcdADC={:#06x} as_intfs={} alts={}{}",
        vid, pid, slot, h.name(), cfg_value, wtotal, n, p.ac_intf, p.bcd_adc, p.as_seen, p.nalt, if p.ran_short { " ran_short=1" } else { "" }
    );
    for a in p.alts[..p.nalt].iter() {
        crate::census_println!(
            "[uac] alt intf={} alt={} fmt={:#06x} type={} ch={} sub={} bits={} rates={}{} ep={:#04x} mps={} interval={} freq_ctl={} ours={}",
            a.intf, a.alt, a.fmt_tag, a.ftype, a.nch, a.subframe, a.bits, if a.continuous { a.lo } else { a.rates[0] }, if a.continuous { "+" } else { "" },
            a.ep.addr, a.ep.mps, a.ep.interval, a.freq_ctl as u8, a.plays_ours() as u8
        ); // UACMIC: `ours=1` is the playback pick; the capture pick is said on its own `[uac] capture ready` line

    }
    if !p.have_ac {
        return fail(slot, "parse", "no-audio-control");
    }
    if p.bcd_adc >= 0x0200 {
        return fail(slot, "parse", "uac2-not-built"); // a UAC2 device (clock entities, a different AS format layout)
    }
    let play = pick(&p);
    let cap = pick_capture(&p);
    if play.is_none() && cap.is_none() {
        return fail(slot, "pick", "no-pcm16-alternate");
    }
    ENUMERATED.store(true, Ordering::Release); SLOT.store(slot, Ordering::Release); VIDPID.store((vid as u32) << 16 | pid as u32, Ordering::Relaxed); // CRAWLC (finding 9): the device is ours only once an alternate of ours was picked (playback or capture)
    if h.control(0x00, 9, cfg_value as u16, 0, 0, false).is_err() {
        return fail(slot, "set-configuration", "stalled-or-timeout");
    }
    if let Some((c, hz)) = cap {
        // UACMIC: recorded, left at alt 0 (zero bandwidth, nothing sampled) until a source opens it.
        if let Err(e) = h.control(0x01, 11, 0, c.intf as u16, 0, false) {
            serial_println!("[uac] set-interface intf={} alt=0 reason={} (continuing)", c.intf, e);
        }
        *CAP.lock() = Some(Cap { slot, alt: c, rate: hz });
        CAP_FAIL.store(false, Ordering::Release);
        *CAP_WHY.lock() = "idle";
        serial_println!(
            "[uac] capture ready slot={} intf={} alt={} ep={:#04x} mps={} interval={} freq_ctl={} fmt=pcm{}/{}/{} -> idle (opens on audio::source)",
            slot, c.intf, c.alt, c.ep.addr, c.ep.mps, c.ep.interval, c.freq_ctl as u8, BITS, c.nch, hz
        );
    }
    let Some(a) = play else {
        *WHY.lock() = "no-playback-alternate";
        serial_println!("[uac] playback none slot={} reason=no-pcm16-2ch-48k-out-alternate", slot);
        return;
    };
    // SET_INTERFACE(alt 0) first: the zero-bandwidth setting, so the device starts from idle (UAC1 §3.7.2.1).
    if let Err(e) = h.control(0x01, 11, 0, a.intf as u16, 0, false) {
        serial_println!("[uac] set-interface intf={} alt=0 reason={} (continuing)", a.intf, e);
    }
    if let Err((cc, name)) = h.iso_configure(&a.ep) {
        serial_println!("[uac] configure-endpoint refused cc={} ({}) ep={:#04x} mps={}", cc, name, a.ep.addr, a.ep.mps);
        return fail(slot, "configure-endpoint", name);
    }
    if let Err(e) = h.control(0x01, 11, a.alt as u16, a.intf as u16, 0, false) {
        h.iso_drop(&a.ep);
        return fail(slot, "set-interface", e);
    }
    if a.freq_ctl {
        // §5.2.3.2.3.1: 3-byte tSampleFreq, little-endian.
        // SAFETY: the EP0 buffer, `cap` >= 3 bytes; no transfer in flight.
        unsafe {
            let b = h.buf();
            *b = RATE as u8;
            *b.add(1) = (RATE >> 8) as u8;
            *b.add(2) = (RATE >> 16) as u8;
        }
        let r = h.control(0x22, RQ_SET_CUR, SAMPLING_FREQ_CONTROL << 8, a.ep.addr as u16, 3, false);
        serial_println!("[uac] set-cur sampling_freq={} ep={:#04x} result={}", RATE, a.ep.addr, match r { Ok(_) => "ok", Err(e) => e });
    }
    TAIL.store(HEAD.load(Ordering::Acquire), Ordering::Release); match h.iso_start(&a.ep) { // CRAWLC: an aborted tone's bytes left in the ring are dropped before the first TD fill (no consumer runs yet)
        Ok((tds, lead, sched)) => {
            STREAMING.store(true, Ordering::Release);
            *WHY.lock() = "ok";
            serial_println!("[uac] stream slot={} intf={} alt={} ep={:#04x} tds={} lead_uframes={} sched={} -> streaming", slot, a.intf, a.alt, a.ep.addr, tds, lead, sched);
            wire();
            crate::audio::usb_arrived(vid, pid);
        }
        Err(e) => {
            h.iso_drop(&a.ep);
            fail(slot, "iso-start", e);
        }
    }
}

/// The device left (or its slot was reset): the stream and the sink go with it.
pub fn disconnect(slot: u8) {
    if slot == 0 || SLOT.load(Ordering::Acquire) != slot {
        return;
    }
    capture_lost(slot, "gone"); // UACMIC: the capture device goes with it (the pipe went in `isoch::disconnect`)
    SLOT.store(0, Ordering::Release);
    let was = STREAMING.swap(false, Ordering::AcqRel);
    ENUMERATED.store(false, Ordering::Release);
    *WHY.lock() = "gone";
    if was {
        serial_println!("[uac] disconnect slot={} — stream and sink gone", slot);
        crate::audio::usb_gone();
    }
}

// ── UACMIC (rmbp-ledger B723): capture ─────────────────────────────────────────────────────────────────────

/// The capture device the probe recorded: its slot, the alternate, the rate it runs at.
#[derive(Clone, Copy)]
struct Cap {
    slot: u8,
    alt: Alt,
    rate: u32,
}

static CAP: crate::sync::Mutex<Option<Cap>> = crate::sync::Mutex::new(None);
/// Open sources (`audio::source` handles alive).
static CAP_WANT: AtomicU32 = AtomicU32::new(0);
/// THE DRIVER'S FACT: the capture pipe runs (from `iso_start` to the drop). The mic dot paints from this.
static CAP_ARMED: AtomicBool = AtomicBool::new(false);
/// An open failed: no retry until the wanted count next rises from 0.
static CAP_FAIL: AtomicBool = AtomicBool::new(false);
static CAP_WHY: crate::sync::Mutex<&'static str> = crate::sync::Mutex::new("none");
/// Capture streams opened since boot.
static CAP_OPENS: AtomicU32 = AtomicU32::new(0);
/// `channels << 24 | rate` of the running stream.
static CFMT: AtomicU32 = AtomicU32::new(0);
static CBUF: RingBuf = RingBuf(UnsafeCell::new([0; RING]));
static CHEAD: AtomicUsize = AtomicUsize::new(0);
static CTAIL: AtomicUsize = AtomicUsize::new(0);
static CFRAMES: AtomicU64 = AtomicU64::new(0);
static COVERRUNS: AtomicU32 = AtomicU32::new(0);

fn cap_frame_bytes() -> usize {
    (((CFMT.load(Ordering::Relaxed) >> 24) as usize).clamp(1, 2)) * 2
}

/// THE CAPTURE PRODUCER — one completed IN TD's bytes (the claim path; lock-free). Whole frames only; what does not
/// fit is dropped and counted as one overrun.
pub fn push_capture(src: &[u8]) {
    let fb = cap_frame_bytes();
    let want = src.len() - src.len() % fb;
    if want == 0 {
        return;
    }
    let t = CTAIL.load(Ordering::Acquire);
    let h = CHEAD.load(Ordering::Relaxed);
    let free = RING - h.wrapping_sub(t);
    let n = free.min(want) / fb * fb;
    let base = CBUF.0.get() as *mut u8;
    for (k, b) in src[..n].iter().enumerate() {
        // SAFETY: index < RING; these bytes lie past the head, inside `free`: the reader has released them.
        unsafe { *base.add((h + k) % RING) = *b };
    }
    CHEAD.store(h.wrapping_add(n), Ordering::Release);
    if n < want {
        COVERRUNS.fetch_add(1, Ordering::Relaxed);
    }
    CFRAMES.fetch_add((n / fb) as u64, Ordering::Relaxed);
}

/// THE CAPTURE CONSUMER — whole frames into `dst`; returns the bytes read (0 = nothing captured yet).
pub fn read_capture(dst: &mut [u8]) -> usize {
    let fb = cap_frame_bytes();
    let h = CHEAD.load(Ordering::Acquire);
    let t = CTAIL.load(Ordering::Relaxed);
    let n = h.wrapping_sub(t).min(dst.len()) / fb * fb;
    let base = CBUF.0.get() as *const u8;
    for (k, b) in dst[..n].iter_mut().enumerate() {
        // SAFETY: index < RING; published by the producer's Release store of CHEAD.
        *b = unsafe { *base.add((t + k) % RING) };
    }
    CTAIL.store(t.wrapping_add(n), Ordering::Release);
    n
}

/// A source opens: Ok((channels, rate)). The first opener discards whatever an earlier session left in the ring (no
/// reader holds it then), so one app never reads another's audio.
pub fn capture_open() -> Result<(u8, u32), &'static str> {
    let Some(c) = *CAP.lock() else { return Err("no-capture-device") };
    if CAP_WANT.fetch_add(1, Ordering::AcqRel) == 0 {
        CTAIL.store(CHEAD.load(Ordering::Acquire), Ordering::Release);
        CAP_FAIL.store(false, Ordering::Release);
    }
    Ok((c.alt.nch, c.rate))
}

/// A source closed (its handle dropped).
pub fn capture_close() {
    let _ = CAP_WANT.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
}

/// The capture pipe runs — THE count the mic dot paints from (0 or 1: the class driver holds one capture stream).
#[inline]
pub fn capture_armed() -> u32 {
    CAP_ARMED.load(Ordering::Acquire) as u32
}

/// `(opens since boot, frames captured, overruns)`.
pub fn capture_counters() -> (u32, u64, u32) {
    (CAP_OPENS.load(Ordering::Relaxed), CFRAMES.load(Ordering::Relaxed), COVERRUNS.load(Ordering::Relaxed))
}

/// The slot whose capture pipe must open or close on this service pass (the wanted state differs from the armed one).
pub fn capture_due() -> Option<u8> {
    let want = CAP_WANT.load(Ordering::Acquire) != 0;
    let armed = CAP_ARMED.load(Ordering::Acquire);
    if want == armed || (want && CAP_FAIL.load(Ordering::Acquire)) {
        return None;
    }
    CAP.try_lock()?.as_ref().map(|c| c.slot)
}

/// `[audio] usb capture vid=<x> pid=<x> fmt=pcm16/<ch>/<rate> frames=<n> overruns=<n>`
pub fn capture_wire(state: &str) {
    let Some(c) = *CAP.lock() else { return };
    let v = VIDPID.load(Ordering::Relaxed);
    let (_, f, o) = capture_counters();
    serial_println!(
        "[audio] usb capture vid={:04x} pid={:04x} fmt=pcm{}/{}/{} frames={} overruns={} state={}",
        v >> 16, v & 0xFFFF, BITS, c.alt.nch, c.rate, f, o, state
    );
}

fn cap_fail(slot: u8, stage: &str, why: &'static str) {
    *CAP_WHY.lock() = why;
    CAP_FAIL.store(true, Ordering::Release);
    serial_println!("[uac] capture abort slot={} stage={} reason={}", slot, stage, why);
}

/// The service pass's half (task context, EP0 free): open the capture pipe while a source wants it, close it when the
/// last source left. One direction per call.
pub fn capture_service(h: &mut dyn IsoHost) {
    let Some(c) = *CAP.lock() else { return };
    let want = CAP_WANT.load(Ordering::Acquire) != 0;
    let armed = CAP_ARMED.load(Ordering::Acquire);
    if want && !armed {
        CFMT.store((c.alt.nch as u32) << 24 | c.rate, Ordering::Relaxed);
        if let Err((cc, name)) = h.iso_configure(&c.alt.ep) {
            serial_println!("[uac] capture configure-endpoint refused cc={} ({}) ep={:#04x} mps={}", cc, name, c.alt.ep.addr, c.alt.ep.mps);
            return cap_fail(c.slot, "configure-endpoint", name);
        }
        if let Err(e) = h.control(0x01, 11, c.alt.alt as u16, c.alt.intf as u16, 0, false) {
            h.iso_drop(&c.alt.ep);
            return cap_fail(c.slot, "set-interface", e);
        }
        if c.alt.freq_ctl {
            // §5.2.3.2.3.1: 3-byte tSampleFreq, little-endian, to the endpoint.
            // SAFETY: the EP0 buffer, `cap` >= 3 bytes; no transfer in flight.
            unsafe {
                let b = h.buf();
                *b = c.rate as u8;
                *b.add(1) = (c.rate >> 8) as u8;
                *b.add(2) = (c.rate >> 16) as u8;
            }
            let r = h.control(0x22, RQ_SET_CUR, SAMPLING_FREQ_CONTROL << 8, c.alt.ep.addr as u16, 3, false);
            serial_println!("[uac] set-cur sampling_freq={} ep={:#04x} result={}", c.rate, c.alt.ep.addr, match r { Ok(_) => "ok", Err(e) => e });
        }
        match h.iso_start(&c.alt.ep) {
            Ok((tds, lead, sched)) => {
                CAP_ARMED.store(true, Ordering::Release);
                let n = CAP_OPENS.fetch_add(1, Ordering::Relaxed) + 1;
                *CAP_WHY.lock() = "ok";
                serial_println!(
                    "[uac] capture slot={} intf={} alt={} ep={:#04x} tds={} lead_uframes={} sched={} opens={} -> streaming",
                    c.slot, c.alt.intf, c.alt.alt, c.alt.ep.addr, tds, lead, sched, n
                );
                capture_wire("open");
            }
            Err(e) => {
                let _ = h.control(0x01, 11, 0, c.alt.intf as u16, 0, false);
                h.iso_drop(&c.alt.ep);
                cap_fail(c.slot, "iso-start", e);
            }
        }
    } else if !want && armed {
        // Device first (it stops sampling), then the host's endpoint; the ring and TD buffers are left to the
        // controller as every other drop here (`isoch::iso_drop`): ~5 KiB per open on a full-speed microphone.
        if let Err(e) = h.control(0x01, 11, 0, c.alt.intf as u16, 0, false) {
            serial_println!("[uac] capture set-interface intf={} alt=0 reason={} (dropping the endpoint anyway)", c.alt.intf, e);
        }
        h.iso_drop(&c.alt.ep);
        CAP_ARMED.store(false, Ordering::Release);
        *CAP_WHY.lock() = "idle";
        serial_println!("[uac] capture slot={} intf={} -> idle", c.slot, c.alt.intf);
        capture_wire("closed");
    }
}

/// The capture device's slot left (or the service could not reach it): the stream and the dot go with it.
pub fn capture_lost(slot: u8, why: &'static str) {
    let mut g = CAP.lock();
    if g.map(|c| c.slot) != Some(slot) {
        return;
    }
    if why == "gone" {
        *g = None;
    }
    drop(g);
    let was = CAP_ARMED.swap(false, Ordering::AcqRel);
    if why != "gone" {
        CAP_FAIL.store(true, Ordering::Release);
    }
    *CAP_WHY.lock() = why;
    serial_println!("[uac] capture lost slot={} reason={} was_streaming={}", slot, why, was as u8);
}

// ── `tests uac1` ────────────────────────────────────────────────────────────────────────────────────────

/// QEMU `usb-audio`'s stereo configuration as this driver expects it (110 bytes): AC interface 0 (header bcdADC
/// 1.00, input terminal 0x0101 USB streaming, feature unit, output terminal 0x0301 speaker), AS interface 1 alt 0
/// (no endpoint) and alt 1 (PCM, Type I 2 x 16-bit, 48 kHz; OUT 0x01 isochronous adaptive, 192 bytes, every frame).
const FIXTURE: [u8; 110] = [
    0x09, 0x02, 0x6e, 0x00, 0x02, 0x01, 0x00, 0x80, 0x32, //
    0x09, 0x04, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, //
    0x09, 0x24, 0x01, 0x00, 0x01, 0x28, 0x00, 0x01, 0x01, //
    0x0c, 0x24, 0x02, 0x01, 0x01, 0x01, 0x00, 0x02, 0x03, 0x00, 0x00, 0x00, //
    0x0a, 0x24, 0x06, 0x02, 0x01, 0x01, 0x01, 0x02, 0x02, 0x00, //
    0x09, 0x24, 0x03, 0x03, 0x01, 0x03, 0x00, 0x02, 0x00, //
    0x09, 0x04, 0x01, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, //
    0x09, 0x04, 0x01, 0x01, 0x01, 0x01, 0x02, 0x00, 0x00, //
    0x07, 0x24, 0x01, 0x01, 0x01, 0x01, 0x00, //
    0x0b, 0x24, 0x02, 0x01, 0x02, 0x02, 0x10, 0x01, 0x80, 0xbb, 0x00, //
    0x09, 0x05, 0x01, 0x0d, 0xc0, 0x00, 0x01, 0x00, 0x00, //
    0x07, 0x25, 0x01, 0x00, 0x00, 0x00, 0x00,
];
/// Offset of the Type I format descriptor's bNrChannels in [`FIXTURE`] (the mono mutation).
const FIXTURE_NCH: usize = 87;

struct Score {
    fails: u32,
    first: (&'static str, u64, u64),
}

impl Score {
    fn check(&mut self, what: &'static str, got: u64, want: u64) {
        if got != want {
            if self.fails == 0 {
                self.first = (what, got, want);
            }
            self.fails += 1;
        }
    }
}

/// `tests uac1`: the descriptor walk on the fixture, then 440 Hz for 1 s through the audio seam to the USB sink.
pub fn fixture() {
    let mut sc = Score { fails: 0, first: ("", 0, 0) };
    let p = parse(&FIXTURE);
    sc.check("cfg.wTotalLength", le16(&FIXTURE, 2) as u64, FIXTURE.len() as u64);
    sc.check("ran_short", p.ran_short as u64, 0);
    sc.check("window.audio", cfg_has_audio(&FIXTURE[..64]) as u64, 1);
    sc.check("window.hid", cfg_has_hid(&FIXTURE[..64]) as u64, 0);
    sc.check("ac", p.have_ac as u64, 1);
    sc.check("ac_intf", p.ac_intf as u64, 0);
    sc.check("bcdADC", p.bcd_adc as u64, 0x0100);
    sc.check("as_intfs", p.as_seen as u64, 2);
    sc.check("alts", p.nalt as u64, 1);
    match pick(&p) {
        Some(a) => {
            sc.check("pick.intf", a.intf as u64, 1);
            sc.check("pick.alt", a.alt as u64, 1);
            sc.check("pick.ep", a.ep.addr as u64, 0x01);
            sc.check("pick.mps", a.ep.mps as u64, 192);
            sc.check("pick.interval", a.ep.interval as u64, 1);
            sc.check("pick.freq_ctl", a.freq_ctl as u64, 0);
        }
        None => sc.check("pick", 0, 1),
    }
    sc.check("td_bytes(fs-frame)", td_bytes(8) as u64, 192);
    sc.check("td_bytes(hs-uframe)", td_bytes(1) as u64, 24);
    let mut mono = FIXTURE;
    mono[FIXTURE_NCH] = 1;
    sc.check("mono.refused", pick(&parse(&mono)).is_none() as u64, 1);
    let (iso_tds, iso_missed, iso_underrun, iso_events) = crate::drivers::xhci::isoch::out_stats();
    if sc.fails != 0 {
        let (w, g, x) = sc.first;
        serial_println!(":: UAC1: enumerated={} stream=parse frames=0 underruns=0 -> FAIL :: fixture misses={} first={} read={} want={}", ENUMERATED.load(Ordering::Relaxed) as u8, sc.fails, w, g, x);
        return;
    }
    if !ENUMERATED.load(Ordering::Acquire) {
        serial_println!(":: UAC1: enumerated=0 stream=none frames=0 underruns=0 -> SKIP :: reason=no-usb-audio-device fixture=ok (UNAOS_UAC=1 attaches QEMU's usb-audio) ::");
        return;
    }
    if !STREAMING.load(Ordering::Acquire) {
        let why = *WHY.lock();
        serial_println!(":: UAC1: enumerated=1 stream={} frames=0 underruns=0 -> FAIL :: fixture=ok ::", why);
        return;
    }
    match crate::audio::tone_on(crate::audio::Sink::Usb, 440, 1000) {
        Ok((frames, und)) => {
            wire();
            let (t1, m1, u1, e1) = crate::drivers::xhci::isoch::out_stats();
            let pass = frames >= RATE as u64 && und == 0;
            serial_println!(
                ":: UAC1: enumerated=1 stream=ok frames={} underruns={} -> {} :: hz=440 ms=1000 want_frames>={} iso_tds={} iso_missed={} iso_ring_underrun={} iso_events={} ::",
                frames, und, if pass { "PASS" } else { "FAIL" }, RATE, t1.wrapping_sub(iso_tds), m1.wrapping_sub(iso_missed), u1.wrapping_sub(iso_underrun), e1.wrapping_sub(iso_events)
            );
        }
        Err(e) => serial_println!(":: UAC1: enumerated=1 stream=ok frames=0 underruns=0 -> FAIL :: tone={} ::", e),
    }
}

/// The sink's display name, `USB <vid:pid>` (no string descriptor is read on this arc).
pub fn name() -> Option<String> {
    device().map(|(v, p, _)| alloc::format!("USB {:04x}:{:04x}", v, p))
}

// ── `tests uacmic` (UACMIC, rmbp-ledger B723) ───────────────────────────────────────────────────────────────

/// A capture-only configuration (79 bytes): AC interface 0 (header bcdADC 1.00), AS interface 1 alt 0 (no endpoint)
/// and alt 1 (PCM, Type I 1 x 16-bit, 48 kHz; IN 0x82 isochronous asynchronous, 96 bytes, every frame; the
/// class-specific endpoint declares the sampling-frequency control).
const MIC_FIXTURE: [u8; 79] = [
    0x09, 0x02, 0x4f, 0x00, 0x02, 0x01, 0x00, 0x80, 0x32, //
    0x09, 0x04, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, //
    0x09, 0x24, 0x01, 0x00, 0x01, 0x09, 0x00, 0x01, 0x01, //
    0x09, 0x04, 0x01, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, //
    0x09, 0x04, 0x01, 0x01, 0x01, 0x01, 0x02, 0x00, 0x00, //
    0x07, 0x24, 0x01, 0x01, 0x01, 0x01, 0x00, //
    0x0b, 0x24, 0x02, 0x01, 0x01, 0x02, 0x10, 0x01, 0x80, 0xbb, 0x00, //
    0x09, 0x05, 0x82, 0x05, 0x60, 0x00, 0x01, 0x00, 0x00, //
    0x07, 0x25, 0x01, 0x01, 0x00, 0x00, 0x00,
];
/// Offsets in [`MIC_FIXTURE`]'s Type I format descriptor: bNrChannels, bSubframeSize, bBitResolution, tSamFreq.
const MIC_NCH: usize = 56;
const MIC_SUB: usize = 57;
const MIC_BITS: usize = 58;
const MIC_FREQ: usize = 60;
/// The ring-3-shaped app the grant leg asks for (never posted: the fixture asks quietly).
const MIC_PROBE_APP: &str = "uacmic-probe";

fn mic_wait(ms: u64, f: &dyn Fn() -> bool) -> bool {
    let t0 = crate::arch::ms();
    while !f() {
        if crate::arch::ms().wrapping_sub(t0) >= ms {
            return false;
        }
        crate::audio::nap(5);
    }
    true
}

/// The mic dot's painted edges `(on, off)` and its painted state, where a bar exists.
fn mic_dot() -> Option<(u32, u32, bool)> {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        let (on, off) = crate::video::menubar::micdot::counts();
        return Some((on, off, crate::video::menubar::micdot::painted()));
    }
    #[allow(unreachable_code)]
    None
}

/// `tests uacmic`: the capture walk on its fixture (pick, refusals, the 44.1 kHz fallback), the grant (a non-kernel
/// app with no kept answer is asked, never streamed), then — with a capture device — the kernel recorder opens the
/// ONE source, reads 1 s, and closes it; the mic dot must go on and off with the driver's count.
/// `:: UACMIC: enumerated=<0|1> capture=<ok|none|why> frames=<n> dot_on=<0|1> dot_off=<0|1> grant=<asked|kept> -> PASS|SKIP|FAIL ::`
pub fn mic_fixture() {
    let mut sc = Score { fails: 0, first: ("", 0, 0) };
    let p = parse(&MIC_FIXTURE);
    sc.check("cfg.wTotalLength", le16(&MIC_FIXTURE, 2) as u64, MIC_FIXTURE.len() as u64);
    sc.check("ran_short", p.ran_short as u64, 0);
    sc.check("window.audio", cfg_has_audio(&MIC_FIXTURE[..64]) as u64, 1);
    sc.check("alts", p.nalt as u64, 1);
    sc.check("play.none", pick(&p).is_none() as u64, 1);
    match pick_capture(&p) {
        Some((a, hz)) => {
            sc.check("cap.intf", a.intf as u64, 1);
            sc.check("cap.alt", a.alt as u64, 1);
            sc.check("cap.ep", a.ep.addr as u64, 0x82);
            sc.check("cap.mps", a.ep.mps as u64, 96);
            sc.check("cap.nch", a.nch as u64, 1);
            sc.check("cap.rate", hz as u64, 48_000);
            sc.check("cap.freq_ctl", a.freq_ctl as u64, 1);
        }
        None => sc.check("cap.pick", 0, 1),
    }
    let mut v = MIC_FIXTURE;
    v[MIC_NCH] = 2;
    sc.check("stereo.taken", pick_capture(&parse(&v)).map(|(a, _)| a.nch as u64).unwrap_or(0), 2);
    let mut v = MIC_FIXTURE;
    v[MIC_SUB] = 3;
    v[MIC_BITS] = 24;
    sc.check("pcm24.refused", pick_capture(&parse(&v)).is_none() as u64, 1);
    let mut v = MIC_FIXTURE;
    v[MIC_FREQ..MIC_FREQ + 3].copy_from_slice(&[0x44, 0xac, 0x00]);
    sc.check("rate.44k1", pick_capture(&parse(&v)).map(|(_, hz)| hz as u64).unwrap_or(0), 44_100);
    let mut v = MIC_FIXTURE;
    v[MIC_FREQ..MIC_FREQ + 3].copy_from_slice(&[0x00, 0x7d, 0x00]);
    sc.check("rate.32k.refused", pick_capture(&parse(&v)).is_none() as u64, 1);
    // The grant: a non-kernel app with no kept answer is ASKED (quietly here) and gets no source; the count holds.
    let want0 = CAP_WANT.load(Ordering::Acquire);
    let grant = match crate::audio::open(MIC_PROBE_APP, false) {
        Err("asked") => "asked",
        Ok(s) => {
            drop(s);
            "kept"
        }
        Err(_) => crate::audio::mic_grant(MIC_PROBE_APP, false), // kept, but no device to open
    };
    sc.check("grant.count", CAP_WANT.load(Ordering::Acquire) as u64, want0 as u64);
    let en = ENUMERATED.load(Ordering::Acquire) as u8;
    if sc.fails != 0 {
        let (w, g, x) = sc.first;
        serial_println!(":: UACMIC: enumerated={} capture=parse frames=0 dot_on=0 dot_off=0 grant={} -> FAIL :: fixture misses={} first={} read={} want={} ::", en, grant, sc.fails, w, g, x);
        return;
    }
    if en == 0 {
        serial_println!(":: UACMIC: enumerated=0 capture=none frames=0 dot_on=0 dot_off=0 grant={} -> SKIP :: reason=no-usb-audio-device fixture=ok (UNAOS_UAC=1 attaches QEMU's usb-audio) ::", grant);
        return;
    }
    let Some(cap) = *CAP.lock() else {
        serial_println!(":: UACMIC: enumerated=1 capture=none frames=0 dot_on=0 dot_off=0 grant={} -> SKIP :: reason=no-capture-alternate fixture=ok ::", grant);
        return;
    };
    let dot0 = mic_dot();
    let (_, f0, o0) = capture_counters();
    let ain0 = crate::drivers::xhci::isoch::ain_stats();
    let mut src = match crate::audio::source(crate::audio::KERNEL_RECORDER) {
        Ok(s) => s,
        Err(e) => {
            serial_println!(":: UACMIC: enumerated=1 capture={} frames=0 dot_on=0 dot_off=0 grant={} -> FAIL :: stage=open ::", e, grant);
            return;
        }
    };
    let armed = mic_wait(1_500, &|| CAP_ARMED.load(Ordering::Acquire) || CAP_FAIL.load(Ordering::Acquire)) && CAP_ARMED.load(Ordering::Acquire);
    let lit = mic_dot().map(|_| mic_wait(2_000, &|| mic_dot().map(|d| d.2).unwrap_or(false)));
    // 1 s of frames, the level read as it arrives (peak |sample|).
    let (ch, rate) = src.format();
    let fb = ch.max(1) as usize * 2;
    let want = rate as usize * fb;
    let mut chunk = [0u8; 960];
    let (mut got, mut peak) = (0usize, 0i32);
    let t0 = crate::arch::ms();
    while armed && got < want && crate::arch::ms().wrapping_sub(t0) < 3_000 {
        let n = src.read(&mut chunk);
        if n == 0 {
            crate::audio::nap(2);
            continue;
        }
        for s in chunk[..n].chunks_exact(2) {
            peak = peak.max((i16::from_le_bytes([s[0], s[1]]) as i32).abs());
        }
        got += n;
    }
    drop(src);
    let closed = mic_wait(1_500, &|| !CAP_ARMED.load(Ordering::Acquire));
    let dark = mic_dot().map(|_| mic_wait(2_000, &|| !mic_dot().map(|d| d.2).unwrap_or(true)));
    let (_, f1, o1) = capture_counters();
    let ain1 = crate::drivers::xhci::isoch::ain_stats();
    let frames = (got / fb) as u64;
    let why = *CAP_WHY.lock();
    let (dot_on, dot_off) = match (dot0, mic_dot()) {
        (Some(a), Some(b)) => (((b.0 > a.0) && lit == Some(true)) as u8, ((b.1 > a.1) && dark == Some(true)) as u8),
        _ => (0, 0),
    };
    let captured = armed && frames >= rate as u64;
    let nonsilent = peak > 0;
    let bar = dot0.is_some();
    let verdict = if !captured || !nonsilent || !closed || (bar && (dot_on == 0 || dot_off == 0)) {
        "FAIL"
    } else if !bar {
        "SKIP(no-bar)"
    } else {
        "PASS"
    };
    serial_println!(
        ":: UACMIC: enumerated=1 capture={} frames={} dot_on={} dot_off={} grant={} -> {} :: level={} nonsilent={} fmt=pcm{}/{}/{} ep={:#04x} closed={} ring_frames={} overruns={} iso_tds={} iso_missed={} iso_overrun={} iso_events={} fixture=ok ::",
        if captured { "ok" } else if armed { "short" } else { why },
        frames, dot_on, dot_off, grant, verdict, peak, nonsilent as u8, BITS, ch, rate, cap.alt.ep.addr, closed as u8,
        f1.wrapping_sub(f0), o1.wrapping_sub(o0),
        ain1.0.wrapping_sub(ain0.0), ain1.1.wrapping_sub(ain0.1), ain1.2.wrapping_sub(ain0.2), ain1.3.wrapping_sub(ain0.3)
    );
}
