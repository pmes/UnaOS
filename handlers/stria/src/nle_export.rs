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
//!   quantiser [`pcm16`] inverts the decoders' `s / 32768.0` exactly.
//! * [`ExportReport::fnv`] is FNV-1a 64 over the rendered RGBA stream: the same number `nle render` prints.
//! * Single-threaded: the sink owns its encoder, files and counters by `&mut self`; an I/O error is latched and
//!   returned by [`ExportSink::finish`].

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use gneiss_pal::dsp::demux::TrackKind;
use gneiss_pal::dsp::demux::build;
use gneiss_pal::dsp::image::{PngError, encode_rgba};
use h264enc_core::yuv::I420;
use h264enc_core::{Config, Encoder, RateControl, RcMode};
use timeline_core::{Timebase, Timeline};

use crate::nle::{RenderError, RenderSink, Sources, render};

/// One encoded unit: a whole PNG, a raw frame, or (NLE5b) one H.264 access unit as an MP4 sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    pub keyframe: bool,
    pub data: Vec<u8>,
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
}

/// A PNG per frame through pixel_core's writer (lossless RGBA, colour type 6).
#[derive(Debug, Default, Clone, Copy)]
pub struct PngSequence;

impl VideoEncoder for PngSequence {
    fn encode(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<Access>, ExportError> {
        let data = encode_rgba(width, height, rgba).map_err(ExportError::Png)?;
        Ok(vec![Access { keyframe: true, data }])
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
        Ok(vec![Access { keyframe: true, data: rgba.to_vec() }])
    }
    fn config(&self) -> VideoConfig {
        VideoConfig { fourcc: *b"raw ", config_box: None, config: Vec::new() }
    }
    fn layout(&self) -> Layout {
        Layout::OneFile("rgba")
    }
}

/// H.264 through H264ENC (Constrained Baseline, constant QP or a CBR target ([`H264Encoder::with_bitrate`], H264ENCRC
/// SR218), an IDR every `gop` frames): each RGBA frame becomes I420
/// by `h264enc_core::yuv::I420::from_rgba`, then one access unit, stored as its avcC sample. The encoder is built at
/// the first frame (its size); a later frame of another size is refused (`h264enc_core::Error::FrameSize`).
pub struct H264Encoder {
    qp: u8,
    gop: u32,
    rate: Option<RateControl>,
    enc: Option<Encoder>,
}

impl H264Encoder {
    /// `qp` 16..=40 and `gop` >= 1 (checked by `Encoder::new` at the first frame).
    pub fn new(qp: u8, gop: u32) -> H264Encoder {
        H264Encoder { qp, gop, rate: None, enc: None }
    }

    /// A constant bit rate of `kbps` kbit/s at `fps_num / fps_den` frames per second (`nle export --bitrate`): `qp`
    /// becomes the rate controller's starting QP.
    pub fn with_bitrate(mut self, kbps: u32, fps_num: u32, fps_den: u32) -> H264Encoder {
        self.rate = Some(RateControl { bits_per_second: kbps.saturating_mul(1000), fps_num, fps_den, mode: RcMode::Cbr });
        self
    }
}

impl VideoEncoder for H264Encoder {
    fn encode(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<Access>, ExportError> {
        if self.enc.is_none() {
            self.enc = Some(Encoder::new(Config { width, height, qp: self.qp, gop: self.gop, rate: self.rate, ..Default::default() }).map_err(ExportError::H264)?);
        }
        let enc = self.enc.as_mut().expect("built above");
        let yuv = I420::from_rgba(width, height, rgba).map_err(ExportError::H264)?;
        let au = enc.encode(&yuv.as_yuv420()).map_err(ExportError::H264)?;
        Ok(vec![Access { keyframe: au.keyframe, data: au.avcc_sample() }])
    }
    fn config(&self) -> VideoConfig {
        VideoConfig { fourcc: *b"avc1", config_box: Some(*b"avcC"), config: self.enc.as_ref().map(Encoder::avcc).unwrap_or_default() }
    }
    fn layout(&self) -> Layout {
        Layout::Mp4
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
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Io(e) => write!(f, "io: {e}"),
            ExportError::Png(e) => write!(f, "png: {e:?}"),
            ExportError::Render(e) => write!(f, "render: {e}"),
            ExportError::H264(e) => write!(f, "{e}"),
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
    /// [`Layout::Mp4`]: (keyframe, sample) per unit, muxed at `finish`.
    samples: Vec<(bool, Vec<u8>)>,
    afmt: Option<(u32, u16)>,
    size: (u32, u32),
    units: u64,
    report: ExportReport,
    err: Option<ExportError>,
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
                Layout::Mp4 => self.samples.push((u.keyframe, u.data)),
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
            AudioOut::Mp4 => self.pcm.extend(s.iter().map(|&x| pcm16(x))),
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
        let audio = match (self.audio, self.afmt) {
            (AudioOut::Mp4, Some((rate, ch))) => Some(build::pcm16_track(if movie { 2 } else { 1 }, rate, ch, &self.pcm, 1024)),
            _ => None,
        };
        let mut tracks: Vec<build::MediaTrack> = Vec::new();
        if movie {
            tracks.push(self.video_track(tb));
        }
        tracks.extend(audio);
        if !tracks.is_empty() {
            let name = match (movie, self.audio == AudioOut::Mp4) {
                (true, true) => "movie.mp4",
                (true, false) => "video.mp4",
                (false, _) => "audio.mp4",
            };
            let mp4 = build::mp4(&tracks, &build::Mp4Options::default());
            std::fs::write(self.dir.join(name), &mp4)?;
            self.report.files += 1;
            self.report.bytes += mp4.len() as u64;
        }
        Ok(self.report)
    }

    /// The [`Layout::Mp4`] video track: the encoder's sample entry, timescale = the project's fps (`tb.den`), sample
    /// `i` at `i * tb.num`.
    fn video_track(&mut self, tb: Timebase) -> build::MediaTrack {
        let c = self.video.config();
        let num = tb.num.max(1) as u32;
        let spec = build::TrackSpec {
            id: 1,
            kind: TrackKind::Video,
            fourcc: c.fourcc,
            config_box: c.config_box,
            codec_id: "",
            config: c.config,
            timescale: tb.den as u32,
            width: self.size.0 as u16,
            height: self.size.1 as u16,
            sample_rate: 0,
            channels: 0,
            bit_depth: 0,
            default_duration_ns: num as u64 * 1_000_000_000 / (tb.den as u64).max(1),
        };
        let samples = std::mem::take(&mut self.samples)
            .into_iter()
            .enumerate()
            .map(|(i, (keyframe, data))| build::SampleSpec { data, dts: i as i64 * num as i64, pts: i as i64 * num as i64, duration: num, keyframe })
            .collect();
        build::MediaTrack { spec, samples }
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
    let mut sink = ExportSink::new(dir, video, audio);
    render(tl, range, sources, &mut sink).map_err(ExportError::Render)?;
    sink.finish(tl.tb)
}
