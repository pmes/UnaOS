//! CHARTER: Comscan — driver (Bluetooth; AVDTP and the A2DP source ride BTHID's link and L2CAP channels, B339; the PCM it frames is the audio seam's, `audio::sink("bt:<addr>")`, Stria's)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A2DP (rmbp-ledger B739) — AVDTP signalling, the A2DP source stream, and the sink the audio seam writes.
//!
//! THE LAYERS. HCI, ACL, the link's security (SSP, encryption) and L2CAP channels are the BTHID host's
//! (`drivers/ehci/bthid.rs`); SDP's client loop too. This module is a child of `bthid` (declared at its tail) so it
//! drives that host's `Bt` state directly — the same lock, the same per-pass `step` — and adds, for a link to an audio
//! device (`a2dp_link`: `bt audio <addr>` named it, or its Class of Device is Audio/Video):
//! * SDP: the AudioSink query instead of the HID one ([`super::sdp`]);
//! * AVDTP signalling on PSM 0x0019 ([`Peer`], pure, driven by `tests a2dp` with no radio). As INITIATOR: DISCOVER
//!   (first free audio SINK SEP), GET_CAPABILITIES (its SBC codec element), SET_CONFIGURATION (SBC, 44.1 kHz, joint
//!   stereo, 16 blocks, 8 subbands, loudness — SNR when loudness is not offered — bitpool [`pick_bitpool`]), OPEN, the
//!   media channel, START. As ACCEPTOR (a headset that reconnects and drives): our one SEP (SEID 1, audio, SOURCE)
//!   answers DISCOVER, GET_(ALL_)CAPABILITIES, SET_CONFIGURATION, OPEN, START, SUSPEND, CLOSE, ABORT and
//!   DELAYREPORT; anything else gets a general reject. Fragmented signalling packets are not reassembled.
//! * THE MEDIA TRANSPORT: the second L2CAP channel on PSM 0x0019 carries RTP (version 2, payload type 96, sequence,
//!   timestamp in samples, SSRC 1) + the one-byte SBC media payload header (frame count) + `fpp` SBC frames from
//!   [`sbc::Encoder`] (the `sbc_core` source, included by path). One packet is at most two ACL packets
//!   ([`super::l2cap::payload_fit`], fragmented by [`super::l2cap::fragment`], A2DPKNOB B750): 500 bytes on the BCM
//!   radio, so bitpool 53 and four frames a packet.
//! * PACING ([`tick`], once per BTHID pass): samples due = elapsed x 44100 / 1000 + a 40 ms lead; at most four packets
//!   a pass and never more than six ACL packets (fragments) queued. The clock starts when PCM arrives and stops when the ring runs dry
//!   with no writer: nothing is sent while nothing plays (BP-audio-3).
//! * THE SINK: [`begin`] / [`write`] (PCM 16-bit LE stereo 44.1 kHz) / [`finish`] / [`queued`] / [`counters`] — the
//!   USB class driver's shape, so `audio::tone_on(Sink::Bt, ..)` feeds it as it feeds UAC. The mixer (AUDIOMIX) is
//!   not on this tip: the sink is written to `audio::sink`'s existing shape.
//!
//! Wire: `[a2dp] dev=<addr> link up -> sdp`, `[a2dp] sdp dev= sink=<yes|no> psm= avdtp= a2dp=`, `[a2dp] dev=
//! signalling channel …`, `[a2dp] dev= media channel …`, `[a2dp] dev=<addr> codec=sbc 44100 js bitpool=<n> mtu=<n>
//! -> streaming`, `[a2dp] dev= -> FAILED reason=`, `[a2dp] dev= gone`; `tests a2dp`: `:: A2DP: sbc_kat=ok avdtp=ok
//! frag=ok stream=<ok|no-device> sdp_server=<ok|FAIL> inbound=<ok|FAIL> avrcp=<ok|FAIL> avrcp_ct=<ok|FAIL> avrcp_mute=<ok|FAIL> -> PASS|SKIP|FAIL ::` (`sdp_server`, `inbound`: SDPSERVER
//! B769; `avrcp=` AVRCP B774; `avrcp_ct=` AVRCPCT B780; `avrcp_mute=` AVRCPMUTE B790); `[a2dp] dev= the peer opened the link -> acceptor …` (a reconnecting headset drives AVDTP, [`acp_due`]). Compiled only under the `a2dp` knob (A2DPKNOB B750).

use super::l2cap::{self, AvdtpRole};
use super::sdp;
use super::{fmt_addr, Bt, LinkSt, PSM_SDP};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// The SBC encoder: `sbc_core`'s one source file (no Cargo dependency; the `install/abimage.rs` shape).
#[path = "../../../../../libs/media/sbc_core/src/sbc.rs"]
#[allow(dead_code)]
pub mod sbc;
use sbc::{Alloc, Config, Encoder};

/// The stream's sampling rate.
pub const RATE: u32 = 44_100;

/// Our one stream end-point.
const OUR_SEID: u8 = 1;

// Signal identifiers.
const DISCOVER: u8 = 0x01;
const GET_CAPS: u8 = 0x02;
const SET_CONFIG: u8 = 0x03;
const OPEN: u8 = 0x06;
const START: u8 = 0x07;
const CLOSE: u8 = 0x08;
const SUSPEND: u8 = 0x09;
const ABORT: u8 = 0x0A;
const GET_ALL_CAPS: u8 = 0x0C;
const DELAY_REPORT: u8 = 0x0D;

// Message types.
const MT_CMD: u8 = 0;
const MT_GREJ: u8 = 1;
const MT_ACC: u8 = 2;
const MT_REJ: u8 = 3;

// Error codes.
const E_BAD_ACP_SEID: u8 = 0x12;
const E_SEP_IN_USE: u8 = 0x13;
const E_UNSUPPORTED_CONFIG: u8 = 0x29;
const E_BAD_STATE: u8 = 0x31;

// The SBC codec information element (A2DP): octet 0 = sampling frequency | channel mode, octet 1 = block length |
// subbands | allocation method, octets 2/3 = minimum / maximum bitpool.
const SBC_44100: u8 = 0x20;
const SBC_JOINT: u8 = 0x01;
const SBC_BLOCKS16: u8 = 0x10;
const SBC_SUBBANDS8: u8 = 0x04;
const SBC_SNR: u8 = 0x02;
const SBC_LOUDNESS: u8 = 0x01;
/// The A2DP specification's high-quality joint-stereo bitpool.
const MAX_BITPOOL: u8 = 53;

/// RTP header (12) + SBC media payload header (1).
const MEDIA_HDR: usize = 13;
const SSRC: u32 = 1;
/// The send clock's lead over real time.
const LEAD_FRAMES: u64 = RATE as u64 * 40 / 1000;
/// An acceptor-side wait before this host takes the initiative (discover, or start an opened stream).
pub(super) const ACP_WAIT_MS: u64 = 1500;

