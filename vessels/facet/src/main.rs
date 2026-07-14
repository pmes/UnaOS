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
//! The picture shows aspect-fit, right-side-up, and color-managed, and the
//! quartzite view gives it hands: zoom about the cursor, drag-pan, reset-to-fit,
//! and a live per-pixel readout (FACET-2).
//!
//! FACET-GPU: presentation defaults to the euclase textured quad — quartzite
//! hosts a `CAMetalLayer` (pinned sRGB) and this vessel builds the wgpu side
//! over it (`Cortex::ignite_layer_blocking` + `TexturedQuad`, frame uploaded
//! `Rgba8UnormSrgb`), redrawn on demand with zoom/pan folded into the quad's
//! `Mat4` uniform. If GPU init fails the vessel logs it and falls back to the
//! CPU blit automatically; `FACET_CPU=1` forces the CPU path — the knob is the
//! eye-witness A/B instrument (the two paths must be visually
//! indistinguishable).

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

/// The sRGB EOTF (sRGB → linear) at f64, for the GPU clear color: wgpu clear
/// values are linear and the sRGB swapchain encodes on store, so feeding the
/// linearized Moonstone triple reproduces the CPU path's field bytes exactly.
#[cfg(target_os = "macos")]
fn srgb8_to_linear_f64(v: u8) -> f64 {
    let c = v as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The console field the picture sits on (UnaOS Moonstone, `#2D2B55` —
/// matching the quartzite CPU view's `FIELD`).
#[cfg(target_os = "macos")]
const FIELD_SRGB: (u8, u8, u8) = (0x2D, 0x2B, 0x55);

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

/// Attempt the GPU presentation path: quartzite hosts the `CAMetalLayer`
/// (already pinned sRGB), and this vessel builds the whole wgpu side over the
/// layer pointer it hands back — a presentation-spec `Cortex` (vsync, no extra
/// features), the `TexturedQuad`, and one `Rgba8UnormSrgb` upload of the same
/// packed sRGB bytes the CPU path blits (the readout keeps consuming them on
/// the view side). Returns `None` on any GPU-init failure so the caller can
/// fall back to the CPU view.
#[cfg(target_os = "macos")]
fn bootstrap_gpu(
    window: &quartzite::NativeWindow,
    rgba: &[u8],
    linear: &[f32],
    w: u32,
    h: u32,
) -> Option<quartzite::NativeView> {
    use euclase::cortex::{Cortex, CortexSpec};
    use euclase::quad::{view_rect_to_ndc, TexturedQuad};
    use quartzite::platforms::macos::gpu_view::{self, GpuFrameParams};

    // The Moonstone field, linearized: the sRGB swapchain re-encodes on store,
    // landing the exact CPU-path field bytes around an aspect-fit picture.
    let clear = euclase::wgpu::Color {
        r: srgb8_to_linear_f64(FIELD_SRGB.0),
        g: srgb8_to_linear_f64(FIELD_SRGB.1),
        b: srgb8_to_linear_f64(FIELD_SRGB.2),
        a: 1.0,
    };

    gpu_view::bootstrap_gpu_view(window, rgba, linear, w, h, |layer, dw, dh| {
        // SAFETY: `layer` is the view's retained CAMetalLayer; the returned
        // closure (owning the Cortex and its Surface) is stored in the view's
        // ivars, which drop before the view releases the layer — the layer
        // outlives the Cortex<'static> (see gpu_view's module doc).
        let mut cortex = match unsafe {
            Cortex::ignite_layer_blocking(layer, dw, dh, CortexSpec::presentation())
        } {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[FACET] :: GPU cortex ignite failed: {e}");
                return None;
            }
        };
        log::info!(
            "[FACET] :: GPU cortex up — {}x{} swapchain, format {:?}.",
            cortex.config.width,
            cortex.config.height,
            cortex.config.format
        );

        // Linear filtering: closest to the CPU blit's AppKit look (nearest at
        // high zoom stays an open eye-witness question).
        let mut quad = TexturedQuad::forge(
            &cortex.device,
            cortex.config.format,
            euclase::wgpu::FilterMode::Linear,
        );
        quad.upload_frame(&cortex.device, &cortex.queue, rgba, w, h);

        Some(Box::new(move |p: &GpuFrameParams| {
            if (cortex.config.width, cortex.config.height) != p.drawable {
                cortex.resize(p.drawable.0, p.drawable.1);
            }
            quad.set_transform(&cortex.queue, &view_rect_to_ndc(p.dest, p.bounds));
            if let Err(e) = quad.render(&cortex, clear) {
                log::warn!("[FACET] :: GPU present failed: {e}");
            }
        }))
    })
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

    // 4. Pack linear → sRGB RGBA once, here in the vessel. Keep the source
    //    linear RGB too — the image view surfaces it in the FACET-2 pixel
    //    readout alongside the packed sRGB.
    let (w, h) = (image.width, image.height);
    let rgba = pack_srgba(&image);
    let linear = image.pixels;

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
                // GPU by default; FACET_CPU=1 is the eye-witness A/B knob.
                let force_cpu = std::env::var("FACET_CPU").is_ok_and(|v| v == "1");
                if force_cpu {
                    log::info!("[FACET] :: FACET_CPU=1 — forcing the CPU blit path.");
                } else if let Some(view) = bootstrap_gpu(window, &rgba, &linear, w, h) {
                    log::info!(
                        "[FACET] :: Presentation path: GPU (euclase textured quad on CAMetalLayer)."
                    );
                    return view;
                } else {
                    log::warn!(
                        "[FACET] :: GPU presentation unavailable; falling back to the CPU blit."
                    );
                }
                log::info!("[FACET] :: Presentation path: CPU (AppKit NSBitmapImageRep blit).");
                quartzite::platforms::macos::image_view::bootstrap_image_view(
                    window, &rgba, &linear, w, h,
                )
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
