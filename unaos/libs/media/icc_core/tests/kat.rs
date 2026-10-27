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
// is held within 1 of the f32 run (`apply_rgba_f32`, SR358's) over a sweep of hand-built profiles; GRAY and CMYK LUT sources
// are untouched (SR380: an RGB LUT source runs in integers too, `int_lut_within_one_of_f32`); a `TransformCache` builds a profile once. Measured max |int - f32| over the sweep: 1 (five profiles, 18^3 grid + greys + ramps).
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

// KERNEL-ICC-INT-LUT + KERNEL-ICC-BUILD + KERNEL-PNG16-ICC (LEDGER SR380): an RGB lut8 / lut16 / mAB source runs in
// integers (the CLUT trilinear as `Clut::eval`, a Lab PCS through f⁻¹ = t³ in Q16) within 1 of the f32 run over
// `sweep()`; `apply_rgba16` of x*257 narrows to the 8-bit apply within 1 and is smoother on a 16-bit ramp; the f32
// tables are no longer built by `to_srgb` (`build_evals`).

fn s15b(v: f32) -> [u8; 4] {
    ((v as f64 * 65536.0).round() as i32).to_be_bytes()
}

fn pad4(t: &mut Vec<u8>) {
    while t.len() % 4 != 0 {
        t.push(0);
    }
}

/// The CLUT nodes of a `g0 x g1 x g2` grid (first input slowest), each node's three outputs from `f` at its 0..=1 coords.
fn nodes(g: [usize; 3], f: impl Fn([f32; 3]) -> [f32; 3]) -> Vec<[f32; 3]> {
    let mut v = Vec::new();
    for i in 0..g[0] {
        for j in 0..g[1] {
            for k in 0..g[2] {
                v.push(f([i as f32 / (g[0] - 1) as f32, j as f32 / (g[1] - 1) as f32, k as f32 / (g[2] - 1) as f32]));
            }
        }
    }
    v
}

/// Adobe RGB (1998) to XYZ D50 (0..=1 each), from its colorants and gamma 563/256.
fn adobe_xyz(rgb: [f32; 3]) -> [f32; 3] {
    let c = [[0.60974, 0.20528, 0.14919], [0.31111, 0.62567, 0.06322], [0.01947, 0.06087, 0.74457]];
    let l = rgb.map(|v| v.powf(563.0 / 256.0));
    core::array::from_fn(|i| c[i][0] * l[0] + c[i][1] * l[1] + c[i][2] * l[2])
}

/// lut16 (mft2) RGB to Lab (legacy encoding), 17-point grid holding the Lab of each node read as sRGB, 256-entry input
/// tables of x^0.8, 2-entry identity output tables.
fn lut16_rgb_lab() -> Vec<u8> {
    let mut t = b"mft2\0\0\0\0".to_vec();
    t.extend([3, 3, 17, 0]);
    for k in [1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
        t.extend(s15b(k));
    }
    t.extend(256u16.to_be_bytes());
    t.extend(2u16.to_be_bytes());
    for _ in 0..3 {
        for i in 0..256 {
            t.extend((((i as f64 / 255.0).powf(0.8) * 65535.0).round() as u16).to_be_bytes());
        }
    }
    for lab in nodes([17; 3], kat::srgb_to_lab) {
        for v in [lab[0] / 100.0 * 65280.0, (lab[1] + 128.0) * 256.0, (lab[2] + 128.0) * 256.0] {
            t.extend((v.round().clamp(0.0, 65535.0) as u16).to_be_bytes());
        }
    }
    for _ in 0..3 {
        t.extend([0u8, 0, 0xFF, 0xFF]);
    }
    kat::profile(2, b"mntr", b"RGB ", b"Lab ", &[(*b"wtpt", kat::xyz_tag([0.9642, 1.0, 0.8249])), (*b"A2B0", t)])
}

