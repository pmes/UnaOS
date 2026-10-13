#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// STRIAMETAL (LEDGER SR139): `APPS/STRIA.ELF` — Stria, UnaOS's A/V handler (CODEX §2; Amendment II: Aether renders,
// Stria plays), as ONE ring-3 program on the image. Peter (2026-10-14): "which is the base handler that takes care of
// playing video? throw getting it up to speed natively into the mix."
//
// WHAT IT IS. A window over one media file read off the volume: `stria <path>` (argv[1]; a Quarry double-click
// delivers the path as a BUS_VERB_NOTICE frame in this program's mailbox, video/quarry/openers.rs, polled every
// pass). The container is demuxed by demux_core (MP4, Matroska, WebM; a bare WAV/FLAC/Ogg/MP3/AAC/AIFF file goes
// straight to audio_core), the pictures decoded by vp8_core / vp9_core / av1_core / h264_core and the sound by
// audio_core — the SAME no_std cores the host Stria (handlers/stria, gneiss_pal's dsp arm) and the kernel's Player
// (video/vplay.rs) play through (seam `shared-core`).
//
// THE LOOP (one thread; mirrors handlers/stria/src/media.rs's `Player::tick` and the kernel's vplay.rs job/consumer
// split, folded into one pass): drain the input ring and the mailbox; advance the CLOCK; feed the sound
// (SYS_AUDIO_WRITE until -EAGAIN, the unaccepted tail kept); decode video packets until one picture is converted and
// waiting (at most PACKETS_PER_TICK per pass); present it once the clock reaches its time (SYS_WIN_PRESENT); sleep
// until the next picture is due (at most 10 ms).
//
// THE CLOCK. Master = the sound (handlers/stria media.rs:18 "master clock = audio when an audio track decodes and an
// output exists, else wall"): `base_ms + SYS_AUDIO_POS(PLAYED) * 1000 / rate` — frames the HDA ring has PLAYED, capped
// by the kernel at the frames written, so an underrun holds the pictures with the sound. Until the stream runs the
// picture holds its first frame (at most AUDIO_WAIT_MS, vplay.rs AUDIO_WAIT_MS's shape); a stream that never runs, is
// refused (aarch64: `refused=no-aarch64-sink`), or dies, hands the clock to the wall (SYS_GETINFO ms), pause-aware.
// When the sound has played out and the pictures continue, the wall carries on from where the sound stopped.
//
// DROPS. A picture is converted only while it can still be shown: once the clock has passed its slot
// (pts + frame_ms <= clock) it is decoded (references) and counted dropped, never drawn — the kernel Player's rule
// (vplay.rs `present`: the newest due frame wins, the older ones are drops).
//
// HEAP. The file is read whole (SYS_STAT-sized, one allocation, at most FILE_CAP) and a picture's decoder state is
// budgeted per codec (media.rs `bytes_per_px`): file + frames + WORK over HEAP_BUDGET is refused BY NAME on the window
// and the wire before any decoder exists.
//
// WIRE (read by the kernel's `tests stria`, striaapp.rs):
//   `:: STRIA: open path=<p> tracks=v:<codec|none> a:<codec|none> dur_ms=<n> size=<w>x<h> bytes=<n> ::`
//   `:: STRIA: open path=<p> -> FAIL reason=<r> ::` (and the window says why)
//   `:: STRIA: audio handle=<h> rate=<r> underruns=<u> state=<open|refused|end> ::` (h < 0 = the open's errno)
//   `:: STRIA: frame n=<n> pts_ms=<p> late_ms=<l> ::` every 30th picture shown
//   `:: STRIA: end -> PASS|FAIL frames=<n> audio=<ok|none> drift_ms=<n> late_max_ms=<n> dropped=<n> clock=<audio|wall> ::`

extern crate alloc;

mod heap;
mod media;
mod sys;
mod ui;

use alloc::string::String;
use alloc::vec::Vec;
use demux_core::Demuxer;
use media::{Out, Sound, VDec};
use sys::{exit, now_ms, sleep_ms, sys, write};
use una_abi::{
    input_ev_payload, input_ev_type, AUDIO_POS_PLAYED, AUDIO_POS_UNDERRUNS, AUDIO_WRITE_MAX, BUS_FRAME_MAX, BUS_HDR_LEN, BUS_KIND_REPLY, BUS_MAGIC,
    BUS_VERB_NOTICE, EAGAIN, INPUT_EV_CLOSE_REQ, INPUT_EV_KEY_DOWN, KEY_ESC, MRECV_NOWAIT, SYS_AUDIO_CLOSE, SYS_AUDIO_OPEN, SYS_AUDIO_POS,
    SYS_AUDIO_WRITE, SYS_INPUT_POLL, SYS_MRECV, SYS_WIN_CREATE, SYS_WIN_PRESENT,
};

