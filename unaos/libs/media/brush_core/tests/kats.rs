// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! BRUSHCORE (LEDGER SR231) host KATs. Design: `docs/dev/evidence/raster/BRUSHCORE.md`.

use brush_core::{BlendMode, Brush, Dab, PressureCurve, Stroke, StrokePoint, apply, dabs, isqrt, paint, stamp};

fn pt(x: f32, y: f32, pressure: u8) -> StrokePoint {
    StrokePoint { x, y, pressure }
}

fn stroke(points: &[StrokePoint]) -> Stroke {
    Stroke { points: points.to_vec() }
}

fn alpha(buf: &[u8], w: u32, x: u32, y: u32) -> u8 {
    buf[((y * w + x) * 4 + 3) as usize]
}

fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

// (1) 100 px at diameter 10, spacing 0.25: 1 + 100 / 2.5 = 41 dabs, first and last on the endpoints.
#[test]
fn straight_stroke_41_dabs() {
    let b = Brush::new(10.0);
    let d = dabs(&b, &stroke(&[pt(0.0, 0.0, 255), pt(100.0, 0.0, 255)]));
    assert_eq!(d.len(), 41);
    assert_eq!((d[0].x, d[0].y), (0.0, 0.0));
    assert_eq!((d[40].x, d[40].y), (100.0, 0.0));
    for w in d.windows(2) {
        assert!((w[1].x - w[0].x - 2.5).abs() < 1e-3, "even spacing");
    }
}

// (2) one point, one dab; no point, no dab.
#[test]
fn one_point_one_dab() {
    let b = Brush::new(8.0);
    let d = dabs(&b, &stroke(&[pt(3.0, 4.0, 200)]));
    assert_eq!(d.len(), 1);
    assert_eq!((d[0].x, d[0].y), (3.0, 4.0));
    assert!(dabs(&b, &Stroke::default()).is_empty());
}

// (3) pressure scales radius and flow; Quadratic sits under Linear; a Lut is its table.
#[test]
fn pressure_curves() {
    let mut b = Brush::new(10.0);
    b.flow = 200;
    assert_eq!(b.radius_at(0), 0.0);
    assert_eq!(b.flow_at(0), 0);
    assert_eq!(b.radius_at(255), 5.0);
    assert_eq!(b.flow_at(255), 200);
    assert!(PressureCurve::Quadratic.map(128) < PressureCurve::Linear.map(128));
    assert_eq!(PressureCurve::Quadratic.map(128), 64);
    let mut t = [0u8; 256];
    for (i, v) in t.iter_mut().enumerate() {
        *v = 255 - i as u8;
    }
    let lut = PressureCurve::Lut(t);
    for p in 0..=255u8 {
        assert_eq!(lut.map(p), t[p as usize]);
    }
    // Pressure interpolates along a segment: the middle dab of a 0 -> 255 ramp is at half radius.
    let d = dabs(&Brush::new(10.0), &stroke(&[pt(0.0, 0.0, 0), pt(5.0, 0.0, 255)]));
    assert_eq!(d.len(), 3);
    assert_eq!(d[1].x, 2.5);
    assert!((d[1].radius - 2.5).abs() < 0.02);
}

fn dab(x: f32, y: f32, radius: f32, hardness: u8) -> Dab {
    Dab { x, y, radius, flow: 255, hardness }
}

// (4) hard radius 4: area within 2 of pi * 16, inner pixels full; soft: monotone falloff.
#[test]
fn hard_and_soft_dabs() {
    let s = stamp(&dab(8.0, 8.0, 4.0, 255));
    let area = s.coverage.iter().map(|&c| c as f64).sum::<f64>() / 255.0;
    assert!((area - 16.0 * std::f64::consts::PI).abs() <= 2.0, "area {area}");
    for y in 0..16i64 {
        for x in 0..16i64 {
            let (dx, dy) = (x as f64 + 0.5 - 8.0, y as f64 + 0.5 - 8.0);
            if dx * dx + dy * dy <= 3.5 * 3.5 {
                assert_eq!(s.at(x, y), 255, "inner ({x},{y})");
            }
            if dx * dx + dy * dy >= 4.5 * 4.5 {
                assert_eq!(s.at(x, y), 0, "outer ({x},{y})");
            }
        }
    }
    let soft = stamp(&dab(8.0, 8.0, 4.0, 0));
    let row: Vec<u8> = (8..14).map(|x| soft.at(x, 8)).collect();
    assert!(row[0] > 0 && row[0] < 255, "soft centre {row:?}");
    assert!(row.windows(2).all(|w| w[1] <= w[0]), "monotone {row:?}");
    assert_eq!(*row.last().unwrap(), 0);
    assert_eq!(isqrt(0), 0);
    assert_eq!(isqrt(99), 9);
    assert_eq!(isqrt(100), 10);
    assert_eq!(isqrt(u64::MAX), 4294967295);
}

