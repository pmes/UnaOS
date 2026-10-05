// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `pixel_core` — the still-image decoders UnaOS owns (PIXELCORE, ledger SR25).
//!
//! One API over six formats, every one written from its specification with no third-party decoder
//! underneath (Peter, 2026-10-04: "true cutting edge, not chicken wire"):
//!
//! | format | spec | module |
//! |---|---|---|
//! | PNG  | ISO/IEC 15948 (W3C PNG 3rd ed.) + RFC 1950/1951 | [`png`], [`inflate`] |
//! | JPEG | ITU-T T.81 (baseline + progressive Huffman), JFIF 1.02, EXIF 2.3 orientation | [`jpeg`] |
//! | GIF  | GIF89a (W3C/CompuServe 1990) | [`gif`] |
//! | BMP  | Windows BITMAPINFOHEADER family | [`bmp`] |
//! | QOI  | qoiformat.org specification 1.0 | [`qoi`] |
//! | WebP | RFC 9649 — the VP8L lossless bitstream; lossy VP8 + ALPH through `vp8_core` (RFC 6386) | [`webp`] |
//!
//! Output is always straight (non-premultiplied) 8-bit RGBA, row-major, top-down. Animated formats
//! (GIF, animated WebP, APNG) additionally carry every fully composited canvas in [`Image::frames`].
//!
//! `#![no_std]` + `alloc`, `#![forbid(unsafe_code)]`, no dependencies: the kernel links this crate by
//! path (its `video/png.rs` and `selfhost/inflate.rs` are re-exports of [`png::encode`] and [`inflate`]),
//! and the host (Aether, `tools/pixel-check`) links the very same code.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::vec::Vec;

pub mod anim;
pub mod bmp;
pub mod crc;
pub mod gif;
pub mod inflate;
pub mod jpeg;
pub mod png;
pub mod qoi;
#[cfg(feature = "svg")]
pub mod svg;
pub mod webp;

pub use anim::{Animation, FrameInfo, decode_first_frame};

/// One composited animation frame: the whole canvas as it looks while this frame is displayed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// How long the frame stays up, in milliseconds (GIF stores centiseconds; 0 is kept as 0 and
    /// left to the player to clamp, the way browsers clamp it to 100 ms).
    pub delay_ms: u32,
    /// `width * height * 4` straight RGBA bytes.
    pub rgba: Vec<u8>,
}

/// A decoded image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` straight RGBA bytes (for an animation: the FIRST composited frame).
    pub rgba: Vec<u8>,
    /// Every composited frame of an animation (GIF, animated WebP or APNG with more than one frame),
    /// else `None`.
    pub frames: Option<Vec<Frame>>,
    /// Animation loop count: `Some(0)` = loop forever (NETSCAPE2.0 with count 0), `Some(n)` = play
    /// `n` extra times, `None` = no loop extension (play once). Always `None` for stills.
    pub loop_count: Option<u16>,
    /// EXIF orientation tag (1..=8) found in the file, 1 when absent. It is REPORTED, not applied:
    /// call [`Image::apply_orientation`] to rotate/flip the pixels (a browser applies it by default,
    /// CSS `image-orientation: from-image`).
    pub orientation: u8,
}

/// Why a decode failed. Every variant names a distinct falsifiable claim about the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The input ended before the format said it would.
    Truncated,
    /// No decoder recognised the leading bytes.
    UnknownFormat,
    /// A legal file using a feature this crate does not decode (named).
    Unsupported(&'static str),
    /// The bytes break the specification (named).
    Malformed(&'static str),
    /// The DEFLATE/zlib layer refused the stream.
    Inflate(inflate::InflateError),
    /// A checksum in the container disagrees with the bytes it covers.
    Checksum(&'static str),
    /// The claimed dimensions exceed [`MAX_DIM`] / [`MAX_PIXELS`] — refused before allocating.
    TooLarge,
    /// The allocator declined an image-sized buffer. Every image-sized allocation goes through
    /// `try_reserve` so a kernel heap (48 MiB) answers with this instead of an allocation panic.
    OutOfMemory,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => f.write_str("truncated"),
            Error::UnknownFormat => f.write_str("unknown format"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::Malformed(s) => write!(f, "malformed: {s}"),
            Error::Inflate(e) => write!(f, "inflate: {}", inflate::inflate_reason(*e)),
            Error::Checksum(s) => write!(f, "checksum mismatch: {s}"),
            Error::TooLarge => f.write_str("dimensions too large"),
            Error::OutOfMemory => f.write_str("out of memory"),
        }
    }
}

/// Largest width or height any decoder accepts.
pub const MAX_DIM: u32 = 1 << 16;
/// Largest pixel count any decoder accepts (256 MiB of RGBA). Every claimed size is multiplied out
/// and checked against this BEFORE a buffer is reserved — the dimensions are attacker-shaped input.
pub const MAX_PIXELS: u64 = 1 << 26;

/// The formats [`sniff`] recognises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Qoi,
    WebP,
    /// SVG markup, rendered by svg_core (feature `svg`).
    #[cfg(feature = "svg")]
    Svg,
}

