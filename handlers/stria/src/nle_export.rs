// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Stria's editor export (NLE5, LEDGER SR204; CODEX §2 A/V).
//!
//! CHARTER: Stria — the export half of the non-linear editor: a [`RenderSink`] over [`crate::nle::render`] that hands
//! each frame to a [`VideoEncoder`] and the PCM to a WAV or a PCM16 MP4; it owns no encoder core (shared-core,
//! BP-handlers-4: PNG is pixel_core's `encode_rgba`, H.264 is h264enc_core's `Encoder` over its `yuv::I420`, the MP4
//! is demux_core's `build::mp4`).
//!
//! * The ENCODER SEAM is [`VideoEncoder`]: RGBA in, [`Access`] units out, plus the [`VideoConfig`] an MP4 sample entry
//!   needs and the [`Layout`] the units are stored in. [`PngSequence`] (one `%06d.png` per frame) and
//!   [`RawSequence`] (one `video.rgba` + a `video.json` sidecar) store files; [`H264Encoder`] (NLE5B, LEDGER SR206)
//!   is H264ENC's encoder fed by its RGBA -> I420 (`h264enc_core::yuv::I420::from_rgba`, the tree's one converter)
//!   and stores [`Layout::Mp4`]: every access unit an `avc1` sample, muxed at the end in ONE `build::mp4` call with
//!   the PCM16 track when the audio is [`AudioOut::Mp4`] (`movie.mp4`), alone otherwise (`video.mp4`).
//! * Audio: [`AudioOut::Wav`] streams RIFF/WAVE PCM16 (`audio.wav`, header patched at the end), [`AudioOut::Mp4`]
//!   muxes an audio-only `sowt` track (`audio.mp4`) through `build::pcm16_track` + `build::mp4`. The f32 → i16
//!   quantiser [`pcm16`] inverts the decoders' `s / 32768.0` exactly. [`AudioOut::Aac`] (NLE6EXPORTAAC, rmbp-ledger
//!   B960; `nle export --audio aac`) writes the same track as AAC-LC: audio_core's `aac::enc::Encoder` (the kernel's
//!   export encodes through it too) into `build::mp4a_track`, its 1024-sample priming trimmed by `build::mp4a_edit_list`
//!   in ONE `build::mp4_with`. PCM16 stays the default.
//! * [`ExportReport::fnv`] is FNV-1a 64 over the rendered RGBA stream: the same number `nle render` prints.
//! * H264ENCBFRAMES (SR333): [`H264Encoder::with_bframes`] codes B pictures through h264enc_core's `push` / `flush`;
//!   each unit carries its (dts, pts) ([`Access::time`]) and the MP4 track writes them per sample, so demux_core's
//!   `build::mp4` writes the `ctts` composition offsets. No edit list (demux_core shifts pts only), so a reordered
//!   video presents from tick 1, or tick 2 under h264enc_core's B pyramid at bframes >= 3 (reorder depth 2, H264ENCPYRAMID SR349).
//! * H264ENCCOLOURHOST (LEDGER SR377): the file states ONE colour, fixed at its first frame ([`VideoEncoder::colour`]):
//!   [`H264Encoder`] always encodes the mix's RGBA by `push_rgba` (BT.601 limited), so its file colour is
//!   [`RGBA_COLOUR`] (matrix 6, limited: the kernel export's RGBA-path tag, byte for byte), written
//!   as the SPS VUI (h264enc_core's `Config::colour`) and as the avc1's `colr` through `build::Mp4Extras::colour`. PNG and
//!   raw state none. EXPORTCOLOURPRIM (rmbp-ledger B979): its primaries / transfer are the reels' one pair
//!   (`timeline_core::Timeline::reels_colour` over `Sources::open_report`, the kernel export's rule too), else 2 / 2.
//! * Single-threaded: the sink owns its encoder, files and counters by `&mut self`; an I/O error is latched and
//!   returned by [`ExportSink::finish`].
//! * ADOPTPNG (LEDGER SR405): [`export_png`] is the PNG sequence ([`PngSequence`] through [`export_as`]) read back per frame
//!   by pixel_core's header reader (`facts_of`): each frame's path and size (Adopt's frames keep their own), every I/O
//!   refusal naming the directory.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use gneiss_pal::dsp::demux::build;
/// NLE6EXPORTMKV (rmbp-ledger B966): the export's container, for the faces that name it (`nle export --mkv`).
pub use gneiss_pal::dsp::demux::build::Container;
use gneiss_pal::dsp::image::{PngError, encode_rgba};
use h264enc_core::rgba::{RgbaEncoder, Unit};
use h264enc_core::{Config, RateControl};
use timeline_core::{Timebase, Timeline};

