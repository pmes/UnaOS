// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `dsp::video::av1` — the host face of AVCODEC (LEDGER SR24, `unaos/libs/media/av1_core`), the
//! from-specification AV1 decoder, behind PLAYBACK's [`VideoDecoder`] seam (SR26).
//!
//! Fold wiring (this file is written against PLAYBACK's `dsp/video.rs`, which declares
//! `#[cfg(feature = "av1")] pub mod av1;` and the registry arm `Codec::Av1 => Av1Decoder::new`):
//! in `libs/gneiss_pal/Cargo.toml` set `av1 = ["dep:av1_core"]` and add
//! `av1_core = { path = "../../unaos/libs/media/av1_core", optional = true, features = ["std"] }`.
//!
//! Honest ceiling (AVCODEC2): key, intra-only, inter and switch frames, show_existing_frame,
//! intra block copy and scalable (temporal / spatial layer) streams decode frame-exact against
//! libaom's per-frame MD5s (docs/dev/evidence/media-1004/AVCODEC2.md); superres returns
//! `DecodeError::Unsupported`, and film grain synthesis is reported in that doc. The decoder keeps
//! the reference frames across packets; `reset()` (a seek) drops them. AV1PLAYERCOLOUR (SR347): frames leave as the decoded
//! planes (`Pixels::I420` at 8 bits, `I420Hi` deeper; mono with neutral chroma; 4:4:4 / 4:2:2 refused, COLOURONE-422), so
//! `Frame::to_rgba_colour` converts them by `Track::colour` and the size default, bilinear (`chroma_of`), as the kernel does.

use super::{DecodeError, Frame, Pixels, VideoDecoder};
use crate::dsp::demux::{Packet, Track};
use av1_core::image::{Planes, StreamDecoder};

pub struct Av1Decoder {
    inner: StreamDecoder,
    timebase: crate::dsp::demux::Timebase,
    open: Option<crate::dsp::demux::colour::Colour>, // AV1INBANDSWITCH (SR373): the open's colour (`Track::colour`), what a header switch merges under
}

fn map_err(e: av1_core::Error) -> DecodeError {
    match e {
        av1_core::Error::Unsupported(s) => DecodeError::Unsupported(format!("AV1 {s} (AVCODEC: owed)")),
        other => DecodeError::Corrupt(format!("AV1: {other}")),
    }
}

impl Av1Decoder {
    /// `track.config` is the av1C record (MP4) or CodecPrivate (Matroska); empty is allowed when
    /// the stream carries its own sequence header.
    pub fn new(track: &Track) -> Result<Av1Decoder, DecodeError> {
        Ok(Av1Decoder { inner: StreamDecoder::new(&track.config).map_err(map_err)?, timebase: track.timebase, open: track.colour })
    }
}

impl VideoDecoder for Av1Decoder {
    fn name(&self) -> &'static str {
        "av1 (AVCODEC)"
    }
    fn decode(&mut self, pkt: &Packet) -> Result<Option<Frame>, DecodeError> {
        let planes = self.inner.decode_temporal_unit(&pkt.data).map_err(map_err)?;
        Ok(Some(self.inband(planes_frame(planes, self.timebase.to_ns(pkt.pts))?))) // AV1INBANDSWITCH (SR373): a mid-stream header switch re-picks
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
}

