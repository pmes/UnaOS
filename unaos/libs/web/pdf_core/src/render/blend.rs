// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — shared-core. PDFTRANSPARENCY (SR327): PDF blend modes (ISO 32000-1 §11.3.5) on svg_core's ONE
//! blend formula. The /BM names map onto `svg_core::fe::BlendMode`; the separable and non-separable functions and the
//! premultiplied result formula (§11.3.6: o = sc(1-da) + dc(1-sa) + sa·da·B(Cb, Cs)) are `svg_core::fe::blend`'s, never a
//! second copy here. This is the one place pdf_core calls it.

use svg_core::fe::{self, BlendMode, IRect};
use svg_core::raster::Pixmap;

use crate::doc::Document;
use crate::object::Object;

/// A PDF blend mode name (Table 136, Table 137); `Compatible` is Normal.
pub fn by_name(n: &[u8]) -> Option<BlendMode> {
    Some(match n {
        b"Normal" | b"Compatible" => BlendMode::Normal,
        b"Multiply" => BlendMode::Multiply,
        b"Screen" => BlendMode::Screen,
        b"Overlay" => BlendMode::Overlay,
        b"Darken" => BlendMode::Darken,
        b"Lighten" => BlendMode::Lighten,
        b"ColorDodge" => BlendMode::ColorDodge,
        b"ColorBurn" => BlendMode::ColorBurn,
        b"HardLight" => BlendMode::HardLight,
        b"SoftLight" => BlendMode::SoftLight,
        b"Difference" => BlendMode::Difference,
        b"Exclusion" => BlendMode::Exclusion,
        b"Hue" => BlendMode::Hue,
        b"Saturation" => BlendMode::Saturation,
        b"Color" => BlendMode::Color,
        b"Luminosity" => BlendMode::Luminosity,
        _ => return None,
    })
}

/// An ExtGState /BM value: a name, or an array whose first recognised name wins (§11.6.3). `None` = none recognised.
pub fn parse(o: &Object, doc: &Document) -> Option<BlendMode> {
    match o {
        Object::Array(a) => a.iter().find_map(|e| doc.get(e).as_name().and_then(by_name)),
        o => o.as_name().and_then(by_name),
    }
}

/// Composite `layer` (premultiplied, transparent outside what was painted) onto `canvas` inside `r` under `mode`, through
/// `fe::blend`; pixels outside `r` are untouched.
pub fn composite(canvas: &mut Pixmap, layer: &Pixmap, r: IRect, mode: BlendMode) {
    let r = r.intersect(&IRect { x0: 0, y0: 0, x1: canvas.w, y1: canvas.h });
    if r.is_empty() {
        return;
    }
    let out = fe::blend(layer, canvas, &r, mode);
    for y in r.y0..r.y1 {
        let (a, b) = ((y * canvas.w + r.x0) * 4, (y * canvas.w + r.x1) * 4);
        canvas.data[a..b].copy_from_slice(&out.data[a..b]);
    }
}
