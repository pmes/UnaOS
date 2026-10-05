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
