// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Stria's editor render (NLE1 M3, LEDGER SR194; CODEX Amendment II: Stria owns A/V).
//!
//! CHARTER: Stria — the render half of the non-linear editor, over `timeline_core` (shared-core, BP-handlers-4).
//!
//! [`render`] turns a [`Timeline`] range into raw output — RGBA frames and interleaved `f32` PCM — handed to a
//! [`RenderSink`] in output order. It is an offline pull with no clock: it walks `timeline_core::schedule::schedule`
//! and drives the SAME dsp registry the realtime [`crate::media::Player`] plays through (`dsp::video::decoder_for`,
//! `dsp::audio_track::audio_decoder_for` + `Timing::of`); it holds no decoder, demuxer or sample conversion of its own.
//!
//! * A step with `seek` opens a fresh cursor on the source ([`Demuxer::share`], no byte copy), lands it on the keyframe
//!   ([`Demuxer::seek_track`]) and decodes forward, discarding frames before the step's source time unseen — the run-up,
//!   the shape vplay and the Player seek with. A continuing step decodes on.
//! * [`Sources`] is the schedule's [`SourceIndex`]: its keyframe answer is NLEPROBE's pure [`Demuxer::keyframe_before`]
//!   (the search `seek_track` shares; DEMUXWIRE, LEDGER SR213). A source opens by its own index when it has one
//!   ([`Demuxer::open_indexed`]) and a Matroska video track gets decode times from its codec's reorder depth
//!   ([`Demuxer::derive_dts`] through `dsp::video::reorder_depth`); [`Sources::open_report`] says how each reel opened.
//! * Source time 0 is a stream's first presentation time (vplay's rule).
//! * A media file is its own timeline (EDITMAP, LEDGER SR223): [`Sources::from_media`] lays each stream's whole edit
//!   list on its track through `timeline_core`'s `Timeline::lay_edit_list` (each segment a clip, an empty edit a gap),
//!   so an edit-listed MP4 renders as its author cut it; [`OpenReport`] says `applied=<n>` (else `first-only`).
//! * Transitions (NLE3, LEDGER SR202): a step carrying `vmix` decodes a second run (the outgoing source, which takes
//!   over the live run it continues) through the same open-or-continue path ([`vid_frame`]) and mixes the two: a
//!   dissolve is pixel_core's `blend_nonpremult` of the incoming at alpha `round(255 t)` over the outgoing, a wipe a
//!   column (`W001`) or row (`W002`) split at `round(w t)` / `round(h t)` with the incoming before the edge. NLE3c
//!   (SR209): `W041` a diagonal from the top-left (`x + y < t (w + h)`), `W101` a centred box growing with `t`, `W201`
//!   a clock hand from twelve sweeping `2 pi t` (exact fractions, the clock by the pixel centre's angle); a transition
//!   from black mixes the incoming over opaque black (the gap's black) the same way.
//! * Video is cut-only and one size: a frame whose size differs from the project's (the first scheduled video source's
//!   track) is refused (scaling is owed). A gap is black (opaque) or silence.
//! * Audio is a MIX (NLE4, LEDGER SR203): each audio step renders its `timeline_core::schedule::audio_runs` run: every
//!   layer decoded on its own track's decoder, resampled to the project rate when its rate differs
//!   (`audio_core::resample::Sinc`, NLE4B's windowed sinc; the project rate is [`Sources::set_audio_rate`]'s, else
//!   the first scheduled audio source's), scaled by its gain and its linear fade envelope, summed in integers and
//!   clamped to 16 bits once. Only a CHANNEL-count mismatch is refused. One layer at 0 dB with no fade passes through
//!   bit for bit. NLE3b (SR209): a
//!   crossfade's UNDER layer (the outgoing's handle, before the incoming layer of the same track) decodes on its own
//!   slot (`tracks + track`), so the track's own decoder carries the incoming on; the sum is the same `mix_into`.
//!
//! [`synth_source`] writes the codec-free test medium the KATs and `tools/nle --make-src` use: demux_core's `utp1`
//! test-pattern video (frame ordinals from `base`) plus 16-bit PCM whose every sample names its source and index.

use std::collections::VecDeque;
use std::sync::Arc;

use gneiss_pal::dsp::audio::meter::Meter;
use gneiss_pal::dsp::audio::resample;
use gneiss_pal::dsp::audio_track::{AudioTrackDecoder, Timing};
use gneiss_pal::dsp::demux::{DecodeTime, Demuxer, TrackKind, build};
use gneiss_pal::dsp::video::{Frame, VideoDecoder, decoder_for, reorder_depth, colour_of};
use timeline_core::schedule::{AudioRun, Layer, ScheduleError, audio_runs, schedule};
use timeline_core::{Kind, SizePolicy, SourceId, SourceIndex, Timebase, Timeline};
use crate::decode_swap::{DecodeSwap, Swap};
use gneiss_pal::dsp::demux::Track;

/// Where rendered output goes, in output order.
pub trait RenderSink {
    /// One output frame at output tick `out_t`: `width * height * 4` RGBA bytes.
    fn frame(&mut self, out_t: i64, width: u32, height: u32, rgba: &[u8]);
    /// The PCM of output ticks `out_t..` (one audio step): interleaved `f32`.
    fn pcm(&mut self, out_t: i64, rate: u32, channels: u16, interleaved: &[f32]);
    /// NLE4C (SR301): true when the sink also wants each audio track's own sum ([`RenderSink::track_pcm`]).
    fn taps_tracks(&self) -> bool {
        false
    }
    /// NLE5DEXPORTSTOP (SR705): true when the sink wants no more output: [`render`] reads it once per step, before the
    /// step's decode, and ends there ([`RenderReport::stopped`]). A cancelled export ends its decode, not only its encode.
    fn stop(&self) -> bool {
        false
    }
    /// STRIALOUDNESSMORE (SR961): false when the sink does not want the video frame at output tick `out_t`: [`render`] asks it once per video step, after [`RenderSink::stop`], and then skips that step's decode (no `frame` call, nothing added to [`RenderReport::frames`]). STRIALOUDNESSNEXT (SR995): [`render`] also asks it for the FIRST video step when no reel sizes the frame, and refuses [`RenderError::NoSize`] only on a true (a sink declining every frame renders its audio under an empty video track).
    fn frame_wanted(&mut self, _out_t: i64) -> bool { true }
    /// NLE4C (SR301): audio track `track`'s share of the [`RenderSink::pcm`] just sent (its layers summed, clamped alone).
    fn track_pcm(&mut self, _out_t: i64, _track: usize, _rate: u32, _channels: u16, _interleaved: &[f32]) {}
    fn proxy(&self) -> bool { false } // STRIAPROXYMEDIA (SR882): true = a source's attached proxy decodes its video (the preview's sinks); an export's sink never (file tail)
}

/// What a render did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderReport {
    pub frames: u64,
    pub pcm_frames: u64,
    /// Decoder (re)starts at a keyframe, video and audio.
    pub seeks: u64,
    /// Video frames decoded and not shown (the run-up from a keyframe, or frames a faster source skips).
    pub runup_frames: u64,
    pub width: u32,
    pub height: u32,
    pub rate: u32,
    pub channels: u16,
    /// NLE5DEXPORTSTOP (SR705): the output tick of the step at which the sink's [`RenderSink::stop`] ended the render
    /// (that step not rendered); `None` when the range ran to its end.
    pub stopped: Option<i64>,
    /// NLESTANDINAUDIOEDITOR (SR713): the reels whose audio played media.rs's silent stand-in (`AudioSwap`: no decoder, or parked after a refused packet), each counted once.
    pub stand_in_audio: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RenderError {
    Schedule(ScheduleError),
    /// The timeline cites a source no media was added for.
    NoSource(SourceId),
    /// The source has no stream of the kind its track needs.
    NoStream(SourceId, Kind),
    Demux(String),
    Decode(String),
    /// A frame's size differs from the project's.
    Size { src: SourceId, width: u32, height: u32 },
    /// An audio stream's channel count differs from the project's (NLE4: a rate is resampled).
    Format { src: SourceId, rate: u32, channels: u16 },
    /// The media ran out before the clip did.
    Short(SourceId),
    /// Video steps exist but no scheduled source has a video stream to size the frame.
    NoSize,
    /// EDITMAP (SR223): a media file's edit list could not be laid as a timeline (a rate edit, or a segment past the media).
    Edit(timeline_core::EditError),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Schedule(e) => write!(f, "schedule: {e}"),
            RenderError::NoSource(s) => write!(f, "no media for source {}", s.0),
            RenderError::NoStream(s, k) => write!(f, "source {} has no {k:?} stream", s.0),
            RenderError::Demux(e) => write!(f, "demux: {e}"),
            RenderError::Decode(e) => write!(f, "decode: {e}"),
            RenderError::Size { src, width, height } => write!(f, "source {} frame {width}x{height} is not the project size", src.0),
            RenderError::Format { src, rate, channels } => write!(f, "source {} audio {rate} Hz x{channels} is not the project format", src.0),
            RenderError::Short(s) => write!(f, "source {} ran out before its clip", s.0),
            RenderError::NoSize => f.write_str("no video source to size the frame"),
            RenderError::Edit(e) => write!(f, "edit list: {e}"),
        }
    }
}
impl std::error::Error for RenderError {}

/// One source's media: the demuxer (its bytes shared by every cursor) and each stream's origin.
struct Media {
    demux: Demuxer,
    video: Option<usize>,
    audio: Option<usize>,
    v0: i64,
    a0: i64,
    /// SWAPBACKSPANCACHE (SR370): the reel's in-band parameter sets, walked once at the first run open and shared by every run.
    sets: std::sync::OnceLock<Arc<crate::decode_swap::SetIndex>>,
    /// SWAPBACK-NLECACHE (SR388): the decoder and swap state the reel's last closed run left ([`Vid`]'s drop), taken by the next run open.
    park: ReelPark,
}

impl Media {
    /// DEMUXWIRE (SR213): the editor's open — by the file's own index when it has one (an index open that declares no
    /// duration opens whole: the schedule bounds clips by it), then decode times from the codec's reorder depth; an
    /// indexed reel loads its first span so the origin (and the audio `Timing`) read real samples.
    fn open(bytes: Vec<u8>) -> Result<Media, RenderError> {
        let err = |e: gneiss_pal::dsp::demux::Error| RenderError::Demux(e.to_string());
        let mut demux = if gneiss_pal::dsp::demux::wav::is_wav(&bytes) { Demuxer::open_wav(bytes) } else { Demuxer::open_indexed(bytes) }.map_err(err)?; // STRIAIMPORTWAV (SR889): a RIFF/WAVE reel is demux_core's one-track PCM table (its own door: Demuxer::open stays WAV-blind for the Player)
        if demux.indexed() && demux.duration_ns() == 0 {
            let data = demux.bytes();
            drop(demux);
            demux = Demuxer::open(Arc::try_unwrap(data).unwrap_or_else(|a| (*a).clone())).map_err(err)?;
        }
        demux.derive_dts(reorder_depth); demux.fill_colour(colour_of);
        let find = |k: TrackKind| demux.tracks().iter().position(|t| t.kind == k);
        let (video, audio) = (find(TrackKind::Video), find(TrackKind::Audio));
        if demux.indexed() {
            if let Some(i) = video.or(audio) {
                demux.load_at(i, i64::MIN).map_err(err)?;
            }
        }
        let origin = |i: Option<usize>| {
            i.map(|i| {
                let tb = demux.tracks()[i].timebase;
                demux.track_samples(i).map(|s| tb.to_ns(s.pts)).min().unwrap_or(0)
            })
            .unwrap_or(0)
        };
        let (v0, a0) = (origin(video), origin(audio));
        Ok(Media { demux, video, audio, v0, a0, sets: Default::default(), park: Default::default() })
    }
    fn stream(&self, kind: Kind) -> Option<(usize, i64)> {
        match kind {
            Kind::Video => self.video.map(|i| (i, self.v0)),
            Kind::Audio => self.audio.map(|i| (i, self.a0)),
        }
    }
}

/// The media behind a timeline's sources; the schedule's [`SourceIndex`].
pub struct Sources {
    tb: Timebase,
    media: Vec<(SourceId, Media)>,
    /// NLE4: the project's audio rate (0 = the first scheduled audio source's).
    audio_rate: u32,
    /// EDITMAP (SR223): the sources laid from their own edit list ([`Sources::from_media`]) and how many clips it gave.
    applied: Vec<(SourceId, usize)>,
    /// SWAPBACK (SR329): the sources a render drew stand-in frames for (a mid-reel decoder refusal); locked only for one
    /// push ([`Sources::note_stand_in`]) or one read ([`Sources::open_report`]), never across a decode.
    stand_in: std::sync::Mutex<Vec<SourceId>>, titles: TitleCache, seqs: Vec<(SourceId, Seq)>, seq_doc: Vec<(SourceId, (u64, u64))>, // STRIASEQRATEEDL (SR9053): the document's sequence rates, read at `new` (file tail); STRIATITLECLIP (SR828): the title stills, keyed (text, w, h) (file tail); STRIAIMPORTIMAGESEQUENCE (SR888): the PNG sequence reels (file tail)
    proxy: std::sync::Mutex<Vec<(SourceId, Arc<Media>)>>, // STRIAPROXYMEDIA (SR882): each source's attached preview proxy, locked for one find or one push (file tail)
    peak_paths: Vec<(SourceId, std::path::PathBuf)>, // STRIAWAVEMULTI (SR9071): the media file of each source opened from a path with the peak cache on (file tail)
}

impl Sources {
    pub fn new(tl: &Timeline) -> Sources {
        Sources { tb: tl.tb, media: Vec::new(), audio_rate: 0, applied: Vec::new(), stand_in: Default::default(), titles: Default::default(), seqs: Vec::new(), proxy: Default::default(), seq_doc: seq_doc_rates(tl), peak_paths: Vec::new() } // STRIASEQRATEEDL (SR9053): the EDL's `* UNA SEQRATE` rates (file tail); STRIAIMPORTIMAGESEQUENCE (SR888): no sequence reel yet
    }
    /// NLE4: render audio at `hz` (sources at other rates are resampled); 0 = the first scheduled audio source's rate.
    pub fn set_audio_rate(&mut self, hz: u32) {
        self.audio_rate = hz;
    }
    /// Open `bytes` (MP4 / Matroska / WebM / WAV) as source `id`.
    pub fn add(&mut self, id: SourceId, bytes: Vec<u8>) -> Result<(), RenderError> {
        let m = Media::open(bytes)?; self.note_open(id, &m); // NLE-STANDIN-OPEN (SR353): a reel no decoder fits opens as a stand-in reel
        self.media.retain(|(s, _)| *s != id); self.seqs.retain(|q| q.0 != id); // STRIAIMPORTIMAGESEQUENCE (SR888): a re-added id forgets its sequence
        self.media.push((id, m));
        Ok(())
    }
    fn get(&self, id: SourceId) -> Result<&Media, RenderError> {
        self.media.iter().find(|(s, _)| *s == id).map(|(_, m)| m).ok_or(RenderError::NoSource(id))
    }
    /// DEMUXWIRE (SR213): how source `id` opened — its video stream's (else its audio stream's) survey facts.
    pub fn open_report(&self, id: SourceId) -> Option<OpenReport> {
        if let Some(q) = self.seq(id) { return Some(q.report()); } let m = self.get(id).ok()?; // STRIASEQREPORT (SR943): a sequence reel reports `open=seq` (file tail)
        let i = m.video.or(m.audio)?;
        let s = m.demux.survey().streams.into_iter().nth(i)?;
        let applied = self.applied.iter().find(|a| a.0 == id).map(|a| a.1);
        let stand_in = self.stand_in.lock().map(|v| v.contains(&id)).unwrap_or(false);
        Some(OpenReport { indexed: s.indexed, decode_time: s.decode_time, edits: s.edits.len(), applied, colour: s.colour, stand_in, seq: false }) // STRIASEQREPORT (SR943): a demuxed reel
    }
}

/// DEMUXWIRE (SR213): one reel's open, as `nle schedule` prints it:
/// `open=<full|indexed> dts=<stored|derived:n|presentation> edits=<n> applied=<n|first-only>`. `edits` counts every `elst`
/// entry; EDITMAP (SR223): `applied` is the clips a reel opened by [`Sources::from_media`] laid from its whole edit list,
/// `first-only` a reel an EDL cuts (only the first media edit applies, as playback does). NLEPROBEHDRWIRE (SR305): then
/// ` colour=<token>` (demux_core's `Colour` Display) only when the video track states a colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenReport {
    pub indexed: bool,
    pub decode_time: DecodeTime,
    pub edits: usize,
    /// EDITMAP (SR223): `Some(clips)` when the reel's whole edit list was laid; `None` = the first media edit only.
    pub applied: Option<usize>,
    /// NLEPROBEHDRWIRE (SR305): the reel's colour (container, else the bitstream's through `fill_colour(colour_of)`).
    pub colour: Option<gneiss_pal::dsp::demux::colour::Colour>,
    /// SWAPBACK (SR329): a render drew stand-in frames for this reel (its decoder refused a span mid-reel); NLE-STANDIN-OPEN (SR353): or no decoder fits it (set at open).
    pub stand_in: bool, pub seq: bool, // STRIASEQREPORT (SR943): a PNG sequence reel (SR888): Display writes `open=seq` alone
}

impl std::fmt::Display for OpenReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.seq { return write!(f, "open=seq"); } let open = if self.indexed { "indexed" } else { "full" }; // STRIASEQREPORT (SR943): no dts / edits / applied tokens for a file sequence
        write!(f, "open={open} dts=")?;
        match self.decode_time {
            DecodeTime::Stored => write!(f, "stored")?,
            DecodeTime::Derived(d) => write!(f, "derived:{d}")?,
            DecodeTime::Presentation => write!(f, "presentation")?,
        }
        write!(f, " edits={}", self.edits)?;
        match self.applied {
            Some(n) => write!(f, " applied={n}")?,
            None => write!(f, " applied=first-only")?,
        }
        if let Some(c) = self.colour {
            write!(f, " {c}")?;
        }
        if self.stand_in {
            write!(f, " decode=stand-in")?;
        }
        Ok(())
    }
}