/// (bitpool, frames per packet) for a peer offering `min_bp..=max_bp` when one packet carries `fit` bytes: the
/// highest bitpool up to 53 that still puts TWO frames in a packet if that keeps it at 35 or more (the
/// specification's middle quality), else the highest that puts one; None when not even one frame fits.
pub fn pick_bitpool(min_bp: u8, max_bp: u8, fit: usize) -> Option<(u8, u8)> {
    let flen = |bp: u8| Config { bitpool: bp, ..Config::A2DP_HQ }.frame_len();
    let (lo, hi) = (min_bp.max(2), max_bp.min(MAX_BITPOOL));
    if lo > hi || fit <= MEDIA_HDR {
        return None;
    }
    let room = fit - MEDIA_HDR;
    let mut bp = hi;
    if room / flen(bp) < 2 {
        let mut b = hi;
        while b > lo.max(35) && room / flen(b) < 2 {
            b -= 1;
        }
        if room / flen(b) >= 2 {
            bp = b;
        }
    }
    while bp > lo && room < flen(bp) {
        bp -= 1;
    }
    if room < flen(bp) {
        return None;
    }
    Some((bp, (room / flen(bp)).min(15) as u8))
}

/// The SBC codec element in a capability list (category 7, audio, codec type 0): its four information octets.
fn sbc_caps(mut c: &[u8]) -> Option<[u8; 4]> {
    while c.len() >= 2 {
        let (cat, len) = (c[0], c[1] as usize);
        if 2 + len > c.len() {
            return None;
        }
        let d = &c[2..2 + len];
        if cat == 0x07 && len >= 6 && d[0] >> 4 == 0 && d[1] == 0 {
            return Some([d[2], d[3], d[4], d[5]]);
        }
        c = &c[2 + len..];
    }
    None
}

/// The RTP + SBC media payload header of one media packet.
pub fn media_header(seq: u16, ts: u32, frames: u8) -> [u8; MEDIA_HDR] {
    let mut h = [0u8; MEDIA_HDR];
    h[0] = 0x80; // version 2, no padding, no extension, no CSRC
    h[1] = 0x60; // no marker, payload type 96 (dynamic)
    h[2..4].copy_from_slice(&seq.to_be_bytes());
    h[4..8].copy_from_slice(&ts.to_be_bytes());
    h[8..12].copy_from_slice(&SSRC.to_be_bytes());
    h[12] = frames & 0x0F; // not fragmented, not first/last, frame count
    h
}

/// The AVDTP stream state of the one remote end-point.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum St {
    Idle,
    Discover,
    Caps,
    Config,
    Configured,
    Opening,
    Open,
    Starting,
    Streaming,
    Suspended,
    Closed,
    Failed,
}

impl St {
    pub fn name(self) -> &'static str {
        match self {
            St::Idle => "idle",
            St::Discover => "discovering",
            St::Caps => "getting-capabilities",
            St::Config => "configuring",
            St::Configured => "configured",
            St::Opening => "opening",
            St::Open => "open",
            St::Starting => "starting",
            St::Streaming => "streaming",
            St::Suspended => "suspended",
            St::Closed => "closed",
            St::Failed => "failed",
        }
    }
}

/// What the state machine asks of the transport.
#[derive(Debug, PartialEq, Eq)]
pub enum Out {
    /// A signalling message for the signalling channel.
    Send(Vec<u8>),
    /// OPEN was accepted: open the media channel.
    OpenMedia,
    /// The stream is started and its media channel is up: send.
    Streaming,
    /// The stream left STREAMING (suspend, close, abort, media channel gone).
    Stopped,
    Failed(&'static str),
}

/// AVDTP signalling with one remote device: pure (bytes in, [`Out`]s out), driven by the glue below and by
/// `tests a2dp`'s synthetic peer.
pub struct Peer {
    pub st: St,
    /// This host drove the configuration.
    pub int: bool,
    label: u8,
    pending: Option<(u8, u8)>,
    cands: [u8; 8],
    ncand: usize,
    cand: usize,
    /// The remote SEID our commands address.
    pub acp: u8,
    pub bitpool: u8,
    pub min_bp: u8,
    pub snr: bool,
    /// SBC frames per media packet.
    pub fpp: u8,
    fit: usize,
    media: bool,
    start_pending: bool,
}

impl Peer {
    pub fn new(fit: usize) -> Peer {
        Peer { st: St::Idle, int: false, label: 0, pending: None, cands: [0; 8], ncand: 0, cand: 0, acp: 0, bitpool: 0, min_bp: 2, snr: false, fpp: 1, fit, media: false, start_pending: false }
    }

    fn cmd(&mut self, sig: u8, args: &[u8], out: &mut Vec<Out>) {
        self.label = (self.label + 1) & 0x0F;
        self.pending = Some((self.label, sig));
        let mut v = Vec::with_capacity(2 + args.len());
        v.push(self.label << 4 | MT_CMD);
        v.push(sig);
        v.extend_from_slice(args);
        out.push(Out::Send(v));
    }

    fn reply(label: u8, mt: u8, sig: u8, args: &[u8], out: &mut Vec<Out>) {
        let mut v = Vec::with_capacity(2 + args.len());
        v.push(label << 4 | mt);
        v.push(sig);
        v.extend_from_slice(args);
        out.push(Out::Send(v));
    }

    fn fail(&mut self, why: &'static str, out: &mut Vec<Out>) {
        self.st = St::Failed;
        self.pending = None;
        out.push(Out::Failed(why));
    }

    fn busy(&self) -> bool {
        matches!(self.st, St::Configured | St::Opening | St::Open | St::Starting | St::Streaming | St::Suspended)
    }

    /// The packet bound changed (the media channel's MTU is known): frames per packet for the chosen bitpool.
    pub fn set_fit(&mut self, fit: usize) {
        self.fit = fit;
        if self.bitpool >= 2 {
            let flen = Config { bitpool: self.bitpool, ..Config::A2DP_HQ }.frame_len();
            self.fpp = (fit.saturating_sub(MEDIA_HDR) / flen).clamp(1, 15) as u8;
        }
    }

    /// Become the initiator: DISCOVER.
    pub fn start(&mut self, out: &mut Vec<Out>) {
        self.int = true;
        self.st = St::Discover;
        self.cmd(DISCOVER, &[], out);
    }

    /// One signalling message from the peer.
    pub fn on_sig(&mut self, m: &[u8], out: &mut Vec<Out>) {
        if m.len() < 2 {
            return;
        }
        let (label, pkt, mt, sig) = (m[0] >> 4, (m[0] >> 2) & 3, m[0] & 3, m[1] & 0x3F);
        if pkt != 0 {
            // Start / continue / end packets: fragmented signalling is not reassembled by this host.
            if self.pending.is_some() {
                self.fail("fragmented-signal", out);
            }
            return;
        }
        match mt {
            MT_CMD => self.on_cmd(label, sig, &m[2..], out),
            _ => {
                if self.pending != Some((label, sig)) {
                    return; // not the answer to our outstanding command
                }
                self.pending = None;
                if mt == MT_ACC {
                    self.on_accept(sig, &m[2..], out);
                } else {
                    self.on_reject(sig, out);
                }
            }
        }
    }

    fn next_cand(&mut self, why: &'static str, out: &mut Vec<Out>) {
        self.cand += 1;
        if self.cand < self.ncand {
            self.st = St::Caps;
            let s = self.cands[self.cand];
            self.cmd(GET_CAPS, &[s << 2], out);
        } else {
            self.fail(why, out);
        }
    }

