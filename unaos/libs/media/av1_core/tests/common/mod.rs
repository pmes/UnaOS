//! Shared test support: SHA-256 (FIPS 180-4, written here — no crates) and the fetch-at-test-time
//! cache for the public vectors listed in tests/vectors.txt.
#![allow(dead_code)]

use std::path::PathBuf;

pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01,
        0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
        0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
        0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08,
        0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    let bitlen = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_be_bytes());
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
    let mut out = [0u8; 32];
    for i in 0..8 {
        out[4 * i..4 * i + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub struct Vector {
    pub name: String,
    pub sha256: String,
    pub url: String,
}

pub fn vectors() -> Vec<Vector> {
    let txt = include_str!("../vectors.txt");
    txt.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            Vector { name: f[0].to_string(), sha256: f[1].to_string(), url: f[3].to_string() }
        })
        .collect()
}

/// Fetch (once, cached under the cargo target tmpdir) and verify a vector. `None` = offline.
pub fn fetch(name: &str) -> Option<Vec<u8>> {
    let v = vectors().into_iter().find(|v| v.name == name).unwrap_or_else(|| panic!("no vector {name}"));
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("av1_core_vectors");
    std::fs::create_dir_all(&dir).ok()?;
    // keep the URL's extension (.avif, .ivf, .md5, ...)
    let ext = v.url.rsplit('/').next().and_then(|b| b.split_once('.')).map(|(_, e)| e.to_string()).unwrap_or_else(|| "bin".into());
    let path = dir.join(format!("{name}.{ext}"));
    if !path.exists() {
        let ok = std::process::Command::new("curl")
            .args(["-sSfL", "--max-time", "60", "-o"])
            .arg(&path)
            .arg(&v.url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            let _ = std::fs::remove_file(&path);
            eprintln!("SKIP {name}: could not fetch {} (offline?)", v.url);
            return None;
        }
    }
    let data = std::fs::read(&path).ok()?;
    let got = hex(&sha256(&data));
    assert_eq!(got, v.sha256, "sha256 mismatch for {name} ({})", v.url);
    Some(data)
}

/// MD5 (RFC 1321) — the libaom test vectors publish one MD5 per decoded frame.
pub fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4,
        11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = (0..16).map(|i| u32::from_le_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (mut f, g);
            if i < 16 {
                f = (b & c) | (!b & d);
                g = i;
            } else if i < 32 {
                f = (d & b) | (!d & c);
                g = (5 * i + 1) % 16;
            } else if i < 48 {
                f = b ^ c ^ d;
                g = (3 * i + 5) % 16;
            } else {
                f = c ^ (b | !d);
                g = (7 * i) % 16;
            }
            f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..].copy_from_slice(&d0.to_le_bytes());
    out
}

/// Split an IVF file into its frame payloads (temporal units).
pub fn ivf_frames(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    if data.len() < 32 || &data[0..4] != b"DKIF" {
        return out;
    }
    let mut off = u16::from_le_bytes([data[6], data[7]]) as usize;
    while off + 12 <= data.len() {
        let sz = u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as usize;
        off += 12;
        if off + sz > data.len() {
            break;
        }
        out.push(&data[off..off + sz]);
        off += sz;
    }
    out
}

/// Matroska / WebM: read an EBML variable-length integer (ID keeps its marker bit).
fn ebml_vint(d: &[u8], off: usize, keep_marker: bool) -> Option<(u64, usize)> {
    let b = *d.get(off)?;
    let len = b.leading_zeros() as usize + 1;
    if len > 8 || off + len > d.len() {
        return None;
    }
    let mut v = if keep_marker { b as u64 } else { (b as u64) & ((1u64 << (8 - len)) - 1) };
    for i in 1..len {
        v = (v << 8) | d[off + i] as u64;
    }
    Some((v, len))
}