/// lut8 (mft1) RGB to XYZ, 9-point grid holding Adobe RGB's XYZ of each node, gamma-1.2 input tables, identity outputs.
fn lut8_rgb_xyz() -> Vec<u8> {
    let mut t = b"mft1\0\0\0\0".to_vec();
    t.extend([3, 3, 9, 0]);
    for k in [1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
        t.extend(s15b(k));
    }
    for _ in 0..3 {
        t.extend((0..256).map(|i| ((i as f64 / 255.0).powf(1.2) * 255.0).round() as u8));
    }
    for xyz in nodes([9; 3], adobe_xyz) {
        t.extend(xyz.map(|v| (v * 32768.0 / 257.0).round().clamp(0.0, 255.0) as u8));
    }
    for _ in 0..3 {
        t.extend(0..=255u8);
    }
    kat::profile(2, b"mntr", b"RGB ", b"XYZ ", &[(*b"wtpt", kat::xyz_tag([0.9642, 1.0, 0.8249])), (*b"A2B0", t)])
}

/// mAB (lutAToB) RGB to `pcs`: B curves, an optional matrix with offsets, M curves, an optional CLUT (grid, 16-bit
/// precision) and optional A curves, each section on a 4-byte boundary.
fn mab(pcs: &[u8; 4], b: [Vec<u8>; 3], mat: Option<[f32; 12]>, m: Option<[Vec<u8>; 3]>, clut: Option<([u8; 3], Vec<[f32; 3]>)>, a: Option<[Vec<u8>; 3]>) -> Vec<u8> {
    let mut t = b"mAB \0\0\0\0".to_vec();
    t.extend([3, 3, 0, 0]);
    t.extend([0u8; 20]);
    let mut off = [0u32; 5];
    let curves = |t: &mut Vec<u8>, cs: &[Vec<u8>; 3]| {
        for c in cs {
            t.extend(c);
            pad4(t);
        }
    };
    off[0] = t.len() as u32;
    curves(&mut t, &b);
    if let Some(mat) = mat {
        off[1] = t.len() as u32;
        for v in mat {
            t.extend(s15b(v));
        }
    }
    if let Some(m) = &m {
        off[2] = t.len() as u32;
        curves(&mut t, m);
    }
    if let Some((g, data)) = &clut {
        off[3] = t.len() as u32;
        let mut grid = [0u8; 16];
        grid[..3].copy_from_slice(g);
        t.extend(grid);
        t.extend([2, 0, 0, 0]);
        for n in data {
            t.extend(n.iter().flat_map(|v| ((v * 65535.0).round().clamp(0.0, 65535.0) as u16).to_be_bytes()));
        }
        pad4(&mut t);
    }
    if let Some(a) = &a {
        off[4] = t.len() as u32;
        curves(&mut t, a);
    }
    for (i, o) in off.iter().enumerate() {
        t[12 + 4 * i..16 + 4 * i].copy_from_slice(&o.to_be_bytes());
    }
    kat::profile(4, b"mntr", b"RGB ", pcs, &[(*b"wtpt", kat::xyz_tag([0.9642, 1.0, 0.8249])), (*b"A2B0", t)])
}

fn ident3() -> [Vec<u8>; 3] {
    [kat::curv_table(&[]), kat::curv_table(&[]), kat::curv_table(&[])]
}

/// mAB RGB to Lab (v4 encoding): A = para gamma 2.2, a NON-CUBE 5 x 7 x 9 CLUT of the nodes' Lab read as linear-light
/// sRGB, M = a curv table of x^0.9, a matrix with nonzero offsets, B = identity.
fn mab_rgb_lab() -> Vec<u8> {
    let g = [5u8, 7, 9];
    let data = nodes([5, 7, 9], |c| {
        let lab = kat::srgb_to_lab(c);
        [lab[0] / 100.0, (lab[1] + 128.0) / 255.0, (lab[2] + 128.0) / 255.0]
    });
    let mt: Vec<u16> = (0..64).map(|i| ((i as f64 / 63.0).powf(0.9) * 65535.0).round() as u16).collect();
    let para = || kat::para(0, &[2.2]);
    mab(
        b"Lab ",
        ident3(),
        Some([0.96, 0.01, 0.0, 0.0, 0.95, 0.02, 0.0, 0.03, 0.94, 0.02, 0.01, 0.015]),
        Some([kat::curv_table(&mt), kat::curv_table(&mt), kat::curv_table(&mt)]),
        Some((g, data)),
        Some([para(), para(), para()]),
    )
}

