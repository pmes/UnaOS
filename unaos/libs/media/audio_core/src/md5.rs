//! MD5 (RFC 1321) — FLAC's STREAMINFO carries the MD5 of the decoded PCM, and recomputing it over our own
//! output is the decoder's correctness proof (RFC 9639 §8.2). Not used for anything security-related.

pub struct Md5 {
    s: [u32; 4],
    buf: [u8; 64],
    n: usize,
    len: u64,
}

const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];
const R: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];

impl Default for Md5 {
    fn default() -> Self { Self::new() }
}

impl Md5 {
    pub fn new() -> Md5 { Md5 { s: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476], buf: [0; 64], n: 0, len: 0 } }

    fn block(s: &mut [u32; 4], b: &[u8]) {
        let mut m = [0u32; 16];
        for i in 0..16 { m[i] = u32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]); }
        let (mut a, mut bb, mut c, mut d) = (s[0], s[1], s[2], s[3]);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((bb & c) | (!bb & d), i),
                1 => ((d & bb) | (!d & c), (5 * i + 1) % 16),
                2 => (bb ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (bb | !d), (7 * i) % 16),
            };
            let t = d;
            d = c;
            c = bb;
            bb = bb.wrapping_add(a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]).rotate_left(R[(i / 16) * 4 + i % 4]));
            a = t;
        }
        s[0] = s[0].wrapping_add(a);
        s[1] = s[1].wrapping_add(bb);
        s[2] = s[2].wrapping_add(c);
        s[3] = s[3].wrapping_add(d);
    }

    pub fn update(&mut self, mut d: &[u8]) {
        self.len += d.len() as u64;
        if self.n > 0 {
            let k = (64 - self.n).min(d.len());
            self.buf[self.n..self.n + k].copy_from_slice(&d[..k]);
            self.n += k;
            d = &d[k..];
            if self.n < 64 { return; }
            let b = self.buf;
            Self::block(&mut self.s, &b);
            self.n = 0;
        }
        while d.len() >= 64 {
            Self::block(&mut self.s, &d[..64]);
            d = &d[64..];
        }
        self.buf[..d.len()].copy_from_slice(d);
        self.n = d.len();
    }

    pub fn finish(mut self) -> [u8; 16] {
        let bits = self.len.wrapping_mul(8);
        self.update(&[0x80]);
        while self.n != 56 { self.update(&[0]); }
        self.update(&bits.to_le_bytes());
        let mut o = [0u8; 16];
        for i in 0..4 { o[4 * i..4 * i + 4].copy_from_slice(&self.s[i].to_le_bytes()); }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hex(d: [u8; 16]) -> alloc::string::String { d.iter().map(|b| alloc::format!("{:02x}", b)).collect() }
    #[test]
    fn rfc1321_suite() {
        // RFC 1321 Appendix A.5.
        for (m, h) in [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            ("abcdefghijklmnopqrstuvwxyz", "c3fcd3d76192e4007dfb496cca67e13b"),
            ("12345678901234567890123456789012345678901234567890123456789012345678901234567890", "57edf4a22be3c955ac49da2e2107b67a"),
        ] {
            let mut x = Md5::new();
            for c in m.as_bytes().chunks(7) { x.update(c); }
            assert_eq!(hex(x.finish()), h, "md5({:?})", m);
        }
    }
}
