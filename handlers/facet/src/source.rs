// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Where decoded pixels come from — the ONE seam between Facet and a decoder.
//!
//! [`ImageSource`] has the shape of PIXELCORE (`unaos/libs/media/pixel_core`, SR25; the same crate the
//! host reaches as `gneiss_pal::dsp::image` and the kernel links by path): `sniff(&[u8]) -> Option<Format>`,
//! `decode(&[u8]) -> Result<Image, Error>`, an `Image` of straight 8-bit RGBA with optional composited
//! animation frames, a loop count, and the EXIF orientation REPORTED (not applied). [`Decoded`] mirrors
//! that `Image` field for field.
//!
//! [`PixelCoreSource`] — UnaOS's own decoders, written from the specifications, no third-party crate —
//! is the default and only product source (FACETPIXEL, SR43). Facet links `pixel_core` directly by path
//! rather than through `gneiss_pal`, whose default `std` feature drags in an HTTP stack Facet has no use
//! for; it is the same code either way.
//!
//! What pixel_core does not decode is REFUSED, by name, never silently handed to something else: a
//! container it recognises but cannot read (lossy WebP until VP8CORE, SR40) answers
//! [`SourceError::Decode`] carrying the format and pixel_core's reason; a container no UnaOS decoder
//! recognises yet (TIFF, ICO, AVIF, HEIF, JPEG XL, ...) answers [`SourceError::Foreign`] naming it
//! ([`unrecognised`]).

/// The formats [`ImageSource::sniff`] recognises (PIXELCORE's `Format`, same variants, same order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Qoi,
    WebP,
    /// SVG markup (pixel_core's `svg` route; the variant exists only when that feature is on, so the
    /// map below takes it through a catch-all — merge19: the workspace test build unifies the feature in).
    Svg,
}

impl Format {
    /// The lower-case container name Facet reports in `FacetImageInfo::format`.
    pub fn name(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpeg",
            Format::Gif => "gif",
            Format::Bmp => "bmp",
            Format::Qoi => "qoi",
            Format::WebP => "webp",
            Format::Svg => "svg",
        }
    }
}

impl From<pixel_core::Format> for Format {
    fn from(f: pixel_core::Format) -> Self {
        match f {
            pixel_core::Format::Png => Format::Png,
            pixel_core::Format::Jpeg => Format::Jpeg,
            pixel_core::Format::Gif => Format::Gif,
            pixel_core::Format::Bmp => Format::Bmp,
            pixel_core::Format::Qoi => Format::Qoi,
            pixel_core::Format::WebP => Format::WebP,
            #[allow(unreachable_patterns)]
            _ => Format::Svg, // `pixel_core::Format::Svg` under feature `svg`; absent otherwise
        }
    }
}

/// Identify a format by its magic bytes — PIXELCORE's own `sniff`, so Facet and its decoder can never
/// disagree on what a file IS.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    pixel_core::sniff(bytes).map(Format::from)
}

/// The name of a well-known image container that [`sniff`] does NOT accept, from its magic bytes —
/// so a refusal says "tiff", not "unknown". `None` when the bytes are not a container Facet can name.
pub fn foreign_format(b: &[u8]) -> Option<&'static str> {
    // ISO BMFF `ftyp` (ISO/IEC 14496-12 §4.3): major brand, then compatible brands.
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        let size = (u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize).clamp(12, b.len());
        let brands: Vec<&[u8]> = b[8..size].chunks_exact(4).enumerate().filter(|(i, _)| *i != 1).map(|(_, c)| c).collect();
        let has = |set: &[&[u8; 4]]| brands.iter().any(|br| set.iter().any(|s| *br == &s[..]));
        if has(&[b"avif", b"avis"]) {
            return Some("avif");
        }
        if has(&[b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1"]) {
            return Some("heif");
        }
        if has(&[b"jxl "]) {
            return Some("jpeg xl");
        }
        return None;
    }
    let text = b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(b);
    let text = &text[text.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(text.len())..];
    match b {
        [b'I', b'I', 0x2A, 0x00, ..] | [b'M', b'M', 0x00, 0x2A, ..] => Some("tiff"),
        [b'I', b'I', 0x2B, 0x00, ..] | [b'M', b'M', 0x00, 0x2B, ..] => Some("bigtiff"),
        [0x00, 0x00, 0x01, 0x00, n, _, ..] if *n > 0 => Some("ico"),
        [0x00, 0x00, 0x02, 0x00, n, _, ..] if *n > 0 => Some("cur"),
        [0xFF, 0x0A, ..] => Some("jpeg xl"),
        [0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' ', ..] => Some("jpeg xl"),
        [0x00, 0x00, 0x00, 0x0C, b'j', b'P', b' ', b' ', ..] | [0xFF, 0x4F, 0xFF, 0x51, ..] => Some("jpeg 2000"),
        [b'8', b'B', b'P', b'S', ..] => Some("psd"),
        [b'P', b'1'..=b'7', w, ..] if w.is_ascii_whitespace() => Some("pnm"),
        _ if text.starts_with(b"<svg") || (text.starts_with(b"<?xml") && text.windows(4).take(1024).any(|w| w == b"<svg")) => {
            Some("svg")
        }
        _ => None,
    }
}

