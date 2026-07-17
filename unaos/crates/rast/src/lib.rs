// `no_std` for every real (cross-compiled) build; `std` links only under `cargo test` so the
// host golden-image oracle runs with a plain `cargo test`. The crate itself uses only `core`
// (+ `libm`), so this changes nothing about the kernel build.
#![cfg_attr(not(test), no_std)]
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

//! # `rast` — the UnaOS software rasterizer
//!
//! A small, deterministic, `no_std`, zero-alloc-in-the-hot-path software 3D
//! renderer. Two jobs:
//!
//! 1. **3D on every platform today.** It rasterizes flat-shaded, z-buffered
//!    triangles into a caller-provided RGBA8 framebuffer slice, so any UnaOS
//!    target with a framebuffer (x86/virt, Pi, Orin) can draw 3D through the
//!    existing panel path.
//! 2. **The V3D pixel-exact oracle.** Because it is platform-neutral and
//!    byte-deterministic (same scene ⇒ identical pixels on x86_64 and aarch64),
//!    it is the reference output future GPU (V3D) rendering is diffed against:
//!    "GPU output == rasterizer reference".
//!
//! ## Determinism contract
//! Every float is `f32`; every float operation is one of the IEEE-754 primitives
//! (`+ - * /`, comparisons) or a `libm` call (`sqrtf`/`floorf`/`sinf`/`cosf`).
//! `libm` is pure-Rust and correctly-rounded, so it is byte-identical on both
//! arches (the same guarantee `libs/fs/unafs` relies on). No fused multiply-add,
//! no platform libm, no fast-math. See [`math`] for the full rationale.
//!
//! ## Pipeline
//! [`render_mesh`] is the whole thing: model→world→clip transform, flat-shade
//! from the world-space face normal, near-plane (w) clip, perspective divide,
//! viewport map, back-face cull, and the z-buffered fill (see [`raster`]).

pub mod math;
pub mod raster;

pub use math::{Mat4, Vec3, Vec4};
pub use raster::{Rgba, ScreenVert, Target, Z_FAR};

/// Clip-space `w` below this is treated as behind the near plane and clipped
/// away, so the perspective divide never sees a zero/negative `w`.
const W_EPS: f32 = 1.0e-5;

/// Flat-shade a face: Lambert `max(0, n·l)` of a base color, plus a fixed ambient
/// floor so back-lit faces aren't pure black. `normal` and `light_dir` need not be
/// normalized (we normalize here). Deterministic (only `math` primitives).
pub fn shade_lambert(normal: Vec3, light_dir: Vec3, base: Rgba, ambient: f32) -> Rgba {
    let n = normal.normalize();
    let l = light_dir.normalize();
    let d = n.dot(l);
    let diff = if d > 0.0 { d } else { 0.0 };
    let k = (ambient + (1.0 - ambient) * diff).min(1.0);
    let scale = |c: u8| -> u8 {
        // Round-to-nearest of c*k, clamped. Deterministic integer-ish path.
        let v = (c as f32) * k + 0.5;
        let v = if v > 255.0 { 255.0 } else { v };
        v as u8
    };
    Rgba([scale(base.0[0]), scale(base.0[1]), scale(base.0[2]), base.0[3]])
}

/// A clip-space vertex produced by the transform stage, carried through the
/// near-plane clipper before the perspective divide.
#[derive(Clone, Copy)]
struct ClipVert {
    pos: Vec4,
}

/// Map a post-divide NDC point to screen space. NDC x∈[-1,1] → [0,width],
/// y∈[-1,1] (up) → [0,height] (down). Depth carried through as NDC z.
#[inline]
fn to_screen(ndc: Vec4, width: f32, height: f32) -> ScreenVert {
    ScreenVert {
        x: (ndc.x * 0.5 + 0.5) * width,
        y: (0.5 - ndc.y * 0.5) * height,
        z: ndc.z,
    }
}

