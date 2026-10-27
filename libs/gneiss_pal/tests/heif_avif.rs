// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HEIFONE (LEDGER SR303): every public AVIF vector av1_core's own tests fetch (`unaos/libs/media/av1_core/tests/
//! vectors.txt`, sha256-pinned, cached, SKIP offline — the ONE fetcher, reused here by path) decodes through
//! `dsp::image::decode_heif` (demux_core's HEIF reader + av1_core) to the pixels av1_core's own item walk gave
//! (`av1_core::image::decode_avif`, retired by HEIFONE-RETIRE SR318): byte-identical when the primary item carries no
//! transform; with `clap` / `irot` (applied by decode_heif, by nobody on the old path) the geometry is the transformed
//! one. `OLD` is that walk's output, pinned before it retired: (name, width, height, FNV-1a 64 of the RGBA).
#![cfg(feature = "av1")]

#[path = "../../../unaos/libs/media/av1_core/tests/common/mod.rs"]
mod common;

use gneiss_pal::dsp::demux::heif::{self, Transform};
use gneiss_pal::dsp::image::decode_heif;

/// `av1_core::image::decode_avif` per vector, recorded at db1ebf8fb (HEIFONE-RETIRE SR318) before the walk retired.
/// AV1COLOURONE (LEDGER SR340) re-pinned 15 of them: av1_core's f64 converter retired onto yuv_core's integer kernel (the
/// one gneiss_pal's Player runs). Per-vector max |d| per channel, retired f64 vs the integer kernel on the same planes
/// (bytes changed of total): white_1x1 0 and extended_pixi 0 (pins unchanged); colors_sdr_srgb 1 (3000 / 160000),
/// colors_text_sdr_srgb 1 (2879 / 160000), kodim23 1 (25970 / 1572864), kodim03 1 (22629 / 1572864), fox 8bpc 4:4:4 1
/// (32433 / 3844788), fox 10bpc 1 (2021 / 3852800), fox 8bpc mono 1 (27507 / 3852800), fox 12bpc 4:2:2 1 (10522 / 3852800),
/// Chimera 1 (2813 / 518400), kids_720p 1 (27764 / 3686400), Irvine_CA 1 (21900 / 1228800), cosmos 4:2:0 limited 1
/// (121869 / 7028736), cosmos 4:4:4 full 1 (95636 / 7028736), Monochrome 1 (54669 / 3686400), fruits_2layer 1
/// (114625 / 4478976). No vector moves by more than 1.
const OLD: &[(&str, u32, u32, u64)] = &[
    ("white_1x1", 1, 1, 0x2fa324587145aacb),
    ("extended_pixi", 4, 4, 0x0aca7e310b81f28e),
    ("colors_sdr_srgb", 200, 200, 0xd8ac97c41cf5d126),
    ("colors_text_sdr_srgb", 200, 200, 0x8aeb27c8f220061b),
    ("kodim23_yuv420_8bpc", 768, 512, 0x047a07e763897181),
    ("kodim03_yuv420_8bpc", 768, 512, 0x8b96062ff3f6bac8),
    ("fox.profile1.8bpc.yuv444.odd-width.odd-height", 1203, 799, 0x4c98e4fdefe27d8a),
    ("fox.profile0.10bpc.yuv420", 1204, 800, 0x358c0b81cb616093),
    ("fox.profile0.8bpc.yuv420.monochrome", 1204, 800, 0x04a860a0f2e12ffb),
    ("fox.profile2.12bpc.yuv422", 1204, 800, 0xc25a132280073723),
    ("Chimera_8bit_cropped_480x256", 480, 270, 0xfd87c5346f79c9dc),
    ("kids_720p", 1280, 720, 0xc6df8be178202bd9),
    ("Irvine_CA", 480, 640, 0x66597fabf0fbf5b4),
    ("sdr_cosmos01000_cicp1-13-6_yuv420_limited_qp40", 2048, 858, 0xc4c9907cbcb2f994),
    ("sdr_cosmos01000_cicp1-13-6_yuv444_full_qp40", 2048, 858, 0x8c46d6580908b617),
    ("Monochrome", 1280, 720, 0x69d2d41a609ad523),
    ("fruits_2layer_thumbsize", 1296, 864, 0x0a421383ce0d627b),
];

fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

#[test]
fn public_avif_vectors_decode_through_heif_as_through_av1_core() {
    let (mut same, mut transformed, mut skipped) = (0, 0, 0);
    for v in common::vectors() {
        if !v.url.ends_with(".avif") {
            continue;
        }
        let Some(file) = common::fetch(&v.name) else {
            skipped += 1;
            continue;
        };
        let &(_, ow, oh, pin) = OLD.iter().find(|o| o.0 == v.name).unwrap_or_else(|| panic!("{}: no pinned old-path RGBA", v.name));
        let h = heif::parse_heif(&file).unwrap_or_else(|e| panic!("{}: heif refuses it: {e:?}", v.name));
        let primary = h.item(h.primary).unwrap();
        let new = decode_heif(&file).unwrap_or_else(|e| panic!("{}: decode_heif: {e:?}", v.name));
        if primary.transforms.is_empty() {
            assert_eq!((new.width, new.height), (ow, oh), "{}", v.name);
            assert_eq!(fnv(&new.rgba), pin, "{}: decode_heif differs from the retired decode_avif", v.name);
            same += 1;
            continue;
        }
        let (mut w, mut ht) = (ow, oh);
        for t in &primary.transforms {
            match *t {
                Transform::Clap(c) => (_, _, w, ht) = c.rect(w, ht).unwrap(),
                Transform::Irot(a) if a & 1 == 1 => (w, ht) = (ht, w),
                _ => {}
            }
        }
        assert_eq!((new.width, new.height), (w, ht), "{}: transformed geometry", v.name);
        transformed += 1;
    }
    eprintln!("heif_avif: {same} vectors byte-identical, {transformed} transformed, {skipped} skipped (offline)");
}
