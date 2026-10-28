// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDFJPX (LEDGER SR335) KATs: JPXDecode through `pdf_core::filters::jpx`.
//!
//! Known answers from outside this crate: codestreams and JP2 files written by OpenJPEG 2.5.0 through ImageMagick
//! 6.9.12-98 from formula images (`grey12` / `rgb12` below, written as PGM / PPM), every one lossless (5/3, RCT),
//! pinned as hex and decoded here bit-exact to the formula:
//!   convert g.pgm -define jp2:number-resolutions=3 g.j2k                                  (OPJ_GREY_12)
//!   convert c.ppm -define jp2:number-resolutions=2 c.jp2                                  (OPJ_RGB_12_JP2)
//!   convert c.ppm -define jp2:progression-order=RPCL -define jp2:quality-layers=3 \
//!                 -define jp2:quality=25,35,0 cr.j2k                                      (OPJ_RGB_12_RPCL)
//!   convert c.ppm -define jp2:progression-order=RLCP -define jp2:quality-layers=2 \
//!                 -define jp2:quality=30,0 cl.j2k                                         (OPJ_RGB_12_RLCP)
//! ImageMagick drives OpenJPEG reversibly only (no 9/7, tiles or code-block style flags): those pieces are pinned by
//! round trips through test-side ENCODERS written from the encoder side of T.800 (the forward 5/3 and 9/7 lifting of
//! F.4, the quantiser of E.1, the EBCOT coding passes of Annex D over the shared MQ encoder `common::mq::MqEnc`, tag
//! trees and packet headers of B.10, the markers of Annex A), never the decoder's code.

mod common;

use common::mq::MqEnc;
use pdf_core::Error;
use pdf_core::filters::jpx::{self, Image, Space};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// The 12 x 12 grey formula image fed to ImageMagick.
fn grey12() -> Vec<u8> {
    (0..12usize).flat_map(|y| (0..12usize).map(move |x| ((x * 21 + y * 7 + (x * y) % 5 * 9) & 255) as u8)).collect()
}

/// The 12 x 12 RGB formula image fed to ImageMagick.
fn rgb12() -> Vec<u8> {
    (0..12usize)
        .flat_map(|y| (0..12usize).flat_map(move |x| [((x * 20 + 3) & 255) as u8, ((y * 19 + 5) & 255) as u8, (((x ^ y) * 17 + x * y) & 255) as u8]))
        .collect()
}

// ---------------------------------------------------------------------------------------------------------------
// OpenJPEG 2.5.0 known answers

