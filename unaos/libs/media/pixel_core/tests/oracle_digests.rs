// SPDX-License-Identifier: LGPL-3.0-or-later
//! Chromium-pinned KATs: every line of `tests/oracle_digests.txt` is a file whose pixel_core decode was
//! byte-identical to Chromium's rendering when it was pinned; the CRC-32 of the RGBA must not move.

mod common;

#[test]
fn chromium_pinned_digests() {
    let list = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/oracle_digests.txt")).unwrap();
    let (mut checked, mut skipped) = (0, 0);
    for line in list.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (path, dims, want, mode) = (f[0], f[1], f[2], f[3]);
        let Some(bytes) = common::load(path) else {
            skipped += 1;
            continue;
        };
        let mut img = pixel_core::decode(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        if mode == "orient" {
            img.apply_orientation();
        }
        let mut c = pixel_core::crc::Crc32::new();
        match img.frames.as_ref() {
            Some(fr) => fr.iter().for_each(|f| c.update(&f.rgba)),
            None => c.update(&img.rgba),
        }
        assert_eq!(format!("{}x{}", img.width, img.height), dims, "{path}: dimensions");
        assert_eq!(format!("{:08x}", c.finish()), want, "{path}: RGBA digest moved off Chromium's answer");
        checked += 1;
    }
    eprintln!("oracle digests: {checked} checked, {skipped} skipped (not fetched)");
}
