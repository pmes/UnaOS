// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// STRIAMETAL (SR139): the pictures and the sound — the shared cores behind two small adapters, gneiss_pal's
// dsp/video.rs `decoder_for` (the host Stria's registry: VP8, VP9, H.264, AV1) and the kernel's video/vplay.rs `Pic`
// in one enum, and audio_core's `Decoder` behind the PCM shaping hda_play.rs `dec_task` does (downmix to <= 2
// channels with Q15 weights, integer decimation to <= 48 kHz, 16-bit).
//
// PICTURES. Every packet of the video track is decoded, in decode order (all four codecs keep references, so none is
// skipped); a shown picture is CONVERTED only when the caller wants it (`want(pts_ms)` — a frame already superseded
// by the clock is decoded and dropped unconverted), straight from the decoder's planes into the window's video rect:
// letterboxed, nearest-sampled, never a full-size RGBA copy (the rect is at most 288 x 260). H.264 reorders: its
// pictures carry their own presentation time (`set_tag`). USERSTRIACOLOURONE (SR363): the private BT.601 table retired
// onto yuv_core, the kernel Player's and host Stria's ONE converter — scale THEN convert: `yuv_core::scale::nearest_rows`
// (the scaler, KERNELSCALEONE B965) gathers each output row's sampled Y / Cb / Cr as a 4:4:4 row, and ONE `yuv_core::to_rgba`
// (Chroma::Nearest) converts it with `yuv_core::pick` of the track's colour (`Track::colour`, the container's statement; else the size default).

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use audio_core::AudioDecoder;
use demux_core::{colour::Colour, Codec, Track};

/// The container's codec token (vplay.rs `tok`, the kernel Player's words).
pub fn tok(c: &Codec) -> &'static str {
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

/// Bytes of decoder state per picture pixel (the heap budget, design §4): VP8 four I420 references, VP9 nine 16-bit
/// frames (+ the 8-bit view), AV1 nine 16-bit frames, H.264 a 16-frame DPB plus the current one.
pub fn bytes_per_px(c: &Codec) -> usize {
    match c {
        Codec::Vp8 => 6,
        Codec::Vp9 => 30,
        Codec::Av1 => 28,
        Codec::Avc => 26,
        _ => 8,
    }
}

/// One picture's planes as yuv_core's `Picture` — THE literal of VP8, VP9 and H.264 (AV1's is av1_core's own
/// `image::picture`). `full` is the pick's, set per conversion in [`scale_then_convert`].
#[allow(clippy::too_many_arguments)]
fn planes<'a, T>(y: &'a [T], u: &'a [T], v: &'a [T], y_stride: usize, uv_stride: usize, w: usize, h: usize, ss: (u32, u32), mono: bool, bit_depth: u32) -> yuv_core::Picture<'a, T> {
    yuv_core::Picture { w, h, y, u, v, y_stride, uv_stride, ss, mono, bit_depth, full: false }
}

const OPAQUE: u32 = 0xFF00_0000;

/// `p` into the `ow x oh` rect `out` (0RGB): letterboxed with black bars, each output row sampled by yuv_core's ONE
/// resampler (`yuv_core::scale::nearest_rows`, KERNELSCALEONE B965, the kernel Player's `blit_fit` too), then ONE
/// `yuv_core::to_rgba` over that row with the pick of `colour` at the picture's size (the kernel Player's pick).
/// Bit for bit a full-size `to_rgba` (Chroma::Nearest) then nearest-sampled, at `dw x dh` conversions.
fn scale_then_convert<T: Copy + Into<i32>>(p: &yuv_core::Picture<'_, T>, colour: Option<Colour>, out: &mut [u32], ow: usize, oh: usize) {
    out.fill(OPAQUE);
    if p.w == 0 || p.h == 0 || ow == 0 || oh == 0 || !yuv_core::scale::fits(p) {
        return;
    }
    let (m, full) = yuv_core::pick(colour.map(|c| c.matrix), colour.and_then(|c| c.full_range), p.w as u32, p.h as u32);
    let fr = yuv_core::scale::fit(p.w, p.h, ow, oh);
    let (dw, ox, oy) = (fr.w, fr.x, fr.y);
    let (mut ry, mut ru, mut rv) = (alloc::vec![p.y[0]; dw], alloc::vec![p.y[0]; dw], alloc::vec![p.y[0]; dw]);
    let mut rgba = alloc::vec![0u8; dw * 4];
    yuv_core::scale::nearest_rows(p, dw, fr.h, &mut ry, &mut ru, &mut rv, |r, s| {
        yuv_core::to_rgba(&yuv_core::Picture { full, ..*s }, m, yuv_core::Chroma::Nearest, false, &mut rgba);
        let orow = (oy + r) * ow + ox;
        if let Some(o) = out.get_mut(orow..orow + dw) {
            for (px, c) in o.iter_mut().zip(rgba.chunks_exact(4)) {
                *px = OPAQUE | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
            }
        }
    });
}

