//! CHARTER: Stria — driver
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! AUDIOMIX (rmbp-ledger B731) — THE MIXER. Until this file the audio seam had one writer: whoever took the HDA stream
//! (`drivers::hda::play`) or the USB class driver's ring (`drivers::usb::uac`) first owned the speaker, and a second
//! program's `SYS_AUDIO_OPEN` was refused `stream-held`. The mixer is the one producer of the sink:
//!
//! * **Sources** — up to [`MAX_SOURCES`], each a ring of PCM16 / 2 ch / 48 kHz frames ([`SRC_CAP`] = 200 ms) with its
//!   own gain (Q8, [`UNITY`] = 256). A source written in another format (8..48 kHz, 1|2 ch, 8|16 bit — the
//!   `SYS_AUDIO_OPEN` range) is widened and duplicated on the way in, its rate converted by ONE held windowed sinc
//!   (`audio_core::resample::Sinc`, NLE4BKERNEL B896; was nearest-sample). A ring-3 program's stream is a source keyed by its (row, slot generation) — the key and liveness
//!   compare STRIAMETAL's doors used — and a source whose owner exited is reaped on the next pump.
//! * **The period** — 10 ms ([`PERIOD_FRAMES`] = 480 frames). [`Mix::period`] takes up to one period from every
//!   source, scales each by its gain, SATURATING-adds them (each add clamps to i16), then applies the master gain
//!   (`video::status::volume()`, the F-row keys' level 0..16) and mute. A source that had been fed, has not ended and
//!   comes up short in a period is an UNDERRUN (its own count and the mixer's).
//! * **The sink pulls from the mixer only** — [`pump`] mixes a period when every live source holds one (or the sink is
//!   about to starve) and the sink has room, and hands it to the sink chosen by `audio::current()`: USB = the class
//!   driver's ring (`uac::begin` / `write` / `finish`, the one-writer seat the mixer holds while it has sources —
//!   BP-audio-4's SPSC ring unchanged); HDA = the HDA output stream, opened through `hda::play`'s existing ring-3 door
//!   (`r3_open(48000, 2, 16)` / `r3_write` / `r3_close`, the 4 x 32 KiB IOC ring armed on the device-service tick) with
//!   the mixer as its only client. The HDA codec amp already carries the volume and mute (`hda::vol::apply`), so the
//!   mixer's master is unity there; on USB (no feature-unit SET_CUR) the mixer applies it.
//! * **Who pumps** — every source write, every position read and the tone's own loop; no new task. Lock order: OUT
//!   then MIX, and neither is held across a call into `hda::play` that takes MIX (its liveness probe uses `try_lock`).
//!
//! * **SINKALL (rmbp-ledger B767)** — the kernel Player (`hda::play`: a WAV, a coded file, the video player's sound
//!   through `play-dec`) and the alert are KERNEL sources here ([`open_fmt`] / [`write`] / [`end`] / [`close`], named
//!   `player` / `video` / `alert`); a kernel source leaves by its opener's [`close`] (it reads the final [`Played`]),
//!   a ring-3 source by the reap. `hda::play`'s stream has ONE client: this file's HDA output. A third output, the
//!   A2DP stream (`OUT_BT`, cfg `a2dp`), takes each period through [`to_bt_rate`] (48 -> 44.1 kHz). The master: HDA by
//!   the codec amp, USB by the device's feature unit when it has one (`uac::volume_want`, SET_CUR on EP0), else — and
//!   always on BT (no AVRCP) — scaled here. A clip shorter than the HDA ring's prime is flushed (end of stream once
//!   every source has ended). A paused source ([`pause`]) is skipped, never an underrun.
//!
//! Wire: `[audio] mix open id= name= rate= ch= bits= gain=` · `[audio] mix sink=<usb|hda> -> ok|refused=<why>` ·
//! `[audio] close handle= pid= reason= frames= underruns=` · `[audio] source=<player|video|alert> sink=<hda|usb|bt|none>
//! frames=<n> underruns=<n>` (a kernel source's close) · `[audio] mix sources=<n> period_us=<n> underruns=<n>
//! master=<n>` · `:: AUDIOMIX: sources=2 mixed=ok underruns=0 mute=ok -> PASS ::` (`tests audiomix`, [`selftest`]).

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering}; use audio_core::resample::Sinc; // NLE4BKERNEL (rmbp-ledger B896): the mixer's rate converters (file tail). LINE-NEUTRAL append, before this comment.

/// The mixer's one format: 48 kHz, 2 x 16-bit.
pub const RATE: u32 = 48_000;
/// One period: 10 ms.
pub const PERIOD_FRAMES: usize = 480;
pub const PERIOD_US: u32 = 10_000;
/// Sources mixed at once.
pub const MAX_SOURCES: usize = 8;
/// Frames a source's ring holds (200 ms).
pub const SRC_CAP: usize = 9_600;
/// Gain 1.0 in Q8.
pub const UNITY: u16 = 256;

/// A ring-3 owner: the caller's process row, its slot generation, the arch's liveness compare, its pid for the wire.
pub struct Owner {
    pub row: usize,
    pub tgen: u64,
    pub alive: fn(usize, u64) -> bool,
    pub pid: u64,
}

pub struct Source {
    pub id: u32,
    name: &'static str,
    gain: u16,
    ring: VecDeque<[i16; 2]>,
    rate: u32,
    ch: u8,
    bits: u8,
    /// Source-format frames accepted / 48 kHz frames produced from them (the resampler's two clocks).
    in_n: u64,
    out_n: u64,
    /// 48 kHz frames mixed out of the ring, and the mixer's emitted-frame index after the last of them.
    taken: u64,
    last_e: u64,
    primed: bool,
    ended: bool,
    /// SINKALL: held out of the mix (the Player's pause): no frame taken, no underrun counted.
    paused: bool,
    underruns: u32,
    owner: Option<Owner>, rs: Option<Sinc>, // NLE4BKERNEL (rmbp-ledger B896): a non-48 kHz source's ONE converter, built before MIX by `ingest_sinc` (file tail)
}

/// SINKALL: a source's account: source frames written and played (capped), its underruns, and drained = ended with
/// every frame played by the sink.
#[derive(Clone, Copy, Default, Debug)]
pub struct Played {
    pub written: u64,
    pub played: u64,
    pub underruns: u32,
    pub drained: bool,
}

impl Source {
    fn live(&self) -> bool {
        self.primed && !self.ended && !self.paused
    }
    /// 48 kHz frames of this source the sink has played, given the sink's play position in emitted frames.
    fn played48(&self, sink_played: u64) -> u64 {
        self.taken.saturating_sub(self.last_e.saturating_sub(sink_played))
    }
    /// The same, in source frames, capped at the frames written (a padded underrun is never counted as played).
    fn played_src(&self, sink_played: u64) -> u64 {
        (self.played48(sink_played) * self.rate as u64 / RATE as u64).min(self.in_n)
    }
    /// The account at the sink's play position.
    fn account(&self, sink_played: u64) -> Played {
        let drained = self.ended && self.ring.is_empty() && self.played48(sink_played) >= self.taken;
        Played { written: self.in_n, played: self.played_src(sink_played), underruns: self.underruns, drained }
    }
}

