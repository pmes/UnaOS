// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDFICC (LEDGER SR358) known answers for icc_core. The Adobe RGB references are computed offline with numpy from the
//! matrices (AdobeRGB1998.icc's colorants as s15Fixed16, curv gamma 563/256, Bradford ICC-D50 -> D65, the sRGB D65
//! matrix and OETF): (200, 100, 50) -> (227.065, 100.067, 42.350); grey 128 -> 129.031.

use icc_core::{Applied, Curve, LutKind, Model, Pcs, Profile, Refusal, SRGB, SRGB_TRC, Space, Transform, apply_icc_to_srgb, bradford, kat, oetf};

fn near(a: u8, b: u8, tol: u8) -> bool {
    a.abs_diff(b) <= tol
}

#[test]
fn header_and_tags_of_a_v2_matrix_trc() {
    let icc = kat::adobe_rgb_v2();
    assert_eq!(u32::from_be_bytes(icc[128..132].try_into().unwrap()), 7, "wtpt + rgbXYZ + rgbTRC");
    let p = Profile::parse(&icc).unwrap();
    assert_eq!((p.version >> 24, &p.class, p.space, p.pcs, p.intent), (2, b"mntr", Space::Rgb, Pcs::Xyz, 0));
    let Model::MatrixTrc { m, trc } = &p.model else { panic!("matrix/TRC expected") };
    let want = [[0.60974, 0.20528, 0.14919], [0.31111, 0.62567, 0.06322], [0.01947, 0.06087, 0.74457]];
    for i in 0..3 {
        for c in 0..3 {
            assert!((m[i][c] - want[i][c]).abs() <= 1.0 / 65536.0, "m[{i}][{c}] {}", m[i][c]);
        }
    }
    assert_eq!(trc[0], Curve::Gamma(563.0 / 256.0));
    assert!(!p.is_srgb());
}

#[test]
fn lut16_cmyk_parses_and_maps_white_and_black() {
    let p = Profile::parse(&kat::lut16_cmyk_v2()).unwrap();
    assert_eq!((&p.class, p.space, p.pcs), (b"prtr", Space::Cmyk, Pcs::Lab));
    let Model::Lut(l) = &p.model else { panic!("A2B0 expected") };
    assert_eq!((l.kind, l.a.len(), l.b.len(), l.clut.as_ref().unwrap().grid.len()), (LutKind::Lut16, 4, 3, 4));
    let t = Transform::to_srgb(&p).unwrap();
    assert_eq!(t.channels(), 4);
    let to8 = |v: [f32; 3]| v.map(|c| (c * 255.0 + 0.5) as u8);
    let white = to8(t.eval(&[0.0, 0.0, 0.0, 0.0]));
    let black = to8(t.eval(&[0.0, 0.0, 0.0, 1.0]));
    let cyan = to8(t.eval(&[1.0, 0.0, 0.0, 0.0]));
    assert!(white.iter().all(|&c| c >= 253), "white {white:?}");
    assert!(black.iter().all(|&c| c <= 2), "black {black:?}");
    assert!(near(cyan[0], 0, 2) && near(cyan[1], 255, 2) && near(cyan[2], 255, 2), "cyan {cyan:?}");
    // A CMYK profile cannot run over RGBA: refused by name, pixels untouched.
    let mut px = [10u8, 20, 30, 40];
    assert_eq!(apply_icc_to_srgb(&kat::lut16_cmyk_v2(), &mut px), Err(Refusal::Unsupported("CMYK profile over RGB samples")));
    assert_eq!(px, [10, 20, 30, 40]);
}

#[test]
fn srgb_round_trip_is_identity() {
    // The built-in sRGB through the full pipeline (no short-circuit): PCS D50 and back out through Bradford.
    let t = Transform::to_srgb(&SRGB).unwrap();
    let mut grid = Vec::new();
    for r in (0..=255u32).step_by(17) {
        for g in (0..=255u32).step_by(17) {
            for b in (0..=255u32).step_by(17) {
                grid.extend([r as u8, g as u8, b as u8, 255]);
            }
        }
    }
    for v in 0..=255u8 {
        grid.extend([v, v, v, 255]);
    }
    let mut out = grid.clone();
    t.apply_rgba(&mut out);
    let worst = grid.iter().zip(&out).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
    assert!(worst <= 1, "worst {worst}");
}

#[test]
fn srgb_v2_short_circuits() {
    let icc = kat::srgb_v2();
    assert!(Profile::parse(&icc).unwrap().is_srgb());
    assert!(SRGB.is_srgb());
    let before: Vec<u8> = (0..=255u8).flat_map(|v| [v, 255 - v, v / 2, 200]).collect();
    let mut px = before.clone();
    assert_eq!(apply_icc_to_srgb(&icc, &mut px), Ok(Applied::Srgb));
    assert_eq!(px, before);
}

