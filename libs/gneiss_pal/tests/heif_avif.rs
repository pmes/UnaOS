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
const OLD: &[(&str, u32, u32, u64)] = &[
    ("white_1x1", 1, 1, 0x2fa324587145aacb),
    ("extended_pixi", 4, 4, 0x0aca7e310b81f28e),
    ("colors_sdr_srgb", 200, 200, 0xbdb5ad19b23882d6),
    ("colors_text_sdr_srgb", 200, 200, 0xe1f173952bb6215a),
    ("kodim23_yuv420_8bpc", 768, 512, 0x9ddd9442c70b3f2b),
    ("kodim03_yuv420_8bpc", 768, 512, 0x2dee3d92f1a64edf),
    ("fox.profile1.8bpc.yuv444.odd-width.odd-height", 1203, 799, 0x8ff85844492eb751),
    ("fox.profile0.10bpc.yuv420", 1204, 800, 0xf5f3c0e83b9e9436),
    ("fox.profile0.8bpc.yuv420.monochrome", 1204, 800, 0x4e8b17ff401a8094),
    ("fox.profile2.12bpc.yuv422", 1204, 800, 0x2c0c11c6e444501d),
    ("Chimera_8bit_cropped_480x256", 480, 270, 0xfda6d8f3902a16e5),
    ("kids_720p", 1280, 720, 0xa0ef485ceba07f61),
    ("Irvine_CA", 480, 640, 0x8a916ec574ea7656),
    ("sdr_cosmos01000_cicp1-13-6_yuv420_limited_qp40", 2048, 858, 0xe7c743bfc5f06aed),
    ("sdr_cosmos01000_cicp1-13-6_yuv444_full_qp40", 2048, 858, 0xc44ebd10e5f7aaa9),
    ("Monochrome", 1280, 720, 0x6fce5b9729507a82),
    ("fruits_2layer_thumbsize", 1296, 864, 0x55a61fdd8ee01112),
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