#[global_allocator]
static HEAP: heap::Heap = heap::Heap::sbrk();

/// The compositor's window ceiling (FB_WIN_MAX_W/H on every arch): the surface is always this stride.
pub const MAX_W: i32 = 288;
pub const MAX_H: i32 = 288;
/// The transport bar under the picture.
pub const BAR_H: i32 = 28;
/// The video rect.
pub const VID_W: usize = MAX_W as usize;
pub const VID_H: usize = (MAX_H - BAR_H) as usize;
/// The largest file read whole (the heap's top size class is 32 MiB).
const FILE_CAP: usize = 24 << 20;
/// What the heap may hold for one file (the 64 MiB ELF window less the image, the stack and the guard).
const HEAP_BUDGET: usize = 52 << 20;
/// The working set beside the file and the frames (the sound's blocks, the demuxer's tables, the back buffer).
const WORK: usize = 4 << 20;
/// How long the picture holds for the sound's stream to run.
const AUDIO_WAIT_MS: u64 = 2_000;
/// Work bounds per pass (the window stays live while a slow decoder catches up).
const PACKETS_PER_TICK: usize = 8;
const BLOCKS_PER_TICK: usize = 6;
/// A written end of stream that has not played out by then is taken as played.
const DRAIN_MS: u64 = 4_000;
/// The default file when none is named (the staged sample, its card twin under a UnaFS root).
const TESTF: [&str; 2] = ["/system/test-f/TEST.WEBM", "/boot/system/test-f/TEST.WEBM"];

pub const BG: u32 = 0x10_1014;
pub const INK: u32 = 0xE8_E8EC;
pub const DIM: u32 = 0x8C_8C96;
pub const ACCENT: u32 = 0x4F_9DFF;