/// The refusal for bytes [`sniff`] does not accept: [`SourceError::Foreign`] when the container has a
/// name, else [`SourceError::UnknownFormat`].
pub fn unrecognised(bytes: &[u8]) -> SourceError {
    foreign_format(bytes).map_or(SourceError::UnknownFormat, SourceError::Foreign)
}

/// One composited animation frame (PIXELCORE's `Frame`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub delay_ms: u32,
    /// `width * height * 4` straight RGBA bytes: the whole canvas while this frame is up.
    pub rgba: Vec<u8>,
}

/// A decoded image (PIXELCORE's `Image`, field for field).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` straight (non-premultiplied) RGBA, row-major, top row first. For an
    /// animation: the first composited frame.
    pub rgba: Vec<u8>,
    /// Every composited frame of an animation, else `None`.
    pub frames: Option<Vec<Frame>>,
    /// `Some(0)` loop forever, `Some(n)` n extra plays, `None` play once / still.
    pub loop_count: Option<u16>,
    /// EXIF orientation 1..=8 as the DECODER found it (1 when absent). Reported, NOT applied. Facet
    /// applies the orientation its own reader ([`crate::meta`]) finds — the authority, which also reads
    /// PNG `eXIf` and WebP `EXIF`; for JPEG the two are asserted to agree.
    pub orientation: u8,
}

impl From<pixel_core::Image> for Decoded {
    fn from(i: pixel_core::Image) -> Self {
        Decoded {
            width: i.width,
            height: i.height,
            rgba: i.rgba,
            frames: i.frames.map(|v| v.into_iter().map(|f| Frame { delay_ms: f.delay_ms, rgba: f.rgba }).collect()),
            loop_count: i.loop_count,
            orientation: i.orientation,
        }
    }
}

/// Why a source refused the bytes. Every refusal past [`SourceError::UnknownFormat`] names the format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// Nothing recognised the leading bytes.
    UnknownFormat,
    /// A known image container no UnaOS decoder reads yet (named: "tiff", "ico", "avif", ...).
    Foreign(&'static str),
    /// The format was recognised but this file was not decoded (lossy WebP, a malformed stream, ...).
    Decode { format: &'static str, reason: String },
    /// The claimed dimensions exceed [`MAX_DIM`] / [`MAX_PIXELS`] — refused before allocating.
    TooLarge { format: &'static str },
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::UnknownFormat => f.write_str("unknown format"),
            SourceError::Foreign(name) => write!(f, "{name}: no UnaOS decoder reads {name} yet (refused, no fallback)"),
            SourceError::Decode { format, reason } => write!(f, "{format}: {reason}"),
            SourceError::TooLarge { format } => write!(f, "{format}: dimensions too large"),
        }
    }
}

impl std::error::Error for SourceError {}

/// PIXELCORE's limits: the largest width or height, and the largest pixel count (256 MiB RGBA).
pub const MAX_DIM: u32 = pixel_core::MAX_DIM;
pub const MAX_PIXELS: u64 = pixel_core::MAX_PIXELS;

/// A decoder Facet can read pixels through. `Send + Sync` so one source serves every handle.
pub trait ImageSource: Send + Sync {
    /// Who decodes (reported in logs).
    fn name(&self) -> &'static str;
    /// Identify the container by magic bytes.
    fn sniff(&self, bytes: &[u8]) -> Option<Format> {
        sniff(bytes)
    }
    /// Decode the whole file to straight RGBA (every animation frame composited).
    fn decode(&self, bytes: &[u8]) -> Result<Decoded, SourceError>;
}

/// The default source: [`PixelCoreSource`].
pub fn default_source() -> Box<dyn ImageSource> {
    Box::new(PixelCoreSource)
}

/// PIXELCORE: PNG, JPEG (baseline + progressive), GIF (animated), BMP, QOI, WebP lossless — UnaOS's
/// own decoders, from the specifications, `no_std`, zero dependencies.
pub struct PixelCoreSource;

