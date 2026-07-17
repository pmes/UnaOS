// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The rasterization target and the flat-shaded, z-buffered triangle fill.
//!
//! # Pixel format
//! The color framebuffer is a caller-provided `&mut [u8]` of **RGBA8** — four
//! bytes per pixel in `[R, G, B, A]` order, row-major, addressed by a caller-
//! supplied `stride` (in pixels). This is the crate's one canonical format; the
//! kernel wire-in converts RGBA8 → the panel's native Rgb/Bgr on blit. "Do it
//! right": we own the format, so it's the simple one.
//!
//! # Depth buffer
//! A parallel `&mut [f32]` z-buffer (one entry per pixel, same `stride`). Smaller
//! z is nearer; cleared to [`Z_FAR`]. The fill writes a fragment only when its
//! interpolated depth is strictly less than the stored value, so z-fighting
//! resolves deterministically by draw order (equal depth ⇒ first writer wins).
//!
//! # Determinism
//! No allocation in the hot path (the caller owns both buffers). The edge
//! functions, the top-left fill rule, and the barycentric depth interpolation
//! are all built from the deterministic `f32` primitives (see [`crate::math`]).

/// A packed RGBA8 color, `[R, G, B, A]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    #[inline]
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba([r, g, b, a])
    }
    #[inline]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Rgba([r, g, b, 0xFF])
    }
}

/// The z-buffer clear value / far plane. Anything the caller draws is nearer.
pub const Z_FAR: f32 = f32::INFINITY;

/// A vertex already projected to the screen: pixel-space `x`/`y` (sub-pixel `f32`)
/// and a depth `z` (any monotonic-with-distance value; NDC z in `[-1, 1]` from the
/// pipeline). The raster stage is pure 2D — all clipping/culling happened upstream.
#[derive(Clone, Copy, Debug)]
pub struct ScreenVert {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// A mutable render target: an RGBA8 color plane and a matching f32 depth plane,
/// both borrowed from the caller (no allocation).
pub struct Target<'a> {
    color: &'a mut [u8],
    depth: &'a mut [f32],
    width: usize,
    height: usize,
    /// Row stride in **pixels** (color plane strides by `4*stride` bytes, depth by `stride`).
    stride: usize,
}

impl<'a> Target<'a> {
    /// Wrap caller buffers. `color` must be `>= 4*stride*height` bytes and `depth`
    /// `>= stride*height` entries, with `stride >= width`. Returns `None` otherwise,
    /// so a mis-sized buffer can never cause an out-of-bounds write.
    pub fn new(
        color: &'a mut [u8],
        depth: &'a mut [f32],
        width: usize,
        height: usize,
        stride: usize,
    ) -> Option<Self> {
        if stride < width
            || color.len() < 4 * stride * height
            || depth.len() < stride * height
        {
            return None;
        }
        Some(Self { color, depth, width, height, stride })
    }

    #[inline]
    pub fn width(&self) -> usize {
        self.width
    }
    #[inline]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Clear color to `c` and depth to [`Z_FAR`].
    pub fn clear(&mut self, c: Rgba) {
        for y in 0..self.height {
            let row = y * self.stride;
            for x in 0..self.width {
                let ci = (row + x) * 4;
                self.color[ci..ci + 4].copy_from_slice(&c.0);
                self.depth[row + x] = Z_FAR;
            }
        }
    }

    #[inline]
    fn put(&mut self, x: usize, y: usize, c: Rgba) {
        let ci = (y * self.stride + x) * 4;
        self.color[ci..ci + 4].copy_from_slice(&c.0);
    }

    /// Read a pixel (test/oracle helper).
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> Rgba {
        let ci = (y * self.stride + x) * 4;
        Rgba([
            self.color[ci],
            self.color[ci + 1],
            self.color[ci + 2],
            self.color[ci + 3],
        ])
    }

    /// Read a depth entry (test/oracle helper).
    #[inline]
    pub fn depth_at(&self, x: usize, y: usize) -> f32 {
        self.depth[y * self.stride + x]
    }

    /// The raw color plane (RGBA8) — for the kernel blit.
    #[inline]
    pub fn color_bytes(&self) -> &[u8] {
        self.color
    }

