// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! FVAR (LEDGER SR150, rmbp-queue B640's cloud half) — the variable-font delta math proved on a SYNTHETIC font
//! assembled here, byte by byte, from the OpenType tables ("fvar", "gvar", "HVAR", "Item Variation Store",
//! "glyf", "loca"): ONE glyph (a 500x700 square, advance 600), ONE axis (`wght` 100 / 400 / 900), ONE tuple
//! (peak +1.0). Every expected number below is worked from the spec, not read back from the code. Plus the
//! real Inter variable face, skipped cleanly when `fonts/inter/InterVariable.ttf` is absent. R78: host only.

mod common;

use font_core::ui::{Role, Style};
use font_core::{Font, PathCmd, VarFont};

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}
fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}
fn fixed(v: &mut Vec<u8>, x: f32) {
    be32(v, (x * 65536.0) as i32 as u32);
}

/// An sfnt from (tag, bytes) tables (directory in the given order; `Font::table` scans it linearly).
fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    be32(&mut out, 0x0001_0000);
    be16(&mut out, tables.len() as u16);
    be16(&mut out, 0);
    be16(&mut out, 0);
    be16(&mut out, 0);
    let mut off = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in tables {
        out.extend_from_slice(*tag);
        be32(&mut out, 0);
        be32(&mut out, off as u32);
        be32(&mut out, data.len() as u32);
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        off = 12 + 16 * tables.len() + body.len();
    }
    out.extend_from_slice(&body);
    out
}

/// `fvar`: one axis `wght` 100/400/900 and one named instance (subfamily 257, wght 700, PostScript name 258).
fn fvar() -> Vec<u8> {
    let mut v = Vec::new();
    for x in [1u16, 0, 16, 2, 1, 20, 1, 10] {
        be16(&mut v, x); // major, minor, axesArrayOffset, reserved, axisCount, axisSize, instanceCount, instanceSize
    }
    v.extend_from_slice(b"wght");
    fixed(&mut v, 100.0);
    fixed(&mut v, 400.0);
    fixed(&mut v, 900.0);
    be16(&mut v, 0);
    be16(&mut v, 256);
    be16(&mut v, 257); // InstanceRecord: subfamilyNameID, flags, coordinates[1], postScriptNameID (size 4*1+6 = 10)
    be16(&mut v, 0);
    fixed(&mut v, 700.0);
    be16(&mut v, 258);
    v
}

/// `gvar` around one glyph's GlyphVariationData; one shared tuple (peak +1.0).
fn gvar(mut glyph: Vec<u8>) -> Vec<u8> {
    if glyph.len() % 2 != 0 {
        glyph.push(0); // short offsets count 2-byte units
    }
    let mut v = Vec::new();
    be16(&mut v, 1); // majorVersion
    be16(&mut v, 0);
    be16(&mut v, 1); // axisCount
    be16(&mut v, 1); // sharedTupleCount
    be32(&mut v, 24); // sharedTuplesOffset
    be16(&mut v, 1); // glyphCount
    be16(&mut v, 0); // flags: short offsets
    be32(&mut v, 26); // glyphVariationDataArrayOffset
    be16(&mut v, 0);
    be16(&mut v, (glyph.len() / 2) as u16);
    be16(&mut v, 0x4000); // shared tuple 0: wght peak +1.0
    v.extend_from_slice(&glyph);
    v
}

/// Glyph variation data (a): SHARED point numbers = all points (the 0 byte), one tuple using SHARED tuple 0;
/// x deltas [0,100,100,0 | phantoms 0,100,0,0], y deltas [0,0,50,50 | 0,0,0,0] (packed as byte runs).
fn glyph_shared_all() -> Vec<u8> {
    let mut v = Vec::new();
    be16(&mut v, 0x8000 | 1); // SHARED_POINT_NUMBERS | tupleVariationCount 1
    be16(&mut v, 8); // dataOffset (4 header + one 4-byte TupleVariationHeader)
    be16(&mut v, 18); // variationDataSize
    be16(&mut v, 0); // tupleIndex: shared tuple 0
    v.push(0); // shared point numbers: 0 = every point
    v.push(0x07); // 8 byte deltas
    v.extend_from_slice(&[0, 100, 100, 0, 0, 100, 0, 0]);
    v.push(0x07);
    v.extend_from_slice(&[0, 0, 50, 50, 0, 0, 0, 0]);
    v
}