impl ImageSource for PixelCoreSource {
    fn name(&self) -> &'static str {
        "pixel_core"
    }

    fn decode(&self, bytes: &[u8]) -> Result<Decoded, SourceError> {
        let format = self.sniff(bytes).ok_or_else(|| unrecognised(bytes))?.name();
        pixel_core::decode(bytes).map(Decoded::from).map_err(|e| match e {
            pixel_core::Error::UnknownFormat => unrecognised(bytes),
            pixel_core::Error::TooLarge => SourceError::TooLarge { format },
            e => SourceError::Decode { format, reason: e.to_string() },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreign_containers_are_named() {
        let mut avif = vec![0, 0, 0, 0x1C];
        avif.extend(b"ftypavif\0\0\0\0avifmif1miaf");
        let mut heic = vec![0, 0, 0, 0x18];
        heic.extend(b"ftypheic\0\0\0\0mif1heic");
        let mut mp4 = vec![0, 0, 0, 0x14];
        mp4.extend(b"ftypisom\0\0\0\0mp41");
        let cases: &[(&[u8], Option<&str>)] = &[
            (b"II*\0\x08\0\0\0", Some("tiff")),
            (b"MM\0*\0\0\0\x08", Some("tiff")),
            (b"\0\0\x01\0\x01\0\x10\x10", Some("ico")),
            (b"\0\0\x02\0\x01\0\x10\x10", Some("cur")),
            (&avif, Some("avif")),
            (&heic, Some("heif")),
            (&mp4, None),
            (b"\xFF\x0A\xFA\x7F", Some("jpeg xl")),
            (b"\0\0\0\x0CJXL \r\n\x87\n", Some("jpeg xl")),
            (b"8BPS\0\x01", Some("psd")),
            (b"P6\n2 2\n255\n", Some("pnm")),
            (b"  <svg xmlns='http://www.w3.org/2000/svg'/>", Some("svg")),
            (b"<?xml version='1.0'?>\n<svg/>", Some("svg")),
            (b"<?xml version='1.0'?><html/>", None),
            (b"hello", None),
            (b"", None),
            (b"\0\0\x01\0\0\0", None),
        ];
        for (bytes, want) in cases {
            assert_eq!(foreign_format(bytes), *want, "{bytes:?}");
            assert_eq!(sniff(bytes), None);
            let e = PixelCoreSource.decode(bytes).unwrap_err();
            match want {
                Some(n) => {
                    assert_eq!(e, SourceError::Foreign(n));
                    assert!(e.to_string().starts_with(&format!("{n}: ")), "{e}");
                }
                None => assert_eq!(e, SourceError::UnknownFormat),
            }
        }
    }

    #[test]
    fn lossy_webp_is_refused_by_name() {
        // RIFF/WEBP with a `VP8 ` (lossy) chunk: pixel_core recognises it and refuses it (VP8CORE owed).
        let mut b = b"RIFF\x1A\0\0\0WEBPVP8 \x0E\0\0\0".to_vec();
        b.extend([0x30, 0x01, 0x00, 0x9D, 0x01, 0x2A, 0x01, 0x00, 0x01, 0x00, 0, 0, 0, 0]);
        assert_eq!(sniff(&b), Some(Format::WebP));
        let e = PixelCoreSource.decode(&b).unwrap_err();
        assert!(matches!(&e, SourceError::Decode { format: "webp", reason } if reason.contains("lossy")), "{e:?}");
        assert!(e.to_string().starts_with("webp: "), "{e}");
    }

    #[test]
    fn decode_errors_carry_the_format() {
        let png = crate::png::encode(3, 2, &[7u8; 24]);
        let d = PixelCoreSource.decode(&png).unwrap();
        assert_eq!((d.width, d.height, d.rgba, d.frames, d.loop_count, d.orientation), (3, 2, vec![7u8; 24], None, None, 1));
        let e = PixelCoreSource.decode(&png[..png.len() - 20]).unwrap_err();
        assert!(matches!(e, SourceError::Decode { format: "png", .. }), "{e:?}");
        // A JPEG cut off after SOI/APP0 names jpeg.
        let e = PixelCoreSource.decode(b"\xFF\xD8\xFF\xE0\0\x10JFIF\0").unwrap_err();
        assert!(e.to_string().starts_with("jpeg: "), "{e}");
    }

    #[test]
    fn oversized_claims_are_refused_before_allocating() {
        // A QOI header claiming 65536 x 65536 (past MAX_PIXELS), no chunks, then the end marker.
        let mut q = b"qoif".to_vec();
        q.extend(65536u32.to_be_bytes());
        q.extend(65536u32.to_be_bytes());
        q.extend([4, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        let e = PixelCoreSource.decode(&q).unwrap_err();
        assert_eq!(e, SourceError::TooLarge { format: "qoi" });
        assert_eq!(e.to_string(), "qoi: dimensions too large");
    }
}