    /// Fill a flat-shaded, z-buffered triangle. Vertices are already in screen
    /// space. Returns `true` if the triangle was rasterized, `false` if it was
    /// culled (back-facing) or degenerate (zero screen area).
    ///
    /// Winding: front faces are **counter-clockwise as drawn on the y-down
    /// screen** — which is a *negative* signed area under this edge function
    /// (the y-down flip inverts orientation). Back faces (area >= 0) are culled.
    /// Pass `cull = false` to draw both sides.
    pub fn triangle(&mut self, v: [ScreenVert; 3], color: Rgba, cull: bool) -> bool {
        // Signed area via the edge function on the three screen-space points.
        // area2 = (v1-v0) x (v2-v0)
        let area2 = edge(v[0], v[1], v[2]);
        if area2 == 0.0 {
            return false; // degenerate / zero-area
        }
        if cull && area2 >= 0.0 {
            return false; // back-facing
        }

        // For a consistently-oriented interior test we normalize to a positive
        // area by flipping the vertex order when needed (only when cull=false,
        // since a culled back-face already returned).
        let (a, b, c) = if area2 > 0.0 {
            (v[0], v[1], v[2])
        } else {
            (v[0], v[2], v[1])
        };
        let inv_area = 1.0 / edge(a, b, c); // > 0 now

        // Bounding box, clamped to the frame.
        let min_x = floor3(a.x, b.x, c.x).max(0.0);
        let min_y = floor3(a.y, b.y, c.y).max(0.0);
        let max_x = ceil3(a.x, b.x, c.x).min((self.width as f32) - 1.0);
        let max_y = ceil3(a.y, b.y, c.y).min((self.height as f32) - 1.0);
        if max_x < min_x || max_y < min_y {
            return true; // fully off-screen but was a valid (accepted) triangle
        }
        let x0 = min_x as usize;
        let y0 = min_y as usize;
        let x1 = max_x as usize;
        let y1 = max_y as usize;

        for py in y0..=y1 {
            for px in x0..=x1 {
                // Sample at pixel center.
                let p = ScreenVert {
                    x: px as f32 + 0.5,
                    y: py as f32 + 0.5,
                    z: 0.0,
                };
                let w0 = edge(b, c, p);
                let w1 = edge(c, a, p);
                let w2 = edge(a, b, p);
                // Inside if all edge functions are >= 0, honoring the top-left
                // fill rule so shared edges between adjacent triangles are
                // covered by exactly one (no double-draw, no seam).
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                if (w0 == 0.0 && !is_top_left(b, c))
                    || (w1 == 0.0 && !is_top_left(c, a))
                    || (w2 == 0.0 && !is_top_left(a, b))
                {
                    continue;
                }
                // Barycentric depth interpolation.
                let l0 = w0 * inv_area;
                let l1 = w1 * inv_area;
                let l2 = w2 * inv_area;
                let z = l0 * a.z + l1 * b.z + l2 * c.z;
                let di = py * self.stride + px;
                if z < self.depth[di] {
                    self.depth[di] = z;
                    self.put(px, py, color);
                }
            }
        }
        true
    }
}

/// The 2D edge function: twice the signed area of triangle (a, b, c). Positive
/// when (a, b, c) wind counter-clockwise under a y-down screen convention.
#[inline]
fn edge(a: ScreenVert, b: ScreenVert, c: ScreenVert) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

/// Top-left rule: an edge is "top-left" (its boundary pixels belong to this
/// triangle) if it is a top edge (horizontal, going left — i.e. dy==0 && dx<0)
/// or a left edge (going down — dy > 0, y-down convention). Ensures shared edges
/// rasterize into exactly one triangle.
#[inline]
fn is_top_left(a: ScreenVert, b: ScreenVert) -> bool {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dy == 0.0 && dx < 0.0) || dy > 0.0
}

#[inline]
fn floor3(a: f32, b: f32, c: f32) -> f32 {
    crate::math::floor(a.min(b).min(c))
}

#[inline]
fn ceil3(a: f32, b: f32, c: f32) -> f32 {
    // ceil(x) = -floor(-x)
    -crate::math::floor(-(a.max(b).max(c)))
}