const OPJ_GREY_12: &str = concat!(
    "ff4fff51002900000000000c0000000c00000000000000000000000c0000000c00000000000000000001070101ff5200",
    "0c00000001000204040001ff5c000a4040484850484850ff640025000143726561746564206279204f70656e4a504547",
    "2076657273696f6e20322e352e30ff90000a0000000000b70001ff93df8060069c7ac9688fc2ed03fa357fc7da151f68",
    "4c3ed0a012e0a5b03fe812a0981b1a7a88c95c02e02467124b0bca90fc859f5abecfc09a7e0471f81100299ce8c0ad4b",
    "c3cf25981463f563d4d6f126f838fd42a8e787312735137dd08b492e18f1f33f610ddfd67f0ec229e09c959e6bd4f5bb",
    "c451c26da2aed74fce549f15a3d4639f205caf2c6441185a2f312dee0f745f7b53350330407bb45781bb9fac1b342f03",
    "ce6b746483ffd9",
);
const OPJ_RGB_12_JP2: &str = concat!(
    "0000000c6a5020200d0a870a00000014667479706a703220000000006a7032200000002d6a7032680000001669686472",
    "0000000c0000000c0003070700000000000f636f6c7201000000000010000001d66a703263ff4fff51002f0000000000",
    "0c0000000c00000000000000000000000c0000000c00000000000000000003070101070101070101ff52000c00000001",
    "010104040001ff5c00074040484850ff640025000143726561746564206279204f70656e4a5045472076657273696f6e",
    "20322e352e30ff90000a00000000015b0001ff93cfb488111dc22ff20d640c23c465bcd1d72aad7cb0b2634cde07f9a5",
    "a33635f2e4d4caf692df814826290f9cf7c25076cae796492e80a97395d17b35e317deb488187a9ebd43fe28f533cfef",
    "dc4d8403bfdf812027734777b474073b9340d98cfb355a13a6dd2c433b2e2045324304cd57f9444875e8268fc1f39a83",
    "e7370fb46c2926eb53027b43b723bad6181558907cf6d0bf9c903a0347e8dd2adf1a1871acf9ca92216f2f496dc37cb8",
    "4633aec000b66fa67b8728b04e2061fbd05841efbb553e86a741e98118daab159dfa90b782c7da431f6904fcc8802926",
    "eb53027b4047a69975cdce74468f85e1500097b920e498f3737dd3746e591f2adf1a1871caa725e742ce189b878501ff",
    "50d355d1116db3e9859af8d9ed26db28b04e2061fbd05841efbb553e86a741ee0270bffb79f97995dbf19997c313a682",
    "91c1f38583e70a2c5fcfc28b37a15383afffd9",
);
const OPJ_RGB_12_RPCL: &str = concat!(
    "ff4fff51002f00000000000c0000000c00000000000000000000000c0000000c00000000000000000003070101070101",
    "070101ff52000c00020003010304040001ff5c000d4040484850484850484850ff640025000143726561746564206279",
    "204f70656e4a5045472076657273696f6e20322e352e30ff90000a0000000001c30001ff93df404007ddf03dfc20805f",
    "c0df40500d9337fe9a80fc40405fdf30400bb058f0fc604005c080c070503e00a07c6080085f05e106bff40e81d000c1",
    "c141f18287d2080826062306df80fc807a07407f8080c07c21c07c210008ff7f01bfc1143e014015260cca8247e7f84e",
    "0a0f80503bc8f3944d120799ddc28ffc217c217c80c0fcef0433d11dadc7859f08b1f00a14f31d843414d0fa44540799",
    "ddc305bc1842d7ccfd017e407f5028941957d72364915785179a2f1fc228706015920e3e1780fd20be7040cdd7e97f90",
    "e10028b04e20c1e3e0e97f09402926eb53027b43b723bad6181558902adf1a1871acf9ca92216f61fbd05841efbb553e",
    "86fc42fe321f18d07cf6d0bf9c903a0347e8dd2f496dc37cb84633aec000b66fa67b87a741e98118daab159dfa90b782",
    "c74a8e953e11802926eb53027b4047a6992adf1a1871caa725e74228b04e2061fbe36374b075cdce74468fce189b8785",
    "01d05841efbb553e86a741eefce47e721f411085e1500097b920e498f3737dd3746e591fff50d355d1116db3e9859af8",
    "d9ed26db0270bffb79f97995dbf19997c313a6829180c1c1c1c1802c5fcf37a153fc80be4040c28b83afffd9",
);
const OPJ_RGB_12_RLCP: &str = concat!(
    "ff4fff51002f00000000000c0000000c00000000000000000000000c0000000c00000000000000000003070101070101",
    "070101ff52000c00010002010304040001ff5c000d4040484850484850484850ff640025000143726561746564206279",
    "204f70656e4a5045472076657273696f6e20322e352e30ff90000a0000000001a90001ff93df404007ddf03ddf40500d",
    "9337fe9adf78280bb058f005fc40405ffc40405fc0c070503e00a07c6080085f05e106bfc1c141f18287d20808260623",
    "06df80f40e81d000fc807a07407fc07c21c07c210008ff7f01bfc1e161f08d07c02815263bc8f30cca8247e74d0799dd",
    "c28fc7859f1068f80514f31d843414d0fa4454420799ddc305c228706015920e3e17fc40fe307f203094fcef120433d1",
    "1dadfd017e80bf5028941957d723d7cc64915785179a2f1ffd20be7040cdd7e97fc18f074b87082926eb53027b432adf",
    "1a1871acf9ca92216f28b04e20c78e8e953e41c02926eb53027b4047a69975cdce742adf1a1871caa725e74228b04e20",
    "61fbd05841efbb553e8680fca4fe321f50b8b723bad6181558907cf6d0bf9c903a0347e8dd2f496dc37cb84633aec000",
    "b66fa67b8761fbd05841efbb553e86a741e98118daab159dfa90b782fd04fe92df4940468f85e1500097b920e498f373",
    "7dd3746e591fce189b878501ff50d355d1116db3e9859af8d9ed26dba741ee0270bffb79f97995dbf19997c313a68291",
    "c1f38583e70a2c5fcfc28b37a15383afffd9",
);