/// The frames of the first video track of a Matroska/WebM file: (CodecPrivate, frames).
/// Walks Segment > Cluster > SimpleBlock / BlockGroup > Block (no lacing).
pub fn mkv_frames(data: &[u8]) -> Option<(Vec<u8>, Vec<&[u8]>)> {
    const SEGMENT: u64 = 0x18538067;
    const CLUSTER: u64 = 0x1F43B675;
    const SIMPLEBLOCK: u64 = 0xA3;
    const BLOCKGROUP: u64 = 0xA0;
    const BLOCK: u64 = 0xA1;
    const TRACKS: u64 = 0x1654AE6B;
    const TRACKENTRY: u64 = 0xAE;
    const TRACKNUMBER: u64 = 0xD7;
    const TRACKTYPE: u64 = 0x83;
    const CODECPRIVATE: u64 = 0x63A2;
    let mut frames = Vec::new();
    let mut codec_private = Vec::new();
    let mut video_track = 0u64;
    fn walk<'a>(d: &'a [u8], mut off: usize, end: usize, f: &mut dyn FnMut(u64, &'a [u8]) -> bool) {
        while off < end {
            let Some((id, il)) = ebml_vint(d, off, true) else { return };
            let Some((size, sl)) = ebml_vint(d, off + il, false) else { return };
            let start = off + il + sl;
            let unknown = size == (1u64 << (7 * sl)) - 1;
            let stop = if unknown { end } else { (start + size as usize).min(end) };
            if f(id, &d[start..stop]) {
                walk(d, start, stop, f);
            }
            off = stop;
        }
    }
    let mut blocks: Vec<&[u8]> = Vec::new();
    walk(data, 0, data.len(), &mut |id, body| match id {
        SEGMENT | CLUSTER | BLOCKGROUP | TRACKS => true,
        TRACKENTRY => {
            let mut num = 0u64;
            let mut typ = 0u64;
            let mut cp: &[u8] = &[];
            walk(body, 0, body.len(), &mut |id2, b2| {
                match id2 {
                    TRACKNUMBER => num = b2.iter().fold(0, |a, &x| (a << 8) | x as u64),
                    TRACKTYPE => typ = b2.iter().fold(0, |a, &x| (a << 8) | x as u64),
                    CODECPRIVATE => cp = b2,
                    _ => {}
                }
                false
            });
            if typ == 1 && video_track == 0 {
                video_track = num;
                codec_private = cp.to_vec();
            }
            false
        }
        SIMPLEBLOCK | BLOCK => {
            blocks.push(body);
            false
        }
        _ => false,
    });
    for b in blocks {
        let Some((track, tl)) = ebml_vint(b, 0, false) else { continue };
        if track != video_track || b.len() < tl + 3 {
            continue;
        }
        let flags = b[tl + 2];
        if flags & 0x06 != 0 {
            continue; // laced blocks are not used for video
        }
        frames.push(&b[tl + 3..]);
    }
    Some((codec_private, frames))
}

/// FNV-1a 64 over 16-bit samples — the regression fingerprint of decoded planes.
pub fn fnv64(planes: &[&[u16]]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in planes {
        for &s in p.iter() {
            for b in s.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
        }
    }
    h
}

/// MSB-first bit writer for hand-built headers.
#[derive(Default)]
pub struct BitWriter {
    pub bytes: Vec<u8>,
    pub nbits: usize,
}
impl BitWriter {
    pub fn put(&mut self, n: u32, v: u64) {
        for i in (0..n).rev() {
            let bit = ((v >> i) & 1) as u8;
            if self.nbits % 8 == 0 {
                self.bytes.push(0);
            }
            let last = self.bytes.len() - 1;
            self.bytes[last] |= bit << (7 - (self.nbits % 8));
            self.nbits += 1;
        }
    }
    pub fn flag(&mut self, b: bool) {
        self.put(1, b as u64);
    }
    /// su(n) of a signed value
    pub fn su(&mut self, n: u32, v: i64) {
        self.put(n, (v as u64) & ((1u64 << n) - 1));
    }
    /// trailing_bits(): a one then zeros to the byte boundary
    pub fn trailing(&mut self) {
        self.put(1, 1);
        while self.nbits % 8 != 0 {
            self.put(1, 0);
        }
    }
    pub fn uvlc(&mut self, v: u32) {
        let x = v as u64 + 1;
        let lz = 63 - x.leading_zeros();
        self.put(lz, 0);
        self.put(lz + 1, x);
    }
}

#[test]
fn sha256_kat() {
    // FIPS 180-4 example "abc"
    assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
}