/// AV1PLAYERCOLOUR (LEDGER SR347): decoded planes -> [`Frame`], the `h264::to_frame` shape: 8-bit 4:2:0 is `Pixels::I420`
/// (the u16 samples narrowed, lossless at 8 bits), deeper is `Pixels::I420Hi` with the sequence header's depth; luma
/// `width` per row, chroma `chroma_width()` (odd sizes as `av1_core::image::picture`). Monochrome (Main profile) fills both
/// chroma planes at 1 << (depth - 1), yuv_core's chroma offset at either range, so every Kr matrix gives the kernel's mono
/// grey. 4:4:4 / 4:2:2 (profiles 1 / 2) are refused by name: the frame contract carries 4:2:0 only (COLOURONE-422).
fn planes_frame(p: Planes, pts_ns: i64) -> Result<Frame, DecodeError> {
    if !p.mono && (p.ss_x, p.ss_y) != (1, 1) {
        let s = if p.ss_x == 0 && p.ss_y == 0 { "4:4:4" } else { "4:2:2" };
        return Err(DecodeError::Unsupported(format!("AV1 {s} chroma (COLOURONE-422: the frame contract carries 4:2:0 only)")));
    }
    let (y_stride, uv_stride) = (p.width as usize, ((p.width + 1) >> 1) as usize);
    let (u, v) = if p.mono {
        let n = uv_stride * ((p.height + 1) >> 1) as usize;
        let mid = 1u16 << (p.bit_depth - 1);
        (vec![mid; n], vec![mid; n])
    } else {
        (p.u, p.v)
    };
    let pixels = if p.bit_depth <= 8 {
        let n8 = |s: Vec<u16>| s.into_iter().map(|x| x as u8).collect::<Vec<u8>>();
        Pixels::I420 { y: n8(p.y), u: n8(u), v: n8(v), y_stride, uv_stride }
    } else {
        Pixels::I420Hi { y: p.y, u, v, y_stride, uv_stride, bit_depth: p.bit_depth as u8 }
    };
    Ok(Frame { width: p.width, height: p.height, pts_ns, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::demux::colour::Colour;
    use crate::dsp::video::Chroma;
    use av1_core::image::{planes_to_rgba, Upsampling};

    fn planes(w: u32, h: u32, n: u32, (ss_x, ss_y): (u32, u32), mono: bool) -> Planes {
        let (cw, ch) = (((w + ss_x) >> ss_x) as usize, ((h + ss_y) >> ss_y) as usize);
        let top = (1u32 << n) - 1;
        // gradients, so bilinear differs from nearest and every matrix term is exercised
        let y = (0..w as usize * h as usize).map(|i| ((i as u32 * 37 + 16) % top) as u16).collect();
        let c = |k: u32| if mono { Vec::new() } else { (0..cw * ch).map(|i| ((i as u32 * k + 64) % top) as u16).collect() };
        Planes {
            width: w,
            height: h,
            bit_depth: n,
            mono,
            ss_x,
            ss_y,
            y,
            u: c(53),
            v: c(29),
            color_primaries: 2,
            transfer_characteristics: 2,
            matrix_coefficients: 2,
            full_range: false,
            seq: Default::default(),
            frame: Default::default(),
            stats: Default::default(),
        }
    }

    fn colour(matrix: u8, full: Option<bool>) -> Colour {
        Colour { primaries: 2, transfer: 2, matrix, full_range: full, mastering: None, light: None }
    }

    /// The kernel Player's `colour_rgba_av1` (vplay.rs), written out: the pick by the track's colour, av1_core's picture
    /// with the pick's range, bilinear chroma.
    fn kernel(p: &Planes, c: Option<&Colour>) -> Vec<u8> {
        let (m, full) = yuv_core::pick(c.map(|c| c.matrix), c.and_then(|c| c.full_range), p.width, p.height);
        let pic = yuv_core::Picture { full, ..av1_core::image::picture(p) };
        let mut out = vec![0u8; pic.w * pic.h * 4];
        yuv_core::to_rgba(&pic, m, Chroma::Bilinear, false, &mut out);
        out
    }

    #[test]
    fn av1playercolour_planes_convert_as_the_kernel_does() {
        let cs = [None, Some(colour(2, None)), Some(colour(1, Some(false))), Some(colour(6, Some(true)))];
        let mut n = 0;
        for depth in [8, 10] {
            for (w, h) in [(5, 3), (1281, 2)] {
                let p = planes(w, h, depth, (1, 1), false);
                let f = planes_frame(p.clone(), 0).unwrap();
                assert!(matches!((&f.pixels, depth), (Pixels::I420 { .. }, 8) | (Pixels::I420Hi { bit_depth: 10, .. }, 10)));
                for c in &cs {
                    assert_eq!(f.to_rgba_colour(c.as_ref(), Chroma::Bilinear), kernel(&p, c.as_ref()), "{w}x{h} {depth}-bit {c:?}");
                    n += 1;
                }
            }
        }
        // Untagged HD: the size default (BT.709) now applies; the retired path's stills pick gave BT.601.
        let p = planes(1281, 2, 8, (1, 1), false);
        let f = planes_frame(p.clone(), 0).unwrap();
        let now = f.to_rgba_colour(None, Chroma::Bilinear);
        assert_eq!(now, f.to_rgba_colour(Some(&colour(1, Some(false))), Chroma::Bilinear));
        assert_ne!(now, planes_to_rgba(&p, Upsampling::Bilinear).rgba);
        assert_eq!(f.to_rgba_colour(Some(&colour(6, Some(false))), Chroma::Bilinear), planes_to_rgba(&p, Upsampling::Bilinear).rgba);
        println!("av1playercolour: planes == kernel colour_rgba_av1 ({n} cases: 8/10-bit, SD/HD, 4 colours); untagged HD bt709");
    }

    #[test]
    fn av1playercolour_mono_fill_equals_the_mono_path() {
        for depth in [8, 10] {
            let p = planes(5, 3, depth, (1, 1), true);
            let f = planes_frame(p.clone(), 0).unwrap();
            for c in [None, Some(colour(1, Some(false))), Some(colour(6, Some(true)))] {
                let k = kernel(&p, c.as_ref());
                assert!(k.chunks_exact(4).all(|q| q[0] == q[1] && q[1] == q[2]), "the kernel's mono is grey");
                assert_eq!(f.to_rgba_colour(c.as_ref(), Chroma::Bilinear), k, "{depth}-bit {c:?} bilinear");
                assert_eq!(f.to_rgba_colour(c.as_ref(), Chroma::Nearest), k, "{depth}-bit {c:?} nearest");
            }
        }
    }

    #[test]
    fn av1playercolour_444_and_422_refused_by_name() {
        for (ss, name) in [((0, 0), "4:4:4"), ((1, 0), "4:2:2")] {
            match planes_frame(planes(4, 2, 8, ss, false), 0) {
                Err(DecodeError::Unsupported(s)) => assert!(s.contains(name) && s.contains("COLOURONE-422"), "{s}"),
                other => panic!("{name}: {other:?}"),
            }
        }
    }
}

// ── AV1INBANDSWITCH (LEDGER SR373): a new sequence header mid-stream re-picks the colour (handlers.md §5 NLEPROBE-HDR) ──────────
// `av1_core::image::StreamDecoder::switch` is the ONE rule (the kernel Player's `inband_colour` calls it too). While the current
// header's code points equal the open's, the frame leaves as planes, byte-identical; once they differ the frame leaves CONVERTED
// by the switched colour through the ONE conversion (`Frame::to_rgba_colour`, AV1's bilinear chroma as `chroma_of`), so the bus
// sink, the editor's `src_rgba` and play-check all pass it through. Wire: `[STRIA] :: video colour switch at <pts_ns>
// matrix=<a>-><b> range=<full|limited>`, once per change.
impl Av1Decoder {
    fn inband(&mut self, f: Frame) -> Frame {
        use crate::dsp::demux::colour::Colour;
        let open = self.open.map(|c| (c.primaries, c.transfer, c.matrix, c.full_range));
        let (now, changed) = self.inner.switch(open);
        let range = |r: Option<bool>| if r == Some(true) { "full" } else { "limited" };
        if let Some((a, b)) = changed {
            log::info!("[STRIA] :: video colour switch at {} matrix={}->{} range={}", f.pts_ns, a, b, range(now.map_or(self.open.and_then(|c| c.full_range), |n| n.3)));
        }
        let Some((primaries, transfer, matrix, full_range)) = now else { return f };
        let c = Colour { primaries, transfer, matrix, full_range, mastering: self.open.and_then(|c| c.mastering), light: self.open.and_then(|c| c.light) };
        let rgba = f.to_rgba_colour(Some(&c), super::Chroma::Bilinear);
        Frame { pixels: Pixels::Rgba(rgba), ..f }
    }
}
