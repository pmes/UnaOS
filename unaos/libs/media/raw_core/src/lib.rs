// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `raw_core` — camera raw files (RAWCORE, rmbp-ledger B444).
//!
//! CHARTER: Facet — shared-core
//!
//! A raw file is a TIFF (TIFF 6.0 / TIFF-EP, ISO 12234-2) whose IFDs hold three things a viewer wants:
//!
//! * the **raw strip** — the sensor's colour-filter mosaic (`PhotometricInterpretation 32803`), stored
//!   uncompressed (`Compression 1`, 16-bit containers) or, on Sony bodies, as `Compression 32767`: with
//!   `BitsPerSample 8` the "cRAW" block code ([`decode`]), with `BitsPerSample 12` packed 12-bit samples;
//! * the **embedded JPEG preview** — `JPEGInterchangeFormat`/`…Length` (513/514); every ARW carries a
//!   full-screen one in IFD0 and a thumbnail in IFD1. It is the FAST path (Quick Look, thumbnails);
//! * the **EXIF facts** — Make/Model, the EXIF IFD's exposure, ISO, focal length, lens, DateTimeOriginal.
//!
//! Every offset in the file is attacker-shaped: each read is bounds-checked against the slice it is given,
//! the IFD walk is bounded ([`MAX_IFDS`]) with a visited set, and claimed dimensions are fenced
//! ([`MAX_PIXELS`]) before any allocation. Allocation of image-sized buffers is fallible.
//!
//! The output surface is pixel_core's: straight RGBA8, row-major, sRGB-encoded ([`color`]).
//! Owed until a real file has been read: the per-model colour matrix, white balance, `Compression 7`
//! (lossless JPEG) and the A100's ARW1. Design: `docs/dev/evidence/rmbp-1005/rawcore.md`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod color;
pub mod decode;
pub mod demosaic;
pub mod exif;
pub mod synth;
pub mod tiff;

pub use decode::RowDecoder;
pub use demosaic::{bilinear_linear, bilinear_rgba, Binner, Tone};
pub use exif::Facts;
pub use tiff::{parse, RawInfo, Strip};

/// Most IFDs one walk visits (IFD0's chain, its SubIFDs, the EXIF IFD).
pub const MAX_IFDS: usize = 16;
/// Largest sensor accepted (512 MP) — checked before anything is reserved.
pub const MAX_PIXELS: u64 = 512 * 1024 * 1024;
/// Largest width or height.
pub const MAX_DIM: u32 = 1 << 16;

/// Why a raw read failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The slice ended before a structure the file names.
    Truncated,
    /// Not `II*\0` / `MM\0*`.
    NotTiff,
    /// The bytes break TIFF (named).
    Malformed(&'static str),
    /// A legal file using something this core does not decode (named).
    Unsupported(&'static str),
    /// No raw strip in any IFD.
    NoRaw,
    /// No embedded JPEG preview.
    NoPreview,
    /// Dimensions beyond [`MAX_DIM`]/[`MAX_PIXELS`].
    TooLarge,
    /// The allocator declined.
    OutOfMemory,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => f.write_str("truncated"),
            Error::NotTiff => f.write_str("not a TIFF"),
            Error::Malformed(s) => write!(f, "malformed: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::NoRaw => f.write_str("no raw strip"),
            Error::NoPreview => f.write_str("no embedded preview"),
            Error::TooLarge => f.write_str("dimensions too large"),
            Error::OutOfMemory => f.write_str("out of memory"),
        }
    }
}

/// `II*\0` or `MM\0*` — a TIFF container (a raw file, or a plain TIFF).
pub fn is_tiff(b: &[u8]) -> bool {
    b.len() >= 8 && (b.starts_with(b"II\x2a\x00") || b.starts_with(b"MM\x00\x2a"))
}

pub const MIME_ARW: &str = "image/x-sony-arw";
pub const MIME_TIFF: &str = "image/tiff";

/// The MIME type of a TIFF head: `image/x-sony-arw` when IFD0's Make is SONY and a raw strip or the ARW
/// SubIFD is named, else `image/tiff`. `None` for anything that is not a TIFF whose IFD0 parses. Pure.
pub fn mime_of(head: &[u8]) -> Option<&'static str> {
    if !is_tiff(head) {
        return None;
    }
    // A TIFF whose IFD0 (or its Make) lies past this head is still a TIFF; a longer head decides the make.
    let Ok(info) = parse(head) else { return Some(MIME_TIFF) };
    if info.facts.make.as_deref().map(|m| m.trim().eq_ignore_ascii_case("SONY")).unwrap_or(false) {
        Some(MIME_ARW)
    } else {
        Some(MIME_TIFF)
    }
}

/// A zeroed `Vec<T>` of `n`, reserved fallibly.
pub(crate) fn zeroed<T: Copy + Default>(n: usize) -> Result<alloc::vec::Vec<T>, Error> {
    let mut v = alloc::vec::Vec::new();
    v.try_reserve_exact(n).map_err(|_| Error::OutOfMemory)?;
    v.resize(n, T::default());
    Ok(v)
}