use crate::nle::{RenderError, RenderSink, Sources, render};
use gneiss_pal::dsp::demux::colour::Colour;

/// One encoded unit: a whole PNG, a raw frame, or (NLE5b) one H.264 access unit as an MP4 sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    pub keyframe: bool,
    pub data: Vec<u8>,
    /// (dts, pts) in frames when the encoder reorders (SR333); None = output order, unit i at i.
    pub time: Option<(i64, i64)>,
}

/// What an MP4 sample entry needs to carry the stream (`build::TrackSpec`'s `fourcc`, `config_box`, `config`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoConfig {
    pub fourcc: [u8; 4],
    pub config_box: Option<[u8; 4]>,
    pub config: Vec<u8>,
}

/// Where an encoder's units are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// One file per unit, `%06d.<ext>` (the frame ordinal from 0).
    FilePerFrame(&'static str),
    /// Every unit appended to `video.<ext>`, with a `video.json` sidecar.
    OneFile(&'static str),
    /// Every unit one MP4 sample (the [`VideoConfig`] its sample entry), muxed at [`ExportSink::finish`]: `movie.mp4`
    /// with the PCM16 track under [`AudioOut::Mp4`], else `video.mp4`.
    Mp4,
}

/// The encoder seam. RGBA frames in output order go in; zero or more [`Access`] units come out (an encoder with
/// delay returns them late and the rest from [`flush`](VideoEncoder::flush)).
///
/// [`H264Encoder`] (NLE5B, SR206) implements this once for `h264enc_core::Encoder`: its
/// `encode -> Access { keyframe, nals }` maps to [`Access`] `{ keyframe, data: Access::avcc_sample() }`, its `avcc()`
/// to [`VideoConfig`] `{ fourcc: *b"avc1", config_box: Some(*b"avcC"), config: avcc }`, and the [`Layout::Mp4`] arm
/// muxes it with the audio in one `build::mp4` call.
pub trait VideoEncoder {
    fn encode(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<Access>, ExportError>;
    fn flush(&mut self) -> Result<Vec<Access>, ExportError> {
        Ok(Vec::new())
    }
    fn config(&self) -> VideoConfig;
    fn layout(&self) -> Layout;
    /// H264ENCCOLOURHOST (SR377): called once, before the first frame's `encode`: the file's one colour, which the encoder
    /// states in its own stream too. None = the stream states no colour. EXPORTCOLOURPRIM (B979): `reels` is the timeline's
    /// one (primaries, transfer) pair (`Timeline::reels_colour`), (2, 2) when the reels state none or differ.
    fn colour(&mut self, reels: (u8, u8)) -> Option<Colour> {
        let _ = reels;
        None
    }
}

/// A PNG per frame through pixel_core's writer (lossless RGBA, colour type 6).
#[derive(Debug, Default, Clone, Copy)]
pub struct PngSequence;

impl VideoEncoder for PngSequence {
    fn encode(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<Access>, ExportError> {
        let data = encode_rgba(width, height, rgba).map_err(ExportError::Png)?;
        Ok(vec![Access { keyframe: true, data, time: None }])
    }
    fn config(&self) -> VideoConfig {
        VideoConfig { fourcc: *b"png ", config_box: None, config: Vec::new() }
    }
    fn layout(&self) -> Layout {
        Layout::FilePerFrame("png")
    }
}

/// The render's RGBA bytes unchanged, every frame appended to one `video.rgba` (`w * h * 4` bytes each).
#[derive(Debug, Default, Clone, Copy)]
pub struct RawSequence;

impl VideoEncoder for RawSequence {
    fn encode(&mut self, _width: u32, _height: u32, rgba: &[u8]) -> Result<Vec<Access>, ExportError> {
        Ok(vec![Access { keyframe: true, data: rgba.to_vec(), time: None }])
    }
    fn config(&self) -> VideoConfig {
        VideoConfig { fourcc: *b"raw ", config_box: None, config: Vec::new() }
    }
    fn layout(&self) -> Layout {
        Layout::OneFile("rgba")
    }
}

/// H.264 through H264ENC (Constrained Baseline, constant QP or a CBR target ([`H264Encoder::with_bitrate`], H264ENCRC
/// SR218) or a constant quality ([`H264Encoder::with_quality`], H264ENCVBRCRF SR383), an IDR every `gop` frames): each RGBA frame becomes I420
/// by `h264enc_core::yuv::I420::from_rgba`, then one access unit, stored as its avcC sample. The encoder is built at
/// the first frame (its size); a later frame of another size is refused (`h264enc_core::Error::FrameSize`).
/// NLE6EXPORT (rmbp-ledger B959): the glue is h264enc_core's `rgba::RgbaEncoder` (the kernel's `nle export` drives the same).
pub struct H264Encoder {
    rgba: RgbaEncoder,
}

impl H264Encoder {
    /// `qp` 16..=40 and `gop` >= 1 (checked by `Encoder::new` at the first frame).
    pub fn new(qp: u8, gop: u32) -> H264Encoder {
        H264Encoder { rgba: RgbaEncoder::new(Config { qp, gop, ..Default::default() }) }
    }

    /// A constant bit rate of `kbps` kbit/s at `fps_num / fps_den` frames per second (`nle export --bitrate`): `qp`
    /// becomes the rate controller's starting QP.
    pub fn with_bitrate(mut self, kbps: u32, fps_num: u32, fps_den: u32) -> H264Encoder {
        self.rgba.cfg.rate = Some(RateControl::cbr(kbps, (fps_num, fps_den)));
        self
    }

    /// H264ENCVBRCRF (SR383): constant quality `crf` (0..=51, h264enc_core's `RcMode::Crf`) at `fps_num / fps_den` frames per
    /// second (`nle export --crf`): the one `Config.rate` [`H264Encoder::with_bitrate`] sets, so the last of the two wins
    /// (the faces refuse both by name).
    pub fn with_quality(mut self, crf: u8, fps_num: u32, fps_den: u32) -> H264Encoder {
        self.rgba.cfg.rate = Some(RateControl::crf(crf, (fps_num, fps_den)));
        self
    }

    /// `n` B pictures between anchors (H264ENCBFRAMES SR333, h264enc_core's `Config::bframes`): units come out in
    /// decoding order with their (dts, pts), the last ones from `flush`. 0 = the pinned stream.
    pub fn with_bframes(mut self, n: u8) -> H264Encoder {
        self.rgba.cfg.bframes = n;
        self
    }
}

/// h264enc_core's units (`rgba::Unit`: the avcC sample and its (dts, pts)) as the seam's [`Access`].
fn h264_units(units: Vec<Unit>) -> Vec<Access> {
    units.into_iter().map(|u| Access { keyframe: u.keyframe, data: u.sample, time: Some((u.dts, u.pts)) }).collect()
}

impl VideoEncoder for H264Encoder {
    fn encode(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<Access>, ExportError> {
        Ok(h264_units(self.rgba.push_rgba(width, height, rgba).map_err(ExportError::H264)?))
    }
    fn flush(&mut self) -> Result<Vec<Access>, ExportError> {
        Ok(h264_units(self.rgba.flush().map_err(ExportError::H264)?))
    }
    fn config(&self) -> VideoConfig {
        VideoConfig { fourcc: *b"avc1", config_box: Some(*b"avcC"), config: self.rgba.avcc() }
    }
    fn layout(&self) -> Layout {
        Layout::Mp4
    }
    fn colour(&mut self, (primaries, transfer): (u8, u8)) -> Option<Colour> {
        let c = Colour { primaries, transfer, ..RGBA_COLOUR }; // H264ENCCOLOURHOST (SR377): the RGBA path's colour, also the SPS VUI. EXPORTCOLOURPRIM (B979): with the reels' one pair
        self.rgba.cfg.colour = Some((c.primaries, c.transfer, c.matrix, c.full_range == Some(true)));
        Some(c)
    }
}

/// Where the PCM goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioOut {
    None,
    /// `audio.wav`: RIFF/WAVE PCM16.
    Wav,
    /// `audio.mp4`: an audio-only MP4, one `sowt` PCM16 track.
    Mp4,
    /// NLE6EXPORTAAC (rmbp-ledger B960): as [`AudioOut::Mp4`] (`audio.mp4`, or `movie.mp4` beside H.264), the track one
    /// `mp4a` AAC-LC track at this many kb/s.
    Aac(u32),
}

/// What an export wrote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExportReport {
    pub frames: u64,
    pub files: u64,
    pub bytes: u64,
    /// FNV-1a 64 over the rendered RGBA stream (`nle render`'s fnv).
    pub fnv: u64,
    pub pcm_frames: u64,
}

#[derive(Debug)]
pub enum ExportError {
    Io(std::io::Error),
    Png(PngError),
    Render(RenderError),
    H264(h264enc_core::Error),
    /// NLE6EXPORTAAC (rmbp-ledger B960): the AAC encoder refused the stream (its rate or channel count).
    Aac(gneiss_pal::dsp::audio::Error),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Io(e) => write!(f, "io: {e}"),
            ExportError::Png(e) => write!(f, "png: {e:?}"),
            ExportError::Render(e) => write!(f, "render: {e}"),
            ExportError::H264(e) => write!(f, "{e}"),
            ExportError::Aac(e) => write!(f, "aac: {e:?}"),
        }
    }
}
impl std::error::Error for ExportError {}

impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        ExportError::Io(e)
    }
}

/// f32 PCM to 16-bit: the inverse of the decoders' `s as f32 / 32768.0`, rounded and clamped.
pub fn pcm16(x: f32) -> i16 {
    (x * 32768.0).round().clamp(-32768.0, 32767.0) as i16
}

/// The 44-byte RIFF/WAVE header of `frames` sample frames of 16-bit PCM (format tag 1).
pub fn wav_header(rate: u32, channels: u16, frames: u64) -> [u8; 44] {
    let align = channels as u32 * 2;
    let data = (frames * align as u64).min(u32::MAX as u64 - 36) as u32;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&channels.to_le_bytes());
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[28..32].copy_from_slice(&(rate * align).to_le_bytes());
    h[32..34].copy_from_slice(&(align as u16).to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data.to_le_bytes());
    h
}

/// The streamed WAV: header placeholder, samples, header rewritten at the end.
struct Wav {
    file: BufWriter<File>,
    rate: u32,
    channels: u16,
    frames: u64,
}

/// The export's [`RenderSink`]: drives the encoder per frame and the audio writer per PCM step.
pub struct ExportSink {
    dir: PathBuf,
    video: Box<dyn VideoEncoder>,
    audio: AudioOut,
    one: Option<BufWriter<File>>,
    wav: Option<Wav>,
    pcm: Vec<i16>,
    /// [`Layout::Mp4`]: (keyframe, sample, (dts, pts)) per unit, muxed at `finish`.
    samples: Vec<(bool, Vec<u8>, Option<(i64, i64)>)>,
    afmt: Option<(u32, u16)>,
    size: (u32, u32),
    units: u64,
    report: ExportReport,
    err: Option<ExportError>,
    /// H264ENCCOLOURHOST (SR377): the file's colour, fixed at the first frame by [`VideoEncoder::colour`].
    colour: Option<Colour>,
    /// EXPORTCOLOURPRIM (B979): the reels' one (primaries, transfer), set by [`export`]; (2, 2) otherwise.
    reels: (u8, u8),
    /// NLE6EXPORTMKV (rmbp-ledger B966): the muxed file's container (its name's extension, `build::Container`).
    container: build::Container,
}

