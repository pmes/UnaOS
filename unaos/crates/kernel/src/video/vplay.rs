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
//!   keeping each picture's planes (KERNELSCALECONVERT B971: converted where used, [`Px`]), and parks the frames in a queue of [`QUEUE`]. It never presents.
//! * THE CONSUMER (the Player's pass — one per vblank): [`present`] reads the clock, takes the NEWEST queued frame
//!   whose time has come, counts every older one it passed over as a drop, and blits it nearest-scaled and
//!   letterboxed into the window's video rect, converting only the shown rows ([`Frame::rows`]). At most one present per pass.
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
//!   (the section at the tail; NLE6MIX rmbp-ledger B899: a transition mixes a second run through timeline_core's one
//!   `vmix::mix_step`; no sound yet).
//!
//! Wire: `[vplay] job spawn|run|exit …`, `[vplay] colour matrix=<n> range=<limited|full> src=<container|bitstream|default>` (KERNELCOLOUR rmbp-ledger B952, the tail), `[player] video codec=<vp8|av1> size=<w>x<h> fps=<n> …` (player.rs),
//! `:: VIDEOPLAYER: open=ok frames=<n> dropped=<n> fps=<n> av_sync_ms=<n> clock=<audio|wall> -> PASS ::`
//! (`tests videoplayer`, R80: typed). Not here: video seek (SEEKTABLE B433), hardware decode.
//! Design: docs/dev/evidence/rmbp-1005/videoplayer.md.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use demux_core::{colour::Colour, Codec, Demuxer};

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

/// One decoded picture at its coded size (its planes or 0RGB, [`Px`]); `pts_ms` from the track's first packet.
pub struct Frame {
    pub pts_ms: u64,
    pub w: usize,
    pub h: usize,
    pub px: Px,
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
pub(crate) fn read_all(path: &str) -> Result<Vec<u8>, String> {
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
    Vp8(Box<vp8_core::Decoder>, Option<Colour>), // KERNELCOLOUR (B952): the track's colour (`open_colour`), converted per picture
    Av1(Box<av1_core::image::StreamDecoder>, Option<Colour>),
    /// NLE6 (SR207): demux_core's `utp1` test stream — its packet carries its ordinal (`build::TestPatternPacket`).
    Utp1,
    /// SWAPBACKVPLAY (rmbp-ledger B956): the labelled stand-in (see [`StandIn`]).
    StandIn(Box<StandIn>),
}

/// SWAPBACKVPLAY (rmbp-ledger B956; handlers.md §5 SWAPBACK-VPLAY): the kernel Player's ONE swap state, the rule of Stria's
/// `handlers/stria/src/decode_swap.rs` `DecodeSwap::decode` over the no_std cores. The stand-in paints demux_core's ONE painter
/// (`build::test_pattern_rgba`, the pixels gneiss_pal's `TestPattern::stand_in` paints: bars + `round(pts / frame)`), at the
/// track's size (at least 64x48). `parked` is the real decoder a core's `Unsupported` refused MID-STREAM, reset: each later
/// keyframe is offered to it once ([`Pic::decode_swap`]); an open-time stand-in (no decoder fits: [`Pic::open`]) parks nothing and
/// is never probed. No core frame buffer to drain (the kernel's decoders show each picture as it decodes). Every run open builds
/// a fresh [`Pic::open`] from the container's track, so no parked decoder crosses a seek (SR353's reseed, the kernel's shape).
/// Owned by the job task (`job_task`'s local, `EdlRun.pic`): no lock. Wire: `[vplay] swap: stand-in <why> at <ns>`,
/// `[vplay] swap: back at <ns>` (source ns).
struct StandIn {
    w: u32,
    h: u32,
    tb: demux_core::Timebase,
    frame_ns: i64,
    parked: Option<Pic>,
}

/// SWAPBACKVPLAY (B956): which way a decode swapped (stria::decode_swap's `Swap`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Swap {
    ToStandIn,
    Back,
}

impl Pic {
    fn new(t: &demux_core::Track) -> Result<Pic, String> {
        match t.codec {
            Codec::Vp8 => Ok(Pic::Vp8(Box::new(vp8_core::Decoder::new()), t.colour)),
            Codec::Av1 => av1_core::image::StreamDecoder::new(&t.config).map(|d| Pic::Av1(Box::new(d), t.colour)).map_err(|e| alloc::format!("av1: {:?}", e)),
            Codec::TestPattern => Ok(Pic::Utp1),
            ref c => Err(alloc::format!("no decoder for {} in this tree (vp8, av1, utp1)", tok(c))),
        }
    }

    /// SWAPBACKVPLAY (B956): the decoder for `t`, else the open-time stand-in (Stria's Player: no decoder fits, the labelled
    /// stand-in plays) with its `[vplay] swap: stand-in <why> at <at_ns>` line.
    fn open(t: &demux_core::Track, at_ns: i64) -> Pic {
        match Pic::new(t) {
            Ok(p) => p,
            Err(why) => {
                serial_println!("[vplay] swap: stand-in {} at {}", why, at_ns);
                Pic::stand_in(t, None)
            }
        }
    }

    /// SWAPBACKVPLAY (B956): the stand-in for `t` (gneiss_pal `TestPattern::new`'s size and frame through demux_core's rule).
    fn stand_in(t: &demux_core::Track, parked: Option<Pic>) -> Pic {
        Pic::StandIn(Box::new(StandIn { w: t.width.max(64), h: t.height.max(48), tb: t.timebase, frame_ns: demux_core::build::stand_in_frame_ns(t), parked }))
    }

    /// SWAPBACKVPLAY (B956): true while the stand-in draws.
    fn on_stand_in(&self) -> bool {
        matches!(self, Pic::StandIn(_))
    }

    /// SWAPBACKVPLAY (B956): forget the pictures, keep the parameter sets (the parked decoder's reset).
    fn reset(&mut self) {
        match self {
            Pic::Vp8(d, _) => d.reset(),
            Pic::Av1(d, _) => d.reset(),
            Pic::Utp1 | Pic::StandIn(_) => {}
        }
    }

    /// SWAPBACKVPLAY (B956): decode `data` (a packet of `t` at `pts`, `key` the demuxer's keyframe flag) swapping to the stand-in
    /// or back by [`StandIn`]'s rule — `DecodeSwap::decode`'s: a parked decoder is offered a keyframe once (a decode = it plays
    /// again, `Swap::Back`; a refusal = reset and parked again); a REAL decoder's core `Unsupported` parks it (reset), the
    /// stand-in paints the refused packet (`Swap::ToStandIn`); any other error is the caller's.
    fn decode_swap(&mut self, t: &demux_core::Track, data: &[u8], pts: i64, key: bool) -> Result<(Option<(usize, usize, Px)>, Option<Swap>), String> {
        let parked = match self {
            Pic::StandIn(s) if key => s.parked.take(),
            _ => None,
        };
        if let Some(mut real) = parked {
            match real.decode_raw(data, pts) {
                Ok(f) => {
                    *self = real;
                    serial_println!("[vplay] swap: back at {}", t.to_ns(pts));
                    return Ok((f, Some(Swap::Back)));
                }
                Err(_) => {
                    real.reset();
                    if let Pic::StandIn(s) = self {
                        s.parked = Some(real);
                    }
                }
            }
        }
        match self.decode_raw(data, pts) {
            Ok(f) => Ok((f, None)),
            Err((true, why)) if !self.on_stand_in() => {
                let mut real = core::mem::replace(self, Pic::stand_in(t, None));
                real.reset();
                if let Pic::StandIn(s) = self {
                    s.parked = Some(real);
                }
                serial_println!("[vplay] swap: stand-in {} at {}", why, t.to_ns(pts));
                Ok((self.decode_raw(data, pts).ok().flatten(), Some(Swap::ToStandIn)))
            }
            Err((_, why)) => Err(why),
        }
    }

    /// One packet → the picture it shows (`None`: an alt-ref / dropped frame that shows nothing); the String error of
    /// [`Pic::decode_raw`] (the poster's door).
    fn decode(&mut self, data: &[u8]) -> Result<Option<(usize, usize, Px)>, String> {
        self.decode_raw(data, 0).map_err(|e| e.1)
    }

