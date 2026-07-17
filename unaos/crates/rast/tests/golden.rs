// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Golden-image oracle tests for the `rast` software rasterizer.
//!
//! The crate is platform-neutral and byte-deterministic, so these host-side
//! `cargo test` runs are the pixel-exact reference future V3D (GPU) output is
//! diffed against. Each "golden" is captured as an FNV-1a digest of the whole
//! RGBA8 color plane (a full-buffer checksum), so any single-pixel drift trips
//! the test. Coverage: single triangle, z-fighting draw order, near-plane clip,
//! degenerate (zero-area) triangles, and a rendered-scene determinism/golden.

use rast::math::PI;
use rast::raster::{Rgba, ScreenVert, Target};
use rast::{render_mesh, Mat4, Vec3};

/// FNV-1a 64-bit over a byte slice — the golden-image digest.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

const W: usize = 64;
const H: usize = 64;

/// Allocate a `W×H` RGBA8 + depth target (host test — `std` is linked here).
fn make_buffers() -> (Vec<u8>, Vec<f32>) {
    (vec![0u8; 4 * W * H], vec![0f32; W * H])
}

const BG: Rgba = Rgba::rgb(0, 0, 0);

// ---------------------------------------------------------------------------
// M2: single triangle
// ---------------------------------------------------------------------------

#[test]
fn single_triangle_covers_center_not_corners() {
    let (mut color, mut depth) = make_buffers();
    let mut t = Target::new(&mut color, &mut depth, W, H, W).unwrap();
    t.clear(BG);
    // A large CCW (screen y-down) triangle covering the middle of the frame.
    let red = Rgba::rgb(0xFF, 0x20, 0x20);
    let v = [
        ScreenVert { x: 32.0, y: 8.0, z: 0.0 },
        ScreenVert { x: 8.0, y: 56.0, z: 0.0 },
        ScreenVert { x: 56.0, y: 56.0, z: 0.0 },
    ];
    assert!(t.triangle(v, red, true), "front-facing triangle must rasterize");
    assert_eq!(t.get(32, 40), red, "center is inside the triangle");
    assert_eq!(t.get(1, 1), BG, "top-left corner is outside");
    assert_eq!(t.get(62, 2), BG, "top-right corner is outside");
}

#[test]
fn backface_is_culled() {
    let (mut color, mut depth) = make_buffers();
    let mut t = Target::new(&mut color, &mut depth, W, H, W).unwrap();
    t.clear(BG);
    // Clockwise winding under y-down → back-facing.
    let v = [
        ScreenVert { x: 32.0, y: 8.0, z: 0.0 },
        ScreenVert { x: 56.0, y: 56.0, z: 0.0 },
        ScreenVert { x: 8.0, y: 56.0, z: 0.0 },
    ];
    assert!(!t.triangle(v, Rgba::rgb(0xFF, 0, 0), true), "back face culled");
    assert_eq!(t.get(32, 40), BG, "nothing drawn");
    // With culling off, the same triangle draws.
    assert!(t.triangle(v, Rgba::rgb(0xFF, 0, 0), false));
    assert_eq!(t.get(32, 40), Rgba::rgb(0xFF, 0, 0));
}

// ---------------------------------------------------------------------------
// M2: z-fighting / depth order (nearer wins regardless of draw order)
// ---------------------------------------------------------------------------

#[test]
fn depth_test_nearer_wins_both_orders() {
    let near = Rgba::rgb(0x00, 0xFF, 0x00); // z = 0.0
    let far = Rgba::rgb(0x00, 0x00, 0xFF); // z = 0.5
    let quad = |z: f32| {
        [
            ScreenVert { x: 8.0, y: 8.0, z },
            ScreenVert { x: 8.0, y: 56.0, z },
            ScreenVert { x: 56.0, y: 8.0, z },
        ]
    };

    // Order A: far first, then near.
    let (mut c1, mut d1) = make_buffers();
    {
        let mut t = Target::new(&mut c1, &mut d1, W, H, W).unwrap();
        t.clear(BG);
        t.triangle(quad(0.5), far, false);
        t.triangle(quad(0.0), near, false);
        assert_eq!(t.get(20, 20), near, "nearer green must win (far-then-near)");
    }
    // Order B: near first, then far. Depth test must still keep near.
    let (mut c2, mut d2) = make_buffers();
    {
        let mut t = Target::new(&mut c2, &mut d2, W, H, W).unwrap();
        t.clear(BG);
        t.triangle(quad(0.0), near, false);
        t.triangle(quad(0.5), far, false);
        assert_eq!(t.get(20, 20), near, "nearer green must win (near-then-far)");
    }
    // Both orders converge to the identical color plane — the depth buffer makes
    // the result order-independent for disjoint depths.
    assert_eq!(fnv1a(&c1), fnv1a(&c2), "depth resolution is draw-order-independent");
}

// ---------------------------------------------------------------------------
// M2: degenerate (zero-area) triangles
// ---------------------------------------------------------------------------

#[test]
fn degenerate_triangles_draw_nothing() {
    let (mut color, mut depth) = make_buffers();
    let mut t = Target::new(&mut color, &mut depth, W, H, W).unwrap();
    t.clear(BG);
    let before = fnv1a(t.color_bytes());
    // Collinear points.
    let line = [
        ScreenVert { x: 8.0, y: 8.0, z: 0.0 },
        ScreenVert { x: 24.0, y: 24.0, z: 0.0 },
        ScreenVert { x: 40.0, y: 40.0, z: 0.0 },
    ];
    assert!(!t.triangle(line, Rgba::rgb(0xFF, 0, 0), false));
    // All-coincident points.
    let point = [
        ScreenVert { x: 30.0, y: 30.0, z: 0.0 },
        ScreenVert { x: 30.0, y: 30.0, z: 0.0 },
        ScreenVert { x: 30.0, y: 30.0, z: 0.0 },
    ];
    assert!(!t.triangle(point, Rgba::rgb(0xFF, 0, 0), false));
    assert_eq!(before, fnv1a(t.color_bytes()), "zero-area triangles change no pixels");
}

