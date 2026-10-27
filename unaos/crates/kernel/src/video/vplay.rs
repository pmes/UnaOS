// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — fulfiller
//!
//! VIDEOPLAYER (rmbp-ledger B434, MACPARITY row 30). CODEX §2: moving pictures are Stria's. R79: the kernel plays
//! a video as Stria's FULFILLER over the shared cores — `demux_core` (MP4 / Matroska / WebM), `vp8_core` and
//! `av1_core` (the pictures), `audio_core` (the sound, `container::open_demuxed`) — never a second decoder and never a store of
//! its own. This file is the job, the queue, the clock and two adapters; the window is PLAYER's (`video/player.rs`).
//!
//! * THE JOB (the DECJOB shape, DECJOBHANG B386): `play-vdec`, its own task on a worker core (never cpu 0 — the
//!   clock core — never the caller's), a [`STACK`]-byte stack whose high-water is on the wire at exit. It reads the
//!   file, demuxes it, decodes every picture in decode order (VP8 and AV1 keep references: no packet is skipped)
//!   to 0RGB, and parks the frames in a queue of [`QUEUE`]. It never presents.
//! * THE CONSUMER (the Player's pass — one per vblank): [`present`] reads the clock, takes the NEWEST queued frame
//!   whose time has come, counts every older one it passed over as a drop, and blits it nearest-scaled and
//!   letterboxed into the window's video rect. At most one present per pass.
//! * THE CLOCK: the audio ring's position while the container's sound plays (a/v sync by the audio clock); a
//!   pause-aware wall clock without sound (TEST.MP4 is AV1 alone) or after it ends. Until the ring arms (≤ 1.5 s)
//!   the picture waits at its first frame.
//! * SOUND (VPLAYAUDIO B475): the job keeps a `Demuxer::share` of the file it opened; `play-dec` (hda_play.rs) asks
//!   [`shared_audio`] FIRST (no I/O) and takes `audio_core::container::open_demuxed` over that share — one read, one
//!   parse, the bytes shared — for MP4 (AAC, MP3) and Matroska/WebM (Opus, Vorbis) alike, through audio_core's one
//!   `Decoder` (its seek is the container's table: `table=mp4|matroska`). [`container_audio`] is the same door for a
//!   Matroska file opened with no picture job. No decoder adapter lives here (R79).
//! * SEEK (VPLAYAUDIO B475): [`start_at`] — the job keyframe-seeks (`Demuxer::seek`), decodes up to the target
//!   unseen, and the clock starts there; the Player seeks the sound through the same door (player.rs `vid::seek`).
//! * QUICK LOOK: [`poster`] — the first frame and the facts, decoded on a `vposter` job (never on the 32 KiB render
//!   task); Quick Look's pass re-renders when it lands ([`poster_fresh`]).
//!
//! * THE EDL MODE (NLE6, LEDGER SR207): a `.edl` path plays an edit — timeline_core's editor state and schedule over one
//!   open per reel, each run's keyframe seek and unseen run-up on this job, the cut's frames through this queue and clock
//!   (the section at the tail; cut-only, no sound yet).
//!
//! Wire: `[vplay] job spawn|run|exit …`, `[player] video codec=<vp8|av1> size=<w>x<h> fps=<n> …` (player.rs),
//! `:: VIDEOPLAYER: open=ok frames=<n> dropped=<n> fps=<n> av_sync_ms=<n> clock=<audio|wall> -> PASS ::`
//! (`tests videoplayer`, R80: typed). Not here: video seek (SEEKTABLE B433), hardware decode.
//! Design: docs/dev/evidence/rmbp-1005/videoplayer.md.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use demux_core::{Codec, Demuxer};

/// `play-vdec`'s stack. The chains are heap-backed (vp8_core's `Decoder` is 1320 B, av1_core's `StreamDecoder`
/// 280 B, host-measured); the decode frames are the cores' own. 256 KiB = AV1's tile/transform frames plus the
/// VFS read chain with margin; `[vplay] job exit … stack high=` re-derives it on metal.
pub const STACK: usize = 256 * 1024;
/// Frames decoded ahead of the clock.
const QUEUE: usize = 3;
/// The largest file the job reads whole (the demuxer takes the bytes).
const FILE_CAP: u64 = 96 << 20;
/// The ring has this long to arm before the picture runs on the wall clock.
const AUDIO_WAIT_MS: u64 = 1_500;
/// A job whose queue nobody drained for this long (and not paused) ends itself.
const ORPHAN_MS: u64 = 30_000;

/// One decoded picture, 0RGB, at its coded size; `pts_ms` from the track's first packet.
pub struct Frame {
    pub pts_ms: u64,
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
    /// NLE6 (SR207): a utp1 picture's `(output tick, frame ordinal)` in the EDL mode — the presented frame counter the
    /// witness compares with the schedule (the kernel has no utp1 renderer: the pixels are a flat field).
    pub ord: Option<(i64, u32)>,
}

/// What the container says (and the job measured).
#[derive(Clone)]
pub struct Facts {
    pub codec: &'static str,
    pub w: u32,
    pub h: u32,
    /// Frames per 100 s (`2997` = 29.97 fps).
    pub fps_x100: u32,
    pub dur_ms: u64,
    pub frames: u64,
    /// The sound track's codec token, `none` without one.
    pub audio: &'static str,
    /// The sound can be played here: whatever `audio_core::container::plays` (VPLAYAUDIO B475: the one predicate).
    pub audio_ok: bool,
}

impl Facts {
    pub fn fps(&self) -> u32 {
        (self.fps_x100 + 50) / 100
    }
}

fn tok(c: &Codec) -> &'static str {
    match c {
        Codec::Vp8 => "vp8",
        Codec::Av1 => "av1",
        Codec::Vp9 => "vp9",
        Codec::Avc => "avc",
        Codec::Hevc => "hevc",
        Codec::Vorbis => "vorbis",
        Codec::Opus => "opus",
        Codec::Aac => "aac",
        Codec::Mp3 => "mp3",
        Codec::Flac => "flac",
        Codec::Pcm { .. } => "pcm",
        Codec::TestPattern => "utp1",
        Codec::Other(_) => "other",
    }
}