impl ExportSink {
    /// Export into the existing directory `dir`.
    pub fn new(dir: &Path, video: Box<dyn VideoEncoder>, audio: AudioOut) -> ExportSink {
        ExportSink {
            dir: dir.to_path_buf(),
            video,
            audio,
            one: None,
            wav: None,
            pcm: Vec::new(),
            samples: Vec::new(),
            afmt: None,
            size: (0, 0),
            units: 0,
            report: ExportReport { fnv: 0xcbf2_9ce4_8422_2325, ..ExportReport::default() },
            err: None,
            colour: None,
            reels: (2, 2),
            container: build::Container::Mp4,
        }
    }

    fn store(&mut self, units: Vec<Access>) -> Result<(), ExportError> {
        for u in units {
            match self.video.layout() {
                Layout::FilePerFrame(ext) => {
                    std::fs::write(self.dir.join(format!("{:06}.{ext}", self.units)), &u.data)?;
                    self.report.files += 1;
                    self.report.bytes += u.data.len() as u64;
                }
                Layout::OneFile(ext) => {
                    if self.one.is_none() {
                        self.one = Some(BufWriter::new(File::create(self.dir.join(format!("video.{ext}")))?));
                        self.report.files += 1;
                    }
                    self.one.as_mut().expect("opened above").write_all(&u.data)?;
                    self.report.bytes += u.data.len() as u64;
                }
                Layout::Mp4 => self.samples.push((u.keyframe, u.data, u.time)),
            }
            self.units += 1;
        }
        Ok(())
    }

