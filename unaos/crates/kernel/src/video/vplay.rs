// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — fulfiller
//!
//! VIDEOPLAYER (rmbp-ledger B434, MACPARITY row 30). CODEX §2: moving pictures are Stria's. R79: the kernel plays
//! a video as Stria's FULFILLER over the shared cores — `demux_core` (MP4 / Matroska / WebM), `vp8_core` and
//! `av1_core` (the pictures), `audio_core::vorbis` (a WebM's sound) — never a second decoder and never a store of
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
//! * SOUND: [`container_audio`] — when `audio_core` refuses a container (a WebM), `play-dec` (hda_play.rs) asks
//!   here: the Matroska Vorbis track as an `audio_core::Source` (the codec mapping's Xiph-laced headers → the
//!   core's `VorbisDecoder`, packets from `demux_core`). MP4 sound rides audio_core's own MP4 reader.
//! * QUICK LOOK: [`poster`] — the first frame and the facts, decoded on a `vposter` job (never on the 32 KiB render
//!   task); Quick Look's pass re-renders when it lands ([`poster_fresh`]).
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
    /// The sound can be played here: Vorbis or Opus in Matroska ([`container_audio`]), or an MP4 track audio_core reads.
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
    let audio_ok = match (d.format(), a.map(|t| &t.codec)) {
        (demux_core::Format::Mp4, Some(Codec::Aac | Codec::Mp3)) => true,
        (demux_core::Format::Matroska | demux_core::Format::WebM, Some(Codec::Vorbis | Codec::Opus)) => true, // SEEKTABLE2 (B469): Opus too
        _ => false,
    };
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
}