/// Glyph variation data (b): one tuple with an EMBEDDED peak (+1.0) and PRIVATE point numbers {1, 2}; dx 100,100,
/// dy 0,50. Points 0 and 3 and the phantoms are untouched (IUP infers the outline's; phantoms stay 0).
fn glyph_private_iup() -> Vec<u8> {
    let mut v = Vec::new();
    be16(&mut v, 1);
    be16(&mut v, 10); // dataOffset: 4 + (4 + 2 peak)
    be16(&mut v, 10); // variationDataSize: points 4 + x 3 + y 3
    be16(&mut v, 0x8000 | 0x2000); // EMBEDDED_PEAK_TUPLE | PRIVATE_POINT_NUMBERS
    be16(&mut v, 0x4000);
    v.extend_from_slice(&[2, 0x01, 1, 1]); // count 2; run of 2 byte deltas: 1, +1 = 2
    v.extend_from_slice(&[0x01, 100, 100]);
    v.extend_from_slice(&[0x01, 0, 50]);
    v
}

/// `HVAR`: implicit advance map, one region (0, +1, +1), glyph 0's advance delta +40.
fn hvar() -> Vec<u8> {
    let mut v = Vec::new();
    be16(&mut v, 1);
    be16(&mut v, 0);
    be32(&mut v, 20); // itemVariationStoreOffset
    be32(&mut v, 0); // advanceWidthMappingOffset: none (implicit)
    be32(&mut v, 0);
    be32(&mut v, 0);
    // ItemVariationStore at 20
    be16(&mut v, 1); // format
    be32(&mut v, 12); // variationRegionListOffset
    be16(&mut v, 1); // itemVariationDataCount
    be32(&mut v, 22); // itemVariationDataOffsets[0]
    be16(&mut v, 1); // regionList: axisCount
    be16(&mut v, 1); // regionCount
    for x in [0u16, 0x4000, 0x4000] {
        be16(&mut v, x); // start, peak, end
    }
    be16(&mut v, 1); // itemCount
    be16(&mut v, 0); // wordDeltaCount
    be16(&mut v, 1); // regionIndexCount
    be16(&mut v, 0); // regionIndexes[0]
    v.push(40); // deltaSets[0][0] (int8)
    v
}

/// The synthetic face: head/maxp/hhea/hmtx/loca/glyf (one square glyph) + fvar + gvar (+ HVAR).
fn synth(glyph_var: Vec<u8>, with_hvar: bool) -> Font<'static> {
    let mut head = vec![0u8; 54];
    head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    head[40..42].copy_from_slice(&500u16.to_be_bytes());
    head[42..44].copy_from_slice(&700u16.to_be_bytes()); // bbox xMax 500, yMax 700; indexToLocFormat (50) stays 0: short loca
    let mut maxp = Vec::new();
    be32(&mut maxp, 0x0000_5000);
    be16(&mut maxp, 1);
    let mut hhea = vec![0u8; 36];
    hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    hhea[4..6].copy_from_slice(&800i16.to_be_bytes());
    hhea[6..8].copy_from_slice(&(-200i16).to_be_bytes());
    hhea[34..36].copy_from_slice(&1u16.to_be_bytes());
    let mut hmtx = Vec::new();
    be16(&mut hmtx, 600);
    be16(&mut hmtx, 0);
    // glyf: 1 contour, bbox, endPts [3], no instructions, 4 on-curve flags with word coordinates.
    let mut glyf = Vec::new();
    for x in [1i16, 0, 0, 500, 700, 3, 0] {
        be16(&mut glyf, x as u16);
    }
    glyf.extend_from_slice(&[0x01; 4]);
    for x in [0i16, 500, 0, -500, 0, 0, 700, 0] {
        be16(&mut glyf, x as u16);
    }
    let mut loca = Vec::new();
    be16(&mut loca, 0);
    be16(&mut loca, (glyf.len() / 2) as u16);
    let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", head),
        (b"maxp", maxp),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
        (b"loca", loca),
        (b"glyf", glyf),
        (b"fvar", fvar()),
        (b"gvar", gvar(glyph_var)),
    ];
    if with_hvar {
        tables.push((b"HVAR", hvar()));
    }
    let bytes = Box::leak(sfnt(&tables).into_boxed_slice());
    Font::parse(bytes).expect("the synthetic variable font parses")
}