#[test]
fn openjpeg_lossless_known_answers() {
    let g = jpx::decode(&unhex(OPJ_GREY_12)).unwrap();
    assert_eq!((g.w, g.h, g.comps, g.space), (12, 12, 1, Space::Unknown));
    assert_eq!(g.data, grey12(), "grey, 3 resolutions");
    let j = jpx::decode(&unhex(OPJ_RGB_12_JP2)).unwrap();
    assert_eq!((j.w, j.h, j.comps, j.space), (12, 12, 3, Space::Rgb), "JP2 colr enumerated sRGB");
    assert_eq!(j.data, rgb12(), "RGB JP2, RCT, 2 resolutions");
    for (name, hex) in [("RPCL 3 layers", OPJ_RGB_12_RPCL), ("RLCP 2 layers", OPJ_RGB_12_RLCP)] {
        let i = jpx::decode(&unhex(hex)).unwrap();
        assert_eq!((i.w, i.h, i.comps), (12, 12, 3), "{name}");
        assert_eq!(i.data, rgb12(), "{name}");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The test-side encoder: one layer, LRCP, one precinct, each band one code-block (images up to 64 x 64)

#[derive(Clone, Copy)]
struct Enc {
    w: usize,
    h: usize,
    comps: usize,
    levels: usize,
    reversible: bool,
    mct: bool,
    style: u8,
    tw: usize,
    th: usize,
    sop_eph: bool,
}

impl Enc {
    fn new(w: usize, h: usize, comps: usize, levels: usize) -> Enc {
        Enc { w, h, comps, levels, reversible: true, mct: false, style: 0, tw: w, th: h, sop_eph: false }
    }
}

const GUARD: u32 = 3;

/// The subband gain of Table E.1 (log2): LL 0, HL / LH 1, HH 2.
fn gain(kind: usize) -> u32 {
    [0, 1, 1, 2][kind]
}

/// The exponent written for band `kind`: 9/7 steps of 1/8 (R_b - eps_b = -3, mantissa 0).
fn eps_of(e: &Enc, kind: usize) -> u32 {
    8 + gain(kind) + if e.reversible { 1 } else { 3 }
}

fn mb_of(e: &Enc, kind: usize) -> u32 {
    GUARD + eps_of(e, kind) - 1
}

fn mirror_e(i: isize, n: usize) -> usize {
    let p = 2 * (n as isize - 1);
    let j = i.rem_euclid(p);
    (if j >= n as isize { p - j } else { j }) as usize
}

/// 1D_SD (F.4.8) over `x`, whose first sample sits at global index `i0`.
fn sd1d(x: &mut [f64], i0: usize, rev: bool) {
    let n = x.len();
    if n == 1 {
        if i0 % 2 == 1 {
            x[0] *= 2.0;
        }
        return;
    }
    let first_odd = if i0 % 2 == 0 { 1 } else { 0 };
    let first_even = 1 - first_odd;
    let around = |x: &[f64], i: usize| x[mirror_e(i as isize - 1, n)] + x[mirror_e(i as isize + 1, n)];
    let step = |x: &mut [f64], start: usize, f: &dyn Fn(f64, f64) -> f64| {
        let mut i = start;
        while i < n {
            let a = around(x, i);
            x[i] = f(x[i], a);
            i += 2;
        }
    };
    if rev {
        // F-? (5/3): Y(2n+1) = X(2n+1) - floor((X(2n) + X(2n+2)) / 2); Y(2n) = X(2n) + floor((Y(2n-1) + Y(2n+1) + 2) / 4).
        step(x, first_odd, &|v, a| v - (a / 2.0).floor());
        step(x, first_even, &|v, a| v + ((a + 2.0) / 4.0).floor());
    } else {
        // Table F.7 (9/7): four lifting steps, then the scaling.
        const A: f64 = -1.586_134_342_059_924;
        const B: f64 = -0.052_980_118_572_961;
        const G: f64 = 0.882_911_075_530_934;
        const D: f64 = 0.443_506_852_043_971;
        const KK: f64 = 1.230_174_104_914_001;
        step(x, first_odd, &|v, a| v + A * a);
        step(x, first_even, &|v, a| v + B * a);
        step(x, first_odd, &|v, a| v + G * a);
        step(x, first_even, &|v, a| v + D * a);
        step(x, first_odd, &|v, _| v * KK);
        step(x, first_even, &|v, _| v / KK);
    }
}

struct EBand {
    kind: usize,
    w: usize,
    h: usize,
    data: Vec<f64>,
}

/// The forward DWT (F.4: VER_SD then HOR_SD, then the four bands): bands per resolution, r = 0 first.
fn fdwt(mut cur: Vec<f64>, rect: (usize, usize, usize, usize), levels: usize, rev: bool) -> Vec<Vec<EBand>> {
    let (mut x0, mut y0, mut x1, mut y1) = rect;
    let mut high = Vec::new();
    for _ in 0..levels {
        let (w, h) = (x1 - x0, y1 - y0);
        for u in 0..w {
            let mut col: Vec<f64> = (0..h).map(|v| cur[v * w + u]).collect();
            sd1d(&mut col, y0, rev);
            for v in 0..h {
                cur[v * w + u] = col[v];
            }
        }
        for v in 0..h {
            sd1d(&mut cur[v * w..(v + 1) * w], x0, rev);
        }
        let (lx0, lx1, hx0, hx1) = (x0.div_ceil(2), x1.div_ceil(2), x0 / 2, x1 / 2);
        let (ly0, ly1, hy0, hy1) = (y0.div_ceil(2), y1.div_ceil(2), y0 / 2, y1 / 2);
        let take = |kind: usize, bx0: usize, bx1: usize, by0: usize, by1: usize, px: usize, py: usize| {
            let (bw, bh) = (bx1 - bx0, by1 - by0);
            let mut data = vec![0.0; bw * bh];
            for gy in y0..y1 {
                for gx in x0..x1 {
                    if gx % 2 == px && gy % 2 == py {
                        data[(gy / 2 - by0) * bw + gx / 2 - bx0] = cur[(gy - y0) * w + gx - x0];
                    }
                }
            }
            EBand { kind, w: bw, h: bh, data }
        };
        let ll = take(0, lx0, lx1, ly0, ly1, 0, 0);
        let hl = take(1, hx0, hx1, ly0, ly1, 1, 0);
        let lh = take(2, lx0, lx1, hy0, hy1, 0, 1);
        let hh = take(3, hx0, hx1, hy0, hy1, 1, 1);
        high.push(vec![hl, lh, hh]);
        cur = ll.data;
        (x0, y0, x1, y1) = (lx0, ly0, lx1, ly1);
    }
    let mut out = vec![vec![EBand { kind: 0, w: x1 - x0, h: y1 - y0, data: cur }]];
    out.extend(high.into_iter().rev());
    out
}

/// The raw (bypass) bit writer (D.6): MSB first; after a 0xFF byte the next carries 7 bits under a 0 MSB.
struct RawW {
    out: Vec<u8>,
    cur: u8,
    n: u32,
    limit: u32,
}

impl RawW {
    fn new() -> RawW {
        RawW { out: Vec::new(), cur: 0, n: 0, limit: 8 }
    }
    fn put(&mut self, b: u32) {
        self.cur = self.cur << 1 | b as u8;
        self.n += 1;
        if self.n == self.limit {
            self.out.push(self.cur);
            self.limit = if self.cur == 0xFF { 7 } else { 8 };
            self.cur = 0;
            self.n = 0;
        }
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.cur << (self.limit - self.n));
        }
        self.out
    }
}