pub struct Mix {
    pub srcs: Vec<Source>,
    /// Frames mixed out since boot (the emitted-frame index), periods mixed, underruns over every source.
    pub emitted: u64,
    pub periods: u64,
    pub underruns: u32,
    next_id: u32,
}

/// The saturating add: `acc + s * gain`, clamped to i16 at every add.
#[inline]
pub fn mix_add(acc: i32, s: i16, gain: u16) -> i32 {
    (acc + ((s as i32 * gain as i32) >> 8)).clamp(-32768, 32767)
}

/// The master stage: `acc * master` (Q8), or silence when muted.
#[inline]
pub fn master_out(acc: i32, master: u16, mute: bool) -> i16 {
    if mute { 0 } else { ((acc * master as i32) >> 8).clamp(-32768, 32767) as i16 }
}

impl Mix {
    pub const fn new() -> Mix {
        Mix { srcs: Vec::new(), emitted: 0, periods: 0, underruns: 0, next_id: 0 }
    }

    /// A new source of `rate` / `ch` / `bits` at `gain`: its id, or (errno, refusal word).
    pub fn open(&mut self, name: &'static str, rate: u32, ch: u8, bits: u8, gain: u16, owner: Option<Owner>) -> Result<u32, (i64, &'static str)> {
        if !(una_abi::AUDIO_RATE_MIN..=una_abi::AUDIO_RATE_MAX).contains(&rate) || !(ch == 1 || ch == 2) || !(bits == 8 || bits == 16) {
            return Err((una_abi::EINVAL, "format"));
        }
        if let Some(o) = owner.as_ref() {
            if self.srcs.iter().any(|s| s.owner.as_ref().is_some_and(|p| p.row == o.row && p.tgen == o.tgen)) {
                return Err((una_abi::EEXIST, "already-open"));
            }
        }
        if self.srcs.len() >= MAX_SOURCES {
            return Err((una_abi::EBUSY, "mixer-full"));
        }
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = self.next_id;
        self.srcs.push(Source {
            id, name, gain, ring: VecDeque::new(), rate, ch, bits, in_n: 0, out_n: 0, taken: 0, last_e: self.emitted,
            primed: false, ended: false, paused: false, underruns: 0, owner, rs: None,
        });
        Ok(id)
    }

    fn at(&mut self, id: u32) -> Option<&mut Source> {
        self.srcs.iter_mut().find(|s| s.id == id)
    }

    /// Whole source-format frames of `data` into source `id`'s ring: the bytes taken (0 = the ring is full).
    pub fn write(&mut self, id: u32, data: &[u8]) -> Option<usize> {
        let s = self.at(id)?;
        let fin = s.ch as usize * s.bits as usize / 8;
        let (src, eff) = (s.rate as u64, RATE as u64);
        let mut i = 0;
        while i + fin <= data.len() && (s.rate == RATE || i < INGEST_MAX * fin) { // NLE4BKERNEL (B896): a converting source takes at most INGEST_MAX frames per call (the soft-float work under MIX)
            // the ring room this source frame needs: the 48 kHz frames at or before it (out k sits at source frame k * src / eff)
            let gi = s.in_n;
            let mut k = 0usize;
            while (s.out_n + k as u64) * src / eff <= gi {
                k += 1;
            }
            if s.ring.len() + k > SRC_CAP {
                break;
            }
            let f = &data[i..i + fin];
            let (l, r) = match (s.bits, s.ch) {
                (8, 1) => { let v = ((f[0] as i16) - 128) << 8; (v, v) }
                (8, _) => (((f[0] as i16) - 128) << 8, ((f[1] as i16) - 128) << 8),
                (_, 1) => { let v = i16::from_le_bytes([f[0], f[1]]); (v, v) }
                _ => (i16::from_le_bytes([f[0], f[1]]), i16::from_le_bytes([f[2], f[3]])),
            };
            match s.rate {
                RATE => { for _ in 0..k { s.ring.push_back([l, r]); } s.out_n += k as u64; } // 48 kHz: k == 1, the integer copy
                _ => s.out_n += sinc_push(&mut s.rs, &mut s.ring, s.rate, s.ch, l, r), // NLE4BKERNEL (rmbp-ledger B896): the source's ONE Sinc (file tail)
            }
            s.in_n += 1;
            i += fin;
        }
        if i > 0 {
            s.primed = true;
        }
        Some(i)
    }

    /// Source `id` has no more frames: what it holds still plays.
    pub fn end(&mut self, id: u32) -> bool {
        self.at(id).map(|s| { if !s.ended { ingest_flush(s); } s.ended = true }).is_some() // NLE4BKERNEL (B896): the converter's lookahead into the ring (file tail)
    }

    pub fn set_gain(&mut self, id: u32, gain: u16) -> bool {
        self.at(id).map(|s| s.gain = gain).is_some()
    }

    /// SINKALL: hold source `id` out of the mix (`on`) or put it back.
    pub fn set_paused(&mut self, id: u32, on: bool) -> bool {
        self.at(id).map(|s| s.paused = on).is_some()
    }

    pub fn remove(&mut self, id: u32) -> Option<Source> {
        let i = self.srcs.iter().position(|s| s.id == id)?;
        Some(self.srcs.remove(i))
    }

    /// Some source holds a frame.
    pub fn has_data(&self) -> bool {
        self.srcs.iter().any(|s| !s.paused && !s.ring.is_empty())
    }

    /// Every live source holds a whole period (mixing now would underrun nobody).
    pub fn ready(&self) -> bool {
        self.has_data() && self.srcs.iter().all(|s| !s.live() || s.ring.len() >= PERIOD_FRAMES)
    }

    /// Mix one period into `out` (interleaved L/R, `out.len() / 2` frames): each source's frames x its gain, the
    /// saturating add, then the master and mute. A live source short of the period counts an underrun.
    pub fn period(&mut self, out: &mut [i16], master: u16, mute: bool) {
        let n = out.len() / 2;
        let mut acc: Vec<[i32; 2]> = alloc::vec![[0i32; 2]; n];
        let e = self.emitted + n as u64;
        for s in self.srcs.iter_mut() {
            if s.paused {
                continue;
            }
            let have = s.ring.len().min(n);
            if have < n && s.live() {
                s.underruns += 1;
                self.underruns += 1;
            }
            for a in acc.iter_mut().take(have) {
                let [l, r] = s.ring.pop_front().unwrap_or([0, 0]);
                a[0] = mix_add(a[0], l, s.gain);
                a[1] = mix_add(a[1], r, s.gain);
            }
            if have > 0 {
                s.taken += have as u64;
                s.last_e = self.emitted + have as u64;
            }
        }
        for (i, a) in acc.iter().enumerate() {
            out[2 * i] = master_out(a[0], master, mute);
            out[2 * i + 1] = master_out(a[1], master, mute);
        }
        self.emitted = e;
        self.periods += 1;
    }
}

// ── The live mixer ────────────────────────────────────────────────────────────────────────────────────

