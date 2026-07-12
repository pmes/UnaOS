// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! `facet` — the UnaOS image vessel: a decoded picture given a window.
//!
//! Like every vessel, this binary is wiring and lifecycle only. It opens the
//! image file named on the command line, decodes it through `libs/lux`
//! (`decode` dispatches on the container's magic bytes: PNG/JPEG/ARW), packs
//! the linear `RgbBuffer` down to 8-bit sRGB RGBA, and hands those pixels to a
//! native Quartzite window that draws them aspect-fit. Decoding and the sRGB
//! OETF live here, in the vessel; the quartzite image view is a dumb blitter.
//!
//! MVP: a static picture on screen, aspect-fit, right-side-up, color-managed.
//! Pan/zoom, per-pixel readout, and the euclase textured-quad (GPU) path are
//! the later arcs (ROADMAP §3a).

use bandy::telemetry;
use gneiss_pal::paths::UnaPaths;
use lux::RgbBuffer;

/// The sRGB opto-electronic transfer function (linear → sRGB), the exact
/// inverse of `lux::color::srgb_to_linear`. `lux` hands us *linear* f32 RGB;
/// a display wants sRGB-encoded samples, so we encode here before packing.
#[inline]
fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Pack a linear `RgbBuffer` into tightly packed 8-bit sRGB RGBA (opaque),
/// row-major, top row first — the format the quartzite image view expects.
fn pack_srgba(buf: &RgbBuffer) -> Vec<u8> {
    let px = buf.width as usize * buf.height as usize;
    let mut out = Vec::with_capacity(px * 4);
    // Guard against a short pixel vec: only iterate whole RGB triples we have.
    let triples = buf.pixels.len() / 3;
    let n = triples.min(px);
    for i in 0..n {
        let r = linear_to_srgb(buf.pixels[i * 3]);
        let g = linear_to_srgb(buf.pixels[i * 3 + 1]);
        let b = linear_to_srgb(buf.pixels[i * 3 + 2]);
        out.push((r * 255.0).round().clamp(0.0, 255.0) as u8);
        out.push((g * 255.0).round().clamp(0.0, 255.0) as u8);
        out.push((b * 255.0).round().clamp(0.0, 255.0) as u8);
        out.push(255);
    }
    // Any pixels the decoder under-delivered stay black+opaque.
    out.resize(px * 4, 0);
    for i in 0..px {
        if i >= n {
            out[i * 4 + 3] = 255;
        }
    }
    out
}

fn main() {
    // 1. Establish Base Camp + Telemetry (the shared vessel boot).
    UnaPaths::awaken().expect("CRITICAL: Failed to awaken spatial paths");
    telemetry::ignite(UnaPaths::root().join("logs"));
    log::info!("Facet Boot Sequence Initiated.");

    // 2. The subject: the image path is the one required argument.
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("facet: usage: facet <image-file>   (PNG, JPEG, or Sony ARW)");
            log::error!("facet: no image path given");
            std::process::exit(2);
        }
    };

    // 3. Read + decode via lux. Fail loudly, before opening any window.
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("facet: cannot read {path}: {e}");
            log::error!("facet: read {path} failed: {e}");
            std::process::exit(1);
        }
    };
    let image = match lux::decode(&bytes) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("facet: cannot decode {path}: {e:?}");
            log::error!("facet: decode {path} failed: {e:?}");
            std::process::exit(1);
        }
    };
    log::info!(
        "[FACET] :: Decoded {} — {}x{} ({} linear samples).",
        path,
        image.width,
        image.height,
        image.pixels.len()
    );

    // 4. Pack linear → sRGB RGBA once, here in the vessel.
    let (w, h) = (image.width, image.height);
    let rgba = pack_srgba(&image);

    // 5. The Window (macOS AppKit via quartzite; further backends follow
    //    quartzite maturity).
    #[cfg(target_os = "macos")]
    {
        let title = std::path::Path::new(&path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("facet")
            .to_string();
        quartzite::Backend::new_vessel(
            "org.unaos.facet",
            &format!("facet — {title}"),
            (
                (w as f64).clamp(240.0, 1600.0),
                (h as f64).clamp(160.0, 1000.0),
            ),
            move |window| {
                quartzite::platforms::macos::image_view::bootstrap_image_view(window, &rgba, w, h)
            },
        )
        .run();
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (rgba, w, h);
        eprintln!("facet: no native backend for this platform yet (macOS first).");
        log::error!("facet: no native backend for this platform yet (macOS first).");
    }
}
