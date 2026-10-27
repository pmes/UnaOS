// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — fulfiller
//!
//! NLE6EXPORT (rmbp-ledger B959; handlers.md §5 NLE6's NLE6-EXPORT, BP-handlers-14). CODEX §2: the editor's export is
//! Stria's; on UnaOS the kernel fulfils it over the shared cores — h264enc_core's RGBA face (`rgba::RgbaEncoder`, the ONE
//! frame-to-encoder glue Stria's host `H264Encoder` drives too) and demux_core's `build::mp4` — never a second encoder,
//! renderer or mixer.
//!
//! * ONE RENDER PATH: `nle export <edl> <out.mp4>` arms an [`Exp`] and starts vplay's EDL job on the EDL ([`start`]). The
//!   job's schedule loop (`vplay::edl_run`) asks [`sink`] per video step: [`Sink::Encode`] hands the step's frame (RGBA, by
//!   vplay's `unpack`) to [`frame`] instead of the queue (`room` + `push_back`); nothing is presented. The frame is the one
//!   the Player would have shown: the same decode, the same transition mix.
//! * ONE SOUND PATH: vplay's `snd` reaches the mixer through [`bus`] (its `use … as mixer`). `bus` forwards to
//!   `audio::mixer` unless the job armed a capture ([`arm_audio`], an Encode job only): then the same calls run on an
//!   OFFLINE `audio::mixer::Mix` — the mixer's own engine (per-frame ramps, `align`, the Sinc ingest, `period`) — pumped as
//!   an infinitely fast sink (after every write / end / state: `period` while `ready`, each period appended to the
//!   capture). snd's feed, crossfade and drain run unchanged; the capture is 48 kHz stereo PCM16, the mixer's own format.
//! * THE FILE (NLE6EXPORTONE, rmbp-ledger B963): ONE `build::Mp4Stream` (ftyp, the mdat, the moov at the end). The `avc1`
//!   track is `build::coded_video_spec` (the constructor Stria's `video_track` uses too); each unit is written to the file as
//!   [`frame`] gets it (`build::coded_video_sample`: its (dts, pts) x the tick, so B pictures write `ctts`), so the export
//!   holds no more than the encoder's reorder window. At [`finish`] (after the job's sound closed) the flush's units, then,
//!   when sound was captured, a `sowt` PCM16 track (`build::pcm16_track`) cut to the video's length (an AAC encoder is
//!   owed: NLE6EXPORT-AAC), then the moov. Written as the shell writes (`write_veto` at start, the kernel principal) through
//!   the ONE VFS writer, `MountTable::put_file` / `write_all_at`; a refused, failed or superseded export unlinks its
//!   partial file. `--bitrate KBPS` is h264enc_core's `Config.rate` (CBR), the field Stria's `with_bitrate` sets.
//! * LOCKS: [`ENC`] (the armed export) is a leaf; [`frame`] TAKES the export out of it and encodes with no lock held (a
//!   1080p picture is far past the lock registry's 50 ms), putting it back unless superseded. `bus::CAP` (the capture) is
//!   taken under vplay's `snd::SND` and takes nothing (the live forward drops it first): the one new edge is SND -> CAP.
//! * The export takes vplay's ONE job: refused while the Player (the job's one presenter) is open (`player::is_open`;
//!   `vplay::finished` cannot say it: a superseded job ends with neither eos nor an error).
//!
//! Wire: `[nle] export start edl= out= qp= gop= bframes= bitrate= jid=` · `[nle] export progress pct=<10..100> frames=<n>/<total>` ·
//! `[nle] export frames=<n> bytes=<n> ms=<n> audio=<mixed>/<target> bframes=<n> out=<p>` · `[nle] export refused why=<w> (<reason>)`.
//! The witness is player.rs `tests nle` (`export=` on `:: NLE: … ::`). Owed: the Pi budget, a progress UI on the kernel
//! face, MKV, AAC, the sound streamed too (NLE6EXPORT-STREAMPCM: the capture's PCM is held whole until [`finish`]).

use alloc::string::String;
use alloc::vec::Vec;

use demux_core::build;
use h264enc_core::rgba::RgbaEncoder;
use timeline_core::Timebase;

/// The export's knobs (`nle export … --qp --gop --bframes`).
#[derive(Clone, Copy, Debug)]
pub struct Opts {
    /// h264enc_core's constant QP, 16..=40 (refused by the encoder at the first frame otherwise).
    pub qp: u8,
    /// An IDR every `gop` frames; 0 = one per second of the EDL's rate.
    pub gop: u32,
    /// B pictures between anchors (0..=4; h264enc_core's `Config::bframes`).
    pub bframes: u8,
    /// A constant bit rate in kbit/s (h264enc_core's `Config.rate`, CBR; `qp` its starting QP); 0 = constant QP.
    pub bitrate: u32,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { qp: 26, gop: 0, bframes: 0, bitrate: 0 }
    }
}