impl SourceIndex for Sources {
    fn duration(&self, src: SourceId) -> Option<i64> {
        if let Some(q) = self.seq(src) { return Some(q.len()); } let m = self.get(src).ok()?; // STRIAIMPORTIMAGESEQUENCE (SR888): a sequence lasts one tick per file number (file tail)
        let ns = [m.video, m.audio].into_iter().flatten().map(|i| m.demux.tracks()[i].duration_ns).max()?;
        // whole ticks only (1 µs of slack for a time base's rounding)
        Some(((ns as i128 + 1_000) * self.tb.den as i128 / (self.tb.num.max(1) as i128 * 1_000_000_000)) as i64)
    }
    fn keyframe_ns(&self, src: SourceId, kind: Kind, t_ns: i64) -> Option<i64> {
        if self.seq(src).is_some() { return (kind == Kind::Video).then_some(t_ns); } let m = self.get(src).ok()?; // STRIAIMPORTIMAGESEQUENCE (SR888): every file of a sequence is its own key (file tail)
        let (i, origin) = m.stream(kind)?;
        // DEMUXWIRE (SR213): NLEPROBE's keyframe-before (the search seek_track shares); an indexed reel's window may not
        // hold t, so the span holding it is read into a share first.
        let t = origin + t_ns;
        let key = if m.demux.indexed() {
            let mut c = m.demux.share();
            c.load_at(i, t).ok()?;
            c.keyframe_before(i, t)
        } else {
            m.demux.keyframe_before(i, t)
        };
        key.map(|(_, p)| p - origin)
    }
}

/// Nanoseconds to a sample index at `rate`, rounded.
fn samples(ns: i64, rate: u32) -> i64 {
    let n = ns as i128 * rate as i128;
    (if n >= 0 { (n + 500_000_000) / 1_000_000_000 } else { (n - 500_000_000) / 1_000_000_000 }) as i64
}

/// A video decode run from one keyframe.
struct Vid {
    src: SourceId,
    origin: i64,
    track_id: u32,
    cursor: Demuxer,
    dec: Box<dyn VideoDecoder>,
    /// SWAPBACK (SR329): the track (the stand-in's size and clock), the run's swap state, and whether it drew the stand-in.
    track: Track,
    swap: DecodeSwap,
    stood_in: bool,
    /// SWAPBACK-NLECACHE (SR388): the reel's slot this run's decoder and swap go back to when it closes.
    park: ReelPark,
    pending: VecDeque<Frame>,
    peeked: Option<Frame>,
    last: Option<Frame>,
    done: bool,
    /// NLESIZESWITCH (SR384): the size of the last frame this run showed (`reel_rgba`'s one line per change).
    size: Option<(u32, u32)>,
    /// STRIASPEEDREVERSE (SR9067): the frames a BACKWARD run decoded up to its target (the last [`BACK_CACHE`]), served in reverse.
    back: VecDeque<Frame>,
}

impl Vid {
    fn open(m: &Media, src: SourceId, key_ns: i64) -> Result<Vid, RenderError> {
        let (i, origin) = m.stream(Kind::Video).ok_or(RenderError::NoStream(src, Kind::Video))?;
        let track = &m.demux.tracks()[i];
        let held = m.park.lock().ok().and_then(|mut p| p.take()); // SWAPBACK-NLECACHE (SR388): the reel's held swap and decoder, reset as a Player seek resets
        let (mut swap, mut dec) = match held {
            Some((swap, mut dec)) => {
                dec.reset();
                (swap, dec)
            }
            None => (DecodeSwap::with_index(m.sets.get_or_init(|| Arc::new(crate::decode_swap::SetIndex::of(track, &m.demux))).clone()), crate::decode_swap::open(track)), // NLE-STANDIN-OPEN (SR353): the Player's decoder-or-stand-in, never a refused render; SWAPBACKSPANCACHE (SR370): the reel's index walked once
        };
        let mut cursor = m.demux.share();
        cursor.seek_track(i, origin + key_ns);
        if let Some(hit) = swap.reseed(&mut dec, track, &cursor, origin + key_ns) { // SWAPBACKSEEK (SR353): the run's keyframe with every set in effect there
            log::info!("[STRIA] :: nle reseed reel={} cache={}", src.0, if hit { "hit" } else { "miss" });
        }
        Ok(Vid { src, origin, track_id: track.id, cursor, track: track.clone(), swap, stood_in: !dec.is_real(), park: m.park.clone(), dec, pending: VecDeque::new(), peeked: None, last: None, done: false, size: None, back: VecDeque::new() })
    }
    fn pull(&mut self) -> Result<Option<Frame>, RenderError> {
        loop {
            if let Some(f) = self.pending.pop_front() {
                return Ok(Some(f));
            }
            if self.done {
                return Ok(None);
            }
            match self.cursor.next_packet() {
                Some(p) if p.track == self.track_id => {
                    let d = self.swap.decode(&mut self.dec, &self.track, &p).map_err(|e| RenderError::Decode(e.to_string()))?;
                    self.stood_in |= d.swap == Some(Swap::ToStandIn);
                    self.pending.extend(d.frames);
                }
                Some(_) => {}
                None => {
                    self.done = true;
                    self.pending.extend(self.dec.flush());
                }
            }
        }
    }
    /// The frame on screen at source time `target` (ns): the first frame within `tol` of it; frames before are
    /// decoded unseen; if the next frame is later (a source slower than the timeline) the last one holds.
    fn show(&mut self, target: i64, tol: i64, runup: &mut u64) -> Result<Frame, RenderError> {
        loop {
            if self.peeked.is_none() {
                self.peeked = self.pull()?;
            }
            let Some(f) = self.peeked.as_ref() else { return Err(RenderError::Short(self.src)) };
            let rel = f.pts_ns - self.origin;
            if rel < target - tol {
                *runup += 1;
                self.last = self.peeked.take();
                continue;
            }
            if rel <= target + tol {
                self.last = self.peeked.take();
                return Ok(self.last.clone().expect("just set"));
            }
            return Ok(self.last.clone().unwrap_or_else(|| f.clone()));
        }
    }
}

/// An audio decode run from one sync point; `buf[0]` is source sample `buf_at`.
struct Aud {
    src: SourceId,
    origin: i64,
    track_id: u32,
    rate: u32,
    ch: usize,
    cursor: Demuxer,
    dec: Box<dyn AudioTrackDecoder>,
    buf: Vec<f32>,
    buf_at: i64,
    started: bool,
    done: bool,
    /// NLE4B: the windowed-sinc converter (its table) for the last output rate asked of `take_at`.
    rs: Option<resample::Sinc>,
}

impl Aud {
    /// NLE4: refuses only a channel-count mismatch; the run keeps the decoder's own rate (`rate`).
    fn open(m: &Media, src: SourceId, key_ns: i64, channels: u16) -> Result<Aud, RenderError> {
        let (i, origin) = m.stream(Kind::Audio).ok_or(RenderError::NoStream(src, Kind::Audio))?;
        let track = &m.demux.tracks()[i];
        let dec = crate::media::AudioSwap::open(track, Timing::of(&m.demux, track), true).map_err(|e| RenderError::Decode(e.to_string()))?; // NLESTANDINAUDIOEDITOR (SR713): the one decoder-or-silent-stand-in (media.rs AudioSwap)
        if dec.channels() != channels {
            return Err(RenderError::Format { src, rate: dec.sample_rate(), channels: dec.channels() });
        }
        let rate = dec.sample_rate();
        let mut cursor = m.demux.share();
        cursor.seek_track(i, origin + key_ns);
        let ch = channels.max(1) as usize;
        Ok(Aud { src, origin, track_id: track.id, rate, ch, cursor, dec, buf: Vec::new(), buf_at: 0, started: false, done: false, rs: None })
    }
    /// Source samples `start..start + n`, interleaved (sample-exact; a hole in the stream is silence).
    fn take(&mut self, start: i64, n: i64) -> Result<Vec<f32>, RenderError> {
        self.fill(start + n, false)?;
        Ok(self.drain(start, n))
    }
    /// Decode until source sample `want` is buffered; at the stream's end `Short`, or (`soft`) whatever there is.
    fn fill(&mut self, want: i64, soft: bool) -> Result<(), RenderError> {
        let ch = self.ch;
        loop {
            let have_end = self.buf_at + (self.buf.len() / ch) as i64;
            if self.started && have_end >= want {
                break;
            }
            if self.done {
                return if soft { Ok(()) } else { Err(RenderError::Short(self.src)) };
            }
            match self.cursor.next_packet() {
                Some(p) if p.track == self.track_id => {
                    let b = self.dec.decode(&p).map_err(|e| RenderError::Decode(e.to_string()))?;
                    if b.samples.is_empty() {
                        continue;
                    }
                    let at = samples(b.pts_ns - self.origin, self.rate);
                    if !self.started {
                        self.started = true;
                        self.buf.clear();
                        self.buf_at = at;
                    }
                    let end = self.buf_at + (self.buf.len() / ch) as i64;
                    if at > end {
                        self.buf.resize(self.buf.len() + (at - end) as usize * ch, 0.0);
                    }
                    let skip = if at < end { (end - at) as usize * ch } else { 0 };
                    if skip < b.samples.len() {
                        self.buf.extend_from_slice(&b.samples[skip..]);
                    }
                }
                Some(_) => {}
                None => self.done = true,
            }
        }
        Ok(())
    }
    /// Move the buffer's head to source sample `start` (a hole before the buffer is silence).
    fn seat(&mut self, start: i64) {
        let ch = self.ch;
        if start >= self.buf_at {
            let drop = ((start - self.buf_at) as usize * ch).min(self.buf.len());
            self.buf.drain(..drop);
        } else {
            let pad = (self.buf_at - start) as usize * ch;
            self.buf.splice(0..0, std::iter::repeat_n(0.0, pad));
        }
        self.buf_at = start;
    }
    /// The buffered source samples `start..start + n`, drained.
    fn drain(&mut self, start: i64, n: i64) -> Vec<f32> {
        let ch = self.ch;
        self.seat(start);
        let take = (n.max(0) as usize * ch).min(self.buf.len());
        let out: Vec<f32> = self.buf.drain(..take).collect();
        self.buf_at += n.max(0);
        out
    }
}

/// Render output ticks `range` of `tl` (see the module docs).
pub fn render(tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources, sink: &mut dyn RenderSink) -> Result<RenderReport, RenderError> {
    let steps = schedule(tl, range.clone(), sources).map_err(RenderError::Schedule)?;
    let runs = audio_runs(tl, range, sources).map_err(RenderError::Schedule)?;
    let mut r = RenderReport::default();
    let mut afmt: Option<(u32, u16)> = None;
    for s in &steps {
        let Some(src) = s.src.or(s.vmix.and_then(|m| m.src)) else { continue }; // NLE3cBLACK: a to-black step sizes by its outgoing
        if titled(tl, src) || seq_size(sources, src, s.kind, &mut r) { continue; } let m = sources.get(src)?; // STRIATITLECLIP (SR828): a title sizes no frame (file tail); STRIAIMPORTIMAGESEQUENCE (SR888): a sequence sizes by its first file's header (file tail)
        match s.kind {
            Kind::Video if r.width == 0 => {
                if let Some(i) = m.video {
                    let t = &m.demux.tracks()[i];
                    (r.width, r.height) = (t.width, t.height);
                }
            }
            Kind::Audio if afmt.is_none() => {
                if let Some(i) = m.audio {
                    let t = &m.demux.tracks()[i];
                    afmt = Some((t.sample_rate, t.channels.max(1)));
                }
            }
            _ => {}
        }
    }
    title_size(tl, sources, &steps, &mut r); if r.width == 0 && steps.iter().find(|s| s.kind == Kind::Video).is_some_and(|s| sink.frame_wanted(s.out_t)) { // STRIATITLECLIP (SR828): a title-only range's size (file tail) // STRIALOUDNESSNEXT (SR995): only a sink that wants the first video frame is refused (file tail)
        return Err(RenderError::NoSize);
    }
    if sources.audio_rate > 0 {
        afmt = afmt.map(|(_, ch)| (sources.audio_rate, ch));
    }
    if let Some((rate, ch)) = afmt {
        (r.rate, r.channels) = (rate, ch);
    }
    let (w, h) = (r.width, r.height);
    let black: Vec<u8> = [0u8, 0, 0, 255].repeat(w as usize * h as usize);
    let tol = tl.tb.to_ns(1) / 2; let proxy = sink.proxy(); // STRIAPROXYMEDIA (SR882): the sink says whether an attached proxy decodes (file tail)
    let mut vmix_run: Option<Vid> = None; let mut stack_runs: [Option<Vid>; timeline_core::schedule::STACK_UNDER] = Default::default(); // STRIASTACK (SR9066): each stack layer's own run. NLE3: a transition's outgoing run
    let mut vid: Option<Vid> = None;
    // NLE4: one audio decoder per track; the k-th audio step is the k-th mix run.
    let mut auds: Vec<Option<Aud>> = (0..2 * tl.tracks.len()).map(|_| None).collect(); // NLE3b: + the under slots
    let mut run_of = runs.iter(); let mut stand_in_audio: Vec<SourceId> = Vec::new(); // NLESTANDINAUDIOEDITOR (SR713): the reels whose audio drew the silent stand-in
    for s in &steps {
        if sink.stop() { r.stopped = Some(s.out_t); break; } if s.kind == Kind::Video && !sink.frame_wanted(s.out_t) { continue; } if s.kind == Kind::Video && w == 0 { return Err(RenderError::NoSize); } // STRIALOUDNESSNEXT (SR995): a wanted frame with nothing sized is refused at its step; STRIALOUDNESSMORE (SR961): a sink that declines the frame costs no decode; NLE5DEXPORTSTOP (SR705): the sink said stop: no decode past this step
        match (s.kind, s.src) {
            (Kind::Video, None) => {
                vmix_take(s, &mut vid, &mut vmix_run); timeline_core::vmix::take_stack(s, &mut stack_runs); // STRIASTACK (SR9066). NLE3cBLACK: a to-black tick takes the live outgoing run; a gap drops it
                match s.vmix {
                    Some(_) => sink.frame(s.out_t, w, h, &vmix_frame(tl, sources, s, &mut vmix_run, &mut stack_runs, &black, w, h, tol, &mut r, proxy)?),
                    None => sink.frame(s.out_t, w, h, &black),
                }
                r.frames += 1;
            }
            (Kind::Video, Some(src)) => {
                vmix_take(s, &mut vid, &mut vmix_run); timeline_core::vmix::take_stack(s, &mut stack_runs); // STRIASTACK (SR9066): a stack index the step leaves empty drops its run
                let f = vid_frame(tl, sources, &mut vid, src, s.src_t, s.seek, tol, &mut r, proxy)?;
                let (rgba, fw, fh) = reel_rgba(sources, src, &f, &mut vid, w, h, reel_fit(tl, src, s.vmix.is_none()), tl.grade_at(s.track, s.out_t, false))?; let rgba = cropped(tl, s.track, s.out_t, false, rgba, fw, fh, reel_fit(tl, src, s.vmix.is_none())); // STRIAGRADEMORE (SR957): the clip's grade inside reel_rgba, on the source frame before the fit, so the bars stay FIELD (file tail); STRIACOLORLIFT (SR869): the clip's grade on the frame it holds; NLESIZESWITCH (SR384): a switched frame letterboxed into its reel's first size; NLESIZEPOLICY (SR394): by the reel's size policy
                sink.frame(s.out_t, fw, fh, &vmix_frame(tl, sources, s, &mut vmix_run, &mut stack_runs, &rgba, w, h, tol, &mut r, proxy)?);
                r.frames += 1;
            }
            (Kind::Audio, _) => {
                let run = run_of.next().expect("one mix run per audio step");
                let Some((rate, ch)) = afmt else { continue };
                let s0 = samples(tl.tb.to_ns(s.out_t), rate);
                let n = samples(tl.tb.to_ns(s.out_t + s.len), rate) - s0;
                let mut taps = sink.taps_tracks().then(Vec::new); // NLE4C: the per-track sums, when the sink meters them
                let pcm = mix_run(tl, run, sources, &mut auds, rate, ch, n, &mut r.seeks, taps.as_mut())?; note_audio_stand_in(&auds, &mut stand_in_audio, &mut r); // NLESTANDINAUDIOEDITOR (SR713)
                sink.pcm(s.out_t, rate, ch, &pcm);
                if let Some(t) = taps {
                    tap_tracks(tl, sink, s.out_t, rate, ch, t);
                }
                r.pcm_frames += n as u64;
            }
        }
    }
    Ok(r)
}

impl Aud {
    /// NLE4: output samples `out_start..out_start + n` at `out_rate` (counted from the source's origin): the source's
    /// own samples when the rates agree, else NLE4B's windowed sinc (`resample::Sinc`, its table kept in `rs`; the window's
    /// neighbours past the media's end hold).
    fn take_at(&mut self, out_start: i64, n: i64, out_rate: u32) -> Result<Vec<f32>, RenderError> {
        if self.rate == out_rate {
            return self.take(out_start, n);
        }
        if n <= 0 {
            return Ok(Vec::new());
        }
        if self.rs.as_ref().map(|s| s.rates()) != Some((self.rate, out_rate)) {
            self.rs = Some(resample::Sinc::new(self.rate, out_rate, self.ch));
        }
        let h = self.rs.as_ref().expect("built above").delay() as i64;
        let (i0, _, _) = resample::linear_pos(out_start, self.rate, out_rate);
        let (i1, _, _) = resample::linear_pos(out_start + n - 1, self.rate, out_rate);
        self.fill(i1 + 1, false)?;
        self.fill(i1 + h + 1, true)?;
        self.seat(i0 - h + 1);
        Ok(self.rs.as_ref().expect("built above").at(&self.buf, self.buf_at, out_start, n as usize))
    }
}

