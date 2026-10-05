// SPDX-License-Identifier: LGPL-3.0-or-later
//! Shared KAT plumbing: the vector cache (fetched at test time with `curl` from the URLs in
//! `tests/vectors.txt`, verified by `sha256sum`; offline => the caller prints SKIP), an IVF reader,
//! and an MD5 (RFC 1321) for the reference decoder's per-frame I420 digests.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

pub fn cache_dir() -> PathBuf {
    std::env::var_os("VP8_VECTORS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("vp8-vectors"))
}

fn entry(rel: &str) -> Option<(String, String)> {
    let list = include_str!("../vectors.txt");
    list.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
        let mut it = l.split_whitespace();
        let (sum, path, url) = (it.next()?, it.next()?, it.next()?);
        (path == rel).then(|| (sum.to_string(), url.to_string()))
    })
}

fn sha256(p: &std::path::Path) -> Option<String> {
    let out = Command::new("sha256sum").arg(p).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).split_whitespace().next()?.to_string())
}

/// The bytes of a listed vector, fetching (and verifying) it on first use. `None` = offline or
/// the digest did not match; the test prints SKIP.
pub fn load(rel: &str) -> Option<Vec<u8>> {
    let (sum, url) = entry(rel).unwrap_or_else(|| panic!("{rel} is not in vectors.txt"));
    let path = cache_dir().join(rel);
    if path.exists() && sha256(&path).as_deref() == Some(sum.as_str()) {
        return std::fs::read(&path).ok();
    }
    std::fs::create_dir_all(path.parent()?).ok()?;
    let part = path.with_extension("part");
    let ok = Command::new("curl").args(["-sSfL", "--retry", "2", "-o"]).arg(&part).arg(&url).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP {rel}: fetch failed (offline?)");
        let _ = std::fs::remove_file(&part);
        return None;
    }
    if sha256(&part).as_deref() != Some(sum.as_str()) {
        eprintln!("SKIP {rel}: sha256 mismatch");
        let _ = std::fs::remove_file(&part);
        return None;
    }
    std::fs::rename(&part, &path).ok()?;
    std::fs::read(&path).ok()
}

/// IVF (the libvpx test container): 32-byte file header, then 12-byte frame headers
/// (size u32 LE, pts u64 LE) each followed by one compressed frame.
pub fn ivf_frames(b: &[u8]) -> Vec<&[u8]> {
    assert_eq!(&b[0..4], b"DKIF", "not IVF");
    let hlen = u16::from_le_bytes([b[6], b[7]]) as usize;
    let mut p = hlen;
    let mut v = Vec::new();
    while p + 12 <= b.len() {
        let n = u32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]) as usize;
        p += 12;
        v.push(&b[p..(p + n).min(b.len())]);
        p += n;
    }
    v
}

/// MD5 (RFC 1321) as lowercase hex.
pub fn md5_hex(data: &[u8]) -> String {
    let s: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16,
        23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut msg = data.to_vec();
    let bitlen = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_le_bytes());
    for chunk in msg.chunks_exact(64) {
        let m: Vec<u32> = chunk.chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f2 = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f2.rotate_left(s[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = String::new();
    for w in [a0, b0, c0, d0] {
        for byte in w.to_le_bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
    }
    out
}

#[test]
fn md5_known_answers() {
    assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(md5_hex(b"The quick brown fox jumps over the lazy dog"), "9e107d9d372bb6826bd81d3542a419d6");
}