    /// SWAPBACKVPLAY (B956): the decode arms; `Err((unsupported, why))`, `unsupported` = the core's own `Unsupported` (the
    /// swap's one trigger: av1_core's; vp8_core and utp1 have none). `pts` (track ticks) clocks the stand-in's counter.
    fn decode_raw(&mut self, data: &[u8], pts: i64) -> Result<Option<(usize, usize, Px)>, (bool, String)> {
        match self {
            Pic::Vp8(d, c) => {
                if data.is_empty() {
                    return Ok(None);
                }
                let pic = match d.decode(data) {
                    Ok(Some(p)) => p,
                    Ok(None) => return Ok(None),
                    Err(e) => return Err((false, alloc::format!("vp8: {:?}", e))),
                };
                let y = vp8_core::Yuv420::from_picture(&pic);
                Ok(Some((y.width as usize, y.height as usize, Px::planes(Src::Vp8(y, *c))))) // KERNELSCALECONVERT (B971): the planes, converted where used
            }
            Pic::Av1(d, c) => {
                let planes = d.decode_temporal_unit(data).map_err(|e| (matches!(e, av1_core::Error::Unsupported(_)), alloc::format!("av1: {:?}", e)))?;
                Ok(Some((planes.width as usize, planes.height as usize, Px::planes(Src::Av1(Box::new(planes), inband_colour(d, *c, pts)))))) // KERNELSCALECONVERT (B971); AV1INBANDSWITCH (SR373): the frame's colour, decided at the decode
            }
            Pic::Utp1 => {
                // the stream's own decoder: the packet names its frame; the picture is a flat field whose level steps
                // with the ordinal (the host's pattern renderer is gneiss_pal's; NLE6-UTP1 owes its move to a core)
                let t = demux_core::build::TestPatternPacket::decode(data).ok_or_else(|| (false, String::from("utp1: not a UTP1 packet")))?;
                let (w, h) = ((t.width as usize).max(1), (t.height as usize).max(1));
                let l = 28 + t.frame % 200;
                Ok(Some((w, h, alloc::vec![(l << 16) | (l << 8) | l; w * h].into())))
            }
            Pic::StandIn(s) => {
                let n = demux_core::build::stand_in_counter(s.tb.to_ns(pts), s.frame_ns);
                Ok(Some((s.w as usize, s.h as usize, pack(&demux_core::build::test_pattern_rgba(s.w, s.h, n)).into())))
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
/// `bar`. Returns the picture's drawn size. KERNELSCALEONE (rmbp-ledger B965): the letterbox and the samples are yuv_core's
/// ONE resampler (`yuv_core::scale::fit` / `nearest_rows`, user-stria's too). KERNELSCALECONVERT (rmbp-ledger B971): only the
/// shown rows convert ([`Frame::rows`]).
pub fn blit_fit(dst: &mut [u32], stride: usize, x: usize, y: usize, bw: usize, bh: usize, f: &Frame, bar: Option<u32>) -> (usize, usize) {
    if f.w == 0 || f.h == 0 || bw == 0 || bh == 0 || !f.fits() {
        return (0, 0);
    }
    let fr = yuv_core::scale::fit(f.w, f.h, bw, bh);
    let (dw, dh, ox, oy) = (fr.w, fr.h, x + fr.x, y + fr.y);
    for r in 0..bh {
        let row = (y + r) * stride;
        if row + x + bw > dst.len() {
            break;
        }
        let Some(c) = bar else { continue };
        if r + y >= oy && r + y < oy + dh {
            dst[row + x..row + ox].fill(c);
            dst[row + ox + dw..row + x + bw].fill(c);
        } else {
            dst[row + x..row + x + bw].fill(c);
        }
    }
    f.rows(dw, dh, |i, s| {
        let row = (oy + i) * stride;
        if row + x + bw <= dst.len() {
            dst[row + ox..row + ox + dw].copy_from_slice(&s[..dw]);
        }
    });
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
    let vh = view_band(dst, w, vh, f.pts_ms); blit_fit(dst, w, 0, 0, w, vh, &f, Some(0)); // NLE6KEYS (rmbp-ledger B961): the EDL's track view in the picture rect's lower band (file tail)
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
    open_colour(&mut d, true); let Some(vt) = d.video_track().cloned() else {
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
    let mut pic = Pic::open(&vt, (seek_ms as i64).saturating_mul(1_000_000)); // SWAPBACKVPLAY (rmbp-ledger B956): no decoder fits = the labelled stand-in (Stria's Player), never codec-refused
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
        let out = pic.decode_swap(&vt, data, p.pts, p.keyframe).map(|r| r.0); // SWAPBACKVPLAY (rmbp-ledger B956): Stria's swap rule
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
        snd::tick(jid); crate::arch::sched::sleep_ms(2); // NLE6MIXAUDIO (rmbp-ledger B953): the EDL's sound is written while the job waits (file tail; no-op outside the EDL mode)
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
        let mut d = Demuxer::open(read_all(&path)?).map_err(|e| alloc::format!("{}", e))?;
        open_colour(&mut d, false); let vt = d.video_track().cloned().ok_or_else(|| String::from("no video track"))?;
        let f = facts_of(&d);
        let mut pic = Pic::new(&vt)?;
        for s in d.track_samples(d.track_index(vt.id).unwrap_or(0)).take(8) {
            let pk = d.packet_at(s);
            if let Some((w, h, px)) = pic.decode(&pk.data)? {
                let _ = px.rgb(); // KERNELSCALECONVERT (B971): the poster converts on its own job; the render task blits the cached 0RGB
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
// [`present`] shows them on the Player's clock. NLE6MIX (rmbp-ledger B899): a step inside a transition (`Step.vmix`) mixes
// the outgoing source's frame (a second run, `mix`: the live run it continues via timeline_core's `vmix::take_outgoing`,
// else opened at its keyframe; dropped at the first step without `vmix`) with its own picture (or the to-black's black)
// through timeline_core's ONE `vmix::mix_step` (the fn Stria's render calls); a slot the heap or the registry cannot give
// is refused by name and the incoming shows. NLE6MIXAUDIO (rmbp-ledger B953): the audio runs play through the mixer,
// a crossfade on a second source (the section at the file tail, `snd`). Wire: `[nle] reel …`, `[nle] open edl=<path> reels=<n> clips=<n> steps=<n>`, `[nle] job exit
// frames=<n> seeks=<n> runup=<n> gaps=<n> mixed=<n> mix_seeks=<n> refused=<n> mix_us_max=<n>`, `[vplay] transition:
// second decoder slot refused (<why>)`; SWAPBACKVPLAY (rmbp-ledger B956): a reel no decoder fits plays the labelled stand-in
// (`Pic::open`, `[vplay] swap: …`, `swaps=<n> standin=<n>` on the exit line); the witness is player.rs `tests nle` (`:: NLE: … ::`).

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
    /// KERNELCOLOUR (B952): `(output tick, FNV-1a 64 of the RGBA)` of the first [`CODED_CAP`] coded pictures (no utp1 ordinal).
    pub coded: Vec<(i64, u64)>,
    /// Per video step: `(output tick, (source, source tick), carries a seek)`; at most [`SHOWN_CAP`].
    pub plan: Vec<(i64, Option<(u32, i64)>, bool)>,
    /// NLE6MIX (B899): frames mixed inside a transition, outgoing runs opened at a keyframe, refused second slots, the
    /// slowest mix (µs), and per mixed frame `(output tick, FNV-1a 64 of the mixed RGBA)` (at most [`SHOWN_CAP`]).
    pub mixed: u64,
    pub mix_seeks: u64,
    pub mix_refused: u64,
    pub mix_us_max: u64,
    /// SWAPBACKVPLAY (rmbp-ledger B956): runs that went to the stand-in (an open no decoder fits, or a mid-stream refusal) and
    /// the stand-in pictures shown.
    pub swaps: u64,
    pub stand_in_frames: u64,
    pub mixes: Vec<(i64, u64)>, pub snd_runs: u64, pub xfades: u64, pub snd_refused: u64, pub snd_underruns: u64, pub snd_frames: u64, // NLE6MIXAUDIO (rmbp-ledger B953): the sound's runs, second sources opened and refused, underruns, frames played
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
    let r = edl_run(jid, &path, seek_ms, &mut rep); snd::close(&mut rep); let r = super::export::finish(jid, r); // NLE6EXPORT (rmbp-ledger B959): an Encode job's MP4 muxed and written once its sound closed (video/export.rs). NLE6MIXAUDIO (rmbp-ledger B953): every source closed, on every path (file tail)
    serial_println!(
        "[nle] job exit frames={} seeks={} runup={} gaps={} mixed={} mix_seeks={} refused={} mix_us_max={} swaps={} standin={}",
        rep.frames, rep.seeks, rep.runup, rep.gaps, rep.mixed, rep.mix_seeks, rep.mix_refused, rep.mix_us_max, rep.swaps, rep.stand_in_frames
    );
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
    let st = timeline_core::editor::EditorState::open(text, 25).map_err(|e| ("edl-refused", Some(alloc::format!("edl: {}", e))))?; let st = edit_seed(jid, path, st); // NLE6KEYS (rmbp-ledger B961): the Player's one editor state survives the job's restarts (file tail)
    let tl = &st.timeline;
    let dir = path.rfind('/').map(|i| &path[..i]).unwrap_or("");
    let mt = crate::shell::vfs_mount_table();
    let mut reels = Reels { tb: tl.tb, reels: Vec::new() };
    for s in &tl.sources {
        let Some(rp) = [".MP4", ".WEBM", ".MKV"].iter().map(|x| alloc::format!("{}/{}{}", dir, s.reel, x)).find(|p| mt.stat(p).is_ok()) else {
            return Err(("reel-missing", Some(alloc::format!("no file for reel {} beside the EDL ({}/{}.MP4|.WEBM|.MKV)", s.reel, dir, s.reel))));
        };
        let mut d = Demuxer::open(read_all(&rp).map_err(|e| ("read-refused", Some(e)))?).map_err(|e| ("demux-refused", Some(alloc::format!("{}: {}", rp, e))))?;
        open_colour(&mut d, true); let v = d.video_track().and_then(|t| d.track_index(t.id).map(|i| (i, t.clone()))).map(|(i, t)| (i, t, origin(&d, i)));
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
        super::export::arm_audio(jid); snd::open(&reels, tl, seek_ms); // NLE6EXPORT (rmbp-ledger B959): an Encode job's sound into the export's offline capture (video/export.rs). NLE6MIXAUDIO (rmbp-ledger B953): the EDL's sound, one mixer source per audio track, the crossfade's second (file tail)
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
    let mut run: Option<EdlRun> = None;
    let mut mix: Option<EdlRun> = None; // NLE6MIX (B899): a transition's outgoing run, the second decoder slot
    for s in steps.iter().filter(|s| s.kind == Kind::Video) {
        let pts_ms = out_ms(s.out_t);
        if rep.plan.len() < SHOWN_CAP {
            rep.plan.push((s.out_t, s.src.map(|id| (id.0, s.src_t)), s.seek.is_some()));
        }
        let refused = edl_take(s, &mut run, &mut mix, fw as usize, fh as usize).err();
        let mut frame = match s.src {
            None => {
                if s.vmix.is_none() {
                    rep.gaps += 1; // a to-black step's black is mixed below (NLE6MIX)
                }
                Frame { pts_ms, w: fw as usize, h: fh as usize, px: alloc::vec![0; fw as usize * fh as usize].into(), ord: None }
            }
            Some(id) => {
                let (w, h, px, ord) = edl_frame(&reels, tl.tb, &mut run, id, s.src_t, s.seek.is_some(), rep, false)?;
                coded(rep, s.out_t, ord, &px); Frame { pts_ms, w, h, px, ord: ord.map(|n| (s.out_t, n)) }
            }
        };
        match (s.vmix, refused) {
            (Some(_), Some(why)) => edl_refuse(rep, &why),
            (Some(m), None) => edl_mix(&reels, tl.tb, s.out_t, m, &mut frame, &mut mix, rep)?,
            (None, _) => {}
        }
        if pts_ms < seek_ms {
            continue; // a Player seek: the steps before it decoded unseen (job_task's rule)
        }
        if let super::export::Sink::Encode = super::export::sink(jid) { super::export::frame(jid, tl.tb, vcount, frame.w, frame.h, &unpack(frame.px.rgb())).map_err(|e| ("encode-refused", Some(e)))?; rep.frames += 1; snd::tick(jid); crate::arch::sched::yield_now(); continue; } face_preview(seek_ms, rep, s.out_t, &frame); room(jid).map_err(|why| (why, None))?; // NLE6EXPORT (rmbp-ledger B959): the Encode sink — the step's frame to the exporter, not the queue (video/export.rs). NLE6MIXFACE (rmbp-ledger B957): the first frame a Player seek shows (file tail)
        {
            let mut g = JOB.lock();
            let Some(j) = g.as_mut().filter(|j| j.jid == jid) else { return Err(("superseded", None)) };
            j.queue.push_back(frame);
            rep.frames += 1;
            j.decoded = rep.frames;
        }
        snd::tick(jid); crate::arch::sched::yield_now(); // NLE6MIXAUDIO (rmbp-ledger B953): the sound, once per step (file tail)
    }
    snd::drain(jid); Ok(()) // NLE6MIXAUDIO (rmbp-ledger B953): the sound plays out after the last picture, bounded (file tail)
}

// ── NLE6MIX (rmbp-ledger B899): the transition mix on the EDL job — the second run, the ONE mix fn, the refusal ─────────

/// One decode run of the EDL job: a reel's video samples from a keyframe on, and the decoder reading them.
struct EdlRun {
    src: timeline_core::SourceId,
    reads: Vec<demux_core::SampleRead>,
    pos: usize,
    pic: Pic,
}

/// The ONE open-or-continue fn (Stria's `vid_frame` shape), for a step's own source and a transition's outgoing alike:
/// a `seek`, or a run on another source (or none), opens `slot` at the keyframe (`Demuxer::samples_from`; the run-up is
/// decoded unseen); otherwise the run decodes on. Returns the picture `(w, h, 0RGB, utp1 ordinal)`.
#[allow(clippy::too_many_arguments)]
fn edl_frame(
    reels: &Reels,
    tb: timeline_core::Timebase,
    slot: &mut Option<EdlRun>,
    id: timeline_core::SourceId,
    src_t: i64,
    seek: bool,
    rep: &mut EdlReport,
    outgoing: bool,
) -> Result<(usize, usize, Px, Option<u32>), (&'static str, Option<String>)> {
    let r = reels.get(id).ok_or_else(|| ("reel-missing", Some(alloc::format!("no reel for source {}", id.0))))?;
    let (ti, vt, o) = r.v.as_ref().ok_or_else(|| ("no-video", Some(alloc::format!("reel {} has no video stream", id.0))))?;
    if seek || slot.as_ref().map(|x| x.src) != Some(id) {
        let reads: Vec<demux_core::SampleRead> = r.d.samples_from(*ti, o + tb.to_ns(src_t)).collect();
        let pic = Pic::open(vt, tb.to_ns(src_t)); rep.swaps += pic.on_stand_in() as u64; // SWAPBACKVPLAY (rmbp-ledger B956): a fresh decoder per run (the kernel's reseed), else the open-time stand-in
        *slot = Some(EdlRun { src: id, reads, pos: 0, pic });
        if outgoing {
            rep.mix_seeks += 1;
        } else {
            rep.seeks += 1;
        }
    }
    let Some(x) = slot.as_mut() else { return Err(("superseded", None)) };
    loop {
        let Some(rd) = x.reads.get(x.pos).copied() else {
            return Err(("reel-short", Some(alloc::format!("reel {} ran out at source tick {}", id.0, src_t))));
        };
        x.pos += 1;
        if rd.skip {
            continue; // LEADSKIP: a leading picture the run's keyframe cannot decode
        }
        let pk = r.d.packet_at(&rd.sample);
        let ord = x.pic.ordinal(&pk.data);
        let out = x.pic.decode_swap(vt, &pk.data, rd.sample.pts, rd.sample.keyframe); rep.swaps += matches!(out, Ok((_, Some(Swap::ToStandIn)))) as u64; // SWAPBACKVPLAY (rmbp-ledger B956): Stria's swap rule
        match out.map(|r| r.0) {
            Ok(Some(_)) if rd.preroll => rep.runup += 1,
            Ok(Some((w, h, px))) => {
                rep.stand_in_frames += x.pic.on_stand_in() as u64;
                return Ok((w, h, px, ord));
            }
            Ok(None) => {}
            Err(e) => return Err(("decode-refused", Some(e))),
        }
    }
}

/// timeline_core's ONE outgoing-run rule (`vmix::take_outgoing`, Stria's too) over the job's two runs; when the live run
/// becomes the outgoing (two decoders now coexist) the heap is asked first, and a refusal drops it (`Err(why)`).
fn edl_take(s: &timeline_core::schedule::Step, run: &mut Option<EdlRun>, mix: &mut Option<EdlRun>, w: usize, h: usize) -> Result<(), String> {
    if timeline_core::vmix::take_outgoing(s, run, mix, |x| x.src) {
        if let Err(why) = slot_room(w, h) {
            *mix = None;
            return Err(why);
        }
    }
    Ok(())
}

/// Mix `frame` (the step's own picture, or the to-black's black) with the outgoing source's frame at `m.src_t` (opaque
/// black for a transition from black) through timeline_core's ONE `vmix::mix_step`; the mixed RGBA's FNV-1a 64 is kept
/// for the witness. A slot the heap or the decoder registry cannot give, or a size the mix cannot pair, is refused by
/// name (the incoming shows); any other error ends the job as the step's own would.
#[allow(clippy::too_many_arguments)]
fn edl_mix(
    reels: &Reels,
    tb: timeline_core::Timebase,
    out_t: i64,
    m: timeline_core::schedule::VideoMix,
    frame: &mut Frame,
    mix: &mut Option<EdlRun>,
    rep: &mut EdlReport,
) -> Result<(), (&'static str, Option<String>)> {
    let t0 = crate::hidstall::now_us();
    let (w, h) = (frame.w, frame.h);
    let mut a = match m.src {
        None => [0u8, 0, 0, 255].repeat(w * h), // NLE3c: from black, the outgoing is opaque black
        Some(o) => {
            if m.seek.is_some() || mix.as_ref().map(|x| x.src) != Some(o) {
                if let Err(why) = slot_room(w, h) {
                    edl_refuse(rep, &why);
                    return Ok(());
                }
            }
            match edl_frame(reels, tb, mix, o, m.src_t, m.seek.is_some(), rep, true) {
                Ok((ow, oh, px, _)) if (ow, oh) == (w, h) => unpack(px.rgb()),
                Ok((ow, oh, _, _)) => {
                    *mix = None;
                    edl_refuse(rep, &alloc::format!("size {}x{} != {}x{}", ow, oh, w, h));
                    return Ok(());
                }
                Err(("codec-refused", e)) => {
                    *mix = None;
                    edl_refuse(rep, &alloc::format!("codec: {}", e.unwrap_or_default()));
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
    };
    let b = unpack(frame.px.rgb());
    timeline_core::vmix::mix_step(m.kind, m.t, &mut a, &b, w as u32, h as u32);
    let sum = frame_fnv_rgba(&a);
    if rep.mixes.len() < SHOWN_CAP {
        rep.mixes.push((out_t, sum));
    }
    frame.px = pack(&a).into();
    rep.mixed += 1;
    rep.mix_us_max = rep.mix_us_max.max(crate::hidstall::now_us().saturating_sub(t0));
    Ok(())
}

/// Room for a second decoder slot of a `w x h` picture: its frames and the mix's two RGBA buffers (`need` = 16 B a
/// pixel), granted while the free heap holds twice that.
fn slot_room(w: usize, h: usize) -> Result<(), String> {
    let (lo, hi) = crate::allocator::heap_bounds();
    let free = hi.saturating_sub(lo).saturating_sub(crate::allocator::heap_used());
    let need = w * h * 16;
    if free < 2 * need {
        return Err(alloc::format!("heap free_kb={} need_kb={}", free / 1024, need / 1024));
    }
    Ok(())
}

/// A refused second slot: counted, and named on the wire for the first four.
fn edl_refuse(rep: &mut EdlReport, why: &str) {
    rep.mix_refused += 1;
    if rep.mix_refused <= 4 {
        serial_println!("[vplay] transition: second decoder slot refused ({})", why);
    }
}

/// [`pack`]'s inverse: 0RGB -> straight RGBA, opaque (the mix fn's pixel form).
fn unpack(px: &[u32]) -> Vec<u8> {
    let mut v = Vec::with_capacity(px.len() * 4);
    for p in px {
        v.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
    }
    v
}

// ── KERNELCOLOUR (rmbp-ledger B952): the open's colour and the picture -> RGBA conversion, Stria's own ────────────────────
// handlers.md §5 NLEPROBE-HDR's KERNEL-COLOUR. An open (`job_task`, `poster_task`, each EDL reel) fills the video track's
// `Track::colour` as Stria's opens do: the container's statement (MP4 `colr`, Matroska `Colour`, read by demux_core at open)
// wins, and the bitstream's (AV1: `av1_core::image::stream_codes`, the av1C configOBUs else the first sample's in-band header, AV1INBANDCOLOUR SR366, mapped by `demux_core::colour::Colour::from_av1`, the
// one mapping gneiss_pal's `colour_of` calls too) fills its gaps through demux_core's one merge, `Demuxer::fill_colour`.
// Each picture then converts through yuv_core's ONE `pick` (the H.273 matrix, else the size default: BT.601 below 1280x720,
// BT.709 at it; full range only when stated) and ONE `to_rgba`: VP8 with nearest chroma (Stria's I420 path,
// `Frame::to_rgba_colour`, so the two agree byte for byte), AV1 with bilinear chroma (Stria's `Av1Decoder` upsampling).
// Wire: `[vplay] colour matrix=<n> range=<limited|full> src=<container|bitstream|default>`, once per vp8 / av1 open on the
// job (the poster is silent); `<n>` is the track's stated code, 2 when none (then the size default converts).

/// Coded pictures whose FNV the EDL report keeps (`tests nle` reads the colour witness from them).
const CODED_CAP: usize = 16;

/// An open's colour: the bitstream's fills what the container did not say ([`Demuxer::fill_colour`]); with `log`, the line.
fn open_colour(d: &mut Demuxer, log: bool) {
    let said = d.video_track().and_then(|t| t.colour).is_some_and(|c| c.matrix != 2);
    d.fill_colour(bitstream_colour);
    let Some(t) = d.video_track().filter(|t| log && matches!(t.codec, Codec::Vp8 | Codec::Av1)) else { return };
    let stated = t.colour.map(|c| c.matrix).filter(|m| *m != 2);
    let (_, full) = yuv_core::pick(stated, t.colour.and_then(|c| c.full_range), t.width, t.height);
    let src = if said { "container" } else if stated.is_some() { "bitstream" } else { "default" };
    serial_println!("[vplay] colour matrix={} range={} src={}", stated.unwrap_or(2), if full { "full" } else { "limited" }, src);
}

/// The bitstream's colour of a video track (what `fill_colour` takes, with the track's first sample): AV1 through the one reading and mapping; VP8 states none.
fn bitstream_colour(t: &demux_core::Track, first: &[u8]) -> Option<Colour> {
    match t.codec {
        Codec::Av1 => av1_core::image::stream_codes(&t.config, first).map(|(c, l, m)| Colour::from_av1(c, l, m)),
        _ => None,
    }
}

/// A VP8 picture -> RGBA by the track's colour: 8-bit 4:2:0, nearest chroma (Stria's `Pixels::I420` path).
fn colour_rgba_vp8(y: &vp8_core::Yuv420, c: Option<Colour>) -> Vec<u8> {
    let (p, m) = vp8_picture(y, c); // KERNELSCALECONVERT (B971): the one pick + Picture, shared with the shown rows (file tail)
    let mut out = alloc::vec![0u8; p.w * p.h * 4];
    yuv_core::to_rgba(&p, m, yuv_core::Chroma::Nearest, false, &mut out);
    out
}

/// An AV1 picture -> RGBA by the track's colour: av1_core's own `picture` (any depth, any subsampling) with the pick's
/// range, bilinear chroma (Stria's `Av1Decoder`).
fn colour_rgba_av1(planes: &av1_core::image::Planes, c: Option<Colour>) -> Vec<u8> {
    let (p, m) = av1_picture(planes, c); // KERNELSCALECONVERT (B971): the one pick + Picture, shared with the shown rows (file tail)
    let mut out = alloc::vec![0u8; p.w * p.h * 4];
    yuv_core::to_rgba(&p, m, yuv_core::Chroma::Bilinear, false, &mut out);
    out
}

/// The EDL report's coded-picture witness: a picture with no utp1 ordinal, while fewer than [`CODED_CAP`] are kept.
fn coded(rep: &mut EdlReport, out_t: i64, ord: Option<u32>, px: &Px) {
    if ord.is_none() && rep.coded.len() < CODED_CAP {
        rep.coded.push((out_t, frame_fnv_rgba(&unpack(px.rgb())))); // KERNELSCALECONVERT (B971): converted (and cached) only when hashed
    }
}

/// FNV-1a 64 of straight RGBA (the mix witness and the coded witness hash the same bytes).
fn frame_fnv_rgba(rgba: &[u8]) -> u64 {
    rgba.iter().fold(crate::video::strip::FNV_BASIS, |acc, x| crate::video::strip::fnv1a(acc, *x))
}

// ── NLE6MIXAUDIO (rmbp-ledger B953): the EDL job's sound — one mixer source per audio track, the crossfade's second ──────
// `snd::open` takes timeline_core's audio runs (`schedule::audio_runs`, the runs Stria's `mix_run` renders) and opens ONE
// mixer source per audio track (`nle`, the format of the track's first playable reel, PCM16 into the mixer's own
// converter). `snd::tick` (from the job's waits: `room`, and once per video step) writes every source as far as its ring
// takes, run by run: a layer's reel through audio_core's ONE door (`container::open_demuxed`, its PCM door included; a
// `seek` or another source reopens it, a jump forward within it skips), a track with no layer as silence (the clock), each
// run's gain a mixer RAMP between its edges (`Layer.env` x `gain_cdb` through `audio_core::ramp::layer_q8`, set before the
// run's first frame is written; the mixer interpolates per frame through `audio_core::ramp::at`). A crossfade run's UNDER
// layer (timeline_core's `AudioRun::under`, Stria's slot rule) plays on a SECOND source, opened when the top source reaches
// the span and `mixer::align`ed to it (its first frame mixes with the top's), ended at the first run without one and
// closed once drained; a source the mixer refuses is named, `[nle] audio crossfade: second source refused (<why>)`, and that
// span is a cut (the incoming at its clip gain). `snd::drain` plays the rest out after the last picture (it gives up after
// `STALL_MS` without progress: no sink); `snd::close` closes every source and prints `[nle] audio exit runs=<n> xfades=<n>
// refused=<n> underruns=<n> frames=<n>`. A Player seek starts every run's sound at the seek (its ramp entered part-way).
// The EDL's clock stays the wall. Lock: `SND` (the job task's) then the mixer's; nothing under the mixer takes `SND`. The
// image without an audio module (`uvc` / `hda-tone` off) carries the silent twin below.
#[cfg(any(feature = "uvc", feature = "hda-tone"))]
mod snd {
    use super::{EdlReport, Reels, GEN};
    use crate::video::export::bus as mixer; // NLE6EXPORT (rmbp-ledger B959): the live mixer, or the export's offline Mix when an Encode job armed it (video/export.rs `bus`)
    use alloc::vec::Vec;
    use audio_core::AudioDecoder;
    use core::sync::atomic::Ordering;
    use timeline_core::schedule::{AudioRun, ENV_ONE};
    use timeline_core::{SourceId, Timebase};

    /// Frames a slot makes per step before offering them to the mixer.
    const CHUNK: u64 = 1024;
    /// The drain gives up after this long with no byte taken and no frame played (no sink).
    const STALL_MS: u64 = 3_000;

    /// A layer's decoder: its reel's sound from source frame `pos` on.
    struct Aud {
        src: SourceId,
        dec: audio_core::Decoder,
        pos: u64,
    }

    /// One mixer source of an audio track: its top layer, or a crossfade's under layer (`under`).
    struct Slot {
        track: usize,
        under: bool,
        id: u32,
        rate: u32,
        ch: u8,
        aud: Option<Aud>,
        /// The run being written, the frames of it written (slot rate), its setup done, a layer plays in it.
        ri: usize,
        off: u64,
        set: bool,
        play: bool,
        /// Bytes made, not yet taken by the mixer.
        pend: Vec<u8>,
        /// Top only: this span's second source was opened (`span`) or refused (`cut`: the span is a cut).
        span: bool,
        cut: bool,
        done: bool,
    }

    /// A reel's sound: its demuxer share when audio_core plays its audio track, and the track's format.
    struct ReelSnd {
        id: SourceId,
        d: Option<demux_core::Demuxer>,
        rate: u32,
        ch: u8,
    }

    struct Snd {
        runs: Vec<AudioRun>,
        tb: Timebase,
        reels: Vec<ReelSnd>,
        slots: Vec<Slot>,
        /// Ended second sources, closed once drained.
        retiring: Vec<u32>,
        skip_ms: u64,
        xfades: u64,
        refused: u64,
        underruns: u64,
        frames: u64,
        /// The last time the mixer took a byte.
        took_ms: u64,
        said: u32,
    }

    static SND: crate::sync::Mutex<Option<Snd>> = crate::sync::Mutex::new(None);

    /// Frames at `rate` before timeline tick `t`.
    fn frames(tb: Timebase, t: i64, rate: u32) -> u64 {
        (tb.to_ns(t).max(0) as u128 * rate as u128 / 1_000_000_000) as u64
    }

    fn slot(track: usize, under: bool, id: u32, rate: u32, ch: u8, ri: usize) -> Slot {
        Slot { track, under, id, rate, ch, aud: None, ri, off: 0, set: false, play: false, pend: Vec::new(), span: false, cut: false, done: false }
    }

    /// The EDL's sound (see the section head); a refusal leaves the pictures playing silent.
    pub fn open(reels: &Reels, tl: &timeline_core::Timeline, seek_ms: u64) {
        *SND.lock() = None;
        let runs = match timeline_core::schedule::audio_runs(tl, 0..tl.end(), reels) {
            Ok(r) => r,
            Err(e) => {
                serial_println!("[nle] audio refused (schedule: {})", e);
                return;
            }
        };
        let rs: Vec<ReelSnd> = reels
            .reels
            .iter()
            .map(|r| {
                let t = r.a.map(|(i, _)| r.d.tracks()[i].clone());
                let ok = t.as_ref().is_some_and(|t| audio_core::container::plays(r.d.format(), &t.codec));
                let (rate, ch) = t.as_ref().map(|t| (t.sample_rate, t.channels.min(255) as u8)).unwrap_or((0, 0));
                ReelSnd { id: r.id, d: if ok { Some(r.d.share()) } else { None }, rate, ch }
            })
            .collect();
        let mut tracks: Vec<usize> = runs.iter().flat_map(|r| r.layers.iter().map(|l| l.track)).collect();
        tracks.sort_unstable();
        tracks.dedup();
        let mut slots = Vec::new();
        for t in tracks {
            let first = runs.iter().flat_map(|r| r.layers.iter()).filter(|l| l.track == t).find_map(|l| rs.iter().find(|r| r.id == l.src && r.d.is_some()));
            let Some(r) = first else {
                serial_println!("[nle] audio refused (track {}: no reel sound audio_core plays)", t);
                continue;
            };
            match mixer::open_fmt("nle", r.rate, r.ch, 16, 0) {
                Ok(id) => slots.push(slot(t, false, id, r.rate, r.ch, 0)),
                Err(why) => serial_println!("[nle] audio refused (track {}: {})", t, why),
            }
        }
        if slots.is_empty() {
            return;
        }
        *SND.lock() = Some(Snd {
            runs,
            tb: tl.tb,
            reels: rs,
            slots,
            retiring: Vec::new(),
            skip_ms: seek_ms,
            xfades: 0,
            refused: 0,
            underruns: 0,
            frames: 0,
            took_ms: crate::arch::ms(),
            said: 0,
        });
    }

    /// Write every source as far as the mixer takes (never waits); true while something is left to write.
    pub fn tick(jid: u32) -> bool {
        if GEN.load(Ordering::Acquire) != jid {
            return false;
        }
        match SND.lock().as_mut() {
            Some(s) => s.pass(),
            None => false,
        }
    }

    /// After the last picture: write the rest and let it play out; bounded by `STALL_MS` without progress.
    pub fn drain(jid: u32) {
        let mut mark = (crate::arch::ms(), u64::MAX);
        loop {
            if GEN.load(Ordering::Acquire) != jid {
                return;
            }
            let (busy, m) = match SND.lock().as_mut() {
                None => return,
                Some(s) => {
                    let left = s.pass();
                    let tops: Vec<u32> = s.slots.iter().filter(|x| !x.under).map(|x| x.id).collect();
                    let mut played = 0u64;
                    let mut playing = false;
                    for id in tops {
                        if let Some(p) = mixer::state(id) {
                            played += p.played;
                            playing |= !p.drained;
                        }
                    }
                    (left || playing || !s.retiring.is_empty(), played.wrapping_add(s.took_ms))
                }
            };
            if !busy {
                return;
            }
            let now = crate::arch::ms();
            if m != mark.1 {
                mark = (now, m);
            } else if now.saturating_sub(mark.0) > STALL_MS {
                serial_println!("[nle] audio drain stalled ms={} (no sink plays the mixer)", STALL_MS);
                return;
            }
            crate::arch::sched::sleep_ms(2);
        }
    }

    /// Close every source (on every path of the job) and fold the account into the report.
    pub fn close(rep: &mut EdlReport) {
        let Some(mut s) = SND.lock().take() else { return };
        let mut ids: Vec<u32> = s.slots.iter().filter(|x| !x.under || !x.done).map(|x| x.id).collect();
        ids.extend(s.retiring.iter().copied());
        for id in ids {
            if let Some(p) = mixer::close(id) {
                s.underruns += p.underruns as u64;
                s.frames += p.played;
            }
        }
        rep.snd_runs = s.runs.len() as u64;
        rep.xfades = s.xfades;
        rep.snd_refused = s.refused;
        rep.snd_underruns = s.underruns;
        rep.snd_frames = s.frames;
        serial_println!("[nle] audio exit runs={} xfades={} refused={} underruns={} frames={}", rep.snd_runs, s.xfades, s.refused, s.underruns, s.frames);
    }

    impl Snd {
        fn pass(&mut self) -> bool {
            self.reap();
            let mut left = false;
            let mut i = 0;
            while i < self.slots.len() {
                left |= self.feed(i); // a top slot may push its span's second source: fed in this pass too
                i += 1;
            }
            left
        }

        /// Close the ended second sources the sink has played out.
        fn reap(&mut self) {
            let mut k = 0;
            while k < self.retiring.len() {
                let id = self.retiring[k];
                if mixer::state(id).is_none_or(|p| p.drained) {
                    if let Some(p) = mixer::close(id) {
                        self.underruns += p.underruns as u64;
                        self.frames += p.played;
                    }
                    self.retiring.swap_remove(k);
                } else {
                    k += 1;
                }
            }
        }

        /// Slot `i`'s writes, run by run, until the mixer's ring is full; true while it has more to write.
        fn feed(&mut self, i: usize) -> bool {
            loop {
                {
                    let sl = &mut self.slots[i];
                    if sl.done {
                        return false;
                    }
                    if !sl.pend.is_empty() {
                        let took = mixer::write(sl.id, &sl.pend);
                        sl.pend.drain(..took.min(sl.pend.len()));
                        if took > 0 {
                            self.took_ms = crate::arch::ms();
                        }
                        if !self.slots[i].pend.is_empty() {
                            return true;
                        }
                    }
                }
                let Some((out_t, len)) = self.runs.get(self.slots[i].ri).map(|r| (r.out_t, r.len)) else {
                    return self.finish(i);
                };
                let rate = self.slots[i].rate;
                let start = frames(self.tb, out_t, rate);
                let cnt = frames(self.tb, out_t + len, rate).saturating_sub(start);
                let opened = self.slots.len();
                if !self.slots[i].set && !self.setup(i, start, cnt) {
                    return false; // a second source whose span ended
                }
                if self.slots.len() > opened {
                    return true; // the span's second source is fed next in this pass, before the top writes on (its pad holds)
                }
                let sl = &mut self.slots[i];
                if sl.off >= cnt {
                    sl.ri += 1;
                    sl.off = 0;
                    sl.set = false;
                    continue;
                }
                let k = (cnt - sl.off).min(CHUNK);
                make(sl, k);
                sl.off += k;
            }
        }

        /// Every run written: the source ends (what it holds plays out); a second source is closed once drained.
        fn finish(&mut self, i: usize) -> bool {
            let (id, under) = (self.slots[i].id, self.slots[i].under);
            mixer::end(id);
            self.slots[i].done = true;
            if under {
                self.retiring.push(id);
            }
            false
        }

        /// Run `ri`'s setup on slot `i` (written from frame `s0`, the Player's seek): the layer it plays (none = silence),
        /// its gain ramp, its decoder; the top slot opens the span's second source. False: a second source's span ended.
        fn setup(&mut self, i: usize, start: u64, cnt: u64) -> bool {
            let run = self.runs[self.slots[i].ri].clone();
            let (track, under, rate, id) = (self.slots[i].track, self.slots[i].under, self.slots[i].rate, self.slots[i].id);
            let li = (0..run.layers.len()).find(|&li| run.layers[li].track == track && run.under(li) == under);
            if under && li.is_none() {
                mixer::end(id);
                self.slots[i].done = true;
                self.retiring.push(id);
                return false;
            }
            let s0 = (self.skip_ms * rate as u64 / 1000).saturating_sub(start).min(cnt);
            if !under {
                let span = (0..run.layers.len()).any(|li| run.layers[li].track == track && run.under(li));
                if !span {
                    (self.slots[i].span, self.slots[i].cut) = (false, false);
                } else if s0 < cnt && !self.slots[i].span && !self.slots[i].cut {
                    self.slots[i].span = true; // the span's first written run opens its second source, once
                    self.open_under(i, &run);
                }
            }
            let mut play = false;
            if let Some(li) = li {
                let l = run.layers[li];
                let env = if self.slots[i].cut { [ENV_ONE, ENV_ONE] } else { l.env };
                let (e0, e1) = (audio_core::ramp::layer_q8(l.gain_cdb, env[0]) as i64, audio_core::ramp::layer_q8(l.gain_cdb, env[1]) as i64);
                let from = audio_core::ramp::at(e0, e1, s0 as i64, cnt as i64) as u16;
                let n48 = ((cnt - s0) * mixer::RATE as u64 / rate.max(1) as u64) as u32;
                if !mixer::set_gain_ramp(id, from, e1 as u16, n48) {
                    self.refuse("ramp-queue");
                }
                play = self.seat(i, l.src, l.src_t, l.seek.is_some(), s0);
            }
            let sl = &mut self.slots[i];
            sl.play = play;
            sl.off = s0;
            sl.set = true;
            true
        }

        /// The span's second source for top slot `top`: opened at the under layer's reel format and aligned to the top
        /// source (its backlog is all before the span: the top's earlier runs are taken); a refusal makes the span a cut.
        fn open_under(&mut self, top: usize, run: &AudioRun) {
            let track = self.slots[top].track;
            if self.slots.iter().any(|x| x.under && x.track == track && !x.done) {
                return self.refuse_span(top, "the previous span still plays");
            }
            let Some(l) = (0..run.layers.len()).find(|&li| run.layers[li].track == track && run.under(li)).map(|li| run.layers[li]) else { return };
            let Some((rate, ch)) = self.reels.iter().find(|r| r.id == l.src && r.d.is_some()).map(|r| (r.rate, r.ch)) else {
                return self.refuse_span(top, "no reel sound");
            };
            let id = match mixer::open_fmt("nle", rate, ch, 16, 0) {
                Ok(id) => id,
                Err(why) => return self.refuse_span(top, why),
            };
            if !mixer::align(id, self.slots[top].id) {
                let _ = mixer::close(id);
                return self.refuse_span(top, "align");
            }
            self.xfades += 1;
            let ri = self.slots[top].ri;
            self.slots.push(slot(track, true, id, rate, ch, ri));
        }

        fn refuse_span(&mut self, top: usize, why: &str) {
            self.slots[top].cut = true;
            self.refuse(why);
        }

        fn refuse(&mut self, why: &str) {
            self.refused += 1;
            if self.refused <= 4 {
                serial_println!("[nle] audio crossfade: second source refused ({})", why);
            }
        }

        /// Slot `i`'s decoder at layer source tick `src_t` + `s0` frames: kept when it continues (no `seek`, the same
        /// source, at or before the frame), else reopened through audio_core's door and seeked; then skipped forward to
        /// the frame. False when the reel has no sound this slot can play (the run is silence).
        fn seat(&mut self, i: usize, src: SourceId, src_t: i64, seek: bool, s0: u64) -> bool {
            let (rate, ch) = (self.slots[i].rate, self.slots[i].ch);
            let Some(r) = self.reels.iter().find(|r| r.id == src) else { return false };
            let Some(d) = r.d.as_ref().filter(|_| r.rate == rate && r.ch == ch) else {
                self.said += 1;
                if self.said <= 4 {
                    serial_println!("[nle] audio refused (reel {}: no sound in the track's format {} Hz x {})", src.0, rate, ch);
                }
                return false;
            };
            let want = frames(self.tb, src_t, rate) + s0;
            let keep = !seek && self.slots[i].aud.as_ref().is_some_and(|a| a.src == src && a.pos <= want);
            if !keep {
                let mut dec = match audio_core::container::open_demuxed(d.share()) {
                    Ok(source) => audio_core::Decoder::from_source(source),
                    Err(e) => {
                        self.said += 1;
                        if self.said <= 4 {
                            serial_println!("[nle] audio refused (reel {}: {})", src.0, e);
                        }
                        self.slots[i].aud = None;
                        return false;
                    }
                };
                let pos = match dec.seek(want * 1000 / rate.max(1) as u64) {
                    Ok(Some(p)) => p.landed,
                    _ => 0, // no table: decoded from the top (a failed seek left the stream at its start)
                };
                self.slots[i].aud = Some(Aud { src, dec, pos });
            }
            let Some(a) = self.slots[i].aud.as_mut() else { return false };
            let ch = ch.max(1) as usize;
            let mut scratch = alloc::vec![0i32; CHUNK as usize * ch];
            while a.pos < want {
                let k = ((want - a.pos) as usize).min(CHUNK as usize);
                let got = a.dec.next_i32(&mut scratch[..k * ch]).unwrap_or(0);
                if got == 0 {
                    break; // the reel ends before the frame: the run plays silence
                }
                a.pos += got as u64;
            }
            true
        }
    }

    /// `k` frames of slot `sl` into its pending bytes (PCM16 LE): its decoder's, silence past the reel's end or in a gap.
    fn make(sl: &mut Slot, k: u64) {
        let ch = sl.ch.max(1) as usize;
        let mut buf = alloc::vec![0i32; k as usize * ch];
        if let (true, Some(a)) = (sl.play, sl.aud.as_mut()) {
            let got = a.dec.next_i32(&mut buf).unwrap_or(0);
            a.pos += got as u64;
        }
        sl.pend.reserve(buf.len() * 2);
        for v in &buf {
            sl.pend.extend_from_slice(&((*v >> 16) as i16).to_le_bytes());
        }
    }
}

/// The silent twin: an image without an audio module plays the EDL's pictures alone.
#[cfg(not(any(feature = "uvc", feature = "hda-tone")))]
mod snd {
    pub fn open(_: &super::Reels, _: &timeline_core::Timeline, _: u64) {
        serial_println!("[nle] audio refused (no audio module in this image)");
    }
    pub fn tick(_: u32) -> bool {
        false
    }
    pub fn drain(_: u32) {}
    pub fn close(_: &mut super::EdlReport) {}
}

// ── NLE6MIXFACE (rmbp-ledger B957): the face's transition preview — the first frame a Player seek shows ──────────────
// handlers.md §5 NLE6's NLE6MIX-FACE. The on-UnaOS face is the Player's EDL mode; its scrubber / arrow seek is `vid::seek` ->
// [`start_at`] -> [`edl_run`], which walks every step from 0 (each transition step mixed by [`edl_mix`] through timeline_core's
// ONE `vmix::mix_step`) and queues from the seek's tick on: a seek or pause inside a transition shows the MIXED frame, no
// second mixer. This records what the seek showed: the job's first queued frame after a seek > 0, its tick, whether
// `edl_mix` mixed it (its `rep.mixes` entry is this tick) and its FNV-1a 64. Wire: `[nle] face preview tick=<n> mixed=<0|1>`
// (once per seek job); `tests nle` takes it through [`face_preview_take`] (player.rs `nle_fx::face_leg`).

/// `(output tick, mixed, FNV-1a 64 of the RGBA)` of the last seek job's first queued frame; written by `play-vedl`, taken by the witness.
static PREVIEW: crate::sync::Mutex<Option<(i64, bool, u64)>> = crate::sync::Mutex::new(None);

/// The seek job's first queued frame (`seek_ms > 0`, nothing queued yet): named on the wire and kept for the witness.
fn face_preview(seek_ms: u64, rep: &EdlReport, out_t: i64, f: &Frame) {
    if seek_ms == 0 || rep.frames > 0 {
        return;
    }
    let mix = rep.mixes.last().filter(|m| m.0 == out_t).map(|m| m.1);
    let sum = mix.unwrap_or_else(|| frame_fnv_rgba(&unpack(f.px.rgb())));
    serial_println!("[nle] face preview tick={} mixed={}", out_t, mix.is_some() as u8);
    *PREVIEW.lock() = Some((out_t, mix.is_some(), sum));
}

/// The last seek job's preview, cleared (`None` before one queued a frame).
pub fn face_preview_take() -> Option<(i64, bool, u64)> {
    PREVIEW.lock().take()
}

// ── NLE6KEYS (rmbp-ledger B961): the kernel face's editing keys and its track view ───────────────────────────────────────
//
// ONE editor state per EDL open in the Player (`EDIT`): seeded by the EDL job's open (`edl_run` -> [`edit_seed`]) and kept
// across the job's restarts (a seek, an edit), so undo survives them; cleared when the Player closes ([`edit_close`]). The
// Player's key door (player.rs `key_route` -> `edit_post`, run on the pass by `edit_pass`) hands a byte to [`edit_key`]:
// [`EDIT_KEYS`] names timeline_core's verb (`EditorState::verb`, the ONE dispatch the host face calls), on the clip at the
// playhead of the highest-numbered video track that has one (what the schedule shows). A changed timeline is written back
// through timeline_core's ONE writer (`edl::write`, titled by the file's stem) and export.rs's `write_file`, and the
// Player's seek door (`vid::seek`) restarts the job at the playhead over the edited state. [`view_band`]: the picture
// rect's lower band shows timeline_core's ONE track view (`view::paint_in`, `Role::Tracks`): the host face's pixels.
// Wire: `[nle] edit <verb> at=<tick> clips=<n> undo=<n> saved=<bytes|refused(..)>`, `[nle] edit <verb> at=<tick> refused
// (<why>)`, `[nle] view w=<n> h=<n> rows=<n> at=<tick> labels=<n>` (once per seed). Witness: player.rs `tests nle` (`keys=`).
// NLE6KEYSLABELS (rmbp-ledger B968): the view band's labels (the ruler's timecode, each clip's reel) go through the view's
// `Text` hook to the kernel's ONE text seam ([`ViewLabels`]: `video::text::draw_with`, `Face::Body`, blended by
// `video::font::blend`) at the host's positions; `+` / `=` / `-` / `0` are the ONE zoom (`EditorState::verb` `zoom+` /
// `zoom-` / `zoom0`) on the Player's editor state ([`edit_zoom`]: no save, the seek door re-presents the band),
// `[nle] edit <zoom+|zoom-|zoom0> at=<tick> zoom=<px>`.

struct Edit {
    path: String,
    st: timeline_core::editor::EditorState,
    said: bool,
}

static EDIT: crate::sync::Mutex<Option<Edit>> = crate::sync::Mutex::new(None);

/// The kernel face's key bindings (bytes) onto timeline_core's editor verbs: `s` split, `x` / Delete / Backspace delete,
/// `u` / `z` undo, `Z` redo, `[` / `]` the in point -1 / +1, `{` / `}` the out point -1 / +1, `i` / `o` the in / out point
/// to the playhead (the host face's meanings; Space and the arrows stay the Player's transport); NLE6KEYSLABELS (B968):
/// `+` / `=` zoom in, `-` zoom out, `0` the default zoom (the host's `KEYS` + / = / -).
const EDIT_KEYS: [(u8, &str); 17] = [
    (b's', "split"),
    (b'x', "delete"),
    (0x7F, "delete"),
    (0x08, "delete"),
    (b'u', "undo"),
    (b'z', "undo"),
    (b'Z', "redo"),
    (b'[', "["),
    (b']', "]"),
    (b'{', "{"),
    (b'}', "}"),
    (b'i', "mark-in"),
    (b'o', "mark-out"),
    (b'+', "zoom+"),
    (b'=', "zoom+"),
    (b'-', "zoom-"),
    (b'0', "zoom0"),
];

/// The EDL job's open: a fresh EDL (or another path) seeds the Player's editor state; the same path keeps it (a restart
/// after a seek or an edit) and the job plays ITS timeline. An Encode (export) job neither seeds nor reads it.
fn edit_seed(jid: u32, path: &str, st: timeline_core::editor::EditorState) -> timeline_core::editor::EditorState {
    if let super::export::Sink::Encode = super::export::sink(jid) {
        return st;
    }
    let mut g = EDIT.lock();
    if let Some(e) = g.as_ref().filter(|e| e.path == path) {
        return timeline_core::editor::EditorState::new(e.st.timeline.clone());
    }
    let fresh = timeline_core::editor::EditorState::new(st.timeline.clone());
    *g = Some(Edit { path: String::from(path), st, said: false });
    fresh
}

/// The Player's EDL at `path` has an editor state (the keys are taken).
pub fn edit_live(path: &str) -> bool {
    EDIT.try_lock().map(|g| g.as_ref().is_some_and(|e| e.path == path)).unwrap_or(false)
}

/// The editor verb a key byte names ([`EDIT_KEYS`]).
pub fn edit_verb(b: u8) -> Option<&'static str> {
    EDIT_KEYS.iter().find(|k| k.0 == b).map(|k| k.1)
}

/// The output tick shown at `ms` (the inverse of the job's `out_ms`).
fn tick_of(tb: timeline_core::Timebase, ms: u64) -> i64 {
    (ms as i128 * 1_000_000 * tb.den as i128 / (tb.num.max(1) as i128 * 1_000_000_000)) as i64
}

/// **One editing key** on the Player's EDL at `path`, the playhead at `ms`: the verb on the shared editor state, the EDL
/// saved back when the timeline changed. `Some(ms)`: the playhead to restart the job at (the seek door); `None`: refused,
/// unchanged, or no EDL open at `path`.
pub fn edit_key(path: &str, b: u8, ms: u64) -> Option<u64> {
    let verb = edit_verb(b)?;
    let (at, tl, clips, undo, back) = {
        let mut g = EDIT.lock();
        let e = g.as_mut().filter(|e| e.path == path)?;
        let st = &mut e.st;
        let tb = st.timeline.tb;
        st.seek(tick_of(tb, ms));
        let at = st.playhead;
        if verb.starts_with("zoom") { return edit_zoom(st, verb, at); } if verb != "undo" && verb != "redo" { // NLE6KEYSLABELS (rmbp-ledger B968): a zoom key neither reselects nor saves (section tail)
            let top = (0..st.timeline.tracks.len())
                .rev()
                .filter(|&i| st.timeline.tracks[i].kind == timeline_core::Kind::Video)
                .find_map(|i| st.timeline.clip_at(i, at).map(|c| (i, c)));
            match top {
                Some((t, i)) => {
                    let _ = st.select(t, i);
                }
                None => st.deselect(),
            }
        }
        let before = st.timeline.clone();
        match st.verb(verb) {
            Some(Ok(())) if st.timeline != before => {}
            Some(Ok(())) => {
                serial_println!("[nle] edit {} at={} unchanged", verb, at);
                return None;
            }
            Some(Err(why)) => {
                serial_println!("[nle] edit {} at={} refused ({})", verb, at, why);
                return None;
            }
            None => return None,
        }
        let clips: usize = st.timeline.tracks.iter().map(|t| t.clips.len()).sum();
        (at, st.timeline.clone(), clips, st.undo_depth(), (tb.to_ns(at).max(0) as u64) / 1_000_000)
    };
    let leaf = path.rsplit('/').next().unwrap_or(path);
    let stem = if is_edl(leaf) { &leaf[..leaf.len() - 4] } else { leaf };
    let saved = match timeline_core::edl::write(&tl, stem) {
        Ok(text) => match super::export::write_file(path, text.as_bytes()) {
            Ok(n) => alloc::format!("{}", n),
            Err(e) => alloc::format!("refused({})", e),
        },
        Err(e) => alloc::format!("refused({})", e),
    };
    serial_println!("[nle] edit {} at={} clips={} undo={} saved={}", verb, at, clips, undo, saved);
    Some(back)
}

/// The Player's editor timeline (the witness reads it).
pub fn edit_timeline() -> Option<timeline_core::Timeline> {
    EDIT.lock().as_ref().map(|e| e.st.timeline.clone())
}

/// The Player closed: its editor state goes (a later open of the same EDL reads the saved file afresh).
pub fn edit_close() {
    *EDIT.lock() = None;
}

/// The present's view band: with an editor state for the live job's EDL, timeline_core's track view (`Role::Tracks`, the
/// playhead at the presented frame's tick) over the picture rect's lower `RULER_H + rows * ROW_H` rows (at most half
/// of it); returns the rows left for the picture (`vh` unchanged when there is no view).
fn view_band(dst: &mut [u32], w: usize, vh: usize, pts_ms: u64) -> usize {
    use timeline_core::view;
    let Some(live) = JOB.try_lock().and_then(|g| g.as_ref().map(|j| j.path.clone())) else { return vh };
    let Some(mut g) = EDIT.try_lock() else { return vh };
    let Some(e) = g.as_mut().filter(|e| e.path == live) else { return vh };
    let rows = e.st.timeline.tracks.len();
    let band = (view::RULER_H as usize + rows * view::ROW_H as usize).min(vh / 2);
    if band == 0 || w == 0 || dst.len() < vh * w {
        return vh;
    }
    let tb = e.st.timeline.tb;
    e.st.seek(tick_of(tb, pts_ms));
    let l = view::Layout::of_role(view::Role::Tracks, w as u32, band as u32, &e.st);
    let mut lab = ViewLabels(0);
    let px = pack(&view::paint_in(&e.st, l, view::Order::Rgba, Some(&mut lab as &mut dyn view::Text), None));
    let y0 = vh - band;
    dst[y0 * w..vh * w].copy_from_slice(&px);
    if !e.said {
        e.said = true;
        serial_println!("[nle] view w={} h={} rows={} at={} labels={}", w, band, rows, e.st.playhead, lab.0);
    }
    y0
}

/// NLE6KEYSLABELS (rmbp-ledger B968): a zoom key on the Player's editor state — `EditorState::verb`'s ONE zoom (the host
/// face's words); the EDL carries no zoom, so nothing is saved. `Some(ms)`: the playhead for the seek door, which
/// re-presents the frame and with it the band at the new zoom; `None`: at a bound, unchanged.
fn edit_zoom(st: &mut timeline_core::editor::EditorState, verb: &str, at: i64) -> Option<u64> {
    let z0 = st.zoom;
    if !matches!(st.verb(verb), Some(Ok(()))) || st.zoom == z0 {
        serial_println!("[nle] edit {} at={} unchanged", verb, at);
        return None;
    }
    serial_println!("[nle] edit {} at={} zoom={}", verb, at, st.zoom);
    Some((st.timeline.tb.to_ns(at).max(0) as u64) / 1_000_000)
}

/// The cell's top sits this far above the view hook's baseline: the ruler label's baseline is the ruler's top + 9, so the
/// 16-px `Face::Body` cell fills the 16-px ruler exactly.
const LABEL_RISE: f32 = 9.0;

/// NLE6KEYSLABELS (rmbp-ledger B968): the view's label hook over the kernel's ONE text seam — `video::text::draw_with` in
/// `Face::Body` (the fixed 7x16 cell), each covered pixel blended into the band's canvas by `video::font::blend`, inside
/// the hook's clip columns; the host's positions, the kernel's glyphs. Counts the labels drawn (the `[nle] view` line).
struct ViewLabels(usize);

impl timeline_core::view::Text for ViewLabels {
    fn text(&mut self, c: &mut timeline_core::view::Canvas, s: &str, x: f32, baseline: f32, clip: (i32, i32)) {
        let (w, h) = (c.w as isize, c.h as isize);
        let (lo, hi) = (clip.0.max(0) as isize, (clip.1 as isize).min(w));
        if x < 0.0 || x as isize >= hi {
            return;
        }
        let x = x as usize;
        let l = timeline_core::view::LABEL;
        let ink = (l[0] as u32) << 16 | (l[1] as u32) << 8 | l[2] as u32;
        let (ri, bi) = match c.order {
            timeline_core::view::Order::Rgba => (0, 2),
            timeline_core::view::Order::Bgra => (2, 0),
        };
        let top = (baseline - LABEL_RISE) as isize;
        let px = &mut c.px;
        let _ = super::text::draw_with(s.as_bytes(), false, super::text::Face::Body, x, top, (hi - x as isize) as usize, ink, &mut |gx, gy, a| {
            if gx < lo || gx >= hi || gy < 0 || gy >= h {
                return;
            }
            let i = ((gy * w + gx) * 4) as usize;
            let Some(p) = px.get_mut(i..i + 4) else { return };
            let bg = (p[ri] as u32) << 16 | (p[1] as u32) << 8 | p[bi] as u32;
            let o = super::font::blend(bg, ink, a);
            p[ri] = (o >> 16) as u8;
            p[1] = (o >> 8) as u8;
            p[bi] = o as u8;
        });
        self.0 += 1;
    }
}

// ── KERNELSCALECONVERT (rmbp-ledger B971): the frame's planes, converted where they are used ─────────────────────────────
// handlers.md §5 NLEPROBE-HDR's KERNEL-SCALE-CONVERT. A decoded VP8 / AV1 picture rides the queue as the decoder's own planes
// (`vp8_core::Yuv420`, `av1_core::image::Planes`) with the track's colour. A consumer that needs the whole picture (the
// exporter, `edl_mix`, the `coded` and face-preview hashes) calls [`Px::rgb`]: the existing full conversion (`colour_rgba_vp8`
// / `colour_rgba_av1` -> `pack`), once, cached, so every pin hashes the bytes it hashed before. The present path
// ([`Frame::rows`]) converts only the rows the window shows: VP8 (Nearest) sampled by `yuv_core::scale::nearest_rows` and
// converted per output row by `yuv_core::to_rgba`; AV1 (Bilinear) one sampled source row at a time by `yuv_core::to_rgba_rows`
// (its neighbour chroma rows read from the whole planes), the columns then sampled; every row and column index is the
// scaler's own. Bit for bit the full conversion then nearest-sampled. A dropped or run-up frame converts nothing. One holder
// at a time (the job, the queue under `JOB`, the presenting pass, the poster under `POSTER`): no lock of its own.

/// A frame's pixels: 0RGB, or a decoder's planes converted on first use ([`Px::rgb`]).
pub struct Px {
    src: Src,
    rgb: core::cell::OnceCell<Vec<u32>>,
}

/// What a [`Px`] was made from.
enum Src {
    /// 0RGB (utp1, the stand-in, the EDL black, a mixed frame): the cell holds it.
    Rgb,
    Vp8(vp8_core::Yuv420, Option<Colour>),
    Av1(Box<av1_core::image::Planes>, Option<Colour>),
}

impl From<Vec<u32>> for Px {
    fn from(px: Vec<u32>) -> Px {
        Px { src: Src::Rgb, rgb: core::cell::OnceCell::from(px) }
    }
}

impl Px {
    /// A decoder's planes, not yet converted.
    fn planes(src: Src) -> Px {
        Px { src, rgb: core::cell::OnceCell::new() }
    }

    /// The whole picture as 0RGB: converted on the first call (byte for byte the decode job's former conversion), then cached.
    pub fn rgb(&self) -> &[u32] {
        self.rgb.get_or_init(|| match &self.src {
            Src::Vp8(y, c) => pack(&colour_rgba_vp8(y, *c)),
            Src::Av1(p, c) => pack(&colour_rgba_av1(p, *c)),
            Src::Rgb => Vec::new(),
        })
    }
}

/// A `w x h` 0RGB buffer as a mono `Picture<u32>` (how `nearest_rows` samples packed pixels).
fn mono(w: usize, h: usize, px: &[u32]) -> yuv_core::Picture<'_, u32> {
    yuv_core::Picture { w, h, y: px, u: &[], v: &[], y_stride: w, uv_stride: 0, ss: (0, 0), mono: true, bit_depth: 8, full: true }
}

impl Frame {
    /// Every sample [`Frame::rows`] reads lies inside the frame's pixels (`w`, `h` > 0).
    pub fn fits(&self) -> bool {
        match (self.px.rgb.get(), &self.px.src) {
            (Some(px), _) => yuv_core::scale::fits(&mono(self.w, self.h, px)),
            (None, Src::Vp8(y, c)) => yuv_core::scale::fits(&vp8_picture(y, *c).0),
            (None, Src::Av1(p, c)) => yuv_core::scale::fits(&av1_picture(p, *c).0),
            (None, Src::Rgb) => false,
        }
    }

    /// The frame nearest-sampled to `dw x dh`, each output row handed to `row(r, 0RGB)`: a cached or 0RGB frame through
    /// `nearest_rows` (KERNELSCALEONE's blit); planes convert only the shown rows. `fits()`; `dw`, `dh` > 0.
    pub fn rows(&self, dw: usize, dh: usize, mut row: impl FnMut(usize, &[u32])) {
        let mut out = alloc::vec![0u32; dw];
        match (self.px.rgb.get(), &self.px.src) {
            (Some(px), _) => yuv_core::scale::nearest_rows(&mono(self.w, self.h, px), dw, dh, &mut out, &mut [], &mut [], |r, s| row(r, s.y)),
            (None, Src::Vp8(y, c)) => {
                let (p, m) = vp8_picture(y, *c);
                let (mut ry, mut ru, mut rv, mut rgba) = (alloc::vec![0u8; dw], alloc::vec![0u8; dw], alloc::vec![0u8; dw], alloc::vec![0u8; dw * 4]);
                yuv_core::scale::nearest_rows(&p, dw, dh, &mut ry, &mut ru, &mut rv, |r, s| {
                    yuv_core::to_rgba(s, m, yuv_core::Chroma::Nearest, false, &mut rgba);
                    row(r, &pack(&rgba));
                });
            }
            (None, Src::Av1(pl, c)) => {
                let (p, m) = av1_picture(pl, *c);
                let idx: Vec<u32> = (0..self.h as u32).collect(); // the source row each output row samples: the scaler's own index
                let (mut one, mut rgba) = ([0u32; 1], alloc::vec![0u8; self.w * 4]);
                let mut line: (usize, Vec<u32>) = (usize::MAX, Vec::new());
                yuv_core::scale::nearest_rows(&mono(1, self.h, &idx), 1, dh, &mut one, &mut [], &mut [], |r, s| {
                    let sy = s.y[0] as usize;
                    if line.0 != sy {
                        yuv_core::to_rgba_rows(&p, sy, sy + 1, m, yuv_core::Chroma::Bilinear, false, &mut rgba);
                        line = (sy, pack(&rgba));
                    }
                    yuv_core::scale::nearest_rows(&mono(self.w, 1, &line.1), dw, 1, &mut out, &mut [], &mut [], |_, t| row(r, t.y));
                });
            }
            (None, Src::Rgb) => {}
        }
    }
}

/// A VP8 picture as yuv_core's `Picture` (8-bit 4:2:0) and the track colour's matrix: the ONE pick at the picture's size
/// (`colour_rgba_vp8`'s and the shown rows').
fn vp8_picture(y: &vp8_core::Yuv420, c: Option<Colour>) -> (yuv_core::Picture<'_, u8>, yuv_core::Matrix) {
    let (w, h) = (y.width as usize, y.height as usize);
    let (m, full) = yuv_core::pick(c.map(|c| c.matrix), c.and_then(|c| c.full_range), y.width, y.height);
    (yuv_core::Picture { w, h, y: &y.y, u: &y.u, v: &y.v, y_stride: w, uv_stride: y.chroma_width() as usize, ss: (1, 1), mono: false, bit_depth: 8, full }, m)
}

/// An AV1 picture as av1_core's own `picture` with the pick's range, and the matrix (`colour_rgba_av1`'s and the shown rows').
fn av1_picture(planes: &av1_core::image::Planes, c: Option<Colour>) -> (yuv_core::Picture<'_, u16>, yuv_core::Matrix) {
    let (m, full) = yuv_core::pick(c.map(|c| c.matrix), c.and_then(|c| c.full_range), planes.width, planes.height);
    (yuv_core::Picture { full, ..av1_core::image::picture(planes) }, m)
}

// ── AV1INBANDSWITCH (LEDGER SR373): a new sequence header mid-stream re-picks the colour (handlers.md §5 NLEPROBE-HDR) ──────────
// `Pic::decode_raw`'s Av1 arm decides each frame's colour right after the decode: the open's (`Track::colour`) until the decoder's
// current sequence header states other code points than the first decoded one, then av1_core's ONE rule
// (`StreamDecoder::switch` / `switch_codes`: a field the open took from the bitstream follows the new header, a container-stated
// one stays), the host `Av1Decoder`'s too. Wire: `[vplay] colour switch at <pts ticks> matrix=<a>-><b> range=<full|limited>`, once
// per change, printed here (in decode_raw), never at the conversion.

/// AV1INBANDSWITCH (SR373): the colour an AV1 frame converts by, `c` the open's; the switch line on a change.
fn inband_colour(d: &mut av1_core::image::StreamDecoder, c: Option<Colour>, pts: i64) -> Option<Colour> {
    let (now, changed) = d.switch(c.map(|c| (c.primaries, c.transfer, c.matrix, c.full_range)));
    let full = now.map_or(c.and_then(|c| c.full_range), |n| n.3);
    if let Some((a, b)) = changed {
        serial_println!("[vplay] colour switch at {} matrix={}->{} range={}", pts, a, b, if full == Some(true) { "full" } else { "limited" });
    }
    match now {
        Some((primaries, transfer, matrix, full_range)) => Some(Colour { primaries, transfer, matrix, full_range, mastering: c.and_then(|c| c.mastering), light: c.and_then(|c| c.light) }),
        None => c,
    }
}
