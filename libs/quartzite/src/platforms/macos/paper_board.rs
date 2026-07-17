// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! The paper sample board — the SURFACE-1 taste-gate surface (macOS/AppKit).
//!
//! A single [`NSView`] laying out a grid of real text views, each rendering the
//! same paragraph over a candidate paper [`Surface`](crate::surface). Rows are
//! the three procedural algorithms ([`PaperAlgo`]); the first column is the
//! honest **no-texture control**, the next three are rising subtlety levels.
//! Every cell is labelled with its exact parameters.
//!
//! # How it stays honest
//!
//!  * **Texture strictly under the glyphs.** Each cell first draws its paper
//!    raster ([`crate::surface::render_rgba8`]) as the card background, then
//!    draws the paragraph with the *native* text stack
//!    (`NSString drawInRect:withAttributes:`) on top. Glyph antialiasing is the
//!    OS's, unchanged over textured vs control cells — the texture never
//!    composites over the ink.
//!  * **Retina scale-2 correct.** The raster is generated at
//!    `cell_points × backingScaleFactor` device pixels and the rep is tagged at
//!    point size, so it presents 1:1 on a 2× display (the 2012 rMBP target
//!    class). Text is native and therefore already resolution-independent.
//!  * **Deterministic.** The field is a pure function of its parameters, so the
//!    board is byte-stable across redraws — no shimmer.
//!  * **Contrast-budgeted.** Amplitudes are the declared budget; the surface
//!    core bounds every pixel to them (proven in `surface`'s tests).
//!
//! The aesthetic is **not** chosen here. This view renders an honest sample
//! space; Peter picks from the real pixels (the attended taste-gate).

use std::cell::Cell;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{class, define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{
    NSBitmapImageRep, NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName,
    NSImageRep, NSView,
};
use objc2_foundation::{MainThreadMarker, NSDictionary, NSPoint, NSRect, NSSize, NSString};

use crate::surface::{self, Paper, PaperAlgo, PaperParams, PAPER_STOCK};

/// The board's designed content size (width, height) in points.
pub const BOARD_SIZE: (f64, f64) = (1180.0, 900.0);

/// Neutral board field the paper cards sit on — a soft, desaturated grey so the
/// warm stock is judged against a calm neutral, the way paper is judged on a
/// desk.
const BOARD_FIELD: (f64, f64, f64) = (0.74, 0.74, 0.75);

/// The ink the paragraph is set in — a warm near-black, never pure `#000`.
const INK: u32 = 0x1E1B18;
/// The parameter-label ink (a quieter grey-brown over the paper).
const LABEL_INK: u32 = 0x4A4038;
/// The board title / caption ink over the neutral field.
const TITLE_INK: u32 = 0x2A2A2E;

/// The subtlety ladder: max relative luminance deviation per level.
const LEVELS: [(f32, &str); 3] = [(0.010, "subtle"), (0.020, "medium"), (0.035, "strong")];

/// The real paragraph every cell sets — long enough to judge texture *under
/// running text*, which is the case that matters.
const SPECIMEN: &str = "Paper is light behaving correctly. Hold a good sheet to the \
window and it is never flat: the stock clouds where the pulp settled thick and thin, \
the laid lines catch a raking sun, and the tooth of the surface scatters the light so \
the white is warm, not clinical. We are after that felt quality here — texture as \
tactility, not decoration. The test is simple and severe: at reading distance you \
should barely see it, and want to touch it anyway.";

// -- per-algorithm feature scale (device px per lattice unit / laid pitch) ---
fn algo_scale(algo: PaperAlgo) -> f32 {
    match algo {
        PaperAlgo::Grain => 2.5,
        PaperAlgo::Laid => 4.0,
        PaperAlgo::Blotch => 6.0,
    }
}

fn algo_octaves(algo: PaperAlgo) -> u32 {
    match algo {
        PaperAlgo::Grain => 3,
        PaperAlgo::Laid => 3,
        PaperAlgo::Blotch => 4,
    }
}

/// The base seed the per-cell seeds decorrelate from.
const SEED_BASE: u32 = 0x51A7_0000;

/// A distinct per-cell seed so no two cards share a field (no visible tiling
/// across the layout).
fn cell_seed(row: usize, col: usize) -> u32 {
    (row as u32)
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add((col as u32).wrapping_mul(0x85EB_CA77))
}

// ---------------------------------------------------------------------------
// THE VIEW
// ---------------------------------------------------------------------------

pub struct PaperBoardIvars {
    /// Cached backing scale; the layout re-derives from live bounds each draw.
    scale: Cell<f64>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "UnaPaperBoardView"]
    #[ivars = PaperBoardIvars]
    pub struct PaperBoardView;

    impl PaperBoardView {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(PaperBoardIvars { scale: Cell::new(2.0) });
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        // Flipped: top-left origin makes the grid layout read top-to-bottom the
        // way the labels describe it. All text/rect math below assumes this.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> objc2::runtime::Bool {
            objc2::runtime::Bool::YES
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.render();
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, new_size: NSSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: new_size] };
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn view_did_move_to_window(&self) {
            if let Some(w) = self.window() {
                let s: f64 = unsafe { msg_send![&w, backingScaleFactor] };
                self.ivars().scale.set(if s > 0.0 { s } else { 2.0 });
            }
            self.setNeedsDisplay(true);
        }
    }
);