/// Identify a format by its magic bytes.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(Format::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Format::Gif)
    } else if bytes.starts_with(b"BM") && bytes.len() >= 26 {
        Some(Format::Bmp)
    } else if bytes.starts_with(b"qoif") {
        Some(Format::Qoi)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(Format::WebP)
    } else {
        #[cfg(feature = "svg")]
        if svg_core::sniff_svg(bytes) {
            return Some(Format::Svg);
        }
        None
    }
}

/// Decode any supported format, chosen by [`sniff`].
pub fn decode(bytes: &[u8]) -> Result<Image, Error> {
    match sniff(bytes).ok_or(Error::UnknownFormat)? {
        Format::Png => decode_png(bytes),
        Format::Jpeg => decode_jpeg(bytes),
        Format::Gif => decode_gif(bytes),
        Format::Bmp => decode_bmp(bytes),
        Format::Qoi => decode_qoi(bytes),
        Format::WebP => decode_webp(bytes),
        #[cfg(feature = "svg")]
        Format::Svg => svg::decode(bytes),
    }
}

pub fn decode_png(bytes: &[u8]) -> Result<Image, Error> {
    png::decode(bytes)
}
pub fn decode_jpeg(bytes: &[u8]) -> Result<Image, Error> {
    jpeg::decode(bytes)
}
pub fn decode_gif(bytes: &[u8]) -> Result<Image, Error> {
    gif::decode(bytes)
}
pub fn decode_bmp(bytes: &[u8]) -> Result<Image, Error> {
    bmp::decode(bytes)
}
pub fn decode_qoi(bytes: &[u8]) -> Result<Image, Error> {
    qoi::decode(bytes)
}
pub fn decode_webp(bytes: &[u8]) -> Result<Image, Error> {
    webp::decode(bytes)
}

impl Image {
    /// A still image.
    pub(crate) fn still(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Image { width, height, rgba, frames: None, loop_count: None, orientation: 1 }
    }

