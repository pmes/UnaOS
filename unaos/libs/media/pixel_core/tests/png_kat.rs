// SPDX-License-Identifier: LGPL-3.0-or-later
//! M1 KATs: the PngSuite (basic, interlaced, sizes, filters, transparency, zlib levels, chunk
//! ordering) must decode pixel-identical to the `image` crate; the corrupt `x*` set must be refused.

mod common;

#[test]
fn pngsuite_matches_second_opinion() {
    let names = common::list("pngsuite", ".png");
    if names.is_empty() {
        eprintln!("SKIP pngsuite: no vectors");
        return;
    }
    let (mut good, mut refused) = (0, 0);
    for n in &names {
        let bytes = common::load(&format!("pngsuite/{n}")).unwrap();
        let ours = pixel_core::decode(&bytes);
        if n.starts_with('x') {
            assert!(ours.is_err(), "{n}: corrupt file must be refused, decoded instead");
            refused += 1;
            continue;
        }
        let img = ours.unwrap_or_else(|e| panic!("{n}: {e}"));
        let (w, h, want) = common::reference(&bytes).unwrap_or_else(|| panic!("{n}: reference failed"));
        assert_eq!((img.width, img.height), (w, h), "{n}: dimensions");
        let (m, cnt) = common::diff(&img.rgba, &want);
        assert_eq!(cnt, 0, "{n}: {cnt} channel bytes differ, max {m}");
        good += 1;
    }
    eprintln!("pngsuite: {good} decoded exact, {refused} corrupt refused");
}

#[test]
fn encoder_round_trips_through_decoder() {
    use pixel_core::png::encode::PngEncoder;
    let (w, h) = (37u32, 23u32);
    let mut enc = PngEncoder::new(w, h).unwrap();
    let mut file = Vec::new();
    let mut rgb = Vec::new();
    for y in 0..h {
        let row: Vec<u8> = (0..w).flat_map(|x| [(x * 7) as u8, (y * 11) as u8, (x ^ y) as u8]).collect();
        enc.push_row(&row).unwrap();
        let mut p = Vec::new();
        while enc.next_piece(&mut p) {
            file.extend_from_slice(&p);
        }
        rgb.extend_from_slice(&row);
    }
    enc.finish().unwrap();
    let mut p = Vec::new();
    while enc.next_piece(&mut p) {
        file.extend_from_slice(&p);
    }
    let img = pixel_core::decode(&file).unwrap();
    let want: Vec<u8> = rgb.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    assert_eq!(img.rgba, want);
}

/// PIXEL-UNFILTER (SR316): forward PNG filter of `rows` (each `bpp`-pixel line) with filter type `ft`, the test's
/// own encoder (§9.2-9.4), so `unfilter_rows` is checked against an independent writer.
fn png_filter(rows: &[Vec<u8>], bpp: usize, ft: u8) -> Vec<u8> {
    fn paeth(a: u8, b: u8, c: u8) -> u8 {
        let p = a as i16 + b as i16 - c as i16;
        let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
        if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
    }
    let mut out = Vec::new();
    let mut prev = vec![0u8; rows[0].len()];
    for r in rows {
        out.push(ft);
        for i in 0..r.len() {
            let a = if i >= bpp { r[i - bpp] } else { 0 };
            let b = prev[i];
            let c = if i >= bpp { prev[i - bpp] } else { 0 };
            let pred = match ft {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                _ => paeth(a, b, c),
            };
            out.push(r[i].wrapping_sub(pred));
        }
        prev = r.clone();
    }
    out
}

#[test]
fn unfilter_rows_five_types() {
    use pixel_core::png::unfilter_rows;
    // 3x2 RGB, 8 bits: bpp 3, 9 bytes a row, stride 10.
    let rows = vec![vec![10u8, 20, 30, 40, 50, 60, 70, 80, 90], vec![15u8, 25, 35, 200, 10, 250, 5, 128, 255]];
    let want = rows.concat();
    for ft in 0..5u8 {
        let mut d = png_filter(&rows, 3, ft);
        unfilter_rows(3, 10, &mut d).unwrap();
        let got: Vec<u8> = d.chunks(10).flat_map(|l| l[1..].to_vec()).collect();
        assert_eq!(got, want, "filter type {ft}");
    }
    // Hand-written Sub row: x[i] - x[i-3].
    let mut d = vec![1u8, 10, 20, 30, 30, 30, 30, 30, 30, 30];
    unfilter_rows(3, 10, &mut d).unwrap();
    assert_eq!(&d[1..], &rows[0][..]);
    // A short last row (PDF predictor streams that stop mid-row): Up over the full row before it.
    let mut d = png_filter(&rows, 3, 2);
    d.truncate(14);
    unfilter_rows(3, 10, &mut d).unwrap();
    assert_eq!(&d[11..14], &rows[1][..3]);
    // An unknown filter type is refused by name.
    let mut d = vec![5u8; 10];
    assert!(matches!(unfilter_rows(3, 10, &mut d), Err(pixel_core::Error::Malformed("png filter type"))));
}