/// Where the EDL job's frames go: the Player's queue, or this exporter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sink {
    Present,
    Encode,
}

/// What a finished export wrote (`err` set when it was refused).
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub frames: u64,
    pub bytes: u64,
    pub ms: u64,
    /// The capture's 48 kHz frames mixed, and the frames the video's length needs (the track is cut / padded to it).
    pub audio_mixed: u64,
    pub audio_target: u64,
    pub audio: bool,
    pub bframes: u8,
    /// The video samples' bytes (the `--bitrate` witness).
    pub video_bytes: u64,
    pub out: String,
    pub err: Option<String>,
}

/// One armed export: the job it rides, the encoder, the file being streamed and its video track, the progress.
struct Exp {
    jid: u32,
    out: String,
    opts: Opts,
    rgba: RgbaEncoder,
    stream: Option<build::Mp4Stream<VfsOut>>,
    vtrack: usize,
    /// The file was created by this export (so a refusal unlinks it; a file that was there before is never touched).
    begun: bool,
    size: (u32, u32),
    tb: Timebase,
    total: u64,
    frames: u64,
    next_pct: u64,
    t0: u64,
}

static ENC: crate::sync::Mutex<Option<Exp>> = crate::sync::Mutex::new(None);
static LAST: crate::sync::Mutex<Option<(u32, Report)>> = crate::sync::Mutex::new(None);

/// **Export `edl` to `out` (an `.mp4`)**: arm the exporter and start vplay's EDL job on it; the job id. The export runs on
/// the job's worker core; [`report`] answers once it has finished.
pub fn start(edl: &str, out: &str, opts: Opts) -> Result<u32, String> {
    if !super::vplay::is_edl(edl) {
        return Err(alloc::format!("{} is not an .edl", edl));
    }
    let n = out.len();
    if !(n > 4 && out.is_char_boundary(n - 4) && out[n - 4..].eq_ignore_ascii_case(".mp4")) {
        return Err(alloc::format!("{}: the export writes .mp4 (MKV owed)", out));
    }
    if super::player::is_open() {
        return Err(String::from("the Player is open (close it first: the export takes vplay's one job)"));
    }
    if let Ok(Some(why)) = crate::shell::vfs_mount_table().write_veto(out) {
        return Err(alloc::format!("{}: read-only ({})", out, why));
    }
    let cfg = h264enc_core::Config { qp: opts.qp, gop: opts.gop, bframes: opts.bframes, ..Default::default() };
    let t0 = crate::arch::ms();
    let jid = {
        // armed under the lock across the start, so the job's first `sink` sees its own jid
        let mut g = ENC.lock();
        *g = Some(Exp { jid: 0, out: String::from(out), opts, rgba: RgbaEncoder::new(cfg), stream: None, vtrack: 0, begun: false, size: (0, 0), tb: Timebase { num: 1, den: 25 }, total: 1, frames: 0, next_pct: 10, t0 });
        let jid = super::vplay::start(edl);
        if let Some(e) = g.as_mut() {
            e.jid = jid;
        }
        jid
    };
    serial_println!("[nle] export start edl={} out={} qp={} gop={} bframes={} bitrate={} jid={}", edl, out, opts.qp, opts.gop, opts.bframes, opts.bitrate, jid);
    Ok(jid)
}

/// The sink of EDL job `jid`.
pub fn sink(jid: u32) -> Sink {
    if ENC.lock().as_ref().is_some_and(|e| e.jid == jid) { Sink::Encode } else { Sink::Present }
}

/// Before the job's `snd::open`: an Encode job's sound goes to the offline capture ([`bus`]).
pub fn arm_audio(jid: u32) {
    if sink(jid) == Sink::Encode {
        bus::arm(jid);
    }
}