fn facts_of(d: &Demuxer) -> Option<Facts> {
    let v = d.video_track()?;
    let a = d.audio_track();
    let dur_ns = if v.duration_ns > 0 { v.duration_ns } else { d.duration_ns() };
    let fps_x100 = if dur_ns > 0 { (v.sample_count as u128 * 100_000_000_000 / dur_ns as u128) as u32 } else { 0 };
    let audio_ok = a.is_some_and(|t| audio_core::container::plays(d.format(), &t.codec)); // VPLAYAUDIO (B475): the Decoder's own predicate
    Some(Facts {
        codec: tok(&v.codec),
        w: v.width,
        h: v.height,
        fps_x100,
        dur_ms: d.duration_ns() / 1_000_000,
        frames: v.sample_count,
        audio: a.map(|t| tok(&t.codec)).unwrap_or("none"),
        audio_ok,
    })
}

/// The whole file (the demuxer takes the bytes). I/O: a job's, never the router's.
fn read_all(path: &str) -> Result<Vec<u8>, String> {
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(path).map_err(|e| alloc::format!("stat: {:?}", e))?;
    if st.size > FILE_CAP {
        return Err(alloc::format!("file {} bytes over the {} cap", st.size, FILE_CAP));
    }
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve_exact(st.size as usize).map_err(|_| String::from("out of memory"))?;
    while (out.len() as u64) < st.size {
        let want = ((st.size - out.len() as u64) as usize).min(256 * 1024);
        let got = mt.read(path, out.len() as u64, want).map_err(|e| alloc::format!("read: {:?}", e))?;
        if got.is_empty() {
            break;
        }
        out.extend_from_slice(&got);
    }
    Ok(out)
}

// ── the picture decoders (the shared cores, behind one enum) ────────────────────────────────────────────────

enum Pic {
    Vp8(Box<vp8_core::Decoder>),
    Av1(Box<av1_core::image::StreamDecoder>),
    /// NLE6 (SR207): demux_core's `utp1` test stream — its packet carries its ordinal (`build::TestPatternPacket`).
    Utp1,
}

impl Pic {
    fn new(t: &demux_core::Track) -> Result<Pic, String> {
        match t.codec {
            Codec::Vp8 => Ok(Pic::Vp8(Box::new(vp8_core::Decoder::new()))),
            Codec::Av1 => av1_core::image::StreamDecoder::new(&t.config).map(|d| Pic::Av1(Box::new(d))).map_err(|e| alloc::format!("av1: {:?}", e)),
            Codec::TestPattern => Ok(Pic::Utp1),
            ref c => Err(alloc::format!("no decoder for {} in this tree (vp8, av1, utp1)", tok(c))),
        }
    }

    /// One packet → the picture it shows (`None`: an alt-ref / dropped frame that shows nothing).
    fn decode(&mut self, data: &[u8]) -> Result<Option<(usize, usize, Vec<u32>)>, String> {
        match self {
            Pic::Vp8(d) => {
                if data.is_empty() {
                    return Ok(None);
                }
                let pic = match d.decode(data) {
                    Ok(Some(p)) => p,
                    Ok(None) => return Ok(None),
                    Err(e) => return Err(alloc::format!("vp8: {:?}", e)),
                };
                let y = vp8_core::Yuv420::from_picture(&pic);
                let rgba = vp8_core::yuv::to_rgba(&y);
                Ok(Some((y.width as usize, y.height as usize, pack(&rgba))))
            }
            Pic::Av1(d) => {
                let planes = d.decode_temporal_unit(data).map_err(|e| alloc::format!("av1: {:?}", e))?;
                let img = av1_core::image::planes_to_rgba(&planes, av1_core::image::Upsampling::Bilinear);
                Ok(Some((img.w as usize, img.h as usize, pack(&img.rgba))))
            }
            Pic::Utp1 => {
                // the stream's own decoder: the packet names its frame; the picture is a flat field whose level steps
                // with the ordinal (the host's pattern renderer is gneiss_pal's; NLE6-UTP1 owes its move to a core)
                let t = demux_core::build::TestPatternPacket::decode(data).ok_or_else(|| String::from("utp1: not a UTP1 packet"))?;
                let (w, h) = ((t.width as usize).max(1), (t.height as usize).max(1));
                let l = 28 + t.frame % 200;
                Ok(Some((w, h, alloc::vec![(l << 16) | (l << 8) | l; w * h])))
            }
        }
    }

    /// NLE6 (SR207): the frame ordinal a utp1 packet carries (`None` for a coded picture).
    fn ordinal(&self, data: &[u8]) -> Option<u32> {
        match self {
            Pic::Utp1 => demux_core::build::TestPatternPacket::decode(data).map(|t| t.frame),
            _ => None,
        }
    }
}

fn pack(rgba: &[u8]) -> Vec<u32> {
    rgba.chunks_exact(4).map(|p| ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32).collect()
}

/// Nearest-scale `f` into the `bw x bh` rect at `(x, y)` of a `stride`-wide surface, aspect kept, the bars
/// `bar`. Returns the picture's drawn size.
pub fn blit_fit(dst: &mut [u32], stride: usize, x: usize, y: usize, bw: usize, bh: usize, f: &Frame, bar: Option<u32>) -> (usize, usize) {
    if f.w == 0 || f.h == 0 || bw == 0 || bh == 0 {
        return (0, 0);
    }
    let (dw, dh) = if bw * f.h <= bh * f.w { (bw, (f.h * bw / f.w).max(1)) } else { ((f.w * bh / f.h).max(1), bh) };
    let (ox, oy) = (x + (bw - dw) / 2, y + (bh - dh) / 2);
    for r in 0..bh {
        let row = (y + r) * stride;
        if row + x + bw > dst.len() {
            break;
        }
        let inside = r + y >= oy && r + y < oy + dh;
        if !inside {
            if let Some(c) = bar {
                dst[row + x..row + x + bw].fill(c);
            }
            continue;
        }
        let sy = ((r + y - oy) * f.h / dh).min(f.h - 1);
        let src = &f.px[sy * f.w..sy * f.w + f.w];
        for c in 0..bw {
            let px = x + c;
            if px >= ox && px < ox + dw {
                dst[row + px] = src[((px - ox) * f.w / dw).min(f.w - 1)];
            } else if let Some(b) = bar {
                dst[row + px] = b;
            }
        }
    }
    (dw, dh)
}