/// The picture decoders (the shared cores, one enum — vplay.rs `Pic` with the host registry's VP9 and H.264).
enum Pic {
    Vp8(Box<vp8_core::Decoder>),
    Vp9(Box<vp9_core::Decoder>),
    #[cfg(feature = "av1")]
    Av1(Box<av1_core::image::StreamDecoder>),
    #[cfg(feature = "h264")]
    Avc { dec: Box<h264_core::Decoder>, lp: bool, ready: VecDeque<h264_core::Frame> },
}

/// A video track's decoder and the track's colour (`Track::colour`), which every shown picture converts by.
pub struct VDec {
    pic: Pic,
    colour: Option<Colour>,
}

/// What one packet gave: nothing shown, a picture shown at `pts_ms` and converted, or shown and passed over.
pub enum Out {
    None,
    Shown(u64),
    Dropped(u64),
}

impl VDec {
    /// The track's decoder; once per open, the colour line: `[stria] colour matrix=<n> range=<limited|full>
    /// src=<container|default>` (`<n>` the track's stated code, 2 when none: then the size default converts).
    pub fn new(t: &Track) -> Result<VDec, String> {
        let pic = match t.codec {
            Codec::Vp8 => Pic::Vp8(Box::new(vp8_core::Decoder::new())),
            Codec::Vp9 => Pic::Vp9(Box::new(vp9_core::Decoder::new())),
            #[cfg(feature = "av1")]
            Codec::Av1 => av1_core::image::StreamDecoder::new(&t.config).map(|d| Pic::Av1(Box::new(d))).map_err(|e| format!("av1: {}", e))?,
            #[cfg(feature = "h264")]
            Codec::Avc => {
                let mut dec = Box::new(h264_core::Decoder::new());
                let lp = !t.config.is_empty();
                if lp {
                    dec.configure_avcc(&t.config).map_err(|e| format!("h264: {}", e))?;
                }
                Pic::Avc { dec, lp, ready: VecDeque::new() }
            }
            ref c => return Err(format!("no-decoder codec={}", tok(c))),
        };
        let stated = t.colour.map(|c| c.matrix).filter(|m| *m != 2);
        let (_, full) = yuv_core::pick(stated, t.colour.and_then(|c| c.full_range), t.width, t.height);
        let src = if stated.is_some() { "container" } else { "default" };
        crate::sys::write(format!("[stria] colour matrix={} range={} src={}\n", stated.unwrap_or(2), if full { "full" } else { "limited" }, src).as_bytes());
        Ok(VDec { pic, colour: t.colour })
    }

    /// After a seek: drop every reference (decoding resumes at the keyframe the demuxer seeked to).
    pub fn reset(&mut self) {
        match &mut self.pic {
            Pic::Vp8(d) => **d = vp8_core::Decoder::new(),
            Pic::Vp9(d) => d.reset(),
            #[cfg(feature = "av1")]
            Pic::Av1(d) => d.reset(),
            #[cfg(feature = "h264")]
            Pic::Avc { dec, ready, .. } => {
                dec.reset();
                ready.clear();
            }
        }
    }

