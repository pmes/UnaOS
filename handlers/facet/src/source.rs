// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Where decoded pixels come from — the ONE seam between Facet and a decoder.
//!
//! [`ImageSource`] is written to the shape of PIXELCORE's `gneiss_pal::dsp::image` (the host face of
//! `unaos/libs/media/pixel_core`, SR25): `sniff(&[u8]) -> Option<Format>`, `decode(&[u8]) ->
//! Result<Image, Error>`, an `Image` of straight 8-bit RGBA with optional composited animation frames,
//! a loop count, and the EXIF orientation REPORTED (not applied). [`Decoded`] mirrors that `Image`
//! field for field, so the day PIXELCORE is on trunk the replacement is one impl:
//!
//! ```ignore
//! impl ImageSource for PixelCoreSource {
//!     fn sniff(&self, b: &[u8]) -> Option<Format> { dsp::image::sniff(b).map(Format::from) }
//!     fn decode(&self, b: &[u8]) -> Result<Decoded, SourceError> { dsp::image::decode(b).map(Decoded::from).map_err(..) }
//! }
//! ```
//!
//! Until then [`ImageCrateSource`] — the `image` crate — is CHICKEN WIRE (R83): a third-party crate
//! doing the decoding UnaOS claims. It is confined to this file, behind the `chicken-wire-image`
//! feature, and it decodes pixels ONLY: the EXIF orientation it reports comes from Facet's own
//! from-spec reader ([`crate::meta`]), never from the crate.

/// The formats [`ImageSource::sniff`] recognises (PIXELCORE's `Format`, same variants, same order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Qoi,
    WebP,
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
        }
    }
}

/// Identify a format by its magic bytes — PIXELCORE's `sniff`, byte for byte, so the two sources
/// agree on what a file IS even where they disagree on whether they can decode it.
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
        None
    }
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
    /// EXIF orientation 1..=8 as found in the file (1 when absent). Reported, NOT applied.
    pub orientation: u8,
}

/// Why a source refused the bytes (PIXELCORE's `Error`, flattened to a message at this seam).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// No decoder recognised the leading bytes.
    UnknownFormat,
    /// The source recognised the format but could not decode this file (named).
    Decode(String),
    /// The claimed dimensions exceed [`MAX_DIM`] / [`MAX_PIXELS`] — refused before allocating.
    TooLarge,
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::UnknownFormat => f.write_str("unknown format"),
            SourceError::Decode(s) => write!(f, "decode: {s}"),
            SourceError::TooLarge => f.write_str("dimensions too large"),
        }
    }
}

/// PIXELCORE's limits: the largest width or height, and the largest pixel count (256 MiB RGBA).
pub const MAX_DIM: u32 = 1 << 16;
pub const MAX_PIXELS: u64 = 1 << 26;

/// A decoder Facet can read pixels through. `Send + Sync` so one source serves every handle.
pub trait ImageSource: Send + Sync {
    /// Who decodes (reported in logs and in the design doc's chicken-wire table).
    fn name(&self) -> &'static str;
    /// Identify the container by magic bytes.
    fn sniff(&self, bytes: &[u8]) -> Option<Format> {
        sniff(bytes)
    }
    /// Decode the whole file to straight RGBA (every animation frame composited).
    fn decode(&self, bytes: &[u8]) -> Result<Decoded, SourceError>;
}

/// The default source for this build.
pub fn default_source() -> Box<dyn ImageSource> {
    #[cfg(feature = "chicken-wire-image")]
    {
        Box::new(ImageCrateSource)
    }
    #[cfg(not(feature = "chicken-wire-image"))]
    {
        Box::new(NoSource)
    }
}

/// The source of a build with no decoder: refuses everything, by name.
pub struct NoSource;

impl ImageSource for NoSource {
    fn name(&self) -> &'static str {
        "none"
    }
    fn decode(&self, _bytes: &[u8]) -> Result<Decoded, SourceError> {
        Err(SourceError::Decode("this build has no image source (PIXELCORE owed, chicken wire off)".into()))
    }
}

/// CHICKEN WIRE (R83): the `image` crate decodes until PIXELCORE replaces it.
#[cfg(feature = "chicken-wire-image")]
pub struct ImageCrateSource;

#[cfg(feature = "chicken-wire-image")]
impl ImageSource for ImageCrateSource {
    fn name(&self) -> &'static str {
        "image-crate (chicken wire)"
    }

    fn decode(&self, bytes: &[u8]) -> Result<Decoded, SourceError> {
        use image::AnimationDecoder;
        let format = self.sniff(bytes).ok_or(SourceError::UnknownFormat)?;
        let fmt = match format {
            Format::Png => image::ImageFormat::Png,
            Format::Jpeg => image::ImageFormat::Jpeg,
            Format::Gif => image::ImageFormat::Gif,
            Format::Bmp => image::ImageFormat::Bmp,
            Format::Qoi => image::ImageFormat::Qoi,
            Format::WebP => image::ImageFormat::WebP,
        };
        let err = |e: image::ImageError| SourceError::Decode(e.to_string());
        // Facet's own reader answers orientation and the claimed size (refused before decoding).
        let meta = crate::meta::read(bytes, format);
        if meta.width > MAX_DIM || meta.height > MAX_DIM || (meta.width as u64) * (meta.height as u64) > MAX_PIXELS {
            return Err(SourceError::TooLarge);
        }
        let orientation = meta.orientation;
        if format == Format::Gif {
            let dec = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).map_err(err)?;
            let frames: Vec<image::Frame> = dec.into_frames().collect_frames().map_err(err)?;
            if frames.len() > 1 {
                let (w, h) = frames[0].buffer().dimensions();
                let out: Vec<Frame> = frames
                    .into_iter()
                    .map(|f| {
                        let (n, d) = f.delay().numer_denom_ms();
                        Frame { delay_ms: if d == 0 { 0 } else { n / d }, rgba: f.into_buffer().into_raw() }
                    })
                    .collect();
                return Ok(Decoded {
                    width: w,
                    height: h,
                    rgba: out[0].rgba.clone(),
                    frames: Some(out),
                    loop_count: meta.loop_count,
                    orientation,
                });
            }
        }
        let img = image::load_from_memory_with_format(bytes, fmt).map_err(err)?.to_rgba8();
        let (width, height) = img.dimensions();
        Ok(Decoded { width, height, rgba: img.into_raw(), frames: None, loop_count: None, orientation })
    }
}