// ── the job ─────────────────────────────────────────────────────────────────────────────────────────────────

struct Job {
    jid: u32,
    path: String,
    facts: Option<Facts>,
    queue: VecDeque<Frame>,
    decoded: u64,
    err: Option<String>,
    eos: bool,
    /// VPLAYAUDIO (B475): the opened file's share for the sound (`shared_audio`), and where the picture starts.
    sound: Option<Demuxer>,
    seek_ms: u64,
}

static JOB: crate::sync::Mutex<Option<Job>> = crate::sync::Mutex::new(None);
static GEN: AtomicU32 = AtomicU32::new(0);
static LIVE: AtomicBool = AtomicBool::new(false);
static SPAWN_MS: AtomicU64 = AtomicU64::new(0);
static DRAIN_MS: AtomicU64 = AtomicU64::new(0);
static PAUSED: AtomicBool = AtomicBool::new(false);

/// The consumer's counters and clock (one player).
struct Play {
    presented: u64,
    dropped: u64,
    sync_sum: u64,
    sync_n: u64,
    sync_max: u64,
    /// Presents timed by the audio ring / by the wall.
    by_audio: u64,
    by_wall: u64,
    media_ms: u64,
    wall_ms: u64,
    open_ms: u64,
    audio_expected: bool,
    audio_seen: bool,
    /// NLE6 (SR207): `(output tick, ordinal)` of each presented frame that carries one (at most [`SHOWN_CAP`]).
    shown: Vec<(i64, u32)>,
}

static PLAY: crate::sync::Mutex<Play> = crate::sync::Mutex::new(Play::new());

impl Play {
    const fn new() -> Play {
        Play { presented: 0, dropped: 0, sync_sum: 0, sync_n: 0, sync_max: 0, by_audio: 0, by_wall: 0, media_ms: 0, wall_ms: 0, open_ms: 0, audio_expected: false, audio_seen: false, shown: Vec::new() }
    }
}

fn now() -> u64 {
    crate::arch::ms()
}

/// `play-vdec`'s core: the LAST worker-pool core that is neither the caller's nor cpu 0 (`play-dec` takes the
/// first); `CPU_AUTO` (named `on=auto`) only when the pool has none.
fn job_cpu() -> (usize, &'static str) {
    let here = crate::arch::percpu::this_cpu().cpu_index as usize;
    let mut pick = None;
    for n in 0..crate::arch::gdt::MAX_CPUS {
        match crate::arch::smp::worker_cpu(n) {
            Some(c) if c != here && c != 0 => pick = Some(c),
            Some(_) => continue,
            None => break,
        }
    }
    match pick {
        Some(c) => (c, "worker"),
        None => (crate::arch::sched::CPU_AUTO, "auto"),
    }
}

fn spawn(name: &'static str, entry: fn(usize), jid: u32, path: &str) {
    let (cpu, on) = job_cpu();
    serial_println!(
        "[vplay] job spawn task={} jid={} path={} stack={} cpu={} on={}",
        name, jid, path, STACK, if cpu == crate::arch::sched::CPU_AUTO { -1 } else { cpu as i64 }, on
    );
    crate::arch::sched::spawn_stack(name, entry, jid as usize, cpu, crate::arch::sched::PRIO_NORMAL, STACK);
}

/// **Start playing `path`'s pictures** (replaces any job). The window's pass calls [`present`] from here on.
pub fn start(path: &str) -> u32 {
    start_at(path, 0)
}

/// VPLAYAUDIO (B475): **start playing `path`'s pictures at `ms`** — the job seeks to the last keyframe at or before
/// it (`Demuxer::seek`, the container's own table), decodes the frames before `ms` without queueing them, and the
/// clock starts at `ms`.
pub fn start_at(path: &str, ms: u64) -> u32 {
    stop();
    let jid = GEN.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    *JOB.lock() = Some(Job { jid, path: String::from(path), facts: None, queue: VecDeque::new(), decoded: 0, err: None, eos: false, sound: None, seek_ms: ms });
    let t = now();
    *PLAY.lock() = Play { open_ms: t, wall_ms: t, media_ms: ms, ..Play::new() };
    PAUSED.store(false, Ordering::Release);
    SPAWN_MS.store(t, Ordering::Release);
    DRAIN_MS.store(t, Ordering::Release);
    LIVE.store(true, Ordering::Release);
    if is_edl(path) { spawn("play-vedl", edl_task, jid, path) } else { spawn("play-vdec", job_task, jid, path) } // NLE6 (SR207): an EDL plays through the same job, queue and clock
    jid
}

/// End the job (the window closed, a replace, F9). A job mid-decode sees it at its next packet.
pub fn stop() {
    GEN.fetch_add(1, Ordering::AcqRel);
    if let Some(j) = JOB.lock().as_mut() {
        j.queue.clear();
    }
}

pub fn facts() -> Option<Facts> {
    JOB.lock().as_ref().and_then(|j| j.facts.clone())
}

pub fn err() -> Option<String> {
    JOB.lock().as_ref().and_then(|j| j.err.clone())
}

/// The job is done and every frame it made has been presented or dropped.
pub fn finished() -> bool {
    let g = JOB.lock();
    match g.as_ref() {
        Some(j) => (j.eos || j.err.is_some()) && j.queue.is_empty() && !LIVE.load(Ordering::Acquire),
        None => true,
    }
}

/// The picture's clock is paused (the Player's pause): the wall clock stops, a queue at rest is not an orphan.
pub fn pause(on: bool) {
    PAUSED.store(on, Ordering::Release);
    PLAY.lock().wall_ms = now();
}

/// The Player opened the container's sound (or could not): the clock waits for the ring when it did.
pub fn audio_expected(on: bool) {
    let mut p = PLAY.lock();
    p.audio_expected = on;
    p.open_ms = now();
}