    fn on_accept(&mut self, sig: u8, p: &[u8], out: &mut Vec<Out>) {
        match (sig, self.st) {
            (DISCOVER, St::Discover) => {
                self.ncand = 0;
                for s in p.chunks_exact(2) {
                    let (seid, in_use, media, sink) = (s[0] >> 2, s[0] & 0x02 != 0, s[1] >> 4, s[1] & 0x08 != 0);
                    if media == 0 && sink && !in_use && self.ncand < self.cands.len() {
                        self.cands[self.ncand] = seid;
                        self.ncand += 1;
                    }
                }
                if self.ncand == 0 {
                    return self.fail("no-free-audio-sink-sep", out);
                }
                self.cand = 0;
                self.st = St::Caps;
                let s = self.cands[0];
                self.cmd(GET_CAPS, &[s << 2], out);
            }
            (GET_CAPS, St::Caps) => {
                let Some(c) = sbc_caps(p) else { return self.next_cand("no-sbc-codec", out) };
                let ok = c[0] & SBC_44100 != 0 && c[0] & SBC_JOINT != 0 && c[1] & SBC_BLOCKS16 != 0 && c[1] & SBC_SUBBANDS8 != 0 && c[1] & (SBC_LOUDNESS | SBC_SNR) != 0;
                if !ok {
                    return self.next_cand("sbc-44100-js-not-offered", out);
                }
                let Some((bp, n)) = pick_bitpool(c[2], c[3], self.fit) else { return self.next_cand("bitpool-does-not-fit", out) };
                self.acp = self.cands[self.cand];
                self.snr = c[1] & SBC_LOUDNESS == 0;
                self.min_bp = c[2].max(2);
                self.bitpool = bp;
                self.fpp = n;
                self.st = St::Config;
                let alloc = if self.snr { SBC_SNR } else { SBC_LOUDNESS };
                let args = [self.acp << 2, OUR_SEID << 2, 0x01, 0x00, 0x07, 0x06, 0x00, 0x00, SBC_44100 | SBC_JOINT, SBC_BLOCKS16 | SBC_SUBBANDS8 | alloc, self.min_bp, bp];
                self.cmd(SET_CONFIG, &args, out);
            }
            (SET_CONFIG, St::Config) => {
                self.st = St::Opening;
                let a = self.acp;
                self.cmd(OPEN, &[a << 2], out);
            }
            (OPEN, St::Opening) => {
                self.st = St::Open;
                out.push(Out::OpenMedia);
            }
            (START, St::Starting) => {
                self.st = St::Streaming;
                out.push(Out::Streaming);
            }
            _ => {}
        }
    }

    fn on_reject(&mut self, sig: u8, out: &mut Vec<Out>) {
        match sig {
            DISCOVER => self.fail("discover-rejected", out),
            GET_CAPS => self.next_cand("get-capabilities-rejected", out),
            SET_CONFIG => self.next_cand("set-configuration-rejected", out),
            OPEN => self.fail("open-rejected", out),
            START => self.fail("start-rejected", out),
            _ => {}
        }
    }

    fn on_cmd(&mut self, label: u8, sig: u8, p: &[u8], out: &mut Vec<Out>) {
        let ours = p.first().map(|b| b >> 2) == Some(OUR_SEID);
        match sig {
            DISCOVER => Self::reply(label, MT_ACC, sig, &[OUR_SEID << 2 | (self.busy() as u8) << 1, 0x00], out),
            GET_CAPS | GET_ALL_CAPS if ours => {
                let caps = [0x01, 0x00, 0x07, 0x06, 0x00, 0x00, SBC_44100 | SBC_JOINT, SBC_BLOCKS16 | SBC_SUBBANDS8 | SBC_LOUDNESS | SBC_SNR, 2, MAX_BITPOOL];
                Self::reply(label, MT_ACC, sig, &caps, out);
            }
            GET_CAPS | GET_ALL_CAPS => Self::reply(label, MT_REJ, sig, &[E_BAD_ACP_SEID], out),
            SET_CONFIG => {
                if !ours || p.len() < 2 {
                    return Self::reply(label, MT_REJ, sig, &[0x00, E_BAD_ACP_SEID], out);
                }
                if self.busy() {
                    return Self::reply(label, MT_REJ, sig, &[0x00, E_SEP_IN_USE], out);
                }
                let fits = sbc_caps(&p[2..]).and_then(|c| {
                    let shape = c[0] == SBC_44100 | SBC_JOINT && c[1] & 0xF0 == SBC_BLOCKS16 && c[1] & 0x0C == SBC_SUBBANDS8 && matches!(c[1] & 0x03, SBC_LOUDNESS | SBC_SNR);
                    if shape { pick_bitpool(c[2], c[3], self.fit).map(|(bp, n)| (c, bp, n)) } else { None }
                });
                let Some((c, bp, n)) = fits else { return Self::reply(label, MT_REJ, sig, &[0x07, E_UNSUPPORTED_CONFIG], out) };
                self.int = false;
                self.pending = None; // a collision: the peer's configuration wins over our own procedure
                self.acp = p[1] >> 2;
                self.snr = c[1] & 0x03 == SBC_SNR;
                self.min_bp = c[2].max(2);
                self.bitpool = bp;
                self.fpp = n;
                self.st = St::Configured;
                Self::reply(label, MT_ACC, sig, &[], out);
            }
            OPEN => {
                if self.st == St::Configured {
                    self.st = St::Open;
                    Self::reply(label, MT_ACC, sig, &[], out);
                } else {
                    Self::reply(label, MT_REJ, sig, &[E_BAD_STATE], out);
                }
            }
            START => {
                if matches!(self.st, St::Open | St::Suspended | St::Starting) {
                    Self::reply(label, MT_ACC, sig, &[], out);
                    if self.media {
                        self.st = St::Streaming;
                        self.pending = None;
                        out.push(Out::Streaming);
                    } else {
                        self.st = St::Starting;
                        self.start_pending = true;
                    }
                } else {
                    Self::reply(label, MT_REJ, sig, &[OUR_SEID << 2, E_BAD_STATE], out);
                }
            }
            SUSPEND => {
                Self::reply(label, MT_ACC, sig, &[], out);
                if self.st == St::Streaming {
                    self.st = St::Suspended;
                    out.push(Out::Stopped);
                }
            }
            CLOSE | ABORT => {
                Self::reply(label, MT_ACC, sig, &[], out);
                let was = self.st == St::Streaming;
                self.st = St::Closed;
                self.pending = None;
                if was {
                    out.push(Out::Stopped);
                }
            }
            DELAY_REPORT => Self::reply(label, MT_ACC, sig, &[], out),
            _ => Self::reply(label, MT_GREJ, sig, &[], out),
        }
    }

    /// The media channel is open.
    pub fn media_up(&mut self, out: &mut Vec<Out>) {
        self.media = true;
        if self.start_pending && self.st == St::Starting {
            self.start_pending = false;
            self.st = St::Streaming;
            out.push(Out::Streaming);
        } else if self.int && self.st == St::Open {
            self.start_now(out);
        }
    }