enum CoderE {
    Mq(MqEnc),
    Raw(RawW),
}

impl CoderE {
    fn put(&mut self, cx: &mut u8, d: u32) {
        match self {
            CoderE::Mq(m) => m.encode(cx, d as u8),
            CoderE::Raw(r) => r.put(d),
        }
    }
    /// The codeword segment: MQ code without JBIG2's trailing 0xFF 0xAC (C.2.9 drops a last 0xFF), or the raw bits.
    fn finish(self) -> Vec<u8> {
        match self {
            CoderE::Mq(m) => {
                let mut v = m.flush();
                assert_eq!(&v[v.len() - 2..], &[0xFF, 0xAC]);
                v.truncate(v.len() - 2);
                v
            }
            CoderE::Raw(r) => r.finish(),
        }
    }
}

/// Table D.9's segment of pass `k`: TERMALL ends every pass; BYPASS ends pass 9, then each raw SP + MR pair and each
/// MQ cleanup.
fn seg_of(style: u8, k: u32) -> u32 {
    match (style & 0x04 != 0, style & 0x01 != 0) {
        (true, _) => k,
        (false, true) if k >= 10 => {
            let (plane, within) = ((k - 10) / 3, (k - 10) % 3);
            1 + 2 * plane + if within == 2 { 1 } else { 0 }
        }
        _ => 0,
    }
}

fn is_raw(style: u8, k: u32) -> bool {
    style & 0x01 != 0 && k >= 10 && (k - 10) % 3 != 2
}

/// The EBCOT coder for one code-block (Annex D, from the encoder's side).
struct T1E<'a> {
    w: usize,
    h: usize,
    q: &'a [i32],
    kind: usize,
    sig: Vec<bool>,
    refined: Vec<bool>,
    visited: Vec<bool>,
    cx: [u8; 19],
}

impl T1E<'_> {
    fn reset(&mut self) {
        self.cx = [0; 19];
        self.cx[0] = 4 << 1; // Table D.7: the all-zero ZC context starts in state 4,
        self.cx[17] = 3 << 1; // run-length in state 3,
        self.cx[18] = 46 << 1; // UNIFORM in state 46.
    }
    fn s(&self, x: isize, y: isize) -> u32 {
        (x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h && self.sig[y as usize * self.w + x as usize]) as u32
    }
    /// The sign contribution of a neighbour: +1 significant positive, -1 significant negative.
    fn chi(&self, x: isize, y: isize) -> i32 {
        if self.s(x, y) == 0 { 0 } else if self.q[y as usize * self.w + x as usize] < 0 { -1 } else { 1 }
    }
    fn hvd(&self, x: usize, y: usize) -> (u32, u32, u32) {
        let (x, y) = (x as isize, y as isize);
        (
            self.s(x - 1, y) + self.s(x + 1, y),
            self.s(x, y - 1) + self.s(x, y + 1),
            self.s(x - 1, y - 1) + self.s(x + 1, y - 1) + self.s(x - 1, y + 1) + self.s(x + 1, y + 1),
        )
    }
    /// Table D.1.
    fn zc(&self, x: usize, y: usize) -> usize {
        let (h, v, d) = self.hvd(x, y);
        if self.kind == 3 {
            let hv = h + v;
            return if d >= 3 {
                8
            } else if d == 2 {
                if hv >= 1 { 7 } else { 6 }
            } else if d == 1 {
                [3, 4, 5][hv.min(2) as usize]
            } else {
                [0, 1, 2][hv.min(2) as usize]
            };
        }
        // LL and LH key on the horizontal count; HL swaps the roles.
        let (a, b) = if self.kind == 1 { (v, h) } else { (h, v) };
        if a == 2 {
            8
        } else if a == 1 {
            if b >= 1 { 7 } else if d >= 1 { 6 } else { 5 }
        } else if b >= 1 {
            2 + b as usize
        } else {
            d.min(2) as usize
        }
    }
    fn code_sign(&mut self, c: &mut CoderE, x: usize, y: usize) {
        let neg = (self.q[y * self.w + x] < 0) as u32;
        if let CoderE::Raw(r) = c {
            r.put(neg);
        } else {
            let (xi, yi) = (x as isize, y as isize);
            let hh = (self.chi(xi - 1, yi) + self.chi(xi + 1, yi)).clamp(-1, 1);
            let vv = (self.chi(xi, yi - 1) + self.chi(xi, yi + 1)).clamp(-1, 1);
            // Table D.3, rows H = -1, 0, 1 by columns V = -1, 0, 1: (context, XOR bit).
            const T: [(usize, u32); 9] = [(13, 1), (12, 1), (11, 1), (10, 1), (9, 0), (10, 0), (11, 0), (12, 0), (13, 0)];
            let (ctx, xor) = T[((hh + 1) * 3 + vv + 1) as usize];
            c.put(&mut self.cx[ctx], neg ^ xor);
        }
        self.sig[y * self.w + x] = true;
    }
    fn bit(&self, x: usize, y: usize, p: u32) -> u32 {
        (self.q[y * self.w + x].unsigned_abs() >> p) & 1
    }
    fn stripes(&self) -> Vec<(usize, usize)> {
        let mut v = Vec::new();
        for y0 in (0..self.h).step_by(4) {
            for x in 0..self.w {
                for y in y0..(y0 + 4).min(self.h) {
                    v.push((x, y));
                }
            }
        }
        v
    }
    fn sp(&mut self, c: &mut CoderE, p: u32) {
        for (x, y) in self.stripes() {
            let i = y * self.w + x;
            if self.sig[i] {
                continue;
            }
            let ctx = self.zc(x, y);
            if ctx == 0 {
                continue;
            }
            let b = self.bit(x, y, p);
            c.put(&mut self.cx[ctx], b);
            if b == 1 {
                self.code_sign(c, x, y);
            }
            self.visited[i] = true;
        }
    }
    fn mr(&mut self, c: &mut CoderE, p: u32) {
        for (x, y) in self.stripes() {
            let i = y * self.w + x;
            if !self.sig[i] || self.visited[i] {
                continue;
            }
            let (h, v, d) = self.hvd(x, y);
            let ctx = if self.refined[i] { 16 } else if h + v + d == 0 { 14 } else { 15 };
            let b = self.bit(x, y, p);
            c.put(&mut self.cx[ctx], b);
            self.refined[i] = true;
        }
    }
    fn cu(&mut self, c: &mut CoderE, p: u32) {
        for y0 in (0..self.h).step_by(4) {
            let y1 = (y0 + 4).min(self.h);
            for x in 0..self.w {
                let mut y = y0;
                let quiet = |t: &Self, yy: usize| !t.sig[yy * t.w + x] && !t.visited[yy * t.w + x] && t.zc(x, yy) == 0;
                if y1 - y0 == 4 && (y0..y1).all(|yy| quiet(self, yy)) {
                    match (y0..y1).find(|&yy| self.bit(x, yy, p) == 1) {
                        None => {
                            c.put(&mut self.cx[17], 0);
                            continue;
                        }
                        Some(first) => {
                            c.put(&mut self.cx[17], 1);
                            let r = (first - y0) as u32;
                            c.put(&mut self.cx[18], r >> 1);
                            c.put(&mut self.cx[18], r & 1);
                            self.code_sign(c, x, first);
                            y = first + 1;
                        }
                    }
                }
                for yy in y..y1 {
                    let i = yy * self.w + x;
                    if self.sig[i] || self.visited[i] {
                        continue;
                    }
                    let ctx = self.zc(x, yy);
                    let b = self.bit(x, yy, p);
                    c.put(&mut self.cx[ctx], b);
                    if b == 1 {
                        self.code_sign(c, x, yy);
                    }
                }
            }
        }
        self.visited.iter_mut().for_each(|v| *v = false);
    }
}