/// `(decoded, presented, dropped, av_sync_ms mean, av_sync_ms max, clock)` — the witness's numbers.
pub fn stats() -> (u64, u64, u64, u64, u64, &'static str) {
    let decoded = JOB.lock().as_ref().map(|j| j.decoded).unwrap_or(0);
    let p = PLAY.lock();
    let clock = if p.by_audio > 0 && p.by_audio >= p.by_wall { "audio" } else if p.by_audio > 0 { "audio+wall" } else { "wall" };
    (decoded, p.presented, p.dropped, p.sync_sum / p.sync_n.max(1), p.sync_max, clock)
}

/// The Player's pass (one per vblank): advance the clock (`audio_ms` = the ring's position while the
/// container's sound runs), present at most ONE frame into `dst`'s `w x vh` video rect, count the frames passed
/// over. Returns `(presented this pass, the clock in ms)`.
pub fn present(dst: &mut [u32], w: usize, vh: usize, audio_ms: Option<u64>, playing: bool) -> (bool, u64) {
    let t = now();
    let media = {
        let mut p = PLAY.lock();
        let dt = t.saturating_sub(p.wall_ms);
        p.wall_ms = t;
        if playing {
            if let Some(a) = audio_ms {
                p.audio_seen = true;
                p.media_ms = a.max(p.media_ms.saturating_sub(40)); // the ring's position; a 40 ms re-read jitter never runs the picture back
            } else if p.audio_expected && !p.audio_seen && t.saturating_sub(p.open_ms) < AUDIO_WAIT_MS {
                // the ring is arming: the picture holds its first frame
            } else {
                p.media_ms += dt;
            }
        }
        p.media_ms
    };
    if !playing {
        DRAIN_MS.store(t, Ordering::Release);
    }
    let frame = {
        let Some(mut g) = JOB.try_lock() else { return (false, media) };
        let Some(j) = g.as_mut() else { return (false, media) };
        let mut take: Option<Frame> = None;
        let mut passed = 0u64;
        while j.queue.front().map(|f| f.pts_ms <= media).unwrap_or(false) {
            if take.is_some() {
                passed += 1;
            }
            take = j.queue.pop_front();
        }
        if take.is_some() {
            DRAIN_MS.store(t, Ordering::Release);
        }
        let mut p = PLAY.lock();
        p.dropped += passed;
        take
    };
    let Some(f) = frame else { return (false, media) };
    let skew = media.abs_diff(f.pts_ms);
    {
        let mut p = PLAY.lock();
        p.presented += 1;
        p.sync_sum += skew;
        p.sync_n += 1;
        p.sync_max = p.sync_max.max(skew);
        if audio_ms.is_some() { p.by_audio += 1 } else { p.by_wall += 1 }
        if let Some(o) = f.ord.filter(|_| p.shown.len() < SHOWN_CAP) {
            p.shown.push(o); // NLE6 (SR207): the presented frame counter
        }
    }
    blit_fit(dst, w, 0, 0, w, vh, &f, Some(0));
    (true, media)
}

/// The first frame for the window (before the clock runs), when the job has one queued.
pub fn first_frame_into(dst: &mut [u32], w: usize, vh: usize) -> bool {
    let g = JOB.lock();
    let Some(f) = g.as_ref().and_then(|j| j.queue.front()) else { return false };
    blit_fit(dst, w, 0, 0, w, vh, f, Some(0));
    true
}

fn job_task(arg: usize) {
    let jid = arg as u32;
    let paint = stack_paint();
    let (path, seek_ms) = match JOB.lock().as_ref() {
        Some(j) if j.jid == jid => (j.path.clone(), j.seek_ms),
        _ => return job_exit(jid, paint, "superseded", 0, 0),
    };
    serial_println!("[vplay] job run jid={} cpu={} wait_ms={}", jid, crate::arch::percpu::this_cpu().cpu_index, now().saturating_sub(SPAWN_MS.load(Ordering::Acquire)));
    let fail = |e: String| {
        if let Some(j) = JOB.lock().as_mut().filter(|j| j.jid == jid) {
            j.err = Some(e);
        }
    };
    let bytes = match read_all(&path) {
        Ok(b) => b,
        Err(e) => {
            fail(e);
            return job_exit(jid, paint, "read-refused", 0, 0);
        }
    };
    let mut d = match Demuxer::open(bytes) {
        Ok(d) => d,
        Err(e) => {
            fail(alloc::format!("{}", e));
            return job_exit(jid, paint, "demux-refused", 0, 0);
        }
    };
    let Some(vt) = d.video_track().cloned() else {
        fail(String::from("no video track"));
        return job_exit(jid, paint, "no-video", 0, 0);
    };
    let f = facts_of(&d);
    let sound = f.as_ref().filter(|f| f.audio_ok).map(|_| d.share()); // VPLAYAUDIO (B475): the sound's door, same bytes
    if let Some(j) = JOB.lock().as_mut().filter(|j| j.jid == jid) {
        j.facts = f;
        j.sound = sound;
    }
    // the clock's zero: the picture track's first presentation time (a seek keeps it)
    let vpts0 = d.track_index(vt.id).and_then(|i| d.track_samples(i).map(|s| s.pts).min());
    if seek_ms > 0 {
        let key = d.seek((seek_ms as i64).saturating_mul(1_000_000)).map(|k| k.saturating_sub(vt.to_ns(vpts0.unwrap_or(0))).max(0) as u64 / 1_000_000);
        serial_println!("[vplay] seek jid={} to_ms={} key_ms={} table=container", jid, seek_ms, key.map(|k| k as i64).unwrap_or(-1));
    }
    let mut pic = match Pic::new(&vt) {
        Ok(p) => p,
        Err(e) => {
            fail(e);
            return job_exit(jid, paint, "codec-refused", 0, 0);
        }
    };
    let t0 = now();
    let (mut decoded, mut errs) = (0u64, 0u64);
    let mut first_pts: Option<i64> = vpts0;
    let mut prefixed: Vec<u8> = Vec::new();
    while let Some(p) = d.next_packet() {
        if p.track != vt.id {
            continue;
        }
        let data: &[u8] = if vt.frame_prefix.is_empty() {
            &p.data
        } else {
            prefixed.clear();
            prefixed.extend_from_slice(&vt.frame_prefix);
            prefixed.extend_from_slice(&p.data);
            &prefixed
        };
        // the bounded queue: decode no further ahead than QUEUE frames
        if let Err(why) = room(jid) {
            return job_exit(jid, paint, why, decoded, errs);
        }
        let out = pic.decode(data);
        let fp = *first_pts.get_or_insert(p.pts);
        let pts_ms = (vt.to_ns(p.pts - fp).max(0) as u64) / 1_000_000;
        match out {
            Ok(Some(_)) if pts_ms < seek_ms => decoded += 1, // VPLAYAUDIO: a seek's run-up from the keyframe, unseen
            Ok(Some((w, h, px))) => {
                decoded += 1;
                let mut g = JOB.lock();
                let Some(j) = g.as_mut().filter(|j| j.jid == jid) else { drop(g); return job_exit(jid, paint, "superseded", decoded, errs) };
                j.decoded = decoded;
                j.queue.push_back(Frame { pts_ms, w, h, px, ord: None });
            }
            Ok(None) => {}
            Err(e) => {
                errs += 1;
                if errs <= 3 {
                    serial_println!("[vplay] decode error jid={} pts_ms={} {}", jid, pts_ms, e);
                }
            }
        }
        crate::arch::sched::yield_now();
    }
    if let Some(j) = JOB.lock().as_mut().filter(|j| j.jid == jid) {
        j.eos = true;
        if errs > 0 && decoded == 0 {
            j.err = Some(alloc::format!("{} decode errors, no picture", errs));
        }
    }
    serial_println!("[vplay] job decode jid={} frames={} err={} ms={}", jid, decoded, errs, now().saturating_sub(t0));
    job_exit(jid, paint, "eos", decoded, errs)
}