    /// START an opened stream (the initiator's step; an acceptor takes it after [`ACP_WAIT_MS`]).
    pub fn start_now(&mut self, out: &mut Vec<Out>) {
        if self.st == St::Open && self.media {
            self.st = St::Starting;
            let a = self.acp;
            self.cmd(START, &[a << 2], out);
        }
    }

    /// The media channel closed.
    pub fn media_down(&mut self, out: &mut Vec<Out>) {
        self.media = false;
        if matches!(self.st, St::Streaming | St::Starting) {
            self.st = St::Closed;
            out.push(Out::Stopped);
        }
    }
}

// ── The glue: one A2DP stream, on the BTHID host's link ─────────────────────────────────────────────────────────

struct Stream {
    addr: [u8; 6],
    /// Signalling channel (lcid, rcid).
    sig: (u16, u16),
    /// Media channel (lcid, rcid).
    media: Option<(u16, u16)>,
    peer: Peer,
    enc: Option<Encoder>,
    seq: u16,
    ts: u32,
    t0: Option<u64>,
    sent: u64,
    opened_at: u64,
    media_at: u64,
    /// The media packet bound in bytes.
    mtu: usize,
}

/// The one A2DP stream. Lock order: BTHID's `BT`, then this, then `PCM`; the audio side takes only `PCM` and atomics.
static A2: crate::sync::Mutex<Option<Box<Stream>>> = crate::sync::Mutex::new(None);
/// The device `bt audio` named (stays named across a reconnect).
static WANT: crate::sync::Mutex<Option<[u8; 6]>> = crate::sync::Mutex::new(None);
/// Settings' label for the device ("BT <name>").
static LABEL: crate::sync::Mutex<Option<String>> = crate::sync::Mutex::new(None);
static STREAMING: AtomicBool = AtomicBool::new(false);
static WRITER: AtomicBool = AtomicBool::new(false);
static FRAMES: AtomicU64 = AtomicU64::new(0);
static PACKETS: AtomicU64 = AtomicU64::new(0);
static UNDERRUNS: AtomicU32 = AtomicU32::new(0);
/// SDPSERVER (B769): a link the PEER opened whose SDP is done (address, when): AVDTP signalling is the peer's to open
/// for [`ACP_WAIT_MS`], then [`acp_due`] opens it. Lock order: BTHID's `BT`, then this alone (never held with `A2`).
static ACP_DUE: crate::sync::Mutex<Option<([u8; 6], u64)>> = crate::sync::Mutex::new(None);

impl Bt {
    /// This link is an A2DP link: `bt audio` named it, a stream already rides it, or its Class of Device is
    /// Audio/Video (major class 0x04).
    pub(super) fn a2dp_link(&self, li: usize) -> bool {
        let a = self.links[li].addr;
        if *WANT.lock() == Some(a) || A2.lock().as_ref().is_some_and(|s| s.addr == a) {
            return true;
        }
        let cod = self.bond(&a).map(|b| b.cod).filter(|&c| c != 0).or_else(|| self.dev(&a).map(|d| d.cod)).unwrap_or(0);
        (cod >> 8) & 0x1F == 0x04
    }

    /// The link is up (authenticated, encrypted): ask SDP for the AudioSink record.
    pub(super) fn a2dp_link_up(&mut self, li: usize, now: u64) {
        serial_println!("[a2dp] dev={} link up -> sdp (AudioSink 0x110b)", fmt_addr(&self.links[li].addr));
        self.open_chan(li, PSM_SDP, now);
    }

    /// SDP's gathered AttributeLists for an audio link.
    pub(super) fn a2dp_sdp(&mut self, li: usize, acc: &[u8]) {
        let a = fmt_addr(&self.links[li].addr);
        match sdp::a2dp_sink(acc) {
            Some(r) => serial_println!("[a2dp] sdp dev={} sink=yes psm={:#06x} avdtp={}.{} a2dp={}.{}", a, r.psm, r.avdtp >> 8, r.avdtp & 0xFF, r.a2dp >> 8, r.a2dp & 0xFF),
            None => serial_println!("[a2dp] sdp dev={} sink=no bytes={} (no AudioSink record read; AVDTP is tried on its fixed PSM 0x0019)", a, acc.len()),
        }
    }

    /// SDP finished (or failed): open AVDTP signalling unless the peer already did — on a link the PEER opened (a
    /// headset reconnecting on power-on reads our source record and opens AVDTP itself), only after [`acp_due`]'s wait.
    pub(super) fn a2dp_after_sdp(&mut self, li: usize, now: u64) {
        let a = self.links[li].addr;
        let have = A2.lock().as_ref().is_some_and(|s| s.addr == a);
        if !have && self.links[li].chan_psm(l2cap::PSM_AVDTP).is_none() {
            if !self.links[li].outbound {
                *ACP_DUE.lock() = Some((a, now)); // SDPSERVER (B769): the acceptor role; two signalling opens at once would make the second "media"
                serial_println!("[a2dp] dev={} the peer opened the link -> acceptor (AVDTP is the peer's to open for {} ms)", fmt_addr(&a), ACP_WAIT_MS);
                return;
            }
            self.open_chan(li, l2cap::PSM_AVDTP, now);
        }
    }

    fn a2dp_label(&self, a: &[u8; 6]) -> String {
        let name = self.bond(a).map(|b| b.name.clone()).filter(|n| !n.is_empty()).or_else(|| self.dev(a).map(|d| d.name()).filter(|n| !n.is_empty()));
        format!("BT {}", name.unwrap_or_else(|| fmt_addr(a)))
    }

    /// An L2CAP channel on PSM 0x0019 is open: the first is signalling, the next the media transport.
    pub(super) fn a2dp_chan_open(&mut self, li: usize, ci: usize, now: u64) {
        let c = self.links[li].chans[ci];
        let addr = self.links[li].addr;
        let fit = l2cap::payload_fit(self.acl_len, c.rmtu);
        let label = self.a2dp_label(&addr);
        let mut outs = Vec::new();
        let mut refuse: Option<&str> = None;
        {
            let mut g = A2.lock();
            let other = g.as_ref().is_some_and(|s| s.addr != addr);
            let sig_open = g.as_ref().is_some_and(|s| s.addr == addr);
            if other {
                refuse = Some("another device holds the A2DP stream");
            } else {
                match l2cap::avdtp_role(sig_open) {
                    AvdtpRole::Media => {
                        let Some(s) = g.as_mut() else { return };
                        if s.media.is_some() {
                            refuse = Some("a media channel is already open");
                        } else {
                            s.media = Some((c.lcid, c.rcid));
                            s.media_at = now;
                            s.mtu = fit;
                            s.peer.set_fit(fit);
                            s.peer.media_up(&mut outs);
                            serial_println!("[a2dp] dev={} media channel lcid={:#06x} rcid={:#06x} peer_mtu={} payload_max={} state={}", fmt_addr(&addr), c.lcid, c.rcid, c.rmtu, fit, s.peer.st.name());
                        }
                    }
                    AvdtpRole::Signalling => {
                        let mut s = Box::new(Stream { addr, sig: (c.lcid, c.rcid), media: None, peer: Peer::new(fit), enc: None, seq: 0, ts: 0, t0: None, sent: 0, opened_at: now, media_at: 0, mtu: fit });
                        if c.ours {
                            s.peer.start(&mut outs);
                        }
                        serial_println!("[a2dp] dev={} signalling channel lcid={:#06x} rcid={:#06x} role={}", fmt_addr(&addr), c.lcid, c.rcid, if c.ours { "initiator (discover sent)" } else { "acceptor (the peer drives)" });
                        *g = Some(s);
                        *LABEL.lock() = Some(label);
                    }
                }
            }
        }
        if let Some(why) = refuse {
            serial_println!("[a2dp] dev={} avdtp channel lcid={:#06x} REFUSED — {}", fmt_addr(&addr), c.lcid, why);
            self.close_chan(li, ci, now);
            return;
        }
        self.a2dp_apply(li, outs, now);
    }