/// mAB RGB to XYZ without a CLUT: M = gamma 2.2, the matrix Adobe RGB's colorants in the XYZ PCS encoding, B = identity.
fn mab_rgb_xyz_no_clut() -> Vec<u8> {
    let c = [[0.60974f32, 0.20528, 0.14919], [0.31111, 0.62567, 0.06322], [0.01947, 0.06087, 0.74457]];
    let k = 32768.0 / 65535.0;
    let mut mat = [0f32; 12];
    for r in 0..3 {
        for j in 0..3 {
            mat[r * 3 + j] = c[r][j] * k;
        }
    }
    mab(b"XYZ ", ident3(), Some(mat), Some([kat::curv_gamma(2.2), kat::curv_gamma(2.2), kat::curv_gamma(2.2)]), None, None)
}

fn lut_profiles() -> [(&'static str, Vec<u8>); 4] {
    [("lut16-rgb-lab17", lut16_rgb_lab()), ("lut8-rgb-xyz9", lut8_rgb_xyz()), ("mab-rgb-lab-5x7x9", mab_rgb_lab()), ("mab-rgb-xyz-noclut", mab_rgb_xyz_no_clut())]
}

#[test]
fn int_lut_within_one_of_f32() {
    let mut worst = 0u8;
    for (name, icc) in &lut_profiles() {
        let p = Profile::parse(icc).unwrap();
        let Model::Lut(l) = &p.model else { panic!("{name}: A2B expected") };
        assert_eq!(p.space, Space::Rgb, "{name}");
        if name.starts_with("mab-rgb-lab") {
            assert_eq!(l.clut.as_ref().unwrap().grid, vec![5, 7, 9], "{name}: the non-cube grid");
        }
        let t = Transform::to_srgb(&p).unwrap();
        assert!(t.build_evals() > 0, "{name}: the integer run was built");
        let src = sweep();
        let (mut a, mut b) = (src.clone(), src.clone());
        t.apply_rgba(&mut a);
        t.apply_rgba_f32(&mut b);
        assert_ne!(a, src, "{name}: converted");
        let w = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
        eprintln!("KERNEL-ICC-INT-LUT {name}: max |int - f32| = {w}");
        assert!(w <= 1, "{name}: worst {w}");
        assert!(a.iter().zip(&src).skip(3).step_by(4).all(|(x, y)| x == y), "{name}: alpha moved");
        worst = worst.max(w);
    }
    eprintln!("KERNEL-ICC-INT-LUT max |int - f32| = {worst}");
}

fn matrix_profiles() -> Vec<(&'static str, Vec<u8>)> {
    let adobe = rows([[0.60974, 0.20528, 0.14919], [0.31111, 0.62567, 0.06322], [0.01947, 0.06087, 0.74457]]);
    vec![("adobe-rgb-v2", kat::adobe_rgb_v2()), ("adobe-gamma-1.8", kat::matrix_trc_v2(adobe, kat::curv_gamma(1.8)))]
}

fn narrow(v: u16) -> u8 {
    ((v as u32 * 255 + 32767) / 65535) as u8
}

#[test]
fn apply16_narrows_to_apply8() {
    let mut all = matrix_profiles();
    all.extend(lut_profiles());
    all.push(("gray-1.8", kat::gray_v2(1.8)));
    for (name, icc) in &all {
        let t = Transform::from_icc(icc).unwrap();
        let src = sweep();
        let mut a = src.clone();
        t.apply_rgba(&mut a);
        let mut w: Vec<u16> = src.iter().map(|&x| x as u16 * 257).collect();
        t.apply_rgba16(&mut w);
        let worst = a.iter().zip(&w).enumerate().filter(|(i, _)| i % 4 != 3).map(|(_, (x, y))| x.abs_diff(narrow(*y))).max().unwrap();
        assert!(worst <= 1, "{name}: 16-bit narrowed vs 8-bit worst {worst}");
        assert!(w.iter().zip(&src).skip(3).step_by(4).all(|(x, y)| *x == *y as u16 * 257), "{name}: alpha moved");
    }
}

#[test]
fn apply16_ramp_is_smoother() {
    let t = Transform::from_icc(&kat::adobe_rgb_v2()).unwrap();
    let ramp: Vec<u16> = (0..=65535u32).step_by(16).chain([65535]).map(|x| x as u16).collect();
    let mut w: Vec<u16> = ramp.iter().flat_map(|&x| [x, x, x, 65535]).collect();
    t.apply_rgba16(&mut w);
    let mut b: Vec<u8> = ramp.iter().flat_map(|&x| [(x >> 8) as u8, (x >> 8) as u8, (x >> 8) as u8, 255]).collect();
    t.apply_rgba(&mut b);
    let mut worst = 0u32;
    for c in 0..3 {
        let o16: Vec<u16> = w.chunks_exact(4).map(|p| p[c]).collect();
        let o8: Vec<u8> = b.chunks_exact(4).map(|p| p[c]).collect();
        assert!(o16.windows(2).all(|p| p[0] <= p[1]), "channel {c}: monotone");
        let (mut d16, mut d8) = (o16.clone(), o8.clone());
        d16.dedup();
        d8.dedup();
        assert!(d16.len() > 4 * d8.len(), "channel {c}: {} distinct at 16 bits vs {} at 8", d16.len(), d8.len());
        for (x, &o) in ramp.iter().zip(&o16) {
            let v = *x as f32 / 65535.0;
            let want = (t.eval(&[v, v, v])[c] * 65535.0).round() as i64;
            worst = worst.max((o as i64 - want).unsigned_abs() as u32);
        }
    }
    eprintln!("KERNEL-PNG16-ICC ramp max |apply16 - eval*65535| = {worst}");
    assert!(worst <= 32, "worst {worst}"); // measured 17 (of 65535)
}

#[test]
fn build_skips_f32_tables() {
    // SR376 built 768 (f32 lin) + 4097 (f32 OETF) + 768 (u16 dec) + 255 (thresholds) = 5888 for a one-TRC source.
    let adobe = Transform::from_icc(&kat::adobe_rgb_v2()).unwrap();
    assert_eq!(adobe.build_evals(), 4097 + 255);
    assert!(adobe.build_evals() < 5888);
    let tags = |trc: [Vec<u8>; 3]| {
        let [r, g, b] = trc;
        kat::profile(
            2,
            b"mntr",
            b"RGB ",
            b"XYZ ",
            &[
                (*b"wtpt", kat::xyz_tag([0.9642, 1.0, 0.8249])),
                (*b"rXYZ", kat::xyz_tag([0.60974, 0.31111, 0.01947])),
                (*b"gXYZ", kat::xyz_tag([0.20528, 0.62567, 0.06087])),
                (*b"bXYZ", kat::xyz_tag([0.14919, 0.06322, 0.74457])),
                (*b"rTRC", r),
                (*b"gTRC", g),
                (*b"bTRC", b),
            ],
        )
    };
    let three = Transform::from_icc(&tags([kat::curv_gamma(1.8), kat::curv_gamma(2.2), kat::para(0, &[2.4])])).unwrap();
    assert_eq!(three.build_evals(), 3 * 4097 + 255, "one decode table per distinct curve");
    assert_eq!(Transform::from_icc(&kat::gray_v2(1.8)).unwrap().build_evals(), 256 * 4);
    // lut16: one shared input curve, the identity m, one shared output curve.
    assert_eq!(Transform::from_icc(&lut16_rgb_lab()).unwrap().build_evals(), 3 * 4097 + 255);
    // The f32 run still answers as SR376's did (its tables now built per call).
    let (mut a, mut b) = (sweep(), sweep());
    adobe.apply_rgba_f32(&mut a);
    three.apply_rgba_f32(&mut b);
    assert_ne!(a, sweep());
}
