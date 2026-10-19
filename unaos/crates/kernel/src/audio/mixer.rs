//! CHARTER: Stria — driver
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! AUDIOMIX (rmbp-ledger B731) — THE MIXER. Until this file the audio seam had one writer: whoever took the HDA stream
//! (`drivers::hda::play`) or the USB class driver's ring (`drivers::usb::uac`) first owned the speaker, and a second
//! program's `SYS_AUDIO_OPEN` was refused `stream-held`. The mixer is the one producer of the sink:
//!
//! * **Sources** — up to [`MAX_SOURCES`], each a ring of PCM16 / 2 ch / 48 kHz frames ([`SRC_CAP`] = 200 ms) with its
//!   own gain (Q8, [`UNITY`] = 256). A source written in another format (8..48 kHz, 1|2 ch, 8|16 bit — the
//!   `SYS_AUDIO_OPEN` range) is widened, duplicated and nearest-sample resampled on the way in (`hda_play::feed`'s
//!   arithmetic). A ring-3 program's stream is a source keyed by its (row, slot generation) — the key and liveness
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
//! Wire: `[audio] mix open id= name= rate= ch= bits= gain=` · `[audio] mix sink=<usb|hda> -> ok|refused=<why>` ·
//! `[audio] close handle= pid= reason= frames= underruns=` · `[audio] mix sources=<n> period_us=<n> underruns=<n>
//! master=<n>` · `:: AUDIOMIX: sources=2 mixed=ok underruns=0 mute=ok -> PASS ::` (`tests audiomix`, [`selftest`]).

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

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
    underruns: u32,
    owner: Option<Owner>,
}