/// A coded block: zero bit-planes, passes, and its codeword segments (passes in each, bytes).
struct Coded {
    zbp: u32,
    passes: u32,
    segs: Vec<(u32, Vec<u8>)>,
}

fn t1_encode(q: &[i32], w: usize, h: usize, kind: usize, mb: u32, style: u8) -> Option<Coded> {
    let mx = q.iter().map(|v| v.unsigned_abs()).max().unwrap_or(0);
    if mx == 0 {
        return None;
    }
    let planes = 32 - mx.leading_zeros();
    assert!(planes <= mb, "Mb {mb} too small for {planes} planes");
    let mut t = T1E { w, h, q, kind, sig: vec![false; w * h], refined: vec![false; w * h], visited: vec![false; w * h], cx: [0; 19] };
    t.reset();
    let npass = 3 * planes - 2;
    let mut segs = Vec::new();
    let mut open: Option<(u32, u32, CoderE)> = None;
    for k in 0..npass {
        let seg = seg_of(style, k);
        if open.as_ref().map(|o| o.0) != Some(seg) {
            if let Some((_, n, c)) = open.take() {
                segs.push((n, c.finish()));
            }
            open = Some((seg, 0, if is_raw(style, k) { CoderE::Raw(RawW::new()) } else { CoderE::Mq(MqEnc::new()) }));
        }
        let (_, n, c) = open.as_mut().unwrap();
        *n += 1;
        // Pass 0 is the cleanup of the most significant plane; then SP, MR, CU per plane below it.
        let (p, kind_of_pass) = if k == 0 { (planes - 1, 2) } else { (planes - 1 - (k + 2) / 3, (k - 1) % 3) };
        match kind_of_pass {
            0 => t.sp(c, p),
            1 => t.mr(c, p),
            _ => {
                t.cu(c, p);
                if style & 0x20 != 0 {
                    for b in [1, 0, 1, 0] {
                        c.put(&mut t.cx[18], b);
                    }
                }
            }
        }
        if style & 0x02 != 0 {
            t.reset();
        }
    }
    let (_, n, c) = open.unwrap();
    segs.push((n, c.finish()));
    Some(Coded { zbp: mb - planes, passes: npass, segs })
}

/// The packet-header bit writer (B.10.1): after a 0xFF byte the next carries 7 bits; never ends on 0xFF.
struct HdrW {
    out: Vec<u8>,
    cur: u8,
    n: u32,
    limit: u32,
}

impl HdrW {
    fn new() -> HdrW {
        HdrW { out: Vec::new(), cur: 0, n: 0, limit: 8 }
    }
    fn put(&mut self, b: u32) {
        self.cur = self.cur << 1 | b as u8;
        self.n += 1;
        if self.n == self.limit {
            self.out.push(self.cur);
            self.limit = if self.cur == 0xFF { 7 } else { 8 };
            self.cur = 0;
            self.n = 0;
        }
    }
    fn put_n(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.put(v >> i & 1);
        }
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.cur << (self.limit - self.n));
        }
        if self.out.last() == Some(&0xFF) {
            self.out.push(0);
        }
        self.out
    }
}