/// NLE4: one mix run's `n` output samples at the project format: each layer on its track's decoder (opened at its
/// `seek`, or when the track's source changed), x gain x envelope, summed in i64 and clamped to i16 once.
#[allow(clippy::too_many_arguments)]
fn mix_run(
    tl: &Timeline,
    run: &AudioRun,
    sources: &Sources,
    auds: &mut [Option<Aud>],
    rate: u32,
    ch: u16,
    n: i64,
    seeks: &mut u64,
    mut taps: Option<&mut Vec<Vec<i64>>>,
) -> Result<Vec<f32>, RenderError> {
    let n = n.max(0);
    let mut acc = vec![0i64; n as usize * ch as usize];
    if let Some(t) = taps.as_mut() {
        **t = vec![vec![0i64; acc.len()]; auds.len() / 2]; // NLE4C: one accumulator per track
    }
    // NLE3b: a layer followed by another of its own track is a crossfade's UNDER layer: its own slot; a track with no
    // under layer in this run drops its under slot.
    let tracks = auds.len() / 2;
    let under = |li: usize| run.under(li); // NLE6MIXAUDIO (rmbp-ledger B953): timeline_core's one slot rule
    for t in 0..tracks {
        if !(0..run.layers.len()).any(|li| under(li) && run.layers[li].track == t) {
            auds[tracks + t] = None;
        }
    }
    for (li, l) in run.layers.iter().enumerate() {
        let slot = &mut auds[if under(li) { tracks + l.track } else { l.track }];
        if !reversed_layer(tl, run, li) && (l.seek.is_some() || slot.as_ref().map(|a| a.src) != Some(l.src)) { // STRIASPEEDREVERSE (SR9067): a reversed layer opens its own slot (file tail)
            let key = match l.seek {
                Some(k) => k,
                None => sources.keyframe_ns(l.src, Kind::Audio, tl.tb.to_ns(l.src_t)).ok_or(RenderError::NoStream(l.src, Kind::Audio))?,
            };
            *slot = Some(Aud::open(sources.get(l.src)?, l.src, key, ch)?);
            *seeks += 1;
        }
        let start = samples(tl.tb.to_ns(l.src_t), rate);
        let pcm = match take_reversed(tl, run, li, sources, slot, rate, ch, n, seeks)? { Some(p) => p, None => take_rated(tl, run, li, slot.as_mut().expect("opened above"), start, n, rate)? }; // STRIASPEEDREVERSE (SR9067): a reversed layer read forward and reversed (file tail); STRIACLIPSPEED (SR829): a rated clip's layer through the same Sinc at the effective rate (file tail)
        mix_into(&mut acc, &pcm, l, n, ch.max(1) as usize);
        if let Some(t) = taps.as_mut() {
            mix_into(&mut t[l.track], &pcm, l, n, ch.max(1) as usize); // NLE4C: the same terms, the master acc untouched
        }
    }
    Ok(out_f32(acc))
}

/// NLE4: add one layer into the accumulator: each sample as i16, x its gain (Q16) x its envelope (Q16, linear from
/// `env[0]` at the run's first sample toward `env[1]` at its end edge).
fn mix_into(acc: &mut [i64], pcm: &[f32], l: &Layer, n: i64, ch: usize) {
    let g = gneiss_pal::dsp::audio::ramp::gain_q16(l.gain_cdb); let pw = [pan_mix(l.pan_cq8, ch), pan_mix(l.pan_to, ch)]; // STRIAPANAUTOMATION (SR9050): the factor at both edges (file tail); STRIAPAN (SR861): the pan factor per channel (file tail); NLE6MIXAUDIO (rmbp-ledger B953): audio_core's one curve
    let (e0, e1) = (l.env[0] as i64, l.env[1] as i64);
    for (j, frame) in pcm.chunks(ch).enumerate() {
        let e = gneiss_pal::dsp::audio::ramp::at(e0, e1, j as i64, n);
        for (c, &x) in frame.iter().enumerate() {
            acc[j * ch + c] += (to_i16(x) * (g * pan_frame(&pw, c.min(1), j as i64, n) >> 16) * e) >> 32;
        }
    }
}

/// NLE4: an `f32` sample (full scale +-1) as a 16-bit value, rounded and clamped.
fn to_i16(x: f32) -> i64 {
    (x as f64 * 32768.0).round().clamp(-32768.0, 32767.0) as i64
}

/// The synthetic sources' audio rate, and sample `i` of the synthetic source whose frame ordinals start at `base`:
/// `(base / 1000 mod 3) * 10000 + i mod 10000` (so a sample names its source and its index). NLE6UTP1SYNTH (SR749): both
/// live in demux_core's `build` beside `synth_reel`, the reel both rings build.
pub use gneiss_pal::dsp::demux::build::{SYNTH_RATE, synth_sample};

/// A codec-free test source (MP4): `frames` `utp1` test-pattern frames, 64x48 at `fps`, ordinals `base..`, a keyframe
/// every 10 frames; plus mono 16-bit PCM at [`SYNTH_RATE`] of [`synth_sample`]s, as long as the video.
pub fn synth_source(base: u32, frames: u32, fps: u32) -> Vec<u8> {
    synth_source_with(base, frames, fps, &build::Mp4Extras::default())
}

/// EDITMAP (SR223): [`synth_source`] written with DEMUXSEEK's [`build::Mp4Extras`] (whole edit lists: track 1 is the video,
/// track 2 the audio).
pub fn synth_source_with(base: u32, frames: u32, fps: u32, x: &build::Mp4Extras) -> Vec<u8> {
    build::synth_reel(base, frames, fps, x) // NLE6UTP1SYNTH (SR749): the body moved to demux_core's build (ONE source)
}

/// The frame at source tick `src_t` of `src` from the decode run `run`: a step with `seek`, or a run on another
/// source (or none), opens a fresh run at the keyframe (the run-up is decoded unseen); otherwise the run decodes on.
/// Shared by a step's own source and a transition's outgoing source (NLE3).
#[allow(clippy::too_many_arguments)]
fn vid_frame(
    tl: &Timeline,
    sources: &Sources,
    run: &mut Option<Vid>,
    src: SourceId,
    src_t: i64,
    seek: Option<i64>,
    tol: i64,
    r: &mut RenderReport,
    proxy: bool, // STRIAPROXYMEDIA (SR882): open on the source's attached proxy when one is (the preview's render)
) -> Result<Frame, RenderError> {
    if let Some(f) = title_frame(tl, sources, run, src, src_t, r) { return Ok(f); } if let Some(f) = seq_frame(tl, sources, run, src, src_t)? { return Ok(f); } let target = tl.tb.to_ns(src_t); // STRIATITLECLIP (SR828): a title's frame is its cached still (file tail); STRIAIMPORTIMAGESEQUENCE (SR888): a sequence's frame is one file read and decoded (file tail)
    if let Some(f) = vid_back(tl, sources, run, src, src_t, seek, tol, r)? { return Ok(f); } if seek.is_some() || run.as_ref().map(|v| v.src) != Some(src) { // STRIASPEEDREVERSE (SR9067): a run stepping backwards is served from its back cache (file tail)
        let key = match seek {
            Some(k) => k,
            None => sources.keyframe_ns(src, Kind::Video, target).ok_or(RenderError::NoStream(src, Kind::Video))?,
        };
        *run = None; *run = Some(match sources.proxied(src, proxy) { Some(p) => Vid::open(&p, src, key).map(|mut v| { v.size = Some((v.track.width, v.track.height)); v })?, None => Vid::open(sources.get(src)?, src, key)? }); // STRIAPROXYMEDIA (SR882): the attached proxy's run, its size seeded (no size-switch line per paint); SWAPBACK-NLECACHE-SHARED (SR397): the outgoing run parks before the new open takes the reel's slot
        r.seeks += 1;
    }
    let f = run.as_mut().expect("opened above").show(target, tol, &mut r.runup_frames);
    if run.as_ref().is_some_and(|v| v.stood_in) {
        sources.note_stand_in(src);
    }
    f
}

/// NLE3: outside a transition the outgoing run is dropped; at a transition tick whose outgoing source continues the
/// live run (`seek` none) and is not already the mix run, that run becomes the mix run (the step's own source then
/// opens its run at the cut, as a plain cut does).
fn vmix_take(s: &timeline_core::schedule::Step, vid: &mut Option<Vid>, vmix_run: &mut Option<Vid>) {
    timeline_core::vmix::take_outgoing(s, vid, vmix_run, |v| v.src); // NLE6MIX (B899): the ONE rule, shared with the kernel Player
}

/// NLE3: the RGBA a video step shows: its own RGBA `b` (`w x h`), or inside a transition `b` mixed with the outgoing
/// source's frame from `vmix_run` (NLE3cBLACK: `b` is the gap's black for a transition to black).
#[allow(clippy::too_many_arguments)]
fn vmix_frame(
    tl: &Timeline,
    sources: &Sources,
    s: &timeline_core::schedule::Step,
    vmix_run: &mut Option<Vid>,
    stack_runs: &mut [Option<Vid>; timeline_core::schedule::STACK_UNDER], // STRIASTACK (SR9066): the layers under the mixed one
    b: &[u8],
    w: u32,
    h: u32,
    tol: i64,
    r: &mut RenderReport,
    proxy: bool, // STRIAPROXYMEDIA (SR882): handed to the outgoing source's vid_frame
) -> Result<Vec<u8>, RenderError> {
    let Some(m) = s.vmix else { return Ok(b.to_vec()) };
    let a = match m.src {
        Some(src) => {
            let a = vid_frame(tl, sources, vmix_run, src, m.src_t, m.seek, tol, r, proxy)?;
            cropped(tl, s.track, s.out_t, true, stack_rgba(tl, sources, s, stack_runs, reel_rgba(sources, src, &a, vmix_run, w, h, reel_fit(tl, src, false), tl.grade_at(s.track, s.out_t, true))?.0, w, h, reel_fit(tl, src, false)), w, h, tol, r)? // STRIAGRADEMORE (SR957): the outgoing clip's grade inside reel_rgba (file tail); STRIACOLORLIFT (SR869): the outgoing clip's grade; STRIASTACK (SR9066): composited over the stack under it (file tail). NLESIZESWITCH (SR384): the outgoing source's frame, as the step's own
        }
        // NLE3c: from black: the outgoing is opaque black (the gap's black).
        None => [0u8, 0, 0, 255].repeat(w as usize * h as usize),
    };
    Ok(xformed(tl, s, &a, b, w, h).unwrap_or_else(|| vmix_rgba(m.kind, m.t, &a, b, w, h))) // STRIAPOSITIONSCALE (SR879): a placed clip over the track below (file tail)
}

/// NLE3: the outgoing frame `a` and the incoming frame `b` (RGBA, `w x h`) mixed at the incoming's weight `t`.
fn vmix_rgba(kind: timeline_core::TransitionKind, t: timeline_core::schedule::Frac, a: &[u8], b: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = a.to_vec();
    timeline_core::vmix::mix_step(kind, t, &mut out, b, w, h); // NLE6MIX (B899): the ONE mix fn, shared with the kernel Player
    out
}

