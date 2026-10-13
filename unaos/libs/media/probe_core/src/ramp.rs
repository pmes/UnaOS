// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The faces' pixels: every frame format as tightly packed RGBA8 (row-major, top row first — the
//! `MediaFrame` / `SurfaceBlit` convention), plus a nearest-neighbour blit for composing panes.
//!
//! The depth ramp is UnaOS's own: near is blue, then cyan, green, yellow, far is red, holes are black —
//! a five-stop ramp over `near_m..far_m` in 1024 integer steps, so it is identical on every ring (no
//! floating point after the one division that places a sample on the ramp).

use alloc::vec;
use alloc::vec::Vec;

use crate::depth::DepthScale;
use crate::frame::{Frame, PixelFormat};

/// The ramp's colour for position `t` in `0..=1023` (0 = near, 1023 = far).
pub fn ramp_rgb(t: u32) -> [u8; 3] {
    let t = t.min(1023);
    let seg = t / 256; // 0..=3
    let f = (t % 256) as u8; // 0..=255 within the segment
    match seg {
        0 => [0, f, 255],       // blue -> cyan
        1 => [0, 255, 255 - f], // cyan -> green
        2 => [f, 255, 0],       // green -> yellow
        _ => [255, 255 - f, 0], // yellow -> red
    }
}

/// A Z16 frame as RGBA through the ramp over `near_m..far_m`. A non-Z16 frame, or a range that is empty
/// or inverted, yields an all-black picture of the frame's size (never a panic, never a guess).
pub fn depth_to_rgba(frame: &Frame, scale: DepthScale, near_m: f32, far_m: f32) -> Vec<u8> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut out = vec![0u8; w * h * 4];
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    if frame.format != PixelFormat::Z16 || !(far_m > near_m) {
        return out;
    }
    let span = far_m - near_m;
    for y in 0..frame.height {
        for x in 0..frame.width {
            let Some(raw) = frame.u16_at(x, y) else { continue };
            let Some(m) = scale.metres(raw) else { continue };
            let t = if m <= near_m {
                0
            } else if m >= far_m {
                1023
            } else {
                ((m - near_m) / span * 1023.0) as u32
            };
            let c = ramp_rgb(t);
            let o = (y as usize * w + x as usize) * 4;
            out[o..o + 3].copy_from_slice(&c);
        }
    }
    out
}

/// Y8 / Y16 as grey RGBA.
pub fn luma_to_rgba(frame: &Frame) -> Vec<u8> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..frame.height {
        for x in 0..frame.width {
            let l = frame.luma_at(x, y).unwrap_or(0);
            let o = (y as usize * w + x as usize) * 4;
            out[o..o + 4].copy_from_slice(&[l, l, l, 255]);
        }
    }
    out
}

#[inline]
fn clamp8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// One Y'CbCr sample → RGB by ITU-R BT.601 (studio range: Y' 16..235, Cb/Cr 16..240), in the standard
/// 8-bit fixed-point form of that recommendation's matrix (coefficients ×256, rounded).
pub fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let c = y as i32 - 16;
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    [
        clamp8((298 * c + 409 * e + 128) >> 8),
        clamp8((298 * c - 100 * d - 208 * e + 128) >> 8),
        clamp8((298 * c + 516 * d + 128) >> 8),
    ]
}

/// YUY2 (Y0 U Y1 V per pixel pair) as RGBA.
pub fn yuy2_to_rgba(frame: &Frame) -> Vec<u8> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut out = vec![0u8; w * h * 4];
    if frame.format != PixelFormat::Yuy2 {
        return out;
    }
    for y in 0..h {
        let row = &frame.data[y * frame.stride..];
        for pair in 0..w / 2 {
            let (y0, u, y1, v) = (row[pair * 4], row[pair * 4 + 1], row[pair * 4 + 2], row[pair * 4 + 3]);
            let o = (y * w + pair * 2) * 4;
            let a = yuv_to_rgb(y0, u, v);
            let b = yuv_to_rgb(y1, u, v);
            out[o..o + 4].copy_from_slice(&[a[0], a[1], a[2], 255]);
            out[o + 4..o + 8].copy_from_slice(&[b[0], b[1], b[2], 255]);
        }
    }
    out
}

/// Any frame as RGBA: depth through the ramp, luminance as grey, YUY2 as colour.
pub fn to_rgba(frame: &Frame, scale: DepthScale, near_m: f32, far_m: f32) -> Vec<u8> {
    match frame.format {
        PixelFormat::Z16 => depth_to_rgba(frame, scale, near_m, far_m),
        PixelFormat::Y8 | PixelFormat::Y16 => luma_to_rgba(frame),
        PixelFormat::Yuy2 => yuy2_to_rgba(frame),
    }
}

/// Nearest-neighbour blit of an RGBA `sw x sh` picture into the `dw x dh` rectangle at `(dx, dy)` of an
/// RGBA destination `dst_w` pixels wide. Clipped to the destination; a short source is skipped whole.
#[allow(clippy::too_many_arguments)]
pub fn blit_scaled(
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    dx: u32,
    dy: u32,
    dw: u32,
    dh: u32,
    src: &[u8],
    sw: u32,
    sh: u32,
) {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() < sw as usize * sh as usize * 4 {
        return;
    }
    if dst.len() < dst_w as usize * dst_h as usize * 4 {
        return;
    }
    for oy in 0..dh {
        let ty = dy + oy;
        if ty >= dst_h {
            break;
        }
        let sy = (oy as u64 * sh as u64 / dh as u64) as u32;
        for ox in 0..dw {
            let tx = dx + ox;
            if tx >= dst_w {
                break;
            }
            let sx = (ox as u64 * sw as u64 / dw as u64) as u32;
            let s = (sy as usize * sw as usize + sx as usize) * 4;
            let d = (ty as usize * dst_w as usize + tx as usize) * 4;
            dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
}