    /// Decode one packet (`data` empty = the end: H.264 drains its reorder buffer). `pts_ms` is the packet's time on
    /// the media timeline; `to_ms` maps an H.264 tag (ns, absolute) to it. A shown picture is converted into `out`
    /// (`ow x oh`) only when `want(pts_ms)`.
    pub fn decode(&mut self, data: &[u8], pts_ms: u64, pts_ns: i64, to_ms: &dyn Fn(i64) -> u64, want: &dyn Fn(u64) -> bool, out: &mut [u32], ow: usize, oh: usize) -> Result<Out, String> {
        let colour = self.colour;
        match &mut self.pic {
            Pic::Vp8(d) => {
                if data.is_empty() {
                    return Ok(Out::None);
                }
                let pic = match d.decode(data) {
                    Ok(Some(p)) => p,
                    Ok(None) => return Ok(Out::None),
                    Err(e) => return Err(format!("vp8: {}", e)),
                };
                if !want(pts_ms) {
                    return Ok(Out::Dropped(pts_ms));
                }
                let pl = planes(pic.y, pic.u, pic.v, pic.y_stride, pic.uv_stride, pic.width as usize, pic.height as usize, (1, 1), false, 8);
                scale_then_convert(&pl, colour, out, ow, oh);
                Ok(Out::Shown(pts_ms))
            }
            Pic::Vp9(d) => {
                if data.is_empty() {
                    return Ok(Out::None);
                }
                let f = match d.decode(data) {
                    Ok(Some(f)) => f,
                    Ok(None) => return Ok(Out::None),
                    Err(e) => return Err(format!("vp9: {}", e)),
                };
                if !want(pts_ms) {
                    return Ok(Out::Dropped(pts_ms));
                }
                let (cw, _) = f.plane_size(1);
                let (w, h) = (f.width as usize, f.height as usize);
                let pl = planes(&f.planes[0][..], &f.planes[1][..], &f.planes[2][..], w, cw, w, h, (f.ss_x as u32, f.ss_y as u32), false, f.bit_depth as u32);
                scale_then_convert(&pl, colour, out, ow, oh);
                Ok(Out::Shown(pts_ms))
            }
            #[cfg(feature = "av1")]
            Pic::Av1(d) => {
                if data.is_empty() {
                    return Ok(Out::None);
                }
                let p = d.decode_temporal_unit(data).map_err(|e| format!("av1: {}", e))?;
                if !want(pts_ms) {
                    return Ok(Out::Dropped(pts_ms));
                }
                scale_then_convert(&av1_core::image::picture(&p), colour, out, ow, oh);
                Ok(Out::Shown(pts_ms))
            }
            #[cfg(feature = "h264")]
            Pic::Avc { dec, lp, ready } => {
                let r = if data.is_empty() {
                    dec.flush();
                    Ok(())
                } else {
                    dec.set_tag(pts_ns);
                    if *lp {
                        dec.decode_sample(data)
                    } else {
                        let r = dec.decode_annexb(data);
                        dec.end_access_unit();
                        r
                    }
                };
                while let Some(f) = dec.next_frame() {
                    ready.push_back(f);
                }
                r.map_err(|e| format!("h264: {}", e))?;
                let Some(f) = ready.pop_front() else { return Ok(Out::None) };
                let at = to_ms(f.tag);
                if !want(at) {
                    return Ok(Out::Dropped(at));
                }
                let (w, h) = (f.width as usize, f.height as usize);
                let (sw, sh) = f.chroma.sub();
                let (ss, mono) = (((sw >> 1) as u32, (sh >> 1) as u32), f.chroma == h264_core::ChromaFormat::Mono);
                match &f.planes16 {
                    Some(p16) => scale_then_convert(&planes(&p16[0][..], &p16[1][..], &p16[2][..], w, w / sw, w, h, ss, mono, f.bit_depth as u32), colour, out, ow, oh),
                    None => scale_then_convert(&planes(&f.planes[0][..], &f.planes[1][..], &f.planes[2][..], w, w / sw, w, h, ss, mono, 8), colour, out, ow, oh),
                }
                Ok(Out::Shown(at))
            }
        }
    }

    /// Pictures still held (H.264's reorder buffer) after the last packet.
    pub fn pending(&self) -> bool {
        match &self.pic {
            #[cfg(feature = "h264")]
            Pic::Avc { ready, .. } => !ready.is_empty(),
            _ => false,
        }
    }
}

// ---- the sound ------------------------------------------------------------------------------------------------

