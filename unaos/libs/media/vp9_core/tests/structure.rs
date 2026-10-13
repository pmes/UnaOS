// SPDX-License-Identifier: LGPL-3.0-or-later
//! VP9CORE decode-structure KATs over hand-built streams (no reference encoder exists on the build
//! host, so these are built here bit by bit with a boolean ENCODER written from §9.2's inverse):
//! an 8x8 key frame whose one block is DC_PRED + skip must reconstruct to mid-grey (128 at 8 bit,
//! 512 at 10 bit, the "no edges" DC of §8.5.1); show_existing_frame re-presents a slot; a superframe
//! presents only its last frame; profiles 1 and 3 are refused by name; the uncompressed header's
//! fields land where §6.2 puts them.

use vp9_core::header::{parse_uncompressed, HeaderState};
use vp9_core::{superframe_frames, Decoder, Error};

struct BitWriter {
    out: Vec<u8>,
    n: usize,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter { out: Vec::new(), n: 0 }
    }
    fn bit(&mut self, b: u32) {
        if self.n % 8 == 0 {
            self.out.push(0);
        }
        if b != 0 {
            *self.out.last_mut().unwrap() |= 0x80 >> (self.n % 8);
        }
        self.n += 1;
    }
    fn f(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.bit((v >> i) & 1);
        }
    }
}

/// The boolean encoder (the inverse of §9.2's read_bool, as libvpx's vpx_writer).
struct BoolEncoder {
    low: u32,
    range: u32,
    count: i32,
    buf: Vec<u8>,
}

impl BoolEncoder {
    fn new() -> Self {
        let mut e = BoolEncoder { low: 0, range: 255, count: -24, buf: Vec::new() };
        e.write(false, 128); // marker bit
        e
    }
    fn write(&mut self, bit: bool, prob: u8) {
        let split = 1 + (((self.range - 1) * prob as u32) >> 8);
        let mut range = split;
        let mut low = self.low;
        if bit {
            low += split;
            range = self.range - split;
        }
        let mut shift = (range as u8).leading_zeros() as i32;
        range <<= shift;
        let mut count = self.count + shift;
        if count >= 0 {
            let offset = shift - count;
            if (low << (offset - 1)) & 0x8000_0000 != 0 {
                let mut x = self.buf.len() as isize - 1;
                while x >= 0 && self.buf[x as usize] == 0xff {
                    self.buf[x as usize] = 0;
                    x -= 1;
                }
                self.buf[x as usize] += 1;
            }
            self.buf.push((low >> (24 - offset)) as u8);
            low <<= offset;
            shift = count;
            low &= 0xff_ffff;
            count -= 8;
        }
        low <<= shift;
        self.count = count;
        self.low = low;
        self.range = range;
    }
    fn literal(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.write((v >> i) & 1 != 0, 128);
        }
    }
    fn finish(mut self) -> Vec<u8> {
        for _ in 0..32 {
            self.write(false, 128);
        }
        if self.buf.last().is_some_and(|b| b & 0xe0 == 0xc0) {
            self.buf.push(0);
        }
        self.buf
    }
}

/// An 8x8 key frame (profile 0 at 8 bit, or profile 2 at 10 bit): one 8x8 block, PARTITION_NONE,
/// skip, DC_PRED luma and chroma, ONLY_4X4, no probability updates, loop filter level 0.
fn key_frame_8x8(profile2: bool) -> Vec<u8> {
    // Compressed header: tx_mode = ONLY_4X4 (two literal bits), no coefficient update (one bit),
    // three skip-probability "no update" flags.
    let mut c = BoolEncoder::new();
    c.literal(0, 2);
    c.write(false, 128);
    for _ in 0..3 {
        c.write(false, 252);
    }
    let comp = c.finish();
    // Tile: partition at the 8x8 level (ctx 0) = NONE, skip (ctx 0, p 192) = 1, y mode DC
    // (kf probs [DC][DC], first node 137) and uv mode DC (kf_uv[DC], first node 144).
    let mut t = BoolEncoder::new();
    t.write(false, 158);
    t.write(true, 192);
    t.write(false, 137);
    t.write(false, 144);
    let tile = t.finish();

    let mut w = BitWriter::new();
    w.f(2, 2); // frame_marker
    if profile2 {
        w.bit(0); // profile low bit
        w.bit(1); // profile high bit
    } else {
        w.bit(0);
        w.bit(0);
    }
    w.bit(0); // show_existing_frame
    w.bit(0); // frame_type = KEY_FRAME
    w.bit(1); // show_frame
    w.bit(0); // error_resilient_mode
    w.f(0x49, 8);
    w.f(0x83, 8);
    w.f(0x42, 8);
    if profile2 {
        w.bit(0); // ten_or_twelve_bit = 10
    }
    w.f(1, 3); // color_space BT.601
    w.bit(0); // color_range studio
    w.f(7, 16); // width - 1
    w.f(7, 16); // height - 1
    w.bit(0); // render_and_frame_size_different
    w.bit(1); // refresh_frame_context
    w.bit(0); // frame_parallel_decoding_mode
    w.f(0, 2); // frame_context_idx
    w.f(0, 6); // loop_filter_level
    w.f(0, 3); // sharpness
    w.bit(0); // mode_ref_delta_enabled
    w.f(60, 8); // base_q_idx
    w.bit(0);
    w.bit(0);
    w.bit(0); // no delta_q
    w.bit(0); // segmentation_enabled
    // tile_info: one 64x64 column -> no tile-column bits; tile_rows_log2 = 0.
    w.bit(0);
    w.f(comp.len() as u32, 16);
    let mut out = w.out;
    out.extend_from_slice(&comp);
    out.extend_from_slice(&tile);
    out
}

