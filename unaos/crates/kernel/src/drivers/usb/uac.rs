//! CHARTER: Stria — driver
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
#![cfg(feature = "uac")] // UACMIC (B723) M1: the class is the `uac` feature's alone — drivers/mod.rs declares `drivers::usb` under `uac` too, so this is the second guard and a `uvc`-only image compiles none of this file

//! UAC1 (rmbp-ledger B706) — USB Audio Class 1 PLAYBACK: a USB speaker, headset or DAC plays what the kernel writes.
//! UACMIC (rmbp-ledger B723) — and CAPTURE: a USB microphone or a headset's mic is read through `audio::source`.
//!
//! The class is the `uac` feature (UACMIC M1; `uac` implies `uvc`, whose isochronous engine it rides: XHCIISO,
//! `drivers/xhci/isoch.rs`, reached only through that file's `ac` door). Declared from `drivers/mod.rs` on the existing
//! `uvc` line under its own `#[cfg(feature = "uac")]` (`pub mod usb { pub mod uac; }`, word moved at the UACMIC fold
//! 2e8a3cd1) and erased again by the inner cfg above, so a `uvc`-without-`uac` image compiles none of it. Host-neutral like `drivers/uvc.rs`: every transfer goes through
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
//!   context that may run EP0 and Configure Endpoint) sees [`ep0_due`] and runs [`ep0_service`]: Configure
//!   Endpoint (the audio IN pipe, `APIPE`), SET_INTERFACE(alt), the sampling-frequency SET_CUR when declared,
//!   `iso_start`. The last source closing runs it the other way: SET_INTERFACE(0), drop the endpoint. [`capture_armed`]
//!   — the IN pipe runs — is the ONE count the menu bar's mic dot paints from (`video::menubar::micdot`). Completed
//!   TDs land in a 64 KiB single-producer single-consumer ring ([`push_capture`] from the claim path, lock-free;
//!   [`read_capture`] from the one reader); a push that does not fit counts an overrun and drops the newest bytes.
//!
//! * **Volume** (SINKALL, rmbp-ledger B767) — [`parse`] records the AC interface's first Feature Unit (its master and
//!   channel 1-2 Mute / Volume controls); while the playback stream runs, the mixer hands the person's level to
//!   [`volume_want`] and the service pass applies it ([`volume_service`]: GET_MIN / GET_MAX once, then SET_CUR Mute and
//!   Volume on EP0), so the mixer plays unity into a device that carries its own volume. It rides the class driver's
//!   one EP0 door from `isoch::service_uvc` ([`ep0_due`] / [`ep0_service`]: one EP0 job per pass).
//!
//! # SOURCES (DRIVERS-METHOD §5)
//!
//! USB Device Class Definition for Audio Devices 1.0: §4.3.2 (class-specific AC interface header, bcdADC), §4.5.2
//! (AS_GENERAL: bTerminalLink, wFormatTag), §4.6.1.2 (class-specific AS isochronous endpoint: bmAttributes bit 0 =
//! Sampling Frequency control), §5.2.3.2.3.1 (Sampling Frequency Control: SET_CUR, bmRequestType 0x22, wValue
//! 0x0100, wIndex = endpoint, 3-byte rate), A.5/A.6 (descriptor subtypes). Audio Data Formats 1.0: §2.2.5 (Type I
//! format descriptor: bNrChannels, bSubframeSize, bBitResolution, bSamFreqType, tSamFreq), A.1.1 (wFormatTag 1 = PCM).
//! SINKALL: UAC1 §4.3.2.5 (Feature Unit descriptor: bUnitID[3] bSourceID[4] bControlSize[5] bmaControls(0..)[6..]; bit 0
//! Mute, bit 1 Volume), §5.2.2.4.3.1-2 (Mute / Volume Control: SET_CUR 0x21 / GET_MIN 0xA1 0x82 / GET_MAX 0x83, wValue =
//! CS << 8 | channel, wIndex = UnitID << 8 | interface; Volume in 1/256 dB, signed, 0x8000 = -inf), A.10.2 (CS 0x01
//! MUTE_CONTROL, 0x02 VOLUME_CONTROL).
//! USB 2.0 Table 9-12/9-13 (interface, endpoint; the audio endpoint is 9 bytes with bRefresh/bSynchAddress). QEMU's
//! `hw/usb/dev-audio.c` (recalled, not re-read on this arc) is the fixture's shape: full-speed, AC interface 0, AS
//! interface 1 alt 1, OUT 0x01 SYNCHRONOUS (bmAttributes 0x0d, sync type 11 — re-read by UACFEEDBACK B789 at
//! dev-audio.c:227; an earlier line here said adaptive), 192-byte packets at 48 kHz stereo, no sampling-frequency
//! control, no feedback endpoint (bNumEndpoints 1), and every OUT packet that is not exactly 192 bytes is dropped
//! (`streambuf_put`).
//!
//! * **Feedback** (UACFEEDBACK, rmbp-ledger B789; USB 2.0 §5.12.4.2, UAC1 §3.7.2.2 / §4.6.2) — an asynchronous sink's
//!   OUT data endpoint names its feedback (synch) endpoint in bSynchAddress; [`probe`] opens it after the OUT pipe as
//!   the engine's feedback pipe ([`feedback_open`]); each completed value is decoded by [`fb_decode`] into ONE atomic
//!   word ([`push_feedback`], lock-free), and each OUT TD's frame count is [`meter`]'s accumulator of it
//!   ([`out_next_len`]) — 48 per ms while no value has arrived, so a synchronous device gets today's packets.

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
const AC_FEATURE_UNIT: u8 = 0x06; // UAC1 A.5 (SINKALL)
const RQ_GET_MIN: u8 = 0x82; // UAC1 A.9
const RQ_GET_MAX: u8 = 0x83;
const MUTE_CONTROL: u16 = 0x01; // UAC1 A.10.2 (feature unit control selectors)
const VOLUME_CONTROL: u16 = 0x02;
/// Volume's -inf (UAC1 §5.2.2.4.3.2): a mute on a unit with no Mute control.
const VOL_SILENT: i16 = -0x8000;
/// The range assumed when GET_MIN / GET_MAX fail: -60 dB .. 0 dB.
const VOL_FALLBACK: (i16, i16) = (-60 * 256, 0);
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
    /// UACFEEDBACK (B789): the data endpoint's sync type (bmAttributes 3:2: 0 none, 1 async, 2 adaptive, 3 sync).
    pub sync: u8,
    /// The data endpoint's bSynchAddress (UAC1 §4.6.1.1, byte 8 of the 9-byte audio endpoint; 0 = none).
    pub synch_addr: u8,
    /// The feedback (synch) endpoint: an isochronous IN with usage 01, or the address bSynchAddress names.
    pub fb: IsoEp,
    pub have_fb: bool,
    /// The feedback endpoint's bRefresh: its value is refreshed every 2^refresh ms.
    pub refresh: u8,
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

