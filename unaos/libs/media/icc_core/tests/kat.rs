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

#[test]
fn desc_v2_and_v4() {
    // METAICCONE (SR367): `profile::description` reads the v2 `desc` ASCII string and the v4 `mluc` first record, from
    // the tag table alone — a profile `Profile::parse` refuses still names itself (Facet's info label).
    use icc_core::profile::description;
    let wtpt = (*b"wtpt", kat::xyz_tag([0.9642, 1.0, 0.8249]));
    let v2 = kat::profile(2, b"mntr", b"GRAY", b"XYZ ", &[wtpt.clone(), (*b"kTRC", kat::curv_gamma(2.2)), (*b"desc", kat::desc_v2("Gray 2.2"))]);
    assert!(Profile::parse(&v2).is_ok());
    assert_eq!(description(&v2).as_deref(), Some("Gray 2.2"));
    let v4 = kat::profile(4, b"mntr", b"GRAY", b"XYZ ", &[(*b"desc", kat::mluc("Display P3 \u{2014} v4")), wtpt]);
    assert_eq!(description(&v4).as_deref(), Some("Display P3 \u{2014} v4"));
    // No `desc` tag, a truncated tag table, and the empty input read as None.
    assert_eq!(description(&kat::adobe_rgb_v2()), None);
    assert_eq!(description(&v2[..140]), None);
    assert_eq!(description(&[]), None);
    // A refused class still yields its description.
    let named = kat::profile(2, b"nmcl", b"RGB ", b"Lab ", &[(*b"desc", kat::desc_v2("Spot"))]);
    assert_eq!(Profile::parse(&named), Err(Refusal::NamedColour));
    assert_eq!(description(&named).as_deref(), Some("Spot"));
}

// KERNEL-DISPLAY-ICC (LEDGER SR372) known answers for `icc_core::display`: sRGB to a matrix/TRC panel. The references are
// hand-derived offline (python, f64): each side's matrix from its EDID-quantised xy primaries + white, Bradford-adapted
// to the ICC D50, `inv(M_panel) . M_sRGB`, the sRGB EOTF in, the panel's pure gamma out. A DCI-P3 (D65) panel at gamma
// 2.20 takes sRGB red (255, 0, 0) to (233.519, 53.904, 40.126), green to (116.278, 251.014, 77.362), blue to
// (0, 12.511, 244.347), (200, 100, 50) to (185.832, 105.024, 64.232) and grey 128 to 127.026; an sRGB-primaries panel
// declaring a pure gamma 2.20 without the sRGB bit takes grey 10 to 18.285.
use icc_core::display::{self, colorants};
use icc_core::{DisplayTransform, edid_colour};

const P3_Q10: [[u16; 2]; 4] = [[696, 328], [271, 707], [154, 61], [320, 337]];
const SRGB_Q10: [[u16; 2]; 4] = [[655, 338], [307, 614], [154, 61], [320, 337]];

fn q10(c: [[u16; 2]; 4]) -> [[f32; 2]; 4] {
    c.map(|p| p.map(|v| v as f32 / 1024.0))
}

fn near3(a: [u8; 3], b: [u8; 3]) -> bool {
    (0..3).all(|k| near(a[k], b[k], 1))
}

#[test]
fn display_edid_colour_bytes() {
    let b = display::kat::edid(P3_Q10, 120, false);
    assert_eq!(b.iter().fold(0u8, |a, x| a.wrapping_add(*x)), 0, "checksum");
    // 0x17 gamma 2.20, 0x18 features, 0x19 / 0x1A the low bits (Rx Ry Gx Gy / Bx By Wx Wy), 0x1B.. the high eight.
    assert_eq!(&b[0x17..0x23], &[120, 0x0A, 0x0F, 0x91, 0xAE, 0x52, 0x43, 0xB0, 0x26, 0x0F, 0x50, 0x54]);
    let e = edid_colour(&b).unwrap();
    assert_eq!(e.chroma, q10(P3_Q10));
    assert_eq!(e.gamma, 2.2);
    assert!(!e.srgb);
    assert!(edid_colour(&display::kat::edid(SRGB_Q10, 120, true)).unwrap().srgb);
    let mut x = b;
    x[0x17] = 0xFF;
    assert_eq!(edid_colour(&x), Err(Refusal::Unsupported("EDID gamma in an extension block")));
    assert_eq!(edid_colour(&b[..127]), Err(Refusal::Malformed("EDID base block")));
    let mut flat = P3_Q10;
    flat[3] = [320, 0];
    let e = edid_colour(&display::kat::edid(flat, 120, false)).unwrap();
    assert_eq!(DisplayTransform::from_edid_colour(&e).err(), Some(Refusal::Unsupported("EDID chromaticity degenerate")));
}

