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
//!   when sound was captured, a `sowt` PCM16 track (`build::pcm16_track`) cut to the video's length — or, under `--aac KBPS`
//!   ([`Opts::aac_kbps`], NLE6EXPORTAAC rmbp-ledger B960), the same cut PCM through audio_core's `aac::enc::Encoder` (Stria's
//!   host export's one encoder) as an `mp4a` track (`build::mp4a_track`) pushed through the same stream, its priming
//!   trimmed by `build::mp4a_edit_list` in the extras `Mp4Stream::finish` takes ([`audio_track`]) — then the moov. Written as the shell writes (`write_veto` at start, the kernel principal) through
//!   the ONE VFS writer, `MountTable::put_file` / `write_all_at`; a refused, failed or superseded export unlinks its
//!   partial file. `--bitrate KBPS` is h264enc_core's `Config.rate` (CBR), the field Stria's `with_bitrate` sets.
//! * THE PICTURE (H264ENCPLANES, rmbp-ledger B973): the file has ONE colour, fixed at its first frame ([`Exp::push`]): an
//!   unmixed 8-bit 4:2:0 decoder picture enters h264enc_core's planar entry (`RgbaEncoder::push_planes`) with no RGBA round
//!   trip when its resolved colour is the file's; any other frame (mixed, black, 10-bit, 4:4:4, another colour) converts its
//!   RGBA through `yuv_core::from_rgba` at the file's matrix. An RGBA-first file is BT.601 limited (the samples it always had);
//!   every file states its colour as `colr` (`build::Mp4Extras::colour`). Wire `[nle] export colour matrix= range= src= planes=`.
//! * LOCKS: [`ENC`] (the armed export) is a leaf; [`frame`] TAKES the export out of it and encodes with no lock held (a
//!   1080p picture is far past the lock registry's 50 ms), putting it back unless superseded. `bus::CAP` (the capture) is
//!   taken under vplay's `snd::SND` and takes nothing (the live forward drops it first): the one new edge is SND -> CAP.
//! * The export takes vplay's ONE job: refused while the Player (the job's one presenter) is open (`player::is_open`;
//!   `vplay::finished` cannot say it: a superseded job ends with neither eos nor an error).
//!
//! Wire: `[nle] export start edl= out= qp= gop= bframes= bitrate= jid=` · `[nle] export progress pct=<10..100> frames=<n>/<total>` ·
//! `[nle] export frames=<n> bytes=<n> ms=<n> audio=<mixed>/<target> bframes=<n> out=<p>` · `[nle] export refused why=<w> (<reason>)`
//! · NLE6EXPORTAAC: `[nle] export audio=aac bytes=<n> units=<n> kbps=<k>` (before the `frames=` line).
//! The witness is player.rs `tests nle` (`export=` on `:: NLE: … ::`; the AAC leg's `[nle] fixture export audio=aac`).
//! Owed: the Pi budget, a progress UI on the kernel face, MKV, the sound streamed too (NLE6EXPORT-STREAMPCM: the
//! capture's PCM is held whole until [`finish`], and the AAC encode runs on it there).

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
    /// NLE6EXPORTAAC (rmbp-ledger B960): the sound as AAC-LC at this many kb/s; 0 = PCM16 `sowt` (the default).
    pub aac_kbps: u32,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { qp: 26, gop: 0, bframes: 0, bitrate: 0, aac_kbps: 0 }
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
    /// H264ENCPLANES (B973): the file's one colour and the frames that entered as planes (file tail).
    col: ExpColour,
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
        *g = Some(Exp { jid: 0, out: String::from(out), opts, rgba: RgbaEncoder::new(cfg), stream: None, vtrack: 0, begun: false, size: (0, 0), tb: Timebase { num: 1, den: 25 }, total: 1, frames: 0, next_pct: 10, t0, col: ExpColour::default() });
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

