// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RAWCORE (rmbp-ledger B444) — camera raw as a pixel_core route, through `raw_core` (no second decoder).
//!
//! * FAST — [`decode_preview`]: the embedded JPEG every ARW carries, decoded by pixel_core's own JPEG decoder
//!   (Quick Look, thumbnails, the wallpaper). Milliseconds, and the camera's own colour rendering.
//! * FULL — [`develop`]: the raw strip through raw_core's row decoder and bilinear demosaic to the RGBA8 surface
//!   (Facet's open). No white balance or colour matrix yet: a flat, greenish image (owed after a real file).
//!
//! [`decode`] is FULL when a raw strip is present, else the preview (a plain TIFF this core reads only so far).

use crate::{Error, Image};

pub use raw_core;

/// A TIFF container (`II*\0` / `MM\0*`).
pub fn is_raw(b: &[u8]) -> bool {
    raw_core::is_tiff(b)
}

/// `image/x-sony-arw` or `image/tiff` (`raw_core::mime_of`).
pub fn mime_of(b: &[u8]) -> Option<&'static str> {
    raw_core::mime_of(b)
}

/// The EXIF facts (camera, lens, exposure, ISO, focal length, taken, size) from a head of the file.
pub fn facts(head: &[u8]) -> Option<raw_core::Facts> {
    raw_core::parse(head).ok().map(|i| i.facts)
}

pub fn map_err(e: raw_core::Error) -> Error {
    match e {
        raw_core::Error::Truncated => Error::Truncated,
        raw_core::Error::NotTiff => Error::UnknownFormat,
        raw_core::Error::Malformed(s) => Error::Malformed(s),
        raw_core::Error::Unsupported(s) => Error::Unsupported(s),
        raw_core::Error::NoRaw => Error::Unsupported("TIFF without a raw strip"),
        raw_core::Error::NoPreview => Error::Unsupported("TIFF without an embedded preview"),
        raw_core::Error::TooLarge => Error::TooLarge,
        raw_core::Error::OutOfMemory => Error::OutOfMemory,
    }
}

/// The embedded JPEG preview of a whole file, decoded. Its EXIF orientation is reported; when the JPEG names
/// none the TIFF's IFD0 Orientation is.
pub fn decode_preview(bytes: &[u8]) -> Result<Image, Error> {
    let info = raw_core::parse(bytes).map_err(map_err)?;
    preview_of(&info, bytes)
}

fn preview_of(info: &raw_core::RawInfo, bytes: &[u8]) -> Result<Image, Error> {
    let (o, l) = info.preview_in(bytes.len() as u64).ok_or(map_err(raw_core::Error::NoPreview))?;
    let mut img = crate::jpeg::decode(&bytes[o as usize..(o + l) as usize])?;
    if img.orientation == 1 {
        img.orientation = info.orientation;
    }
    Ok(img)
}

/// The raw strip of a whole file, developed at full size (bilinear) to RGBA8 sRGB.
pub fn develop(bytes: &[u8]) -> Result<Image, Error> {
    let info = raw_core::parse(bytes).map_err(map_err)?;
    develop_of(&info, bytes)
}

fn develop_of(info: &raw_core::RawInfo, bytes: &[u8]) -> Result<Image, Error> {
    let s = info.strip.as_ref().ok_or(map_err(raw_core::Error::NoRaw))?;
    crate::rgba_len(s.width, s.height)?;
    let d = raw_core::RowDecoder::new(s).map_err(map_err)?;
    let end = s.offset.checked_add(s.len).filter(|&e| e <= bytes.len() as u64).ok_or(Error::Truncated)?;
    let m = d.all(&bytes[s.offset as usize..end as usize]).map_err(map_err)?;
    let tone = raw_core::Tone::new(s.black, s.white, d.max_code).map_err(map_err)?;
    let rgba = raw_core::bilinear_rgba(&m, d.width, d.height, &s.cfa, &tone).map_err(map_err)?;
    let mut img = Image::still(s.width, s.height, rgba);
    img.orientation = info.orientation;
    Ok(img)
}

/// FULL when the file has a raw strip, else its preview.
pub fn decode(bytes: &[u8]) -> Result<Image, Error> {
    let info = raw_core::parse(bytes).map_err(map_err)?;
    if info.strip.is_some() { develop_of(&info, bytes) } else { preview_of(&info, bytes) }
}