#[test]
fn bradford_d50_to_d65() {
    let m = bradford([0.9642, 1.0, 0.8249], [0.95047, 1.0, 1.08883]);
    let want = [[0.9555525, -0.0230521, 0.0632542], [-0.0282989, 1.0099331, 0.0210361], [0.0123181, -0.0205177, 1.3304287]];
    for i in 0..3 {
        for j in 0..3 {
            assert!((m[i][j] - want[i][j]).abs() < 1e-4, "[{i}][{j}] {} vs {}", m[i][j], want[i][j]);
        }
    }
}

#[test]
fn curv_16bit_table() {
    let t: Vec<u16> = (0..4096).map(|i| ((i as f64 / 4095.0).powi(2) * 65535.0).round() as u16).collect();
    let icc = kat::profile(2, b"mntr", b"GRAY", b"XYZ ", &[(*b"kTRC", kat::curv_table(&t))]);
    let Model::GrayTrc(c) = Profile::parse(&icc).unwrap().model else { panic!("kTRC expected") };
    assert!(matches!(&c, Curve::Table(v) if v.len() == 4096));
    for i in [0usize, 1, 100, 2047, 4000, 4095] {
        let x = i as f32 / 4095.0;
        assert!((c.eval(x) - t[i] as f32 / 65535.0).abs() < 1e-5, "node {i}");
    }
    let mid = (1000.5f32) / 4095.0;
    assert!((c.eval(mid) - (t[1000] as f32 + t[1001] as f32) / 2.0 / 65535.0).abs() < 1e-5, "midpoint");
}

#[test]
fn para_type_3() {
    let icc = kat::profile(
        4,
        b"mntr",
        b"GRAY",
        b"XYZ ",
        &[(*b"kTRC", kat::para(3, &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]))],
    );
    let Model::GrayTrc(c) = Profile::parse(&icc).unwrap().model else { panic!("kTRC expected") };
    assert!(matches!(c, Curve::Para(3, _)));
    for k in 0..=64 {
        let x = k as f32 / 64.0;
        assert!((c.eval(oetf(x)) - x).abs() < 1e-4, "x {x}: {}", c.eval(oetf(x)));
        assert!((c.eval(x) - SRGB_TRC.eval(x)).abs() < 1e-4);
    }
}

#[test]
fn adobe_rgb_pixel_to_srgb() {
    let mut px = [200u8, 100, 50, 255, 128, 128, 128, 7, 255, 255, 255, 255];
    assert_eq!(apply_icc_to_srgb(&kat::adobe_rgb_v2(), &mut px), Ok(Applied::Converted));
    assert!(near(px[0], 227, 1) && near(px[1], 100, 1) && near(px[2], 42, 1), "{:?}", &px[..3]);
    assert!(px[4..7].iter().all(|&c| near(c, 129, 1)) && px[7] == 7, "{:?}", &px[4..8]);
    assert!(px[8..11].iter().all(|&c| c >= 254), "{:?}", &px[8..11]);
    // The float path agrees with the cached 8-bit one.
    let t = Transform::from_icc(&kat::adobe_rgb_v2()).unwrap();
    let f = t.eval(&[200.0 / 255.0, 100.0 / 255.0, 50.0 / 255.0]).map(|c| c * 255.0);
    assert!((f[0] - 227.065).abs() < 0.5 && (f[1] - 100.067).abs() < 0.5 && (f[2] - 42.35).abs() < 0.5, "{f:?}");
    // A GRAY profile reads the red channel.
    let mut g = [128u8, 0, 0, 255];
    apply_icc_to_srgb(&kat::gray_v2(1.0), &mut g).unwrap();
    assert!(g[0] == g[1] && g[1] == g[2] && g[0] > 128, "{g:?}");
}

#[test]
fn refusals_by_name() {
    assert_eq!(Profile::parse(&kat::named_colour()), Err(Refusal::NamedColour));
    assert_eq!(Profile::parse(&kat::device_link()), Err(Refusal::DeviceLink));
    assert_eq!(Profile::parse(&kat::abstract_profile()), Err(Refusal::Abstract));
    assert_eq!(Profile::parse(&kat::mpet_rgb()), Err(Refusal::MultiProcessElements));
    let icc = kat::adobe_rgb_v2();
    assert!(matches!(Profile::parse(&icc[..100]), Err(Refusal::Malformed(_))));
    let mut short = icc.clone();
    short.truncate(icc.len() - 8); // the declared size now runs past the bytes
    assert_eq!(Profile::parse(&short), Err(Refusal::Malformed("profile size")));
    let mut v5 = icc.clone();
    v5[8] = 5;
    assert_eq!(Profile::parse(&v5), Err(Refusal::Unsupported("ICC v5 (iccMAX)")));
    assert_eq!(Refusal::NamedColour.to_string(), "named-colour profile");
}
