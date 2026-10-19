//! CHARTER: Stria — driver
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! UAC1 (rmbp-ledger B706) — USB Audio Class 1 PLAYBACK: a USB speaker, headset or DAC plays what the kernel writes.
//!
//! Declared from `drivers/mod.rs` as `#[cfg(feature = "uvc")] pub mod usb { pub mod uac; }` on the existing `uvc`
//! line: the class rides the isochronous engine the `uvc` knob arms (XHCIISO, `drivers/xhci/isoch.rs`), so a
//! knob-off build never lexes this file. Host-neutral like `drivers/uvc.rs`: every transfer goes through
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
    if STREAMING.load(Ordering::Acquire) {
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
        );
    }
    if !p.have_ac {
        return fail(slot, "parse", "no-audio-control");
    }
    if p.bcd_adc >= 0x0200 {
        return fail(slot, "parse", "uac2-not-built"); // a UAC2 device (clock entities, a different AS format layout)
    }
    let Some(a) = pick(&p) else { return fail(slot, "pick", "no-pcm16-2ch-48k-out-alternate") }; ENUMERATED.store(true, Ordering::Release); SLOT.store(slot, Ordering::Release); VIDPID.store((vid as u32) << 16 | pid as u32, Ordering::Relaxed); // CRAWLC: the device is ours only once an OUT alternate plays our format
    if h.control(0x00, 9, cfg_value as u16, 0, 0, false).is_err() {
        return fail(slot, "set-configuration", "stalled-or-timeout");
    }
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
    SLOT.store(0, Ordering::Release);
    let was = STREAMING.swap(false, Ordering::AcqRel);
    ENUMERATED.store(false, Ordering::Release);
    *WHY.lock() = "gone";
    if was {
        serial_println!("[uac] disconnect slot={} — stream and sink gone", slot);
        crate::audio::usb_gone();
    }
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
        serial_println!(":: UAC1: enumerated=0 stream=none frames=0 underruns=0 -> SKIP :: reason=no-usb-audio-device fixture=ok (UNAOS_USBAUDIO=1 attaches QEMU's usb-audio) ::");
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