// (5) one opaque Normal dab on transparent: alpha is exactly the stamp's coverage, the colour is the brush's.
#[test]
fn opaque_normal_dab_alpha_is_coverage() {
    let (w, h) = (16u32, 16u32);
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let b = Brush::new(9.0);
    let st = stroke(&[pt(8.0, 8.0, 255)]);
    paint(&mut buf, w, h, &b, &st, [200, 50, 10, 255]);
    let s = stamp(&dabs(&b, &st)[0]);
    for y in 0..h {
        for x in 0..w {
            assert_eq!(alpha(&buf, w, x, y), s.at(x as i64, y as i64), "({x},{y})");
        }
    }
    assert_eq!(px(&buf, w, 8, 8), [200, 50, 10, 255]);
}

// (6) Erase over opaque: centre alpha 0, colour kept.
#[test]
fn erase_dab_keeps_colour() {
    let (w, h) = (16u32, 16u32);
    let mut buf = [10u8, 20, 30, 255].repeat((w * h) as usize);
    let mut b = Brush::new(6.0);
    b.mode = BlendMode::Erase;
    paint(&mut buf, w, h, &b, &stroke(&[pt(8.0, 8.0, 255)]), [255, 255, 255, 255]);
    assert_eq!(px(&buf, w, 8, 8), [10, 20, 30, 0]);
    assert_eq!(px(&buf, w, 0, 0), [10, 20, 30, 255], "outside the dab");
}

// (7) flow 128 twice builds more alpha than once and stays under 255.
#[test]
fn flow_accumulates() {
    let (w, h) = (16u32, 16u32);
    let mut b = Brush::new(6.0);
    b.flow = 128;
    let st = stroke(&[pt(8.0, 8.0, 255)]);
    let mut once = vec![0u8; (w * h * 4) as usize];
    paint(&mut once, w, h, &b, &st, [255, 0, 0, 255]);
    let mut twice = once.clone();
    paint(&mut twice, w, h, &b, &st, [255, 0, 0, 255]);
    let (a1, a2) = (alpha(&once, w, 8, 8), alpha(&twice, w, 8, 8));
    assert_eq!(a1, 128);
    assert!(a2 > a1 && a2 < 255, "once {a1} twice {a2}");
}

// (8) a dab centred off-canvas clips and paints only the visible part.
#[test]
fn off_canvas_dab_clips() {
    let (w, h) = (16u32, 16u32);
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let s = stamp(&dab(-3.0, -3.0, 5.0, 255));
    assert!(s.x0 < 0 && s.y0 < 0);
    apply(&mut buf, w, h, &s, [0, 0, 255, 255], BlendMode::Normal);
    let mut painted = 0;
    for y in 0..h {
        for x in 0..w {
            let c = s.at(x as i64, y as i64);
            assert_eq!(alpha(&buf, w, x, y), c, "({x},{y})");
            painted += (c > 0) as u32;
        }
    }
    let visible = (s.y0..s.y0 + s.h as i32)
        .flat_map(|y| (s.x0..s.x0 + s.w as i32).map(move |x| (x, y)))
        .filter(|&(x, y)| s.at(x as i64, y as i64) > 0 && x >= 0 && y >= 0)
        .count() as u32;
    let total = s.coverage.iter().filter(|&&c| c > 0).count() as u32;
    assert!(painted > 0 && painted == visible && visible < total, "painted {painted} visible {visible} total {total}");
    // A wrong-length buffer is untouched, never a panic.
    let mut short = vec![0u8; 10];
    apply(&mut short, w, h, &s, [0, 0, 255, 255], BlendMode::Normal);
    assert!(short.iter().all(|&b| b == 0));
}