/// EDITMAP (LEDGER SR223): a media file as its own timeline.
impl Sources {
    /// Open `bytes` as the one source of a new timeline at `fps` (reel `reel`) and lay each stream's whole edit list on
    /// its own track (video V1, audio A1) through `Timeline::lay_edit_list` (source time 0 = the stream's first pts, the
    /// origin [`Media::open`] keeps; no edit list = the whole source). `Ok(None)` when the bytes are no container
    /// demux_core knows (`probe`): the caller reads them as an EDL.
    pub fn from_media(reel: &str, bytes: &[u8], fps: u32) -> Result<Option<(Timeline, Sources)>, RenderError> {
        if gneiss_pal::dsp::demux::probe(bytes).is_none() && !gneiss_pal::dsp::demux::wav::is_wav(bytes) { // STRIAIMPORTWAV (SR889): a WAV is its own one-track timeline
            return Ok(None);
        }
        let mut tl = Timeline::new(fps);
        let mut sources = Sources::new(&tl);
        let id = SourceId(0);
        sources.add(id, bytes.to_vec())?;
        let src = tl.add_source(reel, sources.duration(id).unwrap_or(0));
        let m = sources.get(id)?;
        let mut applied = None;
        for kind in [Kind::Video, Kind::Audio] {
            let Some((i, origin)) = m.stream(kind) else { continue };
            let track = tl.add_track(kind);
            let tb = m.demux.tracks()[i].timebase;
            tl = tl.lay_edit_list(track, src, m.demux.edit_list(i), tb, origin).map_err(RenderError::Edit)?;
            applied.get_or_insert(tl.tracks[track].clips.len());
        }
        sources.applied.push((src, applied.unwrap_or(0)));
        Ok(Some((tl.link_av(), sources))) // STRIALINKAUTO (SR911): each V1 clip and its equal A1 clip linked, one group per pair (timeline_core lib.rs tail)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// THE METERS (NLE4C, LEDGER SR301)
// ---------------------------------------------------------------------------------------------------------------------

/// NLE4: the mix accumulator as the PCM sent: each sum clamped to 16 bits once, as `f32` (full scale +-1).
fn out_f32(acc: Vec<i64>) -> Vec<f32> {
    acc.into_iter().map(|a| a.clamp(-32768, 32767) as f32 / 32768.0).collect()
}

/// NLE4C: hand each AUDIO track's own sum of the run just sent to the sink.
fn tap_tracks(tl: &Timeline, sink: &mut dyn RenderSink, out_t: i64, rate: u32, ch: u16, taps: Vec<Vec<i64>>) {
    for (t, acc) in taps.into_iter().enumerate() {
        if tl.tracks.get(t).is_some_and(|tr| tr.kind == Kind::Audio) {
            sink.track_pcm(out_t, t, rate, ch, &out_f32(acc));
        }
    }
}

/// NLE4C: one meter's reading. `track` is the audio track's index in [`Timeline::tracks`], `None` the master.
pub use timeline_core::view::MeterLevel; // STRIAKMETER (SR9058): the reading type lives in timeline_core::view, both faces paint it

/// NLE4C (SR301): the editor's meters, per audio track and the master, fed by [`render`] itself: the master reads the
/// PCM [`RenderSink::pcm`] is handed (what is sent), each track its own sum from `mix_run` ([`RenderSink::track_pcm`]).
/// One `audio_core::meter::Meter` each; no lock (the owner feeds and reads it on one thread).
#[derive(Debug, Clone, Default)]
pub struct MixMeters {
    tracks: Vec<(usize, Meter)>,
    master: Option<Meter>,
    /// The output tick the last feed ended at.
    next: Option<i64>,
}

impl MixMeters {
    /// Render `range` of `tl` into the meters. A range that does not continue the last feed (a seek, a restart) starts
    /// them from silence: the window follows contiguous playback.
    pub fn feed(&mut self, tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources) -> Result<RenderReport, RenderError> {
        if self.next != Some(range.start) {
            self.reset();
        }
        let end = range.end;
        let r = render(tl, range, sources, self)?;
        self.next = Some(end);
        Ok(r)
    }
    /// Back to silence (no meters until the next feed).
    pub fn reset(&mut self) {
        self.tracks.clear();
        self.master = None;
        self.next = None;
    }
    /// The readings: the audio tracks in order, then the master. Empty before the first feed.
    pub fn levels(&self) -> Vec<MeterLevel> {
        let lv = |track: Option<usize>, m: &Meter| MeterLevel { track, peak_db: m.peak_db(), rms_db: m.rms_db() };
        self.tracks.iter().map(|(t, m)| lv(Some(*t), m)).chain(self.master.iter().map(|m| lv(None, m))).collect()
    }
}

/// A meter at `rate`, rebuilt from silence when the rate changes.
fn meter_at(m: &mut Option<Meter>, rate: u32) -> &mut Meter {
    if m.as_ref().map(Meter::rate) != Some(rate.max(1)) {
        *m = Some(Meter::new(rate));
    }
    m.as_mut().expect("set above")
}

impl RenderSink for MixMeters {
    fn frame(&mut self, _out_t: i64, _width: u32, _height: u32, _rgba: &[u8]) {}
    fn pcm(&mut self, _out_t: i64, rate: u32, channels: u16, interleaved: &[f32]) {
        meter_at(&mut self.master, rate).push(interleaved, channels as usize);
    }
    fn taps_tracks(&self) -> bool {
        true
    }
    fn track_pcm(&mut self, _out_t: i64, track: usize, rate: u32, channels: u16, interleaved: &[f32]) {
        let i = match self.tracks.binary_search_by_key(&track, |(t, _)| *t) {
            Ok(i) => i,
            Err(i) => {
                self.tracks.insert(i, (track, Meter::new(rate)));
                i
            }
        };
        let m = &mut self.tracks[i].1;
        if m.rate() != rate.max(1) {
            *m = Meter::new(rate);
        }
        m.push(interleaved, channels as usize);
    }
}

/// SWAPBACK (LEDGER SR329): the editor's half of [`crate::decode_swap`] — a reel a render drew stand-in frames for says
/// so in its [`OpenReport`] (` decode=stand-in`).
impl Sources {
    fn note_stand_in(&self, id: SourceId) {
        if let Ok(mut v) = self.stand_in.lock() {
            if !v.contains(&id) {
                v.push(id);
            }
        }
    }
    /// NLE-STANDIN-OPEN (LEDGER SR353): a reel opened as source `id` whose video track no decoder fits is a STAND-IN reel
    /// from its open (its runs draw the labelled stand-in, [`crate::decode_swap::open`], as the Player plays it); a
    /// re-added `id` forgets its old mark first.
    fn note_open(&self, id: SourceId, m: &Media) {
        if let Ok(mut v) = self.stand_in.lock() {
            v.retain(|s| *s != id);
        }
        if m.video.is_some_and(|i| decoder_for(&m.demux.tracks()[i]).is_err()) {
            self.note_stand_in(id);
        }
    }
}

/// COLOURONE (LEDGER SR328): a frame of source `src` as RGBA, converted by its reel's colour (`Track::colour`, the
/// container's else the bitstream's) through `dsp::video::Frame::to_rgba_colour`. Both of the render's frame -> RGBA
/// lines (a step's own source and a transition's outgoing source) call it. AV1PLAYERCOLOUR (SR347): by the reel track's `chroma_of`.
fn src_rgba(sources: &Sources, src: SourceId, f: &Frame, run: Option<&Track>) -> Vec<u8> {
    let track = run.or_else(|| sources.get(src).ok().and_then(|m| m.video.map(|i| &m.demux.tracks()[i]))); // STRIAPROXYMEDIA (SR882): the run's own track (a proxy's colour; the same track for an original)
    f.to_rgba_colour(track.and_then(|t| t.colour.as_ref()), track.map_or(gneiss_pal::dsp::video::Chroma::Nearest, gneiss_pal::dsp::video::chroma_of))
}

// ── NLESIZESWITCH (LEDGER SR384): the editor's run across a mid-stream size switch (handlers.md §5 NLEPROBE-HDR's NLE-SIZE-SWITCH) ──
/// A frame of source `src` as RGBA at the project size `w x h`, for both of the render's frame -> RGBA sites. The frame is
/// converted at its own size by [`src_rgba`]; a frame whose size differs from its reel's (the reel track's `width x height`,
/// the reel's first size, the size the project takes) is LETTERBOXED into the reel's size by the editor's ONE fit,
/// `timeline_core::view::Canvas::blit_fit` (aspect kept, centred, over the preview band's black `FIELD`), with one
/// `[STRIA] :: nle size switch reel=<id> <W>x<H>-><w>x<h> scaled` line each time the run `run` sees its frame size change to
/// a size other than the reel's. A reel whose own size is not the project's stays [`RenderError::Size`].
/// NLESIZEPOLICY (LEDGER SR394): `fit` is the reel's [`SizePolicy`] ([`reel_fit`]): Letterbox as above, Fill / Stretch
/// through the same painter's `Canvas::blit_policy` (` fit=<name>` appended to the line), Adopt hands the frame on at its own
/// size (`[STRIA] :: nle size switch reel=<id> <W>x<H> adopted`); returns the RGBA and its size.
#[allow(clippy::too_many_arguments)] // STRIAGRADEMORE (SR957): the grade made it eight
fn reel_rgba(sources: &Sources, src: SourceId, f: &Frame, run: &mut Option<Vid>, w: u32, h: u32, fit: SizePolicy, g: timeline_core::Grade) -> Result<(Vec<u8>, u32, u32), RenderError> { // STRIAGRADEMORE (SR957): `g` the step's clip grade (file tail)
    let now = (f.width, f.height);
    let reel = sources.get(src).ok().and_then(|m| m.video.map(|i| (m.demux.tracks()[i].width, m.demux.tracks()[i].height))).filter(|&(rw, rh)| rw > 0 && rh > 0).unwrap_or(now);
    let last = run.as_mut().and_then(|v| v.size.replace(now)); let rt = run.as_ref().map(|v| &v.track); // STRIAPROXYMEDIA (SR882): the run's track converts its frame
    if now == (w, h) {
        return Ok((graded(g, src_rgba(sources, src, f, rt)), w, h));
    }
    if reel != (w, h) {
        return Err(RenderError::Size { src, width: now.0, height: now.1 });
    }
    let rgba = graded(g, src_rgba(sources, src, f, rt)); let fit = proxy_fit(fit, rt, reel); // STRIAPROXYMORE (SR979): a proxied run's frame up-fits to the reel under Adopt (file tail)
    if fit == SizePolicy::Adopt {
        if last != Some(now) {
            log::info!("[STRIA] :: nle size switch reel={} {}x{} adopted", src.0, now.0, now.1);
        }
        return Ok((rgba, now.0, now.1));
    }
    if last != Some(now) {
        let tok = if fit == SizePolicy::Letterbox { String::new() } else { format!(" fit={}", fit.name()) };
        log::info!("[STRIA] :: nle size switch reel={} {}x{}->{}x{} scaled{}", src.0, now.0, now.1, reel.0, reel.1, tok);
    }
    use timeline_core::view::{Canvas, FIELD, Order, Still};
    let mut c = Canvas::new(reel.0, reel.1, Order::Rgba);
    c.rect(0, 0, reel.0 as i64, reel.1 as i64, FIELD);
    c.blit_policy(&Still { width: now.0, height: now.1, rgba }, 0, 0, reel.0, reel.1, fit);
    Ok((c.px, reel.0, reel.1))
}

/// NLESIZEPOLICY (LEDGER SR394): source `src`'s size policy on `tl` (absent: Letterbox). `alone` is false inside a
/// transition: the mix is one size, so Adopt mixes as Letterbox there.
fn reel_fit(tl: &Timeline, src: SourceId, alone: bool) -> SizePolicy {
    tl.reel_fit(src, alone) // STRIAKCROP (SR9010): the rule lifted into timeline_core, the kernel Player's crop calls it too
}

/// SWAPBACK-NLECACHE (LEDGER SR388): the editor's runs park their decoder across runs. A run's close (a cut, a seek step, the
/// end of a render: every preview tick is one render) hands its decoder and [`DecodeSwap`] (its parked spans, LRU, keyed by set
/// state) back to its reel's `Media`, and the reel's next run open takes them, resets the decoder and reseeds it — the
/// Player's seek, so a scrub back into a span reuses its decoder (`[STRIA] :: nle reseed reel=<id> cache=hit|miss`). The slot
/// is locked only for that take and this put, never across a decode. A second live run of one reel (a transition between two
/// of its clips) opens fresh, and its close leaves an occupied slot as it is (SWAPBACK-NLECACHE-SHARED owed).
type ReelPark = Arc<std::sync::Mutex<Option<(DecodeSwap, Box<dyn VideoDecoder>)>>>;

impl Drop for Vid {
    fn drop(&mut self) {
        let Ok(mut slot) = self.park.lock() else { return };
        if slot.is_none() {
            let dec = std::mem::replace(&mut self.dec, Box::new(gneiss_pal::dsp::video::TestPattern::stand_in(&self.track)));
            *slot = Some((std::mem::take(&mut self.swap), dec));
        }
    }
}

// ---- NLESTANDINAUDIOEDITOR (LEDGER SR713): the editor's audio reels on media.rs's silent stand-in (named tail block) ----
/// NLESTANDINAUDIOEDITOR (LEDGER SR713): the editor's audio reel opens through [`crate::media::AudioSwap::open`] (`Aud::open`),
/// so a reel whose audio no decoder takes, or whose decoder refuses a packet, renders silence instead of failing the render.
/// After each mix run, a live slot whose decoder reads "stand-in" (the open-time stand-in, or a refused decoder parked at the
/// run's end) adds its reel once; [`RenderReport::stand_in_audio`] is their count. Local to one [`render`]: no lock.
fn note_audio_stand_in(auds: &[Option<Aud>], reels: &mut Vec<SourceId>, r: &mut RenderReport) {
    for a in auds.iter().flatten() {
        if a.dec.name() == "stand-in" && !reels.contains(&a.src) {
            reels.push(a.src);
        }
    }
    r.stand_in_audio = reels.len() as u32;
}

// ---- STRIAINFOREEL (LEDGER SR743): a reel's one-line summary for a person (named tail block) ----
/// STRIAINFOREEL (LEDGER SR743): the editor's reels say what they are, as the Player's open does (SR734's `summary_line`).
impl Sources {
    /// Source `id`'s person's line: its video track's codec, size, profile tokens (`dsp::video::profile_level`) and bit rate
    /// (`media::bitrate_of` over the track's btrt, its SPS HRD rate and the file's bytes over its duration), through
    /// `media::summary_parts` — the SAME `Track` facts and fns `Player::open_from` fills `MediaInfo` from. `None` for an
    /// unknown id or a reel with nothing to say. Reads the open demuxer only (no survey walk, no lock). STRIAINFOREELAUDIO (SR761):
    /// then ` | <audio decoder> <rate>` from the reel's audio Track (`media::track_audio_decoder` / `media::track_audio_bitrate`).
    pub fn summary(&self, id: SourceId) -> Option<String> {
        if let Some(q) = self.seq(id) { return Some(q.summary()); } let m = self.get(id).ok()?; // STRIASEQINFO (SR940): a sequence reel's line (file tail)
        let v = m.video.map(|i| &m.demux.tracks()[i]);
        let (codec, w, h) = v.map_or(("", 0, 0), |t| (t.codec_name.as_str(), t.width, t.height));
        let profile = v.and_then(gneiss_pal::dsp::video::profile_level).unwrap_or_default();
        let rate = crate::media::bitrate_of(v.and_then(|t| t.btrt), v.and_then(gneiss_pal::dsp::video::hrd_bit_rate), m.demux.bytes().len() as u64, m.demux.duration_ns());
        let a = m.audio.map(|i| &m.demux.tracks()[i]); // STRIAINFOREELAUDIO (SR761): the reel's audio track, as the Player's first audio track
        let (adec, arate) = a.map_or(("", String::new()), |t| (crate::media::track_audio_decoder(t, &m.demux), crate::media::track_audio_bitrate(t, Some(&m.demux))));
        Some(crate::media::summary_parts(codec, w, h, &profile, &rate, adec, &arate)).filter(|s| !s.is_empty())
    }
}

// ---- STRIAWAVEFORM (LEDGER SR827): one source's peak table through the audio road (named tail block) ----

/// STRIAWAVEFORM (SR827): source `id`'s peak table at `per_sec` buckets per second (timeline_core's ONE `view::peaks_of`),
/// its audio pulled through the render's own road: ONE [`Aud`] (`Aud::open` at source time 0, the decoder's own rate, the
/// channel count `render` reads, `track.channels`), one second at a time (`fill` soft to the stream's end, `drain`: a hole
/// is silence), so a source is never held whole in memory. `Ok(None)`: the source has no audio stream. The face calls it
/// once per source (nle_face.rs `Face::wave_fill`), on its own thread (owed NLE-WAVEFORM-SCAN-THREAD).
pub fn scan_peaks(sources: &Sources, id: SourceId, per_sec: u32) -> Result<Option<timeline_core::view::Peaks>, RenderError> {
    let m = sources.get(id)?; if let Some(p) = sources.peak_file_read(id, per_sec) { return Ok(Some(p)); } // STRIAWAVEMULTI (SR9071): a cache file beside the media whose stamp matches answers without a decode (file tail)
    let Some(i) = m.audio else { return Ok(None) };
    let ch = m.demux.tracks()[i].channels.max(1);
    let mut a = Aud::open(m, id, 0, ch)?;
    let rate = a.rate.max(1);
    let step = rate as i64;
    let mut p = timeline_core::view::Peaks { per_sec, buckets: Vec::new(), chans: Vec::new(), lod: Vec::new() };
    for k in 0i64.. {
        a.fill((k + 1) * step, true)?;
        let pcm = a.drain(k * step, step);
        if pcm.is_empty() {
            break;
        }
        p.append(timeline_core::view::peaks_of(rate, ch, &pcm, per_sec)); // STRIAWAVEFORMSTEREO (SR9051): a stereo source keeps its L / R tables (file tail)
    }
    p.finish(); sources.peak_file_write(id, &p); Ok(Some(p)) // STRIAWAVEFORMSTEREO (SR9051): the max pyramid of the whole table (file tail); STRIAWAVEMULTI (SR9071): the table written beside the media (file tail)
}

// ---- STRIAWAVEMULTI (LEDGER SR9071): the peak table cached beside the media (named tail block) ----
// Stria keeps no on-disk cache anywhere else: the face's tables live in memory (nle_face.rs `WavePeaks.map`), filled by
// `wave_fill` and the SR997 worker, BOTH through [`scan_peaks`]; so the cache file sits inside that one road. OPT-IN: a
// source is cached only after [`Sources::note_path`] names its media file (stria-view's `--peak-cache`); `from_media`,
// the synth fixture, the export thread and every test open bytes without a path and never read or write a file. The file
// is `<media>.una-peaks` beside the media, stamped with the media's byte length and mtime (ns since the epoch): a stamp
// or `per_sec` that does not match, or a length that is not exact, is a miss (rescan, overwrite). The pyramid is never
// stored (`Peaks::finish` rebuilds it on read). A write that fails (a read-only dir) is logged at debug and ignored. No
// lock: `peak_paths` is filled before the face wraps `Sources` in its `Arc` and only read after; a proxy attached in place
// (`set_proxy`) keeps it. Owed: NLE-WAVEFORM-CACHE-DIR (a per-user cache dir for a read-only media dir),
// NLE-WAVEFORM-CACHE-PATHS (tools/nle and `from_media` name no path), NLE-WAVEFORM-CACHE-PREF (a saved preference instead
// of stria-view's flag).

/// STRIAWAVEMULTI (SR9071): the peak cache file's magic and format version.
pub const PEAK_FILE_MAGIC: &[u8; 8] = b"UNAPEAKS";
pub const PEAK_FILE_VERSION: u32 = 1;

impl Sources {
    /// STRIAWAVEMULTI (SR9071): source `id`'s media file is `path`: its peak table is read from and written to
    /// `<path>.una-peaks` from now on (a re-noted `id` forgets its old path, as [`Sources::add`] does).
    pub fn note_path(&mut self, id: SourceId, path: &std::path::Path) {
        self.peak_paths.retain(|(s, _)| *s != id);
        self.peak_paths.push((id, path.to_path_buf()));
    }
    /// STRIAWAVEMULTI (SR9071): source `id`'s cache file and its media's stamp (`None`: no path noted, or the media's
    /// length / mtime unreadable).
    fn peak_stamp(&self, id: SourceId) -> Option<(std::path::PathBuf, u64, u64)> {
        let media = &self.peak_paths.iter().find(|(s, _)| *s == id)?.1;
        let md = std::fs::metadata(media).ok()?;
        let mtime = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos() as u64;
        let mut file = media.clone().into_os_string();
        file.push(".una-peaks");
        Some((std::path::PathBuf::from(file), md.len(), mtime))
    }
    /// STRIAWAVEMULTI (SR9071): source `id`'s cached table when the file's stamp and `per_sec` match (its pyramid rebuilt).
    fn peak_file_read(&self, id: SourceId, per_sec: u32) -> Option<timeline_core::view::Peaks> {
        let (file, size, mtime) = self.peak_stamp(id)?;
        let bytes = std::fs::read(&file).ok()?;
        let p = peak_file_parse(&bytes, size, mtime, per_sec);
        if p.is_some() {
            log::debug!("[nle] peak cache hit {}", file.display());
        }
        p
    }
    /// STRIAWAVEMULTI (SR9071): write source `id`'s table beside its media (no path noted: nothing).
    fn peak_file_write(&self, id: SourceId, p: &timeline_core::view::Peaks) {
        let Some((file, size, mtime)) = self.peak_stamp(id) else { return };
        if let Err(e) = std::fs::write(&file, peak_file_bytes(p, size, mtime)) {
            log::debug!("[nle] peak cache {}: {e}", file.display());
        }
    }
}

/// STRIAWAVEMULTI (SR9071): `p` as a cache file stamped `size` / `mtime_ns` (little-endian): magic, version, size,
/// mtime, `per_sec`, the lane count, the folded table (u32 length + bytes), then each lane (u32 length + bytes).
pub fn peak_file_bytes(p: &timeline_core::view::Peaks, size: u64, mtime_ns: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(34 + p.buckets.len() * (1 + p.chans.len()) + 4 * p.chans.len());
    out.extend_from_slice(PEAK_FILE_MAGIC);
    out.extend_from_slice(&PEAK_FILE_VERSION.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&mtime_ns.to_le_bytes());
    out.extend_from_slice(&p.per_sec.to_le_bytes());
    out.extend_from_slice(&(p.chans.len() as u16).to_le_bytes());
    for t in core::iter::once(&p.buckets).chain(p.chans.iter().map(|c| &c.buckets)) {
        out.extend_from_slice(&(t.len() as u32).to_le_bytes());
        out.extend_from_slice(t);
    }
    out
}

/// STRIAWAVEMULTI (SR9071): the table a cache file holds, when its magic, version, stamp and `per_sec` match and its
/// length is exact; the pyramid of it and of each lane rebuilt (`Peaks::finish`). Anything else: `None`.
pub fn peak_file_parse(bytes: &[u8], size: u64, mtime_ns: u64, per_sec: u32) -> Option<timeline_core::view::Peaks> {
    let mut at = 0usize;
    if peak_take(bytes, &mut at, 8)? != PEAK_FILE_MAGIC {
        return None;
    }
    let u32_of = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let u64_of = |b: &[u8]| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
    if u32_of(peak_take(bytes, &mut at, 4)?) != PEAK_FILE_VERSION || u64_of(peak_take(bytes, &mut at, 8)?) != size || u64_of(peak_take(bytes, &mut at, 8)?) != mtime_ns || u32_of(peak_take(bytes, &mut at, 4)?) != per_sec {
        return None;
    }
    let l = peak_take(bytes, &mut at, 2)?;
    let lanes = u16::from_le_bytes([l[0], l[1]]) as usize;
    let mut tables: Vec<Vec<u8>> = Vec::with_capacity(lanes + 1);
    for _ in 0..=lanes {
        let n = u32_of(peak_take(bytes, &mut at, 4)?) as usize;
        tables.push(peak_take(bytes, &mut at, n)?.to_vec());
    }
    if at != bytes.len() {
        return None;
    }
    let mut it = tables.into_iter();
    let buckets = it.next()?;
    let chans = it.map(|b| timeline_core::view::Peaks { per_sec, buckets: b, chans: Vec::new(), lod: Vec::new() }).collect();
    let mut p = timeline_core::view::Peaks { per_sec, buckets, chans, lod: Vec::new() };
    p.finish();
    Some(p)
}

/// STRIAWAVEMULTI (SR9071): the next `n` bytes of `b` from `*at` (advanced past them); `None` past the end.
fn peak_take<'a>(b: &'a [u8], at: &mut usize, n: usize) -> Option<&'a [u8]> {
    let s = b.get(*at..at.checked_add(n)?)?;
    *at += n;
    Some(s)
}

// ---- STRIATITLECLIP (LEDGER SR828): the title card's frames (named tail block) ----
/// STRIATITLECLIP (LEDGER SR828): a generated title source (`timeline_core::Source::title`) has no bytes: its frame is ONE
/// still, white text on opaque black, one line, centred, drawn by text_host's `Line::paint` (the face's label road) at the
/// render's frame size and cached on [`Sources`]. The frame size stays render's rule (the first scheduled media video step's
/// reel; a titled step sizes nothing); a range whose video steps size nothing but hold a title takes the first untitled
/// source in `tl.sources` with a video stream, else [`TITLE_SIZE`]. The schedule keys a title itself (every frame is a key).
pub const TITLE_SIZE: (u32, u32) = timeline_core::view::TITLE_SIZE; // STRIAKTITLE (SR9009): lifted into timeline_core, the kernel Player's too
/// STRIATITLECLIP: the stills a [`Sources`] keeps, oldest dropped past this many.
const TITLE_CACHE_MAX: usize = 8;
/// STRIATITLECLIP: the title ink (straight alpha).
const TITLE_INK: [u8; 4] = timeline_core::view::TITLE_INK; // STRIAKTITLE (SR9009): lifted into timeline_core

/// STRIATITLECLIP: the still cache, `(text, w, h, still)`; locked for one find or one push, never across a raster.
type TitleCache = std::sync::Mutex<Vec<(String, u32, u32, Arc<timeline_core::view::Still>, timeline_core::TitleStyle)>>; // STRIAFREEZETHREE (SR9029): keyed by the style too

