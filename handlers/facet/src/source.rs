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
//! recognises yet (TIFF, ICO, AVIF and HEIF sequences, JPEG XL, ...) answers [`SourceError::Foreign`] naming it
//! ([`unrecognised`]).
//!
//! HEVCHEIC (SR220): an HEIC still ([`Format::Heic`]: `ftyp` heic / heix, or mif1 without AVIF) is the one container
//! pixel_core does not own: it decodes through `gneiss_pal::dsp::image::decode_heif` (demux_core's HEIF reader +
//! hevc_core, 8- and 10-bit intra, COLOURONE SR328), and its refusals name `heic` the same way.
//! HEIFONE (SR303): an AVIF still ([`Format::Avif`]: `ftyp` avif) decodes through the same `decode_heif` (demux_core's
//! HEIF reader + av1_core), grids included; its refusals name `avif`.

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
    /// HEVCHEIC (SR220): an HEIF still holding HEVC images (not a pixel_core format: [`sniff`] adds it).
    Heic,
    /// HEIFONE (SR303): an AVIF still (an HEIF still holding AV1 images; not a pixel_core format: [`sniff`] adds it).
    Avif,
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
            Format::Heic => "heic",
            Format::Avif => "avif",
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
/// disagree on what a file IS; then demux_core's HEIC and AVIF brand tests (HEVCHEIC SR220, HEIFONE SR303).
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    use gneiss_pal::dsp::image::{is_avif, is_heic};
    pixel_core::sniff(bytes)
        .map(Format::from)
        .or_else(|| is_heic(bytes).then_some(Format::Heic))
        .or_else(|| is_avif(bytes).then_some(Format::Avif))
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
        let decoded = if format == "heic" || format == "avif" { gneiss_pal::dsp::image::decode_heif(bytes) } else { pixel_core::decode(bytes) };
        decoded.map(Decoded::from).map_err(|e| match e {
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
        // HEIFONE (SR303): an AVIF still is Facet's own now (avif_stills_decode_through_heif); an AVIF image SEQUENCE
        // (avis without the avif brand) stays foreign.
        let mut avif = vec![0, 0, 0, 0x18];
        avif.extend(b"ftypavis\0\0\0\0avismsf1");
        // HEVCHEIC (SR220): a heic still is Facet's own now (heic_is_sniffed_and_refusals_name_heic); an HEIF image
        // SEQUENCE stays foreign.
        let mut heis = vec![0, 0, 0, 0x18];
        heis.extend(b"ftypmsf1\0\0\0\0heismsf1");
        let mut mp4 = vec![0, 0, 0, 0x14];
        mp4.extend(b"ftypisom\0\0\0\0mp41");
        let cases: &[(&[u8], Option<&str>)] = &[
            (b"II*\0\x08\0\0\0", Some("tiff")),
            (b"MM\0*\0\0\0\x08", Some("tiff")),
            (b"\0\0\x01\0\x01\0\x10\x10", Some("ico")),
            (b"\0\0\x02\0\x01\0\x10\x10", Some("cur")),
            (&avif, Some("avif")),
            (&heis, Some("heif")),
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
        // pixel_core sniffs SVG itself when its `svg` feature is on (a workspace test build unifies it in through
        // svg_core); then the two svg rows are NATIVE, not foreign, and the facet table's answer is unused.
        let svg_native = sniff(b"<svg xmlns='http://www.w3.org/2000/svg'/>").is_some();
        for (bytes, want) in cases {
            assert_eq!(foreign_format(bytes), *want, "{bytes:?}");
            if svg_native && *want == Some("svg") {
                continue;
            }
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
    fn truncated_lossy_webp_is_refused_by_name() {
        // RIFF/WEBP with a `VP8 ` (lossy) chunk: pixel_core decodes lossy WebP through vp8_core since VIDEOPLAYER
        // (B434); this 14-byte frame is cut short, so the refusal names the format and reads `truncated`.
        let mut b = b"RIFF\x1A\0\0\0WEBPVP8 \x0E\0\0\0".to_vec();
        b.extend([0x30, 0x01, 0x00, 0x9D, 0x01, 0x2A, 0x01, 0x00, 0x01, 0x00, 0, 0, 0, 0]);
        assert_eq!(sniff(&b), Some(Format::WebP));
        let e = PixelCoreSource.decode(&b).unwrap_err();
        assert!(matches!(&e, SourceError::Decode { format: "webp", reason } if reason.contains("truncated")), "{e:?}");
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

    #[test]
    fn heic_is_sniffed_and_refusals_name_heic() {
        let mut heic = vec![0, 0, 0, 0x18];
        heic.extend(b"ftypheic\0\0\0\0mif1heic");
        assert_eq!(foreign_format(&heic), Some("heif"), "the name table still knows the container");
        assert_eq!(sniff(&heic), Some(Format::Heic));
        // An ftyp and nothing else: no meta box, refused by name as heic.
        let e = PixelCoreSource.decode(&heic).unwrap_err();
        assert!(matches!(&e, SourceError::Decode { format: "heic", reason } if reason.contains("no meta")), "{e:?}");
        assert!(e.to_string().starts_with("heic: "), "{e}");
    }

    /// COLOURONE (SR328): Facet opens a 10-bit (Main 10) HEIC, converted from its 16-bit samples (HEVCHEIC-MAIN10).
    #[test]
    fn colourone_facet_opens_a_ten_bit_heic() {
        use gneiss_pal::dsp::image::{decode_heif, heic_kat};
        let bytes = heic_kat::main10_heic();
        assert_eq!(sniff(&bytes), Some(Format::Heic));
        let d = PixelCoreSource.decode(&bytes).unwrap();
        assert_eq!((d.width, d.height, d.frames.is_none()), (32, 32, true));
        assert_eq!(d.rgba, decode_heif(&bytes).unwrap().rgba);
        assert!(d.rgba.chunks_exact(4).all(|p| p == [87, 145, 176, 255]), "{:?}", &d.rgba[..4]);
        let (_, info) = crate::Facet::new().open_bytes("main10.heic", &bytes).unwrap();
        assert_eq!((info.width, info.height), (32, 32));
    }

    #[test]
    fn heic_stills_decode_through_hevc_core() {
        use gneiss_pal::dsp::image::{decode_heif, heic_kat};
        let mut f = crate::Facet::new();
        for (bytes, w, h) in [(heic_kat::flat_heic(), 64, 48), (heic_kat::pcm_heic(), 64, 48), (heic_kat::grid_2x2_heic(), 120, 90)] {
            let d = PixelCoreSource.decode(&bytes).unwrap();
            assert_eq!((d.width, d.height, d.frames.is_none(), d.loop_count, d.orientation), (w, h, true, None, 1));
            assert_eq!(d.rgba, decode_heif(&bytes).unwrap().rgba);
            let (_, info) = f.open_bytes("kat.heic", &bytes).unwrap();
            assert_eq!((info.format.as_str(), info.width, info.height), ("heic", w, h));
        }
        // The flat picture: Y = Cb = Cr = 128, BT.601 limited -> grey 130.
        let flat = PixelCoreSource.decode(&heic_kat::flat_heic()).unwrap();
        assert!(flat.rgba.chunks_exact(4).all(|p| p == [130, 130, 130, 255]));
        // The PCM picture's gradient: luma 7x + 3y at the top-left block, e.g. (0,0) Y=0 and (31,31) Y=310 mod 256.
        let pcm = PixelCoreSource.decode(&heic_kat::pcm_heic()).unwrap();
        let grid = PixelCoreSource.decode(&heic_kat::grid_2x2_heic()).unwrap();
        // The grid's top-right quadrant is the PCM tile, its bottom-right the flat one.
        let at = |d: &Decoded, x: usize, y: usize| d.rgba[(y * d.width as usize + x) * 4..][..4].to_vec();
        for (x, y) in [(0, 0), (31, 31), (10, 40), (55, 47)] {
            assert_eq!(at(&grid, 64 + x, y), at(&pcm, x, y), "({x},{y})");
            assert_eq!(at(&grid, x, 48 + y.min(41)), at(&pcm, x, y.min(41)));
            assert_eq!(at(&grid, 64 + x, 48 + y.min(41)), at(&flat, x, y.min(41)));
        }
        assert_ne!(at(&pcm, 0, 0), at(&pcm, 31, 0), "the PCM block is a gradient, not flat");
    }

    #[test]
    fn avif_stills_decode_through_heif() {
        use gneiss_pal::dsp::image::{avif_kat, decode_heif};
        let mut f = crate::Facet::new();
        for (bytes, w, h) in [(avif_kat::white_avif(), 1, 1), (avif_kat::pixi_avif(), 4, 4), (avif_kat::grid_2x2_avif(), 7, 6)] {
            assert_eq!(sniff(&bytes), Some(Format::Avif));
            let d = PixelCoreSource.decode(&bytes).unwrap();
            assert_eq!((d.width, d.height, d.frames.is_none(), d.loop_count, d.orientation), (w, h, true, None, 1));
            assert_eq!(d.rgba, decode_heif(&bytes).unwrap().rgba);
            let (_, info) = f.open_bytes("kat.avif", &bytes).unwrap();
            assert_eq!((info.format.as_str(), info.width, info.height), ("avif", w, h));
        }
        // An AVIF ftyp and nothing else: no meta box, refused by name as avif (no longer Foreign).
        let mut avif = vec![0, 0, 0, 0x1C];
        avif.extend(b"ftypavif\0\0\0\0avifmif1miaf");
        assert_eq!(foreign_format(&avif), Some("avif"), "the name table still knows the container");
        let e = PixelCoreSource.decode(&avif).unwrap_err();
        assert!(matches!(&e, SourceError::Decode { format: "avif", reason } if reason.contains("no meta")), "{e:?}");
        assert!(e.to_string().starts_with("avif: "), "{e}");
    }
}