/// The step's frame (`w x h` straight RGBA) into the encoder; `tb` the EDL's time base and `total` its video steps (the
/// progress). Encodes with no lock held.
pub fn frame(jid: u32, tb: Timebase, total: u64, w: usize, h: usize, rgba: &[u8]) -> Result<(), String> {
    let Some(mut e) = ENC.lock().take_if(|e| e.jid == jid) else { return Err(String::from("superseded")) };
    if e.frames == 0 {
        e.tb = tb;
        e.total = total.max(1);
        if e.rgba.cfg.gop == 0 {
            e.rgba.cfg.gop = (tb.den / tb.num.max(1)).clamp(1, 600) as u32;
        }
        if e.opts.bitrate > 0 {
            // tools/nle's `with_bitrate(k, tb.den, tb.num)`: the same `Config.rate`, set before the first picture builds the encoder
            e.rgba.cfg.rate = Some(h264enc_core::RateControl { bits_per_second: e.opts.bitrate.saturating_mul(1000), fps_num: tb.den as u32, fps_den: tb.num.max(1) as u32, mode: h264enc_core::RcMode::Cbr });
        }
    }
    e.size = (w as u32, h as u32);
    let r = match e.rgba.push_rgba(w as u32, h as u32, rgba) {
        Ok(u) => e.stream_units(u).map_err(|m| alloc::format!("write-refused: {}", m)),
        Err(err) => Err(alloc::format!("{}", err)),
    };
    e.frames += 1;
    while e.next_pct <= 100 && e.frames * 100 >= e.next_pct * e.total {
        serial_println!("[nle] export progress pct={} frames={}/{}", e.next_pct, e.frames, e.total);
        e.next_pct += 10;
    }
    let mut g = ENC.lock();
    if g.is_none() {
        *g = Some(e); // a new export armed meanwhile supersedes this one (its job ends at the next step)
    } else {
        drop(g);
        e.abandon();
    }
    r
}