// ---- console lines --------------------------------------------------------------------------------------------
pub struct Line {
    b: [u8; 320],
    n: usize,
}
impl Line {
    pub fn new(s: &[u8]) -> Line {
        let mut l = Line { b: [0; 320], n: 0 };
        l.put(s);
        l
    }
    pub fn put(&mut self, s: &[u8]) -> &mut Self {
        for &c in s {
            if self.n < self.b.len() - 1 {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        self
    }
    pub fn dec(&mut self, v: i64) -> &mut Self {
        if v < 0 {
            self.put(b"-");
        }
        let mut v = v.unsigned_abs();
        let mut d = [0u8; 20];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.put(&d[i..])
    }
    pub fn wire(&mut self) {
        self.b[self.n] = b'\n';
        write(&self.b[..self.n + 1]);
    }
}

// ---- the window -------------------------------------------------------------------------------------------------

/// The window landmarks: the RO info page at base + 0x4000, surface slot 0 at base + 0x5000 — an elf-model program
/// finds them at the FIXED slot base (RING3WIN), as LUMEN.ELF and AETHER.ELF do.
#[cfg(target_arch = "x86_64")]
fn landmark_base() -> u64 {
    una_abi::USER_BASE_X86
}
#[cfg(not(target_arch = "x86_64"))]
fn landmark_base() -> u64 {
    match una_abi::args() {
        Some(a) if a.window_base() != 0 => a.window_base(),
        _ => {
            Line::new(b":: STRIA: no args page (window base unknown) ::").wire();
            exit(1)
        }
    }
}

pub struct Win {
    id: i64,
    surf: *mut u32,
    flags: *const u32,
    pub fb: Vec<u32>,
}

impl Win {
    /// Copy the back buffer to the surface (alpha opaque) and composite.
    pub fn present(&mut self) {
        for (i, &c) in self.fb.iter().enumerate() {
            unsafe { self.surf.add(i).write_volatile(0xFF00_0000 | c) };
        }
        sys(SYS_WIN_PRESENT, self.id as u64, 0, 0, 0);
    }
    /// VUGMIN: bit 1 of the info page's flags = every window hidden (a good citizen stops presenting).
    pub fn visible(&self) -> bool {
        (unsafe { self.flags.read_volatile() } & 2) == 0
    }
}

// ---- the session ------------------------------------------------------------------------------------------------

/// One open file: its demuxer and picture decoder, its sound and the stream it plays through, the clock, the counts.
pub struct Session {
    pub path: String,
    demux: Option<Demuxer>,
    vid: Option<demux_core::Track>,
    vdec: Option<VDec>,
    vpts0: i64,
    pub frame_ms: u64,
    pub dur_ms: u64,
    pub v_codec: &'static str,
    pub a_codec: &'static str,
    pub size: (u32, u32),
    v_eos: bool,
    /// The converted picture waiting for its time, in `pend`.
    next: Option<u64>,
    pend: Vec<u32>,
    skip_until: u64,
    prefixed: Vec<u8>,
    // the sound
    sound: Option<Sound>,
    share: Option<Demuxer>,
    pub handle: i64,
    stream_ms: u64,
    base_ms: u64,
    pcm: Vec<u8>,
    pcm_at: usize,
    fed: u64,
    finished: Option<u64>,
    pub audio_ok: bool,
    audio_dead: bool,
    audio_frames: u64,
    pub gain: i32,
    // the clock
    pub media_ms: u64,
    last_ms: u64,
    pub playing: bool,
    pub ended: bool,
    on_audio: bool,
    // the counts
    pub shown: u64,
    pub dropped: u64,
    sync_sum: u64,
    pub late_max: u64,
    by_audio: u64,
    errs: u64,
    pub note: Option<String>,
}

/// Why a file did not open: the wire's reason and the window's words.
pub struct Refusal {
    pub reason: String,
    pub say: String,
}

fn refuse(reason: String, say: String) -> Refusal {
    Refusal { reason, say }
}

impl Session {
    /// Read, budget, demux and open the decoders of `path`.
    pub fn open(path: &str) -> Result<Session, Refusal> {
        let bytes = match sys::read_file(path.as_bytes(), FILE_CAP) {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => return Err(refuse(String::from("empty-file"), String::from("The file is empty."))),
            Err(e) if e == una_abi::E2BIG => {
                return Err(refuse(alloc::format!("over-file-cap cap_kib={}", FILE_CAP / 1024), alloc::format!("Over the {} MiB file cap.", FILE_CAP >> 20)))
            }
            Err(e) => return Err(refuse(alloc::format!("read code={}", e), alloc::format!("Could not read it (code {}).", e))),
        };
        let len = bytes.len();
        let mut s = Session {
            path: String::from(path),
            demux: None,
            vid: None,
            vdec: None,
            vpts0: 0,
            frame_ms: 33,
            dur_ms: 0,
            v_codec: "none",
            a_codec: "none",
            size: (0, 0),
            v_eos: true,
            next: None,
            pend: alloc::vec![0; VID_W * VID_H],
            skip_until: 0,
            prefixed: Vec::new(),
            sound: None,
            share: None,
            handle: 0,
            stream_ms: 0,
            base_ms: 0,
            pcm: Vec::new(),
            pcm_at: 0,
            fed: 0,
            finished: None,
            audio_ok: false,
            audio_dead: false,
            audio_frames: 0,
            gain: 256,
            media_ms: 0,
            last_ms: now_ms(),
            playing: true,
            ended: false,
            on_audio: false,
            shown: 0,
            dropped: 0,
            sync_sum: 0,
            late_max: 0,
            by_audio: 0,
            errs: 0,
            note: None,
        };
        if demux_core::probe(&bytes).is_none() {
            // A bare sound file: audio_core sniffs it (WAV, AIFF, FLAC, Ogg, MP3, ADTS).
            let dec = audio_core::Decoder::open_bytes(bytes).map_err(|e| refuse(alloc::format!("not-media ({:?})", e), String::from("Not a format Stria plays.")))?;
            let snd = Sound::new(dec).map_err(|e| refuse(e.clone(), e))?;
            s.a_codec = snd.codec;
            s.dur_ms = audio_core::AudioDecoder::info(&snd.dec).frames.map_or(0, |f| f * 1000 / snd.rate.max(1) as u64);
            s.sound = Some(snd);
            return Ok(s);
        }
        let d = Demuxer::open(bytes).map_err(|e| refuse(alloc::format!("demux ({})", e), alloc::format!("{}", e)))?;
        s.dur_ms = d.duration_ns() / 1_000_000;
        if let Some(v) = d.video_track().cloned() {
            s.v_codec = media::tok(&v.codec);
            s.size = (v.width, v.height);
            let need = len + v.width as usize * v.height as usize * media::bytes_per_px(&v.codec) + WORK;
            if need > HEAP_BUDGET {
                return Err(refuse(
                    alloc::format!("frame-over-heap size={}x{} need_kib={} heap_kib={}", v.width, v.height, need / 1024, HEAP_BUDGET / 1024),
                    alloc::format!("{}x{} needs {} MiB; the heap holds {} MiB.", v.width, v.height, need >> 20, HEAP_BUDGET >> 20),
                ));
            }
            match VDec::new(&v) {
                Ok(dec) => {
                    s.vpts0 = d.track_index(v.id).and_then(|i| d.track_samples(i).map(|x| x.pts).min()).unwrap_or(0);
                    if v.sample_count > 0 && v.duration_ns > 0 {
                        s.frame_ms = (v.duration_ns / v.sample_count / 1_000_000).clamp(1, 1000);
                    }
                    s.vdec = Some(dec);
                    s.v_eos = false;
                }
                Err(e) => s.note = Some(e),
            }
            s.vid = Some(v);
        }
        if let Some(a) = d.audio_track().cloned() {
            s.a_codec = media::tok(&a.codec);
            if audio_core::container::plays(d.format(), &a.codec) {
                match audio_core::container::open_demuxed(d.share()).map(audio_core::Decoder::from_source) {
                    Ok(dec) => match Sound::new(dec) {
                        Ok(snd) => {
                            s.sound = Some(snd);
                            s.share = Some(d.share());
                        }
                        Err(e) => s.note = Some(e),
                    },
                    Err(e) => s.note = Some(alloc::format!("audio: {:?}", e)),
                }
            } else {
                s.note = Some(alloc::format!("no sound decoder for {} in {:?}", s.a_codec, d.format()));
            }
        }
        if s.vdec.is_none() && s.sound.is_none() {
            let why = s.note.clone().unwrap_or_else(|| String::from("no-tracks"));
            return Err(refuse(alloc::format!("nothing-to-play ({})", why), why));
        }
        s.demux = Some(d);
        Ok(s)
    }

    fn has_video(&self) -> bool {
        self.vdec.is_some()
    }

    /// Open the sound's stream with its frame 0 at `at_ms` on the media timeline.
    fn audio_start(&mut self, at_ms: u64) {
        self.audio_stop();
        let Some(snd) = self.sound.as_ref() else { return };
        if self.audio_dead {
            return;
        }
        let (rate, ch) = (snd.out_rate, snd.out_ch);
        let h = sys(SYS_AUDIO_OPEN, rate as u64, ch as u64, 16, 0);
        self.handle = h;
        self.base_ms = at_ms;
        self.stream_ms = now_ms();
        self.fed = 0;
        self.pcm.clear();
        self.pcm_at = 0;
        self.finished = None;
        if h <= 0 {
            self.audio_dead = true;
        }
        let mut l = Line::new(b":: STRIA: audio handle=");
        l.dec(h).put(b" rate=").dec(rate as i64).put(b" underruns=0 state=").put(if h > 0 { b"open" as &[u8] } else { b"refused" }).put(b" ::");
        l.wire();
    }

    /// Close the stream now (what is queued does not play).
    fn audio_stop(&mut self) {
        if self.handle > 0 {
            sys(SYS_AUDIO_CLOSE, self.handle as u64, 0, 0, 0);
        }
        self.handle = 0;
        self.on_audio = false;
    }

    /// The stream died (-EIO) or never ran: the wall has the clock from here.
    fn audio_fail(&mut self) {
        self.audio_stop();
        self.audio_dead = true;
    }

    /// The sound's underruns so far (the stream's own count).
    fn underruns(&self) -> i64 {
        if self.handle > 0 { sys(SYS_AUDIO_POS, self.handle as u64, AUDIO_POS_UNDERRUNS, 0, 0).max(0) } else { 0 }
    }

    /// Advance the clock to `now`.
    fn clock(&mut self, now: u64) {
        let dt = now.saturating_sub(self.last_ms);
        self.last_ms = now;
        if !self.playing {
            return;
        }
        if self.handle > 0 {
            let p = sys(SYS_AUDIO_POS, self.handle as u64, AUDIO_POS_PLAYED, 0, 0);
            let rate = self.sound.as_ref().map_or(48_000, |s| s.out_rate).max(1) as u64;
            if p < 0 {
                self.audio_fail();
            } else if p > 0 {
                self.audio_ok = true;
                let drained = self.finished.is_some() && p as u64 >= self.fed;
                if !drained {
                    let a = self.base_ms + p as u64 * 1000 / rate;
                    self.media_ms = a.max(self.media_ms.saturating_sub(40)); // a re-read's jitter never runs the picture back
                    self.on_audio = true;
                    return;
                }
                // played out: the wall carries on from the sound's end
            } else if now.saturating_sub(self.stream_ms) < AUDIO_WAIT_MS {
                return; // the ring is arming: the picture holds
            } else {
                Line::new(b":: STRIA: audio handle=").dec(self.handle).put(b" rate=").dec(rate as i64).put(b" underruns=0 state=never-ran ::").wire();
                self.audio_fail();
            }
        }
        self.on_audio = false;
        self.media_ms += dt;
    }

    /// Feed the stream until it is full (or the per-pass bound): decoded blocks, gained, written.
    fn feed_audio(&mut self) {
        if self.handle <= 0 || self.finished.is_some() {
            return;
        }
        let h = self.handle as u64;
        let mut blocks = 0usize;
        let mut fail = false;
        if let Some(snd) = self.sound.as_mut() {
            let fsz = 2 * snd.out_ch;
            loop {
                if self.pcm_at >= self.pcm.len() {
                    self.pcm.clear();
                    self.pcm_at = 0;
                    if snd.eos {
                        if sys(SYS_AUDIO_WRITE, h, 0, 0, 0) == 0 {
                            self.finished = Some(now_ms());
                        }
                        break;
                    }
                    if blocks >= BLOCKS_PER_TICK {
                        break;
                    }
                    self.audio_frames += snd.block(&mut self.pcm, self.gain) as u64;
                    blocks += 1;
                    continue;
                }
                let end = (self.pcm_at + AUDIO_WRITE_MAX).min(self.pcm.len());
                let r = sys(SYS_AUDIO_WRITE, h, self.pcm[self.pcm_at..].as_ptr() as u64, (end - self.pcm_at) as u64, 0);
                if r > 0 {
                    self.pcm_at += r as usize;
                    self.fed += r as u64 / fsz as u64;
                } else if r == EAGAIN {
                    break;
                } else {
                    fail = true;
                    break;
                }
            }
        }
        if fail {
            self.audio_fail();
        }
    }

    /// Decode until one picture is converted and waiting, at most PACKETS_PER_TICK packets.
    fn pull_video(&mut self) {
        let (Some(d), Some(vdec), Some(vt)) = (self.demux.as_mut(), self.vdec.as_mut(), self.vid.as_ref()) else { return };
        let z = vt.to_ns(self.vpts0);
        let to_ms = move |ns: i64| (ns - z).max(0) as u64 / 1_000_000;
        let (skip, fms, media) = (self.skip_until, self.frame_ms, self.media_ms);
        let want = move |pts: u64| pts >= skip && pts + fms > media;
        let mut n = 0usize;
        while self.next.is_none() && n < PACKETS_PER_TICK && !(self.v_eos && !vdec.pending()) {
            n += 1;
            let pkt = loop {
                match d.next_packet() {
                    Some(p) if p.track == vt.id => break Some(p),
                    Some(_) => continue,
                    None => break None,
                }
            };
            let (data, pts_ns): (&[u8], i64) = match &pkt {
                Some(p) if vt.frame_prefix.is_empty() => (p.data.as_slice(), vt.to_ns(p.pts)),
                Some(p) => {
                    self.prefixed.clear();
                    self.prefixed.extend_from_slice(&vt.frame_prefix);
                    self.prefixed.extend_from_slice(&p.data);
                    (self.prefixed.as_slice(), vt.to_ns(p.pts))
                }
                None => {
                    self.v_eos = true;
                    (&[] as &[u8], 0)
                }
            };
            let pts_ms = to_ms(pts_ns);
            match vdec.decode(data, pts_ms, pts_ns, &to_ms, &want, &mut self.pend, VID_W, VID_H) {
                Ok(Out::Shown(t)) => self.next = Some(t),
                Ok(Out::Dropped(t)) => {
                    if t >= skip {
                        self.dropped += 1;
                    }
                }
                Ok(Out::None) => {}
                Err(e) => {
                    self.errs += 1;
                    if self.errs <= 3 {
                        Line::new(b":: STRIA: decode error pts_ms=").dec(pts_ms as i64).put(b" ").put(ui::fit(&e, 80).as_bytes()).put(b" ::").wire();
                    }
                }
            }
        }
    }

    /// Show the waiting picture if its time has come: into the back buffer's video rect. `true` = shown.
    fn show_due(&mut self, fb: &mut [u32]) -> bool {
        let Some(t) = self.next else { return false };
        if t > self.media_ms {
            return false;
        }
        for r in 0..VID_H {
            fb[r * ui::W..r * ui::W + VID_W].copy_from_slice(&self.pend[r * VID_W..r * VID_W + VID_W]);
        }
        self.next = None;
        let late = self.media_ms - t;
        self.shown += 1;
        self.sync_sum += late;
        self.late_max = self.late_max.max(late);
        if self.on_audio {
            self.by_audio += 1;
        }
        if self.shown % 30 == 0 {
            Line::new(b":: STRIA: frame n=").dec(self.shown as i64).put(b" pts_ms=").dec(t as i64).put(b" late_ms=").dec(late as i64).put(b" ::").wire();
        }
        true
    }

    /// Both halves done: the pictures shown or passed, the sound played out (or gone).
    fn done(&self, now: u64) -> bool {
        let v_done = !self.has_video() || (self.v_eos && self.next.is_none() && !self.vdec.as_ref().is_some_and(|v| v.pending()));
        let a_done = match (&self.sound, self.handle > 0) {
            (None, _) => true,
            (Some(s), false) => s.eos || self.audio_dead || self.has_video(),
            (Some(_), true) => match self.finished {
                None => false,
                Some(at) => {
                    let p = sys(SYS_AUDIO_POS, self.handle as u64, AUDIO_POS_PLAYED, 0, 0);
                    p < 0 || p as u64 >= self.fed || now.saturating_sub(at) > DRAIN_MS
                }
            },
        };
        // a video whose sound could not play ends with its pictures; an audio-only file whose sound could not play
        // ends when its decode has run out (the wall clock has nothing else to wait for)
        let a_done = a_done || (self.audio_dead && self.sound.as_ref().map_or(true, |s| s.eos || self.has_video()));
        v_done && a_done
    }

    /// The wire's end line (once).
    fn end_line(&mut self) {
        let under = self.underruns();
        if self.sound.is_some() && self.handle > 0 {
            let rate = self.sound.as_ref().map_or(0, |s| s.out_rate);
            Line::new(b":: STRIA: audio handle=").dec(self.handle).put(b" rate=").dec(rate as i64).put(b" underruns=").dec(under).put(b" state=end ::").wire();
        }
        let pass = if self.has_video() { self.shown > 0 } else { self.audio_frames > 0 };
        let mut l = Line::new(b":: STRIA: end -> ");
        if pass {
            l.put(b"PASS");
        } else {
            l.put(b"FAIL reason=").put(if self.has_video() { b"no-frame" as &[u8] } else { b"no-sound" });
        }
        l.put(b" frames=").dec(self.shown as i64);
        l.put(b" audio=").put(if self.audio_ok { b"ok" as &[u8] } else { b"none" });
        l.put(b" drift_ms=").dec((self.sync_sum / self.shown.max(1)) as i64);
        l.put(b" late_max_ms=").dec(self.late_max as i64);
        l.put(b" dropped=").dec(self.dropped as i64);
        l.put(b" clock=").put(if self.by_audio > 0 && self.by_audio * 2 >= self.shown { b"audio" as &[u8] } else { b"wall" });
        l.put(b" ::");
        l.wire();
    }

    /// Restart the sound at `at_ms` (a seek, a resume): the decoder repositioned by its own table (else from the
    /// top, dropped up to `at_ms`), a fresh stream.
    fn audio_restart(&mut self, at_ms: u64) {
        if self.audio_dead {
            return;
        }
        let share = self.share.as_ref().map(|d| d.share());
        if let Some(snd) = self.sound.as_mut() {
            let rebuild = move || share.as_ref().and_then(|d| audio_core::container::open_demuxed(d.share()).ok().map(audio_core::Decoder::from_source));
            snd.seek(at_ms, &rebuild);
        }
        self.audio_start(at_ms);
    }

    /// Seek both halves to `ms` (the transport bar, the arrows): the picture from the keyframe at or before it,
    /// decoded unseen up to `ms`; the sound repositioned and its stream reopened; the clock set.
    pub fn seek(&mut self, ms: u64) {
        let ms = if self.dur_ms > 0 { ms.min(self.dur_ms.saturating_sub(1)) } else { ms };
        if let (Some(d), Some(vt), Some(vdec)) = (self.demux.as_mut(), self.vid.as_ref(), self.vdec.as_mut()) {
            let target = vt.to_ns(self.vpts0) + ms as i64 * 1_000_000;
            let key = d.seek(target);
            vdec.reset();
            self.v_eos = false;
            self.next = None;
            self.skip_until = ms;
            let mut l = Line::new(b":: STRIA: seek to_ms=");
            l.dec(ms as i64).put(b" key_ms=").dec(key.map_or(-1, |k| (k - target + ms as i64 * 1_000_000) / 1_000_000)).put(b" ::");
            l.wire();
        }
        self.media_ms = ms;
        self.ended = false;
        if self.playing {
            self.audio_restart(ms);
        } else {
            self.audio_stop();
        }
    }

    /// Pause (the stream closes: nothing queued plays on) or resume (the sound restarts where the clock stands).
    pub fn set_playing(&mut self, on: bool) {
        if on == self.playing {
            return;
        }
        self.last_ms = now_ms();
        if on {
            if self.ended {
                self.playing = true;
                self.seek(0);
                return;
            }
            self.playing = true;
            let at = self.media_ms;
            self.audio_restart(at);
        } else {
            self.playing = false;
            self.audio_stop();
        }
    }
}

// ---- the program ------------------------------------------------------------------------------------------------

/// The window's words when nothing plays (a refusal, or no file yet).
fn say(win: &mut Win, title: &str, lines: &[&str]) {
    ui::fill(&mut win.fb, 0, 0, MAX_W, MAX_H, BG);
    ui::text(&mut win.fb, 12, 16, title, INK);
    for (i, s) in lines.iter().enumerate() {
        let mut y = 40 + i as i32 * 24;
        for chunk in ui::fit(s, 33 * 3).as_bytes().chunks(33) {
            ui::text(&mut win.fb, 12, y, core::str::from_utf8(chunk).unwrap_or("?"), DIM);
            y += 10;
        }
    }
    win.present();
}

/// The picture rect when the file has no pictures: what is playing.
fn paint_sound_only(win: &mut Win, s: &Session) {
    ui::fill(&mut win.fb, 0, 0, MAX_W, VID_H as i32, BG);
    ui::text(&mut win.fb, 12, 16, "Stria", INK);
    let name = s.path.rsplit('/').next().unwrap_or(&s.path);
    ui::text(&mut win.fb, 12, 40, &ui::fit(name, 33), INK);
    ui::text(&mut win.fb, 12, 56, &alloc::format!("audio: {}", s.a_codec), DIM);
    if let Some(n) = &s.note {
        ui::text(&mut win.fb, 12, 72, &ui::fit(n, 33), DIM);
    }
}

/// Open `path` and say so on the wire (and on the window when it cannot).
fn open(win: &mut Win, path: &str) -> Option<Session> {
    match Session::open(path) {
        Ok(mut s) => {
            let mut l = Line::new(b":: STRIA: open path=");
            l.put(path.as_bytes()).put(b" tracks=v:").put(s.v_codec.as_bytes()).put(b" a:").put(s.a_codec.as_bytes());
            l.put(b" dur_ms=").dec(s.dur_ms as i64).put(b" size=").dec(s.size.0 as i64).put(b"x").dec(s.size.1 as i64);
            l.put(b" bytes=").dec(sys::stat_size(path.as_bytes()).unwrap_or(0) as i64).put(b" ::");
            l.wire();
            if let Some(n) = &s.note {
                Line::new(b":: STRIA: note ").put(ui::fit(n, 120).as_bytes()).put(b" ::").wire();
            }
            ui::fill(&mut win.fb, 0, 0, MAX_W, MAX_H, 0);
            if !s.has_video() {
                paint_sound_only(win, &s);
            }
            s.audio_start(0);
            Some(s)
        }
        Err(r) => {
            Line::new(b":: STRIA: open path=").put(path.as_bytes()).put(b" -> FAIL reason=").put(r.reason.as_bytes()).put(b" ::").wire();
            Line::new(b":: STRIA: end -> FAIL reason=open frames=0 audio=none drift_ms=0 late_max_ms=0 dropped=0 clock=wall ::").wire();
            say(win, "Stria", &[&ui::fit(path, 99), &r.say]);
            None
        }
    }
}

/// A file named on the command line, else the staged sample.
fn first_path() -> Option<String> {
    if let Some(a) = una_abi::args() {
        if let Some(p) = a.get(1).and_then(|b| core::str::from_utf8(b).ok()) {
            return Some(String::from(p));
        }
    }
    TESTF.iter().find(|p| sys::stat_size(p.as_bytes()).is_ok()).map(|p| String::from(*p))
}

/// The mailbox: a Quarry open is a BUS_VERB_NOTICE frame whose body is the file's path.
fn notice_path() -> Option<String> {
    static mut RX: [u8; BUS_FRAME_MAX] = [0; BUS_FRAME_MAX];
    // SAFETY: one thread; nothing else borrows RX.
    let rx = unsafe { &mut *core::ptr::addr_of_mut!(RX) };
    loop {
        let n = sys(SYS_MRECV, rx.as_mut_ptr() as u64, rx.len() as u64, MRECV_NOWAIT, 0);
        if n < BUS_HDR_LEN as i64 {
            return None;
        }
        if rx[0..4] == BUS_MAGIC && rx[5] == BUS_KIND_REPLY && rx[6] == BUS_VERB_NOTICE {
            let blen = u32::from_le_bytes([rx[48], rx[49], rx[50], rx[51]]) as usize;
            let body = &rx[BUS_HDR_LEN..(BUS_HDR_LEN + blen).min(n as usize)];
            if let Ok(p) = core::str::from_utf8(body) {
                if p.starts_with('/') {
                    return Some(String::from(p));
                }
            }
        }
    }
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let _ = &APP_NOTE;
    let base = landmark_base();
    // The window first (a launch during the desktop's own start may meet a busy compositor: retry for 5 s).
    let mut id = sys(SYS_WIN_CREATE, MAX_W as u64, MAX_H as u64, 0, 0);
    let mut tries = 0;
    while id < 0 && tries < 50 {
        sleep_ms(100);
        id = sys(SYS_WIN_CREATE, MAX_W as u64, MAX_H as u64, 0, 0);
        tries += 1;
    }
    if id < 0 {
        Line::new(b":: STRIA: SYS_WIN_CREATE refused code=").dec(id).put(b" ::").wire();
        exit(1);
    }
    let mut win = Win { id, surf: (base + 0x5000) as *mut u32, flags: unsafe { ((base + 0x4000) as *const u32).add(0x20 / 4) }, fb: alloc::vec![BG; ui::W * ui::H] };
    let mut cur: Option<Session> = match first_path() {
        Some(p) => open(&mut win, &p),
        None => {
            say(&mut win, "Stria", &["Open a video or a sound:", "stria <path>, or double-click it in Quarry."]);
            None
        }
    };
    loop {
        let now = now_ms();
        // Input: Esc and the window's close quit.
        let mut k = 0;
        while k < 32 {
            let ev = sys(SYS_INPUT_POLL, 0, 0, 0, 0);
            if ev < 0 {
                break;
            }
            let ev = ev as u64;
            match input_ev_type(ev) {
                INPUT_EV_KEY_DOWN if input_ev_payload(ev) as u8 == KEY_ESC => exit(0),
                INPUT_EV_CLOSE_REQ => exit(0),
                _ => {}
            }
            k += 1;
        }
        if let Some(p) = notice_path() {
            if let Some(mut s) = cur.take() {
                s.audio_stop();
            }
            cur = open(&mut win, &p);
        }
        let Some(s) = cur.as_mut() else {
            sleep_ms(50);
            continue;
        };
        s.clock(now);
        s.feed_audio();
        s.pull_video();
        let mut dirty = s.show_due(&mut win.fb);
        if !s.ended && s.playing && s.done(now) {
            s.ended = true;
            s.playing = false;
            s.end_line();
            s.audio_stop();
            dirty = true;
        }
        if dirty && win.visible() {
            win.present();
        }
        let wait = match s.next {
            Some(t) if s.playing => t.saturating_sub(s.media_ms).clamp(1, 10),
            _ => 10,
        };
        sleep_ms(wait);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    write(b":: STRIA: panic ::\n");
    exit(3)
}

/// EXECNAME (B322, R82): the launch declaration — it opens a window (SYS_WIN_CREATE), so a bare `stria` detaches like
/// `bg`. Kept by the link scripts under a PT_NOTE header; read by `midden_core::app_note_flags`.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(una_abi::APP_FLAG_WINDOWED);