/// STRIATITLECLIP: source `src` of `tl` is a title.
fn titled(tl: &Timeline, src: SourceId) -> bool {
    tl.source(src).is_some_and(|s| s.title.is_some())
}

/// STRIATITLECLIP: when nothing sized the frame and a video step (or its outgoing) is a title, the size rule above.
fn title_size(tl: &Timeline, sources: &Sources, steps: &[timeline_core::schedule::Step], r: &mut RenderReport) {
    if r.width != 0 {
        return;
    }
    // STRIAKTITLE (SR9009): the rule is timeline_core's `view::title_size` (the kernel Player calls the same fn)
    let size = timeline_core::view::title_size(tl, steps, |id| {
        if let Some(q) = sources.seq(s.id) { return Some((q.w, q.h)); } let m = sources.get(id).ok()?; // STRIASEQMORE (SR978): a sequence reel sizes a title-only range by its first file (file tail)
        let t = &m.demux.tracks()[m.video?];
        (t.width > 0 && t.height > 0).then_some((t.width, t.height))
    });
    if let Some(wh) = size {
        (r.width, r.height) = wh;
    }
}

/// STRIATITLECLIP: the frame of title source `src` at source tick `src_t` (`None`: not a title): the cached still at the
/// render's size as an RGBA `Frame`. The live decode run is dropped (parked), so the next media step opens by its seek.
fn title_frame(tl: &Timeline, sources: &Sources, run: &mut Option<Vid>, src: SourceId, src_t: i64, r: &RenderReport) -> Option<Frame> {
    let text = tl.source(src)?.title.as_deref()?;
    *run = None;
    let still = sources.title_card_cached(text, &tl.source(src)?.title_style, r.width, r.height); // STRIAFREEZETHREE (SR9029): the source's style
    Some(Frame { width: still.width, height: still.height, pts_ns: tl.tb.to_ns(src_t), pixels: gneiss_pal::dsp::video::Pixels::Rgba(still.rgba.clone()) })
}

/// STRIATITLECLIP: `text` as a `w` x `h` RGBA still: opaque black, white ink, one line through text_host at `h / 10` px (8 at
/// least), shrunk once to 90% of the width when wider, centred on its line box (STRIAFREEZETHREE SR9029: [`title_card`] at the default style).
pub fn title_still(text: &str, w: u32, h: u32) -> timeline_core::view::Still {
    title_card(text, &timeline_core::TitleStyle::DEFAULT, w, h) // STRIAFREEZETHREE (SR9029): the one raster at the default style
}

impl Sources {
    /// STRIATITLECLIP: [`title_still`] once per `(text, w, h)`; a second ask returns the same `Arc`.
    pub fn title_still_cached(&self, text: &str, w: u32, h: u32) -> Arc<timeline_core::view::Still> {
        self.title_card_cached(text, &timeline_core::TitleStyle::DEFAULT, w, h) // STRIAFREEZETHREE (SR9029): the one cache at the default style
    }
}

// ---- STRIACLIPSPEED (LEDGER SR829): audio at a clip's speed — the same resampler at the effective rate (named tail block) ----
// THE rule: a layer of a clip at `rate_q16` is resampled by that rate through NLE4B's ONE converter (`Aud::take_at`,
// audio_core's `resample::Sinc`): the run's `n` output samples are asked at the EFFECTIVE output rate
// `eff = round(out_rate * 0x1_0000 / rate_q16)` Hz, so they cover `rate` times the 1x source span (pitch follows speed, as a
// tape does; going faster is anti-aliased by the sinc's cutoff; `eff` equal to the source's rate is the plain sample copy).
// The run's start is the layer's exact `src_t` at `eff`, so the < 1 Hz rounding of `eff` never accumulates past a run. A
// crossfade's UNDER layer and a fade-to-silence tail play the outgoing's rate (STRIACLIPSPEEDTHREE SR9031: src_at past its end). The video needs no
// arm: the schedule names `src_at(t)` and `Vid::show` decodes and drops forward (rate > 1) or holds the last frame (rate < 1).
// A pitch-preserving stretch is owed (NLE-SPEED-PITCH-PRESERVE).

/// STRIACLIPSPEED: layer `li`'s `n` output samples at `rate`: exactly `take_at(start, n, rate)` at 1x, else `take_at` at the
/// effective rate from the layer's `src_t`.
#[allow(clippy::too_many_arguments)]
fn take_rated(tl: &Timeline, run: &AudioRun, li: usize, a: &mut Aud, start: i64, n: i64, rate: u32) -> Result<Vec<f32>, RenderError> {
    let q = run.layer_rate(tl, li); // STRIAKSPEED (SR9012): the rule lifted into timeline_core (schedule.rs `AudioRun::layer_rate`), the kernel Player calls it too
    if q == 0x1_0000 || q == 0 {
        return a.take_at(start, n, rate);
    }
    let eff = timeline_core::schedule::rated_hz(rate, q); // STRIAKSPEED (SR9012): the effective rate lifted into timeline_core (schedule.rs `rated_hz`)
    let l = &run.layers[li];
    a.take_at(samples(tl.tb.to_ns(l.src_t), eff), n, eff)
}

// ---- STRIACROP (LEDGER SR867): a step's frame through the clip's crop (named tail block) ----
/// The RGBA `rgba` (`w x h`) a video step on `track` at `out_t` holds, through its clip's crop (`outgoing`: a transition's
/// outgoing clip, `Timeline::crop_at`) by timeline_core's ONE crop pass `crop_frame`, placed by the reel's policy `fit` over
/// the preview band's black `FIELD`. No crop (or no track): `rgba` as it came, byte for byte.
#[allow(clippy::too_many_arguments)]
fn cropped(tl: &Timeline, track: Option<usize>, out_t: i64, outgoing: bool, rgba: Vec<u8>, w: u32, h: u32, fit: SizePolicy) -> Vec<u8> {
    let crop = track.map_or(timeline_core::Crop::NONE, |ti| tl.crop_at(ti, out_t, outgoing));
    if crop == timeline_core::Crop::NONE || (!outgoing && track.is_some_and(|ti| tl.xform_at(ti, out_t) != timeline_core::Xform::NONE)) { // STRIAPOSITIONSCALE (SR879): a placed clip's crop is applied inside its rect (xformed)
        return rgba;
    }
    let (px, _) = rgba.as_chunks::<4>();
    timeline_core::crop_frame(px, w as usize, h as usize, crop, fit, timeline_core::view::FIELD).into_flattened()
}

// ---- STRIALOUDNESSREADOUT (LEDGER SR866): a range's integrated loudness through render (named tail block) ----

/// STRIALOUDNESSREADOUT (SR866): the PCM-only sink (MixMeters' shape) feeding ONE `audio_core::meter::Loudness` built
/// at the first PCM's rate and channel count (`render` fixes both for the whole range).
#[derive(Debug, Default)]
struct LoudnessSink {
    l: Option<gneiss_pal::dsp::audio::meter::Loudness>, watch: Option<LoudnessWatch>, // STRIALOUDNESSCANCEL (SR903): the stop flag and the ticks-done count (file tail)
}

impl RenderSink for LoudnessSink {
    fn frame(&mut self, out_t: i64, _width: u32, _height: u32, _rgba: &[u8]) { self.seen(out_t); } fn stop(&self) -> bool { self.cancelled() } fn frame_wanted(&mut self, out_t: i64) -> bool { self.seen(out_t); false } // STRIALOUDNESSMORE (SR961): the tick counted, the frame declined (no video decode); STRIALOUDNESSCANCEL (SR903): progress and the stop hook (file tail)
    fn pcm(&mut self, out_t: i64, rate: u32, channels: u16, interleaved: &[f32]) {
        self.seen(out_t); self.l.get_or_insert_with(|| gneiss_pal::dsp::audio::meter::Loudness::new(rate, channels as usize)).push(interleaved);
    }
}

/// STRIALOUDNESSREADOUT (SR866): the BS.1770 integrated loudness (LUFS) of `range` of `tl` as [`render`] mixes it (the
/// PCM an export of that range sends); `None` when the range sent no PCM (no audio track, or no audio in its sources).
pub fn loudness(tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources) -> Result<Option<f64>, RenderError> {
    loudness_watched(tl, range, sources, None) // STRIALOUDNESSCANCEL (SR903): the one measure body (file tail)
}

// ---- STRIALOUDNESSCANCEL (LEDGER SR903): the measure's stop and progress through RenderSink (named tail block) ----

/// STRIALOUDNESSCANCEL (SR903): what a running measure shares with its face: `cancel` read by [`RenderSink::stop`] once
/// per step (the export's road: no decode past it), `done` the ticks of the range rendered so far (raised by the sink's
/// own callbacks). Relaxed atomics: a display count and a stop flag, as the export job's.
#[derive(Debug, Clone)]
pub struct LoudnessWatch {
    pub start: i64,
    pub done: Arc<std::sync::atomic::AtomicU64>,
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl LoudnessWatch {
    /// A fresh watch over `range`: nothing done, not cancelled.
    pub fn new(range: &std::ops::Range<i64>) -> LoudnessWatch {
        LoudnessWatch { start: range.start, done: Arc::new(std::sync::atomic::AtomicU64::new(0)), cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)) }
    }
}

impl LoudnessSink {
    /// The step at `out_t` was sent: raise the ticks-done count (monotonic).
    fn seen(&self, out_t: i64) {
        if let Some(w) = &self.watch {
            w.done.fetch_max((out_t - w.start).max(0) as u64, std::sync::atomic::Ordering::Relaxed);
        }
    }
    /// True once the watch's cancel flag is set.
    fn cancelled(&self) -> bool {
        self.watch.as_ref().is_some_and(|w| w.cancel.load(std::sync::atomic::Ordering::Relaxed))
    }
}

/// STRIALOUDNESSCANCEL (SR903): [`loudness`] with an optional watch: progress through `done`, a stop at the next step
/// once `cancel` is set (the partial range's measure is returned; the face names it cancelled by its own flag).
pub fn loudness_watched(tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources, watch: Option<LoudnessWatch>) -> Result<Option<f64>, RenderError> {
    Ok(loudness_reading(tl, range, sources, watch)?.map(|r| r.lufs)) // STRIALOUDNESSNEXT (SR995): the one measure body (file tail)
}

// ---- STRIACOLORLIFT (LEDGER SR869): the step's clip grade on the RGBA the render holds (named tail block) ----
/// The frame `rgba` (straight RGBA) through timeline_core's ONE per-pixel pass [`timeline_core::Grade::apply`]; an
/// IDENTITY grade hands the frame back untouched (no pass: an ungraded timeline renders byte-identically).
fn graded(g: timeline_core::Grade, mut rgba: Vec<u8>) -> Vec<u8> {
    if g != timeline_core::Grade::IDENTITY {
        g.apply(&mut rgba);
    }
    rgba
}

// ---- STRIAPAN (LEDGER SR861): the clip's pan in the mix (named tail block) ----

/// STRIAPAN: the per-channel factor (Q16, 65536 = unity) `mix_into` multiplies a layer's gain by: timeline_core's ONE rule
/// [`timeline_core::schedule::pan_mix_q16`] (STRIAKPAN SR9015 lifted it there; the kernel Player calls it too). A
/// full-scale clip panned hard can exceed full scale on its side, and `out_f32`'s clamp catches it.
fn pan_mix(pan_cq8: i8, ch: usize) -> [i64; 2] {
    timeline_core::schedule::pan_mix_q16(pan_cq8, ch)
}

#[cfg(test)]
mod striapan_tests {
    use super::*;
    use timeline_core::SourceId;
    use timeline_core::schedule::ENV_ONE;

    fn layer(pan_cq8: i8) -> Layer {
        Layer { track: 0, src: SourceId(0), src_t: 0, seek: None, gain_cdb: 0, pan_cq8, pan_to: pan_cq8, env: [ENV_ONE, ENV_ONE] }
    }

    /// A stereo frame through `mix_into` at centre is the unpanned arithmetic exactly; hard left / right keep their own
    /// channel at +3.01 dB (x 92681 / 65536) and silence the other; mono ignores pan.
    #[test]
    fn striapan_mix_into_stereo_and_mono() {
        let pcm = [0.5f32, -0.25, 0.125, 0.75];
        let mix = |p: i8, ch: usize| {
            let mut acc = vec![0i64; pcm.len()];
            mix_into(&mut acc, &pcm, &layer(p), (pcm.len() / ch) as i64, ch);
            acc
        };
        let plain: Vec<i64> = pcm.iter().map(|&x| to_i16(x)).collect();
        assert_eq!(mix(0, 2), plain, "centre is unity");
        assert_eq!(pan_mix(0, 2), [65536, 65536]);
        assert_eq!(pan_mix(127, 2), [0, 92681]);
        assert_eq!(pan_mix(-128, 2), [92681, 0]);
        let g = |x: i64| (x * (65536 * 92681 >> 16) * 65536) >> 32;
        assert_eq!(mix(-127, 2), vec![g(plain[0]), 0, g(plain[2]), 0]);
        assert_eq!(mix(127, 2), vec![0, g(plain[1]), 0, g(plain[3])]);
        assert_eq!(mix(127, 1), plain, "a mono render ignores pan");
        assert_eq!(mix(-128, 1), plain);
        println!("striapan: stereo hard right {:?}, centre {:?}", mix(127, 2), mix(0, 2));
    }
}

// ---- STRIANORMALIZE (LEDGER SR878): one source range's exact peak through the audio road (named tail block) ----

/// STRIANORMALIZE (SR878): the exact peak (max |sample| over every channel, NaN ignored, NOT clamped at full scale) of
/// source `id` over source time `ns` (nanoseconds from the source's origin), at the decoder's own rate: samples
/// `samples(ns.start, rate) .. samples(ns.end, rate)`. The same road as [`scan_peaks`]: ONE [`Aud`] (opened at the audio
/// keyframe before `ns.start`, as `mix_run` opens one), `fill` soft to the stream's end and `drain` one second at a time (a
/// hole is silence). `Ok(None)`: the source has no audio stream. The face's `normalize` word reads it (SR827's u8 table
/// clamps at 1.0, rounds to nearest and straddles the clip's edges, so it cannot set a peak target).
pub fn peak_of(sources: &Sources, id: SourceId, ns: std::ops::Range<i64>) -> Result<Option<f32>, RenderError> {
    peak_over(sources, id, ns, 1) // STRIANORMALIZEMORE (SR963): the body moved to `peak_over` (file tail); os 1 is the sample peak
}

// ---- STRIANORMALIZEMORE (LEDGER SR963): the true (inter-sample) peak through the mix's one resampler (named tail block) ----

/// STRIANORMALIZEMORE (SR963): the oversampling factor of `normalize ... tp` (ITU-R BS.1770 Annex 2's 4x at 48 kHz).
pub const TRUE_PEAK_OS: u32 = 4;

/// STRIANORMALIZEMORE (SR963): SR878's [`peak_of`] loop (moved here, not copied), with `os` > 1 also streaming the same
/// chunks through ONE `resample::Sinc` (NLE4B's windowed sinc, the mix's own resampler) at `rate * os` and taking the
/// larger of the sample peak and the oversampled peak, so the true peak is never below the sample peak. `os` 1 is
/// exactly SR878's sample peak. The converter's history pre-rolls silence at the range start, so an edge can only
/// over-read (a normalize lands lower, never above its target).
pub fn peak_over(sources: &Sources, id: SourceId, ns: std::ops::Range<i64>, os: u32) -> Result<Option<f32>, RenderError> {
    peak_at(sources, id, ns, os, 0) // STRIANORMALIZETHREE (SR9035): the body moved to `peak_at` (file tail); out rate 0 is the source's own
}

/// STRIANORMALIZEMORE (SR963): the largest |sample| `s` yields for `pcm` (whole interleaved frames of `ch` channels, the
/// converter's own count), every output that did not fit one buffer drained by calling again; `end` flushes the stream
/// instead (`pcm` unused; the window's lookahead is silence). NaN ignored.
pub fn up_peak(s: &mut resample::Sinc, ch: usize, pcm: &[f32], end: bool) -> f32 {
    let ch = ch.max(1);
    let (from, to) = s.rates();
    let room = (pcm.len() / ch) * (to as usize).div_ceil(from.max(1) as usize) + 64;
    let mut buf = vec![0.0f32; room * ch];
    let mut peak = 0.0f32;
    let mut input = pcm;
    loop {
        let made = if end { s.flush(&mut buf) } else { s.process(input, &mut buf).1 };
        input = &[];
        peak = buf[..made * ch].iter().fold(peak, |m, &x| {
            let v = if x < 0.0 { -x } else { x };
            if v > m { v } else { m }
        });
        if made < room {
            break;
        }
    }
    peak
}

// ---- STRIANORMALIZETHREE (LEDGER SR9035): the peak at the rate the mix plays, through the one peak body (named tail block) ----

/// STRIANORMALIZETHREE (SR9035): SR963's [`peak_over`] body (moved here, not copied) with the converter's rate
/// `to * os`, `to` = `out_rate` (0: the source's own rate). The one `resample::Sinc` (the mix's own, as `Aud::take_at`
/// builds it) runs only when that rate differs from the source's, so `out_rate` 0 is SR963's `peak_over` exactly and
/// `out_rate` 0 with `os` 1 is SR878's sample peak exactly. The sample peak is always folded too and the larger kept, so
/// a normalize never lands above its target (a downsample can only lower the peak: conservative).
pub fn peak_at(sources: &Sources, id: SourceId, ns: std::ops::Range<i64>, os: u32, out_rate: u32) -> Result<Option<f32>, RenderError> {
    let m = sources.get(id)?;
    let Some(i) = m.audio else { return Ok(None) };
    let ch = m.demux.tracks()[i].channels.max(1);
    let key = sources.keyframe_ns(id, Kind::Audio, ns.start.max(0)).ok_or(RenderError::NoStream(id, Kind::Audio))?;
    let mut a = Aud::open(m, id, key, ch)?;
    let rate = a.rate.max(1);
    let to = (if out_rate == 0 { rate } else { out_rate }).saturating_mul(os.max(1));
    let mut up = (to != rate).then(|| resample::Sinc::new(rate, to, ch as usize));
    let (s0, s1) = (samples(ns.start.max(0), rate), samples(ns.end.max(0), rate));
    let mut peak = 0.0f32;
    let mut at = s0;
    while at < s1 {
        let n = (rate as i64).min(s1 - at);
        a.fill(at + n, true)?;
        let pcm = a.drain(at, n);
        if pcm.is_empty() {
            break;
        }
        peak = pcm.iter().fold(peak, |m, &x| {
            let v = if x < 0.0 { -x } else { x };
            if v > m { v } else { m }
        });
        if let Some(s) = up.as_mut() {
            peak = peak.max(up_peak(s, ch as usize, &pcm, false));
        }
        at += n;
    }
    if let Some(s) = up.as_mut() {
        peak = peak.max(up_peak(s, ch as usize, &[], true));
    }
    Ok(Some(peak))
}