/// The sound track: audio_core's decoder and the 16-bit PCM it becomes (hda_play.rs `dec_task`'s shaping).
pub struct Sound {
    pub dec: audio_core::Decoder,
    pub codec: &'static str,
    /// The source rate and channels, the integer decimation step, and what the stream is opened with.
    pub rate: u32,
    ch: usize,
    step: usize,
    pub out_rate: u32,
    pub out_ch: usize,
    buf: Vec<i32>,
    phase: usize,
    /// Source frames still to drop (a seek with no table decodes from the top).
    skip: u64,
    pub eos: bool,
}

fn codec_name(c: audio_core::Codec) -> &'static str {
    match c {
        audio_core::Codec::Pcm => "pcm",
        audio_core::Codec::Flac => "flac",
        audio_core::Codec::Opus => "opus",
        audio_core::Codec::Vorbis => "vorbis",
        audio_core::Codec::Mp3 => "mp3",
        audio_core::Codec::Aac => "aac",
    }
}

impl Sound {
    pub fn new(dec: audio_core::Decoder) -> Result<Sound, String> {
        let info = dec.info();
        if info.rate == 0 || info.channels == 0 {
            return Err(String::from("audio: no rate or no channels"));
        }
        let step = (info.rate as usize).div_ceil(una_abi::AUDIO_RATE_MAX as usize).max(1);
        let out_rate = info.rate / step as u32;
        if out_rate < una_abi::AUDIO_RATE_MIN {
            return Err(format!("audio: rate {} under {}", info.rate, una_abi::AUDIO_RATE_MIN));
        }
        let ch = info.channels as usize;
        Ok(Sound {
            dec,
            codec: codec_name(info.codec),
            rate: info.rate,
            ch,
            step,
            out_rate,
            out_ch: if ch >= 2 { 2 } else { 1 },
            buf: alloc::vec![0; 2048 * step * ch],
            phase: 0,
            skip: 0,
            eos: false,
        })
    }

    /// Seek to `ms`: the decoder's own table when the format has one, else decode from the top and drop up to `ms`.
    /// `rebuild` reopens a decoder at the top (the caller's container share).
    pub fn seek(&mut self, ms: u64, rebuild: &dyn Fn() -> Option<audio_core::Decoder>) {
        self.eos = false;
        self.phase = 0;
        self.skip = 0;
        match self.dec.seek(ms) {
            Ok(Some(_)) => {}
            _ => {
                if let Some(d) = rebuild() {
                    self.dec = d;
                }
                self.skip = ms * self.rate as u64 / 1000;
            }
        }
    }

    /// Decode the next block into `pend` as interleaved 16-bit LE PCM at `out_rate` x `out_ch`, scaled by `gain`
    /// (Q8, 256 = unity; 0 = silence, which keeps the stream and its clock running). Frames appended, or 0 at the
    /// end (`eos` set).
    pub fn block(&mut self, pend: &mut Vec<u8>, gain: i32) -> usize {
        if self.eos {
            return 0;
        }
        let n = match self.dec.next_i32(&mut self.buf) {
            Ok(n) => n,
            Err(_) => 0,
        };
        if n == 0 {
            self.eos = true;
            return 0;
        }
        let ch = self.ch;
        let w0: i64 = if ch <= 2 { 32_768 } else { 65_536 / ch as i64 };
        let s0 = (self.skip.min(n as u64)) as usize;
        self.skip -= s0 as u64;
        let mut made = 0usize;
        for i in s0..n {
            let ph = self.phase;
            self.phase = (self.phase + 1) % self.step;
            if ph != 0 {
                continue;
            }
            let f = &self.buf[i * ch..i * ch + ch];
            let v = |k: usize| (f[k] >> 16) as i64;
            let (l, r) = if ch == 1 {
                (v(0), v(0))
            } else if ch == 2 {
                (v(0), v(1))
            } else {
                let rest: i64 = (2..ch).map(v).sum();
                ((v(0) * w0 + rest * w0 / 2) >> 15, (v(1) * w0 + rest * w0 / 2) >> 15)
            };
            let l = ((l * gain as i64) >> 8).clamp(-32_768, 32_767) as i16;
            let r = ((r * gain as i64) >> 8).clamp(-32_768, 32_767) as i16;
            pend.extend_from_slice(&l.to_le_bytes());
            if self.out_ch == 2 {
                pend.extend_from_slice(&r.to_le_bytes());
            }
            made += 1;
        }
        made
    }
}