/// The bounded queue: wait until job `jid` may queue one more frame (at most [`QUEUE`] ahead of the clock). `Err` ends
/// the job: `aborted` (superseded) or `consumer-gone` (nobody drained the queue for [`ORPHAN_MS`], not paused). Shared
/// by [`job_task`] and the EDL mode's [`edl_task`] (NLE6).
fn room(jid: u32) -> Result<(), &'static str> {
    loop {
        if GEN.load(Ordering::Acquire) != jid {
            return Err("aborted");
        }
        let q = JOB.lock().as_ref().map(|j| j.queue.len()).unwrap_or(0);
        if q < QUEUE {
            return Ok(());
        }
        if now().saturating_sub(DRAIN_MS.load(Ordering::Acquire)) > ORPHAN_MS && !PAUSED.load(Ordering::Acquire) {
            return Err("consumer-gone");
        }
        crate::arch::sched::sleep_ms(2);
    }
}

fn job_exit(jid: u32, paint: Option<(u64, u64)>, why: &str, decoded: u64, errs: u64) {
    let high = stack_high(paint);
    serial_println!("[vplay] job exit jid={} why={} decoded={} err={} stack high={} of {}", jid, why, decoded, errs, high, STACK);
    if GEN.load(Ordering::Acquire) == jid {
        LIVE.store(false, Ordering::Release); // a straggler of an older jid never releases the current one
    }
}

const PAINT: u8 = 0xA7;

/// Paint this task's unused stack below the live frame, so [`stack_high`] reads the deepest point reached (the
/// DECJOB measurement, `hda_play::dec_stack_paint`'s arithmetic).
fn stack_paint() -> Option<(u64, u64)> {
    let (low, top) = crate::arch::sched::current_stack_bounds()?;
    let marker = 0u8;
    let here = &marker as *const u8 as u64;
    if here <= low + 2048 || here > top {
        return None;
    }
    let end = here - 1024;
    let mut a = low;
    while a < end {
        // SAFETY: [low, here - 1 KiB) is this task's own stack below its live frame (the scheduler's bounds for
        // the running task); nothing lives there yet, and a later interrupt frame overwriting the paint is what the
        // reading counts.
        unsafe { core::ptr::write_volatile(a as *mut u8, PAINT) };
        a += 1;
    }
    Some((low, top))
}

fn stack_high(paint: Option<(u64, u64)>) -> u64 {
    let Some((low, top)) = paint else { return 0 };
    let mut a = low;
    // SAFETY: the span `stack_paint` wrote, on this task's own stack, read below the live frame.
    while a < top && unsafe { core::ptr::read_volatile(a as *const u8) } == PAINT {
        a += 1;
    }
    top - a
}

// ── the container's sound (VPLAYAUDIO B475): audio_core's one door over the job's Demuxer ─────────────────────

/// `[vplay] sound …` and the `Source` — `audio_core::container::open_demuxed` (MP4 AAC/MP3, Matroska Opus/Vorbis).
fn sound_of(d: Demuxer, parse: &str) -> Option<Box<dyn audio_core::Source>> {
    let t = d.audio_track()?.clone();
    let container = if d.format() == demux_core::Format::Mp4 { "mp4" } else { "matroska" };
    match audio_core::container::open_demuxed(d) {
        Ok(s) => {
            let i = s.info();
            serial_println!(
                "[vplay] sound container={} codec={} rate={} ch={} packets={} delay_ns={} parse={} -> play-dec",
                container, tok(&t.codec), i.rate, i.channels, t.sample_count, t.codec_delay_ns, parse
            );
            Some(s)
        }
        Err(e) => {
            serial_println!("[vplay] sound container={} codec={} -> none ({:?})", container, tok(&t.codec), e);
            None
        }
    }
}

/// `play-dec`'s FIRST door (hda_play.rs): the live picture job's share of `path` (no I/O, no second parse). `None`
/// when no job plays `path` or its sound is not one audio_core plays.
/// A seek mints the picture job and the sound job together: the sound waits (on its own task, ≤ [`SHARE_WAIT_MS`])
/// while the picture job is still reading and parsing.
pub fn shared_audio(path: &str) -> Option<Box<dyn audio_core::Source>> {
    let t0 = now();
    let d = loop {
        {
            let g = JOB.lock();
            let j = g.as_ref().filter(|j| j.path == path && j.jid == GEN.load(Ordering::Acquire))?;
            if let Some(d) = j.sound.as_ref() {
                break d.share();
            }
            if j.facts.is_some() || j.err.is_some() || !LIVE.load(Ordering::Acquire) {
                return None;
            }
        }
        if now().saturating_sub(t0) > SHARE_WAIT_MS {
            return None;
        }
        crate::arch::sched::sleep_ms(5);
    };
    sound_of(d, "shared")
}

