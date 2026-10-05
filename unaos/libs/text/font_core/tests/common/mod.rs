//! Shared helpers for the host tests (std is fine here; the crate itself is no_std).
#![allow(dead_code)]

use font_core::PathCmd;

/// FIPS 180-4 SHA-256 — only to confirm a container font is the exact file the KATs were generated from.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
        0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
        0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
        0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] =
        [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

/// Read a font file if present and (when `sha` is given) identical to the KAT's source.
pub fn load(path: &str, sha: Option<&str>) -> Option<Vec<u8>> {
    let d = std::fs::read(path).ok()?;
    if let Some(s) = sha {
        if sha256_hex(&d) != s {
            eprintln!("SKIP {path}: sha256 differs from the KAT source");
            return None;
        }
    }
    Some(d)
}

/// Signed area of a path by Green's theorem, exact for lines, quadratics and cubics
/// (same sign convention as fontTools' AreaPen: counter-clockwise positive).
pub fn signed_area(cmds: &[PathCmd]) -> f64 {
    let mut a = 0.0f64;
    let (mut x0, mut y0, mut sx, mut sy) = (0f64, 0f64, 0f64, 0f64);
    let line = |a: &mut f64, x0: f64, y0: f64, x1: f64, y1: f64| *a -= (x1 - x0) * (y1 + y0) * 0.5;
    for c in cmds {
        match *c {
            PathCmd::MoveTo(x, y) => {
                x0 = x as f64;
                y0 = y as f64;
                sx = x0;
                sy = y0;
            }
            PathCmd::LineTo(x, y) => {
                line(&mut a, x0, y0, x as f64, y as f64);
                x0 = x as f64;
                y0 = y as f64;
            }
            PathCmd::QuadTo(x1, y1, x, y) => {
                let (ax, ay) = (x1 as f64 - x0, y1 as f64 - y0);
                let (bx, by) = (x as f64 - x0, y as f64 - y0);
                a -= (bx * ay - ax * by) / 3.0;
                line(&mut a, x0, y0, x as f64, y as f64);
                x0 = x as f64;
                y0 = y as f64;
            }
            PathCmd::CubicTo(x1, y1, x2, y2, x, y) => {
                let (ax, ay) = (x1 as f64 - x0, y1 as f64 - y0);
                let (bx, by) = (x2 as f64 - x0, y2 as f64 - y0);
                let (cx, cy) = (x as f64 - x0, y as f64 - y0);
                a -= (ax * (-by - cy) + bx * (ay - 2.0 * cy) + cx * (ay + 2.0 * by)) * 0.15;
                line(&mut a, x0, y0, x as f64, y as f64);
                x0 = x as f64;
                y0 = y as f64;
            }
            PathCmd::Close => {
                line(&mut a, x0, y0, sx, sy);
                x0 = sx;
                y0 = sy;
            }
        }
    }
    a
}

/// A UCD / test-vector file by name: `$FONTBIDI_UCD_DIR/<name>` if set, else a cached copy under the cargo target
/// tmpdir, else fetched with `curl` from `url`. The sha256 must match. `None` (the caller prints SKIP) when offline.
pub fn vector_file(name: &str, url: &str, sha: &str) -> Option<Vec<u8>> {
    let check = |d: Vec<u8>| if sha256_hex(&d) == sha { Some(d) } else { eprintln!("{name}: sha256 mismatch"); None };
    if let Ok(dir) = std::env::var("FONTBIDI_UCD_DIR") {
        if let Ok(d) = std::fs::read(std::path::Path::new(&dir).join(name)) {
            return check(d);
        }
    }
    let cache = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("fontbidi-vectors");
    let _ = std::fs::create_dir_all(&cache);
    let p = cache.join(name);
    if let Ok(d) = std::fs::read(&p) {
        if let Some(d) = check(d) {
            return Some(d);
        }
    }
    let ok = std::process::Command::new("curl")
        .args(["-sSfL", "--max-time", "120", "-o"])
        .arg(&p)
        .arg(url)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("SKIP {name}: could not fetch {url}");
        return None;
    }
    check(std::fs::read(&p).ok()?)
}

/// The unicodetools repository's UCD 17.0.0 directory (www.unicode.org is refused by this host's egress proxy).
pub const UCD_BASE: &str = "https://raw.githubusercontent.com/unicode-org/unicodetools/main/unicodetools/data/ucd/17.0.0/";

/// The Noto fonts the FONTBIDI oracles use (notofonts.github.io, unhinted TTF), fetched at test time and pinned.
pub fn noto_font(name: &str, sha: &str) -> Option<Vec<u8>> {
    let family = name.split('-').next()?;
    let url = format!("https://raw.githubusercontent.com/notofonts/notofonts.github.io/main/fonts/{family}/unhinted/ttf/{name}");
    vector_file(name, &url, sha)
}
