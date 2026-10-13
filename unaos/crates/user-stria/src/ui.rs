// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// STRIAMETAL (SR139): the back buffer's primitives — a filled rect and the font8x8 grid (8 x 8 px glyphs, the SAME
// grid Aether's fallback draws with, user-aether txt.rs), clipped to the window. Colours are 0RGB; the surface copy
// (`main::present`) sets the alpha byte.

use crate::{MAX_H, MAX_W};

pub const W: usize = MAX_W as usize;
pub const H: usize = MAX_H as usize;

/// Fill `[x, x + w) x [y, y + h)` with `c`, clipped.
pub fn fill(fb: &mut [u32], x: i32, y: i32, w: i32, h: i32, c: u32) {
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + w).min(MAX_W), (y + h).min(MAX_H));
    for yy in y0..y1 {
        let row = yy as usize * W;
        for xx in x0..x1 {
            fb[row + xx as usize] = c;
        }
    }
}

/// Draw `s` (ASCII; anything else is `?`) with its top-left at `(x, y)`; returns the pen's x after it.
pub fn text(fb: &mut [u32], x: i32, y: i32, s: &str, c: u32) -> i32 {
    let mut pen = x;
    for ch in s.chars() {
        let g = if (' '..='~').contains(&ch) { ch as usize } else { b'?' as usize };
        let rows = font8x8::legacy::BASIC_LEGACY[g];
        for (ry, row) in rows.iter().enumerate() {
            for cx in 0..8 {
                if row & (1 << cx) != 0 {
                    let (px, py) = (pen + cx, y + ry as i32);
                    if px >= 0 && py >= 0 && px < MAX_W && py < MAX_H {
                        fb[py as usize * W + px as usize] = c;
                    }
                }
            }
        }
        pen += 8;
    }
    pen
}

/// `s` cut to what fits `cols` glyphs (a leading `..` marks the cut; paths keep their tail).
pub fn fit(s: &str, cols: usize) -> alloc::string::String {
    let n = s.chars().count();
    if n <= cols || cols < 3 {
        return alloc::string::String::from(s);
    }
    let tail: alloc::string::String = s.chars().skip(n - (cols - 2)).collect();
    alloc::format!("..{}", tail)
}

/// `m:ss` for `ms`.
pub fn clock_text(ms: u64) -> alloc::string::String {
    let s = ms / 1000;
    alloc::format!("{}:{:02}", s / 60, s % 60)
}