/// How long `play-dec` waits for a just-minted picture job's parse before reading the file itself.
const SHARE_WAIT_MS: u64 = 3_000;

/// `play-dec`'s last door: a Matroska/WebM file `audio_core` does not sniff, opened with no picture job — one read,
/// the same `open_demuxed`. `None` for anything else (the play is refused by audio_core's own reason).
pub fn container_audio(path: &str) -> Option<Box<dyn audio_core::Source>> {
    let bytes = read_all(path).ok()?;
    if demux_core::probe(&bytes) != Some(demux_core::Format::Matroska) {
        return None;
    }
    sound_of(Demuxer::open(bytes).ok()?, "own")
}

// ── Quick Look's poster: the first frame and the facts, on a job ─────────────────────────────────────────────

struct Poster {
    jid: u32,
    path: String,
    facts: Option<Facts>,
    frame: Option<Frame>,
    err: Option<String>,
    done: bool,
}

static POSTER: crate::sync::Mutex<Option<Poster>> = crate::sync::Mutex::new(None);
static POSTER_GEN: AtomicU32 = AtomicU32::new(0);
static POSTER_FRESH: AtomicBool = AtomicBool::new(false);

/// What Quick Look shows for `path`: `Some((facts, err, frame-drawn))` once the poster job is done (the frame is
/// drawn into `dst`'s `(x, y, bw, bh)` rect), `None` while it runs — the first call starts it.
pub fn poster(path: &str, dst: &mut [u32], stride: usize, x: usize, y: usize, bw: usize, bh: usize) -> Option<(Option<Facts>, Option<String>, bool)> {
    {
        let g = POSTER.lock();
        if let Some(p) = g.as_ref().filter(|p| p.path == path) {
            if !p.done {
                return None;
            }
            let drawn = p.frame.as_ref().map(|f| blit_fit(dst, stride, x, y, bw, bh, f, None).0 > 0).unwrap_or(false);
            return Some((p.facts.clone(), p.err.clone(), drawn));
        }
    }
    let jid = POSTER_GEN.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    *POSTER.lock() = Some(Poster { jid, path: String::from(path), facts: None, frame: None, err: None, done: false });
    spawn("vposter", poster_task, jid, path);
    None
}

/// A poster landed since the last ask (Quick Look's pass re-renders once).
pub fn poster_fresh() -> bool {
    POSTER_FRESH.swap(false, Ordering::AcqRel)
}

fn poster_task(arg: usize) {
    let jid = arg as u32;
    let paint = stack_paint();
    let Some(path) = POSTER.lock().as_ref().filter(|p| p.jid == jid).map(|p| p.path.clone()) else { return };
    let t0 = now();
    let r: Result<(Option<Facts>, Option<Frame>), String> = (|| {
        let d = Demuxer::open(read_all(&path)?).map_err(|e| alloc::format!("{}", e))?;
        let vt = d.video_track().cloned().ok_or_else(|| String::from("no video track"))?;
        let f = facts_of(&d);
        let mut pic = Pic::new(&vt)?;
        for s in d.track_samples(d.track_index(vt.id).unwrap_or(0)).take(8) {
            let pk = d.packet_at(s);
            if let Some((w, h, px)) = pic.decode(&pk.data)? {
                return Ok((f, Some(Frame { pts_ms: 0, w, h, px, ord: None })));
            }
        }
        Ok((f, None))
    })();
    let high = stack_high(paint);
    let mut g = POSTER.lock();
    let Some(p) = g.as_mut().filter(|p| p.jid == jid) else { return };
    match r {
        Ok((f, fr)) => {
            serial_println!(
                "[vplay] poster path={} codec={} frame={} ms={} stack high={} of {}",
                path, f.as_ref().map(|f| f.codec).unwrap_or("-"), fr.as_ref().map(|f| alloc::format!("{}x{}", f.w, f.h)).unwrap_or_else(|| String::from("none")),
                now().saturating_sub(t0), high, STACK
            );
            p.facts = f;
            p.frame = fr;
        }
        Err(e) => {
            serial_println!("[vplay] poster path={} refused ({})", path, e);
            p.err = Some(e);
        }
    }
    p.done = true;
    POSTER_FRESH.store(true, Ordering::Release);
}

/// Quick Look's fact lines for a poster's facts.
pub fn fact_lines(f: &Facts) -> [String; 2] {
    [
        alloc::format!("Video: {}  {}x{}  {} fps", f.codec.to_ascii_uppercase(), f.w, f.h, f.fps()),
        alloc::format!("{}.{} s, sound: {}", f.dur_ms / 1000, (f.dur_ms % 1000) / 100, if f.audio == "none" { "none" } else { f.audio }),
    ]
}

// ── NLE6 (LEDGER SR207): the EDL mode — Stria's editor played through this job, queue, clock and sink ──────────────
//
// The Player opens a `.edl` path like a video: [`start_at`] hands it to [`edl_task`] on the same JOB / GEN / LIVE.
// The EDL is opened as timeline_core's ONE editor state (`editor::EditorState::open`, the model the host face edits),
// each reel's file is read and demuxed ONCE (`<the EDL's dir>/<REEL>.MP4|.WEBM|.MKV`), and THE schedule
// (`timeline_core::schedule::schedule`) is walked: a video step that carries `seek` (or cuts to another reel) opens a
// run at the keyframe (`Demuxer::samples_from`, demux_core's one `key_at_or_before`) on a fresh [`Pic`]; the run-up
// (`preroll`) is decoded unseen; each step's picture is queued at its output tick through [`room`], a gap as black.
// [`present`] shows them on the Player's clock. Cut-only: a transition shows its incoming clip; the audio steps are
// counted, not played (audio_core::container has no PCM door; NLE6-MIX). Wire: `[nle] reel …`, `[nle] open edl=<path>
// reels=<n> clips=<n> steps=<n>`, `[nle] job exit frames=<n> seeks=<n> runup=<n> gaps=<n>`; the witness is
// player.rs `tests nle` (`:: NLE: … ::`).