#[test]
fn display_srgb_edid_is_identity() {
    let e = edid_colour(&display::kat::edid(SRGB_Q10, 120, true)).unwrap();
    let x = DisplayTransform::from_edid_colour(&e).unwrap();
    assert!(x.is_identity());
    let mut px = [200, 100, 50, 7, 10, 10, 10, 255];
    let before = px;
    x.apply(&mut px);
    assert_eq!(px, before);
    assert!(DisplayTransform::from_profile(&SRGB).unwrap().is_identity());
    // The same primaries declaring a pure gamma 2.20 without the sRGB bit: the EDID gamma is taken literally.
    let e = edid_colour(&display::kat::edid(SRGB_Q10, 120, false)).unwrap();
    let x = DisplayTransform::from_edid_colour(&e).unwrap();
    assert!(!x.is_identity());
    assert!(near3(x.rgb([10, 10, 10]), [18, 18, 18]), "{:?}", x.rgb([10, 10, 10]));
    assert!(near3(x.rgb([128, 128, 128]), [127, 127, 127]));
    assert!(near3(x.rgb([255, 0, 0]), [255, 0, 0]));
}

#[test]
fn display_p3_known_values() {
    let e = edid_colour(&display::kat::edid(P3_Q10, 120, false)).unwrap();
    let x = DisplayTransform::from_edid_colour(&e).unwrap();
    assert!(!x.is_identity());
    for (i, o) in [
        ([255, 0, 0], [234, 54, 40]),
        ([0, 255, 0], [116, 251, 77]),
        ([0, 0, 255], [0, 13, 244]),
        ([255, 255, 255], [255, 255, 255]),
        ([128, 128, 128], [127, 127, 127]),
        ([200, 100, 50], [186, 105, 64]),
        ([0, 0, 0], [0, 0, 0]),
    ] {
        assert!(near3(x.rgb(i), o), "{i:?} -> {:?}, want {o:?}", x.rgb(i));
    }
    let mut px = [200, 100, 50, 33];
    x.apply(&mut px);
    assert_eq!((px[..3] == x.rgb([200, 100, 50])[..], px[3]), (true, 33), "apply = rgb, alpha untouched");
}

#[test]
fn display_from_profile_equals_from_edid() {
    let ch = q10(P3_Q10);
    let g = 563.0 / 256.0;
    let m = colorants(&ch).unwrap();
    let rows = core::array::from_fn(|c| [m[0][c], m[1][c], m[2][c]]);
    let p = Profile::parse(&kat::matrix_trc_v2(rows, kat::curv_gamma(g))).unwrap();
    let a = DisplayTransform::from_profile(&p).unwrap();
    let b = DisplayTransform::from_edid(ch, g).unwrap();
    let steps: [u8; 17] = core::array::from_fn(|i| (i * 16).min(255) as u8);
    for r in steps {
        for gg in steps {
            for bb in steps {
                assert!(near3(a.rgb([r, gg, bb]), b.rgb([r, gg, bb])), "({r}, {gg}, {bb})");
            }
        }
    }
}

#[test]
fn display_refusals() {
    let gray = Profile::parse(&kat::gray_v2(2.2)).unwrap();
    assert_eq!(DisplayTransform::from_profile(&gray).err(), Some(Refusal::Unsupported("GRAY display profile")));
    let lut = Profile::parse(&kat::lut16_cmyk_v2()).unwrap();
    assert_eq!(DisplayTransform::from_profile(&lut).err(), Some(Refusal::Unsupported("LUT-based display profile")));
    assert_eq!(DisplayTransform::from_edid(q10(P3_Q10), 0.0).err(), Some(Refusal::Malformed("display gamma")));
}

#[test]
fn display_bgra_is_rgba_swapped() {
    let x = DisplayTransform::from_edid(q10(P3_Q10), 2.2).unwrap();
    let mut rgba = [200, 100, 50, 9, 0, 255, 0, 1, 17, 34, 51, 0];
    let mut bgra = [50, 100, 200, 9, 0, 255, 0, 1, 51, 34, 17, 0];
    x.apply(&mut rgba);
    x.apply_bgra(&mut bgra);
    for p in 0..3 {
        assert_eq!([bgra[4 * p + 2], bgra[4 * p + 1], bgra[4 * p], bgra[4 * p + 3]], rgba[4 * p..4 * p + 4]);
    }
}