/// Render an indexed triangle mesh into `target`.
///
/// - `model`: model→world transform (used both to place geometry and to derive
///   the world-space face normal for flat shading).
/// - `view_proj`: world→clip transform (view × projection).
/// - `verts` / `indices`: the mesh; `indices` is a flat list of triples.
/// - `base_color`, `light_dir`, `ambient`: flat-shading inputs.
/// - `cull`: cull back faces (CCW-front) when `true`.
///
/// Returns the number of triangles that were rasterized (post-cull, post-clip).
#[allow(clippy::too_many_arguments)]
pub fn render_mesh(
    target: &mut Target,
    model: &Mat4,
    view_proj: &Mat4,
    verts: &[Vec3],
    indices: &[u32],
    base_color: Rgba,
    light_dir: Vec3,
    ambient: f32,
    cull: bool,
) -> u32 {
    let w = target.width() as f32;
    let h = target.height() as f32;
    let mut drawn = 0u32;

    let mut tri = indices.chunks_exact(3);
    for t in &mut tri {
        let (i0, i1, i2) = (t[0] as usize, t[1] as usize, t[2] as usize);
        if i0 >= verts.len() || i1 >= verts.len() || i2 >= verts.len() {
            continue;
        }
        // Model → world (for shading normal).
        let wp0 = model.transform(Vec4::point(verts[i0]));
        let wp1 = model.transform(Vec4::point(verts[i1]));
        let wp2 = model.transform(Vec4::point(verts[i2]));
        let world = |p: Vec4| Vec3::new(p.x, p.y, p.z);
        let normal = world(wp1).sub(world(wp0)).cross(world(wp2).sub(world(wp0)));
        let color = shade_lambert(normal, light_dir, base_color, ambient);

        // World → clip.
        let c0 = ClipVert { pos: view_proj.transform(wp0) };
        let c1 = ClipVert { pos: view_proj.transform(wp1) };
        let c2 = ClipVert { pos: view_proj.transform(wp2) };

        // Near-plane clip against the `w >= W_EPS` plane. Produces up to 4
        // vertices (fanned into up to 2 triangles) in a fixed-capacity buffer —
        // no allocation.
        let mut poly = [ClipVert { pos: Vec4::new(0.0, 0.0, 0.0, 0.0) }; 4];
        let n = clip_near(&[c0, c1, c2], &mut poly);
        if n < 3 {
            continue; // wholly behind the near plane
        }

        // Fan-triangulate the clipped polygon and rasterize each triangle.
        for k in 1..(n - 1) {
            let sv = [
                divide_and_map(poly[0].pos, w, h),
                divide_and_map(poly[k].pos, w, h),
                divide_and_map(poly[k + 1].pos, w, h),
            ];
            if target.triangle(sv, color, cull) {
                drawn += 1;
            }
        }
    }
    drawn
}

/// Perspective divide + viewport map for one clip-space vertex.
#[inline]
fn divide_and_map(clip: Vec4, w: f32, h: f32) -> ScreenVert {
    let inv_w = 1.0 / clip.w;
    let ndc = Vec4::new(clip.x * inv_w, clip.y * inv_w, clip.z * inv_w, 1.0);
    to_screen(ndc, w, h)
}

/// Sutherland–Hodgman clip of a triangle against the single plane `w >= W_EPS`
/// (the near plane, in the robust homogeneous form that never divides by a
/// non-positive `w`). Writes the resulting polygon into `out` (capacity 4) and
/// returns its vertex count (0 or 3..=4).
fn clip_near(input: &[ClipVert; 3], out: &mut [ClipVert; 4]) -> usize {
    let inside = |v: &ClipVert| v.pos.w >= W_EPS;
    let mut count = 0usize;
    for i in 0..3 {
        let cur = input[i];
        let prev = input[(i + 2) % 3]; // the vertex before `cur`
        let cur_in = inside(&cur);
        let prev_in = inside(&prev);
        if cur_in != prev_in {
            // Edge crosses the plane: emit the intersection.
            // Solve prev.w + t*(cur.w - prev.w) = W_EPS.
            let denom = cur.pos.w - prev.pos.w;
            let t = if denom == 0.0 {
                0.0
            } else {
                (W_EPS - prev.pos.w) / denom
            };
            let pos = prev.pos.lerp(cur.pos, t);
            if count < 4 {
                out[count] = ClipVert { pos };
                count += 1;
            }
        }
        if cur_in && count < 4 {
            out[count] = cur;
            count += 1;
        }
    }
    count
}