/// The `NLE` report's plan cap (output ticks remembered for the witness) and the presented-counter cap.
const SHOWN_CAP: usize = 4096;

/// The Player opens `path` in the EDL mode (a `.edl` name, any case).
pub fn is_edl(path: &str) -> bool {
    let n = path.len();
    n > 4 && path.is_char_boundary(n - 4) && path[n - 4..].eq_ignore_ascii_case(".edl")
}

/// One reel: its file opened once (the demuxer holds the bytes) and its streams as `(track index, origin ns)`; source
/// time 0 is a stream's first presentation time (vplay's rule, stria::nle's too).
struct Reel {
    id: timeline_core::SourceId,
    d: Demuxer,
    v: Option<(usize, demux_core::Track, i64)>,
    a: Option<(usize, i64)>,
}

fn origin(d: &Demuxer, i: usize) -> i64 {
    let tb = d.tracks()[i].timebase;
    d.track_samples(i).map(|s| tb.to_ns(s.pts)).min().unwrap_or(0)
}

/// The schedule's `SourceIndex` over the reels: the kernel's counterpart of `stria::nle::Sources`, answered by
/// demux_core's pure keyframe read (`Demuxer::keyframe_before`).
struct Reels {
    tb: timeline_core::Timebase,
    reels: Vec<Reel>,
}

impl Reels {
    fn get(&self, id: timeline_core::SourceId) -> Option<&Reel> {
        self.reels.iter().find(|r| r.id == id)
    }
}

impl timeline_core::SourceIndex for Reels {
    fn duration(&self, src: timeline_core::SourceId) -> Option<i64> {
        let r = self.get(src)?;
        let ns = [r.v.as_ref().map(|v| v.0), r.a.map(|a| a.0)].into_iter().flatten().map(|i| r.d.tracks()[i].duration_ns).max()?;
        // whole ticks only (1 µs of slack for a time base's rounding), stria::nle::Sources' arithmetic
        Some(((ns as i128 + 1_000) * self.tb.den as i128 / (self.tb.num.max(1) as i128 * 1_000_000_000)) as i64)
    }
    fn keyframe_ns(&self, src: timeline_core::SourceId, kind: timeline_core::Kind, t_ns: i64) -> Option<i64> {
        let r = self.get(src)?;
        let (i, o) = match kind {
            timeline_core::Kind::Video => r.v.as_ref().map(|v| (v.0, v.2))?,
            timeline_core::Kind::Audio => r.a?,
        };
        r.d.keyframe_before(i, o + t_ns).map(|(_, p)| p - o)
    }
}

/// What the EDL job did (the witness reads it after [`finished`]).
#[derive(Clone, Default)]
pub struct EdlReport {
    pub reels: usize,
    pub clips: usize,
    /// Schedule steps, every kind.
    pub steps: usize,
    /// Frames queued (pictures and gaps).
    pub frames: u64,
    /// Runs opened at a keyframe.
    pub seeks: u64,
    /// Pictures decoded unseen (the run-up from a keyframe).
    pub runup: u64,
    pub gaps: u64,
    /// Per video step: `(output tick, (source, source tick), carries a seek)`; at most [`SHOWN_CAP`].
    pub plan: Vec<(i64, Option<(u32, i64)>, bool)>,
}

static NLE: crate::sync::Mutex<Option<EdlReport>> = crate::sync::Mutex::new(None);

/// The last EDL job's report (`None` before one ran to its end).
pub fn edl_report() -> Option<EdlReport> {
    NLE.lock().clone()
}

/// `(output tick, ordinal)` of the frames presented since the last start (utp1 pictures only).
pub fn shown() -> Vec<(i64, u32)> {
    PLAY.lock().shown.clone()
}

fn edl_task(arg: usize) {
    let jid = arg as u32;
    let paint = stack_paint();
    let (path, seek_ms) = match JOB.lock().as_ref() {
        Some(j) if j.jid == jid => (j.path.clone(), j.seek_ms),
        _ => return job_exit(jid, paint, "superseded", 0, 0),
    };
    serial_println!("[vplay] job run jid={} cpu={} wait_ms={} mode=edl", jid, crate::arch::percpu::this_cpu().cpu_index, now().saturating_sub(SPAWN_MS.load(Ordering::Acquire)));
    *NLE.lock() = None;
    let mut rep = EdlReport::default();
    let r = edl_run(jid, &path, seek_ms, &mut rep);
    serial_println!("[nle] job exit frames={} seeks={} runup={} gaps={}", rep.frames, rep.seeks, rep.runup, rep.gaps);
    let frames = rep.frames;
    *NLE.lock() = Some(rep);
    let why = match r {
        Ok(()) => {
            if let Some(j) = JOB.lock().as_mut().filter(|j| j.jid == jid) {
                j.eos = true;
            }
            "eos"
        }
        Err((why, e)) => {
            if let (Some(e), Some(j)) = (e, JOB.lock().as_mut().filter(|j| j.jid == jid)) {
                serial_println!("[nle] refused edl={} why={} ({})", path, why, e);
                j.err = Some(e);
            }
            why
        }
    };
    job_exit(jid, paint, why, frames, 0)
}