    fn frame_inner(&mut self, w: u32, h: u32, rgba: &[u8]) -> Result<(), ExportError> {
        for &b in rgba {
            self.report.fnv = (self.report.fnv ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.size = (w, h);
        if self.report.frames == 0 { self.colour = self.video.colour(self.reels); } // H264ENCCOLOURHOST (SR377): the file colour, fixed at the first frame
        let units = self.video.encode(w, h, rgba)?;
        self.store(units)?;
        self.report.frames += 1;
        Ok(())
    }

    fn pcm_inner(&mut self, rate: u32, ch: u16, s: &[f32]) -> Result<(), ExportError> {
        self.afmt.get_or_insert((rate, ch));
        let n = (s.len() / ch.max(1) as usize) as u64;
        self.report.pcm_frames += n;
        match self.audio {
            AudioOut::None => {}
            AudioOut::Mp4 | AudioOut::Aac(_) => self.pcm.extend(s.iter().map(|&x| pcm16(x))),
            AudioOut::Wav => {
                if self.wav.is_none() {
                    let mut file = BufWriter::new(File::create(self.dir.join("audio.wav"))?);
                    file.write_all(&wav_header(rate, ch, 0))?;
                    self.wav = Some(Wav { file, rate, channels: ch, frames: 0 });
                }
                let wav = self.wav.as_mut().expect("opened above");
                let bytes: Vec<u8> = s.iter().flat_map(|&x| pcm16(x).to_le_bytes()).collect();
                wav.file.write_all(&bytes)?;
                wav.frames += n;
            }
        }
        Ok(())
    }

    /// Flush the encoder, close the files (the raw sidecar, the WAV header, the MP4) and report. `tb` is the
    /// timeline's time base (`1/fps`), written to the raw sidecar and the MP4 video track's timescale.
    pub fn finish(mut self, tb: Timebase) -> Result<ExportReport, ExportError> {
        if let Some(e) = self.err.take() {
            return Err(e);
        }
        let tail = self.video.flush()?;
        self.store(tail)?;
        if let Some(mut one) = self.one.take() {
            one.flush()?;
            if let Layout::OneFile(ext) = self.video.layout() {
                let c = self.video.config();
                let json = format!(
                    "{{\"format\":\"{ext}\",\"fourcc\":\"{}\",\"w\":{},\"h\":{},\"fps_num\":{},\"fps_den\":{},\"frames\":{}}}\n",
                    String::from_utf8_lossy(&c.fourcc),
                    self.size.0,
                    self.size.1,
                    tb.den,
                    tb.num,
                    self.units
                );
                std::fs::write(self.dir.join("video.json"), &json)?;
                self.report.files += 1;
                self.report.bytes += json.len() as u64;
            }
        }
        if let Some(mut wav) = self.wav.take() {
            wav.file.seek(SeekFrom::Start(0))?;
            wav.file.write_all(&wav_header(wav.rate, wav.channels, wav.frames))?;
            wav.file.flush()?;
            self.report.files += 1;
            self.report.bytes += 44 + wav.frames * wav.channels as u64 * 2;
        }
        let movie = self.video.layout() == Layout::Mp4;
        let mut extras = build::Mp4Extras::default();
        if movie { extras.colour.extend(self.colour.map(|c| (1, c))); } // H264ENCCOLOURHOST (SR377): the file colour as the avc1's (track 1) `colr`
        let audio = match (self.audio, self.afmt) {
            (AudioOut::Mp4, Some((rate, ch))) => Some(build::pcm16_track(if movie { 2 } else { 1 }, rate, ch, &self.pcm, 1024)),
            (AudioOut::Aac(kbps), Some((rate, ch))) => {
                let (t, edit) = aac_track(if movie { 2 } else { 1 }, rate, ch, &self.pcm, kbps)?;
                extras.edit_lists.push(edit);
                Some(t)
            }
            _ => None,
        };
        let mut tracks: Vec<build::MediaTrack> = Vec::new();
        if movie {
            tracks.push(self.video_track(tb));
        }
        tracks.extend(audio);
        if !tracks.is_empty() {
            let stem = match (movie, matches!(self.audio, AudioOut::Mp4 | AudioOut::Aac(_))) {
                (true, true) => "movie",
                (true, false) => "video",
                (false, _) => "audio",
            };
            // NLE6EXPORTMKV (rmbp-ledger B966): the container is the file name's extension; ONE one-shot muxer for both
            let name = format!("{stem}.{}", self.container.ext());
            let mp4 = build::mux(&tracks, self.container, &extras).map_err(|e| ExportError::Io(std::io::Error::other(e)))?;
            std::fs::write(self.dir.join(name), &mp4)?;
            self.report.files += 1;
            self.report.bytes += mp4.len() as u64;
        }
        Ok(self.report)
    }

    /// The [`Layout::Mp4`] video track: the encoder's sample entry, timescale = the project's fps (`tb.den`), sample
    /// `i` at `i * tb.num`, or at its unit's (dts, pts) x `tb.num` (SR333: pts != dts writes `ctts`).
    fn video_track(&mut self, tb: Timebase) -> build::MediaTrack {
        let c = self.video.config();
        let spec = build::coded_video_spec(1, c.fourcc, c.config_box, c.config, self.size.0 as u16, self.size.1 as u16, tb);
        let units = std::mem::take(&mut self.samples).into_iter().enumerate().map(|(i, (keyframe, data, time))| {
            let (dts, pts) = time.unwrap_or((i as i64, i as i64));
            (keyframe, data, dts, pts)
        });
        build::coded_video_track(spec, tb, units)
    }
}

impl RenderSink for ExportSink {
    fn frame(&mut self, _out_t: i64, width: u32, height: u32, rgba: &[u8]) {
        if self.err.is_none() {
            if let Err(e) = self.frame_inner(width, height, rgba) {
                self.err = Some(e);
            }
        }
    }
    fn pcm(&mut self, _out_t: i64, rate: u32, channels: u16, interleaved: &[f32]) {
        if self.err.is_none() {
            if let Err(e) = self.pcm_inner(rate, channels, interleaved) {
                self.err = Some(e);
            }
        }
    }
}

/// Render output ticks `range` of `tl` into the existing directory `dir` through `video` and `audio`.
pub fn export(
    tl: &Timeline,
    range: std::ops::Range<i64>,
    sources: &Sources,
    dir: &Path,
    video: Box<dyn VideoEncoder>,
    audio: AudioOut,
) -> Result<ExportReport, ExportError> {
    export_as(tl, range, sources, dir, video, audio, build::Container::Mp4)
}

/// NLE6EXPORTMKV (rmbp-ledger B966): [`export`] with the muxed file's container: `movie.mp4` / `video.mp4` / `audio.mp4`, or
/// the same names in `.mkv` (`build::mux`: the Matroska `build::MkvStream` fed in decode order). The rule is the kernel's:
/// the file name's extension names the container (`build::Container::of`); `nle export --mkv` picks the `.mkv` names.
pub fn export_as(
    tl: &Timeline,
    range: std::ops::Range<i64>,
    sources: &Sources,
    dir: &Path,
    video: Box<dyn VideoEncoder>,
    audio: AudioOut,
    container: build::Container,
) -> Result<ExportReport, ExportError> {
    let mut sink = ExportSink::new(dir, video, audio); sink.reels = tl.reels_colour(|id| sources.open_report(id).and_then(|r| r.colour)); // EXPORTCOLOURPRIM (rmbp-ledger B979): the reels' one primaries / transfer pair, the file's
    sink.container = container;
    render(tl, range, sources, &mut sink).map_err(ExportError::Render)?;
    sink.finish(tl.tb)
}

/// NLE6EXPORTAAC (rmbp-ledger B960): interleaved PCM16 as an `mp4a` AAC-LC track `id` through audio_core's encoder, and
/// the edit list that trims its priming to the PCM's own length.
fn aac_track(id: u32, rate: u32, ch: u16, pcm: &[i16], kbps: u32) -> Result<(build::MediaTrack, (u32, Vec<(u32, i64, i32)>)), ExportError> {
    use gneiss_pal::dsp::audio::aac::enc::{Encoder, PRIMING};
    let mut enc = Encoder::new(rate, ch, kbps.saturating_mul(1000)).map_err(ExportError::Aac)?;
    let mut units = enc.encode(pcm);
    units.extend(enc.flush());
    let frames = (pcm.len() / ch.max(1) as usize) as u64;
    let track = build::mp4a_track(id, rate, ch, &enc.asc(), units.into_iter().map(|u| u.data).collect(), kbps.saturating_mul(1000));
    Ok((track, build::mp4a_edit_list(id, rate, PRIMING, frames)))
}

// ── H264ENCCOLOURHOST (LEDGER SR377): the host export's ONE file colour (handlers.md §5 NLEPROBE-HDR's H264ENC-COLOUR-HOST) ──
/// The RGBA path's colour: `push_rgba` encodes BT.601 limited, primaries / transfer unspecified — the kernel export's
/// `RGBA_COLOUR` (`unaos/crates/kernel/src/video/export.rs`), so host and kernel state the same file colour for it.
/// EXPORTCOLOURPRIM (B979): the base; [`H264Encoder`] states the reels' one primaries / transfer pair over it.
pub const RGBA_COLOUR: Colour = Colour { primaries: 2, transfer: 2, matrix: 6, full_range: Some(false), mastering: None, light: None };

// ── ADOPTPNG (LEDGER SR405): the PNG sequence read back per frame (handlers.md §5 NLEPROBE-HDR's ADOPT-PNG) ──────────────
/// [`export_as`] through [`PngSequence`] into `dir` (created), then every written `%06d.png` read back by pixel_core's ONE
/// header reader (`gneiss_pal::dsp::image::facts_of`): `(path, width, height)` per frame in output order. A reel under
/// `SizePolicy::Adopt` hands its frames on at their own size, so the sizes may differ; every other policy gives the
/// project's size. Every I/O error is re-made naming `dir` (`io: <dir>: <error>`); a file that does not read back as a PNG
/// is `InvalidData` naming its path.
pub fn export_png(
    tl: &Timeline,
    range: std::ops::Range<i64>,
    sources: &Sources,
    dir: &Path,
    audio: AudioOut,
    container: build::Container,
) -> Result<(ExportReport, Vec<(PathBuf, u32, u32)>), ExportError> {
    let named = |e: std::io::Error| ExportError::Io(std::io::Error::new(e.kind(), format!("{}: {e}", dir.display())));
    std::fs::create_dir_all(dir).map_err(named)?;
    let r = export_as(tl, range, sources, dir, Box::new(PngSequence), audio, container).map_err(|e| match e {
        ExportError::Io(e) => named(e),
        e => e,
    })?;
    let mut frames = Vec::with_capacity(r.frames as usize);
    for n in 0..r.frames {
        let path = dir.join(format!("{n:06}.png"));
        let bytes = std::fs::read(&path).map_err(named)?;
        let Some(f) = gneiss_pal::dsp::image::facts_of(&bytes) else {
            return Err(ExportError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{}: not a PNG", path.display()))));
        };
        frames.push((path, f.width, f.height));
    }
    Ok((r, frames))
}
