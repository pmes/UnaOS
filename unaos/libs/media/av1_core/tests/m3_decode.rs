//! M2–M4 known-answer tests: whole-frame intra decodes of the public AVIF vectors (tests/vectors.txt).
//!
//! For every vector this checks (1) the §8.2.4 exit_symbol() padding check passed on EVERY tile
//! (the arithmetic decoder consumed exactly the bits the encoder wrote — a desync anywhere in a
//! tile fails it), and (2) the FNV-1a 64 of the decoded Y/U/V planes equals the value pinned here.
//! The pinned values were recorded on 2026-10-04 from decodes that the Chromium oracle
//! (tools/av1-check/oracle) matched to max abs diff <= 2 in 8-bit RGB (the conversion rounding);
//! see docs/dev/evidence/media-1004/AVCODEC.md for the per-vector PSNR table. A change that moves
//! one of these hashes has changed decoded pixels and must be re-proven against the oracle.
mod common;
use av1_core::image::{planes_to_rgba, planes_to_rgba_with, Conversion, Filters, Upsampling};
use common::{avif_planes, fetch, fnv64};

/// name, width, height, bit depth, FNV-1a 64 of Y‖U‖V.
const PINNED: &[(&str, u32, u32, u32, u64)] = &[
    ("white_1x1", 1, 1, 8, 0x1604086b6d5b2e18),
    ("extended_pixi", 4, 4, 8, 0x3b743c1d15e01570),
    ("colors_sdr_srgb", 200, 200, 8, 0x3f249edc15fb12c5),
    ("colors_text_sdr_srgb", 200, 200, 8, 0xaa84348520f9dffc),
    ("kodim23_yuv420_8bpc", 768, 512, 8, 0x5f6f165c3b2c9aac),
    ("kodim03_yuv420_8bpc", 768, 512, 8, 0xacefae3818629dfb),
    ("fox.profile1.8bpc.yuv444.odd-width.odd-height", 1203, 799, 8, 0xa1fdb876d966c7f1),
    ("fox.profile0.10bpc.yuv420", 1204, 800, 10, 0xa6e260702dce6905),
    ("fox.profile0.8bpc.yuv420.monochrome", 1204, 800, 8, 0xa06b517adcaad4a9),
    // 12-bit 4:2:2: Chromium renders it black (no oracle) — pinned on the exit_symbol check alone.
    ("fox.profile2.12bpc.yuv422", 1204, 800, 12, 0x4473c1b92fccf751),
    ("Chimera_8bit_cropped_480x256", 480, 270, 8, 0xb84d4ce37189d894),
    ("kids_720p", 1280, 720, 8, 0xef1c1e832cc8abd5),
    ("Irvine_CA", 480, 640, 8, 0x58c86d5bd435df29),
    ("sdr_cosmos01000_cicp1-13-6_yuv420_limited_qp40", 2048, 858, 8, 0x2be3896f70e03480),
    ("sdr_cosmos01000_cicp1-13-6_yuv444_full_qp40", 2048, 858, 8, 0xf53592fcded91ea9),
];