impl PaperBoardView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let _ = mtm;
        let this: Allocated<Self> = unsafe { msg_send![Self::class(), alloc] };
        unsafe { msg_send![this, initWithFrame: frame] }
    }

    fn render(&self) {
        let bounds = self.bounds();
        let scale = self.ivars().scale.get();

        // Board field.
        ns_srgb(BOARD_FIELD.0, BOARD_FIELD.1, BOARD_FIELD.2, 1.0).set();
        objc2_app_kit::NSRectFill(bounds);

        let margin = 22.0;
        let title_h = 48.0;
        let gap = 14.0;
        let cols = 4usize; // control + 3 levels
        let rows = PaperAlgo::ALL.len();

        // Title + caption strip.
        self.draw_title(bounds, margin, title_h);

        let grid_x = bounds.origin.x + margin;
        let grid_y = bounds.origin.y + margin + title_h;
        let grid_w = bounds.size.width - margin * 2.0;
        let grid_h = bounds.size.height - margin * 2.0 - title_h;
        if grid_w <= 0.0 || grid_h <= 0.0 {
            return;
        }
        let cell_w = (grid_w - gap * (cols as f64 - 1.0)) / cols as f64;
        let cell_h = (grid_h - gap * (rows as f64 - 1.0)) / rows as f64;

        for (row, &algo) in PaperAlgo::ALL.iter().enumerate() {
            for col in 0..cols {
                let cx = grid_x + col as f64 * (cell_w + gap);
                let cy = grid_y + row as f64 * (cell_h + gap);
                let rect = NSRect::new(NSPoint::new(cx, cy), NSSize::new(cell_w, cell_h));

                let (paper, label) = if col == 0 {
                    (None, "control — no texture".to_string())
                } else {
                    let (amp, level) = LEVELS[col - 1];
                    let params = PaperParams {
                        algo,
                        amplitude: amp,
                        scale: algo_scale(algo),
                        octaves: algo_octaves(algo),
                        seed: SEED_BASE.wrapping_add(cell_seed(row, col)),
                    };
                    let label = format!("{} · {}", level, params.describe());
                    (Some(Paper { base_rgb: PAPER_STOCK, params }), label)
                };

                self.draw_cell(rect, scale, paper.as_ref(), &label);
            }
        }
    }

    /// One card: paper raster background, parameter label, then the specimen
    /// paragraph in native text on top.
    fn draw_cell(&self, rect: NSRect, scale: f64, paper: Option<&Paper>, label: &str) {
        // Device-pixel raster size for this card.
        let dev_w = (rect.size.width * scale).round().max(1.0) as u32;
        let dev_h = (rect.size.height * scale).round().max(1.0) as u32;

        // Background: the paper (or honest flat stock for the control).
        let rgba = match paper {
            Some(p) => surface::render_rgba8(p, dev_w, dev_h),
            None => flat_rgba(PAPER_STOCK, dev_w, dev_h),
        };
        if let Some(rep) = bitmap_rep(&rgba, dev_w, dev_h, rect.size.width, rect.size.height) {
            let _: bool = unsafe {
                let rep_ref: &NSImageRep = rep.as_ref();
                msg_send![rep_ref, drawInRect: rect]
            };
        }

        // A hairline edge so the card reads as a sheet on the field.
        ns_srgb(0.0, 0.0, 0.0, 0.10).set();
        objc2_app_kit::NSFrameRect(rect);

        // Content insets.
        let pad = 16.0;
        let label_h = 16.0;

        // Parameter label (small, over the paper margin).
        let label_font = NSFont::monospacedSystemFontOfSize_weight(10.0, unsafe {
            objc2_app_kit::NSFontWeightMedium
        });
        let label_attrs = text_attrs(&label_font, LABEL_INK);
        let label_rect = NSRect::new(
            NSPoint::new(rect.origin.x + pad, rect.origin.y + pad - 2.0),
            NSSize::new(rect.size.width - pad * 2.0, label_h),
        );
        draw_string(label, label_rect, &label_attrs);

        // The specimen paragraph — native text, wrapped in the content rect.
        let body_font = NSFont::systemFontOfSize(13.5);
        let body_attrs = text_attrs(&body_font, INK);
        let body_rect = NSRect::new(
            NSPoint::new(rect.origin.x + pad, rect.origin.y + pad + label_h + 6.0),
            NSSize::new(
                rect.size.width - pad * 2.0,
                rect.size.height - pad * 2.0 - label_h - 6.0,
            ),
        );
        draw_string(SPECIMEN, body_rect, &body_attrs);
    }

    fn draw_title(&self, bounds: NSRect, margin: f64, title_h: f64) {
        let x = bounds.origin.x + margin;
        let y = bounds.origin.y + margin;
        let w = bounds.size.width - margin * 2.0;

        let title_font = NSFont::boldSystemFontOfSize(17.0);
        let title_attrs = text_attrs(&title_font, TITLE_INK);
        draw_string(
            "SURFACE-1 · paper sample board",
            NSRect::new(NSPoint::new(x, y), NSSize::new(w, 22.0)),
            &title_attrs,
        );

        let cap_font = NSFont::systemFontOfSize(11.5);
        let cap_attrs = text_attrs(&cap_font, TITLE_INK);
        draw_string(
            "rows: grain / laid / blotch   columns: control · subtle · medium · strong   \
             — texture under the ink, budget = max % luminance deviation. Judge at reading \
             distance; lean in to confirm.",
            NSRect::new(NSPoint::new(x, y + 24.0), NSSize::new(w, title_h - 24.0)),
            &cap_attrs,
        );
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn ns_srgb(r: f64, g: f64, b: f64, a: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a)
}