/// The square's four corners, in contour order, from the recorded path (MoveTo + 3 LineTo, then the closing line).
fn corners(f: &Font) -> Vec<(f32, f32)> {
    let p = f.glyph_path(0).expect("glyph 0 has an outline");
    p.cmds
        .iter()
        .filter_map(|c| match *c {
            PathCmd::MoveTo(x, y) | PathCmd::LineTo(x, y) => Some((x, y)),
            _ => None,
        })
        .take(4)
        .collect()
}

fn close(a: &[(f32, f32)], b: &[(f32, f32)]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(p, q)| (p.0 - q.0).abs() < 1e-3 && (p.1 - q.1).abs() < 1e-3)
}

/// wght 700 normalizes to (700-400)/(900-400) = 0.6, rounded to the F2Dot14 grid: 9830/16384.
const N700: f32 = 9830.0 / 16384.0;
const SQUARE: [(f32, f32); 4] = [(0.0, 0.0), (500.0, 0.0), (500.0, 700.0), (0.0, 700.0)];

#[test]
fn shared_tuple_all_points_delta_math() {
    let f = synth(glyph_shared_all(), false);
    assert!(f.is_variable() && f.gvar.is_some() && f.hvar.is_none());
    // Default instance: the static outline and advance.
    assert!(close(&corners(&f), &SQUARE));
    assert_eq!(f.advance(0), 600);
    // wght 900 (n = +1): the full deltas; the advance phantom moves +100 (no HVAR: gvar's phantom points).
    let max = VarFont::instance(&f, &[900.0]);
    assert_eq!(max.coords(), &[1.0]);
    assert!(close(&corners(&max), &[(0.0, 0.0), (600.0, 0.0), (600.0, 750.0), (0.0, 750.0)]), "{:?}", corners(&max));
    assert_eq!(max.advance(0), 700);
    // wght 700 (n = 9830/16384): every delta scales by n; advance round(600 + 100 n) = 660.
    let mid = f.instance(&[700.0]);
    assert_eq!(mid.coords(), &[N700]);
    let want = [(0.0, 0.0), (500.0 + 100.0 * N700, 0.0), (500.0 + 100.0 * N700, 700.0 + 50.0 * N700), (0.0, 700.0 + 50.0 * N700)];
    assert!(close(&corners(&mid), &want), "{:?}", corners(&mid));
    assert!((mid.advance_delta(0) - 100.0 * N700).abs() < 1e-3);
    assert_eq!(mid.advance(0), 660);
    // wght 100 (n = -1): outside the tuple's region (peak +1 → scalar 0): the default outline.
    let min = f.instance(&[100.0]);
    assert_eq!(min.coords(), &[-1.0]);
    assert!(close(&corners(&min), &SQUARE));
    assert_eq!(min.advance(0), 600);
    // Out-of-range user values clamp to the axis; normalized coordinates go straight in.
    assert_eq!(f.instance(&[5000.0]).coords(), &[1.0]);
    assert_eq!(f.with_normalized(&[0.25]).advance(0), 625);
    // instance() is the same position as set_variation().
    assert_eq!(f.instance(&[650.0]).coords(), f.with_variations(&[(*b"wght", 650.0)]).coords());
}

