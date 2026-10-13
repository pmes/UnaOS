// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Deterministic synthetic frames — what a host with no camera is fed (the Sonde handler's replay probe)
//! and what the tests score the depth model against.
//!
//! The depth scene is a PLANE tilted about the vertical axis, so every pixel's depth is known in closed
//! form and a deprojected cloud must come back planar: depth at column `x` is `near_mm + x * slope_mm`
//! (integer millimetres, at the D400 default scale of 1 mm per unit), with a rectangular HOLE (raw 0) in
//! the middle so the hole path is exercised. The colour scene is a set of vertical YUY2 bars whose
//! colours are a function of the frame number, so a face that repaints shows motion.

use alloc::vec;

use crate::frame::{Frame, PixelFormat, StreamKind};

/// The replay probe's id on the bus.
pub const REPLAY_ID: &str = "replay:d435i";
/// The replay probe's description (the D435i is the probe it stands in for).
pub const REPLAY_NAME: &str = "Intel RealSense D435i (replay)";

/// The tilted-plane depth frame. `near_mm` at column 0, rising by `slope_mm` per column; the hole is the
/// centre quarter of the picture (`w/4..w*3/4` × `h/4..h*3/4`) shifted by `seq % 8` columns so it moves.
pub fn depth_plane(width: u32, height: u32, near_mm: u16, slope_mm: u16, seq: u64) -> Frame {
    let mut data = vec![0u8; width as usize * height as usize * 2];
    let shift = (seq % 8) as u32;
    for y in 0..height {
        for x in 0..width {
            let in_hole = x >= width / 4 + shift
                && x < width * 3 / 4 + shift
                && y >= height / 4
                && y < height * 3 / 4;
            let v: u16 = if in_hole { 0 } else { near_mm.saturating_add((x as u16).saturating_mul(slope_mm)) };
            let o = (y as usize * width as usize + x as usize) * 2;
            data[o..o + 2].copy_from_slice(&v.to_le_bytes());
        }
    }
    // 30 fps on the probe's clock: 33_333 us per frame.
    Frame::packed(StreamKind::Depth, PixelFormat::Z16, width, height, seq * 33_333, seq, data)
        .expect("depth_plane sizes its own buffer")
}

/// Eight vertical YUY2 colour bars; the bar order rotates by one every frame. `width` is rounded down to
/// even (YUY2 carries pixel pairs).
pub fn color_bars(width: u32, height: u32, seq: u64) -> Frame {
    let width = (width & !1).max(2);
    // (Y, U, V) of eight bars — 75 % bars in BT.601 studio range.
    const BARS: [(u8, u8, u8); 8] = [
        (180, 128, 128), // white
        (162, 44, 142),  // yellow
        (131, 156, 44),  // cyan
        (112, 72, 58),   // green
        (84, 184, 198),  // magenta
        (65, 100, 212),  // red
        (35, 212, 114),  // blue
        (16, 128, 128),  // black
    ];
    let stride = width as usize * 2;
    let mut data = vec![0u8; stride * height as usize];
    for y in 0..height as usize {
        for pair in 0..(width / 2) as usize {
            let bar = ((pair * 2 * 8 / width as usize) + seq as usize) % 8;
            let (yy, u, v) = BARS[bar];
            let o = y * stride + pair * 4;
            data[o..o + 4].copy_from_slice(&[yy, u, yy, v]);
        }
    }
    Frame::packed(StreamKind::Color, PixelFormat::Yuy2, width, height, seq * 33_333, seq, data)
        .expect("color_bars sizes its own buffer")
}

/// A Y8 infrared frame: a horizontal gradient (the imagers' left-to-right falloff stand-in).
pub fn ir_gradient(width: u32, height: u32, seq: u64) -> Frame {
    let mut data = vec![0u8; width as usize * height as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            data[y * width as usize + x] = ((x * 255) / (width as usize).max(1)) as u8;
        }
    }
    Frame::packed(StreamKind::Infrared, PixelFormat::Y8, width, height, seq * 33_333, seq, data)
        .expect("ir_gradient sizes its own buffer")
}