#[test]
fn uncompressed_header_fields() {
    let f = key_frame_8x8(false);
    let mut st = HeaderState::default();
    let h = parse_uncompressed(&f, &mut st).unwrap();
    assert!(h.key_frame && h.show_frame && !h.error_resilient);
    assert_eq!((h.width, h.height, h.render_width, h.render_height), (8, 8, 8, 8));
    assert_eq!(h.refresh_frame_flags, 0xff);
    assert_eq!(h.base_q_idx, 60);
    assert!(!h.lossless);
    assert_eq!((h.tile_cols_log2, h.tile_rows_log2), (0, 0));
    assert_eq!((st.bit_depth, st.ss_x, st.ss_y, st.color_space), (8, 1, 1, 1));
    assert!(h.past_independence);
    assert_eq!(st.lf.ref_deltas, [1, 0, -1, -1]);
    let h2 = parse_uncompressed(&key_frame_8x8(true), &mut st).unwrap();
    assert_eq!(h2.profile, 2);
    assert_eq!(st.bit_depth, 10);
}

#[test]
fn dc_key_frame_is_mid_grey() {
    let mut d = Decoder::new();
    let f = d.decode(&key_frame_8x8(false)).unwrap().expect("shown");
    assert_eq!((f.width, f.height, f.bit_depth), (8, 8, 8));
    assert_eq!(f.plane_size(1), (4, 4));
    assert!(f.planes[0].iter().all(|&v| v == 128), "{:?}", &f.planes[0][..8]);
    assert!(f.planes[1].iter().chain(&f.planes[2]).all(|&v| v == 128));
    assert_eq!(f.md5_layout().len(), 64 + 16 + 16);
}

#[test]
fn dc_key_frame_10bit_is_mid_grey() {
    let mut d = Decoder::new();
    let f = d.decode(&key_frame_8x8(true)).unwrap().expect("shown");
    assert_eq!(f.bit_depth, 10);
    assert!(f.planes.iter().flatten().all(|&v| v == 512));
    assert_eq!(f.md5_layout().len(), 2 * (64 + 16 + 16));
    assert!(f.to_8bit()[0].iter().all(|&v| v == 128));
}

/// show_existing_frame: one byte — marker 10, profile 00, show_existing 1, slot 000.
#[test]
fn show_existing_frame_represents_a_slot() {
    let mut d = Decoder::new();
    let a = d.decode(&key_frame_8x8(false)).unwrap().unwrap();
    let b = d.decode(&[0b1000_1000]).unwrap().expect("re-presented");
    assert_eq!(a, b);
    // An empty slot cannot be shown.
    assert!(matches!(Decoder::new().decode(&[0b1000_1000]), Err(Error::Invalid(_))));
}

/// Annex B: [key frame][show_existing] + index presents only the last frame.
#[test]
fn superframe_index_splits_and_presents_the_last() {
    let k = key_frame_8x8(false);
    let s = [0b1000_1000u8];
    let mut chunk = k.clone();
    chunk.extend_from_slice(&s);
    // marker: 0b110 mm fff with mag = 2 bytes (mm = 1), frames = 2 (fff = 1).
    let marker = 0xc0 | (1 << 3) | 1;
    chunk.push(marker);
    chunk.extend_from_slice(&(k.len() as u16).to_le_bytes());
    chunk.extend_from_slice(&(s.len() as u16).to_le_bytes());
    chunk.push(marker);
    let frames = superframe_frames(&chunk);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0], &k[..]);
    assert_eq!(frames[1], &s[..]);
    let mut d = Decoder::new();
    let f = d.decode(&chunk).unwrap().expect("the last frame is shown");
    assert!(f.planes[0].iter().all(|&v| v == 128));
    // Without an index the chunk is one frame.
    assert_eq!(superframe_frames(&k).len(), 1);
}

#[test]
fn profiles_1_and_3_are_refused_by_name() {
    // marker 10, profile low 1, high 0 -> profile 1.
    for byte in [0b1010_0000u8, 0b1011_0000] {
        match parse_uncompressed(&[byte, 0, 0, 0], &mut HeaderState::default()) {
            Err(Error::Unsupported(m)) => assert!(m.contains("profile 1/3")),
            other => panic!("{byte:08b}: {other:?}"),
        }
    }
}

#[test]
fn inter_frame_first_is_refused() {
    // marker 10, profile 0, show_existing 0, frame_type 1 (inter), show 1, error_res 0, ...
    let r = Decoder::new().decode(&[0b1000_0110, 0, 0, 0, 0, 0, 0, 0]);
    assert!(r.is_err());
}