/// Table B.4.
fn put_passes(b: &mut HdrW, n: u32) {
    match n {
        1 => b.put(0),
        2 => b.put_n(0b10, 2),
        3..=5 => b.put_n(0b1100 | (n - 3), 4),
        6..=36 => b.put_n(0b1111_00000 | (n - 6), 9),
        _ => b.put_n((0b1_1111_1111 << 7) | (n - 37), 16),
    }
}

fn floor_log2(n: u32) -> u32 {
    31 - n.leading_zeros()
}

/// One packet: each block a single-leaf tag tree (B.10.2: a value v is v zeros then a one).
fn packet(blocks: &[Option<Coded>], sop: Option<u16>, eph: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(n) = sop {
        out.extend_from_slice(&[0xFF, 0x91, 0, 4]);
        out.extend_from_slice(&n.to_be_bytes());
    }
    let mut b = HdrW::new();
    if blocks.iter().all(Option::is_none) {
        b.put(0);
    } else {
        b.put(1);
        for blk in blocks {
            let Some(c) = blk else {
                b.put(0);
                continue;
            };
            b.put(1);
            for _ in 0..c.zbp {
                b.put(0);
            }
            b.put(1);
            put_passes(&mut b, c.passes);
            let need = |s: &(u32, Vec<u8>)| (32 - (s.1.len() as u32).leading_zeros()).saturating_sub(floor_log2(s.0));
            let lblock = c.segs.iter().map(need).max().unwrap().max(3);
            for _ in 3..lblock {
                b.put(1);
            }
            b.put(0);
            for s in &c.segs {
                b.put_n(s.1.len() as u32, lblock + floor_log2(s.0));
            }
        }
    }
    out.extend(b.finish());
    if eph {
        out.extend_from_slice(&[0xFF, 0x92]);
    }
    for c in blocks.iter().flatten() {
        for s in &c.segs {
            out.extend_from_slice(&s.1);
        }
    }
    out
}

fn marker(out: &mut Vec<u8>, m: u16, body: &[u8]) {
    out.extend_from_slice(&m.to_be_bytes());
    out.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
    out.extend_from_slice(body);
}