impl Source {
    fn live(&self) -> bool {
        self.primed && !self.ended
    }
    /// 48 kHz frames of this source the sink has played, given the sink's play position in emitted frames.
    fn played48(&self, sink_played: u64) -> u64 {
        self.taken.saturating_sub(self.last_e.saturating_sub(sink_played))
    }
    /// The same, in source frames, capped at the frames written (a padded underrun is never counted as played).
    fn played_src(&self, sink_played: u64) -> u64 {
        (self.played48(sink_played) * self.rate as u64 / RATE as u64).min(self.in_n)
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
            primed: false, ended: false, underruns: 0, owner,
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
        while i + fin <= data.len() {
            // the 48 kHz frames this source frame yields (nearest-sample: out k plays source frame k * src / eff)
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
            for _ in 0..k {
                s.ring.push_back([l, r]);
            }
            s.out_n += k as u64;
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
        self.at(id).map(|s| s.ended = true).is_some()
    }

    pub fn set_gain(&mut self, id: u32, gain: u16) -> bool {
        self.at(id).map(|s| s.gain = gain).is_some()
    }

    pub fn remove(&mut self, id: u32) -> Option<Source> {
        let i = self.srcs.iter().position(|s| s.id == id)?;
        Some(self.srcs.remove(i))
    }

    /// Some source holds a frame.
    pub fn has_data(&self) -> bool {
        self.srcs.iter().any(|s| !s.ring.is_empty())
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
}
static OUT: crate::sync::Mutex<Out> =
    crate::sync::Mutex::new(Out { kind: OUT_NONE, hda: 0, draining: false, delivered: 0, pend: Vec::new(), pend_off: 0, said: "" });
/// The sink's play position in emitted frames (the A/V clock's base), refreshed by every pump.
static SINK_PLAYED: AtomicU64 = AtomicU64::new(0);

/// The HDA door's key for the mixer (never a process row).
const MIX_ROW: usize = usize::MAX;
const MIX_GEN: u64 = u64::MAX;

/// The master stage the sink needs: (gain Q8, mute, level 0..16 for the wire). HDA's codec amp carries the level.
fn master(kind: u8) -> (u16, bool, u8) {
    let (lv, muted) = crate::video::status::volume();
    if kind == OUT_HDA { (UNITY, false, lv) } else { (lv.min(16) as u16 * 16, muted, lv) }
}

/// The sink `audio::current()` names, as an output kind this image can drive.
fn wanted() -> u8 {
    match super::current() {
        super::Sink::Usb if cfg!(feature = "uvc") => OUT_USB,
        super::Sink::Hda if super::hda_built() => OUT_HDA,
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
        _ => {}
    }
    o.kind = OUT_NONE;
    o.draining = false;
}

fn acquire(o: &mut Out, kind: u8) -> bool {
    let r: Result<u32, &'static str> = match kind {
        #[cfg(feature = "uac")]
        OUT_USB => crate::drivers::usb::uac::begin().map(|_| 0),
        #[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
        OUT_HDA => crate::drivers::hda::play::r3_open(MIX_ROW, MIX_GEN, mix_alive, 0, RATE as u64, 2, 16).map_err(|(_, why)| why),
        _ => Err("no-sink"),
    };
    let name = if kind == OUT_USB { "usb" } else { "hda" };
    match r {
        Ok(h) => {
            o.kind = kind;
            o.hda = h;
            o.draining = false;
            o.delivered = 0;
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
        _ => 0,
    };
    pend + held
}

/// (starve, high) water in frames: mix while below high; below starve a short source is padded (an underrun). The
/// HDA stream runs only once its 128 KiB ring is primed (hda_play's threshold, 32 768 frames), so its high sits above.
fn marks(kind: u8) -> (u64, u64) {
    if kind == OUT_HDA { (2 * PERIOD_FRAMES as u64, 40_960) } else { (2 * PERIOD_FRAMES as u64, 10 * PERIOD_FRAMES as u64) }
}

/// Hand the pending period to the output: true when it was all taken.
fn deliver(o: &mut Out) -> bool {
    while o.pend_off < o.pend.len() {
        let took: i64 = match o.kind {
            #[cfg(feature = "uac")]
            OUT_USB => crate::drivers::usb::uac::write(&o.pend[o.pend_off..]) as i64,
            #[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
            OUT_HDA => crate::drivers::hda::play::r3_write(o.hda as u64, MIX_ROW, MIX_GEN, &o.pend[o.pend_off..]),
            _ => 0,
        };
        if took > 0 {
            o.pend_off += took as usize;
            o.delivered += took as u64 / 4;
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

/// Drop sources whose owner exited and ended sources the sink has played out.
fn reap(m: &mut Mix, sink_played: u64) {
    let mut i = 0;
    while i < m.srcs.len() {
        let s = &m.srcs[i];
        let dead = s.owner.as_ref().is_some_and(|o| !(o.alive)(o.row, o.tgen));
        let done = s.ended && s.ring.is_empty() && s.played48(sink_played) >= s.taken;
        if dead || done {
            let s = m.srcs.remove(i);
            closed(&s, if dead { "owner-exited" } else { "drained" }, sink_played, m);
        } else {
            i += 1;
        }
    }
}

fn closed(s: &Source, reason: &str, sink_played: u64, m: &Mix) {
    let pid = s.owner.as_ref().map_or(0, |o| o.pid);
    serial_println!("[audio] close handle={} pid={} name={} reason={} frames={} underruns={}", s.id, pid, s.name, reason, s.played_src(sink_played), s.underruns);
    status(m, OUT_NONE);
}

fn status(m: &Mix, kind: u8) {
    let (_, _, lv) = master(kind);
    serial_println!("[audio] mix sources={} period_us={} underruns={} master={}", m.srcs.len(), PERIOD_US, m.underruns, lv);
}

/// THE PUMP: bring the output the current sink names up or down, and move mixed periods into it while it has room.
pub fn pump() {
    let Some(mut o) = OUT.try_lock() else { return };
    let o = &mut *o;
    let mut buf = [0i16; PERIOD_FRAMES * 2];
    for _ in 0..64 {
        let q = queued(o);
        {
            let mut m = MIX.lock();
            let p = m.emitted.saturating_sub(q);
            SINK_PLAYED.store(p, Ordering::Release);
            reap(&mut *m, p);
            if m.srcs.is_empty() && o.pend_off >= o.pend.len() {
                drop(m);
                match o.kind {
                    #[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
                    OUT_HDA if !o.draining => {
                        let _ = crate::drivers::hda::play::r3_write(o.hda as u64, MIX_ROW, MIX_GEN, &[]); // end of stream: the ring drains, then stops
                        o.draining = true;
                    }
                    OUT_HDA => {}
                    _ => release(o),
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
        o.pend.clear();
        o.pend_off = 0;
        for v in buf.iter() {
            o.pend.extend_from_slice(&v.to_le_bytes());
        }
        if !deliver(o) {
            return;
        }
    }
}

// ── Kernel sources ────────────────────────────────────────────────────────────────────────────────────

/// A kernel source (48 kHz / 2 / 16) at `gain`.
pub fn open(name: &'static str, gain: u16) -> Result<u32, &'static str> {
    let r = {
        let mut m = MIX.lock();
        let r = m.open(name, RATE, 2, 16, gain, None);
        if r.is_ok() {
            status(&m, wanted());
        }
        r
    };
    if let Ok(id) = r {
        serial_println!("[audio] mix open id={} name={} rate={} ch=2 bits=16 gain={}", id, name, RATE, gain);
    }
    r.map_err(|(_, why)| why)
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
        let mut m = MIX.lock();
        let r = m.open("ring3", rate as u32, ch as u8, bits as u8, UNITY, Some(Owner { row, tgen, alive, pid }));
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