/// The EDL job's body (see the section head). `Err((why, reason))`: `reason` is the Player's info line, `None` when the
/// job was superseded.
fn edl_run(jid: u32, path: &str, seek_ms: u64, rep: &mut EdlReport) -> Result<(), (&'static str, Option<String>)> {
    use timeline_core::Kind;
    let bytes = read_all(path).map_err(|e| ("read-refused", Some(e)))?;
    let text = core::str::from_utf8(&bytes).map_err(|_| ("edl-refused", Some(String::from("the EDL is not UTF-8 text"))))?;
    let st = timeline_core::editor::EditorState::open(text, 25).map_err(|e| ("edl-refused", Some(alloc::format!("edl: {}", e))))?;
    let tl = &st.timeline;
    let dir = path.rfind('/').map(|i| &path[..i]).unwrap_or("");
    let mt = crate::shell::vfs_mount_table();
    let mut reels = Reels { tb: tl.tb, reels: Vec::new() };
    for s in &tl.sources {
        let Some(rp) = [".MP4", ".WEBM", ".MKV"].iter().map(|x| alloc::format!("{}/{}{}", dir, s.reel, x)).find(|p| mt.stat(p).is_ok()) else {
            return Err(("reel-missing", Some(alloc::format!("no file for reel {} beside the EDL ({}/{}.MP4|.WEBM|.MKV)", s.reel, dir, s.reel))));
        };
        let d = Demuxer::open(read_all(&rp).map_err(|e| ("read-refused", Some(e)))?).map_err(|e| ("demux-refused", Some(alloc::format!("{}: {}", rp, e))))?;
        let v = d.video_track().and_then(|t| d.track_index(t.id).map(|i| (i, t.clone()))).map(|(i, t)| (i, t, origin(&d, i)));
        let a = d.audio_track().and_then(|t| d.track_index(t.id)).map(|i| (i, origin(&d, i)));
        serial_println!(
            "[nle] reel {} path={} codec={} frames={}",
            s.reel, rp, v.as_ref().map(|v| tok(&v.1.codec)).unwrap_or("none"), v.as_ref().map(|v| v.1.sample_count).unwrap_or(0)
        );
        reels.reels.push(Reel { id: s.id, d, v, a });
    }
    let steps = timeline_core::schedule::schedule(tl, 0..tl.end(), &reels).map_err(|e| ("schedule-refused", Some(alloc::format!("schedule: {}", e))))?;
    rep.reels = reels.reels.len();
    rep.clips = tl.tracks.iter().map(|t| t.clips.len()).sum();
    rep.steps = steps.len();
    serial_println!("[nle] open edl={} reels={} clips={} steps={}", path, rep.reels, rep.clips, rep.steps);
    let audio = steps.iter().filter(|s| s.kind == Kind::Audio).count();
    if audio > 0 {
        serial_println!("[nle] audio steps={} -> owed (audio_core::container has no PCM door; NLE6-MIX)", audio);
    }
    // the facts: the first scheduled video reel sizes the picture (and the gaps); the EDL's rate is the clock's
    let first = steps.iter().filter(|s| s.kind == Kind::Video).find_map(|s| s.src).and_then(|id| reels.get(id)).and_then(|r| r.v.as_ref());
    let Some((fw, fh, codec)) = first.map(|v| (v.1.width, v.1.height, tok(&v.1.codec))) else {
        return Err(("no-video", Some(String::from("the EDL cuts no video"))));
    };
    let out_ms = |t: i64| (tl.tb.to_ns(t).max(0) as u64) / 1_000_000;
    let vcount = steps.iter().filter(|s| s.kind == Kind::Video).count() as u64;
    let facts = Facts { codec, w: fw, h: fh, fps_x100: (tl.tb.den * 100 / tl.tb.num.max(1)) as u32, dur_ms: out_ms(tl.end()), frames: vcount, audio: "none", audio_ok: false };
    match JOB.lock().as_mut().filter(|j| j.jid == jid) {
        Some(j) => j.facts = Some(facts),
        None => return Err(("superseded", None)),
    }
    struct Run {
        src: timeline_core::SourceId,
        reads: Vec<demux_core::SampleRead>,
        pos: usize,
        pic: Pic,
    }
    let mut run: Option<Run> = None;
    for s in steps.iter().filter(|s| s.kind == Kind::Video) {
        let pts_ms = out_ms(s.out_t);
        if rep.plan.len() < SHOWN_CAP {
            rep.plan.push((s.out_t, s.src.map(|id| (id.0, s.src_t)), s.seek.is_some()));
        }
        let frame = match s.src {
            None => {
                rep.gaps += 1;
                Frame { pts_ms, w: fw as usize, h: fh as usize, px: alloc::vec![0; fw as usize * fh as usize], ord: None }
            }
            Some(id) => {
                let r = reels.get(id).ok_or_else(|| ("reel-missing", Some(alloc::format!("no reel for source {}", id.0))))?;
                let (ti, vt, o) = r.v.as_ref().ok_or_else(|| ("no-video", Some(alloc::format!("reel {} has no video stream", id.0))))?;
                if s.seek.is_some() || run.as_ref().map(|x| x.src) != Some(id) {
                    let reads: Vec<demux_core::SampleRead> = r.d.samples_from(*ti, o + tl.tb.to_ns(s.src_t)).collect();
                    let pic = Pic::new(vt).map_err(|e| ("codec-refused", Some(e)))?;
                    run = Some(Run { src: id, reads, pos: 0, pic });
                    rep.seeks += 1;
                }
                let Some(x) = run.as_mut() else { return Err(("superseded", None)) };
                loop {
                    let Some(rd) = x.reads.get(x.pos).copied() else {
                        return Err(("reel-short", Some(alloc::format!("reel {} ran out at source tick {}", id.0, s.src_t))));
                    };
                    x.pos += 1;
                    if rd.skip {
                        continue; // LEADSKIP: a leading picture the run's keyframe cannot decode
                    }
                    let pk = r.d.packet_at(&rd.sample);
                    let ord = x.pic.ordinal(&pk.data);
                    match x.pic.decode(&pk.data) {
                        Ok(Some(_)) if rd.preroll => rep.runup += 1,
                        Ok(Some((w, h, px))) => break Frame { pts_ms, w, h, px, ord: ord.map(|n| (s.out_t, n)) },
                        Ok(None) => {}
                        Err(e) => return Err(("decode-refused", Some(e))),
                    }
                }
            }
        };
        if pts_ms < seek_ms {
            continue; // a Player seek: the steps before it decoded unseen (job_task's rule)
        }
        room(jid).map_err(|why| (why, None))?;
        {
            let mut g = JOB.lock();
            let Some(j) = g.as_mut().filter(|j| j.jid == jid) else { return Err(("superseded", None)) };
            j.queue.push_back(frame);
            rep.frames += 1;
            j.decoded = rep.frames;
        }
        crate::arch::sched::yield_now();
    }
    Ok(())
}