    /// A PDU on an AVDTP channel.
    pub(super) fn a2dp_rx(&mut self, li: usize, ci: usize, body: &[u8], now: u64) {
        let lcid = self.links[li].chans[ci].lcid;
        let mut outs = Vec::new();
        {
            let mut g = A2.lock();
            let Some(s) = g.as_mut() else { return };
            if s.sig.0 != lcid {
                return; // the media channel: a sink sends nothing on it
            }
            s.peer.on_sig(body, &mut outs);
        }
        self.a2dp_apply(li, outs, now);
    }

    /// The peer closed an AVDTP channel.
    pub(super) fn a2dp_chan_closed(&mut self, li: usize, lcid: u16, now: u64) {
        let addr = self.links[li].addr;
        let mut outs = Vec::new();
        let gone = {
            let mut g = A2.lock();
            match g.as_mut() {
                Some(s) if s.addr == addr && s.sig.0 == lcid => true,
                Some(s) if s.addr == addr && s.media.map(|m| m.0) == Some(lcid) => {
                    s.media = None;
                    s.enc = None;
                    s.peer.media_down(&mut outs);
                    false
                }
                _ => false,
            }
        };
        if gone {
            link_gone(&addr);
        } else {
            self.a2dp_apply(li, outs, now);
        }
    }

    fn a2dp_apply(&mut self, li: usize, outs: Vec<Out>, now: u64) {
        if outs.is_empty() {
            return;
        }
        let (addr, h) = (self.links[li].addr, self.links[li].handle);
        let Some(rcid) = A2.lock().as_ref().map(|s| s.sig.1) else { return };
        for o in outs {
            match o {
                Out::Send(v) => self.l2_send(h, rcid, &v),
                Out::OpenMedia => {
                    serial_println!("[a2dp] dev={} open accepted -> media channel", fmt_addr(&addr));
                    self.open_chan(li, l2cap::PSM_AVDTP, now);
                }
                Out::Streaming => {
                    if let Some(s) = A2.lock().as_mut() {
                        let cfg = Config { bitpool: s.peer.bitpool, alloc: if s.peer.snr { Alloc::Snr } else { Alloc::Loudness }, ..Config::A2DP_HQ };
                        s.enc = Some(Encoder::new(cfg));
                        s.t0 = None;
                        s.sent = 0;
                        serial_println!("[a2dp] dev={} codec=sbc 44100 js bitpool={} mtu={} -> streaming", fmt_addr(&addr), cfg.bitpool, s.mtu);
                        serial_println!("[a2dp] dev={} frame_len={} frames_per_packet={} alloc={} role={}", fmt_addr(&addr), cfg.frame_len(), s.peer.fpp, if s.peer.snr { "snr" } else { "loudness" }, if s.peer.int { "initiator" } else { "acceptor" });
                    }
                    STREAMING.store(true, Ordering::Release);
                    super::avctp::ct_arm(addr, now); // AVRCPCT (B780): the AVRCP controller is bound to the streaming headset
                }
                Out::Stopped => {
                    STREAMING.store(false, Ordering::Release);
                    let st = A2.lock().as_ref().map(|s| s.peer.st.name()).unwrap_or("gone");
                    serial_println!("[a2dp] dev={} stream stopped state={}", fmt_addr(&addr), st);
                }
                Out::Failed(why) => {
                    STREAMING.store(false, Ordering::Release);
                    serial_println!("[a2dp] dev={} -> FAILED reason={}", fmt_addr(&addr), why);
                }
            }
        }
    }
}

/// The link to `addr` went down (or its signalling channel closed): the stream ends with it.
pub(super) fn link_gone(addr: &[u8; 6]) {
    let mut g = A2.lock();
    if g.as_ref().is_some_and(|s| s.addr == *addr) {
        let pk = PACKETS.load(Ordering::Relaxed);
        *g = None;
        STREAMING.store(false, Ordering::Release);
        *LABEL.lock() = None;
        serial_println!("[a2dp] dev={} gone packets={} underruns={}", fmt_addr(addr), pk, UNDERRUNS.load(Ordering::Relaxed));
    }
}

/// SDPSERVER (B769): the acceptor's wait on a link the peer opened ran out with no stream: this host opens AVDTP
/// signalling itself (the initiator path, as after its own SDP query).
fn acp_due(bt: &mut Bt, now: u64) {
    let due = {
        let mut g = ACP_DUE.lock();
        match *g {
            Some((a, t)) if now.saturating_sub(t) > ACP_WAIT_MS => {
                *g = None;
                Some(a)
            }
            _ => None,
        }
    };
    let Some(a) = due else { return };
    let Some(li) = bt.link_addr(&a) else { return };
    let have = A2.lock().as_ref().is_some_and(|s| s.addr == a);
    if !have && bt.links[li].st == LinkSt::Up && bt.links[li].chan_psm(l2cap::PSM_AVDTP).is_none() {
        serial_println!("[a2dp] dev={} the peer opened no AVDTP within {} ms -> signalling opened by this host", fmt_addr(&a), ACP_WAIT_MS);
        bt.open_chan(li, l2cap::PSM_AVDTP, now);
    }
}

/// Once per BTHID pass (`step`, under the `BT` lock): the acceptor-side waits, the AVRCP watch on the master, then the
/// media clock.
pub(super) fn tick(bt: &mut Bt, now: u64) {
    acp_due(bt, now);
    bt.avrcp_watch(now); // AVRCPCT (B780): the CT's outbound AVCTP open, the TG's CHANGED and the CT's SetAbsoluteVolume
    let mut outs = Vec::new();
    let li = {
        let mut g = A2.lock();
        let Some(s) = g.as_mut() else { return };
        let Some(li) = bt.link_addr(&s.addr) else { return };
        if s.peer.st == St::Idle && !s.peer.int && now.saturating_sub(s.opened_at) > ACP_WAIT_MS {
            serial_println!("[a2dp] dev={} the peer opened signalling and sent nothing for {} ms -> discover", fmt_addr(&s.addr), ACP_WAIT_MS);
            s.peer.start(&mut outs);
        }
        if s.peer.st == St::Open && s.media.is_some() && now.saturating_sub(s.media_at) > ACP_WAIT_MS {
            s.peer.start_now(&mut outs);
        }
        li
    };
    if !outs.is_empty() {
        bt.a2dp_apply(li, outs, now);
    }
    if !STREAMING.load(Ordering::Acquire) {
        return;
    }
    let h = bt.links[li].handle;
    let mut g = A2.lock();
    let Some(s) = g.as_mut() else { return };
    let Stream { media, peer, enc, seq, ts, t0, sent, .. } = &mut **s;
    let (Some((_, mrcid)), Some(enc)) = (*media, enc.as_mut()) else { return };
    let cfg = *enc.config();
    let (spf, flen, n) = (cfg.samples(), cfg.frame_len(), peer.fpp.max(1) as usize);
    let start = match *t0 {
        Some(t) => t,
        None => {
            if queued() == 0 {
                return;
            }
            *t0 = Some(now);
            *sent = 0;
            now
        }
    };
    let due = now.saturating_sub(start) * RATE as u64 / 1000 + LEAD_FRAMES;
    let mut k = 0;
    let nf = l2cap::frag_count(4 + MEDIA_HDR + n * flen, bt.acl_len); // A2DPKNOB (B750): the queue bound counts fragments
    while *sent + (n * spf) as u64 <= due && bt.acltx.len() + nf <= 6 && k < 4 {
        let mut pkt = Vec::with_capacity(MEDIA_HDR + n * flen);
        pkt.extend_from_slice(&media_header(*seq, *ts, n as u8));
        let mut short = 0u32;
        for _ in 0..n {
            let mut pcm = [0i16; 256];
            if pcm_take(&mut pcm[..spf * 2]) < spf {
                short += 1;
            }
            let at = pkt.len();
            pkt.resize(at + flen, 0);
            enc.encode(&pcm[..spf * 2], &mut pkt[at..]);
        }
        let writer = WRITER.load(Ordering::Acquire);
        if short == n as u32 && !writer {
            *t0 = None; // the ring ran dry and nobody writes: the clock stops, this silent packet is not sent
            break;
        }
        if short > 0 && writer {
            UNDERRUNS.fetch_add(short, Ordering::Relaxed);
        }
        bt.l2_send(h, mrcid, &pkt);
        *seq = seq.wrapping_add(1);
        *ts = ts.wrapping_add((n * spf) as u32);
        *sent += (n * spf) as u64;
        FRAMES.fetch_add((n * spf) as u64, Ordering::Relaxed);
        PACKETS.fetch_add(1, Ordering::Relaxed);
        k += 1;
    }
}

// ── The sink: a PCM ring the audio seam writes and the media clock drains ──────────────────────────────────────

/// 16384 samples = 8192 stereo frames, 186 ms at 44.1 kHz.
const PCM_CAP: usize = 16384;

struct Pcm {
    buf: [i16; PCM_CAP],
    head: usize,
    len: usize,
}

static PCM: crate::sync::Mutex<Pcm> = crate::sync::Mutex::new(Pcm { buf: [0; PCM_CAP], head: 0, len: 0 });

/// Fill `dst` (interleaved stereo) from the ring, zero the rest; whole frames taken.
fn pcm_take(dst: &mut [i16]) -> usize {
    let mut p = PCM.lock();
    let n = dst.len().min(p.len) & !1;
    for i in 0..n {
        dst[i] = p.buf[(p.head + i) % PCM_CAP];
    }
    p.head = (p.head + n) % PCM_CAP;
    p.len -= n;
    for x in dst[n..].iter_mut() {
        *x = 0;
    }
    n / 2
}

/// A stream is started and its media channel is up.
pub fn streaming() -> bool {
    STREAMING.load(Ordering::Acquire)
}

/// Settings' label of the A2DP device on a link ("BT <name>"), None when no device holds the stream.
pub fn device() -> Option<String> {
    LABEL.lock().clone()
}

/// A writer takes the sink (the ring is drained at the stream's rate while it holds it).
pub fn begin() -> Result<(), &'static str> {
    if !streaming() {
        return Err("no-a2dp-stream");
    }
    WRITER.store(true, Ordering::Release);
    Ok(())
}