// ---------------------------------------------------------------------------
// M2: near-plane clip
// ---------------------------------------------------------------------------

#[test]
fn near_plane_clip_no_panic_partial_visible() {
    // A triangle straddling the near plane: one vertex behind the eye. The clip
    // must not divide by a non-positive w (no NaN/panic) and must still fill the
    // visible part. We render through the full pipeline.
    let (mut color, mut depth) = make_buffers();
    let mut t = Target::new(&mut color, &mut depth, W, H, W).unwrap();
    t.clear(BG);

    let proj = Mat4::perspective(PI / 3.0, 1.0, 0.5, 100.0);
    let view = Mat4::look_at(
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, -1.0),
        Vec3::new(0.0, 1.0, 0.0),
    );
    let view_proj = proj.mul(&view);
    // One vertex in front of the eye (z<0), one behind (z>0) → straddles near.
    let verts = [
        Vec3::new(-1.0, -1.0, -2.0),
        Vec3::new(1.0, -1.0, -2.0),
        Vec3::new(0.0, 2.0, 1.0), // behind the eye
    ];
    let idx = [0u32, 1, 2];
    let drawn = render_mesh(
        &mut t,
        &Mat4::identity(),
        &view_proj,
        &verts,
        &idx,
        Rgba::rgb(0xFF, 0xFF, 0xFF),
        Vec3::new(0.0, 0.0, 1.0),
        0.3,
        false,
    );
    assert!(drawn >= 1, "clipped triangle still rasterizes its visible part");
    // No NaN leaked into depth: at least one pixel got a finite depth written.
    let mut any_finite = false;
    for y in 0..H {
        for x in 0..W {
            let d = t.depth_at(x, y);
            if d.is_finite() && d < rast::Z_FAR {
                any_finite = true;
            }
            assert!(!d.is_nan(), "no NaN depth from the near clip");
        }
    }
    assert!(any_finite, "some fragment survived the clip");
}

// ---------------------------------------------------------------------------
// Rendered-scene golden + determinism: a fixed cube pose.
// ---------------------------------------------------------------------------

/// The canonical unit cube (8 corners, 12 triangles, CCW outward under y-down).
fn cube() -> (Vec<Vec3>, Vec<u32>) {
    let v = vec![
        Vec3::new(-1.0, -1.0, -1.0),
        Vec3::new(1.0, -1.0, -1.0),
        Vec3::new(1.0, 1.0, -1.0),
        Vec3::new(-1.0, 1.0, -1.0),
        Vec3::new(-1.0, -1.0, 1.0),
        Vec3::new(1.0, -1.0, 1.0),
        Vec3::new(1.0, 1.0, 1.0),
        Vec3::new(-1.0, 1.0, 1.0),
    ];
    // 12 triangles, outward-facing.
    let i = vec![
        0, 2, 1, 0, 3, 2, // -Z
        4, 5, 6, 4, 6, 7, // +Z
        0, 1, 5, 0, 5, 4, // -Y
        3, 7, 6, 3, 6, 2, // +Y
        0, 4, 7, 0, 7, 3, // -X
        1, 2, 6, 1, 6, 5, // +X
    ];
    (v, i)
}

fn render_cube_scene(angle: f32) -> Vec<u8> {
    const CW: usize = 96;
    const CH: usize = 96;
    let mut color = vec![0u8; 4 * CW * CH];
    let mut depth = vec![0f32; CW * CH];
    let mut t = Target::new(&mut color, &mut depth, CW, CH, CW).unwrap();
    t.clear(Rgba::rgb(0x10, 0x10, 0x18));

    let (verts, idx) = cube();
    let model = Mat4::rotation_y(angle).mul(&Mat4::rotation_x(angle * 0.5));
    let proj = Mat4::perspective(PI / 3.0, CW as f32 / CH as f32, 0.5, 100.0);
    let view = Mat4::look_at(
        Vec3::new(0.0, 0.0, 5.0),
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    );
    let view_proj = proj.mul(&view);
    render_mesh(
        &mut t,
        &model,
        &view_proj,
        &verts,
        &idx,
        Rgba::rgb(0x40, 0xB0, 0xFF),
        Vec3::new(0.4, 0.8, 0.6),
        0.25,
        true,
    );
    color
}

#[test]
fn cube_scene_is_deterministic() {
    // Same pose rendered twice ⇒ byte-identical. This is the arch-neutrality
    // guarantee in miniature (the CI on the other arch compares against the same
    // golden digest below).
    let a = render_cube_scene(0.7);
    let b = render_cube_scene(0.7);
    assert_eq!(a, b, "identical scene ⇒ identical bytes");
}

#[test]
fn cube_scene_matches_golden() {
    let digest = fnv1a(&render_cube_scene(0.7));
    // GOLDEN: the pixel-exact reference digest. If this changes, the rasterizer's
    // output changed — regenerate deliberately, never to "make it pass".
    assert_eq!(
        digest, GOLDEN_CUBE_07,
        "cube-scene golden digest drift: got {:#018x}",
        digest
    );
}

// Captured on first green run (see landing report). Byte-identical on x86_64 and
// aarch64 by the determinism contract.
const GOLDEN_CUBE_07: u64 = 0x1944_46bc_a3de_a139;
