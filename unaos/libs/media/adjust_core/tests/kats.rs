// SPDX-License-Identifier: LGPL-3.0-or-later
//! ADJUSTCORE (LEDGER SR232) host KATs. Design: `docs/dev/evidence/raster/ADJUSTCORE.md`.

use adjust_core::*;

fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    (0..w * h).flat_map(|_| rgba).collect()
}

fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

/// A 4x4 gradient with varied alpha.
fn gradient4() -> Vec<u8> {
    (0..16u32).flat_map(|i| [(i * 17) as u8, (255 - i * 17) as u8, (i * 5) as u8, (200 + i) as u8]).collect()
}

fn one(v: u8, p: Levels) -> u8 {
    levels(&[v, v, v, 255], 1, 1, p).unwrap()[0]
}

#[test]
fn levels_kat() {
    let g = gradient4();
    assert_eq!(levels(&g, 4, 4, Levels::IDENTITY).unwrap(), g);
    let p = Levels { in_lo: 64, in_hi: 192, ..Levels::IDENTITY };
    assert_eq!(one(64, p), 0);
    assert_eq!(one(192, p), 255);
    assert_eq!(one(10, p), 0);
    assert_eq!(one(250, p), 255);
    assert_eq!(one(128, p), 128);
    let gam = one(128, Levels { gamma_q8: 128, ..Levels::IDENTITY });
    println!("ADJUSTCORE levels gamma_q8=128: 128 -> {gam}");
    assert!(gam > 128);
    // sqrt(128/255) * 255 = 180.67: the fixed-point power lands on 181.
    assert_eq!(gam, 181);
}

#[test]
fn curves_kat() {
    let id = Curves::identity();
    assert_eq!(Curves::from_points(&[]), id);
    assert_eq!(Curves::from_points(&[(0, 0), (255, 255)]), id);
    let t = Curves::from_points(&[(128, 200)]);
    assert_eq!(t[128], 200);
    assert_eq!(t[64], 100);
    assert_eq!(t[0], 0);
    assert_eq!(t[255], 255);
    let c = Curves { master: t, r: None, g: None, b: None };
    let out = curves(&[128, 64, 0, 7], 1, 1, &c).unwrap();
    assert_eq!(out, vec![200, 100, 0, 7]);
    // A channel curve runs before the master.
    let c = Curves { master: id, r: Some(Curves::from_points(&[(0, 255), (255, 0)])), g: None, b: None };
    assert_eq!(curves(&[10, 10, 10, 9], 1, 1, &c).unwrap(), vec![245, 10, 10, 9]);
}

#[test]
fn color_kat() {
    let red = [255u8, 0, 0, 255];
    let g = hue_sat(&red, 1, 1, HueSat { hue_deg: 120, sat_pct: 0, light_pct: 0 }).unwrap();
    println!("ADJUSTCORE hue +120 on red -> {:?}", &g[..3]);
    for (got, want) in g[..3].iter().zip([0u8, 255, 0]) {
        assert!(got.abs_diff(want) <= 2, "{g:?}");
    }
    let c = [200u8, 80, 30, 255];
    let y = luma(200, 80, 30);
    let d = hue_sat(&c, 1, 1, HueSat { hue_deg: 0, sat_pct: -100, light_pct: 0 }).unwrap();
    println!("ADJUSTCORE sat -100 on {:?} (luma {y}) -> {:?}", &c[..3], &d[..3]);
    for v in &d[..3] {
        assert!(v.abs_diff(y) <= 2, "{d:?}");
    }
    let zero = HueSat { hue_deg: 0, sat_pct: 0, light_pct: 0 };
    let gr = gradient4();
    assert_eq!(hue_sat(&gr, 4, 4, zero).unwrap(), gr);
    assert_eq!(invert(&invert(&gr, 4, 4).unwrap(), 4, 4).unwrap(), gr);
}

#[test]
fn threshold_kat() {
    let grey = [128u8, 128, 128, 255];
    assert_eq!(threshold(&grey, 1, 1, 128).unwrap(), vec![255, 255, 255, 255]);
    assert_eq!(threshold(&grey, 1, 1, 129).unwrap(), vec![0, 0, 0, 255]);
}

#[test]
fn box_blur_kat() {
    let mut img = solid(5, 5, [0, 0, 0, 255]);
    let c = ((2 * 5 + 2) * 4) as usize;
    img[c] = 255;
    img[c + 1] = 255;
    img[c + 2] = 255;
    let b = box_blur(&img, 5, 5, 1).unwrap();
    for y in 0..5 {
        for x in 0..5 {
            let want = if (1..=3).contains(&x) && (1..=3).contains(&y) { 28 } else { 0 };
            assert_eq!(px(&b, 5, x, y)[..3], [want; 3], "({x},{y})");
            assert_eq!(px(&b, 5, x, y)[3], 255);
        }
    }
    assert_eq!(box_blur(&img, 5, 5, 0).unwrap(), img);
}

#[test]
fn gaussian_kat() {
    for s in [64u16, 128, 256, 384, 1000] {
        let k = gaussian_kernel(s);
        let sum: u32 = k.iter().sum();
        println!("ADJUSTCORE gaussian sigma_q8={s}: taps={} sum={sum} kernel={k:?}", k.len());
        assert_eq!(sum, 1 << GAUSS_SHIFT);
        assert_eq!(k.len(), 2 * (3 * s as usize).div_ceil(256) + 1);
    }
    let flat = solid(7, 5, [90, 140, 33, 201]);
    assert_eq!(gaussian_blur(&flat, 7, 5, 300).unwrap(), flat);
    let mut img = solid(9, 9, [0, 0, 0, 255]);
    let c = ((4 * 9 + 4) * 4) as usize;
    img[c] = 255;
    let b = gaussian_blur(&img, 9, 9, 256).unwrap();
    let centre = px(&b, 9, 4, 4)[0];
    println!("ADJUSTCORE gaussian sigma 1.0 impulse: centre={centre} right={} diag={}", px(&b, 9, 5, 4)[0], px(&b, 9, 5, 5)[0]);
    for y in 0..9 {
        for x in 0..9 {
            if (x, y) != (4, 4) {
                assert!(px(&b, 9, x, y)[0] < centre, "({x},{y})");
            }
        }
    }
    assert_eq!(gaussian_blur(&img, 9, 9, 0).unwrap(), img);
}