/// PCM 16-bit little-endian stereo 44.1 kHz into the ring: bytes taken (whole frames; 0 when full or not streaming).
pub fn write(bytes: &[u8]) -> usize {
    if !streaming() {
        return 0;
    }
    let mut p = PCM.lock();
    let frames = (bytes.len() / 4).min((PCM_CAP - p.len) / 2);
    for f in 0..frames {
        for c in 0..2 {
            let s = i16::from_le_bytes([bytes[f * 4 + c * 2], bytes[f * 4 + c * 2 + 1]]);
            let at = (p.head + p.len) % PCM_CAP;
            p.buf[at] = s;
            p.len += 1;
        }
    }
    frames * 4
}

/// The writer is done (what is queued still plays).
pub fn finish() {
    WRITER.store(false, Ordering::Release);
}

/// Frames waiting in the ring.
pub fn queued() -> usize {
    PCM.lock().len / 2
}

/// (frames sent, underruns) since boot.
pub fn counters() -> (u64, u32) {
    (FRAMES.load(Ordering::Relaxed), UNDERRUNS.load(Ordering::Relaxed))
}

// ── The verb's pieces (`bt audio <addr>`, the `bt` table's stream line) ──────────────────────────────────────────

/// `bt audio <addr>`: name the device, then page it (pairing when not bonded) or, on a live link, start the A2DP
/// steps there.
pub(super) fn verb_audio(bt: &mut Bt, a: [u8; 6], now: u64, out: &mut dyn FnMut(&str)) {
    *WANT.lock() = Some(a);
    if let Some(li) = bt.link_addr(&a) {
        if bt.links[li].st != LinkSt::Up {
            return out("bt: audio refused — that link is still being set up; try again when `bt` shows it up");
        }
        if A2.lock().as_ref().is_some_and(|s| s.addr == a) {
            return out("bt: an audio stream is already set up on that link (`bt` shows it)");
        }
        bt.a2dp_link_up(li, now);
        return out(&format!("bt: audio {} — SDP, then AVDTP on the live link; `bt` shows the stream", fmt_addr(&a)));
    }
    let pair = bt.bond(&a).is_none();
    match bt.start_connect(a, pair, false, now) {
        Ok(()) => out(&format!("bt: audio {} — {} (5 s page bound), then SDP, AVDTP, SBC; `bt` shows the stream", fmt_addr(&a), if pair { "pairing" } else { "connecting" })),
        Err(e) => out(&format!("bt: audio refused — {}", e)),
    }
}

/// The `bt` table's A2DP line.
pub(super) fn list_line(out: &mut dyn FnMut(&str)) {
    if let Some(s) = A2.lock().as_ref() {
        let (f, u) = counters();
        out(&format!("bt: a2dp {}  state={}  role={}  bitpool={}  frames_per_packet={}  payload_max={}  packets={}  frames={}  underruns={}", fmt_addr(&s.addr), s.peer.st.name(), if s.peer.int { "initiator" } else { "acceptor" }, s.peer.bitpool, s.peer.fpp, s.mtu, PACKETS.load(Ordering::Relaxed), f, u));
    }
}

// ── `tests a2dp` ────────────────────────────────────────────────────────────────────────────────────────────────

fn sent(outs: &[Out]) -> Option<&[u8]> {
    outs.iter().find_map(|o| if let Out::Send(v) = o { Some(&v[..]) } else { None })
}