// ---- STRIAIMPORTIMAGESEQUENCE (LEDGER SR888): a PNG sequence as a video reel (named tail block) ----
/// STRIAIMPORTIMAGESEQUENCE (LEDGER SR888): a reel whose `--src` path is a printf pattern (`<dir>/frame_%04d.png`: one `%d`
/// or `%0Nd`, an image suffix) is a sequence of image files, one per timeline tick: source tick k is file number `first + k`
/// (the timeline's rate). [`Sources::add_sequence`] reads the directory's names once and the first file's header
/// (pixel_core's ONE header reader `facts_of`) for the reel size; the render reads and decodes ONE file per tick through
/// pixel_core's `decode_png`, never the whole sequence. A number between the first and the last with no file is refused,
/// by name, when its tick is reached. The EDL names the reel only; the pattern lives in the loader's `--src`. Immutable
/// after the add: no lock.
struct Seq {
    prefix: String,
    width: usize,
    suffix: String,
    first: u64,
    last: u64,
    w: u32,
    h: u32, held: SeqHeld, rate: SeqRate, // STRIASEQREPORT (SR943): its own frames a second (file tail); STRIASEQINFO (SR940): the last decoded frame (file tail)
}

impl Seq {
    /// STRIAIMPORTIMAGESEQUENCE: the path of file number `n`.
    fn path(&self, n: u64) -> String {
        format!("{}{:0w$}{}", self.prefix, n, self.suffix, w = self.width)
    }
    /// STRIAIMPORTIMAGESEQUENCE: the reel's length in ticks, one per file number `first..=last`.
    fn len(&self) -> i64 {
        self.ticks(self.last - self.first + 1) // STRIASEQREPORT (SR943): at its own rate when set (file tail)
    }
}

/// STRIAIMPORTIMAGESEQUENCE: `path` as an image-sequence pattern: `(prefix, width, suffix)` around its one `%d` / `%0Nd`
/// (width 0 for `%d`), no other `%` after it; `None` when it is no such pattern (a plain media path). The suffix's
/// format is [`Sources::add_sequence`]'s rule (STRIASEQMORE `seq_format`), so a `.tif` pattern is refused by name there, not read as a file.
pub fn seq_pattern(path: &str) -> Option<(&str, usize, &str)> {
    let i = path.find('%')?;
    let rest = &path[i + 1..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let spec = &rest[..digits];
    if rest.as_bytes().get(digits) != Some(&b'd') || (!spec.is_empty() && !spec.starts_with('0')) {
        return None;
    }
    let width = if spec.is_empty() { 0 } else { spec.parse().ok()? };
    let suffix = &rest[digits + 1..];
    if suffix.contains('%') {
        return None;
    }
    Some((&path[..i], width, suffix))
}

impl Sources {
    /// STRIAIMPORTIMAGESEQUENCE: open the image sequence `pattern` ([`seq_pattern`]) as source `id` (whatever `id` held is
    /// forgotten). Refused by name (`RenderError::Demux`, `image sequence <pattern>: <why>`): no `%d` / `%0Nd`, a suffix other
    /// than `.png`, an unreadable directory, no file matching, a first file with no PNG header. Logs
    /// `[STRIA] :: nle seq reel=<id> frames=<first>..=<last> <w>x<h>`.
    pub fn add_sequence(&mut self, id: SourceId, pattern: &str) -> Result<(), RenderError> {
        use std::io::Read;
        let refuse = |why: String| RenderError::Demux(format!("image sequence {pattern}: {why}"));
        let (prefix, width, suffix) = seq_pattern(pattern).ok_or_else(|| refuse(String::from("no %d or %0Nd in the path")))?; let (suffix, rate) = seq_rate(suffix).map_err(refuse)?; let rate = rate.or_else(|| self.seq_doc_rate(id)); // STRIASEQRATEEDL (SR9053): the pattern's `@R` wins, else the document's rate (file tail); STRIASEQREPORT (SR943): a trailing `@<rate>` (file tail)
        if seq_format(suffix).is_none() { // STRIASEQMORE (SR978): the suffix names one of pixel_core's formats (file tail)
            return Err(refuse(String::from("only .png, .jpg, .jpeg, .gif, .bmp, .qoi or .webp frames (pixel_core's decoders)")));
        }
        let (dir, stem) = match prefix.rfind('/') {
            Some(k) => (&prefix[..=k], &prefix[k + 1..]),
            None => ("", prefix),
        };
        let entries = std::fs::read_dir(if dir.is_empty() { "." } else { dir }).map_err(|e| refuse(format!("{e}")))?;
        let (mut first, mut last) = (u64::MAX, 0u64);
        for e in entries.flatten() {
            let Ok(name) = e.file_name().into_string() else { continue };
            let Some(n) = timeline_core::seq::number(stem, width, suffix, &name) else { continue }; (first, last) = (first.min(n), last.max(n)); // STRIAKSEQ (SR9065): the name rule is timeline_core's, the Player's too
        }
        if first == u64::MAX {
            return Err(refuse(String::from("no file matches")));
        }
        let seq = Seq { prefix: String::from(prefix), width, suffix: String::from(suffix), first, last, w: 0, h: 0, held: Default::default(), rate: seq_rate_tick(rate, self.tb).map_err(refuse)? }; // STRIASEQINFO (SR940): nothing kept yet; STRIASEQREPORT (SR943): the rate against the timeline's tick; STRIASEQMORE (SR978): a fractional rate folds into it (file tail)
        let p = seq.path(first);
        let mut head = Vec::new();
        std::fs::File::open(&p).and_then(|f| f.take(SEQ_HEAD).read_to_end(&mut head)).map_err(|e| refuse(format!("{p}: {e}")))?; // STRIASEQMORE (SR978): a head long enough for a JPEG's APP segments (file tail)
        let png = seq_format(suffix).is_some_and(|q| gneiss_pal::dsp::image::sniff(&head) == Some(q.0)); // STRIASEQMORE (SR978): the head is the suffix's format (file tail)
        let f = gneiss_pal::dsp::image::facts_of(&head).filter(|f| png && f.width > 0 && f.height > 0).ok_or_else(|| refuse(format!("{p}: not a {} header", seq_format(suffix).map_or("PNG", |q| q.1))))?; // STRIASEQMORE (SR978): named by the suffix's format
        log::info!("[STRIA] :: nle seq reel={} frames={}..={} {}x{}", id.0, first, last, f.width, f.height);
        self.media.retain(|(s, _)| *s != id);
        self.seqs.retain(|q| q.0 != id);
        if let Ok(mut v) = self.stand_in.lock() {
            v.retain(|s| *s != id);
        }
        self.seqs.push((id, Seq { w: f.width, h: f.height, ..seq }.upright(&p))); // STRIASEQTHREE (SR9022): a JPEG reel's size taken upright (file tail)
        Ok(())
    }
    /// STRIAIMPORTIMAGESEQUENCE: source `id`'s sequence, when it is one.
    fn seq(&self, id: SourceId) -> Option<&Seq> {
        self.seqs.iter().find(|q| q.0 == id).map(|q| &q.1)
    }
}

/// STRIAIMPORTIMAGESEQUENCE: [`render`]'s sizing loop: `true` when `src` is a sequence (the step has no `Media` to read),
/// after sizing the frame from the sequence's first file when a video step finds the frame unsized.
fn seq_size(sources: &Sources, src: SourceId, kind: Kind, r: &mut RenderReport) -> bool {
    let Some(q) = sources.seq(src) else { return false };
    if kind == Kind::Video && r.width == 0 {
        (r.width, r.height) = (q.w, q.h);
    }
    true
}

/// STRIAIMPORTIMAGESEQUENCE: the frame of sequence source `src` at source tick `src_t` (`Ok(None)`: not a sequence): file
/// `first + src_t` read and decoded by `decode_png` as an RGBA `Frame`. The live decode run is dropped (parked), as the
/// title arm does, so the next media step opens by its seek. A tick past the last file is [`RenderError::Short`]; a hole
/// `image sequence reel=<id> frame <n> missing (<path>)`; a bad file [`RenderError::Decode`].
fn seq_frame(tl: &Timeline, sources: &Sources, run: &mut Option<Vid>, src: SourceId, src_t: i64) -> Result<Option<Frame>, RenderError> {
    let Some(q) = sources.seq(src) else { return Ok(None) };
    *run = None;
    let n = u64::try_from(src_t).ok().and_then(|k| q.file_at(k)).filter(|&n| n <= q.last).ok_or(RenderError::Short(src))?; // STRIASEQREPORT (SR943): tick to file at the reel's rate (file tail)
    if let Some(f) = q.held_frame(n, q.frame_in(src_t, n), tl.tb.to_ns(src_t)) { return Ok(Some(f)); } let path = q.path(n); let stamp = seq_stamp(&path); // STRIASEQRATEEDL (SR9053): the frame of an animated file at this tick (file tail); STRIASEQTHREE (SR9022): the file's stamp before its read (file tail); STRIASEQINFO (SR940): a repaint of the kept frame reads no file (file tail)
    let bytes = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => RenderError::Demux(format!("image sequence reel={} frame {n} missing ({path})", src.0)),
        _ => RenderError::Demux(format!("image sequence reel={} frame {n} ({path}): {e}", src.0)),
    })?;
    let img = timeline_core::seq::decode(&bytes).map_err(|e| RenderError::Decode(format!("image sequence reel={} frame {n} ({path}): {e:?}", src.0)))?; // STRIAKSEQ (SR9065): timeline_core's upright decode, the Player's too; STRIASEQTHREE (SR9022): turned upright by its EXIF orientation (file tail); STRIASEQMORE (SR978): the sniffing decode, every format seq_format names
    Ok(Some(q.keep(n, img.width, img.height, seq_canvases(img.frames.take(), img.rgba), q.frame_in(src_t, n), tl.tb.to_ns(src_t), stamp))) // STRIASEQRATEEDL (SR9053): every canvas of an animated file kept, this tick's handed on (file tail); STRIASEQTHREE (SR9022): kept with its stamp (file tail); STRIASEQINFO (SR940): kept as the reel's one frame (file tail)
}

// ---- STRIASEQINFO (LEDGER SR940): a sequence reel's line and its kept frame (named tail block) ----
/// STRIASEQINFO (LEDGER SR940): a sequence reel's ONE kept frame `(file number, w, h, RGBA)`, owned by its [`Seq`] (so keyed by
/// reel, then by number; a re-add drops it with the `Seq`). Locked for one take or one put, never across a read, a decode or
/// the RGBA copy.
type SeqHeld = std::sync::Mutex<SeqKept>; // STRIASEQTHREE (SR9022): a bounded set of kept frames, each with its file stamp (file tail)

impl Seq {
    /// STRIASEQINFO: the reel's person's line for [`Sources::summary`]: `seq <first>..=<last> <w>x<h>`.
    fn summary(&self) -> String {
        format!("seq {}..={} {}x{}{}", self.first, self.last, self.w, self.h, self.rate.map(seq_rate_text).unwrap_or_default()) // STRIASEQREPORT (SR943): ` @<rate>` only when set; STRIASEQMORE (SR978): ` @n/d` for a fractional one
    }
    /// STRIASEQINFO: the kept frame when it is file number `n` (a repaint of the same tick reads no file).
    fn held_frame(&self, n: u64, i: usize, pts_ns: i64) -> Option<Frame> { // STRIASEQRATEEDL (SR9053): `i` the canvas of an animated file (file tail)
        let st = seq_stamp(&self.path(n)); let (w, h, rgba) = self.held.lock().ok().and_then(|mut g| g.get(n, st))?; // STRIASEQTHREE (SR9022): the stat before the lock; a kept frame of a changed or removed file is not served (file tail)
        Some(Frame { width: w, height: h, pts_ns, pixels: gneiss_pal::dsp::video::Pixels::Rgba(seq_pick(&rgba, w, h, i)) }) // STRIASEQRATEEDL (SR9053): canvas `i`, the last held (file tail)
    }
    /// STRIASEQINFO: keep file number `n`'s decoded RGBA as the reel's one frame (the old one dropped) and hand it on as a `Frame`.
    fn keep(&self, n: u64, w: u32, h: u32, rgba: Vec<u8>, i: usize, pts_ns: i64, stamp: SeqStamp) -> Frame { // STRIASEQRATEEDL (SR9053): `rgba` every canvas back to back, `i` the one handed on (file tail); STRIASEQTHREE (SR9022): the stamp taken before the read (file tail)
        let kept = Arc::new(rgba.clone());
        if let Ok(mut g) = self.held.lock() {
            g.put(n, w, h, kept, stamp); // STRIASEQTHREE (SR9022): one of several kept frames (file tail)
        }
        Frame { width: w, height: h, pts_ns, pixels: gneiss_pal::dsp::video::Pixels::Rgba(seq_pick_vec(rgba, w, h, i)) } // STRIASEQRATEEDL (SR9053): canvas `i` (a still: its own bytes, no copy) (file tail)
    }
}

// ---- STRIAPROXYMEDIA (LEDGER SR882): a source's preview proxy (named tail block) ----
/// STRIAPROXYMEDIA (LEDGER SR882): why [`Sources::set_proxy`] refused a proxy. Each refusal by name; the face prints it
/// after `proxy: <reel>: `.
#[derive(Debug)]
pub enum ProxyError {
    /// The proxy's bytes did not open (or the original `id` is not a source: `RenderError::NoSource`).
    Open(RenderError),
    /// The proxy has no video stream.
    NoVideo,
    /// The proxy's video length (whole ticks) differs from the original's by more than one tick.
    Stale { proxy: i64, source: i64 },
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxyError::Open(e) => write!(f, "proxy open: {e}"),
            ProxyError::NoVideo => write!(f, "no video stream"),
            ProxyError::Stale { proxy, source } => write!(f, "stale (proxy {proxy} ticks, source {source})"),
        }
    }
}
impl std::error::Error for ProxyError {}

/// STRIAPROXYMEDIA (LEDGER SR882): a source may carry ONE attached proxy (a smaller encode of the same frames at the same
/// source times). [`render`] opens a video run on it only when its sink says [`RenderSink::proxy`] (the face's preview and
/// played still); an export's sink never does, so an export reads the original. The slot is `&self` behind a Mutex (the
/// face's `Arc<Sources>` is shared with a running export), locked for one find + clone or one push, never across a decode.
impl Sources {
    /// Attach `bytes` as source `id`'s proxy (replacing any earlier one). Refused by name: an unknown `id` or bytes that do not
    /// open ([`ProxyError::Open`]), no video stream ([`ProxyError::NoVideo`]), a video length off the original's by more
    /// than one tick ([`ProxyError::Stale`]).
    pub fn set_proxy(&self, id: SourceId, bytes: Vec<u8>) -> Result<(), ProxyError> {
        let source = self.video_ticks(id).ok_or(ProxyError::Open(RenderError::NoSource(id)))?;
        let m = Media::open(bytes).map_err(ProxyError::Open)?;
        let proxy = self.media_video_ticks(&m).ok_or(ProxyError::NoVideo)?;
        if (proxy - source).abs() > 1 {
            return Err(ProxyError::Stale { proxy, source });
        }
        if let Ok(mut v) = self.proxy.lock() {
            v.retain(|(s, _)| *s != id);
            v.push((id, Arc::new(m)));
        }
        Ok(())
    }
    /// Source `id` has an attached proxy.
    pub fn has_proxy(&self, id: SourceId) -> bool {
        self.proxy.lock().map(|v| v.iter().any(|(s, _)| *s == id)).unwrap_or(false)
    }
    /// Source `id`'s video stream length in whole ticks (the span a proxy of it renders); `None` without a video stream.
    pub fn video_ticks(&self, id: SourceId) -> Option<i64> {
        self.media_video_ticks(self.get(id).ok()?)
    }
    /// The byte length of source `id`'s file as it was opened.
    pub fn byte_len(&self, id: SourceId) -> Option<u64> {
        Some(self.get(id).ok()?.demux.bytes().len() as u64)
    }
    /// `m`'s video track duration in whole ticks ([`SourceIndex::duration`]'s rounding, the video stream alone).
    fn media_video_ticks(&self, m: &Media) -> Option<i64> {
        let ns = m.demux.tracks()[m.video?].duration_ns;
        Some(((ns as i128 + 1_000) * self.tb.den as i128 / (self.tb.num.max(1) as i128 * 1_000_000_000)) as i64)
    }
    /// The run [`vid_frame`] opens for `id`: its attached proxy when `want` (the sink's [`RenderSink::proxy`]) and one is
    /// attached, else `None` (the original through `get`).
    fn proxied(&self, id: SourceId, want: bool) -> Option<Arc<Media>> {
        if !want {
            return None;
        }
        self.proxy.lock().ok()?.iter().find(|(s, _)| *s == id).map(|(_, m)| Arc::clone(m))
    }
}

// ---- STRIAPOSITIONSCALE (LEDGER SR879): a placed clip's frame over the track below (named tail block) ----
/// The RGBA a video step shows when its clip is placed (`Clip::xform`, picture-in-picture): the step's own frame `b`
/// (`w x h`, uncropped: `cropped` leaves a placed clip's frame alone) with its crop, placed by its reel's policy into the
/// placement's rect over the lower frame `a` (the schedule's outgoing: the track below, or black) at the step's mix weight,
/// by timeline_core's `Timeline::place_step` (STRIAKXFORM SR9011: the rule the kernel Player calls too). `None` when the step's clip is not placed (the mix as before).
fn xformed(tl: &Timeline, s: &timeline_core::schedule::Step, a: &[u8], b: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    Some(tl.place_step(s, a.as_chunks::<4>().0, b.as_chunks::<4>().0, w as usize, h as usize)?.into_flattened()) // STRIAKXFORM (LEDGER SR9011): the one placement rule, lifted into timeline_core
}