/// SINKALL: a Feature Unit (UAC1 §4.3.2.5): its id and which of master (bit 0), channel 1 (bit 1), channel 2 (bit 2)
/// carry Mute and Volume. `unit` 0 = none.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Fu {
    pub unit: u8,
    pub mute: u8,
    pub vol: u8,
}

/// The Feature Unit descriptor `d` (bLength >= 7; bmaControls(0..n) of bControlSize bytes, then iFeature).
fn feature_unit(d: &[u8]) -> Fu {
    let size = (d[5] as usize).max(1);
    let n = (d.len() - 7) / size;
    let mut fu = Fu { unit: d[3], ..Fu::default() };
    for ch in 0..n.min(3) {
        let b = d[6 + ch * size];
        if b & 0x01 != 0 {
            fu.mute |= 1 << ch;
        }
        if b & 0x02 != 0 {
            fu.vol |= 1 << ch;
        }
    }
    fu
}

/// The configuration as this driver reads it.
#[derive(Clone, Copy, Default)]
pub struct Parsed {
    pub have_ac: bool,
    pub ac_intf: u8,
    pub bcd_adc: u16,
    /// SINKALL: the AC interface's first Feature Unit.
    pub fu: Fu,
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
                if in_ac && d[2] == AC_FEATURE_UNIT && len >= 7 && p.fu.unit == 0 {
                    p.fu = feature_unit(d); // SINKALL: the volume the device carries itself
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
                    // UACFEEDBACK (B789): bmAttributes 5:4 = usage (01 feedback), 3:2 = sync type; a 9-byte audio
                    // endpoint carries bRefresh[7] bSynchAddress[8]. The first non-feedback isochronous endpoint is the
                    // data endpoint (as before); a later IN with usage 01, or at the address bSynchAddress names, is
                    // its feedback endpoint (UAC1-era synch endpoints carry bmAttributes 0x01: no usage bits).
                    let usage = (d[3] >> 4) & 0x03;
                    if (d[3] & 0x03) == 0x01 {
                        let w = le16(d, 4);
                        let ep = IsoEp { intf: a.intf, alt: a.alt, addr: d[2], mps: w & 0x07FF, hs_extra: ((w >> 11) & 0x03) as u8, interval: d[6], ..IsoEp::default() };
                        if !a.have_ep && usage != 1 {
                            a.ep = ep;
                            a.have_ep = true;
                            a.sync = (d[3] >> 2) & 0x03;
                            a.synch_addr = if len >= 9 { d[8] } else { 0 };
                        } else if !a.have_fb && d[2] & 0x80 != 0 && (usage == 1 || (a.synch_addr != 0 && d[2] == a.synch_addr)) {
                            a.fb = ep;
                            a.have_fb = true;
                            a.refresh = if len >= 9 { d[7] } else { 0 };
                        }
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
    feedback_reset(); // UACFEEDBACK (B789): the first OUT fill is nominal; no value from an earlier device carries over
    TAIL.store(HEAD.load(Ordering::Acquire), Ordering::Release); match h.iso_start(&a.ep) { // CRAWLC: an aborted tone's bytes left in the ring are dropped before the first TD fill (no consumer runs yet)
        Ok((tds, lead, sched)) => {
            STREAMING.store(true, Ordering::Release);
            *WHY.lock() = "ok";
            volume_arm(p.ac_intf, p.fu);
            feedback_open(h, slot, &a); // UACFEEDBACK (B789): an asynchronous sink's feedback endpoint, after the OUT pipe
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
    FU.store(0, Ordering::Release); // SINKALL: the feature unit goes with the device
    feedback_reset(); // UACFEEDBACK (B789): the feedback value goes too (its pipe went in `isoch::disconnect`)
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

/// THE EP0 DOOR (SINKALLDOCS, was `capture_due`): the slot with an EP0 job on this service pass — the playback volume
/// waiting for SET_CUR, or the capture pipe to open or close (the wanted state differs from the armed one).
pub fn ep0_due() -> Option<u8> {
    if volume_due() {
        return Some(SLOT.load(Ordering::Acquire)).filter(|&s| s != 0); // SINKALL: the playback volume's EP0 job first
    }
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

/// THE EP0 DOOR (SINKALLDOCS, was `capture_service`), the service pass's half (task context, EP0 free): the playback
/// volume first; else open the capture pipe while a source wants it, close it when the
/// last source left. One direction per call.
pub fn ep0_service(h: &mut dyn IsoHost) {
    if volume_due() {
        return volume_service(h); // SINKALL: one EP0 job per pass — the volume, then the capture pipe on the next
    }
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
/// (no endpoint) and alt 1 (PCM, Type I 2 x 16-bit, 48 kHz; OUT 0x01 isochronous synchronous (0x0d), 192 bytes, every frame).
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
    feedback_kats(&mut sc); // UACFEEDBACK (B789): the feedback endpoint's walk, its decode and the meter
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
                ":: UAC1: enumerated=1 stream=ok frames={} underruns={} -> {} :: hz=440 ms=1000 want_frames>={} iso_tds={} iso_missed={} iso_ring_underrun={} iso_events={} sync={} feedback={} metered={} ::",
                frames, und, if pass { "PASS" } else { "FAIL" }, RATE, t1.wrapping_sub(iso_tds), m1.wrapping_sub(iso_missed), u1.wrapping_sub(iso_underrun), e1.wrapping_sub(iso_events),
                sync_name(SYNC.load(Ordering::Relaxed)), fb_text(FB_Q16.load(Ordering::Acquire)), METERED.load(Ordering::Relaxed)
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

// ── SINKALL (rmbp-ledger B767): the playback device's Feature Unit — volume and mute on the device ─────────────────────
// The mixer (`audio::mixer::master`) calls [`volume_want`] with the F-row level 0..16 and the mute flag whenever it
// mixes for the USB sink; a device whose Feature Unit has a Volume control takes it (the mixer then plays unity — the
// level is applied once, as the HDA codec amp does). The SET_CUR runs in the service pass ([`volume_service`], reached
// through [`ep0_due`] / [`ep0_service`], the class driver's one EP0 door). One audio device at a time (`probe`'s
// guard), so the door's slot is the playback device's. A refused request leaves the device to the mixer's scale.

/// The playback device's Feature Unit: unit | ac_intf << 8 | mute << 16 | vol << 24 (0 = none).
static FU: AtomicU32 = AtomicU32::new(0);
/// The level wanted: 1 << 16 | mute << 8 | level (0 = none yet); the one last applied; the device's range (min | max << 16
/// as u16 pairs; 0 = unread).
static VOL_WANT: AtomicU32 = AtomicU32::new(0);
static VOL_DONE: AtomicU32 = AtomicU32::new(0);
static VOL_RANGE: AtomicU32 = AtomicU32::new(0);
static VOL_FAIL: AtomicBool = AtomicBool::new(false);
static VOL_SETS: AtomicU32 = AtomicU32::new(0);

/// The stream started: record its device's Feature Unit (if any) and forget the last device's volume state.
fn volume_arm(ac_intf: u8, fu: Fu) {
    let w = if fu.unit == 0 || (fu.mute | fu.vol) == 0 { 0 } else { fu.unit as u32 | (ac_intf as u32) << 8 | (fu.mute as u32) << 16 | (fu.vol as u32) << 24 };
    VOL_DONE.store(0, Ordering::Release);
    VOL_RANGE.store(0, Ordering::Release);
    VOL_FAIL.store(false, Ordering::Release);
    FU.store(w, Ordering::Release);
    serial_println!("[uac] feature unit={} intf={} mute={:#04x} vol={:#04x} -> {}", fu.unit, ac_intf, fu.mute, fu.vol, if fu.vol != 0 { "device-volume" } else { "mixer-volume" });
}

/// The mixer's level for the device: true when the device's Feature Unit carries volume (the mixer plays unity), false
/// when the mixer must scale (no unit, no Volume control, no stream, or the device refused a SET_CUR).
pub fn volume_want(level: u8, muted: bool) -> bool {
    let fu = FU.load(Ordering::Acquire);
    if fu >> 24 == 0 || !STREAMING.load(Ordering::Acquire) || VOL_FAIL.load(Ordering::Acquire) {
        return false;
    }
    VOL_WANT.store(1 << 16 | (muted as u32) << 8 | level.min(16) as u32, Ordering::Release);
    true
}

/// A level is wanted that the device does not hold yet.
fn volume_due() -> bool {
    let w = VOL_WANT.load(Ordering::Acquire);
    w != 0 && w != VOL_DONE.load(Ordering::Acquire) && FU.load(Ordering::Acquire) != 0 && STREAMING.load(Ordering::Acquire) && !VOL_FAIL.load(Ordering::Acquire)
}

/// Level 0..16 to the unit's Volume, linear in dB between its min and max (as `hda::vol::apply` scales its steps).
fn level_db(level: u8, min: i16, max: i16) -> i16 {
    (min as i32 + (max as i32 - min as i32) * level.min(16) as i32 / 16) as i16
}

/// One class-specific request to the Feature Unit (`idx` = UnitID << 8 | interface) with `data` (SET) or into `data` (GET).
fn fu_request(h: &mut dyn IsoHost, get: Option<u8>, cs: u16, ch: u16, idx: u16, data: &mut [u8]) -> bool {
    let n = data.len();
    let r = match get {
        Some(rq) => h.control(0xA1, rq, cs << 8 | ch, idx, n as u16, true),
        None => {
            // SAFETY: the EP0 buffer, `cap` >= 2 bytes; no transfer in flight.
            unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), h.buf(), n) };
            h.control(0x21, RQ_SET_CUR, cs << 8 | ch, idx, n as u16, false)
        }
    };
    if get.is_some() && matches!(r, Ok(got) if got as usize >= n) {
        // SAFETY: the EP0 buffer, just written by the device (`n` <= its length).
        unsafe { core::ptr::copy_nonoverlapping(h.buf() as *const u8, data.as_mut_ptr(), n) };
    }
    r.is_ok() && (get.is_none() || matches!(r, Ok(got) if got as usize >= n))
}

/// The unit's Volume range on channel `ch`, read once per stream (GET_MIN / GET_MAX), else the fallback.
fn vol_range(h: &mut dyn IsoHost, idx: u16, ch: u16) -> (i16, i16) {
    let r = VOL_RANGE.load(Ordering::Acquire);
    if r != 0 {
        return (r as u16 as i16, (r >> 16) as u16 as i16);
    }
    let (mut lo, mut hi) = ([0u8; 2], [0u8; 2]);
    let got = fu_request(h, Some(RQ_GET_MIN), VOLUME_CONTROL, ch, idx, &mut lo) && fu_request(h, Some(RQ_GET_MAX), VOLUME_CONTROL, ch, idx, &mut hi);
    let (min, max) = if got && i16::from_le_bytes(lo) < i16::from_le_bytes(hi) { (i16::from_le_bytes(lo), i16::from_le_bytes(hi)) } else { VOL_FALLBACK };
    VOL_RANGE.store(min as u16 as u32 | (max as u16 as u32) << 16, Ordering::Release); // never 0: min < max
    (min, max)
}

/// The service pass's volume job: Mute on every channel that has it, Volume on every channel that has it.
pub fn volume_service(h: &mut dyn IsoHost) {
    let (want, fu) = (VOL_WANT.load(Ordering::Acquire), FU.load(Ordering::Acquire));
    let (unit, intf, mute, vol) = (fu & 0xFF, (fu >> 8) & 0xFF, (fu >> 16) & 0xFF, (fu >> 24) & 0xFF);
    let (level, muted) = ((want & 0xFF) as u8, want & 0x100 != 0);
    let idx = (unit << 8 | intf) as u16;
    let first = (0..3u16).find(|c| vol & 1 << c != 0).unwrap_or(0);
    let (min, max) = vol_range(h, idx, first);
    let mut ok = true;
    for ch in (0..3u16).filter(|c| mute & 1 << c != 0) {
        ok &= fu_request(h, None, MUTE_CONTROL, ch, idx, &mut [muted as u8]);
    }
    let v = if muted && mute == 0 { VOL_SILENT } else { level_db(level, min, max) };
    for ch in (0..3u16).filter(|c| vol & 1 << c != 0) {
        ok &= fu_request(h, None, VOLUME_CONTROL, ch, idx, &mut v.to_le_bytes());
    }
    VOL_DONE.store(want, Ordering::Release);
    if ok {
        VOL_SETS.fetch_add(1, Ordering::AcqRel);
    } else {
        VOL_FAIL.store(true, Ordering::Release); // the mixer scales from now on (`volume_want` answers false)
    }
    serial_println!(
        "[uac] set-cur feature unit={} level={} mute={} vol={:#06x} min={:#06x} max={:#06x} chans_mute={:#04x} chans_vol={:#04x} result={}",
        unit, level, muted as u8, v as u16, min as u16, max as u16, mute, vol, if ok { "ok" } else { "refused (the mixer scales)" }
    );
}

/// `(SET_CUR rounds the device took, a request refused)` since the stream started — `tests sinkall`'s `usb_volume=`.
pub fn volume_state() -> (u32, bool) {
    (VOL_SETS.load(Ordering::Acquire), VOL_FAIL.load(Ordering::Acquire))
}

/// The device's Feature Unit carries the volume now.
pub fn volume_on_device() -> bool {
    FU.load(Ordering::Acquire) >> 24 != 0 && STREAMING.load(Ordering::Acquire) && !VOL_FAIL.load(Ordering::Acquire)
}

/// QEMU `usb-audio`'s Feature Unit as [`parse`] reads it from [`FIXTURE`]: unit 2, Mute on the master, Volume on
/// channels 1 and 2 — and the level map's ends and middle.
pub fn fu_fixture() -> bool {
    let p = parse(&FIXTURE);
    let ok = p.fu == Fu { unit: 2, mute: 0b001, vol: 0b110 } && level_db(0, -0x7f00, 0) == -0x7f00 && level_db(16, -0x7f00, 0) == 0 && level_db(8, -0x7f00, 0) == -0x3f80;
    serial_println!("[uac] fixture feature unit={} mute={:#04x} vol={:#04x} level_db(8)={:#06x} -> {}", p.fu.unit, p.fu.mute, p.fu.vol, level_db(8, -0x7f00, 0) as u16, if ok { "ok" } else { "FAIL" });
    ok
}

// ── UACFEEDBACK (rmbp-ledger B789): the asynchronous sink's feedback endpoint ───────────────────────────────────────────
// USB 2.0 §5.12.4.2: Ff = samples per (micro)frame — full-speed 10.10 left-justified in 3 bytes (10.14), high-speed 16.16
// in 4 bytes per microframe. UAC1 §3.7.2.2 / §4.6.2: an asynchronous OUT endpoint names its synch endpoint in
// bSynchAddress; the host meters the next OUT packets to the value so the device's FIFO neither drains nor overflows.
// The value is ONE atomic word: written lock-free by [`push_feedback`] from the engine's feedback claim (under the
// feedback pipe's own lock), read by [`out_next_len`] from the OUT TD fill (under the OUT pipe's own lock).

/// The latest feedback value, Q16 frames per ms (0 = none: the stream plays the nominal 48).
static FB_Q16: AtomicU32 = AtomicU32::new(0);
/// The fractional frames carried between OUT packets (one context: the OUT pipe's fill).
static METER_ACC: AtomicU64 = AtomicU64::new(0);
/// The feedback endpoint's address while the stream holds one (0 = none) — the `ac` door's routing word.
static FB_EP: AtomicU8 = AtomicU8::new(0);
/// The stream's sync type (data endpoint bmAttributes 3:2).
static SYNC: AtomicU8 = AtomicU8::new(0);
/// OUT packets sized from a feedback value; values taken; values refused.
static METERED: AtomicU32 = AtomicU32::new(0);
static FB_READS: AtomicU32 = AtomicU32::new(0);
static FB_BAD: AtomicU32 = AtomicU32::new(0);
/// The window a value must lie in: nominal 48 frames per ms +/- 1/8 (42..54), Q16.
const FB_LO: u32 = 42 << 16;
const FB_HI: u32 = 54 << 16;

/// The alternate's feedback endpoint: an OUT data endpoint of sync type 01 (asynchronous) that has one.
pub fn feedback_of(a: &Alt) -> Option<IsoEp> {
    (a.have_ep && a.ep.addr & 0x80 == 0 && a.sync == 1 && a.have_fb).then_some(a.fb)
}

/// One feedback value to Q16 frames per ms: 3 bytes = 10.14 frames per ms (full-speed), 4 bytes = 16.16 frames per
/// microframe (high-speed, x 8). Any other length, or a value outside 42..54 frames per ms, is refused.
pub fn fb_decode(raw: &[u8]) -> Option<u32> {
    let q = match raw.len() {
        3 => le24(raw, 0) << 2,
        4 => u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]).checked_mul(8)?,
        _ => return None,
    };
    (FB_LO..=FB_HI).contains(&q).then_some(q)
}

/// THE METER — frames for the next packet of `interval_uf` microframes at `q16_ms` frames per ms: the integral part of
/// the accumulator (`acc` = the carried fraction, Q19 = Q16 frames x 8 microframes), the remainder carried.
pub fn meter(acc: u64, q16_ms: u32, interval_uf: u32) -> (u32, u64) {
    let t = acc + q16_ms as u64 * interval_uf as u64;
    ((t >> 19) as u32, t & ((1 << 19) - 1))
}

/// THE FEEDBACK PRODUCER — one completed feedback TD's bytes (the engine's claim path; lock-free). A zero-length
/// completion (the device had no new value) is ignored; a value [`fb_decode`] refuses is counted and dropped.
pub fn push_feedback(src: &[u8]) {
    if src.is_empty() {
        return;
    }
    match fb_decode(src) {
        Some(q) => {
            FB_Q16.store(q, Ordering::Release);
            FB_READS.fetch_add(1, Ordering::Relaxed);
        }
        None => {
            FB_BAD.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The `ac` door's routing question: `addr` is the streaming device's feedback endpoint (the engine's feedback pipe).
pub fn feedback_ep(addr: u8) -> bool {
    addr != 0 && FB_EP.load(Ordering::Acquire) == addr
}

/// The OUT TD's bytes (the engine's `Pipe::produce`, under the OUT pipe's lock): `nominal` while no feedback value has
/// arrived (a synchronous or adaptive device: today's packet), else [`meter`]'s frames x 4, within the endpoint's ESIT.
pub fn out_next_len(nominal: u32, interval_uf: u32, esit: u32) -> u32 {
    let q = FB_Q16.load(Ordering::Acquire);
    if q == 0 {
        return nominal;
    }
    let (frames, acc) = meter(METER_ACC.load(Ordering::Relaxed), q, interval_uf);
    METER_ACC.store(acc, Ordering::Relaxed);
    METERED.fetch_add(1, Ordering::Relaxed);
    (frames * FRAME_BYTES).min(esit / FRAME_BYTES * FRAME_BYTES)
}

/// A stream starts or its device leaves: no value, no carried fraction, no feedback endpoint.
fn feedback_reset() {
    FB_EP.store(0, Ordering::Release);
    FB_Q16.store(0, Ordering::Release);
    METER_ACC.store(0, Ordering::Relaxed);
    METERED.store(0, Ordering::Relaxed);
}

/// After the OUT pipe started: open the alternate's feedback endpoint on the host's feedback pipe. A refusal leaves the
/// stream unmetered (48 frames per ms), never failed.
fn feedback_open(h: &mut dyn IsoHost, slot: u8, a: &Alt) {
    SYNC.store(a.sync, Ordering::Relaxed);
    let Some(fb) = feedback_of(a) else {
        serial_println!("[uac] sync={} feedback=none -> unmetered (48 frames per ms)", sync_name(a.sync));
        return;
    };
    FB_EP.store(fb.addr, Ordering::Release);
    if let Err((cc, name)) = h.iso_configure(&fb) {
        FB_EP.store(0, Ordering::Release);
        serial_println!("[uac] feedback slot={} ep={:#04x} refused stage=configure-endpoint reason=cc{}-{} -> unmetered", slot, fb.addr, cc, name);
        return;
    }
    match h.iso_start(&fb) {
        Ok((tds, _, _)) => serial_println!(
            "[uac] feedback slot={} ep={:#04x} mps={} interval={} refresh_ms={} tds={} -> metering",
            slot, fb.addr, fb.mps, fb.interval, 1u32 << a.refresh.min(15), tds
        ),
        Err(e) => {
            h.iso_drop(&fb);
            FB_EP.store(0, Ordering::Release);
            serial_println!("[uac] feedback slot={} ep={:#04x} refused stage=iso-start reason={} -> unmetered", slot, fb.addr, e);
        }
    }
}

/// `n.nnnn` frames per ms, or `none`.
fn fb_text(q: u32) -> String {
    if q == 0 {
        return String::from("none");
    }
    alloc::format!("{}.{:04}", q >> 16, ((q & 0xFFFF) as u64 * 10_000 / 65_536))
}

fn sync_name(s: u8) -> &'static str {
    match s & 3 {
        1 => "async",
        2 => "adaptive",
        3 => "sync",
        _ => "none",
    }
}

/// Offsets in [`FIXTURE`]: alt 1's bNumEndpoints, the data endpoint's bmAttributes and bSynchAddress.
const FIXTURE_NEP: usize = 71;
const FIXTURE_EP_ATTR: usize = 97;
const FIXTURE_EP_SYNCH: usize = 102;
/// A UAC1-era synch endpoint: IN 0x81, bmAttributes 0x01 (no usage bits), 3 bytes, every frame, bRefresh 5 (32 ms).
const FB_DESC: [u8; 9] = [0x09, 0x05, 0x81, 0x01, 0x03, 0x00, 0x01, 0x05, 0x00];

/// `tests uac1`'s feedback KATs: QEMU's synchronous endpoint has none; an asynchronous variant of [`FIXTURE`] finds
/// its synch endpoint by bSynchAddress and by usage bits; [`fb_decode`] in both widths; [`meter`] at 47.75 / 48 / 48.5.
fn feedback_kats(sc: &mut Score) {
    match pick(&parse(&FIXTURE)) {
        Some(a) => {
            sc.check("fb.qemu.sync", a.sync as u64, 3);
            sc.check("fb.qemu.have_fb", a.have_fb as u64, 0);
            sc.check("fb.qemu.none", feedback_of(&a).is_none() as u64, 1);
        }
        None => sc.check("fb.qemu.pick", 0, 1),
    }
    let mut v = [0u8; 119];
    v[..110].copy_from_slice(&FIXTURE);
    v[110..].copy_from_slice(&FB_DESC);
    v[2] = 119;
    v[FIXTURE_NEP] = 2;
    v[FIXTURE_EP_ATTR] = 0x05;
    v[FIXTURE_EP_SYNCH] = 0x81;
    match pick(&parse(&v)) {
        Some(a) => {
            sc.check("fb.async.sync", a.sync as u64, 1);
            sc.check("fb.async.synch_addr", a.synch_addr as u64, 0x81);
            sc.check("fb.async.ep", feedback_of(&a).map(|e| e.addr as u64).unwrap_or(0), 0x81);
            sc.check("fb.async.mps", a.fb.mps as u64, 3);
            sc.check("fb.async.refresh", a.refresh as u64, 5);
            sc.check("fb.async.data_ep", a.ep.addr as u64, 0x01);
        }
        None => sc.check("fb.async.pick", 0, 1),
    }
    v[FIXTURE_EP_SYNCH] = 0;
    v[110 + 3] = 0x11; // usage 01: found by the usage bits alone
    sc.check("fb.usage.ep", pick(&parse(&v)).and_then(|a| feedback_of(&a)).map(|e| e.addr as u64).unwrap_or(0), 0x81);
    let d = |r: &[u8]| fb_decode(r).map(|q| q as u64).unwrap_or(0);
    sc.check("fb.decode.fs48", d(&[0x00, 0x00, 0x0C]), 0x30_0000);
    sc.check("fb.decode.fs47_5", d(&[0x00, 0xE0, 0x0B]), 0x2F_8000);
    sc.check("fb.decode.hs48", d(&[0x00, 0x00, 0x06, 0x00]), 0x30_0000);
    sc.check("fb.decode.hs48_5", d(&[0x00, 0x10, 0x06, 0x00]), 0x30_8000);
    sc.check("fb.decode.zero", d(&[0x00, 0x00, 0x00]), 0);
    sc.check("fb.decode.96", d(&[0x00, 0x00, 0x18]), 0);
    sc.check("fb.decode.2byte", d(&[0x00, 0x0C]), 0);
    let run = |q: u32, uf: u32| {
        let (mut acc, mut out) = (0u64, [0u32; 4]);
        for f in out.iter_mut() {
            let (n, a) = meter(acc, q, uf);
            *f = n;
            acc = a;
        }
        out
    };
    let m = run(0x30_8000, 8);
    sc.check("fb.meter.48_5", (m == [48, 49, 48, 49]) as u64, 1);
    let m = run(0x2F_C000, 8);
    sc.check("fb.meter.47_75.first", m[0] as u64, 47);
    sc.check("fb.meter.47_75.sum", m.iter().sum::<u32>() as u64, 191);
    sc.check("fb.meter.hs", run(0x30_0000, 1)[0] as u64, 6);
    sc.check("fb.meter.fs", run(0x30_0000, 8)[0] as u64, 48);
}