fn step_is(p: &mut Peer, msg: &[u8], want: &[u8], leg: &'static str) -> Result<Vec<Out>, &'static str> {
    let mut o = Vec::new();
    p.on_sig(msg, &mut o);
    if sent(&o) != Some(want) {
        serial_println!("[a2dp] fixture {} sent={:02x?} want={:02x?}", leg, sent(&o), want);
        return Err(leg);
    }
    Ok(o)
}

/// The AVDTP state machine against a synthetic peer, both roles, byte for byte; SDP's record walk; L2CAP's fit.
fn avdtp_legs() -> Result<(), &'static str> {
    // Below AVDTP: L2CAP's decisions and the SDP sink record.
    let l2 = l2cap::payload_fit(1021, 672) == 500 && l2cap::payload_fit(200, 672) == 396 && l2cap::payload_fit(1021, 100) == 100 && l2cap::inbound_served(0x0019) && l2cap::inbound_served(0x0001) && !l2cap::inbound_served(0x0003) && l2cap::avdtp_role(false) == AvdtpRole::Signalling && l2cap::avdtp_role(true) == AvdtpRole::Media;
    if !l2 {
        return Err("l2cap");
    }
    if sdp::a2dp_sink(&sdp::fixture_record()) != Some(sdp::SinkRecord { psm: 0x0019, avdtp: 0x0103, a2dp: 0x0103 }) || sdp::a2dp_sink(&[0x35, 0x03, 0x09, 0x02, 0x06]).is_some() {
        return Err("sdp-record");
    }
    if pick_bitpool(2, 53, 248) != Some((52, 2)) || pick_bitpool(2, 35, 248) != Some((35, 2)) || pick_bitpool(40, 53, 100).is_some() {
        return Err("bitpool");
    }
    if media_header(7, 256, 2) != [0x80, 0x60, 0x00, 0x07, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02] {
        return Err("media-header");
    }
    // INITIATOR: discover -> capabilities -> configuration -> open -> media -> start.
    let mut p = Peer::new(248);
    let mut o = Vec::new();
    p.start(&mut o);
    if sent(&o) != Some(&[0x10, 0x01][..]) {
        return Err("int-discover");
    }
    // SEID 2 a source, SEID 3 a sink in use, SEID 1 a free audio sink.
    step_is(&mut p, &[0x12, 0x01, 0x08, 0x00, 0x0E, 0x08, 0x04, 0x08], &[0x20, 0x02, 0x04], "int-get-capabilities")?;
    step_is(&mut p, &[0x22, 0x02, 0x01, 0x00, 0x07, 0x06, 0x00, 0x00, 0xFF, 0xFF, 0x02, 0x35], &[0x30, 0x03, 0x04, 0x04, 0x01, 0x00, 0x07, 0x06, 0x00, 0x00, 0x21, 0x15, 0x02, 0x34], "int-set-configuration")?;
    step_is(&mut p, &[0x32, 0x03], &[0x40, 0x06, 0x04], "int-open")?;
    let mut o = Vec::new();
    p.on_sig(&[0x42, 0x06], &mut o);
    if o != [Out::OpenMedia] || p.st != St::Open {
        return Err("int-open-media");
    }
    let mut o = Vec::new();
    p.media_up(&mut o);
    if sent(&o) != Some(&[0x50, 0x07, 0x04][..]) {
        return Err("int-start");
    }
    let mut o = Vec::new();
    p.on_sig(&[0x92, 0x07], &mut o); // a stale label: ignored
    p.on_sig(&[0x52, 0x07], &mut o);
    if o != [Out::Streaming] || p.st != St::Streaming || p.bitpool != 52 || p.fpp != 2 || p.snr || p.acp != 1 {
        return Err("int-streaming");
    }
    // Refusals: a rejected DISCOVER fails; a sink without 44.1 kHz fails after its only candidate.
    let mut p = Peer::new(248);
    let mut o = Vec::new();
    p.start(&mut o);
    p.on_sig(&[0x13, 0x01, 0x31], &mut o);
    if !o.contains(&Out::Failed("discover-rejected")) {
        return Err("int-discover-rejected");
    }
    let mut p = Peer::new(248);
    let mut o = Vec::new();
    p.start(&mut o);
    step_is(&mut p, &[0x12, 0x01, 0x04, 0x08], &[0x20, 0x02, 0x04], "int-one-sink")?;
    let mut o = Vec::new();
    p.on_sig(&[0x22, 0x02, 0x01, 0x00, 0x07, 0x06, 0x00, 0x00, 0x11, 0x15, 0x02, 0x35], &mut o);
    if o != [Out::Failed("sbc-44100-js-not-offered")] {
        return Err("int-48k-only");
    }
    // ACCEPTOR: the peer discovers us, configures, opens, starts (before its media channel), suspends.
    let mut q = Peer::new(248);
    step_is(&mut q, &[0x50, 0x01], &[0x52, 0x01, 0x04, 0x00], "acp-discover")?;
    step_is(&mut q, &[0x60, 0x02, 0x04], &[0x62, 0x02, 0x01, 0x00, 0x07, 0x06, 0x00, 0x00, 0x21, 0x17, 0x02, 0x35], "acp-capabilities")?;
    step_is(&mut q, &[0x70, 0x03, 0x04, 0x08, 0x01, 0x00, 0x07, 0x06, 0x00, 0x00, 0x21, 0x15, 0x02, 0x35], &[0x72, 0x03], "acp-set-configuration")?;
    if q.st != St::Configured || q.acp != 2 || q.bitpool != 52 || q.int {
        return Err("acp-configured");
    }
    step_is(&mut q, &[0x80, 0x01], &[0x82, 0x01, 0x06, 0x00], "acp-discover-in-use")?;
    step_is(&mut q, &[0x90, 0x06, 0x04], &[0x92, 0x06], "acp-open")?;
    let o = step_is(&mut q, &[0xA0, 0x07, 0x04], &[0xA2, 0x07], "acp-start")?;
    if o.contains(&Out::Streaming) || q.st != St::Starting {
        return Err("acp-start-before-media");
    }
    let mut o = Vec::new();
    q.media_up(&mut o);
    if o != [Out::Streaming] || q.st != St::Streaming {
        return Err("acp-media-streaming");
    }
    let o = step_is(&mut q, &[0xB0, 0x09, 0x04], &[0xB2, 0x09], "acp-suspend")?;
    if !o.contains(&Out::Stopped) || q.st != St::Suspended {
        return Err("acp-suspended");
    }
    step_is(&mut q, &[0xC0, 0x3F], &[0xC1, 0x3F], "acp-general-reject")?;
    step_is(&mut q, &[0xD0, 0x03, 0x04, 0x08, 0x01, 0x00, 0x07, 0x06, 0x00, 0x00, 0x21, 0x15, 0x02, 0x35], &[0xD3, 0x03, 0x00, E_SEP_IN_USE], "acp-set-configuration-in-use")?;
    Ok(())
}

