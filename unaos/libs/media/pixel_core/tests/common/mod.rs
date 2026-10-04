// SPDX-License-Identifier: LGPL-3.0-or-later
//! Shared KAT plumbing: the vector cache (`tests/vectors/`, filled by `fetch-vectors.sh` from the
//! URLs + sha256 in `tests/vectors.txt`) and the second-opinion comparison against the `image` crate.
#![allow(dead_code)]

use std::path::PathBuf;

pub fn vdir() -> PathBuf {
    std::env::var_os("PIXEL_VECTORS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors"))
}

/// Read a cached vector, or `None` (the test then prints SKIP: offline / not fetched).
pub fn load(rel: &str) -> Option<Vec<u8>> {
    let p = vdir().join(rel);
    match std::fs::read(&p) {
        Ok(b) => Some(b),
        Err(_) => {
            eprintln!("SKIP {rel}: not in {} (run fetch-vectors.sh)", vdir().display());
            None
        }
    }
}

/// All cached files under a vector subdirectory with the given extension, sorted.
pub fn list(sub: &str, ext: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(vdir().join(sub))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(ext))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// The `image` crate's straight-RGBA8 answer for the same bytes.
pub fn reference(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    Some((img.width(), img.height(), img.into_raw()))
}

/// (max abs channel diff, count of differing channel bytes).
pub fn diff(a: &[u8], b: &[u8]) -> (u8, usize) {
    assert_eq!(a.len(), b.len());
    let mut m = 0u8;
    let mut n = 0usize;
    for (x, y) in a.iter().zip(b) {
        let d = x.abs_diff(*y);
        if d != 0 {
            n += 1;
            m = m.max(d);
        }
    }
    (m, n)
}

/// PSNR in dB over the RGB channels (alpha ignored).
pub fn psnr_rgb(a: &[u8], b: &[u8]) -> f64 {
    let mut se = 0f64;
    let mut n = 0f64;
    for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for c in 0..3 {
            let d = p[c] as f64 - q[c] as f64;
            se += d * d;
            n += 1.0;
        }
    }
    if se == 0.0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 / (se / n)).log10() }
}