    /// Rotate/flip the pixels (and every frame) so the image displays upright per its EXIF
    /// orientation tag (EXIF 2.3 §4.6.4 A, tag 0x0112), then reset `orientation` to 1.
    /// Orientations 5..=8 swap width and height.
    pub fn apply_orientation(&mut self) {
        let o = self.orientation;
        if !(2..=8).contains(&o) {
            self.orientation = 1;
            return;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let map = |src: &[u8]| -> Vec<u8> {
            let mut out = alloc::vec![0u8; src.len()];
            let (ow, _oh) = if o >= 5 { (h, w) } else { (w, h) };
            for y in 0..h {
                for x in 0..w {
                    // (x, y) in the stored image goes to (dx, dy) in the displayed one.
                    let (dx, dy) = match o {
                        2 => (w - 1 - x, y),
                        3 => (w - 1 - x, h - 1 - y),
                        4 => (x, h - 1 - y),
                        5 => (y, x),
                        6 => (h - 1 - y, x),
                        7 => (h - 1 - y, w - 1 - x),
                        _ => (y, w - 1 - x), // 8
                    };
                    let s = (y * w + x) * 4;
                    let d = (dy * ow + dx) * 4;
                    out[d..d + 4].copy_from_slice(&src[s..s + 4]);
                }
            }
            out
        };
        self.rgba = map(&self.rgba);
        if let Some(frames) = self.frames.as_mut() {
            for f in frames.iter_mut() {
                f.rgba = map(&f.rgba);
            }
        }
        if o >= 5 {
            core::mem::swap(&mut self.width, &mut self.height);
        }
        self.orientation = 1;
    }
}

/// Integer non-premultiplied `src-over` of one straight-RGBA pixel onto another — libwebp
/// `anim_decode.c` `BlendPixelNonPremult` and Blink `ImageFrame::BlendSrcOverDstRaw`, bit for bit:
/// `dst_factor = dst_a * (256 - src_a) >> 8`, `out_a = src_a + dst_factor`, each colour channel
/// `(src * src_a + dst * dst_factor) * floor(2^24 / out_a) >> 24`. A fully transparent source leaves
/// the destination as it is. Shared by animated WebP (`ANMF` blend) and APNG (`APNG_BLEND_OP_OVER`).
pub fn blend_nonpremult(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    let sa = src[3] as u32;
    if sa == 0 {
        return dst;
    }
    let dfa = (dst[3] as u32 * (256 - sa)) >> 8;
    let ba = sa + dfa;
    let scale = (1u32 << 24) / ba;
    let ch = |s: u8, d: u8| (((s as u32 * sa + d as u32 * dfa) as u64 * scale as u64) >> 24) as u8;
    [ch(src[0], dst[0]), ch(src[1], dst[1]), ch(src[2], dst[2]), ba as u8]
}

/// Float `src-over` of one straight-RGBA pixel onto another, the way Chromium's PNG path (Skia's
/// `SkPngRustCodec`, blending through `SkRasterPipeline` in single precision) composites
/// `APNG_BLEND_OP_OVER`, operation for operation: bytes load as `c * (1/255)`, both pixels are
/// premultiplied, `out = src + dst * (1 - src_a)`, unpremultiplied by multiplying with `1 / out_a`,
/// and each channel stores as `v * 255` rounded half-to-even (`_mm_cvtps_epi32`). It is NOT Blink's
/// WebP integer blend ([`blend_nonpremult`]): the oracle tells the two apart (APNG frames blended over
/// translucent pixels differ by up to 26 between them), and the rounding details were fixed against
/// 14 452 distinct (source, destination) pairs Chromium blended in the APNG corpus, all equal.
pub fn blend_srcover_f32(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    const R255: f32 = 1.0 / 255.0;
    let ld = |c: u8| c as f32 * R255;
    let (sa, da) = (ld(src[3]), ld(dst[3]));
    let inv = 1.0 - sa;
    let oa = sa + da * inv;
    if oa <= 0.0 {
        return [0, 0, 0, 0];
    }
    let scale = 1.0 / oa;
    // Round half to even in f32: adding 2^23 leaves no fraction bits (valid for 0 <= v <= 255).
    let to8 = |v: f32| ((v.clamp(0.0, 1.0) * 255.0 + 8_388_608.0) - 8_388_608.0) as u8;
    let ch = |s: u8, d: u8| to8((ld(s) * sa + ld(d) * da * inv) * scale);
    [ch(src[0], dst[0]), ch(src[1], dst[1]), ch(src[2], dst[2]), to8(oa)]
}

/// Map a "number of plays" field (APNG `num_plays`, WebP ANIM `Loop Count`: 0 = forever) onto
/// [`Image::loop_count`]'s GIF meaning (extra repetitions, `Some(0)` = forever) the way Blink does:
/// 0 -> forever, 1 -> play once (`None`), n -> `n - 1` repetitions.
pub(crate) fn loop_from_plays(plays: u32) -> Option<u16> {
    match plays {
        0 => Some(0),
        1 => None,
        n => Some((n - 1).min(u16::MAX as u32) as u16),
    }
}

/// Check claimed dimensions and return the RGBA byte length, or refuse.
pub(crate) fn rgba_len(width: u32, height: u32) -> Result<usize, Error> {
    if width == 0 || height == 0 {
        return Err(Error::Malformed("zero dimension"));
    }
    if width > MAX_DIM || height > MAX_DIM || (width as u64) * (height as u64) > MAX_PIXELS {
        return Err(Error::TooLarge);
    }
    Ok(width as usize * height as usize * 4)
}

/// A zeroed buffer of `len` bytes, reserved fallibly.
pub(crate) fn zeroed(len: usize) -> Result<Vec<u8>, Error> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| Error::OutOfMemory)?;
    v.resize(len, 0);
    Ok(v)
}

/// A whole-slice [`inflate::ByteSource`].
pub(crate) struct SliceSource<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SliceSource<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
}

impl inflate::ByteSource for SliceSource<'_> {
    #[inline]
    fn next(&mut self) -> Option<u8> {
        let b = self.data.get(self.pos).copied();
        self.pos += 1;
        b
    }
}

/// A [`inflate::Sink`] that keeps the first `cap` bytes and counts (but drops) any excess, so a
/// stream with trailing garbage past the image still gets its Adler-32 checked.
pub(crate) struct CappedSink {
    pub(crate) out: Vec<u8>,
    cap: usize,
}

impl inflate::Sink for CappedSink {
    #[inline]
    fn push(&mut self, byte: u8) -> Result<(), ()> {
        if self.out.len() < self.cap {
            self.out.push(byte);
        }
        Ok(())
    }
}

/// Inflate one zlib stream (RFC 1950) into at most `cap` bytes, through the ONE inflater
/// ([`inflate::zlib_inflate`]) the kernel also uses.
pub fn zlib_decompress(data: &[u8], cap: usize) -> Result<Vec<u8>, Error> {
    let mut src = SliceSource::new(data);
    let mut sink = CappedSink { out: Vec::new(), cap };
    sink.out.try_reserve_exact(cap.min(1 << 28)).map_err(|_| Error::OutOfMemory)?;
    inflate::zlib_inflate(&mut src, &mut sink).map_err(Error::Inflate)?;
    Ok(sink.out)
}

/// Read a big-endian u32 at `i` (caller has bounds-checked).
#[inline]
pub(crate) fn be32(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
#[inline]
pub(crate) fn le32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
#[inline]
pub(crate) fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