/// The EDL job `jid` ended with `r` (its sound closed): an Encode job's file is muxed and written here. Returns `r`, or
/// `export-refused` when the job ran but the file could not be made. A Present job passes `r` through.
pub fn finish(jid: u32, r: Result<(), (&'static str, Option<String>)>) -> Result<(), (&'static str, Option<String>)> {
    let Some(mut e) = ENC.lock().take_if(|e| e.jid == jid) else { return r };
    let pcm = bus::take(jid);
    let res = match &r {
        Ok(()) => e.write(pcm),
        Err((why, m)) => {
            e.abandon();
            Err((*why, m.clone().unwrap_or_else(|| String::from("superseded"))))
        }
    };
    match res {
        Ok(rep) => {
            serial_println!(
                "[nle] export frames={} bytes={} ms={} audio={}/{} bframes={} out={}",
                rep.frames, rep.bytes, rep.ms, rep.audio_mixed, rep.audio_target, rep.bframes, rep.out
            );
            r
        }
        Err((why, msg)) => {
            serial_println!("[nle] export refused why={} ({})", why, msg);
            *LAST.lock() = Some((jid, Report { err: Some(alloc::format!("{}: {}", why, msg)), ..Report::default() }));
            match r {
                Ok(()) => Err(("export-refused", Some(alloc::format!("export: {}", msg)))),
                Err(x) => Err(x),
            }
        }
    }
}

/// The finished export `jid`'s report (None while it runs).
pub fn report(jid: u32) -> Option<Report> {
    LAST.lock().as_ref().filter(|(j, _)| *j == jid).map(|(_, r)| r.clone())
}

impl Exp {
    /// Flush the encoder's tail into the stream, push the capture's PCM16 track, write the moov; the report is kept in
    /// [`LAST`]. Any refusal unlinks the partial file.
    fn write(mut self, pcm: Option<(Vec<i16>, u32)>) -> Result<Report, (&'static str, String)> {
        let r = self.write_inner(pcm);
        if r.is_err() {
            self.abandon();
        }
        r
    }

    fn write_inner(&mut self, pcm: Option<(Vec<i16>, u32)>) -> Result<Report, (&'static str, String)> {
        let tail = self.rgba.flush().map_err(|e| ("encode-refused", alloc::format!("{}", e)))?;
        self.stream_units(tail).map_err(|e| ("write-refused", e))?;
        if self.stream.is_none() {
            return Err(("no-frames", String::from("the EDL encoded no frame")));
        }
        let mut tracks: Vec<build::MediaTrack> = Vec::new();
        let mut rep = Report { frames: self.frames, bframes: self.opts.bframes, out: self.out.clone(), ..Report::default() };
        if let Some((mut pcm, rate)) = pcm.filter(|(p, _)| !p.is_empty()) {
            let target = (self.tb.to_ns(self.total as i64).max(0) as u128 * rate as u128 / 1_000_000_000) as u64;
            rep.audio_mixed = (pcm.len() / 2) as u64;
            rep.audio_target = target;
            rep.audio = true;
            pcm.resize(target as usize * 2, 0);
            tracks.push(build::pcm16_track(2, rate, 2, &pcm, 1024));
        }
        let Some(mut st) = self.stream.take() else { return Err(("no-frames", String::from("the EDL encoded no frame"))) };
        for t in tracks {
            let i = st.track(t.spec);
            for smp in t.samples {
                st.push(i, smp).map_err(|e| ("write-refused", e))?;
            }
        }
        rep.video_bytes = st.video_bytes();
        let (_, len) = st.finish(&build::Mp4Options::default()).map_err(|e| ("write-refused", e))?;
        rep.bytes = len;
        rep.ms = crate::arch::ms().saturating_sub(self.t0);
        *LAST.lock() = Some((self.jid, rep.clone()));
        Ok(rep)
    }

    /// Units out of the encoder into the file: the file is created and the `avc1` track declared at the first one (the
    /// encoder, hence its avcC and size, exists by then).
    fn stream_units(&mut self, units: Vec<h264enc_core::rgba::Unit>) -> Result<(), String> {
        for u in units {
            if self.stream.is_none() {
                crate::shell::vfs_mount_table().put_file(&self.out, &[], crate::fs::vfs::KERNEL_PRINCIPAL).map_err(|e| alloc::format!("create {}: {:?}", self.out, e))?;
                self.begun = true;
                let mut st = build::Mp4Stream::begin(VfsOut { path: self.out.clone() })?;
                self.vtrack = st.track(build::coded_video_spec(1, *b"avc1", Some(*b"avcC"), self.rgba.avcc(), self.size.0 as u16, self.size.1 as u16, self.tb));
                self.stream = Some(st);
            }
            if let Some(st) = self.stream.as_mut() {
                st.push(self.vtrack, build::coded_video_sample(self.tb, u.keyframe, u.sample, u.dts, u.pts))?;
            }
        }
        Ok(())
    }

    /// Drop the stream and unlink the partial file (ftyp + mdat with no moov), if this export created one.
    fn abandon(&mut self) {
        self.stream = None;
        if core::mem::take(&mut self.begun) {
            let _ = crate::shell::vfs_mount_table().unlink(&self.out, crate::fs::vfs::KERNEL_PRINCIPAL);
        }
    }
}

/// `body` as the whole of `path` (replaced if it exists), through the ONE VFS writer as the shell writes (the kernel
/// principal). Kept by name for the editor's EDL save (NLE6KEYS).
pub(crate) fn write_file(path: &str, body: &[u8]) -> Result<u64, String> {
    crate::shell::vfs_mount_table().put_file(path, body, crate::fs::vfs::KERNEL_PRINCIPAL).map(|n| n as u64).map_err(|e| alloc::format!("write {}: {:?}", path, e))
}

/// `nle export <edl> <out.mp4> [--qp N] [--gop N] [--bframes N] [--bitrate KBPS]`: arm and start; the job prints the progress and the end.
pub fn shell_verb(args: &[&str], console: &mut crate::console::Console) {
    const USAGE: &str = "usage: nle export <edl> <out.mp4> [--qp N] [--gop N] [--bframes N] [--bitrate KBPS]";
    if args.first().copied() != Some("export") || args.len() < 3 {
        console.println(USAGE);
        return;
    }
    let mut o = Opts::default();
    let mut i = 3;
    while i < args.len() {
        let v = args.get(i + 1).and_then(|s| s.parse::<u32>().ok());
        match (args[i], v) {
            ("--qp", Some(n)) => o.qp = n.min(255) as u8,
            ("--gop", Some(n)) => o.gop = n,
            ("--bframes", Some(n)) => o.bframes = n.min(255) as u8,
            ("--bitrate", Some(n)) => o.bitrate = n,
            _ => {
                console.println(USAGE);
                return;
            }
        }
        i += 2;
    }
    let (edl, out) = (crate::shell::vfs_path(args[1]), crate::shell::vfs_path(args[2]));
    match start(&edl, &out, o) {
        Ok(jid) => console.println(&alloc::format!("nle export: started jid={} -> {} ([nle] export progress on the serial log)", jid, out)),
        Err(e) => {
            serial_println!("[nle] export refused why=start ({})", e);
            console.println(&alloc::format!("nle export: refused ({})", e));
        }
    }
}

// ── the sound's door: the live mixer, or the export's offline Mix ────────────────────────────────────────────────────────
// vplay's `snd` names exactly these eight (`open_fmt`, `write`, `end`, `state`, `close`, `set_gain_ramp`, `align`, `RATE`).
// With no capture armed each forwards 1:1 to `audio::mixer` (the CAP guard dropped first). Armed, each runs on the
// capture's `Mix` (`Mix::open` / `write` / `end` / `set_gain_ramp` / `align` / `remove`, `Mix::account_of` with every mixed
// frame played) and `write` / `end` / `state` then pump it as the live `write` / `state` pump the live mixer: `period`
// while `ready` (every live source holds a period; once all have ended, the last partial period), at unity master,
// unmuted (the export is not the speaker). Lock: CAP, under snd's SND; it takes nothing.
#[cfg(any(feature = "uvc", feature = "hda-tone"))]
pub mod bus {
    use crate::audio::mixer::{self, Mix, Played};
    use alloc::vec::Vec;

    pub use crate::audio::mixer::RATE;

    /// The armed capture: the job it belongs to, the offline mixer, the mixed 48 kHz stereo PCM16.
    struct Cap {
        jid: u32,
        mix: Mix,
        pcm: Vec<i16>,
    }

    static CAP: crate::sync::Mutex<Option<Cap>> = crate::sync::Mutex::new(None);

    impl Cap {
        /// The infinitely fast sink: every period the sources can give, appended to the capture.
        fn pump(&mut self) {
            let mut buf = [0i16; mixer::PERIOD_FRAMES * 2];
            while self.mix.ready() {
                self.mix.period(&mut buf, mixer::UNITY, false);
                self.pcm.extend_from_slice(&buf);
            }
        }
    }

    pub(super) fn arm(jid: u32) {
        *CAP.lock() = Some(Cap { jid, mix: Mix::new(), pcm: Vec::new() });
    }

    /// Disarm job `jid`'s capture: its PCM (interleaved stereo) and rate.
    pub(super) fn take(jid: u32) -> Option<(Vec<i16>, u32)> {
        CAP.lock().take_if(|c| c.jid == jid).map(|c| (c.pcm, RATE))
    }

    pub fn open_fmt(name: &'static str, rate: u32, ch: u8, bits: u8, gain: u16) -> Result<u32, &'static str> {
        if let Some(c) = CAP.lock().as_mut() {
            return c.mix.open(name, rate, ch, bits, gain, None).map_err(|(_, why)| why);
        }
        mixer::open_fmt(name, rate, ch, bits, gain)
    }

    pub fn write(id: u32, data: &[u8]) -> usize {
        if let Some(c) = CAP.lock().as_mut() {
            let n = c.mix.write(id, data).unwrap_or(0);
            c.pump();
            return n;
        }
        mixer::write(id, data)
    }

    pub fn end(id: u32) {
        if let Some(c) = CAP.lock().as_mut() {
            c.mix.end(id);
            c.pump();
            return;
        }
        mixer::end(id)
    }

    pub fn state(id: u32) -> Option<Played> {
        if let Some(c) = CAP.lock().as_mut() {
            c.pump();
            return c.mix.account_of(id);
        }
        mixer::state(id)
    }

    pub fn close(id: u32) -> Option<Played> {
        if let Some(c) = CAP.lock().as_mut() {
            let p = c.mix.account_of(id);
            let _ = c.mix.remove(id);
            return p;
        }
        mixer::close(id)
    }

    pub fn set_gain_ramp(id: u32, from: u16, to: u16, frames: u32) -> bool {
        if let Some(c) = CAP.lock().as_mut() {
            return c.mix.set_gain_ramp(id, from, to, frames);
        }
        mixer::set_gain_ramp(id, from, to, frames)
    }

    pub fn align(id: u32, to: u32) -> bool {
        if let Some(c) = CAP.lock().as_mut() {
            return c.mix.align(id, to);
        }
        mixer::align(id, to)
    }
}

/// The image without an audio module: no capture (vplay's silent `snd` twin names no mixer); the export is video alone.
#[cfg(not(any(feature = "uvc", feature = "hda-tone")))]
mod bus {
    pub(super) fn arm(_: u32) {}
    pub(super) fn take(_: u32) -> Option<(alloc::vec::Vec<i16>, u32)> {
        None
    }
}

// ── NLE6EXPORTONE (rmbp-ledger B963): the stream's sink, the export's file through the ONE VFS writer ──────────────────────
/// `build::Mp4Stream`'s sink: bytes at an offset of the export's file (`MountTable::write_all_at`, the kernel principal).
struct VfsOut {
    path: String,
}

impl build::Sink for VfsOut {
    type Error = String;
    fn write_at(&mut self, off: u64, bytes: &[u8]) -> Result<(), String> {
        crate::shell::vfs_mount_table()
            .write_all_at(&self.path, off, bytes, crate::fs::vfs::KERNEL_PRINCIPAL)
            .map(|_| ())
            .map_err(|e| alloc::format!("write {} at {}: {:?}", self.path, off, e))
    }
}