/// Encode an 8-bit image (`e.comps` interleaved samples per pixel) as a raw codestream.
fn encode(img: &[u8], e: &Enc) -> Vec<u8> {
    let mut cs = vec![0xFF, 0x4F];
    let mut siz = vec![0, 0];
    for v in [e.w, e.h, 0, 0, e.tw, e.th, 0, 0] {
        siz.extend_from_slice(&(v as u32).to_be_bytes());
    }
    siz.extend_from_slice(&(e.comps as u16).to_be_bytes());
    for _ in 0..e.comps {
        siz.extend_from_slice(&[7, 1, 1]);
    }
    marker(&mut cs, 0xFF51, &siz);
    let scod = if e.sop_eph { 0x06 } else { 0 };
    let cod = [scod, 0, 0, 1, e.mct as u8, e.levels as u8, 4, 4, e.style, e.reversible as u8];
    marker(&mut cs, 0xFF52, &cod);
    let kinds: Vec<usize> = std::iter::once(0).chain((0..e.levels).flat_map(|_| [1, 2, 3])).collect();
    let mut qcd = vec![(GUARD as u8) << 5 | if e.reversible { 0 } else { 2 }];
    for &k in &kinds {
        if e.reversible {
            qcd.push((eps_of(e, k) as u8) << 3);
        } else {
            qcd.extend_from_slice(&((eps_of(e, k) as u16) << 11).to_be_bytes());
        }
    }
    marker(&mut cs, 0xFF5C, &qcd);
    let (ntx, nty) = (e.w.div_ceil(e.tw), e.h.div_ceil(e.th));
    let mut nsop = 0u16;
    for t in 0..ntx * nty {
        let (tx0, ty0) = ((t % ntx) * e.tw, (t / ntx) * e.th);
        let (tx1, ty1) = ((tx0 + e.tw).min(e.w), (ty0 + e.th).min(e.h));
        let (w, h) = (tx1 - tx0, ty1 - ty0);
        // DC level shift, then the forward component transform (Annex G).
        let mut planes: Vec<Vec<f64>> = (0..e.comps)
            .map(|c| (0..w * h).map(|i| img[((ty0 + i / w) * e.w + tx0 + i % w) * e.comps + c] as f64 - 128.0).collect())
            .collect();
        if e.mct {
            for i in 0..w * h {
                let (r, g, b) = (planes[0][i], planes[1][i], planes[2][i]);
                let y = if e.reversible {
                    [((r + 2.0 * g + b) / 4.0).floor(), b - g, r - g]
                } else {
                    [
                        0.299 * r + 0.587 * g + 0.114 * b,
                        -0.168_75 * r - 0.331_26 * g + 0.5 * b,
                        0.5 * r - 0.418_69 * g - 0.081_31 * b,
                    ]
                };
                for c in 0..3 {
                    planes[c][i] = y[c];
                }
            }
        }
        let bands: Vec<Vec<Vec<EBand>>> =
            planes.into_iter().map(|p| fdwt(p, (tx0, ty0, tx1, ty1), e.levels, e.reversible)).collect();
        let mut data = Vec::new();
        for r in 0..=e.levels {
            for comp in &bands {
                let mut blocks = Vec::new();
                for b in &comp[r] {
                    if b.w == 0 || b.h == 0 {
                        continue;
                    }
                    let delta = 0.125;
                    let q: Vec<i32> = b
                        .data
                        .iter()
                        .map(|&v| if e.reversible { v as i32 } else { (v.signum() * (v.abs() / delta).floor()) as i32 })
                        .collect();
                    blocks.push(t1_encode(&q, b.w, b.h, b.kind, mb_of(e, b.kind), e.style));
                }
                data.extend(packet(&blocks, e.sop_eph.then_some(nsop), e.sop_eph));
                nsop = nsop.wrapping_add(1);
            }
        }
        cs.extend_from_slice(&[0xFF, 0x90, 0, 10]);
        cs.extend_from_slice(&(t as u16).to_be_bytes());
        cs.extend_from_slice(&(14 + data.len() as u32).to_be_bytes());
        cs.extend_from_slice(&[0, 1, 0xFF, 0x93]);
        cs.extend(data);
    }
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = (8 + body.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// A JP2 file around `cs`: signature, ftyp, jp2h (ihdr, colr METH 1 `enumcs`, then `extra` boxes), jp2c.
fn jp2(cs: &[u8], w: u32, h: u32, nc: u16, enumcs: u32, extra: &[u8]) -> Vec<u8> {
    let mut f = vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];
    f.extend(bx(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    let mut ihdr = h.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&nc.to_be_bytes());
    ihdr.extend_from_slice(&[7, 7, 0, 0]);
    let mut colr = vec![1, 0, 0];
    colr.extend_from_slice(&enumcs.to_be_bytes());
    let mut h2 = bx(b"ihdr", &ihdr);
    h2.extend(bx(b"colr", &colr));
    h2.extend_from_slice(extra);
    f.extend(bx(b"jp2h", &h2));
    f.extend(bx(b"jp2c", cs));
    f
}

fn img(w: usize, h: usize, n: usize, f: impl Fn(usize, usize, usize) -> usize) -> Vec<u8> {
    (0..h).flat_map(|y| (0..w).flat_map(move |x| (0..n).map(move |c| (x, y, c)))).map(|(x, y, c)| (f(x, y, c) & 255) as u8).collect()
}

fn decoded(cs: &[u8]) -> Image {
    jpx::decode(cs).unwrap()
}

// ---------------------------------------------------------------------------------------------------------------
// Round trips through the test-side encoder

#[test]
fn one_bitplane_8x8_reversible() {
    // Samples 127 / 128 / 129: with no decomposition the coefficients are -1 / 0 / 1, ONE bit-plane, ONE pass.
    let src = img(8, 8, 1, |x, y, _| [128, 129, 127][(x * 3 + y * 5 + x * y) % 3]);
    let e = Enc::new(8, 8, 1, 0);
    let q: Vec<i32> = src.iter().map(|&v| v as i32 - 128).collect();
    let c = t1_encode(&q, 8, 8, 0, mb_of(&e, 0), 0).unwrap();
    assert_eq!((c.passes, c.zbp, c.segs.len()), (1, mb_of(&e, 0) - 1, 1));
    let d = decoded(&encode(&src, &e));
    assert_eq!((d.w, d.h, d.comps), (8, 8, 1));
    assert_eq!(d.data, src);
}

#[test]
fn irreversible_97() {
    let src = img(16, 16, 1, |x, y, _| 40 + x * 9 + y * 5 + (x * y) % 7 * 3);
    let mut e = Enc::new(16, 16, 1, 2);
    e.reversible = false;
    let cs = encode(&src, &e);
    assert_eq!(cs[find(&cs, 0xFF52) + 13], 0, "COD says 9/7");
    let d = decoded(&cs);
    for (i, (&a, &b)) in d.data.iter().zip(&src).enumerate() {
        assert!((a as i32 - b as i32).abs() <= 1, "sample {i}: {a} vs {b}");
    }
}

#[test]
fn rct_three_components() {
    let src = img(8, 8, 3, |x, y, c| match c {
        0 => x * 31 + y * 7,
        1 => y * 29 + x * x * 3,
        _ => (x ^ y) * 33 + 5,
    });
    let mut e = Enc::new(8, 8, 3, 1);
    e.mct = true;
    let d = decoded(&encode(&src, &e));
    assert_eq!((d.comps, d.space), (3, Space::Unknown));
    assert_eq!(d.data, src);
}

#[test]
fn ict_three_components() {
    let src = img(8, 8, 3, |x, y, c| 30 + x * 20 + y * 3 + c * 25);
    let mut e = Enc::new(8, 8, 3, 1);
    (e.mct, e.reversible) = (true, false);
    let d = decoded(&encode(&src, &e));
    for (i, (&a, &b)) in d.data.iter().zip(&src).enumerate() {
        assert!((a as i32 - b as i32).abs() <= 1, "sample {i}: {a} vs {b}");
    }
}

#[test]
fn two_tiles() {
    // 16 x 8 as two 8 x 8 tiles, each coded on its own, with SOP and EPH markers around every packet.
    let src = img(16, 8, 1, |x, y, _| x * 15 + y * 9 + (x * y) % 7 * 11);
    let mut e = Enc::new(16, 8, 1, 1);
    (e.tw, e.sop_eph) = (8, true);
    let cs = encode(&src, &e);
    assert_eq!(cs.windows(2).filter(|w| w == &[0xFF, 0x90]).count(), 2);
    assert_eq!(decoded(&cs).data, src);
}

#[test]
fn jp2_boxed() {
    let src = img(8, 8, 3, |x, y, c| x * 30 + y * 3 + c * 70);
    let mut e = Enc::new(8, 8, 3, 1);
    e.mct = true;
    let cs = encode(&src, &e);
    let d = decoded(&jp2(&cs, 8, 8, 3, 16, &[]));
    assert_eq!((d.space, d.data.clone()), (Space::Rgb, src));
    let g = img(8, 8, 1, |x, y, _| x * 30 + y);
    let d = decoded(&jp2(&encode(&g, &Enc::new(8, 8, 1, 1)), 8, 8, 1, 17, &[]));
    assert_eq!((d.space, d.data.clone()), (Space::Gray, g));
    // sYCC (enumeration 18): the decoder turns Y Cb Cr into RGB.
    let ycc = img(2, 1, 3, |_, _, c| [100, 128, 128][c]);
    let d = decoded(&jp2(&encode(&ycc, &Enc::new(2, 1, 3, 0)), 2, 1, 3, 18, &[]));
    assert_eq!((d.space, d.data.clone()), (Space::Rgb, vec![100; 6]));
}

#[test]
fn bypass_termall_reset_segsym() {
    // Full-range samples: ~8 bit-planes per block, so more than ten passes and the raw segments of BYPASS.
    let src = img(8, 8, 1, |x, y, _| x * 37 + y * 91 + x * y * 13);
    for style in [0x01 | 0x02 | 0x04 | 0x20, 0x01 | 0x10 | 0x20, 0x01, 0x04, 0x02 | 0x20] {
        let mut e = Enc::new(8, 8, 1, 1);
        e.style = style;
        let q: Vec<i32> = src.iter().map(|&v| v as i32 - 128).collect();
        let c = t1_encode(&q, 8, 8, 0, mb_of(&e, 0), style).unwrap();
        assert!(c.passes > 10, "style {style:#x}: {} passes", c.passes);
        assert_eq!(decoded(&encode(&src, &e)).data, src, "style {style:#x}");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Refusals and robustness

/// The offset of main-header marker `m` in `cs`.
fn find(cs: &[u8], m: u16) -> usize {
    let mut at = 2;
    loop {
        let here = u16::from_be_bytes([cs[at], cs[at + 1]]);
        if here == m {
            return at;
        }
        at += 2 + u16::from_be_bytes([cs[at + 2], cs[at + 3]]) as usize;
    }
}

#[test]
fn refusals_by_name() {
    let base = encode(&img(8, 8, 1, |x, y, _| x * 9 + y), &Enc::new(8, 8, 1, 1));
    let refused = |cs: &[u8]| match jpx::decode(cs) {
        Err(Error::Unsupported(why)) => why,
        other => panic!("not refused by name: {:?}", other.map(|i| (i.w, i.h))),
    };
    let insert = |m: u16, body: &[u8]| {
        let at = find(&base, 0xFF90);
        let mut seg = Vec::new();
        marker(&mut seg, m, body);
        let mut cs = base[..at].to_vec();
        cs.extend(seg);
        cs.extend_from_slice(&base[at..]);
        cs
    };
    assert_eq!(refused(&insert(0xFF5E, &[0, 0, 1])), "JPX region of interest");
    assert_eq!(refused(&insert(0xFF5F, &[0, 0, 0, 1, 1, 1, 0])), "JPX progression order change");
    assert_eq!(refused(&insert(0xFF60, &[0])), "JPX packed packet headers");
    let cod = find(&base, 0xFF52);
    let mut cs = base.clone();
    cs[cod + 5] = 4;
    assert_eq!(refused(&cs), "JPX progression CPRL/PCRL");
    let mut cs = base.clone();
    cs[cod + 12] |= 0x08;
    assert_eq!(refused(&cs), "JPX vertical causal");
    let mut cs = base.clone();
    cs[6] |= 0x40;
    assert_eq!(refused(&cs), "JPX HTJ2K");
    let mut cs = base.clone();
    cs[6] |= 0x80;
    assert_eq!(refused(&cs), "JPX Part 2");
    let mut cs = base.clone();
    cs[42] = 16;
    assert_eq!(refused(&cs), "JPX precision");
    assert_eq!(refused(&jp2(&base, 8, 8, 1, 17, &bx(b"pclr", &[0, 2, 1, 7, 0, 0]))), "JPX palette");
    // A 2^20 x 2^20 canvas is over MAX_DECODED.
    let mut cs = base.clone();
    for off in [8, 12, 24, 28] {
        cs[off..off + 4].copy_from_slice(&(1u32 << 20).to_be_bytes());
    }
    assert_eq!(jpx::decode(&cs).map(|_| ()), Err(Error::TooLarge));
}

#[test]
fn truncation_never_panics() {
    let mut e = Enc::new(8, 8, 1, 1);
    e.style = 0x01 | 0x04;
    let ours = encode(&img(8, 8, 1, |x, y, _| x * 37 + y * 91 + x * y * 13), &e);
    for whole in [unhex(OPJ_RGB_12_JP2), unhex(OPJ_RGB_12_RPCL), ours] {
        for n in 0..whole.len() {
            let _ = jpx::decode(&whole[..n]);
        }
        let mut flipped = whole.clone();
        for i in 0..flipped.len() {
            flipped[i] ^= 0x5A;
            let _ = jpx::decode(&flipped);
            flipped[i] ^= 0x5A;
        }
    }
}