static MIX: crate::sync::Mutex<Mix> = crate::sync::Mutex::new(Mix::new());

const OUT_NONE: u8 = 0;
const OUT_USB: u8 = 1;
const OUT_HDA: u8 = 2;
/// SINKALL: the A2DP stream (`drivers/bt/avdtp.rs`'s PCM ring, 44.1 kHz).
const OUT_BT: u8 = 3;
/// The A2DP stream's rate (`avdtp::RATE`), named here so the synthetic leg builds without `a2dp`.
const BT_RATE: u32 = 44_100;

#[cfg(all(target_arch = "x86_64", feature = "a2dp"))]
use crate::drivers::ehci::bthid::avdtp as a2dp;

/// SINKALL witnesses: the output kind now (`Out.kind`'s mirror, read without OUT), frames handed to each output kind,
/// and the mixer's acquisitions of the HDA door (the test's direct-write count compares them with `hda::play`'s epoch).
static OUT_KIND: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(OUT_NONE);
static DELIVERED: [AtomicU64; 4] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
static HDA_OPENS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

struct Out {
    kind: u8,
    /// The HDA stream's handle (hda::play's ring-3 door), and whether its end-of-stream has been sent.
    hda: u32,
    draining: bool,
    /// Frames handed to the current output, and the mixed period not yet taken by it.
    delivered: u64,
    pend: Vec<u8>,
    pend_off: usize,
    said: &'static str,
    /// NLE4BKERNEL (rmbp-ledger B896): the BT output's ONE rate converter (48 -> 44.1 kHz), built once per boot before OUT.
    rs: Option<Sinc>,
}
static OUT: crate::sync::Mutex<Out> =
    crate::sync::Mutex::new(Out { kind: OUT_NONE, hda: 0, draining: false, delivered: 0, pend: Vec::new(), pend_off: 0, said: "", rs: None });
/// The sink's play position in emitted frames (the A/V clock's base), refreshed by every pump.
static SINK_PLAYED: AtomicU64 = AtomicU64::new(0);

/// The HDA door's key for the mixer (never a process row).
const MIX_ROW: usize = usize::MAX;
const MIX_GEN: u64 = u64::MAX;

/// The master stage the sink needs: (gain Q8, mute, level 0..16 for the wire). HDA's codec amp carries the level; a USB
/// device's feature unit carries it when it has one (SINKALL M2); otherwise — and always on BT — the mixer scales.
fn master(kind: u8) -> (u16, bool, u8) {
    let (lv, muted) = status_volume(); #[cfg(all(target_arch = "x86_64", feature = "a2dp"))] if kind == OUT_BT && bt_hw_volume() { return (UNITY, muted, lv); }
    if kind == OUT_HDA || (kind == OUT_USB && usb_hw_volume(lv, muted)) { (UNITY, false, lv) } else { (lv.min(16) as u16 * 16, muted, lv) }
}

/// SINKALL M2: hand the level to the USB device's feature unit; true when it carries it (the mixer is then unity).
#[cfg(feature = "uac")]
fn usb_hw_volume(lv: u8, muted: bool) -> bool {
    crate::drivers::usb::uac::volume_want(lv, muted)
}

#[cfg(not(feature = "uac"))]
fn usb_hw_volume(_: u8, _: bool) -> bool {
    false
}

fn kind_name(kind: u8) -> &'static str {
    match kind {
        OUT_USB => "usb",
        OUT_HDA => "hda",
        OUT_BT => "bt",
        _ => "none",
    }
}

/// The sink `audio::current()` names, as an output kind this image can drive.
fn wanted() -> u8 {
    match super::current() {
        super::Sink::Usb if cfg!(feature = "uvc") => OUT_USB,
        super::Sink::Hda if super::hda_built() => OUT_HDA,
        #[cfg(all(target_arch = "x86_64", feature = "a2dp"))]
        super::Sink::Bt => OUT_BT,
        _ => OUT_NONE,
    }
}

fn say(o: &mut Out, what: &'static str, line: &str) {
    if o.said != what {
        o.said = what;
        serial_println!("{}", line);
    }
}

/// The mixer's liveness answer to hda::play's tick: false only when every source belongs to an exited program.
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn mix_alive(_: usize, _: u64) -> bool {
    match MIX.try_lock() {
        Some(m) => m.srcs.is_empty() || m.srcs.iter().any(|s| s.owner.as_ref().map_or(true, |o| (o.alive)(o.row, o.tgen))),
        None => true,
    }
}

fn release(o: &mut Out) {
    match o.kind {
        #[cfg(feature = "uac")]
        OUT_USB => crate::drivers::usb::uac::finish(),
        #[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
        OUT_HDA => {
            let _ = crate::drivers::hda::play::r3_close(o.hda as u64, MIX_ROW, MIX_GEN);
        }
        #[cfg(all(target_arch = "x86_64", feature = "a2dp"))]
        OUT_BT => { bt_drain(o); a2dp::finish(); } // NLE4BKERNEL (B896): the converter's lookahead plays before the end (file tail)
        _ => {}
    }
    o.kind = OUT_NONE;
    OUT_KIND.store(OUT_NONE, Ordering::Release);
    o.draining = false;
}

fn acquire(o: &mut Out, kind: u8) -> bool {
    let r: Result<u32, &'static str> = match kind {
        #[cfg(feature = "uac")]
        OUT_USB => crate::drivers::usb::uac::begin().map(|_| 0),
        #[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
        OUT_HDA => crate::drivers::hda::play::r3_open(MIX_ROW, MIX_GEN, mix_alive, 0, RATE as u64, 2, 16).map_err(|(_, why)| why),
        #[cfg(all(target_arch = "x86_64", feature = "a2dp"))]
        OUT_BT => a2dp::begin().map(|_| 0),
        _ => Err("no-sink"),
    };
    let name = kind_name(kind);
    match r {
        Ok(h) => {
            o.kind = kind;
            OUT_KIND.store(kind, Ordering::Release);
            if kind == OUT_HDA {
                HDA_OPENS.fetch_add(1, Ordering::AcqRel);
            }
            o.hda = h;
            o.draining = false;
            o.delivered = 0;
            if let Some(r) = o.rs.as_mut() { r.reset(); } // NLE4BKERNEL (B896): a new stream starts the converter from silence
            o.said = "";
            serial_println!("[audio] mix sink={} -> ok", name);
            true
        }
        Err(why) => {
            say(o, why, &alloc::format!("[audio] mix sink={} -> refused={}", name, why));
            false
        }
    }
}

/// Frames the output holds that it has not played (the pending period included).
fn queued(o: &Out) -> u64 {
    let pend = ((o.pend.len() - o.pend_off) / 4) as u64;
    let held = match o.kind {
        #[cfg(feature = "uac")]
        OUT_USB => (crate::drivers::usb::uac::queued() / 4) as u64,
        #[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
        OUT_HDA => {
            let p = crate::drivers::hda::play::r3_pos(o.hda as u64, MIX_ROW, MIX_GEN, una_abi::AUDIO_POS_PLAYED);
            o.delivered.saturating_sub(p.max(0) as u64)
        }
        #[cfg(all(target_arch = "x86_64", feature = "a2dp"))]
        OUT_BT => a2dp::queued() as u64 * RATE as u64 / BT_RATE as u64,
        _ => 0,
    };
    pend + held
}