/// The step's frame into the encoder: its decoder planes with the source's colour when unmixed 8-bit 4:2:0 (H264ENCPLANES
/// B973), and its `w x h` straight RGBA made on demand ([`Exp::push`] decides); `tb` the EDL's time base and `total` its video
/// steps (the progress). Encodes with no lock held.
pub fn frame(jid: u32, tb: Timebase, total: u64, w: usize, h: usize, planes: Option<(h264enc_core::Yuv420<'_>, Option<Colour>)>, rgba: impl FnOnce() -> Vec<u8>) -> Result<(), String> {
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
    let r = match e.push(w, h, planes, rgba) {
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
        let mut extras = build::Mp4Extras::default(); // NLE6EXPORTAAC (rmbp-ledger B960): the AAC track's priming edit; empty for PCM16
        extras.colour.extend(self.colour_entry()); // H264ENCPLANES (rmbp-ledger B973): the file's one colour as the avc1's `colr`, and its wire line (file tail)
        if let Some((mut pcm, rate)) = pcm.filter(|(p, _)| !p.is_empty()) {
            let target = (self.tb.to_ns(self.total as i64).max(0) as u128 * rate as u128 / 1_000_000_000) as u64;
            rep.audio_mixed = (pcm.len() / 2) as u64;
            rep.audio_target = target;
            rep.audio = true;
            pcm.resize(target as usize * 2, 0);
            let (track, edit) = audio_track(&pcm, rate, self.opts.aac_kbps)?;
            tracks.push(track);
            extras.edit_lists.extend(edit);
        }
        let Some(mut st) = self.stream.take() else { return Err(("no-frames", String::from("the EDL encoded no frame"))) };
        for t in tracks {
            let i = st.track(t.spec);
            for smp in t.samples {
                st.push(i, smp).map_err(|e| ("write-refused", e))?;
            }
        }
        rep.video_bytes = st.video_bytes();
        let (_, len) = st.finish(&build::Mp4Options::default(), &extras).map_err(|e| ("write-refused", e))?;
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

/// `nle export <edl> <out.mp4> [--qp N] [--gop N] [--bframes N] [--bitrate KBPS] [--aac KBPS]`: arm and start; the job prints the progress and the end.
pub fn shell_verb(args: &[&str], console: &mut crate::console::Console) {
    const USAGE: &str = "usage: nle export <edl> <out.mp4> [--qp N] [--gop N] [--bframes N] [--bitrate KBPS] [--aac KBPS]";
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
            ("--aac", Some(n)) => o.aac_kbps = n,
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

/// NLE6EXPORTAAC (rmbp-ledger B960): the captured sound's track (track 2, stereo at `rate`, already cut to the video): PCM16
/// `sowt` when `aac_kbps` is 0, else AAC-LC through audio_core's `aac::enc::Encoder` (the encoder Stria's host export
/// takes) as `build::mp4a_track` with its priming edit (`build::mp4a_edit_list`, for `Mp4Stream::finish`'s extras). Runs
/// in [`Exp::write`], no lock held.
fn audio_track(pcm: &[i16], rate: u32, aac_kbps: u32) -> Result<(build::MediaTrack, Option<(u32, Vec<(u32, i64, i32)>)>), (&'static str, String)> {
    if aac_kbps == 0 {
        return Ok((build::pcm16_track(2, rate, 2, pcm, 1024), None));
    }
    use audio_core::aac::enc::{Encoder, PRIMING};
    let bps = aac_kbps.saturating_mul(1000);
    let mut enc = Encoder::new(rate, 2, bps).map_err(|e| ("aac-refused", alloc::format!("{:?}", e)))?;
    let mut units = enc.encode(pcm);
    units.extend(enc.flush());
    let (n, bytes) = (units.len(), units.iter().map(|u| u.data.len()).sum::<usize>());
    serial_println!("[nle] export audio=aac bytes={} units={} kbps={}", bytes, n, aac_kbps);
    let track = build::mp4a_track(2, rate, 2, &enc.asc(), units.into_iter().map(|u| u.data).collect(), bps);
    Ok((track, Some(build::mp4a_edit_list(2, rate, PRIMING, (pcm.len() / 2) as u64))))
}

// ── H264ENCPLANES (rmbp-ledger B973): the export's picture path and its ONE colour (handlers.md §5 NLEPROBE-HDR's H264ENC-PLANES) ──
// vplay's `export_frame` hands an unmixed 8-bit 4:2:0 decoder picture (VP8, 8-bit AV1) as planes with the frame's colour; a mixed,
// black, stand-in, 10-bit, 4:4:4 or mono frame as None (its RGBA made on demand by `Px::rgb`). The FILE's colour is fixed at the
// first frame: that frame's resolved source colour when it came as planes, else BT.601 limited (the matrix the RGBA path always
// encoded with: its samples unchanged, now stated). Every frame enters in the file's colour: planes whose (Kr / Kb, range) match
// go straight to `RgbaEncoder::push_planes` (no conversion); everything else through `push_rgba_in` = `yuv_core::from_rgba` at
// the file's matrix and range. `write_inner` states it as `colr` through `build::Mp4Extras::colour` (the existing writer). The
// state lives in `Exp`, which `frame` holds outside `ENC`: no lock of its own.

use demux_core::colour::Colour;

/// The export file's one colour and how many frames entered as planes.
#[derive(Default)]
struct ExpColour {
    /// (the colour the `colr` states, its Kr / Kb x 10000, full range), fixed at the first frame.
    file: Option<(Colour, (i64, i64), bool)>,
    /// What fixed it: "planes" or "rgba".
    src: &'static str,
    planes: u64,
}

/// The RGBA path's colour: BT.601 limited, primaries / transfer unspecified (H264ENC-COLOUR: the untagged file read as BT.709 at HD).
const RGBA_COLOUR: (Colour, (i64, i64), bool) = (Colour { primaries: 2, transfer: 2, matrix: 6, full_range: Some(false), mastering: None, light: None }, yuv_core::KR_KB_601, false);

/// A planes frame's colour as the Player converts it (`yuv_core::pick` at its size): the code points the `colr` states (the
/// matrix explicit: the source's stated Kr / Kb code, else the size default's 1 / 6; the range explicit; the rest the source's,
/// 2 when it states none), its Kr / Kb and range. None for a non-Kr pick (identity, YCgCo): that frame takes the RGBA path.
fn file_colour(c: Option<Colour>, w: usize, h: usize) -> Option<(Colour, (i64, i64), bool)> {
    let (m, full) = yuv_core::pick(c.map(|c| c.matrix), c.and_then(|c| c.full_range), w as u32, h as u32);
    let yuv_core::Matrix::Kr(k) = m else { return None };
    let code = match c.map(|c| c.matrix) {
        Some(mc) if yuv_core::kr_kb(mc).is_some() => mc,
        _ if k == yuv_core::KR_KB_709 => 1,
        _ => 6,
    };
    let base = c.unwrap_or(Colour { primaries: 2, transfer: 2, matrix: 2, full_range: None, mastering: None, light: None });
    Some((Colour { matrix: code, full_range: Some(full), ..base }, k, full))
}

impl Exp {
    /// One frame into the encoder in the FILE's colour (fixed by the first frame): matching planes straight to the planar entry,
    /// anything else its RGBA converted at the file's matrix and range.
    fn push(&mut self, w: usize, h: usize, planes: Option<(h264enc_core::Yuv420<'_>, Option<Colour>)>, rgba: impl FnOnce() -> Vec<u8>) -> h264enc_core::Result<Vec<h264enc_core::rgba::Unit>> {
        let src = planes.and_then(|(p, c)| file_colour(c, w, h).map(|fc| (p, fc)));
        if self.col.file.is_none() {
            self.col.src = if src.is_some() { "planes" } else { "rgba" };
        }
        let file = *self.col.file.get_or_insert(match &src {
            Some((_, fc)) => *fc,
            None => RGBA_COLOUR,
        }); if !self.rgba.started() { self.rgba.cfg.colour = Some((file.0.primaries, file.0.transfer, file.0.matrix, file.2)); } // H264ENCCOLOURHOST (LEDGER SR377): the SAME file colour as the SPS VUI (h264enc_core `Config::colour`), set before the encoder is built
        match src {
            Some((p, (_, k, full))) if (k, full) == (file.1, file.2) => {
                self.col.planes += 1;
                self.rgba.push_planes(&p)
            }
            _ => self.rgba.push_rgba_in(w as u32, h as u32, &rgba(), file.1, file.2),
        }
    }

    /// The file's `colr` entry for `build::Mp4Extras::colour` (the avc1 track is id 1, [`Exp::stream_units`]), and the wire line.
    fn colour_entry(&self) -> Option<(u32, Colour)> {
        let (c, _, full) = self.col.file?;
        serial_println!(
            "[nle] export colour matrix={} range={} src={} planes={}/{}",
            c.matrix, if full { "full" } else { "limited" }, self.col.src, self.col.planes, self.frames
        );
        Some((1, c))
    }
}