fn text_attrs(font: &NSFont, rgb: u32) -> Retained<NSDictionary<NSString, AnyObject>> {
    let (r, g, b) = (
        ((rgb >> 16) & 0xFF) as f64 / 255.0,
        ((rgb >> 8) & 0xFF) as f64 / 255.0,
        (rgb & 0xFF) as f64 / 255.0,
    );
    let color = ns_srgb(r, g, b, 1.0);
    let keys: [&NSString; 2] = [
        unsafe { NSFontAttributeName },
        unsafe { NSForegroundColorAttributeName },
    ];
    let values: [&AnyObject; 2] = [font.as_ref(), color.as_ref()];
    NSDictionary::from_slices(&keys, &values)
}

fn draw_string(s: &str, rect: NSRect, attrs: &NSDictionary<NSString, AnyObject>) {
    let ns = NSString::from_str(s);
    let _: () = unsafe { msg_send![&*ns, drawInRect: rect, withAttributes: attrs] };
}

/// A flat opaque RGBA8 raster of one colour — the honest control / CPU-path
/// "render it as nothing" case.
fn flat_rgba(rgb: [f32; 3], w: u32, h: u32) -> Vec<u8> {
    let px = [
        (rgb[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (rgb[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (rgb[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        255,
    ];
    let mut out = vec![0u8; w as usize * h as usize * 4];
    for chunk in out.chunks_exact_mut(4) {
        chunk.copy_from_slice(&px);
    }
    out
}

/// Wrap tightly packed sRGB RGBA8 device pixels in an `NSBitmapImageRep` tagged
/// sRGB and sized to `point_w × point_h`, so it presents 1:1 at the backing
/// scale (Retina-correct). Row-major, top row first — matching this view's
/// flipped coordinate system.
fn bitmap_rep(
    rgba: &[u8],
    w: u32,
    h: u32,
    point_w: f64,
    point_h: f64,
) -> Option<Retained<NSBitmapImageRep>> {
    let expected = w as usize * h as usize * 4;
    if w == 0 || h == 0 || rgba.len() < expected {
        return None;
    }
    let bytes_per_row = w as usize * 4;
    let rep: Retained<NSBitmapImageRep> = unsafe {
        let alloc: Allocated<NSBitmapImageRep> = msg_send![NSBitmapImageRep::class(), alloc];
        let planes: *mut *mut u8 = std::ptr::null_mut();
        let rep: Option<Retained<NSBitmapImageRep>> = msg_send![
            alloc,
            initWithBitmapDataPlanes: planes,
            pixelsWide: w as isize,
            pixelsHigh: h as isize,
            bitsPerSample: 8isize,
            samplesPerPixel: 4isize,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: &*NSString::from_str("NSDeviceRGBColorSpace"),
            bytesPerRow: bytes_per_row as isize,
            bitsPerPixel: 32isize,
        ];
        rep?
    };
    let data: *mut u8 = unsafe { msg_send![&rep, bitmapData] };
    if data.is_null() {
        return None;
    }
    unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), data, expected) };

    // Retag (reinterpret) to sRGB so AppKit presents the values faithfully.
    let rep = unsafe {
        let srgb: *mut AnyObject = msg_send![class!(NSColorSpace), sRGBColorSpace];
        if srgb.is_null() {
            rep
        } else {
            let retagged: Option<Retained<NSBitmapImageRep>> =
                msg_send![&rep, bitmapImageRepByRetaggingWithColorSpace: srgb];
            retagged.unwrap_or(rep)
        }
    };

    // Present at point size → 1:1 device pixels at the backing scale.
    let _: () = unsafe { msg_send![&rep, setSize: NSSize::new(point_w, point_h)] };
    Some(rep)
}

// ---------------------------------------------------------------------------
// BOOTSTRAP — the sample-board host seam
// ---------------------------------------------------------------------------

/// Build the paper sample board as a window's content view. Main thread only.
pub fn bootstrap_paper_board(_window: &objc2_app_kit::NSWindow) -> Retained<NSView> {
    let mtm = MainThreadMarker::new().expect("bootstrap_paper_board must run on the main thread");
    let frame = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(BOARD_SIZE.0, BOARD_SIZE.1),
    );
    let view = PaperBoardView::new(mtm, frame);
    Retained::into_super(view)
}