/// SDPSERVER (B769) `inbound` leg — a headset that reconnects and drives: its SDP and AVDTP channels are served
/// inbound, the first AVDTP channel is signalling, and the acceptor accepts its DISCOVER, SET_CONFIGURATION and OPEN
/// without opening the media channel itself (the peer that opened the stream opens its transport).
fn inbound_legs() -> Result<(), &'static str> {
    if !(l2cap::inbound_served(PSM_SDP) && l2cap::inbound_served(l2cap::PSM_AVDTP) && l2cap::avdtp_role(false) == AvdtpRole::Signalling) {
        return Err("inbound-psm");
    }
    let mut q = Peer::new(l2cap::payload_fit(1021, 672));
    step_is(&mut q, &[0x10, 0x01], &[0x12, 0x01, 0x04, 0x00], "inbound-discover")?;
    step_is(&mut q, &[0x20, 0x03, 0x04, 0x08, 0x01, 0x00, 0x07, 0x06, 0x00, 0x00, 0x21, 0x15, 0x02, 0x35], &[0x22, 0x03], "inbound-set-configuration")?;
    let o = step_is(&mut q, &[0x30, 0x06, 0x04], &[0x32, 0x06], "inbound-open")?;
    if q.st != St::Open || q.int || q.bitpool != 53 || q.fpp != 4 || o.contains(&Out::OpenMedia) {
        return Err("inbound-open-state");
    }
    Ok(())
}

/// Reassemble queued ACL payloads by `on_acl`'s rules (bthid.rs): PB 0b10 starts a PDU whose basic header gives its
/// length, PB 0b01 continues it; a continuation with no PDU open, or a PDU longer than declared, is a failure.
fn reassemble(frags: &[(u16, Vec<u8>)]) -> Option<Vec<u8>> {
    let (mut rx, mut want) = (Vec::new(), 0usize);
    for (h, data) in frags {
        match (l2cap::acl_hdr(*h) >> 12) & 0x3 {
            0b01 if want > 0 => rx.extend_from_slice(data),
            0b10 if data.len() >= 4 => {
                rx.clear();
                rx.extend_from_slice(data);
                want = 4 + u16::from_le_bytes([data[0], data[1]]) as usize;
            }
            _ => return None,
        }
    }
    (want > 0 && rx.len() == want).then_some(rx)
}

/// Outbound L2CAP fragmentation (A2DPKNOB B750): the fragments' sizes and boundary flags, their reassembly by the
/// inbound rules byte for byte, a short PDU left whole, a small-buffer radio, and the media packet the new bound picks.
fn frag_legs() -> Result<(), &'static str> {
    let body: Vec<u8> = (0..600u32).map(|i| (i * 7 + 3) as u8).collect();
    let mut pdu = Vec::new();
    pdu.extend_from_slice(&600u16.to_le_bytes());
    pdu.extend_from_slice(&0x0041u16.to_le_bytes());
    pdu.extend_from_slice(&body);
    let f = l2cap::fragment(0x002A, 0x0041, &body, 1021);
    let sizes: Vec<usize> = f.iter().map(|x| x.1.len()).collect();
    let hdrs: Vec<u16> = f.iter().map(|x| l2cap::acl_hdr(x.0)).collect();
    if sizes != [252, 252, 100] || hdrs != [0x202A, 0x102A, 0x102A] || l2cap::frag_count(604, 1021) != 3 || !l2cap::needs_frag(604, 1021) {
        return Err("frag-split");
    }
    if reassemble(&f).as_deref() != Some(&pdu[..]) {
        return Err("frag-reassembly");
    }
    let short = l2cap::fragment(0x002A, 0x0041, &body[..10], 1021);
    let whole: Vec<u8> = [&10u16.to_le_bytes()[..], &[0x41, 0x00][..], &body[..10]].concat(); // what `l2_send` queues
    if short.len() != 1 || short[0].0 != 0x002A || short[0].1 != whole || l2cap::needs_frag(14, 1021) || l2cap::needs_frag(604, 0) {
        return Err("frag-short");
    }
    let tiny = l2cap::fragment(0x0001, 0x0041, &body, 27);
    if tiny.len() != 23 || l2cap::frag_count(604, 27) != 23 || tiny.iter().any(|x| x.1.len() > 27) || reassemble(&tiny).as_deref() != Some(&pdu[..]) {
        return Err("frag-small-buffer");
    }
    let fit = l2cap::payload_fit(1021, 672);
    let flen = Config { bitpool: 53, ..Config::A2DP_HQ }.frame_len();
    if pick_bitpool(2, 53, fit) != Some((53, 4)) || l2cap::frag_count(4 + MEDIA_HDR + 4 * flen, 1021) != 2 {
        return Err("frag-media-packet");
    }
    Ok(())
}

/// `tests a2dp`: the SBC KAT (`sbc_core::kat`, the same function the host test runs), the AVDTP state machine over a
/// synthetic peer (and the SDP record and L2CAP fit below it), outbound L2CAP fragmentation, whether a stream runs
/// now, and (SDPSERVER B769, at the END) the SDP server and the inbound acceptor ([`sdp::server_legs`], [`inbound_legs`]),
/// then (AVRCP B774, at the END) the AVRCP target over a synthetic controller ([`super::avctp::legs`]), then (AVRCPCT
/// B780, at the END) the AVRCP controller and the master watch over a synthetic headset ([`super::avctp::legs_ct`]),
/// then (AVRCPMUTE B790, at the END) the master's mute on the wire ([`super::avctp::legs_mute`]).
pub fn fixture() {
    let kat = sbc::kat();
    let av = avdtp_legs();
    let frag = frag_legs();
    let srv = sdp::server_legs();
    let inb = inbound_legs();
    let avrcp = super::avctp::legs();
    let avrcp_ct = super::avctp::legs_ct();
    let avrcp_mute = super::avctp::legs_mute();
    let stream = if streaming() { "ok" } else { "no-device" };
    for (leg, r) in [("sbc_kat", kat), ("avdtp", av), ("frag", frag), ("sdp_server", srv), ("inbound", inb), ("avrcp", avrcp), ("avrcp_ct", avrcp_ct), ("avrcp_mute", avrcp_mute)] {
        match r {
            Ok(()) => serial_println!("[a2dp] fixture {} -> PASS", leg),
            Err(why) => serial_println!("[a2dp] fixture {} -> FAIL at {}", leg, why),
        }
    }
    let ok = |r: Result<(), &'static str>| if r.is_ok() { "ok" } else { "FAIL" };
    let verdict = if kat.is_err() || av.is_err() || frag.is_err() || srv.is_err() || inb.is_err() || avrcp.is_err() || avrcp_ct.is_err() || avrcp_mute.is_err() { "FAIL" } else if stream == "ok" { "PASS" } else { "SKIP" };
    serial_println!(":: A2DP: sbc_kat={} avdtp={} frag={} stream={} sdp_server={} inbound={} avrcp={} avrcp_ct={} avrcp_mute={} -> {} ::", ok(kat), ok(av), ok(frag), stream, ok(srv), ok(inb), ok(avrcp), ok(avrcp_ct), ok(avrcp_mute), verdict);
    if verdict == "SKIP" {
        serial_println!(":: A2DP: SKIP = both fixtures passed but no A2DP stream runs (`bt audio <addr>` with a headset in pairing mode, then `tests a2dp`) ::");
    }
}