/// (starve, high) water in frames: mix while below high; below starve a short source is padded (an underrun). The
/// HDA stream runs only once its ring is primed, so its high is the stream's own number, the prime plus one entry
/// (SINKALLDOCS: `hda::play::HIGH_FRAMES`, 10 752 frames = 224 ms — a Player pause on HDA lands within it).
fn marks(kind: u8) -> (u64, u64) {
    if kind == OUT_HDA { (2 * PERIOD_FRAMES as u64, HDA_HIGH) } else { (2 * PERIOD_FRAMES as u64, 10 * PERIOD_FRAMES as u64) }
}

/// The HDA output's high water: the stream's (`hda::play` owns the ring); no HDA output without it.
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
const HDA_HIGH: u64 = crate::drivers::hda::play::HIGH_FRAMES;
#[cfg(not(all(target_arch = "x86_64", feature = "hda-tone")))]
const HDA_HIGH: u64 = 0;

/// Hand the pending period to the output: true when it was all taken.
fn deliver(o: &mut Out) -> bool {
    while o.pend_off < o.pend.len() {
        let took: i64 = match o.kind {
            #[cfg(feature = "uac")]
            OUT_USB => crate::drivers::usb::uac::write(&o.pend[o.pend_off..]) as i64,
            #[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
            OUT_HDA => crate::drivers::hda::play::r3_write(o.hda as u64, MIX_ROW, MIX_GEN, &o.pend[o.pend_off..]),
            #[cfg(all(target_arch = "x86_64", feature = "a2dp"))]
            OUT_BT => a2dp::write(&o.pend[o.pend_off..]) as i64,
            _ => 0,
        };
        if took > 0 {
            o.pend_off += took as usize;
            o.delivered += took as u64 / 4;
            DELIVERED[o.kind as usize & 3].fetch_add(took as u64 / 4, Ordering::Relaxed);
        } else {
            if took != una_abi::EAGAIN && took != 0 {
                // the kernel player took the HDA stream (its `start` bumped the epoch): the handle is gone
                serial_println!("[audio] mix sink=hda lost errno={} (superseded)", took);
                release(o);
            }
            return false;
        }
    }
    o.pend.clear();
    o.pend_off = 0;
    true
}

/// Drop ring-3 sources whose owner exited and ended ring-3 sources the sink has played out. A KERNEL source (no owner)
/// leaves by its opener's [`close`], which reads its final [`Played`] (SINKALL).
fn reap(m: &mut Mix, sink_played: u64) {
    let mut i = 0;
    while i < m.srcs.len() {
        let s = &m.srcs[i];
        let dead = s.owner.as_ref().is_some_and(|o| !(o.alive)(o.row, o.tgen));
        let done = s.owner.is_some() && s.ended && s.ring.is_empty() && s.played48(sink_played) >= s.taken;
        if dead || done {
            let s = m.srcs.remove(i);
            closed(&s, if dead { "owner-exited" } else { "drained" }, sink_played, m);
        } else {
            i += 1;
        }
    }
}

fn closed(s: &Source, reason: &str, sink_played: u64, m: &Mix) {
    match s.owner.as_ref() {
        Some(o) => serial_println!("[audio] close handle={} pid={} name={} reason={} frames={} underruns={}", s.id, o.pid, s.name, reason, s.played_src(sink_played), s.underruns),
        None => serial_println!("[audio] source={} sink={} frames={} underruns={}", s.name, kind_name(OUT_KIND.load(Ordering::Acquire)), s.played_src(sink_played), s.underruns),
    }
    status(m, OUT_NONE);
}

fn status(m: &Mix, kind: u8) {
    let (_, _, lv) = master(kind);
    serial_println!("[audio] mix sources={} period_us={} underruns={} master={}", m.srcs.len(), PERIOD_US, m.underruns, lv);
}

/// THE PUMP: bring the output the current sink names up or down, and move mixed periods into it while it has room.
pub fn pump() {
    let bt = bt_sinc_due(); let Some(mut o) = OUT.try_lock() else { return }; // NLE4BKERNEL (rmbp-ledger B896): the BT converter's table is built BEFORE OUT (file tail). LINE-NEUTRAL append, before this comment.
    let o = &mut *o; bt_sinc_keep(o, bt);
    let mut buf = [0i16; PERIOD_FRAMES * 2];
    for _ in 0..64 {
        let q = queued(o);
        {
            let mut m = MIX.lock();
            let p = m.emitted.saturating_sub(q);
            SINK_PLAYED.store(p, Ordering::Release);
            reap(&mut *m, p);
            // SINKALL: nothing more will come once every source has ENDED and holds nothing (not only when none is
            // open): HDA gets its end of stream then, so a clip shorter than the ring's prime still runs and drains.
            let quiet = m.srcs.iter().all(|s| s.ended && s.ring.is_empty());
            if quiet && o.pend_off >= o.pend.len() {
                let empty = m.srcs.is_empty();
                drop(m);
                match o.kind {
                    #[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
                    OUT_HDA if !o.draining => {
                        let _ = crate::drivers::hda::play::r3_write(o.hda as u64, MIX_ROW, MIX_GEN, &[]); // end of stream: the ring drains, then stops
                        o.draining = true;
                    }
                    OUT_HDA => {}
                    _ if empty => release(o),
                    _ => {}
                }
                return;
            }
        }
        let want = wanted();
        if o.kind != want || (o.kind == OUT_HDA && o.draining) {
            release(o);
        }
        if o.kind == OUT_NONE && (want == OUT_NONE || !acquire(o, want)) {
            return;
        }
        if o.pend_off < o.pend.len() && !deliver(o) {
            return;
        }
        let (lo, hi) = marks(o.kind);
        let q = queued(o);
        let (gain, mute, _) = master(o.kind);
        {
            let mut m = MIX.lock();
            // a short source pads silence only when the output is already playing and about to starve
            if !m.has_data() || q >= hi || !(m.ready() || (q < lo && o.delivered > 0)) {
                return;
            }
            m.period(&mut buf, gain, mute);
        }
        fill_pend(o, &buf);
        if !deliver(o) {
            return;
        }
    }
}

// ── Kernel sources ────────────────────────────────────────────────────────────────────────────────────

/// The mixed period into the output's pending bytes: as is (48 kHz), or through [`to_bt_rate`] for the A2DP stream.
fn fill_pend(o: &mut Out, buf: &[i16]) {
    o.pend.clear();
    o.pend_off = 0;
    if o.kind == OUT_BT {
        if let Some(r) = o.rs.as_mut() { to_bt_rate(r, buf, &mut o.pend); } // NLE4BKERNEL (B896): None only when BT became the sink after this pump's prologue: that period is dropped
    } else {
        for v in buf.iter() {
            o.pend.extend_from_slice(&v.to_le_bytes());
        }
    }
}