#[test]
fn unsharp_kat() {
    // 8x1 step: 50 50 50 50 | 200 200 200 200.
    let img: Vec<u8> = (0..8).flat_map(|x| if x < 4 { [50, 50, 50, 255] } else { [200, 200, 200, 255] }).collect();
    let zero = Unsharp { amount_pct: 0, sigma_q8: 256, threshold: 0 };
    assert_eq!(unsharp(&img, 8, 1, zero).unwrap(), img);
    let s = unsharp(&img, 8, 1, Unsharp { amount_pct: 100, sigma_q8: 256, threshold: 0 }).unwrap();
    let dark = px(&s, 8, 3, 0)[0];
    let bright = px(&s, 8, 4, 0)[0];
    println!("ADJUSTCORE unsharp 100% sigma 1.0 on 50|200: dark side {dark}, bright side {bright}");
    assert!(dark < 50);
    assert!(bright > 200);
    // The far ends are untouched (the blur of a flat run is the run).
    assert_eq!(px(&s, 8, 0, 0)[0], 50);
    assert_eq!(px(&s, 8, 7, 0)[0], 200);
}

#[test]
fn refusals_and_alpha_kat() {
    let short = vec![0u8; 15]; // 2x2 wants 16
    let c = Curves { master: Curves::identity(), r: None, g: None, b: None };
    let hs = HueSat { hue_deg: 30, sat_pct: 20, light_pct: -10 };
    let us = Unsharp { amount_pct: 150, sigma_q8: 200, threshold: 3 };
    let bad = Err(Error::BadLength);
    assert_eq!(levels(&short, 2, 2, Levels::IDENTITY), bad);
    assert_eq!(curves(&short, 2, 2, &c), bad);
    assert_eq!(hue_sat(&short, 2, 2, hs), bad);
    assert_eq!(invert(&short, 2, 2), bad);
    assert_eq!(threshold(&short, 2, 2, 9), bad);
    assert_eq!(box_blur(&short, 2, 2, 1), bad);
    assert_eq!(gaussian_blur(&short, 2, 2, 256), bad);
    assert_eq!(unsharp(&short, 2, 2, us), bad);
    assert_eq!(invert(&[], u32::MAX, u32::MAX), bad);
    assert_eq!(Error::BadLength.word(), "bad-length");
    assert_eq!(Error::BadParam.word(), "bad-param");

    assert_eq!(levels(&[0; 4], 1, 1, Levels { in_lo: 9, in_hi: 9, ..Levels::IDENTITY }), Err(Error::BadParam));
    assert_eq!(levels(&[0; 4], 1, 1, Levels { gamma_q8: 0, ..Levels::IDENTITY }), Err(Error::BadParam));
    assert_eq!(hue_sat(&[0; 4], 1, 1, HueSat { hue_deg: 181, ..hs }), Err(Error::BadParam));
    assert_eq!(hue_sat(&[0; 4], 1, 1, HueSat { sat_pct: -101, ..hs }), Err(Error::BadParam));
    assert_eq!(hue_sat(&[0; 4], 1, 1, HueSat { light_pct: 101, ..hs }), Err(Error::BadParam));

    // Alpha kept byte for byte by everything but the blurs.
    let g = gradient4();
    let alpha = |v: &[u8]| v.chunks_exact(4).map(|p| p[3]).collect::<Vec<_>>();
    let want = alpha(&g);
    let lv = Levels { in_lo: 20, in_hi: 220, gamma_q8: 300, out_lo: 10, out_hi: 240 };
    let cv = Curves { master: Curves::from_points(&[(100, 50)]), r: None, g: Some(Curves::identity()), b: None };
    assert_eq!(alpha(&levels(&g, 4, 4, lv).unwrap()), want);
    assert_eq!(alpha(&curves(&g, 4, 4, &cv).unwrap()), want);
    assert_eq!(alpha(&hue_sat(&g, 4, 4, hs).unwrap()), want);
    assert_eq!(alpha(&invert(&g, 4, 4).unwrap()), want);
    assert_eq!(alpha(&threshold(&g, 4, 4, 100).unwrap()), want);
    assert_eq!(alpha(&unsharp(&g, 4, 4, us).unwrap()), want);

    // The blurs blur alpha: constant alpha stays constant, an alpha impulse spreads.
    let mut a = solid(5, 5, [10, 20, 30, 0]);
    a[(2 * 5 + 2) * 4 + 3] = 255;
    let bb = box_blur(&a, 5, 5, 1).unwrap();
    assert_eq!(px(&bb, 5, 1, 1)[3], 28);
    let gb = gaussian_blur(&a, 5, 5, 256).unwrap();
    assert!(px(&gb, 5, 2, 3)[3] > 0 && px(&gb, 5, 2, 2)[3] < 255);
    let flat = solid(5, 5, [10, 20, 30, 77]);
    assert!(alpha(&box_blur(&flat, 5, 5, 2).unwrap()).iter().all(|&v| v == 77));
    assert!(alpha(&gaussian_blur(&flat, 5, 5, 500).unwrap()).iter().all(|&v| v == 77));
}