// ---- STRIAPROXYOFF (LEDGER SR915): detach a source's proxy (named tail block) ----
impl Sources {
    /// Detach source `id`'s proxy (the preview reads the original again); false when none was attached. One retain under
    /// the slot's lock; a render already running keeps its own `Arc` of the proxy until its run ends.
    pub fn clear_proxy(&self, id: SourceId) -> bool {
        let Ok(mut v) = self.proxy.lock() else { return false };
        let before = v.len();
        v.retain(|(s, _)| *s != id);
        v.len() != before
    }
}

// ---- STRIABINSCROLL (LEDGER SR939): what a reel's media holds, for the bin's unused-reel lay (named tail block) ----
impl Sources {
    /// Whether source `id`'s opened media holds a stream of `kind` (its demux tracks at open); false for an id with no
    /// media. The bin (`nle_face`) reads it to lay a reel that sits on no track yet on a track of a kind it can feed.
    pub fn holds(&self, id: SourceId, kind: Kind) -> bool {
        self.get(id).is_ok_and(|m| m.stream(kind).is_some())
    }
}

// ---- STRIASEQREPORT (LEDGER SR943): a sequence reel's open line and its own rate (named tail block) ----
/// STRIASEQREPORT (LEDGER SR943): a sequence reel's own rate `(frames a second R, tick num, tick den)` (the timeline's tick is
/// `num / den` seconds, taken from `Sources.tb` at the add); `None` = one file per timeline tick (the default). Set once at the
/// add, immutable: no lock.
type SeqRate = Option<(u64, u64, u64, u64)>; // STRIASEQMORE (SR978): (n, tick num, tick den * d, d) for a rate n/d

/// STRIASEQREPORT: split a trailing `@<rate>` off a sequence pattern's suffix (the part after its `%d` / `%0Nd`, so a `@` in a
/// directory name is untouched): `(suffix, Some(rate))`, `(suffix, None)` with no `@`. A rate that is not N, N/D or N.F frames
/// a second above 0 (`@0`, `@`, `@x`, `@-3`, `@1/0`; STRIASEQMORE SR978) is refused by name.
fn seq_rate(suffix: &str) -> Result<(&str, Option<(u64, u64)>), String> { // STRIASEQMORE (SR978): the rate as (n, d)
    let Some((head, r)) = suffix.rsplit_once('@') else { return Ok((suffix, None)) };
    match seq_rate_q(r) { // STRIASEQMORE (SR978): N, N/D or N.F (file tail)
        Some(q) => Ok((head, Some(q))),
        _ => Err(format!("rate \"{r}\" is not a frame rate above 0 (N, N/D or N.F)")),
    }
}

impl Seq {
    /// STRIASEQREPORT: the reel's [`OpenReport`] for [`Sources::open_report`]: `seq` set, so it prints `open=seq` alone (no
    /// demuxer: no index, no decode-time derivation, no edit list, no colour, no stand-in).
    fn report(&self) -> OpenReport {
        OpenReport { indexed: false, decode_time: DecodeTime::Presentation, edits: 0, applied: None, colour: None, stand_in: false, seq: true }
    }
    /// STRIASEQREPORT: the ticks `count` files last: `count` at one file per tick, else ceil(count * den / (R * num)), exact.
    fn ticks(&self, count: u64) -> i64 {
        let Some((r, num, den, _)) = self.rate else { return count as i64 };
        let per = r as i128 * num as i128;
        ((count as i128 * den as i128 + per - 1) / per) as i64
    }
    /// STRIASEQREPORT: the file number shown at source tick `k`: `first + k` at one file per tick, else
    /// `first + floor(k * R * num / den)`, exact (`None` on overflow).
    fn file_at(&self, k: u64) -> Option<u64> {
        let Some((r, num, den, _)) = self.rate else { return self.first.checked_add(k) };
        let off = k as u128 * r as u128 * num as u128 / den as u128;
        self.first.checked_add(u64::try_from(off).ok()?)
    }
}

// ---- STRIAGRADEMORE (LEDGER SR957): the grade on the source frame, before the fit (named tail block) ----
// NLE-GRADE-BARS: [`reel_rgba`] takes the step's clip grade and passes the source frame through SR869's [`graded`] BEFORE
// the size policy fits it, so a size switch's Letterbox bars (the FIELD fill painted after) are never graded; Fill / Stretch /
// Adopt get the graded source. Its two callers (the step's own frame and a transition's outgoing frame) pass
// `Timeline::grade_at`; an IDENTITY grade is no pass, so every ungraded render is byte-identical.

// ---- STRIALOUDNESSMORE (LEDGER SR961): the measure decodes no video (named tail block) ----
// STRIALOUDNESSMORE (SR961): `LoudnessSink` answers [`RenderSink::frame_wanted`] false for every video step (raising
// SR903's ticks-done count first), so [`render`] decodes, converts and letterboxes no frame for a loudness measure; its
// audio path (`mix_run` over its own `auds` slots) never reads the video state, so the PCM, and the LUFS, are unchanged.
// Every other sink keeps the default `true`. Owed: NLE-LOUDNESS-AUDIOONLY-NOSIZE (render's `NoSize` check, before the step
// loop, still refuses a measure whose video steps have no sized reel).

/// STRIANORMALIZETHREE (SR9035): source `id`'s peak over `ns` at the rate the mix plays it ([`Sources::set_audio_rate`]'s
/// project rate, 0 = the source's own), `os` times oversampled: an overshoot the mix's resample builds is seen.
pub fn peak_mix(sources: &Sources, id: SourceId, ns: std::ops::Range<i64>, os: u32) -> Result<Option<f32>, RenderError> {
    peak_at(sources, id, ns, os, sources.audio_rate)
}

// ---- STRIAPROXYMORE (LEDGER SR979): an Adopt reel's proxy up-fits (named tail block) ----
/// STRIAPROXYMORE (LEDGER SR979): the policy [`reel_rgba`] paints a frame by. A run whose track size is not the reel's is a
/// proxied run (SR882: the proxy is a scaled stand-in of frames at the reel's size): Adopt fits it as Letterbox, so the
/// preview shows the reel's size, not the proxy's. An original's run (its track IS the reel's) keeps `fit` unchanged.
fn proxy_fit(fit: SizePolicy, run: Option<&Track>, reel: (u32, u32)) -> SizePolicy {
    if fit == SizePolicy::Adopt && run.is_some_and(|t| (t.width, t.height) != reel) { SizePolicy::Letterbox } else { fit }
}

// ---- STRIASEQMORE (LEDGER SR978): sequence formats, fractional rates, the title size (named tail block) ----
/// STRIASEQMORE (LEDGER SR978): the bytes [`Sources::add_sequence`] reads of a sequence's first file for its header: long enough
/// for a JPEG whose EXIF APP1 (up to 64 KiB) and ICC APP2 segments sit before the SOF that [`gneiss_pal::dsp::image::facts_of`]
/// walks to. One bounded read.
const SEQ_HEAD: u64 = 256 * 1024;

/// STRIASEQMORE: the ONE table of sequence frame formats: the pattern's suffix (case-insensitive) names the pixel_core format
/// its files must sniff as, and that format's name for the refusal (`<p>: not a <NAME> header`). `None`: not a sequence format
/// (TIFF / raw, SVG and everything else are refused at the suffix).
fn seq_format(suffix: &str) -> Option<(gneiss_pal::dsp::image::Format, &'static str)> {
    timeline_core::seq::format(suffix) // STRIAKSEQ (SR9065): the table is timeline_core's, the Player's too
}

/// STRIASEQMORE: a sequence rate's text as `(n, d)` frames a second, reduced: `N` (d 1), `N/D` (D above 0) or `N.F` (F one to
/// nine digits: N*10^k+F over 10^k). Digits only (no sign, no space); `None` for anything else or a rate of 0.
fn seq_rate_q(r: &str) -> Option<(u64, u64)> {
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let (n, d) = if let Some((a, b)) = r.split_once('/') {
        if !digits(a) || !digits(b) {
            return None;
        }
        (a.parse::<u64>().ok()?, b.parse::<u64>().ok()?)
    } else if let Some((a, b)) = r.split_once('.') {
        if !digits(a) || !digits(b) || b.len() > 9 {
            return None;
        }
        let scale = 10u64.pow(b.len() as u32);
        (a.parse::<u64>().ok()?.checked_mul(scale)?.checked_add(b.parse::<u64>().ok()?)?, scale)
    } else {
        if !digits(r) {
            return None;
        }
        (r.parse::<u64>().ok()?, 1)
    };
    if n == 0 || d == 0 {
        return None;
    }
    let (mut x, mut y) = (n, d);
    while y != 0 {
        (x, y) = (y, x % y);
    }
    Some((n / x, d / x))
}

/// STRIASEQMORE: the rate `(n, d)` against the timeline's tick `tb` as [`SeqRate`] `(n, num, den * d, d)`: the tick formulas of
/// `Seq::ticks` / `Seq::file_at` (R = n over a tick of num / (den * d)) stay SR943's. Refused by name when `den * d` overflows.
fn seq_rate_tick(rate: Option<(u64, u64)>, tb: Timebase) -> Result<SeqRate, String> {
    let Some((n, d)) = rate else { return Ok(None) };
    let den = tb.den.max(1).checked_mul(d).ok_or_else(|| format!("rate {n}/{d} is too fine for the timeline's tick"))?;
    Ok(Some((n, tb.num.max(1), den, d)))
}

/// STRIASEQMORE: the summary's rate text: ` @n` for a whole rate (SR943's text), ` @n/d` for a fractional one.
fn seq_rate_text(r: (u64, u64, u64, u64)) -> String {
    if r.3 == 1 { format!(" @{}", r.0) } else { format!(" @{}/{}", r.0, r.3) }
}

// ---- STRIALOUDNESSNEXT (LEDGER SR995): an audio-only measure under an empty video track; loudness range and true peak (named tail block) ----
// STRIALOUDNESSNEXT (SR995): `render`'s `NoSize` refusal (before the step loop) now asks the sink's `frame_wanted` for
// the first video step, so a sink that declines it (the loudness sink declines every frame, SR961) renders its audio
// under a video track holding only gaps; a sink that declined the first and wants a later frame with nothing sized is
// refused at that step (never a 0x0 frame). Every frame-taking sink (export, preview, the meters) is refused as before.
// The measure reads, off the one `Loudness` the sink fed, the integrated loudness, the EBU 3342 loudness range and the
// BS.1770 true peak (audio_core meter.rs); `loudness` / `loudness_watched` keep their LUFS-only answer.

/// STRIALOUDNESSNEXT (SR995): one measure's three readings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessReading {
    /// BS.1770 integrated loudness, LUFS.
    pub lufs: f64,
    /// EBU Tech 3342 loudness range, LU.
    pub range_lu: f64,
    /// BS.1770 true peak, dBTP.
    pub true_peak_db: f64,
}

/// STRIALOUDNESSNEXT (SR995): THE measure body: [`loudness_watched`]'s render through the PCM-only sink, read as all
/// three values; `None` when the range sent no PCM.
pub fn loudness_reading(tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources, watch: Option<LoudnessWatch>) -> Result<Option<LoudnessReading>, RenderError> {
    let mut sink = LoudnessSink { l: None, watch };
    render(tl, range, sources, &mut sink)?;
    Ok(sink.l.map(|l| LoudnessReading { lufs: l.integrated(), range_lu: l.range_lu(), true_peak_db: l.true_peak_db() }))
}

// ---- STRIASEQTHREE (LEDGER SR9022): sequence cache span and staleness, upright frames (named tail block) ----
/// STRIASEQTHREE (LEDGER SR9022): a sequence file's stamp `(length, modified)` from one `std::fs::metadata` (no read); `None`
/// when the file is gone or its filesystem keeps no modified time (such a frame is never served from the cache).
type SeqStamp = Option<(u64, std::time::SystemTime)>;

/// STRIASEQTHREE: the stamp of the file at `path`, taken before its read and again before a kept frame is served.
fn seq_stamp(path: &str) -> SeqStamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// STRIASEQTHREE: at most this many kept frames per sequence reel (a short scrub back and forth reads no file).
const SEQ_KEPT: usize = 8;
/// STRIASEQTHREE: at most this many bytes of kept RGBA per sequence reel (the newest frame is kept whatever its size).
const SEQ_KEPT_BYTES: usize = 128 * 1024 * 1024;

/// STRIASEQTHREE: a sequence reel's kept frames `(file number, w, h, RGBA, stamp)`, least recently used first; owned by its
/// [`Seq`] behind SR940's one Mutex (`SeqHeld`), locked for one `get` or one `put`, never across a stat, read or decode.
#[derive(Default)]
struct SeqKept(Vec<(u64, u32, u32, Arc<Vec<u8>>, SeqStamp)>);

impl SeqKept {
    /// STRIASEQTHREE: file number `n`'s kept frame when its stamp is `st` (taken just before); a hit moves to the back, a
    /// kept frame whose file changed or is gone is dropped and not served.
    fn get(&mut self, n: u64, st: SeqStamp) -> Option<(u32, u32, Arc<Vec<u8>>)> {
        let i = self.0.iter().position(|e| e.0 == n)?;
        let e = self.0.remove(i);
        if st.is_none() || e.4 != st {
            return None;
        }
        let out = (e.1, e.2, e.3.clone());
        self.0.push(e);
        Some(out)
    }
    /// STRIASEQTHREE: keep file number `n`'s frame (replacing an older copy of it) at the back, then drop from the front while
    /// more than [`SEQ_KEPT`] frames or [`SEQ_KEPT_BYTES`] bytes are kept, the newest always staying.
    fn put(&mut self, n: u64, w: u32, h: u32, rgba: Arc<Vec<u8>>, stamp: SeqStamp) {
        self.0.retain(|e| e.0 != n);
        self.0.push((n, w, h, rgba, stamp));
        while self.0.len() > 1 && (self.0.len() > SEQ_KEPT || self.0.iter().map(|e| e.3.len()).sum::<usize>() > SEQ_KEPT_BYTES) {
            self.0.remove(0);
        }
    }
}

impl Seq {
    /// STRIASEQTHREE: the reel with its size upright: a `.jpg` / `.jpeg` reel decodes its first file `p` whole (pixel_core's
    /// `decode` reads the EXIF orientation; its header reader does not) and an orientation 5..=8 swaps `w` and `h`. Any other
    /// format, or a first file that does not decode (its tick refuses it by name), keeps the header size.
    fn upright(self, p: &str) -> Seq {
        if seq_format(&self.suffix).map(|q| q.0) != Some(gneiss_pal::dsp::image::Format::Jpeg) {
            return self;
        }
        let o = std::fs::read(p).ok().and_then(|b| gneiss_pal::dsp::image::decode(&b).ok()).map_or(1, |i| i.orientation);
        if (5..=8).contains(&o) { Seq { w: self.h, h: self.w, ..self } } else { self }
    }
}

#[cfg(test)]
mod striaseqthree_kept {
    use super::{Arc, SEQ_KEPT, SEQ_KEPT_BYTES, SeqKept, SeqStamp};

    fn st(len: u64) -> SeqStamp {
        Some((len, std::time::SystemTime::UNIX_EPOCH))
    }

    #[test]
    fn striaseqthree_kept_evicts_least_recent_by_count() {
        let mut k = SeqKept::default();
        for n in 0..SEQ_KEPT as u64 {
            k.put(n, 1, 1, Arc::new(vec![n as u8; 4]), st(n));
        }
        assert!(k.get(0, st(0)).is_some(), "file 0 is touched, so file 1 is now the least recent");
        k.put(99, 1, 1, Arc::new(vec![9; 4]), st(99));
        assert_eq!(k.0.len(), SEQ_KEPT);
        assert!(k.get(1, st(1)).is_none(), "the least recent went first");
        assert!(k.get(0, st(0)).is_some() && k.get(99, st(99)).is_some());
    }

    #[test]
    fn striaseqthree_kept_evicts_by_bytes_keeps_newest() {
        let mut k = SeqKept::default();
        let half = SEQ_KEPT_BYTES / 2 + 1;
        k.put(1, 1, 1, Arc::new(vec![0; half]), st(1));
        k.put(2, 1, 1, Arc::new(vec![0; half]), st(2));
        assert_eq!(k.0.iter().map(|e| e.0).collect::<Vec<_>>(), vec![2], "two halves overflow the budget: the older goes");
        k.put(3, 1, 1, Arc::new(vec![0; SEQ_KEPT_BYTES + 1]), st(3));
        assert_eq!(k.0.iter().map(|e| e.0).collect::<Vec<_>>(), vec![3], "the newest stays whatever its size");
    }

    #[test]
    fn striaseqthree_kept_stale_stamp_misses_and_drops() {
        let mut k = SeqKept::default();
        k.put(5, 2, 2, Arc::new(vec![7; 16]), st(10));
        assert!(k.get(5, st(11)).is_none(), "a rewritten file (other stamp) is not served");
        assert!(k.get(5, st(10)).is_none(), "and its stale frame was dropped");
        k.put(5, 2, 2, Arc::new(vec![7; 16]), st(10));
        assert!(k.get(5, None).is_none(), "a removed file (no stamp) is not served");
        k.put(6, 2, 2, Arc::new(vec![7; 16]), None);
        assert!(k.get(6, None).is_none(), "a frame kept without a stamp is never served");
    }
}

// ---- STRIAFREEZETHREE (LEDGER SR9029): the one title raster — lines and a style (named tail block) ----
// THE title raster (`title_still` and `title_still_cached` are its default-style calls). The text's LF-separated lines (a
// trailing CR dropped) are shaped at `h / 10 * (size / 10)` px (8 at least; the default size 10 is a factor of exactly 1.0,
// so the float is STRIATITLECLIP's), shrunk once so the widest line fits 90% of the width (STRIATITLECLIP's expression)
// and again so the block fits 90% of the height, stacked one line box apart around the frame's middle (one line: the
// baseline STRIATITLECLIP drew, by the same expression), each placed left (5% in), centred, or right (5% in), in the
// style's ink. So a one-line title at `TitleStyle::DEFAULT` is byte-identical to the still SR828 drew.