/// SINKALL: interleaved 48 kHz stereo frames to the A2DP stream's 44.1 kHz through the output's ONE held `Sinc` (NLE4BKERNEL
/// B896; was nearest-sample), streaming across periods; its lookahead is flushed at `release` ([`bt_drain`]).
pub fn to_bt_rate(rs: &mut Sinc, period: &[i16], out: &mut Vec<u8>) {
    let (mut input, mut buf) = (period, [0i16; 2 * 512]);
    loop {
        let (_, made) = rs.process_i16(input, &mut buf);
        input = &[];
        for v in &buf[..made * 2] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if made < 512 { break; }
    }
}

/// A kernel source (48 kHz / 2 / 16) at `gain`.
pub fn open(name: &'static str, gain: u16) -> Result<u32, &'static str> {
    open_fmt(name, RATE, 2, 16, gain)
}

/// SINKALL: a kernel source in any `SYS_AUDIO_OPEN` format (8..48 kHz, 1|2 ch, 8|16 bit; widened and resampled on the
/// way in): the Player (`player` / `video`) and the alert open here. Its opener [`close`]s it.
pub fn open_fmt(name: &'static str, rate: u32, ch: u8, bits: u8, gain: u16) -> Result<u32, &'static str> {
    let r = {
        let rs = ingest_sinc(rate, ch); let mut m = MIX.lock(); // NLE4BKERNEL (B896): a non-48 kHz source's table is built BEFORE MIX (file tail)
        let r = m.open(name, rate, ch, bits, gain, None); m.keep_sinc(&r, rs);
        if r.is_ok() {
            status(&m, wanted());
        }
        r
    };
    if let Ok(id) = r {
        serial_println!("[audio] mix open id={} name={} rate={} ch={} bits={} gain={}", id, name, rate, ch, bits, gain);
    }
    r.map_err(|(_, why)| why)
}

/// SINKALL: kernel source `id`'s account now (after a pump), None once it is closed.
pub fn state(id: u32) -> Option<Played> {
    pump();
    let m = MIX.lock();
    let sp = SINK_PLAYED.load(Ordering::Acquire);
    m.srcs.iter().find(|s| s.id == id).map(|s| s.account(sp))
}

/// SINKALL: source frames of `id` the sink has played — the A/V clock. Paint-context safe: `try_lock`, never a pump;
/// None when the mixer is busy this instant or `id` is closed.
pub fn played_now(id: u32) -> Option<u64> {
    let m = MIX.try_lock()?;
    let sp = SINK_PLAYED.load(Ordering::Acquire); #[cfg(all(target_arch = "x86_64", feature = "a2dp"))] let sp = sp.saturating_sub(bt_delay()); // A2DPKAT (rmbp-ledger B870): on BT the headset's reported delay comes off the A/V clock (file tail). LINE-NEUTRAL append, before this comment.
    m.srcs.iter().find(|s| s.id == id).map(|s| s.played_src(sp))
}

/// SINKALL: the opener's close of kernel source `id` (its unmixed frames dropped): its final account.
pub fn close(id: u32) -> Option<Played> {
    let r = {
        let mut m = MIX.lock();
        let sp = SINK_PLAYED.load(Ordering::Acquire);
        let s = m.remove(id)?;
        let p = s.account(sp);
        closed(&s, "close", sp, &m);
        p
    };
    pump();
    Some(r)
}

/// SINKALL: hold source `id` out of the mix (the Player's pause) or put it back.
pub fn pause(id: u32, on: bool) -> bool {
    let r = MIX.lock().set_paused(id, on);
    pump();
    r
}

/// SINKALL: the output the mixer drives now (`hda` / `usb` / `bt`), None when none.
pub fn output() -> Option<&'static str> {
    match OUT_KIND.load(Ordering::Acquire) {
        OUT_NONE => None,
        k => Some(kind_name(k)),
    }
}

/// SINKALL: frames handed to the HDA, USB and BT outputs since boot (BT in its 44.1 kHz frames).
pub fn delivered() -> [u64; 3] {
    [DELIVERED[OUT_HDA as usize].load(Ordering::Relaxed), DELIVERED[OUT_USB as usize].load(Ordering::Relaxed), DELIVERED[OUT_BT as usize].load(Ordering::Relaxed)]
}

/// SINKALL: the mixer's acquisitions of the HDA stream since boot.
pub fn hda_opens() -> u32 {
    HDA_OPENS.load(Ordering::Acquire)
}

/// PCM16/2/48k bytes into kernel source `id`, then a pump: the bytes taken (0 = full; retry).
pub fn write(id: u32, data: &[u8]) -> usize {
    let n = MIX.lock().write(id, data).unwrap_or(0);
    pump();
    n
}

/// Kernel source `id` has no more frames (what it holds plays out, then it is reaped).
pub fn end(id: u32) {
    MIX.lock().end(id);
    pump();
}

pub fn set_gain(id: u32, gain: u16) -> bool {
    MIX.lock().set_gain(id, gain)
}

/// Sources open now.
pub fn sources() -> usize {
    MIX.lock().srcs.len()
}

// ── Ring 3: una-abi's SYS_AUDIO_* land here (x86 syscall.rs STRIAMETAL arms) ──────────────────────────

fn mine(m: &Mix, handle: u64, row: usize, tgen: u64) -> Option<usize> {
    m.srcs.iter().position(|s| s.id as u64 == handle && s.owner.as_ref().is_some_and(|o| o.row == row && o.tgen == tgen))
}

/// `SYS_AUDIO_OPEN`: a source for the caller `(row, tgen)`; the handle, or (errno, refusal word).
pub fn r3_open(row: usize, tgen: u64, alive: fn(usize, u64) -> bool, pid: u64, rate: u64, ch: u64, bits: u64) -> Result<u32, (i64, &'static str)> {
    if rate > u32::MAX as u64 || ch > 2 || bits > 16 {
        return Err((una_abi::EINVAL, "format"));
    }
    pump(); // reap a stale stream of an exited owner before the duplicate check
    let r = {
        let rs = ingest_sinc(rate as u32, ch as u8); let mut m = MIX.lock(); // NLE4BKERNEL (B896): the table is built BEFORE MIX (file tail)
        let r = m.open("ring3", rate as u32, ch as u8, bits as u8, UNITY, Some(Owner { row, tgen, alive, pid })); m.keep_sinc(&r, rs);
        if r.is_ok() {
            status(&m, wanted());
        }
        r
    };
    if let Ok(id) = r {
        serial_println!("[audio] mix open id={} name=ring3 pid={} rate={} ch={} bits={} gain={}", id, pid, rate, ch, bits, UNITY);
    }
    r
}

/// `SYS_AUDIO_WRITE`: whole frames into the caller's source (bytes taken), -EAGAIN while it is full, -EBADF; empty
/// `data` = end of stream.
pub fn r3_write(handle: u64, row: usize, tgen: u64, data: &[u8]) -> i64 {
    let r = {
        let mut m = MIX.lock();
        match mine(&m, handle, row, tgen) {
            None => una_abi::EBADF,
            Some(i) => {
                let id = m.srcs[i].id;
                if data.is_empty() {
                    m.end(id);
                    0
                } else if m.srcs[i].ended {
                    una_abi::EIO
                } else {
                    let fin = m.srcs[i].ch as usize * m.srcs[i].bits as usize / 8;
                    if data.len() < fin {
                        una_abi::EINVAL
                    } else {
                        m.write(id, data).map_or(una_abi::EBADF, |n| if n == 0 { una_abi::EAGAIN } else { n as i64 })
                    }
                }
            }
        }
    };
    pump();
    if r == una_abi::EAGAIN {
        // the pump may have drained the ring: one more try before the caller sleeps
        let mut m = MIX.lock();
        if let Some(i) = mine(&m, handle, row, tgen) {
            let id = m.srcs[i].id;
            if let Some(n) = m.write(id, data) {
                if n > 0 {
                    return n as i64;
                }
            }
        }
    }
    r
}

/// `SYS_AUDIO_CLOSE`: the caller's source leaves now (its unmixed frames are dropped). 0, or -EBADF.
pub fn r3_close(handle: u64, row: usize, tgen: u64) -> i64 {
    {
        let mut m = MIX.lock();
        let Some(i) = mine(&m, handle, row, tgen) else { return una_abi::EBADF };
        let s = m.srcs.remove(i);
        closed(&s, "close", SINK_PLAYED.load(Ordering::Acquire), &m);
    }
    pump();
    0
}

/// `SYS_AUDIO_POS`: AUDIO_POS_PLAYED = the caller's source frames the sink has played (capped at those written);
/// AUDIO_POS_UNDERRUNS = its underruns. -EBADF, -EINVAL.
pub fn r3_pos(handle: u64, row: usize, tgen: u64, what: u64) -> i64 {
    if what != una_abi::AUDIO_POS_PLAYED && what != una_abi::AUDIO_POS_UNDERRUNS {
        return una_abi::EINVAL;
    }
    pump();
    let m = MIX.lock();
    let Some(i) = mine(&m, handle, row, tgen) else { return una_abi::EBADF };
    let s = &m.srcs[i];
    if what == una_abi::AUDIO_POS_PLAYED { s.played_src(SINK_PLAYED.load(Ordering::Acquire)) as i64 } else { s.underruns as i64 }
}

// ── `tests audiomix` ──────────────────────────────────────────────────────────────────────────────────

/// Sign changes of a mono signal.
fn crossings(x: &[i16]) -> u32 {
    let mut n = 0;
    let mut prev = 0i16;
    for &v in x {
        if v != 0 {
            if prev != 0 && (v > 0) != (prev > 0) {
                n += 1;
            }
            prev = v;
        }
    }
    n
}

fn peak(x: &[i16]) -> i32 {
    x.iter().map(|&v| (v as i32).abs()).max().unwrap_or(0)
}

/// `hz` at `amp` as PCM16/2 bytes for frames `f0..f0 + n` (the seam's integer sine).
fn tone_bytes(hz: u32, amp: i32, f0: usize, n: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(n * 4);
    for fr in f0..f0 + n {
        let s = ((super::sin_q15(fr as u64 * hz as u64 * 1024 / RATE as u64) * amp) >> 15) as i16;
        let b = s.to_le_bytes();
        v.extend_from_slice(&[b[0], b[1], b[0], b[1]]);
    }
    v
}

/// One second through an offline [`Mix`] (the live sink is never touched): sources `(hz, gain)`, the master and mute.
/// Returns (the left channel the sink would have read, the mixer's underruns, sources open, exact = every sample equals
/// the mix rule recomputed from the inputs).
fn run(tones: &[(u32, u16)], master_q8: u16, mute: bool) -> (Vec<i16>, u32, usize, bool) {
    const AMP: i32 = 8192;
    let mut m = Mix::new();
    let ids: Vec<u32> = tones.iter().filter_map(|&(_, g)| m.open("test", RATE, 2, 16, g, None).ok()).collect();
    let mut left = Vec::with_capacity(RATE as usize);
    let mut out = [0i16; PERIOD_FRAMES * 2];
    let mut exact = ids.len() == tones.len();
    for p in 0..(RATE as usize / PERIOD_FRAMES) {
        let f0 = p * PERIOD_FRAMES;
        let ins: Vec<Vec<u8>> = tones.iter().map(|&(hz, _)| tone_bytes(hz, AMP, f0, PERIOD_FRAMES)).collect();
        for (id, b) in ids.iter().zip(ins.iter()) {
            exact &= m.write(*id, b) == Some(b.len());
        }
        m.period(&mut out, master_q8, mute);
        for i in 0..PERIOD_FRAMES {
            let mut acc = 0i32;
            for (k, &(_, g)) in tones.iter().enumerate() {
                acc = mix_add(acc, i16::from_le_bytes([ins[k][4 * i], ins[k][4 * i + 1]]), g);
            }
            let want = if mute { 0 } else { ((acc * master_q8 as i32) >> 8).clamp(-32768, 32767) as i16 };
            exact &= out[2 * i] == want && out[2 * i + 1] == want;
            left.push(out[2 * i]);
        }
    }
    (left, m.underruns, m.srcs.len(), exact)
}

/// `tests audiomix`: a 440 and a 660 Hz source for 1 s each alone and together; the sink's period stream read back.
/// Both present = the sum's zero crossings sit strictly between the two tones' own (880 and 1320 a second) and its
/// peak exceeds 1.5x one tone's; gain = a half-gain 440 peaks at half; master = level 8 halves it; mute = silence.
pub fn selftest() {
    let (a, ua, _, xa) = run(&[(440, UNITY)], UNITY, false);
    let (b, ub, _, xb) = run(&[(660, UNITY)], UNITY, false);
    let (ab, uab, n, xab) = run(&[(440, UNITY), (660, UNITY)], UNITY, false);
    let (half, uh, _, xh) = run(&[(440, UNITY / 2), (660, 0)], UNITY, false);
    let (lv8, u8_, _, x8) = run(&[(440, UNITY), (660, 0)], 8 * 16, false);
    let (muted, um, _, xm) = run(&[(440, UNITY), (660, UNITY)], UNITY, true);
    let (za, zb, zab) = (crossings(&a), crossings(&b), crossings(&ab));
    let (pa, pab, ph, p8) = (peak(&a), peak(&ab), peak(&half), peak(&lv8));
    let near = |x: i32, want: i32| (x - want).abs() <= want / 50 + 2;
    let freq_ok = za.abs_diff(880) <= 4 && zb.abs_diff(1320) <= 4 && zab > za + 4 && zab + 4 < zb;
    let both_ok = freq_ok && pab * 2 > pa * 3;
    let gain_ok = near(ph, pa / 2) && near(p8, pa / 2);
    let exact = xa && xb && xab && xh && x8 && xm;
    let mixed = both_ok && gain_ok && exact;
    let mute_ok = muted.iter().all(|&v| v == 0) && xm;
    let under = ua + ub + uab + uh + u8_ + um;
    serial_println!("[audio] mix test zc_440={} zc_660={} zc_mix={} peak_440={} peak_mix={} peak_gain_half={} peak_master8={} exact={}", za, zb, zab, pa, pab, ph, p8, exact as u8);
    serial_println!("[audio] mix sources={} period_us={} underruns={} master={}", n, PERIOD_US, under, 16);
    let pass = n == 2 && mixed && mute_ok && under == 0;
    let ok = |b: bool| if b { "ok" } else { "fail" };
    serial_println!(":: AUDIOMIX: sources={} mixed={} underruns={} mute={} -> {} ::", n, ok(mixed), under, ok(mute_ok), if pass { "PASS" } else { "FAIL" });
}

// ── SINKALL (rmbp-ledger B767): `tests sinkall`'s synthetic Bluetooth leg ─────────────────────────────────────────────

/// The BT output's path offline (QEMU has no Bluetooth; the live sink untouched): `pcm` (48 kHz mono 16-bit, the alert)
/// as ONE source of an offline [`Mix`], mixed at master level 8 (the mixer's own scale: BT has no AVRCP volume) and
/// taken through [`to_bt_rate`]. Returns (sources while open, sources after the close, 48 kHz frames mixed, 44.1 kHz
/// frames out, ok = the frame count is 44.1/48 of the mix within one and the peak is half the input's).
pub fn bt_synthetic(pcm: &[u8]) -> (usize, usize, u64, u64, bool) {
    let mut m = Mix::new();
    let Ok(id) = m.open("alert", RATE, 1, 16, UNITY, None) else { return (0, 0, 0, 0, false) };
    let open = m.srcs.len();
    let took = m.write(id, pcm).unwrap_or(0);
    m.end(id);
    let (mut rs, mut out, mut buf) = (Sinc::new(RATE, BT_RATE, 2), Vec::new(), [0i16; PERIOD_FRAMES * 2]);
    while m.has_data() {
        m.period(&mut buf, 8 * 16, false);
        to_bt_rate(&mut rs, &buf, &mut out);
    }
    bt_flush(&mut rs, &mut out); m.remove(id); // NLE4BKERNEL (B896): the end of the stream, as `release` flushes the live one
    let after = m.srcs.len();
    let fin = m.emitted;
    let fout = (out.len() / 4) as u64;
    let want = fin * BT_RATE as u64 / RATE as u64;
    let pin = pcm.chunks_exact(2).map(|b| (i16::from_le_bytes([b[0], b[1]]) as i32).abs()).max().unwrap_or(0);
    let pout = out.chunks_exact(2).map(|b| (i16::from_le_bytes([b[0], b[1]]) as i32).abs()).max().unwrap_or(0);
    let ok = took == pcm.len() && fout.abs_diff(want) <= 1 && pout <= pin / 2 + pin / 50 + 2 && pout * 5 >= pin * 2; // the sinc's inter-sample peak (NLE4BKERNEL B896): half, within +2 % / -20 %
    serial_println!("[sinkall] bt synthetic frames_in={} frames_out={} want={} peak_in={} peak_out={} -> {}", fin, fout, want, pin, pout, if ok { "ok" } else { "FAIL" });
    (open, after, fin, fout, ok)
}

// ── AVRCPCT (rmbp-ledger B780) — TAIL-APPENDED: the headset carries the BT level ──────────────────────────────────────
// `master`'s BT arm (folded onto its first line, LINE-NEUTRAL): while the headset has accepted AVRCP SetAbsoluteVolume
// (`avctp::ct_volume`, an atomic), it applies the level, so the mixer plays BT at unity and keeps only the mute
// (BP-audio-14: the master applied once); otherwise the mixer scales, as before.

#[cfg(all(target_arch = "x86_64", feature = "a2dp"))]
fn bt_hw_volume() -> bool {
    crate::drivers::ehci::bthid::avctp::ct_volume()
}

// ── MIXERPUMP (rmbp-ledger B787) — TAIL-APPENDED: a level change runs the master stage now ───────────────────────────
// Every writer of the master model (`video::status`'s `volkey_usage` and `set_volume`: the F-row key, Settings' slider,
// the status menu, Softy, the Player's control, AVRCP CHANGED through `audio::bt_master`) calls [`level_changed`]: the
// same [`master`] the pump runs, for the output in force (`OUT_KIND`, OUT's atomic mirror). It reads and stores atomics
// only (`status::volume`, `uac::volume_want`, `avctp::ct_volume`): no OUT, no MIX, no mix, no sink write — so it is
// safe from the BTHID pass under `BT`. A USB device's Feature Unit then takes the level at the next EP0 service pass,
// the HDA amp in the writer itself, a headset at the next AVRCP watch pass; a mixer-scaled output at its next period.
// IDLEUSBLEVEL (B808): a UAC device's Feature Unit takes the level held or not ([`usb_level`], file tail).

/// MIXERPUMP: the master level moved — hand it to its carrier now (atomics only; any context).
pub fn level_changed() {
    let _ = master(OUT_KIND.load(Ordering::Acquire)); #[cfg(feature = "uac")] usb_level(); // IDLEUSBLEVEL (rmbp-ledger B808): the USB device takes the level held or not (file tail). LINE-NEUTRAL append, before this comment.
}

// ── IDLEUSBLEVEL (rmbp-ledger B808) — TAIL-APPENDED: a level change reaches the USB device held or not ───────────────
// Owner: the audio handler (CODEX §2), the master stage's hand-off to the USB carrier. The class driver's EP0 door runs on every
// service pass whether or not a writer holds the stream (`isoch::service_uvc` -> `uac::ep0_due`, gated on the stream
// and the wanted level only), so the level is handed whenever a device's Feature Unit carries it — idle, or while HDA
// or BT is the output — and the first period after the next `acquire` plays at it (no stale level; audio.md §5).
// Unconditional on `OUT_KIND`: while USB is held it repeats `master`'s identical store. Atomics only (§3's lock order).

/// IDLEUSBLEVEL: hand the master level to the USB device's Feature Unit, whatever the mixer's output (atomics only).
#[cfg(feature = "uac")]
fn usb_level() {
    let (lv, muted) = crate::video::status::volume();
    let _ = usb_hw_volume(lv, muted);
}

/// KNOBOFF (seat, 2026-10-26): `video::status` (the tray's volume word) exists only on the desktop family; the knob-off
/// image's mixer plays at unity, unmuted. One reader so the plain `./arroyo check` compiles.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn status_volume() -> (u8, bool) { crate::video::status::volume() }
#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn status_volume() -> (u8, bool) { (100, false) }

// ── A2DPKAT (rmbp-ledger B870) — TAIL-APPENDED: the headset's delay comes off the A/V clock on BT ─────────────────────
// Owner: the audio handler (CODEX §2), the mixer's OUT_BT position. The sink's AVDTP DelayReport (1/10 ms, read once at
// stream start by `avdtp`) is subtracted ONLY where the A/V clock is read ([`played_now`], folded on its line):
// `queued` keeps the pump's high-water flow control (a 200 ms delay there would exceed BT's 100 ms high mark and stop
// delivery) and SINK_PLAYED keeps `reap`/`account`'s drain test (an ended source must still reap once the output
// idles). No report: 0, the kernel-ms clock alone (as before). Atomics only, so `played_now` stays paint-safe.

/// A2DPKAT: the streaming headset's reported delay in 48 kHz frames while BT is the output, else 0.
#[cfg(all(target_arch = "x86_64", feature = "a2dp"))]
fn bt_delay() -> u64 {
    if OUT_KIND.load(Ordering::Acquire) != OUT_BT {
        return 0;
    }
    a2dp::delay_tenth_ms().map_or(0, |t| t as u64 * RATE as u64 / 10_000)
}

// ── NLE4BKERNEL (rmbp-ledger B896) — TAIL-APPENDED: the mixer's two rate converters are `audio_core::resample::Sinc` ─────
// Owner: Stria (CODEX §2), the mixer's ingest and its A2DP output (handlers.md §5 NLE4b's NLE4B-KERNEL); both were
// nearest-sample. A source at another rate holds ONE Sinc (`Source.rs`): its table is built by [`ingest_sinc`] in the
// opener's task BEFORE MIX is taken (`open_fmt`, `r3_open`) and moved in under MIX ([`Mix::keep_sinc`]); `Mix::write` hands
// it one frame at a time ([`sinc_push`], at most [`INGEST_MAX`] frames per call, so the soft-float work under MIX stays a
// few ms) and `Mix::end` flushes its lookahead ([`ingest_flush`]). The A2DP output holds ONE (`Out.rs`), built once per boot
// by [`bt_sinc_due`] in `pump` BEFORE OUT's try_lock and kept by [`bt_sinc_keep`], reset at `acquire`, its lookahead written
// into the stream at `release` ([`bt_drain`]). No table is built under a spin lock and no lock edge is new (audio.md §3);
// 48 kHz sources keep the integer copy. The i16 <-> f32 crossing is audio_core's ONE (`Sinc::process_i16` / `flush_i16`).

/// Source frames one `Mix::write` call converts at most (~46 ms at 44.1 kHz; every writer takes a partial count).
const INGEST_MAX: usize = 2_048;
/// The A2DP output's converter is kept in `Out` (built once per boot).
static BT_RS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The converter a new source at `rate` / `ch` needs, built here in the opener's task with no lock held (None at 48 kHz
/// or for a format `Mix::open` refuses), with the build's ms for the wire.
fn ingest_sinc(rate: u32, ch: u8) -> Option<(Sinc, u64)> {
    if rate == RATE || !(una_abi::AUDIO_RATE_MIN..=una_abi::AUDIO_RATE_MAX).contains(&rate) || !(ch == 1 || ch == 2) {
        return None;
    }
    let t0 = crate::arch::ms();
    let s = Sinc::new(rate, RATE, ch as usize);
    Some((s, crate::arch::ms().saturating_sub(t0)))
}

impl Mix {
    /// Move the converter [`ingest_sinc`] built into the source `open` just made (under MIX: a move, never a build).
    fn keep_sinc(&mut self, r: &Result<u32, (i64, &'static str)>, rs: Option<(Sinc, u64)>) {
        let (Ok(id), Some((rs, ms))) = (r, rs) else { return };
        let Some(s) = self.at(*id) else { return };
        serial_println!("[audio] mix resample id={} rate={}->{} ch={} taps={} table_ms={} -> sinc", s.id, s.rate, RATE, s.ch, rs.taps(), ms);
        s.rs = Some(rs);
    }
}

/// Interleaved converter output (`ch` = 1 or 2) into a source ring of stereo frames (mono to both channels).
fn push_frames(ring: &mut VecDeque<[i16; 2]>, ch: usize, out: &[i16]) {
    for f in out.chunks_exact(ch) {
        ring.push_back([f[0], f[ch - 1]]);
    }
}

/// One source frame (`l`, `r`; mono reads `l`) through the source's converter into its ring: the 48 kHz frames it yielded
/// (0..6; more than 8 ready wait for the next call). An offline [`Mix`] (a test) builds the converter here on first use;
/// the live mixer's are built by [`ingest_sinc`] before MIX.
fn sinc_push(rs: &mut Option<Sinc>, ring: &mut VecDeque<[i16; 2]>, rate: u32, ch: u8, l: i16, r: i16) -> u64 {
    let ch = ch as usize;
    let rs = rs.get_or_insert_with(|| Sinc::new(rate, RATE, ch));
    let mut out = [0i16; 2 * 8];
    let (_, made) = rs.process_i16(&[l, r][..ch], &mut out[..8 * ch]);
    push_frames(ring, ch, &out[..made * ch]);
    made as u64
}

/// End of a converting source's stream: its converter's lookahead (read as silence) into the ring.
fn ingest_flush(s: &mut Source) {
    let ch = s.ch as usize;
    let Some(rs) = s.rs.as_mut() else { return };
    let mut out = [0i16; 2 * 32];
    loop {
        let n = rs.flush_i16(&mut out[..32 * ch]);
        push_frames(&mut s.ring, ch, &out[..n * ch]);
        s.out_n += n as u64;
        if n < 32 {
            break;
        }
    }
}

/// The A2DP output's converter, built here in `pump` BEFORE OUT is taken, while BT is the sink wanted and none is kept.
fn bt_sinc_due() -> Option<(Sinc, u64)> {
    if BT_RS.load(Ordering::Acquire) || wanted() != OUT_BT {
        return None;
    }
    let t0 = crate::arch::ms();
    let s = Sinc::new(RATE, BT_RATE, 2);
    Some((s, crate::arch::ms().saturating_sub(t0)))
}

/// Keep the converter [`bt_sinc_due`] built (under OUT: a move); one built by a racing pump is dropped.
fn bt_sinc_keep(o: &mut Out, bt: Option<(Sinc, u64)>) {
    let Some((rs, ms)) = bt else { return };
    if o.rs.is_none() {
        serial_println!("[audio] mix resample bt rate={}->{} ch=2 taps={} table_ms={} -> sinc", RATE, BT_RATE, rs.taps(), ms);
        o.rs = Some(rs);
    }
    BT_RS.store(true, Ordering::Release);
}

/// The converter's lookahead as 44.1 kHz stereo bytes: the end of a BT stream (`release`, [`bt_drain`]; `bt_synthetic`).
fn bt_flush(rs: &mut Sinc, out: &mut Vec<u8>) {
    let mut buf = [0i16; 2 * 32];
    loop {
        let n = rs.flush_i16(&mut buf);
        for v in &buf[..n * 2] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if n < 32 {
            break;
        }
    }
}

/// `release` of the A2DP output: the converter's lookahead into the stream before its end, counted as delivered (the
/// live leg's `frames + 2 >= want`).
#[cfg(all(target_arch = "x86_64", feature = "a2dp"))]
fn bt_drain(o: &mut Out) {
    let mut tail = Vec::new();
    if let Some(rs) = o.rs.as_mut() {
        bt_flush(rs, &mut tail);
    }
    if !tail.is_empty() {
        let took = a2dp::write(&tail) as u64;
        DELIVERED[OUT_BT as usize].fetch_add(took / 4, Ordering::Relaxed);
        o.delivered += took / 4;
    }
}
