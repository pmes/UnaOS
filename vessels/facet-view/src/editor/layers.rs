// CHARTER: Facet (Images) — host face over the shared cores (RASTERFACE, LEDGER SR234)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The layers panel: the right strip of the one surface. Rows run top-down from the TOP layer, so the stack reads
//! bottom-to-top as it composites. A row is an eye box (filled = visible), a thumbnail, an opacity bar and eight
//! blend ticks (the lit one is the mode, `BlendMode::ALL` order); the active row is outlined. Below the rows: the
//! colour swatch. ONE geometry ([`row_y`], [`EYE`]) is read by [`paint`] and [`hit`].

use raster_core::Document;

use super::canvas::{check, over_grey};

/// The panel's width, in surface pixels.
pub const PANEL_W: u32 = 160;
/// One row's height.
pub const ROW_H: u32 = 28;
/// The eye box: x span inside the panel, and its size.
pub const EYE: (u32, u32) = (4, 20);
/// The thumbnail's x inside the panel and its size.
pub const THUMB: (u32, u32) = (28, 20);
/// The opacity bar / blend ticks start here (panel x) and span this width.
pub const BAR: (u32, u32) = (54, 96);

/// What a click in the panel hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The row of layer `i`.
    Row(usize),
    /// The eye box of layer `i`.
    Eye(usize),
    /// Nothing.
    None,
}

/// The top of layer `i`'s row (panel y), of `n` layers: the top layer's row first.
pub fn row_y(i: usize, n: usize) -> u32 {
    (n - 1 - i) as u32 * ROW_H
}

/// The panel point `(px, py)` (relative to the panel's left edge) over `n` layers.
pub fn hit(px: f64, py: f64, n: usize) -> Hit {
    if px < 0.0 || py < 0.0 || px >= PANEL_W as f64 {
        return Hit::None;
    }
    let row = (py / ROW_H as f64).floor() as usize;
    if row >= n {
        return Hit::None;
    }
    let i = n - 1 - row;
    let (ex, es) = EYE;
    let in_y = py - row_y(i, n) as f64;
    if px >= ex as f64 && px < (ex + es) as f64 && in_y >= 4.0 && in_y < 4.0 + es as f64 { Hit::Eye(i) } else { Hit::Row(i) }
}

fn put(frame: &mut [u8], stride: u32, x: u32, y: u32, rgb: [u8; 3]) {
    let o = ((y * stride + x) * 4) as usize;
    if o + 4 <= frame.len() {
        frame[o..o + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
}

fn rect(frame: &mut [u8], stride: u32, x0: u32, y0: u32, w: u32, h: u32, rgb: [u8; 3], hmax: u32) {
    for y in y0..(y0 + h).min(hmax) {
        for x in x0..x0 + w {
            put(frame, stride, x, y, rgb);
        }
    }
}

fn outline(frame: &mut [u8], stride: u32, x0: u32, y0: u32, w: u32, h: u32, rgb: [u8; 3], hmax: u32) {
    for x in x0..x0 + w {
        for y in [y0, y0 + h - 1] {
            if y < hmax {
                put(frame, stride, x, y, rgb);
            }
        }
    }
    for y in y0..(y0 + h).min(hmax) {
        put(frame, stride, x0, y, rgb);
        put(frame, stride, x0 + w - 1, y, rgb);
    }
}

/// Paint the panel into the frame at x `x0` (the strip `x0..x0+PANEL_W`, `h` tall).
pub fn paint(frame: &mut [u8], stride: u32, x0: u32, h: u32, doc: &Document, active: usize, colour: [u8; 4]) {
    rect(frame, stride, x0, 0, PANEL_W, h, [40, 40, 44], h);
    let n = doc.layers.len();
    for (i, l) in doc.layers.iter().enumerate() {
        let y = row_y(i, n);
        if y + ROW_H > h {
            continue;
        }
        if i == active {
            rect(frame, stride, x0, y, PANEL_W, ROW_H, [64, 72, 96], h);
        }
        // The eye.
        let (ex, es) = EYE;
        outline(frame, stride, x0 + ex, y + 4, es, es, [200, 200, 200], h);
        if l.visible {
            rect(frame, stride, x0 + ex + 4, y + 8, es - 8, es - 8, [230, 230, 230], h);
        }
        // The thumbnail: nearest sample of the layer over the checkerboard.
        let (tx, ts) = THUMB;
        for v in 0..ts {
            for u in 0..ts {
                let lx = (u as u64 * l.width as u64 / ts as u64) as i64;
                let ly = (v as u64 * l.height as u64 / ts as u64) as i64;
                let c = l.pixel(lx, ly).unwrap_or([0; 4]);
                put(frame, stride, x0 + tx + u, y + 4 + v, over_grey(c, check(u * 2, v * 2)));
            }
        }
        // The opacity bar and the blend ticks.
        let (bx, bw) = BAR;
        rect(frame, stride, x0 + bx, y + 6, bw, 4, [70, 70, 70], h);
        rect(frame, stride, x0 + bx, y + 6, bw * l.opacity as u32 / 255, 4, [120, 200, 120], h);
        let lit = l.blend.to_byte() as u32;
        for k in 0..8u32 {
            let rgb = if k == lit { [240, 200, 80] } else { [90, 90, 90] };
            rect(frame, stride, x0 + bx + k * 12, y + 16, 8, 6, rgb, h);
        }
        if i == active {
            outline(frame, stride, x0, y, PANEL_W, ROW_H, [140, 170, 255], h);
        }
    }
    // The colour swatch below the rows.
    let sy = n as u32 * ROW_H + 8;
    if sy + 24 <= h {
        for v in 0..24 {
            for u in 0..48 {
                put(frame, stride, x0 + 8 + u, sy + v, over_grey(colour, check(u, v)));
            }
        }
        outline(frame, stride, x0 + 8, sy, 48, 24, [200, 200, 200], h);
    }
}
