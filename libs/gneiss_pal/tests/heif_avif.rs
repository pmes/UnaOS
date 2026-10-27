// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HEIFONE (LEDGER SR303): every public AVIF vector av1_core's own tests fetch (`unaos/libs/media/av1_core/tests/
//! vectors.txt`, sha256-pinned, cached, SKIP offline — the ONE fetcher, reused here by path) decodes through
//! `dsp::image::decode_heif` (demux_core's HEIF reader + av1_core) to the pixels av1_core's own item walk gives
//! (`av1_core::image::decode_avif`): byte-identical when the primary item carries no transform; with `clap` / `irot`
//! (applied by decode_heif, by nobody on the old path) the geometry is the transformed one.
#![cfg(feature = "av1")]

#[path = "../../../unaos/libs/media/av1_core/tests/common/mod.rs"]
mod common;

use gneiss_pal::dsp::demux::heif::{self, Transform};
use gneiss_pal::dsp::image::decode_heif;

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
        let Ok(old) = av1_core::image::decode_avif(&file) else {
            continue; // av1_core refuses it on the old path too (an owed feature): nothing to compare
        };
        let h = heif::parse_heif(&file).unwrap_or_else(|e| panic!("{}: heif refuses what avif.rs read: {e:?}", v.name));
        let primary = h.item(h.primary).unwrap();
        let new = decode_heif(&file).unwrap_or_else(|e| panic!("{}: decode_heif: {e:?}", v.name));
        if primary.transforms.is_empty() {
            assert_eq!((new.width, new.height), (old.w, old.h), "{}", v.name);
            assert!(new.rgba == old.rgba, "{}: decode_heif differs from decode_avif", v.name);
            same += 1;
            continue;
        }
        let (mut w, mut ht) = (old.w, old.h);
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
