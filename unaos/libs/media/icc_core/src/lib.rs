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
//! device-link, abstract and multiProcessElements profiles, iccMAX. Owed: output-side mBA / B2A, display profiles,
//! CMYK printing intents.
//!
//! `#![no_std]` + `alloc`, `#![forbid(unsafe_code)]`; the one dependency is libm (`powf`, `cbrtf`).
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod profile;
pub mod transform;

pub use profile::{Clut, Curve, Lut, LutKind, Model, Pcs, Profile, Refusal, SRGB, SRGB_TRC, Space, kat};
pub use transform::{Transform, bradford, oetf, pcs_to_linear_srgb};

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
