// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// CHARTER: Facet (CODEX §2 Images), seam shared-core — the ONE ICC colour engine Facet's open, Aether's <img> and pdf_core convert through.
//! `icc_core` — ICC.1:2010 (v2 / v4) source profiles to sRGB (PDFICC, LEDGER SR358).
//!
//! [`Profile::parse`] reads the header, the tag table and the matrix/TRC (`XYZ `, `curv`, `para`) or A2B (`mft1`
//! lut8, `mft2` lut16, `mAB ` lutAToB with a multilinear CLUT) model of a GRAY, RGB or CMYK profile;
//! [`Transform::to_srgb`] runs it into the PCS (XYZ D50, or Lab D50) and out through ONE matrix,
//! `inv(M_sRGB_D65) . bradford(D50, D65)`, and the sRGB OETF, with 8-bit caches for the matrix/TRC and GRAY paths.
//! [`SRGB`] is the built-in sRGB profile and [`Profile::is_srgb`] the short-circuit (by content, as Chromium's skcms
//! does): a tagged sRGB image is left byte-identical.
//!
//! pixel_core SURFACES a decoded image's embedded profile (`pixel_core::Image::icc`: PNG iCCP, JPEG APP2, WebP ICCP) and
//! never links this crate; the callers run [`apply_icc_to_srgb`] (Facet's open, Aether's `<img>`) or a [`Transform`]
//! (pdf_core's /ICCBased colour space and JPX `colr` profiles). Refused by name ([`Refusal`]): named-colour,
//! device-link, abstract and multiProcessElements profiles, iccMAX. Owed: output-side mBA / B2A, CMYK printing intents;
//! display profiles: matrix/TRC built ([`display`], KERNEL-DISPLAY-ICC SR372), LUT-based owed.
//!
//! A matrix/TRC source runs over RGBA in integers (KERNEL-ICC-INT, SR376: DisplayTransform's Q14 shape); a caller opening
//! many files keeps one built transform per profile in a [`TransformCache`] (KERNEL-ICC-CACHE, SR376).
//!
//! `#![no_std]` + `alloc`, `#![forbid(unsafe_code)]`; the one dependency is libm (`powf`, `cbrtf`).
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod display;
pub mod profile;
pub mod transform;

pub use profile::{Clut, Curve, Lut, LutKind, Model, Pcs, Profile, Refusal, SRGB, SRGB_TRC, Space, kat};
pub use transform::{Transform, bradford, oetf, pcs_to_linear_srgb};
pub use display::{DisplayTransform, EdidColour, edid_colour};

/// What [`apply_icc_to_srgb`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The profile is sRGB: the pixels were left untouched.
    Srgb,
    /// The pixels were converted to sRGB.
    Converted,
}

/// THE entry for decoded images: convert straight RGBA8 (`rgba`, alpha untouched) tagged with the profile `icc` to
/// sRGB in place. An sRGB profile changes nothing; a GRAY profile reads the red channel; a CMYK profile is refused
/// (the decoder already turned CMYK samples into RGB: JPEG-CMYK-ICC owed). A refusal leaves `rgba` as decoded.
pub fn apply_icc_to_srgb(icc: &[u8], rgba: &mut [u8]) -> Result<Applied, Refusal> {
    match Transform::for_rgba(icc)? {
        None => Ok(Applied::Srgb),
        Some(xf) => {
            xf.apply_rgba(rgba);
            Ok(Applied::Converted)
        }
    }
}

// ── KERNEL-ICC-CACHE (LEDGER SR376): one built transform per profile, kept across opens ──

/// The 64-bit FNV-1a hash of `b` (the [`TransformCache`] key, with the length).
pub fn fnv1a64(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// The slots a [`TransformCache`] keeps.
pub const TRANSFORM_CACHE_SLOTS: usize = 4;

/// [`Transform::for_rgba`]'s answers for the last [`TRANSFORM_CACHE_SLOTS`] profiles (least recently used evicted),
/// keyed by [`fnv1a64`] and the length of the bytes; an sRGB answer (`Ok(None)`) and a refusal are kept too. The
/// caller owns the lock (the kernel Facet's one `Mutex`).
pub struct TransformCache {
    /// Most recent first.
    slots: alloc::vec::Vec<(u64, usize, Result<Option<Transform>, Refusal>)>,
    builds: u32,
}

impl TransformCache {
    /// An empty cache (a `static`'s initialiser).
    pub const fn new() -> TransformCache {
        TransformCache { slots: alloc::vec::Vec::new(), builds: 0 }
    }

    /// [`Transform::for_rgba`] of `icc`, built once per profile while it stays among the recent ones (a clone of the
    /// kept transform).
    pub fn for_rgba(&mut self, icc: &[u8]) -> Result<Option<Transform>, Refusal> {
        let key = (fnv1a64(icc), icc.len());
        if let Some(i) = self.slots.iter().position(|s| (s.0, s.1) == key) {
            let hit = self.slots.remove(i);
            self.slots.insert(0, hit);
        } else {
            self.builds = self.builds.wrapping_add(1);
            self.slots.insert(0, (key.0, key.1, Transform::for_rgba(icc)));
            self.slots.truncate(TRANSFORM_CACHE_SLOTS);
        }
        self.slots[0].2.clone()
    }

    /// How many profiles were parsed and built (misses).
    pub fn builds(&self) -> u32 {
        self.builds
    }
}

impl Default for TransformCache {
    fn default() -> TransformCache {
        TransformCache::new()
    }
}
