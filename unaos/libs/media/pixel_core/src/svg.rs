//! SVG as an image format (SVGCORE, ledger SR52): sniffed by its markup (`<svg` or `<?xml … <svg`), parsed
//! and rendered by UnaOS's own `svg_core` (no_std, from the specifications) into the same [`Image`] every
//! other decoder returns — so Aether's `<img src=x.svg>`, Facet and the kernel viewer show SVG through the
//! one `pixel_core::decode` call. Raster `<image>` content inside the SVG decodes through pixel_core itself.
//!
//! Text needs fonts. A `no_std` caller passes them with [`decode_at`]; with the `svg-std` feature the host's
//! /usr/share/fonts are loaded once (lazily) for the plain [`decode`] path.

use crate::{Error, Image, MAX_DIM, MAX_PIXELS};
use alloc::vec::Vec;

pub use svg_core::FontSet;

fn raster_hook(b: &[u8]) -> Option<svg_core::DecodedImage> {
    // Raster images inside an SVG; nested SVG images are handled by svg_core itself.
    let img = crate::decode_first_frame(b).ok()?;
    Some(svg_core::DecodedImage { width: img.width as usize, height: img.height as usize, rgba: img.rgba })
}

fn map_err(e: svg_core::Error) -> Error {
    match e {
        svg_core::Error::Xml(_) => Error::Malformed("svg: not well-formed XML"),
        svg_core::Error::NotSvg => Error::Malformed("svg: root element is not <svg>"),
        svg_core::Error::BadSize => Error::TooLarge,
    }
}

#[cfg(feature = "svg-std")]
fn system_fonts() -> Option<&'static FontSet> {
    extern crate std;
    static FONTS: std::sync::OnceLock<FontSet> = std::sync::OnceLock::new();
    Some(FONTS.get_or_init(FontSet::system))
}

#[cfg(not(feature = "svg-std"))]
fn system_fonts() -> Option<&'static FontSet> {
    None
}

/// The intrinsic size of an SVG in whole pixels (width/height, else from the viewBox, else 100 × 100).
pub fn intrinsic_size(bytes: &[u8]) -> Result<(u32, u32), Error> {
    Ok(svg_core::Svg::parse(bytes).map_err(map_err)?.size())
}

/// Render at the intrinsic size.
pub fn decode(bytes: &[u8]) -> Result<Image, Error> {
    let svg = svg_core::Svg::parse(bytes).map_err(map_err)?;
    let (w, h) = svg.size();
    render(&svg, w, h, system_fonts())
}

/// Render at a requested size (`None` = intrinsic) with the given fonts for `<text>`.
pub fn decode_at(bytes: &[u8], size: Option<(u32, u32)>, fonts: Option<&FontSet>) -> Result<Image, Error> {
    let svg = svg_core::Svg::parse(bytes).map_err(map_err)?;
    let (w, h) = size.unwrap_or_else(|| svg.size());
    render(&svg, w, h, fonts.or(system_fonts()))
}

fn render(svg: &svg_core::Svg, w: u32, h: u32, fonts: Option<&FontSet>) -> Result<Image, Error> {
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM || (w as u64) * (h as u64) > MAX_PIXELS {
        return Err(Error::TooLarge);
    }
    // svg_core's canvas is the image-sized allocation; check the allocator can give it first (kernel heap).
    let mut probe: Vec<u8> = Vec::new();
    probe.try_reserve_exact((w as usize) * (h as usize) * 4).map_err(|_| Error::OutOfMemory)?;
    drop(probe);
    let opts = svg_core::Options { fonts, image_decoder: Some(raster_hook), languages: Vec::new() };
    let rgba = svg.render_rgba(w, h, &opts).map_err(map_err)?;
    Ok(Image::still(w, h, rgba))
}
