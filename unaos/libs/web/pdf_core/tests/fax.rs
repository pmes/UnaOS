// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDFFAX (LEDGER SR331) KATs: CCITTFaxDecode, the MQ coder and JBIG2Decode, and both drawn through PDF2's renderer.
//!
//! Known answers from outside this crate: libtiff's own Group 3 (1-D, EOLs, fill bits) and Group 4 strips, written
//! by ImageMagick 6.9.12-98 (`convert <pbm> -define tiff:photometric=min-is-white -define tiff:rows-per-strip=64
//! -compress Fax|Group4`) and pinned here as hex; and the T.88 Annex H.2 test sequence for the MQ coder. Round trips
//! use test-side ENCODERS written from the other side of each spec (T.4 4.2 / T.6 coding, T.88 Annex E.2 encoding,
//! Annex A integer encoding), never the decoder's own code.

mod common;

use common::PdfBuilder;
use pdf_core::filters::ccitt::{self, Params};
use pdf_core::filters::jbig2::{self, Bitmap};
use pdf_core::filters::mq::{MqDecoder, QE};
use pdf_core::render::{Rendered, TextLog};
use pdf_core::{Document, Error};

fn unhex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// CCITT: the tables

fn prefix_free_and_kraft(codes: &[(u16, u8)]) {
    let mut kraft = 0f64;
    for (i, &(c, l)) in codes.iter().enumerate() {
        assert!(l >= 1 && l <= 13 && (c as u32) < (1u32 << l), "code {i} does not fit its length");
        kraft += 1.0 / (1u64 << l) as f64;
        for (j, &(d, m)) in codes.iter().enumerate() {
            if i != j && m >= l {
                assert_ne!((d >> (m - l)), c, "code {i} ({c:0w$b}) is a prefix of code {j}", w = l as usize);
            }
        }
    }
    assert!(kraft <= 1.0, "kraft sum {kraft}");
}

#[test]
fn ccitt_tables_are_prefix_free() {
    let mut white: Vec<(u16, u8)> = ccitt::WHITE_TERM.to_vec();
    white.extend(ccitt::WHITE_MAKEUP);
    white.extend(ccitt::EXT_MAKEUP);
    white.push(ccitt::EOL);
    prefix_free_and_kraft(&white);
    let mut black: Vec<(u16, u8)> = ccitt::BLACK_TERM.to_vec();
    black.extend(ccitt::BLACK_MAKEUP);
    black.extend(ccitt::EXT_MAKEUP);
    black.push(ccitt::EOL);
    prefix_free_and_kraft(&black);
    let mut modes = vec![ccitt::PASS, ccitt::HORIZONTAL, ccitt::EXTENSION, ccitt::EOL];
    modes.extend(ccitt::VERTICAL);
    prefix_free_and_kraft(&modes);
    // The white set is nearly complete: everything but the all-zero corner (EOL and the 7-zero escape) is coded.
    assert_eq!(ccitt::WHITE_TERM[0], (0b00110101, 8));
    assert_eq!(ccitt::BLACK_TERM[0], (0b0000110111, 10));
}

// ---------------------------------------------------------------------------------------------------------------
// CCITT: a test-side encoder (T.4 4.2.1 / 4.2.2, T.6 2.2)