/// STRIAFREEZETHREE: `text` as a `w` x `h` RGBA still in `style` (see the block comment).
pub fn title_card(text: &str, style: &timeline_core::TitleStyle, w: u32, h: u32) -> timeline_core::view::Still {
    use text_host::line::{PixelOrder, TextStyle};
    let mut rgba = [0u8, 0, 0, 255].repeat(w as usize * h as usize);
    let lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let mut ts = TextStyle::new(&["sans"], (h as f32 / 10.0 * (style.size as f32 / 10.0)).max(8.0));
    // STRIAKTITLE (SR9009): size, shrink and centring are timeline_core's `view::title_fit` (the kernel Player's too)
    let (px, x, baseline) = timeline_core::view::title_fit(w, h, &mut |px| {
        style.size = px;
        let l = style.shape(text);
        (l.width, l.ascent, l.descent)
    });
    style.size = px;
    let shaped: Vec<text_host::line::Line> = lines.iter().map(|l| ts.shape(l)).collect();
    let widest = |v: &[text_host::line::Line]| v.iter().map(|l| l.width).fold(0.0f32, f32::maxlet wide = widest(&shaped);
    widetstswideshapedlines.iter().map(|l| tsl)).collect(block = |v: &[text_host::line::Line]| v.first().map_or(0.0, |l| l.height()) * v.len() as f32;
    let tall = block(&shaped);
    if tall > h as f32 * 0.9 {
        ts.size = (ts.size * h as f32 * 0.9 / tall).max(1.0);
        shaped = lines.iter().map(|l| ts.shape(l)).collect();
    }
    let Some(first) = shaped.first() else { return timeline_core::view::Still { width: w, height: h, rgba } };
    let (mid, step, n) = ((h as f32 + first.ascent - first.descent) / 2.0, first.height(), shaped.len());
    let ink = [style.ink[0], style.ink[1], style.ink[2], TITLE_INK[3]];
    for (i, line) in shaped.iter().enumerate() {
        let  match style.align {
            timeline_core::TitleAlign::Left => (w as f32 * 0.05).round(),
            timeline_core::TitleAlign::Centre =>,
            timeline_core::TitleAlign::Right => (w as f32 * 0.95 - line.width).round(),
        }
        mid - n - 1)*step + i as f32 * step);
        line.paint(&mut rgba, w, h, w as usize * 4, PixelOrder::Rgba, x, baseline, ink, (0, w as i32));
    }
    timeline_core::view::Still { width: w, height: h, rgba }
}

impl Sources {
    /// STRIAFREEZETHREE: [`title_card`] once per `(text, w, h, style)`; a second ask returns the same `Arc`.
    pub fn title_card_cached(&self, text: &str, style: &timeline_core::TitleStyle, w: u32, h: u32) -> Arc<timeline_core::view::Still> {
        let hit = self.titles.lock().ok().and_then(|c| c.iter().find(|e| e.0 == text && e.1 == w && e.2 == h && e.4 == *style).map(|e| e.3.clone()));
        if let Some(s) = hit {
            return s;
        }
        let s = Arc::new(title_card(text, style, w, h));
        if let Ok(mut c) = self.titles.lock() {
            if c.len() >= TITLE_CACHE_MAX {
                c.remove(0);
            }
            c.push((String::from(text), w, h, s.clone(), *style));
        }
        s
    }
}

// ---- STRIAPANAUTOMATION (LEDGER SR9050): the pan factor ramped between a run's two edges (named tail block) ----

/// STRIAPANAUTOMATION: channel `c`'s pan factor at frame `j` of a run of `n` frames: the ONE law's factor at the run's first
/// tick (`pw[0]`, from `Layer::pan_cq8`) ramped to the one at its end edge (`pw[1]`, `Layer::pan_to`) by the envelope's own
/// interpolation (`ramp::at`). Equal edges (every unautomated layer) give `pw[0][c]` exactly: byte-identical.
fn pan_frame(pw: &[[i64; 2]; 2], c: usize, j: i64, n: i64) -> i64 {
    gneiss_pal::dsp::audio::ramp::at(pw[0][c], pw[1][c], j, n)
}

#[cfg(test)]
mod striapanautomation_tests {
    use super::*;
    use timeline_core::SourceId;
    use timeline_core::schedule::ENV_ONE;

    /// A stereo layer panned from hard left at its first tick to hard right at its end edge ramps each channel's factor
    /// linearly over the run; a layer with equal edges mixes exactly as the static pan.
    #[test]
    fn striapanautomation_mix_into_ramps_between_edges() {
        let pcm = [0.5f32; 8];
        let l = Layer { track: 0, src: SourceId(0), src_t: 0, seek: None, gain_cdb: 0, pan_cq8: -127, pan_to: 127, env: [ENV_ONE, ENV_ONE] };
        let mut acc = vec![0i64; pcm.len()];
        mix_into(&mut acc, &pcm, &l, 4, 2);
        let (hi, x) = (92681i64, to_i16(0.5));
        let f = |a: i64, b: i64, j: i64| (x * (65536 * (a + (b - a) * j / 4) >> 16) * 65536) >> 32;
        let want: Vec<i64> = (0..4).flat_map(|j| [f(hi, 0, j), f(0, hi, j)]).collect();
        assert_eq!(acc, want);
        assert_eq!((acc[0], acc[1]), (f(hi, 0, 0), 0), "the first frame is the hard-left edge");
        let flat = Layer { pan_cq8: 64, pan_to: 64, ..l };
        let (mut a, mut b) = (vec![0i64; 8], vec![0i64; 8]);
        mix_into(&mut a, &pcm, &flat, 4, 2);
        let w = pan_mix(64, 2);
        for (j, fr) in pcm.chunks(2).enumerate() {
            for (c, &s) in fr.iter().enumerate() {
                b[j * 2 + c] += (to_i16(s) * (65536 * w[c] >> 16) * 65536) >> 32;
            }
        }
        assert_eq!(a, b, "equal edges are the static pan");
    }
}

// ---- STRIANORMALIZELUFS (LEDGER SR9054): one clip's loudness through the one measure (named tail block) ----
// `normalize lufs` reads the selected clip ALONE at unity: a clone of the timeline with every other clip, every
// transition and every audio crossfade taken out, the export marks cleared, and the one clip put back on its own track
// at gain 0 (its fades kept: the clip as it plays at unity), measured over the clip's own ticks by SR995's
// `loudness_reading`, the one measure body. No timeline literal is built: the clone keeps every field.

/// STRIANORMALIZELUFS (SR9054): clip `idx` of track `track` alone at unity gain, and the ticks it covers; `None` when
/// there is no such clip.
pub fn clip_alone(tl: &Timeline, track: usize, idx: usize) -> Option<(Timeline, std::ops::Range<i64>)> {
    let c = *tl.tracks.get(track)?.clips.get(idx)?;
    let mut alone = tl.clone();
    for t in alone.tracks.iter_mut() {
        t.clips.clear();
        t.transitions.clear();
    }
    alone.audio_xfades.clear();
    alone.marks = (None, None);
    alone.tracks[track].clips.push(timeline_core::Clip { gain_cdb: 0, ..c });
    Some((alone, c.at..c.end()))
}

/// STRIANORMALIZELUFS (SR9054): the integrated loudness, range and true peak of clip `idx` of track `track` alone at
/// unity ([`clip_alone`]) through [`loudness_reading`]; `Ok(None)` when there is no such clip or it sent no PCM.
pub fn loudness_clip(tl: &Timeline, track: usize, idx: usize, sources: &Sources, watch: Option<LoudnessWatch>) -> Result<Option<LoudnessReading>, RenderError> {
    let Some((alone, range)) = clip_alone(tl, track, idx) else { return Ok(None) };
    loudness_reading(&alone, range, sources, watch)
}

// ---- STRIASEQRATEEDL (LEDGER SR9053): the sequence rate in the document, animated files inside a sequence (named tail block) ----
/// STRIASEQRATEEDL (LEDGER SR9053): the document's sequence rates `(source, (n, d))`, from each source's `seq_rate`
/// (`* UNA SEQRATE`), recorded once by [`Sources::new`]; immutable after: no lock.
fn seq_doc_rates(tl: &Timeline) -> Vec<(SourceId, (u64, u64))> {
    tl.sources.iter().filter_map(|s| s.seq_rate.map(|r| (s.id, r))).collect()
}

impl Sources {
    /// STRIASEQRATEEDL: the rate the document gave source `id` (`None`: none), the one [`Sources::add_sequence`] takes when its
    /// pattern carries no `@R`.
    fn seq_doc_rate(&self, id: SourceId) -> Option<(u64, u64)> {
        self.seq_doc.iter().find(|q| q.0 == id).map(|q| q.1)
    }
    /// STRIASEQRATEEDL: `tl` with every SEQUENCE reel's `seq_rate` set to the rate it was opened at (`None` when it has none),
    /// other reels untouched: what a save writes, so a reopened EDL plays each sequence at its rate. A copy: the model the
    /// face's undo and dirty reading compare is left alone (a clean face opened with `@R` and no SEQRATE line still writes it).
    pub fn seq_rated(&self, tl: &Timeline) -> Timeline {
        let mut out = tl.clone();
        for s in out.sources.iter_mut() {
            if let Some(q) = self.seq(s.id) {
                s.seq_rate = q.rate.map(|r| (r.0, r.3));
            }
        }
        out
    }
}

impl Seq {
    /// STRIASEQRATEEDL: the canvas of file `n` shown at source tick `k`: `k` minus the file's first tick (`ticks(n - first)`,
    /// the inverse of `file_at`), so frame order across the ticks the file covers (not the file's own delays: the seat's
    /// ruling). 0 at every tick with no rate (one tick per file).
    fn frame_in(&self, k: i64, n: u64) -> usize {
        usize::try_from(k.saturating_sub(self.ticks(n.saturating_sub(self.first)))).unwrap_or(0)
    }
}

/// STRIASEQRATEEDL: a decoded file's canvases back to back: every composited frame of an animation (more than one), else
/// `first` (the still's own bytes, no copy).
fn seq_canvases(frames: Option<Vec<gneiss_pal::dsp::image::Frame>>, first: Vec<u8>) -> Vec<u8> {
    match frames {
        Some(v) if v.len() > 1 => v.into_iter().flat_map(|f| f.rgba).collect(),
        _ => first,
    }
}

/// STRIASEQRATEEDL: canvas `i` of `all` (`w` x `h` RGBA canvases back to back), the last one held past the end.
fn seq_pick(all: &[u8], w: u32, h: u32, i: usize) -> Vec<u8> {
    let size = w as usize * h as usize * 4;
    let count = if size == 0 { 1 } else { (all.len() / size).max(1) };
    if count == 1 {
        return all.to_vec();
    }
    let j = i.min(count - 1);
    all[j * size..(j + 1) * size].to_vec()
}

/// STRIASEQRATEEDL: [`seq_pick`] over an owned buffer: a single canvas is handed on as it is.
fn seq_pick_vec(all: Vec<u8>, w: u32, h: u32, i: usize) -> Vec<u8> {
    let size = w as usize * h as usize * 4;
    if size == 0 || all.len() <= size { all } else { seq_pick(&all, w, h, i) }
}

// ---- STRIASTACK (LEDGER SR9066): the stack under the mixed layer, composited through the one mix (named tail block) ----
// NLE-OPACITY-STACK. A step's `stack` (timeline_core `schedule::Step::stack`, bottom to top) lies under `vmix`'s source:
// the bottom layer's frame (or opaque black), each next layer put on at the `t` of the entry below it, then
// `vmix`'s own frame at the top stack entry's `t`; that composite is `vmix_frame`'s outgoing, which the step's own frame
// then goes over at `vmix.t`. Every layer decodes on its own run (`stack_runs[j]`) through `vid_frame` / `reel_rgba`, and
// every mix is timeline_core's ONE `vmix::mix_step`. An empty stack returns the frame untouched (byte-identical).

/// STRIASTACK: `top` (the frame of `s.vmix`'s source, `w x h`) composited over `s.stack`.
#[allow(clippy::too_many_arguments)]
fn stack_rgba(
    tl: &Timeline,
    sources: &Sources,
    s: &timeline_core::schedule::Step,
    stack_runs: &mut [Option<Vid>; timeline_core::schedule::STACK_UNDER],
    top: Vec<u8>,
    w: u32,
    h: u32,
    tol: i64,
    r: &mut RenderReport,
) -> Result<Vec<u8>, RenderError> {
    if s.stack == timeline_core::schedule::NO_STACK {
        return Ok(top);
    }
    let mut out: Option<Vec<u8>> = None;
    let mut under_t = None;
    for (j, m) in s.stack.iter().enumerate() {
        let Some(m) = *m else { break };
        let f = match m.src {
            Some(src) => {
                let a = vid_frame(tl, sources, &mut stack_runs[j], src, m.src_t, m.seek, tol, r)?;
                reel_rgba(sources, src, &a, &mut stack_runs[j], w, h, reel_fit(tl, src, false))?.0
            }
            None => [0u8, 0, 0, 255].repeat(w as usize * h as usize),
        };
        out = Some(match (out, under_t) {
            (Some(mut o), Some(t)) => {
                timeline_core::vmix::mix_step(timeline_core::TransitionKind::Dissolve, t, &mut o, &f, w, h);
                o
            }
            _ => f,
        });
        under_t = Some(m.t);
    }
    let (Some(mut o), Some(t)) = (out, under_t) else { return Ok(top) };
    timeline_core::vmix::mix_step(timeline_core::TransitionKind::Dissolve, t, &mut o, &top, w, h);
    Ok(o)
}

// ---- STRIASPEEDREVERSE (LEDGER SR9067): a reversed clip on the one render — the back cache and the reversed take (named tail block) ----
// The schedule names a reversed clip's source ticks DESCENDING and seeks only at its first step (timeline_core's `src_at`
// continuity). VIDEO: a continuing run (same source, no seek) whose target is behind the frame it last showed is served by
// `vid_back` from `Vid.back`, the last `BACK_CACHE` frames the run decoded up to its target; a miss re-opens the run at the
// keyframe at or before the target through THE `Vid::open` and decodes forward to it keeping the last `BACK_CACHE` (at worst
// one GOP decode per `BACK_CACHE` steps; the decoded frames count as `runup_frames`, the open as `seeks`). A forward run never
// fills the cache, so every forward render is unchanged; a forward step on a run with a cache drops the run and the normal
// road re-opens it. AUDIO: a reversed layer's run hears source ticks `lo = src_at(end - 1) ..= src_at(out_t)`; `take_reversed`
// re-opens the layer's slot at the keyframe at or before `lo`, reads the run's `n` samples FORWARD from `lo` through THE
// `take_rated` (the 1x copy or the rated Sinc: a clone of the run whose layer starts at `lo`) and reverses the frames.

/// STRIASPEEDREVERSE: how many decoded frames a backward run keeps.
const BACK_CACHE: usize = 16;

/// STRIASPEEDREVERSE: the frame at `src_t` when run `run` (same `src`, no `seek`) steps backwards; `None`: the normal road.
#[allow(clippy::too_many_arguments)]
fn vid_back(tl: &Timeline, sources: &Sources, run: &mut Option<Vid>, src: SourceId, src_t: i64, seek: Option<i64>, tol: i64, r: &mut RenderReport) -> Result<Option<Frame>, RenderError> {
    let target = tl.tb.to_ns(src_t);
    let Some(v) = run.as_mut().filter(|v| v.src == src && seek.is_none()) else { return Ok(None) };
    let Some(shown) = v.last.as_ref().map(|f| f.pts_ns - v.origin) else { return Ok(None) };
    if target >= shown - tol {
        if v.back.is_empty() {
            return Ok(None);
        }
        if target <= shown + tol {
            return Ok(v.last.clone());
        }
        *run = None;
        return Ok(None);
    }
    let origin = v.origin;
    if v.back.front().is_some_and(|f| f.pts_ns - origin <= target + tol) {
        let f = v.back.iter().rev().find(|f| f.pts_ns - origin <= target + tol).cloned().expect("the front qualifies");
        v.last = Some(f.clone());
        if v.stood_in {
            sources.note_stand_in(src);
        }
        return Ok(Some(f));
    }
    let key = sources.keyframe_ns(src, Kind::Video, target).ok_or(RenderError::NoStream(src, Kind::Video))?;
    *run = None;
    *run = Some(Vid::open(sources.get(src)?, src, key)?);
    r.seeks += 1;
    let v = run.as_mut().expect("opened above");
    loop {
        if v.peeked.is_none() {
            v.peeked = v.pull()?;
        }
        let rel = match v.peeked.as_ref() {
            Some(f) => f.pts_ns - v.origin,
            None => break,
        };
        if rel > target + tol {
            break;
        }
        let f = v.peeked.take().expect("just seen");
        v.back.push_back(f);
        if v.back.len() > BACK_CACHE {
            v.back.pop_front();
        }
        r.runup_frames += 1;
    }
    let f = match v.back.back() {
        Some(f) => {
            r.runup_frames = r.runup_frames.saturating_sub(1);
            f.clone()
        }
        None => v.peeked.clone().ok_or(RenderError::Short(src))?,
    };
    v.last = Some(f.clone());
    if v.stood_in {
        sources.note_stand_in(src);
    }
    Ok(Some(f))
}

/// STRIASPEEDREVERSE: layer `li` of `run` plays a reversed clip (timeline_core's `AudioRun::layer_clip`).
fn reversed_layer(tl: &Timeline, run: &AudioRun, li: usize) -> bool {
    run.layer_clip(tl, li).is_some_and(|c| c.reverse)
}

/// STRIASPEEDREVERSE: a reversed layer's `n` output samples: its slot re-opened at the keyframe at or before the run's lowest
/// source tick, read forward from there through THE `take_rated`, the frames reversed; `None`: not a reversed layer.
#[allow(clippy::too_many_arguments)]
fn take_reversed(tl: &Timeline, run: &AudioRun, li: usize, sources: &Sources, slot: &mut Option<Aud>, rate: u32, ch: u16, n: i64, seeks: &mut u64) -> Result<Option<Vec<f32>>, RenderError> {
    let Some(c) = run.layer_clip(tl, li).filter(|c| c.reverse) else { return Ok(None) };
    let l = run.layers[li];
    let lo = c.src_at(run.out_t + run.len - 1);
    let key = sources.keyframe_ns(l.src, Kind::Audio, tl.tb.to_ns(lo)).ok_or(RenderError::NoStream(l.src, Kind::Audio))?;
    *slot = Some(Aud::open(sources.get(l.src)?, l.src, key, ch)?);
    *seeks += 1;
    let mut fwd = run.clone();
    fwd.layers[li].src_t = lo;
    let pcm = take_rated(tl, &fwd, li, slot.as_mut().expect("opened above"), samples(tl.tb.to_ns(lo), rate), n, rate)?;
    Ok(Some(pcm.chunks(ch.max(1) as usize).rev().flatten().copied().collect()))
}
