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

/// One Y'CbCr sample → RGB by ITU-R BT.601 (studio range: Y' 16..235, Cb/Cr 16..240): yuv_core's ONE converter
/// (VP8COLOURONE SR356), `Matrix::Kr(KR_KB_601)` limited at 8 bits — the 8-bit fixed-point table (x256, rounded).
pub fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let (yp, up, vp) = ([y], [u], [v]);
    let p = yuv_core::Picture { w: 1, h: 1, y: &yp[..], u: &up[..], v: &vp[..], y_stride: 1, uv_stride: 1, ss: (0, 0), mono: false, bit_depth: 8, full: false };
    let mut out = [0u8; 4];
    yuv_core::to_rgba(&p, yuv_core::Matrix::Kr(yuv_core::KR_KB_601), yuv_core::Chroma::Nearest, false, &mut out);
    [out[0], out[1], out[2]]
}

/// YUY2 (Y0 U Y1 V per pixel pair) as RGBA: each row de-interleaved into Y / Cb / Cr and converted as one 4:2:2 row by
/// yuv_core (BT.601 limited, nearest chroma). An odd width's last pixel stays zero, as the pairs do not cover it.
pub fn yuy2_to_rgba(frame: &Frame) -> Vec<u8> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut out = vec![0u8; w * h * 4];
    if frame.format != PixelFormat::Yuy2 {
        return out;
    }
    let pairs = w / 2;
    let (mut ys, mut us, mut vs) = (vec![0u8; pairs * 2], vec![0u8; pairs], vec![0u8; pairs]);
    for y in 0..h {
        let row = &frame.data[y * frame.stride..];
        for pair in 0..pairs {
            (ys[pair * 2], us[pair], ys[pair * 2 + 1], vs[pair]) = (row[pair * 4], row[pair * 4 + 1], row[pair * 4 + 2], row[pair * 4 + 3]);
        }
        let p = yuv_core::Picture { w: pairs * 2, h: 1, y: &ys[..], u: &us[..], v: &vs[..], y_stride: pairs * 2, uv_stride: pairs, ss: (1, 0), mono: false, bit_depth: 8, full: false };
        let o = y * w * 4;
        yuv_core::to_rgba(&p, yuv_core::Matrix::Kr(yuv_core::KR_KB_601), yuv_core::Chroma::Nearest, false, &mut out[o..o + pairs * 8]);
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