// KERNEL-ICC-INT + KERNEL-ICC-CACHE (LEDGER SR376): the matrix/TRC source run in integers (DisplayTransform's Q14 shape)
// is held within 1 of the f32 run (`apply_rgba_f32`, SR358's) over a sweep of hand-built profiles; GRAY and LUT sources
// are untouched; a `TransformCache` builds a profile once. Measured max |int - f32| over the sweep: 1 (five profiles, 18^3 grid + greys + ramps).
fn sweep() -> Vec<u8> {
    let mut v = Vec::new();
    for r in (0..=255u32).step_by(16).chain([255]) {
        for g in (0..=255u32).step_by(16).chain([255]) {
            for b in (0..=255u32).step_by(16).chain([255]) {
                v.extend([r as u8, g as u8, b as u8, 77]);
            }
        }
    }
    for k in 0..=255u8 {
        v.extend([k, k, k, 255, k, 0, 0, 1, 0, k, 0, 2, 0, 0, k, 3]);
    }
    v
}

fn rows(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    core::array::from_fn(|c| [m[0][c], m[1][c], m[2][c]])
}

#[test]
fn int_run_within_one_of_f32() {
    let adobe = rows([[0.60974, 0.20528, 0.14919], [0.31111, 0.62567, 0.06322], [0.01947, 0.06087, 0.74457]]);
    let srgb_para = || kat::para(3, &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]);
    let table: Vec<u16> = (0..4096).map(|i| ((i as f64 / 4095.0).powf(2.2) * 65535.0).round() as u16).collect();
    let profiles = [
        ("adobe-rgb-v2", kat::adobe_rgb_v2()),
        ("srgb-primaries-para", kat::matrix_trc_v2(rows(colorants(&display::SRGB_XY).unwrap()), srgb_para())),
        ("p3-para", kat::matrix_trc_v2(rows(colorants(&q10(P3_Q10)).unwrap()), srgb_para())),
        ("adobe-gamma-1.8", kat::matrix_trc_v2(adobe, kat::curv_gamma(1.8))),
        ("adobe-curv-table", kat::matrix_trc_v2(adobe, kat::curv_table(&table))),
    ];
    let mut worst = 0u8;
    for (name, icc) in &profiles {
        let t = Transform::from_icc(icc).unwrap();
        let src = sweep();
        let (mut a, mut b) = (src.clone(), src.clone());
        t.apply_rgba(&mut a);
        t.apply_rgba_f32(&mut b);
        let w = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
        assert!(w <= 1, "{name}: worst {w}");
        assert!(a.iter().zip(&src).skip(3).step_by(4).all(|(x, y)| x == y), "{name}: alpha moved");
        worst = worst.max(w);
    }
    eprintln!("KERNEL-ICC-INT max |int - f32| = {worst}");
}

#[test]
fn int_run_only_on_matrix_trc() {
    for icc in [kat::gray_v2(1.8), kat::lut16_cmyk_v2()] {
        let t = Transform::from_icc(&icc).unwrap();
        let src = sweep();
        let (mut a, mut b) = (src.clone(), src.clone());
        t.apply_rgba(&mut a);
        t.apply_rgba_f32(&mut b);
        assert_eq!(a, b);
    }
}

#[test]
fn transform_cache_builds_once() {
    use icc_core::{TRANSFORM_CACHE_SLOTS, TransformCache};
    let mut c = TransformCache::new();
    let icc = kat::adobe_rgb_v2();
    let (mut a, mut b) = (sweep(), sweep());
    c.for_rgba(&icc).unwrap().unwrap().apply_rgba(&mut a);
    c.for_rgba(&icc).unwrap().unwrap().apply_rgba(&mut b);
    assert_eq!((c.builds(), &a), (1, &b), "the same profile twice: one build");
    // An sRGB answer and a refusal are kept too.
    assert!(c.for_rgba(&kat::srgb_v2()).unwrap().is_none());
    assert!(c.for_rgba(&kat::srgb_v2()).unwrap().is_none());
    assert_eq!(c.for_rgba(&kat::lut16_cmyk_v2()).err(), Some(Refusal::Unsupported("CMYK profile over RGB samples")));
    assert_eq!(c.for_rgba(&kat::lut16_cmyk_v2()).err(), Some(Refusal::Unsupported("CMYK profile over RGB samples")));
    assert_eq!(c.builds(), 3);
    // Least recently used out: adobe, srgb, cmyk, then two more push adobe out (four slots).
    assert_eq!(TRANSFORM_CACHE_SLOTS, 4);
    c.for_rgba(&kat::gray_v2(1.8)).unwrap();
    c.for_rgba(&kat::gray_v2(2.2)).unwrap();
    assert_eq!(c.builds(), 5);
    let _ = c.for_rgba(&kat::lut16_cmyk_v2());
    assert_eq!(c.builds(), 5, "cmyk still kept");
    c.for_rgba(&icc).unwrap();
    assert_eq!(c.builds(), 6, "adobe was evicted and is rebuilt");
    assert_ne!(icc_core::fnv1a64(b"a"), icc_core::fnv1a64(b"b"));
}