impl Pic {
    fn new(t: &demux_core::Track) -> Result<Pic, String> {
        match t.codec {
            Codec::Vp8 => Ok(Pic::Vp8(Box::new(vp8_core::Decoder::new()))),
            Codec::Av1 => av1_core::image::StreamDecoder::new(&t.config).map(|d| Pic::Av1(Box::new(d))).map_err(|e| alloc::format!("av1: {:?}", e)),
            ref c => Err(alloc::format!("no decoder for {} in this tree (vp8, av1)", tok(c))),
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
}

static JOB: spin::Mutex<Option<Job>> = spin::Mutex::new(None);
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
}

static PLAY: spin::Mutex<Play> = spin::Mutex::new(Play::new());

impl Play {
    const fn new() -> Play {
        Play { presented: 0, dropped: 0, sync_sum: 0, sync_n: 0, sync_max: 0, by_audio: 0, by_wall: 0, media_ms: 0, wall_ms: 0, open_ms: 0, audio_expected: false, audio_seen: false }
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
    stop();
    let jid = GEN.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    *JOB.lock() = Some(Job { jid, path: String::from(path), facts: None, queue: VecDeque::new(), decoded: 0, err: None, eos: false });
    let t = now();
    *PLAY.lock() = Play { open_ms: t, wall_ms: t, ..Play::new() };
    PAUSED.store(false, Ordering::Release);
    SPAWN_MS.store(t, Ordering::Release);
    DRAIN_MS.store(t, Ordering::Release);
    LIVE.store(true, Ordering::Release);
    spawn("play-vdec", job_task, jid, path);
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
    let mine = || GEN.load(Ordering::Acquire) == jid;
    let paint = stack_paint();
    let path = match JOB.lock().as_ref() {
        Some(j) if j.jid == jid => j.path.clone(),
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
    if let Some(j) = JOB.lock().as_mut().filter(|j| j.jid == jid) {
        j.facts = f;
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
    let mut first_pts: Option<i64> = None;
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
        loop {
            if !mine() {
                return job_exit(jid, paint, "aborted", decoded, errs);
            }
            let q = JOB.lock().as_ref().map(|j| j.queue.len()).unwrap_or(0);
            if q < QUEUE {
                break;
            }
            if now().saturating_sub(DRAIN_MS.load(Ordering::Acquire)) > ORPHAN_MS && !PAUSED.load(Ordering::Acquire) {
                return job_exit(jid, paint, "consumer-gone", decoded, errs);
            }
            crate::arch::sched::sleep_ms(2);
        }
        let out = pic.decode(data);
        let fp = *first_pts.get_or_insert(p.pts);
        let pts_ms = (vt.to_ns(p.pts - fp).max(0) as u64) / 1_000_000;
        match out {
            Ok(Some((w, h, px))) => {
                decoded += 1;
                let mut g = JOB.lock();
                let Some(j) = g.as_mut().filter(|j| j.jid == jid) else { drop(g); return job_exit(jid, paint, "superseded", decoded, errs) };
                j.decoded = decoded;
                j.queue.push_back(Frame { pts_ms, w, h, px });
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

// ── the container's sound (Matroska Vorbis) as an audio_core::Source ────────────────────────────────────────

/// The Matroska codec mapping's Xiph lacing (CodecPrivate of `A_VORBIS`): count-1, then the sizes of all but
/// the last in 255-runs, then the packets.
fn xiph_split(c: &[u8]) -> Option<Vec<&[u8]>> {
    let n = *c.first()? as usize + 1;
    let mut i = 1;
    let mut sizes = Vec::new();
    for _ in 0..n - 1 {
        let mut s = 0usize;
        loop {
            let b = *c.get(i)?;
            i += 1;
            s += b as usize;
            if b != 255 {
                break;
            }
        }
        sizes.push(s);
    }
    let mut out = Vec::new();
    for s in sizes {
        out.push(c.get(i..i + s)?);
        i += s;
    }
    out.push(c.get(i..)?);
    Some(out)
}

struct MkvVorbis {
    d: Demuxer,
    track: u32,
    dec: audio_core::vorbis::VorbisDecoder,
    rate: u32,
    ch: u16,
    out: Vec<Vec<f32>>,
}

impl audio_core::Source for MkvVorbis {
    fn info(&self) -> audio_core::Info {
        audio_core::Info { rate: self.rate, channels: self.ch, bits: 0, frames: None, format: audio_core::Format::Unknown, codec: audio_core::Codec::Vorbis, float: true }
    }
    fn block(&mut self, pcm: &mut audio_core::Pcm) -> audio_core::Result<bool> {
        loop {
            let Some(p) = self.d.next_packet() else { return Ok(false) };
            if p.track != self.track {
                continue;
            }
            let n = self.dec.decode(&p.data, &mut self.out).unwrap_or(0); // a corrupt packet decodes to nothing (OggVorbis's rule)
            if n == 0 {
                continue;
            }
            let ch = self.ch as usize;
            pcm.float = true;
            pcm.frames = n;
            pcm.flt.resize_with(ch, Vec::new);
            pcm.flt.truncate(ch);
            for (c, dst) in pcm.flt.iter_mut().enumerate() {
                dst.clear();
                dst.extend_from_slice(&self.out[c][..n.min(self.out[c].len())]);
                dst.resize(n, 0.0);
            }
            return Ok(true);
        }
    }
}

/// `play-dec`'s second door (hda_play.rs): a container `audio_core` does not read — the Matroska/WebM Vorbis
/// track — as a `Source`. `None` for anything else (the play is refused by audio_core's own reason).
pub fn container_audio(path: &str) -> Option<Box<dyn audio_core::Source>> {
    let bytes = read_all(path).ok()?;
    if demux_core::probe(&bytes) != Some(demux_core::Format::Matroska) {
        return None;
    }
    let d = Demuxer::open(bytes).ok()?;
    let t = d.audio_track()?.clone();
    if t.codec == Codec::Opus {
        return mkv_opus(d, t); // SEEKTABLE2 (B469): audio_core's Opus decoder over the track's packets
    }
    if t.codec != Codec::Vorbis {
        serial_println!("[vplay] sound codec={} -> none (Vorbis and Opus are the Matroska sounds this tree plays)", tok(&t.codec));
        return None;
    }
    let h = xiph_split(&t.config)?;
    if h.len() != 3 {
        return None;
    }
    let setup = match audio_core::vorbis::Setup::parse(h[0], h[2]) {
        Ok(s) => s,
        Err(e) => {
            serial_println!("[vplay] sound vorbis headers refused ({})", e);
            return None;
        }
    };
    let (rate, ch) = (setup.rate, setup.channels as u16);
    serial_println!("[vplay] sound container=matroska codec=vorbis rate={} ch={} packets={} -> play-dec", rate, ch, t.sample_count);
    Some(Box::new(MkvVorbis { d, track: t.id, dec: audio_core::vorbis::VorbisDecoder::new(setup), rate, ch, out: Vec::new() }))
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

static POSTER: spin::Mutex<Option<Poster>> = spin::Mutex::new(None);
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
                return Ok((f, Some(Frame { pts_ms: 0, w, h, px })));
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

// ── SEEKTABLE2 (rmbp-ledger B469): Opus inside Matroska/WebM — demux_core's track into audio_core's decoder ──────

/// The Matroska audio track's packets, in file order, for `audio_core::opus::OpusPackets` (no decoder here, R79).
struct MkvPackets {
    d: Demuxer,
    track: u32,
}

impl audio_core::opus::Packets for MkvPackets {
    fn next_packet(&mut self) -> Option<Vec<u8>> {
        loop {
            let p = self.d.next_packet()?;
            if p.track == self.track {
                return Some(p.data);
            }
        }
    }
}

fn mkv_opus(d: Demuxer, t: demux_core::Track) -> Option<Box<dyn audio_core::Source>> {
    let src = Box::new(MkvPackets { d, track: t.id });
    match audio_core::opus::OpusPackets::new(&t.config, t.codec_delay_ns, src) {
        Ok(o) => {
            let ch = audio_core::Source::info(&o).channels;
            serial_println!("[vplay] sound container=matroska codec=opus rate=48000 ch={} packets={} delay_ns={} -> play-dec", ch, t.sample_count, t.codec_delay_ns);
            Some(Box::new(o))
        }
        Err(e) => {
            serial_println!("[vplay] sound opus head refused ({})", e);
            None
        }
    }
}