#[test]
fn private_points_infer_untouched() {
    let f = synth(glyph_private_iup(), false);
    let max = f.instance(&[900.0]);
    // Points 1, 2 move (+100, 0) and (+100, +50). IUP: point 3 lies between touched 2 and 1 (wrapping): in x both
    // references are 500 with equal deltas 100 → +100; in y its 700 is at/above the hi reference (700, +50) → +50.
    // Point 0: x → +100 likewise; y 0 at/below the lo reference (0, 0) → 0. Phantoms untouched → advance 600.
    assert!(close(&corners(&max), &[(100.0, 0.0), (600.0, 0.0), (600.0, 750.0), (100.0, 750.0)]), "{:?}", corners(&max));
    assert_eq!(max.advance(0), 600);
    let mid = f.instance(&[700.0]);
    let (dx, dy) = (100.0 * N700, 50.0 * N700);
    assert!(close(&corners(&mid), &[(dx, 0.0), (500.0 + dx, 0.0), (500.0 + dx, 700.0 + dy), (dx, 700.0 + dy)]), "{:?}", corners(&mid));
}

#[test]
fn hvar_advance_wins_over_phantoms() {
    let f = synth(glyph_shared_all(), true);
    assert!(f.hvar.is_some());
    let mid = f.instance(&[700.0]);
    // HVAR region (0, 1, 1) at n: scalar n; delta 40 n — the phantom's 100 n is NOT used when HVAR is present.
    assert!((mid.advance_delta(0) - 40.0 * N700).abs() < 1e-3);
    assert_eq!(mid.advance(0), 624);
    assert_eq!(f.instance(&[900.0]).advance(0), 640);
    // The outline is still gvar's.
    assert!(close(&corners(&f.instance(&[900.0])), &[(0.0, 0.0), (600.0, 0.0), (600.0, 750.0), (0.0, 750.0)]));
}

#[test]
fn fvar_named_instances() {
    let f = synth(glyph_shared_all(), false);
    let ax = f.axes();
    assert_eq!(ax.len(), 1);
    assert_eq!((ax[0].tag, ax[0].min, ax[0].default, ax[0].max, ax[0].name_id), (*b"wght", 100.0, 400.0, 900.0, 256));
    let ni = f.named_instances();
    assert_eq!(ni.len(), 1);
    assert_eq!((ni[0].subfamily_name_id, ni[0].flags, ni[0].postscript_name_id), (257, 0, Some(258)));
    assert_eq!(ni[0].user_coords(), &[700.0]);
    let bold = f.named_instance(0).expect("instance 0");
    assert_eq!(bold.coords(), &[N700]);
    assert_eq!(bold.advance(0), 660);
    assert!(f.named_instance(1).is_none());
    // The text lever's defaults are today's render: with_axes(None, None) is Style::new.
    let s = Style::new(Role::Sans, false, 13.0);
    assert_eq!(s.with_axes(None, None), s);
    assert_eq!(s.with_axes(Some(700), Some(20.0)), s.with_weight(700).with_opsz(20.0));
}

#[test]
fn inter_variable_instances_when_present() {
    let Some(d) = common::load("fonts/inter/InterVariable.ttf", None) else {
        eprintln!("fvar: fonts/inter/InterVariable.ttf absent — real-font leg skipped");
        return;
    };
    let d: &'static [u8] = Box::leak(d.into_boxed_slice());
    let f = Font::parse(d).expect("Inter parses");
    let axes = f.axes();
    let ni = f.named_instances();
    assert!(!ni.is_empty(), "Inter names its instances");
    for (i, inst) in ni.iter().enumerate() {
        assert_eq!(inst.user_coords().len(), axes.len());
        for (a, &v) in axes.iter().zip(inst.user_coords()) {
            assert!(a.min <= v && v <= a.max, "instance {i}: {v} outside {:?}", a.tag);
        }
        assert_eq!(f.named_instance(i).unwrap().coords(), f.instance(inst.user_coords()).coords());
    }
    let wi = f.fvar.unwrap().index_of(*b"wght").expect("Inter has wght");
    let mut user: Vec<f32> = axes.iter().map(|a| a.default).collect();
    user[wi] = 700.0;
    let h = f.glyph_index('H');
    let bold = f.instance(&user);
    assert!(bold.advance(h) > f.advance(h), "wght 700 widens H: {} vs {}", bold.advance(h), f.advance(h));
}