#[test]
fn intra_vectors_decode_to_pinned_planes() {
    let print = std::env::var("AV1_PRINT_PINS").is_ok();
    let mut checked = 0;
    for name in [
        "white_1x1",
        "extended_pixi",
        "colors_sdr_srgb",
        "colors_text_sdr_srgb",
        "kodim23_yuv420_8bpc",
        "kodim03_yuv420_8bpc",
        "fox.profile1.8bpc.yuv444.odd-width.odd-height",
        "fox.profile0.10bpc.yuv420",
        "fox.profile0.8bpc.yuv420.monochrome",
        "fox.profile2.12bpc.yuv422",
        "Chimera_8bit_cropped_480x256",
        "kids_720p",
        "Irvine_CA",
        "sdr_cosmos01000_cicp1-13-6_yuv420_limited_qp40",
        "sdr_cosmos01000_cicp1-13-6_yuv444_full_qp40",
    ] {
        let Some(file) = fetch(name) else { continue };
        let p = avif_planes(&file, Filters::default()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(p.stats.tiles_exit_ok > 0 && p.stats.tiles_exit_bad == 0, "{name}: exit_symbol padding check failed on {} tile(s)", p.stats.tiles_exit_bad);
        let h = fnv64(&[&p.y, &p.u, &p.v]);
        if print {
            println!("    (\"{name}\", {}, {}, {}, 0x{h:016x}),", p.width, p.height, p.bit_depth);
            continue;
        }
        let pin = PINNED.iter().find(|x| x.0 == name).unwrap_or_else(|| panic!("{name}: no pinned hash"));
        assert_eq!((p.width, p.height, p.bit_depth), (pin.1, pin.2, pin.3), "{name}: geometry");
        assert_eq!(h, pin.4, "{name}: decoded planes changed (FNV-1a 64 0x{h:016x})");
        // The RGBA face is total: one pixel per luma sample.
        let img = planes_to_rgba(&p, Upsampling::Bilinear);
        assert_eq!(img.rgba.len(), (img.w * img.h * 4) as usize);
        checked += 1;
    }
    eprintln!("m3: {checked} vectors matched their pinned planes");
}

#[test]
fn stream_decoder_matches_the_still_path() {
    // The video seam (StreamDecoder): an AV1 key frame fed as one temporal unit, with the item's own av1C
    // record (as demux_core's HEIF reader hands it), decodes to the same planes as the AVIF path.
    let Some(file) = fetch("kodim23_yuv420_8bpc") else { return };
    let item = common::avif_item(&file).unwrap();
    let mut d = av1_core::image::StreamDecoder::new(item.av1c.as_deref().unwrap()).unwrap();
    let p = d.decode_temporal_unit(&item.data).unwrap();
    let pin = PINNED.iter().find(|x| x.0 == "kodim23_yuv420_8bpc").unwrap();
    assert_eq!(fnv64(&[&p.y, &p.u, &p.v]), pin.4);
    // A second unit through the same decoder (state carried) decodes identically.
    let p2 = d.decode_temporal_unit(&item.data).unwrap();
    assert_eq!(fnv64(&[&p2.y, &p2.u, &p2.v]), pin.4);
}

// AV1COLOURONE (LEDGER SR340): planes_to_rgba runs Stria's ONE integer kernel (yuv_core), the one gneiss_pal's
// `Frame::to_rgba_colour` runs: the same planes give COLOURONE's (SR328) hand-computed answers.
fn planes(w: u32, h: u32, n: u32, (ss_x, ss_y): (u32, u32), mono: bool, (y, u, v): (Vec<u16>, Vec<u16>, Vec<u16>), mc: u8, full: bool) -> av1_core::image::Planes {
    av1_core::image::Planes {
        width: w,
        height: h,
        bit_depth: n,
        mono,
        ss_x,
        ss_y,
        y,
        u,
        v,
        color_primaries: 2,
        transfer_characteristics: 2,
        matrix_coefficients: mc,
        full_range: full,
        seq: Default::default(),
        frame: Default::default(),
        stats: Default::default(),
    }
}

#[test]
fn colourone_av1_planes_convert_as_gneiss_pal_does() {
    // 8-bit BT.709 limited Y 120 Cb 140 Cr 100 -> (71, 133, 146); 10-bit BT.2020 limited Y 400 Cb 600 Cr 300 -> (9, 128, 145):
    // gneiss_pal video.rs `colourone_hand_computed_samples`, the same planes through av1_core.
    let p8 = planes(2, 2, 8, (1, 1), false, (vec![120; 4], vec![140], vec![100]), 1, false);
    let p10 = planes(2, 2, 10, (1, 1), false, (vec![400; 4], vec![600], vec![300]), 9, false);
    for up in [Upsampling::Nearest, Upsampling::Bilinear] {
        assert_eq!(planes_to_rgba(&p8, up).rgba[..4], [71, 133, 146, 255], "{up:?}");
        assert_eq!(planes_to_rgba(&p10, up).rgba[..4], [9, 128, 145, 255], "{up:?}");
    }
    // The stills convention is unchanged: 2 (unspecified) is BT.601 -> (76, 139, 145), 10 is BT.2020's Kr / Kb.
    let p2 = planes(2, 2, 8, (1, 1), false, (vec![120; 4], vec![140], vec![100]), 2, false);
    assert_eq!(planes_to_rgba(&p2, Upsampling::Nearest).rgba[..3], [76, 139, 145]);
    let p10cl = planes(2, 2, 10, (1, 1), false, (vec![400; 4], vec![600], vec![300]), 10, false);
    assert_eq!(planes_to_rgba(&p10cl, Upsampling::Nearest).rgba[..3], [9, 128, 145]);
}

#[test]
fn colourone_av1_bilinear_mono_and_the_libyuv_oracle() {
    // 4x2 8-bit 4:2:0 BT.709 limited, Y 120, Cb (100, 180), Cr 128: bilinear 9-3-3-1 gives Cb 100 / 120 / 160 / 180 per column.
    let p = planes(4, 2, 8, (1, 1), false, (vec![120; 8], vec![100, 180], vec![128; 2]), 1, false);
    let img = planes_to_rgba(&p, Upsampling::Bilinear);
    let want = [[121, 127, 62], [121, 123, 104], [121, 114, 189], [121, 110, 231]];
    for (col, w) in want.iter().enumerate() {
        assert_eq!(img.rgba[col * 4..][..3], w[..], "col {col}");
    }
    // Monochrome 10-bit limited Y 502 is grey 127 (no chroma planes).
    let m = planes(2, 2, 10, (1, 1), true, (vec![502; 4], vec![], vec![]), 1, false);
    assert_eq!(planes_to_rgba(&m, Upsampling::Bilinear).rgba[..4], [127, 127, 127, 255]);
    // ORACLE ONLY: libyuv's quantised chroma (BT.2020 Cb -> B capped at 128/64 in 8-bit units) through the same loop.
    let o = planes(2, 2, 8, (1, 1), false, (vec![120; 4], vec![170], vec![128]), 9, false);
    assert_eq!(planes_to_rgba_with(&o, Upsampling::Nearest, Conversion::Exact).rgba[..3], [121, 113, 211]);
    assert_eq!(planes_to_rgba_with(&o, Upsampling::Nearest, Conversion::Libyuv).rgba[..3], [121, 113, 205]);
}