struct BitW {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitW {
    fn new() -> Self {
        BitW { out: Vec::new(), acc: 0, n: 0 }
    }
    fn put(&mut self, (code, len): (u16, u8)) {
        self.acc = (self.acc << len) | code as u64;
        self.n += len as u32;
        while self.n >= 8 {
            self.out.push((self.acc >> (self.n - 8)) as u8);
            self.n -= 8;
        }
        self.acc &= (1 << self.n) - 1;
    }
    fn align(&mut self) {
        if self.n > 0 {
            self.put((0, (8 - self.n) as u8));
        }
    }
    fn finish(mut self) -> Vec<u8> {
        self.align();
        self.out
    }
}

fn put_run(w: &mut BitW, mut run: usize, black: bool) {
    let (term, makeup) = if black { (&ccitt::BLACK_TERM, &ccitt::BLACK_MAKEUP) } else { (&ccitt::WHITE_TERM, &ccitt::WHITE_MAKEUP) };
    while run > 2560 {
        w.put(ccitt::EXT_MAKEUP[12]);
        run -= 2560;
    }
    if run >= 64 {
        let m = run / 64 * 64;
        w.put(if m >= 1792 { ccitt::EXT_MAKEUP[(m - 1792) / 64] } else { makeup[m / 64 - 1] });
        run -= m;
    }
    w.put(term[run]);
}

/// The changing elements of a row of pixels (true = black), white first.
fn changes(row: &[bool]) -> Vec<usize> {
    let mut c = Vec::new();
    let mut prev = false;
    for (x, &p) in row.iter().enumerate() {
        if p != prev {
            c.push(x);
            prev = p;
        }
    }
    c
}

fn enc_1d(w: &mut BitW, row: &[bool]) {
    let mut at = 0;
    let mut black = false;
    for c in changes(row).into_iter().chain([row.len()]) {
        put_run(w, c - at, black);
        at = c;
        black = !black;
    }
}

fn enc_2d(w: &mut BitW, row: &[bool], refl: &[usize]) {
    let cols = row.len();
    let cur = changes(row);
    let mut a0: isize = -1;
    let mut black = false;
    while a0 < cols as isize {
        let ai = cur.iter().position(|&c| c as isize > a0);
        let a1 = ai.map_or(cols, |i| cur[i]);
        let a2 = ai.and_then(|i| cur.get(i + 1).copied()).unwrap_or(cols);
        let bi = (0..refl.len()).find(|&i| refl[i] as isize > a0 && (i % 2 == 1) == black);
        let b1 = bi.map_or(cols, |i| refl[i]);
        let b2 = bi.and_then(|i| refl.get(i + 1).copied()).unwrap_or(cols);
        if b2 < a1 {
            w.put(ccitt::PASS);
            a0 = b2 as isize;
        } else if (a1 as isize - b1 as isize).abs() <= 3 {
            w.put(ccitt::VERTICAL[(a1 as isize - b1 as isize + 3) as usize]);
            a0 = a1 as isize;
            black = !black;
        } else {
            w.put(ccitt::HORIZONTAL);
            put_run(w, a1 - a0.max(0) as usize, black);
            put_run(w, a2 - a1, !black);
            a0 = a2 as isize;
        }
    }
}

/// Encode `rows` with /K `k`; `eol` writes an EOL before each row, `align` byte-aligns each row (before its EOL),
/// `end` writes RTC (K >= 0) or EOFB (K < 0).
fn ccitt_encode(rows: &[Vec<bool>], k: i64, eol: bool, align: bool, end: bool) -> Vec<u8> {
    let mut w = BitW::new();
    let mut refl: Vec<usize> = Vec::new();
    for (y, row) in rows.iter().enumerate() {
        if align {
            w.align();
        }
        if eol {
            w.put(ccitt::EOL);
        }
        let two_d = if k > 0 {
            let two = y as i64 % k != 0;
            w.put((!two as u16, 1));
            two
        } else {
            k < 0
        };
        if two_d { enc_2d(&mut w, row, &refl) } else { enc_1d(&mut w, row) }
        refl = changes(row);
    }
    if end {
        let n = if k < 0 { 2 } else { 6 };
        for _ in 0..n {
            w.put(ccitt::EOL);
            if k > 0 {
                w.put((1, 1));
            }
        }
    }
    w.finish()
}

/// Rows of pixels packed as the decoder writes them (black = 0 unless `black_is_1`).
fn pack(rows: &[Vec<bool>], black_is_1: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for r in rows {
        let mut row = vec![if black_is_1 { 0u8 } else { 0xFF }; r.len().div_ceil(8)];
        for (x, &p) in r.iter().enumerate() {
            if p {
                if black_is_1 { row[x / 8] |= 0x80 >> (x % 8) } else { row[x / 8] &= !(0x80 >> (x % 8)) }
            }
        }
        out.extend(row);
    }
    out
}

/// A deterministic picture: runs of every scale (short, make-up, extended make-up) and rows that shift a little
/// from the row above (so the 2-D coder meets V, pass and horizontal modes).
fn picture(cols: usize, nrows: usize, seed: u32) -> Vec<Vec<bool>> {
    let mut s = seed;
    let mut rnd = move |m: u32| {
        s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (s >> 8) % m
    };
    let mut rows: Vec<Vec<bool>> = Vec::new();
    for y in 0..nrows {
        let row = if y > 0 && rnd(3) != 0 {
            // The row above with its edges nudged by -3..3 and an occasional new speck.
            let prev = &rows[y - 1];
            let mut r = vec![false; cols];
            let mut cur = changes(prev);
            for c in cur.iter_mut() {
                *c = (*c as isize + rnd(7) as isize - 3).clamp(0, cols as isize) as usize;
            }
            cur.sort();
            let mut black = false;
            let mut at = 0;
            for c in cur.into_iter().chain([cols]) {
                for p in &mut r[at..c.max(at)] {
                    *p = black;
                }
                at = c.max(at);
                black = !black;
            }
            if rnd(4) == 0 {
                let x = rnd(cols as u32) as usize;
                r[x] = !r[x];
            }
            r
        } else {
            let mut r = Vec::with_capacity(cols);
            let mut black = false;
            while r.len() < cols {
                let len = match rnd(10) {
                    0 => 64 * (1 + rnd(27) as usize) + rnd(64) as usize,
                    1 => 1792 + rnd(900) as usize,
                    _ => rnd(64) as usize,
                };
                r.extend(std::iter::repeat_n(black, len.min(cols - r.len())));
                black = !black;
            }
            r
        };
        rows.push(row);
    }
    rows
}

fn params(k: i64, cols: usize) -> Params {
    Params { k, columns: cols, ..Params::default() }
}

#[test]
fn ccitt_round_trip_mh_mr_mmr() {
    let rows = picture(3000, 40, 7);
    for k in [0i64, 2, 4, -1] {
        for (eol, end) in [(false, false), (true, true), (false, true)] {
            if k < 0 && eol {
                continue;
            }
            let data = ccitt_encode(&rows, k, eol, false, end);
            let got = ccitt::decode(&data, &Params { end_of_line: eol, ..params(k, 3000) }).unwrap();
            assert_eq!(got, pack(&rows, false), "K={k} eol={eol} end={end}");
        }
    }
}

#[test]
fn ccitt_byte_align_black_is_1_and_rows() {
    let rows = picture(77, 30, 99);
    for k in [0i64, 3, -1] {
        let data = ccitt_encode(&rows, k, k == 0, true, true);
        let p = Params { byte_align: true, black_is_1: true, ..params(k, 77) };
        assert_eq!(ccitt::decode(&data, &p).unwrap(), pack(&rows, true), "K={k}");
        // /Rows stops early.
        let p = Params { rows: 10, ..p };
        assert_eq!(ccitt::decode(&data, &p).unwrap(), pack(&rows[..10], true), "K={k} rows=10");
    }
    // Without /EncodedByteAlign the aligned stream mis-reads; the first row may survive, the picture does not.
    let data = ccitt_encode(&rows, -1, false, true, true);
    assert_ne!(ccitt::decode(&data, &params(-1, 77)).ok(), Some(pack(&rows, false)));
}

#[test]
fn ccitt_uncompressed_mode_refused_by_name() {
    let mut w = BitW::new();
    w.put(ccitt::EXTENSION);
    w.put((0b111, 3));
    w.put((0, 8));
    assert_eq!(ccitt::decode(&w.finish(), &params(-1, 16)), Err(Error::Unsupported("CCITT uncompressed mode")));
}

#[test]
fn ccitt_corrupt_tail_keeps_rows() {
    let rows = picture(64, 8, 3);
    let mut data = ccitt_encode(&rows, -1, false, false, false);
    let cut = data.len() / 2;
    data.truncate(cut);
    data.extend([0x00, 0x00, 0x7F, 0xFF]);
    let got = ccitt::decode(&data, &params(-1, 64)).unwrap();
    assert!(!got.is_empty() && got.len() < 8 * 8);
    assert_eq!(got[..], pack(&rows, false)[..got.len()]);
}

/// The libtiff picture of the KAT: the same formula as the generator that fed ImageMagick.
fn libtiff_rows() -> Vec<Vec<bool>> {
    (0..64)
        .map(|y: usize| {
            let runs = [y, y, 64 * (y % 27 + 1) + (63 - y), 64 * ((y + 13) % 27 + 1) + (y % 7), 1792 + 64 * (y % 13), 5];
            let mut px = Vec::new();
            let mut black = false;
            for r in runs {
                px.extend(std::iter::repeat_n(black, r));
                black = !black;
            }
            px.resize(6000.max(px.len()), black);
            px.truncate(6000);
            px
        })
        .collect()
}

/// The 16 x 4 KAT picture: rows 0x1fc0 0x1fc8 0x1fc0 0x1fc0 (1 = black).
fn small_rows() -> Vec<Vec<bool>> {
    [0x1fc0u16, 0x1fc8, 0x1fc0, 0x1fc0].iter().map(|r| (0..16).map(|x| r >> (15 - x) & 1 == 1).collect()).collect()
}

/// libtiff G3 1-D (Group3Options 4: EOLs, fill bits to byte-align them), 16 x 4.
const LIBTIFF_G3_SMALL: &str = "000181f0000181ba800181f0000181f0";
/// libtiff G4, 16 x 4 (ends in EOFB).
const LIBTIFF_G4_SMALL: &str = "303e5d71f0010010";
/// libtiff G3 1-D, 6000 x 64 (runs 0-63 of both colours, every make-up code scale, extended make-up codes).
const LIBTIFF_G3_BIG: &str = "0001d9a01c8370106a603ed05a00011d48cc0e68061a980f9b9580017d73203a6034d4c07d72900189ba581d60123530\
    1f170001b66c9407660266a60382a001c3375b03b980a1a980a8980001e2645a029100a9a98090d80001f1b2ac814c37\
    0163530102c00198ad0b0054402e6a69a14001a08ce4a055c070d4db6240013846612016a01d3536c0d0000140566aa8\
    16d80f1a9b54080001203b49500c8603e6a6d24800010c11a6a606520106a698560001d01da8a40786e030d4c07d900c\
    0001d430d50b0c84034d4c07db980001a8176b05064e0246a603fb260001ac186b8282dc0266a603aa00014e04360100\
    cd80a1a980b030011819db25a068602a6a6026200001101a1b45806a402c6a60313800012e1b1b65606c0dc05cd4d808\
    0001060dd305406d40386a6982e001080a1325204ac074d4db2300015005d345004b8078d4dad5800156061817026301\
    f35369ea0001261949b1602698083534c8600148197b15039100c35301f65400013019921403986e034d4c07cdaa0001\
    020cd5c4c0e88048d4c07e5a0001030686e24075c04cd4c07880011a069361b03b40286a602e70011b06a371a03bb015\
    3530149800011206b6403029180b1a980de001130d26502029900b9a9a6ec001140d368300540dc070d4d3340001150d\
    46748055403a6a6daf0001160d5661302d603c6a6d7c0001170d666ab02dc03e6a6d480001280d7692803230106a69a1\
    c0012906c6984032980c3536d1a000012a06d6a0303c8069a980f9b19000012b0da6a970c80dc048d4c07d7700012c0d\
    b6b080c940266a603e6a00012d0546b8c05bc050d4c05ccc0001040556c270338054d4c0512c0001050566cd6068c058\
    d4c06ad800010a0576d5406a602e6a69b590010b0646dea06c201c3534c9280001520654c6806d0dc074d4db49400153\
    0524c8604a403c6a6d75a001540534d1004bc07cd4da8b00015502461004c8041a9a68900001240374d9c09ac061a9b6\
    d5000125038dd00e46034d4c07cdd480015802794c0e640246a603eb850001590285fc0e81b8099a980faa80015a0586\
    fc07540286a6038a60015b05936c03b602a6a602a160014a02b37b03bc02c6a60240a0014b02c64802930173530105a0\
    013205a657029980e1a9a68ac00133066681c0a8403a6a6db2900134067673502a86e03c6a6d8040";
/// libtiff G4, 6000 x 64.
const LIBTIFF_G4_BIG: &str = "3b3403906e4041a9c8e8c8cc0e684c8c8eceb99103a6034d4980fae52c32dd2c0eb099191d864d9280ecc4c8c8ec326e\
    b60773132323b0cb22d014884c8c8ec32cab20530dc4c894ec32d0b00544264647619672502ae2646476196612016a13\
    2323b0cb35540b6c4c8c8ec32d2540321899191d865a6a606522020d4c6c32d45203c372030d4c6c31238642132323b0\
    cb5828327132323b0cb5c1416e132323b0cb60100cd899191d865b25a06862646476196d1601a9099191d865b65606c0\
    dc4c894ec32982a036a132323b0ca64a4095899191d865345004b84c8c8ec32c0b8131899191d865362c04d32020d4c6\
    c33b150391096ee6361990a01cc37132253b0cae2607442646476196e24075c4c8c8ec326c3607684c8c8ec326e34077\
    626464761964030291899191d86594080a6426464761968300540dc4c894ec32ce900aa84c8c8ec32cc2605ac4c8c8ec\
    32cd5605b84c8c8ec32d25006464041a98d865a6100ca62646476196a0303c880d3531b0cb54b86406e2644a76196b08\
    0c942646476196b8c05bc4c8c8ec32d84e067099191d865b3581a3132323b0cb6aa0353132323b0cb6f50361099191d8\
    6531a01b437132253b0ca6430252132323b0ca688025e26464761961004c88083531b0ca6ce04d6264647619dd00e462\
    4d9cc6c332981cc84c8c8ec32bf81d037132253b0cb7e03aa132323b0c9b601db132323b0c9bd81de132323b0cb24014\
    9899191d86595c0a66264647619681c0a84264647619673502a86e2644a7001001";

#[test]
fn ccitt_libtiff_known_answers() {
    let small = pack(&small_rows(), false);
    assert_eq!(small, vec![0xe0, 0x3f, 0xe0, 0x37, 0xe0, 0x3f, 0xe0, 0x3f]);
    let p = Params { end_of_line: true, ..params(0, 16) };
    assert_eq!(ccitt::decode(&unhex(LIBTIFF_G3_SMALL), &p).unwrap(), small);
    assert_eq!(ccitt::decode(&unhex(LIBTIFF_G4_SMALL), &params(-1, 16)).unwrap(), small);
    let big = pack(&libtiff_rows(), false);
    let p = Params { end_of_line: true, ..params(0, 6000) };
    assert_eq!(ccitt::decode(&unhex(LIBTIFF_G3_BIG), &p).unwrap(), big);
    assert_eq!(ccitt::decode(&unhex(LIBTIFF_G4_BIG), &params(-1, 6000)).unwrap(), big);
    // /Rows bounds the libtiff stream too.
    let p = Params { rows: 2, ..params(-1, 16) };
    assert_eq!(ccitt::decode(&unhex(LIBTIFF_G4_SMALL), &p).unwrap(), small[..4]);
    // And this crate's encoder writes libtiff's G4 bytes for the small picture, bit for bit, up to the EOFB.
    assert_eq!(ccitt_encode(&small_rows(), -1, false, false, true), unhex(LIBTIFF_G4_SMALL));
}

// ---------------------------------------------------------------------------------------------------------------
// MQ: a test-side encoder (T.88 E.2) and the H.2 test sequence

struct MqEnc {
    a: u32,
    c: u32,
    ct: u32,
    b: u8,
    have: bool,
    out: Vec<u8>,
}

impl MqEnc {
    fn new() -> Self {
        MqEnc { a: 0x8000, c: 0, ct: 12, b: 0, have: false, out: Vec::new() }
    }
    /// BP = BP + 1; B = v (the byte at BPST - 1 is never written out).
    fn emit(&mut self, v: u8) {
        if self.have {
            self.out.push(self.b);
        }
        self.b = v;
        self.have = true;
    }
    fn byte_out(&mut self) {
        if self.have && self.b == 0xFF {
            self.emit((self.c >> 20) as u8);
            self.c &= 0xFFFFF;
            self.ct = 7;
        } else if self.c < 0x800_0000 {
            self.emit((self.c >> 19) as u8);
            self.c &= 0x7FFFF;
            self.ct = 8;
        } else {
            self.b = self.b.wrapping_add(1);
            if self.b == 0xFF {
                self.c &= 0x7FF_FFFF;
                self.emit((self.c >> 20) as u8);
                self.c &= 0xFFFFF;
                self.ct = 7;
            } else {
                self.emit((self.c >> 19) as u8);
                self.c &= 0x7FFFF;
                self.ct = 8;
            }
        }
    }
    fn renorm(&mut self) {
        loop {
            self.a <<= 1;
            self.c <<= 1;
            self.ct -= 1;
            if self.ct == 0 {
                self.byte_out();
            }
            if self.a & 0x8000 != 0 {
                break;
            }
        }
    }
    fn encode(&mut self, cx: &mut u8, d: u8) {
        let mut idx = (*cx >> 1) as usize;
        let mut mps = *cx & 1;
        let (qe, nmps, nlps, sw) = QE[idx];
        let qe = qe as u32;
        self.a -= qe;
        if d == mps {
            if self.a & 0x8000 == 0 {
                if self.a < qe {
                    self.a = qe;
                } else {
                    self.c += qe;
                }
                idx = nmps as usize;
                self.renorm();
            } else {
                self.c += qe;
            }
        } else {
            if self.a < qe {
                self.c += qe;
            } else {
                self.a = qe;
            }
            if sw == 1 {
                mps ^= 1;
            }
            idx = nlps as usize;
            self.renorm();
        }
        *cx = (idx as u8) << 1 | mps;
    }
    fn flush(mut self) -> Vec<u8> {
        let temp = self.c + self.a;
        self.c |= 0xFFFF;
        if self.c >= temp {
            self.c -= 0x8000;
        }
        self.c <<= self.ct;
        self.byte_out();
        self.c <<= self.ct;
        self.byte_out();
        if self.b != 0xFF {
            self.emit(0xFF);
        }
        self.emit(0xAC);
        self.out.push(self.b);
        self.out
    }
}

/// T.88 H.2: the 256 decisions (as 32 bytes, MSB first) and their code, all in one context.
const H2_DATA: &str = "00020051000000c00352872aaaaaaaaa82c02000fcd79ef6bf7fed904f46a3bf";
const H2_CODE: &str = "84c73bfce1a1430402200000410dbb86f4317fff88ff37471adb6adfffac";

#[test]
fn mq_h2_test_sequence() {
    let bits: Vec<u8> = unhex(H2_DATA).iter().flat_map(|b| (0..8).map(move |k| b >> (7 - k) & 1)).collect();
    assert_eq!(bits.len(), 256);
    let code = unhex(H2_CODE);
    assert_eq!(code.len(), 30);
    let mut dec = MqDecoder::new(&code);
    let mut cx = 0u8;
    let got: Vec<u8> = (0..256).map(|_| dec.decode(&mut cx)).collect();
    assert_eq!(got, bits);
    let mut enc = MqEnc::new();
    let mut cx = 0u8;
    for &b in &bits {
        enc.encode(&mut cx, b);
    }
    assert_eq!(enc.flush(), code);
}

#[test]
fn mq_round_trip_many_contexts() {
    let mut s = 1u32;
    let mut cxs = vec![0u8; 64];
    let mut enc = MqEnc::new();
    let mut seq = Vec::new();
    for i in 0..20000u32 {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let c = (i % 64) as usize;
        // Skewed per context, so states climb and switch.
        let d = ((s >> 16) % 100 < (c as u32 * 3 % 97)) as u8;
        seq.push((c, d));
        enc.encode(&mut cxs[c], d);
    }
    let code = enc.flush();
    let mut dec = MqDecoder::new(&code);
    let mut cxs = vec![0u8; 64];
    for (i, &(c, d)) in seq.iter().enumerate() {
        assert_eq!(dec.decode(&mut cxs[c]), d, "decision {i}");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// JBIG2: test-side encoders and segments

fn get(bm: &[Vec<u8>], x: isize, y: isize) -> u32 {
    if x < 0 || y < 0 || y as usize >= bm.len() || x as usize >= bm[0].len() {
        0
    } else {
        bm[y as usize][x as usize] as u32
    }
}

/// The template's context at (x, y), written from T.88 Figures 3-6 (standard bit order).
fn context(bm: &[Vec<u8>], x: isize, y: isize, t: u8, at: &[(i8, i8); 4]) -> usize {
    let p = |dx: isize, dy: isize| get(bm, x + dx, y + dy);
    let a = |i: usize| p(at[i].0 as isize, at[i].1 as isize);
    let bits: Vec<u32> = match t {
        0 => vec![p(-1, 0), p(-2, 0), p(-3, 0), p(-4, 0), a(0), p(2, -1), p(1, -1), p(0, -1), p(-1, -1), p(-2, -1), a(1), a(2), p(1, -2), p(0, -2), p(-1, -2), a(3)],
        1 => vec![p(-1, 0), p(-2, 0), p(-3, 0), a(0), p(2, -1), p(1, -1), p(0, -1), p(-1, -1), p(-2, -1), p(2, -2), p(1, -2), p(0, -2), p(-1, -2)],
        2 => vec![p(-1, 0), p(-2, 0), a(0), p(1, -1), p(0, -1), p(-1, -1), p(-2, -1), p(1, -2), p(0, -2), p(-1, -2)],
        _ => vec![p(-1, 0), p(-2, 0), p(-3, 0), p(-4, 0), a(0), p(1, -1), p(0, -1), p(-1, -1), p(-2, -1), p(-3, -1)],
    };
    bits.iter().enumerate().fold(0, |acc, (i, &b)| acc | (b as usize) << i)
}

const DEFAULT_AT: [[(i8, i8); 4]; 4] =
    [[(3, -1), (-3, -1), (2, -2), (-2, -2)], [(3, -1), (0, 0), (0, 0), (0, 0)], [(2, -1), (0, 0), (0, 0), (0, 0)], [(2, -1), (0, 0), (0, 0), (0, 0)]];

/// A generic region, arithmetic (6.2.5.7 in reverse); TPGDON's row flag in the template's pseudo-pixel context.
fn enc_generic(enc: &mut MqEnc, cx: &mut [u8], bm: &[Vec<u8>], t: u8, tpgdon: bool, at: &[(i8, i8); 4]) {
    let sltp = [0x9B25usize, 0x0795, 0x00E5, 0x0195][t as usize];
    let mut ltp = 0u8;
    for y in 0..bm.len() {
        if tpgdon {
            let same = if y == 0 { bm[0].iter().all(|&p| p == 0) } else { bm[y] == bm[y - 1] } as u8;
            enc.encode(&mut cx[sltp], same ^ ltp);
            ltp = same;
            if same == 1 {
                continue;
            }
        }
        for x in 0..bm[y].len() {
            let c = context(bm, x as isize, y as isize, t, at);
            enc.encode(&mut cx[c], bm[y][x]);
        }
    }
}

/// Annex A.2 in reverse: one value (None = OOB).
fn enc_int(enc: &mut MqEnc, cx: &mut [u8], v: Option<i64>) {
    let mut prev = 1usize;
    let mut bit = |enc: &mut MqEnc, d: u8| {
        enc.encode(&mut cx[prev], d);
        prev = if prev < 256 { (prev << 1) | d as usize } else { (((prev << 1) | d as usize) & 511) | 256 };
    };
    let (s, a) = match v {
        None => (1u8, 0i64),
        Some(v) => ((v < 0) as u8, v.abs()),
    };
    bit(enc, s);
    let ranges: [(i64, u32, u32); 6] = [(0, 0, 2), (4, 1, 4), (20, 2, 6), (84, 3, 8), (340, 4, 12), (4436, 5, 32)];
    let &(lo, ones, n) = ranges.iter().rev().find(|r| a >= r.0).unwrap();
    for _ in 0..ones {
        bit(enc, 1);
    }
    if ones < 5 {
        bit(enc, 0);
    }
    for k in (0..n).rev() {
        bit(enc, ((a - lo) >> k & 1) as u8);
    }
}

fn enc_iaid(enc: &mut MqEnc, cx: &mut [u8], id: usize, len: u32) {
    let mut prev = 1usize;
    for k in (0..len).rev() {
        let d = (id >> k & 1) as u8;
        enc.encode(&mut cx[prev], d);
        prev = (prev << 1) | d as usize;
    }
}

/// A segment header (7.2.1) with a 1-byte page association and short-form referred-to segments.
fn seg(num: u32, ty: u8, refs: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s = num.to_be_bytes().to_vec();
    s.push(ty);
    s.push((refs.len() as u8) << 5);
    s.extend_from_slice(refs);
    s.push(1);
    s.extend((data.len() as u32).to_be_bytes());
    s.extend_from_slice(data);
    s
}

fn page_info(w: u32, h: u32, def: u8) -> Vec<u8> {
    let mut d = w.to_be_bytes().to_vec();
    d.extend(h.to_be_bytes());
    d.extend([0; 8]);
    d.push(def << 2);
    d.extend([0, 0]);
    d
}

fn region(w: u32, h: u32, x: u32, y: u32, op: u8) -> Vec<u8> {
    let mut d = Vec::new();
    for v in [w, h, x, y] {
        d.extend(v.to_be_bytes());
    }
    d.push(op);
    d
}

fn at_bytes(t: u8) -> Vec<u8> {
    let n = if t == 0 { 4 } else { 1 };
    DEFAULT_AT[t as usize][..n].iter().flat_map(|&(x, y)| [x as u8, y as u8]).collect()
}

/// The 16 x 16 pattern: a diagonal, a box, a speckle, and three equal rows (TPGDON's case).
fn pattern() -> Vec<Vec<u8>> {
    (0..16)
        .map(|y| {
            let yy = if (6..9).contains(&y) { 6 } else { y };
            (0..16)
                .map(|x| ((x == yy) || ((3..12).contains(&x) && (yy == 2 || yy == 13)) || (x * 7 + yy * 3) % 11 == 0) as u8)
                .collect()
        })
        .collect()
}

fn generic_segment(bm: &[Vec<u8>], t: u8, tpgdon: bool) -> Vec<u8> {
    let mut enc = MqEnc::new();
    let mut cx = vec![0u8; 1 << 16];
    enc_generic(&mut enc, &mut cx, bm, t, tpgdon, &DEFAULT_AT[t as usize]);
    let mut d = region(bm[0].len() as u32, bm.len() as u32, 0, 0, 0);
    d.push(t << 1 | (tpgdon as u8) << 3);
    d.extend(at_bytes(t));
    d.extend(enc.flush());
    d
}

fn jbig2_page(regions: &[(u8, Vec<u8>)], w: u32, h: u32) -> Vec<u8> {
    let mut s = seg(0, 48, &[], &page_info(w, h, 0));
    for (i, (ty, d)) in regions.iter().enumerate() {
        s.extend(seg(1 + i as u32, *ty, &[], d));
    }
    s.extend(seg(9, 49, &[], &[]));
    s
}

fn bitmap(rows: &[Vec<u8>]) -> Bitmap {
    Bitmap { w: rows[0].len(), h: rows.len(), data: rows.concat() }
}

#[test]
fn jbig2_generic_region_templates_and_tpgdon() {
    let pat = pattern();
    for t in 0..4u8 {
        for tpgdon in [false, true] {
            let stream = jbig2_page(&[(38, generic_segment(&pat, t, tpgdon))], 16, 16);
            assert_eq!(jbig2::decode(&stream, None).unwrap(), bitmap(&pat), "template {t} tpgdon {tpgdon}");
        }
    }
}

#[test]
fn jbig2_generic_region_mmr_through_ccitt() {
    let pat = pattern();
    let rows: Vec<Vec<bool>> = pat.iter().map(|r| r.iter().map(|&p| p == 1).collect()).collect();
    let mut d = region(16, 16, 0, 0, 0);
    d.push(1);
    d.extend(ccitt_encode(&rows, -1, false, false, false));
    let stream = jbig2_page(&[(38, d)], 16, 16);
    assert_eq!(jbig2::decode(&stream, None).unwrap(), bitmap(&pat));
}

#[test]
fn jbig2_region_offset_and_combination() {
    let pat = pattern();
    let mut g = generic_segment(&pat, 0, false);
    g[8..12].copy_from_slice(&4u32.to_be_bytes()); // x = 4
    let mut x = generic_segment(&pat, 0, false);
    x[16] = 2; // XOR over the first
    let stream = jbig2_page(&[(38, generic_segment(&pat, 0, false)), (38, x), (39, g)], 24, 16);
    let got = jbig2::decode(&stream, None).unwrap();
    assert_eq!((got.w, got.h), (24, 16));
    for y in 0..16 {
        for xx in 0..24 {
            let want = if xx >= 4 && xx < 20 { pat[y][xx - 4] } else { 0 };
            assert_eq!(got.data[y * 24 + xx], want, "({xx},{y})");
        }
    }
}

/// Three symbols: C 3x3 (height class 3), B 4x7 and A 5x7 (height class 7), decode order C B A.
fn symbols() -> Vec<Vec<Vec<u8>>> {
    let c: Vec<Vec<u8>> = vec![vec![1, 1, 1], vec![1, 0, 0], vec![1, 1, 1]];
    let b: Vec<Vec<u8>> = (0..7).map(|y| (0..4).map(|x| (x == 0 || y % 3 == 0 && x < 3 || x == 3 && y % 3 != 0) as u8).collect()).collect();
    let a: Vec<Vec<u8>> = (0..7).map(|y| (0..5).map(|x| (y == 0 && x > 0 && x < 4 || y > 0 && (x == 0 || x == 4) || y == 3) as u8).collect()).collect();
    vec![c, b, a]
}

fn symbol_dict_data(flags: u16) -> Vec<u8> {
    let syms = symbols();
    let mut d = flags.to_be_bytes().to_vec();
    d.extend(at_bytes(0));
    d.extend(3u32.to_be_bytes());
    d.extend(3u32.to_be_bytes());
    let mut enc = MqEnc::new();
    let mut gb = vec![0u8; 1 << 16];
    let (mut iadh, mut iadw, mut iaex) = (vec![0u8; 512], vec![0u8; 512], vec![0u8; 512]);
    // Height class 3: C.
    enc_int(&mut enc, &mut iadh, Some(3));
    enc_int(&mut enc, &mut iadw, Some(3));
    enc_generic(&mut enc, &mut gb, &syms[0], 0, false, &DEFAULT_AT[0]);
    enc_int(&mut enc, &mut iadw, None);
    // Height class 7: B (4 wide), A (5 wide).
    enc_int(&mut enc, &mut iadh, Some(4));
    enc_int(&mut enc, &mut iadw, Some(4));
    enc_generic(&mut enc, &mut gb, &syms[1], 0, false, &DEFAULT_AT[0]);
    enc_int(&mut enc, &mut iadw, Some(1));
    enc_generic(&mut enc, &mut gb, &syms[2], 0, false, &DEFAULT_AT[0]);
    enc_int(&mut enc, &mut iadw, None);
    // Export: none of the 0 inputs is skipped, all 3 new symbols exported.
    enc_int(&mut enc, &mut iaex, Some(0));
    enc_int(&mut enc, &mut iaex, Some(3));
    d.extend(enc.flush());
    d
}

/// A 32 x 16 text region (TOPLEFT, 1 strip): A at (1, 2), B at (8, 2); C at (20, 9).
fn text_region_data(flags: u16) -> Vec<u8> {
    let mut d = region(32, 16, 0, 0, 0);
    d.extend(flags.to_be_bytes());
    d.extend(3u32.to_be_bytes());
    let mut enc = MqEnc::new();
    let (mut iadt, mut iafs, mut iads) = (vec![0u8; 512], vec![0u8; 512], vec![0u8; 512]);
    let mut iaid = vec![0u8; 4];
    enc_int(&mut enc, &mut iadt, Some(0)); // STRIPT = 0
    enc_int(&mut enc, &mut iadt, Some(2)); // strip T = 2
    enc_int(&mut enc, &mut iafs, Some(1)); // FIRSTS = 1
    enc_iaid(&mut enc, &mut iaid, 2, 2); // A, 5 wide: CURS 1 -> 5
    enc_int(&mut enc, &mut iads, Some(3)); // CURS = 8
    enc_iaid(&mut enc, &mut iaid, 1, 2); // B
    enc_int(&mut enc, &mut iads, None); // end of strip
    enc_int(&mut enc, &mut iadt, Some(7)); // strip T = 9
    enc_int(&mut enc, &mut iafs, Some(19)); // FIRSTS = 20
    enc_iaid(&mut enc, &mut iaid, 0, 2); // C
    enc_int(&mut enc, &mut iads, None);
    d.extend(enc.flush());
    d
}

fn text_expected() -> Bitmap {
    let syms = symbols();
    let mut rows = vec![vec![0u8; 32]; 16];
    for (s, x0, y0) in [(2usize, 1usize, 2usize), (1, 8, 2), (0, 20, 9)] {
        for (y, r) in syms[s].iter().enumerate() {
            for (x, &p) in r.iter().enumerate() {
                rows[y0 + y][x0 + x] |= p;
            }
        }
    }
    bitmap(&rows)
}

const TOPLEFT: u16 = 1 << 4;

#[test]
fn jbig2_symbol_dictionary_and_text_region() {
    let mut stream = seg(0, 48, &[], &page_info(32, 16, 0));
    stream.extend(seg(1, 0, &[], &symbol_dict_data(0)));
    stream.extend(seg(2, 6, &[1], &text_region_data(TOPLEFT)));
    stream.extend(seg(3, 49, &[], &[]));
    assert_eq!(jbig2::decode(&stream, None).unwrap(), text_expected());
    // The dictionary in /JBIG2Globals, the page and the text region in the stream.
    let globals = seg(1, 0, &[], &symbol_dict_data(0));
    let mut stream = seg(2, 48, &[], &page_info(32, 16, 0));
    stream.extend(seg(3, 6, &[1], &text_region_data(TOPLEFT)));
    assert_eq!(jbig2::decode(&stream, Some(&globals)).unwrap(), text_expected());
}

#[test]
fn jbig2_striped_page_grows() {
    let pat = pattern();
    let mut stream = seg(0, 48, &[], &page_info(16, u32::MAX, 0));
    stream.extend(seg(1, 38, &[], &generic_segment(&pat, 0, true)));
    stream.extend(seg(2, 50, &[], &17u32.to_be_bytes()));
    let got = jbig2::decode(&stream, None).unwrap();
    assert_eq!((got.w, got.h), (16, 18));
    assert_eq!(got.data[..256], bitmap(&pat).data[..]);
    assert!(got.data[256..].iter().all(|&p| p == 0));
}

#[test]
fn jbig2_refusals_by_name() {
    let unsupported = |s: &[u8]| match jbig2::decode(s, None) {
        Err(Error::Unsupported(why)) => why,
        other => panic!("{other:?}"),
    };
    let pat = pattern();
    let mut g = generic_segment(&pat, 0, false);
    g[17] |= 0x10;
    assert_eq!(unsupported(&jbig2_page(&[(38, g)], 16, 16)), "JBIG2 extended template");
    let mut s = seg(0, 48, &[], &page_info(32, 16, 0));
    s.extend(seg(1, 0, &[], &symbol_dict_data(1)));
    assert_eq!(unsupported(&s), "JBIG2 Huffman");
    let mut s = seg(0, 48, &[], &page_info(32, 16, 0));
    s.extend(seg(1, 0, &[], &symbol_dict_data(0)));
    s.extend(seg(2, 6, &[1], &text_region_data(TOPLEFT | 2)));
    assert_eq!(unsupported(&s), "JBIG2 refinement");
    assert_eq!(unsupported(&jbig2_page(&[(22, region(1, 1, 0, 0, 0))], 4, 4)), "JBIG2 halftone region");
    assert_eq!(unsupported(&jbig2_page(&[(42, region(1, 1, 0, 0, 0))], 4, 4)), "JBIG2 refinement");
    let mut s = seg(0, 48, &[], &page_info(4, 4, 0));
    let n = s.len();
    s[n - 4 - 19..n - 19].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(unsupported(&s), "JBIG2 unknown segment length");
    assert!(jbig2::decode(&seg(1, 38, &[], &generic_segment(&pat, 0, false)), None).is_err(), "region before page info");
}

// ---------------------------------------------------------------------------------------------------------------
// Render: through PDF1's builder and PDF2's renderer (40 x 40 pt at 72 dpi; device y = 40 - user y)

fn render(resources: &str, content: &str, extra: &[(u32, &str, Vec<u8>)]) -> Rendered {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>");
    b.obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.obj(3, &format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Resources << {resources} >> /Contents 4 0 R >>"));
    b.stream(4, "", content.as_bytes());
    for (n, d, data) in extra {
        b.stream(*n, d, data);
    }
    let doc = Document::parse(&b.classic()).unwrap();
    let pages = doc.pages().unwrap();
    pages[0].render(&doc, 72.0, &mut TextLog::default()).unwrap()
}

fn px(r: &Rendered, x: usize, y: usize) -> [u8; 4] {
    let i = (y * r.pixmap.w + x) * 4;
    r.pixmap.data[i..i + 4].try_into().unwrap()
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];

#[test]
fn render_ccitt_image() {
    // libtiff's G4 16 x 4 strip as a DeviceGray image at (10, 10), 16 x 4 pt: device rows 26-29, columns 10-25.
    let r = render(
        "/XObject << /Im1 5 0 R >>",
        "q 16 0 0 4 10 10 cm /Im1 Do Q",
        &[(5, "/Type /XObject /Subtype /Image /Width 16 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 16 /Rows 4 >>", unhex(LIBTIFF_G4_SMALL))],
    );
    assert!(r.refused.is_empty(), "{:?}", r.refused);
    assert_eq!(px(&r, 13, 26), BLACK); // row 0, column 3
    assert_eq!(px(&r, 19, 29), BLACK); // row 3, column 9
    assert_eq!(px(&r, 22, 27), BLACK); // row 1, column 12: the speck
    assert_eq!(px(&r, 22, 26), WHITE);
    assert_eq!(px(&r, 10, 26), WHITE);
    assert_eq!(px(&r, 20, 28), WHITE);
    assert_eq!(px(&r, 5, 5), WHITE);
}

#[test]
fn render_jbig2_image_mask_with_decode() {
    // A JBIG2 /ImageMask with /Decode [1 0]: JBIG2's WHITE pixels paint in the fill colour. 16 x 16 at (10, 10).
    let pat = pattern();
    let stream = jbig2_page(&[(38, generic_segment(&pat, 0, true))], 16, 16);
    let r = render(
        "/XObject << /Im1 5 0 R >>",
        "1 0 0 rg q 16 0 0 16 10 10 cm /Im1 Do Q",
        &[(5, "/Type /XObject /Subtype /Image /Width 16 /Height 16 /ImageMask true /Decode [1 0] /Filter /JBIG2Decode", stream)],
    );
    assert!(r.refused.is_empty(), "{:?}", r.refused);
    for (x, y) in [(0usize, 0usize), (5, 5), (1, 0), (4, 2), (15, 15), (7, 7), (2, 9)] {
        let want = if pat[y][x] == 1 { WHITE } else { RED };
        assert_eq!(px(&r, 10 + x, 14 + y), want, "image pixel ({x},{y})");
    }
    assert_eq!(px(&r, 5, 5), WHITE);
}

#[test]
fn render_jbig2_text_with_globals() {
    // The symbol dictionary in a /JBIG2Globals stream (itself Flate-wrapped), the text region in the image.
    let globals = common::zlib_stored(&seg(1, 0, &[], &symbol_dict_data(0)));
    let mut stream = seg(2, 48, &[], &page_info(32, 16, 0));
    stream.extend(seg(3, 6, &[1], &text_region_data(TOPLEFT)));
    let r = render(
        "/XObject << /Im1 5 0 R >>",
        "q 32 0 0 16 4 4 cm /Im1 Do Q",
        &[
            (5, "/Type /XObject /Subtype /Image /Width 32 /Height 16 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /JBIG2Decode /DecodeParms << /JBIG2Globals 6 0 R >>", stream),
            (6, "/Filter /FlateDecode", globals),
        ],
    );
    assert!(r.refused.is_empty(), "{:?}", r.refused);
    let want = text_expected();
    // Image (x, y) is device (4 + x, 20 + y).
    for y in 0..16 {
        for x in 0..32 {
            let p = if want.data[y * 32 + x] == 1 { BLACK } else { WHITE };
            assert_eq!(px(&r, 4 + x, 20 + y), p, "image pixel ({x},{y})");
        }
    }
}

#[test]
fn render_jbig2_huffman_refused_grey_box() {
    let mut s = seg(0, 48, &[], &page_info(32, 16, 0));
    s.extend(seg(1, 0, &[], &symbol_dict_data(1)));
    let r = render(
        "/XObject << /Im1 5 0 R >>",
        "q 10 0 0 10 10 10 cm /Im1 Do Q",
        &[(5, "/Type /XObject /Subtype /Image /Width 32 /Height 16 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /JBIG2Decode", s)],
    );
    assert_eq!(px(&r, 15, 25), [128, 128, 128, 255]);
    assert_eq!(px(&r, 25, 25), WHITE);
    assert_eq!(r.refused, vec!["JBIG2 Huffman"]);
}
